use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    Icon, IconName, Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{
    div, prelude::*, px, rgb, rgba, App, ClickEvent, Div, FontWeight, Pixels, Point, SharedString,
    Window,
};

/// Floating status banner shown over an active remote desktop session.
pub fn status_banner(theme: &Theme, status: impl Into<String>) -> Div {
    div()
        .absolute()
        .bottom_3()
        .left_3()
        .right_3()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .text_color(theme.foreground)
        .child(status.into())
}

/// Session status banner with Reconnect / Disconnect buttons, so a dropped or failed stream
/// can be recovered without knowing the Session Controls shortcut.
pub fn status_banner_with_actions(
    theme: &Theme,
    status: impl Into<String>,
    on_reconnect: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_disconnect: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_kit::Stateful<Div> {
    status_banner_labeled(
        theme,
        status,
        ("Reconnect", Box::new(on_reconnect)),
        ("Disconnect", Box::new(on_disconnect)),
    )
}

/// A banner button's label and click handler.
pub type BannerAction = (
    &'static str,
    Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
);

/// The status banner with its own two buttons: Display 2's "Try Again" / "Close"
/// (`ARCH-039`). The text wraps, and the banner grows with it.
pub fn status_banner_labeled(
    theme: &Theme,
    status: impl Into<String>,
    primary: BannerAction,
    secondary: BannerAction,
) -> gpui_kit::Stateful<Div> {
    let (primary_label, on_primary) = primary;
    let (secondary_label, on_secondary) = secondary;
    div()
        .id("session-status-banner")
        .absolute()
        .bottom_3()
        .left_3()
        .right_3()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .text_color(theme.foreground)
        .occlude()
        .h_flex()
        .items_center()
        .gap_3()
        .child(div().flex_1().min_w_0().child(status.into()))
        .child(
            Button::new("session-banner-reconnect")
                .primary()
                .small()
                .label(primary_label)
                .on_click(on_primary),
        )
        .child(
            Button::new("session-banner-disconnect")
                .outline()
                .small()
                .label(secondary_label)
                .on_click(on_secondary),
        )
}

/// The side picker's sides that are offered, with their labels, in `[Left, Right, Above,
/// Below]` order (`ARCH-039`).
pub fn offered_sides(sides: [bool; 4]) -> Vec<(usize, &'static str)> {
    const LABELS: [&str; 4] = ["Left", "Right", "Above", "Below"];
    (0..4)
        .filter(|&side| sides[side])
        .map(|side| (side, LABELS[side]))
        .collect()
}

/// The side an arrow key picks, if that side is offered. Other keys and sides do nothing.
pub fn placement_key(key: &str, sides: [bool; 4]) -> Option<usize> {
    let side = match key {
        "left" => 0,
        "right" => 1,
        "up" => 2,
        "down" => 3,
        _ => return None,
    };
    sides[side].then_some(side)
}

/// A picker callback for one side (its index).
pub type SideHandler = std::rc::Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// "Where is your second display?" over the session window (`ARCH-039`): a scrim, one
/// button on each offered edge, and a centre card with the arrangement. A click on the
/// scrim dismisses; nothing reaches the remote desktop while it is up.
pub fn screen_placement(
    theme: &Theme,
    sides: [bool; 4],
    hovered: Option<usize>,
    on_pick: SideHandler,
    on_hover: std::rc::Rc<dyn Fn(Option<usize>, &mut Window, &mut App)>,
    on_dismiss: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_kit::Stateful<Div> {
    let icons = [
        IconName::ChevronLeft,
        IconName::ChevronRight,
        IconName::ChevronUp,
        IconName::ChevronDown,
    ];
    let mut scrim = div()
        .id("screen-placement")
        .absolute()
        .inset_0()
        .bg(rgba(0x00000073))
        .occlude()
        .on_click(on_dismiss);
    for (side, label) in offered_sides(sides) {
        let pick = on_pick.clone();
        let hover = on_hover.clone();
        let button = div()
            .id(("placement-side", side))
            .size(px(88.))
            .rounded(px(16.))
            .border_1()
            .border_color(theme.border)
            .bg(if hovered == Some(side) {
                theme.secondary_hover
            } else {
                theme.secondary
            })
            .shadow(crate::components::elevated_shadow(theme))
            .occlude()
            .cursor_pointer()
            .v_flex()
            .items_center()
            .justify_center()
            .gap(px(4.))
            .child(
                Icon::new(icons[side].clone())
                    .size(px(32.))
                    .text_color(theme.foreground),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(label),
            )
            .on_hover(move |over, window, cx| hover(over.then_some(side), window, cx))
            .on_click(move |_, window, cx| pick(side, window, cx));
        // Centred on its edge, 40 pt in, by a wrapper with no hitbox of its own.
        let edge = div().absolute().flex().items_center().justify_center();
        let edge = match side {
            0 => edge.left(px(40.)).top_0().bottom_0(),
            1 => edge.right(px(40.)).top_0().bottom_0(),
            2 => edge.top(px(40.)).left_0().right_0(),
            _ => edge.bottom(px(40.)).left_0().right_0(),
        };
        scrim = scrim.child(edge.child(button));
    }
    scrim.child(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(placement_card(theme, sides, hovered)),
    )
}

/// The picker's centre card: the arrangement diagram (the session window as "1", the
/// offered sides as "2"), the title and the hint.
fn placement_card(
    theme: &Theme,
    sides: [bool; 4],
    hovered: Option<usize>,
) -> gpui_kit::Stateful<Div> {
    // The grid cell of each side: (row, column).
    const CELLS: [(usize, usize); 4] = [(1, 0), (1, 2), (0, 1), (2, 1)];
    let cell = |row: usize, column: usize| {
        let base = div()
            .w(px(64.))
            .h(px(40.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD);
        if (row, column) == (1, 1) {
            return base
                .relative()
                .bg(theme.primary)
                .text_color(rgb(0xffffff))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(3.))
                        .rounded_t(px(4.))
                        .bg(rgb(0xffffff)),
                )
                .child("1");
        }
        let Some(side) = CELLS.iter().position(|&at| at == (row, column)) else {
            return base;
        };
        if !sides[side] {
            return base;
        }
        // Outlined, not filled (the design's solid fallback for a dashed border).
        if hovered == Some(side) {
            base.border_1()
                .border_color(theme.primary)
                .bg(theme.primary.opacity(0.2))
                .text_color(theme.foreground)
                .child("2")
        } else {
            base.border_1()
                .border_color(theme.border)
                .bg(theme.muted)
                .text_color(theme.muted_foreground)
                .child("2")
        }
    };
    let diagram = div().v_flex().gap(px(6.)).children((0..3).map(|row| {
        div()
            .h_flex()
            .gap(px(6.))
            .children((0..3).map(move |column| cell(row, column)))
    }));
    div()
        .id("placement-card")
        .w(px(280.))
        .p(px(20.))
        .rounded(px(12.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .shadow(crate::components::elevated_shadow(theme))
        .occlude()
        .v_flex()
        .items_center()
        .gap(px(12.))
        .child(diagram)
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("Where is your second display?"),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .text_center()
                .child("Choose the side Display 2 sits on. Esc cancels."),
        )
}

/// What a print notice's or a Printing card row's button asks (`ARCH-033`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrintJobAction {
    Print,
    Open,
    Save,
    Reveal,
    Dismiss,
    Remove,
    /// The "+N more" line: Session Controls with Printing open (id 0).
    ShowAll,
}

/// One print notice's state, its sentences already written.
#[derive(Clone, Debug, PartialEq)]
pub enum PrintNoticeKind {
    /// `total` 0: the size is unknown.
    Receiving {
        received: u64,
        total: u64,
    },
    /// Ready, busy, or pending an auto-open: the detail says which. `saved` swaps Save… for
    /// Show in Finder.
    Ready {
        detail: SharedString,
        saved: bool,
    },
    Unreadable,
    Failed {
        detail: SharedString,
    },
    TooLarge {
        detail: SharedString,
    },
    Missing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrintNoticeProps {
    pub id: u64,
    pub name: SharedString,
    pub kind: PrintNoticeKind,
}

pub type PrintHandler = std::rc::Rc<dyn Fn(u64, PrintJobAction, &mut Window, &mut App)>;
pub type PrintHoverHandler = std::rc::Rc<dyn Fn(u64, bool, &mut Window, &mut App)>;

/// Below the ⋯ trigger (8 + 32 + 8), or below the stats strip when it reaches lower, on
/// the 4 pt grid.
pub fn print_notices_top(strip_bottom: Option<Pixels>) -> Pixels {
    let strip = strip_bottom.map_or(0., f32::from);
    px(48f32.max((strip / 4.).ceil() * 4.))
}

/// How many notice cards fit above the hint and banners: 104 pt per card, 112 pt kept at the
/// bottom, 28 pt for the "+N more" line; at least 1, at most 3.
pub fn print_notices_shown(window_height: Pixels, top: Pixels) -> usize {
    let room = f32::from(window_height) - f32::from(top) - 112. - 28.;
    ((room / 104.).floor().max(1.) as usize).min(3)
}

/// Windows narrower than this show the stack full width.
const PRINT_NARROW: f32 = 420.;

/// The print notices at the top right of the session window (`ARCH-033`), newest first,
/// with "+N more in Session Controls › Printing" past the ones shown. Only the cards and
/// that line take clicks.
pub fn print_notices(
    theme: &Theme,
    notices: &[PrintNoticeProps],
    more: usize,
    viewport_width: Pixels,
    top: Pixels,
    on_action: PrintHandler,
    on_hover: PrintHoverHandler,
) -> Div {
    let narrow = f32::from(viewport_width) < PRINT_NARROW;
    let stack = div()
        .absolute()
        .top(top)
        .right(px(12.))
        .v_flex()
        .items_end()
        .gap(px(8.));
    let stack = if narrow {
        stack.left(px(12.))
    } else {
        stack.w(px(340.))
    };
    stack
        .children(
            notices
                .iter()
                .map(|notice| print_notice(theme, notice, &on_action, &on_hover)),
        )
        .when(more > 0, |stack| {
            let on_action = on_action.clone();
            stack.child(
                div()
                    .id("print-more")
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .occlude()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(theme.link)
                    .child(format!("+{more} more in Session Controls › Printing"))
                    .on_click(move |_, window, cx| {
                        on_action(0, PrintJobAction::ShowAll, window, cx)
                    }),
            )
        })
}

/// Splits `name` into what may truncate and the extension that stays visible.
fn print_name_parts(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    }
}

/// A title with the name truncated in the middle: `prefix`, the stem (shrinks), then the
/// extension and `suffix`, which never shrink.
fn print_title(prefix: &str, name: &str, suffix: &str) -> Div {
    let (stem, ext) = print_name_parts(name);
    div()
        .h_flex()
        .min_w_0()
        .whitespace_nowrap()
        .when(!prefix.is_empty(), |row| {
            row.child(div().flex_none().child(prefix.to_string()))
        })
        .child(div().min_w_0().truncate().child(stem.to_string()))
        .child(div().flex_none().child(format!("{ext}{suffix}")))
}

/// The glyph and its tint for a state.
fn print_glyph(theme: &Theme, kind: &PrintNoticeKind) -> (Icon, gpui_kit::Hsla) {
    let file_x: Icon = gpui_kit::assets::IconName::FileX.into();
    match kind {
        PrintNoticeKind::Unreadable => (IconName::FileText.into(), theme.warning),
        PrintNoticeKind::Failed { .. } => (file_x, theme.danger),
        PrintNoticeKind::TooLarge { .. } => (IconName::TriangleAlert.into(), theme.warning),
        PrintNoticeKind::Missing => (file_x, theme.muted_foreground),
        _ => (IconName::FileText.into(), theme.muted_foreground),
    }
}

fn print_button(
    id: (&'static str, u64),
    label: &'static str,
    tooltip: &'static str,
    primary: bool,
    action: PrintJobAction,
    on_action: &PrintHandler,
) -> Button {
    let on_action = on_action.clone();
    let job = id.1;
    let button = Button::new(SharedString::from(format!("{}-{}", id.0, id.1)))
        .small()
        .label(label)
        .tooltip(tooltip)
        .tab_stop(false)
        .on_click(move |_, window, cx| on_action(job, action, window, cx));
    if primary {
        button.primary()
    } else {
        button.outline()
    }
}

fn print_notice(
    theme: &Theme,
    notice: &PrintNoticeProps,
    on_action: &PrintHandler,
    on_hover: &PrintHoverHandler,
) -> gpui_kit::AnyElement {
    let id = notice.id;
    let name = notice.name.as_ref();
    let (title, detail): (Div, SharedString) = match &notice.kind {
        PrintNoticeKind::Receiving { received, total } => (
            print_title("Receiving ", name, "…"),
            if *total == 0 {
                size_label(*received).into()
            } else {
                format!("{} of {}", size_label(*received), size_label(*total)).into()
            },
        ),
        PrintNoticeKind::Ready { detail, .. } => {
            (print_title("", name, " is ready"), detail.clone())
        }
        PrintNoticeKind::Unreadable => (
            print_title("Skerry can’t print ", name, ""),
            "It may be protected or damaged. Open it in Preview to print.".into(),
        ),
        PrintNoticeKind::Failed { detail } => {
            (print_title("Couldn’t receive ", name, ""), detail.clone())
        }
        PrintNoticeKind::TooLarge { detail } => (
            print_title("", name, " is too large to receive"),
            detail.clone(),
        ),
        PrintNoticeKind::Missing => (
            print_title("", name, " is no longer on this Mac"),
            "macOS cleared it from the cache. Print it again from the session.".into(),
        ),
    };
    let (glyph, tint) = print_glyph(theme, &notice.kind);
    let close_always = !matches!(
        notice.kind,
        PrintNoticeKind::Receiving { .. } | PrintNoticeKind::Ready { .. }
    );
    let group = SharedString::from(format!("print-notice-{id}"));
    let dismiss = {
        let on_action = on_action.clone();
        Button::new(SharedString::from(format!("print-dismiss-{id}")))
            .ghost()
            .xsmall()
            .icon(IconName::Close)
            .tooltip("Dismiss")
            .tab_stop(false)
            .on_click(move |_, window, cx| on_action(id, PrintJobAction::Dismiss, window, cx))
    };
    let buttons: Vec<Button> = match &notice.kind {
        PrintNoticeKind::Ready { saved, .. } => {
            let mut row = vec![
                print_button(
                    ("print-print", id),
                    "Print…",
                    "Open the macOS print dialog",
                    true,
                    PrintJobAction::Print,
                    on_action,
                ),
                print_button(
                    ("print-open", id),
                    "Open",
                    "Open in Preview",
                    false,
                    PrintJobAction::Open,
                    on_action,
                ),
            ];
            row.push(if *saved {
                print_button(
                    ("print-reveal", id),
                    "Show in Finder",
                    "Show the saved copy in Finder",
                    false,
                    PrintJobAction::Reveal,
                    on_action,
                )
            } else {
                print_button(
                    ("print-save", id),
                    "Save…",
                    "Save a copy…",
                    false,
                    PrintJobAction::Save,
                    on_action,
                )
            });
            row
        }
        PrintNoticeKind::Unreadable => vec![
            print_button(
                ("print-open", id),
                "Open",
                "Open in Preview",
                true,
                PrintJobAction::Open,
                on_action,
            ),
            print_button(
                ("print-save", id),
                "Save…",
                "Save a copy…",
                false,
                PrintJobAction::Save,
                on_action,
            ),
        ],
        _ => Vec::new(),
    };
    let progress = match &notice.kind {
        PrintNoticeKind::Receiving { received, total } => Some(if *total == 0 {
            None
        } else {
            Some((*received as f32 / *total as f32).clamp(0., 1.))
        }),
        _ => None,
    };
    let on_hover = on_hover.clone();
    let card = div()
        .id(group.clone())
        .group(group.clone())
        .w_full()
        .p(px(12.))
        .rounded(px(10.))
        .bg(theme.popover)
        .border_1()
        .border_color(theme.border)
        .shadow(crate::components::elevated_shadow(theme))
        .text_color(theme.foreground)
        .occlude()
        .on_hover(move |over, window, cx| on_hover(id, *over, window, cx))
        .v_flex()
        .child(
            div()
                .h_flex()
                .items_start()
                .gap(px(10.))
                .child(
                    div()
                        .size(px(28.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(glyph.with_size(px(20.)).text_color(tint)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .v_flex()
                        .gap(px(2.))
                        .child(title.text_size(px(13.)).font_weight(FontWeight::SEMIBOLD))
                        .child(
                            div()
                                .text_size(px(12.))
                                .line_clamp(2)
                                .text_color(theme.muted_foreground)
                                .child(detail),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .when(!close_always, |slot| {
                            slot.invisible().group_hover(group.clone(), |s| s.visible())
                        })
                        .child(dismiss),
                ),
        )
        .when_some(progress, |card, fraction| {
            card.child(
                div()
                    .mt(px(8.))
                    .h(px(3.))
                    .w_full()
                    .rounded_full()
                    .bg(theme.muted)
                    .relative()
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .rounded_full()
                            .bg(theme.accent)
                            .map(|fill| match fraction {
                                Some(fraction) => fill.left_0().w(gpui_kit::relative(fraction)),
                                // Unknown size: a still 30 % segment.
                                None => fill
                                    .left(gpui_kit::relative(0.35))
                                    .w(gpui_kit::relative(0.3)),
                            }),
                    ),
            )
        })
        .when(!buttons.is_empty(), |card| {
            card.child(
                div()
                    .mt(px(10.))
                    .h_flex()
                    .justify_end()
                    .gap(px(6.))
                    .children(buttons.into_iter().rev()),
            )
        });
    gpui_kit::AnimationExt::with_animation(
        card,
        SharedString::from(format!("print-enter-{id}")),
        gpui_kit::Animation::new(std::time::Duration::from_millis(150))
            .with_easing(gpui_kit::ease_out_quint()),
        |card, delta| card.opacity(delta).mt(px(-8. * (1. - delta))),
    )
    .into_any_element()
}

/// A size as Finder writes it, in decimal units: "512 bytes", "12 KB", "3.4 MB", "1 GB".
pub fn size_label(bytes: u64) -> String {
    match bytes {
        1 => "1 byte".into(),
        0..1000 => format!("{bytes} bytes"),
        1000..999_500 => format!("{} KB", (bytes as f64 / 1e3).round()),
        _ => {
            let (value, unit) = if bytes < 999_950_000 {
                (bytes as f64 / 1e6, "MB")
            } else {
                (bytes as f64 / 1e9, "GB")
            };
            let text = format!("{value:.1}");
            format!("{} {unit}", text.trim_end_matches(".0"))
        }
    }
}

/// Inline macOS IME marked-text composition box positioned near the active cursor/click.
pub fn ime_preedit(anchor: Point<Pixels>, text: impl Into<SharedString>) -> Div {
    div()
        .absolute()
        .left(anchor.x)
        .top(anchor.y)
        .bg(rgb(0x22262f))
        .text_color(rgb(0xffffff))
        .px_2()
        .child(text.into())
}

/// Floating Session Controls trigger button positioned outside the remote picture.
pub fn controls_trigger(
    position: Point<Pixels>,
    size: Pixels,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .absolute()
        .top(position.y)
        .left(position.x)
        .size(size)
        .occlude()
        .child(
            Button::new("session-controls-trigger")
                .secondary()
                .small()
                .size(size)
                .icon(IconName::Ellipsis)
                .tooltip("Session Controls")
                .on_click(on_click),
        )
}

/// Placeholder overlay shown before the first remote video frame arrives: "Connecting to
/// Firefox…", or "Waiting for Firefox to start…" during Keep Waiting. An empty label keeps
/// the generic text.
pub fn waiting_overlay(label: impl Into<SharedString>) -> Div {
    let label = label.into();
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(0xb6bdcc))
        .child(if label.is_empty() {
            SharedString::from("Waiting for the remote display…")
        } else {
            label
        })
}

/// The banner when a session hasn't answered within the start-up grace (`ARCH-027`):
/// Keep Waiting (5 more minutes), Retry (a fresh 45 s), or Disconnect.
pub fn startup_banner(
    theme: &Theme,
    message: impl Into<SharedString>,
    on_keep_waiting: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_disconnect: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_kit::Stateful<Div> {
    div()
        .id("session-startup-banner")
        .absolute()
        .bottom_3()
        .left_3()
        .right_3()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .text_color(theme.foreground)
        .occlude()
        .h_flex()
        .items_center()
        .gap_3()
        .child(
            Icon::new(IconName::Info)
                .with_size(px(16.))
                .text_color(theme.muted_foreground),
        )
        .child(div().flex_1().min_w_0().text_sm().child(message.into()))
        .child(
            Button::new("session-startup-keep-waiting")
                .primary()
                .small()
                .label("Keep Waiting")
                .tooltip("Keep trying for 5 more minutes")
                .on_click(on_keep_waiting),
        )
        .child(
            Button::new("session-startup-retry")
                .outline()
                .small()
                .label("Retry")
                .tooltip("Try connecting again now")
                .on_click(on_retry),
        )
        .child(
            Button::new("session-startup-disconnect")
                .ghost()
                .small()
                .label("Disconnect")
                .on_click(on_disconnect),
        )
}

/// Confirmation dialog shown before stopping a running remote session.
pub fn stop_confirm_modal(
    theme: &Theme,
    app_name: impl Into<String>,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_confirm: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_kit::Stateful<Div> {
    let app_name = app_name.into();
    confirm_modal(
        theme,
        ConfirmCopy {
            id: "stop-session",
            title: format!("Stop {app_name}?"),
            body: vec!["Stopping this session terminates the remote application and permanently deletes any temporary cleanroom data.".into()],
            confirm_label: "Stop Session",
            confirm_icon: None,
        },
        on_cancel,
        on_confirm,
    )
}

/// Where the signed-in application key came from, which decides what Sign Out says it
/// removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySourceLabel {
    /// Saved in `~/.config/skerry/config.toml` (read at startup, or a typed key saved there).
    SavedFile,
    Environment,
    DotEnv,
    /// A typed key Skerry couldn't save.
    ThisRunOnly,
}

impl KeySourceLabel {
    /// The first line of the Sign Out confirmation.
    pub fn sign_out_text(self) -> &'static str {
        match self {
            Self::SavedFile => "Skerry will remove the application key saved in ~/.config/skerry/config.toml. To reconnect, you’ll need to paste the key again.",
            Self::Environment => "This key comes from the SEALSKIN_APPLICATION_KEY environment variable. Skerry doesn’t store it, so it will be used again the next time Skerry starts.",
            Self::DotEnv => "This key comes from the .env file in Skerry’s working folder. Skerry doesn’t store it, so it will be used again the next time Skerry starts.",
            Self::ThisRunOnly => "Skerry hasn’t saved this key, so there’s nothing to remove. To reconnect, you’ll need to paste the key again.",
        }
    }
}

/// Confirmation dialog shown before signing out of a server. Escape and the backdrop cancel.
pub fn sign_out_confirm_modal(
    theme: &Theme,
    server: &str,
    source: KeySourceLabel,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_confirm: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_kit::Stateful<Div> {
    confirm_modal(
        theme,
        ConfirmCopy {
            id: "sign-out",
            title: format!("Sign out of {server}?"),
            body: vec![
                source.sign_out_text().into(),
                "Open session windows stay connected, and running sessions keep running on the server."
                    .into(),
            ],
            confirm_label: "Sign Out",
            confirm_icon: Some(gpui_kit::assets::IconName::LogOut.into()),
        },
        on_cancel,
        on_confirm,
    )
}

struct ConfirmCopy {
    id: &'static str,
    title: String,
    body: Vec<SharedString>,
    confirm_label: &'static str,
    confirm_icon: Option<Icon>,
}

/// The shared shell of destructive confirmations: dimmed backdrop (click cancels), centred
/// card, Cancel and a danger button on the right. Enter does nothing.
fn confirm_modal(
    theme: &Theme,
    copy: ConfirmCopy,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_confirm: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_kit::Stateful<Div> {
    let ConfirmCopy {
        id,
        title,
        body,
        confirm_label,
        confirm_icon,
    } = copy;
    let on_cancel = std::rc::Rc::new(on_cancel);
    let backdrop_cancel = std::rc::Rc::clone(&on_cancel);
    let mut confirm = Button::new(SharedString::from(format!("{id}-confirm")))
        .danger()
        .small()
        .label(confirm_label)
        .on_click(on_confirm);
    if let Some(icon) = confirm_icon {
        confirm = confirm.icon(icon);
    }
    div()
        .id(SharedString::from(format!("{id}-backdrop")))
        .absolute()
        .inset_0()
        .bg(rgba(0x00000099))
        .flex()
        .items_center()
        .justify_center()
        .p_6()
        .occlude()
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            backdrop_cancel(event, window, cx)
        })
        .child(
            div()
                .id(SharedString::from(format!("{id}-card")))
                .w(px(420.))
                .max_w_full()
                .p_5()
                .rounded_xl()
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .text_color(theme.foreground)
                .v_flex()
                .gap_4()
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .v_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .children(body.into_iter().map(|line| div().child(line))),
                )
                .child(
                    div()
                        .h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new(SharedString::from(format!("{id}-cancel")))
                                .outline()
                                .small()
                                .label("Cancel")
                                .on_click(move |event, window, cx| on_cancel(event, window, cx)),
                        )
                        .child(confirm),
                ),
        )
}

/// The Capture Pointer hint's text (`ARCH-032`): `lead`, then the chord in white semibold,
/// then `tail`. "Pointer captured · " + "⌃⌥⇧Z" + " releases it", or one plain line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureHintText {
    pub lead: SharedString,
    pub chord: Option<SharedString>,
    pub tail: SharedString,
}

/// How far above the bottom edge the hint sits: 24 pt, or 72 pt over a bottom banner.
pub fn capture_hint_bottom(banner: bool) -> Pixels {
    px(if banner { 72. } else { 24. })
}

/// Fade-in and fade-out of the capture hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintFade {
    /// Fully shown (the preview stories).
    None,
    In,
    Out,
}

/// The Capture Pointer hint: a bottom-centre, always-dark pill over the picture with no
/// hitbox, so clicks reach the desktop under it (`ARCH-032`). `generation` keys the fade.
pub fn capture_hint(
    text: &CaptureHintText,
    bottom: Pixels,
    viewport_width: Pixels,
    fade: HintFade,
    generation: u64,
) -> Div {
    use gpui_kit::{Animation, AnimationExt as _, FontFeatures};
    use std::sync::Arc;
    use std::time::Duration;
    let pill = div()
        .h_flex()
        .max_w((viewport_width - px(32.)).max(px(0.)))
        .px(px(12.))
        .py(px(6.))
        .rounded(px(6.))
        .bg(rgba(0x000000b3))
        .text_size(px(13.))
        .line_height(px(17.))
        .text_color(rgba(0xffffffd9))
        .whitespace_nowrap()
        .overflow_hidden()
        .child(div().min_w_0().text_ellipsis().child(text.lead.clone()))
        .when_some(text.chord.clone(), |pill, chord| {
            pill.child(
                div()
                    .flex_none()
                    .text_color(rgb(0xffffff))
                    .font_weight(FontWeight::SEMIBOLD)
                    .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
                    .child(chord),
            )
        })
        .when(!text.tail.is_empty(), |pill| {
            pill.child(div().min_w_0().text_ellipsis().child(text.tail.clone()))
        });
    let pill = match fade {
        HintFade::None => pill.into_any_element(),
        HintFade::In => pill
            .with_animation(
                ("capture-hint-in", generation),
                Animation::new(Duration::from_millis(120)).with_easing(gpui_kit::ease_out_quint()),
                |pill, delta| pill.opacity(delta),
            )
            .into_any_element(),
        HintFade::Out => pill
            .with_animation(
                ("capture-hint-out", generation),
                Animation::new(Duration::from_millis(200)).with_easing(gpui_kit::ease_out_quint()),
                |pill, delta| pill.opacity(1. - delta),
            )
            .into_any_element(),
    };
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom(bottom)
        .flex()
        .justify_center()
        .child(pill)
}

/// A one-line notice in the capture hint's place and style (`ARCH-039`): "This session
/// doesn't allow a second display."
pub fn notice_pill(
    text: impl Into<SharedString>,
    bottom: Pixels,
    viewport_width: Pixels,
    fade: HintFade,
    generation: u64,
) -> Div {
    let text = CaptureHintText {
        lead: text.into(),
        chord: None,
        tail: SharedString::default(),
    };
    capture_hint(&text, bottom, viewport_width, fade, generation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_finder() {
        assert_eq!(size_label(0), "0 bytes");
        assert_eq!(size_label(1), "1 byte");
        assert_eq!(size_label(999), "999 bytes");
        assert_eq!(size_label(12_345), "12 KB");
        assert_eq!(size_label(3_400_000), "3.4 MB");
        assert_eq!(size_label(3_000_000), "3 MB");
        assert_eq!(size_label(999_999), "1 MB");
        assert_eq!(size_label(1_400_000_000), "1.4 GB");
        assert_eq!(size_label(1 << 30), "1.1 GB");
    }

    #[test]
    fn the_notice_stack_fits_above_the_hint_and_banners() {
        let top = print_notices_top(None);
        assert_eq!(top, px(48.));
        assert_eq!(print_notices_shown(px(800.), top), 3);
        assert_eq!(print_notices_shown(px(480.), top), 2);
        assert_eq!(print_notices_shown(px(360.), top), 1);
        // Below the stats strip: one line ends at 51, two at 82.
        assert_eq!(print_notices_top(Some(px(51.))), px(52.));
        assert_eq!(print_notices_top(Some(px(82.))), px(84.));
        assert_eq!(print_notices_top(Some(px(8.))), px(48.));
    }

    #[test]
    fn placement_picker_shows_only_offered_sides() {
        assert_eq!(
            offered_sides([true, false, true, false]),
            [(0, "Left"), (2, "Above")]
        );
        assert_eq!(offered_sides([true; 4]).len(), 4);
    }

    #[test]
    fn placement_picker_arrows_pick_only_offered_sides() {
        let sides = [false, true, false, true];
        assert_eq!(placement_key("right", sides), Some(1));
        assert_eq!(placement_key("down", sides), Some(3));
        assert_eq!(placement_key("left", sides), None);
        assert_eq!(placement_key("escape", sides), None);
    }

    #[test]
    fn notice_names_keep_their_extension() {
        assert_eq!(print_name_parts("Report (2).pdf"), ("Report (2)", ".pdf"));
        assert_eq!(print_name_parts("document"), ("document", ""));
    }
}
