//! Gamepads for the session (`ARCH-032`): the W3C mapping, the `js,…` commands, the app-wide
//! slot table, each stream's [`Link`], the transport's [`JsTracker`] heartbeat, and rumble.
//!
//! Pure, apart from [`native`], the only GameController and CoreHaptics caller (main thread).

pub mod native;

use base64::Engine as _;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// Selkies drives player slots 0–3; a fifth pad gets none.
pub const SLOTS: usize = 4;
pub const BUTTONS: usize = 17;
pub const AXES: usize = 4;
/// The web's radial stick deadzone (`gamepad.ts::_deadzone`).
pub const DEADZONE: f32 = 0.05;
/// How often `js,h` goes out for a slot off rest; the server sweeps after 2 s.
pub const HEARTBEAT: Duration = Duration::from_millis(100);
/// The web's `RUMBLE_MAX_MS`: no single rumble holds longer.
pub const RUMBLE_MAX: Duration = Duration::from_millis(5000);

/// One reading of a `GCExtendedGamepad`, Apple's way up (Y positive up). Face and shoulder
/// buttons are 0 or 1 (`isPressed`); triggers are analog.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RawPad {
    pub a: f32,
    pub b: f32,
    pub x: f32,
    pub y: f32,
    pub lb: f32,
    pub rb: f32,
    pub lt: f32,
    pub rt: f32,
    /// Optional elements: `None` when the pad has none (read as 0).
    pub options: Option<bool>,
    pub menu: Option<bool>,
    pub ls: Option<bool>,
    pub rs: Option<bool>,
    pub home: Option<bool>,
    /// Up, down, left, right.
    pub dpad: [bool; 4],
    pub left: (f32, f32),
    pub right: (f32, f32),
}

/// A pad's controls in thousandths, W3C order: buttons 0–16, axes 0–3 (Y negative up).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadState {
    pub buttons: [i16; BUTTONS],
    pub axes: [i16; AXES],
}

impl PadState {
    pub const REST: Self = Self {
        buttons: [0; BUTTONS],
        axes: [0; AXES],
    };

    /// The W3C standard gamepad from a `GCExtendedGamepad` reading: A 0, B 1, X 2, Y 3,
    /// shoulders 4/5, triggers 6/7 (analog, no deadzone), Options 8, Menu 9, stick presses
    /// 10/11, D-pad 12–15, Home 16; sticks on axes 0/1 and 2/3 with Y inverted and a radial
    /// deadzone.
    pub fn from_raw(raw: &RawPad) -> Self {
        let flag = |pressed: Option<bool>| f32::from(u8::from(pressed == Some(true)));
        let dpad = |index: usize| f32::from(u8::from(raw.dpad[index]));
        let values = [
            raw.a,
            raw.b,
            raw.x,
            raw.y,
            raw.lb,
            raw.rb,
            raw.lt,
            raw.rt,
            flag(raw.options),
            flag(raw.menu),
            flag(raw.ls),
            flag(raw.rs),
            dpad(0),
            dpad(1),
            dpad(2),
            dpad(3),
            flag(raw.home),
        ];
        let stick = |(x, y): (f32, f32)| {
            if x.hypot(y) < DEADZONE || !x.is_finite() || !y.is_finite() {
                (0., 0.)
            } else {
                (x, -y)
            }
        };
        let (lx, ly) = stick(raw.left);
        let (rx, ry) = stick(raw.right);
        Self {
            buttons: values.map(milli),
            axes: [lx, ly, rx, ry].map(milli),
        }
    }

    #[cfg(test)]
    pub fn at_rest(&self) -> bool {
        *self == Self::REST
    }
}

/// A value in thousandths, clamped to −1..1 and rounded; non-finite reads 0.
pub fn milli(value: f32) -> i16 {
    if !value.is_finite() {
        return 0;
    }
    (value.clamp(-1., 1.) * 1000.).round() as i16
}

/// Thousandths as Selkies reads a float: "1", "0", "0.5", "-0.25", "0.123".
pub fn format_milli(value: i16) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let magnitude = value.unsigned_abs();
    let (whole, fraction) = (magnitude / 1000, magnitude % 1000);
    if fraction == 0 {
        return format!("{sign}{whole}");
    }
    let digits = format!("{fraction:03}");
    format!("{sign}{whole}.{}", digits.trim_end_matches('0'))
}

/// `js,b` for each changed button in index order, then `js,a` for each changed axis.
pub fn diff(slot: u8, from: &PadState, to: &PadState) -> Vec<String> {
    let buttons = (0..BUTTONS)
        .filter(|&i| from.buttons[i] != to.buttons[i])
        .map(|i| format!("js,b,{slot},{i},{}", format_milli(to.buttons[i])));
    let axes = (0..AXES)
        .filter(|&i| from.axes[i] != to.axes[i])
        .map(|i| format!("js,a,{slot},{i},{}", format_milli(to.axes[i])));
    buttons.chain(axes).collect()
}

/// The pad's name for `js,c`: Latin-1 (other characters become '?'), at most 255 of them,
/// base64-encoded. An empty name is "Gamepad".
pub fn encode_name(name: &str) -> String {
    let name = if name.is_empty() { "Gamepad" } else { name };
    let bytes: Vec<u8> = name
        .chars()
        .take(255)
        .map(|ch| u8::try_from(u32::from(ch)).unwrap_or(b'?'))
        .collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// `js,c,{slot},{name},4,17`: axes before buttons. The server presents an Xbox 360 pad
/// whatever the counts say.
pub fn connect_command(slot: u8, name: &str) -> String {
    format!("js,c,{slot},{},{AXES},{BUTTONS}", encode_name(name))
}

pub fn disconnect_command(slot: u8) -> String {
    format!("js,d,{slot}")
}

/// A connected pad, as GameController identifies it (its `GCController` pointer).
pub type PadId = u64;

/// Which pad drives which slot, app-wide, so a pad is the same Player in every window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlotTable([Option<PadId>; SLOTS]);

impl SlotTable {
    /// The pad's slot: the one it already has, else the lowest free one; `None` when all
    /// four are taken.
    pub fn assign(&mut self, id: PadId) -> Option<u8> {
        if let Some(slot) = self.slot_of(id) {
            return Some(slot);
        }
        let free = self.0.iter().position(Option::is_none)?;
        self.0[free] = Some(id);
        Some(free as u8)
    }

    pub fn free(&mut self, id: PadId) -> Option<u8> {
        let slot = self.slot_of(id)?;
        self.0[usize::from(slot)] = None;
        Some(slot)
    }

    pub fn slot_of(&self, id: PadId) -> Option<u8> {
        self.0
            .iter()
            .position(|held| *held == Some(id))
            .map(|slot| slot as u8)
    }
}

/// One connected pad and its latest state.
#[derive(Clone, Debug, PartialEq)]
pub struct Pad {
    pub id: PadId,
    pub name: String,
    pub slot: Option<u8>,
    pub state: PadState,
    /// The pad has haptics Skerry can drive.
    pub rumble: bool,
}

/// What lets a stream send its pads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gate {
    /// `server_settings` arrived and `gamepad_enabled` is on.
    pub ready_and_enabled: bool,
    /// The Gamepad input switch.
    pub input_on: bool,
    /// The session window is key.
    pub key: bool,
}

/// What one stream has told the server about its slots. Sends changes only.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Link {
    announced: BTreeMap<u8, (PadId, PadState)>,
}

impl Link {
    /// Brings the server up to date with `pads` under `gate`:
    /// - not ready, disabled, or input off: `js,d` every announced slot;
    /// - not key: announced slots go back to rest (kept announced), a gone pad's slot gets
    ///   `js,d`, and no new pad is announced;
    /// - key: a new pad gets `js,c` (after `js,d` if another pad held the slot), then
    ///   whatever changed.
    pub fn sync(&mut self, pads: &[Pad], gate: Gate) -> Vec<String> {
        if !gate.ready_and_enabled || !gate.input_on {
            return self.close();
        }
        let mut commands = Vec::new();
        let by_slot: BTreeMap<u8, &Pad> = pads
            .iter()
            .filter_map(|pad| pad.slot.map(|slot| (slot, pad)))
            .collect();
        let slots: BTreeSet<u8> = self
            .announced
            .keys()
            .chain(by_slot.keys())
            .copied()
            .collect();
        for slot in slots {
            let pad = by_slot.get(&slot);
            let announced = self.announced.get(&slot).copied();
            match (announced, pad) {
                (Some((id, _)), pad) if pad.is_none_or(|pad| pad.id != id) => {
                    commands.push(disconnect_command(slot));
                    self.announced.remove(&slot);
                    if let (Some(pad), true) = (pad, gate.key) {
                        commands.extend(self.announce(slot, pad));
                    }
                }
                (Some((id, sent)), Some(pad)) => {
                    let target = if gate.key { pad.state } else { PadState::REST };
                    commands.extend(diff(slot, &sent, &target));
                    self.announced.insert(slot, (id, target));
                }
                (None, Some(pad)) if gate.key => commands.extend(self.announce(slot, pad)),
                _ => {}
            }
        }
        commands
    }

    fn announce(&mut self, slot: u8, pad: &Pad) -> Vec<String> {
        let mut commands = vec![connect_command(slot, &pad.name)];
        commands.extend(diff(slot, &PadState::REST, &pad.state));
        self.announced.insert(slot, (pad.id, pad.state));
        commands
    }

    /// `js,d` for every announced slot; nothing stays announced.
    pub fn close(&mut self) -> Vec<String> {
        std::mem::take(&mut self.announced)
            .into_keys()
            .map(disconnect_command)
            .collect()
    }

    /// A new stream: nothing is announced on it yet.
    pub fn reset(&mut self) {
        self.announced.clear();
    }
}

/// The transport's view of the slots it announced, from the `js,…` it sends: which are off
/// rest (for `js,h`, as `kh` for keys) and which to disconnect when the stream closes.
#[derive(Clone, Debug, Default)]
pub struct JsTracker {
    /// Each announced slot and its controls off rest, as `(axis?, index)`.
    slots: BTreeMap<u8, BTreeSet<(bool, u8)>>,
}

impl JsTracker {
    pub fn observe(&mut self, text: &str) {
        let mut parts = text.split(',');
        if parts.next() != Some("js") {
            return;
        }
        let verb = parts.next();
        let Some(slot) = parts.next().and_then(|slot| slot.parse::<u8>().ok()) else {
            return;
        };
        match verb {
            Some("c") => {
                self.slots.insert(slot, BTreeSet::new());
            }
            Some("d") => {
                self.slots.remove(&slot);
            }
            Some(kind @ ("b" | "a")) => {
                let (Some(index), Some(value)) = (
                    parts.next().and_then(|index| index.parse::<u8>().ok()),
                    parts.next().and_then(|value| value.parse::<f64>().ok()),
                ) else {
                    return;
                };
                if let Some(off) = self.slots.get_mut(&slot) {
                    let control = (kind == "a", index);
                    if value == 0. {
                        off.remove(&control);
                    } else {
                        off.insert(control);
                    }
                }
            }
            _ => {}
        }
    }

    /// Whether any slot holds a control off rest: the heartbeat runs only then.
    pub fn active(&self) -> bool {
        self.slots.values().any(|off| !off.is_empty())
    }

    /// `js,h` for each slot off rest.
    pub fn heartbeat(&self) -> Vec<String> {
        self.slots
            .iter()
            .filter(|(_, off)| !off.is_empty())
            .map(|(slot, _)| format!("js,h,{slot}"))
            .collect()
    }

    /// `js,d` for every slot still announced, on any close path.
    pub fn closing(&mut self) -> Vec<String> {
        std::mem::take(&mut self.slots)
            .into_keys()
            .map(disconnect_command)
            .collect()
    }
}

/// `rumble,<slot>,<strong>,<weak>,<ms>` from the server: replaces the slot's rumble and
/// holds for `duration`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rumble {
    pub slot: u8,
    pub strong: f32,
    pub weak: f32,
    pub duration: Duration,
}

impl Rumble {
    pub fn parse(action: &str) -> Option<Self> {
        let mut parts = action.split(',');
        if parts.next()? != "rumble" {
            return None;
        }
        let slot = parts.next()?.trim().parse::<u8>().ok()?;
        let strong = parts.next()?.trim().parse::<f32>().ok()?;
        let weak = parts.next()?.trim().parse::<f32>().ok()?;
        let ms = parts.next()?.trim().parse::<f64>().ok()?;
        if usize::from(slot) >= SLOTS || !strong.is_finite() || !weak.is_finite() || !ms.is_finite()
        {
            return None;
        }
        let ms = ms.clamp(0., RUMBLE_MAX.as_millis() as f64);
        Some(Self {
            slot,
            strong: strong.clamp(0., 1.),
            weak: weak.clamp(0., 1.),
            duration: Duration::from_secs_f64(ms / 1000.),
        })
    }

    /// Zero levels or zero time stop the rumble.
    pub fn stops(&self) -> bool {
        (self.strong == 0. && self.weak == 0.) || self.duration.is_zero()
    }

    /// One motor plays the stronger level, as the web does.
    pub fn single_motor(&self) -> f32 {
        self.strong.max(self.weak)
    }
}

/// A pad's haptic motors: left/right handles, or one default engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Motor {
    /// The left handle: the strong motor.
    Left,
    /// The right handle: the weak motor.
    Right,
    Default,
}

/// What a pad's haptics do for one rumble message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Haptic {
    Stop(Motor),
    /// Play `intensity` for `seconds`, from the start.
    Play {
        motor: Motor,
        intensity: f32,
        seconds: f64,
    },
}

/// The haptic steps for `rumble` on a pad with handle motors (`dual`) or one: every
/// message stops what plays and, unless it stops, starts a fresh player for its duration,
/// so a renewed (leased) endless effect never runs out mid-game.
pub fn haptics_for(rumble: &Rumble, dual: bool) -> Vec<Haptic> {
    let levels = if dual {
        vec![(Motor::Left, rumble.strong), (Motor::Right, rumble.weak)]
    } else {
        vec![(Motor::Default, rumble.single_motor())]
    };
    let mut steps: Vec<Haptic> = levels
        .iter()
        .map(|&(motor, _)| Haptic::Stop(motor))
        .collect();
    if !rumble.stops() {
        steps.extend(levels.into_iter().filter(|&(_, level)| level > 0.).map(
            |(motor, intensity)| Haptic::Play {
                motor,
                intensity,
                seconds: rumble.duration.as_secs_f64(),
            },
        ));
    }
    steps
}

/// The server's gamepad settings, each a `{value, locked}` object in `server_settings`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GamepadPolicy {
    /// `gamepad_enabled`: off, the server drops every `js,`.
    pub enabled: bool,
    /// `gamepad_on_start`: whether input starts on with no saved choice.
    pub on_start: bool,
    pub on_start_locked: bool,
    /// `ui_sidebar_show_gamepads`: hides the Gamepads card only.
    pub show_panel: bool,
}

impl Default for GamepadPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            on_start: true,
            on_start_locked: false,
            show_panel: true,
        }
    }
}

impl GamepadPolicy {
    pub fn from_server(settings: &Value) -> Self {
        let value = |name: &str| settings[name]["value"].as_bool().unwrap_or(true);
        Self {
            enabled: value("gamepad_enabled"),
            on_start: value("gamepad_on_start"),
            on_start_locked: settings["gamepad_on_start"]["locked"] == true,
            show_panel: value("ui_sidebar_show_gamepads"),
        }
    }
}

/// Whether Gamepad input is on: the saved choice, else the server's `gamepad_on_start`. A
/// locked `gamepad_on_start` with no saved choice holds.
pub fn input_on(policy: GamepadPolicy, saved: Option<bool>) -> bool {
    match saved {
        Some(_) if policy.on_start_locked => policy.on_start,
        Some(saved) => saved,
        None => policy.on_start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pad(id: PadId, slot: Option<u8>, state: PadState) -> Pad {
        Pad {
            id,
            name: format!("Pad {id}"),
            slot,
            state,
            rumble: true,
        }
    }

    fn pressed_a() -> PadState {
        let mut state = PadState::REST;
        state.buttons[0] = 1000;
        state
    }

    const KEY: Gate = Gate {
        ready_and_enabled: true,
        input_on: true,
        key: true,
    };
    const NOT_KEY: Gate = Gate { key: false, ..KEY };

    #[test]
    fn w3c_buttons_follow_the_standard_layout() {
        let raw = RawPad {
            a: 1.,
            y: 1.,
            rb: 1.,
            options: Some(true),
            menu: Some(false),
            rs: Some(true),
            home: Some(true),
            dpad: [false, true, false, true],
            ..RawPad::default()
        };
        let state = PadState::from_raw(&raw);
        let on: Vec<usize> = (0..BUTTONS).filter(|&i| state.buttons[i] != 0).collect();
        assert_eq!(on, [0, 3, 5, 8, 11, 13, 15, 16]);
    }

    #[test]
    fn vertical_sticks_are_inverted_to_w3c() {
        let raw = RawPad {
            left: (0.5, 1.),
            right: (-1., -0.25),
            ..RawPad::default()
        };
        assert_eq!(PadState::from_raw(&raw).axes, [500, -1000, -1000, 250]);
    }

    #[test]
    fn triggers_are_analog_buttons_6_and_7() {
        let raw = RawPad {
            lt: 0.25,
            rt: 0.999,
            ..RawPad::default()
        };
        let state = PadState::from_raw(&raw);
        assert_eq!((state.buttons[6], state.buttons[7]), (250, 999));
    }

    #[test]
    fn triggers_are_not_deadzoned() {
        let raw = RawPad {
            lt: 0.01,
            ..RawPad::default()
        };
        assert_eq!(PadState::from_raw(&raw).buttons[6], 10);
    }

    #[test]
    fn missing_optional_buttons_read_zero() {
        let state = PadState::from_raw(&RawPad::default());
        assert!(state.at_rest());
    }

    #[test]
    fn stick_deadzone_is_radial() {
        // Each axis under 0.05, but together over it: kept.
        let raw = RawPad {
            left: (0.04, 0.04),
            right: (0.03, 0.03),
            ..RawPad::default()
        };
        let state = PadState::from_raw(&raw);
        assert_eq!(&state.axes[..2], [40, -40]);
        assert_eq!(&state.axes[2..], [0, 0]);
    }

    #[test]
    fn values_are_quantized_and_formatted_plainly() {
        assert_eq!(milli(0.1234), 123);
        assert_eq!(milli(2.), 1000);
        assert_eq!(milli(f32::NAN), 0);
        for (value, text) in [
            (1000, "1"),
            (0, "0"),
            (500, "0.5"),
            (-250, "-0.25"),
            (123, "0.123"),
            (-1000, "-1"),
            (5, "0.005"),
        ] {
            assert_eq!(format_milli(value), text);
        }
    }

    #[test]
    fn diff_sends_changes_only_in_index_order() {
        let mut to = PadState::REST;
        to.buttons[7] = 500;
        to.buttons[1] = 1000;
        to.axes[3] = -250;
        assert_eq!(
            diff(2, &PadState::REST, &to),
            ["js,b,2,1,1", "js,b,2,7,0.5", "js,a,2,3,-0.25"]
        );
        assert!(diff(2, &to, &to).is_empty());
    }

    #[test]
    fn name_is_latin1_base64_capped_at_255() {
        assert_eq!(encode_name("Pad"), "UGFk");
        // é is Latin-1 (0xe9); ✓ is not.
        let decoded = |b64: &str| {
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .unwrap()
        };
        assert_eq!(decoded(&encode_name("é✓")), [0xe9, b'?']);
        assert_eq!(decoded(&encode_name(&"x".repeat(300))).len(), 255);
    }

    #[test]
    fn empty_name_becomes_gamepad() {
        assert_eq!(encode_name(""), encode_name("Gamepad"));
    }

    #[test]
    fn connect_command_reports_4_axes_and_17_buttons() {
        assert_eq!(connect_command(1, "Pad"), "js,c,1,UGFk,4,17");
        assert_eq!(disconnect_command(1), "js,d,1");
    }

    #[test]
    fn slots_fill_lowest_free_first() {
        let mut slots = SlotTable::default();
        assert_eq!(slots.assign(10), Some(0));
        assert_eq!(slots.assign(11), Some(1));
        assert_eq!(slots.assign(12), Some(2));
    }

    #[test]
    fn a_freed_slot_is_reused() {
        let mut slots = SlotTable::default();
        slots.assign(10);
        slots.assign(11);
        assert_eq!(slots.free(10), Some(0));
        assert_eq!(slots.assign(12), Some(0));
        assert_eq!(slots.free(99), None);
    }

    #[test]
    fn a_fifth_pad_gets_no_slot() {
        let mut slots = SlotTable::default();
        for id in 0..4 {
            assert!(slots.assign(id).is_some());
        }
        assert_eq!(slots.assign(4), None);
    }

    #[test]
    fn a_slot_stays_while_its_pad_is_connected() {
        let mut slots = SlotTable::default();
        slots.assign(10);
        slots.assign(11);
        assert_eq!(slots.assign(11), Some(1));
        assert_eq!(slots.slot_of(11), Some(1));
    }

    #[test]
    fn link_announces_then_sends_state() {
        let mut link = Link::default();
        let pads = [pad(7, Some(0), pressed_a())];
        assert_eq!(
            link.sync(&pads, KEY),
            [connect_command(0, "Pad 7"), "js,b,0,0,1".into()]
        );
        assert!(link.sync(&pads, KEY).is_empty());
    }

    #[test]
    fn unplugging_sends_js_d_for_its_slot() {
        let mut link = Link::default();
        link.sync(&[pad(7, Some(1), PadState::REST)], KEY);
        assert_eq!(link.sync(&[], KEY), ["js,d,1"]);
        assert!(link.sync(&[], KEY).is_empty());
    }

    #[test]
    fn unplugging_while_not_key_still_sends_js_d() {
        let mut link = Link::default();
        link.sync(&[pad(7, Some(0), PadState::REST)], KEY);
        assert_eq!(link.sync(&[], NOT_KEY), ["js,d,0"]);
    }

    #[test]
    fn resigning_key_sends_rest_and_keeps_the_slot() {
        let mut link = Link::default();
        let pads = [pad(7, Some(0), pressed_a())];
        link.sync(&pads, KEY);
        assert_eq!(link.sync(&pads, NOT_KEY), ["js,b,0,0,0"]);
        assert!(link.sync(&pads, NOT_KEY).is_empty());
    }

    #[test]
    fn regaining_key_sends_the_live_state() {
        let mut link = Link::default();
        let pads = [pad(7, Some(0), pressed_a())];
        link.sync(&pads, KEY);
        link.sync(&pads, NOT_KEY);
        assert_eq!(link.sync(&pads, KEY), ["js,b,0,0,1"]);
    }

    #[test]
    fn a_new_pad_is_not_announced_while_not_key() {
        let mut link = Link::default();
        assert!(link
            .sync(&[pad(7, Some(0), pressed_a())], NOT_KEY)
            .is_empty());
    }

    #[test]
    fn a_pad_replacing_another_in_a_slot_sends_d_then_c() {
        let mut link = Link::default();
        link.sync(&[pad(7, Some(0), PadState::REST)], KEY);
        assert_eq!(
            link.sync(&[pad(8, Some(0), PadState::REST)], KEY),
            ["js,d,0".to_string(), connect_command(0, "Pad 8")]
        );
    }

    #[test]
    fn nothing_is_sent_before_ready_or_when_the_server_disables() {
        let mut link = Link::default();
        let pads = [pad(7, Some(0), pressed_a())];
        let off = Gate {
            ready_and_enabled: false,
            ..KEY
        };
        assert!(link.sync(&pads, off).is_empty());
    }

    #[test]
    fn turning_input_off_disconnects_announced_slots() {
        let mut link = Link::default();
        let pads = [
            pad(7, Some(0), PadState::REST),
            pad(8, Some(1), PadState::REST),
        ];
        link.sync(&pads, KEY);
        let off = Gate {
            input_on: false,
            ..KEY
        };
        assert_eq!(link.sync(&pads, off), ["js,d,0", "js,d,1"]);
        assert!(link.sync(&pads, off).is_empty());
    }

    #[test]
    fn heartbeat_only_for_slots_off_rest() {
        let mut tracker = JsTracker::default();
        for command in [
            connect_command(0, "A"),
            connect_command(1, "B"),
            "js,b,1,0,1".into(),
        ] {
            tracker.observe(&command);
        }
        assert_eq!(tracker.heartbeat(), ["js,h,1"]);
    }

    #[test]
    fn close_disconnects_every_announced_slot() {
        let mut link = Link::default();
        link.sync(
            &[
                pad(7, Some(0), PadState::REST),
                pad(8, Some(2), PadState::REST),
            ],
            KEY,
        );
        assert_eq!(link.close(), ["js,d,0", "js,d,2"]);
        assert!(link.close().is_empty());
    }

    #[test]
    fn reset_announces_again_after_reconnect() {
        let mut link = Link::default();
        let pads = [pad(7, Some(0), PadState::REST)];
        link.sync(&pads, KEY);
        link.reset();
        assert_eq!(link.sync(&pads, KEY), [connect_command(0, "Pad 7")]);
    }

    #[test]
    fn rumble_parses_the_system_action() {
        assert_eq!(
            Rumble::parse("rumble,1,0.500,0.250,200"),
            Some(Rumble {
                slot: 1,
                strong: 0.5,
                weak: 0.25,
                duration: Duration::from_millis(200),
            })
        );
        assert_eq!(Rumble::parse("rumble,1,0.5"), None);
        assert_eq!(Rumble::parse("other,1,0.5,0.5,10"), None);
    }

    #[test]
    fn rumble_rejects_bad_slots_and_non_finite_levels() {
        assert_eq!(Rumble::parse("rumble,4,0.5,0.5,100"), None);
        assert_eq!(Rumble::parse("rumble,0,NaN,0.5,100"), None);
        assert_eq!(Rumble::parse("rumble,0,0.5,inf,100"), None);
        let clamped = Rumble::parse("rumble,0,2,-1,100").unwrap();
        assert_eq!((clamped.strong, clamped.weak), (1., 0.));
    }

    #[test]
    fn rumble_caps_duration_at_5s() {
        let long = Rumble::parse("rumble,0,1,1,60000").unwrap();
        assert_eq!(long.duration, RUMBLE_MAX);
    }

    #[test]
    fn rumble_zero_level_or_duration_stops() {
        assert!(Rumble::parse("rumble,0,0,0,100").unwrap().stops());
        assert!(Rumble::parse("rumble,0,1,1,0").unwrap().stops());
        assert!(!Rumble::parse("rumble,0,0,0.2,100").unwrap().stops());
    }

    #[test]
    fn single_motor_rumble_takes_the_stronger_level() {
        let rumble = Rumble::parse("rumble,0,0.3,0.7,100").unwrap();
        assert_eq!(rumble.single_motor(), 0.7);
        assert_eq!(
            haptics_for(&rumble, false),
            [
                Haptic::Stop(Motor::Default),
                Haptic::Play {
                    motor: Motor::Default,
                    intensity: 0.7,
                    seconds: 0.1
                }
            ]
        );
    }

    #[test]
    fn a_renewed_rumble_restarts_the_player() {
        let rumble = Rumble::parse("rumble,0,1,0.5,1000").unwrap();
        let steps = haptics_for(&rumble, true);
        // Every message stops what plays, then starts it again for its own duration.
        assert_eq!(
            steps[..2],
            [Haptic::Stop(Motor::Left), Haptic::Stop(Motor::Right)]
        );
        assert_eq!(steps.len(), 4);
        assert_eq!(haptics_for(&rumble, true), steps);
        let stop = Rumble::parse("rumble,0,0,0,0").unwrap();
        assert_eq!(
            haptics_for(&stop, true),
            [Haptic::Stop(Motor::Left), Haptic::Stop(Motor::Right)]
        );
    }

    #[test]
    fn policy_defaults_to_on_when_missing() {
        assert_eq!(
            GamepadPolicy::from_server(&json!({})),
            GamepadPolicy::default()
        );
    }

    #[test]
    fn policy_reads_selkies_value_objects() {
        let settings = json!({
            "gamepad_enabled": {"value": false, "locked": true},
            "gamepad_on_start": {"value": false, "locked": true},
            "ui_sidebar_show_gamepads": {"value": false, "locked": false},
        });
        assert_eq!(
            GamepadPolicy::from_server(&settings),
            GamepadPolicy {
                enabled: false,
                on_start: false,
                on_start_locked: true,
                show_panel: false,
            }
        );
    }

    #[test]
    fn input_follows_saved_choice_else_on_start() {
        let policy = |on_start, on_start_locked| GamepadPolicy {
            on_start,
            on_start_locked,
            ..GamepadPolicy::default()
        };
        assert!(input_on(policy(true, false), None));
        assert!(!input_on(policy(false, false), None));
        assert!(!input_on(policy(true, false), Some(false)));
        assert!(input_on(policy(false, false), Some(true)));
        assert!(!input_on(policy(false, true), Some(true)));
    }
}
