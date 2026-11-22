//! The session socket's writer task (`ARCH-038`, M-review change 9, M2-QA-1/2).
//!
//! A WebSocket message can't be interleaved with another, and `send` flushes, so a camera
//! frame sent inline on a congested uplink would hold the transport's whole `select!` loop:
//! incoming video, `kd`/`ku`, pointer motion, gamepad heartbeats and the microphone. So the
//! socket is split:
//! - the transport's loop keeps the read half and only *queues* writes;
//! - a writer task owns the write half and serves, in strict priority, the **control queue**
//!   (every text command and the microphone's `0x02` frames, in order) and then a small
//!   **camera queue**.
//!
//! **Input latency wins over camera quality** (PM, M2). The writer reads the socket's send
//! side (`TCP_CONNECTION_INFO`): the bytes not yet sent, which is what input would wait
//! behind, apart from bytes in flight, which are only round-trip time (R2-1), and the RTT,
//! whose rise above its minimum shows a queue at a bloated bottleneck. From how fast bytes are
//! acknowledged while the link is the limit it measures the uplink, and tells the camera,
//! through its [`Feedback`], the bitrate to encode at (down on a queue, back up slowly) and
//! the largest frame the link carries in 1/8 s. A camera frame starts only when the unsent
//! bytes plus the frame fit about 100 ms of the link, or nothing waits (so a capped keyframe
//! still goes), so input waits for at most about one capped frame.
//!
//! Frames that wait too long (250 ms), or overflow the queue, are dropped together; the
//! camera is asked for a keyframe and frames are skipped until one arrives (a P frame after
//! a gap can't decode). A keyframe still queued or being written holds back further forced
//! keyframes ([`Feedback::take_keyframe_request`]).
//!
//! **Bounded**: the transport takes a microphone frame only while fewer than [`MIC_ROOM`] are
//! queued here, and a camera frame only while the camera queue has room. A stalled writer
//! therefore leaves the capture threads' own channels full, and their 2 s stall stop works
//! (M-QA-2).

use crate::camera::{Feedback, START_FRAME_CAP};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use std::collections::VecDeque;
use std::os::fd::RawFd;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

/// Microphone frames the writer may hold: 200 ms.
pub const MIC_ROOM: usize = 10;
/// Camera frames the writer may hold.
pub const CAMERA_ROOM: usize = 6;
/// A camera frame older than this is dropped with everything behind it.
const CAMERA_MAX_AGE: Duration = Duration::from_millis(250);
/// A backlog this small counts as an idle link: any frame may start.
const IDLE: usize = 2 * 1024;
/// A backlog above this means the link is saturated, so its drain rate is the link's.
const SATURATED: usize = 4 * 1024;
/// How much of the link a camera frame may queue behind: input waits at most about this.
const QUEUE_TARGET: f64 = 0.1;
/// The largest frame's share of a second of the link (the keyframe cap).
const FRAME_SHARE: f64 = 0.125;
/// The budget before the link is measured: 1 Mbps × 100 ms.
const DEFAULT_BUDGET: usize = 12 * 1024;
/// How long a close waits for the writer to drain and send the close frame.
const CLOSE_WAIT: Duration = Duration::from_secs(2);
/// WebSocket framing per client message (header and mask), for the byte count.
const FRAMING: u64 = 8;

/// What the socket's send side holds now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SendState {
    /// Bytes in the send buffer: unsent, or sent and not yet acknowledged.
    pub buffered: usize,
    /// Bytes not yet sent: what a new frame (or a `kd`) would wait behind.
    pub unsent: usize,
    /// The smoothed round-trip time, when the socket knows it.
    pub srtt: Option<Duration>,
}

/// How the socket's send side stands.
pub trait Backlog: Send + 'static {
    /// `None` when the socket can't say: camera frames are then never held back.
    fn state(&mut self) -> Option<SendState>;
}

/// No socket to ask.
pub struct NoBacklog;

impl Backlog for NoBacklog {
    fn state(&mut self) -> Option<SendState> {
        None
    }
}

/// `<netinet/tcp.h>`: `TCP_CONNECTION_INFO` and its `struct tcp_connection_info` (not in
/// `libc`), laid out as the SDK declares it.
const TCP_CONNECTION_INFO: libc::c_int = 0x106;

#[repr(C)]
#[derive(Default)]
struct TcpConnectionInfo {
    state: u8,
    snd_wscale: u8,
    rcv_wscale: u8,
    pad1: u8,
    options: u32,
    flags: u32,
    rto: u32,
    maxseg: u32,
    snd_ssthresh: u32,
    snd_cwnd: u32,
    snd_wnd: u32,
    snd_sbbytes: u32,
    rcv_wnd: u32,
    rttcur: u32,
    srtt: u32,
    rttvar: u32,
    tfo: u32,
    txpackets: u64,
    txbytes: u64,
    txretransmitbytes: u64,
    rxpackets: u64,
    rxbytes: u64,
    rxoutoforderbytes: u64,
    txretransmitpackets: u64,
}

/// The TCP socket's own view, from `TCP_CONNECTION_INFO`: the bytes in its send buffer, and
/// the RTT. TCP keeps at most `min(cwnd, send window)` bytes in flight, so what the buffer
/// holds beyond that is unsent (R2-1: in-flight bytes are round-trip time, not congestion).
pub struct SocketBacklog(RawFd);

impl SocketBacklog {
    /// `None` if the socket doesn't answer `TCP_CONNECTION_INFO`.
    pub fn new(fd: RawFd) -> Option<Self> {
        let mut backlog = Self(fd);
        backlog.state().map(|_| backlog)
    }
}

impl Backlog for SocketBacklog {
    fn state(&mut self) -> Option<SendState> {
        let mut info = TcpConnectionInfo::default();
        let mut len = size_of::<TcpConnectionInfo>() as libc::socklen_t;
        // SAFETY: a live socket fd (the writer owns half of the socket) and an out struct of
        // the size passed.
        let status = unsafe {
            libc::getsockopt(
                self.0,
                libc::IPPROTO_TCP,
                TCP_CONNECTION_INFO,
                (&mut info as *mut TcpConnectionInfo).cast(),
                &mut len,
            )
        };
        if status != 0 {
            return None;
        }
        let buffered = info.snd_sbbytes as usize;
        let flight = info.snd_cwnd.min(info.snd_wnd) as usize;
        Some(SendState {
            buffered,
            unsent: buffered.saturating_sub(flight),
            srtt: (info.srtt > 0).then(|| Duration::from_millis(u64::from(info.srtt))),
        })
    }
}

/// The socket's fd under any TLS layer, for [`SocketBacklog`].
pub fn socket_fd<S: std::os::fd::AsRawFd>(
    stream: &tokio_tungstenite::MaybeTlsStream<S>,
) -> Option<RawFd> {
    match stream {
        tokio_tungstenite::MaybeTlsStream::Plain(tcp) => Some(tcp.as_raw_fd()),
        tokio_tungstenite::MaybeTlsStream::Rustls(tls) => Some(tls.get_ref().0.as_raw_fd()),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

/// The uplink as measured, and the camera's rate decisions.
#[derive(Debug)]
struct Meter {
    /// Bytes handed to the socket so far.
    written: u64,
    /// The last sample: when, and how many bytes had been acknowledged by then.
    sample: Option<(Instant, u64)>,
    /// Bytes per second the link carried while it, not the camera, was the limit.
    rate: Option<f64>,
    /// The lowest RTT seen: the path's own, without queueing.
    min_rtt: Option<Duration>,
    /// Since when bytes have waited unsent: the link, not the camera, is the limit.
    limited_since: Option<Instant>,
    /// When the meter started: the calm before the first trouble counts from here.
    start: Instant,
    last_down: Option<Instant>,
    last_up: Instant,
}

impl Meter {
    fn new(now: Instant) -> Self {
        Self {
            written: 0,
            sample: None,
            rate: None,
            min_rtt: None,
            limited_since: None,
            start: now,
            last_down: None,
            last_up: now,
        }
    }

    fn wrote(&mut self, bytes: usize) {
        self.written += bytes as u64 + FRAMING;
    }

    /// The link's measured rate (bytes/s), once it has been the limit.
    #[cfg(test)]
    fn rate(&self) -> Option<f64> {
        self.rate
    }

    /// Queueing beyond the path's own RTT (a bloated bottleneck TCP has filled).
    fn queue_delay(&self, state: SendState) -> Duration {
        match (state.srtt, self.min_rtt) {
            (Some(srtt), Some(min)) => srtt.saturating_sub(min),
            _ => Duration::ZERO,
        }
    }

    /// Whether a camera frame of `len` may start: the unsent bytes plus the frame fit about
    /// 100 ms of the link, or nothing waits (so a capped keyframe still goes). Bytes in
    /// flight don't count: they hold up nothing sent after them.
    fn admits(&self, state: Option<SendState>, len: usize) -> bool {
        let Some(state) = state else {
            return true;
        };
        if self.queue_delay(state) > Duration::from_secs_f64(QUEUE_TARGET * 1.5) {
            return false;
        }
        if state.unsent <= IDLE {
            return true;
        }
        let budget = self
            .rate
            .map_or(DEFAULT_BUDGET, |rate| (rate * QUEUE_TARGET) as usize)
            .max(IDLE);
        state.unsent + len <= budget
    }

    /// A periodic look at the socket: updates the rate, then the camera's bitrate (down at
    /// once on a queue or drops, up 10% every 500 ms after 2 s without trouble) and frame cap.
    fn observe(
        &mut self,
        now: Instant,
        state: Option<SendState>,
        dropped: bool,
        feedback: &Feedback,
    ) {
        // The frame guard cut the bitrate itself: that counts as trouble for the climb.
        if feedback.take_cut() {
            self.last_down = Some(now);
        }
        let Some(state) = state else {
            return;
        };
        if let Some(srtt) = state.srtt {
            self.min_rtt = Some(self.min_rtt.map_or(srtt, |min| min.min(srtt)));
        }
        let acked = self.written.saturating_sub(state.buffered as u64);
        if state.unsent > SATURATED {
            self.limited_since.get_or_insert(now);
        } else {
            self.limited_since = None;
        }
        // Acknowledgements trail sends by an RTT, so a window shorter than that measures the
        // RTT, not the link.
        let window = state
            .srtt
            .unwrap_or_default()
            .max(Duration::from_millis(100));
        match self.sample {
            Some((at, before)) if now.duration_since(at) >= window => {
                let seconds = now.duration_since(at).as_secs_f64();
                let measured = acked.saturating_sub(before) as f64 / seconds;
                let limited = self.limited_since.is_some_and(|since| since <= at);
                if limited {
                    // Bytes waited to be sent the whole window: the link was the limit, so
                    // this is its rate.
                    self.rate = Some(match self.rate {
                        Some(rate) => rate * 0.7 + measured * 0.3,
                        None => measured,
                    });
                } else if self.rate.is_some_and(|rate| measured > rate) {
                    // Not the limit and faster than thought: the link got better.
                    self.rate = Some(measured);
                }
                self.sample = Some((now, acked));
            }
            Some(_) => {}
            None => self.sample = Some((now, acked)),
        }
        let limit = Duration::from_secs_f64(QUEUE_TARGET * 1.5);
        let unsent_wait = self
            .rate
            .filter(|rate| *rate > 0.0)
            .map_or(Duration::ZERO, |rate| {
                Duration::from_secs_f64(state.unsent as f64 / rate)
            });
        let queued = unsent_wait > limit || self.queue_delay(state) > limit;
        let bitrate = feedback.bitrate();
        let calm_since = self.last_down.unwrap_or(self.start);
        if (dropped || queued)
            && self
                .last_down
                .is_none_or(|at| now.duration_since(at) >= Duration::from_millis(300))
        {
            let mut next = bitrate / 10 * 7;
            if let Some(rate) = self.rate {
                next = next.min((rate * 8.0 * 0.6) as u32);
            }
            feedback.set_bitrate(next);
            self.last_down = Some(now);
        } else if !dropped
            && !queued
            && now.duration_since(calm_since) >= Duration::from_secs(2)
            && now.duration_since(self.last_up) >= Duration::from_millis(500)
        {
            feedback.set_bitrate(bitrate / 10 * 11);
            self.last_up = now;
        }
        let cap = match self.rate {
            Some(rate) => rate * FRAME_SHARE,
            // Never the limit: at least half a second of the camera's own bitrate.
            None => (f64::from(feedback.bitrate()) / 16.0).max(f64::from(START_FRAME_CAP)),
        };
        feedback.set_frame_cap((cap as u32).clamp(4 * 1024, 64 * 1024));
    }
}

/// The camera frames waiting for the writer, oldest first.
#[derive(Default)]
struct CameraQueue {
    frames: VecDeque<(Instant, Vec<u8>)>,
    /// A frame was dropped: skip until a keyframe.
    gap: bool,
    /// Drops since the writer last looked.
    drops: usize,
    /// A keyframe is being written.
    writing_key: bool,
}

impl CameraQueue {
    fn key_unsent(&self) -> bool {
        self.writing_key
            || self
                .frames
                .iter()
                .any(|(_, frame)| crate::capture::webcam_keyframe(frame))
    }

    fn drop_all(&mut self, feedback: &Feedback) {
        self.frames.clear();
        self.gap = true;
        self.drops += 1;
        feedback.request_keyframe();
    }

    fn push(&mut self, frame: Vec<u8>, now: Instant, feedback: &Feedback) {
        let key = crate::capture::webcam_keyframe(&frame);
        if self.frames.len() >= CAMERA_ROOM {
            self.drop_all(feedback);
        }
        if self.gap && !key {
            feedback.request_keyframe();
        } else {
            self.gap = false;
            self.frames.push_back((now, frame));
        }
        feedback.set_key_unsent(self.key_unsent());
    }

    /// Drops everything once the oldest frame waited too long.
    fn expire(&mut self, now: Instant, feedback: &Feedback) {
        if self
            .frames
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > CAMERA_MAX_AGE)
        {
            self.drop_all(feedback);
            feedback.set_key_unsent(self.key_unsent());
        }
    }
}

enum Out {
    Message(Message),
    Flush,
}

/// What the socket's two halves share.
struct Shared {
    camera: Mutex<CameraQueue>,
    camera_ready: Notify,
    feedback: Arc<Feedback>,
    /// Microphone frames queued for the writer and not yet written.
    mic_queued: AtomicUsize,
    /// Why the writer stopped, for the next send.
    error: Mutex<Option<WsError>>,
}

impl Shared {
    fn take_drops(&self) -> bool {
        self.camera
            .lock()
            .is_ok_and(|mut queue| std::mem::take(&mut queue.drops) > 0)
    }
}

/// Writes queued camera frames while the link has room and no control frame waits.
async fn write_camera<S>(
    sink: &mut SplitSink<S, Message>,
    control: &mpsc::UnboundedReceiver<Out>,
    shared: &Shared,
    backlog: &mut dyn Backlog,
    meter: &mut Meter,
) -> Result<(), WsError>
where
    S: Sink<Message, Error = WsError> + Unpin,
{
    while control.is_empty() {
        let frame = {
            let Ok(mut queue) = shared.camera.lock() else {
                return Ok(());
            };
            queue.expire(Instant::now(), &shared.feedback);
            let Some((_, head)) = queue.frames.front() else {
                return Ok(());
            };
            if !meter.admits(backlog.state(), head.len()) {
                return Ok(());
            }
            let Some((_, frame)) = queue.frames.pop_front() else {
                return Ok(());
            };
            queue.writing_key = crate::capture::webcam_keyframe(&frame);
            frame
        };
        let len = frame.len();
        let sent = sink.send(Message::Binary(frame.into())).await;
        meter.wrote(len);
        if let Ok(mut queue) = shared.camera.lock() {
            queue.writing_key = false;
            shared.feedback.set_key_unsent(queue.key_unsent());
        }
        sent?;
    }
    Ok(())
}

async fn write_loop<S>(
    mut sink: SplitSink<S, Message>,
    mut control: mpsc::UnboundedReceiver<Out>,
    shared: Arc<Shared>,
    mut backlog: Box<dyn Backlog>,
) -> Result<(), WsError>
where
    S: Sink<Message, Error = WsError> + Unpin,
{
    let mut meter = Meter::new(Instant::now());
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            out = control.recv() => match out {
                Some(Out::Message(message)) => {
                    let close = matches!(message, Message::Close(_));
                    let binary = matches!(message, Message::Binary(_));
                    let len = message.len();
                    let sent = sink.send(message).await;
                    meter.wrote(len);
                    if binary {
                        shared.mic_queued.fetch_sub(1, Ordering::AcqRel);
                    }
                    sent?;
                    if close {
                        return sink.close().await;
                    }
                }
                Some(Out::Flush) => sink.flush().await?,
                None => return Ok(()),
            },
            _ = shared.camera_ready.notified() => {}
            _ = tick.tick() => {
                let dropped = shared.take_drops();
                meter.observe(Instant::now(), backlog.state(), dropped, &shared.feedback);
            }
        }
        write_camera(&mut sink, &control, &shared, &mut *backlog, &mut meter).await?;
    }
}

/// The session socket, split: reads here, writes queued for the writer task.
pub struct SplitSocket<S> {
    reader: SplitStream<S>,
    control: mpsc::UnboundedSender<Out>,
    shared: Arc<Shared>,
    writer: Option<tokio::task::JoinHandle<Result<(), ()>>>,
}

impl<S> SplitSocket<S>
where
    S: Stream<Item = Result<Message, WsError>> + Sink<Message, Error = WsError> + Unpin,
    S: Send + 'static,
{
    /// Splits `socket` and starts its writer. `feedback` is the camera's (drops, bitrate,
    /// frame cap); `backlog` measures the link.
    pub fn new(socket: S, feedback: Arc<Feedback>, backlog: Box<dyn Backlog>) -> Self {
        let (sink, reader) = socket.split();
        let (control, queued) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            camera: Mutex::new(CameraQueue::default()),
            camera_ready: Notify::new(),
            feedback,
            mic_queued: AtomicUsize::new(0),
            error: Mutex::new(None),
        });
        let task = shared.clone();
        let writer = tokio::spawn(async move {
            let result = write_loop(sink, queued, task.clone(), backlog).await;
            result.map_err(|error| {
                if let Ok(mut slot) = task.error.lock() {
                    *slot = Some(error);
                }
            })
        });
        Self {
            reader,
            control,
            shared,
            writer: Some(writer),
        }
    }
}

impl<S> SplitSocket<S> {
    /// The writer's own error if it failed, else a plain closed connection.
    fn stopped(&self) -> WsError {
        self.shared
            .error
            .lock()
            .ok()
            .and_then(|mut error| error.take())
            .unwrap_or(WsError::ConnectionClosed)
    }

    /// Queues `message` behind every earlier one; fails, with the writer's own error, once
    /// the writer has stopped.
    pub async fn send(&mut self, message: Message) -> Result<(), WsError> {
        let binary = matches!(message, Message::Binary(_));
        if binary {
            self.shared.mic_queued.fetch_add(1, Ordering::AcqRel);
        }
        if self.control.send(Out::Message(message)).is_err() {
            if binary {
                self.shared.mic_queued.fetch_sub(1, Ordering::AcqRel);
            }
            return Err(self.stopped());
        }
        Ok(())
    }

    /// Flushes what the read side queued (a pong).
    pub async fn flush(&mut self) -> Result<(), WsError> {
        self.control.send(Out::Flush).map_err(|_| self.stopped())
    }

    /// Whether the writer has room for another microphone frame.
    pub fn mic_room(&self) -> bool {
        self.shared.mic_queued.load(Ordering::Acquire) < MIC_ROOM
    }

    /// Whether the writer has room for another camera frame.
    pub fn camera_room(&self) -> bool {
        self.shared
            .camera
            .lock()
            .is_ok_and(|queue| queue.frames.len() < CAMERA_ROOM)
    }

    /// Queues a camera frame (see the module docs for when it goes, waits or is dropped).
    pub fn send_camera(&self, frame: Vec<u8>) {
        if let Ok(mut queue) = self.shared.camera.lock() {
            queue.push(frame, Instant::now(), &self.shared.feedback);
        }
        self.shared.camera_ready.notify_one();
    }

    /// Sends everything queued, then the close frame, and waits (briefly) for the writer.
    pub async fn close(&mut self, frame: Option<CloseFrame>) -> Result<(), WsError> {
        let _ = self.control.send(Out::Message(Message::Close(frame)));
        let Some(writer) = self.writer.take() else {
            return Ok(());
        };
        match tokio::time::timeout(CLOSE_WAIT, writer).await {
            Ok(Ok(Ok(()))) => Ok(()),
            _ => Err(self.stopped()),
        }
    }
}

impl<S> Drop for SplitSocket<S> {
    fn drop(&mut self) {
        // A stream that ends without a close: the writer goes with it.
        if let Some(writer) = self.writer.take() {
            writer.abort();
        }
    }
}

impl<S> Stream for SplitSocket<S>
where
    S: Stream<Item = Result<Message, WsError>> + Unpin,
{
    type Item = Result<Message, WsError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.reader.poll_next_unpin(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{FrameGuard, KeyframeGate, MAX_BITRATE, MIN_BITRATE, START_BITRATE};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    fn key(len: usize) -> Vec<u8> {
        crate::capture::webcam_frame(true, &vec![0; len])
    }

    fn delta(len: usize) -> Vec<u8> {
        crate::capture::webcam_frame(false, &vec![0; len])
    }

    #[test]
    fn a_dropped_frame_skips_to_the_next_keyframe() {
        let feedback = Feedback::default();
        let mut queue = CameraQueue::default();
        let start = Instant::now();
        queue.push(key(10), start, &feedback);
        assert!(feedback.key_unsent());
        queue.push(delta(10), start, &feedback);
        // Waited too long: everything goes, and a keyframe is asked for.
        queue.expire(start + Duration::from_millis(300), &feedback);
        assert!(queue.frames.is_empty() && queue.gap);
        assert!(feedback.keyframe_requested() && !feedback.key_unsent());
        // A P frame after the gap can't decode: skipped until the keyframe.
        queue.push(delta(10), start, &feedback);
        assert!(queue.frames.is_empty());
        queue.push(key(10), start, &feedback);
        queue.push(delta(10), start, &feedback);
        assert_eq!(queue.frames.len(), 2);
        // Overflow is a gap too.
        for _ in 0..CAMERA_ROOM {
            queue.push(delta(10), start, &feedback);
        }
        assert!(queue.frames.len() < CAMERA_ROOM && queue.gap);
        assert_eq!(queue.drops, 2);
    }

    fn unsent(bytes: usize) -> Option<SendState> {
        Some(SendState {
            buffered: bytes,
            unsent: bytes,
            srtt: None,
        })
    }

    #[test]
    fn a_frame_starts_only_within_100_ms_of_the_link() {
        let mut meter = Meter::new(Instant::now());
        // Unknown socket, or nothing waiting: always.
        assert!(meter.admits(None, 100_000));
        assert!(meter.admits(unsent(IDLE), 100_000));
        // 1 Mbps measured: 12.5 KB of room for unsent bytes.
        meter.rate = Some(125_000.0);
        assert!(meter.admits(unsent(4_000), 8_000));
        assert!(!meter.admits(unsent(4_000), 9_000));
        assert!(!meter.admits(unsent(13_000), 100));
        // Bytes in flight hold nothing up (R2-1).
        let flight = SendState {
            buffered: 200_000,
            unsent: 0,
            srtt: Some(Duration::from_millis(150)),
        };
        meter.min_rtt = Some(Duration::from_millis(150));
        assert!(meter.admits(Some(flight), 8_000));
        // But an RTT 200 ms above the path's own is a queue at the bottleneck.
        let bloated = SendState {
            srtt: Some(Duration::from_millis(350)),
            ..flight
        };
        assert!(!meter.admits(Some(bloated), 1_000));
    }

    #[test]
    fn the_bitrate_steps_down_on_backlog_and_back_up_slowly() {
        let feedback = Feedback::default();
        let start = Instant::now();
        let mut meter = Meter::new(start);
        // A 1 Mbps link held saturated: 12.5 KB acknowledged per 100 ms, 40 KB unsent.
        let mut at = start;
        meter.written = 40_000;
        meter.observe(at, unsent(40_000), false, &feedback);
        for _ in 0..10 {
            at += Duration::from_millis(100);
            meter.written += 12_500;
            meter.observe(at, unsent(40_000), false, &feedback);
        }
        let rate = meter.rate().unwrap();
        assert!((rate - 125_000.0).abs() < 2_000.0, "{rate}");
        assert!(feedback.bitrate() <= 600_000, "{}", feedback.bitrate());
        assert_eq!(feedback.frame_cap(), 15_625);
        // The queue clears: nothing for 2 s, then +10% every 500 ms.
        let low = feedback.bitrate();
        for step in 1..=40 {
            at += Duration::from_millis(100);
            meter.written += 2_000;
            meter.observe(at, unsent(0), false, &feedback);
            if step == 15 {
                assert_eq!(feedback.bitrate(), low, "not before 2 s");
            }
        }
        assert!(feedback.bitrate() > low && feedback.bitrate() < low * 2);
        // A drop steps down at once.
        let before = feedback.bitrate();
        at += Duration::from_millis(10);
        meter.observe(at, unsent(0), true, &feedback);
        assert!(feedback.bitrate() < before);
        assert!(feedback.bitrate() >= MIN_BITRATE);
        // The frame guard's own cut restarts the calm timer too (R2-2).
        for _ in 0..30 {
            at += Duration::from_millis(100);
            meter.observe(at, unsent(0), false, &feedback);
        }
        let climbed = feedback.bitrate();
        feedback.set_bitrate(climbed / 2);
        let cut = feedback.bitrate();
        feedback.note_cut();
        meter.observe(at + Duration::from_millis(100), unsent(0), false, &feedback);
        for step in 2..=15 {
            meter.observe(
                at + Duration::from_millis(100 * step),
                unsent(0),
                false,
                &feedback,
            );
        }
        assert_eq!(feedback.bitrate(), cut, "no climb within 2 s of a cut");
    }

    #[test]
    fn round_trip_time_alone_is_not_a_queue() {
        // R2-1: a 10 Mbps path with 150 ms RTT, the camera app-limited at 500 kbps: 9.4 KB
        // always in flight, nothing unsent, the RTT flat. The bitrate climbs.
        let feedback = Feedback::default();
        let start = Instant::now();
        let mut meter = Meter::new(start);
        let flight = SendState {
            buffered: 9_400,
            unsent: 0,
            srtt: Some(Duration::from_millis(150)),
        };
        for step in 0..100 {
            meter.written += 6_250;
            meter.observe(
                start + Duration::from_millis(100 * step),
                Some(flight),
                false,
                &feedback,
            );
        }
        assert!(feedback.bitrate() >= 1_000_000, "{}", feedback.bitrate());
    }

    /// A slow uplink between the client and the loopback socket: writes fill a send buffer
    /// of `capacity` bytes (the kernel's), which is sent at `rate` bytes per second; sent
    /// bytes reach the server half an RTT later and stay unacknowledged for a whole RTT.
    /// Loopback itself can't be slowed (macOS grows its buffers to megabytes), so this stands
    /// in for the kernel's socket, and [`LinkBacklog`] for `TCP_CONNECTION_INFO` on it.
    struct Link {
        read: tokio::net::tcp::OwnedReadHalf,
        state: Arc<Mutex<LinkState>>,
    }

    #[derive(Default)]
    struct LinkState {
        /// Unsent bytes.
        queue: VecDeque<u8>,
        /// Sent bytes waiting for their acknowledgement: (when it comes, how many).
        flight: VecDeque<(Instant, usize)>,
        capacity: usize,
        /// Bytes per second; 0 stalls the link.
        rate: f64,
        rtt: Duration,
        waker: Option<std::task::Waker>,
        closed: bool,
    }

    impl LinkState {
        fn in_flight(&self) -> usize {
            self.flight.iter().map(|(_, bytes)| bytes).sum()
        }
    }

    /// The link's send side, as `TCP_CONNECTION_INFO` would report it.
    struct LinkBacklog(Arc<Mutex<LinkState>>);

    impl Backlog for LinkBacklog {
        fn state(&mut self) -> Option<SendState> {
            let state = self.0.lock().unwrap();
            Some(SendState {
                buffered: state.queue.len() + state.in_flight(),
                unsent: state.queue.len(),
                srtt: Some(state.rtt.max(Duration::from_millis(1))),
            })
        }
    }

    impl Link {
        fn new(tcp: tokio::net::TcpStream, rate: f64, rtt: Duration, capacity: usize) -> Self {
            let state = Arc::new(Mutex::new(LinkState {
                capacity,
                rate,
                rtt,
                ..LinkState::default()
            }));
            let (read, mut write) = tcp.into_split();
            let drain = state.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt as _;
                let mut last = Instant::now();
                let mut credit = 0.0f64;
                // Sent bytes on their way: (when they arrive, the bytes).
                let mut wire: VecDeque<(Instant, Vec<u8>)> = VecDeque::new();
                loop {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    let now = Instant::now();
                    let finished = {
                        let mut state = drain.lock().unwrap();
                        credit += now.duration_since(last).as_secs_f64() * state.rate;
                        last = now;
                        let take = (credit as usize).min(state.queue.len());
                        if state.queue.is_empty() {
                            // An idle link saves up no burst.
                            credit = 0.0;
                        }
                        credit -= take as f64;
                        if take > 0 {
                            let chunk: Vec<u8> = state.queue.drain(..take).collect();
                            let rtt = state.rtt;
                            state.flight.push_back((now + rtt, take));
                            wire.push_back((now + rtt / 2, chunk));
                        }
                        while state.flight.front().is_some_and(|(at, _)| *at <= now) {
                            state.flight.pop_front();
                        }
                        if let Some(waker) = state.waker.take() {
                            waker.wake();
                        }
                        state.closed && state.queue.is_empty() && wire.is_empty()
                    };
                    while wire.front().is_some_and(|(at, _)| *at <= now) {
                        let (_, chunk) = wire.pop_front().unwrap();
                        if write.write_all(&chunk).await.is_err() {
                            return;
                        }
                    }
                    if finished {
                        break;
                    }
                }
                let _ = write.shutdown().await;
            });
            Self { read, state }
        }
    }

    impl AsyncRead for Link {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.read).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for Link {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            let mut state = self.state.lock().unwrap();
            let room = state.capacity.saturating_sub(state.queue.len());
            if room == 0 {
                state.waker = Some(cx.waker().clone());
                return Poll::Pending;
            }
            let take = room.min(buf.len());
            state.queue.extend(&buf[..take]);
            Poll::Ready(Ok(take))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            // Bytes in the send buffer count as flushed, as with a kernel socket.
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            self.state.lock().unwrap().closed = true;
            Poll::Ready(Ok(()))
        }
    }

    /// A server that records when each text frame arrives, and counts camera frames
    /// (keyframes, P frames).
    struct Server {
        address: std::net::SocketAddr,
        arrived: mpsc::UnboundedReceiver<(String, Instant)>,
        done: tokio::task::JoinHandle<(usize, usize)>,
    }

    async fn server() -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (arrived_tx, arrived) = mpsc::unbounded_channel::<(String, Instant)>();
        let done = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let (mut keys, mut deltas) = (0, 0);
            while let Some(Ok(message)) = socket.next().await {
                match message {
                    Message::Text(text) => {
                        let _ = arrived_tx.send((text.to_string(), Instant::now()));
                    }
                    Message::Binary(data) if crate::capture::webcam_keyframe(&data) => keys += 1,
                    Message::Binary(_) => deltas += 1,
                    _ => {}
                }
            }
            (keys, deltas)
        });
        Server {
            address,
            arrived,
            done,
        }
    }

    /// How a flood went.
    #[derive(Debug)]
    struct Flood {
        /// `kd` latencies beyond the link's own one-way delay (half the RTT), sorted.
        latencies: Vec<Duration>,
        offered: usize,
        keys: usize,
        deltas: usize,
        bitrate: u32,
    }

    impl Flood {
        fn p95(&self) -> Duration {
            self.latencies[(self.latencies.len() * 95).div_ceil(100) - 1]
        }
    }

    /// Floods a modelled uplink of `rate` bytes/s (64 KB send buffer) for 8 s with a camera
    /// that behaves like the VideoToolbox session: P frames at the bitrate the writer asks
    /// for, and keyframes of 60–120 KB at the old fixed 2.5 Mbps, scaled to that bitrate
    /// (the hardware encoder ignores `DataRateLimits` for keyframes). The real keyframe gate,
    /// frame guard and camera queue decide what goes. A `kd` every 100 ms after 2 s measures
    /// input latency.
    async fn flood(rate: f64, rtt: Duration, seconds: u64, start_bitrate: u32) -> Flood {
        let mut server = server().await;
        let tcp = tokio::net::TcpStream::connect(server.address)
            .await
            .unwrap();
        let link = Link::new(tcp, rate, rtt, 256 * 1024);
        let backlog = Box::new(LinkBacklog(link.state.clone()));
        let (socket, _) =
            tokio_tungstenite::client_async(format!("ws://{}/", server.address), link)
                .await
                .unwrap();
        let feedback = Arc::new(Feedback::default());
        feedback.set_bitrate(start_bitrate);
        let mut socket = SplitSocket::new(socket, feedback.clone(), backlog);
        let start = Instant::now();
        let mut gate = KeyframeGate::default();
        let mut guard = FrameGuard::default();
        let mut queued = Vec::new();
        let mut frame = 0usize;
        let mut offered = 0;
        let mut tick = tokio::time::interval(Duration::from_millis(33));
        while start.elapsed() < Duration::from_secs(seconds) {
            tick.tick().await;
            frame += 1;
            if feedback.take_keyframe_request() {
                gate.request();
            }
            let is_key = gate.take(Instant::now());
            let bitrate = feedback.bitrate() as usize;
            let jitter = 70 + (frame * 37) % 61;
            let len = if is_key {
                // 60, 90 or 120 KB at 2.5 Mbps.
                (60_000 + (frame % 3) * 30_000) * bitrate / 2_500_000
            } else {
                bitrate / 8 / 30 * jitter / 100
            };
            if guard.admit(is_key, len, &feedback) && socket.camera_room() {
                offered += 1;
                socket.send_camera(crate::capture::webcam_frame(is_key, &vec![0; len]));
            }
            if start.elapsed() > Duration::from_secs(2) && frame.is_multiple_of(3) {
                let text = format!("kd,{frame}");
                queued.push((text.clone(), Instant::now()));
                socket.send(text.into()).await.unwrap();
            }
        }
        let mut latencies = Vec::new();
        for (text, at) in &queued {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            loop {
                let (got, when) = tokio::time::timeout_at(deadline, server.arrived.recv())
                    .await
                    .expect("the kd arrives")
                    .unwrap();
                if &got == text {
                    latencies.push(when.duration_since(*at).saturating_sub(rtt / 2));
                    break;
                }
            }
        }
        latencies.sort();
        let bitrate = feedback.bitrate();
        let _ = socket.close(None).await;
        drop(socket);
        let (keys, deltas) = tokio::time::timeout(Duration::from_secs(20), server.done)
            .await
            .unwrap()
            .unwrap();
        Flood {
            latencies,
            offered,
            keys,
            deltas,
            bitrate,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn input_stays_within_150_ms_at_p95_on_a_1_mbps_uplink() {
        // From the starting 500 kbps, and from 1.5 Mbps (a link that just got slower, with
        // 36–72 KB keyframes): the writer steps the camera down to what the link carries.
        for start in [START_BITRATE, MAX_BITRATE] {
            let flood = flood(125_000.0, Duration::ZERO, 8, start).await;
            eprintln!(
                "1 Mbps from {start}: p95 {:?}, max {:?}, camera {} keys + {} P of {} offered, \
                 bitrate {}",
                flood.p95(),
                flood.latencies.last().unwrap(),
                flood.keys,
                flood.deltas,
                flood.offered,
                flood.bitrate
            );
            assert!(flood.latencies.len() >= 50);
            assert!(flood.p95() <= Duration::from_millis(150), "{flood:?}");
            // The camera keeps going, mostly P frames, at a bitrate the link carries.
            assert!(flood.deltas >= 120, "{flood:?}");
            assert!(flood.deltas > flood.keys * 10, "{flood:?}");
            assert!(flood.bitrate <= 1_000_000, "{flood:?}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_camera_never_falls_to_keyframes_only_on_a_3_mbps_uplink() {
        let flood = flood(375_000.0, Duration::ZERO, 8, START_BITRATE).await;
        eprintln!(
            "3 Mbps: p95 {:?}, max {:?}, camera {} keys + {} P of {} offered, bitrate {}",
            flood.p95(),
            flood.latencies.last().unwrap(),
            flood.keys,
            flood.deltas,
            flood.offered,
            flood.bitrate
        );
        assert!(flood.p95() <= Duration::from_millis(150), "{flood:?}");
        // About 30 fps of mostly P frames: 8 s offers about 240 frames.
        assert!(flood.deltas >= 180, "{flood:?}");
        assert!(flood.deltas > flood.keys * 10, "{flood:?}");
        assert!(flood.bitrate >= START_BITRATE / 2, "{flood:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_far_fast_link_carries_at_least_1_mbps_of_camera() {
        // R2-1: 10 Mbps with 150 ms RTT. In-flight bytes are RTT, not a queue.
        let flood = flood(1_250_000.0, Duration::from_millis(150), 12, START_BITRATE).await;
        eprintln!(
            "10 Mbps / 150 ms: p95 {:?} beyond RTT/2, max {:?}, camera {} keys + {} P of {} \
             offered, bitrate {}",
            flood.p95(),
            flood.latencies.last().unwrap(),
            flood.keys,
            flood.deltas,
            flood.offered,
            flood.bitrate
        );
        assert!(flood.bitrate >= 1_000_000, "{flood:?}");
        assert!(flood.p95() <= Duration::from_millis(150), "{flood:?}");
        assert!(flood.deltas > flood.keys * 10, "{flood:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn input_stays_within_150_ms_at_p95_on_a_far_1_mbps_uplink() {
        for start in [START_BITRATE, MAX_BITRATE] {
            let flood = flood(125_000.0, Duration::from_millis(150), 10, start).await;
            eprintln!(
                "1 Mbps / 150 ms from {start}: p95 {:?} beyond RTT/2, max {:?}, camera {} keys + \
                 {} P of {} offered, bitrate {}",
                flood.p95(),
                flood.latencies.last().unwrap(),
                flood.keys,
                flood.deltas,
                flood.offered,
                flood.bitrate
            );
            assert!(flood.p95() <= Duration::from_millis(150), "{flood:?}");
            assert!(flood.deltas >= 120, "{flood:?}");
            assert!(flood.bitrate <= 1_000_000, "{flood:?}");
        }
    }

    #[tokio::test]
    async fn a_stalled_writer_stops_taking_mic_and_camera_frames() {
        let server = server().await;
        let tcp = tokio::net::TcpStream::connect(server.address)
            .await
            .unwrap();
        let link = Link::new(tcp, 1e9, Duration::ZERO, 16 * 1024);
        let state = link.state.clone();
        let (socket, _) =
            tokio_tungstenite::client_async(format!("ws://{}/", server.address), link)
                .await
                .unwrap();
        let mut socket = SplitSocket::new(
            socket,
            Arc::new(Feedback::default()),
            Box::new(LinkBacklog(state.clone())),
        );
        // The uplink stops: nothing drains, and the send buffer fills.
        state.lock().unwrap().rate = 0.0;
        socket.send(vec![0u8; 20_000].into()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut taken = 0;
        while socket.mic_room() && taken < 100 {
            socket
                .send(Message::Binary(
                    crate::capture::mic_frame(&[0x68; 80]).into(),
                ))
                .await
                .unwrap();
            taken += 1;
        }
        assert_eq!(
            taken, MIC_ROOM,
            "the transport stops taking microphone frames"
        );
        let mut cameras = 0;
        while socket.camera_room() && cameras < 100 {
            socket.send_camera(key(1_000));
            cameras += 1;
        }
        assert_eq!(cameras, CAMERA_ROOM);
        // So the capture threads' own channels fill, and their 2 s stall stop fires. The
        // link comes back: room again.
        state.lock().unwrap().rate = 1e9;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(socket.mic_room());
    }

    #[tokio::test]
    async fn a_writer_error_keeps_its_cause() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            drop(socket);
        });
        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
            .await
            .unwrap();
        server.await.unwrap();
        let mut socket =
            SplitSocket::new(socket, Arc::new(Feedback::default()), Box::new(NoBacklog));
        let mut error = None;
        for _ in 0..200 {
            if let Err(cause) = socket.send(vec![0u8; 64 * 1024].into()).await {
                error = Some(cause);
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let error = error.expect("the dead peer fails a send");
        assert!(
            !matches!(error, WsError::ConnectionClosed),
            "the writer's own error, not a generic close: {error:?}"
        );
    }

    #[tokio::test]
    async fn the_backlog_reads_a_real_socket() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move { listener.accept().await.unwrap() });
        let tcp = tokio::net::TcpStream::connect(address).await.unwrap();
        let _server = accept.await.unwrap();
        use std::os::fd::AsRawFd;
        let mut backlog = SocketBacklog::new(tcp.as_raw_fd()).expect("TCP_CONNECTION_INFO");
        let state = backlog.state().unwrap();
        assert_eq!(
            (state.buffered, state.unsent),
            (0, 0),
            "an idle socket holds nothing"
        );
        assert_eq!(size_of::<TcpConnectionInfo>(), 112, "the SDK's layout");
        let stream = tokio_tungstenite::MaybeTlsStream::Plain(tcp);
        assert!(socket_fd(&stream).is_some());
    }

    #[tokio::test]
    async fn writes_keep_their_order_and_close_after_them() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut seen = Vec::new();
            while let Some(Ok(message)) = socket.next().await {
                match message {
                    Message::Text(text) => seen.push(text.to_string()),
                    Message::Binary(data) => seen.push(format!("bin {}", data.len())),
                    Message::Close(_) => seen.push("close".into()),
                    _ => {}
                }
            }
            seen
        });
        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
            .await
            .unwrap();
        let mut socket =
            SplitSocket::new(socket, Arc::new(Feedback::default()), Box::new(NoBacklog));
        for i in 0..50 {
            socket.send(format!("kd,{i}").into()).await.unwrap();
            if i == 10 {
                socket
                    .send(Message::Binary(crate::capture::mic_frame(&[1, 2]).into()))
                    .await
                    .unwrap();
            }
        }
        socket.close(None).await.unwrap();
        let seen = server.await.unwrap();
        assert_eq!(seen.len(), 52);
        assert_eq!(seen[11], "bin 3");
        assert_eq!(seen[12], "kd,11");
        assert_eq!(seen.last().unwrap(), "close");
        assert!(
            socket.mic_room(),
            "the written microphone frame left the count"
        );
        // Sending after the writer stopped fails, as a dead socket's send did.
        assert!(socket.send("kd,1".into()).await.is_err());
    }
}
