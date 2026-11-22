//! The session window's microphone and camera (`ARCH-038`): their `skerry-mic` and
//! `skerry-camera` threads, the pure capture policy, the device lists for the card, and the
//! one place every stream end stops capture.
//!
//! [`MediaCapture::stop`] runs on every stream end: a server close or network drop
//! (`SessionEvent::Error`), the start-up timeout, Reconnect, window close and quit. A
//! `*_DISABLED` or a lock stops the device through the policy. Each thread also stops itself
//! once the stream's frame channel closes, so the device goes off even if an event is missed.
//!
//! Nothing touches a device before capture is asked for **and** macOS granted it (M-QA-1):
//! a capture thread is created only by an `Effect::Start` (which the policy raises only once
//! authorised) and dropped by `Effect::Stop`. The card's microphone list comes from plain
//! CoreAudio properties on a short-lived `skerry-mic-list` thread, and its camera list from
//! `skerry-cam-list`, which lists nothing until macOS says the camera is authorised; at most
//! one of each at a time per window. A thread that blocks is detached, and no new one is
//! stacked on it.
//!
//! A start that lands while the device's previous thread is still exiting (a quick
//! off-and-on, or a Reconnect whose settings beat the old thread's exit) is kept pending and
//! retried from the 1 s ticker once that thread has ended, never dropped.

use crate::camera::{self, CamProblem, CamStatus, Camera};
use crate::capture::{self, CapturePolicy, CaptureState, Effect, Phase, Reply, Subject, Ticket};
use crate::input_devices::InputList;
use crate::microphone::{MicProblem, MicStatus, Microphone};
use crate::transport::{CameraFrames, SessionEvent};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

/// What the window drives a microphone thread with; a seam for tests.
pub(super) trait MicHandle {
    fn set_input(&self, chosen: Option<String>);
    /// True until the thread has really ended.
    fn alive(&self) -> Arc<AtomicBool>;
}

impl MicHandle for Microphone {
    fn set_input(&self, chosen: Option<String>) {
        Microphone::set_input(self, chosen);
    }

    fn alive(&self) -> Arc<AtomicBool> {
        Microphone::alive(self)
    }
}

/// What the window drives a camera thread with; a seam for tests.
pub(super) trait CamHandle {
    fn set_device(&self, chosen: Option<String>);
    fn set_size(&self, size: (u32, u32));
    /// True until the thread has really ended.
    fn alive(&self) -> Arc<AtomicBool>;
}

impl CamHandle for Camera {
    fn set_device(&self, chosen: Option<String>) {
        Camera::set_device(self, chosen);
    }

    fn set_size(&self, size: (u32, u32)) {
        Camera::set_size(self, size);
    }

    fn alive(&self) -> Arc<AtomicBool> {
        Camera::alive(self)
    }
}

/// Makes a capturing microphone thread from the stream's sink and a status reporter.
pub(super) type MakeMic<'a, M> =
    Box<dyn FnOnce(mpsc::Sender<Vec<u8>>, Box<dyn Fn(MicStatus) + Send>) -> Option<M> + 'a>;

/// Makes a capturing camera thread from the stream's uplink, the server's size and a status
/// reporter.
pub(super) type MakeCam<'a, C> =
    Box<dyn FnOnce(camera::Uplink, (u32, u32), Box<dyn Fn(CamStatus) + Send>) -> Option<C> + 'a>;

/// One device's thread and what the window keeps about it.
struct Slot<H> {
    handle: Option<H>,
    /// The last dropped thread's flag: while it is still alive (blocked in a call), no new
    /// thread starts.
    lingering: Option<Arc<AtomicBool>>,
    /// A start waits for the lingering thread to end; the ticker retries it.
    pending: bool,
    /// The id the current thread tags its statuses with.
    id: u64,
    /// The device list from the last listing, for the card.
    listed: Option<InputList>,
    /// A listing is running (one at a time, across reconnects).
    listing: Arc<AtomicBool>,
}

impl<H> Default for Slot<H> {
    fn default() -> Self {
        Self {
            handle: None,
            lingering: None,
            pending: false,
            id: 0,
            listed: None,
            listing: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl<H> Slot<H> {
    /// Whether the previous thread is still exiting.
    fn blocked(&self) -> bool {
        self.lingering
            .as_ref()
            .is_some_and(|alive| alive.load(Ordering::Acquire))
    }

    fn drop_handle(&mut self, alive: impl FnOnce(&H) -> Arc<AtomicBool>) {
        if let Some(handle) = self.handle.take() {
            self.lingering = Some(alive(&handle));
        }
        self.pending = false;
    }
}

pub(super) struct MediaCapture<M: MicHandle = Microphone, C: CamHandle = Camera> {
    pub(super) state: CaptureState,
    mic: Slot<M>,
    cam: Slot<C>,
    next_id: u64,
    pub(super) mic_status: Option<MicStatus>,
    pub(super) cam_status: Option<CamStatus>,
    /// The microphone thread reported it can't encode; kept for the card after it is gone.
    pub(super) encoder_unavailable: bool,
    /// The camera thread reported it can't encode H.264.
    pub(super) cam_encoder_unavailable: bool,
    /// Re-lists every second, and retries pending starts, while the stream lives; dropped by
    /// `stop`.
    pub(super) ticker: Option<gpui_kit::Task<()>>,
    stream_id: u64,
    /// The current stream's frame channels and event sender, for a thread created later.
    sink: Option<mpsc::Sender<Vec<u8>>>,
    cam_sink: Option<mpsc::Sender<Vec<u8>>>,
    /// The stream's camera keyframe request (`WEBCAM_KEYFRAME`, or a dropped frame).
    cam_keyframe: Arc<AtomicBool>,
    events: Option<mpsc::UnboundedSender<SessionEvent>>,
}

impl<M: MicHandle, C: CamHandle> Default for MediaCapture<M, C> {
    fn default() -> Self {
        Self {
            state: CaptureState::default(),
            mic: Slot::default(),
            cam: Slot::default(),
            next_id: 0,
            mic_status: None,
            cam_status: None,
            encoder_unavailable: false,
            cam_encoder_unavailable: false,
            ticker: None,
            stream_id: 0,
            sink: None,
            cam_sink: None,
            cam_keyframe: Arc::default(),
            events: None,
        }
    }
}

/// Events that end the stream without `end_stream`: they stop capture at once.
fn ends_stream(event: &SessionEvent) -> bool {
    matches!(event, SessionEvent::Error(_) | SessionEvent::StartupTimeout)
}

impl<M: MicHandle, C: CamHandle> MediaCapture<M, C> {
    /// A new stream: its frame channels (the receivers go to the transport).
    pub(super) fn begin_stream(
        &mut self,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> (mpsc::Receiver<Vec<u8>>, CameraFrames) {
        let (sink, frames) = mpsc::channel(capture::MIC_QUEUE);
        let (cam_sink, cam_frames) = mpsc::channel(capture::CAM_QUEUE);
        self.stream_id += 1;
        self.sink = Some(sink);
        self.cam_sink = Some(cam_sink);
        self.cam_keyframe = Arc::default();
        self.events = Some(events);
        let camera = CameraFrames {
            frames: cam_frames,
            keyframe: self.cam_keyframe.clone(),
        };
        (frames, camera)
    }

    /// The first thing `SessionWindow::handle_event` does with every event: one that ends the
    /// stream stops capture.
    pub(super) fn on_stream_event(&mut self, event: &SessionEvent) {
        if ends_stream(event) {
            self.stop();
        }
    }

    /// Stops every device: each thread gets `Shutdown`, pending permission answers go stale,
    /// and the server's word is forgotten. The person's own choice stays for a Reconnect.
    pub(super) fn stop(&mut self) {
        self.state.on_stream_end();
        self.drop_microphone();
        self.drop_camera();
        self.mic.listed = None;
        self.cam.listed = None;
        self.ticker = None;
        self.sink = None;
        self.cam_sink = None;
        self.events = None;
    }

    fn drop_microphone(&mut self) {
        self.mic.drop_handle(|mic| mic.alive());
        self.mic_status = None;
    }

    fn drop_camera(&mut self) {
        self.cam.drop_handle(|cam| cam.alive());
        self.cam_status = None;
    }

    /// Runs `effects`: a start creates the capturing thread (the policy starts only once
    /// macOS granted it), a stop drops it. Returns the permission prompts to show.
    pub(super) fn apply_with(
        &mut self,
        effects: Vec<Effect>,
        make_mic: MakeMic<'_, M>,
        make_cam: MakeCam<'_, C>,
    ) -> Vec<Ticket> {
        let mut make_mic = Some(make_mic);
        let mut make_cam = Some(make_cam);
        let mut asks = Vec::new();
        for effect in effects {
            match effect {
                Effect::Start(Subject::Microphone) => {
                    if let Some(make) = make_mic.take() {
                        self.start_microphone(make);
                    }
                }
                Effect::Start(Subject::Camera) => {
                    if let Some(make) = make_cam.take() {
                        self.start_camera(make);
                    }
                }
                Effect::Stop(Subject::Microphone) => self.drop_microphone(),
                Effect::Stop(Subject::Camera) => self.drop_camera(),
                Effect::AskPermission(ticket) => asks.push(ticket),
            }
        }
        asks
    }

    fn start_microphone(&mut self, make: MakeMic<'_, M>) {
        if self.mic.handle.is_some() {
            return;
        }
        // A thread still stuck in a call is never stacked with a new one: retried later.
        if self.mic.blocked() {
            self.mic.pending = true;
            return;
        }
        let (Some(sink), Some(events)) = (self.sink.clone(), self.events.clone()) else {
            return;
        };
        self.mic.pending = false;
        self.next_id += 1;
        let id = self.next_id;
        let report = Box::new(move |status| {
            let _ = events.send(SessionEvent::MicStatus(id, status));
        });
        self.encoder_unavailable = false;
        self.mic.handle = make(sink, report);
        if self.mic.handle.is_some() {
            self.mic.id = id;
        }
    }

    fn start_camera(&mut self, make: MakeCam<'_, C>) {
        if self.cam.handle.is_some() {
            return;
        }
        if self.cam.blocked() {
            self.cam.pending = true;
            return;
        }
        let (Some(sink), Some(events)) = (self.cam_sink.clone(), self.events.clone()) else {
            return;
        };
        self.cam.pending = false;
        self.next_id += 1;
        let id = self.next_id;
        let report = Box::new(move |status| {
            let _ = events.send(SessionEvent::CamStatus(id, status));
        });
        self.cam_encoder_unavailable = false;
        let size = self
            .state
            .policy
            .map_or(capture::DEFAULT_CAM_SIZE, |policy| policy.cam_size);
        let uplink = camera::Uplink::new(sink, self.cam_keyframe.clone());
        self.cam.handle = make(uplink, size, report);
        if self.cam.handle.is_some() {
            self.cam.id = id;
        }
    }

    /// Whether a start is waiting for a previous thread to end.
    pub(super) fn has_pending(&self) -> bool {
        self.mic.pending || self.cam.pending
    }

    /// The ticker: starts what was held back once the previous thread has ended, if the
    /// policy still wants the device on.
    pub(super) fn retry_pending_with(
        &mut self,
        make_mic: MakeMic<'_, M>,
        make_cam: MakeCam<'_, C>,
    ) {
        if self.mic.pending && !self.mic.blocked() {
            self.mic.pending = false;
            if self.state.mic.phase == Phase::On {
                self.start_microphone(make_mic);
            }
        }
        if self.cam.pending && !self.cam.blocked() {
            self.cam.pending = false;
            if self.state.cam.phase == Phase::On {
                self.start_camera(make_cam);
            }
        }
    }

    /// A status from a microphone thread: ignored unless it is the current one's. A thread
    /// that can't encode turns the switch off.
    pub(super) fn on_status(&mut self, id: u64, status: MicStatus) -> Vec<Effect> {
        if id != self.mic.id || self.mic.handle.is_none() {
            return Vec::new();
        }
        let unavailable = status.problem == Some(MicProblem::EncoderUnavailable);
        self.mic_status = Some(status);
        if unavailable {
            self.encoder_unavailable = true;
            self.state.on_unavailable(Subject::Microphone)
        } else {
            Vec::new()
        }
    }

    /// A status from a camera thread, as [`Self::on_status`].
    pub(super) fn on_cam_status(&mut self, id: u64, status: CamStatus) -> Vec<Effect> {
        if id != self.cam.id || self.cam.handle.is_none() {
            return Vec::new();
        }
        let unavailable = status.problem == Some(CamProblem::EncoderUnavailable);
        self.cam_status = Some(status);
        if unavailable {
            self.cam_encoder_unavailable = true;
            self.state.on_unavailable(Subject::Camera)
        } else {
            Vec::new()
        }
    }

    /// A capture reply: `WEBCAM_KEYFRAME` asks the camera for a keyframe; the rest go to
    /// the policy.
    pub(super) fn on_reply(&mut self, reply: Reply, auth: capture::Auth) -> Vec<Effect> {
        if reply == Reply::Keyframe {
            self.cam_keyframe.store(true, Ordering::Release);
        }
        self.state.on_reply(reply, auth)
    }

    pub(super) fn set_input(&self, chosen: Option<String>) {
        if let Some(mic) = &self.mic.handle {
            mic.set_input(chosen);
        }
    }

    pub(super) fn set_camera_device(&self, chosen: Option<String>) {
        if let Some(cam) = &self.cam.handle {
            cam.set_device(chosen);
        }
    }

    /// The badge shows while a device is open.
    pub(super) fn badge(&self) -> Option<(bool, bool)> {
        capture::badge(
            self.mic_status.as_ref().is_some_and(|status| status.live),
            self.cam_status.as_ref().is_some_and(|status| status.live),
        )
    }

    /// A primary `server_settings`. A new camera size reaches a running camera.
    pub(super) fn on_policy(
        &mut self,
        policy: CapturePolicy,
        auth: impl Into<capture::Auths>,
    ) -> Vec<Effect> {
        let size = self.state.policy.map(|policy| policy.cam_size);
        let effects = self.state.on_policy(policy, auth);
        if size.is_some_and(|size| size != policy.cam_size) {
            if let Some(cam) = &self.cam.handle {
                cam.set_size(policy.cam_size);
            }
        }
        effects
    }

    /// Whether the ticker should run: a live stream whose server allows a device, or a start
    /// waiting on a previous thread.
    pub(super) fn wants_ticks(&self) -> bool {
        self.events.is_some()
            && (self.state.permitted(Subject::Microphone)
                || self.state.permitted(Subject::Camera)
                || self.has_pending())
    }

    /// Whether the card wants a microphone list now.
    pub(super) fn wants_inputs(&self) -> bool {
        self.events.is_some() && self.state.permitted(Subject::Microphone)
    }

    /// Whether the card wants a camera list now (the listing itself waits for macOS's
    /// permission).
    pub(super) fn wants_cameras(&self) -> bool {
        self.events.is_some() && self.state.permitted(Subject::Camera)
    }

    /// Lists on a named thread unless one of the same kind is still running (blocked or
    /// slow): never more than one, however many reconnects. Returns whether one started.
    fn list_with(
        &mut self,
        subject: Subject,
        list: impl FnOnce() -> Option<InputList> + Send + 'static,
    ) -> bool {
        let Some(events) = self.events.clone() else {
            return false;
        };
        let (listing, name) = match subject {
            Subject::Microphone => (self.mic.listing.clone(), "skerry-mic-list"),
            Subject::Camera => (self.cam.listing.clone(), "skerry-cam-list"),
        };
        if listing.swap(true, Ordering::AcqRel) {
            return false;
        }
        let id = self.stream_id;
        let running = listing.clone();
        let spawned = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let listed = list();
                running.store(false, Ordering::Release);
                if let Some(listed) = listed {
                    let _ = events.send(match subject {
                        Subject::Microphone => SessionEvent::MicInputs(id, listed),
                        Subject::Camera => SessionEvent::CamInputs(id, listed),
                    });
                }
            });
        if spawned.is_err() {
            listing.store(false, Ordering::Release);
        }
        spawned.is_ok()
    }

    /// Lists inputs with `list` on a `skerry-mic-list` thread (one at a time).
    pub(super) fn refresh_inputs_with(
        &mut self,
        list: impl FnOnce() -> Option<InputList> + Send + 'static,
    ) -> bool {
        self.list_with(Subject::Microphone, list)
    }

    /// Lists cameras with `list` on a `skerry-cam-list` thread (one at a time).
    pub(super) fn refresh_cameras_with(
        &mut self,
        list: impl FnOnce() -> Option<InputList> + Send + 'static,
    ) -> bool {
        self.list_with(Subject::Camera, list)
    }

    /// The inputs the card shows, labelled, and the default's UID: the capturing thread's
    /// own list while it runs, else the last listing.
    pub(super) fn devices(&self) -> (Vec<crate::audio::OutputDevice>, Option<String>) {
        match (&self.mic_status, &self.mic.listed) {
            (Some(status), _) => (status.devices.clone(), status.default_uid.clone()),
            (None, Some(inputs)) => (
                crate::audio::label_devices_as(inputs.devices.clone(), "Microphone"),
                inputs.default_uid.clone(),
            ),
            (None, None) => (Vec::new(), None),
        }
    }

    /// The cameras the card shows, as [`Self::devices`].
    pub(super) fn cameras(&self) -> (Vec<crate::audio::OutputDevice>, Option<String>) {
        match (&self.cam_status, &self.cam.listed) {
            (Some(status), _) => (status.devices.clone(), status.default_uid.clone()),
            (None, Some(cameras)) => (
                crate::audio::label_devices_as(cameras.devices.clone(), "Camera"),
                cameras.default_uid.clone(),
            ),
            (None, None) => (Vec::new(), None),
        }
    }

    /// The last microphone listing, for the card.
    pub(super) fn inputs(&self) -> Option<&InputList> {
        self.mic.listed.as_ref()
    }

    /// A listing's answer, if it is this stream's.
    pub(super) fn on_inputs(&mut self, id: u64, inputs: InputList) {
        if id == self.stream_id && self.events.is_some() {
            self.mic.listed = Some(inputs);
        }
    }

    /// A camera listing's answer, if it is this stream's.
    pub(super) fn on_cameras(&mut self, id: u64, cameras: InputList) {
        if id == self.stream_id && self.events.is_some() {
            self.cam.listed = Some(cameras);
        }
    }
}

impl MediaCapture<Microphone, Camera> {
    /// `apply_with` making the real threads for the chosen devices.
    pub(super) fn apply(
        &mut self,
        effects: Vec<Effect>,
        mic: Option<String>,
        cam: Option<String>,
    ) -> Vec<Ticket> {
        self.apply_with(effects, real_mic(mic), real_cam(cam))
    }

    /// `retry_pending_with` making the real threads.
    pub(super) fn retry_pending(&mut self, mic: Option<String>, cam: Option<String>) {
        self.retry_pending_with(real_mic(mic), real_cam(cam));
    }

    pub(super) fn refresh_inputs(&mut self) -> bool {
        self.refresh_inputs_with(crate::input_devices::list_inputs)
    }

    pub(super) fn refresh_cameras(&mut self) -> bool {
        self.refresh_cameras_with(camera::list_cameras)
    }
}

fn real_mic(chosen: Option<String>) -> MakeMic<'static, Microphone> {
    Box::new(move |sink, report| Microphone::new(chosen, sink, report))
}

fn real_cam(chosen: Option<String>) -> MakeCam<'static, Camera> {
    Box::new(move |uplink, size, report| Camera::new(chosen, size, uplink, report))
}

/// The chosen device's name to save: when a list (the thread's, or the card's own) shows the
/// device and its name differs from the saved one.
pub(super) fn input_name_to_save(
    chosen: Option<&str>,
    saved: Option<&str>,
    devices: &[crate::audio::OutputDevice],
) -> Option<String> {
    let chosen = chosen?;
    let name = &devices.iter().find(|device| device.uid == chosen)?.name;
    (saved != Some(name.as_str())).then(|| name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Auth, DevicePolicy, Phase, Reply};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    #[derive(Default)]
    struct Log {
        made: usize,
        calls: Vec<String>,
        dropped: usize,
    }

    struct FakeMic {
        log: Rc<RefCell<Log>>,
        alive: Arc<AtomicBool>,
    }

    impl MicHandle for FakeMic {
        fn set_input(&self, chosen: Option<String>) {
            self.log
                .borrow_mut()
                .calls
                .push(format!("input {chosen:?}"));
        }
        fn alive(&self) -> Arc<AtomicBool> {
            self.alive.clone()
        }
    }

    impl Drop for FakeMic {
        fn drop(&mut self) {
            self.log.borrow_mut().dropped += 1;
        }
    }

    /// A thread that ends at once on drop, or (`stuck`) one that stays blocked.
    fn maker(log: &Rc<RefCell<Log>>, stuck: bool) -> MakeMic<'static, FakeMic> {
        let log = log.clone();
        Box::new(move |_, _| {
            log.borrow_mut().made += 1;
            Some(FakeMic {
                log,
                alive: Arc::new(AtomicBool::new(stuck)),
            })
        })
    }

    struct FakeCam {
        log: Rc<RefCell<Log>>,
        alive: Arc<AtomicBool>,
    }

    impl CamHandle for FakeCam {
        fn set_device(&self, chosen: Option<String>) {
            self.log
                .borrow_mut()
                .calls
                .push(format!("camera {chosen:?}"));
        }
        fn set_size(&self, size: (u32, u32)) {
            self.log.borrow_mut().calls.push(format!("size {size:?}"));
        }
        fn alive(&self) -> Arc<AtomicBool> {
            self.alive.clone()
        }
    }

    impl Drop for FakeCam {
        fn drop(&mut self) {
            self.log.borrow_mut().dropped += 1;
        }
    }

    /// A camera maker for tests about the microphone: never called.
    fn no_cam() -> MakeCam<'static, FakeCam> {
        Box::new(|_, _, _| panic!("no camera expected"))
    }

    fn no_mic() -> MakeMic<'static, FakeMic> {
        Box::new(|_, _| panic!("no microphone expected"))
    }

    /// A camera thread that ends at once on drop, or (`stuck`) one that stays blocked. It
    /// records the size it was made with.
    fn cam_maker(log: &Rc<RefCell<Log>>, stuck: bool) -> MakeCam<'static, FakeCam> {
        let log = log.clone();
        Box::new(move |_, size, _| {
            log.borrow_mut().made += 1;
            log.borrow_mut().calls.push(format!("made {size:?}"));
            Some(FakeCam {
                log,
                alive: Arc::new(AtomicBool::new(stuck)),
            })
        })
    }

    fn cam_policy(on_start: bool) -> CapturePolicy {
        CapturePolicy {
            cam: DevicePolicy {
                permitted: true,
                on_start,
                ..DevicePolicy::default()
            },
            ..CapturePolicy::default()
        }
    }

    fn policy(on_start: bool) -> CapturePolicy {
        CapturePolicy {
            mic: DevicePolicy {
                permitted: true,
                on_start,
                ..DevicePolicy::default()
            },
            ..CapturePolicy::default()
        }
    }

    type Rig = (
        MediaCapture<FakeMic, FakeCam>,
        Rc<RefCell<Log>>,
        mpsc::Receiver<Vec<u8>>,
        mpsc::UnboundedReceiver<SessionEvent>,
    );

    /// A window's capture with a live stream whose server turned the microphone on.
    fn capturing() -> Rig {
        let mut capture = MediaCapture::<FakeMic, FakeCam>::default();
        let (events, events_rx) = mpsc::unbounded_channel();
        let (frames, _camera) = capture.begin_stream(events);
        let log = Rc::new(RefCell::new(Log::default()));
        let effects = capture.on_policy(policy(true), Auth::Authorized);
        assert!(capture
            .apply_with(effects, maker(&log, false), no_cam())
            .is_empty());
        assert_eq!(log.borrow().made, 1);
        (capture, log, frames, events_rx)
    }

    fn live() -> MicStatus {
        MicStatus {
            live: true,
            ..MicStatus::default()
        }
    }

    #[test]
    fn error_event_stops_capture() {
        let (mut capture, log, _frames, _events) = capturing();
        capture.on_status(1, live());
        assert_eq!(capture.badge(), Some((true, false)));
        capture.on_stream_event(&SessionEvent::Video(true));
        assert_eq!(log.borrow().dropped, 0);
        capture.on_stream_event(&SessionEvent::Error("closed".into()));
        assert_eq!(log.borrow().dropped, 1, "the thread got Shutdown");
        assert_eq!(capture.badge(), None);
        assert_eq!(capture.state.mic.phase, Phase::Off);
    }

    #[test]
    fn startup_timeout_stops_capture() {
        let (mut capture, log, _frames, _events) = capturing();
        capture.on_stream_event(&SessionEvent::StartupTimeout);
        assert_eq!(log.borrow().dropped, 1);
    }

    #[test]
    fn stop_closes_the_frame_channel() {
        let (mut capture, _log, frames, _events) = capturing();
        capture.stop();
        assert!(frames.is_closed());
    }

    #[test]
    fn no_thread_until_capture_is_granted() {
        // Allowed by the server but not asked for: nothing is created.
        let mut capture = MediaCapture::<FakeMic, FakeCam>::default();
        let (events, _rx) = mpsc::unbounded_channel();
        let _frames = capture.begin_stream(events);
        let log = Rc::new(RefCell::new(Log::default()));
        let effects = capture.on_policy(policy(false), Auth::Authorized);
        capture.apply_with(effects, maker(&log, false), no_cam());
        assert_eq!(log.borrow().made, 0);
        // Asked for while undetermined: a prompt, still no thread.
        let effects = capture
            .state
            .on_toggle(Subject::Microphone, true, Auth::NotDetermined);
        let asks = capture.apply_with(effects, maker(&log, false), no_cam());
        assert_eq!(asks.len(), 1);
        assert_eq!(log.borrow().made, 0);
        // Granted: now the thread starts.
        let effects = capture.state.on_auth(asks[0], true);
        capture.apply_with(effects, maker(&log, false), no_cam());
        assert_eq!(log.borrow().made, 1);
    }

    #[test]
    fn stop_effect_drops_the_thread() {
        let (mut capture, log, _frames, _events) = capturing();
        let effects = capture
            .state
            .on_reply(Reply::Disabled(Subject::Microphone), Auth::Authorized);
        capture.apply_with(effects, maker(&log, false), no_cam());
        assert_eq!(log.borrow().dropped, 1);
        assert_eq!(capture.badge(), None);
    }

    #[test]
    fn a_blocked_thread_is_never_stacked() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut capture = MediaCapture::<FakeMic, FakeCam>::default();
        for _ in 0..5 {
            // Reconnect: a new stream, the person's on carried over, the old thread stuck.
            let (events, _rx) = mpsc::unbounded_channel();
            let _frames = capture.begin_stream(events);
            let effects = capture.on_policy(policy(true), Auth::Authorized);
            capture.apply_with(effects, maker(&log, true), no_cam());
            capture.stop();
        }
        assert_eq!(log.borrow().made, 1, "one stuck thread, no more");
    }

    #[test]
    fn reconnects_do_not_grow_threads_while_permission_is_undetermined() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut capture = MediaCapture::<FakeMic, FakeCam>::default();
        // A device listing that blocks for good, as cpal's did while TCC waited.
        let (hold, held) = std::sync::mpsc::channel::<()>();
        let held = Arc::new(std::sync::Mutex::new(held));
        let mut listings = 0;
        for _ in 0..10 {
            let (events, _rx) = mpsc::unbounded_channel();
            let _frames = capture.begin_stream(events);
            let effects = capture.on_policy(policy(true), Auth::NotDetermined);
            let asks = capture.apply_with(effects, maker(&log, false), no_cam());
            assert_eq!(asks.len(), 1, "a prompt, never a thread");
            let held = held.clone();
            if capture.refresh_inputs_with(move || {
                let _ = held.lock().unwrap().recv();
                None
            }) {
                listings += 1;
            }
            capture.stop();
        }
        assert_eq!(log.borrow().made, 0);
        // One skerry-mic-list thread, still blocked; no skerry-mic thread at all.
        assert_eq!(listings, 1, "one blocked listing, none stacked");
        assert!(capture.mic.listing.load(Ordering::Acquire));
        drop(hold);
    }

    #[test]
    fn a_listing_reaches_the_card_only_for_its_stream() {
        let mut capture = MediaCapture::<FakeMic, FakeCam>::default();
        let (events, mut events_rx) = mpsc::unbounded_channel();
        let _frames = capture.begin_stream(events);
        capture.on_policy(policy(false), Auth::Authorized);
        assert!(capture.wants_inputs());
        let list = InputList {
            devices: vec![("BuiltIn".into(), Some("MacBook Pro Microphone".into()))],
            default_uid: Some("BuiltIn".into()),
        };
        let sent = list.clone();
        assert!(capture.refresh_inputs_with(move || Some(sent)));
        let event = loop {
            if let Ok(event) = events_rx.try_recv() {
                break event;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let SessionEvent::MicInputs(id, inputs) = event else {
            panic!("expected inputs");
        };
        capture.on_inputs(id, inputs);
        assert_eq!(capture.inputs(), Some(&list));
        // A late answer after the stream ended is dropped.
        capture.stop();
        capture.on_inputs(id, list);
        assert!(capture.inputs().is_none());
        assert!(!capture.wants_inputs());
    }

    #[test]
    fn a_late_status_from_a_stopped_thread_is_ignored() {
        let (mut capture, log, _frames, _events) = capturing();
        capture.stop();
        assert!(capture.on_status(1, live()).is_empty());
        assert_eq!(capture.badge(), None);
        let (events, _rx) = mpsc::unbounded_channel();
        let _frames = capture.begin_stream(events);
        let effects = capture.on_policy(policy(true), Auth::Authorized);
        capture.apply_with(effects, maker(&log, false), no_cam());
        capture.on_status(1, live());
        assert_eq!(capture.badge(), None);
        capture.on_status(2, live());
        assert_eq!(capture.badge(), Some((true, false)));
    }

    #[test]
    fn the_thread_reports_through_the_stream_events() {
        let mut capture = MediaCapture::<FakeMic, FakeCam>::default();
        let (events, mut events_rx) = mpsc::unbounded_channel();
        let _frames = capture.begin_stream(events);
        let mut reporter = None;
        let effects = capture.on_policy(policy(true), Auth::Authorized);
        capture.apply_with(
            effects,
            Box::new(|_, report| {
                reporter = Some(report);
                Some(FakeMic {
                    log: Rc::default(),
                    alive: Arc::default(),
                })
            }),
            no_cam(),
        );
        reporter.unwrap()(live());
        assert!(matches!(
            events_rx.try_recv(),
            Ok(SessionEvent::MicStatus(1, status)) if status.live
        ));
    }

    #[test]
    fn badge_shows_while_device_open() {
        let (mut capture, _log, _frames, _events) = capturing();
        // The thread reports live as soon as the device is open, before any packet.
        capture.on_status(1, live());
        assert_eq!(capture.badge(), Some((true, false)));
        capture.on_status(1, MicStatus::default());
        assert_eq!(capture.badge(), None);
    }

    #[test]
    fn encoder_unavailable_turns_the_switch_off() {
        let (mut capture, log, _frames, _events) = capturing();
        let effects = capture.on_status(
            1,
            MicStatus {
                problem: Some(MicProblem::EncoderUnavailable),
                ..MicStatus::default()
            },
        );
        capture.apply_with(effects, maker(&log, false), no_cam());
        assert!(!capture.state.mic.wanted);
        assert!(capture.encoder_unavailable);
        assert_eq!(log.borrow().dropped, 1);
    }

    #[test]
    fn the_chosen_input_name_is_saved_only_when_it_changes() {
        let devices = [crate::audio::OutputDevice {
            uid: "USB".into(),
            name: "Shure MV7".into(),
        }];
        assert_eq!(
            input_name_to_save(Some("USB"), Some("USB Mic"), &devices).as_deref(),
            Some("Shure MV7")
        );
        assert_eq!(
            input_name_to_save(Some("USB"), Some("Shure MV7"), &devices),
            None
        );
        assert_eq!(input_name_to_save(None, None, &devices), None);
        assert_eq!(input_name_to_save(Some("Gone"), None, &devices), None);
    }

    /// A window's capture with a live stream whose server turned the camera on.
    fn filming() -> (
        MediaCapture<FakeMic, FakeCam>,
        Rc<RefCell<Log>>,
        CameraFrames,
        mpsc::UnboundedReceiver<SessionEvent>,
    ) {
        let mut capture = MediaCapture::<FakeMic, FakeCam>::default();
        let (events, events_rx) = mpsc::unbounded_channel();
        let (_frames, camera) = capture.begin_stream(events);
        let log = Rc::new(RefCell::new(Log::default()));
        let effects = capture.on_policy(cam_policy(true), Auth::Authorized);
        assert!(capture
            .apply_with(effects, no_mic(), cam_maker(&log, false))
            .is_empty());
        assert_eq!(log.borrow().made, 1);
        (capture, log, camera, events_rx)
    }

    fn cam_live() -> CamStatus {
        CamStatus {
            live: true,
            ..CamStatus::default()
        }
    }

    #[test]
    fn the_camera_starts_with_the_servers_size_and_stops_on_every_stream_end() {
        let (mut capture, log, camera, _events) = filming();
        assert_eq!(log.borrow().calls, ["made (1280, 720)"]);
        capture.on_cam_status(1, cam_live());
        assert_eq!(capture.badge(), Some((false, true)));
        capture.on_stream_event(&SessionEvent::Error("closed".into()));
        assert_eq!(log.borrow().dropped, 1, "the camera thread got Shutdown");
        assert_eq!(capture.badge(), None);
        assert!(camera.frames.is_closed(), "the transport's channel closed");
        // The start-up timeout too.
        let (mut capture, log, _camera, _events) = filming();
        capture.on_stream_event(&SessionEvent::StartupTimeout);
        assert_eq!(log.borrow().dropped, 1);
    }

    #[test]
    fn webcam_keyframe_reply_sets_camera_flag() {
        let (mut capture, _log, camera, _events) = filming();
        assert!(!camera.keyframe.load(Ordering::Acquire));
        assert!(capture
            .on_reply(Reply::Keyframe, Auth::Authorized)
            .is_empty());
        assert!(camera.keyframe.load(Ordering::Acquire));
        // WEBCAM_DISABLED stops the camera through the policy.
        let effects = capture.on_reply(Reply::Disabled(Subject::Camera), Auth::Authorized);
        assert_eq!(effects, [Effect::Stop(Subject::Camera)]);
    }

    #[test]
    fn a_new_camera_size_reaches_the_running_camera() {
        let (mut capture, log, _camera, _events) = filming();
        let mut bigger = cam_policy(true);
        bigger.cam_size = (1920, 1080);
        capture.on_policy(bigger, Auth::Authorized);
        assert_eq!(log.borrow().calls.last().unwrap(), "size (1920, 1080)");
        // The same size again: nothing more.
        capture.on_policy(bigger, Auth::Authorized);
        assert_eq!(log.borrow().calls.len(), 2);
    }

    #[test]
    fn a_late_camera_status_is_ignored() {
        let (mut capture, _log, _camera, _events) = filming();
        capture.stop();
        assert!(capture.on_cam_status(1, cam_live()).is_empty());
        assert_eq!(capture.badge(), None);
    }

    #[test]
    fn camera_encoder_unavailable_turns_the_switch_off() {
        let (mut capture, log, _camera, _events) = filming();
        let effects = capture.on_cam_status(
            1,
            CamStatus {
                problem: Some(CamProblem::EncoderUnavailable),
                ..CamStatus::default()
            },
        );
        capture.apply_with(effects, no_mic(), cam_maker(&log, false));
        assert!(!capture.state.cam.wanted);
        assert!(capture.cam_encoder_unavailable);
        assert_eq!(log.borrow().dropped, 1);
    }

    #[test]
    fn a_start_while_the_old_thread_exits_is_retried_from_the_ticker() {
        // M1 review (Round 2): a Start that lands while the previous thread is still
        // exiting was dropped, leaving the switch on with nothing capturing.
        let log = Rc::new(RefCell::new(Log::default()));
        let mut capture = MediaCapture::<FakeMic, FakeCam>::default();
        let (events, _rx) = mpsc::unbounded_channel();
        let _frames = capture.begin_stream(events);
        capture.on_policy(policy(false), Auth::Authorized);
        let effects = capture
            .state
            .on_toggle(Subject::Microphone, true, Auth::Authorized);
        capture.apply_with(effects, maker(&log, true), no_cam());
        assert_eq!(log.borrow().made, 1);
        // Off and quickly on again: the first thread is still in its teardown.
        let stuck = capture.mic.handle.as_ref().unwrap().alive();
        let effects = capture
            .state
            .on_toggle(Subject::Microphone, false, Auth::Authorized);
        capture.apply_with(effects, maker(&log, false), no_cam());
        let effects = capture
            .state
            .on_toggle(Subject::Microphone, true, Auth::Authorized);
        capture.apply_with(effects, maker(&log, false), no_cam());
        assert_eq!(log.borrow().made, 1, "never stacked on a blocked thread");
        assert!(capture.has_pending() && capture.wants_ticks());
        // A tick while it still lingers: still waiting.
        capture.retry_pending_with(maker(&log, false), no_cam());
        assert_eq!(log.borrow().made, 1);
        // The old thread ends: the next tick starts the new one.
        stuck.store(false, Ordering::Release);
        capture.retry_pending_with(maker(&log, false), no_cam());
        assert_eq!(log.borrow().made, 2);
        assert!(!capture.has_pending());
        // Turned off meanwhile: a pending start is dropped, not run.
        let effects = capture
            .state
            .on_toggle(Subject::Microphone, false, Auth::Authorized);
        let stuck = capture.mic.handle.as_ref().unwrap().alive();
        stuck.store(true, Ordering::Release);
        capture.apply_with(effects, maker(&log, false), no_cam());
        let effects = capture
            .state
            .on_toggle(Subject::Microphone, true, Auth::Authorized);
        capture.apply_with(effects, maker(&log, false), no_cam());
        let effects = capture
            .state
            .on_toggle(Subject::Microphone, false, Auth::Authorized);
        capture.apply_with(effects, maker(&log, false), no_cam());
        stuck.store(false, Ordering::Release);
        capture.retry_pending_with(maker(&log, false), no_cam());
        assert_eq!(log.borrow().made, 2);
    }

    #[test]
    fn a_camera_start_on_reconnect_waits_for_the_old_camera_thread() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut capture = MediaCapture::<FakeMic, FakeCam>::default();
        let (events, _rx) = mpsc::unbounded_channel();
        let _frames = capture.begin_stream(events);
        capture.on_policy(cam_policy(false), Auth::Authorized);
        let effects = capture
            .state
            .on_toggle(Subject::Camera, true, Auth::Authorized);
        capture.apply_with(effects, no_mic(), cam_maker(&log, true));
        let stuck = capture.cam.handle.as_ref().unwrap().alive();
        // Reconnect: the person's on carries over, the old thread is still stopping.
        capture.stop();
        let (events, _rx) = mpsc::unbounded_channel();
        let _frames = capture.begin_stream(events);
        let effects = capture.on_policy(cam_policy(false), Auth::Authorized);
        assert_eq!(effects, [Effect::Start(Subject::Camera)]);
        capture.apply_with(effects, no_mic(), cam_maker(&log, false));
        assert_eq!(log.borrow().made, 1);
        stuck.store(false, Ordering::Release);
        capture.retry_pending_with(no_mic(), cam_maker(&log, false));
        assert_eq!(log.borrow().made, 2);
        // A stream end forgets a pending start.
        capture.stop();
        assert!(!capture.has_pending());
    }

    #[test]
    fn a_camera_listing_reaches_the_card_only_for_its_stream() {
        let mut capture = MediaCapture::<FakeMic, FakeCam>::default();
        let (events, mut events_rx) = mpsc::unbounded_channel();
        let _frames = capture.begin_stream(events);
        capture.on_policy(cam_policy(false), Auth::Authorized);
        assert!(capture.wants_cameras() && !capture.wants_inputs());
        let list = InputList {
            devices: vec![("FaceTime".into(), Some("FaceTime HD Camera".into()))],
            default_uid: Some("FaceTime".into()),
        };
        let sent = list.clone();
        assert!(capture.refresh_cameras_with(move || Some(sent)));
        let event = loop {
            if let Ok(event) = events_rx.try_recv() {
                break event;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let SessionEvent::CamInputs(id, cameras) = event else {
            panic!("expected cameras");
        };
        capture.on_cameras(id, cameras);
        assert_eq!(capture.cameras().0[0].name, "FaceTime HD Camera");
        capture.stop();
        capture.on_cameras(id, list);
        assert!(capture.cameras().0.is_empty());
    }
}
