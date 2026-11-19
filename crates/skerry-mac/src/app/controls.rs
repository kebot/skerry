use super::{
    native_window, CloseSessionControls, Colors, DisplayMode, Launcher, ToggleFullScreen,
    ToggleSessionControls,
};
use crate::input::{InputFeature, InputSettings, RemoteChord};
use crate::video::Codec;
use crate::{
    settings::{Key, ScreenChange},
    transport::Resolution,
};
use gpui::{
    div, point, prelude::*, px, rgb, size, App, Bounds, Context, Div, FocusHandle, FontWeight,
    SharedString, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions,
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
    width: Dimension,
    height: Dimension,
    size_error: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Picker {
    Preset,
    Dpi,
}

struct Dimension {
    focus: FocusHandle,
    value: String,
    replace: bool,
}

impl Dimension {
    fn new(cx: &mut Context<ControlPanel>) -> Self {
        Self {
            focus: cx.focus_handle(),
            value: String::new(),
            replace: false,
        }
    }
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
    ) -> gpui::Result<WindowHandle<Self>> {
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
        cx.open_window(
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
                    let mut width = Dimension::new(cx);
                    let mut height = Dimension::new(cx);
                    if let Ok(launcher) = owner.read(cx) {
                        if let DisplayMode::Fixed(size) = launcher.display_mode {
                            width.value = size.0.to_string();
                            height.value = size.1.to_string();
                        }
                    }
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
                window.focus(&panel.read(cx).focus);
                window.activate_window();
                panel
            },
        )
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

    fn action(&self, colors: Colors, label: &'static str, command: Command) -> gpui::Stateful<Div> {
        let owner = self.owner;
        colors
            .button(label, label, false)
            .on_click(move |_, _, cx| Self::send(owner, command, cx))
    }

    fn picker_button(
        &self,
        picker: Picker,
        label: String,
        colors: Colors,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let id = match picker {
            Picker::Preset => "preset-picker",
            Picker::Dpi => "dpi-picker",
        };
        colors
            .button(id, label, self.picker == Some(picker))
            .w_full()
            .flex()
            .justify_between()
            .child("⌄")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.picker = (this.picker != Some(picker)).then_some(picker);
                cx.notify();
            }))
    }

    fn input_field(
        &self,
        horizontal: bool,
        colors: Colors,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let (field, label, example) = if horizontal {
            (&self.width, "Width", "e.g. 1920")
        } else {
            (&self.height, "Height", "e.g. 1080")
        };
        let focus = field.focus.clone();
        let focused = focus.is_focused(window);
        let text = if field.value.is_empty() {
            example.to_string()
        } else {
            field.value.clone()
        };
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(label)
            .child(
                div()
                    .id(label)
                    .track_focus(&focus)
                    .border_1()
                    .rounded_md()
                    .py_2()
                    .px_3()
                    .cursor_text()
                    .border_color(rgb(if focused { colors.accent } else { colors.edge }))
                    .bg(rgb(colors.background))
                    .text_color(rgb(if field.value.is_empty() {
                        colors.muted
                    } else {
                        colors.ink
                    }))
                    .child(text)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let field = if horizontal {
                            &mut this.width
                        } else {
                            &mut this.height
                        };
                        window.focus(&field.focus);
                        field.replace = true;
                        cx.notify();
                    }))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            let field = if horizontal {
                                &mut this.width
                            } else {
                                &mut this.height
                            };
                            let key = &event.keystroke;
                            match (key.modifiers.platform, key.key.as_str()) {
                                (true, "a") => field.replace = true,
                                (true, "v") => {
                                    if let Some(paste) =
                                        cx.read_from_clipboard().and_then(|item| item.text())
                                    {
                                        let paste = paste.trim();
                                        if paste.len() <= 4
                                            && paste.bytes().all(|byte| byte.is_ascii_digit())
                                        {
                                            field.value = paste.into();
                                            field.replace = false;
                                        }
                                    }
                                }
                                (false, "backspace" | "delete") => {
                                    if field.replace {
                                        field.value.clear();
                                    } else {
                                        field.value.pop();
                                    }
                                    field.replace = false;
                                }
                                (false, "enter") => this.apply_size(cx),
                                (false, "tab") => {
                                    let next = if horizontal {
                                        &mut this.height
                                    } else {
                                        &mut this.width
                                    };
                                    next.replace = true;
                                    window.focus(&next.focus);
                                }
                                (false, _)
                                    if key.key.len() == 1
                                        && key.key.as_bytes()[0].is_ascii_digit() =>
                                {
                                    if field.replace {
                                        field.value.clear();
                                    }
                                    field.replace = false;
                                    if field.value.len() < 4 {
                                        field.value.push_str(&key.key);
                                    }
                                }
                                _ => return,
                            }
                            cx.stop_propagation();
                            cx.notify();
                        },
                    )),
            )
    }

    fn apply_size(&mut self, cx: &mut Context<Self>) {
        let resolution = Resolution::manual(&self.width.value, &self.height.value);
        self.size_error = resolution.is_none();
        if let Some(size) = resolution {
            self.picker = None;
            self.width.value = size.0.to_string();
            self.height.value = size.1.to_string();
            Self::send(self.owner, Command::Display(DisplayMode::Fixed(size)), cx);
        }
        cx.notify();
    }

    fn display_settings(
        &self,
        launcher: &Launcher,
        colors: Colors,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let owner = self.owner;
        let matching_retina = launcher.display_mode == DisplayMode::Retina;
        let mut panel = div().flex().flex_col().gap_3();
        if launcher.resize_allowed {
            panel = panel.child(switch_row(
                colors,
                "HiDPI (Pixel Perfect)",
                matching_retina,
                Command::Retina(!matching_retina),
                owner,
            ));
        } else {
            panel = panel.child(
                div()
                    .text_color(rgb(colors.muted))
                    .child("Remote size controlled by server"),
            );
        }
        if let Some((aligned, locked)) = launcher.screen_policy.alignment {
            panel = panel.child(if locked {
                div()
                    .flex()
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
                    colors,
                    "Force Aligned Resolution",
                    aligned,
                    Command::Screen(ScreenChange::Alignment(!aligned)),
                    owner,
                )
            });
        }
        if let Some(dpi) = launcher.screen_policy.dpi {
            let mut label = dpi_label(dpi);
            let row = div().flex().flex_col().gap_1().child("UI Scaling");
            if launcher.screen_policy.dpi_choices.len() > 1 {
                let automatic = launcher.dpi_override.is_none();
                if automatic {
                    label.push_str(" · Match Mac Display");
                }
                panel = panel.child(
                    row.child(self.picker_button(Picker::Dpi, label, colors, cx))
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
                                        colors
                                            .button(
                                                SharedString::from(label.clone()),
                                                label,
                                                launcher.dpi_override == dpi,
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
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
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child("Preset")
                        .child(self.picker_button(Picker::Preset, preset, colors, cx))
                        .when(self.picker == Some(Picker::Preset), |row| {
                            row.child(colors.button("fit", "Keep Remote Size", false).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.picker = None;
                                    this.size_error = false;
                                    Self::send(owner, Command::Display(DisplayMode::Fit), cx);
                                    cx.notify();
                                }),
                            ))
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
                                    colors
                                        .button(SharedString::from(label.clone()), label, false)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.picker = None;
                                            this.width.value = size.0.to_string();
                                            this.height.value = size.1.to_string();
                                            this.apply_size(cx);
                                        }))
                                }),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(self.input_field(true, colors, window, cx))
                        .child(self.input_field(false, colors, window, cx)),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            colors
                                .button("apply-manual", "Set Manual Resolution", true)
                                .on_click(cx.listener(|this, _, _, cx| this.apply_size(cx))),
                        )
                        .child(
                            colors
                                .button("reset-to-window", "Reset to Window", false)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.picker = None;
                                    this.width.value.clear();
                                    this.height.value.clear();
                                    this.size_error = false;
                                    Self::send(owner, Command::ResetToWindow, cx);
                                    cx.notify();
                                })),
                        ),
                )
                .when(self.size_error, |panel| {
                    panel.child("Enter a width of 2–4096 and a height of 2–2160.")
                });
        }
        panel.child(
            self.action(
                colors,
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

    fn input_settings(&self, input: InputSettings, colors: Colors) -> Div {
        let owner = self.owner;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .children(InputFeature::ALL.into_iter().map(|(feature, label)| {
                let enabled = feature.enabled(input);
                switch_row(
                    colors,
                    label,
                    enabled,
                    Command::Input(feature, !enabled),
                    owner,
                )
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child("Send Key Combination")
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(self.action(
                                colors,
                                "Alt+Tab",
                                Command::Chord(RemoteChord::SwitchApp),
                            ))
                            .child(self.action(
                                colors,
                                "Ctrl+Alt+Delete",
                                Command::Chord(RemoteChord::ControlAltDelete),
                            )),
                    ),
            )
    }

    fn video_settings(&self, launcher: &Launcher, colors: Colors) -> Div {
        let panel = div().flex().flex_col().gap_2();
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
            .child("Codec")
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(settings.codecs.iter().map(|&choice| {
                        colors
                            .button(choice.wire(), choice.label(), choice == codec)
                            .on_click(move |_, _, cx| {
                                if choice != codec || choice != preferred {
                                    Self::send(owner, Command::Codec(choice), cx);
                                }
                            })
                    })),
            )
            .when(launcher.preferences.codec != codec, |panel| {
                panel.child(div().text_color(rgb(colors.muted)).child(format!(
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
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(label);
                    let mut values = div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_color(rgb(colors.muted));
                    if key == Key::PaintOver {
                        let text = if value == 0 { "Off" } else { "On" };
                        values = values.child(text);
                        if settings.request(key, 1 - value).is_some() {
                            let label = if value == 0 { "Turn On" } else { "Turn Off" };
                            values =
                                values.child(colors.button("paint-over", label, false).on_click(
                                    move |_, _, cx| {
                                        Self::send(owner, Command::Setting(key, 1 - value), cx);
                                    },
                                ));
                        }
                    } else if key == Key::RateControl {
                        for (mode, label) in [(0, "CBR"), (1, "CRF")] {
                            if value == mode || settings.request(key, mode).is_some() {
                                values = values.child(
                                    colors.button(label, label, value == mode).on_click(
                                        move |_, _, cx| {
                                            if value != mode {
                                                Self::send(owner, Command::Setting(key, mode), cx);
                                            }
                                        },
                                    ),
                                );
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
                                values = values.child(colors.button(id, sign, false).on_click(
                                    move |_, _, cx| {
                                        Self::send(owner, Command::Setting(key, next), cx);
                                    },
                                ));
                            }
                        }
                    }
                    Some(group.child(values))
                }),
            )
            .child(div().pt_1().child(self.action(
                colors,
                "Reset Video Settings",
                Command::ResetVideo,
            )))
    }
}

impl Render for ControlPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Colors::for_window(window);
        let owner = self.owner;
        let root = div()
            .id("session-controls")
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(colors.background))
            .text_color(rgb(colors.ink))
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
            .flex()
            .flex_wrap()
            .gap_2()
            .when_some(launcher.audio, |row, enabled| {
                row.child(self.action(
                    colors,
                    if enabled { "Mute" } else { "Unmute" },
                    Command::Audio(!enabled),
                ))
            })
            .when_some(launcher.video, |row, enabled| {
                row.child(self.action(
                    colors,
                    if enabled {
                        "Pause Video"
                    } else {
                        "Resume Video"
                    },
                    Command::Playback(!enabled),
                ))
            })
            .child(self.action(
                colors,
                if self.fullscreen {
                    "Exit Full Screen"
                } else {
                    "Full Screen"
                },
                Command::FullScreen,
            ))
            .child(self.action(colors, "Reconnect", Command::Reconnect))
            .child(self.action(colors, "Disconnect", Command::Disconnect));
        root.child(
            div()
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(session.app_name.clone()),
                )
                .child(actions)
                .when_some(launcher.audio_unavailable, |view, reason| {
                    view.child(div().text_color(rgb(colors.muted)).child(reason))
                }),
        )
        .child(section(
            colors,
            "Input",
            self.input,
            || self.input_settings(launcher.preferences.input, colors),
            cx.listener(|this, _, _, cx| {
                this.input = !this.input;
                cx.notify();
            }),
        ))
        .child(section(
            colors,
            "Screen Settings",
            self.screen,
            || self.display_settings(launcher, colors, window, cx),
            cx.listener(|this, _, _, cx| {
                this.screen = !this.screen;
                cx.notify();
            }),
        ))
        .child(section(
            colors,
            "Video",
            self.video,
            || self.video_settings(launcher, colors),
            cx.listener(|this, _, _, cx| {
                this.video = !this.video;
                cx.notify();
            }),
        ))
        .child(section(
            colors,
            "Stream Diagnostics",
            self.stats,
            || {
                let mut rows = div().flex().flex_col().gap_2();
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
                            .flex()
                            .justify_between()
                            .child(label)
                            .child(div().text_color(rgb(colors.muted)).child(value))
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
    colors: Colors,
    title: &'static str,
    open: bool,
    body: impl FnOnce() -> Div,
    toggle: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .border_t_1()
        .border_color(rgb(colors.edge))
        .child(
            div()
                .id(title)
                .px_4()
                .py_3()
                .bg(rgb(colors.panel))
                .cursor_pointer()
                .flex()
                .justify_between()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title)
                .child(if open { "▾" } else { "▸" })
                .on_click(toggle),
        )
        .when(open, |view| view.child(div().p_4().child(body())))
}

fn dpi_label(dpi: u16) -> String {
    format!("{}%", (u32::from(dpi) * 100 + 48) / 96)
}

fn switch_row(
    colors: Colors,
    label: &'static str,
    enabled: bool,
    command: Command,
    owner: WindowHandle<Launcher>,
) -> gpui::AnyElement {
    div()
        .id(label)
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .cursor_pointer()
        .child(label)
        .child(
            div()
                .flex()
                .items_center()
                .flex_none()
                .w(px(36.))
                .h(px(20.))
                .px(px(2.))
                .rounded_full()
                .border_1()
                .border_color(rgb(if enabled { colors.accent } else { colors.muted }))
                .bg(rgb(if enabled {
                    colors.accent
                } else {
                    colors.background
                }))
                .when(enabled, |track| track.justify_end())
                .child(div().size(px(14.)).rounded_full().bg(rgb(if enabled {
                    0xffffff
                } else {
                    colors.muted
                }))),
        )
        .on_click(move |_, _, cx| ControlPanel::send(owner, command, cx))
        .into_any_element()
}
