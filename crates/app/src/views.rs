//! Root view: top bar, Sidebar (Tabs, Groups, Connection chips), Mirror.
//!
//! One window shows every Connection of `config.toml`. Each opened
//! Connection is a [`Conn`] with its own `rb_cdp::Connection`, pages, dialogs
//! and Mirror; only the Active Tab (one, app-wide) streams.
//!
//! All CDP and Manager I/O runs on the app's tokio runtime. It reports back
//! to this view through one unbounded channel of [`Msg`]s that a GPUI task
//! drains (the bridge described in docs/gpui-notes.md). Every message names
//! its Connection and a generation, so late messages of a Connection that was
//! disconnected (or started again) are dropped.
//!
//! Tab Groups, pins and Placeholder Tabs live in an `rb_store::State`, loaded
//! from `state.toml` at startup, reconciled with each Connection's live pages
//! independently and saved (debounced) after mutations. The Sidebar renders
//! from `State::sidebar_layout` (see `views/sidebar.rs`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, prelude::*, px, AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Point, Render,
    SharedString, Styled, Window,
};
use rb_cdp::agent_proxy::{random_path_secret, AgentProxy, AgentProxyConfig, AgentState};
use rb_cdp::{
    ActiveTab, Connection, ConnectionEvent, ConnectionState, Dialog, ScreencastOptions, TargetInfo,
};
use rb_store::{ConnectionConfig, SidebarItem, State};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::actions::{self, PageCmd};
use crate::agent_model::{self, AgentLine, Marker};
use crate::connect::{self, ManagerTarget, Prepared};
use crate::dialog_model;
use crate::mirror::{Mirror, MirrorBackend};
use crate::model::{self, Overlay, OverlayAction};
use crate::multi::{self, ProbeEffect};
use crate::sidebar_model::{
    self as sm, ChipAction, ChipStatus, ConnStyle, DropTarget, LineEdit, LinkState, MenuAction,
    MenuKind,
};
use crate::startup::Startup;
use crate::state::{
    CloseTab, FocusAddress, GoBack, GoForward, NewTab, Select1, Select2, Select3, Select4, Select5,
    Select6, Select7, Select8, Select9, ToggleSidebar,
};
use crate::theme::{self, hsla, Palette};

mod dialog;
mod sidebar;

/// Debounce before `state.toml` is written after a change.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Messages from the tokio side to the view. `usize` is the Connection's
/// index, `u64` its generation (see [`Conn::gen`]).
enum Msg {
    Prepared(usize, u64, Prepared),
    /// Profile status check of a Connection that is not running.
    Probe(usize, u64, Prepared),
    Launched(usize, Result<(), String>),
    State(usize, u64, ConnectionState),
    Pages(usize, u64, Vec<TargetInfo>),
    Dialogs(usize, u64, Vec<Dialog>),
    Event(usize, u64, ConnectionEvent),
    /// Answering the dialog of a Tab finished (`Err`: it did not go through).
    DialogAnswered(usize, u64, Result<(), String>),
    Notice(usize, String),
    /// The Agent proxy of a Connection started (or could not), by agent generation.
    AgentStarted(usize, u64, Result<AgentProxy, String>),
    AgentState(usize, u64, AgentState),
}

/// A running Agent proxy and its latest snapshot.
struct AgentRun {
    /// Dropping it stops the listener and all Agent clients.
    _proxy: AgentProxy,
    endpoint: String,
    state: AgentState,
}

enum Phase {
    /// Checking the Profile / connecting for the first time.
    Connecting,
    /// The Connection config is unusable: its Overlay says why.
    Problem(Overlay),
    /// The Connection cannot connect (Profile stopped, Manager error): the
    /// Sidebar stays, the Mirror area shows this when it is the focus.
    Blocked(Overlay),
    /// Not connected (lazy, or the user disconnected).
    Idle,
    /// A Connection exists; its state decides about an overlay.
    Running,
}

/// One configured Connection and everything it runs.
struct Conn {
    name: String,
    cfg: ConnectionConfig,
    phase: Phase,
    /// Set for Manager Connections (status, launch).
    manager: Option<ManagerTarget>,
    /// Config for the CDP Connection, until it is started.
    pending_cdp: Option<rb_cdp::ConnectionConfig>,
    conn: Option<Arc<Connection>>,
    mirror: Option<Entity<Mirror>>,
    conn_state: ConnectionState,
    pages: Vec<TargetInfo>,
    /// JS dialogs blocking pages of this Connection.
    dialogs: Vec<Dialog>,
    /// The Manager Profile is being started.
    starting: bool,
    /// Last launch or command error, shown in the status bar and overlay.
    notice: Option<String>,
    /// Bumped whenever the Connection is (re)started or dropped.
    gen: u64,
    // ---- Agent proxy (off unless enabled in config or from the popover) ----
    /// Upstream URL and token for the proxy (same as the Connection's own).
    upstream: Option<(String, Option<rb_cdp::Token>)>,
    agent_on: bool,
    /// Port to bind; the config value, then the one actually bound, so the
    /// endpoint stays the same across reconnects and Disable/Enable.
    agent_port: u16,
    /// Path secret: from config, else random per run (never written out).
    agent_secret: Option<String>,
    agent: Option<AgentRun>,
    agent_starting: bool,
    /// Bumped whenever the proxy is started or stopped (drops late messages).
    agent_gen: u64,
    agent_error: Option<String>,
    /// "Copy endpoint" was pressed (the button says "Copied").
    agent_copied: bool,
    /// The Connection was connected at least once since it was started.
    was_connected: bool,
}

impl Conn {
    fn new(cfg: ConnectionConfig) -> Conn {
        let mut c = Conn {
            name: cfg.name.clone(),
            cfg,
            phase: Phase::Idle,
            manager: None,
            pending_cdp: None,
            conn: None,
            mirror: None,
            conn_state: ConnectionState::Closed,
            pages: Vec::new(),
            dialogs: Vec::new(),
            starting: false,
            notice: None,
            gen: 0,
            upstream: None,
            agent_on: false,
            agent_port: 0,
            agent_secret: None,
            agent: None,
            agent_starting: false,
            agent_gen: 0,
            agent_error: None,
            agent_copied: false,
            was_connected: false,
        };
        c.agent_on = c.cfg.agent_proxy;
        c.agent_port = c.cfg.agent_proxy_port;
        c.agent_secret = c
            .cfg
            .agent_proxy_secret
            .as_ref()
            .map(|s| s.expose().to_string());
        c.arm();
        c
    }

    /// Build the CDP config (and Manager client) from the config entry.
    fn arm(&mut self) {
        match connect::resolve(&self.cfg) {
            Ok(r) => {
                self.manager = r.manager;
                self.upstream = Some((r.cdp.url.clone(), r.cdp.token.clone()));
                self.pending_cdp = Some(r.cdp);
            }
            Err(e) => {
                self.phase =
                    Phase::Problem(Overlay::new("Invalid Connection in config.toml").detail(e));
            }
        }
    }

    /// Reconnecting or first connect in progress.
    fn connecting(&self) -> bool {
        matches!(self.phase, Phase::Connecting)
            || (matches!(self.phase, Phase::Running)
                && self.conn_state == ConnectionState::Connecting)
    }

    /// Idle or showing a Blocked overlay: a Profile probe may change it.
    fn probeable(&self) -> bool {
        self.manager.is_some() && matches!(self.phase, Phase::Idle | Phase::Blocked(_))
    }
}

/// The address field: a minimal single-line editor (append, backspace, paste).
struct Address {
    focus: FocusHandle,
    buf: String,
    /// The whole text is selected: the next character replaces it.
    select_all: bool,
}

/// An open context menu.
struct Menu {
    kind: MenuKind,
    pos: Point<gpui::Pixels>,
}

/// A Tab Group name being edited in its header.
struct Rename {
    group: String,
    edit: LineEdit,
    focus: FocusHandle,
    /// The field has had focus; losing it commits.
    seen_focus: bool,
}

/// The modal of the Active Tab's pending dialog: prompt text, focus.
struct DialogInput {
    /// Connection and Tab the modal belongs to.
    key: (usize, String),
    edit: LineEdit,
    focus: FocusHandle,
    /// An answer was sent; the modal stays until the dialog leaves
    /// `pending_dialogs`.
    busy: bool,
}

pub struct Root {
    focus: FocusHandle,
    rt: Handle,
    ui_tx: UnboundedSender<Msg>,
    /// The Sidebar is the narrow icon rail.
    rail: bool,
    /// Styles of all Connections, in config order (parallel to `conns`).
    conn_styles: Vec<ConnStyle>,
    conns: Vec<Conn>,
    warnings: Vec<String>,
    /// Nothing can run (first run, config problem): the whole window shows this.
    problem: Option<Overlay>,
    /// The Connection named on the command line (opens first).
    preferred: Option<usize>,
    /// The Connection last connected, clicked or started: its state fills the
    /// Mirror area while no Tab is Active.
    last_conn: Option<usize>,
    /// The one Active Tab: Connection index and target id.
    active: Option<(usize, String)>,
    /// The Connection whose screencast runs (at most one).
    streaming: Option<usize>,
    /// Last command error not tied to a Connection.
    notice: Option<String>,
    address: Address,
    /// Focus the Mirror on the next render (needs a Window).
    focus_mirror: bool,
    // ---- persisted local state ----
    state: State,
    state_path: Option<PathBuf>,
    /// False when `state.toml` exists but could not be read: never overwrite it.
    state_writable: bool,
    save_gen: u64,
    dirty: bool,
    // ---- Sidebar interaction ----
    menu: Option<Menu>,
    rename: Option<Rename>,
    /// Chip whose popover is open.
    chip_open: Option<usize>,
    /// The chip overflow menu is open.
    overflow_open: bool,
    chip_over: bool,
    pop_over: bool,
    hover_gen: u64,
    drop_hint: Option<DropTarget>,
    /// A Tab (by id) to activate as soon as its Connection is up.
    pending_select: Option<String>,
    /// The next new page of this Connection becomes the Active Tab (Cmd+T).
    expect_new_tab: Option<usize>,
    /// Open a blank Tab in this Connection as soon as it is connected.
    new_tab_after_connect: Option<usize>,
    // ---- JS dialog modal ----
    dialog_input: Option<DialogInput>,
}

impl Root {
    pub fn new(startup: Startup, rt: Handle, cx: &mut Context<Self>) -> Self {
        let (ui_tx, mut ui_rx) = unbounded_channel::<Msg>();
        cx.spawn(async move |this, cx| {
            while let Some(msg) = ui_rx.recv().await {
                if this.update(cx, |r, cx| r.apply(msg, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // Flush pending changes when the app quits.
        cx.on_app_quit(|this, _| {
            this.save_now();
            for c in &mut this.conns {
                c.agent = None; // stop listening
            }
            async {}
        })
        .detach();

        let mut root = Root {
            focus: cx.focus_handle(),
            rt,
            ui_tx,
            rail: false,
            conn_styles: Vec::new(),
            conns: Vec::new(),
            warnings: Vec::new(),
            problem: None,
            preferred: None,
            last_conn: None,
            active: None,
            streaming: None,
            notice: None,
            address: Address {
                focus: cx.focus_handle(),
                buf: String::new(),
                select_all: false,
            },
            focus_mirror: false,
            state: State::default(),
            state_path: None,
            state_writable: true,
            save_gen: 0,
            dirty: false,
            menu: None,
            rename: None,
            chip_open: None,
            overflow_open: false,
            chip_over: false,
            pop_over: false,
            hover_gen: 0,
            drop_hint: None,
            pending_select: None,
            expect_new_tab: None,
            new_tab_after_connect: None,
            dialog_input: None,
        };
        match startup {
            Startup::Problem(o) => root.problem = Some(o),
            Startup::Ready {
                connections,
                selected,
                warnings,
            } => {
                root.conn_styles = connections
                    .iter()
                    .enumerate()
                    .map(|(i, c)| ConnStyle::new(&c.name, c.color.as_deref(), i))
                    .collect();
                root.warnings = warnings;
                root.warnings
                    .extend(multi::style_warnings(&root.conn_styles));
                root.load_state();
                let auto: Vec<bool> = connections.iter().map(|c| c.auto_connect).collect();
                root.conns = connections.into_iter().map(Conn::new).collect();
                root.preferred = selected;
                let start = multi::startup_connects(&auto, selected);
                root.last_conn = selected.or(start.first().copied());
                for i in 0..root.conns.len() {
                    if start.contains(&i) {
                        root.begin_prepare(i, cx);
                    } else {
                        root.probe(i);
                    }
                }
            }
        }
        root
    }

    /// Load `state.toml` (`RB_STATE` or the default path).
    fn load_state(&mut self) {
        self.state_path = rb_store::default_state_path();
        let Some(path) = self.state_path.clone() else {
            return;
        };
        match State::load(&path) {
            Ok(s) => self.state = s,
            Err(e) => {
                self.state_writable = false;
                self.warnings
                    .push(format!("{e}; Tab Groups are not saved this run"));
            }
        }
    }

    // ---- persistence --------------------------------------------------------

    /// Schedule a debounced save.
    fn mark_dirty(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.save_gen += 1;
        let gen = self.save_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let _ = this.update(cx, |r, _| {
                if r.save_gen == gen {
                    r.save_now();
                }
            });
        })
        .detach();
    }

    /// Write `state.toml` now if there are unsaved changes.
    fn save_now(&mut self) {
        if !self.dirty || !self.state_writable {
            return;
        }
        self.dirty = false;
        if let Some(p) = &self.state_path {
            if let Err(e) = self.state.save(p) {
                self.notice = Some(format!("Could not save state: {e}"));
            }
        }
    }

    /// Change the state; marks it dirty only if something changed.
    fn edit_state(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut State)) {
        let before = self.state.clone();
        f(&mut self.state);
        if self.state != before {
            self.mark_dirty(cx);
        }
        cx.notify();
    }

    // ---- derived data -------------------------------------------------------

    fn dark(&self, window: &Window) -> bool {
        theme::is_dark(window.appearance())
    }

    fn idx_of(&self, name: &str) -> Option<usize> {
        self.conns.iter().position(|c| c.name == name)
    }

    /// A Connection's link state, for chips and Placeholder Tabs.
    fn link_of(&self, i: usize) -> LinkState {
        let c = &self.conns[i];
        match &c.phase {
            Phase::Running => LinkState::Live(c.conn_state.clone()),
            Phase::Connecting => LinkState::Connecting,
            Phase::Blocked(o) if o.action == Some(OverlayAction::StartProfile) => {
                LinkState::ProfileStopped
            }
            Phase::Blocked(_) | Phase::Problem(_) => LinkState::Failed,
            Phase::Idle => LinkState::Off,
        }
    }

    fn chip_status_of(&self, i: usize) -> ChipStatus {
        sm::chip_status(&self.link_of(i), self.conns[i].manager.is_some())
    }

    /// Why a Connection is failing, for its chip popover.
    fn detail_of(&self, i: usize) -> Option<String> {
        let c = &self.conns[i];
        let o = match &c.phase {
            Phase::Blocked(o) | Phase::Problem(o)
                if o.action != Some(OverlayAction::StartProfile) =>
            {
                o.clone()
            }
            Phase::Running => model::overlay_for(&c.conn_state, c.manager.is_some())
                .filter(|_| matches!(c.conn_state, ConnectionState::Failed { .. }))?,
            _ => return None,
        };
        Some(match &o.detail {
            Some(d) => format!("{}: {d}", o.title),
            None => o.title.clone(),
        })
    }

    fn tabs_live_of(&self, i: usize) -> bool {
        sm::tabs_live(&self.link_of(i))
    }

    fn layout(&self) -> Vec<SidebarItem> {
        self.state
            .sidebar_layout(&|c: &str| self.idx_of(c).is_some_and(|i| self.tabs_live_of(i)))
    }

    fn is_live_target(&self, conn: &str, target_id: Option<&str>) -> bool {
        let Some(i) = self.idx_of(conn) else {
            return false;
        };
        self.tabs_live_of(i)
            && target_id.is_some_and(|t| self.conns[i].pages.iter().any(|p| p.target_id == t))
    }

    fn is_active(&self, conn: &str, target_id: Option<&str>) -> bool {
        match (&self.active, target_id, self.idx_of(conn)) {
            (Some((c, t)), Some(id), Some(i)) => *c == i && t == id,
            _ => false,
        }
    }

    /// The Connection a new Tab opens in: the Active Tab's, else the last
    /// used one, else the first.
    fn target_conn_idx(&self) -> Option<usize> {
        self.active
            .as_ref()
            .map(|a| a.0)
            .or(self.last_conn)
            .or((!self.conns.is_empty()).then_some(0))
    }

    fn target_connection(&self) -> Option<&ConnStyle> {
        self.target_conn_idx().and_then(|i| self.conn_styles.get(i))
    }

    /// The Connection whose state fills the Mirror area: the Active Tab's,
    /// else the last used one.
    fn focus_conn(&self) -> Option<usize> {
        self.active.as_ref().map(|a| a.0).or(self.last_conn)
    }

    /// A pending JS dialog of the Active Tab (with its Connection index), only
    /// while that Connection is connected.
    fn active_dialog(&self) -> Option<(usize, Dialog)> {
        let (ci, t) = self.active.as_ref()?;
        let c = &self.conns[*ci];
        if c.conn_state != ConnectionState::Connected {
            return None;
        }
        dialog_model::for_target(&c.dialogs, t).map(|d| (*ci, d.clone()))
    }

    /// A background Tab (not the Active Tab) with a pending dialog.
    fn has_dialog_badge(&self, conn: &str, target_id: Option<&str>) -> bool {
        let (Some(i), Some(t)) = (self.idx_of(conn), target_id) else {
            return false;
        };
        !self.is_active(conn, Some(t))
            && self.conns[i].conn_state == ConnectionState::Connected
            && dialog_model::for_target(&self.conns[i].dialogs, t).is_some()
    }

    // ---- connecting -------------------------------------------------------

    /// Check the Profile status in the background (Manager Connections that
    /// are not running), so a stopped Profile shows its Start button.
    fn probe(&self, i: usize) {
        let c = &self.conns[i];
        if !c.probeable() {
            return;
        }
        let manager = c.manager.clone();
        let (tx, gen) = (self.ui_tx.clone(), c.gen);
        self.rt.spawn(async move {
            let _ = tx.send(Msg::Probe(i, gen, connect::prepare(manager.as_ref()).await));
        });
    }

    /// Manager pre-flight, then start the Connection.
    fn begin_prepare(&mut self, i: usize, cx: &mut Context<Self>) {
        let c = &mut self.conns[i];
        if matches!(c.phase, Phase::Problem(_)) {
            return;
        }
        if c.pending_cdp.is_none() {
            c.arm();
            if matches!(c.phase, Phase::Problem(_)) {
                return;
            }
        }
        c.phase = Phase::Connecting;
        c.gen += 1;
        if c.manager.is_none() {
            self.start_connection(i, cx);
            return;
        }
        let manager = c.manager.clone();
        let (tx, gen) = (self.ui_tx.clone(), c.gen);
        self.rt.spawn(async move {
            let _ = tx.send(Msg::Prepared(
                i,
                gen,
                connect::prepare(manager.as_ref()).await,
            ));
        });
        cx.notify();
    }

    /// Connect (or retry) a Connection. Never starts a stopped Profile.
    fn connect_conn(&mut self, i: usize, cx: &mut Context<Self>) {
        self.last_conn = Some(i);
        let c = &self.conns[i];
        match (&c.conn, &c.phase) {
            (Some(conn), _) => conn.retry(),
            (None, Phase::Connecting) => {}
            (None, _) => self.begin_prepare(i, cx),
        }
        cx.notify();
    }

    fn disconnect(&mut self, i: usize, cx: &mut Context<Self>) {
        let c = &mut self.conns[i];
        if let Some(conn) = c.conn.take() {
            self.rt.spawn(async move { conn.close().await });
        }
        c.mirror = None;
        c.pending_cdp = None;
        c.was_connected = false;
        c.pages.clear();
        c.dialogs.clear();
        c.gen += 1;
        c.conn_state = ConnectionState::Closed;
        c.phase = Phase::Idle;
        c.notice = None;
        c.arm();
        self.stop_agent(i);
        if self.streaming == Some(i) {
            self.streaming = None;
        }
        if self.new_tab_after_connect == Some(i) {
            self.new_tab_after_connect = None;
        }
        if self.active.as_ref().is_some_and(|a| a.0 == i) {
            self.active = self.any_live_tab();
            self.sync_active();
            self.focus_mirror = true;
        }
        self.probe(i);
        cx.notify();
    }

    /// The first live page of any Connection.
    fn any_live_tab(&self) -> Option<(usize, String)> {
        self.conns.iter().enumerate().find_map(|(i, c)| {
            (self.tabs_live_of(i))
                .then(|| c.pages.first().map(|p| (i, p.target_id.clone())))
                .flatten()
        })
    }

    fn start_connection(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(cfg) = self.conns[i].pending_cdp.take() else {
            return;
        };
        let conn = {
            let _enter = self.rt.enter(); // Connection::start spawns on tokio
            Arc::new(Connection::start(cfg))
        };
        let backend =
            MirrorBackend::from_connection(&conn, self.rt.clone()).expect("fresh Connection");
        let mirror = cx.new(|cx| Mirror::new(backend, cx));

        // One pump forwards state, Tab list, dialogs and events to the UI.
        let gen = self.conns[i].gen;
        let mut state = conn.state();
        let mut pages = conn.pages();
        let mut dialogs = conn.pending_dialogs();
        let mut events = conn.events();
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            let _ = tx.send(Msg::State(i, gen, state.borrow().clone()));
            let _ = tx.send(Msg::Pages(i, gen, pages.borrow().clone()));
            loop {
                let sent = tokio::select! {
                    r = state.changed() => r.is_ok() && tx.send(Msg::State(i, gen, state.borrow_and_update().clone())).is_ok(),
                    r = pages.changed() => r.is_ok() && tx.send(Msg::Pages(i, gen, pages.borrow_and_update().clone())).is_ok(),
                    r = dialogs.changed() => r.is_ok() && tx.send(Msg::Dialogs(i, gen, dialogs.borrow_and_update().clone())).is_ok(),
                    ev = events.recv() => match ev {
                        Ok(e) => tx.send(Msg::Event(i, gen, e)).is_ok(),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                        Err(_) => false,
                    },
                };
                if !sent {
                    break;
                }
            }
        });
        let c = &mut self.conns[i];
        c.conn = Some(conn);
        c.mirror = Some(mirror);
        c.conn_state = ConnectionState::Connecting;
        c.phase = Phase::Running;
        if self.active.as_ref().is_some_and(|a| a.0 == i) || self.active.is_none() {
            self.focus_mirror = true;
        }
        cx.notify();
    }

    fn apply(&mut self, msg: Msg, cx: &mut Context<Self>) {
        match msg {
            Msg::Prepared(i, gen, p) if self.fresh(i, gen) => match p {
                Prepared::Ready => self.start_connection(i, cx),
                Prepared::NeedsStart => {
                    self.conns[i].phase = Phase::Blocked(
                        model::overlay_for(&ConnectionState::NotRunning, true).expect("overlay"),
                    );
                }
                Prepared::Failed { title, detail } => {
                    self.conns[i].phase = Phase::Blocked(
                        Overlay::new(title)
                            .detail(detail)
                            .action(OverlayAction::Retry),
                    );
                }
            },
            Msg::Probe(i, gen, p) if self.fresh(i, gen) => {
                let c = &mut self.conns[i];
                match multi::probe_effect(&p, c.probeable()) {
                    ProbeEffect::Stopped => {
                        c.phase = Phase::Blocked(
                            model::overlay_for(&ConnectionState::NotRunning, true)
                                .expect("overlay"),
                        )
                    }
                    ProbeEffect::Idle => c.phase = Phase::Idle,
                    ProbeEffect::Failed => {
                        if let Prepared::Failed { title, detail } = p {
                            c.phase = Phase::Blocked(
                                Overlay::new(title)
                                    .detail(detail)
                                    .action(OverlayAction::Retry),
                            );
                        }
                    }
                    ProbeEffect::Ignore => {}
                }
            }
            Msg::Launched(i, r) => {
                let c = &mut self.conns[i];
                c.starting = false;
                match r {
                    Ok(()) => {
                        c.notice = None;
                        match &c.conn {
                            Some(conn) => conn.retry(),
                            None => {
                                c.pending_cdp = None;
                                self.begin_prepare(i, cx)
                            }
                        }
                    }
                    Err(e) => c.notice = Some(format!("Could not start the Profile: {e}")),
                }
            }
            Msg::State(i, gen, s) if self.fresh(i, gen) => {
                let c = &mut self.conns[i];
                if s == ConnectionState::Connected {
                    c.notice = None;
                }
                if s != ConnectionState::Connected {
                    // No modals for a Connection that is not connected.
                    c.dialogs.clear();
                }
                let was_up = c.conn_state == ConnectionState::Connected;
                let reconnected = s == ConnectionState::Connected && !was_up && c.was_connected;
                c.conn_state = s.clone();
                if s == ConnectionState::Connected {
                    c.was_connected = true;
                    if reconnected {
                        // Target ids may have changed: drop the old marks.
                        self.stop_agent(i);
                    }
                    self.start_agent(i);
                }
                if s == ConnectionState::Connected && self.new_tab_after_connect == Some(i) {
                    self.new_tab_after_connect = None;
                    self.expect_new_tab = Some(i);
                    self.open_url_tab(i, "about:blank");
                }
                self.resolve_pending();
            }
            Msg::Pages(i, gen, p) if self.fresh(i, gen) => self.apply_pages(i, p, cx),
            Msg::Dialogs(i, gen, d) if self.fresh(i, gen) => {
                let c = &mut self.conns[i];
                c.dialogs = if c.conn_state == ConnectionState::Connected {
                    d
                } else {
                    Vec::new()
                };
            }
            Msg::Event(i, gen, ConnectionEvent::ScreencastFailed { error, .. })
                if self.fresh(i, gen) =>
            {
                self.conns[i].notice = Some(format!("Screencast failed: {error}"));
            }
            // Success needs nothing: the modal goes when the dialog leaves the
            // pending list. Failure: let the user try again.
            Msg::DialogAnswered(i, gen, Err(e)) if self.fresh(i, gen) => {
                if let Some(d) = &mut self.dialog_input {
                    d.busy = false;
                }
                self.conns[i].notice = Some(format!("Could not answer the dialog: {e}"));
            }
            Msg::AgentStarted(i, gen, r) if self.conns[i].agent_gen == gen => {
                let c = &mut self.conns[i];
                c.agent_starting = false;
                match r {
                    Ok(proxy) => {
                        c.agent_port = proxy.port();
                        c.agent_error = None;
                        c.agent = Some(AgentRun {
                            endpoint: proxy.endpoint(),
                            _proxy: proxy,
                            state: AgentState::default(),
                        });
                    }
                    Err(e) => c.agent_error = Some(format!("Agent endpoint: {e}")),
                }
            }
            Msg::AgentState(i, gen, st) if self.conns[i].agent_gen == gen => {
                if let Some(a) = &mut self.conns[i].agent {
                    a.state = st;
                }
            }
            Msg::Notice(i, n) => {
                if let Some(c) = self.conns.get_mut(i) {
                    c.notice = Some(n);
                }
            }
            _ => {} // stale generation, or an event nobody needs
        }
        cx.notify();
    }

    // ---- Agent proxy ------------------------------------------------------

    /// Start the Agent proxy of Connection `i` if it is enabled, connected
    /// and not already running. Binds 127.0.0.1 only, on the remembered port.
    fn start_agent(&mut self, i: usize) {
        let c = &mut self.conns[i];
        if !c.agent_on
            || c.agent.is_some()
            || c.agent_starting
            || c.conn_state != ConnectionState::Connected
        {
            return;
        }
        let Some((url, token)) = c.upstream.clone() else {
            return;
        };
        let secret = c
            .agent_secret
            .get_or_insert_with(random_path_secret)
            .clone();
        c.agent_starting = true;
        c.agent_error = None;
        c.agent_gen += 1;
        let (gen, port, tx) = (c.agent_gen, c.agent_port, self.ui_tx.clone());
        self.rt.spawn(async move {
            let mut cfg = AgentProxyConfig::new(url, token);
            cfg.bind_port = port;
            cfg.path_secret = Some(secret);
            let mut res = AgentProxy::start(cfg.clone()).await;
            // A just-stopped proxy may still hold its port for a moment.
            for _ in 0..10 {
                if res.is_ok() || port == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                res = AgentProxy::start(cfg.clone()).await;
            }
            match res {
                Ok(proxy) => {
                    let mut st = proxy.state();
                    if tx.send(Msg::AgentStarted(i, gen, Ok(proxy))).is_err() {
                        return;
                    }
                    while st.changed().await.is_ok() {
                        let snap = st.borrow_and_update().clone();
                        if tx.send(Msg::AgentState(i, gen, snap)).is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Msg::AgentStarted(i, gen, Err(port_error(port, &e))));
                }
            }
        });
    }

    /// Stop the Agent proxy (Agent clients are dropped, marks cleared).
    fn stop_agent(&mut self, i: usize) {
        let c = &mut self.conns[i];
        c.agent = None;
        c.agent_starting = false;
        c.agent_gen += 1;
        c.agent_copied = false;
    }

    fn set_agent_enabled(&mut self, i: usize, on: bool, cx: &mut Context<Self>) {
        self.conns[i].agent_on = on;
        if on {
            self.start_agent(i);
        } else {
            self.stop_agent(i);
            self.conns[i].agent_error = None;
        }
        cx.notify();
    }

    fn copy_endpoint(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(a) = &self.conns[i].agent {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(a.endpoint.clone()));
            self.conns[i].agent_copied = true;
            cx.notify();
        }
    }

    /// The Agent marker of a Tab of Connection `conn`.
    fn agent_marker(&self, conn: &str, target_id: Option<&str>) -> Marker {
        let state = self
            .idx_of(conn)
            .and_then(|i| self.conns[i].agent.as_ref())
            .map(|a| &a.state);
        agent_model::marker(state, target_id, std::time::Instant::now())
    }

    /// An Agent client is attached to Connection `i`.
    fn agent_attached(&self, i: usize) -> bool {
        agent_model::chip_marked(self.conns[i].agent.as_ref().map(|a| &a.state))
    }

    /// What the chip popover shows about the Agent endpoint.
    fn agent_line(&self, i: usize) -> AgentLine {
        let c = &self.conns[i];
        match (&c.agent, &c.agent_error) {
            (Some(a), _) => AgentLine::On {
                endpoint: a.endpoint.clone(),
                clients: a.state.clients,
            },
            (None, Some(e)) => AgentLine::Error(e.clone()),
            (None, None) if c.agent_on => AgentLine::Waiting,
            _ => AgentLine::Off,
        }
    }

    /// Whether a message of generation `gen` still belongs to Connection `i`.
    fn fresh(&self, i: usize, gen: u64) -> bool {
        self.conns.get(i).is_some_and(|c| c.gen == gen)
    }

    /// A new list of live pages of Connection `ci`: prune its closed Tabs,
    /// reconcile its stored Tabs (other Connections' entries are untouched),
    /// place popups, keep the Active Tab valid.
    fn apply_pages(&mut self, ci: usize, pages: Vec<TargetInfo>, cx: &mut Context<Self>) {
        let c = &mut self.conns[ci];
        let old: Vec<String> = c.pages.iter().map(|p| p.target_id.clone()).collect();
        let new: Vec<String> = pages.iter().map(|p| p.target_id.clone()).collect();
        let connected = c.conn_state == ConnectionState::Connected;
        let popups = multi::new_popups(&c.pages, &pages);
        c.pages = pages;
        let live: Vec<rb_store::TargetInfo> = c
            .pages
            .iter()
            .map(|p| rb_store::TargetInfo {
                target_id: p.target_id.clone(),
                title: p.title.clone(),
                url: p.url.clone(),
            })
            .collect();
        let name = c.name.clone();
        let gone = sm::closed_targets(&old, &new, connected);
        self.edit_state(cx, |s| {
            sm::prune_closed(s, &name, &gone);
            s.reconcile(&name, &live);
        });
        // Popups: next to their opener; active when the opener was.
        for (popup, opener) in &popups {
            let activate = multi::popup_activates(
                self.active.as_ref().map(|(c, t)| (*c, t.as_str())),
                ci,
                opener,
            );
            self.edit_state(cx, |s| {
                multi::place_popup(s, &name, popup, opener);
            });
            if activate {
                self.active = Some((ci, popup.clone()));
                self.sync_active();
                self.focus_mirror = true;
            }
        }
        if self.expect_new_tab == Some(ci) {
            if let Some(n) = new.iter().find(|id| !old.contains(id)) {
                self.expect_new_tab = None;
                self.active = Some((ci, n.clone()));
                self.sync_active();
                self.focus_mirror = true;
            }
        }
        self.resolve_pending();
        let hold = self
            .preferred
            .is_some_and(|p| p != ci && self.conns[p].connecting());
        let want = multi::choose_active(
            self.active.as_ref().map(|(c, t)| (*c, t.as_str())),
            ci,
            &self.conns[ci].pages,
            hold,
        );
        if want != self.active {
            self.active = want;
            self.sync_active();
            self.focus_mirror = true;
        }
    }

    /// Activate the Tab the user clicked while its Connection was down, once
    /// it is live.
    fn resolve_pending(&mut self) {
        let Some(tab_id) = self.pending_select.clone() else {
            return;
        };
        let Some(i) = self.state.tab_index(&tab_id) else {
            self.pending_select = None;
            return;
        };
        let e = self.state.tabs[i].clone();
        if self.is_live_target(&e.connection, e.target_id.as_deref()) {
            self.pending_select = None;
            if let (Some(ci), Some(t)) = (self.idx_of(&e.connection), e.target_id) {
                self.active = Some((ci, t));
                self.sync_active();
                self.focus_mirror = true;
            }
        }
    }

    // ---- actions ----------------------------------------------------------

    /// Tell the Connections which Tab is the Active Tab: the owner starts its
    /// screencast, the Connection that streamed before stops. Exactly one
    /// Connection streams.
    fn sync_active(&mut self) {
        let new_conn = self.active.as_ref().map(|a| a.0);
        if let Some(old) = multi::stop_streaming(self.streaming, new_conn) {
            if let Some(conn) = self.conns.get(old).and_then(|c| c.conn.as_ref()) {
                conn.set_active_tab(None);
            }
        }
        if let Some((ci, target_id)) = self.active.clone() {
            if let Some(conn) = &self.conns[ci].conn {
                conn.set_active_tab(Some(ActiveTab {
                    target_id,
                    screencast: ScreencastOptions::default(),
                }));
            }
        }
        self.streaming = new_conn;
    }

    fn select_tab(
        &mut self,
        ci: usize,
        target_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_select = None;
        if self.active.as_ref().map(|(c, t)| (*c, t.as_str())) != Some((ci, target_id)) {
            self.active = Some((ci, target_id.to_string()));
            self.last_conn = Some(ci);
            self.sync_active();
        }
        self.focus_mirror_now(window, cx);
        cx.notify();
    }

    /// A click (or Cmd+n) on a Sidebar Tab. A live Tab becomes Active; a
    /// Placeholder Tab connects its Connection first and is activated when its
    /// pages arrive (matched by target id, then URL, by the state
    /// reconcile); a leftover entry on a connected Connection reopens its URL.
    fn activate_tab(&mut self, tab_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.state.tab_index(tab_id) else {
            return;
        };
        let entry = self.state.tabs[i].clone();
        let Some(ci) = self.idx_of(&entry.connection) else {
            return; // a Tab of a Connection that is no longer configured
        };
        if self.is_live_target(&entry.connection, entry.target_id.as_deref()) {
            self.select_tab(ci, &entry.target_id.unwrap_or_default(), window, cx);
        } else if self.tabs_live_of(ci) {
            self.pending_select = Some(tab_id.to_string());
            self.open_url_tab(ci, &entry.url);
        } else {
            self.pending_select = Some(tab_id.to_string());
            self.connect_conn(ci, cx);
        }
    }

    fn focus_mirror_now(&mut self, window: &mut Window, cx: &App) {
        if let Some(m) = self.active_mirror() {
            window.focus(&m.read(cx).focus_handle(cx));
        }
    }

    /// The Mirror of the Active Tab's Connection.
    fn active_mirror(&self) -> Option<Entity<Mirror>> {
        let (ci, _) = self.active.as_ref()?;
        self.conns[*ci].mirror.clone()
    }

    fn report<F>(&self, ci: usize, fut: F)
    where
        F: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            if let Err(e) = fut.await {
                let _ = tx.send(Msg::Notice(ci, e));
            }
        });
    }

    fn page_cmd(&self, cmd: PageCmd) {
        let Some((ci, _)) = &self.active else { return };
        let Some(conn) = &self.conns[*ci].conn else {
            return;
        };
        let session = conn.active_session().borrow().clone();
        let Some(session) = session else { return };
        self.report(*ci, actions::run_page(session, cmd));
    }

    fn open_url_tab(&mut self, ci: usize, url: &str) {
        if let Some(client) = self.conns[ci].conn.as_ref().and_then(|c| c.client()) {
            let url = if url.is_empty() { "about:blank" } else { url };
            let url = url.to_string();
            self.report(ci, async move { actions::new_tab(&client, &url).await });
        }
    }

    /// Cmd+T and the "+ New Tab" row: a blank Tab in the target Connection.
    fn new_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(i) = self.target_conn_idx() {
            self.new_tab_in(i, cx);
        }
    }

    /// A blank Tab in Connection `i` (becomes the Active Tab). A disconnected
    /// Connection connects first and then opens it; a stopped Profile is not
    /// started.
    fn new_tab_in(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.tabs_live_of(i)
            && self.conns[i]
                .conn
                .as_ref()
                .is_some_and(|c| c.client().is_some())
        {
            self.expect_new_tab = Some(i);
            self.open_url_tab(i, "about:blank");
        } else {
            self.new_tab_after_connect = Some(i);
            self.connect_conn(i, cx);
        }
    }

    /// Close a Sidebar Tab. A live Tab is closed on the Remote Browser (a
    /// pinned one stays in the Sidebar); an entry with no live page is just
    /// forgotten.
    fn close_tab(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(i) = self.state.tab_index(tab_id) else {
            return;
        };
        let entry = self.state.tabs[i].clone();
        if self.is_live_target(&entry.connection, entry.target_id.as_deref()) {
            let (Some(ci), Some(target)) = (self.idx_of(&entry.connection), entry.target_id) else {
                return;
            };
            if self.is_active(&entry.connection, Some(&target)) {
                self.active = self.neighbour_of(tab_id).or(self.active.take());
                self.sync_active();
                self.focus_mirror = true;
            }
            if let Some(client) = self.conns[ci].conn.as_ref().and_then(|c| c.client()) {
                self.report(
                    ci,
                    async move { actions::close_tab(&client, &target).await },
                );
            }
        } else {
            self.edit_state(cx, |s| {
                let _ = s.remove_tab_id(tab_id);
            });
        }
    }

    /// The live Tab next to `tab_id` in Sidebar order (next, else previous).
    fn neighbour_of(&self, tab_id: &str) -> Option<(usize, String)> {
        let layout = self.layout();
        let flat: Vec<_> = sm::flat_tabs(&layout)
            .into_iter()
            .filter(|t| self.is_live_target(&t.connection, t.target_id.as_deref()))
            .collect();
        let pos = flat.iter().position(|t| t.id == tab_id)?;
        let t = flat
            .get(pos + 1)
            .or(pos.checked_sub(1).and_then(|p| flat.get(p)))?;
        Some((self.idx_of(&t.connection)?, t.target_id.clone()?))
    }

    fn close_active(&mut self, cx: &mut Context<Self>) {
        let Some((ci, active)) = self.active.clone() else {
            return;
        };
        let name = self.conns[ci].name.clone();
        let id = self
            .state
            .tabs
            .iter()
            .find(|e| e.connection == name && e.target_id.as_deref() == Some(active.as_str()))
            .and_then(|e| e.id.clone());
        if let Some(id) = id {
            self.close_tab(&id, cx);
        }
    }

    fn start_profile(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(m) = self.conns[i].manager.clone() else {
            return;
        };
        self.last_conn = Some(i);
        self.conns[i].starting = true;
        self.conns[i].notice = None;
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            let _ = tx.send(Msg::Launched(i, connect::launch(&m).await));
        });
        cx.notify();
    }

    fn overlay_action(&mut self, i: usize, action: OverlayAction, cx: &mut Context<Self>) {
        match action {
            OverlayAction::Retry | OverlayAction::Connect => self.connect_conn(i, cx),
            OverlayAction::StartProfile => self.start_profile(i, cx),
        }
        cx.notify();
    }

    fn chip_action(&mut self, i: usize, action: ChipAction, cx: &mut Context<Self>) {
        match action {
            ChipAction::NewTab => self.new_tab_in(i, cx),
            ChipAction::Disconnect => self.disconnect(i, cx),
            ChipAction::RetryNow | ChipAction::Connect => self.connect_conn(i, cx),
            ChipAction::Start => self.start_profile(i, cx),
        }
        self.chip_open = None;
        self.overflow_open = false;
        cx.notify();
    }

    /// Apply a context menu choice.
    fn menu_action(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        if let MenuAction::CloseTab(id) = &action {
            self.close_tab(id, cx);
            return;
        }
        let mut renamed = None;
        self.edit_state(cx, |s| renamed = sm::apply_menu_action(s, &action));
        if let Some(g) = renamed {
            self.start_rename(&g, window, cx);
        }
    }

    fn start_rename(&mut self, group: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = self.state.group(group) else {
            return;
        };
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.rename = Some(Rename {
            group: group.to_string(),
            edit: LineEdit::with_text(&g.name),
            focus,
            seen_focus: false,
        });
        cx.notify();
    }

    fn finish_rename(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.rename.take() else { return };
        if commit && !r.edit.buf.trim().is_empty() {
            let name = r.edit.buf.clone();
            self.edit_state(cx, |s| {
                let _ = s.rename_group(&r.group, &name);
            });
        }
        self.focus_mirror_now(window, cx);
        cx.notify();
    }

    fn rename_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        if ks.modifiers.platform {
            return; // app shortcuts pass through
        }
        let Some(r) = &mut self.rename else { return };
        let out = r.edit.key(
            ks.key.as_str(),
            ks.key_char.as_deref(),
            ks.modifiers.control || ks.modifiers.function,
        );
        match out {
            sm::EditOutcome::Commit => self.finish_rename(true, window, cx),
            sm::EditOutcome::Cancel => self.finish_rename(false, window, cx),
            sm::EditOutcome::Changed => {}
            sm::EditOutcome::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Finish a drag: apply the drop hint to the state.
    fn drop_tab(&mut self, dragged: &str, cx: &mut Context<Self>) {
        let Some(target) = self.drop_hint.take() else {
            return;
        };
        let layout = self.layout();
        self.edit_state(cx, |s| {
            sm::apply_drop(s, &layout, dragged, &target);
        });
    }

    fn set_drop_hint(&mut self, hint: DropTarget, cx: &mut Context<Self>) {
        if self.drop_hint.as_ref() != Some(&hint) {
            self.drop_hint = Some(hint);
            cx.notify();
        }
    }

    fn chip_hover_changed(&mut self, i: usize, hovered: bool, cx: &mut Context<Self>) {
        self.chip_over = hovered;
        if hovered {
            self.chip_open = Some(i);
            self.hover_gen += 1;
        } else {
            self.schedule_popover_close(cx);
        }
        cx.notify();
    }

    fn popover_hover_changed(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.pop_over = hovered;
        if !hovered {
            self.schedule_popover_close(cx);
        } else {
            self.hover_gen += 1;
        }
    }

    fn schedule_popover_close(&mut self, cx: &mut Context<Self>) {
        self.hover_gen += 1;
        let gen = self.hover_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(220))
                .await;
            let _ = this.update(cx, |r, cx| {
                if r.hover_gen == gen && !r.chip_over && !r.pop_over {
                    r.chip_open = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn current_url(&self) -> String {
        let Some((ci, t)) = &self.active else {
            return String::new();
        };
        self.conns[*ci]
            .pages
            .iter()
            .find(|p| &p.target_id == t)
            .map(|p| p.url.clone())
            .unwrap_or_default()
    }

    fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.address.buf = self.current_url();
        self.address.select_all = true;
        window.focus(&self.address.focus);
        cx.notify();
    }

    fn address_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        if ks.modifiers.platform {
            // Cmd+V / Cmd+A edit the field; other Cmd shortcuts (Cmd+B, Cmd+Q) pass on.
            match ks.key.as_str() {
                "v" => {
                    if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        let t = t.replace(['\n', '\r'], " ");
                        if self.address.select_all {
                            self.address.buf.clear();
                            self.address.select_all = false;
                        }
                        self.address.buf.push_str(&t);
                    }
                    cx.stop_propagation();
                }
                "a" => {
                    self.address.select_all = true;
                    cx.stop_propagation();
                }
                _ => return,
            }
            cx.notify();
            return;
        }
        let mut edit = LineEdit {
            buf: std::mem::take(&mut self.address.buf),
            select_all: self.address.select_all,
        };
        let out = edit.key(
            ks.key.as_str(),
            ks.key_char.as_deref(),
            ks.modifiers.control || ks.modifiers.function,
        );
        self.address.buf = edit.buf;
        self.address.select_all = edit.select_all;
        match out {
            sm::EditOutcome::Commit => {
                if let Some(url) = model::normalize_url(&self.address.buf) {
                    self.page_cmd(PageCmd::Navigate(url));
                }
                self.address.select_all = false;
                self.focus_mirror_now(window, cx);
            }
            sm::EditOutcome::Cancel => self.focus_mirror_now(window, cx),
            sm::EditOutcome::Changed => {}
            sm::EditOutcome::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    // ---- rendering --------------------------------------------------------

    /// A message over the Mirror area (or the whole window): what is wrong and
    /// what the user can do. `conn` is the Connection it is about.
    fn overlay_view(
        &self,
        conn: Option<usize>,
        o: &Overlay,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let starting = conn.is_some_and(|i| self.conns[i].starting);
        let mut detail = o.detail.clone().unwrap_or_default();
        if let Some(n) = conn.and_then(|i| self.conns[i].notice.as_ref()) {
            if !detail.is_empty() {
                detail.push_str("\n\n");
            }
            detail.push_str(n);
        }
        let button = match (o.action, starting, conn) {
            (Some(_), true, _) => Some(
                div()
                    .text_color(hsla(p.ink2))
                    .child("Starting the Profile...")
                    .into_any_element(),
            ),
            (Some(a), false, Some(i)) => {
                let label = match a {
                    OverlayAction::StartProfile => "Start Profile",
                    OverlayAction::Retry => "Retry",
                    OverlayAction::Connect => "Connect",
                };
                Some(
                    div()
                        .id("overlay-action")
                        .px_4()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(hsla(p.accent))
                        .text_color(gpui::white())
                        .hover(|s| s.opacity(0.88))
                        .child(label)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.overlay_action(i, a, cx)
                        }))
                        .into_any_element(),
                )
            }
            _ => None,
        };
        let name = conn.map(|i| self.conns[i].name.clone());
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .p_6()
            .bg(hsla(p.win))
            .text_color(hsla(p.ink))
            .child(div().text_xl().child(o.title.clone()))
            .children(name.map(|n| {
                div()
                    .text_sm()
                    .text_color(hsla(p.ink3))
                    .child(SharedString::from(n))
            }))
            .children((!detail.is_empty()).then(|| {
                div()
                    .max_w(px(720.))
                    .font_family("Menlo")
                    .text_sm()
                    .text_color(hsla(p.ink2))
                    .child(detail)
            }))
            .children(button)
    }

    fn top_bar(&self, window: &Window, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .w(px(30.))
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .text_color(hsla(p.ink2))
                .hover(|s| s.bg(hsla(p.hover)))
                .child(label)
        };
        let editing = self.address.focus.is_focused(window);
        let text = if editing {
            self.address.buf.clone()
        } else {
            self.current_url()
        };
        let dark = theme::is_dark(window.appearance());
        let style = self.target_connection().cloned();
        let field =
            div()
                .id("address")
                .track_focus(&self.address.focus)
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    this.address_key(ev, window, cx)
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        if !this.address.focus.is_focused(window) {
                            this.focus_address(window, cx);
                        }
                    }),
                )
                .flex_1()
                .min_w_0()
                .h(px(26.))
                .mx_2()
                .px_2()
                .gap_2()
                .flex()
                .items_center()
                .rounded_md()
                .bg(hsla(p.field))
                .border_1()
                .border_color(if editing {
                    hsla(p.accent)
                } else {
                    hsla(p.line)
                })
                .overflow_hidden()
                .text_color(hsla(p.ink))
                .children(style.as_ref().map(|s| {
                    sidebar::conn_dot(s.shape, hsla(s.color(dark)), 9.).into_any_element()
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .when(editing && self.address.select_all, |d| {
                            d.bg(hsla(p.accent & 0xffffff55))
                        })
                        .whitespace_nowrap()
                        .child(SharedString::from(if editing {
                            format!("{text}|")
                        } else {
                            text
                        })),
                )
                .children(style.map(|s| {
                    div()
                        .flex_none()
                        .px_1()
                        .rounded(px(4.))
                        .bg(hsla(p.hover))
                        .text_xs()
                        .text_color(hsla(p.ink3))
                        .child(SharedString::from(s.name))
                }));
        div()
            .flex_none()
            .h(px(46.))
            .px_3()
            .flex()
            .flex_row()
            .items_center()
            .border_b_1()
            .border_color(hsla(p.line))
            .bg(hsla(p.win))
            .child(
                button("back", "←").on_click(
                    cx.listener(|this, _: &ClickEvent, _, _| this.page_cmd(PageCmd::Back)),
                ),
            )
            .child(button("forward", "→").on_click(
                cx.listener(|this, _: &ClickEvent, _, _| this.page_cmd(PageCmd::Forward)),
            ))
            .child(
                button("reload", "↻").on_click(
                    cx.listener(|this, _: &ClickEvent, _, _| this.page_cmd(PageCmd::Reload)),
                ),
            )
            .child(field)
    }

    /// Warnings and the last error, one slim line (hidden when empty).
    fn status_bar(&self, p: &Palette) -> Option<impl IntoElement> {
        let mut extra: Vec<String> = self.warnings.clone();
        extra.extend(self.notice.clone());
        extra.extend(
            self.conns
                .iter()
                .filter_map(|c| c.agent_error.as_ref().map(|e| format!("{}: {e}", c.name))),
        );
        extra.extend(self.focus_conn().and_then(|i| {
            self.conns[i]
                .notice
                .as_ref()
                .map(|n| format!("{}: {n}", self.conns[i].name))
        }));
        if extra.is_empty() {
            return None;
        }
        Some(
            div()
                .flex_none()
                .h(px(22.))
                .px_2()
                .flex()
                .items_center()
                .text_xs()
                .border_t_1()
                .border_color(hsla(p.line))
                .bg(hsla(p.win))
                .text_color(hsla(p.warn))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(extra.join("  |  ")),
        )
    }

    /// The overlay for the Mirror area, if any, and the Connection it is about.
    fn main_overlay(&self) -> Option<(Option<usize>, Overlay)> {
        let Some(i) = self.focus_conn() else {
            return (!self.conns.is_empty()).then(|| {
                (
                    None,
                    Overlay::new("No Tab selected")
                        .detail("Click a Tab in the Sidebar, or a Connection chip to connect."),
                )
            });
        };
        let c = &self.conns[i];
        let o = match &c.phase {
            Phase::Running => model::overlay_for(&c.conn_state, c.manager.is_some())
                .filter(|_| c.mirror.is_some()),
            Phase::Connecting => Some(Overlay::new("Connecting...")),
            Phase::Blocked(o) | Phase::Problem(o) => Some(o.clone()),
            Phase::Idle => Some(
                Overlay::new("Not connected")
                    .detail("Not connected. Connect to see this Connection's Tabs.")
                    .action(OverlayAction::Connect),
            ),
        };
        o.map(|o| (Some(i), o))
    }

    fn running_body(
        &mut self,
        window: &mut Window,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let overlay = self.main_overlay();
        let mirror = self.active_mirror();
        let covering = mirror.is_some();
        let dialog = self.dialog_overlay(p, cx);
        let main = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(hsla(p.page))
            .children(mirror)
            .children(overlay.map(|(ci, o)| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .when(covering, |d| d.bg(theme::hsla(p.win & 0xffffffe6)))
                    .child(self.overlay_view(ci, &o, p, cx))
            }))
            .children(dialog);
        let sidebar = self.sidebar_view(window, p, cx);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(self.top_bar(window, p, cx))
                    .child(div().flex_1().min_h_0().child(main)),
            )
            .into_any_element()
    }
}

impl Focusable for Root {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(self.dark(window));
        self.sync_dialog_input(window, cx);
        if self.focus_mirror && self.active_mirror().is_some() && self.rename.is_none() {
            self.focus_mirror = false;
            if self.dialog_input.is_none() {
                self.focus_mirror_now(window, cx);
            }
        }
        // A Group rename ends when its field loses focus.
        if let Some(r) = &mut self.rename {
            if r.focus.is_focused(window) {
                r.seen_focus = true;
            } else if r.seen_focus {
                self.finish_rename(true, window, cx);
            } else {
                window.focus(&r.focus);
            }
        }
        let body = match self.problem.clone() {
            Some(o) => self.overlay_view(None, &o, &p, cx).into_any_element(),
            None => self.running_body(window, &p, cx),
        };
        let overlays = self.sidebar_overlays(window, &p, cx);
        let status = self.status_bar(&p);
        div()
            .key_context("Root")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.rail = !this.rail;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NewTab, _, cx| this.new_tab(cx)))
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| this.close_active(cx)))
            .on_action(
                cx.listener(|this, _: &FocusAddress, window, cx| this.focus_address(window, cx)),
            )
            .on_action(cx.listener(|this, _: &GoBack, _, _| this.page_cmd(PageCmd::Back)))
            .on_action(cx.listener(|this, _: &GoForward, _, _| this.page_cmd(PageCmd::Forward)))
            .on_action(cx.listener(|this, _: &Select1, w, cx| this.select_nth(1, w, cx)))
            .on_action(cx.listener(|this, _: &Select2, w, cx| this.select_nth(2, w, cx)))
            .on_action(cx.listener(|this, _: &Select3, w, cx| this.select_nth(3, w, cx)))
            .on_action(cx.listener(|this, _: &Select4, w, cx| this.select_nth(4, w, cx)))
            .on_action(cx.listener(|this, _: &Select5, w, cx| this.select_nth(5, w, cx)))
            .on_action(cx.listener(|this, _: &Select6, w, cx| this.select_nth(6, w, cx)))
            .on_action(cx.listener(|this, _: &Select7, w, cx| this.select_nth(7, w, cx)))
            .on_action(cx.listener(|this, _: &Select8, w, cx| this.select_nth(8, w, cx)))
            .on_action(cx.listener(|this, _: &Select9, w, cx| this.select_nth(9, w, cx)))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(13.))
            .bg(hsla(p.win))
            .text_color(hsla(p.ink))
            .child(body)
            .children(status)
            .children(overlays)
    }
}

impl Root {
    /// Cmd+`n`: the nth visible Tab.
    fn select_nth(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let layout = self.layout();
        let id = sm::tab_for_shortcut(&layout, n).map(|t| t.id.clone());
        if let Some(id) = id {
            self.activate_tab(&id, window, cx);
        }
    }
}

/// A readable reason for a failed bind.
fn port_error(port: u16, e: &rb_cdp::CdpError) -> String {
    let text = e.to_string();
    if text.contains("in use") {
        format!("port {port} is already in use; pick another `agent_proxy_port` or use 0")
    } else {
        text
    }
}
