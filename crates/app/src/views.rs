//! Root view: top bar, Sidebar (Tabs, Groups, Connection chips), Mirror.
//!
//! All CDP and Manager I/O runs on the app's tokio runtime. It reports back
//! to this view through one unbounded channel of [`Msg`]s that a GPUI task
//! drains (the bridge described in docs/gpui-notes.md).
//!
//! Tab Groups, pins and Placeholder Tabs live in an `rb_store::State`, loaded
//! from `state.toml` at startup, reconciled with the live pages on every
//! change and saved (debounced) after mutations. The Sidebar renders from
//! `State::sidebar_layout` (see `views/sidebar.rs`).

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    div, prelude::*, px, AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Point, Render,
    SharedString, Styled, Window,
};
use rb_cdp::{
    ActiveTab, Connection, ConnectionEvent, ConnectionState, ScreencastOptions, TargetInfo,
};
use rb_store::{ConnectionConfig, SidebarItem, State};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::actions::{self, PageCmd};
use crate::connect::{self, ManagerTarget, Prepared};
use crate::mirror::{Mirror, MirrorBackend};
use crate::model::{self, Overlay, OverlayAction};
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

mod sidebar;

/// Debounce before `state.toml` is written after a change.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Messages from the tokio side to the view.
enum Msg {
    Prepared(Prepared),
    Launched(Result<(), String>),
    State(ConnectionState),
    Pages(Vec<TargetInfo>),
    Event(ConnectionEvent),
    Notice(String),
}

enum Phase {
    /// Checking the Profile / connecting for the first time.
    Connecting,
    /// Nothing can run (first run, config problem): the whole window shows this.
    Problem(Overlay),
    /// The Connection cannot connect (Profile stopped, Manager error): the
    /// Sidebar stays, the Mirror area shows this.
    Blocked(Overlay),
    /// Not connected on purpose (the user disconnected).
    Idle,
    /// A Connection exists; its state decides about an overlay.
    Running,
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

pub struct Root {
    focus: FocusHandle,
    rt: Handle,
    ui_tx: UnboundedSender<Msg>,
    /// The Sidebar is the narrow icon rail.
    rail: bool,
    /// Styles of all known Connections, in config order (one for now).
    conn_styles: Vec<ConnStyle>,
    connection_name: String,
    /// Config of the Connection, to arm it again after a disconnect.
    connection_cfg: Option<ConnectionConfig>,
    warnings: Vec<String>,
    phase: Phase,
    manager: Option<ManagerTarget>,
    /// Config for the Connection, until it is started.
    pending_cdp: Option<rb_cdp::ConnectionConfig>,
    conn: Option<Connection>,
    mirror: Option<Entity<Mirror>>,
    conn_state: ConnectionState,
    pages: Vec<TargetInfo>,
    active: Option<String>,
    /// The Manager Profile is being started.
    starting: bool,
    /// Last launch or command error, shown in the status bar and overlay.
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
    chip_over: bool,
    pop_over: bool,
    hover_gen: u64,
    drop_hint: Option<DropTarget>,
    /// A Tab (by id) to activate as soon as its Connection is up.
    pending_select: Option<String>,
    /// The next new page becomes the Active Tab (after Cmd+T).
    expect_new_tab: bool,
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
            async {}
        })
        .detach();

        let mut root = Root {
            focus: cx.focus_handle(),
            rt,
            ui_tx,
            rail: false,
            conn_styles: Vec::new(),
            connection_name: String::new(),
            connection_cfg: None,
            warnings: Vec::new(),
            phase: Phase::Connecting,
            manager: None,
            pending_cdp: None,
            conn: None,
            mirror: None,
            conn_state: ConnectionState::Connecting,
            pages: Vec::new(),
            active: None,
            starting: false,
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
            chip_over: false,
            pop_over: false,
            hover_gen: 0,
            drop_hint: None,
            pending_select: None,
            expect_new_tab: false,
        };
        match startup {
            Startup::Problem(o) => root.phase = Phase::Problem(o),
            Startup::Ready {
                connection,
                index,
                warnings,
            } => {
                root.connection_name = connection.name.clone();
                root.conn_styles = vec![ConnStyle::new(
                    &connection.name,
                    connection.color.as_deref(),
                    index,
                )];
                root.warnings = warnings;
                root.load_state();
                root.connection_cfg = Some(connection);
                match root.arm() {
                    Ok(()) => root.begin_prepare(cx),
                    Err(e) => {
                        root.phase = Phase::Problem(
                            Overlay::new("Invalid Connection in config.toml").detail(e),
                        )
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

    /// The Connection's link state, for chips and Placeholder Tabs.
    fn link(&self) -> LinkState {
        match &self.phase {
            Phase::Running => LinkState::Live(self.conn_state.clone()),
            Phase::Connecting => LinkState::Connecting,
            Phase::Blocked(o) if o.action == Some(OverlayAction::StartProfile) => {
                LinkState::ProfileStopped
            }
            Phase::Blocked(_) | Phase::Idle | Phase::Problem(_) => LinkState::Off,
        }
    }

    fn chip_status(&self) -> ChipStatus {
        sm::chip_status(&self.link(), self.manager.is_some())
    }

    fn layout(&self) -> Vec<SidebarItem> {
        let live = sm::tabs_live(&self.link());
        let name = self.connection_name.clone();
        self.state.sidebar_layout(&move |c: &str| live && c == name)
    }

    fn is_live_target(&self, target_id: Option<&str>) -> bool {
        sm::tabs_live(&self.link())
            && target_id.is_some_and(|t| self.pages.iter().any(|p| p.target_id == t))
    }

    /// The Connection a new Tab opens in: the Active Tab's, else the first.
    fn target_connection(&self) -> Option<&ConnStyle> {
        let active = self
            .active
            .as_deref()
            .and_then(|t| {
                self.state
                    .tabs
                    .iter()
                    .find(|e| e.target_id.as_deref() == Some(t))
            })
            .and_then(|e| sm::style_of(&self.conn_styles, &e.connection));
        active.or_else(|| self.conn_styles.first())
    }

    // ---- connecting -------------------------------------------------------

    /// Build the CDP config (and Manager client) from the saved Connection.
    fn arm(&mut self) -> Result<(), String> {
        let cfg = self.connection_cfg.as_ref().ok_or("no Connection")?;
        let r = connect::resolve(cfg)?;
        self.manager = r.manager;
        self.pending_cdp = Some(r.cdp);
        Ok(())
    }

    /// Manager pre-flight, then start the Connection.
    fn begin_prepare(&mut self, cx: &mut Context<Self>) {
        if self.pending_cdp.is_none() {
            if let Err(e) = self.arm() {
                self.phase =
                    Phase::Problem(Overlay::new("Invalid Connection in config.toml").detail(e));
                return;
            }
        }
        self.phase = Phase::Connecting;
        if self.manager.is_none() {
            self.start_connection(cx);
            return;
        }
        let manager = self.manager.clone();
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            let _ = tx.send(Msg::Prepared(connect::prepare(manager.as_ref()).await));
        });
        cx.notify();
    }

    /// Connect (or retry) the Connection. Never starts a stopped Profile.
    fn connect(&mut self, cx: &mut Context<Self>) {
        match (&self.conn, &self.phase) {
            (Some(c), _) => c.retry(),
            (None, Phase::Connecting) => {}
            (None, _) => self.begin_prepare(cx),
        }
        cx.notify();
    }

    fn disconnect(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.conn.take() {
            self.rt.spawn(async move { c.close().await });
        }
        self.mirror = None;
        self.pending_cdp = None;
        self.pages.clear();
        self.active = None;
        self.conn_state = ConnectionState::Closed;
        self.phase = Phase::Idle;
        self.notice = None;
        cx.notify();
    }

    fn start_connection(&mut self, cx: &mut Context<Self>) {
        let Some(cfg) = self.pending_cdp.take() else {
            return;
        };
        let conn = {
            let _enter = self.rt.enter(); // Connection::start spawns on tokio
            Connection::start(cfg)
        };
        let backend =
            MirrorBackend::from_connection(&conn, self.rt.clone()).expect("fresh Connection");
        self.mirror = Some(cx.new(|cx| Mirror::new(backend, cx)));
        self.focus_mirror = true;

        // One pump forwards state, Tab list and events to the UI.
        let mut state = conn.state();
        let mut pages = conn.pages();
        let mut events = conn.events();
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            let _ = tx.send(Msg::State(state.borrow().clone()));
            let _ = tx.send(Msg::Pages(pages.borrow().clone()));
            loop {
                let sent = tokio::select! {
                    r = state.changed() => r.is_ok() && tx.send(Msg::State(state.borrow_and_update().clone())).is_ok(),
                    r = pages.changed() => r.is_ok() && tx.send(Msg::Pages(pages.borrow_and_update().clone())).is_ok(),
                    ev = events.recv() => match ev {
                        Ok(e) => tx.send(Msg::Event(e)).is_ok(),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                        Err(_) => false,
                    },
                };
                if !sent {
                    break;
                }
            }
        });
        self.conn = Some(conn);
        self.phase = Phase::Running;
        cx.notify();
    }

    fn apply(&mut self, msg: Msg, cx: &mut Context<Self>) {
        match msg {
            Msg::Prepared(Prepared::Ready) => self.start_connection(cx),
            Msg::Prepared(Prepared::NeedsStart) => {
                self.phase = Phase::Blocked(
                    model::overlay_for(&ConnectionState::NotRunning, true).expect("overlay"),
                );
            }
            Msg::Prepared(Prepared::Failed { title, detail }) => {
                self.phase = Phase::Blocked(
                    Overlay::new(title)
                        .detail(detail)
                        .action(OverlayAction::Retry),
                );
            }
            Msg::Launched(r) => {
                self.starting = false;
                match r {
                    Ok(()) => {
                        self.notice = None;
                        match &self.conn {
                            Some(c) => c.retry(),
                            None => {
                                self.pending_cdp = None;
                                self.begin_prepare(cx)
                            }
                        }
                    }
                    Err(e) => self.notice = Some(format!("Could not start the Profile: {e}")),
                }
            }
            Msg::State(s) => {
                if s == ConnectionState::Connected {
                    self.notice = None;
                }
                self.conn_state = s;
                self.resolve_pending();
            }
            Msg::Pages(p) => self.apply_pages(p, cx),
            Msg::Event(ConnectionEvent::ScreencastFailed { error, .. }) => {
                self.notice = Some(format!("Screencast failed: {error}"));
            }
            Msg::Event(_) => {}
            Msg::Notice(n) => self.notice = Some(n),
        }
        cx.notify();
    }

    /// A new list of live pages: prune closed Tabs, reconcile the stored
    /// Tabs, keep the Active Tab valid.
    fn apply_pages(&mut self, pages: Vec<TargetInfo>, cx: &mut Context<Self>) {
        let old: Vec<String> = self.pages.iter().map(|p| p.target_id.clone()).collect();
        let new: Vec<String> = pages.iter().map(|p| p.target_id.clone()).collect();
        let connected = self.conn_state == ConnectionState::Connected;
        self.pages = pages;
        let live: Vec<rb_store::TargetInfo> = self
            .pages
            .iter()
            .map(|p| rb_store::TargetInfo {
                target_id: p.target_id.clone(),
                title: p.title.clone(),
                url: p.url.clone(),
            })
            .collect();
        let name = self.connection_name.clone();
        let gone = sm::closed_targets(&old, &new, connected);
        self.edit_state(cx, |s| {
            sm::prune_closed(s, &name, &gone);
            s.reconcile(&name, &live);
        });
        if self.expect_new_tab {
            if let Some(n) = new.iter().find(|id| !old.contains(id)) {
                self.expect_new_tab = false;
                self.active = Some(n.clone());
                self.sync_active();
                self.focus_mirror = true;
            }
        }
        self.resolve_pending();
        let want = model::choose_active(&self.pages, self.active.as_deref());
        if want != self.active {
            self.active = want;
            self.sync_active();
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
        let target = self.state.tabs[i].target_id.clone();
        if self.is_live_target(target.as_deref()) {
            self.pending_select = None;
            self.active = target;
            self.sync_active();
            self.focus_mirror = true;
        }
    }

    // ---- actions ----------------------------------------------------------

    /// Tell the Connection which Tab is the Active Tab (starts its screencast).
    fn sync_active(&self) {
        if let Some(conn) = &self.conn {
            conn.set_active_tab(self.active.clone().map(|target_id| ActiveTab {
                target_id,
                screencast: ScreencastOptions::default(),
            }));
        }
    }

    fn select_tab(&mut self, target_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.active.as_deref() != Some(target_id) {
            self.active = Some(target_id.to_string());
            self.sync_active();
        }
        self.focus_mirror_now(window, cx);
        cx.notify();
    }

    /// A click (or Cmd+n) on a Sidebar Tab. A live Tab becomes Active; a
    /// Placeholder Tab connects its Connection first; a leftover entry on a
    /// connected Connection reopens its URL.
    fn activate_tab(&mut self, tab_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.state.tab_index(tab_id) else {
            return;
        };
        let entry = self.state.tabs[i].clone();
        if self.is_live_target(entry.target_id.as_deref()) {
            self.pending_select = None;
            self.select_tab(&entry.target_id.unwrap_or_default(), window, cx);
        } else if sm::tabs_live(&self.link()) {
            self.pending_select = Some(tab_id.to_string());
            self.open_url_tab(&entry.url);
        } else {
            self.pending_select = Some(tab_id.to_string());
            self.connect(cx);
        }
    }

    fn focus_mirror_now(&mut self, window: &mut Window, cx: &App) {
        if let Some(m) = &self.mirror {
            window.focus(&m.read(cx).focus_handle(cx));
        }
    }

    fn report<F>(&self, fut: F)
    where
        F: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let tx = self.ui_tx.clone();
        self.rt.spawn(async move {
            if let Err(e) = fut.await {
                let _ = tx.send(Msg::Notice(e));
            }
        });
    }

    fn page_cmd(&self, cmd: PageCmd) {
        let Some(conn) = &self.conn else { return };
        let session = conn.active_session().borrow().clone();
        let Some(session) = session else { return };
        self.report(actions::run_page(session, cmd));
    }

    fn open_url_tab(&mut self, url: &str) {
        if let Some(client) = self.conn.as_ref().and_then(|c| c.client()) {
            let url = if url.is_empty() { "about:blank" } else { url };
            let url = url.to_string();
            self.report(async move { actions::new_tab(&client, &url).await });
        }
    }

    /// Cmd+T, the "+ New Tab" row and a chip's "+": a blank Tab in the target
    /// Connection (becomes the Active Tab). A disconnected Connection connects
    /// instead; a stopped Profile is not started.
    fn new_tab(&mut self, cx: &mut Context<Self>) {
        if sm::tabs_live(&self.link()) && self.conn.as_ref().is_some_and(|c| c.client().is_some()) {
            self.expect_new_tab = true;
            self.open_url_tab("about:blank");
        } else {
            self.connect(cx);
        }
    }

    /// Close a Sidebar Tab. A live Tab is closed on the Remote Browser (a
    /// pinned one stays in the Sidebar); an entry with no live page is just
    /// forgotten.
    fn close_tab(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(i) = self.state.tab_index(tab_id) else {
            return;
        };
        let target = self.state.tabs[i].target_id.clone();
        if self.is_live_target(target.as_deref()) {
            let target = target.unwrap_or_default();
            if self.active.as_deref() == Some(target.as_str()) {
                self.active = self.neighbour_of(tab_id).or(self.active.take());
                self.sync_active();
                self.focus_mirror = true;
            }
            if let Some(client) = self.conn.as_ref().and_then(|c| c.client()) {
                self.report(async move { actions::close_tab(&client, &target).await });
            }
        } else {
            self.edit_state(cx, |s| {
                let _ = s.remove_tab_id(tab_id);
            });
        }
    }

    /// The live Tab next to `tab_id` in Sidebar order (next, else previous).
    fn neighbour_of(&self, tab_id: &str) -> Option<String> {
        let layout = self.layout();
        let flat: Vec<_> = sm::flat_tabs(&layout)
            .into_iter()
            .filter(|t| self.is_live_target(t.target_id.as_deref()))
            .collect();
        let pos = flat.iter().position(|t| t.id == tab_id)?;
        flat.get(pos + 1)
            .or(pos.checked_sub(1).and_then(|p| flat.get(p)))
            .and_then(|t| t.target_id.clone())
    }

    fn close_active(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.active.clone() else {
            return;
        };
        let id = self
            .state
            .tabs
            .iter()
            .find(|e| e.target_id.as_deref() == Some(active.as_str()))
            .and_then(|e| e.id.clone());
        if let Some(id) = id {
            self.close_tab(&id, cx);
        }
    }

    fn overlay_action(&mut self, action: OverlayAction, cx: &mut Context<Self>) {
        match action {
            OverlayAction::Retry | OverlayAction::Connect => self.connect(cx),
            OverlayAction::StartProfile => {
                let Some(m) = self.manager.clone() else {
                    return;
                };
                self.starting = true;
                self.notice = None;
                let tx = self.ui_tx.clone();
                self.rt.spawn(async move {
                    let _ = tx.send(Msg::Launched(connect::launch(&m).await));
                });
            }
        }
        cx.notify();
    }

    fn chip_action(&mut self, action: ChipAction, cx: &mut Context<Self>) {
        match action {
            ChipAction::NewTab => self.new_tab(cx),
            ChipAction::Disconnect => self.disconnect(cx),
            ChipAction::RetryNow | ChipAction::Connect => self.connect(cx),
            ChipAction::Start => self.overlay_action(OverlayAction::StartProfile, cx),
        }
        self.chip_open = None;
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
        self.active
            .as_deref()
            .and_then(|id| self.pages.iter().find(|p| p.target_id == id))
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

    fn overlay_view(&self, o: &Overlay, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let starting = self.starting;
        let mut detail = o.detail.clone().unwrap_or_default();
        if let Some(n) = &self.notice {
            if !detail.is_empty() {
                detail.push_str("\n\n");
            }
            detail.push_str(n);
        }
        let button = match (o.action, starting) {
            (Some(_), true) => Some(
                div()
                    .text_color(hsla(p.ink2))
                    .child("Starting the Profile...")
                    .into_any_element(),
            ),
            (Some(a), false) => {
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
                            this.overlay_action(a, cx)
                        }))
                        .into_any_element(),
                )
            }
            (None, _) => None,
        };
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

    /// The overlay for the Mirror area, if any.
    fn main_overlay(&self) -> Option<Overlay> {
        match &self.phase {
            Phase::Running => model::overlay_for(&self.conn_state, self.manager.is_some())
                .filter(|_| self.mirror.is_some()),
            Phase::Connecting => Some(Overlay::new("Connecting...")),
            Phase::Blocked(o) | Phase::Problem(o) => Some(o.clone()),
            Phase::Idle => Some(
                Overlay::new("Not connected")
                    .detail("Disconnected. Connect to see this Connection's Tabs.")
                    .action(OverlayAction::Connect),
            ),
        }
    }

    fn running_body(
        &mut self,
        window: &mut Window,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let overlay = self.main_overlay();
        let main = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(hsla(p.page))
            .children(self.mirror.clone())
            .children(overlay.map(|o| {
                let covering = self.mirror.is_some();
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .when(covering, |d| d.bg(theme::hsla(p.win & 0xffffffe6)))
                    .child(self.overlay_view(&o, p, cx))
            }));
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
        if self.focus_mirror && self.mirror.is_some() && self.rename.is_none() {
            self.focus_mirror = false;
            self.focus_mirror_now(window, cx);
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
        let body = match &self.phase {
            Phase::Problem(o) if self.connection_cfg.is_none() => {
                let o = o.clone();
                self.overlay_view(&o, &p, cx).into_any_element()
            }
            _ => self.running_body(window, &p, cx),
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
