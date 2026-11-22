//! The Launcher's direct Selkies side (`ARCH-036`): the Connect card's Selkies tab, the
//! certificate prompt, and the Library for a direct server (one Desktop card). The SealSkin
//! side stays in `app.rs`; the Launcher holds `api` or `selkies`, never both.

use super::files_window::{FilesOwner, FilesSource, FilesWindow};
use super::session_window::{OpenWindows, SessionServer};
use super::{
    close_library_files, decode_image, folder_notice, raise_files, split_drop, Launcher, LoadGate,
    LoadKind, APP_NAME,
};
use crate::direct::{
    self, GateError, GateReport, NetworkReason, SavedServer, SelkiesAuth, SelkiesServer,
    ServerSource,
};
use crate::open_target::{Incoming, OpenTarget};
use crate::persist::WriteQueue;
use crate::tls::{self, Pin};
use gpui_kit::component::input::InputState;
use gpui_kit::component::Theme;
use gpui_kit::{prelude::*, App, Context, Div, Entity, KeyDownEvent, RenderImage, Window};
use skerry_ui::selkies::{
    self as ui, AlertTone, CertificateProps, ConnectTab, RecentServerRow, SecurityBadge,
    SelkiesConnectHandlers, SelkiesConnectProps, SelkiesField, SelkiesHomeHandlers,
    SelkiesHomeProps, SelkiesProblem,
};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

/// The Library thumbnail's longest edge.
const THUMBNAIL_MAX_EDGE: u32 = 512;

/// A connected direct server, as the Library shows it.
pub(super) struct SelkiesHome {
    pub(super) server: SelkiesServer,
    report: GateReport,
    thumbnail: Option<Arc<RenderImage>>,
    thumbnail_loading: bool,
    /// Bumped per fetch, so an older screenshot never replaces a newer one.
    thumbnail_generation: u64,
    pub(super) menu_open: bool,
    pub(super) pending_forget: bool,
    /// Files from Open… or Finder waiting for "Copy to Desktop" (`ARCH-028`: nothing is
    /// copied without a click).
    pending_copy: Vec<PathBuf>,
    status: Option<String>,
}

/// The trust prompt for a server whose certificate the roots refused.
pub(super) struct CertPrompt {
    server: SelkiesServer,
    leaf: Pin,
    previous: Option<Pin>,
}

/// The Selkies tab's typed state, kept while the other tab shows.
pub(super) struct SelkiesForm {
    pub(super) inputs: ui::SelkiesInputs,
    pub(super) remember: bool,
    /// The session token a pasted link carried (the chip).
    pub(super) token: Option<String>,
    /// The base of the address the token came with: it is sent only there.
    pub(super) token_base: Option<String>,
    pub(super) problem: Option<SelkiesProblem>,
    pub(super) status: Option<String>,
    /// The host the gate is checking, for "Checking <host>…".
    pub(super) checking: Option<String>,
    pub(super) recent: Vec<SavedServer>,
    pub(super) cert: Option<CertPrompt>,
    /// Applied on the next render (setting an input needs a Window).
    pub(super) fill: Option<Fill>,
    pub(super) focus: Option<Focus>,
}

#[derive(Default)]
pub(super) struct Fill {
    address: Option<String>,
    username: Option<String>,
    password: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Focus {
    Address,
    Username,
    Password,
    /// The Launcher itself, so Escape and Return reach the certificate prompt.
    Launcher,
}

/// Whether a token that came with `token_base` may go to `base`: only the server whose
/// link carried it (K-QA-2). A token with no recorded base is kept.
pub(super) fn token_belongs(token_base: Option<&str>, base: &str) -> bool {
    token_base.is_none_or(|from| from == base)
}

/// Drops `token` (and its chip's error) unless it came with `base`.
pub(super) fn retarget_token(
    token: &mut Option<String>,
    token_base: &mut Option<String>,
    problem: &mut Option<SelkiesProblem>,
    base: &str,
) {
    if token_belongs(token_base.as_deref(), base) {
        return;
    }
    *token = None;
    *token_base = None;
    if matches!(
        problem,
        Some(SelkiesProblem::Field {
            field: SelkiesField::Token,
            ..
        })
    ) {
        *problem = None;
    }
}

impl SelkiesForm {
    /// A token from a link (or a saved entry) for `base`.
    pub(super) fn set_token(&mut self, token: Option<String>, base: &str) {
        self.token_base = token.as_ref().map(|_| base.to_string());
        self.token = token;
    }

    /// Drops the token when the address now names another server.
    pub(super) fn retarget(&mut self, base: &str) {
        retarget_token(
            &mut self.token,
            &mut self.token_base,
            &mut self.problem,
            base,
        );
    }

    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            inputs: ui::new_selkies_inputs(window, cx),
            remember: true,
            token: None,
            token_base: None,
            problem: None,
            status: None,
            checking: None,
            recent: Vec::new(),
            cert: None,
            fill: None,
            focus: None,
        }
    }
}

/// The SealSkin Connect state a Selkies connect clears (review 11). `startup_credential`
/// isn't here: the saved key stays for the SealSkin tab's empty Connect.
pub(super) struct SealSkinLeftovers<'a> {
    pub(super) key_problem: &'a mut Option<skerry_ui::connect::KeyProblem>,
    pub(super) refused_server: &'a mut Option<String>,
    pub(super) key_hint: &'a mut Option<String>,
    pub(super) different_key: &'a mut bool,
    pub(super) pending_save: &'a mut Option<crate::sealskin::Credential>,
    pub(super) retry_api: &'a mut Option<crate::sealskin::SealSkinClient>,
    pub(super) status: &'a mut Option<String>,
}

impl SealSkinLeftovers<'_> {
    pub(super) fn clear(self) {
        *self.key_problem = None;
        *self.refused_server = None;
        *self.key_hint = None;
        *self.different_key = false;
        *self.pending_save = None;
        *self.retry_api = None;
        *self.status = None;
    }
}

/// How the card shows a gate refusal (design §3). `None` for the certificate cases, which
/// open the trust prompt instead.
pub(super) fn problem_for(error: &GateError, host: &str, has_chip: bool) -> Option<SelkiesProblem> {
    let field = |field, error: &GateError| SelkiesProblem::Field {
        field,
        text: error.message(host),
    };
    let alert = |tone, title: String, body: String| SelkiesProblem::Alert {
        tone,
        title,
        body,
        server: Some(host.to_string()),
        switch_to_sealskin: false,
    };
    Some(match error {
        GateError::Untrusted(_) | GateError::PinMismatch { .. } => return None,
        GateError::NeedsLogin | GateError::BadLogin => field(SelkiesField::Login, error),
        GateError::NeedsToken => field(SelkiesField::Address, error),
        GateError::BadToken if has_chip => field(SelkiesField::Token, error),
        GateError::BadToken => field(SelkiesField::Address, error),
        GateError::WebRtc { dual_mode } => {
            let mut body = "Skerry needs the WebSockets mode: ask its operator to run Selkies with --mode=websockets.".to_string();
            if *dual_mode {
                body.push_str(" Dual mode is on, but the server is in WebRTC mode right now; Skerry won’t switch it, because that would change the stream for everyone watching.");
            }
            alert(
                AlertTone::Warning,
                "This server streams over WebRTC".into(),
                body,
            )
        }
        GateError::Network(reason) => {
            let reason = match reason {
                NetworkReason::Timeout => "The server didn’t answer within 10 seconds.".to_string(),
                NetworkReason::Refused => "Nothing is listening at that address and port.".into(),
                NetworkReason::Dns => "There’s no server with that name.".into(),
                NetworkReason::Tls(detail) => format!("The secure connection failed: {detail}."),
                NetworkReason::Other(_) => error.message(host),
            };
            alert(
                AlertTone::Danger,
                format!("Couldn’t reach {host}"),
                format!("{reason} Check the address, and that the server is running."),
            )
        }
        GateError::NotSelkies => alert(
            AlertTone::Danger,
            "This isn’t a Selkies server".into(),
            "/api/status didn’t answer as Selkies does. Check the address and subfolder.".into(),
        ),
        GateError::SealSkinAddress => SelkiesProblem::Alert {
            tone: AlertTone::Info,
            title: "This is a SealSkin address".into(),
            body: "Connect with an application key instead.".into(),
            server: Some(host.to_string()),
            switch_to_sealskin: true,
        },
        GateError::Starting => alert(
            AlertTone::Warning,
            format!("{host} is still starting"),
            "Try again in a moment.".into(),
        ),
        GateError::Server(_) => alert(
            AlertTone::Danger,
            "The server returned an error".into(),
            error.message(host),
        ),
    })
}

/// Which field a refusal focuses: Username for a missing login (Password once a username
/// is typed), Password for a wrong one, the address otherwise.
pub(super) fn focus_for(error: &GateError, username_typed: bool) -> Option<Focus> {
    match error {
        GateError::NeedsLogin if username_typed => Some(Focus::Password),
        GateError::NeedsLogin => Some(Focus::Username),
        GateError::BadLogin => Some(Focus::Password),
        GateError::NeedsToken
        | GateError::BadToken
        | GateError::WebRtc { .. }
        | GateError::Network(_)
        | GateError::NotSelkies => Some(Focus::Address),
        _ => None,
    }
}

/// Items for a connected direct server: files wait for Copy to Desktop; links can't open
/// here (`ARCH-036`), and folders go through Files.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct SelkiesReceive {
    pub(super) files: Vec<PathBuf>,
    pub(super) links: usize,
    pub(super) folders: usize,
}

pub(super) fn plan_receive(items: Vec<Incoming>) -> SelkiesReceive {
    let mut plan = SelkiesReceive::default();
    for item in items {
        match item {
            Incoming::Target(OpenTarget::File(path)) => plan.files.push(path),
            Incoming::Target(_) | Incoming::NoAddress | Incoming::Rejected { .. } => {
                plan.links += 1
            }
            Incoming::Folder(_) => plan.folders += 1,
            Incoming::Ignored => {}
        }
    }
    plan
}

pub(super) const LINKS_NOTICE: &str = "Links open only in SealSkin sessions.";

/// "Copy “a.pdf” to the Desktop of host?" or "Copy 3 files …".
fn pending_copy_label(files: &[PathBuf], host: &str) -> Option<String> {
    match files {
        [] => None,
        [one] => Some(format!(
            "Copy “{}” to the Desktop of {host}?",
            super::file_name(one)
        )),
        many => Some(format!(
            "Copy {} files to the Desktop of {host}?",
            many.len()
        )),
    }
}

/// Pages other than this Mac's window, from `api/sessions`.
pub(super) fn other_pages(report: &GateReport, open_here: bool) -> usize {
    report.pages.len().saturating_sub(usize::from(open_here))
}

/// The newest page is this Mac's window after Open: "View only" when it is a viewer
/// (best effort, `ARCH-GAP-DC-07`).
fn view_only(report: &GateReport, open_here: bool) -> bool {
    open_here
        && report
            .pages
            .iter()
            .max_by(|a, b| a.connected_at.total_cmp(&b.connected_at))
            .is_some_and(|page| page.role == "viewer")
}

fn recent_row(saved: &SavedServer) -> RecentServerRow {
    RecentServerRow {
        url: saved.url.clone(),
        label: saved.label(),
        login: saved.login_label(),
        pinned: saved.is_pinned(),
        loopback: saved.is_loopback_http(),
    }
}

fn badge(server: &SelkiesServer) -> SecurityBadge {
    match server.security() {
        direct::Security::Https => SecurityBadge::Https,
        direct::Security::HttpsPinned(_) => SecurityBadge::Pinned,
        direct::Security::LoopbackHttp => SecurityBadge::LoopbackHttp,
    }
}

fn input_text(input: &Entity<InputState>, cx: &App) -> String {
    input.read(cx).value().to_string()
}

impl SelkiesHome {
    fn new(server: SelkiesServer, report: GateReport) -> Self {
        Self {
            server,
            report,
            thumbnail: None,
            thumbnail_loading: false,
            thumbnail_generation: 0,
            menu_open: false,
            pending_forget: false,
            pending_copy: Vec::new(),
            status: None,
        }
    }

    /// Items for the connected server: files wait for Copy to Desktop, links get the notice.
    fn receive(&mut self, items: Vec<Incoming>) {
        let plan = plan_receive(items);
        self.pending_copy.extend(plan.files);
        self.pending_copy.truncate(crate::open_target::QUEUE_CAP);
        self.status = if plan.links > 0 {
            Some(LINKS_NOTICE.into())
        } else if plan.folders > 0 {
            Some(folder_notice(plan.folders, !self.pending_copy.is_empty()))
        } else {
            None
        };
    }

    /// Takes everything that queued before the connect (review 11: nothing waits forever).
    fn take_queued(&mut self, queued: &mut Vec<Incoming>) {
        self.receive(std::mem::take(queued));
    }
}

impl Launcher {
    /// The saved servers, read again after every queued write.
    pub(super) fn reload_recent(&mut self, cx: &mut Context<Self>) {
        let read = cx.global::<WriteQueue>().saved_servers();
        let window = self.window;
        cx.spawn(async move |this, cx| {
            let result = read.await.unwrap_or_else(|_| Ok(Vec::new()));
            let _ = window.update(cx, |_, _, cx| {
                this.update(cx, |this, cx| {
                    match result {
                        Ok(recent) => this.selkies_form.recent = recent,
                        Err(error) => this.selkies_form.status = Some(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    pub(super) fn set_connect_tab(&mut self, tab: ConnectTab, cx: &mut Context<Self>) {
        if self.busy.is_none() {
            self.connect_tab = tab;
            self.selkies_form.focus = Some(match tab {
                ConnectTab::Selkies => Focus::Address,
                ConnectTab::SealSkin => Focus::Launcher,
            });
            if tab == ConnectTab::SealSkin {
                self.focus_key_input = !self.recovery.is_some();
            }
            cx.notify();
        }
    }

    /// The address field changed: a pasted link's login moves into the fields and its token
    /// into the chip; the field keeps the cleaned address. Editing clears its error.
    pub(super) fn selkies_address_changed(&mut self, cx: &mut Context<Self>) {
        let text = input_text(&self.selkies_form.inputs.address, cx);
        let form = &mut self.selkies_form;
        if matches!(
            form.problem,
            Some(SelkiesProblem::Alert { .. })
                | Some(SelkiesProblem::Field {
                    field: SelkiesField::Address | SelkiesField::Token,
                    ..
                })
        ) {
            form.problem = None;
        }
        form.status = None;
        if let Ok(address) = direct::parse_address(&text) {
            let carried =
                address.token.is_some() || address.username.is_some() || address.password.is_some();
            let base = address.base.to_string();
            if address.token.is_some() {
                form.set_token(address.token.clone(), &base);
            } else {
                form.retarget(&base);
            }
            if carried {
                form.fill = Some(Fill {
                    address: Some(address.base.to_string()),
                    username: address.username,
                    password: address.password,
                });
            }
        }
        cx.notify();
    }

    pub(super) fn selkies_login_changed(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.selkies_form.problem,
            Some(SelkiesProblem::Field {
                field: SelkiesField::Login,
                ..
            })
        ) {
            self.selkies_form.problem = None;
            cx.notify();
        }
    }

    /// A pin trusted before for this address, so retyping it doesn't ask again.
    fn known_pin(&self, base: &str) -> Option<Pin> {
        self.selkies_form
            .recent
            .iter()
            .find(|saved| saved.url == base)
            .and_then(|saved| saved.certificate_sha256.as_deref())
            .and_then(tls::parse_fingerprint)
    }

    pub(super) fn submit_selkies(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let form = &mut self.selkies_form;
        form.problem = None;
        form.status = None;
        let text = input_text(&form.inputs.address, cx);
        let address = match direct::parse_address(&text) {
            Ok(address) => address,
            Err(direct::AddressError::LooksLikeKey) => {
                // A key in the address field: it belongs on the SealSkin tab.
                self.connect_tab = ConnectTab::SealSkin;
                self.move_to_key = Some(text.trim().to_string());
                self.selkies_form.fill = Some(Fill {
                    address: Some(String::new()),
                    ..Fill::default()
                });
                self.status = Some(direct::AddressError::LooksLikeKey.to_string());
                cx.notify();
                return;
            }
            Err(error) => {
                form.status = Some(error.to_string());
                form.focus = Some(Focus::Address);
                cx.notify();
                return;
            }
        };
        let base = address.base.to_string();
        if address.token.is_some() {
            form.set_token(address.token.clone(), &base);
        } else {
            // A token from another server's link never goes here.
            form.retarget(&base);
        }
        let typed_user = input_text(&form.inputs.username, cx);
        let typed_password = input_text(&form.inputs.password, cx);
        // The fields win over a link's userinfo.
        let username = if typed_user.is_empty() {
            address.username.clone().unwrap_or_default()
        } else {
            typed_user
        };
        let password = if typed_password.is_empty() {
            address.password.clone().unwrap_or_default()
        } else {
            typed_password
        };
        form.fill = Some(Fill {
            address: Some(address.base.to_string()),
            username: Some(username.clone()),
            password: Some(password.clone()),
        });
        let auth = SelkiesAuth::new(&username, &password, form.token.clone());
        let remember = form.remember;
        let pin = self.known_pin(address.base.as_str());
        match SelkiesServer::new(address.base, auth, pin, ServerSource::Typed, remember) {
            Ok(server) => self.connect_selkies(server, LoadKind::Connect, cx),
            Err(error) => self.selkies_form.status = Some(error),
        }
        cx.notify();
    }

    /// A Recent Servers row: fill the form, and connect when nothing more needs typing.
    pub(super) fn pick_recent(&mut self, url: &str, cx: &mut Context<Self>) {
        let Some(saved) = self
            .selkies_form
            .recent
            .iter()
            .find(|saved| saved.url == url)
            .cloned()
        else {
            return;
        };
        let form = &mut self.selkies_form;
        form.problem = None;
        form.status = None;
        form.set_token(saved.token.clone(), &saved.url);
        form.remember = saved.password.is_some() || saved.token.is_some() || form.remember;
        form.fill = Some(Fill {
            address: Some(saved.url.clone()),
            username: Some(saved.username.clone().unwrap_or_default()),
            password: Some(saved.password.clone().unwrap_or_default()),
        });
        if !saved.ready() {
            form.focus = Some(Focus::Password);
            cx.notify();
            return;
        }
        match SelkiesServer::from_saved(&saved) {
            Some(server) => self.connect_selkies(server, LoadKind::Connect, cx),
            None => self.selkies_form.status = Some(direct::AddressError::Invalid.to_string()),
        }
        cx.notify();
    }

    pub(super) fn remove_recent(&mut self, url: &str, cx: &mut Context<Self>) {
        self.selkies_form.recent.retain(|saved| saved.url != url);
        let done = cx.global::<WriteQueue>().forget_server(url);
        self.on_written(
            done,
            |this, error| this.selkies_form.status = Some(error),
            cx,
        );
        self.reload_recent(cx);
        cx.notify();
    }

    /// Runs the gate on `server` (Connect, or a Refresh of the connected one).
    pub(super) fn connect_selkies(
        &mut self,
        server: SelkiesServer,
        kind: LoadKind,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        let connecting = kind == LoadKind::Connect;
        let gate = LoadGate(self.server_generation);
        self.busy = Some(
            if connecting {
                "Connecting…"
            } else {
                "Refreshing…"
            }
            .into(),
        );
        if connecting {
            self.selkies_form.checking = Some(server.host_port());
            self.selkies_form.problem = None;
            self.selkies_form.status = None;
        }
        let checked = server.clone();
        let pending = self.rt.spawn(async move { direct::gate(&checked).await });
        let window = self.window;
        cx.spawn(async move |this, cx| {
            let result = pending
                .await
                .unwrap_or(Err(GateError::Network(NetworkReason::Other(
                    "interrupted".into(),
                ))));
            let _ = window.update(cx, |_, _, cx| {
                this.update(cx, |this, cx| {
                    if !gate.admits(this.server_generation) {
                        return;
                    }
                    this.busy = None;
                    this.selkies_form.checking = None;
                    match result {
                        Ok(report) if connecting => this.selkies_connected(server, report, cx),
                        Ok(report) => this.selkies_refreshed(report, kind, cx),
                        Err(error) => this.selkies_refused(server, error, connecting, cx),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }

    fn selkies_connected(
        &mut self,
        server: SelkiesServer,
        report: GateReport,
        cx: &mut Context<Self>,
    ) {
        // One server at a time: any SealSkin state goes (`leave_server` bumps the
        // generation, so SealSkin work still in flight is dropped).
        self.leave_server();
        self.clear_sealskin_for_selkies();
        self.selkies_form.cert = None;
        self.selkies_form.problem = None;
        self.selkies_form.set_token(None, "");
        self.selkies_form.fill = Some(Fill {
            address: Some(String::new()),
            username: Some(String::new()),
            password: Some(String::new()),
        });
        self.connect_tab = ConnectTab::Selkies;
        self.preferences = cx.global::<WriteQueue>().read(&server.prefs_key());
        if server.source() != ServerSource::Environment {
            let done = cx
                .global::<WriteQueue>()
                .save_server(server.saved(), server.remember());
            self.on_written(
                done,
                |this, error| {
                    if let Some(home) = this.selkies.as_mut() {
                        home.status = Some(error);
                    }
                },
                cx,
            );
            self.reload_recent(cx);
        }
        let mut home = SelkiesHome::new(server, report);
        // Items that arrived before this connect.
        home.take_queued(&mut self.pending_opens);
        self.selkies = Some(home);
        self.fetch_selkies_thumbnail(cx);
    }

    /// What connecting to a direct server clears of the SealSkin side (review 11): a
    /// dormant-key panel or a pending save must not outlive the switch. The saved key stays
    /// as `startup_credential`, so the SealSkin tab's empty Connect still retries it.
    pub(super) fn clear_sealskin_for_selkies(&mut self) {
        self.clear_recovery();
        SealSkinLeftovers {
            key_problem: &mut self.key_problem,
            refused_server: &mut self.refused_server,
            key_hint: &mut self.key_hint,
            different_key: &mut self.different_key,
            pending_save: &mut self.pending_save,
            retry_api: &mut self.retry_api,
            status: &mut self.status,
        }
        .clear();
    }

    fn selkies_refreshed(&mut self, report: GateReport, kind: LoadKind, cx: &mut Context<Self>) {
        let Some(home) = self.selkies.as_mut() else {
            return;
        };
        home.report = report;
        home.status = None;
        self.fetch_selkies_thumbnail(cx);
        if kind == LoadKind::ManualRefresh {
            self.show_toast("Library refreshed", cx);
        }
    }

    fn selkies_refused(
        &mut self,
        server: SelkiesServer,
        error: GateError,
        connecting: bool,
        cx: &mut Context<Self>,
    ) {
        if !connecting && error.keeps_library() {
            // ARCH-009's Refresh rule: a server that didn't answer is kept.
            if let Some(home) = self.selkies.as_mut() {
                home.status = Some(error.message(&server.host_port()));
            }
            return;
        }
        if !connecting {
            // A refusal on Refresh: back to Connect, the form filled.
            self.leave_server();
            close_library_files(cx, |id| id.starts_with("selkies|"));
            self.connect_tab = ConnectTab::Selkies;
            let auth = server.auth();
            self.selkies_form
                .set_token(auth.token.clone(), server.base().as_str());
            self.selkies_form.fill = Some(Fill {
                address: Some(server.base().to_string()),
                username: auth.basic.as_ref().map(|login| login.username.clone()),
                password: auth.basic.as_ref().map(|login| login.password.clone()),
            });
        }
        match error {
            GateError::Untrusted(leaf) => {
                self.selkies_form.cert = Some(CertPrompt {
                    server,
                    leaf,
                    previous: None,
                });
                self.selkies_form.focus = Some(Focus::Launcher);
            }
            GateError::PinMismatch { new, old } => {
                self.selkies_form.cert = Some(CertPrompt {
                    server,
                    leaf: new,
                    previous: Some(old),
                });
                self.selkies_form.focus = Some(Focus::Launcher);
            }
            error => {
                let username_typed = !input_text(&self.selkies_form.inputs.username, cx).is_empty()
                    || server
                        .auth()
                        .basic
                        .as_ref()
                        .is_some_and(|login| !login.username.is_empty());
                self.selkies_form.problem = problem_for(
                    &error,
                    &server.host_port(),
                    self.selkies_form.token.is_some(),
                );
                self.selkies_form.focus = focus_for(&error, username_typed);
            }
        }
    }

    /// Trust and Connect: the same server, pinned to the leaf the sheet showed.
    pub(super) fn trust_certificate(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.selkies_form.cert.take() else {
            return;
        };
        match prompt.server.with_pin(prompt.leaf) {
            Ok(server) => self.connect_selkies(server, LoadKind::Connect, cx),
            Err(error) => self.selkies_form.status = Some(error),
        }
        cx.notify();
    }

    pub(super) fn cancel_certificate(&mut self, cx: &mut Context<Self>) {
        self.selkies_form.cert = None;
        self.selkies_form.focus = Some(Focus::Address);
        cx.notify();
    }

    pub(super) fn refresh_selkies(&mut self, manual: bool, cx: &mut Context<Self>) {
        let Some(server) = self.selkies.as_ref().map(|home| home.server.clone()) else {
            return;
        };
        let kind = if manual {
            LoadKind::ManualRefresh
        } else {
            LoadKind::Refresh
        };
        self.connect_selkies(server, kind, cx);
    }

    fn fetch_selkies_thumbnail(&mut self, cx: &mut Context<Self>) {
        let Some(home) = self.selkies.as_mut() else {
            return;
        };
        home.thumbnail_generation += 1;
        home.thumbnail_loading = home.thumbnail.is_none();
        let generation = home.thumbnail_generation;
        let server = home.server.clone();
        let pending = self.rt.spawn(async move {
            direct::screenshot(&server)
                .await
                .ok()
                .and_then(|bytes| decode_image(&bytes, THUMBNAIL_MAX_EDGE, super::THUMBNAIL_LIMITS))
        });
        let window = self.window;
        let server_generation = self.server_generation;
        cx.spawn(async move |this, cx| {
            let image = pending.await.ok().flatten();
            let _ = window.update(cx, |_, _, cx| {
                this.update(cx, |this, cx| {
                    let current = this.server_generation == server_generation
                        && this
                            .selkies
                            .as_ref()
                            .is_some_and(|home| home.thumbnail_generation == generation);
                    if !current {
                        if let Some(image) = image {
                            this.retire(image);
                        }
                        return;
                    }
                    let mut old = None;
                    if let Some(home) = this.selkies.as_mut() {
                        home.thumbnail_loading = false;
                        // A failed fetch keeps what was there, or the placeholder.
                        if let Some(image) = image {
                            old = home.thumbnail.replace(image);
                        }
                    }
                    if let Some(old) = old {
                        this.retire(old);
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    /// Drops the direct server's Library and retires its thumbnail (part of
    /// `leave_server`).
    pub(super) fn drop_selkies_home(&mut self) {
        if let Some(home) = self.selkies.take() {
            if let Some(image) = home.thumbnail {
                self.retire(image);
            }
        }
    }

    pub(super) fn open_selkies_desktop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = self.selkies.as_ref().map(|home| home.server.clone()) else {
            return;
        };
        let session = server.as_session();
        self.open_session_window(SessionServer::Selkies(server), session, window, cx);
    }

    /// The desktop's Files window (no stream, no cookie exchange): brought forward if open,
    /// else opened. `uploads` go to its Desktop.
    pub(super) fn open_selkies_files(&mut self, uploads: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(server) = self.selkies.as_ref().map(|home| home.server.clone()) else {
            return;
        };
        let session_id = server.session_id();
        if raise_files(&session_id, uploads.clone(), cx) {
            return;
        }
        let source = FilesSource {
            title: server.display_name(),
            selkies: server.selkies_client(),
            // No stream, no `server_settings`: both directions until a 403.
            policy: crate::selkies::Policy::default(),
            // No SealSkin storage: no Delete (`ARCH-024`).
            storage: None,
        };
        let library = self.window;
        let rt = self.rt.clone();
        // Opened outside this update (ARCH-006).
        cx.defer(move |cx| {
            if raise_files(&session_id, uploads.clone(), cx) {
                return;
            }
            let opened = library.update(cx, |_, window, cx| {
                FilesWindow::open(
                    source,
                    rt,
                    window,
                    FilesOwner::Library,
                    session_id.clone(),
                    cx,
                )
            });
            if let Ok(Ok((_, files))) = opened {
                if !uploads.is_empty() {
                    files.update(cx, |files, cx| files.enqueue_upload(uploads, cx));
                }
            }
        });
    }

    /// Finder items dropped on the Desktop card (or the window): files are copied to the
    /// Desktop through Files.
    pub(super) fn drop_on_selkies(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let (files, folders) = split_drop(paths);
        let status = match (files.is_empty(), folders) {
            (_, 0) => None,
            (true, _) => Some("To copy a folder, open Files and drop it there.".to_string()),
            (false, n) => Some(folder_notice(n, false)),
        };
        if let Some(home) = self.selkies.as_mut() {
            home.status = status;
        }
        if !files.is_empty() {
            self.open_selkies_files(files, cx);
        }
        cx.notify();
    }

    /// `receive` while a direct server is connected.
    pub(super) fn receive_on_selkies(&mut self, items: Vec<Incoming>, cx: &mut Context<Self>) {
        if let Some(home) = self.selkies.as_mut() {
            home.receive(items);
        }
        cx.notify();
    }

    pub(super) fn copy_pending(&mut self, cx: &mut Context<Self>) {
        let files = self
            .selkies
            .as_mut()
            .map(|home| std::mem::take(&mut home.pending_copy))
            .unwrap_or_default();
        if !files.is_empty() {
            self.open_selkies_files(files, cx);
        }
        cx.notify();
    }

    /// Disconnect (the entry stays, startup no longer reconnects) or Forget (the entry and
    /// its secrets go). Open session windows keep streaming (`ARCH-022`).
    pub(super) fn disconnect_selkies(&mut self, forget: bool, cx: &mut Context<Self>) {
        let Some(home) = self.selkies.as_ref() else {
            return;
        };
        let server = home.server.clone();
        if server.source() != ServerSource::Environment {
            let queue = cx.global::<WriteQueue>();
            let done = if forget {
                queue.forget_server(server.base().as_str())
            } else {
                queue.clear_start_with()
            };
            self.on_written(
                done,
                |this, error| this.selkies_form.status = Some(error),
                cx,
            );
        }
        self.busy = None;
        self.leave_server();
        close_library_files(cx, |id| id.starts_with("selkies|"));
        self.connect_tab = ConnectTab::Selkies;
        self.reload_recent(cx);
        self.show_toast(format!("Disconnected from {}", server.host_port()), cx);
        cx.notify();
    }

    fn connect_props(&self) -> SelkiesConnectProps {
        let form = &self.selkies_form;
        SelkiesConnectProps {
            recent: form
                .recent
                .iter()
                .take(direct::MAX_SAVED)
                .map(recent_row)
                .collect(),
            token_chip: form.token.is_some(),
            remember: form.remember,
            problem: form.problem.clone(),
            status: form.status.clone(),
            busy: self
                .busy
                .as_ref()
                .and(form.checking.as_ref())
                .map(|host| format!("Checking {host}…")),
            pending_open: super::pending_open_banner(&self.pending_opens),
            certificate: form.cert.as_ref().map(|prompt| CertificateProps {
                host: prompt.server.host(),
                host_port: prompt.server.host_port(),
                fingerprint: tls::fingerprint_text(&prompt.leaf),
                previous: prompt.previous.as_ref().map(tls::fingerprint_text),
            }),
        }
    }

    fn connect_handlers(&self, cx: &mut Context<Self>) -> SelkiesConnectHandlers {
        let entity = cx.entity();
        let e = entity.clone();
        let on_tab: ui::TabHandler = Rc::new(move |tab, _, cx| {
            e.update(cx, |this, cx| this.set_connect_tab(tab, cx));
        });
        let e = entity.clone();
        let on_recent: ui::UrlHandler = Rc::new(move |url, _, cx| {
            e.update(cx, |this, cx| this.pick_recent(&url, cx));
        });
        let e = entity.clone();
        let on_recent_remove: ui::UrlHandler = Rc::new(move |url, _, cx| {
            e.update(cx, |this, cx| this.remove_recent(&url, cx));
        });
        let e = entity;
        let on_remember: ui::FlagHandler = Rc::new(move |on, _, cx| {
            e.update(cx, |this, cx| {
                this.selkies_form.remember = on;
                cx.notify();
            });
        });
        SelkiesConnectHandlers {
            on_tab,
            on_connect: Rc::new(cx.listener(|this, _, _, cx| this.submit_selkies(cx))),
            on_recent,
            on_recent_remove,
            on_remember,
            on_drop_token: Rc::new(cx.listener(|this, _, _, cx| {
                this.selkies_form.set_token(None, "");
                if matches!(
                    this.selkies_form.problem,
                    Some(SelkiesProblem::Field {
                        field: SelkiesField::Token,
                        ..
                    })
                ) {
                    this.selkies_form.problem = None;
                }
                cx.notify();
            })),
            on_trust: Rc::new(cx.listener(|this, _, _, cx| this.trust_certificate(cx))),
            on_cert_cancel: Rc::new(cx.listener(|this, _, _, cx| this.cancel_certificate(cx))),
            on_switch_sealskin: Rc::new(cx.listener(|this, _, _, cx| {
                this.selkies_form.problem = None;
                this.set_connect_tab(ConnectTab::SealSkin, cx);
            })),
        }
    }

    /// The Selkies tab's parts for `connect::render_with_tabs`.
    pub(super) fn connect_tabs(
        &self,
        cx: &mut Context<Self>,
    ) -> (SelkiesConnectProps, SelkiesConnectHandlers) {
        (self.connect_props(), self.connect_handlers(cx))
    }

    /// Applies queued input changes and focus (they need a Window).
    pub(super) fn apply_selkies_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.move_to_key.take() {
            self.key_input
                .update(cx, |state, cx| state.set_value(text, window, cx));
        }
        if let Some(fill) = self.selkies_form.fill.take() {
            let inputs = &self.selkies_form.inputs;
            for (input, value) in [
                (&inputs.address, fill.address),
                (&inputs.username, fill.username),
                (&inputs.password, fill.password),
            ] {
                if let Some(value) = value {
                    if input.read(cx).value() != value.as_str() {
                        input.update(cx, |state, cx| state.set_value(value, window, cx));
                    }
                }
            }
        }
        if let Some(focus) = self.selkies_form.focus.take() {
            let inputs = &self.selkies_form.inputs;
            let input = match focus {
                Focus::Address => Some(&inputs.address),
                Focus::Username => Some(&inputs.username),
                Focus::Password => Some(&inputs.password),
                Focus::Launcher => None,
            };
            match input {
                Some(input) => input.update(cx, |state, cx| state.focus(window, cx)),
                None => window.focus(&self.focus_handle, cx),
            }
        }
    }

    /// Escape or Return while the certificate prompt shows: Cancel, the safe default.
    pub(super) fn certificate_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.selkies_form.cert.is_none() || self.connect_tab != ConnectTab::Selkies {
            return false;
        }
        if matches!(event.keystroke.key.as_str(), "escape" | "enter") {
            self.cancel_certificate(cx);
            return true;
        }
        false
    }

    pub(super) fn render_selkies_home(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let _ = window;
        let Some(home) = self.selkies.as_ref() else {
            return gpui_kit::div();
        };
        let server = &home.server;
        let session_id = server.session_id();
        let open_here = cx
            .default_global::<OpenWindows>()
            .session_window(&session_id)
            .is_some();
        let props = SelkiesHomeProps {
            app_name: APP_NAME.to_string(),
            display_name: server.display_name(),
            base_url: server.web_url().to_string(),
            login_label: server.login_label(),
            security: Some(badge(server)),
            fingerprint: server.pin().as_ref().map(tls::fingerprint_text),
            open_here,
            others: other_pages(&home.report, open_here),
            view_only: view_only(&home.report, open_here),
            thumbnail: home.thumbnail.clone(),
            thumbnail_loading: home.thumbnail_loading,
            files_pending: false,
            accepts_drop: !home.pending_forget,
            busy: self.busy.clone(),
            status: home.status.clone(),
            menu_open: home.menu_open,
            pending_copy: pending_copy_label(&home.pending_copy, &server.host_port()),
            // A token-only server's page would only answer 401 in a browser.
            can_open_web: server.auth().basic.is_some() || server.auth().token.is_none(),
        };
        let pending_forget = home.pending_forget;
        let display_name = server.display_name();
        let e = cx.entity();
        let on_menu_open: ui::FlagHandler = Rc::new(move |open, _, cx| {
            e.update(cx, |this, cx| {
                if let Some(home) = this.selkies.as_mut() {
                    home.menu_open = open;
                }
                cx.notify();
            });
        });
        let e = cx.entity();
        let on_drop: ui::DropHandler = Rc::new(move |paths, _, cx| {
            e.update(cx, |this, cx| this.drop_on_selkies(paths, cx));
        });
        let window_drop = on_drop.clone();
        let handlers = SelkiesHomeHandlers {
            on_open: Rc::new(
                cx.listener(|this, _, window, cx| this.open_selkies_desktop(window, cx)),
            ),
            on_files: Rc::new(
                cx.listener(|this, _, _, cx| this.open_selkies_files(Vec::new(), cx)),
            ),
            on_refresh: Rc::new(cx.listener(|this, _, _, cx| this.refresh_selkies(true, cx))),
            on_menu_open,
            on_open_web: Rc::new(cx.listener(|this, _, _, cx| {
                if let Some(home) = this.selkies.as_mut() {
                    home.menu_open = false;
                    cx.open_url(home.server.web_url().as_str());
                }
                cx.notify();
            })),
            on_disconnect: Rc::new(
                cx.listener(|this, _, _, cx| this.disconnect_selkies(false, cx)),
            ),
            on_forget: Rc::new(cx.listener(|this, _, window, cx| {
                // The popover closes first, then the confirmation opens (Task A's order).
                if let Some(home) = this.selkies.as_mut() {
                    home.menu_open = false;
                    home.pending_forget = true;
                }
                window.focus(&this.focus_handle, cx);
                cx.notify();
            })),
            on_drop,
            on_copy_pending: Rc::new(cx.listener(|this, _, _, cx| this.copy_pending(cx))),
            on_cancel_pending: Rc::new(cx.listener(|this, _, _, cx| {
                if let Some(home) = this.selkies.as_mut() {
                    home.pending_copy.clear();
                }
                cx.notify();
            })),
        };
        gpui_kit::div()
            .size_full()
            .relative()
            .key_context(super::LIBRARY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let pending = this
                    .selkies
                    .as_ref()
                    .is_some_and(|home| home.pending_forget);
                match event.keystroke.key.as_str() {
                    "escape" if pending => {
                        if let Some(home) = this.selkies.as_mut() {
                            home.pending_forget = false;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }
                    // Return opens the desktop (⌘O stays Open…).
                    "enter" if !pending => {
                        this.open_selkies_desktop(window, cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .on_drop(move |paths: &gpui_kit::ExternalPaths, window, cx| {
                window_drop(paths.paths().to_vec(), window, cx)
            })
            .child(ui::render_home(theme, &props, handlers))
            .when(pending_forget, |view| {
                view.child(skerry_ui::session::forget_server_confirm_modal(
                    theme,
                    &display_name,
                    cx.listener(|this, _, _, cx| {
                        if let Some(home) = this.selkies.as_mut() {
                            home.pending_forget = false;
                        }
                        cx.notify();
                    }),
                    cx.listener(|this, _, _, cx| this.disconnect_selkies(true, cx)),
                ))
            })
            .when_some(self.toast.as_ref(), |view, toast| {
                view.child(skerry_ui::components::toast_layer(theme, toast))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direct::PageInfo;

    fn page(role: &str, at: f64) -> PageInfo {
        PageInfo {
            id: format!("{at}"),
            role: role.into(),
            connected_at: at,
        }
    }

    #[test]
    fn status_line_counts_other_pages_only() {
        let report = GateReport {
            dual_mode: false,
            pages: vec![page("controller", 1.0), page("viewer", 2.0)],
        };
        assert_eq!(other_pages(&report, true), 1);
        assert_eq!(other_pages(&report, false), 2);
        assert_eq!(other_pages(&GateReport::default(), true), 0);
        assert!(view_only(&report, true));
        assert!(!view_only(&report, false));
    }

    #[test]
    fn links_dropped_while_on_selkies_get_a_notice_not_a_launch() {
        let items = vec![
            Incoming::Target(OpenTarget::File("/tmp/a.pdf".into())),
            Incoming::Target(OpenTarget::Url("https://example.com/".parse().unwrap())),
            Incoming::Folder("/tmp/dir".into()),
            Incoming::Ignored,
        ];
        let plan = plan_receive(items);
        assert_eq!(plan.files, vec![PathBuf::from("/tmp/a.pdf")]);
        assert_eq!(plan.links, 1);
        assert_eq!(plan.folders, 1);
        assert_eq!(
            pending_copy_label(&plan.files, "h:8443").as_deref(),
            Some("Copy “a.pdf” to the Desktop of h:8443?")
        );
    }

    fn test_server() -> SelkiesServer {
        SelkiesServer::new(
            "https://h.example/".parse().unwrap(),
            SelkiesAuth::default(),
            None,
            ServerSource::Typed,
            false,
        )
        .unwrap()
    }

    #[test]
    fn opens_queued_before_a_selkies_connect_are_handled_after_it() {
        let mut queued = vec![
            Incoming::Target(OpenTarget::File("/tmp/a.pdf".into())),
            Incoming::Target(OpenTarget::Url("https://example.com/".parse().unwrap())),
            Incoming::Target(OpenTarget::File("/tmp/b.pdf".into())),
        ];
        let mut home = SelkiesHome::new(test_server(), GateReport::default());
        home.take_queued(&mut queued);
        assert!(queued.is_empty(), "the queue is drained, not left waiting");
        assert_eq!(
            home.pending_copy,
            vec![PathBuf::from("/tmp/a.pdf"), PathBuf::from("/tmp/b.pdf")]
        );
        assert_eq!(home.status.as_deref(), Some(LINKS_NOTICE));
        assert_eq!(
            pending_copy_label(&home.pending_copy, "h").as_deref(),
            Some("Copy 2 files to the Desktop of h?")
        );
        // Later items join the same chooser.
        home.receive(vec![Incoming::Target(OpenTarget::File(
            "/tmp/c.pdf".into(),
        ))]);
        assert_eq!(home.pending_copy.len(), 3);
        assert_eq!(home.status, None);
    }

    #[test]
    fn connecting_to_selkies_clears_sealskin_recovery_and_pending_save() {
        let credential = crate::sealskin::Credential::parse("ssk_id_secret@seal.example").unwrap();
        let mut key_problem = Some(skerry_ui::connect::KeyProblem {
            kind: skerry_ui::connect::ProblemKind::Revoked,
            env_note: None,
        });
        let mut refused_server = Some("seal.example".to_string());
        let mut key_hint = Some("Paste a new application key.".to_string());
        let mut different_key = true;
        let mut pending_save = Some(credential.clone());
        let mut retry_api = Some(crate::sealskin::SealSkinClient::new(&credential).unwrap());
        let mut status = Some("old".to_string());
        SealSkinLeftovers {
            key_problem: &mut key_problem,
            refused_server: &mut refused_server,
            key_hint: &mut key_hint,
            different_key: &mut different_key,
            pending_save: &mut pending_save,
            retry_api: &mut retry_api,
            status: &mut status,
        }
        .clear();
        assert!(key_problem.is_none() && refused_server.is_none() && key_hint.is_none());
        assert!(!different_key);
        assert!(
            pending_save.is_none(),
            "a pending save never lands after the switch"
        );
        assert!(retry_api.is_none() && status.is_none());
    }

    #[test]
    fn a_link_token_is_dropped_when_the_address_names_another_server() {
        assert!(token_belongs(
            Some("https://a.example/"),
            "https://a.example/"
        ));
        assert!(!token_belongs(
            Some("https://a.example/"),
            "https://b.example/"
        ));
        assert!(!token_belongs(
            Some("https://a.example/"),
            "https://a.example/other/"
        ));
        assert!(token_belongs(None, "https://b.example/"));
        let a = direct::parse_address("https://a.example/#token=T").unwrap();
        let mut token = a.token.clone();
        let mut token_base = Some(a.base.to_string());
        let mut problem = Some(SelkiesProblem::Field {
            field: SelkiesField::Token,
            text: "bad".into(),
        });
        // Same server: kept.
        retarget_token(&mut token, &mut token_base, &mut problem, a.base.as_str());
        assert_eq!(token.as_deref(), Some("T"));
        let b = direct::parse_address("https://b.example/").unwrap();
        retarget_token(&mut token, &mut token_base, &mut problem, b.base.as_str());
        assert_eq!(token, None, "T is never sent to b.example");
        assert_eq!(problem, None);
        assert_eq!(token_base, None);
        // Nor does it ride b's requests: the auth built for b has no token.
        let auth = SelkiesAuth::new("ubuntu", "pw", token);
        assert!(auth.http_headers().get("cookie").is_none());
    }

    #[test]
    fn login_errors_go_under_their_field_and_refusals_are_alerts() {
        let host = "h:8443";
        assert!(matches!(
            problem_for(&GateError::BadLogin, host, false),
            Some(SelkiesProblem::Field {
                field: SelkiesField::Login,
                ..
            })
        ));
        assert!(matches!(
            problem_for(&GateError::BadToken, host, true),
            Some(SelkiesProblem::Field {
                field: SelkiesField::Token,
                ..
            })
        ));
        assert!(matches!(
            problem_for(&GateError::NeedsToken, host, false),
            Some(SelkiesProblem::Field {
                field: SelkiesField::Address,
                ..
            })
        ));
        match problem_for(&GateError::WebRtc { dual_mode: true }, host, false) {
            Some(SelkiesProblem::Alert { tone, body, .. }) => {
                assert_eq!(tone, AlertTone::Warning);
                assert!(body.contains("Dual mode"));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            problem_for(&GateError::SealSkinAddress, host, false),
            Some(SelkiesProblem::Alert {
                switch_to_sealskin: true,
                ..
            })
        ));
        assert_eq!(
            problem_for(&GateError::Untrusted([0; 32]), host, false),
            None
        );
        assert_eq!(
            focus_for(&GateError::NeedsLogin, false),
            Some(Focus::Username)
        );
        assert_eq!(
            focus_for(&GateError::NeedsLogin, true),
            Some(Focus::Password)
        );
        assert_eq!(focus_for(&GateError::BadLogin, true), Some(Focus::Password));
    }

    #[test]
    fn recent_row_without_password_focuses_password() {
        let saved = SavedServer {
            url: "https://h.example/".into(),
            username: Some("ubuntu".into()),
            ..SavedServer::default()
        };
        assert!(!saved.ready(), "a row without its password waits for it");
        let open = SavedServer {
            url: "http://localhost:8080/".into(),
            ..SavedServer::default()
        };
        assert!(open.ready(), "a server with no login connects at once");
        let row = recent_row(&open);
        assert!(row.loopback && !row.pinned);
        assert_eq!(row.login, "No login");
    }

    #[test]
    fn selkies_card_shows_the_token_chip_only_with_a_token() {
        // The chip follows the form's token, which only a link (or a saved entry) sets.
        let mut form_token: Option<String> = None;
        let address = direct::parse_address("https://h.example/").unwrap();
        if address.token.is_some() {
            form_token = address.token;
        }
        assert!(form_token.is_none());
        let address = direct::parse_address("https://h.example/#token=abc").unwrap();
        if address.token.is_some() {
            form_token = address.token;
        }
        assert_eq!(form_token.as_deref(), Some("abc"));
    }

    #[test]
    fn a_gate_result_from_before_disconnect_is_dropped() {
        let gate = LoadGate(4);
        assert!(gate.admits(4));
        // Disconnect calls `leave_server`, which bumps the generation.
        assert!(!gate.admits(5));
    }
}
