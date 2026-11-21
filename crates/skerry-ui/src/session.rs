use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    IconName, Sizable as _, StyledExt as _, Theme,
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
                .label("Reconnect")
                .on_click(on_reconnect),
        )
        .child(
            Button::new("session-banner-disconnect")
                .outline()
                .small()
                .label("Disconnect")
                .on_click(on_disconnect),
        )
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

/// Confirmation dialog shown before stopping a running remote session.
pub fn stop_confirm_modal(
    theme: &Theme,
    app_name: impl Into<String>,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_confirm: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_kit::Stateful<Div> {
    let app_name = app_name.into();
    let on_cancel = std::rc::Rc::new(on_cancel);
    let backdrop_cancel = std::rc::Rc::clone(&on_cancel);
    div()
        .id("stop-session-backdrop")
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
                .id("stop-session-card")
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
                        .child(format!("Stop {app_name}?")),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Stopping this session terminates the remote application and permanently deletes any temporary cleanroom data."),
                )
                .child(
                    div()
                        .h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("stop-cancel")
                                .outline()
                                .small()
                                .label("Cancel")
                                .on_click(move |event, window, cx| on_cancel(event, window, cx)),
                        )
                        .child(
                            Button::new("stop-confirm")
                                .danger()
                                .small()
                                .label("Stop Session")
                                .on_click(on_confirm),
                        ),
                ),
        )
}
