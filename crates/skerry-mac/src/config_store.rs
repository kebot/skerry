//! `~/.config/skerry/config.toml`: the one file that holds secrets (`ARCH-011`, amended by
//! `ARCH-036`). It keeps the SealSkin `application_key`, the direct Selkies servers
//! (`[[selkies]]`) and `start_with`. Every mutation goes through [`edit_config`], and only
//! `persist`'s write queue calls the blocking writers built on it.

use std::io;
use std::path::{Path, PathBuf};
use toml_edit::DocumentMut;

pub(crate) const CONFIG_DISPLAY: &str = "~/.config/skerry/config.toml";
const START_WITH: &str = "start_with";

pub(crate) fn config_file() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .ok_or("Skerry can’t find your home folder to keep its settings.")?;
    Ok(config_file_in(Path::new(&home)))
}

pub(crate) fn config_file_in(home: &Path) -> PathBuf {
    home.join(".config/skerry/config.toml")
}

/// The parsed file, `None` when it doesn't exist. Invalid TOML is `InvalidData`.
pub(crate) fn read_config(path: &Path) -> io::Result<Option<DocumentMut>> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse()
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Runs `edit` on the file's document (an empty one when the file doesn't exist) and writes
/// the result only when `edit` reports a change, keeping every other setting and comment.
/// The folder is private to the user (0700) and the file is replaced atomically by a
/// temporary file created 0600 before any secret bytes are written.
pub(crate) fn edit_config(path: &Path, edit: impl FnOnce(&mut DocumentMut) -> bool) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    let mut document = read_config(path)?.unwrap_or_default();
    if !edit(&mut document) {
        return Ok(());
    }
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("no config folder"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let mut file = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o600))
        .tempfile_in(dir)?;
    file.write_all(document.to_string().as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    // The rename is durable only once the folder is synced.
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

/// Which kind of server startup reconnects to: written on each successful Connect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartWith {
    SealSkin,
    Selkies,
}

impl StartWith {
    fn text(self) -> &'static str {
        match self {
            Self::SealSkin => "sealskin",
            Self::Selkies => "selkies",
        }
    }
}

/// `start_with` in the document; anything unrecognised reads as unset.
pub(crate) fn start_with_in(document: &DocumentMut) -> Option<StartWith> {
    match document.get(START_WITH)?.as_str()? {
        "sealskin" => Some(StartWith::SealSkin),
        "selkies" => Some(StartWith::Selkies),
        _ => None,
    }
}

/// Sets (`Some`) or removes (`None`) `start_with`; reports whether the document changed.
pub(crate) fn set_start_with_in(document: &mut DocumentMut, value: Option<StartWith>) -> bool {
    match value {
        Some(value) if start_with_in(document) == Some(value) => false,
        Some(value) => {
            document[START_WITH] = toml_edit::value(value.text());
            true
        }
        None => document.remove(START_WITH).is_some(),
    }
}

/// `start_with` in the file at `path`. A small synchronous read.
pub(crate) fn start_with(path: &Path) -> Result<Option<StartWith>, String> {
    read_config(path)
        .map(|document| document.as_ref().and_then(start_with_in))
        .map_err(|_| format!("Couldn’t read {CONFIG_DISPLAY}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edit_that_changes_nothing_writes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let path = config_file_in(home.path());
        edit_config(&path, |_| false).unwrap();
        assert!(!path.exists());
        edit_config(&path, |doc| set_start_with_in(doc, Some(StartWith::Selkies))).unwrap();
        assert_eq!(start_with(&path).unwrap(), Some(StartWith::Selkies));
        let before = std::fs::read_to_string(&path).unwrap();
        edit_config(&path, |doc| set_start_with_in(doc, Some(StartWith::Selkies))).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        edit_config(&path, |doc| set_start_with_in(doc, None)).unwrap();
        assert_eq!(start_with(&path).unwrap(), None);
    }
}
