//! The Files window's browsing state (`ARCH-023`): the folder shown, its sorted entries, the
//! selection, and Back/Forward history. No I/O, so it is tested on its own.

use crate::selkies::{self, Entry};
use skerry_ui::files::{Crumb, FileRow, SortColumn};
use std::cmp::Ordering;
use std::collections::BTreeSet;

pub struct Browser {
    /// The folder shown, root-relative.
    path: String,
    /// The listing in display order.
    entries: Vec<Entry>,
    /// Indices into `entries`.
    selected: BTreeSet<usize>,
    /// Where Shift-selection extends from.
    anchor: Option<usize>,
    back: Vec<String>,
    forward: Vec<String>,
    pub sort: SortColumn,
    pub ascending: bool,
}

impl Default for Browser {
    fn default() -> Self {
        Browser {
            path: String::new(),
            entries: Vec::new(),
            selected: BTreeSet::new(),
            anchor: None,
            back: Vec::new(),
            forward: Vec::new(),
            sort: SortColumn::Name,
            ascending: true,
        }
    }
}

impl Browser {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Moves to `path`, remembering the folder left for Back. Returns whether it moved.
    pub fn navigate(&mut self, path: &str) -> bool {
        let path = path.trim_matches('/').to_owned();
        if path == self.path {
            return false;
        }
        self.back.push(std::mem::replace(&mut self.path, path));
        self.forward.clear();
        self.clear_folder();
        true
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn back(&mut self) -> bool {
        let Some(path) = self.back.pop() else {
            return false;
        };
        self.forward.push(std::mem::replace(&mut self.path, path));
        self.clear_folder();
        true
    }

    pub fn forward(&mut self) -> bool {
        let Some(path) = self.forward.pop() else {
            return false;
        };
        self.back.push(std::mem::replace(&mut self.path, path));
        self.clear_folder();
        true
    }

    /// The enclosing folder, or `None` at the root.
    pub fn parent(&self) -> Option<String> {
        (!self.path.is_empty()).then(|| selkies::parent(&self.path).to_owned())
    }

    fn clear_folder(&mut self) {
        self.entries.clear();
        self.selected.clear();
        self.anchor = None;
    }

    /// Shows a fresh listing of the current folder, keeping selected names selected.
    pub fn set_listing(&mut self, entries: Vec<Entry>) {
        let keep: BTreeSet<String> = self
            .selected_entries()
            .into_iter()
            .map(|entry| entry.name.clone())
            .collect();
        let anchor = self
            .anchor
            .and_then(|index| self.entries.get(index))
            .map(|entry| entry.name.clone());
        self.entries = entries;
        self.sort_entries();
        self.reselect(&keep, anchor.as_deref());
    }

    /// Sorts by `column`; the same column again flips the direction.
    pub fn sort_by(&mut self, column: SortColumn) {
        if self.sort == column {
            self.ascending = !self.ascending;
        } else {
            self.sort = column;
            self.ascending = column == SortColumn::Name;
        }
        let keep: BTreeSet<String> = self
            .selected_entries()
            .into_iter()
            .map(|entry| entry.name.clone())
            .collect();
        let anchor = self
            .anchor
            .and_then(|index| self.entries.get(index))
            .map(|entry| entry.name.clone());
        self.sort_entries();
        self.reselect(&keep, anchor.as_deref());
    }

    fn reselect(&mut self, names: &BTreeSet<String>, anchor: Option<&str>) {
        self.selected = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| names.contains(&entry.name))
            .map(|(index, _)| index)
            .collect();
        self.anchor = anchor.and_then(|name| self.entries.iter().position(|e| e.name == name));
    }

    fn sort_entries(&mut self) {
        let (column, ascending) = (self.sort, self.ascending);
        self.entries.sort_by(|a, b| {
            // Folders stay on top in either direction, as Finder's "Keep folders on top".
            b.is_dir.cmp(&a.is_dir).then_with(|| {
                let order = match column {
                    SortColumn::Name => by_name(a, b),
                    SortColumn::Modified => a.mtime.cmp(&b.mtime).then_with(|| by_name(a, b)),
                    SortColumn::Size => a.size.cmp(&b.size).then_with(|| by_name(a, b)),
                };
                if ascending {
                    order
                } else {
                    order.reverse()
                }
            })
        });
    }

    /// A click on row `index`.
    pub fn select(&mut self, index: usize, extend: bool, toggle: bool) {
        if index >= self.entries.len() {
            return;
        }
        if toggle {
            if !self.selected.remove(&index) {
                self.selected.insert(index);
            }
            self.anchor = Some(index);
        } else if extend {
            let anchor = self.anchor.unwrap_or(index);
            let (low, high) = (anchor.min(index), anchor.max(index));
            self.selected = (low..=high).collect();
        } else {
            self.selected = BTreeSet::from([index]);
            self.anchor = Some(index);
        }
    }

    /// Arrow keys: move the selection by `delta` rows, or grow it with Shift.
    pub fn step(&mut self, delta: isize, extend: bool) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() - 1;
        let current = if delta < 0 {
            self.selected.first().copied()
        } else {
            self.selected.last().copied()
        };
        let target = match current {
            None if delta < 0 => last,
            None => 0,
            Some(index) => index.saturating_add_signed(delta).min(last),
        };
        if extend {
            self.anchor.get_or_insert(current.unwrap_or(target));
            self.select(target, true, false);
        } else {
            self.select(target, false, false);
        }
    }

    pub fn select_all(&mut self) {
        self.selected = (0..self.entries.len()).collect();
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    pub fn selected_entries(&self) -> Vec<&Entry> {
        self.selected
            .iter()
            .filter_map(|index| self.entries.get(*index))
            .collect()
    }

    /// The first selected row, for Return and scrolling.
    pub fn focused(&self) -> Option<usize> {
        self.selected.first().copied()
    }

    pub fn rows(&self) -> Vec<FileRow> {
        let today = today();
        self.entries
            .iter()
            .enumerate()
            .map(|(index, entry)| FileRow {
                name: entry.name.clone(),
                is_dir: entry.is_dir,
                modified: date_label(&entry.mtime, today),
                size: entry.size.map(size_label).unwrap_or_else(|| "--".into()),
                selected: self.selected.contains(&index),
            })
            .collect()
    }

    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    /// The crumbs from the file-manager root (`root`) to the current folder.
    pub fn crumbs(&self, root: &str) -> Vec<Crumb> {
        let mut crumbs = vec![Crumb {
            label: root.to_owned(),
            path: String::new(),
        }];
        let mut path = String::new();
        for part in self.path.split('/').filter(|part| !part.is_empty()) {
            path = selkies::join(&path, part);
            crumbs.push(Crumb {
                label: part.to_owned(),
                path: path.clone(),
            });
        }
        crumbs
    }
}

fn by_name(a: &Entry, b: &Entry) -> Ordering {
    a.name
        .to_lowercase()
        .cmp(&b.name.to_lowercase())
        .then_with(|| a.name.cmp(&b.name))
}

/// Finder's sizes: decimal units, one decimal place below 10.
pub fn size_label(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return if bytes == 1 {
            "1 byte".into()
        } else {
            format!("{bytes} bytes")
        };
    }
    let mut value = bytes as f64 / 1000.;
    let mut unit = 0;
    while value >= 999.95 && unit < UNITS.len() - 1 {
        value /= 1000.;
        unit += 1;
    }
    if value < 9.95 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// Finder's dates: `Today at 10:04`, `Yesterday at 10:04`, else `19 Nov 2026 at 11:40`. The
/// server sends its own local time without a zone (`2026-11-21T10:04:05.5`); it is shown as is.
pub fn date_label(mtime: &str, today: chrono::NaiveDate) -> String {
    let Some(time) = chrono::NaiveDateTime::parse_from_str(mtime, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .or_else(|| chrono::NaiveDateTime::parse_from_str(mtime, "%Y-%m-%dT%H:%M:%S").ok())
    else {
        return mtime.to_owned();
    };
    let clock = time.format("%H:%M");
    if time.date() == today {
        format!("Today at {clock}")
    } else if today.pred_opt() == Some(time.date()) {
        format!("Yesterday at {clock}")
    } else {
        format!("{} at {clock}", time.format("%-d %b %Y"))
    }
}

/// Today on this Mac, for [`date_label`].
pub fn today() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool, size: Option<u64>, mtime: &str) -> Entry {
        Entry {
            name: name.into(),
            path: name.into(),
            is_dir,
            size,
            mtime: mtime.into(),
        }
    }

    fn listing() -> Vec<Entry> {
        vec![
            entry("b.txt", false, Some(30), "2026-01-03T00:00:00"),
            entry("Zeta", true, None, "2026-01-01T00:00:00"),
            entry("A.txt", false, Some(10), "2026-01-02T00:00:00"),
            entry("alpha", true, None, "2026-01-04T00:00:00"),
        ]
    }

    fn names(browser: &Browser) -> Vec<&str> {
        browser.entries().iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn folders_stay_on_top_and_names_sort_without_case() {
        let mut browser = Browser::default();
        browser.set_listing(listing());
        assert_eq!(names(&browser), ["alpha", "Zeta", "A.txt", "b.txt"]);
        browser.sort_by(SortColumn::Name);
        assert_eq!(names(&browser), ["Zeta", "alpha", "b.txt", "A.txt"]);
        browser.sort_by(SortColumn::Size);
        assert!(!browser.ascending);
        assert_eq!(names(&browser), ["Zeta", "alpha", "b.txt", "A.txt"]);
        browser.sort_by(SortColumn::Size);
        assert_eq!(names(&browser), ["alpha", "Zeta", "A.txt", "b.txt"]);
    }

    #[test]
    fn selection_survives_a_resort_and_a_refresh() {
        let mut browser = Browser::default();
        browser.set_listing(listing());
        browser.select(2, false, false);
        browser.select(3, false, true);
        browser.sort_by(SortColumn::Name);
        let selected: Vec<_> = browser
            .selected_entries()
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(selected, ["b.txt", "A.txt"]);
        browser.set_listing(listing());
        assert_eq!(browser.selected_count(), 2);
    }

    #[test]
    fn shift_and_command_clicks_select_like_finder() {
        let mut browser = Browser::default();
        browser.set_listing(listing());
        browser.select(1, false, false);
        browser.select(3, true, false);
        assert_eq!(browser.selected_count(), 3);
        browser.select(2, false, true);
        assert_eq!(browser.selected_count(), 2);
        browser.select(0, false, false);
        assert_eq!(browser.selected_count(), 1);
        browser.select(9, false, false);
        assert_eq!(browser.focused(), Some(0));
    }

    #[test]
    fn arrow_keys_move_and_extend_the_selection() {
        let mut browser = Browser::default();
        browser.set_listing(listing());
        browser.step(1, false);
        assert_eq!(browser.focused(), Some(0));
        browser.step(1, false);
        assert_eq!(browser.focused(), Some(1));
        browser.step(1, true);
        assert_eq!(browser.selected_count(), 2);
        browser.step(10, false);
        assert_eq!(browser.focused(), Some(3));
        browser.clear_selection();
        browser.step(-1, false);
        assert_eq!(browser.focused(), Some(3));
    }

    #[test]
    fn history_goes_back_and_forward() {
        let mut browser = Browser::default();
        assert!(!browser.navigate(""));
        assert!(browser.navigate("Docs"));
        assert!(browser.navigate("/Docs/sub/"));
        assert_eq!(browser.path(), "Docs/sub");
        assert_eq!(browser.parent().as_deref(), Some("Docs"));
        assert!(browser.back());
        assert_eq!(browser.path(), "Docs");
        assert!(browser.can_forward());
        assert!(browser.forward());
        assert_eq!(browser.path(), "Docs/sub");
        browser.back();
        browser.navigate("Other");
        assert!(!browser.can_forward());
        let crumbs = browser.crumbs("Desktop");
        assert_eq!(crumbs.len(), 2);
        assert_eq!(crumbs[1].path, "Other");
    }

    #[test]
    fn sizes_and_dates_read_like_finder() {
        assert_eq!(size_label(0), "0 bytes");
        assert_eq!(size_label(1), "1 byte");
        assert_eq!(size_label(999), "999 bytes");
        assert_eq!(size_label(1_500), "1.5 KB");
        assert_eq!(size_label(42_000_000), "42 MB");
        assert_eq!(size_label(999_999), "1.0 MB");
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        assert_eq!(date_label("2026-11-21T10:04:05.5", today), "Today at 10:04");
        assert_eq!(
            date_label("2026-11-20T23:59:00", today),
            "Yesterday at 23:59"
        );
        assert_eq!(
            date_label("2026-11-02T08:05:00", today),
            "2 Nov 2026 at 08:05"
        );
        assert_eq!(date_label("odd", today), "odd");
    }
}
