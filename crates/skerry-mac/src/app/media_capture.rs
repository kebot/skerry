//! The session window's microphone (`ARCH-038`): its `skerry-mic` thread, the pure capture
//! policy, and the one place every stream end stops capture.
//!
//! [`MediaCapture::stop`] runs on every stream end: a server close or network drop
//! (`SessionEvent::Error`), the start-up timeout, Reconnect, window close and quit. A
//! `*_DISABLED` or a lock stops the device through the policy. The thread also stops itself
//! once the stream's frame channel closes, so the device goes off even if an event is missed.

use crate::capture::{self, CapturePolicy, CaptureState, Effect, Subject, Ticket};
use crate::microphone::{MicProblem, MicStatus, Microphone};
use crate::transport::SessionEvent;
use tokio::sync::mpsc;

/// What the window drives a capture thread with; a seam for tests.
pub(super) trait MicHandle {
    fn set_capture(&self, on: bool);
    fn set_input(&self, chosen: Option<String>);
}

impl MicHandle for Microphone {
    fn set_capture(&self, on: bool) {
        Microphone::set_capture(self, on);
    }

    fn set_input(&self, chosen: Option<String>) {
        Microphone::set_input(self, chosen);
    }
}

pub(super) struct MediaCapture<M: MicHandle = Microphone> {
    pub(super) state: CaptureState,
    microphone: Option<M>,
    /// The id the current thread tags its statuses with.
    mic_id: u64,
    next_id: u64,
    pub(super) mic_status: Option<MicStatus>,
    /// The current stream's frame channel and event sender, for a thread created later.
    sink: Option<mpsc::Sender<Vec<u8>>>,
    events: Option<mpsc::UnboundedSender<SessionEvent>>,
}

impl<M: MicHandle> Default for MediaCapture<M> {
    fn default() -> Self {
        Self {
            state: CaptureState::default(),
            microphone: None,
            mic_id: 0,
            next_id: 0,
            mic_status: None,
            sink: None,
            events: None,
        }
    }
}

/// Events that end the stream without `end_stream`: they stop capture at once.
pub(super) fn ends_stream(event: &SessionEvent) -> bool {
    matches!(event, SessionEvent::Error(_) | SessionEvent::StartupTimeout)
}

impl<M: MicHandle> MediaCapture<M> {
    /// A new stream: its frame channel (the receiver goes to the transport).
    pub(super) fn begin_stream(
        &mut self,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> mpsc::Receiver<Vec<u8>> {
        let (sink, frames) = mpsc::channel(capture::MIC_QUEUE);
        self.sink = Some(sink);
        self.events = Some(events);
        frames
    }

    /// Stops every device: the thread gets `Shutdown`, pending permission answers go stale,
    /// and the server's word is forgotten. The person's own choice stays for a Reconnect.
    pub(super) fn stop(&mut self) {
        self.state.on_stream_end();
        self.microphone = None;
        self.mic_status = None;
        self.sink = None;
        self.events = None;
    }

    /// Runs `effects` on the thread; returns the permission prompts to show.
    pub(super) fn apply(&mut self, effects: Vec<Effect>) -> Vec<Ticket> {
        let mut asks = Vec::new();
        for effect in effects {
            match effect {
                Effect::Start(Subject::Microphone) => {
                    if let Some(mic) = &self.microphone {
                        mic.set_capture(true);
                    }
                }
                Effect::Stop(Subject::Microphone) => {
                    if let Some(mic) = &self.microphone {
                        mic.set_capture(false);
                    }
                }
                Effect::AskPermission(ticket) => asks.push(ticket),
            }
        }
        asks
    }

    /// A status from a thread: ignored unless it is the current one's. A thread that can't
    /// encode turns the switch off.
    pub(super) fn on_status(&mut self, id: u64, status: MicStatus) -> Vec<Effect> {
        if id != self.mic_id || self.microphone.is_none() {
            return Vec::new();
        }
        let unavailable = status.problem == Some(MicProblem::EncoderUnavailable);
        self.mic_status = Some(status);
        if unavailable && self.state.mic.wanted {
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

    /// Attaches a thread made by `make` (given the stream's sink and a status reporter)
    /// when there is none yet and the stream is live.
    fn ensure_microphone(
        &mut self,
        make: impl FnOnce(mpsc::Sender<Vec<u8>>, Box<dyn Fn(MicStatus) + Send>) -> Option<M>,
    ) {
        if self.microphone.is_some() {
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
        self.microphone = make(sink, report);
        if self.microphone.is_some() {
            self.mic_id = id;
        }
    }

    /// A primary `server_settings`: the thread starts (device listing only) once the server
    /// allows the microphone, then the policy runs.
    pub(super) fn on_policy_with(
        &mut self,
        policy: CapturePolicy,
        auth: capture::Auth,
        make: impl FnOnce(mpsc::Sender<Vec<u8>>, Box<dyn Fn(MicStatus) + Send>) -> Option<M>,
    ) -> Vec<Effect> {
        if policy.mic.permitted {
            self.ensure_microphone(make);
        }
        self.state.on_policy(policy, auth)
    }
}

impl MediaCapture<Microphone> {
    pub(super) fn on_policy(
        &mut self,
        policy: CapturePolicy,
        chosen: Option<String>,
    ) -> Vec<Effect> {
        let auth = crate::permissions::authorization(Subject::Microphone);
        self.on_policy_with(policy, auth, move |sink, report| {
            Microphone::new(chosen, sink, report)
        })
    }
}

/// The chosen microphone's name to save: when the status answers that choice, lists the
/// device, and its name differs from the saved one.
pub(super) fn input_name_to_save(
    chosen: Option<&str>,
    saved: Option<&str>,
    status: &MicStatus,
) -> Option<String> {
    let chosen = chosen?;
    if status.chosen.as_deref() != Some(chosen) {
        return None;
    }
    let name = &status.device(chosen)?.name;
    (saved != Some(name.as_str())).then(|| name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Auth, DevicePolicy, Phase, Reply};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct Log {
        calls: Vec<String>,
        dropped: bool,
    }

    struct FakeMic(Rc<RefCell<Log>>);

    impl MicHandle for FakeMic {
        fn set_capture(&self, on: bool) {
            self.0.borrow_mut().calls.push(format!("capture {on}"));
        }
        fn set_input(&self, chosen: Option<String>) {
            self.0.borrow_mut().calls.push(format!("input {chosen:?}"));
        }
    }

    impl Drop for FakeMic {
        fn drop(&mut self) {
            self.0.borrow_mut().dropped = true;
        }
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

    /// A window's capture with a live stream whose server turned the microphone on.
    type Rig = (
        MediaCapture<FakeMic>,
        Rc<RefCell<Log>>,
        mpsc::Receiver<Vec<u8>>,
        mpsc::UnboundedReceiver<SessionEvent>,
    );

    fn capturing() -> Rig {
        let mut capture = MediaCapture::<FakeMic>::default();
        let (events, events_rx) = mpsc::unbounded_channel();
        let frames = capture.begin_stream(events);
        let log = Rc::new(RefCell::new(Log::default()));
        let made = log.clone();
        let effects = capture.on_policy_with(policy(true), Auth::Authorized, move |_, _| {
            Some(FakeMic(made))
        });
        assert!(capture.apply(effects).is_empty());
        assert_eq!(log.borrow().calls, ["capture true"]);
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
        assert!(ends_stream(&SessionEvent::Error("closed".into())));
        let (mut capture, log, _frames, _events) = capturing();
        capture.on_status(1, live());
        assert_eq!(capture.badge(), Some(true));
        capture.stop();
        assert!(log.borrow().dropped, "the thread got Shutdown");
        assert_eq!(capture.badge(), None);
        assert_eq!(capture.state.mic.phase, Phase::Off);
    }

    #[test]
    fn startup_timeout_stops_capture() {
        assert!(ends_stream(&SessionEvent::StartupTimeout));
        assert!(!ends_stream(&SessionEvent::Video(true)));
        let (mut capture, log, _frames, _events) = capturing();
        capture.stop();
        assert!(log.borrow().dropped);
    }

    #[test]
    fn stop_closes_the_frame_channel() {
        // Close, Reconnect and quit all end in `stop`: the transport's channel then closes,
        // which also stops a thread that is still running (`engine_stops_when_sink_closed`).
        let (mut capture, _log, mut frames, _events) = capturing();
        capture.stop();
        assert!(frames.try_recv().is_err());
        assert!(frames.is_closed());
    }

    #[test]
    fn a_late_status_from_a_stopped_thread_is_ignored() {
        let (mut capture, _log, _frames, _events) = capturing();
        capture.stop();
        assert!(capture.on_status(1, live()).is_empty());
        assert_eq!(capture.badge(), None);
        // The next stream's thread has a new id; the old one's statuses still don't count.
        let (events, _rx) = mpsc::unbounded_channel();
        let _frames = capture.begin_stream(events);
        let log = Rc::new(RefCell::new(Log::default()));
        let made = log.clone();
        capture.on_policy_with(policy(false), Auth::Authorized, move |_, _| {
            Some(FakeMic(made))
        });
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
        capture.on_policy_with(policy(false), Auth::Authorized, |_, report| {
            reporter = Some(report);
            Some(FakeMic(Rc::default()))
        });
        reporter.unwrap()(live());
        assert!(matches!(
            events_rx.try_recv(),
            Ok(SessionEvent::MicStatus(1, status)) if status.live
        ));
    }

    #[test]
    fn badge_shows_while_device_open() {
        let (mut capture, _log, _frames, _events) = capturing();
        // Open but no packet delivered yet: the badge already shows.
        assert_eq!(capture.badge(), None, "no status yet");
        capture.on_status(1, live());
        assert_eq!(capture.badge(), Some(true));
        capture.on_status(1, MicStatus::default());
        assert_eq!(capture.badge(), None);
    }

    #[test]
    fn disabled_reply_stops_the_thread_capture() {
        let (mut capture, log, _frames, _events) = capturing();
        let effects = capture
            .state
            .on_reply(Reply::Disabled(Subject::Microphone), Auth::Authorized);
        capture.apply(effects);
        assert_eq!(log.borrow().calls, ["capture true", "capture false"]);
    }

    #[test]
    fn no_thread_until_the_server_allows_the_microphone() {
        let mut capture = MediaCapture::<FakeMic>::default();
        let (events, _rx) = mpsc::unbounded_channel();
        let _frames = capture.begin_stream(events);
        let mut made = false;
        capture.on_policy_with(CapturePolicy::default(), Auth::Authorized, |_, _| {
            made = true;
            None
        });
        assert!(!made);
        // And none without a live stream at all.
        let mut capture = MediaCapture::<FakeMic>::default();
        capture.on_policy_with(policy(true), Auth::Authorized, |_, _| {
            made = true;
            None
        });
        assert!(!made);
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
        capture.apply(effects);
        assert!(!capture.state.mic.wanted);
        assert_eq!(log.borrow().calls, ["capture true", "capture false"]);
    }

    #[test]
    fn the_chosen_input_name_is_saved_only_when_it_changes() {
        let status = MicStatus {
            devices: vec![crate::audio::OutputDevice {
                uid: "USB".into(),
                name: "Shure MV7".into(),
            }],
            chosen: Some("USB".into()),
            ..MicStatus::default()
        };
        assert_eq!(
            input_name_to_save(Some("USB"), Some("USB Mic"), &status).as_deref(),
            Some("Shure MV7")
        );
        assert_eq!(
            input_name_to_save(Some("USB"), Some("Shure MV7"), &status),
            None
        );
        assert_eq!(input_name_to_save(None, None, &status), None);
    }
}
