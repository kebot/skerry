//! The one writer of Skerry's files: preferences, and config.toml (the saved application
//! key, the direct Selkies servers and `start_with`, `ARCH-011`/`ARCH-036`).
//!
//! Every write is a job on one FIFO queue. A single consumer task on the Tokio runtime runs
//! each job with `spawn_blocking`, one at a time, so two read-modify-writes never interleave
//! and two quick changes land in the order they were made (`ARCH-022`'s re-read-then-write
//! stays correct). GPUI code queues a job and gets the result back on a oneshot channel; it
//! never calls `settings::update_at` or the key writers itself.

use crate::direct::{self, SavedServer};
use crate::sealskin::{self, Credential};
use crate::settings::{self, Preferences};
use gpui_kit::Global;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};

/// A preference change. It runs twice: on the in-memory copy when queued (so reads see it
/// at once), and on the re-read file when its job runs.
type Change = Arc<dyn Fn(&mut Preferences) + Send + Sync>;

enum Job {
    Preferences {
        server: String,
        change: Change,
        done: oneshot::Sender<io::Result<()>>,
    },
    Config {
        write: ConfigWrite,
        done: oneshot::Sender<Result<(), String>>,
    },
    /// The saved Selkies servers, read after every write queued before it.
    Servers(oneshot::Sender<Result<Vec<SavedServer>, String>>),
    Flush(oneshot::Sender<()>),
}

/// A change to config.toml.
enum ConfigWrite {
    /// The application key, and `start_with = "sealskin"`.
    SaveKey(String),
    /// Remove the saved key if it is this one.
    ForgetKey(String),
    /// A direct server at the front of `[[selkies]]`, and `start_with = "selkies"`;
    /// `remember` keeps its password and token.
    SaveServer(Box<SavedServer>, bool),
    /// Remove a direct server's entry (and `start_with`).
    ForgetServer(String),
    /// Disconnect: the entry stays, startup no longer reconnects.
    ClearStartWith,
}

/// Where the files live: the real locations, or a test folder.
#[derive(Clone)]
enum Root {
    Home,
    #[cfg(test)]
    Under(PathBuf),
}

impl Root {
    fn preferences(&self, server: &str) -> io::Result<PathBuf> {
        match self {
            Self::Home => {
                settings::path(server).ok_or_else(|| io::Error::other("Home directory unavailable"))
            }
            #[cfg(test)]
            Self::Under(root) => Ok(root.join(format!("{server}.json"))),
        }
    }

    fn config(&self) -> Result<PathBuf, String> {
        match self {
            Self::Home => sealskin::config_file(),
            #[cfg(test)]
            Self::Under(root) => Ok(sealskin::config_file_in(root)),
        }
    }

    fn load(&self, server: &str) -> Preferences {
        match self {
            Self::Home => [settings::path(server), settings::legacy_path(server)]
                .into_iter()
                .flatten()
                .find(|path| path.exists())
                .map(|path| settings::load_at(&path))
                .unwrap_or_default(),
            #[cfg(test)]
            Self::Under(root) => settings::load_at(&root.join(format!("{server}.json"))),
        }
    }
}

impl Job {
    fn run(self, root: &Root) {
        match self {
            Self::Preferences {
                server,
                change,
                done,
            } => {
                let result = root
                    .preferences(&server)
                    .and_then(|path| settings::update_at(&path, |p| change(p)));
                let _ = done.send(result);
            }
            Self::Config { write, done } => {
                let result = root.config().and_then(|path| match write {
                    ConfigWrite::SaveKey(raw) => sealskin::save_key_at(&path, &raw),
                    ConfigWrite::ForgetKey(raw) => sealskin::forget_key_at(&path, &raw),
                    ConfigWrite::SaveServer(server, remember) => {
                        direct::save_server_at(&path, &server, remember)
                    }
                    ConfigWrite::ForgetServer(url) => direct::forget_server_at(&path, &url),
                    ConfigWrite::ClearStartWith => direct::clear_start_with_at(&path),
                });
                let _ = done.send(result);
            }
            Self::Servers(done) => {
                let _ = done.send(
                    root.config()
                        .and_then(|path| direct::saved_servers_at(&path)),
                );
            }
            Self::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

/// The write queue, a GPUI global set once in `app::run`.
#[derive(Clone)]
pub struct WriteQueue {
    tx: mpsc::UnboundedSender<Job>,
    rt: Handle,
    root: Root,
    /// Each server's preferences with every queued change applied: what reads return, so
    /// they never miss a write still in the queue.
    cache: Arc<Mutex<HashMap<String, Preferences>>>,
}

impl Global for WriteQueue {}

pub fn start(rt: &Handle) -> WriteQueue {
    start_at(rt, Root::Home)
}

fn start_at(rt: &Handle, root: Root) -> WriteQueue {
    let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
    let consumer_root = root.clone();
    rt.spawn(async move {
        while let Some(job) = rx.recv().await {
            let root = consumer_root.clone();
            let _ = tokio::task::spawn_blocking(move || job.run(&root)).await;
        }
    });
    WriteQueue {
        tx,
        rt: rt.clone(),
        root,
        cache: Arc::default(),
    }
}

impl WriteQueue {
    /// `server`'s preferences, including every change still queued. Reads the file only the
    /// first time a server is asked for.
    pub fn read(&self, server: &str) -> Preferences {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache
            .entry(server.to_string())
            .or_insert_with(|| self.root.load(server))
            .clone()
    }

    /// Applies `change` to `server`'s preferences now (for reads), then queues the write:
    /// re-read the file, apply `change`, save.
    pub fn preferences(
        &self,
        server: &str,
        change: impl Fn(&mut Preferences) + Send + Sync + 'static,
    ) -> oneshot::Receiver<io::Result<()>> {
        let change: Change = Arc::new(change);
        {
            let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
            change(
                cache
                    .entry(server.to_string())
                    .or_insert_with(|| self.root.load(server)),
            );
        }
        let (done, result) = oneshot::channel();
        self.send(Job::Preferences {
            server: server.to_string(),
            change,
            done,
        });
        result
    }

    pub fn save_key(&self, credential: &Credential) -> oneshot::Receiver<Result<(), String>> {
        self.config(ConfigWrite::SaveKey(credential.raw().to_string()))
    }

    /// Removes `application_key` from config.toml if it holds this key (`raw`).
    pub fn forget_key(&self, raw: &str) -> oneshot::Receiver<Result<(), String>> {
        self.config(ConfigWrite::ForgetKey(raw.to_string()))
    }

    /// Saves a direct server (`ARCH-036`); with `remember` off, without its secrets.
    pub fn save_server(
        &self,
        server: SavedServer,
        remember: bool,
    ) -> oneshot::Receiver<Result<(), String>> {
        self.config(ConfigWrite::SaveServer(Box::new(server), remember))
    }

    /// Removes a direct server's entry, its secrets and `start_with`.
    pub fn forget_server(&self, url: &str) -> oneshot::Receiver<Result<(), String>> {
        self.config(ConfigWrite::ForgetServer(url.to_string()))
    }

    /// Removes `start_with`, so startup reconnects to nothing.
    pub fn clear_start_with(&self) -> oneshot::Receiver<Result<(), String>> {
        self.config(ConfigWrite::ClearStartWith)
    }

    /// The saved direct servers, after every write queued before this read.
    pub fn saved_servers(&self) -> oneshot::Receiver<Result<Vec<SavedServer>, String>> {
        let (done, result) = oneshot::channel();
        self.send(Job::Servers(done));
        result
    }

    fn config(&self, write: ConfigWrite) -> oneshot::Receiver<Result<(), String>> {
        let (done, result) = oneshot::channel();
        self.send(Job::Config { write, done });
        result
    }

    /// Resolves once every write queued before it has finished.
    pub fn flush(&self) -> oneshot::Receiver<()> {
        let (done, result) = oneshot::channel();
        self.send(Job::Flush(done));
        result
    }

    /// Waits on the calling thread, at most `limit`, for every queued write. For app quit:
    /// GPUI gives `on_app_quit` futures only 200 ms, so the wait happens before returning
    /// one. The main thread isn't a Tokio worker, and the consumer never needs it.
    pub fn flush_blocking(&self, limit: Duration) {
        let pending = self.flush();
        let _ = self
            .rt
            .block_on(async move { tokio::time::timeout(limit, pending).await });
    }

    fn send(&self, job: Job) {
        // The consumer lives as long as the runtime; a send fails only during shutdown,
        // and then the dropped `done` sender reports the job as not done.
        let _ = self.tx.send(job);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::Codec;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap()
    }

    fn saved(root: &std::path::Path, server: &str) -> Preferences {
        let bytes = std::fs::read(root.join(format!("{server}.json"))).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn queued_preference_writes_apply_in_submission_order() {
        let rt = runtime();
        let dir = tempfile::tempdir().unwrap();
        let queue = start_at(rt.handle(), Root::Under(dir.path().into()));
        let first = queue.preferences("seal", |p| p.codec = Codec::H264);
        let second = queue.preferences("seal", |p| p.codec = Codec::Hevc);
        rt.block_on(async {
            first.await.unwrap().unwrap();
            second.await.unwrap().unwrap();
        });
        assert_eq!(saved(dir.path(), "seal").codec, Codec::Hevc);
    }

    #[test]
    fn concurrent_windows_keep_each_others_changes() {
        let rt = runtime();
        let dir = tempfile::tempdir().unwrap();
        let queue = start_at(rt.handle(), Root::Under(dir.path().into()));
        drop(queue.preferences("seal", |p| p.codec = Codec::Hevc));
        drop(queue.preferences("seal", |p| p.input.command_as_control = true));
        queue.flush_blocking(Duration::from_secs(5));
        let prefs = saved(dir.path(), "seal");
        assert_eq!(prefs.codec, Codec::Hevc);
        assert!(prefs.input.command_as_control);
    }

    #[test]
    fn flush_resolves_after_pending_writes() {
        let rt = runtime();
        let dir = tempfile::tempdir().unwrap();
        let queue = start_at(rt.handle(), Root::Under(dir.path().into()));
        for fps in 1..=20 {
            drop(queue.preferences("seal", move |p| {
                p.video.insert(settings::Key::Fps, fps);
            }));
        }
        queue.flush_blocking(Duration::from_secs(5));
        assert_eq!(
            saved(dir.path(), "seal").video.get(&settings::Key::Fps),
            Some(&20)
        );
    }

    #[test]
    fn sign_out_after_connect_leaves_no_key() {
        let rt = runtime();
        let dir = tempfile::tempdir().unwrap();
        let queue = start_at(rt.handle(), Root::Under(dir.path().into()));
        let credential = Credential::parse("ssk_id_secret@seal.example").unwrap();
        let saved = queue.save_key(&credential);
        let forgotten = queue.forget_key(credential.raw());
        rt.block_on(async {
            saved.await.unwrap().unwrap();
            forgotten.await.unwrap().unwrap();
        });
        let config = std::fs::read_to_string(sealskin::config_file_in(dir.path())).unwrap();
        assert!(!config.contains("application_key"), "{config}");
    }

    #[test]
    fn signing_out_leaves_a_different_saved_key_alone() {
        let rt = runtime();
        let dir = tempfile::tempdir().unwrap();
        let queue = start_at(rt.handle(), Root::Under(dir.path().into()));
        let saved = Credential::parse("ssk_id_secret@seal.example").unwrap();
        drop(queue.save_key(&saved));
        let forgotten = queue.forget_key("ssk_other_key@seal.example");
        rt.block_on(async { forgotten.await.unwrap().unwrap() });
        let config = std::fs::read_to_string(sealskin::config_file_in(dir.path())).unwrap();
        assert!(config.contains("ssk_id_secret"), "{config}");
    }

    #[test]
    fn disconnect_clears_start_with_and_forget_removes_the_entry() {
        let rt = runtime();
        let dir = tempfile::tempdir().unwrap();
        let queue = start_at(rt.handle(), Root::Under(dir.path().into()));
        let server = SavedServer {
            url: "https://h.example/".into(),
            username: Some("ubuntu".into()),
            password: Some("pw".into()),
            ..SavedServer::default()
        };
        drop(queue.save_server(server.clone(), true));
        drop(queue.clear_start_with());
        let read = queue.saved_servers();
        let path = sealskin::config_file_in(dir.path());
        let servers = rt.block_on(read).unwrap().unwrap();
        assert_eq!(servers, vec![server]);
        assert_eq!(crate::config_store::start_with(&path).unwrap(), None);
        let forgotten = queue.forget_server("https://h.example");
        rt.block_on(forgotten).unwrap().unwrap();
        assert!(rt
            .block_on(queue.saved_servers())
            .unwrap()
            .unwrap()
            .is_empty());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("pw"), "{text}");
    }

    #[test]
    fn reads_see_writes_still_in_the_queue() {
        let rt = runtime();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("seal.json"), r#"{"jpeg": 45}"#).unwrap();
        let queue = start_at(rt.handle(), Root::Under(dir.path().into()));
        // Nothing has been awaited: the job may not have run yet.
        drop(queue.preferences("seal", |p| p.codec = Codec::Hevc));
        let read = queue.read("seal");
        assert_eq!(read.codec, Codec::Hevc);
        assert_eq!(read.video.get(&settings::Key::Jpeg), Some(&45));
        queue.flush_blocking(Duration::from_secs(5));
        assert_eq!(saved(dir.path(), "seal").codec, Codec::Hevc);
    }

    /// The queue is the only writer: nothing outside it calls the blocking writers. Later
    /// work must queue writes too, or a main-thread write could race a queued one.
    #[test]
    fn only_the_write_queue_writes_files() {
        let writers = [
            "update_at(",
            "save_key_at(",
            "forget_key_at(",
            "write_saved_key(",
            "save_server_at(",
            "forget_server_at(",
            "clear_start_with_at(",
            "edit_config(",
        ];
        let allowed = [
            "persist.rs",
            "settings.rs",
            "sealskin.rs",
            "direct.rs",
            "config_store.rs",
        ];
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                // direct.rs's own tests write through the blocking writers too.
                let direct_tests = path.ends_with("direct/tests.rs");
                if allowed.contains(&name.as_str()) || direct_tests || !name.ends_with(".rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for writer in writers {
                    assert!(
                        !text.contains(writer),
                        "{} calls {writer}; queue the write on persist::WriteQueue",
                        path.display()
                    );
                }
            }
        }
    }
}
