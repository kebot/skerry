//! The Mirror's handle to a Tab: a Frame receiver plus an input sender.
//!
//! The Mirror never sees a concrete connection. T8 builds a
//! [`MirrorBackend`] from a real `Session` with [`MirrorBackend::from_session`];
//! tests and the demo use the same constructor with a Session on the fake CDP
//! server. All CDP I/O runs on a tokio runtime owned by the app, not on GPUI's
//! executors (see docs/gpui-notes.md).

use rb_cdp::{CdpError, Connection, Frame, KeyEvent, MouseEvent, ScreencastOptions, Session};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

/// Input captured over the Mirror, in CDP terms (remote CSS pixels).
#[derive(Debug, Clone, PartialEq)]
pub enum MirrorInput {
    Mouse(MouseEvent),
    Key(KeyEvent),
    /// IME / composed text (`Input.insertText`).
    InsertText(String),
}

/// What the Mirror needs from a Tab.
pub struct MirrorBackend {
    /// Frames of the Active Tab. Keep it small (capacity 1): the browser
    /// only sends the next Frame after [`Frame::ack`].
    pub frames: mpsc::Receiver<Frame>,
    /// Input events, dispatched in order.
    pub input: mpsc::UnboundedSender<MirrorInput>,
    /// The tokio runtime that `Frame::ack` runs on.
    pub runtime: Handle,
}

impl MirrorBackend {
    /// Start a screencast on `session` and pump input into it. Must be
    /// called inside the tokio runtime. Dropping the Mirror (the receiver)
    /// stops the screencast.
    pub async fn from_session(
        session: Session,
        opts: ScreencastOptions,
    ) -> Result<MirrorBackend, CdpError> {
        let mut screencast = session.start_screencast(opts).await?;
        let (frame_tx, frames) = mpsc::channel(1);
        let (input, mut input_rx) = mpsc::unbounded_channel::<MirrorInput>();

        tokio::spawn(async move {
            while let Some(frame) = screencast.next().await {
                if frame_tx.send(frame).await.is_err() {
                    break;
                }
            }
            let _ = screencast.stop().await;
        });
        tokio::spawn(async move {
            while let Some(msg) = input_rx.recv().await {
                // Input is best effort; a failed event must not stop the pump.
                let _ = match msg {
                    MirrorInput::Mouse(ev) => session.dispatch_mouse(ev).await,
                    MirrorInput::Key(ev) => session.dispatch_key(ev).await,
                    MirrorInput::InsertText(t) => session.insert_text(&t).await,
                };
            }
        });
        Ok(MirrorBackend {
            frames,
            input,
            runtime: Handle::current(),
        })
    }

    /// Plug a supervised [`Connection`] into the Mirror.
    ///
    /// Frames are the Connection's single Frame stream (it continues across
    /// reconnects and Active Tab changes; takes it, so call once). Input is
    /// sent to whichever Session is the Active Tab's *now*
    /// (`Connection::active_session`), so it keeps working after a reconnect
    /// or a Tab switch; while there is no Session it is dropped. Frames are
    /// acknowledged by the Mirror through the Frame's own Session, so
    /// ack-after-display needs nothing here. Returns `None` if the Frame
    /// stream was already taken. Spawns on `runtime`.
    pub fn from_connection(conn: &Connection, runtime: Handle) -> Option<MirrorBackend> {
        let frames = conn.take_frames()?;
        let session = conn.active_session();
        let (input, mut input_rx) = mpsc::unbounded_channel::<MirrorInput>();
        runtime.spawn(async move {
            while let Some(msg) = input_rx.recv().await {
                let Some(session) = session.borrow().clone() else {
                    continue;
                };
                // Best effort, like `from_session`.
                let _ = match msg {
                    MirrorInput::Mouse(ev) => session.dispatch_mouse(ev).await,
                    MirrorInput::Key(ev) => session.dispatch_key(ev).await,
                    MirrorInput::InsertText(t) => session.insert_text(&t).await,
                };
            }
        });
        Some(MirrorBackend {
            frames,
            input,
            runtime,
        })
    }
}
