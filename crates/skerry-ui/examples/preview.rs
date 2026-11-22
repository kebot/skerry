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
    account::{self, AccessChip, AccountProps, Limits, StorageMeasure, StorageUse},
    apps::{
        self as apps_ui, ActivityRow, AppTile, AppsAction, AppsProps, Badge, Banner, ButtonKind,
        CatalogView, CategoryRow, ConsoleProps, FilterId, Inspector, InspectorButtons, JobBadge,
    },
    components::{self, Toast, ToastKind},
    connect::{
        self, ConnectHandler, ConnectHandlers, ConnectProps, EnvNote, KeyProblem, ProblemKind,
        RecoveryKind, RecoveryProps,
    },
    controls::{
        self, AudioBitrateProps, AudioOutputItem, AudioProps, CaptureProps, ChordItem,
        ClipboardProps, CodecChoice, ControlsAction, ControlsProps, ControlsUiState, GamepadsProps,
        PadRow, PadStatus, PrintJobRow, PrintRowKind, PrintingProps, ScreenProps,
        SecondScreenProps, ShortcutRow, ShortcutsProps, SliderUnit, SwitchItem, VideoProps,
        VideoSettingControl, VideoSettingRow,
    },
    files::{
        self as files_ui, Crumb, Direction, FileRow, FilesAction, FilesProps, Notice, SortColumn,
        TransferRow, TransferStatus,
    },
    launch_dialog::{
        self, LaunchDialogAction, LaunchDialogProps, LaunchDialogSelects, LaunchOption,
        LaunchOptions, LaunchSelect, OpeningLabel, RunningMatch,
    },
    library::{
        self, AppItem, DropPreview, Filter, LaunchNotice, LaunchingItem, LibraryHandlers,
        LibraryProps, OmnibarIcon, OpenHandlers, OpeningAppRow, OpeningKind, OpeningProps,
        OpeningSessionRow, SessionItem, StageRow, StageState, UploadProgress,
    },
    session::{self, KeySourceLabel, PrintJobAction, PrintNoticeKind, PrintNoticeProps},
    stats,
};
use std::collections::{HashMap, HashSet};
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
    ConnectDormant,
    ConnectDormantWaiting,
    ConnectDormantTimedOut,
    ConnectHeld,
    ConnectKeyRevoked,
    ConnectKeyExpiredEnv,
    ConnectKeyDisabled,
    ConnectKeyClient,
    ConnectDifferentKey,
    LibraryPopulated,
    LibraryLaunchDialog,
    LibraryLaunchDialogMinimal,
    LibraryEmpty,
    LibraryBusy,
    LibraryOpenLink,
    LibraryOpenLinkPending,
    LibraryOpenLinkNotice,
    LibraryOpenLinkNone,
    LibraryOpenLinkRejected,
    LibraryOpenFile,
    LibraryOpenFileFallback,
    LibraryDropTarget,
    LibraryDropOnSession,
    LibraryDropFolders,
    LibraryUploading,
    LibraryUploadFailed,
    LaunchDialogOpenFile,
    LaunchDialogOpenLink,
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
    LibraryAccountUser,
    LibraryAccountMeasuring,
    LibraryAccountLimits,
    LibraryAccountAdmin,
    LibraryAccountFallback,
    ControlsHardware,
    ControlsJpeg,
    ControlsServerLocked,
    ControlsDropdownOpen,
    ControlsRestoredSections,
    ControlsShortcutsActive,
    ControlsShortcutsOff,
    ControlsShortcutsLocked,
    ControlsStatsStripOn,
    ControlsStatsWaiting,
    ControlsInputCapture,
    ControlsInputCaptureOn,
    ControlsInputCaptureUnavailable,
    GamepadsEmpty,
    GamepadsSending,
    GamepadsReady,
    GamepadsFull,
    GamepadsInputOff,
    GamepadsServerOff,
    ControlsPrintingEmpty,
    ControlsPrintingJobs,
    ControlsPrintingOff,
    ControlsApps,
    AudioDefault,
    AudioChosen,
    AudioMissing,
    AudioNoDevice,
    AudioRetrying,
    AudioBitrateLocked,
    AudioBitrateHidden,
    AudioOff,
    SessionOverlays,
    SessionStartupStalled,
    SessionKeepWaiting,
    SessionStatsStrip,
    SessionStatsStripWarn,
    SessionStatsStripNarrow,
    SessionStatsStripLight,
    SessionStatsStripWithBanner,
    SessionStatsStripFullScreen,
    SessionCaptureHint,
    SessionReleaseHint,
    SessionCaptureHintWithStrip,
    SessionCaptureHintWithBanner,
    SessionCaptureHintNarrow,
    SessionCaptureHintLight,
    ControlsSecondScreen,
    ControlsSecondScreenOpen,
    SessionPlacementAllSides,
    SessionPlacementTwoSides,
    SecondDisplayWaiting,
    SecondDisplayRefused,
    SessionSupersededPill,
    FilesBrowsing,
    FilesEmpty,
    FilesDownloadOnly,
    PrintNoticeStates,
    PrintNoticeLongName,
    PrintNoticeStack,
    PrintNoticeWithStrip,
    PrintNoticeNarrow,
    AppsLoading,
    AppsBrowsing,
    AppsInstalled,
    AppsInstalling,
    AppsFailed,
    AppsSearchEmpty,
    AppsCatalogError,
    AppsOffline,
}

impl Story {
    const ALL: &'static [Story] = &[
        Story::ConnectDefault,
        Story::ConnectBusy,
        Story::ConnectError,
        Story::ConnectDormant,
        Story::ConnectDormantWaiting,
        Story::ConnectDormantTimedOut,
        Story::ConnectHeld,
        Story::ConnectKeyRevoked,
        Story::ConnectKeyExpiredEnv,
        Story::ConnectKeyDisabled,
        Story::ConnectKeyClient,
        Story::ConnectDifferentKey,
        Story::LibraryPopulated,
        Story::LibraryLaunchDialog,
        Story::LibraryLaunchDialogMinimal,
        Story::LibraryEmpty,
        Story::LibraryBusy,
        Story::LibraryOpenLink,
        Story::LibraryOpenLinkPending,
        Story::LibraryOpenLinkNotice,
        Story::LibraryOpenLinkNone,
        Story::LibraryOpenLinkRejected,
        Story::LibraryOpenFile,
        Story::LibraryOpenFileFallback,
        Story::LibraryDropTarget,
        Story::LibraryDropOnSession,
        Story::LibraryDropFolders,
        Story::LibraryUploading,
        Story::LibraryUploadFailed,
        Story::LaunchDialogOpenFile,
        Story::LaunchDialogOpenLink,
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
        Story::LibraryAccountUser,
        Story::LibraryAccountMeasuring,
        Story::LibraryAccountLimits,
        Story::LibraryAccountAdmin,
        Story::LibraryAccountFallback,
        Story::ControlsHardware,
        Story::ControlsJpeg,
        Story::ControlsServerLocked,
        Story::ControlsDropdownOpen,
        Story::ControlsRestoredSections,
        Story::ControlsShortcutsActive,
        Story::ControlsShortcutsOff,
        Story::ControlsShortcutsLocked,
        Story::ControlsStatsStripOn,
        Story::ControlsStatsWaiting,
        Story::ControlsInputCapture,
        Story::ControlsInputCaptureOn,
        Story::ControlsInputCaptureUnavailable,
        Story::GamepadsEmpty,
        Story::GamepadsSending,
        Story::GamepadsReady,
        Story::GamepadsFull,
        Story::GamepadsInputOff,
        Story::GamepadsServerOff,
        Story::ControlsPrintingEmpty,
        Story::ControlsPrintingJobs,
        Story::ControlsPrintingOff,
        Story::ControlsApps,
        Story::AudioDefault,
        Story::AudioChosen,
        Story::AudioMissing,
        Story::AudioNoDevice,
        Story::AudioRetrying,
        Story::AudioBitrateLocked,
        Story::AudioBitrateHidden,
        Story::AudioOff,
        Story::SessionOverlays,
        Story::SessionStartupStalled,
        Story::SessionKeepWaiting,
        Story::SessionStatsStrip,
        Story::SessionStatsStripWarn,
        Story::SessionStatsStripNarrow,
        Story::SessionStatsStripLight,
        Story::SessionStatsStripWithBanner,
        Story::SessionStatsStripFullScreen,
        Story::SessionCaptureHint,
        Story::SessionReleaseHint,
        Story::SessionCaptureHintWithStrip,
        Story::SessionCaptureHintWithBanner,
        Story::SessionCaptureHintNarrow,
        Story::SessionCaptureHintLight,
        Story::ControlsSecondScreen,
        Story::ControlsSecondScreenOpen,
        Story::SessionPlacementAllSides,
        Story::SessionPlacementTwoSides,
        Story::SecondDisplayWaiting,
        Story::SecondDisplayRefused,
        Story::SessionSupersededPill,
        Story::FilesBrowsing,
        Story::FilesEmpty,
        Story::FilesDownloadOnly,
        Story::PrintNoticeStates,
        Story::PrintNoticeLongName,
        Story::PrintNoticeStack,
        Story::PrintNoticeWithStrip,
        Story::PrintNoticeNarrow,
        Story::AppsLoading,
        Story::AppsBrowsing,
        Story::AppsInstalled,
        Story::AppsInstalling,
        Story::AppsFailed,
        Story::AppsSearchEmpty,
        Story::AppsCatalogError,
        Story::AppsOffline,
    ];

    fn group(self) -> &'static str {
        match self {
            Story::ConnectDefault
            | Story::ConnectBusy
            | Story::ConnectError
            | Story::ConnectDormant
            | Story::ConnectDormantWaiting
            | Story::ConnectDormantTimedOut
            | Story::ConnectHeld
            | Story::ConnectKeyRevoked
            | Story::ConnectKeyExpiredEnv
            | Story::ConnectKeyDisabled
            | Story::ConnectKeyClient
            | Story::ConnectDifferentKey => "Connect",
            Story::LibraryPopulated
            | Story::LibraryLaunchDialog
            | Story::LibraryLaunchDialogMinimal
            | Story::LibraryEmpty
            | Story::LibraryBusy
            | Story::LibraryOpenLink
            | Story::LibraryOpenLinkPending
            | Story::LibraryOpenLinkNotice
            | Story::LibraryOpenLinkNone
            | Story::LibraryOpenLinkRejected
            | Story::LibraryOpenFile
            | Story::LibraryOpenFileFallback
            | Story::LibraryDropTarget
            | Story::LibraryDropOnSession
            | Story::LibraryDropFolders
            | Story::LibraryUploading
            | Story::LibraryUploadFailed
            | Story::LaunchDialogOpenFile
            | Story::LaunchDialogOpenLink
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
            | Story::LibraryLaunchDialogHidden
            | Story::LibraryAccountUser
            | Story::LibraryAccountMeasuring
            | Story::LibraryAccountLimits
            | Story::LibraryAccountAdmin
            | Story::LibraryAccountFallback => "Library",
            Story::ControlsHardware
            | Story::ControlsJpeg
            | Story::ControlsServerLocked
            | Story::ControlsDropdownOpen
            | Story::ControlsRestoredSections
            | Story::ControlsShortcutsActive
            | Story::ControlsShortcutsOff
            | Story::ControlsShortcutsLocked
            | Story::ControlsStatsStripOn
            | Story::ControlsStatsWaiting
            | Story::ControlsSecondScreen
            | Story::ControlsSecondScreenOpen
            | Story::ControlsInputCapture
            | Story::ControlsInputCaptureOn
            | Story::ControlsInputCaptureUnavailable
            | Story::GamepadsEmpty
            | Story::GamepadsSending
            | Story::GamepadsReady
            | Story::GamepadsFull
            | Story::GamepadsInputOff
            | Story::GamepadsServerOff
            | Story::ControlsPrintingEmpty
            | Story::ControlsPrintingJobs
            | Story::ControlsPrintingOff
            | Story::ControlsApps => "Session Controls",
            Story::AudioDefault
            | Story::AudioChosen
            | Story::AudioMissing
            | Story::AudioNoDevice
            | Story::AudioRetrying
            | Story::AudioBitrateLocked
            | Story::AudioBitrateHidden
            | Story::AudioOff => "Session Audio",
            Story::SessionOverlays
            | Story::SessionStartupStalled
            | Story::SessionKeepWaiting
            | Story::SessionStatsStrip
            | Story::SessionStatsStripWarn
            | Story::SessionStatsStripNarrow
            | Story::SessionStatsStripLight
            | Story::SessionStatsStripWithBanner
            | Story::SessionStatsStripFullScreen
            | Story::SessionCaptureHint
            | Story::SessionReleaseHint
            | Story::SessionCaptureHintWithStrip
            | Story::SessionCaptureHintWithBanner
            | Story::SessionCaptureHintNarrow
            | Story::SessionCaptureHintLight
            | Story::SessionPlacementAllSides
            | Story::SessionPlacementTwoSides
            | Story::SecondDisplayWaiting
            | Story::SecondDisplayRefused
            | Story::SessionSupersededPill => "Session Overlays",
            Story::FilesBrowsing | Story::FilesEmpty | Story::FilesDownloadOnly => "Session Files",
            Story::PrintNoticeStates
            | Story::PrintNoticeLongName
            | Story::PrintNoticeStack
            | Story::PrintNoticeWithStrip
            | Story::PrintNoticeNarrow => "Session Printing",
            Story::AppsLoading
            | Story::AppsBrowsing
            | Story::AppsInstalled
            | Story::AppsInstalling
            | Story::AppsFailed
            | Story::AppsSearchEmpty
            | Story::AppsCatalogError
            | Story::AppsOffline => "Session Apps",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Story::ConnectDefault => "Default",
            Story::ConnectBusy => "Connecting",
            Story::ConnectError => "Error",
            Story::ConnectDormant => "Dormant Key",
            Story::ConnectDormantWaiting => "Dormant · Waiting",
            Story::ConnectDormantTimedOut => "Dormant · Still Asleep",
            Story::ConnectHeld => "Waiting for Admin",
            Story::ConnectKeyRevoked => "Key Revoked",
            Story::ConnectKeyExpiredEnv => "Key Expired + Env Note",
            Story::ConnectKeyDisabled => "Keys Turned Off",
            Story::ConnectKeyClient => "App Not Allowed",
            Story::ConnectDifferentKey => "Use a Different Key",
            Story::LibraryPopulated => "Populated",
            Story::LibraryLaunchDialog => "Launch Dialog",
            Story::LibraryLaunchDialogMinimal => "Launch Dialog · Minimal",
            Story::LibraryEmpty => "Empty",
            Story::LibraryBusy => "Busy + Status",
            Story::LibraryOpenLink => "Open Link",
            Story::LibraryOpenLinkPending => "Open Link · Opening",
            Story::LibraryOpenLinkNotice => "Open Link · Not Taken",
            Story::LibraryOpenLinkNone => "Open Link · No Apps",
            Story::LibraryOpenLinkRejected => "Open Link · Rejected",
            Story::LibraryOpenFile => "Open File",
            Story::LibraryOpenFileFallback => "Open File · Unknown Type",
            Story::LibraryDropTarget => "Drop Target",
            Story::LibraryDropOnSession => "Drop on Session",
            Story::LibraryDropFolders => "Drop · Folders Only",
            Story::LibraryUploading => "Uploading + Files",
            Story::LibraryUploadFailed => "Upload Failed",
            Story::LaunchDialogOpenFile => "Launch Dialog · Open File",
            Story::LaunchDialogOpenLink => "Launch Dialog · Open Link",
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
            Story::LibraryAccountUser => "Account Menu · User",
            Story::LibraryAccountMeasuring => "Account Menu · Measuring",
            Story::LibraryAccountLimits => "Account Menu · Near Limits",
            Story::LibraryAccountAdmin => "Account Menu · Admin",
            Story::LibraryAccountFallback => "Account Menu · Unavailable",
            Story::ControlsHardware => "Hardware H.265",
            Story::ControlsJpeg => "JPEG + Paint-Over",
            Story::ControlsServerLocked => "Server Locked",
            Story::ControlsDropdownOpen => "Dropdown Open",
            Story::ControlsRestoredSections => "Restored Sections",
            Story::ControlsShortcutsActive => "Shortcuts · Mac",
            Story::ControlsShortcutsOff => "Shortcuts · To Session",
            Story::ControlsShortcutsLocked => "Shortcuts · Locked",
            Story::ControlsStatsStripOn => "Stats · Strip On",
            Story::ControlsStatsWaiting => "Stats · Waiting",
            Story::ControlsInputCapture => "Input · Capture Pointer",
            Story::ControlsInputCaptureOn => "Input · Pointer Captured",
            Story::ControlsInputCaptureUnavailable => "Input · Capture Unavailable",
            Story::GamepadsEmpty => "Gamepads · Empty",
            Story::GamepadsSending => "Gamepads · Sending",
            Story::GamepadsReady => "Gamepads · Ready",
            Story::GamepadsFull => "Gamepads · Full",
            Story::GamepadsInputOff => "Gamepads · Input Off",
            Story::GamepadsServerOff => "Gamepads · Server Off",
            Story::AudioDefault => "Default",
            Story::AudioChosen => "Chosen Device",
            Story::AudioMissing => "Saved Device Missing",
            Story::AudioNoDevice => "No Output Device",
            Story::AudioRetrying => "Output Stopped · Retrying",
            Story::AudioBitrateLocked => "Bitrate Locked",
            Story::AudioBitrateHidden => "Bitrate Hidden + Long Names",
            Story::AudioOff => "Turned Off by Server",
            Story::SessionOverlays => "Waiting + IME + Banner",
            Story::SessionStartupStalled => "Still Starting",
            Story::SessionKeepWaiting => "Waiting to Start",
            Story::SessionStatsStrip => "Stats Strip",
            Story::SessionStatsStripWarn => "Stats Strip · Warn",
            Story::SessionStatsStripNarrow => "Stats Strip · Narrow",
            Story::SessionStatsStripLight => "Stats Strip · Light",
            Story::SessionStatsStripWithBanner => "Stats Strip + Banner",
            Story::SessionStatsStripFullScreen => "Stats Strip · Full Screen",
            Story::SessionCaptureHint => "Capture Hint",
            Story::SessionReleaseHint => "Release Hint",
            Story::SessionCaptureHintWithStrip => "Capture Hint + Strip",
            Story::SessionCaptureHintWithBanner => "Capture Hint + Banner",
            Story::SessionCaptureHintNarrow => "Capture Hint · Narrow",
            Story::SessionCaptureHintLight => "Capture Hint · Light",
            Story::ControlsSecondScreen => "Screen · Add Screen",
            Story::ControlsSecondScreenOpen => "Screen · Screen 2 Open",
            Story::SessionPlacementAllSides => "Display 2 Picker · All Sides",
            Story::SessionPlacementTwoSides => "Display 2 Picker · Two Sides",
            Story::SecondDisplayWaiting => "Display 2 · Waiting",
            Story::SecondDisplayRefused => "Display 2 · Refused",
            Story::SessionSupersededPill => "Superseded + Pill",
            Story::FilesBrowsing => "Browsing + Transfers",
            Story::FilesEmpty => "Empty Folder + New Folder",
            Story::FilesDownloadOnly => "Download Only + Error",
            Story::ControlsPrintingEmpty => "Printing · Empty",
            Story::ControlsPrintingJobs => "Printing · Jobs, Auto Off",
            Story::ControlsPrintingOff => "Printing · Off by Server",
            Story::ControlsApps => "Header · Apps Button",
            Story::AppsLoading => "Loading Catalog",
            Story::AppsBrowsing => "Browsing + Activity",
            Story::AppsInstalled => "Installed App",
            Story::AppsInstalling => "Installing",
            Story::AppsFailed => "Failed + Error Notice",
            Story::AppsSearchEmpty => "No Search Results",
            Story::AppsCatalogError => "Catalog Error",
            Story::AppsOffline => "Waiting, Narrow (720×460)",

            Story::PrintNoticeStates => "Every Notice State",
            Story::PrintNoticeLongName => "Long Name",
            Story::PrintNoticeStack => "Stack + More",
            Story::PrintNoticeWithStrip => "Below the Stats Strip",
            Story::PrintNoticeNarrow => "Narrow Window",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Story::ConnectDefault => "story-connect-default",
            Story::ConnectBusy => "story-connect-busy",
            Story::ConnectError => "story-connect-error",
            Story::ConnectDormant => "story-connect-dormant",
            Story::ConnectDormantWaiting => "story-connect-dormant-waiting",
            Story::ConnectDormantTimedOut => "story-connect-dormant-timed-out",
            Story::ConnectHeld => "story-connect-held",
            Story::ConnectKeyRevoked => "story-connect-key-revoked",
            Story::ConnectKeyExpiredEnv => "story-connect-key-expired-env",
            Story::ConnectKeyDisabled => "story-connect-key-disabled",
            Story::ConnectKeyClient => "story-connect-key-client",
            Story::ConnectDifferentKey => "story-connect-different-key",
            Story::LibraryPopulated => "story-library-populated",
            Story::LibraryLaunchDialog => "story-library-launch-dialog",
            Story::LibraryLaunchDialogMinimal => "story-library-launch-dialog-minimal",
            Story::LibraryEmpty => "story-library-empty",
            Story::LibraryBusy => "story-library-busy",
            Story::LibraryOpenLink => "story-library-open-link",
            Story::LibraryOpenLinkPending => "story-library-open-link-pending",
            Story::LibraryOpenLinkNotice => "story-library-open-link-notice",
            Story::LibraryOpenLinkNone => "story-library-open-link-none",
            Story::LibraryOpenLinkRejected => "story-library-open-link-rejected",
            Story::LibraryOpenFile => "story-library-open-file",
            Story::LibraryOpenFileFallback => "story-library-open-file-fallback",
            Story::LibraryDropTarget => "story-library-drop-target",
            Story::LibraryDropOnSession => "story-library-drop-session",
            Story::LibraryDropFolders => "story-library-drop-folders",
            Story::LibraryUploading => "story-library-uploading",
            Story::LibraryUploadFailed => "story-library-upload-failed",
            Story::LaunchDialogOpenFile => "story-library-launch-dialog-open-file",
            Story::LaunchDialogOpenLink => "story-library-launch-dialog-open-link",
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
            Story::LibraryAccountUser => "story-library-account-user",
            Story::LibraryAccountMeasuring => "story-library-account-measuring",
            Story::LibraryAccountLimits => "story-library-account-limits",
            Story::LibraryAccountAdmin => "story-library-account-admin",
            Story::LibraryAccountFallback => "story-library-account-fallback",
            Story::ControlsHardware => "story-controls-hardware",
            Story::ControlsJpeg => "story-controls-jpeg",
            Story::ControlsServerLocked => "story-controls-locked",
            Story::ControlsDropdownOpen => "story-controls-dropdown-open",
            Story::ControlsRestoredSections => "story-controls-restored-sections",
            Story::ControlsShortcutsActive => "story-controls-shortcuts-active",
            Story::ControlsShortcutsOff => "story-controls-shortcuts-off",
            Story::ControlsShortcutsLocked => "story-controls-shortcuts-locked",
            Story::ControlsStatsStripOn => "story-controls-stats-strip-on",
            Story::ControlsStatsWaiting => "story-controls-stats-waiting",
            Story::ControlsInputCapture => "story-controls-input-capture",
            Story::ControlsInputCaptureOn => "story-controls-input-capture-on",
            Story::ControlsInputCaptureUnavailable => "story-controls-input-capture-unavailable",
            Story::GamepadsEmpty => "story-controls-gamepads-empty",
            Story::GamepadsSending => "story-controls-gamepads-sending",
            Story::GamepadsReady => "story-controls-gamepads-ready",
            Story::GamepadsFull => "story-controls-gamepads-full",
            Story::GamepadsInputOff => "story-controls-gamepads-input-off",
            Story::GamepadsServerOff => "story-controls-gamepads-server-off",
            Story::AudioDefault => "story-audio-default",
            Story::AudioChosen => "story-audio-chosen",
            Story::AudioMissing => "story-audio-missing",
            Story::AudioNoDevice => "story-audio-no-device",
            Story::AudioRetrying => "story-audio-retrying",
            Story::AudioBitrateLocked => "story-audio-bitrate-locked",
            Story::AudioBitrateHidden => "story-audio-bitrate-hidden",
            Story::AudioOff => "story-audio-off",
            Story::SessionOverlays => "story-session-overlays",
            Story::SessionStartupStalled => "story-session-startup-stalled",
            Story::SessionKeepWaiting => "story-session-keep-waiting",
            Story::SessionStatsStrip => "story-session-stats-strip",
            Story::SessionStatsStripWarn => "story-session-stats-strip-warn",
            Story::SessionStatsStripNarrow => "story-session-stats-strip-narrow",
            Story::SessionStatsStripLight => "story-session-stats-strip-light",
            Story::SessionStatsStripWithBanner => "story-session-stats-strip-banner",
            Story::SessionStatsStripFullScreen => "story-session-stats-strip-fullscreen",
            Story::SessionCaptureHint => "story-session-capture-hint",
            Story::SessionReleaseHint => "story-session-release-hint",
            Story::SessionCaptureHintWithStrip => "story-session-capture-hint-strip",
            Story::SessionCaptureHintWithBanner => "story-session-capture-hint-banner",
            Story::SessionCaptureHintNarrow => "story-session-capture-hint-narrow",
            Story::SessionCaptureHintLight => "story-session-capture-hint-light",
            Story::ControlsSecondScreen => "story-controls-second-screen",
            Story::ControlsSecondScreenOpen => "story-controls-second-screen-open",
            Story::SessionPlacementAllSides => "story-session-placement-all-sides",
            Story::SessionPlacementTwoSides => "story-session-placement-two-sides",
            Story::SecondDisplayWaiting => "story-second-display-waiting",
            Story::SecondDisplayRefused => "story-second-display-refused",
            Story::SessionSupersededPill => "story-session-superseded-pill",
            Story::FilesBrowsing => "story-files-browsing",
            Story::FilesEmpty => "story-files-empty",
            Story::FilesDownloadOnly => "story-files-download-only",
            Story::ControlsPrintingEmpty => "story-controls-printing-empty",
            Story::ControlsPrintingJobs => "story-controls-printing-jobs",
            Story::ControlsPrintingOff => "story-controls-printing-off",
            Story::ControlsApps => "story-controls-apps",
            Story::AppsLoading => "story-apps-loading",
            Story::AppsBrowsing => "story-apps-browsing",
            Story::AppsInstalled => "story-apps-installed",
            Story::AppsInstalling => "story-apps-installing",
            Story::AppsFailed => "story-apps-failed",
            Story::AppsSearchEmpty => "story-apps-search-empty",
            Story::AppsCatalogError => "story-apps-catalog-error",
            Story::AppsOffline => "story-apps-offline",

            Story::PrintNoticeStates => "story-print-notice-states",
            Story::PrintNoticeLongName => "story-print-notice-long-name",
            Story::PrintNoticeStack => "story-print-notice-stack",
            Story::PrintNoticeWithStrip => "story-print-notice-strip",
            Story::PrintNoticeNarrow => "story-print-notice-narrow",
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
                | Story::LaunchDialogOpenFile
                | Story::LaunchDialogOpenLink
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
        match self {
            Story::LibrarySearchHidden => "o",
            Story::LibraryOpenLink
            | Story::LibraryOpenLinkPending
            | Story::LibraryOpenLinkNotice => "wiki.lan/page",
            Story::LibraryOpenLinkNone => "https://wiki.lan/page",
            Story::LibraryOpenLinkRejected => "vnc://desk.lan",
            _ => "",
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
                | Story::ControlsShortcutsActive
                | Story::ControlsShortcutsOff
                | Story::ControlsShortcutsLocked
                | Story::ControlsStatsStripOn
                | Story::ControlsStatsWaiting
                | Story::ControlsSecondScreen
                | Story::ControlsSecondScreenOpen
                | Story::ControlsInputCapture
                | Story::ControlsInputCaptureOn
                | Story::ControlsInputCaptureUnavailable
                | Story::GamepadsEmpty
                | Story::GamepadsSending
                | Story::GamepadsReady
                | Story::GamepadsFull
                | Story::GamepadsInputOff
                | Story::GamepadsServerOff
                | Story::ControlsPrintingEmpty
                | Story::ControlsPrintingJobs
                | Story::ControlsPrintingOff
                | Story::ControlsApps
        ) || self.is_audio()
    }

    /// Session Audio stories: Session Controls with only Audio Settings open.
    fn is_audio(self) -> bool {
        self.group() == "Session Audio"
    }

    /// Stories that open with the account menu open.
    fn account_open(self) -> bool {
        matches!(
            self,
            Story::LibraryAccountUser
                | Story::LibraryAccountMeasuring
                | Story::LibraryAccountLimits
                | Story::LibraryAccountAdmin
                | Story::LibraryAccountFallback
        )
    }

    /// What a Connect story shows.
    fn connect_props(self) -> ConnectProps {
        let server = Some("sealskin.example.internal".to_string());
        let recovery = |kind, waiting, timed_out| ConnectProps {
            server: server.clone(),
            recovery: Some(RecoveryProps {
                kind,
                waiting,
                timed_out,
            }),
            ..ConnectProps::default()
        };
        let problem = |kind, env_note, hint: Option<&str>| ConnectProps {
            server: server.clone(),
            problem: Some(KeyProblem { kind, env_note }),
            hint: hint.map(Into::into),
            ..ConnectProps::default()
        };
        match self {
            Story::ConnectBusy => ConnectProps {
                busy: Some("Connecting to sealskin.example.internal…".into()),
                pending_open: Some("Connect to a server to open “report.pdf”.".into()),
                ..ConnectProps::default()
            },
            Story::ConnectError => ConnectProps {
                status: Some(
                    "Invalid application key format. Expected ssk_<id>_<secret>@<host>.".into(),
                ),
                ..ConnectProps::default()
            },
            Story::ConnectDormant => recovery(RecoveryKind::Dormant, false, false),
            Story::ConnectDormantWaiting => recovery(RecoveryKind::Dormant, true, false),
            Story::ConnectDormantTimedOut => recovery(RecoveryKind::Dormant, false, true),
            Story::ConnectHeld => recovery(RecoveryKind::Held, false, false),
            Story::ConnectKeyRevoked => problem(ProblemKind::Revoked, None, None),
            Story::ConnectKeyExpiredEnv => problem(
                ProblemKind::Expired,
                Some(EnvNote::Environment),
                Some(connect::PASTE_NEW_KEY),
            ),
            Story::ConnectKeyDisabled => problem(ProblemKind::KeysDisabled, None, None),
            Story::ConnectKeyClient => problem(ProblemKind::ClientNotAllowed, None, None),
            Story::ConnectDifferentKey => ConnectProps {
                different_key: true,
                ..ConnectProps::default()
            },
            _ => ConnectProps::default(),
        }
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
            // Input, Shortcuts, Printing and Video open, Screen closed: what a server remembered.
            Story::ControlsRestoredSections => ControlsUiState::default()
                .with_sections([true, true, true, false, true, false, true, false, false]),
            Story::ControlsPrintingEmpty | Story::ControlsPrintingJobs => {
                ControlsUiState::default()
                    .with_sections([false, false, false, true, true, false, false, false, false])
            }
            Story::ControlsPrintingOff => ControlsUiState::default()
                .with_sections([false, false, false, true, false, false, false, false, false]),
            _ if self.is_audio() => ControlsUiState::default()
                .with_sections([false, false, false, false, false, false, false, true, false]),
            Story::ControlsShortcutsActive
            | Story::ControlsShortcutsOff
            | Story::ControlsShortcutsLocked => ControlsUiState::default()
                .with_sections([false, true, false, false, false, false, false, false, false]),
            Story::ControlsStatsStripOn | Story::ControlsStatsWaiting => ControlsUiState::default()
                .with_sections([false, false, false, false, false, false, false, false, true]),
            Story::GamepadsEmpty
            | Story::GamepadsSending
            | Story::GamepadsReady
            | Story::GamepadsFull
            | Story::GamepadsInputOff
            | Story::GamepadsServerOff => ControlsUiState::default()
                .with_sections([false, false, true, false, false, false, false, false, false]),
            Story::ControlsInputCapture
            | Story::ControlsInputCaptureOn
            | Story::ControlsInputCaptureUnavailable => ControlsUiState::default()
                .with_sections([true, false, false, false, false, false, false, false, false]),
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
    apps_props: AppsProps,
    apps_search: Entity<InputState>,
    apps_grid: gpui_kit::UniformListScrollHandle,
    apps_focus: gpui_kit::FocusHandle,
    apps_console: gpui_kit::ScrollHandle,
    /// The account menu is open (controlled, as in the Launcher).
    account_open: bool,
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
            apps_props: Self::fixture_apps(story, &assets),
            apps_search: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Search Apps")
                    .default_value(Self::apps_query(story))
            }),
            apps_grid: gpui_kit::UniformListScrollHandle::new(),
            apps_focus: cx.focus_handle(),
            apps_console: gpui_kit::ScrollHandle::new(),
            canvas_width,
            account_open: story.account_open(),
            assets,
        }
    }

    /// The Apps search field's text for a story.
    fn apps_query(story: Story) -> &'static str {
        if story == Story::AppsSearchEmpty {
            "zzz"
        } else {
            ""
        }
    }

    /// The Apps window's props for a story (`ARCH-035`): the Network category's nine
    /// browsers, two with embedded icons, the rest initials.
    fn fixture_apps(story: Story, assets: &PreviewAssets) -> AppsProps {
        use gpui_kit::assets::IconName as I;
        let offline = story == Story::AppsOffline;
        let installed_filter = story == Story::AppsInstalled;
        let selected = match story {
            Story::AppsInstalled | Story::AppsOffline => Some("firefox"),
            Story::AppsInstalling => Some("chromium"),
            Story::AppsFailed => Some("brave"),
            _ => None,
        };
        let installed = ["firefox", "zen", "vivaldi"];
        let browsers = [
            ("brave", "Brave"),
            ("chromium", "Chromium"),
            ("firefox", "Firefox"),
            ("librewolf", "LibreWolf"),
            ("mullvad-browser", "Mullvad Browser"),
            ("opera", "Opera"),
            ("tor-browser", "Tor Browser"),
            ("vivaldi", "Vivaldi"),
            ("zen", "Zen Browser"),
        ];
        let tile = |name: &str, full_name: &str| {
            let is_installed = !offline && installed.contains(&name);
            let (badge, progress, state) = match (story, name) {
                (Story::AppsBrowsing | Story::AppsInstalling, "chromium") => {
                    (Badge::Running, Some(45.), Some("Downloading 45%"))
                }
                (Story::AppsBrowsing, "brave") => {
                    (Badge::Queued, None, Some("Waiting to install…"))
                }
                (Story::AppsFailed, "brave") => (Badge::Failed, None, Some("Failed (exit 1)")),
                _ if is_installed => (Badge::Installed, None, Some("Installed")),
                _ => (Badge::None, None, None),
            };
            AppTile {
                name: name.into(),
                full_name: full_name.into(),
                initial: full_name.chars().next().unwrap_or('?'),
                icon: match name {
                    "firefox" => Some(assets.icon_firefox.clone()),
                    "zen" => Some(assets.icon_zen.clone()),
                    _ => None,
                },
                want_icon: false,
                badge,
                progress,
                tooltip: match state {
                    Some(state) => format!("{full_name} — {state}"),
                    None => full_name.into(),
                },
                selected: selected == Some(name),
            }
        };
        let tiles: Vec<AppTile> = match story {
            Story::AppsLoading | Story::AppsSearchEmpty | Story::AppsCatalogError => Vec::new(),
            Story::AppsInstalled => browsers
                .iter()
                .filter(|(name, _)| installed.contains(name))
                .map(|(name, full)| tile(name, full))
                .collect(),
            _ => browsers
                .iter()
                .map(|(name, full)| tile(name, full))
                .collect(),
        };
        let category = |id: &str, label: &str, icon: I, count: usize| CategoryRow {
            id: FilterId::Category(id.into()),
            label: label.into(),
            icon,
            count: Some(count),
            selected: !installed_filter && id == "Network",
        };
        let categories = if story == Story::AppsLoading {
            Vec::new()
        } else {
            vec![
                CategoryRow {
                    id: FilterId::Installed,
                    label: "Installed".into(),
                    icon: I::PackageCheck,
                    count: (!offline && story != Story::AppsCatalogError).then_some(3),
                    selected: installed_filter,
                },
                CategoryRow {
                    id: FilterId::All,
                    label: "All Apps".into(),
                    icon: I::LayoutGrid,
                    count: (story != Story::AppsCatalogError).then_some(161),
                    selected: false,
                },
                category("AI", "AI", I::Sparkles, 8),
                category("AudioVideo", "Audio Video", I::Clapperboard, 21),
                category("Creative", "Creative", I::Palette, 23),
                category("Development", "Development", I::Code, 14),
                category("Education", "Education", I::GraduationCap, 2),
                category("Game", "Game", I::Gamepad2, 27),
                category("Network", "Network", I::Network, 9),
                category("Office", "Office", I::FileText, 16),
                category("Science", "Science", I::FlaskConical, 2),
                category("System", "System", I::Cpu, 4),
                category("Utility", "Utility", I::Wrench, 12),
            ]
        };
        let lines: String = (1..=30)
            .map(|i| format!("[{i:02}/30] Pulling layer sha256:{:012x}… done\n", i * 7919))
            .collect();
        let console = |job, status: &str, state, code, progress, text: String| ConsoleProps {
            job,
            status: status.into(),
            state,
            code,
            progress,
            text: text.into(),
            can_cancel: !offline && matches!(state, JobBadge::Queued | JobBadge::Running),
            can_clear: !offline && !matches!(state, JobBadge::Queued | JobBadge::Running),
        };
        let inspector = selected.map(|name| {
            let full_name = browsers
                .iter()
                .find(|(n, _)| *n == name)
                .map_or(name, |(_, full)| *full);
            let is_installed = !offline && installed.contains(&name);
            let (buttons, console) = match story {
                Story::AppsInstalling => (
                    InspectorButtons {
                        shown_installed: false,
                        pending: Some(ButtonKind::Install),
                        pending_tooltip: Some("Downloading 45%".into()),
                        enabled: false,
                    },
                    Some(console(
                        12,
                        "Downloading 45%",
                        JobBadge::Running,
                        None,
                        Some(45.),
                        format!("{lines}Downloading chromium: 45% (88.1 MB of 196 MB)"),
                    )),
                ),
                Story::AppsFailed => (
                    InspectorButtons {
                        shown_installed: false,
                        pending: None,
                        pending_tooltip: None,
                        enabled: true,
                    },
                    Some(console(
                        9,
                        "Failed (exit 1)",
                        JobBadge::Failed,
                        Some(1),
                        None,
                        "Resolving ghcr.io/linuxserver/proot-apps:brave…\nerror: manifest unknown\nproot-apps: install failed".into(),
                    )),
                ),
                Story::AppsInstalled => (
                    InspectorButtons {
                        shown_installed: true,
                        pending: None,
                        pending_tooltip: None,
                        enabled: true,
                    },
                    Some(console(
                        4,
                        "Updated",
                        JobBadge::Done,
                        Some(0),
                        None,
                        "Checking ghcr.io/linuxserver/proot-apps:firefox…\nfirefox is up to date".into(),
                    )),
                ),
                _ => (
                    InspectorButtons {
                        shown_installed: is_installed,
                        pending: None,
                        pending_tooltip: None,
                        enabled: !offline,
                    },
                    None,
                ),
            };
            Inspector {
                name: name.into(),
                full_name: full_name.into(),
                description: "A free and open-source web browser developed by the Mozilla Foundation, with tracking protection and extensions.".into(),
                icon: match name {
                    "firefox" => Some(assets.icon_firefox.clone()),
                    "zen" => Some(assets.icon_zen.clone()),
                    _ => None,
                },
                initial: full_name.chars().next().unwrap_or('?'),
                installed: is_installed,
                buttons,
                console,
            }
        });
        let activity = match story {
            Story::AppsBrowsing => vec![
                ActivityRow {
                    id: 12,
                    app: "chromium".into(),
                    full_name: "Chromium".into(),
                    status: "Downloading 45%".into(),
                    state: JobBadge::Running,
                    progress: Some(45.),
                    canceling: false,
                },
                ActivityRow {
                    id: 13,
                    app: "brave".into(),
                    full_name: "Brave".into(),
                    status: "Waiting to install…".into(),
                    state: JobBadge::Queued,
                    progress: None,
                    canceling: false,
                },
                ActivityRow {
                    id: 11,
                    app: "vivaldi".into(),
                    full_name: "Vivaldi".into(),
                    status: "Installed".into(),
                    state: JobBadge::Done,
                    progress: None,
                    canceling: false,
                },
            ],
            Story::AppsInstalling => vec![ActivityRow {
                id: 12,
                app: "chromium".into(),
                full_name: "Chromium".into(),
                status: "Downloading 45%".into(),
                state: JobBadge::Running,
                progress: Some(45.),
                canceling: false,
            }],
            Story::AppsFailed => vec![ActivityRow {
                id: 9,
                app: "brave".into(),
                full_name: "Brave".into(),
                status: "Failed (exit 1)".into(),
                state: JobBadge::Failed,
                progress: None,
                canceling: false,
            }],
            _ => Vec::new(),
        };
        let width = px(if offline { 720. } else { 980. });
        let (source, inspector_width) = apps_ui::pane_widths(width);
        AppsProps {
            categories,
            filter: if installed_filter {
                FilterId::Installed
            } else {
                FilterId::Category("Network".into())
            },
            search: Self::apps_query(story).into(),
            tiles: Rc::new(tiles),
            columns: apps_ui::columns(width - px(source + inspector_width + 2.)),
            inspector,
            activity,
            catalog: match story {
                Story::AppsLoading => CatalogView::Loading,
                Story::AppsCatalogError => CatalogView::Failed(
                    "Couldn’t load the app catalog. Check the Mac’s internet connection.".into(),
                ),
                _ => CatalogView::Ready,
            },
            installed_count: (!offline).then_some(3),
            notice: (story == Story::AppsFailed).then(|| Notice {
                text: "Couldn’t open “Firefox”: firefox: command not found".into(),
                error: true,
            }),
            banner: offline.then_some(Banner::Waiting),
            enabled: !offline,
            focused: true,
            width,
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
        self.account_open = story.account_open();
        self.files_props = Self::fixture_files(story);
        self.apps_props = Self::fixture_apps(story, &self.assets);
        let query = Self::apps_query(story);
        self.apps_search
            .update(cx, |state, cx| state.set_value(query, window, cx));
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
        if matches!(
            story,
            Story::SessionStatsStripLight | Story::SessionCaptureHintLight
        ) {
            // The pill stays dark whatever the theme.
            self.appearance = Appearance::Light;
            Theme::change(ThemeMode::Light, Some(window), cx);
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
        match story {
            Story::LaunchDialogOpenFile => {
                dialog = Self::fixture_launch_dialog("libreoffice", "LibreOffice", None, false);
                dialog.opening = Some(OpeningLabel {
                    kind: OpeningKind::File,
                    label: "report.pdf".into(),
                    detail: Some("PDF · 2.4 MB".into()),
                    tooltip: Some("/Users/kim/Documents/report.pdf".into()),
                });
            }
            Story::LaunchDialogOpenLink => {
                dialog.opening = Some(OpeningLabel {
                    kind: OpeningKind::Link,
                    label: "wiki.lan/very/long/path/…/page".into(),
                    detail: None,
                    tooltip: Some("https://wiki.lan/very/long/path/to/the/page".into()),
                });
            }
            _ => {}
        }
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
            opening: None,
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
                files: true,
                accepts_drop: true,
            };
        let mut sessions: Vec<SessionItem> = match story {
            Story::LibraryLaunching
            | Story::LibraryLaunchingPull
            | Story::LibraryLaunchFailed
            | Story::LibraryUploadFailed => Vec::new(),
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
        if story == Story::LibraryUploading {
            // A collaboration room: no Files button, not a drop target.
            sessions.push(SessionItem {
                id: "sess-room".into(),
                app_name: "Firefox (room)".into(),
                started_label: Some("Started 4m ago".into()),
                gpu: false,
                storage_label: Some("Cleanroom".into()),
                thumbnail: None,
                app_icon: Some(Arc::clone(&self.assets.icon_firefox)),
                files: false,
                accepts_drop: false,
            });
        }
        if matches!(story, Story::LibraryOpenFile) {
            for (id, name) in [
                ("libreoffice", "LibreOffice"),
                ("evince", "Evince"),
                ("gimp", "GIMP"),
                ("okular", "Okular"),
                ("calibre", "Calibre"),
                ("xournal", "Xournal++"),
                ("scribus", "Scribus"),
            ] {
                apps.push(self.app_item(id, name, "office"));
            }
        }
        let (launching, launching_apps) = self.fixture_launching();
        let opening = self.fixture_opening();
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
            status: (story == Story::LibraryDropFolders).then(|| {
                "Open one file at a time. To copy a folder, use Files on a running session.".into()
            }),
            busy: None,
            account: Self::fixture_account(self.story, server),
            account_open: self.account_open,
            omnibar: match story {
                Story::LibraryOpenLink
                | Story::LibraryOpenLinkPending
                | Story::LibraryOpenLinkNotice
                | Story::LibraryOpenLinkNone => OmnibarIcon::Link,
                Story::LibraryOpenLinkRejected => OmnibarIcon::Rejected,
                _ => OmnibarIcon::Search,
            },
            opening,
            files_pending: HashSet::new(),
            accepts_drops: true,
            drop_preview: match story {
                Story::LibraryDropTarget => Some(DropPreview::Library),
                Story::LibraryDropOnSession => Some(DropPreview::Session("sess-zen".into())),
                Story::LibraryDropFolders => Some(DropPreview::FoldersOnly),
                _ => None,
            },
        }
    }

    /// The open chooser of the Open Link and Open File stories.
    fn fixture_opening(&self) -> Option<OpeningProps> {
        let story = self.story;
        let zen = Some(Arc::clone(&self.assets.icon_zen));
        let lightcraft = Some(Arc::clone(&self.assets.icon_lightcraft));
        let session = |id: &str, name: &str, detail: &str, icon: Option<Arc<RenderImage>>| {
            OpeningSessionRow {
                id: id.into(),
                app_name: name.into(),
                detail: detail.into(),
                icon,
            }
        };
        let app = |id: &str, name: &str, icon: Option<Arc<RenderImage>>| OpeningAppRow {
            id: id.into(),
            name: name.into(),
            icon,
            launching: false,
        };
        let base = OpeningProps {
            kind: OpeningKind::Link,
            label: "wiki.lan/page".into(),
            tooltip: Some("https://wiki.lan/page".into()),
            detail: None,
            rejected: false,
            sessions: Vec::new(),
            apps: Vec::new(),
            apps_expanded: false,
            queue_more: false,
            pending_session: None,
            note: None,
            enter_hint: false,
            generation: 1,
        };
        let link_rows = |mut props: OpeningProps| {
            props.sessions = vec![
                session("sess-zen", "Zen", "Started 5m ago · auto-zen", zen.clone()),
                session("sess-chromium", "Chromium", "Started 2 days ago", None),
            ];
            props.apps = vec![
                app("zen", "Zen", zen.clone()),
                app("chromium", "Chromium", None),
                app(
                    "firefox",
                    "Firefox ESR",
                    Some(Arc::clone(&self.assets.icon_firefox)),
                ),
            ];
            props.enter_hint = true;
            props
        };
        let note = |text: &str, error: bool| {
            Some(Notice {
                text: text.into(),
                error,
            })
        };
        match story {
            Story::LibraryOpenLink => Some(link_rows(base)),
            Story::LibraryOpenLinkPending => Some(OpeningProps {
                pending_session: Some("sess-zen".into()),
                ..link_rows(base)
            }),
            Story::LibraryOpenLinkNotice => Some(OpeningProps {
                note: note(
                    "Zen couldn’t take the link. Choose another session or launch a new one.",
                    true,
                ),
                ..link_rows(base)
            }),
            Story::LibraryOpenLinkNone => Some(OpeningProps {
                note: note("No app on this server opens links.", false),
                ..base
            }),
            Story::LibraryOpenLinkRejected => Some(OpeningProps {
                label: "vnc://desk.lan".into(),
                tooltip: None,
                rejected: true,
                note: note(
                    "Skerry opens web (http, https), ftp and magnet links.",
                    false,
                ),
                ..base
            }),
            Story::LibraryOpenFile => {
                let mut apps = vec![
                    app("libreoffice", "LibreOffice", None),
                    app("evince", "Evince", None),
                    app("zen", "Zen", zen.clone()),
                ];
                for (id, name) in [
                    ("gimp", "GIMP"),
                    ("okular", "Okular"),
                    ("calibre", "Calibre"),
                    ("xournal", "Xournal++"),
                    ("scribus", "Scribus"),
                    ("inkscape", "Inkscape"),
                    ("krita", "Krita"),
                ] {
                    apps.push(app(id, name, None));
                }
                Some(OpeningProps {
                    kind: OpeningKind::File,
                    label: "report.pdf".into(),
                    tooltip: Some("/Users/kim/Documents/report.pdf".into()),
                    detail: Some("PDF · 2.4 MB · 1 of 3".into()),
                    sessions: vec![
                        session("sess-zen", "Zen", "Started 5m ago · auto-zen", zen.clone()),
                        session(
                            "sess-lightcraft",
                            "LightCraft",
                            "Started 2 days ago",
                            lightcraft,
                        ),
                    ],
                    apps,
                    queue_more: true,
                    ..base
                })
            }
            Story::LibraryOpenFileFallback => Some(OpeningProps {
                kind: OpeningKind::File,
                label: "data.xyz".into(),
                tooltip: Some("/Users/kim/Downloads/data.xyz".into()),
                detail: Some("XYZ · 120 KB".into()),
                apps: vec![
                    app("lightcraft", "LightCraft", lightcraft),
                    app(
                        "weixin",
                        "Weixin",
                        Some(Arc::clone(&self.assets.icon_weixin)),
                    ),
                    app("zen", "Zen", zen),
                    app(
                        "vscode",
                        "VS Code",
                        Some(Arc::clone(&self.assets.icon_vscode)),
                    ),
                    app(
                        "firefox",
                        "Firefox",
                        Some(Arc::clone(&self.assets.icon_firefox)),
                    ),
                ],
                note: note("No app lists .xyz files, so all apps are shown.", false),
                ..base
            }),
            _ => None,
        }
    }

    /// The account menu each Library story shows.
    fn fixture_account(story: Story, server: &str) -> AccountProps {
        const GIB: f64 = 1024. * 1024. * 1024.;
        let chip = |label: &'static str, allowed| AccessChip {
            label: label.into(),
            allowed,
        };
        let limits = |storage, allowance, session_limit| Limits {
            running: if session_limit < 0 { 2 } else { 1 },
            session_limit,
            session_cpus: if session_limit < 0 { -1. } else { 4. },
            session_memory_mb: if session_limit < 0 { -1 } else { 8192 },
            session_hours: if session_limit < 0 { -1. } else { 8. },
            session_idle_minutes: if session_limit < 0 { -1. } else { 30. },
            storage: Some(StorageUse {
                limit_gb: 50,
                used: storage,
            }),
            allowance: Some(allowance),
        };
        let user = AccountProps {
            user: "kebot".into(),
            server: server.into(),
            admin: false,
            groups: vec!["users".into(), "designers".into()],
            access: vec![
                chip("Persistent storage", true),
                chip("GPU", true),
                chip("VPN", false),
            ],
            limits: account::limit_rows(&limits(
                StorageMeasure::Bytes((12.4 * GIB) as u64),
                (12.5, 40., "month".into()),
                2,
            )),
            key_source: Some(KeySourceLabel::SavedFile),
            key_id: "3f9a1c21".into(),
            dormancy: account::dormancy_text(24., false),
        };
        match story {
            Story::LibraryAccountMeasuring => AccountProps {
                limits: account::limit_rows(&limits(
                    StorageMeasure::Measuring,
                    (12.5, 40., "month".into()),
                    2,
                )),
                ..user
            },
            Story::LibraryAccountLimits => AccountProps {
                groups: Vec::new(),
                limits: account::limit_rows(&limits(
                    StorageMeasure::Bytes((43.1 * GIB) as u64),
                    (40., 40., "week".into()),
                    -1,
                )),
                key_source: Some(KeySourceLabel::Environment),
                dormancy: account::dormancy_text(24., true),
                ..user
            },
            Story::LibraryAccountAdmin => AccountProps {
                user: "admin".into(),
                admin: true,
                groups: Vec::new(),
                limits: Vec::new(),
                dormancy: None,
                ..user
            },
            Story::LibraryAccountFallback => AccountProps {
                user: String::new(),
                server: server.into(),
                key_source: Some(KeySourceLabel::SavedFile),
                key_id: "3f9a1c21".into(),
                ..AccountProps::default()
            },
            _ => user,
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
                    upload: None,
                    subject: None,
                    cancellable: false,
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
            Story::LibraryUploading => (
                vec![
                    LaunchingItem {
                        upload: Some(UploadProgress {
                            fraction: 0.3,
                            label: "Uploading 12 MB of 40 MB".into(),
                        }),
                        subject: Some((
                            OpeningKind::File,
                            "quarterly-report-final.xlsx".into(),
                        )),
                        cancellable: true,
                        detail: None,
                        ..card(
                            "l3",
                            "LibreOffice",
                            None,
                            Vec::new(),
                            None,
                            "Launching · 0:09",
                            None,
                        )
                    },
                    LaunchingItem {
                        subject: Some((OpeningKind::Link, "wiki.lan/page".into())),
                        ..card(
                            "l4",
                            "Zen",
                            Some(Arc::clone(&self.assets.icon_zen)),
                            vec![
                                row("Requesting", Some("1 s"), StageState::Done),
                                row("Starting", Some("3 s"), StageState::Current),
                                row("Connecting", None, StageState::Pending),
                            ],
                            Some("Creating the container"),
                            "Launching · 0:04",
                            None,
                        )
                    },
                ],
                vec![("zen", "Starting…")],
            ),
            Story::LibraryUploadFailed => (
                vec![LaunchingItem {
                    subject: Some((OpeningKind::File, "report.pdf".into())),
                    ..card(
                        "l5",
                        "LibreOffice",
                        None,
                        vec![row("Requesting", Some("0 s"), StageState::Failed)],
                        None,
                        "Launch failed",
                        Some(LaunchNotice::Failed {
                            headline: "Couldn’t launch LibreOffice".into(),
                            body: "Your SealSkin storage is full. Free space in Files, then try again."
                                .into(),
                        }),
                    )
                }],
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
            ControlsAction::SecondScreen => {
                let second = &mut self.controls_props.screen.second_screen;
                second.open = !second.open;
                second.note = second.open.then(|| "Screen 2 · Connecting…".into());
            }
            ControlsAction::SetGamepadInput(on) => {
                if let Some(gamepads) = self.controls_props.gamepads.as_mut() {
                    gamepads.input = on;
                }
            }
            ControlsAction::SetGamepadRumble(on) => {
                if let Some(gamepads) = self.controls_props.gamepads.as_mut() {
                    gamepads.rumble = on;
                }
            }
            ControlsAction::CapturePointer => {
                let capture = &mut self.controls_props.capture;
                if capture.available {
                    capture.captured = !capture.captured;
                }
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
            ControlsAction::SelectAudioOutput(uid) => {
                if let Some(audio) = self.controls_props.audio_settings.as_mut() {
                    // Choosing anything replaces a missing saved device, and its note.
                    if audio.outputs.iter().any(|item| !item.connected) {
                        audio.outputs.retain(|item| item.connected);
                        audio.note = None;
                    }
                    for item in &mut audio.outputs {
                        item.checked = item.uid == uid;
                    }
                    if let Some(item) = audio.outputs.iter().find(|item| item.checked) {
                        audio.output_label = item.label.clone();
                    }
                    self.last_event = Some(format!("Audio: Output → {}", audio.output_label));
                }
            }
            ControlsAction::SetAudioBitrate(bps) => {
                if let Some(bitrate) = self
                    .controls_props
                    .audio_settings
                    .as_mut()
                    .and_then(|audio| audio.bitrate.as_mut())
                {
                    bitrate.value = bps;
                }
                self.last_event = Some(format!("Audio: Bitrate → {}", controls::kbps_label(bps)));
            }
            ControlsAction::CopyStats => {
                self.controls_props.stats_copied = !self.controls_props.stats_copied;
            }
            ControlsAction::SetStatsStrip(on) => self.controls_props.stats_strip = on,
            ControlsAction::SetMacShortcuts(active) => {
                if !self.controls_props.shortcuts.locked {
                    self.controls_props.shortcuts.mac_active = active;
                }
            }
            ControlsAction::SetAutoPrint(on) => {
                if let Some(printing) = self.controls_props.printing.as_mut() {
                    printing.auto_print = on;
                }
            }
            ControlsAction::PrintJob(id, PrintJobAction::Remove) => {
                if let Some(printing) = self.controls_props.printing.as_mut() {
                    printing.jobs.retain(|row| row.id != id);
                }
            }
            ControlsAction::PrintJob(..) => {}
            ControlsAction::Reconnect
            | ControlsAction::OpenFiles
            | ControlsAction::OpenApps
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
                apps_available: story == Story::ControlsApps,
                app_name: "Zen Browser".into(),
                fullscreen: false,
                audio: Some(true),
                video: Some(true),
                audio_unavailable: None,
                audio_settings: Some(Self::fixture_audio(Story::AudioDefault)),
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
                    second_screen: SecondScreenProps::default(),
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
                stats_strip: false,
                shortcuts: Self::fixture_shortcuts(true, false),
                capture: Self::fixture_capture(true, false),
                gamepads: Some(Self::fixture_gamepads(Story::GamepadsSending)),
                printing: Some(Self::fixture_printing(true, false)),
            },
            Story::ControlsServerLocked => ControlsProps {
                apps_available: story == Story::ControlsApps,
                app_name: "Locked Enterprise Desktop".into(),
                fullscreen: true,
                audio: Some(true),
                video: Some(false),
                audio_unavailable: None,
                audio_settings: Some(Self::fixture_audio(Story::AudioBitrateLocked)),
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
                    second_screen: SecondScreenProps::default(),
                },
                video_settings: None,
                stats: None,
                stats_copied: false,
                stats_strip: false,
                shortcuts: Self::fixture_shortcuts(true, false),
                capture: Self::fixture_capture(true, false),
                gamepads: Some(Self::fixture_gamepads(Story::GamepadsSending)),
                printing: Some(Self::fixture_printing(true, false)),
            },
            _ if story.is_audio() => {
                let mut props = Self::fixture_controls(Story::ControlsHardware);
                props.audio_settings = Some(Self::fixture_audio(story));
                props.audio_unavailable = match story {
                    Story::AudioNoDevice => Some("No audio output".into()),
                    Story::AudioRetrying => Some("Audio output stopped".into()),
                    Story::AudioOff => Some("Audio turned off by the server".into()),
                    _ => None,
                };
                if story == Story::AudioOff {
                    props.audio = None;
                }
                props
            }
            Story::ControlsShortcutsActive
            | Story::ControlsShortcutsOff
            | Story::ControlsShortcutsLocked => {
                let mut props = Self::fixture_controls(Story::ControlsHardware);
                props.shortcuts = match story {
                    Story::ControlsShortcutsOff => Self::fixture_shortcuts(false, false),
                    Story::ControlsShortcutsLocked => Self::fixture_shortcuts(false, true),
                    _ => Self::fixture_shortcuts(true, false),
                };
                props
            }
            Story::GamepadsEmpty
            | Story::GamepadsSending
            | Story::GamepadsReady
            | Story::GamepadsFull
            | Story::GamepadsInputOff
            | Story::GamepadsServerOff => {
                let mut props = Self::fixture_controls(Story::ControlsHardware);
                props.gamepads = Some(Self::fixture_gamepads(story));
                props
            }
            Story::ControlsInputCapture
            | Story::ControlsInputCaptureOn
            | Story::ControlsInputCaptureUnavailable => {
                let mut props = Self::fixture_controls(Story::ControlsHardware);
                props.capture = Self::fixture_capture(
                    story != Story::ControlsInputCaptureUnavailable,
                    story == Story::ControlsInputCaptureOn,
                );
                props
            }
            Story::ControlsPrintingEmpty
            | Story::ControlsPrintingJobs
            | Story::ControlsPrintingOff => {
                let mut props = Self::fixture_controls(Story::ControlsHardware);
                props.printing = match story {
                    Story::ControlsPrintingOff => None,
                    Story::ControlsPrintingJobs => Some(Self::fixture_printing(false, true)),
                    _ => Some(Self::fixture_printing(true, false)),
                };
                props
            }
            Story::ControlsSecondScreen | Story::ControlsSecondScreenOpen => {
                let mut props = Self::fixture_controls(Story::ControlsHardware);
                props.screen.second_screen = SecondScreenProps {
                    available: true,
                    open: story == Story::ControlsSecondScreenOpen,
                    note: (story == Story::ControlsSecondScreenOpen)
                        .then(|| "Screen 2 · Right · 1920 × 1080".into()),
                };
                props
            }
            Story::ControlsStatsStripOn | Story::ControlsStatsWaiting => {
                let mut props = Self::fixture_controls(Story::ControlsHardware);
                props.stats_strip = story == Story::ControlsStatsStripOn;
                if story == Story::ControlsStatsWaiting {
                    props.stats = None;
                }
                props
            }
            _ => ControlsProps {
                apps_available: story == Story::ControlsApps,
                app_name: "Visual Studio Code".into(),
                fullscreen: false,
                audio: Some(true),
                video: Some(true),
                audio_unavailable: None,
                audio_settings: Some(Self::fixture_audio(Story::AudioDefault)),
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
                    second_screen: SecondScreenProps::default(),
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
                stats_strip: false,
                shortcuts: Self::fixture_shortcuts(true, false),
                capture: Self::fixture_capture(true, false),
                gamepads: Some(Self::fixture_gamepads(Story::GamepadsSending)),
                printing: Some(Self::fixture_printing(true, false)),
            },
        }
    }

    /// The Audio Settings card each Session Audio story shows, as `skerry-mac` builds it.
    fn fixture_audio(story: Story) -> AudioProps {
        let item = |uid: Option<&str>, label: &str, checked: bool| AudioOutputItem {
            uid: uid.map(str::to_string),
            label: label.into(),
            checked,
            connected: true,
        };
        let selkies = vec![
            32_000, 48_000, 64_000, 96_000, 128_000, 192_000, 256_000, 320_000, 384_000, 510_000,
        ];
        let bitrate = |value: u32| {
            Some(AudioBitrateProps {
                value,
                choices: selkies.clone(),
                locked: false,
            })
        };
        // MacBook Pro Speakers is the default; `chosen` is checked.
        let outputs = |chosen: Option<&str>| {
            vec![
                item(
                    None,
                    "System Default (MacBook Pro Speakers)",
                    chosen.is_none(),
                ),
                item(Some("airpods"), "AirPods Pro", chosen == Some("airpods")),
                item(
                    Some("builtin"),
                    "MacBook Pro Speakers",
                    chosen == Some("builtin"),
                ),
                item(
                    Some("studio"),
                    "Studio Display Speakers",
                    chosen == Some("studio"),
                ),
            ]
        };
        let card = |output_label: &str, outputs, note: Option<&str>, bitrate| AudioProps {
            output_label: output_label.into(),
            outputs,
            note: note.map(str::to_string),
            bitrate,
            unavailable: None,
        };
        match story {
            Story::AudioChosen => card(
                "AirPods Pro",
                outputs(Some("airpods")),
                None,
                bitrate(320_000),
            ),
            Story::AudioMissing => {
                let mut list: Vec<_> = outputs(Some("airpods"))
                    .into_iter()
                    .filter(|item| item.uid.as_deref() != Some("airpods"))
                    .collect();
                list.push(AudioOutputItem {
                    uid: Some("airpods".into()),
                    label: "AirPods Pro (not connected)".into(),
                    checked: true,
                    connected: false,
                });
                card(
                    "AirPods Pro (not connected)",
                    list,
                    Some("AirPods Pro isn’t connected. Playing through MacBook Pro Speakers until it’s back."),
                    bitrate(128_000),
                )
            }
            Story::AudioNoDevice => card(
                "System Default",
                vec![item(None, "System Default", true)],
                Some("No audio output is connected. Audio resumes when one is."),
                bitrate(128_000),
            ),
            Story::AudioRetrying => card(
                "System Default (MacBook Pro Speakers)",
                outputs(None),
                Some("Audio output stopped. Trying again…"),
                bitrate(128_000),
            ),
            Story::AudioBitrateLocked => card(
                "System Default (MacBook Pro Speakers)",
                outputs(None),
                None,
                Some(AudioBitrateProps {
                    value: 96_000,
                    choices: vec![96_000],
                    locked: true,
                }),
            ),
            Story::AudioBitrateHidden => card(
                "Elgato Wave XLR Studio Monitor Output",
                vec![
                    item(None, "System Default (LG UltraFine Display Audio)", false),
                    item(
                        Some("elgato"),
                        "Elgato Wave XLR Studio Monitor Output",
                        true,
                    ),
                    item(Some("lg"), "LG UltraFine Display Audio", false),
                    item(Some("builtin"), "MacBook Pro Speakers", false),
                    item(Some("unnamed"), "Output Device 1", false),
                    item(Some("usb-a"), "USB Audio Device", false),
                    item(Some("usb-b"), "USB Audio Device (2)", false),
                ],
                None,
                None,
            ),
            Story::AudioOff => AudioProps {
                output_label: String::new(),
                outputs: Vec::new(),
                note: None,
                bitrate: None,
                unavailable: Some("Audio turned off by the server".into()),
            },
            _ => card(
                "System Default (MacBook Pro Speakers)",
                outputs(None),
                None,
                bitrate(128_000),
            ),
        }
    }

    /// Stats as a session shows them a minute after the section opened:
    /// `hardware` is an H.265 stream on a GPU host, else a JPEG stream whose
    /// capture fell back to a readback and whose rate overshoots its target.
    /// The Shortcuts card as `skerry-mac` builds it from `shortcuts::CHORDS`.
    fn fixture_shortcuts(mac_active: bool, locked: bool) -> ShortcutsProps {
        let row = |label: &str, modifiers: &str, key: &str, remote: Option<&str>| ShortcutRow {
            label: label.into(),
            modifiers: modifiers.into(),
            key: key.into(),
            remote: remote.map(|sent| format!("Sent as {sent}")),
            always_local: remote.is_none(),
        };
        ShortcutsProps {
            mac_active,
            locked,
            rows: vec![
                row("Session Controls", "⇧⌘", "M", Some("Ctrl+Shift+M")),
                row("Full Screen", "⌃⌘", "F", Some("Ctrl+F")),
                row("Files", "⇧⌘", "F", Some("Ctrl+Shift+F")),
                row("Library", "⌘", "0", Some("Ctrl+0")),
                row("Capture Pointer", "⌃⌥⇧", "Z", None),
                row("Quit", "⌘", "Q", None),
            ],
        }
    }

    /// The Gamepads card for a story.
    fn fixture_gamepads(story: Story) -> GamepadsProps {
        let pad = |name: &str, status, rumble| {
            Some(PadRow {
                name: name.into(),
                status,
                rumble,
            })
        };
        let status = match story {
            Story::GamepadsReady => PadStatus::Ready,
            Story::GamepadsInputOff | Story::GamepadsServerOff => PadStatus::NotSent,
            _ => PadStatus::Sending,
        };
        let two = [
            pad("DualSense Wireless Controller", status, true),
            pad("Xbox Wireless Controller", status, false),
            None,
            None,
        ];
        let (slots, overflow) = match story {
            Story::GamepadsEmpty => ([None, None, None, None], 0),
            Story::GamepadsFull => (
                [
                    pad("DualSense Wireless Controller", status, true),
                    pad("Xbox Wireless Controller", status, false),
                    pad("Pro Controller", status, true),
                    pad("8BitDo Ultimate", status, false),
                ],
                1,
            ),
            _ => (two, 0),
        };
        GamepadsProps {
            input: story != Story::GamepadsInputOff,
            input_locked: false,
            rumble: true,
            server_off: story == Story::GamepadsServerOff,
            key: story != Story::GamepadsReady,
            slots,
            overflow,
        }
    }

    /// The Input card's Capture Pointer button.
    fn fixture_capture(available: bool, captured: bool) -> CaptureProps {
        CaptureProps {
            available,
            captured,
            modifiers: "⌃⌥⇧".into(),
            key: "Z".into(),
        }
    }

    /// The capture hint over a session frame (`ARCH-032`).
    fn capture_hint_overlay(hint: &str, width: f32, banner: bool) -> gpui_kit::Div {
        let text = if hint == "captured" {
            session::CaptureHintText {
                lead: "Pointer captured · ".into(),
                chord: Some("⌃⌥⇧Z".into()),
                tail: " releases it".into(),
            }
        } else if hint == "unavailable" {
            session::CaptureHintText {
                lead: "Capture Pointer is available once the picture appears".into(),
                chord: None,
                tail: "".into(),
            }
        } else {
            session::CaptureHintText {
                lead: "Pointer released".into(),
                chord: None,
                tail: "".into(),
            }
        };
        session::capture_hint(
            &text,
            session::capture_hint_bottom(banner),
            px(width),
            session::HintFade::None,
            0,
        )
    }

    /// The strip over the stream: all good, or the warn case (throttled, over target,
    /// nothing sent lately, no decoder yet).
    /// The Printing card (`ARCH-033`): empty, or a row in every state.
    fn fixture_printing(auto_print: bool, jobs: bool) -> PrintingProps {
        let row = |id: u64, name: &str, meta: &str, kind: PrintRowKind| PrintJobRow {
            id,
            name: name.into(),
            meta: meta.into(),
            kind,
            saved_to: None,
            opened: false,
        };
        let rows = if jobs {
            vec![
                row(
                    8,
                    "Invoice 2026-11.pdf",
                    "1.2 MB of 3.4 MB",
                    PrintRowKind::Receiving {
                        fraction: Some(0.35),
                    },
                ),
                row(7, "Report.pdf", "3.4 MB · Just now", PrintRowKind::Ready),
                PrintJobRow {
                    saved_to: Some("Saved to Downloads".into()),
                    ..row(6, "Slides.pdf", "8.1 MB · 2 min ago", PrintRowKind::Ready)
                },
                row(
                    5,
                    "Protected.pdf",
                    "640 KB · 5 min ago · Can’t print here",
                    PrintRowKind::Unreadable,
                ),
                row(
                    4,
                    "Invoice.pdf",
                    "3.4 MB · 12 min ago · Couldn’t receive",
                    PrintRowKind::Failed,
                ),
                row(
                    3,
                    "Archive scan.pdf",
                    "1.4 GB · 1 hr ago · Too large to receive",
                    PrintRowKind::TooLarge,
                ),
                PrintJobRow {
                    opened: true,
                    ..row(
                        2,
                        "Old notes.pdf",
                        "220 KB · 3 hr ago · No longer on this Mac",
                        PrintRowKind::Missing,
                    )
                },
                row(
                    1,
                    "Quarterly Financial Statement for the Board of Directors – Final (3).pdf",
                    "2.2 MB · 3 hr ago",
                    PrintRowKind::Ready,
                ),
            ]
        } else {
            Vec::new()
        };
        PrintingProps {
            auto_print,
            jobs: rows,
            at_cap: false,
        }
    }

    /// A print notice in every state, in the order of the design's table.
    fn fixture_print_notices() -> Vec<PrintNoticeProps> {
        let notice = |id: u64, name: &str, kind: PrintNoticeKind| PrintNoticeProps {
            id,
            name: name.to_string().into(),
            kind,
        };
        let ready = |detail: &str, saved: bool| PrintNoticeKind::Ready {
            detail: detail.to_string().into(),
            saved,
        };
        let failed = |detail: &str| PrintNoticeKind::Failed {
            detail: detail.to_string().into(),
        };
        vec![
            notice(
                1,
                "Report.pdf",
                PrintNoticeKind::Receiving {
                    received: 1_200_000,
                    total: 3_400_000,
                },
            ),
            notice(
                2,
                "Report.pdf",
                PrintNoticeKind::Receiving {
                    received: 1_200_000,
                    total: 0,
                },
            ),
            notice(3, "Report.pdf", ready("3.4 MB · From Firefox", false)),
            notice(
                4,
                "Report.pdf",
                ready("3.4 MB · The print dialog opens when you come back", false),
            ),
            notice(5, "Report.pdf", ready("Saved to Downloads", true)),
            notice(
                6,
                "Report.pdf",
                ready("Finish the open print dialog first.", false),
            ),
            notice(7, "Report.pdf", PrintNoticeKind::Unreadable),
            notice(
                8,
                "Report.pdf",
                failed("The connection dropped partway. Print it again from the session."),
            ),
            notice(
                9,
                "Report.pdf",
                failed("What arrived wasn’t a PDF. Print it again from the session."),
            ),
            notice(
                10,
                "Report.pdf",
                failed("Printing is turned off for this session."),
            ),
            notice(
                11,
                "Report.pdf",
                failed("Couldn’t reach the session. Reconnect to receive Report.pdf."),
            ),
            notice(
                12,
                "Report.pdf",
                PrintNoticeKind::TooLarge {
                    detail: "It’s 1.4 GB. Skerry receives print jobs up to 1 GiB.".into(),
                },
            ),
            notice(13, "Report.pdf", PrintNoticeKind::Missing),
        ]
    }

    /// The notice stack over a session frame, as `SessionWindow` draws it.
    fn print_frame(
        theme: &gpui_kit::component::Theme,
        width: f32,
        height: f32,
        notices: &[PrintNoticeProps],
        strip: Option<&stats::StripProps>,
        entity: Entity<Self>,
    ) -> gpui_kit::Div {
        let top =
            session::print_notices_top(strip.map(|strip| stats::strip_bottom(strip, px(width))));
        let shown = session::print_notices_shown(px(height), top).min(notices.len());
        let more = notices.len() - shown;
        let frame = match strip {
            Some(strip) => Self::session_frame(width, height, strip),
            None => div()
                .relative()
                .flex_none()
                .w(px(width))
                .h(px(height))
                .overflow_hidden()
                .bg(gpui_kit::linear_gradient(
                    180.,
                    gpui_kit::linear_color_stop(rgb(0xe9eef4), 0.),
                    gpui_kit::linear_color_stop(rgb(0x1b2430), 0.55),
                )),
        };
        let action_entity = entity.clone();
        frame
            .child(session::controls_trigger(
                point(px(width - 40.), px(8.)),
                px(32.),
                |_, _, _| {},
            ))
            .child(session::print_notices(
                theme,
                &notices[..shown],
                more,
                px(width),
                top,
                Rc::new(move |id, action, _, cx| {
                    action_entity.update(cx, |this, cx| {
                        this.record_event(format!("Print: {action:?} #{id}"), cx)
                    });
                }),
                Rc::new(|_, _, _, _| {}),
            ))
    }

    fn fixture_strip(warn: bool) -> stats::StripProps {
        use stats::{Status, StripFigure, StripProps};
        let figure = |value: &str, caption: &str, warn: bool| StripFigure {
            value: value.into(),
            caption: caption.into(),
            warn,
        };
        if warn {
            StripProps {
                marks: vec![Status::Good, Status::Good, Status::Neutral, Status::Warn],
                figures: vec![
                    figure("41 fps", "FPS", false),
                    figure("9.4 / 8 Mbps", "Video rate", true),
                    figure("idle", "Capture to sent", false),
                    figure("38 ms", "Latency", false),
                ],
            }
        } else {
            StripProps {
                marks: vec![Status::Good; 4],
                figures: vec![
                    figure("60 fps", "FPS", false),
                    figure("7.8 / 8 Mbps", "Video rate", false),
                    figure("12 ms", "Capture to sent", false),
                    figure("24 ms", "Latency", false),
                ],
            }
        }
    }

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
            Story::ConnectDefault
            | Story::ConnectBusy
            | Story::ConnectError
            | Story::ConnectDormant
            | Story::ConnectDormantWaiting
            | Story::ConnectDormantTimedOut
            | Story::ConnectHeld
            | Story::ConnectKeyRevoked
            | Story::ConnectKeyExpiredEnv
            | Story::ConnectKeyDisabled
            | Story::ConnectKeyClient
            | Story::ConnectDifferentKey => {
                let props = self.story.connect_props();
                let connect_entity = entity.clone();
                let mut handlers = ConnectHandlers::new(move |_, _, cx| {
                    connect_entity.update(cx, |this, cx| {
                        let len = this.key_input.read(cx).value().trim().len();
                        this.record_event(format!("Connect: Submit ({len} chars)"), cx);
                    });
                });
                let event = |label: &'static str| -> Option<ConnectHandler> {
                    let entity = entity.clone();
                    Some(Rc::new(move |_, _, cx| {
                        entity.update(cx, |this, cx| this.record_event(label, cx));
                    }))
                };
                handlers.on_sign_in_web = event("Recovery: Sign In on the Web");
                handlers.on_try_again = event("Recovery: Try Again");
                handlers.on_other_key = event("Recovery: Use a Different Key");
                handlers.on_open_web = event("Recovery: Open SealSkin in Browser");
                connect::render(theme, &props, &self.key_input, handlers)
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
            | Story::LibraryLaunchDialogHidden
            | Story::LibraryOpenLink
            | Story::LibraryOpenLinkPending
            | Story::LibraryOpenLinkNotice
            | Story::LibraryOpenLinkNone
            | Story::LibraryOpenLinkRejected
            | Story::LibraryOpenFile
            | Story::LibraryOpenFileFallback
            | Story::LibraryDropTarget
            | Story::LibraryDropOnSession
            | Story::LibraryDropFolders
            | Story::LibraryUploading
            | Story::LibraryUploadFailed
            | Story::LaunchDialogOpenFile
            | Story::LaunchDialogOpenLink
            | Story::LibraryAccountUser
            | Story::LibraryAccountMeasuring
            | Story::LibraryAccountLimits
            | Story::LibraryAccountAdmin
            | Story::LibraryAccountFallback => {
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
            | Story::ControlsRestoredSections
            | Story::ControlsShortcutsActive
            | Story::ControlsShortcutsOff
            | Story::ControlsShortcutsLocked
            | Story::ControlsStatsStripOn
            | Story::ControlsStatsWaiting
            | Story::ControlsSecondScreen
            | Story::ControlsSecondScreenOpen
            | Story::ControlsInputCapture
            | Story::ControlsInputCaptureOn
            | Story::ControlsInputCaptureUnavailable
            | Story::GamepadsEmpty
            | Story::GamepadsSending
            | Story::GamepadsReady
            | Story::GamepadsFull
            | Story::GamepadsInputOff
            | Story::GamepadsServerOff
            | Story::ControlsPrintingEmpty
            | Story::ControlsPrintingJobs
            | Story::ControlsPrintingOff
            | Story::ControlsApps
            | Story::AudioDefault
            | Story::AudioChosen
            | Story::AudioMissing
            | Story::AudioNoDevice
            | Story::AudioRetrying
            | Story::AudioBitrateLocked
            | Story::AudioBitrateHidden
            | Story::AudioOff => {
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
            Story::AppsLoading
            | Story::AppsBrowsing
            | Story::AppsInstalled
            | Story::AppsInstalling
            | Story::AppsFailed
            | Story::AppsSearchEmpty
            | Story::AppsCatalogError
            | Story::AppsOffline => {
                let entity = entity.clone();
                let handler: apps_ui::AppsHandler =
                    Rc::new(move |action: AppsAction, _, cx: &mut App| {
                        if matches!(action, AppsAction::IconsVisible(_)) {
                            return;
                        }
                        entity.update(cx, |this, cx| {
                            if let AppsAction::DismissNotice = action {
                                this.apps_props.notice = None;
                            }
                            this.record_event(format!("Apps: {action:?}"), cx);
                            cx.notify();
                        });
                    });
                let (w, h) = if self.story == Story::AppsOffline {
                    (720., 460.)
                } else {
                    (980., 640.)
                };
                div()
                    .size_full()
                    .bg(theme.secondary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_6()
                    .child(
                        div()
                            .id("apps-preview-frame")
                            .w(px(w))
                            .h(px(h))
                            .flex_none()
                            .rounded_xl()
                            .overflow_hidden()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .text_color(theme.foreground)
                            .text_sm()
                            .child(apps_ui::render(
                                theme,
                                &self.apps_props,
                                &self.apps_search,
                                &self.apps_grid,
                                &self.apps_focus,
                                &self.apps_console,
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
                    .child(session::waiting_overlay(""))
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
            Story::SessionStartupStalled => {
                let (keep, retry, disconnect) = (entity.clone(), entity.clone(), entity);
                div()
                    .size_full()
                    .relative()
                    .bg(rgb(0x111318))
                    .child(session::startup_banner(
                        theme,
                        "Firefox hasn’t answered yet. It may still be starting.",
                        move |_, _, cx| {
                            keep.update(cx, |this, cx| {
                                this.record_event("Session: Keep Waiting", cx)
                            });
                        },
                        move |_, _, cx| {
                            retry.update(cx, |this, cx| this.record_event("Session: Retry", cx));
                        },
                        move |_, _, cx| {
                            disconnect.update(cx, |this, cx| {
                                this.record_event("Session: Disconnect", cx)
                            });
                        },
                    ))
            }
            Story::SessionKeepWaiting => div()
                .size_full()
                .relative()
                .bg(rgb(0x111318))
                .child(session::waiting_overlay("Waiting for Firefox to start…")),
            Story::SessionStatsStrip
            | Story::SessionStatsStripWarn
            | Story::SessionStatsStripLight
            | Story::SessionStatsStripWithBanner
            | Story::SessionStatsStripFullScreen => {
                let (width, height) = match self.story {
                    Story::SessionStatsStripFullScreen => (1512., 982.),
                    _ => (1280., 800.),
                };
                let strip = Self::fixture_strip(self.story == Story::SessionStatsStripWarn);
                let mut frame = Self::session_frame(width, height, &strip);
                if self.story == Story::SessionStatsStripWithBanner {
                    frame = frame
                        .child(session::controls_trigger(
                            point(px(width - 40.), px(8.)),
                            px(32.),
                            move |_, _, cx| {
                                entity.update(cx, |this, cx| {
                                    this.record_event("Session: Open Controls Trigger", cx);
                                });
                            },
                        ))
                        .child(session::status_banner(
                            theme,
                            "Reconnecting to Selkies stream (attempt 2)…",
                        ));
                }
                Self::strip_stage(frame)
            }
            Story::SessionCaptureHint
            | Story::SessionReleaseHint
            | Story::SessionCaptureHintWithStrip
            | Story::SessionCaptureHintWithBanner
            | Story::SessionCaptureHintLight => {
                let strip = Self::fixture_strip(false);
                let strip = if self.story == Story::SessionCaptureHintWithStrip {
                    strip
                } else {
                    stats::StripProps {
                        marks: Vec::new(),
                        figures: Vec::new(),
                    }
                };
                let hint = match self.story {
                    Story::SessionReleaseHint => "released",
                    Story::SessionCaptureHintWithBanner => "unavailable",
                    _ => "captured",
                };
                let banner = self.story == Story::SessionCaptureHintWithBanner;
                let mut frame = Self::session_frame(1280., 800., &strip);
                if self.story == Story::SessionCaptureHintWithStrip {
                    frame = frame.child(session::controls_trigger(
                        point(px(1240.), px(8.)),
                        px(32.),
                        |_, _, _| {},
                    ));
                }
                if banner {
                    frame = frame.child(session::startup_banner(
                        theme,
                        "Firefox hasn’t answered yet. It may still be starting.",
                        |_, _, _| {},
                        |_, _, _| {},
                        |_, _, _| {},
                    ));
                }
                Self::strip_stage(frame.child(Self::capture_hint_overlay(hint, 1280., banner)))
            }
            Story::SessionPlacementAllSides | Story::SessionPlacementTwoSides => {
                let all = self.story == Story::SessionPlacementAllSides;
                let (width, height) = if all { (1280., 800.) } else { (800., 500.) };
                let strip = stats::StripProps {
                    marks: Vec::new(),
                    figures: Vec::new(),
                };
                let sides = if all {
                    [true; 4]
                } else {
                    [true, false, true, false]
                };
                let picker = session::screen_placement(
                    theme,
                    sides,
                    all.then_some(1),
                    Rc::new(move |side, _, cx| {
                        entity.update(cx, |this, cx| {
                            this.record_event(format!("Display 2: Pick side {side}"), cx)
                        });
                    }),
                    Rc::new(|_, _, _| {}),
                    |_, _, _| {},
                );
                Self::strip_stage(Self::session_frame(width, height, &strip).child(picker))
            }
            Story::SecondDisplayWaiting | Story::SecondDisplayRefused => {
                // Display 2 shows no strip and no notices, whatever the session has.
                let frame = div()
                    .relative()
                    .flex_none()
                    .w(px(1280.))
                    .h(px(800.))
                    .bg(rgb(0x000000))
                    .child(session::controls_trigger(
                        point(px(1240.), px(8.)),
                        px(32.),
                        |_, _, _| {},
                    ));
                let frame = if self.story == Story::SecondDisplayWaiting {
                    frame.child(session::waiting_overlay("Waiting for the main display…"))
                } else {
                    frame.child(session::status_banner_labeled(
                        theme,
                        "Display 2 isn’t available. The host compositor has a single output, so a second display has nothing to capture.",
                        ("Try Again", Box::new(|_, _, _| {})),
                        ("Close", Box::new(|_, _, _| {})),
                    ))
                };
                Self::strip_stage(frame)
            }
            Story::SessionSupersededPill => {
                let strip = stats::StripProps {
                    marks: Vec::new(),
                    figures: Vec::new(),
                };
                Self::strip_stage(
                    Self::session_frame(1280., 800., &strip)
                        .child(session::status_banner_with_actions(
                            theme,
                            "This session was opened somewhere else.",
                            |_, _, _| {},
                            |_, _, _| {},
                        ))
                        .child(session::notice_pill(
                            "This session doesn’t allow a second display.",
                            session::capture_hint_bottom(true),
                            px(1280.),
                            session::HintFade::None,
                            0,
                        )),
                )
            }
            Story::SessionCaptureHintNarrow => {
                let strip = stats::StripProps {
                    marks: Vec::new(),
                    figures: Vec::new(),
                };
                Self::strip_stage(
                    Self::session_frame(360., 400., &strip)
                        .child(Self::capture_hint_overlay("captured", 360., false)),
                )
            }
            Story::PrintNoticeStates => {
                // Every state, three frames side by side; each frame shows its notices whole.
                let all = Self::fixture_print_notices();
                let frame = |notices: &[PrintNoticeProps], entity: Entity<Self>| {
                    let stack = session::print_notices(
                        theme,
                        notices,
                        0,
                        px(380.),
                        px(48.),
                        Rc::new(move |id, action, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.record_event(format!("Print: {action:?} #{id}"), cx)
                            });
                        }),
                        Rc::new(|_, _, _, _| {}),
                    );
                    div()
                        .relative()
                        .flex_none()
                        .w(px(380.))
                        .h(px(620.))
                        .rounded_lg()
                        .bg(rgb(0x1b2430))
                        .child(stack.left(px(12.)).w_auto())
                };
                Self::strip_stage(
                    div()
                        .h_flex()
                        .items_start()
                        .gap_4()
                        .child(frame(&all[..5], entity.clone()))
                        .child(frame(&all[5..9], entity.clone()))
                        .child(frame(&all[9..], entity)),
                )
            }
            Story::PrintNoticeLongName => {
                let notices = vec![PrintNoticeProps {
                    id: 1,
                    name:
                        "Quarterly Financial Statement for the Board of Directors – Final (3).pdf"
                            .into(),
                    kind: PrintNoticeKind::Ready {
                        detail: "2.2 MB · From Firefox".into(),
                        saved: false,
                    },
                }];
                Self::strip_stage(Self::print_frame(
                    theme, 1280., 800., &notices, None, entity,
                ))
            }
            Story::PrintNoticeStack => {
                let all = Self::fixture_print_notices();
                let notices = [
                    all[2].clone(),
                    PrintNoticeProps {
                        id: 20,
                        name: "Slides.pdf".into(),
                        ..all[2].clone()
                    },
                    all[0].clone(),
                    all[7].clone(),
                    all[12].clone(),
                ];
                Self::strip_stage(Self::print_frame(
                    theme, 1280., 800., &notices, None, entity,
                ))
            }
            Story::PrintNoticeWithStrip => {
                let strip = Self::fixture_strip(false);
                let all = Self::fixture_print_notices();
                let notices = [all[2].clone(), all[0].clone()];
                Self::strip_stage(Self::print_frame(
                    theme,
                    1280.,
                    800.,
                    &notices,
                    Some(&strip),
                    entity,
                ))
            }
            Story::PrintNoticeNarrow => {
                let strip = Self::fixture_strip(false);
                let all = Self::fixture_print_notices();
                let notices = [all[2].clone(), all[0].clone(), all[7].clone()];
                Self::strip_stage(Self::print_frame(
                    theme,
                    400.,
                    360.,
                    &notices,
                    Some(&strip),
                    entity,
                ))
            }
            Story::SessionStatsStripNarrow => {
                let strip = Self::fixture_strip(false);
                Self::strip_stage(
                    div()
                        .h_flex()
                        .items_start()
                        .gap_4()
                        .child(Self::session_frame(640., 400., &strip))
                        .child(Self::session_frame(480., 400., &strip)),
                )
            }
        }
    }

    /// Session frames larger than the stage scroll inside it.
    fn strip_stage(content: impl IntoElement) -> gpui_kit::Div {
        div().size_full().child(
            div()
                .id("session-strip-stage")
                .size_full()
                .overflow_scroll()
                .p_4()
                .child(content),
        )
    }

    /// A session window `width` × `height` pt over a picture-like gradient (a light top
    /// band, to stress the strip's contrast), with the strip over it.
    fn session_frame(width: f32, height: f32, strip: &stats::StripProps) -> gpui_kit::Div {
        div()
            .relative()
            .flex_none()
            .w(px(width))
            .h(px(height))
            .overflow_hidden()
            .bg(gpui_kit::linear_gradient(
                180.,
                gpui_kit::linear_color_stop(rgb(0xe9eef4), 0.),
                gpui_kit::linear_color_stop(rgb(0x1b2430), 0.55),
            ))
            .child(stats::strip(strip, px(width)))
    }

    fn library_handlers(&self, cx: &mut Context<Self>) -> LibraryHandlers {
        let on_refresh_entity = cx.entity().clone();
        let on_sign_out_entity = cx.entity().clone();
        let on_account_entity = cx.entity().clone();
        let on_web_entity = cx.entity().clone();
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
                    this.record_event("Account: Sign Out…", cx);
                });
            }),
            on_account_open: Rc::new(move |open, _, cx| {
                on_account_entity.update(cx, |this, cx| {
                    this.account_open = open;
                    this.record_event(
                        if open {
                            "Account: Open"
                        } else {
                            "Account: Close"
                        },
                        cx,
                    );
                });
            }),
            on_open_web: Box::new(move |_, _, cx| {
                on_web_entity.update(cx, |this, cx| {
                    this.record_event("Account: Open SealSkin in Browser", cx);
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
            open: self.open_handlers(cx),
        }
    }

    /// The chooser, drop and Files handlers only record what they would do.
    fn open_handlers(&self, cx: &mut Context<Self>) -> OpenHandlers {
        let entity = cx.entity();
        let record = |label: &'static str| {
            let entity = entity.clone();
            Rc::new(move |id: String, _: u64, _: &mut Window, cx: &mut App| {
                entity.update(cx, |this, cx| {
                    this.record_event(format!("{label}: {id}"), cx)
                });
            }) as library::OpeningHandler
        };
        let action = |label: &'static str| {
            let entity = entity.clone();
            Rc::new(move |_: &mut Window, cx: &mut App| {
                entity.update(cx, |this, cx| this.record_event(label, cx));
            }) as library::ActionHandler
        };
        let files_entity = entity.clone();
        let drop_entity = entity.clone();
        let session_drop_entity = entity.clone();
        OpenHandlers {
            on_open_in_session: Some(record("Chooser: Running row")),
            on_open_with_app: Some(record("Chooser: Launch & Open")),
            on_clear_opening: Some(action("Chooser: Cancel")),
            on_skip_opening: Some(action("Chooser: Skip")),
            on_toggle_opening_apps: Some(action("Chooser: Show All")),
            on_files: Some(Rc::new(move |id, _, cx| {
                files_entity.update(cx, |this, cx| this.record_event(format!("Files: {id}"), cx));
            })),
            on_drop: Some(Rc::new(move |paths, _, cx| {
                drop_entity.update(cx, |this, cx| {
                    this.record_event(format!("Drop on Library: {} items", paths.len()), cx)
                });
            })),
            on_drop_to_session: Some(Rc::new(move |id, paths, _, cx| {
                session_drop_entity.update(cx, |this, cx| {
                    this.record_event(format!("Drop on {id}: {} items", paths.len()), cx)
                });
            })),
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
