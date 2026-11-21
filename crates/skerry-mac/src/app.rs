mod controls;
mod display;

use crate::api::{Api, Credential, LaunchParams, Library, RemoteApp, RemoteSession};
use crate::input::InputFeature;
use crate::settings::{
    self, Key, Preferences, ScreenChange, ScreenPolicy, Settings, VideoSettings,
};
use crate::transport::{DisplayRequest, Resolution, SessionEvent, SessionOptions, StreamSample};
use crate::video::Codec;
use crate::view::NativeSession;
use controls::ControlPanel;
use gpui_kit::component::{
    input::{InputEvent, InputState},
    select::SelectEvent,
    ActiveTheme as _, Theme,
};
use gpui_kit::{
    actions, div, prelude::*, px, rgb, size, AnyWindowHandle, App, Bounds, ClipboardItem, Context,
    Entity, KeyBinding, Menu, MenuItem, Pixels, RenderImage, Size, Task, TitlebarOptions, Window,
    WindowAppearance, WindowBounds, WindowOptions,
};
use objc2_app_kit::{NSView, NSWindow, NSWindowButton, NSWindowStyleMask, NSWindowTitleVisibility};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSUserDefaults};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use skerry_ui::{
    connect::{self, ConnectProps},
    launch_dialog::{
        self, LaunchDialogAction, LaunchDialogProps, LaunchDialogSelects, LaunchOption,
    },
    library::{self, AppItem, LibraryHandlers, LibraryProps, SessionItem},
    session,
};
use smallvec::smallvec;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
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
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-shift-m", ToggleSessionControls, None),
                KeyBinding::new("escape", CloseSessionControls, Some("SessionControls")),
                KeyBinding::new("cmd-w", CloseSessionControls, Some("SessionControls")),
                KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.set_menus(vec![
                Menu {
                    name: APP_NAME.into(),
                    items: vec![MenuItem::action("Quit SealSkin Native", Quit)],
                    disabled: false,
                },
                Menu {
                    name: "View".into(),
                    items: vec![
                        MenuItem::action("Session Controls", ToggleSessionControls),
                        MenuItem::action("Toggle Full Screen", ToggleFullScreen),
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
            let bounds = Bounds::centered(None, size(px(1080.), px(720.)), cx);
            gpui_kit::open_window(
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
                move |window, cx| cx.new(|cx| Launcher::new(handle.clone(), window, cx)),
            )
            .expect("open SealSkin window");
            cx.activate(true);
        });
}

actions!(
    sealskin,
    [
        Quit,
        ToggleSessionControls,
        CloseSessionControls,
        ToggleFullScreen
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
    api: Option<Api>,
    library: Option<Library>,
    current: Option<(RemoteSession, NativeSession)>,
    stream_events: Option<Task<()>>,
    audio: Option<bool>,
    audio_unavailable: Option<&'static str>,
    muted: bool,
    video: Option<bool>,
    settings: Option<Settings>,
    controls: Option<AnyWindowHandle>,
    stats: Option<StreamSample>,
    display_mode: DisplayMode,
    retina: bool,
    scale_locally: bool,
    screen_policy: ScreenPolicy,
    dpi_override: Option<u16>,
    ui_scale: display::UiScale,
    applied_resolution: Option<DisplayRequest>,
    applied_dpi: Option<u16>,
    resize_allowed: bool,
    resize_task: Option<Task<()>>,
    preferences: Preferences,
    pending_settings: VideoSettings,
    settings_task: Option<Task<()>>,
    key_input: Entity<InputState>,
    search_input: Entity<InputState>,
    selected_filter: String,
    app_icons: HashMap<String, Arc<RenderImage>>,
    session_thumbnails: HashMap<String, Arc<RenderImage>>,
    launch_dialog: Option<(RemoteApp, LaunchDialogProps, LaunchDialogSelects)>,
    pending_stop: Option<RemoteSession>,
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
            current: None,
            stream_events: None,
            audio: None,
            audio_unavailable: None,
            muted: false,
            video: None,
            settings: None,
            controls: None,
            stats: None,
            display_mode: DisplayMode::default(),
            retina: true,
            scale_locally: true,
            screen_policy: ScreenPolicy::default(),
            dpi_override: None,
            ui_scale: display::UiScale::default(),
            applied_resolution: None,
            applied_dpi: None,
            resize_allowed: false,
            resize_task: None,
            preferences: Preferences::default(),
            pending_settings: VideoSettings::new(),
            settings_task: None,
            key_input,
            search_input,
            selected_filter: "All".into(),
            app_icons: HashMap::new(),
            session_thumbnails: HashMap::new(),
            launch_dialog: None,
            pending_stop: None,
            busy: None,
            status: None,
        };
        let owner = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = owner.update(cx, |launcher, cx| launcher.remove_controls(cx));
            true
        });
        cx.observe_window_bounds(window, |this, window, cx| this.resize_display(window, cx))
            .detach();
        Theme::sync_system_appearance(Some(window), cx);
        cx.observe_window_appearance(window, |this, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
            if let Some((_, stream)) = &this.current {
                stream.set_appearance(dark_appearance(window));
            }
            cx.notify();
        })
        .detach();
        match Credential::startup() {
            Ok(Some(credential)) => launcher.connect(credential, false, cx),
            Ok(None) => {}
            Err(error) => launcher.status = Some(error),
        }
        launcher
    }

    fn submit_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let text = self.key_input.read(cx).value().to_string();
        let trimmed = text.trim();
        let result = if trimmed.is_empty() {
            Err("Enter a SealSkin application key first.".to_string())
        } else {
            Credential::parse(trimmed)
        };
        match result {
            Ok(credential) => {
                self.key_input
                    .update(cx, |state, cx| state.set_value("", window, cx));
                self.connect(credential, true, cx);
            }
            Err(error) => self.status = Some(error),
        }
        cx.notify();
    }

    fn connect(&mut self, credential: Credential, save: bool, cx: &mut Context<Self>) {
        match Api::new(&credential) {
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
        api: Api,
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
        cx.spawn(async move |this, cx| {
            let result = pending
                .await
                .unwrap_or_else(|_| Err("The connection was interrupted.".into()));
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(library) => {
                        let apps = library.apps.clone();
                        let sessions = library.sessions.clone();
                        this.api = Some(api.clone());
                        this.library = Some(library);
                        this.status = save.and_then(|credential| credential.save().err());
                        this.fetch_library_media(api, apps, sessions, cx);
                    }
                    Err(error) => this.status = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn fetch_library_media(
        &self,
        api: Api,
        apps: Vec<RemoteApp>,
        sessions: Vec<RemoteSession>,
        cx: &mut Context<Self>,
    ) {
        let missing_apps: Vec<RemoteApp> = apps
            .into_iter()
            .filter(|app| !self.app_icons.contains_key(&app.id))
            .collect();
        for app in missing_apps {
            let api = api.clone();
            let app_id = app.id.clone();
            let pending = self
                .rt
                .spawn(async move { api.app_icon(&app).await.and_then(|b| decode_image(&b)) });
            cx.spawn(async move |this, cx| {
                if let Ok(Some(image)) = pending.await {
                    let _ = this.update(cx, |this, cx| {
                        this.app_icons.insert(app_id, image);
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
                    .and_then(|b| decode_image(&b))
            });
            cx.spawn(async move |this, cx| {
                if let Ok(Some(image)) = pending.await {
                    let _ = this.update(cx, |this, cx| {
                        this.session_thumbnails.insert(session.session_id, image);
                        cx.notify();
                    });
                }
            })
            .detach();
        }
    }

    fn forget(&mut self, cx: &mut Context<Self>) {
        self.status = Credential::forget().err();
        self.api = None;
        self.library = None;
        self.app_icons.clear();
        self.session_thumbnails.clear();
        self.launch_dialog = None;
        self.pending_stop = None;
        cx.notify();
    }

    fn open_launch_dialog(&mut self, app: RemoteApp, window: &mut Window, cx: &mut Context<Self>) {
        let storage_options = if app.home_directories {
            vec![
                LaunchOption {
                    id: "auto".into(),
                    label: "Auto (per-app persistence)".into(),
                },
                LaunchOption {
                    id: "cleanroom".into(),
                    label: "Cleanroom (temporary)".into(),
                },
            ]
        } else {
            vec![LaunchOption {
                id: "cleanroom".into(),
                label: "Cleanroom (temporary)".into(),
            }]
        };
        let selected_storage = storage_options[0].id.clone();
        let language_options = vec![
            LaunchOption {
                id: "".into(),
                label: "Default (en_US)".into(),
            },
            LaunchOption {
                id: "zh_CN.UTF-8".into(),
                label: "简体中文 (zh_CN)".into(),
            },
            LaunchOption {
                id: "ja_JP.UTF-8".into(),
                label: "日本語 (ja_JP)".into(),
            },
            LaunchOption {
                id: "de_DE.UTF-8".into(),
                label: "Deutsch (de_DE)".into(),
            },
        ];
        let props = LaunchDialogProps {
            app_id: app.id.clone(),
            app_name: app.name.clone(),
            app_icon: self.app_icons.get(&app.id).cloned(),
            favorite: false,
            hidden: false,
            storage_options,
            selected_storage,
            gpu_options: vec![],
            selected_gpu: String::new(),
            language_options,
            selected_language: String::new(),
            collaborative: false,
            wayland: true,
            busy: false,
            show_copy_link: true,
        };
        let selects = LaunchDialogSelects::from_props(&props, window, cx);
        if let Some(storage) = &selects.storage {
            cx.subscribe_in(
                storage,
                window,
                |this, _, event: &SelectEvent<Vec<LaunchOption>>, _, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event {
                        if let Some((_, dialog, _)) = this.launch_dialog.as_mut() {
                            dialog.selected_storage = value.clone();
                        }
                        cx.notify();
                    }
                },
            )
            .detach();
        }
        if let Some(language) = &selects.language {
            cx.subscribe_in(
                language,
                window,
                |this, _, event: &SelectEvent<Vec<LaunchOption>>, _, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event {
                        if let Some((_, dialog, _)) = this.launch_dialog.as_mut() {
                            dialog.selected_language = value.clone();
                        }
                        cx.notify();
                    }
                },
            )
            .detach();
        }
        self.launch_dialog = Some((app, props, selects));
        cx.notify();
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
                }
            }
            LaunchDialogAction::SetWayland(enabled) => {
                if let Some((_, dialog, _)) = self.launch_dialog.as_mut() {
                    dialog.wayland = enabled;
                }
            }
            LaunchDialogAction::CopyLink => {
                if let (Some(api), Some((app, _, _))) = (&self.api, &self.launch_dialog) {
                    if let Some(link) = api.app_deep_link(&app.id) {
                        cx.write_to_clipboard(ClipboardItem::new_string(link));
                        self.status = Some(format!("Copied link for {}.", app.name));
                    }
                }
            }
            LaunchDialogAction::Launch => {
                if let Some((app, dialog, _)) = self.launch_dialog.take() {
                    let home_name = storage_home_name(&dialog.selected_storage);
                    let language =
                        (!dialog.selected_language.is_empty()).then_some(dialog.selected_language);
                    let params = LaunchParams {
                        home_name,
                        language,
                        wayland_mode: dialog.wayland,
                        launch_in_room_mode: dialog.collaborative,
                    };
                    self.launch_with_options(app, params, window, cx);
                }
            }
        }
        cx.notify();
    }

    fn launch_with_options(
        &mut self,
        app: RemoteApp,
        params: LaunchParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(api) = self.api.clone().filter(|_| self.busy.is_none()) else {
            return;
        };
        self.status = None;
        self.busy = Some(format!("Launching {}…", app.name));
        let pending = self
            .rt
            .spawn(async move { api.launch_with_options(app, params).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = pending
                .await
                .unwrap_or_else(|_| Err("The launch was interrupted.".into()));
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(session) => {
                        if let Some(library) = &mut this.library {
                            library.sessions.insert(0, session.clone());
                        }
                        this.open_session(session, window, cx);
                    }
                    Err(error) => this.status = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
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
                        this.session_thumbnails.remove(&session.session_id);
                    }
                    Err(error) => this.status = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open_session(
        &mut self,
        session: RemoteSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.display_mode = DisplayMode::default();
        self.retina = true;
        self.scale_locally = true;
        self.dpi_override = None;
        self.open(session, window, cx);
    }

    fn open(&mut self, session: RemoteSession, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = &self.api else {
            return;
        };
        self.preferences = settings::load(api.server());
        let preferences = self
            .preferences
            .video
            .iter()
            .map(|(&key, &value)| (key, value))
            .collect();
        let (events, mut incoming) = tokio::sync::mpsc::unbounded_channel();
        let stream = api.session_url(&session.session_url).map(|url| {
            NativeSession::open(
                url,
                window,
                cx,
                &self.rt,
                SessionOptions {
                    preferences,
                    dark: dark_appearance(window),
                    muted: self.muted,
                    codec: self.preferences.codec,
                    display: self
                        .display_mode
                        .request(window.viewport_size(), window.scale_factor()),
                },
                events,
            )
        });
        match stream {
            Ok(stream) => {
                self.audio = None;
                self.status = None;
                window.set_window_title(&format!("{} — {APP_NAME}", session.app_name));
                if !self.scale_locally {
                    stream.scale_locally(false, cx);
                }
                stream.set_input_settings(self.preferences.input, window, cx);
                self.current = Some((session, stream));
                session_chrome(window, false);
                self.stream_events = Some(cx.spawn_in(window, async move |this, cx| {
                    while let Some(event) = incoming.recv().await {
                        if this
                            .update_in(cx, |this, window, cx| {
                                match event {
                                    SessionEvent::Audio(enabled) => this.audio = Some(enabled),
                                    SessionEvent::AudioUnavailable(reason) => {
                                        this.audio_unavailable = Some(reason)
                                    }
                                    SessionEvent::Video(enabled) => this.video = Some(enabled),
                                    SessionEvent::Settings(settings) => {
                                        this.settings = Some(settings)
                                    }
                                    SessionEvent::Stats(sample) => this.stats = Some(sample),
                                    SessionEvent::FingerScroll(enabled) => {
                                        if let Some((_, stream)) = &this.current {
                                            stream.allow_finger_scroll(enabled, cx);
                                        }
                                    }
                                    SessionEvent::Cursor(cursor) => {
                                        if let Some((_, stream)) = &this.current {
                                            stream.set_cursor(cursor, window, cx);
                                        }
                                    }
                                    SessionEvent::ScreenPolicy(policy) => {
                                        this.screen_policy = policy;
                                        this.resize_display(window, cx);
                                    }
                                    SessionEvent::ResizeAllowed(allowed) => {
                                        if this.resize_allowed != allowed {
                                            this.resize_allowed = allowed;
                                            this.applied_resolution = None;
                                            this.resize_display(window, cx);
                                        }
                                    }
                                    SessionEvent::Error(error) => {
                                        this.resize_allowed = false;
                                        this.settings = None;
                                        this.audio = None;
                                        this.video = None;
                                        this.status = Some(error);
                                    }
                                }
                                cx.notify();
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
            }
            Err(error) => self.status = Some(error),
        }
        cx.notify();
    }

    fn reconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((session, _)) = &self.current {
            let session = session.clone();
            self.close_stream(window, cx);
            self.open(session, window, cx);
        }
    }

    fn change_codec(&mut self, codec: Codec, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .settings
            .as_ref()
            .is_some_and(|settings| settings.codecs.contains(&codec))
        {
            return;
        }
        let Some(api) = &self.api else { return };
        let previous = self.preferences.codec;
        self.preferences.codec = codec;
        if settings::save(api.server(), &self.preferences).is_err() {
            self.preferences.codec = previous;
            self.status = Some("Couldn’t save the codec preference.".into());
            cx.notify();
            return;
        }
        self.reconnect(window, cx);
    }

    fn change_input(
        &mut self,
        feature: InputFeature,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        feature.set(&mut self.preferences.input, enabled);
        if let Some((_, stream)) = &self.current {
            stream.set_input_settings(self.preferences.input, window, cx);
        }
        if let Some(api) = &self.api {
            if settings::save(api.server(), &self.preferences).is_err() {
                self.status = Some("Couldn’t save input preferences.".into());
            }
        }
        cx.notify();
    }

    fn change_setting(&mut self, key: Key, value: u32, cx: &mut Context<Self>) {
        let Some(value) = self
            .settings
            .as_ref()
            .and_then(|settings| settings.request(key, value))
        else {
            return;
        };
        self.preferences.video.insert(key, value);
        self.pending_settings.insert(key, value);
        let timer = cx.background_executor().timer(Duration::from_millis(180));
        self.settings_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                this.apply_settings();
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn apply_settings(&mut self) {
        if let Some((_, stream)) = &self.current {
            stream.set_settings(
                std::mem::take(&mut self.pending_settings)
                    .into_iter()
                    .collect(),
            );
        }
        if let Some(api) = &self.api {
            if settings::save(api.server(), &self.preferences).is_err() {
                self.status = Some("Couldn’t save video preferences.".into());
            }
        }
    }

    fn reset_settings(&mut self, cx: &mut Context<Self>) {
        let Some(settings) = &self.settings else {
            return;
        };
        self.settings_task = None;
        self.preferences.video.clear();
        self.pending_settings = Key::ALL
            .into_iter()
            .filter_map(|key| {
                let control = settings.get(key)?;
                settings
                    .request(key, control.initial)
                    .map(|value| (key, value))
            })
            .collect();
        self.apply_settings();
        cx.notify();
    }

    fn resize_display(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.current.is_none() {
            return;
        }
        let scale = window.scale_factor();
        let resolution = self
            .resize_allowed
            .then(|| self.display_mode.request(window.viewport_size(), scale))
            .flatten();
        let dpi = match self.dpi_override {
            Some(dpi) => self.screen_policy.dpi_choices.contains(&dpi).then_some(dpi),
            None if !self.screen_policy.dpi_choices.is_empty() => self
                .screen_policy
                .display_dpi(self.ui_scale.for_window(window, cx)),
            None => None,
        };
        if resolution == self.applied_resolution && dpi == self.applied_dpi {
            self.resize_task = None;
            return;
        }
        let timer = cx.background_executor().timer(Duration::from_millis(250));
        self.resize_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, _| {
                if let Some((_, stream)) = &this.current {
                    if resolution != this.applied_resolution {
                        if let Some(request) = resolution {
                            stream.set_resolution(request);
                        }
                        this.applied_resolution = resolution;
                    }
                    if dpi != this.applied_dpi {
                        if let Some(value) = dpi {
                            stream.set_screen(ScreenChange::Dpi(value));
                        }
                        this.applied_dpi = dpi;
                    }
                }
            });
        }));
    }

    fn toggle_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.controls.is_some() {
            self.close_controls(window, cx);
            return;
        }
        if self.current.is_some() {
            let display_mode = self.display_mode;
            let owner_window = Window::window_handle(window);
            let launcher = cx.entity();
            cx.defer(move |cx| {
                let _ = owner_window.update(cx, |_, window, cx| {
                    let result = ControlPanel::open(display_mode, window, launcher.clone(), cx);
                    launcher.update(cx, |launcher, cx| {
                        match result {
                            Ok(panel) => launcher.controls = Some(panel),
                            Err(_) => {
                                launcher.status = Some("Couldn’t open Session Controls.".into())
                            }
                        }
                        cx.notify();
                    });
                });
            });
        }
    }

    fn remove_controls(&mut self, cx: &mut App) -> bool {
        self.controls.take().is_some_and(|panel| {
            let _ = panel.update(cx, |_, panel, _| panel.remove_window());
            true
        })
    }

    fn close_controls(&mut self, window: &mut Window, cx: &mut App) {
        if self.remove_controls(cx) {
            window.activate_window();
            if let Some((_, stream)) = &self.current {
                stream.focus(window, cx);
            }
        }
    }

    fn close_stream(&mut self, window: &mut Window, cx: &mut App) {
        self.settings_task = None;
        if !self.pending_settings.is_empty() {
            self.apply_settings();
        }
        self.settings = None;
        self.screen_policy = ScreenPolicy::default();
        self.applied_resolution = None;
        self.applied_dpi = None;
        self.stats = None;
        self.resize_allowed = false;
        self.resize_task = None;
        self.close_controls(window, cx);
        self.stream_events = None;
        if let Some((_, stream)) = self.current.take() {
            stream.release_focus(window, cx);
        }
        self.audio = None;
        self.audio_unavailable = None;
        self.video = None;
        window.set_window_title(APP_NAME);
        session_chrome(window, true);
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

fn dark_appearance(window: &Window) -> bool {
    matches!(
        window.appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

impl Render for Launcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if let Some((_, stream)) = &self.current {
            return div()
                .size_full()
                .relative()
                .bg(rgb(0x000000))
                .text_color(theme.foreground)
                .on_action(cx.listener(|this, _: &ToggleSessionControls, window, cx| {
                    this.toggle_controls(window, cx);
                }))
                .on_action(cx.listener(|this, _: &ToggleFullScreen, window, cx| {
                    this.close_controls(window, cx);
                    window.toggle_fullscreen();
                }))
                .child(stream.view())
                .when_some(self.status.clone(), |root, status| {
                    root.child(session::status_banner(&theme, status))
                });
        }
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
                    storage_label: (!session.home.is_empty()).then(|| session.home.clone()),
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
                    on_stop: Some(Rc::new(move |session_id, _, cx| {
                        if let Some(session) = sessions_for_stop
                            .iter()
                            .find(|s| s.session_id == session_id)
                            .cloned()
                        {
                            launcher_for_stop.update(cx, |this, cx| {
                                this.pending_stop = Some(session);
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

fn storage_home_name(selected: &str) -> Option<String> {
    match selected {
        "auto" | "" => None,
        "cleanroom" => Some("cleanroom".to_string()),
        named => Some(named.to_string()),
    }
}

fn decode_image(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let mut pixels = image::load_from_memory(bytes).ok()?.into_rgba8();
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
    fn storage_home_name_maps_auto_cleanroom_and_named_homes() {
        assert_eq!(storage_home_name("auto"), None);
        assert_eq!(storage_home_name(""), None);
        assert_eq!(storage_home_name("cleanroom").as_deref(), Some("cleanroom"));
        assert_eq!(storage_home_name("work-home").as_deref(), Some("work-home"));
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
