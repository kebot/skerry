/// Errors from the CDP client.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CdpError {
    /// The browser answered a command with a protocol error
    /// (`{"error": {"code", "message"}}`). The connection is still healthy.
    #[error("CDP error {code}: {message}")]
    Protocol {
        code: i64,
        message: String,
        data: Option<String>,
    },
    /// The connection is closed (by either side) or was dropped before the
    /// command could be answered. Pending commands fail with this.
    #[error("connection closed")]
    Closed,
    /// WebSocket or I/O failure (connect failure, write error, bad URL).
    #[error("transport error: {0}")]
    Transport(String),
    /// A response could not be decoded into the expected type.
    #[error("invalid CDP payload: {0}")]
    Decode(String),
    /// A screencast is already running on this Session.
    #[error("a screencast is already active on this session")]
    ScreencastActive,
}

impl CdpError {
    /// True if this is a protocol error (the browser said no) as opposed to
    /// a dead connection.
    pub fn is_protocol(&self) -> bool {
        matches!(self, CdpError::Protocol { .. })
    }
}
