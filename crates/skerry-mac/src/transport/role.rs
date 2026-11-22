//! Which display a stream shows (`ARCH-039`): the session window's `primary`, or Display 2
//! (`display2`) beside it. One transport serves both; this module holds what differs.
//!
//! Selkies re-seats a socket on **every** `SETTINGS,` from that payload's `displayId`, and a
//! missing id means `primary`. One unstamped frame from Display 2 would therefore claim the
//! primary and KILL the person's main stream. [`Wire`] is the only way the transport writes
//! to its socket, and its only text send stamps every `SETTINGS,` frame, so a raw send that
//! skips the stamp does not compile.

use super::Resolution;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// Where Display 2 sits relative to the primary, in the server's layout. Index order is
/// `[Left, Right, Up, Down]`, the order of the side picker's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Left, Side::Right, Side::Up, Side::Down];

    /// The `displayPosition` Selkies reads.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
        }
    }

    /// The picker's and the controls note's word for it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Up => "Above",
            Self::Down => "Below",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
            Self::Up => 2,
            Self::Down => 3,
        }
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

/// The display a stream claims.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamRole {
    #[default]
    Primary,
    /// Display 2, on this side of the primary.
    Secondary(Side),
}

impl StreamRole {
    /// The `displayId` this stream claims.
    pub fn id(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary(_) => "display2",
        }
    }

    pub fn is_primary(self) -> bool {
        self == Self::Primary
    }

    /// Writes this stream's claim into a `SETTINGS` payload, **overwriting** any
    /// `displayId` or `displayPosition` already there.
    pub fn claim(self, settings: &mut Value) {
        settings["displayId"] = json!(self.id());
        if let Self::Secondary(side) = self {
            settings["displayPosition"] = json!(side.wire());
        }
    }

    /// A text frame as it goes on the wire. A secondary's `SETTINGS,{…}` gets its claim
    /// (any id in it is overwritten, never trusted); every other frame, and anything that
    /// isn't a JSON object, is left alone. The primary's frames go out byte for byte: the
    /// server reads a missing id as `primary` already.
    pub fn stamp(self, text: String) -> String {
        if self.is_primary() {
            return text;
        }
        let Some(json) = text.strip_prefix("SETTINGS,") else {
            return text;
        };
        let Ok(mut payload) = serde_json::from_str::<Value>(json) else {
            return text;
        };
        if !payload.is_object() {
            return text;
        }
        self.claim(&mut payload);
        format!("SETTINGS,{payload}")
    }

    /// `r,WxH,<id>`: the web client's resize, naming this stream's display.
    pub fn resize_command(self, Resolution(width, height): Resolution) -> String {
        format!("r,{width}x{height},{}", self.id())
    }

    /// Whether a server message is about this stream's display. A message with no
    /// `displayId` counts only when `unnamed` allows it: the on-connect `server_settings`
    /// carries none, while live per-display messages always name theirs.
    pub fn owns(self, data: &Value, unnamed: bool) -> bool {
        match data["displayId"].as_str() {
            Some(id) => id == self.id(),
            None => unnamed || self.is_primary(),
        }
    }
}

/// The stream's socket. Its only text send is [`Wire::send`], which stamps, and its only
/// binary send is the RTT ping: there is no way to write an unstamped frame.
pub(super) struct Wire {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    role: StreamRole,
}

impl Wire {
    pub(super) fn new(
        socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
        role: StreamRole,
    ) -> Self {
        Self { socket, role }
    }

    pub(super) fn role(&self) -> StreamRole {
        self.role
    }

    /// Display 2 moved: every later frame carries the new `displayPosition`. A primary
    /// stays the primary.
    pub(super) fn set_side(&mut self, side: Side) {
        if let StreamRole::Secondary(_) = self.role {
            self.role = StreamRole::Secondary(side);
        }
    }

    /// Sends one text frame, stamped with this stream's claim.
    pub(super) async fn send(&mut self, text: String) -> Result<(), tungstenite::Error> {
        self.socket.send(Message::text(self.role.stamp(text))).await
    }

    /// Sends one binary frame (the microphone's `0x02` packets), on the primary only:
    /// Display 2 carries no audio either way, so a secondary drops it unsent.
    #[allow(dead_code)]
    pub(super) async fn send_binary(&mut self, data: Vec<u8>) -> Result<(), tungstenite::Error> {
        if !self.role.is_primary() {
            return Ok(());
        }
        self.socket.send(Message::binary(data)).await
    }

    pub(super) async fn ping(&mut self, payload: &'static [u8]) -> Result<(), tungstenite::Error> {
        self.socket.send(Message::Ping(payload.into())).await
    }

    pub(super) async fn next(&mut self) -> Option<Result<Message, tungstenite::Error>> {
        self.socket.next().await
    }

    pub(super) async fn flush(&mut self) -> Result<(), tungstenite::Error> {
        self.socket.flush().await
    }

    pub(super) async fn close(
        &mut self,
        frame: Option<tungstenite::protocol::CloseFrame>,
    ) -> Result<(), tungstenite::Error> {
        self.socket.close(frame).await
    }
}

/// Why the server ended a stream with `KILL <reason>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kill {
    /// Another client claimed this display ("a new <id> client connected connection
    /// killed").
    Superseded,
    /// The server refused the display; its own words, trimmed.
    Refused(String),
}

impl std::fmt::Display for Kill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Superseded => f.write_str("This session was opened somewhere else."),
            Self::Refused(reason) => f.write_str(reason),
        }
    }
}

/// The longest server reason Skerry shows.
const KILL_REASON_MAX: usize = 200;

/// A refused secondary whose `server_settings.second_screen` is off.
pub(super) const SECOND_SCREEN_OFF: &str = "This session doesn’t allow a second display.";

/// A `KILL <reason>` text frame as its [`Kill`]; `None` for anything else.
pub(super) fn kill_reason(text: &str) -> Option<Kill> {
    let reason = text.strip_prefix("KILL")?;
    if !(reason.is_empty() || reason.starts_with(' ')) {
        return None;
    }
    let reason = reason.trim();
    if reason.contains("client connected connection killed") {
        return Some(Kill::Superseded);
    }
    Some(Kill::Refused(
        reason.chars().take(KILL_REASON_MAX).collect(),
    ))
}

/// One display's rectangle in the server's combined layout, in remote pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayoutRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A `DISPLAY_CONFIG_UPDATE`: the displays the server has, where each sits, and whether
/// it scrolls with fingers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayConfig {
    pub displays: Vec<String>,
    pub layouts: BTreeMap<String, LayoutRect>,
    pub finger_scroll: bool,
}

/// Parses `DISPLAY_CONFIG_UPDATE,{…}` (or its bare JSON); `None` for anything else.
pub(super) fn display_config(text: &str) -> Option<DisplayConfig> {
    let payload = text.strip_prefix("DISPLAY_CONFIG_UPDATE,").unwrap_or(text);
    let data: Value = serde_json::from_str(payload).ok()?;
    if data["type"] != "display_config_update" {
        return None;
    }
    let int = |value: &Value| {
        value
            .as_i64()
            .or_else(|| value.as_f64().map(|v| v as i64))
            .and_then(|v| i32::try_from(v).ok())
    };
    let layouts = data["layouts"]
        .as_object()
        .map(|layouts| {
            layouts
                .iter()
                .filter_map(|(id, rect)| {
                    Some((
                        id.clone(),
                        LayoutRect {
                            x: int(&rect["x"])?,
                            y: int(&rect["y"])?,
                            w: int(&rect["w"])?,
                            h: int(&rect["h"])?,
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(DisplayConfig {
        displays: data["displays"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        layouts,
        finger_scroll: data["finger_scroll"].as_bool().unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(frame: &str) -> Value {
        serde_json::from_str(frame.strip_prefix("SETTINGS,").unwrap()).unwrap()
    }

    #[test]
    fn stamp_leaves_non_settings_frames_alone() {
        let role = StreamRole::Secondary(Side::Right);
        for frame in [
            "kd,97",
            "r,1920x1080,display2",
            "_stats,1",
            "START_AUDIO",
            "cr",
        ] {
            assert_eq!(role.stamp(frame.into()), frame);
        }
    }

    #[test]
    fn stamp_keeps_a_frame_that_is_not_json() {
        let role = StreamRole::Secondary(Side::Left);
        assert_eq!(role.stamp("SETTINGS,nope".into()), "SETTINGS,nope");
        assert_eq!(role.stamp("SETTINGS,[1]".into()), "SETTINGS,[1]");
    }

    #[test]
    fn stamp_overwrites_any_display_id_on_a_secondary() {
        let role = StreamRole::Secondary(Side::Up);
        let stamped = role.stamp(r#"SETTINGS,{"displayId":"primary","framerate":30}"#.into());
        let value = payload(&stamped);
        assert_eq!(value["displayId"], "display2");
        assert_eq!(value["displayPosition"], "up");
        assert_eq!(value["framerate"], 30);
    }

    #[test]
    fn the_primary_frames_go_out_unchanged() {
        let frame = r#"SETTINGS,{"framerate":30}"#;
        assert_eq!(StreamRole::Primary.stamp(frame.into()), frame);
    }

    #[test]
    fn resize_command_names_the_streams_display() {
        let size = Resolution(1920, 1080);
        assert_eq!(
            StreamRole::Primary.resize_command(size),
            "r,1920x1080,primary"
        );
        assert_eq!(
            StreamRole::Secondary(Side::Down).resize_command(size),
            "r,1920x1080,display2"
        );
    }

    #[test]
    fn messages_for_another_display_are_ignored() {
        let second = StreamRole::Secondary(Side::Right);
        let for_primary = json!({"type": "stream_resolution", "displayId": "primary"});
        let for_second = json!({"type": "stream_resolution", "displayId": "display2"});
        assert!(!second.owns(&for_primary, false));
        assert!(second.owns(&for_second, false));
        assert!(StreamRole::Primary.owns(&for_primary, false));
        assert!(!StreamRole::Primary.owns(&for_second, false));
    }

    #[test]
    fn unnamed_server_settings_count_for_either_role() {
        let unnamed = json!({"type": "server_settings"});
        let second = StreamRole::Secondary(Side::Right);
        assert!(second.owns(&unnamed, true));
        assert!(StreamRole::Primary.owns(&unnamed, true));
        // An unnamed stream_resolution is the primary's, never Display 2's.
        assert!(!second.owns(&unnamed, false));
        assert!(StreamRole::Primary.owns(&unnamed, false));
    }

    #[test]
    fn kill_superseded_is_recognised() {
        assert_eq!(
            kill_reason("KILL a new display2 client connected connection killed"),
            Some(Kill::Superseded)
        );
        assert_eq!(
            Kill::Superseded.to_string(),
            "This session was opened somewhere else."
        );
    }

    #[test]
    fn kill_refusal_keeps_the_servers_reason() {
        assert_eq!(
            kill_reason("KILL Second screens are disabled on this server."),
            Some(Kill::Refused(
                "Second screens are disabled on this server.".into()
            ))
        );
        assert_eq!(kill_reason("KILLER"), None);
        assert_eq!(kill_reason("kd,97"), None);
    }

    #[test]
    fn kill_reason_is_trimmed() {
        let long = format!("KILL {}", "x".repeat(500));
        match kill_reason(&long) {
            Some(Kill::Refused(reason)) => assert_eq!(reason.chars().count(), 200),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn display_config_parses_displays_and_layouts() {
        let config = display_config(
            r#"DISPLAY_CONFIG_UPDATE,{"type":"display_config_update","displays":["primary","display2"],"wayland":false,"finger_scroll":false,"layouts":{"primary":{"x":0,"y":0,"w":1920,"h":1080},"display2":{"x":1920,"y":0,"w":1280,"h":720,"scale":1.0}}}"#,
        )
        .unwrap();
        assert_eq!(config.displays, ["primary", "display2"]);
        assert_eq!(
            config.layouts["display2"],
            LayoutRect {
                x: 1920,
                y: 0,
                w: 1280,
                h: 720
            }
        );
    }

    #[test]
    fn display_config_still_reports_finger_scroll() {
        let config =
            display_config(r#"{"type":"display_config_update","finger_scroll":true}"#).unwrap();
        assert!(config.finger_scroll);
        assert_eq!(display_config(r#"{"type":"stream_info"}"#), None);
    }

    #[test]
    fn display_config_without_layouts_is_empty() {
        let config = display_config(
            r#"DISPLAY_CONFIG_UPDATE,{"type":"display_config_update","displays":["primary"]}"#,
        )
        .unwrap();
        assert!(config.layouts.is_empty());
        assert!(!config.finger_scroll);
    }
}
