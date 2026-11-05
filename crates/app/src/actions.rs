//! CDP commands issued by the UI on the Active Tab or the browser.
//! Run these on the tokio runtime, never on a GPUI executor.

use rb_cdp::{CdpError, Client, Session};
use serde_json::{json, Value};

use crate::model::history_target;

/// A command for the Active Tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageCmd {
    Navigate(String),
    Reload,
    Back,
    Forward,
}

pub async fn run_page(session: Session, cmd: PageCmd) -> Result<(), String> {
    let r = match cmd {
        PageCmd::Navigate(url) => navigate(&session, &url).await,
        PageCmd::Reload => session.send("Page.reload", None).await.map(|_| ()),
        PageCmd::Back => step_history(&session, -1).await,
        PageCmd::Forward => step_history(&session, 1).await,
    };
    r.map_err(|e| e.to_string())
}

async fn navigate(session: &Session, url: &str) -> Result<(), CdpError> {
    let r = session
        .send("Page.navigate", Some(json!({ "url": url })))
        .await?;
    // A failed navigation is reported in the result, not as a protocol error.
    match r.get("errorText").and_then(Value::as_str) {
        Some(e) if !e.is_empty() => Err(CdpError::Decode(format!("navigation failed: {e}"))),
        _ => Ok(()),
    }
}

async fn step_history(session: &Session, delta: i64) -> Result<(), CdpError> {
    let h = session.send("Page.getNavigationHistory", None).await?;
    let current = h.get("currentIndex").and_then(Value::as_i64).unwrap_or(0);
    let entries = h
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(n) = history_target(current, entries.len(), delta) else {
        return Ok(()); // nothing to go back/forward to
    };
    let Some(id) = entries[n].get("id").and_then(Value::as_i64) else {
        return Err(CdpError::Decode("history entry without id".into()));
    };
    session
        .send(
            "Page.navigateToHistoryEntry",
            Some(json!({ "entryId": id })),
        )
        .await
        .map(|_| ())
}

/// `Target.createTarget` (a new blank Tab). It appears in the Sidebar through
/// the Connection's target events.
pub async fn new_tab(client: &Client) -> Result<(), String> {
    client
        .send(
            "Target.createTarget",
            Some(json!({ "url": "about:blank" })),
            None,
        )
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// `Target.closeTarget`.
pub async fn close_tab(client: &Client, target_id: &str) -> Result<(), String> {
    client
        .send(
            "Target.closeTarget",
            Some(json!({ "targetId": target_id })),
            None,
        )
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}
