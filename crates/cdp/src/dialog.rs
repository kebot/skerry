//! JS dialogs (`alert`, `confirm`, `prompt`, `beforeunload`) and the
//! permission policy.
//!
//! A JS dialog blocks the remote page and is **not** drawn into the
//! screencast, so rb shows a local modal and answers with
//! `Page.handleJavaScriptDialog`. [`Connection`](crate::Connection) observes
//! dialogs of every page target (see its docs for the mechanism); this module
//! holds the types and the decoding of the two CDP events.

use serde_json::Value;

use crate::Event;

/// The kind of a JS dialog (`Page.DialogType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    Alert,
    Confirm,
    Prompt,
    BeforeUnload,
}

impl DialogKind {
    fn parse(s: &str) -> Option<DialogKind> {
        match s {
            "alert" => Some(DialogKind::Alert),
            "confirm" => Some(DialogKind::Confirm),
            "prompt" => Some(DialogKind::Prompt),
            "beforeunload" => Some(DialogKind::BeforeUnload),
            _ => None,
        }
    }
}

/// An open JS dialog blocking a Tab's page.
#[derive(Debug, Clone, PartialEq)]
pub struct Dialog {
    pub target_id: String,
    /// The flat session the dialog was observed on (the one
    /// [`Connection::handle_dialog`](crate::Connection::handle_dialog) uses).
    pub session_id: String,
    pub kind: DialogKind,
    pub message: String,
    /// URL of the frame that opened the dialog.
    pub url: String,
    /// Initial text of a `prompt` dialog.
    pub default_prompt: Option<String>,
}

/// The user's answer to a [`Dialog`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogResponse {
    /// OK / Leave (`true`) or Cancel / Stay (`false`).
    pub accept: bool,
    /// Text entered into a `prompt`; ignored by other kinds.
    pub prompt_text: Option<String>,
}

/// Typed `Page.javascriptDialog*` events (session events).
#[derive(Debug, Clone, PartialEq)]
pub enum DialogEvent {
    /// `Page.javascriptDialogOpening`. The target and session ids are filled
    /// by the caller (the raw event only carries the sessionId).
    Opening {
        session_id: String,
        kind: DialogKind,
        message: String,
        url: String,
        default_prompt: Option<String>,
    },
    /// `Page.javascriptDialogClosed`.
    Closed {
        session_id: String,
        /// Whether the dialog was accepted.
        result: bool,
        /// Text entered into a prompt.
        user_input: String,
    },
}

impl DialogEvent {
    /// Decode a raw [`Event`]; `None` if it is not a (well-formed) dialog
    /// event or has no session.
    pub fn from_event(ev: &Event) -> Option<DialogEvent> {
        let session_id = ev.session_id.clone()?;
        let p = &ev.params;
        let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
        match ev.method.as_str() {
            "Page.javascriptDialogOpening" => Some(DialogEvent::Opening {
                session_id,
                kind: DialogKind::parse(p.get("type")?.as_str()?)?,
                message: s("message").unwrap_or_default(),
                url: s("url").unwrap_or_default(),
                default_prompt: s("defaultPrompt").filter(|d| !d.is_empty()),
            }),
            "Page.javascriptDialogClosed" => Some(DialogEvent::Closed {
                session_id,
                result: p.get("result").and_then(Value::as_bool).unwrap_or(false),
                user_input: s("userInput").unwrap_or_default(),
            }),
            _ => None,
        }
    }
}

/// What to do about permission prompts (geolocation, notifications, camera,
/// ...). Chromium's permission bubble is native browser UI on the Remote
/// Browser and is not part of the screencast, so nobody could answer it.
///
/// The policy is applied with `Browser.setPermission` for each origin (per
/// browser context) the first time a page target shows it, and again after a
/// reconnect. It is best effort: a page that asks in the first milliseconds
/// after its first navigation commits can still beat the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionPolicy {
    /// Set the common prompting permissions to `denied` (default).
    #[default]
    Deny,
    /// Reserved for a later UI that lets the user decide. Currently sends
    /// nothing: Chromium's own (invisible) prompt stays in charge.
    Ask,
    /// Set the common prompting permissions to `granted`.
    Allow,
}

/// Permission names (Permissions API / `Browser.setPermission` descriptor
/// names) covered by [`PermissionPolicy`].
pub const POLICY_PERMISSIONS: &[&str] = &[
    "geolocation",
    "notifications",
    "camera",
    "microphone",
    "clipboard-read",
    "midi",
    "display-capture",
    "idle-detection",
    "window-management",
];

impl PermissionPolicy {
    /// The `PermissionSetting` to apply, `None` if the policy sends nothing.
    pub(crate) fn setting(self) -> Option<&'static str> {
        match self {
            PermissionPolicy::Deny => Some("denied"),
            PermissionPolicy::Allow => Some("granted"),
            PermissionPolicy::Ask => None,
        }
    }
}

/// `scheme://host[:port]` of an http(s) URL; `None` for anything else
/// (about:blank, chrome://, data:, ...), where permissions do not apply.
pub(crate) fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    (!host.is_empty()).then(|| format!("{scheme}://{host}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn origin_extraction() {
        assert_eq!(
            origin_of("https://a.b:8080/x?y#z").as_deref(),
            Some("https://a.b:8080")
        );
        assert_eq!(origin_of("http://u:p@h/").as_deref(), Some("http://h"));
        assert_eq!(origin_of("https://h").as_deref(), Some("https://h"));
        assert_eq!(origin_of("about:blank"), None);
        assert_eq!(origin_of("chrome://settings"), None);
    }

    #[test]
    fn decodes_events() {
        let ev = Event {
            method: "Page.javascriptDialogOpening".into(),
            params: json!({"url": "u", "message": "m", "type": "prompt",
                "hasBrowserHandler": false, "defaultPrompt": "d"}),
            session_id: Some("S".into()),
        };
        assert_eq!(
            DialogEvent::from_event(&ev),
            Some(DialogEvent::Opening {
                session_id: "S".into(),
                kind: DialogKind::Prompt,
                message: "m".into(),
                url: "u".into(),
                default_prompt: Some("d".into())
            })
        );
        let ev = Event {
            method: "Page.javascriptDialogClosed".into(),
            params: json!({"result": true, "userInput": "x"}),
            session_id: Some("S".into()),
        };
        assert_eq!(
            DialogEvent::from_event(&ev),
            Some(DialogEvent::Closed {
                session_id: "S".into(),
                result: true,
                user_input: "x".into()
            })
        );
    }
}
