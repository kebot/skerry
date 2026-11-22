//! The Storage window's own parts (`ARCH-037`): the sidebar of Shared Files, homes and app
//! homes, the quota block at its foot, and the New Home sheet. The right side is the Files
//! pane (`files.rs`). Pure presentation: `skerry-mac` owns the data and the requests.

use crate::components;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    menu::{ContextMenuExt as _, PopupMenu},
    progress::Progress,
    spinner::Spinner,
    tooltip::Tooltip,
    Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{
    div, prelude::*, px, rgba, App, Div, Entity, FontWeight, Hsla, MouseButton, SharedString,
    Window,
};
use std::rc::Rc;

/// The sidebar's width.
pub const SIDEBAR_WIDTH: f32 = 210.;

/// A sidebar row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarRow {
    Shared,
    /// A home, by its full name (`work`, `auto-zen`).
    Home(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HomesState {
    Loading,
    Failed,
    Ready {
        named: Vec<String>,
        /// `auto-*`, labelled without the prefix.
        app: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaTone {
    Normal,
    Warning,
    Full,
}

#[derive(Clone, Debug, PartialEq)]
pub enum QuotaState {
    /// The first measure, with nothing to show yet.
    Measuring,
    Failed,
    Ready {
        text: String,
        fraction: Option<f32>,
        tone: QuotaTone,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SidebarProps {
    pub selected: SidebarRow,
    pub homes: HomesState,
    /// The home being checked or deleted: a spinner, not clickable.
    pub deleting: Option<String>,
    /// The window is key: the selection shows in the accent colour.
    pub focused: bool,
    pub quota: QuotaState,
    /// "Measured just now", "Measured 3 min ago", "Click to try again".
    pub quota_tooltip: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageAction {
    Select(SidebarRow),
    NewHome,
    RetryHomes,
    Measure,
    CreateHome,
    CancelSheet,
}

pub type StorageHandler = Rc<dyn Fn(StorageAction, &mut Window, &mut App)>;
/// Builds a sidebar row's right-click menu (the row is selected first).
pub type RowMenu = Rc<dyn Fn(PopupMenu, &SidebarRow) -> PopupMenu>;

/// An app home's label: its name without `auto-`.
pub fn app_home_label(home: &str) -> &str {
    home.strip_prefix("auto-")
        .filter(|rest| !rest.is_empty())
        .unwrap_or(home)
}

pub fn render_sidebar(
    theme: &Theme,
    props: &SidebarProps,
    on_action: StorageHandler,
    row_menu: RowMenu,
) -> Div {
    let mut list = div().v_flex().px(px(6.)).pb_2();
    list = list.child(section_header(theme, "Shared")).child(row(
        theme,
        props,
        SidebarRow::Shared,
        "Shared Files".into(),
        None,
        &on_action,
        &row_menu,
    ));
    match &props.homes {
        HomesState::Loading => {
            list = list.child(section_header(theme, "Homes")).child(
                div()
                    .h(px(28.))
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(Spinner::new().xsmall())
                    .child("Loading homes…"),
            );
        }
        HomesState::Failed => {
            let retry = Rc::clone(&on_action);
            list = list.child(section_header(theme, "Homes")).child(
                div()
                    .h(px(28.))
                    .h_flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Couldn’t load homes")
                    .child(
                        Button::new("storage-homes-retry")
                            .ghost()
                            .xsmall()
                            .icon(IconName::RefreshCw)
                            .tooltip("Try Again")
                            .on_click(move |_, window, cx| {
                                retry(StorageAction::RetryHomes, window, cx)
                            }),
                    ),
            );
        }
        HomesState::Ready { named, app } => {
            if !named.is_empty() {
                list = list.child(section_header(theme, "Homes"));
                for home in named {
                    list = list.child(row(
                        theme,
                        props,
                        SidebarRow::Home(home.clone()),
                        home.clone(),
                        None,
                        &on_action,
                        &row_menu,
                    ));
                }
            }
            if !app.is_empty() {
                list = list.child(section_header(theme, "App Homes"));
                for home in app {
                    list = list.child(row(
                        theme,
                        props,
                        SidebarRow::Home(home.clone()),
                        app_home_label(home).to_string(),
                        Some(home.clone()),
                        &on_action,
                        &row_menu,
                    ));
                }
            }
        }
    }
    let new_home = Rc::clone(&on_action);
    let empty_menu = Rc::clone(&row_menu);
    let selected = props.selected.clone();
    div()
        .w(px(SIDEBAR_WIDTH))
        .flex_none()
        .h_full()
        .v_flex()
        .bg(theme.sidebar)
        .border_r_1()
        .border_color(theme.border)
        .child(
            div()
                .id("storage-sidebar")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(list)
                .context_menu(move |menu, _, _| empty_menu(menu, &selected)),
        )
        .child(
            div()
                .flex_none()
                .v_flex()
                .gap_2()
                .p(px(10.))
                .border_t_1()
                .border_color(theme.border)
                .child(
                    div().child(
                        Button::new("storage-new-home")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip("New Home…")
                            .disabled(props.homes == HomesState::Loading)
                            .on_click(move |_, window, cx| {
                                new_home(StorageAction::NewHome, window, cx)
                            }),
                    ),
                )
                .child(quota_block(theme, props, &on_action)),
        )
}

fn section_header(theme: &Theme, label: &'static str) -> Div {
    div()
        .pt(px(14.))
        .pb(px(4.))
        .pl(px(8.))
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.muted_foreground)
        .child(label)
}

fn row(
    theme: &Theme,
    props: &SidebarProps,
    which: SidebarRow,
    label: String,
    tooltip: Option<String>,
    on_action: &StorageHandler,
    row_menu: &RowMenu,
) -> impl IntoElement {
    let selected = props.selected == which;
    let deleting =
        matches!(&which, SidebarRow::Home(home) if props.deleting.as_ref() == Some(home));
    let (background, foreground): (Option<Hsla>, Hsla) = match (selected, props.focused) {
        (true, true) => (Some(theme.accent), theme.accent_foreground),
        (true, false) => (Some(theme.secondary), theme.foreground),
        _ => (None, theme.foreground),
    };
    let icon = match &which {
        SidebarRow::Shared => Icon::new(gpui_kit::assets::IconName::FolderHeart),
        SidebarRow::Home(home) if home.starts_with("auto-") => {
            Icon::new(gpui_kit::assets::IconName::LayoutGrid)
        }
        SidebarRow::Home(_) => Icon::new(gpui_kit::assets::IconName::HardDrive),
    };
    let icon_tint = if selected && props.focused {
        foreground
    } else if which == SidebarRow::Shared {
        theme.accent
    } else {
        theme.muted_foreground
    };
    let id = SharedString::from(match &which {
        SidebarRow::Shared => "storage-row-shared".to_string(),
        SidebarRow::Home(home) => format!("storage-row-{home}"),
    });
    let click = Rc::clone(on_action);
    let press = Rc::clone(on_action);
    let menu = Rc::clone(row_menu);
    let click_row = which.clone();
    let press_row = which.clone();
    let menu_row = which.clone();
    let hover = theme.secondary.opacity(0.5);
    div()
        .id(id)
        .h(px(28.))
        .h_flex()
        .items_center()
        .gap_2()
        .px_2()
        .rounded(px(8.))
        .text_sm()
        .text_color(foreground)
        .when_some(background, |row, color| row.bg(color))
        .when(!selected, |row| row.hover(move |style| style.bg(hover)))
        .child(if deleting {
            Spinner::new().xsmall().into_any_element()
        } else {
            icon.size(px(16.)).text_color(icon_tint).into_any_element()
        })
        .child(div().flex_1().min_w_0().truncate().child(label))
        .when_some(tooltip, |row, text| {
            row.tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
        })
        .when(!deleting, |row| {
            row.cursor_pointer()
                .on_click(move |_, window, cx| {
                    click(StorageAction::Select(click_row.clone()), window, cx)
                })
                // Right-click selects the row first, as in Finder.
                .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                    press(StorageAction::Select(press_row.clone()), window, cx)
                })
        })
        .context_menu(move |popup, _, _| menu(popup, &menu_row))
}

fn quota_block(
    theme: &Theme,
    props: &SidebarProps,
    on_action: &StorageHandler,
) -> impl IntoElement {
    let measure = Rc::clone(on_action);
    let tooltip = props.quota_tooltip.clone();
    let mut block = div()
        .id("storage-quota")
        .v_flex()
        .gap_1()
        .cursor_pointer()
        .text_xs()
        .child(div().text_color(theme.muted_foreground).child("Storage"))
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .on_click(move |_, window, cx| measure(StorageAction::Measure, window, cx));
    match &props.quota {
        QuotaState::Measuring => {
            block = block.child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(Spinner::new().xsmall())
                    .child("Measuring…"),
            );
        }
        QuotaState::Failed => {
            block = block.child(
                div()
                    .text_color(theme.muted_foreground)
                    .child("Couldn’t measure storage"),
            );
        }
        QuotaState::Ready {
            text,
            fraction,
            tone,
        } => {
            if let Some(fraction) = fraction {
                let color = match tone {
                    QuotaTone::Normal => theme.accent,
                    QuotaTone::Warning => theme.warning,
                    QuotaTone::Full => theme.danger,
                };
                block = block.child(
                    Progress::new("storage-quota-meter")
                        .h(px(4.))
                        .color(color)
                        .value(fraction * 100.),
                );
            }
            block = block.child(div().child(text.clone()));
            if *tone == QuotaTone::Full {
                block = block.child(
                    div()
                        .text_color(theme.danger)
                        .child("Storage is full. Delete files to upload again."),
                );
            }
        }
    }
    block
}

/// What the New Home sheet shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewHomeProps {
    /// The danger line under the field, in place of the hint.
    pub error: Option<String>,
    /// Create is enabled.
    pub valid: bool,
    pub creating: bool,
}

/// The New Home sheet over the window, near its top.
pub fn render_new_home(
    theme: &Theme,
    props: &NewHomeProps,
    input: &Entity<InputState>,
    on_action: StorageHandler,
) -> gpui_kit::Stateful<Div> {
    let cancel = Rc::clone(&on_action);
    let create = Rc::clone(&on_action);
    let backdrop = Rc::clone(&on_action);
    let (line, line_color) = match &props.error {
        Some(error) => (error.clone(), theme.danger),
        None => (
            "Letters, numbers, - and _ only.".to_string(),
            theme.muted_foreground,
        ),
    };
    div()
        .id("storage-sheet-backdrop")
        .absolute()
        .inset_0()
        .bg(rgba(0x00000066))
        .flex()
        .justify_center()
        .pt(px(48.))
        .occlude()
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            if !window.default_prevented() {
                backdrop(StorageAction::CancelSheet, window, cx)
            }
        })
        .child(
            div()
                .id("storage-new-home")
                .w(px(380.))
                .h_flex()
                .items_start()
                .child(
                    div()
                        .w_full()
                        .v_flex()
                        .gap_3()
                        .p_5()
                        .rounded_xl()
                        .bg(theme.popover)
                        .border_1()
                        .border_color(theme.border)
                        .shadow(components::elevated_shadow(theme))
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("New Home"),
                        )
                        .child(
                            div().text_sm().text_color(theme.muted_foreground).child(
                                "A home keeps an app’s settings and files between sessions.",
                            ),
                        )
                        .child(
                            div()
                                .v_flex()
                                .gap_1()
                                .child(div().text_sm().child("Name"))
                                .child(Input::new(input).disabled(props.creating))
                                .child(div().text_xs().text_color(line_color).child(line)),
                        )
                        .child(
                            div()
                                .h_flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("storage-new-home-cancel")
                                        .outline()
                                        .small()
                                        .label("Cancel")
                                        .disabled(props.creating)
                                        .on_click(move |_, window, cx| {
                                            cancel(StorageAction::CancelSheet, window, cx)
                                        }),
                                )
                                .child(
                                    Button::new("storage-new-home-create")
                                        .primary()
                                        .small()
                                        .loading(props.creating)
                                        .label(if props.creating {
                                            "Creating…"
                                        } else {
                                            "Create"
                                        })
                                        .disabled(!props.valid || props.creating)
                                        .on_click(move |_, window, cx| {
                                            create(StorageAction::CreateHome, window, cx)
                                        }),
                                ),
                        ),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_homes_drop_their_prefix() {
        assert_eq!(app_home_label("auto-zen"), "zen");
        assert_eq!(app_home_label("work"), "work");
        assert_eq!(app_home_label("auto-"), "auto-");
    }
}
