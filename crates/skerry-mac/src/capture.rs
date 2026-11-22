//! Media capture policy for the uplink (`ARCH-038`): what the server allows, what the person
//! chose, and when a capture device starts or stops. Pure: no objc, no cpal.
//!
//! The rules are the Selkies web client's (715df7c, `selkies-ws-core.ts`):
//! - The start rule (`*_on_start` true, `*_on_demand` false, permitted, untouched) runs only on
//!   a stream's **first** primary `server_settings`. Later ones only apply locks.
//! - `CAPTURE_DEMAND <subject> <0|1>` is obeyed only for a device the person hasn't touched
//!   and that hasn't been refused.
//! - A switch the person flips wins for the rest of the window's life (Reconnect included).
//!   Nothing about on/off is saved to disk.
//! - `*_DISABLED` stops the device and blocks it until the next `server_settings`.
//!
//! Every macOS permission answer carries a ticket (stream generation and request id); a late
//! answer that no longer matches starts nothing.
//!
//! M1 ships the microphone only. Each device has its own [`DeviceState`] and
//! [`DevicePolicy`], so the camera adds one more of each.

use serde_json::Value;

/// A capture device the session can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    Microphone,
}

impl Subject {
    /// The name in `CAPTURE_DEMAND`.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Microphone => "microphone",
        }
    }
}

/// What the server says about one device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DevicePolicy {
    /// The server accepts this device's frames.
    pub permitted: bool,
    /// `*_enabled` is locked: the person can't flip the switch, on or off.
    pub locked: bool,
    /// Session audio is off, so the microphone can't be used (microphone only).
    pub audio_off: bool,
    pub on_start: bool,
    pub on_demand: bool,
}

/// The capture part of `server_settings`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapturePolicy {
    pub mic: DevicePolicy,
}

/// `{value: true}`, `{value: "true"}` or a bare `true`.
fn truthy(setting: &Value) -> bool {
    let value = setting.get("value").unwrap_or(setting);
    value.as_bool() == Some(true) || value.as_str() == Some("true")
}

impl CapturePolicy {
    /// From `server_settings.settings`. The server's own gate: microphone frames are refused
    /// only when `!(audio_enabled && (microphone_enabled || !locked))`, so an unlocked false
    /// still accepts frames; it only means "not on by default".
    pub fn from_server(settings: &Value) -> Self {
        let enabled = &settings["microphone_enabled"];
        let locked = enabled["locked"].as_bool() == Some(true);
        let on = truthy(enabled);
        let audio_off = settings["audio_enabled"]["value"].as_bool() == Some(false);
        let start = &settings["microphone_on_start"];
        Self {
            mic: DevicePolicy {
                permitted: !audio_off && (on || !locked),
                locked,
                audio_off,
                on_start: truthy(start),
                on_demand: truthy(&settings["microphone_on_demand"])
                    || start["value"].as_str() == Some("demand"),
            },
        }
    }
}

/// A capture reply from the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// `MICROPHONE_DISABLED`: the server refused the frames.
    Disabled(Subject),
    /// `CAPTURE_DEMAND <subject> <0|1>`: an app in the session opened or closed the device.
    Demand(Subject, bool),
}

/// The exact literals; anything else is not a capture reply.
pub fn server_reply(text: &str) -> Option<Reply> {
    if text == "MICROPHONE_DISABLED" {
        return Some(Reply::Disabled(Subject::Microphone));
    }
    let rest = text.strip_prefix("CAPTURE_DEMAND ")?;
    let (subject, value) = rest.split_once(' ')?;
    let subject = [Subject::Microphone]
        .into_iter()
        .find(|known| known.wire() == subject)?;
    Some(Reply::Demand(subject, value == "1"))
}

/// macOS's answer for a device (`AVCaptureDevice authorizationStatusForMediaType:`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auth {
    NotDetermined,
    /// Denied or Restricted.
    Denied,
    Authorized,
}

/// Where a device is, as the window drives it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Off,
    /// The macOS prompt is up.
    Asking,
    /// The capture thread was told to capture.
    On,
}

/// One permission request, so a late answer can be told from the current one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub subject: Subject,
    generation: u64,
    request: u64,
    /// The server asked for this start (`on_start` or demand), not the person.
    pub by_server: bool,
}

/// What the window does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Start(Subject),
    Stop(Subject),
    AskPermission(Ticket),
}

/// One device's state in this window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceState {
    /// The person flipped the switch: the server's start rules and demand no longer apply.
    pub touched: bool,
    /// The device should be capturing.
    pub wanted: bool,
    /// The current start came from the server (on_start or demand).
    pub by_server: bool,
    /// A start the server asked for was refused by macOS: demand is ignored until a toggle.
    pub refused: bool,
    /// The person's own start was refused by macOS: the card says so.
    pub denied: bool,
    /// `*_DISABLED` arrived; cleared by the next `server_settings` or stream.
    pub server_disabled: bool,
    pub phase: Phase,
    request: u64,
}

/// Every device's state, the policy of the current stream, and its generation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureState {
    pub mic: DeviceState,
    pub policy: Option<CapturePolicy>,
    generation: u64,
}

impl CaptureState {
    fn device(&mut self, subject: Subject) -> &mut DeviceState {
        match subject {
            Subject::Microphone => &mut self.mic,
        }
    }

    fn device_policy(&self, subject: Subject) -> Option<DevicePolicy> {
        self.policy.map(|policy| match subject {
            Subject::Microphone => policy.mic,
        })
    }

    /// The server accepts this device now.
    pub fn permitted(&self, subject: Subject) -> bool {
        let state = match subject {
            Subject::Microphone => &self.mic,
        };
        self.device_policy(subject)
            .is_some_and(|policy| policy.permitted)
            && !state.server_disabled
    }

    /// Turns the device on, or asks first.
    fn start(&mut self, subject: Subject, by_server: bool, auth: Auth) -> Vec<Effect> {
        let generation = self.generation;
        let device = self.device(subject);
        device.wanted = true;
        device.by_server = by_server;
        match auth {
            Auth::Authorized => {
                device.phase = Phase::On;
                vec![Effect::Start(subject)]
            }
            Auth::NotDetermined => {
                device.phase = Phase::Asking;
                device.request += 1;
                vec![Effect::AskPermission(Ticket {
                    subject,
                    generation,
                    request: device.request,
                    by_server,
                })]
            }
            Auth::Denied => {
                device.refuse(by_server);
                Vec::new()
            }
        }
    }

    /// Turns the device off.
    fn stop(&mut self, subject: Subject) -> Vec<Effect> {
        let device = self.device(subject);
        device.wanted = false;
        device.by_server = false;
        let was = std::mem::take(&mut device.phase);
        if was == Phase::On {
            vec![Effect::Stop(subject)]
        } else {
            Vec::new()
        }
    }

    /// A primary `server_settings`. The start rule runs only on the stream's first; later
    /// ones apply locks only. A person's choice carried over a Reconnect starts again here.
    pub fn on_policy(&mut self, policy: CapturePolicy, mic_auth: Auth) -> Vec<Effect> {
        let first = self.policy.is_none();
        self.policy = Some(policy);
        self.mic.server_disabled = false;
        let subject = Subject::Microphone;
        let device = self.mic;
        let rules = policy.mic;
        if !self.permitted(subject) {
            return self.stop(subject);
        }
        if !first || device.phase != Phase::Off {
            return Vec::new();
        }
        if device.touched {
            if device.wanted {
                return self.start(subject, false, mic_auth);
            }
            return Vec::new();
        }
        if rules.on_start && !rules.on_demand {
            return self.start(subject, true, mic_auth);
        }
        Vec::new()
    }

    /// The person flipped the switch.
    pub fn on_toggle(&mut self, subject: Subject, on: bool, auth: Auth) -> Vec<Effect> {
        let permitted = self.permitted(subject);
        let device = self.device(subject);
        device.touched = true;
        device.refused = false;
        if !on {
            return self.stop(subject);
        }
        device.denied = false;
        if !permitted || device.phase != Phase::Off {
            return Vec::new();
        }
        self.start(subject, false, auth)
    }

    pub fn on_reply(&mut self, reply: Reply, auth: Auth) -> Vec<Effect> {
        match reply {
            Reply::Disabled(subject) => {
                self.device(subject).server_disabled = true;
                self.stop(subject)
            }
            Reply::Demand(subject, true) => {
                let permitted = self.permitted(subject);
                let device = *self.device(subject);
                if device.touched || device.refused || !permitted || device.phase != Phase::Off {
                    return Vec::new();
                }
                self.start(subject, true, auth)
            }
            Reply::Demand(subject, false) => {
                let device = *self.device(subject);
                if device.touched || !device.by_server {
                    return Vec::new();
                }
                self.stop(subject)
            }
        }
    }

    /// macOS answered a permission request. A ticket from an older stream or request, or for
    /// a device no longer wanted, waiting or permitted, starts nothing.
    pub fn on_auth(&mut self, ticket: Ticket, granted: bool) -> Vec<Effect> {
        let permitted = self.permitted(ticket.subject);
        let generation = self.generation;
        let device = self.device(ticket.subject);
        let current = ticket.generation == generation
            && ticket.request == device.request
            && device.phase == Phase::Asking
            && device.wanted;
        if !current {
            return Vec::new();
        }
        if !permitted {
            device.phase = Phase::Off;
            device.wanted = false;
            return Vec::new();
        }
        if granted {
            device.phase = Phase::On;
            vec![Effect::Start(ticket.subject)]
        } else {
            device.phase = Phase::Off;
            device.refuse(ticket.by_server);
            Vec::new()
        }
    }

    /// The capture thread can't encode: the device goes off.
    pub fn on_unavailable(&mut self, subject: Subject) -> Vec<Effect> {
        self.stop(subject)
    }

    /// The stream ended (disconnect, error, start-up timeout, Reconnect, close, quit). Every
    /// device goes off, pending permission answers go stale, and the server's word is
    /// forgotten. The person's own choice stays for a Reconnect.
    pub fn on_stream_end(&mut self) -> Vec<Effect> {
        self.generation += 1;
        self.policy = None;
        let device = &mut self.mic;
        let was = std::mem::take(&mut device.phase);
        device.server_disabled = false;
        device.refused = false;
        if device.by_server {
            device.wanted = false;
            device.by_server = false;
        }
        if was == Phase::On {
            vec![Effect::Stop(Subject::Microphone)]
        } else {
            Vec::new()
        }
    }
}

impl DeviceState {
    fn refuse(&mut self, by_server: bool) {
        self.wanted = false;
        self.by_server = false;
        self.phase = Phase::Off;
        if by_server {
            self.refused = true;
        } else {
            self.denied = true;
        }
    }
}

/// A microphone uplink frame: `0x02` and one bare Opus packet.
pub fn mic_frame(packet: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(packet.len() + 1);
    frame.push(0x02);
    frame.extend_from_slice(packet);
    frame
}

/// Microphone frames waiting for the socket: 200 ms. A full queue drops the packet.
pub const MIC_QUEUE: usize = 10;

/// The in-window badge: `Some(microphone)` only while a device is open.
pub fn badge(mic_open: bool) -> Option<bool> {
    mic_open.then_some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MIC: Subject = Subject::Microphone;

    fn policy(on_start: bool, on_demand: bool) -> CapturePolicy {
        CapturePolicy {
            mic: DevicePolicy {
                permitted: true,
                locked: false,
                audio_off: false,
                on_start,
                on_demand,
            },
        }
    }

    fn locked_off() -> CapturePolicy {
        CapturePolicy {
            mic: DevicePolicy {
                permitted: false,
                locked: true,
                ..DevicePolicy::default()
            },
        }
    }

    fn ticket(effects: &[Effect]) -> Ticket {
        match effects {
            [Effect::AskPermission(ticket)] => *ticket,
            other => panic!("expected one ask, got {other:?}"),
        }
    }

    #[test]
    fn policy_reads_selkies_payload_shape() {
        let policy = CapturePolicy::from_server(&json!({
            "microphone_enabled": {"value": false, "locked": false},
            "microphone_on_start": {"value": true},
            "microphone_on_demand": {"value": false},
            "audio_enabled": {"value": true}
        }));
        assert!(policy.mic.permitted && policy.mic.on_start && !policy.mic.on_demand);
        assert!(!policy.mic.locked && !policy.mic.audio_off);
        // The raw string forms some servers send.
        let policy = CapturePolicy::from_server(&json!({
            "microphone_on_start": {"value": "demand"}
        }));
        assert!(policy.mic.on_demand && !policy.mic.on_start);
    }

    #[test]
    fn unlocked_false_still_permits_microphone() {
        let policy = CapturePolicy::from_server(&json!({
            "microphone_enabled": {"value": false, "locked": false}
        }));
        assert!(policy.mic.permitted);
        assert!(CapturePolicy::from_server(&json!({})).mic.permitted);
    }

    #[test]
    fn locked_off_forbids_and_marks_locked() {
        let policy = CapturePolicy::from_server(&json!({
            "microphone_enabled": {"value": false, "locked": true}
        }));
        assert!(!policy.mic.permitted && policy.mic.locked);
    }

    #[test]
    fn locked_on_mic_switch_disabled() {
        let policy = CapturePolicy::from_server(&json!({
            "microphone_enabled": {"value": true, "locked": true}
        }));
        assert!(policy.mic.permitted && policy.mic.locked);
    }

    #[test]
    fn audio_disabled_forbids_microphone() {
        let policy = CapturePolicy::from_server(&json!({
            "microphone_enabled": {"value": true, "locked": false},
            "audio_enabled": {"value": false}
        }));
        assert!(!policy.mic.permitted && policy.mic.audio_off);
    }

    #[test]
    fn server_reply_parses_exact_literals() {
        assert_eq!(
            server_reply("MICROPHONE_DISABLED"),
            Some(Reply::Disabled(MIC))
        );
        assert_eq!(
            server_reply("CAPTURE_DEMAND microphone 1"),
            Some(Reply::Demand(MIC, true))
        );
        assert_eq!(
            server_reply("CAPTURE_DEMAND microphone 0"),
            Some(Reply::Demand(MIC, false))
        );
        assert_eq!(
            server_reply("CAPTURE_DEMAND microphone yes"),
            Some(Reply::Demand(MIC, false))
        );
        // The camera is not built yet (M2): its replies are not ours.
        assert_eq!(server_reply("CAPTURE_DEMAND webcam 1"), None);
        assert_eq!(server_reply("CAPTURE_DEMAND speaker 1"), None);
        assert_eq!(server_reply("CAPTURE_DEMAND_X microphone 1"), None);
        assert_eq!(server_reply("MICROPHONE_DISABLED "), None);
        assert_eq!(server_reply("WEBCAM_KEYFRAME"), None);
    }

    #[test]
    fn ready_starts_only_untouched_on_start_not_demand() {
        let mut state = CaptureState::default();
        assert_eq!(
            state.on_policy(policy(true, false), Auth::Authorized),
            [Effect::Start(MIC)]
        );
        assert!(state.mic.by_server);
        let mut state = CaptureState::default();
        assert!(state
            .on_policy(policy(true, true), Auth::Authorized)
            .is_empty());
        let mut state = CaptureState::default();
        assert!(state
            .on_policy(policy(false, false), Auth::Authorized)
            .is_empty());
        // Touched off before a reconnect: the server's on_start no longer applies.
        let mut state = CaptureState::default();
        state.on_toggle(MIC, false, Auth::Authorized);
        assert!(state
            .on_policy(policy(true, false), Auth::Authorized)
            .is_empty());
    }

    #[test]
    fn ready_asks_permission_when_not_determined() {
        let mut state = CaptureState::default();
        let effects = state.on_policy(policy(true, false), Auth::NotDetermined);
        let ticket = ticket(&effects);
        assert!(ticket.by_server);
        assert_eq!(state.mic.phase, Phase::Asking);
        assert_eq!(state.on_auth(ticket, true), [Effect::Start(MIC)]);
        assert_eq!(state.mic.phase, Phase::On);
    }

    #[test]
    fn live_server_settings_does_not_rerun_start_policy() {
        let mut state = CaptureState::default();
        assert!(state
            .on_policy(policy(false, false), Auth::Authorized)
            .is_empty());
        // A live change turns on_start on: the web runs the start rule only once.
        assert!(state
            .on_policy(policy(true, false), Auth::Authorized)
            .is_empty());
        assert_eq!(state.mic.phase, Phase::Off);
        // Started by the server, then a live settings echo: still on, not restarted.
        let mut state = CaptureState::default();
        state.on_policy(policy(true, false), Auth::Authorized);
        assert!(state
            .on_policy(policy(true, false), Auth::Authorized)
            .is_empty());
        assert_eq!(state.mic.phase, Phase::On);
    }

    #[test]
    fn demand_one_starts_untouched_device() {
        let mut state = CaptureState::default();
        state.on_policy(policy(false, true), Auth::Authorized);
        assert_eq!(
            state.on_reply(Reply::Demand(MIC, true), Auth::Authorized),
            [Effect::Start(MIC)]
        );
        // A repeat while on does nothing.
        assert!(state
            .on_reply(Reply::Demand(MIC, true), Auth::Authorized)
            .is_empty());
    }

    #[test]
    fn demand_ignored_after_user_toggle() {
        let mut state = CaptureState::default();
        state.on_policy(policy(false, true), Auth::Authorized);
        state.on_toggle(MIC, false, Auth::Authorized);
        assert!(state
            .on_reply(Reply::Demand(MIC, true), Auth::Authorized)
            .is_empty());
        // Turned on by the person: demand 0 doesn't turn it off.
        state.on_toggle(MIC, true, Auth::Authorized);
        assert!(state
            .on_reply(Reply::Demand(MIC, false), Auth::Authorized)
            .is_empty());
        assert_eq!(state.mic.phase, Phase::On);
    }

    #[test]
    fn demand_zero_stops_server_started_device_only() {
        let mut state = CaptureState::default();
        state.on_policy(policy(false, true), Auth::Authorized);
        state.on_reply(Reply::Demand(MIC, true), Auth::Authorized);
        assert_eq!(
            state.on_reply(Reply::Demand(MIC, false), Auth::Authorized),
            [Effect::Stop(MIC)]
        );
        assert_eq!(state.mic.phase, Phase::Off);
        // Never started: nothing to stop.
        assert!(state
            .on_reply(Reply::Demand(MIC, false), Auth::Authorized)
            .is_empty());
    }

    #[test]
    fn demand_refusal_latches_until_toggle() {
        let mut state = CaptureState::default();
        state.on_policy(policy(false, true), Auth::Denied);
        assert!(state
            .on_reply(Reply::Demand(MIC, true), Auth::Denied)
            .is_empty());
        assert!(state.mic.refused && !state.mic.denied);
        // Allowed in System Settings since: the latch still holds until the person toggles.
        assert!(state
            .on_reply(Reply::Demand(MIC, true), Auth::Authorized)
            .is_empty());
        state.on_toggle(MIC, false, Auth::Authorized);
        assert!(!state.mic.refused);
    }

    #[test]
    fn disabled_reply_stops_and_blocks_until_next_settings() {
        let mut state = CaptureState::default();
        state.on_policy(policy(true, false), Auth::Authorized);
        assert_eq!(
            state.on_reply(Reply::Disabled(MIC), Auth::Authorized),
            [Effect::Stop(MIC)]
        );
        assert!(!state.permitted(MIC));
        assert!(state.on_toggle(MIC, true, Auth::Authorized).is_empty());
        assert!(state
            .on_reply(Reply::Demand(MIC, true), Auth::Authorized)
            .is_empty());
        // The next settings clear it; nothing restarts on its own.
        assert!(state
            .on_policy(policy(true, false), Auth::Authorized)
            .is_empty());
        assert!(state.permitted(MIC));
        assert_eq!(
            state.on_toggle(MIC, true, Auth::Authorized),
            [Effect::Start(MIC)]
        );
    }

    #[test]
    fn disabled_while_asking_never_starts() {
        let mut state = CaptureState::default();
        let ticket = ticket(&state.on_policy(policy(true, false), Auth::NotDetermined));
        assert!(state
            .on_reply(Reply::Disabled(MIC), Auth::NotDetermined)
            .is_empty());
        assert!(state.on_auth(ticket, true).is_empty());
        assert_eq!(state.mic.phase, Phase::Off);
    }

    #[test]
    fn lock_in_live_settings_stops_capture() {
        let mut state = CaptureState::default();
        state.on_policy(policy(true, false), Auth::Authorized);
        assert_eq!(
            state.on_policy(locked_off(), Auth::Authorized),
            [Effect::Stop(MIC)]
        );
        assert_eq!(state.mic.phase, Phase::Off);
    }

    #[test]
    fn stream_end_keeps_user_choice_for_reconnect() {
        let mut state = CaptureState::default();
        state.on_policy(policy(false, false), Auth::Authorized);
        state.on_toggle(MIC, true, Auth::Authorized);
        assert_eq!(state.on_stream_end(), [Effect::Stop(MIC)]);
        assert_eq!(state.mic.phase, Phase::Off);
        assert!(state.mic.touched && state.mic.wanted);
        // The reconnected stream's first settings start it again, without a prompt.
        assert_eq!(
            state.on_policy(policy(false, false), Auth::Authorized),
            [Effect::Start(MIC)]
        );
    }

    #[test]
    fn reconnect_restarts_only_wanted_devices() {
        // Started by the server's on_start: the new stream's own rules decide.
        let mut state = CaptureState::default();
        state.on_policy(policy(true, false), Auth::Authorized);
        state.on_stream_end();
        assert!(!state.mic.wanted);
        assert!(state
            .on_policy(policy(false, false), Auth::Authorized)
            .is_empty());
        // Turned off by the person: stays off whatever the server says.
        let mut state = CaptureState::default();
        state.on_policy(policy(true, false), Auth::Authorized);
        state.on_toggle(MIC, false, Auth::Authorized);
        state.on_stream_end();
        assert!(state
            .on_policy(policy(true, false), Auth::Authorized)
            .is_empty());
    }

    #[test]
    fn stream_end_clears_server_disabled_but_keeps_touched() {
        let mut state = CaptureState::default();
        state.on_policy(policy(false, false), Auth::Authorized);
        state.on_toggle(MIC, true, Auth::Authorized);
        state.on_reply(Reply::Disabled(MIC), Auth::Authorized);
        assert!(state.mic.server_disabled);
        state.on_stream_end();
        assert!(!state.mic.server_disabled && state.mic.touched);
        assert!(state.policy.is_none());
    }

    #[test]
    fn user_denied_shows_problem_not_latch() {
        let mut state = CaptureState::default();
        state.on_policy(policy(false, false), Auth::Denied);
        assert!(state.on_toggle(MIC, true, Auth::Denied).is_empty());
        assert!(state.mic.denied && !state.mic.refused);
        assert_eq!(state.mic.phase, Phase::Off);
        assert!(!state.mic.wanted);
        // Asked and declined at the prompt: the same.
        let mut state = CaptureState::default();
        state.on_policy(policy(false, false), Auth::NotDetermined);
        let ticket = ticket(&state.on_toggle(MIC, true, Auth::NotDetermined));
        assert!(state.on_auth(ticket, false).is_empty());
        assert!(state.mic.denied && !state.mic.refused);
    }

    #[test]
    fn toggle_on_after_denied_rereads_authorization() {
        let mut state = CaptureState::default();
        state.on_policy(policy(false, false), Auth::Denied);
        state.on_toggle(MIC, true, Auth::Denied);
        assert!(state.mic.denied);
        // Allowed in System Settings meanwhile: the next toggle starts it.
        assert_eq!(
            state.on_toggle(MIC, true, Auth::Authorized),
            [Effect::Start(MIC)]
        );
        assert!(!state.mic.denied);
    }

    #[test]
    fn auth_after_stream_end_does_not_start() {
        let mut state = CaptureState::default();
        state.on_policy(policy(false, false), Auth::NotDetermined);
        let ticket = ticket(&state.on_toggle(MIC, true, Auth::NotDetermined));
        state.on_stream_end();
        // Even once the reconnected stream asks again, the old answer is stale.
        state.on_policy(policy(false, false), Auth::NotDetermined);
        assert!(state.on_auth(ticket, true).is_empty());
        assert_eq!(state.mic.phase, Phase::Asking);
    }

    #[test]
    fn auth_after_disabled_does_not_start() {
        let mut state = CaptureState::default();
        state.on_policy(policy(false, false), Auth::NotDetermined);
        let ticket = ticket(&state.on_toggle(MIC, true, Auth::NotDetermined));
        state.on_reply(Reply::Disabled(MIC), Auth::NotDetermined);
        assert!(state.on_auth(ticket, true).is_empty());
        assert_eq!(state.mic.phase, Phase::Off);
    }

    #[test]
    fn auth_after_toggle_off_does_not_start() {
        let mut state = CaptureState::default();
        state.on_policy(policy(false, false), Auth::NotDetermined);
        let first = ticket(&state.on_toggle(MIC, true, Auth::NotDetermined));
        state.on_toggle(MIC, false, Auth::NotDetermined);
        assert!(state.on_auth(first, true).is_empty());
        // Off and on again: only the newest request's answer counts.
        let second = ticket(&state.on_toggle(MIC, true, Auth::NotDetermined));
        assert!(state.on_auth(first, true).is_empty());
        assert_eq!(state.on_auth(second, true), [Effect::Start(MIC)]);
    }

    #[test]
    fn error_event_stops_capture() {
        // The window calls `on_stream_end` from the Error arm (`app::media_capture`).
        let mut state = CaptureState::default();
        state.on_policy(policy(true, false), Auth::Authorized);
        assert_eq!(state.on_stream_end(), [Effect::Stop(MIC)]);
        assert_eq!(state.mic.phase, Phase::Off);
        assert!(!state.permitted(MIC));
    }

    #[test]
    fn unavailable_encoder_turns_the_device_off() {
        let mut state = CaptureState::default();
        state.on_policy(policy(false, false), Auth::Authorized);
        state.on_toggle(MIC, true, Auth::Authorized);
        assert_eq!(state.on_unavailable(MIC), [Effect::Stop(MIC)]);
        assert!(!state.mic.wanted);
    }

    #[test]
    fn mic_frame_prefixes_0x02() {
        assert_eq!(mic_frame(&[0x68, 1, 2]), [0x02, 0x68, 1, 2]);
        assert_eq!(mic_frame(&[]), [0x02]);
    }

    #[test]
    fn badge_shows_while_starting() {
        // The badge follows the open device, not delivered packets.
        assert_eq!(badge(true), Some(true));
        assert_eq!(badge(false), None);
    }
}
