//! Per-Connection local CDP proxy for Agents (ADR 0003).
//!
//! [`AgentProxy`] listens on `127.0.0.1` only and lets any CDP-speaking
//! Agent (Playwright / Puppeteer `connect_over_cdp`, Playwright MCP
//! `--cdp-endpoint`, browser-use, chrome-remote-interface) attach to the
//! Remote Browser through rb. The Agent never sees the Manager token: the
//! proxy adds the `Authorization: Bearer` header on its own upstream
//! WebSocket. Every downstream client gets its own upstream WebSocket (like
//! the Manager's own `/cdp` proxy), so the Agent's sessions are independent
//! of rb's own [`crate::Connection`].
//!
//! Served from `http://127.0.0.1:PORT[/SECRET]`:
//! `GET /json/version`, `GET /json` and `/json/list`, `/json/new?URL`,
//! `/json/activate/ID`, `/json/close/ID`, the browser-level WebSocket
//! (`/devtools/browser/<any>`) and page-level WebSockets
//! (`/devtools/page/<targetId>`, implemented by attaching a flat session
//! upstream and translating `sessionId`).
//!
//! Security: bind to 127.0.0.1 only; any request with an `Origin` header is
//! refused with 403 (browsers always send one on WebSockets; Playwright and
//! Puppeteer send none); the `Host` header must be `127.0.0.1`, `localhost`
//! or `[::1]` (DNS rebinding); an optional random path secret is required in
//! the URL. Nothing listens until [`AgentProxy::start`] is called. The token
//! is never logged.
//!
//! Activity tracking: frames are inspected (never altered on the browser
//! relay) to learn which targets the Agent touches; see [`AgentEvent`] and
//! [`AgentState`]. Concurrency: the Agent may call e.g.
//! `Emulation.setDeviceMetricsOverride`; there is no arbitration in v1, but
//! it is recorded as [`TargetActivity::emulation_override`].

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{header::AUTHORIZATION, HeaderValue};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::{CdpError, Client, Token};

/// An Agent counts as "acting now" on a target if it acted within this long.
pub const ACTING_WINDOW: Duration = Duration::from_secs(2);

const MAX_HEAD: usize = 16 * 1024;
const MAX_PARSED_UPSTREAM: usize = 256 * 1024;

/// Configuration for [`AgentProxy::start`].
#[derive(Clone)]
pub struct AgentProxyConfig {
    /// Browser-level CDP WebSocket of the Remote Browser
    /// (`wss://host/api/profiles/1/cdp`).
    pub upstream_url: String,
    /// Manager token, added upstream as `Authorization: Bearer`.
    pub upstream_token: Option<Token>,
    /// Port on 127.0.0.1; 0 picks an ephemeral port.
    pub bind_port: u16,
    /// Optional path secret; see [`random_path_secret`].
    pub path_secret: Option<String>,
}

impl AgentProxyConfig {
    pub fn new(upstream_url: impl Into<String>, upstream_token: Option<Token>) -> Self {
        AgentProxyConfig {
            upstream_url: upstream_url.into(),
            upstream_token,
            bind_port: 0,
            path_secret: None,
        }
    }
}

impl std::fmt::Debug for AgentProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentProxyConfig")
            .field("upstream_url", &self.upstream_url)
            .field("upstream_token", &self.upstream_token)
            .field("bind_port", &self.bind_port)
            .field(
                "path_secret",
                &self.path_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// A 32-hex-char secret from OS-seeded hashers (no extra RNG dependency).
pub fn random_path_secret() -> String {
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            h.write_u128(d.as_nanos());
        }
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// Something the Agent did, as seen by the proxy.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// A downstream Agent client connected (its upstream is open).
    Attached { client_id: u64 },
    /// A downstream Agent client went away (or its upstream dropped).
    Detached { client_id: u64 },
    /// The Agent sent a command that touches `target_id`.
    Acted {
        target_id: String,
        method: String,
        at: Instant,
    },
}

/// What the Agent did to one target.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetActivity {
    pub last_acted_at: Instant,
    pub last_method: String,
    /// The Agent changed emulation (`Emulation.set*Override`) on this target;
    /// that affects what the user sees (no arbitration in v1).
    pub emulation_override: bool,
}

/// Snapshot published on a `watch` channel.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentState {
    /// Connected Agent clients.
    pub clients: usize,
    /// Targets the Agent has touched since the proxy started.
    pub targets: HashMap<String, TargetActivity>,
}

impl AgentState {
    /// Acted on `target_id` within `within` of `now` ("acting now").
    pub fn is_acting(&self, target_id: &str, within: Duration, now: Instant) -> bool {
        self.targets
            .get(target_id)
            .is_some_and(|a| now.saturating_duration_since(a.last_acted_at) <= within)
    }
    /// Acted on `target_id` at any point ("touched earlier").
    pub fn touched(&self, target_id: &str) -> bool {
        self.targets.contains_key(target_id)
    }
}

struct Shared {
    cfg: AgentProxyConfig,
    events: broadcast::Sender<AgentEvent>,
    state: watch::Sender<AgentState>,
    next_client: AtomicU64,
    shutdown: watch::Receiver<bool>,
}

impl Shared {
    fn acted(&self, target: &str, method: &str) {
        let at = Instant::now();
        let emu = method.starts_with("Emulation.") && method.contains("Override");
        self.state.send_modify(|s| {
            let e = s
                .targets
                .entry(target.to_string())
                .or_insert_with(|| TargetActivity {
                    last_acted_at: at,
                    last_method: String::new(),
                    emulation_override: false,
                });
            e.last_acted_at = at;
            e.last_method = method.to_string();
            e.emulation_override |= emu;
        });
        let _ = self.events.send(AgentEvent::Acted {
            target_id: target.to_string(),
            method: method.to_string(),
            at,
        });
    }
}

/// The running proxy. Dropping it stops it.
pub struct AgentProxy {
    addr: SocketAddr,
    secret: Option<String>,
    shared: Arc<Shared>,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl AgentProxy {
    /// Bind `127.0.0.1:bind_port` and start serving.
    pub async fn start(cfg: AgentProxyConfig) -> Result<AgentProxy, CdpError> {
        let listener = TcpListener::bind(("127.0.0.1", cfg.bind_port))
            .await
            .map_err(|e| CdpError::Transport(format!("bind: {e}")))?;
        let addr = listener
            .local_addr()
            .map_err(|e| CdpError::Transport(e.to_string()))?;
        let (events, _) = broadcast::channel(1024);
        let (state, _) = watch::channel(AgentState::default());
        let (sd_tx, sd_rx) = watch::channel(false);
        let secret = cfg.path_secret.clone().filter(|s| !s.is_empty());
        let shared = Arc::new(Shared {
            cfg,
            events,
            state,
            next_client: AtomicU64::new(1),
            shutdown: sd_rx,
        });
        let sh = shared.clone();
        let sec = secret.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let sh = sh.clone();
                let sec = sec.clone();
                tokio::spawn(async move {
                    let mut stop = sh.shutdown.clone();
                    tokio::select! {
                        _ = handle_conn(stream, sh, sec) => {}
                        _ = stopped(&mut stop) => {}
                    }
                });
            }
        });
        Ok(AgentProxy {
            addr,
            secret,
            shared,
            shutdown: sd_tx,
            task,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    fn base_path(&self) -> String {
        match &self.secret {
            Some(s) => format!("/{s}"),
            None => String::new(),
        }
    }

    /// The URL to give the Agent: `http://127.0.0.1:PORT[/SECRET]` (no
    /// trailing slash). Works for Playwright (`connect_over_cdp`) and
    /// Playwright MCP `--cdp-endpoint`. Puppeteer resolves `/json/version`
    /// against the origin, so with a path secret give it
    /// [`AgentProxy::ws_endpoint`] as `browserWSEndpoint` instead.
    pub fn endpoint(&self) -> String {
        format!("http://{}{}", self.addr, self.base_path())
    }

    /// Browser-level WebSocket URL (works without an HTTP lookup).
    pub fn ws_endpoint(&self) -> String {
        format!("ws://{}{}/devtools/browser/rb", self.addr, self.base_path())
    }

    /// Subscribe to Agent activity events.
    pub fn events(&self) -> broadcast::Receiver<AgentEvent> {
        self.shared.events.subscribe()
    }

    /// Watch the Agent state snapshot.
    pub fn state(&self) -> watch::Receiver<AgentState> {
        self.shared.state.subscribe()
    }

    /// Stop listening and drop all Agent connections.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for AgentProxy {
    fn drop(&mut self) {
        self.task.abort();
        let _ = self.shutdown.send(true);
    }
}

// ---------------------------------------------------------------- HTTP --

struct Head {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
    len: usize,
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

async fn peek_head(stream: &TcpStream) -> Option<Head> {
    let mut buf = vec![0u8; MAX_HEAD];
    let fut = async {
        loop {
            let n = stream.peek(&mut buf).await.ok()?;
            if n == 0 {
                return None;
            }
            if let Some(p) = find(&buf[..n], b"\r\n\r\n") {
                return Some(p + 4);
            }
            if n == buf.len() {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    let len = tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .ok()??;
    let text = String::from_utf8_lossy(&buf[..len]).into_owned();
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut headers = HashMap::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    Some(Head {
        method,
        path: path.to_string(),
        query: query.to_string(),
        headers,
        len,
    })
}

/// Respond to a request whose head is still unread: drain it first, or the
/// close would reset the connection before the client reads the answer.
async fn reject(stream: &mut TcpStream, head_len: usize, status: u16, reason: &str, body: &str) {
    let mut sink = vec![0u8; head_len];
    let _ = stream.read_exact(&mut sink).await;
    respond(stream, status, reason, body).await;
}

async fn respond(stream: &mut TcpStream, status: u16, reason: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn host_ok(host: &str) -> bool {
    let h = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host.rsplit_once(':').map_or(host, |(h, _)| h)
    };
    matches!(
        h.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}

/// Strip the path secret; `None` if it does not match.
fn strip_secret<'a>(path: &'a str, secret: &Option<String>) -> Option<&'a str> {
    let Some(s) = secret else { return Some(path) };
    let rest = path.strip_prefix('/')?;
    let (first, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    ct_eq(first, s).then_some(tail)
}

async fn handle_conn(mut stream: TcpStream, shared: Arc<Shared>, secret: Option<String>) {
    let Some(head) = peek_head(&stream).await else {
        return;
    };
    // Security checks, before anything touches the upstream.
    if head.headers.contains_key("origin") {
        return reject(
            &mut stream,
            head.len,
            403,
            "Forbidden",
            "{\"error\":\"Origin not allowed\"}",
        )
        .await;
    }
    if !head.headers.get("host").is_some_and(|h| host_ok(h)) {
        return reject(
            &mut stream,
            head.len,
            403,
            "Forbidden",
            "{\"error\":\"Host not allowed\"}",
        )
        .await;
    }
    let Some(path) = strip_secret(&head.path, &secret) else {
        return reject(
            &mut stream,
            head.len,
            404,
            "Not Found",
            "{\"error\":\"not found\"}",
        )
        .await;
    };
    let path = path.to_string();
    let host = head.headers["host"].clone();
    let is_ws = head
        .headers
        .get("upgrade")
        .is_some_and(|u| u.eq_ignore_ascii_case("websocket"));

    if is_ws {
        let target = if path.starts_with("/devtools/browser/") {
            None
        } else if let Some(id) = path.strip_prefix("/devtools/page/") {
            Some(id.trim_end_matches('/').to_string())
        } else {
            return reject(
                &mut stream,
                head.len,
                404,
                "Not Found",
                "{\"error\":\"not found\"}",
            )
            .await;
        };
        // Open the upstream first so a dead browser is a clean 502.
        let mut up = match connect_upstream(&shared.cfg).await {
            Ok(u) => u,
            Err(_) => {
                return reject(
                    &mut stream,
                    head.len,
                    502,
                    "Bad Gateway",
                    "{\"error\":\"upstream unavailable\"}",
                )
                .await
            }
        };
        let mut sessions = HashMap::new();
        let mut page_session = None;
        if let Some(t) = &target {
            match attach_page(&mut up, t).await {
                Some(sid) => {
                    sessions.insert(sid.clone(), t.clone());
                    page_session = Some(sid);
                }
                None => {
                    return reject(
                        &mut stream,
                        head.len,
                        404,
                        "Not Found",
                        "{\"error\":\"no such target\"}",
                    )
                    .await
                }
            }
        }
        let Ok(down) = tokio_tungstenite::accept_async(stream).await else {
            return;
        };
        let client_id = shared.next_client.fetch_add(1, Ordering::Relaxed);
        shared.state.send_modify(|s| s.clients += 1);
        let _ = shared.events.send(AgentEvent::Attached { client_id });
        let mut tracker = Tracker {
            shared: shared.clone(),
            sessions,
            pending: HashMap::new(),
        };
        relay(down, up, &mut tracker, page_session).await;
        shared
            .state
            .send_modify(|s| s.clients = s.clients.saturating_sub(1));
        let _ = shared.events.send(AgentEvent::Detached { client_id });
        return;
    }

    // Plain HTTP: consume the head, ignore any body.
    let mut sink = vec![0u8; head.len];
    if stream.read_exact(&mut sink).await.is_err() {
        return;
    }
    if head.method != "GET" && head.method != "PUT" {
        return respond(&mut stream, 405, "Method Not Allowed", "{}").await;
    }
    match http_route(&shared, &path, &head.query, &host).await {
        Ok(body) => respond(&mut stream, 200, "OK", &body).await,
        Err((404, m)) => {
            respond(
                &mut stream,
                404,
                "Not Found",
                &json!({"error": m}).to_string(),
            )
            .await
        }
        Err((_, m)) => {
            respond(
                &mut stream,
                502,
                "Bad Gateway",
                &json!({"error": m}).to_string(),
            )
            .await
        }
    }
}

fn target_entry(t: &Value, host: &str, secret_base: &str) -> Value {
    let id = t["targetId"].as_str().unwrap_or("");
    json!({
        "description": "",
        "devtoolsFrontendUrl": "",
        "id": id,
        "title": t["title"],
        "type": t["type"],
        "url": t["url"],
        "webSocketDebuggerUrl": format!("ws://{host}{secret_base}/devtools/page/{id}"),
    })
}

async fn http_route(
    shared: &Shared,
    path: &str,
    query: &str,
    host: &str,
) -> Result<String, (u16, String)> {
    let cfg = &shared.cfg;
    let secret_base = match &cfg.path_secret {
        Some(s) if !s.is_empty() => format!("/{s}"),
        _ => String::new(),
    };
    let path = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    };
    let up_err = |e: CdpError| (502u16, format!("upstream: {e}"));
    let connect = || async {
        Client::connect(&cfg.upstream_url, cfg.upstream_token.clone())
            .await
            .map_err(up_err)
    };
    match path {
        "/json/version" => {
            let c = connect().await?;
            let v = c.send("Browser.getVersion", None, None).await;
            c.close().await;
            let v = v.map_err(up_err)?;
            let s = |k: &str, d: &str| v[k].as_str().unwrap_or(d).to_string();
            Ok(json!({
                "Browser": s("product", "rb-proxy"),
                "Protocol-Version": s("protocolVersion", "1.3"),
                "User-Agent": s("userAgent", ""),
                "V8-Version": s("jsVersion", ""),
                "WebKit-Version": s("revision", ""),
                "webSocketDebuggerUrl": format!("ws://{host}{secret_base}/devtools/browser/rb"),
            })
            .to_string())
        }
        "/json" | "/json/list" => {
            let c = connect().await?;
            let r = c.send("Target.getTargets", None, None).await;
            c.close().await;
            let r = r.map_err(up_err)?;
            let list: Vec<Value> = r["targetInfos"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|t| t["type"] != "browser")
                        .map(|t| target_entry(t, host, &secret_base))
                        .collect()
                })
                .unwrap_or_default();
            Ok(Value::Array(list).to_string())
        }
        "/json/new" => {
            let url = if query.is_empty() {
                "about:blank"
            } else {
                query
            };
            let c = connect().await?;
            let r = c
                .send("Target.createTarget", Some(json!({"url": url})), None)
                .await;
            let r = match r {
                Ok(r) => r,
                Err(e) => {
                    c.close().await;
                    return Err(up_err(e));
                }
            };
            let id = r["targetId"].as_str().unwrap_or("").to_string();
            let info = c
                .send("Target.getTargetInfo", Some(json!({"targetId": id})), None)
                .await
                .ok()
                .map(|i| i["targetInfo"].clone())
                .unwrap_or_else(
                    || json!({"targetId": id, "type": "page", "title": "", "url": url}),
                );
            c.close().await;
            shared.acted(&id, "Target.createTarget");
            Ok(target_entry(&info, host, &secret_base).to_string())
        }
        p if p.starts_with("/json/activate/") || p.starts_with("/json/close/") => {
            let (method, id) = match p.strip_prefix("/json/activate/") {
                Some(id) => ("Target.activateTarget", id),
                None => ("Target.closeTarget", &p["/json/close/".len()..]),
            };
            let c = connect().await?;
            let r = c.send(method, Some(json!({"targetId": id})), None).await;
            c.close().await;
            match r {
                Ok(_) => {
                    shared.acted(id, method);
                    Ok(if method == "Target.closeTarget" {
                        "Target is closing".into()
                    } else {
                        "Target activated".into()
                    })
                }
                Err(e) if e.is_protocol() => Err((404, "no such target".into())),
                Err(e) => Err(up_err(e)),
            }
        }
        _ => Err((404, "not found".into())),
    }
}

// ------------------------------------------------------------ WebSocket --

type Up = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Down = WebSocketStream<TcpStream>;

async fn connect_upstream(cfg: &AgentProxyConfig) -> Result<Up, String> {
    let mut req = cfg
        .upstream_url
        .as_str()
        .into_client_request()
        .map_err(|e| e.to_string())?;
    if let Some(t) = &cfg.upstream_token {
        let mut v = HeaderValue::from_str(&format!("Bearer {}", t.expose_secret()))
            .map_err(|_| "token is not a valid header value".to_string())?;
        v.set_sensitive(true);
        req.headers_mut().insert(AUTHORIZATION, v);
    }
    let (ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .map_err(|e| e.to_string())?;
    Ok(ws)
}

/// Attach a flat session to `target` on a fresh upstream (page-level WS).
async fn attach_page(up: &mut Up, target: &str) -> Option<String> {
    let cmd = json!({"id": 1, "method": "Target.attachToTarget",
        "params": {"targetId": target, "flatten": true}});
    up.send(Message::text(cmd.to_string())).await.ok()?;
    let wait = async {
        while let Some(Ok(m)) = up.next().await {
            if let Message::Text(t) = m {
                let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else {
                    continue;
                };
                if v["id"] == 1 && v.get("sessionId").is_none() {
                    return v["result"]["sessionId"].as_str().map(str::to_string);
                }
            }
        }
        None
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .ok()?
}

enum Pending {
    Attach(String),
    Create,
}

/// Learns targets and activity from frames; never modifies them.
struct Tracker {
    shared: Arc<Shared>,
    sessions: HashMap<String, String>,
    pending: HashMap<u64, Pending>,
}

impl Tracker {
    /// A frame the Agent sent upstream.
    fn on_command(&mut self, text: &str) {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let Some(method) = v["method"].as_str() else {
            return;
        };
        let id = v["id"].as_u64();
        if let Some(sid) = v["sessionId"].as_str() {
            if let Some(t) = self.sessions.get(sid) {
                self.shared.acted(t, method);
            }
            return;
        }
        let tid = v["params"]["targetId"].as_str();
        match (method, tid, id) {
            ("Target.attachToTarget", Some(t), Some(id)) => {
                self.pending.insert(id, Pending::Attach(t.to_string()));
            }
            ("Target.createTarget", _, Some(id)) => {
                self.pending.insert(id, Pending::Create);
            }
            ("Target.activateTarget" | "Target.closeTarget", Some(t), _) => {
                self.shared.acted(t, method);
            }
            _ => {}
        }
    }

    /// A frame the browser sent downstream.
    fn on_event(&mut self, text: &str) {
        let is_target_event =
            text.contains("Target.attachedToTarget") || text.contains("Target.detachedFromTarget");
        if !is_target_event && (self.pending.is_empty() || text.len() > MAX_PARSED_UPSTREAM) {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return;
        };
        if let Some(id) = v["id"].as_u64() {
            match self.pending.remove(&id) {
                Some(Pending::Attach(t)) => {
                    if let Some(s) = v["result"]["sessionId"].as_str() {
                        self.sessions.insert(s.to_string(), t);
                    }
                }
                Some(Pending::Create) => {
                    if let Some(t) = v["result"]["targetId"].as_str() {
                        self.shared.acted(t, "Target.createTarget");
                    }
                }
                None => {}
            }
            return;
        }
        match v["method"].as_str() {
            Some("Target.attachedToTarget") => {
                if let (Some(s), Some(t)) = (
                    v["params"]["sessionId"].as_str(),
                    v["params"]["targetInfo"]["targetId"].as_str(),
                ) {
                    self.sessions.insert(s.to_string(), t.to_string());
                }
            }
            Some("Target.detachedFromTarget") => {
                if let Some(s) = v["params"]["sessionId"].as_str() {
                    self.sessions.remove(s);
                }
            }
            _ => {}
        }
    }
}

/// Relay frames both ways until either side closes. With `page_session`
/// (page-level WS) the Agent's frames get that `sessionId` injected and the
/// browser's frames are filtered to it and stripped of it.
async fn relay(mut down: Down, mut up: Up, tracker: &mut Tracker, page_session: Option<String>) {
    let mut stop = tracker.shared.shutdown.clone();
    loop {
        tokio::select! {
            _ = stopped(&mut stop) => break,
            d = down.next() => match d {
                Some(Ok(Message::Text(t))) => {
                    let out = match &page_session {
                        Some(sid) => match inject_session(t.as_str(), sid) {
                            Some(s) => Message::text(s),
                            None => Message::Text(t),
                        },
                        None => Message::Text(t),
                    };
                    if let Message::Text(t) = &out { tracker.on_command(t.as_str()); }
                    if up.send(out).await.is_err() { break; }
                }
                Some(Ok(m @ Message::Binary(_))) => { if up.send(m).await.is_err() { break; } }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            u = up.next() => match u {
                Some(Ok(Message::Text(t))) => {
                    tracker.on_event(t.as_str());
                    let out = match &page_session {
                        Some(sid) => match strip_session(t.as_str(), sid) {
                            Strip::Forward(s) => Message::text(s),
                            Strip::Drop => continue,
                            Strip::Detached => break,
                        },
                        None => Message::Text(t),
                    };
                    if down.send(out).await.is_err() { break; }
                }
                Some(Ok(m @ Message::Binary(_))) => { if down.send(m).await.is_err() { break; } }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    let _ = down.close(None).await;
    let _ = up.close(None).await;
}

fn inject_session(text: &str, sid: &str) -> Option<String> {
    let mut v: Value = serde_json::from_str(text).ok()?;
    let o: &mut Map<String, Value> = v.as_object_mut()?;
    o.entry("sessionId").or_insert_with(|| json!(sid));
    Some(v.to_string())
}

enum Strip {
    Forward(String),
    Drop,
    Detached,
}

fn strip_session(text: &str, sid: &str) -> Strip {
    let Ok(mut v) = serde_json::from_str::<Value>(text) else {
        return Strip::Drop;
    };
    if v["method"] == "Target.detachedFromTarget" && v["params"]["sessionId"] == sid {
        return Strip::Detached;
    }
    if v["sessionId"] == sid {
        if let Some(o) = v.as_object_mut() {
            o.remove("sessionId");
        }
        Strip::Forward(v.to_string())
    } else {
        Strip::Drop
    }
}

async fn stopped(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|s| *s).await;
}
