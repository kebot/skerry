//! `config.toml`: the list of Connections.

use crate::secret::Secret;
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

/// How a Connection reaches its Remote Browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionKind {
    /// A Manager plus Profile.
    Manager {
        manager_url: String,
        profile_id: String,
        token: Secret,
    },
    /// A raw CDP URL, with an optional Bearer token.
    Cdp {
        cdp_url: String,
        token: Option<Secret>,
    },
}

/// One validated `[[connection]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionConfig {
    /// Unique name; also the key used by `state.toml`.
    pub name: String,
    /// Connection colour such as `#3b82f6`.
    pub color: Option<String>,
    /// Connect at startup (default false).
    pub auto_connect: bool,
    /// Run the local Agent CDP proxy for this Connection (default false:
    /// nothing listens unless enabled).
    pub agent_proxy: bool,
    /// Fixed 127.0.0.1 port for the Agent proxy; 0 picks an ephemeral port.
    pub agent_proxy_port: u16,
    /// Path secret for the Agent endpoint. `None`: the app generates a random
    /// one each run (never written to the config).
    pub agent_proxy_secret: Option<Secret>,
    pub kind: ConnectionKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub connections: Vec<ConnectionConfig>,
}

impl Config {
    pub fn connection(&self, name: &str) -> Option<&ConnectionConfig> {
        self.connections.iter().find(|c| c.name == name)
    }
}

/// Non-fatal: the config file is readable by group or others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionWarning {
    pub path: PathBuf,
    /// Permission bits (`mode & 0o777`).
    pub mode: u32,
}

impl fmt::Display for PermissionWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} has mode {:04o}; it holds tokens, run `chmod 600` on it",
            self.path.display(),
            self.mode
        )
    }
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: Config,
    pub warnings: Vec<PermissionWarning>,
}

#[derive(Debug)]
pub enum ConfigError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        message: String,
    },
    /// A problem in one `[[connection]]` entry (`name` is the entry's name, or
    /// `#N` when it has none).
    Invalid {
        name: String,
        message: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            }
            ConfigError::Parse { message } => write!(f, "invalid config.toml: {message}"),
            ConfigError::Invalid { name, message } => {
                write!(f, "connection \"{name}\": {message}")
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct RawConfig {
    #[serde(default)]
    connection: Vec<RawConnection>,
}

// Unknown keys are rejected so typos (e.g. `autoconnect`) are not silently ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConnection {
    name: Option<String>,
    manager_url: Option<String>,
    profile_id: Option<String>,
    cdp_url: Option<String>,
    token: Option<Secret>,
    color: Option<String>,
    #[serde(default)]
    auto_connect: bool,
    #[serde(default)]
    agent_proxy: bool,
    agent_proxy_port: Option<i64>,
    agent_proxy_secret: Option<Secret>,
}

/// Default config path: `$RB_CONFIG` if set, else `~/.config/rb/config.toml`.
pub fn default_config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RB_CONFIG") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    crate::default_dir().map(|d| d.join("config.toml"))
}

/// Load and validate a config file. A missing file is an error (a Connection
/// list is required); a too-open file mode is only a warning.
pub fn load_config(path: &Path) -> Result<LoadedConfig, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let config = load_config_str(&text)?;
    let warnings = check_permissions(path).into_iter().collect();
    Ok(LoadedConfig { config, warnings })
}

/// Parse and validate config text.
pub fn load_config_str(text: &str) -> Result<Config, ConfigError> {
    let raw: RawConfig = toml::from_str(text).map_err(|e| ConfigError::Parse {
        message: e.to_string(),
    })?;
    let mut seen = HashSet::new();
    let mut connections = Vec::new();
    for (i, entry) in raw.connection.into_iter().enumerate() {
        let label = match entry.name.as_deref() {
            Some(n) if !n.trim().is_empty() => n.to_string(),
            _ => format!("#{}", i + 1),
        };
        let conn = validate(entry, &label)?;
        if !seen.insert(conn.name.clone()) {
            return Err(invalid(&conn.name, "duplicate connection name"));
        }
        connections.push(conn);
    }
    Ok(Config { connections })
}

fn invalid(name: &str, message: &str) -> ConfigError {
    ConfigError::Invalid {
        name: name.to_string(),
        message: message.to_string(),
    }
}

fn validate(raw: RawConnection, label: &str) -> Result<ConnectionConfig, ConfigError> {
    let name = match raw.name {
        Some(n) if !n.trim().is_empty() => n,
        _ => return Err(invalid(label, "missing `name`")),
    };
    if let Some(c) = &raw.color {
        if !is_hex_color(c) {
            return Err(invalid(&name, "`color` must look like #rrggbb"));
        }
    }
    let agent_proxy_port = match raw.agent_proxy_port {
        None => 0,
        Some(n) => {
            u16::try_from(n).map_err(|_| invalid(&name, "`agent_proxy_port` must be 0..=65535"))?
        }
    };
    if let Some(s) = &raw.agent_proxy_secret {
        let v = s.expose();
        if v.len() < 8
            || !v
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(invalid(
                &name,
                "`agent_proxy_secret` must be at least 8 characters of A-Z a-z 0-9 - _",
            ));
        }
    }
    let kind =
        match (raw.manager_url, raw.cdp_url) {
            (Some(_), Some(_)) => return Err(invalid(
                &name,
                "set either `manager_url` (with `profile_id` and `token`) or `cdp_url`, not both",
            )),
            (None, None) => {
                return Err(invalid(
                    &name,
                    "needs `manager_url` (with `profile_id` and `token`) or `cdp_url`",
                ))
            }
            (Some(manager_url), None) => {
                check_url(&name, "manager_url", &manager_url, &["http", "https"])?;
                let profile_id = match raw.profile_id {
                    Some(p) if !p.trim().is_empty() => p,
                    _ => return Err(invalid(&name, "Manager connection needs `profile_id`")),
                };
                let token = match raw.token {
                    Some(t) => t,
                    None => return Err(invalid(&name, "Manager connection needs `token`")),
                };
                ConnectionKind::Manager {
                    manager_url,
                    profile_id,
                    token,
                }
            }
            (None, Some(cdp_url)) => {
                check_url(&name, "cdp_url", &cdp_url, &["ws", "wss", "http", "https"])?;
                if raw.profile_id.is_some() {
                    return Err(invalid(&name, "`profile_id` only applies to `manager_url`"));
                }
                ConnectionKind::Cdp {
                    cdp_url,
                    token: raw.token,
                }
            }
        };
    Ok(ConnectionConfig {
        name,
        color: raw.color,
        auto_connect: raw.auto_connect,
        agent_proxy: raw.agent_proxy,
        agent_proxy_port,
        agent_proxy_secret: raw.agent_proxy_secret,
        kind,
    })
}

fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Minimal URL sanity check: allowed scheme, non-empty host, no whitespace.
fn check_url(name: &str, field: &str, url: &str, schemes: &[&str]) -> Result<(), ConfigError> {
    let bad = |why: &str| invalid(name, &format!("`{field}` is not a valid URL: {why}"));
    let (scheme, rest) = url.split_once("://").ok_or_else(|| bad("missing scheme"))?;
    if !schemes.contains(&scheme.to_ascii_lowercase().as_str()) {
        return Err(bad(&format!(
            "scheme must be one of {}",
            schemes.join(", ")
        )));
    }
    if url.chars().any(char::is_whitespace) {
        return Err(bad("contains whitespace"));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    if host.is_empty() || host.starts_with(':') {
        return Err(bad("missing host"));
    }
    Ok(())
}

/// On Unix, warn if the file is accessible by group or others (looser than
/// 0600). Returns `None` elsewhere, or if the file cannot be inspected.
pub fn check_permissions(path: &Path) -> Option<PermissionWarning> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Some(PermissionWarning {
                path: path.to_path_buf(),
                mode,
            });
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    None
}

/// Create (or truncate) `path` with mode 0600 from the start (no window where
/// it is world-readable), creating parent directories, and write `contents`.
pub fn write_private_file(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    #[cfg(unix)]
    {
        // An existing file keeps its old mode; tighten it.
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(contents.as_bytes())?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r##"
[[connection]]
name = "work"
manager_url = "https://mgr.example.com"
profile_id = "abc"
token = "s3cr3t-token"
color = "#3b82f6"
auto_connect = true

[[connection]]
name = "raw"
cdp_url = "ws://10.0.0.5:9222/devtools/browser/x"
"##;

    #[test]
    fn agent_proxy_defaults_off() {
        let c = load_config_str(GOOD).unwrap();
        for conn in &c.connections {
            assert!(!conn.agent_proxy);
            assert_eq!(conn.agent_proxy_port, 0);
            assert!(conn.agent_proxy_secret.is_none());
        }
    }

    #[test]
    fn agent_proxy_keys_parse_and_secret_is_redacted() {
        let c = load_config_str(
            r#"
[[connection]]
name = "a"
cdp_url = "ws://h:1/x"
agent_proxy = true
agent_proxy_port = 9333
agent_proxy_secret = "abcDEF-123_xyz"
"#,
        )
        .unwrap();
        let a = &c.connections[0];
        assert!(a.agent_proxy);
        assert_eq!(a.agent_proxy_port, 9333);
        assert_eq!(
            a.agent_proxy_secret.as_ref().unwrap().expose(),
            "abcDEF-123_xyz"
        );
        assert!(!format!("{a:?}").contains("abcDEF"));
    }

    #[test]
    fn agent_proxy_validation() {
        let base = "[[connection]]\nname = \"a\"\ncdp_url = \"ws://h:1/x\"\n";
        assert!(err(&format!("{base}agent_proxy_port = 70000\n")).contains("agent_proxy_port"));
        assert!(err(&format!("{base}agent_proxy_port = -1\n")).contains("agent_proxy_port"));
        assert!(
            err(&format!("{base}agent_proxy_secret = \"short\"\n")).contains("agent_proxy_secret")
        );
        assert!(
            err(&format!("{base}agent_proxy_secret = \"has/slash12\"\n"))
                .contains("agent_proxy_secret")
        );
        // Typos are still rejected.
        assert!(err(&format!("{base}agent_proxi = true\n")).contains("agent_proxi"));
        // Wrong type.
        assert!(load_config_str(&format!("{base}agent_proxy = \"yes\"\n")).is_err());
    }

    fn err(text: &str) -> String {
        load_config_str(text).unwrap_err().to_string()
    }

    #[test]
    fn parses_both_shapes() {
        let c = load_config_str(GOOD).unwrap();
        assert_eq!(c.connections.len(), 2);
        let work = c.connection("work").unwrap();
        assert!(work.auto_connect);
        assert_eq!(work.color.as_deref(), Some("#3b82f6"));
        match &work.kind {
            ConnectionKind::Manager {
                profile_id, token, ..
            } => {
                assert_eq!(profile_id, "abc");
                assert_eq!(token.expose(), "s3cr3t-token");
            }
            _ => panic!("expected Manager"),
        }
        let raw = c.connection("raw").unwrap();
        assert!(!raw.auto_connect);
        assert!(matches!(&raw.kind, ConnectionKind::Cdp { token: None, .. }));
    }

    #[test]
    fn cdp_with_token() {
        let c = load_config_str("[[connection]]\nname=\"a\"\ncdp_url=\"wss://h/x\"\ntoken=\"t\"\n")
            .unwrap();
        assert!(matches!(
            &c.connections[0].kind,
            ConnectionKind::Cdp { token: Some(_), .. }
        ));
    }

    #[test]
    fn empty_config_is_ok() {
        assert!(load_config_str("").unwrap().connections.is_empty());
    }

    #[test]
    fn rejects_both_shapes() {
        let e = err("[[connection]]\nname=\"x\"\nmanager_url=\"http://a\"\ncdp_url=\"ws://a\"\n");
        assert!(e.contains("\"x\"") && e.contains("not both"), "{e}");
    }

    #[test]
    fn rejects_neither_shape() {
        let e = err("[[connection]]\nname=\"x\"\n");
        assert!(e.contains("\"x\"") && e.contains("needs"), "{e}");
    }

    #[test]
    fn manager_needs_profile_and_token() {
        let e = err("[[connection]]\nname=\"m\"\nmanager_url=\"http://a\"\ntoken=\"t\"\n");
        assert!(e.contains("\"m\"") && e.contains("profile_id"), "{e}");
        let e = err("[[connection]]\nname=\"m\"\nmanager_url=\"http://a\"\nprofile_id=\"p\"\n");
        assert!(e.contains("\"m\"") && e.contains("token"), "{e}");
    }

    #[test]
    fn rejects_duplicate_names_and_missing_name() {
        let e = err("[[connection]]\nname=\"a\"\ncdp_url=\"ws://h\"\n[[connection]]\nname=\"a\"\ncdp_url=\"ws://h2\"\n");
        assert!(e.contains("\"a\"") && e.contains("duplicate"), "{e}");
        let e = err("[[connection]]\ncdp_url=\"ws://h\"\n");
        assert!(e.contains("#1") && e.contains("name"), "{e}");
    }

    #[test]
    fn rejects_bad_urls_and_colors() {
        for url in [
            "localhost:9222",
            "ftp://h",
            "ws://",
            "ws:///path",
            "ws://a b",
        ] {
            let e = err(&format!("[[connection]]\nname=\"u\"\ncdp_url=\"{url}\"\n"));
            assert!(e.contains("\"u\"") && e.contains("cdp_url"), "{url}: {e}");
        }
        let e = err("[[connection]]\nname=\"c\"\ncdp_url=\"ws://h\"\ncolor=\"blue\"\n");
        assert!(e.contains("\"c\"") && e.contains("color"), "{e}");
    }

    #[test]
    fn rejects_unknown_key_and_bad_toml() {
        let e = err("[[connection]]\nname=\"a\"\ncdp_url=\"ws://h\"\nautoconnect=true\n");
        assert!(e.contains("autoconnect"), "{e}");
        assert!(err("[[connection").contains("invalid config.toml"));
    }

    #[test]
    fn tokens_never_appear_in_debug_or_display() {
        let c = load_config_str(GOOD).unwrap();
        let dbg = format!("{c:?} {:#?}", c.connections[0]);
        assert!(!dbg.contains("s3cr3t-token"), "{dbg}");
        assert!(dbg.contains("redacted"));
        let s = Secret::new("hunter2");
        assert!(!format!("{s:?}{s}").contains("hunter2"));
        // Errors never include tokens either.
        let e = err("[[connection]]\nname=\"m\"\nmanager_url=\"bad\"\nprofile_id=\"p\"\ntoken=\"hunter2\"\n");
        assert!(!e.contains("hunter2"));
    }

    #[cfg(unix)]
    #[test]
    fn permission_warning_and_private_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        write_private_file(&path, GOOD).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let loaded = load_config(&path).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.config.connections.len(), 2);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let loaded = load_config(&path).unwrap();
        assert_eq!(loaded.warnings.len(), 1);
        assert_eq!(loaded.warnings[0].mode, 0o644);
        assert!(loaded.warnings[0].to_string().contains("644"));

        // Rewriting an existing loose file tightens it.
        write_private_file(&path, GOOD).unwrap();
        assert!(check_permissions(&path).is_none());
    }

    #[test]
    fn missing_file_is_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let e = load_config(&dir.path().join("nope.toml")).unwrap_err();
        assert!(matches!(e, ConfigError::Io { .. }));
    }

    #[test]
    fn rb_config_env_override() {
        // Single test touching the env var, to avoid races with other tests.
        std::env::set_var("RB_CONFIG", "/tmp/x/rb.toml");
        assert_eq!(default_config_path(), Some(PathBuf::from("/tmp/x/rb.toml")));
        std::env::remove_var("RB_CONFIG");
        if std::env::var_os("HOME").is_some() {
            assert!(default_config_path()
                .unwrap()
                .ends_with(".config/rb/config.toml"));
        }
    }
}
