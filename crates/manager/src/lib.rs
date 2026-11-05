//! rb-manager: REST client for the CloakBrowser Manager.
//!
//! Lists Profiles, launches and stops their Remote Browsers, and builds the
//! CDP WebSocket URL for a Profile (consumed by `rb-cdp`). Auth is one
//! optional static token sent as `Authorization: Bearer <token>`.

use std::fmt;

use reqwest::{header, Method, StatusCode, Url};
use serde::Deserialize;

/// A Manager auth token. Its `Debug` output is redacted; never log it.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    pub fn new(token: impl Into<String>) -> Self {
        Token(token.into())
    }

    /// The raw token, for building an `Authorization` header only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// Errors from the Manager client.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// HTTP 401: token missing or wrong. (WebSocket close code 4401 means the same.)
    #[error("manager rejected the token (401 unauthorized)")]
    Unauthorized,
    /// HTTP 404: unknown Profile, or Profile not running (for stop).
    #[error("not found: {detail}")]
    NotFound { detail: String },
    /// HTTP 409 on launch: the Profile is already running. Non-fatal; the
    /// caller can simply connect.
    #[error("profile is already running: {detail}")]
    AlreadyRunning { detail: String },
    /// HTTP 402 (out of seats) or 403 (other license problem) on launch.
    #[error("license error (HTTP {status}): {detail}")]
    License { status: u16, detail: String },
    /// Could not reach the Manager, or the request/URL was invalid.
    #[error("transport error: {0}")]
    Transport(String),
    /// Any other status, or a body that did not parse.
    #[error("unexpected response (HTTP {status}): {detail}")]
    UnexpectedResponse { status: u16, detail: String },
}

impl Error {
    pub fn is_already_running(&self) -> bool {
        matches!(self, Error::AlreadyRunning { .. })
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        // reqwest errors may embed the URL; they never contain our headers.
        Error::Transport(e.without_url().to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A Profile as listed by `GET /api/profiles`. Unknown fields are ignored.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// `"running"` or `"stopped"` (kept as a string to tolerate new values).
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub runtime_mode: Option<String>,
    #[serde(default)]
    pub viewer_mode: Option<String>,
    #[serde(default)]
    pub vnc_ws_port: Option<u16>,
    /// Relative path, e.g. `/api/profiles/{id}/cdp`; only set while running.
    #[serde(default)]
    pub cdp_url: Option<String>,
}

impl Profile {
    pub fn is_running(&self) -> bool {
        self.status == "running"
    }
}

/// Result of `POST /api/profiles/{id}/launch`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Launch {
    pub profile_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub runtime_mode: Option<String>,
    #[serde(default)]
    pub viewer_mode: Option<String>,
    #[serde(default)]
    pub vnc_ws_port: Option<u16>,
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default)]
    pub cdp_url: Option<String>,
}

/// Result of `GET /api/profiles/{id}/status`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ProfileStatus {
    pub status: String,
    #[serde(default)]
    pub runtime_mode: Option<String>,
    #[serde(default)]
    pub viewer_mode: Option<String>,
    #[serde(default)]
    pub vnc_ws_port: Option<u16>,
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default)]
    pub cdp_url: Option<String>,
}

impl ProfileStatus {
    pub fn is_running(&self) -> bool {
        self.status == "running"
    }
}

/// Parse a Manager base URL (`http`/`https`, optional path prefix).
fn parse_base(base_url: &str) -> Result<Url> {
    let url =
        Url::parse(base_url).map_err(|e| Error::Transport(format!("invalid manager url: {e}")))?;
    match url.scheme() {
        "http" | "https" if url.host_str().is_some() => Ok(url),
        other => Err(Error::Transport(format!(
            "manager url must be http(s) with a host, got scheme {other:?}"
        ))),
    }
}

/// Build `base + /api/<segments...>` keeping any base path prefix.
fn api_url(base: &Url, segments: &[&str]) -> Url {
    let mut url = base.clone();
    url.set_query(None);
    url.set_fragment(None);
    url.path_segments_mut()
        .expect("http url has path segments")
        .pop_if_empty()
        .push("api")
        .extend(segments);
    url
}

fn profile_url(base: &Url, id: &str, action: &str) -> Url {
    api_url(base, &["profiles", id, action])
}

/// The CDP WebSocket URL of a Profile on a Manager: `ws(s)://host/api/profiles/{id}/cdp`.
/// The scheme follows the base URL (`http` gives `ws`, `https` gives `wss`).
/// The token is not part of the URL; send it as a Bearer header.
pub fn cdp_ws_url(base_url: &str, profile_id: &str) -> Result<String> {
    let base = parse_base(base_url)?;
    let mut url = profile_url(&base, profile_id, "cdp");
    let scheme = if base.scheme() == "https" {
        "wss"
    } else {
        "ws"
    };
    url.set_scheme(scheme)
        .map_err(|_| Error::Transport("cannot set websocket scheme".into()))?;
    Ok(url.into())
}

/// Client for one Manager.
#[derive(Clone)]
pub struct ManagerClient {
    base: Url,
    token: Option<Token>,
    http: reqwest::Client,
}

impl fmt::Debug for ManagerClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManagerClient")
            .field("base", &self.base.as_str())
            .field("token", &self.token) // redacting type
            .finish()
    }
}

impl ManagerClient {
    pub fn new(base_url: &str, token: Option<Token>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(120)) // launch can be slow
            .build()?;
        Ok(Self {
            base: parse_base(base_url)?,
            token,
            http,
        })
    }

    /// CDP WebSocket URL for a Profile on this Manager.
    pub fn cdp_ws_url(&self, profile_id: &str) -> String {
        cdp_ws_url(self.base.as_str(), profile_id).expect("base already validated")
    }

    /// The token to send as a Bearer header when opening the CDP WebSocket.
    pub fn token(&self) -> Option<&Token> {
        self.token.as_ref()
    }

    /// `GET /api/health` (unauthenticated on the Manager).
    pub async fn health(&self) -> Result<()> {
        let url = api_url(&self.base, &["health"]);
        self.send(Method::GET, url, false).await.map(|_| ())
    }

    /// `GET /api/profiles`.
    pub async fn list_profiles(&self) -> Result<Vec<Profile>> {
        let url = api_url(&self.base, &["profiles"]);
        self.json(Method::GET, url, false).await
    }

    /// `GET /api/profiles/{id}/status`.
    pub async fn status(&self, profile_id: &str) -> Result<ProfileStatus> {
        let url = profile_url(&self.base, profile_id, "status");
        self.json(Method::GET, url, false).await
    }

    /// `POST /api/profiles/{id}/launch`. Returns `Error::AlreadyRunning` on 409
    /// (non-fatal: the caller can just connect) and `Error::License` on 402/403.
    pub async fn launch(&self, profile_id: &str) -> Result<Launch> {
        let url = profile_url(&self.base, profile_id, "launch");
        self.json(Method::POST, url, true).await
    }

    /// `POST /api/profiles/{id}/stop`. `Error::NotFound` if it was not running.
    pub async fn stop(&self, profile_id: &str) -> Result<()> {
        let url = profile_url(&self.base, profile_id, "stop");
        self.send(Method::POST, url, false).await.map(|_| ())
    }

    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        is_launch: bool,
    ) -> Result<T> {
        let resp = self.send(method, url, is_launch).await?;
        let status = resp.status();
        let body = resp.bytes().await?;
        serde_json::from_slice(&body).map_err(|e| Error::UnexpectedResponse {
            status: status.as_u16(),
            detail: format!("invalid JSON body: {e}"),
        })
    }

    async fn send(&self, method: Method, url: Url, is_launch: bool) -> Result<reqwest::Response> {
        let mut req = self.http.request(method, url);
        if let Some(t) = &self.token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {}", t.expose()));
        }
        let resp = req.send().await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let detail = error_detail(resp).await;
        Err(match status {
            StatusCode::UNAUTHORIZED => Error::Unauthorized,
            StatusCode::NOT_FOUND => Error::NotFound { detail },
            StatusCode::CONFLICT if is_launch => Error::AlreadyRunning { detail },
            StatusCode::PAYMENT_REQUIRED | StatusCode::FORBIDDEN if is_launch => Error::License {
                status: status.as_u16(),
                detail,
            },
            _ => Error::UnexpectedResponse {
                status: status.as_u16(),
                detail,
            },
        })
    }
}

/// FastAPI errors are `{"detail": <string or object>}`; fall back to raw text.
async fn error_detail(resp: reqwest::Response) -> String {
    let text = resp.text().await.unwrap_or_default();
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(mut m)) => match m.remove("detail") {
            Some(serde_json::Value::String(s)) => s,
            Some(other) => other.to_string(),
            None => text,
        },
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::Path,
        http::{HeaderMap, StatusCode as S},
        routing::{get, post},
        Json, Router,
    };
    use serde_json::json;

    const TOKEN: &str = "s3cret-token-value";

    fn authed(h: &HeaderMap) -> bool {
        h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {TOKEN}"))
    }

    async fn serve(app: Router) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}")
    }

    /// Fake Manager with auth required (except health), like AUTH_TOKEN set.
    async fn spawn() -> String {
        let app = Router::new()
            .route("/api/health", get(|| async { Json(json!({"status":"ok"})) }))
            .route(
                "/api/profiles",
                get(|h: HeaderMap| async move {
                    if !authed(&h) {
                        return (S::UNAUTHORIZED, Json(json!({"detail":"Unauthorized"})));
                    }
                    (
                        S::OK,
                        Json(json!([
                            {"id":"a","name":"Work","status":"running","cdp_url":"/api/profiles/a/cdp","future_field":{"x":1}},
                            {"id":"b","name":"Home","status":"stopped","fingerprint_seed":5}
                        ])),
                    )
                }),
            )
            .route(
                "/api/profiles/{id}/launch",
                post(|h: HeaderMap, Path(id): Path<String>| async move {
                    if !authed(&h) {
                        return (S::UNAUTHORIZED, Json(json!({"detail":"Unauthorized"})));
                    }
                    match id.as_str() {
                        "running" => (S::CONFLICT, Json(json!({"detail":"Profile is already running"}))),
                        "seats" => (S::PAYMENT_REQUIRED, Json(json!({"detail":{"message":"no seats"}}))),
                        "badkey" => (S::FORBIDDEN, Json(json!({"detail":"bad key"}))),
                        "missing" => (S::NOT_FOUND, Json(json!({"detail":"Profile not found"}))),
                        _ => (
                            S::OK,
                            Json(json!({"profile_id":id,"status":"running","runtime_mode":"docker",
                                "viewer_mode":"vnc","vnc_ws_port":6100,"display":":100",
                                "cdp_url":format!("/api/profiles/{id}/cdp"),"extra":true})),
                        ),
                    }
                }),
            )
            .route(
                "/api/profiles/{id}/stop",
                post(|Path(id): Path<String>| async move {
                    if id == "idle" {
                        (S::NOT_FOUND, Json(json!({"detail":"Profile is not running"})))
                    } else {
                        (S::OK, Json(json!({"ok":true})))
                    }
                }),
            )
            .route(
                "/api/profiles/{id}/status",
                get(|Path(_): Path<String>| async {
                    Json(json!({"status":"stopped","runtime_mode":"docker","viewer_mode":"vnc"}))
                }),
            );
        serve(app).await
    }

    fn client(base: &str) -> ManagerClient {
        ManagerClient::new(base, Some(Token::new(TOKEN))).unwrap()
    }

    #[tokio::test]
    async fn lists_profiles_tolerating_unknown_fields() {
        let base = spawn().await;
        let c = client(&base);
        c.health().await.unwrap();
        let ps = c.list_profiles().await.unwrap();
        assert_eq!(ps.len(), 2);
        assert!(ps[0].is_running());
        assert_eq!(ps[0].cdp_url.as_deref(), Some("/api/profiles/a/cdp"));
        assert!(!ps[1].is_running());
    }

    #[tokio::test]
    async fn launch_success() {
        let base = spawn().await;
        let l = client(&base).launch("p1").await.unwrap();
        assert_eq!(l.profile_id, "p1");
        assert_eq!(l.status, "running");
        assert_eq!(l.vnc_ws_port, Some(6100));
        assert_eq!(l.cdp_url.as_deref(), Some("/api/profiles/p1/cdp"));
    }

    #[tokio::test]
    async fn launch_errors() {
        let base = spawn().await;
        let c = client(&base);
        let e = c.launch("running").await.unwrap_err();
        assert!(e.is_already_running(), "{e:?}");
        assert!(matches!(
            c.launch("seats").await,
            Err(Error::License { status: 402, .. })
        ));
        assert!(matches!(
            c.launch("badkey").await,
            Err(Error::License { status: 403, .. })
        ));
        assert!(matches!(
            c.launch("missing").await,
            Err(Error::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn unauthorized_without_or_with_wrong_token() {
        let base = spawn().await;
        let none = ManagerClient::new(&base, None).unwrap();
        assert!(matches!(
            none.list_profiles().await,
            Err(Error::Unauthorized)
        ));
        let wrong = ManagerClient::new(&base, Some(Token::new("nope"))).unwrap();
        assert!(matches!(
            wrong.list_profiles().await,
            Err(Error::Unauthorized)
        ));
        assert!(matches!(wrong.launch("p").await, Err(Error::Unauthorized)));
    }

    #[tokio::test]
    async fn stop_and_status() {
        let base = spawn().await;
        let c = client(&base);
        c.stop("a").await.unwrap();
        assert!(matches!(c.stop("idle").await, Err(Error::NotFound { .. })));
        assert!(!c.status("a").await.unwrap().is_running());
    }

    #[tokio::test]
    async fn transport_error_when_unreachable() {
        let c = ManagerClient::new("http://127.0.0.1:1", None).unwrap();
        assert!(matches!(c.health().await, Err(Error::Transport(_))));
    }

    #[test]
    fn cdp_url_http_and_https() {
        assert_eq!(
            cdp_ws_url("http://host:8080", "abc").unwrap(),
            "ws://host:8080/api/profiles/abc/cdp"
        );
        assert_eq!(
            cdp_ws_url("https://m.example.com/", "abc").unwrap(),
            "wss://m.example.com/api/profiles/abc/cdp"
        );
        assert_eq!(
            cdp_ws_url("https://m.example.com/prefix/?x=1", "a b").unwrap(),
            "wss://m.example.com/prefix/api/profiles/a%20b/cdp"
        );
        assert!(cdp_ws_url("ftp://x", "a").is_err());
        assert!(cdp_ws_url("nonsense", "a").is_err());
    }

    #[test]
    fn token_never_in_debug() {
        let c = client("http://localhost:1");
        let dbg = format!("{c:?} {:?} {:?}", c.token(), Token::new(TOKEN));
        assert!(!dbg.contains(TOKEN), "{dbg}");
        assert!(dbg.contains("redacted"));
    }

    #[tokio::test]
    async fn bad_json_is_unexpected_response() {
        let base = serve(Router::new().route("/api/profiles", get(|| async { "not json" }))).await;
        let c = ManagerClient::new(&base, None).unwrap();
        assert!(matches!(
            c.list_profiles().await,
            Err(Error::UnexpectedResponse { .. })
        ));
    }
}
