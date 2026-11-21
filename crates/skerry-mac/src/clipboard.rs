//! The Selkies clipboard protocol (`ARCH-017`): what crosses the WebSocket, with no AppKit.
//!
//! Outbound, content becomes `cw` / `cb` frames, or a `cws`/`cwd`/`cwe` (`cbs`/`cbd`/`cbe`)
//! multipart transfer once it reaches [`CHUNK`] bytes. Inbound, [`Receiver`] reassembles the
//! server's `clipboard*` frames, carrying the `clipboard_secret` and `clipboard_reply,cr` marks
//! that precede a payload.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::hash::{DefaultHasher, Hash, Hasher};

/// A copy with markup and the plain text written for it, as Selkies carries it.
pub const FLAVORS_MIME: &str = "application/x-selkies-clipboard-flavours";
/// Raw bytes per multipart chunk, the web core's `CLIPBOARD_CHUNK_SIZE`.
pub const CHUNK: usize = 16383;
/// The server's bound on one multipart transfer.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Content {
    Text(String),
    Rich { html: String, text: String },
    Image { mime: String, bytes: Vec<u8> },
}

impl Content {
    /// Outbound text means what the session means by it: LF line ends.
    pub fn text(text: &str) -> Self {
        Self::Text(text.replace("\r\n", "\n").replace('\r', "\n"))
    }

    /// The plain text this content carries.
    pub fn into_text(self) -> String {
        match self {
            Self::Text(text) | Self::Rich { text, .. } => text,
            Self::Image { .. } => String::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Text(text) => text.is_empty(),
            Self::Rich { html, text } => html.is_empty() && text.is_empty(),
            Self::Image { bytes, .. } => bytes.is_empty(),
        }
    }

    /// Whether only the binary clipboard may carry it (the flavour envelope may not need it).
    pub fn needs_binary(&self) -> bool {
        matches!(self, Self::Image { .. })
    }

    /// The one value both directions compare against, so unchanged content never re-crosses.
    pub fn signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }

    /// The wire mime (`None` for plain text) and bytes.
    fn wire(&self) -> (Option<&str>, Vec<u8>) {
        match self {
            Self::Text(text) => (None, text.as_bytes().to_vec()),
            Self::Rich { html, text } => (
                Some(FLAVORS_MIME),
                serde_json::to_vec(&serde_json::json!({"text/html": html, "text/plain": text}))
                    .unwrap_or_default(),
            ),
            Self::Image { mime, bytes } => (Some(mime), bytes.clone()),
        }
    }

    fn decode(mime: &str, bytes: Vec<u8>) -> Option<Self> {
        match mime {
            "text/plain" => Some(Self::Text(String::from_utf8_lossy(&bytes).into_owned())),
            FLAVORS_MIME => {
                let flavours: Value = serde_json::from_slice(&bytes).ok()?;
                let get = |mime: &str| flavours[mime].as_str().unwrap_or_default().to_owned();
                let (html, text) = (get("text/html"), get("text/plain"));
                if html.is_empty() {
                    (!text.is_empty()).then_some(Self::Text(text))
                } else {
                    Some(Self::Rich { html, text })
                }
            }
            "image/png" | "image/jpeg" => Some(Self::Image {
                mime: mime.to_owned(),
                bytes,
            }),
            mime if mime.starts_with("text/") => {
                Some(Self::Text(String::from_utf8_lossy(&bytes).into_owned()))
            }
            _ => None,
        }
    }
}

/// The frames that write `content` into the session's clipboard; `id` names a multipart
/// transfer. Empty when the content exceeds the server's bound.
pub fn encode(content: &Content, id: u64) -> Vec<String> {
    let (mime, bytes) = content.wire();
    if bytes.len() > MAX_BYTES {
        return Vec::new();
    }
    if bytes.len() < CHUNK {
        let data = BASE64.encode(&bytes);
        return vec![match mime {
            None => format!("cw,{data}"),
            Some(mime) => format!("cb,{mime},{data}"),
        }];
    }
    let (start, chunk, end) = match mime {
        None => (format!("cws,{id},{}", bytes.len()), "cwd", "cwe"),
        Some(mime) => (format!("cbs,{id},{mime},{}", bytes.len()), "cbd", "cbe"),
    };
    std::iter::once(start)
        .chain(
            bytes
                .chunks(CHUNK)
                .map(|part| format!("{chunk},{id},{}", BASE64.encode(part))),
        )
        .chain(std::iter::once(format!("{end},{id}")))
        .collect()
}

/// What the server allows, from `server_settings`; the server enforces it as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Mac → session.
    pub inbound: bool,
    /// Session → Mac.
    pub outbound: bool,
    pub binary: bool,
    pub seamless: bool,
    pub seamless_locked: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            inbound: false,
            outbound: false,
            binary: false,
            seamless: true,
            seamless_locked: false,
        }
    }
}

impl Policy {
    pub fn from_server(settings: &Value) -> Self {
        let on = |name: &str, default: bool| settings[name]["value"].as_bool().unwrap_or(default);
        Self {
            inbound: on("clipboard_in_enabled", false),
            outbound: on("clipboard_out_enabled", false),
            binary: on("enable_binary_clipboard", false),
            seamless: on("clipboard_seamless", true),
            seamless_locked: settings["clipboard_seamless"]["locked"].as_bool() == Some(true),
        }
    }
}

/// The person's per-server clipboard switches, within [`Policy`].
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct ClipboardSettings {
    pub send: bool,
    pub receive: bool,
    /// `None` follows the server's `clipboard_seamless`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seamless: Option<bool>,
}

impl Default for ClipboardSettings {
    fn default() -> Self {
        Self {
            send: true,
            receive: true,
            seamless: None,
        }
    }
}

impl ClipboardSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// What the Clipboard section of Session Controls asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardAction {
    SetSend(bool),
    SetReceive(bool),
    SetSeamless(bool),
    SendNow,
    CopyNow,
}

impl ClipboardSettings {
    /// Apply a switch; the deliberate buttons change no setting.
    pub fn apply(&mut self, action: ClipboardAction) {
        match action {
            ClipboardAction::SetSend(on) => self.send = on,
            ClipboardAction::SetReceive(on) => self.receive = on,
            ClipboardAction::SetSeamless(on) => self.seamless = Some(on),
            ClipboardAction::SendNow | ClipboardAction::CopyNow => {}
        }
    }
}

/// What policy and the person's switches together allow right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Effective {
    pub send: bool,
    pub receive: bool,
    pub binary: bool,
    pub seamless: bool,
}

impl Effective {
    pub fn new(policy: Policy, settings: ClipboardSettings) -> Self {
        Self {
            send: policy.inbound && settings.send,
            receive: policy.outbound && settings.receive,
            binary: policy.binary,
            seamless: if policy.seamless_locked {
                policy.seamless
            } else {
                settings.seamless.unwrap_or(policy.seamless)
            },
        }
    }

    /// Whether `content` may go to the session; images need the binary clipboard.
    pub fn sends(&self, content: &Content) -> bool {
        self.send && !content.is_empty() && (self.binary || !content.needs_binary())
    }
}

/// One payload from the session.
#[derive(Debug, PartialEq, Eq)]
pub struct Incoming {
    /// `None` for an empty payload: an empty secret retracts one, an empty reply means the
    /// session's clipboard is empty.
    pub content: Option<Content>,
    pub secret: bool,
    /// Answers our `cr` fetch rather than announcing a change.
    pub reply: bool,
}

struct Transfer {
    mime: String,
    declared: usize,
    bytes: Vec<u8>,
}

#[derive(Default)]
pub struct Receiver {
    secret: bool,
    reply: bool,
    transfer: Option<Transfer>,
}

/// The outcome of one server text frame.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    NotClipboard,
    Pending,
    Payload(Incoming),
}

impl Receiver {
    pub fn on_text(&mut self, text: &str) -> Frame {
        if text == "clipboard_secret" {
            self.secret = true;
            return Frame::Pending;
        }
        if let Some(verb) = text.strip_prefix("clipboard_reply,") {
            self.reply = verb == "cr";
            return Frame::Pending;
        }
        if let Some(data) = text.strip_prefix("clipboard,") {
            return self.finish("text/plain", BASE64.decode(data).ok());
        }
        if let Some(rest) = text.strip_prefix("clipboard_binary,") {
            let Some((mime, data)) = rest.split_once(',') else {
                return self.finish("", None);
            };
            return self.finish(mime, BASE64.decode(data).ok());
        }
        if let Some(rest) = text.strip_prefix("clipboard_start,") {
            // A new start drops whatever was still arriving: the clipboard is last-value-wins.
            self.transfer = rest.rsplit_once(',').and_then(|(mime, size)| {
                let declared = size
                    .parse::<usize>()
                    .ok()
                    .filter(|size| *size <= MAX_BYTES)?;
                Some(Transfer {
                    mime: mime.to_owned(),
                    declared,
                    bytes: Vec::with_capacity(declared.min(CHUNK * 64)),
                })
            });
            return Frame::Pending;
        }
        if let Some(data) = text.strip_prefix("clipboard_data,") {
            let fits = self.transfer.as_mut().and_then(|transfer| {
                let chunk = BASE64.decode(data).ok()?;
                (transfer.bytes.len() + chunk.len() <= transfer.declared).then(|| {
                    transfer.bytes.extend_from_slice(&chunk);
                })
            });
            if fits.is_none() {
                self.transfer = None;
            }
            return Frame::Pending;
        }
        if text == "clipboard_finish" {
            let Some(transfer) = self.transfer.take() else {
                return self.finish("", None);
            };
            let complete = transfer.bytes.len() == transfer.declared;
            return self.finish(&transfer.mime, complete.then_some(transfer.bytes));
        }
        Frame::NotClipboard
    }

    fn finish(&mut self, mime: &str, bytes: Option<Vec<u8>>) -> Frame {
        let secret = std::mem::take(&mut self.secret);
        let reply = std::mem::take(&mut self.reply);
        match bytes {
            // A broken payload is dropped whole; it never reaches the Mac.
            None => Frame::Pending,
            Some(bytes) if bytes.is_empty() => Frame::Payload(Incoming {
                content: None,
                secret,
                reply,
            }),
            Some(bytes) => match Content::decode(mime, bytes) {
                Some(content) => Frame::Payload(Incoming {
                    content: Some(content),
                    secret,
                    reply,
                }),
                None => Frame::Pending,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode_frames(frames: &[String]) -> (Option<String>, Vec<u8>) {
        let mut bytes = Vec::new();
        let mut mime = None;
        for frame in frames {
            let parts: Vec<&str> = frame.split(',').collect();
            match parts[0] {
                "cw" => bytes = BASE64.decode(parts[1]).unwrap(),
                "cb" => {
                    mime = Some(parts[1].to_owned());
                    bytes = BASE64.decode(parts[2]).unwrap();
                }
                "cbs" => mime = Some(parts[2].to_owned()),
                "cwd" | "cbd" => bytes.extend(BASE64.decode(parts[2]).unwrap()),
                _ => {}
            }
        }
        (mime, bytes)
    }

    #[test]
    fn small_text_and_unicode_go_as_one_cw_frame() {
        let frames = encode(&Content::text("日本語, 한글\r\nline"), 1);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].starts_with("cw,"));
        assert_eq!(
            decode_frames(&frames),
            (None, "日本語, 한글\nline".as_bytes().to_vec())
        );
    }

    #[test]
    fn large_payloads_go_multipart_with_declared_sizes() {
        let text = "x".repeat(CHUNK * 2 + 5);
        let frames = encode(&Content::Text(text.clone()), 7);
        assert_eq!(frames[0], format!("cws,7,{}", text.len()));
        assert_eq!(frames.len(), 5);
        assert_eq!(frames[4], "cwe,7");
        assert_eq!(decode_frames(&frames).1, text.into_bytes());

        let image = Content::Image {
            mime: "image/png".into(),
            bytes: vec![9; CHUNK],
        };
        let frames = encode(&image, 8);
        assert_eq!(frames[0], format!("cbs,8,image/png,{CHUNK}"));
        assert_eq!(frames.last().unwrap(), "cbe,8");
    }

    #[test]
    fn rich_text_rides_the_flavour_envelope() {
        let frames = encode(
            &Content::Rich {
                html: "<b>hi</b>".into(),
                text: "hi".into(),
            },
            1,
        );
        let (mime, bytes) = decode_frames(&frames);
        assert_eq!(mime.as_deref(), Some(FLAVORS_MIME));
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value, json!({"text/html": "<b>hi</b>", "text/plain": "hi"}));
    }

    #[test]
    fn receiver_reassembles_marks_and_drops_broken_payloads() {
        let mut receiver = Receiver::default();
        assert_eq!(receiver.on_text("cursor,{}"), Frame::NotClipboard);
        assert_eq!(
            receiver.on_text(&format!("clipboard,{}", BASE64.encode("hello"))),
            Frame::Payload(Incoming {
                content: Some(Content::Text("hello".into())),
                secret: false,
                reply: false,
            })
        );

        assert_eq!(receiver.on_text("clipboard_secret"), Frame::Pending);
        assert_eq!(
            receiver.on_text("clipboard,"),
            Frame::Payload(Incoming {
                content: None,
                secret: true,
                reply: false,
            })
        );

        let png = vec![0x89; CHUNK + 10];
        assert_eq!(
            receiver.on_text(&format!("clipboard_start,image/png,{}", png.len())),
            Frame::Pending
        );
        for part in png.chunks(CHUNK) {
            receiver.on_text(&format!("clipboard_data,{}", BASE64.encode(part)));
        }
        assert_eq!(
            receiver.on_text("clipboard_finish"),
            Frame::Payload(Incoming {
                content: Some(Content::Image {
                    mime: "image/png".into(),
                    bytes: png.clone(),
                }),
                secret: false,
                reply: false,
            })
        );

        // Truncated, then superseded by a newer start: the first never lands.
        receiver.on_text("clipboard_start,text/plain,100");
        receiver.on_text(&format!("clipboard_data,{}", BASE64.encode("partial")));
        assert_eq!(receiver.on_text("clipboard_finish"), Frame::Pending);
        receiver.on_text("clipboard_start,text/plain,3");
        receiver.on_text("clipboard_start,text/plain,2");
        receiver.on_text(&format!("clipboard_data,{}", BASE64.encode("ok")));
        assert!(matches!(
            receiver.on_text("clipboard_finish"),
            Frame::Payload(Incoming { content: Some(Content::Text(text)), .. }) if text == "ok"
        ));

        receiver.on_text("clipboard_reply,cr");
        let envelope = BASE64.encode(br#"{"text/html":"<i>a</i>","text/plain":"a"}"#);
        assert_eq!(
            receiver.on_text(&format!("clipboard_binary,{FLAVORS_MIME},{envelope}")),
            Frame::Payload(Incoming {
                content: Some(Content::Rich {
                    html: "<i>a</i>".into(),
                    text: "a".into(),
                }),
                secret: false,
                reply: true,
            })
        );
    }

    #[test]
    fn policy_bounds_the_persons_switches() {
        let policy = Policy::from_server(&json!({
            "clipboard_in_enabled": {"value": true},
            "clipboard_out_enabled": {"value": false},
            "enable_binary_clipboard": {"value": false, "locked": false},
            "clipboard_seamless": {"value": false, "locked": true},
        }));
        let settings = ClipboardSettings {
            send: true,
            receive: true,
            seamless: Some(true),
        };
        let effective = Effective::new(policy, settings);
        assert!(effective.send && !effective.receive && !effective.seamless);
        assert!(effective.sends(&Content::Text("a".into())));
        assert!(!effective.sends(&Content::Image {
            mime: "image/png".into(),
            bytes: vec![1],
        }));
        assert!(!effective.sends(&Content::Text(String::new())));

        let unlocked = Policy {
            seamless_locked: false,
            ..policy
        };
        assert!(Effective::new(unlocked, settings).seamless);
        assert!(!Effective::new(unlocked, ClipboardSettings::default()).seamless);
    }
}
