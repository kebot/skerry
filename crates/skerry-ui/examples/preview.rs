use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::{InputEvent, InputState},
    scroll::ScrollableElement as _,
    ActiveTheme as _, Sizable as _, StyledExt as _, Theme, ThemeMode,
};
use gpui_kit::{
    actions, div, point, prelude::*, px, rgb, size, App, Bounds, Context, Entity, FontWeight,
    KeyBinding, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use skerry_ui::{
    connect::{self, ConnectProps},
    controls::{
        self, ChordItem, CodecChoice, ControlsAction, ControlsProps, ControlsUiState, ScreenProps,
        Section, SwitchItem, VideoProps, VideoSettingControl, VideoSettingRow,
    },
    library::{self, AppItem, LibraryHandlers, LibraryProps, SessionItem},
    session,
};
use std::rc::Rc;

actions!(skerry_preview, [Quit]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Story {
    ConnectDefault,
    ConnectBusy,
    ConnectError,
    LibraryPopulated,
    LibraryEmpty,
    LibraryBusy,
    ControlsHardware,
    ControlsJpeg,
    ControlsServerLocked,
    SessionOverlays,
}

impl Story {
    const ALL: &'static [Story] = &[
        Story::ConnectDefault,
        Story::ConnectBusy,
        Story::ConnectError,
        Story::LibraryPopulated,
        Story::LibraryEmpty,
        Story::LibraryBusy,
        Story::ControlsHardware,
        Story::ControlsJpeg,
        Story::ControlsServerLocked,
        Story::SessionOverlays,
    ];

    fn group(self) -> &'static str {
        match self {
            Story::ConnectDefault | Story::ConnectBusy | Story::ConnectError => "Connect",
            Story::LibraryPopulated | Story::LibraryEmpty | Story::LibraryBusy => "Library",
            Story::ControlsHardware | Story::ControlsJpeg | Story::ControlsServerLocked => {
                "Session Controls"
            }
            Story::SessionOverlays => "Session Overlays",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Story::ConnectDefault => "Default",
            Story::ConnectBusy => "Connecting",
            Story::ConnectError => "Error",
            Story::LibraryPopulated => "Populated",
            Story::LibraryEmpty => "Empty",
            Story::LibraryBusy => "Busy + Status",
            Story::ControlsHardware => "Hardware H.265",
            Story::ControlsJpeg => "JPEG + Paint-Over",
            Story::ControlsServerLocked => "Server Locked",
            Story::SessionOverlays => "Waiting + IME + Banner",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Story::ConnectDefault => "story-connect-default",
            Story::ConnectBusy => "story-connect-busy",
            Story::ConnectError => "story-connect-error",
            Story::LibraryPopulated => "story-library-populated",
            Story::LibraryEmpty => "story-library-empty",
            Story::LibraryBusy => "story-library-busy",
            Story::ControlsHardware => "story-controls-hardware",
            Story::ControlsJpeg => "story-controls-jpeg",
            Story::ControlsServerLocked => "story-controls-locked",
            Story::SessionOverlays => "story-session-overlays",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Appearance {
    System,
    Light,
    Dark,
}

struct PreviewApp {
    story: Story,
    appearance: Appearance,
    last_event: Option<String>,
    key_input: Entity<InputState>,
    controls_ui: ControlsUiState,
    controls_props: ControlsProps,
    width: Entity<InputState>,
    height: Entity<InputState>,
}

impl PreviewApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key_input = connect::new_key_input(window, cx);
        cx.subscribe_in(&key_input, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                let value = this.key_input.read(cx).value().to_string();
                let preview = if value.trim().is_empty() {
                    "Connect: Submit (empty key)".into()
                } else {
                    format!("Connect: Submit ({} chars)", value.trim().len())
                };
                this.record_event(preview, cx);
            }
        })
        .detach();
        let digit_filter =
            |s: &str, _: &mut App| s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit());
        let width = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("e.g. 1920")
                .validate(digit_filter)
        });
        let height = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("e.g. 1080")
                .validate(digit_filter)
        });
        cx.subscribe_in(&width, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.handle_controls_action(ControlsAction::ApplyManualSize, window, cx);
            }
        })
        .detach();
        cx.subscribe_in(
            &height,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.handle_controls_action(ControlsAction::ApplyManualSize, window, cx);
                }
            },
        )
        .detach();

        Self {
            story: Story::ConnectDefault,
            appearance: Appearance::System,
            last_event: None,
            key_input,
            controls_ui: ControlsUiState {
                input_open: false,
                screen_open: true,
                video_open: true,
                stats_open: false,
                picker: None,
                size_error: false,
            },
            controls_props: Self::fixture_controls(Story::ControlsHardware),
            width,
            height,
        }
    }

    fn select_story(&mut self, story: Story, window: &mut Window, cx: &mut Context<Self>) {
        self.story = story;
        self.last_event = None;
        if matches!(
            story,
            Story::ControlsHardware | Story::ControlsJpeg | Story::ControlsServerLocked
        ) {
            self.controls_props = Self::fixture_controls(story);
            self.controls_ui = ControlsUiState {
                input_open: false,
                screen_open: true,
                video_open: true,
                stats_open: matches!(story, Story::ControlsServerLocked),
                picker: None,
                size_error: false,
            };
            self.width.update(cx, |s, cx| s.set_value("", window, cx));
            self.height.update(cx, |s, cx| s.set_value("", window, cx));
        }
        cx.notify();
    }

    fn record_event(&mut self, event: impl Into<String>, cx: &mut Context<Self>) {
        self.last_event = Some(event.into());
        cx.notify();
    }

    fn set_appearance(
        &mut self,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance = appearance;
        match appearance {
            Appearance::System => Theme::sync_system_appearance(Some(window), cx),
            Appearance::Light => Theme::change(ThemeMode::Light, Some(window), cx),
            Appearance::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
        }
        cx.notify();
    }

    fn handle_controls_action(
        &mut self,
        action: ControlsAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.last_event = Some(format!("Controls: {action:?}"));
        match action {
            ControlsAction::ToggleSection(section) => match section {
                Section::Input => self.controls_ui.input_open = !self.controls_ui.input_open,
                Section::Screen => self.controls_ui.screen_open = !self.controls_ui.screen_open,
                Section::Video => self.controls_ui.video_open = !self.controls_ui.video_open,
                Section::Stats => self.controls_ui.stats_open = !self.controls_ui.stats_open,
            },
            ControlsAction::TogglePicker(picker) => {
                self.controls_ui.picker =
                    (self.controls_ui.picker != Some(picker)).then_some(picker);
            }
            ControlsAction::SetAudio(enabled) => self.controls_props.audio = Some(enabled),
            ControlsAction::SetPlayback(enabled) => self.controls_props.video = Some(enabled),
            ControlsAction::ToggleFullScreen => {
                self.controls_props.fullscreen = !self.controls_props.fullscreen;
            }
            ControlsAction::SetInput { id, enabled } => {
                if let Some(item) = self
                    .controls_props
                    .input_switches
                    .iter_mut()
                    .find(|item| item.id == id)
                {
                    item.enabled = enabled;
                }
            }
            ControlsAction::SetRetina(enabled) => {
                self.controls_props.screen.matching_retina = enabled;
                self.controls_props.screen.preset_label = "Match Window".into();
            }
            ControlsAction::SetAlignment(enabled) => {
                if let Some((aligned, locked)) = self.controls_props.screen.alignment.as_mut() {
                    if !*locked {
                        *aligned = enabled;
                    }
                }
            }
            ControlsAction::SetUiScaling(dpi) => {
                self.controls_ui.picker = None;
                self.controls_props.screen.dpi_override = dpi;
                if let Some(dpi) = dpi {
                    self.controls_props.screen.dpi = Some(dpi);
                }
            }
            ControlsAction::SetFitMode => {
                self.controls_ui.picker = None;
                self.controls_ui.size_error = false;
                self.controls_props.screen.preset_label = "Keep Remote Size".into();
            }
            ControlsAction::SelectPreset(w, h) => {
                self.controls_ui.picker = None;
                self.controls_ui.size_error = false;
                self.width
                    .update(cx, |s, cx| s.set_value(w.to_string(), window, cx));
                self.height
                    .update(cx, |s, cx| s.set_value(h.to_string(), window, cx));
                self.controls_props.screen.preset_label = format!("{w} × {h}");
                self.controls_props.screen.matching_retina = false;
            }
            ControlsAction::ApplyManualSize => {
                let w = self.width.read(cx).value().parse::<u16>().ok();
                let h = self.height.read(cx).value().parse::<u16>().ok();
                if let (Some(w), Some(h)) = (w, h) {
                    if (2..=4096).contains(&w) && (2..=2160).contains(&h) {
                        self.controls_ui.size_error = false;
                        self.controls_ui.picker = None;
                        self.controls_props.screen.preset_label = format!("{w} × {h}");
                        self.controls_props.screen.matching_retina = false;
                    } else {
                        self.controls_ui.size_error = true;
                    }
                } else {
                    self.controls_ui.size_error = true;
                }
            }
            ControlsAction::ResetToWindow => {
                self.controls_ui.picker = None;
                self.controls_ui.size_error = false;
                self.width.update(cx, |s, cx| s.set_value("", window, cx));
                self.height.update(cx, |s, cx| s.set_value("", window, cx));
                self.controls_props.screen.dpi_override = None;
                self.controls_props.screen.preset_label = "Match Window".into();
                self.controls_props.screen.matching_retina = true;
            }
            ControlsAction::SetScaleLocally(enabled) => {
                self.controls_props.screen.scale_locally = enabled;
            }
            ControlsAction::SelectCodec(wire) => {
                if let Some(video) = self.controls_props.video_settings.as_mut() {
                    for choice in &mut video.codecs {
                        choice.selected = choice.id == wire;
                    }
                }
            }
            ControlsAction::SetVideoSetting { id, value } => {
                if let Some(video) = self.controls_props.video_settings.as_mut() {
                    if let Some(row) = video.rows.iter_mut().find(|r| r.id == id) {
                        match &mut row.control {
                            VideoSettingControl::Toggle { enabled, .. } => *enabled = value != 0,
                            VideoSettingControl::Segmented { options } => {
                                for (mode, _, selected) in options {
                                    *selected = *mode == value;
                                }
                            }
                            VideoSettingControl::Stepper {
                                display,
                                decrement,
                                increment,
                            } => {
                                *display = format!("{value}");
                                *decrement = value.checked_sub(5);
                                *increment = Some(value + 5);
                            }
                        }
                    }
                }
            }
            ControlsAction::Reconnect
            | ControlsAction::Disconnect
            | ControlsAction::SendChord(_)
            | ControlsAction::ResetVideo => {}
        }
        cx.notify();
    }

    fn fixture_controls(story: Story) -> ControlsProps {
        let base_inputs = vec![
            SwitchItem {
                id: "Command sends Control".into(),
                label: "Command sends Control".into(),
                enabled: false,
            },
            SwitchItem {
                id: "Remote Cursor".into(),
                label: "Remote Cursor".into(),
                enabled: true,
            },
            SwitchItem {
                id: "Smooth Trackpad Scrolling".into(),
                label: "Smooth Trackpad Scrolling".into(),
                enabled: true,
            },
            SwitchItem {
                id: "Use Mac Input Method".into(),
                label: "Use Mac Input Method".into(),
                enabled: true,
            },
        ];
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

        match story {
            Story::ControlsJpeg => ControlsProps {
                app_name: "Zen Browser".into(),
                fullscreen: false,
                audio: Some(true),
                video: Some(true),
                audio_unavailable: None,
                input_switches: base_inputs,
                chords,
                screen: ScreenProps {
                    resize_allowed: true,
                    matching_retina: true,
                    alignment: Some((true, false)),
                    dpi: Some(192),
                    dpi_choices: vec![96, 144, 192],
                    dpi_override: None,
                    preset_label: "Match Window".into(),
                    scale_locally: false,
                },
                video_settings: Some(VideoProps {
                    codecs: vec![
                        CodecChoice {
                            id: "jpeg".into(),
                            label: "JPEG".into(),
                            selected: true,
                        },
                        CodecChoice {
                            id: "h264".into(),
                            label: "H.264".into(),
                            selected: false,
                        },
                        CodecChoice {
                            id: "h265".into(),
                            label: "H.265".into(),
                            selected: false,
                        },
                    ],
                    preferred_notice: Some("Preferred codec · H.265".into()),
                    rows: vec![
                        VideoSettingRow {
                            id: "framerate".into(),
                            label: "Frame rate cap".into(),
                            control: VideoSettingControl::Stepper {
                                display: "60 fps".into(),
                                decrement: Some(55),
                                increment: None,
                            },
                        },
                        VideoSettingRow {
                            id: "jpeg_quality".into(),
                            label: "JPEG quality".into(),
                            control: VideoSettingControl::Stepper {
                                display: "60 %".into(),
                                decrement: Some(55),
                                increment: Some(65),
                            },
                        },
                        VideoSettingRow {
                            id: "paint_over".into(),
                            label: "Improve still areas".into(),
                            control: VideoSettingControl::Toggle {
                                enabled: true,
                                can_toggle: true,
                            },
                        },
                        VideoSettingRow {
                            id: "paint_quality".into(),
                            label: "Still image quality".into(),
                            control: VideoSettingControl::Stepper {
                                display: "90 %".into(),
                                decrement: Some(85),
                                increment: Some(95),
                            },
                        },
                    ],
                }),
                diagnostics: Some(vec![
                    ("Resolution".into(), "1966×2160".into()),
                    ("Client FPS".into(), "58".into()),
                    ("Received bitrate".into(), "14.20 Mb/s".into()),
                    ("Decode time".into(), "4.3 ms per packet".into()),
                    ("Rejected packets".into(), "0".into()),
                    ("Round trip".into(), "12 ms".into()),
                    ("Transport".into(), "WebSocket".into()),
                    ("Client decoder".into(), "JPEG (Software)".into()),
                    ("Server encoder".into(), "jpeg".into()),
                ]),
            },
            Story::ControlsServerLocked => ControlsProps {
                app_name: "Locked Enterprise Desktop".into(),
                fullscreen: true,
                audio: None,
                video: Some(false),
                audio_unavailable: Some("Audio is disabled by server policy.".into()),
                input_switches: base_inputs,
                chords,
                screen: ScreenProps {
                    resize_allowed: false,
                    matching_retina: false,
                    alignment: Some((true, true)),
                    dpi: Some(96),
                    dpi_choices: vec![96],
                    dpi_override: None,
                    preset_label: "1280 × 720".into(),
                    scale_locally: true,
                },
                video_settings: None,
                diagnostics: None,
            },
            _ => ControlsProps {
                app_name: "Visual Studio Code".into(),
                fullscreen: false,
                audio: Some(true),
                video: Some(true),
                audio_unavailable: None,
                input_switches: base_inputs,
                chords,
                screen: ScreenProps {
                    resize_allowed: true,
                    matching_retina: true,
                    alignment: Some((false, false)),
                    dpi: Some(192),
                    dpi_choices: vec![96, 120, 144, 192],
                    dpi_override: None,
                    preset_label: "Match Window".into(),
                    scale_locally: false,
                },
                video_settings: Some(VideoProps {
                    codecs: vec![
                        CodecChoice {
                            id: "jpeg".into(),
                            label: "JPEG".into(),
                            selected: false,
                        },
                        CodecChoice {
                            id: "h264".into(),
                            label: "H.264".into(),
                            selected: false,
                        },
                        CodecChoice {
                            id: "h265".into(),
                            label: "H.265".into(),
                            selected: true,
                        },
                        CodecChoice {
                            id: "av1".into(),
                            label: "AV1".into(),
                            selected: false,
                        },
                    ],
                    preferred_notice: None,
                    rows: vec![
                        VideoSettingRow {
                            id: "framerate".into(),
                            label: "Frame rate cap".into(),
                            control: VideoSettingControl::Stepper {
                                display: "60 fps".into(),
                                decrement: Some(55),
                                increment: None,
                            },
                        },
                        VideoSettingRow {
                            id: "rate_control".into(),
                            label: "Rate control".into(),
                            control: VideoSettingControl::Segmented {
                                options: vec![(0, "CBR".into(), true), (1, "CRF".into(), false)],
                            },
                        },
                        VideoSettingRow {
                            id: "bitrate".into(),
                            label: "Target bitrate".into(),
                            control: VideoSettingControl::Stepper {
                                display: "8000 kbps".into(),
                                decrement: Some(7500),
                                increment: Some(8500),
                            },
                        },
                    ],
                }),
                diagnostics: Some(vec![
                    ("Resolution".into(), "1966×2160".into()),
                    ("Client FPS".into(), "60".into()),
                    ("Received bitrate".into(), "6.45 Mb/s".into()),
                    ("Decode time".into(), "0.8 ms per packet".into()),
                    ("Rejected packets".into(), "0".into()),
                    ("Round trip".into(), "9 ms".into()),
                    ("Transport".into(), "WebSocket".into()),
                    ("Client decoder".into(), "H.265 (VideoToolbox)".into()),
                    ("Server encoder".into(), "x265_vaapi".into()),
                ]),
            },
        }
    }

    fn render_story(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui_kit::Div {
        let entity = cx.entity().clone();
        match self.story {
            Story::ConnectDefault => {
                let props = ConnectProps::default();
                let entity = entity.clone();
                connect::render(theme, &props, &self.key_input, move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        let len = this.key_input.read(cx).value().trim().len();
                        this.record_event(format!("Connect: Submit ({len} chars)"), cx);
                    });
                })
            }
            Story::ConnectBusy => {
                let props = ConnectProps {
                    busy: Some("Connecting to sealskin.example.internal…".into()),
                    status: None,
                };
                connect::render(theme, &props, &self.key_input, |_, _, _| {})
            }
            Story::ConnectError => {
                let props = ConnectProps {
                    status: Some(
                        "Invalid application key format. Expected ssk_<id>_<secret>@<host>.".into(),
                    ),
                    busy: None,
                };
                let entity = entity.clone();
                connect::render(theme, &props, &self.key_input, move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        let len = this.key_input.read(cx).value().trim().len();
                        this.record_event(format!("Connect: Retry ({len} chars)"), cx);
                    });
                })
            }
            Story::LibraryPopulated => {
                let props = LibraryProps {
                    app_name: "SealSkin Native".into(),
                    server: "https://sealskin.example.internal".into(),
                    apps: vec![
                        AppItem {
                            id: "zen".into(),
                            name: "Zen Browser".into(),
                        },
                        AppItem {
                            id: "vscode".into(),
                            name: "Visual Studio Code".into(),
                        },
                        AppItem {
                            id: "firefox".into(),
                            name: "Firefox Developer Edition".into(),
                        },
                        AppItem {
                            id: "terminal".into(),
                            name: "Linux Desktop Terminal".into(),
                        },
                    ],
                    sessions: vec![
                        SessionItem {
                            id: "sess-01".into(),
                            app_name: "Zen Browser (Cleanroom)".into(),
                        },
                        SessionItem {
                            id: "sess-02".into(),
                            app_name: "Visual Studio Code".into(),
                        },
                    ],
                    status: None,
                    busy: None,
                };
                library::render(theme, &props, self.library_handlers(cx))
            }
            Story::LibraryEmpty => {
                let props = LibraryProps {
                    app_name: "SealSkin Native".into(),
                    server: "https://sealskin.example.internal".into(),
                    apps: vec![],
                    sessions: vec![],
                    status: None,
                    busy: None,
                };
                library::render(theme, &props, self.library_handlers(cx))
            }
            Story::LibraryBusy => {
                let props = LibraryProps {
                    app_name: "SealSkin Native".into(),
                    server: "https://sealskin.example.internal".into(),
                    apps: vec![AppItem {
                        id: "zen".into(),
                        name: "Zen Browser".into(),
                    }],
                    sessions: vec![],
                    status: Some("Previous launch timed out; retrying connection.".into()),
                    busy: Some("Launching Zen Browser…".into()),
                };
                library::render(theme, &props, self.library_handlers(cx))
            }
            Story::ControlsHardware | Story::ControlsJpeg | Story::ControlsServerLocked => {
                let handler = Rc::new(
                    move |action: ControlsAction, window: &mut Window, cx: &mut App| {
                        entity.update(cx, |this, cx| {
                            this.handle_controls_action(action, window, cx)
                        });
                    },
                );
                div()
                    .size_full()
                    .bg(theme.secondary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_6()
                    .child(
                        div()
                            .id("controls-preview-frame")
                            .w(px(388.))
                            .h(px(640.))
                            .max_h_full()
                            .overflow_y_scrollbar()
                            .rounded_xl()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .text_color(theme.foreground)
                            .text_sm()
                            .child(controls::render(
                                theme,
                                &self.controls_props,
                                &self.controls_ui,
                                &self.width,
                                &self.height,
                                handler,
                            )),
                    )
            }
            Story::SessionOverlays => {
                let entity = entity.clone();
                div()
                    .size_full()
                    .relative()
                    .bg(rgb(0x111318))
                    .child(session::waiting_overlay())
                    .child(session::ime_preedit(point(px(120.), px(96.)), "にほんご"))
                    .child(session::controls_trigger(
                        point(px(20.), px(20.)),
                        px(28.),
                        move |_, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.record_event("Session: Open Controls Trigger", cx);
                            });
                        },
                    ))
                    .child(session::status_banner(
                        theme,
                        "Reconnecting to Selkies stream (attempt 2)…",
                    ))
            }
        }
    }

    fn library_handlers(&self, cx: &mut Context<Self>) -> LibraryHandlers {
        let on_refresh_entity = cx.entity().clone();
        let on_forget_entity = cx.entity().clone();
        let on_launch_entity = cx.entity().clone();
        let on_resume_entity = cx.entity().clone();
        LibraryHandlers {
            on_refresh: Box::new(move |_, _, cx| {
                on_refresh_entity.update(cx, |this, cx| {
                    this.record_event("Library: Refresh", cx);
                });
            }),
            on_forget: Box::new(move |_, _, cx| {
                on_forget_entity.update(cx, |this, cx| {
                    this.record_event("Library: Remove Connection", cx);
                });
            }),
            on_launch: Rc::new(move |id, _, cx| {
                on_launch_entity.update(cx, |this, cx| {
                    this.record_event(format!("Library: Launch app `{id}`"), cx);
                });
            }),
            on_resume: Rc::new(move |id, _, cx| {
                on_resume_entity.update(cx, |this, cx| {
                    this.record_event(format!("Library: Resume session `{id}`"), cx);
                });
            }),
        }
    }
}

impl Render for PreviewApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active_story = self.story;
        let appearance = self.appearance;

        let mut gallery = div()
            .id("preview-sidebar")
            .w(px(220.))
            .flex_none()
            .h_full()
            .overflow_y_scrollbar()
            .p_3()
            .v_flex()
            .gap_1()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .px_2()
                    .pb_2()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Skerry UI Preview"),
            );

        let mut current_group = "";
        for &story in Story::ALL {
            if story.group() != current_group {
                current_group = story.group();
                gallery = gallery.child(
                    div()
                        .px_2()
                        .pt_3()
                        .pb_1()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted_foreground)
                        .child(current_group),
                );
            }
            let selected = story == active_story;
            let mut button = Button::new(story.id())
                .small()
                .w_full()
                .label(story.label())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_story(story, window, cx);
                }));
            if selected {
                button = button.primary();
            } else {
                button = button.ghost();
            }
            gallery = gallery.child(button);
        }

        let theme_button = |id: &'static str, label: &'static str, target: Appearance| {
            let mut button = Button::new(id).xsmall().label(label).on_click(cx.listener(
                move |this, _, window, cx| {
                    this.set_appearance(target, window, cx);
                },
            ));
            if appearance == target {
                button = button.primary();
            } else {
                button = button.outline();
            }
            button
        };

        let toolbar = div()
            .h_flex()
            .items_center()
            .justify_between()
            .gap_3()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(
                        SharedString::from(format!(
                            "{} / {}",
                            active_story.group(),
                            active_story.label()
                        )),
                    ))
                    .when_some(self.last_event.clone(), |bar, event| {
                        bar.child(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.popover)
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .truncate()
                                .child(event),
                        )
                    }),
            )
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_1()
                    .flex_none()
                    .child(theme_button("theme-system", "System", Appearance::System))
                    .child(theme_button("theme-light", "Light", Appearance::Light))
                    .child(theme_button("theme-dark", "Dark", Appearance::Dark)),
            );

        let stage = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .v_flex()
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_story(&theme, cx)),
            );

        div()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .flex()
            .child(gallery)
            .child(stage)
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(1220.), px(780.)), cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(760.), px(480.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Skerry UI Preview".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| PreviewApp::new(window, cx)),
            )
            .expect("open Skerry UI preview window");
            cx.activate(true);
        });
}
