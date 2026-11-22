//! Camera uplink (`ARCH-038`): capture on a per-session `skerry-camera` thread, H.264 through
//! a hardware `VTCompressionSession`, and `0x06` frames towards the session's socket.
//!
//! The thread model is `ARCH-030`'s: the `skerry-camera` thread owns the `AVCaptureSession`
//! and runs `startRunning` / `stopRunning`, never GPUI or Tokio. Frames arrive as NV12
//! `CVPixelBuffer`s on one serial dispatch queue, which is also the **only owner** of the
//! compression session: it creates, encodes, resizes and invalidates it (M-review change 8).
//! Encoded frames are converted from AVCC to Annex B, with SPS/PPS on every keyframe, and go
//! out as `0x06, 0x01, flags, payload`.
//!
//! A keyframe is sent on the first frame, every 4 s, on `WEBCAM_KEYFRAME`, and after any
//! frame dropped for back-pressure; forced ones at most every 500 ms, so drop → keyframe →
//! bigger frame → drop can't spiral (M-review change 9).
//!
//! The thread exists only while the camera captures: the window starts it after macOS
//! granted the permission and drops it to stop. Nothing lists or opens a camera before that.
//! It stops its own session as soon as the session's frame channel closes, or while the
//! channel has stayed full for 2 s, so the camera goes off even if the window misses the
//! stream's end. Dropping `Camera` never blocks: it sends `Shutdown` and detaches the thread.

mod native;

pub use native::list_cameras;

use crate::audio::{self, OutputDevice};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::error::TrySendError;

/// A camera as the menu shows it.
pub type CameraDevice = OutputDevice;

/// The longest gap between keyframes.
pub const KEYFRAME_EVERY: Duration = Duration::from_secs(4);
/// The shortest gap before a *forced* keyframe (a request or a drop).
pub const FORCED_KEYFRAME_GAP: Duration = Duration::from_millis(500);
/// The frame rate the camera is set to and frames are admitted at.
pub const FRAME_RATE: u32 = 30;
/// The encoder's average bitrate, as the Selkies web client's.
pub const BITRATE: u32 = 2_500_000;

/// How often the thread checks its session while capturing.
const CAPTURE_TICK: Duration = Duration::from_millis(100);
/// How often it wakes while not capturing.
const IDLE_TICK: Duration = Duration::from_millis(500);
const ENUMERATE_EVERY: Duration = Duration::from_secs(1);
/// A session running this long clears its device's strikes.
const HEALTHY_AFTER: Duration = Duration::from_secs(5);
/// A failed open waits at least this long.
const BUILD_RETRY: Duration = Duration::from_secs(2);
/// A frame channel full this long means the socket stalled: the camera closes.
const SINK_STALL: Duration = Duration::from_secs(2);

const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// One AVCC sample (length-prefixed NAL units) as Annex B, with `params` (SPS, PPS) first.
/// `None` for a truncated length or an unsupported prefix size.
pub fn avcc_to_annexb(
    sample: &[u8],
    nal_len: usize,
    params: Option<(&[u8], &[u8])>,
) -> Option<Vec<u8>> {
    if !(1..=4).contains(&nal_len) {
        return None;
    }
    let mut out = Vec::with_capacity(sample.len() + 64);
    if let Some((sps, pps)) = params {
        for set in [sps, pps] {
            out.extend_from_slice(&START_CODE);
            out.extend_from_slice(set);
        }
    }
    let mut rest = sample;
    while !rest.is_empty() {
        if rest.len() < nal_len {
            return None;
        }
        let len = rest[..nal_len]
            .iter()
            .fold(0usize, |len, byte| (len << 8) | usize::from(*byte));
        rest = &rest[nal_len..];
        if len > rest.len() {
            return None;
        }
        if len > 0 {
            out.extend_from_slice(&START_CODE);
            out.extend_from_slice(&rest[..len]);
        }
        rest = &rest[len..];
    }
    Some(out)
}

/// The device format to use for `want`: the smallest at least `want` that reaches 30 fps,
/// else the largest that reaches 30 fps, else the largest. `formats` are `(width, height,
/// max fps)`.
pub fn pick_format(formats: &[(u32, u32, f64)], want: (u32, u32)) -> Option<usize> {
    let area = |index: &usize| {
        let (width, height, _) = formats[*index];
        u64::from(width) * u64::from(height)
    };
    let fast = |index: &usize| formats[*index].2 >= f64::from(FRAME_RATE) - 0.5;
    let big_enough = |index: &usize| formats[*index].0 >= want.0 && formats[*index].1 >= want.1;
    let all = 0..formats.len();
    all.clone()
        .filter(|i| fast(i) && big_enough(i))
        .min_by_key(area)
        .or_else(|| all.clone().filter(fast).max_by_key(area))
        .or_else(|| all.max_by_key(area))
}

/// When the next frame must be a keyframe.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyframeGate {
    force: bool,
    last: Option<Instant>,
}

impl KeyframeGate {
    /// The server asked (`WEBCAM_KEYFRAME`), or a frame was dropped (the next ones can't
    /// decode until a keyframe). Both arrive through the uplink's keyframe flag.
    pub fn request(&mut self) {
        self.force = true;
    }

    /// Whether the frame at `now` is a keyframe: the first, one 4 s after the last, or a
    /// forced one at least 500 ms after the last (a forced request waits until then).
    pub fn take(&mut self, now: Instant) -> bool {
        let due = self.last.is_none_or(|last| {
            let since = now.saturating_duration_since(last);
            since >= KEYFRAME_EVERY || (self.force && since >= FORCED_KEYFRAME_GAP)
        });
        if due {
            self.last = Some(now);
            self.force = false;
        }
        due
    }
}

/// Where encoded frames go: the stream's camera channel, and its keyframe request flag,
/// which the window (`WEBCAM_KEYFRAME`) and the transport (a dropped frame) set too.
#[derive(Clone, Debug)]
pub struct Uplink {
    sink: tokio::sync::mpsc::Sender<Vec<u8>>,
    keyframe: Arc<AtomicBool>,
}

impl Uplink {
    pub fn new(sink: tokio::sync::mpsc::Sender<Vec<u8>>, keyframe: Arc<AtomicBool>) -> Self {
        Self { sink, keyframe }
    }

    /// Hands a frame on; a full channel drops it and asks for a keyframe. Returns false once
    /// the channel closed (the stream ended).
    pub fn send(&self, frame: Vec<u8>) -> bool {
        match self.sink.try_send(frame) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.keyframe.store(true, Ordering::Release);
                true
            }
            Err(TrySendError::Closed(_)) => false,
        }
    }

    /// Takes a pending keyframe request.
    pub fn take_keyframe_request(&self) -> bool {
        self.keyframe.swap(false, Ordering::AcqRel)
    }

    fn closed(&self) -> bool {
        self.sink.is_closed()
    }

    fn full(&self) -> bool {
        self.sink.capacity() == 0
    }
}

/// Why the camera isn't capturing where the person asked, when it isn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CamProblem {
    /// The chosen camera isn't connected; the default is used, if there is one.
    ChosenMissing,
    /// No camera at all.
    NoDevice,
    /// Another app holds the camera, or it was interrupted.
    InUse,
    /// The session stopped and retries keep failing.
    Failed,
    /// This Mac can't encode H.264.
    EncoderUnavailable,
}

/// What the camera thread reports, only when it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CamStatus {
    /// Connected cameras, sorted by name.
    pub devices: Vec<CameraDevice>,
    pub default_uid: Option<String>,
    /// The camera capturing now.
    pub active: Option<CameraDevice>,
    /// The choice this status answers; `None` is System Default.
    pub chosen: Option<String>,
    pub problem: Option<CamProblem>,
    /// A session is open: the camera is live, whether or not frames reach the session.
    pub live: bool,
}

impl CamStatus {
    pub fn device(&self, uid: &str) -> Option<&CameraDevice> {
        self.devices.iter().find(|device| device.uid == uid)
    }
}

/// How an open session is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Health {
    Running,
    /// Another app took it, or macOS interrupted it.
    InUse,
    Stopped,
}

/// AVFoundation behind a seam, so the loop runs in tests without a camera.
trait CamBackend {
    type Session;
    /// Connected cameras as `(unique ID, name)`; `None` when listing failed.
    fn devices(&mut self) -> Option<Vec<(String, Option<String>)>>;
    fn default_uid(&mut self) -> Option<String>;
    /// This Mac can make an H.264 compression session.
    fn encoder_ok(&mut self) -> bool;
    /// Builds and starts a session on `uid` near `size`, sending into `uplink`.
    fn open(
        &mut self,
        uid: &str,
        size: (u32, u32),
        uplink: Uplink,
    ) -> Result<Self::Session, Health>;
    fn health(&mut self, session: &Self::Session) -> Health;
}

enum CamControl {
    Choose(Option<String>),
    Capture(bool),
    Size(u32, u32),
    Shutdown,
}

/// The window's handle on its `skerry-camera` thread.
pub struct Camera {
    control: Sender<CamControl>,
    alive: Arc<AtomicBool>,
}

/// Clears the thread's `alive` flag when the thread ends, however it ends.
struct AliveGuard(Arc<AtomicBool>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Camera {
    /// Starts the thread, capturing at once: call it only once macOS granted the permission.
    /// `None` only if the thread can't start.
    pub fn new(
        chosen: Option<String>,
        size: (u32, u32),
        uplink: Uplink,
        report: impl Fn(CamStatus) + Send + 'static,
    ) -> Option<Self> {
        let (control, controls) = mpsc::channel();
        let alive = Arc::new(AtomicBool::new(true));
        let guard = AliveGuard(alive.clone());
        let _ = control.send(CamControl::Capture(true));
        std::thread::Builder::new()
            .name("skerry-camera".into())
            .spawn(move || {
                let _guard = guard;
                let engine = Engine::new(native::AvfCamera::default(), chosen, size, uplink);
                run(engine, &controls, report);
            })
            .ok()?;
        Some(Self { control, alive })
    }

    /// The thread's own flag: true until it has ended (a blocked call keeps it true).
    pub fn alive(&self) -> Arc<AtomicBool> {
        self.alive.clone()
    }

    /// Captures from `chosen` from now on (`None` follows the Mac's default). Never blocks.
    pub fn set_device(&self, chosen: Option<String>) {
        let _ = self.control.send(CamControl::Choose(chosen));
    }

    /// The server's `webcam_width` × `webcam_height` changed.
    pub fn set_size(&self, size: (u32, u32)) {
        let _ = self.control.send(CamControl::Size(size.0, size.1));
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        // The thread tears its session down on its own; never join here.
        let _ = self.control.send(CamControl::Shutdown);
    }
}

struct Active<S> {
    device: CameraDevice,
    session: S,
    size: (u32, u32),
    /// The default when this session was built on it: rebuilt on any default change.
    built_on_default: Option<String>,
    opened: Instant,
}

#[derive(Clone, Copy, Debug)]
struct Strikes {
    count: u32,
    retry_at: Instant,
    in_use: bool,
}

/// The camera thread's state, driven one tick at a time.
struct Engine<B: CamBackend> {
    backend: B,
    uplink: Uplink,
    encoder_ok: bool,
    sink_closed: bool,
    full_since: Option<Instant>,
    capture: bool,
    chosen: Option<String>,
    size: (u32, u32),
    devices: Vec<CameraDevice>,
    default_uid: Option<String>,
    enumerated: Option<Instant>,
    session: Option<Active<B::Session>>,
    strikes: BTreeMap<String, Strikes>,
    last_rebuild: Option<Instant>,
    /// A pick, a size or a start waiting to apply: it skips the rate limit and the backoff.
    picked: bool,
}

impl<B: CamBackend> Engine<B> {
    fn new(mut backend: B, chosen: Option<String>, size: (u32, u32), uplink: Uplink) -> Self {
        let encoder_ok = backend.encoder_ok();
        Self {
            backend,
            uplink,
            encoder_ok,
            sink_closed: false,
            full_since: None,
            capture: false,
            chosen,
            size,
            devices: Vec::new(),
            default_uid: None,
            enumerated: None,
            session: None,
            strikes: BTreeMap::new(),
            last_rebuild: None,
            picked: true,
        }
    }

    fn choose(&mut self, chosen: Option<String>) {
        if let Some(uid) = &chosen {
            self.strikes.remove(uid);
        }
        self.chosen = chosen;
        self.picked = true;
    }

    fn set_capture(&mut self, on: bool) {
        self.capture = on;
        self.picked = true;
    }

    fn set_size(&mut self, size: (u32, u32)) {
        if size != self.size {
            self.size = size;
            self.picked = true;
        }
    }

    fn capturing(&self) -> bool {
        self.capture && self.session.is_some()
    }

    fn strike(&mut self, uid: &str, build_failed: bool, in_use: bool, now: Instant) {
        let count = self.strikes.get(uid).map_or(0, |strikes| strikes.count) + 1;
        let wait = if build_failed {
            audio::backoff(count).max(BUILD_RETRY)
        } else {
            audio::backoff(count)
        };
        self.strikes.insert(
            uid.to_string(),
            Strikes {
                count,
                retry_at: now + wait,
                in_use,
            },
        );
    }

    fn enumerate(&mut self) {
        let Some(devices) = self.backend.devices() else {
            return;
        };
        self.devices = audio::label_devices_as(devices, "Camera");
        self.default_uid = self
            .backend
            .default_uid()
            .filter(|uid| self.devices.iter().any(|device| &device.uid == uid));
    }

    fn present(&self, uid: &str) -> bool {
        self.devices.iter().any(|device| device.uid == uid)
    }

    /// The chosen camera when connected, else the default, else the first camera.
    fn target(&self) -> Option<String> {
        match &self.chosen {
            Some(uid) if self.present(uid) => Some(uid.clone()),
            _ => self
                .default_uid
                .clone()
                .or_else(|| self.devices.first().map(|device| device.uid.clone())),
        }
    }

    fn needs_rebuild(&self, target: Option<&str>) -> bool {
        match (&self.session, target) {
            (None, Some(_)) | (Some(_), None) => true,
            (Some(active), Some(target)) => {
                active.device.uid != target
                    || active.size != self.size
                    || active
                        .built_on_default
                        .as_ref()
                        .is_some_and(|built| Some(built) != self.default_uid.as_ref())
            }
            (None, None) => false,
        }
    }

    /// Tears the session down (on this thread).
    fn close(&mut self) {
        self.session = None;
    }

    fn tick(&mut self, now: Instant) -> CamStatus {
        let due = self
            .enumerated
            .is_none_or(|last| now.duration_since(last) >= ENUMERATE_EVERY);
        if due || self.picked {
            self.enumerate();
            self.enumerated = Some(now);
        }
        let target = self.target();
        if self.uplink.closed() {
            self.sink_closed = true;
        }
        if self.uplink.full() {
            self.full_since.get_or_insert(now);
        } else {
            self.full_since = None;
        }
        let stalled = self
            .full_since
            .is_some_and(|since| now.duration_since(since) >= SINK_STALL);
        if !self.capture || !self.encoder_ok || self.sink_closed || stalled {
            self.close();
            self.picked = false;
            return self.status(target.as_deref());
        }
        let health = self.session.as_ref().map(|active| {
            (
                active.device.uid.clone(),
                now.duration_since(active.opened) >= HEALTHY_AFTER,
            )
        });
        if let Some((uid, steady)) = health {
            let state = match &self.session {
                Some(active) => self.backend.health(&active.session),
                None => Health::Running,
            };
            match state {
                Health::Running if steady => {
                    self.strikes.remove(&uid);
                }
                Health::Running => {}
                failed => {
                    self.close();
                    self.strike(&uid, false, failed == Health::InUse, now);
                }
            }
        }
        if self.needs_rebuild(target.as_deref()) {
            let limited = self
                .last_rebuild
                .is_some_and(|last| now.duration_since(last) < audio::REBUILD_LIMIT);
            let waiting = target.as_ref().is_some_and(|uid| {
                self.strikes
                    .get(uid)
                    .is_some_and(|strikes| strikes.retry_at > now)
            });
            if self.picked || (!limited && !waiting) {
                self.rebuild(target.as_deref(), now);
            }
        }
        self.picked = false;
        self.status(target.as_deref())
    }

    fn rebuild(&mut self, target: Option<&str>, now: Instant) {
        self.close();
        let Some(uid) = target else {
            return;
        };
        self.last_rebuild = Some(now);
        let Some(device) = self.devices.iter().find(|d| d.uid == uid).cloned() else {
            return;
        };
        // A fresh session starts on a keyframe whatever happened before.
        self.uplink.keyframe.store(true, Ordering::Release);
        match self.backend.open(uid, self.size, self.uplink.clone()) {
            Ok(session) => {
                self.session = Some(Active {
                    built_on_default: (self.default_uid.as_deref() == Some(uid))
                        .then(|| uid.to_string()),
                    device,
                    session,
                    size: self.size,
                    opened: now,
                });
            }
            Err(health) => self.strike(uid, true, health == Health::InUse, now),
        }
    }

    fn status(&self, target: Option<&str>) -> CamStatus {
        let strikes = target.and_then(|uid| self.strikes.get(uid));
        let problem = if !self.encoder_ok {
            Some(CamProblem::EncoderUnavailable)
        } else if self.chosen.as_deref().is_some_and(|uid| !self.present(uid)) {
            Some(CamProblem::ChosenMissing)
        } else if target.is_none() {
            Some(CamProblem::NoDevice)
        } else if self.capture && self.session.is_none() {
            match strikes {
                Some(strikes) if strikes.in_use => Some(CamProblem::InUse),
                Some(strikes) if strikes.count >= 2 => Some(CamProblem::Failed),
                _ => None,
            }
        } else {
            None
        };
        CamStatus {
            devices: self.devices.clone(),
            default_uid: self.default_uid.clone(),
            active: self.session.as_ref().map(|active| active.device.clone()),
            chosen: self.chosen.clone(),
            problem,
            live: self.session.is_some(),
        }
    }
}

/// What the queued controls amount to: only the last of each kind counts.
#[derive(Debug, Default, PartialEq, Eq)]
struct Collapsed {
    shutdown: bool,
    choose: Option<Option<String>>,
    capture: Option<bool>,
    size: Option<(u32, u32)>,
}

fn collapse(controls: impl IntoIterator<Item = CamControl>) -> Collapsed {
    let mut collapsed = Collapsed::default();
    for control in controls {
        match control {
            CamControl::Choose(chosen) => collapsed.choose = Some(chosen),
            CamControl::Capture(on) => collapsed.capture = Some(on),
            CamControl::Size(width, height) => collapsed.size = Some((width, height)),
            CamControl::Shutdown => collapsed.shutdown = true,
        }
    }
    collapsed
}

/// The camera thread: ticks at once, then every 100 ms while capturing (500 ms otherwise),
/// reports changed statuses, and ends on `Shutdown` or once `Camera` is gone, tearing its
/// session down here.
fn run<B: CamBackend>(
    mut engine: Engine<B>,
    controls: &Receiver<CamControl>,
    report: impl Fn(CamStatus),
) {
    let mut last = None;
    let mut wait = Duration::ZERO;
    loop {
        let first = match controls.recv_timeout(wait) {
            Ok(control) => Some(control),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let collapsed = collapse(first.into_iter().chain(controls.try_iter()));
        if collapsed.shutdown {
            return;
        }
        if let Some(chosen) = collapsed.choose {
            engine.choose(chosen);
        }
        if let Some(size) = collapsed.size {
            engine.set_size(size);
        }
        if let Some(on) = collapsed.capture {
            engine.set_capture(on);
        }
        let status = engine.tick(Instant::now());
        wait = if engine.capturing() {
            CAPTURE_TICK
        } else {
            IDLE_TICK
        };
        if last.as_ref() != Some(&status) {
            report(status.clone());
            last = Some(status);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn avcc_to_annexb_converts_multiple_nals() {
        let sample = [0, 0, 0, 2, 0x41, 1, 0, 0, 0, 3, 0x41, 2, 3];
        assert_eq!(
            avcc_to_annexb(&sample, 4, None).unwrap(),
            [0, 0, 0, 1, 0x41, 1, 0, 0, 0, 1, 0x41, 2, 3]
        );
        // Two-byte lengths too.
        assert_eq!(
            avcc_to_annexb(&[0, 1, 9], 2, None).unwrap(),
            [0, 0, 0, 1, 9]
        );
        assert!(avcc_to_annexb(&[1, 9], 5, None).is_none());
    }

    #[test]
    fn avcc_to_annexb_prepends_params_on_keyframe() {
        let sample = [0, 0, 0, 2, 0x65, 7];
        let frame = avcc_to_annexb(&sample, 4, Some((&[0x67, 1], &[0x68, 2]))).unwrap();
        assert_eq!(
            frame,
            [0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65, 7]
        );
    }

    #[test]
    fn avcc_to_annexb_rejects_truncated_length() {
        assert!(avcc_to_annexb(&[0, 0, 0, 9, 0x41, 1], 4, None).is_none());
        assert!(avcc_to_annexb(&[0, 0, 0, 1, 0x41, 0, 0], 4, None).is_none());
    }

    #[test]
    fn pick_format_prefers_smallest_at_least_target_at_30fps() {
        let formats = [
            (640, 480, 30.0),
            (1920, 1080, 30.0),
            (1280, 720, 30.0),
            (1280, 720, 15.0),
            (3840, 2160, 30.0),
        ];
        assert_eq!(pick_format(&formats, (1280, 720)), Some(2));
        assert_eq!(pick_format(&formats, (1300, 720)), Some(1));
        assert_eq!(pick_format(&formats, (320, 240)), Some(0));
        // 29.97 counts as 30.
        assert_eq!(pick_format(&[(1280, 720, 29.97)], (1280, 720)), Some(0));
    }

    #[test]
    fn pick_format_falls_back_to_largest_30fps() {
        let formats = [(640, 480, 30.0), (1280, 720, 30.0), (3840, 2160, 15.0)];
        assert_eq!(pick_format(&formats, (1920, 1080)), Some(1));
        // Nothing reaches 30 fps: the largest.
        let slow = [(640, 480, 15.0), (1920, 1080, 10.0)];
        assert_eq!(pick_format(&slow, (1280, 720)), Some(1));
        assert_eq!(pick_format(&[], (1280, 720)), None);
    }

    #[test]
    fn keyframe_gate_first_frame_forced() {
        let mut gate = KeyframeGate::default();
        let now = Instant::now();
        assert!(gate.take(now));
        assert!(!gate.take(now + Duration::from_millis(33)));
    }

    #[test]
    fn keyframe_gate_request_and_drop_force_next() {
        let start = Instant::now();
        let mut gate = KeyframeGate::default();
        gate.take(start);
        gate.request();
        assert!(gate.take(start + Duration::from_secs(1)));
        assert!(!gate.take(start + Duration::from_millis(1033)));
        gate.request();
        assert!(gate.take(start + Duration::from_secs(2)));
    }

    #[test]
    fn keyframe_gate_caps_forced_keyframes() {
        let start = Instant::now();
        let mut gate = KeyframeGate::default();
        gate.take(start);
        // A request right after a keyframe waits for 500 ms, then still applies.
        gate.request();
        assert!(!gate.take(start + Duration::from_millis(100)));
        gate.request();
        assert!(!gate.take(start + Duration::from_millis(400)));
        assert!(gate.take(start + Duration::from_millis(500)));
        // Requests in a burst give one keyframe per 500 ms, never more.
        let mut keys = 0;
        for frame in 1..=60u64 {
            gate.request();
            if gate.take(start + Duration::from_millis(500 + frame * 33)) {
                keys += 1;
            }
        }
        assert_eq!(keys, 3, "about 2 s of forced requests");
    }

    #[test]
    fn keyframe_gate_every_four_seconds() {
        let start = Instant::now();
        let mut gate = KeyframeGate::default();
        gate.take(start);
        assert!(!gate.take(start + Duration::from_millis(3999)));
        assert!(gate.take(start + Duration::from_secs(4)));
        assert!(!gate.take(start + Duration::from_secs(5)));
    }

    #[test]
    fn a_full_channel_asks_for_a_keyframe_and_a_closed_one_reports() {
        let (sink, mut frames) = tokio::sync::mpsc::channel(1);
        let keyframe = Arc::new(AtomicBool::new(false));
        let uplink = Uplink::new(sink, keyframe.clone());
        assert!(uplink.send(vec![1]));
        assert!(!uplink.take_keyframe_request());
        assert!(uplink.send(vec![2]));
        assert!(
            uplink.take_keyframe_request(),
            "dropped: the next frame is a keyframe"
        );
        assert!(!uplink.take_keyframe_request());
        assert_eq!(frames.try_recv().unwrap(), [1]);
        drop(frames);
        assert!(!uplink.send(vec![3]));
    }

    #[derive(Default)]
    struct Log {
        opens: Vec<(String, (u32, u32))>,
        sessions: usize,
        dropped: usize,
    }

    struct FakeSession {
        log: Rc<RefCell<Log>>,
        running: Rc<RefCell<Health>>,
    }

    impl Drop for FakeSession {
        fn drop(&mut self) {
            self.log.borrow_mut().dropped += 1;
        }
    }

    type Listed = Rc<RefCell<Vec<(String, Option<String>)>>>;

    struct FakeCam {
        log: Rc<RefCell<Log>>,
        devices: Listed,
        default: Rc<RefCell<Option<String>>>,
        running: Rc<RefCell<Health>>,
        open_fails: Rc<RefCell<Option<Health>>>,
        encoder: bool,
    }

    impl CamBackend for FakeCam {
        type Session = FakeSession;
        fn devices(&mut self) -> Option<Vec<(String, Option<String>)>> {
            Some(self.devices.borrow().clone())
        }
        fn default_uid(&mut self) -> Option<String> {
            self.default.borrow().clone()
        }
        fn encoder_ok(&mut self) -> bool {
            self.encoder
        }
        fn open(&mut self, uid: &str, size: (u32, u32), _: Uplink) -> Result<FakeSession, Health> {
            self.log.borrow_mut().opens.push((uid.to_string(), size));
            if let Some(failure) = *self.open_fails.borrow() {
                return Err(failure);
            }
            self.log.borrow_mut().sessions += 1;
            *self.running.borrow_mut() = Health::Running;
            Ok(FakeSession {
                log: self.log.clone(),
                running: self.running.clone(),
            })
        }
        fn health(&mut self, session: &FakeSession) -> Health {
            *session.running.borrow()
        }
    }

    struct Rig {
        engine: Engine<FakeCam>,
        log: Rc<RefCell<Log>>,
        running: Rc<RefCell<Health>>,
        open_fails: Rc<RefCell<Option<Health>>>,
        default: Rc<RefCell<Option<String>>>,
        frames: tokio::sync::mpsc::Receiver<Vec<u8>>,
        keyframe: Arc<AtomicBool>,
    }

    fn rig(chosen: Option<&str>, encoder: bool) -> Rig {
        let log = Rc::new(RefCell::new(Log::default()));
        let running = Rc::new(RefCell::new(Health::Running));
        let open_fails = Rc::new(RefCell::new(None));
        let default = Rc::new(RefCell::new(Some("FaceTime".to_string())));
        let devices = Rc::new(RefCell::new(vec![
            (
                "FaceTime".to_string(),
                Some("FaceTime HD Camera".to_string()),
            ),
            ("iPhone".to_string(), Some("iPhone Camera".to_string())),
        ]));
        let (sink, frames) = tokio::sync::mpsc::channel(crate::capture::CAM_QUEUE);
        let keyframe = Arc::new(AtomicBool::new(false));
        let backend = FakeCam {
            log: log.clone(),
            devices,
            default: default.clone(),
            running: running.clone(),
            open_fails: open_fails.clone(),
            encoder,
        };
        let engine = Engine::new(
            backend,
            chosen.map(str::to_string),
            (1280, 720),
            Uplink::new(sink, keyframe.clone()),
        );
        Rig {
            engine,
            log,
            running,
            open_fails,
            default,
            frames,
            keyframe,
        }
    }

    #[test]
    fn cam_engine_opens_on_capture_and_rebuilds_when_not_running() {
        let mut rig = rig(None, true);
        let start = Instant::now();
        // Not capturing: nothing opens.
        let status = rig.engine.tick(start);
        assert!(rig.log.borrow().opens.is_empty());
        assert!(!status.live);
        rig.engine.set_capture(true);
        let status = rig.engine.tick(start);
        assert_eq!(rig.log.borrow().opens, [("FaceTime".into(), (1280, 720))]);
        assert!(status.live && status.problem.is_none());
        assert!(
            rig.keyframe.load(Ordering::Acquire),
            "a new session starts on a keyframe"
        );
        // The session stopped: closed, and rebuilt at once the first time.
        *rig.running.borrow_mut() = Health::Stopped;
        let status = rig.engine.tick(start + Duration::from_secs(1));
        assert!(status.live);
        assert_eq!(
            (rig.log.borrow().dropped, rig.log.borrow().sessions),
            (1, 2)
        );
        // Stopped again soon after: it waits out the rate limit and the backoff.
        *rig.running.borrow_mut() = Health::Stopped;
        assert!(!rig.engine.tick(start + Duration::from_millis(1500)).live);
        assert_eq!(
            rig.engine.tick(start + Duration::from_secs(3)).problem,
            Some(CamProblem::Failed)
        );
        assert!(rig.engine.tick(start + Duration::from_millis(3600)).live);
        assert_eq!(rig.log.borrow().sessions, 3);
        // A size change rebuilds at once.
        rig.engine.set_size((640, 480));
        rig.engine.tick(start + Duration::from_millis(3700));
        assert_eq!(
            rig.log.borrow().opens.last(),
            Some(&("FaceTime".into(), (640, 480)))
        );
        // Off: the session goes.
        rig.engine.set_capture(false);
        let status = rig.engine.tick(start + Duration::from_secs(4));
        assert!(!status.live);
        assert_eq!(rig.log.borrow().dropped, 4);
    }

    #[test]
    fn cam_engine_reports_chosen_missing_and_uses_default() {
        let mut rig = rig(Some("Gone"), true);
        rig.engine.set_capture(true);
        let status = rig.engine.tick(Instant::now());
        assert_eq!(status.problem, Some(CamProblem::ChosenMissing));
        assert_eq!(rig.log.borrow().opens[0].0, "FaceTime");
        assert_eq!(status.active.unwrap().name, "FaceTime HD Camera");
        // The default moves: the session follows.
        *rig.default.borrow_mut() = Some("iPhone".into());
        rig.engine.tick(Instant::now() + Duration::from_secs(2));
        assert_eq!(rig.log.borrow().opens.last().unwrap().0, "iPhone");
    }

    #[test]
    fn cam_engine_reports_in_use() {
        let mut rig = rig(None, true);
        *rig.open_fails.borrow_mut() = Some(Health::InUse);
        rig.engine.set_capture(true);
        let status = rig.engine.tick(Instant::now());
        assert_eq!(status.problem, Some(CamProblem::InUse));
        assert!(!status.live);
    }

    #[test]
    fn cam_engine_stops_when_sink_closed() {
        let mut rig = rig(None, true);
        rig.engine.set_capture(true);
        let start = Instant::now();
        assert!(rig.engine.tick(start).live);
        drop(rig.frames);
        let status = rig.engine.tick(start + CAPTURE_TICK);
        assert!(!status.live);
        assert_eq!(rig.log.borrow().dropped, 1);
        // Asked to capture again after the stream ended: nothing opens.
        rig.engine.set_capture(true);
        rig.engine.tick(start + Duration::from_secs(5));
        assert_eq!(rig.log.borrow().sessions, 1);
    }

    #[test]
    fn cam_engine_closes_while_the_channel_stays_full() {
        let mut rig = rig(None, true);
        rig.engine.set_capture(true);
        let start = Instant::now();
        rig.engine.tick(start);
        for _ in 0..crate::capture::CAM_QUEUE {
            rig.engine.uplink.send(vec![6, 1, 0]);
        }
        assert!(rig.engine.tick(start + Duration::from_secs(1)).live);
        assert!(!rig.engine.tick(start + Duration::from_secs(3)).live);
        // Drained: it opens again.
        while rig.frames.try_recv().is_ok() {}
        assert!(rig.engine.tick(start + Duration::from_secs(5)).live);
    }

    #[test]
    fn cam_engine_reports_encoder_unavailable() {
        let mut rig = rig(None, false);
        rig.engine.set_capture(true);
        let status = rig.engine.tick(Instant::now());
        assert_eq!(status.problem, Some(CamProblem::EncoderUnavailable));
        assert!(rig.log.borrow().opens.is_empty());
    }

    #[test]
    fn controls_collapse_to_the_last_of_each() {
        let collapsed = collapse([
            CamControl::Size(640, 480),
            CamControl::Capture(true),
            CamControl::Choose(Some("a".into())),
            CamControl::Size(1280, 720),
            CamControl::Capture(false),
        ]);
        assert_eq!(collapsed.size, Some((1280, 720)));
        assert_eq!(collapsed.capture, Some(false));
        assert_eq!(collapsed.choose, Some(Some("a".into())));
        assert!(!collapsed.shutdown);
    }
}
