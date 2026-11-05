use serde::Deserialize;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::{CdpError, Client, Event};

/// `Target.TargetInfo` (the fields rb uses).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInfo {
    pub target_id: String,
    /// `"page"`, `"iframe"`, `"service_worker"`, ...
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub attached: bool,
    #[serde(default)]
    pub opener_id: Option<String>,
}

impl TargetInfo {
    pub fn is_page(&self) -> bool {
        self.kind == "page"
    }
}

/// Typed `Target.*` events.
#[derive(Debug, Clone, PartialEq)]
pub enum TargetEvent {
    Created(TargetInfo),
    InfoChanged(TargetInfo),
    Destroyed {
        target_id: String,
    },
    Attached {
        session_id: String,
        target_info: TargetInfo,
    },
    Detached {
        session_id: String,
        target_id: Option<String>,
    },
}

impl TargetEvent {
    /// Decode a raw [`Event`]; `None` if it is not a (well-formed) Target event.
    pub fn from_event(ev: &Event) -> Option<TargetEvent> {
        let p = &ev.params;
        let info = || serde_json::from_value::<TargetInfo>(p.get("targetInfo")?.clone()).ok();
        let s = |k: &str| p.get(k).and_then(|v| v.as_str()).map(str::to_string);
        match ev.method.as_str() {
            "Target.targetCreated" => Some(TargetEvent::Created(info()?)),
            "Target.targetInfoChanged" => Some(TargetEvent::InfoChanged(info()?)),
            "Target.targetDestroyed" => Some(TargetEvent::Destroyed {
                target_id: s("targetId")?,
            }),
            "Target.attachedToTarget" => Some(TargetEvent::Attached {
                session_id: s("sessionId")?,
                target_info: info()?,
            }),
            "Target.detachedFromTarget" => Some(TargetEvent::Detached {
                session_id: s("sessionId")?,
                target_id: s("targetId"),
            }),
            _ => None,
        }
    }
}

/// Pure state: the current page targets in creation order.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TargetList {
    pages: Vec<TargetInfo>,
}

impl TargetList {
    pub fn pages(&self) -> &[TargetInfo] {
        &self.pages
    }

    /// Apply an event; returns true if the page list changed.
    pub fn apply(&mut self, ev: &TargetEvent) -> bool {
        match ev {
            TargetEvent::Created(i) | TargetEvent::InfoChanged(i) => {
                let pos = self.pages.iter().position(|p| p.target_id == i.target_id);
                match (pos, i.is_page()) {
                    (Some(n), true) if self.pages[n] != *i => {
                        self.pages[n] = i.clone();
                        true
                    }
                    (None, true) => {
                        self.pages.push(i.clone());
                        true
                    }
                    (Some(n), false) => {
                        self.pages.remove(n);
                        true
                    }
                    _ => false,
                }
            }
            TargetEvent::Destroyed { target_id } => {
                let before = self.pages.len();
                self.pages.retain(|p| &p.target_id != target_id);
                before != self.pages.len()
            }
            TargetEvent::Attached { target_info, .. } => {
                self.apply(&TargetEvent::InfoChanged(target_info.clone()))
            }
            TargetEvent::Detached { .. } => false,
        }
    }

    /// Replace everything from a `Target.getTargets` result.
    pub fn replace(&mut self, infos: Vec<TargetInfo>) {
        self.pages = infos.into_iter().filter(TargetInfo::is_page).collect();
    }
}

/// Keeps the list of page targets (the Tabs of a Connection) current by
/// following `Target.*` events on a background task. Dropping it stops the
/// task.
pub struct TargetTracker {
    rx: watch::Receiver<Vec<TargetInfo>>,
    task: JoinHandle<()>,
}

impl TargetTracker {
    /// Subscribe, enable `Target.setDiscoverTargets` (the browser replays
    /// existing targets as `targetCreated`), and start tracking.
    pub async fn start(client: &Client) -> Result<TargetTracker, CdpError> {
        let mut events = client.events();
        let (tx, rx) = watch::channel(Vec::new());
        let task = tokio::spawn(async move {
            let mut list = TargetList::default();
            while let Some(ev) = events.recv().await {
                if ev.session_id.is_some() {
                    continue;
                }
                if let Some(te) = TargetEvent::from_event(&ev) {
                    if list.apply(&te) {
                        let _ = tx.send(list.pages().to_vec());
                    }
                }
            }
        });
        let t = TargetTracker { rx, task };
        client.set_discover_targets(true).await?;
        Ok(t)
    }

    /// Snapshot of the current page targets.
    pub fn pages(&self) -> Vec<TargetInfo> {
        self.rx.borrow().clone()
    }

    /// A receiver that is notified whenever the page list changes.
    pub fn watch(&self) -> watch::Receiver<Vec<TargetInfo>> {
        self.rx.clone()
    }
}

impl Drop for TargetTracker {
    fn drop(&mut self) {
        self.task.abort();
    }
}
