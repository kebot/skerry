use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    scroll::ScrollableElement as _,
    Disableable as _, IconName, Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{
    div, img, prelude::*, px, rgb, App, ClickEvent, Div, Entity, FontWeight, ObjectFit,
    RenderImage, Rgba, SharedString, Window,
};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

pub type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
pub type ItemHandler = Rc<dyn Fn(String, &mut Window, &mut App)>;

#[derive(Clone, Debug)]
pub struct AppItem {
    pub id: String,
    pub name: String,
    pub categories: Vec<String>,
    pub recent: bool,
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
}

#[derive(Clone, Debug, Default)]
pub struct LibraryProps {
    pub app_name: String,
    pub server: String,
    pub apps: Vec<AppItem>,
    pub sessions: Vec<SessionItem>,
    pub search_query: String,
    pub selected_filter: String,
    pub status: Option<String>,
    pub busy: Option<String>,
}

pub struct LibraryHandlers {
    pub on_refresh: ClickHandler,
    pub on_forget: ClickHandler,
    pub on_launch: ItemHandler,
    pub on_resume: ItemHandler,
    pub on_stop: Option<ItemHandler>,
    pub on_select_filter: ItemHandler,
}

/// Creates a borderless omnibar input configured for searching apps or pasting URLs.
pub fn new_search_input(window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder("Search applications and running sessions"))
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
        on_forget,
        on_launch,
        on_resume,
        on_stop,
        on_select_filter,
    } = handlers;

    let active_filter = if props.selected_filter.is_empty() {
        "All"
    } else {
        props.selected_filter.as_str()
    };

    let mut categories = BTreeSet::new();
    for app in &props.apps {
        for cat in &app.categories {
            if !cat.is_empty() {
                categories.insert(cat.clone());
            }
        }
    }
    let mut filters = vec!["All".to_string()];
    if props.apps.iter().any(|app| app.recent) || active_filter == "Recent" {
        filters.push("Recent".to_string());
    }
    filters.extend(categories);

    let query = props.search_query.trim().to_ascii_lowercase();
    let filtered_apps: Vec<&AppItem> = props
        .apps
        .iter()
        .filter(|app| {
            let matches_filter = match active_filter {
                "All" => true,
                "Recent" => app.recent,
                category => app.categories.iter().any(|c| c == category),
            };
            let matches_query = query.is_empty()
                || app.name.to_ascii_lowercase().contains(&query)
                || app
                    .categories
                    .iter()
                    .any(|c| c.to_ascii_lowercase().contains(&query));
            matches_filter && matches_query
        })
        .collect();

    let filtered_sessions: Vec<&SessionItem> = props
        .sessions
        .iter()
        .filter(|session| {
            query.is_empty() || session.app_name.to_ascii_lowercase().contains(&query)
        })
        .collect();

    let header = div()
        .h_flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(props.app_name.clone()),
                )
                .child(
                    div()
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
                        .child(IconName::Globe)
                        .child(props.server.clone()),
                ),
        )
        .child(
            div()
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
                    Button::new("forget")
                        .ghost()
                        .xsmall()
                        .label("Disconnect Server")
                        .tooltip("Disconnect and forget the saved application key")
                        .disabled(!enabled)
                        .on_click(move |event, window, cx| on_forget(event, window, cx)),
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
        .child(
            div()
                .text_color(theme.muted_foreground)
                .child(IconName::Search),
        )
        .child(
            div().flex_1().min_w_0().child(
                Input::new(search_input)
                    .appearance(false)
                    .cleanable(true)
                    .disabled(!enabled),
            ),
        );

    let sessions_section = div()
        .v_flex()
        .gap_3p5()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Running sessions"),
        )
        .when(filtered_sessions.is_empty(), |section| {
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
        })
        .when(!filtered_sessions.is_empty(), |section| {
            section.child(div().h_flex().flex_wrap().items_stretch().gap_4().children(
                filtered_sessions.iter().map(|session| {
                    render_session_card(
                        theme,
                        session,
                        enabled,
                        Rc::clone(&on_resume),
                        on_stop.clone(),
                    )
                }),
            ))
        });

    let filter_bar =
        div()
            .h_flex()
            .items_center()
            .flex_wrap()
            .gap_2()
            .children(filters.into_iter().map(|filter| {
                let selected = filter == active_filter;
                let on_select = Rc::clone(&on_select_filter);
                let value = filter.clone();
                let mut button = Button::new(SharedString::from(format!("filter-{filter}")))
                    .xsmall()
                    .rounded_full()
                    .label(filter)
                    .on_click(move |_, window, cx| on_select(value.clone(), window, cx));
                if selected {
                    button = button.primary();
                } else {
                    button = button.outline();
                }
                button
            }));

    let apps_section = div()
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
        .when(filtered_apps.is_empty(), |section| {
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
        .when(!filtered_apps.is_empty(), |section| {
            section.child(
                div().h_flex().flex_wrap().gap_4().children(
                    filtered_apps
                        .into_iter()
                        .map(|app| render_app_tile(theme, app, enabled, Rc::clone(&on_launch))),
                ),
            )
        });

    div()
        .size_full()
        .bg(theme.background)
        .text_color(theme.foreground)
        .child(
            div()
                .id("library")
                .size_full()
                .overflow_y_scrollbar()
                .child(
                    div()
                        .w_full()
                        .px_7()
                        .py_5()
                        .v_flex()
                        .gap_6()
                        .child(header)
                        .child(omnibar)
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
                        .child(apps_section),
                ),
        )
}

fn render_session_card(
    theme: &Theme,
    session: &SessionItem,
    enabled: bool,
    on_resume: ItemHandler,
    on_stop: Option<ItemHandler>,
) -> Div {
    let session_id = session.id.clone();
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

    div()
        .w(px(336.))
        .rounded_xl()
        .overflow_hidden()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .v_flex()
        .child(preview)
        // Fill the row height so cards in one row line up even when one card's pills wrap.
        .child(footer.flex_1())
}

fn render_app_tile(
    theme: &Theme,
    app: &AppItem,
    enabled: bool,
    on_launch: ItemHandler,
) -> gpui_kit::Stateful<Div> {
    let app_id = app.id.clone();
    let border_hover = theme.primary;
    let bg_hover = theme.secondary;

    div()
        .id(SharedString::from(format!("app-tile-{app_id}")))
        .w(px(156.))
        .h(px(142.))
        .p_4()
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .v_flex()
        .items_center()
        .justify_center()
        .gap_3()
        .when(enabled, |tile| {
            tile.cursor_pointer()
                .hover(move |s| s.border_color(border_hover).bg(bg_hover))
                .on_click(move |_, window, cx| on_launch(app_id.clone(), window, cx))
        })
        .when(!enabled, |tile| tile.opacity(0.6))
        .child(app_badge(&app.name, app.icon.clone(), px(56.)))
        .child(
            div()
                .w_full()
                .text_center()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .truncate()
                .child(app.name.clone()),
        )
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
