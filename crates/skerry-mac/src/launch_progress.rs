//! Following one launch until its session is ready (`ARCH-027`, `ARCH-GAP-LS-01`).
//!
//! A [`Flight`] is one launch in progress. It combines two sources: the launch request
//! itself (`POST /companion/api/launch/simple`, which can take many minutes and can be
//! lost) and the server's progress route (`GET /companion/api/launch/progress/<id>`).
//! Progress decides while the server knows the launch; a POST success always wins. All the
//! timing logic is here and pure, so tests pass fake `Instant`s and need neither a window
//! nor a server. Task C's `/launch/url` and `/launch/file` can drive the same `Flight`: it
//! takes any launch result plus the launch id.

use crate::launch_options::LaunchSelection;
use crate::sealskin::{
    session_from_progress, LaunchError, LaunchParams, LaunchProgress, ProgressAnswer, RemoteApp,
    RemoteSession,
};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// No change of stage for this long shows the stall prompt.
pub const STALL: Duration = Duration::from_secs(180);
/// Progress unknown for this long after the request went out (with the request lost) means
/// the server never saw the launch (`UNSEEN_MS` in SealSkin's `launching.js`).
pub const UNSEEN: Duration = Duration::from_secs(20);
/// Polling cadence: 600 ms for the first 30 s, then 1500 ms (`launching.js`).
const FAST_POLL: Duration = Duration::from_millis(600);
const SLOW_POLL: Duration = Duration::from_millis(1500);
const FAST_PHASE: Duration = Duration::from_secs(30);
/// A session of the same app created this long before the flight started can still be its.
const ADOPT_SLACK: f64 = 60.0;

/// The four stages the Library shows, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UserStage {
    Requesting,
    PullingImage,
    Starting,
    Connecting,
}

impl UserStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Requesting => "Requesting",
            Self::PullingImage => "Pulling image",
            Self::Starting => "Starting",
            Self::Connecting => "Connecting",
        }
    }

    /// The tile's status line while the app launches.
    pub fn tile_label(self) -> &'static str {
        match self {
            Self::Requesting => "Requesting…",
            Self::PullingImage => "Pulling image…",
            Self::Starting => "Starting…",
            Self::Connecting => "Connecting…",
        }
    }

    pub fn stall_message(self, minutes: u64) -> String {
        match self {
            Self::Requesting => format!("Still waiting for the server after {minutes} min."),
            Self::PullingImage => format!("Still pulling the image after {minutes} min."),
            Self::Starting | Self::Connecting => format!("Still starting after {minutes} min."),
        }
    }

    /// Whether the stall prompt explains first-launch downloads.
    pub fn stall_hint(self) -> Option<&'static str> {
        (self != Self::Requesting).then_some(STALL_HINT)
    }
}

pub const STALL_HINT: &str = "The first launch of an app on a server downloads it, which can take several minutes. Later launches are fast.";
pub const LOST_BODY: &str =
    "The launch request didn’t reach the server. Check your connection, then try again.";
pub const NO_REASON: &str = "The server didn’t say why.";

/// The Library's stage for a server stage. `client` runs after the container is up, so it
/// is Starting; only `image` and `baking` are Pulling image (Kubernetes never reports
/// them: its pull happens inside `starting`). Unknown stages are Requesting.
pub fn user_stage(server_stage: &str) -> UserStage {
    match server_stage {
        "image" | "baking" => UserStage::PullingImage,
        "starting" | "waiting" | "client" => UserStage::Starting,
        "ready" => UserStage::Connecting,
        _ => UserStage::Requesting,
    }
}

/// The server's own sentence for a stage, as SealSkin's web launcher words it.
pub fn stage_sentence(server_stage: &str, detail: &Map<String, Value>) -> String {
    let sentence = match server_stage {
        "requesting" => "Sending the launch request",
        "placing" => "Checking limits and choosing a node",
        "forwarding" => {
            return match detail.get("node").and_then(Value::as_str) {
                Some(node) => format!("Handing the launch to node {node}"),
                None => "Handing the launch to another node".into(),
            }
        }
        "storage" => "Preparing the home directory",
        "files" => "Syncing shared files",
        "vpn" => "Bringing up the VPN gateway",
        "baking" => "Baking the application’s image",
        "image" => "Downloading the application image",
        "client" => "Preparing the application’s web client",
        "starting" => "Creating the container",
        "waiting" => "Waiting for the desktop to respond",
        "ready" => "Opening the session",
        other => other,
    };
    sentence.to_string()
}

/// "2 s" up to 59 s, then "1:05".
pub fn stage_time(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds} s")
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

/// "0:42", "12:07", and "1:02:03" past an hour.
pub fn clock(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, (seconds / 60) % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Where a flight has got to.
#[derive(Clone, Debug)]
pub enum Outcome {
    Running,
    /// The request was lost and the server doesn't know the launch: the caller looks for
    /// a new session of the app once ([`Flight::sessions_checked`]) before giving up.
    Checking,
    /// Never seen by the server, and no new session appeared.
    Lost,
    /// The server's error, if it gave one.
    Failed(Option<String>),
    Ready(RemoteSession),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    Done,
    Current,
    Pending,
    Failed,
    Stalled,
}

/// One checklist row: the stage, its seconds (done: how long it took; current: so far).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecklistRow {
    pub stage: UserStage,
    pub seconds: Option<u64>,
    pub state: RowState,
}

/// One launch in progress.
#[derive(Clone, Debug)]
pub struct Flight {
    pub launch_id: String,
    pub app: RemoteApp,
    /// Carries `launch_id`.
    pub params: LaunchParams,
    /// Becomes the app's remembered options once the launch is ready.
    pub remember: Option<LaunchSelection>,
    /// Sessions the Library listed when the flight started; a new one may be adopted.
    known_sessions: HashSet<String>,
    started: Instant,
    started_unix: f64,
    /// Each user stage reached and when.
    reached: Vec<(UserStage, Instant)>,
    pub server_stage: String,
    pub detail: Map<String, Value>,
    /// A launch request is in flight.
    posting: bool,
    /// When the current request went out (after any Auto home was created).
    sent_at: Option<Instant>,
    last_post_error: Option<LaunchError>,
    /// Progress said `failed`; it ends the flight once no request is outstanding.
    progress_failed: Option<Option<String>>,
    prompt_after: Instant,
    poll_errors: u32,
    pub outcome: Outcome,
}

impl Flight {
    /// A new flight with a fresh launch id. `known_sessions`: the sessions listed now.
    pub fn new(
        app: RemoteApp,
        params: LaunchParams,
        remember: Option<LaunchSelection>,
        known_sessions: HashSet<String>,
        now: Instant,
        now_unix: f64,
    ) -> Self {
        let mut flight = Self {
            launch_id: String::new(),
            app,
            params,
            remember,
            known_sessions,
            started: now,
            started_unix: now_unix,
            reached: Vec::new(),
            server_stage: String::new(),
            detail: Map::new(),
            posting: false,
            sent_at: None,
            last_post_error: None,
            progress_failed: None,
            prompt_after: now,
            poll_errors: 0,
            outcome: Outcome::Running,
        };
        flight.reset(now, now_unix);
        flight
    }

    /// Starts over under a fresh launch id.
    fn reset(&mut self, now: Instant, now_unix: f64) {
        self.launch_id = uuid::Uuid::new_v4().to_string();
        self.params.launch_id = Some(self.launch_id.clone());
        self.started = now;
        self.started_unix = now_unix;
        self.reached = vec![(UserStage::Requesting, now)];
        self.server_stage = "requesting".into();
        self.detail = Map::new();
        self.posting = false;
        self.sent_at = None;
        self.last_post_error = None;
        self.progress_failed = None;
        self.prompt_after = now + STALL;
        self.poll_errors = 0;
        self.outcome = Outcome::Running;
    }

    pub fn is_running(&self) -> bool {
        matches!(self.outcome, Outcome::Running)
    }

    /// The flight still occupies its app: running or checking for a session.
    pub fn in_flight(&self) -> bool {
        matches!(self.outcome, Outcome::Running | Outcome::Checking)
    }

    pub fn stage(&self) -> UserStage {
        self.reached
            .last()
            .map_or(UserStage::Requesting, |(stage, _)| *stage)
    }

    /// A launch request is about to be spawned.
    pub fn begin_attempt(&mut self) {
        self.posting = true;
        self.sent_at = None;
        self.last_post_error = None;
    }

    /// The request went out now.
    pub fn sent(&mut self, now: Instant) {
        if self.posting {
            self.sent_at = Some(now);
        }
    }

    /// The launch request ended.
    pub fn posted(&mut self, result: Result<RemoteSession, LaunchError>, now: Instant) {
        if !self.in_flight() {
            return;
        }
        self.posting = false;
        match result {
            // A success always wins, even over a `failed` progress: a same-id resend after
            // a failure runs the launch again while progress keeps saying failed.
            Ok(session) => self.finish(session, now),
            Err(error) => {
                self.last_post_error = Some(error);
                self.settle();
            }
        }
    }

    /// What the progress route answered.
    pub fn observe(&mut self, answer: &ProgressAnswer, now: Instant) {
        if !self.is_running() {
            return;
        }
        self.poll_errors = 0;
        match answer {
            ProgressAnswer::Known(progress) => self.progress(progress, now),
            ProgressAnswer::Unknown => {
                if self.posting {
                    // The server may not have seen the request yet; stay at Requesting.
                    return;
                }
                match &self.last_post_error {
                    Some(LaunchError::Server(message)) => {
                        self.outcome = Outcome::Failed(Some(message.clone()));
                    }
                    Some(LaunchError::Network(_))
                        if self
                            .sent_at
                            .is_none_or(|sent| now.saturating_duration_since(sent) > UNSEEN) =>
                    {
                        self.outcome = Outcome::Checking;
                    }
                    _ => {}
                }
            }
            ProgressAnswer::SignedOut => {
                self.outcome = Outcome::Failed(Some(
                    "The server didn’t accept this application key.".into(),
                ));
            }
        }
    }

    fn progress(&mut self, progress: &LaunchProgress, now: Instant) {
        if progress.stage != self.server_stage || progress.detail != self.detail {
            self.prompt_after = now + STALL;
        }
        self.server_stage = progress.stage.clone();
        self.detail = progress.detail.clone();
        match progress.stage.as_str() {
            "ready" => {
                if let Some(session) = session_from_progress(&self.app, &self.params, progress) {
                    self.finish(session, now);
                }
            }
            "failed" => {
                self.progress_failed = Some(progress.error.clone());
                self.settle();
            }
            stage => {
                // User stages only move forward; a later server stage that maps to an
                // earlier one (`client` after `starting`) changes the detail line only.
                let stage = user_stage(stage);
                if stage > self.stage() {
                    self.reached.push((stage, now));
                }
            }
        }
    }

    /// Ends the flight on a failure once nothing outstanding can still change it.
    fn settle(&mut self) {
        if self.posting {
            return;
        }
        if let Some(error) = &self.progress_failed {
            self.outcome = Outcome::Failed(error.clone());
        }
    }

    fn finish(&mut self, session: RemoteSession, now: Instant) {
        if self.stage() < UserStage::Connecting {
            self.reached.push((UserStage::Connecting, now));
        }
        self.server_stage = "ready".into();
        self.outcome = Outcome::Ready(session);
    }

    /// The one sessions lookup before a lost flight gives up: a session of this app that
    /// appeared since the flight started is its launch.
    pub fn sessions_checked(&mut self, sessions: Option<&[RemoteSession]>, now: Instant) {
        if !matches!(self.outcome, Outcome::Checking) {
            return;
        }
        let adopted = sessions.into_iter().flatten().find(|session| {
            session.app_id == self.app.id
                && !self.known_sessions.contains(&session.session_id)
                && session.created_at >= self.started_unix - ADOPT_SLACK
        });
        match adopted {
            Some(session) => self.finish(session.clone(), now),
            None => self.outcome = Outcome::Lost,
        }
    }

    pub fn poll_failed(&mut self) {
        self.poll_errors += 1;
    }

    pub fn poll_interval(&self, now: Instant) -> Duration {
        if self.poll_errors > 0 || now.saturating_duration_since(self.started) >= FAST_PHASE {
            SLOW_POLL
        } else {
            FAST_POLL
        }
    }

    /// The stall prompt shows: running, not yet connecting, and no stage change for
    /// [`STALL`] (or since the last Keep Waiting).
    pub fn stalled(&self, now: Instant) -> bool {
        self.is_running() && self.stage() < UserStage::Connecting && now >= self.prompt_after
    }

    pub fn keep_waiting(&mut self, now: Instant) {
        self.prompt_after = now + STALL;
    }

    /// Retry with the same id is offered only after the request was lost: while the first
    /// request is pending, a resend would only join it.
    pub fn can_retry_same(&self) -> bool {
        self.is_running()
            && !self.posting
            && matches!(self.last_post_error, Some(LaunchError::Network(_)))
    }

    /// Resends the request under the same id (the server joins a launch still running).
    pub fn retry_same(&mut self, now: Instant) {
        self.begin_attempt();
        self.prompt_after = now + STALL;
    }

    /// A new launch under a fresh id, after a failure or a lost launch.
    pub fn retry_fresh(&mut self, known_sessions: HashSet<String>, now: Instant, now_unix: f64) {
        self.known_sessions = known_sessions;
        self.reset(now, now_unix);
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.started)
    }

    /// Whole minutes in the current stage, for the stall message.
    pub fn stage_minutes(&self, now: Instant) -> u64 {
        self.reached.last().map_or(0, |(_, since)| {
            now.saturating_duration_since(*since).as_secs() / 60
        })
    }

    /// The checklist: the stages reached with their times, then (while running) the ones
    /// to come. Pulling image shows only once reached; a failed flight shows only what it
    /// reached.
    pub fn checklist(&self, now: Instant) -> Vec<ChecklistRow> {
        let failed = matches!(self.outcome, Outcome::Failed(_) | Outcome::Lost);
        let stalled = self.stalled(now);
        let mut rows: Vec<ChecklistRow> = self
            .reached
            .iter()
            .enumerate()
            .map(|(index, (stage, since))| {
                let next = self.reached.get(index + 1).map(|(_, at)| *at);
                let last = next.is_none();
                let state = match (last, failed, stalled) {
                    (false, _, _) => RowState::Done,
                    (true, true, _) => RowState::Failed,
                    (true, _, true) => RowState::Stalled,
                    (true, _, _) => RowState::Current,
                };
                let until = next.unwrap_or(now);
                ChecklistRow {
                    stage: *stage,
                    seconds: Some(until.saturating_duration_since(*since).as_secs()),
                    state,
                }
            })
            .collect();
        if !failed {
            let current = self.stage();
            for stage in [UserStage::Starting, UserStage::Connecting] {
                if stage > current {
                    rows.push(ChecklistRow {
                        stage,
                        seconds: None,
                        state: RowState::Pending,
                    });
                }
            }
        }
        rows
    }

    /// The detail line: the server's sentence for its current stage.
    pub fn detail_line(&self) -> String {
        stage_sentence(&self.server_stage, &self.detail)
    }

    /// The failed card's body.
    pub fn failure_body(&self) -> Option<&str> {
        match &self.outcome {
            Outcome::Lost => Some(LOST_BODY),
            Outcome::Failed(Some(error)) if !error.trim().is_empty() => Some(error),
            Outcome::Failed(_) => Some(NO_REASON),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app() -> RemoteApp {
        serde_json::from_value(json!({"id": "firefox", "name": "Firefox"})).unwrap()
    }

    fn session(id: &str, app_id: &str, created_at: f64) -> RemoteSession {
        serde_json::from_value(json!({
            "session_id": id, "app_id": app_id, "app_name": app_id,
            "session_url": format!("/{id}/"), "created_at": created_at,
        }))
        .unwrap()
    }

    const T0: f64 = 1_000_000.0;

    fn flight(now: Instant) -> Flight {
        let params = LaunchParams {
            home_name: Some("cleanroom".into()),
            ..LaunchParams::default()
        };
        Flight::new(app(), params, None, HashSet::from(["old".into()]), now, T0)
    }

    fn known(stage: &str) -> ProgressAnswer {
        ProgressAnswer::Known(LaunchProgress {
            stage: stage.into(),
            ..LaunchProgress::default()
        })
    }

    fn ready() -> ProgressAnswer {
        ProgressAnswer::Known(LaunchProgress {
            stage: "ready".into(),
            session_id: Some("new".into()),
            session_url: Some("/new/".into()),
            ..LaunchProgress::default()
        })
    }

    fn lost() -> LaunchError {
        LaunchError::Network("lost".into())
    }

    fn stages(flight: &Flight, now: Instant) -> Vec<(UserStage, RowState)> {
        flight
            .checklist(now)
            .into_iter()
            .map(|row| (row.stage, row.state))
            .collect()
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn server_stages_map_to_four_user_stages() {
        for stage in [
            "requesting",
            "placing",
            "forwarding",
            "storage",
            "files",
            "vpn",
            "x",
        ] {
            assert_eq!(user_stage(stage), UserStage::Requesting, "{stage}");
        }
        assert_eq!(user_stage("image"), UserStage::PullingImage);
        assert_eq!(user_stage("baking"), UserStage::PullingImage);
        for stage in ["starting", "waiting", "client"] {
            assert_eq!(user_stage(stage), UserStage::Starting, "{stage}");
        }
        assert_eq!(user_stage("ready"), UserStage::Connecting);
    }

    #[test]
    fn client_stage_is_starting() {
        assert_eq!(user_stage("client"), UserStage::Starting);
    }

    #[test]
    fn user_stage_never_regresses() {
        let t = Instant::now();
        let mut f = flight(t);
        f.begin_attempt();
        f.observe(&known("starting"), t + secs(1));
        f.observe(&known("client"), t + secs(2));
        f.observe(&known("image"), t + secs(3));
        assert_eq!(
            f.reached.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            [UserStage::Requesting, UserStage::Starting]
        );
        assert_eq!(f.detail_line(), "Downloading the application image");
    }

    #[test]
    fn pulling_image_appears_only_when_reported() {
        let t = Instant::now();
        let mut kube = flight(t);
        kube.begin_attempt();
        kube.observe(&known("starting"), t + secs(2));
        assert_eq!(
            stages(&kube, t + secs(3)),
            [
                (UserStage::Requesting, RowState::Done),
                (UserStage::Starting, RowState::Current),
                (UserStage::Connecting, RowState::Pending),
            ]
        );
        let mut docker = flight(t);
        docker.begin_attempt();
        docker.observe(&known("image"), t + secs(3));
        let rows = docker.checklist(t + secs(51));
        assert_eq!(rows[0].seconds, Some(3));
        assert_eq!(
            (rows[1].stage, rows[1].seconds, rows[1].state),
            (UserStage::PullingImage, Some(48), RowState::Current)
        );
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn stage_sentences_match_the_web_launcher() {
        let none = Map::new();
        assert_eq!(
            stage_sentence("requesting", &none),
            "Sending the launch request"
        );
        assert_eq!(
            stage_sentence("placing", &none),
            "Checking limits and choosing a node"
        );
        let node = json!({"node": "n2"}).as_object().unwrap().clone();
        assert_eq!(
            stage_sentence("forwarding", &node),
            "Handing the launch to node n2"
        );
        assert_eq!(
            stage_sentence("waiting", &none),
            "Waiting for the desktop to respond"
        );
        assert_eq!(
            stage_sentence("client", &none),
            "Preparing the application’s web client"
        );
        assert_eq!(stage_sentence("mystery", &none), "mystery");
    }

    #[test]
    fn times_read_as_seconds_then_minutes() {
        assert_eq!(stage_time(2), "2 s");
        assert_eq!(stage_time(59), "59 s");
        assert_eq!(stage_time(65), "1:05");
        assert_eq!(clock(42), "0:42");
        assert_eq!(clock(727), "12:07");
        assert_eq!(clock(3723), "1:02:03");
    }

    #[test]
    fn poll_interval_slows_after_thirty_seconds_and_on_errors() {
        let t = Instant::now();
        let mut f = flight(t);
        assert_eq!(f.poll_interval(t + secs(1)), FAST_POLL);
        assert_eq!(f.poll_interval(t + secs(30)), SLOW_POLL);
        f.poll_failed();
        assert_eq!(f.poll_interval(t + secs(1)), SLOW_POLL);
        f.observe(&known("placing"), t + secs(2));
        assert_eq!(f.poll_interval(t + secs(2)), FAST_POLL);
    }

    #[test]
    fn stall_prompt_appears_after_180s_without_stage_change_and_keep_waiting_defers_it() {
        let t = Instant::now();
        let mut f = flight(t);
        f.begin_attempt();
        f.observe(&known("starting"), t + secs(10));
        assert!(!f.stalled(t + secs(189)));
        assert!(f.stalled(t + secs(190)));
        assert_eq!(f.stage_minutes(t + secs(190)), 3);
        assert_eq!(f.stage().stall_message(3), "Still starting after 3 min.");
        assert!(f.stage().stall_hint().is_some());
        assert_eq!(f.checklist(t + secs(190))[1].state, RowState::Stalled);
        f.keep_waiting(t + secs(190));
        assert!(!f.stalled(t + secs(300)));
        assert!(f.stalled(t + secs(370)));
        // A new stage resets the prompt.
        f.observe(&known("waiting"), t + secs(371));
        assert!(!f.stalled(t + secs(400)));
        assert_eq!(UserStage::Requesting.stall_hint(), None);
    }

    #[test]
    fn lost_request_keeps_following_while_the_server_knows_the_launch() {
        let t = Instant::now();
        let mut f = flight(t);
        f.begin_attempt();
        f.sent(t + secs(1));
        f.observe(&known("starting"), t + secs(5));
        f.posted(Err(lost()), t + secs(60));
        f.observe(&known("starting"), t + secs(100));
        assert!(f.is_running());
        f.observe(&ready(), t + secs(120));
        assert!(matches!(&f.outcome, Outcome::Ready(s) if s.session_id == "new"));
    }

    #[test]
    fn a_404_while_the_post_is_outstanding_is_not_lost() {
        let t = Instant::now();
        let mut f = flight(t);
        f.begin_attempt();
        f.sent(t);
        f.observe(&ProgressAnswer::Unknown, t + secs(300));
        assert!(f.is_running());
        assert_eq!(f.stage(), UserStage::Requesting);
    }

    #[test]
    fn unknown_launch_twenty_seconds_after_sending_is_lost() {
        let t = Instant::now();
        let mut f = flight(t);
        f.begin_attempt();
        f.sent(t);
        f.posted(Err(lost()), t + secs(5));
        f.observe(&ProgressAnswer::Unknown, t + secs(10));
        assert!(f.is_running());
        f.observe(&ProgressAnswer::Unknown, t + secs(21));
        assert!(matches!(f.outcome, Outcome::Checking));
        f.sessions_checked(Some(&[session("old", "firefox", T0)]), t + secs(22));
        assert!(matches!(f.outcome, Outcome::Lost));
        assert_eq!(f.failure_body(), Some(LOST_BODY));
        assert_eq!(
            stages(&f, t + secs(22)),
            [(UserStage::Requesting, RowState::Failed)]
        );
    }

    #[test]
    fn unseen_counts_from_sent_not_started() {
        let t = Instant::now();
        let mut f = flight(t);
        f.begin_attempt();
        // Creating the Auto home took 30 s before the request went out.
        f.sent(t + secs(30));
        f.posted(Err(lost()), t + secs(31));
        f.observe(&ProgressAnswer::Unknown, t + secs(45));
        assert!(f.is_running());
        f.observe(&ProgressAnswer::Unknown, t + secs(51));
        assert!(matches!(f.outcome, Outcome::Checking));
    }

    #[test]
    fn a_404_after_known_waits_for_the_post() {
        let t = Instant::now();
        let mut f = flight(t);
        f.begin_attempt();
        f.sent(t);
        f.observe(&known("starting"), t + secs(5));
        // Pruned while the request still runs: keep waiting for it.
        f.observe(&ProgressAnswer::Unknown, t + secs(900));
        assert!(f.is_running());
        f.posted(Ok(session("new", "firefox", T0)), t + secs(950));
        assert!(matches!(f.outcome, Outcome::Ready(_)));
    }

    #[test]
    fn lost_flight_adopts_a_new_session_of_the_same_app() {
        let t = Instant::now();
        let mut f = flight(t);
        f.begin_attempt();
        f.sent(t);
        f.posted(Err(lost()), t + secs(1));
        f.observe(&ProgressAnswer::Unknown, t + secs(30));
        let sessions = [
            session("old", "firefox", T0 - 500.0),
            session("other", "zen", T0 + 5.0),
            session("stale", "firefox", T0 - 3600.0),
            session("adopt", "firefox", T0 + 10.0),
        ];
        f.sessions_checked(Some(&sessions), t + secs(31));
        assert!(matches!(&f.outcome, Outcome::Ready(s) if s.session_id == "adopt"));
    }

    #[test]
    fn failed_progress_waits_for_an_outstanding_post() {
        let t = Instant::now();
        let mut f = flight(t);
        f.begin_attempt();
        f.observe(
            &ProgressAnswer::Known(LaunchProgress {
                stage: "failed".into(),
                error: Some("No GPU is free on any node.".into()),
                ..LaunchProgress::default()
            }),
            t + secs(5),
        );
        assert!(f.is_running());
        f.posted(Err(LaunchError::Server("refused".into())), t + secs(6));
        assert_eq!(f.failure_body(), Some("No GPU is free on any node."));
    }

    #[test]
    fn post_ok_wins_over_failed_progress() {
        let t = Instant::now();
        let mut f = flight(t);
        f.begin_attempt();
        f.observe(&known("failed"), t + secs(5));
        f.posted(Ok(session("new", "firefox", T0)), t + secs(6));
        assert!(matches!(f.outcome, Outcome::Ready(_)));
    }

    #[test]
    fn retry_is_offered_only_after_a_lost_request() {
        let t = Instant::now();
        let mut f = flight(t);
        f.begin_attempt();
        f.observe(&known("starting"), t + secs(5));
        assert!(f.stalled(t + secs(200)));
        assert!(!f.can_retry_same());
        f.posted(Err(lost()), t + secs(210));
        assert!(f.can_retry_same());
        f.retry_same(t + secs(211));
        assert!(!f.can_retry_same());
    }

    #[test]
    fn retry_after_stall_keeps_the_launch_id_retry_after_failure_makes_a_new_one() {
        let t = Instant::now();
        let mut f = flight(t);
        let first = f.launch_id.clone();
        assert_eq!(f.params.launch_id.as_deref(), Some(first.as_str()));
        f.begin_attempt();
        f.posted(Err(lost()), t + secs(5));
        f.retry_same(t + secs(200));
        assert_eq!(f.launch_id, first);
        f.posted(Err(LaunchError::Server("no".into())), t + secs(201));
        f.observe(&ProgressAnswer::Unknown, t + secs(202));
        assert!(matches!(f.outcome, Outcome::Failed(_)));
        f.retry_fresh(HashSet::new(), t + secs(300), T0 + 300.0);
        assert_ne!(f.launch_id, first);
        assert_eq!(f.params.launch_id.as_deref(), Some(f.launch_id.as_str()));
        assert!(f.is_running());
        assert_eq!(f.stage(), UserStage::Requesting);
    }

    #[test]
    fn ready_progress_finishes_even_when_the_post_failed() {
        let t = Instant::now();
        let mut f = flight(t);
        f.begin_attempt();
        f.posted(Err(lost()), t + secs(5));
        f.observe(&ready(), t + secs(6));
        assert!(matches!(&f.outcome, Outcome::Ready(s) if s.session_url == "/new/"));
    }

    #[test]
    fn late_post_after_ready_progress_is_ignored() {
        let t = Instant::now();
        let mut f = flight(t);
        f.begin_attempt();
        f.observe(&ready(), t + secs(6));
        f.posted(Ok(session("late", "firefox", T0)), t + secs(7));
        assert!(matches!(&f.outcome, Outcome::Ready(s) if s.session_id == "new"));
        f.observe(&known("failed"), t + secs(8));
        assert!(matches!(&f.outcome, Outcome::Ready(s) if s.session_id == "new"));
    }
}
