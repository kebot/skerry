//! Skerry's own chords in a session window, and whether they act on the Mac or go to the
//! remote session (`ARCH-031`, `ARCH-GAP-DC-12`).
//!
//! [`CHORDS`] is the one list: `app::key_bindings` binds from it, the Shortcuts card lists
//! it, and [`suppress_bindings`] hands every passable chord to the session while **Mac
//! shortcuts active** is off. Pure, apart from GPUI's key types.

use gpui_kit::{KeyBinding, Keystroke, NoAction};
use serde_json::Value;

/// Which Skerry command a chord runs; `app::key_bindings` maps each to its action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordId {
    SessionControls,
    FullScreen,
    Files,
    Library,
    /// Capture Pointer (`ARCH-032`): always on the Mac, the one way out of a capture.
    CapturePointer,
    Quit,
}

#[derive(Clone, Copy, Debug)]
pub struct SkerryChord {
    pub id: ChordId,
    pub label: &'static str,
    /// A GPUI binding string, as `KeyBinding::new` reads it.
    pub keystroke: &'static str,
    /// Never handed to the session (⌘Q, ⌃⌥⇧Z).
    pub always_local: bool,
}

/// Every chord that acts in a session window, in the Shortcuts card's order. Library-scoped
/// chords (⌘O, ⌘L) and the Files / Session Controls windows' own chords are not here: they
/// never act in a session window.
pub const CHORDS: [SkerryChord; 6] = [
    SkerryChord {
        id: ChordId::SessionControls,
        label: "Session Controls",
        keystroke: "cmd-shift-m",
        always_local: false,
    },
    SkerryChord {
        id: ChordId::FullScreen,
        label: "Full Screen",
        keystroke: "ctrl-cmd-f",
        always_local: false,
    },
    SkerryChord {
        id: ChordId::Files,
        label: "Files",
        keystroke: "cmd-shift-f",
        always_local: false,
    },
    SkerryChord {
        id: ChordId::Library,
        label: "Library",
        keystroke: "cmd-0",
        always_local: false,
    },
    // A capture you can't leave by keyboard is a trap: the menu bar is out of reach while
    // the cursor is frozen, so this one never goes to the session (`ARCH-032`).
    SkerryChord {
        id: ChordId::CapturePointer,
        label: "Capture Pointer",
        keystroke: "ctrl-alt-shift-z",
        always_local: true,
    },
    SkerryChord {
        id: ChordId::Quit,
        label: "Quit",
        keystroke: "cmd-q",
        always_local: true,
    },
];

/// The flag the session desktop adds to its key context while Mac shortcuts are off.
#[cfg(test)]
const REMOTE_FLAG: &str = "remote_shortcuts";
/// The desktop's key context while Mac shortcuts are off (`view::DESKTOP_CONTEXT` plus
/// the `remote_shortcuts` flag).
pub const REMOTE_DESKTOP_CONTEXT: &str = "SessionDesktop remote_shortcuts";
/// The predicate the suppressing bindings match.
pub const REMOTE_PREDICATE: &str = "SessionDesktop && remote_shortcuts";

/// The server's `keyboard_shortcuts` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortcutPolicy {
    pub value: bool,
    pub locked: bool,
}

impl Default for ShortcutPolicy {
    fn default() -> Self {
        Self {
            value: true,
            locked: false,
        }
    }
}

impl ShortcutPolicy {
    /// Reads `keyboard_shortcuts` from `server_settings`; on unless it says otherwise.
    pub fn from_server(settings: &Value) -> Self {
        let entry = &settings["keyboard_shortcuts"];
        Self {
            value: entry["value"].as_bool().unwrap_or(true),
            locked: entry["locked"] == true,
        }
    }
}

/// Whether Mac shortcuts are active: a lock wins, then the saved choice, then the server's
/// value. Before the server's settings arrive, the saved choice or on.
pub fn effective(policy: Option<ShortcutPolicy>, saved: Option<bool>) -> bool {
    match policy {
        Some(policy) if policy.locked => policy.value,
        Some(policy) => saved.unwrap_or(policy.value),
        None => saved.unwrap_or(true),
    }
}

/// One `NoAction` binding per passable chord under [`REMOTE_PREDICATE`]. Registered after
/// the global bindings, they outrank them while the desktop's context has the flag, so the
/// keystroke falls through to the desktop's key-down listener.
pub fn suppress_bindings() -> Vec<KeyBinding> {
    CHORDS
        .iter()
        .filter(|chord| !chord.always_local)
        .map(|chord| KeyBinding::new(chord.keystroke, NoAction, Some(REMOTE_PREDICATE)))
        .collect()
}

/// Whether the desktop forwards this keystroke as a Skerry chord: exactly the keystrokes
/// [`suppress_bindings`] suppresses, matched the way GPUI matches them, and only while Mac
/// shortcuts are off.
pub fn passes_to_session(keystroke: &Keystroke, mac_active: bool) -> bool {
    thread_local! {
        /// Parsed once per thread (GPUI's bindings hold `Rc`s); key-downs only match.
        static SUPPRESSED: Vec<KeyBinding> = suppress_bindings();
    }
    !mac_active
        && SUPPRESSED.with(|bindings| {
            bindings
                .iter()
                .any(|binding| binding.match_keystrokes(&[keystroke]) == Some(false))
        })
}

/// The chord with this id.
pub fn chord(id: ChordId) -> &'static SkerryChord {
    CHORDS
        .iter()
        .find(|chord| chord.id == id)
        .expect("every ChordId is in CHORDS")
}

/// A chord as macOS menus write it: the modifiers in ⌃⌥⇧⌘ order, and the key.
pub fn glyphs(keystroke: &'static str) -> (String, String) {
    let Ok(parsed) = Keystroke::parse(keystroke) else {
        return (String::new(), keystroke.to_uppercase());
    };
    let m = parsed.modifiers;
    let modifiers = [
        (m.control, "⌃"),
        (m.alt, "⌥"),
        (m.shift, "⇧"),
        (m.platform, "⌘"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, glyph)| glyph)
    .collect();
    (modifiers, parsed.key.to_uppercase())
}

/// The chord as the session receives it: ⌘ sent as Control, in Ctrl, Alt, Shift order.
pub fn remote_label(keystroke: &'static str) -> String {
    let Ok(parsed) = Keystroke::parse(keystroke) else {
        return keystroke.to_string();
    };
    let m = parsed.modifiers;
    let mut parts: Vec<String> = [
        (m.control || m.platform, "Ctrl"),
        (m.alt, "Alt"),
        (m.shift, "Shift"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, name)| name.to_string())
    .collect();
    parts.push(parsed.key.to_uppercase());
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn typed(source: &'static str) -> Keystroke {
        Keystroke::parse(source).unwrap()
    }

    #[test]
    fn policy_reads_value_and_lock() {
        let locked = json!({"keyboard_shortcuts": {"value": false, "locked": true}});
        assert_eq!(
            ShortcutPolicy::from_server(&locked),
            ShortcutPolicy {
                value: false,
                locked: true
            }
        );
        assert_eq!(
            ShortcutPolicy::from_server(&json!({})),
            ShortcutPolicy::default()
        );
        let odd = json!({"keyboard_shortcuts": {"value": "no"}});
        assert_eq!(ShortcutPolicy::from_server(&odd), ShortcutPolicy::default());
    }

    #[test]
    fn effective_prefers_saved_unless_locked() {
        let policy = |value, locked| Some(ShortcutPolicy { value, locked });
        assert!(!effective(policy(true, false), Some(false)));
        assert!(!effective(policy(false, false), None));
        assert!(!effective(policy(false, true), Some(true)));
        assert!(effective(policy(true, true), Some(false)));
    }

    #[test]
    fn effective_defaults_on_before_settings() {
        assert!(effective(None, None));
        assert!(!effective(None, Some(false)));
    }

    #[test]
    fn only_listed_chords_pass_and_only_when_off() {
        for chord in ["cmd-shift-m", "ctrl-cmd-f", "cmd-shift-f", "cmd-0"] {
            assert!(passes_to_session(&typed(chord), false), "{chord}");
            assert!(!passes_to_session(&typed(chord), true), "{chord}");
        }
        for other in [
            "cmd-q",
            "ctrl-alt-shift-z",
            "cmd-c",
            "cmd-shift-n",
            "ctrl-shift-m",
            "m",
            "cmd-m",
        ] {
            assert!(!passes_to_session(&typed(other), false), "{other}");
        }
        // Shift arrives as key `m` with key_char `M`.
        let shifted = Keystroke {
            key_char: Some("M".into()),
            ..typed("cmd-shift-m")
        };
        assert!(passes_to_session(&shifted, false));
    }

    #[test]
    fn glyphs_follow_mac_modifier_order() {
        assert_eq!(glyphs("ctrl-cmd-f"), ("⌃⌘".into(), "F".into()));
        assert_eq!(glyphs("cmd-shift-m"), ("⇧⌘".into(), "M".into()));
        assert_eq!(glyphs("cmd-0"), ("⌘".into(), "0".into()));
        assert_eq!(glyphs("ctrl-alt-shift-cmd-x"), ("⌃⌥⇧⌘".into(), "X".into()));
    }

    #[test]
    fn remote_label_sends_command_as_control() {
        assert_eq!(remote_label("cmd-shift-m"), "Ctrl+Shift+M");
        assert_eq!(remote_label("ctrl-cmd-f"), "Ctrl+F");
        assert_eq!(remote_label("cmd-0"), "Ctrl+0");
    }

    #[test]
    fn quit_is_never_suppressed() {
        let suppressed = suppress_bindings();
        assert_eq!(suppressed.len(), CHORDS.len() - 2);
        assert!(suppressed
            .iter()
            .all(|binding| binding.match_keystrokes(&[typed("cmd-q")]).is_none()));
    }

    #[test]
    fn capture_chord_is_listed_and_always_local() {
        let capture = chord(ChordId::CapturePointer);
        assert_eq!(capture.label, "Capture Pointer");
        assert_eq!(capture.keystroke, "ctrl-alt-shift-z");
        assert!(capture.always_local);
        // Directly before Quit: the two always-local rows sit together at the bottom.
        let ids: Vec<ChordId> = CHORDS.iter().map(|chord| chord.id).collect();
        assert_eq!(&ids[4..], [ChordId::CapturePointer, ChordId::Quit]);
        assert_eq!(glyphs(capture.keystroke), ("⌃⌥⇧".into(), "Z".into()));
        assert!(suppress_bindings().iter().all(|binding| binding
            .match_keystrokes(&[typed(capture.keystroke)])
            .is_none()));
    }

    #[test]
    fn the_remote_context_is_the_desktop_context_plus_the_flag() {
        assert_eq!(
            REMOTE_DESKTOP_CONTEXT,
            format!("{} {REMOTE_FLAG}", crate::view::DESKTOP_CONTEXT)
        );
        assert_eq!(
            REMOTE_PREDICATE,
            format!("{} && {REMOTE_FLAG}", crate::view::DESKTOP_CONTEXT)
        );
    }
}
