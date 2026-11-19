use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct InputSettings {
    pub command_as_control: bool,
    pub remote_cursor: bool,
    pub trackpad_scroll: bool,
    pub mac_ime: bool,
}

impl Default for InputSettings {
    fn default() -> Self {
        Self {
            command_as_control: false,
            remote_cursor: false,
            trackpad_scroll: true,
            mac_ime: false,
        }
    }
}

impl InputSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputFeature {
    CommandAsControl,
    RemoteCursor,
    TrackpadScroll,
    MacIme,
}

impl InputFeature {
    pub const ALL: [(Self, &'static str); 4] = [
        (Self::CommandAsControl, "Command sends Control"),
        (Self::RemoteCursor, "Remote Cursor"),
        (Self::TrackpadScroll, "Smooth Trackpad Scrolling"),
        (Self::MacIme, "Use Mac Input Method"),
    ];

    fn value(self, settings: &mut InputSettings) -> &mut bool {
        match self {
            Self::CommandAsControl => &mut settings.command_as_control,
            Self::RemoteCursor => &mut settings.remote_cursor,
            Self::TrackpadScroll => &mut settings.trackpad_scroll,
            Self::MacIme => &mut settings.mac_ime,
        }
    }

    pub fn enabled(self, mut settings: InputSettings) -> bool {
        *self.value(&mut settings)
    }

    pub fn set(self, settings: &mut InputSettings, enabled: bool) {
        *self.value(settings) = enabled;
    }
}

#[derive(Clone, Copy)]
pub enum RemoteChord {
    SwitchApp,
    ControlAltDelete,
}

impl RemoteChord {
    pub fn keys(self) -> &'static [u32] {
        match self {
            Self::SwitchApp => &[0xffe9, 0xff09],
            Self::ControlAltDelete => &[0xffe3, 0xffe9, 0xffff],
        }
    }
}

pub fn commit_command(text: &str) -> Option<String> {
    (!text.is_empty() && !text.contains(char::is_control)).then(|| format!("co,end,{text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_commit_preserves_unicode_and_commas() {
        assert_eq!(
            commit_command("日本語, 한글"),
            Some("co,end,日本語, 한글".into())
        );
        assert_eq!(commit_command(""), None);
        assert_eq!(commit_command("\u{1b}"), None);
    }
}
