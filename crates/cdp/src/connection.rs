//! [`Connection`]: a supervised, self-healing CDP connection to a Remote
//! Browser.
//!
//! A bare [`Client`] dies with its WebSocket. A `Connection` owns the URL and
//! token, connects (and reconnects) with exponential backoff, and rebuilds
//! the state the UI relies on after every successful connect:
//!
//! * `Target.setDiscoverTargets` is enabled again and the page-target list
//!   is replaced from `Target.getTargets`. The list ([`Connection::pages`])
//!   outlives drops, so the Sidebar can keep showing (dimmed) Tabs while
//!   reconnecting.
//! * If an Active Tab was declared with [`Connection::set_active_tab`], it is
//!   re-attached and its screencast restarted. Frames keep arriving on the
//!   one stream from [`Connection::take_frames`]. Target ids may change after
//!   a browser restart: if the Active Tab's target id no longer exists,
//!   [`ConnectionEvent::ActiveTabGone`] is emitted and nothing is guessed.
//!
//! # Handles and pending commands
//!
//! A [`Client`] or [`Session`] belongs to one WebSocket. When it drops,
//! pending commands fail immediately with [`CdpError::Closed`] (nothing is
//! queued). After a reconnect callers get a *fresh* handle: watch
//! [`Connection::active_session`] for the Active Tab's Session, or call
//! [`Connection::client`]. Old handles stay dead.
//!
//! # States
//!
//! See [`ConnectionState`]. Transport errors are retried forever with
//! backoff. [`ConnectionState::Failed`] (HTTP 401/403 on the handshake, close
//! code 4401/4403) and [`ConnectionState::NotRunning`] (HTTP 404 on the
//! handshake, close code 4004: the Profile is not running) are *not* retried;
//! call [`Connection::retry`] once the cause is fixed (e.g. after the
//! Manager started the Profile).

use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::client::ConnectFailure;
use crate::{
    CdpError, Client, Frame, ScreencastOptions, Session, TargetEvent, TargetInfo, TargetList, Token,
};

/// Timeout for each connect attempt and each rebuild command.
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

/// Exponential backoff settings. Default: 1 s doubling to 30 s with 20 %
/// jitter; a connection must stay up 10 s to reset the schedule.
#[derive(Debug, Clone, PartialEq)]
pub struct BackoffConfig {
    pub initial: Duration,
    pub max: Duration,
    /// Fraction `0.0..=1.0` of the delay that may be randomly subtracted.
    /// `0.0` gives an exact schedule (useful in tests).
    pub jitter: f64,
    /// A connection that lasted at least this long resets the schedule.
    pub stable_after: Duration,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        BackoffConfig {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(30),
            jitter: 0.2,
            stable_after: Duration::from_secs(10),
        }
    }
}

impl BackoffConfig {
    /// Delay before retry number `attempt` (1-based): `initial * 2^(n-1)`
    /// capped at `max`, minus up to `jitter` of it. `unit_random` in
    /// `0.0..1.0` picks the point inside the jitter range.
    pub fn delay(&self, attempt: u32, unit_random: f64) -> Duration {
        let exp = attempt.saturating_sub(1).min(32);
        let base = self
            .initial
            .saturating_mul(1u32.checked_shl(exp).unwrap_or(u32::MAX))
            .min(self.max);
        let j = self.jitter.clamp(0.0, 1.0) * unit_random.clamp(0.0, 1.0);
        base.mul_f64(1.0 - j)
    }
}

/// What to connect to and how to retry.
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub url: String,
    pub token: Option<Token>,
    pub backoff: BackoffConfig,
}

impl ConnectionConfig {
    pub fn new(url: impl Into<String>, token: Option<Token>) -> Self {
        ConnectionConfig {
            url: url.into(),
            token,
            backoff: BackoffConfig::default(),
        }
    }
}

/// Why a Connection gave up (not retried automatically).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureReason {
    /// HTTP 401 on the handshake or WebSocket close 4401: bad token.
    Unauthorized,
    /// HTTP 403 on the handshake or WebSocket close 4403: origin rejected.
    OriginRejected,
}

/// Connection status for the UI (e.g. a "Reconnecting..." overlay).
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    /// First connect attempt in progress.
    Connecting,
    /// Connected and state rebuilt.
    Connected,
    /// Down; retry number `attempt` (1-based) starts in `next_retry_in`
    /// (measured from when this state was published).
    Reconnecting {
        attempt: u32,
        next_retry_in: Duration,
    },
    /// The Remote Browser's Profile is not running (HTTP 404 on the
    /// handshake, close code 4004). Not retried: offer "Start Profile",
    /// then call [`Connection::retry`].
    NotRunning,
    /// Authentication or origin failure. Not retried.
    Failed { reason: FailureReason },
    /// [`Connection::close`] was called.
    Closed,
}

/// The Active Tab to keep mirrored across reconnects.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveTab {
    pub target_id: String,
    pub screencast: ScreencastOptions,
}

/// Notable things that happen on a Connection.
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionEvent {
    /// A (re)connect succeeded and state was rebuilt.
    Connected,
    /// The WebSocket went away; `close_code` is set if the server sent one.
    Disconnected { close_code: Option<u16> },
    /// The declared Active Tab's target no longer exists (after a reconnect,
    /// or it was destroyed). The screencast is not running; pick another Tab
    /// with [`Connection::set_active_tab`].
    ActiveTabGone { target_id: String },
    /// Attaching or starting the screencast for the Active Tab failed.
    ScreencastFailed { target_id: String, error: String },
}

enum Cmd {
    SetActive(Option<ActiveTab>),
    Retry,
    Close,
}

/// A supervised CDP connection. See the [module docs](self). Dropping it
/// stops the supervisor and closes the WebSocket.
pub struct Connection {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    state: watch::Receiver<ConnectionState>,
    pages: watch::Receiver<Vec<TargetInfo>>,
    session: watch::Receiver<Option<Session>>,
    client: watch::Receiver<Option<Client>>,
    events: broadcast::Sender<ConnectionEvent>,
    frames: std::sync::Mutex<Option<mpsc::Receiver<Frame>>>,
    task: JoinHandle<()>,
}

impl Connection {
    /// Start supervising. Returns immediately; the first connect happens in
    /// the background (state starts as [`ConnectionState::Connecting`]).
    pub fn start(config: ConnectionConfig) -> Connection {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (state_tx, state) = watch::channel(ConnectionState::Connecting);
        let (pages_tx, pages) = watch::channel(Vec::new());
        let (session_tx, session) = watch::channel(None);
        let (client_tx, client) = watch::channel(None);
        let (events, _) = broadcast::channel(64);
        let (frames_tx, frames_rx) = mpsc::channel(2);
        let ctx = Ctx {
            config,
            state_tx,
            pages_tx,
            session_tx,
            client_tx,
            events: events.clone(),
            frames_tx,
            desired: None,
            rng: seed(),
        };
        let task = tokio::spawn(run(ctx, cmd_rx));
        Connection {
            cmd_tx,
            state,
            pages,
            session,
            client,
            events,
            frames: std::sync::Mutex::new(Some(frames_rx)),
            task,
        }
    }

    /// Current state and change notifications.
    pub fn state(&self) -> watch::Receiver<ConnectionState> {
        self.state.clone()
    }

    /// Page targets (the raw material for Tabs). Replaced from
    /// `Target.getTargets` after each reconnect and kept (stale) while down.
    pub fn pages(&self) -> watch::Receiver<Vec<TargetInfo>> {
        self.pages.clone()
    }

    /// The Session of the Active Tab, `None` while down or if no Active Tab
    /// is running. A new Session is published after every reconnect.
    pub fn active_session(&self) -> watch::Receiver<Option<Session>> {
        self.session.clone()
    }

    /// The live [`Client`], `None` while down. Handles die with their
    /// WebSocket; fetch a fresh one after a reconnect.
    pub fn client(&self) -> Option<Client> {
        self.client.borrow().clone()
    }

    /// Subscribe to [`ConnectionEvent`]s (events before this call are not
    /// replayed).
    pub fn events(&self) -> broadcast::Receiver<ConnectionEvent> {
        self.events.subscribe()
    }

    /// The single Frame stream of the Active Tab; it continues across
    /// reconnects and ends (`None`) only when the Connection is closed.
    /// Can be taken once; later calls return `None`.
    pub fn take_frames(&self) -> Option<mpsc::Receiver<Frame>> {
        self.frames.lock().unwrap().take()
    }

    /// Declare (or clear, with `None`) the Active Tab. If connected it is
    /// attached and its screencast (re)started now, and again after every
    /// reconnect. Replaces any previous Active Tab.
    pub fn set_active_tab(&self, tab: Option<ActiveTab>) {
        let _ = self.cmd_tx.send(Cmd::SetActive(tab));
    }

    /// Reconnect now: skips a backoff wait, or leaves
    /// [`ConnectionState::NotRunning`] / [`ConnectionState::Failed`] and
    /// resets the backoff. No effect while connected.
    pub fn retry(&self) {
        let _ = self.cmd_tx.send(Cmd::Retry);
    }

    /// Close for good; resolves when the state is [`ConnectionState::Closed`].
    pub async fn close(&self) {
        let _ = self.cmd_tx.send(Cmd::Close);
        let mut s = self.state.clone();
        let _ = s.wait_for(|s| *s == ConnectionState::Closed).await;
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Ctx {
    config: ConnectionConfig,
    state_tx: watch::Sender<ConnectionState>,
    pages_tx: watch::Sender<Vec<TargetInfo>>,
    session_tx: watch::Sender<Option<Session>>,
    client_tx: watch::Sender<Option<Client>>,
    events: broadcast::Sender<ConnectionEvent>,
    frames_tx: mpsc::Sender<Frame>,
    desired: Option<ActiveTab>,
    rng: u64,
}

impl Ctx {
    fn set_state(&self, s: ConnectionState) {
        self.state_tx.send_replace(s);
    }

    fn emit(&self, e: ConnectionEvent) {
        let _ = self.events.send(e);
    }

    /// xorshift64*, enough for jitter.
    fn unit_random(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
        | 1
}

enum Idle {
    Elapsed,
    Retry,
    Close,
}

/// Wait for `dur` (forever if `None`), handling commands meanwhile.
async fn idle(
    ctx: &mut Ctx,
    cmd_rx: &mut mpsc::UnboundedReceiver<Cmd>,
    dur: Option<Duration>,
) -> Idle {
    let sleep = async {
        match dur {
            Some(d) => tokio::time::sleep(d).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            _ = &mut sleep => return Idle::Elapsed,
            cmd = cmd_rx.recv() => match cmd {
                None | Some(Cmd::Close) => return Idle::Close,
                Some(Cmd::Retry) => return Idle::Retry,
                Some(Cmd::SetActive(a)) => ctx.desired = a,
            },
        }
    }
}

enum End {
    Close,
    Dropped,
}

async fn run(mut ctx: Ctx, mut cmd_rx: mpsc::UnboundedReceiver<Cmd>) {
    let mut attempt: u32 = 0;
    loop {
        let (url, token) = (ctx.config.url.clone(), ctx.config.token.clone());
        let connect = tokio::time::timeout(STEP_TIMEOUT, Client::connect_raw(&url, token));
        // Commands (Close) must stay responsive while connecting.
        let res = tokio::select! {
            r = connect => r.unwrap_or_else(|_| Err(ConnectFailure::Transport("connect timed out".into()))),
            _ = next_close_or_active(&mut ctx, &mut cmd_rx) => { ctx.set_state(ConnectionState::Closed); return; }
        };

        let mut fatal = None;
        match res {
            Ok(client) => {
                let up_since = Instant::now();
                ctx.client_tx.send_replace(Some(client.clone()));
                let end = serve(&mut ctx, &mut cmd_rx, &client).await;
                ctx.client_tx.send_replace(None);
                ctx.session_tx.send_replace(None);
                if let End::Close = end {
                    ctx.set_state(ConnectionState::Closed);
                    return;
                }
                let code = client.close_code();
                ctx.emit(ConnectionEvent::Disconnected { close_code: code });
                if up_since.elapsed() >= ctx.config.backoff.stable_after {
                    attempt = 0;
                }
                fatal = match code {
                    Some(4401) => Some(ConnectionState::Failed {
                        reason: FailureReason::Unauthorized,
                    }),
                    Some(4403) => Some(ConnectionState::Failed {
                        reason: FailureReason::OriginRejected,
                    }),
                    Some(4004) => Some(ConnectionState::NotRunning),
                    _ => None,
                };
            }
            Err(ConnectFailure::Http(401)) => {
                fatal = Some(ConnectionState::Failed {
                    reason: FailureReason::Unauthorized,
                })
            }
            Err(ConnectFailure::Http(403)) => {
                fatal = Some(ConnectionState::Failed {
                    reason: FailureReason::OriginRejected,
                })
            }
            Err(ConnectFailure::Http(404)) => fatal = Some(ConnectionState::NotRunning),
            Err(_) => {}
        }

        if let Some(state) = fatal {
            ctx.set_state(state);
            match idle(&mut ctx, &mut cmd_rx, None).await {
                Idle::Close => {
                    ctx.set_state(ConnectionState::Closed);
                    return;
                }
                _ => {
                    attempt = 0;
                    ctx.set_state(ConnectionState::Connecting);
                    continue;
                }
            }
        }

        attempt += 1;
        let r = ctx.unit_random();
        let delay = ctx.config.backoff.delay(attempt, r);
        ctx.set_state(ConnectionState::Reconnecting {
            attempt,
            next_retry_in: delay,
        });
        if let Idle::Close = idle(&mut ctx, &mut cmd_rx, Some(delay)).await {
            ctx.set_state(ConnectionState::Closed);
            return;
        }
    }
}

/// Resolves on Close (or channel end); applies other commands to `ctx`
/// without resolving. Used to keep a connect attempt cancellable.
async fn next_close_or_active(ctx: &mut Ctx, cmd_rx: &mut mpsc::UnboundedReceiver<Cmd>) {
    loop {
        match cmd_rx.recv().await {
            None | Some(Cmd::Close) => return,
            Some(Cmd::Retry) => {}
            Some(Cmd::SetActive(a)) => ctx.desired = a,
        }
    }
}

struct ActiveRun {
    target_id: String,
    fwd: JoinHandle<()>,
    session: Session,
}

/// One live WebSocket: rebuild state, then follow events and commands until
/// it drops or the Connection is closed.
async fn serve(ctx: &mut Ctx, cmd_rx: &mut mpsc::UnboundedReceiver<Cmd>, client: &Client) -> End {
    let mut events = client.events(); // before discover: no lost replays
    let setup = async {
        client.set_discover_targets(true).await?;
        client.get_targets().await
    };
    let infos = match tokio::time::timeout(STEP_TIMEOUT, setup).await {
        Ok(Ok(i)) => i,
        _ => {
            client.close().await;
            return End::Dropped;
        }
    };
    let mut list = TargetList::default();
    list.replace(infos);
    ctx.pages_tx.send_replace(list.pages().to_vec());
    ctx.set_state(ConnectionState::Connected);
    ctx.emit(ConnectionEvent::Connected);

    let mut active: Option<ActiveRun> = None;
    start_active(ctx, client, &list, &mut active).await;

    let end = loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break End::Dropped };
                if ev.session_id.is_some() { continue; }
                if let Some(te) = TargetEvent::from_event(&ev) {
                    if list.apply(&te) {
                        ctx.pages_tx.send_replace(list.pages().to_vec());
                    }
                    if let TargetEvent::Destroyed { target_id } = &te {
                        if active.as_ref().is_some_and(|a| &a.target_id == target_id) {
                            stop_active(ctx, &mut active, false).await;
                            ctx.emit(ConnectionEvent::ActiveTabGone { target_id: target_id.clone() });
                        }
                    }
                }
            }
            _ = client.closed() => break End::Dropped,
            cmd = cmd_rx.recv() => match cmd {
                None | Some(Cmd::Close) => break End::Close,
                Some(Cmd::Retry) => {}
                Some(Cmd::SetActive(a)) => {
                    ctx.desired = a;
                    stop_active(ctx, &mut active, true).await;
                    start_active(ctx, client, &list, &mut active).await;
                }
            },
        }
    };
    stop_active(ctx, &mut active, matches!(end, End::Close)).await;
    client.close().await;
    end
}

async fn stop_active(ctx: &Ctx, active: &mut Option<ActiveRun>, tell_browser: bool) {
    if let Some(a) = active.take() {
        a.fwd.abort();
        ctx.session_tx.send_replace(None);
        if tell_browser {
            let _ = tokio::time::timeout(STEP_TIMEOUT, a.session.stop_screencast()).await;
        }
    }
}

async fn start_active(
    ctx: &Ctx,
    client: &Client,
    list: &TargetList,
    active: &mut Option<ActiveRun>,
) {
    let Some(want) = ctx.desired.clone() else {
        return;
    };
    if !list.pages().iter().any(|p| p.target_id == want.target_id) {
        ctx.emit(ConnectionEvent::ActiveTabGone {
            target_id: want.target_id,
        });
        return;
    }
    let started = async {
        let session = client.attach_to_target(&want.target_id).await?;
        let sc = session.start_screencast(want.screencast.clone()).await?;
        Ok::<_, CdpError>((session, sc))
    };
    match tokio::time::timeout(STEP_TIMEOUT, started).await {
        Ok(Ok((session, mut sc))) => {
            let tx = ctx.frames_tx.clone();
            let fwd = tokio::spawn(async move {
                while let Some(f) = sc.next().await {
                    if tx.send(f).await.is_err() {
                        break;
                    }
                }
            });
            ctx.session_tx.send_replace(Some(session.clone()));
            *active = Some(ActiveRun {
                target_id: want.target_id,
                fwd,
                session,
            });
        }
        Ok(Err(e)) => ctx.emit(ConnectionEvent::ScreencastFailed {
            target_id: want.target_id,
            error: e.to_string(),
        }),
        Err(_) => ctx.emit(ConnectionEvent::ScreencastFailed {
            target_id: want.target_id,
            error: "timed out".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_schedule_doubles_and_caps() {
        let b = BackoffConfig {
            jitter: 0.0,
            ..Default::default()
        };
        let secs: Vec<u64> = (1..=8).map(|n| b.delay(n, 0.7).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(b.delay(u32::MAX, 0.0), Duration::from_secs(30));
    }

    #[test]
    fn jitter_only_shortens_within_fraction() {
        let b = BackoffConfig::default();
        assert_eq!(b.delay(3, 0.0), Duration::from_secs(4));
        let d = b.delay(3, 0.999);
        assert!(d >= Duration::from_millis(3200) && d < Duration::from_secs(4));
    }
}
