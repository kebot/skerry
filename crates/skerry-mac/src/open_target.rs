//! Opening links and files in remote apps (`ARCH-028`, `ARCH-GAP-LS-03`). Pure: no GPUI,
//! no network, so every rule here is unit-tested.
//!
//! - What counts as a link, and the exact text sent: SealSkin checks every address against
//!   `^(?:(?:https?|ftp)://|magnet:\?)[^\s\x00-\x1f\x7f]+$` with at most 8192 characters
//!   (`models.py::LaunchRequestURL`, `OpenUrlRequest`). [`validate_url`] is that check, run on
//!   the very string that goes out.
//! - Which apps and sessions can take a link or a file (the catalog's `url_support` and
//!   `extensions`, as the web launcher's `visibleApps` reads them).
//! - The chooser's state ([`OpeningState`]): what shows, what waits, and what Return may do.
//!   Nothing that arrives from outside (a `skerry:` link, Finder, a drop) ever acts without a
//!   click, and a target never changes under an open Launch & Open dialog.

use crate::sealskin::{RemoteApp, RemoteSession};
use percent_encoding::percent_decode_str;
use reqwest::Url;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// SealSkin's `max_length` for an address. Pydantic counts characters, not bytes.
pub const MAX_URL_CHARS: usize = 8192;
/// `/launch/file` reads the whole upload into the server's memory; bigger files are copied
/// to a running session instead (Selkies uploads in slices).
pub const MAX_OPEN_FILE_BYTES: u64 = 4 << 30;
/// The most items waiting behind the one showing; a page spamming `skerry:` links stops here.
pub const QUEUE_CAP: usize = 32;
/// Rejected text is shown at most this long.
const REJECTED_LABEL_CHARS: usize = 80;
/// A link label is middle-truncated past this.
const LINK_LABEL_CHARS: usize = 64;

/// A link or a local file to open in a remote app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenTarget {
    /// The exact text sent to SealSkin; it passed [`validate_url`].
    Url(String),
    File(PathBuf),
}

/// Why text that looks like a link can't be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectReason {
    /// Not http, https, ftp, or a space-free magnet link.
    Scheme,
    /// Over [`MAX_URL_CHARS`].
    TooLong,
    /// An allowed scheme with spaces or control characters (a magnet whose `dn` wasn't
    /// encoded, say).
    Spaces,
    /// A `skerry:` link whose address can't be told apart (an unencoded `&`, two `url`s).
    Ambiguous,
}

impl RejectReason {
    pub fn note(self) -> &'static str {
        match self {
            Self::Scheme => "Skerry opens web (http, https), ftp and magnet links.",
            Self::Spaces => "This link contains spaces, which SealSkin doesn’t accept.",
            Self::TooLong => {
                "This link is longer than 8,192 characters, which SealSkin doesn’t accept."
            }
            Self::Ambiguous => {
                "That skerry: link holds more than one address. Encode the address, “&” as %26."
            }
        }
    }
}

/// What the omnibar's text is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Typed {
    Link(String),
    Rejected { label: String, reason: RejectReason },
    Search,
}

/// What one item from the platform's open-URLs callback is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incoming {
    Target(OpenTarget),
    /// A `skerry:` link whose address SealSkin wouldn't take: shown so the user sees why.
    Rejected {
        label: String,
        reason: RejectReason,
    },
    /// A folder from Finder or the Dock (`launch/file` takes one file).
    Folder(PathBuf),
    /// A `skerry:` link with no address to open.
    NoAddress,
    /// Anything else (a scheme Skerry never registered, a missing file).
    Ignored,
}

const ALLOWED_PREFIXES: [&str; 4] = ["http://", "https://", "ftp://", "magnet:?"];
const DANGEROUS_SCHEMES: [&str; 5] = ["javascript:", "data:", "file:", "blob:", "vbscript:"];

fn forbidden_char(c: char) -> bool {
    c.is_whitespace() || c.is_control()
}

/// SealSkin's address check, on the exact string to send: the literal lower-case prefix,
/// no whitespace or control characters, at most 8192 characters, and (for http, https and
/// ftp) a real host.
pub fn validate_url(url: &str) -> Result<(), RejectReason> {
    let Some(prefix) = ALLOWED_PREFIXES.iter().find(|p| url.starts_with(**p)) else {
        return Err(RejectReason::Scheme);
    };
    let rest = &url[prefix.len()..];
    if rest.is_empty() {
        return Err(RejectReason::Scheme);
    }
    if rest.chars().any(forbidden_char) {
        return Err(RejectReason::Spaces);
    }
    if url.chars().count() > MAX_URL_CHARS {
        return Err(RejectReason::TooLong);
    }
    if *prefix != "magnet:?" {
        // The authority as written: WHATWG parsing would turn `http:///path` into a host.
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = authority.rsplit('@').next().unwrap_or_default();
        let host = if host.starts_with('[') {
            host.split(']')
                .next()
                .unwrap_or_default()
                .trim_start_matches('[')
        } else {
            host.split(':').next().unwrap_or_default()
        };
        if host.is_empty() {
            return Err(RejectReason::Scheme);
        }
        let parsed = Url::parse(url).map_err(|_| RejectReason::Scheme)?;
        if parsed.host_str().is_none_or(str::is_empty) {
            return Err(RejectReason::Scheme);
        }
    }
    Ok(())
}

/// `HTTP://Example.com` → `http://Example.com`: the server's pattern is case-sensitive.
fn lower_scheme(text: &str) -> String {
    match text.split_once(':') {
        Some((scheme, rest)) if is_scheme(scheme) => {
            format!("{}:{rest}", scheme.to_ascii_lowercase())
        }
        _ => text.to_string(),
    }
}

fn is_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// `host[.host]+[:port][/path]`, as the web omnibar's `LOOKS_LIKE_URL`, with a letter in the
/// last label (or a dotted IPv4), so "3.14" stays a search.
fn bare_host(text: &str) -> bool {
    let (authority, path) = match text.find('/') {
        Some(at) => (&text[..at], &text[at..]),
        None => (text, ""),
    };
    if path.chars().any(forbidden_char) {
        return false;
    }
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if port.is_some_and(|port| port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit())) {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2
        || labels.iter().any(|label| {
            label.is_empty()
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return false;
    }
    let ipv4 = labels.len() == 4
        && labels
            .iter()
            .all(|label| label.len() <= 3 && label.bytes().all(|b| b.is_ascii_digit()));
    let last_has_letter = labels
        .last()
        .is_some_and(|label| label.bytes().any(|b| b.is_ascii_alphabetic()));
    ipv4 || last_has_letter
}

/// The address to send for typed text, or `None` when it isn't one. A bare host gets
/// `https://`; an explicit scheme is lower-cased; the rest is sent as typed (trimmed).
pub fn parse_typed_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let candidate = if bare_host(text) {
        format!("https://{text}")
    } else {
        lower_scheme(text)
    };
    validate_url(&candidate).ok().map(|()| candidate)
}

/// Top-level domains common enough that a bare `host.tld` typed in the omnibar is surely an
/// address. Anything else ("notes.txt", "report.pdf", "wiki.lan", an IP) still shows the
/// chooser but needs a click, so Return never opens a file name searched for (`ARCH-028`).
const RETURN_TLDS: [&str; 32] = [
    "com", "net", "org", "edu", "gov", "mil", "int", "io", "co", "dev", "app", "ai", "me", "info",
    "biz", "us", "uk", "de", "fr", "eu", "ca", "au", "jp", "cn", "in", "nl", "ch", "se", "no",
    "es", "it", "br",
];

/// Whether Return may act on typed text: an explicit scheme, or a bare host ending in a
/// well-known TLD.
pub fn return_may_open(text: &str) -> bool {
    let text = text.trim();
    if text.contains("://") || text.to_ascii_lowercase().starts_with("magnet:?") {
        return true;
    }
    let host = text.split(['/', ':', '?', '#']).next().unwrap_or_default();
    host.rsplit_once('.')
        .is_some_and(|(_, tld)| RETURN_TLDS.contains(&tld.to_ascii_lowercase().as_str()))
}

/// What the omnibar's text is: a link, clearly a link that can't be opened, or a search.
/// Text with whitespace is never a rejected link ("javascript tutorial" searches).
pub fn classify_typed(text: &str) -> Typed {
    let text = text.trim();
    if let Some(link) = parse_typed_url(text) {
        return Typed::Link(link);
    }
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return Typed::Search;
    }
    let lowered = lower_scheme(text);
    let has_scheme = text
        .split_once("://")
        .is_some_and(|(scheme, rest)| is_scheme(scheme) && !rest.is_empty());
    let dangerous = DANGEROUS_SCHEMES
        .iter()
        .any(|scheme| lowered.starts_with(scheme));
    if !has_scheme && !dangerous {
        return Typed::Search;
    }
    let reason = match validate_url(&lowered) {
        Err(reason @ (RejectReason::TooLong | RejectReason::Spaces)) => reason,
        _ => RejectReason::Scheme,
    };
    Typed::Rejected {
        label: truncate_end(text, REJECTED_LABEL_CHARS),
        reason,
    }
}

/// One item of the platform's open-URLs callback: a Finder file (`file://`), or a
/// `skerry://open?url=<percent-encoded>` hand-off. Nothing else was registered.
pub fn from_open_url(raw: &str) -> Incoming {
    let Ok(url) = Url::parse(raw) else {
        return Incoming::Ignored;
    };
    match url.scheme() {
        "file" => match url.to_file_path() {
            Ok(path) => match std::fs::metadata(&path) {
                Ok(meta) if meta.is_file() => Incoming::Target(OpenTarget::File(path)),
                Ok(meta) if meta.is_dir() => Incoming::Folder(path),
                _ => Incoming::Ignored,
            },
            Err(()) => Incoming::Ignored,
        },
        "skerry" => skerry_link(&url),
        _ => Incoming::Ignored,
    }
}

/// `skerry://open?url=…`: host `open`, no path, and exactly one query pair, named `url`.
fn skerry_link(url: &Url) -> Incoming {
    let host_ok = url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("open"));
    if !host_ok || !matches!(url.path(), "" | "/") {
        return Incoming::NoAddress;
    }
    let query = url.query().unwrap_or_default();
    let pairs: Vec<(&str, &str)> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
        .collect();
    let values: Vec<&str> = pairs
        .iter()
        .filter(|(key, _)| *key == "url")
        .map(|(_, value)| *value)
        .collect();
    if values.is_empty() || values.iter().all(|value| value.is_empty()) {
        return Incoming::NoAddress;
    }
    let decode = |value: &str| percent_decode_str(value).decode_utf8_lossy().into_owned();
    if pairs.len() != 1 {
        return Incoming::Rejected {
            label: truncate_end(&decode(query), REJECTED_LABEL_CHARS),
            reason: RejectReason::Ambiguous,
        };
    }
    let address = lower_scheme(decode(values[0]).trim());
    match validate_url(&address) {
        Ok(()) => Incoming::Target(OpenTarget::Url(address)),
        Err(reason) => Incoming::Rejected {
            label: truncate_end(&address, REJECTED_LABEL_CHARS),
            reason,
        },
    }
}

/// How a link is shown: the real host (never the userinfo before an `@`), so
/// `https://bank.example@evil.test/` reads `evil.test`. `https://` is dropped; `http://` and
/// `ftp://` stay, as they say something. A magnet shows its `dn`. Returns (label, tooltip).
pub fn display_link(url: &str) -> (String, String) {
    if let Some(query) = url.strip_prefix("magnet:?") {
        let name = query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == "dn")
            .map(|(_, value)| {
                percent_decode_str(&value.replace('+', " "))
                    .decode_utf8_lossy()
                    .into_owned()
            })
            .filter(|name| !name.trim().is_empty());
        let label = match name {
            Some(name) => format!("magnet link “{}”", truncate_end(&name, LINK_LABEL_CHARS)),
            None => "magnet link".to_string(),
        };
        return (label, truncate_end(url, 200));
    }
    let Ok(parsed) = Url::parse(url) else {
        return (truncate_end(url, LINK_LABEL_CHARS), truncate_end(url, 200));
    };
    let mut host = parsed.host_str().unwrap_or_default().to_string();
    if let Some(port) = parsed.port() {
        host.push_str(&format!(":{port}"));
    }
    if parsed.scheme() != "https" {
        host = format!("{}://{host}", parsed.scheme());
    }
    let mut rest = parsed.path().to_string();
    if rest == "/" {
        rest.clear();
    }
    if let Some(query) = parsed.query() {
        rest.push('?');
        rest.push_str(query);
    }
    let label = middle_truncate(&host, &rest, LINK_LABEL_CHARS);
    let signed = !parsed.username().is_empty() || parsed.password().is_some();
    let mut shown = parsed.clone();
    let _ = shown.set_username("");
    let _ = shown.set_password(None);
    let mut tooltip = truncate_end(shown.as_str(), 200);
    if signed {
        tooltip.push_str(" (with sign-in details)");
    }
    (label, tooltip)
}

/// `host` in full, then as much of `rest` as fits, cut in the middle: `wiki.lan/a/…/page`.
fn middle_truncate(host: &str, rest: &str, max: usize) -> String {
    let host_len = host.chars().count();
    let rest_len = rest.chars().count();
    if host_len + rest_len <= max {
        return format!("{host}{rest}");
    }
    let room = max.saturating_sub(host_len + 1);
    if room == 0 {
        return format!("{host}…");
    }
    let head = room / 2;
    let tail = room - head;
    let start: String = rest.chars().take(head).collect();
    let end: String = rest.chars().skip(rest_len - tail).collect();
    format!("{host}{start}…{end}")
}

fn truncate_end(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The extension after the last `.`, lower-cased, as the web launcher's
/// `filename.split('.').pop()` with its `includes('.')` guard: `.bashrc` → `bashrc`.
pub fn extension_of(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    let (_, ext) = name.rsplit_once('.')?;
    (!ext.is_empty()).then(|| ext.to_lowercase())
}

/// A catalog extension, compared without its dot: `.PDF` → `pdf`.
pub fn normalize_extension(declared: &str) -> String {
    declared.trim().trim_start_matches('.').to_lowercase()
}

pub fn app_opens_extension(declared: &[String], ext: &str) -> bool {
    declared
        .iter()
        .any(|declared| normalize_extension(declared) == ext)
}

fn newest_first(mut sessions: Vec<&RemoteSession>) -> Vec<&RemoteSession> {
    sessions.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
    sessions
}

/// Where a link can go: running sessions whose app opens links (newest first, never a
/// collaboration room, never an app missing from the catalog) and the apps that open links,
/// in the order given.
pub fn url_targets<'a>(
    apps: &'a [RemoteApp],
    sessions: &'a [RemoteSession],
) -> (Vec<&'a RemoteSession>, Vec<&'a RemoteApp>) {
    let opens = |app_id: &str| apps.iter().any(|app| app.id == app_id && app.url_support);
    let running = sessions
        .iter()
        .filter(|session| !session.is_collaboration && opens(&session.app_id))
        .collect();
    let apps = apps.iter().filter(|app| app.url_support).collect();
    (newest_first(running), apps)
}

/// Where a file can go: every solo session (to copy it to the Desktop), the ones whose app
/// lists the extension first; and the apps that list it, or every app (`true`) when none
/// does or the file has no extension.
pub fn file_targets<'a>(
    apps: &'a [RemoteApp],
    sessions: &'a [RemoteSession],
    ext: Option<&str>,
) -> (Vec<&'a RemoteSession>, Vec<&'a RemoteApp>, bool) {
    let lists = |app: &RemoteApp| ext.is_some_and(|ext| app_opens_extension(&app.extensions, ext));
    let session_lists = |session: &RemoteSession| {
        apps.iter()
            .any(|app| app.id == session.app_id && lists(app))
    };
    let mut running = newest_first(
        sessions
            .iter()
            .filter(|session| !session.is_collaboration)
            .collect(),
    );
    // Stable: newest first within each group.
    running.sort_by_key(|session| !session_lists(session));
    let matching: Vec<&RemoteApp> = apps.iter().filter(|app| lists(app)).collect();
    if matching.is_empty() {
        (running, apps.iter().collect(), true)
    } else {
        (running, matching, false)
    }
}

/// Where a top-level Shared Files file can open (`ARCH-037`): only apps that mount Shared
/// Files (they keep homes, or are meta apps; any other app gets an ephemeral
/// `Desktop/files`), the ones listing the extension first; every such app (`true`) when none
/// lists it. No running session: those already have the file in `Desktop › files`.
pub fn stored_targets<'a>(apps: &'a [RemoteApp], ext: Option<&str>) -> (Vec<&'a RemoteApp>, bool) {
    let mounts: Vec<&RemoteApp> = apps
        .iter()
        .filter(|app| app.home_directories || app.is_meta_app)
        .collect();
    let matching: Vec<&RemoteApp> = mounts
        .iter()
        .copied()
        .filter(|app| ext.is_some_and(|ext| app_opens_extension(&app.extensions, ext)))
        .collect();
    if matching.is_empty() {
        let fallback = !mounts.is_empty();
        (mounts, fallback)
    } else {
        (matching, false)
    }
}

/// What the chooser is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    Link(String),
    File(PathBuf),
    Rejected {
        label: String,
        reason: RejectReason,
    },
    /// A top-level file of Shared Files, from the Storage window's Open in… (`ARCH-037`).
    Stored {
        filename: String,
        size: u64,
    },
}

impl From<OpenTarget> for Subject {
    fn from(target: OpenTarget) -> Self {
        match target {
            OpenTarget::Url(url) => Self::Link(url),
            OpenTarget::File(path) => Self::File(path),
        }
    }
}

/// The context showing in the chooser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opening {
    pub subject: Subject,
    /// It came from the omnibar's own text; only then may Return act.
    pub typed: bool,
    /// Return may act: the typed text had a scheme or ends in a well-known TLD
    /// ([`return_may_open`]). A guess like "notes.txt" shows the chooser but needs a click.
    pub enter_ok: bool,
}

/// What Return in the omnibar does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowRef {
    Session(String),
    App(String),
}

/// The chooser's state: the context showing and what waits behind it (`ARCH-028`).
#[derive(Debug, Default)]
pub struct OpeningState {
    current: Option<Opening>,
    queue: VecDeque<Subject>,
    /// Bumped on every change; a chooser click from an older render is ignored.
    generation: u64,
    /// Items already handled in this run of the queue, for "2 of 3".
    taken: usize,
    /// A Launch & Open dialog is open or an `open_url` is pending: nothing replaces the
    /// current context, everything from outside queues.
    locked: bool,
}

impl OpeningState {
    pub fn current(&self) -> Option<&Opening> {
        self.current.as_ref()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    /// (position, total), e.g. (2, 3), counting the context showing and those queued.
    pub fn position(&self) -> (usize, usize) {
        let showing = usize::from(self.current.as_ref().is_some_and(|c| !c.typed));
        (self.taken + 1, self.taken + showing + self.queue.len())
    }

    fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn set_locked(&mut self, locked: bool) {
        if self.locked != locked {
            self.locked = locked;
            self.bump();
        }
    }

    /// Something from outside (a `skerry:` link, Finder, a drop, ⌘O). It shows at once only
    /// in place of nothing or of a typed link, and never while locked; otherwise it waits.
    /// Returns how many were dropped at the cap.
    pub fn push_external(&mut self, subject: Subject) -> usize {
        let replaceable = self.current.as_ref().is_none_or(|current| current.typed);
        if replaceable && !self.locked {
            if self.current.is_none() {
                self.taken = 0;
            }
            self.current = Some(Opening {
                subject,
                typed: false,
                enter_ok: false,
            });
            self.bump();
            return 0;
        }
        if self.queue.len() >= QUEUE_CAP {
            return 1;
        }
        self.queue.push_back(subject);
        self.bump();
        0
    }

    /// The omnibar's text changed. A typed link shows, or a typed context goes away; it
    /// never replaces a context from outside, and nothing changes while locked.
    #[cfg(test)]
    pub fn set_typed(&mut self, subject: Option<Subject>) {
        self.set_typed_opening(subject, true);
    }

    /// The omnibar's text changed: classify it, and allow Return only when the text is
    /// clearly an address (`return_may_open`).
    pub fn set_typed_text(&mut self, text: &str) {
        let subject = match classify_typed(text) {
            Typed::Link(url) => Some(Subject::Link(url)),
            Typed::Rejected { label, reason } => Some(Subject::Rejected { label, reason }),
            Typed::Search => None,
        };
        self.set_typed_opening(subject, return_may_open(text));
    }

    fn set_typed_opening(&mut self, subject: Option<Subject>, enter_ok: bool) {
        if self.locked || self.current.as_ref().is_some_and(|current| !current.typed) {
            return;
        }
        let next = subject.map(|subject| Opening {
            subject,
            typed: true,
            enter_ok,
        });
        if next == self.current {
            return;
        }
        self.current = next;
        self.taken = 0;
        self.bump();
    }

    #[cfg(test)]
    pub fn clear_typed(&mut self) {
        self.set_typed(None);
    }

    /// The next queued item, or nothing. Skip and a finished action both do this.
    pub fn advance(&mut self) {
        if self.current.as_ref().is_some_and(|current| !current.typed) {
            self.taken += 1;
        }
        self.current = self.queue.pop_front().map(|subject| Opening {
            subject,
            typed: false,
            enter_ok: false,
        });
        if self.current.is_none() {
            self.taken = 0;
        }
        self.locked = false;
        self.bump();
    }

    pub fn skip(&mut self) {
        self.advance();
    }

    /// A row's action was done (a link opened, a copy started, a launch sent).
    pub fn advance_after_action(&mut self) {
        self.advance();
    }

    /// ✕ or sign-out: the context and everything queued.
    pub fn cancel_all(&mut self) {
        self.current = None;
        self.queue.clear();
        self.taken = 0;
        self.locked = false;
        self.bump();
    }

    /// Return in the omnibar: the first row, only for a link typed there. A context from
    /// outside needs a click.
    pub fn enter_action(
        &self,
        first_session: Option<&str>,
        first_app: Option<&str>,
    ) -> Option<RowRef> {
        let current = self.current.as_ref()?;
        if !current.typed
            || !current.enter_ok
            || self.locked
            || !matches!(current.subject, Subject::Link(_))
        {
            return None;
        }
        first_session
            .map(|id| RowRef::Session(id.to_string()))
            .or_else(|| first_app.map(|id| RowRef::App(id.to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn typed(text: &str) -> Option<String> {
        parse_typed_url(text)
    }

    #[test]
    fn typed_links_follow_the_web_omnibar() {
        let cases: &[(&str, Option<&str>)] = &[
            (
                "https://example.com/a?b=c",
                Some("https://example.com/a?b=c"),
            ),
            ("example.com", Some("https://example.com")),
            ("wiki.lan:8080/page", Some("https://wiki.lan:8080/page")),
            ("192.168.1.10/admin", Some("https://192.168.1.10/admin")),
            (
                "ftp://files.example.org/pub",
                Some("ftp://files.example.org/pub"),
            ),
            (
                "magnet:?xt=urn:btih:abc&dn=x",
                Some("magnet:?xt=urn:btih:abc&dn=x"),
            ),
            ("MAGNET:?xt=urn:btih:abc", Some("magnet:?xt=urn:btih:abc")),
            ("notes.txt", Some("https://notes.txt")),
            ("magnet:?xt=urn:btih:abc dn", None),
            ("magnet:xt=urn", None),
            ("javascript:alert(1)", None),
            ("file:///etc/passwd", None),
            ("data:text/html,x", None),
            ("skerry://open?url=x", None),
            ("vnc://h", None),
            ("https://exa mple.com", None),
            ("https://a.com/\u{7}", None),
            ("https://a.com/\u{7f}", None),
            ("https://a.com/\u{a0}x", None),
            ("https://", None),
            ("http:///path", None),
            ("https:\\\\a.com", None),
            ("https:/a.com", None),
            ("https:a.com", None),
            ("3.14", None),
            ("1.2", None),
            ("firefox", None),
            ("zen browser", None),
        ];
        for (input, expected) in cases {
            assert_eq!(typed(input).as_deref(), *expected, "{input:?}");
        }
    }

    #[test]
    fn typed_scheme_is_lowercased_for_server_pattern() {
        assert_eq!(
            typed("  HTTP://Example.com  ").as_deref(),
            Some("http://Example.com")
        );
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        let base = "https://a.com/";
        let fits = format!("{base}{}", "é".repeat(MAX_URL_CHARS - base.len()));
        assert_eq!(fits.chars().count(), MAX_URL_CHARS);
        assert!(typed(&fits).is_some());
        let over = format!("{fits}x");
        assert!(typed(&over).is_none());
        assert_eq!(validate_url(&over), Err(RejectReason::TooLong));
        let ascii = format!("{base}{}", "x".repeat(MAX_URL_CHARS - base.len()));
        assert!(typed(&ascii).is_some());
        assert!(typed(&format!("{ascii}x")).is_none());
    }

    #[test]
    fn validate_url_matches_server_pattern() {
        let server =
            regex::Regex::new(r"^(?:(?:https?|ftp)://|magnet:\?)[^\s\x00-\x1f\x7f]+$").unwrap();
        let inputs = [
            "https://example.com/a?b=c",
            "  HTTP://Example.com  ",
            "example.com",
            "wiki.lan:8080/page",
            "192.168.1.10/admin",
            "ftp://files.example.org/pub",
            "magnet:?xt=urn:btih:abc&dn=x",
            "MAGNET:?xt=1",
            "notes.txt",
            "https://bank.example@evil.test/login",
            "https://a.com/é",
        ];
        for input in inputs {
            let sent = typed(input).unwrap_or_else(|| panic!("{input} is a link"));
            assert!(server.is_match(&sent), "{sent}");
            assert!(sent.chars().count() <= MAX_URL_CHARS);
        }
    }

    #[test]
    fn classify_tells_rejected_links_from_searches() {
        assert_eq!(
            classify_typed("wiki.lan/page"),
            Typed::Link("https://wiki.lan/page".into())
        );
        assert!(matches!(
            classify_typed("vnc://desk.lan"),
            Typed::Rejected { reason: RejectReason::Scheme, ref label } if label == "vnc://desk.lan"
        ));
        assert!(matches!(
            classify_typed("javascript:alert(1)"),
            Typed::Rejected {
                reason: RejectReason::Scheme,
                ..
            }
        ));
        let long = format!("https://a.com/{}", "x".repeat(MAX_URL_CHARS));
        match classify_typed(&long) {
            Typed::Rejected { reason, label } => {
                assert_eq!(reason, RejectReason::TooLong);
                assert_eq!(label.chars().count(), REJECTED_LABEL_CHARS);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(classify_typed("javascript tutorial"), Typed::Search);
        assert_eq!(classify_typed("javascript: tutorial"), Typed::Search);
        assert_eq!(classify_typed("localhost"), Typed::Search);
        assert_eq!(classify_typed("notes"), Typed::Search);
        assert_eq!(classify_typed("1.2"), Typed::Search);
        assert_eq!(classify_typed(""), Typed::Search);
    }

    #[test]
    fn open_urls_read_files_and_skerry_links_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a b.pdf");
        std::fs::write(&file, b"x").unwrap();
        let url = Url::from_file_path(&file).unwrap();
        assert!(url.as_str().ends_with("a%20b.pdf"));
        assert_eq!(
            from_open_url(url.as_str()),
            Incoming::Target(OpenTarget::File(file.clone()))
        );
        let folder = Url::from_directory_path(dir.path()).unwrap();
        assert!(matches!(
            from_open_url(folder.as_str()),
            Incoming::Folder(_)
        ));
        let missing = Url::from_file_path(dir.path().join("gone.pdf")).unwrap();
        assert_eq!(from_open_url(missing.as_str()), Incoming::Ignored);
        assert_eq!(
            from_open_url("skerry://open?url=https%3A%2F%2Fa.com"),
            Incoming::Target(OpenTarget::Url("https://a.com".into()))
        );
        assert_eq!(
            from_open_url("skerry://open?url=https%3A%2F%2Fa.com%2F%3Fq%3Da%2Bb"),
            Incoming::Target(OpenTarget::Url("https://a.com/?q=a+b".into()))
        );
        assert_eq!(
            from_open_url("skerry://OPEN?url=https%3A%2F%2Fa.com"),
            Incoming::Target(OpenTarget::Url("https://a.com".into()))
        );
        assert!(matches!(
            from_open_url("skerry://open?url=javascript%3Ax"),
            Incoming::Rejected {
                reason: RejectReason::Scheme,
                ..
            }
        ));
        assert!(matches!(
            from_open_url("skerry://open?url=https://a.com/?x=1&y=2"),
            Incoming::Rejected {
                reason: RejectReason::Ambiguous,
                ..
            }
        ));
        assert!(matches!(
            from_open_url("skerry://open?url=a&url=b"),
            Incoming::Rejected {
                reason: RejectReason::Ambiguous,
                ..
            }
        ));
        assert_eq!(
            from_open_url("skerry://open/extra?url=https%3A%2F%2Fa.com"),
            Incoming::NoAddress
        );
        assert_eq!(from_open_url("skerry://other"), Incoming::NoAddress);
        assert_eq!(from_open_url("skerry://open"), Incoming::NoAddress);
        assert_eq!(from_open_url("https://a.com"), Incoming::Ignored);
        assert_eq!(from_open_url("not a url"), Incoming::Ignored);
    }

    #[test]
    fn links_show_the_real_host() {
        let cases = [
            ("https://wiki.lan/page", "wiki.lan/page"),
            ("https://wiki.lan/", "wiki.lan"),
            ("http://intranet/x", "http://intranet/x"),
            ("ftp://files.lan/pub/a.iso", "ftp://files.lan/pub/a.iso"),
            ("https://wiki.lan:8080/p?q=1", "wiki.lan:8080/p?q=1"),
            (
                "magnet:?xt=urn:btih:abc&dn=ubuntu-24.04.iso",
                "magnet link “ubuntu-24.04.iso”",
            ),
            (
                "magnet:?xt=urn:btih:abc&dn=my%20file+name.iso",
                "magnet link “my file name.iso”",
            ),
            ("magnet:?xt=urn:btih:abc", "magnet link"),
        ];
        for (url, label) in cases {
            assert_eq!(display_link(url).0, label, "{url}");
        }
        let (label, tooltip) = display_link("https://bank.example@evil.test/login");
        assert_eq!(label, "evil.test/login");
        assert!(!tooltip.contains("bank.example"), "{tooltip}");
        assert!(tooltip.ends_with("(with sign-in details)"), "{tooltip}");
        let long = format!("https://wiki.lan/{}/page", "segment/".repeat(20));
        let (label, _) = display_link(&long);
        assert!(label.starts_with("wiki.lan/"), "{label}");
        assert!(label.ends_with("/page"), "{label}");
        assert!(label.contains('…'));
        assert_eq!(label.chars().count(), LINK_LABEL_CHARS);
    }

    #[test]
    fn extensions_compare_without_dot_or_case() {
        assert_eq!(normalize_extension(".PDF"), "pdf");
        assert_eq!(normalize_extension("pdf"), "pdf");
        assert_eq!(extension_of(Path::new("a.tar.GZ")).as_deref(), Some("gz"));
        assert_eq!(extension_of(Path::new("Makefile")), None);
        assert_eq!(
            extension_of(Path::new(".bashrc")).as_deref(),
            Some("bashrc")
        );
        assert_eq!(
            extension_of(Path::new("/x/y/report.Pdf")).as_deref(),
            Some("pdf")
        );
        assert!(app_opens_extension(&[".html".into(), "PDF".into()], "pdf"));
    }

    fn app(id: &str, url_support: bool, extensions: &[&str]) -> RemoteApp {
        serde_json::from_value(json!({
            "id": id, "name": id, "url_support": url_support, "extensions": extensions,
        }))
        .unwrap()
    }

    fn session(id: &str, app_id: &str, created_at: f64, collaboration: bool) -> RemoteSession {
        serde_json::from_value(json!({
            "session_id": id, "app_id": app_id, "app_name": app_id, "created_at": created_at,
            "session_url": format!("/{id}/"), "is_collaboration": collaboration,
        }))
        .unwrap()
    }

    fn ids_of_sessions(sessions: &[&RemoteSession]) -> Vec<String> {
        sessions.iter().map(|s| s.session_id.clone()).collect()
    }

    fn ids_of_apps(apps: &[&RemoteApp]) -> Vec<String> {
        apps.iter().map(|a| a.id.clone()).collect()
    }

    #[test]
    fn link_targets_are_url_apps_and_their_solo_sessions() {
        let apps = [
            app("zen", true, &[]),
            app("office", false, &["pdf"]),
            app("chromium", true, &[]),
        ];
        let sessions = [
            session("s1", "zen", 10.0, false),
            session("s2", "chromium", 30.0, false),
            session("s3", "zen", 20.0, true),
            session("s4", "office", 40.0, false),
            session("s5", "gone", 50.0, false),
        ];
        let (running, apps) = url_targets(&apps, &sessions);
        assert_eq!(ids_of_sessions(&running), ["s2", "s1"]);
        assert_eq!(ids_of_apps(&apps), ["zen", "chromium"]);
    }

    fn stored_app(id: &str, homes: bool, meta: bool, extensions: &[&str]) -> RemoteApp {
        serde_json::from_value(json!({
            "id": id, "name": id, "home_directories": homes, "is_meta_app": meta,
            "extensions": extensions,
        }))
        .unwrap()
    }

    #[test]
    fn stored_files_offer_apps_by_extension_and_no_sessions() {
        let apps = [
            stored_app("office", true, false, &["pdf"]),
            stored_app("zen", true, false, &[]),
            stored_app("desk", false, true, &["pdf"]),
        ];
        let (offered, fallback) = stored_targets(&apps, Some("pdf"));
        assert_eq!(ids_of_apps(&offered), ["office", "desk"]);
        assert!(!fallback);
        let (offered, fallback) = stored_targets(&apps, Some("xyz"));
        assert_eq!(ids_of_apps(&offered), ["office", "zen", "desk"]);
        assert!(fallback);
    }

    #[test]
    fn stored_files_skip_apps_without_homes() {
        let apps = [
            stored_app("kiosk", false, false, &["pdf"]),
            stored_app("office", true, false, &["docx"]),
        ];
        let (offered, _) = stored_targets(&apps, Some("pdf"));
        assert_eq!(ids_of_apps(&offered), ["office"]);
        let none = [stored_app("kiosk", false, false, &["pdf"])];
        let (offered, fallback) = stored_targets(&none, Some("pdf"));
        assert!(offered.is_empty() && !fallback, "never every app");
    }

    #[test]
    fn file_targets_filter_by_extension_or_fall_back_to_all() {
        let apps = [
            app("office", false, &[".PDF", "docx"]),
            app("zen", true, &["html"]),
            app("evince", false, &["pdf"]),
        ];
        let sessions = [
            session("s1", "zen", 30.0, false),
            session("s2", "office", 10.0, false),
            session("s3", "evince", 20.0, true),
        ];
        let (running, matching, fallback) = file_targets(&apps, &sessions, Some("pdf"));
        assert_eq!(ids_of_apps(&matching), ["office", "evince"]);
        assert!(!fallback);
        // The session whose app lists .pdf first, then newest; never the room.
        assert_eq!(ids_of_sessions(&running), ["s2", "s1"]);
        let (_, all, fallback) = file_targets(&apps, &sessions, Some("xyz"));
        assert_eq!(all.len(), 3);
        assert!(fallback);
        let (_, all, fallback) = file_targets(&apps, &sessions, None);
        assert_eq!(all.len(), 3);
        assert!(fallback);
    }

    fn file(name: &str) -> Subject {
        Subject::File(PathBuf::from(name))
    }

    #[test]
    fn files_queue_in_order_with_a_position() {
        let mut state = OpeningState::default();
        state.push_external(file("a.pdf"));
        state.push_external(file("b.pdf"));
        state.push_external(file("c.pdf"));
        assert_eq!(state.current().unwrap().subject, file("a.pdf"));
        assert_eq!(state.position(), (1, 3));
        state.advance_after_action();
        assert_eq!(state.current().unwrap().subject, file("b.pdf"));
        assert_eq!(state.position(), (2, 3));
        state.skip();
        assert_eq!(state.position(), (3, 3));
        state.skip();
        assert!(state.current().is_none());
        state.push_external(file("d.pdf"));
        assert_eq!(state.position(), (1, 1));
    }

    #[test]
    fn typed_text_never_replaces_a_file() {
        let mut state = OpeningState::default();
        state.push_external(file("a.pdf"));
        state.set_typed(Some(Subject::Link("https://a.com".into())));
        assert_eq!(state.current().unwrap().subject, file("a.pdf"));
        state.clear_typed();
        assert_eq!(state.current().unwrap().subject, file("a.pdf"));
    }

    #[test]
    fn an_external_link_replaces_a_typed_link_but_queues_behind_a_file() {
        let mut state = OpeningState::default();
        state.set_typed(Some(Subject::Link("https://typed.com".into())));
        state.push_external(Subject::Link("https://outside.com".into()));
        let current = state.current().unwrap();
        assert_eq!(current.subject, Subject::Link("https://outside.com".into()));
        assert!(!current.typed);
        state.cancel_all();
        state.push_external(file("a.pdf"));
        state.push_external(Subject::Link("https://outside.com".into()));
        assert_eq!(state.current().unwrap().subject, file("a.pdf"));
        assert_eq!(state.queued(), 1);
    }

    #[test]
    fn the_queue_is_capped() {
        let mut state = OpeningState::default();
        state.push_external(file("first"));
        let dropped: usize = (0..QUEUE_CAP + 5)
            .map(|n| state.push_external(file(&format!("{n}"))))
            .sum();
        assert_eq!(state.queued(), QUEUE_CAP);
        assert_eq!(dropped, 5);
    }

    #[test]
    fn the_generation_bumps_on_every_change() {
        let mut state = OpeningState::default();
        let mut last = state.generation();
        let mut check = |state: &OpeningState| {
            assert!(state.generation() != last);
            last = state.generation();
        };
        state.set_typed(Some(Subject::Link("https://a.com".into())));
        check(&state);
        state.push_external(file("a"));
        check(&state);
        state.push_external(file("b"));
        check(&state);
        state.set_locked(true);
        check(&state);
        state.skip();
        check(&state);
        state.cancel_all();
        check(&state);
    }

    #[test]
    fn external_open_never_replaces_target_under_open_dialog() {
        let mut state = OpeningState::default();
        state.set_typed(Some(Subject::Link("https://wiki.lan/page".into())));
        // The Launch & Open dialog (or a pending open_url) holds the context.
        state.set_locked(true);
        state.push_external(Subject::Link("https://evil.test".into()));
        state.set_typed(Some(Subject::Link("https://other.test".into())));
        assert_eq!(
            state.current().unwrap().subject,
            Subject::Link("https://wiki.lan/page".into())
        );
        assert_eq!(state.queued(), 1);
        // Done: the queued link shows next, as a context from outside.
        state.advance_after_action();
        let current = state.current().unwrap();
        assert_eq!(current.subject, Subject::Link("https://evil.test".into()));
        assert!(!current.typed);
    }

    #[test]
    fn return_acts_only_on_a_typed_link() {
        let mut state = OpeningState::default();
        assert_eq!(state.enter_action(Some("s1"), Some("zen")), None);
        state.set_typed(Some(Subject::Link("https://a.com".into())));
        assert_eq!(
            state.enter_action(Some("s1"), Some("zen")),
            Some(RowRef::Session("s1".into()))
        );
        assert_eq!(
            state.enter_action(None, Some("zen")),
            Some(RowRef::App("zen".into()))
        );
        state.set_locked(true);
        assert_eq!(state.enter_action(Some("s1"), None), None);
        state.set_locked(false);
        state.set_typed(Some(Subject::Rejected {
            label: "vnc://x".into(),
            reason: RejectReason::Scheme,
        }));
        assert_eq!(state.enter_action(Some("s1"), None), None);
        // A link from outside, or a file, needs a click.
        state.cancel_all();
        state.push_external(Subject::Link("https://a.com".into()));
        assert_eq!(state.enter_action(Some("s1"), Some("zen")), None);
        state.cancel_all();
        state.push_external(file("a.pdf"));
        assert_eq!(state.enter_action(Some("s1"), Some("zen")), None);
    }

    #[test]
    fn return_acts_only_on_clear_addresses() {
        for text in [
            "https://notes.txt",
            "example.com/a",
            "wiki.Example.ORG",
            "magnet:?xt=1",
        ] {
            assert!(return_may_open(text), "{text}");
        }
        for text in [
            "notes.txt",
            "report.pdf",
            "wiki.lan/page",
            "1.2.3.4",
            "localhost",
        ] {
            assert!(!return_may_open(text), "{text}");
        }
        let mut state = OpeningState::default();
        state.set_typed_text("notes.txt");
        assert!(matches!(state.current().unwrap().subject, Subject::Link(_)));
        assert_eq!(state.enter_action(Some("s1"), Some("zen")), None);
        state.set_typed_text("example.com");
        assert_eq!(
            state.enter_action(Some("s1"), Some("zen")),
            Some(RowRef::Session("s1".into()))
        );
    }

    #[test]
    fn a_magnet_with_spaces_says_so() {
        assert!(matches!(
            from_open_url("skerry://open?url=magnet%3A%3Fxt%3Durn%3Abtih%3Aabc%26dn%3DMy%20File"),
            Incoming::Rejected {
                reason: RejectReason::Spaces,
                ..
            }
        ));
        assert_eq!(
            RejectReason::Spaces.note(),
            "This link contains spaces, which SealSkin doesn’t accept."
        );
    }
}
