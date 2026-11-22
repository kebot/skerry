//! The Storage window (`ARCH-037`): one per SealSkin server, a sidebar of Shared Files,
//! homes and app homes with the quota at its foot, and the session Files window's own list
//! pane on the right, working on persistent storage through `FilesBackend::Storage`.
//!
//! Homes are created here (New Home…) and deleted (Delete Home…) only after re-reading the
//! running sessions twice: before the confirmation, and again once Delete Home is clicked,
//! since the server checks nothing. Shared Files is never deleted. Open in… hands a
//! top-level Shared Files file to the Library's chooser (`ARCH-028`). The window closes on
//! Sign Out, recovery and a server change, and is never opened for a direct Selkies server.

use super::files_backend::{FilesBackend, StorageBackend};
use super::files_window::{self, FilesOwner, FilesWindow, PaneEvent};
use crate::sealskin::{self, HomeError, RemoteApp, SealSkinClient};
use crate::storage::{self, InUse, NameCheck, QuotaLevel, StoragePlace};
use gpui_kit::component::{
    input::{InputEvent, InputState},
    menu::PopupMenu,
    ActiveTheme as _, StyledExt as _,
};
use gpui_kit::{
    actions, div, point, prelude::*, px, size, AnyWindowHandle, App, Bounds, Context, Entity,
    FocusHandle, Global, SharedString, Subscription, Task, TitlebarOptions, WeakEntity, Window,
    WindowBounds, WindowOptions,
};
use skerry_ui::components::{self, Toast, ToastKind, TOAST_DURATION, TOAST_EXIT};
use skerry_ui::storage::{
    self as ui, HomesState, NewHomeProps, QuotaState, QuotaTone, SidebarProps, SidebarRow,
    StorageAction,
};

use std::rc::Rc;
use std::time::{Duration, Instant};
use tokio::runtime::Handle;

actions!(
    storage_window,
    [
        /// File → New Home….
        NewHome,
        /// File → Delete Home…, on the selected home.
        DeleteHome,
        /// Go → Shared Files.
        GoShared,
        /// Escape: the New Home sheet goes.
        CancelSheet,
        CloseStorage
    ]
);

/// The Storage window's key context.
pub(super) const STORAGE_CONTEXT: &str = "Storage";

/// Keys of the Storage window, all scoped to it. ⌘W from the sidebar closes it too (the
/// pane binds its own).
pub(super) fn key_bindings() -> Vec<gpui_kit::KeyBinding> {
    use gpui_kit::KeyBinding;
    let window = Some(STORAGE_CONTEXT);
    vec![
        KeyBinding::new("escape", CancelSheet, Some("StorageSheet")),
        KeyBinding::new("cmd-w", CloseStorage, window),
    ]
}

/// The open Storage window and the server it shows; one per app (one server at a time).
#[derive(Default)]
pub(super) struct OpenStorage(Option<(AnyWindowHandle, WeakEntity<StorageWindow>, String)>);

impl Global for OpenStorage {}

/// Re-measures at most this often when the window becomes key (a measure can walk the whole
/// storage).
const ACTIVATION_MEASURE: Duration = Duration::from_secs(60);
/// A batch of transfers or a delete ends: measure once things settle.
const CHANGE_DEBOUNCE: Duration = Duration::from_secs(2);

enum Homes {
    Loading,
    Failed,
    Ready(Vec<String>),
}

enum Measure {
    Measuring,
    Failed,
    Ready(storage::Quota),
}

struct Sheet {
    input: Entity<InputState>,
    creating: bool,
    /// The server's answer, until the name changes.
    error: Option<String>,
}

pub(super) struct StorageWindow {
    client: SealSkinClient,
    rt: Handle,
    place: StoragePlace,
    homes: Homes,
    pane: Entity<FilesWindow>,
    measure: Measure,
    measured_at: Option<Instant>,
    measuring: Option<Task<()>>,
    change_debounce: Option<Task<()>>,
    sheet: Option<Sheet>,
    /// The home being checked or deleted.
    deleting: Option<String>,
    /// SealSkin refused the key: no further requests until reopened (`ARCH-029`).
    stopped: bool,
    toast: Option<Toast>,
    toast_task: Option<Task<()>>,
    toast_id: u64,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

/// Opens the Storage window for `client`'s server, or brings it forward if open. A window
/// for another server is closed first. Call outside any entity update (`ARCH-006`).
pub(super) fn open(
    client: SealSkinClient,
    rt: Handle,
    near: Option<Bounds<gpui_kit::Pixels>>,
    cx: &mut App,
) {
    let server = client.server().to_string();
    if let Some((window, entity, open_server)) = cx.default_global::<OpenStorage>().0.clone() {
        if entity.upgrade().is_some() {
            if open_server == server
                && window
                    .update(cx, |_, window, _| window.activate_window())
                    .is_ok()
            {
                cx.activate(true);
                return;
            }
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
        cx.default_global::<OpenStorage>().0 = None;
    }
    let bounds = match near {
        Some(near) => Bounds::new(
            point(near.left() + px(60.), near.top() + px(40.)),
            size(px(900.), px(580.)),
        ),
        None => Bounds::centered(None, size(px(900.), px(580.)), cx),
    };
    let opened = gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(620.), px(380.))),
            titlebar: Some(TitlebarOptions {
                title: Some("Storage".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        cx,
        move |window, cx| cx.new(|cx| StorageWindow::new(client, rt, window, cx)),
    );
    if let Ok((window, entity)) = opened {
        cx.default_global::<OpenStorage>().0 = Some((window, entity.downgrade(), server));
        cx.activate(true);
    }
}

/// Closes the Storage window, if open (Sign Out, recovery, a server change).
pub(super) fn close(cx: &mut App) {
    cx.defer(|cx| {
        if let Some((window, _, _)) = cx.default_global::<OpenStorage>().0.take() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
    });
}

/// Closes the Storage window if it shows another server than `server` (a new connect).
pub(super) fn close_other(server: String, cx: &mut App) {
    cx.defer(move |cx| {
        let other = cx
            .default_global::<OpenStorage>()
            .0
            .as_ref()
            .is_some_and(|(_, _, open)| *open != server);
        if other {
            if let Some((window, _, _)) = cx.default_global::<OpenStorage>().0.take() {
                let _ = window.update(cx, |_, window, _| window.remove_window());
            }
        }
    });
}

impl StorageWindow {
    fn new(
        client: SealSkinClient,
        rt: Handle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let place = StoragePlace::Shared;
        let backend = FilesBackend::Storage(StorageBackend {
            client: client.clone(),
            place: place.clone(),
        });
        let pane = FilesWindow::pane(backend, rt.clone(), FilesOwner::Storage, window, cx);
        let mut subscriptions = vec![cx.subscribe_in(&pane, window, Self::on_pane_event)];
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.activated(cx);
            }
            cx.notify();
        }));
        window.on_window_should_close(cx, |window, cx| {
            let closing = window.window_handle();
            let open = cx.default_global::<OpenStorage>();
            if open
                .0
                .as_ref()
                .is_some_and(|(handle, _, _)| *handle == closing)
            {
                open.0 = None;
            }
            true
        });
        let focus = pane.read(cx).list_focus();
        window.focus(&focus, cx);
        let mut this = Self {
            client,
            rt,
            place,
            homes: Homes::Loading,
            pane,
            measure: Measure::Measuring,
            measured_at: None,
            measuring: None,
            change_debounce: None,
            sheet: None,
            deleting: None,
            stopped: false,
            toast: None,
            toast_task: None,
            toast_id: 0,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.load_homes(cx);
        this.measure(cx);
        this
    }

    // ---- Homes ---------------------------------------------------------------------------

    fn load_homes(&mut self, cx: &mut Context<Self>) {
        if self.stopped {
            return;
        }
        let client = self.client.clone();
        let pending = self
            .rt
            .spawn(async move { client.home_directories().await });
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(homes)) => {
                        // The selected home went away elsewhere: back to Shared Files.
                        if let StoragePlace::Home(home) = &this.place {
                            if !homes.contains(home) {
                                this.select(StoragePlace::Shared, cx);
                            }
                        }
                        super::set_library_homes(homes.clone(), cx);
                        this.homes = Homes::Ready(homes);
                    }
                    Ok(Err(error)) => {
                        this.refused(&error, cx);
                        this.homes = Homes::Failed;
                    }
                    Err(_) => this.homes = Homes::Failed,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn home_names(&self) -> Vec<String> {
        match &self.homes {
            Homes::Ready(homes) => homes.clone(),
            _ => Vec::new(),
        }
    }

    fn select(&mut self, place: StoragePlace, cx: &mut Context<Self>) {
        if self.place == place {
            return;
        }
        self.place = place.clone();
        let backend = FilesBackend::Storage(StorageBackend {
            client: self.client.clone(),
            place,
        });
        self.pane
            .update(cx, |pane, cx| pane.set_backend(backend, cx));
        cx.notify();
    }

    fn start_new_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sheet.is_some() || matches!(self.homes, Homes::Loading) || self.stopped {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("home name"));
        self._subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.create_home(window, cx),
                InputEvent::Change => {
                    if let Some(sheet) = &mut this.sheet {
                        sheet.error = None;
                    }
                    cx.notify();
                }
                _ => {}
            },
        ));
        input.update(cx, |state, cx| state.focus(window, cx));
        self.sheet = Some(Sheet {
            input,
            creating: false,
            error: None,
        });
        cx.notify();
    }

    fn sheet_check(&self, cx: &App) -> Option<NameCheck> {
        let sheet = self.sheet.as_ref()?;
        let name = sheet.input.read(cx).value().trim().to_string();
        Some(storage::check_home_name(&name, &self.home_names()))
    }

    fn create_home(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.sheet_check(cx) != Some(NameCheck::Ok) {
            return;
        }
        let Some(sheet) = self.sheet.as_mut().filter(|sheet| !sheet.creating) else {
            return;
        };
        let name = sheet.input.read(cx).value().trim().to_string();
        sheet.creating = true;
        let client = self.client.clone();
        let wanted = name.clone();
        let pending = self
            .rt
            .spawn(async move { client.create_home(&wanted).await });
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            let _ = this.update(cx, |this, cx| {
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(HomeError::Exists)) => {
                        Some(format!("A home named “{name}” already exists."))
                    }
                    Ok(Err(HomeError::Api(error))) => {
                        this.refused(&error, cx);
                        Some(error.to_string())
                    }
                    Ok(Err(error)) => Some(error.to_string()),
                    Err(_) => Some("Creating the home was interrupted.".into()),
                };
                match error {
                    None => {
                        this.sheet = None;
                        if let Homes::Ready(homes) = &mut this.homes {
                            homes.push(name.clone());
                        }
                        this.select(StoragePlace::Home(name.clone()), cx);
                        this.show_toast(format!("Created “{name}”"), cx);
                        this.load_homes(cx);
                    }
                    Some(error) => {
                        if let Some(sheet) = &mut this.sheet {
                            sheet.creating = false;
                            sheet.error = Some(error);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Delete Home…: sessions are read before the confirmation and again after it, since
    /// the server deletes a home under a running session without a word (`ARCH-037`).
    fn confirm_delete_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let StoragePlace::Home(home) = self.place.clone() else {
            return;
        };
        if sealskin::undeletable_home(&home) || self.deleting.is_some() || self.stopped {
            return;
        }
        if self.pane.read(cx).busy_homes().contains(&home) {
            drop(window.prompt(
                gpui_kit::PromptLevel::Warning,
                &format!("“{home}” has transfers in progress."),
                Some("Wait for them to finish or cancel them, then delete the home."),
                &[gpui_kit::PromptButton::cancel("OK")],
                cx,
            ));
            return;
        }
        self.deleting = Some(home.clone());
        cx.notify();
        let client = self.client.clone();
        let rt = self.rt.clone();
        cx.spawn(async move |this, cx| {
            // 1. Who uses it now, to choose the alert.
            let used = in_use(&client, &rt, &home, cx).await;
            let confirmed = this.update_in(cx, |this, window, cx| {
                match used {
                    None => {
                        this.deleting = None;
                        drop(window.prompt(
                            gpui_kit::PromptLevel::Warning,
                            &format!("Couldn’t check whether “{home}” is in use."),
                            Some("Try again in a moment."),
                            &[gpui_kit::PromptButton::cancel("OK")],
                            cx,
                        ));
                        None
                    }
                    Some(used) if used.any() => {
                        this.deleting = None;
                        show_in_use(&home, &used, window, cx);
                        None
                    }
                    Some(_) => Some(window.prompt(
                        gpui_kit::PromptLevel::Critical,
                        &format!("Delete the home “{home}”?"),
                        Some("Everything in it is deleted permanently, including app settings and the files on its Desktop. You can’t undo this action."),
                        &[
                            gpui_kit::PromptButton::new("Delete Home"),
                            gpui_kit::PromptButton::cancel("Cancel"),
                        ],
                        cx,
                    )),
                }
            });
            let Ok(Some(answer)) = confirmed else {
                let _ = this.update(cx, |_, cx| cx.notify());
                return;
            };
            if answer.await != Ok(0) {
                let _ = this.update(cx, |this, cx| {
                    this.deleting = None;
                    cx.notify();
                });
                return;
            }
            // 2. Again, now: the confirmation may have stayed up for minutes.
            let used = in_use(&client, &rt, &home, cx).await;
            match used {
                Some(used) if !used.any() => {}
                other => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.deleting = None;
                        match other {
                            Some(used) => show_in_use(&home, &used, window, cx),
                            None => {
                                drop(window.prompt(
                                    gpui_kit::PromptLevel::Warning,
                                    &format!("Couldn’t check whether “{home}” is in use."),
                                    Some("Try again in a moment."),
                                    &[gpui_kit::PromptButton::cancel("OK")],
                                    cx,
                                ));
                            }
                        }
                        cx.notify();
                    });
                    return;
                }
            }
            let target = home.clone();
            let result = rt
                .spawn(async move { client.delete_home(&target).await })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.deleting = None;
                match result {
                    Ok(Ok(())) => {
                        this.select(StoragePlace::Shared, cx);
                        this.show_toast(format!("Deleted “{home}”"), cx);
                        this.schedule_measure(cx);
                    }
                    Ok(Err(HomeError::Gone)) => {
                        this.select(StoragePlace::Shared, cx);
                        this.say("That home was already deleted.", false, cx);
                    }
                    Ok(Err(HomeError::Api(error))) => {
                        this.refused(&error, cx);
                        this.say(error.to_string(), true, cx);
                    }
                    Ok(Err(error)) => this.say(error.to_string(), true, cx),
                    Err(_) => this.say("Deleting the home was interrupted.", true, cx),
                }
                this.load_homes(cx);
                cx.notify();
            });
        })
        .detach();
    }

    // ---- Quota ---------------------------------------------------------------------------

    fn measure(&mut self, cx: &mut Context<Self>) {
        if self.stopped || self.measuring.is_some() {
            return;
        }
        let client = self.client.clone();
        let pending = self.rt.spawn(async move { client.account(true).await });
        self.measuring = Some(cx.spawn(async move |this, cx| {
            let result = pending.await;
            let _ = this.update(cx, |this, cx| {
                this.measuring = None;
                this.measured_at = Some(Instant::now());
                this.measure = match result {
                    Ok(Ok(status)) => match status.storage_used {
                        Some(used) => Measure::Ready(storage::quota(
                            used,
                            status.settings.storage_limit,
                            status.is_admin,
                        )),
                        None => Measure::Failed,
                    },
                    Ok(Err(error)) => {
                        this.refused(&error, cx);
                        Measure::Failed
                    }
                    Err(_) => Measure::Failed,
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Measures once a burst of changes has settled.
    fn schedule_measure(&mut self, cx: &mut Context<Self>) {
        self.change_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CHANGE_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                this.change_debounce = None;
                this.measure(cx);
            });
        }));
    }

    fn activated(&mut self, cx: &mut Context<Self>) {
        if self.stopped {
            return;
        }
        // A session may have written meanwhile.
        self.pane.update(cx, |pane, cx| pane.refresh(cx));
        if self
            .measured_at
            .is_none_or(|at| at.elapsed() >= ACTIVATION_MEASURE)
        {
            self.measure(cx);
        }
    }

    // ---- Pane ----------------------------------------------------------------------------

    fn on_pane_event(
        &mut self,
        _: &Entity<FilesWindow>,
        event: &PaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaneEvent::GoShared => {
                self.select(StoragePlace::Shared, cx);
                self.say(
                    "This folder is your Shared Files. Every session with storage sees it here.",
                    false,
                    cx,
                );
            }
            PaneEvent::UploadToShared(paths) => {
                self.select(StoragePlace::Shared, cx);
                let paths = paths.clone();
                self.pane.update(cx, |pane, cx| {
                    pane.enqueue_upload(paths, cx);
                    pane.notify(
                        "This folder is your Shared Files. Every session with storage sees it here.",
                        false,
                        cx,
                    );
                });
            }
            PaneEvent::OpenIn(entry) => {
                let filename = entry.name.clone();
                let size = entry.size.unwrap_or(0);
                cx.defer(move |cx| super::open_stored(filename, size, cx));
            }
            PaneEvent::Changed => self.schedule_measure(cx),
            PaneEvent::Refreshed => {
                self.load_homes(cx);
                self.measure(cx);
            }
        }
        let _ = window;
        cx.notify();
    }

    fn say(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        let text = text.into();
        self.pane
            .update(cx, |pane, cx| pane.notify(text, error, cx));
    }

    /// A refused key or account stops the window's requests and says why in the pane.
    fn refused(&mut self, error: &sealskin::ApiError, cx: &mut Context<Self>) {
        if error.is_refusal() && !self.stopped {
            self.stopped = true;
            self.say(error.to_string(), true, cx);
        }
    }

    // ---- Toasts (successes only, `ARCH-026`) ----------------------------------------------

    fn show_toast(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast_id += 1;
        let id = self.toast_id;
        self.toast = Some(Toast {
            id,
            message: message.into(),
            kind: ToastKind::Success,
            leaving: false,
        });
        self.toast_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_DURATION).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(toast) = this.toast.as_mut().filter(|toast| toast.id == id) {
                    toast.leaving = true;
                    cx.notify();
                }
            });
            cx.background_executor().timer(TOAST_EXIT).await;
            let _ = this.update(cx, |this, cx| {
                if this.toast.as_ref().is_some_and(|toast| toast.id == id) {
                    this.toast = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn handle(&mut self, action: StorageAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            StorageAction::Select(SidebarRow::Shared) => self.select(StoragePlace::Shared, cx),
            StorageAction::Select(SidebarRow::Home(home)) => {
                self.select(StoragePlace::Home(home), cx)
            }
            StorageAction::NewHome => self.start_new_home(window, cx),
            StorageAction::RetryHomes => {
                self.homes = Homes::Loading;
                self.load_homes(cx);
            }
            StorageAction::Measure => self.measure(cx),
            StorageAction::CreateHome => self.create_home(window, cx),
            StorageAction::CancelSheet => self.cancel_sheet(window, cx),
        }
        cx.notify();
    }

    fn cancel_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sheet.as_ref().is_some_and(|sheet| !sheet.creating) {
            self.sheet = None;
            let focus = self.pane.read(cx).list_focus();
            window.focus(&focus, cx);
        }
    }

    fn sidebar_props(&self, window: &Window, cx: &App) -> SidebarProps {
        let homes = match &self.homes {
            Homes::Loading => HomesState::Loading,
            Homes::Failed => HomesState::Failed,
            Homes::Ready(homes) => {
                let sorted = storage::sidebar_homes(homes);
                HomesState::Ready {
                    named: sorted.named,
                    app: sorted.app,
                }
            }
        };
        let quota = match &self.measure {
            Measure::Measuring => QuotaState::Measuring,
            Measure::Failed => QuotaState::Failed,
            Measure::Ready(quota) => QuotaState::Ready {
                text: quota.text.clone(),
                fraction: quota.fraction,
                tone: match quota.level {
                    QuotaLevel::Normal => QuotaTone::Normal,
                    QuotaLevel::Warning => QuotaTone::Warning,
                    QuotaLevel::Full => QuotaTone::Full,
                },
            },
        };
        let quota_tooltip = match (&self.measure, self.measured_at) {
            (Measure::Failed, _) => "Click to try again".to_string(),
            (_, Some(at)) if at.elapsed() >= Duration::from_secs(60) => {
                format!("Measured {} min ago", at.elapsed().as_secs() / 60)
            }
            (_, Some(_)) => "Measured just now".into(),
            (_, None) => "Measuring…".into(),
        };
        let _ = cx;
        SidebarProps {
            selected: match &self.place {
                StoragePlace::Shared => SidebarRow::Shared,
                StoragePlace::Home(home) => SidebarRow::Home(home.clone()),
            },
            homes,
            deleting: self.deleting.clone(),
            focused: window.is_window_active(),
            quota,
            quota_tooltip,
        }
    }
}

/// The running sessions and Library launches using `home`, read fresh from the server;
/// `None` when the sessions can't be read.
async fn in_use(
    client: &SealSkinClient,
    rt: &Handle,
    home: &str,
    cx: &mut gpui_kit::AsyncApp,
) -> Option<InUse> {
    let reader = client.clone();
    let sessions = rt
        .spawn(async move { reader.sessions().await })
        .await
        .ok()?
        .ok()?;
    let (apps, launching): (Vec<RemoteApp>, Vec<(String, String)>) =
        cx.update(super::library_launch_snapshot);
    Some(storage::sessions_using(home, &sessions, &apps, &launching))
}

fn show_in_use(home: &str, used: &InUse, window: &mut Window, cx: &mut App) {
    drop(window.prompt(
        gpui_kit::PromptLevel::Warning,
        &storage::in_use_title(home, used),
        Some("Stop the session first, then delete the home."),
        &[gpui_kit::PromptButton::cancel("OK")],
        cx,
    ));
}

impl Render for StorageWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let entity = cx.entity();
        let on_action: ui::StorageHandler = Rc::new(move |action, window, cx| {
            entity.update(cx, |this, cx| this.handle(action, window, cx));
        });
        let focus = self.focus.clone();
        let row_menu: ui::RowMenu = Rc::new(move |menu: PopupMenu, row: &SidebarRow| {
            let menu = menu.action_context(focus.clone());
            match row {
                SidebarRow::Shared => menu.menu("Open", Box::new(GoShared)),
                SidebarRow::Home(_) => menu
                    .menu("New Home…", Box::new(NewHome))
                    .separator()
                    .menu("Delete Home…", Box::new(DeleteHome)),
            }
        });
        let sidebar = self.sidebar_props(window, cx);
        let sheet_props = self.sheet.as_ref().map(|sheet| {
            let check = self.sheet_check(cx).unwrap_or(NameCheck::Empty);
            NewHomeProps {
                error: sheet.error.clone().or(match &check {
                    NameCheck::Problem(problem) => Some(problem.clone()),
                    _ => None,
                }),
                valid: check == NameCheck::Ok,
                creating: sheet.creating,
            }
        });
        div()
            .size_full()
            .relative()
            .h_flex()
            .bg(theme.background)
            .text_color(theme.foreground)
            .key_context(STORAGE_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &NewHome, window, cx| this.start_new_home(window, cx)))
            .on_action(
                cx.listener(|this, _: &DeleteHome, window, cx| {
                    this.confirm_delete_home(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &GoShared, _, cx| this.select(StoragePlace::Shared, cx)),
            )
            .on_action(cx.listener(|this, _: &CancelSheet, window, cx| {
                this.cancel_sheet(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &CloseStorage, window, _| window.remove_window()))
            .on_action(
                cx.listener(|_, _: &files_window::CloseFiles, window, _| window.remove_window()),
            )
            .child(ui::render_sidebar(
                &theme,
                &sidebar,
                Rc::clone(&on_action),
                row_menu,
            ))
            .child(div().flex_1().min_w_0().h_full().child(self.pane.clone()))
            .when_some(
                self.sheet.as_ref().zip(sheet_props),
                |view, (sheet, props)| {
                    view.child(
                        div()
                            .absolute()
                            .inset_0()
                            .key_context("StorageSheet")
                            .child(ui::render_new_home(
                                &theme,
                                &props,
                                &sheet.input,
                                Rc::clone(&on_action),
                            )),
                    )
                },
            )
            .when_some(self.toast.as_ref(), |view, toast| {
                view.child(components::toast_layer(&theme, toast))
            })
    }
}
