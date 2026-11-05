//! `state.toml`: local-only Tab Groups, pinned Tabs, and Placeholder Tab data.
//! The Remote Browser never sees any of it.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A flat Tab Group. May hold Tabs from several Connections. `State::groups`
/// order is the Sidebar order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupEntry {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub collapsed: bool,
}

/// The stored record of one Tab; doubles as its Placeholder Tab data while its
/// Connection is disconnected. `State::tabs` order is the Sidebar order
/// (within the pinned section, a Group, or the loose list).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabEntry {
    /// Name of the Connection in `config.toml`.
    pub connection: String,
    /// Last-known remote target id (changes if the Remote Browser restarts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    /// Last-known title.
    #[serde(default)]
    pub title: String,
    /// Last-known URL.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub pinned: bool,
    /// Id of the Tab Group this Tab belongs to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// Unknown fields are ignored on load for forward compatibility.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default, rename = "group")]
    pub groups: Vec<GroupEntry>,
    #[serde(default, rename = "tab")]
    pub tabs: Vec<TabEntry>,
}

#[derive(Debug)]
pub enum StateError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        message: String,
    },
    Serialize(String),
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            StateError::Parse { path, message } => {
                write!(f, "invalid state file {}: {message}", path.display())
            }
            StateError::Serialize(m) => write!(f, "cannot serialize state: {m}"),
        }
    }
}

impl std::error::Error for StateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StateError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Default state path: `~/.config/rb/state.toml`.
pub fn default_state_path() -> Option<PathBuf> {
    crate::default_dir().map(|d| d.join("state.toml"))
}

impl State {
    /// Load state. A missing file gives an empty state.
    pub fn load(path: &Path) -> Result<State, StateError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| StateError::Parse {
                path: path.to_path_buf(),
                message: e.to_string(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(source) => Err(StateError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Save atomically: write a temp file in the same directory, fsync, then
    /// rename over `path`.
    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        let io = |source| StateError::Io {
            path: path.to_path_buf(),
            source,
        };
        let text =
            toml::to_string_pretty(self).map_err(|e| StateError::Serialize(e.to_string()))?;
        let dir = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        std::fs::create_dir_all(dir).map_err(io)?;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        let tmp = dir.join(format!(".{file_name}.tmp.{}", std::process::id()));
        let result = (|| {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, path)
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(io(e));
        }
        Ok(())
    }

    pub fn group(&self, id: &str) -> Option<&GroupEntry> {
        self.groups.iter().find(|g| g.id == id)
    }
}

/// A Tab currently reported by a connected Connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTab {
    pub connection: String,
    pub target_id: String,
    pub url: String,
}

/// Find the stored entry for one live Tab.
///
/// Order: an unclaimed entry of the same Connection with the same target id;
/// otherwise the first unclaimed entry (in stored order) of the same
/// Connection with exactly the same URL. Duplicate URLs therefore resolve
/// deterministically to the earliest unclaimed entry. `claimed` holds indices
/// already assigned to other Tabs. Prefer [`match_tabs`] when matching many
/// Tabs so that target-id matches win over URL guesses.
pub fn match_tab(
    entries: &[TabEntry],
    claimed: &HashSet<usize>,
    connection: &str,
    target_id: &str,
    url: &str,
) -> Option<usize> {
    let free = |i: &usize| !claimed.contains(i) && entries[*i].connection == connection;
    (0..entries.len())
        .filter(free)
        .find(|&i| entries[i].target_id.as_deref() == Some(target_id))
        .or_else(|| {
            (0..entries.len())
                .filter(free)
                .find(|&i| entries[i].url == url)
        })
}

/// Match all live Tabs at once: first pass by target id for every Tab, second
/// pass by exact URL for the rest, in live order. Result is parallel to `live`.
/// Each entry is assigned to at most one live Tab.
pub fn match_tabs(entries: &[TabEntry], live: &[LiveTab]) -> Vec<Option<usize>> {
    let mut claimed = HashSet::new();
    let mut out = vec![None; live.len()];
    for (k, t) in live.iter().enumerate() {
        let hit = (0..entries.len()).find(|&i| {
            !claimed.contains(&i)
                && entries[i].connection == t.connection
                && entries[i].target_id.as_deref() == Some(t.target_id.as_str())
        });
        if let Some(i) = hit {
            claimed.insert(i);
            out[k] = Some(i);
        }
    }
    for (k, t) in live.iter().enumerate() {
        if out[k].is_some() {
            continue;
        }
        let hit = (0..entries.len()).find(|&i| {
            !claimed.contains(&i)
                && entries[i].connection == t.connection
                && entries[i].url == t.url
        });
        if let Some(i) = hit {
            claimed.insert(i);
            out[k] = Some(i);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(conn: &str, id: Option<&str>, url: &str) -> TabEntry {
        TabEntry {
            connection: conn.into(),
            target_id: id.map(Into::into),
            title: format!("title of {url}"),
            url: url.into(),
            ..Default::default()
        }
    }

    fn live(conn: &str, id: &str, url: &str) -> LiveTab {
        LiveTab {
            connection: conn.into(),
            target_id: id.into(),
            url: url.into(),
        }
    }

    #[test]
    fn match_by_target_id_first() {
        let e = vec![
            tab("a", Some("T1"), "http://x"),
            tab("a", Some("T2"), "http://y"),
        ];
        // URL says entry 0, target id says entry 1: target id wins.
        assert_eq!(
            match_tab(&e, &HashSet::new(), "a", "T2", "http://x"),
            Some(1)
        );
    }

    #[test]
    fn match_falls_back_to_exact_url() {
        let e = vec![tab("a", Some("OLD"), "http://x")];
        assert_eq!(
            match_tab(&e, &HashSet::new(), "a", "NEW", "http://x"),
            Some(0)
        );
        assert_eq!(
            match_tab(&e, &HashSet::new(), "a", "NEW", "http://x/"),
            None
        );
    }

    #[test]
    fn match_respects_connection() {
        let e = vec![tab("a", Some("T1"), "http://x")];
        assert_eq!(match_tab(&e, &HashSet::new(), "b", "T1", "http://x"), None);
    }

    #[test]
    fn duplicate_urls_are_deterministic() {
        let e = vec![tab("a", None, "http://x"), tab("a", None, "http://x")];
        let mut claimed = HashSet::new();
        assert_eq!(match_tab(&e, &claimed, "a", "N1", "http://x"), Some(0));
        claimed.insert(0);
        assert_eq!(match_tab(&e, &claimed, "a", "N2", "http://x"), Some(1));
        claimed.insert(1);
        assert_eq!(match_tab(&e, &claimed, "a", "N3", "http://x"), None);
    }

    #[test]
    fn match_tabs_target_id_beats_url_guess() {
        // Entry 0 is for T2 by id, though its URL equals T1's. T1 must not steal it.
        let e = vec![
            tab("a", Some("T2"), "http://x"),
            tab("a", Some("GONE"), "http://x"),
        ];
        let l = vec![live("a", "T1", "http://x"), live("a", "T2", "http://x")];
        assert_eq!(match_tabs(&e, &l), vec![Some(1), Some(0)]);
    }

    #[test]
    fn match_tabs_duplicate_urls_in_order_and_unique() {
        let e = vec![tab("a", None, "http://x"), tab("a", None, "http://x")];
        let l = vec![
            live("a", "N1", "http://x"),
            live("a", "N2", "http://x"),
            live("a", "N3", "http://x"),
        ];
        assert_eq!(match_tabs(&e, &l), vec![Some(0), Some(1), None]);
    }

    fn sample() -> State {
        State {
            groups: vec![
                GroupEntry {
                    id: "g1".into(),
                    name: "Work".into(),
                    color: Some("#ff0000".into()),
                    collapsed: true,
                },
                GroupEntry {
                    id: "g2".into(),
                    name: "Misc".into(),
                    color: None,
                    collapsed: false,
                },
            ],
            tabs: vec![
                TabEntry {
                    pinned: true,
                    ..tab("a", Some("T1"), "http://pin")
                },
                TabEntry {
                    group: Some("g1".into()),
                    ..tab("b", None, "http://grouped")
                },
                tab("a", Some("T3"), "http://loose"),
            ],
        }
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.toml");
        let s = sample();
        s.save(&p).unwrap();
        assert_eq!(State::load(&p).unwrap(), s);
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            State::load(&dir.path().join("none.toml")).unwrap(),
            State::default()
        );
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.toml");
        std::fs::write(
            &p,
            "future = 1\n[[group]]\nid=\"g\"\nname=\"G\"\nicon=\"x\"\n[[tab]]\nconnection=\"a\"\nurl=\"u\"\nnew_field=[1,2]\n",
        )
        .unwrap();
        let s = State::load(&p).unwrap();
        assert_eq!(s.groups[0].name, "G");
        assert_eq!(s.tabs[0].url, "u");
        assert_eq!(s.tabs[0].title, "");
    }

    #[test]
    fn corrupt_file_is_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.toml");
        std::fs::write(&p, "[[tab").unwrap();
        assert!(matches!(State::load(&p), Err(StateError::Parse { .. })));
    }

    #[test]
    fn save_is_atomic_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested").join("state.toml");
        let mut s = sample();
        s.save(&p).unwrap();
        s.groups.clear();
        s.save(&p).unwrap();
        assert_eq!(State::load(&p).unwrap(), s);
        let names: Vec<_> = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["state.toml"]);
    }

    #[test]
    fn failed_save_keeps_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.toml");
        let s = sample();
        s.save(&p).unwrap();
        // Target is a directory: rename fails; temp file is cleaned up.
        let bad = dir.path().join("adir");
        std::fs::create_dir(&bad).unwrap();
        std::fs::write(bad.join("x"), "x").unwrap();
        assert!(State::default().save(&bad).is_err());
        assert_eq!(State::load(&p).unwrap(), s);
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp.")
            })
            .count();
        assert_eq!(leftovers, 0);
    }
}
