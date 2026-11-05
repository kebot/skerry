//! A fake CDP server for tests (public module, no feature flag needed).
//!
//! [`FakeCdpServer`] is a local WebSocket server on `127.0.0.1` that speaks
//! just enough CDP to stand in for a Remote Browser: it records every
//! command it receives, answers configured methods with canned results,
//! emits events on demand, can drop connections, and can require a Bearer
//! token on the handshake. Unconfigured methods get a protocol error
//! `-32601 method not found`. Responses echo the command's `sessionId`.
//!
//! ```
//! # #[tokio::main(flavor = "current_thread")] async fn main() {
//! use rb_cdp::{testing::FakeCdpServer, Client};
//! use serde_json::json;
//!
//! let server = FakeCdpServer::start().await;
//! server.reply("Browser.getVersion", json!({"product": "Fake/1.0"}));
//!
//! let client = Client::connect(&server.url(), None).await.unwrap();
//! let v = client.send("Browser.getVersion", None, None).await.unwrap();
//! assert_eq!(v["product"], "Fake/1.0");
//! assert_eq!(server.commands()[0].method, "Browser.getVersion");
//!
//! // Push an event to every connected client.
//! let mut events = client.events();
//! server.emit("Page.loadEventFired", json!({"timestamp": 1.0}));
//! assert_eq!(events.recv().await.unwrap().method, "Page.loadEventFired");
//! # }
//! ```

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

/// A command the fake server received.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedCommand {
    pub id: u64,
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

#[derive(Clone)]
enum Handler {
    Result(Value),
    Error(i64, String),
    /// Never answer (to test pending commands).
    Silent,
    /// Compute the answer from the command (`Err` is a protocol error).
    Dynamic(DynamicHandler),
}

/// See [`FakeCdpServer::reply_with`].
type DynamicHandler = Arc<dyn Fn(&RecordedCommand) -> Result<Value, (i64, String)> + Send + Sync>;

#[derive(Clone)]
struct Emit {
    method: String,
    params: Value,
    session_id: Option<String>,
}

enum ConnMsg {
    Text(String),
    Drop,
    Close(u16),
}

#[derive(Default)]
struct State {
    commands: Vec<RecordedCommand>,
    handlers: HashMap<String, Handler>,
    after: HashMap<String, Vec<Emit>>,
    conns: Vec<(u64, mpsc::UnboundedSender<ConnMsg>)>,
    next_conn: u64,
    accepted: usize,
    authorizations: Vec<Option<String>>,
    require_token: Option<String>,
    reject_next: usize,
    reject_status: u16,
    reject_all: Option<u16>,
    refusing: bool,
}

struct Shared {
    state: Mutex<State>,
    notify: Notify,
}

/// Fake CDP WebSocket server. See the [module docs](self). The listener
/// stops when this value is dropped.
pub struct FakeCdpServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

const WAIT: Duration = Duration::from_secs(5);

impl FakeCdpServer {
    /// Bind to an ephemeral port on 127.0.0.1 and start accepting.
    pub async fn start() -> FakeCdpServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            notify: Notify::new(),
        });
        let sh = shared.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                if sh.state.lock().unwrap().refusing {
                    drop(stream); // looks like a dead server: connection reset / EOF
                    continue;
                }
                tokio::spawn(serve(stream, sh.clone()));
            }
        });
        FakeCdpServer { addr, shared, task }
    }

    /// `ws://127.0.0.1:<port>/`
    pub fn url(&self) -> String {
        format!("ws://{}/", self.addr)
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared.state.lock().unwrap()
    }

    /// Answer `method` with `result`.
    pub fn reply(&self, method: &str, result: Value) {
        self.st()
            .handlers
            .insert(method.into(), Handler::Result(result));
    }

    /// Answer `method` with a protocol error.
    pub fn reply_error(&self, method: &str, code: i64, message: &str) {
        self.st()
            .handlers
            .insert(method.into(), Handler::Error(code, message.into()));
    }

    /// Answer `method` by calling `f` for every command (e.g. a different
    /// `sessionId` per `targetId`, or an error only for one session).
    /// `Err((code, message))` becomes a protocol error.
    pub fn reply_with(
        &self,
        method: &str,
        f: impl Fn(&RecordedCommand) -> Result<Value, (i64, String)> + Send + Sync + 'static,
    ) {
        self.st()
            .handlers
            .insert(method.into(), Handler::Dynamic(Arc::new(f)));
    }

    /// Record `method` but never answer it.
    pub fn never_reply(&self, method: &str) {
        self.st().handlers.insert(method.into(), Handler::Silent);
    }

    /// After answering `method`, also emit this event (browser-level), e.g.
    /// to replay targets on `Target.setDiscoverTargets`.
    pub fn emit_after(&self, method: &str, event: &str, params: Value) {
        self.st()
            .after
            .entry(method.into())
            .or_default()
            .push(Emit {
                method: event.into(),
                params,
                session_id: None,
            });
    }

    /// Emit a browser-level event to all connected clients.
    pub fn emit(&self, method: &str, params: Value) {
        self.send_event(method, params, None);
    }

    /// Emit an event belonging to a flat session.
    pub fn emit_in_session(&self, session_id: &str, method: &str, params: Value) {
        self.send_event(method, params, Some(session_id.into()));
    }

    fn send_event(&self, method: &str, params: Value, session_id: Option<String>) {
        let text = event_json(&Emit {
            method: method.into(),
            params,
            session_id,
        });
        for (_, tx) in &self.st().conns {
            let _ = tx.send(ConnMsg::Text(text.clone()));
        }
    }

    /// Close every open connection (no Close frame is guaranteed; the
    /// client sees the stream end).
    pub fn drop_connections(&self) {
        for (_, tx) in self.st().conns.drain(..) {
            let _ = tx.send(ConnMsg::Drop);
        }
    }

    /// Close every open connection with a WebSocket Close frame carrying
    /// `code` (e.g. 4401, 4403, 4004).
    pub fn close_connections_with(&self, code: u16) {
        for (_, tx) in self.st().conns.drain(..) {
            let _ = tx.send(ConnMsg::Close(code));
        }
    }

    /// Answer the next `n` handshakes with HTTP `status` (e.g. 404, 503).
    pub fn reject_next_handshakes(&self, n: usize, status: u16) {
        let mut st = self.st();
        st.reject_next = n;
        st.reject_status = status;
    }

    /// Answer every handshake with HTTP `status` until called with `None`.
    pub fn reject_all_handshakes(&self, status: Option<u16>) {
        self.st().reject_all = status;
    }

    /// While `true`, accepted TCP connections are closed immediately, as if
    /// the server were down. Combine with [`FakeCdpServer::drop_connections`]
    /// to simulate a restart on the same port; handlers and recorded state
    /// are kept.
    pub fn refuse_connections(&self, refuse: bool) {
        self.st().refusing = refuse;
    }

    /// Forget the events registered with [`FakeCdpServer::emit_after`] for
    /// `method` (e.g. to change the targets after a simulated restart).
    pub fn clear_emit_after(&self, method: &str) {
        self.st().after.remove(method);
    }

    /// Reject handshakes that lack `Authorization: Bearer <token>` with 401.
    pub fn require_bearer(&self, token: &str) {
        self.st().require_token = Some(token.into());
    }

    /// The `Authorization` header of each handshake so far (accepted or
    /// not), in order.
    pub fn authorizations(&self) -> Vec<Option<String>> {
        self.st().authorizations.clone()
    }

    /// All commands received so far, in order.
    pub fn commands(&self) -> Vec<RecordedCommand> {
        self.st().commands.clone()
    }

    /// Number of connections accepted so far.
    pub fn accepted_connections(&self) -> usize {
        self.st().accepted
    }

    /// Wait (5 s max) until a command with `method` has been received at
    /// least `count` times; returns the first matching command after
    /// waiting, or `None` on timeout.
    pub async fn wait_for_command(&self, method: &str, count: usize) -> Option<RecordedCommand> {
        self.wait(|s| {
            let m: Vec<_> = s.commands.iter().filter(|c| c.method == method).collect();
            (m.len() >= count).then(|| m[count - 1].clone())
        })
        .await
    }

    /// Wait (5 s max) until at least `n` connections are open.
    pub async fn wait_for_connections(&self, n: usize) -> bool {
        self.wait(|s| (s.conns.len() >= n).then_some(()))
            .await
            .is_some()
    }

    async fn wait<T>(&self, mut f: impl FnMut(&State) -> Option<T>) -> Option<T> {
        let fut = async {
            loop {
                let notified = self.shared.notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if let Some(v) = f(&self.st()) {
                    return v;
                }
                notified.await;
            }
        };
        tokio::time::timeout(WAIT, fut).await.ok()
    }
}

impl Drop for FakeCdpServer {
    fn drop(&mut self) {
        self.task.abort();
        self.drop_connections();
    }
}

fn event_json(e: &Emit) -> String {
    let mut v = json!({"method": e.method, "params": e.params});
    if let Some(s) = &e.session_id {
        v["sessionId"] = json!(s);
    }
    v.to_string()
}

// The handshake callback's Err type is dictated by tungstenite.
#[allow(clippy::result_large_err)]
async fn serve(stream: tokio::net::TcpStream, shared: Arc<Shared>) {
    let sh = shared.clone();
    let cb = move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let auth = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let mut st = sh.state.lock().unwrap();
        st.authorizations.push(auth.clone());
        let reject = if let Some(s) = st.reject_all {
            Some(s)
        } else if st.reject_next > 0 {
            st.reject_next -= 1;
            Some(st.reject_status)
        } else {
            None
        };
        let ok = reject.is_none()
            && match &st.require_token {
                Some(t) => auth.as_deref() == Some(&format!("Bearer {t}")),
                None => true,
            };
        drop(st);
        sh.notify.notify_waiters();
        if ok {
            Ok(resp)
        } else {
            let mut r = ErrorResponse::new(Some("rejected".to_string()));
            *r.status_mut() = reject
                .and_then(|s| StatusCode::from_u16(s).ok())
                .unwrap_or(StatusCode::UNAUTHORIZED);
            Err(r)
        }
    };
    let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, cb).await else {
        return;
    };
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let conn_id = {
        let mut st = shared.state.lock().unwrap();
        let id = st.next_conn;
        st.next_conn += 1;
        st.accepted += 1;
        st.conns.push((id, tx.clone()));
        id
    };
    shared.notify.notify_waiters();

    loop {
        tokio::select! {
            out = rx.recv() => match out {
                Some(ConnMsg::Text(t)) => { if sink.send(Message::text(t)).await.is_err() { break; } }
                Some(ConnMsg::Close(code)) => {
                    let frame = CloseFrame { code: CloseCode::from(code), reason: "".into() };
                    let _ = sink.send(Message::Close(Some(frame))).await;
                    break;
                }
                Some(ConnMsg::Drop) | None => break,
            },
            inc = stream.next() => match inc {
                Some(Ok(Message::Text(t))) => {
                    let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else { continue };
                    let cmd = RecordedCommand {
                        id: v["id"].as_u64().unwrap_or(0),
                        method: v["method"].as_str().unwrap_or("").to_string(),
                        params: v.get("params").cloned().unwrap_or(Value::Null),
                        session_id: v.get("sessionId").and_then(Value::as_str).map(str::to_string),
                    };
                    let (handler, after) = {
                        let mut st = shared.state.lock().unwrap();
                        st.commands.push(cmd.clone());
                        (st.handlers.get(&cmd.method).cloned(), st.after.get(&cmd.method).cloned())
                    };
                    shared.notify.notify_waiters();
                    let mut resp = json!({"id": cmd.id});
                    if let Some(s) = &cmd.session_id { resp["sessionId"] = json!(s); }
                    match handler {
                        Some(Handler::Silent) => continue,
                        Some(Handler::Result(r)) => resp["result"] = r,
                        Some(Handler::Dynamic(f)) => match f(&cmd) {
                            Ok(r) => resp["result"] = r,
                            Err((c, m)) => resp["error"] = json!({"code": c, "message": m}),
                        },
                        Some(Handler::Error(c, m)) => resp["error"] = json!({"code": c, "message": m}),
                        None => resp["error"] = json!({"code": -32601, "message": "method not found"}),
                    }
                    if sink.send(Message::text(resp.to_string())).await.is_err() { break; }
                    for e in after.unwrap_or_default() {
                        if sink.send(Message::text(event_json(&e))).await.is_err() { break; }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    shared
        .state
        .lock()
        .unwrap()
        .conns
        .retain(|(id, _)| *id != conn_id);
    shared.notify.notify_waiters();
    // Dropping sink/stream closes the TCP connection.
}
