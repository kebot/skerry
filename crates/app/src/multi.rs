//! Pure logic for several Connections in one window: startup connects,
//! duplicate colour/initial warnings, popup placement, which Tab becomes
//! Active, Profile probe results, chip overflow. No GPUI, no I/O.

use std::collections::HashSet;

use rb_cdp::TargetInfo;
use rb_store::State;

use crate::connect::Prepared;
use crate::sidebar_model::ConnStyle;

/// Chips shown in the row; more Connections overflow into a menu.
pub const MAX_CHIPS: usize = 8;

/// Which Connections to connect at startup: the one named on the command line
/// (`selected`), every `auto_connect` one, and the only Connection of a
/// single-Connection config (an empty window would be useless). Everything
/// else connects lazily, on click.
pub fn startup_connects(auto: &[bool], selected: Option<usize>) -> Vec<usize> {
    (0..auto.len())
        .filter(|&i| auto[i] || selected == Some(i) || auto.len() == 1)
        .collect()
}

/// Warnings for Connections that are hard to tell apart: the same initial, or
/// the same colour when at least one of the two was set in `config.toml`
/// (automatic colours repeat after four Connections, which is not a mistake).
pub fn style_warnings(styles: &[ConnStyle]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, a) in styles.iter().enumerate() {
        for b in &styles[i + 1..] {
            if a.initial == b.initial {
                out.push(format!(
                    "Connections \"{}\" and \"{}\" share the initial {}",
                    a.name, b.name, a.initial
                ));
            }
            if (a.custom.is_some() || b.custom.is_some()) && a.color(false) == b.color(false) {
                out.push(format!(
                    "Connections \"{}\" and \"{}\" have the same colour",
                    a.name, b.name
                ));
            }
        }
    }
    out
}

/// New page targets that were opened by another page of the same Connection:
/// `(target_id, opener_id)`. Only popups whose opener is a known page count.
pub fn new_popups(old: &[TargetInfo], new: &[TargetInfo]) -> Vec<(String, String)> {
    new.iter()
        .filter(|p| !old.iter().any(|o| o.target_id == p.target_id))
        .filter_map(|p| {
            let opener = p.opener_id.as_ref()?;
            new.iter()
                .any(|o| &o.target_id == opener)
                .then(|| (p.target_id.clone(), opener.clone()))
        })
        .collect()
}

/// Move the stored popup Tab right behind its opener and give it the
/// opener's Tab Group (a pinned opener's popup stays unpinned). Returns
/// whether anything moved.
pub fn place_popup(state: &mut State, connection: &str, popup: &str, opener: &str) -> bool {
    let find = |s: &State, t: &str| {
        s.tabs
            .iter()
            .position(|e| e.connection == connection && e.target_id.as_deref() == Some(t))
    };
    let (Some(pi), Some(oi)) = (find(state, popup), find(state, opener)) else {
        return false;
    };
    if pi == oi {
        return false;
    }
    let before = state.clone();
    let mut entry = state.tabs.remove(pi);
    let oi = find(state, opener).unwrap_or(oi.min(state.tabs.len().saturating_sub(1)));
    entry.pinned = false;
    entry.group = state.tabs[oi].group.clone();
    state.tabs.insert(oi + 1, entry);
    *state != before
}

/// A popup becomes the Active Tab when its opener is the Active Tab of the
/// same Connection: the user is looking at the opener, so the popup is almost
/// always the result of their click. A popup of a background Tab (or an Agent
/// working elsewhere) appears in the Sidebar without taking over.
pub fn popup_activates(active: Option<(usize, &str)>, conn: usize, opener: &str) -> bool {
    active == Some((conn, opener))
}

/// What a Profile probe (status check of a Connection that is not running)
/// changes. `idle` is true while the Connection is Idle or already shows a
/// Blocked overlay; a running or connecting Connection ignores probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeEffect {
    /// Show "Profile stopped" with a Start button.
    Stopped,
    /// The Profile runs (or the Manager is gone): back to plain not-connected.
    Idle,
    /// Show this failure (Authentication failed, cannot reach the Manager).
    Failed,
    Ignore,
}

pub fn probe_effect(p: &Prepared, idle: bool) -> ProbeEffect {
    if !idle {
        return ProbeEffect::Ignore;
    }
    match p {
        Prepared::NeedsStart => ProbeEffect::Stopped,
        Prepared::Ready => ProbeEffect::Idle,
        Prepared::Failed { .. } => ProbeEffect::Failed,
    }
}

/// The chips drawn in the row and the ones in the overflow menu.
pub fn overflow_split(n: usize) -> (usize, usize) {
    if n <= MAX_CHIPS {
        (n, 0)
    } else {
        (MAX_CHIPS - 1, n - (MAX_CHIPS - 1))
    }
}

/// The Active Tab after the pages of Connection `conn` changed.
///
/// * the Active Tab is still there: keep it;
/// * the Active Tab (of this Connection) is gone: the first page of the same
///   Connection, else nothing;
/// * no Active Tab: the first page of this Connection, unless `hold` says a
///   preferred Connection is still connecting.
pub fn choose_active(
    active: Option<(usize, &str)>,
    conn: usize,
    pages: &[TargetInfo],
    hold: bool,
) -> Option<(usize, String)> {
    match active {
        Some((c, t)) if c != conn => Some((c, t.to_string())),
        Some((c, t)) if pages.iter().any(|p| p.target_id == t) => Some((c, t.to_string())),
        Some(_) => pages.first().map(|p| (conn, p.target_id.clone())),
        None if hold => None,
        None => pages.first().map(|p| (conn, p.target_id.clone())),
    }
}

/// Connections whose ids in `all` are not in `streaming`: which Connection
/// must stop its screencast when the Active Tab moves to `new`. Only one
/// Connection streams at a time.
pub fn stop_streaming(streaming: Option<usize>, new: Option<usize>) -> Option<usize> {
    streaming.filter(|s| Some(*s) != new)
}

/// Target ids that have a pending dialog, for Sidebar badges.
pub fn dialog_targets(dialogs: &[rb_cdp::Dialog]) -> HashSet<String> {
    dialogs.iter().map(|d| d.target_id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rb_store::TabEntry;

    fn page(id: &str, opener: Option<&str>) -> TargetInfo {
        TargetInfo {
            target_id: id.into(),
            kind: "page".into(),
            title: String::new(),
            url: format!("https://{id}/"),
            attached: false,
            opener_id: opener.map(str::to_string),
        }
    }

    fn entry(conn: &str, id: &str, group: Option<&str>, pinned: bool) -> TabEntry {
        TabEntry {
            id: Some(format!("t-{id}")),
            connection: conn.into(),
            target_id: Some(id.into()),
            group: group.map(str::to_string),
            pinned,
            ..Default::default()
        }
    }

    #[test]
    fn startup_connects_selected_auto_and_lone() {
        assert_eq!(startup_connects(&[false, true, false], None), vec![1]);
        assert_eq!(startup_connects(&[false, false, false], Some(2)), vec![2]);
        assert_eq!(
            startup_connects(&[true, false, true], Some(1)),
            vec![0, 1, 2]
        );
        assert_eq!(startup_connects(&[false], None), vec![0]);
        assert!(startup_connects(&[false, false], None).is_empty());
    }

    #[test]
    fn warns_about_duplicate_initials_and_colours_only() {
        let s = |n: &str, c: Option<&str>, i| ConnStyle::new(n, c, i);
        let w = style_warnings(&[s("work", None, 0), s("web", None, 1)]);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("initial W"), "{w:?}");
        let w = style_warnings(&[
            s("a", Some("#ff0000"), 0),
            s("b", Some("#ff0000"), 1),
            s("c", None, 2),
        ]);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("same colour"));
        // Five automatic colours repeat the first one: no warning.
        let many: Vec<ConnStyle> = (0..5)
            .map(|i| s(&format!("{}x", (b'a' + i as u8) as char), None, i))
            .collect();
        assert!(style_warnings(&many).is_empty());
    }

    #[test]
    fn popups_need_a_known_opener_and_are_new() {
        let old = [page("A", None)];
        let new = [
            page("A", None),
            page("P", Some("A")),
            page("Q", Some("gone")),
            page("R", None),
        ];
        assert_eq!(
            new_popups(&old, &new),
            vec![("P".to_string(), "A".to_string())]
        );
        assert!(new_popups(&new, &new).is_empty());
    }

    #[test]
    fn popup_is_placed_behind_its_opener_in_its_group() {
        let mut s = State {
            tabs: vec![
                entry("c", "A", Some("g1"), false),
                entry("c", "B", None, false),
                entry("c", "P", None, false),
            ],
            ..Default::default()
        };
        assert!(place_popup(&mut s, "c", "P", "A"));
        let order: Vec<_> = s
            .tabs
            .iter()
            .map(|t| t.target_id.clone().unwrap())
            .collect();
        assert_eq!(order, ["A", "P", "B"]);
        assert_eq!(s.tabs[1].group.as_deref(), Some("g1"));
        // Pinned opener: the popup is not pinned.
        let mut s = State {
            tabs: vec![
                entry("c", "A", None, true),
                entry("c", "P", Some("g"), true),
            ],
            ..Default::default()
        };
        assert!(place_popup(&mut s, "c", "P", "A"));
        assert!(!s.tabs[1].pinned && s.tabs[1].group.is_none());
        // Other Connection's Tab with the same target id is not an opener.
        let mut s = State {
            tabs: vec![
                entry("other", "A", None, false),
                entry("c", "P", None, false),
            ],
            ..Default::default()
        };
        assert!(!place_popup(&mut s, "c", "P", "A"));
    }

    #[test]
    fn popup_activates_only_from_the_active_opener() {
        assert!(popup_activates(Some((1, "A")), 1, "A"));
        assert!(!popup_activates(Some((0, "A")), 1, "A"));
        assert!(!popup_activates(Some((1, "B")), 1, "A"));
        assert!(!popup_activates(None, 1, "A"));
    }

    #[test]
    fn probe_effects() {
        let failed = Prepared::Failed {
            title: "t".into(),
            detail: "d".into(),
        };
        assert_eq!(
            probe_effect(&Prepared::NeedsStart, true),
            ProbeEffect::Stopped
        );
        assert_eq!(probe_effect(&Prepared::Ready, true), ProbeEffect::Idle);
        assert_eq!(probe_effect(&failed, true), ProbeEffect::Failed);
        assert_eq!(
            probe_effect(&Prepared::NeedsStart, false),
            ProbeEffect::Ignore
        );
    }

    #[test]
    fn overflow_starts_beyond_eight() {
        assert_eq!(overflow_split(3), (3, 0));
        assert_eq!(overflow_split(8), (8, 0));
        assert_eq!(overflow_split(11), (7, 4));
    }

    #[test]
    fn active_choice_across_connections() {
        let pages = [page("A", None), page("B", None)];
        // Another Connection's Active Tab is never touched.
        assert_eq!(
            choose_active(Some((0, "X")), 1, &pages, false),
            Some((0, "X".into()))
        );
        assert_eq!(
            choose_active(Some((1, "B")), 1, &pages, false),
            Some((1, "B".into()))
        );
        assert_eq!(
            choose_active(Some((1, "gone")), 1, &pages, false),
            Some((1, "A".into()))
        );
        assert_eq!(choose_active(Some((1, "gone")), 1, &[], false), None);
        assert_eq!(choose_active(None, 1, &pages, false), Some((1, "A".into())));
        assert_eq!(choose_active(None, 1, &pages, true), None);
    }

    #[test]
    fn only_the_previous_streamer_stops() {
        assert_eq!(stop_streaming(Some(0), Some(1)), Some(0));
        assert_eq!(stop_streaming(Some(1), Some(1)), None);
        assert_eq!(stop_streaming(None, Some(1)), None);
        assert_eq!(stop_streaming(Some(0), None), Some(0));
    }
}
