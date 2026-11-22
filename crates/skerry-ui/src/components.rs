use gpui_kit::component::{Icon, IconName, Sizable as _, StyledExt as _, Theme};
use gpui_kit::{
    div, ease_out_quint, hsla, point, prelude::*, px, quadratic, Animation, AnimationExt as _,
    BoxShadow, Div, FontWeight, SharedString,
};
use std::time::Duration;

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

/// A short confirmation shown at the bottom of the Library window. Errors never toast;
/// they stay in the status banner.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    /// A new id replays the enter animation; the same id (same message again) doesn't.
    pub id: u64,
    pub message: SharedString,
    pub kind: ToastKind,
    /// Set when the toast's time is up, for its exit fade before it is removed.
    pub leaving: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Success,
}

/// How long a toast stays before its exit fade.
pub const TOAST_DURATION: Duration = Duration::from_secs(3);
/// The exit fade; the toast is removed after it.
pub const TOAST_EXIT: Duration = Duration::from_millis(200);

/// The toast over the whole window, centred 24 pt above the bottom edge. Nothing in it
/// takes clicks, so it never covers a button beneath it.
pub fn toast_layer(theme: &Theme, toast: &Toast) -> Div {
    let icon_color = match toast.kind {
        ToastKind::Success => theme.success,
    };
    let pill = div()
        .relative()
        .h(px(32.))
        .px(px(14.))
        .py(px(8.))
        .h_flex()
        .items_center()
        .gap_2()
        .rounded(px(10.))
        .bg(theme.popover)
        .border_1()
        .border_color(theme.border)
        .shadow(elevated_shadow(theme))
        .child(
            Icon::new(IconName::CircleCheck)
                .with_size(px(16.))
                .text_color(icon_color),
        )
        .child(
            div()
                .max_w(px(360.))
                .truncate()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.foreground)
                .child(toast.message.clone()),
        );
    let pill = if toast.leaving {
        pill.with_animation(
            SharedString::from(format!("toast-leave-{}", toast.id)),
            Animation::new(TOAST_EXIT).with_easing(quadratic),
            |pill, delta| pill.opacity(1. - delta),
        )
        .into_any_element()
    } else {
        pill.with_animation(
            SharedString::from(format!("toast-enter-{}", toast.id)),
            Animation::new(Duration::from_millis(150)).with_easing(ease_out_quint()),
            |pill, delta| pill.opacity(delta).top(px(8. * (1. - delta))),
        )
        .into_any_element()
    };
    div()
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .items_end()
        .pb(px(24.))
        .child(pill)
}

/// The shadow of surfaces that float above others (menus, toasts), in GPUI units: blur is
/// a gaussian sigma, half the CSS blur. Dark adds a light ring, since a black shadow can't
/// be seen on Skerry's near-black panels.
pub fn elevated_shadow(theme: &Theme) -> Vec<BoxShadow> {
    let layer = |color, y: f32, blur: f32, spread: f32| BoxShadow {
        color,
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: false,
    };
    if theme.mode.is_dark() {
        vec![
            layer(hsla(0., 0., 0., 0.60), 12., 16., 0.),
            layer(hsla(0., 0., 1., 0.12), 0., 0., 1.),
        ]
    } else {
        vec![
            layer(hsla(0., 0., 0., 0.16), 8., 12., 0.),
            layer(hsla(0., 0., 0., 0.08), 2., 3., 0.),
        ]
    }
}
