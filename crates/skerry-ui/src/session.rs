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
