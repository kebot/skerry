//! Screencast: a stream of [`Frame`]s from a page Session (the raw material
//! for the Mirror).
//!
//! Only the Active Tab should stream: start a screencast when a Tab becomes
//! active and stop it when it stops being active. A Session carries at most
//! one screencast; a second [`Session::start_screencast`] fails with
//! [`CdpError::ScreencastActive`]. We never call
//! `Emulation.setDeviceMetricsOverride` (ADR 0002).
//!
//! Backpressure: Chrome sends the next Frame only after the previous one is
//! acknowledged, so the consumer decides the pace by calling
//! [`Frame::ack`] (typically after decoding the JPEG).

use std::sync::Mutex;

use base64::Engine;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::{CdpError, Session};

/// Options for `Page.startScreencast`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreencastOptions {
    /// JPEG quality 0..=100. Default 60.
    pub quality: u8,
    /// Maximum Frame width in device pixels. Default: unlimited.
    pub max_width: Option<u32>,
    /// Maximum Frame height in device pixels. Default: unlimited.
    pub max_height: Option<u32>,
    /// Send every Nth Frame. Default 1.
    pub every_nth_frame: u32,
}

impl Default for ScreencastOptions {
    fn default() -> Self {
        ScreencastOptions {
            quality: 60,
            max_width: None,
            max_height: None,
            every_nth_frame: 1,
        }
    }
}

/// `Page.screencastFrame` metadata. Sizes are CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameMetadata {
    pub device_width: f64,
    pub device_height: f64,
    pub offset_top: f64,
    pub page_scale_factor: f64,
    pub scroll_offset_x: f64,
    pub scroll_offset_y: f64,
    /// Seconds since the epoch, if the browser sent it.
    pub timestamp: Option<f64>,
}

/// One screencast Frame. Must be acknowledged with [`Frame::ack`] for the
/// browser to send more; dropping it without acking stalls the stream.
#[derive(Debug, Clone)]
pub struct Frame {
    /// JPEG bytes.
    pub data: Vec<u8>,
    /// The id to acknowledge (the event's `sessionId` field, which is a
    /// per-screencast frame counter and not the flat session id).
    pub frame_id: i64,
    pub metadata: FrameMetadata,
    session: Session,
}

impl Frame {
    /// Send `Page.screencastFrameAck` for this Frame. Call after decoding.
    pub async fn ack(&self) -> Result<(), CdpError> {
        self.session
            .send(
                "Page.screencastFrameAck",
                Some(json!({"sessionId": self.frame_id})),
            )
            .await
            .map(|_| ())
    }

    /// The flat session id this Frame came from.
    pub fn session_id(&self) -> &str {
        self.session.id()
    }
}

#[derive(Default, Debug)]
pub(crate) struct Slot {
    inner: Mutex<SlotInner>,
}

#[derive(Default, Debug)]
struct SlotInner {
    gen: u64,
    active: Option<(u64, AbortHandle)>,
}

/// Stream of [`Frame`]s. Dropping it stops delivery locally and frees the
/// Session slot, but cannot send `Page.stopScreencast` (use
/// [`Screencast::stop`] or [`Session::stop_screencast`] for that).
pub struct Screencast {
    rx: mpsc::Receiver<Frame>,
    session: Session,
    gen: u64,
}

impl Screencast {
    /// Next Frame, or `None` when the connection closed or the screencast
    /// was stopped.
    pub async fn next(&mut self) -> Option<Frame> {
        self.rx.recv().await
    }

    /// Stop the screencast in the browser.
    pub async fn stop(self) -> Result<(), CdpError> {
        self.session.stop_screencast().await
    }
}

impl Drop for Screencast {
    fn drop(&mut self) {
        let mut g = self.session.screencast.inner.lock().unwrap();
        if matches!(g.active, Some((gen, _)) if gen == self.gen) {
            if let Some((_, h)) = g.active.take() {
                h.abort();
            }
        }
    }
}

impl Session {
    /// `Page.startScreencast` (JPEG). Frames arrive on the returned stream.
    /// Fails with [`CdpError::ScreencastActive`] if one is already running.
    pub async fn start_screencast(&self, opts: ScreencastOptions) -> Result<Screencast, CdpError> {
        let mut events = self.events(); // subscribe before starting: no lost Frames
        let (tx, rx) = mpsc::channel(2);
        let gen = {
            let mut g = self.screencast.inner.lock().unwrap();
            if g.active.is_some() {
                return Err(CdpError::ScreencastActive);
            }
            g.gen += 1;
            let gen = g.gen;
            let session = self.clone();
            let task = tokio::spawn(async move {
                while let Some(ev) = events.recv().await {
                    if ev.method != "Page.screencastFrame" {
                        continue;
                    }
                    let Some(frame) = parse_frame(&ev.params, &session) else {
                        continue;
                    };
                    if tx.send(frame).await.is_err() {
                        break;
                    }
                }
            });
            g.active = Some((gen, task.abort_handle()));
            gen
        };

        let mut params = json!({
            "format": "jpeg",
            "quality": opts.quality,
            "everyNthFrame": opts.every_nth_frame.max(1),
        });
        if let Some(w) = opts.max_width {
            params["maxWidth"] = json!(w);
        }
        if let Some(h) = opts.max_height {
            params["maxHeight"] = json!(h);
        }
        if let Err(e) = self.send("Page.startScreencast", Some(params)).await {
            let mut g = self.screencast.inner.lock().unwrap();
            if matches!(g.active, Some((x, _)) if x == gen) {
                if let Some((_, h)) = g.active.take() {
                    h.abort();
                }
            }
            return Err(e);
        }
        Ok(Screencast {
            rx,
            session: self.clone(),
            gen,
        })
    }

    /// `Page.stopScreencast`; ends the local stream. Safe to call when no
    /// screencast is running.
    pub async fn stop_screencast(&self) -> Result<(), CdpError> {
        if let Some((_, h)) = self.screencast.inner.lock().unwrap().active.take() {
            h.abort();
        }
        self.send("Page.stopScreencast", None).await.map(|_| ())
    }
}

fn parse_frame(p: &Value, session: &Session) -> Option<Frame> {
    let b64 = p.get("data")?.as_str()?;
    let frame_id = p.get("sessionId")?.as_i64()?;
    let data = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let m = p.get("metadata").unwrap_or(&Value::Null);
    let f = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    Some(Frame {
        data,
        frame_id,
        metadata: FrameMetadata {
            device_width: f("deviceWidth"),
            device_height: f("deviceHeight"),
            offset_top: f("offsetTop"),
            page_scale_factor: f("pageScaleFactor"),
            scroll_offset_x: f("scrollOffsetX"),
            scroll_offset_y: f("scrollOffsetY"),
            timestamp: m.get("timestamp").and_then(Value::as_f64),
        },
        session: session.clone(),
    })
}
