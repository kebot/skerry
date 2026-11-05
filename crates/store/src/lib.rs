//! rb-store: `config.toml` (Connections, tokens) and `state.toml` (Tab Groups,
//! pins, Placeholder Tabs). No GPUI, no async.

pub mod config;
pub mod model;
pub mod secret;
pub mod state;

pub use config::{
    check_permissions, default_config_path, load_config, load_config_str, write_private_file,
    Config, ConfigError, ConnectionConfig, ConnectionKind, LoadedConfig, PermissionWarning,
};
pub use model::{
    GroupColor, ModelError, ReconcileReport, Section, SidebarGroup, SidebarItem, SidebarTab,
    TargetInfo,
};
pub use secret::Secret;
pub use state::{
    default_state_path, match_tab, match_tabs, GroupEntry, LiveTab, State, StateError, TabEntry,
};

use std::path::PathBuf;

/// `~/.config/rb`, or `None` if `HOME` is unset.
pub(crate) fn default_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join("rb"))
}
