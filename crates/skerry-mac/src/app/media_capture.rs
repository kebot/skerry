//! The session window's microphone (`ARCH-038`): its `skerry-mic` thread, the pure capture
//! policy, the device list for the card, and the one place every stream end stops capture.
//!
//! [`MediaCapture::stop`] runs on every stream end: a server close or network drop
//! (`SessionEvent::Error`), the start-up timeout, Reconnect, window close and quit. A
//! `*_DISABLED` or a lock stops the device through the policy. The thread also stops itself
//! once the stream's frame channel closes, so the device goes off even if an event is missed.
//!
//! Nothing touches the microphone before capture is asked for **and** macOS granted it
//! (M-QA-1): the `skerry-mic` thread is created only by an `Effect::Start` (which the policy
//! raises only once authorised) and dropped by `Effect::Stop`. The card's list comes from
//! plain CoreAudio properties on a short-lived `skerry-mic-list` thread, at most one at a
//! time per window. A thread that blocks is detached, and no new one is stacked on it.

use crate::capture::{self, CapturePolicy, CaptureState, Effect, Subject, Ticket};
use crate::input_devices::InputList;
use crate::microphone::{MicProblem, MicStatus, Microphone};
use crate::transport::SessionEvent;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

/// What the window drives a capture thread with; a seam for tests.
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

/// Makes a capturing thread from the stream's sink and a status reporter.
pub(super) type MakeMic<'a, M> =
    Box<dyn FnOnce(mpsc::Sender<Vec<u8>>, Box<dyn Fn(MicStatus) + Send>) -> Option<M> + 'a>;

pub(super) struct MediaCapture<M: MicHandle = Microphone> {
    pub(super) state: CaptureState,
    microphone: Option<M>,
    /// The last dropped thread's flag: while it is still alive (blocked in a call), no new
    /// thread starts.
    lingering: Option<Arc<AtomicBool>>,
    /// The id the current thread tags its statuses with.
    mic_id: u64,
    next_id: u64,
    pub(super) mic_status: Option<MicStatus>,
    /// The thread reported it can't encode; kept for the card after the thread is gone.
    pub(super) encoder_unavailable: bool,
    /// The inputs from the last listing, for the card.
    pub(super) inputs: Option<InputList>,
    /// A listing is running (one at a time, across reconnects).
    listing: Arc<AtomicBool>,
    /// Re-lists every second while the stream lives; dropped by `stop`.
    pub(super) ticker: Option<gpui_kit::Task<()>>,
    stream_id: u64,
    /// The current stream's frame channel and event sender, for a thread created later.
    sink: Option<mpsc::Sender<Vec<u8>>>,
    events: Option<mpsc::UnboundedSender<SessionEvent>>,
}

impl<M: MicHandle> Default for MediaCapture<M> {
    fn default() -> Self {
        Self {
            state: CaptureState::default(),
            microphone: None,
            lingering: None,
            mic_id: 0,
            next_id: 0,
            mic_status: None,
            encoder_unavailable: false,
            inputs: None,
            listing: Arc::new(AtomicBool::new(false)),
            ticker: None,
            stream_id: 0,
            sink: None,
            events: None,
        }
    }
}

/// Events that end the stream without `end_stream`: they stop capture at once.
fn ends_stream(event: &SessionEvent) -> bool {
    matches!(event, SessionEvent::Error(_) | SessionEvent::StartupTimeout)
}

impl<M: MicHandle> MediaCapture<M> {
    /// A new stream: its frame channel (the receiver goes to the transport).
    pub(super) fn begin_stream(
        &mut self,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> mpsc::Receiver<Vec<u8>> {
        let (sink, frames) = mpsc::channel(capture::MIC_QUEUE);
        self.stream_id += 1;
        self.sink = Some(sink);
        self.events = Some(events);
        frames
    }

    /// The first thing `SessionWindow::handle_event` does with every event: one that ends the
    /// stream stops capture.
    pub(super) fn on_stream_event(&mut self, event: &SessionEvent) {
        if ends_stream(event) {
            self.stop();
        }
    }

    /// Stops every device: the thread gets `Shutdown`, pending permission answers go stale,
    /// and the server's word is forgotten. The person's own choice stays for a Reconnect.
    pub(super) fn stop(&mut self) {
        self.state.on_stream_end();
        self.drop_microphone();
        self.inputs = None;
        self.ticker = None;
        self.sink = None;
        self.events = None;
    }

    fn drop_microphone(&mut self) {
        if let Some(mic) = self.microphone.take() {
            self.lingering = Some(mic.alive());
        }
        self.mic_status = None;
    }

    /// Runs `effects`: a start creates the capturing thread (the policy starts only once
    /// macOS granted it), a stop drops it. Returns the permission prompts to show.
    pub(super) fn apply_with(&mut self, effects: Vec<Effect>, make: MakeMic<'_, M>) -> Vec<Ticket> {
        let mut make = Some(make);
        let mut asks = Vec::new();
        for effect in effects {
            match effect {
                Effect::Start(Subject::Microphone) => {
                    if let Some(make) = make.take() {
                        self.start_microphone(make);
                    }
                }
                Effect::Stop(Subject::Microphone) => self.drop_microphone(),
                Effect::AskPermission(ticket) => asks.push(ticket),
            }
        }
        asks
    }

    fn start_microphone(&mut self, make: MakeMic<'_, M>) {
        if self.microphone.is_some() {
            return;
        }
        // A thread still stuck in a call is never stacked with a new one.
        if self
            .lingering
            .as_ref()
            .is_some_and(|alive| alive.load(Ordering::Acquire))
        {
            return;
        }
        let (Some(sink), Some(events)) = (self.sink.clone(), self.events.clone()) else {
            return;
        };
        self.next_id += 1;
        let id = self.next_id;
        let report = Box::new(move |status| {
            let _ = events.send(SessionEvent::MicStatus(id, status));
        });
        self.encoder_unavailable = false;
        self.microphone = make(sink, report);
        if self.microphone.is_some() {
            self.mic_id = id;
        }
    }

    /// A status from a thread: ignored unless it is the current one's. A thread that can't
    /// encode turns the switch off.
    pub(super) fn on_status(&mut self, id: u64, status: MicStatus) -> Vec<Effect> {
        if id != self.mic_id || self.microphone.is_none() {
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

    pub(super) fn set_input(&self, chosen: Option<String>) {
        if let Some(mic) = &self.microphone {
            mic.set_input(chosen);
        }
    }

    /// The badge shows while the device is open.
    pub(super) fn badge(&self) -> Option<bool> {
        capture::badge(self.mic_status.as_ref().is_some_and(|status| status.live))
    }

    /// A primary `server_settings`.
    pub(super) fn on_policy(&mut self, policy: CapturePolicy, auth: capture::Auth) -> Vec<Effect> {
        self.state.on_policy(policy, auth)
    }

    /// Whether the card wants a device list now: a live stream whose server allows the
    /// microphone.
    pub(super) fn wants_inputs(&self) -> bool {
        self.events.is_some() && self.state.permitted(Subject::Microphone)
    }

    /// Lists inputs with `list` on a `skerry-mic-list` thread, unless one is still running
    /// (blocked or slow): never more than one, however many reconnects. Returns whether a
    /// listing started.
    pub(super) fn refresh_inputs_with(
        &mut self,
        list: impl FnOnce() -> Option<InputList> + Send + 'static,
    ) -> bool {
        let Some(events) = self.events.clone() else {
            return false;
        };
        if self.listing.swap(true, Ordering::AcqRel) {
            return false;
        }
        let listing = self.listing.clone();
        let id = self.stream_id;
        let spawned = std::thread::Builder::new()
            .name("skerry-mic-list".into())
            .spawn(move || {
                let inputs = list();
                listing.store(false, Ordering::Release);
                if let Some(inputs) = inputs {
                    let _ = events.send(SessionEvent::MicInputs(id, inputs));
                }
            });
        if spawned.is_err() {
            self.listing.store(false, Ordering::Release);
        }
        spawned.is_ok()
    }

    /// The inputs the card shows, labelled, and the default's UID: the capturing thread's
    /// own list while it runs, else the last listing.
    pub(super) fn devices(&self) -> (Vec<crate::audio::OutputDevice>, Option<String>) {
        match (&self.mic_status, &self.inputs) {
            (Some(status), _) => (status.devices.clone(), status.default_uid.clone()),
            (None, Some(inputs)) => (
                crate::audio::label_devices_as(inputs.devices.clone(), "Microphone"),
                inputs.default_uid.clone(),
            ),
            (None, None) => (Vec::new(), None),
        }
    }

    /// A listing's answer, if it is this stream's.
    pub(super) fn on_inputs(&mut self, id: u64, inputs: InputList) {
        if id == self.stream_id && self.events.is_some() {
            self.inputs = Some(inputs);
        }
    }
}

impl MediaCapture<Microphone> {
    /// `apply_with` making the real `skerry-mic` thread for `chosen`.
    pub(super) fn apply(&mut self, effects: Vec<Effect>, chosen: Option<String>) -> Vec<Ticket> {
        self.apply_with(
            effects,
            Box::new(move |sink, report| Microphone::new(chosen, sink, report)),
        )
    }

    pub(super) fn refresh_inputs(&mut self) -> bool {
        self.refresh_inputs_with(crate::input_devices::list_inputs)
    }
}

/// The chosen microphone's name to save: when a list (the thread's, or the card's own)
/// shows the device and its name differs from the saved one.
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

    fn policy(on_start: bool) -> CapturePolicy {
        CapturePolicy {
            mic: DevicePolicy {
                permitted: true,
                on_start,
                ..DevicePolicy::default()
            },
        }
    }

    type Rig = (
        MediaCapture<FakeMic>,
        Rc<RefCell<Log>>,
        mpsc::Receiver<Vec<u8>>,
        mpsc::UnboundedReceiver<SessionEvent>,
    );

    /// A window's capture with a live stream whose server turned the microphone on.
    fn capturing() -> Rig {
        let mut capture = MediaCapture::<FakeMic>::default();
        let (events, events_rx) = mpsc::unbounded_channel();
        let frames = capture.begin_stream(events);
        let log = Rc::new(RefCell::new(Log::default()));
        let effects = capture.on_policy(policy(true), Auth::Authorized);
        assert!(capture.apply_with(effects, maker(&log, false)).is_empty());
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
        assert_eq!(capture.badge(), Some(true));
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
        let mut capture = MediaCapture::<FakeMic>::default();
        let (events, _rx) = mpsc::unbounded_channel();
        let _frames = capture.begin_stream(events);
        let log = Rc::new(RefCell::new(Log::default()));
        let effects = capture.on_policy(policy(false), Auth::Authorized);
        capture.apply_with(effects, maker(&log, false));
        assert_eq!(log.borrow().made, 0);
        // Asked for while undetermined: a prompt, still no thread.
        let effects = capture
            .state
            .on_toggle(Subject::Microphone, true, Auth::NotDetermined);
        let asks = capture.apply_with(effects, maker(&log, false));
        assert_eq!(asks.len(), 1);
        assert_eq!(log.borrow().made, 0);
        // Granted: now the thread starts.
        let effects = capture.state.on_auth(asks[0], true);
        capture.apply_with(effects, maker(&log, false));
        assert_eq!(log.borrow().made, 1);
    }

    #[test]
    fn stop_effect_drops_the_thread() {
        let (mut capture, log, _frames, _events) = capturing();
        let effects = capture
            .state
            .on_reply(Reply::Disabled(Subject::Microphone), Auth::Authorized);
        capture.apply_with(effects, maker(&log, false));
        assert_eq!(log.borrow().dropped, 1);
        assert_eq!(capture.badge(), None);
    }

    #[test]
    fn a_blocked_thread_is_never_stacked() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut capture = MediaCapture::<FakeMic>::default();
        for _ in 0..5 {
            // Reconnect: a new stream, the person's on carried over, the old thread stuck.
            let (events, _rx) = mpsc::unbounded_channel();
            let _frames = capture.begin_stream(events);
            let effects = capture.on_policy(policy(true), Auth::Authorized);
            capture.apply_with(effects, maker(&log, true));
            capture.stop();
        }
        assert_eq!(log.borrow().made, 1, "one stuck thread, no more");
    }

    #[test]
    fn reconnects_do_not_grow_threads_while_permission_is_undetermined() {
        // Threads named skerry-mic*, as `ps -M` would show them.
        let threads = || {
            let out = std::process::Command::new("ps")
                .args(["-M", "-p", &std::process::id().to_string()])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).lines().count()
        };
        let log = Rc::new(RefCell::new(Log::default()));
        let mut capture = MediaCapture::<FakeMic>::default();
        // A device listing that blocks for good, as cpal's did while TCC waited.
        let (hold, held) = std::sync::mpsc::channel::<()>();
        let held = Arc::new(std::sync::Mutex::new(held));
        let mut listings = 0;
        let mut before = None;
        for round in 0..10 {
            let (events, _rx) = mpsc::unbounded_channel();
            let _frames = capture.begin_stream(events);
            let effects = capture.on_policy(policy(true), Auth::NotDetermined);
            let asks = capture.apply_with(effects, maker(&log, false));
            assert_eq!(asks.len(), 1, "a prompt, never a thread");
            let held = held.clone();
            if capture.refresh_inputs_with(move || {
                let _ = held.lock().unwrap().recv();
                None
            }) {
                listings += 1;
            }
            if round == 0 {
                std::thread::sleep(Duration::from_millis(50));
                before = Some(threads());
            }
            capture.stop();
        }
        assert_eq!(log.borrow().made, 0);
        assert_eq!(listings, 1, "one blocked listing, none stacked");
        assert!(
            threads() <= before.unwrap(),
            "the thread count did not grow"
        );
        drop(hold);
    }

    #[test]
    fn a_listing_reaches_the_card_only_for_its_stream() {
        let mut capture = MediaCapture::<FakeMic>::default();
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
        assert_eq!(capture.inputs.as_ref(), Some(&list));
        // A late answer after the stream ended is dropped.
        capture.stop();
        capture.on_inputs(id, list);
        assert!(capture.inputs.is_none());
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
        capture.apply_with(effects, maker(&log, false));
        capture.on_status(1, live());
        assert_eq!(capture.badge(), None);
        capture.on_status(2, live());
        assert_eq!(capture.badge(), Some(true));
    }

    #[test]
    fn the_thread_reports_through_the_stream_events() {
        let mut capture = MediaCapture::<FakeMic>::default();
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
        assert_eq!(capture.badge(), Some(true));
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
        capture.apply_with(effects, maker(&log, false));
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
}
