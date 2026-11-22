//! The account menu and key recovery (`ARCH-029`): what a refused key or account leads to,
//! the wake poll after Sign In on the Web, and the account menu's storage measure.
//!
//! Recovery never touches `OpenWindows`: session windows stream on their own per-session
//! cookies and keep their own `SealSkinClient`, so a refused or replaced key reaches them
//! only when their window is opened again.

use super::{Launcher, LoadKind};
use crate::sealskin::{
    AccountBlock, ApiError, Credential, KeyRefusal, KeySource, SealSkinClient, ServerStatus,
};
use gpui_kit::component::Theme;
use gpui_kit::{div, prelude::*, Context, Div, KeyDownEvent, Window};
use skerry_ui::{
    account::{self, AccessChip, AccountProps, Limits, StorageMeasure, StorageUse},
    connect::{
        self, ConnectHandlers, ConnectProps, EnvNote, KeyProblem, ProblemKind, RecoveryKind,
        RecoveryProps,
    },
    session::KeySourceLabel,
};
use std::future::Future;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How often the wake poll checks the key.
pub(super) const WAKE_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// How long the wake poll keeps checking. Each refused check is audited on the server, so
/// the poll is capped (36 checks). Coming back to the window checks at once while waiting,
/// and Try Again covers a sign-in that took longer, so this cap needn’t grow.
pub(super) const WAKE_POLL_LIMIT: Duration = Duration::from_secs(180);

/// The panel a dormant key or held account shows on the Connect screen.
pub(super) struct Recovery {
    pub kind: RecoveryKind,
    /// The refused key, kept to retry; never saved or removed here.
    pub api: SealSkinClient,
    /// Set when Sign In on the Web opened the browser and the poll runs.
    pub waking_since: Option<Instant>,
    pub timed_out: bool,
}

impl Recovery {
    pub fn props(&self) -> RecoveryProps {
        RecoveryProps {
            kind: self.kind,
            waiting: self.waking_since.is_some(),
            timed_out: self.timed_out,
        }
    }
}

/// The account menu's storage measure, once per Library load; only a failed one is retried.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Measure {
    #[default]
    NotAsked,
    Measuring,
    /// The server answered (`None`: it reported no usage).
    Done(Option<u64>),
    /// The request failed; the next opening of the menu measures again.
    Failed,
}

impl Measure {
    pub fn wants_measure(self) -> bool {
        matches!(self, Self::NotAsked | Self::Failed)
    }
}

/// The wake poll's bookkeeping: which wake is current, whether its check is out, and
/// whether a check succeeded while a load was running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct WakeState {
    /// Bumped whenever a poll starts or stops, so a stale check is ignored.
    pub generation: u64,
    in_flight: bool,
    /// A check said the key works while a load was busy; the load's Dormant answer (sent
    /// before the wake landed) then reloads once instead of showing the panel again.
    pub confirmed: bool,
}

impl WakeState {
    /// A poll starts (or stops): a check still out belongs to the old one, so nothing is
    /// in flight for the new one.
    pub fn restart(&mut self) -> u64 {
        self.generation += 1;
        self.in_flight = false;
        self.confirmed = false;
        self.generation
    }

    /// Whether a check may start now; marks it in flight if so.
    pub fn begin_check(&mut self, busy: bool) -> bool {
        if busy || self.in_flight {
            return false;
        }
        self.in_flight = true;
        true
    }

    /// A check of wake `generation` answered; whether it is still the current wake.
    pub fn finish_check(&mut self, generation: u64) -> bool {
        if generation != self.generation {
            return false;
        }
        self.in_flight = false;
        true
    }
}

/// The key to save once a kept client connects: the pasted credential, if it is the one the
/// client uses (a key pasted, then refused, then waking or retried is saved when it works).
pub(super) fn kept_save(pending: Option<&Credential>, api: &SealSkinClient) -> Option<Credential> {
    pending
        .filter(|credential| credential.raw() == api.raw_key())
        .cloned()
}

/// What a refusal leads to.
#[derive(Debug, PartialEq)]
pub(super) struct RecoveryPlan {
    /// A panel instead of the key form (dormant, held).
    pub panel: Option<RecoveryKind>,
    /// An alert above the key field.
    pub problem: Option<KeyProblem>,
    /// An empty Connect may retry this key (an administrator can fix it without a new key).
    pub retry_saved: bool,
    pub focus_key: bool,
}

/// What a key's source means for the refusal's environment note.
fn env_note(source: KeySourceLabel) -> Option<EnvNote> {
    match source {
        KeySourceLabel::Environment => Some(EnvNote::Environment),
        KeySourceLabel::DotEnv => Some(EnvNote::DotEnv),
        KeySourceLabel::SavedFile | KeySourceLabel::ThisRunOnly => None,
    }
}

/// The label of the key a client uses, before any sign-in recorded it.
pub(super) fn source_label(source: KeySource, in_config: impl FnOnce() -> bool) -> KeySourceLabel {
    match source {
        KeySource::Environment => KeySourceLabel::Environment,
        KeySource::DotEnv => KeySourceLabel::DotEnv,
        KeySource::ConfigFile => KeySourceLabel::SavedFile,
        KeySource::Typed if in_config() => KeySourceLabel::SavedFile,
        KeySource::Typed => KeySourceLabel::ThisRunOnly,
    }
}

/// What a refused key or account leads to (`ARCH-029`). `None` for an error that isn't a
/// refusal.
pub(super) fn recovery_for(error: &ApiError, source: KeySourceLabel) -> Option<RecoveryPlan> {
    let problem = |kind: ProblemKind| {
        let retry_saved = !matches!(kind, ProblemKind::Revoked | ProblemKind::Expired);
        Some(RecoveryPlan {
            panel: None,
            problem: Some(KeyProblem {
                kind,
                env_note: env_note(source),
            }),
            retry_saved,
            focus_key: true,
        })
    };
    let panel = |kind| {
        Some(RecoveryPlan {
            panel: Some(kind),
            problem: None,
            retry_saved: true,
            focus_key: false,
        })
    };
    match error {
        ApiError::Key(KeyRefusal::Dormant) => panel(RecoveryKind::Dormant),
        ApiError::Account(AccountBlock::Held) => panel(RecoveryKind::Held),
        ApiError::Key(KeyRefusal::Revoked) => problem(ProblemKind::Revoked),
        ApiError::Key(KeyRefusal::Expired) => problem(ProblemKind::Expired),
        ApiError::Key(KeyRefusal::KeysDisabled) => problem(ProblemKind::KeysDisabled),
        ApiError::Key(KeyRefusal::ClientNotAllowed) => problem(ProblemKind::ClientNotAllowed),
        ApiError::Key(KeyRefusal::Other(detail)) => problem(ProblemKind::OtherKey(detail.clone())),
        ApiError::Account(AccountBlock::Inactive) => problem(ProblemKind::Inactive),
        ApiError::Account(AccountBlock::Other(detail)) => {
            problem(ProblemKind::OtherAccount(detail.clone()))
        }
        ApiError::Forbidden(_) | ApiError::Message(_) => None,
    }
}

/// What Connect does.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Submit {
    /// Parse the key in the field.
    Parse,
    /// Retry the client kept after a refusal, a version refusal, or Use a Different Key.
    RetryKept,
    /// Retry the key read at startup.
    RetrySaved,
    /// Say this under the field instead.
    Hint(&'static str),
}

/// What Connect does with the field as it is. A dead key (revoked, expired) is never sent
/// again, whatever is kept.
pub(super) fn submit_decision(
    field_empty: bool,
    problem: Option<&KeyProblem>,
    has_retry_api: bool,
    has_startup_credential: bool,
) -> Submit {
    if !field_empty {
        Submit::Parse
    } else if problem.is_some_and(KeyProblem::needs_new_key) {
        Submit::Hint(skerry_ui::connect::PASTE_NEW_KEY)
    } else if has_retry_api {
        Submit::RetryKept
    } else if has_startup_credential {
        Submit::RetrySaved
    } else {
        Submit::Hint("Enter a SealSkin application key first.")
    }
}

/// Whether the wake poll keeps going: within its time limit and still the current wake.
pub(super) fn poll_should_continue(
    started: Instant,
    now: Instant,
    generation: u64,
    current: u64,
) -> bool {
    generation == current && now.saturating_duration_since(started) < WAKE_POLL_LIMIT
}

/// One step of the wake poll.
pub(super) enum Tick<C> {
    Stop,
    /// Not this time (a load is running); wait for the next tick.
    Skip,
    Check(C),
}

/// The wake poll's loop: every `interval`, `next` says whether to check, `check` checks,
/// and `apply` takes the result and says whether to go on. Dropping the future (the
/// Launcher's `wake_poll` task) stops it between any two steps.
pub(super) async fn poll_ticks<C, R, S, SF, K, KF>(
    interval: Duration,
    mut sleep: S,
    mut next: impl FnMut() -> Tick<C>,
    mut check: K,
    mut apply: impl FnMut(R) -> bool,
) where
    S: FnMut(Duration) -> SF,
    SF: Future<Output = ()>,
    K: FnMut(C) -> KF,
    KF: Future<Output = R>,
{
    loop {
        sleep(interval).await;
        let checking = match next() {
            Tick::Stop => return,
            Tick::Skip => continue,
            Tick::Check(checking) => checking,
        };
        let result = check(checking).await;
        if !apply(result) {
            return;
        }
    }
}

/// The account menu's props, from the Library's status. The built-in `admin` user's keys
/// never sleep, so it gets no dormancy line; other administrators' keys do.
pub(super) fn account_props(
    status: &ServerStatus,
    running: usize,
    measure: Measure,
    server: &str,
    key_source: Option<KeySourceLabel>,
    key_id: &str,
) -> AccountProps {
    let settings = &status.settings;
    let storage = settings.persistent_storage.then_some(StorageUse {
        limit_gb: settings.storage_limit,
        used: match (measure, status.storage_used) {
            (Measure::Measuring, _) => StorageMeasure::Measuring,
            (_, Some(used)) => StorageMeasure::Bytes(used),
            _ => StorageMeasure::Unknown,
        },
    });
    let limits = Limits {
        running,
        session_limit: settings.session_limit,
        session_cpus: settings.session_cpus,
        session_memory_mb: settings.session_memory_mb,
        session_hours: settings.session_hours,
        session_idle_minutes: settings.session_idle_minutes,
        storage,
        allowance: status
            .allowance
            .as_ref()
            .map(|allowance| (allowance.used, allowance.hours, allowance.period.clone())),
    };
    let chip = |label: &'static str, allowed| AccessChip {
        label: label.into(),
        allowed,
    };
    AccountProps {
        user: status.username.clone(),
        server: server.to_string(),
        admin: status.is_admin,
        groups: settings.groups.clone(),
        access: vec![
            chip("Persistent storage", settings.persistent_storage),
            chip("GPU", settings.gpu),
            chip("VPN", settings.vpn),
        ],
        limits: if status.is_admin {
            Vec::new()
        } else {
            account::limit_rows(&limits)
        },
        key_source,
        key_id: key_id.to_string(),
        dormancy: (status.username != BUILT_IN_ADMIN)
            .then(|| {
                account::dormancy_text(
                    settings.application_key_dormant_hours,
                    settings.application_key_follow_sign_in,
                )
            })
            .flatten(),
    }
}

/// SealSkin's built-in administrator, whose keys never sleep (`application_keys.py`).
const BUILT_IN_ADMIN: &str = "admin";

impl Launcher {
    /// SealSkin refused the key or the account: drop the Library and show what to do next.
    /// The saved key is untouched (a replacement replaces it once it connects), and session
    /// windows are left alone (`ARCH-029`).
    pub(super) fn on_refusal(
        &mut self,
        api: SealSkinClient,
        error: ApiError,
        cx: &mut Context<Self>,
    ) {
        let source = self
            .api
            .as_ref()
            .filter(|current| current.raw_key() == api.raw_key())
            .map(|_| self.sign_out_label())
            .unwrap_or_else(|| {
                source_label(api.key_source(), || {
                    crate::sealskin::saved_key_is(api.raw_key())
                })
            });
        let Some(plan) = recovery_for(&error, source) else {
            self.status = Some(error.to_string());
            cx.notify();
            return;
        };
        let dormant = plan.panel == Some(RecoveryKind::Dormant);
        // A dormant answer while waking (Try Again raced the browser) keeps the poll.
        let keep_poll = if dormant
            && self
                .recovery
                .as_ref()
                .is_some_and(|recovery| recovery.kind == RecoveryKind::Dormant)
        {
            self.wake_poll.take()
        } else {
            None
        };
        let previous = self.recovery.take();
        self.leave_server();
        self.wake_poll = keep_poll;
        self.status = None;
        self.key_hint = None;
        self.different_key = false;
        self.refused_server = Some(api.server().to_string());
        self.key_problem = plan.problem;
        self.focus_key_input = plan.focus_key;
        if plan.retry_saved {
            self.retry_api = Some(api.clone());
        } else {
            // A dead key is never resent by an empty Connect.
            self.retry_api = None;
            self.startup_credential = None;
            self.pending_save = None;
        }
        self.recovery = plan.panel.map(|kind| {
            let (waking_since, timed_out) = previous
                .filter(|previous| previous.kind == kind && self.wake_poll.is_some())
                .map_or((None, false), |previous| {
                    (previous.waking_since, previous.timed_out)
                });
            Recovery {
                kind,
                api,
                waking_since,
                timed_out,
            }
        });
        if self.recovery.is_none() {
            self.wake_poll = None;
        }
        // A wake check said the key works while this load was out: the load's Dormant
        // answer predates the wake, so reload once.
        if std::mem::take(&mut self.wake.confirmed) && dormant {
            if let Some(api) = self.recovery.as_ref().map(|recovery| recovery.api.clone()) {
                let save = kept_save(self.pending_save.as_ref(), &api);
                self.load(api, save, LoadKind::Connect, cx);
            }
        }
        cx.notify();
    }

    /// A Library load was refused. A pasted key that is kept (to wake or retry) is kept
    /// with its credential, so it is saved once it connects.
    pub(super) fn refused_load(
        &mut self,
        api: SealSkinClient,
        save: Option<Credential>,
        error: ApiError,
        cx: &mut Context<Self>,
    ) {
        let raw = api.raw_key().to_string();
        self.on_refusal(api, error, cx);
        let kept = self
            .retry_api
            .as_ref()
            .is_some_and(|kept| kept.raw_key() == raw);
        if let Some(save) = save.filter(|_| kept) {
            self.pending_save = Some(save);
        }
    }

    /// Opens `<origin>/?wake=1` in the browser (never with the key) and checks the key every
    /// few seconds until it works, for at most three minutes.
    pub(super) fn sign_in_on_web(&mut self, cx: &mut Context<Self>) {
        let Some(recovery) = self.recovery.as_mut() else {
            return;
        };
        cx.open_url(recovery.api.wake_url().as_str());
        recovery.waking_since = Some(Instant::now());
        recovery.timed_out = false;
        self.start_wake_poll(cx);
        cx.notify();
    }

    /// Opens the server's web app (`/`, no key).
    pub(super) fn open_web(&mut self, cx: &mut Context<Self>) {
        let api = self
            .api
            .as_ref()
            .or(self.recovery.as_ref().map(|recovery| &recovery.api));
        if let Some(api) = api {
            cx.open_url(api.web_url().as_str());
        }
    }

    pub(super) fn stop_wake_poll(&mut self) {
        self.wake_poll = None;
        self.wake.restart();
        if let Some(recovery) = self.recovery.as_mut() {
            recovery.waking_since = None;
        }
    }

    fn start_wake_poll(&mut self, cx: &mut Context<Self>) {
        let generation = self.wake.restart();
        let started = Instant::now();
        let window = self.window;
        self.wake_poll = Some(cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            let (mut read, mut write) = (cx.clone(), cx.clone());
            let (this_read, this_write) = (this.clone(), this);
            poll_ticks(
                WAKE_POLL_INTERVAL,
                move |interval| executor.timer(interval),
                move || {
                    window
                        .update(&mut read, |_, _, cx| {
                            this_read.update(cx, |this, cx| this.wake_tick(generation, started, cx))
                        })
                        .ok()
                        .and_then(Result::ok)
                        .unwrap_or(Tick::Stop)
                },
                |(api, rt): (SealSkinClient, tokio::runtime::Handle)| {
                    let pending = rt.spawn({
                        let api = api.clone();
                        async move { api.key_works().await }
                    });
                    async move {
                        let result = pending.await.unwrap_or_else(|_| {
                            Err(ApiError::Message("The check was interrupted.".into()))
                        });
                        (api, result)
                    }
                },
                move |(api, result)| {
                    window
                        .update(&mut write, |_, _, cx| {
                            this_write.update(cx, |this, cx| {
                                this.wake_checked(generation, api, result, cx)
                            })
                        })
                        .ok()
                        .and_then(Result::ok)
                        .unwrap_or(false)
                },
            )
            .await;
        }));
    }

    /// Whether the poll checks now. A running load (Try Again) is never interrupted, and
    /// its own answer decides; the poll checks again at the next tick.
    fn wake_tick(
        &mut self,
        generation: u64,
        started: Instant,
        cx: &mut Context<Self>,
    ) -> Tick<(SealSkinClient, tokio::runtime::Handle)> {
        let Some(recovery) = self
            .recovery
            .as_mut()
            .filter(|recovery| recovery.kind == RecoveryKind::Dormant)
        else {
            return Tick::Stop;
        };
        if !poll_should_continue(started, Instant::now(), generation, self.wake.generation) {
            if generation == self.wake.generation {
                recovery.waking_since = None;
                recovery.timed_out = true;
                self.wake_poll = None;
                cx.notify();
            }
            return Tick::Stop;
        }
        if !self.wake.begin_check(self.busy.is_some()) {
            return Tick::Skip;
        }
        Tick::Check((recovery.api.clone(), self.rt.clone()))
    }

    /// A wake check answered; returns whether the poll goes on.
    fn wake_checked(
        &mut self,
        generation: u64,
        api: SealSkinClient,
        result: Result<(), ApiError>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.wake.finish_check(generation) || self.recovery.is_none() {
            return false;
        }
        match result {
            Ok(()) => {
                self.stop_wake_poll();
                if self.busy.is_none() {
                    let save = kept_save(self.pending_save.as_ref(), &api);
                    self.load(api, save, LoadKind::Connect, cx);
                } else {
                    // A load is running: if it answers Dormant (sent before the wake),
                    // `on_refusal` reloads once instead of showing the panel again.
                    self.wake.confirmed = true;
                }
                false
            }
            Err(ApiError::Key(KeyRefusal::Dormant)) => true,
            Err(error) if error.is_refusal() => {
                self.on_refusal(api, error, cx);
                false
            }
            // A network blip: keep checking.
            Err(_) => true,
        }
    }

    /// The Library window came forward: while a wake is pending, check once now,
    /// unless a check or load is already running.
    pub(super) fn wake_check_on_activation(&mut self, cx: &mut Context<Self>) {
        let pending = self.recovery.as_ref().is_some_and(|recovery| {
            recovery.kind == RecoveryKind::Dormant && recovery.waking_since.is_some()
        });
        if !pending || !self.wake.begin_check(self.busy.is_some()) {
            return;
        }
        let Some(api) = self.recovery.as_ref().map(|recovery| recovery.api.clone()) else {
            return;
        };
        let generation = self.wake.generation;
        let window = self.window;
        let client = api.clone();
        let pending = self.rt.spawn(async move { client.key_works().await });
        cx.spawn(async move |this, cx| {
            let result = pending
                .await
                .unwrap_or_else(|_| Err(ApiError::Message("interrupted".into())));
            let _ = window.update(cx, |_, _, cx| {
                this.update(cx, |this, cx| {
                    let continues = this.wake_checked(generation, api, result, cx);
                    let _ = continues;
                })
            });
        })
        .detach();
    }

    /// Try Again on a recovery panel: one load with the kept key.
    pub(super) fn try_again(&mut self, cx: &mut Context<Self>) {
        if let Some(api) = self.recovery.as_ref().map(|recovery| recovery.api.clone()) {
            let save = kept_save(self.pending_save.as_ref(), &api);
            self.load(api, save, LoadKind::Connect, cx);
        }
    }

    /// Use a Different Key: the normal Connect card, asking for a new key. The kept key
    /// stays the retry target, so an empty Connect tries it again.
    pub(super) fn use_different_key(&mut self, cx: &mut Context<Self>) {
        if let Some(recovery) = self.recovery.take() {
            self.retry_api = Some(recovery.api);
        }
        self.stop_wake_poll();
        self.key_problem = None;
        self.key_hint = None;
        self.status = None;
        self.different_key = true;
        self.focus_key_input = true;
        cx.notify();
    }

    /// Clears every recovery state: a new connect is starting or one succeeded.
    pub(super) fn clear_recovery(&mut self) {
        self.recovery = None;
        self.key_problem = None;
        self.refused_server = None;
        self.key_hint = None;
        self.different_key = false;
        self.stop_wake_poll();
    }

    /// The account menu opened or closed. Opening measures storage once per Library load
    /// (again after a failed measure), never for an administrator (as the web profile).
    pub(super) fn set_account_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.account_open = open;
        if open {
            self.measure_storage(cx);
        }
        cx.notify();
    }

    fn measure_storage(&mut self, cx: &mut Context<Self>) {
        let wanted = self.library.as_ref().is_some_and(|library| {
            library.status.settings.persistent_storage
                && !library.status.is_admin
                && !library.status.username.is_empty()
        });
        if !wanted || !self.account_storage.wants_measure() {
            return;
        }
        let Some(api) = self.api.clone() else {
            return;
        };
        self.account_storage = Measure::Measuring;
        let gate = super::LoadGate(self.server_generation);
        let window = self.window;
        let client = api.clone();
        let pending = self.rt.spawn(async move { client.account(true).await });
        cx.spawn(async move |this, cx| {
            let result = pending
                .await
                .unwrap_or_else(|_| Err(ApiError::Message("interrupted".into())));
            let _ = window.update(cx, |_, _, cx| {
                this.update(cx, |this, cx| {
                    if !gate.admits(this.server_generation) {
                        return;
                    }
                    match result {
                        Ok(fresh) => {
                            this.account_storage = Measure::Done(fresh.storage_used);
                            if let Some(library) = this.library.as_mut() {
                                library.status.storage_used = fresh.storage_used;
                                library.status.allowance = fresh.allowance;
                            }
                        }
                        Err(error) if error.is_refusal() => this.on_refusal(api, error, cx),
                        Err(_) => this.account_storage = Measure::Failed,
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    /// The account menu, from the loaded Library.
    pub(super) fn account_menu(&self) -> AccountProps {
        let (Some(api), Some(library)) = (&self.api, &self.library) else {
            return AccountProps::default();
        };
        account_props(
            &library.status,
            library.sessions.len(),
            self.account_storage,
            api.server(),
            Some(self.sign_out_label()),
            api.key_id(),
        )
    }

    /// The Connect screen: the key card (with a refusal's alert) or a recovery panel.
    pub(super) fn render_connect(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let server = self
            .recovery
            .as_ref()
            .map(|recovery| recovery.api.server().to_string())
            .or_else(|| self.key_problem.as_ref().and(self.refused_server.clone()));
        let props = ConnectProps {
            status: self.status.clone(),
            busy: self.busy.clone(),
            server,
            problem: self.key_problem.clone(),
            recovery: self.recovery.as_ref().map(Recovery::props),
            different_key: self.different_key,
            hint: self.key_hint.clone(),
            pending_open: super::pending_open_banner(&self.pending_opens),
        };
        let on_selkies_tab = self.connect_tab == skerry_ui::selkies::ConnectTab::Selkies;
        let mut handlers =
            ConnectHandlers::new(cx.listener(|this, _, window, cx| this.submit_key(window, cx)));
        if self.recovery.is_some() && !on_selkies_tab {
            handlers.on_sign_in_web = Some(Rc::new(
                cx.listener(|this, _, _, cx| this.sign_in_on_web(cx)),
            ));
            handlers.on_try_again = Some(Rc::new(cx.listener(|this, _, _, cx| this.try_again(cx))));
            handlers.on_other_key = Some(Rc::new(
                cx.listener(|this, _, _, cx| this.use_different_key(cx)),
            ));
            handlers.on_open_web = Some(Rc::new(cx.listener(|this, _, _, cx| this.open_web(cx))));
            // No key field on a panel: Return goes to the panel's primary button.
            if !self.focus_handle.contains_focused(window, cx) {
                window.focus(&self.focus_handle, cx);
            }
        }
        let (selkies, selkies_handlers) = self.connect_tabs(cx);
        let tabs = connect::ConnectTabs {
            tab: self.connect_tab,
            selkies: &selkies,
            inputs: &self.selkies_form.inputs,
            handlers: selkies_handlers,
        };
        let on_selkies = self.connect_tab == skerry_ui::selkies::ConnectTab::Selkies;
        div()
            .size_full()
            .relative()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if this.certificate_key(event, cx) {
                    cx.stop_propagation();
                    return;
                }
                if on_selkies || event.keystroke.key != "enter" || this.busy.is_some() {
                    return;
                }
                let Some(recovery) = this.recovery.as_ref() else {
                    return;
                };
                match (recovery.kind, recovery.timed_out) {
                    (RecoveryKind::Dormant, false) => this.sign_in_on_web(cx),
                    (RecoveryKind::Dormant, true) => this.try_again(cx),
                    (RecoveryKind::Held, _) => this.open_web(cx),
                }
                cx.stop_propagation();
            }))
            .child(connect::render_with_tabs(
                theme,
                &props,
                &self.key_input,
                handlers,
                tabs,
            ))
            .when_some(self.toast.as_ref(), |view, toast| {
                view.child(skerry_ui::components::toast_layer(theme, toast))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sealskin::{Allowance, UserSettings};

    #[test]
    fn recovery_plan_matches_each_refusal() {
        let saved = KeySourceLabel::SavedFile;
        let dormant = recovery_for(&ApiError::Key(KeyRefusal::Dormant), saved).unwrap();
        assert_eq!(dormant.panel, Some(RecoveryKind::Dormant));
        assert!(!dormant.focus_key && dormant.retry_saved && dormant.problem.is_none());
        let held = recovery_for(&ApiError::Account(AccountBlock::Held), saved).unwrap();
        assert_eq!(held.panel, Some(RecoveryKind::Held));
        for dead in [KeyRefusal::Revoked, KeyRefusal::Expired] {
            let plan = recovery_for(&ApiError::Key(dead), saved).unwrap();
            assert!(plan.focus_key && !plan.retry_saved && plan.panel.is_none());
        }
        for policy in [
            ApiError::Key(KeyRefusal::KeysDisabled),
            ApiError::Key(KeyRefusal::ClientNotAllowed),
            ApiError::Key(KeyRefusal::Other("Invalid token.".into())),
            ApiError::Account(AccountBlock::Inactive),
            ApiError::Account(AccountBlock::Other("No.".into())),
        ] {
            let plan = recovery_for(&policy, saved).unwrap();
            assert!(plan.focus_key && plan.retry_saved, "{policy:?}");
        }
        assert!(recovery_for(&ApiError::Message("x".into()), saved).is_none());
        assert!(recovery_for(&ApiError::Forbidden("x".into()), saved).is_none());
    }

    #[test]
    fn the_env_note_follows_the_key_source() {
        let note = |source| {
            recovery_for(&ApiError::Key(KeyRefusal::Revoked), source)
                .unwrap()
                .problem
                .unwrap()
                .env_note_text()
                .is_some()
        };
        assert!(note(KeySourceLabel::Environment));
        assert!(note(KeySourceLabel::DotEnv));
        assert!(!note(KeySourceLabel::SavedFile));
        assert!(!note(KeySourceLabel::ThisRunOnly));
        // Not on a policy refusal: the same key works once the administrator acts.
        let disabled = recovery_for(
            &ApiError::Key(KeyRefusal::KeysDisabled),
            KeySourceLabel::Environment,
        )
        .unwrap();
        assert!(disabled.problem.unwrap().env_note_text().is_none());
        assert_eq!(
            source_label(KeySource::Typed, || true),
            KeySourceLabel::SavedFile
        );
        assert_eq!(
            source_label(KeySource::Typed, || false),
            KeySourceLabel::ThisRunOnly
        );
        assert_eq!(
            source_label(KeySource::ConfigFile, || false),
            KeySourceLabel::SavedFile
        );
        assert_eq!(
            source_label(KeySource::Environment, || true),
            KeySourceLabel::Environment
        );
        assert_eq!(
            source_label(KeySource::DotEnv, || true),
            KeySourceLabel::DotEnv
        );
    }

    #[test]
    fn an_empty_connect_never_resends_a_dead_key() {
        let revoked = recovery_for(
            &ApiError::Key(KeyRefusal::Revoked),
            KeySourceLabel::SavedFile,
        )
        .unwrap()
        .problem;
        assert_eq!(
            submit_decision(true, revoked.as_ref(), true, true),
            Submit::Hint(skerry_ui::connect::PASTE_NEW_KEY)
        );
        assert_eq!(
            submit_decision(false, revoked.as_ref(), false, false),
            Submit::Parse
        );
        let disabled = recovery_for(
            &ApiError::Key(KeyRefusal::KeysDisabled),
            KeySourceLabel::SavedFile,
        )
        .unwrap()
        .problem;
        assert_eq!(
            submit_decision(true, disabled.as_ref(), true, false),
            Submit::RetryKept
        );
        // Use a Different Key after a typed key: the kept client is the retry target.
        assert_eq!(submit_decision(true, None, true, false), Submit::RetryKept);
        assert_eq!(submit_decision(true, None, false, true), Submit::RetrySaved);
        assert!(matches!(
            submit_decision(true, None, false, false),
            Submit::Hint(_)
        ));
    }

    #[test]
    fn a_check_dropped_with_its_poll_never_blocks_the_next_poll() {
        let mut wake = WakeState::default();
        let first = wake.restart();
        assert!(wake.begin_check(false));
        // A second check (activation, the next tick) waits for the one out.
        assert!(!wake.begin_check(false));
        // The poll is dropped while its check is out (Use a Different Key, a new wake).
        wake.restart();
        let second = wake.restart();
        assert!(wake.begin_check(false), "the new poll's first tick checks");
        // The old check's answer, if it ever comes, is stale and changes nothing.
        assert!(!wake.finish_check(first));
        assert!(!wake.begin_check(false));
        assert!(wake.finish_check(second));
        assert!(!wake.begin_check(true), "never while a load runs");
        assert!(wake.begin_check(false));
        wake.confirmed = true;
        wake.restart();
        assert!(!wake.confirmed);
    }

    #[test]
    fn a_pasted_key_that_connects_later_is_saved() {
        let typed = Credential::parse("ssk_new_secret@seal.example").unwrap();
        let api = SealSkinClient::new(&typed).unwrap();
        // Refused (dormant, held, disabled) and kept: the wake poll, Try Again or an empty
        // Connect reloads with the credential, so the load saves it.
        let save = kept_save(Some(&typed), &api).expect("saved once it connects");
        assert_eq!(save.raw(), typed.raw());
        // Never another key's credential.
        let other = Credential::parse("ssk_old_secret@seal.example").unwrap();
        let other_api = SealSkinClient::new(&other).unwrap();
        assert!(kept_save(Some(&typed), &other_api).is_none());
        assert!(kept_save(None, &api).is_none());
    }

    #[test]
    fn only_a_failed_storage_measure_is_retried() {
        assert!(Measure::NotAsked.wants_measure());
        assert!(Measure::Failed.wants_measure());
        assert!(!Measure::Measuring.wants_measure());
        assert!(!Measure::Done(None).wants_measure());
        assert!(!Measure::Done(Some(1)).wants_measure());
    }

    #[test]
    fn wake_polling_stops_after_three_minutes_or_a_new_wake() {
        let started = Instant::now();
        assert!(poll_should_continue(
            started,
            started + Duration::from_secs(175),
            2,
            2
        ));
        assert!(!poll_should_continue(
            started,
            started + WAKE_POLL_LIMIT,
            2,
            2
        ));
        assert!(!poll_should_continue(started, started, 2, 3));
        assert_eq!(WAKE_POLL_LIMIT.as_secs() / WAKE_POLL_INTERVAL.as_secs(), 36);
    }

    /// The poll's loop against the fake server, at a 20 ms tick: it checks, and once its
    /// task is cancelled (as dropping `wake_poll` does), nothing more reaches the server.
    #[tokio::test]
    async fn a_cancelled_wake_poll_sends_nothing_more() {
        use crate::sealskin::fake;
        let (base, seen) = fake::serve(|path| {
            if path.starts_with("/companion/api/admin/status") {
                fake::reply("401 Unauthorized", r#"{"detail":"dormant"}"#)
            } else {
                fake::reply("404 Not Found", "{}")
            }
        })
        .await;
        let api = fake::client(base.clone());
        let tick = Duration::from_millis(20);
        let checks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = checks.clone();
        let poll = tokio::spawn(async move {
            poll_ticks(
                tick,
                tokio::time::sleep,
                || Tick::Check(api.clone()),
                |api: SealSkinClient| async move { api.key_works().await },
                |result: Result<(), ApiError>| {
                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    // Still asleep: keep polling.
                    result == Err(ApiError::Key(KeyRefusal::Dormant))
                },
            )
            .await
        });
        while checks.load(std::sync::atomic::Ordering::SeqCst) < 2 {
            tokio::time::sleep(tick).await;
        }
        poll.abort();
        let _ = poll.await;
        let after_cancel = seen.lock().unwrap().len();
        tokio::time::sleep(tick * 3).await;
        assert_eq!(seen.lock().unwrap().len(), after_cancel);
        for request in seen.lock().unwrap().iter() {
            assert!(
                request.starts_with("post /companion/api/admin/status "),
                "{request}"
            );
            assert!(request.contains(&format!("host: {}", base.authority())));
        }
    }

    #[tokio::test]
    async fn the_wake_poll_stops_when_apply_says_so_and_skips_without_checking() {
        let checks = std::cell::Cell::new(0);
        let mut steps = vec![Tick::Skip, Tick::Check(()), Tick::Check(())].into_iter();
        poll_ticks(
            Duration::from_millis(1),
            tokio::time::sleep,
            || steps.next().unwrap_or(Tick::Stop),
            |()| async {},
            |()| {
                checks.set(checks.get() + 1);
                checks.get() < 2
            },
        )
        .await;
        assert_eq!(checks.get(), 2);
    }

    fn status(admin: bool, username: &str) -> ServerStatus {
        ServerStatus {
            settings: UserSettings {
                persistent_storage: true,
                gpu: true,
                groups: vec!["users".into()],
                storage_limit: 50,
                session_limit: 2,
                application_key_dormant_hours: 24.,
                ..UserSettings::default()
            },
            is_admin: admin,
            username: username.into(),
            allowance: Some(Allowance {
                hours: 40.,
                period: "month".into(),
                used: 12.5,
            }),
            ..ServerStatus::default()
        }
    }

    #[test]
    fn account_props_show_limits_and_the_admin_sleep_rule() {
        let user = account_props(
            &status(false, "kebot"),
            1,
            Measure::Measuring,
            "seal.example",
            Some(KeySourceLabel::SavedFile),
            "abc",
        );
        assert_eq!(user.limits.len(), 7);
        assert_eq!(user.limits[5].value, account::LimitValue::Measuring);
        assert!(user.access[0].allowed && !user.access[2].allowed);
        assert!(user.dormancy.is_some());
        // Other administrators' keys sleep too; only the built-in `admin` never does.
        let other_admin = account_props(&status(true, "ops"), 0, Measure::NotAsked, "s", None, "");
        assert!(other_admin.limits.is_empty());
        assert!(other_admin.dormancy.is_some());
        let builtin = account_props(&status(true, "admin"), 0, Measure::NotAsked, "s", None, "");
        assert!(builtin.dormancy.is_none());
        // An unreadable status: the menu says the account is unavailable.
        let unknown = account_props(
            &ServerStatus::default(),
            0,
            Measure::NotAsked,
            "s",
            None,
            "",
        );
        assert!(!unknown.available());
    }
}
