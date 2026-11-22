//! The session's Files window (`ARCH-023`): the Selkies file-manager folder as a Finder-style
//! list, with uploads from Finder drops, ⌘V, and Upload…, and downloads to `~/Downloads`, a
//! chosen folder, the Mac clipboard (⌘C), Open, Quick Look, and drags to Finder.
//!
//! The window owns its transfers: they run on Tokio, report bytes through shared counters the
//! window redraws from, and are cancelled when the window closes or the session ends.

use super::files_backend::{BackendError, FilesBackend};
use super::files_model::{size_label, Browser};
use super::promise;
use super::session_window::{OpenFiles, OpenWindows, SessionWindow};

use crate::pasteboard;
use crate::selkies::{self, Entry, Policy, Progress, SelkiesClient};
use crate::storage::{self, StoragePlace, UploadTarget};
use gpui_kit::component::{
    input::{InputEvent, InputState},
    menu::ContextMenuExt as _,
    ActiveTheme as _,
};
use gpui_kit::{
    actions, div, point, prelude::*, px, size, AnyWindowHandle, App, Bounds, Context, Entity,
    ExternalPaths, FocusHandle, PathPromptOptions, ScrollStrategy, Task, TitlebarOptions,
    UniformListScrollHandle, WeakEntity, Window, WindowBounds, WindowOptions,
};
use reqwest::StatusCode;
use skerry_ui::files::{
    self as ui, Direction, FilesAction, FilesProps, TransferRow, TransferStatus,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::runtime::Handle;

actions!(
    files_window,
    [
        SelectPrevious,
        SelectNext,
        ExtendPrevious,
        ExtendNext,
        OpenSelection,
        OpenParent,
        GoBack,
        GoForward,
        Refresh,
        NewFolder,
        QuickLook,
        CopyToMac,
        PasteFromMac,
        SelectAll,
        DeleteSelection,
        DownloadSelection,
        DownloadSelectionTo,
        UploadFiles,
        CloseFiles,
        Cancel,
        /// Open in… on a top-level Shared Files file (`ARCH-037`).
        OpenIn,
        /// ⌘F: the Storage window's Filter field.
        FocusFilter
    ]
);

/// The list's keys apply unless a text field inside it has focus: the New Folder field renders
/// inside the `FilesList` element, and GPUI matches a binding at any depth of the context stack,
/// so a plain `FilesList` would take Space, ⌘O, and ⌘Y from the field (`!Input` looks at the
/// whole stack).
const LIST_CONTEXT: &str = "FilesList && !Input";

/// Keys of the Files window: `FilesWindow` everywhere in it, `FilesList` while the list (not the
/// New Folder field) has focus, so Space and the arrows type into the field.
pub(super) fn key_bindings() -> Vec<gpui_kit::KeyBinding> {
    use gpui_kit::KeyBinding;
    let list = Some(LIST_CONTEXT);
    let window = Some("FilesWindow");
    vec![
        KeyBinding::new("up", SelectPrevious, list),
        KeyBinding::new("down", SelectNext, list),
        KeyBinding::new("shift-up", ExtendPrevious, list),
        KeyBinding::new("shift-down", ExtendNext, list),
        KeyBinding::new("enter", OpenSelection, list),
        KeyBinding::new("cmd-down", OpenSelection, list),
        KeyBinding::new("cmd-o", OpenSelection, list),
        KeyBinding::new("cmd-up", OpenParent, list),
        KeyBinding::new("space", QuickLook, list),
        KeyBinding::new("cmd-y", QuickLook, list),
        KeyBinding::new("cmd-c", CopyToMac, list),
        KeyBinding::new("cmd-v", PasteFromMac, list),
        KeyBinding::new("cmd-a", SelectAll, list),
        KeyBinding::new("cmd-backspace", DeleteSelection, list),
        KeyBinding::new("escape", Cancel, window),
        KeyBinding::new("cmd-[", GoBack, window),
        KeyBinding::new("cmd-]", GoForward, window),
        KeyBinding::new("cmd-r", Refresh, window),
        KeyBinding::new("cmd-shift-n", NewFolder, window),
        KeyBinding::new("cmd-shift-u", UploadFiles, window),
        KeyBinding::new("cmd-w", CloseFiles, window),
        KeyBinding::new("alt-cmd-o", OpenIn, window),
        KeyBinding::new("cmd-f", FocusFilter, window),
    ]
}

/// How far, in points, the mouse moves with the button down before a row drag starts.
const DRAG_THRESHOLD: f32 = 4.;

/// What happens once a download lands.
enum After {
    /// Offer Show in Finder.
    Reveal,
    Open,
    QuickLook,
    /// Put the files on the Mac clipboard.
    Clipboard,
    /// Tell Finder the promised file is written (`promise.rs`).
    Promise(promise::Completion),
    /// An upload: refresh the folder if it is still shown, in the same home.
    Uploaded {
        home: Option<String>,
        folder: String,
    },
}

/// What the Storage window's pane tells its window (`ARCH-037`).
#[derive(Clone, Debug)]
pub(super) enum PaneEvent {
    /// Open in… on this top-level Shared Files file.
    OpenIn(Entry),
    /// The pane reached a home's `Desktop/files`: show Shared Files.
    GoShared,
    /// Files bound for a home's `Desktop/files`: upload them to Shared Files instead.
    UploadToShared(Vec<PathBuf>),
    /// An upload, folder or delete finished: the quota may have moved.
    Changed,
    /// ⌘R or Refresh: measure again too.
    Refreshed,
}

impl gpui_kit::EventEmitter<PaneEvent> for FilesWindow {}

enum State {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

/// Bytes moved and expected, shared with the job.
#[derive(Clone, Default)]
struct Meter {
    done: Progress,
    /// `u64::MAX` until the job has measured what it moves.
    total: Arc<AtomicU64>,
    /// The partial file a download is writing, removed if it is cancelled.
    partial: Arc<Mutex<Option<PathBuf>>>,
    /// What the finished transfer row says instead of plain Uploaded / Downloaded.
    note: Arc<Mutex<Option<String>>>,
    /// The transfer's own route answered 403: the server doesn't allow that direction.
    forbidden: Arc<AtomicBool>,
}

struct Transfer {
    id: u64,
    name: String,
    direction: Direction,
    meter: Meter,
    state: State,
    job: Option<tokio::task::AbortHandle>,
    /// The files a finished download made.
    made: Vec<PathBuf>,
    after: Option<After>,
    /// The storage home it moves to or from, for Delete Home (`ARCH-037`).
    home: Option<String>,
}

pub(super) struct FilesWindow {
    backend: FilesBackend,
    /// The Storage window's Filter field (`ARCH-037`); `None` in a session's window.
    filter: Option<Entity<InputState>>,
    /// The next render empties the Filter field (it needs a Window).
    clear_filter: bool,
    /// The folder holds more than the listing shows.
    truncated: bool,
    policy: Policy,
    rt: Handle,
    /// The session's home in SealSkin's persistent storage, which can delete (`ARCH-024`).
    storage: Option<Storage>,
    /// The home's `Desktop` was seen to be the folder Selkies serves, so deleting there is safe.
    storage_matches: bool,
    root_label: String,
    list_focus: FocusHandle,
    scroll: UniformListScrollHandle,
    browser: Browser,
    loading: bool,
    error: Option<String>,
    notice: Option<ui::Notice>,
    /// Bumped by every notice, so an information notice fades only if nothing replaced it.
    notice_generation: u64,
    /// Work shown in the status bar with a spinner.
    busy: Option<String>,
    transfers_open: bool,
    listing: Option<Task<()>>,
    generation: u64,
    transfers: Vec<Transfer>,
    next_id: u64,
    ticker: Option<Task<()>>,
    folder_name: Entity<InputState>,
    creating_folder: bool,
    /// The row the mouse went down on, and whether a drag out has started from it.
    pressed: Option<usize>,
    /// Where the mouse went down, so a drag starts only after it really moves.
    press_at: Option<gpui_kit::Point<gpui_kit::Pixels>>,
    dragging: bool,
    /// The list scrolls to the selection after a keyboard move only, not on every redraw.
    reveal_selection: bool,
    /// Who opened the window, told when it closes.
    owner: FilesOwner,
    /// This window, to leave the app-wide registry when it closes.
    handle: Option<AnyWindowHandle>,
}

/// Who opened a Files window (`ARCH-028`): a session window (it closes with that stream), or
/// the Library's Files button (no stream; it closes on Stop, Sign Out, or ⌘W).
#[derive(Clone)]
pub(super) enum FilesOwner {
    Session(AnyWindowHandle, WeakEntity<SessionWindow>),
    Library,
    /// The Storage window's own pane: never in `OpenWindows.files`, closed with its window
    /// (`ARCH-037`).
    Storage,
}

/// Drops every hold on the Files window, so the window's entity (and with it every transfer,
/// `Drop`) goes once the window does: the session window's, if it owns it, and the app-wide
/// registry entry. Needed for every way the window closes: `remove_window` (⌘W, session end)
/// does not run `on_window_should_close`.
fn release(owner: &FilesOwner, handle: Option<AnyWindowHandle>, cx: &mut App) {
    let owner = owner.clone();
    cx.defer(move |cx| {
        if let Some(handle) = handle {
            if cx.has_global::<OpenWindows>() {
                cx.global_mut::<OpenWindows>().forget_files(handle);
            }
        }
        if let FilesOwner::Session(session_window, session) = owner {
            let _ = session_window.update(cx, |_, _, cx| {
                let _ = session.update(cx, |session, _| session.files_window = None);
            });
        }
    });
}

/// Which Selkies route a 403 came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Route {
    Upload,
    Download,
    Browse,
    Mkdir,
}

/// The policy after `route` answered `status`. Without the stream's `server_settings`, a
/// Library-opened window learns the policy from 403s, but only from the direction's own
/// route: a mkdir or browse 403 says nothing about uploads (`ARCH-028`).
pub(super) fn policy_after(policy: Policy, route: Route, status: StatusCode) -> Policy {
    if status != StatusCode::FORBIDDEN {
        return policy;
    }
    match route {
        Route::Upload => Policy {
            upload: false,
            ..policy
        },
        Route::Download => Policy {
            download: false,
            ..policy
        },
        Route::Browse | Route::Mkdir => policy,
    }
}

impl Drop for FilesWindow {
    fn drop(&mut self) {
        for transfer in &mut self.transfers {
            if let Some(job) = transfer.job.take() {
                job.abort();
            }
            if let Some(partial) = transfer.meter.partial.lock().ok().and_then(|p| p.clone()) {
                let _ = std::fs::remove_file(partial);
            }
            if let Some(After::Promise(completion)) = transfer.after.take() {
                completion.finish(Some("The Files window closed."));
            }
        }
    }
}

impl FilesWindow {
    /// Opens a Files window placed over `window` (the session window or the Library) and
    /// registers it app-wide, so there is one per session whoever opened it (`ARCH-028`).
    pub(super) fn open(
        source: FilesSource,
        rt: Handle,
        window: &Window,
        owner: FilesOwner,
        session_id: String,
        cx: &mut App,
    ) -> gpui_kit::Result<(AnyWindowHandle, Entity<Self>)> {
        let FilesSource {
            title,
            selkies: access,
            policy,
            storage,
        } = source;
        let library_owned = matches!(owner, FilesOwner::Library);
        let bounds = window.bounds();
        let window_size = size(px(760.), px(520.));
        let origin = point(bounds.left() + px(40.), bounds.top() + px(60.));
        let created: Rc<std::cell::RefCell<Option<Entity<Self>>>> = Rc::default();
        let slot = Rc::clone(&created);
        let (handle, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, window_size))),
                window_min_size: Some(size(px(460.), px(280.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("{title} — Files").into()),
                    ..Default::default()
                }),
                display_id: window.display(cx).map(|display| display.id()),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                let close_owner = owner.clone();
                let own_handle = window.window_handle();
                let view = cx.new(|cx: &mut Context<Self>| {
                    let mut this = Self::build(
                        FilesBackend::Selkies(access),
                        policy,
                        storage,
                        "Desktop".into(),
                        rt,
                        owner,
                        Some(own_handle),
                        window,
                        cx,
                    );
                    this.reload(cx);
                    this.check_storage(cx);
                    this
                });
                window.on_window_should_close(cx, move |_, cx| {
                    release(&close_owner, Some(own_handle), cx);
                    true
                });
                let focus = view.read(cx).list_focus.clone();
                window.focus(&focus, cx);
                *slot.borrow_mut() = Some(view.clone());
                view
            },
        )?;
        let view = created
            .borrow_mut()
            .take()
            .ok_or_else(|| std::io::Error::other("the Files window has no view"))?;
        cx.default_global::<OpenWindows>().files.push(OpenFiles {
            session_id,
            window: handle,
            entity: view.downgrade(),
            library_owned,
        });
        Ok((handle, view))
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        backend: FilesBackend,
        policy: Policy,
        storage: Option<Storage>,
        root_label: String,
        rt: Handle,
        owner: FilesOwner,
        handle: Option<AnyWindowHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let folder_name = cx.new(|cx| InputState::new(window, cx).placeholder("untitled folder"));
        cx.subscribe_in(
            &folder_name,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.create_folder(window, cx);
                }
            },
        )
        .detach();
        let filter = backend.storage().map(|_| {
            let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
            cx.subscribe_in(&filter, window, |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = input.read(cx).value().to_string();
                    this.browser.set_filter(&text);
                    cx.notify();
                }
            })
            .detach();
            filter
        });
        // Selection turns grey when the window is not key, as in Finder.
        cx.observe_window_activation(window, |_, _, cx| cx.notify())
            .detach();
        Self {
            backend,
            filter,
            clear_filter: false,
            truncated: false,
            policy,
            rt,
            storage,
            storage_matches: false,
            root_label,
            list_focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            browser: Browser::default(),
            loading: false,
            error: None,
            notice: None,
            notice_generation: 0,
            busy: None,
            transfers_open: true,
            listing: None,
            generation: 0,
            transfers: Vec::new(),
            next_id: 1,
            ticker: None,
            folder_name,
            creating_folder: false,
            pressed: None,
            press_at: None,
            dragging: false,
            reveal_selection: false,
            owner,
            handle,
        }
    }

    /// The Storage window's pane (`ARCH-037`): the same list, built inside that window and
    /// never registered in `OpenWindows.files`, so nothing that looks Files windows up by
    /// session ever finds it.
    pub(super) fn pane(
        backend: FilesBackend,
        rt: Handle,
        owner: FilesOwner,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let root_label = backend.place().map(StoragePlace::label).unwrap_or_default();
        cx.new(|cx| {
            // Storage has no per-direction policy: the server's sentence answers a refusal.
            let mut this = Self::build(
                backend,
                Policy::default(),
                None,
                root_label,
                rt,
                owner,
                None,
                window,
                cx,
            );
            this.reload(cx);
            this
        })
    }

    /// Shows another home (or Shared Files): back to its root with no history. Running
    /// transfers keep their own backend and go on.
    pub(super) fn set_backend(&mut self, backend: FilesBackend, cx: &mut Context<Self>) {
        self.root_label = backend.place().map(StoragePlace::label).unwrap_or_default();
        self.backend = backend;
        self.browser.reset();
        self.clear_filter = true;
        self.truncated = false;
        self.error = None;
        self.notice = None;
        self.creating_folder = false;
        self.reload(cx);
    }

    /// The storage homes running transfers move to or from (`ARCH-037`).
    pub(super) fn busy_homes(&self) -> HashSet<String> {
        self.transfers
            .iter()
            .filter(|t| matches!(t.state, State::Running))
            .filter_map(|t| t.home.clone())
            .collect()
    }

    /// The list's focus, for the Storage window to focus at open.
    pub(super) fn list_focus(&self) -> FocusHandle {
        self.list_focus.clone()
    }

    /// Shows a notice in the pane, for the Storage window's own errors and information.
    pub(super) fn notify(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.say(text, error, cx);
    }

    /// Reloads the folder shown (window activation, ⌘R from the sidebar).
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
    }

    /// The selected entry Open in… would take, if exactly one top-level Shared Files file.
    fn open_in_entry(&self) -> Option<Entry> {
        let place = self.backend.place()?;
        match self.browser.selected_entries().as_slice() {
            [one] if storage::can_open_in(place, self.browser.path(), one.is_dir) => {
                Some((*one).clone())
            }
            _ => None,
        }
    }

    fn open_in(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.open_in_entry() {
            cx.emit(PaneEvent::OpenIn(entry));
        }
    }

    /// The session's transfer policy changed while the window is open.
    pub(super) fn set_policy(&mut self, policy: Policy, cx: &mut Context<Self>) {
        if self.policy != policy {
            let listable = policy.download && !self.policy.download;
            self.policy = policy;
            if listable {
                self.reload(cx);
            }
            cx.notify();
        }
    }

    /// Uploads Mac files into the session's Desktop (the root), as a drop there would: the
    /// Library's Copy to Desktop and a drop on a session card (`ARCH-028`).
    pub(super) fn enqueue_upload(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.upload_into(paths, String::new(), cx);
    }

    /// What a Selkies route's error says about the policy (for browse and mkdir: nothing).
    /// Storage errors never change it (`ARCH-037`).
    fn learn_route(&mut self, route: Route, error: &BackendError) {
        if let BackendError::Selkies(selkies::Error::Server(status, _)) = error {
            self.policy = policy_after(self.policy, route, *status);
        }
    }

    /// The direction a transfer's 403 refused goes off for this window's lifetime.
    fn learn_forbidden(&mut self, direction: Direction, cx: &mut Context<Self>) {
        let (route, text) = match direction {
            Direction::Upload => (Route::Upload, "This session doesn’t allow uploads."),
            Direction::Download => (Route::Download, "This session doesn’t allow downloads."),
        };
        self.learn_route(
            route,
            &BackendError::Selkies(selkies::Error::Server(StatusCode::FORBIDDEN, String::new())),
        );
        self.say(text, true, cx);
    }

    // ---- Listing -------------------------------------------------------------------------

    fn reload(&mut self, cx: &mut Context<Self>) {
        if !self.policy.download {
            self.error = Some("The server doesn’t allow listing or downloading files.".into());
            self.loading = false;
            cx.notify();
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        let backend = self.backend.clone();
        let path = self.browser.path().to_owned();
        let pending = self.rt.spawn(async move { backend.browse(&path).await });
        self.listing = Some(cx.spawn(async move |this, cx| {
            let result = pending.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(Ok(listing)) => {
                        this.error = None;
                        if listing.truncated && !this.truncated {
                            this.say("Showing the first 10,000 items.", false, cx);
                        }
                        this.truncated = listing.truncated;
                        this.browser.set_listing(listing.entries);
                    }
                    Ok(Err(error)) => {
                        this.learn_route(Route::Browse, &error);
                        this.error = Some(error.to_string());
                    }
                    Err(_) => this.error = Some("The listing was interrupted.".into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn after_move(&mut self, moved: bool, cx: &mut Context<Self>) {
        if moved {
            // A home's `Desktop/files` is the Shared Files mount point: show Shared Files.
            if self
                .backend
                .place()
                .is_some_and(|place| storage::opens_shared_files(place, self.browser.path()))
            {
                cx.emit(PaneEvent::GoShared);
                return;
            }
            self.clear_filter = self.filter.is_some();
            self.truncated = false;
            self.error = None;
            self.creating_folder = false;
            if self.notice.as_ref().is_some_and(|notice| !notice.error) {
                self.notice = None;
            }
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            self.reload(cx);
        }
    }

    fn open_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected: Vec<Entry> = self
            .browser
            .selected_entries()
            .into_iter()
            .cloned()
            .collect();
        if let [single] = selected.as_slice() {
            if single.is_dir {
                let moved = self.browser.navigate(&single.path);
                self.after_move(moved, cx);
                return;
            }
        }
        let files: Vec<Entry> = selected.into_iter().filter(|e| !e.is_dir).collect();
        if !files.is_empty() {
            self.download(files, scratch_dir("Open"), After::Open, window, cx);
        }
    }

    // ---- Actions -------------------------------------------------------------------------

    fn handle(&mut self, action: FilesAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            FilesAction::Back => {
                let moved = self.browser.back();
                self.after_move(moved, cx);
            }
            FilesAction::Forward => {
                let moved = self.browser.forward();
                self.after_move(moved, cx);
            }
            FilesAction::Navigate(path) => {
                let moved = self.browser.navigate(&path);
                self.after_move(moved, cx);
            }
            FilesAction::Refresh => {
                self.reload(cx);
                self.check_storage(cx);
                cx.emit(PaneEvent::Refreshed);
            }
            FilesAction::OpenIn => self.open_in(cx),
            FilesAction::Sort(column) => self.browser.sort_by(column),
            FilesAction::Select {
                index,
                extend,
                toggle,
            } => {
                self.browser.select(index, extend, toggle);
                window.focus(&self.list_focus, cx);
            }
            FilesAction::Open(index) => {
                self.browser.select(index, false, false);
                self.open_selection(window, cx);
            }
            FilesAction::ClearSelection => {
                self.pressed = None;
                self.browser.clear_selection();
                window.focus(&self.list_focus, cx);
            }
            FilesAction::ContextRow(index) => {
                let selected = self.browser.entries().get(index).is_some_and(|row| {
                    self.browser
                        .selected_entries()
                        .iter()
                        .any(|e| e.name == row.name)
                });
                if !selected {
                    self.browser.select(index, false, false);
                }
                window.focus(&self.list_focus, cx);
            }
            FilesAction::Press(index) => {
                self.pressed = Some(index);
                self.press_at = Some(window.mouse_position());
                self.dragging = false;
            }
            FilesAction::DragOut(_) => self.drag_out(window, cx),
            FilesAction::NewFolder => self.start_new_folder(window, cx),
            FilesAction::ConfirmNewFolder => self.create_folder(window, cx),
            FilesAction::CancelNewFolder => {
                self.creating_folder = false;
                window.focus(&self.list_focus, cx);
            }
            FilesAction::Upload => self.choose_uploads(window, cx),
            FilesAction::Download => {
                let entries = self.selected();
                if !entries.is_empty() {
                    self.download(entries, downloads_dir(), After::Reveal, window, cx);
                }
            }
            FilesAction::DownloadTo => self.choose_download_folder(window, cx),
            FilesAction::QuickLook => self.quick_look(window, cx),
            FilesAction::CancelTransfer(id) => self.cancel(id),
            FilesAction::RevealTransfer(id) => {
                if let Some(path) = self
                    .transfers
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.made.first())
                {
                    cx.reveal_path(path);
                }
            }
            FilesAction::ClearTransfers => {
                self.transfers.retain(|t| matches!(t.state, State::Running))
            }
            FilesAction::DismissNotice => self.notice = None,
            FilesAction::ToggleTransfers => self.transfers_open = !self.transfers_open,
            FilesAction::DropInto { index, paths } => {
                if let Some(folder) = self
                    .browser
                    .entries()
                    .get(index)
                    .filter(|entry| entry.is_dir)
                    .map(|entry| entry.path.clone())
                {
                    self.upload_into(paths, folder, cx);
                }
            }
            FilesAction::Delete => self.confirm_delete(window, cx),
        }
        cx.notify();
    }

    fn selected(&self) -> Vec<Entry> {
        self.browser
            .selected_entries()
            .into_iter()
            .cloned()
            .collect()
    }

    fn start_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.policy.upload {
            return;
        }
        self.creating_folder = true;
        self.folder_name.update(cx, |state, cx| {
            state.set_value("untitled folder", window, cx)
        });
        self.folder_name.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        cx.notify();
    }

    fn create_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.folder_name.read(cx).value().trim().to_owned();
        if selkies::local_name(&name).is_none() {
            self.say("A folder name can’t be empty or contain “/”.", true, cx);
            cx.notify();
            return;
        }
        if self.browser.entries().iter().any(|e| e.name == name) {
            self.say(format!("“{name}” already exists here."), true, cx);
            cx.notify();
            return;
        }
        self.creating_folder = false;
        window.focus(&self.list_focus, cx);
        if self.backend.place().is_some_and(|place| {
            storage::upload_target(place, self.browser.path()) == UploadTarget::SharedRoot
        }) {
            self.say(SHARED_MOUNT_NOTICE, false, cx);
            return;
        }
        let backend = self.backend.clone();
        let path = selkies::join(self.browser.path(), &name);
        let folder = self.browser.path().to_owned();
        let pending = self.rt.spawn(async move { backend.mkdir(&path).await });
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(())) => cx.emit(PaneEvent::Changed),
                    Ok(Err(error)) => {
                        this.learn_route(Route::Mkdir, &error);
                        this.say(error.to_string(), true, cx);
                    }
                    Err(_) => this.say("The folder couldn’t be created.", true, cx),
                }
                if this.browser.path() == folder {
                    this.reload(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn choose_uploads(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.policy.upload {
            return;
        }
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Upload".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await {
                let _ = this.update(cx, |this, cx| this.upload(paths, cx));
            }
        })
        .detach();
    }

    fn choose_download_folder(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.selected();
        if entries.is_empty() || !self.policy.download {
            return;
        }
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Download Here".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = chosen.await {
                if let Some(folder) = paths.pop() {
                    let _ = this.update_in_files(cx, move |this, window, cx| {
                        this.download(entries, folder, After::Reveal, window, cx)
                    });
                }
            }
        })
        .detach();
    }

    fn quick_look(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files: Vec<Entry> = self.selected().into_iter().filter(|e| !e.is_dir).collect();
        if !files.is_empty() {
            self.download(
                files,
                scratch_dir("Quick Look"),
                After::QuickLook,
                window,
                cx,
            );
        }
    }

    fn copy_to_mac(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.selected();
        if !entries.is_empty() {
            self.download(
                entries,
                scratch_dir("Clipboard"),
                After::Clipboard,
                window,
                cx,
            );
        }
    }

    fn paste_from_mac(&mut self, cx: &mut Context<Self>) {
        let paths = pasteboard::file_paths();
        if paths.is_empty() {
            self.say("There are no files on the clipboard to paste.", true, cx);
            cx.notify();
            return;
        }
        self.upload(paths, cx);
    }

    fn drag_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dragging || !self.policy.download {
            return;
        }
        let Some(pressed) = self.pressed else {
            return;
        };
        // AppKit's drag threshold: a click that wobbles a pixel stays a click.
        let moved = self.press_at.is_some_and(|at| {
            let delta = window.mouse_position() - at;
            f32::from(delta.x).abs() + f32::from(delta.y).abs() >= DRAG_THRESHOLD
        });
        if !moved {
            return;
        }
        let in_selection = self.browser.selected_entries().iter().any(|e| {
            self.browser
                .entries()
                .get(pressed)
                .is_some_and(|p| p.name == e.name)
        });
        if !in_selection {
            self.browser.select(pressed, false, false);
        }
        let entries = self.selected();
        if entries.is_empty() {
            return;
        }
        self.dragging = true;
        let view = cx.entity().downgrade();
        let window_handle = window.window_handle();
        let started = promise::begin_drag(
            window,
            &entries,
            move |entry, folder, completion, cx| {
                // Finder waits for every promise's completion handler, so it is called even
                // when the window (and so the update) is gone.
                let mut completion = Some(completion);
                let _ = window_handle.update(cx, |_, window, cx| {
                    if let Some(view) = view.upgrade() {
                        if let Some(completion) = completion.take() {
                            view.update(cx, |this, cx| {
                                this.download(
                                    vec![entry],
                                    folder,
                                    After::Promise(completion),
                                    window,
                                    cx,
                                );
                            });
                        }
                    }
                });
                if let Some(completion) = completion {
                    completion.finish(Some("The Files window closed."));
                }
            },
            cx,
        );
        if !started {
            self.dragging = false;
        }
        cx.notify();
    }

    // ---- Delete (SealSkin storage, `ARCH-024`) -------------------------------------------

    /// Compares the Selkies root with the home's `Desktop`: Selkies keeps where its folder is
    /// to itself, so Delete is offered only once the two are seen to list the same names.
    fn check_storage(&mut self, cx: &mut Context<Self>) {
        let Some(storage) = self.storage.clone() else {
            return;
        };
        let FilesBackend::Selkies(selkies) = self.backend.clone() else {
            return;
        };
        let pending = self.rt.spawn(async move {
            let ours = selkies
                .browse("")
                .await
                .map_err(|e| crate::sealskin::ApiError::Message(e.to_string()))?;
            let theirs = storage
                .client
                .storage_names(&storage.home, STORAGE_ROOT)
                .await?;
            Ok::<_, crate::sealskin::ApiError>(same_folder(
                ours.entries.iter().map(|e| e.name.as_str()),
                theirs.iter().map(String::as_str),
            ))
        });
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            let matches = matches!(result, Ok(Ok(true)));
            let _ = this.update(cx, |this, cx| {
                this.storage_matches = matches;
                // A refused key shows here; the session and this window stay open. This
                // window keeps the key it opened with: a key replaced in the Library
                // reaches it when the session window opens again (`ARCH-029`).
                if let Ok(Err(error)) = &result {
                    if error.is_refusal() {
                        this.say(error.to_string(), true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Where Delete sends what: the session's home `Desktop` once matched (`ARCH-024`), or
    /// the Storage window's home itself, never its protected folders (`ARCH-037`).
    fn delete_plan(&self) -> Option<(Storage, String, Vec<Entry>, Vec<String>)> {
        let entries = self.selected();
        if let Some(backend) = self.backend.storage() {
            let place = &backend.place;
            let entries: Vec<Entry> = entries
                .into_iter()
                .filter(|entry| !storage::protected(place, &entry.path))
                .collect();
            if entries.is_empty() {
                return None;
            }
            let paths = deletable_paths(place, &entries);
            let target = Storage {
                client: backend.client.clone(),
                home: place.home().to_string(),
            };
            return Some((target, place.full_name(), entries, paths));
        }
        let storage = self.storage.clone().filter(|_| self.storage_matches)?;
        if entries.is_empty() {
            return None;
        }
        let paths = entries.iter().map(|e| storage_path(&e.path)).collect();
        let label = storage.home.clone();
        Some((storage, label, entries, paths))
    }

    fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((storage, home_label, entries, paths)) = self.delete_plan() else {
            return;
        };
        let message = match entries.as_slice() {
            [one] => format!("Delete “{}”?", one.name),
            many => format!("Delete {} items?", many.len()),
        };
        let detail = format!(
            "{} deleted immediately from the persistent storage “{}”. You can’t undo this action.",
            if entries.len() == 1 {
                "This item will be"
            } else {
                "These items will be"
            },
            home_label
        );
        let busy_label = if entries.len() == 1 {
            "Deleting 1 item…".to_owned()
        } else {
            format!("Deleting {} items…", entries.len())
        };
        let answer = window.prompt(
            gpui_kit::PromptLevel::Warning,
            &message,
            Some(&detail),
            &[
                gpui_kit::PromptButton::new("Delete"),
                gpui_kit::PromptButton::cancel("Cancel"),
            ],
            cx,
        );
        let folder = self.browser.path().to_owned();
        let rt = self.rt.clone();
        cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let pending =
                rt.spawn(async move { storage.client.delete_storage(&storage.home, &paths).await });
            let _ = this.update(cx, |this, cx| {
                this.busy = Some(busy_label);
                cx.notify();
            });
            let result = pending.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => this.say(error, true, cx),
                    Err(_) => this.say("Deleting was interrupted.", true, cx),
                }
                cx.emit(PaneEvent::Changed);
                if this.browser.path() == folder {
                    this.browser.clear_selection();
                    this.reload(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows a notice: a failure stays until dismissed, information fades after a few seconds.
    fn say(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.notice_generation += 1;
        let generation = self.notice_generation;
        self.notice = Some(ui::Notice {
            text: text.into(),
            error,
        });
        if !error {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(4)).await;
                let _ = this.update(cx, |this, cx| {
                    if this.notice_generation == generation {
                        this.notice = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    // ---- Transfers -----------------------------------------------------------------------

    fn upload(&mut self, sources: Vec<PathBuf>, cx: &mut Context<Self>) {
        let folder = self.browser.path().to_owned();
        self.upload_into(sources, folder, cx);
    }

    /// Uploads `sources` into the remote `folder`: the folder shown, or a folder row they were
    /// dropped on.
    fn upload_into(&mut self, sources: Vec<PathBuf>, folder: String, cx: &mut Context<Self>) {
        if !self.policy.upload {
            self.say("The server doesn’t allow uploads.", true, cx);
            cx.notify();
            return;
        }
        if sources.is_empty() {
            return;
        }
        // Into a home's `Desktop/files`, it would be hidden under the Shared Files mount in
        // every session: it goes to Shared Files instead (`ARCH-037`).
        if self
            .backend
            .place()
            .is_some_and(|place| storage::upload_target(place, &folder) == UploadTarget::SharedRoot)
        {
            cx.emit(PaneEvent::UploadToShared(sources));
            return;
        }
        // The job lists the destination again before naming; these are a fallback.
        let taken: HashSet<String> = if folder == self.browser.path() {
            self.browser
                .entries()
                .iter()
                .map(|e| e.name.clone())
                .collect()
        } else {
            HashSet::new()
        };
        let name = batch_name(
            sources
                .iter()
                .filter_map(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned()),
        );
        let backend = self.backend.clone();
        let home = backend.home_key();
        let meter = Meter::default();
        let job_meter = meter.clone();
        let destination = folder.clone();
        let job = self.rt.spawn(async move {
            upload_job(backend, sources, destination, taken, job_meter)
                .await
                .map(|()| Vec::new())
        });
        self.track(
            name,
            Direction::Upload,
            meter,
            job,
            After::Uploaded { home, folder },
            cx,
        );
    }

    fn download(
        &mut self,
        entries: Vec<Entry>,
        folder: PathBuf,
        after: After,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.policy.download {
            self.say("The server doesn’t allow downloads.", true, cx);
            if let After::Promise(completion) = after {
                completion.finish(Some("The server doesn’t allow downloads."));
            }
            cx.notify();
            return;
        }
        let name = batch_name(entries.iter().map(|e| e.name.clone()));
        let backend = self.backend.clone();
        let meter = Meter::default();
        let job_meter = meter.clone();
        // A promise names its file itself: Finder already chose the final path.
        let exact = matches!(after, After::Promise(_));
        let job = self
            .rt
            .spawn(async move { download_job(backend, entries, folder, exact, job_meter).await });
        self.track(name, Direction::Download, meter, job, after, cx);
    }

    fn track(
        &mut self,
        name: String,
        direction: Direction,
        meter: Meter,
        job: tokio::task::JoinHandle<Result<Vec<PathBuf>, String>>,
        after: After,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        self.transfers_open = true;
        self.transfers.push(Transfer {
            id,
            name,
            direction,
            meter,
            state: State::Running,
            job: Some(job.abort_handle()),
            made: Vec::new(),
            after: Some(after),
            home: self.backend.home_key(),
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| this.finish(id, result, cx));
        })
        .detach();
        self.tick(cx);
        cx.notify();
    }

    /// Redraws while anything moves.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let running = this
                .update(cx, |this, cx| {
                    cx.notify();
                    let running = this
                        .transfers
                        .iter()
                        .any(|t| matches!(t.state, State::Running));
                    if !running {
                        this.ticker = None;
                    }
                    running
                })
                .unwrap_or(false);
            if !running {
                break;
            }
        }));
    }

    fn cancel(&mut self, id: u64) {
        if let Some(transfer) = self.transfers.iter_mut().find(|t| t.id == id) {
            if let Some(job) = transfer.job.take() {
                job.abort();
            }
        }
    }

    fn finish(
        &mut self,
        id: u64,
        result: Result<Result<Vec<PathBuf>, String>, tokio::task::JoinError>,
        cx: &mut Context<Self>,
    ) {
        let Some(transfer) = self.transfers.iter_mut().find(|t| t.id == id) else {
            return;
        };
        transfer.job = None;
        let after = transfer.after.take();
        let failure = match result {
            Ok(Ok(made)) => {
                transfer.state = State::Done;
                transfer.made = made;
                None
            }
            Ok(Err(error)) => {
                transfer.state = State::Failed(error.clone());
                Some(error)
            }
            Err(_) => {
                transfer.state = State::Cancelled;
                if let Some(partial) = transfer
                    .meter
                    .partial
                    .lock()
                    .ok()
                    .and_then(|mut p| p.take())
                {
                    let _ = std::fs::remove_file(partial);
                }
                Some("Cancelled".to_owned())
            }
        };
        let made = transfer.made.clone();
        let direction = transfer.direction;
        if transfer.meter.forbidden.load(Ordering::Relaxed) {
            self.learn_forbidden(direction, cx);
        }
        match after {
            Some(After::Promise(completion)) => completion.finish(failure.as_deref()),
            Some(After::Uploaded { home, folder }) => {
                if uploaded_shows(self.backend.home_key(), self.browser.path(), &home, &folder) {
                    self.reload(cx);
                }
                cx.emit(PaneEvent::Changed);
            }
            Some(After::Open) if failure.is_none() => {
                for path in &made {
                    cx.open_with_system(path);
                }
            }
            Some(After::QuickLook) if failure.is_none() => quick_look(&made),
            Some(After::Clipboard) if failure.is_none() => {
                pasteboard::write_file_paths(&made);
                let text = if made.len() == 1 {
                    "Copied. Paste in Finder to save it.".to_owned()
                } else {
                    format!("Copied {} items. Paste in Finder to save them.", made.len())
                };
                self.say(text, false, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn transfer_rows(&self) -> Vec<TransferRow> {
        self.transfers
            .iter()
            .rev()
            .map(|transfer| {
                let done = transfer.meter.done.load(Ordering::Relaxed);
                let total = transfer.meter.total.load(Ordering::Relaxed);
                let status = match &transfer.state {
                    State::Running if total == u64::MAX => TransferStatus::Running {
                        fraction: None,
                        detail: "Preparing…".into(),
                    },
                    State::Running => TransferStatus::Running {
                        fraction: Some(if total == 0 {
                            0.
                        } else {
                            (done as f64 / total as f64).min(1.) as f32
                        }),
                        detail: format!("{} of {}", size_label(done), size_label(total)),
                    },
                    State::Done => TransferStatus::Done(
                        transfer
                            .meter
                            .note
                            .lock()
                            .ok()
                            .and_then(|note| note.clone())
                            .unwrap_or_else(|| match transfer.direction {
                                Direction::Upload => "Uploaded".into(),
                                Direction::Download => "Downloaded".into(),
                            }),
                    ),
                    State::Failed(error) => TransferStatus::Failed(error.clone()),
                    State::Cancelled => TransferStatus::Cancelled,
                };
                TransferRow {
                    id: transfer.id,
                    name: transfer.name.clone(),
                    direction: transfer.direction,
                    status,
                    revealable: matches!(transfer.state, State::Done)
                        && transfer.direction == Direction::Download
                        && !transfer.made.is_empty(),
                }
            })
            .collect()
    }
}

/// `this.update` with the window, for work that needs it after an await.
trait UpdateInFiles {
    fn update_in_files(
        &self,
        cx: &mut gpui_kit::AsyncApp,
        f: impl FnOnce(&mut FilesWindow, &mut Window, &mut Context<FilesWindow>),
    ) -> gpui_kit::Result<()>;
}

impl UpdateInFiles for gpui_kit::WeakEntity<FilesWindow> {
    fn update_in_files(
        &self,
        cx: &mut gpui_kit::AsyncApp,
        f: impl FnOnce(&mut FilesWindow, &mut Window, &mut Context<FilesWindow>),
    ) -> gpui_kit::Result<()> {
        // The Files window is rendered by its own window alone, so its current window is the
        // one to update through (`ARCH-015` concerns entities other windows render).
        self.update_in(cx, f)
    }
}

impl Render for FilesWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if std::mem::take(&mut self.reveal_selection) {
            if let Some(focused) = self.browser.focused() {
                self.scroll.scroll_to_item(focused, ScrollStrategy::Nearest);
            }
        }
        if std::mem::take(&mut self.clear_filter) {
            if let Some(filter) = &self.filter {
                filter.update(cx, |state, cx| state.set_value("", window, cx));
            }
        }
        let place = self.backend.place().cloned();
        let storage_props = place.as_ref().map(|place| ui::StoragePaneProps {
            open_in: (place.is_shared() && self.browser.path().is_empty())
                .then(|| self.open_in_entry().is_some()),
            total: self.browser.total(),
            filter: self.browser.filter().to_string(),
            empty_text: if place.is_shared() && self.browser.path().is_empty() {
                "Shared Files is empty. Drop files here to keep them for every session.".into()
            } else {
                "This folder is empty.".into()
            },
            narrow: window.bounds().size.width < px(760.),
            delete_enabled: storage_can_delete(place, &self.browser.selected_entries()),
        });
        let can_delete = self.storage.is_some() && self.storage_matches;
        let props = FilesProps {
            crumbs: self.browser.crumbs(&self.root_label),
            can_back: self.browser.can_back(),
            can_forward: self.browser.can_forward(),
            can_upload: self.policy.upload,
            can_download: self.policy.download,
            can_delete: can_delete || place.is_some(),
            rows: Rc::new(self.browser.rows(|path| {
                place
                    .as_ref()
                    .is_some_and(|place| storage::protected(place, path))
            })),
            sort: self.browser.sort,
            ascending: self.browser.ascending,
            loading: self.loading,
            error: self.error.clone(),
            notice: self.notice.clone(),
            busy: self.busy.clone(),
            transfers_open: self.transfers_open,
            focused: window.is_window_active(),
            transfers: self.transfer_rows(),
            creating_folder: self.creating_folder,
            selected: self.browser.selected_count(),
            storage: storage_props,
        };
        let entity = cx.entity();
        let handler: ui::FilesHandler = Rc::new(move |action, window, cx| {
            entity.update(cx, |this, cx| this.handle(action, window, cx));
        });
        let ring = theme.ring;
        let menu_focus = self.list_focus.clone();
        let selection = self.browser.selected_entries();
        let menu_state = MenuState {
            selected: selection.len(),
            files_only: !selection.is_empty() && selection.iter().all(|e| !e.is_dir),
            upload: self.policy.upload,
            download: self.policy.download,
            delete: props.can_delete,
            delete_enabled: props
                .storage
                .as_ref()
                .map_or(!selection.is_empty(), |storage| storage.delete_enabled),
            open_in: match &place {
                Some(place) if place.is_shared() && selection.iter().all(|e| !e.is_dir) => {
                    Some(self.open_in_entry().is_some())
                }
                _ => None,
            },
        };
        div()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_sm()
            .key_context("FilesWindow")
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.pressed = None;
                    this.press_at = None;
                }),
            )
            .on_action(cx.listener(|this, _: &GoBack, _, cx| {
                let moved = this.browser.back();
                this.after_move(moved, cx);
            }))
            .on_action(cx.listener(|this, _: &GoForward, _, cx| {
                let moved = this.browser.forward();
                this.after_move(moved, cx);
            }))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| {
                this.reload(cx);
                cx.emit(PaneEvent::Refreshed);
            }))
            .on_action(cx.listener(|this, _: &OpenIn, _, cx| this.open_in(cx)))
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                if let Some(filter) = &this.filter {
                    filter.update(cx, |state, cx| state.focus(window, cx));
                }
            }))
            .on_action(
                cx.listener(|this, _: &NewFolder, window, cx| this.start_new_folder(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &UploadFiles, window, cx| this.choose_uploads(window, cx)),
            )
            .on_action(cx.listener(|this, _: &CloseFiles, window, cx| {
                release(&this.owner, this.handle, cx);
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &Cancel, window, cx| {
                if this.creating_folder {
                    this.creating_folder = false;
                    window.focus(&this.list_focus, cx);
                } else {
                    this.browser.clear_selection();
                }
                cx.notify();
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.upload(paths.paths().to_vec(), cx);
            }))
            .when(self.policy.upload, |root| {
                root.drag_over::<ExternalPaths>(move |style, _, _, _| {
                    style.border_2().border_color(ring)
                })
            })
            .child(
                div()
                    .id("files-pane")
                    .size_full()
                    .track_focus(&self.list_focus)
                    .key_context("FilesList")
                    .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| {
                        this.browser.step(-1, false);
                        this.reveal_selection = true;
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &SelectNext, _, cx| {
                        this.browser.step(1, false);
                        this.reveal_selection = true;
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &ExtendPrevious, _, cx| {
                        this.browser.step(-1, true);
                        this.reveal_selection = true;
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &ExtendNext, _, cx| {
                        this.browser.step(1, true);
                        this.reveal_selection = true;
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &OpenSelection, window, cx| {
                        this.open_selection(window, cx);
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &OpenParent, _, cx| {
                        if let Some(parent) = this.browser.parent() {
                            let moved = this.browser.navigate(&parent);
                            this.after_move(moved, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &QuickLook, window, cx| {
                        this.quick_look(window, cx);
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &CopyToMac, window, cx| {
                        this.copy_to_mac(window, cx);
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &PasteFromMac, _, cx| this.paste_from_mac(cx)))
                    .on_action(cx.listener(|this, _: &DownloadSelection, window, cx| {
                        this.handle(FilesAction::Download, window, cx);
                    }))
                    .on_action(cx.listener(|this, _: &DownloadSelectionTo, window, cx| {
                        this.handle(FilesAction::DownloadTo, window, cx);
                    }))
                    .on_action(cx.listener(|this, _: &DeleteSelection, window, cx| {
                        this.confirm_delete(window, cx);
                    }))
                    .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                        this.browser.select_all();
                        cx.notify();
                    }))
                    .child(ui::render(
                        &theme,
                        &props,
                        &self.folder_name,
                        self.filter.as_ref(),
                        &self.scroll,
                        handler,
                    ))
                    .context_menu(move |menu, _, _| context_menu(menu, &menu_focus, menu_state)),
            )
    }
}

/// What the right-click menu may offer, read when the window renders.
#[derive(Clone, Copy)]
struct MenuState {
    selected: usize,
    files_only: bool,
    upload: bool,
    download: bool,
    delete: bool,
    delete_enabled: bool,
    /// Open in… shows in Shared Files (`Some`), enabled at its top level only.
    open_in: Option<bool>,
}

/// Finder's right-click menu: the selection's commands, then the folder's.
fn context_menu(
    menu: gpui_kit::component::menu::PopupMenu,
    focus: &FocusHandle,
    state: MenuState,
) -> gpui_kit::component::menu::PopupMenu {
    let none = state.selected == 0;
    let mut menu = menu
        .action_context(focus.clone())
        .menu_with_disabled("Open", Box::new(OpenSelection), none)
        .menu_with_disabled("Quick Look", Box::new(QuickLook), !state.files_only)
        .separator();
    match state.open_in {
        Some(true) => menu = menu.menu("Open in…", Box::new(OpenIn)),
        Some(false) if state.files_only => {
            menu = menu.menu_with_disabled("Open in… (Top level only)", Box::new(OpenIn), true)
        }
        _ => {}
    }
    if state.download {
        menu = menu
            .menu_with_disabled("Download", Box::new(DownloadSelection), none)
            .menu_with_disabled("Download To…", Box::new(DownloadSelectionTo), none)
            .menu_with_disabled("Copy", Box::new(CopyToMac), none)
            .separator();
    }
    if state.upload {
        menu = menu
            .menu("New Folder", Box::new(NewFolder))
            .menu("Upload…", Box::new(UploadFiles))
            .menu("Paste", Box::new(PasteFromMac));
    }
    if state.delete {
        menu = menu.separator().menu_with_disabled(
            "Delete…",
            Box::new(DeleteSelection),
            !state.delete_enabled,
        );
    }
    menu
}

// ---- Jobs (Tokio) ------------------------------------------------------------------------

/// Uploads `sources` into the remote `folder`, each top-level item under a name not `taken`
/// there yet (Finder's "keep both"), folders walked on the Mac and recreated.
async fn upload_job(
    access: FilesBackend,
    sources: Vec<PathBuf>,
    folder: String,
    taken: HashSet<String>,
    meter: Meter,
) -> Result<(), String> {
    // The window's listing may be stale or still loading (it is empty right after moving to
    // another folder), and the server replaces a file of the same name, so check the folder
    // again before choosing names. A server that refuses listings (downloads off) keeps the
    // window's names. Storage never overwrites; its `name-1.ext` covers a race.
    let mut taken = taken;
    if let Ok(listing) = access.browse(&folder).await {
        taken.extend(listing.entries.into_iter().map(|entry| entry.name));
    }
    let plan = tokio::task::spawn_blocking(move || plan_upload(&sources, &folder, taken))
        .await
        .map_err(|_| "Reading the files was interrupted.".to_owned())?
        .map_err(|error| error.to_string())?;
    meter.total.store(plan.bytes, Ordering::Relaxed);
    if let Ok(mut note) = meter.note.lock() {
        *note = keep_both_note(&plan.renamed);
    }
    for folder in &plan.folders {
        access.mkdir(folder).await.map_err(|e| e.to_string())?;
    }
    for (local, remote) in &plan.files {
        access
            .upload(local, remote, &meter.done)
            .await
            .map_err(|e| {
                note_forbidden(&meter, &e);
                format!("{}: {e}", display_name(local))
            })?;
    }
    Ok(())
}

#[derive(Debug, Default, PartialEq)]
struct UploadPlan {
    /// Remote folders to make, parents first.
    folders: Vec<String>,
    files: Vec<(PathBuf, String)>,
    bytes: u64,
    /// Top-level items given a new name to keep both: `(original, new)`.
    renamed: Vec<(String, String)>,
}

fn plan_upload(
    sources: &[PathBuf],
    folder: &str,
    mut taken: HashSet<String>,
) -> std::io::Result<UploadPlan> {
    let mut plan = UploadPlan::default();
    for source in sources {
        let meta = std::fs::symlink_metadata(source)?;
        let Some(name) = source.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let original = name;
        let name = unique_name(&original, meta.is_dir(), |candidate| {
            taken.contains(candidate)
        });
        if name != original {
            plan.renamed.push((original, name.clone()));
        }
        taken.insert(name.clone());
        let remote = selkies::join(folder, &name);
        if meta.is_dir() {
            plan.folders.push(remote.clone());
            walk_local(source, &remote, &mut plan)?;
        } else if meta.is_file() {
            plan.bytes += meta.len();
            plan.files.push((source.clone(), remote));
        }
    }
    Ok(plan)
}

/// Files and folders below `dir`; symbolic links are left out, as the server's zip does.
fn walk_local(dir: &Path, remote: &str, plan: &mut UploadPlan) -> std::io::Result<()> {
    let mut children: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    children.sort_by_key(|child| child.file_name());
    for child in children {
        let name = child.file_name().to_string_lossy().into_owned();
        if name == ".DS_Store" {
            continue;
        }
        let meta = child.metadata()?;
        let kind = child.file_type()?;
        let path = selkies::join(remote, &name);
        if kind.is_dir() {
            plan.folders.push(path.clone());
            walk_local(&child.path(), &path, plan)?;
        } else if kind.is_file() {
            plan.bytes += meta.len();
            plan.files.push((child.path(), path));
        }
    }
    Ok(())
}

/// Downloads `entries` into the Mac `folder`, each under a free name (or, for a file promise,
/// the name Finder chose), folders walked and recreated; returns what it made.
async fn download_job(
    access: FilesBackend,
    entries: Vec<Entry>,
    folder: PathBuf,
    exact: bool,
    meter: Meter,
) -> Result<Vec<PathBuf>, String> {
    let fail = |error: BackendError| error.to_string();
    let mut made = Vec::new();
    let mut files = Vec::new();
    let mut folders = Vec::new();
    let mut bytes = 0;
    let target_folder = if exact {
        folder.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        folder.clone()
    };
    tokio::fs::create_dir_all(&target_folder)
        .await
        .map_err(|e| e.to_string())?;
    let mut reserved = HashSet::new();
    for entry in entries {
        let name = selkies::local_name(&entry.name)
            .ok_or_else(|| fail(selkies::Error::UnsafeName(entry.name.clone()).into()))?;
        let top = if exact {
            folder.clone()
        } else {
            let name = unique_name(name, entry.is_dir, |candidate| {
                reserved.contains(candidate) || target_folder.join(candidate).exists()
            });
            reserved.insert(name.clone());
            target_folder.join(name)
        };
        made.push(top.clone());
        if entry.is_dir {
            folders.push(top.clone());
            for (child, relative) in access.walk(&entry.path).await.map_err(fail)? {
                let local = top.join(&relative);
                if child.is_dir {
                    folders.push(local);
                } else {
                    bytes += child.size.unwrap_or(0);
                    files.push((child.path, local, child.size));
                }
            }
        } else {
            bytes += entry.size.unwrap_or(0);
            files.push((entry.path, top, entry.size));
        }
    }
    meter.total.store(bytes, Ordering::Relaxed);
    for folder in &folders {
        tokio::fs::create_dir_all(folder)
            .await
            .map_err(|e| e.to_string())?;
    }
    for (remote, local, expected) in files {
        let partial = selkies::partial_path(&local);
        let _ = tokio::fs::remove_file(&partial).await;
        if let Ok(mut current) = meter.partial.lock() {
            *current = Some(partial);
        }
        let before = meter.done.load(Ordering::Relaxed);
        let mut attempt = 0;
        loop {
            // A retry resumes the partial file where the last attempt stopped.
            meter.done.store(before, Ordering::Relaxed);
            match access
                .download(&remote, &local, &meter.done, expected)
                .await
            {
                Ok(()) => break,
                Err(error) if error.is_network() && attempt < 2 => {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(error) => {
                    note_forbidden(&meter, &error);
                    return Err(format!("{}: {error}", display_name(&local)));
                }
            }
        }
        if let Ok(mut current) = meter.partial.lock() {
            *current = None;
        }
    }
    Ok(made)
}

/// Marks the transfer as refused by its own Selkies route (never for storage, `ARCH-037`).
fn note_forbidden(meter: &Meter, error: &BackendError) {
    if error.forbidden() {
        meter.forbidden.store(true, Ordering::Relaxed);
    }
}

/// What the Storage window says when something would land in a home's `Desktop/files`.
const SHARED_MOUNT_NOTICE: &str =
    "This folder is your Shared Files. Every session with storage sees it here.";

/// The home-rooted paths a storage Delete sends: never a home's `Desktop` or
/// `Desktop/files` (`ARCH-037`).
fn deletable_paths(place: &StoragePlace, entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| !storage::protected(place, &entry.path))
        .map(|entry| storage::home_path(&entry.path))
        .collect()
}

/// Delete is enabled in the Storage window for a selection with something it may send.
fn storage_can_delete(place: &StoragePlace, selection: &[&Entry]) -> bool {
    selection
        .iter()
        .any(|entry| !storage::protected(place, &entry.path))
}

/// Whether a finished upload's folder is the one shown, in the same home.
fn uploaded_shows(
    shown_home: Option<String>,
    shown_folder: &str,
    home: &Option<String>,
    folder: &str,
) -> bool {
    shown_home == *home && shown_folder == folder
}

/// Where the Selkies file-manager folder sits in a SealSkin home: `FILE_MANAGER_PATH` defaults
/// to `~/Desktop`, and SealSkin mounts the home as `~` (`ARCH-024`).
const STORAGE_ROOT: &str = "/Desktop";

/// A Selkies root-relative path as a home path for SealSkin.
fn storage_path(remote: &str) -> String {
    format!("{STORAGE_ROOT}/{}", remote.trim_matches('/'))
}

/// Whether two listings name the same entries, leaving out the transient staging files an
/// upload or download in flight leaves behind.
fn same_folder<'a>(
    selkies: impl Iterator<Item = &'a str>,
    storage: impl Iterator<Item = &'a str>,
) -> bool {
    let settled =
        |name: &&str| !name.starts_with(".selkies-upload-") && !name.ends_with(".skerrydownload");
    let ours: HashSet<&str> = selkies.filter(settled).collect();
    let theirs: HashSet<&str> = storage.filter(settled).collect();
    ours == theirs
}

/// What a Files window works through (`ARCH-024`): the Selkies session's own file routes, and,
/// for a SealSkin session with a home, that home's storage, which can also delete.
pub(super) struct FilesSource {
    pub title: String,
    pub selkies: SelkiesClient,
    pub policy: Policy,
    pub storage: Option<Storage>,
}

/// The session's home in SealSkin's persistent storage.
#[derive(Clone)]
pub(super) struct Storage {
    pub client: crate::sealskin::SealSkinClient,
    pub home: String,
}

/// What a finished upload says when it had to rename to keep both copies.
fn keep_both_note(renamed: &[(String, String)]) -> Option<String> {
    match renamed {
        [] => None,
        [(_, new)] => Some(format!("Uploaded as “{new}”")),
        many => Some(format!("Uploaded; {} renamed to keep both", many.len())),
    }
}

/// `name`, or Finder's `name 2.ext`, `name 3.ext`, … when `taken`.
fn unique_name(name: &str, is_dir: bool, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_owned();
    }
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if !is_dir && dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| format!("{stem} {n}{extension}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| name.to_owned())
}

fn batch_name(names: impl Iterator<Item = String>) -> String {
    let names: Vec<String> = names.collect();
    match names.as_slice() {
        [one] => one.clone(),
        _ => format!("{} items", names.len()),
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn downloads_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("Downloads")
}

/// A fresh folder under `~/Library/Caches/SealSkin Native/Files/` for files opened, previewed,
/// or copied, so two of them never collide and macOS may clean them up.
fn scratch_dir(purpose: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|t| t.as_millis())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("Library/Caches/SealSkin Native/Files")
        .join(format!("{purpose} {stamp}-{count}"))
}

/// macOS Quick Look on downloaded copies, through `qlmanage`.
fn quick_look(paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    let _ = std::process::Command::new("/usr/bin/qlmanage")
        .arg("-p")
        .args(paths)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_keep_both_like_finder() {
        let taken = ["a.txt", "a 2.txt", "Folder", ".env"];
        let is_taken = |name: &str| taken.contains(&name);
        assert_eq!(unique_name("b.txt", false, is_taken), "b.txt");
        assert_eq!(unique_name("a.txt", false, is_taken), "a 3.txt");
        assert_eq!(unique_name("Folder", true, is_taken), "Folder 2");
        assert_eq!(unique_name(".env", false, is_taken), ".env 2");
        assert_eq!(unique_name("v1.2", true, |n| n == "v1.2"), "v1.2 2");
    }

    #[test]
    fn uploads_walk_folders_and_skip_links() {
        let root = tempfile::tempdir().unwrap();
        let top = root.path().join("Project");
        std::fs::create_dir_all(top.join("src/empty")).unwrap();
        std::fs::write(top.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(top.join("README"), b"hi").unwrap();
        std::fs::write(top.join(".DS_Store"), b"x").unwrap();
        std::os::unix::fs::symlink("/etc/hosts", top.join("link")).unwrap();
        let single = root.path().join("notes.txt");
        std::fs::write(&single, b"12345").unwrap();

        let plan = plan_upload(
            &[top.clone(), single.clone()],
            "Docs",
            HashSet::from(["notes.txt".to_owned()]),
        )
        .unwrap();
        assert_eq!(
            plan.folders,
            ["Docs/Project", "Docs/Project/src", "Docs/Project/src/empty"]
        );
        let remotes: Vec<_> = plan.files.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(
            remotes,
            [
                "Docs/Project/README",
                "Docs/Project/src/main.rs",
                "Docs/notes 2.txt"
            ]
        );
        assert_eq!(plan.bytes, 12 + 2 + 5);
        assert_eq!(
            plan.renamed,
            [("notes.txt".to_owned(), "notes 2.txt".to_owned())]
        );
        assert_eq!(
            keep_both_note(&plan.renamed).as_deref(),
            Some("Uploaded as “notes 2.txt”")
        );
        assert_eq!(keep_both_note(&[]), None);
    }

    #[test]
    fn list_keys_leave_the_new_folder_field_alone() {
        use gpui_kit::{KeyContext, Keymap, Keystroke};
        let keymap = Keymap::new(key_bindings());
        let stack = |names: &[&str]| -> Vec<KeyContext> {
            names
                .iter()
                .map(|name| KeyContext::parse(name).unwrap())
                .collect()
        };
        let list = stack(&["FilesWindow", "FilesList"]);
        let field = stack(&["FilesWindow", "FilesList", "Input"]);
        for key in ["space", "cmd-o", "cmd-y", "cmd-c", "cmd-v", "cmd-a"] {
            let keystroke = [Keystroke::parse(key).unwrap()];
            let (on_list, _) = keymap.bindings_for_input(&keystroke, &list);
            assert!(!on_list.is_empty(), "{key} works in the list");
            let (in_field, _) = keymap.bindings_for_input(&keystroke, &field);
            assert!(in_field.is_empty(), "{key} is left to the New Folder field");
        }
        let (refresh, _) = keymap.bindings_for_input(&[Keystroke::parse("cmd-r").unwrap()], &field);
        assert!(!refresh.is_empty(), "window keys still work from the field");
    }

    #[test]
    fn deletes_map_into_the_home_desktop_only_when_the_folders_match() {
        assert_eq!(storage_path("notes.md"), "/Desktop/notes.md");
        assert_eq!(storage_path("/Work/a b.txt"), "/Desktop/Work/a b.txt");
        let ours = ["a.txt", "Work", ".selkies-upload-1234"];
        assert!(same_folder(ours.into_iter(), ["Work", "a.txt"].into_iter()));
        assert!(!same_folder(ours.into_iter(), ["Work"].into_iter()));
        assert!(!same_folder(["a"].into_iter(), ["a", "b"].into_iter()));
        assert!(same_folder([].into_iter(), [].into_iter()));
    }

    fn entry(path: &str, is_dir: bool) -> Entry {
        Entry {
            name: path.rsplit('/').next().unwrap().into(),
            path: path.into(),
            is_dir,
            size: None,
            mtime: String::new(),
        }
    }

    #[test]
    fn storage_delete_needs_no_folder_match_but_skips_protected_rows() {
        let home = StoragePlace::Home("work".into());
        let desktop = entry("Desktop", true);
        let mount = entry("Desktop/files", true);
        let docs = entry("Documents", true);
        let note = entry("Desktop/note.txt", false);
        assert!(!storage_can_delete(&home, &[&desktop]));
        assert!(!storage_can_delete(&home, &[&desktop, &mount]));
        assert!(storage_can_delete(&home, &[&desktop, &docs]));
        assert_eq!(
            deletable_paths(&home, &[desktop.clone(), docs, mount.clone(), note]),
            ["/Documents", "/Desktop/note.txt"]
        );
        // Shared Files has no mount points: its own Desktop folder may go.
        assert_eq!(
            deletable_paths(&StoragePlace::Shared, &[desktop]),
            ["/Desktop"]
        );
    }

    #[test]
    fn uploaded_refresh_only_for_the_same_home() {
        let work = Some("work".to_string());
        assert!(uploaded_shows(work.clone(), "Docs", &work, "Docs"));
        assert!(!uploaded_shows(Some("other".into()), "Docs", &work, "Docs"));
        assert!(!uploaded_shows(work.clone(), "", &work, "Docs"));
        assert!(uploaded_shows(None, "", &None, ""), "a session's window");
    }

    #[test]
    fn storage_quota_403_keeps_upload_enabled_and_shows_the_sentence() {
        let quota = BackendError::Storage(crate::sealskin::StorageError::Invalid(
            "Storage limit reached: 50 GB.".into(),
        ));
        let meter = Meter::default();
        note_forbidden(&meter, &quota);
        assert!(!meter.forbidden.load(Ordering::Relaxed));
        let refused = BackendError::Storage(crate::sealskin::StorageError::Api(
            crate::sealskin::ApiError::Forbidden(
                "Persistent storage is disabled for this account.".into(),
            ),
        ));
        note_forbidden(&meter, &refused);
        assert!(!meter.forbidden.load(Ordering::Relaxed));
        assert_eq!(
            refused.to_string(),
            "Persistent storage is disabled for this account."
        );
        let session =
            BackendError::Selkies(selkies::Error::Server(StatusCode::FORBIDDEN, String::new()));
        note_forbidden(&meter, &session);
        assert!(meter.forbidden.load(Ordering::Relaxed));
    }

    #[test]
    fn storage_keys_stay_in_the_files_window() {
        use gpui_kit::{KeyContext, Keymap, Keystroke};
        let keymap = Keymap::new(key_bindings());
        let stack = |names: &[&str]| -> Vec<KeyContext> {
            names
                .iter()
                .map(|n| KeyContext::parse(n).unwrap())
                .collect()
        };
        for key in ["alt-cmd-o", "cmd-f"] {
            let keystroke = [Keystroke::parse(key).unwrap()];
            let (pane, _) =
                keymap.bindings_for_input(&keystroke, &stack(&["Storage", "FilesWindow"]));
            assert!(!pane.is_empty(), "{key}");
            let (desktop, _) =
                keymap.bindings_for_input(&keystroke, &stack(&[crate::view::DESKTOP_CONTEXT]));
            assert!(desktop.is_empty(), "{key}");
        }
    }

    #[test]
    fn batches_are_named_for_their_one_item_or_their_count() {
        assert_eq!(batch_name(["a".to_owned()].into_iter()), "a");
        assert_eq!(batch_name(["a".into(), "b".into()].into_iter()), "2 items");
    }

    #[test]
    fn forbidden_upload_turns_off_upload_only() {
        let both = Policy::default();
        let forbidden = StatusCode::FORBIDDEN;
        assert_eq!(
            policy_after(both, Route::Upload, forbidden),
            Policy {
                upload: false,
                download: true
            }
        );
        assert_eq!(
            policy_after(both, Route::Download, forbidden),
            Policy {
                upload: true,
                download: false
            }
        );
        assert_eq!(policy_after(both, Route::Mkdir, forbidden), both);
        assert_eq!(policy_after(both, Route::Browse, forbidden), both);
        assert_eq!(
            policy_after(both, Route::Upload, StatusCode::INTERNAL_SERVER_ERROR),
            both
        );
    }
}
