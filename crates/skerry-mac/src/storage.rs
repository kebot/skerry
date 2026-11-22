//! Persistent storage's rules (`ARCH-037`): which homes the Storage window lists, which
//! paths are the mount points sessions need, where an upload may go, whether a home is in
//! use, and what the quota block says. No I/O, so every rule is tested here.

use crate::sealskin::{RemoteApp, RemoteSession, StorageEntry, SHARED_FILES_HOME};
use crate::selkies::{self, Entry};
use chrono::TimeZone;

/// What the Storage window shows on its right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoragePlace {
    Shared,
    Home(String),
}

impl StoragePlace {
    /// The home name the routes take.
    pub fn home(&self) -> &str {
        match self {
            Self::Shared => SHARED_FILES_HOME,
            Self::Home(home) => home,
        }
    }

    pub fn is_shared(&self) -> bool {
        matches!(self, Self::Shared)
    }

    /// The crumbs' root and the sidebar's label: "Shared Files", "work", or the app's name
    /// for an app home ("zen" for `auto-zen`).
    pub fn label(&self) -> String {
        match self {
            Self::Shared => "Shared Files".into(),
            Self::Home(home) => app_home_label(home).to_string(),
        }
    }

    /// The name the Delete alerts use: Shared Files, or the home's full name.
    pub fn full_name(&self) -> String {
        match self {
            Self::Shared => "Shared Files".into(),
            Self::Home(home) => home.clone(),
        }
    }
}

/// The sidebar's homes, each group sorted without case.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Homes {
    pub named: Vec<String>,
    /// `auto-*`, an app's own home.
    pub app: Vec<String>,
}

/// Sorts `homes` (as `GET /homedirs` lists them) into named and app homes. Shared Files and
/// `cleanroom` are never homes here.
pub fn sidebar_homes(homes: &[String]) -> Homes {
    let mut sorted: Vec<&String> = homes
        .iter()
        .filter(|home| {
            home.as_str() != SHARED_FILES_HOME && !home.eq_ignore_ascii_case("cleanroom")
        })
        .collect();
    sorted.sort_by_key(|home| (home.to_lowercase(), home.to_string()));
    sorted.dedup();
    let (app, named): (Vec<&String>, Vec<&String>) = sorted
        .into_iter()
        .partition(|home| home.starts_with("auto-"));
    Homes {
        named: named.into_iter().cloned().collect(),
        app: app.into_iter().cloned().collect(),
    }
}

/// An app home's row label: its name without `auto-`.
pub fn app_home_label(home: &str) -> &str {
    home.strip_prefix("auto-")
        .filter(|rest| !rest.is_empty())
        .unwrap_or(home)
}

/// The longest home name the New Home sheet takes; the server sets no limit.
pub const HOME_NAME_MAX: usize = 64;

/// What the New Home sheet's name field says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameCheck {
    /// Nothing typed: the hint shows and Create is off.
    Empty,
    Ok,
    Problem(String),
}

/// Checks a new home's name as the server's pattern does (`^[a-zA-Z0-9_-]+$`), plus the names
/// launches treat specially and the homes that exist.
pub fn check_home_name(name: &str, existing: &[String]) -> NameCheck {
    if name.is_empty() {
        return NameCheck::Empty;
    }
    let problem = if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        "Letters, numbers, - and _ only.".to_string()
    } else if name.len() > HOME_NAME_MAX {
        "Use 64 characters or fewer.".into()
    } else if name.eq_ignore_ascii_case("cleanroom") {
        "“cleanroom” is kept for sessions without storage.".into()
    } else if name == SHARED_FILES_HOME {
        "That name is kept for Shared Files.".into()
    } else if name.to_ascii_lowercase().starts_with("auto-") {
        "Names starting with “auto-” are kept for app homes.".into()
    } else if existing.iter().any(|home| home == name) {
        format!("A home named “{name}” already exists.")
    } else {
        return NameCheck::Ok;
    };
    NameCheck::Problem(problem)
}

/// A pane path (root-relative, no leading `/`) as the home-rooted path the routes take.
pub fn home_path(pane_path: &str) -> String {
    format!("/{}", pane_path.trim_matches('/'))
}

/// A home-rooted path as a pane path.
pub fn pane_path(home_path: &str) -> String {
    home_path.trim_matches('/').to_string()
}

/// A home's `Desktop` and its `Desktop/files` (the Shared Files mount point): sessions need
/// both, so they are never deleted. App homes are homes too; Shared Files has neither.
pub fn protected(place: &StoragePlace, pane_path: &str) -> bool {
    let path = pane_path.trim_matches('/');
    !place.is_shared() && (path == "Desktop" || path == "Desktop/files")
}

/// A home's `Desktop/files` and anything below it: the mount point, empty on the server and
/// hidden under Shared Files in every session.
pub fn opens_shared_files(place: &StoragePlace, pane_path: &str) -> bool {
    let path = pane_path.trim_matches('/');
    !place.is_shared() && (path == "Desktop/files" || path.starts_with("Desktop/files/"))
}

/// Where an upload into a folder goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadTarget {
    Here,
    /// Into a home's `Desktop/files`: anything written there would be hidden under the
    /// mount, so it goes to the top of Shared Files instead.
    SharedRoot,
}

/// Every upload entry point (drop, drop on a row, ⌘V, Upload…, Copy to Desktop) asks this.
pub fn upload_target(place: &StoragePlace, dir: &str) -> UploadTarget {
    if opens_shared_files(place, dir) {
        UploadTarget::SharedRoot
    } else {
        UploadTarget::Here
    }
}

/// `launch/file_path` opens a file by its name at the top of Shared Files only.
pub fn can_open_in(place: &StoragePlace, dir: &str, is_dir: bool) -> bool {
    place.is_shared() && dir.trim_matches('/').is_empty() && !is_dir
}

/// A storage listing's item as a Files pane entry in the folder `dir` (a pane path). Its
/// path is built from `dir` and its own name, never from the server's `path`; folders have
/// no size; the epoch time reads in `zone` as the ISO text the pane's dates parse.
pub fn entry_from<Z: TimeZone>(item: StorageEntry, dir: &str, zone: &Z) -> Entry
where
    Z::Offset: std::fmt::Display,
{
    let mtime = zone
        .timestamp_opt(item.mtime.floor() as i64, 0)
        .single()
        .map(|time| time.naive_local().format("%Y-%m-%dT%H:%M:%S").to_string())
        .unwrap_or_default();
    Entry {
        path: selkies::join(dir, &item.name),
        name: item.name,
        is_dir: item.is_dir,
        size: (!item.is_dir).then_some(item.size),
        mtime,
    }
}

/// SealSkin's `sanitize_for_filename`: trimmed, lower case, runs of spaces and underscores
/// as one hyphen, anything else outside `a-z0-9-` dropped, at most 50 characters, and
/// `unnamed` when nothing is left.
pub fn sanitize_for_filename(name: &str) -> String {
    let mut out = String::new();
    let mut in_gap = false;
    for c in name.trim().to_lowercase().chars() {
        if c.is_whitespace() || c == '_' {
            if !in_gap {
                out.push('-');
            }
            in_gap = true;
            continue;
        }
        in_gap = false;
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
            out.push(c);
        }
    }
    let out: String = out.chars().take(50).collect();
    if out.is_empty() {
        "unnamed".into()
    } else {
        out
    }
}

/// The home a meta app mounts whatever the launch sent: `auto-<sanitized name>`.
pub fn meta_auto_home(app_name: &str) -> String {
    format!("auto-{}", sanitize_for_filename(app_name))
}

/// Who uses a home right now, as far as this Mac can tell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InUse {
    /// The apps of the sessions (and launches) using it.
    pub apps: Vec<String>,
    /// A session of an app Skerry doesn't know, with no home listed, may be using this app
    /// home: treated as in use (fail closed).
    pub unsure: bool,
}

impl InUse {
    pub fn any(&self) -> bool {
        !self.apps.is_empty() || self.unsure
    }
}

/// The sessions and launches in flight that use `home`: a session whose `home` is it, a meta
/// app's session (the server mounts `auto-<name>` and lists no home), and a launch whose
/// resolved home is it.
pub fn sessions_using(
    home: &str,
    sessions: &[RemoteSession],
    apps: &[RemoteApp],
    launching: &[(String, String)],
) -> InUse {
    let mut used = InUse::default();
    for session in sessions {
        let app = apps.iter().find(|app| app.id == session.app_id);
        let meta_home = app
            .filter(|app| app.is_meta_app)
            .is_some_and(|app| meta_auto_home(&app.name) == home);
        if session.home == home || meta_home {
            used.apps.push(session.app_name.clone());
        } else if session.home.is_empty() && app.is_none() && home.starts_with("auto-") {
            used.unsure = true;
        }
    }
    for (app_name, launch_home) in launching {
        if launch_home == home {
            used.apps.push(app_name.clone());
        }
    }
    used
}

/// The in-use alert's title.
pub fn in_use_title(home: &str, used: &InUse) -> String {
    match used.apps.as_slice() {
        [] => format!("“{home}” may be in use by a running session."),
        [one] => format!("“{home}” is in use by {one}."),
        [one, rest @ ..] => {
            let others = if rest.len() == 1 {
                "1 other session".to_string()
            } else {
                format!("{} other sessions", rest.len())
            };
            format!("“{home}” is in use by {one} and {others}.")
        }
    }
}

/// How full the quota meter is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaLevel {
    Normal,
    Warning,
    Full,
}

/// What the sidebar's quota block shows once storage is measured.
#[derive(Clone, Debug, PartialEq)]
pub struct Quota {
    pub text: String,
    /// `None`: unlimited, so no meter.
    pub fraction: Option<f32>,
    pub level: QuotaLevel,
}

const GIB: f64 = 1024. * 1024. * 1024.;

/// The quota for `used` bytes of `limit_gb` (binary GB; negative is unlimited). An
/// administrator has no limit.
pub fn quota(used: u64, limit_gb: i64, admin: bool) -> Quota {
    let limit = if admin { -1 } else { limit_gb };
    let mut text = skerry_ui::account::storage_text(used, limit);
    if limit >= 0 {
        text.push_str(" used");
    }
    let fraction = skerry_ui::account::meter_fraction(used as f64, limit as f64 * GIB);
    let level = match fraction {
        Some(f) if f >= 1. => QuotaLevel::Full,
        Some(f) if f >= 0.8 => QuotaLevel::Warning,
        _ => QuotaLevel::Normal,
    };
    Quota {
        text,
        fraction,
        level,
    }
}

/// The Filter field: the name contains the text, ignoring case.
pub fn matches_filter(name: &str, filter: &str) -> bool {
    let filter = filter.trim();
    filter.is_empty() || name.to_lowercase().contains(&filter.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn homes(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn sidebar_puts_shared_files_first_then_homes_then_app_homes() {
        let listed = homes(&[
            "work",
            "_sealskin_shared_files",
            "auto-zen",
            "Default",
            "cleanroom",
            "auto-firefox",
        ]);
        let sorted = sidebar_homes(&listed);
        assert_eq!(sorted.named, ["Default", "work"]);
        assert_eq!(sorted.app, ["auto-firefox", "auto-zen"]);
        assert_eq!(app_home_label("auto-zen"), "zen");
        assert_eq!(app_home_label("auto-"), "auto-");
        assert_eq!(StoragePlace::Home("auto-zen".into()).label(), "zen");
        assert_eq!(
            StoragePlace::Home("auto-zen".into()).full_name(),
            "auto-zen"
        );
        assert_eq!(StoragePlace::Shared.home(), SHARED_FILES_HOME);
        assert_eq!(StoragePlace::Shared.label(), "Shared Files");
    }

    #[test]
    fn new_home_names_follow_the_server_pattern_and_refuse_reserved_ones() {
        let existing = homes(&["work"]);
        assert_eq!(check_home_name("", &existing), NameCheck::Empty);
        assert_eq!(check_home_name("games_2-b", &existing), NameCheck::Ok);
        let problem = |name: &str| match check_home_name(name, &existing) {
            NameCheck::Problem(text) => text,
            other => panic!("{name}: {other:?}"),
        };
        assert_eq!(problem("my home"), "Letters, numbers, - and _ only.");
        assert_eq!(problem("café"), "Letters, numbers, - and _ only.");
        assert_eq!(problem(&"a".repeat(65)), "Use 64 characters or fewer.");
        assert!(matches!(
            check_home_name(&"a".repeat(64), &existing),
            NameCheck::Ok
        ));
        assert_eq!(
            problem("CleanRoom"),
            "“cleanroom” is kept for sessions without storage."
        );
        assert_eq!(
            problem("_sealskin_shared_files"),
            "That name is kept for Shared Files."
        );
        assert_eq!(
            problem("Auto-zen"),
            "Names starting with “auto-” are kept for app homes."
        );
        assert_eq!(problem("work"), "A home named “work” already exists.");
    }

    #[test]
    fn protected_paths_are_the_desktop_and_its_files_mount_in_homes_only() {
        let work = StoragePlace::Home("work".into());
        let app = StoragePlace::Home("auto-zen".into());
        for place in [&work, &app] {
            assert!(protected(place, "Desktop"));
            assert!(protected(place, "/Desktop/files"));
            assert!(!protected(place, "Desktop/files/a.txt"));
            assert!(!protected(place, "Documents"));
        }
        assert!(!protected(&StoragePlace::Shared, "Desktop"));
        assert!(!protected(&StoragePlace::Shared, "Desktop/files"));
    }

    #[test]
    fn desktop_files_in_a_home_opens_shared_files() {
        let work = StoragePlace::Home("work".into());
        assert!(opens_shared_files(&work, "Desktop/files"));
        assert!(opens_shared_files(&work, "Desktop/files/sub"));
        assert!(!opens_shared_files(&work, "Desktop"));
        assert!(!opens_shared_files(&work, "Desktop/filesystem"));
        assert!(!opens_shared_files(&StoragePlace::Shared, "Desktop/files"));
    }

    #[test]
    fn uploads_into_a_homes_mount_point_go_to_shared_files() {
        let work = StoragePlace::Home("work".into());
        let app = StoragePlace::Home("auto-zen".into());
        assert_eq!(
            upload_target(&work, "Desktop/files"),
            UploadTarget::SharedRoot
        );
        assert_eq!(
            upload_target(&app, "Desktop/files/x"),
            UploadTarget::SharedRoot
        );
        assert_eq!(upload_target(&work, "Desktop"), UploadTarget::Here);
        assert_eq!(upload_target(&work, ""), UploadTarget::Here);
        assert_eq!(
            upload_target(&StoragePlace::Shared, "Desktop/files"),
            UploadTarget::Here
        );
    }

    #[test]
    fn open_in_is_offered_for_top_level_shared_files_only() {
        assert!(can_open_in(&StoragePlace::Shared, "", false));
        assert!(!can_open_in(&StoragePlace::Shared, "", true));
        assert!(!can_open_in(&StoragePlace::Shared, "Reports", false));
        assert!(!can_open_in(&StoragePlace::Home("work".into()), "", false));
    }

    fn item(name: &str, is_dir: bool, size: u64, mtime: f64) -> StorageEntry {
        StorageEntry {
            name: name.into(),
            path: format!("/elsewhere/{name}"),
            is_dir,
            size,
            mtime,
        }
    }

    #[test]
    fn epoch_mtimes_read_in_local_time() {
        let zone = chrono::FixedOffset::east_opt(3600).unwrap();
        // 2026-11-22T10:04:05Z.
        let entry = entry_from(item("a.pdf", false, 2048, 1_795_341_845.75), "Docs", &zone);
        assert_eq!(entry.mtime, "2026-11-22T11:04:05");
        assert_eq!(
            entry.path, "Docs/a.pdf",
            "built from the folder, not the server"
        );
        assert_eq!(entry.size, Some(2048));
    }

    #[test]
    fn folder_sizes_read_as_none() {
        let entry = entry_from(item("Reports", true, 4096, 0.), "", &chrono::Utc);
        assert_eq!(entry.size, None);
        assert_eq!(entry.path, "Reports");
        assert_eq!(home_path("Reports/x"), "/Reports/x");
        assert_eq!(home_path(""), "/");
        assert_eq!(pane_path("/Desktop/x/"), "Desktop/x");
    }

    fn session(app_id: &str, app_name: &str, home: &str) -> RemoteSession {
        serde_json::from_value(json!({
            "session_id": format!("s-{app_id}"),
            "app_id": app_id,
            "app_name": app_name,
            "session_url": "/s/",
            "home": home,
        }))
        .unwrap()
    }

    fn app(id: &str, name: &str, meta: bool) -> RemoteApp {
        serde_json::from_value(json!({"id": id, "name": name, "is_meta_app": meta})).unwrap()
    }

    #[test]
    fn sessions_using_a_home_are_named() {
        let sessions = [
            session("zen", "Zen", "work"),
            session("code", "VS Code", "work"),
            session("gimp", "GIMP", "other"),
        ];
        let apps = [app("zen", "Zen", false), app("code", "VS Code", false)];
        let used = sessions_using("work", &sessions, &apps, &[]);
        assert_eq!(used.apps, ["Zen", "VS Code"]);
        assert!(!used.unsure);
        assert_eq!(
            in_use_title("work", &used),
            "“work” is in use by Zen and 1 other session."
        );
        assert_eq!(
            in_use_title(
                "work",
                &InUse {
                    apps: vec!["Zen".into()],
                    unsure: false
                }
            ),
            "“work” is in use by Zen."
        );
        assert!(!sessions_using("free", &sessions, &apps, &[]).any());
    }

    #[test]
    fn meta_app_sessions_use_their_auto_home_without_a_home_field() {
        let sessions = [session("desk", "  My Desk_Top ", "")];
        let apps = [app("desk", "  My Desk_Top ", true)];
        assert_eq!(meta_auto_home("  My Desk_Top "), "auto-my-desk-top");
        assert_eq!(meta_auto_home("!!!"), "auto-unnamed");
        assert_eq!(sanitize_for_filename(&"x".repeat(80)).len(), 50);
        let used = sessions_using("auto-my-desk-top", &sessions, &apps, &[]);
        assert_eq!(used.apps, ["  My Desk_Top "]);
        // An app this Mac doesn't know, with no home listed: maybe this app home.
        let unknown = [session("ghost", "Ghost", "")];
        let used = sessions_using("auto-zen", &unknown, &apps, &[]);
        assert!(used.unsure && used.any());
        assert!(!sessions_using("work", &unknown, &apps, &[]).any());
    }

    #[test]
    fn a_launch_in_flight_blocks_delete_home() {
        let launching = [("Zen".to_string(), "auto-zen".to_string())];
        let used = sessions_using("auto-zen", &[], &[], &launching);
        assert_eq!(used.apps, ["Zen"]);
        assert!(!sessions_using("work", &[], &[], &launching).any());
    }

    #[test]
    fn quota_levels_warn_at_80_and_fill_at_100() {
        let gib = 1024 * 1024 * 1024;
        let normal = quota(10 * gib, 50, false);
        assert_eq!(normal.text, "10.0 GB of 50 GB used");
        assert_eq!(normal.level, QuotaLevel::Normal);
        assert_eq!(quota(42 * gib, 50, false).level, QuotaLevel::Warning);
        assert_eq!(quota(50 * gib, 50, false).level, QuotaLevel::Full);
        let unlimited = quota(12 * gib, -1, false);
        assert_eq!(unlimited.text, "12.0 GB used");
        assert_eq!(unlimited.fraction, None);
        let admin = quota(12 * gib, 5, true);
        assert_eq!(admin.text, "12.0 GB used");
        assert_eq!(admin.level, QuotaLevel::Normal);
    }

    #[test]
    fn filter_matches_names_without_case() {
        assert!(matches_filter("Budget 2026.xlsx", "budget"));
        assert!(matches_filter("anything", "  "));
        assert!(!matches_filter("notes.txt", "budget"));
    }
}
