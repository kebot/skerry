use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{header::AUTHORIZATION, HeaderValue};
use tokio_tungstenite::tungstenite::Message;

use crate::error::CdpError;

const EVENT_BUFFER: usize = 1024;

/// A Manager/CDP bearer token. `Debug` never prints the secret.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    pub fn new(secret: impl Into<String>) -> Self {
        Token(secret.into())
    }
    /// The raw secret, for building headers. Never log this.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// A CDP event. `session_id` is set for events from a flat [`Session`] and
/// `None` for browser-level events.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

/// Receiver of [`Event`]s. Returns `None` once the connection is closed and
/// the buffered events are drained. If a slow consumer falls more than 1024
/// events behind, the oldest events are skipped.
pub struct Events {
    rx: broadcast::Receiver<Event>,
    closed: watch::Receiver<bool>,
    session: Option<String>,
}

impl Events {
    pub async fn recv(&mut self) -> Option<Event> {
        loop {
            let got = tokio::select! {
                r = self.rx.recv() => Some(r),
                _ = self.closed.wait_for(|c| *c) => None,
            };
            let ev = match got {
                Some(Ok(ev)) => ev,
                Some(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Some(Err(broadcast::error::RecvError::Closed)) => return None,
                // Connection closed: drain whatever was already broadcast.
                None => loop {
                    match self.rx.try_recv() {
                        Ok(ev) if self.matches(&ev) => return Some(ev),
                        Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                        Err(_) => return None,
                    }
                },
            };
            if self.matches(&ev) {
                return Some(ev);
            }
        }
    }

    fn matches(&self, ev: &Event) -> bool {
        match &self.session {
            Some(s) => ev.session_id.as_deref() == Some(s),
            None => true,
        }
    }
}

enum Cmd {
    Send {
        method: String,
        params: Option<Value>,
        session_id: Option<String>,
        reply: oneshot::Sender<Result<Value, CdpError>>,
    },
    Close,
}

/// One CDP WebSocket connection. Cheap to clone; all clones share the
/// connection, which closes when the last clone is dropped or
/// [`Client::close`] is called.
#[derive(Clone)]
pub struct Client {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    events_tx: broadcast::Sender<Event>,
    closed: watch::Receiver<bool>,
    close_code: Arc<AtomicU16>,
}

/// Why a connect attempt failed, with the HTTP status of a rejected
/// handshake kept apart (the supervisor classifies 401/403/404).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConnectFailure {
    Http(u16),
    Transport(String),
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl Client {
    /// Connect to a `ws://` or `wss://` CDP endpoint, optionally sending
    /// `Authorization: Bearer <token>` on the handshake.
    pub async fn connect(url: &str, token: Option<Token>) -> Result<Client, CdpError> {
        Self::connect_raw(url, token).await.map_err(|f| match f {
            ConnectFailure::Http(s) => CdpError::Transport(format!("HTTP error: {s}")),
            ConnectFailure::Transport(e) => CdpError::Transport(e),
        })
    }

    pub(crate) async fn connect_raw(
        url: &str,
        token: Option<Token>,
    ) -> Result<Client, ConnectFailure> {
        let mut req = url
            .into_client_request()
            .map_err(|e| ConnectFailure::Transport(e.to_string()))?;
        if let Some(t) = &token {
            let mut v =
                HeaderValue::from_str(&format!("Bearer {}", t.expose_secret())).map_err(|_| {
                    ConnectFailure::Transport("token is not a valid header value".into())
                })?;
            v.set_sensitive(true);
            req.headers_mut().insert(AUTHORIZATION, v);
        }
        let (ws, _) = tokio_tungstenite::connect_async(req)
            .await
            .map_err(|e| match e {
                tokio_tungstenite::tungstenite::Error::Http(r) => {
                    ConnectFailure::Http(r.status().as_u16())
                }
                e => ConnectFailure::Transport(e.to_string()),
            })?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (events_tx, _) = broadcast::channel(EVENT_BUFFER);
        let (closed_tx, closed) = watch::channel(false);
        let close_code = Arc::new(AtomicU16::new(0));
        tokio::spawn(drive(
            ws,
            cmd_rx,
            events_tx.clone(),
            closed_tx,
            close_code.clone(),
        ));
        Ok(Client {
            cmd_tx,
            events_tx,
            closed,
            close_code,
        })
    }

    /// Send a command and wait for its result. `session_id` routes it to a
    /// flat session (prefer [`Session::send`]).
    pub async fn send(
        &self,
        method: &str,
        params: Option<Value>,
        session_id: Option<&str>,
    ) -> Result<Value, CdpError> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::Send {
                method: method.to_string(),
                params,
                session_id: session_id.map(str::to_string),
                reply,
            })
            .map_err(|_| CdpError::Closed)?;
        rx.await.unwrap_or(Err(CdpError::Closed))
    }

    /// Subscribe to all events (browser-level and every session). Events
    /// sent before this call are not replayed.
    pub fn events(&self) -> Events {
        Events {
            rx: self.events_tx.subscribe(),
            closed: self.closed.clone(),
            session: None,
        }
    }

    /// True once the connection is gone.
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// The WebSocket close code the server sent, if it closed the
    /// connection with a Close frame that carried one (e.g. 4401, 4403).
    pub fn close_code(&self) -> Option<u16> {
        match self.close_code.load(Ordering::Relaxed) {
            0 => None,
            c => Some(c),
        }
    }

    /// Resolves when the connection has closed for any reason.
    pub async fn closed(&self) {
        let mut c = self.closed.clone();
        let _ = c.wait_for(|c| *c).await;
    }

    /// Close the WebSocket cleanly. Pending commands fail with
    /// [`CdpError::Closed`].
    pub async fn close(&self) {
        let _ = self.cmd_tx.send(Cmd::Close);
        self.closed().await;
    }

    /// `Target.attachToTarget` with `flatten: true`.
    pub async fn attach_to_target(&self, target_id: &str) -> Result<Session, CdpError> {
        let v = self
            .send(
                "Target.attachToTarget",
                Some(json!({"targetId": target_id, "flatten": true})),
                None,
            )
            .await?;
        let id = v
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| CdpError::Decode("attachToTarget: missing sessionId".into()))?;
        Ok(Session {
            client: self.clone(),
            id: id.to_string(),
            screencast: Default::default(),
        })
    }

    /// `Target.setDiscoverTargets`. When enabling, the browser replays a
    /// `targetCreated` event for every existing target.
    pub async fn set_discover_targets(&self, discover: bool) -> Result<(), CdpError> {
        self.send(
            "Target.setDiscoverTargets",
            Some(json!({"discover": discover})),
            None,
        )
        .await
        .map(|_| ())
    }

    /// `Target.getTargets`.
    pub async fn get_targets(&self) -> Result<Vec<crate::TargetInfo>, CdpError> {
        let v = self.send("Target.getTargets", None, None).await?;
        let infos = v.get("targetInfos").cloned().unwrap_or(json!([]));
        serde_json::from_value(infos).map_err(|e| CdpError::Decode(e.to_string()))
    }
}

/// A flat session attached to one target (a Tab's page). Commands sent
/// through it carry its `sessionId`.
#[derive(Clone, Debug)]
pub struct Session {
    client: Client,
    id: String,
    pub(crate) screencast: std::sync::Arc<crate::screencast::Slot>,
}

impl Session {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub async fn send(&self, method: &str, params: Option<Value>) -> Result<Value, CdpError> {
        self.client.send(method, params, Some(&self.id)).await
    }

    /// Events belonging to this session only.
    pub fn events(&self) -> Events {
        let mut e = self.client.events();
        e.session = Some(self.id.clone());
        e
    }

    /// `Target.detachFromTarget` for this session.
    pub async fn detach(&self) -> Result<(), CdpError> {
        self.client
            .send(
                "Target.detachFromTarget",
                Some(json!({"sessionId": self.id})),
                None,
            )
            .await
            .map(|_| ())
    }
}

type Pending = oneshot::Sender<Result<Value, CdpError>>;
type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn drive(
    ws: Ws,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    events_tx: broadcast::Sender<Event>,
    closed_tx: watch::Sender<bool>,
    close_code: Arc<AtomicU16>,
) {
    let (mut sink, mut stream) = ws.split();
    let mut pending: HashMap<u64, Pending> = HashMap::new();
    let mut next_id: u64 = 1;

    let end: CdpError = loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                None | Some(Cmd::Close) => {
                    let _ = sink.send(Message::Close(None)).await;
                    break CdpError::Closed;
                }
                Some(Cmd::Send { method, params, session_id, reply }) => {
                    let id = next_id;
                    next_id += 1;
                    let mut msg = json!({"id": id, "method": method});
                    if let Some(p) = params { msg["params"] = p; }
                    if let Some(s) = session_id { msg["sessionId"] = json!(s); }
                    match sink.send(Message::text(msg.to_string())).await {
                        Ok(()) => { pending.insert(id, reply); }
                        Err(e) => {
                            let err = CdpError::Transport(e.to_string());
                            let _ = reply.send(Err(err.clone()));
                            break err;
                        }
                    }
                }
            },
            msg = stream.next() => match msg {
                None => break CdpError::Closed,
                Some(Err(e)) => break CdpError::Transport(e.to_string()),
                Some(Ok(Message::Close(frame))) => {
                    if let Some(f) = frame {
                        close_code.store(u16::from(f.code), Ordering::Relaxed);
                    }
                    break CdpError::Closed;
                }
                Some(Ok(Message::Text(t))) => handle_text(t.as_str(), &mut pending, &events_tx),
                Some(Ok(_)) => {}
            },
        }
    };

    for (_, p) in pending.drain() {
        let _ = p.send(Err(end.clone()));
    }
    // Commands queued but never written.
    cmd_rx.close();
    while let Ok(cmd) = cmd_rx.try_recv() {
        if let Cmd::Send { reply, .. } = cmd {
            let _ = reply.send(Err(CdpError::Closed));
        }
    }
    let _ = closed_tx.send(true);
}

fn handle_text(
    text: &str,
    pending: &mut HashMap<u64, Pending>,
    events_tx: &broadcast::Sender<Event>,
) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    if let Some(id) = v.get("id").and_then(Value::as_u64) {
        if let Some(p) = pending.remove(&id) {
            let res = match v.get("error") {
                Some(e) => Err(CdpError::Protocol {
                    code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: e
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    data: e.get("data").and_then(Value::as_str).map(str::to_string),
                }),
                None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = p.send(res);
        }
    } else if let Some(method) = v.get("method").and_then(Value::as_str) {
        let _ = events_tx.send(Event {
            method: method.to_string(),
            params: v.get("params").cloned().unwrap_or(Value::Null),
            session_id: v
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
}
