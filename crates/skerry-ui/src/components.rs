use gpui_kit::component::{StyledExt as _, Theme};
use gpui_kit::{div, prelude::*, px, Div};

/// Standard list row card used in the launcher library.
pub fn list_row(theme: &Theme, name: impl Into<String>) -> Div {
    div()
        .h_flex()
        .items_center()
        .justify_between()
        .gap_3()
        .px_4()
        .py_2()
        .min_h(px(48.))
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .child(div().min_w_0().truncate().child(name.into()))
}
