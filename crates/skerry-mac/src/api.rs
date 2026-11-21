use base64::Engine as _;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::{Client, Method, RequestBuilder, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const KEY_VARIABLE: &str = "SEALSKIN_APPLICATION_KEY";
const KEYCHAIN_SERVICE: &str = "dev.kebot.sealskin";
const KEYCHAIN_ACCOUNT: &str = "application-key";

/// The oldest SealSkin server version Skerry supports (ARCH-009). Older servers are refused.
pub const MINIMUM_SEALSKIN_VERSION: (u64, u64, u64) = (0, 5, 0);

pub struct Credential {
    raw: String,
    base: Url,
    authorization: HeaderValue,
}

impl Credential {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let (key, address) = raw.rsplit_once('@').ok_or_else(|| {
            "Paste the full application key, including the server address after @.".to_string()
        })?;
        let key_format = key.strip_prefix("ssk_").and_then(|key| key.split_once('_'));
        if !key_format.is_some_and(|(id, secret)| !id.is_empty() && !secret.is_empty())
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err("That doesn’t look like a SealSkin application key.".into());
        }
        let address = if address.contains("://") {
            address.to_string()
        } else {
            format!("https://{address}")
        };
        let base = Url::parse(&address).map_err(|_| "The key has an invalid server address.")?;
        if base.scheme() != "https"
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.path() != "/"
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err("The key must name a SealSkin server with an HTTPS address.".into());
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| "The key contains invalid characters.")?;
        authorization.set_sensitive(true);
        Ok(Self {
            raw: raw.into(),
            base,
            authorization,
        })
    }

    pub fn startup() -> Result<Option<Self>, String> {
        if let Ok(value) = std::env::var(KEY_VARIABLE) {
            return Self::parse(&value).map(Some);
        }
        if let Ok(entries) = dotenvy::from_path_iter(".env") {
            for (name, value) in entries.flatten() {
                if name == KEY_VARIABLE {
                    return Self::parse(&value).map(Some);
                }
            }
        }
        match security_framework::passwords::get_generic_password(
            KEYCHAIN_SERVICE,
            KEYCHAIN_ACCOUNT,
        ) {
            Ok(bytes) => Self::parse(&String::from_utf8_lossy(&bytes)).map(Some),
            Err(error) if error.code() == -25300 => Ok(None), // errSecItemNotFound
            Err(_) => Err("Couldn’t read the saved application key from Keychain.".into()),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            KEYCHAIN_ACCOUNT,
            self.raw.as_bytes(),
        )
        .map_err(|_| "Connected for this run. Keychain couldn’t save the key.".into())
    }

    pub fn forget() -> Result<(), String> {
        match security_framework::passwords::delete_generic_password(
            KEYCHAIN_SERVICE,
            KEYCHAIN_ACCOUNT,
        ) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == -25300 => Ok(()),
            Err(_) => Err("Couldn’t remove the saved application key from Keychain.".into()),
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct RemoteApp {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub logo: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub home_directories: bool,
    #[serde(default)]
    pub is_meta_app: bool,
}

#[derive(Clone, Deserialize)]
pub struct RemoteSession {
    pub session_id: String,
    #[serde(default)]
    pub app_id: String,
    pub app_name: String,
    #[serde(default)]
    pub created_at: f64,
    pub session_url: String,
    #[serde(default)]
    pub home: String,
    #[serde(default)]
    pub gpu: bool,
}

#[derive(Clone, Debug)]
pub struct LaunchParams {
    pub home_name: Option<String>,
    pub language: Option<String>,
    pub wayland_mode: bool,
    pub launch_in_room_mode: bool,
}

impl Default for LaunchParams {
    fn default() -> Self {
        Self {
            home_name: None,
            language: None,
            wayland_mode: true,
            launch_in_room_mode: false,
        }
    }
}

#[derive(Deserialize)]
struct AppIconResponse {
    icon_data_b64: String,
}

#[derive(Deserialize)]
struct ScreenshotResponse {
    image: String,
}

#[derive(Deserialize)]
struct Launched {
    session_id: String,
    session_url: String,
}

#[derive(Deserialize)]
struct ServerVersion {
    version: String,
}

#[derive(Deserialize)]
struct Failure {
    detail: String,
}

pub struct Library {
    pub apps: Vec<RemoteApp>,
    pub sessions: Vec<RemoteSession>,
}

#[derive(Clone)]
pub struct Api {
    client: Client,
    base: Url,
}

impl Api {
    pub fn new(credential: &Credential) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, credential.authorization.clone());
        headers.insert("x-sealskin-app", HeaderValue::from_static("other"));
        let client = Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| "Couldn’t initialize the secure connection.")?;
        Ok(Self {
            client,
            base: credential.base.clone(),
        })
    }

    pub fn server(&self) -> &str {
        self.base
            .as_str()
            .trim_start_matches("https://")
            .trim_end_matches('/')
    }

    pub fn session_url(&self, path: &str) -> Result<Url, String> {
        let url = self
            .base
            .join(path)
            .map_err(|_| "The server returned an invalid session address.")?;
        if url.origin() != self.base.origin()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(
                "The server returned a session address outside this SealSkin server.".into(),
            );
        }
        Ok(url)
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let mut url = self.base.clone();
        url.set_path(path);
        self.client.request(method, url)
    }

    /// Refuses any SealSkin server whose `GET /companion/api/ui/version` is older than
    /// [`MINIMUM_SEALSKIN_VERSION`] or unreadable. Servers older than 0.5.0 have no such route.
    pub async fn check_version(&self) -> Result<(), String> {
        let response = self
            .request(Method::GET, "/companion/api/ui/version")
            .send()
            .await
            .map_err(|_| {
                "Couldn’t reach the SealSkin server. Check the address, network, and HTTPS certificate."
                    .to_string()
            })?;
        if !response.status().is_success() {
            return Err(unsupported_version(None));
        }
        let server: ServerVersion = response
            .json()
            .await
            .map_err(|_| unsupported_version(None))?;
        supported_version(&server.version)
    }

    pub async fn library(&self) -> Result<Library, String> {
        self.check_version().await?;
        let apps = send(
            self.request(Method::POST, "/companion/api/applications")
                .json(&json!({})),
        );
        let sessions = send(self.request(Method::GET, "/companion/api/sessions"));
        let (apps, sessions) = tokio::try_join!(apps, sessions)?;
        Ok(Library { apps, sessions })
    }

    pub async fn launch_with_options(
        &self,
        app: RemoteApp,
        params: LaunchParams,
    ) -> Result<RemoteSession, String> {
        let mut body = serde_json::Map::new();
        body.insert("application_id".into(), json!(app.id));
        if let Some(home_name) = params.home_name {
            body.insert("home_name".into(), json!(home_name));
        }
        if let Some(language) = params.language {
            body.insert("language".into(), json!(language));
        }
        body.insert("wayland_mode".into(), json!(params.wayland_mode));
        body.insert(
            "launch_in_room_mode".into(),
            json!(params.launch_in_room_mode),
        );
        let response: Launched = send(
            self.request(Method::POST, "/companion/api/launch/simple")
                .timeout(Duration::from_secs(180))
                .json(&serde_json::Value::Object(body)),
        )
        .await?;
        Ok(RemoteSession {
            session_id: response.session_id,
            app_id: app.id,
            app_name: app.name,
            created_at: 0.0,
            session_url: response.session_url,
            home: String::new(),
            gpu: false,
        })
    }

    pub async fn stop_session(&self, session_id: &str) -> Result<(), String> {
        if !valid_path_segment(session_id) {
            return Err("Invalid session identifier.".into());
        }
        let path = format!("/companion/api/sessions/{session_id}");
        let response = self
            .request(Method::DELETE, &path)
            .send()
            .await
            .map_err(|_| "Couldn’t reach the SealSkin server to stop the session.".to_string())?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(format!(
            "SealSkin returned HTTP {} while stopping the session.",
            response.status().as_u16()
        ))
    }

    /// Fetches an application's icon strictly from the SealSkin server's own origin.
    pub async fn app_icon(&self, app: &RemoteApp) -> Option<Vec<u8>> {
        if !valid_path_segment(&app.id) {
            return None;
        }
        if app.is_meta_app {
            let path = format!("/companion/api/app_icon/{}", app.id);
            if let Ok(resp) = send::<AppIconResponse>(self.request(Method::GET, &path)).await {
                if let Ok(bytes) =
                    base64::engine::general_purpose::STANDARD.decode(resp.icon_data_b64.trim())
                {
                    if !bytes.is_empty() {
                        return Some(bytes);
                    }
                }
            }
        }
        if app.logo.starts_with('/') {
            if let Ok(url) = self.same_origin_url(&app.logo) {
                if let Some(bytes) = self.fetch_bytes(url).await {
                    return Some(bytes);
                }
            }
        }
        let fallback = format!("/app/{}/icon.png", app.id);
        let url = self.same_origin_url(&fallback).ok()?;
        self.fetch_bytes(url).await
    }

    /// Fetches the cached JPEG thumbnail for a running session.
    pub async fn session_thumbnail(&self, session_id: &str) -> Option<Vec<u8>> {
        if !valid_path_segment(session_id) {
            return None;
        }
        let path = format!("/companion/api/sessions/{session_id}/screenshot");
        let resp: ScreenshotResponse = send(self.request(Method::GET, &path)).await.ok()?;
        decode_data_uri(&resp.image)
    }

    pub fn app_deep_link(&self, app_id: &str) -> Option<String> {
        if !valid_path_segment(app_id) {
            return None;
        }
        let mut url = self.base.clone();
        url.set_path(&format!("/app/{app_id}/"));
        Some(url.to_string())
    }

    fn same_origin_url(&self, path: &str) -> Result<Url, String> {
        let url = self
            .base
            .join(path)
            .map_err(|_| "Invalid server resource path.".to_string())?;
        if url.origin() != self.base.origin()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("Resource URL is outside the SealSkin server origin.".into());
        }
        Ok(url)
    }

    async fn fetch_bytes(&self, url: Url) -> Option<Vec<u8>> {
        let response = self.client.get(url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let bytes = response.bytes().await.ok()?;
        (!bytes.is_empty()).then(|| bytes.to_vec())
    }
}

fn valid_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn decode_data_uri(uri: &str) -> Option<Vec<u8>> {
    let encoded = uri
        .trim()
        .split_once(";base64,")
        .map_or(uri.trim(), |(_, data)| data);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    (!bytes.is_empty()).then_some(bytes)
}

/// Parses `MAJOR.MINOR.PATCH[-pre][+build]` and reports whether a pre-release tag is present.
fn parse_version(version: &str) -> Option<((u64, u64, u64), bool)> {
    let version = version.trim();
    let version = version.split_once('+').map_or(version, |(core, _)| core);
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, !pre.is_empty()),
        None => (version, false),
    };
    let mut parts = core.split('.').map(|part| {
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u64>().ok())
            .flatten()
    });
    let parsed = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some((parsed, pre))
}

fn supported_version(version: &str) -> Result<(), String> {
    match parse_version(version) {
        // A pre-release of the minimum (e.g. `0.5.0-rc1`) precedes it, per SemVer.
        Some((core, pre))
            if core > MINIMUM_SEALSKIN_VERSION || (core == MINIMUM_SEALSKIN_VERSION && !pre) =>
        {
            Ok(())
        }
        _ => Err(unsupported_version(Some(version.trim()))),
    }
}

fn minimum_version() -> String {
    let (major, minor, patch) = MINIMUM_SEALSKIN_VERSION;
    format!("{major}.{minor}.{patch}")
}

fn unsupported_version(found: Option<&str>) -> String {
    let minimum = minimum_version();
    match found {
        Some(found) => format!(
            "Skerry requires SealSkin {minimum} or newer, but this server runs SealSkin {found}."
        ),
        None => format!(
            "Skerry requires SealSkin {minimum} or newer. This server is older or isn’t SealSkin."
        ),
    }
}

async fn send<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, String> {
    let response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            "The server took too long to respond. Refresh to check for running sessions."
        } else {
            "Couldn’t reach the SealSkin server. Check the address, network, and HTTPS certificate."
        }
    })?;
    let status = response.status();
    if status.is_success() {
        return response
            .json()
            .await
            .map_err(|_| "The server returned an unexpected response.".into());
    }
    let detail = response
        .json::<Failure>()
        .await
        .ok()
        .map(|failure| failure.detail);
    let error = match (status, detail.as_deref()) {
        (StatusCode::UNAUTHORIZED, Some("client")) => {
            "The account must allow the Other client type for application keys."
        }
        (StatusCode::UNAUTHORIZED, Some("expired")) => {
            "This application key has expired. Create a new key in SealSkin."
        }
        (StatusCode::UNAUTHORIZED, Some("disabled")) => {
            "Application keys are disabled for this account."
        }
        (StatusCode::UNAUTHORIZED, Some("dormant")) => {
            "This key is dormant. Sign in to the SealSkin web app before retrying."
        }
        (StatusCode::UNAUTHORIZED, _) => "The server didn’t accept this application key.",
        (StatusCode::FORBIDDEN, _) => "This account doesn’t have permission for that action.",
        _ => return Err(format!("SealSkin returned HTTP {}.", status.as_u16())),
    };
    Err(error.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_key_requires_a_trusted_https_origin() {
        let credential = Credential::parse("ssk_id_secret@seal.example:8443").unwrap();
        assert_eq!(credential.base.as_str(), "https://seal.example:8443/");
        assert!(credential.authorization.is_sensitive());
        for input in [
            "ssk_id_secret",
            "ssk_id_secret@http://seal.example",
            "ssk_id_secret@seal.example/path",
        ] {
            assert!(Credential::parse(input).is_err());
        }
    }

    #[test]
    fn sealskin_0_5_0_or_newer_is_supported() {
        assert_eq!(minimum_version(), "0.5.0");
        for version in [
            "0.5.0",
            " 0.5.0\n",
            "0.5.1",
            "0.6.0",
            "1.0.0",
            "0.10.0",
            "0.5.1-rc1",
            "0.5.0+build7",
        ] {
            assert!(supported_version(version).is_ok(), "{version}");
        }
        for version in [
            "0.4.2",
            "0.4.99",
            "0.5.0-rc1",
            "v0.5.0",
            "0.5",
            "0.5.0.1",
            "",
            "x.y.z",
        ] {
            let error = supported_version(version).unwrap_err();
            assert!(
                error.contains("requires SealSkin 0.5.0 or newer"),
                "{error}"
            );
        }
    }

    #[test]
    fn session_urls_stay_within_the_sealskin_origin() {
        let credential = Credential::parse("ssk_id_secret@seal.example:8443").unwrap();
        let api = Api::new(&credential).unwrap();
        let session = api.session_url("/session/?access_token=mock").unwrap();
        assert_eq!(
            session.as_str(),
            "https://seal.example:8443/session/?access_token=mock"
        );
        for path in [
            "//other.example/session/",
            "https://seal.example/session/",
            "http://seal.example:8443/session/",
        ] {
            assert!(api.session_url(path).is_err());
        }
    }

    #[test]
    fn decodes_sealskin_0_5_application_session_and_data_uri() {
        let app: RemoteApp = serde_json::from_value(json!({
            "id": "zen",
            "name": "Zen",
            "logo": "/api/logos/zen.png",
            "type": "browser",
            "home_directories": true,
            "nvidia_support": true,
            "dri3_support": true,
            "url_support": true,
            "extensions": [".html"],
            "is_meta_app": false
        }))
        .unwrap();
        assert_eq!(app.kind, "browser");
        assert!(app.home_directories);

        let session: RemoteSession = serde_json::from_value(json!({
            "session_id": "sess-1",
            "app_id": "zen",
            "app_name": "Zen",
            "app_logo": "/api/logos/zen.png",
            "created_at": 1700000000.0,
            "session_url": "/sess-1/?access_token=tok",
            "home": "auto-zen",
            "gpu": true,
            "wayland_mode": true
        }))
        .unwrap();
        assert_eq!(session.home, "auto-zen");
        assert!(session.gpu);

        let bytes = decode_data_uri("data:image/jpeg;base64,AQID").unwrap();
        assert_eq!(bytes, vec![1, 2, 3]);
    }
}
