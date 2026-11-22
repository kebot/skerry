//! The session's Apps window (`ARCH-035`): a category source list, the proot-apps icon grid,
//! and an inspector with the selected app's buttons and job console. Pure presentation:
//! `skerry-mac` owns the catalog, the board, the icons, and every request.

use crate::files::Notice;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    skeleton::Skeleton,
    spinner::Spinner,
    tooltip::Tooltip,
    Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{
    div, img, prelude::*, px, relative, uniform_list, App, ClickEvent, Div, Entity, FocusHandle,
    FontWeight, Hsla, MouseButton, ObjectFit, Pixels, RenderImage, ScrollHandle, SharedString,
    UniformListScrollHandle, Window,
};
use std::rc::Rc;
use std::sync::Arc;

pub type AppsHandler = Rc<dyn Fn(AppsAction, &mut Window, &mut App)>;

/// A tile, 112×104 pt, with a 12 pt gap.
pub const TILE_WIDTH: f32 = 112.;
pub const TILE_HEIGHT: f32 = 104.;
pub const TILE_GAP: f32 = 12.;
/// The grid's padding.
pub const GRID_PADDING: f32 = 16.;
/// Source list and inspector widths, and the window width below which they narrow.
pub const SOURCE_WIDTH: f32 = 188.;
pub const SOURCE_WIDTH_NARROW: f32 = 168.;
pub const SOURCE_NARROW_BELOW: f32 = 800.;
pub const INSPECTOR_WIDTH: f32 = 300.;
pub const INSPECTOR_WIDTH_NARROW: f32 = 280.;
pub const INSPECTOR_NARROW_BELOW: f32 = 820.;
/// Activity shows about four and a half rows before it scrolls.
const ACTIVITY_ROW: f32 = 36.;
const ACTIVITY_MAX_HEIGHT: f32 = 4.5 * ACTIVITY_ROW;

/// How many tiles fit across a grid pane `width` wide (padding included), 2–8.
pub fn columns(width: Pixels) -> usize {
    let inner = f32::from(width) - 2. * GRID_PADDING;
    let fit = ((inner + TILE_GAP) / (TILE_WIDTH + TILE_GAP)).floor();
    (fit.max(0.) as usize).clamp(2, 8)
}

/// The source list's and the inspector's widths for a window `width` wide.
pub fn pane_widths(width: Pixels) -> (f32, f32) {
    let width = f32::from(width);
    (
        if width < SOURCE_NARROW_BELOW {
            SOURCE_WIDTH_NARROW
        } else {
            SOURCE_WIDTH
        },
        if width < INSPECTOR_NARROW_BELOW {
            INSPECTOR_WIDTH_NARROW
        } else {
            INSPECTOR_WIDTH
        },
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FilterId {
    Installed,
    All,
    Category(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CategoryRow {
    pub id: FilterId,
    pub label: String,
    pub icon: gpui_kit::assets::IconName,
    /// The capsule; `None` hides it.
    pub count: Option<usize>,
    pub selected: bool,
}

/// A tile's corner badge, by precedence Running > Queued > Failed > Installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Badge {
    None,
    Installed,
    Queued,
    Running,
    Failed,
}

#[derive(Clone, Debug)]
pub struct AppTile {
    pub name: String,
    pub full_name: String,
    pub initial: char,
    pub icon: Option<Arc<RenderImage>>,
    /// Its icon hasn't been asked for yet: a visible tile asks.
    pub want_icon: bool,
    pub badge: Badge,
    /// A download's percentage, drawn along the foot.
    pub progress: Option<f32>,
    pub tooltip: String,
    pub selected: bool,
}

impl PartialEq for AppTile {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.full_name == other.full_name
            && self.icon.as_ref().map(Arc::as_ptr) == other.icon.as_ref().map(Arc::as_ptr)
            && self.want_icon == other.want_icon
            && self.badge == other.badge
            && self.progress == other.progress
            && self.tooltip == other.tooltip
            && self.selected == other.selected
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobBadge {
    Queued,
    Running,
    Done,
    Failed,
    Canceled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityRow {
    pub id: u64,
    pub app: String,
    pub full_name: String,
    pub status: String,
    pub state: JobBadge,
    pub progress: Option<f32>,
    /// Cancel was pressed: the Cancel button is off until the job ends.
    pub canceling: bool,
}

/// Which button a pending job or request spins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    Install,
    Open,
    Update,
    Remove,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InspectorButtons {
    pub shown_installed: bool,
    pub pending: Option<ButtonKind>,
    /// "Requested…" or the job's status, the spinning button's tooltip.
    pub pending_tooltip: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConsoleProps {
    pub job: u64,
    pub status: String,
    pub state: JobBadge,
    pub code: Option<i64>,
    pub progress: Option<f32>,
    pub text: SharedString,
    pub can_cancel: bool,
    pub can_clear: bool,
}

#[derive(Clone, Debug)]
pub struct Inspector {
    pub name: String,
    pub full_name: String,
    pub description: String,
    pub icon: Option<Arc<RenderImage>>,
    pub initial: char,
    pub installed: bool,
    pub buttons: InspectorButtons,
    pub console: Option<ConsoleProps>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogView {
    Loading,
    Ready,
    Failed(String),
}

/// A state across the top (not dismissible), one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Banner {
    Waiting,
    CommandsDisabled,
    InstalledUnknown,
}

impl Banner {
    pub fn text(self) -> &'static str {
        match self {
            Banner::Waiting => "Waiting for the session…",
            Banner::CommandsDisabled => {
                "App installation is unavailable: remote commands are disabled on this server."
            }
            Banner::InstalledUnknown => "The session couldn’t say which apps are installed.",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppsProps {
    pub categories: Vec<CategoryRow>,
    pub filter: FilterId,
    /// The search field's text, trimmed; non-empty while searching.
    pub search: String,
    pub tiles: Rc<Vec<AppTile>>,
    pub columns: usize,
    pub inspector: Option<Inspector>,
    pub activity: Vec<ActivityRow>,
    pub catalog: CatalogView,
    /// `None` while the session hasn't said which apps are installed.
    pub installed_count: Option<usize>,
    pub notice: Option<Notice>,
    pub banner: Option<Banner>,
    /// Requests may go out (connected and allowed).
    pub enabled: bool,
    /// The window is key: selection in the accent colour, else grey.
    pub focused: bool,
    /// The window's width, for the narrow layout.
    pub width: Pixels,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppsAction {
    Filter(FilterId),
    Select(String),
    ClearSelection,
    /// Double-click or Return: Open when installed, Install otherwise.
    Primary(String),
    Install(String),
    Open(String),
    Update(String),
    /// Remove…, after the confirmation.
    Remove(String),
    Cancel(u64),
    Clear(u64),
    ClearFinished,
    CopyConsole,
    Retry,
    DismissNotice,
    ClearSearch,
    /// An Activity row: select its app.
    SelectActivity(String),
    /// Visible tiles whose icons aren't loaded yet.
    IconsVisible(Vec<String>),
}

/// The toolbar's title for the filter or search.
pub fn title(props: &AppsProps) -> String {
    if props.catalog != CatalogView::Ready {
        return "Apps".into();
    }
    if !props.search.is_empty() {
        return "Search Results".into();
    }
    match &props.filter {
        FilterId::Installed => "Installed".into(),
        FilterId::All => "All Apps".into(),
        FilterId::Category(_) => props
            .categories
            .iter()
            .find(|row| row.id == props.filter)
            .map(|row| row.label.clone())
            .unwrap_or_else(|| "Apps".into()),
    }
}

pub fn app_count(count: usize) -> String {
    if count == 1 {
        "1 app".into()
    } else {
        format!("{count} apps")
    }
}

/// The status bar's left text.
pub fn status_text(props: &AppsProps) -> String {
    if props.catalog != CatalogView::Ready {
        return String::new();
    }
    let mut text = app_count(props.tiles.len());
    if let Some(installed) = props.installed_count {
        text.push_str(&format!(" · {installed} installed"));
    }
    text
}

/// An empty grid's icon, sentence, and link button, if any.
pub type EmptyState = (
    gpui_kit::assets::IconName,
    String,
    Option<(&'static str, AppsAction)>,
);

/// The grid's empty state, when it shows one.
pub fn empty_state(props: &AppsProps) -> Option<EmptyState> {
    use gpui_kit::assets::IconName as I;
    if !props.tiles.is_empty() || props.catalog != CatalogView::Ready {
        return None;
    }
    Some(if !props.search.is_empty() {
        (
            I::SearchX,
            format!("No apps match “{}”.", props.search),
            Some(("Clear Search", AppsAction::ClearSearch)),
        )
    } else {
        match (&props.filter, props.installed_count) {
            (FilterId::Installed, Some(_)) => (
                I::PackageCheck,
                "No apps installed yet.".into(),
                Some(("Browse All Apps", AppsAction::Filter(FilterId::All))),
            ),
            (FilterId::Installed, None) => (
                I::PackageCheck,
                "The session hasn’t said which apps are installed.".into(),
                None,
            ),
            _ => (I::Shapes, "No apps in this category.".into(), None),
        }
    })
}

pub fn render(
    theme: &Theme,
    props: &AppsProps,
    search: &Entity<InputState>,
    grid: &UniformListScrollHandle,
    grid_focus: &FocusHandle,
    console: &ScrollHandle,
    on_action: AppsHandler,
) -> Div {
    let (source_width, inspector_width) = pane_widths(props.width);
    div()
        .v_flex()
        .size_full()
        .child(toolbar(theme, props, search))
        .when_some(props.banner, |view, banner| {
            view.child(banner_row(theme, banner))
        })
        .when_some(props.notice.clone(), |view, notice| {
            view.child(notice_row(theme, notice, &on_action))
        })
        .child(
            div()
                .flex_1()
                .min_h_0()
                .h_flex()
                .child(source_list(theme, props, source_width, &on_action))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .track_focus(grid_focus)
                        .key_context("AppsGrid")
                        .child(grid_pane(theme, props, grid, &on_action)),
                )
                .child(inspector(
                    theme,
                    props,
                    inspector_width,
                    console,
                    &on_action,
                )),
        )
        .child(status_bar(theme, props))
}

fn toolbar(theme: &Theme, props: &AppsProps, search: &Entity<InputState>) -> Div {
    let ready = props.catalog == CatalogView::Ready;
    let search_width = if f32::from(props.width) < INSPECTOR_NARROW_BELOW {
        200.
    } else {
        240.
    };
    div()
        .flex_none()
        .h(px(44.))
        .h_flex()
        .items_center()
        .gap_2()
        .px_3()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title(props)),
        )
        .when(ready, |bar| {
            bar.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(app_count(props.tiles.len())),
            )
        })
        .child(div().flex_1())
        .child(
            div().w(px(search_width)).flex_none().child(
                Input::new(search)
                    .small()
                    .cleanable(true)
                    .disabled(!ready)
                    .prefix(
                        Icon::new(IconName::Search)
                            .small()
                            .text_color(theme.muted_foreground),
                    ),
            ),
        )
}

fn banner_row(theme: &Theme, banner: Banner) -> Div {
    div()
        .flex_none()
        .h_flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_1p5()
        .text_xs()
        .bg(theme.secondary)
        .border_b_1()
        .border_color(theme.border)
        .child(if banner == Banner::Waiting {
            Spinner::new().xsmall().into_any_element()
        } else {
            Icon::new(IconName::Info)
                .small()
                .text_color(theme.muted_foreground)
                .into_any_element()
        })
        .child(div().flex_1().min_w_0().child(banner.text()))
}

fn notice_row(theme: &Theme, notice: Notice, on_action: &AppsHandler) -> Div {
    let (icon, tint) = if notice.error {
        (IconName::CircleX, theme.danger)
    } else {
        (IconName::Info, theme.muted_foreground)
    };
    div()
        .flex_none()
        .h_flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_1p5()
        .border_b_1()
        .border_color(theme.border)
        .bg(if notice.error {
            theme.danger.opacity(0.1)
        } else {
            theme.secondary
        })
        .child(Icon::new(icon).small().text_color(tint))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .when(notice.error, |text| text.text_color(theme.danger))
                .child(notice.text),
        )
        .child(icon_button(
            "apps-notice-dismiss",
            IconName::Close,
            "Dismiss",
            true,
            AppsAction::DismissNotice,
            on_action,
        ))
}

fn section_header(theme: &Theme, label: &'static str) -> Div {
    div()
        .px_2()
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.muted_foreground)
        .child(label)
}

fn source_list(theme: &Theme, props: &AppsProps, width: f32, on_action: &AppsHandler) -> Div {
    let searching = !props.search.is_empty();
    let loading = props.catalog == CatalogView::Loading;
    let failed = matches!(props.catalog, CatalogView::Failed(_));
    let rows = props
        .categories
        .iter()
        .filter(|row| !failed || matches!(row.id, FilterId::Installed | FilterId::All));
    let mut list = div()
        .id("apps-sources")
        .flex_1()
        .min_h_0()
        .v_flex()
        .gap_0p5()
        .p_2()
        .overflow_y_scroll()
        .when(searching, |list| list.opacity(0.5));
    if loading {
        list = list.children((0..6).map(|_| {
            div()
                .h(px(28.))
                .h_flex()
                .items_center()
                .gap_2()
                .px_2()
                .child(Skeleton::new().size(px(16.)).rounded_sm())
                .child(Skeleton::new().w(px(100.)).h(px(10.)).rounded_sm())
        }));
    } else {
        let mut seen_category = false;
        for row in rows {
            if matches!(row.id, FilterId::Category(_)) && !seen_category {
                seen_category = true;
                list = list.child(
                    div()
                        .mt_3()
                        .mb_1()
                        .child(section_header(theme, "CATEGORIES")),
                );
            }
            let selected = row.selected && !searching;
            list = list.child(source_row(theme, row, selected, props.focused, on_action));
        }
    }
    div()
        .w(px(width))
        .flex_none()
        .h_full()
        .v_flex()
        .bg(theme.secondary)
        .border_r_1()
        .border_color(theme.border)
        .child(list)
        .when(!props.activity.is_empty(), |pane| {
            pane.child(activity(theme, props, on_action))
        })
}

fn source_row(
    theme: &Theme,
    row: &CategoryRow,
    selected: bool,
    focused: bool,
    on_action: &AppsHandler,
) -> impl IntoElement {
    let on_click = Rc::clone(on_action);
    let id = row.id.clone();
    let (background, foreground): (Option<Hsla>, Hsla) = match (selected, focused) {
        (true, true) => (Some(theme.primary), theme.primary_foreground),
        (true, false) => (Some(theme.secondary_hover), theme.foreground),
        _ => (None, theme.foreground),
    };
    let element_id = SharedString::from(format!("apps-source-{}", row.label));
    div()
        .id(element_id)
        .h(px(28.))
        .flex_none()
        .h_flex()
        .items_center()
        .gap_2()
        .px_2()
        .rounded(px(6.))
        .cursor_pointer()
        .text_color(foreground)
        .when_some(background, |row, color| row.bg(color))
        .when(!selected, |row| {
            row.hover(|style| style.bg(theme.secondary_hover))
        })
        .child(Icon::new(row.icon).size(px(16.)).flex_none())
        .child(div().flex_1().min_w_0().truncate().child(row.label.clone()))
        .when_some(row.count, |line, count| {
            line.child(
                div()
                    .flex_none()
                    .px_1p5()
                    .rounded(px(8.))
                    .text_size(px(11.))
                    .when(selected && focused, |capsule| {
                        capsule.bg(gpui_kit::white().opacity(0.2))
                    })
                    .when(!(selected && focused), |capsule| {
                        capsule.bg(theme.muted).text_color(theme.muted_foreground)
                    })
                    .child(count.to_string()),
            )
        })
        .on_click(move |_, window, cx| on_click(AppsAction::Filter(id.clone()), window, cx))
}

fn job_icon(theme: &Theme, state: JobBadge, size: f32) -> gpui_kit::AnyElement {
    use gpui_kit::assets::IconName as I;
    match state {
        JobBadge::Running => Spinner::new()
            .with_size(px(size))
            .color(theme.primary)
            .into_any_element(),
        JobBadge::Queued => Icon::new(I::Clock)
            .size(px(size))
            .text_color(theme.muted_foreground)
            .into_any_element(),
        JobBadge::Done => Icon::new(IconName::CircleCheck)
            .size(px(size))
            .text_color(theme.primary)
            .into_any_element(),
        JobBadge::Failed => Icon::new(IconName::CircleX)
            .size(px(size))
            .text_color(theme.danger)
            .into_any_element(),
        JobBadge::Canceled => Icon::new(I::Ban)
            .size(px(size))
            .text_color(theme.muted_foreground)
            .into_any_element(),
    }
}

/// A thin bar, `percent` of 100 filled.
fn bar(theme: &Theme, percent: f32, height: f32) -> Div {
    div()
        .w_full()
        .h(px(height))
        .rounded_full()
        .bg(theme.muted)
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(theme.primary)
                .w(relative((percent / 100.).clamp(0., 1.))),
        )
}

fn activity(theme: &Theme, props: &AppsProps, on_action: &AppsHandler) -> Div {
    let finished = props.activity.iter().any(|row| {
        matches!(
            row.state,
            JobBadge::Done | JobBadge::Failed | JobBadge::Canceled
        )
    });
    let clear = Rc::clone(on_action);
    div()
        .flex_none()
        .v_flex()
        .pt_2()
        .pb_2()
        .px_2()
        .border_t_1()
        .border_color(theme.border)
        .child(
            div()
                .h(px(24.))
                .h_flex()
                .items_center()
                .justify_between()
                .child(section_header(theme, "ACTIVITY"))
                .when(finished, |header| {
                    header.child(
                        Button::new("apps-clear-finished")
                            .ghost()
                            .xsmall()
                            .label("Clear Finished")
                            .disabled(!props.enabled)
                            .on_click(move |_, window, cx| {
                                clear(AppsAction::ClearFinished, window, cx)
                            }),
                    )
                }),
        )
        .child(
            div()
                .id("apps-activity")
                .v_flex()
                .max_h(px(ACTIVITY_MAX_HEIGHT))
                .overflow_y_scroll()
                .children(
                    props
                        .activity
                        .iter()
                        .map(|row| activity_row(theme, row, props.enabled, on_action)),
                ),
        )
}

fn activity_row(
    theme: &Theme,
    row: &ActivityRow,
    enabled: bool,
    on_action: &AppsHandler,
) -> impl IntoElement {
    let select = Rc::clone(on_action);
    let app = row.app.clone();
    let pending = matches!(row.state, JobBadge::Queued | JobBadge::Running);
    let group = SharedString::from(format!("apps-activity-{}", row.id));
    let (action, icon, tooltip, can) = if pending {
        (
            AppsAction::Cancel(row.id),
            gpui_kit::assets::IconName::Ban,
            "Cancel",
            enabled && !row.canceling,
        )
    } else {
        (
            AppsAction::Clear(row.id),
            gpui_kit::assets::IconName::X,
            "Clear",
            enabled,
        )
    };
    div()
        .id(group.clone())
        .group(group.clone())
        .h(px(ACTIVITY_ROW))
        .flex_none()
        .h_flex()
        .items_center()
        .gap_2()
        .px_1()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|style| style.bg(theme.secondary_hover))
        .child(div().flex_none().child(job_icon(theme, row.state, 14.)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .v_flex()
                .child(div().text_xs().truncate().child(row.full_name.clone()))
                .child(
                    div()
                        .text_size(px(11.))
                        .truncate()
                        .text_color(if row.state == JobBadge::Failed {
                            theme.danger
                        } else {
                            theme.muted_foreground
                        })
                        .child(row.status.clone()),
                )
                .when_some(row.progress, |line, progress| {
                    line.child(div().pt_0p5().child(bar(theme, progress, 2.)))
                }),
        )
        .child(
            div()
                .flex_none()
                .invisible()
                .group_hover(group, |style| style.visible())
                .child(icon_button(
                    SharedString::from(format!("apps-activity-action-{}", row.id)),
                    icon,
                    tooltip,
                    can,
                    action,
                    on_action,
                )),
        )
        .on_click(move |_, window, cx| select(AppsAction::SelectActivity(app.clone()), window, cx))
}

fn placeholder(theme: &Theme) -> Div {
    div()
        .size_full()
        .v_flex()
        .gap_3()
        .items_center()
        .justify_center()
        .p_6()
        .text_center()
        .text_color(theme.muted_foreground)
}

fn grid_pane(
    theme: &Theme,
    props: &AppsProps,
    scroll: &UniformListScrollHandle,
    on_action: &AppsHandler,
) -> Div {
    let container = div().size_full().relative();
    match &props.catalog {
        CatalogView::Loading => {
            return container.child(
                div()
                    .p(px(GRID_PADDING))
                    .h_flex()
                    .flex_wrap()
                    .gap(px(TILE_GAP))
                    .children((0..12).map(|_| {
                        div()
                            .w(px(TILE_WIDTH))
                            .h(px(TILE_HEIGHT))
                            .v_flex()
                            .items_center()
                            .gap_2()
                            .pt(px(14.))
                            .child(Skeleton::new().size(px(40.)).rounded(px(8.)))
                            .child(Skeleton::new().w(px(72.)).h(px(10.)).rounded_sm())
                    })),
            );
        }
        CatalogView::Failed(message) => {
            let retry = Rc::clone(on_action);
            return container.child(
                placeholder(theme)
                    .child(
                        Icon::new(IconName::CircleAlert)
                            .size(px(44.))
                            .text_color(theme.danger),
                    )
                    .child(message.clone())
                    .child(
                        Button::new("apps-try-again")
                            .outline()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label("Try Again")
                            .on_click(move |_, window, cx| retry(AppsAction::Retry, window, cx)),
                    ),
            );
        }
        CatalogView::Ready => {}
    }
    if let Some((icon, text, link)) = empty_state(props) {
        return container.child(
            placeholder(theme)
                .child(Icon::new(icon).size(px(44.)))
                .child(text)
                .when_some(link, |view, (label, action)| {
                    let on_action = Rc::clone(on_action);
                    view.child(
                        Button::new("apps-empty-link")
                            .ghost()
                            .small()
                            .label(label)
                            .text_color(theme.primary)
                            .on_click(move |_, window, cx| on_action(action.clone(), window, cx)),
                    )
                }),
        );
    }
    let tiles = Rc::clone(&props.tiles);
    let columns = props.columns.max(1);
    let rows = tiles.len().div_ceil(columns);
    let theme_row = theme.clone();
    let on_tiles = Rc::clone(on_action);
    let on_background = Rc::clone(on_action);
    let focused = props.focused;
    container
        .child(
            uniform_list("apps-grid", rows, move |range, window, cx| {
                let wanted: Vec<String> = range
                    .clone()
                    .flat_map(|row| row * columns..((row + 1) * columns).min(tiles.len()))
                    .filter(|&index| tiles[index].want_icon)
                    .map(|index| tiles[index].name.clone())
                    .collect();
                if !wanted.is_empty() {
                    let on_action = Rc::clone(&on_tiles);
                    window.defer(cx, move |window, cx| {
                        on_action(AppsAction::IconsVisible(wanted), window, cx)
                    });
                }
                range
                    .map(|row| {
                        let start = row * columns;
                        let end = (start + columns).min(tiles.len());
                        div()
                            .px(px(GRID_PADDING))
                            .pt(px(if row == 0 { GRID_PADDING } else { 0. }))
                            .pb(px(TILE_GAP))
                            .h_flex()
                            .gap(px(TILE_GAP))
                            .children(
                                tiles[start..end]
                                    .iter()
                                    .map(|tile| tile_view(&theme_row, tile, focused, &on_tiles)),
                            )
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(scroll)
            .size_full(),
        )
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            // A click between tiles clears the selection; a tile's own mouse-down marks the
            // event handled first.
            if !window.default_prevented() {
                on_background(AppsAction::ClearSelection, window, cx);
            }
        })
}

/// A missing, failed or loading icon: the initial on a muted rounded square.
fn initial_square(theme: &Theme, initial: char, size: f32, text: f32) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(size / 5.))
        .bg(theme.muted)
        .h_flex()
        .items_center()
        .justify_center()
        .text_size(px(text))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.muted_foreground)
        .child(initial.to_string())
}

fn app_icon(
    theme: &Theme,
    icon: Option<Arc<RenderImage>>,
    initial: char,
    size: f32,
    text: f32,
) -> Div {
    match icon {
        Some(icon) => div()
            .size(px(size))
            .flex_none()
            .rounded(px(size / 5.))
            .overflow_hidden()
            .child(img(icon).size_full().object_fit(ObjectFit::Contain)),
        None => initial_square(theme, initial, size, text),
    }
}

fn tile_view(
    theme: &Theme,
    tile: &AppTile,
    focused: bool,
    on_action: &AppsHandler,
) -> impl IntoElement {
    let on_click = Rc::clone(on_action);
    let on_press = Rc::clone(on_action);
    let name = tile.name.clone();
    let press_name = tile.name.clone();
    let tooltip = SharedString::from(tile.tooltip.clone());
    let ring = if focused { theme.primary } else { theme.border };
    let badge: Option<gpui_kit::AnyElement> = match tile.badge {
        Badge::None => None,
        Badge::Installed => Some(
            div()
                .size(px(16.))
                .rounded_full()
                .bg(theme.primary)
                .h_flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(IconName::Check)
                        .size(px(10.))
                        .text_color(gpui_kit::white()),
                )
                .into_any_element(),
        ),
        Badge::Running => Some(job_icon(theme, JobBadge::Running, 16.)),
        Badge::Queued => Some(job_icon(theme, JobBadge::Queued, 16.)),
        Badge::Failed => Some(
            Icon::new(IconName::CircleAlert)
                .size(px(16.))
                .text_color(theme.danger)
                .into_any_element(),
        ),
    };
    div()
        .id(SharedString::from(format!("apps-tile-{}", tile.name)))
        .w(px(TILE_WIDTH))
        .h(px(TILE_HEIGHT))
        .flex_none()
        .relative()
        .v_flex()
        .items_center()
        .pt(px(14.))
        .rounded(px(10.))
        .cursor_pointer()
        .when(tile.selected, |tile| {
            tile.bg(theme.muted.opacity(0.5))
                .border_2()
                .border_color(ring)
        })
        .when(!tile.selected, |tile| {
            tile.hover(|style| style.bg(theme.muted.opacity(0.5)))
        })
        .child(app_icon(theme, tile.icon.clone(), tile.initial, 40., 18.))
        .child(
            div()
                .mt_2()
                .px_2()
                .w_full()
                .text_center()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .line_clamp(2)
                .text_ellipsis()
                .child(tile.full_name.clone()),
        )
        .when_some(badge, |view, badge| {
            view.child(div().absolute().top(px(6.)).right(px(6.)).child(badge))
        })
        .when_some(tile.progress, |view, progress| {
            view.child(
                div()
                    .absolute()
                    .bottom(px(6.))
                    .left(px(8.))
                    .right(px(8.))
                    .child(bar(theme, progress, 2.)),
            )
        })
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.prevent_default();
            on_press(AppsAction::Select(press_name.clone()), window, cx);
        })
        .on_click(move |event: &ClickEvent, window, cx| {
            if event.click_count() >= 2 {
                on_click(AppsAction::Primary(name.clone()), window, cx);
            }
        })
}

fn inspector(
    theme: &Theme,
    props: &AppsProps,
    width: f32,
    console: &ScrollHandle,
    on_action: &AppsHandler,
) -> Div {
    let pane = div()
        .w(px(width))
        .flex_none()
        .h_full()
        .v_flex()
        .gap_3()
        .p_4()
        .border_l_1()
        .border_color(theme.border);
    let Some(app) = &props.inspector else {
        return pane.child(
            placeholder(theme)
                .text_sm()
                .child("Select an app to see its details."),
        );
    };
    let has_console = app.console.is_some();
    pane.child(app_icon(theme, app.icon.clone(), app.initial, 64., 28.))
        .child(
            div()
                .text_size(px(17.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(app.full_name.clone()),
        )
        .child(
            div()
                .h_flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .px_1p5()
                        .rounded(px(4.))
                        .bg(theme.muted)
                        .text_size(px(11.))
                        .font_family("Menlo")
                        .child(app.name.clone()),
                )
                .when(app.installed, |row| {
                    row.child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_1()
                            .px_1p5()
                            .rounded(px(8.))
                            .bg(theme.primary.opacity(0.1))
                            .text_color(theme.primary)
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(Icon::new(IconName::Check).size(px(10.)))
                            .child("Installed"),
                    )
                }),
        )
        .when(!app.description.is_empty(), |pane| {
            pane.child(
                div()
                    .text_sm()
                    .line_height(relative(1.4))
                    .when(has_console, |text| text.line_clamp(6).text_ellipsis())
                    .child(app.description.clone()),
            )
        })
        .child(buttons(app, on_action))
        .when_some(app.console.clone(), |pane, props| {
            pane.child(console_card(theme, &props, console, on_action))
        })
}

fn buttons(app: &Inspector, on_action: &AppsHandler) -> Div {
    use gpui_kit::assets::IconName as I;
    let b = &app.buttons;
    let make = |id: &'static str,
                kind: ButtonKind,
                label: &'static str,
                tooltip: String,
                icon: gpui_kit::assets::IconName,
                action: AppsAction| {
        let on_action = Rc::clone(on_action);
        let spinning = b.pending == Some(kind);
        let tooltip = if spinning {
            b.pending_tooltip.clone().unwrap_or(tooltip)
        } else {
            tooltip
        };
        Button::new(id)
            .label(label)
            .icon(icon)
            .loading(spinning)
            .tooltip(tooltip)
            .disabled(!b.enabled)
            .w_full()
            .on_click(move |_, window, cx| on_action(action.clone(), window, cx))
    };
    let name = &app.full_name;
    if !b.shown_installed {
        return div().child(
            make(
                "apps-install",
                ButtonKind::Install,
                "Install",
                format!("Install {name} in this session"),
                I::Download,
                AppsAction::Install(app.name.clone()),
            )
            .primary(),
        );
    }
    div()
        .v_flex()
        .gap_2()
        .child(
            make(
                "apps-open",
                ButtonKind::Open,
                "Open",
                format!("Open {name} in the session"),
                I::Play,
                AppsAction::Open(app.name.clone()),
            )
            .primary(),
        )
        .child(
            div()
                .h_flex()
                .gap_2()
                .child(
                    div().flex_1().child(
                        make(
                            "apps-update",
                            ButtonKind::Update,
                            "Update",
                            format!("Update {name}"),
                            I::RotateCw,
                            AppsAction::Update(app.name.clone()),
                        )
                        .outline(),
                    ),
                )
                .child(
                    div().flex_1().child(
                        make(
                            "apps-remove",
                            ButtonKind::Remove,
                            "Remove…",
                            format!("Remove {name} from this session"),
                            I::Trash,
                            AppsAction::Remove(app.name.clone()),
                        )
                        .danger()
                        .outline(),
                    ),
                ),
        )
}

fn console_card(
    theme: &Theme,
    props: &ConsoleProps,
    scroll: &ScrollHandle,
    on_action: &AppsHandler,
) -> Div {
    let failed = props.state == JobBadge::Failed;
    let pending = matches!(props.state, JobBadge::Queued | JobBadge::Running);
    let empty = props.text.is_empty();
    div()
        .flex_1()
        .min_h(px(160.))
        .v_flex()
        .rounded(px(10.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted.opacity(0.3))
        .overflow_hidden()
        .child(
            div()
                .flex_none()
                .min_h(px(36.))
                .h_flex()
                .items_center()
                .gap_2()
                .px_3()
                .child(div().flex_none().child(job_icon(theme, props.state, 16.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .v_flex()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .when(failed, |text| text.text_color(theme.danger))
                                .child(props.status.clone()),
                        )
                        .when_some(props.code.filter(|_| failed), |line, code| {
                            line.child(
                                div()
                                    .text_size(px(11.))
                                    .font_family("Menlo")
                                    .text_color(theme.danger)
                                    .child(format!("exit {code}")),
                            )
                        }),
                )
                .child(icon_button(
                    "apps-console-copy",
                    IconName::Copy,
                    "Copy Output",
                    !empty,
                    AppsAction::CopyConsole,
                    on_action,
                ))
                .child(if pending {
                    icon_button(
                        "apps-console-cancel",
                        gpui_kit::assets::IconName::Ban,
                        "Cancel",
                        props.can_cancel,
                        AppsAction::Cancel(props.job),
                        on_action,
                    )
                } else {
                    icon_button(
                        "apps-console-clear",
                        IconName::Close,
                        "Clear",
                        props.can_clear,
                        AppsAction::Clear(props.job),
                        on_action,
                    )
                }),
        )
        .when_some(props.progress, |card, progress| {
            card.child(div().px_3().pb_1().child(bar(theme, progress, 4.)))
        })
        .child(
            div()
                .id("apps-console")
                .flex_1()
                .min_h_0()
                .m_1()
                .p_2()
                .rounded(px(6.))
                .bg(theme.background.opacity(0.6))
                .overflow_y_scroll()
                .track_scroll(scroll)
                .text_size(px(11.))
                .line_height(relative(1.5))
                .font_family("Menlo")
                .text_color(theme.muted_foreground)
                .when(empty, |body| body.italic().child("No output yet."))
                .when(!empty, |body| body.child(props.text.clone())),
        )
}

fn status_bar(theme: &Theme, props: &AppsProps) -> Div {
    div()
        .flex_none()
        .h_flex()
        .items_center()
        .justify_between()
        .px_3()
        .py_1()
        .text_xs()
        .text_color(theme.muted_foreground)
        .border_t_1()
        .border_color(theme.border)
        .child(status_text(props))
        .when(props.catalog == CatalogView::Loading, |bar| {
            bar.child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(Spinner::new().xsmall())
                    .child("Loading catalog…"),
            )
        })
}

fn icon_button(
    id: impl Into<SharedString>,
    icon: impl Into<Icon>,
    tooltip: &'static str,
    enabled: bool,
    action: AppsAction,
    on_action: &AppsHandler,
) -> Button {
    let on_action = Rc::clone(on_action);
    Button::new(id.into())
        .ghost()
        .xsmall()
        .icon(icon)
        .tooltip(tooltip)
        .disabled(!enabled)
        .on_click(move |_, window, cx| on_action(action.clone(), window, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(tiles: usize) -> AppsProps {
        AppsProps {
            categories: vec![CategoryRow {
                id: FilterId::Category("AudioVideo".into()),
                label: "Audio Video".into(),
                icon: gpui_kit::assets::IconName::Clapperboard,
                count: Some(2),
                selected: true,
            }],
            filter: FilterId::Category("AudioVideo".into()),
            search: String::new(),
            tiles: Rc::new(
                (0..tiles)
                    .map(|i| AppTile {
                        name: format!("a{i}"),
                        full_name: format!("A{i}"),
                        initial: 'A',
                        icon: None,
                        want_icon: false,
                        badge: Badge::None,
                        progress: None,
                        tooltip: String::new(),
                        selected: false,
                    })
                    .collect(),
            ),
            columns: 3,
            inspector: None,
            activity: Vec::new(),
            catalog: CatalogView::Ready,
            installed_count: Some(1),
            notice: None,
            banner: None,
            enabled: true,
            focused: true,
            width: px(980.),
        }
    }

    #[test]
    fn columns_fit_the_width() {
        // Three tiles and two gaps, plus the padding.
        let three = 3. * TILE_WIDTH + 2. * TILE_GAP + 2. * GRID_PADDING;
        assert_eq!(columns(px(three)), 3);
        assert_eq!(columns(px(three - 1.)), 2);
        assert_eq!(columns(px(1400.)), 8);
        assert_eq!(columns(px(200.)), 2);
        // The default window: 980 less the 188 pt list, the 300 pt inspector and dividers.
        assert_eq!(columns(px(980. - 188. - 300. - 2.)), 3);
        assert_eq!(pane_widths(px(980.)), (188., 300.));
        assert_eq!(pane_widths(px(720.)), (168., 280.));
        assert_eq!(pane_widths(px(810.)), (188., 280.));
    }

    #[test]
    fn the_toolbar_and_status_bar_name_what_is_shown() {
        let mut p = props(2);
        assert_eq!(title(&p), "Audio Video");
        assert_eq!(status_text(&p), "2 apps · 1 installed");
        p.installed_count = None;
        assert_eq!(status_text(&p), "2 apps");
        p.search = "code".into();
        assert_eq!(title(&p), "Search Results");
        p.catalog = CatalogView::Loading;
        assert_eq!(title(&p), "Apps");
        assert_eq!(status_text(&p), "");
        assert_eq!(app_count(1), "1 app");
    }

    #[test]
    fn empty_states_follow_the_filter_and_search() {
        let mut p = props(0);
        p.search = "zzz".into();
        let (_, text, link) = empty_state(&p).unwrap();
        assert_eq!(text, "No apps match “zzz”.");
        assert_eq!(link.map(|(label, _)| label), Some("Clear Search"));
        p.search.clear();
        p.filter = FilterId::Installed;
        assert_eq!(empty_state(&p).unwrap().1, "No apps installed yet.");
        p.installed_count = None;
        assert_eq!(
            empty_state(&p).unwrap().1,
            "The session hasn’t said which apps are installed."
        );
        p.filter = FilterId::All;
        assert_eq!(empty_state(&p).unwrap().1, "No apps in this category.");
        assert!(empty_state(&props(1)).is_none());
    }

    #[test]
    fn banners_read_as_designed() {
        assert_eq!(Banner::Waiting.text(), "Waiting for the session…");
        assert!(Banner::CommandsDisabled
            .text()
            .contains("remote commands are disabled"));
    }
}
