use super::{
    native_window, CloseSessionControls, DisplayMode, Launcher, ToggleFullScreen,
    ToggleSessionControls,
};
use crate::input::{InputFeature, InputSettings, RemoteChord};
use crate::video::Codec;
use crate::{
    settings::{Key, ScreenChange},
    transport::Resolution,
};
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    collapsible::Collapsible,
    input::{Input, InputEvent, InputState},
    label::Label,
    scroll::ScrollableElement as _,
    switch::Switch,
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{
    div, point, prelude::*, px, size, AnyElement, AnyWindowHandle, App, Bounds, ClickEvent,
    Context, Div, Entity, FocusHandle, FontWeight, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowHandle, WindowKind, WindowOptions,
};
use objc2_app_kit::{NSWindowCollectionBehavior, NSWindowOrderingMode};

pub(super) struct ControlPanel {
    owner: WindowHandle<Launcher>,
    focus: FocusHandle,
    fullscreen: bool,
    screen: bool,
    input: bool,
    video: bool,
    stats: bool,
    picker: Option<Picker>,
    width: Entity<InputState>,
    height: Entity<InputState>,
    size_error: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Picker {
    Preset,
    Dpi,
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
}

impl ControlPanel {
    pub(super) fn open(
        window: &Window,
        cx: &mut Context<Launcher>,
    ) -> gpui_kit::Result<AnyWindowHandle> {
        let owner = window
            .window_handle()
            .downcast::<Launcher>()
            .expect("launcher window");
        let launcher = cx.entity();
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
                let panel = cx.new(|cx: &mut Context<Self>| {
                    cx.observe(&launcher, |_, _, cx| cx.notify()).detach();
                    let initial = owner.read(cx).ok().and_then(|launcher| {
                        if let DisplayMode::Fixed(size) = launcher.display_mode {
                            Some(size)
                        } else {
                            None
                        }
                    });
                    let digit_filter =
                        |s: &str, _: &mut App| s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit());
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
                    cx.subscribe_in(&height, window, |this, _, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            this.apply_size(window, cx);
                        }
                    })
                    .detach();
                    Self {
                        owner,
                        focus: cx.focus_handle(),
                        fullscreen,
                        screen: true,
                        input: false,
                        video: false,
                        stats: false,
                        picker: None,
                        width,
                        height,
                        size_error: false,
                    }
                });
                window.on_window_should_close(cx, move |_, cx| {
                    Self::send(owner, Command::Close, cx);
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

    fn send(owner: WindowHandle<Launcher>, command: Command, cx: &mut App) {
        // Window removal and focus changes are performed after the panel event dispatch finishes.
        cx.defer(move |cx| {
            let _ = owner.update(cx, |launcher, window, cx| {
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
                    }
                    Command::Codec(codec) => launcher.change_codec(codec, window, cx),
                    Command::Setting(key, value) => launcher.change_setting(key, value, cx),
                    Command::ResetVideo => launcher.reset_settings(cx),
                    Command::Input(feature, enabled) => {
                        launcher.change_input(feature, enabled, window, cx)
                    }
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
    }

    fn action(&self, label: &'static str, command: Command) -> Button {
        let owner = self.owner;
        Button::new(label)
            .outline()
            .small()
            .label(label)
            .on_click(move |_, _, cx| Self::send(owner, command, cx))
    }

    fn picker_button(&self, picker: Picker, label: String, cx: &Context<Self>) -> Button {
        let id = match picker {
            Picker::Preset => "preset-picker",
            Picker::Dpi => "dpi-picker",
        };
        let button = Button::new(id)
            .small()
            .w_full()
            .label(label)
            .dropdown_caret(true);
        let button = if self.picker == Some(picker) {
            button.primary()
        } else {
            button.outline()
        };
        button.on_click(cx.listener(move |this, _, _, cx| {
            this.picker = (this.picker != Some(picker)).then_some(picker);
            cx.notify();
        }))
    }

    fn input_field(&self, horizontal: bool) -> Div {
        let (field, label) = if horizontal {
            (&self.width, "Width")
        } else {
            (&self.height, "Height")
        };
        div()
            .flex_1()
            .min_w_0()
            .v_flex()
            .gap_1()
            .child(Label::new(label))
            .child(Input::new(field).small())
    }

    fn apply_size(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width_val = self.width.read(cx).value();
        let height_val = self.height.read(cx).value();
        let resolution = Resolution::manual(&width_val, &height_val);
        self.size_error = resolution.is_none();
        if let Some(size) = resolution {
            self.picker = None;
            self.width
                .update(cx, |s, cx| s.set_value(size.0.to_string(), window, cx));
            self.height
                .update(cx, |s, cx| s.set_value(size.1.to_string(), window, cx));
            Self::send(self.owner, Command::Display(DisplayMode::Fixed(size)), cx);
        }
        cx.notify();
    }

    fn display_settings(&self, launcher: &Launcher, theme: &Theme, cx: &Context<Self>) -> Div {
        let owner = self.owner;
        let matching_retina = launcher.display_mode == DisplayMode::Retina;
        let mut panel = div().v_flex().gap_3();
        if launcher.resize_allowed {
            panel = panel.child(switch_row(
                "HiDPI (Pixel Perfect)",
                matching_retina,
                Command::Retina(!matching_retina),
                owner,
            ));
        } else {
            panel = panel.child(
                div()
                    .text_color(theme.muted_foreground)
                    .child("Remote size controlled by server"),
            );
        }
        if let Some((aligned, locked)) = launcher.screen_policy.alignment {
            panel = panel.child(if locked {
                div()
                    .h_flex()
                    .justify_between()
                    .child("Force Aligned Resolution")
                    .child(if aligned {
                        "On · Locked"
                    } else {
                        "Off · Locked"
                    })
                    .into_any_element()
            } else {
                switch_row(
                    "Force Aligned Resolution",
                    aligned,
                    Command::Screen(ScreenChange::Alignment(!aligned)),
                    owner,
                )
            });
        }
        if let Some(dpi) = launcher.screen_policy.dpi {
            let mut label = dpi_label(dpi);
            let row = div().v_flex().gap_1().child(Label::new("UI Scaling"));
            if launcher.screen_policy.dpi_choices.len() > 1 {
                let automatic = launcher.dpi_override.is_none();
                if automatic {
                    label.push_str(" · Match Mac Display");
                }
                panel = panel.child(
                    row.child(self.picker_button(Picker::Dpi, label, cx))
                        .when(self.picker == Some(Picker::Dpi), |row| {
                            row.children(
                                std::iter::once(None)
                                    .chain(
                                        launcher
                                            .screen_policy
                                            .dpi_choices
                                            .iter()
                                            .copied()
                                            .map(Some),
                                    )
                                    .map(|dpi| {
                                        let label = dpi
                                            .map_or_else(|| "Match Mac Display".into(), dpi_label);
                                        let button = Button::new(SharedString::from(label.clone()))
                                            .small()
                                            .label(label);
                                        let button = if launcher.dpi_override == dpi {
                                            button.primary()
                                        } else {
                                            button.outline()
                                        };
                                        button.on_click(cx.listener(move |this, _, _, cx| {
                                            this.picker = None;
                                            Self::send(owner, Command::UiScaling(dpi), cx);
                                            cx.notify();
                                        }))
                                    }),
                            )
                        }),
                );
            } else {
                panel = panel.child(row.child(label));
            }
        }
        if launcher.resize_allowed {
            let preset = match launcher.display_mode {
                DisplayMode::Fixed(size) => format!("{} × {}", size.0, size.1),
                DisplayMode::Fit => "Keep Remote Size".into(),
                DisplayMode::MatchWindow | DisplayMode::Retina => "Match Window".into(),
            };
            panel = panel
                .child(
                    div()
                        .v_flex()
                        .gap_1()
                        .child(Label::new("Preset"))
                        .child(self.picker_button(Picker::Preset, preset, cx))
                        .when(self.picker == Some(Picker::Preset), |row| {
                            row.child(
                                Button::new("fit")
                                    .outline()
                                    .small()
                                    .label("Keep Remote Size")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.picker = None;
                                        this.size_error = false;
                                        Self::send(owner, Command::Display(DisplayMode::Fit), cx);
                                        cx.notify();
                                    })),
                            )
                            .children(
                                [
                                    Resolution(1280, 720),
                                    Resolution(1920, 1080),
                                    Resolution(2560, 1440),
                                    Resolution(3840, 2160),
                                ]
                                .into_iter()
                                .map(|size| {
                                    let label = format!("{} × {}", size.0, size.1);
                                    Button::new(SharedString::from(label.clone()))
                                        .outline()
                                        .small()
                                        .label(label)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.picker = None;
                                            this.width.update(cx, |s, cx| {
                                                s.set_value(size.0.to_string(), window, cx)
                                            });
                                            this.height.update(cx, |s, cx| {
                                                s.set_value(size.1.to_string(), window, cx)
                                            });
                                            this.apply_size(window, cx);
                                        }))
                                }),
                            )
                        }),
                )
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(self.input_field(true))
                        .child(self.input_field(false)),
                )
                .child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("apply-manual")
                                .primary()
                                .small()
                                .label("Set Manual Resolution")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.apply_size(window, cx)
                                })),
                        )
                        .child(
                            Button::new("reset-to-window")
                                .outline()
                                .small()
                                .label("Reset to Window")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.picker = None;
                                    this.width.update(cx, |s, cx| s.set_value("", window, cx));
                                    this.height.update(cx, |s, cx| s.set_value("", window, cx));
                                    this.size_error = false;
                                    Self::send(owner, Command::ResetToWindow, cx);
                                    cx.notify();
                                })),
                        ),
                )
                .when(self.size_error, |panel| {
                    panel.child(
                        div()
                            .text_color(theme.danger)
                            .child("Enter a width of 2–4096 and a height of 2–2160."),
                    )
                });
        }
        panel.child(
            self.action(
                if launcher.scale_locally {
                    "Scale Locally: On"
                } else {
                    "Scale Locally: Off"
                },
                Command::ScaleLocally(!launcher.scale_locally),
            )
            .w_full(),
        )
    }

    fn input_settings(&self, input: InputSettings) -> Div {
        let owner = self.owner;
        div()
            .v_flex()
            .gap_3()
            .children(InputFeature::ALL.into_iter().map(|(feature, label)| {
                let enabled = feature.enabled(input);
                switch_row(label, enabled, Command::Input(feature, !enabled), owner)
            }))
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(Label::new("Send Key Combination"))
                    .child(
                        div()
                            .h_flex()
                            .flex_wrap()
                            .gap_2()
                            .child(self.action("Alt+Tab", Command::Chord(RemoteChord::SwitchApp)))
                            .child(self.action(
                                "Ctrl+Alt+Delete",
                                Command::Chord(RemoteChord::ControlAltDelete),
                            )),
                    ),
            )
    }

    fn video_settings(&self, launcher: &Launcher, theme: &Theme) -> Div {
        let panel = div().v_flex().gap_2();
        let Some(settings) = &launcher.settings else {
            return panel.child("Video settings are unavailable for this connection.");
        };
        let owner = self.owner;
        let preferred = launcher.preferences.codec;
        let codec = launcher
            .stats
            .as_ref()
            .and_then(|stats| stats.client_codec)
            .unwrap_or_else(|| {
                if settings.codecs.contains(&preferred) {
                    preferred
                } else {
                    settings.codecs.first().copied().unwrap_or_default()
                }
            });
        let rate = launcher
            .pending_settings
            .get(&Key::RateControl)
            .copied()
            .or_else(|| {
                settings
                    .get(Key::RateControl)
                    .map(|setting| setting.current)
            });
        panel
            .child(Label::new("Codec"))
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(settings.codecs.iter().map(|&choice| {
                        let button = Button::new(choice.wire()).small().label(choice.label());
                        let button = if choice == codec {
                            button.primary()
                        } else {
                            button.outline()
                        };
                        button.on_click(move |_, _, cx| {
                            if choice != codec || choice != preferred {
                                Self::send(owner, Command::Codec(choice), cx);
                            }
                        })
                    })),
            )
            .when(launcher.preferences.codec != codec, |panel| {
                panel.child(div().text_color(theme.muted_foreground).child(format!(
                    "Preferred codec · {}",
                    launcher.preferences.codec.label()
                )))
            })
            .children(
                [
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
                    let control = settings.get(key)?;
                    let value = launcher
                        .pending_settings
                        .get(&key)
                        .copied()
                        .unwrap_or(control.current);
                    let group = div()
                        .h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(label);
                    let mut values = div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .text_color(theme.muted_foreground);
                    if key == Key::PaintOver {
                        let text = if value == 0 { "Off" } else { "On" };
                        values = values.child(text);
                        if settings.request(key, 1 - value).is_some() {
                            let label = if value == 0 { "Turn On" } else { "Turn Off" };
                            values = values.child(
                                Button::new("paint-over")
                                    .outline()
                                    .small()
                                    .label(label)
                                    .on_click(move |_, _, cx| {
                                        Self::send(owner, Command::Setting(key, 1 - value), cx);
                                    }),
                            );
                        }
                    } else if key == Key::RateControl {
                        for (mode, label) in [(0, "CBR"), (1, "CRF")] {
                            if value == mode || settings.request(key, mode).is_some() {
                                let button = Button::new(label).small().label(label);
                                let button = if value == mode {
                                    button.primary()
                                } else {
                                    button.outline()
                                };
                                values = values.child(button.on_click(move |_, _, cx| {
                                    if value != mode {
                                        Self::send(owner, Command::Setting(key, mode), cx);
                                    }
                                }));
                            }
                        }
                    } else {
                        values = values.child(format!("{value} {unit}").trim_end().to_owned());
                        for (next, sign) in [
                            (value.saturating_sub(step).max(control.min), "−"),
                            (value.saturating_add(step).min(control.max), "+"),
                        ] {
                            if next != value && settings.request(key, next).is_some() {
                                let id = SharedString::from(format!("{}{sign}", key.wire()));
                                values = values.child(
                                    Button::new(id)
                                        .outline()
                                        .small()
                                        .label(sign)
                                        .on_click(move |_, _, cx| {
                                            Self::send(owner, Command::Setting(key, next), cx);
                                        }),
                                );
                            }
                        }
                    }
                    Some(group.child(values))
                }),
            )
            .child(
                div()
                    .pt_1()
                    .child(self.action("Reset Video Settings", Command::ResetVideo)),
            )
    }
}

impl Render for ControlPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let owner = self.owner;
        let root = div()
            .id("session-controls")
            .size_full()
            .overflow_y_scrollbar()
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_sm()
            .track_focus(&self.focus)
            .key_context("SessionControls")
            .on_action(move |_: &CloseSessionControls, _, cx| Self::send(owner, Command::Close, cx))
            .on_action(move |_: &ToggleSessionControls, _, cx| {
                Self::send(owner, Command::Close, cx)
            })
            .on_action(move |_: &ToggleFullScreen, _, cx| {
                Self::send(owner, Command::FullScreen, cx)
            });
        let Ok(launcher) = owner.read(cx) else {
            return root;
        };
        let Some((session, _)) = &launcher.current else {
            return root;
        };
        let actions = div()
            .h_flex()
            .flex_wrap()
            .gap_2()
            .when_some(launcher.audio, |row, enabled| {
                row.child(self.action(
                    if enabled { "Mute" } else { "Unmute" },
                    Command::Audio(!enabled),
                ))
            })
            .when_some(launcher.video, |row, enabled| {
                row.child(self.action(
                    if enabled {
                        "Pause Video"
                    } else {
                        "Resume Video"
                    },
                    Command::Playback(!enabled),
                ))
            })
            .child(self.action(
                if self.fullscreen {
                    "Exit Full Screen"
                } else {
                    "Full Screen"
                },
                Command::FullScreen,
            ))
            .child(self.action("Reconnect", Command::Reconnect))
            .child(self.action("Disconnect", Command::Disconnect));
        root.child(
            div()
                .p_4()
                .v_flex()
                .gap_3()
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(session.app_name.clone()),
                )
                .child(actions)
                .when_some(launcher.audio_unavailable, |view, reason| {
                    view.child(div().text_color(theme.muted_foreground).child(reason))
                }),
        )
        .child(section(
            &theme,
            "Input",
            self.input,
            || self.input_settings(launcher.preferences.input),
            cx.listener(|this, _, _, cx| {
                this.input = !this.input;
                cx.notify();
            }),
        ))
        .child(section(
            &theme,
            "Screen Settings",
            self.screen,
            || self.display_settings(launcher, &theme, cx),
            cx.listener(|this, _, _, cx| {
                this.screen = !this.screen;
                cx.notify();
            }),
        ))
        .child(section(
            &theme,
            "Video",
            self.video,
            || self.video_settings(launcher, &theme),
            cx.listener(|this, _, _, cx| {
                this.video = !this.video;
                cx.notify();
            }),
        ))
        .child(section(
            &theme,
            "Stream Diagnostics",
            self.stats,
            || {
                let mut rows = div().v_flex().gap_2();
                if let Some(stats) = &launcher.stats {
                    let fields = [
                        ("Resolution", format!("{}×{}", stats.width, stats.height)),
                        ("Client FPS", format!("{:.0}", stats.fps)),
                        ("Received bitrate", format!("{:.2} Mb/s", stats.wire_mbps)),
                        (
                            "Decode time",
                            format!("{:.1} ms per packet", stats.decode_ms),
                        ),
                        ("Rejected packets", stats.dropped.to_string()),
                        (
                            "Round trip",
                            stats
                                .rtt_ms
                                .map_or("Unavailable".into(), |ms| format!("{ms:.0} ms")),
                        ),
                        ("Transport", "WebSocket".into()),
                        (
                            "Client decoder",
                            stats
                                .client_codec
                                .map_or("Waiting for video", Codec::decoder_label)
                                .into(),
                        ),
                        (
                            "Server encoder",
                            stats.server_encoder.clone().unwrap_or("Unreported".into()),
                        ),
                    ];
                    rows = rows.children(fields.into_iter().map(|(label, value)| {
                        div()
                            .h_flex()
                            .justify_between()
                            .child(label)
                            .child(div().text_color(theme.muted_foreground).child(value))
                    }));
                } else {
                    rows = rows.child("Waiting for stream diagnostics…");
                }
                rows
            },
            cx.listener(|this, _, _, cx| {
                this.stats = !this.stats;
                cx.notify();
            }),
        ))
    }
}

fn section(
    theme: &Theme,
    title: &'static str,
    open: bool,
    body: impl FnOnce() -> Div,
    toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .border_t_1()
        .border_color(theme.border)
        .child(
            div()
                .id(title)
                .px_4()
                .py_3()
                .bg(theme.secondary)
                .cursor_pointer()
                .h_flex()
                .items_center()
                .justify_between()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title)
                .child(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .on_click(toggle),
        )
        .child(
            Collapsible::new()
                .open(open)
                .motion_id(SharedString::from(format!("section-{title}")))
                .content(div().p_4().child(body())),
        )
}

fn dpi_label(dpi: u16) -> String {
    format!("{}%", (u32::from(dpi) * 100 + 48) / 96)
}

fn switch_row(
    label: &'static str,
    enabled: bool,
    command: Command,
    owner: WindowHandle<Launcher>,
) -> AnyElement {
    div()
        .h_flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(Label::new(label))
        .child(
            Switch::new(label)
                .checked(enabled)
                .on_click(move |_, _, cx| ControlPanel::send(owner, command, cx)),
        )
        .into_any_element()
}
