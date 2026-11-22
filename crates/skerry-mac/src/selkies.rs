//! The session's files over Selkies' HTTP file routes (`ARCH-023`), as the web client uses them
//! (Selkies `715df7c` `web/selkies-web-core/lib/file-upload.ts`, `crates/selkies/src/files/`).
//!
//! Every route lives on the session origin beside `api/websockets` and is authorized by the
//! SealSkin session cookie `transport::socket_request` obtained. The server confines every path
//! to its file-manager folder; paths here are relative to it, `/`-separated. No request carries
//! an `Origin` header: Selkies treats a request without one as same-origin, which `mkdir` needs.

use crate::print_jobs::{
    has_pdf_header, FetchFailure, Fetched, MAX_PRINT_BYTES, PDF_HEADER_WINDOW,
};
use percent_encoding::{percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::header::{HeaderValue, CONTENT_TYPE, COOKIE, RANGE};
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Files above this go up as slices of this size, as in the web client, so no request body
/// exceeds a fronting proxy's cap (Cloudflare refuses bodies over 100 MB).
pub const UPLOAD_CHUNK_BYTES: u64 = 64 * 1024 * 1024;
/// How much of a file is read from disk at a time while it streams up.
const READ_BUFFER_BYTES: usize = 256 * 1024;
/// A download's bytes go here until complete, then the file is renamed into place.
const PARTIAL_SUFFIX: &str = ".skerrydownload";

/// `encodeURIComponent`: everything but `A–Z a–z 0–9 - _ . ! ~ * ' ( )` is percent-encoded.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// Which directions the server allows, from `server_settings.file_transfers`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub upload: bool,
    pub download: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            upload: true,
            download: true,
        }
    }
}

impl Policy {
    /// As the dashboard reads it: a missing setting allows both directions.
    pub fn from_server(settings: &Value) -> Self {
        let Some(list) = settings["file_transfers"]["value"].as_array() else {
            return Policy::default();
        };
        let has = |name: &str| list.iter().any(|item| item.as_str() == Some(name));
        Policy {
            upload: has("upload"),
            download: has("download"),
        }
    }
}

/// One entry of a folder listing.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// Root-relative, `/`-separated.
    pub path: String,
    pub is_dir: bool,
    /// `None` for a folder.
    pub size: Option<u64>,
    /// Server local time, ISO 8601 without a zone.
    pub mtime: String,
}

/// One folder, as `api/file-manager/browse` lists it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Listing {
    pub path: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug)]
pub enum Error {
    /// The server answered with an error status and, usually, a message.
    Server(StatusCode, String),
    Network(String),
    Local(std::io::Error),
    /// A name from the server that cannot be a Mac file name (`/`, `..`, empty).
    UnsafeName(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Server(status, message) if message.is_empty() => {
                f.write_str(match status.as_u16() {
                    403 => "The session doesn’t allow this.",
                    404 => "That item no longer exists in the session.",
                    409 => "Another transfer to the same file is in progress.",
                    413 => "This folder holds too many items to show.",
                    _ => "The session couldn’t complete the request.",
                })
            }
            Error::Server(_, message) => write!(f, "{}", sentence(message)),
            Error::Network(message) => {
                write!(
                    f,
                    "Couldn’t reach the session ({}).",
                    message.trim_end_matches('.')
                )
            }
            Error::Local(error) => write!(f, "{error}"),
            Error::UnsafeName(name) => write!(f, "“{name}” can’t be saved on this Mac."),
        }
    }
}

/// The server's message as a sentence: capitalised and ending in a full stop.
fn sentence(message: &str) -> String {
    let message = message.trim();
    let mut chars = message.chars();
    let mut text = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
        None => String::new(),
    };
    if !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    text
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Local(error)
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Error::Network(error.without_url().to_string())
    }
}

/// Bytes moved so far, shared with whoever shows progress.
pub type Progress = Arc<AtomicU64>;

/// One request of an upload: the whole file, or one slice of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice {
    pub offset: u64,
    pub len: u64,
    pub headers: Vec<(&'static str, String)>,
}

/// The requests that upload a file of `total` bytes to `remote`: one plain POST at or under
/// [`UPLOAD_CHUNK_BYTES`], otherwise sequential slices sharing `id`.
pub fn upload_plan(remote: &str, total: u64, id: &str) -> Vec<Slice> {
    let path = ("X-Upload-Path", encode_component(remote));
    if total <= UPLOAD_CHUNK_BYTES {
        return vec![Slice {
            offset: 0,
            len: total,
            headers: vec![path],
        }];
    }
    let mut slices = Vec::new();
    let mut offset = 0;
    while offset < total {
        let len = UPLOAD_CHUNK_BYTES.min(total - offset);
        let mut headers = vec![
            path.clone(),
            ("X-Upload-Id", id.to_owned()),
            ("X-Upload-Offset", offset.to_string()),
            ("X-Upload-Total", total.to_string()),
        ];
        if offset + len >= total {
            headers.push(("X-Upload-Final", "1".to_owned()));
        }
        slices.push(Slice {
            offset,
            len,
            headers,
        });
        offset += len;
    }
    slices
}

pub fn encode_component(text: &str) -> String {
    percent_encode(text.as_bytes(), URI_COMPONENT).to_string()
}

/// `dir/name`, root-relative.
pub fn join(dir: &str, name: &str) -> String {
    let dir = dir.trim_matches('/');
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// The parent of a root-relative path; the root is its own parent.
pub fn parent(path: &str) -> &str {
    path.trim_matches('/')
        .rsplit_once('/')
        .map_or("", |(parent, _)| parent)
}

/// `name` as a single Mac file name, or `None` when it would leave the folder it is meant
/// for: the server is trusted with its own paths, not with this Mac's.
pub fn local_name(name: &str) -> Option<&str> {
    let unsafe_name = name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']);
    (!unsafe_name).then_some(name)
}

/// The session's file routes.
#[derive(Clone)]
pub struct SelkiesClient {
    /// The session page, ending in `/`; the routes are `api/...` below it.
    base: Url,
    cookie: HeaderValue,
    http: reqwest::Client,
}

impl SelkiesClient {
    pub fn new(base: Url, mut cookie: HeaderValue) -> Result<Self, Error> {
        cookie.set_sensitive(true);
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(SelkiesClient { base, cookie, http })
    }

    fn url(&self, route: &str) -> Url {
        // `base` ends in `/` (checked by `socket_request`), so this stays below it.
        self.base.join(route).unwrap_or_else(|_| self.base.clone())
    }

    fn request(&self, method: Method, url: Url) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .header(COOKIE, self.cookie.clone())
    }

    pub async fn browse(&self, dir: &str) -> Result<Listing, Error> {
        let mut url = self.url("api/file-manager/browse");
        url.query_pairs_mut().append_pair("path", dir);
        let response = self
            .request(Method::GET, url)
            .timeout(Duration::from_secs(30))
            .send()
            .await?;
        let response = checked(response).await?;
        Ok(response.json().await?)
    }

    pub async fn mkdir(&self, path: &str) -> Result<(), Error> {
        let response = self
            .request(Method::POST, self.url("api/file-manager/mkdir"))
            .header(CONTENT_TYPE, "application/json")
            .body(serde_json::json!({ "path": path }).to_string())
            .timeout(Duration::from_secs(30))
            .send()
            .await?;
        checked(response).await.map(drop)
    }

    /// Uploads the file at `local` to `remote`, its folders created by the server, streamed
    /// from disk. Dropping the future cancels it; the server reaps the abandoned slices.
    pub async fn upload(
        &self,
        local: &Path,
        remote: &str,
        progress: &Progress,
    ) -> Result<(), Error> {
        let mut file = tokio::fs::File::open(local).await?;
        let total = file.metadata().await?.len();
        let id = transfer_id();
        let base = progress.load(Ordering::Relaxed);
        for slice in upload_plan(remote, total, &id) {
            file.seek(std::io::SeekFrom::Start(slice.offset)).await?;
            let reader = file.try_clone().await?;
            let body = reqwest::Body::wrap_stream(read_stream(reader, slice.len, progress.clone()));
            let mut request = self
                .request(Method::POST, self.url("api/upload"))
                .header(CONTENT_TYPE, "application/octet-stream")
                .header(reqwest::header::CONTENT_LENGTH, slice.len);
            for (name, value) in &slice.headers {
                request = request.header(*name, value);
            }
            checked(request.body(body).send().await?).await?;
            progress.store(base + slice.offset + slice.len, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Downloads `remote` into `destination`, resuming a partial download left beside it.
    /// Bytes land in a hidden partial file renamed into place when complete.
    pub async fn download(
        &self,
        remote: &str,
        destination: &Path,
        progress: &Progress,
    ) -> Result<(), Error> {
        let partial = partial_path(destination);
        let mut have = tokio::fs::metadata(&partial)
            .await
            .map(|meta| meta.len())
            .unwrap_or(0);
        let mut request = self.request(Method::GET, self.files_url(remote));
        if have > 0 {
            request = request.header(RANGE, range_from(have));
        }
        let mut response = request.send().await?;
        if have > 0 && response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
            // The partial file is no prefix of the file any more (it already holds every
            // byte, or the file shrank): start over rather than fail on every retry.
            have = 0;
            response = self
                .request(Method::GET, self.files_url(remote))
                .send()
                .await?;
        }
        let mut response = checked(response).await?;
        let resumed = have > 0 && response.status() == StatusCode::PARTIAL_CONTENT;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&partial)
            .await?;
        let base = progress.load(Ordering::Relaxed);
        let mut written = if resumed { have } else { 0 };
        progress.store(base + written, Ordering::Relaxed);
        let streamed: Result<(), Error> = async {
            while let Some(chunk) = response.chunk().await? {
                file.write_all(&chunk).await?;
                written += chunk.len() as u64;
                progress.store(base + written, Ordering::Relaxed);
            }
            Ok(())
        }
        .await;
        // Tokio finishes a file write in the background, so settle it even when the
        // connection dropped: a retry measures the partial file to resume from it.
        file.flush().await?;
        drop(file);
        streamed?;
        tokio::fs::rename(&partial, destination).await?;
        Ok(())
    }

    /// `api/print/<name>`, the name encoded as one path segment.
    fn print_url(&self, name: &str) -> Url {
        self.url(&format!("api/print/{}", encode_component(name)))
    }

    /// Fetches a printed document into `file` (`ARCH-033`). One plain `GET`, never a
    /// `Range` and never through [`download`](Self::download): Selkies deletes the document
    /// as it answers 200, so only a whole 200 body is a document. The headers and every
    /// chunk must each come within `stall` (no total deadline: a large, slow, healthy
    /// transfer finishes). Failures before the 200 leave the document in the spool
    /// ([`FetchFailure::Retry`]).
    pub async fn print_document(
        &self,
        name: &str,
        file: &mut tokio::fs::File,
        progress: &Progress,
        stall: Duration,
    ) -> Result<Fetched, FetchFailure> {
        self.print_document_capped(name, file, progress, stall, MAX_PRINT_BYTES)
            .await
    }

    async fn print_document_capped(
        &self,
        name: &str,
        file: &mut tokio::fs::File,
        progress: &Progress,
        stall: Duration,
        cap: u64,
    ) -> Result<Fetched, FetchFailure> {
        let retry = |why: String| FetchFailure::Retry(why);
        let request = self.request(Method::GET, self.print_url(name)).send();
        let mut response = match tokio::time::timeout(stall, request).await {
            Err(_) => return Err(retry("no answer in time".into())),
            Ok(Err(error)) => return Err(retry(Error::from(error).to_string())),
            Ok(Ok(response)) => response,
        };
        let status = response.status();
        if status != StatusCode::OK {
            let body = match tokio::time::timeout(stall, response.text()).await {
                Ok(Ok(body)) => body,
                _ => String::new(),
            };
            return Err(match status.as_u16() {
                404 => FetchFailure::Gone,
                // SealSkin's forward_auth answers a stale or bad cookie with a JSON 403
                // (`{"detail": …}`) before Selkies sees the request: the document is still
                // in the spool. Only Selkies' own plain-text 403s are refusals.
                403 if sealskin_detail(&body).is_some() => {
                    retry(sealskin_detail(&body).unwrap_or_default())
                }
                403 => FetchFailure::Refused(sentence(&server_message(status, &body))),
                // A 206 deletes nothing; any other answer (a sign-in redirect, a proxy's
                // 5xx) never reached the document either.
                _ => retry(format!("HTTP {status}")),
            });
        }
        let length = response.content_length();
        if let Some(length) = length.filter(|&length| length > cap) {
            return Err(FetchFailure::TooLarge(length));
        }
        let limit = length.unwrap_or(cap);
        let printed_at = response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| chrono::DateTime::parse_from_rfc2822(value).ok())
            .map(SystemTime::from);
        let mut head = Vec::with_capacity(PDF_HEADER_WINDOW);
        let mut written = 0u64;
        let unsaved = |error: std::io::Error| FetchFailure::Unsaved(error.to_string());
        loop {
            let chunk = match tokio::time::timeout(stall, response.chunk()).await {
                Err(_) => return Err(FetchFailure::Broken("stalled".into())),
                Ok(Err(error)) => return Err(FetchFailure::Broken(Error::from(error).to_string())),
                Ok(Ok(None)) => break,
                Ok(Ok(Some(chunk))) => chunk,
            };
            written += chunk.len() as u64;
            if written > limit {
                return Err(if written > cap {
                    FetchFailure::TooLarge(written)
                } else {
                    FetchFailure::Broken("longer than its length".into())
                });
            }
            if head.len() < PDF_HEADER_WINDOW {
                let take = (PDF_HEADER_WINDOW - head.len()).min(chunk.len());
                head.extend_from_slice(&chunk[..take]);
                if head.len() == PDF_HEADER_WINDOW && !has_pdf_header(&head) {
                    return Err(FetchFailure::NotPdf);
                }
            }
            file.write_all(&chunk).await.map_err(unsaved)?;
            progress.store(written, Ordering::Relaxed);
        }
        if length.is_some_and(|length| written != length) {
            return Err(FetchFailure::Broken("shorter than its length".into()));
        }
        if !has_pdf_header(&head) {
            return Err(FetchFailure::NotPdf);
        }
        Ok(Fetched {
            bytes: written,
            printed_at,
        })
    }

    /// The local repository's catalog document, `api/apps/metadata.yml` (`ARCH-035`).
    pub fn apps_metadata_url(&self) -> Url {
        self.url("api/apps/metadata.yml")
    }

    /// Fetches the local repository's catalog with the session cookie and no `Origin`,
    /// within [`crate::apps::CATALOG_CAP`] and [`crate::apps::CATALOG_TIMEOUT`].
    pub async fn apps_metadata(&self) -> Result<Vec<u8>, Error> {
        self.capped_get(
            self.apps_metadata_url(),
            crate::apps::CATALOG_CAP,
            crate::apps::CATALOG_TIMEOUT,
        )
        .await
    }

    /// Fetches one catalog icon, `icon_path` from [`crate::apps::icon_path`] (already one
    /// encoded segment), within [`crate::apps::ICON_CAP`] and [`crate::apps::ICON_TIMEOUT`].
    pub async fn apps_icon(&self, icon_path: &str) -> Result<Vec<u8>, Error> {
        if icon_path.is_empty() || icon_path.contains('/') {
            return Err(Error::UnsafeName(icon_path.to_owned()));
        }
        self.capped_get(
            self.url(&format!("api/apps/img/{icon_path}")),
            crate::apps::ICON_CAP,
            crate::apps::ICON_TIMEOUT,
        )
        .await
    }

    async fn capped_get(&self, url: Url, cap: usize, timeout: Duration) -> Result<Vec<u8>, Error> {
        let fetch = async {
            let mut response = checked(self.request(Method::GET, url).send().await?).await?;
            if response
                .content_length()
                .is_some_and(|length| length > cap as u64)
            {
                return Err(Error::Network("the answer is too large".into()));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if body.len() + chunk.len() > cap {
                    return Err(Error::Network("the answer is too large".into()));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(body)
        };
        tokio::time::timeout(timeout, fetch)
            .await
            .map_err(|_| Error::Network("timed out".into()))?
    }

    fn files_url(&self, remote: &str) -> Url {
        let path = remote
            .split('/')
            .filter(|part| !part.is_empty())
            .map(encode_component)
            .collect::<Vec<_>>()
            .join("/");
        self.url(&format!("api/files/{path}"))
    }

    /// Every file below `dir`, with its path relative to `dir`, and every folder (so empty
    /// ones are made too), walked one listing at a time.
    pub async fn walk(&self, dir: &str) -> Result<Vec<(Entry, String)>, Error> {
        let mut found = Vec::new();
        let mut pending = vec![(dir.trim_matches('/').to_owned(), String::new())];
        while let Some((remote, relative)) = pending.pop() {
            for entry in self.browse(&remote).await?.entries {
                let name = local_name(&entry.name)
                    .ok_or_else(|| Error::UnsafeName(entry.name.clone()))?
                    .to_owned();
                let relative = if relative.is_empty() {
                    name
                } else {
                    format!("{relative}/{name}")
                };
                if entry.is_dir {
                    pending.push((join(&remote, &entry.name), relative.clone()));
                }
                found.push((entry, relative));
            }
        }
        Ok(found)
    }
}

/// Where a download's bytes wait: a hidden sibling of the destination.
pub fn partial_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    destination.with_file_name(format!(".{name}{PARTIAL_SUFFIX}"))
}

fn range_from(have: u64) -> String {
    format!("bytes={have}-")
}

fn transfer_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_nanos())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("skerry-{nanos:x}-{count:x}-{:x}", std::process::id())
}

/// `len` bytes of `file` from its current position, counted into `progress` as they are read.
fn read_stream(
    file: tokio::fs::File,
    len: u64,
    progress: Progress,
) -> impl futures_util::Stream<Item = std::io::Result<Vec<u8>>> {
    futures_util::stream::unfold((file, len), move |(mut file, left)| {
        let progress = progress.clone();
        async move {
            if left == 0 {
                return None;
            }
            let mut buffer = vec![0; READ_BUFFER_BYTES.min(left as usize)];
            match file.read(&mut buffer).await {
                Ok(0) => Some((
                    Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "the file got shorter while it was uploading",
                    )),
                    (file, 0),
                )),
                Ok(read) => {
                    buffer.truncate(read);
                    progress.fetch_add(read as u64, Ordering::Relaxed);
                    Some((Ok(buffer), (file, left - read as u64)))
                }
                Err(error) => Some((Err(error), (file, 0))),
            }
        }
    })
}

/// The response when it is a success, otherwise its status and the server's message.
async fn checked(response: reqwest::Response) -> Result<reqwest::Response, Error> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    Err(Error::Server(status, server_message(status, &body)))
}

/// SealSkin's own error body, `{"detail": …}` (FastAPI), as opposed to Selkies'.
fn sealskin_detail(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("detail")?
        .as_str()
        .map(str::to_owned)
}

/// What a session's proxy answers while its container is still starting: nginx's HTML 404
/// or a gateway error, before Selkies serves its routes (`ARCH-028`).
pub const STILL_STARTING: &str = "The session is still starting. Try again in a moment.";

/// `{"status": "error", "message": …}`, or a short plain-text answer. An HTML page (a
/// proxy's, never Selkies') is never shown: on 404 or a gateway error it means the session
/// is still starting; otherwise the status speaks for itself.
fn server_message(status: StatusCode, body: &str) -> String {
    if body.trim_start().starts_with('<') {
        return if matches!(status.as_u16(), 404 | 502 | 503 | 504) {
            STILL_STARTING.into()
        } else {
            String::new()
        };
    }
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        if let Some(message) = value["message"].as_str() {
            return message.to_owned();
        }
        if let Some(detail) = value["detail"].as_str() {
            return detail.to_owned();
        }
    }
    body.trim().chars().take(160).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn small_files_upload_as_one_plain_post() {
        for total in [0, 1, UPLOAD_CHUNK_BYTES] {
            let plan = upload_plan("Docs/a b.txt", total, "id");
            assert_eq!(
                plan,
                vec![Slice {
                    offset: 0,
                    len: total,
                    headers: vec![("X-Upload-Path", "Docs%2Fa%20b.txt".into())],
                }]
            );
        }
    }

    #[test]
    fn large_files_upload_as_sequential_slices() {
        let total = UPLOAD_CHUNK_BYTES * 2 + 5;
        let plan = upload_plan("big.iso", total, "xfer");
        assert_eq!(plan.len(), 3);
        let mut expected_offset = 0;
        for (index, slice) in plan.iter().enumerate() {
            assert_eq!(slice.offset, expected_offset);
            let header = |name: &str| {
                slice
                    .headers
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.as_str())
            };
            assert_eq!(header("X-Upload-Path"), Some("big.iso"));
            assert_eq!(header("X-Upload-Id"), Some("xfer"));
            assert_eq!(header("X-Upload-Offset"), Some(&*slice.offset.to_string()));
            assert_eq!(header("X-Upload-Total"), Some(&*total.to_string()));
            let last = index == plan.len() - 1;
            assert_eq!(header("X-Upload-Final"), last.then_some("1"));
            expected_offset += slice.len;
        }
        assert_eq!(expected_offset, total);
        assert_eq!(plan[2].len, 5);
    }

    #[test]
    fn paths_are_encoded_like_encode_uri_component() {
        assert_eq!(
            encode_component("a/b c&d?é!~*'()_-."),
            "a%2Fb%20c%26d%3F%C3%A9!~*'()_-."
        );
    }

    #[test]
    fn policy_follows_file_transfers() {
        assert_eq!(Policy::from_server(&json!({})), Policy::default());
        let only = |list: Value| Policy::from_server(&json!({"file_transfers": {"value": list}}));
        assert_eq!(
            only(json!(["download"])),
            Policy {
                upload: false,
                download: true
            }
        );
        assert_eq!(
            only(json!([])),
            Policy {
                upload: false,
                download: false
            }
        );
    }

    #[test]
    fn listings_decode() {
        let listing: Listing = serde_json::from_value(json!({
            "status": "ok",
            "path": "Docs",
            "entries": [
                {"name": "a.txt", "path": "Docs/a.txt", "is_dir": false, "size": 3, "mtime": "2026-11-21T10:00:00"},
                {"name": "sub", "path": "Docs/sub", "is_dir": true, "size": null, "mtime": "2026-11-21T10:00:00.5"},
            ]
        }))
        .unwrap();
        assert_eq!(listing.path, "Docs");
        assert_eq!(listing.entries[0].size, Some(3));
        assert!(listing.entries[1].is_dir && listing.entries[1].size.is_none());
    }

    #[test]
    fn server_names_never_leave_the_chosen_folder() {
        for name in ["", ".", "..", "a/b", "../x", "a\0b"] {
            assert_eq!(local_name(name), None, "{name:?}");
        }
        assert_eq!(local_name("..hidden"), Some("..hidden"));
        // `:` is legal on both sides (Finder shows it as `/`).
        assert_eq!(local_name("log 10:04.txt"), Some("log 10:04.txt"));
        assert_eq!(local_name("report.pdf"), Some("report.pdf"));
    }

    #[test]
    fn remote_paths_join_and_split() {
        assert_eq!(join("", "a"), "a");
        assert_eq!(join("/Docs/", "a"), "Docs/a");
        assert_eq!(parent("Docs/sub/a"), "Docs/sub");
        assert_eq!(parent("a"), "");
        assert_eq!(parent(""), "");
    }

    #[test]
    fn requests_carry_the_cookie_and_no_origin() {
        let access = SelkiesClient::new(
            Url::parse("https://seal.example/sess-1/").unwrap(),
            HeaderValue::from_static("session=abc"),
        )
        .unwrap();
        let request = access
            .request(Method::GET, access.files_url("Docs/a b#.txt"))
            .build()
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://seal.example/sess-1/api/files/Docs/a%20b%23.txt"
        );
        assert_eq!(request.headers()[COOKIE], "session=abc");
        assert!(request.headers().get(reqwest::header::ORIGIN).is_none());
    }

    #[test]
    fn print_urls_encode_the_name_like_encode_uri_component() {
        let access = SelkiesClient::new(
            Url::parse("https://seal.example/sess-1/").unwrap(),
            HeaderValue::from_static("session=abc"),
        )
        .unwrap();
        assert_eq!(
            access.print_url("Report (2).pdf").as_str(),
            "https://seal.example/sess-1/api/print/Report%20(2).pdf"
        );
        assert_eq!(
            access.print_url("Résumé #2 50%?.pdf").as_str(),
            "https://seal.example/sess-1/api/print/R%C3%A9sum%C3%A9%20%232%2050%25%3F.pdf"
        );
    }

    #[test]
    fn resumed_downloads_ask_for_the_rest() {
        assert_eq!(range_from(1024), "bytes=1024-");
        assert_eq!(
            partial_path(Path::new("/tmp/x/report.pdf")),
            Path::new("/tmp/x/.report.pdf.skerrydownload")
        );
    }

    #[test]
    fn server_errors_keep_their_message() {
        assert_eq!(
            server_message(
                StatusCode::FORBIDDEN,
                r#"{"status":"error","message":"uploads disabled"}"#
            ),
            "uploads disabled"
        );
        assert_eq!(
            server_message(StatusCode::FORBIDDEN, "  Forbidden \n"),
            "Forbidden"
        );
        let nginx = "<html>\r\n<head><title>404 Not Found</title></head><center>nginx</center>";
        assert_eq!(server_message(StatusCode::NOT_FOUND, nginx), STILL_STARTING);
        assert_eq!(
            server_message(StatusCode::BAD_GATEWAY, nginx),
            STILL_STARTING
        );
        assert_eq!(server_message(StatusCode::FORBIDDEN, nginx), "");
        assert_eq!(
            Error::Server(
                StatusCode::NOT_FOUND,
                server_message(StatusCode::NOT_FOUND, nginx)
            )
            .to_string(),
            STILL_STARTING
        );
    }

    /// `SelkiesClient` end to end against an in-process stand-in for Selkies' file routes on
    /// 127.0.0.1, written from Selkies `715df7c` `files/{upload,manager,index}.rs`: the
    /// `X-Upload-Path` header percent-decoded, chunked slices checked against the bytes banked
    /// and renamed into place on `X-Upload-Final: 1`, browse/mkdir JSON, `api/files/<path>`
    /// decoded per segment, and `Range` answered with 206 (or 416 past the end).
    mod end_to_end {
        use super::*;
        use std::collections::HashMap;
        use std::sync::atomic::AtomicBool;
        use std::sync::Mutex;
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::net::{TcpListener, TcpStream};

        const COOKIE_VALUE: &str = "selkies_session=abc";

        #[derive(Clone, Debug)]
        struct Seen {
            method: String,
            target: String,
            headers: HashMap<String, String>,
        }

        #[derive(Default)]
        struct Fake {
            root: PathBuf,
            seen: Mutex<Vec<Seen>>,
            /// Destination → (transfer id, bytes banked), as `ChunkState`.
            chunked: Mutex<HashMap<PathBuf, (String, u64)>>,
            /// Cut the next whole-file download off halfway, as a dropped connection.
            cut_next_download: AtomicBool,
            /// The print spool by name; a 200 takes the document, as Selkies deletes it.
            prints: Mutex<HashMap<String, PrintReply>>,
        }

        #[derive(Clone)]
        enum PrintReply {
            Pdf(Vec<u8>),
            /// Claims `claimed` bytes, sends `sent`, then closes.
            Truncate {
                claimed: usize,
                sent: Vec<u8>,
            },
            Status(&'static str, &'static str),
            /// No `Content-Length`: the body runs until the connection closes.
            NoLength(Vec<u8>),
            /// Claims a length over the cap.
            Huge,
            /// Never answers.
            HoldHeaders,
            /// SealSkin's forward_auth refusing a stale cookie.
            Detail403,
            /// Sends the headers and part of the body, then goes quiet.
            StallBody(Vec<u8>),
        }

        struct Server {
            fake: Arc<Fake>,
            access: SelkiesClient,
            _root: tempfile::TempDir,
        }

        async fn start() -> Server {
            let root = tempfile::tempdir().unwrap();
            let fake = Arc::new(Fake {
                root: root.path().to_owned(),
                ..Fake::default()
            });
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let serving = fake.clone();
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    tokio::spawn(handle(stream, serving.clone()));
                }
            });
            let access = SelkiesClient::new(
                Url::parse(&format!("http://127.0.0.1:{port}/sess-1/")).unwrap(),
                HeaderValue::from_static(COOKIE_VALUE),
            )
            .unwrap();
            Server {
                fake,
                access,
                _root: root,
            }
        }

        fn decode(text: &str) -> String {
            percent_encoding::percent_decode_str(text)
                .decode_utf8()
                .unwrap()
                .into_owned()
        }

        /// A root-relative path as the server confines it, or `None` for an escape.
        fn confined(root: &Path, rel: &str) -> Option<PathBuf> {
            let mut path = root.to_owned();
            for part in rel.split('/').filter(|p| !p.is_empty() && *p != ".") {
                if part == ".." {
                    return None;
                }
                path.push(part);
            }
            Some(path)
        }

        async fn respond(
            stream: &mut TcpStream,
            status: &str,
            headers: &[(&str, String)],
            body: &[u8],
        ) {
            let mut head = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
            for (name, value) in headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
        }

        async fn json(stream: &mut TcpStream, status: &str, value: Value) {
            let body = value.to_string();
            let headers = [("Content-Type", "application/json".to_owned())];
            respond(stream, status, &headers, body.as_bytes()).await;
        }

        async fn error(stream: &mut TcpStream, status: &str, message: &str) {
            json(
                stream,
                status,
                serde_json::json!({"status": "error", "message": message}),
            )
            .await;
        }

        async fn handle(stream: TcpStream, fake: Arc<Fake>) {
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let mut words = line.split_whitespace();
            let method = words.next().unwrap_or_default().to_owned();
            let target = words.next().unwrap_or_default().to_owned();
            let mut headers = HashMap::new();
            loop {
                line.clear();
                reader.read_line(&mut line).await.unwrap();
                let Some((name, value)) = line.trim_end().split_once(':') else {
                    break;
                };
                headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
            }
            assert!(
                !headers.contains_key("transfer-encoding"),
                "bodies declare their length, which Selkies enforces"
            );
            let length: usize = headers
                .get("content-length")
                .map_or(0, |value| value.parse().unwrap());
            let mut body = vec![0; length];
            reader.read_exact(&mut body).await.unwrap();
            let mut stream = reader.into_inner();
            fake.seen.lock().unwrap().push(Seen {
                method: method.clone(),
                target: target.clone(),
                headers: headers.clone(),
            });
            if headers.get("cookie").map(String::as_str) != Some(COOKIE_VALUE) {
                return error(&mut stream, "401 Unauthorized", "no session").await;
            }
            if headers.contains_key("origin") {
                return error(&mut stream, "403 Forbidden", "Forbidden origin").await;
            }
            let url = Url::parse(&format!("http://fake{target}")).unwrap();
            let Some(route) = url.path().strip_prefix("/sess-1/api/") else {
                return error(&mut stream, "404 Not Found", "no route").await;
            };
            match (method.as_str(), route) {
                ("POST", "upload") => upload(&mut stream, &fake, &headers, body).await,
                ("GET", "file-manager/browse") => {
                    let rel = url
                        .query_pairs()
                        .find(|(key, _)| key == "path")
                        .map(|(_, value)| value.into_owned())
                        .unwrap_or_default();
                    browse(&mut stream, &fake, &rel).await
                }
                ("POST", "file-manager/mkdir") => {
                    let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    let rel = value["path"].as_str().unwrap_or_default();
                    match confined(&fake.root, rel) {
                        Some(dir) if !rel.trim_matches('/').is_empty() => {
                            match std::fs::create_dir(&dir) {
                                Ok(()) => {
                                    json(&mut stream, "200 OK", serde_json::json!({"status": "ok"}))
                                        .await
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                                    error(&mut stream, "409 Conflict", "already exists").await
                                }
                                Err(e) => {
                                    error(&mut stream, "500 Internal Server Error", &e.to_string())
                                        .await
                                }
                            }
                        }
                        _ => error(&mut stream, "400 Bad Request", "invalid path").await,
                    }
                }
                ("GET", files) if files.starts_with("files/") => {
                    let rel = decode(&files["files/".len()..]);
                    download(&mut stream, &fake, &rel, &headers).await
                }
                ("GET", "apps/metadata.yml") => {
                    let body =
                        b"include:\n  - {name: a, full_name: A, icon: a.svg, description: x}\n";
                    respond(&mut stream, "200 OK", &[], body).await
                }
                ("GET", "apps/img/a.svg") => {
                    respond(
                        &mut stream,
                        "200 OK",
                        &[],
                        b"<svg xmlns='http://www.w3.org/2000/svg'/>",
                    )
                    .await
                }
                ("GET", "apps/img/huge.png") => {
                    respond(
                        &mut stream,
                        "200 OK",
                        &[],
                        &vec![0u8; crate::apps::ICON_CAP + 1],
                    )
                    .await
                }
                ("GET", print) if print.starts_with("print/") => {
                    let name = decode(&print["print/".len()..]);
                    print_document(&mut stream, &fake, &name, &headers).await
                }
                _ => error(&mut stream, "404 Not Found", "no route").await,
            }
        }

        async fn upload(
            stream: &mut TcpStream,
            fake: &Fake,
            headers: &HashMap<String, String>,
            body: Vec<u8>,
        ) {
            let rel = decode(headers.get("x-upload-path").map_or("", String::as_str));
            let Some(dest) = confined(&fake.root, &rel).filter(|d| *d != fake.root) else {
                return error(stream, "400 Bad Request", "invalid upload path").await;
            };
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            let Some(id) = headers.get("x-upload-id") else {
                std::fs::write(&dest, &body).unwrap();
                let reply = serde_json::json!({"status": "success", "bytes": body.len()});
                return json(stream, "200 OK", reply).await;
            };
            let offset: u64 = headers["x-upload-offset"].parse().unwrap();
            let total: u64 = headers["x-upload-total"].parse().unwrap();
            let part = dest.with_file_name(".selkies-upload-test.part");
            let banked = {
                let mut table = fake.chunked.lock().unwrap();
                if offset == 0 {
                    table.insert(dest.clone(), (id.clone(), 0));
                    std::fs::write(&part, b"").unwrap();
                }
                match table.get_mut(&dest) {
                    Some((held, banked)) if held == id && *banked == offset => {
                        use std::io::Write;
                        let mut file = std::fs::OpenOptions::new()
                            .append(true)
                            .open(&part)
                            .unwrap();
                        file.write_all(&body).unwrap();
                        *banked += body.len() as u64;
                        Some(*banked)
                    }
                    _ => {
                        table.remove(&dest);
                        None
                    }
                }
            };
            let Some(banked) = banked else {
                return error(stream, "409 Conflict", "chunk sequence mismatch").await;
            };
            if headers.get("x-upload-final").map(String::as_str) != Some("1") {
                let reply =
                    serde_json::json!({"status": "success", "bytes": banked, "complete": false});
                return json(stream, "200 OK", reply).await;
            }
            fake.chunked.lock().unwrap().remove(&dest);
            if banked != total {
                return error(stream, "400 Bad Request", "size mismatch").await;
            }
            std::fs::rename(&part, &dest).unwrap();
            let reply = serde_json::json!({"status": "success", "bytes": banked, "complete": true});
            json(stream, "200 OK", reply).await
        }

        async fn browse(stream: &mut TcpStream, fake: &Fake, rel: &str) {
            let Some(dir) = confined(&fake.root, rel) else {
                return error(stream, "400 Bad Request", "invalid path").await;
            };
            let Ok(children) = std::fs::read_dir(&dir) else {
                return error(stream, "404 Not Found", "not a directory").await;
            };
            let rel = rel.trim_matches('/');
            let mut entries: Vec<Value> = children
                .map(|child| {
                    let child = child.unwrap();
                    let name = child.file_name().to_string_lossy().into_owned();
                    let meta = child.metadata().unwrap();
                    serde_json::json!({
                        "name": name,
                        "path": join(rel, &name),
                        "is_dir": meta.is_dir(),
                        "size": (!meta.is_dir()).then_some(meta.len()),
                        "mtime": "2026-11-21T10:04:05.123456",
                    })
                })
                .collect();
            entries.sort_by_key(|entry| entry["name"].as_str().unwrap().to_lowercase());
            let reply = serde_json::json!({"status": "ok", "path": rel, "entries": entries});
            json(stream, "200 OK", reply).await
        }

        async fn download(
            stream: &mut TcpStream,
            fake: &Fake,
            rel: &str,
            headers: &HashMap<String, String>,
        ) {
            let Some(bytes) = confined(&fake.root, rel).and_then(|path| std::fs::read(path).ok())
            else {
                let plain = [("Content-Type", "text/plain; charset=utf-8".to_owned())];
                return respond(stream, "404 Not Found", &plain, b"Not Found").await;
            };
            let size = bytes.len() as u64;
            let disposition = [("Content-Disposition", "attachment".to_owned())];
            if let Some(range) = headers.get("range") {
                let start: u64 = range
                    .strip_prefix("bytes=")
                    .and_then(|r| r.strip_suffix('-'))
                    .unwrap()
                    .parse()
                    .unwrap();
                if start >= size {
                    let range = [("Content-Range", format!("bytes */{size}"))];
                    return respond(stream, "416 Range Not Satisfiable", &range, b"").await;
                }
                let range = [
                    (
                        "Content-Range",
                        format!("bytes {start}-{}/{size}", size - 1),
                    ),
                    disposition[0].clone(),
                ];
                return respond(
                    stream,
                    "206 Partial Content",
                    &range,
                    &bytes[start as usize..],
                )
                .await;
            }
            if fake.cut_next_download.swap(false, Ordering::SeqCst) {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {size}\r\n\r\n"
                );
                stream.write_all(head.as_bytes()).await.unwrap();
                stream.write_all(&bytes[..bytes.len() / 2]).await.unwrap();
                stream.flush().await.unwrap();
                return; // dropped: the connection closes short of Content-Length
            }
            respond(stream, "200 OK", &disposition, &bytes).await
        }

        /// `handle_print_document`: the spool's document whole with a 200 (taken from the
        /// spool as the answer is built), 206 for a `Range` (kept), 404 when absent.
        async fn print_document(
            stream: &mut TcpStream,
            fake: &Fake,
            name: &str,
            headers: &HashMap<String, String>,
        ) {
            let reply = fake.prints.lock().unwrap().get(name).cloned();
            let Some(reply) = reply else {
                let plain = [("Content-Type", "text/plain; charset=utf-8".to_owned())];
                return respond(stream, "404 Not Found", &plain, b"Not Found").await;
            };
            let modified = [("Last-Modified", "Sun, 22 Nov 2026 04:47:00 GMT".to_owned())];
            if headers.contains_key("range") {
                return respond(stream, "206 Partial Content", &modified, b"%PDF-").await;
            }
            let taken = !matches!(reply, PrintReply::Status(..) | PrintReply::Detail403);
            if taken {
                fake.prints.lock().unwrap().remove(name);
            }
            match reply {
                PrintReply::Pdf(bytes) => respond(stream, "200 OK", &modified, &bytes).await,
                PrintReply::Status(status, message) => error(stream, status, message).await,
                PrintReply::Truncate { claimed, sent } => {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {claimed}\r\n\r\n"
                    );
                    stream.write_all(head.as_bytes()).await.unwrap();
                    stream.write_all(&sent).await.unwrap();
                    stream.flush().await.unwrap();
                }
                PrintReply::NoLength(bytes) => {
                    let head = "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
                    stream.write_all(head.as_bytes()).await.unwrap();
                    let _ = stream.write_all(&bytes).await;
                    let _ = stream.flush().await;
                }
                PrintReply::Huge => {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n%PDF-",
                        MAX_PRINT_BYTES + 1
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                PrintReply::HoldHeaders => tokio::time::sleep(Duration::from_secs(5)).await,
                PrintReply::Detail403 => {
                    let body = br#"{"detail":"Forbidden: Invalid session or token."}"#;
                    let json = [("Content-Type", "application/json".to_owned())];
                    respond(stream, "403 Forbidden", &json, body).await
                }
                PrintReply::StallBody(bytes) => {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                        bytes.len() * 2
                    );
                    stream.write_all(head.as_bytes()).await.unwrap();
                    stream.write_all(&bytes).await.unwrap();
                    stream.flush().await.unwrap();
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }

        fn requests(fake: &Fake, method: &str, prefix: &str) -> Vec<Seen> {
            fake.seen
                .lock()
                .unwrap()
                .iter()
                .filter(|seen| seen.method == method && seen.target.starts_with(prefix))
                .cloned()
                .collect()
        }

        /// Bytes that differ at every offset a slice boundary could fall on.
        fn pattern(len: usize) -> Vec<u8> {
            (0..len)
                .map(|i| (i % 251) as u8 ^ (i >> 20) as u8)
                .collect()
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn plain_upload_browse_mkdir_and_download() {
            let server = start().await;
            let (access, fake) = (&server.access, &server.fake);
            let local = tempfile::tempdir().unwrap();

            // Upload a small file into a folder that does not exist yet (the server makes it).
            let source = local.path().join("source.txt");
            std::fs::write(&source, b"hello, session").unwrap();
            let progress = Progress::default();
            let remote = "Docs/a b#%+é.txt";
            access.upload(&source, remote, &progress).await.unwrap();
            assert_eq!(progress.load(Ordering::Relaxed), 14);
            assert_eq!(
                std::fs::read(fake.root.join("Docs/a b#%+é.txt")).unwrap(),
                b"hello, session"
            );
            let posts = requests(fake, "POST", "/sess-1/api/upload");
            assert_eq!(posts.len(), 1);
            assert_eq!(
                posts[0].headers["x-upload-path"],
                "Docs%2Fa%20b%23%25%2B%C3%A9.txt"
            );
            assert_eq!(posts[0].headers["content-length"], "14");
            assert!(!posts[0].headers.contains_key("x-upload-id"));

            // New Folder, then the same again: the server's 409 message comes through.
            access.mkdir("Docs/New Folder").await.unwrap();
            match access.mkdir("Docs/New Folder").await {
                Err(Error::Server(status, message)) => {
                    assert_eq!(status, StatusCode::CONFLICT);
                    assert_eq!(message, "already exists");
                }
                other => panic!("expected a 409, got {other:?}"),
            }
            let mkdir = &requests(fake, "POST", "/sess-1/api/file-manager/mkdir")[0];
            assert_eq!(mkdir.headers["content-type"], "application/json");

            // Browse lists both, with the path query form-encoded.
            let listing = access.browse("Docs").await.unwrap();
            assert_eq!(listing.path, "Docs");
            let names: Vec<_> = listing.entries.iter().map(|e| e.name.as_str()).collect();
            assert_eq!(names, ["a b#%+é.txt", "New Folder"]);
            assert!(listing.entries[1].is_dir && listing.entries[1].size.is_none());
            assert_eq!(listing.entries[0].path, "Docs/a b#%+é.txt");
            let root = access.browse("").await.unwrap();
            assert_eq!(root.entries.len(), 1);

            // Download it back by the path the listing gave.
            let destination = local.path().join("copy.txt");
            let progress = Progress::default();
            access
                .download(&listing.entries[0].path, &destination, &progress)
                .await
                .unwrap();
            assert_eq!(std::fs::read(&destination).unwrap(), b"hello, session");
            assert!(!partial_path(&destination).exists());
            assert_eq!(progress.load(Ordering::Relaxed), 14);
            let get = &requests(fake, "GET", "/sess-1/api/files/")[0];
            assert_eq!(
                get.target,
                "/sess-1/api/files/Docs/a%20b%23%25%2B%C3%A9.txt"
            );
            assert!(!get.headers.contains_key("range"));

            // A missing file is the server's plain-text 404.
            match access
                .download("Docs/gone.txt", &destination, &progress)
                .await
            {
                Err(Error::Server(status, message)) => {
                    assert_eq!(status, StatusCode::NOT_FOUND);
                    assert_eq!(message, "Not Found");
                }
                other => panic!("expected a 404, got {other:?}"),
            }

            // Walk lists everything below a folder, relative to it.
            access.mkdir("Docs/New Folder/Inner").await.unwrap();
            let walked = access.walk("Docs").await.unwrap();
            let mut relative: Vec<_> = walked.iter().map(|(_, r)| r.as_str()).collect();
            relative.sort();
            assert_eq!(relative, ["New Folder", "New Folder/Inner", "a b#%+é.txt"]);

            // Every request carried the cookie and no Origin (the fake refuses otherwise).
            for seen in fake.seen.lock().unwrap().iter() {
                assert_eq!(seen.headers["cookie"], COOKIE_VALUE);
                assert!(!seen.headers.contains_key("origin"));
            }
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn large_files_upload_in_slices_the_server_reassembles() {
            let server = start().await;
            let (access, fake) = (&server.access, &server.fake);
            let local = tempfile::tempdir().unwrap();
            let total = 2 * UPLOAD_CHUNK_BYTES as usize + 3 * 1024 * 1024 + 7;
            let bytes = pattern(total);
            let source = local.path().join("big.bin");
            std::fs::write(&source, &bytes).unwrap();

            // A multi-file job's meter: progress already holds an earlier file's bytes.
            let progress = Progress::new(AtomicU64::new(1000));
            access
                .upload(&source, "Big/big.bin", &progress)
                .await
                .unwrap();
            assert_eq!(progress.load(Ordering::Relaxed), 1000 + total as u64);
            assert!(std::fs::read(fake.root.join("Big/big.bin")).unwrap() == bytes);
            assert!(fake.chunked.lock().unwrap().is_empty());

            let posts = requests(fake, "POST", "/sess-1/api/upload");
            assert_eq!(posts.len(), 3);
            let id = &posts[0].headers["x-upload-id"];
            for (index, post) in posts.iter().enumerate() {
                let offset = index as u64 * UPLOAD_CHUNK_BYTES;
                assert_eq!(post.headers["x-upload-path"], "Big%2Fbig.bin");
                assert_eq!(&post.headers["x-upload-id"], id);
                assert_eq!(post.headers["x-upload-offset"], offset.to_string());
                assert_eq!(post.headers["x-upload-total"], total.to_string());
                let len = UPLOAD_CHUNK_BYTES.min(total as u64 - offset);
                assert_eq!(post.headers["content-length"], len.to_string());
                assert_eq!(
                    post.headers.get("x-upload-final").map(String::as_str),
                    (index == 2).then_some("1")
                );
            }

            // A second upload of the same file is a new transfer with its own id.
            access
                .upload(&source, "Big/big.bin", &Progress::default())
                .await
                .unwrap();
            let posts = requests(fake, "POST", "/sess-1/api/upload");
            assert_ne!(&posts[3].headers["x-upload-id"], id);
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn an_interrupted_download_resumes_with_a_range() {
            let server = start().await;
            let (access, fake) = (&server.access, &server.fake);
            let local = tempfile::tempdir().unwrap();
            let bytes = pattern(3 * 1024 * 1024 + 11);
            std::fs::write(fake.root.join("movie.mp4"), &bytes).unwrap();
            let destination = local.path().join("movie.mp4");
            let partial = partial_path(&destination);

            // The connection drops halfway: a network error, the bytes kept in the partial file.
            fake.cut_next_download.store(true, Ordering::SeqCst);
            let progress = Progress::default();
            match access.download("movie.mp4", &destination, &progress).await {
                Err(Error::Network(_)) => {}
                other => panic!("expected a network error, got {other:?}"),
            }
            assert!(!destination.exists());
            let have = std::fs::metadata(&partial).unwrap().len();
            assert!(have > 0 && have < bytes.len() as u64, "kept {have} bytes");

            // The retry asks for the rest, gets 206, and finishes the same file.
            progress.store(0, Ordering::Relaxed);
            access
                .download("movie.mp4", &destination, &progress)
                .await
                .unwrap();
            assert!(std::fs::read(&destination).unwrap() == bytes);
            assert!(!partial.exists());
            assert_eq!(progress.load(Ordering::Relaxed), bytes.len() as u64);
            let gets = requests(fake, "GET", "/sess-1/api/files/movie.mp4");
            assert_eq!(gets.len(), 2);
            assert_eq!(gets[1].headers["range"], format!("bytes={have}-"));

            // A partial that already holds every byte gets 416; the download starts over.
            std::fs::remove_file(&destination).unwrap();
            std::fs::write(&partial, &bytes).unwrap();
            access
                .download("movie.mp4", &destination, &Progress::default())
                .await
                .unwrap();
            assert!(std::fs::read(&destination).unwrap() == bytes);
            assert!(!partial.exists());
        }

        fn pdf(len: usize) -> Vec<u8> {
            let mut bytes = b"%PDF-1.7\n".to_vec();
            bytes.extend(pattern(len.saturating_sub(bytes.len())));
            bytes
        }

        fn spool(fake: &Fake, name: &str, reply: PrintReply) {
            fake.prints.lock().unwrap().insert(name.into(), reply);
        }

        const STALL: Duration = Duration::from_secs(5);

        /// `print_jobs::fetch` into a fresh job folder; returns the result and the folder.
        async fn fetch_print(
            access: &SelkiesClient,
            name: &str,
            stall: Duration,
        ) -> (crate::print_jobs::FetchResult, tempfile::TempDir, PathBuf) {
            let local = tempfile::tempdir().unwrap();
            let job_dir = local.path().join("w1/1");
            let closed = AtomicBool::new(false);
            let result = crate::print_jobs::fetch(
                access,
                name,
                &job_dir,
                &crate::print_jobs::local_file_name(name),
                &Progress::default(),
                &closed,
                stall,
            )
            .await;
            (result, local, job_dir)
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn apps_catalog_and_icons_carry_the_cookie_and_no_origin() {
            let server = start().await;
            let (access, fake) = (&server.access, &server.fake);
            let catalog = access.apps_metadata().await.unwrap();
            let apps = crate::apps::parse_catalog(std::str::from_utf8(&catalog).unwrap()).unwrap();
            assert_eq!(apps[0].name, "a");
            let icon = access.apps_icon("a.svg").await.unwrap();
            assert!(icon.starts_with(b"<svg"));
            assert!(access.apps_icon("huge.png").await.is_err(), "over the cap");
            assert!(access.apps_icon("../x").await.is_err());
            let gets = requests(fake, "GET", "/sess-1/api/apps/");
            assert_eq!(gets.len(), 3, "the refused path never left");
            for get in &gets {
                assert_eq!(get.headers["cookie"], COOKIE_VALUE);
                assert!(!get.headers.contains_key("origin"));
            }
            assert_eq!(
                access.apps_metadata_url().as_str(),
                format!("{}api/apps/metadata.yml", access.base)
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_printed_document_is_fetched_once_with_the_cookie_and_no_origin() {
            let server = start().await;
            let (access, fake) = (&server.access, &server.fake);
            let bytes = pdf(10_000);
            spool(fake, "Report (2).pdf", PrintReply::Pdf(bytes.clone()));
            let (result, _local, job_dir) = fetch_print(access, "Report (2).pdf", STALL).await;
            let fetched = result.unwrap();
            assert_eq!(fetched.bytes, 10_000);
            assert!(fetched.printed_at.is_some(), "Last-Modified is read");
            assert!(std::fs::read(job_dir.join("Report (2).pdf")).unwrap() == bytes);
            assert!(!job_dir.join(".partial").exists());
            let gets = requests(fake, "GET", "/sess-1/api/print/");
            assert_eq!(gets.len(), 1);
            assert_eq!(gets[0].target, "/sess-1/api/print/Report%20(2).pdf");
            assert_eq!(gets[0].headers["cookie"], COOKIE_VALUE);
            assert!(!gets[0].headers.contains_key("origin"));
            assert!(!gets[0].headers.contains_key("range"));
            // The server took it: a second fetch finds nothing, quietly.
            let (again, _local, _) = fetch_print(access, "Report (2).pdf", STALL).await;
            assert_eq!(again, Err(FetchFailure::Gone));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_truncated_print_fails_and_leaves_no_file() {
            let server = start().await;
            let (access, fake) = (&server.access, &server.fake);
            spool(
                fake,
                "cut.pdf",
                PrintReply::Truncate {
                    claimed: 10_000,
                    sent: pdf(4_000),
                },
            );
            let (result, _local, job_dir) = fetch_print(access, "cut.pdf", STALL).await;
            assert!(matches!(result, Err(FetchFailure::Broken(_))), "{result:?}");
            assert!(!job_dir.join("cut.pdf").exists());
            assert!(!job_dir.join(".partial").exists());
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_refused_print_keeps_the_server_sentence() {
            let server = start().await;
            let (access, fake) = (&server.access, &server.fake);
            spool(
                fake,
                "a.pdf",
                PrintReply::Status("403 Forbidden", "Forbidden: printing disabled"),
            );
            let (result, _local, job_dir) = fetch_print(access, "a.pdf", STALL).await;
            assert_eq!(
                result,
                Err(FetchFailure::Refused(
                    "Forbidden: printing disabled.".into()
                ))
            );
            assert!(!job_dir.join(".partial").exists());
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_sealskin_cookie_refusal_says_reconnect() {
            let server = start().await;
            let (access, fake) = (&server.access, &server.fake);
            spool(fake, "a.pdf", PrintReply::Detail403);
            let (result, _local, _) = fetch_print(access, "a.pdf", STALL).await;
            let Err(failure) = result else {
                panic!("expected a failure");
            };
            assert!(failure.still_on_server(), "{failure:?}");
            let detail = failure.detail("a.pdf");
            assert_eq!(
                detail,
                "Couldn’t reach the session. Reconnect to receive a.pdf."
            );
            assert!(!detail.contains('{'));
            assert_eq!(
                server_message(
                    StatusCode::FORBIDDEN,
                    r#"{"detail":"Forbidden: Invalid session or token."}"#
                ),
                "Forbidden: Invalid session or token."
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_print_that_is_not_a_pdf_is_refused() {
            let server = start().await;
            let (access, fake) = (&server.access, &server.fake);
            spool(fake, "page.pdf", PrintReply::Pdf(vec![b'<'; 4096]));
            let (result, _local, job_dir) = fetch_print(access, "page.pdf", STALL).await;
            assert_eq!(result, Err(FetchFailure::NotPdf));
            assert!(!job_dir.join("page.pdf").exists());
            // A short body is checked at its end.
            spool(fake, "tiny.pdf", PrintReply::Pdf(b"hello".to_vec()));
            let (result, _local, _) = fetch_print(access, "tiny.pdf", STALL).await;
            assert_eq!(result, Err(FetchFailure::NotPdf));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_local_failure_sends_no_request() {
            let server = start().await;
            let (access, fake) = (&server.access, &server.fake);
            spool(fake, "a.pdf", PrintReply::Pdf(pdf(100)));
            let local = tempfile::tempdir().unwrap();
            // A file where the window's folder should be.
            std::fs::write(local.path().join("w1"), b"").unwrap();
            let result = crate::print_jobs::fetch(
                access,
                "a.pdf",
                &local.path().join("w1/1"),
                "a.pdf",
                &Progress::default(),
                &AtomicBool::new(false),
                STALL,
            )
            .await;
            assert!(matches!(result, Err(FetchFailure::Local(_))), "{result:?}");
            assert!(requests(fake, "GET", "/sess-1/api/print/").is_empty());
            assert!(fake.prints.lock().unwrap().contains_key("a.pdf"));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_print_never_sends_range_even_with_a_stale_partial() {
            let server = start().await;
            let (access, fake) = (&server.access, &server.fake);
            let bytes = pdf(5_000);
            spool(fake, "a.pdf", PrintReply::Pdf(bytes.clone()));
            let local = tempfile::tempdir().unwrap();
            let job_dir = local.path().join("1");
            std::fs::create_dir_all(&job_dir).unwrap();
            std::fs::write(job_dir.join(".partial"), vec![b'x'; 9_000]).unwrap();
            crate::print_jobs::fetch(
                access,
                "a.pdf",
                &job_dir,
                "a.pdf",
                &Progress::default(),
                &AtomicBool::new(false),
                STALL,
            )
            .await
            .unwrap();
            assert!(std::fs::read(job_dir.join("a.pdf")).unwrap() == bytes);
            let gets = requests(fake, "GET", "/sess-1/api/print/");
            assert!(!gets[0].headers.contains_key("range"));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_partial_content_answer_fails() {
            let server = start().await;
            let local = tempfile::tempdir().unwrap();
            let mut file = tokio::fs::File::create(local.path().join("p"))
                .await
                .unwrap();
            spool(
                &server.fake,
                "b.pdf",
                PrintReply::Status("206 Partial Content", "x"),
            );
            let result = server
                .access
                .print_document("b.pdf", &mut file, &Progress::default(), STALL)
                .await;
            assert!(matches!(result, Err(FetchFailure::Retry(_))), "{result:?}");
            assert_eq!(std::fs::metadata(local.path().join("p")).unwrap().len(), 0);
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_body_longer_than_its_length_or_the_cap_is_cut_and_removed() {
            let server = start().await;
            let (access, fake) = (&server.access, &server.fake);
            let local = tempfile::tempdir().unwrap();
            let mut file = tokio::fs::File::create(local.path().join("p"))
                .await
                .unwrap();
            // No length: cut once past the cap.
            spool(fake, "long.pdf", PrintReply::NoLength(pdf(9_000)));
            let result = access
                .print_document_capped("long.pdf", &mut file, &Progress::default(), STALL, 4_000)
                .await;
            assert!(
                matches!(result, Err(FetchFailure::TooLarge(_))),
                "{result:?}"
            );
            // A length over the cap: refused before the first byte is written.
            spool(fake, "huge.pdf", PrintReply::Huge);
            let mut file = tokio::fs::File::create(local.path().join("q"))
                .await
                .unwrap();
            let result = access
                .print_document("huge.pdf", &mut file, &Progress::default(), STALL)
                .await;
            assert_eq!(result, Err(FetchFailure::TooLarge(MAX_PRINT_BYTES + 1)));
            assert_eq!(std::fs::metadata(local.path().join("q")).unwrap().len(), 0);
            // Through `fetch`, the partial file is removed.
            spool(fake, "huge.pdf", PrintReply::Huge);
            let (result, _dir, job_dir) = fetch_print(access, "huge.pdf", STALL).await;
            assert!(matches!(result, Err(FetchFailure::TooLarge(_))));
            assert!(!job_dir.join(".partial").exists());
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn header_and_chunk_stalls_time_out() {
            let server = start().await;
            let (access, fake) = (&server.access, &server.fake);
            let stall = Duration::from_millis(300);
            spool(fake, "held.pdf", PrintReply::HoldHeaders);
            let (result, _local, _) = fetch_print(access, "held.pdf", stall).await;
            assert!(matches!(result, Err(FetchFailure::Retry(_))), "{result:?}");
            spool(fake, "quiet.pdf", PrintReply::StallBody(pdf(2_000)));
            let (result, _local, job_dir) = fetch_print(access, "quiet.pdf", stall).await;
            assert_eq!(result, Err(FetchFailure::Broken("stalled".into())));
            assert!(!job_dir.join(".partial").exists());
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_fetch_whose_window_closed_removes_its_folder() {
            let server = start().await;
            spool(&server.fake, "a.pdf", PrintReply::Pdf(pdf(100)));
            let local = tempfile::tempdir().unwrap();
            let job_dir = local.path().join("w1/1");
            let result = crate::print_jobs::fetch(
                &server.access,
                "a.pdf",
                &job_dir,
                "a.pdf",
                &Progress::default(),
                &AtomicBool::new(true),
                STALL,
            )
            .await;
            assert!(
                matches!(&result, Err(f) if f.still_on_server()),
                "{result:?}"
            );
            assert!(!job_dir.exists());
            assert!(requests(&server.fake, "GET", "/sess-1/api/print/").is_empty());
            assert!(server.fake.prints.lock().unwrap().contains_key("a.pdf"));
        }
    }
}
