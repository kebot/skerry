use crate::components::elevated_shadow;
use gpui_kit::component::{
    button::Button,
    collapsible::Collapsible,
    input::{Input, InputState},
    menu::{PopupMenu, PopupMenuItem},
    popover::Popover,
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{
    div, prelude::*, px, Anchor, AnyElement, App, ClickEvent, DismissEvent, Div, Entity,
    FocusHandle, Focusable as _, FontWeight, SharedString, Window,
};
use std::rc::Rc;

pub type ControlsHandler = Rc<dyn Fn(ControlsAction, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Input,
    Clipboard,
    Screen,
    Video,
    Audio,
    Stats,
}

impl Section {
    /// Every section, top to bottom; the order of `ControlsUiState::sections`.
    pub const ALL: [Self; 6] = [
        Self::Input,
        Self::Clipboard,
        Self::Screen,
        Self::Video,
        Self::Audio,
        Self::Stats,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlsUiState {
    pub input_open: bool,
    pub clipboard_open: bool,
    pub screen_open: bool,
    pub video_open: bool,
    pub audio_open: bool,
    pub stats_open: bool,
    pub size_error: bool,
    /// Focus returned to once a dropdown menu closes: the panel's, so Escape and cmd-w
    /// still close the panel after a pick.
    pub return_focus: Option<FocusHandle>,
    /// A dropdown (by id) shown open on its first render, for the preview workbench.
    pub open_menu: Option<&'static str>,
}

impl Default for ControlsUiState {
    fn default() -> Self {
        Self {
            input_open: false,
            clipboard_open: false,
            screen_open: true,
            video_open: false,
            audio_open: false,
            stats_open: false,
            size_error: false,
            return_focus: None,
            open_menu: None,
        }
    }
}

impl ControlsUiState {
    pub fn is_open(&self, section: Section) -> bool {
        match section {
            Section::Input => self.input_open,
            Section::Clipboard => self.clipboard_open,
            Section::Screen => self.screen_open,
            Section::Video => self.video_open,
            Section::Audio => self.audio_open,
            Section::Stats => self.stats_open,
        }
    }

    /// Opens or closes `section`; returns whether it is now open.
    pub fn toggle(&mut self, section: Section) -> bool {
        let open = match section {
            Section::Input => &mut self.input_open,
            Section::Clipboard => &mut self.clipboard_open,
            Section::Screen => &mut self.screen_open,
            Section::Video => &mut self.video_open,
            Section::Audio => &mut self.audio_open,
            Section::Stats => &mut self.stats_open,
        };
        *open = !*open;
        *open
    }

    /// Which sections are open, in `Section::ALL` order.
    pub fn sections(&self) -> [bool; 6] {
        Section::ALL.map(|section| self.is_open(section))
    }

    /// This state with the sections opened as `open` says, in `Section::ALL` order.
    pub fn with_sections(mut self, open: [bool; 6]) -> Self {
        for (section, open) in Section::ALL.into_iter().zip(open) {
            if self.is_open(section) != open {
                self.toggle(section);
            }
        }
        self
    }
}

/// What the Session Controls dropdowns need besides their items.
struct MenuContext {
    return_focus: Option<FocusHandle>,
    open: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlsAction {
    ToggleSection(Section),
    SetAudio(bool),
    SetPlayback(bool),
    ToggleFullScreen,
    Reconnect,
    Disconnect,
    /// Open the session's Files window (`ARCH-023`).
    OpenFiles,
    SetInput {
        id: String,
        enabled: bool,
    },
    SendChord(String),
    SetClipboardSend(bool),
    SetClipboardReceive(bool),
    SetClipboardSeamless(bool),
    /// Deliberate mode: send the Mac's clipboard to the session now.
    SendClipboard,
    /// Deliberate mode: fetch the session's clipboard onto the Mac now.
    CopySessionClipboard,
    SetRetina(bool),
    SetAlignment(bool),
    SetUiScaling(Option<u16>),
    SetFitMode,
    SelectPreset(u16, u16),
    ApplyManualSize,
    ResetToWindow,
    SetScaleLocally(bool),
    SelectCodec(String),
    SetVideoSetting {
        id: String,
        value: u32,
    },
    ResetVideo,
    /// Play through this output device (its Core Audio UID); `None` follows the Mac.
    SelectAudioOutput(Option<String>),
    /// Ask the server for this audio bitrate (bps).
    SetAudioBitrate(u32),
    CopyStats,
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

/// The clipboard switches the server leaves to the person (`ARCH-017`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardProps {
    /// Mac → session; `None` when the server refuses that direction.
    pub send: Option<bool>,
    /// Session → Mac; `None` when the server refuses that direction.
    pub receive: Option<bool>,
    pub seamless: bool,
    pub seamless_locked: bool,
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

/// How a slider's value reads in its label, as the web sidebar writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliderUnit {
    /// The bare number: "Video CRF (25)".
    Plain,
    /// "Frames per second (60 FPS)".
    Fps,
    /// A value in kbps, shown in Mbps: "Video Bitrate (8 Mbps)".
    Kbps,
}

impl SliderUnit {
    pub fn format(self, value: u32) -> String {
        match self {
            SliderUnit::Plain => value.to_string(),
            SliderUnit::Fps => format!("{value} FPS"),
            SliderUnit::Kbps if value.is_multiple_of(1000) => format!("{} Mbps", value / 1000),
            SliderUnit::Kbps => format!("{:.1} Mbps", f64::from(value) / 1000.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoSettingControl {
    Switch {
        enabled: bool,
        locked: bool,
    },
    Select {
        value: u32,
        options: Vec<(u32, String)>,
        locked: bool,
    },
    Slider {
        value: u32,
        min: u32,
        max: u32,
        step: u32,
        unit: SliderUnit,
        locked: bool,
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

/// One entry of the Output Device menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioOutputItem {
    /// `None` is System Default.
    pub uid: Option<String>,
    /// Already formatted: "System Default (MacBook Pro Speakers)", "AirPods Pro (not connected)".
    pub label: String,
    pub checked: bool,
    /// `false` only for the saved device that isn't connected: shown checked but disabled.
    pub connected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioBitrateProps {
    pub value: u32,
    pub choices: Vec<u32>,
    /// Locked by the server or a single choice: shown, disabled.
    pub locked: bool,
}

/// The Audio Settings card (`ARCH-GAP-DC-03`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioProps {
    pub output_label: String,
    /// System Default, the connected devices, then the missing saved device if any;
    /// `skerry-ui` draws the separators.
    pub outputs: Vec<AudioOutputItem>,
    /// Shown under the Output Device dropdown.
    pub note: Option<String>,
    /// `None` hides the Audio Bitrate field.
    pub bitrate: Option<AudioBitrateProps>,
    /// When set, the card shows only this.
    pub unavailable: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ControlsProps {
    pub app_name: String,
    pub fullscreen: bool,
    pub audio: Option<bool>,
    pub video: Option<bool>,
    pub audio_unavailable: Option<String>,
    pub input_switches: Vec<SwitchItem>,
    pub chords: Vec<ChordItem>,
    /// `None` until the server announces its settings.
    pub clipboard: Option<ClipboardProps>,
    pub screen: ScreenProps,
    pub video_settings: Option<VideoProps>,
    /// The Audio Settings card; `None` until the session is ready.
    pub audio_settings: Option<AudioProps>,
    /// The Stats section; `None` until the first reading arrives.
    pub stats: Option<crate::stats::StatsProps>,
    /// Shows the copy button's check for a moment after a copy.
    pub stats_copied: bool,
}

pub fn dpi_label(dpi: u16) -> String {
    format!("{}%", (u32::from(dpi) * 100 + 48) / 96)
}

/// An audio bitrate in bps as the menu reads it: "128 kbps", or "96.5 kbps" when it isn't a
/// whole number of kbps.
pub fn kbps_label(bps: u32) -> String {
    if bps.is_multiple_of(1000) {
        format!("{} kbps", bps / 1000)
    } else {
        format!("{:.1} kbps", f64::from(bps) / 1000.0)
    }
}

pub fn render(
    props: &ControlsProps,
    state: &ControlsUiState,
    width: &Entity<InputState>,
    height: &Entity<InputState>,
    on_action: ControlsHandler,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let theme = &cx.theme().clone();
    let menus = &MenuContext {
        return_focus: state.return_focus.clone(),
        open: state.open_menu,
    };
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
            "Files",
            ControlsAction::OpenFiles,
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
        // Sections are separate cards with a 10px gap, as in the web sidebar
        // (`.sidebar-section { margin-bottom: 10px }`).
        .child(
            div()
                .px_4()
                .pb_4()
                .v_flex()
                .gap(px(10.))
                .child(section(
                    theme,
                    "Input",
                    state.input_open,
                    || render_input(props, &on_action),
                    toggle(Section::Input),
                ))
                .child(section(
                    theme,
                    "Clipboard",
                    state.clipboard_open,
                    || render_clipboard(theme, props.clipboard.as_ref(), &on_action),
                    toggle(Section::Clipboard),
                ))
                .child(section(
                    theme,
                    "Screen Settings",
                    state.screen_open,
                    || {
                        render_screen(
                            theme,
                            &props.screen,
                            state,
                            width,
                            height,
                            &on_action,
                            menus,
                        )
                    },
                    toggle(Section::Screen),
                ))
                .child(section(
                    theme,
                    "Video Settings",
                    state.video_open,
                    || {
                        render_video(
                            theme,
                            props.video_settings.as_ref(),
                            &on_action,
                            menus,
                            window,
                            cx,
                        )
                    },
                    toggle(Section::Video),
                ))
                .child(section(
                    theme,
                    "Audio Settings",
                    state.audio_open,
                    || render_audio(theme, props.audio_settings.as_ref(), &on_action, menus),
                    toggle(Section::Audio),
                ))
                .child(section(
                    theme,
                    "Stats",
                    state.stats_open,
                    || render_stats(theme, props, &on_action),
                    toggle(Section::Stats),
                )),
        )
}

/// The web sidebar's rhythm: 14px between settings, 6px from a label to its control.
const STACK_GAP: f32 = 14.;
const FIELD_GAP: f32 = 6.;
/// The web sidebar's control height and corner radius.
const CONTROL_H: f32 = 40.;
const CONTROL_RADIUS: f32 = 8.;

fn render_input(props: &ControlsProps, on_action: &ControlsHandler) -> Div {
    div()
        .v_flex()
        .gap(px(STACK_GAP))
        .children(props.input_switches.iter().map(|item| {
            switch_row(
                item.label.clone(),
                item.enabled,
                false,
                ControlsAction::SetInput {
                    id: item.id.clone(),
                    enabled: !item.enabled,
                },
                on_action,
            )
        }))
        .child(field(
            "Send Key Combination",
            div()
                .h_flex()
                .gap_2()
                .children(props.chords.iter().map(|chord| {
                    div().flex_1().min_w_0().child(field_button(
                        SharedString::from(chord.id.clone()),
                        chord.label.clone(),
                        ControlsAction::SendChord(chord.id.clone()),
                        on_action,
                    ))
                })),
        ))
}

fn render_clipboard(
    theme: &Theme,
    clipboard: Option<&ClipboardProps>,
    on_action: &ControlsHandler,
) -> Div {
    let note = |text: &'static str| div().text_color(theme.muted_foreground).child(text);
    let Some(clipboard) = clipboard else {
        return div().child(note("Waiting for the session…"));
    };
    if clipboard.send.is_none() && clipboard.receive.is_none() {
        return div().child(note("Clipboard turned off by the server"));
    }
    let mut panel = div().v_flex().gap(px(STACK_GAP));
    if let Some(send) = clipboard.send {
        panel = panel.child(switch_row(
            "Send to Session",
            send,
            false,
            ControlsAction::SetClipboardSend(!send),
            on_action,
        ));
    }
    if let Some(receive) = clipboard.receive {
        panel = panel.child(switch_row(
            "Receive from Session",
            receive,
            false,
            ControlsAction::SetClipboardReceive(!receive),
            on_action,
        ));
    }
    panel = panel.child(switch_row(
        "Seamless",
        clipboard.seamless,
        clipboard.seamless_locked,
        ControlsAction::SetClipboardSeamless(!clipboard.seamless),
        on_action,
    ));
    if clipboard.seamless {
        return panel.child(note(
            "A copy on either side is ready to paste on the other.",
        ));
    }
    let send = clipboard.send == Some(true);
    let receive = clipboard.receive == Some(true);
    panel
        .child(note("Nothing moves until you choose."))
        .when(send || receive, |panel| {
            panel
                .when(send, |panel| {
                    panel.child(field_button(
                        "send-clipboard",
                        "Send Mac Clipboard",
                        ControlsAction::SendClipboard,
                        on_action,
                    ))
                })
                .when(receive, |panel| {
                    panel.child(field_button(
                        "copy-session-clipboard",
                        "Copy Session Clipboard",
                        ControlsAction::CopySessionClipboard,
                        on_action,
                    ))
                })
        })
}

fn render_screen(
    theme: &Theme,
    screen: &ScreenProps,
    state: &ControlsUiState,
    width: &Entity<InputState>,
    height: &Entity<InputState>,
    on_action: &ControlsHandler,
    menus: &MenuContext,
) -> Div {
    let mut panel = div().v_flex().gap(px(STACK_GAP));
    if screen.resize_allowed {
        panel = panel.child(switch_row(
            "HiDPI (Pixel Perfect)",
            screen.matching_retina,
            false,
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
        panel = panel.child(switch_row(
            "Force Aligned Resolution",
            aligned,
            locked,
            ControlsAction::SetAlignment(!aligned),
            on_action,
        ));
    }

    if let Some(dpi) = screen.dpi {
        let mut label = dpi_label(dpi);
        if screen.dpi_choices.len() > 1 && screen.dpi_override.is_none() {
            label.push_str(" · Match Mac Display");
        }
        let items = std::iter::once(None)
            .chain(screen.dpi_choices.iter().copied().map(Some))
            .map(|choice| {
                (
                    choice.map_or_else(|| "Match Mac Display".into(), dpi_label),
                    screen.dpi_override == choice,
                    ControlsAction::SetUiScaling(choice),
                )
            })
            .collect();
        panel = panel.child(field(
            "UI Scaling",
            dropdown(
                "dpi-picker",
                label,
                items,
                screen.dpi_choices.len() <= 1,
                on_action,
                menus,
            ),
        ));
    }

    if screen.resize_allowed {
        let presets = std::iter::once(("Keep Remote Size".to_string(), ControlsAction::SetFitMode))
            .chain(
                [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)]
                    .into_iter()
                    .map(|(w, h)| (format!("{w} × {h}"), ControlsAction::SelectPreset(w, h))),
            )
            .map(|(label, action)| {
                let checked = label == screen.preset_label;
                (label, checked, action)
            })
            .collect();
        panel = panel
            .child(field(
                "Preset",
                dropdown(
                    "preset-picker",
                    screen.preset_label.clone(),
                    presets,
                    false,
                    on_action,
                    menus,
                ),
            ))
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
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(wrapping_button(
                        "apply-manual",
                        "Set Manual Resolution",
                        ControlsAction::ApplyManualSize,
                        on_action,
                    )))
                    .child(div().flex_1().min_w_0().child(wrapping_button(
                        "reset-to-window",
                        "Reset to Window",
                        ControlsAction::ResetToWindow,
                        on_action,
                    ))),
            )
            .when(state.size_error, |panel| {
                panel.child(
                    div()
                        .text_color(theme.danger)
                        .child("Enter a width of 2–4096 and a height of 2–2160."),
                )
            });
    }

    panel.child(field_button(
        "scale-locally",
        if screen.scale_locally {
            "Scale Locally: ON"
        } else {
            "Scale Locally: OFF"
        },
        ControlsAction::SetScaleLocally(!screen.scale_locally),
        on_action,
    ))
}

fn render_video(
    theme: &Theme,
    video: Option<&VideoProps>,
    on_action: &ControlsHandler,
    menus: &MenuContext,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let panel = div().v_flex().gap(px(STACK_GAP));
    let Some(video) = video else {
        return panel.child("Video settings are unavailable for this connection.");
    };

    let current = video
        .codecs
        .iter()
        .find(|choice| choice.selected)
        .map_or_else(String::new, |choice| choice.label.clone());
    let codecs = video
        .codecs
        .iter()
        .map(|choice| {
            (
                choice.label.clone(),
                choice.selected,
                ControlsAction::SelectCodec(choice.id.clone()),
            )
        })
        .collect();

    let mut panel = panel
        .child(field(
            "Encoder",
            dropdown(
                "codec-picker",
                current,
                codecs,
                video.codecs.len() <= 1,
                on_action,
                menus,
            ),
        ))
        .when_some(video.preferred_notice.clone(), |panel, notice| {
            panel.child(div().text_color(theme.muted_foreground).child(notice))
        });

    for row in &video.rows {
        panel = panel.child(match &row.control {
            VideoSettingControl::Switch { enabled, locked } => switch_row(
                row.label.clone(),
                *enabled,
                *locked,
                ControlsAction::SetVideoSetting {
                    id: row.id.clone(),
                    value: u32::from(!*enabled),
                },
                on_action,
            ),
            VideoSettingControl::Select {
                value,
                options,
                locked,
            } => {
                let current = options
                    .iter()
                    .find(|(mode, _)| mode == value)
                    .map_or_else(|| value.to_string(), |(_, label)| label.clone());
                let items = options
                    .iter()
                    .map(|(mode, label)| {
                        (
                            label.clone(),
                            mode == value,
                            ControlsAction::SetVideoSetting {
                                id: row.id.clone(),
                                value: *mode,
                            },
                        )
                    })
                    .collect();
                field(
                    row.label.clone(),
                    dropdown(
                        SharedString::from(format!("{}-picker", row.id)),
                        current,
                        items,
                        *locked || options.len() <= 1,
                        on_action,
                        menus,
                    ),
                )
                .into_any_element()
            }
            VideoSettingControl::Slider {
                value,
                min,
                max,
                step,
                unit,
                locked,
            } => slider_field(
                &row.id,
                &row.label,
                SliderSpec {
                    value: *value,
                    min: *min,
                    max: *max,
                    step: *step,
                    unit: *unit,
                    locked: *locked,
                },
                on_action,
                window,
                cx,
            ),
        });
    }

    panel.child(field_button(
        "reset-video",
        "Reset Video Settings",
        ControlsAction::ResetVideo,
        on_action,
    ))
}

fn render_audio(
    theme: &Theme,
    audio: Option<&AudioProps>,
    on_action: &ControlsHandler,
    menus: &MenuContext,
) -> Div {
    let note = |text: String| div().text_color(theme.muted_foreground).child(text);
    let Some(audio) = audio else {
        return div().child(note("Waiting for the session…".into()));
    };
    if let Some(reason) = &audio.unavailable {
        return div().child(note(reason.clone()));
    }
    let (default, devices): (Vec<_>, Vec<_>) =
        audio.outputs.iter().partition(|item| item.uid.is_none());
    let entry = |item: &AudioOutputItem| MenuItem::Action {
        label: item.label.clone(),
        checked: item.checked,
        disabled: !item.connected,
        action: ControlsAction::SelectAudioOutput(item.uid.clone()),
    };
    let mut items: Vec<MenuItem> = default.into_iter().map(entry).collect();
    if !devices.is_empty() {
        items.push(MenuItem::Separator);
        items.extend(devices.into_iter().map(entry));
    }
    // Never disabled: the menu always holds System Default.
    let output = div()
        .v_flex()
        .gap(px(FIELD_GAP))
        .child(field(
            "Output Device",
            menu_select(
                "audio-output",
                audio.output_label.clone(),
                items,
                false,
                on_action,
                menus,
            ),
        ))
        .when_some(audio.note.clone(), |output, text| output.child(note(text)));
    div().v_flex().gap(px(STACK_GAP)).child(output).when_some(
        audio.bitrate.as_ref(),
        |panel, bitrate| {
            let items = bitrate
                .choices
                .iter()
                .map(|&bps| MenuItem::Action {
                    label: kbps_label(bps),
                    checked: bps == bitrate.value,
                    disabled: false,
                    action: ControlsAction::SetAudioBitrate(bps),
                })
                .collect();
            panel.child(field(
                "Audio Bitrate",
                menu_select(
                    "audio-bitrate",
                    kbps_label(bitrate.value),
                    items,
                    bitrate.locked,
                    on_action,
                    menus,
                ),
            ))
        },
    )
}

fn render_stats(theme: &Theme, props: &ControlsProps, on_action: &ControlsHandler) -> Div {
    let Some(stats) = props.stats.as_ref() else {
        return div()
            .text_color(theme.muted_foreground)
            .child("Waiting for stream stats…");
    };
    let on_action = Rc::clone(on_action);
    crate::stats::render(theme, stats, props.stats_copied, move |window, cx| {
        on_action(ControlsAction::CopyStats, window, cx)
    })
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

/// A label over its control, as the web sidebar's `.setting-item`.
fn field(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
    div()
        .v_flex()
        .gap(px(FIELD_GAP))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(label.into()))
        .child(control)
}

/// A full-width button at the web sidebar's control height.
fn field_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    action: ControlsAction,
    on_action: &ControlsHandler,
) -> Button {
    let on_action = Rc::clone(on_action);
    Button::new(id.into())
        .outline()
        .w_full()
        .h(px(CONTROL_H))
        .rounded(px(CONTROL_RADIUS))
        .label(label.into())
        .on_click(move |_, window, cx| on_action(action.clone(), window, cx))
}

/// A full-width button whose label wraps onto a second line rather than being
/// cut, as the web sidebar's Set Manual Resolution / Reset to Window pair.
fn wrapping_button(
    id: &'static str,
    label: &'static str,
    action: ControlsAction,
    on_action: &ControlsHandler,
) -> Button {
    let on_action = Rc::clone(on_action);
    Button::new(id)
        .outline()
        .w_full()
        .h_auto()
        .min_h(px(CONTROL_H))
        .py(px(6.))
        .px(px(8.))
        .rounded(px(CONTROL_RADIUS))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .text_center()
                .child(label),
        )
        .on_click(move |_, window, cx| on_action(action.clone(), window, cx))
}

/// A full-width dropdown, the web sidebar's `<select>`: the current choice
/// on the button, the choices in a popup menu with the current one checked.
fn dropdown(
    id: impl Into<SharedString>,
    current: String,
    items: Vec<(String, bool, ControlsAction)>,
    disabled: bool,
    on_action: &ControlsHandler,
    menus: &MenuContext,
) -> AnyElement {
    let items = items
        .into_iter()
        .map(|(label, checked, action)| MenuItem::Action {
            label,
            checked,
            disabled: false,
            action,
        })
        .collect();
    menu_select(id, current, items, disabled, on_action, menus)
}

/// `dropdown` with separators and disabled entries.
fn menu_select(
    id: impl Into<SharedString>,
    current: String,
    items: Vec<MenuItem>,
    disabled: bool,
    on_action: &ControlsHandler,
    menus: &MenuContext,
) -> AnyElement {
    let id = id.into();
    let trigger = Button::new(id.clone())
        .outline()
        .w_full()
        .h(px(CONTROL_H))
        .rounded(px(CONTROL_RADIUS))
        .label(current)
        .dropdown_caret(true)
        .disabled(disabled);
    menu_dropdown(id, trigger, Rc::new(items), Rc::clone(on_action), menus)
}

/// One row of a dropdown's menu.
#[derive(Clone, Debug, PartialEq, Eq)]
enum MenuItem {
    Action {
        label: String,
        checked: bool,
        /// Greyed out and unclickable; a checked one still shows its check.
        disabled: bool,
        action: ControlsAction,
    },
    Separator,
}

/// The menu a dropdown opens, kept across renders so its highlighted row survives.
#[derive(Default)]
struct MenuState {
    menu: Option<Entity<PopupMenu>>,
    /// The items the menu was built with.
    built_for: Vec<MenuItem>,
}

/// Whether a menu built for `built_for` still shows `items`. A menu can close without a
/// `DismissEvent` (the popover's own Escape or trigger click), so the cache is checked
/// against the current items instead of trusting the dismissal to clear it.
fn menu_is_current(built_for: &[MenuItem], items: &[MenuItem]) -> bool {
    built_for == items
}

/// gpui-kit's `dropdown_menu`, rebuilt so the popup gets `elevated_shadow` (gpui-kit's own
/// shadow vanishes in Dark). It follows gpui-kit's wiring: one cached `PopupMenu`, one
/// dismiss subscription holding only Weak handles (a strong one would leak the menu), and
/// no overlay close. gpui-kit's trigger-focus hook is private, so focus goes back to
/// `menus.return_focus` instead.
///
/// A click on the trigger while the menu is open closes it and doesn't reopen it: the menu's
/// mouse-down-out runs in the capture phase and dismisses the popover first, and the
/// popover's trigger handler toggles only when the open state is still the one it rendered
/// with (gpui-base `Popover`).
fn menu_dropdown(
    id: SharedString,
    trigger: Button,
    items: Rc<Vec<MenuItem>>,
    on_action: ControlsHandler,
    menus: &MenuContext,
) -> AnyElement {
    let return_focus = menus.return_focus.clone();
    let open = menus.open == Some(id.as_ref());
    let state_key = SharedString::from(format!("menu-state:{id}"));
    Popover::new(SharedString::from(format!("menu:{id}")))
        .appearance(false)
        .overlay_closable(false)
        .anchor(Anchor::TopLeft)
        .default_open(open)
        .trigger(trigger)
        .content(move |_, window, cx| {
            let menu_state =
                window.use_keyed_state(state_key.clone(), cx, |_, _| MenuState::default());
            // The wrapper takes the menu's own radius, so the shadow's corners match.
            let radius = cx.theme().radius;
            let shadow = elevated_shadow(cx.theme());
            let cached = menu_state
                .read(cx)
                .menu
                .clone()
                .filter(|_| menu_is_current(&menu_state.read(cx).built_for, &items));
            let menu = match cached {
                Some(menu) => menu,
                None => {
                    let menu_items = Rc::clone(&items);
                    let on_action = Rc::clone(&on_action);
                    let menu = PopupMenu::build(window, cx, move |menu, _, _| {
                        menu_items.iter().fold(menu, |menu, item| match item {
                            MenuItem::Separator => menu.separator(),
                            MenuItem::Action {
                                label,
                                checked,
                                disabled,
                                action,
                            } => {
                                let on_action = Rc::clone(&on_action);
                                let action = action.clone();
                                menu.item(
                                    PopupMenuItem::new(label.clone())
                                        .checked(*checked)
                                        .disabled(*disabled)
                                        .on_click(move |_, window, cx| {
                                            on_action(action.clone(), window, cx)
                                        }),
                                )
                            }
                        })
                    });
                    let built_for = items.as_ref().clone();
                    menu_state.update(cx, |state, _| {
                        state.menu = Some(menu.clone());
                        state.built_for = built_for;
                    });
                    menu.focus_handle(cx).focus(window, cx);
                    let popover_state = cx.entity().downgrade();
                    let weak_state = menu_state.downgrade();
                    let return_focus = return_focus.clone();
                    window
                        .subscribe(&menu, cx, move |_, _: &DismissEvent, window, cx| {
                            if let Some(popover_state) = popover_state.upgrade() {
                                popover_state.update(cx, |state, cx| state.dismiss(window, cx));
                            }
                            let _ = weak_state.update(cx, |state, _| state.menu = None);
                            if let Some(focus) = &return_focus {
                                window.focus(focus, cx);
                            }
                        })
                        .detach();
                    menu
                }
            };
            div().rounded(radius).shadow(shadow).child(menu)
        })
        .into_any_element()
}

struct SliderSpec {
    value: u32,
    min: u32,
    max: u32,
    step: u32,
    unit: SliderUnit,
    locked: bool,
}

/// A slider's state, kept by `skerry-ui` across renders so callers pass only
/// values. `synced` is the last value the caller passed: a new one (the
/// server's answer, a reset) moves the thumb; otherwise a drag is left alone.
struct LiveSlider {
    state: Entity<SliderState>,
    synced: u32,
}

/// A slider with its live value in the label ("Frames per second (60 FPS)").
/// The setting is requested once, when the thumb is released.
fn slider_field(
    id: &str,
    label: &str,
    spec: SliderSpec,
    on_action: &ControlsHandler,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let SliderSpec {
        value,
        min,
        max,
        step,
        unit,
        locked,
    } = spec;
    let key = SharedString::from(format!("controls-slider-{id}-{min}-{max}-{step}"));
    let live = window.use_keyed_state(key, cx, |window, cx| {
        let state = cx.new(|_| {
            SliderState::new()
                .min(min as f32)
                .max(max as f32)
                .step(step as f32)
                .default_value(value as f32)
        });
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let on_action = Rc::clone(on_action);
        let id = id.to_string();
        cx.subscribe_in(
            &state,
            window,
            move |live: &mut LiveSlider, _, event: &SliderEvent, window, cx| {
                if let SliderEvent::Release(released) = event {
                    let released = released.end().round() as u32;
                    if released != live.synced {
                        on_action(
                            ControlsAction::SetVideoSetting {
                                id: id.clone(),
                                value: released,
                            },
                            window,
                            cx,
                        );
                    }
                }
            },
        )
        .detach();
        LiveSlider {
            state,
            synced: value,
        }
    });
    if live.read(cx).synced != value {
        live.update(cx, |live, cx| {
            live.synced = value;
            live.state
                .update(cx, |state, cx| state.set_value(value as f32, window, cx));
        });
    }
    let state = live.read(cx).state.clone();
    let shown = state.read(cx).value().end().round() as u32;
    field(
        format!("{label} ({})", unit.format(shown)),
        Slider::new(&state).disabled(locked),
    )
    .into_any_element()
}

fn input_field(label: &'static str, input: &Entity<InputState>) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .child(field(label, Input::new(input).h(px(CONTROL_H))))
}

fn section(
    theme: &Theme,
    title: &'static str,
    open: bool,
    body: impl FnOnce() -> Div,
    toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    // Card metrics follow the web sidebar: 1px border, 12px radius,
    // 11px/14px header padding, 4px/14px/14px content padding.
    div()
        .border_1()
        .border_color(theme.border)
        .rounded(px(12.))
        .overflow_hidden()
        .bg(theme.secondary)
        .child(
            div()
                .id(title)
                .px(px(14.))
                .py(px(11.))
                .cursor_pointer()
                .h_flex()
                .items_center()
                .justify_between()
                .font_weight(FontWeight::BOLD)
                .child(title.to_uppercase())
                .child(div().text_color(theme.muted_foreground).child(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                }))
                .on_click(toggle),
        )
        .child(
            Collapsible::new()
                .open(open)
                .motion_id(SharedString::from(format!("section-{title}")))
                .content(div().pt_1().px(px(14.)).pb(px(14.)).child(body())),
        )
}

/// A label with its switch at the row's end, as the web sidebar's
/// `.toggle-item`; a server-locked setting shows a disabled switch.
fn switch_row(
    label: impl Into<String>,
    enabled: bool,
    locked: bool,
    action: ControlsAction,
    on_action: &ControlsHandler,
) -> AnyElement {
    let label = label.into();
    let on_action = Rc::clone(on_action);
    div()
        .h_flex()
        .items_center()
        .justify_between()
        .gap(px(10.))
        .child(
            div()
                .min_w_0()
                .font_weight(FontWeight::SEMIBOLD)
                .child(label.clone()),
        )
        .child(
            Switch::new(SharedString::from(label))
                .checked(enabled)
                .disabled(locked)
                .on_click(move |_, window, cx| on_action(action.clone(), window, cx)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menu_is_rebuilt_when_its_items_change() {
        let item = |label: &str, checked, disabled| MenuItem::Action {
            label: label.to_string(),
            checked,
            disabled,
            action: ControlsAction::SetFitMode,
        };
        let built = vec![
            item("1280 × 720", true, false),
            MenuItem::Separator,
            item("1920 × 1080", false, false),
        ];
        let same = built.clone();
        let moved_check = [
            item("1280 × 720", false, false),
            MenuItem::Separator,
            item("1920 × 1080", true, false),
        ];
        let now_disabled = [
            item("1280 × 720", true, false),
            MenuItem::Separator,
            item("1920 × 1080", false, true),
        ];
        assert!(menu_is_current(&built, &same));
        assert!(!menu_is_current(&built, &moved_check));
        assert!(!menu_is_current(&built, &now_disabled));
        assert!(!menu_is_current(&built, &same[..1]));
        assert!(!menu_is_current(&[], &same));
    }

    #[test]
    fn kbps_label_reads_whole_kbps() {
        assert_eq!(kbps_label(128_000), "128 kbps");
        assert_eq!(kbps_label(6_000), "6 kbps");
        assert_eq!(kbps_label(96_500), "96.5 kbps");
    }

    #[test]
    fn toggle_flips_only_that_section() {
        let mut state = ControlsUiState::default();
        let before = state.sections();
        assert!(state.toggle(Section::Video));
        let after = state.sections();
        for (index, section) in Section::ALL.into_iter().enumerate() {
            assert_eq!(after[index] != before[index], section == Section::Video);
        }
        assert!(!state.toggle(Section::Video));
        assert_eq!(state.sections(), before);
    }

    #[test]
    fn sections_round_trip_in_section_order() {
        // Input, Video and Audio open, Screen closed.
        let open = [true, false, false, true, true, false];
        let state = ControlsUiState::default().with_sections(open);
        assert_eq!(state.sections(), open);
        assert!(state.input_open && state.video_open && state.audio_open && !state.screen_open);
        assert_eq!(
            ControlsUiState::default().sections(),
            [false, false, true, false, false, false]
        );
    }
}
