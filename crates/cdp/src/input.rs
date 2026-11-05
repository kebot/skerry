//! Input dispatch on a page Session. Coordinates are CSS pixels in the
//! remote page (use [`crate::FrameFit`] to map from the Mirror).

use std::ops::{BitOr, BitOrAssign};

use serde_json::{json, Value};

use crate::{CdpError, Session};

/// CDP modifier bitmask: alt=1, ctrl=2, meta=4, shift=8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Modifiers(pub u8);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const ALT: Modifiers = Modifiers(1);
    pub const CTRL: Modifiers = Modifiers(2);
    pub const META: Modifiers = Modifiers(4);
    pub const SHIFT: Modifiers = Modifiers(8);

    pub fn bits(self) -> u8 {
        self.0
    }
    pub fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, rhs: Modifiers) -> Modifiers {
        Modifiers(self.0 | rhs.0)
    }
}

impl BitOrAssign for Modifiers {
    fn bitor_assign(&mut self, rhs: Modifiers) {
        self.0 |= rhs.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Moved,
    Pressed,
    Released,
    Wheel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseButton {
    #[default]
    None,
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

impl MouseButton {
    fn as_str(self) -> &'static str {
        match self {
            MouseButton::None => "none",
            MouseButton::Left => "left",
            MouseButton::Middle => "middle",
            MouseButton::Right => "right",
            MouseButton::Back => "back",
            MouseButton::Forward => "forward",
        }
    }
    /// This button's bit in the `buttons` mask (left=1, right=2, middle=4,
    /// back=8, forward=16).
    pub fn mask(self) -> u32 {
        match self {
            MouseButton::None => 0,
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 4,
            MouseButton::Back => 8,
            MouseButton::Forward => 16,
        }
    }
}

/// `Input.dispatchMouseEvent`.
#[derive(Debug, Clone, PartialEq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub x: f64,
    pub y: f64,
    pub button: MouseButton,
    /// Bitmask of buttons held (see [`MouseButton::mask`]).
    pub buttons: u32,
    pub click_count: u32,
    pub modifiers: Modifiers,
    pub delta_x: f64,
    pub delta_y: f64,
}

impl MouseEvent {
    fn base(kind: MouseEventKind, x: f64, y: f64) -> Self {
        MouseEvent {
            kind,
            x,
            y,
            button: MouseButton::None,
            buttons: 0,
            click_count: 0,
            modifiers: Modifiers::NONE,
            delta_x: 0.0,
            delta_y: 0.0,
        }
    }
    pub fn moved(x: f64, y: f64) -> Self {
        Self::base(MouseEventKind::Moved, x, y)
    }
    /// Press; `buttons` is set to the button's own mask.
    pub fn pressed(x: f64, y: f64, button: MouseButton, click_count: u32) -> Self {
        MouseEvent {
            button,
            buttons: button.mask(),
            click_count,
            ..Self::base(MouseEventKind::Pressed, x, y)
        }
    }
    /// Release; `buttons` is 0 (no other buttons held).
    pub fn released(x: f64, y: f64, button: MouseButton, click_count: u32) -> Self {
        MouseEvent {
            button,
            click_count,
            ..Self::base(MouseEventKind::Released, x, y)
        }
    }
    pub fn wheel(x: f64, y: f64, delta_x: f64, delta_y: f64) -> Self {
        MouseEvent {
            delta_x,
            delta_y,
            ..Self::base(MouseEventKind::Wheel, x, y)
        }
    }
    pub fn with_modifiers(mut self, m: Modifiers) -> Self {
        self.modifiers = m;
        self
    }
    pub fn with_buttons(mut self, buttons: u32) -> Self {
        self.buttons = buttons;
        self
    }

    fn to_params(&self) -> Value {
        let ty = match self.kind {
            MouseEventKind::Moved => "mouseMoved",
            MouseEventKind::Pressed => "mousePressed",
            MouseEventKind::Released => "mouseReleased",
            MouseEventKind::Wheel => "mouseWheel",
        };
        let mut p = json!({
            "type": ty,
            "x": self.x,
            "y": self.y,
            "button": self.button.as_str(),
            "buttons": self.buttons,
            "clickCount": self.click_count,
            "modifiers": self.modifiers.bits(),
        });
        if self.kind == MouseEventKind::Wheel {
            p["deltaX"] = json!(self.delta_x);
            p["deltaY"] = json!(self.delta_y);
        }
        p
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventKind {
    KeyDown,
    KeyUp,
    RawKeyDown,
    Char,
}

/// `Input.dispatchKeyEvent`.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyEvent {
    pub kind: KeyEventKind,
    /// DOM `key` value, e.g. "a", "Enter".
    pub key: String,
    /// DOM `code` value, e.g. "KeyA".
    pub code: Option<String>,
    /// Text generated by the key (keyDown/char).
    pub text: Option<String>,
    pub windows_virtual_key_code: Option<i32>,
    pub modifiers: Modifiers,
}

impl KeyEvent {
    pub fn new(kind: KeyEventKind, key: impl Into<String>) -> Self {
        KeyEvent {
            kind,
            key: key.into(),
            code: None,
            text: None,
            windows_virtual_key_code: None,
            modifiers: Modifiers::NONE,
        }
    }
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }
    pub fn with_virtual_key_code(mut self, vk: i32) -> Self {
        self.windows_virtual_key_code = Some(vk);
        self
    }
    pub fn with_modifiers(mut self, m: Modifiers) -> Self {
        self.modifiers = m;
        self
    }

    fn to_params(&self) -> Value {
        let ty = match self.kind {
            KeyEventKind::KeyDown => "keyDown",
            KeyEventKind::KeyUp => "keyUp",
            KeyEventKind::RawKeyDown => "rawKeyDown",
            KeyEventKind::Char => "char",
        };
        let mut p = json!({"type": ty, "key": self.key, "modifiers": self.modifiers.bits()});
        if let Some(c) = &self.code {
            p["code"] = json!(c);
        }
        if let Some(t) = &self.text {
            p["text"] = json!(t);
        }
        if let Some(vk) = self.windows_virtual_key_code {
            p["windowsVirtualKeyCode"] = json!(vk);
        }
        p
    }
}

impl Session {
    /// `Input.dispatchMouseEvent` (CSS pixels in the remote page).
    pub async fn dispatch_mouse(&self, ev: MouseEvent) -> Result<(), CdpError> {
        self.send("Input.dispatchMouseEvent", Some(ev.to_params()))
            .await
            .map(|_| ())
    }

    /// `Input.dispatchKeyEvent`.
    pub async fn dispatch_key(&self, ev: KeyEvent) -> Result<(), CdpError> {
        self.send("Input.dispatchKeyEvent", Some(ev.to_params()))
            .await
            .map(|_| ())
    }

    /// `Input.insertText`: insert text at the caret (IME / paste style).
    pub async fn insert_text(&self, text: &str) -> Result<(), CdpError> {
        self.send("Input.insertText", Some(json!({"text": text})))
            .await
            .map(|_| ())
    }
}
