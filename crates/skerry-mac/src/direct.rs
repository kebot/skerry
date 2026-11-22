//! Direct Selkies servers, with no SealSkin in front (`ARCH-036`): the address a person
//! opens in a browser, its login (HTTP Basic, a session token, both, or none), the gate that
//! decides whether Skerry can use it, and the `[[selkies]]` entries in config.toml.
//!
//! Server facts are from Selkies `715df7c` (`crates/selkies/src/http/auth.rs`,
//! `http/routes.rs`, `ws/handler.rs`, `web/selkies-web-core/lib/session-token.ts`).
//! Direct requests never carry an `Origin` header: Selkies treats a request without one as
//! same-origin. Redirects are never followed, so credentials never leave the server's base.

use crate::config_store::{self, StartWith, CONFIG_DISPLAY};
use crate::sealskin::RemoteSession;
use crate::selkies::SelkiesClient;
use crate::tls::{self, Pin, Rejected};
use base64::Engine as _;
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, COOKIE, WWW_AUTHENTICATE};
use reqwest::{StatusCode, Url};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

const URL_VARIABLE: &str = "SKERRY_SELKIES_URL";
const USERNAME_VARIABLE: &str = "SKERRY_SELKIES_USERNAME";
const PASSWORD_VARIABLE: &str = "SKERRY_SELKIES_PASSWORD";
const PIN_VARIABLE: &str = "SKERRY_SELKIES_CERT_SHA256";
/// `[[selkies]]` entries kept in config.toml: as many as Recent Servers shows.
pub const MAX_SAVED: usize = 5;
const SELKIES_KEY: &str = "selkies";
const GATE_TIMEOUT: Duration = Duration::from_secs(10);
const SCREENSHOT_MAX_BYTES: usize = 16 * 1024 * 1024;
/// The session-window error when Selkies refuses the login on the WebSocket.
pub const DIRECT_LOGIN_ERROR: &str =
    "Selkies didn’t accept the login. Open the Library to sign in again.";
pub const DIRECT_WEBRTC_ERROR: &str =
    "This server streams over WebRTC now. Skerry needs the WebSockets mode.";

// ---------------------------------------------------------------------------------------
// Addresses

/// A parsed browser link: the base (ending in `/`), and what the link carried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelkiesAddress {
    pub base: Url,
    pub token: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressError {
    Empty,
    Invalid,
    HttpRefused,
    NotPrimary,
    LooksLikeKey,
}

impl std::fmt::Display for AddressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Empty => "Enter the address you open in a browser.",
            Self::Invalid => "That isn’t a web address Skerry can use.",
            Self::HttpRefused => {
                "Use an HTTPS address. Plain HTTP works only for this Mac (localhost)."
            }
            Self::NotPrimary => "Skerry opens the main desktop only. Remove “#shared” (or the display or player part) from the link.",
            Self::LooksLikeKey => "That’s a SealSkin application key.",
        })
    }
}

/// `application/x-www-form-urlencoded` decoding, as `URLSearchParams` does: `+` is a space.
fn form_decode(text: &str) -> String {
    percent_decode_str(&text.replace('+', " "))
        .decode_utf8_lossy()
        .into_owned()
}

fn non_empty(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

pub fn parse_address(raw: &str) -> Result<SelkiesAddress, AddressError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(AddressError::Empty);
    }
    if raw.starts_with("ssk_") || crate::sealskin::Credential::parse(raw).is_ok() {
        return Err(AddressError::LooksLikeKey);
    }
    let text = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let mut url = Url::parse(&text).map_err(|_| AddressError::Invalid)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return Err(AddressError::Invalid);
    }
    // The fragment beats the query, as `session-token.ts::getSessionToken` reads them.
    let mut token = None;
    if let Some(fragment) = url.fragment() {
        for item in fragment.split('&').filter(|item| !item.is_empty()) {
            match item.strip_prefix("token=") {
                Some(value) => token = non_empty(form_decode(value)),
                None => return Err(AddressError::NotPrimary),
            }
        }
    }
    if token.is_none() {
        token = url
            .query_pairs()
            .find(|(key, _)| key == "token")
            .and_then(|(_, value)| non_empty(value.into_owned()));
    }
    let username = non_empty(
        percent_decode_str(url.username())
            .decode_utf8_lossy()
            .into_owned(),
    );
    let password = url
        .password()
        .map(|p| percent_decode_str(p).decode_utf8_lossy().into_owned())
        .and_then(non_empty);
    if url.scheme() == "http" && !is_loopback(&url) {
        return Err(AddressError::HttpRefused);
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(SelkiesAddress {
        base: url,
        token,
        username,
        password,
    })
}

/// Exactly `localhost` (with or without a trailing dot), `127.0.0.0/8` and `::1`. Not
/// `*.localhost` (macOS doesn't promise it resolves to loopback), `0.0.0.0` or IPv4-mapped
/// addresses.
pub fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.strip_suffix('.').unwrap_or(name);
            name.eq_ignore_ascii_case("localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip == std::net::Ipv6Addr::LOCALHOST,
        None => false,
    }
}

// ---------------------------------------------------------------------------------------
// Logins

#[derive(Clone, PartialEq, Eq)]
pub struct SelkiesLogin {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for SelkiesLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SelkiesLogin({}, <password>)", self.username)
    }
}

/// How a direct server's requests authenticate (`ARCH-036` D6).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SelkiesAuth {
    pub basic: Option<SelkiesLogin>,
    pub token: Option<String>,
}

impl std::fmt::Debug for SelkiesAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SelkiesAuth(basic: {:?}, token: {})",
            self.basic,
            self.token.is_some()
        )
    }
}

fn sensitive(text: &str) -> Option<HeaderValue> {
    let mut value = HeaderValue::from_str(text).ok()?;
    value.set_sensitive(true);
    Some(value)
}

impl SelkiesAuth {
    pub fn new(username: &str, password: &str, token: Option<String>) -> Self {
        let basic = (!username.is_empty() || !password.is_empty()).then(|| SelkiesLogin {
            username: username.to_string(),
            password: password.to_string(),
        });
        Self {
            basic,
            token: token.filter(|t| !t.is_empty()),
        }
    }

    fn basic_value(&self) -> Option<HeaderValue> {
        let login = self.basic.as_ref()?;
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", login.username, login.password));
        sensitive(&format!("Basic {encoded}"))
    }

    fn bearer_value(&self) -> Option<HeaderValue> {
        sensitive(&format!("Bearer {}", self.token.as_ref()?))
    }

    /// Basic only: `Authorization: Basic`. Token only: `Authorization: Bearer`. Both:
    /// Basic, and the token in the `selkies_token` cookie, since one header can't carry
    /// both (Selkies reads Bearer, then `?token=`, then the cookie).
    pub fn http_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        match (self.basic_value(), &self.token) {
            (Some(basic), Some(token)) => {
                headers.insert(AUTHORIZATION, basic);
                let cookie = format!(
                    "selkies_token={}",
                    utf8_percent_encode(token, NON_ALPHANUMERIC)
                );
                if let Some(cookie) = sensitive(&cookie) {
                    headers.insert(COOKIE, cookie);
                }
            }
            (Some(basic), None) => {
                headers.insert(AUTHORIZATION, basic);
            }
            (None, Some(_)) => {
                if let Some(bearer) = self.bearer_value() {
                    headers.insert(AUTHORIZATION, bearer);
                }
            }
            (None, None) => {}
        }
        headers
    }

    /// The token alone, as `Authorization: Bearer`: the gate checks it without Basic, which
    /// would otherwise let a bad token pass as a viewer.
    fn token_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(bearer) = self.bearer_value() {
            headers.insert(AUTHORIZATION, bearer);
        }
        headers
    }

    /// The upgrade: Basic when there is a login, and the token as the
    /// `selkies.token.<base64url>` subprotocol offered after `selkies`.
    pub fn websocket_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(basic) = self.basic_value() {
            headers.insert(AUTHORIZATION, basic);
        }
        if let Some(token) = &self.token {
            let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token);
            if let Some(value) = sensitive(&format!("selkies, selkies.token.{encoded}")) {
                headers.insert("sec-websocket-protocol", value);
            }
        }
        headers
    }
}

// ---------------------------------------------------------------------------------------
// Servers

/// Where a direct server came from: an environment server is never saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerSource {
    Typed,
    Saved,
    Environment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Security {
    Https,
    HttpsPinned(Pin),
    LoopbackHttp,
}

#[derive(Clone)]
struct Net {
    http: reqwest::Client,
    tls: Arc<rustls::ClientConfig>,
    rejected: Rejected,
}

/// One direct Selkies server: its base, login and pinned leaf, with the HTTP client and
/// TLS config built once for it.
#[derive(Clone)]
pub struct SelkiesServer {
    base: Url,
    auth: SelkiesAuth,
    pin: Option<Pin>,
    source: ServerSource,
    /// Remember on this Mac: save the password and token with the entry.
    remember: bool,
    net: Net,
}

impl std::fmt::Debug for SelkiesServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SelkiesServer({}, {:?})", self.base, self.auth)
    }
}

impl SelkiesServer {
    pub fn new(
        base: Url,
        auth: SelkiesAuth,
        pin: Option<Pin>,
        source: ServerSource,
        remember: bool,
    ) -> Result<Self, String> {
        let (tls, rejected) = tls::client_config(pin);
        Self::with_tls(base, auth, pin, source, remember, tls, rejected)
    }

    fn with_tls(
        base: Url,
        auth: SelkiesAuth,
        pin: Option<Pin>,
        source: ServerSource,
        remember: bool,
        tls: Arc<rustls::ClientConfig>,
        rejected: Rejected,
    ) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .use_preconfigured_tls((*tls).clone())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(GATE_TIMEOUT)
            .build()
            .map_err(|error| format!("Skerry couldn’t prepare the connection ({error})."))?;
        Ok(Self {
            base,
            auth,
            pin,
            source,
            remember,
            net: Net {
                http,
                tls,
                rejected,
            },
        })
    }

    /// The same server trusting `pin` (a new certificate sheet answer).
    pub fn with_pin(&self, pin: Pin) -> Result<Self, String> {
        Self::new(
            self.base.clone(),
            self.auth.clone(),
            Some(pin),
            self.source,
            self.remember,
        )
    }

    pub fn base(&self) -> &Url {
        &self.base
    }

    pub fn auth(&self) -> &SelkiesAuth {
        &self.auth
    }

    pub fn pin(&self) -> Option<Pin> {
        self.pin
    }

    pub fn source(&self) -> ServerSource {
        self.source
    }

    pub fn remember(&self) -> bool {
        self.remember
    }

    pub fn tls_config(&self) -> Arc<rustls::ClientConfig> {
        self.net.tls.clone()
    }

    pub fn is_http(&self) -> bool {
        self.base.scheme() == "http"
    }

    /// The preferences file's name: `selkies-<host>[_port][-<path slug>-<hash>]`, only
    /// `[A-Za-z0-9._-]`, never a SealSkin server's key (`host[:port]`).
    pub fn prefs_key(&self) -> String {
        prefs_key_of(&self.base)
    }

    /// `host[:port]`, plus the subfolder when there is one.
    pub fn display_name(&self) -> String {
        let host = self.host_port();
        match self.base.path() {
            "/" => host,
            path => format!("{host}{path}"),
        }
    }

    pub fn host(&self) -> String {
        self.base.host_str().unwrap_or_default().to_string()
    }

    pub fn host_port(&self) -> String {
        let host = self.host();
        match self.base.port() {
            Some(port) => format!("{host}:{port}"),
            None => host,
        }
    }

    /// What the card says about the login.
    pub fn login_label(&self) -> String {
        match (&self.auth.basic, &self.auth.token) {
            (Some(login), _) if !login.username.is_empty() => format!("{}@", login.username),
            (_, Some(_)) => "Session token".into(),
            (Some(_), None) => "Password".into(),
            (None, None) => "No login".into(),
        }
    }

    pub fn security(&self) -> Security {
        if self.is_http() {
            Security::LoopbackHttp
        } else if let Some(pin) = self.pin {
            Security::HttpsPinned(pin)
        } else {
            Security::Https
        }
    }

    fn url(&self, route: &str) -> Url {
        // `base` ends in `/`, so every route stays below it.
        self.base.join(route).unwrap_or_else(|_| self.base.clone())
    }

    /// The file and print routes under this server's login (`ARCH-023`, `ARCH-033`).
    pub fn selkies_client(&self) -> SelkiesClient {
        SelkiesClient::direct(
            self.base.clone(),
            self.net.http.clone(),
            self.auth.http_headers(),
        )
    }

    /// The WebSocket upgrade: `wss` (or `ws` on loopback HTTP) to `api/websockets` under
    /// the base, with the login and no `Origin`.
    pub fn websocket_request(
        &self,
    ) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, &'static str> {
        const ERROR: &str = "The Selkies address isn’t valid.";
        let mut url = self.url("api/websockets");
        url.set_scheme(if self.is_http() { "ws" } else { "wss" })
            .map_err(|_| ERROR)?;
        let mut request = url.as_str().into_client_request().map_err(|_| ERROR)?;
        request.headers_mut().extend(self.auth.websocket_headers());
        Ok(request)
    }

    /// The desktop as a session: no storage (`home` empty), no SealSkin URL.
    pub fn as_session(&self) -> RemoteSession {
        RemoteSession {
            session_id: self.session_id(),
            app_id: String::new(),
            app_name: self.display_name(),
            created_at: 0.0,
            session_url: String::new(),
            home: String::new(),
            gpu: false,
            is_collaboration: false,
        }
    }

    pub fn session_id(&self) -> String {
        format!("selkies|{}", self.prefs_key())
    }

    /// The base, never with credentials.
    pub fn web_url(&self) -> Url {
        self.base.clone()
    }

    /// What config.toml keeps for this server.
    pub fn saved(&self) -> SavedServer {
        SavedServer {
            url: self.base.to_string(),
            username: self
                .auth
                .basic
                .as_ref()
                .map(|login| login.username.clone())
                .filter(|name| !name.is_empty()),
            password: self.auth.basic.as_ref().map(|login| login.password.clone()),
            token: self.auth.token.clone(),
            certificate_sha256: self.pin.as_ref().map(tls::fingerprint_text),
        }
    }

    /// A server from a saved entry; `None` when its address no longer parses.
    pub fn from_saved(saved: &SavedServer) -> Option<Self> {
        let address = parse_address(&saved.url).ok()?;
        let auth = SelkiesAuth::new(
            saved.username.as_deref().unwrap_or_default(),
            saved.password.as_deref().unwrap_or_default(),
            saved.token.clone(),
        );
        let pin = saved
            .certificate_sha256
            .as_deref()
            .and_then(tls::parse_fingerprint);
        let remember = saved.password.is_some() || saved.token.is_some();
        Self::new(address.base, auth, pin, ServerSource::Saved, remember).ok()
    }
}

fn safe_name(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn prefs_key_of(base: &Url) -> String {
    let mut key = format!("selkies-{}", safe_name(base.host_str().unwrap_or_default()));
    if let Some(port) = base.port() {
        key.push_str(&format!("_{port}"));
    }
    let slug = base.path().trim_matches('/');
    if !slug.is_empty() {
        let mut slug = safe_name(slug);
        if slug.len() > 80 {
            slug.truncate(80);
        }
        let hash = tls::fingerprint(base.as_str().as_bytes());
        key.push_str(&format!(
            "-{slug}-{:02x}{:02x}{:02x}{:02x}",
            hash[0], hash[1], hash[2], hash[3]
        ));
    }
    key
}

// ---------------------------------------------------------------------------------------
// The gate

/// One page connected to the desktop, from `api/sessions`.
#[derive(Clone, Debug, PartialEq)]
pub struct PageInfo {
    pub id: String,
    pub role: String,
    pub connected_at: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GateReport {
    pub dual_mode: bool,
    pub pages: Vec<PageInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkReason {
    Timeout,
    Refused,
    Dns,
    Tls(String),
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateError {
    Network(NetworkReason),
    /// The roots refused the leaf and nothing is pinned.
    Untrusted(Pin),
    /// A pin is set and the server presented another leaf.
    PinMismatch {
        new: Pin,
        old: Pin,
    },
    NotSelkies,
    SealSkinAddress,
    WebRtc {
        dual_mode: bool,
    },
    /// `current_mode` is null: Selkies is still starting.
    Starting,
    NeedsLogin,
    BadLogin,
    NeedsToken,
    BadToken,
    Server(u16),
}

impl GateError {
    /// Refresh keeps the Library on these (ARCH-009's rule): nothing says the server
    /// changed, only that it didn't answer.
    pub fn keeps_library(&self) -> bool {
        matches!(self, Self::Network(_) | Self::Starting)
            || matches!(self, Self::Server(code) if *code >= 500)
    }

    pub fn message(&self, host: &str) -> String {
        match self {
            Self::Network(reason) => match reason {
                NetworkReason::Timeout => format!("Couldn’t reach {host} (no answer in 10 seconds)."),
                NetworkReason::Refused => format!("Couldn’t reach {host} (connection refused)."),
                NetworkReason::Dns => format!("Couldn’t reach {host} (no server with that name)."),
                NetworkReason::Tls(detail) | NetworkReason::Other(detail) => {
                    format!("Couldn’t reach {host} ({detail}).")
                }
            },
            Self::Untrusted(_) => format!("Skerry can’t verify “{host}”."),
            Self::PinMismatch { .. } => format!("The certificate of “{host}” changed since you trusted it."),
            Self::NotSelkies => "This doesn’t look like a Selkies server: /api/status didn’t answer as Selkies does.".into(),
            Self::SealSkinAddress => "This is a SealSkin session address. Connect with an application key instead.".into(),
            Self::WebRtc { .. } => "This server streams over WebRTC. Skerry needs the WebSockets mode: ask its operator to run Selkies with --mode=websockets.".into(),
            Self::Starting => format!("{host} is still starting. Try again in a moment."),
            Self::NeedsLogin => "This server asks for a username and password.".into(),
            Self::BadLogin => "The username or password is incorrect.".into(),
            Self::NeedsToken => "This server needs a session token. Paste the full link you were given, including #token=….".into(),
            Self::BadToken => "The session token isn’t accepted by this server any more.".into(),
            Self::Server(code) => format!("HTTP {code} from {host}. Try again later."),
        }
    }
}

fn network_reason(error: &reqwest::Error) -> NetworkReason {
    if error.is_timeout() {
        return NetworkReason::Timeout;
    }
    let mut chain = Vec::new();
    let mut source: Option<&dyn std::error::Error> = Some(error);
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            if io.kind() == std::io::ErrorKind::ConnectionRefused {
                return NetworkReason::Refused;
            }
            if io.kind() == std::io::ErrorKind::TimedOut {
                return NetworkReason::Timeout;
            }
        }
        if let Some(tls) = current.downcast_ref::<rustls::Error>() {
            return NetworkReason::Tls(tls.to_string());
        }
        chain.push(current.to_string());
        source = current.source();
    }
    let text = chain.join(": ");
    if text.contains("dns error") || text.contains("failed to lookup address") {
        NetworkReason::Dns
    } else {
        NetworkReason::Other(chain.last().cloned().unwrap_or(text))
    }
}

enum Challenge {
    Basic,
    Bearer,
    Other,
}

fn challenge(headers: &HeaderMap) -> Challenge {
    let value = headers
        .get(WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .trim_start()
        .to_ascii_lowercase();
    if value.starts_with("basic") {
        Challenge::Basic
    } else if value.starts_with("bearer") {
        Challenge::Bearer
    } else {
        Challenge::Other
    }
}

fn sealskin_detail(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .is_some_and(|value| value.get("detail").is_some_and(Value::is_string))
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl SelkiesServer {
    async fn get(&self, route: &str, headers: HeaderMap) -> Result<Answer, GateError> {
        let request = self
            .net
            .http
            .get(self.url(route))
            .headers(headers)
            .timeout(GATE_TIMEOUT)
            .send();
        let response = match request.await {
            Ok(response) => response,
            Err(error) => {
                return Err(match (self.net.rejected.take(), self.pin) {
                    (Some(new), Some(old)) => GateError::PinMismatch { new, old },
                    (Some(leaf), None) => GateError::Untrusted(leaf),
                    (None, _) => GateError::Network(network_reason(&error)),
                })
            }
        };
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().await.unwrap_or_default();
        Ok(Answer {
            status,
            headers,
            body,
        })
    }

    /// A 401 on a credentialed probe, read by scheme and body (`auth.rs`'s challenges).
    fn refusal(&self, answer: &Answer) -> GateError {
        match challenge(&answer.headers) {
            Challenge::Basic if answer.body.contains("Invalid Credentials") => GateError::BadLogin,
            Challenge::Basic if answer.body.contains("Authorization Required") => {
                GateError::NeedsLogin
            }
            Challenge::Basic if self.auth.basic.is_some() => GateError::BadLogin,
            Challenge::Basic => GateError::NeedsLogin,
            Challenge::Bearer if self.auth.token.is_some() => GateError::BadToken,
            Challenge::Bearer => GateError::NeedsToken,
            Challenge::Other if self.auth.basic.is_some() => GateError::BadLogin,
            Challenge::Other => GateError::NeedsLogin,
        }
    }

    fn other_status(answer: &Answer) -> GateError {
        match answer.status.as_u16() {
            403 if sealskin_detail(&answer.body) => GateError::SealSkinAddress,
            code if code >= 500 => GateError::Server(code),
            _ => GateError::NotSelkies,
        }
    }
}

/// Whether Skerry can use `server` (D4/D5 and the review's secure-mode probes):
/// 1. `GET api/status` without credentials (once more with the login if a proxy asks):
///    Selkies in WebSockets mode.
/// 2. `GET api/tokens` without credentials: a Bearer 401 means secure mode, which needs a
///    session token on the WebSocket whatever Basic says.
/// 3. In secure mode, `GET api/sessions` with the token alone: a Basic login would let a
///    bad token pass as a viewer.
/// 4. `GET api/sessions` with the full login: proves it and lists the pages.
///
/// No redirect is followed; a 3xx is "not Selkies".
pub async fn gate(server: &SelkiesServer) -> Result<GateReport, GateError> {
    let mut status = server.get("api/status", HeaderMap::new()).await?;
    if status.status == StatusCode::UNAUTHORIZED {
        let retried = server.get("api/status", server.auth.http_headers()).await?;
        if retried.status == StatusCode::UNAUTHORIZED {
            return Err(server.refusal(&retried));
        }
        status = retried;
    }
    if status.status != StatusCode::OK {
        return Err(SelkiesServer::other_status(&status));
    }
    let value: Value = serde_json::from_str(&status.body).map_err(|_| GateError::NotSelkies)?;
    let modes = value["available_modes"]
        .as_array()
        .ok_or(GateError::NotSelkies)?;
    if !modes.iter().all(Value::is_string) {
        return Err(GateError::NotSelkies);
    }
    let dual_mode = value["enable_dual_mode"].as_bool().unwrap_or(false);
    if !modes.iter().any(|mode| mode == "websockets") {
        return Err(GateError::WebRtc { dual_mode });
    }
    match &value["current_mode"] {
        Value::Null => return Err(GateError::Starting),
        Value::String(mode) if mode == "websockets" => {}
        Value::String(_) => return Err(GateError::WebRtc { dual_mode }),
        _ => return Err(GateError::NotSelkies),
    }

    let tokens = server.get("api/tokens", HeaderMap::new()).await?;
    let secure = tokens.status == StatusCode::UNAUTHORIZED
        && matches!(challenge(&tokens.headers), Challenge::Bearer);
    if secure {
        if server.auth.token.is_none() {
            return Err(GateError::NeedsToken);
        }
        let alone = server
            .get("api/sessions", server.auth.token_headers())
            .await?;
        match alone.status.as_u16() {
            200 => {}
            401 => return Err(GateError::BadToken),
            _ => return Err(SelkiesServer::other_status(&alone)),
        }
    }

    let sessions = server
        .get("api/sessions", server.auth.http_headers())
        .await?;
    match sessions.status.as_u16() {
        200 => {}
        401 => return Err(server.refusal(&sessions)),
        _ => return Err(SelkiesServer::other_status(&sessions)),
    }
    let value: Value = serde_json::from_str(&sessions.body).map_err(|_| GateError::NotSelkies)?;
    let pages = value["sessions"]
        .as_array()
        .ok_or(GateError::NotSelkies)?
        .iter()
        .map(|page| PageInfo {
            id: page["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| page["id"].to_string()),
            role: page["role"].as_str().unwrap_or_default().to_string(),
            connected_at: page["connected_at"].as_f64().unwrap_or_default(),
        })
        .collect();
    Ok(GateReport { dual_mode, pages })
}

/// `GET api/screenshot`: a PNG of at most 16 MB, or nothing. Fetched on Connect, Refresh
/// and when a window closes, never polled; a 503 is not retried.
pub async fn screenshot(server: &SelkiesServer) -> Result<Vec<u8>, ()> {
    let mut response = server
        .net
        .http
        .get(server.url("api/screenshot"))
        .headers(server.auth.http_headers())
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|_| ())?;
    if response.status() != StatusCode::OK
        || response
            .content_length()
            .is_some_and(|length| length as usize > SCREENSHOT_MAX_BYTES)
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if bytes.len() + chunk.len() > SCREENSHOT_MAX_BYTES {
            return Err(());
        }
        bytes.extend_from_slice(&chunk);
    }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(());
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------------------
// Saved servers (`[[selkies]]` in config.toml; ARCH-011 amended by ARCH-036)

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SavedServer {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub token: Option<String>,
    pub certificate_sha256: Option<String>,
}

impl std::fmt::Debug for SavedServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SavedServer({}, user {:?}, password {}, token {}, pin {})",
            self.url,
            self.username,
            self.password.is_some(),
            self.token.is_some(),
            self.certificate_sha256.is_some()
        )
    }
}

impl SavedServer {
    /// The label Recent Servers shows: host[:port] plus the subfolder.
    pub fn label(&self) -> String {
        match parse_address(&self.url) {
            Ok(address) => {
                let base = address.base;
                let host = base.host_str().unwrap_or_default();
                let host = match base.port() {
                    Some(port) => format!("{host}:{port}"),
                    None => host.to_string(),
                };
                match base.path() {
                    "/" => host,
                    path => format!("{host}{path}"),
                }
            }
            Err(_) => self.url.clone(),
        }
    }

    pub fn login_label(&self) -> String {
        match (&self.username, &self.token) {
            (Some(name), _) => format!("{name}@"),
            (None, Some(_)) => "Session token".into(),
            (None, None) if self.password.is_some() => "Password".into(),
            (None, None) => "No login".into(),
        }
    }

    pub fn is_pinned(&self) -> bool {
        self.certificate_sha256.is_some()
    }

    pub fn is_loopback_http(&self) -> bool {
        self.url.starts_with("http://")
    }

    /// Whether connecting needs nothing more typed: a password or token was remembered, or
    /// the server never had a login.
    pub fn ready(&self) -> bool {
        self.password.is_some() || self.token.is_some() || self.username.is_none()
    }
}

fn normalised(url: &str) -> String {
    parse_address(url)
        .map(|address| address.base.to_string())
        .unwrap_or_else(|_| url.trim().to_string())
}

fn entries(document: &DocumentMut) -> Result<Vec<Table>, String> {
    match document.get(SELKIES_KEY) {
        None => Ok(Vec::new()),
        Some(Item::ArrayOfTables(array)) => Ok(array.iter().cloned().collect()),
        Some(Item::Value(toml_edit::Value::Array(array))) => Ok(array
            .iter()
            .filter_map(|value| value.as_inline_table())
            .map(|inline| inline.clone().into_table())
            .collect()),
        Some(_) => Err(format!(
            "`{SELKIES_KEY}` in {CONFIG_DISPLAY} must be a list of [[selkies]] tables."
        )),
    }
}

fn string(table: &Table, key: &str) -> Option<String> {
    table
        .get(key)?
        .as_str()
        .map(str::to_owned)
        .filter(|text| !text.is_empty())
}

fn saved_of(table: &Table) -> Option<SavedServer> {
    Some(SavedServer {
        url: string(table, "url")?,
        username: string(table, "username"),
        password: string(table, "password"),
        token: string(table, "token"),
        certificate_sha256: string(table, "certificate_sha256"),
    })
}

/// The saved servers, most recent first. Entries without a string `url` are skipped; a
/// `selkies` key of another shape is an error, shown on Connect and never overwritten.
pub fn saved_servers_at(path: &Path) -> Result<Vec<SavedServer>, String> {
    let document = config_store::read_config(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidData {
            format!("{CONFIG_DISPLAY} isn’t valid TOML, so the saved servers couldn’t be read.")
        } else {
            format!("Couldn’t read the saved servers in {CONFIG_DISPLAY}.")
        }
    })?;
    let Some(document) = document else {
        return Ok(Vec::new());
    };
    Ok(entries(&document)?.iter().filter_map(saved_of).collect())
}

fn set_or_remove(table: &mut Table, key: &str, value: Option<&str>) {
    match value.filter(|value| !value.is_empty()) {
        Some(value) => {
            table.insert(key, toml_edit::value(value));
        }
        None => {
            table.remove(key);
        }
    }
}

fn write_entries(document: &mut DocumentMut, tables: Vec<Table>) {
    if tables.is_empty() {
        document.remove(SELKIES_KEY);
        return;
    }
    let mut array = ArrayOfTables::new();
    for mut table in tables {
        // Rendered in list order: a kept table's old position would put it back in place.
        table.set_position(None);
        array.push(table);
    }
    document.insert(SELKIES_KEY, Item::ArrayOfTables(array));
}

fn write_error(error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::InvalidData {
        format!("Connected for this run. The server wasn’t saved because {CONFIG_DISPLAY} isn’t valid TOML.")
    } else {
        format!("Connected for this run. Skerry couldn’t save the server to {CONFIG_DISPLAY}.")
    }
}

/// Saves `server` at the front of the list (one entry per address, at most
/// [`MAX_SAVED`]), and `start_with = "selkies"`. With `remember` off, no password or token
/// is written, and any saved before is removed; the address, username and pin always are.
/// Other keys inside the entry are kept. Blocking: only `persist`'s queue calls it.
pub fn save_server_at(path: &Path, server: &SavedServer, remember: bool) -> Result<(), String> {
    let mut failure = None;
    config_store::edit_config(path, |document| {
        let mut tables = match entries(document) {
            Ok(tables) => tables,
            Err(error) => {
                failure = Some(error);
                return false;
            }
        };
        let key = normalised(&server.url);
        let mut table = tables
            .iter()
            .position(|table| string(table, "url").is_some_and(|url| normalised(&url) == key))
            .map(|index| tables.remove(index))
            .unwrap_or_default();
        set_or_remove(&mut table, "url", Some(&key));
        set_or_remove(&mut table, "username", server.username.as_deref());
        let (password, token) = if remember {
            (server.password.as_deref(), server.token.as_deref())
        } else {
            (None, None)
        };
        set_or_remove(&mut table, "password", password);
        set_or_remove(&mut table, "token", token);
        set_or_remove(
            &mut table,
            "certificate_sha256",
            server.certificate_sha256.as_deref(),
        );
        tables.insert(0, table);
        tables.truncate(MAX_SAVED);
        write_entries(document, tables);
        config_store::set_start_with_in(document, Some(StartWith::Selkies));
        true
    })
    .map_err(write_error)?;
    failure.map_or(Ok(()), Err)
}

/// Removes the entry for `url` (and its secrets), and `start_with` with it.
pub fn forget_server_at(path: &Path, url: &str) -> Result<(), String> {
    let failed = || format!("Couldn’t remove the server from {CONFIG_DISPLAY}.");
    let mut failure = false;
    config_store::edit_config(path, |document| {
        let Ok(mut tables) = entries(document) else {
            failure = true;
            return false;
        };
        let key = normalised(url);
        let before = tables.len();
        tables.retain(|table| string(table, "url").is_none_or(|saved| normalised(&saved) != key));
        let removed = tables.len() != before;
        if removed {
            write_entries(document, tables);
        }
        config_store::set_start_with_in(document, None) | removed
    })
    .map_err(|_| failed())?;
    if failure {
        return Err(failed());
    }
    Ok(())
}

/// Removes `start_with` (Disconnect: the entry stays in Recent Servers).
pub fn clear_start_with_at(path: &Path) -> Result<(), String> {
    config_store::edit_config(path, |document| {
        config_store::set_start_with_in(document, None)
    })
    .map_err(|_| format!("Couldn’t update {CONFIG_DISPLAY}."))
}

/// The server in `SKERRY_SELKIES_URL` (+ `_USERNAME`, `_PASSWORD`, `_CERT_SHA256`), never
/// saved. `None` when the variable isn't set.
pub fn environment() -> Option<Result<SelkiesServer, String>> {
    let url = std::env::var(URL_VARIABLE)
        .ok()
        .filter(|url| !url.trim().is_empty())?;
    let var = |name| std::env::var(name).unwrap_or_default();
    Some(environment_server(
        &url,
        &var(USERNAME_VARIABLE),
        &var(PASSWORD_VARIABLE),
        &var(PIN_VARIABLE),
    ))
}

fn environment_server(
    url: &str,
    username: &str,
    password: &str,
    pin: &str,
) -> Result<SelkiesServer, String> {
    let address = parse_address(url).map_err(|error| format!("{URL_VARIABLE}: {error}"))?;
    let pin = match pin.trim() {
        "" => None,
        text => Some(
            tls::parse_fingerprint(text)
                .ok_or_else(|| format!("{PIN_VARIABLE} isn’t a SHA-256 fingerprint."))?,
        ),
    };
    let username = if username.is_empty() {
        address.username.unwrap_or_default()
    } else {
        username.to_string()
    };
    let password = if password.is_empty() {
        address.password.unwrap_or_default()
    } else {
        password.to_string()
    };
    let auth = SelkiesAuth::new(&username, &password, address.token);
    SelkiesServer::new(address.base, auth, pin, ServerSource::Environment, false)
}

/// What startup connects to (`ARCH-036` D7 and the review): a SealSkin key from the
/// environment or `.env` wins over everything; then `SKERRY_SELKIES_URL`; then, with
/// `start_with = "selkies"`, the first saved server; otherwise a saved SealSkin key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Startup {
    SealSkin,
    SelkiesEnvironment,
    SelkiesSaved,
    Nothing,
}

pub fn startup_choice(
    sealskin: Option<crate::sealskin::KeySource>,
    selkies_environment: bool,
    start_with: Option<StartWith>,
    saved_selkies: bool,
) -> Startup {
    use crate::sealskin::KeySource;
    match sealskin {
        Some(KeySource::Environment | KeySource::DotEnv) => Startup::SealSkin,
        _ if selkies_environment => Startup::SelkiesEnvironment,
        _ if start_with == Some(StartWith::Selkies) && saved_selkies => Startup::SelkiesSaved,
        Some(_) => Startup::SealSkin,
        None => Startup::Nothing,
    }
}

#[cfg(test)]
mod tests;
