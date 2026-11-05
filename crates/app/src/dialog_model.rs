//! Pure model of the JS dialog modal: which buttons a dialog kind has, the
//! title, the answer a key gives. No GPUI, no I/O.

use rb_cdp::{Dialog, DialogKind, DialogResponse};

use crate::sidebar_model::{EditOutcome, LineEdit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialogButton {
    pub label: &'static str,
    /// OK / Leave (`true`) or Cancel / Stay (`false`).
    pub accept: bool,
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogView {
    pub title: String,
    pub message: String,
    pub buttons: Vec<DialogButton>,
    /// The kind has a text field (`prompt`); holds its initial text.
    pub prompt: Option<String>,
}

/// The host of `url` (without a port path), or `None` for `about:blank`,
/// `data:` and the like.
pub fn origin_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

const OK: DialogButton = DialogButton {
    label: "OK",
    accept: true,
    primary: true,
};
const CANCEL: DialogButton = DialogButton {
    label: "Cancel",
    accept: false,
    primary: false,
};

pub fn view_for(d: &Dialog) -> DialogView {
    let origin = origin_of(&d.url);
    let says = match &origin {
        Some(o) => format!("{o} says"),
        None => "This page says".to_string(),
    };
    match d.kind {
        DialogKind::Alert => DialogView {
            title: says,
            message: d.message.clone(),
            buttons: vec![OK],
            prompt: None,
        },
        DialogKind::Confirm => DialogView {
            title: says,
            message: d.message.clone(),
            buttons: vec![CANCEL, OK],
            prompt: None,
        },
        DialogKind::Prompt => DialogView {
            title: says,
            message: d.message.clone(),
            buttons: vec![CANCEL, OK],
            prompt: Some(d.default_prompt.clone().unwrap_or_default()),
        },
        DialogKind::BeforeUnload => DialogView {
            title: match &origin {
                Some(o) => format!("Leave {o}?"),
                None => "Leave this page?".to_string(),
            },
            message: if d.message.is_empty() {
                "Changes you made may not be saved.".to_string()
            } else {
                d.message.clone()
            },
            buttons: vec![
                DialogButton {
                    label: "Stay",
                    accept: false,
                    primary: false,
                },
                DialogButton {
                    label: "Leave",
                    accept: true,
                    primary: true,
                },
            ],
            prompt: None,
        },
    }
}

/// The CDP answer for a button press. Only a `prompt` sends text.
pub fn response(kind: DialogKind, accept: bool, text: &str) -> DialogResponse {
    DialogResponse {
        accept,
        prompt_text: (kind == DialogKind::Prompt && accept).then(|| text.to_string()),
    }
}

/// What a key press does in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKey {
    /// Answer now (Enter: accept, Escape: cancel).
    Answer(bool),
    /// The prompt text changed.
    Edited,
    Ignored,
}

/// Escape = Cancel / Stay, Enter = OK / Leave; other keys edit the prompt
/// field when there is one.
pub fn key(
    kind: DialogKind,
    edit: &mut LineEdit,
    key: &str,
    key_char: Option<&str>,
    ctrl: bool,
) -> DialogKey {
    if kind != DialogKind::Prompt {
        return match key {
            "enter" => DialogKey::Answer(true),
            "escape" => DialogKey::Answer(false),
            _ => DialogKey::Ignored,
        };
    }
    match edit.key(key, key_char, ctrl) {
        EditOutcome::Commit => DialogKey::Answer(true),
        EditOutcome::Cancel => DialogKey::Answer(false),
        EditOutcome::Changed => DialogKey::Edited,
        EditOutcome::Ignored => DialogKey::Ignored,
    }
}

/// The pending dialog of a Tab.
pub fn for_target<'a>(dialogs: &'a [Dialog], target_id: &str) -> Option<&'a Dialog> {
    dialogs.iter().find(|d| d.target_id == target_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dlg(kind: DialogKind, msg: &str, url: &str, def: Option<&str>) -> Dialog {
        Dialog {
            target_id: "T".into(),
            session_id: "S".into(),
            kind,
            message: msg.into(),
            url: url.into(),
            default_prompt: def.map(str::to_string),
        }
    }

    #[test]
    fn buttons_per_kind() {
        let labels =
            |d: &Dialog| -> Vec<&str> { view_for(d).buttons.iter().map(|b| b.label).collect() };
        let u = "https://shop.example.com:8443/cart?x=1";
        assert_eq!(labels(&dlg(DialogKind::Alert, "m", u, None)), ["OK"]);
        assert_eq!(
            labels(&dlg(DialogKind::Confirm, "m", u, None)),
            ["Cancel", "OK"]
        );
        assert_eq!(
            labels(&dlg(DialogKind::Prompt, "m", u, None)),
            ["Cancel", "OK"]
        );
        assert_eq!(
            labels(&dlg(DialogKind::BeforeUnload, "", u, None)),
            ["Stay", "Leave"]
        );
    }

    #[test]
    fn titles_show_the_origin() {
        let v = view_for(&dlg(DialogKind::Alert, "hi", "https://a.test:81/x", None));
        assert_eq!(v.title, "a.test:81 says");
        assert_eq!(
            view_for(&dlg(DialogKind::Alert, "hi", "about:blank", None)).title,
            "This page says"
        );
        let v = view_for(&dlg(DialogKind::BeforeUnload, "", "https://a.test/", None));
        assert_eq!(v.title, "Leave a.test?");
        assert!(v.message.contains("may not be saved"));
    }

    #[test]
    fn prompt_is_prefilled_and_only_prompts_send_text() {
        let v = view_for(&dlg(DialogKind::Prompt, "name?", "https://a/", Some("Bob")));
        assert_eq!(v.prompt.as_deref(), Some("Bob"));
        assert_eq!(
            view_for(&dlg(DialogKind::Confirm, "m", "https://a/", None)).prompt,
            None
        );
        assert_eq!(
            response(DialogKind::Prompt, true, "Al")
                .prompt_text
                .as_deref(),
            Some("Al")
        );
        assert_eq!(response(DialogKind::Prompt, false, "Al").prompt_text, None);
        assert_eq!(response(DialogKind::Confirm, true, "x").prompt_text, None);
        assert!(!response(DialogKind::Confirm, false, "").accept);
    }

    #[test]
    fn escape_cancels_enter_accepts_and_prompt_edits() {
        let mut e = LineEdit::default();
        assert_eq!(
            key(DialogKind::Alert, &mut e, "enter", None, false),
            DialogKey::Answer(true)
        );
        assert_eq!(
            key(DialogKind::Confirm, &mut e, "escape", None, false),
            DialogKey::Answer(false)
        );
        assert_eq!(
            key(DialogKind::Alert, &mut e, "a", Some("a"), false),
            DialogKey::Ignored
        );
        let mut e = LineEdit::with_text("Bob");
        assert_eq!(
            key(DialogKind::Prompt, &mut e, "x", Some("x"), false),
            DialogKey::Edited
        );
        assert_eq!(e.buf, "x");
        assert_eq!(
            key(DialogKind::Prompt, &mut e, "enter", None, false),
            DialogKey::Answer(true)
        );
        assert_eq!(
            key(DialogKind::Prompt, &mut e, "escape", None, false),
            DialogKey::Answer(false)
        );
    }

    #[test]
    fn finds_the_dialog_of_a_tab() {
        let d = [dlg(DialogKind::Alert, "m", "https://a/", None)];
        assert!(for_target(&d, "T").is_some() && for_target(&d, "U").is_none());
    }
}
