use crate::components::elevated_shadow;
use crate::session::PrintJobAction;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    collapsible::Collapsible,
    input::{Input, InputState},
    menu::{PopupMenu, PopupMenuItem},
    popover::Popover,
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{
    div, prelude::*, px, Anchor, AnyElement, App, ClickEvent, DismissEvent, Div, Entity,
    FocusHandle, Focusable as _, FontFeatures, FontWeight, SharedString, Window,
};
use std::rc::Rc;
use std::sync::Arc;

pub type ControlsHandler = Rc<dyn Fn(ControlsAction, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Input,
    Shortcuts,
    Gamepads,
    Clipboard,
    Printing,
    Screen,
    Video,
    Audio,
    /// Microphone & Camera (`ARCH-038`).
    Capture,
    Stats,
}

impl Section {
    /// Every section, top to bottom; the order of `ControlsUiState::sections`.
    pub const ALL: [Self; 10] = [
        Self::Input,
        Self::Shortcuts,
        Self::Gamepads,
        Self::Clipboard,
        Self::Printing,
        Self::Screen,
        Self::Video,
        Self::Audio,
        Self::Capture,
        Self::Stats,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlsUiState {
    pub input_open: bool,
    pub shortcuts_open: bool,
    pub gamepads_open: bool,
    pub clipboard_open: bool,
    pub printing_open: bool,
    pub screen_open: bool,
    pub video_open: bool,
    pub audio_open: bool,
    pub capture_open: bool,
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
            shortcuts_open: false,
            gamepads_open: false,
            clipboard_open: false,
            printing_open: false,
            screen_open: true,
            video_open: false,
            audio_open: false,
            capture_open: false,
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
            Section::Shortcuts => self.shortcuts_open,
            Section::Gamepads => self.gamepads_open,
            Section::Clipboard => self.clipboard_open,
            Section::Printing => self.printing_open,
            Section::Screen => self.screen_open,
            Section::Video => self.video_open,
            Section::Audio => self.audio_open,
            Section::Capture => self.capture_open,
            Section::Stats => self.stats_open,
        }
    }

    /// Opens or closes `section`; returns whether it is now open.
    pub fn toggle(&mut self, section: Section) -> bool {
        let open = match section {
            Section::Input => &mut self.input_open,
            Section::Shortcuts => &mut self.shortcuts_open,
            Section::Gamepads => &mut self.gamepads_open,
            Section::Clipboard => &mut self.clipboard_open,
            Section::Printing => &mut self.printing_open,
            Section::Screen => &mut self.screen_open,
            Section::Video => &mut self.video_open,
            Section::Audio => &mut self.audio_open,
            Section::Capture => &mut self.capture_open,
            Section::Stats => &mut self.stats_open,
        };
        *open = !*open;
        *open
    }

    /// Which sections are open, in `Section::ALL` order.
    pub fn sections(&self) -> [bool; 10] {
        Section::ALL.map(|section| self.is_open(section))
    }

    /// This state with the sections opened as `open` says, in `Section::ALL` order.
    pub fn with_sections(mut self, open: [bool; 10]) -> Self {
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
    /// Open the session's Apps window (`ARCH-035`).
    OpenApps,
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
    /// **Show over the stream** (`ARCH-031`).
    SetStatsStrip(bool),
    /// **Mac shortcuts active** (`ARCH-031`).
    SetMacShortcuts(bool),
    /// Capture Pointer, or release it (`ARCH-032`).
    CapturePointer,
    /// **Gamepad input** (`ARCH-032`).
    SetGamepadInput(bool),
    /// **Rumble** (`ARCH-032`).
    SetGamepadRumble(bool),
    /// **Open Print Dialog Automatically** (`ARCH-033`).
    SetAutoPrint(bool),
    /// A Printing card row's button.
    PrintJob(u64, PrintJobAction),
    /// The **Microphone** switch (`ARCH-038`).
    SetMicrophone(bool),
    /// Capture from this input device (its Core Audio UID); `None` follows the Mac.
    SelectInputDevice(Option<String>),
    /// The **Camera** switch (`ARCH-038`).
    SetCamera(bool),
    /// Capture from this camera (its `AVCaptureDevice.uniqueID`); `None` follows the Mac.
    SelectCamera(Option<String>),
    /// **Open Privacy Settings** after macOS refused a device.
    OpenPrivacySettings(CaptureSubject),
}

/// A capture device the Microphone & Camera card shows (`ARCH-038`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureSubject {
    Microphone,
    Camera,
}

/// One entry of a capture device menu: E's Output Device entry shape.
pub type CaptureDeviceItem = AudioOutputItem;

/// One device's group in the Microphone & Camera card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureDeviceProps {
    /// The switch position.
    pub on: bool,
    /// The server decides: the switch is disabled.
    pub locked: bool,
    /// The server doesn't allow the device, so there is no list: the picker is disabled.
    pub picker_locked: bool,
    /// The device is open: a dot before the label, and the header status.
    pub live: bool,
    /// The muted line under the label.
    pub detail: Option<String>,
    /// macOS refused the device: the detail in the danger colour, and Open Privacy Settings.
    pub denied: bool,
    pub device_label: String,
    /// System Default, the connected devices, then the missing saved device if any.
    pub devices: Vec<CaptureDeviceItem>,
    /// Shown under the picker.
    pub note: Option<String>,
    /// `Some(0..=5)` shows the level meter.
    pub level: Option<u8>,
}

/// The Microphone & Camera card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaCaptureProps {
    pub mic: CaptureDeviceProps,
    /// `None` when the server hides the camera (`ui_sidebar_show_webcam` false).
    pub camera: Option<CaptureDeviceProps>,
}

/// The card header's status while a device is open: which dots, and the text.
pub fn media_capture_header(props: &MediaCaptureProps) -> Option<&'static str> {
    let camera = props.camera.as_ref().is_some_and(|camera| camera.live);
    match (props.mic.live, camera) {
        (true, true) => Some("Microphone and camera on"),
        (true, false) => Some("Microphone on"),
        (false, true) => Some("Camera on"),
        (false, false) => None,
    }
}

/// Whether a pad in a slot reaches the session (`ARCH-032`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadStatus {
    Sending,
    /// Input is on but the session window isn't key.
    Ready,
    NotSent,
}

/// One connected pad in a Player slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PadRow {
    pub name: String,
    pub status: PadStatus,
    /// The pad has haptics.
    pub rumble: bool,
}

/// The Gamepads card (`ARCH-032`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GamepadsProps {
    pub input: bool,
    /// The server's `gamepad_on_start` lock holds the input switch.
    pub input_locked: bool,
    pub rumble: bool,
    pub server_off: bool,
    /// The session window is key.
    pub key: bool,
    /// Players 1 to 4 (wire slots 0 to 3).
    pub slots: [Option<PadRow>; 4],
    /// Connected pads with no slot left.
    pub overflow: usize,
}

/// The Input card's Capture Pointer button (`ARCH-032`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureProps {
    /// A picture is painted and the session is ready.
    pub available: bool,
    /// The session window holds the pointer now.
    pub captured: bool,
    /// The chord in menu glyphs, from the Mac's chord table: "⌃⌥⇧" and "Z".
    pub modifiers: String,
    pub key: String,
}

/// A print job's state in the Printing card (`ARCH-033`).
#[derive(Clone, Debug, PartialEq)]
pub enum PrintRowKind {
    Receiving { fraction: Option<f32> },
    Ready,
    Unreadable,
    Failed,
    TooLarge,
    Missing,
}

/// One row of the Printing card.
#[derive(Clone, Debug, PartialEq)]
pub struct PrintJobRow {
    pub id: u64,
    pub name: String,
    /// `size · age[ · state]`.
    pub meta: String,
    pub kind: PrintRowKind,
    /// "Saved to Downloads", shown as a link that reveals the copy.
    pub saved_to: Option<String>,
    /// Open in another app: Remove keeps the file.
    pub opened: bool,
}

/// The Printing card; `None` in `ControlsProps` hides it (printing off or unknown).
#[derive(Clone, Debug, PartialEq)]
pub struct PrintingProps {
    pub auto_print: bool,
    /// Newest first.
    pub jobs: Vec<PrintJobRow>,
    /// The list holds as many jobs as it keeps.
    pub at_cap: bool,
}

/// Whether the Printing card shows: only while the server allows printing.
pub fn printing_card_shown(printing: Option<&PrintingProps>) -> bool {
    printing.is_some()
}

/// One row of the Shortcuts card, its chord in macOS menu glyphs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutRow {
    pub label: String,
    /// "⇧⌘", in ⌃⌥⇧⌘ order.
    pub modifiers: String,
    /// "M".
    pub key: String,
    /// "Sent as Ctrl+Shift+M" while the chord goes to the session.
    pub remote: Option<String>,
    /// Quit: "Always on the Mac", never dimmed.
    pub always_local: bool,
}

/// The Shortcuts card (`ARCH-031`, `ARCH-GAP-DC-12`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutsProps {
    /// The effective value: the server's when locked.
    pub mac_active: bool,
    pub locked: bool,
    pub rows: Vec<ShortcutRow>,
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
    /// The stats strip is shown over the stream.
    pub stats_strip: bool,
    pub shortcuts: ShortcutsProps,
    pub capture: CaptureProps,
    /// The Gamepads card; `None` hides it (`ui_sidebar_show_gamepads`, or no settings yet).
    pub gamepads: Option<GamepadsProps>,
    /// The Printing card (`ARCH-033`); `None` hides it.
    pub printing: Option<PrintingProps>,
    /// The Microphone & Camera card (`ARCH-038`); `None` (no settings yet) hides it.
    pub media_capture: Option<MediaCaptureProps>,
    /// The server allows installing apps: the header offers Apps (`ARCH-035`).
    pub apps_available: bool,
}

/// The header's action buttons, in order: Mute/Unmute, Pause/Resume, Full Screen, Files,
/// Apps (only when available), Reconnect, Disconnect.
pub fn header_actions(props: &ControlsProps) -> Vec<(&'static str, ControlsAction)> {
    header_actions_for(
        props.audio,
        props.video,
        props.fullscreen,
        props.apps_available,
    )
}

fn header_actions_for(
    audio: Option<bool>,
    video: Option<bool>,
    fullscreen: bool,
    apps_available: bool,
) -> Vec<(&'static str, ControlsAction)> {
    let mut actions = Vec::new();
    if let Some(enabled) = audio {
        actions.push((
            if enabled { "Mute" } else { "Unmute" },
            ControlsAction::SetAudio(!enabled),
        ));
    }
    if let Some(enabled) = video {
        actions.push((
            if enabled {
                "Pause Video"
            } else {
                "Resume Video"
            },
            ControlsAction::SetPlayback(!enabled),
        ));
    }
    actions.push((
        if fullscreen {
            "Exit Full Screen"
        } else {
            "Full Screen"
        },
        ControlsAction::ToggleFullScreen,
    ));
    actions.push(("Files", ControlsAction::OpenFiles));
    if apps_available {
        actions.push(("Apps", ControlsAction::OpenApps));
    }
    actions.push(("Reconnect", ControlsAction::Reconnect));
    actions.push(("Disconnect", ControlsAction::Disconnect));
    actions
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
    let actions = div().h_flex().flex_wrap().gap_2().children(
        header_actions(props)
            .into_iter()
            .map(|(label, action)| action_button(label, action, &on_action)),
    );

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
                    || render_input(theme, props, &on_action),
                    toggle(Section::Input),
                ))
                .child(section(
                    theme,
                    "Shortcuts",
                    state.shortcuts_open,
                    || render_shortcuts(theme, &props.shortcuts, &on_action),
                    toggle(Section::Shortcuts),
                ))
                .when_some(props.gamepads.as_ref(), |cards, gamepads| {
                    cards.child(section(
                        theme,
                        "Gamepads",
                        state.gamepads_open,
                        || render_gamepads(theme, gamepads, &on_action),
                        toggle(Section::Gamepads),
                    ))
                })
                .child(section(
                    theme,
                    "Clipboard",
                    state.clipboard_open,
                    || render_clipboard(theme, props.clipboard.as_ref(), &on_action),
                    toggle(Section::Clipboard),
                ))
                .when_some(
                    props
                        .printing
                        .as_ref()
                        .filter(|printing| printing_card_shown(Some(printing))),
                    |list, printing| {
                        list.child(section(
                            theme,
                            "Printing",
                            state.printing_open,
                            || render_printing(theme, printing, &on_action),
                            toggle(Section::Printing),
                        ))
                    },
                )
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
                .when_some(props.media_capture.as_ref(), |cards, capture| {
                    cards.child(section_with_status(
                        theme,
                        "Microphone & Camera",
                        state.capture_open,
                        media_capture_header(capture)
                            .map(|text| header_status(theme, capture, text)),
                        || render_media_capture(theme, capture, &on_action, menus),
                        toggle(Section::Capture),
                    ))
                })
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

fn render_input(theme: &Theme, props: &ControlsProps, on_action: &ControlsHandler) -> Div {
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
        .child(render_capture(theme, &props.capture, on_action))
}

/// Capture Pointer: the label with its crosshair, the chord's glyphs on the right, and a
/// note on how to get out (`ARCH-032`).
fn render_capture(theme: &Theme, capture: &CaptureProps, on_action: &ControlsHandler) -> Div {
    let chord = format!("{}{}", capture.modifiers, capture.key);
    let (label, note) = if capture.captured {
        ("Pointer Captured", format!("Press {chord} to release it."))
    } else if capture.available {
        (
            "Capture Pointer",
            format!("{chord}, ⌘-Tab, or leaving the window releases it."),
        )
    } else {
        (
            "Capture Pointer",
            "Available once the picture appears.".to_string(),
        )
    };
    let on_action = Rc::clone(on_action);
    let button = Button::new("input-capture-pointer")
        .w_full()
        .h(px(CONTROL_H))
        .rounded(px(CONTROL_RADIUS))
        .disabled(!capture.available && !capture.captured)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_flex()
                .items_center()
                .gap(px(8.))
                .child(Icon::new(gpui_kit::assets::IconName::Crosshair).with_size(px(16.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .text_left()
                        .child(label),
                )
                .child(
                    div()
                        .w(px(56.))
                        .flex_none()
                        .h_flex()
                        .text_size(px(13.))
                        .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
                        .child(
                            div()
                                .flex_1()
                                .text_right()
                                .whitespace_nowrap()
                                .when(!capture.captured, |glyphs| {
                                    glyphs.text_color(theme.muted_foreground)
                                })
                                .child(capture.modifiers.clone()),
                        )
                        .child(
                            div()
                                .w(px(14.))
                                .flex_none()
                                .whitespace_nowrap()
                                .child(capture.key.clone()),
                        ),
                ),
        )
        .on_click(move |_, window, cx| on_action(ControlsAction::CapturePointer, window, cx));
    let button = if capture.captured {
        button.primary()
    } else {
        button.outline()
    };
    div().v_flex().gap(px(FIELD_GAP)).child(button).child(
        div()
            .text_size(px(12.))
            .text_color(theme.muted_foreground)
            .child(note),
    )
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

/// macOS's own microphone indicator colour.
const MIC_DOT: u32 = 0xFF9F0A;
/// macOS's own camera indicator colour.
const CAM_DOT: u32 = 0x30D158;

/// An 8 px dot in `color`.
fn dot(color: u32) -> Div {
    div()
        .flex_none()
        .size(px(8.))
        .rounded_full()
        .bg(gpui_kit::rgb(color))
}

/// The header's status: the open devices' dots, then the text.
fn header_status(theme: &Theme, capture: &MediaCaptureProps, text: &'static str) -> AnyElement {
    let camera = capture.camera.as_ref().is_some_and(|camera| camera.live);
    div()
        .h_flex()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .h_flex()
                .gap(px(4.))
                .when(capture.mic.live, |dots| dots.child(dot(MIC_DOT)))
                .when(camera, |dots| dots.child(dot(CAM_DOT))),
        )
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::NORMAL)
                .text_color(theme.muted_foreground)
                .whitespace_nowrap()
                .child(text),
        )
        .into_any_element()
}

/// Five 6×10 segments; lit ones in the accent, the top one amber.
fn level_meter(theme: &Theme, level: u8) -> Div {
    div().h_flex().gap(px(3.)).children((0..5u8).map(|index| {
        let segment = div().w(px(6.)).h(px(10.)).rounded(px(2.));
        if index >= level {
            segment.bg(theme.muted)
        } else if index == 4 {
            segment.bg(gpui_kit::rgb(MIC_DOT))
        } else {
            segment.bg(theme.accent)
        }
    }))
}

/// The Microphone & Camera card (`ARCH-038`): the microphone's switch, picker and meter, the
/// camera's switch and picker (unless the server hides it), then the menu-bar footnote.
fn render_media_capture(
    theme: &Theme,
    capture: &MediaCaptureProps,
    on_action: &ControlsHandler,
    menus: &MenuContext,
) -> Div {
    div()
        .v_flex()
        .gap(px(STACK_GAP))
        .child(capture_device(
            theme,
            &capture.mic,
            CaptureSubject::Microphone,
            on_action,
            menus,
        ))
        .when_some(capture.camera.as_ref(), |card, camera| {
            card.child(div().h(px(1.)).w_full().bg(theme.border))
                .child(capture_device(
                    theme,
                    camera,
                    CaptureSubject::Camera,
                    on_action,
                    menus,
                ))
        })
        .child(div().h(px(1.)).w_full().bg(theme.border))
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child(if capture.camera.is_some() {
                    "macOS also shows its own indicator in the menu bar while the microphone or camera is in use."
                } else {
                    "macOS also shows its own indicator in the menu bar while the microphone is in use."
                }),
        )
}

/// One device's switch row, privacy button, picker, note and meter.
fn capture_device(
    theme: &Theme,
    device: &CaptureDeviceProps,
    subject: CaptureSubject,
    on_action: &ControlsHandler,
    menus: &MenuContext,
) -> Div {
    type Pick = fn(Option<String>) -> ControlsAction;
    let (label, picker_label, id, color, toggle, pick): (
        _,
        _,
        _,
        _,
        fn(bool) -> ControlsAction,
        Pick,
    ) = match subject {
        CaptureSubject::Microphone => (
            "Microphone",
            "Input Device",
            "capture-mic-device",
            MIC_DOT,
            ControlsAction::SetMicrophone,
            ControlsAction::SelectInputDevice,
        ),
        CaptureSubject::Camera => (
            "Camera",
            "Camera Device",
            "capture-cam-device",
            CAM_DOT,
            ControlsAction::SetCamera,
            ControlsAction::SelectCamera,
        ),
    };
    let detail_color = if device.denied {
        theme.danger
    } else {
        theme.muted_foreground
    };
    let switch_action = toggle(!device.on);
    let switch_handler = Rc::clone(on_action);
    let switch = div()
        .h_flex()
        .items_center()
        .justify_between()
        .gap(px(10.))
        .child(
            div()
                .min_w_0()
                .v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap(px(6.))
                        .when(device.live, |row| row.child(dot(color)))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(label)),
                )
                .when_some(device.detail.clone(), |column, detail| {
                    column.child(
                        div()
                            .text_size(px(12.))
                            .text_color(detail_color)
                            .child(detail),
                    )
                }),
        )
        .child(
            Switch::new(SharedString::from(format!("{id}-switch")))
                .checked(device.on)
                .disabled(device.locked)
                .on_click(move |_, window, cx| switch_handler(switch_action.clone(), window, cx)),
        );
    let privacy_handler = Rc::clone(on_action);
    let switch_group =
        div()
            .v_flex()
            .gap(px(FIELD_GAP))
            .child(switch)
            .when(device.denied, |group| {
                group.child(
                    div().h_flex().child(
                        Button::new(SharedString::from(format!("{id}-privacy")))
                            .outline()
                            .small()
                            .label("Open Privacy Settings")
                            .on_click(move |_, window, cx| {
                                privacy_handler(
                                    ControlsAction::OpenPrivacySettings(subject),
                                    window,
                                    cx,
                                )
                            }),
                    ),
                )
            });
    let (default, devices): (Vec<_>, Vec<_>) =
        device.devices.iter().partition(|item| item.uid.is_none());
    let entry = |item: &CaptureDeviceItem| MenuItem::Action {
        label: item.label.clone(),
        checked: item.checked,
        disabled: !item.connected,
        action: pick(item.uid.clone()),
    };
    let mut items: Vec<MenuItem> = default.into_iter().map(entry).collect();
    if !devices.is_empty() {
        items.push(MenuItem::Separator);
        items.extend(devices.into_iter().map(entry));
    }
    let muted = |text: String| {
        div()
            .text_size(px(12.))
            .text_color(theme.muted_foreground)
            .child(text)
    };
    let picker = div()
        .v_flex()
        .gap(px(FIELD_GAP))
        .child(field(
            picker_label,
            menu_select(
                id,
                device.device_label.clone(),
                items,
                device.picker_locked,
                on_action,
                menus,
            ),
        ))
        .when_some(device.note.clone(), |picker, note| {
            picker.child(muted(note))
        })
        .when_some(device.level, |picker, level| {
            picker.child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(8.))
                    .child(muted("Input level".into()))
                    .child(level_meter(theme, level)),
            )
        });
    div()
        .v_flex()
        .gap(px(STACK_GAP))
        .child(switch_group)
        .child(picker)
}

/// The Printing card (`ARCH-033`): the auto-open switch, then this window's jobs.
fn render_printing(theme: &Theme, printing: &PrintingProps, on_action: &ControlsHandler) -> Div {
    let list = if printing.jobs.is_empty() {
        div()
            .text_size(px(13.))
            .text_color(theme.muted_foreground)
            .child("Nothing printed yet. Print from an app in the session to get the PDF here.")
    } else {
        div()
            .v_flex()
            .gap(px(6.))
            .children(
                printing
                    .jobs
                    .iter()
                    .map(|row| print_row(theme, row, on_action)),
            )
            .when(printing.at_cap, |list| {
                list.child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child("Keeps the 20 most recent."),
                )
            })
    };
    div()
        .v_flex()
        .gap(px(STACK_GAP))
        .child(switch_row_with_detail(
            theme,
            "Open Print Dialog Automatically",
            Some("When the session prints, the macOS print dialog opens here.".into()),
            printing.auto_print,
            false,
            ControlsAction::SetAutoPrint(!printing.auto_print),
            on_action,
        ))
        .child(div().h(px(1.)).bg(theme.border))
        .child(list)
}

fn print_row(theme: &Theme, row: &PrintJobRow, on_action: &ControlsHandler) -> Div {
    let file_x: Icon = gpui_kit::assets::IconName::FileX.into();
    let (glyph, tint) = match row.kind {
        PrintRowKind::Unreadable => (Icon::from(IconName::FileText), theme.warning),
        PrintRowKind::Failed => (file_x, theme.danger),
        PrintRowKind::TooLarge => (Icon::from(IconName::TriangleAlert), theme.warning),
        PrintRowKind::Missing => (file_x, theme.muted_foreground),
        _ => (Icon::from(IconName::FileText), theme.muted_foreground),
    };
    let receiving = matches!(row.kind, PrintRowKind::Receiving { .. });
    let has_file = matches!(
        row.kind,
        PrintRowKind::Ready | PrintRowKind::Unreadable | PrintRowKind::Receiving { .. }
    );
    let id = row.id;
    let button = |name: &'static str,
                  icon: Icon,
                  tooltip: &'static str,
                  action: PrintJobAction,
                  shown: bool,
                  enabled: bool| {
        let on_action = Rc::clone(on_action);
        div().when(!shown, |slot| slot.invisible()).child(
            Button::new(SharedString::from(format!("print-row-{name}-{id}")))
                .ghost()
                .xsmall()
                .icon(icon)
                .tooltip(tooltip)
                .tab_stop(false)
                .disabled(!enabled || !shown)
                .on_click(move |_, window, cx| {
                    on_action(ControlsAction::PrintJob(id, action), window, cx)
                }),
        )
    };
    let print_tooltip = if row.kind == PrintRowKind::Unreadable {
        "Open it in Preview to print"
    } else {
        "Print…"
    };
    let remove_tooltip = if row.opened {
        "Remove from this list"
    } else {
        "Remove from this list and delete the file"
    };
    let (stem, ext) = row
        .name
        .rfind('.')
        .filter(|dot| *dot > 0)
        .map_or((row.name.as_str(), ""), |dot| row.name.split_at(dot));
    let fraction = match row.kind {
        PrintRowKind::Receiving { fraction } => Some(fraction),
        _ => None,
    };
    div()
        .h_flex()
        .items_center()
        .gap(px(10.))
        .min_h(px(44.))
        .child(glyph.with_size(px(20.)).text_color(tint))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .v_flex()
                .gap(px(2.))
                .child(
                    div()
                        .h_flex()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(div().min_w_0().truncate().child(stem.to_string()))
                        .child(div().flex_none().child(ext.to_string())),
                )
                .child(
                    div()
                        .h_flex()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(div().min_w_0().truncate().child(row.meta.clone()))
                        .when_some(row.saved_to.clone(), |meta, saved| {
                            let on_action = Rc::clone(on_action);
                            meta.child(div().flex_none().child(" · ")).child(
                                div()
                                    .id(SharedString::from(format!("print-row-saved-{id}")))
                                    .flex_none()
                                    .cursor_pointer()
                                    .text_color(theme.link)
                                    .child(saved)
                                    .on_click(move |_, window, cx| {
                                        on_action(
                                            ControlsAction::PrintJob(id, PrintJobAction::Reveal),
                                            window,
                                            cx,
                                        )
                                    }),
                            )
                        }),
                )
                .when_some(fraction, |text, fraction| {
                    text.child(
                        div()
                            .h(px(2.))
                            .w_full()
                            .rounded_full()
                            .bg(theme.muted)
                            .relative()
                            .overflow_hidden()
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left_0()
                                    .bg(theme.accent)
                                    .w(gpui_kit::relative(fraction.unwrap_or(0.3))),
                            ),
                    )
                }),
        )
        .child(
            div()
                .flex_none()
                .h_flex()
                .gap(px(2.))
                .child(button(
                    "print",
                    gpui_kit::assets::IconName::Printer.into(),
                    print_tooltip,
                    PrintJobAction::Print,
                    has_file,
                    !receiving && row.kind != PrintRowKind::Unreadable,
                ))
                .child(button(
                    "open",
                    IconName::ExternalLink.into(),
                    "Open in Preview",
                    PrintJobAction::Open,
                    has_file,
                    !receiving,
                ))
                .child(button(
                    "save",
                    gpui_kit::assets::IconName::Download.into(),
                    "Save a copy…",
                    PrintJobAction::Save,
                    has_file,
                    !receiving,
                ))
                .child(button(
                    "remove",
                    IconName::Close.into(),
                    remove_tooltip,
                    PrintJobAction::Remove,
                    !receiving,
                    true,
                )),
        )
}

fn render_stats(theme: &Theme, props: &ControlsProps, on_action: &ControlsHandler) -> Div {
    let strip = switch_row_with_detail(
        theme,
        "Show over the stream",
        Some("Frame rate, video rate and latency at the top of the window.".into()),
        props.stats_strip,
        false,
        ControlsAction::SetStatsStrip(!props.stats_strip),
        on_action,
    );
    let panel = div().v_flex().gap(px(STACK_GAP)).child(strip);
    let Some(stats) = props.stats.as_ref() else {
        return panel.child(
            div()
                .text_color(theme.muted_foreground)
                .child("Waiting for stream stats…"),
        );
    };
    let on_action = Rc::clone(on_action);
    panel.child(crate::stats::render(
        theme,
        stats,
        props.stats_copied,
        move |window, cx| on_action(ControlsAction::CopyStats, window, cx),
    ))
}

/// The Shortcuts card: the switch, Skerry's chords as macOS menus write them, and, while
/// they go to the session, where Session Controls still is (`ARCH-031`).
fn render_shortcuts(theme: &Theme, shortcuts: &ShortcutsProps, on_action: &ControlsHandler) -> Div {
    let detail = if shortcuts.locked {
        "Set by the server."
    } else {
        "Let these chords act on the Mac. Off, they go to the remote session instead."
    };
    let rows = shortcuts.rows.iter().map(|row| {
        let note = if row.always_local {
            Some("Always on the Mac".to_string())
        } else if shortcuts.mac_active {
            None
        } else {
            row.remote.clone()
        };
        let dimmed = !shortcuts.mac_active && !row.always_local;
        div()
            .h_flex()
            .items_start()
            .justify_between()
            .gap(px(10.))
            .child(
                div()
                    .min_w_0()
                    .v_flex()
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(px(18.))
                            .text_color(theme.foreground)
                            .child(row.label.clone()),
                    )
                    .when_some(note, |label, note| {
                        label.child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.muted_foreground)
                                .child(note),
                        )
                    }),
            )
            .child(
                div()
                    .w(px(56.))
                    .flex_none()
                    .h_flex()
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .text_color(theme.muted_foreground)
                    .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
                    .when(dimmed, |glyphs| glyphs.opacity(0.45))
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .whitespace_nowrap()
                            .child(row.modifiers.clone()),
                    )
                    .child(
                        div()
                            .w(px(14.))
                            .flex_none()
                            .whitespace_nowrap()
                            .child(row.key.clone()),
                    ),
            )
    });
    div()
        .v_flex()
        .gap(px(STACK_GAP))
        .child(switch_row_with_detail(
            theme,
            "Mac shortcuts active",
            Some(detail.into()),
            shortcuts.mac_active,
            shortcuts.locked,
            ControlsAction::SetMacShortcuts(!shortcuts.mac_active),
            on_action,
        ))
        .child(div().v_flex().gap(px(8.)).children(rows))
        .when(!shortcuts.mac_active, |panel| {
            panel.child(
                div()
                    .h_flex()
                    .items_start()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(
                        div()
                            .pt(px(2.))
                            .child(Icon::new(IconName::Info).with_size(px(12.))),
                    )
                    .child(div().flex_1().min_w_0().child(
                        "Session Controls is still in the View menu and behind the ⋯ button \
                         at the top-right.",
                    )),
            )
        })
}

/// Gamepads: the two switches, one note at most, then four Player rows (`ARCH-032`).
fn render_gamepads(theme: &Theme, gamepads: &GamepadsProps, on_action: &ControlsHandler) -> Div {
    let muted_line = |icon: Option<IconName>, text: String| {
        div()
            .h_flex()
            .items_start()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(theme.muted_foreground)
            .when_some(icon, |line, icon| {
                line.child(div().pt(px(2.)).child(Icon::new(icon).with_size(px(12.))))
            })
            .child(div().flex_1().min_w_0().child(text))
    };
    let note = if gamepads.server_off {
        Some("Turned off by the server.")
    } else if gamepads.input && !gamepads.key {
        Some(
            "Paused while another window is in front, including this one. Click the session \
             to play.",
        )
    } else {
        None
    };
    let rows = gamepads.slots.iter().enumerate().map(|(index, slot)| {
        let number = index + 1;
        let badge = div()
            .size(px(22.))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
            .child(number.to_string());
        let badge = match slot.as_ref().map(|pad| pad.status) {
            Some(PadStatus::Sending) => badge.bg(theme.accent).text_color(gpui_kit::white()),
            Some(PadStatus::Ready) => badge
                .border_1()
                .border_color(theme.accent)
                .text_color(theme.accent),
            _ => badge
                .border_1()
                .border_color(theme.border)
                .text_color(theme.muted_foreground),
        };
        let middle = match slot {
            Some(pad) => {
                let status = match pad.status {
                    PadStatus::Sending => "Sending",
                    PadStatus::Ready => "Ready",
                    PadStatus::NotSent => "Not sent",
                };
                let status = if pad.rumble {
                    status.to_string()
                } else {
                    format!("{status} · No rumble")
                };
                div()
                    .v_flex()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(pad.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(status),
                    )
            }
            None => div()
                .text_size(px(13.))
                .text_color(theme.muted_foreground)
                .child("No controller"),
        };
        div()
            .h(px(44.))
            .px(px(4.))
            .h_flex()
            .items_center()
            .gap(px(10.))
            .child(badge)
            .child(div().flex_1().min_w_0().child(middle))
    });
    let empty = gamepads.slots.iter().all(Option::is_none);
    div()
        .v_flex()
        .gap(px(STACK_GAP))
        .child(switch_row_with_detail(
            theme,
            "Gamepad input",
            Some("Send controllers to this session while its window is in front.".into()),
            gamepads.input,
            gamepads.server_off || gamepads.input_locked,
            ControlsAction::SetGamepadInput(!gamepads.input),
            on_action,
        ))
        .child(switch_row_with_detail(
            theme,
            "Rumble",
            Some("Let the session's games vibrate your controllers.".into()),
            gamepads.rumble && gamepads.input,
            gamepads.server_off || !gamepads.input,
            ControlsAction::SetGamepadRumble(!gamepads.rumble),
            on_action,
        ))
        .when_some(note, |panel, note| {
            panel.child(muted_line(Some(IconName::Info), note.to_string()))
        })
        .child(field("Players", div().v_flex().gap(px(4.)).children(rows)))
        .when(empty, |panel| {
            panel.child(muted_line(
                None,
                "Pair a controller in System Settings › Bluetooth.".into(),
            ))
        })
        .when(gamepads.overflow > 0, |panel| {
            let text = if gamepads.overflow == 1 {
                "1 more controller isn't sent: all four players are in use.".to_string()
            } else {
                format!(
                    "{} more controllers aren't sent: all four players are in use.",
                    gamepads.overflow
                )
            };
            panel.child(muted_line(Some(IconName::TriangleAlert), text))
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
    section_with_status(theme, title, open, None, body, toggle)
}

/// A card with a status before its chevron, shown open or collapsed.
fn section_with_status(
    theme: &Theme,
    title: &'static str,
    open: bool,
    status: Option<AnyElement>,
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
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap(px(8.))
                        .children(status)
                        .child(div().text_color(theme.muted_foreground).child(if open {
                            IconName::ChevronUp
                        } else {
                            IconName::ChevronDown
                        })),
                )
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

/// A switch row with a muted 12 pt line under its label.
fn switch_row_with_detail(
    theme: &Theme,
    label: impl Into<String>,
    detail: Option<String>,
    enabled: bool,
    locked: bool,
    action: ControlsAction,
    on_action: &ControlsHandler,
) -> AnyElement {
    let Some(detail) = detail else {
        return switch_row(label, enabled, locked, action, on_action);
    };
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
                .v_flex()
                .gap(px(2.))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(label.clone()))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(detail),
                ),
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
    fn header_offers_apps_only_when_available() {
        let labels = |apps: bool| -> Vec<&'static str> {
            header_actions_for(Some(true), Some(true), false, apps)
                .into_iter()
                .map(|(label, _)| label)
                .collect()
        };
        assert_eq!(
            labels(true),
            [
                "Mute",
                "Pause Video",
                "Full Screen",
                "Files",
                "Apps",
                "Reconnect",
                "Disconnect"
            ]
        );
        assert!(header_actions_for(None, None, true, true)
            .contains(&("Apps", ControlsAction::OpenApps)));
        assert_eq!(
            labels(false),
            [
                "Mute",
                "Pause Video",
                "Full Screen",
                "Files",
                "Reconnect",
                "Disconnect"
            ]
        );
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
        // Input, Shortcuts, Gamepads, Printing, Video and Audio open, Screen closed.
        let open = [
            true, true, true, false, true, false, true, true, false, false,
        ];
        let state = ControlsUiState::default().with_sections(open);
        assert_eq!(state.sections(), open);
        assert!(state.input_open && state.video_open && state.audio_open && !state.screen_open);
        assert!(state.shortcuts_open && state.gamepads_open && state.printing_open);
        assert_eq!(Section::ALL[1], Section::Shortcuts);
        assert_eq!(Section::ALL[2], Section::Gamepads);
        assert_eq!(
            ControlsUiState::default().sections(),
            [false, false, false, false, false, true, false, false, false, false]
        );
    }

    #[test]
    fn microphone_and_camera_sit_between_audio_and_stats() {
        let audio = Section::ALL
            .iter()
            .position(|s| *s == Section::Audio)
            .unwrap();
        assert_eq!(Section::ALL[audio + 1], Section::Capture);
        assert_eq!(Section::ALL[audio + 2], Section::Stats);
        let state = ControlsUiState::default().with_sections([
            false, false, false, false, false, false, false, false, true, false,
        ]);
        assert!(state.capture_open && !state.stats_open && !state.audio_open);
        assert!(
            !ControlsUiState::default().capture_open,
            "collapsed by default"
        );
    }

    #[test]
    fn media_capture_header_line() {
        let device = CaptureDeviceProps {
            on: false,
            locked: false,
            picker_locked: false,
            live: false,
            detail: None,
            denied: false,
            device_label: "System Default".into(),
            devices: Vec::new(),
            note: None,
            level: None,
        };
        let mut props = MediaCaptureProps {
            mic: device.clone(),
            camera: Some(device),
        };
        assert_eq!(media_capture_header(&props), None);
        props.mic.live = true;
        assert_eq!(media_capture_header(&props), Some("Microphone on"));
        props.camera.as_mut().unwrap().live = true;
        assert_eq!(
            media_capture_header(&props),
            Some("Microphone and camera on")
        );
        props.mic.live = false;
        assert_eq!(media_capture_header(&props), Some("Camera on"));
        props.camera = None;
        assert_eq!(media_capture_header(&props), None);
    }

    #[test]
    fn the_printing_card_shows_only_while_printing_is_on() {
        assert!(!printing_card_shown(None));
        let printing = PrintingProps {
            auto_print: true,
            jobs: Vec::new(),
            at_cap: false,
        };
        assert!(printing_card_shown(Some(&printing)));
    }

    #[test]
    fn printing_follows_clipboard_in_section_order() {
        let clipboard = Section::ALL.iter().position(|s| *s == Section::Clipboard);
        let printing = Section::ALL.iter().position(|s| *s == Section::Printing);
        assert_eq!(printing, clipboard.map(|index| index + 1));
        let state = ControlsUiState::default().with_sections([
            false, false, false, false, true, false, false, false, false, false,
        ]);
        assert!(state.printing_open && !state.screen_open);
    }
}
