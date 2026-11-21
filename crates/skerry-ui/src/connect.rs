use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    label::Label,
    Disableable as _, StyledExt as _, Theme,
};
use gpui_kit::{div, prelude::*, px, App, ClickEvent, Div, Entity, FontWeight, Window};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectProps {
    pub status: Option<String>,
    pub busy: Option<String>,
}

/// Creates a masked single-line input configured for SealSkin application keys.
pub fn new_key_input(window: &mut Window, cx: &mut App) -> Entity<InputState> {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("ssk_<id>_<secret>@<host>"));
    input.update(cx, |state, cx| state.set_masked(true, window, cx));
    input
}

pub fn render(
    theme: &Theme,
    props: &ConnectProps,
    key_input: &Entity<InputState>,
    on_connect: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let busy = props.busy.is_some();
    div()
        .size_full()
        .bg(theme.background)
        .text_color(theme.foreground)
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(440.))
                .p_6()
                .rounded_xl()
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .v_flex()
                .gap_4()
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Connect to SealSkin"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Enter an application key from Settings → Application Keys on your SealSkin server. Skerry saves it to ~/.config/skerry/config.toml after connecting."),
                )
                .child(
                    div()
                        .v_flex()
                        .gap_1p5()
                        .child(Label::new("Application Key"))
                        .child(
                            Input::new(key_input)
                                .mask_toggle()
                                .cleanable(true)
                                .disabled(busy),
                        ),
                )
                .when_some(props.status.clone(), |view, status| {
                    view.child(div().text_sm().text_color(theme.danger).child(status))
                })
                .when_some(props.busy.clone(), |view, busy_text| {
                    view.child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(busy_text),
                    )
                })
                .child(
                    Button::new("connect")
                        .primary()
                        .w_full()
                        .label(if busy { "Connecting…" } else { "Connect" })
                        .disabled(busy)
                        .on_click(on_connect),
                ),
        )
}
