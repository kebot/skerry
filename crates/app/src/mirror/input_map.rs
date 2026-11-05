//! Pure mapping from GPUI input to CDP input for the Mirror (no windows, no
//! I/O, unit-tested).
//!
//! Rules:
//! * Every Cmd (platform) key combination is an app shortcut and is never
//!   forwarded to the page ([`is_app_shortcut`]).
//! * Printable keys become `keyDown` with `text`; non-printable keys (arrows,
//!   Backspace, ...) become `rawKeyDown` with `windowsVirtualKeyCode`, `code`
//!   and `key`. Enter carries text `"\r"` so pages see a keypress.
//! * Mouse positions are mapped with [`FrameFit`]: ignored in the letterbox
//!   bars, clamped while a button is held (drags).

use gpui::{Keystroke, Modifiers, ScrollDelta};
use rb_cdp::{
    FrameFit, KeyEvent, KeyEventKind, Modifiers as CdpModifiers, MouseButton as CdpButton, Point,
};

/// Pixels per line for non-precise (line based) wheels.
pub const PIXELS_PER_LINE: f32 = 40.0;

/// True when the modifiers make this an app shortcut (Cmd on macOS). Such
/// key events are not forwarded to the page.
pub fn is_app_shortcut(m: &Modifiers) -> bool {
    m.platform
}

/// CDP modifier mask (alt=1, ctrl=2, meta=4, shift=8) for GPUI modifiers.
pub fn cdp_modifiers(m: &Modifiers) -> CdpModifiers {
    let mut out = CdpModifiers::NONE;
    if m.alt {
        out |= CdpModifiers::ALT;
    }
    if m.control {
        out |= CdpModifiers::CTRL;
    }
    if m.platform {
        out |= CdpModifiers::META;
    }
    if m.shift {
        out |= CdpModifiers::SHIFT;
    }
    out
}

/// GPUI mouse button to CDP button.
pub fn cdp_button(b: gpui::MouseButton) -> CdpButton {
    match b {
        gpui::MouseButton::Left => CdpButton::Left,
        gpui::MouseButton::Right => CdpButton::Right,
        gpui::MouseButton::Middle => CdpButton::Middle,
        gpui::MouseButton::Navigate(gpui::NavigationDirection::Back) => CdpButton::Back,
        gpui::MouseButton::Navigate(gpui::NavigationDirection::Forward) => CdpButton::Forward,
    }
}

/// Map a Mirror-local position to remote CSS pixels. `None` in the letterbox
/// bars unless a button is held (`dragging`), in which case the position is
/// clamped to the remote viewport so drags keep working outside the bars.
pub fn map_position(fit: &FrameFit, p: Point, dragging: bool) -> Option<Point> {
    if fit.is_empty() {
        return None;
    }
    if dragging {
        Some(fit.mirror_to_remote_clamped(p))
    } else {
        fit.mirror_to_remote(p)
    }
}

/// CDP wheel deltas `(deltaX, deltaY)` for a GPUI scroll event.
///
/// macOS reports `scrollingDeltaY` as the direction the *content* moves, with
/// the user's "natural scrolling" preference already applied (finger up with
/// natural scrolling, or wheel down without it, both give a negative value
/// and scroll the page down). CDP `deltaY > 0` scrolls the page down, so both
/// axes are negated and the OS preference is honoured automatically.
pub fn wheel_deltas(d: &ScrollDelta) -> (f64, f64) {
    let (x, y) = match d {
        ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
        ScrollDelta::Lines(p) => (p.x * PIXELS_PER_LINE, p.y * PIXELS_PER_LINE),
    };
    (-(x as f64), -(y as f64))
}

/// Static description of a non-printable or special key.
struct Special {
    gpui: &'static str,
    key: &'static str,
    code: &'static str,
    vk: i32,
    /// Text for `keyDown` (Enter only).
    text: Option<&'static str>,
}

const SPECIALS: &[Special] = &[
    Special {
        gpui: "enter",
        key: "Enter",
        code: "Enter",
        vk: 13,
        text: Some("\r"),
    },
    Special {
        gpui: "tab",
        key: "Tab",
        code: "Tab",
        vk: 9,
        text: None,
    },
    Special {
        gpui: "backspace",
        key: "Backspace",
        code: "Backspace",
        vk: 8,
        text: None,
    },
    Special {
        gpui: "escape",
        key: "Escape",
        code: "Escape",
        vk: 27,
        text: None,
    },
    Special {
        gpui: "delete",
        key: "Delete",
        code: "Delete",
        vk: 46,
        text: None,
    },
    Special {
        gpui: "insert",
        key: "Insert",
        code: "Insert",
        vk: 45,
        text: None,
    },
    Special {
        gpui: "home",
        key: "Home",
        code: "Home",
        vk: 36,
        text: None,
    },
    Special {
        gpui: "end",
        key: "End",
        code: "End",
        vk: 35,
        text: None,
    },
    Special {
        gpui: "pageup",
        key: "PageUp",
        code: "PageUp",
        vk: 33,
        text: None,
    },
    Special {
        gpui: "pagedown",
        key: "PageDown",
        code: "PageDown",
        vk: 34,
        text: None,
    },
    Special {
        gpui: "left",
        key: "ArrowLeft",
        code: "ArrowLeft",
        vk: 37,
        text: None,
    },
    Special {
        gpui: "up",
        key: "ArrowUp",
        code: "ArrowUp",
        vk: 38,
        text: None,
    },
    Special {
        gpui: "right",
        key: "ArrowRight",
        code: "ArrowRight",
        vk: 39,
        text: None,
    },
    Special {
        gpui: "down",
        key: "ArrowDown",
        code: "ArrowDown",
        vk: 40,
        text: None,
    },
    Special {
        gpui: "f1",
        key: "F1",
        code: "F1",
        vk: 112,
        text: None,
    },
    Special {
        gpui: "f2",
        key: "F2",
        code: "F2",
        vk: 113,
        text: None,
    },
    Special {
        gpui: "f3",
        key: "F3",
        code: "F3",
        vk: 114,
        text: None,
    },
    Special {
        gpui: "f4",
        key: "F4",
        code: "F4",
        vk: 115,
        text: None,
    },
    Special {
        gpui: "f5",
        key: "F5",
        code: "F5",
        vk: 116,
        text: None,
    },
    Special {
        gpui: "f6",
        key: "F6",
        code: "F6",
        vk: 117,
        text: None,
    },
    Special {
        gpui: "f7",
        key: "F7",
        code: "F7",
        vk: 118,
        text: None,
    },
    Special {
        gpui: "f8",
        key: "F8",
        code: "F8",
        vk: 119,
        text: None,
    },
    Special {
        gpui: "f9",
        key: "F9",
        code: "F9",
        vk: 120,
        text: None,
    },
    Special {
        gpui: "f10",
        key: "F10",
        code: "F10",
        vk: 121,
        text: None,
    },
    Special {
        gpui: "f11",
        key: "F11",
        code: "F11",
        vk: 122,
        text: None,
    },
    Special {
        gpui: "f12",
        key: "F12",
        code: "F12",
        vk: 123,
        text: None,
    },
];

/// `(character on the unshifted US key, code, windowsVirtualKeyCode)` for
/// punctuation, space and digits.
const PUNCT: &[(&str, &str, i32)] = &[
    (" ", "Space", 32),
    ("space", "Space", 32),
    (";", "Semicolon", 186),
    ("=", "Equal", 187),
    (",", "Comma", 188),
    ("-", "Minus", 189),
    (".", "Period", 190),
    ("/", "Slash", 191),
    ("`", "Backquote", 192),
    ("[", "BracketLeft", 219),
    ("\\", "Backslash", 220),
    ("]", "BracketRight", 221),
    ("'", "Quote", 222),
];

/// `(code, vk)` for a physical key named by GPUI's (unshifted, ASCII) `key`.
fn code_and_vk(key: &str) -> Option<(String, i32)> {
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_none() {
        if c.is_ascii_alphabetic() {
            let up = c.to_ascii_uppercase();
            return Some((format!("Key{up}"), up as i32));
        }
        if c.is_ascii_digit() {
            return Some((format!("Digit{c}"), c as i32));
        }
    }
    PUNCT
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, code, vk)| ((*code).to_string(), *vk))
}

/// The CDP events for a key press (`is_held`: auto-repeat). `None` when the
/// key must not be forwarded (Cmd combinations, unknown keys).
pub fn key_down_events(ks: &Keystroke) -> Option<KeyEvent> {
    if is_app_shortcut(&ks.modifiers) {
        return None;
    }
    let mods = cdp_modifiers(&ks.modifiers);
    if let Some(s) = SPECIALS.iter().find(|s| s.gpui == ks.key) {
        let (kind, text) = match s.text {
            Some(t) => (KeyEventKind::KeyDown, Some(t)),
            None => (KeyEventKind::RawKeyDown, None),
        };
        let mut ev = KeyEvent::new(kind, s.key)
            .with_code(s.code)
            .with_virtual_key_code(s.vk)
            .with_modifiers(mods);
        if let Some(t) = text {
            ev = ev.with_text(t);
        }
        return Some(ev);
    }
    let (code, vk) = code_and_vk(&ks.key)?;
    // Ctrl-chords produce no text; everything else uses the typed character
    // (shifted, or what Alt produces).
    let typed = if ks.modifiers.control {
        None
    } else {
        ks.key_char.as_deref().filter(|t| !t.is_empty())
    };
    let key = typed.map(str::to_string).unwrap_or_else(|| ks.key.clone());
    let key = if key == "space" { " ".to_string() } else { key };
    let mut ev = KeyEvent::new(
        if typed.is_some() {
            KeyEventKind::KeyDown
        } else {
            KeyEventKind::RawKeyDown
        },
        key,
    )
    .with_code(code)
    .with_virtual_key_code(vk)
    .with_modifiers(mods);
    if let Some(t) = typed {
        ev = ev.with_text(t);
    }
    Some(ev)
}

/// The CDP `keyUp` for a key release (same filtering as [`key_down_events`]).
pub fn key_up_event(ks: &Keystroke) -> Option<KeyEvent> {
    let down = key_down_events(ks)?;
    let mut up = KeyEvent::new(KeyEventKind::KeyUp, down.key).with_modifiers(down.modifiers);
    up.code = down.code;
    up.windows_virtual_key_code = down.windows_virtual_key_code;
    Some(up)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px, NavigationDirection};

    fn ks(key: &str, key_char: Option<&str>, f: impl FnOnce(&mut Modifiers)) -> Keystroke {
        let mut modifiers = Modifiers::default();
        f(&mut modifiers);
        Keystroke {
            modifiers,
            key: key.into(),
            key_char: key_char.map(Into::into),
        }
    }

    #[test]
    fn modifier_mask() {
        let m = |c, a, s, p| Modifiers {
            control: c,
            alt: a,
            shift: s,
            platform: p,
            function: false,
        };
        assert_eq!(cdp_modifiers(&m(false, false, false, false)).bits(), 0);
        assert_eq!(cdp_modifiers(&m(false, true, false, false)).bits(), 1);
        assert_eq!(cdp_modifiers(&m(true, false, false, false)).bits(), 2);
        assert_eq!(cdp_modifiers(&m(false, false, false, true)).bits(), 4);
        assert_eq!(cdp_modifiers(&m(false, false, true, false)).bits(), 8);
        assert_eq!(cdp_modifiers(&m(true, true, true, false)).bits(), 11);
    }

    #[test]
    fn cmd_combinations_are_never_forwarded() {
        for key in ["a", "q", "b", "enter", "left", "1", "space", "f5"] {
            let k = ks(key, Some("x"), |m| m.platform = true);
            assert!(is_app_shortcut(&k.modifiers));
            assert!(key_down_events(&k).is_none(), "{key}");
            assert!(key_up_event(&k).is_none(), "{key}");
        }
        let k = ks("a", Some("A"), |m| {
            m.platform = true;
            m.shift = true;
        });
        assert!(key_down_events(&k).is_none());
        assert!(!is_app_shortcut(&Modifiers::default()));
    }

    #[test]
    fn printable_letter_has_text_code_and_vk() {
        let ev = key_down_events(&ks("a", Some("a"), |_| {})).unwrap();
        assert_eq!(ev.kind, KeyEventKind::KeyDown);
        assert_eq!(ev.key, "a");
        assert_eq!(ev.code.as_deref(), Some("KeyA"));
        assert_eq!(ev.text.as_deref(), Some("a"));
        assert_eq!(ev.windows_virtual_key_code, Some(65));
        assert_eq!(ev.modifiers.bits(), 0);
    }

    #[test]
    fn shifted_symbol_keeps_physical_code() {
        let ev = key_down_events(&ks("1", Some("!"), |m| m.shift = true)).unwrap();
        assert_eq!(ev.key, "!");
        assert_eq!(ev.text.as_deref(), Some("!"));
        assert_eq!(ev.code.as_deref(), Some("Digit1"));
        assert_eq!(ev.windows_virtual_key_code, Some(49));
        assert_eq!(ev.modifiers.bits(), 8);
    }

    #[test]
    fn space_and_punctuation() {
        let ev = key_down_events(&ks("space", Some(" "), |_| {})).unwrap();
        assert_eq!(ev.key, " ");
        assert_eq!(ev.code.as_deref(), Some("Space"));
        assert_eq!(ev.windows_virtual_key_code, Some(32));
        let ev = key_down_events(&ks("-", Some("-"), |_| {})).unwrap();
        assert_eq!(ev.code.as_deref(), Some("Minus"));
        assert_eq!(ev.windows_virtual_key_code, Some(189));
    }

    #[test]
    fn ctrl_chord_has_no_text() {
        let ev = key_down_events(&ks("a", Some("a"), |m| m.control = true)).unwrap();
        assert_eq!(ev.kind, KeyEventKind::RawKeyDown);
        assert_eq!(ev.text, None);
        assert_eq!(ev.key, "a");
        assert_eq!(ev.modifiers.bits(), 2);
    }

    #[test]
    fn non_printables_are_raw_with_vk() {
        let table = [
            ("left", "ArrowLeft", 37),
            ("up", "ArrowUp", 38),
            ("right", "ArrowRight", 39),
            ("down", "ArrowDown", 40),
            ("tab", "Tab", 9),
            ("escape", "Escape", 27),
            ("backspace", "Backspace", 8),
            ("delete", "Delete", 46),
            ("home", "Home", 36),
            ("end", "End", 35),
            ("pageup", "PageUp", 33),
            ("pagedown", "PageDown", 34),
            ("f5", "F5", 116),
        ];
        for (g, dom, vk) in table {
            // GPUI sets key_char for tab; it must be ignored.
            let ev = key_down_events(&ks(g, Some("\t"), |_| {})).unwrap();
            assert_eq!(ev.kind, KeyEventKind::RawKeyDown, "{g}");
            assert_eq!(ev.key, dom);
            assert_eq!(ev.code.as_deref(), Some(dom));
            assert_eq!(ev.windows_virtual_key_code, Some(vk));
            assert_eq!(ev.text, None);
        }
    }

    #[test]
    fn enter_carries_carriage_return_text() {
        let ev = key_down_events(&ks("enter", Some("\n"), |_| {})).unwrap();
        assert_eq!(ev.kind, KeyEventKind::KeyDown);
        assert_eq!(ev.key, "Enter");
        assert_eq!(ev.text.as_deref(), Some("\r"));
        assert_eq!(ev.windows_virtual_key_code, Some(13));
    }

    #[test]
    fn key_up_has_no_text() {
        let up = key_up_event(&ks("a", Some("a"), |m| m.shift = true)).unwrap();
        assert_eq!(up.kind, KeyEventKind::KeyUp);
        assert_eq!(up.text, None);
        assert_eq!(up.code.as_deref(), Some("KeyA"));
        assert_eq!(up.modifiers.bits(), 8);
        let up = key_up_event(&ks("left", None, |_| {})).unwrap();
        assert_eq!(up.key, "ArrowLeft");
        assert_eq!(up.windows_virtual_key_code, Some(37));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        assert!(key_down_events(&ks("capslock", None, |_| {})).is_none());
        assert!(key_down_events(&ks("ß", Some("ß"), |_| {})).is_none());
    }

    #[test]
    fn scroll_direction_is_negated() {
        // Content moving down (positive macOS delta) scrolls the page up.
        let d = ScrollDelta::Pixels(point(px(3.0), px(10.0)));
        assert_eq!(wheel_deltas(&d), (-3.0, -10.0));
        // Finger up / wheel down gives a negative macOS delta: page scrolls down.
        let d = ScrollDelta::Pixels(point(px(0.0), px(-25.0)));
        assert_eq!(wheel_deltas(&d), (-0.0, 25.0));
        let d = ScrollDelta::Lines(point(0.0, -1.0));
        assert_eq!(wheel_deltas(&d), (-0.0, f64::from(PIXELS_PER_LINE)));
    }

    #[test]
    fn mouse_buttons() {
        assert_eq!(cdp_button(gpui::MouseButton::Left), CdpButton::Left);
        assert_eq!(cdp_button(gpui::MouseButton::Right), CdpButton::Right);
        assert_eq!(cdp_button(gpui::MouseButton::Middle), CdpButton::Middle);
        assert_eq!(
            cdp_button(gpui::MouseButton::Navigate(NavigationDirection::Back)),
            CdpButton::Back
        );
    }

    #[test]
    fn position_mapping_ignores_bars_and_clamps_drags() {
        // 1000x500 remote in a 1000x1000 mirror: bars of 250 above and below.
        let fit = FrameFit::new(1000.0, 500.0, 1000.0, 1000.0);
        assert_eq!(map_position(&fit, Point::new(10.0, 100.0), false), None);
        let p = map_position(&fit, Point::new(10.0, 500.0), false).unwrap();
        assert!((p.x - 10.0).abs() < 1e-3 && (p.y - 250.0).abs() < 1e-3);
        let p = map_position(&fit, Point::new(-50.0, 100.0), true).unwrap();
        assert_eq!((p.x, p.y), (0.0, 0.0));
        assert_eq!(
            map_position(
                &FrameFit::new(0.0, 0.0, 1.0, 1.0),
                Point::new(1.0, 1.0),
                true
            ),
            None
        );
    }
}
