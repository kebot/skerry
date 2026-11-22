//! Where a Files pane's I/O goes (`ARCH-023`, `ARCH-037`): a session's Selkies file routes,
//! or a home of SealSkin's persistent storage for the Storage window. An enum, so the pane
//! has no async trait objects; the Selkies arms call exactly the `SelkiesClient` methods
//! the Files window always called.

use crate::sealskin::{SealSkinClient, StorageError, UploadError};
use crate::selkies::{self, Entry, Progress, SelkiesClient};
use crate::storage::{self, StoragePlace};
use reqwest::StatusCode;
use std::path::Path;

/// A storage folder download walks at most this many entries.
pub(super) const WALK_MAX: usize = 100_000;

#[derive(Clone)]
pub(super) enum FilesBackend {
    Selkies(SelkiesClient),
    Storage(StorageBackend),
}

/// One home of the user's persistent storage (or Shared Files).
#[derive(Clone)]
pub(super) struct StorageBackend {
    pub client: SealSkinClient,
    pub place: StoragePlace,
}

#[derive(Debug)]
pub(super) enum BackendError {
    Selkies(selkies::Error),
    Storage(StorageError),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Selkies(error) => write!(f, "{error}"),
            Self::Storage(error) => write!(f, "{error}"),
        }
    }
}

impl From<selkies::Error> for BackendError {
    fn from(error: selkies::Error) -> Self {
        Self::Selkies(error)
    }
}

impl From<StorageError> for BackendError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<crate::sealskin::ApiError> for BackendError {
    fn from(error: crate::sealskin::ApiError) -> Self {
        Self::Storage(StorageError::Api(error))
    }
}

impl BackendError {
    /// A Selkies route's own 403, which says the session doesn't allow that direction.
    /// Never for storage: a storage 403 is the quota, storage turned off, or a refused key,
    /// and must not change the pane's policy (`ARCH-037`).
    pub(super) fn forbidden(&self) -> bool {
        matches!(
            self,
            Self::Selkies(selkies::Error::Server(StatusCode::FORBIDDEN, _))
        )
    }

    /// The request was lost on the way: a retry may work.
    pub(super) fn is_network(&self) -> bool {
        matches!(
            self,
            Self::Selkies(selkies::Error::Network(_)) | Self::Storage(StorageError::Network(_))
        )
    }
}

/// A folder as the pane shows it.
pub(super) struct Browsed {
    pub entries: Vec<Entry>,
    /// The folder holds more than the listing cap (storage only).
    pub truncated: bool,
}

impl FilesBackend {
    pub(super) fn storage(&self) -> Option<&StorageBackend> {
        match self {
            Self::Storage(storage) => Some(storage),
            Self::Selkies(_) => None,
        }
    }

    /// The storage place shown, if this is the Storage window's pane.
    pub(super) fn place(&self) -> Option<&StoragePlace> {
        self.storage().map(|storage| &storage.place)
    }

    /// What keys an upload's refresh: the storage home, or nothing for a session.
    pub(super) fn home_key(&self) -> Option<String> {
        self.place().map(|place| place.home().to_string())
    }

    pub(super) async fn browse(&self, dir: &str) -> Result<Browsed, BackendError> {
        match self {
            Self::Selkies(access) => Ok(Browsed {
                entries: access.browse(dir).await?.entries,
                truncated: false,
            }),
            Self::Storage(storage) => {
                let listing = storage
                    .client
                    .list_storage(storage.place.home(), &storage::home_path(dir))
                    .await?;
                Ok(Browsed {
                    entries: listing
                        .entries
                        .into_iter()
                        .map(|item| storage::entry_from(item, dir, &chrono::Local))
                        .collect(),
                    truncated: listing.truncated,
                })
            }
        }
    }

    pub(super) async fn mkdir(&self, path: &str) -> Result<(), BackendError> {
        match self {
            Self::Selkies(access) => Ok(access.mkdir(path).await?),
            Self::Storage(storage) => {
                let path = path.trim_matches('/');
                let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
                storage
                    .client
                    .create_storage_folder(storage.place.home(), &storage::home_path(parent), name)
                    .await?;
                Ok(())
            }
        }
    }

    /// Uploads `local` to the pane path `remote` (its folder and name).
    pub(super) async fn upload(
        &self,
        local: &Path,
        remote: &str,
        progress: &Progress,
    ) -> Result<(), BackendError> {
        match self {
            Self::Selkies(access) => Ok(access.upload(local, remote, progress).await?),
            Self::Storage(storage) => {
                let remote = remote.trim_matches('/');
                let (parent, name) = remote.rsplit_once('/').unwrap_or(("", remote));
                storage
                    .client
                    .upload_to_storage(
                        storage.place.home(),
                        &storage::home_path(parent),
                        local,
                        name,
                        progress,
                    )
                    .await
                    .map_err(|error| {
                        BackendError::Storage(match error {
                            UploadError::Refused(error) => StorageError::Api(error),
                            UploadError::Failed(message) => StorageError::Invalid(message),
                        })
                    })
            }
        }
    }

    /// Downloads the pane path `remote` to `local` (resuming a partial). `expected` is the
    /// listing's size, checked by storage downloads.
    pub(super) async fn download(
        &self,
        remote: &str,
        local: &Path,
        progress: &Progress,
        expected: Option<u64>,
    ) -> Result<(), BackendError> {
        match self {
            Self::Selkies(access) => Ok(access.download(remote, local, progress).await?),
            Self::Storage(storage) => Ok(storage
                .client
                .download_storage(
                    storage.place.home(),
                    &storage::home_path(remote),
                    local,
                    expected,
                    progress,
                )
                .await?),
        }
    }

    /// Everything below the folder `dir`, with each entry's local relative path built from
    /// checked names only.
    pub(super) async fn walk(&self, dir: &str) -> Result<Vec<(Entry, String)>, BackendError> {
        match self {
            Self::Selkies(access) => Ok(access.walk(dir).await?),
            Self::Storage(_) => {
                let mut found = Vec::new();
                let mut pending = vec![(dir.trim_matches('/').to_owned(), String::new())];
                while let Some((remote, relative)) = pending.pop() {
                    for entry in self.browse(&remote).await?.entries {
                        let name = selkies::local_name(&entry.name)
                            .ok_or_else(|| selkies::Error::UnsafeName(entry.name.clone()))?
                            .to_owned();
                        let relative = if relative.is_empty() {
                            name
                        } else {
                            format!("{relative}/{name}")
                        };
                        if entry.is_dir {
                            pending.push((entry.path.clone(), relative.clone()));
                        }
                        found.push((entry, relative));
                        if found.len() > WALK_MAX {
                            return Err(BackendError::Storage(StorageError::Invalid(
                                "That folder holds too many items to download at once.".into(),
                            )));
                        }
                    }
                }
                Ok(found)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage_backend(base: reqwest::Url, place: StoragePlace) -> FilesBackend {
        FilesBackend::Storage(StorageBackend {
            client: crate::sealskin::fake::client(base),
            place,
        })
    }

    #[tokio::test]
    async fn storage_backend_maps_root_and_entry_paths() {
        use crate::sealskin::fake;
        let (base, seen) = fake::serve(|target| {
            if target.contains("path=%2FDocs") {
                fake::reply(
                    "200 OK",
                    r#"{"items":[{"name":"a.txt","path":"/x/y","is_dir":false,"size":3,"mtime":0}],"total":1}"#,
                )
            } else {
                fake::reply(
                    "200 OK",
                    r#"{"items":[{"name":"Docs","is_dir":true,"size":4096,"mtime":0}],"total":1}"#,
                )
            }
        })
        .await;
        let backend = storage_backend(base, StoragePlace::Home("work".into()));
        let root = backend.browse("").await.unwrap();
        assert_eq!(root.entries[0].path, "Docs");
        assert_eq!(root.entries[0].size, None);
        let docs = backend.browse("Docs").await.unwrap();
        assert_eq!(docs.entries[0].path, "Docs/a.txt");
        let seen = seen.lock().unwrap().clone();
        assert!(seen[0].starts_with("get /companion/api/files/list/work?path=%2f&"));
        assert!(seen[1].starts_with("get /companion/api/files/list/work?path=%2fdocs&"));
        assert_eq!(backend.home_key().as_deref(), Some("work"));
    }

    #[tokio::test]
    async fn storage_walk_refuses_unsafe_names() {
        use crate::sealskin::fake;
        let (base, _) = fake::serve(|_| {
            fake::reply(
                "200 OK",
                r#"{"items":[{"name":"..","is_dir":false,"size":1,"mtime":0}],"total":1}"#,
            )
        })
        .await;
        let backend = storage_backend(base, StoragePlace::Shared);
        let error = backend.walk("Docs").await.unwrap_err();
        assert!(matches!(
            error,
            BackendError::Selkies(selkies::Error::UnsafeName(_))
        ));
    }

    #[test]
    fn storage_errors_never_turn_off_a_direction() {
        let quota = BackendError::Storage(StorageError::Api(crate::sealskin::ApiError::Forbidden(
            "Storage limit reached.".into(),
        )));
        assert!(!quota.forbidden());
        let selkies =
            BackendError::Selkies(selkies::Error::Server(StatusCode::FORBIDDEN, String::new()));
        assert!(selkies.forbidden());
        assert!(BackendError::Storage(StorageError::Network("x".into())).is_network());
    }
}
