use base64::Engine as _;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::{Client, Method, RequestBuilder, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const KEY_VARIABLE: &str = "SEALSKIN_APPLICATION_KEY";

/// Why a Library load failed. `Refused` means the server failed the minimum-version gate
/// (ARCH-009): Skerry must not keep using that server, even on Refresh. `Key` and `Account`
/// are SealSkin refusing the key or the account (`ARCH-029`).
#[derive(Debug)]
pub enum LibraryError {
    Refused(String),
    Failed(String),
    Key(KeyRefusal),
    Account(AccountBlock),
}

/// Why SealSkin refused the application key: a 401 and its `detail` code, as SealSkin 0.5's
/// `application_keys.authenticate` documents them (`docs/content/api.md`). The codes are
/// matched exactly, as SealSkin's own shells do (`client/src/shell/host.js` `keyRefusal`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyRefusal {
    /// `unknown`: revoked (the record is deleted) or never existed.
    Revoked,
    /// `expired`.
    Expired,
    /// `dormant`: asleep until a web sign-in at `<origin>/?wake=1`.
    Dormant,
    /// `disabled`: the account's settings turn application keys off.
    KeysDisabled,
    /// `client`: the account doesn't allow the "Other clients" type Skerry sends.
    ClientNotAllowed,
    /// Any other 401, with its detail (`Invalid token.`, a proxy's page, a lane setting).
    Other(String),
}

/// An account state SealSkin refuses every route for (403). Only `Held` has a structural
/// signal (`held: true` in `admin/status`); the 403 texts are prose, pinned to 0.5.0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountBlock {
    /// 403 `User account is inactive.`
    Inactive,
    /// Waiting for an administrator to place it in a group.
    Held,
    /// Every Library request answered the same 403 nobody recognises; its text verbatim.
    Other(String),
}

/// A failed control-plane request, classified (`classify`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    Key(KeyRefusal),
    Account(AccountBlock),
    /// A 403 Skerry doesn't recognise, with the server's detail.
    Forbidden(String),
    /// Everything else, as user-facing text.
    Message(String),
}

impl ApiError {
    /// The key or the account was refused: the Library can't go on with this key.
    pub fn is_refusal(&self) -> bool {
        matches!(self, Self::Key(_) | Self::Account(_))
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Key(KeyRefusal::Revoked) => {
                "This application key no longer signs you in. Open the Library to paste a new one."
            }
            Self::Key(KeyRefusal::Expired) => {
                "This application key has expired. Open the Library to paste a new one."
            }
            Self::Key(KeyRefusal::Dormant) => {
                "The key for this server is asleep. Open the Library to sign in again."
            }
            Self::Key(KeyRefusal::KeysDisabled) => {
                "Application keys are turned off for this account."
            }
            Self::Key(KeyRefusal::ClientNotAllowed) => {
                "The account must allow “Other clients” to use application keys."
            }
            Self::Key(KeyRefusal::Other(_)) => "The server didn’t accept this application key.",
            Self::Account(AccountBlock::Inactive) => "This SealSkin account is turned off.",
            Self::Account(AccountBlock::Held) => {
                "This account is waiting for an administrator to add it to a group."
            }
            Self::Account(AccountBlock::Other(detail)) | Self::Forbidden(detail)
                if !detail.trim().is_empty() =>
            {
                // Quota and permission refusals: the server's own sentence says it best.
                return f.write_str(detail.trim());
            }
            Self::Account(AccountBlock::Other(_)) | Self::Forbidden(_) => {
                "This account doesn’t have permission for that action."
            }
            Self::Message(message) => message,
        };
        f.write_str(text)
    }
}

impl From<ApiError> for String {
    fn from(error: ApiError) -> Self {
        error.to_string()
    }
}

const HELD_DETAIL: &str = "This account waits for an administrator to place it in a group.";
const INACTIVE_DETAIL: &str = "User account is inactive.";

/// Classifies a failed response from its status and SealSkin `detail` (`None` when the body
/// isn't SealSkin's JSON, a proxy's HTML page for one). Only the 401 codes and `held` are
/// SealSkin contract; the two 403 texts are best-effort, pinned to 0.5.0 by a test.
pub(crate) fn classify(status: StatusCode, detail: Option<&str>) -> ApiError {
    match (status, detail) {
        (StatusCode::UNAUTHORIZED, Some("unknown")) => ApiError::Key(KeyRefusal::Revoked),
        (StatusCode::UNAUTHORIZED, Some("expired")) => ApiError::Key(KeyRefusal::Expired),
        (StatusCode::UNAUTHORIZED, Some("dormant")) => ApiError::Key(KeyRefusal::Dormant),
        (StatusCode::UNAUTHORIZED, Some("disabled")) => ApiError::Key(KeyRefusal::KeysDisabled),
        (StatusCode::UNAUTHORIZED, Some("client")) => ApiError::Key(KeyRefusal::ClientNotAllowed),
        (StatusCode::UNAUTHORIZED, detail) => ApiError::Key(KeyRefusal::Other(
            detail.unwrap_or_default().trim().to_string(),
        )),
        (StatusCode::FORBIDDEN, Some(INACTIVE_DETAIL)) => ApiError::Account(AccountBlock::Inactive),
        (StatusCode::FORBIDDEN, Some(HELD_DETAIL)) => ApiError::Account(AccountBlock::Held),
        (StatusCode::FORBIDDEN, detail) => {
            ApiError::Forbidden(detail.unwrap_or_default().trim().to_string())
        }
        _ => ApiError::Message(format!("SealSkin returned HTTP {}.", status.as_u16())),
    }
}

/// Reads a failed response's SealSkin `detail` (if its body is SealSkin's JSON) and
/// classifies it.
async fn classify_response(response: reqwest::Response) -> ApiError {
    let status = response.status();
    let detail = response
        .json::<Failure>()
        .await
        .ok()
        .map(|failure| failure.detail);
    classify(status, detail.as_deref())
}

/// The oldest SealSkin server version Skerry supports (ARCH-009). Older servers are refused.
pub const MINIMUM_SEALSKIN_VERSION: (u64, u64, u64) = (0, 5, 0);

#[derive(Clone)]
pub struct Credential {
    raw: String,
    base: Url,
    authorization: HeaderValue,
    source: KeySource,
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
            source: KeySource::Typed,
        })
    }

    /// Loads the key from `SEALSKIN_APPLICATION_KEY`, then `.env`, then `application_key` in
    /// `~/.config/skerry/config.toml`. Skerry never reads or writes the macOS Keychain
    /// (ARCH-011).
    pub fn startup() -> Result<Option<Self>, String> {
        let from = |raw: &str, source| Self::parse(raw).map(|key| Some(key.with_source(source)));
        if let Ok(value) = std::env::var(KEY_VARIABLE) {
            return from(&value, KeySource::Environment);
        }
        if let Ok(entries) = dotenvy::from_path_iter(".env") {
            for (name, value) in entries.flatten() {
                if name == KEY_VARIABLE {
                    return from(&value, KeySource::DotEnv);
                }
            }
        }
        match read_saved_key(&config_file()?)? {
            Some(raw) => from(&raw, KeySource::ConfigFile),
            None => Ok(None),
        }
    }

    fn with_source(mut self, source: KeySource) -> Self {
        self.source = source;
        self
    }

    pub fn source(&self) -> KeySource {
        self.source
    }

    /// The key as pasted, for the write queue to save.
    pub(crate) fn raw(&self) -> &str {
        &self.raw
    }
}

/// Where an application key came from, which decides what Sign Out can remove (`ARCH-011`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    Environment,
    DotEnv,
    ConfigFile,
    /// Pasted on the Connect screen.
    Typed,
}

/// Saves `raw` as `application_key` in the config file at `path`. Blocking: only `persist`'s
/// write queue calls it.
pub(crate) fn save_key_at(path: &Path, raw: &str) -> Result<(), String> {
    // One write: the key and `start_with`, so startup reconnects to SealSkin (ARCH-036).
    crate::config_store::edit_config(path, |document| {
        set_key_in(document, Some(raw));
        crate::config_store::set_start_with_in(
            document,
            Some(crate::config_store::StartWith::SealSkin),
        );
        true
    })
    .map_err(|error| {
            if error.kind() == io::ErrorKind::InvalidData {
                format!(
                    "Connected for this run. The key wasn’t saved because {CONFIG_DISPLAY} isn’t valid TOML. Fix or delete that file, then connect again."
                )
            } else {
                format!("Connected for this run. Skerry couldn’t save the key to {CONFIG_DISPLAY}.")
            }
        })
}

/// Whether config.toml holds `raw` as the saved key. A small synchronous read.
pub(crate) fn saved_key_is(raw: &str) -> bool {
    config_file()
        .and_then(|path| read_saved_key(&path))
        .is_ok_and(|saved| saved.as_deref() == Some(raw.trim()))
}

/// Removes `application_key` from the config file at `path` when it holds `raw` (the key
/// being signed out); another saved key is left alone. Blocking: only `persist`'s write
/// queue calls it.
pub(crate) fn forget_key_at(path: &Path, raw: &str) -> Result<(), String> {
    let failed = || format!("Couldn’t remove the saved application key from {CONFIG_DISPLAY}.");
    match read_saved_key(path) {
        Ok(Some(saved)) if saved == raw.trim() => write_saved_key(path, None).map_err(|_| failed()),
        Ok(_) => Ok(()),
        Err(_) => Err(failed()),
    }
}

const CONFIG_KEY: &str = "application_key";

pub(crate) use crate::config_store::config_file;
#[cfg(test)]
pub(crate) use crate::config_store::config_file_in;
use crate::config_store::{read_config, CONFIG_DISPLAY};

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
/// comment in the file, through the one config writer (`config_store::edit_config`).
fn write_saved_key(path: &Path, raw: Option<&str>) -> io::Result<()> {
    crate::config_store::edit_config(path, |document| set_key_in(document, raw))
}

fn set_key_in(document: &mut toml_edit::DocumentMut, raw: Option<&str>) -> bool {
    match raw {
        Some(raw) => {
            document[CONFIG_KEY] = toml_edit::value(raw.trim());
            true
        }
        None => document.remove(CONFIG_KEY).is_some(),
    }
}

#[derive(Clone, Debug, Deserialize)]
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
    /// The app opens web addresses: `/launch/url` and a session's `open_url` (`ARCH-028`).
    #[serde(default)]
    pub url_support: bool,
    /// File extensions the app declares, as the catalog writes them (`pdf` or `.pdf`).
    #[serde(default)]
    pub extensions: Vec<String>,
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

/// The user's effective settings: launch rights, groups, and limits. A negative limit means
/// unlimited, as SealSkin's `UserSettings` says (`models.py`); a missing one reads unlimited.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct UserSettings {
    #[serde(default)]
    pub persistent_storage: bool,
    #[serde(default)]
    pub gpu: bool,
    #[serde(default)]
    pub vpn: bool,
    #[serde(default)]
    pub groups: Vec<String>,
    /// GB (binary) per node.
    #[serde(default = "unlimited", deserialize_with = "whole_number")]
    pub storage_limit: i64,
    /// Sessions at once, across nodes.
    #[serde(default = "unlimited", deserialize_with = "whole_number")]
    pub session_limit: i64,
    #[serde(default = "unlimited_f", deserialize_with = "lenient_number")]
    pub session_cpus: f64,
    #[serde(default = "unlimited", deserialize_with = "whole_number")]
    pub session_memory_mb: i64,
    #[serde(default = "unlimited_f", deserialize_with = "lenient_number")]
    pub session_hours: f64,
    #[serde(default = "unlimited_f", deserialize_with = "lenient_number")]
    pub session_idle_minutes: f64,
    /// Hours after the last web sign-in before a key sleeps; negative: never.
    #[serde(default = "unlimited_f", deserialize_with = "lenient_number")]
    pub application_key_dormant_hours: f64,
    /// A key sleeps when the web sign-in ends.
    #[serde(default)]
    pub application_key_follow_sign_in: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            persistent_storage: false,
            gpu: false,
            vpn: false,
            groups: Vec::new(),
            storage_limit: -1,
            session_limit: -1,
            session_cpus: -1.,
            session_memory_mb: -1,
            session_hours: -1.,
            session_idle_minutes: -1.,
            application_key_dormant_hours: -1.,
            application_key_follow_sign_in: false,
        }
    }
}

fn unlimited() -> i64 {
    -1
}

fn unlimited_f() -> f64 {
    -1.
}

/// A limit that isn't a number (null, a string) reads unlimited rather than failing the
/// whole status, which would also lose its structural `held`.
fn lenient_number<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_f64()
        .unwrap_or(-1.))
}

/// An integer limit, accepting any JSON number (pydantic may write `40.0`).
fn whole_number<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    Ok(lenient_number(deserializer)?.round() as i64)
}

/// Hours of session time per period, and how many are used (`quota.py`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Allowance {
    pub hours: f64,
    #[serde(default)]
    pub period: String,
    #[serde(default)]
    pub used: f64,
}

/// What `POST /companion/api/admin/status` says about the user: their launch choices,
/// account, limits, and allowance (`AdminStatusResponse`).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServerStatus {
    #[serde(default)]
    pub settings: UserSettings,
    #[serde(default)]
    pub gpus: Vec<GpuInfo>,
    #[serde(default)]
    pub vpns: Vec<VpnChoice>,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default)]
    pub held: bool,
    /// Empty when the status couldn't be read.
    #[serde(default)]
    pub username: String,
    /// `None` for admins and unlimited accounts.
    #[serde(default)]
    pub allowance: Option<Allowance>,
    /// Bytes in the user's storage, only when asked with `?storage=1`.
    #[serde(default)]
    pub storage_used: Option<u64>,
}

#[derive(Deserialize)]
struct HomeDirectories {
    #[serde(default)]
    home_dirs: Vec<String>,
}

/// Pages of a storage listing to read at most (200 names each).
const STORAGE_LIST_MAX_PAGES: u32 = 50;

/// One entry of a folder in persistent storage (`GET /companion/api/files/list/<home>`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StorageEntry {
    pub name: String,
    /// Home-rooted (`/Desktop/x`). Never used for a local path (`ARCH-037`).
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub is_dir: bool,
    /// `st_size`: a folder reads its block size, never shown.
    #[serde(default)]
    pub size: u64,
    /// Unix epoch seconds.
    #[serde(default)]
    pub mtime: f64,
}

#[derive(Deserialize)]
struct StoragePage {
    items: Vec<StorageEntry>,
    #[serde(default)]
    total: u64,
}

/// A folder of persistent storage, every page of it up to [`STORAGE_LIST_MAX_PAGES`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StorageListing {
    pub entries: Vec<StorageEntry>,
    /// More items exist past the cap.
    pub truncated: bool,
}

/// The home every user has beside their homes: Shared Files, mounted read-write into every
/// session with storage at `/config/Desktop/files` (`ARCH-037`). Never deleted, never
/// offered as a launch home.
pub const SHARED_FILES_HOME: &str = "_sealskin_shared_files";

/// Persistent storage moves in 2 MiB chunks both ways, as SealSkin's web file manager does.
pub const STORAGE_CHUNK: usize = 2 << 20;

/// A folder created in storage, or one that was there already.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Created {
    New,
    Existed,
}

/// Why a home couldn't be created or deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HomeError {
    /// 409 "already exists".
    Exists,
    /// The server refused the name, with its sentence.
    Invalid(String),
    /// 404 on delete: someone deleted it already.
    Gone,
    /// Shared Files or Cleanroom: never sent (`ARCH-037`).
    Reserved,
    Api(ApiError),
}

impl std::fmt::Display for HomeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exists => f.write_str("A home with that name already exists."),
            Self::Invalid(detail) => f.write_str(detail),
            Self::Gone => f.write_str("That home was already deleted."),
            Self::Reserved => f.write_str("That home can’t be deleted."),
            Self::Api(error) => write!(f, "{error}"),
        }
    }
}

/// Why a storage transfer stopped.
#[derive(Debug)]
pub enum StorageError {
    /// The request didn't reach the server or its answer was lost: a retry may work.
    Network(String),
    /// SealSkin answered and refused (classified, `ARCH-029`).
    Api(ApiError),
    /// The Mac side: reading or writing a local file.
    Local(io::Error),
    /// The answer made no sense (bad base64, an oversized chunk, a size that changed).
    Invalid(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(message) | Self::Invalid(message) => f.write_str(message),
            Self::Api(error) => write!(f, "{error}"),
            Self::Local(error) => write!(f, "{error}"),
        }
    }
}

impl From<io::Error> for StorageError {
    fn from(error: io::Error) -> Self {
        Self::Local(error)
    }
}

#[derive(Deserialize)]
struct DownloadChunk {
    chunk_data_b64: String,
    #[serde(default)]
    is_last_chunk: bool,
}

/// What the listing said about a stored file, which a kept partial must match to resume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialSource {
    pub size: u64,
    /// The server's mtime as the pane shows it.
    pub mtime: String,
}

/// The extended attribute naming where a `.skerrydownload` partial came from.
const PARTIAL_XATTR: &str = "com.skerry.partial";

fn partial_tag(home: &str, path: &str, source: &PartialSource) -> String {
    format!("{home}\n{path}\n{}\n{}", source.size, source.mtime)
}

fn xattr_name() -> std::ffi::CString {
    std::ffi::CString::new(PARTIAL_XATTR).unwrap_or_default()
}

fn c_path(path: &Path) -> Option<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt as _;
    std::ffi::CString::new(path.as_os_str().as_bytes()).ok()
}

fn read_partial_tag(path: &Path) -> Option<String> {
    let path = c_path(path)?;
    let name = xattr_name();
    let mut buffer = vec![0u8; 4096];
    // SAFETY: valid C strings and a buffer of the length passed.
    let read = unsafe {
        libc::getxattr(
            path.as_ptr(),
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            0,
            0,
        )
    };
    if read < 0 {
        return None;
    }
    buffer.truncate(read as usize);
    String::from_utf8(buffer).ok()
}

fn write_partial_tag(path: &Path, tag: &str) -> io::Result<()> {
    let path = c_path(path).ok_or_else(|| io::Error::other("bad path"))?;
    let name = xattr_name();
    // SAFETY: valid C strings and a value of the length passed.
    let done = unsafe {
        libc::setxattr(
            path.as_ptr(),
            name.as_ptr(),
            tag.as_ptr().cast(),
            tag.len(),
            0,
            0,
        )
    };
    if done == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn remove_partial_tag(path: &Path) {
    if let Some(path) = c_path(path) {
        let name = xattr_name();
        // SAFETY: valid C strings.
        unsafe { libc::removexattr(path.as_ptr(), name.as_ptr(), 0) };
    }
}

/// Whether a home name is one Skerry must never delete: Shared Files (every session's
/// `Desktop/files`) and Cleanroom, in any case (`ARCH-037`).
pub fn undeletable_home(name: &str) -> bool {
    name.eq_ignore_ascii_case(SHARED_FILES_HOME) || name.eq_ignore_ascii_case("cleanroom")
}

/// A server name for one path segment (a file or folder name inside a folder).
fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
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

#[derive(Clone, Debug, Deserialize)]
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
    /// A collaborative (room) session; reuse never mixes it with a solo one (`ARCH-027`).
    #[serde(default)]
    pub is_collaboration: bool,
}

#[derive(Clone, Debug)]
pub struct LaunchParams {
    /// The id this launch reports progress under, also sent as `X-Idempotency-Key` so a
    /// resend joins the attempt already running instead of starting a second container.
    pub launch_id: Option<String>,
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
            launch_id: None,
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
            ("launch_id", &self.launch_id),
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

/// One launch's progress, as `GET /companion/api/launch/progress/<id>` reports it.
/// `session_url` and `session_id` come with `ready`, `error` with `failed`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct LaunchProgress {
    pub stage: String,
    #[serde(default)]
    pub detail: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub elapsed: f64,
    #[serde(default)]
    pub session_url: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// What the progress route said about a launch.
#[derive(Clone, Debug, PartialEq)]
pub enum ProgressAnswer {
    Known(LaunchProgress),
    /// 404: the server isn't following this id (not seen yet, pruned, or never followed).
    Unknown,
    /// 401: the key no longer works, and why.
    SignedOut(KeyRefusal),
}

/// Why a launch request ended without a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchError {
    /// The request may not have reached the server, or its answer was lost: a connect,
    /// reset, or timeout, or a gateway's 502/503/504 without SealSkin's JSON `detail`. The
    /// launch may still be running; progress decides.
    Network(String),
    /// SealSkin answered and refused.
    Server(String),
    /// SealSkin refused the key or the account (`ApiError::is_refusal`); the Launcher
    /// handles it as a refused Library load (`ARCH-029`).
    Refused(ApiError),
}

/// The ceiling on one launch request: the server's 900 s pending limit plus a margin. A
/// launch that outlives it is still followed through progress.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(960);

/// Which launch route a launch takes (`ARCH-028`).
#[derive(Clone, Copy, Debug)]
pub enum LaunchRoute<'a> {
    Simple,
    /// `/launch/url`; the address passed `open_target::validate_url`.
    Url(&'a str),
    /// `/launch/file`, after [`SealSkinClient::upload_for_launch`].
    File(&'a Uploaded),
    /// `/launch/file_path`: a top-level file of Shared Files, by its name (`ARCH-037`).
    FilePath(&'a str),
}

/// A finished chunked upload, ready for `/launch/file`. The server deletes it once a launch
/// reassembles it, so a retry uploads again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uploaded {
    pub upload_id: String,
    pub total_chunks: u64,
    /// The file's own name, no folders.
    pub filename: String,
}

/// Why a running session didn't take a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenUrlError {
    /// 409 (the app opens no addresses, or nothing took it) or 404 (not this user's).
    NotTaken,
    /// 422: the address failed the server's pattern.
    Refused,
    /// Unreachable, or another status.
    Failed(String),
    /// SealSkin refused the key or the account (`ARCH-029`): the Library handles it.
    Key(ApiError),
}

/// Why a launch upload stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadError {
    /// SealSkin refused the key or the account (`ARCH-029`): the Library handles it.
    Refused(ApiError),
    /// Everything else, as the flight card's text.
    Failed(String),
}

impl From<String> for UploadError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(error) => write!(f, "{error}"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

/// `/launch/file` chunks: 1 MiB, as the web client sends them (the server takes up to 8).
pub const LAUNCH_UPLOAD_CHUNK: usize = 1 << 20;
const UPLOAD_CHUNK_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Deserialize)]
struct Initiated {
    upload_id: String,
}

/// An upload id is a lower-case UUID, as `uploads.py` makes them.
fn valid_upload_id(id: &str) -> bool {
    valid_launch_id(id) && !id.bytes().any(|b| b.is_ascii_uppercase())
}

/// The ceiling on `admin/status?storage=1`: with no earlier measure the server walks every
/// file in the user's storage (`storage_usage.py`), which can outlast the usual 30 s.
const STORAGE_MEASURE_TIMEOUT: Duration = Duration::from_secs(120);

/// The `<id>` in `ssk_<id>_<secret>@<host>`, which isn't secret (the web lists key ids).
fn key_id_of(raw: &str) -> &str {
    raw.trim()
        .strip_prefix("ssk_")
        .and_then(|rest| rest.split_once('_'))
        .map_or("", |(id, _)| id)
}

/// A launch id is a UUID (8-4-4-4-12 hex), as the server's progress route expects.
pub fn valid_launch_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, len)| group.len() == len && group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The session a launch of `app` with `params` started, as the Library lists it until the
/// next Refresh brings the server's own record.
pub fn launched_session(
    app: &RemoteApp,
    params: &LaunchParams,
    session_id: String,
    session_url: String,
) -> RemoteSession {
    RemoteSession {
        session_id,
        app_id: app.id.clone(),
        app_name: app.name.clone(),
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |elapsed| elapsed.as_secs_f64()),
        session_url,
        // The server lists a cleanroom session with no home.
        home: params
            .home_name
            .clone()
            .filter(|home| home != "cleanroom")
            .unwrap_or_default(),
        gpu: params.selected_gpu.is_some(),
        is_collaboration: params.launch_in_room_mode,
    }
}

/// The session a launch reached `ready` with, when its own request was lost.
pub fn session_from_progress(
    app: &RemoteApp,
    params: &LaunchParams,
    progress: &LaunchProgress,
) -> Option<RemoteSession> {
    if progress.stage != "ready" {
        return None;
    }
    let session_id = progress.session_id.clone()?;
    let session_url = progress.session_url.clone()?;
    valid_path_segment(&session_id).then(|| launched_session(app, params, session_id, session_url))
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

/// A Library load's outcome from its three requests, most decisive first: a refused key,
/// then `held: true` (structural, so it wins over any English text), then a refused account,
/// then the same unrecognised 403 on all three, then the first failure. `status` is the
/// status's `held` flag, or its error.
fn library_parts<A, S>(
    apps: Result<A, ApiError>,
    sessions: Result<S, ApiError>,
    status: Result<bool, &ApiError>,
) -> Result<(A, S), LibraryError> {
    let refused = {
        let errors: Vec<&ApiError> = [apps.as_ref().err(), sessions.as_ref().err(), status.err()]
            .into_iter()
            .flatten()
            .collect();
        let key = errors.iter().find_map(|error| match error {
            ApiError::Key(refusal) => Some(LibraryError::Key(refusal.clone())),
            _ => None,
        });
        let account = errors.iter().find_map(|error| match error {
            ApiError::Account(block) => Some(LibraryError::Account(block.clone())),
            _ => None,
        });
        let forbidden = match errors.as_slice() {
            [ApiError::Forbidden(a), ApiError::Forbidden(b), ApiError::Forbidden(c)]
                if a == b && b == c =>
            {
                Some(LibraryError::Account(AccountBlock::Other(a.clone())))
            }
            _ => None,
        };
        key.or((status == Ok(true)).then_some(LibraryError::Account(AccountBlock::Held)))
            .or(account)
            .or(forbidden)
    };
    if let Some(refused) = refused {
        return Err(refused);
    }
    match (apps, sessions) {
        (Ok(apps), Ok(sessions)) => Ok((apps, sessions)),
        (Err(error), _) | (_, Err(error)) => Err(LibraryError::Failed(error.to_string())),
    }
}

/// The only paths that get the application key.
const COMPANION_API_PREFIX: &str = "/companion/api/";

/// Whether a request to `path` (already resolved, as `Url::path` gives it) carries the key.
pub(crate) fn requires_key(path: &str) -> bool {
    path.starts_with(COMPANION_API_PREFIX)
}

/// The SealSkin control plane. The key goes only to `/companion/api/`. Everything else
/// (logos, `/app/<id>/icon.png`, which session containers serve) is fetched without
/// credentials, through a second client that has no `Authorization` header at all.
#[derive(Clone)]
pub struct SealSkinClient {
    authed: Client,
    public: Client,
    base: Url,
    key_source: KeySource,
    /// The key as pasted, so Sign Out can remove it from config.toml if it is saved there.
    raw_key: Arc<str>,
}

impl SealSkinClient {
    pub fn new(credential: &Credential) -> Result<Self, String> {
        let build = |key: Option<&HeaderValue>| {
            let mut headers = HeaderMap::new();
            if let Some(key) = key {
                headers.insert(AUTHORIZATION, key.clone());
            }
            headers.insert("x-sealskin-app", HeaderValue::from_static("other"));
            Client::builder()
                .default_headers(headers)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|_| "Couldn’t initialize the secure connection.".to_string())
        };
        Ok(Self {
            authed: build(Some(&credential.authorization))?,
            public: build(None)?,
            base: credential.base.clone(),
            key_source: credential.source,
            raw_key: credential.raw.as_str().into(),
        })
    }

    /// A client for `credential` that talks to `base` (a plain-HTTP test listener).
    #[cfg(test)]
    fn with_base(credential: &Credential, base: Url) -> Self {
        Self {
            base,
            ..Self::new(credential).unwrap()
        }
    }

    /// Where the key this client uses came from.
    pub fn key_source(&self) -> KeySource {
        self.key_source
    }

    /// The key this client uses, as pasted.
    pub(crate) fn raw_key(&self) -> &str {
        &self.raw_key
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

    /// A request to `path` on this server. The check runs on the resolved URL, in release
    /// builds too: a path outside `/companion/api/` goes out without the key.
    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let mut url = self.base.clone();
        url.set_path(path);
        if requires_key(url.path()) {
            &self.authed
        } else {
            &self.public
        }
        .request(method, url)
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
            return Err(classify_version_status(response.status()));
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
        let status = self.account(false);
        let (apps, sessions, status) = tokio::join!(apps, sessions, status);
        let (apps, sessions) = library_parts(apps, sessions, status.as_ref().map(|s| s.held))?;
        // Launch choices and the account menu are extras: without the status the dialog
        // offers what it always did and the menu says the account is unavailable.
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

    /// The user's account, limits, and launch choices (`POST /companion/api/admin/status`,
    /// open to every signed-in user, a held one too). `measure_storage` asks the server to
    /// walk the user's storage (`?storage=1`), which can take a while, so it has its own
    /// longer timeout; only the account menu asks for it.
    pub async fn account(&self, measure_storage: bool) -> Result<ServerStatus, ApiError> {
        let mut request = self
            .request(Method::POST, "/companion/api/admin/status")
            .json(&json!({}));
        if measure_storage {
            request = request
                .query(&[("storage", "1")])
                .timeout(STORAGE_MEASURE_TIMEOUT);
        }
        send(request).await
    }

    /// Whether the key signs in now: the wake poll's one cheap request.
    pub async fn key_works(&self) -> Result<(), ApiError> {
        self.account(false).await.map(|_| ())
    }

    /// Where a dormant key wakes: the server's web app at `/?wake=1`, opened in the browser.
    /// It carries no part of the key; the browser signs in on its own (`ARCH-029`).
    pub fn wake_url(&self) -> Url {
        let mut url = self.web_url();
        url.set_query(Some("wake=1"));
        url
    }

    /// The server's web app (`/`), without the key.
    pub fn web_url(&self) -> Url {
        let mut url = self.base.clone();
        url.set_path("/");
        url.set_query(None);
        url.set_fragment(None);
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url
    }

    /// The key's id, the non-secret `<id>` in `ssk_<id>_<secret>`.
    pub fn key_id(&self) -> &str {
        key_id_of(&self.raw_key)
    }

    /// Every directory in the user's storage, Shared Files and app homes included
    /// (`GET /companion/api/homedirs`).
    pub async fn home_directories(&self) -> Result<Vec<String>, ApiError> {
        let homes: HomeDirectories =
            send(self.request(Method::GET, "/companion/api/homedirs")).await?;
        Ok(homes.home_dirs)
    }

    /// Creates the home `name` (`POST /companion/api/homedirs`). Unlike the launch's Auto
    /// step, a 409 is an error here: "already exists", or pydantic's validation sentence,
    /// which SealSkin 0.5 also answers with 409 (`ARCH-037`).
    pub async fn create_home(&self, name: &str) -> Result<(), HomeError> {
        if !valid_path_segment(name) || undeletable_home(name) {
            return Err(HomeError::Invalid("Letters, numbers, - and _ only.".into()));
        }
        match send_with_detail::<serde_json::Value>(
            self.request(Method::POST, "/companion/api/homedirs")
                .json(&json!({ "home_name": name })),
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(refusal) => Err(refusal.home_error()),
        }
    }

    /// Deletes the home `name` (`DELETE /companion/api/homedirs/<name>`). The server would
    /// delete Shared Files too, so Shared Files and Cleanroom are refused here, before any
    /// request (`ARCH-037`). The caller checks running sessions first: the server doesn't.
    pub async fn delete_home(&self, name: &str) -> Result<(), HomeError> {
        if undeletable_home(name) {
            return Err(HomeError::Reserved);
        }
        if !valid_path_segment(name) {
            return Err(HomeError::Invalid("Invalid home name.".into()));
        }
        let path = format!("/companion/api/homedirs/{name}");
        let response = self
            .request(Method::DELETE, &path)
            .send()
            .await
            .map_err(|_| {
                HomeError::Api(ApiError::Message(
                    "Couldn’t reach the SealSkin server to delete the home.".into(),
                ))
            })?;
        match response.status() {
            status if status.is_success() => Ok(()),
            StatusCode::NOT_FOUND => Err(HomeError::Gone),
            _ => Err(HomeError::Api(classify_response(response).await)),
        }
    }

    async fn create_home_directory(&self, home_name: &str) -> Result<(), ApiError> {
        let response = self
            .request(Method::POST, "/companion/api/homedirs")
            .json(&json!({ "home_name": home_name }))
            .send()
            .await
            .map_err(|_| {
                ApiError::Message("Couldn’t reach the SealSkin server to create the home.".into())
            })?;
        // 409: it exists already (made elsewhere since the Library loaded).
        if response.status().is_success() || response.status() == StatusCode::CONFLICT {
            return Ok(());
        }
        Err(match classify_response(response).await {
            ApiError::Message(_) => {
                ApiError::Message(format!("SealSkin couldn’t create the home “{home_name}”."))
            }
            error => error,
        })
    }

    /// Launches `app`. `on_send` runs just before the launch request goes out (after any
    /// Auto home is created), so a follower counts "unseen" time from then.
    ///
    /// With a `launch_id`, the request carries it as `X-Idempotency-Key` too: SealSkin runs
    /// a keyed request at most once per credential and key for 600 s, so a resend joins
    /// the first attempt. The key is scoped by the request's cookie and authorization, so
    /// the authed client must never keep a cookie store: a cookie that changed between two
    /// attempts would make them different requests.
    pub async fn launch_with_options(
        &self,
        app: RemoteApp,
        params: LaunchParams,
        on_send: impl FnOnce() + Send,
    ) -> Result<RemoteSession, LaunchError> {
        self.launch(app, params, LaunchRoute::Simple, on_send).await
    }

    /// Launches `app` through one of SealSkin's launch routes (`/launch/simple`,
    /// `/launch/url`, `/launch/file`, `ARCH-028`). All three share the body, `launch_id` and
    /// `X-Idempotency-Key`, the timeout, and the error mapping of a simple launch.
    pub async fn launch(
        &self,
        app: RemoteApp,
        params: LaunchParams,
        route: LaunchRoute<'_>,
        on_send: impl FnOnce() + Send,
    ) -> Result<RemoteSession, LaunchError> {
        if let (true, Some(home_name)) = (params.create_home, &params.home_name) {
            self.create_home_directory(home_name)
                .await
                .map_err(|error| {
                    if error.is_refusal() {
                        LaunchError::Refused(error)
                    } else {
                        LaunchError::Server(error.to_string())
                    }
                })?;
        }
        let mut body = params.body(&app.id);
        let path = match route {
            LaunchRoute::Simple => "/companion/api/launch/simple",
            LaunchRoute::Url(url) => {
                body["url"] = json!(url);
                "/companion/api/launch/url"
            }
            LaunchRoute::File(uploaded) => {
                if !valid_upload_id(&uploaded.upload_id) {
                    return Err(LaunchError::Server("Invalid upload identifier.".into()));
                }
                body["filename"] = json!(uploaded.filename);
                body["upload_id"] = json!(uploaded.upload_id);
                body["total_chunks"] = json!(uploaded.total_chunks);
                // The user chose "open this file" (no checkbox, ARCH-028).
                body["open_file_on_launch"] = json!(true);
                "/companion/api/launch/file"
            }
            LaunchRoute::FilePath(filename) => {
                // The server opens the basename inside Shared Files and refuses Cleanroom;
                // neither is ever sent (`ARCH-037`).
                if !valid_name(filename) {
                    return Err(LaunchError::Server(
                        "Only files at the top of Shared Files can be opened this way.".into(),
                    ));
                }
                if params
                    .home_name
                    .as_deref()
                    .is_none_or(|home| home.eq_ignore_ascii_case("cleanroom"))
                {
                    return Err(LaunchError::Server(
                        "Choose a home: Cleanroom sessions can’t open Shared Files.".into(),
                    ));
                }
                body["filename"] = json!(filename);
                // `LaunchRequestFilePath` has no room flag.
                if let Some(map) = body.as_object_mut() {
                    map.remove("launch_in_room_mode");
                }
                "/companion/api/launch/file_path"
            }
        };
        let mut request = self
            .request(Method::POST, path)
            .timeout(LAUNCH_TIMEOUT)
            .json(&body);
        if let Some(launch_id) = &params.launch_id {
            request = request.header("X-Idempotency-Key", launch_id);
        }
        on_send();
        let response: Launched = send_launch(request).await?;
        Ok(launched_session(
            &app,
            &params,
            response.session_id,
            response.session_url,
        ))
    }

    /// Uploads `path` for `/launch/file` through SealSkin's chunked upload
    /// (`/companion/api/upload/initiate`, then `/upload/chunk`), streaming it from disk in
    /// 1 MiB chunks, one at a time: at most one chunk and its base64 are in memory. `sent`
    /// counts the bytes sent. A file that changes size while it uploads is refused, and no
    /// launch follows.
    pub async fn upload_for_launch(
        &self,
        path: &Path,
        sent: &AtomicU64,
    ) -> Result<Uploaded, UploadError> {
        let filename = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "That isn’t a file Skerry can open.".to_string())?;
        let unreadable = |error: io::Error| format!("Couldn’t read “{filename}”: {error}");
        let mut file = tokio::fs::File::open(path).await.map_err(unreadable)?;
        // The size of the file this handle reads, not of whatever the path names later.
        let total = file.metadata().await.map_err(unreadable)?.len();
        let (upload_id, total_chunks) = self
            .upload_chunks(
                &mut file,
                &filename,
                total,
                LAUNCH_UPLOAD_CHUNK,
                false,
                sent,
            )
            .await?;
        Ok(Uploaded {
            upload_id,
            total_chunks,
            filename,
        })
    }

    /// SealSkin's chunked upload of the open `file` (`total` bytes): `upload/initiate`, then
    /// `upload/chunk` in `chunk`-byte pieces, one at a time, streamed from disk. A file that
    /// changes size is refused. With `retry` (storage), `initiate` carries a fresh
    /// `X-Idempotency-Key` and is resent once after a lost answer, and a chunk is resent
    /// twice after a network error or a gateway's 502/503/504, never after a 4xx (resending
    /// an index overwrites it). Returns the upload id and the number of chunks.
    async fn upload_chunks(
        &self,
        file: &mut tokio::fs::File,
        filename: &str,
        total: u64,
        chunk: usize,
        retry: bool,
        sent: &AtomicU64,
    ) -> Result<(String, u64), UploadError> {
        use tokio::io::AsyncReadExt as _;
        let unreadable = |error: io::Error| format!("Couldn’t read “{filename}”: {error}");
        let initiate_key = uuid::Uuid::new_v4().to_string();
        let mut attempt = 0;
        let initiated: Initiated = loop {
            let mut request = self
                .request(Method::POST, "/companion/api/upload/initiate")
                .json(&json!({ "filename": filename, "total_size": total }));
            if retry {
                request = request.header("X-Idempotency-Key", &initiate_key);
            }
            match send_with_detail(request).await {
                Ok(initiated) => break initiated,
                Err(refusal) if retry && attempt == 0 && refusal.status.is_none() => attempt += 1,
                Err(refusal) => {
                    return Err(refusal.upload_error_in(UploadStep::Initiate, filename, retry))
                }
            }
        };
        let upload_id = initiated.upload_id;
        if !valid_upload_id(&upload_id) {
            return Err(UploadError::Failed(
                "The server returned an invalid upload identifier.".into(),
            ));
        }
        let changed = || format!("“{filename}” changed while it was uploading.");
        let mut buffer = vec![0u8; chunk];
        let mut done: u64 = 0;
        let mut index: u64 = 0;
        while done < total {
            let want = (total - done).min(chunk as u64) as usize;
            let mut filled = 0;
            while filled < want {
                let read = file
                    .read(&mut buffer[filled..want])
                    .await
                    .map_err(unreadable)?;
                if read == 0 {
                    return Err(changed().into());
                }
                filled += read;
            }
            let encoded = base64::engine::general_purpose::STANDARD.encode(&buffer[..filled]);
            let body = json!({
                "upload_id": upload_id,
                "chunk_index": index,
                "chunk_data_b64": encoded,
            });
            drop(encoded);
            let mut attempt = 0;
            loop {
                let result = send_with_detail::<serde_json::Value>(
                    self.request(Method::POST, "/companion/api/upload/chunk")
                        .timeout(UPLOAD_CHUNK_TIMEOUT)
                        .json(&body),
                )
                .await;
                match result {
                    Ok(_) => break,
                    Err(refusal) if retry && attempt < 2 && refusal.transient() => {
                        attempt += 1;
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    Err(refusal) => {
                        return Err(refusal.upload_error_in(UploadStep::Chunk, filename, retry));
                    }
                }
            }
            done += filled as u64;
            index += 1;
            sent.store(done, Ordering::Relaxed);
        }
        // Bytes past the size measured at the start: the file grew.
        if file.read(&mut buffer[..1]).await.map_err(unreadable)? != 0 {
            return Err(changed().into());
        }
        Ok((upload_id, index))
    }

    /// Hands `url` to a running session (`POST /companion/api/sessions/<id>/open_url`).
    pub async fn open_url_in_session(
        &self,
        session_id: &str,
        url: &str,
    ) -> Result<(), OpenUrlError> {
        if !valid_path_segment(session_id) {
            return Err(OpenUrlError::Failed("Invalid session identifier.".into()));
        }
        let path = format!("/companion/api/sessions/{session_id}/open_url");
        match send_with_detail::<serde_json::Value>(
            self.request(Method::POST, &path)
                .json(&json!({ "url": url })),
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(refusal) => Err(match (refusal.key_refusal(), refusal.status) {
                (Some(error), _) => OpenUrlError::Key(error),
                (None, Some(StatusCode::NOT_FOUND | StatusCode::CONFLICT)) => {
                    OpenUrlError::NotTaken
                }
                (None, Some(StatusCode::UNPROCESSABLE_ENTITY)) => OpenUrlError::Refused,
                (None, _) => OpenUrlError::Failed(refusal.message()),
            }),
        }
    }

    /// Where launch `launch_id` has got to (`GET /companion/api/launch/progress/<id>`).
    pub async fn launch_progress(&self, launch_id: &str) -> Result<ProgressAnswer, String> {
        if !valid_launch_id(launch_id) {
            return Err("Invalid launch identifier.".into());
        }
        let path = format!("/companion/api/launch/progress/{launch_id}");
        let response = self
            .request(Method::GET, &path)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|_| "Couldn’t reach the SealSkin server.".to_string())?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(ProgressAnswer::Unknown),
            StatusCode::UNAUTHORIZED => match classify_response(response).await {
                ApiError::Key(refusal) => Ok(ProgressAnswer::SignedOut(refusal)),
                _ => Ok(ProgressAnswer::SignedOut(KeyRefusal::Other(String::new()))),
            },
            status if status.is_success() => response
                .json()
                .await
                .map(ProgressAnswer::Known)
                .map_err(|_| "The server returned an unexpected response.".into()),
            status => Err(format!("SealSkin returned HTTP {}.", status.as_u16())),
        }
    }

    /// The user's running sessions (`GET /companion/api/sessions`).
    pub async fn sessions(&self) -> Result<Vec<RemoteSession>, String> {
        Ok(send(self.request(Method::GET, "/companion/api/sessions")).await?)
    }

    pub async fn stop_session(&self, session_id: &str) -> Result<(), ApiError> {
        if !valid_path_segment(session_id) {
            return Err(ApiError::Message("Invalid session identifier.".into()));
        }
        let path = format!("/companion/api/sessions/{session_id}");
        let response = self
            .request(Method::DELETE, &path)
            .send()
            .await
            .map_err(|_| {
                ApiError::Message("Couldn’t reach the SealSkin server to stop the session.".into())
            })?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(match classify_response(response).await {
            ApiError::Message(_) => ApiError::Message("SealSkin couldn’t stop the session.".into()),
            error => error,
        })
    }

    /// The names in one folder of a home's persistent storage, every page of it
    /// (`GET /companion/api/files/list/<home>`, `ARCH-024`).
    pub async fn storage_names(&self, home: &str, path: &str) -> Result<Vec<String>, ApiError> {
        let listing = self.list_storage(home, path).await?;
        // A partial list must never feed `same_folder` (`ARCH-024`).
        if listing.truncated {
            return Err(ApiError::Message(
                "The folder holds too many items to compare.".into(),
            ));
        }
        Ok(listing
            .entries
            .into_iter()
            .map(|entry| entry.name)
            .collect())
    }

    /// One folder of a home's persistent storage, every page of it up to 10,000 items
    /// (`GET /companion/api/files/list/<home>?path=&page=&per_page=200`). `path` is
    /// home-rooted (`/`, `/Desktop`). Names repeated across pages (the folder changed while
    /// it was read) are kept once.
    pub async fn list_storage(&self, home: &str, path: &str) -> Result<StorageListing, ApiError> {
        if !valid_path_segment(home) {
            return Err(ApiError::Message("Invalid home name.".into()));
        }
        let route = format!("/companion/api/files/list/{home}");
        let mut entries: Vec<StorageEntry> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut read = 0u64;
        for page in 1..=STORAGE_LIST_MAX_PAGES {
            let listing: StoragePage = send(self.request(Method::GET, &route).query(&[
                ("path", path),
                ("page", &page.to_string()),
                ("per_page", "200"),
            ]))
            .await?;
            let count = listing.items.len();
            read += count as u64;
            for item in listing.items {
                if seen.insert(item.name.clone()) {
                    entries.push(item);
                }
            }
            if count == 0 || read >= listing.total {
                return Ok(StorageListing {
                    entries,
                    truncated: false,
                });
            }
        }
        Ok(StorageListing {
            entries,
            truncated: true,
        })
    }

    /// Makes the folder `name` in `parent` (home-rooted) of `home`. A 409 is "exists" only
    /// when a fresh listing shows a folder of that name; a file there is an error
    /// (`ARCH-037`).
    pub async fn create_storage_folder(
        &self,
        home: &str,
        parent: &str,
        name: &str,
    ) -> Result<Created, ApiError> {
        if !valid_path_segment(home) {
            return Err(ApiError::Message("Invalid home name.".into()));
        }
        if !valid_name(name) {
            return Err(ApiError::Message(
                "A folder name can’t be empty or contain “/”.".into(),
            ));
        }
        let route = format!("/companion/api/files/create_folder/{home}");
        match send_with_detail::<serde_json::Value>(
            self.request(Method::POST, &route)
                .json(&json!({ "path": parent, "folder_name": name })),
        )
        .await
        {
            Ok(_) => Ok(Created::New),
            Err(refusal) if refusal.status == Some(StatusCode::CONFLICT) => {
                let listing = self.list_storage(home, parent).await?;
                match listing.entries.iter().find(|entry| entry.name == name) {
                    Some(entry) if entry.is_dir => Ok(Created::Existed),
                    Some(_) => Err(ApiError::Message(format!(
                        "A file named “{name}” is in the way."
                    ))),
                    None => Err(ApiError::Message(refusal.message())),
                }
            }
            Err(refusal) => Err(refusal.api_error()),
        }
    }

    /// Uploads the local file `local` into the folder `dir` (home-rooted) of `home` as
    /// `filename`: `upload/initiate`, 2 MiB chunks in order, then
    /// `files/upload_to_dir/<home>`, which never overwrites (a taken name becomes
    /// `name-1.ext`). At most one chunk is in memory. `sent` counts bytes sent.
    pub async fn upload_to_storage(
        &self,
        home: &str,
        dir: &str,
        local: &Path,
        filename: &str,
        sent: &AtomicU64,
    ) -> Result<(), UploadError> {
        if !valid_path_segment(home) {
            return Err(UploadError::Failed("Invalid home name.".into()));
        }
        if !valid_name(filename) {
            return Err(UploadError::Failed(format!(
                "“{filename}” can’t be stored on the server."
            )));
        }
        let unreadable = |error: io::Error| format!("Couldn’t read “{filename}”: {error}");
        // Opened and measured before `initiate`: a file that can't be read never takes one
        // of the server's 16 upload slots.
        let mut file = tokio::fs::File::open(local).await.map_err(unreadable)?;
        let total = file.metadata().await.map_err(unreadable)?.len();
        let (upload_id, chunks) = self
            .upload_chunks(&mut file, filename, total, STORAGE_CHUNK, true, sent)
            .await?;
        let route = format!("/companion/api/files/upload_to_dir/{home}");
        let body = json!({
            "path": dir,
            "filename": filename,
            "upload_id": upload_id,
            "total_chunks": chunks,
        });
        let mut attempt = 0;
        loop {
            // Keyed by the upload: a resend after a lost answer gets the first answer back
            // instead of "Upload session not found" (`security.py` replays keyed POSTs).
            let result = send_with_detail::<serde_json::Value>(
                self.request(Method::POST, &route)
                    .timeout(UPLOAD_CHUNK_TIMEOUT)
                    .header("X-Idempotency-Key", &upload_id)
                    .json(&body),
            )
            .await;
            match result {
                Ok(_) => return Ok(()),
                Err(refusal) if refusal.status.is_none() && attempt == 0 => attempt += 1,
                Err(refusal) if refusal.status == Some(StatusCode::NOT_FOUND) && attempt > 0 => {
                    return Err(UploadError::Failed(
                        "The upload may have finished; refresh to check.".into(),
                    ))
                }
                Err(refusal) => {
                    return Err(match refusal.key_refusal() {
                        Some(error) => UploadError::Refused(error),
                        None => UploadError::Failed(format!(
                            "The upload stopped: {}",
                            refusal.message()
                        )),
                    })
                }
            }
        }
    }

    /// One 2 MiB chunk of a stored file (`GET /companion/api/files/download/chunk/<home>`),
    /// decoded, and whether it is the last.
    pub async fn download_storage_chunk(
        &self,
        home: &str,
        path: &str,
        index: u64,
    ) -> Result<(Vec<u8>, bool), StorageError> {
        if !valid_path_segment(home) {
            return Err(StorageError::Invalid("Invalid home name.".into()));
        }
        let route = format!("/companion/api/files/download/chunk/{home}");
        let chunk: DownloadChunk = send_with_detail(
            self.request(Method::GET, &route)
                .timeout(UPLOAD_CHUNK_TIMEOUT)
                .query(&[("path", path), ("chunk_index", &index.to_string())]),
        )
        .await
        .map_err(|refusal| match refusal.status {
            None => StorageError::Network(refusal.message()),
            Some(status) if matches!(status.as_u16(), 502..=504) => {
                StorageError::Network(refusal.message())
            }
            Some(_) => StorageError::Api(refusal.api_error()),
        })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(chunk.chunk_data_b64.trim())
            .map_err(|_| StorageError::Invalid("The server sent a damaged chunk.".into()))?;
        Ok((bytes, chunk.is_last_chunk))
    }

    /// Downloads the stored file `path` of `home` to `destination` through its
    /// `.skerrydownload` partial. A partial left by an earlier attempt is resumed at its
    /// last whole chunk (SealSkin has no `Range`). `expected` is the listing's size: a file
    /// that ends at another size changed while it downloaded and fails. `progress` counts
    /// bytes written on top of what it held. (Tests: the listing's mtime reads empty.)
    #[cfg(test)]
    pub async fn download_storage(
        &self,
        home: &str,
        path: &str,
        destination: &Path,
        expected: Option<u64>,
        progress: &AtomicU64,
    ) -> Result<(), StorageError> {
        let source = expected.map(|size| PartialSource {
            size,
            mtime: String::new(),
        });
        self.download_storage_from(home, path, destination, source.as_ref(), progress)
            .await
    }

    /// [`Self::download_storage`] for the listed `source` (size and server mtime). A kept
    /// partial is resumed only when its `com.skerry.partial` tag names this home, path,
    /// size and mtime; any other partial (another home's file of the same name, a same-size
    /// edit, an untagged file) is discarded and the download starts at 0 (`ARCH-037`).
    pub async fn download_storage_from(
        &self,
        home: &str,
        path: &str,
        destination: &Path,
        source: Option<&PartialSource>,
        progress: &AtomicU64,
    ) -> Result<(), StorageError> {
        use tokio::io::{AsyncSeekExt as _, AsyncWriteExt as _};
        let expected = source.map(|source| source.size);
        let partial = crate::selkies::partial_path(destination);
        let tag = source.map(|source| partial_tag(home, path, source));
        let same_source = tag.is_some() && read_partial_tag(&partial) == tag;
        // The earlier attempt flushed and closed the partial, so its length is settled.
        let have = if same_source {
            tokio::fs::metadata(&partial)
                .await
                .map(|meta| meta.len())
                .unwrap_or(0)
        } else {
            0
        };
        let chunk = STORAGE_CHUNK as u64;
        let start = have / chunk;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&partial)
            .await?;
        file.set_len(start * chunk).await?;
        match &tag {
            Some(tag) => write_partial_tag(&partial, tag)?,
            None => remove_partial_tag(&partial),
        }
        file.seek(std::io::SeekFrom::Start(start * chunk)).await?;
        let base = progress.load(Ordering::Relaxed);
        let mut written = start * chunk;
        progress.store(base + written, Ordering::Relaxed);
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let streamed: Result<(), StorageError> = async {
            let mut index = start;
            loop {
                let (bytes, last) = self.download_storage_chunk(home, path, index).await?;
                if bytes.len() > STORAGE_CHUNK || (!last && bytes.len() < STORAGE_CHUNK) {
                    return Err(StorageError::Invalid(format!(
                        "The server sent a chunk of “{name}” of the wrong size."
                    )));
                }
                file.write_all(&bytes).await?;
                written += bytes.len() as u64;
                progress.store(base + written, Ordering::Relaxed);
                if last {
                    return Ok(());
                }
                index += 1;
            }
        }
        .await;
        // Settled even when the connection dropped: a retry measures the partial.
        file.flush().await?;
        drop(file);
        streamed?;
        if expected.is_some_and(|size| size != written) {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(StorageError::Invalid(format!(
                "“{name}” changed while downloading; try again."
            )));
        }
        // The finished file carries no partial tag.
        remove_partial_tag(&partial);
        tokio::fs::rename(&partial, destination).await?;
        Ok(())
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
        let response = self.public.get(url).send().await.ok()?;
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

/// Classifies a non-2xx response from `GET /companion/api/ui/version`:
/// only 404/405 mean the route is absent (older SealSkin or another app); 5xx and other
/// statuses mean the server or a proxy in front of it is down or errored.
fn classify_version_status(status: StatusCode) -> LibraryError {
    match status {
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => {
            LibraryError::Refused(unsupported_version(None))
        }
        _ if status.is_server_error() => LibraryError::Failed(format!(
            "The SealSkin server is unavailable (HTTP {}).",
            status.as_u16()
        )),
        _ => LibraryError::Failed(format!("SealSkin returned HTTP {}.", status.as_u16())),
    }
}

/// Sends a launch request, telling a lost request (`Network`) from a refusal (`Server`).
async fn send_launch<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, LaunchError> {
    let response = request.send().await.map_err(|error| {
        LaunchError::Network(
            if error.is_timeout() {
                "The server took too long to answer the launch request."
            } else {
                "The launch request didn’t reach the server."
            }
            .into(),
        )
    })?;
    let status = response.status();
    if status.is_success() {
        return response
            .json()
            .await
            .map_err(|_| LaunchError::Network("The server’s answer was cut off.".into()));
    }
    let detail = response
        .json::<Failure>()
        .await
        .ok()
        .map(|failure| failure.detail);
    match (status.as_u16(), detail) {
        // A proxy in front of SealSkin gave up on the long request; the launch goes on.
        (502..=504, None) => Err(LaunchError::Network(format!(
            "A gateway returned HTTP {} for the launch request.",
            status.as_u16()
        ))),
        (401 | 403, detail) => match classify(status, detail.as_deref()) {
            error if error.is_refusal() => Err(LaunchError::Refused(error)),
            error => Err(LaunchError::Server(error.to_string())),
        },
        (_, Some(detail)) if !detail.trim().is_empty() => Err(LaunchError::Server(detail)),
        _ => Err(LaunchError::Server(format!(
            "SealSkin returned HTTP {}.",
            status.as_u16()
        ))),
    }
}

/// Sends a key-bearing request and decodes its answer; every failure goes through
/// [`classify`], so a refused key is recognised wherever it shows up.
async fn send<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, ApiError> {
    let response = request.send().await.map_err(|error| {
        ApiError::Message(
            if error.is_timeout() {
                "The server took too long to respond. Refresh to check for running sessions."
            } else {
                "Couldn’t reach the SealSkin server. Check the address, network, and HTTPS certificate."
            }
            .into(),
        )
    })?;
    if response.status().is_success() {
        return response
            .json()
            .await
            .map_err(|_| ApiError::Message("The server returned an unexpected response.".into()));
    }
    Err(classify_response(response).await)
}

/// An in-process SealSkin stand-in for tests: it answers every request by its path.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::Mutex;

    /// Every request the fake saw: its head, lowercased.
    pub type Seen = Arc<Mutex<Vec<String>>>;

    pub fn reply(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// Serves until dropped (the test's runtime ends), answering each request with
    /// `answer(path)`, where `path` is the request line's target.
    pub async fn serve(answer: impl Fn(&str) -> String + Send + Sync + 'static) -> (Url, Seen) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let seen: Seen = Arc::default();
        let log = seen.clone();
        let answer = Arc::new(answer);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let (log, answer) = (log.clone(), answer.clone());
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(read) => request.extend_from_slice(&chunk[..read]),
                        }
                    }
                    let head = String::from_utf8_lossy(&request).to_string();
                    let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
                    log.lock().unwrap().push(head.to_lowercase());
                    let _ = stream.write_all(answer(&target).as_bytes()).await;
                });
            }
        });
        (base, seen)
    }

    pub fn client(base: Url) -> SealSkinClient {
        let credential = Credential::parse("ssk_id_secret@seal.example").unwrap();
        SealSkinClient::with_base(&credential, base)
    }
}

/// A request SealSkin refused (with its status and `detail`), or that never got an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Refusal {
    /// `None`: the request didn't reach the server or its answer was lost.
    status: Option<StatusCode>,
    detail: Option<String>,
    timed_out: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UploadStep {
    Initiate,
    Chunk,
}

impl Refusal {
    /// The general message: the key's problem for 401, else the server's detail.
    fn message(&self) -> String {
        match self.status {
            None if self.timed_out => "The server took too long to answer.".into(),
            None => "Couldn’t reach the SealSkin server.".into(),
            Some(StatusCode::UNAUTHORIZED) => {
                classify(StatusCode::UNAUTHORIZED, self.detail.as_deref()).to_string()
            }
            Some(status) => match self.detail.as_deref().map(str::trim) {
                Some(detail) if !detail.is_empty() => detail.to_string(),
                _ => format!("SealSkin returned HTTP {}.", status.as_u16()),
            },
        }
    }

    /// The key or account refusal this is, through D's classification (`ARCH-029`).
    fn key_refusal(&self) -> Option<ApiError> {
        let status = self
            .status
            .filter(|status| matches!(status.as_u16(), 401 | 403))?;
        Some(classify(status, self.detail.as_deref())).filter(ApiError::is_refusal)
    }

    /// Lost on the way (no answer) or a gateway's 502/503/504: a resend may work.
    fn transient(&self) -> bool {
        match self.status {
            None => true,
            Some(status) => matches!(status.as_u16(), 502..=504),
        }
    }

    /// Through [`classify`], as `send` reports failures; a lost request reads as one.
    fn api_error(&self) -> ApiError {
        match self.status {
            None => ApiError::Message(self.message()),
            Some(status) => match classify(status, self.detail.as_deref()) {
                ApiError::Message(_) => ApiError::Message(self.message()),
                error => error,
            },
        }
    }

    /// What a refused `POST /homedirs` means. SealSkin 0.5 answers 409 for a name that
    /// exists and also for one pydantic refused, so only "already exists" is `Exists`.
    fn home_error(&self) -> HomeError {
        match (self.status, self.detail.as_deref()) {
            (Some(StatusCode::CONFLICT), Some(detail))
                if detail.to_lowercase().contains("already exists") =>
            {
                HomeError::Exists
            }
            (Some(StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY), detail) => {
                HomeError::Invalid(
                    detail
                        .map(str::trim)
                        .filter(|detail| !detail.is_empty() && detail.len() < 200)
                        .map(str::to_string)
                        .unwrap_or_else(|| "The server didn’t accept that name.".into()),
                )
            }
            _ => HomeError::Api(self.api_error()),
        }
    }

    /// `storage`: an upload into persistent storage, where a 403 at `initiate` is the
    /// server's own sentence (quota, or storage turned off), shown as is (`ARCH-037`).
    fn upload_error_in(&self, step: UploadStep, filename: &str, storage: bool) -> UploadError {
        match self.key_refusal() {
            Some(error) => UploadError::Refused(error),
            None => UploadError::Failed(self.upload_message(step, filename, storage)),
        }
    }

    /// What an upload refusal means, by route and status (`ARCH-028`).
    fn upload_message(&self, step: UploadStep, filename: &str, storage: bool) -> String {
        let detail = self
            .detail
            .as_deref()
            .map(str::trim)
            .filter(|detail| !detail.is_empty());
        match (step, self.status.map(|status| status.as_u16())) {
            (UploadStep::Initiate, Some(403)) if storage && detail.is_some() => {
                detail.unwrap_or_default().to_string()
            }
            (UploadStep::Initiate, Some(403)) => {
                "Your SealSkin storage is full. Free space in Files, then try again.".into()
            }
            (UploadStep::Initiate, Some(413)) => {
                format!("“{filename}” is larger than this account may store.")
            }
            (UploadStep::Initiate, Some(429)) => {
                "Too many uploads are waiting on the server. Unfinished uploads clear after 24 hours.".into()
            }
            (UploadStep::Chunk, Some(413)) => {
                format!("“{filename}” changed while it was uploading.")
            }
            (_, Some(401)) => self.message(),
            _ => format!("The upload stopped: {}", self.message()),
        }
    }
}

/// Sends `request`, keeping the status and SealSkin's `detail` of a refusal, so each caller
/// maps statuses itself (`send` turns every 403 into a permission message).
async fn send_with_detail<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, Refusal> {
    let response = request.send().await.map_err(|error| Refusal {
        status: None,
        detail: None,
        timed_out: error.is_timeout(),
    })?;
    let status = response.status();
    if status.is_success() {
        // A body cut off on the way (a reset, the timeout mid-body) is a lost answer, so
        // retries and resume run; only a whole body that isn't the JSON expected is a refusal.
        let body = response.bytes().await.map_err(|error| Refusal {
            status: None,
            detail: None,
            timed_out: error.is_timeout(),
        })?;
        return serde_json::from_slice(&body).map_err(|_| Refusal {
            status: Some(status),
            detail: Some("The server returned an unexpected response.".into()),
            timed_out: false,
        });
    }
    let detail = response
        .json::<Failure>()
        .await
        .ok()
        .map(|failure| failure.detail);
    Err(Refusal {
        status: Some(status),
        detail,
        timed_out: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn key_refusals_are_classified_from_status_and_detail() {
        let unauthorized = |detail| classify(StatusCode::UNAUTHORIZED, detail);
        assert_eq!(
            unauthorized(Some("unknown")),
            ApiError::Key(KeyRefusal::Revoked)
        );
        assert_eq!(
            unauthorized(Some("expired")),
            ApiError::Key(KeyRefusal::Expired)
        );
        assert_eq!(
            unauthorized(Some("dormant")),
            ApiError::Key(KeyRefusal::Dormant)
        );
        assert_eq!(
            unauthorized(Some("disabled")),
            ApiError::Key(KeyRefusal::KeysDisabled)
        );
        assert_eq!(
            unauthorized(Some("client")),
            ApiError::Key(KeyRefusal::ClientNotAllowed)
        );
        // The key's user was deleted.
        assert_eq!(
            unauthorized(Some("Invalid token.")),
            ApiError::Key(KeyRefusal::Other("Invalid token.".into()))
        );
        assert_eq!(
            unauthorized(Some("The companion lane takes application keys alone.")),
            ApiError::Key(KeyRefusal::Other(
                "The companion lane takes application keys alone.".into()
            ))
        );
        // A proxy's HTML page has no detail.
        assert_eq!(
            unauthorized(None),
            ApiError::Key(KeyRefusal::Other(String::new()))
        );
        // The codes are exact and case-sensitive, as in SealSkin's own shells.
        for near in ["Dormant", " dormant", "dormant "] {
            assert!(
                matches!(
                    unauthorized(Some(near)),
                    ApiError::Key(KeyRefusal::Other(_))
                ),
                "{near:?}"
            );
        }
        assert_eq!(
            classify(StatusCode::TOO_MANY_REQUESTS, Some("locked")),
            ApiError::Message("SealSkin returned HTTP 429.".into())
        );
        assert_eq!(
            classify(StatusCode::INTERNAL_SERVER_ERROR, None),
            ApiError::Message("SealSkin returned HTTP 500.".into())
        );
    }

    #[test]
    fn sealskin_0_5_0_account_refusals_are_recognised_and_others_show_their_text() {
        // Pinned to SealSkin 0.5.0's `security.py` wording; only `held: true` is contract.
        assert_eq!(
            classify(StatusCode::FORBIDDEN, Some("User account is inactive.")),
            ApiError::Account(AccountBlock::Inactive)
        );
        assert_eq!(
            classify(
                StatusCode::FORBIDDEN,
                Some("This account waits for an administrator to place it in a group.")
            ),
            ApiError::Account(AccountBlock::Held)
        );
        let reworded = classify(StatusCode::FORBIDDEN, Some("Your account is paused."));
        assert_eq!(
            reworded,
            ApiError::Forbidden("Your account is paused.".into())
        );
        assert!(!reworded.is_refusal());
        assert_eq!(reworded.to_string(), "Your account is paused.");
        // Display never carries the key.
        for error in [
            ApiError::Key(KeyRefusal::Dormant),
            ApiError::Key(KeyRefusal::Other("x".into())),
        ] {
            assert!(!error.to_string().contains("ssk_"));
        }
    }

    #[test]
    fn library_prefers_the_held_flag_over_any_403_text() {
        let forbidden = || Err::<(), _>(ApiError::Forbidden("Some new wording.".into()));
        // Held comes from `held: true` even when the 403s were reworded.
        assert!(matches!(
            library_parts(forbidden(), forbidden(), Ok(true)),
            Err(LibraryError::Account(AccountBlock::Held))
        ));
        // A refused key wins over everything.
        let dormant = ApiError::Key(KeyRefusal::Dormant);
        assert!(matches!(
            library_parts(Ok(()), forbidden(), Err(&dormant)),
            Err(LibraryError::Key(KeyRefusal::Dormant))
        ));
        // The same unrecognised 403 everywhere: the account itself is refused, with its text.
        let same = ApiError::Forbidden("Some new wording.".into());
        match library_parts(forbidden(), forbidden(), Err(&same)) {
            Err(LibraryError::Account(AccountBlock::Other(text))) => {
                assert_eq!(text, "Some new wording.")
            }
            other => panic!("{other:?}"),
        }
        // A 403 on one request only is an ordinary failure.
        let failure = ApiError::Message("down".into());
        assert!(matches!(
            library_parts(forbidden(), Ok(()), Err(&failure)),
            Err(LibraryError::Failed(_))
        ));
        assert!(library_parts(Ok(()), Ok(()), Err(&failure)).is_ok());
    }

    #[tokio::test]
    async fn library_prefers_a_key_refusal_and_keeps_the_key_on_its_origin() {
        let (base, seen) = fake::serve(|path| {
            if path.starts_with("/companion/api/ui/version") {
                fake::reply("200 OK", r#"{"version":"0.5.0"}"#)
            } else if path.starts_with("/companion/api/applications") {
                fake::reply("401 Unauthorized", r#"{"detail":"dormant"}"#)
            } else if path.starts_with("/companion/api/sessions") {
                fake::reply("200 OK", "[]")
            } else {
                fake::reply("403 Forbidden", r#"{"detail":"whatever"}"#)
            }
        })
        .await;
        let client = fake::client(base.clone());
        assert!(matches!(
            client.library().await,
            Err(LibraryError::Key(KeyRefusal::Dormant))
        ));
        // One wake check, as the poll's tick sends it.
        assert!(client.key_works().await.is_err());
        let seen = seen.lock().unwrap().clone();
        assert!(seen.len() >= 4, "{seen:?}");
        for request in &seen {
            let target = request.split_whitespace().nth(1).unwrap_or("");
            assert!(target.starts_with("/companion/api/"), "{request}");
            assert!(!target.contains("wake"), "{request}");
            assert!(request.contains("authorization: bearer ssk_id_secret"));
            let host = format!("host: {}", base.authority());
            assert!(request.contains(&host), "{request}");
        }
    }

    #[tokio::test]
    async fn a_held_status_wins_over_reworded_403s() {
        let (base, _) = fake::serve(|path| {
            if path.starts_with("/companion/api/ui/version") {
                fake::reply("200 OK", r#"{"version":"0.5.0"}"#)
            } else if path.starts_with("/companion/api/admin/status") {
                fake::reply(
                    "200 OK",
                    r#"{"held":true,"username":"new","is_admin":false}"#,
                )
            } else {
                fake::reply("403 Forbidden", r#"{"detail":"Wait for a group, please."}"#)
            }
        })
        .await;
        assert!(matches!(
            fake::client(base).library().await,
            Err(LibraryError::Account(AccountBlock::Held))
        ));
    }

    #[tokio::test]
    async fn stop_and_home_creation_classify_a_refused_key() {
        let (base, _) = fake::serve(|path| {
            if path.starts_with("/companion/api/sessions/") {
                fake::reply("401 Unauthorized", r#"{"detail":"dormant"}"#)
            } else {
                fake::reply("401 Unauthorized", r#"{"detail":"expired"}"#)
            }
        })
        .await;
        let client = fake::client(base);
        assert_eq!(
            client.stop_session("abc").await,
            Err(ApiError::Key(KeyRefusal::Dormant))
        );
        let params = LaunchParams {
            home_name: Some("auto-firefox".into()),
            create_home: true,
            ..LaunchParams::default()
        };
        assert_eq!(
            client
                .launch_with_options(firefox(), params, || {})
                .await
                .err(),
            Some(LaunchError::Refused(ApiError::Key(KeyRefusal::Expired)))
        );
        let launch = client
            .launch_with_options(firefox(), LaunchParams::default(), || {})
            .await;
        assert_eq!(
            launch.err(),
            Some(LaunchError::Refused(ApiError::Key(KeyRefusal::Expired)))
        );
    }

    #[test]
    fn wake_and_web_urls_stay_on_the_control_plane_and_carry_no_key() {
        let credential = Credential::parse("ssk_abc123_s3cr3t@sealskin.example.com:8443").unwrap();
        let api = SealSkinClient::new(&credential).unwrap();
        let wake = api.wake_url();
        let web = api.web_url();
        assert_eq!(wake.as_str(), "https://sealskin.example.com:8443/?wake=1");
        assert_eq!(web.as_str(), "https://sealskin.example.com:8443/");
        for url in [&wake, &web] {
            assert_eq!(url.origin(), credential.base.origin());
            assert!(url.username().is_empty() && url.password().is_none());
            for secret in ["abc123", "s3cr3t", "ssk_"] {
                assert!(!url.as_str().contains(secret), "{url}");
            }
        }
        assert_eq!(api.key_id(), "abc123");
        assert_eq!(key_id_of("nonsense"), "");
    }

    #[test]
    fn decodes_the_account_limits_and_allowance() {
        let status: ServerStatus = serde_json::from_value(json!({
            "is_admin": false,
            "held": false,
            "username": "kebot",
            "via": "app_key",
            "settings": {
                "groups": ["users", "designers"],
                "persistent_storage": true, "public_sharing": false, "gpu": true, "vpn": false,
                "storage_limit": 50, "session_limit": 2.0, "session_cpus": 4,
                "session_memory_mb": 8192, "session_hours": 8.0, "session_idle_minutes": 30,
                "allowance_hours": 40, "allowance_period": "month",
                "application_key_days": -1, "application_key_dormant_hours": 24,
                "application_key_follow_sign_in": false
            },
            "allowance": {"hours": 40, "period": "month", "used": 12.5},
            "storage_used": 1073741824u64,
            "provider_groups": [], "clustered": false, "node_id": "",
            "gpus": [], "vpns": []
        }))
        .unwrap();
        assert_eq!(status.username, "kebot");
        assert_eq!(status.settings.groups, ["users", "designers"]);
        assert_eq!(status.settings.session_limit, 2);
        assert_eq!(status.settings.storage_limit, 50);
        assert_eq!(status.settings.session_cpus, 4.);
        assert_eq!(status.settings.application_key_dormant_hours, 24.);
        assert_eq!(status.allowance.as_ref().unwrap().used, 12.5);
        assert_eq!(status.storage_used, Some(1 << 30));
        // A limit that isn't a number doesn't lose the status (or its `held`).
        let odd: ServerStatus = serde_json::from_value(json!({
            "held": true, "username": "new",
            "settings": {"session_limit": null, "session_hours": "8", "storage_limit": 5.0}
        }))
        .unwrap();
        assert!(odd.held);
        assert_eq!(odd.settings.session_limit, -1);
        assert_eq!(odd.settings.session_hours, -1.);
        assert_eq!(odd.settings.storage_limit, 5);
        let minimal: ServerStatus = serde_json::from_value(json!({"username": "x"})).unwrap();
        assert_eq!(minimal.settings.session_limit, -1);
        assert_eq!(minimal.settings.application_key_dormant_hours, -1.);
        assert!(minimal.allowance.is_none() && minimal.storage_used.is_none());
    }

    const NOT_FOUND: &str =
        "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

    /// Answers `count` HTTP requests with 404 and returns each raw request, lowercased.
    async fn capture_requests(count: usize) -> (Url, tokio::task::JoinHandle<Vec<String>>) {
        answer_requests(vec![NOT_FOUND.to_string(); count]).await
    }

    /// Answers one request per response, in order, and returns each raw request (its
    /// head), lowercased.
    async fn answer_requests(
        responses: Vec<String>,
    ) -> (Url, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let task = tokio::spawn(async move {
            let mut seen = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let read = stream.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                }
                stream.write_all(response.as_bytes()).await.unwrap();
                seen.push(String::from_utf8_lossy(&request).to_lowercase());
            }
            seen
        });
        (base, task)
    }

    fn test_client(base: Url) -> SealSkinClient {
        let credential = Credential::parse("ssk_id_secret@seal.example").unwrap();
        SealSkinClient::with_base(&credential, base)
    }

    #[tokio::test]
    async fn logo_fetches_never_carry_the_application_key() {
        let (base, requests) = capture_requests(2).await;
        let app: RemoteApp = serde_json::from_value(json!({
            "id": "firefox",
            "name": "Firefox",
            "logo": "/api/logos/x.png",
            "is_meta_app": false,
        }))
        .unwrap();
        assert!(test_client(base).app_icon(&app).await.is_none());
        let requests = requests.await.unwrap();
        assert!(requests[0].starts_with("get /api/logos/x.png "));
        assert!(requests[1].starts_with("get /app/firefox/icon.png "));
        for request in requests {
            assert!(!request.contains("authorization:"), "{request}");
            assert!(!request.contains("ssk_id_secret"), "{request}");
        }
    }

    #[tokio::test]
    async fn companion_requests_carry_the_application_key() {
        let (base, requests) = capture_requests(1).await;
        assert!(matches!(
            test_client(base).check_version().await,
            Err(LibraryError::Refused(_))
        ));
        let request = requests.await.unwrap().remove(0);
        assert!(request.starts_with("get /companion/api/ui/version "));
        assert!(
            request.contains("authorization: bearer ssk_id_secret"),
            "{request}"
        );
    }

    fn reply(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn firefox() -> RemoteApp {
        serde_json::from_value(json!({"id": "firefox", "name": "Firefox"})).unwrap()
    }

    const LAUNCH_ID: &str = "0b9c3e2a-4f1d-4c8e-9a7b-2d6f5e1c3a90";

    #[tokio::test]
    async fn launch_request_sends_idempotency_key_equal_to_launch_id() {
        let (base, requests) = capture_requests(1).await;
        let params = LaunchParams {
            launch_id: Some(LAUNCH_ID.into()),
            home_name: Some("cleanroom".into()),
            ..LaunchParams::default()
        };
        let mut sent = false;
        let result = test_client(base)
            .launch_with_options(firefox(), params, || sent = true)
            .await;
        assert!(matches!(result, Err(LaunchError::Server(_))), "{result:?}");
        assert!(sent);
        let request = requests.await.unwrap().remove(0);
        assert!(request.starts_with("post /companion/api/launch/simple "));
        assert!(
            request.contains(&format!("x-idempotency-key: {LAUNCH_ID}")),
            "{request}"
        );
    }

    #[test]
    fn launch_body_carries_launch_id_when_following_progress() {
        let params = LaunchParams {
            launch_id: Some(LAUNCH_ID.into()),
            ..LaunchParams::default()
        };
        assert_eq!(params.body("firefox")["launch_id"], json!(LAUNCH_ID));
        assert!(LaunchParams::default()
            .body("firefox")
            .get("launch_id")
            .is_none());
    }

    #[tokio::test]
    async fn progress_401_is_signed_out_and_404_is_unknown() {
        let ready = r#"{"stage":"ready","detail":{},"elapsed":12.5,"session_url":"/s/abc/","session_id":"abc"}"#;
        let (base, requests) = answer_requests(vec![
            reply("401 Unauthorized", r#"{"detail":"expired"}"#),
            NOT_FOUND.to_string(),
            reply("200 OK", ready),
        ])
        .await;
        let client = test_client(base);
        assert_eq!(
            client.launch_progress(LAUNCH_ID).await,
            Ok(ProgressAnswer::SignedOut(KeyRefusal::Expired))
        );
        assert_eq!(
            client.launch_progress(LAUNCH_ID).await,
            Ok(ProgressAnswer::Unknown)
        );
        let Ok(ProgressAnswer::Known(progress)) = client.launch_progress(LAUNCH_ID).await else {
            panic!("no progress");
        };
        assert_eq!(progress.stage, "ready");
        assert_eq!(progress.session_id.as_deref(), Some("abc"));
        let requests = requests.await.unwrap();
        assert!(
            requests[0].starts_with(&format!("get /companion/api/launch/progress/{LAUNCH_ID} "))
        );
        assert!(requests[0].contains("authorization: bearer ssk_id_secret"));
    }

    #[tokio::test]
    async fn gateway_errors_count_as_a_lost_request() {
        let (base, _requests) = answer_requests(vec![
            reply("504 Gateway Timeout", "<html>timeout</html>"),
            reply("502 Bad Gateway", ""),
            reply(
                "503 Service Unavailable",
                r#"{"detail":"Launch capacity reached."}"#,
            ),
            reply(
                "400 Bad Request",
                r#"{"detail":"No GPU is free on any node."}"#,
            ),
        ])
        .await;
        let client = test_client(base);
        let launch = || {
            client.launch_with_options(
                firefox(),
                LaunchParams {
                    launch_id: Some(LAUNCH_ID.into()),
                    ..LaunchParams::default()
                },
                || {},
            )
        };
        assert!(matches!(launch().await, Err(LaunchError::Network(_))));
        assert!(matches!(launch().await, Err(LaunchError::Network(_))));
        assert_eq!(
            launch().await.err(),
            Some(LaunchError::Server("Launch capacity reached.".into()))
        );
        assert_eq!(
            launch().await.err(),
            Some(LaunchError::Server("No GPU is free on any node.".into()))
        );
    }

    #[test]
    fn decodes_launch_progress_stages_ready_and_failed() {
        let forwarding: LaunchProgress = serde_json::from_value(
            json!({"stage": "forwarding", "detail": {"node": "n2"}, "elapsed": 1.2}),
        )
        .unwrap();
        assert_eq!(forwarding.detail["node"], json!("n2"));
        assert_eq!(forwarding.session_url, None);
        let failed: LaunchProgress = serde_json::from_value(
            json!({"stage": "failed", "detail": {}, "elapsed": 41.0, "error": "No GPU"}),
        )
        .unwrap();
        assert_eq!(failed.error.as_deref(), Some("No GPU"));
        let minimal: LaunchProgress = serde_json::from_value(json!({"stage": "placing"})).unwrap();
        assert!(minimal.detail.is_empty());
    }

    #[tokio::test]
    async fn launch_progress_refuses_ids_that_are_not_uuids() {
        let client = test_client(Url::parse("http://127.0.0.1:9/").unwrap());
        for id in [
            "",
            "abc",
            "../admin",
            "0b9c3e2a-4f1d-4c8e-9a7b-2d6f5e1c3a9z",
        ] {
            assert!(!valid_launch_id(id), "{id}");
            assert!(client.launch_progress(id).await.is_err());
        }
        assert!(valid_launch_id(LAUNCH_ID));
        assert!(valid_launch_id(&uuid::Uuid::new_v4().to_string()));
    }

    #[test]
    fn session_from_progress_matches_what_the_post_would_return() {
        let params = LaunchParams {
            home_name: Some("auto-firefox".into()),
            selected_gpu: Some("intel-gpu".into()),
            launch_in_room_mode: true,
            ..LaunchParams::default()
        };
        let progress = LaunchProgress {
            stage: "ready".into(),
            session_id: Some("abc".into()),
            session_url: Some("/s/abc/".into()),
            ..LaunchProgress::default()
        };
        let from_progress = session_from_progress(&firefox(), &params, &progress).unwrap();
        let posted = launched_session(&firefox(), &params, "abc".into(), "/s/abc/".into());
        assert_eq!(from_progress.session_id, posted.session_id);
        assert_eq!(from_progress.session_url, posted.session_url);
        assert_eq!(from_progress.home, "auto-firefox");
        assert!(from_progress.gpu && from_progress.is_collaboration);
        let waiting = LaunchProgress {
            stage: "waiting".into(),
            ..progress
        };
        assert!(session_from_progress(&firefox(), &params, &waiting).is_none());
    }

    #[test]
    fn launched_session_has_a_current_created_at() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let session = launched_session(
            &firefox(),
            &LaunchParams {
                home_name: Some("cleanroom".into()),
                ..LaunchParams::default()
            },
            "abc".into(),
            "/s/abc/".into(),
        );
        assert!((session.created_at - now).abs() < 5.0);
        assert_eq!(session.home, "");
    }

    #[test]
    fn sessions_decode_is_collaboration_with_default_false() {
        let solo: RemoteSession = serde_json::from_value(json!({
            "session_id": "a", "app_name": "Zen", "session_url": "/a/"
        }))
        .unwrap();
        assert!(!solo.is_collaboration);
        let room: RemoteSession = serde_json::from_value(json!({
            "session_id": "b", "app_name": "Zen", "session_url": "/b/", "is_collaboration": true
        }))
        .unwrap();
        assert!(room.is_collaboration);
    }

    #[test]
    fn only_companion_api_paths_get_the_key() {
        assert!(requires_key("/companion/api/sessions"));
        assert!(!requires_key("/api/logos/a.png"));
        assert!(!requires_key("/app/x/icon.png"));
        assert!(!requires_key("/companion/apix"));
        // The check runs on the resolved path, after `..` is folded away.
        let mut url = Url::parse("https://seal.example/").unwrap();
        url.set_path("/companion/api/../../app/x/icon.png");
        assert!(!requires_key(url.path()), "{}", url.path());
    }

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
    fn version_check_http_errors_distinguish_server_down_from_old_sealskin() {
        for status in [StatusCode::NOT_FOUND, StatusCode::METHOD_NOT_ALLOWED] {
            match classify_version_status(status) {
                LibraryError::Refused(msg) => {
                    assert!(msg.contains("requires SealSkin 0.5.0 or newer"), "{msg}");
                }
                other => panic!("expected Refused for {status}, got {other:?}"),
            }
        }
        for status in [
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            match classify_version_status(status) {
                LibraryError::Failed(msg) => {
                    assert!(msg.contains("unavailable"), "{msg}");
                    assert!(msg.contains(&status.as_u16().to_string()), "{msg}");
                }
                other => panic!("expected Failed for {status}, got {other:?}"),
            }
        }
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            match classify_version_status(status) {
                LibraryError::Failed(msg) => {
                    assert!(msg.contains(&status.as_u16().to_string()), "{msg}");
                }
                other => panic!("expected Failed for {status}, got {other:?}"),
            }
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
            launch_id: None,
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

    // ---- Open links and files (ARCH-028) ------------------------------------------------

    /// One request a [`serve`] fake received: its head (lowercased) and its body.
    #[derive(Clone, Debug)]
    struct Recorded {
        head: String,
        body: Vec<u8>,
    }

    impl Recorded {
        fn json(&self) -> serde_json::Value {
            serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
        }

        fn is(&self, method_path: &str) -> bool {
            self.head.starts_with(&format!("{method_path} "))
        }
    }

    type Requests = Arc<std::sync::Mutex<Vec<Recorded>>>;

    /// A fake SealSkin that reads whole requests (head and body) and answers each with
    /// `respond`, until the test ends.
    async fn serve(
        mut respond: impl FnMut(&Recorded, usize) -> String + Send + 'static,
    ) -> (Url, Requests) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let seen: Requests = Arc::default();
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            let mut count = 0;
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut data = Vec::new();
                let mut chunk = vec![0u8; 64 * 1024];
                let head_end = loop {
                    if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                    let read = stream.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        break data.len();
                    }
                    data.extend_from_slice(&chunk[..read]);
                };
                let head = String::from_utf8_lossy(&data[..head_end]).to_lowercase();
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while data.len() < head_end + length {
                    let read = stream.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    data.extend_from_slice(&chunk[..read]);
                }
                let request = Recorded {
                    head,
                    body: data[head_end..].to_vec(),
                };
                let response = respond(&request, count);
                count += 1;
                log.lock().unwrap().push(request);
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        (base, seen)
    }

    const UPLOAD_ID: &str = "5f0c2b1a-8d3e-4c7f-9a6b-1e2d3c4b5a69";

    fn upload_server(request: &Recorded) -> String {
        if request.is("post /companion/api/upload/initiate") {
            reply("200 OK", &format!(r#"{{"upload_id":"{UPLOAD_ID}"}}"#))
        } else if request.is("post /companion/api/upload/chunk") {
            reply("200 OK", r#"{"status":"ok","chunk_index":0}"#)
        } else {
            reply(
                "200 OK",
                r#"{"session_id":"s1","session_url":"/s1/?access_token=t"}"#,
            )
        }
    }

    fn requests_of(seen: &Requests) -> Vec<Recorded> {
        seen.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn launch_url_body_and_path() {
        let (base, seen) = serve(|request, _| upload_server(request)).await;
        let params = LaunchParams {
            launch_id: Some(LAUNCH_ID.into()),
            home_name: Some("work".into()),
            ..LaunchParams::default()
        };
        let session = test_client(base)
            .launch(
                firefox(),
                params,
                LaunchRoute::Url("https://wiki.lan/page"),
                || {},
            )
            .await
            .unwrap();
        assert_eq!(session.session_id, "s1");
        let request = requests_of(&seen).remove(0);
        assert!(
            request.is("post /companion/api/launch/url"),
            "{}",
            request.head
        );
        assert!(request
            .head
            .contains(&format!("x-idempotency-key: {LAUNCH_ID}")));
        assert_eq!(
            request.json(),
            json!({
                "application_id": "firefox",
                "url": "https://wiki.lan/page",
                "launch_id": LAUNCH_ID,
                "home_name": "work",
                "wayland_mode": true,
                "launch_in_room_mode": false,
            })
        );
    }

    #[tokio::test]
    async fn upload_for_launch_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.pdf");
        let bytes: Vec<u8> = (0..2_621_440u32).map(|n| (n % 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let (base, seen) = serve(|request, _| upload_server(request)).await;
        let sent = AtomicU64::new(0);
        let uploaded = test_client(base)
            .upload_for_launch(&path, &sent)
            .await
            .unwrap();
        assert_eq!(uploaded.total_chunks, 3);
        assert_eq!(uploaded.filename, "report.pdf");
        assert_eq!(uploaded.upload_id, UPLOAD_ID);
        assert_eq!(sent.load(Ordering::Relaxed), 2_621_440);
        let requests = requests_of(&seen);
        assert_eq!(
            requests[0].json(),
            json!({"filename": "report.pdf", "total_size": 2_621_440})
        );
        let mut joined = Vec::new();
        for (index, chunk) in requests[1..].iter().enumerate() {
            assert!(chunk.is("post /companion/api/upload/chunk"));
            let body = chunk.json();
            assert_eq!(body["upload_id"], json!(UPLOAD_ID));
            assert_eq!(body["chunk_index"], json!(index));
            let data = base64::engine::general_purpose::STANDARD
                .decode(body["chunk_data_b64"].as_str().unwrap())
                .unwrap();
            if index < 2 {
                assert_eq!(data.len(), 1_048_576);
            }
            joined.extend(data);
        }
        assert_eq!(requests.len(), 4);
        assert_eq!(joined, bytes);
    }

    #[tokio::test]
    async fn an_empty_file_uploads_no_chunks_and_still_launches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.txt");
        std::fs::write(&path, b"").unwrap();
        let (base, seen) = serve(|request, _| upload_server(request)).await;
        let api = test_client(base);
        let uploaded = api
            .upload_for_launch(&path, &AtomicU64::new(0))
            .await
            .unwrap();
        assert_eq!(uploaded.total_chunks, 0);
        api.launch(
            firefox(),
            LaunchParams::default(),
            LaunchRoute::File(&uploaded),
            || {},
        )
        .await
        .unwrap();
        let requests = requests_of(&seen);
        assert_eq!(requests.len(), 2);
        assert!(requests[1].is("post /companion/api/launch/file"));
        assert_eq!(requests[1].json()["total_chunks"], json!(0));
    }

    #[tokio::test]
    async fn launch_file_body() {
        let (base, seen) = serve(|request, _| upload_server(request)).await;
        let uploaded = Uploaded {
            upload_id: UPLOAD_ID.into(),
            total_chunks: 3,
            filename: "report.pdf".into(),
        };
        test_client(base)
            .launch(
                firefox(),
                LaunchParams::default(),
                LaunchRoute::File(&uploaded),
                || {},
            )
            .await
            .unwrap();
        let request = requests_of(&seen).remove(0);
        assert!(request.is("post /companion/api/launch/file"));
        let body = request.json();
        assert_eq!(body["filename"], json!("report.pdf"));
        assert_eq!(body["upload_id"], json!(UPLOAD_ID));
        assert_eq!(body["total_chunks"], json!(3));
        assert_eq!(body["open_file_on_launch"], json!(true));
        assert_eq!(body["application_id"], json!("firefox"));
    }

    #[tokio::test]
    async fn upload_for_launch_sends_only_the_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        let path = nested.join("notes.txt");
        std::fs::write(&path, b"hello").unwrap();
        let (base, seen) = serve(|request, _| upload_server(request)).await;
        test_client(base)
            .upload_for_launch(&path, &AtomicU64::new(0))
            .await
            .unwrap();
        assert_eq!(requests_of(&seen)[0].json()["filename"], json!("notes.txt"));
    }

    #[tokio::test]
    async fn upload_for_launch_refuses_a_file_that_grows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("growing.log");
        std::fs::write(&path, vec![7u8; 1_500_000]).unwrap();
        let grow = path.clone();
        let (base, seen) = serve(move |request, _| {
            if request.is("post /companion/api/upload/chunk") {
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&grow)
                    .unwrap();
                std::io::Write::write_all(&mut file, &[1u8; 10]).unwrap();
            }
            upload_server(request)
        })
        .await;
        let error = test_client(base)
            .upload_for_launch(&path, &AtomicU64::new(0))
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(error, "“growing.log” changed while it was uploading.");
        assert!(!requests_of(&seen)
            .iter()
            .any(|r| r.is("post /companion/api/launch/file")));
    }

    #[tokio::test]
    async fn upload_errors_map_by_status() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.pdf");
        std::fs::write(&path, b"data").unwrap();
        let cases = [
            (
                "initiate",
                "403 Forbidden",
                "Your SealSkin storage is full. Free space in Files, then try again.",
            ),
            (
                "initiate",
                "413 Payload Too Large",
                "“report.pdf” is larger than this account may store.",
            ),
            (
                "initiate",
                "429 Too Many Requests",
                "Too many uploads are waiting on the server. Unfinished uploads clear after 24 hours.",
            ),
            (
                "chunk",
                "413 Payload Too Large",
                "“report.pdf” changed while it was uploading.",
            ),
            (
                "chunk",
                "500 Internal Server Error",
                "The upload stopped: Disk on fire",
            ),
        ];
        for (step, status, expected) in cases {
            let (base, _) = serve(move |request, _| {
                if request.is(&format!("post /companion/api/upload/{step}")) {
                    reply(status, r#"{"detail":"Disk on fire"}"#)
                } else {
                    upload_server(request)
                }
            })
            .await;
            let error = test_client(base)
                .upload_for_launch(&path, &AtomicU64::new(0))
                .await
                .unwrap_err()
                .to_string();
            assert_eq!(error, expected, "{step} {status}");
        }
        // Nothing listening: the chunk request never reaches the server.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        drop(listener);
        let error = test_client(base)
            .upload_for_launch(&path, &AtomicU64::new(0))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("The upload stopped: "), "{error}");
    }

    // Multi-threaded: the fake server holds chunk 1 on its own thread while the abort lands.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn aborting_an_upload_stops_chunks_and_the_launch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        std::fs::write(&path, vec![3u8; 3 * 1_048_576]).unwrap();
        let (chunk_tx, chunk_rx) = std::sync::mpsc::channel::<()>();
        let (base, seen) = serve(move |request, _| {
            if request.is("post /companion/api/upload/chunk")
                && request.json()["chunk_index"] == json!(1)
            {
                let _ = chunk_tx.send(());
                // Hold chunk 1 so the abort lands while it is outstanding.
                std::thread::sleep(Duration::from_millis(300));
            }
            upload_server(request)
        })
        .await;
        let api = test_client(base);
        let job = tokio::spawn(async move {
            let uploaded = api
                .upload_for_launch(&path, &AtomicU64::new(0))
                .await
                .map_err(|e| e.to_string())?;
            api.launch(
                firefox(),
                LaunchParams::default(),
                LaunchRoute::File(&uploaded),
                || {},
            )
            .await
            .map_err(|_| "launch failed".to_string())
        });
        let abort = job.abort_handle();
        tokio::task::spawn_blocking(move || chunk_rx.recv().unwrap())
            .await
            .unwrap();
        abort.abort();
        tokio::time::sleep(Duration::from_millis(600)).await;
        let requests = requests_of(&seen);
        assert!(
            !requests.iter().any(|r| r.json()["chunk_index"] == json!(2)),
            "{requests:?}"
        );
        assert!(!requests
            .iter()
            .any(|r| r.is("post /companion/api/launch/file")));
    }

    #[tokio::test]
    async fn open_url_answers_by_status() {
        let cases: [(&str, Result<(), OpenUrlError>); 5] = [
            ("200 OK", Ok(())),
            ("409 Conflict", Err(OpenUrlError::NotTaken)),
            ("404 Not Found", Err(OpenUrlError::NotTaken)),
            ("422 Unprocessable Entity", Err(OpenUrlError::Refused)),
            (
                "500 Internal Server Error",
                Err(OpenUrlError::Failed("SealSkin returned HTTP 500.".into())),
            ),
        ];
        for (status, expected) in cases {
            let (base, seen) = serve(move |_, _| reply(status, r#"{"status":"opened"}"#)).await;
            let result = test_client(base)
                .open_url_in_session("abc-123", "https://a.com")
                .await;
            assert_eq!(result, expected, "{status}");
            let request = requests_of(&seen).remove(0);
            assert!(request.is("post /companion/api/sessions/abc-123/open_url"));
            assert_eq!(request.json(), json!({"url": "https://a.com"}));
        }
        let (base, seen) = serve(|_, _| reply("200 OK", "{}")).await;
        assert!(matches!(
            test_client(base)
                .open_url_in_session("../admin", "https://a.com")
                .await,
            Err(OpenUrlError::Failed(_))
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(requests_of(&seen).is_empty());
    }

    #[tokio::test]
    async fn open_routes_carry_the_application_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.pdf");
        std::fs::write(&path, b"data").unwrap();
        let (base, seen) = serve(|request, _| upload_server(request)).await;
        let api = test_client(base);
        let uploaded = api
            .upload_for_launch(&path, &AtomicU64::new(0))
            .await
            .unwrap();
        for route in [
            LaunchRoute::File(&uploaded),
            LaunchRoute::Url("https://a.com"),
        ] {
            api.launch(firefox(), LaunchParams::default(), route, || {})
                .await
                .unwrap();
        }
        let _ = api.open_url_in_session("s1", "https://a.com").await;
        let requests = requests_of(&seen);
        let paths: Vec<&str> = requests
            .iter()
            .map(|r| r.head.split(' ').nth(1).unwrap())
            .collect();
        assert_eq!(
            paths,
            [
                "/companion/api/upload/initiate",
                "/companion/api/upload/chunk",
                "/companion/api/launch/file",
                "/companion/api/launch/url",
                "/companion/api/sessions/s1/open_url",
            ]
        );
        for request in requests {
            assert!(
                request.head.contains("authorization: bearer ssk_id_secret"),
                "{}",
                request.head
            );
        }
    }

    #[test]
    fn apps_decode_url_support_and_extensions_with_defaults() {
        let app: RemoteApp = serde_json::from_value(json!({
            "id": "office", "name": "Office", "url_support": true, "extensions": [".pdf", "docx"],
        }))
        .unwrap();
        assert!(app.url_support);
        assert_eq!(app.extensions, [".pdf", "docx"]);
        let bare = firefox();
        assert!(!bare.url_support);
        assert!(bare.extensions.is_empty());
    }

    #[tokio::test]
    async fn open_routes_report_a_refused_key_through_the_classification() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.pdf");
        std::fs::write(&path, b"data").unwrap();
        let (base, _) = serve(|_, _| reply("401 Unauthorized", r#"{"detail":"expired"}"#)).await;
        let api = test_client(base);
        assert_eq!(
            api.upload_for_launch(&path, &AtomicU64::new(0)).await,
            Err(UploadError::Refused(ApiError::Key(KeyRefusal::Expired)))
        );
        assert_eq!(
            api.open_url_in_session("s1", "https://a.com").await,
            Err(OpenUrlError::Key(ApiError::Key(KeyRefusal::Expired)))
        );
        let launched = api
            .launch(
                firefox(),
                LaunchParams::default(),
                LaunchRoute::Url("https://a.com"),
                || {},
            )
            .await;
        assert_eq!(
            launched.err(),
            Some(LaunchError::Refused(ApiError::Key(KeyRefusal::Expired)))
        );
        // A held account (403) is a refusal; a storage 403 is the upload's own message.
        let held = format!(r#"{{"detail":"{HELD_DETAIL}"}}"#);
        let (base, _) = serve(move |_, _| reply("403 Forbidden", &held)).await;
        assert_eq!(
            test_client(base)
                .upload_for_launch(&path, &AtomicU64::new(0))
                .await,
            Err(UploadError::Refused(ApiError::Account(AccountBlock::Held)))
        );
        let (base, _) =
            serve(|_, _| reply("403 Forbidden", r#"{"detail":"Storage limit reached."}"#)).await;
        assert_eq!(
            test_client(base)
                .upload_for_launch(&path, &AtomicU64::new(0))
                .await,
            Err(UploadError::Failed(
                "Your SealSkin storage is full. Free space in Files, then try again.".into()
            ))
        );
    }

    #[test]
    fn upload_ids_are_lowercase_uuids() {
        assert!(valid_upload_id(UPLOAD_ID));
        assert!(!valid_upload_id(&UPLOAD_ID.to_uppercase()));
        assert!(!valid_upload_id("../x"));
    }

    // ---- Persistent storage (`ARCH-037`) ------------------------------------------------

    fn query_value(request: &Recorded, name: &str) -> Option<String> {
        let target = request
            .head
            .split_whitespace()
            .nth(1)
            .unwrap_or("")
            .to_string();
        let url = Url::parse(&format!("http://x{target}")).ok()?;
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }

    const NO_CONTENT: &str =
        "HTTP/1.1 204 No Content\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

    #[tokio::test]
    async fn storage_list_reads_every_page_and_keeps_epoch_times() {
        let (base, seen) = serve(|request, _| {
            let page = query_value(request, "page").unwrap();
            let body = if page == "1" {
                r#"{"items":[{"name":"Docs","path":"/Docs","is_dir":true,"size":4096,"mtime":1795341845.5},{"name":"a.txt","path":"/a.txt","is_dir":false,"size":3,"mtime":1.0}],"total":3}"#
            } else {
                r#"{"items":[{"name":"a.txt","path":"/a.txt","is_dir":false,"size":3,"mtime":1.0},{"name":"b.txt","path":"/b.txt","is_dir":false,"size":7,"mtime":2.0}],"total":3}"#
            };
            reply("200 OK", body)
        })
        .await;
        let listing = test_client(base).list_storage("work", "/").await.unwrap();
        assert!(!listing.truncated);
        let names: Vec<_> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["Docs", "a.txt", "b.txt"],
            "a repeated name is kept once"
        );
        assert!(listing.entries[0].is_dir);
        assert_eq!(listing.entries[0].mtime, 1_795_341_845.5);
        let requests = requests_of(&seen);
        assert!(
            requests[0].is("get /companion/api/files/list/work?path=%2f&page=1&per_page=200"),
            "{}",
            requests[0].head
        );
        assert!(requests[0]
            .head
            .contains("authorization: bearer ssk_id_secret"));
    }

    #[tokio::test]
    async fn storage_list_stops_at_ten_thousand_items() {
        let (base, seen) = serve(|request, _| {
            let page: u32 = query_value(request, "page").unwrap().parse().unwrap();
            let items: Vec<String> = (0..200)
                .map(|n| format!(r#"{{"name":"f{page}-{n}","is_dir":false,"size":1,"mtime":0}}"#))
                .collect();
            reply(
                "200 OK",
                &format!(r#"{{"items":[{}],"total":20000}}"#, items.join(",")),
            )
        })
        .await;
        let client = test_client(base);
        let listing = client.list_storage("work", "/").await.unwrap();
        assert!(listing.truncated);
        assert_eq!(listing.entries.len(), 10_000);
        assert_eq!(requests_of(&seen).len(), 50);
        // `ARCH-024`'s comparison never sees a partial list.
        assert!(client.storage_names("work", "/Desktop").await.is_err());
    }

    #[tokio::test]
    async fn storage_names_still_compares_the_desktop() {
        let (base, seen) = serve(|_, _| {
            reply(
                "200 OK",
                r#"{"items":[{"name":"a.txt"},{"name":"Work","is_dir":true}],"total":2}"#,
            )
        })
        .await;
        let names = test_client(base)
            .storage_names("work", "/Desktop")
            .await
            .unwrap();
        assert_eq!(names, ["a.txt", "Work"]);
        assert!(requests_of(&seen)[0]
            .is("get /companion/api/files/list/work?path=%2fdesktop&page=1&per_page=200"));
    }

    #[tokio::test]
    async fn create_storage_folder_body_and_existing_folder() {
        let (base, seen) = serve(|request, _| {
            if request.is("post /companion/api/files/create_folder/work") {
                match request.json()["folder_name"].as_str() {
                    Some("New") => reply("201 Created", r#"{"status":"success"}"#),
                    _ => reply("409 Conflict", r#"{"detail":"Folder already exists."}"#),
                }
            } else {
                reply(
                    "200 OK",
                    r#"{"items":[{"name":"Old","is_dir":true},{"name":"note","is_dir":false}],"total":2}"#,
                )
            }
        })
        .await;
        let client = test_client(base);
        assert_eq!(
            client.create_storage_folder("work", "/Docs", "New").await,
            Ok(Created::New)
        );
        assert_eq!(
            requests_of(&seen)[0].json(),
            json!({"path": "/Docs", "folder_name": "New"})
        );
        assert_eq!(
            client.create_storage_folder("work", "/Docs", "Old").await,
            Ok(Created::Existed)
        );
        assert_eq!(
            client
                .create_storage_folder("work", "/Docs", "note")
                .await
                .unwrap_err()
                .to_string(),
            "A file named “note” is in the way."
        );
        assert!(client
            .create_storage_folder("work", "/", "a/b")
            .await
            .is_err());
    }

    fn storage_upload_server(request: &Recorded) -> String {
        if request.is("post /companion/api/files/upload_to_dir/work") {
            reply("200 OK", r#"{"status":"success"}"#)
        } else {
            upload_server(request)
        }
    }

    #[tokio::test]
    async fn storage_upload_initiates_chunks_in_order_then_places_into_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local name.bin");
        let bytes: Vec<u8> = (0..5_000_000u32).map(|n| (n % 253) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let (base, seen) = serve(|request, _| storage_upload_server(request)).await;
        let sent = AtomicU64::new(0);
        test_client(base)
            .upload_to_storage("work", "/Docs", &path, "report.bin", &sent)
            .await
            .unwrap();
        assert_eq!(sent.load(Ordering::Relaxed), 5_000_000);
        let requests = requests_of(&seen);
        assert_eq!(
            requests[0].json(),
            json!({"filename": "report.bin", "total_size": 5_000_000})
        );
        assert!(requests[0].head.contains("x-idempotency-key: "));
        let mut joined = Vec::new();
        for (index, chunk) in requests[1..4].iter().enumerate() {
            assert!(chunk.is("post /companion/api/upload/chunk"));
            let body = chunk.json();
            assert_eq!(body["chunk_index"], json!(index));
            let data = base64::engine::general_purpose::STANDARD
                .decode(body["chunk_data_b64"].as_str().unwrap())
                .unwrap();
            if index < 2 {
                assert_eq!(data.len(), STORAGE_CHUNK);
            }
            joined.extend(data);
        }
        assert_eq!(joined, bytes);
        assert!(requests[4].is("post /companion/api/files/upload_to_dir/work"));
        assert_eq!(
            requests[4].json(),
            json!({"path": "/Docs", "filename": "report.bin", "upload_id": UPLOAD_ID, "total_chunks": 3})
        );
        assert_eq!(requests.len(), 5);
    }

    #[tokio::test]
    async fn storage_finalize_carries_the_upload_id_as_idempotency_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"hello").unwrap();
        // Every finalize answer is lost; the one resend is keyed by the upload.
        let (base, seen) = serve(|request, _| {
            if request.is("post /companion/api/files/upload_to_dir/work") {
                String::new()
            } else {
                storage_upload_server(request)
            }
        })
        .await;
        let error = test_client(base)
            .upload_to_storage("work", "/", &path, "a.txt", &AtomicU64::new(0))
            .await
            .unwrap_err();
        let finals: Vec<_> = requests_of(&seen)
            .into_iter()
            .filter(|r| r.is("post /companion/api/files/upload_to_dir/work"))
            .collect();
        assert_eq!(finals.len(), 2, "resent once after a lost answer");
        for request in &finals {
            assert!(request
                .head
                .contains(&format!("x-idempotency-key: {UPLOAD_ID}")));
        }
        assert!(
            error.to_string().starts_with("The upload stopped"),
            "{error}"
        );
        // A 404 after the resend: the first attempt may have placed the file.
        let calls = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&calls);
        let (base, _) = serve(move |request, _| {
            if request.is("post /companion/api/files/upload_to_dir/work") {
                if counter.fetch_add(1, Ordering::Relaxed) == 0 {
                    String::new()
                } else {
                    reply("404 Not Found", r#"{"detail":"Upload session not found."}"#)
                }
            } else {
                storage_upload_server(request)
            }
        })
        .await;
        let error = test_client(base)
            .upload_to_storage("work", "/", &path, "a.txt", &AtomicU64::new(0))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "The upload may have finished; refresh to check."
        );
    }

    #[tokio::test]
    async fn storage_upload_retries_a_chunk_after_a_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, vec![9u8; 3_000_000]).unwrap();
        let resets = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&resets);
        let (base, seen) = serve(move |request, _| {
            if request.is("post /companion/api/upload/chunk")
                && request.json()["chunk_index"] == json!(1)
                && counter.fetch_add(1, Ordering::Relaxed) == 0
            {
                return String::new();
            }
            storage_upload_server(request)
        })
        .await;
        test_client(base)
            .upload_to_storage("work", "/", &path, "a.bin", &AtomicU64::new(0))
            .await
            .unwrap();
        let indexes: Vec<_> = requests_of(&seen)
            .iter()
            .filter(|r| r.is("post /companion/api/upload/chunk"))
            .map(|r| r.json()["chunk_index"].as_u64().unwrap())
            .collect();
        assert_eq!(indexes, [0, 1, 1]);
    }

    #[tokio::test]
    async fn storage_upload_does_not_retry_a_4xx_chunk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, b"abc").unwrap();
        let (base, seen) = serve(|request, _| {
            if request.is("post /companion/api/upload/chunk") {
                reply("400 Bad Request", r#"{"detail":"Bad chunk."}"#)
            } else {
                storage_upload_server(request)
            }
        })
        .await;
        let error = test_client(base)
            .upload_to_storage("work", "/", &path, "a.bin", &AtomicU64::new(0))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "The upload stopped: Bad chunk.");
        let chunks = requests_of(&seen)
            .iter()
            .filter(|r| r.is("post /companion/api/upload/chunk"))
            .count();
        assert_eq!(chunks, 1);
    }

    #[tokio::test]
    async fn storage_upload_shows_the_quota_and_size_sentences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.iso");
        std::fs::write(&path, b"abc").unwrap();
        let cases = [
            (
                "403 Forbidden",
                r#"{"detail":"Storage limit reached: 50 GB."}"#,
                "Storage limit reached: 50 GB.",
            ),
            (
                "413 Payload Too Large",
                r#"{"detail":"x"}"#,
                "“big.iso” is larger than this account may store.",
            ),
            (
                "429 Too Many Requests",
                r#"{"detail":"x"}"#,
                "Too many uploads are waiting on the server. Unfinished uploads clear after 24 hours.",
            ),
        ];
        for (status, body, expected) in cases {
            let (base, seen) = serve(move |request, _| {
                if request.is("post /companion/api/upload/initiate") {
                    reply(status, body)
                } else {
                    storage_upload_server(request)
                }
            })
            .await;
            let error = test_client(base)
                .upload_to_storage("work", "/", &path, "big.iso", &AtomicU64::new(0))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), expected, "{status}");
            assert_eq!(requests_of(&seen).len(), 1, "{status}");
        }
        // A file that can't be read never takes an upload slot.
        let (base, seen) = serve(|request, _| storage_upload_server(request)).await;
        let missing = dir.path().join("missing");
        assert!(test_client(base)
            .upload_to_storage("work", "/", &missing, "m", &AtomicU64::new(0))
            .await
            .is_err());
        assert!(requests_of(&seen).is_empty());
    }

    #[tokio::test]
    async fn storage_upload_refuses_a_file_that_changes_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("growing.log");
        std::fs::write(&path, vec![7u8; 2_500_000]).unwrap();
        let grow = path.clone();
        let (base, seen) = serve(move |request, _| {
            if request.is("post /companion/api/upload/chunk") {
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&grow)
                    .unwrap();
                std::io::Write::write_all(&mut file, &[1u8; 10]).unwrap();
            }
            storage_upload_server(request)
        })
        .await;
        let error = test_client(base)
            .upload_to_storage("work", "/", &path, "growing.log", &AtomicU64::new(0))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "“growing.log” changed while it was uploading."
        );
        assert!(!requests_of(&seen)
            .iter()
            .any(|r| r.is("post /companion/api/files/upload_to_dir/work")));
    }

    #[tokio::test]
    async fn launch_upload_still_sends_one_mib_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, vec![1u8; 1_500_000]).unwrap();
        let (base, seen) = serve(|request, _| upload_server(request)).await;
        let uploaded = test_client(base)
            .upload_for_launch(&path, &AtomicU64::new(0))
            .await
            .unwrap();
        assert_eq!(uploaded.total_chunks, 2);
        assert!(!requests_of(&seen)[0].head.contains("x-idempotency-key"));
    }

    type Indexes = Arc<std::sync::Mutex<Vec<u64>>>;

    /// A stored file served in 2 MiB chunks, recording each chunk index asked for.
    fn chunk_server(
        data: Vec<u8>,
        indexes: Indexes,
    ) -> impl FnMut(&Recorded, usize) -> String + Send + 'static {
        move |request, _| {
            let index: u64 = query_value(request, "chunk_index")
                .unwrap()
                .parse()
                .unwrap();
            indexes.lock().unwrap().push(index);
            let start = (index as usize * STORAGE_CHUNK).min(data.len());
            let end = (start + STORAGE_CHUNK).min(data.len());
            let chunk = &data[start..end];
            reply(
                "200 OK",
                &json!({
                    "chunk_data_b64": base64::engine::general_purpose::STANDARD.encode(chunk),
                    "is_last_chunk": chunk.len() < STORAGE_CHUNK,
                })
                .to_string(),
            )
        }
    }

    #[tokio::test]
    async fn storage_download_resumes_at_a_chunk_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.bin");
        let data: Vec<u8> = (0..5_000_000u32).map(|n| (n % 241) as u8).collect();
        // An earlier attempt left 2 MiB plus a torn part of the next chunk.
        let partial = crate::selkies::partial_path(&destination);
        let mut torn = data[..STORAGE_CHUNK + 1000].to_vec();
        torn[STORAGE_CHUNK + 10] ^= 0xff;
        std::fs::write(&partial, &torn).unwrap();
        let source = PartialSource {
            size: 5_000_000,
            mtime: String::new(),
        };
        write_partial_tag(&partial, &partial_tag("work", "/a.bin", &source)).unwrap();
        let indexes = Indexes::default();
        let (base, seen) = serve(chunk_server(data.clone(), Arc::clone(&indexes))).await;
        let progress = AtomicU64::new(0);
        test_client(base)
            .download_storage("work", "/a.bin", &destination, Some(5_000_000), &progress)
            .await
            .unwrap();
        assert_eq!(*indexes.lock().unwrap(), [1, 2]);
        assert_eq!(std::fs::read(&destination).unwrap(), data);
        assert!(!partial.exists());
        assert_eq!(progress.load(Ordering::Relaxed), 5_000_000);
        let request = &requests_of(&seen)[0];
        assert!(
            request.is("get /companion/api/files/download/chunk/work?path=%2fa.bin&chunk_index=1"),
            "{}",
            request.head
        );
    }

    #[tokio::test]
    async fn storage_download_ends_on_a_short_or_empty_last_chunk() {
        let dir = tempfile::tempdir().unwrap();
        for size in [STORAGE_CHUNK * 2, 10, 0] {
            let destination = dir.path().join(format!("f{size}"));
            let data = vec![3u8; size];
            let indexes = Indexes::default();
            let (base, _) = serve(chunk_server(data.clone(), Arc::clone(&indexes))).await;
            test_client(base)
                .download_storage(
                    "work",
                    "/f",
                    &destination,
                    Some(size as u64),
                    &AtomicU64::new(0),
                )
                .await
                .unwrap();
            assert_eq!(std::fs::read(&destination).unwrap(), data, "{size}");
            assert_eq!(
                indexes.lock().unwrap().len(),
                size / STORAGE_CHUNK + 1,
                "{size}"
            );
        }
    }

    #[tokio::test]
    async fn storage_download_refuses_bad_base64_and_short_middle_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a");
        for body in [
            r#"{"chunk_data_b64":"%%%","is_last_chunk":true}"#,
            r#"{"chunk_data_b64":"YWJj","is_last_chunk":false}"#,
        ] {
            let (base, _) = serve(move |_, _| reply("200 OK", body)).await;
            let error = test_client(base)
                .download_storage("work", "/a", &destination, None, &AtomicU64::new(0))
                .await
                .unwrap_err();
            assert!(matches!(error, StorageError::Invalid(_)), "{error}");
            assert!(!destination.exists());
        }
    }

    #[tokio::test]
    async fn a_chunk_cut_off_mid_body_is_a_network_error_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.bin");
        let data: Vec<u8> = (0..3_000_000u32).map(|n| (n % 239) as u8).collect();
        let indexes = Indexes::default();
        let mut whole = chunk_server(data.clone(), Arc::clone(&indexes));
        let cuts = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&cuts);
        let (base, _) = serve(move |request, n| {
            let answer = whole(request, n);
            // The second chunk's first answer promises its length but stops halfway.
            if answer.contains("\"is_last_chunk\":true")
                && counter.fetch_add(1, Ordering::Relaxed) == 0
            {
                let cut = answer.len() / 2;
                return answer[..cut].to_string();
            }
            answer
        })
        .await;
        let client = test_client(base);
        let progress = AtomicU64::new(0);
        let error = client
            .download_storage("work", "/a.bin", &destination, Some(3_000_000), &progress)
            .await
            .unwrap_err();
        assert!(matches!(error, StorageError::Network(_)), "{error}");
        // The retry resumes at the whole first chunk and finishes.
        client
            .download_storage("work", "/a.bin", &destination, Some(3_000_000), &progress)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), data);
        assert_eq!(*indexes.lock().unwrap(), [0, 1, 1]);
    }

    /// A partial of 2 MiB + a bit, tagged as from `home`/`path` at `size`/`mtime`, and a
    /// download from (`work`, `/a.bin`, 5,000,000 bytes, mtime `T1`): which chunks it asks for.
    async fn resume_from(tag: Option<(&str, &str, u64, &str)>) -> Vec<u64> {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.bin");
        let data: Vec<u8> = (0..5_000_000u32).map(|n| (n % 233) as u8).collect();
        let partial = crate::selkies::partial_path(&destination);
        // A stale partial holding other bytes: resuming it would splice two files.
        std::fs::write(&partial, vec![0xEEu8; STORAGE_CHUNK + 500]).unwrap();
        if let Some((home, path, size, mtime)) = tag {
            let source = PartialSource {
                size,
                mtime: mtime.into(),
            };
            write_partial_tag(&partial, &partial_tag(home, path, &source)).unwrap();
        }
        let indexes = Indexes::default();
        let (base, _) = serve(chunk_server(data.clone(), Arc::clone(&indexes))).await;
        let source = PartialSource {
            size: 5_000_000,
            mtime: "T1".into(),
        };
        test_client(base)
            .download_storage_from(
                "work",
                "/a.bin",
                &destination,
                Some(&source),
                &AtomicU64::new(0),
            )
            .await
            .unwrap();
        let resumed = indexes.lock().unwrap().clone();
        if resumed.first() == Some(&0) {
            assert_eq!(std::fs::read(&destination).unwrap(), data, "started over");
        }
        assert_eq!(
            read_partial_tag(&destination),
            None,
            "no tag on the finished file"
        );
        resumed
    }

    #[tokio::test]
    async fn a_kept_partial_from_the_same_source_resumes() {
        assert_eq!(
            resume_from(Some(("work", "/a.bin", 5_000_000, "T1"))).await,
            [1, 2]
        );
    }

    #[tokio::test]
    async fn a_same_size_edit_discards_the_partial() {
        assert_eq!(
            resume_from(Some(("work", "/a.bin", 5_000_000, "T0"))).await,
            [0, 1, 2]
        );
        // Untagged (from before tags, or another tool): never trusted.
        assert_eq!(resume_from(None).await, [0, 1, 2]);
    }

    #[tokio::test]
    async fn the_same_name_in_another_home_discards_the_partial() {
        assert_eq!(
            resume_from(Some(("_sealskin_shared_files", "/a.bin", 5_000_000, "T1"))).await,
            [0, 1, 2]
        );
        assert_eq!(
            resume_from(Some(("work", "/Docs/a.bin", 5_000_000, "T1"))).await,
            [0, 1, 2]
        );
    }

    #[tokio::test]
    async fn storage_download_refuses_an_oversized_chunk() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a");
        let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; STORAGE_CHUNK + 1]);
        let body = json!({"chunk_data_b64": big, "is_last_chunk": true}).to_string();
        let (base, _) = serve(move |_, _| reply("200 OK", &body)).await;
        let error = test_client(base)
            .download_storage("work", "/a", &destination, None, &AtomicU64::new(0))
            .await
            .unwrap_err();
        assert!(matches!(error, StorageError::Invalid(_)), "{error}");
        assert!(!destination.exists());
    }

    #[tokio::test]
    async fn storage_download_checks_the_final_size() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("a.txt");
        let (base, _) = serve(chunk_server(vec![1u8; 10], Indexes::default())).await;
        let error = test_client(base)
            .download_storage("work", "/a.txt", &destination, Some(12), &AtomicU64::new(0))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "“a.txt” changed while downloading; try again."
        );
        assert!(!destination.exists());
        assert!(!crate::selkies::partial_path(&destination).exists());
    }

    #[tokio::test]
    async fn create_home_tells_conflict_and_bad_names() {
        let (base, seen) = serve(|request, _| match request.json()["home_name"].as_str() {
            Some("work") => reply(
                "409 Conflict",
                r#"{"detail":"Home directory 'work' already exists."}"#,
            ),
            Some("odd") => reply(
                "409 Conflict",
                r#"{"detail":"1 validation error for HomeDirCreate"}"#,
            ),
            _ => reply("201 Created", r#"{"status":"success","home_name":"games"}"#),
        })
        .await;
        let client = test_client(base);
        assert_eq!(client.create_home("games").await, Ok(()));
        assert_eq!(requests_of(&seen)[0].json(), json!({"home_name": "games"}));
        assert_eq!(client.create_home("work").await, Err(HomeError::Exists));
        assert_eq!(
            client.create_home("odd").await,
            Err(HomeError::Invalid(
                "1 validation error for HomeDirCreate".into()
            ))
        );
        // Refused here: never sent.
        let before = requests_of(&seen).len();
        assert!(client.create_home("a b").await.is_err());
        assert!(client.create_home("_sealskin_shared_files").await.is_err());
        assert_eq!(requests_of(&seen).len(), before);
    }

    #[tokio::test]
    async fn delete_home_answers_204_and_404() {
        let (base, seen) = serve(|request, _| {
            if request.is("delete /companion/api/homedirs/work") {
                NO_CONTENT.to_string()
            } else {
                NOT_FOUND.to_string()
            }
        })
        .await;
        let client = test_client(base);
        assert_eq!(client.delete_home("work").await, Ok(()));
        assert_eq!(client.delete_home("gone").await, Err(HomeError::Gone));
        assert!(requests_of(&seen)[0]
            .head
            .contains("authorization: bearer ssk_id_secret"));
    }

    #[tokio::test]
    async fn delete_home_refuses_shared_files_without_a_request() {
        let (base, seen) = serve(|_, _| NO_CONTENT.to_string()).await;
        let client = test_client(base);
        for name in [
            "_sealskin_shared_files",
            "_SEALSKIN_SHARED_FILES",
            "cleanroom",
            "CleanRoom",
            "../x",
            "",
        ] {
            assert!(client.delete_home(name).await.is_err(), "{name}");
        }
        assert_eq!(
            client.delete_home(SHARED_FILES_HOME).await,
            Err(HomeError::Reserved)
        );
        assert!(requests_of(&seen).is_empty());
    }

    #[tokio::test]
    async fn launch_file_path_body_has_the_filename_and_no_room_flag() {
        let (base, seen) = serve(|request, _| upload_server(request)).await;
        let client = test_client(base);
        let params = LaunchParams {
            launch_id: Some(LAUNCH_ID.into()),
            home_name: Some("work".into()),
            launch_in_room_mode: true,
            ..LaunchParams::default()
        };
        client
            .launch(
                firefox(),
                params,
                LaunchRoute::FilePath("report.pdf"),
                || {},
            )
            .await
            .unwrap();
        let request = requests_of(&seen).remove(0);
        assert!(
            request.is("post /companion/api/launch/file_path"),
            "{}",
            request.head
        );
        assert!(request
            .head
            .contains(&format!("x-idempotency-key: {LAUNCH_ID}")));
        let body = request.json();
        assert_eq!(body["filename"], json!("report.pdf"));
        assert_eq!(body["home_name"], json!("work"));
        assert!(body.get("launch_in_room_mode").is_none());
        // A nested name or Cleanroom is never sent.
        for (name, home) in [("a/b.pdf", "work"), ("..", "work"), ("a.pdf", "cleanroom")] {
            let params = LaunchParams {
                home_name: Some(home.into()),
                ..LaunchParams::default()
            };
            let result = client
                .launch(firefox(), params, LaunchRoute::FilePath(name), || {})
                .await;
            assert!(matches!(result, Err(LaunchError::Server(_))), "{name}");
        }
        assert_eq!(requests_of(&seen).len(), 1);
    }
}
