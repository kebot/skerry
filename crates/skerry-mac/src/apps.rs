//! The session's Apps catalog (`ARCH-035`): the Selkies apps contract as the web dashboard
//! speaks it (Selkies `715df7c` `web/selkies-web-core/lib/app-commands.ts`,
//! `crates/selkies/src/session_screens/app_jobs.rs`), the proot-apps catalog, and its icons.
//!
//! Pure: no GPUI windows. Requests go out as `apps,{"op":…}` text frames; the server answers
//! with `system,{"action":"apps_state,…"}` / `apps_job,…` / `command_error,…`, which
//! [`parse_action`] reads on Tokio. The board ([`Board`]) applies them as
//! `applyAppsEvent` does. Catalog and icon bytes are untrusted: YAML is parsed under a budget,
//! SVG is pre-checked and rendered with every external resource refused, raster images are
//! decoded under tight limits.

use reqwest::Url;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where the catalog and its icons live when the server has no local repository: the same
/// URL the Selkies web client fetches (`REMOTE_CATALOG_BASE`).
pub const REMOTE_BASE: &str =
    "https://raw.githubusercontent.com/linuxserver/proot-apps/master/metadata/";
/// The console lines a job keeps, as the server keeps them.
pub const CONSOLE_LINES: usize = 200;
/// The catalog document's byte cap and fetch timeout.
pub const CATALOG_CAP: usize = 2 * 1024 * 1024;
pub const CATALOG_TIMEOUT: Duration = Duration::from_secs(10);
/// One icon's byte cap and fetch timeout. 1 MiB: the real `handbrake.svg` is 729 KB; the
/// SVG cost checks bound the work, not the bytes.
pub const ICON_CAP: usize = 1024 * 1024;
/// A raster icon's byte cap, under [`ICON_CAP`].
pub const RASTER_ICON_CAP: usize = 512 * 1024;
pub const ICON_TIMEOUT: Duration = Duration::from_secs(5);
/// Icons are rasterized or downscaled into a box this many pixels on the long edge.
pub const ICON_EDGE: u32 = 128;
/// Icon fetches in flight at once, per window.
pub const ICON_FETCHES: usize = 6;
/// Icon decodes running at once, per window: separate from the fetches.
pub const ICON_DECODES: usize = 2;
/// A second line behind the worker's own limits ([`SVG_WORKER_FLAG`], killed after 3 s): an
/// SVG decode still unanswered after this (a worker slot wait included) marks the icon
/// failed and stops SVG decoding for the window.
pub const SVG_WATCHDOG: Duration = Duration::from_secs(10);
/// A request with no `apps_job` for its app after this gives up ("Requested…" ends).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// A `command_error` this soon after a launch belongs to it (Selkies `LAUNCH_FAILURE_WINDOW`).
pub const LAUNCH_FAILURE_WINDOW: Duration = Duration::from_secs(15);

/// SVG limits checked before usvg sees the document.
const SVG_MAX_NODES: u32 = 40_000;
const SVG_MAX_ELEMENTS: usize = 20_000;
const SVG_MAX_USES: usize = 512;
/// Elements after every `<use>` is expanded, counted with memoisation.
const SVG_MAX_EXPANDED: u64 = 50_000;
const SVG_MAX_DEPTH: usize = 64;
/// A width, height or viewBox larger than this (or not finite, or not positive) is absurd.
const SVG_MAX_SIZE: f32 = 100_000.;
/// Uses of masks, clip paths and patterns an icon may make. None may reach another: each
/// level of nesting multiplies the render work.
const SVG_MAX_PAINT_REFS: usize = 512;
/// Masks, clip paths and patterns an icon may define.
const SVG_MAX_PAINT_SERVERS: usize = 64;
/// The elements whose content renders once per use, so nesting them multiplies.
const SVG_NESTING: [&str; 3] = ["mask", "clipPath", "pattern"];
/// Refused outright: `<marker>` renders its content once per path vertex, `<filter>` chains
/// have no cheap bound. App icons use neither.
const SVG_REFUSED: [&str; 2] = ["marker", "filter"];
/// How long the apps support probe waits for `apps_state` after `list`.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// A port of Selkies' `valid_app_name` (`app_jobs.rs`): 1–200 bytes of `[A-Za-z0-9._/:-]`,
/// not `all`, not starting with `-`. A name that fails is never offered or sent.
pub fn valid_app_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name != "all"
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | ':' | '-'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    Install,
    Remove,
    Update,
    Launch,
    List,
    Cancel,
    Clear,
}

impl Op {
    pub fn wire(self) -> &'static str {
        match self {
            Op::Install => "install",
            Op::Remove => "remove",
            Op::Update => "update",
            Op::Launch => "launch",
            Op::List => "list",
            Op::Cancel => "cancel",
            Op::Clear => "clear",
        }
    }

    fn needs_app(self) -> bool {
        matches!(self, Op::Install | Op::Remove | Op::Update | Op::Launch)
    }
}

/// The `apps,` request for `op`, keys in the web's order (`op`, `app`, `id`) and left out when
/// unused. `None` when an op that names an app has no valid one, or `cancel` has no id.
pub fn request(op: Op, app: Option<&str>, id: Option<u64>) -> Option<String> {
    let mut text = format!(r#"apps,{{"op":"{}""#, op.wire());
    if op.needs_app() {
        let app = app.filter(|app| valid_app_name(app))?;
        // A valid name needs no escaping; serde_json keeps that true if the rule changes.
        text.push_str(&format!(r#","app":{}"#, serde_json::to_string(app).ok()?));
    }
    match (op, id) {
        (Op::Cancel | Op::Clear, Some(id)) => text.push_str(&format!(r#","id":{id}"#)),
        (Op::Cancel, None) => return None,
        _ => {}
    }
    text.push('}');
    Some(text)
}

/// What the server allows, from `server_settings`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Policy {
    /// `command_enabled.value == true` and `ui_sidebar_show_apps.value != false`.
    pub allowed: bool,
    /// `apps_local_repo.value == true`: the catalog is the session's own `api/apps/`.
    pub local_repo: bool,
}

impl Policy {
    pub fn from_server(settings: &Value) -> Self {
        Policy {
            allowed: settings["command_enabled"]["value"] == Value::Bool(true)
                && settings["ui_sidebar_show_apps"]["value"] != Value::Bool(false),
            local_repo: settings["apps_local_repo"]["value"] == Value::Bool(true),
        }
    }
}

/// Whether a request may go out: only while the session is connected and allows it. Every
/// request the Apps window makes passes this gate.
pub fn can_send(connected: bool, policy: Option<Policy>) -> bool {
    connected && policy.is_some_and(|policy| policy.allowed)
}

/// The Apps window's view of the stream: its policy (`None` while disconnected).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Link {
    policy: Option<Policy>,
}

/// What a policy changed for the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyChange {
    /// Anything differs from before (redraw).
    pub changed: bool,
    /// The catalog source moved between GitHub and the session.
    pub source_changed: bool,
}

impl Link {
    pub fn connected(&self) -> bool {
        self.policy.is_some()
    }

    pub fn can_send(&self) -> bool {
        can_send(self.connected(), self.policy)
    }

    /// A `server_settings` arrived (the first of a stream, or a re-broadcast).
    pub fn on_policy(&mut self, policy: Policy) -> PolicyChange {
        let before = self.policy;
        self.policy = Some(policy);
        PolicyChange {
            changed: before != Some(policy),
            source_changed: before.is_some_and(|before| before.local_repo != policy.local_repo),
        }
    }

    /// The stream ended: nothing may be sent until the next stream says its policy.
    pub fn on_disconnect(&mut self) {
        self.policy = None;
    }
}

/// Whether the session's Selkies speaks the apps protocol. Selkies before `c72a9ef7` drops
/// `apps,` silently (its panel ran `cmd,selkies-proot …`), so each stream sends `list` once
/// on its first allowed policy and waits [`PROBE_TIMEOUT`] for `apps_state`. Apps shows only
/// once supported; the old protocol is not spoken. A reconnect starts over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Support {
    #[default]
    Unknown,
    Probing {
        since: Instant,
        /// Bumped by every probe, so a stale timeout is ignored.
        generation: u64,
    },
    Supported,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    support: Support,
    generation: u64,
}

impl Probe {
    #[cfg(test)]
    pub fn support(&self) -> Support {
        self.support
    }

    pub fn supported(&self) -> bool {
        self.support == Support::Supported
    }

    pub fn unsupported(&self) -> bool {
        self.support == Support::Unsupported
    }

    /// A policy arrived: `Some(generation)` when `list` should go out now and a timeout be
    /// armed for that generation (the first allowed policy of the stream).
    pub fn on_policy(&mut self, policy: Policy, now: Instant) -> Option<u64> {
        if !policy.allowed || self.support != Support::Unknown {
            return None;
        }
        self.generation += 1;
        self.support = Support::Probing {
            since: now,
            generation: self.generation,
        };
        Some(self.generation)
    }

    /// An apps event came back: the server speaks the protocol. `true` when that is news.
    pub fn on_answer(&mut self) -> bool {
        let news = self.support != Support::Supported;
        self.support = Support::Supported;
        news
    }

    /// The probe's timer for `generation` fired: `true` when the session is now known to be
    /// unsupported.
    pub fn on_timeout(&mut self, generation: u64, now: Instant) -> bool {
        match self.support {
            Support::Probing {
                since,
                generation: probing,
            } if probing == generation && now.saturating_duration_since(since) >= PROBE_TIMEOUT => {
                self.support = Support::Unsupported;
                true
            }
            _ => false,
        }
    }

    /// How long the probe for `generation` still has to wait, if it is still waiting: a timer
    /// that fires early re-arms for the rest rather than leaving the probe stuck.
    pub fn remaining(&self, generation: u64, now: Instant) -> Option<Duration> {
        match self.support {
            Support::Probing {
                since,
                generation: probing,
            } if probing == generation => {
                let waited = now.saturating_duration_since(since);
                (waited < PROBE_TIMEOUT).then(|| PROBE_TIMEOUT - waited)
            }
            _ => None,
        }
    }

    /// The stream ended: the next one probes again.
    pub fn reset(&mut self) {
        self.support = Support::Unknown;
        self.generation += 1;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Canceled,
}

impl JobState {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "queued" => JobState::Queued,
            "running" => JobState::Running,
            "done" => JobState::Done,
            "failed" => JobState::Failed,
            "canceled" => JobState::Canceled,
            _ => return None,
        })
    }

    pub fn finished(self) -> bool {
        matches!(self, JobState::Done | JobState::Failed | JobState::Canceled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobOp {
    Install,
    Remove,
    Update,
}

impl JobOp {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "install" => JobOp::Install,
            "remove" => JobOp::Remove,
            "update" => JobOp::Update,
            _ => return None,
        })
    }

    pub fn op(self) -> Op {
        match self {
            JobOp::Install => Op::Install,
            JobOp::Remove => Op::Remove,
            JobOp::Update => Op::Update,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: u64,
    pub app: String,
    pub op: JobOp,
    pub state: JobState,
    /// The step the runner last named (`download`, `install`, `remove`).
    pub phase: Option<String>,
    /// A download's percentage.
    pub progress: Option<f32>,
    /// The exit status once it ran to an end; signal deaths are negative.
    pub code: Option<i64>,
    pub lines: VecDeque<String>,
    pub current: String,
}

impl Job {
    pub fn pending(&self) -> bool {
        !self.state.finished()
    }

    /// The console as one text: the committed lines, then the line being written.
    pub fn console_text(&self) -> String {
        let mut text = String::new();
        for line in &self.lines {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str(&self.current);
        if self.current.is_empty() && text.ends_with('\n') {
            text.pop();
        }
        text
    }

    /// A job from a payload carrying its whole console (`jobFrom`); `None` without an id and
    /// an app.
    fn from_payload(payload: &Value) -> Option<Self> {
        let id = payload["id"].as_u64()?;
        let app = payload["app"].as_str()?.to_owned();
        let mut job = Job {
            id,
            app,
            op: payload["op"]
                .as_str()
                .and_then(JobOp::parse)
                .unwrap_or(JobOp::Install),
            state: payload["state"]
                .as_str()
                .and_then(JobState::parse)
                .unwrap_or(JobState::Queued),
            phase: payload["phase"].as_str().map(str::to_owned),
            progress: payload["progress"].as_f64().map(|p| p as f32),
            code: payload["code"].as_i64(),
            lines: VecDeque::new(),
            current: payload["current"].as_str().unwrap_or_default().to_owned(),
        };
        job.append(&payload["lines"]);
        job.append(&payload["append"]);
        Some(job)
    }

    fn append(&mut self, lines: &Value) {
        if let Some(lines) = lines.as_array() {
            self.lines
                .extend(lines.iter().filter_map(Value::as_str).map(str::to_owned));
            while self.lines.len() > CONSOLE_LINES {
                self.lines.pop_front();
            }
        }
    }
}

/// Everything the server reported, as the window reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    /// The installed app names; `None` until the server could say.
    pub installed: Option<BTreeSet<String>>,
    /// The jobs kept, in the order they were asked for.
    pub jobs: Vec<Job>,
}

impl Board {
    /// `apps_state`: replaces the jobs; `installed: null` keeps the last set.
    pub fn apply_state(&mut self, payload: &Value) -> bool {
        if !payload.is_object() {
            return false;
        }
        if let Some(installed) = payload["installed"].as_array() {
            self.installed = Some(
                installed
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            );
        }
        self.jobs = payload["jobs"]
            .as_array()
            .map(|jobs| jobs.iter().filter_map(Job::from_payload).collect())
            .unwrap_or_default();
        true
    }

    /// `apps_job`: one job's change. An unknown id (a finish can arrive first) or `reset`
    /// rebuilds the job; otherwise a missing key keeps the old value and an explicit null
    /// clears it, and `append` extends the console.
    pub fn apply_job(&mut self, payload: &Value) -> bool {
        if !payload.is_object() {
            return false;
        }
        let at = payload["id"]
            .as_u64()
            .and_then(|id| self.jobs.iter().position(|job| job.id == id));
        let reset = payload["reset"].as_bool().unwrap_or(false);
        match at {
            Some(at) if !reset => {
                let job = &mut self.jobs[at];
                let get = |key: &str| payload.get(key);
                if let Some(state) = get("state")
                    .and_then(Value::as_str)
                    .and_then(JobState::parse)
                {
                    job.state = state;
                }
                if let Some(phase) = get("phase") {
                    job.phase = phase.as_str().map(str::to_owned);
                }
                if let Some(progress) = get("progress") {
                    job.progress = progress.as_f64().map(|p| p as f32);
                }
                if let Some(code) = get("code") {
                    job.code = code.as_i64();
                }
                if let Some(append) = get("append") {
                    job.append(append);
                }
                if let Some(current) = get("current").and_then(Value::as_str) {
                    job.current = current.to_owned();
                }
            }
            _ => {
                let Some(job) = Job::from_payload(payload) else {
                    return false;
                };
                match at {
                    Some(at) => self.jobs[at] = job,
                    None => self.jobs.push(job),
                }
            }
        }
        true
    }

    /// The app's job to show: its pending job, else its last one.
    pub fn job_for_app(&self, app: &str) -> Option<&Job> {
        self.jobs
            .iter()
            .find(|job| job.app == app && job.pending())
            .or_else(|| self.jobs.iter().rev().find(|job| job.app == app))
    }

    /// The highest job id known, for telling a new job from a stale one.
    pub fn max_id(&self) -> u64 {
        self.jobs.iter().map(|job| job.id).max().unwrap_or(0)
    }

    pub fn is_installed(&self, app: &str) -> bool {
        self.installed
            .as_ref()
            .is_some_and(|installed| installed.contains(app))
    }
}

/// A request sent for an app, waiting for its job to show up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Requested {
    pub op: Op,
    pub at: Instant,
    /// [`Board::max_id`] when it was sent.
    pub after_id: u64,
}

impl Requested {
    /// Whether the board answers this request: a pending job for the app, or a job for it
    /// newer than the request. A finished job that was already there never does.
    pub fn answered_by(&self, board: &Board, app: &str) -> bool {
        board
            .jobs
            .iter()
            .any(|job| job.app == app && (job.pending() || job.id > self.after_id))
    }

    pub fn expired(&self, now: Instant) -> bool {
        requested_expired(now, self.at)
    }
}

/// The server answers nothing to a refused request, so this is when "Requested…" gives up.
pub fn requested_expired(now: Instant, at: Instant) -> bool {
    now.saturating_duration_since(at) >= REQUEST_TIMEOUT
}

/// Whether a `command_error` at `now` belongs to the launch made at `launched`.
pub fn launch_error_applies(launched: Option<Instant>, now: Instant) -> bool {
    launched.is_some_and(|at| now.saturating_duration_since(at) <= LAUNCH_FAILURE_WINDOW)
}

/// One `system,` action the Apps window cares about, parsed on Tokio.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    State(Value),
    Job(Value),
    CommandError(String),
    /// An `apps_state` or `apps_job` whose JSON didn't parse: nothing to show, but the
    /// server does speak the apps protocol.
    Unreadable,
}

/// The apps event in a `system,` action (`apps_state,{…}`, `apps_job,{…}`,
/// `command_error,<text>`); `None` for any other action or malformed JSON.
pub fn parse_action(action: &str) -> Option<Event> {
    let (kind, rest) = action.split_once(',')?;
    match kind {
        "apps_state" | "apps_job" => {
            let value: Value = serde_json::from_str(rest).ok()?;
            if !value.is_object() {
                return None;
            }
            Some(if kind == "apps_state" {
                Event::State(value)
            } else {
                Event::Job(value)
            })
        }
        "command_error" => Some(Event::CommandError(rest.to_owned())),
        _ => None,
    }
}

/// The job's status line (design §7.2); `canceling` once Cancel was pressed on a pending job.
pub fn status_text(job: &Job, canceling: bool) -> String {
    let by_op = |install: &str, update: &str, remove: &str| -> String {
        match job.op {
            JobOp::Install => install,
            JobOp::Update => update,
            JobOp::Remove => remove,
        }
        .to_owned()
    };
    if canceling && job.pending() {
        return "Canceling…".into();
    }
    match job.state {
        JobState::Queued => by_op(
            "Waiting to install…",
            "Waiting to update…",
            "Waiting to remove…",
        ),
        JobState::Running => match (job.phase.as_deref(), job.progress) {
            (Some("download"), Some(progress)) => {
                format!("Downloading {}%", progress.clamp(0., 100.).round() as u32)
            }
            (Some("download"), None) => "Downloading…".into(),
            (Some("install"), _) => "Installing…".into(),
            (Some("remove"), _) => "Removing…".into(),
            _ => by_op("Installing…", "Updating…", "Removing…"),
        },
        JobState::Done => by_op("Installed", "Updated", "Removed"),
        JobState::Failed => match job.code {
            Some(code) => format!("Failed (exit {code})"),
            None => "Failed".into(),
        },
        JobState::Canceled => "Canceled".into(),
    }
}

/// A download's progress (0–100) while it is downloading, else `None`.
pub fn download_progress(job: &Job) -> Option<f32> {
    (job.state == JobState::Running && job.phase.as_deref() == Some("download"))
        .then_some(job.progress)
        .flatten()
        .map(|p| p.clamp(0., 100.))
}

/// What the inspector's buttons show (the web's `shownInstalled` rule).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buttons {
    /// Open / Update / Remove… rather than Install.
    pub shown_installed: bool,
    /// The button that spins: the pending job's op, or the request awaiting its job.
    pub pending: Option<Op>,
    pub enabled: bool,
}

pub fn buttons(
    installed: bool,
    job: Option<&Job>,
    requested: Option<Op>,
    actions_enabled: bool,
) -> Buttons {
    let pending = job
        .filter(|job| job.pending())
        .map(|job| job.op.op())
        .or(requested);
    let shown_installed = match pending {
        Some(Op::Install) => false,
        Some(Op::Remove | Op::Update) => true,
        _ => installed,
    };
    Buttons {
        shown_installed,
        pending,
        enabled: actions_enabled && pending.is_none(),
    }
}

// ---- Catalog --------------------------------------------------------------------------

/// One installable app of the proot-apps catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogApp {
    pub name: String,
    pub full_name: String,
    pub description: String,
    /// The icon's file name; empty for a generated local-repo catalog (no fetch).
    pub icon: String,
    pub category: Option<String>,
}

/// The YAML budget for a catalog: far above the real 47 KB document, far below what a hostile
/// one would need to hurt.
fn catalog_options() -> serde_saphyr::Options {
    let mut budget = serde_saphyr::Budget::default();
    budget.max_events = 200_000;
    budget.max_aliases = 256;
    budget.max_anchors = 256;
    budget.max_depth = 16;
    budget.max_documents = 1;
    budget.max_nodes = 100_000;
    budget.max_total_scalar_bytes = 4 * 1024 * 1024;
    budget.max_recorded_anchor_events = 10_000;
    budget.max_recorded_anchor_bytes = 1024 * 1024;
    budget.max_merge_keys = 256;
    let mut aliases = serde_saphyr::options::AliasLimits::default();
    aliases.max_total_replayed_events = 10_000;
    aliases.max_replay_stack_depth = 8;
    aliases.max_alias_expansions_per_anchor = 64;
    let mut options = serde_saphyr::Options::default();
    options.budget = Some(budget);
    options.alias_limits = aliases;
    options
}

/// The catalog's apps (`include`), as the web's `isApp` filter keeps them: `name`,
/// `full_name`, `description` and `icon` must be strings (empty allowed: a local repository
/// without `metadata/` generates exactly that), `category` a string or missing, `disabled`
/// a boolean or missing. Disabled apps and names `valid_app_name` refuses are dropped, and
/// one bad entry is skipped, never fatal. `arch` is ignored (web parity).
pub fn parse_catalog(text: &str) -> Result<Vec<CatalogApp>, String> {
    if text.len() > CATALOG_CAP {
        return Err("The catalog is too large.".into());
    }
    let root: Value = serde_saphyr::from_str_with_options(text, catalog_options())
        .map_err(|error| format!("The catalog couldn’t be read: {error}"))?;
    let entries = root
        .get("include")
        .and_then(Value::as_array)
        .ok_or("The catalog lists no apps.")?;
    Ok(entries.iter().filter_map(catalog_entry).collect())
}

fn catalog_entry(entry: &Value) -> Option<CatalogApp> {
    let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
    let category = match entry.get("category") {
        None | Some(Value::Null) => None,
        Some(Value::String(category)) => Some(category.trim().to_owned()),
        Some(_) => return None,
    };
    match entry.get("disabled") {
        None | Some(Value::Bool(false)) => {}
        _ => return None,
    }
    let name = text("name")?;
    if !valid_app_name(&name) {
        return None;
    }
    Some(CatalogApp {
        full_name: text("full_name")?.trim().to_owned(),
        description: text("description")?.trim().to_owned(),
        icon: text("icon")?.trim().to_owned(),
        category: category.filter(|c| !c.is_empty()),
        name,
    })
}

/// `AudioVideo` reads "Audio Video" (the web's `categoryLabel`); "AI" stays "AI".
pub fn category_label(category: &str) -> String {
    let mut label = String::with_capacity(category.len() + 4);
    let mut previous: Option<char> = None;
    for c in category.chars() {
        if previous.is_some_and(|p| p.is_ascii_lowercase()) && c.is_ascii_uppercase() {
            label.push(' ');
        }
        label.push(c);
        previous = Some(c);
    }
    label
}

/// The catalog's categories, sorted by label.
pub fn categories(apps: &[CatalogApp]) -> Vec<String> {
    let names: BTreeSet<&str> = apps.iter().filter_map(|a| a.category.as_deref()).collect();
    let mut names: Vec<String> = names.into_iter().map(str::to_owned).collect();
    names.sort_by_key(|name| category_label(name).to_lowercase());
    names
}

/// The source list's selection.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Filter {
    Installed,
    All,
    Category(String),
}

impl Filter {
    /// As saved in `Preferences.apps_category`.
    pub fn saved(&self) -> String {
        match self {
            Filter::Installed => "Installed".into(),
            Filter::All => "All".into(),
            Filter::Category(category) => category.clone(),
        }
    }

    pub fn from_saved(saved: &str) -> Self {
        match saved {
            "Installed" => Filter::Installed,
            "All" => Filter::All,
            category => Filter::Category(category.to_owned()),
        }
    }
}

/// The first visit opens on `WebBrowser`, else `Network`, else the first category, else All
/// Apps (a catalog without categories); later visits on the saved choice while it exists.
pub fn default_filter(categories: &[String], saved: Option<&str>) -> Filter {
    if let Some(saved) = saved.map(Filter::from_saved) {
        match &saved {
            Filter::Category(category) if !categories.contains(category) => {}
            _ => return saved,
        }
    }
    ["WebBrowser", "Network"]
        .iter()
        .find(|c| categories.iter().any(|have| have == *c))
        .map(|c| (*c).to_owned())
        .or_else(|| categories.first().cloned())
        .map_or(Filter::All, Filter::Category)
}

/// The apps the grid shows: a non-empty search spans every category (name, full name and
/// description, ignoring case); otherwise the filter's. Sorted by full name.
pub fn filtered<'a>(
    apps: &'a [CatalogApp],
    filter: &Filter,
    search: &str,
    installed: Option<&BTreeSet<String>>,
) -> Vec<&'a CatalogApp> {
    let needle = search.trim().to_lowercase();
    let mut shown: Vec<&CatalogApp> = apps
        .iter()
        .filter(|app| {
            if !needle.is_empty() {
                return app.name.to_lowercase().contains(&needle)
                    || app.full_name.to_lowercase().contains(&needle)
                    || app.description.to_lowercase().contains(&needle);
            }
            match filter {
                Filter::Installed => installed.is_some_and(|set| set.contains(&app.name)),
                Filter::All => true,
                Filter::Category(category) => app.category.as_deref() == Some(category),
            }
        })
        .collect();
    shown.sort_by_cached_key(|app| app.full_name.to_lowercase());
    shown
}

/// An icon's path segment below `img/`: one encoded file name, or `None` for an empty name or
/// anything that could leave the folder.
pub fn icon_path(icon: &str) -> Option<String> {
    if icon.is_empty() || icon.contains(['/', '\\', '\0']) || icon.contains("..") {
        return None;
    }
    Some(crate::selkies::encode_component(icon))
}

/// Whether the icon file is an SVG, by its name.
pub fn is_svg(icon: &str) -> bool {
    icon.to_ascii_lowercase().ends_with(".svg")
}

/// The remote catalog's document URL.
pub fn remote_catalog_url() -> Url {
    Url::parse(REMOTE_BASE)
        .and_then(|base| base.join("metadata.yml"))
        .expect("the catalog URL parses")
}

/// The remote icon URL for an icon path from [`icon_path`].
pub fn remote_icon_url(path: &str) -> Option<Url> {
    Url::parse(REMOTE_BASE)
        .ok()?
        .join(&format!("img/{path}"))
        .ok()
}

// ---- Network --------------------------------------------------------------------------

/// The client for GitHub's catalog and icons: no default headers, no cookies, no keys, HTTPS
/// only, and at most 3 redirects, each to HTTPS. Never used for SealSkin or the session.
/// `None` if the builder fails: the catalog then fails to load rather than falling back to an
/// unrestricted client.
pub fn plain_client() -> Option<reqwest::Client> {
    plain_client_with(true)
}

fn plain_client_with(https_only: bool) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(https_only)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 3 {
                attempt.error("too many redirects")
            } else if attempt.url().scheme() != "https" {
                attempt.error("a redirect left HTTPS")
            } else {
                attempt.follow()
            }
        }))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .ok()
}

/// GETs `url`, giving up past `cap` bytes or after `timeout` in all.
pub async fn fetch_capped(
    client: &reqwest::Client,
    url: Url,
    cap: usize,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let fetch = async {
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|error| error.without_url().to_string())?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > cap as u64)
        {
            return Err("too large".into());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| error.without_url().to_string())?
        {
            if body.len() + chunk.len() > cap {
                return Err("too large".into());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    };
    tokio::time::timeout(timeout, fetch)
        .await
        .map_err(|_| "timed out".to_owned())?
}

// ---- Icons ----------------------------------------------------------------------------

/// Decoded icon pixels: straight (not premultiplied) BGRA, as GPUI's `RenderImage` takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconPixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Raster decode limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RasterLimits {
    pub max_dimension: u32,
    pub max_alloc: u64,
}

/// Icons: a 512 KiB PNG can't expand past 32 MB.
pub const ICON_RASTER_LIMITS: RasterLimits = RasterLimits {
    max_dimension: 2048,
    max_alloc: 32 * 1024 * 1024,
};

/// Decodes raster bytes under `limits` and downscales to `max_edge`, as BGRA.
pub fn decode_raster(bytes: &[u8], max_edge: u32, limits: RasterLimits) -> Option<IconPixels> {
    let mut image_limits = image::Limits::default();
    image_limits.max_image_width = Some(limits.max_dimension);
    image_limits.max_image_height = Some(limits.max_dimension);
    image_limits.max_alloc = Some(limits.max_alloc);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.limits(image_limits);
    let mut decoded = reader.decode().ok()?;
    if decoded.width() > max_edge || decoded.height() > max_edge {
        decoded = decoded.thumbnail(max_edge, max_edge);
    }
    let mut pixels = decoded.into_rgba8();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(IconPixels {
        width: pixels.width(),
        height: pixels.height(),
        bgra: pixels.into_raw(),
    })
}

/// Decodes an icon: SVG through [`decode_svg`], anything else as a raster, into a box
/// [`ICON_EDGE`] pixels on the long edge. More than [`ICON_CAP`] bytes is refused.
pub fn decode_icon(bytes: &[u8], svg: bool) -> Option<IconPixels> {
    if bytes.len() > ICON_CAP || bytes.is_empty() {
        return None;
    }
    if svg {
        decode_svg(bytes)
    } else if bytes.len() <= RASTER_ICON_CAP {
        decode_raster(bytes, ICON_EDGE, ICON_RASTER_LIMITS)
    } else {
        None
    }
}

/// usvg options that read nothing beyond the bytes given: every `<image>` href (paths, URLs,
/// data URIs, nested SVG) resolves to nothing, there is no resources folder, and the font
/// database stays empty (system fonts are never loaded), so text draws nothing.
fn svg_options() -> resvg::usvg::Options<'static> {
    resvg::usvg::Options {
        resources_dir: None,
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        fontdb: Arc::new(resvg::usvg::fontdb::Database::new()),
        ..Default::default()
    }
}

/// Checks the parsed document's cost before usvg converts it: element count, `<use>` count,
/// nesting depth, and the element count once every `<use>` is expanded (a `<use>` bomb).
fn svg_cost_ok(doc: &resvg::usvg::roxmltree::Document) -> bool {
    use resvg::usvg::roxmltree::NodeId;
    let xlink = "http://www.w3.org/1999/xlink";
    let mut elements = 0usize;
    let mut uses = 0usize;
    let mut depth: HashMap<NodeId, usize> = HashMap::new();
    let mut ids: HashMap<&str, NodeId> = HashMap::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        elements += 1;
        if node.tag_name().name() == "use" {
            uses += 1;
        }
        let level = node
            .parent_element()
            .and_then(|parent| depth.get(&parent.id()))
            .map_or(0, |level| level + 1);
        if level > SVG_MAX_DEPTH {
            return false;
        }
        depth.insert(node.id(), level);
        if let Some(id) = node.attribute("id") {
            ids.entry(id).or_insert(node.id());
        }
    }
    if elements > SVG_MAX_ELEMENTS || uses > SVG_MAX_USES {
        return false;
    }
    // Post-order sizes with memoisation; a reference cycle is refused.
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Open,
        Done(u64),
    }
    let mut marks: HashMap<NodeId, Mark> = HashMap::new();
    let deps = |id: NodeId| -> Vec<NodeId> {
        let Some(node) = doc.get_node(id) else {
            return Vec::new();
        };
        let mut deps: Vec<NodeId> = node
            .children()
            .filter(|c| c.is_element())
            .map(|c| c.id())
            .collect();
        if node.tag_name().name() == "use" {
            let href = node
                .attribute("href")
                .or_else(|| node.attribute((xlink, "href")));
            if let Some(target) = href
                .and_then(|h| h.strip_prefix('#'))
                .and_then(|h| ids.get(h))
            {
                deps.push(*target);
            }
        }
        deps
    };
    let root = doc.root_element().id();
    let mut stack = vec![(root, false)];
    while let Some((id, closing)) = stack.pop() {
        if closing {
            let total = deps(id).iter().fold(1u64, |total, dep| {
                let size = match marks.get(dep) {
                    Some(Mark::Done(size)) => *size,
                    _ => SVG_MAX_EXPANDED + 1,
                };
                total.saturating_add(size)
            });
            if total > SVG_MAX_EXPANDED {
                return false;
            }
            marks.insert(id, Mark::Done(total));
            continue;
        }
        match marks.get(&id) {
            Some(Mark::Done(_)) => continue,
            Some(Mark::Open) => return false,
            None => {}
        }
        marks.insert(id, Mark::Open);
        stack.push((id, true));
        for dep in deps(id) {
            match marks.get(&dep) {
                Some(Mark::Done(_)) => {}
                Some(Mark::Open) => return false,
                None => stack.push((dep, false)),
            }
        }
    }
    true
}

/// Refuses, before usvg converts anything, the elements whose cost has no cheap bound:
/// `<marker>` (usvg copies its content once per path vertex while converting) and `<filter>`
/// (primitive chains). App icons with a filter (drop shadows) fall back to their initial.
fn svg_elements_ok(doc: &resvg::usvg::roxmltree::Document) -> bool {
    !doc.descendants()
        .any(|n| n.is_element() && SVG_REFUSED.contains(&n.tag_name().name()))
}

/// The `#id`s `url(...)` references in `text` name, read as svgtypes reads a FuncIRI:
/// whitespace (`' ' \t \n \r`) after `url(`, an optional quote, whitespace again, `#`, then the
/// id up to the closing quote (trimmed) or, unquoted, up to `)` or whitespace. A `url(` that
/// can't be read this way yields `""`, which [`names_server`] treats as a reference: fail
/// closed.
fn url_ids(text: &str) -> Vec<&str> {
    const WHITESPACE: [char; 4] = [' ', '\t', '\n', '\r'];
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("url(") {
        rest = &rest[at + 4..];
        let inner = rest.trim_start_matches(WHITESPACE);
        let (quote, inner) = match inner.chars().next() {
            Some(quote @ ('"' | '\'')) => (Some(quote), &inner[1..]),
            _ => (None, inner),
        };
        let inner = inner.trim_start_matches(WHITESPACE);
        let Some(id) = inner.strip_prefix('#') else {
            ids.push("");
            continue;
        };
        let end = match quote {
            Some(quote) => id.find(quote),
            None => id.find(|c: char| c == ')' || c.is_ascii_whitespace()),
        };
        match end {
            Some(end) => ids.push(id[..end].trim_matches(|c: char| c.is_ascii_whitespace())),
            None => ids.push(""),
        }
    }
    ids
}

/// Whether a `url(...)` id from [`url_ids`] names one of `servers` (or couldn't be read).
fn names_server(id: &str, servers: &HashMap<&str, ()>) -> bool {
    id.is_empty() || servers.contains_key(id)
}

/// The classes whose stylesheet rules reference one of `servers`, and whether a rule without a
/// class selector (`rect`, `*`) does, which then reaches every element.
fn css_server_classes(stylesheet: &str, servers: &HashMap<&str, ()>) -> (HashSet<String>, bool) {
    let mut classes = HashSet::new();
    let mut every = false;
    for rule in stylesheet.split('}') {
        let Some((selectors, body)) = rule.split_once('{') else {
            continue;
        };
        if !url_ids(body).iter().any(|id| names_server(id, servers)) {
            continue;
        }
        for selector in selectors.split(',') {
            // Only plain class selectors (`.a`, `.a.b`, `.a .b`) are resolved; any other
            // selector (`rect`, `#id`, `*`, `[x]`, `g > .a`) is taken to reach every element.
            let plain = selector.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ' | '\t' | '\n' | '\r')
            });
            let mut named = false;
            for part in selector.split('.').skip(1) {
                let name: String = part
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                    .collect();
                if !name.is_empty() {
                    named = true;
                    classes.insert(name);
                }
            }
            every |= !plain || (!named && !selector.trim().is_empty());
        }
    }
    (classes, every)
}

/// Refuses, before usvg converts anything (its conversion of nested masks and clip paths is
/// itself exponential), a mask, clip path or pattern that reaches another: through its own
/// attributes, its content, a `<use>` inside it, a stylesheet rule, or what its ancestors
/// pass down (an inherited `fill: url(#p)`). Also bounds how many there are.
fn svg_nesting_ok(doc: &resvg::usvg::roxmltree::Document) -> bool {
    use resvg::usvg::roxmltree::{Node, NodeId};
    let xlink = "http://www.w3.org/1999/xlink";
    let mut servers: HashMap<&str, ()> = HashMap::new();
    let mut roots: Vec<NodeId> = Vec::new();
    let mut ids: HashMap<&str, NodeId> = HashMap::new();
    let mut stylesheet = String::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        let tag = node.tag_name().name();
        if tag == "style" {
            stylesheet.extend(node.descendants().filter_map(|n| n.text()));
        }
        // CSS escapes (`url(#n\30)`, `.c\31`) would hide a reference from these checks.
        if node
            .attribute("style")
            .is_some_and(|style| style.contains('\\'))
        {
            return false;
        }
        if let Some(id) = node.attribute("id") {
            ids.entry(id).or_insert(node.id());
            if SVG_NESTING.contains(&tag) {
                servers.insert(id, ());
                roots.push(node.id());
            }
        }
    }
    if stylesheet.contains('\\') || stylesheet.to_ascii_lowercase().contains("@import") {
        return false;
    }
    if roots.len() > SVG_MAX_PAINT_SERVERS {
        return false;
    }
    if roots.is_empty() {
        return true;
    }
    // Uses by attribute (the worker's tree check also counts stylesheet uses).
    let uses: usize = doc
        .descendants()
        .filter(|n| n.is_element())
        .flat_map(|n| n.attributes())
        .map(|a| {
            url_ids(a.value())
                .iter()
                .filter(|id| names_server(id, &servers))
                .count()
        })
        .sum();
    if uses > SVG_MAX_PAINT_REFS {
        return false;
    }
    let (classes, every) = css_server_classes(&stylesheet, &servers);
    let href = |node: Node| -> Option<NodeId> {
        let target = node
            .attribute("href")
            .or_else(|| node.attribute((xlink, "href")))?
            .strip_prefix('#')?;
        ids.get(target).copied()
    };
    let reaches = |node: Node| -> bool {
        every
            || node.attributes().any(|a| {
                url_ids(a.value())
                    .iter()
                    .any(|id| names_server(id, &servers))
            })
            || node
                .attribute("class")
                .is_some_and(|names| names.split_whitespace().any(|n| classes.contains(n)))
            || href(node).is_some_and(|target| {
                doc.get_node(target)
                    .is_some_and(|t| SVG_NESTING.contains(&t.tag_name().name()))
            })
    };
    for &root in &roots {
        let Some(server) = doc.get_node(root) else {
            continue;
        };
        // Everything the server's ancestors could pass down, by attribute, `style`, or a
        // stylesheet class: the same rule as inside it.
        if server
            .ancestors()
            .skip(1)
            .filter(|n| n.is_element())
            .any(reaches)
        {
            return false;
        }
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if seen.len() > SVG_MAX_EXPANDED as usize {
                return false;
            }
            let Some(node) = doc.get_node(id) else {
                continue;
            };
            if id != root && SVG_NESTING.contains(&node.tag_name().name()) {
                return false;
            }
            if reaches(node) {
                return false;
            }
            stack.extend(node.children().filter(|c| c.is_element()).map(|c| c.id()));
            if node.tag_name().name() == "use" {
                stack.extend(href(node));
            }
        }
    }
    true
}

/// Bounds the render work in the converted tree, which resvg can't be stopped in once it
/// starts: no filter survived, at most [`SVG_MAX_PAINT_REFS`] uses of masks, clip paths and
/// patterns, and none of them reaches another (a clip on a clip, a mask on a mask, or anything
/// inside one that is itself clipped, masked or pattern-filled). Each nesting level multiplies
/// the work, so a few KB would otherwise burn a core for minutes. usvg has already resolved
/// stylesheets, inheritance and `<use>`, so this reads what will actually render.
fn svg_tree_ok(tree: &resvg::usvg::Tree) -> bool {
    use resvg::usvg::{Group, Node};
    if !tree.filters().is_empty() {
        return false;
    }
    /// Every node under `group`, its sub-roots not included.
    fn each(group: &Group, f: &mut dyn FnMut(&Node) -> bool) -> bool {
        let mut stack = vec![group];
        while let Some(group) = stack.pop() {
            for node in group.children() {
                if !f(node) {
                    return false;
                }
                if let Node::Group(child) = node {
                    stack.push(child);
                }
            }
        }
        true
    }
    let nests = |node: &Node| -> bool {
        let mut any = false;
        node.subroots(|_| any = true);
        if let Node::Group(group) = node {
            any |= group
                .clip_path()
                .is_some_and(|clip| clip.clip_path().is_some())
                || group.mask().is_some_and(|mask| mask.mask().is_some());
        }
        any
    };
    let mut uses = 0usize;
    let mut checked: HashSet<*const Group> = HashSet::new();
    each(tree.root(), &mut |node| {
        if let Node::Group(group) = node {
            if group
                .clip_path()
                .is_some_and(|clip| clip.clip_path().is_some())
                || group.mask().is_some_and(|mask| mask.mask().is_some())
            {
                return false;
            }
        }
        let mut fine = true;
        node.subroots(|sub| {
            uses += 1;
            if uses > SVG_MAX_PAINT_REFS {
                fine = false;
            }
            if fine && checked.insert(sub as *const Group) {
                fine = each(sub, &mut |inner| !nests(inner));
            }
        });
        fine
    })
}

/// The parsing options for an icon's text, or `None` when its DTD could define entities: an
/// `<!ENTITY` anywhere, or a DOCTYPE with an internal subset (`[`), is refused, since entities
/// expand without bound before any other check runs. An external-only DOCTYPE (`<!DOCTYPE svg
/// PUBLIC "…" "…dtd">`) is harmless: nothing external is ever resolved.
fn svg_parsing<'a>(text: &'a str) -> Option<resvg::usvg::roxmltree::ParsingOptions<'a>> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("<!entity") {
        return None;
    }
    let doctype = match lower.find("<!doctype") {
        Some(at) => {
            let declaration = &lower[at..];
            let end = declaration.find('>')?;
            if declaration[..end].contains('[') {
                return None;
            }
            true
        }
        None => false,
    };
    Some(resvg::usvg::roxmltree::ParsingOptions {
        allow_dtd: doctype,
        nodes_limit: SVG_MAX_NODES,
        entity_resolver: None,
    })
}

/// CSS declarations with every `filter` property dropped (a rule body or a `style` value).
fn css_without_filter(declarations: &str) -> String {
    declarations
        .split(';')
        .filter(|declaration| {
            declaration
                .split_once(':')
                .is_none_or(|(property, _)| !property.trim().eq_ignore_ascii_case("filter"))
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// A stylesheet with every `filter` property dropped from its rule bodies.
fn stylesheet_without_filter(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..=open]);
        rest = &rest[open + 1..];
        let close = rest.find('}').unwrap_or(rest.len());
        out.push_str(&css_without_filter(&rest[..close]));
        rest = &rest[close..];
    }
    out.push_str(rest);
    out
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The icon's text with every `<filter>` element, `filter=` attribute and `filter` style
/// property removed, so resvg never runs a filter chain; `Some(None)` when there was none,
/// `None` when the text can't be rewritten safely.
fn without_filters(text: &str, doc: &resvg::usvg::roxmltree::Document) -> Option<Option<String>> {
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        match node.tag_name().name() {
            "filter" => {
                edits.push((node.range(), String::new()));
                continue;
            }
            "style" => {
                let css: String = node.descendants().filter_map(|n| n.text()).collect();
                if css.to_ascii_lowercase().contains("filter") {
                    let cleaned = stylesheet_without_filter(&css);
                    if cleaned.contains("]]>") {
                        return None;
                    }
                    edits.push((
                        node.range(),
                        format!("<style><![CDATA[{cleaned}]]></style>"),
                    ));
                }
            }
            _ => {}
        }
        for attribute in node.attributes() {
            if attribute.name() == "filter" {
                edits.push((attribute.range(), String::new()));
            } else if attribute.name() == "style"
                && attribute.value().to_ascii_lowercase().contains("filter")
            {
                edits.push((
                    attribute.range_value(),
                    xml_escape(&css_without_filter(attribute.value())),
                ));
            }
        }
    }
    if edits.is_empty() {
        return Some(None);
    }
    // Document order; anything inside an edit already made (a filter's own content) goes.
    edits.sort_by_key(|(range, _)| (range.start, std::cmp::Reverse(range.end)));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (range, replacement) in edits {
        if range.start < at {
            continue;
        }
        out.push_str(text.get(at..range.start)?);
        out.push_str(&replacement);
        at = range.end;
    }
    out.push_str(text.get(at..)?);
    Some(Some(out))
}

/// Rasterizes an SVG icon into a box [`ICON_EDGE`] pixels on the long edge, with no external
/// resources ([`svg_options`]), after [`svg_cost_ok`], [`svg_elements_ok`] and [`svg_tree_ok`]. Gzip (SVGZ) and any
/// DTD are refused: their expansion would escape the byte cap.
///
/// The cheap checks run here; usvg and resvg run only in a short-lived worker process
/// ([`SVG_WORKER_FLAG`]) under CPU and memory limits and a wall timeout, so a bypass of the
/// checks costs at most one killed child, never the app.
pub fn decode_svg(bytes: &[u8]) -> Option<IconPixels> {
    svg_checked_text(bytes)?;
    rasterize_in_worker(bytes, false)
}

/// The icon's text once it passes every check before usvg: the byte cap, no gzip, UTF-8, no
/// entities ([`svg_parsing`]), [`svg_cost_ok`], filters removed ([`without_filters`]), and
/// then on the rewritten text [`svg_cost_ok`], [`svg_elements_ok`] and [`svg_nesting_ok`].
fn svg_checked_text(bytes: &[u8]) -> Option<String> {
    use resvg::usvg::roxmltree;
    if bytes.len() > ICON_CAP || bytes.starts_with(&[0x1f, 0x8b]) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let stripped = {
        let doc = roxmltree::Document::parse_with_options(text, svg_parsing(text)?).ok()?;
        if !svg_cost_ok(&doc) {
            return None;
        }
        // Filters are removed, not refused: the icon renders without its drop shadow.
        without_filters(text, &doc)?
    };
    let text = stripped.unwrap_or_else(|| text.to_owned());
    let doc = roxmltree::Document::parse_with_options(&text, svg_parsing(&text)?).ok()?;
    if !svg_cost_ok(&doc) || !svg_elements_ok(&doc) || !svg_nesting_ok(&doc) {
        return None;
    }
    drop(doc);
    Some(text)
}

/// Converts and renders checked icon text (in the worker process).
fn render_svg(text: &str) -> Option<IconPixels> {
    use resvg::tiny_skia::{Pixmap, Transform};
    use resvg::usvg::roxmltree;
    let doc = roxmltree::Document::parse_with_options(text, svg_parsing(text)?).ok()?;
    let doc = &doc;
    let tree = resvg::usvg::Tree::from_xmltree(doc, &svg_options()).ok()?;
    if !svg_tree_ok(&tree) {
        return None;
    }
    let size = tree.size();
    let (width, height) = (size.width(), size.height());
    let sane = |v: f32| v.is_finite() && v > 0. && v <= SVG_MAX_SIZE;
    if !sane(width) || !sane(height) {
        return None;
    }
    let scale = ICON_EDGE as f32 / width.max(height);
    let pixel_width = ((width * scale).round() as u32).clamp(1, ICON_EDGE);
    let pixel_height = ((height * scale).round() as u32).clamp(1, ICON_EDGE);
    let mut pixmap = Pixmap::new(pixel_width, pixel_height)?;
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut bgra = pixmap.take();
    for pixel in bgra.as_chunks_mut::<4>().0 {
        // Premultiplied RGBA to straight BGRA, as gpui's `swap_rgba_pa_to_bgra`.
        pixel.swap(0, 2);
        let alpha = pixel[3];
        if alpha > 0 && alpha < 255 {
            let a = f32::from(alpha) / 255.;
            for channel in &mut pixel[..3] {
                *channel = (f32::from(*channel) / a).round().min(255.) as u8;
            }
        }
    }
    Some(IconPixels {
        width: pixel_width,
        height: pixel_height,
        bgra,
    })
}

// ---- SVG worker process -----------------------------------------------------------------

/// The hidden argument that makes the app binary an SVG worker: read SVG bytes on stdin,
/// write the icon's pixels on stdout, exit. `main` checks it before GPUI or Tokio start.
pub const SVG_WORKER_FLAG: &str = "--skerry-svg-worker";
/// What precedes the pixels on the worker's stdout.
const WORKER_MAGIC: &[u8; 6] = b"SKSVG1";
/// The parent kills a worker still running after this.
const WORKER_WALL: Duration = Duration::from_secs(3);
/// The worker's own limits, set before it parses anything.
const WORKER_CPU_SECONDS: u64 = 2;
const WORKER_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
/// Workers running at once, app-wide.
const WORKER_SLOTS: usize = 2;

static WORKERS: (std::sync::Mutex<usize>, std::sync::Condvar) =
    (std::sync::Mutex::new(0), std::sync::Condvar::new());

/// One of the [`WORKER_SLOTS`], released on drop.
struct WorkerSlot;

impl WorkerSlot {
    fn acquire() -> Self {
        let (lock, freed) = &WORKERS;
        let mut running = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *running >= WORKER_SLOTS {
            running = freed
                .wait(running)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *running += 1;
        WorkerSlot
    }
}

impl Drop for WorkerSlot {
    fn drop(&mut self) {
        let (lock, freed) = &WORKERS;
        let mut running = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *running = running.saturating_sub(1);
        freed.notify_one();
    }
}

/// The worker's body. It touches no file and no network: stdin in, stdout out. Returns the
/// process exit code.
pub fn svg_worker_main() -> i32 {
    use std::io::{Read as _, Write as _};
    limit_worker();
    let mut input = Vec::new();
    if std::io::stdin()
        .lock()
        .take(ICON_CAP as u64 + 1)
        .read_to_end(&mut input)
        .is_err()
    {
        return 2;
    }
    let Some(pixels) = rasterize_svg(&input, !worker_unchecked()) else {
        return 1;
    };
    let mut out = Vec::with_capacity(WORKER_MAGIC.len() + 8 + pixels.bgra.len());
    out.extend_from_slice(WORKER_MAGIC);
    out.extend_from_slice(&pixels.width.to_le_bytes());
    out.extend_from_slice(&pixels.height.to_le_bytes());
    out.extend_from_slice(&pixels.bgra);
    let mut stdout = std::io::stdout().lock();
    if stdout.write_all(&out).and_then(|_| stdout.flush()).is_err() {
        return 3;
    }
    0
}

/// The worker in tests may skip the checks, to prove the limits alone stop a bomb.
fn worker_unchecked() -> bool {
    cfg!(test) && std::env::var_os("SKERRY_SVG_UNCHECKED").is_some()
}

/// CPU seconds (SIGXCPU, then SIGKILL a second later) and address space, best effort: macOS
/// may not enforce every memory limit, and the parent's wall timeout backs both up.
fn limit_worker() {
    let set = |resource: libc::c_int, soft: u64, hard: u64| {
        let limit = libc::rlimit {
            rlim_cur: soft as libc::rlim_t,
            rlim_max: hard as libc::rlim_t,
        };
        // SAFETY: a plain syscall on a valid, initialised struct.
        unsafe { libc::setrlimit(resource, &limit) };
    };
    set(libc::RLIMIT_CPU, WORKER_CPU_SECONDS, WORKER_CPU_SECONDS + 1);
    set(libc::RLIMIT_AS, WORKER_MEMORY_BYTES, WORKER_MEMORY_BYTES);
    set(libc::RLIMIT_DATA, WORKER_MEMORY_BYTES, WORKER_MEMORY_BYTES);
}

/// Rasterizes in this process: the worker's job.
fn rasterize_svg(bytes: &[u8], checked: bool) -> Option<IconPixels> {
    if checked {
        render_svg(&svg_checked_text(bytes)?)
    } else {
        render_svg(std::str::from_utf8(bytes).ok()?)
    }
}

/// The command that starts a worker: this same binary with [`SVG_WORKER_FLAG`] (inside the
/// `.app` too: `current_exe` is `Contents/MacOS/<name>`), with an empty environment.
#[cfg(not(test))]
fn worker_command() -> Option<std::process::Command> {
    let mut command = std::process::Command::new(std::env::current_exe().ok()?);
    command.arg(SVG_WORKER_FLAG).env_clear();
    Some(command)
}

/// Under `cargo test` the binary is the test harness: it runs the one test that turns into a
/// worker.
#[cfg(test)]
fn worker_command() -> Option<std::process::Command> {
    let mut command = std::process::Command::new(std::env::current_exe().ok()?);
    command
        .args([
            "--exact",
            "apps::tests::svg_worker_entry",
            "--nocapture",
            "--test-threads=1",
            "-q",
        ])
        .env_clear()
        .env("SKERRY_SVG_WORKER", "1");
    Some(command)
}

/// Runs one worker on `bytes`: at most [`WORKER_SLOTS`] at once, killed after
/// [`WORKER_WALL`]. A crash, kill, timeout or bad answer is `None` (the initial tile).
fn rasterize_in_worker(bytes: &[u8], unchecked: bool) -> Option<IconPixels> {
    use std::io::{Read as _, Write as _};
    use std::process::Stdio;
    let _slot = WorkerSlot::acquire();
    let mut command = worker_command()?;
    if unchecked && cfg!(test) {
        command.env("SKERRY_SVG_UNCHECKED", "1");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input = bytes.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let stdout = child.stdout.take()?;
    // The pixels, plus room for the test harness's own banner.
    let most = (WORKER_MAGIC.len() + 8 + (ICON_EDGE * ICON_EDGE * 4) as usize + 4096) as u64;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.take(most).read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + WORKER_WALL;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let _ = writer.join();
    let out = reader.join().ok()?;
    if !status?.success() {
        return None;
    }
    worker_pixels(&out)
}

/// The pixels in a worker's stdout, checked against the icon box.
fn worker_pixels(out: &[u8]) -> Option<IconPixels> {
    let at = out
        .windows(WORKER_MAGIC.len())
        .position(|window| window == WORKER_MAGIC)?;
    let body = &out[at + WORKER_MAGIC.len()..];
    let width = u32::from_le_bytes(body.get(0..4)?.try_into().ok()?);
    let height = u32::from_le_bytes(body.get(4..8)?.try_into().ok()?);
    if width == 0 || height == 0 || width > ICON_EDGE || height > ICON_EDGE {
        return None;
    }
    let bgra = body.get(8..8 + (width * height * 4) as usize)?.to_vec();
    Some(IconPixels {
        width,
        height,
        bgra,
    })
}

/// The window's icon loads: deduplicated, at most [`ICON_FETCHES`] in flight, and a
/// generation that a source change or Retry bumps so late results are dropped (`ARCH-014`).
#[derive(Debug, Default)]
pub struct IconQueue {
    generation: u64,
    queued: VecDeque<String>,
    seen: HashSet<String>,
    in_flight: usize,
    /// Loads of earlier generations still running: they keep their fetch slots until they
    /// land, so a reset never lets more than [`ICON_FETCHES`] run.
    stale: usize,
}

impl IconQueue {
    /// Asks for `icon`; `false` when it was already asked for in this generation.
    pub fn want(&mut self, icon: &str) -> bool {
        if !self.seen.insert(icon.to_owned()) {
            return false;
        }
        self.queued.push_back(icon.to_owned());
        true
    }

    /// The next icon to fetch, with its generation, while a fetch slot is free.
    pub fn next(&mut self) -> Option<(String, u64)> {
        if self.in_flight + self.stale >= ICON_FETCHES {
            return None;
        }
        let icon = self.queued.pop_front()?;
        self.in_flight += 1;
        Some((icon, self.generation))
    }

    /// A load of `generation` ended; `true` when its result is still wanted.
    pub fn finished(&mut self, generation: u64) -> bool {
        if generation != self.generation {
            self.stale = self.stale.saturating_sub(1);
            return false;
        }
        self.in_flight = self.in_flight.saturating_sub(1);
        true
    }

    /// Lets `icon` be asked for again (a load that failed only for want of the session).
    pub fn forget(&mut self, icon: &str) {
        self.seen.remove(icon);
    }

    /// The source changed or Retry was pressed: everything in flight is stale.
    pub fn reset(&mut self) {
        self.generation += 1;
        self.queued.clear();
        self.seen.clear();
        self.stale += self.in_flight;
        self.in_flight = 0;
    }
}

/// Whether the console should follow its end: within 8 pt of the bottom.
pub fn follows_end(offset_y: f32, max_y: f32) -> bool {
    max_y + offset_y <= 8.
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(id: u64, app: &str, state: &str) -> Value {
        json!({"id": id, "app": app, "op": "install", "state": state,
               "phase": null, "progress": null, "code": null, "reset": true,
               "lines": ["a"], "current": ""})
    }

    #[test]
    fn requests_match_the_web_contract() {
        let parsed = |text: Option<String>| -> Value {
            serde_json::from_str(text.unwrap().strip_prefix("apps,").unwrap()).unwrap()
        };
        assert_eq!(
            request(Op::Install, Some("firefox"), None).as_deref(),
            Some(r#"apps,{"op":"install","app":"firefox"}"#)
        );
        assert_eq!(
            request(Op::Cancel, None, Some(7)).as_deref(),
            Some(r#"apps,{"op":"cancel","id":7}"#)
        );
        assert_eq!(
            request(Op::Clear, None, None).as_deref(),
            Some(r#"apps,{"op":"clear"}"#)
        );
        assert_eq!(
            request(Op::Clear, None, Some(3)).as_deref(),
            Some(r#"apps,{"op":"clear","id":3}"#)
        );
        assert_eq!(
            request(Op::List, Some("ignored"), Some(1)).as_deref(),
            Some(r#"apps,{"op":"list"}"#)
        );
        assert_eq!(request(Op::Launch, None, None), None);
        assert_eq!(request(Op::Cancel, None, None), None);
        assert_eq!(
            parsed(request(Op::Launch, Some("ghcr.io/x/y:1"), None)),
            json!({"op": "launch", "app": "ghcr.io/x/y:1"})
        );
    }

    #[test]
    fn invalid_names_are_never_sent() {
        let long = "a".repeat(201);
        for bad in ["", "all", "-rf", long.as_str(), "a b", "a;b", "é", "a\"b"] {
            assert!(!valid_app_name(bad), "{bad:?}");
            for op in [Op::Install, Op::Remove, Op::Update, Op::Launch] {
                assert_eq!(request(op, Some(bad), None), None, "{bad:?}");
            }
        }
        for good in [
            "firefox",
            "ghcr.io/linuxserver/proot-apps:firefox",
            "86box",
            &"a".repeat(200),
        ] {
            assert!(valid_app_name(good), "{good:?}");
        }
    }

    #[test]
    fn policy_needs_command_enabled_and_the_sidebar_toggle() {
        let allowed = |v: Value| Policy::from_server(&v).allowed;
        assert!(allowed(json!({"command_enabled": {"value": true}})));
        assert!(!allowed(json!({"command_enabled": {"value": true},
                                "ui_sidebar_show_apps": {"value": false}})));
        assert!(allowed(json!({"command_enabled": {"value": true},
                               "ui_sidebar_show_apps": {"value": true}})));
        assert!(!allowed(json!({"command_enabled": {"value": false}})));
        assert!(!allowed(json!({})));
        assert!(!allowed(json!({"command_enabled": {"value": "true"}})));
        assert!(Policy::from_server(&json!({"apps_local_repo": {"value": true}})).local_repo);
        assert!(!Policy::from_server(&json!({"apps_local_repo": {"value": "true"}})).local_repo);
    }

    #[test]
    fn nothing_is_sent_while_disconnected_or_not_allowed() {
        let on = Policy {
            allowed: true,
            local_repo: false,
        };
        let off = Policy {
            allowed: false,
            local_repo: false,
        };
        assert!(can_send(true, Some(on)));
        assert!(!can_send(false, Some(on)));
        assert!(!can_send(true, Some(off)));
        assert!(!can_send(true, None));
        let mut link = Link::default();
        assert!(!link.can_send());
        link.on_policy(off);
        assert!(!link.can_send());
        link.on_policy(on);
        assert!(link.can_send());
        link.on_disconnect();
        assert!(!link.can_send(), "disconnected");
    }

    #[test]
    fn the_source_moves_when_the_local_repository_flips() {
        let on = Policy {
            allowed: true,
            local_repo: false,
        };
        let mut link = Link::default();
        assert!(link.on_policy(on).changed);
        assert!(
            !link.on_policy(on).changed,
            "a re-broadcast changes nothing"
        );
        let local = Policy {
            local_repo: true,
            ..on
        };
        assert!(link.on_policy(local).source_changed);
        assert!(!link.on_policy(local).source_changed);
    }

    #[test]
    fn one_probe_list_per_stream_on_its_first_allowed_policy() {
        let on = Policy {
            allowed: true,
            local_repo: false,
        };
        let off = Policy {
            allowed: false,
            ..on
        };
        let now = Instant::now();
        let mut probe = Probe::default();
        assert_eq!(probe.on_policy(off, now), None, "not allowed: no list");
        let first = probe
            .on_policy(on, now)
            .expect("the first allowed policy lists");
        // A re-broadcast `server_settings` (an encoder change) lists nothing.
        assert_eq!(probe.on_policy(on, now), None);
        assert!(!probe.supported() && !probe.unsupported(), "probing");
        assert!(probe.on_answer());
        assert!(probe.supported());
        assert!(
            !probe.on_timeout(first, now + PROBE_TIMEOUT),
            "answered in time"
        );
        assert_eq!(probe.on_policy(on, now), None);
        // Reconnect: exactly one list again.
        probe.reset();
        assert!(!probe.supported());
        assert!(probe.on_policy(on, now).is_some());
        assert_eq!(probe.on_policy(on, now), None);
    }

    #[test]
    fn an_early_probe_timer_rearms_for_the_rest() {
        let on = Policy {
            allowed: true,
            local_repo: false,
        };
        let now = Instant::now();
        let mut probe = Probe::default();
        let generation = probe.on_policy(on, now).unwrap();
        let early = now + Duration::from_millis(4_990);
        assert!(!probe.on_timeout(generation, early));
        assert_eq!(
            probe.remaining(generation, early),
            Some(Duration::from_millis(10))
        );
        assert!(probe.on_timeout(generation, early + Duration::from_millis(10)));
        assert_eq!(probe.remaining(generation, early), None, "settled");
        assert_eq!(
            probe.remaining(generation + 1, now),
            None,
            "another stream's timer"
        );
    }

    #[test]
    fn a_selkies_that_never_answers_list_is_unsupported() {
        let on = Policy {
            allowed: true,
            local_repo: false,
        };
        let now = Instant::now();
        let mut probe = Probe::default();
        let generation = probe.on_policy(on, now).unwrap();
        assert!(!probe.on_timeout(generation, now + Duration::from_millis(4_999)));
        assert!(probe.on_timeout(generation, now + PROBE_TIMEOUT));
        assert!(probe.unsupported() && !probe.supported());
        assert!(!probe.on_timeout(generation, now + PROBE_TIMEOUT), "once");
        // A late answer still counts.
        assert!(probe.on_answer());
        assert!(probe.supported());
        // A timer from before a reconnect never marks the new stream.
        probe.reset();
        let fresh = probe.on_policy(on, now + PROBE_TIMEOUT).unwrap();
        assert!(!probe.on_timeout(generation, now + PROBE_TIMEOUT * 3));
        assert_eq!(
            probe.support(),
            Support::Probing {
                since: now + PROBE_TIMEOUT,
                generation: fresh
            }
        );
        // Detection resets on reconnect, unsupported included.
        assert!(probe.on_timeout(fresh, now + PROBE_TIMEOUT * 2));
        probe.reset();
        assert_eq!(probe.support(), Support::Unknown);
    }

    #[test]
    fn state_replaces_jobs_and_a_null_installed_keeps_the_last_set() {
        let mut board = Board::default();
        board.apply_state(
            &json!({"installed": ["firefox", "gimp"], "jobs": [job(1, "gimp", "done")]}),
        );
        assert!(board.is_installed("firefox"));
        assert_eq!(board.jobs.len(), 1);
        board.apply_state(&json!({"installed": null, "jobs": [job(2, "vlc", "running")]}));
        assert!(board.is_installed("gimp"), "null keeps the last set");
        assert_eq!(board.jobs.len(), 1);
        assert_eq!(board.jobs[0].id, 2);
        board.apply_state(&json!({"installed": [], "jobs": []}));
        assert_eq!(board.installed, Some(BTreeSet::new()));
        assert!(board.jobs.is_empty());
    }

    #[test]
    fn an_unknown_job_is_added_in_request_order() {
        let mut board = Board::default();
        board.apply_job(&job(3, "a", "queued"));
        board.apply_job(&job(4, "b", "queued"));
        let ids: Vec<u64> = board.jobs.iter().map(|j| j.id).collect();
        assert_eq!(ids, [3, 4]);
    }

    #[test]
    fn an_unknown_job_without_reset_is_still_built() {
        let mut board = Board::default();
        // A finish can arrive for a job this window never saw.
        board.apply_job(
            &json!({"id": 9, "app": "vlc", "op": "remove", "state": "done",
                                "phase": null, "progress": null, "code": 0,
                                "append": ["bye"], "current": ""}),
        );
        let job = &board.jobs[0];
        assert_eq!(
            (job.id, job.op, job.state),
            (9, JobOp::Remove, JobState::Done)
        );
        assert_eq!(job.code, Some(0));
        assert!(!board.apply_job(&json!({"app": "x"})), "no id");
    }

    #[test]
    fn a_reset_replaces_the_console() {
        let mut board = Board::default();
        board.apply_job(&job(1, "a", "running"));
        board.apply_job(&json!({"id": 1, "append": ["b", "c"], "current": "d"}));
        assert_eq!(board.jobs[0].console_text(), "a\nb\nc\nd");
        board.apply_job(
            &json!({"id": 1, "app": "a", "op": "install", "state": "running",
                                "reset": true, "lines": ["x"], "current": ""}),
        );
        assert_eq!(board.jobs[0].console_text(), "x");
    }

    #[test]
    fn an_append_extends_the_console_and_caps_it_at_200_lines() {
        let mut board = Board::default();
        let first: Vec<String> = (0..150).map(|i| format!("line {i}")).collect();
        board.apply_job(&json!({"id": 1, "app": "a", "reset": true, "lines": first}));
        let more: Vec<String> = (150..250).map(|i| format!("line {i}")).collect();
        board.apply_job(&json!({"id": 1, "append": more}));
        let lines = &board.jobs[0].lines;
        assert_eq!(lines.len(), CONSOLE_LINES);
        assert_eq!(lines.front().map(String::as_str), Some("line 50"));
        assert_eq!(lines.back().map(String::as_str), Some("line 249"));
    }

    #[test]
    fn a_partial_update_keeps_missing_fields_and_clears_explicit_nulls() {
        let mut board = Board::default();
        board.apply_job(&job(1, "a", "running"));
        board.apply_job(&json!({"id": 1, "phase": "download", "progress": 45.0}));
        assert_eq!(board.jobs[0].progress, Some(45.0));
        board.apply_job(&json!({"id": 1, "current": "45%"}));
        assert_eq!(board.jobs[0].progress, Some(45.0), "missing keeps it");
        assert_eq!(board.jobs[0].phase.as_deref(), Some("download"));
        board.apply_job(&json!({"id": 1, "progress": null, "phase": "install"}));
        assert_eq!(board.jobs[0].progress, None, "null clears it");
        assert_eq!(board.jobs[0].phase.as_deref(), Some("install"));
    }

    #[test]
    fn job_for_app_prefers_the_pending_job_then_the_last_finished() {
        let mut board = Board::default();
        board.apply_job(&job(1, "a", "failed"));
        board.apply_job(&job(2, "a", "queued"));
        board.apply_job(&job(3, "a", "done"));
        assert_eq!(board.job_for_app("a").map(|j| j.id), Some(2));
        board.apply_job(&json!({"id": 2, "state": "canceled"}));
        assert_eq!(board.job_for_app("a").map(|j| j.id), Some(3));
        assert!(board.job_for_app("b").is_none());
        assert!(board.jobs.iter().all(|job| !job.pending()));
    }

    #[test]
    fn a_request_is_answered_only_by_a_pending_or_newer_job() {
        let mut board = Board::default();
        board.apply_job(&job(5, "gimp", "done"));
        let requested = Requested {
            op: Op::Update,
            at: Instant::now(),
            after_id: board.max_id(),
        };
        assert!(
            !requested.answered_by(&board, "gimp"),
            "a stale finished job"
        );
        // An `apps_state` re-sending the same finished job still doesn't.
        board.apply_state(&json!({"installed": ["gimp"], "jobs": [job(5, "gimp", "done")]}));
        assert!(!requested.answered_by(&board, "gimp"));
        // Another app's pending job doesn't either.
        board.apply_job(&job(6, "vlc", "queued"));
        assert!(!requested.answered_by(&board, "gimp"));
        board.apply_job(&job(7, "gimp", "queued"));
        assert!(requested.answered_by(&board, "gimp"));
        // A brand-new job that already finished answers too.
        let mut quick = Board::default();
        quick.apply_job(&job(8, "gimp", "done"));
        let early = Requested {
            after_id: 7,
            ..requested
        };
        assert!(early.answered_by(&quick, "gimp"));
    }

    #[test]
    fn an_unanswered_request_times_out_after_5_seconds() {
        let at = Instant::now();
        assert!(!requested_expired(at + Duration::from_millis(4_999), at));
        assert!(requested_expired(at + REQUEST_TIMEOUT, at));
    }

    #[test]
    fn a_launch_error_shows_only_within_15_seconds_of_a_launch() {
        let at = Instant::now();
        assert!(!launch_error_applies(None, at));
        assert!(launch_error_applies(Some(at), at + Duration::from_secs(15)));
        assert!(!launch_error_applies(
            Some(at),
            at + Duration::from_millis(15_001)
        ));
    }

    #[test]
    fn system_actions_parse_into_apps_events() {
        assert!(matches!(
            parse_action(r#"apps_state,{"installed":null,"jobs":[]}"#),
            Some(Event::State(_))
        ));
        assert!(matches!(
            parse_action(r#"apps_job,{"id":1}"#),
            Some(Event::Job(_))
        ));
        assert_eq!(
            parse_action("command_error,boom"),
            Some(Event::CommandError("boom".into()))
        );
        assert_eq!(
            parse_action("command_error,no such app, really: a,b"),
            Some(Event::CommandError("no such app, really: a,b".into())),
            "commas stay in the text"
        );
        assert_eq!(parse_action("rumble,1,0.5,0.5,200"), None);
        assert_eq!(parse_action("apps_job,{bad"), None);
        assert_eq!(parse_action("apps_job,[1]"), None);
        assert_eq!(parse_action("apps_other,{}"), None);
        assert_eq!(parse_action("apps_state"), None);
    }

    #[test]
    fn status_text_follows_phase_progress_and_outcome() {
        let base = Job::from_payload(&job(1, "a", "queued")).unwrap();
        let with = |state, op, phase: Option<&str>, progress, code| Job {
            state,
            op,
            phase: phase.map(str::to_owned),
            progress,
            code,
            ..base.clone()
        };
        use JobOp::*;
        use JobState::*;
        let cases = [
            (
                with(Queued, Install, None, None, None),
                "Waiting to install…",
            ),
            (with(Queued, Update, None, None, None), "Waiting to update…"),
            (with(Queued, Remove, None, None, None), "Waiting to remove…"),
            (
                with(Running, Install, Some("download"), Some(44.6), None),
                "Downloading 45%",
            ),
            (
                with(Running, Install, Some("download"), None, None),
                "Downloading…",
            ),
            (
                with(Running, Update, Some("install"), None, None),
                "Installing…",
            ),
            (
                with(Running, Remove, Some("remove"), None, None),
                "Removing…",
            ),
            (with(Running, Install, None, None, None), "Installing…"),
            (
                with(Running, Update, Some("other"), None, None),
                "Updating…",
            ),
            (with(Running, Remove, None, None, None), "Removing…"),
            (with(Done, Install, None, None, Some(0)), "Installed"),
            (with(Done, Update, None, None, Some(0)), "Updated"),
            (with(Done, Remove, None, None, Some(0)), "Removed"),
            (
                with(Failed, Install, None, None, Some(1)),
                "Failed (exit 1)",
            ),
            (
                with(Failed, Install, None, None, Some(-15)),
                "Failed (exit -15)",
            ),
            (with(Failed, Install, None, None, None), "Failed"),
            (with(Canceled, Install, None, None, None), "Canceled"),
        ];
        for (job, text) in cases {
            assert_eq!(status_text(&job, false), text, "{job:?}");
        }
        let running = with(Running, Install, Some("download"), Some(45.), None);
        assert_eq!(status_text(&running, true), "Canceling…");
        assert_eq!(download_progress(&running), Some(45.));
        let done = with(Done, Install, None, None, Some(0));
        assert_eq!(
            status_text(&done, true),
            "Installed",
            "a finished job is past canceling"
        );
        assert_eq!(download_progress(&done), None);
    }

    #[test]
    fn buttons_keep_the_pending_jobs_layout() {
        let base = Job::from_payload(&job(1, "a", "running")).unwrap();
        let install = Job {
            op: JobOp::Install,
            ..base.clone()
        };
        let remove = Job {
            op: JobOp::Remove,
            ..base.clone()
        };
        let b = buttons(true, Some(&install), None, true);
        assert_eq!(
            b,
            Buttons {
                shown_installed: false,
                pending: Some(Op::Install),
                enabled: false
            }
        );
        let b = buttons(false, Some(&remove), None, true);
        assert!(b.shown_installed && b.pending == Some(Op::Remove) && !b.enabled);
        assert_eq!(
            buttons(true, None, None, true),
            Buttons {
                shown_installed: true,
                pending: None,
                enabled: true
            }
        );
        assert!(!buttons(false, None, None, true).shown_installed);
        // A request awaiting its job spins the same way.
        let b = buttons(true, None, Some(Op::Update), true);
        assert!(b.shown_installed && b.pending == Some(Op::Update) && !b.enabled);
        // A finished job leaves the buttons to `installed`.
        let done = Job {
            state: JobState::Done,
            ..install
        };
        assert!(buttons(true, Some(&done), None, true).enabled);
        assert!(
            !buttons(true, None, None, false).enabled,
            "actions disabled"
        );
    }

    const CATALOG: &str = r#"
include:
  - name: 86box
    full_name: 86Box
    arch: linux/amd64
    category: Game
    icon: 86box.png
    description: "An x86 emulator."
  - name: anki
    full_name: Anki
    arch: linux/amd64
    category: Education
    icon: anki.svg
    description: "Flashcards."
    disabled: True
  - name: firefox
    full_name: "Firefox "
    arch: linux/amd64,linux/arm64
    icon: firefox.svg
    description: "A web browser."
  - name: nodesc
    full_name: No Description
    category: Utility
    icon: x.png
  - name: all
    full_name: All
    category: Utility
    icon: all.png
    description: "Not a name."
  - name: gimp
    full_name: GIMP
    category: Creative
    icon: gimp.svg
    description: "Images."
    disabled: false
  - name: numeric
    full_name: 5
    icon: n.png
    description: "x"
  - name: nested
    full_name: {a: b}
    icon: n.png
    description: "x"
  - name: badcat
    full_name: Bad
    category: [1]
    icon: n.png
    description: "x"
"#;

    #[test]
    fn catalog_parses_the_proot_apps_metadata() {
        let apps = parse_catalog(CATALOG).unwrap();
        let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["86box", "firefox", "gimp"]);
        assert_eq!(apps[1].full_name, "Firefox", "trimmed");
        assert_eq!(apps[1].category, None);
        assert_eq!(apps[0].category.as_deref(), Some("Game"));
    }

    #[test]
    fn a_catalog_entry_of_the_wrong_shape_is_skipped_not_fatal() {
        let apps = parse_catalog(
            "include:\n  - name: 5\n    full_name: a\n    icon: ''\n    description: ''\n  - {name: {x: 1}, full_name: a, icon: '', description: ''}\n  - 7\n  - name: ok\n    full_name: OK\n    icon: ''\n    description: ''\n",
        )
        .unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "ok");
        assert!(parse_catalog("just: text").is_err());
        assert!(parse_catalog("{{{{").is_err());
    }

    #[test]
    fn the_generated_local_repo_catalog_is_accepted() {
        // Selkies' document for a repository without `metadata/` (`files/apps.rs`).
        let generated = r#"{
  "include": [
    {"name": "ghcr.io/ns/img:tag", "full_name": "ghcr.io/ns/img:tag", "description": "", "icon": ""},
    {"name": "gedit", "full_name": "gedit", "description": "", "icon": ""}
  ]
}
"#;
        let apps = parse_catalog(generated).unwrap();
        assert_eq!(apps.len(), 2);
        assert!(apps
            .iter()
            .all(|a| a.icon.is_empty() && a.category.is_none()));
        assert_eq!(icon_path(&apps[0].icon), None, "no icon fetch");
        assert!(categories(&apps).is_empty());
        assert_eq!(default_filter(&categories(&apps), None), Filter::All);
    }

    #[test]
    fn a_yaml_alias_bomb_fails_fast() {
        let bomb = "a: &a [x, x, x, x, x, x, x, x, x, x]\nb: &b [*a, *a, *a, *a, *a, *a, *a, *a, *a, *a]\nc: &c [*b, *b, *b, *b, *b, *b, *b, *b, *b, *b]\nd: &d [*c, *c, *c, *c, *c, *c, *c, *c, *c, *c]\ne: &e [*d, *d, *d, *d, *d, *d, *d, *d, *d, *d]\nf: &f [*e, *e, *e, *e, *e, *e, *e, *e, *e, *e]\ng: &g [*f, *f, *f, *f, *f, *f, *f, *f, *f, *f]\ninclude: [*g]\n";
        let started = Instant::now();
        assert!(parse_catalog(bomb).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        let huge = "x".repeat(CATALOG_CAP + 1);
        assert!(parse_catalog(&huge).is_err());
    }

    #[test]
    fn the_live_catalog_parses() {
        // The real proot-apps `metadata.yml`, fetched 2026-11-22.
        let text = include_str!("../tests/fixtures/proot-apps-metadata.yml");
        let apps = parse_catalog(text).unwrap();
        assert_eq!(apps.len(), 161, "168 entries, 7 disabled");
        assert_eq!(categories(&apps).len(), 11);
    }

    #[test]
    fn categories_sort_by_label_and_split_camel_case() {
        assert_eq!(category_label("AudioVideo"), "Audio Video");
        assert_eq!(category_label("WebBrowser"), "Web Browser");
        assert_eq!(category_label("AI"), "AI");
        assert_eq!(category_label("Game"), "Game");
        let app = |c: &str| CatalogApp {
            name: "x".into(),
            full_name: "x".into(),
            description: String::new(),
            icon: String::new(),
            category: Some(c.into()),
        };
        let apps = [
            app("Office"),
            app("AudioVideo"),
            app("AI"),
            app("Audio"),
            app("Office"),
        ];
        assert_eq!(categories(&apps), ["AI", "Audio", "AudioVideo", "Office"]);
    }

    #[test]
    fn the_first_visit_opens_on_web_browsers_then_the_saved_choice() {
        let cats: Vec<String> = ["Game", "Network", "WebBrowser"].map(String::from).to_vec();
        assert_eq!(
            default_filter(&cats, None),
            Filter::Category("WebBrowser".into())
        );
        let no_web: Vec<String> = ["Game", "Network"].map(String::from).to_vec();
        assert_eq!(
            default_filter(&no_web, None),
            Filter::Category("Network".into())
        );
        let other: Vec<String> = ["Game", "Office"].map(String::from).to_vec();
        assert_eq!(
            default_filter(&other, None),
            Filter::Category("Game".into())
        );
        assert_eq!(
            default_filter(&cats, Some("Game")),
            Filter::Category("Game".into())
        );
        assert_eq!(default_filter(&cats, Some("Installed")), Filter::Installed);
        assert_eq!(default_filter(&cats, Some("All")), Filter::All);
        assert_eq!(
            default_filter(&cats, Some("Gone")),
            Filter::Category("WebBrowser".into()),
            "a category that left the catalog"
        );
        assert_eq!(
            Filter::from_saved(&Filter::Installed.saved()),
            Filter::Installed
        );
    }

    #[test]
    fn search_spans_every_category_and_ignores_case() {
        let apps = parse_catalog(CATALOG).unwrap();
        let installed: BTreeSet<String> = ["gimp".to_owned()].into();
        let names = |shown: Vec<&CatalogApp>| -> Vec<String> {
            shown.iter().map(|a| a.name.clone()).collect()
        };
        let game = Filter::Category("Game".into());
        assert_eq!(names(filtered(&apps, &game, "", None)), ["86box"]);
        assert_eq!(
            names(filtered(&apps, &game, "  BROWSER ", None)),
            ["firefox"]
        );
        assert_eq!(names(filtered(&apps, &game, "gim", None)), ["gimp"]);
        assert_eq!(
            names(filtered(&apps, &Filter::Installed, "", Some(&installed))),
            ["gimp"]
        );
        assert!(filtered(&apps, &Filter::Installed, "", None).is_empty());
        assert_eq!(
            names(filtered(&apps, &Filter::All, "", None)),
            ["86box", "firefox", "gimp"]
        );
    }

    #[test]
    fn icon_paths_refuse_traversal_and_are_encoded() {
        assert_eq!(icon_path("../x.png"), None);
        assert_eq!(icon_path("a/b.svg"), None);
        assert_eq!(icon_path("a\\b.svg"), None);
        assert_eq!(icon_path(""), None);
        assert_eq!(icon_path("my icon.svg").as_deref(), Some("my%20icon.svg"));
        assert_eq!(
            remote_icon_url("my%20icon.svg").unwrap().as_str(),
            "https://raw.githubusercontent.com/linuxserver/proot-apps/master/metadata/img/my%20icon.svg"
        );
        assert_eq!(
            remote_catalog_url().as_str(),
            "https://raw.githubusercontent.com/linuxserver/proot-apps/master/metadata/metadata.yml"
        );
        assert!(is_svg("A.SVG") && !is_svg("a.png"));
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([255, 0, 0, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    const SQUARE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512"><rect width="512" height="512" fill="#0000ff" fill-opacity="0.5"/></svg>"##;

    #[test]
    fn icons_decode_png_and_svg_within_limits() {
        let svg = decode_icon(SQUARE_SVG.as_bytes(), true).unwrap();
        assert_eq!((svg.width, svg.height), (128, 128));
        assert_eq!(svg.bgra.len(), 128 * 128 * 4);
        // Half-transparent blue, straight BGRA: blue first, full strength.
        assert_eq!(&svg.bgra[..4], &[255, 0, 0, 128]);
        let raster = decode_icon(&png(300, 150), false).unwrap();
        assert_eq!((raster.width, raster.height), (128, 64));
        assert_eq!(&raster.bgra[..4], &[0, 0, 255, 255], "red as BGRA");
        assert!(decode_icon(&vec![b' '; ICON_CAP + 1], true).is_none());
        assert!(decode_icon(&vec![0; RASTER_ICON_CAP + 1], false).is_none());
        assert!(decode_icon(b"garbage", false).is_none());
        assert!(decode_icon(b"<svg", true).is_none());
        assert!(decode_icon(&[0x1f, 0x8b, 8, 0, 0], true).is_none(), "svgz");
        let absurd = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1e30" height="1"/>"#;
        assert!(decode_icon(absurd.as_bytes(), true).is_none());
    }

    #[test]
    fn a_huge_png_is_refused_by_the_icon_limits() {
        // 8000×8000 of one colour compresses to a few hundred KB.
        let image = image::GrayImage::new(8000, 8000);
        let mut bytes = Vec::new();
        image::DynamicImage::ImageLuma8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        assert!(bytes.len() <= RASTER_ICON_CAP, "{} bytes", bytes.len());
        assert!(decode_icon(&bytes, false).is_none());
        assert!(decode_raster(&png(2048, 16), ICON_EDGE, ICON_RASTER_LIMITS).is_some());
    }

    #[test]
    fn icons_never_load_external_images() {
        let dir = tempfile::tempdir().unwrap();
        let red = dir.path().join("red.png");
        std::fs::write(&red, png(64, 64)).unwrap();
        let red_data = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(png(64, 64))
        };
        for href in [
            red.display().to_string(),
            format!("file://{}", red.display()),
            "red.png".to_owned(),
            "https://example.com/red.png".to_owned(),
            format!("data:image/png;base64,{red_data}"),
        ] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="64" height="64"><image width="64" height="64" href="{href}"/><image width="64" height="64" xlink:href="{href}"/></svg>"#
            );
            let icon = decode_svg(svg.as_bytes()).unwrap();
            assert!(
                icon.bgra.chunks(4).all(|p| p[3] == 0),
                "{href} drew something"
            );
        }
        assert_eq!(svg_options().fontdb.len(), 0, "no fonts");
        // Text draws nothing without fonts, and doesn't fail the icon.
        let text = r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><text x="0" y="40" font-size="40">Hi</text></svg>"#;
        let icon = decode_svg(text.as_bytes()).unwrap();
        assert!(icon.bgra.chunks(4).all(|p| p[3] == 0));
    }

    #[test]
    fn a_use_bomb_svg_is_refused_quickly() {
        let mut svg = String::from(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10"><defs><rect id="l0" width="1" height="1"/>"#,
        );
        for level in 1..=30 {
            let prev = level - 1;
            svg.push_str(&format!(
                r##"<g id="l{level}"><use xlink:href="#l{prev}"/><use href="#l{prev}"/></g>"##
            ));
        }
        svg.push_str(r##"</defs><use href="#l30"/></svg>"##);
        assert!(svg.len() < ICON_CAP);
        let started = Instant::now();
        assert!(decode_svg(svg.as_bytes()).is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
        // A cycle is refused too.
        let cycle = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><g id="a"><use href="#b"/></g><g id="b"><use href="#a"/></g></svg>"##;
        assert!(decode_svg(cycle.as_bytes()).is_none());
        // Too many elements.
        let many = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}</svg>"#,
            "<g/>".repeat(SVG_MAX_ELEMENTS + 1)
        );
        assert!(decode_svg(many.as_bytes()).is_none());
        // A reasonable reuse is fine.
        let fine = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><rect id="r" width="5" height="5"/></defs><use href="#r"/><use href="#r" x="5"/></svg>"##;
        assert!(decode_svg(fine.as_bytes()).is_some());
    }

    #[test]
    fn an_entity_bomb_svg_is_refused() {
        let b = "x".repeat(1000);
        let a = "&b;".repeat(250);
        let svg = format!(
            r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY b "{b}"><!ENTITY a "{a}">]><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><text>{}</text></svg>"#,
            "&a;".repeat(400)
        );
        let started = Instant::now();
        assert!(decode_svg(svg.as_bytes()).is_none());
        assert!(started.elapsed() < Duration::from_millis(50));
        // A DOCTYPE with an internal subset is refused even without entities, and so is a bare
        // ENTITY anywhere.
        let subset = r#"<!DOCTYPE svg [ ]><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#;
        assert!(decode_svg(subset.as_bytes()).is_none());
        assert!(decode_svg(b"<svg><!ENTITY a 'b'></svg>").is_none());
    }

    #[test]
    fn an_external_only_doctype_is_allowed() {
        // flameshot.svg and filezilla.svg carry exactly this; nothing external is resolved.
        let doctype = r#"<?xml version="1.0"?><!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd"><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#;
        let icon = decode_svg(doctype.as_bytes()).unwrap();
        assert!(icon.bgra.chunks(4).all(|p| p[3] == 255));
        let system = r#"<!DOCTYPE svg SYSTEM "file:///etc/passwd"><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#;
        assert!(
            decode_svg(system.as_bytes()).is_some(),
            "the external ID is never read"
        );
    }

    /// The nested paint-server bombs QA timed at seconds to hours (`J-QA-2`), plus markers and
    /// filters.
    fn nested(tag: &str, attribute: &str, levels: usize, fan: usize) -> String {
        let (open, close) = match tag {
            "pattern" => (
                r#" width="10" height="10" patternUnits="userSpaceOnUse""#,
                "",
            ),
            _ => ("", ""),
        };
        let mut svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><defs><{tag} id="n0"{open}><rect width="64" height="64" fill="white"/></{tag}>"#
        );
        for level in 1..levels {
            svg.push_str(&format!(r#"<{tag} id="n{level}"{open}>"#));
            for _ in 0..fan {
                svg.push_str(&format!(
                    r#"<rect width="64" height="64" fill="white" {attribute}="url(#n{})"/>"#,
                    level - 1
                ));
            }
            svg.push_str(&format!("</{tag}>{close}"));
        }
        svg.push_str(&format!(
            r#"</defs><rect width="64" height="64" {attribute}="url(#n{})"/></svg>"#,
            levels - 1
        ));
        svg
    }

    fn refused_fast(svg: &str) {
        let started = Instant::now();
        assert!(
            decode_svg(svg.as_bytes()).is_none(),
            "{}",
            &svg[..svg.len().min(200)]
        );
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "{:?} {} bytes: {}",
            started.elapsed(),
            svg.len(),
            &svg[..svg.len().min(120)]
        );
    }

    #[test]
    fn nested_masks_clips_patterns_filters_and_markers_are_refused_quickly() {
        for levels in [2, 4, 5, 6, 7] {
            refused_fast(&nested("mask", "mask", levels, 10));
            refused_fast(&nested("clipPath", "clip-path", levels, 10));
            refused_fast(&nested("pattern", "fill", levels, 10));
        }
        // Through a style attribute, a stylesheet class, a `<use>`, an inherited fill, and a
        // mask on the mask itself.
        refused_fast(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><mask id="a"><rect width="9" height="9"/></mask><mask id="b"><rect width="9" height="9" style="mask:url(#a)"/></mask><rect width="9" height="9" mask="url(#b)"/></svg>"##,
        );
        refused_fast(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><style>.c{clip-path:url(#a)}</style><clipPath id="a"><rect width="9" height="9"/></clipPath><clipPath id="b"><rect class="c" width="9" height="9"/></clipPath><rect width="9" height="9" clip-path="url(#b)"/></svg>"##,
        );
        refused_fast(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><defs><g id="g"><rect width="9" height="9" mask="url(#a)"/></g><mask id="a"><rect width="9" height="9"/></mask><mask id="b"><use href="#g"/></mask></defs><rect width="9" height="9" mask="url(#b)"/></svg>"##,
        );
        refused_fast(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><pattern id="a" width="2" height="2"><rect width="1" height="1"/></pattern><g fill="url(#a)"><pattern id="b" width="3" height="3"><rect width="1" height="1"/></pattern></g><rect width="9" height="9" fill="url(#b)"/></svg>"##,
        );
        refused_fast(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><clipPath id="a"><rect width="9" height="9"/></clipPath><clipPath id="b" clip-path="url(#a)"><rect width="9" height="9"/></clipPath><rect width="9" height="9" clip-path="url(#b)"/></svg>"##,
        );
        refused_fast(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><pattern id="a" width="2" height="2"><rect width="1" height="1"/></pattern><pattern id="b" href="#a"/><rect width="9" height="9" fill="url(#b)"/></svg>"##,
        );
        // Filters and markers, by element, attribute, or stylesheet.
        let blurs: String = (0..200)
            .map(|i| {
                format!(
                    r#"<feGaussianBlur in="b{i}" result="b{}" stdDeviation="5"/>"#,
                    i + 1
                )
            })
            .collect();
        // A filter chain is removed, fast, and the rest still draws.
        let chain = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><filter id="f">{blurs}</filter><rect width="9" height="9" filter="url(#f)"/></svg>"#
        );
        // The checks and the stripping, all in this process, take under 50 ms; the drawing is
        // the worker's (a process start included).
        let started = Instant::now();
        assert!(svg_checked_text(chain.as_bytes()).is_some());
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "{:?}",
            started.elapsed()
        );
        let icon = decode_svg(chain.as_bytes()).expect("drawn without its filter");
        assert!(icon.bgra.chunks(4).all(|p| p[3] == 255));
        refused_fast(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><marker id="m"><rect width="1" height="1"/></marker><path d="M0 0L9 9" marker-mid="url(#m)"/></svg>"##,
        );
        // Too many uses of one mask.
        let uses = r#"<rect width="1" height="1" mask="url(#m)"/>"#.repeat(SVG_MAX_PAINT_REFS + 1);
        refused_fast(&format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><mask id="m"><rect width="1" height="1" fill="white"/></mask>{uses}</svg>"#
        ));
        // Inkscape's `marker:none` styles, with no `<marker>`, are harmless.
        assert!(decode_svg(br##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><rect width="9" height="9" style="marker:none;marker-end:none"/></svg>"##).is_some());
        // QA's own cases, when they are on this Mac.
        if let Ok(dir) = std::env::var("SKERRY_SVG_CASES") {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let svg = std::fs::read_to_string(&path).unwrap();
                if path.to_string_lossy().contains("blur") {
                    // Filter chains: stripped, not refused, and just as fast.
                    let started = Instant::now();
                    assert!(svg_checked_text(svg.as_bytes()).is_some(), "{path:?}");
                    assert!(started.elapsed() < Duration::from_millis(50), "{path:?}");
                    assert!(decode_svg(svg.as_bytes()).is_some(), "{path:?}");
                } else {
                    refused_fast(&svg);
                }
            }
        }
    }

    #[test]
    fn filters_are_removed_from_attributes_styles_and_stylesheets() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><defs><filter id="s"><feGaussianBlur stdDeviation="9"/></filter></defs><style>.a{fill:#00f;filter:url(#s)}rect{filter : url(#s)}</style><rect class="a" width="8" height="4" filter="url(#s)"/><rect y="4" width="8" height="4" fill="#00f" style="filter:url(#s);opacity:1"/></svg>"##;
        let doc = resvg::usvg::roxmltree::Document::parse(svg).unwrap();
        let stripped = without_filters(svg, &doc).unwrap().unwrap();
        assert!(!stripped.contains("filter"), "{stripped}");
        let icon = decode_svg(svg.as_bytes()).unwrap();
        assert!(
            icon.bgra.chunks(4).all(|p| p == [255, 0, 0, 255]),
            "solid, unblurred blue"
        );
        let plain = r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"/>"#;
        let doc = resvg::usvg::roxmltree::Document::parse(plain).unwrap();
        assert_eq!(
            without_filters(plain, &doc),
            Some(None),
            "nothing to rewrite"
        );
    }

    #[test]
    fn nesting_through_ancestor_classes_styles_and_imports_is_refused_quickly() {
        // The reviewer's case (R2-H1): each pattern inherits `fill` from its ancestor's class.
        let levels = 7;
        let rules: String = (1..levels)
            .map(|k| format!(".c{k}{{fill:url(#n{})}} ", k - 1))
            .collect();
        let mut svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><style>{rules}</style><defs><pattern id="n0" width="2" height="2" patternUnits="userSpaceOnUse"><rect width="1" height="1"/></pattern>"#
        );
        for k in 1..levels {
            svg.push_str(&format!(
                r#"<g class="c{k}"><pattern id="n{k}" width="10" height="10" patternUnits="userSpaceOnUse">{}</pattern></g>"#,
                r#"<rect width="1" height="1"/>"#.repeat(10)
            ));
        }
        svg.push_str(&format!(
            r#"</defs><rect width="64" height="64" fill="url(#n{})"/></svg>"#,
            levels - 1
        ));
        refused_fast(&svg);
        // The class on a farther ancestor, and among several classes.
        refused_fast(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><style>.x{fill:url(#a)}</style><pattern id="a" width="2" height="2"><rect width="1" height="1"/></pattern><g class="y x"><g><pattern id="b" width="3" height="3"><rect width="1" height="1"/></pattern></g></g><rect width="9" height="9" fill="url(#b)"/></svg>"##,
        );
        // A `style=` on an ancestor that references a paint server.
        refused_fast(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><pattern id="a" width="2" height="2"><rect width="1" height="1"/></pattern><g style="fill:url(#a)"><pattern id="b" width="3" height="3"><rect width="1" height="1"/></pattern></g><rect width="9" height="9" fill="url(#b)"/></svg>"##,
        );
        // A non-class selector that references a server reaches every element.
        refused_fast(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><style>g>rect{fill:url(#a)}</style><pattern id="a" width="2" height="2"><rect width="1" height="1"/></pattern><pattern id="b" width="3" height="3"><g><rect width="1" height="1"/></g></pattern><rect width="9" height="9" fill="url(#b)"/></svg>"##,
        );
        // `@import`, and CSS escapes that would hide a reference.
        refused_fast(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><style>@import url(x.css);</style><mask id="a"><rect width="9" height="9"/></mask><rect width="9" height="9" mask="url(#a)"/></svg>"##,
        );
        refused_fast(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><style>.c{fill:url(#a\30)}</style><pattern id="a0" width="2" height="2"><rect width="1" height="1"/></pattern><pattern id="b" width="3" height="3"><rect class="c" width="1" height="1"/></pattern><rect width="9" height="9" fill="url(#b)"/></svg>"##,
        );
        refused_fast(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><pattern id="a0" width="2" height="2"><rect width="1" height="1"/></pattern><pattern id="b" width="3" height="3"><rect style="fill:url(#a\30)" width="1" height="1"/></pattern><rect width="9" height="9" fill="url(#b)"/></svg>"##,
        );
    }

    /// Turns this test binary into an SVG worker when the parent asks (`worker_command`).
    #[test]
    fn svg_worker_entry() {
        if std::env::var_os("SKERRY_SVG_WORKER").is_some() {
            std::process::exit(svg_worker_main());
        }
    }

    #[test]
    fn a_bomb_past_the_checks_is_killed_and_the_parent_stays_healthy() {
        // The R3 shape: objectBoundingBox patterns, 7 levels, each filling with the one below.
        let mut svg = String::from(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><defs><pattern id="n0" width="0.5" height="0.5"><rect width="1" height="1"/></pattern>"#,
        );
        for k in 1..7 {
            svg.push_str(&format!(
                r#"<pattern id="n{k}" width="0.1" height="0.1">{}</pattern>"#,
                format!(r#"<rect width="1" height="1" fill="url(#n{})"/>"#, k - 1).repeat(10)
            ));
        }
        svg.push_str(r#"</defs><rect width="64" height="64" fill="url(#n6)"/></svg>"#);
        assert!(
            svg_checked_text(svg.as_bytes()).is_none(),
            "the checks catch it"
        );
        // Skip the checks in the worker: its limits alone must stop it.
        let started = Instant::now();
        assert!(rasterize_in_worker(svg.as_bytes(), true).is_none());
        assert!(
            started.elapsed() < Duration::from_millis(3_500),
            "{:?}",
            started.elapsed()
        );
        // The parent carries on.
        assert!(decode_svg(SQUARE_SVG.as_bytes()).is_some());
    }

    #[test]
    fn worker_answers_are_checked() {
        let mut good = WORKER_MAGIC.to_vec();
        good.extend_from_slice(&1u32.to_le_bytes());
        good.extend_from_slice(&1u32.to_le_bytes());
        good.extend_from_slice(&[1, 2, 3, 4]);
        let mut noisy = b"running 1 test\n".to_vec();
        noisy.extend_from_slice(&good);
        assert_eq!(
            worker_pixels(&noisy).map(|p| p.bgra),
            Some(vec![1, 2, 3, 4])
        );
        assert!(worker_pixels(&good[..good.len() - 1]).is_none(), "short");
        let mut huge = WORKER_MAGIC.to_vec();
        huge.extend_from_slice(&1000u32.to_le_bytes());
        huge.extend_from_slice(&1u32.to_le_bytes());
        assert!(worker_pixels(&huge).is_none(), "outside the icon box");
        assert!(worker_pixels(b"nothing").is_none());
    }

    #[test]
    fn url_references_are_read_as_svgtypes_reads_them() {
        let servers: HashMap<&str, ()> = [("n0", ())].into();
        let hits = |text: &str| url_ids(text).iter().any(|id| names_server(id, &servers));
        for text in [
            "fill:url(\"#n0\t\")",
            "fill:url('#n0\n')",
            "fill:url(\r#n0)",
            "url(\r#n0)",
            "url('#n0\t')",
            "fill:url( \t\r\n ' \r#n0 ' )",
            "url(#n0)",
            "url(x.svg#n0)",
            "url('#n0",
        ] {
            assert!(hits(text), "{text:?}");
        }
        assert!(!hits("fill:url(#other) stroke:none"));
        assert_eq!(url_ids("url( '#a b ' )"), ["a b"]);
    }

    #[test]
    fn whitespace_inside_url_never_hides_nesting() {
        // The reviewer's six variants (R3-H1), as XML would carry them.
        let pattern = |fill_ref: &str, style: &str| {
            format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9">{style}<pattern id="n0" width="2" height="2"><rect width="1" height="1"/></pattern><pattern id="n1" width="3" height="3"><rect class="c" width="1" height="1" {fill_ref}/></pattern><rect width="9" height="9" fill="url(#n1)"/></svg>"##
            )
        };
        refused_fast(&pattern("", "<style>.c{fill:url(\"#n0\t\")}</style>"));
        refused_fast(&pattern("", "<style>.c{fill:url('#n0\n')}</style>"));
        refused_fast(&pattern("", "<style>.c{fill:url(&#13;#n0)}</style>"));
        refused_fast(&pattern(r#"fill="url(&#13;#n0)""#, ""));
        refused_fast(&pattern(r#"fill="url('#n0&#9;')""#, ""));
        refused_fast(&pattern(r#"style="fill:url(&#13;#n0)""#, ""));
        // And the 6-level objectBoundingBox timing case through an attribute.
        let mut svg = String::from(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><defs><pattern id="n0" width="0.5" height="0.5"><rect width="1" height="1"/></pattern>"#,
        );
        for k in 1..6 {
            svg.push_str(&format!(
                r#"<pattern id="n{k}" width="0.1" height="0.1">{}</pattern>"#,
                format!(
                    r#"<rect width="1" height="1" fill="url(&#13;#n{})"/>"#,
                    k - 1
                )
                .repeat(10)
            ));
        }
        svg.push_str(r#"</defs><rect width="64" height="64" fill="url(#n5)"/></svg>"#);
        refused_fast(&svg);
    }

    #[test]
    fn plain_masks_clips_patterns_and_gradients_still_render() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><defs><linearGradient id="g0"><stop offset="0" stop-color="red"/></linearGradient><linearGradient id="g" href="#g0"/><style>.a{fill:url(#g)}</style><clipPath id="c"><circle cx="32" cy="32" r="30"/></clipPath><mask id="m"><rect width="64" height="64" fill="white"/></mask><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="2" height="2" fill="url(#g)"/></pattern></defs><g clip-path="url(#c)" mask="url(#m)"><rect class="a" width="64" height="32"/><rect y="32" width="64" height="32" fill="url(#p)"/></g></svg>"##;
        let icon = decode_svg(svg.as_bytes()).unwrap();
        assert!(icon.bgra.chunks(4).any(|p| p[3] > 0));
    }

    #[test]
    fn the_real_catalog_icons_still_decode() {
        // `SKERRY_SVG_REAL=<folder of proot-apps metadata/img/*.svg>` checks no real icon is
        // refused by the limits.
        let Ok(dir) = std::env::var("SKERRY_SVG_REAL") else {
            return;
        };
        let mut refused = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "svg") {
                let bytes = std::fs::read(&path).unwrap();
                if fetch_ok(&bytes) && decode_icon(&bytes, true).is_none() {
                    refused.push(path.display().to_string());
                }
            }
        }
        assert!(refused.is_empty(), "{refused:?}");
    }

    fn fetch_ok(bytes: &[u8]) -> bool {
        bytes.len() <= ICON_CAP
    }

    #[test]
    fn late_icon_loads_are_dropped_after_a_reset() {
        let mut queue = IconQueue::default();
        for i in 0..8 {
            assert!(queue.want(&format!("{i}.svg")));
        }
        assert!(!queue.want("0.svg"), "deduplicated");
        let started: Vec<(String, u64)> = std::iter::from_fn(|| queue.next()).collect();
        assert_eq!(started.len(), ICON_FETCHES);
        let old = started[0].1;
        assert!(queue.finished(old));
        assert!(queue.next().is_some(), "a slot freed");
        queue.reset();
        assert!(queue.want("0.svg"), "asked for again in the new generation");
        assert_eq!(queue.next(), None, "the stale loads still hold every slot");
        assert!(!queue.finished(old), "a stale result is dropped");
        assert_eq!(
            queue.next().map(|(_, g)| g),
            Some(old + 1),
            "and frees its slot"
        );
        assert!(queue.want("1.svg"));
        assert_eq!(queue.next(), None, "never more than ICON_FETCHES in all");
        queue.forget("1.svg");
        assert!(
            queue.want("1.svg"),
            "a forgotten icon may be asked for again"
        );
    }

    #[test]
    fn the_console_follows_its_end_unless_scrolled_up() {
        assert!(follows_end(-100., 100.));
        assert!(follows_end(-93., 100.));
        assert!(!follows_end(-50., 100.));
        assert!(follows_end(0., 0.));
    }

    mod network {
        use super::*;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        /// Serves each connection `reply(request_head)` and records the request heads.
        async fn serve(
            reply: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static,
        ) -> (Url, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let log = seen.clone();
            let reply = std::sync::Arc::new(reply);
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        if stream.read(&mut byte).await.unwrap_or(0) == 0 {
                            break;
                        }
                        head.push(byte[0]);
                    }
                    let head = String::from_utf8_lossy(&head).into_owned();
                    log.lock().unwrap().push(head.clone());
                    let _ = stream.write_all(&reply(&head)).await;
                    let _ = stream.shutdown().await;
                }
            });
            (
                Url::parse(&format!("http://127.0.0.1:{port}/metadata.yml")).unwrap(),
                seen,
            )
        }

        fn ok(body: &[u8]) -> Vec<u8> {
            let mut reply = format!(
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            reply.extend_from_slice(body);
            reply
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn capped_fetch_stops_past_the_cap() {
            let big = vec![b'x'; 3 * 1024 * 1024];
            let (url, _) = serve(move |_| ok(&big)).await;
            let client = plain_client_with(false).unwrap();
            let result = fetch_capped(&client, url.clone(), CATALOG_CAP, CATALOG_TIMEOUT).await;
            assert!(result.is_err());
            // No length header: the stream is cut at the cap.
            let (url, _) = serve(|_| {
                let mut reply = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
                reply.extend(vec![b'y'; 3 * 1024 * 1024]);
                reply
            })
            .await;
            assert!(fetch_capped(&client, url, CATALOG_CAP, CATALOG_TIMEOUT)
                .await
                .is_err());
            let (url, _) = serve(|_| ok(b"include: []")).await;
            assert_eq!(
                fetch_capped(&client, url, CATALOG_CAP, CATALOG_TIMEOUT)
                    .await
                    .unwrap(),
                b"include: []"
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn the_catalog_client_sends_no_credentials() {
            let (url, seen) = serve(|_| ok(b"include: []")).await;
            let client = plain_client_with(false).unwrap();
            fetch_capped(&client, url, CATALOG_CAP, CATALOG_TIMEOUT)
                .await
                .unwrap();
            let head = seen.lock().unwrap()[0].to_ascii_lowercase();
            assert!(head.starts_with("get /metadata.yml "), "{head}");
            for header in ["authorization", "cookie", "origin", "x-api-key", "referer"] {
                assert!(
                    !head.contains(&format!("\r\n{header}:")),
                    "{header} in {head}"
                );
            }
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_redirect_off_https_is_refused() {
            let (url, seen) = serve(|head| {
                if head.starts_with("GET /metadata.yml") {
                    b"HTTP/1.1 302 Found\r\nLocation: /plain\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                } else {
                    ok(b"include: []")
                }
            })
            .await;
            let client = plain_client_with(false).unwrap();
            assert!(fetch_capped(&client, url, CATALOG_CAP, CATALOG_TIMEOUT)
                .await
                .is_err());
            assert_eq!(
                seen.lock().unwrap().len(),
                1,
                "the http:// target is never asked"
            );
            // The real client refuses plain HTTP outright.
            let (url, seen) = serve(|_| ok(b"include: []")).await;
            assert!(
                fetch_capped(&plain_client().unwrap(), url, CATALOG_CAP, CATALOG_TIMEOUT)
                    .await
                    .is_err()
            );
            assert!(seen.lock().unwrap().is_empty());
        }
    }
}
