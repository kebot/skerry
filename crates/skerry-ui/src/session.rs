use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    IconName, Sizable as _, Theme,
};
use gpui_kit::{div, prelude::*, rgb, App, ClickEvent, Div, Pixels, Point, SharedString, Window};

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

/// Placeholder overlay shown before the first remote video frame arrives.
pub fn waiting_overlay() -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(0xb6bdcc))
        .child("Waiting for the remote display…")
}
