//! One remote session in its own window (`ARCH-010`, `ARCH-022`).
//!
//! Each `SessionWindow` owns its stream, the server's settings, stats, display mode, and
//! its Session Controls. The Library (`Launcher`) only opens these windows and keeps track
//! of them through the [`OpenWindows`] global.

use super::controls::ControlPanel;
use super::files_window::{self, FilesWindow};
use super::{
    dark_appearance, display, session_chrome, DisconnectSession, DisplayMode, Launcher,
    ToggleFiles, ToggleFullScreen, ToggleSessionControls, APP_NAME,
};
use crate::audio::OutputStatus;
use crate::input::InputFeature;
use crate::persist::WriteQueue;
use crate::sealskin::{RemoteSession, SealSkinClient};
use crate::settings::{Key, Preferences, ScreenChange, ScreenPolicy, Settings, VideoSettings};
use crate::stats;
use crate::transport::{
    DisplayRequest, SessionEvent, SessionOptions, StreamInfo, StreamSample, KEEP_WAITING_GRACE,
    SOCKET_STARTUP_GRACE,
};
use crate::video::Codec;
use crate::view::NativeSession;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    div, prelude::*, rgb, AnyWindowHandle, App, Context, Entity, Global, ScrollHandle, Task,
    WeakEntity, Window,
};
use skerry_ui::session;
use std::collections::VecDeque;
use std::time::Duration;
use tokio::runtime::Handle;

/// The Library window and every open session window, kept app-wide so a reopened Library
/// still knows which sessions are already showing.
#[derive(Default)]
pub(super) struct OpenWindows {
    pub(super) library: Option<(AnyWindowHandle, WeakEntity<Launcher>)>,
    pub(super) sessions: Vec<OpenSession>,
}

impl Global for OpenWindows {}

pub(super) struct OpenSession {
    pub(super) window: AnyWindowHandle,
    pub(super) session_id: String,
    pub(super) entity: WeakEntity<SessionWindow>,
}

impl OpenWindows {
    /// The window already showing `session_id`, if any. Windows that have gone are dropped.
    pub(super) fn session_window(&mut self, session_id: &str) -> Option<AnyWindowHandle> {
        self.sessions.retain(|open| open.entity.upgrade().is_some());
        self.sessions
            .iter()
            .find(|open| open.session_id == session_id)
            .map(|open| open.window)
    }

    fn forget_session(&mut self, window: AnyWindowHandle) {
        self.sessions.retain(|open| open.window != window);
    }
}

/// Refreshes the Library (if it is open) so its session cards and thumbnails catch up.
fn refresh_library(cx: &mut App) {
    let library = cx
        .try_global::<OpenWindows>()
        .and_then(|windows| windows.library.as_ref())
        .map(|(_, launcher)| launcher.clone());
    if let Some(launcher) = library {
        let _ = launcher.update(cx, |launcher, cx| launcher.refresh(false, cx));
    }
}

pub(super) struct SessionWindow {
    /// This session's own window, for async updates (`ARCH-015`).
    window: AnyWindowHandle,
    rt: Handle,
    api: SealSkinClient,
    pub(super) remote: RemoteSession,
    pub(super) current: Option<NativeSession>,
    stream_events: Option<Task<()>>,
    pub(super) audio: Option<bool>,
    pub(super) audio_unavailable: Option<&'static str>,
    /// The output devices and where sound plays, from the audio thread.
    pub(super) audio_output: Option<OutputStatus>,
    pub(super) muted: bool,
    pub(super) video: Option<bool>,
    pub(super) settings: Option<Settings>,
    controls: Option<AnyWindowHandle>,
    /// Session Controls' scroll position, kept here so it survives closing and reopening
    /// the panel while this window stays open.
    pub(super) controls_scroll: ScrollHandle,
    pub(super) stats: Option<StreamSample>,
    /// Whether the Stats section is open in Session Controls, which is what
    /// subscribes the session to the server's `stream_stats` (`ARCH-019`).
    stats_open: bool,
    /// Every second since the Stats section opened, up to `stats::HISTORY_MAX`.
    pub(super) stats_history: VecDeque<StreamSample>,
    pub(super) stream_info: Option<StreamInfo>,
    pub(super) display_mode: DisplayMode,
    pub(super) retina: bool,
    pub(super) scale_locally: bool,
    pub(super) screen_policy: ScreenPolicy,
    pub(super) clipboard_policy: Option<crate::clipboard::Policy>,
    /// The session's Files window (`ARCH-023`).
    pub(super) files_window: Option<(AnyWindowHandle, Entity<FilesWindow>)>,
    /// The session's file routes and what the server allows (`ARCH-023`).
    file_access: Option<crate::selkies::SelkiesClient>,
    file_policy: Option<crate::selkies::Policy>,
    pub(super) dpi_override: Option<u16>,
    ui_scale: display::UiScale,
    pub(super) applied_resolution: Option<DisplayRequest>,
    applied_dpi: Option<u16>,
    pub(super) resize_allowed: bool,
    pub(super) resize_task: Option<Task<()>>,
    /// This window's copy of the server's preferences, read when it opened. Changes are
    /// saved for the next session; other open windows keep their own (`ARCH-022`).
    pub(super) preferences: Preferences,
    pub(super) pending_settings: VideoSettings,
    settings_task: Option<Task<()>>,
    status: Option<String>,
    /// The session hasn't answered within the start-up grace: the banner offers Keep
    /// Waiting and Retry. `true` once a Keep Waiting period has run out too (`ARCH-027`).
    startup_stalled: Option<bool>,
    /// The current stream retries start-up errors for 5 minutes (Keep Waiting).
    keep_waiting: bool,
    /// Last seen full-screen state; a change releases held keys (`ARCH-GAP-DC-09`).
    fullscreen: bool,
}

impl SessionWindow {
    pub(super) fn new(
        rt: Handle,
        api: SealSkinClient,
        remote: RemoteSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The queue's copy includes changes another window queued that aren't on disk yet.
        let preferences = cx.global::<WriteQueue>().read(api.server());
        let mut this = Self {
            window: Window::window_handle(window),
            rt,
            api,
            remote,
            current: None,
            stream_events: None,
            audio: None,
            audio_unavailable: None,
            audio_output: None,
            muted: false,
            video: None,
            settings: None,
            controls: None,
            controls_scroll: ScrollHandle::new(),
            stats: None,
            stats_open: false,
            stats_history: VecDeque::new(),
            stream_info: None,
            display_mode: DisplayMode::default(),
            retina: true,
            scale_locally: true,
            screen_policy: ScreenPolicy::default(),
            clipboard_policy: None,
            files_window: None,
            file_access: None,
            file_policy: None,
            dpi_override: None,
            ui_scale: display::UiScale::default(),
            applied_resolution: None,
            applied_dpi: None,
            resize_allowed: false,
            resize_task: None,
            preferences,
            pending_settings: VideoSettings::new(),
            settings_task: None,
            status: None,
            startup_stalled: None,
            keep_waiting: false,
            fullscreen: window.is_fullscreen(),
        };
        let owner = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = owner.update(cx, |this, cx| this.teardown(window, cx));
            true
        });
        cx.observe_window_bounds(window, |this, window, cx| {
            // Leaving full screen restores the standard title bar; hide it again so it doesn't
            // cover the top of the remote picture.
            session_chrome(window, false);
            if this.fullscreen != window.is_fullscreen() {
                this.fullscreen = window.is_fullscreen();
                this.release_keys(cx);
            }
            this.resize_display(window, cx)
        })
        .detach();
        cx.observe_window_appearance(window, |this, window, cx| {
            gpui_kit::component::Theme::sync_system_appearance(Some(window), cx);
            if let Some(stream) = &this.current {
                stream.set_appearance(dark_appearance(window));
            }
            cx.notify();
        })
        .detach();
        window.set_window_title(&format!("{} — {APP_NAME}", this.remote.app_name));
        session_chrome(window, false);
        this.open(window, cx);
        this
    }

    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let preferences = self
            .preferences
            .video
            .iter()
            .map(|(&key, &value)| (key, value))
            .collect();
        let (events, mut incoming) = tokio::sync::mpsc::unbounded_channel();
        let stream = self.api.session_url(&self.remote.session_url).map(|url| {
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
                    stats: self.stats_open,
                    startup_grace: if self.keep_waiting {
                        KEEP_WAITING_GRACE
                    } else {
                        SOCKET_STARTUP_GRACE
                    },
                    waiting: if self.keep_waiting {
                        format!("Waiting for {} to start…", self.remote.app_name)
                    } else {
                        format!("Connecting to {}…", self.remote.app_name)
                    },
                    audio_output: self.preferences.audio.output_device.clone(),
                    audio_bitrate: self.preferences.audio.bitrate,
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
                self.audio_output = None;
                self.status = None;
                // A new stream replaces whatever the start-up banner was about.
                self.startup_stalled = None;
                if !self.scale_locally {
                    stream.scale_locally(false, cx);
                }
                stream.set_input_settings(self.preferences.input, window, cx);
                stream.set_clipboard_settings(self.preferences.clipboard, window, cx);
                self.current = Some(stream);
                // Session events update this window's entity through this window's handle
                // (ARCH-015): Session Controls also renders it.
                let session_window = Window::window_handle(window);
                self.stream_events = Some(cx.spawn(async move |this, cx| {
                    while let Some(event) = incoming.recv().await {
                        if session_window
                            .update(cx, |_, window, cx| {
                                this.update(cx, |this, cx| {
                                    this.handle_event(event, window, cx);
                                    cx.notify();
                                })
                            })
                            .map_or(true, |updated| updated.is_err())
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

    fn handle_event(&mut self, event: SessionEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            SessionEvent::Audio(enabled) => self.audio = Some(enabled),
            SessionEvent::AudioUnavailable(reason) => self.audio_unavailable = Some(reason),
            // Audio starts only after the settings arrive, so a status while they are
            // unknown is a late one from a stream that already ended (an error).
            SessionEvent::AudioOutput(status) if self.settings.is_some() => {
                self.remember_output_name(&status, cx);
                self.audio_output = Some(status);
            }
            SessionEvent::AudioOutput(_) => {}
            SessionEvent::Video(enabled) => self.video = Some(enabled),
            SessionEvent::Settings(settings) => {
                // The session answered: later reconnects get the normal 45 s grace.
                self.keep_waiting = false;
                self.settings = Some(settings);
            }
            SessionEvent::Stats(sample) => {
                if self.stats_open {
                    if self.stats_history.len() == stats::HISTORY_MAX {
                        self.stats_history.pop_front();
                    }
                    self.stats_history.push_back(sample.clone());
                }
                self.stats = Some(sample);
            }
            SessionEvent::StreamInfo(info) => self.stream_info = Some(*info),
            SessionEvent::FingerScroll(enabled) => {
                if let Some(stream) = &self.current {
                    stream.allow_finger_scroll(enabled, cx);
                }
            }
            SessionEvent::Cursor(cursor) => {
                if let Some(stream) = &self.current {
                    stream.set_cursor(cursor, window, cx);
                }
            }
            SessionEvent::ScreenPolicy(policy) => {
                self.screen_policy = policy;
                self.resize_display(window, cx);
            }
            SessionEvent::ResizeAllowed(allowed) => {
                if self.resize_allowed != allowed {
                    self.resize_allowed = allowed;
                    self.applied_resolution = None;
                    self.resize_display(window, cx);
                }
            }
            SessionEvent::ClipboardPolicy(policy) => {
                self.clipboard_policy = Some(policy);
                if let Some(stream) = &self.current {
                    stream.set_clipboard_policy(policy, window, cx);
                }
            }
            SessionEvent::Files(access) => self.file_access = Some(access),
            SessionEvent::FilePolicy(policy) => {
                self.file_policy = Some(policy);
                if let Some((_, files)) = &self.files_window {
                    files.update(cx, |files, cx| files.set_policy(policy, cx));
                }
            }
            SessionEvent::Clipboard(incoming) => {
                if let Some(stream) = &self.current {
                    stream.receive_clipboard(incoming, cx);
                }
            }
            SessionEvent::StartupTimeout => {
                self.resize_allowed = false;
                self.settings = None;
                self.audio = None;
                self.video = None;
                self.startup_stalled = Some(self.keep_waiting);
            }
            SessionEvent::Error(error) => {
                self.resize_allowed = false;
                self.settings = None;
                self.audio = None;
                self.audio_output = None;
                self.video = None;
                self.status = Some(error);
            }
        }
    }

    /// After the start-up banner: Keep Waiting retries for 5 more minutes behind "Waiting
    /// for {App} to start…"; Retry starts a fresh 45 s window.
    fn retry_startup(&mut self, keep_waiting: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.startup_stalled = None;
        self.end_stream(window, cx);
        self.keep_waiting = keep_waiting;
        self.open(window, cx);
        cx.notify();
    }

    pub(super) fn reconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Only the start-up banner's Keep Waiting extends the grace.
        self.keep_waiting = false;
        // Keep Session Controls open across codec / HiDPI reconnects; it re-renders from
        // the new stream's settings once they arrive.
        self.end_stream(window, cx);
        self.open(window, cx);
    }

    /// Queues one preference change for the next session on this server. The queue re-reads
    /// the file first, so another window's changes to other preferences survive
    /// (`ARCH-022`). The change already applies here; a failed save shows in the banner.
    fn save(
        &self,
        what: &'static str,
        change: impl Fn(&mut Preferences) + Send + Sync + 'static,
        cx: &mut Context<Self>,
    ) {
        let done = cx
            .global::<WriteQueue>()
            .preferences(self.api.server(), change);
        let window = self.window;
        cx.spawn(async move |this, cx| {
            if !matches!(done.await, Ok(Ok(()))) {
                let _ = window.update(cx, |_, _, cx| {
                    this.update(cx, |this, cx| {
                        this.status = Some(format!("Couldn’t save {what}."));
                        cx.notify();
                    })
                });
            }
        })
        .detach();
    }

    pub(super) fn change_codec(
        &mut self,
        codec: Codec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .settings
            .as_ref()
            .is_some_and(|settings| settings.codecs.contains(&codec))
        {
            return;
        }
        self.save("the codec preference", move |p| p.codec = codec, cx);
        self.preferences.codec = codec;
        self.reconnect(window, cx);
    }

    pub(super) fn change_input(
        &mut self,
        feature: InputFeature,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        feature.set(&mut self.preferences.input, enabled);
        if let Some(stream) = &self.current {
            stream.set_input_settings(self.preferences.input, window, cx);
        }
        let input = self.preferences.input;
        self.save("input preferences", move |p| p.input = input, cx);
        cx.notify();
    }

    pub(super) fn change_clipboard(
        &mut self,
        action: crate::clipboard::ClipboardAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::clipboard::ClipboardAction;
        let Some(stream) = &self.current else {
            return;
        };
        match action {
            ClipboardAction::SendNow => stream.send_clipboard(cx),
            ClipboardAction::CopyNow => stream.copy_session_clipboard(cx),
            _ => {
                self.preferences.clipboard.apply(action);
                stream.set_clipboard_settings(self.preferences.clipboard, window, cx);
                let clipboard = self.preferences.clipboard;
                self.save(
                    "clipboard preferences",
                    move |p| p.clipboard = clipboard,
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// Plays through `uid` (`None` follows the Mac) and saves the choice with the device's
    /// name, to label it while it isn't connected.
    pub(super) fn change_audio_output(&mut self, uid: Option<String>, cx: &mut Context<Self>) {
        let name = uid.as_deref().and_then(|uid| {
            self.audio_output
                .as_ref()
                .and_then(|status| status.device(uid))
                .map(|device| device.name.clone())
        });
        let audio = &mut self.preferences.audio;
        audio.output_device = uid.clone();
        audio.output_device_name = name;
        let (device, name) = (
            audio.output_device.clone(),
            audio.output_device_name.clone(),
        );
        self.save(
            "the output device",
            move |p| {
                p.audio.output_device = device.clone();
                p.audio.output_device_name = name.clone();
            },
            cx,
        );
        if let Some(stream) = &self.current {
            stream.set_audio_output(uid);
        }
        cx.notify();
    }

    /// Asks the server for `bps` and saves it for the next session on this server.
    pub(super) fn change_audio_bitrate(&mut self, bps: u32, cx: &mut Context<Self>) {
        if self
            .settings
            .as_ref()
            .and_then(|settings| settings.audio_bitrate.as_ref())
            .and_then(|bitrate| bitrate.request(bps))
            .is_none()
        {
            return;
        }
        self.preferences.audio.bitrate = Some(bps);
        self.save(
            "the audio bitrate",
            move |p| p.audio.bitrate = Some(bps),
            cx,
        );
        if let Some(stream) = &self.current {
            stream.set_audio_bitrate(bps);
        }
        cx.notify();
    }

    /// Keeps the chosen device's latest name, saving only when it changed so status churn
    /// never queues writes.
    fn remember_output_name(&mut self, status: &OutputStatus, cx: &mut Context<Self>) {
        let audio = &self.preferences.audio;
        let Some(name) = output_name_to_save(
            audio.output_device.as_deref(),
            audio.output_device_name.as_deref(),
            status,
        ) else {
            return;
        };
        self.preferences.audio.output_device_name = Some(name.clone());
        self.save(
            "the output device",
            move |p| p.audio.output_device_name = Some(name.clone()),
            cx,
        );
    }

    pub(super) fn change_setting(&mut self, key: Key, value: u32, cx: &mut Context<Self>) {
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
                this.apply_settings(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Sends the pending video settings and saves those the person chose.
    fn apply_settings(&mut self, cx: &mut Context<Self>) {
        let changes = std::mem::take(&mut self.pending_settings);
        if let Some(stream) = &self.current {
            stream.set_settings(changes.iter().map(|(&key, &value)| (key, value)).collect());
        }
        let chosen: Vec<(Key, u32)> = changes
            .keys()
            .filter_map(|key| self.preferences.video.get(key).map(|&value| (*key, value)))
            .collect();
        if !chosen.is_empty() {
            self.save(
                "video preferences",
                move |p| p.video.extend(chosen.iter().copied()),
                cx,
            );
        }
    }

    pub(super) fn reset_settings(&mut self, cx: &mut Context<Self>) {
        let Some(settings) = &self.settings else {
            return;
        };
        self.settings_task = None;
        self.pending_settings = Key::ALL
            .into_iter()
            .filter_map(|key| {
                let control = settings.get(key)?;
                settings
                    .request(key, control.initial)
                    .map(|value| (key, value))
            })
            .collect();
        self.preferences.video.clear();
        self.save("video preferences", |p| p.video.clear(), cx);
        self.apply_settings(cx);
        cx.notify();
    }

    pub(super) fn resize_display(&mut self, window: &Window, cx: &mut Context<Self>) {
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
                if let Some(stream) = &this.current {
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

    /// The server this session runs on.
    pub(super) fn server(&self) -> &str {
        self.api.server()
    }

    /// Shows `status` in this window's banner.
    pub(super) fn set_status(&mut self, status: String) {
        self.status = Some(status);
    }

    /// Lifts every key the session holds, before something else takes the keyboard.
    fn release_keys(&self, cx: &mut App) {
        if let Some(stream) = &self.current {
            stream.release_keys(cx);
        }
    }

    fn toggle_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.controls.is_some() {
            self.close_controls(window, cx);
            return;
        }
        if self.current.is_some() {
            // The panel may open without taking focus while a key is held (cmd-shift-m).
            self.release_keys(cx);
            let display_mode = self.display_mode;
            let owner_window = Window::window_handle(window);
            let session = cx.entity();
            // Opened outside this entity's update: the panel's first render reads it.
            cx.defer(move |cx| {
                let _ = owner_window.update(cx, |_, window, cx| {
                    let result = ControlPanel::open(display_mode, window, session.clone(), cx);
                    session.update(cx, |session, cx| {
                        match result {
                            Ok(panel) => session.controls = Some(panel),
                            Err(_) => {
                                session.status = Some("Couldn’t open Session Controls.".into())
                            }
                        }
                        cx.notify();
                    });
                });
            });
        }
    }

    /// Opens the session's Files window, or brings it forward (`ARCH-023`).
    pub(super) fn toggle_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((handle, _)) = &self.files_window {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
            self.files_window = None;
        }
        if self.current.is_none() {
            return;
        }
        let (Some(access), Some(policy)) = (self.file_access.clone(), self.file_policy) else {
            self.status =
                Some("Session files are available once the session has connected.".into());
            cx.notify();
            return;
        };
        if !policy.upload && !policy.download {
            self.status = Some("The server doesn’t allow file transfers in this session.".into());
            cx.notify();
            return;
        }
        self.release_keys(cx);
        let title = self.remote.app_name.clone();
        let storage =
            crate::sealskin::has_storage(&self.remote.home).then(|| files_window::Storage {
                client: self.api.clone(),
                home: self.remote.home.clone(),
            });
        let rt = self.rt.clone();
        let owner_window = Window::window_handle(window);
        let session = cx.entity();
        cx.defer(move |cx| {
            let _ = owner_window.update(cx, |_, window, cx| {
                let source = files_window::FilesSource {
                    title,
                    selkies: access,
                    policy,
                    storage,
                };
                let result = FilesWindow::open(source, rt, window, session.clone(), cx);
                session.update(cx, |session, cx| {
                    match result {
                        Ok(files) => session.files_window = Some(files),
                        Err(_) => session.status = Some("Couldn’t open Files.".into()),
                    }
                    cx.notify();
                });
            });
        });
    }

    fn close_files(&mut self, cx: &mut App) {
        if let Some((handle, _)) = self.files_window.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    /// The Stats section opened or shut: a fresh history, and the session
    /// subscribes to the server's figures only while it is open.
    pub(super) fn set_stats_open(&mut self, open: bool) {
        if open == self.stats_open {
            return;
        }
        self.stats_open = open;
        self.stats_history.clear();
        if let Some(stream) = &self.current {
            stream.set_stats(open);
        }
    }

    fn remove_controls(&mut self, cx: &mut App) -> bool {
        self.set_stats_open(false);
        self.controls.take().is_some_and(|panel| {
            let _ = panel.update(cx, |_, panel, _| panel.remove_window());
            true
        })
    }

    pub(super) fn close_controls(&mut self, window: &mut Window, cx: &mut App) {
        if self.remove_controls(cx) {
            window.activate_window();
            if let Some(stream) = &self.current {
                stream.focus(window, cx);
            }
        }
    }

    /// Ends the stream; the window and Session Controls stay for a reconnect.
    fn end_stream(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_task = None;
        if !self.pending_settings.is_empty() {
            self.apply_settings(cx);
        }
        self.settings = None;
        self.screen_policy = ScreenPolicy::default();
        self.clipboard_policy = None;
        self.file_access = None;
        self.file_policy = None;
        self.close_files(cx);
        self.applied_resolution = None;
        self.applied_dpi = None;
        self.stats = None;
        self.stats_history.clear();
        self.stream_info = None;
        self.resize_allowed = false;
        self.resize_task = None;
        self.stream_events = None;
        // `release_focus` queues `ku` for held keys; dropping the stream then queues
        // `Input::Close` behind them, so they reach the server before the close frame
        // (`ARCH-GAP-DC-09`).
        if let Some(stream) = self.current.take() {
            stream.release_focus(window, cx);
        }
        self.audio = None;
        self.audio_unavailable = None;
        self.audio_output = None;
        self.video = None;
    }

    /// The window is going away: end only this session's stream (never stop the remote
    /// app), close its Session Controls, and let the Library catch up (`ARCH-022`).
    fn teardown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.remove_controls(cx);
        self.end_stream(window, cx);
        let handle = Window::window_handle(window);
        if cx.has_global::<OpenWindows>() {
            cx.global_mut::<OpenWindows>().forget_session(handle);
        }
        cx.defer(refresh_library);
    }

    /// Disconnects and closes this window. The remote session keeps running.
    pub(super) fn disconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.teardown(window, cx);
        window.remove_window();
    }
}

/// The chosen device's name to save: when the status answers that choice, lists the device,
/// and its name differs from the saved one.
fn output_name_to_save(
    chosen: Option<&str>,
    saved: Option<&str>,
    status: &OutputStatus,
) -> Option<String> {
    let chosen = chosen?;
    if status.chosen.as_deref() != Some(chosen) {
        return None;
    }
    let name = &status.device(chosen)?.name;
    (saved != Some(name.as_str())).then(|| name.clone())
}

impl Render for SessionWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .size_full()
            .relative()
            .bg(rgb(0x000000))
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &ToggleSessionControls, window, cx| {
                this.toggle_controls(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleFiles, window, cx| {
                this.toggle_files(window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &DisconnectSession, window, cx| this.disconnect(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleFullScreen, window, cx| {
                this.close_controls(window, cx);
                this.release_keys(cx);
                window.toggle_fullscreen();
            }))
            .when_some(self.current.as_ref(), |root, stream| {
                root.child(stream.view())
            })
            .when_some(self.status.clone(), |root, status| {
                root.child(session::status_banner_with_actions(
                    &theme,
                    status,
                    cx.listener(|this, _, window, cx| {
                        this.status = None;
                        this.keep_waiting = false;
                        this.reconnect(window, cx);
                        cx.notify();
                    }),
                    cx.listener(|this, _, window, cx| this.disconnect(window, cx)),
                ))
            })
            .when_some(
                self.startup_stalled.filter(|_| self.status.is_none()),
                |root, after_waiting| {
                    let app = &self.remote.app_name;
                    let message = if after_waiting {
                        format!("{app} still hasn’t answered after 5 more minutes.")
                    } else {
                        format!("{app} hasn’t answered yet. It may still be starting.")
                    };
                    root.child(session::startup_banner(
                        &theme,
                        message,
                        cx.listener(|this, _, window, cx| this.retry_startup(true, window, cx)),
                        cx.listener(|this, _, window, cx| this.retry_startup(false, window, cx)),
                        cx.listener(|this, _, window, cx| this.disconnect(window, cx)),
                    ))
                },
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::OutputDevice;

    #[test]
    fn the_chosen_name_is_saved_only_when_it_changes() {
        let status = OutputStatus {
            devices: vec![OutputDevice {
                uid: "AirPods".into(),
                name: "Office AirPods".into(),
            }],
            chosen: Some("AirPods".into()),
            ..OutputStatus::default()
        };
        assert_eq!(
            output_name_to_save(Some("AirPods"), Some("AirPods Pro"), &status).as_deref(),
            Some("Office AirPods")
        );
        assert_eq!(
            output_name_to_save(Some("AirPods"), Some("Office AirPods"), &status),
            None
        );
        assert_eq!(output_name_to_save(None, None, &status), None);
        // Not connected: keep the last-seen name.
        assert_eq!(
            output_name_to_save(Some("USB"), Some("USB DAC"), &status),
            None
        );
        // A status for an older choice says nothing about this one.
        let stale = OutputStatus {
            chosen: None,
            ..status
        };
        assert_eq!(output_name_to_save(Some("AirPods"), None, &stale), None);
    }
}
