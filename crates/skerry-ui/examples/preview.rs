use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::{InputEvent, InputState},
    scroll::ScrollableElement as _,
    select::SelectEvent,
    ActiveTheme as _, Sizable as _, StyledExt as _, Theme, ThemeMode,
};
use gpui_kit::{
    actions, div, point, prelude::*, px, rgb, size, App, Bounds, Context, Entity, FontWeight,
    KeyBinding, RenderImage, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use skerry_ui::{
    components::{self, Toast, ToastKind},
    connect::{self, ConnectProps},
    controls::{
        self, ChordItem, ClipboardProps, CodecChoice, ControlsAction, ControlsProps,
        ControlsUiState, ScreenProps, SliderUnit, SwitchItem, VideoProps, VideoSettingControl,
        VideoSettingRow,
    },
    files::{
        self as files_ui, Crumb, Direction, FileRow, FilesAction, FilesProps, SortColumn,
        TransferRow, TransferStatus,
    },
    launch_dialog::{
        self, LaunchDialogAction, LaunchDialogProps, LaunchDialogSelects, LaunchOption,
        LaunchOptions, LaunchSelect, RunningMatch,
    },
    library::{
        self, AppItem, Filter, LaunchNotice, LaunchingItem, LibraryHandlers, LibraryProps,
        SessionItem, StageRow, StageState,
    },
    session::{self, KeySourceLabel},
    stats,
};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

actions!(skerry_preview, [Quit]);

/// Writes one dropdown's confirmed choice into the Launch Dialog's props.
type ApplyChoice = fn(&mut LaunchDialogProps, String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Story {
    ConnectDefault,
    ConnectBusy,
    ConnectError,
    LibraryPopulated,
    LibraryLaunchDialog,
    LibraryLaunchDialogMinimal,
    LibraryEmpty,
    LibraryBusy,
    LibrarySignOut,
    LibrarySignOutEnv,
    LibrarySignOutDotEnv,
    LibrarySignOutUnsaved,
    LibraryToast,
    LibraryLaunchDialogCopied,
    LibraryLongServer,
    LibraryOrganized,
    LibraryHiddenChip,
    LibrarySearchHidden,
    LibraryContextMenu,
    LibraryLaunching,
    LibraryLaunchingPull,
    LibraryLaunchStalled,
    LibraryLaunchFailed,
    LibraryLaunchDialogRunning,
    LibraryLaunchDialogHidden,
    ControlsHardware,
    ControlsJpeg,
    ControlsServerLocked,
    ControlsDropdownOpen,
    ControlsRestoredSections,
    SessionOverlays,
    FilesBrowsing,
    FilesEmpty,
    FilesDownloadOnly,
}

impl Story {
    const ALL: &'static [Story] = &[
        Story::ConnectDefault,
        Story::ConnectBusy,
        Story::ConnectError,
        Story::LibraryPopulated,
        Story::LibraryLaunchDialog,
        Story::LibraryLaunchDialogMinimal,
        Story::LibraryEmpty,
        Story::LibraryBusy,
        Story::LibrarySignOut,
        Story::LibrarySignOutEnv,
        Story::LibrarySignOutDotEnv,
        Story::LibrarySignOutUnsaved,
        Story::LibraryToast,
        Story::LibraryLaunchDialogCopied,
        Story::LibraryLongServer,
        Story::LibraryOrganized,
        Story::LibraryHiddenChip,
        Story::LibrarySearchHidden,
        Story::LibraryContextMenu,
        Story::LibraryLaunching,
        Story::LibraryLaunchingPull,
        Story::LibraryLaunchStalled,
        Story::LibraryLaunchFailed,
        Story::LibraryLaunchDialogRunning,
        Story::LibraryLaunchDialogHidden,
        Story::ControlsHardware,
        Story::ControlsJpeg,
        Story::ControlsServerLocked,
        Story::ControlsDropdownOpen,
        Story::ControlsRestoredSections,
        Story::SessionOverlays,
        Story::FilesBrowsing,
        Story::FilesEmpty,
        Story::FilesDownloadOnly,
    ];

    fn group(self) -> &'static str {
        match self {
            Story::ConnectDefault | Story::ConnectBusy | Story::ConnectError => "Connect",
            Story::LibraryPopulated
            | Story::LibraryLaunchDialog
            | Story::LibraryLaunchDialogMinimal
            | Story::LibraryEmpty
            | Story::LibraryBusy
            | Story::LibrarySignOut
            | Story::LibrarySignOutEnv
            | Story::LibrarySignOutDotEnv
            | Story::LibrarySignOutUnsaved
            | Story::LibraryToast
            | Story::LibraryLaunchDialogCopied
            | Story::LibraryLongServer
            | Story::LibraryOrganized
            | Story::LibraryHiddenChip
            | Story::LibrarySearchHidden
            | Story::LibraryContextMenu
            | Story::LibraryLaunching
            | Story::LibraryLaunchingPull
            | Story::LibraryLaunchStalled
            | Story::LibraryLaunchFailed
            | Story::LibraryLaunchDialogRunning
            | Story::LibraryLaunchDialogHidden => "Library",
            Story::ControlsHardware
            | Story::ControlsJpeg
            | Story::ControlsServerLocked
            | Story::ControlsDropdownOpen
            | Story::ControlsRestoredSections => "Session Controls",
            Story::SessionOverlays => "Session Overlays",
            Story::FilesBrowsing | Story::FilesEmpty | Story::FilesDownloadOnly => "Session Files",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Story::ConnectDefault => "Default",
            Story::ConnectBusy => "Connecting",
            Story::ConnectError => "Error",
            Story::LibraryPopulated => "Populated",
            Story::LibraryLaunchDialog => "Launch Dialog",
            Story::LibraryLaunchDialogMinimal => "Launch Dialog · Minimal",
            Story::LibraryEmpty => "Empty",
            Story::LibraryBusy => "Busy + Status",
            Story::LibrarySignOut => "Sign Out · Saved key",
            Story::LibrarySignOutEnv => "Sign Out · Environment key",
            Story::LibrarySignOutDotEnv => "Sign Out · .env key",
            Story::LibrarySignOutUnsaved => "Sign Out · Not saved",
            Story::LibraryToast => "Toast · Library refreshed",
            Story::LibraryLaunchDialogCopied => "Launch Dialog · Copied",
            Story::LibraryLongServer => "Long server name",
            Story::LibraryOrganized => "Organized",
            Story::LibraryHiddenChip => "Hidden Chip",
            Story::LibrarySearchHidden => "Search Finds Hidden",
            Story::LibraryContextMenu => "Tile Menu",
            Story::LibraryLaunching => "Launching",
            Story::LibraryLaunchingPull => "Pulling Image",
            Story::LibraryLaunchStalled => "Launch Stalled",
            Story::LibraryLaunchFailed => "Launch Failed",
            Story::LibraryLaunchDialogRunning => "Launch Dialog · Running",
            Story::LibraryLaunchDialogHidden => "Launch Dialog · Hidden",
            Story::ControlsHardware => "Hardware H.265",
            Story::ControlsJpeg => "JPEG + Paint-Over",
            Story::ControlsServerLocked => "Server Locked",
            Story::ControlsDropdownOpen => "Dropdown Open",
            Story::ControlsRestoredSections => "Restored Sections",
            Story::SessionOverlays => "Waiting + IME + Banner",
            Story::FilesBrowsing => "Browsing + Transfers",
            Story::FilesEmpty => "Empty Folder + New Folder",
            Story::FilesDownloadOnly => "Download Only + Error",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Story::ConnectDefault => "story-connect-default",
            Story::ConnectBusy => "story-connect-busy",
            Story::ConnectError => "story-connect-error",
            Story::LibraryPopulated => "story-library-populated",
            Story::LibraryLaunchDialog => "story-library-launch-dialog",
            Story::LibraryLaunchDialogMinimal => "story-library-launch-dialog-minimal",
            Story::LibraryEmpty => "story-library-empty",
            Story::LibraryBusy => "story-library-busy",
            Story::LibrarySignOut => "story-library-sign-out",
            Story::LibrarySignOutEnv => "story-library-sign-out-env",
            Story::LibrarySignOutDotEnv => "story-library-sign-out-dotenv",
            Story::LibrarySignOutUnsaved => "story-library-sign-out-unsaved",
            Story::LibraryToast => "story-library-toast",
            Story::LibraryLaunchDialogCopied => "story-library-launch-dialog-copied",
            Story::LibraryLongServer => "story-library-long-server",
            Story::LibraryOrganized => "story-library-organized",
            Story::LibraryHiddenChip => "story-library-hidden-chip",
            Story::LibrarySearchHidden => "story-library-search-hidden",
            Story::LibraryContextMenu => "story-library-context-menu",
            Story::LibraryLaunching => "story-library-launching",
            Story::LibraryLaunchingPull => "story-library-launching-pull",
            Story::LibraryLaunchStalled => "story-library-launch-stalled",
            Story::LibraryLaunchFailed => "story-library-launch-failed",
            Story::LibraryLaunchDialogRunning => "story-library-launch-dialog-running",
            Story::LibraryLaunchDialogHidden => "story-library-launch-dialog-hidden",
            Story::ControlsHardware => "story-controls-hardware",
            Story::ControlsJpeg => "story-controls-jpeg",
            Story::ControlsServerLocked => "story-controls-locked",
            Story::ControlsDropdownOpen => "story-controls-dropdown-open",
            Story::ControlsRestoredSections => "story-controls-restored-sections",
            Story::SessionOverlays => "story-session-overlays",
            Story::FilesBrowsing => "story-files-browsing",
            Story::FilesEmpty => "story-files-empty",
            Story::FilesDownloadOnly => "story-files-download-only",
        }
    }

    /// Stories that show the Launch Dialog over the Library.
    fn has_launch_dialog(self) -> bool {
        matches!(
            self,
            Story::LibraryLaunchDialog
                | Story::LibraryLaunchDialogMinimal
                | Story::LibraryLaunchDialogCopied
                | Story::LibraryLaunchDialogRunning
                | Story::LibraryLaunchDialogHidden
        )
    }

    /// The chip a story opens on.
    fn filter(self) -> Filter {
        if self == Story::LibraryHiddenChip {
            Filter::Hidden
        } else {
            Filter::All
        }
    }

    /// The search a story opens with.
    fn query(self) -> &'static str {
        if self == Story::LibrarySearchHidden {
            "o"
        } else {
            ""
        }
    }

    /// The note in the workbench's top bar when the story opens.
    fn note(self) -> Option<&'static str> {
        (self == Story::LibraryContextMenu).then_some("Right-click a tile to open its menu")
    }

    /// Krita and Obsidian are the hidden apps (Krita a favorite too); elsewhere two others
    /// are hidden so Krita and Obsidian can fill the Recent row.
    fn hides_krita(self) -> bool {
        matches!(self, Story::LibraryHiddenChip | Story::LibrarySearchHidden)
    }

    /// Stories drawn with the Session Controls panel.
    fn is_controls(self) -> bool {
        matches!(
            self,
            Story::ControlsHardware
                | Story::ControlsJpeg
                | Story::ControlsServerLocked
                | Story::ControlsDropdownOpen
                | Story::ControlsRestoredSections
        )
    }

    /// The Sign Out confirmation a story shows, if any.
    fn sign_out(self) -> Option<KeySourceLabel> {
        match self {
            Story::LibrarySignOut => Some(KeySourceLabel::SavedFile),
            Story::LibrarySignOutEnv => Some(KeySourceLabel::Environment),
            Story::LibrarySignOutDotEnv => Some(KeySourceLabel::DotEnv),
            Story::LibrarySignOutUnsaved => Some(KeySourceLabel::ThisRunOnly),
            _ => None,
        }
    }

    /// The Session Controls state a story opens with.
    fn controls_ui(self) -> ControlsUiState {
        let base = ControlsUiState {
            video_open: true,
            stats_open: true,
            ..ControlsUiState::default()
        };
        match self {
            // Input and Video open, Screen closed: what a server remembered.
            Story::ControlsRestoredSections => {
                ControlsUiState::default().with_sections([true, false, false, true, false])
            }
            Story::ControlsDropdownOpen => ControlsUiState {
                open_menu: Some("preset-picker"),
                ..ControlsUiState::default()
            },
            _ => base,
        }
    }
}

/// The canvas width presets: `None` fills the stage.
const CANVAS_WIDTHS: [Option<f32>; 5] = [None, Some(700.), Some(1200.), Some(1800.), Some(2540.)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Appearance {
    System,
    Light,
    Dark,
}

struct PreviewAssets {
    icon_zen: Arc<RenderImage>,
    icon_lightcraft: Arc<RenderImage>,
    icon_weixin: Arc<RenderImage>,
    icon_vscode: Arc<RenderImage>,
    icon_firefox: Arc<RenderImage>,
    thumb_zen: Arc<RenderImage>,
    thumb_lightcraft: Arc<RenderImage>,
}

impl PreviewAssets {
    fn build() -> Self {
        Self {
            icon_zen: fixtures::zen_icon(),
            icon_lightcraft: fixtures::lightcraft_icon(),
            icon_weixin: fixtures::weixin_icon(),
            icon_vscode: fixtures::vscode_icon(),
            icon_firefox: fixtures::firefox_icon(),
            thumb_zen: fixtures::zen_thumbnail(),
            thumb_lightcraft: fixtures::lightcraft_thumbnail(),
        }
    }
}

struct PreviewApp {
    story: Story,
    appearance: Appearance,
    last_event: Option<String>,
    assets: PreviewAssets,
    key_input: Entity<InputState>,
    search_input: Entity<InputState>,
    selected_filter: Filter,
    launch_dialog: Option<(LaunchDialogProps, LaunchDialogSelects)>,
    controls_ui: ControlsUiState,
    controls_props: ControlsProps,
    width: Entity<InputState>,
    height: Entity<InputState>,
    files_props: FilesProps,
    folder_name: Entity<InputState>,
    files_scroll: gpui_kit::UniformListScrollHandle,
    /// A fixed story canvas width, to check wide layouts on a small screen.
    canvas_width: Option<f32>,
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
        let search_input = library::new_search_input(window, cx);
        cx.subscribe_in(
            &search_input,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => {
                    let query = this.search_input.read(cx).value().to_string();
                    if !query.trim().is_empty() {
                        this.record_event(format!("Omnibar: Submit `{}`", query.trim()), cx);
                    }
                }
                _ => {}
            },
        )
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

        // "System" is the initial choice, so start in the system appearance and follow it.
        Theme::sync_system_appearance(Some(window), cx);
        cx.observe_window_appearance(window, |this, window, cx| {
            if matches!(this.appearance, Appearance::System) {
                Theme::sync_system_appearance(Some(window), cx);
                cx.notify();
            }
        })
        .detach();
        let assets = PreviewAssets::build();
        // `SKERRY_PREVIEW_STORY=<story id>` opens the workbench on that story (for screenshots).
        let story = std::env::var("SKERRY_PREVIEW_STORY")
            .ok()
            .and_then(|id| Story::ALL.iter().copied().find(|story| story.id() == id))
            .unwrap_or(Story::LibraryLaunchDialog);
        let initial_dialog = Self::story_dialog(story, &assets).map(|dialog| {
            let selects = Self::bind_launch_selects(&dialog, window, cx);
            (dialog, selects)
        });
        search_input.update(cx, |state, cx| state.set_value(story.query(), window, cx));
        // `SKERRY_PREVIEW_APPEARANCE=light|dark` and `SKERRY_PREVIEW_CANVAS_W=<pt>` set the
        // appearance and canvas width at launch (for screenshots).
        let appearance = match std::env::var("SKERRY_PREVIEW_APPEARANCE").as_deref() {
            Ok("light") => Appearance::Light,
            Ok("dark") => Appearance::Dark,
            _ => Appearance::System,
        };
        match appearance {
            Appearance::Light => Theme::change(ThemeMode::Light, Some(window), cx),
            Appearance::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
            Appearance::System => {}
        }
        let canvas_width = std::env::var("SKERRY_PREVIEW_CANVAS_W")
            .ok()
            .and_then(|width| width.parse::<f32>().ok())
            .filter(|width| *width > 0.)
            .or((story == Story::LibraryLongServer).then_some(700.));
        Self {
            story,
            appearance,
            last_event: story.note().map(Into::into),
            assets,
            key_input,
            search_input,
            selected_filter: story.filter(),
            launch_dialog: initial_dialog,
            controls_ui: story.controls_ui(),
            controls_props: Self::fixture_controls(story),
            width,
            height,
            files_props: Self::fixture_files(story),
            folder_name: cx.new(|cx| InputState::new(window, cx).default_value("untitled folder")),
            files_scroll: gpui_kit::UniformListScrollHandle::new(),
            canvas_width,
        }
    }

    fn fixture_files(story: Story) -> FilesProps {
        let row = |name: &str, is_dir: bool, modified: &str, size: &str| FileRow {
            name: name.into(),
            is_dir,
            modified: modified.into(),
            size: size.into(),
            selected: false,
        };
        let mut rows = vec![
            row("Projects", true, "Yesterday at 18:02", "--"),
            row("Screenshots", true, "Today at 09:14", "--"),
            row("budget 2027.xlsx", false, "19 Nov 2026 at 11:40", "48 KB"),
            row("holiday-photos.zip", false, "2 Nov 2026 at 08:05", "1.2 GB"),
            row("notes.md", false, "Today at 10:04", "3.1 KB"),
            row(
                "A very long file name that keeps going and going until it truncates.pdf",
                false,
                "30 Oct 2026 at 16:22",
                "820 KB",
            ),
        ];
        rows[2].selected = true;
        rows[4].selected = true;
        let crumbs = vec![
            Crumb {
                label: "Desktop".into(),
                path: String::new(),
            },
            Crumb {
                label: "Work".into(),
                path: "Work".into(),
            },
        ];
        match story {
            Story::FilesEmpty => FilesProps {
                crumbs,
                can_back: true,
                can_forward: false,
                can_upload: true,
                can_download: true,
                can_delete: true,
                rows: Rc::new(Vec::new()),
                sort: SortColumn::Name,
                ascending: true,
                loading: false,
                error: None,
                notice: None,
                busy: None,
                transfers_open: true,
                focused: true,
                transfers: Vec::new(),
                creating_folder: true,
                selected: 0,
            },
            Story::FilesDownloadOnly => FilesProps {
                crumbs,
                can_back: false,
                can_forward: true,
                can_upload: false,
                can_download: true,
                can_delete: false,
                rows: Rc::new(Vec::new()),
                sort: SortColumn::Modified,
                ascending: false,
                loading: false,
                error: Some("The session refused the request: permission denied.".into()),
                notice: Some(files_ui::Notice {
                    text: "The server doesn’t allow uploads.".into(),
                    error: true,
                }),
                busy: None,
                transfers_open: true,
                focused: true,
                transfers: Vec::new(),
                creating_folder: false,
                selected: 0,
            },
            _ => FilesProps {
                crumbs,
                can_back: true,
                can_forward: false,
                can_upload: true,
                can_download: true,
                can_delete: true,
                rows: Rc::new(rows),
                sort: SortColumn::Name,
                ascending: true,
                loading: false,
                error: None,
                notice: Some(files_ui::Notice {
                    text: "Copied 2 items. Paste in Finder to save them.".into(),
                    error: false,
                }),
                busy: Some("Deleting 1 item…".into()),
                transfers_open: true,
                focused: true,
                transfers: vec![
                    TransferRow {
                        id: 1,
                        name: "holiday-photos.zip".into(),
                        direction: Direction::Download,
                        status: TransferStatus::Running {
                            fraction: Some(0.42),
                            detail: "504 MB of 1.2 GB".into(),
                        },
                        revealable: false,
                    },
                    TransferRow {
                        id: 2,
                        name: "3 items".into(),
                        direction: Direction::Upload,
                        status: TransferStatus::Running {
                            fraction: None,
                            detail: "Preparing…".into(),
                        },
                        revealable: false,
                    },
                    TransferRow {
                        id: 3,
                        name: "notes.md".into(),
                        direction: Direction::Download,
                        status: TransferStatus::Done("Uploaded as “notes 2.md”".into()),
                        revealable: true,
                    },
                    TransferRow {
                        id: 4,
                        name: "huge.iso".into(),
                        direction: Direction::Upload,
                        status: TransferStatus::Failed("Uploads disabled.".into()),
                        revealable: false,
                    },
                ],
                creating_folder: false,
                selected: 2,
            },
        }
    }

    fn select_story(&mut self, story: Story, window: &mut Window, cx: &mut Context<Self>) {
        self.story = story;
        self.files_props = Self::fixture_files(story);
        self.last_event = story.note().map(Into::into);
        if let Some(dialog) = Self::story_dialog(story, &self.assets) {
            let selects = Self::bind_launch_selects(&dialog, window, cx);
            self.launch_dialog = Some((dialog, selects));
        } else if story.group() == "Library" {
            self.launch_dialog = None;
        }
        if story.group() == "Library" {
            self.selected_filter = story.filter();
            self.search_input
                .update(cx, |state, cx| state.set_value(story.query(), window, cx));
        }
        if story == Story::LibraryLongServer {
            self.canvas_width = Some(700.);
        }
        if story.is_controls() {
            self.controls_props = Self::fixture_controls(story);
            self.controls_ui = story.controls_ui();
            self.width.update(cx, |s, cx| s.set_value("", window, cx));
            self.height.update(cx, |s, cx| s.set_value("", window, cx));
        }
        cx.notify();
    }

    /// The Launch Dialog a story opens with, if any.
    fn story_dialog(story: Story, assets: &PreviewAssets) -> Option<LaunchDialogProps> {
        if !story.has_launch_dialog() {
            return None;
        }
        let mut dialog = if story == Story::LibraryLaunchDialogHidden {
            let mut dialog = Self::fixture_launch_dialog("krita", "Krita", None, false);
            dialog.hidden = true;
            dialog
        } else {
            Self::fixture_launch_dialog(
                "zen",
                "Zen",
                Some(Arc::clone(&assets.icon_zen)),
                story == Story::LibraryLaunchDialogMinimal,
            )
        };
        dialog.link_copied = story == Story::LibraryLaunchDialogCopied;
        if story == Story::LibraryLaunchDialogRunning {
            dialog.favorite = true;
            dialog.running_match = Some(RunningMatch {
                started_label: Some("started 12m ago".into()),
            });
        }
        Some(dialog)
    }

    /// A Launch Dialog as a SealSkin server on Kubernetes fills it: named homes, an Intel GPU
    /// PodTemplate, a VPN with two endpoints, and the full locale list. `minimal` is a user
    /// without persistent storage, GPU, or VPN rights, where only Cleanroom and Language show.
    fn fixture_launch_dialog(
        app_id: &str,
        app_name: &str,
        app_icon: Option<Arc<RenderImage>>,
        minimal: bool,
    ) -> LaunchDialogProps {
        let option = |id: &str, label: &str| LaunchOption {
            id: id.into(),
            label: label.into(),
        };
        // Without persistent storage the web client hides Storage and launches in Cleanroom.
        let storage_options = if minimal {
            vec![]
        } else {
            vec![
                option("auto", "Auto (per-app persistence)"),
                option("cleanroom", "Cleanroom (temporary)"),
                option("work", "work"),
                option("research", "research"),
            ]
        };
        let gpu_options = if minimal {
            vec![]
        } else {
            vec![option("", "None"), option("intel-gpu", "intel-gpu (i915)")]
        };
        let vpn_options = if minimal {
            vec![]
        } else {
            vec![
                option("", "No VPN"),
                option("mullvad", "Mullvad"),
                option("office", "Office"),
            ]
        };
        LaunchDialogProps {
            app_id: app_id.into(),
            app_name: app_name.into(),
            app_icon,
            favorite: false,
            hidden: false,
            selected_storage: if minimal { "cleanroom" } else { "auto" }.into(),
            storage_options,
            gpu_options,
            // The last launch of this app used the GPU, so it is chosen again.
            selected_gpu: "intel-gpu".into(),
            vpn_options,
            selected_vpn: if minimal {
                String::new()
            } else {
                "mullvad".into()
            },
            vpn_endpoint_options: Self::fixture_vpn_endpoints(if minimal { "" } else { "mullvad" }),
            selected_vpn_endpoint: "mullvad-de".into(),
            language_options: fixture_languages(),
            selected_language: "de_DE.UTF-8".into(),
            collaborative: false,
            wayland: true,
            busy: false,
            show_copy_link: true,
            show_organize: !minimal,
            link_copied: false,
            running_match: None,
        }
    }

    fn fixture_vpn_endpoints(vpn: &str) -> Vec<LaunchOption> {
        let option = |id: &str, label: &str| LaunchOption {
            id: id.into(),
            label: label.into(),
        };
        match vpn {
            "mullvad" => vec![
                option("mullvad-de", "de-fra-wg-001"),
                option("mullvad-nl", "nl-ams-wg-004"),
                option("mullvad-se", "se-sto-wg-012"),
            ],
            "office" => vec![option("office-1", "vpn.office.example")],
            _ => vec![],
        }
    }

    /// Writes a confirmed choice into the dialog's props and records it in the event log.
    fn on_launch_select(
        select: &LaunchSelect,
        field: &'static str,
        apply: ApplyChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(
            select,
            window,
            move |this, _, event: &SelectEvent<LaunchOptions>, window, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    if let Some((dialog, selects)) = this.launch_dialog.as_mut() {
                        apply(dialog, value.clone());
                        if field == "VPN" {
                            // A different VPN has different endpoints; rebuild that field.
                            dialog.vpn_endpoint_options = Self::fixture_vpn_endpoints(value);
                            dialog.selected_vpn_endpoint = dialog
                                .vpn_endpoint_options
                                .first()
                                .map(|endpoint| endpoint.id.clone())
                                .unwrap_or_default();
                            let dialog = dialog.clone();
                            selects.vpn_endpoint =
                                LaunchDialogSelects::build_vpn_endpoint(&dialog, window, cx);
                            if let Some(endpoint) = selects.vpn_endpoint.clone() {
                                Self::on_launch_select(
                                    &endpoint,
                                    "VPN endpoint",
                                    |dialog, value| dialog.selected_vpn_endpoint = value,
                                    window,
                                    cx,
                                );
                            }
                        }
                    }
                    this.record_event(format!("LaunchDialog: {field} -> `{value}`"), cx);
                }
            },
        )
        .detach();
    }

    fn bind_launch_selects(
        props: &LaunchDialogProps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> LaunchDialogSelects {
        let selects = LaunchDialogSelects::from_props(props, window, cx);
        let fields: [(&Option<LaunchSelect>, &'static str, ApplyChoice); 5] = [
            (&selects.storage, "Storage", |d, v| d.selected_storage = v),
            (&selects.gpu, "GPU", |d, v| d.selected_gpu = v),
            (&selects.vpn, "VPN", |d, v| d.selected_vpn = v),
            (&selects.vpn_endpoint, "VPN endpoint", |d, v| {
                d.selected_vpn_endpoint = v
            }),
            (&selects.language, "Language", |d, v| {
                d.selected_language = v
            }),
        ];
        for (select, field, apply) in fields {
            if let Some(select) = select {
                Self::on_launch_select(select, field, apply, window, cx);
            }
        }
        selects
    }

    fn handle_launch_dialog_action(&mut self, action: LaunchDialogAction, cx: &mut Context<Self>) {
        self.last_event = Some(format!("LaunchDialog: {action:?}"));
        match action {
            LaunchDialogAction::Close => {
                if self.story == Story::LibraryPopulated {
                    self.launch_dialog = None;
                }
            }
            LaunchDialogAction::ToggleFavorite => {
                if let Some((dialog, _)) = self.launch_dialog.as_mut() {
                    dialog.favorite = !dialog.favorite;
                }
            }
            LaunchDialogAction::ToggleHidden => {
                if let Some((dialog, _)) = self.launch_dialog.as_mut() {
                    dialog.hidden = !dialog.hidden;
                }
            }
            LaunchDialogAction::SetCollaborative(enabled) => {
                if let Some((dialog, _)) = self.launch_dialog.as_mut() {
                    dialog.collaborative = enabled;
                }
            }
            LaunchDialogAction::SetWayland(enabled) => {
                if let Some((dialog, _)) = self.launch_dialog.as_mut() {
                    dialog.wayland = enabled;
                }
            }
            LaunchDialogAction::CopyLink => {}
            LaunchDialogAction::Launch => {
                if self.story == Story::LibraryPopulated {
                    self.launch_dialog = None;
                }
            }
            LaunchDialogAction::OpenRunning => {
                self.last_event = Some("Dialog: Open Running Session".into());
            }
        }
        cx.notify();
    }

    fn app_item(&self, id: &str, name: &str, category: &str) -> AppItem {
        let icon = match id {
            "lightcraft" => Some(&self.assets.icon_lightcraft),
            "weixin" => Some(&self.assets.icon_weixin),
            "zen" => Some(&self.assets.icon_zen),
            "vscode" => Some(&self.assets.icon_vscode),
            "firefox" => Some(&self.assets.icon_firefox),
            _ => None,
        };
        AppItem {
            id: id.into(),
            name: name.into(),
            categories: vec![category.into()],
            favorite: false,
            hidden: false,
            remembered: false,
            icon: icon.map(Arc::clone),
        }
    }

    /// The Library each Library story draws.
    fn fixture_library(&self, server: &str, cx: &App) -> LibraryProps {
        let story = self.story;
        let organized = !matches!(
            story,
            Story::LibraryPopulated
                | Story::LibraryLaunchDialog
                | Story::LibraryLaunchDialogMinimal
                | Story::LibrarySignOut
                | Story::LibrarySignOutEnv
                | Story::LibrarySignOutDotEnv
                | Story::LibrarySignOutUnsaved
                | Story::LibraryToast
                | Story::LibraryLaunchDialogCopied
                | Story::LibraryLongServer
        );
        let mut apps = vec![
            self.app_item("lightcraft", "LightCraft", "app"),
            self.app_item("weixin", "Weixin", "app"),
            self.app_item("zen", "Zen", "browser"),
            self.app_item("vscode", "VS Code", "app"),
            self.app_item("firefox", "Firefox", "browser"),
        ];
        let (mut favorites, mut recent) = (Vec::new(), Vec::new());
        if organized {
            apps.push(self.app_item("krita", "Krita", "app"));
            apps.push(self.app_item("obsidian", "Obsidian", "app"));
            let hidden: &[&str] = if story.hides_krita() {
                &["krita", "obsidian"]
            } else {
                apps.push(self.app_item("blender", "Blender", "app"));
                apps.push(self.app_item("inkscape", "Inkscape", "app"));
                &["blender", "inkscape"]
            };
            favorites = if story.hides_krita() {
                vec!["zen".to_string(), "krita".to_string()]
            } else {
                vec![
                    "zen".to_string(),
                    "vscode".to_string(),
                    "firefox".to_string(),
                ]
            };
            recent = ["zen", "lightcraft", "weixin", "krita", "obsidian"]
                .map(String::from)
                .to_vec();
            for app in &mut apps {
                app.favorite = favorites.contains(&app.id);
                app.hidden = hidden.contains(&app.id.as_str());
                app.remembered = app.id == "zen";
            }
        }
        let session =
            |id: &str, name: &str, storage: &str, thumb: &Arc<RenderImage>, icon| SessionItem {
                id: id.into(),
                app_name: name.into(),
                started_label: Some("Started 2 days ago".into()),
                gpu: true,
                storage_label: Some(storage.into()),
                thumbnail: Some(Arc::clone(thumb)),
                app_icon: Some(Arc::clone(icon)),
            };
        let sessions: Vec<SessionItem> = match story {
            Story::LibraryLaunching | Story::LibraryLaunchingPull | Story::LibraryLaunchFailed => {
                Vec::new()
            }
            Story::LibraryLaunchStalled => vec![session(
                "sess-zen",
                "Zen",
                "auto-zen",
                &self.assets.thumb_zen,
                &self.assets.icon_zen,
            )],
            _ => vec![
                session(
                    "sess-zen",
                    "Zen",
                    "auto-zen",
                    &self.assets.thumb_zen,
                    &self.assets.icon_zen,
                ),
                session(
                    "sess-lightcraft",
                    "LightCraft",
                    "auto-lightcraft",
                    &self.assets.thumb_lightcraft,
                    &self.assets.icon_lightcraft,
                ),
            ],
        };
        let (launching, launching_apps) = self.fixture_launching();
        LibraryProps {
            app_name: "SealSkin Native".into(),
            server: server.into(),
            apps: apps.into(),
            sessions: sessions.into(),
            favorites,
            recent,
            launching,
            launching_apps,
            search_query: self.search_input.read(cx).value().to_string(),
            selected_filter: self.selected_filter.clone(),
            status: None,
            busy: None,
        }
    }

    /// The launch cards of the Launching stories, and their tiles' status lines.
    fn fixture_launching(&self) -> (Vec<LaunchingItem>, HashMap<String, SharedString>) {
        let row = |label: &str, time: Option<&str>, state| StageRow {
            label: label.into(),
            time: time.map(Into::into),
            state,
        };
        let firefox = Some(Arc::clone(&self.assets.icon_firefox));
        let hint = "The first launch of an app on a server downloads it, which can take several minutes. Later launches are fast.";
        let card =
            |id: &str, name: &str, icon, stages, detail: Option<&str>, status: &str, notice| {
                LaunchingItem {
                    launch_id: id.into(),
                    app_name: name.into(),
                    app_icon: icon,
                    stages,
                    detail: detail.map(Into::into),
                    status: status.into(),
                    notice,
                }
            };
        let (cards, tiles): (Vec<LaunchingItem>, Vec<(&str, &str)>) = match self.story {
            Story::LibraryLaunching => (
                vec![card(
                    "l1",
                    "Firefox",
                    firefox,
                    vec![
                        row("Requesting", Some("2 s"), StageState::Done),
                        row("Starting", Some("14 s"), StageState::Current),
                        row("Connecting", None, StageState::Pending),
                    ],
                    Some("Waiting for the desktop to respond"),
                    "Launching · 0:16",
                    None,
                )],
                vec![("firefox", "Starting…")],
            ),
            Story::LibraryLaunchingPull => (
                vec![
                    card(
                        "l2",
                        "VS Code",
                        Some(Arc::clone(&self.assets.icon_vscode)),
                        vec![
                            row("Requesting", Some("1 s"), StageState::Current),
                            row("Starting", None, StageState::Pending),
                            row("Connecting", None, StageState::Pending),
                        ],
                        Some("Checking limits and choosing a node"),
                        "Launching · 0:01",
                        None,
                    ),
                    card(
                        "l1",
                        "Firefox",
                        firefox,
                        vec![
                            row("Requesting", Some("3 s"), StageState::Done),
                            row("Pulling image", Some("48 s"), StageState::Current),
                            row("Starting", None, StageState::Pending),
                            row("Connecting", None, StageState::Pending),
                        ],
                        Some("Downloading the application image"),
                        "Launching · 0:51",
                        None,
                    ),
                ],
                vec![("firefox", "Pulling image…"), ("vscode", "Requesting…")],
            ),
            Story::LibraryLaunchStalled => (
                vec![card(
                    "l1",
                    "Firefox",
                    firefox,
                    vec![
                        row("Requesting", Some("3 s"), StageState::Done),
                        row("Pulling image", Some("3:04"), StageState::Stalled),
                        row("Starting", None, StageState::Pending),
                        row("Connecting", None, StageState::Pending),
                    ],
                    Some("Downloading the application image"),
                    "Launching · 3:07",
                    Some(LaunchNotice::Stalled {
                        message: "Still pulling the image after 3 min.".into(),
                        hint: Some(hint.into()),
                        can_retry: true,
                    }),
                )],
                vec![("firefox", "Pulling image…")],
            ),
            Story::LibraryLaunchFailed => (
                vec![card(
                    "l1",
                    "Firefox",
                    firefox,
                    vec![
                        row("Requesting", Some("2 s"), StageState::Done),
                        row("Starting", Some("41 s"), StageState::Failed),
                    ],
                    None,
                    "Launch failed",
                    Some(LaunchNotice::Failed {
                        headline: "Couldn’t launch Firefox".into(),
                        body: "No GPU is free on any node.".into(),
                    }),
                )],
                Vec::new(),
            ),
            _ => (Vec::new(), Vec::new()),
        };
        let tiles = tiles
            .into_iter()
            .map(|(id, label)| (id.to_string(), SharedString::from(label)))
            .collect();
        (cards, tiles)
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
            ControlsAction::ToggleSection(section) => {
                self.controls_ui.toggle(section);
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
            ControlsAction::SetClipboardSend(on) => {
                if let Some(clipboard) = self.controls_props.clipboard.as_mut() {
                    clipboard.send = clipboard.send.map(|_| on);
                }
            }
            ControlsAction::SetClipboardReceive(on) => {
                if let Some(clipboard) = self.controls_props.clipboard.as_mut() {
                    clipboard.receive = clipboard.receive.map(|_| on);
                }
            }
            // The event line above shows them; the preview has no clipboard.
            ControlsAction::SendClipboard | ControlsAction::CopySessionClipboard => {}
            ControlsAction::SetClipboardSeamless(on) => {
                if let Some(clipboard) = self.controls_props.clipboard.as_mut() {
                    clipboard.seamless = on;
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
                self.controls_props.screen.dpi_override = dpi;
                if let Some(dpi) = dpi {
                    self.controls_props.screen.dpi = Some(dpi);
                }
            }
            ControlsAction::SetFitMode => {
                self.controls_ui.size_error = false;
                self.controls_props.screen.preset_label = "Keep Remote Size".into();
            }
            ControlsAction::SelectPreset(w, h) => {
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
                            VideoSettingControl::Switch { enabled, .. } => *enabled = value != 0,
                            VideoSettingControl::Select { value: v, .. }
                            | VideoSettingControl::Slider { value: v, .. } => *v = value,
                        }
                    }
                }
            }
            ControlsAction::CopyStats => {
                self.controls_props.stats_copied = !self.controls_props.stats_copied;
            }
            ControlsAction::Reconnect
            | ControlsAction::OpenFiles
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
                clipboard: Some(ClipboardProps {
                    send: Some(true),
                    receive: Some(true),
                    seamless: false,
                    seamless_locked: false,
                }),
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
                            label: "Frames per second".into(),
                            control: VideoSettingControl::Slider {
                                value: 60,
                                min: 15,
                                max: 60,
                                step: 1,
                                unit: SliderUnit::Fps,
                                locked: false,
                            },
                        },
                        VideoSettingRow {
                            id: "jpeg_quality".into(),
                            label: "Base Quality".into(),
                            control: VideoSettingControl::Slider {
                                value: 60,
                                min: 1,
                                max: 100,
                                step: 1,
                                unit: SliderUnit::Plain,
                                locked: false,
                            },
                        },
                        VideoSettingRow {
                            id: "paint_over".into(),
                            label: "Use Paint-Overs".into(),
                            control: VideoSettingControl::Switch {
                                enabled: true,
                                locked: false,
                            },
                        },
                        VideoSettingRow {
                            id: "paint_quality".into(),
                            label: "Paintover Quality".into(),
                            control: VideoSettingControl::Slider {
                                value: 90,
                                min: 1,
                                max: 100,
                                step: 1,
                                unit: SliderUnit::Plain,
                                locked: false,
                            },
                        },
                    ],
                }),
                stats: Some(Self::fixture_stats(false)),
                stats_copied: false,
            },
            Story::ControlsServerLocked => ControlsProps {
                app_name: "Locked Enterprise Desktop".into(),
                fullscreen: true,
                audio: None,
                video: Some(false),
                audio_unavailable: Some("Audio is disabled by server policy.".into()),
                input_switches: base_inputs,
                chords,
                clipboard: Some(ClipboardProps {
                    send: None,
                    receive: Some(true),
                    seamless: false,
                    seamless_locked: true,
                }),
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
                stats: None,
                stats_copied: false,
            },
            _ => ControlsProps {
                app_name: "Visual Studio Code".into(),
                fullscreen: false,
                audio: Some(true),
                video: Some(true),
                audio_unavailable: None,
                input_switches: base_inputs,
                chords,
                clipboard: Some(ClipboardProps {
                    send: Some(true),
                    receive: Some(true),
                    seamless: true,
                    seamless_locked: false,
                }),
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
                            label: "Frames per second".into(),
                            control: VideoSettingControl::Slider {
                                value: 60,
                                min: 15,
                                max: 60,
                                step: 1,
                                unit: SliderUnit::Fps,
                                locked: false,
                            },
                        },
                        VideoSettingRow {
                            id: "rate_control".into(),
                            label: "Encoder Rate Control Mode".into(),
                            control: VideoSettingControl::Select {
                                value: 0,
                                options: vec![
                                    (0, "CBR (Constant Bitrate)".into()),
                                    (1, "CRF (Constant Quality)".into()),
                                ],
                                locked: false,
                            },
                        },
                        VideoSettingRow {
                            id: "bitrate".into(),
                            label: "Video Bitrate".into(),
                            control: VideoSettingControl::Slider {
                                value: 8000,
                                min: 1000,
                                max: 100000,
                                step: 500,
                                unit: SliderUnit::Kbps,
                                locked: false,
                            },
                        },
                    ],
                }),
                stats: Some(Self::fixture_stats(true)),
                stats_copied: false,
            },
        }
    }

    /// Stats as a session shows them a minute after the section opened:
    /// `hardware` is an H.265 stream on a GPU host, else a JPEG stream whose
    /// capture fell back to a readback and whose rate overshoots its target.
    fn fixture_stats(hardware: bool) -> stats::StatsProps {
        use stats::{Graph, Meter, Series, StatRow, StatsProps, Status, Tile};
        let wave = |base: f32, swing: f32, seed: f32| -> Vec<f32> {
            (0..60)
                .map(|i| {
                    let t = i as f32 + seed;
                    (base + swing * (t * 0.37).sin() + swing * 0.5 * (t * 1.3).cos()).max(0.)
                })
                .collect()
        };
        let row = |label: &str, status, value: &str, detail: &str, reason: &str| StatRow {
            label: label.into(),
            status,
            value: value.into(),
            detail: detail.into(),
            reason: reason.into(),
        };
        let tile = |label: &str, value: &str, detail: &str, warn| Tile {
            label: label.into(),
            value: value.into(),
            detail: detail.into(),
            warn,
        };
        let fps = wave(if hardware { 58. } else { 27. }, 2., 0.);
        let encoded = wave(if hardware { 59. } else { 29. }, 1., 3.);
        if hardware {
            StatsProps {
                rows: vec![
                    row(
                        "Encoder",
                        Status::Good,
                        "nvh265enc · H.265 4:2:0",
                        "NVIDIA RTX A2000 · renderD128 · nvidia",
                        "",
                    ),
                    row(
                        "Capture",
                        Status::Good,
                        "Wayland · Zero-copy",
                        "GL on renderD128 · NVIDIA RTX A2000",
                        "",
                    ),
                    row(
                        "Decoder",
                        Status::Good,
                        "Hardware",
                        "VideoToolbox · H.265 · 1966×2160",
                        "",
                    ),
                    row("Connection", Status::Neutral, "WebSockets", "", ""),
                ],
                graphs: vec![
                    Graph {
                        label: "FPS".into(),
                        unit: "fps".into(),
                        max: 66.,
                        series: vec![
                            Series {
                                name: "Client".into(),
                                values: fps,
                            },
                            Series {
                                name: "Server".into(),
                                values: encoded,
                            },
                        ],
                    },
                    Graph {
                        label: "Bandwidth".into(),
                        unit: "Mbps".into(),
                        max: 10.,
                        series: vec![Series {
                            name: String::new(),
                            values: wave(6.4, 1.8, 1.),
                        }],
                    },
                    Graph {
                        label: "Latency".into(),
                        unit: "ms".into(),
                        max: 20.,
                        series: vec![Series {
                            name: String::new(),
                            values: wave(9., 2., 5.),
                        }],
                    },
                ],
                tiles: vec![
                    tile("Encode", "2.1 ms", "", false),
                    tile("Capture to encoded", "4.6 ms", "3.9–6.2", false),
                    tile("Capture to sent", "5.3 ms", "4.4–7.8", false),
                    tile("Decode", "0.8 ms", "", false),
                    tile("Video rate", "6.45 / 8 Mbps", "", false),
                    tile("Peak, 250 ms", "11.2 Mbps", "", false),
                    tile("Received", "48.37 MB", "", false),
                    tile("Lost frames", "0", "", false),
                    tile("Key frame requests", "0", "", false),
                ],
                meters: vec![
                    Meter {
                        label: "CPU".into(),
                        percent: 23.,
                        text: "23%".into(),
                        bar: true,
                    },
                    Meter {
                        label: "GPU".into(),
                        percent: 41.,
                        text: "41%".into(),
                        bar: true,
                    },
                    Meter {
                        label: "MEM".into(),
                        percent: 37.,
                        text: "5.9 GiB / 15.6 GiB · 38%".into(),
                        bar: false,
                    },
                    Meter {
                        label: "VRAM".into(),
                        percent: 19.,
                        text: "1.1 GiB / 6.0 GiB · 19%".into(),
                        bar: false,
                    },
                ],
            }
        } else {
            StatsProps {
                rows: vec![
                    row(
                        "Encoder",
                        Status::Neutral,
                        "jpeg · JPEG · Striped",
                        "No GPU exposed to the server",
                        "",
                    ),
                    row("Capture", Status::Neutral, "X11 · Readback (xshm)", "", ""),
                    row(
                        "Decoder",
                        Status::Neutral,
                        "Software",
                        "JPEG · 1966×2160",
                        "",
                    ),
                    row(
                        "Connection",
                        Status::Warn,
                        "WebSockets",
                        "",
                        "The server is holding frames back because this client is falling behind",
                    ),
                ],
                graphs: vec![
                    Graph {
                        label: "FPS".into(),
                        unit: "fps".into(),
                        max: 33.,
                        series: vec![Series {
                            name: String::new(),
                            values: fps,
                        }],
                    },
                    Graph {
                        label: "Bandwidth".into(),
                        unit: "Mbps".into(),
                        max: 21.,
                        series: vec![Series {
                            name: String::new(),
                            values: wave(14., 4., 2.),
                        }],
                    },
                    Graph {
                        label: "Latency".into(),
                        unit: "ms".into(),
                        max: 48.,
                        series: vec![Series {
                            name: String::new(),
                            values: wave(31., 9., 4.),
                        }],
                    },
                ],
                tiles: vec![
                    tile("Encode", "idle", "", false),
                    tile("Capture to encoded", "11.8 ms", "9.1–17.4", false),
                    tile("Capture to sent", "14.2 ms", "10.3–21", false),
                    tile("Decode", "4.3 ms", "", false),
                    tile("Video rate", "14.2 / 12 Mbps", "", true),
                    tile("Peak, 250 ms", "22.6 Mbps", "", false),
                    tile("Received", "106.4 MB", "", false),
                    tile("Lost frames", "3", "", false),
                    tile("Key frame requests", "–", "", false),
                ],
                meters: vec![
                    Meter {
                        label: "CPU".into(),
                        percent: 78.,
                        text: "78%".into(),
                        bar: true,
                    },
                    Meter {
                        label: "MEM".into(),
                        percent: 52.,
                        text: "8.1 GiB / 15.6 GiB · 52%".into(),
                        bar: false,
                    },
                ],
            }
        }
    }

    fn render_story(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
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
            Story::LibraryPopulated
            | Story::LibraryLaunchDialog
            | Story::LibraryLaunchDialogMinimal
            | Story::LibrarySignOut
            | Story::LibrarySignOutEnv
            | Story::LibrarySignOutDotEnv
            | Story::LibrarySignOutUnsaved
            | Story::LibraryToast
            | Story::LibraryLaunchDialogCopied
            | Story::LibraryLongServer
            | Story::LibraryOrganized
            | Story::LibraryHiddenChip
            | Story::LibrarySearchHidden
            | Story::LibraryContextMenu
            | Story::LibraryLaunching
            | Story::LibraryLaunchingPull
            | Story::LibraryLaunchStalled
            | Story::LibraryLaunchFailed
            | Story::LibraryLaunchDialogRunning
            | Story::LibraryLaunchDialogHidden => {
                let server = if self.story == Story::LibraryLongServer {
                    "analytics-gpu-cluster-07.eu-central.sealskin.example.internal"
                } else {
                    "sealskin.example.internal"
                };
                // The toasts and modals are static here: the 3 s timer never runs.
                let toast = match self.story {
                    Story::LibraryToast => Some("Library refreshed"),
                    Story::LibraryLaunchDialogCopied => Some("Link copied"),
                    _ => None,
                }
                .map(|message| Toast {
                    id: 1,
                    message: message.into(),
                    kind: ToastKind::Success,
                    leaving: false,
                });
                let sign_out = self.story.sign_out();
                let props = self.fixture_library(server, cx);
                let dialog_entity = entity.clone();
                div()
                    .size_full()
                    .relative()
                    .child(library::render(
                        theme,
                        &props,
                        &self.search_input,
                        self.library_handlers(cx),
                    ))
                    .when_some(self.launch_dialog.as_ref(), |view, (dialog, selects)| {
                        let on_action = Rc::new(
                            move |action: LaunchDialogAction, _: &mut Window, cx: &mut App| {
                                dialog_entity.update(cx, |this, cx| {
                                    this.handle_launch_dialog_action(action, cx);
                                });
                            },
                        );
                        view.child(launch_dialog::render_modal(
                            theme, dialog, selects, on_action,
                        ))
                    })
                    .when_some(sign_out, |view, source| {
                        view.child(session::sign_out_confirm_modal(
                            theme,
                            server,
                            source,
                            |_, _, _| {},
                            |_, _, _| {},
                        ))
                    })
                    .when_some(toast, |view, toast| {
                        view.child(components::toast_layer(theme, &toast))
                    })
            }
            Story::LibraryEmpty => {
                let props = LibraryProps {
                    app_name: "SealSkin Native".into(),
                    server: "sealskin.example.internal".into(),
                    apps: Rc::from([]),
                    sessions: Rc::from([]),
                    search_query: self.search_input.read(cx).value().to_string(),
                    selected_filter: self.selected_filter.clone(),
                    status: None,
                    busy: None,
                    ..LibraryProps::default()
                };
                library::render(theme, &props, &self.search_input, self.library_handlers(cx))
            }
            Story::LibraryBusy => {
                let props = LibraryProps {
                    app_name: "SealSkin Native".into(),
                    server: "sealskin.example.internal".into(),
                    apps: vec![AppItem {
                        icon: None,
                        ..self.app_item("zen", "Zen", "browser")
                    }]
                    .into(),
                    sessions: Rc::from([]),
                    search_query: self.search_input.read(cx).value().to_string(),
                    selected_filter: self.selected_filter.clone(),
                    status: Some("Couldn’t save your Library changes.".into()),
                    busy: Some("Refreshing…".into()),
                    ..LibraryProps::default()
                };
                library::render(theme, &props, &self.search_input, self.library_handlers(cx))
            }
            Story::ControlsHardware
            | Story::ControlsJpeg
            | Story::ControlsServerLocked
            | Story::ControlsDropdownOpen
            | Story::ControlsRestoredSections => {
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
                                &self.controls_props,
                                &self.controls_ui,
                                &self.width,
                                &self.height,
                                handler,
                                window,
                                cx,
                            )),
                    )
            }
            Story::FilesBrowsing | Story::FilesEmpty | Story::FilesDownloadOnly => {
                let entity = entity.clone();
                let handler: files_ui::FilesHandler = Rc::new(move |action: FilesAction, _, cx| {
                    entity.update(cx, |this, cx| {
                        match &action {
                            FilesAction::Select { index, .. } => {
                                let mut rows = (*this.files_props.rows).clone();
                                for (i, row) in rows.iter_mut().enumerate() {
                                    row.selected = i == *index;
                                }
                                this.files_props.selected = 1;
                                this.files_props.rows = Rc::new(rows);
                            }
                            FilesAction::Sort(column) => {
                                if this.files_props.sort == *column {
                                    this.files_props.ascending = !this.files_props.ascending;
                                } else {
                                    this.files_props.sort = *column;
                                }
                            }
                            FilesAction::NewFolder => this.files_props.creating_folder = true,
                            FilesAction::CancelNewFolder | FilesAction::ConfirmNewFolder => {
                                this.files_props.creating_folder = false
                            }
                            FilesAction::DismissNotice => this.files_props.notice = None,
                            FilesAction::ToggleTransfers => {
                                this.files_props.transfers_open = !this.files_props.transfers_open
                            }
                            FilesAction::ClearTransfers => this
                                .files_props
                                .transfers
                                .retain(|t| matches!(t.status, TransferStatus::Running { .. })),
                            _ => {}
                        }
                        if !matches!(action, FilesAction::DragOut(_) | FilesAction::Press(_)) {
                            this.record_event(format!("Files: {action:?}"), cx);
                        }
                        cx.notify();
                    });
                });
                div().size_full().p_6().child(
                    div()
                        .size_full()
                        .max_w(px(820.))
                        .max_h(px(560.))
                        .rounded_xl()
                        .overflow_hidden()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.background)
                        .text_color(theme.foreground)
                        .text_sm()
                        .child(files_ui::render(
                            theme,
                            &self.files_props,
                            &self.folder_name,
                            &self.files_scroll,
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
        let on_sign_out_entity = cx.entity().clone();
        let on_launch_entity = cx.entity().clone();
        let on_resume_entity = cx.entity().clone();
        let on_stop_entity = cx.entity().clone();
        let on_filter_entity = cx.entity().clone();
        let on_quick_entity = cx.entity().clone();
        let on_favorite_entity = cx.entity().clone();
        let on_hidden_entity = cx.entity().clone();
        let on_flight_entity = cx.entity().clone();
        LibraryHandlers {
            on_quick_launch: Rc::new(move |id, _, cx| {
                on_quick_entity.update(cx, |this, cx| {
                    this.record_event(format!("Quick Launch: {id}"), cx);
                });
            }),
            on_toggle_favorite: Rc::new(move |id, _, cx| {
                on_favorite_entity.update(cx, |this, cx| {
                    this.record_event(format!("Toggle Favorite: {id}"), cx);
                });
            }),
            on_toggle_hidden: Rc::new(move |id, _, cx| {
                on_hidden_entity.update(cx, |this, cx| {
                    this.record_event(format!("Toggle Hidden: {id}"), cx);
                });
            }),
            on_flight_action: Rc::new(move |id, action, _, cx| {
                on_flight_entity.update(cx, |this, cx| {
                    this.record_event(format!("Flight: {action:?} ({id})"), cx);
                });
            }),
            on_refresh: Box::new(move |_, _, cx| {
                on_refresh_entity.update(cx, |this, cx| {
                    this.record_event("Library: Refresh", cx);
                });
            }),
            on_sign_out: Box::new(move |_, _, cx| {
                on_sign_out_entity.update(cx, |this, cx| {
                    this.record_event("Library: Sign Out…", cx);
                });
            }),
            on_launch: Rc::new(move |id, window, cx| {
                on_launch_entity.update(cx, |this, cx| {
                    let (name, icon) = match id.as_str() {
                        "lightcraft" => {
                            ("LightCraft", Some(Arc::clone(&this.assets.icon_lightcraft)))
                        }
                        "weixin" => ("Weixin", Some(Arc::clone(&this.assets.icon_weixin))),
                        "zen" => ("Zen", Some(Arc::clone(&this.assets.icon_zen))),
                        "vscode" => ("VS Code", Some(Arc::clone(&this.assets.icon_vscode))),
                        "firefox" => ("Firefox", Some(Arc::clone(&this.assets.icon_firefox))),
                        "krita" => ("Krita", None),
                        "obsidian" => ("Obsidian", None),
                        other => (other, None),
                    };
                    let dialog = Self::fixture_launch_dialog(&id, name, icon, false);
                    let selects = Self::bind_launch_selects(&dialog, window, cx);
                    this.launch_dialog = Some((dialog, selects));
                    this.record_event(format!("Library: Open Launch Dialog for `{name}`"), cx);
                });
            }),
            on_resume: Rc::new(move |id, _, cx| {
                on_resume_entity.update(cx, |this, cx| {
                    this.record_event(format!("Library: Open session `{id}`"), cx);
                });
            }),
            on_stop: Some(Rc::new(move |id, _, cx| {
                on_stop_entity.update(cx, |this, cx| {
                    this.record_event(format!("Library: Stop session `{id}`"), cx);
                });
            })),
            on_select_filter: Rc::new(move |filter, _, cx| {
                on_filter_entity.update(cx, |this, cx| {
                    this.record_event(format!("Library: Filter `{filter:?}`"), cx);
                    this.selected_filter = filter;
                });
            }),
        }
    }
}

impl Render for PreviewApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        let canvas_widths = CANVAS_WIDTHS.map(|width| {
            let label: SharedString = width.map_or("Fit".into(), |w| format!("{w:.0}").into());
            let mut button = Button::new(SharedString::from(format!("canvas-{label}")))
                .xsmall()
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.canvas_width = width;
                    cx.notify();
                }));
            if self.canvas_width == width {
                button = button.primary();
            } else {
                button = button.outline();
            }
            button
        });

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
                    .children(canvas_widths)
                    .child(div().w(px(8.)))
                    .child(theme_button("theme-system", "System", Appearance::System))
                    .child(theme_button("theme-light", "Light", Appearance::Light))
                    .child(theme_button("theme-dark", "Dark", Appearance::Dark)),
            );
        let story = self.render_story(&theme, window, cx);
        // A fixed width scrolls sideways inside the stage.
        let canvas = match self.canvas_width {
            Some(width) => div()
                .id("preview-canvas")
                .size_full()
                .overflow_x_scroll()
                .child(div().w(px(width)).h_full().flex_none().child(story))
                .into_any_element(),
            None => story.size_full().into_any_element(),
        };

        let stage = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .v_flex()
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(canvas));

        div()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .flex()
            .child(gallery)
            .child(stage)
    }
}

/// A slice of the server's locale list, labelled as macOS names each locale.
fn fixture_languages() -> Vec<LaunchOption> {
    let mut options = vec![LaunchOption {
        id: "en_US.UTF-8".into(),
        label: "Default (en_US)".into(),
    }];
    for (id, name) in [
        ("ar_EG", "Arabic (Egypt)"),
        ("cs_CZ", "Czech (Czechia)"),
        ("da_DK", "Danish (Denmark)"),
        ("de_AT", "German (Austria)"),
        ("de_CH", "German (Switzerland)"),
        ("de_DE", "German (Germany)"),
        ("el_GR", "Greek (Greece)"),
        ("en_AU", "English (Australia)"),
        ("en_GB", "English (United Kingdom)"),
        ("es_ES", "Spanish (Spain)"),
        ("es_MX", "Spanish (Mexico)"),
        ("fi_FI", "Finnish (Finland)"),
        ("fr_CA", "French (Canada)"),
        ("fr_FR", "French (France)"),
        ("he_IL", "Hebrew (Israel)"),
        ("hi_IN", "Hindi (India)"),
        ("it_IT", "Italian (Italy)"),
        ("ja_JP", "Japanese (Japan)"),
        ("ko_KR", "Korean (South Korea)"),
        ("nl_NL", "Dutch (Netherlands)"),
        ("pl_PL", "Polish (Poland)"),
        ("pt_BR", "Portuguese (Brazil)"),
        ("ru_RU", "Russian (Russia)"),
        ("sv_SE", "Swedish (Sweden)"),
        ("tr_TR", "Turkish (Türkiye)"),
        ("uk_UA", "Ukrainian (Ukraine)"),
        ("zh_CN", "Chinese (China mainland)"),
        ("zh_TW", "Chinese (Taiwan)"),
    ] {
        options.push(LaunchOption {
            id: format!("{id}.UTF-8"),
            label: format!("{name} · {id}"),
        });
    }
    options
}

fn main() {
    gpui_kit::application()
        .with_assets(skerry_ui::assets::SkerryAssets)
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

mod fixtures {
    use gpui_kit::RenderImage;
    use image::{Frame, Rgba, RgbaImage};
    use smallvec::smallvec;
    use std::sync::Arc;

    fn to_render_image(mut rgba: RgbaImage) -> Arc<RenderImage> {
        for pixel in rgba.pixels_mut() {
            pixel.0.swap(0, 2); // RGBA -> BGRA for GPUI RenderImage
        }
        Arc::new(RenderImage::new(smallvec![Frame::new(rgba)]))
    }

    fn blend(dst: &mut Rgba<u8>, src: [u8; 4]) {
        let a = f32::from(src[3]) / 255.0;
        let inv = 1.0 - a;
        dst[0] = (f32::from(src[0]) * a + f32::from(dst[0]) * inv) as u8;
        dst[1] = (f32::from(src[1]) * a + f32::from(dst[1]) * inv) as u8;
        dst[2] = (f32::from(src[2]) * a + f32::from(dst[2]) * inv) as u8;
        dst[3] = ((a + (f32::from(dst[3]) / 255.0) * inv).min(1.0) * 255.0) as u8;
    }

    fn fill_rounded_rect(
        img: &mut RgbaImage,
        x0: f32,
        y0: f32,
        w: f32,
        h: f32,
        r: f32,
        color: [u8; 4],
    ) {
        let min_x = (x0.floor() as i32).max(0) as u32;
        let min_y = (y0.floor() as i32).max(0) as u32;
        let max_x = ((x0 + w).ceil() as u32).min(img.width());
        let max_y = ((y0 + h).ceil() as u32).min(img.height());
        let cx = x0 + w * 0.5;
        let cy = y0 + h * 0.5;
        let hx = (w * 0.5 - r).max(0.0);
        let hy = (h * 0.5 - r).max(0.0);
        for y in min_y..max_y {
            for x in min_x..max_x {
                let dx = (x as f32 + 0.5 - cx).abs() - hx;
                let dy = (y as f32 + 0.5 - cy).abs() - hy;
                let dist = (dx.max(0.0).hypot(dy.max(0.0))) + dx.max(dy).min(0.0) - r;
                let cov = (0.5 - dist).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let mut c = color;
                    c[3] = (f32::from(c[3]) * cov) as u8;
                    blend(img.get_pixel_mut(x, y), c);
                }
            }
        }
    }

    fn fill_circle(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, color: [u8; 4]) {
        for y in 0..img.height() {
            for x in 0..img.width() {
                let d = (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy) - r;
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let mut c = color;
                    c[3] = (f32::from(c[3]) * cov) as u8;
                    blend(img.get_pixel_mut(x, y), c);
                }
            }
        }
    }

    fn stroke_ring(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, thickness: f32, color: [u8; 4]) {
        let half = thickness * 0.5;
        for y in 0..img.height() {
            for x in 0..img.width() {
                let d = ((x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy) - r).abs() - half;
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let mut c = color;
                    c[3] = (f32::from(c[3]) * cov) as u8;
                    blend(img.get_pixel_mut(x, y), c);
                }
            }
        }
    }

    fn draw_segment(
        img: &mut RgbaImage,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        thickness: f32,
        color: [u8; 4],
    ) {
        let half = thickness * 0.5;
        let vx = x1 - x0;
        let vy = y1 - y0;
        let len2 = (vx * vx + vy * vy).max(1e-4);
        for y in 0..img.height() {
            for x in 0..img.width() {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let t = (((px - x0) * vx + (py - y0) * vy) / len2).clamp(0.0, 1.0);
                let dx = px - (x0 + t * vx);
                let dy = py - (y0 + t * vy);
                let d = dx.hypot(dy) - half;
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let mut c = color;
                    c[3] = (f32::from(c[3]) * cov) as u8;
                    blend(img.get_pixel_mut(x, y), c);
                }
            }
        }
    }

    pub fn zen_icon() -> Arc<RenderImage> {
        let mut img = RgbaImage::from_pixel(128, 128, Rgba([0, 0, 0, 0]));
        let coral = [236, 108, 72, 255];
        stroke_ring(&mut img, 64.0, 64.0, 50.0, 12.0, coral);
        stroke_ring(&mut img, 64.0, 64.0, 32.0, 9.0, coral);
        stroke_ring(&mut img, 64.0, 64.0, 16.0, 6.0, coral);
        to_render_image(img)
    }

    pub fn lightcraft_icon() -> Arc<RenderImage> {
        let mut img = RgbaImage::from_pixel(128, 128, Rgba([0, 0, 0, 0]));
        let blue = [92, 114, 248, 255];
        draw_segment(&mut img, 64.0, 20.0, 22.0, 106.0, 18.0, blue);
        draw_segment(&mut img, 64.0, 20.0, 104.0, 106.0, 18.0, blue);
        draw_segment(&mut img, 40.0, 76.0, 88.0, 76.0, 14.0, blue);
        to_render_image(img)
    }

    pub fn weixin_icon() -> Arc<RenderImage> {
        let mut img = RgbaImage::from_pixel(128, 128, Rgba([0, 0, 0, 0]));
        fill_rounded_rect(&mut img, 8.0, 8.0, 112.0, 112.0, 26.0, [52, 199, 73, 255]);
        fill_circle(&mut img, 50.0, 56.0, 26.0, [255, 255, 255, 255]);
        fill_circle(&mut img, 80.0, 74.0, 21.0, [242, 252, 244, 255]);
        fill_circle(&mut img, 42.0, 52.0, 3.5, [52, 199, 73, 255]);
        fill_circle(&mut img, 58.0, 52.0, 3.5, [52, 199, 73, 255]);
        fill_circle(&mut img, 73.0, 71.0, 3.0, [52, 199, 73, 255]);
        fill_circle(&mut img, 87.0, 71.0, 3.0, [52, 199, 73, 255]);
        to_render_image(img)
    }

    pub fn vscode_icon() -> Arc<RenderImage> {
        let mut img = RgbaImage::from_pixel(128, 128, Rgba([0, 0, 0, 0]));
        let light = [56, 178, 248, 255];
        let mid = [0, 122, 204, 255];
        draw_segment(&mut img, 96.0, 22.0, 96.0, 106.0, 20.0, light);
        draw_segment(&mut img, 96.0, 22.0, 26.0, 88.0, 15.0, mid);
        draw_segment(&mut img, 96.0, 106.0, 26.0, 40.0, 15.0, light);
        to_render_image(img)
    }

    pub fn firefox_icon() -> Arc<RenderImage> {
        let mut img = RgbaImage::from_pixel(128, 128, Rgba([0, 0, 0, 0]));
        fill_circle(&mut img, 64.0, 64.0, 52.0, [118, 66, 214, 255]);
        stroke_ring(&mut img, 66.0, 66.0, 38.0, 20.0, [249, 132, 44, 255]);
        fill_circle(&mut img, 74.0, 52.0, 18.0, [255, 204, 72, 255]);
        to_render_image(img)
    }

    pub fn zen_thumbnail() -> Arc<RenderImage> {
        let (w, h) = (640, 360);
        let mut img = RgbaImage::from_pixel(w, h, Rgba([226, 228, 232, 255]));
        // Subtle sidebar on the left
        fill_rounded_rect(
            &mut img,
            0.0,
            0.0,
            156.0,
            h as f32,
            0.0,
            [215, 218, 224, 255],
        );
        // Sidebar divider
        fill_rounded_rect(
            &mut img,
            156.0,
            0.0,
            2.0,
            h as f32,
            0.0,
            [196, 200, 208, 255],
        );
        // Floating white address bar
        fill_rounded_rect(
            &mut img,
            34.0,
            34.0,
            572.0,
            52.0,
            12.0,
            [255, 255, 255, 255],
        );
        // Search icon dot + placeholder bar inside address bar
        stroke_ring(&mut img, 64.0, 60.0, 8.0, 2.5, [156, 163, 175, 255]);
        fill_rounded_rect(&mut img, 84.0, 54.0, 190.0, 11.0, 5.0, [205, 210, 218, 255]);
        to_render_image(img)
    }

    pub fn lightcraft_thumbnail() -> Arc<RenderImage> {
        let (w, h) = (640, 360);
        let mut img = RgbaImage::from_pixel(w, h, Rgba([24, 25, 28, 255]));
        // Left catalog sidebar
        fill_rounded_rect(&mut img, 0.0, 0.0, 128.0, h as f32, 0.0, [32, 33, 38, 255]);
        for i in 0..8 {
            let y = 26.0 + i as f32 * 26.0;
            fill_rounded_rect(&mut img, 16.0, y, 86.0, 9.0, 4.0, [58, 61, 70, 255]);
        }
        // Right tool rail
        fill_rounded_rect(
            &mut img,
            (w - 44) as f32,
            0.0,
            44.0,
            h as f32,
            0.0,
            [32, 33, 38, 255],
        );
        for i in 0..6 {
            fill_circle(
                &mut img,
                (w - 22) as f32,
                36.0 + i as f32 * 38.0,
                7.0,
                [88, 92, 104, 255],
            );
        }
        // Centered vertical photo preview (warm sky + silhouette towers)
        let (px0, py0, pw, ph) = (210.0, 14.0, 224.0, 332.0);
        for y in (py0 as u32)..((py0 + ph) as u32) {
            let t = (y as f32 - py0) / ph;
            let r = (232.0 - t * 90.0) as u8;
            let g = (224.0 - t * 110.0) as u8;
            let b = (214.0 - t * 125.0) as u8;
            for x in (px0 as u32)..((px0 + pw) as u32) {
                *img.get_pixel_mut(x, y) = Rgba([r, g, b, 255]);
            }
        }
        // Architectural spires silhouette inside the photo
        for (cx, top, base_w) in [
            (256.0, 74.0, 38.0),
            (296.0, 46.0, 44.0),
            (340.0, 38.0, 46.0),
            (384.0, 68.0, 38.0),
        ] {
            for y in (top as u32)..((py0 + ph) as u32) {
                let progress = (y as f32 - top) / (py0 + ph - top);
                let half_w = (base_w * 0.5) * (0.18 + 0.82 * progress);
                let x_start = ((cx - half_w) as u32).max(px0 as u32);
                let x_end = ((cx + half_w) as u32).min((px0 + pw) as u32);
                for x in x_start..x_end {
                    let shade = if (x + y) % 5 == 0 { 82 } else { 62 };
                    *img.get_pixel_mut(x, y) = Rgba([shade, shade - 6, shade - 14, 255]);
                }
            }
        }
        to_render_image(img)
    }
}
