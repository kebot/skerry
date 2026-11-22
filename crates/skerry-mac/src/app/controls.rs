use super::session_window::SessionWindow;
use super::{
    native_window, CloseSessionControls, DisplayMode, ToggleFullScreen, ToggleSessionControls,
};
use crate::clipboard::{ClipboardAction, Effective};
use crate::input::{InputFeature, RemoteChord};
use crate::stats;
use crate::video::Codec;
use crate::{
    settings::{Key, ScreenChange},
    transport::Resolution,
};
use gpui_kit::component::{
    input::{InputEvent, InputState},
    scroll::ScrollableElement as _,
    ActiveTheme as _,
};
use gpui_kit::{
    div, point, prelude::*, px, size, AnyWindowHandle, App, Bounds, ClipboardItem, Context, Entity,
    FocusHandle, Task, TitlebarOptions, Window, WindowBounds, WindowKind, WindowOptions,
};
use objc2_app_kit::{NSWindowCollectionBehavior, NSWindowOrderingMode};
use skerry_ui::controls::{
    self, ChordItem, ClipboardProps, CodecChoice, ControlsAction, ControlsProps, ControlsUiState,
    ScreenProps, Section, SliderUnit, SwitchItem, VideoProps, VideoSettingControl, VideoSettingRow,
};
use std::rc::Rc;

#[derive(Clone)]
struct Owner {
    window: AnyWindowHandle,
    session: Entity<SessionWindow>,
}

pub(super) struct ControlPanel {
    owner: Owner,
    focus: FocusHandle,
    fullscreen: bool,
    ui: ControlsUiState,
    width: Entity<InputState>,
    height: Entity<InputState>,
    /// Shows the Stats copy button's check, and clears it after a moment.
    copied: Option<Task<()>>,
}

#[derive(Clone, Copy)]
enum Command {
    Close,
    FullScreen,
    Reconnect,
    Files,
    Disconnect,
    Audio(bool),
    Playback(bool),
    Display(DisplayMode),
    Retina(bool),
    ResetToWindow,
    UiScaling(Option<u16>),
    ScaleLocally(bool),
    Screen(ScreenChange),
    Setting(Key, u32),
    Codec(Codec),
    ResetVideo,
    Input(InputFeature, bool),
    Chord(RemoteChord),
    Clipboard(ClipboardAction),
    Stats(bool),
}

impl ControlPanel {
    pub(super) fn open(
        display_mode: DisplayMode,
        window: &Window,
        session: Entity<SessionWindow>,
        cx: &mut App,
    ) -> gpui_kit::Result<AnyWindowHandle> {
        let owner = Owner {
            window: window.window_handle(),
            session: session.clone(),
        };
        let parent = native_window(window);
        let fullscreen = window.is_fullscreen();
        let bounds = window.bounds();
        let panel_size = size(px(388.), px(680.));
        let display = window.display(cx);
        let bounds = display.as_ref().map_or_else(
            || {
                Bounds::new(
                    point(bounds.right() - panel_size.width, bounds.top()),
                    panel_size,
                )
            },
            |display| {
                let screen = display.bounds();
                let panel_size = panel_size.min(&screen.size);
                let x = (bounds.right() - panel_size.width - px(12.))
                    .clamp(screen.left(), screen.right() - panel_size.width);
                let y = (bounds.top() + px(40.))
                    .clamp(screen.top(), screen.bottom() - panel_size.height);
                Bounds::new(point(x, y), panel_size)
            },
        );
        let (handle, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(330.), px(300.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Session Controls".into()),
                    ..Default::default()
                }),
                display_id: display.map(|display| display.id()),
                kind: WindowKind::Floating,
                focus: false,
                show: false,
                is_minimizable: false,
                ..Default::default()
            },
            cx,
            move |window, cx| {
                if let Some(panel) = native_window(window) {
                    panel.setCollectionBehavior(
                        panel.collectionBehavior()
                            | NSWindowCollectionBehavior::FullScreenAuxiliary,
                    );
                    if let Some(parent) = &parent {
                        // Both windows are live on the main thread and the panel has a single owner.
                        unsafe {
                            parent.addChildWindow_ordered(&panel, NSWindowOrderingMode::Above)
                        };
                    }
                }
                let panel_owner = owner.clone();
                let panel = cx.new(|cx: &mut Context<Self>| {
                    cx.observe(&session, |_, _, cx| cx.notify()).detach();
                    let initial = if let DisplayMode::Fixed(size) = display_mode {
                        Some(size)
                    } else {
                        None
                    };
                    let digit_filter = |s: &str, _: &mut App| {
                        s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit())
                    };
                    let width = cx.new(|cx| {
                        let state = InputState::new(window, cx)
                            .placeholder("e.g. 1920")
                            .validate(digit_filter);
                        if let Some(size) = initial {
                            state.default_value(size.0.to_string())
                        } else {
                            state
                        }
                    });
                    let height = cx.new(|cx| {
                        let state = InputState::new(window, cx)
                            .placeholder("e.g. 1080")
                            .validate(digit_filter);
                        if let Some(size) = initial {
                            state.default_value(size.1.to_string())
                        } else {
                            state
                        }
                    });
                    cx.subscribe_in(&width, window, |this, _, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            this.apply_size(window, cx);
                        }
                    })
                    .detach();
                    cx.subscribe_in(
                        &height,
                        window,
                        |this, _, event: &InputEvent, window, cx| {
                            if matches!(event, InputEvent::PressEnter { .. }) {
                                this.apply_size(window, cx);
                            }
                        },
                    )
                    .detach();
                    Self {
                        owner: panel_owner,
                        focus: cx.focus_handle(),
                        fullscreen,
                        ui: ControlsUiState::default(),
                        width,
                        height,
                        copied: None,
                    }
                });
                window.on_window_should_close(cx, move |_, cx| {
                    Self::send(owner.clone(), Command::Close, cx);
                    false
                });
                let focus = panel.read(cx).focus.clone();
                window.focus(&focus, cx);
                window.activate_window();
                panel
            },
        )?;
        Ok(handle)
    }

    fn send(owner: Owner, command: Command, cx: &mut App) {
        cx.defer(move |cx| {
            let _ = owner.window.update(cx, |_, window, cx| {
                owner.session.update(cx, |session, cx| {
                    match command {
                        Command::Close => session.close_controls(window, cx),
                        Command::FullScreen => {
                            session.close_controls(window, cx);
                            window.toggle_fullscreen();
                        }
                        Command::Reconnect => session.reconnect(window, cx),
                        Command::Files => session.toggle_files(window, cx),
                        Command::Disconnect => session.disconnect(window, cx),
                        Command::Audio(enabled) => {
                            if let Some(stream) = &session.current {
                                session.muted = !enabled;
                                stream.set_audio(enabled);
                            }
                        }
                        Command::Playback(enabled) => {
                            if let Some(stream) = &session.current {
                                stream.set_video(enabled);
                            }
                        }
                        Command::Display(mode) => {
                            session.resize_task = None;
                            session.display_mode = mode;
                            session.resize_display(window, cx);
                        }
                        Command::Retina(enabled) => {
                            session.retina = enabled;
                            session.resize_task = None;
                            session.display_mode = if enabled {
                                DisplayMode::Retina
                            } else {
                                DisplayMode::MatchWindow
                            };
                            session.resize_display(window, cx);
                        }
                        Command::ResetToWindow => {
                            session.dpi_override = None;
                            session.display_mode = if session.retina {
                                DisplayMode::Retina
                            } else {
                                DisplayMode::MatchWindow
                            };
                            session.resize_display(window, cx);
                        }
                        Command::UiScaling(dpi) => {
                            session.dpi_override = dpi;
                            session.resize_display(window, cx);
                        }
                        Command::ScaleLocally(enabled) => {
                            session.scale_locally = enabled;
                            if let Some(stream) = &session.current {
                                stream.scale_locally(enabled, cx);
                            }
                        }
                        Command::Screen(change) => {
                            if let Some(stream) = &session.current {
                                stream.set_screen(change);
                            }
                            if matches!(change, ScreenChange::Alignment(_)) {
                                // Selkies aligns whatever size it last stored, which may not be
                                // the size this window wants; re-request ours so it is aligned
                                // (or restored exactly when alignment is turned off).
                                session.applied_resolution = None;
                                session.resize_display(window, cx);
                            }
                        }
                        Command::Codec(codec) => session.change_codec(codec, window, cx),
                        Command::Setting(key, value) => session.change_setting(key, value, cx),
                        Command::ResetVideo => session.reset_settings(cx),
                        Command::Input(feature, enabled) => {
                            session.change_input(feature, enabled, window, cx)
                        }
                        Command::Clipboard(action) => session.change_clipboard(action, window, cx),
                        Command::Stats(open) => session.set_stats_open(open),
                        Command::Chord(chord) => {
                            session.close_controls(window, cx);
                            if let Some(stream) = &session.current {
                                stream.send_chord(chord, cx);
                            }
                        }
                    }
                    cx.notify();
                });
            });
        });
    }

    fn apply_size(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width_val = self.width.read(cx).value();
        let height_val = self.height.read(cx).value();
        let resolution = Resolution::manual(&width_val, &height_val);
        self.ui.size_error = resolution.is_none();
        if let Some(size) = resolution {
            self.width
                .update(cx, |s, cx| s.set_value(size.0.to_string(), window, cx));
            self.height
                .update(cx, |s, cx| s.set_value(size.1.to_string(), window, cx));
            Self::send(
                self.owner.clone(),
                Command::Display(DisplayMode::Fixed(size)),
                cx,
            );
        }
        cx.notify();
    }

    fn handle_action(
        &mut self,
        action: ControlsAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            ControlsAction::ToggleSection(section) => {
                match section {
                    Section::Input => self.ui.input_open = !self.ui.input_open,
                    Section::Clipboard => self.ui.clipboard_open = !self.ui.clipboard_open,
                    Section::Screen => self.ui.screen_open = !self.ui.screen_open,
                    Section::Video => self.ui.video_open = !self.ui.video_open,
                    Section::Stats => {
                        self.ui.stats_open = !self.ui.stats_open;
                        Self::send(self.owner.clone(), Command::Stats(self.ui.stats_open), cx);
                    }
                }
                cx.notify();
            }
            ControlsAction::SetAudio(enabled) => {
                Self::send(self.owner.clone(), Command::Audio(enabled), cx)
            }
            ControlsAction::SetPlayback(enabled) => {
                Self::send(self.owner.clone(), Command::Playback(enabled), cx)
            }
            ControlsAction::ToggleFullScreen => {
                Self::send(self.owner.clone(), Command::FullScreen, cx)
            }
            ControlsAction::Reconnect => Self::send(self.owner.clone(), Command::Reconnect, cx),
            ControlsAction::OpenFiles => Self::send(self.owner.clone(), Command::Files, cx),
            ControlsAction::Disconnect => Self::send(self.owner.clone(), Command::Disconnect, cx),
            ControlsAction::SetInput { id, enabled } => {
                if let Some((feature, _)) = InputFeature::ALL
                    .into_iter()
                    .find(|(_, label)| *label == id)
                {
                    Self::send(self.owner.clone(), Command::Input(feature, enabled), cx);
                }
            }
            ControlsAction::SetClipboardSend(on) => Self::send(
                self.owner.clone(),
                Command::Clipboard(ClipboardAction::SetSend(on)),
                cx,
            ),
            ControlsAction::SetClipboardReceive(on) => Self::send(
                self.owner.clone(),
                Command::Clipboard(ClipboardAction::SetReceive(on)),
                cx,
            ),
            ControlsAction::SetClipboardSeamless(on) => Self::send(
                self.owner.clone(),
                Command::Clipboard(ClipboardAction::SetSeamless(on)),
                cx,
            ),
            ControlsAction::SendClipboard => Self::send(
                self.owner.clone(),
                Command::Clipboard(ClipboardAction::SendNow),
                cx,
            ),
            ControlsAction::CopySessionClipboard => Self::send(
                self.owner.clone(),
                Command::Clipboard(ClipboardAction::CopyNow),
                cx,
            ),
            ControlsAction::SendChord(id) => {
                let chord = match id.as_str() {
                    "alt-tab" => Some(RemoteChord::SwitchApp),
                    "ctrl-alt-delete" => Some(RemoteChord::ControlAltDelete),
                    _ => None,
                };
                if let Some(chord) = chord {
                    Self::send(self.owner.clone(), Command::Chord(chord), cx);
                }
            }
            ControlsAction::SetRetina(enabled) => {
                Self::send(self.owner.clone(), Command::Retina(enabled), cx)
            }
            ControlsAction::SetAlignment(enabled) => Self::send(
                self.owner.clone(),
                Command::Screen(ScreenChange::Alignment(enabled)),
                cx,
            ),
            ControlsAction::SetUiScaling(dpi) => {
                Self::send(self.owner.clone(), Command::UiScaling(dpi), cx);
                cx.notify();
            }
            ControlsAction::SetFitMode => {
                self.ui.size_error = false;
                Self::send(self.owner.clone(), Command::Display(DisplayMode::Fit), cx);
                cx.notify();
            }
            ControlsAction::SelectPreset(w, h) => {
                self.width
                    .update(cx, |s, cx| s.set_value(w.to_string(), window, cx));
                self.height
                    .update(cx, |s, cx| s.set_value(h.to_string(), window, cx));
                self.apply_size(window, cx);
            }
            ControlsAction::ApplyManualSize => self.apply_size(window, cx),
            ControlsAction::ResetToWindow => {
                self.width.update(cx, |s, cx| s.set_value("", window, cx));
                self.height.update(cx, |s, cx| s.set_value("", window, cx));
                self.ui.size_error = false;
                Self::send(self.owner.clone(), Command::ResetToWindow, cx);
                cx.notify();
            }
            ControlsAction::SetScaleLocally(enabled) => {
                Self::send(self.owner.clone(), Command::ScaleLocally(enabled), cx)
            }
            ControlsAction::SelectCodec(wire) => {
                if let Some(choice) = Codec::ALL.into_iter().find(|c| c.wire() == wire) {
                    let session = self.owner.session.read(cx);
                    let preferred = session.preferences.codec;
                    let current = current_codec(session);
                    if choice != current || choice != preferred {
                        Self::send(self.owner.clone(), Command::Codec(choice), cx);
                    }
                }
            }
            ControlsAction::SetVideoSetting { id, value } => {
                if let Some(key) = Key::ALL.into_iter().find(|k| k.wire() == id) {
                    Self::send(self.owner.clone(), Command::Setting(key, value), cx);
                }
            }
            ControlsAction::ResetVideo => Self::send(self.owner.clone(), Command::ResetVideo, cx),
            ControlsAction::CopyStats => {
                let session = self.owner.session.read(cx);
                let history: Vec<_> = session.stats_history.iter().cloned().collect();
                let text = stats::report(session.stream_info.as_ref(), &history);
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.copied = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1500))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.copied = None;
                        cx.notify();
                    });
                }));
                cx.notify();
            }
        }
    }

    fn build_props(&self, session: &SessionWindow, app_name: String) -> ControlsProps {
        let input = session.preferences.input;
        let input_switches = InputFeature::ALL
            .into_iter()
            .map(|(feature, label)| SwitchItem {
                id: label.to_string(),
                label: label.to_string(),
                enabled: feature.enabled(input),
            })
            .collect();

        let chords = vec![
            ChordItem {
                id: "alt-tab".into(),
                label: "Alt+Tab".into(),
            },
            ChordItem {
                id: "ctrl-alt-delete".into(),
                label: "Ctrl+Alt+Delete".into(),
            },
        ];

        let clipboard = session.clipboard_policy.map(|policy| {
            let settings = session.preferences.clipboard;
            ClipboardProps {
                send: policy.inbound.then_some(settings.send),
                receive: policy.outbound.then_some(settings.receive),
                seamless: Effective::new(policy, settings).seamless,
                seamless_locked: policy.seamless_locked,
            }
        });

        let preset_label = match session.display_mode {
            DisplayMode::Fixed(size) => format!("{} × {}", size.0, size.1),
            DisplayMode::Fit => "Keep Remote Size".into(),
            DisplayMode::MatchWindow | DisplayMode::Retina => "Match Window".into(),
        };

        let screen = ScreenProps {
            resize_allowed: session.resize_allowed,
            matching_retina: session.display_mode == DisplayMode::Retina,
            alignment: session.screen_policy.alignment,
            dpi: session.screen_policy.dpi,
            dpi_choices: session.screen_policy.dpi_choices.clone(),
            dpi_override: session.dpi_override,
            preset_label,
            scale_locally: session.scale_locally,
        };

        let video_settings = build_video_props(session);

        let stats = (!session.stats_history.is_empty()).then(|| {
            let history: Vec<_> = session.stats_history.iter().cloned().collect();
            let framerate = session
                .settings
                .as_ref()
                .and_then(|settings| settings.get(Key::Fps))
                .map_or(0, |fps| fps.current);
            stats::props(session.stream_info.as_ref(), &history, framerate)
        });

        ControlsProps {
            app_name,
            fullscreen: self.fullscreen,
            audio: session.audio,
            video: session.video,
            audio_unavailable: session.audio_unavailable.map(str::to_string),
            input_switches,
            chords,
            clipboard,
            screen,
            video_settings,
            stats,
            stats_copied: self.copied.is_some(),
        }
    }
}

fn current_codec(session: &SessionWindow) -> Codec {
    let preferred = session.preferences.codec;
    session
        .stats
        .as_ref()
        .and_then(|stats| stats.client_codec)
        .unwrap_or_else(|| {
            session
                .settings
                .as_ref()
                .map(|settings| {
                    if settings.codecs.contains(&preferred) {
                        preferred
                    } else {
                        settings.codecs.first().copied().unwrap_or_default()
                    }
                })
                .unwrap_or(preferred)
        })
}

fn build_video_props(session: &SessionWindow) -> Option<VideoProps> {
    let settings = session.settings.as_ref()?;
    let codec = current_codec(session);
    let rate = session
        .pending_settings
        .get(&Key::RateControl)
        .copied()
        .or_else(|| {
            settings
                .get(Key::RateControl)
                .map(|setting| setting.current)
        });

    let codecs = settings
        .codecs
        .iter()
        .map(|&choice| CodecChoice {
            id: choice.wire().into(),
            label: choice.label().into(),
            selected: choice == codec,
        })
        .collect();

    let preferred_notice = (session.preferences.codec != codec)
        .then(|| format!("Preferred codec · {}", session.preferences.codec.label()));

    let rows = [
        (Key::Fps, "Frames per second", SliderUnit::Fps, 1),
        (Key::Jpeg, "Base Quality", SliderUnit::Plain, 1),
        (Key::PaintOver, "Use Paint-Overs", SliderUnit::Plain, 1),
        (Key::PaintQuality, "Paintover Quality", SliderUnit::Plain, 1),
        (
            Key::RateControl,
            "Encoder Rate Control Mode",
            SliderUnit::Plain,
            1,
        ),
        (Key::Bitrate, "Video Bitrate", SliderUnit::Kbps, 500),
        (Key::Crf, "Video CRF", SliderUnit::Plain, 1),
    ]
    .into_iter()
    .filter_map(|(key, label, unit, step)| {
        let visible = match key {
            Key::Fps => true,
            Key::Jpeg | Key::PaintOver | Key::PaintQuality => codec == Codec::Jpeg,
            Key::RateControl => codec != Codec::Jpeg,
            Key::Bitrate => codec != Codec::Jpeg && rate != Some(1),
            Key::Crf => codec != Codec::Jpeg && rate == Some(1),
        };
        if !visible {
            return None;
        }
        let control_spec = settings.get(key)?;
        let value = session
            .pending_settings
            .get(&key)
            .copied()
            .unwrap_or(control_spec.current);

        let control = if key == Key::PaintOver {
            VideoSettingControl::Switch {
                enabled: value != 0,
                locked: settings.request(key, 1 - value).is_none(),
            }
        } else if key == Key::RateControl {
            let options: Vec<_> = [(0, "CBR (Constant Bitrate)"), (1, "CRF (Constant Quality)")]
                .into_iter()
                .filter(|&(mode, _)| value == mode || settings.request(key, mode).is_some())
                .map(|(mode, label)| (mode, label.to_string()))
                .collect();
            VideoSettingControl::Select {
                value,
                locked: options.len() <= 1,
                options,
            }
        } else {
            VideoSettingControl::Slider {
                value,
                min: control_spec.min,
                max: control_spec.max,
                step,
                unit,
                locked: settings.request(key, value).is_none(),
            }
        };

        Some(VideoSettingRow {
            id: key.wire().into(),
            label: label.into(),
            control,
        })
    })
    .collect();

    Some(VideoProps {
        codecs,
        preferred_notice,
        rows,
    })
}

impl Render for ControlPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let owner = self.owner.clone();
        let close_owner = owner.clone();
        let toggle_owner = owner.clone();
        let fullscreen_owner = owner.clone();
        let root = div()
            .id("session-controls")
            .size_full()
            .overflow_y_scrollbar()
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_sm()
            .track_focus(&self.focus)
            .key_context("SessionControls")
            .on_action(move |_: &CloseSessionControls, _, cx| {
                Self::send(close_owner.clone(), Command::Close, cx)
            })
            .on_action(move |_: &ToggleSessionControls, _, cx| {
                Self::send(toggle_owner.clone(), Command::Close, cx)
            })
            .on_action(move |_: &ToggleFullScreen, _, cx| {
                Self::send(fullscreen_owner.clone(), Command::FullScreen, cx)
            });

        let session = owner.session.read(cx);
        if session.current.is_none() {
            return root;
        }
        let props = self.build_props(session, session.remote.app_name.clone());
        let entity = cx.entity().clone();
        let handler = Rc::new(
            move |action: ControlsAction, window: &mut Window, cx: &mut App| {
                entity.update(cx, |this, cx| this.handle_action(action, window, cx));
            },
        );

        root.child(controls::render(
            &props,
            &self.ui,
            &self.width,
            &self.height,
            handler,
            window,
            cx,
        ))
    }
}
