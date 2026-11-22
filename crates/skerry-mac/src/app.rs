mod controls;
mod display;
mod files_model;
mod files_window;
mod promise;
mod session_window;

use crate::launch_options::{self, LaunchContext, LaunchSelection};
use crate::sealskin::{
    Credential, LaunchParams, Library, LibraryError, RemoteApp, RemoteSession, SealSkinClient,
};
use crate::settings::{self, Preferences};
use crate::transport::{DisplayRequest, Resolution};
use gpui_kit::component::{
    input::{InputEvent, InputState},
    select::SelectEvent,
    ActiveTheme as _, Theme,
};
use gpui_kit::{
    actions, div, point, prelude::*, px, size, App, Bounds, ClipboardItem, Context, Entity,
    FocusHandle, KeyBinding, KeyDownEvent, Menu, MenuItem, Pixels, RenderImage, Size,
    TitlebarOptions, Window, WindowAppearance, WindowBounds, WindowOptions,
};
use objc2_app_kit::{NSView, NSWindow, NSWindowButton, NSWindowStyleMask, NSWindowTitleVisibility};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSUserDefaults};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use session_window::{OpenSession, OpenWindows, SessionWindow};
use skerry_ui::{
    connect::{self, ConnectProps},
    launch_dialog::{
        self, LaunchDialogAction, LaunchDialogProps, LaunchDialogSelects, LaunchOption,
        LaunchOptions, LaunchSelect,
    },
    library::{self, AppItem, LibraryHandlers, LibraryProps, SessionItem},
    session,
};
use smallvec::smallvec;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
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
    rt: Handle,
    api: Option<SealSkinClient>,
    library: Option<Library>,
    preferences: Preferences,
    key_input: Entity<InputState>,
    search_input: Entity<InputState>,
    selected_filter: String,
    app_icons: HashMap<String, Arc<RenderImage>>,
    session_thumbnails: HashMap<String, Arc<RenderImage>>,
    launch_dialog: Option<(RemoteApp, LaunchDialogProps, LaunchDialogSelects)>,
    /// Every session locale with the Mac's name for it; built on the first Launch Dialog.
    language_options: Option<Vec<LaunchOption>>,
    pending_stop: Option<RemoteSession>,
    /// Images replaced or removed since the last render; their GPU textures are freed with
    /// `window.drop_image` on the next render (GPUI never frees them on drop).
    retired_images: Vec<Arc<RenderImage>>,
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
            rt,
            api: None,
            library: None,
            preferences: Preferences::default(),
            key_input,
            search_input,
            selected_filter: "All".into(),
            app_icons: HashMap::new(),
            session_thumbnails: HashMap::new(),
            launch_dialog: None,
            language_options: None,
            pending_stop: None,
            retired_images: Vec::new(),
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
                self.load(api, None, true, cx);
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
            Ok(api) => self.load(api, save.then_some(credential), true, cx),
            Err(error) => self.status = Some(error),
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(api) = self.api.clone() {
            self.load(api, None, false, cx);
        }
    }

    fn load(
        &mut self,
        api: SealSkinClient,
        save: Option<Credential>,
        connecting: bool,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
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
                this.busy = None;
                match result {
                    Ok(library) => {
                        let apps = library.apps.clone();
                        let sessions = library.sessions.clone();
                        if save.is_some() {
                            this.clear_key_input = true;
                        }
                        this.startup_credential = None;
                        this.retry_api = None;
                        // Launch options remembered per app live with the other preferences.
                        this.preferences = settings::load(api.server());
                        this.api = Some(api.clone());
                        this.library = Some(library);
                        this.status = save.and_then(|credential| credential.save().err());
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
                self.retired_images.push(image);
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
                            this.retired_images.push(image);
                            return;
                        }
                        if let Some(old) = this.app_icons.insert(app_id, image) {
                            this.retired_images.push(old);
                        }
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
                            this.retired_images.push(image);
                            return;
                        }
                        if let Some(old) = this.session_thumbnails.insert(session.session_id, image)
                        {
                            this.retired_images.push(old);
                        }
                        cx.notify();
                    });
                }
            })
            .detach();
        }
    }

    fn forget(&mut self, cx: &mut Context<Self>) {
        self.status = Credential::forget().err();
        self.startup_credential = None;
        self.retry_api = None;
        self.leave_server();
        cx.notify();
    }

    /// Returns to the Connect screen, dropping the Library and its textures.
    fn leave_server(&mut self) {
        self.api = None;
        self.library = None;
        self.media_generation += 1;
        self.retired_images
            .extend(self.app_icons.drain().map(|(_, image)| image));
        self.retired_images
            .extend(self.session_thumbnails.drain().map(|(_, image)| image));
        self.launch_dialog = None;
        self.pending_stop = None;
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
            favorite: false,
            hidden: false,
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
            show_organize: false,
            link_copied: false,
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
        // Take focus off the search field so Escape reaches the dialog.
        window.focus(&self.focus_handle, cx);
        cx.notify();
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
            LaunchDialogAction::ToggleFavorite => {
                if let Some((_, dialog, _)) = self.launch_dialog.as_mut() {
                    dialog.favorite = !dialog.favorite;
                }
            }
            LaunchDialogAction::ToggleHidden => {
                if let Some((_, dialog, _)) = self.launch_dialog.as_mut() {
                    dialog.hidden = !dialog.hidden;
                }
            }
            LaunchDialogAction::SetCollaborative(enabled) => {
                if let Some((_, dialog, _)) = self.launch_dialog.as_mut() {
                    dialog.collaborative = enabled;
                    dialog.link_copied = false;
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
                        dialog.link_copied = true;
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
                    self.launch_with_options(app, params, Some(selection), window, cx);
                }
            }
        }
        cx.notify();
    }

    /// Launches `app`; once it runs, `remember` becomes the app's remembered options.
    fn launch_with_options(
        &mut self,
        app: RemoteApp,
        params: LaunchParams,
        remember: Option<LaunchSelection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(api) = self.api.clone().filter(|_| self.busy.is_none()) else {
            return;
        };
        let app_id = app.id.clone();
        self.status = None;
        self.busy = Some(format!("Launching {}…", app.name));
        let pending = self
            .rt
            .spawn(async move { api.launch_with_options(app, params).await });
        // Resolve the Launcher window by handle: gpui-pre's `update_in` uses the entity's
        // *current* window, which is Session Controls whenever that panel last rendered the
        // Launcher (ARCH-015).
        let launcher_window = Window::window_handle(window);
        cx.spawn(async move |this, cx| {
            let result = pending
                .await
                .unwrap_or_else(|_| Err("The launch was interrupted.".into()));
            let _ = launcher_window.update(cx, |_, window, cx| {
                this.update(cx, |this, cx| {
                    this.busy = None;
                    match result {
                        Ok(session) => {
                            if let Some(library) = &mut this.library {
                                library.sessions.insert(0, session.clone());
                                if session.home.starts_with("auto-")
                                    && !library.homes.contains(&session.home)
                                {
                                    library.homes.push(session.home.clone());
                                }
                            }
                            if let Some(selection) = &remember {
                                this.remember_launch(&app_id, selection);
                            }
                            this.open_session(session, window, cx);
                        }
                        Err(error) => this.status = Some(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }

    /// Keeps `selection` as the options `app_id` opens with next time (saved per server).
    fn remember_launch(&mut self, app_id: &str, selection: &LaunchSelection) {
        let Some(api) = &self.api else {
            return;
        };
        let memory = selection.memory();
        self.preferences
            .launch
            .insert(app_id.to_string(), memory.clone());
        let app_id = app_id.to_string();
        // Re-read first so a session window's saved codec or input changes survive (ARCH-022).
        if settings::update(api.server(), |p| {
            p.launch.insert(app_id, memory);
        })
        .is_err()
        {
            self.status = Some("Couldn’t save this app’s launch options.".into());
        }
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
        cx.spawn(async move |this, cx| {
            let result = pending
                .await
                .unwrap_or_else(|_| Err("Stopping the session was interrupted.".into()));
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(()) => {
                        if let Some(library) = &mut this.library {
                            library
                                .sessions
                                .retain(|s| s.session_id != session.session_id);
                        }
                        if let Some(image) = this.session_thumbnails.remove(&session.session_id) {
                            this.retired_images.push(image);
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

        let props = LibraryProps {
            app_name: APP_NAME.to_string(),
            server: api.server().to_string(),
            apps: library
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
                    recent: false,
                    icon: self.app_icons.get(&app.id).cloned(),
                })
                .collect(),
            sessions: library
                .sessions
                .iter()
                .map(|session| SessionItem {
                    id: session.session_id.clone(),
                    app_name: session.app_name.clone(),
                    started_label: format_started_ago(session.created_at),
                    gpu: session.gpu,
                    // No home means a cleanroom session; label it so every card shows its
                    // storage and the cards line up.
                    storage_label: Some(if session.home.is_empty() {
                        "Cleanroom".to_string()
                    } else {
                        session.home.clone()
                    }),
                    thumbnail: self.session_thumbnails.get(&session.session_id).cloned(),
                    app_icon: self.app_icons.get(&session.app_id).cloned(),
                })
                .collect(),
            search_query: self.search_input.read(cx).value().to_string(),
            selected_filter: self.selected_filter.clone(),
            status: self.status.clone(),
            busy: self.busy.clone(),
        };

        let apps_by_id = library.apps.clone();
        let sessions_by_id = library.sessions.clone();
        let sessions_for_stop = library.sessions.clone();
        let launcher_for_launch = cx.entity().clone();
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
                    && (this.launch_dialog.is_some() || this.pending_stop.is_some())
                {
                    this.launch_dialog = None;
                    this.pending_stop = None;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(library::render(
                &theme,
                &props,
                &self.search_input,
                LibraryHandlers {
                    on_refresh: Box::new(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    on_forget: Box::new(cx.listener(|this, _, _, cx| this.forget(cx))),
                    on_launch: Rc::new(move |app_id, window, cx| {
                        if let Some(app) = apps_by_id.iter().find(|a| a.id == app_id).cloned() {
                            launcher_for_launch
                                .update(cx, |this, cx| this.open_launch_dialog(app, window, cx));
                        }
                    }),
                    on_resume: Rc::new(move |session_id, window, cx| {
                        if let Some(session) = sessions_by_id
                            .iter()
                            .find(|s| s.session_id == session_id)
                            .cloned()
                        {
                            launcher_for_resume
                                .update(cx, |this, cx| this.open_session(session, window, cx));
                        }
                    }),
                    on_stop: Some(Rc::new(move |session_id, window, cx| {
                        if let Some(session) = sessions_for_stop
                            .iter()
                            .find(|s| s.session_id == session_id)
                            .cloned()
                        {
                            launcher_for_stop.update(cx, |this, cx| {
                                this.pending_stop = Some(session);
                                window.focus(&this.focus_handle, cx);
                                cx.notify();
                            });
                        }
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

fn format_started_ago(created_at: f64) -> Option<String> {
    if created_at <= 0.0 {
        return None;
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
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
}
