use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    collapsible::Collapsible,
    input::{Input, InputState},
    label::Label,
    switch::Switch,
    IconName, Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{
    div, prelude::*, AnyElement, App, ClickEvent, Div, Entity, FontWeight, SharedString, Window,
};
use std::rc::Rc;

pub type ControlsHandler = Rc<dyn Fn(ControlsAction, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picker {
    Preset,
    Dpi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Input,
    Screen,
    Video,
    Stats,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlsUiState {
    pub input_open: bool,
    pub screen_open: bool,
    pub video_open: bool,
    pub stats_open: bool,
    pub picker: Option<Picker>,
    pub size_error: bool,
}

impl Default for ControlsUiState {
    fn default() -> Self {
        Self {
            input_open: false,
            screen_open: true,
            video_open: false,
            stats_open: false,
            picker: None,
            size_error: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlsAction {
    ToggleSection(Section),
    TogglePicker(Picker),
    SetAudio(bool),
    SetPlayback(bool),
    ToggleFullScreen,
    Reconnect,
    Disconnect,
    SetInput { id: String, enabled: bool },
    SendChord(String),
    SetRetina(bool),
    SetAlignment(bool),
    SetUiScaling(Option<u16>),
    SetFitMode,
    SelectPreset(u16, u16),
    ApplyManualSize,
    ResetToWindow,
    SetScaleLocally(bool),
    SelectCodec(String),
    SetVideoSetting { id: String, value: u32 },
    ResetVideo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchItem {
    pub id: String,
    pub label: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChordItem {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenProps {
    pub resize_allowed: bool,
    pub matching_retina: bool,
    pub alignment: Option<(bool, bool)>,
    pub dpi: Option<u16>,
    pub dpi_choices: Vec<u16>,
    pub dpi_override: Option<u16>,
    pub preset_label: String,
    pub scale_locally: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecChoice {
    pub id: String,
    pub label: String,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoSettingControl {
    Toggle {
        enabled: bool,
        can_toggle: bool,
    },
    Segmented {
        options: Vec<(u32, String, bool)>,
    },
    Stepper {
        display: String,
        decrement: Option<u32>,
        increment: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoSettingRow {
    pub id: String,
    pub label: String,
    pub control: VideoSettingControl,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoProps {
    pub codecs: Vec<CodecChoice>,
    pub preferred_notice: Option<String>,
    pub rows: Vec<VideoSettingRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlsProps {
    pub app_name: String,
    pub fullscreen: bool,
    pub audio: Option<bool>,
    pub video: Option<bool>,
    pub audio_unavailable: Option<String>,
    pub input_switches: Vec<SwitchItem>,
    pub chords: Vec<ChordItem>,
    pub screen: ScreenProps,
    pub video_settings: Option<VideoProps>,
    pub diagnostics: Option<Vec<(String, String)>>,
}

pub fn dpi_label(dpi: u16) -> String {
    format!("{}%", (u32::from(dpi) * 100 + 48) / 96)
}

pub fn render(
    theme: &Theme,
    props: &ControlsProps,
    state: &ControlsUiState,
    width: &Entity<InputState>,
    height: &Entity<InputState>,
    on_action: ControlsHandler,
) -> Div {
    let actions = div()
        .h_flex()
        .flex_wrap()
        .gap_2()
        .when_some(props.audio, |row, enabled| {
            row.child(action_button(
                if enabled { "Mute" } else { "Unmute" },
                ControlsAction::SetAudio(!enabled),
                &on_action,
            ))
        })
        .when_some(props.video, |row, enabled| {
            row.child(action_button(
                if enabled {
                    "Pause Video"
                } else {
                    "Resume Video"
                },
                ControlsAction::SetPlayback(!enabled),
                &on_action,
            ))
        })
        .child(action_button(
            if props.fullscreen {
                "Exit Full Screen"
            } else {
                "Full Screen"
            },
            ControlsAction::ToggleFullScreen,
            &on_action,
        ))
        .child(action_button(
            "Reconnect",
            ControlsAction::Reconnect,
            &on_action,
        ))
        .child(action_button(
            "Disconnect",
            ControlsAction::Disconnect,
            &on_action,
        ));

    let toggle = |section_id: Section| {
        let on_action = Rc::clone(&on_action);
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            on_action(ControlsAction::ToggleSection(section_id), window, cx);
        }
    };

    div()
        .v_flex()
        .child(
            div()
                .p_4()
                .v_flex()
                .gap_3()
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(props.app_name.clone()),
                )
                .child(actions)
                .when_some(props.audio_unavailable.clone(), |view, reason| {
                    view.child(div().text_color(theme.muted_foreground).child(reason))
                }),
        )
        .child(section(
            theme,
            "Input",
            state.input_open,
            || render_input(props, &on_action),
            toggle(Section::Input),
        ))
        .child(section(
            theme,
            "Screen Settings",
            state.screen_open,
            || render_screen(theme, &props.screen, state, width, height, &on_action),
            toggle(Section::Screen),
        ))
        .child(section(
            theme,
            "Video",
            state.video_open,
            || render_video(theme, props.video_settings.as_ref(), &on_action),
            toggle(Section::Video),
        ))
        .child(section(
            theme,
            "Stream Diagnostics",
            state.stats_open,
            || render_diagnostics(theme, props.diagnostics.as_deref()),
            toggle(Section::Stats),
        ))
}

fn render_input(props: &ControlsProps, on_action: &ControlsHandler) -> Div {
    div()
        .v_flex()
        .gap_3()
        .children(props.input_switches.iter().map(|item| {
            switch_row(
                item.label.clone(),
                item.enabled,
                ControlsAction::SetInput {
                    id: item.id.clone(),
                    enabled: !item.enabled,
                },
                on_action,
            )
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
                        .children(props.chords.iter().map(|chord| {
                            action_button_owned(
                                chord.label.clone(),
                                ControlsAction::SendChord(chord.id.clone()),
                                on_action,
                            )
                        })),
                ),
        )
}

fn render_screen(
    theme: &Theme,
    screen: &ScreenProps,
    state: &ControlsUiState,
    width: &Entity<InputState>,
    height: &Entity<InputState>,
    on_action: &ControlsHandler,
) -> Div {
    let mut panel = div().v_flex().gap_3();
    if screen.resize_allowed {
        panel = panel.child(switch_row(
            "HiDPI (Pixel Perfect)",
            screen.matching_retina,
            ControlsAction::SetRetina(!screen.matching_retina),
            on_action,
        ));
    } else {
        panel = panel.child(
            div()
                .text_color(theme.muted_foreground)
                .child("Remote size controlled by server"),
        );
    }

    if let Some((aligned, locked)) = screen.alignment {
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
                ControlsAction::SetAlignment(!aligned),
                on_action,
            )
        });
    }

    if let Some(dpi) = screen.dpi {
        let mut label = dpi_label(dpi);
        let row = div().v_flex().gap_1().child(Label::new("UI Scaling"));
        if screen.dpi_choices.len() > 1 {
            if screen.dpi_override.is_none() {
                label.push_str(" · Match Mac Display");
            }
            panel = panel.child(
                row.child(picker_button(Picker::Dpi, label, state.picker, on_action))
                    .when(state.picker == Some(Picker::Dpi), |row| {
                        row.children(
                            std::iter::once(None)
                                .chain(screen.dpi_choices.iter().copied().map(Some))
                                .map(|choice| {
                                    let on_action = Rc::clone(on_action);
                                    let label = choice
                                        .map_or_else(|| "Match Mac Display".into(), dpi_label);
                                    let button = Button::new(SharedString::from(label.clone()))
                                        .small()
                                        .label(label);
                                    let button = if screen.dpi_override == choice {
                                        button.primary()
                                    } else {
                                        button.outline()
                                    };
                                    button.on_click(move |_, window, cx| {
                                        on_action(ControlsAction::SetUiScaling(choice), window, cx);
                                    })
                                }),
                        )
                    }),
            );
        } else {
            panel = panel.child(row.child(label));
        }
    }

    if screen.resize_allowed {
        let on_apply = Rc::clone(on_action);
        let on_reset = Rc::clone(on_action);
        let on_fit = Rc::clone(on_action);
        panel = panel
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(Label::new("Preset"))
                    .child(picker_button(
                        Picker::Preset,
                        screen.preset_label.clone(),
                        state.picker,
                        on_action,
                    ))
                    .when(state.picker == Some(Picker::Preset), |row| {
                        row.child(
                            Button::new("fit")
                                .outline()
                                .small()
                                .label("Keep Remote Size")
                                .on_click(move |_, window, cx| {
                                    on_fit(ControlsAction::SetFitMode, window, cx);
                                }),
                        )
                        .children(
                            [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)]
                                .into_iter()
                                .map(|(w, h)| {
                                    let on_action = Rc::clone(on_action);
                                    let label = format!("{w} × {h}");
                                    Button::new(SharedString::from(label.clone()))
                                        .outline()
                                        .small()
                                        .label(label)
                                        .on_click(move |_, window, cx| {
                                            on_action(
                                                ControlsAction::SelectPreset(w, h),
                                                window,
                                                cx,
                                            );
                                        })
                                }),
                        )
                    }),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(input_field("Width", width))
                    .child(input_field("Height", height)),
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
                            .on_click(move |_, window, cx| {
                                on_apply(ControlsAction::ApplyManualSize, window, cx);
                            }),
                    )
                    .child(
                        Button::new("reset-to-window")
                            .outline()
                            .small()
                            .label("Reset to Window")
                            .on_click(move |_, window, cx| {
                                on_reset(ControlsAction::ResetToWindow, window, cx);
                            }),
                    ),
            )
            .when(state.size_error, |panel| {
                panel.child(
                    div()
                        .text_color(theme.danger)
                        .child("Enter a width of 2–4096 and a height of 2–2160."),
                )
            });
    }

    panel.child(
        action_button(
            if screen.scale_locally {
                "Scale Locally: On"
            } else {
                "Scale Locally: Off"
            },
            ControlsAction::SetScaleLocally(!screen.scale_locally),
            on_action,
        )
        .w_full(),
    )
}

fn render_video(theme: &Theme, video: Option<&VideoProps>, on_action: &ControlsHandler) -> Div {
    let panel = div().v_flex().gap_2();
    let Some(video) = video else {
        return panel.child("Video settings are unavailable for this connection.");
    };

    panel
        .child(Label::new("Codec"))
        .child(
            div()
                .h_flex()
                .flex_wrap()
                .gap_2()
                .children(video.codecs.iter().map(|choice| {
                    let on_action = Rc::clone(on_action);
                    let id = choice.id.clone();
                    let button = Button::new(SharedString::from(choice.id.clone()))
                        .small()
                        .label(choice.label.clone());
                    let button = if choice.selected {
                        button.primary()
                    } else {
                        button.outline()
                    };
                    button.on_click(move |_, window, cx| {
                        on_action(ControlsAction::SelectCodec(id.clone()), window, cx);
                    })
                })),
        )
        .when_some(video.preferred_notice.clone(), |panel, notice| {
            panel.child(div().text_color(theme.muted_foreground).child(notice))
        })
        .children(video.rows.iter().map(|row| {
            let group = div()
                .h_flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(row.label.clone());
            let mut values = div()
                .h_flex()
                .items_center()
                .gap_2()
                .text_color(theme.muted_foreground);

            match &row.control {
                VideoSettingControl::Toggle {
                    enabled,
                    can_toggle,
                } => {
                    values = values.child(if *enabled { "On" } else { "Off" });
                    if *can_toggle {
                        let on_action = Rc::clone(on_action);
                        let id = row.id.clone();
                        let next = if *enabled { 0 } else { 1 };
                        let label = if *enabled { "Turn Off" } else { "Turn On" };
                        values = values.child(
                            Button::new(SharedString::from(row.id.clone()))
                                .outline()
                                .small()
                                .label(label)
                                .on_click(move |_, window, cx| {
                                    on_action(
                                        ControlsAction::SetVideoSetting {
                                            id: id.clone(),
                                            value: next,
                                        },
                                        window,
                                        cx,
                                    );
                                }),
                        );
                    }
                }
                VideoSettingControl::Segmented { options } => {
                    for (mode, label, selected) in options {
                        let on_action = Rc::clone(on_action);
                        let id = row.id.clone();
                        let mode = *mode;
                        let selected = *selected;
                        let button = Button::new(SharedString::from(label.clone()))
                            .small()
                            .label(label.clone());
                        let button = if selected {
                            button.primary()
                        } else {
                            button.outline()
                        };
                        values = values.child(button.on_click(move |_, window, cx| {
                            if !selected {
                                on_action(
                                    ControlsAction::SetVideoSetting {
                                        id: id.clone(),
                                        value: mode,
                                    },
                                    window,
                                    cx,
                                );
                            }
                        }));
                    }
                }
                VideoSettingControl::Stepper {
                    display,
                    decrement,
                    increment,
                } => {
                    values = values.child(display.clone());
                    for (next, sign) in [(*decrement, "−"), (*increment, "+")] {
                        if let Some(next) = next {
                            let on_action = Rc::clone(on_action);
                            let id = row.id.clone();
                            let button_id = SharedString::from(format!("{}{sign}", row.id));
                            values = values.child(
                                Button::new(button_id)
                                    .outline()
                                    .small()
                                    .label(sign)
                                    .on_click(move |_, window, cx| {
                                        on_action(
                                            ControlsAction::SetVideoSetting {
                                                id: id.clone(),
                                                value: next,
                                            },
                                            window,
                                            cx,
                                        );
                                    }),
                            );
                        }
                    }
                }
            }
            group.child(values)
        }))
        .child(div().pt_1().child(action_button(
            "Reset Video Settings",
            ControlsAction::ResetVideo,
            on_action,
        )))
}

fn render_diagnostics(theme: &Theme, diagnostics: Option<&[(String, String)]>) -> Div {
    let mut rows = div().v_flex().gap_2();
    if let Some(fields) = diagnostics {
        rows = rows.children(fields.iter().map(|(label, value)| {
            div().h_flex().justify_between().child(label.clone()).child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(value.clone()),
            )
        }));
    } else {
        rows = rows.child("Waiting for stream diagnostics…");
    }
    rows
}

fn action_button(
    label: &'static str,
    action: ControlsAction,
    on_action: &ControlsHandler,
) -> Button {
    let on_action = Rc::clone(on_action);
    Button::new(label)
        .outline()
        .small()
        .label(label)
        .on_click(move |_, window, cx| on_action(action.clone(), window, cx))
}

fn action_button_owned(
    label: String,
    action: ControlsAction,
    on_action: &ControlsHandler,
) -> Button {
    let on_action = Rc::clone(on_action);
    Button::new(SharedString::from(label.clone()))
        .outline()
        .small()
        .label(label)
        .on_click(move |_, window, cx| on_action(action.clone(), window, cx))
}

fn picker_button(
    picker: Picker,
    label: String,
    active: Option<Picker>,
    on_action: &ControlsHandler,
) -> Button {
    let on_action = Rc::clone(on_action);
    let id = match picker {
        Picker::Preset => "preset-picker",
        Picker::Dpi => "dpi-picker",
    };
    let button = Button::new(id)
        .small()
        .w_full()
        .label(label)
        .dropdown_caret(true);
    let button = if active == Some(picker) {
        button.primary()
    } else {
        button.outline()
    };
    button.on_click(move |_, window, cx| {
        on_action(ControlsAction::TogglePicker(picker), window, cx);
    })
}

fn input_field(label: &'static str, field: &Entity<InputState>) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .v_flex()
        .gap_1()
        .child(Label::new(label))
        .child(Input::new(field).small())
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

fn switch_row(
    label: impl Into<String>,
    enabled: bool,
    action: ControlsAction,
    on_action: &ControlsHandler,
) -> AnyElement {
    let label = label.into();
    let on_action = Rc::clone(on_action);
    div()
        .h_flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(Label::new(label.clone()))
        .child(
            Switch::new(SharedString::from(label))
                .checked(enabled)
                .on_click(move |_, window, cx| on_action(action.clone(), window, cx)),
        )
        .into_any_element()
}
