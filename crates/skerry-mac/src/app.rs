mod controls;
mod display;
mod files_model;
mod files_window;
mod promise;
mod session_window;

use crate::launch_options::{self, LaunchContext, LaunchMemory, LaunchSelection};
use crate::launch_progress::{self, Flight, Outcome, RowState};
use crate::persist::{self, WriteQueue};
use crate::sealskin::{
    Credential, KeySource, LaunchError, LaunchParams, Library, LibraryError, RemoteApp,
    RemoteSession, SealSkinClient,
};
use crate::settings::Preferences;
use crate::transport::{DisplayRequest, Resolution};
use gpui_kit::component::{
    input::{InputEvent, InputState},
    select::SelectEvent,
    ActiveTheme as _, Theme,
};
use gpui_kit::{
    actions, div, point, prelude::*, px, size, AnyWindowHandle, App, Bounds, ClipboardItem,
    Context, Entity, FocusHandle, KeyBinding, KeyDownEvent, Menu, MenuItem, Pixels, RenderImage,
    SharedString, Size, Task, TitlebarOptions, Window, WindowAppearance, WindowBounds,
    WindowOptions,
};
use objc2_app_kit::{NSView, NSWindow, NSWindowButton, NSWindowStyleMask, NSWindowTitleVisibility};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSUserDefaults};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use session_window::{OpenSession, OpenWindows, SessionWindow};
use skerry_ui::{
    components::{self, Toast, ToastKind, TOAST_DURATION, TOAST_EXIT},
    connect::{self, ConnectProps},
    launch_dialog::{
        self, LaunchDialogAction, LaunchDialogProps, LaunchDialogSelects, LaunchOption,
        LaunchOptions, LaunchSelect, RunningMatch,
    },
    library::{
        self, AppItem, Filter, FlightAction, LaunchNotice, LaunchingItem, LibraryHandlers,
        LibraryProps, SessionItem, StageRow, StageState,
    },
    session::{self, KeySourceLabel},
};
use smallvec::smallvec;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::runtime::Handle;

const APP_NAME: &str = "SealSkin Native";

pub fn run() {
    // GPUI 0.2.2 can reenter its window lock when AppKit restores key-window state.
    // The launcher restores remote sessions itself, so skip local window restoration.
    let key = NSString::from_str("ApplePersistenceIgnoreState");
    let enabled = NSNumber::numberWithBool(true);
    let defaults =
        NSDictionary::<NSString, objc2::runtime::AnyObject>::from_slices(&[&*key], &[&enabled]);
    // The registered value is a property-list boolean and lasts only for this run.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&defaults) };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("create SealSkin runtime");
    let handle = runtime.handle().clone();
    let reopen_handle = handle.clone();
    let application = gpui_kit::application().with_assets(skerry_ui::assets::SkerryAssets);
    // A Dock-icon click brings the Library back (ARCH-022).
    application.on_reopen(move |cx| show_library(&reopen_handle, cx));
    application.run(move |cx: &mut App| {
        gpui_kit::init(cx);
        cx.set_global(OpenWindows::default());
        // Every preference and key write goes through this one queue (`persist`).
        cx.set_global(persist::start(&handle));
        // GPUI gives quit futures 200 ms; wait for queued writes (at most 1 s) before
        // returning one.
        cx.on_app_quit(|cx| {
            cx.global::<WriteQueue>()
                .flush_blocking(Duration::from_secs(1));
            async {}
        })
        .detach();
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-0", ShowLibrary, None),
            KeyBinding::new("cmd-shift-m", ToggleSessionControls, None),
            KeyBinding::new("escape", CloseSessionControls, Some("SessionControls")),
            KeyBinding::new("cmd-w", CloseSessionControls, Some("SessionControls")),
            KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
            KeyBinding::new("cmd-shift-f", ToggleFiles, None),
        ]);
        cx.bind_keys(files_window::key_bindings());
        // Quitting disconnects every session window; no remote session is stopped.
        cx.on_action(|_: &Quit, cx| cx.quit());
        let library_handle = handle.clone();
        cx.on_action(move |_: &ShowLibrary, cx| show_library(&library_handle, cx));
        cx.set_menus(vec![
            Menu {
                name: APP_NAME.into(),
                items: vec![MenuItem::action("Quit SealSkin Native", Quit)],
                disabled: false,
            },
            // The Files window's commands (`ARCH-023`); AppKit disables them wherever no
            // Files window has focus.
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("New Folder", files_window::NewFolder),
                    MenuItem::action("Upload…", files_window::UploadFiles),
                    MenuItem::action("Download", files_window::DownloadSelection),
                    MenuItem::action("Download To…", files_window::DownloadSelectionTo),
                    MenuItem::separator(),
                    MenuItem::action("Open", files_window::OpenSelection),
                    MenuItem::action("Quick Look", files_window::QuickLook),
                    MenuItem::separator(),
                    MenuItem::action("Delete…", files_window::DeleteSelection),
                    MenuItem::separator(),
                    MenuItem::action("Close Window", files_window::CloseFiles),
                ],
                disabled: false,
            },
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::action("Copy", files_window::CopyToMac),
                    MenuItem::action("Paste", files_window::PasteFromMac),
                    MenuItem::action("Select All", files_window::SelectAll),
                ],
                disabled: false,
            },
            Menu {
                name: "Go".into(),
                items: vec![
                    MenuItem::action("Back", files_window::GoBack),
                    MenuItem::action("Forward", files_window::GoForward),
                    MenuItem::action("Enclosing Folder", files_window::OpenParent),
                ],
                disabled: false,
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Session Controls", ToggleSessionControls),
                    MenuItem::action("Files", ToggleFiles),
                    MenuItem::action("Toggle Full Screen", ToggleFullScreen),
                ],
                disabled: false,
            },
            Menu {
                name: "Window".into(),
                items: vec![
                    MenuItem::action("Library", ShowLibrary),
                    MenuItem::action("Disconnect Session", DisconnectSession),
                ],
                disabled: false,
            },
        ]);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        show_library(&handle, cx);
        cx.activate(true);
    });
}

/// Brings the Library window forward, or opens a new one if it was closed (`ARCH-022`).
fn show_library(rt: &Handle, cx: &mut App) {
    let open = cx
        .try_global::<OpenWindows>()
        .and_then(|windows| windows.library.clone())
        .filter(|(_, launcher)| launcher.upgrade().is_some());
    if let Some((window, _)) = open {
        if window
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            cx.activate(true);
            return;
        }
    }
    let rt = rt.clone();
    let bounds = Bounds::centered(None, size(px(1080.), px(720.)), cx);
    let opened = gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(640.), px(420.))),
            titlebar: Some(TitlebarOptions {
                title: Some(APP_NAME.into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        cx,
        move |window, cx| cx.new(|cx| Launcher::new(rt, window, cx)),
    );
    if let Ok((window, launcher)) = opened {
        cx.default_global::<OpenWindows>().library = Some((window, launcher.downgrade()));
        cx.activate(true);
    }
}

actions!(
    sealskin,
    [
        Quit,
        ShowLibrary,
        DisconnectSession,
        ToggleSessionControls,
        CloseSessionControls,
        ToggleFullScreen,
        ToggleFiles
    ]
);

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum DisplayMode {
    Fit,
    MatchWindow,
    #[default]
    Retina,
    Fixed(Resolution),
}

impl DisplayMode {
    fn request(self, viewport: Size<Pixels>, backing_scale: f32) -> Option<DisplayRequest> {
        let scale = match self {
            Self::Fit => return None,
            Self::Fixed(resolution) => {
                return Some(DisplayRequest {
                    resolution,
                    scale: None,
                })
            }
            Self::MatchWindow => 1.,
            Self::Retina => backing_scale,
        };
        Some(DisplayRequest {
            resolution: Resolution::from_window(
                (f32::from(viewport.width) * scale) as u16,
                (f32::from(viewport.height) * scale) as u16,
            ),
            scale: Some(scale),
        })
    }
}

struct Launcher {
    /// The Library window, for async updates (`ARCH-015`).
    window: AnyWindowHandle,
    rt: Handle,
    api: Option<SealSkinClient>,
    library: Option<Library>,
    preferences: Preferences,
    key_input: Entity<InputState>,
    search_input: Entity<InputState>,
    selected_filter: Filter,
    /// Launches in progress, newest first; at most one per app (`ARCH-027`). They live
    /// with this window: closing the Library abandons them (the server still finishes,
    /// and the session shows under Running sessions when the Library opens again).
    flights: Vec<Flight>,
    app_icons: HashMap<String, Arc<RenderImage>>,
    session_thumbnails: HashMap<String, Arc<RenderImage>>,
    launch_dialog: Option<(RemoteApp, LaunchDialogProps, LaunchDialogSelects)>,
    /// Every session locale with the Mac's name for it; built on the first Launch Dialog.
    language_options: Option<Vec<LaunchOption>>,
    pending_stop: Option<RemoteSession>,
    /// The Sign Out confirmation is showing, with what it says about the key.
    pending_sign_out: Option<KeySourceLabel>,
    /// Where the signed-in key came from, which decides what Sign Out removes.
    signed_in: Option<SignedInKey>,
    /// Bumped whenever the Launcher leaves a server, so a load, launch or stop still in
    /// flight from before never brings the old server back.
    server_generation: u64,
    toast: Option<Toast>,
    toast_task: Option<Task<()>>,
    toast_id: u64,
    /// Refreshes "Started … ago" every 30 s while the Library shows sessions.
    age_tick: Option<Task<()>>,
    /// Images replaced or removed since the last render; their GPU textures are freed with
    /// `window.drop_image` on the next render (GPUI never frees them on drop). Only
    /// `retire` adds to it, and it also drops `items`, which may still hold the image.
    retired_images: Vec<Arc<RenderImage>>,
    /// The Library's cards and tiles, built once per change instead of every frame.
    items: ItemCache,
    /// Bumped on every Library load and on Disconnect Server, so icon/thumbnail downloads
    /// started for an older Library never write into a newer one.
    media_generation: u64,
    /// The saved key from startup, kept so Connect can retry it after a failed first connect.
    startup_credential: Option<Credential>,
    /// A server refused on Refresh (ARCH-009); Connect with an empty field checks it again.
    retry_api: Option<SealSkinClient>,
    focus_handle: FocusHandle,
    /// Set after a typed key connects; the next render clears the key field (needs a Window).
    clear_key_input: bool,
    busy: Option<String>,
    status: Option<String>,
}

impl Launcher {
    fn new(rt: Handle, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key_input = connect::new_key_input(window, cx);
        cx.subscribe_in(
            &key_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_key(window, cx);
                }
            },
        )
        .detach();
        let search_input = library::new_search_input(window, cx);
        cx.subscribe_in(&search_input, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        let mut launcher = Self {
            window: Window::window_handle(window),
            rt,
            api: None,
            library: None,
            preferences: Preferences::default(),
            key_input,
            search_input,
            selected_filter: Filter::All,
            flights: Vec::new(),
            app_icons: HashMap::new(),
            session_thumbnails: HashMap::new(),
            launch_dialog: None,
            language_options: None,
            pending_stop: None,
            pending_sign_out: None,
            signed_in: None,
            server_generation: 0,
            toast: None,
            toast_task: None,
            toast_id: 0,
            age_tick: None,
            retired_images: Vec::new(),
            items: ItemCache::default(),
            media_generation: 0,
            startup_credential: None,
            retry_api: None,
            focus_handle: cx.focus_handle(),
            clear_key_input: false,
            busy: None,
            status: None,
        };
        // Closing the Library leaves session windows streaming; Window → Library or the
        // Dock icon opens it again (ARCH-022).
        window.on_window_should_close(cx, |window, cx| {
            let closing = Window::window_handle(window);
            let windows = cx.default_global::<OpenWindows>();
            if windows
                .library
                .as_ref()
                .is_some_and(|(open, _)| *open == closing)
            {
                windows.library = None;
            }
            true
        });
        // Back on screen: fresh "Started … ago" labels, and the render restarts the age tick.
        let this = cx.weak_entity();
        window
            .observe_window_visibility(move |visibility, _, cx| {
                if visibility.is_visible() {
                    let _ = this.update(cx, |this, cx| {
                        this.items.invalidate();
                        cx.notify();
                    });
                }
            })
            .detach();
        Theme::sync_system_appearance(Some(window), cx);
        cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
            cx.notify();
        })
        .detach();
        match Credential::startup() {
            Ok(Some(credential)) => {
                launcher.startup_credential = Some(credential.clone());
                launcher.connect(credential, false, cx)
            }
            Ok(None) => {}
            Err(error) => launcher.status = Some(error),
        }
        launcher
    }

    fn submit_key(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let text = self.key_input.read(cx).value().to_string();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            if let Some(api) = self.retry_api.take() {
                self.load(api, None, LoadKind::Connect, cx);
            } else if let Some(saved) = self.startup_credential.clone() {
                self.connect(saved, false, cx);
            } else {
                self.status = Some("Enter a SealSkin application key first.".to_string());
            }
            cx.notify();
            return;
        }
        match Credential::parse(trimmed) {
            Ok(credential) => {
                // The field is cleared once the key is accepted (see `load`), so a failed
                // connect leaves it in place to fix or retry.
                self.connect(credential, true, cx);
            }
            Err(error) => self.status = Some(error),
        }
        cx.notify();
    }

    fn connect(&mut self, credential: Credential, save: bool, cx: &mut Context<Self>) {
        match SealSkinClient::new(&credential) {
            Ok(api) => self.load(api, save.then_some(credential), LoadKind::Connect, cx),
            Err(error) => self.status = Some(error),
        }
    }

    /// Reloads the Library; `manual` (the Refresh button) confirms success with a toast.
    fn refresh(&mut self, manual: bool, cx: &mut Context<Self>) {
        if let Some(api) = self.api.clone() {
            let kind = if manual {
                LoadKind::ManualRefresh
            } else {
                LoadKind::Refresh
            };
            self.load(api, None, kind, cx);
        }
    }

    fn load(
        &mut self,
        api: SealSkinClient,
        save: Option<Credential>,
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
        self.status = None;
        let client = api.clone();
        let pending = self.rt.spawn(async move { client.library().await });
        let saved_startup_key = connecting && save.is_none();
        cx.spawn(async move |this, cx| {
            let result = pending.await.unwrap_or_else(|_| {
                Err(LibraryError::Failed(
                    "The connection was interrupted.".into(),
                ))
            });
            let _ = this.update(cx, |this, cx| {
                if !gate.admits(this.server_generation) {
                    // Signed out meanwhile: this result belongs to a server left behind.
                    return;
                }
                this.busy = None;
                match result {
                    Ok(library) => {
                        let apps = library.apps.clone();
                        let sessions = library.sessions.clone();
                        if save.is_some() {
                            this.clear_key_input = true;
                        }
                        if connecting {
                            this.signed_in =
                                Some(SignedInKey::of(api.key_source(), save.is_some()));
                        }
                        if kind == LoadKind::ManualRefresh {
                            this.show_toast("Library refreshed", cx);
                        }
                        this.startup_credential = None;
                        this.retry_api = None;
                        // Launch options remembered per app live with the other preferences.
                        // Through the queue's copy, which includes writes not yet on disk.
                        this.preferences = cx.global::<WriteQueue>().read(api.server());
                        this.api = Some(api.clone());
                        this.set_library(Some(library));
                        if let Some(credential) = save {
                            this.save_key(&credential, cx);
                        }
                        this.fetch_library_media(api, apps, sessions, cx);
                    }
                    Err(LibraryError::Refused(error)) => {
                        // ARCH-009: a server that fails the version gate is never used, even
                        // on Refresh, so drop back to Connect. Connect (empty field) retries.
                        this.retry_api = Some(api.clone());
                        this.leave_server();
                        this.status = Some(error);
                    }
                    Err(LibraryError::Failed(error)) if saved_startup_key && this.api.is_none() => {
                        this.status = Some(format!(
                            "{error} Choose Connect to try the saved key again, or paste a new one."
                        ));
                    }
                    Err(LibraryError::Failed(error)) => this.status = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn fetch_library_media(
        &mut self,
        api: SealSkinClient,
        apps: Vec<RemoteApp>,
        sessions: Vec<RemoteSession>,
        cx: &mut Context<Self>,
    ) {
        self.media_generation += 1;
        let generation = self.media_generation;
        // Drop thumbnails of sessions that are gone; their textures are retired on render.
        let live: std::collections::HashSet<&str> =
            sessions.iter().map(|s| s.session_id.as_str()).collect();
        let gone: Vec<String> = self
            .session_thumbnails
            .keys()
            .filter(|id| !live.contains(id.as_str()))
            .cloned()
            .collect();
        for id in gone {
            if let Some(image) = self.session_thumbnails.remove(&id) {
                self.retire(image);
            }
        }
        let missing_apps: Vec<RemoteApp> = apps
            .into_iter()
            .filter(|app| !self.app_icons.contains_key(&app.id))
            .collect();
        for app in missing_apps {
            let api = api.clone();
            let app_id = app.id.clone();
            let pending = self.rt.spawn(async move {
                api.app_icon(&app)
                    .await
                    .and_then(|b| decode_image(&b, ICON_MAX_EDGE))
            });
            cx.spawn(async move |this, cx| {
                if let Ok(Some(image)) = pending.await {
                    let _ = this.update(cx, |this, cx| {
                        if this.media_generation != generation {
                            this.retire(image);
                            return;
                        }
                        if let Some(old) = this.app_icons.insert(app_id, image) {
                            this.retire(old);
                        }
                        this.items.invalidate();
                        cx.notify();
                    });
                }
            })
            .detach();
        }
        for session in sessions {
            let api = api.clone();
            let session_id = session.session_id.clone();
            let pending = self.rt.spawn(async move {
                api.session_thumbnail(&session_id)
                    .await
                    .and_then(|b| decode_image(&b, THUMBNAIL_MAX_EDGE))
            });
            cx.spawn(async move |this, cx| {
                if let Ok(Some(image)) = pending.await {
                    let _ = this.update(cx, |this, cx| {
                        if this.media_generation != generation {
                            this.retire(image);
                            return;
                        }
                        if let Some(old) = this.session_thumbnails.insert(session.session_id, image)
                        {
                            this.retire(old);
                        }
                        this.items.invalidate();
                        cx.notify();
                    });
                }
            })
            .detach();
        }
    }

    /// What the Sign Out confirmation says about the key: saved whenever config.toml holds
    /// it (whichever way it got there, a retry included), unless it comes from the
    /// environment or `.env`.
    fn sign_out_label(&self) -> KeySourceLabel {
        SignedInKey::label(self.signed_in, || {
            self.api
                .as_ref()
                .is_some_and(|api| crate::sealskin::saved_key_is(api.raw_key()))
        })
    }

    /// Signs out after the confirmation: back to Connect, removing the key from config.toml
    /// whenever it is saved there. Open session windows keep streaming (`ARCH-022`).
    fn sign_out(&mut self, cx: &mut Context<Self>) {
        self.pending_sign_out = None;
        self.status = None;
        // An in-flight load, launch or stop is ignored once the generation is bumped below
        // (see `LoadGate`); a launched session shows in the Library at the next sign-in.
        self.busy = None;
        let from_environment = matches!(
            self.signed_in,
            Some(SignedInKey::Environment | SignedInKey::DotEnv)
        );
        if let Some(api) = self.api.as_ref().filter(|_| !from_environment) {
            // Removes the key only if config.toml holds this one. Queued after any pending
            // save of it, so the removal wins.
            let done = cx.global::<WriteQueue>().forget_key(api.raw_key());
            self.on_written(
                done,
                |this, error| this.status = Some(format!("Signed out. {error}")),
                cx,
            );
        }
        // An environment or .env key is never written, but don't reconnect with it silently.
        self.startup_credential = None;
        self.retry_api = None;
        self.leave_server();
        cx.notify();
    }

    /// Queues `credential` to be saved; a failure shows in the status banner and leaves the
    /// key marked as not saved.
    fn save_key(&mut self, credential: &Credential, cx: &mut Context<Self>) {
        let done = cx.global::<WriteQueue>().save_key(credential);
        let gate = LoadGate(self.server_generation);
        let window = self.window;
        cx.spawn(async move |this, cx| {
            let result = done
                .await
                .unwrap_or_else(|_| Err("Skerry couldn’t finish saving the key.".into()));
            let _ = window.update(cx, |_, _, cx| {
                this.update(cx, |this, cx| {
                    if !gate.admits(this.server_generation) {
                        return;
                    }
                    match result {
                        Ok(()) => this.signed_in = Some(SignedInKey::Saved),
                        Err(error) => {
                            this.signed_in = Some(SignedInKey::Unsaved);
                            this.status = Some(error);
                        }
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    /// Shows `message` as a toast for 3 s, replacing any toast showing. The same message
    /// again just restarts the timer.
    fn show_toast(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let message = message.into();
        let id = match &self.toast {
            Some(toast) if toast.message == message && !toast.leaving => toast.id,
            _ => {
                self.toast_id += 1;
                self.toast_id
            }
        };
        self.toast = Some(Toast {
            id,
            message,
            kind: ToastKind::Success,
            leaving: false,
        });
        let window = self.window;
        // Replacing the task drops the previous toast's timer.
        self.toast_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_DURATION).await;
            let still_shown = window
                .update(cx, |_, _, cx| {
                    this.update(cx, |this, cx| match &mut this.toast {
                        Some(toast) if toast.id == id => {
                            toast.leaving = true;
                            cx.notify();
                            true
                        }
                        _ => false,
                    })
                })
                .is_ok_and(|shown| shown.unwrap_or(false));
            if !still_shown {
                return;
            }
            cx.background_executor().timer(TOAST_EXIT).await;
            let _ = window.update(cx, |_, _, cx| {
                this.update(cx, |this, cx| {
                    if this.toast.as_ref().is_some_and(|toast| toast.id == id) {
                        this.toast = None;
                        cx.notify();
                    }
                })
            });
        }));
        cx.notify();
    }

    /// Starts the 30 s "Started … ago" refresh while the Library is on screen and shows
    /// sessions. It stops itself once there are none or the window is hidden (minimized,
    /// covered, asleep), and on sign-out; closing the window drops it. A render restarts it.
    fn ensure_age_tick(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.age_tick.is_some() || !visible || !self.has_sessions() {
            return;
        }
        let window = self.window;
        self.age_tick = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(AGE_TICK).await;
            let keep = window
                .update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| {
                        if this.has_sessions() && window.is_visible() {
                            this.items.invalidate();
                            cx.notify();
                            true
                        } else {
                            this.age_tick = None;
                            false
                        }
                    })
                })
                .is_ok_and(|keep| keep.unwrap_or(false));
            if !keep {
                break;
            }
        }));
    }

    fn has_sessions(&self) -> bool {
        self.library
            .as_ref()
            .is_some_and(|library| !library.sessions.is_empty())
    }

    /// Runs `failed` with the error once a queued write fails, through the Library window.
    fn on_written<E: ToString + 'static>(
        &self,
        done: tokio::sync::oneshot::Receiver<Result<(), E>>,
        failed: impl FnOnce(&mut Self, String) + 'static,
        cx: &mut Context<Self>,
    ) {
        let window = self.window;
        cx.spawn(async move |this, cx| {
            let error = match done.await {
                Ok(Ok(())) => return,
                Ok(Err(error)) => error.to_string(),
                Err(_) => "Skerry couldn’t finish saving its settings.".to_string(),
            };
            let _ = window.update(cx, |_, _, cx| {
                this.update(cx, |this, cx| {
                    failed(this, error);
                    cx.notify();
                })
            });
        })
        .detach();
    }

    /// Frees `image`'s texture on the next render. The item cache may hold it, so it is
    /// dropped in the same update (`ARCH-014`).
    fn retire(&mut self, image: Arc<RenderImage>) {
        self.retired_images.push(image);
        self.items.invalidate();
    }

    /// Replaces the Library; the cards and tiles rebuild on the next render.
    fn set_library(&mut self, library: Option<Library>) {
        self.library = library;
        self.items.invalidate();
    }

    /// Changes the loaded Library in place; the cards and tiles rebuild on the next render.
    fn edit_library(&mut self, edit: impl FnOnce(&mut Library)) {
        if let Some(library) = &mut self.library {
            edit(library);
            self.items.invalidate();
        }
    }

    /// Returns to the Connect screen, dropping the Library and its textures.
    fn leave_server(&mut self) {
        self.api = None;
        self.set_library(None);
        self.media_generation += 1;
        self.server_generation += 1;
        self.signed_in = None;
        self.pending_sign_out = None;
        self.age_tick = None;
        self.toast = None;
        self.toast_task = None;
        let images: Vec<_> = self
            .app_icons
            .drain()
            .chain(self.session_thumbnails.drain())
            .map(|(_, image)| image)
            .collect();
        for image in images {
            self.retire(image);
        }
        self.launch_dialog = None;
        self.pending_stop = None;
        // Their poll and request tasks find no flight and end; the server still finishes
        // each launch, which shows at the next sign-in.
        self.flights.clear();
    }

    fn find_app(&self, app_id: &str) -> Option<RemoteApp> {
        let library = self.library.as_ref()?;
        library.apps.iter().find(|app| app.id == app_id).cloned()
    }

    fn find_session(&self, session_id: &str) -> Option<RemoteSession> {
        let library = self.library.as_ref()?;
        library
            .sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .cloned()
    }

    /// What this server lets the user launch with, from the loaded Library.
    fn launch_context(&self) -> LaunchContext {
        self.library
            .as_ref()
            .map(|library| LaunchContext {
                status: library.status.clone(),
                homes: library.homes.clone(),
            })
            .unwrap_or_default()
    }

    fn open_launch_dialog(&mut self, app: RemoteApp, window: &mut Window, cx: &mut Context<Self>) {
        let context = self.launch_context();
        let memory = self
            .preferences
            .launch
            .get(&app.id)
            .cloned()
            .unwrap_or_default();
        let default_language = launch_options::default_language(&launch_options::mac_locale());
        let selection = context.selection(&app, &memory, &default_language);
        let language_options = self
            .language_options
            .get_or_insert_with(|| {
                launch_options::language_options(launch_options::mac_locale_name)
            })
            .clone();
        let props = LaunchDialogProps {
            app_id: app.id.clone(),
            app_name: app.name.clone(),
            app_icon: self.app_icons.get(&app.id).cloned(),
            favorite: self.preferences.library.is_favorite(&app.id),
            hidden: self.preferences.library.is_hidden(&app.id),
            storage_options: context.storage_options(&app),
            selected_storage: selection.home_dir,
            gpu_options: context.gpu_options(&app),
            selected_gpu: selection.gpu,
            vpn_options: context.vpn_options(),
            vpn_endpoint_options: context.vpn_endpoints(&selection.vpn),
            selected_vpn: selection.vpn,
            selected_vpn_endpoint: selection.vpn_endpoint,
            language_options,
            selected_language: selection.language,
            collaborative: selection.room,
            wayland: selection.wayland,
            busy: false,
            show_copy_link: true,
            show_organize: true,
            link_copied: false,
            running_match: None,
        };
        let selects = LaunchDialogSelects::from_props(&props, window, cx);
        let fields = [
            (&selects.storage, LaunchField::Storage),
            (&selects.gpu, LaunchField::Gpu),
            (&selects.vpn, LaunchField::Vpn),
            (&selects.vpn_endpoint, LaunchField::VpnEndpoint),
            (&selects.language, LaunchField::Language),
        ];
        for (select, field) in fields {
            if let Some(select) = select {
                self.bind_launch_select(select, field, window, cx);
            }
        }
        self.launch_dialog = Some((app, props, selects));
        self.update_running_match();
        // Take focus off the search field so Escape reaches the dialog.
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// The running session the dialog's current choices would duplicate (`ARCH-027`).
    fn running_match(&self) -> Option<RemoteSession> {
        let (app, dialog, _) = self.launch_dialog.as_ref()?;
        let library = self.library.as_ref()?;
        self.launch_context()
            .matching_session(app, &selection_of(dialog), &library.sessions)
            .cloned()
    }

    /// Recomputes the dialog's running match after its storage or room flag changed.
    fn update_running_match(&mut self) {
        let found = self.running_match().map(|session| RunningMatch {
            started_label: format_started_ago_at(session.created_at, unix_now())
                .map(|label| lowercase_first(&label)),
        });
        if let Some((_, dialog, _)) = self.launch_dialog.as_mut() {
            dialog.running_match = found;
        }
    }

    /// Writes a confirmed dropdown choice into the dialog. A new VPN brings its own endpoints,
    /// so that field is rebuilt with the first one chosen.
    fn bind_launch_select(
        &mut self,
        select: &LaunchSelect,
        field: LaunchField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(
            select,
            window,
            move |this, _, event: &SelectEvent<LaunchOptions>, window, cx| {
                let SelectEvent::Confirm(Some(value)) = event else {
                    return;
                };
                let context = this.launch_context();
                let Some((_, dialog, selects)) = this.launch_dialog.as_mut() else {
                    return;
                };
                dialog.link_copied = false;
                match field {
                    LaunchField::Storage => dialog.selected_storage = value.clone(),
                    LaunchField::Gpu => dialog.selected_gpu = value.clone(),
                    LaunchField::VpnEndpoint => dialog.selected_vpn_endpoint = value.clone(),
                    LaunchField::Language => dialog.selected_language = value.clone(),
                    LaunchField::Vpn => {
                        dialog.selected_vpn = value.clone();
                        dialog.vpn_endpoint_options = context.vpn_endpoints(value);
                        dialog.selected_vpn_endpoint = dialog
                            .vpn_endpoint_options
                            .first()
                            .map(|endpoint| endpoint.id.clone())
                            .unwrap_or_default();
                        let props = dialog.clone();
                        selects.vpn_endpoint =
                            LaunchDialogSelects::build_vpn_endpoint(&props, window, cx);
                        if let Some(endpoint) = selects.vpn_endpoint.clone() {
                            this.bind_launch_select(
                                &endpoint,
                                LaunchField::VpnEndpoint,
                                window,
                                cx,
                            );
                        }
                    }
                }
                this.update_running_match();
                cx.notify();
            },
        )
        .detach();
    }

    fn handle_launch_dialog_action(
        &mut self,
        action: LaunchDialogAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            LaunchDialogAction::Close => {
                self.launch_dialog = None;
            }
            // Saved at once; the dialog stays open so the change can be undone there.
            LaunchDialogAction::ToggleFavorite => {
                if let Some((app, dialog, _)) = self.launch_dialog.as_ref() {
                    let (app_id, favorite) = (app.id.clone(), !dialog.favorite);
                    self.set_favorite(&app_id, favorite, cx);
                }
            }
            LaunchDialogAction::ToggleHidden => {
                if let Some((app, dialog, _)) = self.launch_dialog.as_ref() {
                    let (app_id, hidden) = (app.id.clone(), !dialog.hidden);
                    self.set_hidden(&app_id, hidden, false, cx);
                }
            }
            LaunchDialogAction::SetCollaborative(enabled) => {
                if let Some((_, dialog, _)) = self.launch_dialog.as_mut() {
                    dialog.collaborative = enabled;
                    dialog.link_copied = false;
                }
                self.update_running_match();
            }
            LaunchDialogAction::OpenRunning => {
                // Close the dialog first, in this same update; `open_session` defers the
                // window open (ARCH-006, ARCH-022).
                let running = self.running_match();
                self.launch_dialog = None;
                if let Some(session) = running {
                    self.open_session(session, window, cx);
                }
            }
            LaunchDialogAction::SetWayland(enabled) => {
                if let Some((_, dialog, _)) = self.launch_dialog.as_mut() {
                    dialog.wayland = enabled;
                    dialog.link_copied = false;
                }
            }
            LaunchDialogAction::CopyLink => {
                let context = self.launch_context();
                if let (Some(api), Some((app, dialog, _))) = (&self.api, &mut self.launch_dialog) {
                    let default_language =
                        launch_options::default_language(&launch_options::mac_locale());
                    let query = context.link_query(app, &selection_of(dialog), &default_language);
                    if let Some(link) = api.app_deep_link(&app.id, &query) {
                        cx.write_to_clipboard(ClipboardItem::new_string(link));
                        // The button's own "Copied" stays too (ARCH-013).
                        dialog.link_copied = true;
                        self.show_toast("Link copied", cx);
                    }
                }
            }
            LaunchDialogAction::Launch => {
                if let Some((app, dialog, _)) = self.launch_dialog.take() {
                    let selection = selection_of(&dialog);
                    let params = self.launch_context().params(
                        &app,
                        &selection,
                        launch_options::mac_timezone(),
                    );
                    self.start_launch(app, params, Some(selection), cx);
                }
            }
        }
        cx.notify();
    }

    /// Launches `app_id` with its remembered options at once (Option-click, the hover ▶,
    /// or the tile menu). Without remembered options it opens the dialog. It always starts
    /// a new session: reuse is what the session card's Open is for.
    fn quick_launch(&mut self, app_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(app) = self.find_app(app_id) else {
            return;
        };
        match quick_launch_plan(self.preferences.launch.get(app_id)) {
            QuickLaunch::Dialog => self.open_launch_dialog(app, window, cx),
            QuickLaunch::Launch(memory) => {
                // Revalidated as the dialog would be, so the two never disagree (ARCH-016).
                let context = self.launch_context();
                let default_language =
                    launch_options::default_language(&launch_options::mac_locale());
                let selection = context.selection(&app, &memory, &default_language);
                let params = context.params(&app, &selection, launch_options::mac_timezone());
                self.start_launch(app, params, Some(selection), cx);
            }
        }
    }

    fn session_ids(&self) -> HashSet<String> {
        self.library
            .as_ref()
            .map(|library| {
                library
                    .sessions
                    .iter()
                    .map(|session| session.session_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn flight_mut(&mut self, launch_id: &str) -> Option<&mut Flight> {
        self.flights
            .iter_mut()
            .find(|flight| flight.launch_id == launch_id)
    }

    /// Starts following a launch of `app`; once ready, `remember` becomes the app's
    /// remembered options. One launch per app at a time; the Library stays usable.
    fn start_launch(
        &mut self,
        app: RemoteApp,
        params: LaunchParams,
        remember: Option<LaunchSelection>,
        cx: &mut Context<Self>,
    ) {
        if self.api.is_none() || !can_start(&self.flights, &app.id) {
            return;
        }
        // A failed card for this app gives way to the new launch.
        self.flights.retain(|flight| flight.app.id != app.id);
        let flight = Flight::new(
            app,
            params,
            remember,
            self.session_ids(),
            Instant::now(),
            unix_now_f64(),
        );
        let launch_id = flight.launch_id.clone();
        self.flights.insert(0, flight);
        self.status = None;
        self.post_flight(&launch_id, cx);
        self.poll_flight(launch_id, cx);
        cx.notify();
    }

    /// Sends (or resends, under the same id) the flight's launch request.
    fn post_flight(&mut self, launch_id: &str, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let Some(flight) = self.flight_mut(launch_id) else {
            return;
        };
        flight.begin_attempt();
        let (app, params) = (flight.app.clone(), flight.params.clone());
        let (sent, on_sent) = tokio::sync::oneshot::channel();
        let pending = self.rt.spawn(async move {
            api.launch_with_options(app, params, move || {
                let _ = sent.send(());
            })
            .await
        });
        // Applied through the Library window's handle: gpui-pre's `update_in` uses the
        // entity's *current* window, which is Session Controls whenever that panel last
        // rendered the Launcher (ARCH-015).
        let window = self.window;
        let gate = LoadGate(self.server_generation);
        let launch_id = launch_id.to_string();
        cx.spawn(async move |this, cx| {
            if on_sent.await.is_ok() {
                let now = Instant::now();
                let _ = window.update(cx, |_, _, cx| {
                    this.update(cx, |this, _| {
                        if let Some(flight) = this.flight_mut(&launch_id) {
                            flight.sent(now);
                        }
                    })
                });
            }
            let result = pending.await.unwrap_or_else(|_| {
                Err(LaunchError::Network("The launch was interrupted.".into()))
            });
            let _ = window.update(cx, |_, window, cx| {
                this.update(cx, |this, cx| {
                    if !gate.admits(this.server_generation) {
                        return;
                    }
                    if let Some(flight) = this.flight_mut(&launch_id) {
                        flight.posted(result, Instant::now());
                        this.flight_changed(&launch_id, window, cx);
                    }
                })
            });
        })
        .detach();
    }

    /// Polls the progress route while the flight runs. The loop ends once the flight is
    /// gone (Dismiss, sign-out, a fresh Retry under a new id) or no longer running, or the
    /// Library window has closed.
    fn poll_flight(&mut self, launch_id: String, cx: &mut Context<Self>) {
        let window = self.window;
        let gate = LoadGate(self.server_generation);
        cx.spawn(async move |this, cx| loop {
            let next = window
                .update(cx, |_, _, cx| {
                    this.update(cx, |this, _| {
                        let api = this.api.clone()?;
                        let rt = this.rt.clone();
                        let flight = this.flight_mut(&launch_id)?;
                        flight
                            .is_running()
                            .then(|| (flight.poll_interval(Instant::now()), api, rt))
                    })
                })
                .ok()
                .and_then(Result::ok)
                .flatten();
            let Some((interval, api, rt)) = next else {
                break;
            };
            cx.background_executor().timer(interval).await;
            let id = launch_id.clone();
            let answer = rt
                .spawn(async move { api.launch_progress(&id).await })
                .await
                .unwrap_or_else(|_| Err("interrupted".into()));
            let applied = window.update(cx, |_, window, cx| {
                this.update(cx, |this, cx| {
                    if !gate.admits(this.server_generation) {
                        return;
                    }
                    let Some(flight) = this.flight_mut(&launch_id) else {
                        return;
                    };
                    match &answer {
                        Ok(answer) => flight.observe(answer, Instant::now()),
                        Err(_) => flight.poll_failed(),
                    }
                    if matches!(flight.outcome, Outcome::Checking) {
                        this.check_sessions(&launch_id, cx);
                    }
                    this.flight_changed(&launch_id, window, cx);
                })
            });
            if !matches!(applied, Ok(Ok(()))) {
                break;
            }
        })
        .detach();
    }

    /// The one sessions lookup before a lost launch gives up: a new session of the app
    /// may be the launch, started by a request whose answer was lost.
    fn check_sessions(&mut self, launch_id: &str, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let pending = self.rt.spawn(async move { api.sessions().await });
        let window = self.window;
        let gate = LoadGate(self.server_generation);
        let launch_id = launch_id.to_string();
        cx.spawn(async move |this, cx| {
            let sessions = pending.await.ok().and_then(Result::ok);
            let _ = window.update(cx, |_, window, cx| {
                this.update(cx, |this, cx| {
                    if !gate.admits(this.server_generation) {
                        return;
                    }
                    if let Some(flight) = this.flight_mut(&launch_id) {
                        flight.sessions_checked(sessions.as_deref(), Instant::now());
                        this.flight_changed(&launch_id, window, cx);
                    }
                })
            });
        })
        .detach();
    }

    /// Acts on a flight's new state: a ready launch opens its session.
    fn flight_changed(&mut self, launch_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let ready = self.flights.iter().position(|flight| {
            flight.launch_id == launch_id && matches!(flight.outcome, Outcome::Ready(_))
        });
        if let Some(index) = ready {
            let flight = self.flights.remove(index);
            if let Outcome::Ready(session) = flight.outcome {
                self.finish_launch(
                    &flight.app.id,
                    flight.remember.as_ref(),
                    session,
                    window,
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// A launch is ready: list its session (once), record it as recent, remember its
    /// options, and open its window.
    fn finish_launch(
        &mut self,
        app_id: &str,
        remember: Option<&LaunchSelection>,
        session: RemoteSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_library(|library| {
            if !library
                .sessions
                .iter()
                .any(|listed| listed.session_id == session.session_id)
            {
                library.sessions.insert(0, session.clone());
            }
            if session.home.starts_with("auto-") && !library.homes.contains(&session.home) {
                library.homes.push(session.home.clone());
            }
        });
        let memory = remember.map(LaunchSelection::memory);
        let id = app_id.to_string();
        // The recent and the remembered options go out as one queued write.
        self.save_organization(
            move |p| {
                p.library.record_launch(&id);
                if let Some(memory) = &memory {
                    p.launch.insert(id.clone(), memory.clone());
                }
            },
            cx,
        );
        self.open_session(session, window, cx);
    }

    fn flight_action(&mut self, launch_id: &str, action: FlightAction, cx: &mut Context<Self>) {
        let now = Instant::now();
        let known = self.session_ids();
        let Some(flight) = self.flight_mut(launch_id) else {
            return;
        };
        match action {
            FlightAction::KeepWaiting => flight.keep_waiting(now),
            FlightAction::Dismiss => self.flights.retain(|f| f.launch_id != launch_id),
            FlightAction::Retry if flight.can_retry_same() => {
                // The same id: the server joins the launch if it still runs.
                flight.retry_same(now);
                self.post_flight(launch_id, cx);
            }
            FlightAction::Retry if !flight.in_flight() => {
                flight.retry_fresh(known, now, unix_now_f64());
                let fresh = flight.launch_id.clone();
                self.post_flight(&fresh, cx);
                self.poll_flight(fresh, cx);
            }
            FlightAction::Retry => {}
        }
        cx.notify();
    }

    /// Queues one change to this server's preferences: applied to the Launcher's copy now
    /// and through the write queue (one writer, re-read first, `ARCH-022`). Tiles rebuild.
    fn save_organization(
        &mut self,
        change: impl Fn(&mut Preferences) + Send + Sync + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(api) = &self.api else {
            return;
        };
        organize(&mut self.preferences, &mut self.items, &change);
        let done = cx.global::<WriteQueue>().preferences(api.server(), change);
        // The in-memory change stays, so the Library doesn't flicker back; the next write
        // that succeeds saves it.
        self.on_written(
            done,
            |this, _| this.status = Some("Couldn’t save your Library changes.".into()),
            cx,
        );
    }

    fn set_favorite(&mut self, app_id: &str, favorite: bool, cx: &mut Context<Self>) {
        let id = app_id.to_string();
        self.save_organization(move |p| p.library.set_favorite(&id, favorite), cx);
        if let Some((app, dialog, _)) = self.launch_dialog.as_mut() {
            if app.id == app_id {
                dialog.favorite = favorite;
            }
        }
        cx.notify();
    }

    /// Hides or shows an app. `toast`: from the tile menu, where the tile leaves (or comes
    /// back) and a toast says where it went.
    fn set_hidden(&mut self, app_id: &str, hidden: bool, toast: bool, cx: &mut Context<Self>) {
        let id = app_id.to_string();
        self.save_organization(move |p| p.library.set_hidden(&id, hidden), cx);
        if let Some((app, dialog, _)) = self.launch_dialog.as_mut() {
            if app.id == app_id {
                dialog.hidden = hidden;
            }
        }
        if self.selected_filter == Filter::Hidden && self.preferences.library.hidden.is_empty() {
            self.selected_filter = Filter::All;
        }
        if toast {
            if let Some(app) = self.find_app(app_id) {
                let message = if hidden {
                    format!("{} hidden. Find it under Hidden.", app.name)
                } else {
                    format!("{} is back in the Library.", app.name)
                };
                self.show_toast(message, cx);
            }
        }
        cx.notify();
    }

    fn stop_session(&mut self, session: RemoteSession, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| self.busy.is_none()) else {
            return;
        };
        self.pending_stop = None;
        self.status = None;
        self.busy = Some(format!("Stopping {}…", session.app_name));
        let session_id = session.session_id.clone();
        let pending = self
            .rt
            .spawn(async move { api.stop_session(&session_id).await });
        let gate = LoadGate(self.server_generation);
        cx.spawn(async move |this, cx| {
            let result = pending
                .await
                .unwrap_or_else(|_| Err("Stopping the session was interrupted.".into()));
            let _ = this.update(cx, |this, cx| {
                if !gate.admits(this.server_generation) {
                    return;
                }
                this.busy = None;
                match result {
                    Ok(()) => {
                        this.edit_library(|library| {
                            library
                                .sessions
                                .retain(|s| s.session_id != session.session_id);
                        });
                        if let Some(image) = this.session_thumbnails.remove(&session.session_id) {
                            this.retire(image);
                        }
                    }
                    Err(error) => this.status = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Opens `session` in a window of its own, or brings forward the window already showing
    /// it: Skerry never opens a second connection to a session it shows (`ARCH-022`).
    fn open_session(
        &mut self,
        session: RemoteSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let rt = self.rt.clone();
        let library = window.bounds();
        // Opened outside this update: GPUI must not render a new window while the Launcher
        // is leased (ARCH-006).
        cx.defer(move |cx| {
            let windows = cx.default_global::<OpenWindows>();
            if let Some(open) = windows.session_window(&session.session_id) {
                let _ = open.update(cx, |_, window, _| window.activate_window());
                return;
            }
            let step = px(28.) * (windows.sessions.len() as f32 + 1.);
            let bounds = Bounds::new(
                point(library.origin.x + step, library.origin.y + step),
                size(
                    library.size.width.max(px(1080.)),
                    library.size.height.max(px(720.)),
                ),
            );
            let session_id = session.session_id.clone();
            let opened = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(640.), px(420.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some(format!("{} — {APP_NAME}", session.app_name).into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                move |window, cx| cx.new(|cx| SessionWindow::new(rt, api, session, window, cx)),
            );
            if let Ok((window, entity)) = opened {
                cx.default_global::<OpenWindows>()
                    .sessions
                    .push(OpenSession {
                        window,
                        session_id,
                        entity: entity.downgrade(),
                    });
            }
        });
    }
}

pub(crate) fn native_window(window: &Window) -> Option<objc2::rc::Retained<NSWindow>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // GPUI supplies its live NSView. This call runs on the main thread.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window()
}

fn session_chrome(window: &Window, visible: bool) {
    let Some(native) = native_window(window) else {
        return;
    };
    if !window.is_fullscreen() {
        let mut style = native.styleMask();
        style.set(NSWindowStyleMask::FullSizeContentView, !visible);
        if native.styleMask() != style {
            native.setStyleMask(style);
        }
        native.setTitlebarAppearsTransparent(!visible);
    }
    native.setTitleVisibility(if visible {
        NSWindowTitleVisibility::Visible
    } else {
        NSWindowTitleVisibility::Hidden
    });
    set_window_buttons(&native, visible);
}

fn set_window_buttons(native: &NSWindow, visible: bool) {
    for button in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = native.standardWindowButton(button) {
            button.setHidden(!visible);
        }
    }
}

/// Shows the close / minimize / zoom buttons over a session's picture while the pointer is
/// in the window's top-left corner, so a session window can always be closed (`ARCH-022`).
pub(crate) fn reveal_window_buttons(window: &Window, visible: bool) {
    if let Some(native) = native_window(window) {
        set_window_buttons(&native, visible);
    }
}

fn dark_appearance(window: &Window) -> bool {
    matches!(
        window.appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.retired_images.drain(..) {
            let _ = window.drop_image(image);
        }
        if std::mem::take(&mut self.clear_key_input) {
            self.key_input
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
        let theme = cx.theme().clone();
        self.ensure_age_tick(window.is_visible(), cx);
        let (Some(api), Some(library)) = (&self.api, &self.library) else {
            let props = ConnectProps {
                status: self.status.clone(),
                busy: self.busy.clone(),
            };
            return connect::render(
                &theme,
                &props,
                &self.key_input,
                cx.listener(|this, _, window, cx| this.submit_key(window, cx)),
            );
        };

        let server = api.server().to_string();
        // Built after the retired textures were dropped above and before any element uses
        // them, so no cached item hands a freed texture to `img` (`ARCH-014`).
        let (apps, sessions) = self.items.get(|| {
            build_items(
                library,
                &self.preferences,
                &self.app_icons,
                &self.session_thumbnails,
                unix_now(),
            )
        });
        // Flights change on every poll, so they stay out of the cached items.
        let now = Instant::now();
        let props = LibraryProps {
            app_name: APP_NAME.to_string(),
            server: server.clone(),
            apps,
            sessions,
            favorites: self.preferences.library.favorites.clone(),
            recent: self.preferences.library.recent.clone(),
            launching: self
                .flights
                .iter()
                .map(|flight| launching_item(flight, self.app_icons.get(&flight.app.id), now))
                .collect(),
            launching_apps: self
                .flights
                .iter()
                .filter(|flight| flight.in_flight())
                .map(|flight| {
                    (
                        flight.app.id.clone(),
                        SharedString::from(flight.stage().tile_label()),
                    )
                })
                .collect(),
            search_query: self.search_input.read(cx).value().to_string(),
            selected_filter: self.selected_filter.clone(),
            status: self.status.clone(),
            busy: self.busy.clone(),
        };

        // Handlers capture only the entity and look the app or flight up by id (ARCH-026).
        let launcher_for_launch = cx.entity().clone();
        let launcher_for_quick = cx.entity().clone();
        let launcher_for_favorite = cx.entity().clone();
        let launcher_for_hidden = cx.entity().clone();
        let launcher_for_flight = cx.entity().clone();
        let launcher_for_resume = cx.entity().clone();
        let launcher_for_stop = cx.entity().clone();
        let launcher_for_filter = cx.entity().clone();
        let launcher_for_dialog = cx.entity().clone();

        div()
            .size_full()
            .relative()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape"
                    && (this.launch_dialog.is_some()
                        || this.pending_stop.is_some()
                        || this.pending_sign_out.is_some())
                {
                    this.launch_dialog = None;
                    this.pending_stop = None;
                    this.pending_sign_out = None;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(library::render(
                &theme,
                &props,
                &self.search_input,
                LibraryHandlers {
                    on_refresh: Box::new(cx.listener(|this, _, _, cx| this.refresh(true, cx))),
                    on_sign_out: Box::new(cx.listener(|this, _, window, cx| {
                        this.pending_sign_out = Some(this.sign_out_label());
                        // Focus the Launcher so Escape reaches the confirmation.
                        window.focus(&this.focus_handle, cx);
                        cx.notify();
                    })),
                    on_launch: Rc::new(move |app_id, window, cx| {
                        launcher_for_launch.update(cx, |this, cx| {
                            if let Some(app) = this.find_app(&app_id) {
                                this.open_launch_dialog(app, window, cx);
                            }
                        });
                    }),
                    on_quick_launch: Rc::new(move |app_id, window, cx| {
                        launcher_for_quick
                            .update(cx, |this, cx| this.quick_launch(&app_id, window, cx));
                    }),
                    on_toggle_favorite: Rc::new(move |app_id, _, cx| {
                        launcher_for_favorite.update(cx, |this, cx| {
                            let favorite = !this.preferences.library.is_favorite(&app_id);
                            this.set_favorite(&app_id, favorite, cx);
                        });
                    }),
                    on_toggle_hidden: Rc::new(move |app_id, _, cx| {
                        launcher_for_hidden.update(cx, |this, cx| {
                            let hidden = !this.preferences.library.is_hidden(&app_id);
                            this.set_hidden(&app_id, hidden, true, cx);
                        });
                    }),
                    on_flight_action: Rc::new(move |launch_id, action, _, cx| {
                        launcher_for_flight
                            .update(cx, |this, cx| this.flight_action(&launch_id, action, cx));
                    }),
                    on_resume: Rc::new(move |session_id, window, cx| {
                        launcher_for_resume.update(cx, |this, cx| {
                            if let Some(session) = this.find_session(&session_id) {
                                this.open_session(session, window, cx);
                            }
                        });
                    }),
                    on_stop: Some(Rc::new(move |session_id, window, cx| {
                        launcher_for_stop.update(cx, |this, cx| {
                            if let Some(session) = this.find_session(&session_id) {
                                this.pending_stop = Some(session);
                                window.focus(&this.focus_handle, cx);
                                cx.notify();
                            }
                        });
                    })),
                    on_select_filter: Rc::new(move |filter, _, cx| {
                        launcher_for_filter.update(cx, |this, cx| {
                            this.selected_filter = filter;
                            cx.notify();
                        });
                    }),
                },
            ))
            .when_some(self.launch_dialog.as_ref(), |view, (_, dialog, selects)| {
                let on_action = Rc::new(
                    move |action: LaunchDialogAction, window: &mut Window, cx: &mut App| {
                        launcher_for_dialog.update(cx, |this, cx| {
                            this.handle_launch_dialog_action(action, window, cx)
                        });
                    },
                );
                view.child(launch_dialog::render_modal(
                    &theme, dialog, selects, on_action,
                ))
            })
            .when_some(self.pending_stop.clone(), |view, pending_session| {
                let app_name = pending_session.app_name.clone();
                view.child(session::stop_confirm_modal(
                    &theme,
                    app_name,
                    cx.listener(|this, _, _, cx| {
                        this.pending_stop = None;
                        cx.notify();
                    }),
                    cx.listener(move |this, _, _, cx| {
                        this.stop_session(pending_session.clone(), cx);
                    }),
                ))
            })
            .when_some(self.pending_sign_out, |view, source| {
                view.child(session::sign_out_confirm_modal(
                    &theme,
                    &server,
                    source,
                    cx.listener(|this, _, _, cx| {
                        this.pending_sign_out = None;
                        cx.notify();
                    }),
                    cx.listener(|this, _, _, cx| this.sign_out(cx)),
                ))
            })
            // Last, so it sits above the Launch Dialog and both confirmations.
            .when_some(self.toast.as_ref(), |view, toast| {
                view.child(components::toast_layer(&theme, toast))
            })
    }
}

/// How often "Started … ago" labels refresh; minutes are the finest unit they show.
const AGE_TICK: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadKind {
    Connect,
    Refresh,
    /// The Refresh button: success shows a toast.
    ManualRefresh,
}

/// The server generation a piece of async work started in; its result is used only if the
/// Launcher hasn't left that server since.
#[derive(Clone, Copy)]
struct LoadGate(u64);

impl LoadGate {
    fn admits(self, current: u64) -> bool {
        self.0 == current
    }
}

/// Where the signed-in key came from and whether Skerry saved it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SignedInKey {
    /// In config.toml: read at startup, or typed and saved.
    Saved,
    /// Typed, with its save still queued.
    Saving,
    Environment,
    DotEnv,
    /// Typed, and the save failed (or it connected through a retry that never saves).
    Unsaved,
}

impl SignedInKey {
    /// What a connect with a key from `source` leaves; `saving` when its save is queued.
    fn of(source: KeySource, saving: bool) -> Self {
        match (source, saving) {
            (_, true) => Self::Saving,
            (KeySource::ConfigFile, false) => Self::Saved,
            (KeySource::Environment, false) => Self::Environment,
            (KeySource::DotEnv, false) => Self::DotEnv,
            (KeySource::Typed, false) => Self::Unsaved,
        }
    }

    /// What Sign Out says about the key. `in_config`: config.toml holds this key now, which
    /// makes it saved whatever way it connected (a paste saved, then a Refused retry).
    fn label(signed_in: Option<Self>, in_config: impl FnOnce() -> bool) -> KeySourceLabel {
        match signed_in {
            Some(Self::Environment) => KeySourceLabel::Environment,
            Some(Self::DotEnv) => KeySourceLabel::DotEnv,
            Some(Self::Saved | Self::Saving) => KeySourceLabel::SavedFile,
            _ if in_config() => KeySourceLabel::SavedFile,
            _ => KeySourceLabel::ThisRunOnly,
        }
    }
}

/// The Launch Dialog's dropdowns.
#[derive(Clone, Copy)]
enum LaunchField {
    Storage,
    Gpu,
    Vpn,
    VpnEndpoint,
    Language,
}

/// The dialog's current choices.
fn selection_of(dialog: &LaunchDialogProps) -> LaunchSelection {
    LaunchSelection {
        home_dir: dialog.selected_storage.clone(),
        gpu: dialog.selected_gpu.clone(),
        vpn: dialog.selected_vpn.clone(),
        vpn_endpoint: dialog.selected_vpn_endpoint.clone(),
        language: dialog.selected_language.clone(),
        wayland: dialog.wayland,
        room: dialog.collaborative,
    }
}

/// Longest edge kept for app icons (shown at most ~80 pt @2x).
const ICON_MAX_EDGE: u32 = 256;
/// Longest edge kept for session thumbnails (card is ~470 pt wide @2x).
const THUMBNAIL_MAX_EDGE: u32 = 960;

/// Decodes server-supplied image bytes with hard size limits and downscales to `max_edge`,
/// so a huge or hostile image can't exhaust memory or pin a full-size texture.
fn decode_image(bytes: &[u8], max_edge: u32) -> Option<Arc<RenderImage>> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.limits(limits);
    let mut decoded = reader.decode().ok()?;
    if decoded.width() > max_edge || decoded.height() > max_edge {
        decoded = decoded.thumbnail(max_edge, max_edge);
    }
    let mut pixels = decoded.into_rgba8();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2); // RGBA -> BGRA for GPUI RenderImage
    }
    Some(Arc::new(RenderImage::new(smallvec![image::Frame::new(
        pixels
    )])))
}

/// The apps and the sessions, as the Library draws them.
type LibraryItems = (Rc<[AppItem]>, Rc<[SessionItem]>);

/// The Library's cards and tiles, built once and shared until the Library, an icon or a
/// thumbnail changes (or the age tick asks for fresh "Started … ago" labels).
#[derive(Default)]
struct ItemCache {
    items: Option<LibraryItems>,
}

impl ItemCache {
    fn invalidate(&mut self) {
        self.items = None;
    }

    fn get(&mut self, build: impl FnOnce() -> LibraryItems) -> LibraryItems {
        self.items.get_or_insert_with(build).clone()
    }
}

fn build_items(
    library: &Library,
    preferences: &Preferences,
    icons: &HashMap<String, Arc<RenderImage>>,
    thumbnails: &HashMap<String, Arc<RenderImage>>,
    now: u64,
) -> LibraryItems {
    let organization = &preferences.library;
    let apps = library
        .apps
        .iter()
        .map(|app| AppItem {
            id: app.id.clone(),
            name: app.name.clone(),
            categories: if app.kind.is_empty() {
                Vec::new()
            } else {
                vec![app.kind.clone()]
            },
            favorite: organization.is_favorite(&app.id),
            hidden: organization.is_hidden(&app.id),
            remembered: preferences.launch.contains_key(&app.id),
            icon: icons.get(&app.id).cloned(),
        })
        .collect();
    let sessions = library
        .sessions
        .iter()
        .map(|session| SessionItem {
            id: session.session_id.clone(),
            app_name: session.app_name.clone(),
            started_label: format_started_ago_at(session.created_at, now),
            gpu: session.gpu,
            // No home means a cleanroom session; label it so every card shows its
            // storage and the cards line up.
            storage_label: Some(if session.home.is_empty() {
                "Cleanroom".to_string()
            } else {
                session.home.clone()
            }),
            thumbnail: thumbnails.get(&session.session_id).cloned(),
            app_icon: icons.get(&session.app_id).cloned(),
        })
        .collect();
    (apps, sessions)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn unix_now_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

/// "Started 12m ago" → "started 12m ago", for use after a "·".
fn lowercase_first(label: &str) -> String {
    let mut chars = label.chars();
    chars
        .next()
        .map(|first| first.to_lowercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Applies a preference change to the Launcher's copy and marks the tiles for a rebuild,
/// since favorites, hidden apps, and remembered options live in the cached items.
fn organize(
    preferences: &mut Preferences,
    items: &mut ItemCache,
    change: &dyn Fn(&mut Preferences),
) {
    change(preferences);
    items.invalidate();
}

/// At most one launch per app is in flight; its tile and menu are disabled meanwhile.
fn can_start(flights: &[Flight], app_id: &str) -> bool {
    !flights
        .iter()
        .any(|flight| flight.app.id == app_id && flight.in_flight())
}

/// What a one-click launch does.
#[derive(Debug, PartialEq)]
enum QuickLaunch {
    /// Launch with these remembered options (revalidated first).
    Launch(LaunchMemory),
    /// Nothing remembered: open the Launch Dialog.
    Dialog,
}

fn quick_launch_plan(memory: Option<&LaunchMemory>) -> QuickLaunch {
    memory.map_or(QuickLaunch::Dialog, |memory| {
        QuickLaunch::Launch(memory.clone())
    })
}

/// A flight as its card in Running sessions shows it.
fn launching_item(flight: &Flight, icon: Option<&Arc<RenderImage>>, now: Instant) -> LaunchingItem {
    let stages = flight
        .checklist(now)
        .into_iter()
        .map(|row| StageRow {
            label: row.stage.label().to_string(),
            time: row
                .seconds
                .filter(|_| row.state != RowState::Pending)
                .map(launch_progress::stage_time),
            state: match row.state {
                RowState::Done => StageState::Done,
                RowState::Current => StageState::Current,
                RowState::Pending => StageState::Pending,
                RowState::Failed => StageState::Failed,
                RowState::Stalled => StageState::Stalled,
            },
        })
        .collect();
    let notice = if let Some(body) = flight.failure_body() {
        Some(LaunchNotice::Failed {
            headline: format!("Couldn’t launch {}", flight.app.name),
            body: body.to_string(),
        })
    } else if flight.stalled(now) {
        let stage = flight.stage();
        Some(LaunchNotice::Stalled {
            message: stage.stall_message(flight.stage_minutes(now)),
            hint: stage.stall_hint().map(str::to_string),
            can_retry: flight.can_retry_same(),
        })
    } else {
        None
    };
    let status = if flight.failure_body().is_some() {
        "Launch failed".to_string()
    } else {
        format!(
            "Launching · {}",
            launch_progress::clock(flight.elapsed(now).as_secs())
        )
    };
    LaunchingItem {
        launch_id: flight.launch_id.clone(),
        app_name: flight.app.name.clone(),
        app_icon: icon.cloned(),
        stages,
        detail: Some(flight.detail_line()),
        status,
        notice,
    }
}

#[cfg(test)]
fn format_started_ago(created_at: f64) -> Option<String> {
    format_started_ago_at(created_at, unix_now())
}

/// "Started … ago" for a session created at `created_at` (Unix seconds), seen at `now`.
fn format_started_ago_at(created_at: f64, now: u64) -> Option<String> {
    if created_at <= 0.0 {
        return None;
    }
    let elapsed = now.saturating_sub(created_at as u64);
    let label = if elapsed < 60 {
        "Just started".to_string()
    } else if elapsed < 3600 {
        let mins = elapsed / 60;
        format!("Started {mins}m ago")
    } else if elapsed < 86_400 {
        let hours = elapsed / 3600;
        format!("Started {hours}h ago")
    } else {
        let days = elapsed / 86_400;
        if days == 1 {
            "Started 1 day ago".to_string()
        } else {
            format!("Started {days} days ago")
        }
    };
    Some(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_window_resolution_follows_the_display_backing_scale() {
        let viewport = size(px(800.), px(600.));
        let mode = DisplayMode::default();
        assert_eq!(
            mode.request(viewport, 2.).unwrap().resolution,
            Resolution(1600, 1200)
        );
        assert_eq!(
            mode.request(viewport, 1.).unwrap().resolution,
            Resolution(800, 600)
        );
        assert_eq!(
            DisplayMode::MatchWindow
                .request(viewport, 2.)
                .unwrap()
                .resolution,
            Resolution(800, 600)
        );
        let fixed = DisplayMode::Fixed(Resolution(1280, 720));
        assert_eq!(fixed.request(viewport, 2.), fixed.request(viewport, 1.));
    }

    #[test]
    fn format_started_ago_handles_zero_and_recent_timestamps() {
        assert_eq!(format_started_ago(0.0), None);
        assert_eq!(format_started_ago(-5.0), None);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as f64;
        assert_eq!(
            format_started_ago(now - 10.0).as_deref(),
            Some("Just started")
        );
        assert_eq!(
            format_started_ago(now - 300.0).as_deref(),
            Some("Started 5m ago")
        );
        assert_eq!(
            format_started_ago(now - 7200.0).as_deref(),
            Some("Started 2h ago")
        );
        assert_eq!(
            format_started_ago(now - 172_800.0).as_deref(),
            Some("Started 2 days ago")
        );
    }

    #[test]
    fn format_started_ago_at_ticks_by_minute() {
        let created = 1_000_000.0;
        let at = |elapsed: u64| format_started_ago_at(created, 1_000_000 + elapsed);
        assert_eq!(at(59).as_deref(), Some("Just started"));
        assert_eq!(at(60).as_deref(), Some("Started 1m ago"));
        assert_eq!(at(119).as_deref(), Some("Started 1m ago"));
        assert_eq!(at(120).as_deref(), Some("Started 2m ago"));
        assert_eq!(at(3_599).as_deref(), Some("Started 59m ago"));
        assert_eq!(at(3_600).as_deref(), Some("Started 1h ago"));
        assert_eq!(at(86_399).as_deref(), Some("Started 23h ago"));
        assert_eq!(at(86_400).as_deref(), Some("Started 1 day ago"));
        assert_eq!(at(172_799).as_deref(), Some("Started 1 day ago"));
        assert_eq!(at(172_800).as_deref(), Some("Started 2 days ago"));
        // Clock skew: a session "from the future" has just started.
        assert_eq!(
            format_started_ago_at(created, 999_000).as_deref(),
            Some("Just started")
        );
    }

    fn library_with_one_session() -> Library {
        Library {
            apps: Vec::new(),
            sessions: vec![serde_json::from_value(serde_json::json!({
                "session_id": "s1",
                "app_id": "zen",
                "app_name": "Zen",
                "session_url": "/s1/",
                "created_at": 1_000.0,
            }))
            .unwrap()],
            status: Default::default(),
            homes: Vec::new(),
        }
    }

    #[test]
    fn a_load_from_before_sign_out_is_dropped() {
        let gate = LoadGate(3);
        assert!(gate.admits(3));
        assert!(!gate.admits(4));
    }

    #[test]
    fn sign_out_removes_only_keys_skerry_saved() {
        // A config-file key reused by a retry keeps its source (and is removed).
        assert_eq!(
            SignedInKey::of(KeySource::ConfigFile, false),
            SignedInKey::Saved
        );
        assert_eq!(SignedInKey::of(KeySource::Typed, true), SignedInKey::Saving);
        assert_eq!(
            SignedInKey::of(KeySource::Typed, false),
            SignedInKey::Unsaved
        );
        let label = |key, in_config| SignedInKey::label(Some(key), move || in_config);
        assert_eq!(label(SignedInKey::Saving, false), KeySourceLabel::SavedFile);
        assert_eq!(
            label(SignedInKey::Unsaved, false),
            KeySourceLabel::ThisRunOnly
        );
        // Pasted and saved, then a Refused Refresh and a retry: the client still says
        // Typed, but config.toml holds the key, so Sign Out removes it.
        assert_eq!(label(SignedInKey::Unsaved, true), KeySourceLabel::SavedFile);
        // An environment or .env key is never removed, even if the file has the same key.
        assert_eq!(label(SignedInKey::DotEnv, true), KeySourceLabel::DotEnv);
        assert_eq!(
            label(SignedInKey::Environment, true),
            KeySourceLabel::Environment
        );
    }

    fn test_flight(app_id: &str) -> Flight {
        let app: RemoteApp =
            serde_json::from_value(serde_json::json!({"id": app_id, "name": app_id})).unwrap();
        Flight::new(
            app,
            LaunchParams::default(),
            None,
            HashSet::new(),
            Instant::now(),
            1_000.0,
        )
    }

    #[test]
    fn quick_launch_without_memory_opens_the_dialog() {
        assert_eq!(quick_launch_plan(None), QuickLaunch::Dialog);
        let memory = LaunchMemory {
            gpu: Some("intel-gpu".into()),
            ..LaunchMemory::default()
        };
        assert_eq!(
            quick_launch_plan(Some(&memory)),
            QuickLaunch::Launch(memory.clone())
        );
    }

    #[test]
    fn one_flight_per_app() {
        let mut flights = vec![test_flight("zen")];
        assert!(!can_start(&flights, "zen"));
        assert!(can_start(&flights, "firefox"));
        // A failed launch no longer holds the app.
        flights[0].outcome = Outcome::Failed(None);
        assert!(can_start(&flights, "zen"));
    }

    #[test]
    fn leave_server_drops_flights() {
        // `leave_server` clears the flights; their tasks then find nothing and end.
        let mut flights = vec![test_flight("zen")];
        let id = flights[0].launch_id.clone();
        flights.clear();
        assert!(!flights.iter().any(|flight| flight.launch_id == id));
    }

    #[test]
    fn toggles_mark_items_dirty() {
        let library = library_with_one_session();
        let none = HashMap::new();
        let mut prefs = Preferences::default();
        let mut cache = ItemCache::default();
        let (first, _) = cache.get(|| build_items(&library, &prefs, &none, &none, 1_000));
        organize(&mut prefs, &mut cache, &|p: &mut Preferences| {
            p.library.set_favorite("zen", true)
        });
        assert!(prefs.library.is_favorite("zen"));
        let (rebuilt, _) = cache.get(|| build_items(&library, &prefs, &none, &none, 1_000));
        assert!(!Rc::ptr_eq(&first, &rebuilt));
    }

    #[test]
    fn launching_cards_show_stages_and_the_stall_notice() {
        let flight = test_flight("firefox");
        let item = launching_item(&flight, None, Instant::now() + Duration::from_secs(16));
        assert_eq!(item.status, "Launching · 0:16");
        assert_eq!(item.stages[0].state, StageState::Current);
        assert_eq!(item.stages[0].time.as_deref(), Some("16 s"));
        assert_eq!(item.stages.last().unwrap().time, None);
        assert_eq!(item.detail.as_deref(), Some("Sending the launch request"));
        assert!(item.notice.is_none());
        let stalled = launching_item(&flight, None, Instant::now() + Duration::from_secs(200));
        assert!(matches!(
            stalled.notice,
            Some(LaunchNotice::Stalled {
                can_retry: false,
                ..
            })
        ));
        assert_eq!(lowercase_first("Started 12m ago"), "started 12m ago");
    }

    #[test]
    fn library_items_rebuild_only_when_dirty() {
        let library = library_with_one_session();
        let none = HashMap::new();
        let prefs = Preferences::default();
        let mut cache = ItemCache::default();
        let build = || build_items(&library, &prefs, &none, &none, 1_000);
        let (_, first) = cache.get(build);
        let (_, again) = cache.get(build);
        assert!(Rc::ptr_eq(&first, &again));
        cache.invalidate();
        let (_, rebuilt) = cache.get(build);
        assert!(!Rc::ptr_eq(&first, &rebuilt));
        assert_eq!(rebuilt[0].started_label.as_deref(), Some("Just started"));
    }
}
