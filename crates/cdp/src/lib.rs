//! rb-cdp: Chrome DevTools Protocol client for rb (no GPUI dependency).
//!
//! A [`Client`] is one WebSocket to a Remote Browser (a Connection's
//! browser-level CDP endpoint). Commands are matched to responses by id,
//! events are broadcast, and per-Tab work happens through flat [`Session`]s
//! obtained with [`Client::attach_to_target`]. [`TargetTracker`] keeps the
//! list of page targets (the raw material for Tabs) current.
//!
//! The [`testing`] module contains a fake CDP server for tests.
//!
//! ```no_run
//! # async fn demo() -> Result<(), rb_cdp::CdpError> {
//! use rb_cdp::{Client, Token};
//! let client = Client::connect("wss://host/api/profiles/1/cdp", Some(Token::new("secret"))).await?;
//! let tracker = rb_cdp::TargetTracker::start(&client).await?;
//! if let Some(page) = tracker.pages().first() {
//!     let session = client.attach_to_target(&page.target_id).await?;
//!     session.send("Page.enable", None).await?;
//! }
//! # Ok(()) }
//! ```

mod client;
mod connection;
mod error;
mod input;
mod screencast;
mod target;
pub mod testing;
mod viewport;

pub use client::{Client, Event, Events, Session, Token};
pub use connection::{
    ActiveTab, BackoffConfig, Connection, ConnectionConfig, ConnectionEvent, ConnectionState,
    FailureReason,
};
pub use error::CdpError;
pub use input::{KeyEvent, KeyEventKind, Modifiers, MouseButton, MouseEvent, MouseEventKind};
pub use screencast::{Frame, FrameMetadata, Screencast, ScreencastOptions};
pub use target::{TargetEvent, TargetInfo, TargetList, TargetTracker};
pub use viewport::{FrameFit, Point, Rect};
