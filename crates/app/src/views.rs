//! Root view: top bar, Sidebar (Tab list), Mirror, status bar.
//!
//! All CDP and Manager I/O runs on the app's tokio runtime. It reports back
//! to this view through one unbounded channel of [`Msg`]s that a GPUI task
//! drains (the bridge described in docs/gpui-notes.md).

use gpui::{
    div, prelude::*, px, rgb, AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Render, SharedString,
    Styled, Window,
};
use rb_cdp::{
    ActiveTab, Connection, ConnectionEvent, ConnectionState, ScreencastOptions, TargetInfo,
};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::actions::{self, PageCmd};
use crate::connect::{self, ManagerTarget, Prepared};
use crate::mirror::{Mirror, MirrorBackend};
use crate::model::{self, Overlay, OverlayAction, Tone};
use crate::startup::Startup;
use crate::state::ToggleSidebar;

const BG: u32 = 0x1e1e1e;
const BG_BAR: u32 = 0x2b2b2d;
const BG_SIDEBAR: u32 = 0x252526;
const BG_HOVER: u32 = 0x37373d;
const BG_ACTIVE: u32 = 0x094771;
const FG: u32 = 0xdddddd;
const FG_DIM: u32 = 0x9a9a9a;
const GOOD: u32 = 0x4ec9b0;
const WARN: u32 = 0xe5c07b;
const BAD: u32 = 0xf48771;

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
    /// Nothing runs: first-run, config problem, Profile stopped, Manager error.
    Problem(Overlay),
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

pub struct Root {
    focus: FocusHandle,
    rt: Handle,
    ui_tx: UnboundedSender<Msg>,
    sidebar_visible: bool,
    connection_name: String,
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

        let mut root = Root {
            focus: cx.focus_handle(),
            rt,
            ui_tx,
            sidebar_visible: true,
            connection_name: String::new(),
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
        };
        match startup {
            Startup::Problem(o) => root.phase = Phase::Problem(o),
            Startup::Ready {
                connection,
                warnings,
            } => {
                root.connection_name = connection.name.clone();
                root.warnings = warnings;
                match connect::resolve(&connection) {
                    Ok(r) => {
                        root.manager = r.manager;
                        root.pending_cdp = Some(r.cdp);
                        root.begin_prepare(cx);
                    }
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

    // ---- connecting -------------------------------------------------------

    /// Manager pre-flight, then start the Connection.
    fn begin_prepare(&mut self, cx: &mut Context<Self>) {
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
                self.phase = Phase::Problem(
                    model::overlay_for(&ConnectionState::NotRunning, true).expect("overlay"),
                );
            }
            Msg::Prepared(Prepared::Failed { title, detail }) => {
                self.phase = Phase::Problem(
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
                            None => self.begin_prepare(cx),
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
            }
            Msg::Pages(p) => {
                self.pages = p;
                let want = model::choose_active(&self.pages, self.active.as_deref());
                if want != self.active {
                    self.active = want;
                    self.sync_active();
                }
            }
            Msg::Event(ConnectionEvent::ScreencastFailed { error, .. }) => {
                self.notice = Some(format!("Screencast failed: {error}"));
            }
            Msg::Event(_) => {}
            Msg::Notice(n) => self.notice = Some(n),
        }
        cx.notify();
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

    fn new_tab(&self) {
        if let Some(client) = self.conn.as_ref().and_then(|c| c.client()) {
            self.report(async move { actions::new_tab(&client).await });
        }
    }

    fn close_tab(&self, target_id: String) {
        if let Some(client) = self.conn.as_ref().and_then(|c| c.client()) {
            self.report(async move { actions::close_tab(&client, &target_id).await });
        }
    }

    fn overlay_action(&mut self, action: OverlayAction, cx: &mut Context<Self>) {
        match action {
            OverlayAction::Retry => match &self.conn {
                Some(c) => c.retry(),
                None => self.begin_prepare(cx),
            },
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

    fn current_url(&self) -> String {
        self.active
            .as_deref()
            .and_then(|id| self.pages.iter().find(|p| p.target_id == id))
            .map(|p| p.url.clone())
            .unwrap_or_default()
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
        match ks.key.as_str() {
            "enter" => {
                if let Some(url) = model::normalize_url(&self.address.buf) {
                    self.page_cmd(PageCmd::Navigate(url));
                }
                self.address.select_all = false;
                self.focus_mirror_now(window, cx);
            }
            "escape" => self.focus_mirror_now(window, cx),
            "backspace" => {
                if self.address.select_all {
                    self.address.buf.clear();
                    self.address.select_all = false;
                } else {
                    self.address.buf.pop();
                }
            }
            _ => {
                let typed = ks
                    .key_char
                    .as_deref()
                    .filter(|c| !c.is_empty() && !ks.modifiers.control && !ks.modifiers.function);
                let Some(c) = typed else { return };
                if self.address.select_all {
                    self.address.buf.clear();
                    self.address.select_all = false;
                }
                self.address.buf.push_str(c);
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    // ---- rendering --------------------------------------------------------

    fn overlay_view(&self, o: &Overlay, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .text_color(rgb(FG_DIM))
                    .child("Starting the Profile...")
                    .into_any_element(),
            ),
            (Some(a), false) => {
                let label = match a {
                    OverlayAction::StartProfile => "Start Profile",
                    OverlayAction::Retry => "Retry",
                };
                Some(
                    div()
                        .id("overlay-action")
                        .px_4()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(rgb(BG_ACTIVE))
                        .hover(|s| s.bg(rgb(0x0e639c)))
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
            .bg(rgb(BG))
            .text_color(rgb(FG))
            .child(div().text_xl().child(o.title.clone()))
            .children((!detail.is_empty()).then(|| {
                div()
                    .max_w(px(720.))
                    .font_family("Menlo")
                    .text_sm()
                    .text_color(rgb(FG_DIM))
                    .child(detail)
            }))
            .children(button)
    }

    fn top_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .w(px(28.))
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(BG_HOVER)))
                .child(label)
        };
        let editing = self.address.focus.is_focused(window);
        let text = if editing {
            self.address.buf.clone()
        } else {
            self.current_url()
        };
        let field = div()
            .id("address")
            .track_focus(&self.address.focus)
            .on_key_down(
                cx.listener(|this, ev: &KeyDownEvent, window, cx| this.address_key(ev, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    if !this.address.focus.is_focused(window) {
                        this.address.buf = this.current_url();
                        this.address.select_all = true;
                        window.focus(&this.address.focus);
                        cx.notify();
                    }
                }),
            )
            .flex_1()
            .h(px(26.))
            .px_2()
            .flex()
            .items_center()
            .rounded_md()
            .bg(rgb(0x1a1a1a))
            .border_1()
            .border_color(rgb(if editing { 0x3794ff } else { 0x3c3c3c }))
            .overflow_hidden()
            .child(
                div()
                    .when(editing && self.address.select_all, |d| d.bg(rgb(0x264f78)))
                    .whitespace_nowrap()
                    .child(SharedString::from(if editing {
                        format!("{text}|")
                    } else {
                        text
                    })),
            );
        div()
            .flex_none()
            .h(px(40.))
            .px_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .bg(rgb(BG_BAR))
            .child(
                button("back", "◀").on_click(
                    cx.listener(|this, _: &ClickEvent, _, _| this.page_cmd(PageCmd::Back)),
                ),
            )
            .child(button("forward", "▶").on_click(
                cx.listener(|this, _: &ClickEvent, _, _| this.page_cmd(PageCmd::Forward)),
            ))
            .child(
                button("reload", "⟳").on_click(
                    cx.listener(|this, _: &ClickEvent, _, _| this.page_cmd(PageCmd::Reload)),
                ),
            )
            .child(field)
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = model::tab_rows(&self.pages, self.active.as_deref());
        let mut list = div().flex().flex_col().gap_px().overflow_hidden();
        for row in rows {
            let id = row.target_id.clone();
            let close_id = row.target_id.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("tab-{}", row.target_id)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(row.active, |d| d.bg(rgb(BG_ACTIVE)))
                    .when(!row.active, |d| d.hover(|s| s.bg(rgb(BG_HOVER))))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.select_tab(&id, window, cx)
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_sm()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(model::truncate(&row.title, 28)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(FG_DIM))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(row.url.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("close-{}", row.target_id)))
                            .w(px(20.))
                            .h(px(20.))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .text_color(rgb(FG_DIM))
                            .hover(|s| s.bg(rgb(0x555555)).text_color(rgb(FG)))
                            .child("×")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(close_id.clone());
                            })),
                    ),
            );
        }
        div()
            .w(px(260.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .bg(rgb(BG_SIDEBAR))
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(SharedString::from(format!("TABS ({})", self.pages.len()))),
            )
            .child(list)
            .child(
                div()
                    .id("new-tab")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(rgb(FG_DIM))
                    .hover(|s| s.bg(rgb(BG_HOVER)).text_color(rgb(FG)))
                    .child("+ New Tab")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, _| this.new_tab())),
            )
    }

    fn status_bar(&self) -> impl IntoElement {
        let (text, tone) = if matches!(self.phase, Phase::Problem(_)) {
            ("Not connected".to_string(), Tone::Bad)
        } else {
            model::status_line(&self.connection_name, &self.conn_state)
        };
        let color = match tone {
            Tone::Good => GOOD,
            Tone::Warn => WARN,
            Tone::Bad => BAD,
        };
        let mut extra: Vec<String> = self.warnings.clone();
        extra.extend(self.notice.clone());
        div()
            .flex_none()
            .h(px(22.))
            .px_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .text_xs()
            .bg(rgb(BG_BAR))
            .child(div().text_color(rgb(color)).child(format!("● {text}")))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(rgb(WARN))
                    .child(extra.join("  |  ")),
            )
    }

    fn running_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let overlay = model::overlay_for(&self.conn_state, self.manager.is_some())
            .filter(|_| self.mirror.is_some());
        let main = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .children(self.mirror.clone())
            .children(overlay.map(|o| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .bg(gpui::rgba(0x1e1e1ed9))
                    .child(self.overlay_view(&o, cx))
            }));
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.top_bar(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .children(self.sidebar_visible.then(|| self.sidebar(cx)))
                    .child(main),
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
        if self.focus_mirror && self.mirror.is_some() {
            self.focus_mirror = false;
            self.focus_mirror_now(window, cx);
        }
        let body = match &self.phase {
            Phase::Running => self.running_body(window, cx),
            Phase::Problem(o) => {
                let o = o.clone();
                self.overlay_view(&o, cx).into_any_element()
            }
            Phase::Connecting => self
                .overlay_view(&Overlay::new("Connecting..."), cx)
                .into_any_element(),
        };
        div()
            .key_context("Root")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.sidebar_visible = !this.sidebar_visible;
                cx.notify();
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(FG))
            .child(body)
            .child(self.status_bar())
    }
}
