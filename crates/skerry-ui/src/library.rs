use crate::files::Notice;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    menu::{ContextMenuExt as _, PopupMenuItem},
    progress::Progress,
    scroll::ScrollableElement as _,
    spinner::Spinner,
    tooltip::Tooltip,
    Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{
    div, img, prelude::*, px, rgb, AnyElement, App, ClickEvent, Div, Entity, ExternalPaths,
    FontWeight, ObjectFit, RenderImage, Rgba, SharedString, Window,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

pub type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
pub type ItemHandler = Rc<dyn Fn(String, &mut Window, &mut App)>;
pub type FilterHandler = Rc<dyn Fn(Filter, &mut Window, &mut App)>;
pub type FlightHandler = Rc<dyn Fn(String, FlightAction, &mut Window, &mut App)>;
/// A chooser row or chip: its id and the chooser generation it was drawn for (`ARCH-028`).
pub type OpeningHandler = Rc<dyn Fn(String, u64, &mut Window, &mut App)>;
pub type ActionHandler = Rc<dyn Fn(&mut Window, &mut App)>;
pub type DropHandler = Rc<dyn Fn(Vec<PathBuf>, &mut Window, &mut App)>;
pub type SessionDropHandler = Rc<dyn Fn(String, Vec<PathBuf>, &mut Window, &mut App)>;

#[derive(Clone, Debug)]
pub struct AppItem {
    pub id: String,
    pub name: String,
    pub categories: Vec<String>,
    pub favorite: bool,
    pub hidden: bool,
    /// The app has remembered launch options: one-click launch is offered.
    pub remembered: bool,
    pub icon: Option<Arc<RenderImage>>,
}

#[derive(Clone, Debug)]
pub struct SessionItem {
    pub id: String,
    pub app_name: String,
    pub started_label: Option<String>,
    pub gpu: bool,
    pub storage_label: Option<String>,
    pub thumbnail: Option<Arc<RenderImage>>,
    pub app_icon: Option<Arc<RenderImage>>,
    /// Offers the Files button (not on a collaboration room, `ARCH-028`).
    pub files: bool,
    /// Takes Finder drops: they are copied to its Desktop.
    pub accepts_drop: bool,
}

/// An Applications chip. A category is its own variant, so a server category named "All"
/// or "Hidden" never collides with the built-in chips.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Filter {
    #[default]
    All,
    Category(String),
    Hidden,
}

impl Filter {
    fn element_id(&self) -> SharedString {
        match self {
            Self::All => "filter-All".into(),
            Self::Category(name) => format!("filter-cat-{name}").into(),
            Self::Hidden => "filter-Hidden".into(),
        }
    }
}

/// One launch in progress, as its card in Running sessions shows it.
#[derive(Clone, Debug)]
pub struct LaunchingItem {
    pub launch_id: String,
    pub app_name: String,
    pub app_icon: Option<Arc<RenderImage>>,
    pub stages: Vec<StageRow>,
    /// The server's sentence for its current stage.
    pub detail: Option<String>,
    /// "Launching · 0:42" or "Launch failed".
    pub status: String,
    pub notice: Option<LaunchNotice>,
    /// A file going up before the launch request (`/launch/file`, `ARCH-028`).
    pub upload: Option<UploadProgress>,
    /// What the launch opens: the link label or the file name, for the card's pill.
    pub subject: Option<(OpeningKind, String)>,
    /// Cancel shows (only while the file uploads; the launch request can't be taken back).
    pub cancellable: bool,
}

/// The upload row of a file launch.
#[derive(Clone, Debug, PartialEq)]
pub struct UploadProgress {
    pub fraction: f32,
    /// "Uploading 12 MB of 40 MB".
    pub label: String,
}

/// What the open chooser is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpeningKind {
    Link,
    File,
}

/// The "Open … in…" chooser under the omnibar (`ARCH-028`).
#[derive(Clone, Debug)]
pub struct OpeningProps {
    pub kind: OpeningKind,
    /// The link's label (the real host first) or the file's name.
    pub label: String,
    /// The full address or path.
    pub tooltip: Option<String>,
    /// "PDF · 2.4 MB · 1 of 3".
    pub detail: Option<String>,
    /// Clearly a link, but not one SealSkin takes: "Can't open …", no rows.
    pub rejected: bool,
    pub sessions: Vec<OpeningSessionRow>,
    pub apps: Vec<OpeningAppRow>,
    /// "Show All (n)" was clicked.
    pub apps_expanded: bool,
    /// More files wait behind this one: Skip shows and ✕ reads "Cancel All".
    pub queue_more: bool,
    /// The running row whose `open_url` is pending.
    pub pending_session: Option<String>,
    pub note: Option<Notice>,
    /// Return acts on the first row (a link typed in the omnibar): draw the ↩ hint.
    pub enter_hint: bool,
    /// The chooser generation these rows were built for; a click passes it back.
    pub generation: u64,
}

#[derive(Clone, Debug)]
pub struct OpeningSessionRow {
    pub id: String,
    pub app_name: String,
    /// "Started 5 min ago · auto-zen".
    pub detail: String,
    pub icon: Option<Arc<RenderImage>>,
}

#[derive(Clone, Debug)]
pub struct OpeningAppRow {
    pub id: String,
    pub name: String,
    pub icon: Option<Arc<RenderImage>>,
    /// The app is launching already (one launch per app): the chip is disabled.
    pub launching: bool,
}

/// The chooser shows this many app chips before "Show All (n)".
pub const OPENING_CHIPS: usize = 8;

/// The omnibar's leading icon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OmnibarIcon {
    #[default]
    Search,
    Link,
    Rejected,
}

/// Drop highlights drawn without a live drag, for the workbench and screenshots only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropPreview {
    Library,
    Session(String),
    FoldersOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageRow {
    pub label: String,
    pub time: Option<String>,
    pub state: StageState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageState {
    Done,
    Current,
    Pending,
    Failed,
    Stalled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchNotice {
    /// No change of stage for 3 minutes. Retry shows only after the request was lost.
    Stalled {
        message: String,
        hint: Option<String>,
        can_retry: bool,
    },
    Failed {
        headline: String,
        body: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightAction {
    KeepWaiting,
    Retry,
    Dismiss,
    /// Stops a file launch while its file uploads.
    Cancel,
}

#[derive(Clone, Debug, Default)]
pub struct LibraryProps {
    pub app_name: String,
    pub server: String,
    /// Built once per Library change and shared, never rebuilt per frame.
    pub apps: Rc<[AppItem]>,
    pub sessions: Rc<[SessionItem]>,
    /// Favorite app ids, oldest first; ids the server no longer lists are skipped.
    pub favorites: Vec<String>,
    /// Recently launched app ids, newest first.
    pub recent: Vec<String>,
    /// Launch cards, newest first. Rebuilt every render, apart from the cached items.
    pub launching: Vec<LaunchingItem>,
    /// The tile status line of each app launching now, by app id.
    pub launching_apps: HashMap<String, SharedString>,
    pub search_query: String,
    pub selected_filter: Filter,
    pub status: Option<String>,
    pub busy: Option<String>,
    /// The open chooser, when a link or file is waiting for a place to open.
    pub opening: Option<OpeningProps>,
    pub omnibar: OmnibarIcon,
    /// Sessions whose Files button is waiting for the cookie exchange.
    pub files_pending: HashSet<String>,
    /// Takes Finder drops (not while busy).
    pub accepts_drops: bool,
    pub drop_preview: Option<DropPreview>,
}

/// The widest the Library's content column grows; wider windows centre it.
pub const LIBRARY_MAX_WIDTH: f32 = 1200.0;
/// The most tiles the Recent row shows.
pub const RECENT_ROW_MAX: usize = 8;

pub struct LibraryHandlers {
    pub on_refresh: ClickHandler,
    /// Asks to sign out; the caller shows the confirmation.
    pub on_sign_out: ClickHandler,
    /// Opens the Launch Dialog for an app.
    pub on_launch: ItemHandler,
    /// Launches an app with its remembered options at once.
    pub on_quick_launch: ItemHandler,
    pub on_toggle_favorite: ItemHandler,
    pub on_toggle_hidden: ItemHandler,
    pub on_resume: ItemHandler,
    pub on_stop: Option<ItemHandler>,
    pub on_select_filter: FilterHandler,
    pub on_flight_action: FlightHandler,
    pub open: OpenHandlers,
}

/// The open chooser's, the drops', and the Files button's handlers (`ARCH-028`).
#[derive(Clone, Default)]
pub struct OpenHandlers {
    /// A running row: open the link there, or copy the file to its Desktop.
    pub on_open_in_session: Option<OpeningHandler>,
    /// An app chip: the Launch & Open dialog.
    pub on_open_with_app: Option<OpeningHandler>,
    /// ✕: the context and everything queued.
    pub on_clear_opening: Option<ActionHandler>,
    pub on_skip_opening: Option<ActionHandler>,
    pub on_toggle_opening_apps: Option<ActionHandler>,
    /// A session card's Files button.
    pub on_files: Option<ItemHandler>,
    /// Finder items dropped on the Library.
    pub on_drop: Option<DropHandler>,
    /// Finder items dropped on one session card.
    pub on_drop_to_session: Option<SessionDropHandler>,
}

/// What the Library shows, worked out from the props alone.
#[derive(Debug)]
pub struct Arrangement<'a> {
    pub favorites: Vec<&'a AppItem>,
    pub recent: Vec<&'a AppItem>,
    pub grid: Vec<&'a AppItem>,
    /// The chips: All, the categories, then Hidden when anything is hidden.
    pub filters: Vec<Filter>,
    /// The chip in effect; a selected chip no longer offered falls back to All.
    pub active: Filter,
    pub hidden_count: usize,
    pub sessions: Vec<&'a SessionItem>,
    /// Launch cards are progress, not results: a search never hides them.
    pub launching: &'a [LaunchingItem],
    /// A chooser shows: sections the filter leaves empty are hidden, not "no match".
    pub hide_empty: bool,
}

/// Favorites and Recent rows (only on All with an empty search), the grid, and the chips
/// (`ARCH-027`). Hidden apps leave All, the categories, and both rows, but a search still
/// finds them by name, after the visible matches.
pub fn arrange(props: &LibraryProps) -> Arrangement<'_> {
    let apps = &props.apps[..];
    let by_id: HashMap<&str, &AppItem> = apps.iter().map(|app| (app.id.as_str(), app)).collect();
    let hidden_count = apps.iter().filter(|app| app.hidden).count();
    let categories: BTreeSet<&str> = apps
        .iter()
        .filter(|app| !app.hidden)
        .flat_map(|app| app.categories.iter().map(String::as_str))
        .filter(|category| !category.is_empty())
        .collect();
    let mut filters = vec![Filter::All];
    filters.extend(
        categories
            .iter()
            .map(|category| Filter::Category(category.to_string())),
    );
    if hidden_count > 0 {
        filters.push(Filter::Hidden);
    }
    let active = if filters.contains(&props.selected_filter) {
        props.selected_filter.clone()
    } else {
        Filter::All
    };

    let query = props.search_query.trim().to_lowercase();
    let name_matches = |app: &AppItem| query.is_empty() || app.name.to_lowercase().contains(&query);
    let matches = |app: &AppItem| {
        name_matches(app)
            || app
                .categories
                .iter()
                .any(|category| category.to_lowercase().contains(&query))
    };

    let rows = active == Filter::All && query.is_empty();
    let listed = |ids: &[String]| -> Vec<&AppItem> {
        let mut seen = BTreeSet::new();
        ids.iter()
            .filter_map(|id| by_id.get(id.as_str()).copied())
            .filter(|app| !app.hidden && seen.insert(app.id.as_str()))
            .collect()
    };
    let (favorites, recent) = if rows {
        let favorites = listed(&props.favorites);
        let recent = listed(&props.recent)
            .into_iter()
            .filter(|app| !app.favorite)
            .take(RECENT_ROW_MAX)
            .collect();
        (favorites, recent)
    } else {
        (Vec::new(), Vec::new())
    };

    let grid = match &active {
        Filter::Hidden => apps
            .iter()
            .filter(|app| app.hidden && name_matches(app))
            .collect(),
        Filter::Category(category) => apps
            .iter()
            .filter(|app| !app.hidden && app.categories.contains(category) && matches(app))
            .collect(),
        Filter::All => {
            let mut grid: Vec<&AppItem> = apps
                .iter()
                .filter(|app| !app.hidden && matches(app))
                .collect();
            if !query.is_empty() {
                grid.extend(apps.iter().filter(|app| app.hidden && name_matches(app)));
            }
            grid
        }
    };

    let sessions = props
        .sessions
        .iter()
        .filter(|session| query.is_empty() || session.app_name.to_lowercase().contains(&query))
        .collect();

    Arrangement {
        favorites,
        recent,
        grid,
        filters,
        active,
        hidden_count,
        sessions,
        launching: &props.launching,
        hide_empty: props.opening.is_some(),
    }
}

/// Where a tile sits. The same app can show twice (Favorites and the grid), so each
/// placement has its own element id, and with it its own menu and hover state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Favorites,
    Recent,
    Grid,
}

impl Placement {
    fn compact(self) -> bool {
        self != Self::Grid
    }
}

pub fn tile_id(placement: Placement, app_id: &str) -> String {
    match placement {
        Placement::Favorites => format!("app-tile-fav-{app_id}"),
        Placement::Recent => format!("app-tile-recent-{app_id}"),
        Placement::Grid => format!("app-tile-{app_id}"),
    }
}

/// Creates a borderless omnibar input configured for searching apps or pasting URLs.
pub fn new_search_input(window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx).placeholder("Search applications and sessions, or paste a link")
    })
}

/// What every tile needs besides its app.
struct TileContext<'a> {
    theme: &'a Theme,
    launching: &'a HashMap<String, SharedString>,
    on_launch: ItemHandler,
    on_quick_launch: ItemHandler,
    on_toggle_favorite: ItemHandler,
    on_toggle_hidden: ItemHandler,
}

pub fn render(
    theme: &Theme,
    props: &LibraryProps,
    search_input: &Entity<InputState>,
    handlers: LibraryHandlers,
) -> Div {
    let enabled = props.busy.is_none();
    let LibraryHandlers {
        on_refresh,
        on_sign_out,
        on_launch,
        on_quick_launch,
        on_toggle_favorite,
        on_toggle_hidden,
        on_resume,
        on_stop,
        on_select_filter,
        on_flight_action,
        open,
    } = handlers;
    let arranged = arrange(props);
    let tiles = TileContext {
        theme,
        launching: &props.launching_apps,
        on_launch,
        on_quick_launch,
        on_toggle_favorite,
        on_toggle_hidden,
    };
    let server = props.server.clone();
    let header = div()
        .h_flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .flex_1()
                .h_flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .flex_none()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(props.app_name.clone()),
                )
                .child(
                    div()
                        .id("server-chip")
                        .min_w_0()
                        .overflow_hidden()
                        .h_flex()
                        .items_center()
                        .gap_1p5()
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .bg(theme.secondary)
                        .border_1()
                        .border_color(theme.border)
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(div().flex_none().child(IconName::Globe))
                        .child(div().min_w_0().truncate().child(props.server.clone()))
                        .tooltip(move |window, cx| Tooltip::new(server.clone()).build(window, cx)),
                ),
        )
        .child(
            div()
                .flex_none()
                .h_flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new("refresh")
                        .outline()
                        .xsmall()
                        .icon(IconName::RefreshCw)
                        .label("Refresh")
                        .disabled(!enabled)
                        .on_click(move |event, window, cx| on_refresh(event, window, cx)),
                )
                .child(
                    Button::new("sign-out")
                        .ghost()
                        .xsmall()
                        .icon(gpui_kit::assets::IconName::LogOut)
                        .label("Sign Out…")
                        .tooltip("Sign out of this server")
                        .disabled(!enabled)
                        .on_click(move |event, window, cx| on_sign_out(event, window, cx)),
                ),
        );

    let omnibar = div()
        .h_flex()
        .items_center()
        .gap_2()
        .px_3p5()
        .py_2()
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .child(match props.omnibar {
            OmnibarIcon::Search => div()
                .text_color(theme.muted_foreground)
                .child(Icon::new(IconName::Search)),
            OmnibarIcon::Link => div()
                .text_color(theme.foreground)
                .child(Icon::new(gpui_kit::assets::IconName::Link)),
            OmnibarIcon::Rejected => div()
                .text_color(theme.muted_foreground)
                .child(Icon::new(gpui_kit::assets::IconName::Ban)),
        })
        .child(
            div().flex_1().min_w_0().child(
                Input::new(search_input)
                    .appearance(false)
                    .cleanable(true)
                    .disabled(!enabled),
            ),
        );

    let hide_sessions =
        arranged.hide_empty && arranged.sessions.is_empty() && arranged.launching.is_empty();
    let sessions_section = div()
        .when(hide_sessions, |section| section.hidden())
        .v_flex()
        .gap_3p5()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Running sessions"),
        )
        .when(
            arranged.sessions.is_empty() && arranged.launching.is_empty(),
            |section| {
                let message = if props.sessions.is_empty() {
                    "No running sessions. Launch an application below to start one."
                } else {
                    "No running sessions match your search."
                };
                section.child(
                    div()
                        .p_4()
                        .rounded_xl()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.popover)
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(message),
                )
            },
        )
        .when(
            !arranged.sessions.is_empty() || !arranged.launching.is_empty(),
            |section| {
                section.child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .items_stretch()
                        .gap_4()
                        .children(arranged.launching.iter().map(|item| {
                            render_launching_card(theme, item, Rc::clone(&on_flight_action))
                        }))
                        .children(arranged.sessions.iter().map(|session| {
                            render_session_card(
                                theme,
                                session,
                                enabled,
                                Rc::clone(&on_resume),
                                on_stop.clone(),
                                &open,
                                props,
                            )
                        })),
                )
            },
        );

    let app_row = |title: &'static str, apps: &[&AppItem], placement: Placement| {
        (!apps.is_empty()).then(|| {
            div()
                .v_flex()
                .gap_3p5()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div().h_flex().flex_wrap().gap_3().children(
                        apps.iter()
                            .map(|app| render_app_tile(&tiles, app, placement)),
                    ),
                )
        })
    };
    let favorites_section = app_row("Favorites", &arranged.favorites, Placement::Favorites);
    let recent_section = app_row("Recent", &arranged.recent, Placement::Recent);

    let filter_bar =
        div()
            .h_flex()
            .items_center()
            .flex_wrap()
            .gap_2()
            .children(arranged.filters.iter().map(|filter| {
                let selected = *filter == arranged.active;
                let on_select = Rc::clone(&on_select_filter);
                let value = filter.clone();
                let label = match filter {
                    Filter::All => "All".to_string(),
                    Filter::Category(name) => name.clone(),
                    Filter::Hidden => format!("Hidden ({})", arranged.hidden_count),
                };
                let button = Button::new(filter.element_id())
                    .xsmall()
                    .rounded_full()
                    .label(label)
                    .on_click(move |_, window, cx| on_select(value.clone(), window, cx));
                if selected {
                    button.primary()
                } else {
                    button.outline()
                }
            }));

    let showing_hidden = arranged.active == Filter::Hidden;
    let hide_apps = arranged.hide_empty && arranged.grid.is_empty() && !showing_hidden;
    let apps_section = div()
        .when(hide_apps, |section| section.hidden())
        .v_flex()
        .gap_3p5()
        .child(
            div()
                .h_flex()
                .items_center()
                .flex_wrap()
                .gap_3()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Applications"),
                )
                .child(filter_bar),
        )
        .when(showing_hidden, |section| {
            section.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(if arranged.grid.is_empty() {
                        "No hidden applications match your search."
                    } else {
                        "Hidden apps stay out of the Library. Right-click one and choose Show in Library."
                    }),
            )
        })
        .when(arranged.grid.is_empty() && !showing_hidden, |section| {
            let empty_message = if props.apps.is_empty() {
                "No applications are available for this account."
            } else {
                "No applications match your search or filter."
            };
            section.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(empty_message),
            )
        })
        .when(!arranged.grid.is_empty(), |section| {
            section.child(
                div().h_flex().flex_wrap().gap_4().children(
                    arranged
                        .grid
                        .iter()
                        .map(|app| render_app_tile(&tiles, app, Placement::Grid)),
                ),
            )
        });
    let chooser = props
        .opening
        .as_ref()
        .map(|opening| render_chooser(theme, opening, &open, enabled));
    let drops = props.accepts_drops && enabled;
    let droppable: Vec<String> = arranged
        .sessions
        .iter()
        .filter(|session| session.accepts_drop)
        .map(|session| session.id.clone())
        .collect();
    let overlay =
        drops.then(|| render_drop_overlay(theme, props.drop_preview.as_ref(), &droppable));
    let on_drop = open.on_drop.clone().filter(|_| drops);
    div()
        .size_full()
        .relative()
        .bg(theme.background)
        .text_color(theme.foreground)
        .when_some(on_drop, |root, on_drop| {
            root.on_drop(move |paths: &ExternalPaths, window, cx| {
                on_drop(paths.paths().to_vec(), window, cx)
            })
        })
        .child(
            div()
                .id("library")
                .size_full()
                .overflow_y_scrollbar()
                .child(
                    div().w_full().px_7().py_5().child(
                        // One centred column, capped on content width; the scrollbar stays at
                        // the window edge.
                        div()
                            .w_full()
                            .max_w(px(LIBRARY_MAX_WIDTH))
                            .mx_auto()
                            .v_flex()
                            .gap_6()
                            .child(header)
                            .child(omnibar)
                            .children(chooser)
                            .when_some(props.busy.clone(), |body, busy| {
                                body.child(
                                    div()
                                        .px_3p5()
                                        .py_2()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(theme.border)
                                        .bg(theme.secondary)
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .child(busy),
                                )
                            })
                            .when_some(props.status.clone(), |body, status| {
                                body.child(
                                    div()
                                        .px_3p5()
                                        .py_2()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(theme.border)
                                        .bg(theme.popover)
                                        .text_sm()
                                        .text_color(theme.danger)
                                        .child(status),
                                )
                            })
                            .child(sessions_section)
                            .children(favorites_section)
                            .children(recent_section)
                            .child(apps_section),
                    ),
                ),
        )
        .children(overlay)
}

fn render_session_card(
    theme: &Theme,
    session: &SessionItem,
    enabled: bool,
    on_resume: ItemHandler,
    on_stop: Option<ItemHandler>,
    open: &OpenHandlers,
    props: &LibraryProps,
) -> Div {
    let session_id = session.id.clone();
    let files_pending = props.files_pending.contains(&session.id);
    let on_files = open.on_files.clone().filter(|_| session.files);
    let on_drop = open
        .on_drop_to_session
        .clone()
        .filter(|_| session.accepts_drop && props.accepts_drops && enabled);
    let previewed = props.drop_preview == Some(DropPreview::Session(session.id.clone()));
    let preview = div()
        .h(px(172.))
        .w_full()
        .relative()
        .bg(theme.secondary)
        .overflow_hidden()
        .child(if let Some(thumb) = session.thumbnail.clone() {
            img(thumb)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element()
        } else {
            div()
                .size_full()
                .v_flex()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(theme.muted_foreground)
                .child(IconName::LayoutDashboard)
                .child(div().text_xs().child("Active session"))
                .into_any_element()
        })
        .child(div().absolute().bottom_2p5().left_2p5().child(app_badge(
            &session.app_name,
            session.app_icon.clone(),
            px(32.),
        )));

    let mut meta = div().h_flex().flex_wrap().gap_1p5();
    if session.gpu {
        meta = meta.child(meta_pill(theme, Some(IconName::Cpu), "GPU"));
    }
    if let Some(storage) = &session.storage_label {
        meta = meta.child(meta_pill(theme, Some(IconName::HardDrive), storage));
    }

    let has_meta = session.gpu || session.storage_label.is_some();

    let footer = div()
        .p_3p5()
        .border_t_1()
        .border_color(theme.border)
        .h_flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .v_flex()
                .gap_1()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(session.app_name.clone()),
                )
                .when_some(session.started_label.clone(), |col, started| {
                    col.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(started),
                    )
                })
                .when(has_meta, |col| col.child(div().pt_1().child(meta))),
        )
        .child(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .flex_none()
                .child(
                    Button::new(SharedString::from(format!("open-{session_id}")))
                        .primary()
                        .small()
                        .label("Open")
                        .disabled(!enabled)
                        .on_click(move |_, window, cx| on_resume(session_id.clone(), window, cx)),
                )
                .when_some(on_files, |actions, on_files| {
                    let files_id = session.id.clone();
                    actions.child(
                        Button::new(SharedString::from(format!("files-{files_id}")))
                            .outline()
                            .small()
                            .icon(IconName::FolderOpen)
                            .loading(files_pending)
                            .tooltip("Files: show this session’s Desktop files")
                            .disabled(!enabled || files_pending)
                            .on_click(move |_, window, cx| on_files(files_id.clone(), window, cx)),
                    )
                })
                .when_some(on_stop, |actions, on_stop| {
                    let stop_id = session.id.clone();
                    actions.child(
                        Button::new(SharedString::from(format!("stop-{stop_id}")))
                            .outline()
                            .small()
                            .icon(IconName::Square)
                            .label("Stop")
                            .tooltip("Stop Session…")
                            .disabled(!enabled)
                            .on_click(move |_, window, cx| on_stop(stop_id.clone(), window, cx)),
                    )
                }),
        );

    let group = SharedString::from(session_drop_group(&session.id));
    let ring = theme.blue;
    let card = div()
        .w(px(336.))
        .relative()
        .rounded_xl()
        .overflow_hidden()
        .border_1()
        .border_color(if previewed { ring } else { theme.border })
        .bg(theme.popover)
        .v_flex()
        .child(preview)
        // Fill the row height so cards in one row line up even when one card's pills wrap.
        .child(footer.flex_1());
    let Some(on_drop) = on_drop else {
        return card;
    };
    let session_id = session.id.clone();
    card.group(group)
        .drag_over::<ExternalPaths>(move |style, _, _, _| style.border_color(ring))
        .on_drop(move |paths: &ExternalPaths, window, cx| {
            // The Library's own drop must not open the chooser too.
            cx.stop_propagation();
            on_drop(session_id.clone(), paths.paths().to_vec(), window, cx)
        })
        .child(session_drop_overlay(theme, &session.app_name, previewed))
}

/// The group a droppable session card carries, so the Library's pill can hide over it.
fn session_drop_group(session_id: &str) -> String {
    format!("session-drop-{session_id}")
}

/// "Copy to <App> Desktop" over a session card while Finder items hover it.
fn session_drop_overlay(theme: &Theme, app_name: &str, previewed: bool) -> Div {
    let overlay = div()
        .absolute()
        .inset_0()
        .rounded_xl()
        .border_2()
        .border_color(theme.blue)
        .bg(theme.drop_target)
        .h_flex()
        .items_center()
        .justify_center()
        .child(drop_pill(
            theme,
            gpui_kit::assets::IconName::FolderUp,
            div()
                .h_flex()
                .min_w_0()
                .child("Copy to ")
                .child(div().max_w(px(140.)).truncate().child(app_name.to_string()))
                .child(" Desktop"),
        ));
    if previewed {
        overlay
    } else {
        // Shown only while the drag is over this card; the drag_over gives it a hitbox
        // that doesn't block anything beneath.
        overlay
            .invisible()
            .drag_over::<ExternalPaths>(|style, _, _, _| style.visible())
    }
}

fn drop_pill(theme: &Theme, icon: gpui_kit::assets::IconName, text: impl IntoElement) -> Div {
    div()
        .h_flex()
        .items_center()
        .gap_2()
        .px_4()
        .py_2()
        .rounded_full()
        .bg(theme.popover)
        .border_1()
        .border_color(theme.border)
        .shadow_md()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.foreground)
        .child(Icon::new(icon))
        .child(text)
}

/// Whether every dropped item is a folder (`launch/file` takes files).
fn folders_only(paths: &ExternalPaths) -> bool {
    let paths = paths.paths();
    !paths.is_empty() && paths.iter().all(|path| path.is_dir())
}

/// The Library's drop highlight: a ring inset 8 pt over the visible area and a pill. Stateless:
/// it shows through `drag_over` while Finder items hover the window, and the pill hides over
/// a droppable session card, whose own message shows instead.
fn render_drop_overlay(theme: &Theme, preview: Option<&DropPreview>, droppable: &[String]) -> Div {
    let muted = theme.muted_foreground;
    let mut pill = drop_pill(
        theme,
        gpui_kit::assets::IconName::FileUp,
        "Drop to open in a remote app",
    )
    .drag_over::<ExternalPaths>(|style, _, _, _| style);
    for id in droppable {
        pill = pill
            .group_drag_over::<ExternalPaths>(session_drop_group(id), |style| style.invisible());
    }
    let previewing_session = matches!(preview, Some(DropPreview::Session(_)));
    let overlay = div()
        .absolute()
        .inset(px(8.))
        .rounded_xl()
        .border_2()
        .border_color(theme.blue)
        .bg(theme.drop_target)
        .flex()
        .items_center()
        .justify_center()
        .child(pill.when(previewing_session, |pill| pill.invisible()));
    match preview {
        Some(DropPreview::FoldersOnly) => overlay
            .border_color(muted)
            .bg(gpui_kit::transparent_black()),
        Some(_) => overlay,
        None => overlay
            .invisible()
            .drag_over::<ExternalPaths>(move |style, paths, _, _| {
                if folders_only(paths) {
                    style
                        .visible()
                        .border_color(muted)
                        .bg(gpui_kit::transparent_black())
                } else {
                    style.visible()
                }
            }),
    }
}

/// The "Open … in…" chooser (`ARCH-028`): the context, then the running sessions that can
/// take it, then the apps that can launch with it. Rows pass their generation back, so a
/// click drawn for an older context is ignored.
fn render_chooser(
    theme: &Theme,
    opening: &OpeningProps,
    open: &OpenHandlers,
    enabled: bool,
) -> Div {
    let pending = opening.pending_session.is_some();
    let generation = opening.generation;
    let icon = if opening.rejected {
        Icon::new(gpui_kit::assets::IconName::Ban).text_color(theme.muted_foreground)
    } else {
        match opening.kind {
            OpeningKind::Link => Icon::new(gpui_kit::assets::IconName::Link),
            OpeningKind::File => Icon::new(IconName::FileText),
        }
    };
    let name = match opening.kind {
        OpeningKind::File => format!("“{}”", opening.label),
        OpeningKind::Link => opening.label.clone(),
    };
    let tooltip = opening.tooltip.clone();
    let title = div()
        .flex_1()
        .min_w_0()
        .h_flex()
        .items_center()
        .gap_1()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .child(div().flex_none().child(if opening.rejected {
            "Can’t open"
        } else {
            "Open"
        }))
        .child(
            div()
                .id("opening-label")
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .child(name)
                .when_some(tooltip, |label, tooltip| {
                    label.tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                }),
        )
        .when(!opening.rejected, |title| {
            title.child(div().flex_none().child("in…"))
        });
    let on_skip = open.on_skip_opening.clone();
    let on_clear = open.on_clear_opening.clone();
    let header = div()
        .h_flex()
        .items_center()
        .gap_2()
        .child(div().flex_none().child(icon.with_size(px(20.))))
        .child(title)
        .when_some(opening.detail.clone(), |header, detail| {
            header.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(detail),
            )
        })
        .when_some(on_skip.filter(|_| opening.queue_more), |header, on_skip| {
            header.child(
                Button::new("opening-skip")
                    .ghost()
                    .xsmall()
                    .label("Skip")
                    .tooltip("Skip this file")
                    .on_click(move |_, window, cx| on_skip(window, cx)),
            )
        })
        .when_some(on_clear, |header, on_clear| {
            header.child(
                Button::new("opening-cancel")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip(if opening.queue_more {
                        "Cancel All"
                    } else {
                        "Cancel"
                    })
                    .on_click(move |_, window, cx| on_clear(window, cx)),
            )
        });

    let group_label = |label: &'static str| {
        div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.muted_foreground)
            .child(label)
    };
    let (running_label, action_label) = match opening.kind {
        OpeningKind::Link => ("Open in a running session", "Open Here"),
        OpeningKind::File => ("Copy to a running session", "Copy to Desktop"),
    };
    let key_hint = || {
        div()
            .flex_none()
            .px_1()
            .rounded_sm()
            .border_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child("↩")
    };
    let running = (!opening.sessions.is_empty()).then(|| {
        let rows = opening.sessions.iter().enumerate().map(|(index, row)| {
            let this_pending = opening.pending_session.as_deref() == Some(row.id.as_str());
            let usable = enabled && !pending;
            let on_open = open.on_open_in_session.clone();
            let row_id = row.id.clone();
            let button_id = row.id.clone();
            let on_button = open.on_open_in_session.clone();
            let file_tooltip = (opening.kind == OpeningKind::File).then(|| {
                format!(
                    "Copy “{}” to this session’s Desktop and show its files",
                    opening.label
                )
            });
            let mut button = Button::new(SharedString::from(format!("opening-run-btn-{}", row.id)))
                .outline()
                .small()
                .disabled(!usable)
                .when_some(on_button, |button, on_open| {
                    button.on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        on_open(button_id.clone(), generation, window, cx)
                    })
                });
            button = if this_pending {
                button.loading(true).label("Opening…")
            } else {
                button.label(action_label)
            };
            if let Some(tooltip) = file_tooltip {
                button = button.tooltip(tooltip);
            }
            div()
                .id(SharedString::from(format!("opening-run-{}", row.id)))
                .h(px(44.))
                .px_2()
                .h_flex()
                .items_center()
                .gap_2p5()
                .when(index > 0, |row| row.border_t_1().border_color(theme.border))
                .when(pending && !this_pending, |row| row.opacity(0.6))
                .when(usable, |row| {
                    row.cursor_pointer().hover(|s| s.bg(theme.secondary))
                })
                .when_some(on_open.filter(|_| usable), |row, on_open| {
                    row.on_click(move |_, window, cx| {
                        on_open(row_id.clone(), generation, window, cx)
                    })
                })
                .child(app_badge(&row.app_name, row.icon.clone(), px(24.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_flex()
                        .items_baseline()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(200.))
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(row.app_name.clone()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(row.detail.clone()),
                        ),
                )
                .when(index == 0 && opening.enter_hint, |row| {
                    row.child(key_hint())
                })
                .child(button)
        });
        div()
            .v_flex()
            .gap_1p5()
            .child(group_label(running_label))
            .child(
                div()
                    .v_flex()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .children(rows),
            )
    });

    let apps = (!opening.apps.is_empty()).then(|| {
        let shown = if opening.apps_expanded {
            opening.apps.len()
        } else {
            opening.apps.len().min(OPENING_CHIPS)
        };
        let hint_first = opening.enter_hint && opening.sessions.is_empty();
        let chips = opening.apps[..shown]
            .iter()
            .enumerate()
            .map(|(index, app)| {
                let usable = enabled && !pending && !app.launching;
                let on_open = open.on_open_with_app.clone();
                let app_id = app.id.clone();
                let border_hover = theme.primary;
                let bg_hover = theme.secondary;
                let mut chip = div()
                    .id(SharedString::from(format!("opening-app-{}", app.id)))
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2p5()
                    .py_1p5()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .child(app_badge(&app.name, app.icon.clone(), px(20.)))
                    .child(
                        div()
                            .max_w(px(160.))
                            .truncate()
                            .text_sm()
                            .child(app.name.clone()),
                    )
                    .when(index == 0 && hint_first, |chip| chip.child(key_hint()));
                if usable {
                    chip = chip
                        .cursor_pointer()
                        .hover(move |s| s.border_color(border_hover).bg(bg_hover))
                        .when_some(on_open, |chip, on_open| {
                            chip.on_click(move |_, window, cx| {
                                on_open(app_id.clone(), generation, window, cx)
                            })
                        });
                } else {
                    chip = chip.opacity(0.6);
                }
                if app.launching {
                    let tooltip = format!("{} is already launching", app.name);
                    chip = chip
                        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx));
                }
                chip
            });
        let more = opening.apps.len() > OPENING_CHIPS;
        let on_toggle = open.on_toggle_opening_apps.clone();
        div()
            .v_flex()
            .gap_1p5()
            .child(group_label("Launch a new session"))
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(chips)
                    .when_some(on_toggle.filter(|_| more), |row, on_toggle| {
                        row.child(
                            Button::new("opening-show-all")
                                .ghost()
                                .small()
                                .label(if opening.apps_expanded {
                                    "Show Fewer".to_string()
                                } else {
                                    format!("Show All ({})", opening.apps.len())
                                })
                                .on_click(move |_, window, cx| on_toggle(window, cx)),
                        )
                    }),
            )
    });

    let note = opening.note.clone().map(|note| {
        let (icon, tint) = if note.error {
            (IconName::CircleX, theme.danger)
        } else {
            (IconName::Info, theme.muted_foreground)
        };
        div()
            .h_flex()
            .items_center()
            .gap_2()
            .text_sm()
            .when(note.error, |row| {
                row.px_2p5()
                    .py_1p5()
                    .rounded_md()
                    .bg(theme.danger.opacity(0.1))
            })
            .child(Icon::new(icon).small().text_color(tint))
            .child(div().flex_1().min_w_0().text_color(tint).child(note.text))
    });

    div()
        .w_full()
        .p_4()
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .v_flex()
        .gap_3()
        .child(header)
        .children(running)
        .children(apps)
        .children(note)
}

/// An app tile: regular (156 × 142) in the grid, compact (120 × 112) in Favorites and
/// Recent. A click opens the Launch Dialog; Option-click or the hover ▶ launches with the
/// remembered options; right-click opens the tile menu.
fn render_app_tile(ctx: &TileContext, app: &AppItem, placement: Placement) -> AnyElement {
    let theme = ctx.theme;
    let compact = placement.compact();
    let app_id = app.id.clone();
    let launching = ctx.launching.get(&app.id).cloned();
    // Tiles stay usable during a Refresh or Stop: a launch no longer locks the Library.
    let interactive = launching.is_none();
    let remembered = app.remembered;
    let id = tile_id(placement, &app.id);
    let group = SharedString::from(format!("tile-{id}"));
    let border_hover = theme.primary;
    let bg_hover = theme.secondary;

    let status_line = if let Some(stage) = launching.clone() {
        Some(
            div()
                .h(px(16.))
                .h_flex()
                .items_center()
                .gap_1()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(Spinner::new().xsmall())
                .child(stage),
        )
    } else if app.hidden {
        Some(
            div()
                .h(px(16.))
                .h_flex()
                .items_center()
                .gap_1()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(Icon::new(IconName::EyeOff).with_size(px(12.)))
                .child("Hidden"),
        )
    } else {
        None
    };

    let mut tile = div()
        .id(SharedString::from(id.clone()))
        .group(group.clone())
        .relative()
        .w(px(if compact { 120. } else { 156. }))
        .h(px(if compact { 112. } else { 142. }))
        .when(compact, |tile| tile.p_3().gap_2())
        .when(!compact, |tile| tile.p_4().gap_3())
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .v_flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .when(app.hidden, |icon| icon.opacity(0.55))
                .child(app_badge(
                    &app.name,
                    app.icon.clone(),
                    px(if compact { 44. } else { 56. }),
                )),
        )
        .child(
            div()
                .w_full()
                .v_flex()
                .items_center()
                .when(compact, |name| name.gap_0p5())
                .when(!compact, |name| name.gap_1())
                .child(
                    div()
                        .w_full()
                        .text_center()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(app.name.clone()),
                )
                .children(status_line),
        );

    if interactive {
        let on_launch = Rc::clone(&ctx.on_launch);
        let on_quick = Rc::clone(&ctx.on_quick_launch);
        let clicked = app_id.clone();
        tile = tile
            .cursor_pointer()
            .hover(move |s| s.border_color(border_hover).bg(bg_hover))
            .on_click(move |event, window, cx| {
                // Keyboard clicks report no modifiers, so Option+Enter isn't quick launch.
                if remembered && event.modifiers().alt {
                    on_quick(clicked.clone(), window, cx);
                } else {
                    on_launch(clicked.clone(), window, cx);
                }
            });
        if remembered {
            tile = tile.tooltip(|window, cx| {
                Tooltip::new("Option-click to launch with your last options").build(window, cx)
            });
        }
    } else {
        tile = tile.opacity(0.6);
    }

    if app.favorite && placement != Placement::Favorites {
        tile = tile.child(
            div().absolute().top_2().left_2().child(
                Icon::new(IconName::StarFill)
                    .with_size(px(12.))
                    .text_color(theme.warning),
            ),
        );
    }

    if remembered && interactive {
        let on_quick = Rc::clone(&ctx.on_quick_launch);
        let quick = app_id.clone();
        tile = tile.child(
            div()
                .absolute()
                .top_2()
                .right_2()
                .invisible()
                .group_hover(group, |style| style.visible())
                .child(
                    Button::new(SharedString::from(format!("quick-{id}")))
                        .primary()
                        .xsmall()
                        .rounded_full()
                        .icon(IconName::Play)
                        .tooltip("Launch with Last Options")
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            on_quick(quick.clone(), window, cx);
                        }),
                ),
        );
    }

    let menu = TileMenu {
        app_id,
        remembered,
        launching: launching.is_some(),
        favorite: app.favorite,
        hidden: app.hidden,
        on_launch: Rc::clone(&ctx.on_launch),
        on_quick_launch: Rc::clone(&ctx.on_quick_launch),
        on_toggle_favorite: Rc::clone(&ctx.on_toggle_favorite),
        on_toggle_hidden: Rc::clone(&ctx.on_toggle_hidden),
    };
    tile.context_menu(move |popup, _, _| menu.build(popup))
        .into_any_element()
}

/// The tile's right-click menu.
struct TileMenu {
    app_id: String,
    remembered: bool,
    launching: bool,
    favorite: bool,
    hidden: bool,
    on_launch: ItemHandler,
    on_quick_launch: ItemHandler,
    on_toggle_favorite: ItemHandler,
    on_toggle_hidden: ItemHandler,
}

impl TileMenu {
    fn build(
        &self,
        mut popup: gpui_kit::component::menu::PopupMenu,
    ) -> gpui_kit::component::menu::PopupMenu {
        let item = |label: &'static str, handler: &ItemHandler, app_id: &str| {
            let handler = Rc::clone(handler);
            let app_id = app_id.to_string();
            PopupMenuItem::new(label)
                .on_click(move |_, window, cx| handler(app_id.clone(), window, cx))
        };
        if self.remembered {
            popup = popup.item(
                item(
                    "Launch with Last Options",
                    &self.on_quick_launch,
                    &self.app_id,
                )
                .icon(IconName::Play)
                .disabled(self.launching),
            );
        }
        popup
            .item(item("Launch Options…", &self.on_launch, &self.app_id).disabled(self.launching))
            .separator()
            .item(
                item(
                    if self.favorite {
                        "Remove from Favorites"
                    } else {
                        "Add to Favorites"
                    },
                    &self.on_toggle_favorite,
                    &self.app_id,
                )
                .icon(if self.favorite {
                    IconName::StarOff
                } else {
                    IconName::Star
                }),
            )
            .item(
                item(
                    if self.hidden {
                        "Show in Library"
                    } else {
                        "Hide from Library"
                    },
                    &self.on_toggle_hidden,
                    &self.app_id,
                )
                .icon(if self.hidden {
                    IconName::Eye
                } else {
                    IconName::EyeOff
                }),
            )
    }
}

/// A launch in progress, first in Running sessions: the stage checklist, the server's
/// sentence, and (stalled or failed) a notice with its actions.
fn render_launching_card(
    theme: &Theme,
    item: &LaunchingItem,
    on_action: FlightHandler,
) -> gpui_kit::Stateful<Div> {
    let failed = matches!(item.notice, Some(LaunchNotice::Failed { .. }));
    let rows = item.stages.iter().map(|row| {
        let glyph: AnyElement = match row.state {
            StageState::Done => Icon::new(IconName::CircleCheck)
                .with_size(px(16.))
                .text_color(theme.muted_foreground)
                .into_any_element(),
            StageState::Current => Spinner::new().xsmall().into_any_element(),
            StageState::Pending => div()
                .opacity(0.5)
                .child(
                    Icon::new(gpui_kit::assets::IconName::Circle)
                        .with_size(px(16.))
                        .text_color(theme.muted_foreground),
                )
                .into_any_element(),
            StageState::Failed => Icon::new(IconName::CircleX)
                .with_size(px(16.))
                .text_color(theme.danger)
                .into_any_element(),
            StageState::Stalled => Icon::new(IconName::TriangleAlert)
                .with_size(px(16.))
                .text_color(theme.warning)
                .into_any_element(),
        };
        let (color, strong) = match row.state {
            StageState::Current | StageState::Stalled => (theme.foreground, true),
            StageState::Failed => (theme.danger, false),
            StageState::Done | StageState::Pending => (theme.muted_foreground, false),
        };
        div()
            .h(px(24.))
            .h_flex()
            .items_center()
            .gap_2()
            .text_sm()
            .child(
                div()
                    .size(px(16.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(glyph),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(color)
                    .when(strong, |label| label.font_weight(FontWeight::SEMIBOLD))
                    .child(row.label.clone()),
            )
            .child(
                div()
                    .w(px(44.))
                    .flex_none()
                    .text_right()
                    .text_color(theme.muted_foreground)
                    .children(row.time.clone()),
            )
    });

    let id = item.launch_id.clone();
    let notice = item.notice.as_ref().map(|notice| {
        let button = |label: &'static str, action: FlightAction, suffix: &str| {
            let on_action = Rc::clone(&on_action);
            let id = id.clone();
            Button::new(SharedString::from(format!("flight-{suffix}-{id}")))
                .small()
                .label(label)
                .on_click(move |_, window, cx| on_action(id.clone(), action, window, cx))
        };
        let (message, body, message_color, buttons): (String, Option<String>, _, Vec<Button>) =
            match notice {
                LaunchNotice::Stalled {
                    message,
                    hint,
                    can_retry,
                } => {
                    let mut buttons =
                        vec![button("Keep Waiting", FlightAction::KeepWaiting, "keep")
                            .primary()
                            .tooltip("Hide this for another 3 minutes")];
                    if *can_retry {
                        buttons.push(
                            button("Retry", FlightAction::Retry, "retry")
                                .outline()
                                .tooltip(
                                "Send the launch request again. It won’t start a second session.",
                            ),
                        );
                    }
                    (message.clone(), hint.clone(), theme.foreground, buttons)
                }
                LaunchNotice::Failed { headline, body } => (
                    headline.clone(),
                    Some(body.clone()),
                    theme.danger,
                    vec![
                        button("Retry", FlightAction::Retry, "retry").primary(),
                        button("Dismiss", FlightAction::Dismiss, "dismiss").ghost(),
                    ],
                ),
            };
        div()
            .mt_2()
            .pt_3()
            .border_t_1()
            .border_color(theme.border)
            .v_flex()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(message_color)
                    .child(message),
            )
            .when_some(body, |notice, body| {
                notice.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .line_clamp(4)
                        .child(body),
                )
            })
            .child(div().pt_1().h_flex().gap_2().children(buttons))
    });

    let upload = item.upload.as_ref().map(|upload| {
        div()
            .v_flex()
            .gap_1()
            .pb_1()
            .child(
                div()
                    .h(px(24.))
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .child(
                        div()
                            .size(px(16.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Spinner::new().xsmall()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(upload.label.clone()),
                    ),
            )
            .child(
                Progress::new(SharedString::from(format!("upload-{}", item.launch_id)))
                    .value(upload.fraction * 100.),
            )
    });
    let uploading = upload.is_some();
    let subject = item.subject.clone().map(|(kind, label)| {
        let icon = match kind {
            OpeningKind::Link => Icon::new(gpui_kit::assets::IconName::Link),
            OpeningKind::File => Icon::new(IconName::FileText),
        };
        div().pt_2().h_flex().child(
            div()
                .max_w_full()
                .h_flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded_md()
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(div().flex_none().child(icon.with_size(px(12.))))
                .child(div().min_w_0().truncate().child(label)),
        )
    });
    let top = div()
        .min_h(px(172.))
        .w_full()
        .p_4()
        .bg(theme.secondary)
        .v_flex()
        .children(upload)
        .when(!uploading, |top| top.children(rows))
        .when(notice.is_none(), |top| {
            top.when_some(item.detail.clone(), |top, detail| {
                top.child(
                    div()
                        .pt_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .truncate()
                        .child(detail),
                )
            })
        })
        .children(notice)
        .children(subject);

    let footer = div()
        .p_3p5()
        .border_t_1()
        .border_color(theme.border)
        .h_flex()
        .items_center()
        .gap_3()
        .child(app_badge(&item.app_name, item.app_icon.clone(), px(32.)))
        .child(
            div()
                .v_flex()
                .gap_1()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(item.app_name.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(if failed {
                            theme.danger
                        } else {
                            theme.muted_foreground
                        })
                        .truncate()
                        .child(item.status.clone()),
                ),
        )
        .when(item.cancellable, |footer| {
            let on_action = Rc::clone(&on_action);
            let id = item.launch_id.clone();
            footer.child(
                Button::new(SharedString::from(format!("flight-cancel-{id}")))
                    .outline()
                    .small()
                    .label("Cancel")
                    .tooltip("Stop the upload")
                    .on_click(move |_, window, cx| {
                        on_action(id.clone(), FlightAction::Cancel, window, cx)
                    }),
            )
        });

    div()
        .id(SharedString::from(format!("launching-{}", item.launch_id)))
        .w(px(336.))
        .rounded_xl()
        .overflow_hidden()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .v_flex()
        .child(top)
        .child(footer.flex_1())
}

fn meta_pill(theme: &Theme, icon: Option<IconName>, label: &str) -> Div {
    div()
        .h_flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded_md()
        .bg(theme.secondary)
        .border_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(theme.muted_foreground)
        .when_some(icon, |pill, icon| pill.child(icon))
        .child(label.to_string())
}

fn app_badge(name: &str, icon: Option<Arc<RenderImage>>, size: gpui_kit::Pixels) -> Div {
    if let Some(icon) = icon {
        return div()
            .size(size)
            .rounded_xl()
            .overflow_hidden()
            .child(img(icon).size_full().object_fit(ObjectFit::Contain));
    }

    let initials = app_initials(name);
    let accent = accent_color(name);
    let is_small = size <= px(36.);

    let badge = div()
        .size(size)
        .bg(accent)
        .text_color(rgb(0xffffff))
        .flex()
        .items_center()
        .justify_center()
        .font_weight(FontWeight::BOLD);

    if is_small {
        badge.rounded_lg().text_xs().child(initials)
    } else {
        badge.rounded_xl().text_xl().child(initials)
    }
}

fn app_initials(name: &str) -> String {
    let mut parts = name.split_whitespace();
    let first = parts
        .next()
        .and_then(|w| w.chars().next())
        .map(|c| c.to_ascii_uppercase());
    let second = parts
        .next()
        .and_then(|w| w.chars().next())
        .map(|c| c.to_ascii_uppercase());
    match (first, second) {
        (Some(a), Some(b)) => format!("{a}{b}"),
        (Some(a), None) => a.to_string(),
        _ => "?".to_string(),
    }
}

fn accent_color(name: &str) -> Rgba {
    const PALETTE: [u32; 6] = [
        0x5b6ef5, // indigo
        0xe56b4e, // coral/orange
        0x2eb85c, // green
        0x3b82f6, // blue
        0x9b5de5, // violet
        0x0ea5e9, // sky
    ];
    let hash = name.bytes().fold(0usize, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(b as usize)
    });
    rgb(PALETTE[hash % PALETTE.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, category: &str) -> AppItem {
        AppItem {
            id: id.into(),
            name: id[..1].to_uppercase() + &id[1..],
            categories: if category.is_empty() {
                Vec::new()
            } else {
                vec![category.into()]
            },
            favorite: false,
            hidden: false,
            remembered: false,
            icon: None,
        }
    }

    fn props(apps: Vec<AppItem>) -> LibraryProps {
        let favorites = apps
            .iter()
            .filter(|app| app.favorite)
            .map(|app| app.id.clone())
            .collect();
        LibraryProps {
            apps: apps.into(),
            favorites,
            ..LibraryProps::default()
        }
    }

    fn ids(apps: &[&AppItem]) -> Vec<String> {
        apps.iter().map(|app| app.id.clone()).collect()
    }

    fn fav(mut app: AppItem) -> AppItem {
        app.favorite = true;
        app
    }

    fn hide(mut app: AppItem) -> AppItem {
        app.hidden = true;
        app
    }

    #[test]
    fn favorites_row_lists_visible_favorites_in_saved_order() {
        let mut p = props(vec![
            fav(app("zen", "browser")),
            fav(app("code", "app")),
            hide(fav(app("krita", "app"))),
        ]);
        p.favorites = vec!["code".into(), "krita".into(), "zen".into()];
        let a = arrange(&p);
        assert_eq!(ids(&a.favorites), ["code", "zen"]);
        // Favorites stay in the grid too, where they carry the star.
        assert_eq!(ids(&a.grid), ["zen", "code"]);
    }

    #[test]
    fn recent_row_excludes_favorites_and_hidden_and_caps_at_eight() {
        let mut apps: Vec<AppItem> = (0..12).map(|n| app(&format!("a{n}"), "")).collect();
        apps[0].favorite = true;
        apps[1].hidden = true;
        let mut p = props(apps);
        p.recent = (0..12).map(|n| format!("a{n}")).collect();
        let a = arrange(&p);
        assert_eq!(
            ids(&a.recent),
            ["a2", "a3", "a4", "a5", "a6", "a7", "a8", "a9"]
        );
    }

    #[test]
    fn rows_step_aside_for_search_or_a_category() {
        let mut p = props(vec![fav(app("zen", "browser")), app("code", "app")]);
        p.recent = vec!["code".into()];
        assert_eq!(arrange(&p).favorites.len(), 1);
        assert_eq!(arrange(&p).recent.len(), 1);
        p.search_query = "z".into();
        let a = arrange(&p);
        assert!(a.favorites.is_empty() && a.recent.is_empty());
        p.search_query.clear();
        p.selected_filter = Filter::Category("app".into());
        let a = arrange(&p);
        assert!(a.favorites.is_empty() && a.recent.is_empty());
        assert_eq!(ids(&a.grid), ["code"]);
    }

    #[test]
    fn hidden_apps_leave_all_and_categories_but_match_search() {
        let mut p = props(vec![
            app("obsidian", "notes"),
            hide(app("organizer", "app")),
            app("zen", "browser"),
            hide(app("krita", "browsing")),
        ]);
        assert_eq!(ids(&arrange(&p).grid), ["obsidian", "zen"]);
        p.selected_filter = Filter::Category("app".into());
        // The category's only app is hidden, so the chip is gone and All shows.
        assert_eq!(arrange(&p).active, Filter::All);
        p.selected_filter = Filter::All;
        // Visible apps match names and categories; hidden ones match by name only, after.
        p.search_query = "o".into();
        assert_eq!(ids(&arrange(&p).grid), ["obsidian", "zen", "organizer"]);
        p.search_query = "brows".into();
        assert_eq!(ids(&arrange(&p).grid), ["zen"]);
    }

    #[test]
    fn hidden_chip_shows_count_and_only_when_any_are_hidden() {
        let p = props(vec![app("zen", ""), app("code", "")]);
        let a = arrange(&p);
        assert_eq!(a.hidden_count, 0);
        assert!(!a.filters.contains(&Filter::Hidden));
        let mut p = props(vec![
            hide(app("zen", "")),
            hide(app("code", "")),
            app("x", ""),
        ]);
        p.selected_filter = Filter::Hidden;
        let a = arrange(&p);
        assert_eq!(a.hidden_count, 2);
        assert_eq!(a.filters.last(), Some(&Filter::Hidden));
        assert_eq!(a.active, Filter::Hidden);
        assert_eq!(ids(&a.grid), ["zen", "code"]);
    }

    #[test]
    fn selected_filter_that_disappears_falls_back_to_all() {
        let mut p = props(vec![app("zen", "browser")]);
        p.selected_filter = Filter::Hidden;
        assert_eq!(arrange(&p).active, Filter::All);
        p.selected_filter = Filter::Category("gone".into());
        assert_eq!(arrange(&p).active, Filter::All);
        assert_eq!(ids(&arrange(&p).grid), ["zen"]);
    }

    #[test]
    fn recent_chip_is_gone() {
        let mut p = props(vec![app("zen", "browser")]);
        p.recent = vec!["zen".into()];
        assert_eq!(
            arrange(&p).filters,
            [Filter::All, Filter::Category("browser".into())]
        );
    }

    #[test]
    fn a_category_named_hidden_does_not_collide() {
        let mut p = props(vec![app("zen", "Hidden"), app("all", "All")]);
        let a = arrange(&p);
        assert_eq!(
            a.filters,
            [
                Filter::All,
                Filter::Category("All".into()),
                Filter::Category("Hidden".into())
            ]
        );
        assert_ne!(
            Filter::Category("Hidden".into()).element_id(),
            Filter::Hidden.element_id()
        );
        p.selected_filter = Filter::Category("Hidden".into());
        assert_eq!(ids(&arrange(&p).grid), ["zen"]);
    }

    #[test]
    fn rows_skip_ids_the_server_no_longer_lists() {
        let mut p = props(vec![fav(app("zen", ""))]);
        p.favorites = vec!["gone".into(), "zen".into()];
        p.recent = vec!["gone".into()];
        let a = arrange(&p);
        assert_eq!(ids(&a.favorites), ["zen"]);
        assert!(a.recent.is_empty());
    }

    #[test]
    fn tile_ids_are_unique_per_placement() {
        let ids = [
            tile_id(Placement::Favorites, "zen"),
            tile_id(Placement::Recent, "zen"),
            tile_id(Placement::Grid, "zen"),
        ];
        assert_eq!(
            ids,
            ["app-tile-fav-zen", "app-tile-recent-zen", "app-tile-zen"]
        );
    }

    #[test]
    fn launching_cards_ignore_the_search_query() {
        let mut p = props(vec![app("zen", "")]);
        p.launching = vec![LaunchingItem {
            launch_id: "l1".into(),
            app_name: "Firefox".into(),
            app_icon: None,
            stages: Vec::new(),
            detail: None,
            status: "Launching · 0:01".into(),
            notice: None,
            upload: None,
            subject: None,
            cancellable: false,
        }];
        p.search_query = "zen".into();
        assert_eq!(arrange(&p).launching.len(), 1);
    }

    fn opening(kind: OpeningKind) -> OpeningProps {
        OpeningProps {
            kind,
            label: "x".into(),
            tooltip: None,
            detail: None,
            rejected: false,
            sessions: Vec::new(),
            apps: Vec::new(),
            apps_expanded: false,
            queue_more: false,
            pending_session: None,
            note: None,
            enter_hint: false,
            generation: 0,
        }
    }

    #[test]
    fn a_chooser_hides_sections_the_filter_leaves_empty() {
        let mut p = props(vec![app("zen", "")]);
        p.search_query = "wiki.lan/page".into();
        assert!(!arrange(&p).hide_empty);
        p.opening = Some(opening(OpeningKind::Link));
        let a = arrange(&p);
        assert!(a.hide_empty);
        assert!(a.grid.is_empty() && a.sessions.is_empty());
    }
}
