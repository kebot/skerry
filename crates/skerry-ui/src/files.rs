//! The session's Files window (`ARCH-023`): a Finder-style list of the session's file-manager
//! folder with breadcrumbs, Back/Forward, sorting, multi-select, New Folder, Upload, Download,
//! Delete, and a transfers list. Pure presentation: `skerry-mac` owns the data, the transfers,
//! and the macOS bridges (drops, pasteboard, file promises, Quick Look).

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    progress::Progress,
    spinner::Spinner,
    Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{
    div, prelude::*, px, uniform_list, App, ClickEvent, Div, Entity, ExternalPaths, FontWeight,
    Hsla, MouseButton, MouseMoveEvent, SharedString, UniformListScrollHandle, Window,
};
use std::path::PathBuf;
use std::rc::Rc;

pub type FilesHandler = Rc<dyn Fn(FilesAction, &mut Window, &mut App)>;

/// Height of one list row; the list is a uniform list, so every row has it.
pub const ROW_HEIGHT: f32 = 24.;
/// Width of the Date Modified column, shared by the header and the rows.
const DATE_WIDTH: f32 = 170.;
/// Width of the Size column, shared by the header and the rows.
const SIZE_WIDTH: f32 = 72.;
/// Width of a row's file icon, and of the empty header cell above it.
const ICON_WIDTH: f32 = 16.;
/// Gap between a row's columns, shared by the header and the rows.
const COLUMN_GAP: f32 = 8.;
/// The transfers list shows about four rows before it scrolls.
const TRANSFERS_MAX_HEIGHT: f32 = 4.5 * 28.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortColumn {
    Name,
    Modified,
    Size,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crumb {
    pub label: String,
    /// Root-relative path the crumb opens.
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRow {
    pub name: String,
    pub is_dir: bool,
    pub modified: String,
    pub size: String,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TransferStatus {
    /// `fraction` in `0.0..=1.0`, or `None` while the size is not known yet.
    Running {
        fraction: Option<f32>,
        detail: String,
    },
    Done(String),
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransferRow {
    pub id: u64,
    pub name: String,
    pub direction: Direction,
    pub status: TransferStatus,
    /// A finished download that can be shown in Finder.
    pub revealable: bool,
}

/// A one-line message about the last action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    /// A failure: tinted and kept until dismissed; information fades on its own.
    pub error: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FilesProps {
    pub crumbs: Vec<Crumb>,
    pub can_back: bool,
    pub can_forward: bool,
    pub can_upload: bool,
    pub can_download: bool,
    /// The session's home can delete what is selected (`ARCH-024`).
    pub can_delete: bool,
    pub rows: Rc<Vec<FileRow>>,
    pub sort: SortColumn,
    pub ascending: bool,
    pub loading: bool,
    /// Why the folder could not be listed.
    pub error: Option<String>,
    pub notice: Option<Notice>,
    /// Work in progress shown in the status bar with a spinner (deleting, …).
    pub busy: Option<String>,
    pub transfers: Vec<TransferRow>,
    pub transfers_open: bool,
    /// The New Folder name field is showing.
    pub creating_folder: bool,
    pub selected: usize,
    /// The window is key: selection shows in the accent colour, otherwise in grey.
    pub focused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilesAction {
    Back,
    Forward,
    Navigate(String),
    Refresh,
    Sort(SortColumn),
    /// A row was clicked: `extend` (Shift) selects the range, `toggle` (Command) flips one.
    Select {
        index: usize,
        extend: bool,
        toggle: bool,
    },
    /// Double-click or Return: a folder opens, a file opens in its Mac app.
    Open(usize),
    ClearSelection,
    /// The mouse went down on row `index`; a drag may follow.
    Press(usize),
    /// Row `index` was right-clicked: it joins the selection the context menu acts on.
    ContextRow(usize),
    /// The pointer moved with the button held after a [`FilesAction::Press`]: drag the
    /// selection out to Finder.
    DragOut(usize),
    /// Finder files were dropped onto the folder in row `index`.
    DropInto {
        index: usize,
        paths: Vec<PathBuf>,
    },
    NewFolder,
    ConfirmNewFolder,
    CancelNewFolder,
    Upload,
    /// Download the selection to `~/Downloads`.
    Download,
    /// Download the selection to a folder the person chooses.
    DownloadTo,
    QuickLook,
    /// Delete the selection, after a confirmation.
    Delete,
    CancelTransfer(u64),
    RevealTransfer(u64),
    ClearTransfers,
    ToggleTransfers,
    DismissNotice,
}

pub fn render(
    theme: &Theme,
    props: &FilesProps,
    folder_name: &Entity<InputState>,
    scroll: &UniformListScrollHandle,
    on_action: FilesHandler,
) -> Div {
    div()
        .v_flex()
        .size_full()
        .child(toolbar(theme, props, &on_action))
        .when(props.creating_folder, |view| {
            view.child(new_folder_row(theme, folder_name, &on_action))
        })
        .when_some(props.notice.clone(), |view, notice| {
            view.child(notice_row(theme, notice, &on_action))
        })
        .child(header(theme, props, &on_action))
        .child(body(theme, props, scroll, &on_action))
        .when(!props.transfers.is_empty(), |view| {
            view.child(transfers(theme, props, &on_action))
        })
        .child(status_bar(theme, props))
}

fn toolbar(theme: &Theme, props: &FilesProps, on_action: &FilesHandler) -> Div {
    let selected = props.selected > 0;
    div()
        .flex_none()
        .h_flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_1p5()
        .border_b_1()
        .border_color(theme.border)
        .child(icon_button(
            "files-back",
            IconName::ChevronLeft,
            "Back (⌘[)",
            props.can_back,
            FilesAction::Back,
            on_action,
        ))
        .child(icon_button(
            "files-forward",
            IconName::ChevronRight,
            "Forward (⌘])",
            props.can_forward,
            FilesAction::Forward,
            on_action,
        ))
        .child(crumbs(theme, &props.crumbs, on_action))
        .child(icon_button(
            "files-refresh",
            IconName::RefreshCw,
            "Refresh (⌘R)",
            !props.loading,
            FilesAction::Refresh,
            on_action,
        ))
        .when(props.can_upload, |bar| {
            bar.child(icon_button(
                "files-new-folder",
                gpui_kit::assets::IconName::FolderPlus,
                "New Folder (⇧⌘N)",
                !props.creating_folder,
                FilesAction::NewFolder,
                on_action,
            ))
            .child(text_button(
                "files-upload",
                "Upload…",
                "Upload files from this Mac (⇧⌘U)",
                true,
                FilesAction::Upload,
                on_action,
            ))
        })
        .when(props.can_download, |bar| {
            bar.child(text_button(
                "files-download",
                "Download",
                "Download to your Downloads folder",
                selected,
                FilesAction::Download,
                on_action,
            ))
        })
        .when(props.can_delete, |bar| {
            // Kept apart at the end, where a slip of the pointer cannot reach it.
            bar.child(div().w(px(8.))).child(icon_button(
                "files-delete",
                gpui_kit::assets::IconName::Trash,
                "Delete (⌘⌫)",
                selected,
                FilesAction::Delete,
                on_action,
            ))
        })
}

fn crumbs(theme: &Theme, crumbs: &[Crumb], on_action: &FilesHandler) -> Div {
    let last = crumbs.len().saturating_sub(1);
    div()
        .flex_1()
        .min_w_0()
        .h_flex()
        .items_center()
        .gap_0p5()
        .px_2()
        .overflow_hidden()
        .children(crumbs.iter().enumerate().map(|(index, crumb)| {
            let on_action = Rc::clone(on_action);
            let path = crumb.path.clone();
            let current = index == last;
            div()
                .h_flex()
                .items_center()
                .gap_0p5()
                .min_w_0()
                // The current folder keeps its width; ancestors give way first.
                .when(current, |crumb| crumb.flex_none())
                .when(index > 0, |row| {
                    row.child(div().text_color(theme.muted_foreground).child("›"))
                })
                .child(
                    div()
                        .id(SharedString::from(format!("crumb-{index}")))
                        .px_1()
                        .rounded_sm()
                        .truncate()
                        .when(current, |label| label.font_weight(FontWeight::SEMIBOLD))
                        .when(!current, |label| {
                            label
                                .text_color(theme.muted_foreground)
                                .cursor_pointer()
                                .hover(|style| style.bg(theme.secondary_hover))
                                .on_click(move |_, window, cx| {
                                    on_action(FilesAction::Navigate(path.clone()), window, cx)
                                })
                        })
                        .child(crumb.label.clone()),
                )
        }))
}

fn new_folder_row(theme: &Theme, name: &Entity<InputState>, on_action: &FilesHandler) -> Div {
    let create = Rc::clone(on_action);
    div()
        .flex_none()
        .h_flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .child(div().flex_1().child(Input::new(name).small()))
        .child(text_button(
            "files-new-folder-cancel",
            "Cancel",
            "Cancel (Esc)",
            true,
            FilesAction::CancelNewFolder,
            on_action,
        ))
        .child(
            Button::new("files-new-folder-create")
                .primary()
                .small()
                .label("Create")
                .on_click(move |_, window, cx| create(FilesAction::ConfirmNewFolder, window, cx)),
        )
}

fn notice_row(theme: &Theme, notice: Notice, on_action: &FilesHandler) -> Div {
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
            "files-notice-dismiss",
            IconName::Close,
            "Dismiss",
            true,
            FilesAction::DismissNotice,
            on_action,
        ))
}

fn header(theme: &Theme, props: &FilesProps, on_action: &FilesHandler) -> Div {
    let column = |id: &'static str, label: &'static str, column: SortColumn| {
        let on_action = Rc::clone(on_action);
        let sorted = props.sort == column;
        div()
            .id(id)
            .h_flex()
            .items_center()
            .gap_1()
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.bg(theme.secondary_hover))
            .when(sorted, |cell| {
                cell.font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.foreground)
            })
            .child(label)
            .when(sorted, |cell| {
                cell.child(
                    Icon::new(if props.ascending {
                        IconName::ChevronUp
                    } else {
                        IconName::ChevronDown
                    })
                    .xsmall(),
                )
            })
            .on_click(move |_, window, cx| on_action(FilesAction::Sort(column), window, cx))
    };
    // The same columns as a row, with an empty cell where a row has its icon, so every label
    // sits over its values.
    div()
        .flex_none()
        .h_flex()
        .items_center()
        .gap(px(COLUMN_GAP))
        .px_3()
        .py_0p5()
        .text_xs()
        .text_color(theme.muted_foreground)
        .border_b_1()
        .border_color(theme.border)
        .child(div().w(px(ICON_WIDTH)).flex_none())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(column("sort-name", "Name", SortColumn::Name)),
        )
        .child(div().w(px(DATE_WIDTH)).flex_none().child(column(
            "sort-modified",
            "Date Modified",
            SortColumn::Modified,
        )))
        .child(
            div()
                .w(px(SIZE_WIDTH))
                .flex_none()
                .h_flex()
                .justify_end()
                .child(column("sort-size", "Size", SortColumn::Size)),
        )
}

fn body(
    theme: &Theme,
    props: &FilesProps,
    scroll: &UniformListScrollHandle,
    on_action: &FilesHandler,
) -> Div {
    let container = div().flex_1().min_h_0().relative();
    if let Some(error) = &props.error {
        let retry = Rc::clone(on_action);
        return container.child(
            placeholder(theme, error.clone()).child(
                Button::new("files-try-again")
                    .outline()
                    .small()
                    .label("Try Again")
                    .on_click(move |_, window, cx| retry(FilesAction::Refresh, window, cx)),
            ),
        );
    }
    if props.rows.is_empty() {
        if props.loading {
            return container
                .child(placeholder(theme, "Loading…".into()).child(Spinner::new().small()));
        }
        let message = if props.can_upload {
            "This folder is empty. Drop files here to upload them."
        } else {
            "This folder is empty."
        };
        return container.child(placeholder(theme, message.into()));
    }
    let rows = Rc::clone(&props.rows);
    let theme_row = theme.clone();
    let on_rows = Rc::clone(on_action);
    let on_background = Rc::clone(on_action);
    let focused = props.focused;
    let droppable = props.can_upload;
    container
        .child(
            uniform_list("files-list", rows.len(), move |range, _, _| {
                range
                    .map(|index| {
                        row(
                            &theme_row,
                            index,
                            &rows[index],
                            focused,
                            droppable,
                            &on_rows,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(scroll)
            .size_full(),
        )
        .on_mouse_down(MouseButton::Left, {
            let on_background = Rc::clone(&on_background);
            move |_, window, cx| {
                // A click below the last row clears the selection, as in Finder; a row's own
                // mouse-down marks the event handled first.
                if !window.default_prevented() {
                    on_background(FilesAction::ClearSelection, window, cx);
                }
            }
        })
        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
            if !window.default_prevented() {
                on_background(FilesAction::ClearSelection, window, cx);
            }
        })
}

/// `name` as the part that may shrink and the extension that always shows, as Finder keeps
/// `….pdf` visible on a truncated name.
fn split_extension(name: &str, is_dir: bool) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if !is_dir && dot > 0 && name.len() - dot <= 8 => name.split_at(dot),
        _ => (name, ""),
    }
}

fn row(
    theme: &Theme,
    index: usize,
    file: &FileRow,
    focused: bool,
    droppable: bool,
    on_action: &FilesHandler,
) -> impl IntoElement {
    let on_click = Rc::clone(on_action);
    let on_move = Rc::clone(on_action);
    let on_press = Rc::clone(on_action);
    let on_drop = Rc::clone(on_action);
    let on_context = Rc::clone(on_action);
    let highlighted = file.selected && focused;
    let (background, foreground, secondary): (Option<Hsla>, Hsla, Hsla) = if highlighted {
        (
            Some(theme.blue),
            gpui_kit::white(),
            gpui_kit::white().opacity(0.85),
        )
    } else if file.selected {
        (Some(theme.border), theme.foreground, theme.muted_foreground)
    } else {
        (
            (index % 2 == 1).then_some(theme.muted),
            theme.foreground,
            theme.muted_foreground,
        )
    };
    let (stem, extension) = split_extension(&file.name, file.is_dir);
    let ring = theme.blue;
    div()
        .id(("file-row", index))
        .w_full()
        .h(px(ROW_HEIGHT))
        .h_flex()
        .items_center()
        .px_3()
        .gap(px(COLUMN_GAP))
        .text_color(foreground)
        .when_some(background, |row, color| row.bg(color))
        .child(
            Icon::new(if file.is_dir {
                IconName::Folder
            } else {
                IconName::File
            })
            .size(px(ICON_WIDTH))
            .flex_none()
            .text_color(if highlighted {
                foreground
            } else if file.is_dir {
                theme.blue
            } else {
                theme.muted_foreground
            }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_flex()
                .child(div().min_w_0().truncate().child(stem.to_owned()))
                .child(div().flex_none().child(extension.to_owned())),
        )
        .child(
            div()
                .w(px(DATE_WIDTH))
                .flex_none()
                .text_color(secondary)
                .truncate()
                .child(file.modified.clone()),
        )
        .child(
            div()
                .w(px(SIZE_WIDTH))
                .flex_none()
                .h_flex()
                .justify_end()
                .text_color(secondary)
                .child(file.size.clone()),
        )
        .when(file.is_dir && droppable, |row| {
            row.drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(ring.opacity(0.25)))
                .on_drop(move |paths: &ExternalPaths, window, cx| {
                    on_drop(
                        FilesAction::DropInto {
                            index,
                            paths: paths.paths().to_vec(),
                        },
                        window,
                        cx,
                    );
                })
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.prevent_default();
            on_press(FilesAction::Press(index), window, cx);
        })
        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
            window.prevent_default();
            on_context(FilesAction::ContextRow(index), window, cx);
        })
        .on_click(move |event: &ClickEvent, window, cx| {
            let modifiers = event.modifiers();
            if event.click_count() >= 2 {
                on_click(FilesAction::Open(index), window, cx);
            } else {
                on_click(
                    FilesAction::Select {
                        index,
                        extend: modifiers.shift,
                        toggle: modifiers.platform,
                    },
                    window,
                    cx,
                );
            }
        })
        .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
            if event.pressed_button == Some(MouseButton::Left) {
                on_move(FilesAction::DragOut(index), window, cx);
            }
        })
}

fn placeholder(theme: &Theme, message: String) -> Div {
    div()
        .size_full()
        .v_flex()
        .gap_3()
        .items_center()
        .justify_center()
        .p_6()
        .text_center()
        .text_color(theme.muted_foreground)
        .child(message)
}

fn status_bar(theme: &Theme, props: &FilesProps) -> Div {
    let count = props.rows.len();
    let items = if count == 1 {
        "1 item".to_owned()
    } else {
        format!("{count} items")
    };
    let text = if props.error.is_some() || (props.loading && count == 0) {
        String::new()
    } else if props.selected > 0 {
        format!("{} of {items} selected", props.selected)
    } else {
        items
    };
    let activity = props
        .busy
        .clone()
        .or_else(|| (props.loading && count > 0).then(|| "Loading…".to_owned()));
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
        .child(text)
        .when_some(activity, |bar, activity| {
            bar.child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(Spinner::new().xsmall())
                    .child(activity),
            )
        })
}

fn transfers(theme: &Theme, props: &FilesProps, on_action: &FilesHandler) -> Div {
    let rows = &props.transfers;
    let running = rows
        .iter()
        .filter(|row| matches!(row.status, TransferStatus::Running { .. }))
        .count();
    let finished = running < rows.len();
    let toggle = Rc::clone(on_action);
    let heading = if running > 0 {
        format!("Transfers ({running} in progress)")
    } else {
        format!("Transfers ({})", rows.len())
    };
    div()
        .v_flex()
        .flex_none()
        .border_t_1()
        .border_color(theme.border)
        .child(
            div()
                .h_flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_0p5()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(
                    div()
                        .id("files-transfers-toggle")
                        .h_flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .child(
                            Icon::new(if props.transfers_open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .xsmall(),
                        )
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(heading))
                        .on_click(move |_, window, cx| {
                            toggle(FilesAction::ToggleTransfers, window, cx)
                        }),
                )
                .when(finished && props.transfers_open, |bar| {
                    bar.child(
                        Button::new("files-transfers-clear")
                            .ghost()
                            .xsmall()
                            .label("Clear Finished")
                            .on_click({
                                let on_action = Rc::clone(on_action);
                                move |_, window, cx| {
                                    on_action(FilesAction::ClearTransfers, window, cx)
                                }
                            }),
                    )
                }),
        )
        .when(props.transfers_open, |panel| {
            panel.child(
                div()
                    .id("files-transfers")
                    .v_flex()
                    .max_h(px(TRANSFERS_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .children(rows.iter().map(|row| transfer(theme, row, on_action))),
            )
        })
}

/// One transfer on one line: direction, name, progress or outcome, and its actions.
fn transfer(theme: &Theme, row: &TransferRow, on_action: &FilesHandler) -> Div {
    let arrow = match row.direction {
        Direction::Upload => IconName::ArrowUp,
        Direction::Download => IconName::ArrowDown,
    };
    let running = matches!(row.status, TransferStatus::Running { .. });
    let (detail, color) = match &row.status {
        TransferStatus::Running { detail, .. } => (detail.clone(), theme.muted_foreground),
        TransferStatus::Done(detail) => (detail.clone(), theme.muted_foreground),
        TransferStatus::Failed(error) => (error.clone(), theme.danger),
        TransferStatus::Cancelled => ("Cancelled".into(), theme.muted_foreground),
    };
    div()
        .h(px(28.))
        .h_flex()
        .items_center()
        .gap_2()
        .px_3()
        .child(Icon::new(arrow).xsmall().text_color(theme.muted_foreground))
        .child(
            div()
                .w(px(160.))
                .flex_shrink(1.)
                .min_w_0()
                .truncate()
                .child(row.name.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_flex()
                .items_center()
                .gap_2()
                .when_some(
                    match &row.status {
                        TransferStatus::Running { fraction, .. } => Some(*fraction),
                        _ => None,
                    },
                    |cell, fraction| {
                        cell.child(
                            div().w(px(90.)).flex_none().child(
                                Progress::new(("transfer-progress", row.id as usize))
                                    .loading(fraction.is_none())
                                    .value(fraction.unwrap_or(0.) * 100.),
                            ),
                        )
                    },
                )
                .child(
                    div()
                        .min_w_0()
                        .text_xs()
                        .text_color(color)
                        .truncate()
                        .child(detail),
                ),
        )
        .when(running, |cell| {
            cell.child(icon_button(
                SharedString::from(format!("transfer-cancel-{}", row.id)),
                IconName::CircleX,
                "Cancel",
                true,
                FilesAction::CancelTransfer(row.id),
                on_action,
            ))
        })
        .when(row.revealable, |cell| {
            cell.child(icon_button(
                SharedString::from(format!("transfer-reveal-{}", row.id)),
                IconName::Search,
                "Show in Finder",
                true,
                FilesAction::RevealTransfer(row.id),
                on_action,
            ))
        })
}

fn icon_button(
    id: impl Into<SharedString>,
    icon: impl Into<Icon>,
    tooltip: &'static str,
    enabled: bool,
    action: FilesAction,
    on_action: &FilesHandler,
) -> Button {
    let on_action = Rc::clone(on_action);
    Button::new(id.into())
        .ghost()
        .small()
        .icon(icon)
        .tooltip(tooltip)
        .disabled(!enabled)
        .on_click(move |_, window, cx| on_action(action.clone(), window, cx))
}

fn text_button(
    id: &'static str,
    label: &'static str,
    tooltip: &'static str,
    enabled: bool,
    action: FilesAction,
    on_action: &FilesHandler,
) -> Button {
    let on_action = Rc::clone(on_action);
    Button::new(id)
        .outline()
        .small()
        .label(label)
        .tooltip(tooltip)
        .disabled(!enabled)
        .on_click(move |_, window, cx| on_action(action.clone(), window, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_stay_visible_when_names_truncate() {
        assert_eq!(split_extension("report.pdf", false), ("report", ".pdf"));
        assert_eq!(
            split_extension("archive.tar.gz", false),
            ("archive.tar", ".gz")
        );
        assert_eq!(split_extension(".env", false), (".env", ""));
        assert_eq!(split_extension("v1.2", true), ("v1.2", ""));
        assert_eq!(
            split_extension("notes.thisisnotanextension", false),
            ("notes.thisisnotanextension", "")
        );
    }
}
