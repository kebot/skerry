mod capture;
mod keys;
mod pinch;
mod stripes;

pub use capture::CaptureHint;

use crate::app::ToggleSessionControls;
use crate::clipboard::{ClipboardSettings, Content, Effective, Incoming, Policy};
use crate::input::{commit_command, InputSettings, RemoteChord};
use crate::pasteboard;
use crate::settings::{Key, ScreenChange};
use crate::shortcuts;
use crate::transport::{
    self, DisplayRequest, Frame, Input, Picture, Pointer, RemoteCursor, Resolution, SessionEvent,
    SessionOptions,
};
use gpui_kit::{
    canvas, div, point, prelude::*, px, size, AnyView, App, Bounds, ContentMask, Context, Corners,
    DispatchPhase, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, HitboxBehavior,
    KeyDownEvent, KeyUpEvent, Modifiers, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollEvent, SharedString, Task,
    UTF16Selection, Window,
};
use reqwest::Url;
use skerry_ui::session;
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;
use tokio::{
    runtime::Handle,
    sync::{mpsc, watch},
};

/// The key context of a session's remote desktop.
pub(crate) const DESKTOP_CONTEXT: &str = "SessionDesktop";
const MENU_SIZE: Pixels = px(32.);
/// How long a secret the session copied may stay on the Mac (the web core's
/// `SECRET_CLIPBOARD_TTL_MS`).
const SECRET_TTL: Duration = Duration::from_secs(60);
const KEY_V: u32 = 0x76;
const KEY_INSERT: u32 = 0xff63;
const MENU_MARGIN: Pixels = px(8.);

pub struct NativeSession {
    view: Entity<Desktop>,
    input: mpsc::UnboundedSender<Input>,
    task: tokio::task::JoinHandle<()>,
    /// Set on drop, so a transport still connecting stops before it talks to the server.
    cancel: watch::Sender<bool>,
    runtime: Handle,
    _pinch: Option<pinch::Monitor>,
}

impl NativeSession {
    pub fn open(
        address: Url,
        window: &mut Window,
        cx: &mut App,
        runtime: &Handle,
        options: SessionOptions,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        let (frame_tx, frames) = watch::channel(Frame::default());
        let (input, commands) = mpsc::unbounded_channel();
        let (pointer, motion) = watch::channel(None);
        let waiting = SharedString::from(options.waiting.clone());
        let capture_events = events.clone();
        let view = cx.new(|cx| {
            let mut desktop =
                Desktop::new(input.clone(), pointer, frames, capture_events, window, cx);
            desktop.waiting = waiting;
            desktop
        });
        let pinch = crate::app::native_window(window).and_then(|native| {
            let desktop = view.read(cx);
            pinch::Monitor::install(native, desktop.pinch_target.clone(), input.clone())
        });
        let focus = view.read(cx).focus.clone();
        window.focus(&focus, cx);
        let (cancel, cancelled) = watch::channel(false);
        let task = runtime.spawn(async move {
            match transport::run(
                address, frame_tx, commands, motion, options, &events, cancelled,
            )
            .await
            {
                Ok(()) => {}
                Err(transport::RunError::StartupTimeout) => {
                    let _ = events.send(SessionEvent::StartupTimeout);
                }
                Err(error) => {
                    let _ = events.send(SessionEvent::Error(error.to_string()));
                }
            }
        });
        Self {
            view,
            input,
            task,
            cancel,
            runtime: runtime.clone(),
            _pinch: pinch,
        }
    }

    pub fn view(&self) -> AnyView {
        self.view.clone().into()
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let focus = self.view.read(cx).focus.clone();
        window.focus(&focus, cx);
    }

    pub fn set_settings(&self, changes: Vec<(Key, u32)>) {
        let _ = self.input.send(Input::Settings(changes));
    }

    pub fn set_screen(&self, change: ScreenChange) {
        let _ = self.input.send(Input::Screen(change));
    }

    pub fn scale_locally(&self, enabled: bool, cx: &mut App) {
        self.view.update(cx, |desktop, cx| {
            desktop.scale_locally = enabled;
            desktop.painted_display = None;
            desktop.release_pointer();
            cx.notify();
        });
    }

    pub fn set_input_settings(&self, settings: InputSettings, window: &mut Window, cx: &mut App) {
        self.view.update(cx, |desktop, cx| {
            if desktop.settings != settings {
                desktop.release_input();
                desktop.settings = settings;
                window.refresh();
                cx.notify();
            }
        });
    }

    pub fn allow_finger_scroll(&self, enabled: bool, cx: &mut App) {
        self.view.update(cx, |desktop, _| {
            if !enabled {
                desktop.end_finger_scroll();
            }
            desktop.finger_scroll_allowed = enabled;
        });
    }

    pub fn set_cursor(&self, cursor: RemoteCursor, window: &mut Window, cx: &mut App) {
        self.view.update(cx, |desktop, cx| {
            if let Some(RemoteCursor::Image { image, .. }) = desktop.cursor.replace(cursor) {
                let _ = window.drop_image(image);
            }
            if desktop.settings.remote_cursor {
                cx.notify();
            }
        });
    }

    pub fn send_chord(&self, chord: RemoteChord, cx: &mut App) {
        self.view.update(cx, |desktop, _| {
            desktop.release_input();
            let keys = chord.keys();
            for key in keys {
                desktop.command(format!("kd,{key}"));
            }
            for key in keys.iter().rev() {
                desktop.command(format!("ku,{key}"));
            }
        });
    }

    /// Lifts every key and modifier the session holds. Idempotent; called before a child
    /// window (Session Controls, Files) or a full-screen change takes the keyboard.
    pub fn release_keys(&self, cx: &mut App) {
        self.view.update(cx, |desktop, _| desktop.release_input());
    }

    pub fn set_resolution(&self, request: DisplayRequest) {
        let _ = self.input.send(Input::Resolution(request));
    }

    pub fn set_appearance(&self, dark: bool) {
        let _ = self.input.send(Input::Appearance(dark));
    }

    pub fn set_video(&self, enabled: bool) {
        let _ = self.input.send(Input::Video(enabled));
    }

    pub fn set_audio(&self, enabled: bool) {
        let _ = self.input.send(Input::Audio(enabled));
    }

    /// Plays through this output device's Core Audio UID; `None` follows the Mac's default.
    pub fn set_audio_output(&self, uid: Option<String>) {
        let _ = self.input.send(Input::AudioOutput(uid));
    }

    pub fn set_audio_bitrate(&self, bps: u32) {
        let _ = self.input.send(Input::AudioBitrate(bps));
    }

    pub fn set_clipboard_policy(&self, policy: Policy, window: &mut Window, cx: &mut App) {
        self.view.update(cx, |desktop, _| {
            desktop.clipboard.policy = Some(policy);
            if window.is_window_active() {
                desktop.sync_to_session(false);
            }
        });
    }

    pub fn set_clipboard_settings(
        &self,
        settings: ClipboardSettings,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |desktop, _| {
            desktop.clipboard.settings = settings;
            if window.is_window_active() {
                desktop.sync_to_session(false);
            }
        });
    }

    pub fn receive_clipboard(&self, incoming: Incoming, cx: &mut App) {
        self.view
            .update(cx, |desktop, cx| desktop.receive_clipboard(incoming, cx));
    }

    /// Deliberate mode's **Send Mac Clipboard**.
    pub fn send_clipboard(&self, cx: &mut App) {
        self.view
            .update(cx, |desktop, _| desktop.sync_to_session(true));
    }

    /// Deliberate mode's **Copy Session Clipboard**: fetch with `cr`, write the tagged reply.
    pub fn copy_session_clipboard(&self, cx: &mut App) {
        self.view.update(cx, |desktop, _| {
            if desktop.clipboard.effective().receive {
                desktop.clipboard.fetch = true;
                desktop.command("cr".into());
            }
        });
    }

    /// **Mac shortcuts active** changed (or a server lock arrived): the desktop's key context
    /// follows on the next render, and nothing stays held across the switch.
    pub fn set_mac_shortcuts(&self, active: bool, cx: &mut App) {
        self.view.update(cx, |desktop, cx| {
            if desktop.mac_shortcuts != active {
                desktop.release_input();
                desktop.mac_shortcuts = active;
                cx.notify();
            }
        });
    }

    /// Gamepad slots another window drives now: the transport forgets them.
    pub fn forget_pads(&self, slots: Vec<u8>) {
        if !slots.is_empty() {
            let _ = self.input.send(Input::ForgetPads(slots));
        }
    }

    /// Sends an `apps,` request (`ARCH-035`). It holds no secrets, so the input trace may
    /// print it as is.
    pub fn send_apps(&self, request: String) {
        let _ = self.input.send(Input::Command(request));
    }

    /// Sends each command in order (the gamepads' `js,…`).
    pub fn send_commands(&self, commands: Vec<String>) {
        for command in commands {
            let _ = self.input.send(Input::Command(command));
        }
    }

    pub fn set_stats(&self, open: bool) {
        let _ = self.input.send(Input::Stats(open));
    }

    /// ⌃⌥⇧Z or View → Capture Pointer in the key session window (`ARCH-032`): the chord's
    /// modifiers are retracted, then the pointer is captured or released.
    pub fn toggle_capture(&self, window: &mut Window, cx: &mut App) {
        self.view
            .update(cx, |desktop, cx| desktop.toggle_capture(window, cx));
    }

    /// Captures the pointer, if this window is active (a pending capture from Session
    /// Controls or the menu, once the window has activated).
    pub fn engage_capture(&self, window: &mut Window, cx: &mut App) {
        self.view.update(cx, |desktop, cx| {
            // The chord may have been pressed in another window: modifiers the ledger
            // recorded are lifted, the ones still held never reach the session, and the
            // chord's auto-repeats here don't release what this engages.
            let mac = window.modifiers();
            let commands = desktop.keys.mute_mods_until_clear(mac);
            desktop.commands(commands);
            desktop.chord.hold(capture::modifiers_held(mac));
            desktop.engage_capture(window, cx)
        });
    }

    /// Entering or leaving full screen: the capture ends with the "Pointer released" hint.
    pub fn release_capture_for_full_screen(&self, cx: &mut App) {
        self.view.update(cx, |desktop, _| {
            desktop.release_capture(capture::Release::FullScreen)
        });
    }

    /// The stream failed: the capture ends with no hint (the banner explains).
    pub fn release_capture(&self, cx: &mut App) {
        self.view.update(cx, |desktop, _| {
            desktop.release_capture(capture::Release::Disconnected)
        });
    }

    pub fn picture_painted(&self, cx: &App) -> bool {
        self.view.read(cx).painted_display.is_some()
    }

    pub fn pointer_captured(&self, cx: &App) -> bool {
        self.view.read(cx).captured()
    }

    pub fn release_focus(&self, window: &mut Window, cx: &mut App) {
        self.view.update(cx, |desktop, _| {
            desktop.release_input();
            if let Some(RemoteCursor::Image { image, .. }) = desktop.cursor.take() {
                let _ = window.drop_image(image);
            }
            for image in desktop.painted.borrow_mut().take_all() {
                let _ = window.drop_image(image);
            }
        });
    }
}

impl Drop for NativeSession {
    /// Closes the stream in order (`ARCH-GAP-DC-09`): `Input::Close` queues behind the `ku`s
    /// that `release_focus` sent, so the transport sends them, then a close frame. Aborting
    /// here would drop them, and the server would hold those keys through its reconnect
    /// grace. A watchdog aborts a transport that hasn't finished within a second.
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
        let _ = self.input.send(Input::Close);
        let abort = self.task.abort_handle();
        self.runtime.spawn(async move {
            tokio::time::sleep(CLOSE_WATCHDOG).await;
            if !abort.is_finished() {
                transport::trace_input_event("watchdog abort");
                abort.abort();
            }
        });
    }
}

/// How long a closing transport may take to send its key releases and close frame.
const CLOSE_WATCHDOG: Duration = Duration::from_secs(1);

struct Desktop {
    focus: FocusHandle,
    /// The overlay's text until the first frame: "Connecting to Firefox…".
    waiting: SharedString,
    scale_locally: bool,
    frame: Frame,
    painted: Rc<RefCell<stripes::StripeRetirer>>,
    painted_display: Option<PaintedDisplay>,
    pinch_target: pinch::Target,
    input: mpsc::UnboundedSender<Input>,
    pointer: watch::Sender<Option<Pointer>>,
    buttons: u8,
    position: Option<(Resolution, (u16, u16))>,
    keys: keys::KeyLedger,
    scroll: (f32, f32),
    finger_scroll_allowed: bool,
    finger_scroll_task: Option<Task<()>>,
    settings: InputSettings,
    cursor: Option<RemoteCursor>,
    marked: String,
    selection: Range<usize>,
    ime_click: Option<Point<Pixels>>,
    ime_bounds: Option<Bounds<Pixels>>,
    /// The pointer is in the top-right hot corner, so the Session Controls trigger is shown
    /// even when the picture fills the window and leaves no margin for it.
    hot_corner: bool,
    /// The pointer is in the top-left corner, so the window's own buttons are shown.
    buttons_corner: bool,
    clipboard: ClipboardBridge,
    /// **Mac shortcuts active** (`ARCH-031`): off, Skerry's own chords go to the session.
    mac_shortcuts: bool,
    /// Capture Pointer (`ARCH-032`), shared with the capture's AppKit monitor.
    capture: Rc<RefCell<capture::Capture>>,
    capture_monitor: Option<capture::Monitor>,
    cursor_lock: Option<capture::CursorLock>,
    /// One toggle per press of ⌃⌥⇧Z, whatever its auto-repeats.
    chord: capture::ChordGuard,
    /// Session events this view raises itself (the capture's state and hints).
    events: mpsc::UnboundedSender<SessionEvent>,
}

/// The Mac side of clipboard sync (`ARCH-017`).
#[derive(Default)]
struct ClipboardBridge {
    /// `None` until the server announces its settings.
    policy: Option<Policy>,
    settings: ClipboardSettings,
    /// The pasteboard change count last read or written here.
    seen: Option<isize>,
    /// The latest value synced in either direction.
    synced: Option<u64>,
    /// A deliberate **Copy Session Clipboard** waits for its `cr` reply.
    fetch: bool,
    /// The change count of a secret written here, and the timer that takes it back.
    secret: Option<(isize, Task<()>)>,
}

impl ClipboardBridge {
    fn effective(&self) -> Effective {
        self.policy
            .map(|policy| Effective::new(policy, self.settings))
            .unwrap_or_default()
    }

    /// Take a secret written here back off the pasteboard, if nothing replaced it.
    fn retract_secret(&mut self) {
        if let Some((count, _)) = self.secret.take() {
            pasteboard::clear_if_unchanged(count);
        }
    }
}

impl Desktop {
    fn new(
        input: mpsc::UnboundedSender<Input>,
        pointer: watch::Sender<Option<Pointer>>,
        mut frames: watch::Receiver<Frame>,
        events: mpsc::UnboundedSender<SessionEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let pinch_target = pinch::Target::default();
        cx.on_focus(&focus, window, |desktop, window, _| {
            if window.is_window_active() {
                desktop.sync_to_session(false);
            }
        })
        .detach();
        cx.on_blur(&focus, window, |desktop, _, cx| {
            desktop.release_capture(capture::Release::FocusLost);
            desktop.release_input();
            cx.notify();
        })
        .detach();
        cx.observe_window_activation(window, |desktop, window, cx| {
            if window.is_window_active() {
                desktop.sync_to_session(false);
            } else {
                // ⌘-Tab leaves the window on screen; a miniaturize or a screen lock doesn't.
                let visible = crate::app::native_window(window).is_some_and(|native| {
                    native
                        .occlusionState()
                        .contains(objc2_app_kit::NSWindowOcclusionState::Visible)
                });
                desktop.release_capture(capture::Release::Window { visible });
                desktop.release_input();
            }
            cx.notify();
        })
        .detach();
        cx.spawn(async move |desktop, cx| {
            while frames.changed().await.is_ok() {
                let frame = frames.borrow_and_update().clone();
                if desktop
                    .update(cx, |desktop, cx| {
                        if (desktop.frame.width, desktop.frame.height)
                            != (frame.width, frame.height)
                        {
                            // A capture stays: buttons lift, motion waits for the picture.
                            let commands = desktop.capture.borrow_mut().frame_changed();
                            desktop.commands(commands);
                            desktop.painted_display = None;
                            desktop.release_pointer();
                        }
                        desktop.frame = frame;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            focus,
            waiting: SharedString::default(),
            scale_locally: true,
            frame: Frame::default(),
            painted: Rc::default(),
            painted_display: None,
            pinch_target,
            input,
            pointer,
            buttons: 0,
            position: None,
            keys: keys::KeyLedger::default(),
            scroll: (0.0, 0.0),
            finger_scroll_allowed: false,
            finger_scroll_task: None,
            settings: InputSettings::default(),
            cursor: None,
            marked: String::new(),
            selection: 0..0,
            ime_click: None,
            ime_bounds: None,
            hot_corner: false,
            buttons_corner: false,
            clipboard: ClipboardBridge::default(),
            mac_shortcuts: true,
            capture: Rc::default(),
            capture_monitor: None,
            cursor_lock: None,
            chord: capture::ChordGuard::default(),
            events,
        }
    }

    fn captured(&self) -> bool {
        self.capture.borrow().is_on()
    }

    /// The chord (or the menu) in the key session window. The chord's modifiers already
    /// reached the session, so they are lifted and muted until they come up (`ARCH-032`).
    fn toggle_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mac = window.modifiers();
        if !self.chord.press(capture::modifiers_held(mac)) {
            return;
        }
        let mut commands = self.keys.release_all();
        commands.extend(self.keys.mute_mods_until_clear(mac));
        self.commands(commands);
        if self.captured() {
            self.release_capture(capture::Release::Chord);
        } else {
            self.engage_capture(window, cx);
        }
    }

    /// Captures the pointer: `p,1`, the cursor frozen at the picture's centre and hidden,
    /// and the monitor sending relative motion. Only in an active window with a painted
    /// picture; with no picture yet, a hint says so.
    fn engage_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.captured() || !window.is_window_active() {
            return;
        }
        let Some(display) = self.painted_display else {
            let _ = self
                .events
                .send(SessionEvent::PointerCapture(Some(CaptureHint::Unavailable)));
            return;
        };
        let Some(native) = crate::app::native_window(window) else {
            return;
        };
        window.focus(&self.focus, cx);
        self.release_pointer();
        let commands = {
            let mut capture = self.capture.borrow_mut();
            capture.trackpad_scroll = self.settings.trackpad_scroll;
            capture.engage(capture::scale(display), capture::event_clock())
        };
        self.commands(commands);
        self.cursor_lock = Some(capture::CursorLock::lock(
            native.clone(),
            display.bounds.center(),
        ));
        self.capture_monitor =
            capture::Monitor::install(native, self.capture.clone(), self.input.clone());
        self.pinch_target.set(None);
        let _ = self
            .events
            .send(SessionEvent::PointerCapture(Some(CaptureHint::Captured)));
        cx.notify();
    }

    /// Ends a capture: `p,0`, the monitor removed, the cursor shown again. The only caller
    /// of `CursorLock::unlock`. Does nothing while not captured.
    fn release_capture(&mut self, reason: capture::Release) {
        let commands = self.capture.borrow_mut().release();
        if commands.is_empty() {
            return;
        }
        self.commands(commands);
        self.capture_monitor = None;
        if let Some(lock) = self.cursor_lock.take() {
            lock.unlock();
        }
        let _ = self
            .events
            .send(SessionEvent::PointerCapture(reason.hint()));
    }

    /// Send the Mac's clipboard to the session when it changed since it was last seen, or,
    /// `forced` by the deliberate button, whatever it holds.
    fn sync_to_session(&mut self, forced: bool) {
        let effective = self.clipboard.effective();
        if !effective.send || !(forced || effective.seamless) {
            return;
        }
        let count = pasteboard::change_count();
        if !forced && self.clipboard.seen == Some(count) {
            return;
        }
        self.clipboard.seen = Some(count);
        let Some(content) = pasteboard::read().filter(|content| effective.sends(content)) else {
            return;
        };
        let signature = content.signature();
        if !forced && self.clipboard.synced == Some(signature) {
            return;
        }
        self.clipboard.synced = Some(signature);
        let _ = self.input.send(Input::Clipboard(content));
    }

    fn receive_clipboard(&mut self, incoming: Incoming, cx: &mut Context<Self>) {
        let effective = self.clipboard.effective();
        let wanted = if incoming.reply {
            std::mem::take(&mut self.clipboard.fetch)
        } else {
            effective.seamless
        };
        if !effective.receive || !wanted {
            return;
        }
        // Any newer payload means the session's clipboard moved on from a secret.
        self.clipboard.retract_secret();
        let Some(content) = incoming.content else {
            return;
        };
        if matches!(content, Content::Image { .. }) && !effective.binary {
            return;
        }
        let signature = content.signature();
        if !incoming.reply
            && !incoming.secret
            && self.clipboard.synced == Some(signature)
            && self.clipboard.seen == Some(pasteboard::change_count())
        {
            return;
        }
        let count = pasteboard::write(Some(&content), incoming.secret);
        self.clipboard.seen = Some(count);
        self.clipboard.synced = Some(signature);
        if incoming.secret {
            let timer = cx.background_executor().timer(SECRET_TTL);
            let task = cx.spawn(async move |desktop, cx| {
                timer.await;
                let _ = desktop.update(cx, |desktop, _| {
                    if desktop
                        .clipboard
                        .secret
                        .as_ref()
                        .is_some_and(|(secret, _)| *secret == count)
                    {
                        desktop.clipboard.retract_secret();
                    }
                });
            });
            self.clipboard.secret = Some((count, task));
        }
    }

    fn command(&self, text: String) {
        let _ = self.input.send(Input::Command(text));
    }

    fn commands(&self, commands: Vec<String>) {
        for text in commands {
            self.command(text);
        }
    }

    fn mouse_position(&mut self, point: Point<Pixels>, clamp: bool) -> bool {
        self.position = self.painted_display.and_then(|display| {
            display
                .position(point, clamp)
                .map(|position| (display.resolution, position))
        });
        self.position.is_some()
    }

    fn mouse_state(&self) -> Option<Pointer> {
        let (display, position) = self.position?;
        Some(Pointer {
            display,
            position,
            buttons: self.buttons,
        })
    }

    fn send_pointer(&self) {
        if let Some(pointer) = self.mouse_state() {
            let _ = self.input.send(Input::Pointer(pointer));
        }
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) {
        let mask = match button {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 4,
            _ => return,
        };
        if down {
            self.buttons |= mask;
        } else {
            self.buttons &= !mask;
        }
        if let Some((display, _)) = self.pinch_target.get() {
            self.pinch_target.set(Some((display, self.buttons)));
        }
        self.pointer.send_replace(None);
        self.send_pointer();
    }

    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        if let ScrollDelta::Pixels(delta) = event.delta {
            if self.settings.trackpad_scroll && self.finger_scroll_allowed {
                if let Some(display) = self.painted_display {
                    let x = -f32::from(delta.x) * f32::from(display.resolution.0)
                        / f32::from(display.bounds.size.width);
                    let y = -f32::from(delta.y) * f32::from(display.resolution.1)
                        / f32::from(display.bounds.size.height);
                    if x != 0. || y != 0. {
                        self.command(format!("sf,{x:.2},{y:.2}"));
                        let idle = cx.background_executor().timer(Duration::from_millis(100));
                        self.finger_scroll_task = Some(cx.spawn(async move |this, cx| {
                            idle.await;
                            let _ = this.update(cx, |desktop, _| desktop.end_finger_scroll());
                        }));
                    }
                    return;
                }
            }
        }
        self.end_finger_scroll();
        let commands = capture::wheel_pulses(
            &mut self.scroll,
            event.delta,
            self.settings.trackpad_scroll,
            self.buttons,
        );
        self.commands(commands);
    }

    fn end_finger_scroll(&mut self) {
        if self.finger_scroll_task.take().is_some() {
            self.command("sfe".into());
        }
    }

    fn modifiers(&mut self, next: Modifiers) {
        let commands = self.keys.mac_modifiers_with(
            next,
            self.settings.command_as_control,
            &keys::physically_down,
        );
        self.commands(commands);
    }

    fn release_input(&mut self) {
        // Child windows, full screen, settings changes and the stream's end all pass here.
        self.release_capture(capture::Release::Closed);
        // Whatever took the keyboard, the next ⌃⌥⇧Z is a fresh press.
        self.chord = capture::ChordGuard::default();
        let commands = self.keys.release_all();
        self.commands(commands);
        self.marked.clear();
        self.selection = 0..0;
        self.ime_click = None;
        self.ime_bounds = None;
        self.scroll = (0., 0.);
        self.end_finger_scroll();
        self.release_pointer();
    }

    fn release_pointer(&mut self) {
        self.pinch_target.set(None);
        self.pointer.send_replace(None);
        self.position = None;
        if std::mem::take(&mut self.buttons) != 0 {
            self.command("m2,0,0,0,0".into());
        }
    }
}

impl EntityInputHandler for Desktop {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text: Vec<_> = self.marked.encode_utf16().collect();
        let slice = text.get(range.clone())?;
        let value = String::from_utf16(slice).ok()?;
        *adjusted = Some(range);
        Some(value)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.selection.clone(),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.marked.is_empty()).then(|| 0..self.marked.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked.clear();
        self.selection = 0..0;
        self.ime_bounds = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.mac_ime && self.focus.is_focused(window) && window.is_window_active() {
            if let Some(command) = commit_command(text) {
                self.modifiers(Modifiers::default());
                if let Some(key) = keysym(text, None) {
                    self.commands(self.keys.tap(key));
                } else {
                    self.command(command);
                }
            }
        }
        self.unmark_text(window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.mac_ime && self.focus.is_focused(window) && window.is_window_active() {
            if self.marked.is_empty() {
                let viewport = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
                self.ime_bounds = Some(composition_bounds(
                    viewport,
                    self.painted_display.map(|display| display.bounds),
                    self.ime_click,
                    window.mouse_position(),
                ));
            }
            self.marked = text.into();
            let len = text.encode_utf16().count();
            self.selection =
                selected.map_or(len..len, |range| range.start.min(len)..range.end.min(len));
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        element: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.ime_bounds.or_else(|| {
            Some(composition_bounds(
                element,
                self.painted_display.map(|display| display.bounds),
                self.ime_click,
                window.mouse_position(),
            ))
        })
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let desktop = cx.entity();
        let frame = self.frame.clone();
        let empty = frame.picture.is_none();
        let bounds = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
        let native_scale = (!self.scale_locally).then_some(window.scale_factor());
        let picture = picture_bounds(bounds, frame.width, frame.height, native_scale);
        let menu = picture
            .and_then(|picture| menu_position(bounds, picture))
            .or_else(|| self.hot_corner.then(|| corner_position(bounds)));
        let painted = self.painted.clone();
        let focus = self.focus.clone();
        let mac_ime = self.settings.mac_ime;
        let cursor = self.cursor.clone().filter(|_| self.settings.remote_cursor);
        let marked = self.marked.clone();
        let anchor = self
            .ime_bounds
            .unwrap_or_else(|| {
                composition_bounds(bounds, picture, self.ime_click, window.mouse_position())
            })
            .origin;
        let surface = canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, cx| {
                let display = frame.picture.as_ref().and_then(|_| {
                    picture_bounds(bounds, frame.width, frame.height, native_scale).map(|bounds| {
                        PaintedDisplay {
                            bounds,
                            resolution: Resolution(frame.width, frame.height),
                        }
                    })
                });
                let captured = desktop.update(cx, |desktop, _| {
                    desktop.painted_display = display;
                    let captured = desktop.captured();
                    if captured {
                        desktop
                            .capture
                            .borrow_mut()
                            .set_scale(display.and_then(capture::scale));
                    }
                    desktop.pinch_target.set(
                        display
                            .filter(|_| {
                                !captured && focus.is_focused(window) && window.is_window_active()
                            })
                            .map(|display| (display, desktop.buttons)),
                    );
                    captured
                });
                if mac_ime {
                    window.handle_input(
                        &focus,
                        ElementInputHandler::new(bounds, desktop.clone()),
                        cx,
                    );
                }
                let v = desktop.clone();
                let hb = hitbox.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && hb.is_hovered(window) {
                        v.update(cx, |desktop, cx| {
                            // Captured, the monitor owns the pointer; this is a defence.
                            if desktop.captured() {
                                return;
                            }
                            if desktop.mouse_position(event.position, false) {
                                window.focus(&desktop.focus, cx);
                                desktop.ime_click = Some(event.position);
                                desktop.modifiers(event.modifiers);
                                desktop.mouse_button(event.button, true);
                            }
                        });
                    }
                });
                let v = desktop.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        v.update(cx, |desktop, _| {
                            if desktop.captured() {
                                return;
                            }
                            if desktop.buttons != 0 && desktop.mouse_position(event.position, true)
                            {
                                desktop.mouse_button(event.button, false);
                            }
                        });
                    }
                });
                let v = desktop.clone();
                let hb = hitbox.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        let hovered = hb.is_hovered(window);
                        let viewport = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
                        let hot = in_hot_corner(viewport, event.position);
                        let buttons = in_buttons_corner(viewport, event.position);
                        v.update(cx, |desktop, cx| {
                            if desktop.captured() {
                                return;
                            }
                            if desktop.buttons_corner != buttons && !window.is_fullscreen() {
                                desktop.buttons_corner = buttons;
                                crate::app::reveal_window_buttons(window, buttons);
                            }
                            if desktop.settings.remote_cursor || desktop.hot_corner != hot {
                                desktop.hot_corner = hot;
                                cx.notify();
                            }
                            let dragging = desktop.buttons != 0;
                            if (hovered || dragging)
                                && desktop.mouse_position(event.position, dragging)
                            {
                                desktop.pointer.send_replace(desktop.mouse_state());
                            }
                        });
                    }
                });
                let v = desktop.clone();
                let hb = hitbox.clone();
                window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && hb.is_hovered(window) {
                        v.update(cx, |desktop, cx| {
                            if desktop.captured() {
                                return;
                            }
                            if desktop.mouse_position(event.position, false) {
                                desktop.pointer.send_replace(None);
                                desktop.send_pointer();
                                desktop.scroll(event, cx);
                            }
                        });
                    }
                });
                let mut shown = Vec::new();
                if let (Some(display), Some(picture)) = (display, &frame.picture) {
                    match picture {
                        Picture::Jpeg(jpeg) => {
                            let scale_factor = window.scale_factor();
                            for stripe in jpeg.stripes.iter() {
                                let (bounds, image_bounds) = stripes::stripe_bounds(
                                    display.bounds,
                                    scale_factor,
                                    frame.height,
                                    stripe.y,
                                    stripe.height,
                                );
                                let _ = window.paint_image(
                                    bounds,
                                    image_bounds,
                                    Corners::default(),
                                    stripe.image.clone(),
                                    0,
                                    false,
                                );
                                shown.push(stripe.image.clone());
                            }
                        }
                        Picture::Video(surface) => {
                            window.paint_surface(display.bounds, surface.pixel_buffer())
                        }
                    }
                }
                // Captured, the server composites the cursor (`p,1`): no second one here.
                if let (Some(cursor), Some(display), false) = (&cursor, display, captured) {
                    let mouse = window.mouse_position();
                    if window.is_window_active()
                        && focus.is_focused(window)
                        && hitbox.is_hovered(window)
                        && display.bounds.contains(&mouse)
                    {
                        unsafe {
                            let _: () = objc2::msg_send![
                                objc2::class!(NSCursor),
                                setHiddenUntilMouseMoves: true
                            ];
                        }
                        if let RemoteCursor::Image {
                            image,
                            hotspot,
                            size: (width, height),
                        } = cursor
                        {
                            let scale = f32::from(display.bounds.size.width)
                                / f32::from(display.resolution.0);
                            let position = point(
                                mouse.x - px(f32::from(hotspot.0) * scale),
                                mouse.y - px(f32::from(hotspot.1) * scale),
                            );
                            let image_bounds = Bounds::new(
                                position,
                                size(
                                    px(f32::from(*width) * scale),
                                    px(f32::from(*height) * scale),
                                ),
                            );
                            window.with_content_mask(
                                Some(ContentMask {
                                    bounds: display.bounds,
                                }),
                                |window| {
                                    let _ = window.paint_image(
                                        image_bounds,
                                        image_bounds,
                                        Corners::default(),
                                        image.clone(),
                                        0,
                                        false,
                                    );
                                },
                            );
                        }
                    }
                }
                for old in painted.borrow_mut().painted(&shown) {
                    let _ = window.drop_image(old);
                }
            },
        )
        .size_full();
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            // The Library's ⌘O / ⌘L are bound to "Library" only, so they never resolve here
            // and the keys reach the remote app (`ARCH-028`). With Mac shortcuts off, the
            // flag lets `shortcuts::suppress_bindings` hand Skerry's chords to the session
            // (`ARCH-031`).
            .key_context(if self.mac_shortcuts {
                DESKTOP_CONTEXT
            } else {
                shortcuts::REMOTE_DESKTOP_CONTEXT
            })
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|desktop, event: &KeyDownEvent, _, cx| {
                let stroke = &event.keystroke;
                // A Skerry chord GPUI suppressed: forwarded with ⌘ as Control, and always
                // claimed, or AppKit's menu key equivalent would run it locally.
                let pass = shortcuts::passes_to_session(stroke, desktop.mac_shortcuts);
                if pass {
                    cx.stop_propagation();
                    desktop.keys.latch_command();
                } else if !keys::sends_key_down(
                    stroke.modifiers,
                    desktop.settings.command_as_control,
                ) {
                    // ⌘O / ⌘L: claimed, so the File menu's key equivalents don't fire.
                    if keys::claims_library_shortcut(stroke.modifiers, &stroke.key) {
                        cx.stop_propagation();
                    }
                    return;
                }
                let shortcut = stroke.modifiers.control || stroke.modifiers.platform;
                // macOS can report an empty string for a dead key or an IME key.
                let text_input = stroke
                    .key_char
                    .as_deref()
                    .is_some_and(|text| text.chars().all(|ch| !ch.is_control()));
                if desktop.settings.mac_ime
                    && !shortcut
                    && (text_input || !desktop.marked.is_empty())
                {
                    return;
                }
                desktop.modifiers(stroke.modifiers);
                let text = (!shortcut).then_some(stroke.key_char.as_deref()).flatten();
                if let Some(key) = keysym(&stroke.key, text) {
                    let paste = (key == KEY_V && desktop.keys.control())
                        || (key == KEY_INSERT && desktop.keys.shift());
                    if paste {
                        // Ordered socket: the clipboard lands before the paste keystroke.
                        desktop.sync_to_session(false);
                    }
                    let commands = if pass {
                        desktop.keys.tap_chord(&event.keystroke.key, key)
                    } else {
                        desktop.keys.press(event.keystroke.key.clone(), key)
                    };
                    desktop.commands(commands);
                    cx.stop_propagation();
                }
            }))
            .on_key_up(cx.listener(|desktop, event: &KeyUpEvent, _, cx| {
                desktop.chord.key_up(&event.keystroke.key);
                let commands = desktop.keys.release(&event.keystroke.key);
                if !commands.is_empty() {
                    desktop.commands(commands);
                    cx.stop_propagation();
                }
            }))
            .on_modifiers_changed(cx.listener(|desktop, event: &ModifiersChangedEvent, _, _| {
                desktop
                    .chord
                    .modifiers(capture::modifiers_held(event.modifiers));
                desktop.modifiers(event.modifiers);
            }))
            .child(surface)
            .when(!marked.is_empty(), |view| {
                view.child(session::ime_preedit(anchor, marked))
            })
            .when_some(menu, |view, position| {
                view.child(session::controls_trigger(
                    position,
                    MENU_SIZE,
                    |_, window, cx| window.dispatch_action(Box::new(ToggleSessionControls), cx),
                ))
            })
            .when(empty, |view| {
                view.child(session::waiting_overlay(self.waiting.clone()))
            })
    }
}

#[derive(Clone, Copy)]
struct PaintedDisplay {
    bounds: Bounds<Pixels>,
    resolution: Resolution,
}

impl PaintedDisplay {
    #[cfg(test)]
    fn new(viewport: Bounds<Pixels>, resolution: Resolution) -> Option<Self> {
        let bounds = picture_bounds(viewport, resolution.0, resolution.1, None)?;
        Some(Self { bounds, resolution })
    }

    fn position(self, point: Point<Pixels>, clamp: bool) -> Option<(u16, u16)> {
        let bounds = self.bounds;
        if bounds.size.width <= px(0.)
            || bounds.size.height <= px(0.)
            || (!clamp && !bounds.contains(&point))
        {
            return None;
        }
        let coordinate = |fraction: f32, extent: u16| {
            (fraction * f32::from(extent)).clamp(0., f32::from(extent - 1)) as u16
        };
        Some((
            coordinate(
                (point.x - bounds.origin.x) / bounds.size.width,
                self.resolution.0,
            ),
            coordinate(
                (point.y - bounds.origin.y) / bounds.size.height,
                self.resolution.1,
            ),
        ))
    }
}

fn picture_bounds(
    bounds: Bounds<Pixels>,
    width: u16,
    height: u16,
    native_scale: Option<f32>,
) -> Option<Bounds<Pixels>> {
    if width == 0 || height == 0 {
        return None;
    }
    let scale = native_scale.map_or_else(
        || {
            (f32::from(bounds.size.width) / f32::from(width))
                .min(f32::from(bounds.size.height) / f32::from(height))
        },
        |backing_scale| 1.0 / backing_scale,
    );
    let width = px(f32::from(width) * scale);
    let height = px(f32::from(height) * scale);
    Some(Bounds::new(
        point(
            bounds.origin.x + (bounds.size.width - width) / 2.,
            bounds.origin.y + (bounds.size.height - height) / 2.,
        ),
        size(width, height),
    ))
}

fn corner_position(viewport: Bounds<Pixels>) -> Point<Pixels> {
    point(
        viewport.right() - MENU_MARGIN - MENU_SIZE,
        viewport.top() + MENU_MARGIN,
    )
}

/// Top-right square (trigger plus margin, doubled for easy targeting) that reveals the
/// Session Controls trigger over the picture.
fn in_hot_corner(viewport: Bounds<Pixels>, position: Point<Pixels>) -> bool {
    let reach = (MENU_SIZE + MENU_MARGIN * 2.) * 2.;
    position.x >= viewport.right() - reach && position.y <= viewport.top() + reach
}

/// Top-left area over the hidden close / minimize / zoom buttons that reveals them.
fn in_buttons_corner(viewport: Bounds<Pixels>, position: Point<Pixels>) -> bool {
    position.x <= viewport.left() + px(96.) && position.y <= viewport.top() + px(40.)
}

fn menu_position(viewport: Bounds<Pixels>, picture: Bounds<Pixels>) -> Option<Point<Pixels>> {
    let needed = MENU_SIZE + MENU_MARGIN * 2.;
    let top_fits = picture.top() - viewport.top() >= needed;
    let side_fits = viewport.right() - picture.right() >= needed;
    (viewport.size.width >= needed && viewport.size.height >= needed && (top_fits || side_fits))
        .then(|| {
            point(
                viewport.right() - MENU_MARGIN - MENU_SIZE,
                viewport.top() + MENU_MARGIN,
            )
        })
}

fn composition_bounds(
    viewport: Bounds<Pixels>,
    picture: Option<Bounds<Pixels>>,
    click: Option<Point<Pixels>>,
    pointer: Point<Pixels>,
) -> Bounds<Pixels> {
    let visible = picture
        .map(|bounds| viewport.intersect(&bounds))
        .unwrap_or(viewport);
    let anchor = click
        .filter(|point| visible.contains(point))
        .or_else(|| visible.contains(&pointer).then_some(pointer))
        .unwrap_or_else(|| {
            point(
                visible.left() + visible.size.width * 0.5,
                visible.top() + visible.size.height * 0.75,
            )
        });
    let height = px(22.).min(visible.size.height);
    Bounds::new(
        point(anchor.x, anchor.y.min(visible.bottom() - height)),
        size(px(1.), height),
    )
}

fn keysym(key: &str, text: Option<&str>) -> Option<u32> {
    let special = match key {
        "backspace" => 0xff08,
        "tab" => 0xff09,
        "enter" => 0xff0d,
        "escape" => 0xff1b,
        "home" => 0xff50,
        "left" => 0xff51,
        "up" => 0xff52,
        "right" => 0xff53,
        "down" => 0xff54,
        "pageup" => 0xff55,
        "pagedown" => 0xff56,
        "end" => 0xff57,
        "insert" => 0xff63,
        "delete" => 0xffff,
        "space" => 0x20,
        _ => {
            if let Some(n) = key
                .strip_prefix('f')
                .and_then(|suffix| suffix.parse::<u32>().ok())
                .filter(|n| (1..=12).contains(n))
            {
                return Some(0xffbd + n);
            }
            let value = text
                .filter(|s| s.chars().count() == 1 && !s.starts_with(char::is_control))
                .unwrap_or(key);
            let mut chars = value.chars();
            let one = chars.next()? as u32;
            return chars.next().is_none().then_some(if one <= 0xff {
                one
            } else {
                0x0100_0000 | one
            });
        }
    };
    Some(special)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where a Super regression would come from: no GPUI key name maps to Super or Meta.
    #[test]
    fn no_key_name_maps_to_super_or_meta() {
        let names = [
            "cmd",
            "command",
            "super",
            "meta",
            "platform",
            "win",
            "hyper",
            "shift",
            "control",
            "ctrl",
            "alt",
            "option",
            "fn",
            "function",
            "capslock",
            "escape",
            "tab",
            "space",
            "a",
            "z",
            "1",
            "f1",
            "f12",
            "f13",
            "left",
            "delete",
            "backspace",
            "enter",
        ];
        for name in names {
            for text in [None, Some(name)] {
                if let Some(key) = keysym(name, text) {
                    assert!(!(0xffe7..=0xffee).contains(&key), "{name} -> {key:#x}");
                }
            }
        }
    }

    #[test]
    fn ime_anchor_uses_click_pointer_or_visible_picture() {
        let viewport = Bounds::new(point(px(0.), px(0.)), size(px(800.), px(600.)));
        let picture = Bounds::new(point(px(100.), px(100.)), size(px(600.), px(400.)));
        let click = point(px(250.), px(200.));
        let pointer = point(px(350.), px(300.));
        let outside = point(px(5.), px(5.));
        assert_eq!(
            composition_bounds(viewport, Some(picture), Some(click), pointer).origin,
            click
        );
        assert_eq!(
            composition_bounds(viewport, Some(picture), Some(outside), pointer).origin,
            pointer
        );
        assert_eq!(
            composition_bounds(viewport, Some(picture), None, outside).origin,
            point(px(400.), px(400.))
        );
    }

    #[test]
    fn click_uses_the_geometry_of_the_picture_that_was_painted() {
        let viewport = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(800.)));
        let click = point(px(750.), px(400.));
        let normal = PaintedDisplay::new(viewport, Resolution(1000, 500)).unwrap();
        let retina = PaintedDisplay::new(viewport, Resolution(2000, 1000)).unwrap();
        assert_eq!(normal.position(click, false), Some((750, 250)));
        assert_eq!(retina.position(click, false), Some((1500, 500)));
        assert_eq!(retina.position(point(px(750.), px(100.)), false), None);
    }

    #[test]
    fn native_scale_centers_and_maps_a_picture_larger_than_the_window() {
        let viewport = Bounds::new(point(px(0.), px(0.)), size(px(600.), px(400.)));
        let bounds = picture_bounds(viewport, 1920, 1080, Some(2.)).unwrap();
        assert_eq!(
            bounds,
            Bounds::new(point(px(-180.), px(-70.)), size(px(960.), px(540.)))
        );
        assert!(menu_position(viewport, bounds).is_none());
    }

    #[test]
    fn menu_button_fits_only_outside_the_picture() {
        let viewport = |w, h| Bounds::new(point(px(0.), px(0.)), size(px(w), px(h)));
        let fit = |viewport| picture_bounds(viewport, 1280, 720, None).unwrap();
        let exact = viewport(1280., 720.);
        let narrow_margin = viewport(1280., 740.);
        assert!(menu_position(exact, fit(exact)).is_none());
        assert!(in_hot_corner(
            exact,
            point(exact.right() - px(10.), exact.top() + px(10.))
        ));
        assert!(!in_hot_corner(
            exact,
            point(exact.left() + px(10.), exact.top() + px(10.))
        ));
        assert!(!in_hot_corner(
            exact,
            point(exact.right() - px(10.), exact.bottom() - px(10.))
        ));
        assert!(menu_position(narrow_margin, fit(narrow_margin)).is_none());
        let tall = viewport(1280., 860.);
        let wide = viewport(1500., 720.);
        for available in [tall, wide] {
            let picture = fit(available);
            let position = menu_position(available, picture).unwrap();
            assert!(position.y + MENU_SIZE <= picture.top() || position.x >= picture.right());
            assert!(position.x + MENU_SIZE <= available.right());
        }
        assert!(picture_bounds(wide, 0, 0, None).is_none());
    }
}
