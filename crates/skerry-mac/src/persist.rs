//! The one writer of Skerry's files: preferences and the saved application key.
//!
//! Every write is a job on one FIFO queue. A single consumer task on the Tokio runtime runs
//! each job with `spawn_blocking`, one at a time, so two read-modify-writes never interleave
//! and two quick changes land in the order they were made (`ARCH-022`'s re-read-then-write
//! stays correct). GPUI code queues a job and gets the result back on a oneshot channel; it
//! never calls `settings::update_at` or the key writers itself.

use crate::sealskin::{self, Credential};
use crate::settings::{self, Preferences};
use gpui_kit::Global;
use std::io;
use std::path::PathBuf;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};

type Change = Box<dyn FnOnce(&mut Preferences) + Send>;

enum Job {
    Preferences {
        server: String,
        change: Change,
        done: oneshot::Sender<io::Result<()>>,
    },
    /// Save (`Some`) or remove (`None`) the application key.
    Key {
        raw: Option<String>,
        done: oneshot::Sender<Result<(), String>>,
    },
    Flush(oneshot::Sender<()>),
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

    fn key(&self) -> Result<PathBuf, String> {
        match self {
            Self::Home => sealskin::config_file(),
            #[cfg(test)]
            Self::Under(root) => Ok(sealskin::config_file_in(root)),
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
                    .and_then(|path| settings::update_at(&path, change));
                let _ = done.send(result);
            }
            Self::Key { raw, done } => {
                let result = root.key().and_then(|path| match &raw {
                    Some(raw) => sealskin::save_key_at(&path, raw),
                    None => sealskin::forget_key_at(&path),
                });
                let _ = done.send(result);
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
}

impl Global for WriteQueue {}

pub fn start(rt: &Handle) -> WriteQueue {
    start_at(rt, Root::Home)
}

fn start_at(rt: &Handle, root: Root) -> WriteQueue {
    let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
    rt.spawn(async move {
        while let Some(job) = rx.recv().await {
            let root = root.clone();
            let _ = tokio::task::spawn_blocking(move || job.run(&root)).await;
        }
    });
    WriteQueue { tx, rt: rt.clone() }
}

impl WriteQueue {
    /// Re-reads `server`'s preferences, applies `change`, and saves them.
    pub fn preferences(
        &self,
        server: &str,
        change: impl FnOnce(&mut Preferences) + Send + 'static,
    ) -> oneshot::Receiver<io::Result<()>> {
        let (done, result) = oneshot::channel();
        self.send(Job::Preferences {
            server: server.to_string(),
            change: Box::new(change),
            done,
        });
        result
    }

    pub fn save_key(&self, credential: &Credential) -> oneshot::Receiver<Result<(), String>> {
        self.key(Some(credential.raw().to_string()))
    }

    pub fn forget_key(&self) -> oneshot::Receiver<Result<(), String>> {
        self.key(None)
    }

    fn key(&self, raw: Option<String>) -> oneshot::Receiver<Result<(), String>> {
        let (done, result) = oneshot::channel();
        self.send(Job::Key { raw, done });
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
        let _ = queue.preferences("seal", |p| p.codec = Codec::Hevc);
        let _ = queue.preferences("seal", |p| p.input.command_as_control = true);
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
            let _ = queue.preferences("seal", move |p| {
                p.video.insert(settings::Key::Fps, fps);
            });
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
        let forgotten = queue.forget_key();
        rt.block_on(async {
            saved.await.unwrap().unwrap();
            forgotten.await.unwrap().unwrap();
        });
        let config = std::fs::read_to_string(sealskin::config_file_in(dir.path())).unwrap();
        assert!(!config.contains("application_key"), "{config}");
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
        ];
        let allowed = ["persist.rs", "settings.rs", "sealskin.rs"];
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
                if allowed.contains(&name.as_str()) || !name.ends_with(".rs") {
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
