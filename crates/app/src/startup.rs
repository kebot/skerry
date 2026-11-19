//! Startup: find and load `config.toml`, pick the Connection to use.
//!
//! Never prints tokens: store types redact them, and TOML parse errors are
//! cut to their first line (the rest quotes the offending source line).

use std::path::{Path, PathBuf};

use rb_store::{Config, ConfigError, ConnectionConfig};

use crate::model::Overlay;

/// A minimal config for a CloakBrowser Manager.
pub const EXAMPLE_CONFIG: &str = r#"[[connection]]
name = "work"
manager_url = "https://manager.example.com"
profile_id = "your-profile-id"
token = "your-manager-token"
"#;

/// What `main` hands to the app.
#[derive(Debug)]
pub enum Startup {
    /// All Connections of the config.
    Ready {
        /// In config order (that order picks shapes and automatic colours).
        connections: Vec<ConnectionConfig>,
        /// The Connection named by `--connection` / `RB_CONNECTION`: it
        /// connects first and is selected. `None`: no preference.
        selected: Option<usize>,
        /// File permission warnings and similar, shown in the status bar.
        warnings: Vec<String>,
    },
    /// Nothing to connect to; show this instead.
    Problem(Overlay),
}

/// The Connection name from `--connection <name>` / `--connection=<name>`,
/// else the `RB_CONNECTION` value.
pub fn connection_selector(
    args: impl IntoIterator<Item = String>,
    env: Option<String>,
) -> Option<String> {
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--connection" {
            return it.next();
        }
        if let Some(v) = a.strip_prefix("--connection=") {
            return Some(v.to_string());
        }
    }
    env.filter(|s| !s.is_empty())
}

/// The named Connection, or the first one.
pub fn select_connection<'a>(
    config: &'a Config,
    name: Option<&str>,
) -> Result<&'a ConnectionConfig, String> {
    match name {
        Some(n) => config.connection(n).ok_or_else(|| {
            let names: Vec<&str> = config.connections.iter().map(|c| c.name.as_str()).collect();
            format!(
                "No connection named \"{n}\". Available: {}.",
                names.join(", ")
            )
        }),
        None => config
            .connections
            .first()
            .ok_or_else(|| "config.toml has no [[connection]] entries.".to_string()),
    }
}

fn first_run(path: &Path) -> Overlay {
    Overlay::new("Welcome to rb")
        .detail(format!(
            "No config file found. Create {} (mode 0600, it holds tokens) with a Connection, for example:\n\n{}\nSee docs/rb/running.md.",
            path.display(),
            EXAMPLE_CONFIG
        ))
}

fn error_text(e: &ConfigError) -> String {
    match e {
        // TOML errors quote the source line, which may hold a token.
        ConfigError::Parse { message } => format!(
            "invalid config.toml: {}",
            message.lines().next().unwrap_or("parse error")
        ),
        other => other.to_string(),
    }
}

/// Load the config at `path` (`None`: no home directory). `selector` names
/// the Connection to open first; it is optional.
pub fn load(path: Option<PathBuf>, selector: Option<&str>) -> Startup {
    let Some(path) = path else {
        return Startup::Problem(
            Overlay::new("Cannot find the config directory")
                .detail("HOME is not set. Set RB_CONFIG to the path of config.toml."),
        );
    };
    let loaded = match rb_store::load_config(&path) {
        Ok(l) => l,
        Err(ConfigError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            return Startup::Problem(first_run(&path));
        }
        Err(e) => {
            return Startup::Problem(Overlay::new("Cannot read config.toml").detail(format!(
                "{}\n\nFile: {}",
                error_text(&e),
                path.display()
            )));
        }
    };
    if loaded.config.connections.is_empty() {
        return Startup::Problem(first_run(&path));
    }
    let selected = match selector {
        None => None,
        Some(_) => match select_connection(&loaded.config, selector) {
            Ok(c) => loaded
                .config
                .connections
                .iter()
                .position(|x| x.name == c.name),
            Err(msg) => {
                return Startup::Problem(Overlay::new("Cannot choose a Connection").detail(msg))
            }
        },
    };
    Startup::Ready {
        connections: loaded.config.connections.clone(),
        selected,
        warnings: loaded.warnings.iter().map(|w| w.to_string()).collect(),
    }
}

/// [`load`] with the default path (`RB_CONFIG` or `~/.config/rb/config.toml`).
pub fn load_default(selector: Option<&str>) -> Startup {
    load(rb_store::default_config_path(), selector)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rb_store::{load_config_str, ConnectionKind};

    fn cfg() -> Config {
        load_config_str(
            r#"
[[connection]]
name = "a"
cdp_url = "ws://127.0.0.1:9222/"
[[connection]]
name = "b"
manager_url = "http://m"
profile_id = "p1"
token = "SECRET-TOKEN"
"#,
        )
        .unwrap()
    }

    #[test]
    fn first_connection_is_the_default() {
        assert_eq!(select_connection(&cfg(), None).unwrap().name, "a");
    }

    #[test]
    fn named_connection_and_unknown_name() {
        let c = cfg();
        let b = select_connection(&c, Some("b")).unwrap();
        assert!(matches!(b.kind, ConnectionKind::Manager { .. }));
        let e = select_connection(&c, Some("zzz")).unwrap_err();
        assert!(e.contains("zzz") && e.contains("a, b"), "{e}");
    }

    #[test]
    fn selector_from_args_beats_env() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            connection_selector(args(&["--connection", "x"]), Some("env".into())).as_deref(),
            Some("x")
        );
        assert_eq!(
            connection_selector(args(&["--connection=y"]), None).as_deref(),
            Some("y")
        );
        assert_eq!(
            connection_selector(args(&[]), Some("env".into())).as_deref(),
            Some("env")
        );
        assert_eq!(connection_selector(args(&[]), Some(String::new())), None);
    }

    #[test]
    fn selector_is_optional_and_picks_an_index() {
        let dir = std::env::temp_dir().join(format!("rb-startup-sel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        rb_store::write_private_file(
            &path,
            "[[connection]]\nname=\"a\"\ncdp_url=\"ws://h/\"\n[[connection]]\nname=\"b\"\ncdp_url=\"ws://h2/\"\n",
        )
        .unwrap();
        let Startup::Ready {
            connections,
            selected,
            ..
        } = load(Some(path.clone()), Some("b"))
        else {
            panic!("expected ready");
        };
        assert_eq!((connections.len(), selected), (2, Some(1)));
        assert!(matches!(
            load(Some(path.clone()), Some("zz")),
            Startup::Problem(_)
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file_is_a_first_run_screen() {
        let p = std::env::temp_dir().join("rb-no-such-dir-xyz/config.toml");
        let Startup::Problem(o) = load(Some(p.clone()), None) else {
            panic!("expected a problem");
        };
        assert_eq!(o.title, "Welcome to rb");
        let d = o.detail.unwrap();
        assert!(d.contains(&p.display().to_string()) && d.contains("manager_url"));
    }

    #[test]
    fn ready_from_a_real_file_and_no_token_in_problems() {
        let dir = std::env::temp_dir().join(format!("rb-startup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        rb_store::write_private_file(
            &path,
            "[[connection]]\nname=\"n\"\ncdp_url=\"ws://h/\"\ntoken=\"SECRET-TOKEN\"\n",
        )
        .unwrap();
        let Startup::Ready {
            connections,
            selected,
            warnings,
        } = load(Some(path.clone()), None)
        else {
            panic!("expected ready");
        };
        let connection = &connections[0];
        assert_eq!(connection.name, "n");
        assert_eq!(selected, None);
        assert!(warnings.is_empty());
        assert!(!format!("{connection:?}").contains("SECRET-TOKEN"));

        // A broken file: the message must not quote the source line.
        std::fs::write(&path, "[[connection]]\ntoken = \"SECRET-TOKEN\" oops\n").unwrap();
        let Startup::Problem(o) = load(Some(path), None) else {
            panic!("expected a problem");
        };
        let text = format!("{} {}", o.title, o.detail.unwrap());
        assert!(!text.contains("SECRET-TOKEN"), "{text}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
