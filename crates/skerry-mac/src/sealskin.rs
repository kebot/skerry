use base64::Engine as _;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::{Client, Method, RequestBuilder, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

const KEY_VARIABLE: &str = "SEALSKIN_APPLICATION_KEY";

/// Why a Library load failed. `Refused` means the server failed the minimum-version gate
/// (ARCH-009): Skerry must not keep using that server, even on Refresh.
#[derive(Debug)]
pub enum LibraryError {
    Refused(String),
    Failed(String),
}

/// The oldest SealSkin server version Skerry supports (ARCH-009). Older servers are refused.
pub const MINIMUM_SEALSKIN_VERSION: (u64, u64, u64) = (0, 5, 0);

#[derive(Clone)]
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

    /// Loads the key from `SEALSKIN_APPLICATION_KEY`, then `.env`, then `application_key` in
    /// `~/.config/skerry/config.toml`. Skerry never reads or writes the macOS Keychain
    /// (ARCH-011).
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
        read_saved_key(&config_file()?)?
            .map(|raw| Self::parse(&raw))
            .transpose()
    }

    pub fn save(&self) -> Result<(), String> {
        write_saved_key(&config_file()?, Some(&self.raw)).map_err(|error| {
            if error.kind() == io::ErrorKind::InvalidData {
                format!(
                    "Connected for this run. The key wasn’t saved because {CONFIG_DISPLAY} isn’t valid TOML. Fix or delete that file, then connect again."
                )
            } else {
                format!("Connected for this run. Skerry couldn’t save the key to {CONFIG_DISPLAY}.")
            }
        })
    }

    pub fn forget() -> Result<(), String> {
        write_saved_key(&config_file()?, None).map_err(|_| {
            format!("Couldn’t remove the saved application key from {CONFIG_DISPLAY}.")
        })
    }
}

const CONFIG_DISPLAY: &str = "~/.config/skerry/config.toml";
const CONFIG_KEY: &str = "application_key";

fn config_file() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .ok_or("Skerry can’t find your home folder to keep the application key.")?;
    Ok(config_file_in(Path::new(&home)))
}

fn config_file_in(home: &Path) -> PathBuf {
    home.join(".config/skerry/config.toml")
}

fn read_config(path: &Path) -> io::Result<Option<toml_edit::DocumentMut>> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse()
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_saved_key(path: &Path) -> Result<Option<String>, String> {
    let document = read_config(path)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::InvalidData {
                format!("{CONFIG_DISPLAY} isn’t valid TOML, so the saved key couldn’t be read. Fix or delete that file, or paste a key to connect for this run.")
            } else {
                format!("Couldn’t read the saved application key in {CONFIG_DISPLAY}.")
            }
        })?;
    let Some(document) = document else {
        return Ok(None);
    };
    match document.get(CONFIG_KEY) {
        None => Ok(None),
        Some(item) => match item.as_str().map(str::trim) {
            Some("") => Ok(None),
            Some(raw) => Ok(Some(raw.to_string())),
            None => Err(format!(
                "`{CONFIG_KEY}` in {CONFIG_DISPLAY} must be a quoted string."
            )),
        },
    }
}

/// Sets (`Some`) or removes (`None`) `application_key`, keeping every other setting and
/// comment in the file. The folder is private to the user (0700) and the file is replaced
/// atomically by a temporary file created 0600 before any secret bytes are written.
fn write_saved_key(path: &Path, raw: Option<&str>) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    let existing = read_config(path)?;
    if raw.is_none() && existing.is_none() {
        return Ok(());
    }
    let mut document = existing.unwrap_or_default();
    match raw {
        Some(raw) => document[CONFIG_KEY] = toml_edit::value(raw.trim()),
        None => {
            if document.remove(CONFIG_KEY).is_none() {
                return Ok(());
            }
        }
    }
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("no config folder"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let mut file = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o600))
        .tempfile_in(dir)?;
    file.write_all(document.to_string().as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
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
    /// The app runs on NVIDIA GPUs.
    #[serde(default)]
    pub nvidia_support: bool,
    /// The app runs on Intel/AMD (Mesa, DRI3) GPUs.
    #[serde(default)]
    pub dri3_support: bool,
}

/// A GPU the server offers sessions: a render node on Docker (`/dev/dri/renderD128`), a
/// PodTemplate or resource name on Kubernetes (`intel-gpu`).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct GpuInfo {
    pub device: String,
    #[serde(default)]
    pub driver: String,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct VpnEndpoint {
    pub id: String,
    pub label: String,
}

/// A VPN profile open to the user; its first endpoint is the default.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct VpnChoice {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub endpoints: Vec<VpnEndpoint>,
}

/// The launch rights in the user's effective settings.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct UserSettings {
    #[serde(default)]
    pub persistent_storage: bool,
    #[serde(default)]
    pub gpu: bool,
    #[serde(default)]
    pub vpn: bool,
}

/// What `POST /companion/api/admin/status` says about the user's launch choices.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServerStatus {
    #[serde(default)]
    pub settings: UserSettings,
    #[serde(default)]
    pub gpus: Vec<GpuInfo>,
    #[serde(default)]
    pub vpns: Vec<VpnChoice>,
}

#[derive(Deserialize)]
struct HomeDirectories {
    #[serde(default)]
    home_dirs: Vec<String>,
}

/// Pages of a storage listing to read at most (200 names each).
const STORAGE_LIST_MAX_PAGES: u32 = 50;

#[derive(Deserialize)]
struct StorageItem {
    name: String,
}

#[derive(Deserialize)]
struct StorageListing {
    items: Vec<StorageItem>,
    #[serde(default)]
    total: u64,
}

#[derive(Deserialize)]
struct DeleteTask {
    task_id: String,
}

#[derive(Deserialize)]
struct DeleteStatus {
    status: String,
    #[serde(default)]
    message: Option<String>,
}

/// Whether a session's home is persistent storage SealSkin can reach (not Cleanroom).
pub fn has_storage(home: &str) -> bool {
    !home.is_empty() && home != "cleanroom" && valid_path_segment(home)
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
    /// A named home, `cleanroom`, or `None` (only a meta app's own Auto home).
    pub home_name: Option<String>,
    /// Create `home_name` first when the server doesn't have it yet (the Auto home).
    pub create_home: bool,
    pub language: Option<String>,
    pub timezone: Option<String>,
    pub selected_gpu: Option<String>,
    pub vpn: Option<String>,
    pub vpn_endpoint: Option<String>,
    pub wayland_mode: bool,
    pub launch_in_room_mode: bool,
}

impl Default for LaunchParams {
    fn default() -> Self {
        Self {
            home_name: None,
            create_home: false,
            language: None,
            timezone: None,
            selected_gpu: None,
            vpn: None,
            vpn_endpoint: None,
            wayland_mode: true,
            launch_in_room_mode: false,
        }
    }
}

impl LaunchParams {
    /// The `/launch/simple` body, as SealSkin's web launcher sends it.
    fn body(&self, app_id: &str) -> serde_json::Value {
        let mut body = serde_json::Map::new();
        body.insert("application_id".into(), json!(app_id));
        let optional = [
            ("home_name", &self.home_name),
            ("language", &self.language),
            ("timezone", &self.timezone),
            ("selected_gpu", &self.selected_gpu),
            ("vpn", &self.vpn),
        ];
        for (name, value) in optional {
            if let Some(value) = value {
                body.insert(name.into(), json!(value));
            }
        }
        if let (Some(_), Some(endpoint)) = (&self.vpn, &self.vpn_endpoint) {
            body.insert("vpn_endpoint".into(), json!(endpoint));
        }
        body.insert("wayland_mode".into(), json!(self.wayland_mode));
        body.insert(
            "launch_in_room_mode".into(),
            json!(self.launch_in_room_mode),
        );
        serde_json::Value::Object(body)
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
    /// The user's launch rights, GPUs, and VPNs; default (nothing offered) when unreadable.
    pub status: ServerStatus,
    /// The user's home directories, when they have persistent storage.
    pub homes: Vec<String>,
}

#[derive(Clone)]
pub struct SealSkinClient {
    client: Client,
    base: Url,
}

impl SealSkinClient {
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
    pub async fn check_version(&self) -> Result<(), LibraryError> {
        let response = self
            .request(Method::GET, "/companion/api/ui/version")
            .send()
            .await
            .map_err(|_| {
                LibraryError::Failed(
                    "Couldn’t reach the SealSkin server. Check the address, network, and HTTPS certificate."
                        .to_string(),
                )
            })?;
        if !response.status().is_success() {
            return Err(LibraryError::Refused(unsupported_version(None)));
        }
        let server: ServerVersion = response
            .json()
            .await
            .map_err(|_| LibraryError::Refused(unsupported_version(None)))?;
        supported_version(&server.version).map_err(LibraryError::Refused)
    }

    pub async fn library(&self) -> Result<Library, LibraryError> {
        self.check_version().await?;
        let apps = send(
            self.request(Method::POST, "/companion/api/applications")
                .json(&json!({})),
        );
        let sessions = send(self.request(Method::GET, "/companion/api/sessions"));
        let status = send::<ServerStatus>(
            self.request(Method::POST, "/companion/api/admin/status")
                .json(&json!({})),
        );
        let (apps, sessions, status) = tokio::join!(apps, sessions, status);
        let (apps, sessions) = (
            apps.map_err(LibraryError::Failed)?,
            sessions.map_err(LibraryError::Failed)?,
        );
        // Launch choices are extras: without the status the dialog offers what it always did.
        let status = status.unwrap_or_default();
        let homes = if status.settings.persistent_storage {
            self.home_directories().await.unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(Library {
            apps,
            sessions,
            status,
            homes,
        })
    }

    async fn home_directories(&self) -> Result<Vec<String>, String> {
        let homes: HomeDirectories =
            send(self.request(Method::GET, "/companion/api/homedirs")).await?;
        Ok(homes.home_dirs)
    }

    async fn create_home_directory(&self, home_name: &str) -> Result<(), String> {
        let response = self
            .request(Method::POST, "/companion/api/homedirs")
            .json(&json!({ "home_name": home_name }))
            .send()
            .await
            .map_err(|_| "Couldn’t reach the SealSkin server to create the home.".to_string())?;
        // 409: it exists already (made elsewhere since the Library loaded).
        if response.status().is_success() || response.status() == StatusCode::CONFLICT {
            return Ok(());
        }
        Err(format!(
            "SealSkin returned HTTP {} while creating the home “{home_name}”.",
            response.status().as_u16()
        ))
    }

    pub async fn launch_with_options(
        &self,
        app: RemoteApp,
        params: LaunchParams,
    ) -> Result<RemoteSession, String> {
        if let (true, Some(home_name)) = (params.create_home, &params.home_name) {
            self.create_home_directory(home_name).await?;
        }
        let response: Launched = send(
            self.request(Method::POST, "/companion/api/launch/simple")
                .timeout(Duration::from_secs(180))
                .json(&params.body(&app.id)),
        )
        .await?;
        Ok(RemoteSession {
            session_id: response.session_id,
            app_id: app.id,
            app_name: app.name,
            created_at: 0.0,
            session_url: response.session_url,
            // The server lists a cleanroom session with no home.
            home: params
                .home_name
                .filter(|home| home != "cleanroom")
                .unwrap_or_default(),
            gpu: params.selected_gpu.is_some(),
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

    /// The names in one folder of a home's persistent storage, every page of it
    /// (`GET /companion/api/files/list/<home>`, `ARCH-024`).
    pub async fn storage_names(&self, home: &str, path: &str) -> Result<Vec<String>, String> {
        if !valid_path_segment(home) {
            return Err("Invalid home name.".into());
        }
        let route = format!("/companion/api/files/list/{home}");
        let mut names = Vec::new();
        for page in 1..=STORAGE_LIST_MAX_PAGES {
            let listing: StorageListing = send(self.request(Method::GET, &route).query(&[
                ("path", path),
                ("page", &page.to_string()),
                ("per_page", "200"),
            ]))
            .await?;
            let count = listing.items.len();
            names.extend(listing.items.into_iter().map(|item| item.name));
            if count == 0 || names.len() as u64 >= listing.total {
                return Ok(names);
            }
        }
        Err("The folder holds too many items to compare.".into())
    }

    /// Deletes `paths` (home-relative, `/`-rooted) from a home's persistent storage and waits
    /// for SealSkin's background deletion to finish (`POST /companion/api/files/delete/<home>`,
    /// then `GET …/delete_status/<task>`, `ARCH-024`).
    pub async fn delete_storage(&self, home: &str, paths: &[String]) -> Result<String, String> {
        if !valid_path_segment(home) {
            return Err("Invalid home name.".into());
        }
        let task: DeleteTask = send(
            self.request(Method::POST, &format!("/companion/api/files/delete/{home}"))
                .json(&json!({ "paths": paths })),
        )
        .await?;
        if !valid_path_segment(&task.task_id) {
            return Err("SealSkin returned an invalid deletion task.".into());
        }
        let route = format!("/companion/api/files/delete_status/{}", task.task_id);
        let deadline = std::time::Instant::now() + Duration::from_secs(600);
        loop {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let status: DeleteStatus = send(self.request(Method::GET, &route)).await?;
            match status.status.as_str() {
                "completed" => return Ok(status.message.unwrap_or_default()),
                "error" => {
                    return Err(status
                        .message
                        .unwrap_or_else(|| "SealSkin couldn’t delete the items.".into()))
                }
                _ if std::time::Instant::now() > deadline => {
                    return Err("SealSkin is still deleting; refresh later to see.".into())
                }
                _ => {}
            }
        }
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

    /// The app's address on the server, `/app/<id>/`, with `query` (the launch options, as
    /// the web launcher's Copy link writes them).
    pub fn app_deep_link(&self, app_id: &str, query: &[(&str, String)]) -> Option<String> {
        if !valid_path_segment(app_id) {
            return None;
        }
        let mut url = self.base.clone();
        url.set_path(&format!("/app/{app_id}/"));
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        }
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
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn saved_key_round_trips_through_a_private_config_toml() {
        let home = tempfile::tempdir().unwrap();
        let path = config_file_in(home.path());
        assert_eq!(path, home.path().join(".config/skerry/config.toml"));
        assert_eq!(read_saved_key(&path).unwrap(), None);
        write_saved_key(&path, None).unwrap();
        assert!(
            !path.exists(),
            "forgetting with no config must not create one"
        );

        write_saved_key(&path, Some(" ssk_id_secret@seal.example ")).unwrap();
        assert_eq!(
            read_saved_key(&path).unwrap().as_deref(),
            Some("ssk_id_secret@seal.example")
        );
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);

        write_saved_key(&path, None).unwrap();
        assert_eq!(read_saved_key(&path).unwrap(), None);
    }

    #[test]
    fn saving_the_key_keeps_other_settings_and_comments() {
        let home = tempfile::tempdir().unwrap();
        let path = config_file_in(home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "# my notes\ntheme = \"dark\"\n").unwrap();
        write_saved_key(&path, Some("ssk_id_secret@seal.example")).unwrap();
        write_saved_key(&path, None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my notes") && text.contains("theme = \"dark\""));
        assert!(!text.contains("application_key"));
    }

    #[test]
    fn malformed_saved_keys_are_reported_not_ignored() {
        let home = tempfile::tempdir().unwrap();
        let path = config_file_in(home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "application_key = 42\n").unwrap();
        assert!(read_saved_key(&path).is_err());
        std::fs::write(&path, "application_key = [unterminated\n").unwrap();
        assert!(read_saved_key(&path).is_err());
    }

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
        let api = SealSkinClient::new(&credential).unwrap();
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
        assert!(app.nvidia_support && app.dri3_support);

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

    #[test]
    fn decodes_the_status_launch_rights_gpus_and_vpns() {
        let status: ServerStatus = serde_json::from_value(json!({
            "is_admin": false,
            "username": "kebot",
            "via": "app_key",
            "settings": {"persistent_storage": true, "gpu": true, "vpn": false, "group": "default"},
            "gpus": [
                {"device": "intel-gpu", "driver": "i915"},
                {"device": "/dev/dri/renderD128", "driver": "nvidia", "model": "NVIDIA L40S"}
            ],
            "vpns": [{"id": "m", "name": "Mullvad", "kind": "wireguard",
                      "endpoints": [{"id": "de", "label": "de-fra"}]}],
            "cpu_model": "EPYC"
        }))
        .unwrap();
        assert!(status.settings.persistent_storage && status.settings.gpu && !status.settings.vpn);
        assert_eq!(status.gpus[0].device, "intel-gpu");
        assert_eq!(status.gpus[1].model.as_deref(), Some("NVIDIA L40S"));
        assert_eq!(status.vpns[0].endpoints[0].label, "de-fra");
        let minimal: ServerStatus = serde_json::from_value(json!({"username": "x"})).unwrap();
        assert!(minimal.gpus.is_empty() && !minimal.settings.gpu);
    }

    #[test]
    fn launch_body_carries_only_the_chosen_options() {
        let plain = LaunchParams::default().body("zen");
        assert_eq!(
            plain,
            json!({"application_id": "zen", "wayland_mode": true, "launch_in_room_mode": false})
        );
        let params = LaunchParams {
            home_name: Some("auto-zen".into()),
            create_home: true,
            language: Some("de_DE.UTF-8".into()),
            timezone: Some("Europe/Berlin".into()),
            selected_gpu: Some("intel-gpu".into()),
            vpn: Some("mullvad".into()),
            vpn_endpoint: Some("nl".into()),
            wayland_mode: false,
            launch_in_room_mode: true,
        };
        assert_eq!(
            params.body("zen"),
            json!({
                "application_id": "zen",
                "home_name": "auto-zen",
                "language": "de_DE.UTF-8",
                "timezone": "Europe/Berlin",
                "selected_gpu": "intel-gpu",
                "vpn": "mullvad",
                "vpn_endpoint": "nl",
                "wayland_mode": false,
                "launch_in_room_mode": true
            })
        );
        // An endpoint without a VPN is never sent.
        let stray = LaunchParams {
            vpn_endpoint: Some("nl".into()),
            ..LaunchParams::default()
        };
        assert!(stray.body("zen").get("vpn_endpoint").is_none());
    }

    #[test]
    fn deep_link_carries_the_launch_options() {
        let credential = Credential::parse("ssk_id_secret@seal.example").unwrap();
        let api = SealSkinClient::new(&credential).unwrap();
        assert_eq!(
            api.app_deep_link("zen", &[]).as_deref(),
            Some("https://seal.example/app/zen/")
        );
        assert_eq!(
            api.app_deep_link(
                "zen",
                &[("gpu", "intel-gpu".into()), ("lang", "de_DE.UTF-8".into())]
            )
            .as_deref(),
            Some("https://seal.example/app/zen/?gpu=intel-gpu&lang=de_DE.UTF-8")
        );
        assert!(api.app_deep_link("../x", &[]).is_none());
    }
}
