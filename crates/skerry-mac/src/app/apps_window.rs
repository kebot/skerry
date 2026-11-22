//! The session's Apps window (`ARCH-035`): the proot-apps catalog as a native three-pane
//! window, one per session, owned by its `SessionWindow`.
//!
//! The window holds the board (`apps::Board`); the session window only forwards events to it.
//! Opening sends `list`, which brings the whole state. A reconnect keeps the window open
//! (unlike Files, `ARCH-023`): it clears the board, waits, and lists again on the next
//! stream's first allowed `server_settings`. Teardown closes it. Every request passes the
//! `apps::Link` gate, so nothing goes out while disconnected or not allowed.
//!
//! Icons are fetched lazily for visible tiles (at most `apps::ICON_FETCHES` at once), decoded
//! off the main thread (at most `apps::ICON_DECODES` at once, SVG under a watchdog), and kept
//! as this window's textures: retired through a queue drained in `render`
//! (`window.drop_image`, `ARCH-014`), and freed with the window's atlas when it closes. A
//! generation drops loads that finish after a source change or Retry. Catalogs (never
//! textures) are cached app-wide by URL.

use super::session_window::SessionWindow;
use crate::apps::{
    self, Board, CatalogApp, Event, Filter, IconPixels, IconQueue, JobState, Op, Policy, Requested,
};
use crate::persist::WriteQueue;
use crate::selkies::SelkiesClient;
use gpui_kit::component::{
    input::{InputEvent, InputState},
    ActiveTheme as _,
};
use gpui_kit::{
    actions, point, prelude::*, px, size, AnyWindowHandle, App, Bounds, ClipboardItem, Context,
    Entity, FocusHandle, Global, RenderImage, ScrollHandle, ScrollStrategy, SharedString, Task,
    TitlebarOptions, UniformListScrollHandle, WeakEntity, Window, WindowBounds, WindowOptions,
};
use skerry_ui::apps::{
    self as ui, ActivityRow, AppTile, AppsAction, AppsProps, Badge, Banner, ButtonKind,
    CatalogView, CategoryRow, ConsoleProps, FilterId, Inspector, InspectorButtons, JobBadge,
};
use skerry_ui::files::Notice;
use smallvec::smallvec;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Handle;

actions!(
    apps_window,
    [
        FocusSearch,
        ClearOrDeselect,
        CloseApps,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        PrimaryAction,
        RemoveSelection
    ]
);

/// The Apps window's keys, all scoped to it: `AppsWindow` everywhere in it, `AppsGrid` while
/// the grid (not the search field) has focus. None of them is global, so a session window
/// never loses a key to Apps (`ARCH-031`).
pub(super) fn key_bindings() -> Vec<gpui_kit::KeyBinding> {
    use gpui_kit::KeyBinding;
    let window = Some("AppsWindow");
    let grid = Some("AppsGrid");
    vec![
        KeyBinding::new("cmd-f", FocusSearch, window),
        KeyBinding::new("escape", ClearOrDeselect, window),
        KeyBinding::new("cmd-w", CloseApps, window),
        KeyBinding::new("left", SelectLeft, grid),
        KeyBinding::new("right", SelectRight, grid),
        KeyBinding::new("up", SelectUp, grid),
        KeyBinding::new("down", SelectDown, grid),
        KeyBinding::new("enter", PrimaryAction, grid),
        // ⌘⌫ is Remove…, through the same confirmation.
        KeyBinding::new("cmd-backspace", RemoveSelection, grid),
    ]
}

/// Who opened the Apps window: its session window, told when it closes.
#[derive(Clone)]
pub(super) struct AppsOwner {
    pub(super) window: AnyWindowHandle,
    pub(super) session: WeakEntity<SessionWindow>,
}

/// What the session window hands the Apps window when it opens.
pub(super) struct AppsSource {
    pub(super) title: String,
    pub(super) policy: Policy,
    pub(super) selkies: Option<SelkiesClient>,
    /// The server, for saving `apps_category`.
    pub(super) server: String,
    pub(super) saved_filter: Option<String>,
}

/// Catalogs fetched this run, by URL. Only the parsed apps: textures belong to windows.
#[derive(Default)]
struct AppsCatalogCache(HashMap<String, Rc<[CatalogApp]>>);

impl Global for AppsCatalogCache {}

enum CatalogState {
    Loading,
    Ready(Rc<[CatalogApp]>),
    Failed(String),
}

enum IconState {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

/// Where the catalog and icons come from.
#[derive(Clone)]
enum Fetcher {
    /// `None`: the restricted client couldn't be built, so nothing is fetched.
    Remote(Option<reqwest::Client>),
    Local(SelkiesClient),
}

pub(super) struct AppsWindow {
    owner: AppsOwner,
    rt: Handle,
    server: String,
    link: apps::Link,
    /// The session's Selkies never answered `list` (`apps::Probe`): nothing is sent.
    too_old: bool,
    selkies: Option<SelkiesClient>,
    /// `None` if it couldn't be built: GitHub is then never asked.
    plain: Option<reqwest::Client>,
    board: Board,
    /// Bumped by every board change, so the console text is rebuilt only then.
    board_version: u64,
    /// An `apps_state` arrived on this stream: only then is a missing installed set news.
    state_seen: bool,
    catalog: CatalogState,
    /// The URL of the catalog shown, and whether it is the session's own.
    catalog_key: Option<String>,
    catalog_local: bool,
    catalog_generation: u64,
    catalog_task: Option<Task<()>>,
    categories: Vec<String>,
    filter: Option<Filter>,
    saved_filter: Option<String>,
    search: Entity<InputState>,
    search_text: String,
    selected: Option<String>,
    icons: HashMap<String, IconState>,
    icon_queue: IconQueue,
    decodes: Arc<tokio::sync::Semaphore>,
    /// An SVG decode overran the watchdog: no more SVG decodes in this window.
    svg_stopped: Arc<AtomicBool>,
    /// Textures to free with `window.drop_image` on the next render.
    retired: Vec<Arc<RenderImage>>,
    requested: HashMap<String, Requested>,
    canceling: HashSet<u64>,
    last_launch: Option<(String, Instant)>,
    notice: Option<Notice>,
    notice_generation: u64,
    grid_focus: FocusHandle,
    grid_scroll: UniformListScrollHandle,
    console_scroll: ScrollHandle,
    /// The console shown (job id, board version) and its text.
    console_cache: Option<(u64, u64, SharedString)>,
    columns: usize,
    reveal_selection: bool,
}

/// Lets go of the Apps window: the session window's hold, deferred (`ARCH-015`). Needed for
/// every way the window closes; `remove_window` does not run `on_window_should_close`.
fn release(owner: &AppsOwner, cx: &mut App) {
    let owner = owner.clone();
    cx.defer(move |cx| {
        let _ = owner.window.update(cx, |_, _, cx| {
            let _ = owner
                .session
                .update(cx, |session, _| session.apps_window = None);
        });
    });
}

/// The source list's icon for a category (the web's `CATEGORY_ICONS`).
fn category_icon(category: &str) -> gpui_kit::assets::IconName {
    use gpui_kit::assets::IconName as I;
    match category {
        "WebBrowser" => I::Globe,
        "Network" => I::Network,
        "Game" => I::Gamepad2,
        "Creative" => I::Palette,
        "AudioVideo" => I::Clapperboard,
        "Office" => I::FileText,
        "Development" => I::Code,
        "Utility" => I::Wrench,
        "AI" => I::Sparkles,
        "System" => I::Cpu,
        "Science" => I::FlaskConical,
        "Education" => I::GraduationCap,
        _ => I::Shapes,
    }
}

fn job_badge(state: JobState) -> JobBadge {
    match state {
        JobState::Queued => JobBadge::Queued,
        JobState::Running => JobBadge::Running,
        JobState::Done => JobBadge::Done,
        JobState::Failed => JobBadge::Failed,
        JobState::Canceled => JobBadge::Canceled,
    }
}

fn button_kind(op: Op) -> Option<ButtonKind> {
    Some(match op {
        Op::Install => ButtonKind::Install,
        Op::Remove => ButtonKind::Remove,
        Op::Update => ButtonKind::Update,
        Op::Launch => ButtonKind::Open,
        Op::List | Op::Cancel | Op::Clear => return None,
    })
}

fn filter_id(filter: &Filter) -> FilterId {
    match filter {
        Filter::Installed => FilterId::Installed,
        Filter::All => FilterId::All,
        Filter::Category(category) => FilterId::Category(category.clone()),
    }
}

fn initial(app: &CatalogApp) -> char {
    app.full_name
        .chars()
        .chain(app.name.chars())
        .find(|c| c.is_alphanumeric())
        .map_or('?', |c| c.to_uppercase().next().unwrap_or(c))
}

fn render_image(pixels: IconPixels) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.bgra)?;
    Some(Arc::new(RenderImage::new(smallvec![image::Frame::new(
        buffer
    )])))
}

impl AppsWindow {
    /// Opens the Apps window over `window` (the session window), titled "<app> — Apps".
    pub(super) fn open(
        source: AppsSource,
        rt: Handle,
        window: &Window,
        owner: AppsOwner,
        cx: &mut App,
    ) -> gpui_kit::Result<(AnyWindowHandle, Entity<Self>)> {
        let bounds = window.bounds();
        let origin = point(bounds.left() + px(40.), bounds.top() + px(60.));
        let created: Rc<std::cell::RefCell<Option<Entity<Self>>>> = Rc::default();
        let slot = Rc::clone(&created);
        let (handle, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    origin,
                    size(px(980.), px(640.)),
                ))),
                window_min_size: Some(size(px(720.), px(460.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("{} — Apps", source.title).into()),
                    ..Default::default()
                }),
                display_id: window.display(cx).map(|display| display.id()),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                let close_owner = owner.clone();
                let view = cx.new(|cx: &mut Context<Self>| {
                    let search =
                        cx.new(|cx| InputState::new(window, cx).placeholder("Search Apps"));
                    cx.subscribe_in(&search, window, |this, state, event: &InputEvent, _, cx| {
                        if matches!(event, InputEvent::Change) {
                            this.search_text = state.read(cx).value().trim().to_owned();
                            cx.notify();
                        }
                    })
                    .detach();
                    cx.observe_window_activation(window, |_, _, cx| cx.notify())
                        .detach();
                    let mut this = Self {
                        owner,
                        rt,
                        server: source.server,
                        link: apps::Link::default(),
                        too_old: false,
                        selkies: source.selkies,
                        plain: apps::plain_client(),
                        board: Board::default(),
                        board_version: 0,
                        state_seen: false,
                        catalog: CatalogState::Loading,
                        catalog_key: None,
                        catalog_local: source.policy.local_repo,
                        catalog_generation: 0,
                        catalog_task: None,
                        categories: Vec::new(),
                        filter: None,
                        saved_filter: source.saved_filter,
                        search,
                        search_text: String::new(),
                        selected: None,
                        icons: HashMap::new(),
                        icon_queue: IconQueue::default(),
                        decodes: Arc::new(tokio::sync::Semaphore::new(apps::ICON_DECODES)),
                        svg_stopped: Arc::default(),
                        retired: Vec::new(),
                        requested: HashMap::new(),
                        canceling: HashSet::new(),
                        last_launch: None,
                        notice: None,
                        notice_generation: 0,
                        grid_focus: cx.focus_handle(),
                        grid_scroll: UniformListScrollHandle::new(),
                        console_scroll: ScrollHandle::new(),
                        console_cache: None,
                        columns: 3,
                        reveal_selection: false,
                    };
                    this.load_catalog(false, cx);
                    // The stream is up, allows Apps, and answers `list` (checked by the
                    // session): opening lists the whole state. A reconnect's list is the
                    // session's support probe, whose answer reaches this window.
                    this.set_policy(source.policy, cx);
                    this.send(Op::List, None, None, cx);
                    this
                });
                window.on_window_should_close(cx, move |_, cx| {
                    release(&close_owner, cx);
                    true
                });
                let search = view.read(cx).search.clone();
                search.update(cx, |search, cx| search.focus(window, cx));
                *slot.borrow_mut() = Some(view.clone());
                view
            },
        )?;
        let view = created
            .borrow_mut()
            .take()
            .ok_or_else(|| std::io::Error::other("the Apps window has no view"))?;
        Ok((handle, view))
    }

    // ---- From the session window -----------------------------------------------------

    /// A `server_settings` policy: the first allowed one of a stream lists; a change of
    /// catalog source reloads the catalog and starts the icons over.
    pub(super) fn set_policy(&mut self, policy: Policy, cx: &mut Context<Self>) {
        let change = self.link.on_policy(policy);
        if change.source_changed || policy.local_repo != self.catalog_local {
            self.catalog_local = policy.local_repo;
            self.reset_icons();
            self.load_catalog(false, cx);
        }
        if change.changed {
            cx.notify();
        }
    }

    /// The session's file routes (and so its local catalog) for the current stream.
    pub(super) fn set_access(&mut self, access: Option<SelkiesClient>, cx: &mut Context<Self>) {
        self.selkies = access;
        if self.selkies.is_some() && self.catalog_local {
            // Icons that failed for want of the session get another go.
            let failed: Vec<String> = self
                .icons
                .iter()
                .filter(|(_, state)| matches!(state, IconState::Failed))
                .map(|(icon, _)| icon.clone())
                .collect();
            for icon in failed {
                self.icons.remove(&icon);
                self.icon_queue.forget(&icon);
            }
            cx.notify();
        }
        if self.catalog_local && !matches!(self.catalog, CatalogState::Ready(_)) {
            self.load_catalog(false, cx);
        }
    }

    /// The stream ended: the window stays, waiting, with nothing pending (`ARCH-035`).
    pub(super) fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.link.on_disconnect();
        // The next stream probes again.
        self.too_old = false;
        self.board = Board::default();
        self.board_version += 1;
        self.state_seen = false;
        self.requested.clear();
        self.canceling.clear();
        self.last_launch = None;
        self.selkies = None;
        cx.notify();
    }

    /// The session's support probe settled: the server never answered `list`.
    pub(super) fn set_too_old(&mut self, too_old: bool, cx: &mut Context<Self>) {
        if self.too_old != too_old {
            self.too_old = too_old;
            cx.notify();
        }
    }

    fn can_send(&self) -> bool {
        self.link.can_send() && !self.too_old
    }

    /// An apps event from the stream, already parsed on Tokio.
    pub(super) fn receive(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::State(state) => {
                self.board.apply_state(&state);
                self.state_seen = true;
            }
            Event::Job(job) => {
                self.board.apply_job(&job);
            }
            Event::CommandError(text) => {
                if let Some((app, at)) = &self.last_launch {
                    if apps::launch_error_applies(Some(*at), Instant::now()) {
                        let name = self.full_name(app);
                        self.say(format!("Couldn’t open “{name}”: {}", text.trim()), true, cx);
                    }
                }
                return;
            }
        }
        self.board_version += 1;
        let board = &self.board;
        self.requested
            .retain(|app, requested| !requested.answered_by(board, app));
        self.canceling
            .retain(|id| board.jobs.iter().any(|job| job.id == *id && job.pending()));
        cx.notify();
    }

    // ---- Requests --------------------------------------------------------------------

    /// Sends one request through the gate; `false` when it didn't go.
    fn send(&mut self, op: Op, app: Option<&str>, id: Option<u64>, cx: &mut Context<Self>) -> bool {
        if !self.can_send() {
            return false;
        }
        let Some(request) = apps::request(op, app, id) else {
            return false;
        };
        let owner = self.owner.clone();
        // The session window is another window's entity: reach it through its handle, after
        // this update (`ARCH-015`).
        cx.defer(move |cx| {
            let _ = owner.window.update(cx, |_, _, cx| {
                let _ = owner
                    .session
                    .update(cx, |session, _| session.send_apps(request));
            });
        });
        true
    }

    /// Install, Update or Remove: "Requested…" until the app's job shows up, or 5 s.
    fn request_job(&mut self, op: Op, app: &str, cx: &mut Context<Self>) {
        if self.busy(app) || !self.send(op, Some(app), None, cx) {
            return;
        }
        let at = Instant::now();
        self.requested.insert(
            app.to_owned(),
            Requested {
                op,
                at,
                after_id: self.board.max_id(),
            },
        );
        let app = app.to_owned();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(apps::REQUEST_TIMEOUT).await;
            let _ = this.update(cx, |this, cx| {
                let expired = this
                    .requested
                    .get(&app)
                    .is_some_and(|r| r.at == at && r.expired(Instant::now()));
                if expired {
                    this.requested.remove(&app);
                    let name = this.full_name(&app);
                    this.say(
                        format!(
                            "The session didn’t start this. Another job for “{name}” may be running."
                        ),
                        true,
                        cx,
                    );
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// The app has a pending job or a request awaiting one.
    fn busy(&self, app: &str) -> bool {
        self.requested.contains_key(app)
            || self.board.job_for_app(app).is_some_and(|job| job.pending())
    }

    fn launch(&mut self, app: &str, cx: &mut Context<Self>) {
        if !self.send(Op::Launch, Some(app), None, cx) {
            return;
        }
        self.last_launch = Some((app.to_owned(), Instant::now()));
        // The person opens an app to use it: bring the session forward.
        let window = self.owner.window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, _| window.activate_window());
        });
    }

    fn cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        if !self.canceling.contains(&id) && self.send(Op::Cancel, None, Some(id), cx) {
            // A running job says nothing until the runner exits: "Canceling…" until then.
            self.canceling.insert(id);
        }
    }

    fn primary(&mut self, app: &str, cx: &mut Context<Self>) {
        if self.busy(app) || !self.can_send() {
            return;
        }
        if self.board.is_installed(app) {
            self.launch(app, cx);
        } else {
            self.request_job(Op::Install, app, cx);
        }
    }

    /// Remove… asks first, in a native warning alert.
    fn confirm_remove(&mut self, app: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy(&app) || !self.can_send() || !self.board.is_installed(&app) {
            return;
        }
        let name = self.full_name(&app);
        let answer = window.prompt(
            gpui_kit::PromptLevel::Warning,
            &format!("Remove “{name}” from this session?"),
            Some("The app is deleted from the session. Its files in your home folder are not touched."),
            &[
                gpui_kit::PromptButton::new("Remove"),
                gpui_kit::PromptButton::cancel("Cancel"),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |this, cx| this.request_job(Op::Remove, &app, cx));
            }
        })
        .detach();
    }

    // ---- Catalog ---------------------------------------------------------------------

    fn fetcher(&self) -> Option<(String, Fetcher)> {
        if self.catalog_local {
            let selkies = self.selkies.clone()?;
            Some((
                selkies.apps_metadata_url().to_string(),
                Fetcher::Local(selkies),
            ))
        } else {
            Some((
                apps::remote_catalog_url().to_string(),
                Fetcher::Remote(self.plain.clone()),
            ))
        }
    }

    /// Shows the catalog for the current source: from the app-wide cache, else fetched
    /// (`force`, from Try Again, always fetches).
    fn load_catalog(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some((key, fetcher)) = self.fetcher() else {
            // A local repository waits for the session's file routes.
            self.catalog = CatalogState::Loading;
            self.catalog_key = None;
            cx.notify();
            return;
        };
        if !force {
            if self.catalog_key.as_ref() == Some(&key)
                && matches!(self.catalog, CatalogState::Ready(_))
            {
                return;
            }
            let cached = cx
                .try_global::<AppsCatalogCache>()
                .and_then(|cache| cache.0.get(&key).cloned());
            if let Some(apps) = cached {
                self.show_catalog(key, apps, cx);
                return;
            }
        }
        self.catalog_generation += 1;
        let generation = self.catalog_generation;
        self.catalog = CatalogState::Loading;
        self.catalog_key = Some(key.clone());
        let local = matches!(fetcher, Fetcher::Local(_));
        let pending = self.rt.spawn(async move {
            let bytes = match fetcher {
                Fetcher::Remote(client) => {
                    let client = client.ok_or("no HTTPS client")?;
                    apps::fetch_capped(
                        &client,
                        apps::remote_catalog_url(),
                        apps::CATALOG_CAP,
                        apps::CATALOG_TIMEOUT,
                    )
                    .await
                }
                Fetcher::Local(selkies) => selkies.apps_metadata().await.map_err(|e| e.to_string()),
            }?;
            let text = String::from_utf8(bytes).map_err(|_| "not text".to_owned())?;
            apps::parse_catalog(&text)
        });
        self.catalog_task = Some(cx.spawn(async move |this, cx| {
            let result = pending.await;
            let _ = this.update(cx, |this, cx| {
                if this.catalog_generation != generation {
                    return;
                }
                match result {
                    Ok(Ok(apps)) => {
                        let apps: Rc<[CatalogApp]> = apps.into();
                        cx.default_global::<AppsCatalogCache>()
                            .0
                            .insert(key.clone(), Rc::clone(&apps));
                        this.show_catalog(key, apps, cx);
                    }
                    Ok(Err(error)) => {
                        eprintln!("Apps catalog: {error}");
                        this.catalog = CatalogState::Failed(
                            if local {
                                "Couldn’t load the app catalog from the session."
                            } else {
                                "Couldn’t load the app catalog. Check the Mac’s internet connection."
                            }
                            .into(),
                        );
                    }
                    Err(_) => {
                        this.catalog = CatalogState::Failed(
                            "Couldn’t load the app catalog from the session.".into(),
                        )
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn show_catalog(&mut self, key: String, apps: Rc<[CatalogApp]>, cx: &mut Context<Self>) {
        self.categories = apps::categories(&apps);
        let keep = self.filter.clone().filter(|filter| match filter {
            Filter::Category(category) => self.categories.contains(category),
            _ => true,
        });
        self.filter = Some(keep.unwrap_or_else(|| {
            apps::default_filter(&self.categories, self.saved_filter.as_deref())
        }));
        self.catalog = CatalogState::Ready(apps);
        self.catalog_key = Some(key);
        cx.notify();
    }

    fn apps(&self) -> &[CatalogApp] {
        match &self.catalog {
            CatalogState::Ready(apps) => apps,
            _ => &[],
        }
    }

    fn app(&self, name: &str) -> Option<&CatalogApp> {
        self.apps().iter().find(|app| app.name == name)
    }

    fn full_name(&self, name: &str) -> String {
        self.app(name)
            .map_or_else(|| name.to_owned(), |app| app.full_name.clone())
    }

    fn choose_filter(&mut self, filter: Filter, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_text.is_empty() {
            self.clear_search(window, cx);
        }
        let saved = filter.saved();
        self.saved_filter = Some(saved.clone());
        self.filter = Some(filter);
        self.grid_scroll.scroll_to_item(0, ScrollStrategy::Top);
        let done = cx
            .global::<WriteQueue>()
            .preferences(&self.server, move |p| p.apps_category = Some(saved.clone()));
        cx.spawn(async move |this, cx| {
            if !matches!(done.await, Ok(Ok(()))) {
                let _ = this.update(cx, |this, cx| {
                    this.say("Couldn’t save the chosen category.", true, cx);
                });
            }
        })
        .detach();
    }

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
        self.search_text.clear();
    }

    /// The tiles shown, in order.
    fn shown(&self) -> Vec<&CatalogApp> {
        let filter = self.filter.clone().unwrap_or(Filter::All);
        apps::filtered(
            self.apps(),
            &filter,
            &self.search_text,
            self.board.installed.as_ref(),
        )
    }

    /// Moves the selection by `step` tiles in the shown order.
    fn step(&mut self, step: isize, cx: &mut Context<Self>) {
        let names: Vec<String> = self.shown().iter().map(|app| app.name.clone()).collect();
        if names.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|selected| names.iter().position(|name| name == selected));
        let next = match at {
            None => 0,
            Some(at) => (at as isize + step).clamp(0, names.len() as isize - 1) as usize,
        };
        self.selected = Some(names[next].clone());
        self.reveal_selection = true;
        cx.notify();
    }

    // ---- Icons -----------------------------------------------------------------------

    fn want_icons(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        let mut asked = false;
        for name in names {
            let Some(icon) = self.app(&name).map(|app| app.icon.clone()) else {
                continue;
            };
            if icon.is_empty() || self.icons.contains_key(&icon) {
                continue;
            }
            if self.catalog_local && self.selkies.is_none() {
                // Asked again once the session's routes are back.
                continue;
            }
            if apps::icon_path(&icon).is_none() {
                self.icons.insert(icon, IconState::Failed);
                continue;
            }
            self.icons.insert(icon.clone(), IconState::Loading);
            asked |= self.icon_queue.want(&icon);
        }
        if asked {
            self.pump_icons(cx);
            cx.notify();
        }
    }

    fn pump_icons(&mut self, cx: &mut Context<Self>) {
        while let Some((icon, generation)) = self.icon_queue.next() {
            let fetcher = if self.catalog_local {
                self.selkies.clone().map(Fetcher::Local)
            } else {
                Some(Fetcher::Remote(self.plain.clone()))
            };
            let path = apps::icon_path(&icon);
            let svg = apps::is_svg(&icon);
            let decodes = Arc::clone(&self.decodes);
            let svg_stopped = Arc::clone(&self.svg_stopped);
            let pending = self.rt.spawn(async move {
                let (Some(fetcher), Some(path)) = (fetcher, path) else {
                    return None;
                };
                let bytes = match fetcher {
                    Fetcher::Remote(client) => {
                        let client = client?;
                        let url = apps::remote_icon_url(&path)?;
                        apps::fetch_capped(&client, url, apps::ICON_CAP, apps::ICON_TIMEOUT)
                            .await
                            .ok()?
                    }
                    Fetcher::Local(selkies) => selkies.apps_icon(&path).await.ok()?,
                };
                if svg && svg_stopped.load(Ordering::Relaxed) {
                    return None;
                }
                // The permit goes with the decode, so a decode the watchdog gave up on still
                // holds its slot until it really ends.
                let permit = decodes.acquire_owned().await.ok()?;
                // A watchdog may have tripped while this load waited for its slot.
                if svg && svg_stopped.load(Ordering::Relaxed) {
                    return None;
                }
                let decode = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    apps::decode_icon(&bytes, svg)
                });
                if svg {
                    match tokio::time::timeout(apps::SVG_WATCHDOG, decode).await {
                        Ok(decoded) => decoded.ok().flatten(),
                        Err(_) => {
                            svg_stopped.store(true, Ordering::Relaxed);
                            None
                        }
                    }
                } else {
                    decode.await.ok().flatten()
                }
            });
            cx.spawn(async move |this, cx| {
                let pixels = pending.await.ok().flatten();
                let _ = this.update(cx, |this, cx| {
                    if !this.icon_queue.finished(generation) {
                        // A source change or Retry since: nothing was made, nothing to free.
                        return;
                    }
                    let state = pixels
                        .and_then(render_image)
                        .map_or(IconState::Failed, IconState::Ready);
                    this.icons.insert(icon, state);
                    this.pump_icons(cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// The source changed or Retry was pressed: textures go to the retire queue (freed on
    /// the next render) and loads in flight are dropped when they land.
    fn reset_icons(&mut self) {
        for (_, state) in self.icons.drain() {
            if let IconState::Ready(image) = state {
                self.retired.push(image);
            }
        }
        self.icon_queue.reset();
    }

    fn icon(&self, app: &CatalogApp) -> (Option<Arc<RenderImage>>, bool) {
        match self.icons.get(&app.icon) {
            Some(IconState::Ready(image)) => (Some(Arc::clone(image)), false),
            Some(IconState::Loading | IconState::Failed) => (None, false),
            None => (None, !app.icon.is_empty()),
        }
    }

    // ---- Notices and actions ---------------------------------------------------------

    /// A failure stays until dismissed; information fades after 4 s (the Files rule).
    fn say(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.notice_generation += 1;
        let generation = self.notice_generation;
        self.notice = Some(Notice {
            text: text.into(),
            error,
        });
        if !error {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(4)).await;
                let _ = this.update(cx, |this, cx| {
                    if this.notice_generation == generation {
                        this.notice = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn handle(&mut self, action: AppsAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            AppsAction::Filter(id) => {
                let filter = match id {
                    FilterId::Installed => Filter::Installed,
                    FilterId::All => Filter::All,
                    FilterId::Category(category) => Filter::Category(category),
                };
                self.choose_filter(filter, window, cx);
            }
            AppsAction::Select(name) => {
                self.selected = Some(name);
                window.focus(&self.grid_focus, cx);
            }
            AppsAction::ClearSelection => {
                self.selected = None;
                window.focus(&self.grid_focus, cx);
            }
            AppsAction::Primary(name) => self.primary(&name, cx),
            AppsAction::Install(name) => self.request_job(Op::Install, &name, cx),
            AppsAction::Update(name) => self.request_job(Op::Update, &name, cx),
            AppsAction::Open(name) => self.launch(&name, cx),
            AppsAction::Remove(name) => self.confirm_remove(name, window, cx),
            AppsAction::Cancel(id) => self.cancel(id, cx),
            AppsAction::Clear(id) => {
                self.send(Op::Clear, None, Some(id), cx);
            }
            AppsAction::ClearFinished => {
                self.send(Op::Clear, None, None, cx);
            }
            AppsAction::CopyConsole => {
                let text = self
                    .selected
                    .as_deref()
                    .and_then(|app| self.board.job_for_app(app))
                    .map(apps::Job::console_text)
                    .unwrap_or_default();
                if !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    self.say("Copied the output.", false, cx);
                }
            }
            AppsAction::Retry => {
                if let Some(key) = self.catalog_key.clone() {
                    if cx.has_global::<AppsCatalogCache>() {
                        cx.global_mut::<AppsCatalogCache>().0.remove(&key);
                    }
                }
                self.reset_icons();
                self.load_catalog(true, cx);
            }
            AppsAction::DismissNotice => self.notice = None,
            AppsAction::ClearSearch => self.clear_search(window, cx),
            AppsAction::SelectActivity(app) => {
                if !self.search_text.is_empty() {
                    self.clear_search(window, cx);
                }
                let shown = self.shown().iter().any(|shown| shown.name == app);
                if !shown {
                    self.filter = Some(Filter::All);
                }
                self.selected = Some(app);
                self.reveal_selection = true;
            }
            AppsAction::IconsVisible(names) => {
                self.want_icons(names, cx);
                return;
            }
        }
        cx.notify();
    }

    // ---- Props -----------------------------------------------------------------------

    fn props(&mut self, window: &Window) -> AppsProps {
        let enabled = self.can_send();
        let width = window.viewport_size().width;
        let (source_width, inspector_width) = ui::pane_widths(width);
        self.columns = ui::columns(width - px(source_width + inspector_width + 2.));
        let installed = self.board.installed.as_ref();
        let apps = self.apps();
        let filter = self.filter.clone().unwrap_or(Filter::All);
        let installed_count =
            installed.map(|set| apps.iter().filter(|a| set.contains(&a.name)).count());
        let mut categories = vec![
            CategoryRow {
                id: FilterId::Installed,
                label: "Installed".into(),
                icon: gpui_kit::assets::IconName::PackageCheck,
                count: installed_count,
                selected: filter == Filter::Installed,
            },
            CategoryRow {
                id: FilterId::All,
                label: "All Apps".into(),
                icon: gpui_kit::assets::IconName::LayoutGrid,
                count: matches!(self.catalog, CatalogState::Ready(_)).then_some(apps.len()),
                selected: filter == Filter::All,
            },
        ];
        categories.extend(self.categories.iter().map(|category| {
            CategoryRow {
                id: FilterId::Category(category.clone()),
                label: apps::category_label(category),
                icon: category_icon(category),
                count: Some(
                    apps.iter()
                        .filter(|a| a.category.as_deref() == Some(category))
                        .count(),
                ),
                selected: filter == Filter::Category(category.clone()),
            }
        }));
        let tiles: Vec<AppTile> = self
            .shown()
            .into_iter()
            .map(|app| {
                let job = self.board.job_for_app(&app.name);
                let is_installed = self.board.is_installed(&app.name);
                let badge = match job.map(|job| job.state) {
                    Some(JobState::Running) => Badge::Running,
                    Some(JobState::Queued) => Badge::Queued,
                    Some(JobState::Failed) => Badge::Failed,
                    _ if is_installed => Badge::Installed,
                    _ => Badge::None,
                };
                let state_text = match (badge, job) {
                    (Badge::Running | Badge::Queued | Badge::Failed, Some(job)) => {
                        Some(apps::status_text(job, self.canceling.contains(&job.id)))
                    }
                    (Badge::Installed, _) => Some("Installed".to_owned()),
                    _ => None,
                };
                let (icon, want_icon) = self.icon(app);
                AppTile {
                    name: app.name.clone(),
                    full_name: app.full_name.clone(),
                    initial: initial(app),
                    icon,
                    want_icon,
                    badge,
                    progress: job.and_then(apps::download_progress),
                    tooltip: match state_text {
                        Some(state) => format!("{} — {state}", app.full_name),
                        None => app.full_name.clone(),
                    },
                    selected: self.selected.as_deref() == Some(app.name.as_str()),
                }
            })
            .collect();
        if std::mem::take(&mut self.reveal_selection) {
            if let Some(at) = tiles.iter().position(|tile| tile.selected) {
                self.grid_scroll
                    .scroll_to_item(at / self.columns.max(1), ScrollStrategy::Nearest);
            }
        }
        let inspector = self.inspector(enabled);
        let mut jobs: Vec<&apps::Job> = self.board.jobs.iter().collect();
        jobs.sort_by_key(|job| match job.state {
            JobState::Running => (0, job.id as i64),
            JobState::Queued => (1, job.id as i64),
            _ => (2, -(job.id as i64)),
        });
        let activity = jobs
            .into_iter()
            .map(|job| ActivityRow {
                id: job.id,
                app: job.app.clone(),
                full_name: self.full_name(&job.app),
                status: apps::status_text(job, self.canceling.contains(&job.id)),
                state: job_badge(job.state),
                progress: apps::download_progress(job),
                canceling: self.canceling.contains(&job.id),
            })
            .collect();
        let banner = if !self.link.connected() {
            Some(Banner::Waiting)
        } else if self.too_old {
            Some(Banner::TooOld)
        } else if !enabled {
            Some(Banner::CommandsDisabled)
        } else if self.state_seen && self.board.installed.is_none() {
            Some(Banner::InstalledUnknown)
        } else {
            None
        };
        AppsProps {
            categories,
            filter: filter_id(&filter),
            search: self.search_text.clone(),
            tiles: Rc::new(tiles),
            columns: self.columns,
            inspector,
            activity,
            catalog: match &self.catalog {
                CatalogState::Loading => CatalogView::Loading,
                CatalogState::Ready(_) => CatalogView::Ready,
                CatalogState::Failed(message) => CatalogView::Failed(message.clone()),
            },
            installed_count,
            notice: self.notice.clone(),
            banner,
            enabled,
            focused: window.is_window_active(),
            width,
        }
    }

    fn inspector(&mut self, enabled: bool) -> Option<Inspector> {
        let app = self.app(self.selected.as_deref()?)?.clone();
        let job = self.board.job_for_app(&app.name).cloned();
        let installed = self.board.is_installed(&app.name);
        let requested = self.requested.get(&app.name).map(|r| r.op);
        let state = apps::buttons(installed, job.as_ref(), requested, enabled);
        let canceling = job
            .as_ref()
            .is_some_and(|job| self.canceling.contains(&job.id));
        let pending_tooltip = match (&job, requested) {
            (Some(job), _) if job.pending() => Some(apps::status_text(job, canceling)),
            (_, Some(_)) => Some("Requested…".to_owned()),
            _ => None,
        };
        let console = job.map(|job| {
            let text = match &self.console_cache {
                Some((id, version, text)) if *id == job.id && *version == self.board_version => {
                    text.clone()
                }
                _ => {
                    let text = SharedString::from(job.console_text());
                    let switched = self
                        .console_cache
                        .as_ref()
                        .is_none_or(|(id, ..)| *id != job.id);
                    let offset = self.console_scroll.offset().y;
                    let max = self.console_scroll.max_offset().y;
                    // Follow the end unless the person scrolled up; another app starts there.
                    if switched || apps::follows_end(f32::from(offset), f32::from(max)) {
                        self.console_scroll.scroll_to_bottom();
                    }
                    self.console_cache = Some((job.id, self.board_version, text.clone()));
                    text
                }
            };
            ConsoleProps {
                job: job.id,
                status: apps::status_text(&job, canceling),
                state: job_badge(job.state),
                code: job.code,
                progress: apps::download_progress(&job),
                text,
                can_cancel: enabled && job.pending() && !canceling,
                can_clear: enabled && !job.pending(),
            }
        });
        let (icon, _) = self.icon(&app);
        Some(Inspector {
            initial: initial(&app),
            icon,
            installed,
            buttons: InspectorButtons {
                shown_installed: state.shown_installed,
                pending: state.pending.and_then(button_kind),
                pending_tooltip,
                enabled: state.enabled,
            },
            console,
            name: app.name,
            full_name: app.full_name,
            description: app.description,
        })
    }
}

impl Render for AppsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Free retired icon textures first (`ARCH-014`): GPUI never frees them on drop.
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        let theme = cx.theme().clone();
        let props = self.props(window);
        let entity = cx.entity();
        let handler: ui::AppsHandler = Rc::new(move |action, window, cx| {
            entity.update(cx, |this, cx| this.handle(action, window, cx));
        });
        gpui_kit::div()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_sm()
            .key_context("AppsWindow")
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.search
                    .update(cx, |search, cx| search.focus(window, cx));
            }))
            .on_action(cx.listener(|this, _: &ClearOrDeselect, window, cx| {
                if !this.search_text.is_empty() {
                    this.clear_search(window, cx);
                } else {
                    this.selected = None;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CloseApps, window, cx| {
                release(&this.owner, cx);
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| {
                let columns = this.columns.max(1) as isize;
                this.step(-columns, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| {
                let columns = this.columns.max(1) as isize;
                this.step(columns, cx);
            }))
            .on_action(cx.listener(|this, _: &PrimaryAction, _, cx| {
                if let Some(app) = this.selected.clone() {
                    this.primary(&app, cx);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &RemoveSelection, window, cx| {
                if let Some(app) = this.selected.clone() {
                    this.confirm_remove(app, window, cx);
                }
            }))
            .child(ui::render(
                &theme,
                &props,
                &self.search,
                &self.grid_scroll,
                &self.grid_focus,
                &self.console_scroll,
                handler,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_scoped_to_the_apps_window() {
        for binding in key_bindings() {
            let context = binding
                .predicate()
                .map(|predicate| predicate.to_string())
                .unwrap_or_default();
            assert!(
                context == "AppsWindow" || context == "AppsGrid",
                "{context:?} for {:?}",
                binding.action().name()
            );
        }
        let remove = key_bindings()
            .into_iter()
            .find(|binding| binding.action().name() == "apps_window::RemoveSelection")
            .expect("⌘⌫ removes");
        let keys = &remove.keystrokes()[0];
        assert_eq!(keys.key(), "backspace", "an alias for Remove…");
        assert!(keys.modifiers().platform);
    }

    #[test]
    fn categories_get_the_web_icons_and_a_generic_one() {
        use gpui_kit::assets::IconName as I;
        assert_eq!(category_icon("AudioVideo"), I::Clapperboard);
        assert_eq!(category_icon("WebBrowser"), I::Globe);
        assert_eq!(category_icon("Gardening"), I::Shapes);
    }

    #[test]
    fn a_failed_icon_tile_shows_the_initial() {
        let app = CatalogApp {
            name: "86box".into(),
            full_name: "86Box".into(),
            description: String::new(),
            icon: String::new(),
            category: None,
        };
        assert_eq!(initial(&app), '8');
        let firefox = CatalogApp {
            full_name: "firefox".into(),
            ..app
        };
        assert_eq!(initial(&firefox), 'F');
    }
}
