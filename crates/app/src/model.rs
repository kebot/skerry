//! Pure UI model: no GPUI, no I/O. Everything here is unit-tested.

use rb_cdp::{ConnectionState, FailureReason, TargetInfo};

/// What to do when the user presses the button of an [`Overlay`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayAction {
    /// Launch the Profile on the Manager, then reconnect.
    StartProfile,
    /// Try to connect again now.
    Retry,
}

/// A message that covers the Mirror (or the whole window before a Connection
/// exists): what is wrong and what the user can do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlay {
    pub title: String,
    pub detail: Option<String>,
    pub action: Option<OverlayAction>,
}

impl Overlay {
    pub fn new(title: impl Into<String>) -> Self {
        Overlay {
            title: title.into(),
            detail: None,
            action: None,
        }
    }

    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }

    pub fn action(mut self, a: OverlayAction) -> Self {
        self.action = Some(a);
        self
    }
}

/// The overlay for a Connection state, if one is needed. `can_start` is true
/// for Manager Connections (a stopped Profile can be started).
pub fn overlay_for(state: &ConnectionState, can_start: bool) -> Option<Overlay> {
    match state {
        ConnectionState::Connected => None,
        ConnectionState::Connecting => Some(Overlay::new("Connecting...")),
        ConnectionState::Reconnecting { attempt, .. } => Some(
            Overlay::new("Reconnecting...")
                .detail(format!("The connection was lost (attempt {attempt}).")),
        ),
        ConnectionState::NotRunning if can_start => Some(
            Overlay::new("Profile not running")
                .detail("The browser Profile is stopped.")
                .action(OverlayAction::StartProfile),
        ),
        ConnectionState::NotRunning => Some(
            Overlay::new("Remote browser not found")
                .detail("The server answered 404 (not running or wrong URL).")
                .action(OverlayAction::Retry),
        ),
        ConnectionState::Failed { reason } => Some(
            match reason {
                FailureReason::Unauthorized => Overlay::new("Authentication failed")
                    .detail("The token was rejected (401). Check `token` in config.toml."),
                FailureReason::OriginRejected => Overlay::new("Connection refused")
                    .detail("The server rejected the connection (403, origin)."),
            }
            .action(OverlayAction::Retry),
        ),
        ConnectionState::Closed => Some(Overlay::new("Connection closed")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Warn,
    Bad,
}

/// One line for the status bar.
pub fn status_line(name: &str, state: &ConnectionState) -> (String, Tone) {
    match state {
        ConnectionState::Connecting => (format!("{name}: connecting..."), Tone::Warn),
        ConnectionState::Connected => (format!("{name}: connected"), Tone::Good),
        ConnectionState::Reconnecting { attempt, .. } => (
            format!("{name}: reconnecting (attempt {attempt})..."),
            Tone::Warn,
        ),
        ConnectionState::NotRunning => (format!("{name}: Profile not running"), Tone::Bad),
        ConnectionState::Failed {
            reason: FailureReason::Unauthorized,
        } => (format!("{name}: authentication failed"), Tone::Bad),
        ConnectionState::Failed {
            reason: FailureReason::OriginRejected,
        } => (format!("{name}: connection refused"), Tone::Bad),
        ConnectionState::Closed => (format!("{name}: closed"), Tone::Bad),
    }
}

/// One Sidebar row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabRow {
    pub target_id: String,
    pub title: String,
    pub url: String,
    pub active: bool,
}

/// Sidebar rows for the page Tabs, in Connection order.
pub fn tab_rows(pages: &[TargetInfo], active: Option<&str>) -> Vec<TabRow> {
    pages
        .iter()
        .map(|p| TabRow {
            target_id: p.target_id.clone(),
            title: if p.title.trim().is_empty() {
                if p.url.is_empty() {
                    "New Tab".to_string()
                } else {
                    shorten_url(&p.url, 40)
                }
            } else {
                p.title.clone()
            },
            url: shorten_url(&p.url, 44),
            active: active == Some(p.target_id.as_str()),
        })
        .collect()
}

/// Keep the current Active Tab if it still exists, else the first Tab, else none.
pub fn choose_active(pages: &[TargetInfo], current: Option<&str>) -> Option<String> {
    match current {
        Some(c) if pages.iter().any(|p| p.target_id == c) => Some(c.to_string()),
        _ => pages.first().map(|p| p.target_id.clone()),
    }
}

/// Drop the scheme and a leading `www.`, and cut to `max` characters with an
/// ellipsis. For display only.
pub fn shorten_url(url: &str, max: usize) -> String {
    let s = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let s = s.strip_prefix("www.").unwrap_or(s);
    let s = s.strip_suffix('/').unwrap_or(s);
    truncate(s, max)
}

/// Cut to `max` characters (not bytes), ending in an ellipsis if cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn is_local_host(host: &str) -> bool {
    let host = host.split(':').next().unwrap_or(host);
    host == "localhost" || host.parse::<std::net::Ipv4Addr>().is_ok() || host.ends_with(".local")
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Turn what the user typed in the address field into a URL for
/// `Page.navigate`. `None` for empty input.
///
/// * anything with a scheme (`https://`, `about:blank`, `file:`) is kept;
/// * `localhost`, IPv4 addresses and `.local` hosts get `http://`;
/// * other single words with a dot (`example.com/path`) get `https://`;
/// * everything else (spaces, no dot) becomes a web search.
pub fn normalize_url(input: &str) -> Option<String> {
    let t = input.trim();
    if t.is_empty() {
        return None;
    }
    let has_scheme = t.contains("://")
        || ["about:", "data:", "file:", "chrome:", "view-source:"]
            .iter()
            .any(|p| t.starts_with(p));
    if has_scheme {
        return Some(t.to_string());
    }
    if t.chars().any(char::is_whitespace) {
        return Some(format!("https://duckduckgo.com/?q={}", percent_encode(t)));
    }
    let host = t.split(['/', '?', '#']).next().unwrap_or(t);
    if is_local_host(host) {
        return Some(format!("http://{t}"));
    }
    if host.contains('.') {
        return Some(format!("https://{t}"));
    }
    Some(format!("https://duckduckgo.com/?q={}", percent_encode(t)))
}

/// Index of the history entry to go to, `delta` steps from `current`
/// (`-1` back, `+1` forward), if it exists.
pub fn history_target(current: i64, len: usize, delta: i64) -> Option<usize> {
    let n = current.checked_add(delta)?;
    (n >= 0 && (n as usize) < len).then_some(n as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn page(id: &str, title: &str, url: &str) -> TargetInfo {
        TargetInfo {
            target_id: id.into(),
            kind: "page".into(),
            title: title.into(),
            url: url.into(),
            attached: false,
            opener_id: None,
        }
    }

    #[test]
    fn normalizes_urls() {
        assert_eq!(normalize_url("example.com").unwrap(), "https://example.com");
        assert_eq!(
            normalize_url("  example.com/a?b=1 ").unwrap(),
            "https://example.com/a?b=1"
        );
        assert_eq!(normalize_url("http://x.org").unwrap(), "http://x.org");
        assert_eq!(normalize_url("about:blank").unwrap(), "about:blank");
        assert_eq!(
            normalize_url("localhost:3000/x").unwrap(),
            "http://localhost:3000/x"
        );
        assert_eq!(
            normalize_url("192.168.1.5:8080").unwrap(),
            "http://192.168.1.5:8080"
        );
        assert_eq!(
            normalize_url("rust lang").unwrap(),
            "https://duckduckgo.com/?q=rust+lang"
        );
        assert_eq!(
            normalize_url("hello").unwrap(),
            "https://duckduckgo.com/?q=hello"
        );
        assert_eq!(normalize_url("   "), None);
        assert_eq!(normalize_url(""), None);
    }

    #[test]
    fn shortens_and_truncates() {
        assert_eq!(shorten_url("https://www.example.com/", 40), "example.com");
        assert_eq!(
            shorten_url("https://example.com/abcdefghij", 10),
            "example.c…"
        );
        assert_eq!(truncate("héllo", 5), "héllo");
        assert_eq!(truncate("héllo wörld", 6), "héllo…");
    }

    #[test]
    fn rows_mark_the_active_tab_and_fall_back_to_url() {
        let pages = [
            page("A", "Alpha", "https://a.test/"),
            page("B", "", "https://b.test/x"),
        ];
        let rows = tab_rows(&pages, Some("B"));
        assert_eq!(rows[0].title, "Alpha");
        assert!(!rows[0].active);
        assert_eq!(rows[1].title, "b.test/x");
        assert!(rows[1].active);
        assert_eq!(tab_rows(&[page("C", "", "")], None)[0].title, "New Tab");
    }

    #[test]
    fn active_tab_choice() {
        let pages = [page("A", "", ""), page("B", "", "")];
        assert_eq!(choose_active(&pages, None).as_deref(), Some("A"));
        assert_eq!(choose_active(&pages, Some("B")).as_deref(), Some("B"));
        assert_eq!(choose_active(&pages, Some("gone")).as_deref(), Some("A"));
        assert_eq!(choose_active(&[], Some("B")), None);
    }

    #[test]
    fn overlays_follow_connection_state() {
        assert_eq!(overlay_for(&ConnectionState::Connected, true), None);
        let o = overlay_for(
            &ConnectionState::Reconnecting {
                attempt: 2,
                next_retry_in: Duration::from_secs(2),
            },
            true,
        )
        .unwrap();
        assert_eq!(o.title, "Reconnecting...");
        let o = overlay_for(&ConnectionState::NotRunning, true).unwrap();
        assert_eq!(o.title, "Profile not running");
        assert_eq!(o.action, Some(OverlayAction::StartProfile));
        let o = overlay_for(&ConnectionState::NotRunning, false).unwrap();
        assert_eq!(o.action, Some(OverlayAction::Retry));
        let o = overlay_for(
            &ConnectionState::Failed {
                reason: FailureReason::Unauthorized,
            },
            true,
        )
        .unwrap();
        assert_eq!(o.title, "Authentication failed");
    }

    #[test]
    fn status_lines() {
        assert_eq!(
            status_line("work", &ConnectionState::Connected),
            ("work: connected".to_string(), Tone::Good)
        );
        assert_eq!(status_line("w", &ConnectionState::NotRunning).1, Tone::Bad);
    }

    #[test]
    fn history_steps() {
        assert_eq!(history_target(1, 3, -1), Some(0));
        assert_eq!(history_target(0, 3, -1), None);
        assert_eq!(history_target(2, 3, 1), None);
        assert_eq!(history_target(1, 3, 1), Some(2));
    }
}
