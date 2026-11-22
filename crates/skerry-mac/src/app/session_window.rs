//! One remote session in its own window (`ARCH-010`, `ARCH-022`).
//!
//! Each `SessionWindow` owns its stream, the server's settings, stats, display mode, and
//! its Session Controls. The Library (`Launcher`) only opens these windows and keeps track
//! of them through the [`OpenWindows`] global.

use super::controls::ControlPanel;
use super::files_window::{self, FilesOwner, FilesWindow};
use super::gamepads::Gamepads;
use super::{
    dark_appearance, display, session_chrome, DisconnectSession, DisplayMode, Launcher,
    ToggleFiles, ToggleFullScreen, TogglePointerCapture, ToggleSessionControls, ToggleStatsStrip,
    APP_NAME,
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
use crate::view::{CaptureHint, NativeSession};
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
    /// Every Files window, whoever opened it: one per session (`ARCH-023`, `ARCH-028`).
    pub(super) files: Vec<OpenFiles>,
}

impl Global for OpenWindows {}

pub(super) struct OpenSession {
    pub(super) window: AnyWindowHandle,
    pub(super) session_id: String,
    pub(super) entity: WeakEntity<SessionWindow>,
}

pub(super) struct OpenFiles {
    pub(super) session_id: String,
    pub(super) window: AnyWindowHandle,
    pub(super) entity: WeakEntity<FilesWindow>,
    /// Opened from the Library's Files button, with no stream.
    pub(super) library_owned: bool,
}

impl OpenWindows {
    /// The Files window already open for `session_id`, if any. Windows that have gone are
    /// dropped.
    pub(super) fn files_window(
        &mut self,
        session_id: &str,
    ) -> Option<(AnyWindowHandle, WeakEntity<FilesWindow>)> {
        self.files.retain(|open| open.entity.upgrade().is_some());
        self.files
            .iter()
            .find(|open| open.session_id == session_id)
            .map(|open| (open.window, open.entity.clone()))
    }

    pub(super) fn forget_files(&mut self, window: AnyWindowHandle) {
        self.files.retain(|open| open.window != window);
    }

    /// The Library-opened Files windows of the sessions `matches` picks, taken out of the
    /// registry so the caller can close them.
    pub(super) fn take_library_files(
        &mut self,
        matches: impl Fn(&str) -> bool,
    ) -> Vec<AnyWindowHandle> {
        let mut taken = Vec::new();
        self.files.retain(|open| {
            let take = open.library_owned && matches(&open.session_id);
            if take {
                taken.push(open.window);
            }
            !take
        });
        taken
    }

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
    /// The Stats section is open, or the strip is shown: either subscribes the session to
    /// the server's `stream_stats` (`ARCH-019`, `ARCH-031`).
    stats_interest: stats::StatsInterest,
    /// The server's last `keyboard_shortcuts`, kept across stream ends and reconnects so a
    /// lock holds until the next `server_settings` (`ARCH-031`).
    shortcut_policy: Option<crate::shortcuts::ShortcutPolicy>,
    /// Every second since the subscription started (on this stream), up to
    /// `stats::HISTORY_MAX`.
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
    /// Capture Pointer was asked for from Session Controls or the menu while another
    /// window was key: it engages when this window activates (`ARCH-032`). Any
    /// deactivation clears it.
    pending_capture: bool,
    /// The capture hint over the picture: which, its generation, and whether it is fading.
    capture_hint: Option<(CaptureHint, u64, bool)>,
    hint_generation: u64,
    hint_task: Option<Task<()>>,
    /// What this stream told the server about its gamepad slots (`ARCH-032`).
    gamepad_link: crate::gamepad::Link,
    /// This window is key: only then does it send pads, or capture the pointer.
    window_active: bool,
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
            stats_interest: stats::StatsInterest::new(preferences.stats_strip),
            shortcut_policy: None,
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
            pending_capture: false,
            capture_hint: None,
            hint_generation: 0,
            hint_task: None,
            gamepad_link: crate::gamepad::Link::default(),
            window_active: window.is_window_active(),
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
        cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            this.window_active = active;
            // Pads follow the key window: the global only records it (this window holds its
            // own lease), this window syncs itself, and the one that lost key is synced
            // after this update (`ARCH-015`).
            let handle = Window::window_handle(window);
            let entity = cx.weak_entity();
            let lost = cx
                .try_global::<Gamepads>()
                .is_some()
                .then(|| {
                    cx.update_global::<Gamepads, _>(|gamepads, _| {
                        gamepads.set_active(handle, entity, active)
                    })
                })
                .flatten();
            this.sync_gamepads(cx);
            if let Some((other, session)) = lost {
                cx.defer(move |cx| {
                    let _ = other.update(cx, |_, _, cx| {
                        session.update(cx, |session, cx| session.sync_gamepads(cx))
                    });
                });
            }
            if active {
                if std::mem::take(&mut this.pending_capture) {
                    if let Some(stream) = &this.current {
                        stream.engage_capture(window, cx);
                    }
                }
            } else {
                this.pending_capture = false;
            }
            cx.notify();
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
                    stats: self.stats_interest.wanted(),
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
                // The saved choice until the server's settings arrive (a lock may follow).
                stream.set_mac_shortcuts(self.mac_shortcuts(), cx);
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
                self.shortcut_policy = Some(settings.keyboard_shortcuts);
                self.settings = Some(settings);
                self.sync_gamepads(cx);
                // The server's keyboard_shortcuts, possibly locked (`ARCH-031`).
                if let Some(stream) = &self.current {
                    stream.set_mac_shortcuts(self.mac_shortcuts(), cx);
                }
            }
            SessionEvent::Stats(sample) => {
                if self.stats_interest.wanted() {
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
                // Whichever Files window shows this session, a Library-opened one too.
                let files = cx
                    .default_global::<OpenWindows>()
                    .files_window(&self.remote.session_id)
                    .and_then(|(_, files)| files.upgrade());
                if let Some(files) = files {
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
            SessionEvent::Rumble(rumble) => {
                if self.window_active && self.gamepad_input() && self.preferences.gamepads.rumble {
                    if let Some(gamepads) = cx.try_global::<Gamepads>() {
                        gamepads.rumble(&rumble);
                    }
                }
            }
            SessionEvent::PointerCapture(hint) => {
                if let Some(hint) = hint {
                    self.show_capture_hint(hint, cx);
                }
            }
            SessionEvent::Error(error) => {
                // The capture ends with the stream; the banner explains, so no hint.
                if let Some(stream) = &self.current {
                    stream.release_capture(cx);
                }
                self.capture_hint = None;
                self.hint_task = None;
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

    /// ⌃⌥⇧Z, View → Capture Pointer, or Session Controls' button (`ARCH-032`). In the key
    /// session window it toggles at once; otherwise this window is activated and captures
    /// once it is key.
    pub(super) fn request_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stream) = &self.current else {
            return;
        };
        if window.is_window_active() {
            stream.toggle_capture(window, cx);
        } else if !stream.pointer_captured(cx) {
            self.pending_capture = true;
            window.activate_window();
        }
    }

    /// Whether this window is key.
    pub(super) fn window_active(&self) -> bool {
        self.window_active
    }

    /// **Gamepad input**: the saved choice, else the server's `gamepad_on_start`. Off until
    /// the server's settings arrive.
    pub(super) fn gamepad_input(&self) -> bool {
        self.settings.as_ref().is_some_and(|settings| {
            crate::gamepad::input_on(settings.gamepad, self.preferences.gamepads.input)
        })
    }

    /// Brings the server up to date with the pads (`ARCH-032`): only while the server
    /// allows it, input is on, and this window is key; at rest otherwise.
    pub(super) fn sync_gamepads(&mut self, cx: &mut Context<Self>) {
        let Some(stream) = &self.current else {
            return;
        };
        let gate = crate::gamepad::Gate {
            ready_and_enabled: self
                .settings
                .as_ref()
                .is_some_and(|settings| settings.gamepad.enabled),
            input_on: self.gamepad_input(),
            key: self.window_active,
        };
        let commands = match cx.try_global::<Gamepads>() {
            Some(gamepads) => self.gamepad_link.sync(gamepads.pads(), gate),
            None => self.gamepad_link.sync(&[], gate),
        };
        stream.send_commands(commands);
    }

    /// **Gamepad input** switched; saved for this server.
    pub(super) fn change_gamepad_input(&mut self, on: bool, cx: &mut Context<Self>) {
        self.preferences.gamepads.input = Some(on);
        self.save(
            "the gamepad preference",
            move |p| p.gamepads.input = Some(on),
            cx,
        );
        if !on {
            if let Some(gamepads) = cx.try_global::<Gamepads>() {
                gamepads.stop_rumble();
            }
        }
        self.sync_gamepads(cx);
        cx.notify();
    }

    /// **Rumble** switched; saved for this server. Off stops what plays.
    pub(super) fn change_gamepad_rumble(&mut self, on: bool, cx: &mut Context<Self>) {
        self.preferences.gamepads.rumble = on;
        self.save("the rumble preference", move |p| p.gamepads.rumble = on, cx);
        if !on {
            if let Some(gamepads) = cx.try_global::<Gamepads>() {
                gamepads.stop_rumble();
            }
        }
        cx.notify();
    }

    /// Whether this window holds the pointer (read from the stream, never mirrored).
    /// Whether the stream has painted a picture yet.
    pub(super) fn picture_painted(&self, cx: &App) -> bool {
        self.current
            .as_ref()
            .is_some_and(|stream| stream.picture_painted(cx))
    }

    pub(super) fn pointer_captured(&self, cx: &App) -> bool {
        self.current
            .as_ref()
            .is_some_and(|stream| stream.pointer_captured(cx))
    }

    /// Shows `hint` over the picture, replacing any other. It fades out over its last
    /// 200 ms, then clears, unless a newer hint replaced it.
    fn show_capture_hint(&mut self, hint: CaptureHint, cx: &mut Context<Self>) {
        self.hint_generation += 1;
        let generation = self.hint_generation;
        self.capture_hint = Some((hint, generation, false));
        let shown = hint.text().1;
        let fade = Duration::from_millis(200);
        let executor = cx.background_executor().clone();
        self.hint_task = Some(cx.spawn(async move |this, cx| {
            executor.timer(shown.saturating_sub(fade)).await;
            let _ = this.update(cx, |this, cx| {
                if let Some((_, current, fading)) = &mut this.capture_hint {
                    if *current == generation {
                        *fading = true;
                        cx.notify();
                    }
                }
            });
            executor.timer(fade).await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .capture_hint
                    .is_some_and(|(_, current, _)| current == generation)
                {
                    this.capture_hint = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
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
        // One Files window per session, whoever opened it (`ARCH-028`).
        let open = cx
            .default_global::<OpenWindows>()
            .files_window(&self.remote.session_id);
        if let Some((handle, _)) = open {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
        }
        self.files_window = None;
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
        let session_id = self.remote.session_id.clone();
        cx.defer(move |cx| {
            // The Library may have opened this session's Files window since the check above.
            let open = cx.default_global::<OpenWindows>().files_window(&session_id);
            if let Some((handle, _)) = open {
                if handle
                    .update(cx, |_, window, _| window.activate_window())
                    .is_ok()
                {
                    return;
                }
            }
            let _ = owner_window.update(cx, |_, window, cx| {
                let source = files_window::FilesSource {
                    title,
                    selkies: access,
                    policy,
                    storage,
                };
                let owner = FilesOwner::Session(owner_window, session.downgrade());
                let result = FilesWindow::open(source, rt, window, owner, session_id, cx);
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

    /// The Stats section opened or shut. The session subscribes to the server's figures
    /// while it is open or the strip is shown (`ARCH-031`).
    pub(super) fn set_stats_open(&mut self, open: bool) {
        let change = self.stats_interest.set_section(open);
        self.apply_interest(change);
    }

    /// **Show over the stream** switched, from the Stats card or the View menu; saved for
    /// this server.
    pub(super) fn set_stats_strip(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.preferences.stats_strip != on {
            self.preferences.stats_strip = on;
            self.save(
                "the stats strip preference",
                move |p| p.stats_strip = on,
                cx,
            );
        }
        let change = self.stats_interest.set_strip(on);
        self.apply_interest(change);
        cx.notify();
    }

    /// The subscription started or stopped: a fresh history, and the stream is told.
    fn apply_interest(&mut self, change: Option<bool>) {
        let Some(wanted) = change else {
            return;
        };
        self.stats_history.clear();
        if let Some(stream) = &self.current {
            stream.set_stats(wanted);
        }
    }

    /// Whether Skerry's chords act on the Mac (`ARCH-031`): a server lock wins, then the
    /// saved choice, then the server's value.
    pub(super) fn mac_shortcuts(&self) -> bool {
        crate::shortcuts::effective(self.shortcut_policy, self.preferences.mac_shortcuts)
    }

    pub(super) fn shortcuts_locked(&self) -> bool {
        self.shortcut_policy.is_some_and(|policy| policy.locked)
    }

    /// **Mac shortcuts active** switched; ignored while the server locks it.
    pub(super) fn change_mac_shortcuts(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.shortcuts_locked() {
            return;
        }
        self.preferences.mac_shortcuts = Some(active);
        self.save(
            "the Mac shortcuts preference",
            move |p| p.mac_shortcuts = Some(active),
            cx,
        );
        if let Some(stream) = &self.current {
            stream.set_mac_shortcuts(self.mac_shortcuts(), cx);
        }
        cx.notify();
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
        self.pending_capture = false;
        self.capture_hint = None;
        self.hint_task = None;
        self.stream_events = None;
        // Announced pads leave before the key releases and the close frame (`ARCH-032`).
        if let Some(stream) = &self.current {
            stream.send_commands(self.gamepad_link.close());
            if self.window_active {
                if let Some(gamepads) = cx.try_global::<Gamepads>() {
                    gamepads.stop_rumble();
                }
            }
        }
        self.gamepad_link.reset();
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // Hidden until the first sample, as the web's strip is. Built only while shown: this
        // window re-renders on every cursor event.
        let strip = (self.preferences.stats_strip && !self.stats_history.is_empty()).then(|| {
            let start = self
                .stats_history
                .len()
                .saturating_sub(stats::STRIP_SAMPLES);
            let recent: Vec<_> = self.stats_history.range(start..).cloned().collect();
            stats::strip(self.stream_info.as_ref(), &recent)
        });
        let viewport_width = window.viewport_size().width;
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
            .on_action(cx.listener(|this, _: &ToggleStatsStrip, _, cx| {
                this.set_stats_strip(!this.preferences.stats_strip, cx);
            }))
            .on_action(cx.listener(|this, _: &TogglePointerCapture, window, cx| {
                this.request_capture(window, cx);
            }))
            .when_some(self.current.as_ref(), |root, stream| {
                root.child(stream.view())
            })
            // Over the picture and the ⋯ trigger, but with no hitbox: clicks reach the
            // desktop and the trigger under it (`ARCH-031`).
            .when_some(strip, |root, strip| {
                root.child(skerry_ui::stats::strip(&strip, viewport_width))
            })
            // Bottom-centre with no hitbox, above a bottom banner when one shows (`ARCH-032`).
            .when_some(self.capture_hint, |root, (hint, generation, fading)| {
                let banner = self.status.is_some() || self.startup_stalled.is_some();
                root.child(session::capture_hint(
                    &hint.text().0,
                    session::capture_hint_bottom(banner),
                    viewport_width,
                    if fading {
                        session::HintFade::Out
                    } else {
                        session::HintFade::In
                    },
                    generation,
                ))
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
