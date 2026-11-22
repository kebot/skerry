//! Remote printing (`ARCH-033`, `ARCH-GAP-DC-04`): the documents the session prints through
//! Selkies' CUPS queue, fetched once each and kept in a per-window cache.
//!
//! Selkies `715df7c` announces each document on the socket
//! (`{"type":"print_document","name":…,"size_bytes":…}`, `ws/server.rs`) and again to every
//! page that connects while it waits (`ws/handler.rs:488`). `GET api/print/<name>` deletes it
//! as the 200 answer is built, before the body goes out (`files/transfer.rs:297`), so a fetch
//! is one-shot: Skerry fetches at once, on Tokio, never with a `Range`, and opens its file
//! before it asks so a local failure leaves the document in the spool.
//!
//! This module holds no GPUI or AppKit: the window (`app/session_window.rs`) drives it.

use crate::selkies::{Progress, SelkiesClient};
use serde_json::Value;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// The largest document Skerry receives; a larger one stays in the session's spool.
pub const MAX_PRINT_BYTES: u64 = 1 << 30;
/// The jobs one window lists; the oldest beyond it is removed.
pub const JOBS_CAP: usize = 20;
/// How long a Ready notice stays.
pub const READY_NOTICE: Duration = Duration::from_secs(15);
/// The startup (and quit) sweep removes cache entries older than this.
pub const SWEEP_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// A job older than this never opens the print dialog on its own.
pub const AUTO_STALE: Duration = Duration::from_secs(5 * 60);
/// Announcements this soon after `server_settings` are documents that were already waiting
/// (`ws/handler.rs:488` sends them right after it).
pub const CONNECT_BURST: Duration = Duration::from_secs(3);
/// No header and no body chunk may take longer than this.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(30);
/// APFS and HFS+ file names are at most 255 bytes.
const MAX_NAME_BYTES: usize = 255;
/// Bytes land here inside the job's folder, renamed once complete.
const PARTIAL_NAME: &str = ".partial";
/// `%PDF-` may follow leading junk within the first kilobyte (PDF 1.7 §7.5.2).
pub const PDF_HEADER_WINDOW: usize = 1024;

/// Selkies' own `document_name` (`files/printing.rs:85`): not empty, no `/`, no leading `.`,
/// no NUL, and ending in `.pdf` in any case. Anything else is never fetched nor made a path.
pub fn document_name(name: &str) -> Option<&str> {
    let pdf = name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(".pdf");
    let valid = !name.is_empty()
        && !name.contains(['/', '\0'])
        && !name.starts_with('.')
        && name.is_char_boundary(name.len().saturating_sub(4))
        && pdf;
    valid.then_some(name)
}

/// One announced document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announcement {
    pub name: String,
    pub size: u64,
    /// Over [`MAX_PRINT_BYTES`]: listed, never fetched.
    pub too_large: bool,
}

impl Announcement {
    /// The socket's `{type, name, size_bytes}`, or WebRTC's `{type, data: {…}}`. `None` for
    /// a name Selkies itself would refuse. A missing size counts as 0: it is advisory.
    pub fn parse(data: &Value) -> Option<Self> {
        if data["type"].as_str() != Some("print_document") {
            return None;
        }
        let body = if data["name"].is_string() {
            data
        } else {
            &data["data"]
        };
        let name = document_name(body["name"].as_str()?)?.to_owned();
        let size = body["size_bytes"].as_u64().unwrap_or(0);
        Some(Announcement {
            name,
            size,
            too_large: size > MAX_PRINT_BYTES,
        })
    }
}

/// `server_settings.printing_enabled`: on unless the server says otherwise.
pub fn enabled(settings: &Value) -> bool {
    settings["printing_enabled"]["value"]
        .as_bool()
        .unwrap_or(true)
}

/// Whether an announcement is fetched: never once the server turned printing off, and only
/// with the session's file routes in hand. While the policy is unknown the server's 403 is
/// the guard.
pub fn should_fetch(policy: Option<bool>, has_access: bool) -> bool {
    policy != Some(false) && has_access
}

/// Whether an announcement `since_settings` after this stream's `server_settings` is a
/// document that was already waiting.
pub fn is_replay(since_settings: Option<Duration>) -> bool {
    since_settings.is_some_and(|since| since < CONNECT_BURST)
}

/// The document's name as a Mac file name: a name over 255 bytes is cut, keeping `.pdf`.
pub fn local_file_name(name: &str) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name.to_owned();
    }
    let stem = &name[..name.len() - 4];
    let mut end = MAX_NAME_BYTES - 4;
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}.pdf", &stem[..end])
}

/// How long ago, as the Printing card reads it: "Just now", "1 min ago", "3 hr ago".
pub fn age_label(age: Duration) -> String {
    let minutes = age.as_secs() / 60;
    match minutes {
        0 => "Just now".into(),
        1..60 => format!("{minutes} min ago"),
        _ => format!("{} hr ago", minutes / 60),
    }
}

/// `%PDF-` within the first kilobyte.
pub fn has_pdf_header(head: &[u8]) -> bool {
    let head = &head[..head.len().min(PDF_HEADER_WINDOW)];
    head.windows(5).any(|window| window == b"%PDF-")
}

/// Why a fetch did not produce a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchFailure {
    /// 404: another page took it first. The job disappears quietly.
    Gone,
    /// Nothing was taken from the spool (no answer, a proxy's or sign-in answer, a 206, a
    /// stall before the headers): the next connect announces it again.
    Retry(String),
    /// 403, with the server's sentence.
    Refused(String),
    /// After the 200: the body was cut off, stalled, or overran its length. Lost.
    Broken(String),
    /// What arrived wasn't a PDF. Lost.
    NotPdf,
    /// Larger than [`MAX_PRINT_BYTES`] by its `Content-Length`. Lost.
    TooLarge(u64),
    /// The file couldn't be created before asking: nothing was sent.
    Local(String),
    /// Writing on this Mac failed after the 200. Lost.
    Unsaved(String),
}

impl FetchFailure {
    /// Whether the document is still in the session's spool.
    pub fn still_on_server(&self) -> bool {
        matches!(self, FetchFailure::Retry(_) | FetchFailure::Local(_))
    }

    /// The notice's second line.
    pub fn detail(&self, name: &str) -> String {
        match self {
            FetchFailure::Gone => String::new(),
            FetchFailure::Retry(_) => {
                format!("Couldn’t reach the session. Reconnect to receive {name}.")
            }
            FetchFailure::Refused(sentence) if sentence.contains("printing disabled") => {
                "Printing is turned off for this session.".into()
            }
            FetchFailure::Refused(sentence) => format!("The server refused it: {sentence}"),
            FetchFailure::Broken(_) => {
                "The connection dropped partway. Print it again from the session.".into()
            }
            FetchFailure::NotPdf => {
                "What arrived wasn’t a PDF. Print it again from the session.".into()
            }
            FetchFailure::TooLarge(_) => too_large_detail(0),
            FetchFailure::Local(error) => {
                format!("Couldn’t make room for it on this Mac ({error}). Reconnect to try again.")
            }
            FetchFailure::Unsaved(error) => {
                format!("Couldn’t write it on this Mac ({error}). Print it again from the session.")
            }
        }
    }
}

/// The Too large notice's second line.
pub fn too_large_detail(size: u64) -> String {
    if size == 0 {
        "Skerry receives print jobs up to 1 GB.".into()
    } else {
        format!(
            "It’s {}. Skerry receives print jobs up to 1 GB.",
            skerry_ui::session::size_label(size)
        )
    }
}

/// A fetched document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub bytes: u64,
    /// The server's `Last-Modified`: when the session printed it.
    pub printed_at: Option<SystemTime>,
}

pub type FetchResult = Result<Fetched, FetchFailure>;

/// Fetches `name` into `job_dir/file_name`. The folder and the partial file are made first:
/// if that fails nothing is asked, and the document stays in the spool. When `closed` is set
/// (the window went away) the folder is removed instead of the file renamed into place.
pub async fn fetch(
    client: &SelkiesClient,
    name: &str,
    job_dir: &Path,
    file_name: &str,
    progress: &Progress,
    closed: &AtomicBool,
    stall: Duration,
) -> FetchResult {
    let local = |error: io::Error| FetchFailure::Local(error.to_string());
    tokio::fs::create_dir_all(job_dir).await.map_err(local)?;
    let partial = job_dir.join(PARTIAL_NAME);
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&partial)
        .await
        .map_err(local)?;
    let received = client
        .print_document(name, &mut file, progress, stall)
        .await;
    let flushed = tokio::io::AsyncWriteExt::flush(&mut file).await;
    drop(file);
    let finished = received.and_then(|fetched| {
        flushed
            .map(|()| fetched)
            .map_err(|error| FetchFailure::Unsaved(error.to_string()))
    });
    match finished {
        Ok(fetched) if !closed.load(Ordering::SeqCst) => {
            match tokio::fs::rename(&partial, job_dir.join(file_name)).await {
                Ok(()) => Ok(fetched),
                Err(error) => {
                    let _ = tokio::fs::remove_file(&partial).await;
                    Err(FetchFailure::Unsaved(error.to_string()))
                }
            }
        }
        Ok(_) => {
            let _ = tokio::fs::remove_dir_all(job_dir).await;
            Err(FetchFailure::Broken("the window closed".into()))
        }
        Err(failure) => {
            let _ = tokio::fs::remove_file(&partial).await;
            if failure.still_on_server() || closed.load(Ordering::SeqCst) {
                let _ = tokio::fs::remove_dir(job_dir).await;
            }
            Err(failure)
        }
    }
}

/// Hands the result to the window. When the window has gone (the receiver dropped), the
/// job's folder is removed here, then its window's folder if that is now empty.
pub fn deliver(
    result: FetchResult,
    to: tokio::sync::oneshot::Sender<FetchResult>,
    job_dir: &Path,
) -> bool {
    if to.send(result).is_ok() {
        return true;
    }
    let _ = std::fs::remove_dir_all(job_dir);
    if let Some(window_root) = job_dir.parent() {
        let _ = std::fs::remove_dir(window_root);
    }
    false
}

pub type JobId = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Receiving,
    Ready,
    Failed(FetchFailure),
    /// PDFKit can't print it (protected or damaged): Open is offered instead.
    Unreadable,
    /// Announced over the cap; never fetched.
    TooLarge,
    /// macOS purged it from the cache.
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    /// `until`: when it hides on its own; `None` never (a state that stays, a pending
    /// auto-open, or the pointer over it).
    Shown {
        until: Option<Instant>,
    },
    Hidden,
}

#[derive(Clone, Debug)]
pub struct PrintJob {
    pub id: JobId,
    pub name: String,
    pub size: u64,
    /// `root/<id>`.
    pub dir: PathBuf,
    /// `dir/<name>`.
    pub path: PathBuf,
    pub state: JobState,
    pub arrived: Instant,
    /// When it arrived, by the wall clock.
    pub arrived_at: SystemTime,
    /// `Last-Modified`, once fetched.
    pub printed_at: Option<SystemTime>,
    /// Was already waiting when the session connected.
    pub replayed: bool,
    pub received: Progress,
    /// Opened in another app: its file is kept until the sweep.
    pub opened: bool,
    pub saved_to: Option<PathBuf>,
    pub save_failed: bool,
    /// Its Print… found another sheet up.
    pub busy: bool,
    pub notice: Notice,
}

impl PrintJob {
    /// How long ago the session printed it: by `Last-Modified` for a document that was
    /// waiting (unknown counts as stale), since it arrived otherwise.
    pub fn age(&self, now: Instant, wall: SystemTime) -> Duration {
        if self.replayed {
            return self.printed_at.map_or(Duration::MAX, |at| {
                wall.duration_since(at).unwrap_or_default()
            });
        }
        now.saturating_duration_since(self.arrived)
    }

    /// Newest first, by when the session printed it.
    fn stamp(&self) -> SystemTime {
        self.printed_at.unwrap_or(self.arrived_at)
    }

    fn file_actions(&self) -> bool {
        matches!(self.state, JobState::Ready | JobState::Unreadable)
    }
}

/// What an announcement starts.
#[derive(Debug)]
pub enum Announced {
    /// Fetch into `dir/file_name`.
    Fetch {
        id: JobId,
        dir: PathBuf,
        file_name: String,
        progress: Progress,
    },
    /// Listed as too large; nothing is fetched.
    Listed,
    /// The same name is already receiving or listed as too large.
    Ignored,
}

/// What the auto-open rule decides for one job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoDecision {
    Open,
    Wait,
    Drop,
}

/// Everything the auto-open rule reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoGuard {
    pub auto: bool,
    pub window_active: bool,
    /// A sheet Skerry started, or any sheet on the window.
    pub sheet_open: bool,
    /// The start-up banner (Keep Waiting / Retry) is up.
    pub modal_open: bool,
    pub age: Duration,
    pub dismissed: bool,
    pub newest: bool,
    /// Another document of the connect burst is still arriving.
    pub burst_pending: bool,
}

/// The auto-open rule (`ARCH-033`): only the newest pending job, never once dismissed or
/// stale, and only into an active window with no sheet and no start-up banner.
pub fn auto_print_now(guard: AutoGuard) -> AutoDecision {
    if !guard.auto || guard.dismissed || !guard.newest || guard.age > AUTO_STALE {
        return AutoDecision::Drop;
    }
    if guard.burst_pending || !guard.window_active || guard.sheet_open || guard.modal_open {
        return AutoDecision::Wait;
    }
    AutoDecision::Open
}

/// One window's print jobs.
#[derive(Debug)]
pub struct PrintJobs {
    root: PathBuf,
    jobs: Vec<PrintJob>,
    next: JobId,
    /// The job that opens the print dialog once the window allows.
    pub pending_auto: Option<JobId>,
    /// The job whose sheet Skerry started and that may still be up.
    pub sheet_job: Option<JobId>,
    /// A sheet is being started (between scheduling and AppKit).
    pub opening: Option<JobId>,
    /// Set when the window goes away; in-flight fetches remove their folders.
    closed: Arc<AtomicBool>,
    /// Folders to delete, taken by the window.
    doomed: Vec<PathBuf>,
}

impl PrintJobs {
    pub fn new(root: PathBuf) -> Self {
        PrintJobs {
            root,
            jobs: Vec::new(),
            next: 1,
            pending_auto: None,
            sheet_job: None,
            opening: None,
            closed: Arc::new(AtomicBool::new(false)),
            doomed: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn closed(&self) -> Arc<AtomicBool> {
        self.closed.clone()
    }

    /// Oldest first.
    pub fn jobs(&self) -> &[PrintJob] {
        &self.jobs
    }

    pub fn get(&self, id: JobId) -> Option<&PrintJob> {
        self.jobs.iter().find(|job| job.id == id)
    }

    fn get_mut(&mut self, id: JobId) -> Option<&mut PrintJob> {
        self.jobs.iter_mut().find(|job| job.id == id)
    }

    /// Folders to delete since the last call.
    pub fn take_doomed(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.doomed)
    }

    pub fn announce(
        &mut self,
        announcement: Announcement,
        replayed: bool,
        now: Instant,
        wall: SystemTime,
    ) -> Announced {
        let repeat = self.jobs.iter().any(|job| {
            job.name == announcement.name
                && matches!(job.state, JobState::Receiving | JobState::TooLarge)
        });
        if repeat {
            return Announced::Ignored;
        }
        let id = self.next;
        self.next += 1;
        let dir = self.root.join(id.to_string());
        let file_name = local_file_name(&announcement.name);
        let progress = Progress::default();
        self.jobs.push(PrintJob {
            id,
            name: announcement.name,
            size: announcement.size,
            path: dir.join(&file_name),
            dir: dir.clone(),
            state: if announcement.too_large {
                JobState::TooLarge
            } else {
                JobState::Receiving
            },
            arrived: now,
            arrived_at: wall,
            printed_at: None,
            replayed,
            received: progress.clone(),
            opened: false,
            saved_to: None,
            save_failed: false,
            busy: false,
            notice: Notice::Shown { until: None },
        });
        self.enforce_cap();
        if announcement.too_large {
            Announced::Listed
        } else {
            Announced::Fetch {
                id,
                dir,
                file_name,
                progress,
            }
        }
    }

    /// Records a fetch's result. Returns whether the job is now Ready.
    pub fn finished(&mut self, id: JobId, result: FetchResult, now: Instant) -> bool {
        let Some(job) = self.get_mut(id) else {
            return false;
        };
        match result {
            Ok(fetched) => {
                job.state = JobState::Ready;
                job.printed_at = fetched.printed_at;
                if job.notice != Notice::Hidden {
                    job.notice = Notice::Shown {
                        until: Some(now + READY_NOTICE),
                    };
                }
                true
            }
            Err(FetchFailure::Gone) => {
                eprintln!("Print job {} was taken by another page.", job.name);
                let dir = job.dir.clone();
                self.jobs.retain(|job| job.id != id);
                self.doomed.push(dir);
                false
            }
            Err(failure) => {
                job.state = JobState::Failed(failure);
                // A failure always shows, even after its Receiving notice was dismissed.
                job.notice = Notice::Shown { until: None };
                false
            }
        }
    }

    /// A Ready job asks to open the print dialog by itself. The newest wins: an older
    /// pending job falls back to a plain Ready notice.
    pub fn offer_auto(&mut self, id: JobId, now: Instant) {
        let Some(stamp) = self.get(id).map(PrintJob::stamp) else {
            return;
        };
        match self.pending_auto.and_then(|pending| self.get(pending)) {
            Some(pending) if pending.stamp() > stamp => self.restart_timer(id, now),
            Some(pending) => {
                let older = pending.id;
                self.pending_auto = Some(id);
                self.pause_timer(id);
                self.restart_timer(older, now);
            }
            None => {
                self.pending_auto = Some(id);
                self.pause_timer(id);
            }
        }
    }

    /// Whether the pending job opens now. A dropped job falls back to a plain Ready notice.
    pub fn auto_step(
        &mut self,
        auto: bool,
        window_active: bool,
        sheet_open: bool,
        modal_open: bool,
        now: Instant,
        wall: SystemTime,
    ) -> Option<JobId> {
        let id = self.pending_auto?;
        if self.opening.is_some() {
            return None;
        }
        let Some(job) = self.get(id).filter(|job| job.state == JobState::Ready) else {
            self.pending_auto = None;
            return None;
        };
        let guard = AutoGuard {
            auto,
            window_active,
            sheet_open: sheet_open || self.sheet_job.is_some(),
            modal_open,
            age: job.age(now, wall),
            dismissed: job.notice == Notice::Hidden,
            // `offer_auto` keeps only the newest offered job pending.
            newest: true,
            burst_pending: job.replayed
                && self
                    .jobs
                    .iter()
                    .any(|other| other.replayed && other.state == JobState::Receiving),
        };
        match auto_print_now(guard) {
            AutoDecision::Open => {
                self.opening = Some(id);
                Some(id)
            }
            AutoDecision::Wait => None,
            AutoDecision::Drop => {
                self.pending_auto = None;
                self.restart_timer(id, now);
                None
            }
        }
    }

    /// The print sheet for `id` is up: its notice leaves, as the sheet is the feedback.
    pub fn sheet_shown(&mut self, id: JobId) {
        self.opening = None;
        self.sheet_job = Some(id);
        if self.pending_auto == Some(id) {
            self.pending_auto = None;
        }
        if let Some(job) = self.get_mut(id) {
            job.busy = false;
            job.notice = Notice::Hidden;
        }
    }

    /// Print… found another sheet up.
    pub fn sheet_busy(&mut self, id: JobId, auto: bool) {
        self.opening = None;
        if auto {
            return;
        }
        if let Some(job) = self.get_mut(id) {
            job.busy = true;
            job.notice = Notice::Shown { until: None };
        }
    }

    /// The sheet couldn't start for a reason that may pass (the window wasn't key).
    pub fn sheet_deferred(&mut self) {
        self.opening = None;
    }

    pub fn set_unreadable(&mut self, id: JobId) {
        self.opening = None;
        if self.pending_auto == Some(id) {
            self.pending_auto = None;
        }
        if let Some(job) = self.get_mut(id) {
            job.state = JobState::Unreadable;
            job.busy = false;
            job.notice = Notice::Shown { until: None };
        }
    }

    /// The window has no sheet any more: forget the one Skerry started, and let busy
    /// notices count down again.
    pub fn sheet_closed(&mut self, now: Instant) {
        self.sheet_job = None;
        let busy: Vec<JobId> = self
            .jobs
            .iter()
            .filter(|job| job.busy)
            .map(|job| job.id)
            .collect();
        for id in busy {
            if let Some(job) = self.get_mut(id) {
                job.busy = false;
            }
            self.restart_timer(id, now);
        }
    }

    /// The file is gone from the cache.
    pub fn set_missing(&mut self, id: JobId) {
        if self.pending_auto == Some(id) {
            self.pending_auto = None;
        }
        if let Some(job) = self.get_mut(id) {
            job.state = JobState::Missing;
            job.notice = Notice::Shown { until: None };
        }
    }

    pub fn mark_opened(&mut self, id: JobId) {
        if let Some(job) = self.get_mut(id) {
            job.opened = true;
            job.busy = false;
        }
    }

    pub fn mark_saved(&mut self, id: JobId, to: Option<PathBuf>, now: Instant) {
        let Some(job) = self.get_mut(id) else {
            return;
        };
        job.save_failed = to.is_none();
        job.busy = false;
        if to.is_some() {
            job.saved_to = to;
        }
        if job.notice != Notice::Hidden {
            self.restart_timer(id, now);
        }
    }

    /// × on a notice: hidden for good, and a pending auto-open is cancelled.
    pub fn dismiss(&mut self, id: JobId) {
        if self.pending_auto == Some(id) {
            self.pending_auto = None;
        }
        if let Some(job) = self.get_mut(id) {
            job.notice = Notice::Hidden;
        }
    }

    /// The pointer over a notice pauses its countdown; leaving restarts it.
    pub fn hover(&mut self, id: JobId, over: bool, now: Instant) {
        if over {
            self.pause_timer(id);
        } else if self.pending_auto != Some(id) {
            self.restart_timer(id, now);
        }
    }

    fn pause_timer(&mut self, id: JobId) {
        if let Some(job) = self.get_mut(id) {
            if let Notice::Shown { until } = &mut job.notice {
                *until = None;
            }
        }
    }

    /// Only Ready notices count down, and not while busy.
    fn restart_timer(&mut self, id: JobId, now: Instant) {
        if let Some(job) = self.get_mut(id) {
            if let Notice::Shown { until } = &mut job.notice {
                let counts = job.state == JobState::Ready && !job.busy;
                *until = counts.then_some(now + READY_NOTICE);
            }
        }
    }

    /// Removes a job from the list (Remove in the card). Its folder is deleted unless the
    /// file is open elsewhere or its sheet may still be up. A job still receiving stays.
    pub fn remove(&mut self, id: JobId) -> bool {
        let Some(index) = self.jobs.iter().position(|job| job.id == id) else {
            return false;
        };
        if self.jobs[index].state == JobState::Receiving || self.sheet_job == Some(id) {
            return false;
        }
        let job = self.jobs.remove(index);
        if self.pending_auto == Some(id) {
            self.pending_auto = None;
        }
        if !job.opened {
            self.doomed.push(job.dir);
        }
        true
    }

    /// Past [`JOBS_CAP`], the oldest removable jobs go.
    fn enforce_cap(&mut self) {
        while self.jobs.len() > JOBS_CAP {
            let Some(oldest) = self
                .jobs
                .iter()
                .find(|job| job.state != JobState::Receiving && self.sheet_job != Some(job.id))
                .map(|job| job.id)
            else {
                return;
            };
            self.remove(oldest);
        }
    }

    /// Hides Ready notices whose time is up. Returns whether anything changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for job in &mut self.jobs {
            if let Notice::Shown { until: Some(until) } = job.notice {
                if until <= now {
                    job.notice = Notice::Hidden;
                    changed = true;
                }
            }
        }
        changed
    }

    /// Whether the 1 s tick has anything to do: a progress bar, a countdown, a pending
    /// auto-open, or a sheet to watch close.
    pub fn needs_tick(&self) -> bool {
        self.pending_auto.is_some()
            || self.sheet_job.is_some()
            || self.jobs.iter().any(|job| {
                job.state == JobState::Receiving
                    || job.busy
                    || matches!(job.notice, Notice::Shown { until: Some(_) })
            })
    }

    /// Shown notices, newest first.
    pub fn notices(&self) -> Vec<&PrintJob> {
        self.jobs
            .iter()
            .rev()
            .filter(|job| matches!(job.notice, Notice::Shown { .. }))
            .collect()
    }

    /// The window is going away: every folder but those opened elsewhere or under a sheet
    /// that may still be up, and the window's folder once nothing in it is kept. In-flight
    /// fetches see `closed` and remove their own.
    pub fn teardown(&mut self) -> Vec<PathBuf> {
        self.closed.store(true, Ordering::SeqCst);
        let sheet = self.sheet_job;
        let (kept, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.jobs)
            .into_iter()
            .partition(|job| job.opened || sheet == Some(job.id));
        let mut doomed = self.take_doomed();
        if kept.is_empty() {
            doomed.push(self.root.clone());
        } else {
            doomed.extend(gone.into_iter().map(|job| job.dir));
        }
        self.jobs = kept;
        self.pending_auto = None;
        doomed
    }

    /// Whether `id`'s file actions (Print…, Open, Save…) apply.
    pub fn has_file(&self, id: JobId) -> bool {
        self.get(id).is_some_and(PrintJob::file_actions)
    }
}

/// Every window's cleanup at quit, run synchronously: `cx.quit()` skips the windows' close
/// handlers, so `teardown` would never run.
pub fn quit_cleanup<'a>(windows: impl IntoIterator<Item = &'a mut PrintJobs>) -> Vec<PathBuf> {
    windows
        .into_iter()
        .flat_map(|jobs| jobs.teardown())
        .collect()
}

/// Deletes folders now, with `std::fs` (the runtime may be going away).
pub fn remove_now(paths: &[PathBuf]) {
    for path in paths {
        if let Err(error) = std::fs::remove_dir_all(path) {
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!("Couldn’t remove {}: {error}", path.display());
            }
        }
    }
}

/// `~/Library/Caches/SealSkin Native/Print`.
pub fn cache_root() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Caches/SealSkin Native/Print"))
}

/// One window's folder below the cache root.
pub fn window_root(base: &Path, uuid: &str) -> PathBuf {
    base.join(uuid)
}

/// Removes every top-level entry of `root` whose newest modification time is older than
/// `max_age` at `now`. Symbolic links are never followed: a link is judged and removed as
/// itself.
pub fn sweep(root: &Path, now: SystemTime, max_age: Duration) -> io::Result<usize> {
    let mut removed = 0;
    let entries = match std::fs::read_dir(root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        other => other?,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(newest) = newest_mtime(&path) else {
            continue;
        };
        if now.duration_since(newest).unwrap_or_default() <= max_age {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path)?;
        let result = if meta.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if result.is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// The newest mtime of `path` and, for a real folder, everything below it.
fn newest_mtime(path: &Path) -> Option<SystemTime> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let mut newest = meta.modified().ok()?;
    if meta.is_dir() {
        for entry in std::fs::read_dir(path).ok()?.flatten() {
            if let Some(time) = newest_mtime(&entry.path()) {
                newest = newest.max(time);
            }
        }
    }
    Some(newest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn announcement(name: &str) -> Announcement {
        Announcement {
            name: name.into(),
            size: 1000,
            too_large: false,
        }
    }

    fn jobs() -> PrintJobs {
        PrintJobs::new(PathBuf::from("/cache/Print/w1"))
    }

    fn fetch_id(announced: Announced) -> JobId {
        match announced {
            Announced::Fetch { id, .. } => id,
            other => panic!("expected a fetch, got {other:?}"),
        }
    }

    fn ready(at: SystemTime) -> FetchResult {
        Ok(Fetched {
            bytes: 1000,
            printed_at: Some(at),
        })
    }

    #[test]
    fn announcement_parses_websocket_and_data_channel_shapes() {
        let ws = json!({"type": "print_document", "name": "Report.pdf", "size_bytes": 3400});
        let rtc =
            json!({"type": "print_document", "data": {"name": "Report.pdf", "size_bytes": 3400}});
        for frame in [ws, rtc] {
            assert_eq!(
                Announcement::parse(&frame),
                Some(Announcement {
                    name: "Report.pdf".into(),
                    size: 3400,
                    too_large: false
                })
            );
        }
        let no_size = json!({"type": "print_document", "name": "a.pdf"});
        assert_eq!(Announcement::parse(&no_size).unwrap().size, 0);
        assert_eq!(
            Announcement::parse(&json!({"type": "clipboard", "name": "a.pdf"})),
            None
        );
    }

    #[test]
    fn announcement_refuses_paths_hidden_and_non_pdf_names() {
        for name in [
            "../x.pdf",
            "sub/x.pdf",
            ".hidden.pdf",
            "x\0.pdf",
            "x.txt",
            "",
            ".pdf",
        ] {
            let frame = json!({"type": "print_document", "name": name, "size_bytes": 1});
            assert_eq!(Announcement::parse(&frame), None, "{name:?}");
        }
        for name in ["Report (2).pdf", "A.PDF", "Résumé #2.pdf", "x.Pdf"] {
            assert_eq!(document_name(name), Some(name), "{name:?}");
        }
        // A multi-byte character just before the extension never splits.
        assert_eq!(document_name("é"), None);
    }

    #[test]
    fn announcement_refuses_documents_over_the_size_cap() {
        let over =
            json!({"type": "print_document", "name": "big.pdf", "size_bytes": MAX_PRINT_BYTES + 1});
        assert!(Announcement::parse(&over).unwrap().too_large);
        let at =
            json!({"type": "print_document", "name": "big.pdf", "size_bytes": MAX_PRINT_BYTES});
        assert!(!Announcement::parse(&at).unwrap().too_large);
    }

    #[test]
    fn an_oversized_announcement_is_listed_but_never_fetched() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        let big = Announcement {
            too_large: true,
            ..announcement("big.pdf")
        };
        let Announced::Listed = jobs.announce(big.clone(), false, now, wall) else {
            panic!("listed");
        };
        assert_eq!(jobs.jobs()[0].state, JobState::TooLarge);
        // The reconnect's repeat doesn't stack another warning.
        assert!(matches!(
            jobs.announce(big, true, now, wall),
            Announced::Ignored
        ));
        assert_eq!(jobs.jobs().len(), 1);
    }

    #[test]
    fn printing_follows_printing_enabled_and_defaults_on() {
        assert!(enabled(&json!({})));
        assert!(enabled(&json!({"printing_enabled": {"value": true}})));
        assert!(!enabled(&json!({"printing_enabled": {"value": false}})));
        assert!(!enabled(
            &json!({"printing_enabled": {"value": false, "locked": true}})
        ));
    }

    #[test]
    fn announcements_are_fetched_only_when_allowed() {
        assert!(!should_fetch(Some(false), true));
        assert!(should_fetch(None, true));
        assert!(should_fetch(Some(true), true));
        assert!(!should_fetch(Some(true), false));
        assert!(!should_fetch(None, false));
    }

    #[test]
    fn documents_announced_right_after_settings_are_replays() {
        assert!(is_replay(Some(Duration::from_millis(40))));
        assert!(!is_replay(Some(Duration::from_secs(60))));
        assert!(!is_replay(None));
    }

    #[test]
    fn a_name_still_receiving_is_not_fetched_twice() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        fetch_id(jobs.announce(announcement("Report.pdf"), false, now, wall));
        assert!(matches!(
            jobs.announce(announcement("Report.pdf"), true, now, wall),
            Announced::Ignored
        ));
    }

    #[test]
    fn a_name_can_arrive_again_after_its_first_copy() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        let first = fetch_id(jobs.announce(announcement("Report.pdf"), false, now, wall));
        jobs.finished(first, ready(wall), now);
        let second = fetch_id(jobs.announce(announcement("Report.pdf"), false, now, wall));
        assert_ne!(first, second);
        assert_ne!(jobs.get(first).unwrap().dir, jobs.get(second).unwrap().dir);
        // A failure that left the document on the server lets its re-announce start afresh.
        jobs.finished(second, Err(FetchFailure::Retry("timeout".into())), now);
        fetch_id(jobs.announce(announcement("Report.pdf"), true, now, wall));
    }

    #[test]
    fn a_failure_before_the_answer_says_reconnect_not_reprint() {
        let retry = FetchFailure::Retry("connect".into());
        assert!(retry.still_on_server());
        assert_eq!(
            retry.detail("Report.pdf"),
            "Couldn’t reach the session. Reconnect to receive Report.pdf."
        );
        let local = FetchFailure::Local("denied".into());
        assert!(local.still_on_server() && local.detail("a.pdf").contains("Reconnect"));
        for lost in [
            FetchFailure::Broken("cut".into()),
            FetchFailure::NotPdf,
            FetchFailure::Unsaved("full".into()),
        ] {
            assert!(!lost.still_on_server());
            assert!(lost.detail("a.pdf").contains("Print it again"), "{lost:?}");
        }
        assert_eq!(
            FetchFailure::Refused("Forbidden: printing disabled".into()).detail("a.pdf"),
            "Printing is turned off for this session."
        );
    }

    #[test]
    fn a_gone_document_disappears_quietly() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        let id = fetch_id(jobs.announce(announcement("Report.pdf"), false, now, wall));
        assert!(!jobs.finished(id, Err(FetchFailure::Gone), now));
        assert!(jobs.jobs().is_empty());
        assert!(jobs.notices().is_empty());
        assert_eq!(jobs.take_doomed(), [PathBuf::from("/cache/Print/w1/1")]);
    }

    #[test]
    fn the_oldest_job_beyond_the_cap_is_removed_and_its_file_deleted() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        for n in 0..JOBS_CAP {
            let id = fetch_id(jobs.announce(announcement(&format!("{n}.pdf")), false, now, wall));
            jobs.finished(id, ready(wall), now);
        }
        jobs.mark_opened(2);
        assert!(jobs.take_doomed().is_empty());
        fetch_id(jobs.announce(announcement("new.pdf"), false, now, wall));
        assert_eq!(jobs.jobs().len(), JOBS_CAP);
        assert!(jobs.get(1).is_none());
        assert_eq!(jobs.take_doomed(), [PathBuf::from("/cache/Print/w1/1")]);
        // An opened job goes from the list but keeps its file.
        fetch_id(jobs.announce(announcement("newer.pdf"), false, now, wall));
        assert!(jobs.get(2).is_none());
        assert!(jobs.take_doomed().is_empty());
    }

    #[test]
    fn the_cap_never_drops_the_job_whose_sheet_is_up() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        for n in 0..JOBS_CAP {
            let id = fetch_id(jobs.announce(announcement(&format!("{n}.pdf")), false, now, wall));
            jobs.finished(id, ready(wall), now);
        }
        jobs.sheet_shown(1);
        fetch_id(jobs.announce(announcement("new.pdf"), false, now, wall));
        assert!(jobs.get(1).is_some());
        assert!(jobs.get(2).is_none());
    }

    #[test]
    fn remove_keeps_files_that_were_opened() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        let a = fetch_id(jobs.announce(announcement("a.pdf"), false, now, wall));
        let b = fetch_id(jobs.announce(announcement("b.pdf"), false, now, wall));
        jobs.finished(a, ready(wall), now);
        jobs.finished(b, ready(wall), now);
        jobs.mark_opened(a);
        assert!(jobs.remove(a));
        assert!(jobs.take_doomed().is_empty());
        assert!(jobs.remove(b));
        assert_eq!(jobs.take_doomed(), [PathBuf::from("/cache/Print/w1/2")]);
        // A job still receiving can't be removed: its fetch can't be cancelled.
        let c = fetch_id(jobs.announce(announcement("c.pdf"), false, now, wall));
        assert!(!jobs.remove(c));
    }

    #[test]
    fn remove_keeps_the_job_whose_sheet_is_up() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        let a = fetch_id(jobs.announce(announcement("a.pdf"), false, now, wall));
        jobs.finished(a, ready(wall), now);
        jobs.sheet_shown(a);
        assert!(!jobs.remove(a));
        jobs.sheet_closed(now);
        assert!(jobs.remove(a));
    }

    #[test]
    fn teardown_deletes_only_unopened_jobs() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        let a = fetch_id(jobs.announce(announcement("a.pdf"), false, now, wall));
        let b = fetch_id(jobs.announce(announcement("b.pdf"), false, now, wall));
        let c = fetch_id(jobs.announce(announcement("c.pdf"), false, now, wall));
        jobs.finished(a, ready(wall), now);
        jobs.finished(b, ready(wall), now);
        jobs.mark_opened(a);
        jobs.sheet_shown(b);
        let closed = jobs.closed();
        let doomed = jobs.teardown();
        assert_eq!(doomed, [PathBuf::from("/cache/Print/w1/3")]);
        assert!(closed.load(Ordering::SeqCst));
        let _ = c;
        // Nothing kept: the window's whole folder goes.
        let mut empty = PrintJobs::new(PathBuf::from("/cache/Print/w2"));
        fetch_id(empty.announce(announcement("x.pdf"), false, now, wall));
        assert_eq!(empty.teardown(), [PathBuf::from("/cache/Print/w2")]);
    }

    #[test]
    fn quit_cleanup_deletes_unopened_jobs_of_every_window() {
        let dir = tempfile::tempdir().unwrap();
        let (now, wall) = (Instant::now(), SystemTime::now());
        let mut first = PrintJobs::new(dir.path().join("w1"));
        let mut second = PrintJobs::new(dir.path().join("w2"));
        for jobs in [&mut first, &mut second] {
            let id = fetch_id(jobs.announce(announcement("a.pdf"), false, now, wall));
            jobs.finished(id, ready(wall), now);
            std::fs::create_dir_all(&jobs.get(id).unwrap().dir).unwrap();
            std::fs::write(&jobs.get(id).unwrap().path, b"%PDF-1.7").unwrap();
        }
        let id = fetch_id(second.announce(announcement("kept.pdf"), false, now, wall));
        second.finished(id, ready(wall), now);
        let kept = second.get(id).unwrap().path.clone();
        std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
        std::fs::write(&kept, b"%PDF-1.7").unwrap();
        second.mark_opened(id);

        let doomed = quit_cleanup([&mut first, &mut second]);
        remove_now(&doomed);
        assert!(!dir.path().join("w1").exists());
        assert!(!dir.path().join("w2/1").exists());
        assert!(kept.exists());
    }

    #[test]
    fn ready_notices_hide_after_fifteen_seconds_unless_hovered() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        let a = fetch_id(jobs.announce(announcement("a.pdf"), false, now, wall));
        let b = fetch_id(jobs.announce(announcement("b.pdf"), false, now, wall));
        jobs.finished(a, ready(wall), now);
        jobs.finished(b, ready(wall), now);
        jobs.hover(b, true, now);
        assert!(!jobs.tick(now + Duration::from_secs(14)));
        assert!(jobs.tick(now + READY_NOTICE));
        let shown: Vec<_> = jobs.notices().iter().map(|job| job.id).collect();
        assert_eq!(shown, [b]);
        // Leaving restarts the full countdown.
        let later = now + Duration::from_secs(60);
        jobs.hover(b, false, later);
        assert!(!jobs.tick(later + Duration::from_secs(14)));
        assert!(jobs.tick(later + READY_NOTICE));
        assert!(jobs.notices().is_empty());
        // Hidden is for good, the job stays listed.
        assert_eq!(jobs.jobs().len(), 2);
    }

    #[test]
    fn failed_and_receiving_notices_never_hide_on_their_own() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        let a = fetch_id(jobs.announce(announcement("a.pdf"), false, now, wall));
        let b = fetch_id(jobs.announce(announcement("b.pdf"), false, now, wall));
        jobs.finished(a, Err(FetchFailure::NotPdf), now);
        assert!(!jobs.tick(now + Duration::from_secs(3600)));
        assert_eq!(jobs.notices().len(), 2);
        assert!(jobs.needs_tick(), "b is still receiving");
        // A dismissed Receiving notice stays hidden once Ready.
        jobs.dismiss(b);
        jobs.finished(b, ready(wall), now);
        assert_eq!(jobs.get(b).unwrap().notice, Notice::Hidden);
        assert!(!jobs.needs_tick());
    }

    #[test]
    fn only_three_notices_show_and_the_rest_are_counted() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        for n in 0..5 {
            fetch_id(jobs.announce(announcement(&format!("{n}.pdf")), false, now, wall));
        }
        let notices = jobs.notices();
        assert_eq!(notices.len(), 5);
        let shown: Vec<_> = notices.iter().take(3).map(|job| job.id).collect();
        assert_eq!(shown, [5, 4, 3], "newest first");
        assert_eq!(notices.len() - shown.len(), 2);
    }

    #[test]
    fn auto_print_waits_for_an_active_window_without_a_sheet() {
        let base = AutoGuard {
            auto: true,
            window_active: true,
            sheet_open: false,
            modal_open: false,
            age: Duration::from_secs(1),
            dismissed: false,
            newest: true,
            burst_pending: false,
        };
        assert_eq!(auto_print_now(base), AutoDecision::Open);
        let with = |change: fn(&mut AutoGuard)| {
            let mut guard = base;
            change(&mut guard);
            auto_print_now(guard)
        };
        assert_eq!(with(|g| g.window_active = false), AutoDecision::Wait);
        assert_eq!(with(|g| g.sheet_open = true), AutoDecision::Wait);
        assert_eq!(with(|g| g.modal_open = true), AutoDecision::Wait);
        assert_eq!(with(|g| g.burst_pending = true), AutoDecision::Wait);
        assert_eq!(with(|g| g.auto = false), AutoDecision::Drop);
        assert_eq!(with(|g| g.dismissed = true), AutoDecision::Drop);
        assert_eq!(with(|g| g.newest = false), AutoDecision::Drop);
        assert_eq!(
            with(|g| g.age = AUTO_STALE + Duration::from_secs(1)),
            AutoDecision::Drop
        );

        // Through the list: only the newest pending job opens; an older one goes back to a
        // plain Ready notice with its countdown.
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        let a = fetch_id(jobs.announce(announcement("a.pdf"), false, now, wall));
        let b = fetch_id(jobs.announce(
            announcement("b.pdf"),
            false,
            now,
            wall + Duration::from_secs(1),
        ));
        jobs.finished(
            a,
            Ok(Fetched {
                bytes: 1,
                printed_at: None,
            }),
            now,
        );
        jobs.offer_auto(a, now);
        jobs.finished(
            b,
            Ok(Fetched {
                bytes: 1,
                printed_at: None,
            }),
            now,
        );
        jobs.offer_auto(b, now);
        assert_eq!(jobs.pending_auto, Some(b));
        assert!(matches!(
            jobs.get(a).unwrap().notice,
            Notice::Shown { until: Some(_) }
        ));
        assert_eq!(jobs.get(b).unwrap().notice, Notice::Shown { until: None });
        // Inactive: wait. Active: open, once (the sheet is starting).
        assert_eq!(jobs.auto_step(true, false, false, false, now, wall), None);
        assert_eq!(jobs.auto_step(true, true, false, false, now, wall), Some(b));
        assert_eq!(jobs.auto_step(true, true, false, false, now, wall), None);
        jobs.sheet_shown(b);
        assert_eq!(jobs.get(b).unwrap().notice, Notice::Hidden);
        assert_eq!(jobs.pending_auto, None);

        // Stale at activation: nothing opens, the notice counts down.
        let c = fetch_id(jobs.announce(announcement("c.pdf"), false, now, wall));
        jobs.finished(c, ready(wall), now);
        jobs.sheet_closed(now);
        jobs.offer_auto(c, now);
        let late = now + AUTO_STALE + Duration::from_secs(1);
        assert_eq!(jobs.auto_step(true, true, false, false, late, wall), None);
        assert_eq!(jobs.pending_auto, None);
        assert!(matches!(
            jobs.get(c).unwrap().notice,
            Notice::Shown { until: Some(_) }
        ));

        // Dismissed while pending: cancelled.
        let d = fetch_id(jobs.announce(announcement("d.pdf"), false, now, wall));
        jobs.finished(d, ready(wall), now);
        jobs.offer_auto(d, now);
        jobs.dismiss(d);
        assert_eq!(jobs.auto_step(true, true, false, false, now, wall), None);

        // The start-up banner holds it.
        let e = fetch_id(jobs.announce(announcement("e.pdf"), false, now, wall));
        jobs.finished(e, ready(wall), now);
        jobs.offer_auto(e, now);
        assert_eq!(jobs.auto_step(true, true, false, true, now, wall), None);
        assert_eq!(jobs.pending_auto, Some(e));
    }

    #[test]
    fn documents_waiting_at_connect_open_only_the_newest_and_only_when_fresh() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        let old = fetch_id(jobs.announce(announcement("old.pdf"), true, now, wall));
        let new = fetch_id(jobs.announce(announcement("new.pdf"), true, now, wall));
        // The newer arrives first; the older one, still receiving, holds the auto-open.
        jobs.finished(new, ready(wall - Duration::from_secs(60)), now);
        jobs.offer_auto(new, now);
        assert_eq!(jobs.auto_step(true, true, false, false, now, wall), None);
        jobs.finished(old, ready(wall - Duration::from_secs(3600)), now);
        jobs.offer_auto(old, now);
        assert_eq!(jobs.pending_auto, Some(new));
        // Both got a notice; only the newest opens.
        assert_eq!(jobs.notices().len(), 2);
        assert_eq!(
            jobs.auto_step(true, true, false, false, now, wall),
            Some(new)
        );

        // Printed over 5 minutes ago (by Last-Modified): nothing opens.
        let mut jobs = PrintJobs::new(PathBuf::from("/cache/Print/w3"));
        let stale = fetch_id(jobs.announce(announcement("s.pdf"), true, now, wall));
        jobs.finished(
            stale,
            ready(wall - AUTO_STALE - Duration::from_secs(1)),
            now,
        );
        jobs.offer_auto(stale, now);
        assert_eq!(jobs.auto_step(true, true, false, false, now, wall), None);
        assert_eq!(jobs.pending_auto, None);
    }

    #[test]
    fn sweep_removes_only_entries_older_than_a_day() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("w1/1")).unwrap();
        std::fs::write(dir.path().join("w1/1/a.pdf"), b"x").unwrap();
        std::fs::write(dir.path().join("loose.partial"), b"x").unwrap();
        assert_eq!(sweep(dir.path(), SystemTime::now(), SWEEP_AGE).unwrap(), 0);
        let tomorrow = SystemTime::now() + SWEEP_AGE + Duration::from_secs(60);
        assert_eq!(sweep(dir.path(), tomorrow, SWEEP_AGE).unwrap(), 2);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        assert_eq!(
            sweep(&dir.path().join("missing"), tomorrow, SWEEP_AGE).unwrap(),
            0
        );
    }

    #[test]
    fn sweep_does_not_follow_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("keep.txt"), b"x").unwrap();
        let root = dir.path().join("Print");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
        let tomorrow = SystemTime::now() + SWEEP_AGE + Duration::from_secs(60);
        assert_eq!(sweep(&root, tomorrow, SWEEP_AGE).unwrap(), 1);
        assert!(!root.join("link").exists());
        assert!(outside.path().join("keep.txt").exists());
    }

    #[test]
    fn destinations_stay_inside_the_window_folder() {
        let mut jobs = jobs();
        let (now, wall) = (Instant::now(), SystemTime::now());
        for name in ["Report.pdf", "..pdf.pdf", "a b (2).PDF"] {
            let Announced::Fetch { dir, file_name, .. } =
                jobs.announce(announcement(name), false, now, wall)
            else {
                panic!("fetch {name}");
            };
            assert_eq!(dir.parent(), Some(jobs.root()));
            let path = dir.join(&file_name);
            assert_eq!(path.parent(), Some(dir.as_path()));
            assert_eq!(file_name, name);
        }
        assert_eq!(window_root(Path::new("/c"), "u"), PathBuf::from("/c/u"));
    }

    #[test]
    fn a_long_name_is_shortened_keeping_pdf() {
        let long = format!("{}.pdf", "é".repeat(200));
        let short = local_file_name(&long);
        assert!(short.len() <= MAX_NAME_BYTES, "{}", short.len());
        assert!(short.ends_with(".pdf"));
        assert!(long.starts_with(&short[..short.len() - 4]));
        assert_eq!(local_file_name("a.pdf"), "a.pdf");
    }

    #[test]
    fn ages_read_in_minutes_then_hours() {
        assert_eq!(age_label(Duration::from_secs(59)), "Just now");
        assert_eq!(age_label(Duration::from_secs(60)), "1 min ago");
        assert_eq!(age_label(Duration::from_secs(12 * 60 + 5)), "12 min ago");
        assert_eq!(age_label(Duration::from_secs(3600)), "1 hr ago");
        assert_eq!(age_label(Duration::from_secs(3 * 3600 + 59)), "3 hr ago");
    }

    #[test]
    fn pdf_header_is_found_within_the_first_kilobyte() {
        assert!(has_pdf_header(b"%PDF-1.7\n"));
        let mut junk = vec![b' '; 1000];
        junk.extend_from_slice(b"%PDF-1.4");
        assert!(has_pdf_header(&junk));
        let mut late = vec![b' '; PDF_HEADER_WINDOW];
        late.extend_from_slice(b"%PDF-1.4");
        assert!(!has_pdf_header(&late));
        assert!(!has_pdf_header(b"<html>"));
    }

    #[tokio::test]
    async fn a_window_that_closed_mid_download_cleans_up_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let job_dir = dir.path().join("w1/7");
        std::fs::create_dir_all(&job_dir).unwrap();
        std::fs::write(job_dir.join("a.pdf"), b"%PDF-").unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        drop(rx);
        assert!(!deliver(ready(SystemTime::now()), tx, &job_dir));
        assert!(!job_dir.exists());
        assert!(
            !dir.path().join("w1").exists(),
            "the empty window folder goes too"
        );

        let job_dir = dir.path().join("w2/1");
        std::fs::create_dir_all(&job_dir).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        assert!(deliver(Err(FetchFailure::NotPdf), tx, &job_dir));
        assert_eq!(rx.await.unwrap(), Err(FetchFailure::NotPdf));
        assert!(job_dir.exists());
    }
}
