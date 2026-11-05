//! Pure Sidebar logic: Connection styles, drag-and-drop targets, context menu
//! contents, Connection chip status, keyboard shortcut table, and a tiny
//! line editor. No GPUI, no I/O; everything here is unit-tested.

use std::collections::HashSet;

use rb_cdp::ConnectionState;
use rb_store::{GroupColor, Section, SidebarItem, SidebarTab, State};

use crate::theme::{self, Rgba32, Shape};

// ---- Connection identity --------------------------------------------------

/// How one Connection is drawn: colour, shape, initial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnStyle {
    pub name: String,
    pub initial: char,
    pub shape: Shape,
    /// Colour from config `color`; `None` uses the automatic palette.
    pub custom: Option<Rgba32>,
    /// Index in the config order: picks the shape and the automatic colour.
    pub index: usize,
}

impl ConnStyle {
    pub fn new(name: &str, color: Option<&str>, index: usize) -> ConnStyle {
        ConnStyle {
            name: name.to_string(),
            initial: name
                .chars()
                .find(|c| c.is_alphanumeric())
                .map(|c| c.to_uppercase().next().unwrap_or(c))
                .unwrap_or('?'),
            shape: Shape::ALL[index % Shape::ALL.len()],
            custom: color.and_then(theme::parse_hex),
            index,
        }
    }

    pub fn color(&self, dark: bool) -> Rgba32 {
        self.custom.unwrap_or_else(|| {
            let (l, d) = theme::CONNECTION_PALETTE[self.index % theme::CONNECTION_PALETTE.len()];
            if dark {
                d
            } else {
                l
            }
        })
    }
}

/// The style of `name`, or a neutral one for a Connection no longer in the
/// list (an orphaned Tab).
pub fn style_of<'a>(styles: &'a [ConnStyle], name: &str) -> Option<&'a ConnStyle> {
    styles.iter().find(|s| s.name == name)
}

/// Two letters for a Tab's favicon stand-in: the first letters of the first
/// two words, or the first two letters of one word.
pub fn fav_text(title: &str, url: &str) -> String {
    let source = if title.trim().is_empty() { url } else { title };
    let source = source
        .strip_prefix("https://")
        .or_else(|| source.strip_prefix("http://"))
        .unwrap_or(source);
    let words: Vec<&str> = source
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let mut out = String::new();
    match words.as_slice() {
        [] => out.push('·'),
        [one] => out.extend(one.chars().take(2)),
        [a, b, ..] => {
            out.extend(a.chars().take(1));
            out.extend(b.chars().take(1));
        }
    }
    let mut cs = out.chars();
    match cs.next() {
        Some(f) => f.to_uppercase().chain(cs).collect(),
        None => out,
    }
}

// ---- layout helpers ---------------------------------------------------------

/// Ids of the Tabs in visible order: pinned, then each expanded Group's Tabs,
/// then ungrouped Tabs. Cmd+1..9 selects by this order.
pub fn flat_tabs(layout: &[SidebarItem]) -> Vec<&SidebarTab> {
    let mut out = Vec::new();
    for item in layout {
        match item {
            SidebarItem::Tab(t) => out.push(t),
            SidebarItem::Group(g) => out.extend(g.tabs.iter()),
        }
    }
    out
}

/// The Tab for Cmd+`n` (1-based, 1..=9).
pub fn tab_for_shortcut(layout: &[SidebarItem], n: usize) -> Option<&SidebarTab> {
    if n == 0 {
        return None;
    }
    flat_tabs(layout).get(n - 1).copied()
}

/// Remove the Stored entries for Tabs that were closed on the Remote Browser.
/// `gone` holds target ids that were live before and are not now. Pinned
/// entries stay (they become Placeholder-like and can be reopened). Returns
/// how many were removed.
pub fn prune_closed(state: &mut State, connection: &str, gone: &HashSet<String>) -> usize {
    let ids: Vec<String> = state
        .tabs
        .iter()
        .filter(|t| {
            t.connection == connection
                && !t.pinned
                && t.target_id.as_ref().is_some_and(|id| gone.contains(id))
        })
        .filter_map(|t| t.id.clone())
        .collect();
    for id in &ids {
        let _ = state.remove_tab_id(id);
    }
    ids.len()
}

/// Whether a pages update means Tabs were closed (not a restart that replaced
/// every id): `old` and `new` share a Tab, or `new` is empty while connected.
pub fn closed_targets(old: &[String], new: &[String], connected: bool) -> HashSet<String> {
    let old_set: HashSet<&String> = old.iter().collect();
    let new_set: HashSet<&String> = new.iter().collect();
    let shared = old_set.intersection(&new_set).next().is_some();
    if old.is_empty() || !(shared || (new.is_empty() && connected)) {
        return HashSet::new();
    }
    old.iter()
        .filter(|id| !new_set.contains(id))
        .cloned()
        .collect()
}

// ---- drag and drop ----------------------------------------------------------

/// Where a dragged Tab is hovering (what the drop indicator shows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropTarget {
    /// On a Tab row: before it, or after it.
    Tab { id: String, after: bool },
    /// On a Group header: into the Group (last).
    Group(String),
    /// Empty part of the pinned area: pin as the last pinned Tab.
    PinnedEnd,
    /// The "Tabs" label: first ungrouped Tab.
    LooseStart,
    /// Below the last row: last ungrouped Tab.
    LooseEnd,
}

/// `true` for the second half of a row (`pos` within `0..len`).
pub fn is_after(pos: f32, len: f32) -> bool {
    len > 0.0 && pos >= len / 2.0
}

/// What `place_tab` should do for a drop on `target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub section: Section,
    pub before: Option<String>,
}

fn section_of(layout: &[SidebarItem], id: &str) -> Option<(Section, Vec<String>)> {
    let pinned: Vec<String> = layout
        .iter()
        .filter_map(|i| match i {
            SidebarItem::Tab(t) if t.pinned => Some(t.id.clone()),
            _ => None,
        })
        .collect();
    if pinned.iter().any(|p| p == id) {
        return Some((Section::Pinned, pinned));
    }
    for item in layout {
        if let SidebarItem::Group(g) = item {
            if g.tabs.iter().any(|t| t.id == id) {
                return Some((
                    Section::Group(g.id.clone()),
                    g.tabs.iter().map(|t| t.id.clone()).collect(),
                ));
            }
        }
    }
    let loose: Vec<String> = layout
        .iter()
        .filter_map(|i| match i {
            SidebarItem::Tab(t) if !t.pinned => Some(t.id.clone()),
            _ => None,
        })
        .collect();
    loose
        .iter()
        .any(|p| p == id)
        .then_some((Section::Loose, loose))
}

/// Translate a drop target into a section and a `before` Tab, using the
/// current layout. `dragged` is skipped when looking for the neighbour, so
/// dropping "after" the Tab right before the dragged one is a no-op.
pub fn resolve_drop(layout: &[SidebarItem], dragged: &str, target: &DropTarget) -> Option<Place> {
    match target {
        DropTarget::Tab { id, after } => {
            let (section, members) = section_of(layout, id)?;
            let before = if *after {
                let pos = members.iter().position(|m| m == id)?;
                members[pos + 1..].iter().find(|m| *m != dragged).cloned()
            } else {
                Some(id.clone())
            };
            Some(Place { section, before })
        }
        DropTarget::Group(g) => Some(Place {
            section: Section::Group(g.clone()),
            before: None,
        }),
        DropTarget::PinnedEnd => Some(Place {
            section: Section::Pinned,
            before: None,
        }),
        DropTarget::LooseStart => {
            let first = layout.iter().find_map(|i| match i {
                SidebarItem::Tab(t) if !t.pinned && t.id != dragged => Some(t.id.clone()),
                _ => None,
            });
            Some(Place {
                section: Section::Loose,
                before: first,
            })
        }
        DropTarget::LooseEnd => Some(Place {
            section: Section::Loose,
            before: None,
        }),
    }
}

/// Apply a drop to the state. Returns `true` if something changed.
pub fn apply_drop(
    state: &mut State,
    layout: &[SidebarItem],
    dragged: &str,
    t: &DropTarget,
) -> bool {
    let Some(place) = resolve_drop(layout, dragged, t) else {
        return false;
    };
    let before = state.clone();
    if state
        .place_tab(dragged, &place.section, place.before.as_deref())
        .is_err()
    {
        return false;
    }
    *state != before
}

// ---- context menu -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuKind {
    Tab(String),
    Group(String),
    Sidebar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    Pin(String),
    Unpin(String),
    MoveToGroup { tab: String, group: String },
    NewGroupWith(String),
    RemoveFromGroup(String),
    CloseTab(String),
    RenameGroup(String),
    ColorGroup { group: String, color: GroupColor },
    ToggleGroup(String),
    DeleteGroup(String),
    NewGroup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    pub action: MenuAction,
    /// A colour swatch shown before the label (Group colours).
    pub swatch: Option<GroupColor>,
    pub danger: bool,
    /// Draw a separator line above this item.
    pub separator: bool,
}

fn item(label: impl Into<String>, action: MenuAction) -> MenuItem {
    MenuItem {
        label: label.into(),
        action,
        swatch: None,
        danger: false,
        separator: false,
    }
}

/// The items for a context menu, from the current state.
pub fn menu_items(kind: &MenuKind, state: &State) -> Vec<MenuItem> {
    match kind {
        MenuKind::Sidebar => vec![item("New Group", MenuAction::NewGroup)],
        MenuKind::Tab(id) => {
            let Some(i) = state.tab_index(id) else {
                return Vec::new();
            };
            let t = &state.tabs[i];
            let mut v = Vec::new();
            if t.pinned {
                v.push(item("Unpin Tab", MenuAction::Unpin(id.clone())));
            } else {
                v.push(item("Pin Tab", MenuAction::Pin(id.clone())));
            }
            let mut first_group = true;
            for g in &state.groups {
                if t.group.as_deref() == Some(g.id.as_str()) && !t.pinned {
                    continue;
                }
                let mut m = item(
                    format!("Move to {}", g.name),
                    MenuAction::MoveToGroup {
                        tab: id.clone(),
                        group: g.id.clone(),
                    },
                );
                m.swatch = g.color.as_deref().and_then(GroupColor::from_name);
                m.separator = first_group;
                first_group = false;
                v.push(m);
            }
            let mut ng = item("New Group with Tab", MenuAction::NewGroupWith(id.clone()));
            ng.separator = first_group;
            v.push(ng);
            if t.group.is_some() {
                v.push(item(
                    "Remove from Group",
                    MenuAction::RemoveFromGroup(id.clone()),
                ));
            }
            let mut close = item("Close Tab", MenuAction::CloseTab(id.clone()));
            close.danger = true;
            close.separator = true;
            v.push(close);
            v
        }
        MenuKind::Group(id) => {
            let Some(g) = state.group(id) else {
                return Vec::new();
            };
            let mut v = vec![
                item("Rename Group", MenuAction::RenameGroup(id.clone())),
                item(
                    if g.collapsed {
                        "Expand Group"
                    } else {
                        "Collapse Group"
                    },
                    MenuAction::ToggleGroup(id.clone()),
                ),
            ];
            let current = g.color.as_deref();
            for (n, c) in GroupColor::PALETTE.into_iter().enumerate() {
                if current == Some(c.name()) {
                    continue;
                }
                let mut m = item(
                    format!("Colour: {}", c.name()),
                    MenuAction::ColorGroup {
                        group: id.clone(),
                        color: c,
                    },
                );
                m.swatch = Some(c);
                m.separator = n == 0 || (current == Some(GroupColor::PALETTE[0].name()) && n == 1);
                v.push(m);
            }
            let mut del = item("Delete Group", MenuAction::DeleteGroup(id.clone()));
            del.danger = true;
            del.separator = true;
            v.push(del);
            v
        }
    }
}

/// Apply a state-only menu action. Returns the id of a Group that should
/// enter rename mode (new Groups, Rename), if any. `CloseTab` is handled by
/// the caller (it talks to the Remote Browser).
pub fn apply_menu_action(state: &mut State, action: &MenuAction) -> Option<String> {
    match action {
        MenuAction::Pin(id) => {
            let _ = state.pin_tab_id(id);
        }
        MenuAction::Unpin(id) => {
            let _ = state.unpin_tab_id(id);
        }
        MenuAction::MoveToGroup { tab, group } => {
            let _ = state.place_tab(tab, &Section::Group(group.clone()), None);
        }
        MenuAction::NewGroupWith(tab) => {
            if let Ok(g) = state.create_group("New Group", None) {
                let _ = state.place_tab(tab, &Section::Group(g.clone()), None);
                return Some(g);
            }
        }
        MenuAction::RemoveFromGroup(id) => {
            let _ = state.remove_tab_from_group_id(id);
        }
        MenuAction::CloseTab(_) => {}
        MenuAction::RenameGroup(g) => return Some(g.clone()),
        MenuAction::ColorGroup { group, color } => {
            let _ = state.set_group_color(group, *color);
        }
        MenuAction::ToggleGroup(g) => {
            if let Some(c) = state.group(g).map(|g| g.collapsed) {
                let _ = state.set_group_collapsed(g, !c);
            }
        }
        MenuAction::DeleteGroup(g) => {
            let _ = state.delete_group(g);
        }
        MenuAction::NewGroup => {
            if let Ok(g) = state.create_group("New Group", None) {
                return Some(g);
            }
        }
    }
    None
}

// ---- Connection chips -------------------------------------------------------

/// The status shown by a chip's dot (shape and motion, not colour alone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipStatus {
    /// Round green.
    Connected,
    /// Spinning amber ring.
    Reconnecting,
    /// Red square.
    Stopped,
    /// Hollow grey ring.
    NotConnected,
    /// Red ring: authentication failed or the Manager cannot be reached.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipAction {
    NewTab,
    Disconnect,
    RetryNow,
    Start,
    Connect,
}

impl ChipAction {
    pub fn label(self) -> &'static str {
        match self {
            ChipAction::NewTab => "New Tab",
            ChipAction::Disconnect => "Disconnect",
            ChipAction::RetryNow => "Retry now",
            ChipAction::Start => "Start",
            ChipAction::Connect => "Connect",
        }
    }

    /// Drawn as the primary (filled) button.
    pub fn primary(self) -> bool {
        matches!(self, ChipAction::Start | ChipAction::Connect)
    }
}

/// The inputs that decide a chip's status.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkState {
    /// First pre-flight / connect in progress.
    Connecting,
    /// A Connection runs with this state.
    Live(ConnectionState),
    /// The Manager says the Profile is stopped.
    ProfileStopped,
    /// Nothing runs (disconnected by the user, or a problem).
    Off,
    /// Connecting failed for a reason the user must fix (token rejected,
    /// Manager unreachable); a retry is offered.
    Failed,
}

pub fn chip_status(link: &LinkState, can_start: bool) -> ChipStatus {
    match link {
        LinkState::Connecting => ChipStatus::Reconnecting,
        LinkState::ProfileStopped => ChipStatus::Stopped,
        LinkState::Off => ChipStatus::NotConnected,
        LinkState::Failed => ChipStatus::Failed,
        LinkState::Live(s) => match s {
            ConnectionState::Connected => ChipStatus::Connected,
            ConnectionState::Connecting | ConnectionState::Reconnecting { .. } => {
                ChipStatus::Reconnecting
            }
            ConnectionState::NotRunning if can_start => ChipStatus::Stopped,
            ConnectionState::Failed { .. } => ChipStatus::Failed,
            ConnectionState::NotRunning | ConnectionState::Closed => ChipStatus::NotConnected,
        },
    }
}

/// Whether the Connection's Tabs are live (not Placeholder Tabs): connected,
/// or reconnecting (the mock keeps Tabs while a Connection reconnects).
pub fn tabs_live(link: &LinkState) -> bool {
    matches!(
        link,
        LinkState::Live(ConnectionState::Connected | ConnectionState::Reconnecting { .. })
    )
}

/// Popover content for a chip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Popover {
    pub text: String,
    pub actions: Vec<ChipAction>,
}

pub fn popover(
    status: ChipStatus,
    link: &LinkState,
    live_tabs: usize,
    placeholder_tabs: usize,
    detail: Option<&str>,
) -> Popover {
    let (text, actions) = match status {
        ChipStatus::Connected => (
            format!("Connected · {live_tabs} Tab{}", plural(live_tabs)),
            vec![ChipAction::NewTab, ChipAction::Disconnect],
        ),
        ChipStatus::Reconnecting => (
            match link {
                LinkState::Live(ConnectionState::Reconnecting { attempt, .. }) => {
                    format!("Reconnecting... attempt {attempt}. Tabs are kept.")
                }
                _ => "Connecting...".to_string(),
            },
            vec![ChipAction::RetryNow],
        ),
        ChipStatus::Stopped => (
            "Profile stopped on the Manager. Start it to connect.".to_string(),
            vec![ChipAction::Start],
        ),
        ChipStatus::Failed => (
            detail
                .map(str::to_string)
                .unwrap_or_else(|| "Could not connect.".to_string()),
            vec![ChipAction::RetryNow],
        ),
        ChipStatus::NotConnected => (
            detail.map(str::to_string).unwrap_or_else(|| {
                format!(
                    "Not connected · {placeholder_tabs} Placeholder Tab{}. Click to connect.",
                    plural(placeholder_tabs)
                )
            }),
            vec![ChipAction::Connect],
        ),
    };
    Popover { text, actions }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

// ---- keyboard shortcuts -----------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    Quit,
    ToggleSidebar,
    NewTab,
    CloseTab,
    FocusAddress,
    Back,
    Forward,
    /// 0-based position (Cmd+1 is 0).
    SelectTab(usize),
}

/// Every app shortcut. These are never forwarded to the page.
pub const KEYMAP: [(&str, Shortcut); 16] = [
    ("cmd-q", Shortcut::Quit),
    ("cmd-b", Shortcut::ToggleSidebar),
    ("cmd-t", Shortcut::NewTab),
    ("cmd-w", Shortcut::CloseTab),
    ("cmd-l", Shortcut::FocusAddress),
    ("cmd-[", Shortcut::Back),
    ("cmd-]", Shortcut::Forward),
    ("cmd-1", Shortcut::SelectTab(0)),
    ("cmd-2", Shortcut::SelectTab(1)),
    ("cmd-3", Shortcut::SelectTab(2)),
    ("cmd-4", Shortcut::SelectTab(3)),
    ("cmd-5", Shortcut::SelectTab(4)),
    ("cmd-6", Shortcut::SelectTab(5)),
    ("cmd-7", Shortcut::SelectTab(6)),
    ("cmd-8", Shortcut::SelectTab(7)),
    ("cmd-9", Shortcut::SelectTab(8)),
];

pub fn shortcut_for(keystroke: &str) -> Option<Shortcut> {
    KEYMAP
        .iter()
        .find(|(k, _)| *k == keystroke)
        .map(|(_, s)| *s)
}

// ---- line editor ------------------------------------------------------------

/// What a key did to a [`LineEdit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    Changed,
    Commit,
    Cancel,
    Ignored,
}

/// A minimal single-line editor: append, backspace, select-all-then-type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineEdit {
    pub buf: String,
    pub select_all: bool,
}

impl LineEdit {
    pub fn with_text(text: &str) -> LineEdit {
        LineEdit {
            buf: text.to_string(),
            select_all: true,
        }
    }

    /// Apply a key: `key` is the key name, `key_char` the typed text.
    pub fn key(&mut self, key: &str, key_char: Option<&str>, ctrl_or_fn: bool) -> EditOutcome {
        match key {
            "enter" => EditOutcome::Commit,
            "escape" => EditOutcome::Cancel,
            "backspace" => {
                if self.select_all {
                    self.buf.clear();
                    self.select_all = false;
                } else {
                    self.buf.pop();
                }
                EditOutcome::Changed
            }
            _ => match key_char.filter(|c| !c.is_empty() && !ctrl_or_fn) {
                Some(c) => {
                    if self.select_all {
                        self.buf.clear();
                        self.select_all = false;
                    }
                    self.buf.push_str(c);
                    EditOutcome::Changed
                }
                None => EditOutcome::Ignored,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rb_store::TargetInfo;

    fn live(id: &str, url: &str) -> TargetInfo {
        TargetInfo {
            target_id: id.into(),
            title: format!("T {url}"),
            url: url.into(),
        }
    }

    /// pinned: p1; group G: g1 g2; loose: l1 l2
    fn state() -> State {
        let mut s = State::default();
        s.reconcile(
            "c",
            &[
                live("A", "http://p1"),
                live("B", "http://g1"),
                live("C", "http://g2"),
                live("D", "http://l1"),
                live("E", "http://l2"),
            ],
        );
        let g = s.create_group("G", None).unwrap();
        s.pin_tab_id("t1").unwrap();
        s.move_tab_to_group_id("t2", &g).unwrap();
        s.move_tab_to_group_id("t3", &g).unwrap();
        s
    }

    fn layout(s: &State) -> Vec<SidebarItem> {
        s.sidebar_layout(&|_| true)
    }

    fn order(s: &State) -> Vec<String> {
        flat_tabs(&layout(s)).iter().map(|t| t.id.clone()).collect()
    }

    #[test]
    fn styles_pick_shape_colour_and_initial() {
        let a = ConnStyle::new("work", None, 0);
        let b = ConnStyle::new("personal", None, 1);
        let c = ConnStyle::new("_x", Some("#112233"), 2);
        assert_eq!(
            (a.shape, b.shape, c.shape),
            (Shape::Circle, Shape::Diamond, Shape::Triangle)
        );
        assert_eq!(ConnStyle::new("r", None, 3).shape, Shape::Square);
        assert_eq!(ConnStyle::new("r", None, 4).shape, Shape::Circle);
        assert_eq!(a.initial, 'W');
        assert_eq!(c.initial, 'X');
        assert_eq!(ConnStyle::new("", None, 0).initial, '?');
        assert_ne!(a.color(false), b.color(false));
        assert_ne!(a.color(false), a.color(true));
        assert_eq!(c.color(false), 0x112233ff);
        assert_eq!(c.color(true), 0x112233ff);
        // A bad colour falls back to the palette.
        assert_eq!(ConnStyle::new("x", Some("red"), 0).custom, None);
    }

    #[test]
    fn fav_text_cases() {
        assert_eq!(fav_text("Invoices – Acme Billing", ""), "IA");
        assert_eq!(fav_text("Hacker News", ""), "HN");
        assert_eq!(fav_text("GitHub", ""), "Gi");
        assert_eq!(fav_text("", "https://example.com/x"), "Ec");
        assert_eq!(fav_text("", ""), "·");
    }

    #[test]
    fn shortcut_numbers_follow_visible_order() {
        let s = state();
        let l = layout(&s);
        assert_eq!(order(&s), ["t1", "t2", "t3", "t4", "t5"]);
        assert_eq!(tab_for_shortcut(&l, 1).unwrap().id, "t1");
        assert_eq!(tab_for_shortcut(&l, 5).unwrap().id, "t5");
        assert!(tab_for_shortcut(&l, 6).is_none());
        assert!(tab_for_shortcut(&l, 0).is_none());
        // A collapsed Group's Tabs are not numbered.
        let mut s2 = s.clone();
        s2.set_group_collapsed("g1", true).unwrap();
        assert_eq!(order(&s2), ["t1", "t4", "t5"]);
    }

    #[test]
    fn keymap_is_complete_and_unique() {
        let mut keys: Vec<&str> = KEYMAP.iter().map(|(k, _)| *k).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), KEYMAP.len());
        assert_eq!(shortcut_for("cmd-t"), Some(Shortcut::NewTab));
        assert_eq!(shortcut_for("cmd-w"), Some(Shortcut::CloseTab));
        assert_eq!(shortcut_for("cmd-l"), Some(Shortcut::FocusAddress));
        assert_eq!(shortcut_for("cmd-["), Some(Shortcut::Back));
        assert_eq!(shortcut_for("cmd-]"), Some(Shortcut::Forward));
        for n in 1..=9usize {
            assert_eq!(
                shortcut_for(&format!("cmd-{n}")),
                Some(Shortcut::SelectTab(n - 1))
            );
        }
        assert_eq!(shortcut_for("cmd-0"), None);
        assert!(KEYMAP.iter().all(|(k, _)| k.starts_with("cmd-")));
    }

    #[test]
    fn drop_before_and_after_a_tab() {
        let mut s = state();
        let l = layout(&s);
        // t5 before t4 (loose)
        let p = resolve_drop(
            &l,
            "t5",
            &DropTarget::Tab {
                id: "t4".into(),
                after: false,
            },
        )
        .unwrap();
        assert_eq!(
            p,
            Place {
                section: Section::Loose,
                before: Some("t4".into())
            }
        );
        // after the last loose Tab: append
        let p = resolve_drop(
            &l,
            "t1",
            &DropTarget::Tab {
                id: "t5".into(),
                after: true,
            },
        )
        .unwrap();
        assert_eq!(
            p,
            Place {
                section: Section::Loose,
                before: None
            }
        );
        // after a Group member: before its successor, in that Group
        let p = resolve_drop(
            &l,
            "t4",
            &DropTarget::Tab {
                id: "t2".into(),
                after: true,
            },
        )
        .unwrap();
        assert_eq!(
            p,
            Place {
                section: Section::Group("g1".into()),
                before: Some("t3".into())
            }
        );
        // the dragged Tab is skipped as a neighbour
        let p = resolve_drop(
            &l,
            "t3",
            &DropTarget::Tab {
                id: "t2".into(),
                after: true,
            },
        )
        .unwrap();
        assert_eq!(p.before, None);
        // on a pinned Tab
        let p = resolve_drop(
            &l,
            "t4",
            &DropTarget::Tab {
                id: "t1".into(),
                after: false,
            },
        )
        .unwrap();
        assert_eq!(p.section, Section::Pinned);
        assert!(apply_drop(
            &mut s,
            &l,
            "t4",
            &DropTarget::Tab {
                id: "t1".into(),
                after: false
            }
        ));
        assert!(s.tabs[0].pinned && s.tabs[0].id.as_deref() == Some("t4"));
    }

    #[test]
    fn drop_on_group_pinned_end_and_loose_edges() {
        let mut s = state();
        let l = layout(&s);
        let p = resolve_drop(&l, "t4", &DropTarget::Group("g1".into())).unwrap();
        assert_eq!(
            p,
            Place {
                section: Section::Group("g1".into()),
                before: None
            }
        );
        assert!(apply_drop(
            &mut s,
            &l,
            "t4",
            &DropTarget::Group("g1".into())
        ));
        assert_eq!(
            s.tabs
                .iter()
                .find(|t| t.id.as_deref() == Some("t4"))
                .unwrap()
                .group
                .as_deref(),
            Some("g1")
        );

        let l = layout(&s);
        assert!(apply_drop(&mut s, &l, "t2", &DropTarget::PinnedEnd));
        assert!(
            s.tabs
                .iter()
                .find(|t| t.id.as_deref() == Some("t2"))
                .unwrap()
                .pinned
        );

        let l = layout(&s);
        let p = resolve_drop(&l, "t2", &DropTarget::LooseStart).unwrap();
        assert_eq!(
            p,
            Place {
                section: Section::Loose,
                before: Some("t5".into())
            }
        );
        assert!(apply_drop(&mut s, &l, "t2", &DropTarget::LooseStart));
        let t2 = s
            .tabs
            .iter()
            .find(|t| t.id.as_deref() == Some("t2"))
            .unwrap();
        assert!(!t2.pinned && t2.group.is_none());
        let l = layout(&s);
        assert!(apply_drop(&mut s, &l, "t1", &DropTarget::LooseEnd));
        assert_eq!(s.tabs.last().unwrap().id.as_deref(), Some("t1"));
    }

    #[test]
    fn drop_in_place_is_not_a_change() {
        let mut s = state();
        let l = layout(&s);
        // t4 dropped before t5 is where it already is
        assert!(!apply_drop(
            &mut s,
            &l,
            "t4",
            &DropTarget::Tab {
                id: "t5".into(),
                after: false
            }
        ));
        assert!(!apply_drop(
            &mut s,
            &l,
            "t4",
            &DropTarget::Tab {
                id: "t4".into(),
                after: false
            }
        ));
        assert!(!apply_drop(
            &mut s,
            &l,
            "t4",
            &DropTarget::Tab {
                id: "zz".into(),
                after: false
            }
        ));
    }

    #[test]
    fn half_of_a_row() {
        assert!(!is_after(3.0, 28.0));
        assert!(is_after(14.0, 28.0));
        assert!(!is_after(1.0, 0.0));
    }

    #[test]
    fn tab_menu_reflects_state() {
        let s = state();
        let labels = |k: MenuKind| -> Vec<String> {
            menu_items(&k, &s).into_iter().map(|m| m.label).collect()
        };
        assert_eq!(
            labels(MenuKind::Tab("t4".into())),
            ["Pin Tab", "Move to G", "New Group with Tab", "Close Tab"]
        );
        assert_eq!(
            labels(MenuKind::Tab("t2".into())),
            [
                "Pin Tab",
                "New Group with Tab",
                "Remove from Group",
                "Close Tab"
            ]
        );
        assert_eq!(labels(MenuKind::Tab("t1".into()))[0], "Unpin Tab");
        assert!(menu_items(&MenuKind::Tab("zz".into()), &s).is_empty());
        let g = labels(MenuKind::Group("g1".into()));
        assert_eq!(g[0], "Rename Group");
        assert_eq!(g[1], "Collapse Group");
        assert!(
            g.contains(&"Colour: olive".to_string()) && !g.contains(&"Colour: violet".to_string())
        );
        assert_eq!(g.last().unwrap(), "Delete Group");
        assert_eq!(labels(MenuKind::Sidebar), ["New Group"]);
    }

    #[test]
    fn menu_actions_change_state() {
        let mut s = state();
        assert_eq!(
            apply_menu_action(&mut s, &MenuAction::Unpin("t1".into())),
            None
        );
        assert!(!s.tabs[0].pinned);
        assert_eq!(
            apply_menu_action(&mut s, &MenuAction::Pin("t4".into())),
            None
        );
        let n = apply_menu_action(&mut s, &MenuAction::NewGroupWith("t5".into())).unwrap();
        assert_eq!(s.group(&n).unwrap().name, "New Group");
        assert_eq!(
            s.tabs
                .iter()
                .find(|t| t.id.as_deref() == Some("t5"))
                .unwrap()
                .group
                .as_deref(),
            Some(n.as_str())
        );
        apply_menu_action(
            &mut s,
            &MenuAction::ColorGroup {
                group: n.clone(),
                color: GroupColor::Stone,
            },
        );
        assert_eq!(s.group(&n).unwrap().color.as_deref(), Some("stone"));
        apply_menu_action(&mut s, &MenuAction::ToggleGroup(n.clone()));
        assert!(s.group(&n).unwrap().collapsed);
        assert_eq!(
            apply_menu_action(&mut s, &MenuAction::RenameGroup(n.clone())),
            Some(n.clone())
        );
        apply_menu_action(&mut s, &MenuAction::DeleteGroup(n.clone()));
        assert!(s.group(&n).is_none());
        assert!(s
            .tabs
            .iter()
            .find(|t| t.id.as_deref() == Some("t5"))
            .unwrap()
            .group
            .is_none());
        let g = apply_menu_action(&mut s, &MenuAction::NewGroup).unwrap();
        assert!(s.group(&g).is_some());
    }

    #[test]
    fn chip_status_by_shape_and_motion() {
        use std::time::Duration;
        let rc = ConnectionState::Reconnecting {
            attempt: 2,
            next_retry_in: Duration::from_secs(4),
        };
        assert_eq!(
            chip_status(&LinkState::Live(ConnectionState::Connected), true),
            ChipStatus::Connected
        );
        assert_eq!(
            chip_status(&LinkState::Live(rc.clone()), true),
            ChipStatus::Reconnecting
        );
        assert_eq!(
            chip_status(&LinkState::Connecting, true),
            ChipStatus::Reconnecting
        );
        assert_eq!(
            chip_status(&LinkState::Live(ConnectionState::NotRunning), true),
            ChipStatus::Stopped
        );
        assert_eq!(
            chip_status(&LinkState::Live(ConnectionState::NotRunning), false),
            ChipStatus::NotConnected
        );
        assert_eq!(
            chip_status(&LinkState::ProfileStopped, true),
            ChipStatus::Stopped
        );
        assert_eq!(chip_status(&LinkState::Off, true), ChipStatus::NotConnected);
        assert_eq!(
            chip_status(&LinkState::Live(ConnectionState::Closed), true),
            ChipStatus::NotConnected
        );
        assert!(tabs_live(&LinkState::Live(ConnectionState::Connected)));
        assert!(tabs_live(&LinkState::Live(rc)));
        assert!(!tabs_live(&LinkState::Off));
        assert!(!tabs_live(&LinkState::Live(ConnectionState::NotRunning)));
    }

    #[test]
    fn popover_offers_the_right_action() {
        let act = |st, link: LinkState| popover(st, &link, 3, 2, None).actions;
        assert_eq!(
            act(
                ChipStatus::Connected,
                LinkState::Live(ConnectionState::Connected)
            ),
            [ChipAction::NewTab, ChipAction::Disconnect]
        );
        assert_eq!(
            act(ChipStatus::Reconnecting, LinkState::Connecting),
            [ChipAction::RetryNow]
        );
        assert_eq!(
            act(ChipStatus::Stopped, LinkState::ProfileStopped),
            [ChipAction::Start]
        );
        assert_eq!(
            act(ChipStatus::NotConnected, LinkState::Off),
            [ChipAction::Connect]
        );
        let p = popover(ChipStatus::NotConnected, &LinkState::Off, 0, 1, None);
        assert!(p.text.contains("1 Placeholder Tab."), "{}", p.text);
        let p = popover(
            ChipStatus::NotConnected,
            &LinkState::Off,
            0,
            1,
            Some("Authentication failed"),
        );
        assert_eq!(p.text, "Authentication failed");
        assert!(ChipAction::Start.primary() && !ChipAction::Disconnect.primary());
        let p = popover(
            ChipStatus::Failed,
            &LinkState::Failed,
            0,
            0,
            Some("Authentication failed"),
        );
        assert_eq!(
            (p.text.as_str(), p.actions),
            ("Authentication failed", vec![ChipAction::RetryNow])
        );
        assert_eq!(chip_status(&LinkState::Failed, true), ChipStatus::Failed);
        assert_eq!(
            chip_status(
                &LinkState::Live(ConnectionState::Failed {
                    reason: rb_cdp::FailureReason::Unauthorized
                }),
                true
            ),
            ChipStatus::Failed
        );
        assert!(!tabs_live(&LinkState::Failed));
    }

    #[test]
    fn closed_targets_ignore_restarts() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let g = closed_targets(&v(&["A", "B"]), &v(&["A"]), true);
        assert_eq!(g, HashSet::from(["B".to_string()]));
        // Every id replaced: a browser restart, not a close.
        assert!(closed_targets(&v(&["A", "B"]), &v(&["X", "Y"]), true).is_empty());
        // The last Tab closed.
        assert_eq!(closed_targets(&v(&["A"]), &v(&[]), true).len(), 1);
        assert!(closed_targets(&v(&["A"]), &v(&[]), false).is_empty());
        assert!(closed_targets(&v(&[]), &v(&["A"]), true).is_empty());
    }

    #[test]
    fn prune_keeps_pinned_and_other_connections() {
        let mut s = state();
        s.reconcile("other", &[live("A", "http://o")]);
        let gone: HashSet<String> = ["A", "D"].iter().map(|s| s.to_string()).collect();
        // t1 (pinned, target A) stays; t4 (target D) goes; the other
        // Connection's "A" is untouched.
        assert_eq!(prune_closed(&mut s, "c", &gone), 1);
        assert!(s.tab_index("t1").is_some() && s.tab_index("t4").is_none());
        assert!(s.tabs.iter().any(|t| t.connection == "other"));
    }

    #[test]
    fn line_edit_keys() {
        let mut e = LineEdit::with_text("Old");
        assert_eq!(e.key("x", Some("x"), false), EditOutcome::Changed);
        assert_eq!(e.buf, "x"); // select-all replaced
        e.key("y", Some("y"), false);
        assert_eq!(e.key("backspace", None, false), EditOutcome::Changed);
        assert_eq!(e.buf, "x");
        assert_eq!(e.key("enter", None, false), EditOutcome::Commit);
        assert_eq!(e.key("escape", None, false), EditOutcome::Cancel);
        assert_eq!(e.key("left", None, false), EditOutcome::Ignored);
        assert_eq!(e.key("a", Some("a"), true), EditOutcome::Ignored);
        let mut e = LineEdit::with_text("abc");
        e.key("backspace", None, false);
        assert_eq!(e.buf, "");
    }
}
