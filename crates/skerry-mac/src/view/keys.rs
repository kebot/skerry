//! Which keys and modifiers the session holds down, and the `kd` / `ku` commands that
//! change that (`ARCH-GAP-DC-09`).
//!
//! Pure bookkeeping, no GPUI state: `Desktop` feeds it Mac key events and sends every
//! command it returns, so release paths (blur, deactivate, child windows, stream close) can
//! be tested without a window.

use gpui_kit::Modifiers;
use std::collections::{BTreeMap, BTreeSet};

pub const SHIFT_L: u32 = 0xffe1;
pub const CONTROL_L: u32 = 0xffe3;
pub const ALT_L: u32 = 0xffe9;

/// The modifiers Skerry sends. Command maps to Control or is dropped, so Skerry never sends
/// Super or Meta (0xffe7, 0xffe8, 0xffeb, 0xffec).
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mods {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

impl Mods {
    pub fn from_mac(modifiers: Modifiers, command_as_control: bool) -> Self {
        Self {
            shift: modifiers.shift,
            control: modifiers.control || (command_as_control && modifiers.platform),
            alt: modifiers.alt,
        }
    }
}

/// "⌘ means Control" for a forwarded Skerry chord (`ARCH-031`), held until ⌘ itself lifts,
/// so releasing Shift mid-chord doesn't drop Control early.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandLatch(bool);

impl CommandLatch {
    pub fn engage(&mut self) {
        self.0 = true;
    }

    /// Clears the latch once ⌘ is up.
    pub fn observe(&mut self, modifiers: Modifiers) {
        if !modifiers.platform {
            self.0 = false;
        }
    }

    pub fn as_control(&self, command_as_control: bool) -> bool {
        command_as_control || self.0
    }
}

#[derive(Default)]
pub struct KeyLedger {
    /// Held keys by GPUI key name, so a key-up finds the keysym its key-down sent.
    keys: BTreeMap<String, u32>,
    mods: Mods,
    /// Held keys that went down while ⌘ was held. AppKit sends no key-up for them while ⌘
    /// is down, so they are released when ⌘ lifts (`ARCH-031`).
    command_keys: BTreeSet<String>,
    /// ⌘ was down at the last modifier change.
    command_held: bool,
    latch: CommandLatch,
}

impl KeyLedger {
    /// A key went down. A macOS auto-repeat sends `kd` again on purpose: Skerry's 1 s `kh`
    /// heartbeat alone would let the server's repeat loop pause. A repeat that now maps to a
    /// different keysym (a held `a` repeating as `A` once Shift goes down) releases the
    /// first keysym, or it would stay down on the server.
    pub fn press(&mut self, code: String, keysym: u32) -> Vec<String> {
        let mut commands = Vec::new();
        if !self.command_held {
            self.command_keys.remove(&code);
        } else if !self.keys.contains_key(&code) {
            // Only a key that went down under ⌘; one held before it (now repeating) keeps
            // its own key-up.
            self.command_keys.insert(code.clone());
        }
        if let Some(previous) = self.keys.insert(code, keysym) {
            if previous != keysym {
                commands.push(format!("ku,{previous}"));
            }
        }
        commands.push(format!("kd,{keysym}"));
        commands
    }

    /// A key came up; only a key this ledger holds is released.
    pub fn release(&mut self, code: &str) -> Vec<String> {
        self.command_keys.remove(code);
        self.keys
            .remove(code)
            .map(|keysym| vec![format!("ku,{keysym}")])
            .unwrap_or_default()
    }

    /// One keystroke that is never held (an IME commit).
    pub fn tap(&self, keysym: u32) -> Vec<String> {
        vec![format!("kd,{keysym}"), format!("ku,{keysym}")]
    }

    /// A forwarded Skerry chord's key (`ARCH-031`): tapped, never held, since AppKit sends
    /// no key-up while ⌘ is down and a held key would autorepeat on the remote. A macOS
    /// autorepeat taps again. If the same key was already held, it is released first.
    pub fn tap_chord(&mut self, code: &str, keysym: u32) -> Vec<String> {
        let mut commands = self.release(code);
        commands.extend(self.tap(keysym));
        commands
    }

    /// Moves the held modifiers to `next`, in Shift, Control, Alt order.
    pub fn set_mods(&mut self, next: Mods) -> Vec<String> {
        let mut commands = Vec::new();
        for (current, wanted, keysym) in [
            (self.mods.shift, next.shift, SHIFT_L),
            (self.mods.control, next.control, CONTROL_L),
            (self.mods.alt, next.alt, ALT_L),
        ] {
            if current != wanted {
                commands.push(format!("{},{keysym}", if wanted { "kd" } else { "ku" }));
            }
        }
        self.mods = next;
        commands
    }

    /// Latches "⌘ means Control" until ⌘ lifts: a Skerry chord is being forwarded.
    pub fn latch_command(&mut self) {
        self.latch.engage();
    }

    /// [`Self::mac_modifiers_with`] with no physical key state: only keys pressed under ⌘
    /// are released when it lifts.
    #[cfg(test)]
    pub fn mac_modifiers(&mut self, next: Modifiers, command_as_control: bool) -> Vec<String> {
        self.mac_modifiers_with(next, command_as_control, &|_| None)
    }

    /// Follows the Mac's modifiers. AppKit drops every key-up while ⌘ is down, so when ⌘
    /// lifts each held key is checked against `is_down` (the keyboard's real state; `None`
    /// when unknown): one no longer down is released, one still down stays held. A key of
    /// unknown state is released only if it went down under ⌘. Then the latch clears and the
    /// modifiers move, so Control comes up last.
    pub fn mac_modifiers_with(
        &mut self,
        next: Modifiers,
        command_as_control: bool,
        is_down: &dyn Fn(&str) -> Option<bool>,
    ) -> Vec<String> {
        let mut commands = if self.command_held && !next.platform {
            self.release_lifted_keys(is_down)
        } else {
            Vec::new()
        };
        self.command_held = next.platform;
        self.latch.observe(next);
        commands.extend(self.set_mods(Mods::from_mac(
            next,
            self.latch.as_control(command_as_control),
        )));
        commands
    }

    /// Releases the held keys that are no longer down (see [`Self::mac_modifiers_with`]).
    fn release_lifted_keys(&mut self, is_down: &dyn Fn(&str) -> Option<bool>) -> Vec<String> {
        let command_keys = std::mem::take(&mut self.command_keys);
        let lifted: Vec<String> = self
            .keys
            .keys()
            .filter(|code| match is_down(code) {
                Some(down) => !down,
                None => command_keys.contains(*code),
            })
            .cloned()
            .collect();
        lifted
            .into_iter()
            .filter_map(|code| self.keys.remove(&code))
            .map(|keysym| format!("ku,{keysym}"))
            .collect()
    }

    /// Lifts every held key, then every modifier, and clears the latch. Calling it again
    /// sends nothing.
    pub fn release_all(&mut self) -> Vec<String> {
        self.command_keys.clear();
        self.command_held = false;
        self.latch = CommandLatch::default();
        let mut commands: Vec<String> = std::mem::take(&mut self.keys)
            .into_values()
            .map(|keysym| format!("ku,{keysym}"))
            .collect();
        commands.extend(self.set_mods(Mods::default()));
        commands
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.mods == Mods::default()
    }

    pub fn control(&self) -> bool {
        self.mods.control
    }

    pub fn shift(&self) -> bool {
        self.mods.shift
    }
}

/// Whether a key-down with these modifiers goes to the session at all: with Command not
/// mapped to Control, Command shortcuts stay on the Mac.
pub fn sends_key_down(modifiers: Modifiers, command_as_control: bool) -> bool {
    !modifiers.platform || command_as_control
}

/// ⌘O and ⌘L are the Library's (File → Open…, Open Location…). In a session window the
/// desktop claims them, so the menu's key equivalents never pull the user out to the Library
/// (`ARCH-028`). With Command as Control they go to the remote app as Control+O / Control+L;
/// otherwise they are swallowed, since Skerry never sends Super.
pub fn claims_library_shortcut(modifiers: Modifiers, key: &str) -> bool {
    modifiers.platform
        && !modifiers.control
        && !modifiers.alt
        && !modifiers.shift
        && matches!(key, "o" | "l")
}

/// The macOS virtual key code (`kVK_*`, ANSI layout positions) of a GPUI key name, for the
/// keys a held-key check needs; `None` for the rest.
pub fn virtual_key(code: &str) -> Option<u16> {
    const LETTERS: [u16; 26] = [
        0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D, 0x1F,
        0x23, 0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06,
    ];
    const DIGITS: [u16; 10] = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
    let mut chars = code.chars();
    if let (Some(ch), None) = (chars.next(), chars.next()) {
        if ch.is_ascii_lowercase() {
            return Some(LETTERS[(ch as u8 - b'a') as usize]);
        }
        if ch.is_ascii_digit() {
            return Some(DIGITS[(ch as u8 - b'0') as usize]);
        }
    }
    Some(match code {
        "-" => 0x1B,
        "=" => 0x18,
        "[" => 0x21,
        "]" => 0x1E,
        "\\" => 0x2A,
        ";" => 0x29,
        "'" => 0x27,
        "," => 0x2B,
        "." => 0x2F,
        "/" => 0x2C,
        "`" => 0x32,
        "space" => 0x31,
        "enter" => 0x24,
        "tab" => 0x30,
        "backspace" => 0x33,
        "escape" => 0x35,
        "delete" => 0x75,
        "left" => 0x7B,
        "right" => 0x7C,
        "down" => 0x7D,
        "up" => 0x7E,
        "home" => 0x73,
        "end" => 0x77,
        "pageup" => 0x74,
        "pagedown" => 0x79,
        _ => return None,
    })
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
}

/// `kCGEventSourceStateHIDSystemState`: the keyboard's hardware state.
const HID_SYSTEM_STATE: i32 = 1;

/// Whether the key is physically down right now; `None` for a key with no known code.
pub fn physically_down(code: &str) -> Option<bool> {
    let key = virtual_key(code)?;
    // A read-only query of the HID key state; safe from any thread.
    Some(unsafe { CGEventSourceKeyState(HID_SYSTEM_STATE, key) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_claims_cmd_o_and_cmd_l() {
        assert!(claims_library_shortcut(mac(false, false, false, true), "o"));
        assert!(claims_library_shortcut(mac(false, false, false, true), "l"));
        assert!(!claims_library_shortcut(
            mac(false, false, false, false),
            "o"
        ));
        assert!(!claims_library_shortcut(mac(true, false, false, true), "o"));
        assert!(!claims_library_shortcut(
            mac(false, false, false, true),
            "w"
        ));
    }

    const SUPER_AND_META: [u32; 4] = [0xffe7, 0xffe8, 0xffeb, 0xffec];

    fn mac(shift: bool, control: bool, alt: bool, platform: bool) -> Modifiers {
        Modifiers {
            shift,
            control,
            alt,
            platform,
            ..Default::default()
        }
    }

    #[test]
    fn press_then_release_sends_kd_then_ku() {
        let mut ledger = KeyLedger::default();
        assert_eq!(ledger.press("a".into(), 0x61), ["kd,97"]);
        assert_eq!(ledger.release("a"), ["ku,97"]);
        assert!(ledger.is_empty());
    }

    #[test]
    fn a_held_letter_that_shift_turns_into_another_keysym_releases_the_first() {
        let mut ledger = KeyLedger::default();
        assert_eq!(ledger.press("a".into(), 0x61), ["kd,97"]);
        assert_eq!(ledger.press("a".into(), 0x61), ["kd,97"]);
        ledger.set_mods(Mods {
            shift: true,
            ..Mods::default()
        });
        assert_eq!(ledger.press("a".into(), 0x41), ["ku,97", "kd,65"]);
        assert_eq!(ledger.release("a"), ["ku,65"]);
        assert_eq!(ledger.release_all(), ["ku,65505"]);
    }

    #[test]
    fn release_of_an_unpressed_key_sends_nothing() {
        let mut ledger = KeyLedger::default();
        assert!(ledger.release("a").is_empty());
        ledger.press("a".into(), 0x61);
        ledger.release("a");
        assert!(ledger.release("a").is_empty());
    }

    #[test]
    fn release_all_lifts_every_key_then_every_modifier() {
        let mut ledger = KeyLedger::default();
        ledger.set_mods(Mods {
            shift: true,
            control: true,
            alt: false,
        });
        ledger.press("a".into(), 0x61);
        ledger.press("escape".into(), 0xff1b);
        let commands = ledger.release_all();
        assert_eq!(commands.len(), 4);
        let mut keys = commands[..2].to_vec();
        keys.sort();
        assert_eq!(keys, ["ku,65307", "ku,97"]);
        assert_eq!(commands[2..], ["ku,65505", "ku,65507"]);
        assert!(ledger.is_empty());
    }

    #[test]
    fn release_all_is_idempotent() {
        let mut ledger = KeyLedger::default();
        ledger.press("a".into(), 0x61);
        assert!(!ledger.release_all().is_empty());
        assert!(ledger.release_all().is_empty());
    }

    #[test]
    fn command_maps_to_control_only_when_enabled() {
        let command = mac(false, false, false, true);
        assert!(Mods::from_mac(command, true).control);
        assert!(!Mods::from_mac(command, false).control);
        assert!(sends_key_down(command, true));
        assert!(!sends_key_down(command, false));
        assert!(sends_key_down(mac(true, true, true, false), false));
    }

    #[test]
    fn command_never_sends_super_or_meta() {
        for bits in 0..16u8 {
            let modifiers = mac(bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
            for command_as_control in [true, false] {
                let mut ledger = KeyLedger::default();
                let mut commands = ledger.set_mods(Mods::from_mac(modifiers, command_as_control));
                commands.extend(ledger.release_all());
                for command in commands {
                    for keysym in SUPER_AND_META {
                        assert!(!command.ends_with(&format!(",{keysym}")), "{command}");
                    }
                }
            }
        }
    }

    #[test]
    fn modifier_release_order_is_stable() {
        let mut ledger = KeyLedger::default();
        let all = Mods {
            shift: true,
            control: true,
            alt: true,
        };
        assert_eq!(ledger.set_mods(all), ["kd,65505", "kd,65507", "kd,65513"]);
        assert_eq!(ledger.release_all(), ["ku,65505", "ku,65507", "ku,65513"]);
    }

    #[test]
    fn held_control_from_command_is_released_on_blur() {
        let mut ledger = KeyLedger::default();
        ledger.set_mods(Mods::from_mac(mac(false, false, false, true), true));
        assert!(ledger.control());
        ledger.press("escape".into(), 0xff1b);
        let commands = ledger.release_all();
        assert!(commands.contains(&"ku,65307".to_string()));
        assert_eq!(commands.last().map(String::as_str), Some("ku,65507"));
        assert!(!ledger.control());
    }

    #[test]
    fn a_forwarded_chord_holds_control_until_command_lifts() {
        let mut latch = CommandLatch::default();
        assert!(!latch.as_control(false));
        latch.engage();
        latch.observe(mac(true, false, false, true));
        assert!(latch.as_control(false));
        // Shift lifts first: ⌘ is still down, so Control stays.
        latch.observe(mac(false, false, false, true));
        assert!(latch.as_control(false));
        let mods = Mods::from_mac(mac(false, false, false, true), latch.as_control(false));
        assert!(mods.control && !mods.shift);
        latch.observe(mac(false, false, false, false));
        assert!(!latch.as_control(false));
        assert!(latch.as_control(true));

        let mut ledger = KeyLedger::default();
        let mut commands = ledger.mac_modifiers(mac(true, false, false, true), false);
        assert_eq!(commands, ["kd,65505"]);
        ledger.latch_command();
        commands.extend(ledger.mac_modifiers(mac(true, false, false, true), false));
        commands.extend(ledger.press("m".into(), 109));
        commands.extend(ledger.mac_modifiers(mac(false, false, false, true), false));
        commands.extend(ledger.mac_modifiers(mac(false, false, false, false), false));
        assert_eq!(
            commands,
            ["kd,65505", "kd,65507", "kd,109", "ku,65505", "ku,109", "ku,65507"]
        );
        for command in commands {
            for keysym in SUPER_AND_META {
                assert!(!command.ends_with(&format!(",{keysym}")), "{command}");
            }
        }
        assert!(ledger.is_empty());
    }

    #[test]
    fn a_command_chord_key_is_released_when_command_lifts() {
        let mut ledger = KeyLedger::default();
        ledger.mac_modifiers(mac(true, false, false, true), false);
        ledger.latch_command();
        ledger.mac_modifiers(mac(true, false, false, true), false);
        ledger.press("m".into(), 109);
        // No key-up for m: ⌘ lifts with Shift.
        assert_eq!(
            ledger.mac_modifiers(mac(false, false, false, false), false),
            ["ku,109", "ku,65505", "ku,65507"]
        );
        assert!(ledger.is_empty());
        // A key pressed without ⌘ stays held when ⌘ goes down and up.
        ledger.press("a".into(), 0x61);
        ledger.mac_modifiers(mac(false, false, false, true), false);
        assert!(ledger
            .mac_modifiers(mac(false, false, false, false), false)
            .is_empty());
        assert_eq!(ledger.release("a"), ["ku,97"]);
        // A key that came up normally isn't released again.
        ledger.mac_modifiers(mac(false, false, false, true), true);
        ledger.press("c".into(), 0x63);
        assert_eq!(ledger.release("c"), ["ku,99"]);
        assert_eq!(
            ledger.mac_modifiers(mac(false, false, false, false), true),
            ["ku,65507"]
        );
    }

    #[test]
    fn a_forwarded_chord_taps_its_key_while_control_stays_latched() {
        let mut ledger = KeyLedger::default();
        let held = mac(true, false, false, true);
        ledger.mac_modifiers(held, false);
        ledger.latch_command();
        let mut commands = ledger.mac_modifiers(held, false);
        commands.extend(ledger.tap_chord("m", 109));
        // A macOS autorepeat while ⌘ is held taps again; nothing stays down.
        commands.extend(ledger.tap_chord("m", 109));
        assert_eq!(
            commands,
            ["kd,65507", "kd,109", "ku,109", "kd,109", "ku,109"]
        );
        assert!(ledger.control());
        assert_eq!(
            ledger.mac_modifiers(mac(false, false, false, false), false),
            ["ku,65505", "ku,65507"]
        );
        assert!(ledger.is_empty());
    }

    #[test]
    fn a_letter_held_before_the_chord_is_not_released_when_command_lifts() {
        let mut ledger = KeyLedger::default();
        ledger.press("a".into(), 0x61);
        ledger.mac_modifiers(mac(false, false, false, true), true);
        // The held letter repeats under ⌘: still its own key.
        assert_eq!(ledger.press("a".into(), 0x61), ["kd,97"]);
        ledger.press("c".into(), 0x63);
        assert_eq!(
            ledger.mac_modifiers(mac(false, false, false, false), true),
            ["ku,99", "ku,65507"]
        );
        assert_eq!(ledger.release("a"), ["ku,97"]);
        assert!(ledger.is_empty());
    }

    #[test]
    fn a_letter_released_under_command_is_released_when_command_lifts() {
        // Hold a, press ⌘, release a (AppKit drops that key-up), then release ⌘.
        let mut ledger = KeyLedger::default();
        ledger.press("a".into(), 0x61);
        ledger.mac_modifiers_with(mac(false, false, false, true), true, &|_| Some(true));
        let up = |_: &str| Some(false);
        assert_eq!(
            ledger.mac_modifiers_with(mac(false, false, false, false), true, &up),
            ["ku,97", "ku,65507"]
        );
        assert!(ledger.is_empty());
    }

    #[test]
    fn keys_still_down_when_command_lifts_stay_held() {
        // A letter held before the chord, still held: kept. A ⌘ key let go: released.
        let mut ledger = KeyLedger::default();
        ledger.press("a".into(), 0x61);
        ledger.mac_modifiers_with(mac(false, false, false, true), true, &|_| Some(true));
        ledger.press("c".into(), 0x63);
        let real = |code: &str| Some(code == "a");
        assert_eq!(
            ledger.mac_modifiers_with(mac(false, false, false, false), true, &real),
            ["ku,99", "ku,65507"]
        );
        assert_eq!(ledger.release("a"), ["ku,97"]);
        assert!(ledger.is_empty());
    }

    #[test]
    fn gpui_key_names_map_to_ansi_virtual_keys() {
        assert_eq!(virtual_key("a"), Some(0x00));
        assert_eq!(virtual_key("m"), Some(0x2E));
        assert_eq!(virtual_key("z"), Some(0x06));
        assert_eq!(virtual_key("0"), Some(0x1D));
        assert_eq!(virtual_key("9"), Some(0x19));
        assert_eq!(virtual_key("escape"), Some(0x35));
        assert_eq!(virtual_key("f13"), None);
    }

    #[test]
    fn a_tap_is_never_held() {
        let mut ledger = KeyLedger::default();
        assert_eq!(ledger.tap(0x61), ["kd,97", "ku,97"]);
        assert!(ledger.release_all().is_empty());
    }
}
