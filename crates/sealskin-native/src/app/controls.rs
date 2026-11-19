use super::{
    native_window, CloseSessionControls, Colors, DisplayMode, Launcher, ToggleFullScreen,
    ToggleSessionControls,
};
use crate::{settings::Key, transport::Resolution};
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
    video: bool,
    stats: bool,
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
    Setting(Key, u16),
    ResetVideo,
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
                    Self {
                        owner,
                        focus: cx.focus_handle(),
                        fullscreen,
                        screen: true,
                        video: true,
                        stats: false,
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
                        launcher.display_mode = DisplayMode::Fit;
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
                    Command::Setting(key, value) => launcher.change_setting(key, value, cx),
                    Command::ResetVideo => launcher.reset_settings(cx),
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

    fn display_settings(&self, launcher: &Launcher, colors: Colors) -> Div {
        let row = div().flex().gap_2().flex_wrap();
        if !launcher.resize_allowed {
            return row.child(
                div()
                    .text_color(rgb(colors.muted))
                    .child("Remote size controlled by server"),
            );
        }
        row.children(
            [
                (DisplayMode::Fit, "Fit"),
                (DisplayMode::MatchWindow, "Match Window"),
                (DisplayMode::Retina, "Match Retina"),
                (DisplayMode::Fixed(Resolution(1280, 720)), "1280×720"),
                (DisplayMode::Fixed(Resolution(1920, 1080)), "1920×1080"),
                (DisplayMode::Fixed(Resolution(3840, 2160)), "3840×2160"),
            ]
            .into_iter()
            .map(|(mode, label)| {
                let owner = self.owner;
                colors
                    .button(label, label, launcher.display_mode == mode)
                    .on_click(move |_, _, cx| Self::send(owner, Command::Display(mode), cx))
            }),
        )
    }

    fn video_settings(&self, launcher: &Launcher, colors: Colors) -> Div {
        let panel = div().flex().flex_col().gap_2();
        let Some(settings) = &launcher.settings else {
            return panel.child("Video settings are unavailable for this connection.");
        };
        let owner = self.owner;
        panel
            .children(
                [
                    (Key::Fps, "Frame rate cap", "fps", 5),
                    (Key::Jpeg, "JPEG quality", "%", 5),
                    (Key::PaintOver, "Improve still areas", "", 1),
                    (Key::PaintQuality, "Still image quality", "%", 5),
                ]
                .into_iter()
                .filter_map(|(key, label, unit, step)| {
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
                    } else {
                        values = values.child(format!("{value} {unit}"));
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
            "Screen",
            self.screen,
            || self.display_settings(launcher, colors),
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
                            format!("{:.1} ms per stripe", stats.decode_ms),
                        ),
                        ("Rejected stripes", stats.dropped.to_string()),
                        (
                            "Round trip",
                            stats
                                .rtt_ms
                                .map_or("Unavailable".into(), |ms| format!("{ms:.0} ms")),
                        ),
                        ("Transport", "JPEG over WebSocket".into()),
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
