//! Which keys and modifiers the session holds down, and the `kd` / `ku` commands that
//! change that (`ARCH-GAP-DC-09`).
//!
//! Pure bookkeeping, no GPUI state: `Desktop` feeds it Mac key events and sends every
//! command it returns, so release paths (blur, deactivate, child windows, stream close) can
//! be tested without a window.

use gpui_kit::Modifiers;
use std::collections::BTreeMap;

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

#[derive(Default)]
pub struct KeyLedger {
    /// Held keys by GPUI key name, so a key-up finds the keysym its key-down sent.
    keys: BTreeMap<String, u32>,
    mods: Mods,
}

impl KeyLedger {
    /// A key went down. A macOS auto-repeat sends `kd` again on purpose: Skerry's 1 s `kh`
    /// heartbeat alone would let the server's repeat loop pause. A repeat that now maps to a
    /// different keysym (a held `a` repeating as `A` once Shift goes down) releases the
    /// first keysym, or it would stay down on the server.
    pub fn press(&mut self, code: String, keysym: u32) -> Vec<String> {
        let mut commands = Vec::new();
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
        self.keys
            .remove(code)
            .map(|keysym| vec![format!("ku,{keysym}")])
            .unwrap_or_default()
    }

    /// One keystroke that is never held (an IME commit).
    pub fn tap(&self, keysym: u32) -> Vec<String> {
        vec![format!("kd,{keysym}"), format!("ku,{keysym}")]
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

    /// Lifts every held key, then every modifier. Calling it again sends nothing.
    pub fn release_all(&mut self) -> Vec<String> {
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
    fn a_tap_is_never_held() {
        let mut ledger = KeyLedger::default();
        assert_eq!(ledger.tap(0x61), ["kd,97", "ku,97"]);
        assert!(ledger.release_all().is_empty());
    }
}
