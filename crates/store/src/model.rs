//! Tab Group and pin model on top of [`State`]: pure operations, a derived
//! Sidebar layout, and reconciliation with a Connection's live targets.
//! No UI, no I/O.
//!
//! A Tab is addressed by its index in `State::tabs` (stored order) or, stably,
//! by its id (`TabEntry::id`, `t<N>`, never reused). Indices are valid only
//! until the next call that adds, removes or moves Tabs; the `*_id` variants
//! survive that. Every Tab created through this module gets an id.
//!
//! Sidebar order: pinned Tabs first (stored order), then Tab Groups in
//! `State::groups` order (members in stored order), then ungrouped Tabs in
//! stored order. Pinned Tabs are top level: pinning removes a Tab from its
//! Group, and moving a pinned Tab into a Group unpins it.

use crate::state::{match_tabs, GroupEntry, LiveTab, State, TabEntry};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

/// Reserved Tab Group palette. Excludes the Connection colours (blue, pink,
/// amber, teal) so a Group never looks like a Connection. Only names live
/// here; the UI maps them to colour values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupColor {
    Violet,
    Olive,
    Stone,
}

impl GroupColor {
    /// Palette in assignment order.
    pub const PALETTE: [GroupColor; 3] = [GroupColor::Violet, GroupColor::Olive, GroupColor::Stone];

    /// The serialized name, as stored in `GroupEntry::color`.
    pub fn name(self) -> &'static str {
        match self {
            GroupColor::Violet => "violet",
            GroupColor::Olive => "olive",
            GroupColor::Stone => "stone",
        }
    }

    pub fn from_name(name: &str) -> Option<GroupColor> {
        GroupColor::PALETTE.into_iter().find(|c| c.name() == name)
    }

    /// The first palette colour no Group uses; when all are used, the least
    /// used one (first in palette order on ties).
    pub fn next_unused<'a>(used: impl IntoIterator<Item = &'a GroupColor>) -> GroupColor {
        let used: Vec<GroupColor> = used.into_iter().copied().collect();
        let count = |c: &GroupColor| used.iter().filter(|u| *u == c).count();
        GroupColor::PALETTE
            .into_iter()
            .min_by_key(count)
            .unwrap_or(GroupColor::Violet)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    NoSuchGroup(String),
    NoSuchTab(usize),
    NoSuchTabId(String),
    /// Group names must not be blank.
    EmptyName,
    /// A target position is past the end.
    PositionOutOfRange {
        position: usize,
        len: usize,
    },
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::NoSuchGroup(id) => write!(f, "no Tab Group with id {id:?}"),
            ModelError::NoSuchTab(i) => write!(f, "no Tab at index {i}"),
            ModelError::NoSuchTabId(id) => write!(f, "no Tab with id {id:?}"),
            ModelError::EmptyName => write!(f, "Tab Group name must not be empty"),
            ModelError::PositionOutOfRange { position, len } => {
                write!(f, "position {position} is out of range (len {len})")
            }
        }
    }
}

impl std::error::Error for ModelError {}

/// A Sidebar section a Tab can be placed in (see [`State::place_tab`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section {
    Pinned,
    Group(String),
    Loose,
}

/// One live target as reported by a Connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInfo {
    pub target_id: String,
    pub title: String,
    pub url: String,
}

/// Outcome of [`State::reconcile`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Stored entries matched to a live target and refreshed.
    pub matched: usize,
    /// Live targets with no stored entry; added as ungrouped Tabs.
    pub new: usize,
    /// Stored entries of this Connection with no live target; kept as
    /// Placeholder Tabs.
    pub orphaned: usize,
}

/// One Tab as the Sidebar renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarTab {
    /// Stable Tab id (empty only for a Tab that never got one).
    pub id: String,
    /// Index in `State::tabs`.
    pub index: usize,
    pub connection: String,
    pub target_id: Option<String>,
    pub title: String,
    pub url: String,
    pub pinned: bool,
    /// True when the Tab's Connection is not connected (dimmed entry).
    pub placeholder: bool,
}

/// One Tab Group as the Sidebar renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarGroup {
    pub id: String,
    pub name: String,
    /// Palette name as stored; see [`GroupEntry::color`].
    pub color: Option<String>,
    pub collapsed: bool,
    /// Member Tabs. Empty when `collapsed`; see `tab_count`.
    pub tabs: Vec<SidebarTab>,
    /// Number of member Tabs, also when collapsed.
    pub tab_count: usize,
    /// Distinct Connections of the member Tabs, in first-appearance order.
    /// A collapsed Group shows these as badges.
    pub connections: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarItem {
    Tab(SidebarTab),
    Group(SidebarGroup),
}

impl State {
    fn group_pos(&self, id: &str) -> Result<usize, ModelError> {
        self.groups
            .iter()
            .position(|g| g.id == id)
            .ok_or_else(|| ModelError::NoSuchGroup(id.to_string()))
    }

    fn tab_mut(&mut self, index: usize) -> Result<&mut TabEntry, ModelError> {
        self.tabs.get_mut(index).ok_or(ModelError::NoSuchTab(index))
    }

    /// A fresh Tab id (`t<N>`): above every id in use and above the persisted
    /// counter, so the id of a closed Tab is not reused.
    pub fn alloc_tab_id(&mut self) -> String {
        let max = self
            .tabs
            .iter()
            .filter_map(|t| t.id.as_deref()?.strip_prefix('t')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        let n = self.next_tab_id.max(max + 1).max(1);
        self.next_tab_id = n + 1;
        format!("t{n}")
    }

    /// Give an id to every Tab without one (old files) and replace duplicate
    /// ids, so ids are unique. Existing ids are kept.
    pub fn ensure_tab_ids(&mut self) {
        let mut seen = HashSet::new();
        for i in 0..self.tabs.len() {
            let keep = match &self.tabs[i].id {
                Some(id) if !id.is_empty() => seen.insert(id.clone()),
                _ => false,
            };
            if !keep {
                self.tabs[i].id = None;
            }
        }
        for i in 0..self.tabs.len() {
            if self.tabs[i].id.is_none() {
                let id = self.alloc_tab_id();
                seen.insert(id.clone());
                self.tabs[i].id = Some(id);
            }
        }
        // The counter must stay above every id in use (a hand-edited file).
        let max = self
            .tabs
            .iter()
            .filter_map(|t| t.id.as_deref()?.strip_prefix('t')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        self.next_tab_id = self.next_tab_id.max(max + 1);
    }

    /// Index of the Tab with this id.
    pub fn tab_index(&self, id: &str) -> Option<usize> {
        self.tabs.iter().position(|t| t.id.as_deref() == Some(id))
    }

    fn idx(&self, id: &str) -> Result<usize, ModelError> {
        self.tab_index(id)
            .ok_or_else(|| ModelError::NoSuchTabId(id.to_string()))
    }

    pub fn pin_tab_id(&mut self, id: &str) -> Result<(), ModelError> {
        let i = self.idx(id)?;
        self.pin_tab(i)
    }

    pub fn unpin_tab_id(&mut self, id: &str) -> Result<(), ModelError> {
        let i = self.idx(id)?;
        self.unpin_tab(i)
    }

    pub fn move_tab_to_group_id(&mut self, id: &str, group: &str) -> Result<(), ModelError> {
        let i = self.idx(id)?;
        self.move_tab_to_group(i, group)
    }

    pub fn remove_tab_from_group_id(&mut self, id: &str) -> Result<(), ModelError> {
        let i = self.idx(id)?;
        self.remove_tab_from_group(i)
    }

    pub fn remove_tab_id(&mut self, id: &str) -> Result<TabEntry, ModelError> {
        let i = self.idx(id)?;
        self.remove_tab(i)
    }

    /// Put a Tab into a Sidebar section and at a position: before the Tab
    /// `before`, or last in the section when `None`. This is what a drag and
    /// drop does. `before` should belong to the target section; the stored
    /// order is global, so the position is only meaningful inside it.
    pub fn place_tab(
        &mut self,
        id: &str,
        section: &Section,
        before: Option<&str>,
    ) -> Result<(), ModelError> {
        let from = self.idx(id)?;
        if let Section::Group(g) = section {
            self.group_pos(g)?;
        }
        if let Some(b) = before {
            self.idx(b)?;
            if b == id {
                return Ok(()); // dropped on itself: nothing moves
            }
        }
        let mut t = self.tabs.remove(from);
        match section {
            Section::Pinned => {
                t.pinned = true;
                t.group = None;
            }
            Section::Group(g) => {
                t.pinned = false;
                t.group = Some(g.clone());
            }
            Section::Loose => {
                t.pinned = false;
                t.group = None;
            }
        }
        let at = before
            .and_then(|b| self.tab_index(b))
            .unwrap_or(self.tabs.len());
        self.tabs.insert(at, t);
        Ok(())
    }

    fn new_group_id(&self) -> String {
        let max = self
            .groups
            .iter()
            .filter_map(|g| g.id.strip_prefix('g')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        let mut n = max + 1;
        loop {
            let id = format!("g{n}");
            if self.group(&id).is_none() {
                return id;
            }
            n += 1;
        }
    }

    /// Append a Tab Group and return its new unique id. With `color: None`
    /// the next unused palette colour is assigned.
    pub fn create_group(
        &mut self,
        name: &str,
        color: Option<GroupColor>,
    ) -> Result<String, ModelError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ModelError::EmptyName);
        }
        let color = color.unwrap_or_else(|| {
            let used: Vec<GroupColor> = self
                .groups
                .iter()
                .filter_map(|g| GroupColor::from_name(g.color.as_deref()?))
                .collect();
            GroupColor::next_unused(&used)
        });
        let id = self.new_group_id();
        self.groups.push(GroupEntry {
            id: id.clone(),
            name: name.to_string(),
            color: Some(color.name().to_string()),
            collapsed: false,
        });
        Ok(id)
    }

    pub fn rename_group(&mut self, id: &str, name: &str) -> Result<(), ModelError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ModelError::EmptyName);
        }
        let i = self.group_pos(id)?;
        self.groups[i].name = name.to_string();
        Ok(())
    }

    pub fn set_group_color(&mut self, id: &str, color: GroupColor) -> Result<(), ModelError> {
        let i = self.group_pos(id)?;
        self.groups[i].color = Some(color.name().to_string());
        Ok(())
    }

    pub fn set_group_collapsed(&mut self, id: &str, collapsed: bool) -> Result<(), ModelError> {
        let i = self.group_pos(id)?;
        self.groups[i].collapsed = collapsed;
        Ok(())
    }

    /// Delete a Tab Group. Its Tabs become ungrouped; they are not deleted.
    pub fn delete_group(&mut self, id: &str) -> Result<GroupEntry, ModelError> {
        let i = self.group_pos(id)?;
        for t in self
            .tabs
            .iter_mut()
            .filter(|t| t.group.as_deref() == Some(id))
        {
            t.group = None;
        }
        Ok(self.groups.remove(i))
    }

    /// Move a Tab Group so it ends up at index `to` in `State::groups`.
    pub fn move_group(&mut self, id: &str, to: usize) -> Result<(), ModelError> {
        let from = self.group_pos(id)?;
        if to >= self.groups.len() {
            return Err(ModelError::PositionOutOfRange {
                position: to,
                len: self.groups.len(),
            });
        }
        let g = self.groups.remove(from);
        self.groups.insert(to, g);
        Ok(())
    }

    /// Pin a Tab. A pinned Tab is top level, so it leaves its Group.
    pub fn pin_tab(&mut self, index: usize) -> Result<(), ModelError> {
        let t = self.tab_mut(index)?;
        t.pinned = true;
        t.group = None;
        Ok(())
    }

    pub fn unpin_tab(&mut self, index: usize) -> Result<(), ModelError> {
        self.tab_mut(index)?.pinned = false;
        Ok(())
    }

    /// Put a Tab into a Tab Group (any Connection's Tab may join). A pinned
    /// Tab is unpinned.
    pub fn move_tab_to_group(&mut self, index: usize, group: &str) -> Result<(), ModelError> {
        self.group_pos(group)?;
        let t = self.tab_mut(index)?;
        t.group = Some(group.to_string());
        t.pinned = false;
        Ok(())
    }

    /// Make a Tab ungrouped. No-op if it already is.
    pub fn remove_tab_from_group(&mut self, index: usize) -> Result<(), ModelError> {
        self.tab_mut(index)?.group = None;
        Ok(())
    }

    /// Move a Tab so it ends up at index `to` in `State::tabs` (the stored
    /// order the Sidebar derives from). Returns nothing; indices of other
    /// Tabs shift.
    pub fn reorder_tab(&mut self, index: usize, to: usize) -> Result<(), ModelError> {
        if index >= self.tabs.len() {
            return Err(ModelError::NoSuchTab(index));
        }
        if to >= self.tabs.len() {
            return Err(ModelError::PositionOutOfRange {
                position: to,
                len: self.tabs.len(),
            });
        }
        let t = self.tabs.remove(index);
        self.tabs.insert(to, t);
        Ok(())
    }

    /// Record last-known Placeholder Tab data for a live Tab. Updates the
    /// entry matched by target id (then exact URL), else appends an ungrouped
    /// Tab. Returns the entry's index.
    pub fn add_or_update_tab(
        &mut self,
        connection: &str,
        target_id: &str,
        title: &str,
        url: &str,
    ) -> usize {
        match crate::state::match_tab(&self.tabs, &HashSet::new(), connection, target_id, url) {
            Some(i) => {
                let t = &mut self.tabs[i];
                t.target_id = Some(target_id.to_string());
                t.title = title.to_string();
                t.url = url.to_string();
                i
            }
            None => {
                let id = self.alloc_tab_id();
                self.tabs.push(TabEntry {
                    id: Some(id),
                    connection: connection.to_string(),
                    target_id: Some(target_id.to_string()),
                    title: title.to_string(),
                    url: url.to_string(),
                    ..Default::default()
                });
                self.tabs.len() - 1
            }
        }
    }

    /// Remove a Tab entry (the user closed it).
    pub fn remove_tab(&mut self, index: usize) -> Result<TabEntry, ModelError> {
        if index >= self.tabs.len() {
            return Err(ModelError::NoSuchTab(index));
        }
        Ok(self.tabs.remove(index))
    }

    /// Reconcile stored Tabs of `connection` with its live targets (after a
    /// connect, reconnect, or restart). Matching is by target id, then exact
    /// URL ([`match_tabs`]). Matched entries get fresh target id, title and
    /// URL and keep their Group and pin. Unmatched live targets are appended
    /// as ungrouped Tabs. Unmatched stored entries are kept untouched (they
    /// show as Placeholder Tabs); nothing is ever deleted.
    pub fn reconcile(&mut self, connection: &str, live: &[TargetInfo]) -> ReconcileReport {
        let live_tabs: Vec<LiveTab> = live
            .iter()
            .map(|t| LiveTab {
                connection: connection.to_string(),
                target_id: t.target_id.clone(),
                url: t.url.clone(),
            })
            .collect();
        let matches = match_tabs(&self.tabs, &live_tabs);
        let mut report = ReconcileReport::default();
        let mut claimed = HashSet::new();
        for (t, m) in live.iter().zip(&matches) {
            match m {
                Some(i) => {
                    let e = &mut self.tabs[*i];
                    e.target_id = Some(t.target_id.clone());
                    e.title = t.title.clone();
                    e.url = t.url.clone();
                    claimed.insert(*i);
                    report.matched += 1;
                }
                None => {
                    let id = self.alloc_tab_id();
                    self.tabs.push(TabEntry {
                        id: Some(id),
                        connection: connection.to_string(),
                        target_id: Some(t.target_id.clone()),
                        title: t.title.clone(),
                        url: t.url.clone(),
                        ..Default::default()
                    });
                    report.new += 1;
                }
            }
        }
        report.orphaned = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(i, e)| e.connection == connection && !claimed.contains(i))
            .count()
            - report.new;
        report
    }

    /// The Sidebar display structure. `is_connected(connection)` says whether
    /// a Connection is connected; Tabs of other Connections are Placeholder
    /// Tabs. A Tab pointing at a missing Group is shown ungrouped.
    pub fn sidebar_layout(&self, is_connected: &dyn Fn(&str) -> bool) -> Vec<SidebarItem> {
        let view = |index: usize, t: &TabEntry| SidebarTab {
            id: t.id.clone().unwrap_or_default(),
            index,
            connection: t.connection.clone(),
            target_id: t.target_id.clone(),
            title: t.title.clone(),
            url: t.url.clone(),
            pinned: t.pinned,
            placeholder: !is_connected(&t.connection),
        };
        let mut out = Vec::new();
        for (i, t) in self.tabs.iter().enumerate().filter(|(_, t)| t.pinned) {
            out.push(SidebarItem::Tab(view(i, t)));
        }
        let in_group =
            |t: &TabEntry, g: &GroupEntry| !t.pinned && t.group.as_deref() == Some(g.id.as_str());
        for g in &self.groups {
            let members: Vec<SidebarTab> = self
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, t)| in_group(t, g))
                .map(|(i, t)| view(i, t))
                .collect();
            let mut connections: Vec<String> = Vec::new();
            for m in &members {
                if !connections.contains(&m.connection) {
                    connections.push(m.connection.clone());
                }
            }
            out.push(SidebarItem::Group(SidebarGroup {
                id: g.id.clone(),
                name: g.name.clone(),
                color: g.color.clone(),
                collapsed: g.collapsed,
                tab_count: members.len(),
                tabs: if g.collapsed { Vec::new() } else { members },
                connections,
            }));
        }
        for (i, t) in self.tabs.iter().enumerate() {
            let grouped = t
                .group
                .as_deref()
                .is_some_and(|id| self.group(id).is_some());
            if !t.pinned && !grouped {
                out.push(SidebarItem::Tab(view(i, t)));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(conn: &str, id: &str, url: &str) -> TabEntry {
        TabEntry {
            connection: conn.into(),
            target_id: Some(id.into()),
            title: format!("t {url}"),
            url: url.into(),
            ..Default::default()
        }
    }

    fn target(id: &str, url: &str) -> TargetInfo {
        TargetInfo {
            target_id: id.into(),
            title: format!("new {url}"),
            url: url.into(),
        }
    }

    fn all_connected(_: &str) -> bool {
        true
    }

    fn state3() -> State {
        let mut s = State {
            groups: vec![],
            tabs: vec![
                tab("a", "T0", "http://0"),
                tab("b", "T1", "http://1"),
                tab("a", "T2", "http://2"),
            ],
            ..Default::default()
        };
        s.ensure_tab_ids();
        s
    }

    fn urls(items: &[SidebarItem]) -> Vec<String> {
        let mut v = Vec::new();
        for it in items {
            match it {
                SidebarItem::Tab(t) => v.push(t.url.clone()),
                SidebarItem::Group(g) => {
                    v.push(format!("[{}]", g.name));
                    v.extend(g.tabs.iter().map(|t| t.url.clone()));
                }
            }
        }
        v
    }

    #[test]
    fn colors_serde_and_assignment() {
        assert_eq!(GroupColor::from_name("violet"), Some(GroupColor::Violet));
        assert_eq!(GroupColor::from_name("blue"), None);
        for c in GroupColor::PALETTE {
            assert_eq!(GroupColor::from_name(c.name()), Some(c));
        }
        assert_eq!(GroupColor::next_unused(&[]), GroupColor::Violet);
        assert_eq!(
            GroupColor::next_unused(&[GroupColor::Violet]),
            GroupColor::Olive
        );
        assert_eq!(
            GroupColor::next_unused(&[GroupColor::Violet, GroupColor::Stone]),
            GroupColor::Olive
        );
        // All used: least used wins.
        let all = [
            GroupColor::Violet,
            GroupColor::Olive,
            GroupColor::Stone,
            GroupColor::Violet,
        ];
        assert_eq!(GroupColor::next_unused(&all), GroupColor::Olive);
    }

    #[test]
    fn create_group_ids_unique_and_colors_rotate() {
        let mut s = State::default();
        let a = s.create_group("A", None).unwrap();
        let b = s.create_group("  B ", None).unwrap();
        let c = s.create_group("C", Some(GroupColor::Violet)).unwrap();
        assert!(a != b && b != c && a != c);
        assert_eq!(s.group(&a).unwrap().color.as_deref(), Some("violet"));
        assert_eq!(s.group(&b).unwrap().color.as_deref(), Some("olive"));
        assert_eq!(s.group(&b).unwrap().name, "B");
        // Ids are not reused after deleting the newest.
        s.delete_group(&c).unwrap();
        let d = s.create_group("D", None).unwrap();
        assert_ne!(d, a);
        assert_ne!(d, b);
        assert_eq!(s.create_group("   ", None), Err(ModelError::EmptyName));
    }

    #[test]
    fn group_id_skips_existing_and_ignores_odd_ids() {
        let mut s = State::default();
        s.groups.push(GroupEntry {
            id: "g7".into(),
            name: "x".into(),
            ..Default::default()
        });
        s.groups.push(GroupEntry {
            id: "custom".into(),
            name: "y".into(),
            ..Default::default()
        });
        assert_eq!(s.create_group("n", None).unwrap(), "g8");
    }

    #[test]
    fn rename_color_collapse_and_errors() {
        let mut s = State::default();
        let g = s.create_group("A", None).unwrap();
        s.rename_group(&g, "Renamed").unwrap();
        s.set_group_color(&g, GroupColor::Stone).unwrap();
        s.set_group_collapsed(&g, true).unwrap();
        let e = s.group(&g).unwrap();
        assert_eq!(
            (e.name.as_str(), e.color.as_deref(), e.collapsed),
            ("Renamed", Some("stone"), true)
        );
        assert_eq!(s.rename_group(&g, " "), Err(ModelError::EmptyName));
        assert_eq!(
            s.rename_group("nope", "x"),
            Err(ModelError::NoSuchGroup("nope".into()))
        );
        assert!(s.set_group_color("nope", GroupColor::Olive).is_err());
        assert!(s.set_group_collapsed("nope", true).is_err());
        assert!(s.delete_group("nope").is_err());
        assert!(s.move_group("nope", 0).is_err());
        assert_eq!(s.group(&g).unwrap().name, "Renamed");
    }

    #[test]
    fn delete_group_ungroups_tabs_without_deleting() {
        let mut s = state3();
        let g = s.create_group("G", None).unwrap();
        let h = s.create_group("H", None).unwrap();
        s.move_tab_to_group(0, &g).unwrap();
        s.move_tab_to_group(1, &g).unwrap();
        s.move_tab_to_group(2, &h).unwrap();
        let removed = s.delete_group(&g).unwrap();
        assert_eq!(removed.name, "G");
        assert_eq!(s.tabs.len(), 3);
        assert_eq!(s.tabs[0].group, None);
        assert_eq!(s.tabs[1].group, None);
        assert_eq!(s.tabs[2].group.as_deref(), Some(h.as_str()));
    }

    #[test]
    fn move_group_reorders() {
        let mut s = State::default();
        let a = s.create_group("A", None).unwrap();
        let b = s.create_group("B", None).unwrap();
        let c = s.create_group("C", None).unwrap();
        s.move_group(&c, 0).unwrap();
        let ids: Vec<_> = s.groups.iter().map(|g| g.id.clone()).collect();
        assert_eq!(ids, vec![c.clone(), a.clone(), b.clone()]);
        s.move_group(&c, 2).unwrap();
        assert_eq!(s.groups.last().unwrap().id, c);
        assert_eq!(
            s.move_group(&a, 3),
            Err(ModelError::PositionOutOfRange {
                position: 3,
                len: 3
            })
        );
    }

    #[test]
    fn pin_unpin_group_interplay_and_errors() {
        let mut s = state3();
        let g = s.create_group("G", None).unwrap();
        s.move_tab_to_group(1, &g).unwrap();
        s.pin_tab(1).unwrap();
        assert!(s.tabs[1].pinned);
        assert_eq!(s.tabs[1].group, None, "pinned Tabs are top level");
        s.move_tab_to_group(1, &g).unwrap();
        assert!(!s.tabs[1].pinned);
        s.remove_tab_from_group(1).unwrap();
        s.remove_tab_from_group(1).unwrap();
        s.unpin_tab(1).unwrap();
        assert_eq!(s.pin_tab(9), Err(ModelError::NoSuchTab(9)));
        assert_eq!(s.unpin_tab(9), Err(ModelError::NoSuchTab(9)));
        assert_eq!(s.move_tab_to_group(9, &g), Err(ModelError::NoSuchTab(9)));
        assert_eq!(
            s.move_tab_to_group(0, "zzz"),
            Err(ModelError::NoSuchGroup("zzz".into()))
        );
        assert_eq!(s.tabs[0].group, None);
        assert_eq!(s.remove_tab_from_group(9), Err(ModelError::NoSuchTab(9)));
    }

    #[test]
    fn reorder_tab_and_errors() {
        let mut s = state3();
        s.reorder_tab(2, 0).unwrap();
        let u: Vec<_> = s.tabs.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(u, vec!["http://2", "http://0", "http://1"]);
        s.reorder_tab(0, 2).unwrap();
        assert_eq!(s.tabs[2].url, "http://2");
        assert_eq!(s.reorder_tab(5, 0), Err(ModelError::NoSuchTab(5)));
        assert_eq!(
            s.reorder_tab(0, 3),
            Err(ModelError::PositionOutOfRange {
                position: 3,
                len: 3
            })
        );
        assert_eq!(s, {
            let mut x = state3();
            x.reorder_tab(2, 0).unwrap();
            x.reorder_tab(0, 2).unwrap();
            x
        });
    }

    #[test]
    fn add_or_update_and_remove_tab() {
        let mut s = State::default();
        let i = s.add_or_update_tab("a", "T1", "One", "http://1");
        assert_eq!(i, 0);
        s.pin_tab(i).unwrap();
        // Same target id: update in place, pin kept.
        assert_eq!(s.add_or_update_tab("a", "T1", "One!", "http://1/next"), 0);
        assert_eq!(s.tabs.len(), 1);
        assert_eq!(
            (s.tabs[0].title.as_str(), s.tabs[0].url.as_str()),
            ("One!", "http://1/next")
        );
        assert!(s.tabs[0].pinned);
        // Same URL, new target id (restart): still the same entry.
        assert_eq!(s.add_or_update_tab("a", "T9", "One!", "http://1/next"), 0);
        assert_eq!(s.tabs[0].target_id.as_deref(), Some("T9"));
        // Other Connection: new entry.
        assert_eq!(s.add_or_update_tab("b", "T9", "x", "http://1/next"), 1);
        assert_eq!(s.remove_tab(0).unwrap().connection, "a");
        assert_eq!(s.tabs.len(), 1);
        assert_eq!(s.remove_tab(4).unwrap_err(), ModelError::NoSuchTab(4));
    }

    #[test]
    fn layout_pinned_first_then_groups_then_loose() {
        let mut s = state3();
        s.tabs.push(tab("a", "T3", "http://3"));
        let g = s.create_group("G", None).unwrap();
        s.move_tab_to_group(1, &g).unwrap();
        s.pin_tab(2).unwrap();
        let l = s.sidebar_layout(&all_connected);
        assert_eq!(
            urls(&l),
            vec!["http://2", "[G]", "http://1", "http://0", "http://3"]
        );
        match &l[0] {
            SidebarItem::Tab(t) => assert!(t.pinned && t.index == 2),
            _ => panic!(),
        }
    }

    #[test]
    fn layout_pinned_tab_with_stale_group_is_only_in_pinned_section() {
        let mut s = state3();
        let g = s.create_group("G", None).unwrap();
        // Hand-edited file: pinned and grouped at once.
        s.tabs[0].pinned = true;
        s.tabs[0].group = Some(g);
        let l = s.sidebar_layout(&all_connected);
        assert_eq!(urls(&l), vec!["http://0", "[G]", "http://1", "http://2"]);
    }

    #[test]
    fn layout_dangling_group_ref_is_ungrouped() {
        let mut s = state3();
        s.tabs[1].group = Some("gone".into());
        assert_eq!(
            urls(&s.sidebar_layout(&all_connected)),
            vec!["http://0", "http://1", "http://2"]
        );
    }

    #[test]
    fn collapsed_group_hides_tabs_but_lists_distinct_connections() {
        let mut s = state3();
        let g = s.create_group("G", None).unwrap();
        for i in 0..3 {
            s.move_tab_to_group(i, &g).unwrap();
        }
        s.set_group_collapsed(&g, true).unwrap();
        let l = s.sidebar_layout(&all_connected);
        let SidebarItem::Group(gr) = &l[0] else {
            panic!()
        };
        assert!(gr.tabs.is_empty());
        assert_eq!(gr.tab_count, 3);
        assert_eq!(gr.connections, vec!["a".to_string(), "b".to_string()]);
        s.set_group_collapsed(&g, false).unwrap();
        let l = s.sidebar_layout(&all_connected);
        let SidebarItem::Group(gr) = &l[0] else {
            panic!()
        };
        assert_eq!(gr.tabs.len(), 3, "group spans two Connections");
        assert_eq!(gr.connections.len(), 2);
    }

    #[test]
    fn empty_group_is_listed() {
        let mut s = State::default();
        s.create_group("Empty", None).unwrap();
        let l = s.sidebar_layout(&all_connected);
        let SidebarItem::Group(g) = &l[0] else {
            panic!()
        };
        assert_eq!((g.tab_count, g.connections.len()), (0, 0));
    }

    #[test]
    fn placeholder_marking_follows_connection_state() {
        let s = state3();
        let l = s.sidebar_layout(&|c| c == "a");
        let flags: Vec<(String, bool)> = l
            .iter()
            .map(|i| match i {
                SidebarItem::Tab(t) => (t.connection.clone(), t.placeholder),
                _ => panic!(),
            })
            .collect();
        assert_eq!(
            flags,
            vec![("a".into(), false), ("b".into(), true), ("a".into(), false)]
        );
        assert!(s
            .sidebar_layout(&|_| false)
            .iter()
            .all(|i| matches!(i, SidebarItem::Tab(t) if t.placeholder)));
    }

    #[test]
    fn reconcile_restart_changes_target_ids() {
        let mut s = State::default();
        s.add_or_update_tab("a", "OLD1", "t", "http://1");
        s.add_or_update_tab("a", "OLD2", "t", "http://2");
        let g = s.create_group("G", None).unwrap();
        s.move_tab_to_group(0, &g).unwrap();
        s.pin_tab(1).unwrap();
        let r = s.reconcile("a", &[target("N2", "http://2"), target("N1", "http://1")]);
        assert_eq!(
            r,
            ReconcileReport {
                matched: 2,
                new: 0,
                orphaned: 0
            }
        );
        assert_eq!(s.tabs.len(), 2);
        assert_eq!(s.tabs[0].target_id.as_deref(), Some("N1"));
        assert_eq!(s.tabs[0].group.as_deref(), Some(g.as_str()));
        assert_eq!(s.tabs[0].title, "new http://1");
        assert_eq!(s.tabs[1].target_id.as_deref(), Some("N2"));
        assert!(s.tabs[1].pinned);
    }

    #[test]
    fn reconcile_target_id_match_follows_navigation() {
        let mut s = State::default();
        s.add_or_update_tab("a", "T1", "t", "http://old");
        let r = s.reconcile("a", &[target("T1", "http://new")]);
        assert_eq!(
            r,
            ReconcileReport {
                matched: 1,
                new: 0,
                orphaned: 0
            }
        );
        assert_eq!(s.tabs[0].url, "http://new");
    }

    #[test]
    fn reconcile_duplicate_urls() {
        let mut s = State::default();
        s.tabs.push(TabEntry {
            connection: "a".into(),
            url: "http://x".into(),
            pinned: true,
            ..Default::default()
        });
        s.tabs.push(TabEntry {
            connection: "a".into(),
            url: "http://x".into(),
            ..Default::default()
        });
        let r = s.reconcile(
            "a",
            &[
                target("N1", "http://x"),
                target("N2", "http://x"),
                target("N3", "http://x"),
            ],
        );
        assert_eq!(
            r,
            ReconcileReport {
                matched: 2,
                new: 1,
                orphaned: 0
            }
        );
        assert_eq!(s.tabs[0].target_id.as_deref(), Some("N1"));
        assert!(s.tabs[0].pinned);
        assert_eq!(s.tabs[1].target_id.as_deref(), Some("N2"));
        assert_eq!(s.tabs[2].target_id.as_deref(), Some("N3"));
        assert!(!s.tabs[2].pinned && s.tabs[2].group.is_none());
    }

    #[test]
    fn reconcile_closed_remotely_stays_placeholder() {
        let mut s = State::default();
        s.add_or_update_tab("a", "T1", "Keep", "http://keep");
        s.add_or_update_tab("a", "T2", "Gone", "http://gone");
        let g = s.create_group("G", None).unwrap();
        s.move_tab_to_group(1, &g).unwrap();
        let r = s.reconcile("a", &[target("T1", "http://keep")]);
        assert_eq!(
            r,
            ReconcileReport {
                matched: 1,
                new: 0,
                orphaned: 1
            }
        );
        assert_eq!(s.tabs.len(), 2);
        assert_eq!(s.tabs[1].title, "Gone");
        assert_eq!(s.tabs[1].group.as_deref(), Some(g.as_str()));
        assert_eq!(s.tabs[1].target_id.as_deref(), Some("T2"));
    }

    #[test]
    fn reconcile_new_tabs_and_other_connections_untouched() {
        let mut s = State::default();
        s.add_or_update_tab("b", "T1", "B", "http://b");
        let r = s.reconcile("a", &[target("T1", "http://b")]);
        assert_eq!(
            r,
            ReconcileReport {
                matched: 0,
                new: 1,
                orphaned: 0
            }
        );
        assert_eq!(s.tabs.len(), 2);
        assert_eq!(s.tabs[0].connection, "b");
        assert_eq!(s.tabs[0].title, "B");
        assert_eq!(s.tabs[1].connection, "a");
        // Empty live list: everything of "a" is orphaned, nothing deleted.
        let r = s.reconcile("a", &[]);
        assert_eq!(
            r,
            ReconcileReport {
                matched: 0,
                new: 0,
                orphaned: 1
            }
        );
        assert_eq!(s.tabs.len(), 2);
    }

    #[test]
    fn save_load_round_trip_of_model_state() {
        let mut s = state3();
        let g = s.create_group("G", None).unwrap();
        s.move_tab_to_group(0, &g).unwrap();
        s.set_group_collapsed(&g, true).unwrap();
        s.pin_tab(2).unwrap();
        s.reorder_tab(2, 0).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.toml");
        s.save(&p).unwrap();
        let loaded = State::load(&p).unwrap();
        assert_eq!(loaded, s);
        assert_eq!(
            loaded.sidebar_layout(&all_connected),
            s.sidebar_layout(&all_connected)
        );
    }

    #[test]
    fn ids_are_assigned_unique_and_never_reused() {
        let mut s = state3();
        let ids: Vec<_> = s.tabs.iter().map(|t| t.id.clone().unwrap()).collect();
        assert_eq!(ids, ["t1", "t2", "t3"]);
        s.remove_tab_id("t3").unwrap();
        let i = s.add_or_update_tab("a", "NEW", "n", "http://new");
        assert_eq!(s.tabs[i].id.as_deref(), Some("t4"));
        let rep = s.reconcile("a", &[target("Z", "http://z")]);
        assert_eq!(rep.new, 1);
        assert_eq!(s.tabs.last().unwrap().id.as_deref(), Some("t5"));
        // Duplicate and blank ids are repaired, valid ones kept.
        s.tabs[0].id = Some("t5".into());
        s.tabs[1].id = Some(String::new());
        s.ensure_tab_ids();
        let ids: Vec<_> = s.tabs.iter().map(|t| t.id.clone().unwrap()).collect();
        let mut dedup = ids.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(dedup.len(), ids.len());
        assert_eq!(ids[0], "t5");
    }

    #[test]
    fn operations_by_id_survive_reordering() {
        let mut s = state3();
        let g = s.create_group("G", None).unwrap();
        s.reorder_tab(0, 2).unwrap(); // t1 is now last
        s.move_tab_to_group_id("t1", &g).unwrap();
        assert_eq!(s.tabs[2].group.as_deref(), Some(g.as_str()));
        s.pin_tab_id("t1").unwrap();
        assert!(s.tabs[2].pinned && s.tabs[2].group.is_none());
        s.unpin_tab_id("t1").unwrap();
        s.remove_tab_from_group_id("t2").unwrap();
        assert_eq!(s.remove_tab_id("t1").unwrap().url, "http://0");
        assert_eq!(
            s.pin_tab_id("t1"),
            Err(ModelError::NoSuchTabId("t1".into()))
        );
        assert_eq!(s.tab_index("t3"), Some(1));
    }

    #[test]
    fn place_tab_sets_section_and_position() {
        let mut s = state3(); // t1 t2 t3
        let g = s.create_group("G", None).unwrap();
        s.place_tab("t3", &Section::Group(g.clone()), None).unwrap();
        assert_eq!(s.tabs[2].group.as_deref(), Some(g.as_str()));
        s.place_tab("t1", &Section::Group(g.clone()), Some("t3"))
            .unwrap();
        let order: Vec<_> = s.tabs.iter().map(|t| t.id.clone().unwrap()).collect();
        assert_eq!(order, ["t2", "t1", "t3"]);
        assert_eq!(s.tabs[1].group.as_deref(), Some(g.as_str()));
        s.place_tab("t3", &Section::Pinned, Some("t2")).unwrap();
        assert!(s.tabs[0].pinned && s.tabs[0].group.is_none());
        assert_eq!(s.tabs[0].id.as_deref(), Some("t3"));
        s.place_tab("t3", &Section::Loose, None).unwrap();
        assert!(!s.tabs[2].pinned);
        assert_eq!(s.tabs[2].id.as_deref(), Some("t3"));
        // Dropping on itself moves nothing; bad ids and Groups are errors.
        let before = s.clone();
        s.place_tab("t1", &Section::Loose, Some("t1")).unwrap();
        assert_eq!(s, before);
        assert!(s.place_tab("zz", &Section::Loose, None).is_err());
        assert!(s
            .place_tab("t1", &Section::Group("nope".into()), None)
            .is_err());
        assert!(s.place_tab("t1", &Section::Loose, Some("zz")).is_err());
        assert_eq!(s, before);
    }

    #[test]
    fn layout_exposes_ids() {
        let s = state3();
        let ids: Vec<String> = s
            .sidebar_layout(&all_connected)
            .into_iter()
            .filter_map(|i| match i {
                SidebarItem::Tab(t) => Some(t.id),
                _ => None,
            })
            .collect();
        assert_eq!(ids, ["t1", "t2", "t3"]);
    }

    #[test]
    fn old_file_without_ids_gets_ids_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.toml");
        std::fs::write(&p, PREVIOUS_VERSION).unwrap();
        let s = State::load(&p).unwrap();
        let ids: Vec<_> = s.tabs.iter().map(|t| t.id.clone().unwrap()).collect();
        assert_eq!(ids, ["t1", "t2", "t3"]);
        // Saved and reloaded: the ids are stable.
        s.save(&p).unwrap();
        assert_eq!(State::load(&p).unwrap(), s);
        assert_eq!(s.next_tab_id, 4);
    }

    /// A `state.toml` as written before the model layer: free-form colour,
    /// no pinned/group keys on some Tabs, an unknown field.
    const PREVIOUS_VERSION: &str = r##"
[[group]]
id = "g1"
name = "Work"
color = "#ff0000"
collapsed = true

[[group]]
id = "g2"
name = "Misc"

[[tab]]
connection = "a"
target_id = "T1"
title = "Pinned"
url = "http://pin"
pinned = true

[[tab]]
connection = "b"
title = "Grouped"
url = "http://grouped"
group = "g1"
extra = "ignored"

[[tab]]
connection = "a"
url = "http://loose"
"##;

    #[test]
    fn loads_state_written_by_previous_version() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.toml");
        std::fs::write(&p, PREVIOUS_VERSION).unwrap();
        let mut s = State::load(&p).unwrap();
        assert_eq!(s.groups.len(), 2);
        assert_eq!(s.groups[0].color.as_deref(), Some("#ff0000"));
        assert_eq!(
            GroupColor::from_name(s.groups[0].color.as_deref().unwrap()),
            None
        );
        assert_eq!(
            urls(&s.sidebar_layout(&|c| c == "a")),
            vec!["http://pin", "[Work]", "[Misc]", "http://loose"]
        );
        // Operations work on old data; new Groups pick the first palette colour.
        let n = s.create_group("New", None).unwrap();
        assert_eq!(n, "g3");
        assert_eq!(s.group(&n).unwrap().color.as_deref(), Some("violet"));
        s.reconcile("b", &[target("X", "http://grouped")]);
        assert_eq!(s.tabs[1].group.as_deref(), Some("g1"));
        assert_eq!(s.tabs[1].target_id.as_deref(), Some("X"));
    }
}
