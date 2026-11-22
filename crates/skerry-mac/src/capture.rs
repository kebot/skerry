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
//! The microphone and the camera each have their own [`DeviceState`] and [`DevicePolicy`];
//! the rules are the same for both.

use serde_json::Value;

/// A capture device the session can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    Microphone,
    Camera,
}

impl Subject {
    pub const ALL: [Self; 2] = [Self::Microphone, Self::Camera];

    /// The name in `CAPTURE_DEMAND`.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Microphone => "microphone",
            Self::Camera => "webcam",
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

/// The camera size the server asks for when it says nothing.
pub const DEFAULT_CAM_SIZE: (u32, u32) = (1280, 720);

/// The capture part of `server_settings`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapturePolicy {
    pub mic: DevicePolicy,
    pub cam: DevicePolicy,
    /// `ui_sidebar_show_webcam`: the camera is offered at all. Hidden means not permitted.
    pub cam_shown: bool,
    /// `webcam_width` × `webcam_height`, clamped to 160..=3840.
    pub cam_size: (u32, u32),
}

impl Default for CapturePolicy {
    fn default() -> Self {
        Self {
            mic: DevicePolicy::default(),
            cam: DevicePolicy::default(),
            cam_shown: true,
            cam_size: DEFAULT_CAM_SIZE,
        }
    }
}

/// `{value: true}`, `{value: "true"}` or a bare `true`.
fn truthy(setting: &Value) -> bool {
    let value = setting.get("value").unwrap_or(setting);
    value.as_bool() == Some(true) || value.as_str() == Some("true")
}

/// `{value: false}`, `{value: "false"}` or a bare `false`.
fn falsy(setting: &Value) -> bool {
    let value = setting.get("value").unwrap_or(setting);
    value.as_bool() == Some(false) || value.as_str() == Some("false")
}

/// A whole number setting, `{value: 720}`, `{value: "720"}` or a bare `720`.
fn number(setting: &Value) -> Option<u64> {
    let value = setting.get("value").unwrap_or(setting);
    value
        .as_u64()
        .or_else(|| value.as_f64().filter(|n| *n >= 0.0).map(|n| n as u64))
        .or_else(|| value.as_str()?.trim().parse().ok())
}

/// One device's start settings: `*_on_start` (a bool, or the raw "demand") and
/// `*_on_demand`.
fn start_rules(settings: &Value, wire: &str) -> (bool, bool) {
    let start = &settings[format!("{wire}_on_start")];
    let on_demand =
        truthy(&settings[format!("{wire}_on_demand")]) || start["value"].as_str() == Some("demand");
    (truthy(start), on_demand)
}

impl CapturePolicy {
    /// From `server_settings.settings`. The server's own gates:
    /// - microphone frames are refused only when `!(audio_enabled && (microphone_enabled ||
    ///   !locked))`, so an unlocked false still accepts frames; it only means "not on by
    ///   default";
    /// - camera frames are refused only when `webcam_enabled` is locked off (`webcam/mod.rs`).
    pub fn from_server(settings: &Value) -> Self {
        let enabled = &settings["microphone_enabled"];
        let locked = enabled["locked"].as_bool() == Some(true);
        let on = truthy(enabled);
        let audio_off = falsy(&settings["audio_enabled"]);
        let (on_start, on_demand) = start_rules(settings, "microphone");
        let mic = DevicePolicy {
            permitted: !audio_off && (on || !locked),
            locked,
            audio_off,
            on_start,
            on_demand,
        };
        let shown = !falsy(&settings["ui_sidebar_show_webcam"]);
        let enabled = &settings["webcam_enabled"];
        let locked = enabled["locked"].as_bool() == Some(true);
        let on = truthy(enabled);
        let (on_start, on_demand) = start_rules(settings, "webcam");
        let cam = DevicePolicy {
            permitted: shown && !(locked && !on),
            locked,
            audio_off: false,
            on_start,
            on_demand,
        };
        let side = |key: &str, default: u32| {
            number(&settings[key]).map_or(default, |n| n.clamp(160, 3840) as u32)
        };
        Self {
            mic,
            cam,
            cam_shown: shown,
            cam_size: (
                side("webcam_width", DEFAULT_CAM_SIZE.0),
                side("webcam_height", DEFAULT_CAM_SIZE.1),
            ),
        }
    }

    pub fn device(&self, subject: Subject) -> DevicePolicy {
        match subject {
            Subject::Microphone => self.mic,
            Subject::Camera => self.cam,
        }
    }
}

/// A capture reply from the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// `MICROPHONE_DISABLED` / `WEBCAM_DISABLED`: the server refused the frames.
    Disabled(Subject),
    /// `CAPTURE_DEMAND <subject> <0|1>`: an app in the session opened or closed the device.
    Demand(Subject, bool),
    /// `WEBCAM_KEYFRAME`: the server's camera decoder needs a keyframe.
    Keyframe,
}

impl Reply {
    /// The device the reply is about, when it changes the policy.
    pub fn subject(self) -> Option<Subject> {
        match self {
            Self::Disabled(subject) | Self::Demand(subject, _) => Some(subject),
            Self::Keyframe => None,
        }
    }
}

/// The exact literals; anything else is not a capture reply.
pub fn server_reply(text: &str) -> Option<Reply> {
    match text {
        "MICROPHONE_DISABLED" => return Some(Reply::Disabled(Subject::Microphone)),
        "WEBCAM_DISABLED" => return Some(Reply::Disabled(Subject::Camera)),
        "WEBCAM_KEYFRAME" => return Some(Reply::Keyframe),
        _ => {}
    }
    let rest = text.strip_prefix("CAPTURE_DEMAND ")?;
    let (subject, value) = rest.split_once(' ')?;
    let subject = Subject::ALL
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

/// macOS's answers for both devices; one [`Auth`] stands for both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Auths {
    pub mic: Auth,
    pub cam: Auth,
}

impl From<Auth> for Auths {
    fn from(auth: Auth) -> Self {
        Self {
            mic: auth,
            cam: auth,
        }
    }
}

impl Auths {
    pub fn get(self, subject: Subject) -> Auth {
        match subject {
            Subject::Microphone => self.mic,
            Subject::Camera => self.cam,
        }
    }
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
    pub cam: DeviceState,
    pub policy: Option<CapturePolicy>,
    generation: u64,
}

impl CaptureState {
    fn device(&mut self, subject: Subject) -> &mut DeviceState {
        match subject {
            Subject::Microphone => &mut self.mic,
            Subject::Camera => &mut self.cam,
        }
    }

    /// One device's state.
    pub fn state(&self, subject: Subject) -> &DeviceState {
        match subject {
            Subject::Microphone => &self.mic,
            Subject::Camera => &self.cam,
        }
    }

    fn device_policy(&self, subject: Subject) -> Option<DevicePolicy> {
        self.policy.map(|policy| policy.device(subject))
    }

    /// The server accepts this device now.
    pub fn permitted(&self, subject: Subject) -> bool {
        self.device_policy(subject)
            .is_some_and(|policy| policy.permitted)
            && !self.state(subject).server_disabled
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

    /// Turns the device off. It clears `wanted` on purpose, whatever turned it off (a toggle,
    /// a lock, `*_DISABLED`, demand 0): a person's "on" is never restored by itself after the
    /// server takes the device away and gives it back. They flip the switch again
    /// (the safe direction, as the web client; M-QA-4).
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
    pub fn on_policy(&mut self, policy: CapturePolicy, auth: impl Into<Auths>) -> Vec<Effect> {
        let auths = auth.into();
        let first = self.policy.is_none();
        self.policy = Some(policy);
        let mut effects = Vec::new();
        for subject in Subject::ALL {
            self.device(subject).server_disabled = false;
            effects.extend(self.apply_policy(subject, first, auths.get(subject)));
        }
        effects
    }

    fn apply_policy(&mut self, subject: Subject, first: bool, auth: Auth) -> Vec<Effect> {
        let device = *self.state(subject);
        let Some(rules) = self.device_policy(subject) else {
            return Vec::new();
        };
        if !self.permitted(subject) {
            return self.stop(subject);
        }
        // Locked on counts as on_start true, on every settings (M-QA-3): it starts after the
        // permission, and the switch shows on and disabled. A refused prompt stays latched.
        if rules.locked && device.phase == Phase::Off && !device.refused && !device.denied {
            return self.start(subject, true, auth);
        }
        if !first || device.phase != Phase::Off {
            return Vec::new();
        }
        if device.touched {
            if device.wanted {
                return self.start(subject, false, auth);
            }
            return Vec::new();
        }
        if rules.on_start && !rules.on_demand {
            return self.start(subject, true, auth);
        }
        Vec::new()
    }

    /// The person flipped the switch.
    pub fn on_toggle(&mut self, subject: Subject, on: bool, auth: Auth) -> Vec<Effect> {
        // A locked switch is the server's (the card disables it).
        if self
            .device_policy(subject)
            .is_some_and(|policy| policy.locked)
        {
            return Vec::new();
        }
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

    /// A capture reply; `auth` is macOS's answer for the reply's device. A keyframe request
    /// changes no state (the window hands it to the camera).
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
                let locked = self
                    .device_policy(subject)
                    .is_some_and(|policy| policy.locked);
                if device.touched || !device.by_server || locked {
                    return Vec::new();
                }
                self.stop(subject)
            }
            Reply::Keyframe => Vec::new(),
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
        let mut effects = Vec::new();
        for subject in Subject::ALL {
            let device = self.device(subject);
            let was = std::mem::take(&mut device.phase);
            device.server_disabled = false;
            device.refused = false;
            if device.by_server {
                device.wanted = false;
                device.by_server = false;
            }
            if was == Phase::On {
                effects.push(Effect::Stop(subject));
            }
        }
        effects
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

/// The camera's opcode, and its codec id for H.264 (`webcam/mod.rs`).
pub const WEBCAM_OPCODE: u8 = 0x06;
pub const WEBCAM_H264: u8 = 1;
/// Bit 0 of the flags byte: the frame is a keyframe. No rotation or flip bits: a Mac camera
/// is upright landscape.
pub const WEBCAM_KEY: u8 = 0x01;

/// A camera uplink frame: `0x06`, the codec (H.264), the flags, then one Annex B access unit.
pub fn webcam_frame(keyframe: bool, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 3);
    frame.extend_from_slice(&[
        WEBCAM_OPCODE,
        WEBCAM_H264,
        if keyframe { WEBCAM_KEY } else { 0 },
    ]);
    frame.extend_from_slice(payload);
    frame
}

/// Whether a camera frame (as [`webcam_frame`] built it) is a keyframe.
pub fn webcam_keyframe(frame: &[u8]) -> bool {
    frame.get(2).is_some_and(|flags| flags & WEBCAM_KEY != 0)
}

/// Camera frames waiting for the transport: about 100 ms. A full queue drops the frame and
/// forces the next keyframe.
pub const CAM_QUEUE: usize = 3;

/// The in-window badge: `Some((microphone, camera))` only while a device is open.
pub fn badge(mic_open: bool, cam_open: bool) -> Option<(bool, bool)> {
    (mic_open || cam_open).then_some((mic_open, cam_open))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MIC: Subject = Subject::Microphone;
    const CAM: Subject = Subject::Camera;

    fn policy(on_start: bool, on_demand: bool) -> CapturePolicy {
        CapturePolicy {
            mic: DevicePolicy {
                permitted: true,
                locked: false,
                audio_off: false,
                on_start,
                on_demand,
            },
            ..CapturePolicy::default()
        }
    }

    fn locked_off() -> CapturePolicy {
        CapturePolicy {
            mic: DevicePolicy {
                permitted: false,
                locked: true,
                ..DevicePolicy::default()
            },
            ..CapturePolicy::default()
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
    fn locked_on_mic_starts_like_on_start() {
        let locked_on = CapturePolicy::from_server(&json!({
            "microphone_enabled": {"value": true, "locked": true},
            "microphone_on_start": {"value": false}
        }));
        let mut state = CaptureState::default();
        let ticket = ticket(&state.on_policy(locked_on, Auth::NotDetermined));
        assert_eq!(state.on_auth(ticket, true), [Effect::Start(MIC)]);
        // The switch and demand 0 can't turn it off.
        assert!(state.on_toggle(MIC, false, Auth::Authorized).is_empty());
        assert!(state
            .on_reply(Reply::Demand(MIC, false), Auth::Authorized)
            .is_empty());
        assert_eq!(state.mic.phase, Phase::On);
        // Locked on by a live change: it starts then too.
        let mut state = CaptureState::default();
        state.on_policy(policy(false, false), Auth::Authorized);
        assert_eq!(
            state.on_policy(locked_on, Auth::Authorized),
            [Effect::Start(MIC)]
        );
        // Refused at the prompt: no loop of prompts.
        let mut state = CaptureState::default();
        state.on_policy(locked_on, Auth::Denied);
        assert!(state.mic.refused);
        assert!(state.on_policy(locked_on, Auth::Denied).is_empty());
    }

    #[test]
    fn a_persons_on_is_not_restored_after_a_lock() {
        let mut state = CaptureState::default();
        state.on_policy(policy(false, false), Auth::Authorized);
        state.on_toggle(MIC, true, Auth::Authorized);
        state.on_policy(locked_off(), Auth::Authorized);
        assert!(state
            .on_policy(policy(false, false), Auth::Authorized)
            .is_empty());
        assert!(!state.mic.wanted);
    }

    #[test]
    fn audio_disabled_forbids_microphone() {
        let policy = CapturePolicy::from_server(&json!({
            "microphone_enabled": {"value": true, "locked": false},
            "audio_enabled": {"value": false}
        }));
        assert!(!policy.mic.permitted && policy.mic.audio_off);
        let policy = CapturePolicy::from_server(&json!({"audio_enabled": {"value": "false"}}));
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
        assert_eq!(
            server_reply("CAPTURE_DEMAND webcam 1"),
            Some(Reply::Demand(CAM, true))
        );
        assert_eq!(server_reply("WEBCAM_DISABLED"), Some(Reply::Disabled(CAM)));
        assert_eq!(server_reply("WEBCAM_KEYFRAME"), Some(Reply::Keyframe));
        assert_eq!(server_reply("WEBCAM_KEYFRAME 1"), None);
        assert_eq!(server_reply("CAPTURE_DEMAND camera 1"), None);
        assert_eq!(server_reply("CAPTURE_DEMAND speaker 1"), None);
        assert_eq!(server_reply("CAPTURE_DEMAND_X microphone 1"), None);
        assert_eq!(server_reply("MICROPHONE_DISABLED "), None);
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
        assert_eq!(badge(true, false), Some((true, false)));
        assert_eq!(badge(false, true), Some((false, true)));
        assert_eq!(badge(true, true), Some((true, true)));
        assert_eq!(badge(false, false), None);
    }

    fn cam_policy(on_start: bool, on_demand: bool) -> CapturePolicy {
        CapturePolicy {
            cam: DevicePolicy {
                permitted: true,
                on_start,
                on_demand,
                ..DevicePolicy::default()
            },
            ..CapturePolicy::default()
        }
    }

    #[test]
    fn webcam_policy_reads_selkies_payload_shape() {
        let policy = CapturePolicy::from_server(&json!({
            "webcam_enabled": {"value": false, "locked": false},
            "webcam_on_start": {"value": true},
            "webcam_on_demand": {"value": false},
            "audio_enabled": {"value": false}
        }));
        // Unlocked false still permits, and session audio has nothing to do with the camera.
        assert!(policy.cam.permitted && policy.cam.on_start && !policy.cam.on_demand);
        let policy = CapturePolicy::from_server(&json!({
            "webcam_enabled": {"value": false, "locked": true},
            "webcam_on_start": {"value": "demand"}
        }));
        assert!(!policy.cam.permitted && policy.cam.locked && policy.cam.on_demand);
        let policy = CapturePolicy::from_server(&json!({
            "webcam_enabled": {"value": true, "locked": true}
        }));
        assert!(policy.cam.permitted && policy.cam.locked);
    }

    #[test]
    fn webcam_hidden_by_sidebar_setting() {
        let policy = CapturePolicy::from_server(&json!({
            "webcam_enabled": {"value": true, "locked": false},
            "ui_sidebar_show_webcam": {"value": false}
        }));
        assert!(!policy.cam_shown && !policy.cam.permitted);
        assert!(CapturePolicy::from_server(&json!({})).cam_shown);
        // Hidden: on_start never starts it.
        let mut state = CaptureState::default();
        let mut hidden = cam_policy(true, false);
        hidden.cam.permitted = false;
        hidden.cam_shown = false;
        assert!(state.on_policy(hidden, Auth::Authorized).is_empty());
    }

    #[test]
    fn webcam_size_defaults_and_clamps() {
        assert_eq!(CapturePolicy::from_server(&json!({})).cam_size, (1280, 720));
        let policy = CapturePolicy::from_server(&json!({
            "webcam_width": {"value": 640},
            "webcam_height": {"value": "480"}
        }));
        assert_eq!(policy.cam_size, (640, 480));
        let policy = CapturePolicy::from_server(&json!({
            "webcam_width": {"value": 10_000},
            "webcam_height": {"value": 12}
        }));
        assert_eq!(policy.cam_size, (3840, 160));
        let policy = CapturePolicy::from_server(&json!({"webcam_width": {"value": "wide"}}));
        assert_eq!(policy.cam_size, (1280, 720));
    }

    #[test]
    fn audio_enabled_string_forms_read_as_off() {
        for off in [
            json!(false),
            json!({"value": "false"}),
            json!({"value": false}),
        ] {
            let policy = CapturePolicy::from_server(&json!({"audio_enabled": off}));
            assert!(policy.mic.audio_off, "{policy:?}");
        }
    }

    #[test]
    fn webcam_frame_header_is_06_01_flags() {
        assert_eq!(
            webcam_frame(true, &[0, 0, 0, 1, 0x65]),
            [6, 1, 1, 0, 0, 0, 1, 0x65]
        );
        assert_eq!(
            webcam_frame(false, &[0, 0, 0, 1, 0x41]),
            [6, 1, 0, 0, 0, 0, 1, 0x41]
        );
        assert!(webcam_keyframe(&webcam_frame(true, &[1])));
        assert!(!webcam_keyframe(&webcam_frame(false, &[1])));
        assert!(!webcam_keyframe(&[6]));
    }

    #[test]
    fn camera_follows_its_own_rules() {
        // Camera on_start, microphone not: only the camera starts.
        let mut state = CaptureState::default();
        let mut both = cam_policy(true, false);
        both.mic = policy(false, false).mic;
        assert_eq!(
            state.on_policy(both, Auth::Authorized),
            [Effect::Start(CAM)]
        );
        // Each device is asked with its own answer.
        let mut state = CaptureState::default();
        let mut both = cam_policy(true, false);
        both.mic = policy(true, false).mic;
        let effects = state.on_policy(
            both,
            Auths {
                mic: Auth::Authorized,
                cam: Auth::NotDetermined,
            },
        );
        assert_eq!(effects[0], Effect::Start(MIC));
        assert!(matches!(effects[1], Effect::AskPermission(t) if t.subject == CAM));
        // WEBCAM_DISABLED stops the camera only.
        assert!(state
            .on_reply(Reply::Disabled(CAM), Auth::Authorized)
            .is_empty());
        assert_eq!(state.mic.phase, Phase::On);
        assert!(!state.permitted(CAM) && state.permitted(MIC));
        // The stream's end stops both.
        let mut state = CaptureState::default();
        let mut both = cam_policy(true, false);
        both.mic = policy(true, false).mic;
        state.on_policy(both, Auth::Authorized);
        assert_eq!(
            state.on_stream_end(),
            [Effect::Stop(MIC), Effect::Stop(CAM)]
        );
    }

    #[test]
    fn camera_demand_starts_and_stops() {
        let mut state = CaptureState::default();
        state.on_policy(cam_policy(false, true), Auth::Authorized);
        assert_eq!(
            state.on_reply(Reply::Demand(CAM, true), Auth::Authorized),
            [Effect::Start(CAM)]
        );
        assert_eq!(state.mic.phase, Phase::Off);
        assert_eq!(
            state.on_reply(Reply::Demand(CAM, false), Auth::Authorized),
            [Effect::Stop(CAM)]
        );
    }

    #[test]
    fn keyframe_reply_changes_no_state() {
        let mut state = CaptureState::default();
        state.on_policy(cam_policy(true, false), Auth::Authorized);
        let before = state.clone();
        assert!(state.on_reply(Reply::Keyframe, Auth::Authorized).is_empty());
        assert_eq!(state, before);
        assert_eq!(Reply::Keyframe.subject(), None);
        assert_eq!(Reply::Disabled(CAM).subject(), Some(CAM));
    }
}
