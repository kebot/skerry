//! Direct Selkies servers (`ARCH-036`): the Connect card's Selkies tab, the certificate
//! trust prompt, and the Library for a direct server (one Desktop card and a server menu).
//! Pure presentation: the Launcher fills the props.

use crate::connect::server_pill;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    input::{Input, InputState},
    label::Label,
    popover::Popover,
    spinner::Spinner,
    tab::{Tab, TabBar},
    tooltip::Tooltip,
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    StyledExt as _, Theme,
};
use gpui_kit::{
    div, img, prelude::*, px, rgba, Anchor, App, ClickEvent, ClipboardItem, Div, Entity,
    ExternalPaths, FontWeight, Hsla, ObjectFit, RenderImage, SharedString, Window,
};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

pub type Click = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
pub type UrlHandler = Rc<dyn Fn(String, &mut Window, &mut App)>;
pub type FlagHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;
pub type TabHandler = Rc<dyn Fn(ConnectTab, &mut Window, &mut App)>;
pub type DropHandler = Rc<dyn Fn(Vec<PathBuf>, &mut Window, &mut App)>;

pub const TITLE: &str = "Connect to a Selkies Server";
pub const HELPER: &str =
    "Paste the address you open in a browser. A link with #token= signs in with its session token.";
pub const REMEMBER: &str = "Remember on this Mac";
pub const REMEMBER_ON: &str = "Saved to ~/.config/skerry/config.toml.";
pub const REMEMBER_OFF: &str =
    "Skerry keeps the address and username. You’ll type the password next time.";
pub const TOKEN_CHIP: &str = "Session token from the link";

// ---------------------------------------------------------------------------------------
// Connect

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectTab {
    #[default]
    SealSkin,
    Selkies,
}

/// The field a login or token error is about: its line shows under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelkiesField {
    Address,
    Login,
    Token,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertTone {
    Danger,
    Warning,
    Info,
}

/// Why the gate refused, as the card shows it (design §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelkiesProblem {
    /// An `xs` danger line under the field it names.
    Field { field: SelkiesField, text: String },
    /// An alert above the address.
    Alert {
        tone: AlertTone,
        title: String,
        body: String,
        server: Option<String>,
        /// A SealSkin address: the alert offers Switch to SealSkin.
        switch_to_sealskin: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentServerRow {
    pub url: String,
    pub label: String,
    pub login: String,
    pub pinned: bool,
    pub loopback: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertificateProps {
    /// The bare host, for the title.
    pub host: String,
    /// Host and port, for the pill.
    pub host_port: String,
    /// `AB:CD:…`.
    pub fingerprint: String,
    /// The pinned fingerprint, when the certificate changed.
    pub previous: Option<String>,
}

impl CertificateProps {
    pub fn changed(&self) -> bool {
        self.previous.is_some()
    }

    pub fn title(&self) -> String {
        if self.changed() {
            format!("The certificate of “{}” changed", self.host)
        } else {
            format!("Skerry can’t verify “{}”", self.host)
        }
    }

    pub fn body(&self) -> &'static str {
        if self.changed() {
            "It changed since you trusted it. Someone may be intercepting the connection, or the server got a new certificate, or was reinstalled or its container recreated. Check the new fingerprint with the server’s operator before you trust it."
        } else {
            "Its certificate isn’t signed by an authority your Mac trusts. A Selkies container’s built-in certificate looks like this. Connect only if this fingerprint matches the server’s."
        }
    }

    pub fn trust_label(&self) -> &'static str {
        if self.changed() {
            "Trust New Certificate"
        } else {
            "Trust and Connect"
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelkiesConnectProps {
    pub recent: Vec<RecentServerRow>,
    /// The address carried a session token: the chip shows under it.
    pub token_chip: bool,
    pub remember: bool,
    pub problem: Option<SelkiesProblem>,
    /// Address-parse errors: a danger line above the button.
    pub status: Option<String>,
    /// "Checking <host>…" while the gate runs.
    pub busy: Option<String>,
    pub pending_open: Option<String>,
    pub certificate: Option<CertificateProps>,
}

#[derive(Clone)]
pub struct SelkiesConnectHandlers {
    pub on_tab: TabHandler,
    pub on_connect: Click,
    pub on_recent: UrlHandler,
    pub on_recent_remove: UrlHandler,
    pub on_remember: FlagHandler,
    pub on_drop_token: Click,
    pub on_trust: Click,
    pub on_cert_cancel: Click,
    pub on_switch_sealskin: Click,
}

pub struct SelkiesInputs {
    pub address: Entity<InputState>,
    pub username: Entity<InputState>,
    pub password: Entity<InputState>,
}

pub fn new_selkies_inputs(window: &mut Window, cx: &mut App) -> SelkiesInputs {
    let address =
        cx.new(|cx| InputState::new(window, cx).placeholder("https://desktop.example.com/"));
    let username = cx.new(|cx| InputState::new(window, cx).placeholder("ubuntu"));
    let password = cx.new(|cx| InputState::new(window, cx).placeholder("Password"));
    password.update(cx, |state, cx| state.set_masked(true, window, cx));
    SelkiesInputs {
        address,
        username,
        password,
    }
}

/// The segmented control at the top of the Connect card.
pub fn tab_bar(tab: ConnectTab, enabled: bool, on_tab: TabHandler) -> impl IntoElement {
    TabBar::new("connect-tabs")
        .segmented()
        .w_full()
        .selected_index(match tab {
            ConnectTab::SealSkin => 0,
            ConnectTab::Selkies => 1,
        })
        .child(Tab::new().label("SealSkin").flex_1())
        .child(Tab::new().label("Selkies").flex_1())
        .on_click(move |index, window, cx| {
            if enabled {
                on_tab(
                    if *index == 0 {
                        ConnectTab::SealSkin
                    } else {
                        ConnectTab::Selkies
                    },
                    window,
                    cx,
                )
            }
        })
}

fn field_line(theme: &Theme, problem: Option<&SelkiesProblem>, field: SelkiesField) -> Option<Div> {
    match problem? {
        SelkiesProblem::Field { field: f, text } if *f == field => {
            Some(div().text_xs().text_color(theme.danger).child(text.clone()))
        }
        _ => None,
    }
}

fn tone_color(theme: &Theme, tone: AlertTone) -> (Hsla, IconName) {
    match tone {
        AlertTone::Danger => (theme.danger, IconName::CircleX),
        AlertTone::Warning => (theme.warning, IconName::TriangleAlert),
        AlertTone::Info => (theme.info, IconName::Info),
    }
}

fn alert(theme: &Theme, problem: &SelkiesProblem, on_switch: Click) -> Option<Div> {
    let SelkiesProblem::Alert {
        tone,
        title,
        body,
        server,
        switch_to_sealskin,
    } = problem
    else {
        return None;
    };
    let (color, icon) = tone_color(theme, *tone);
    Some(
        div()
            .h_flex()
            .items_start()
            .gap_2p5()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(color.opacity(0.4))
            .bg(color.opacity(0.08))
            .child(div().flex_none().pt_0p5().text_color(color).child(icon))
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
                            .child(title.clone()),
                    )
                    .when_some(server.as_deref(), |view, server| {
                        view.child(server_pill(theme, server))
                    })
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(body.clone()),
                    )
                    .when(*switch_to_sealskin, |view| {
                        view.child(
                            div().child(
                                Button::new("switch-sealskin")
                                    .link()
                                    .small()
                                    .label("Switch to SealSkin")
                                    .on_click(move |e, w, cx| on_switch(e, w, cx)),
                            ),
                        )
                    }),
            ),
    )
}

fn recent_rows(
    theme: &Theme,
    rows: &[RecentServerRow],
    enabled: bool,
    handlers: &SelkiesConnectHandlers,
) -> Div {
    let list = div()
        .v_flex()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .overflow_hidden()
        .children(rows.iter().take(5).enumerate().map(|(index, row)| {
            let group = SharedString::from(format!("recent-{index}"));
            let (on_recent, url) = (handlers.on_recent.clone(), row.url.clone());
            let (on_remove, remove_url) = (handlers.on_recent_remove.clone(), row.url.clone());
            div()
                .id(SharedString::from(format!("recent-row-{index}")))
                .group(group.clone())
                .h(px(36.))
                .px_2p5()
                .h_flex()
                .items_center()
                .gap_2()
                .when(index > 0, |row| row.border_t_1().border_color(theme.border))
                .hover(|style| style.bg(theme.secondary))
                .when(enabled, |row| {
                    row.cursor_pointer()
                        .on_click(move |_, window, cx| on_recent(url.clone(), window, cx))
                })
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child(IconName::Globe),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(row.label.clone()),
                        )
                        .when(row.pinned, |label| {
                            label.child(
                                Icon::new(gpui_kit::assets::IconName::Lock)
                                    .size(px(12.))
                                    .text_color(theme.muted_foreground),
                            )
                        })
                        .when(row.loopback, |label| {
                            label.child(
                                Icon::new(gpui_kit::assets::IconName::LockOpen)
                                    .size(px(12.))
                                    .text_color(theme.warning),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(row.login.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .invisible()
                        .group_hover(group, |style| style.visible())
                        .child(
                            Button::new(SharedString::from(format!("recent-remove-{index}")))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("Remove from Recent Servers")
                                .disabled(!enabled)
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    on_remove(remove_url.clone(), window, cx)
                                }),
                        ),
                )
        }));
    div()
        .v_flex()
        .gap_1p5()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child("RECENT SERVERS"),
        )
        .child(list)
}

/// The Selkies tab's card body (below the tab bar).
pub fn selkies_card(
    theme: &Theme,
    props: &SelkiesConnectProps,
    inputs: &SelkiesInputs,
    handlers: &SelkiesConnectHandlers,
) -> Div {
    let busy = props.busy.is_some();
    let problem = props.problem.as_ref();
    let on_connect = handlers.on_connect.clone();
    let on_remember = handlers.on_remember.clone();
    let on_drop_token = handlers.on_drop_token.clone();
    let alert = problem.and_then(|p| alert(theme, p, handlers.on_switch_sealskin.clone()));
    div()
        .v_flex()
        .gap_4()
        .child(
            div()
                .v_flex()
                .gap_1p5()
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(TITLE),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(HELPER),
                ),
        )
        .when(!props.recent.is_empty(), |card| {
            card.child(recent_rows(theme, &props.recent, !busy, handlers))
        })
        .when_some(alert, |card, alert| card.child(alert))
        .child(
            div()
                .v_flex()
                .gap_1p5()
                .child(Label::new("Server Address"))
                .child(Input::new(&inputs.address).cleanable(true).disabled(busy))
                .when_some(field_line(theme, problem, SelkiesField::Address), |v, l| {
                    v.child(l)
                })
                .when(props.token_chip, |view| {
                    view.child(
                        div().h_flex().child(
                            div()
                                .h_flex()
                                .items_center()
                                .gap_1p5()
                                .pl_2()
                                .pr_1()
                                .py_0p5()
                                .rounded_full()
                                .bg(theme.secondary)
                                .border_1()
                                .border_color(theme.border)
                                .text_xs()
                                .child(
                                    Icon::new(gpui_kit::assets::IconName::KeyRound).size(px(12.)),
                                )
                                .child(TOKEN_CHIP)
                                .child(
                                    Button::new("drop-token")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .tooltip("Remove the session token")
                                        .disabled(busy)
                                        .on_click(move |e, w, cx| on_drop_token(e, w, cx)),
                                ),
                        ),
                    )
                })
                .when_some(field_line(theme, problem, SelkiesField::Token), |v, l| {
                    v.child(l)
                }),
        )
        .child(
            div()
                .v_flex()
                .gap_1p5()
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .v_flex()
                                .gap_1p5()
                                .child(Label::new("Username"))
                                .child(Input::new(&inputs.username).disabled(busy)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .v_flex()
                                .gap_1p5()
                                .child(Label::new("Password"))
                                .child(Input::new(&inputs.password).mask_toggle().disabled(busy)),
                        ),
                )
                .when_some(field_line(theme, problem, SelkiesField::Login), |v, l| {
                    v.child(l)
                }),
        )
        .child(
            div()
                .v_flex()
                .gap_1()
                .child(
                    Checkbox::new("selkies-remember")
                        .label(REMEMBER)
                        .checked(props.remember)
                        .disabled(busy)
                        .on_click(move |checked, window, cx| on_remember(*checked, window, cx)),
                )
                .child(
                    div()
                        .pl(px(24.))
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(if props.remember {
                            REMEMBER_ON
                        } else {
                            REMEMBER_OFF
                        }),
                ),
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
        .when_some(props.status.clone(), |view, status| {
            view.child(div().text_sm().text_color(theme.danger).child(status))
        })
        .child(
            Button::new("selkies-connect")
                .primary()
                .w_full()
                .label(if busy { "Connecting…" } else { "Connect" })
                .loading(busy)
                .disabled(busy)
                .on_click(move |event, window, cx| on_connect(event, window, cx)),
        )
        .when_some(props.busy.clone(), |view, busy| {
            view.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(busy),
            )
        })
}

/// `AB:CD:…` as lines of 8 groups.
pub fn fingerprint_lines(fingerprint: &str) -> Vec<String> {
    let groups: Vec<&str> = fingerprint.split(':').collect();
    groups.chunks(8).map(|line| line.join(":")).collect()
}

fn fingerprint_box(theme: &Theme, fingerprint: &str, struck: bool) -> Div {
    div()
        .p_2()
        .rounded_md()
        .bg(theme.secondary)
        .font_family(theme.mono_font_family.clone())
        .text_xs()
        .when(struck, |view| {
            view.text_color(theme.muted_foreground).line_through()
        })
        .v_flex()
        .children(
            fingerprint_lines(fingerprint)
                .into_iter()
                .map(|line| div().child(line)),
        )
}

/// The trust prompt over the dimmed Connect window: slow on purpose. Clicking the backdrop
/// does nothing; Cancel is the default button.
pub fn certificate_sheet(
    theme: &Theme,
    cert: &CertificateProps,
    handlers: &SelkiesConnectHandlers,
) -> Div {
    let changed = cert.changed();
    let badge_tone = if changed { theme.danger } else { theme.warning };
    let on_cancel = handlers.on_cert_cancel.clone();
    let on_trust = handlers.on_trust.clone();
    let fingerprint = cert.fingerprint.clone();
    let trust = Button::new("cert-trust")
        .small()
        .label(cert.trust_label())
        .on_click(move |e, w, cx| on_trust(e, w, cx));
    let trust = if changed { trust.danger() } else { trust };
    div()
        .absolute()
        .inset_0()
        .bg(rgba(0x00000099))
        .flex()
        .items_center()
        .justify_center()
        .p_6()
        .child(
            div()
                .id("cert-backdrop")
                .absolute()
                .inset_0()
                .occlude(),
        )
        .child(
            div()
                .id("cert-card")
                .w(px(420.))
                .max_w_full()
                .p_5()
                .rounded_xl()
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .shadow(crate::components::elevated_shadow(theme))
                .text_color(theme.foreground)
                .occlude()
                .v_flex()
                .gap_3()
                .child(
                    div()
                        .size(px(64.))
                        .relative()
                        .child(
                            div()
                                .size(px(56.))
                                .rounded_xl()
                                .bg(theme.secondary)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(gpui_kit::assets::IconName::Monitor).size(px(28.))),
                        )
                        .child(
                            div()
                                .absolute()
                                .right_0()
                                .bottom_0()
                                .size(px(24.))
                                .rounded_full()
                                .bg(theme.popover)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(gpui_kit::assets::IconName::ShieldAlert)
                                        .size(px(18.))
                                        .text_color(badge_tone),
                                ),
                        ),
                )
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(cert.title()),
                )
                .child(div().h_flex().child(server_pill(theme, &cert.host_port)))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(cert.body()),
                )
                .when_some(cert.previous.as_deref(), |view, previous| {
                    view.child(
                        div()
                            .v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.muted_foreground)
                                    .child("PREVIOUSLY TRUSTED"),
                            )
                            .child(fingerprint_box(theme, previous, true)),
                    )
                })
                .child(
                    div()
                        .v_flex()
                        .gap_1()
                        .child(
                            div()
                                .h_flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.muted_foreground)
                                        .child(if changed {
                                            "NEW FINGERPRINT"
                                        } else {
                                            "SHA-256 FINGERPRINT"
                                        }),
                                )
                                .child(
                                    Button::new("cert-copy")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Copy)
                                        .label("Copy")
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                fingerprint.clone(),
                                            ))
                                        }),
                                ),
                        )
                        .child(fingerprint_box(theme, &cert.fingerprint, false))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("On the server, run: openssl x509 -noout -fingerprint -sha256 -in ~/.local/state/selkies/selkies.pem (or the file passed to --https-cert)"),
                        ),
                )
                .child(
                    div()
                        .h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("cert-cancel")
                                .outline()
                                .small()
                                .label("Cancel")
                                .on_click(move |e, w, cx| on_cancel(e, w, cx)),
                        )
                        .child(trust),
                ),
        )
}

// ---------------------------------------------------------------------------------------
// The Library for a direct server

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecurityBadge {
    Https,
    Pinned,
    LoopbackHttp,
}

impl SecurityBadge {
    pub fn text(self) -> &'static str {
        match self {
            Self::Https => "HTTPS",
            Self::Pinned => "HTTPS · certificate trusted manually",
            Self::LoopbackHttp => "HTTP · this Mac only",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusDot {
    /// Open in a window on this Mac.
    Here,
    /// Other pages are connected.
    Others,
    Idle,
}

/// The card's status line and dot: open here first, then the other pages.
pub fn status_line(open_here: bool, others: usize) -> (String, StatusDot) {
    let others_text = match others {
        0 => None,
        1 => Some("1 other page connected".to_string()),
        n => Some(format!("{n} other pages connected")),
    };
    match (open_here, others_text) {
        (true, Some(others)) => (format!("Open in a window · {others}"), StatusDot::Here),
        (true, None) => ("Open in a window".into(), StatusDot::Here),
        (false, Some(others)) => (others, StatusDot::Others),
        (false, None) => ("Not open".into(), StatusDot::Idle),
    }
}

#[derive(Clone, Default)]
pub struct SelkiesHomeProps {
    pub app_name: String,
    pub display_name: String,
    pub base_url: String,
    pub login_label: String,
    pub security: Option<SecurityBadge>,
    pub fingerprint: Option<String>,
    pub open_here: bool,
    pub others: usize,
    pub view_only: bool,
    pub thumbnail: Option<Arc<RenderImage>>,
    pub thumbnail_loading: bool,
    pub files_pending: bool,
    pub accepts_drop: bool,
    pub busy: Option<String>,
    pub status: Option<String>,
    pub menu_open: bool,
    /// Files waiting to be copied to the Desktop (Open…, Finder): the one-row chooser.
    pub pending_copy: Option<String>,
    /// Open in Browser: hidden for a token-only server, whose page would answer 401.
    pub can_open_web: bool,
}

#[derive(Clone)]
pub struct SelkiesHomeHandlers {
    pub on_open: Click,
    pub on_files: Click,
    pub on_refresh: Click,
    pub on_menu_open: FlagHandler,
    pub on_open_web: Click,
    pub on_disconnect: Click,
    pub on_forget: Click,
    pub on_drop: DropHandler,
    pub on_copy_pending: Click,
    pub on_cancel_pending: Click,
}

fn security_icon(theme: &Theme, badge: SecurityBadge) -> Option<impl IntoElement> {
    match badge {
        SecurityBadge::Https => None,
        SecurityBadge::Pinned => Some(
            Icon::new(gpui_kit::assets::IconName::Lock)
                .size(px(12.))
                .text_color(theme.muted_foreground),
        ),
        SecurityBadge::LoopbackHttp => Some(
            Icon::new(gpui_kit::assets::IconName::LockOpen)
                .size(px(12.))
                .text_color(theme.warning),
        ),
    }
}

fn header_pill(theme: &Theme, props: &SelkiesHomeProps) -> impl IntoElement {
    let url = props.base_url.clone();
    div()
        .id("server-chip")
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
        .child(div().min_w_0().truncate().child(props.display_name.clone()))
        .when_some(
            props.security.and_then(|b| security_icon(theme, b)),
            |pill, icon| pill.child(icon),
        )
        .tooltip(move |window, cx| Tooltip::new(url.clone()).build(window, cx))
}

fn menu_row(theme: &Theme, key: &'static str, value: impl IntoElement) -> Div {
    div()
        .h_flex()
        .items_start()
        .gap_2()
        .text_sm()
        .child(
            div()
                .w(px(80.))
                .flex_none()
                .text_color(theme.muted_foreground)
                .child(key),
        )
        .child(div().flex_1().min_w_0().child(value))
}

fn server_menu(
    props: &SelkiesHomeProps,
    enabled: bool,
    handlers: &SelkiesHomeHandlers,
) -> impl IntoElement {
    let trigger = Button::new("server-menu")
        .outline()
        .xsmall()
        .icon(gpui_kit::assets::IconName::Server)
        .dropdown_caret(true)
        .tooltip("Server")
        .disabled(!enabled)
        .selected(props.menu_open);
    let on_open_change = handlers.on_menu_open.clone();
    let props = props.clone();
    let handlers = handlers.clone();
    Popover::new("server-menu-popover")
        .anchor(Anchor::TopRight)
        .overlay_closable(true)
        .open(props.menu_open && enabled)
        .on_open_change(move |open, window, cx| on_open_change(*open, window, cx))
        .trigger(trigger)
        .content(move |_, _, cx| {
            let theme = cx.theme().clone();
            let security = props.security.unwrap_or(SecurityBadge::Https);
            let fingerprint = props.fingerprint.clone();
            let security_value = div()
                .id("menu-security")
                .h_flex()
                .items_center()
                .gap_1()
                .when_some(security_icon(&theme, security), |row, icon| row.child(icon))
                .child(security.text())
                .when_some(fingerprint, |row, fingerprint| {
                    row.tooltip(move |window, cx| {
                        Tooltip::new(fingerprint_lines(&fingerprint).join("\n")).build(window, cx)
                    })
                });
            let (on_web, on_disconnect, on_forget) = (
                handlers.on_open_web.clone(),
                handlers.on_disconnect.clone(),
                handlers.on_forget.clone(),
            );
            div()
                .w(px(crate::account::MENU_WIDTH))
                .v_flex()
                .gap_3()
                .p_1()
                .child(
                    div()
                        .v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(props.display_name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .font_family(theme.mono_font_family.clone())
                                .truncate()
                                .child(props.base_url.clone()),
                        ),
                )
                .child(
                    div()
                        .v_flex()
                        .gap_1p5()
                        .child(menu_row(&theme, "Address", props.display_name.clone()))
                        .child(menu_row(&theme, "Login", login_text(&props.login_label)))
                        .child(menu_row(&theme, "Security", security_value))
                        .child(menu_row(&theme, "Mode", "WebSockets")),
                )
                .child(div().h(px(1.)).w_full().bg(theme.border))
                .child(
                    div()
                        .v_flex()
                        .gap_0p5()
                        .when(props.can_open_web, |rows| {
                            rows.child(
                                Button::new("menu-open-web")
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .justify_start()
                                    .icon(IconName::ExternalLink)
                                    .label("Open in Browser")
                                    .on_click(move |e, w, cx| on_web(e, w, cx)),
                            )
                        })
                        .child(
                            Button::new("menu-disconnect")
                                .ghost()
                                .small()
                                .w_full()
                                .justify_start()
                                .icon(gpui_kit::assets::IconName::LogOut)
                                .label("Disconnect")
                                .on_click(move |e, w, cx| on_disconnect(e, w, cx)),
                        )
                        .child(
                            Button::new("menu-forget")
                                .ghost()
                                .small()
                                .w_full()
                                .justify_start()
                                .text_color(theme.danger)
                                .icon(gpui_kit::assets::IconName::Trash)
                                .label("Forget This Server…")
                                .on_click(move |e, w, cx| on_forget(e, w, cx)),
                        ),
                )
        })
}

/// The menu's Login value: `ubuntu@` reads as `ubuntu`.
fn login_text(label: &str) -> String {
    label.strip_suffix('@').unwrap_or(label).to_string()
}

fn desktop_card(
    theme: &Theme,
    props: &SelkiesHomeProps,
    enabled: bool,
    handlers: &SelkiesHomeHandlers,
) -> Div {
    let (line, dot) = status_line(props.open_here, props.others);
    let dot_color = match dot {
        StatusDot::Here => theme.green,
        StatusDot::Others => theme.blue,
        StatusDot::Idle => theme.muted_foreground,
    };
    let subtitle = if props.view_only {
        format!("{} · View only", props.login_label)
    } else {
        props.login_label.clone()
    };
    let preview = div()
        .w_full()
        .h(px(270.))
        .rounded(px(12.))
        .overflow_hidden()
        .bg(theme.secondary)
        .child(match props.thumbnail.clone() {
            Some(thumb) => img(thumb)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None if props.thumbnail_loading => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child(Icon::new(gpui_kit::assets::IconName::Monitor).size(px(32.)))
                .into_any_element(),
        });
    let (on_open, on_files) = (handlers.on_open.clone(), handlers.on_files.clone());
    let card = div()
        .w(px(480.))
        .min_w(px(320.))
        .max_w_full()
        .relative()
        .p_3()
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .v_flex()
        .gap_3()
        .child(preview)
        .child(
            div()
                .h_flex()
                .items_end()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(props.display_name.clone()),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .truncate()
                                .child(subtitle),
                        )
                        .child(
                            div()
                                .h_flex()
                                .items_center()
                                .gap_1p5()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(div().size(px(6.)).rounded_full().bg(dot_color))
                                .child(line),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("desktop-files")
                                .ghost()
                                .small()
                                .icon(IconName::FolderOpen)
                                .tooltip("Files")
                                .loading(props.files_pending)
                                .disabled(!enabled || props.files_pending)
                                .on_click(move |e, w, cx| on_files(e, w, cx)),
                        )
                        .child(
                            Button::new("desktop-open")
                                .primary()
                                .small()
                                .label(if props.open_here {
                                    "Show Window"
                                } else {
                                    "Open"
                                })
                                .disabled(!enabled)
                                .on_click(move |e, w, cx| on_open(e, w, cx)),
                        ),
                ),
        );
    if !props.accepts_drop {
        return card;
    }
    let ring = theme.blue;
    let on_drop = handlers.on_drop.clone();
    card.drag_over::<ExternalPaths>(move |style, _, _, _| style.border_color(ring))
        .on_drop(move |paths: &ExternalPaths, window, cx| {
            cx.stop_propagation();
            on_drop(paths.paths().to_vec(), window, cx)
        })
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded_xl()
                .border_2()
                .border_color(theme.blue)
                .bg(theme.drop_target)
                .flex()
                .items_center()
                .justify_center()
                .invisible()
                .drag_over::<ExternalPaths>(|style, _, _, _| style.visible())
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .px_4()
                        .py_2()
                        .rounded_full()
                        .bg(theme.popover)
                        .border_1()
                        .border_color(theme.border)
                        .shadow_md()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(Icon::new(gpui_kit::assets::IconName::FolderUp))
                        .child("Copy to Desktop"),
                ),
        )
}

fn pending_copy(theme: &Theme, label: &str, handlers: &SelkiesHomeHandlers) -> Div {
    let (on_copy, on_cancel) = (
        handlers.on_copy_pending.clone(),
        handlers.on_cancel_pending.clone(),
    );
    div()
        .max_w(px(480.))
        .p_3()
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .h_flex()
        .items_center()
        .gap_3()
        .child(Icon::new(gpui_kit::assets::IconName::FolderUp))
        .child(div().flex_1().min_w_0().text_sm().child(label.to_string()))
        .child(
            Button::new("pending-cancel")
                .ghost()
                .small()
                .label("Cancel")
                .on_click(move |e, w, cx| on_cancel(e, w, cx)),
        )
        .child(
            Button::new("pending-copy")
                .primary()
                .small()
                .label("Copy to Desktop")
                .on_click(move |e, w, cx| on_copy(e, w, cx)),
        )
}

pub fn render_home(theme: &Theme, props: &SelkiesHomeProps, handlers: SelkiesHomeHandlers) -> Div {
    let enabled = props.busy.is_none();
    let on_refresh = handlers.on_refresh.clone();
    let refreshing = props.busy.is_some();
    let header = div()
        .h_flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .flex_1()
                .h_flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .flex_none()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(props.app_name.clone()),
                )
                .child(header_pill(theme, props)),
        )
        .child(
            div()
                .flex_none()
                .h_flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new("refresh")
                        .outline()
                        .xsmall()
                        .icon(IconName::RefreshCw)
                        .label(if refreshing {
                            "Refreshing…"
                        } else {
                            "Refresh"
                        })
                        .loading(refreshing)
                        .disabled(!enabled)
                        .on_click(move |e, w, cx| on_refresh(e, w, cx)),
                )
                .child(server_menu(props, enabled, &handlers)),
        );
    div()
        .size_full()
        .bg(theme.background)
        .text_color(theme.foreground)
        .flex()
        .justify_center()
        .child(
            div()
                .w_full()
                .max_w(px(crate::library::LIBRARY_MAX_WIDTH))
                .px_6()
                .py_4()
                .v_flex()
                .gap_5()
                .child(header)
                .when_some(props.status.clone(), |view, status| {
                    view.child(div().text_sm().text_color(theme.danger).child(status))
                })
                .when_some(props.pending_copy.as_deref(), |view, label| {
                    view.child(pending_copy(theme, label, &handlers))
                })
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted_foreground)
                        .child("DESKTOP"),
                )
                .child(desktop_card(theme, props, enabled, &handlers)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line_dot_tone_follows_open_here_then_others() {
        assert_eq!(status_line(false, 0), ("Not open".into(), StatusDot::Idle));
        assert_eq!(
            status_line(true, 0),
            ("Open in a window".into(), StatusDot::Here)
        );
        assert_eq!(
            status_line(false, 1),
            ("1 other page connected".into(), StatusDot::Others)
        );
        assert_eq!(
            status_line(true, 3),
            (
                "Open in a window · 3 other pages connected".into(),
                StatusDot::Here
            )
        );
    }

    #[test]
    fn certificate_sheet_uses_danger_only_when_the_certificate_changed() {
        let first = CertificateProps {
            host: "desktop.lab".into(),
            host_port: "desktop.lab:8443".into(),
            fingerprint: "AB:CD".into(),
            previous: None,
        };
        assert!(!first.changed());
        assert_eq!(first.trust_label(), "Trust and Connect");
        assert_eq!(first.title(), "Skerry can’t verify “desktop.lab”");
        let changed = CertificateProps {
            previous: Some("00:11".into()),
            ..first
        };
        assert!(changed.changed());
        assert_eq!(changed.trust_label(), "Trust New Certificate");
        assert!(changed.body().contains("container recreated"));
    }

    #[test]
    fn changed_certificate_shows_previous_fingerprint() {
        let fingerprint = (0..32)
            .map(|i| format!("{i:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        let lines = fingerprint_lines(&fingerprint);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "00:01:02:03:04:05:06:07");
    }

    #[test]
    fn the_menu_login_drops_the_at_sign() {
        assert_eq!(login_text("ubuntu@"), "ubuntu");
        assert_eq!(login_text("Session token"), "Session token");
    }
}
