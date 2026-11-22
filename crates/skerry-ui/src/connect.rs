use crate::selkies::{
    self as direct, ConnectTab, SelkiesConnectHandlers, SelkiesConnectProps, SelkiesInputs,
};
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    label::Label,
    Disableable as _, IconName, Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{
    div, prelude::*, px, AnyElement, App, ClickEvent, Div, Entity, FontWeight, Hsla, Window,
};
use std::rc::Rc;

pub type ConnectHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectProps {
    pub status: Option<String>,
    pub busy: Option<String>,
    /// Links or files that arrived before a server was connected (`ARCH-028`): "Connect to
    /// a server to open “report.pdf”."
    pub pending_open: Option<String>,
    /// The server the problem or recovery is about, shown as the Library's server pill.
    pub server: Option<String>,
    /// SealSkin refused the key or account: an alert above the key field (`ARCH-029`).
    pub problem: Option<KeyProblem>,
    /// A dormant key or a held account: a panel instead of the key form.
    pub recovery: Option<RecoveryProps>,
    /// "Use a Different Key": the field asks for a new key, and empty retries the current one.
    pub different_key: bool,
    /// Under the field, after an empty Connect that can't retry ("Paste a new application key.").
    pub hint: Option<String>,
}

/// Why SealSkin refused the key or the account, with what the alert says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    /// 401 `unknown`.
    Revoked,
    /// 401 `expired`.
    Expired,
    /// 401 `disabled`.
    KeysDisabled,
    /// 401 `client`.
    ClientNotAllowed,
    /// Any other 401, with the server's detail.
    OtherKey(String),
    /// 403 `User account is inactive.`
    Inactive,
    /// The same unrecognised 403 on every Library request, with the server's text.
    OtherAccount(String),
}

/// Where the refused key came from, when that's outside Skerry's config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvNote {
    Environment,
    DotEnv,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyProblem {
    pub kind: ProblemKind,
    /// Shown only for Revoked, Expired and other 401s (`env_note_applies`).
    pub env_note: Option<EnvNote>,
}

impl KeyProblem {
    /// A dead key: the field asks for a new one, and an empty Connect never resends it.
    pub fn needs_new_key(&self) -> bool {
        matches!(self.kind, ProblemKind::Revoked | ProblemKind::Expired)
    }

    pub fn title(&self) -> &'static str {
        match self.kind {
            ProblemKind::Revoked => "This key no longer signs you in",
            ProblemKind::Expired => "This key has expired",
            ProblemKind::KeysDisabled => "Application keys are turned off",
            ProblemKind::ClientNotAllowed => "Skerry isn’t allowed to use keys",
            ProblemKind::OtherKey(_) => "The server didn’t accept this key",
            ProblemKind::Inactive => "This account is turned off",
            ProblemKind::OtherAccount(_) => "SealSkin refused the request",
        }
    }

    pub fn body(&self) -> String {
        match &self.kind {
            ProblemKind::Revoked => "It was revoked, or it never existed on this server. Make a new key in SealSkin under Settings → Application Keys, then paste it below.".into(),
            ProblemKind::Expired => "Make a new key in SealSkin under Settings → Application Keys, then paste it below.".into(),
            ProblemKind::KeysDisabled => "Your administrator doesn’t allow application keys for this account. Once they turn them back on, choose Connect to try this key again.".into(),
            ProblemKind::ClientNotAllowed => "Your administrator allows application keys only in some apps. Ask them to allow “Other clients”, then choose Connect to try again.".into(),
            ProblemKind::OtherKey(detail) if !detail.trim().is_empty() => detail.trim().into(),
            ProblemKind::OtherKey(_) => "SealSkin refused the key without saying why.".into(),
            ProblemKind::Inactive => "Ask your SealSkin administrator to turn it back on, then choose Connect to try again.".into(),
            ProblemKind::OtherAccount(detail) if !detail.trim().is_empty() => {
                detail.trim().into()
            }
            ProblemKind::OtherAccount(_) => "SealSkin didn’t say why.".into(),
        }
    }

    /// The environment note's text, when it applies.
    pub fn env_note_text(&self) -> Option<&'static str> {
        let applies = matches!(
            self.kind,
            ProblemKind::Revoked | ProblemKind::Expired | ProblemKind::OtherKey(_)
        );
        match self.env_note.filter(|_| applies)? {
            EnvNote::Environment => Some("This key comes from the SEALSKIN_APPLICATION_KEY environment variable. A new key you paste here is saved and works now, but the environment key is used again the next time Skerry starts. Update or remove it there."),
            EnvNote::DotEnv => Some("This key comes from SEALSKIN_APPLICATION_KEY in the .env file in Skerry’s working folder. A new key you paste here is saved and works now, but the .env key is used again the next time Skerry starts. Update or remove it there."),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryKind {
    /// 401 `dormant`: sign in on the web to wake the key.
    Dormant,
    /// Waiting for an administrator to place the account in a group.
    Held,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryProps {
    pub kind: RecoveryKind,
    /// The browser was opened and Skerry checks the key every few seconds.
    pub waiting: bool,
    /// The wake check gave up after 3 minutes.
    pub timed_out: bool,
}

pub const WAITING_TEXT: &str = "Waiting for your web sign-in…";
pub const TIMED_OUT_TEXT: &str =
    "Still asleep. Finish signing in in your browser, then choose Try Again.";
pub const PASTE_NEW_KEY: &str = "Paste a new application key.";
pub const RETRY_HELPER: &str = "Leave empty to try your current key again.";

/// What the Connect card and its recovery panels do. Only `on_connect` is required.
#[derive(Clone)]
pub struct ConnectHandlers {
    pub on_connect: ConnectHandler,
    pub on_sign_in_web: Option<ConnectHandler>,
    pub on_try_again: Option<ConnectHandler>,
    pub on_other_key: Option<ConnectHandler>,
    pub on_open_web: Option<ConnectHandler>,
}

impl ConnectHandlers {
    pub fn new(on_connect: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        Self {
            on_connect: Rc::new(on_connect),
            on_sign_in_web: None,
            on_try_again: None,
            on_other_key: None,
            on_open_web: None,
        }
    }
}

/// Creates a masked single-line input configured for SealSkin application keys.
pub fn new_key_input(window: &mut Window, cx: &mut App) -> Entity<InputState> {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("ssk_<id>_<secret>@<host>"));
    input.update(cx, |state, cx| state.set_masked(true, window, cx));
    input
}

fn card(theme: &Theme) -> Div {
    div()
        .w(px(440.))
        .p_6()
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .v_flex()
        .gap_4()
}

/// The Library header's server pill, so a problem names its server the same way.
pub fn server_pill(theme: &Theme, server: &str) -> Div {
    div()
        .flex_none()
        .max_w_full()
        .min_w_0()
        .overflow_hidden()
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
        .child(div().flex_none().child(IconName::Globe))
        .child(div().min_w_0().truncate().child(server.to_string()))
}

fn invoke(handler: &Option<ConnectHandler>) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    let handler = handler.clone();
    move |event, window, cx| {
        if let Some(handler) = &handler {
            handler(event, window, cx);
        }
    }
}

pub fn render(
    theme: &Theme,
    props: &ConnectProps,
    key_input: &Entity<InputState>,
    handlers: ConnectHandlers,
) -> Div {
    let card = match props.recovery {
        Some(recovery) => recovery_panel(theme, props, recovery, &handlers, None),
        None => key_card(theme, props, key_input, &handlers, None),
    };
    screen(theme, card)
}

fn screen(theme: &Theme, card: Div) -> Div {
    div()
        .size_full()
        .relative()
        .bg(theme.background)
        .text_color(theme.foreground)
        .flex()
        .items_center()
        .justify_center()
        .child(card)
}

/// The Selkies tab's parts (`ARCH-036`).
pub struct ConnectTabs<'a> {
    pub tab: ConnectTab,
    pub selkies: &'a SelkiesConnectProps,
    pub inputs: &'a SelkiesInputs,
    pub handlers: SelkiesConnectHandlers,
}

/// The Connect card with its SealSkin | Selkies control, and the certificate prompt over
/// it when one is open.
pub fn render_with_tabs(
    theme: &Theme,
    props: &ConnectProps,
    key_input: &Entity<InputState>,
    handlers: ConnectHandlers,
    tabs: ConnectTabs<'_>,
) -> Div {
    let busy = props.busy.is_some() || tabs.selkies.busy.is_some();
    let bar = || direct::tab_bar(tabs.tab, !busy, tabs.handlers.on_tab.clone()).into_any_element();
    let card = match (tabs.tab, props.recovery) {
        (ConnectTab::Selkies, _) => card(theme).child(bar()).child(direct::selkies_card(
            theme,
            tabs.selkies,
            tabs.inputs,
            &tabs.handlers,
        )),
        (ConnectTab::SealSkin, Some(recovery)) => {
            recovery_panel(theme, props, recovery, &handlers, Some(bar()))
        }
        (ConnectTab::SealSkin, None) => key_card(theme, props, key_input, &handlers, Some(bar())),
    };
    screen(theme, card).when_some(
        tabs.selkies
            .certificate
            .as_ref()
            .filter(|_| tabs.tab == ConnectTab::Selkies),
        |view, cert| view.child(direct::certificate_sheet(theme, cert, &tabs.handlers)),
    )
}

fn key_card(
    theme: &Theme,
    props: &ConnectProps,
    key_input: &Entity<InputState>,
    handlers: &ConnectHandlers,
    top: Option<AnyElement>,
) -> Div {
    let busy = props.busy.is_some();
    let on_connect = handlers.on_connect.clone();
    let new_key = props.different_key
        || props
            .problem
            .as_ref()
            .is_some_and(KeyProblem::needs_new_key);
    let retry_helper = props.different_key
        || props
            .problem
            .as_ref()
            .is_some_and(|problem| !problem.needs_new_key());
    card(theme)
        .children(top)
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
        .when_some(props.problem.as_ref(), |view, problem| {
            view.child(problem_alert(theme, problem, props.server.as_deref()))
        })
        .child(
            div()
                .v_flex()
                .gap_1p5()
                .child(Label::new(if new_key {
                    "New Application Key"
                } else {
                    "Application Key"
                }))
                .child(
                    Input::new(key_input)
                        .mask_toggle()
                        .cleanable(true)
                        .disabled(busy),
                )
                .when_some(props.hint.clone(), |view, hint| {
                    view.child(div().text_xs().text_color(theme.danger).child(hint))
                })
                .when(retry_helper && props.hint.is_none(), |view| {
                    view.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(RETRY_HELPER),
                    )
                }),
        )
        .when_some(props.pending_open.clone(), |view, pending| {
            view.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.secondary)
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(pending),
            )
        })
        // The alert already says what went wrong; the red line is for everything else.
        .when_some(props.status.clone().filter(|_| props.problem.is_none()), |view, status| {
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
                .on_click(move |event, window, cx| on_connect(event, window, cx)),
        )
}

/// The refusal alert: an error for a dead key, a warning for a policy the administrator can
/// change. Yellow is used only for the icon and tint, never for text.
fn problem_alert(theme: &Theme, problem: &KeyProblem, server: Option<&str>) -> Div {
    let (tone, icon): (Hsla, IconName) = if problem.needs_new_key() {
        (theme.danger, IconName::CircleX)
    } else {
        (theme.warning, IconName::TriangleAlert)
    };
    div()
        .h_flex()
        .items_start()
        .gap_2p5()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(tone.opacity(0.4))
        .bg(tone.opacity(0.08))
        .child(div().flex_none().pt_0p5().text_color(tone).child(icon))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .v_flex()
                .gap_1p5()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(problem.title()),
                )
                .when_some(server, |view, server| {
                    view.child(server_pill(theme, server))
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(problem.body()),
                )
                .when_some(problem.env_note_text(), |view, note| {
                    view.child(env_note(theme, note))
                }),
        )
}

/// The environment note, with the variable name in the mono font.
fn env_note(theme: &Theme, note: &'static str) -> Div {
    const VARIABLE: &str = "SEALSKIN_APPLICATION_KEY";
    let (before, after) = note.split_once(VARIABLE).unwrap_or((note, ""));
    let mut line = div()
        .text_xs()
        .text_color(theme.muted_foreground)
        .h_flex()
        .flex_wrap()
        .child(before.to_string());
    if !after.is_empty() || note.contains(VARIABLE) {
        line = line
            .child(
                div()
                    .font_family(theme.mono_font_family.clone())
                    .child(VARIABLE),
            )
            .child(after.to_string());
    }
    line
}

fn recovery_panel(
    theme: &Theme,
    props: &ConnectProps,
    recovery: RecoveryProps,
    handlers: &ConnectHandlers,
    top: Option<AnyElement>,
) -> Div {
    let busy = props.busy.is_some();
    let (icon, tone, title, body) = match recovery.kind {
        RecoveryKind::Dormant => (
            gpui_kit::component::Icon::new(IconName::Moon),
            theme.info,
            "Sign in on the web to continue",
            "Your application key is asleep: your administrator asks for a sign-in on the web now and then. Sign in there, and Skerry connects again by itself.",
        ),
        RecoveryKind::Held => (
            gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Hourglass),
            theme.warning,
            "Waiting for an administrator",
            "Your account needs an administrator to add it to a group before you can use it. Once they have, choose Try Again.",
        ),
    };
    // After the wait times out, Try Again becomes the primary action.
    let try_again_primary = recovery.kind == RecoveryKind::Dormant && recovery.timed_out;
    let (web_label, web_handler) = match recovery.kind {
        RecoveryKind::Dormant => ("Sign In on the Web", &handlers.on_sign_in_web),
        RecoveryKind::Held => ("Open SealSkin in Browser", &handlers.on_open_web),
    };
    let web = Button::new("recovery-web")
        .w_full()
        .icon(IconName::ExternalLink)
        .label(web_label)
        .disabled(busy)
        .on_click(invoke(web_handler));
    let web = if try_again_primary {
        web.outline()
    } else {
        web.primary()
    };
    let try_again = Button::new("recovery-try-again")
        .w_full()
        .label(if busy { "Checking…" } else { "Try Again" })
        .loading(busy)
        .disabled(busy)
        .on_click(invoke(&handlers.on_try_again));
    let try_again = if try_again_primary {
        try_again.primary()
    } else {
        try_again.outline()
    };
    let status_row = if recovery.timed_out {
        Some(
            div()
                .h_flex()
                .items_start()
                .gap_2()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(div().flex_none().pt_0p5().child(IconName::Info))
                .child(TIMED_OUT_TEXT),
        )
    } else if recovery.waiting {
        Some(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(gpui_kit::component::spinner::Spinner::new().xsmall())
                .child(WAITING_TEXT),
        )
    } else {
        None
    };
    let (first, second) = if try_again_primary {
        (try_again.into_any_element(), web.into_any_element())
    } else {
        (web.into_any_element(), try_again.into_any_element())
    };
    card(theme)
        .children(top)
        .child(
            div()
                .size(px(40.))
                .rounded_full()
                .bg(tone.opacity(0.14))
                .flex()
                .items_center()
                .justify_center()
                .child(icon.size(px(20.)).text_color(tone)),
        )
        .child(
            div()
                .v_flex()
                .gap_2()
                .items_start()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .when_some(props.server.as_deref(), |view, server| {
                    view.child(server_pill(theme, server))
                }),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(body),
        )
        .when_some(status_row, |view, row| view.child(row))
        .when_some(props.status.clone(), |view, status| {
            view.child(div().text_sm().text_color(theme.danger).child(status))
        })
        .child(
            div().v_flex().gap_2().child(first).child(second).child(
                div().flex().justify_center().child(
                    Button::new("recovery-other-key")
                        .ghost()
                        .small()
                        .label("Use a Different Key")
                        .disabled(busy)
                        .on_click(invoke(&handlers.on_other_key)),
                ),
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(kind: ProblemKind, env: Option<EnvNote>) -> KeyProblem {
        KeyProblem {
            kind,
            env_note: env,
        }
    }

    #[test]
    fn only_dead_keys_ask_for_a_new_one() {
        assert!(problem(ProblemKind::Revoked, None).needs_new_key());
        assert!(problem(ProblemKind::Expired, None).needs_new_key());
        for kind in [
            ProblemKind::KeysDisabled,
            ProblemKind::ClientNotAllowed,
            ProblemKind::OtherKey("x".into()),
            ProblemKind::Inactive,
            ProblemKind::OtherAccount("x".into()),
        ] {
            assert!(!problem(kind, None).needs_new_key());
        }
    }

    #[test]
    fn the_env_note_shows_only_for_keys_that_need_replacing_there() {
        let env = Some(EnvNote::Environment);
        assert!(problem(ProblemKind::Revoked, env).env_note_text().is_some());
        assert!(problem(ProblemKind::Expired, Some(EnvNote::DotEnv))
            .env_note_text()
            .is_some_and(|note| note.contains(".env")));
        assert!(problem(ProblemKind::OtherKey(String::new()), env)
            .env_note_text()
            .is_some());
        assert!(problem(ProblemKind::KeysDisabled, env)
            .env_note_text()
            .is_none());
        assert!(problem(ProblemKind::Inactive, env)
            .env_note_text()
            .is_none());
        assert!(problem(ProblemKind::Revoked, None)
            .env_note_text()
            .is_none());
    }

    #[test]
    fn unknown_refusals_show_the_server_text() {
        assert_eq!(
            problem(ProblemKind::OtherKey("Invalid token.".into()), None).body(),
            "Invalid token."
        );
        assert_eq!(
            problem(ProblemKind::OtherKey(String::new()), None).body(),
            "SealSkin refused the key without saying why."
        );
        assert!(problem(ProblemKind::OtherAccount("Nope.".into()), None)
            .body()
            .ends_with("Nope."));
    }
}
