use super::session_window::SessionWindow;
use super::{
    native_window, CloseSessionControls, DisplayMode, ToggleFullScreen, ToggleSessionControls,
    ToggleStatsStrip,
};
use crate::audio::{OutputProblem, OutputStatus};
use crate::clipboard::{ClipboardAction, Effective};
use crate::input::{InputFeature, RemoteChord};
use crate::persist::WriteQueue;
use crate::settings::{AudioBitrate, ControlsSections};
use crate::shortcuts;
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
    FocusHandle, Pixels, Point, Task, TitlebarOptions, Window, WindowBounds, WindowKind,
    WindowOptions,
};
use objc2_app_kit::{NSWindowCollectionBehavior, NSWindowOrderingMode};
use skerry_ui::controls::{
    self, AudioBitrateProps, AudioOutputItem, AudioProps, ChordItem, ClipboardProps, CodecChoice,
    ControlsAction, ControlsProps, ControlsUiState, ScreenProps, Section, ShortcutRow,
    ShortcutsProps, SliderUnit, SwitchItem, VideoProps, VideoSettingControl, VideoSettingRow,
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
    /// The scroll offset kept while the panel's content is incomplete (a reconnect), to
    /// restore once it has refilled.
    scroll_saved: Option<Point<Pixels>>,
}

#[derive(Clone)]
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
    AudioOutput(Option<String>),
    AudioBitrate(u32),
    StatsStrip(bool),
    MacShortcuts(bool),
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
        // The queue's copy of this server's preferences already has every toggle queued
        // from any window.
        let sections = {
            let server = session.read(cx).server().to_string();
            cx.global::<WriteQueue>().read(&server).controls
        };
        let ui = ControlsUiState::default().with_sections(sections.to_array());
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
                    let focus = cx.focus_handle();
                    Self {
                        owner: panel_owner,
                        ui: ControlsUiState {
                            return_focus: Some(focus.clone()),
                            ..ui
                        },
                        focus,
                        fullscreen,
                        width,
                        height,
                        copied: None,
                        scroll_saved: None,
                    }
                });
                if sections.stats {
                    // A restored open Stats section subscribes, as a click on it would.
                    Self::send(owner.clone(), Command::Stats(true), cx);
                }
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
                        Command::AudioOutput(uid) => session.change_audio_output(uid, cx),
                        Command::AudioBitrate(bps) => session.change_audio_bitrate(bps, cx),
                        Command::StatsStrip(on) => session.set_stats_strip(on, cx),
                        Command::MacShortcuts(active) => session.change_mac_shortcuts(active, cx),
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
                let open = self.ui.toggle(section);
                if section == Section::Stats {
                    Self::send(self.owner.clone(), Command::Stats(open), cx);
                }
                self.remember_sections(cx);
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
            ControlsAction::SelectAudioOutput(uid) => {
                Self::send(self.owner.clone(), Command::AudioOutput(uid), cx)
            }
            ControlsAction::SetAudioBitrate(bps) => {
                Self::send(self.owner.clone(), Command::AudioBitrate(bps), cx)
            }
            ControlsAction::SetStatsStrip(on) => {
                Self::send(self.owner.clone(), Command::StatsStrip(on), cx)
            }
            ControlsAction::SetMacShortcuts(active) => {
                Self::send(self.owner.clone(), Command::MacShortcuts(active), cx)
            }
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

    /// Keeps the open sections for this server through the write queue (its in-memory copy
    /// at once, the file after). A failed save shows in the session window's banner.
    fn remember_sections(&self, cx: &mut Context<Self>) {
        let sections = ControlsSections::from_array(self.ui.sections());
        let server = self.owner.session.read(cx).server().to_string();
        let done = cx
            .global::<WriteQueue>()
            .preferences(&server, move |p| p.controls = sections);
        let owner = self.owner.clone();
        cx.spawn(async move |_, cx| {
            if !matches!(done.await, Ok(Ok(()))) {
                let _ = owner.window.update(cx, |_, _, cx| {
                    owner.session.update(cx, |session, cx| {
                        session
                            .set_status("Couldn’t save the open Session Controls sections.".into());
                        cx.notify();
                    })
                });
            }
        })
        .detach();
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
        let audio = AudioInputs {
            status: session.audio_output.as_ref(),
            chosen: session.preferences.audio.output_device.as_deref(),
            saved_name: session.preferences.audio.output_device_name.as_deref(),
            bitrate: session
                .settings
                .as_ref()
                .and_then(|settings| settings.audio_bitrate.as_ref()),
            unavailable: session.audio_unavailable,
        };

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
            audio_unavailable: audio_header_line(&audio),
            input_switches,
            chords,
            clipboard,
            screen,
            video_settings,
            audio_settings: audio_props(&audio),
            stats,
            stats_copied: self.copied.is_some(),
            stats_strip: session.preferences.stats_strip,
            shortcuts: shortcuts_props(session.mac_shortcuts(), session.shortcuts_locked()),
        }
    }
}

/// The Shortcuts card, from `shortcuts::CHORDS` (`ARCH-031`).
fn shortcuts_props(mac_active: bool, locked: bool) -> ShortcutsProps {
    let rows = shortcuts::CHORDS
        .iter()
        .map(|chord| {
            let (modifiers, key) = shortcuts::glyphs(chord.keystroke);
            ShortcutRow {
                label: chord.label.into(),
                modifiers,
                key,
                remote: (!chord.always_local)
                    .then(|| format!("Sent as {}", shortcuts::remote_label(chord.keystroke))),
                always_local: chord.always_local,
            }
        })
        .collect();
    ShortcutsProps {
        mac_active,
        locked,
        rows,
    }
}

/// What the Audio Settings card is drawn from.
struct AudioInputs<'a> {
    status: Option<&'a OutputStatus>,
    /// The saved choice (its UID); `None` is System Default.
    chosen: Option<&'a str>,
    saved_name: Option<&'a str>,
    bitrate: Option<&'a AudioBitrate>,
    /// Audio is off this session (server or decoder), as the card says it.
    unavailable: Option<&'a str>,
}

impl AudioInputs<'_> {
    /// The saved device when it isn't connected.
    fn missing(&self, status: &OutputStatus) -> bool {
        self.chosen.is_some_and(|uid| status.device(uid).is_none())
    }

    /// The status's problem, if it answers the current choice (a pick waits for the
    /// audio thread for a moment; until then the old choice's problem is stale).
    fn problem(&self, status: &OutputStatus) -> Option<OutputProblem> {
        status
            .problem
            .filter(|_| status.chosen.as_deref() == self.chosen)
    }
}

/// The muted line under the header buttons: only while no sound can play, since the Audio
/// card is collapsed by default and would hide it.
fn audio_header_line(audio: &AudioInputs) -> Option<String> {
    if let Some(reason) = audio.unavailable {
        return Some(reason.trim_end_matches('.').to_string());
    }
    let status = audio.status?;
    match audio.problem(status) {
        Some(OutputProblem::Failed) => Some("Audio output stopped".into()),
        Some(OutputProblem::ChosenFailing) if status.active.is_none() => {
            Some("Audio output stopped".into())
        }
        _ if status.devices.is_empty() => Some("No audio output".into()),
        _ => None,
    }
}

/// The Audio Settings card; `None` until the session's audio is ready (`ARCH-GAP-DC-03`).
fn audio_props(audio: &AudioInputs) -> Option<AudioProps> {
    if let Some(reason) = audio.unavailable {
        return Some(AudioProps {
            output_label: String::new(),
            outputs: Vec::new(),
            note: None,
            bitrate: None,
            unavailable: Some(reason.to_string()),
        });
    }
    let status = audio.status?;
    let default_label = status.default_device().map_or_else(
        || "System Default".to_string(),
        |device| format!("System Default ({})", device.name),
    );
    let missing = audio.missing(status);
    let saved = audio.saved_name.unwrap_or("Saved Device");
    let missing_label = format!("{saved} (not connected)");
    let mut outputs = vec![AudioOutputItem {
        uid: None,
        label: default_label.clone(),
        checked: audio.chosen.is_none(),
        connected: true,
    }];
    outputs.extend(status.devices.iter().map(|device| AudioOutputItem {
        uid: Some(device.uid.clone()),
        label: device.name.clone(),
        checked: audio.chosen == Some(device.uid.as_str()),
        connected: true,
    }));
    if missing {
        outputs.push(AudioOutputItem {
            uid: audio.chosen.map(str::to_string),
            label: missing_label.clone(),
            checked: true,
            connected: false,
        });
    }
    let output_label = match audio.chosen {
        None => default_label,
        Some(_) if missing => missing_label,
        Some(uid) => status
            .device(uid)
            .map_or_else(|| saved.to_string(), |device| device.name.clone()),
    };
    let subject = audio
        .saved_name
        .map_or_else(|| "Your saved output device".to_string(), str::to_string);
    let playing = status.active.as_ref().map(|device| device.name.as_str());
    let note = match (audio.problem(status), playing) {
        (Some(OutputProblem::Failed), _) | (Some(OutputProblem::ChosenFailing), None) => {
            Some("Audio output stopped. Trying again…".into())
        }
        _ if missing => Some(match playing {
            Some(active) => {
                format!("{subject} isn’t connected. Playing through {active} until it’s back.")
            }
            None => format!(
                "{subject} isn’t connected, and no other audio output is available. Audio \
                 resumes when one is."
            ),
        }),
        _ if status.devices.is_empty() => {
            Some("No audio output is connected. Audio resumes when one is.".into())
        }
        (Some(OutputProblem::ChosenFailing), Some(active)) => Some(format!(
            "{} isn’t responding. Playing through {active} while Skerry keeps trying.",
            audio
                .chosen
                .and_then(|uid| status.device(uid))
                .map_or(saved, |d| d.name.as_str())
        )),
        _ => None,
    };
    Some(AudioProps {
        output_label,
        outputs,
        note,
        bitrate: audio.bitrate.map(|bitrate| AudioBitrateProps {
            value: bitrate.value,
            choices: bitrate.choices.clone(),
            locked: !bitrate.editable(),
        }),
        unavailable: None,
    })
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
        let strip_owner = owner.clone();
        let scroll = owner.session.read(cx).controls_scroll.clone();
        let root = div()
            .id("session-controls")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .vertical_scrollbar(&scroll)
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
            })
            .on_action(move |_: &ToggleStatsStrip, _, cx| {
                // View → Toggle Stats Over Stream while the panel is key.
                let on = !strip_owner.session.read(cx).preferences.stats_strip;
                Self::send(strip_owner.clone(), Command::StatsStrip(on), cx)
            });

        let session = owner.session.read(cx);
        // A reconnect empties the panel for a moment (no stream, then no settings), and GPUI
        // would clamp the offset to the short content. Keep it, and put it back as the
        // content refills (`next_scroll`).
        let complete = session.current.is_some() && session.settings.is_some();
        let (saved, apply) = next_scroll(
            self.scroll_saved,
            complete,
            scroll.offset(),
            scroll.max_offset().y,
        );
        self.scroll_saved = saved;
        if let Some(offset) = apply {
            scroll.set_offset(offset);
        }
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

/// One frame of the scroll keeper: `(offset to keep, offset to apply)`. While the content is
/// incomplete the current offset is kept (once, before GPUI clamps it). Once complete, the
/// kept offset is applied each frame until the content is tall enough for it, or until the
/// person scrolls (the offset is no longer what applying it left). GPUI offsets are
/// negative; `max_y` is the largest scroll distance the last layout allowed.
fn next_scroll(
    saved: Option<Point<Pixels>>,
    complete: bool,
    current: Point<Pixels>,
    max_y: Pixels,
) -> (Option<Point<Pixels>>, Option<Point<Pixels>>) {
    if !complete {
        return (saved.or(Some(current)), None);
    }
    let Some(saved) = saved else {
        return (None, None);
    };
    let left = saved.y.max(-max_y);
    if (f32::from(current.y) - f32::from(left)).abs() > 0.5 {
        return (None, None);
    }
    if max_y >= -saved.y {
        (None, Some(saved))
    } else {
        (Some(saved), Some(saved))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::OutputDevice;
    use gpui_kit::px;

    fn device(uid: &str, name: &str) -> OutputDevice {
        OutputDevice {
            uid: uid.into(),
            name: name.into(),
        }
    }

    /// Speakers (the default) and AirPods connected, playing through `active`.
    fn status(chosen: Option<&str>, active: Option<&str>) -> OutputStatus {
        let devices = vec![
            device("AirPods", "AirPods Pro"),
            device("BuiltIn", "MacBook Pro Speakers"),
        ];
        OutputStatus {
            active: active.and_then(|uid| devices.iter().find(|d| d.uid == uid).cloned()),
            devices,
            default_uid: Some("BuiltIn".into()),
            chosen: chosen.map(str::to_string),
            problem: None,
        }
    }

    fn inputs<'a>(
        status: Option<&'a OutputStatus>,
        chosen: Option<&'a str>,
        saved_name: Option<&'a str>,
    ) -> AudioInputs<'a> {
        AudioInputs {
            status,
            chosen,
            saved_name,
            bitrate: None,
            unavailable: None,
        }
    }

    #[test]
    fn audio_props_label_system_default_with_active_device() {
        let status = status(None, Some("BuiltIn"));
        let props = audio_props(&inputs(Some(&status), None, None)).unwrap();
        assert_eq!(props.output_label, "System Default (MacBook Pro Speakers)");
        let labels: Vec<_> = props
            .outputs
            .iter()
            .map(|o| (o.label.as_str(), o.checked, o.connected))
            .collect();
        assert_eq!(
            labels,
            [
                ("System Default (MacBook Pro Speakers)", true, true),
                ("AirPods Pro", false, true),
                ("MacBook Pro Speakers", false, true),
            ]
        );
        assert_eq!(props.note, None);
        // A chosen device shows its name only, even when it is also the default.
        let chosen = status_with_chosen("BuiltIn");
        let props = audio_props(&inputs(Some(&chosen), Some("BuiltIn"), None)).unwrap();
        assert_eq!(props.output_label, "MacBook Pro Speakers");
        assert!(props.outputs[2].checked && !props.outputs[0].checked);
    }

    fn status_with_chosen(uid: &str) -> OutputStatus {
        status(Some(uid), Some(uid))
    }

    #[test]
    fn audio_props_mark_missing_saved_device_and_note_fallback() {
        let mut status = status(Some("USB"), Some("BuiltIn"));
        status.problem = Some(OutputProblem::ChosenMissing);
        let props = audio_props(&inputs(Some(&status), Some("USB"), Some("USB DAC"))).unwrap();
        assert_eq!(props.output_label, "USB DAC (not connected)");
        let last = props.outputs.last().unwrap();
        assert_eq!(
            (last.label.as_str(), last.checked, last.connected),
            ("USB DAC (not connected)", true, false)
        );
        assert!(!props.outputs[0].checked);
        assert_eq!(
            props.note.as_deref(),
            Some("USB DAC isn’t connected. Playing through MacBook Pro Speakers until it’s back.")
        );
        // A name never seen.
        let props = audio_props(&inputs(Some(&status), Some("USB"), None)).unwrap();
        assert_eq!(props.output_label, "Saved Device (not connected)");
        assert!(props
            .note
            .unwrap()
            .starts_with("Your saved output device isn’t connected."));
    }

    #[test]
    fn audio_props_missing_device_with_no_other_output() {
        let status = OutputStatus {
            chosen: Some("USB".into()),
            problem: Some(OutputProblem::ChosenMissing),
            ..OutputStatus::default()
        };
        let audio = inputs(Some(&status), Some("USB"), Some("USB DAC"));
        let props = audio_props(&audio).unwrap();
        assert_eq!(
            props.note.as_deref(),
            Some(
                "USB DAC isn’t connected, and no other audio output is available. Audio \
                 resumes when one is."
            )
        );
        assert_eq!(props.outputs.len(), 2);
        assert_eq!(
            audio_header_line(&audio).as_deref(),
            Some("No audio output")
        );
    }

    #[test]
    fn audio_props_hide_bitrate_when_server_does_not_expose_it() {
        let status = status(None, Some("BuiltIn"));
        let props = audio_props(&inputs(Some(&status), None, None)).unwrap();
        assert_eq!(props.bitrate, None);
        let bitrate = AudioBitrate {
            value: 96_000,
            choices: vec![96_000],
            locked: false,
        };
        let mut audio = inputs(Some(&status), None, None);
        audio.bitrate = Some(&bitrate);
        assert_eq!(
            audio_props(&audio).unwrap().bitrate,
            Some(AudioBitrateProps {
                value: 96_000,
                choices: vec![96_000],
                locked: true
            })
        );
    }

    #[test]
    fn audio_props_show_only_reason_when_audio_disabled_by_server() {
        let mut audio = inputs(None, None, None);
        audio.unavailable = Some("Audio turned off by the server");
        let props = audio_props(&audio).unwrap();
        assert_eq!(
            props.unavailable.as_deref(),
            Some("Audio turned off by the server")
        );
        assert!(props.outputs.is_empty() && props.bitrate.is_none());
        audio.unavailable = Some("Audio couldn’t start on this Mac.");
        assert_eq!(
            audio_header_line(&audio).as_deref(),
            Some("Audio couldn’t start on this Mac")
        );
        // Before the session is ready the card waits.
        assert_eq!(audio_props(&inputs(None, None, None)), None);
    }

    #[test]
    fn audio_props_header_line_only_when_no_sound_can_play() {
        let playing = status(None, Some("BuiltIn"));
        assert_eq!(audio_header_line(&inputs(Some(&playing), None, None)), None);
        let mut missing = status(Some("USB"), Some("BuiltIn"));
        missing.problem = Some(OutputProblem::ChosenMissing);
        assert_eq!(
            audio_header_line(&inputs(Some(&missing), Some("USB"), None)),
            None,
            "sound still plays"
        );
        let none = OutputStatus {
            problem: Some(OutputProblem::NoDevice),
            ..OutputStatus::default()
        };
        let audio = inputs(Some(&none), None, None);
        assert_eq!(
            audio_header_line(&audio).as_deref(),
            Some("No audio output")
        );
        assert_eq!(
            audio_props(&audio).unwrap().note.as_deref(),
            Some("No audio output is connected. Audio resumes when one is.")
        );
        let mut failed = status(None, None);
        failed.problem = Some(OutputProblem::Failed);
        let audio = inputs(Some(&failed), None, None);
        assert_eq!(
            audio_header_line(&audio).as_deref(),
            Some("Audio output stopped")
        );
        assert_eq!(
            audio_props(&audio).unwrap().note.as_deref(),
            Some("Audio output stopped. Trying again…")
        );
    }

    #[test]
    fn a_failing_choice_with_nothing_playing_says_so() {
        let mut stuck = status(Some("AirPods"), None);
        stuck.problem = Some(OutputProblem::ChosenFailing);
        let audio = inputs(Some(&stuck), Some("AirPods"), None);
        assert_eq!(
            audio_header_line(&audio).as_deref(),
            Some("Audio output stopped")
        );
        assert_eq!(
            audio_props(&audio).unwrap().note.as_deref(),
            Some("Audio output stopped. Trying again…")
        );
    }

    #[test]
    fn a_fresh_pick_ignores_the_previous_choices_problem() {
        // Picked System Default; the thread still reports the failing AirPods choice.
        let mut stale = status(Some("AirPods"), Some("BuiltIn"));
        stale.problem = Some(OutputProblem::ChosenFailing);
        let props = audio_props(&inputs(Some(&stale), None, None)).unwrap();
        assert_eq!(props.note, None);
        let props = audio_props(&inputs(Some(&stale), Some("AirPods"), None)).unwrap();
        assert_eq!(
            props.note.as_deref(),
            Some("AirPods Pro isn’t responding. Playing through MacBook Pro Speakers while Skerry keeps trying.")
        );
    }

    fn at(y: f32) -> Point<Pixels> {
        point(px(0.), px(y))
    }

    #[test]
    fn scroll_offset_survives_an_empty_frame() {
        // Scrolled 600 down, then a reconnect empties the panel.
        let (saved, apply) = next_scroll(None, false, at(-600.), px(900.));
        assert_eq!((saved, apply), (Some(at(-600.)), None));
        // GPUI clamps the empty panel to 0; the kept offset stays.
        let (saved, apply) = next_scroll(saved, false, at(0.), px(0.));
        assert_eq!((saved, apply), (Some(at(-600.)), None));
        // Settings arrive: the content is 300 tall so far. Apply, and keep waiting.
        let (saved, apply) = next_scroll(saved, true, at(0.), px(0.));
        assert_eq!((saved, apply), (Some(at(-600.)), Some(at(-600.))));
        let (saved, apply) = next_scroll(saved, true, at(-300.), px(300.));
        assert_eq!((saved, apply), (Some(at(-600.)), Some(at(-600.))));
        // The policies refill the rest: the offset fits, applied one last time.
        let (saved, apply) = next_scroll(saved, true, at(-600.), px(900.));
        assert_eq!((saved, apply), (None, Some(at(-600.))));
        assert_eq!(next_scroll(saved, true, at(-600.), px(900.)), (None, None));
    }

    #[test]
    fn scrolling_during_the_refill_wins() {
        let saved = Some(at(-600.));
        // Content can scroll 300 but the person moved to 100.
        assert_eq!(next_scroll(saved, true, at(-100.), px(300.)), (None, None));
    }
}
