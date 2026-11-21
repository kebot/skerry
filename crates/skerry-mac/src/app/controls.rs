use super::{
    native_window, CloseSessionControls, DisplayMode, Launcher, ToggleFullScreen,
    ToggleSessionControls,
};
use crate::clipboard::{ClipboardAction, Effective};
use crate::input::{InputFeature, RemoteChord};
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
    div, point, prelude::*, px, size, AnyWindowHandle, App, Bounds, Context, Entity, FocusHandle,
    TitlebarOptions, Window, WindowBounds, WindowKind, WindowOptions,
};
use objc2_app_kit::{NSWindowCollectionBehavior, NSWindowOrderingMode};
use skerry_ui::controls::{
    self, ChordItem, ClipboardProps, CodecChoice, ControlsAction, ControlsProps, ControlsUiState,
    ScreenProps, Section, SwitchItem, VideoProps, VideoSettingControl, VideoSettingRow,
};
use std::rc::Rc;

#[derive(Clone)]
struct Owner {
    window: AnyWindowHandle,
    launcher: Entity<Launcher>,
}

pub(super) struct ControlPanel {
    owner: Owner,
    focus: FocusHandle,
    fullscreen: bool,
    ui: ControlsUiState,
    width: Entity<InputState>,
    height: Entity<InputState>,
}

#[derive(Clone, Copy)]
enum Command {
    Close,
    FullScreen,
    Reconnect,
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
}

impl ControlPanel {
    pub(super) fn open(
        display_mode: DisplayMode,
        window: &Window,
        launcher: Entity<Launcher>,
        cx: &mut App,
    ) -> gpui_kit::Result<AnyWindowHandle> {
        let owner = Owner {
            window: window.window_handle(),
            launcher: launcher.clone(),
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
                    cx.observe(&launcher, |_, _, cx| cx.notify()).detach();
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
                owner.launcher.update(cx, |launcher, cx| {
                    match command {
                        Command::Close => launcher.close_controls(window, cx),
                        Command::FullScreen => {
                            launcher.close_controls(window, cx);
                            window.toggle_fullscreen();
                        }
                        Command::Reconnect => launcher.reconnect(window, cx),
                        Command::Disconnect => {
                            launcher.close_stream(window, cx);
                            launcher.muted = false;
                            launcher.refresh(cx);
                        }
                        Command::Audio(enabled) => {
                            if let Some((_, stream)) = &launcher.current {
                                launcher.muted = !enabled;
                                stream.set_audio(enabled);
                            }
                        }
                        Command::Playback(enabled) => {
                            if let Some((_, stream)) = &launcher.current {
                                stream.set_video(enabled);
                            }
                        }
                        Command::Display(mode) => {
                            launcher.resize_task = None;
                            launcher.display_mode = mode;
                            launcher.resize_display(window, cx);
                        }
                        Command::Retina(enabled) => {
                            launcher.retina = enabled;
                            launcher.resize_task = None;
                            launcher.display_mode = if enabled {
                                DisplayMode::Retina
                            } else {
                                DisplayMode::MatchWindow
                            };
                            launcher.resize_display(window, cx);
                        }
                        Command::ResetToWindow => {
                            launcher.dpi_override = None;
                            launcher.display_mode = if launcher.retina {
                                DisplayMode::Retina
                            } else {
                                DisplayMode::MatchWindow
                            };
                            launcher.resize_display(window, cx);
                        }
                        Command::UiScaling(dpi) => {
                            launcher.dpi_override = dpi;
                            launcher.resize_display(window, cx);
                        }
                        Command::ScaleLocally(enabled) => {
                            launcher.scale_locally = enabled;
                            if let Some((_, stream)) = &launcher.current {
                                stream.scale_locally(enabled, cx);
                            }
                        }
                        Command::Screen(change) => {
                            if let Some((_, stream)) = &launcher.current {
                                stream.set_screen(change);
                            }
                            if matches!(change, ScreenChange::Alignment(_)) {
                                // Selkies aligns whatever size it last stored, which may not be
                                // the size this window wants; re-request ours so it is aligned
                                // (or restored exactly when alignment is turned off).
                                launcher.applied_resolution = None;
                                launcher.resize_display(window, cx);
                            }
                        }
                        Command::Codec(codec) => launcher.change_codec(codec, window, cx),
                        Command::Setting(key, value) => launcher.change_setting(key, value, cx),
                        Command::ResetVideo => launcher.reset_settings(cx),
                        Command::Input(feature, enabled) => {
                            launcher.change_input(feature, enabled, window, cx)
                        }
                        Command::Clipboard(action) => launcher.change_clipboard(action, window, cx),
                        Command::Chord(chord) => {
                            launcher.close_controls(window, cx);
                            if let Some((_, stream)) = &launcher.current {
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
            self.ui.picker = None;
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
                    Section::Stats => self.ui.stats_open = !self.ui.stats_open,
                }
                cx.notify();
            }
            ControlsAction::TogglePicker(picker) => {
                self.ui.picker = (self.ui.picker != Some(picker)).then_some(picker);
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
                self.ui.picker = None;
                Self::send(self.owner.clone(), Command::UiScaling(dpi), cx);
                cx.notify();
            }
            ControlsAction::SetFitMode => {
                self.ui.picker = None;
                self.ui.size_error = false;
                Self::send(self.owner.clone(), Command::Display(DisplayMode::Fit), cx);
                cx.notify();
            }
            ControlsAction::SelectPreset(w, h) => {
                self.ui.picker = None;
                self.width
                    .update(cx, |s, cx| s.set_value(w.to_string(), window, cx));
                self.height
                    .update(cx, |s, cx| s.set_value(h.to_string(), window, cx));
                self.apply_size(window, cx);
            }
            ControlsAction::ApplyManualSize => self.apply_size(window, cx),
            ControlsAction::ResetToWindow => {
                self.ui.picker = None;
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
                    let launcher = self.owner.launcher.read(cx);
                    let preferred = launcher.preferences.codec;
                    let current = current_codec(launcher);
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
        }
    }

    fn build_props(&self, launcher: &Launcher, app_name: String) -> ControlsProps {
        let input = launcher.preferences.input;
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

        let clipboard = launcher.clipboard_policy.map(|policy| {
            let settings = launcher.preferences.clipboard;
            ClipboardProps {
                send: policy.inbound.then_some(settings.send),
                receive: policy.outbound.then_some(settings.receive),
                seamless: Effective::new(policy, settings).seamless,
                seamless_locked: policy.seamless_locked,
            }
        });

        let preset_label = match launcher.display_mode {
            DisplayMode::Fixed(size) => format!("{} × {}", size.0, size.1),
            DisplayMode::Fit => "Keep Remote Size".into(),
            DisplayMode::MatchWindow | DisplayMode::Retina => "Match Window".into(),
        };

        let screen = ScreenProps {
            resize_allowed: launcher.resize_allowed,
            matching_retina: launcher.display_mode == DisplayMode::Retina,
            alignment: launcher.screen_policy.alignment,
            dpi: launcher.screen_policy.dpi,
            dpi_choices: launcher.screen_policy.dpi_choices.clone(),
            dpi_override: launcher.dpi_override,
            preset_label,
            scale_locally: launcher.scale_locally,
        };

        let video_settings = build_video_props(launcher);

        let diagnostics = launcher.stats.as_ref().map(|stats| {
            vec![
                (
                    "Resolution".into(),
                    format!("{}×{}", stats.width, stats.height),
                ),
                ("Client FPS".into(), format!("{:.0}", stats.fps)),
                (
                    "Received bitrate".into(),
                    format!("{:.2} Mb/s", stats.wire_mbps),
                ),
                (
                    "Decode time".into(),
                    format!("{:.1} ms per packet", stats.decode_ms),
                ),
                ("Rejected packets".into(), stats.dropped.to_string()),
                (
                    "Round trip".into(),
                    stats
                        .rtt_ms
                        .map_or("Unavailable".into(), |ms| format!("{ms:.0} ms")),
                ),
                ("Transport".into(), "WebSocket".into()),
                (
                    "Client decoder".into(),
                    stats
                        .client_codec
                        .map_or("Waiting for video", Codec::decoder_label)
                        .into(),
                ),
                (
                    "Server encoder".into(),
                    stats.server_encoder.clone().unwrap_or("Unreported".into()),
                ),
            ]
        });

        ControlsProps {
            app_name,
            fullscreen: self.fullscreen,
            audio: launcher.audio,
            video: launcher.video,
            audio_unavailable: launcher.audio_unavailable.map(str::to_string),
            input_switches,
            chords,
            clipboard,
            screen,
            video_settings,
            diagnostics,
        }
    }
}

fn current_codec(launcher: &Launcher) -> Codec {
    let preferred = launcher.preferences.codec;
    launcher
        .stats
        .as_ref()
        .and_then(|stats| stats.client_codec)
        .unwrap_or_else(|| {
            launcher
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

fn build_video_props(launcher: &Launcher) -> Option<VideoProps> {
    let settings = launcher.settings.as_ref()?;
    let codec = current_codec(launcher);
    let rate = launcher
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

    let preferred_notice = (launcher.preferences.codec != codec)
        .then(|| format!("Preferred codec · {}", launcher.preferences.codec.label()));

    let rows = [
        (Key::Fps, "Frame rate cap", "fps", 5),
        (Key::Jpeg, "JPEG quality", "%", 5),
        (Key::PaintOver, "Improve still areas", "", 1),
        (Key::PaintQuality, "Still image quality", "%", 5),
        (Key::RateControl, "Rate control", "", 1),
        (Key::Bitrate, "Target bitrate", "kbps", 500),
        (Key::Crf, "Constant quality (CRF)", "", 1),
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
        let value = launcher
            .pending_settings
            .get(&key)
            .copied()
            .unwrap_or(control_spec.current);

        let control = if key == Key::PaintOver {
            VideoSettingControl::Toggle {
                enabled: value != 0,
                can_toggle: settings.request(key, 1 - value).is_some(),
            }
        } else if key == Key::RateControl {
            let options = [(0, "CBR"), (1, "CRF")]
                .into_iter()
                .filter(|&(mode, _)| value == mode || settings.request(key, mode).is_some())
                .map(|(mode, label)| (mode, label.to_string(), value == mode))
                .collect();
            VideoSettingControl::Segmented { options }
        } else {
            let dec = value.saturating_sub(step).max(control_spec.min);
            let inc = value.saturating_add(step).min(control_spec.max);
            VideoSettingControl::Stepper {
                display: format!("{value} {unit}").trim_end().to_owned(),
                decrement: (dec != value && settings.request(key, dec).is_some()).then_some(dec),
                increment: (inc != value && settings.request(key, inc).is_some()).then_some(inc),
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        let launcher = owner.launcher.read(cx);
        let Some((session, _)) = &launcher.current else {
            return root;
        };
        let props = self.build_props(launcher, session.app_name.clone());
        let entity = cx.entity().clone();
        let handler = Rc::new(
            move |action: ControlsAction, window: &mut Window, cx: &mut App| {
                entity.update(cx, |this, cx| this.handle_action(action, window, cx));
            },
        );

        root.child(controls::render(
            &theme,
            &props,
            &self.ui,
            &self.width,
            &self.height,
            handler,
        ))
    }
}
