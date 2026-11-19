mod controls;

use crate::api::{Api, Credential, Library, RemoteApp, RemoteSession};
use crate::settings::{
    self, Key, Preferences, ScreenChange, ScreenPolicy, Settings, VideoSettings,
};
use crate::transport::{DisplayRequest, Resolution, SessionEvent, SessionOptions, StreamSample};
use crate::video::Codec;
use crate::view::NativeSession;
use controls::ControlPanel;
use gpui::{
    actions, div, prelude::*, px, rgb, size, App, Application, Bounds, Context, Div, KeyBinding,
    Menu, MenuItem, SharedString, Stateful, TitlebarOptions, Window, WindowAppearance,
    WindowBounds, WindowHandle, WindowOptions,
};
use objc2_app_kit::{NSView, NSWindow, NSWindowButton, NSWindowStyleMask, NSWindowTitleVisibility};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::time::Duration;
use tokio::runtime::Handle;

const APP_NAME: &str = "SealSkin Native";

pub fn run() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("create SealSkin runtime");
    let handle = runtime.handle().clone();
    Application::new().run(move |cx: &mut App| {
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
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Session Controls", ToggleSessionControls),
                    MenuItem::action("Toggle Full Screen", ToggleFullScreen),
                ],
            },
        ]);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1080.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(640.), px(420.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(APP_NAME.into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
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
    fn request(
        self,
        viewport: gpui::Size<gpui::Pixels>,
        backing_scale: f32,
    ) -> Option<DisplayRequest> {
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
    stream_events: Option<gpui::Task<()>>,
    audio: Option<bool>,
    audio_unavailable: Option<&'static str>,
    muted: bool,
    video: Option<bool>,
    settings: Option<Settings>,
    controls: Option<WindowHandle<ControlPanel>>,
    stats: Option<StreamSample>,
    display_mode: DisplayMode,
    retina: bool,
    scale_locally: bool,
    screen_policy: ScreenPolicy,
    dpi_override: Option<u16>,
    applied_resolution: Option<DisplayRequest>,
    applied_dpi: Option<u16>,
    resize_allowed: bool,
    resize_task: Option<gpui::Task<()>>,
    preferences: Preferences,
    pending_settings: VideoSettings,
    settings_task: Option<gpui::Task<()>>,
    busy: Option<String>,
    status: Option<String>,
}

impl Launcher {
    fn new(rt: Handle, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            applied_resolution: None,
            applied_dpi: None,
            resize_allowed: false,
            resize_task: None,
            preferences: Preferences::default(),
            pending_settings: VideoSettings::new(),
            settings_task: None,
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
        cx.observe_window_appearance(window, |this, window, cx| {
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

    fn paste(&mut self, cx: &mut Context<Self>) {
        let result = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .ok_or_else(|| "Copy a SealSkin application key first.".to_string())
            .and_then(|text| Credential::parse(&text));
        match result {
            Ok(credential) => self.connect(credential, true, cx),
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
                        this.api = Some(api);
                        this.library = Some(library);
                        this.status = save.and_then(|credential| credential.save().err());
                    }
                    Err(error) => this.status = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn forget(&mut self, cx: &mut Context<Self>) {
        self.status = Credential::forget().err();
        self.api = None;
        self.library = None;
        cx.notify();
    }

    fn launch(&mut self, app: RemoteApp, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| self.busy.is_none()) else {
            return;
        };
        self.status = None;
        self.busy = Some(format!("Launching {}…", app.name));
        let pending = self.rt.spawn(async move { api.launch(app).await });
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
            None => self.screen_policy.display_dpi(scale),
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
            match ControlPanel::open(window, cx) {
                Ok(panel) => self.controls = Some(panel),
                Err(_) => self.status = Some("Couldn’t open Session Controls.".into()),
            }
            cx.notify();
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

fn native_window(window: &Window) -> Option<objc2::rc::Retained<NSWindow>> {
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

#[derive(Clone, Copy)]
struct Colors {
    background: u32,
    sidebar: u32,
    panel: u32,
    ink: u32,
    muted: u32,
    edge: u32,
    accent: u32,
}

impl Colors {
    fn for_window(window: &Window) -> Self {
        match window.appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self {
                background: 0x191c22,
                sidebar: 0x14171d,
                panel: 0x22262f,
                ink: 0xf1f3f6,
                muted: 0xa3acbb,
                edge: 0x343b48,
                accent: 0x346ce0,
            },
            _ => Self {
                background: 0xf5f6f9,
                sidebar: 0xedeff4,
                panel: 0xffffff,
                ink: 0x182238,
                muted: 0x56647a,
                edge: 0xdde2ec,
                accent: 0x2658cb,
            },
        }
    }

    fn button(
        self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        primary: bool,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex_none()
            .px_3()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(rgb(if primary { self.accent } else { self.edge }))
            .bg(rgb(if primary { self.accent } else { self.panel }))
            .text_color(rgb(if primary { 0xffffff } else { self.ink }))
            .text_sm()
            .cursor_pointer()
            .hover(move |style| style.border_color(rgb(self.muted)))
            .child(label.into())
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Colors::for_window(window);
        let root = div()
            .size_full()
            .bg(rgb(colors.background))
            .text_color(rgb(colors.ink));
        if let Some((_, stream)) = &self.current {
            return root
                .relative()
                .bg(rgb(0x000000))
                .on_action(cx.listener(|this, _: &ToggleSessionControls, window, cx| {
                    this.toggle_controls(window, cx);
                }))
                .on_action(cx.listener(|this, _: &ToggleFullScreen, window, cx| {
                    this.close_controls(window, cx);
                    window.toggle_fullscreen();
                }))
                .child(stream.view())
                .when_some(self.status.clone(), |root, status| {
                    root.child(
                        div()
                            .absolute()
                            .bottom_3()
                            .left_3()
                            .right_3()
                            .p_3()
                            .rounded_md()
                            .bg(rgb(colors.panel))
                            .child(status),
                    )
                });
        }
        let (Some(api), Some(library)) = (&self.api, &self.library) else {
            return root.flex().items_center().justify_center().child(
                div().w(px(416.)).p_6().rounded_xl().border_1().border_color(rgb(colors.edge)).bg(rgb(colors.panel)).flex().flex_col().gap_4()
                    .child(div().text_2xl().font_weight(gpui::FontWeight::SEMIBOLD).child("Connect to SealSkin"))
                    .child(div().text_sm().text_color(rgb(colors.muted)).child("Copy an application key from Settings → Application Keys on your SealSkin server. SealSkin stores it in macOS Keychain after connecting."))
                    .when_some(self.status.clone(), |view, status| view.child(status))
                    .when_some(self.busy.clone(), |view, busy| view.child(busy))
                    .when(self.busy.is_none(), |view| view.child(colors.button("paste", "Paste Application Key", true)
                        .on_click(cx.listener(|this, _, _, cx| this.paste(cx))))));
        };
        let sidebar = div()
            .w(px(222.))
            .flex_none()
            .h_full()
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(colors.sidebar))
            .border_r_1()
            .border_color(rgb(colors.edge))
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(APP_NAME),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(colors.muted))
                    .truncate()
                    .child(api.server().to_string()),
            )
            .child(div().flex_1())
            .when(self.busy.is_none(), |view| {
                view.child(
                    colors
                        .button("forget", "Remove Connection", false)
                        .on_click(cx.listener(|this, _, _, cx| this.forget(cx))),
                )
            });
        let enabled = self.busy.is_none();
        let content = div()
            .id("library")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p_6()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Applications"),
                    )
                    .when(enabled, |view| {
                        view.child(
                            colors
                                .button("refresh", "Refresh", false)
                                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                        )
                    }),
            )
            .when_some(self.busy.clone(), |view, busy| {
                view.child(div().text_sm().text_color(rgb(colors.muted)).child(busy))
            })
            .when_some(self.status.clone(), |view, status| {
                view.child(div().text_sm().child(status))
            })
            .when(library.apps.is_empty(), |view| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(rgb(colors.muted))
                        .child("No applications are available for this account."),
                )
            })
            .children(library.apps.iter().map(|app| {
                let app = app.clone();
                row(colors, app.name.clone()).when(enabled, |view| {
                    view.child(
                        colors
                            .button(
                                SharedString::from(format!("launch-{}", app.id)),
                                "Launch",
                                true,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.launch(app.clone(), window, cx)
                            })),
                    )
                })
            }))
            .child(
                div()
                    .mt_4()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Running Sessions"),
            )
            .when(library.sessions.is_empty(), |view| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(rgb(colors.muted))
                        .child("No running sessions."),
                )
            })
            .children(library.sessions.iter().map(|session| {
                let session = session.clone();
                row(colors, session.app_name.clone()).when(enabled, |view| {
                    view.child(
                        colors
                            .button(
                                SharedString::from(format!("resume-{}", session.session_id)),
                                "Resume",
                                false,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_session(session.clone(), window, cx)
                            })),
                    )
                })
            }));
        root.flex().child(sidebar).child(content)
    }
}

fn row(colors: Colors, name: String) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .px_4()
        .py_2()
        .min_h(px(48.))
        .rounded_lg()
        .border_1()
        .border_color(rgb(colors.edge))
        .bg(rgb(colors.panel))
        .child(div().min_w_0().truncate().child(name))
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
}
