//! Display 2: a second window per session with its own stream claiming `display2`
//! (`ARCH-039`, `ARCH-GAP-VS-01`).
//!
//! The window lives inside its session window's lifetime, and its stream inside the primary
//! stream's: it waits while the primary is down and resumes on the primary's first
//! picture. Audio, capture, gamepads, stats, printing, files and the session → Mac
//! clipboard stay on the primary; Display 2 forwards those actions to it.
//!
//! A drag that starts in one window and crosses into the other (the web's
//! `Input._mapToLayout`) is **deferred**, not built: `window.bounds()` is relative to each
//! Mac display, so the mapping needs its own design (`ARCH-GAP-VS-07`). Selkies' own
//! drag self-heal covers the common case.

use super::session_window::SessionWindow;
use super::{
    dark_appearance, session_chrome, DisconnectSession, DisplayMode, ToggleApps, ToggleFiles,
    ToggleFullScreen, TogglePointerCapture, ToggleSecondDisplay, ToggleSessionControls,
    ToggleStatsStrip, APP_NAME,
};
use crate::clipboard::ClipboardSettings;
use crate::input::InputSettings;
use crate::settings::Key;
use crate::transport::{
    DisplayConfig, DisplayRequest, Kill, SessionEvent, SessionOptions, Side, StreamRole,
    SOCKET_STARTUP_GRACE,
};
use crate::video::Codec;
use crate::view::NativeSession;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    div, point, prelude::*, px, rgb, size, AnyWindowHandle, App, Bounds, Context, DisplayId,
    FocusHandle, Pixels, Task, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions,
};
use reqwest::Url;
use skerry_ui::controls::SecondScreenProps;
use skerry_ui::session;
use std::time::Duration;
use tokio::runtime::Handle;

/// A Mac display in global coordinates (`CGDisplayBounds`: the main display's top-left
/// is the origin, y grows down). GPUI's display bounds always have a zero origin, so the
/// arrangement must come from here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ScreenRect {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) w: f64,
    pub(super) h: f64,
}

impl ScreenRect {
    fn right(self) -> f64 {
        self.x + self.w
    }
    fn bottom(self) -> f64 {
        self.y + self.h
    }
    fn centre(self) -> (f64, f64) {
        (self.x + self.w / 2., self.y + self.h / 2.)
    }
}

/// The global rectangle of a GPUI display.
fn global_rect(id: DisplayId) -> ScreenRect {
    let rect = objc2_core_graphics::CGDisplayBounds(u64::from(id) as u32);
    ScreenRect {
        x: rect.origin.x,
        y: rect.origin.y,
        w: rect.size.width,
        h: rect.size.height,
    }
}

/// The display beside the session window's on each side, indexed by [`Side::index`].
pub(super) type Placements = [Option<DisplayId>; 4];

/// The web dashboard's rule (`dashboard.tsx` `addScreen`): a display is right of the
/// current one when its left edge is at or past the current right edge, and so on. Each
/// display counts for its first matching side only; each side keeps its first display.
pub(super) fn placements(current: ScreenRect, others: &[(DisplayId, ScreenRect)]) -> Placements {
    let mut sides: Placements = [None; 4];
    for &(id, other) in others {
        let side = if other.x >= current.right() {
            Side::Right
        } else if other.right() <= current.x {
            Side::Left
        } else if other.y >= current.bottom() {
            Side::Down
        } else if other.bottom() <= current.y {
            Side::Up
        } else {
            continue;
        };
        sides[side.index()].get_or_insert(id);
    }
    sides
}

/// The one side with a display, when exactly one has: Display 2 opens there unasked.
fn single_side(placements: &Placements) -> Option<(Side, DisplayId)> {
    let mut found = Side::ALL
        .into_iter()
        .filter_map(|side| placements[side.index()].map(|id| (side, id)));
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// Which side of `primary` the display `second` is on: the dominant axis of the centres.
pub(super) fn side_between(primary: ScreenRect, second: ScreenRect) -> Side {
    let (px_, py) = primary.centre();
    let (sx, sy) = second.centre();
    let (dx, dy) = (sx - px_, sy - py);
    if dx.abs() >= dy.abs() {
        if dx >= 0. {
            Side::Right
        } else {
            Side::Left
        }
    } else if dy >= 0. {
        Side::Down
    } else {
        Side::Up
    }
}

/// Display 2's mode follows the primary's: HiDPI stays HiDPI, anything else matches the
/// window (Display 2 always resizes, as the web's does).
fn second_mode(primary: DisplayMode) -> DisplayMode {
    match primary {
        DisplayMode::Retina => DisplayMode::Retina,
        _ => DisplayMode::MatchWindow,
    }
}

/// Display 2 beside the session window on the same Mac display: its size, offset 28 pt
/// toward `side`, clamped into the display's visible frame. Every rectangle here is
/// relative to that display, as GPUI's window bounds and `visible_bounds()` are.
pub(super) fn beside_bounds(
    session: Bounds<Pixels>,
    visible: Bounds<Pixels>,
    side: Side,
) -> Bounds<Pixels> {
    let step = px(28.);
    let size = size(
        session.size.width.min(visible.size.width),
        session.size.height.min(visible.size.height),
    );
    let (dx, dy) = match side {
        Side::Left => (-step, px(0.)),
        Side::Right => (step, px(0.)),
        Side::Up => (px(0.), -step),
        Side::Down => (px(0.), step),
    };
    let x = (session.origin.x + dx)
        .max(visible.origin.x)
        .min(visible.origin.x + visible.size.width - size.width);
    let y = (session.origin.y + dy)
        .max(visible.origin.y)
        .min(visible.origin.y + visible.size.height - size.height);
    Bounds::new(point(x, y), size)
}

/// Which of the session's windows is key. Pads follow the session while either is, and a
/// resign is applied after a short grace, so focus moving between the two windows never
/// drops the pads to rest (`ARCH-032`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct SessionKey {
    pub(super) primary: bool,
    pub(super) second: bool,
}

/// When a key change reaches the pads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeySync {
    Now,
    /// After [`KEY_HANDOFF`]: the other window may take key in between.
    AfterHandOff,
}

/// How long a resign waits for the session's other window to become key.
pub(super) const KEY_HANDOFF: Duration = Duration::from_millis(120);

impl SessionKey {
    pub(super) fn key(self) -> bool {
        self.primary || self.second
    }

    pub(super) fn set_primary(&mut self, active: bool, second_open: bool) -> KeySync {
        self.primary = active;
        if active || !second_open {
            KeySync::Now
        } else {
            KeySync::AfterHandOff
        }
    }

    pub(super) fn set_second(&mut self, active: bool) -> KeySync {
        self.second = active;
        if active {
            KeySync::Now
        } else {
            KeySync::AfterHandOff
        }
    }
}

/// Where Display 2's stream stands, as its window and the controls note show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SecondState {
    Connecting,
    Streaming,
    /// The primary stream is down; Display 2 resumes with it.
    Waiting,
    Ended,
    Refused(String),
    Superseded,
}

/// The controls note under "Close Screen 2".
fn note_text(state: &SecondState, side: Side, layout: Option<&DisplayConfig>) -> String {
    match state {
        SecondState::Streaming => match layout.and_then(|config| config.layouts.get("display2")) {
            Some(rect) => format!("Screen 2 · {} · {} × {}", side.label(), rect.w, rect.h),
            None => "Screen 2 · Connecting…".into(),
        },
        SecondState::Connecting => "Screen 2 · Connecting…".into(),
        SecondState::Waiting => "Screen 2 · Waiting for the main display…".into(),
        SecondState::Ended => "Screen 2 · Disconnected".into(),
        SecondState::Refused(_) => "Screen 2 · Not available".into(),
        SecondState::Superseded => "Screen 2 · Opened somewhere else".into(),
    }
}

/// Display 2's banner text, or `None` while it connects, streams or waits.
fn banner_text(state: &SecondState) -> Option<String> {
    match state {
        SecondState::Ended => Some("Display 2 disconnected.".into()),
        SecondState::Refused(reason) if reason.trim().is_empty() => {
            Some("Display 2 isn’t available.".into())
        }
        SecondState::Refused(reason) => {
            let reason = reason.trim();
            let stop = if reason.ends_with(['.', '!', '?']) {
                ""
            } else {
                "."
            };
            Some(format!("Display 2 isn’t available. {reason}{stop}"))
        }
        SecondState::Superseded => Some("Display 2 was opened somewhere else.".into()),
        _ => None,
    }
}

/// The current stream's generation: events from a stream already dropped are ignored, so
/// a late KILL or error never puts a banner on the stream that replaced it.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Streams {
    generation: u64,
}

impl Streams {
    fn begin(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }
    fn drop_current(&mut self) {
        self.generation += 1;
    }
    fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }
}

/// What Display 2's stream starts with, read from the session window at that moment (so
/// a codec change made since is followed).
pub(super) struct StreamSetup {
    pub(super) url: Url,
    pub(super) rt: Handle,
    pub(super) preferences: Vec<(Key, u32)>,
    pub(super) codec: Codec,
    pub(super) input: InputSettings,
    pub(super) clipboard: ClipboardSettings,
    pub(super) mac_shortcuts: bool,
    pub(super) mode: DisplayMode,
}

/// The session window's side of Display 2.
#[derive(Default)]
pub(super) struct SecondHost {
    display: Option<(AnyWindowHandle, WeakEntity<DisplayWindow>)>,
    /// The side Display 2 was opened on; `None` once the person closed it.
    wanted: Option<Side>,
    opening: bool,
    pub(super) key: SessionKey,
    /// The side picker is up, with these sides.
    placing: Option<Placements>,
    hovered: Option<usize>,
    picker_focus: Option<FocusHandle>,
    layout: Option<DisplayConfig>,
    /// Display 2 was resumed (or opened streaming) on this primary stream.
    resumed: bool,
    /// The mode Display 2 was last given.
    mode: Option<DisplayMode>,
    /// The bottom pill: text, generation, fading.
    pill: Option<(&'static str, u64, bool)>,
    pill_generation: u64,
    pill_task: Option<Task<()>>,
}

impl SecondHost {
    pub(super) fn is_open(&self) -> bool {
        self.display.is_some() || self.opening
    }

    pub(super) fn set_layout(&mut self, config: DisplayConfig) {
        self.layout = Some(config);
    }

    /// The primary stream ended (closed, failed, or KILLed): Display 2 waits for the next
    /// one, and resumes on its first picture.
    pub(super) fn on_primary_end(&mut self, cx: &mut App) {
        if self.primary_ended() {
            self.update_display(cx, |display, window, cx| display.suspend(window, cx));
        }
    }

    /// The pure half of [`Self::on_primary_end`]: whether a Display 2 needs suspending.
    fn primary_ended(&mut self) -> bool {
        self.resumed = false;
        self.layout = None;
        self.display.is_some() || self.opening
    }

    /// The primary's display mode changed: Display 2 follows it (HiDPI or Match Window).
    pub(super) fn follow_mode(&mut self, primary: DisplayMode, cx: &mut App) {
        let mode = second_mode(primary);
        if self.mode == Some(mode) {
            return;
        }
        self.mode = Some(mode);
        self.update_display(cx, move |display, window, cx| {
            display.set_mode(mode, window, cx)
        });
    }

    /// Whether the primary's first picture should resume Display 2: it is wanted and
    /// open, the server allows it, and it hasn't resumed on this stream yet. Marks it.
    pub(super) fn take_resume(&mut self, open: bool, second_screen: Option<bool>) -> bool {
        let resume = !self.resumed && open && self.wanted.is_some() && second_screen == Some(true);
        self.resumed |= resume;
        resume
    }

    /// Display 2's row in Session Controls.
    pub(super) fn props(&self, second_screen: Option<bool>, cx: &App) -> SecondScreenProps {
        let state = self
            .display
            .as_ref()
            .and_then(|(_, display)| display.upgrade())
            .map(|display| {
                let display = display.read(cx);
                (display.state.clone(), display.side)
            });
        SecondScreenProps {
            available: second_screen == Some(true),
            open: self.is_open(),
            note: self.is_open().then(|| match &state {
                Some((state, side)) => note_text(state, *side, self.layout.as_ref()),
                None => "Screen 2 · Connecting…".into(),
            }),
        }
    }

    /// Updates Display 2 through its own window, after the caller's update (`ARCH-015`).
    fn update_display(
        &self,
        cx: &mut App,
        f: impl FnOnce(&mut DisplayWindow, &mut Window, &mut Context<DisplayWindow>) + 'static,
    ) {
        let Some((handle, display)) = self.display.clone() else {
            return;
        };
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = display.update(cx, |display, cx| f(display, window, cx));
            });
        });
    }

    /// Sends the session's video changes to Display 2 too.
    pub(super) fn set_settings(&self, changes: Vec<(Key, u32)>, cx: &mut App) {
        if !changes.is_empty() {
            self.update_display(cx, move |display, _, _| {
                if let Some(stream) = &display.current {
                    stream.set_settings(changes);
                }
            });
        }
    }

    /// The session window is going away: Display 2 releases its keys, drops its stream
    /// (key releases, then the close frame), and its window closes.
    pub(super) fn close(&mut self, cx: &mut App) {
        self.wanted = None;
        self.placing = None;
        self.key.second = false;
        let Some((handle, display)) = self.display.take() else {
            return;
        };
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = display.update(cx, |display, cx| display.shutdown(window, cx));
                window.remove_window();
            });
        });
    }
}

impl SessionWindow {
    /// View → Second Display, or Session Controls' "Add Screen +" / "Close Screen 2".
    pub(super) fn toggle_second_display(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.second.is_open() {
            self.second.close(cx);
            self.sync_gamepads(cx);
            cx.notify();
            return;
        }
        match self
            .settings
            .as_ref()
            .and_then(|settings| settings.second_screen)
        {
            Some(true) => {}
            Some(false) => {
                return self.show_second_pill("This session doesn’t allow a second display.", cx)
            }
            None => {
                return self.show_second_pill(
                    "A second display is available once the session connects.",
                    cx,
                )
            }
        }
        let Some(current) = window.display(cx) else {
            return self.open_second_display(Side::Right, None, window, cx);
        };
        let current_id = current.id();
        let others: Vec<_> = cx
            .displays()
            .into_iter()
            .map(|display| display.id())
            .filter(|id| *id != current_id)
            .map(|id| (id, global_rect(id)))
            .collect();
        let found = placements(global_rect(current_id), &others);
        if let Some((side, target)) = single_side(&found) {
            return self.open_second_display(side, Some(target), window, cx);
        }
        // Several sides, or no other display: ask (no other display offers all four).
        let offered = if found.iter().all(Option::is_none) {
            [None; 4]
        } else {
            found
        };
        self.second.placing = Some(offered);
        self.second.hovered = None;
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.second.picker_focus = Some(focus);
        cx.notify();
    }

    /// The sides the picker offers, `None` while it is down.
    fn picker_sides(&self) -> Option<[bool; 4]> {
        let placing = self.second.placing?;
        Some(if placing.iter().all(Option::is_none) {
            [true; 4]
        } else {
            placing.map(|display| display.is_some())
        })
    }

    pub(super) fn pick_side(&mut self, side: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(placing) = self.second.placing.take() else {
            return;
        };
        let Some(side) = Side::from_index(side) else {
            return;
        };
        self.second.picker_focus = None;
        self.open_second_display(side, placing[side.index()], window, cx);
        if let Some(stream) = &self.current {
            stream.focus(window, cx);
        }
    }

    pub(super) fn dismiss_placement(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.second.placing = None;
        self.second.picker_focus = None;
        if let Some(stream) = &self.current {
            stream.focus(window, cx);
        }
        cx.notify();
    }

    /// Opens Display 2 on `target` (filling its visible frame), or beside this window on
    /// its own Mac display. Every bounds here is relative to the display it opens on.
    fn open_second_display(
        &mut self,
        side: Side,
        target: Option<DisplayId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target_display = target.and_then(|id| cx.displays().into_iter().find(|d| d.id() == id));
        let (bounds, display_id) = match target_display {
            Some(display) => (display.visible_bounds(), Some(display.id())),
            None => {
                let current = window.display(cx);
                let visible = current.as_ref().map_or_else(
                    || Bounds::new(point(px(0.), px(0.)), window.bounds().size),
                    |display| display.visible_bounds(),
                );
                (
                    beside_bounds(window.bounds(), visible, side),
                    current.map(|display| display.id()),
                )
            }
        };
        // Streams at once when the primary already shows a picture; waits otherwise.
        let setup = self.second_setup_when_streaming();
        self.second.resumed = setup.is_some();
        self.second.mode = Some(second_mode(self.display_mode));
        self.second.wanted = Some(side);
        self.second.opening = true;
        self.release_second_keys(cx);
        let owner = (Window::window_handle(window), cx.weak_entity());
        let session = cx.entity();
        let title = format!("{} — Display 2 — {APP_NAME}", self.remote.app_name);
        // Opened outside this update: GPUI must not render a new window while this one is
        // leased (`ARCH-006`).
        cx.defer(move |cx| {
            let opened = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    display_id,
                    window_min_size: Some(size(px(320.), px(240.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some(title.into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                move |window, cx| cx.new(|cx| DisplayWindow::new(owner, side, setup, window, cx)),
            );
            session.update(cx, |session, cx| {
                session.second.opening = false;
                match opened {
                    Ok((handle, display)) if session.second.wanted.is_some() => {
                        session.second.display = Some((handle, display.downgrade()));
                    }
                    // Closed (or the session went) while it was opening.
                    Ok((handle, display)) => {
                        cx.defer(move |cx| {
                            let _ = handle.update(cx, |_, window, cx| {
                                display.update(cx, |display, cx| display.shutdown(window, cx));
                                window.remove_window();
                            });
                        });
                    }
                    Err(_) => {
                        session.second.wanted = None;
                        session.show_second_pill("Couldn’t open Display 2.", cx);
                    }
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    /// The person closed Display 2 (its red button, Close, or the menu in it).
    pub(super) fn second_closed(&mut self, handle: AnyWindowHandle, cx: &mut Context<Self>) {
        if self
            .second
            .display
            .as_ref()
            .is_some_and(|(open, _)| *open == handle)
        {
            self.second.display = None;
            self.second.wanted = None;
            self.second.key.second = false;
            self.sync_gamepads(cx);
            cx.notify();
        }
    }

    /// Display 2 moved to another side of the primary.
    pub(super) fn second_moved(&mut self, side: Side) {
        if self.second.wanted.is_some() {
            self.second.wanted = Some(side);
        }
    }

    /// The primary showed its first picture: Display 2 resumes, reading the session's
    /// settings now.
    pub(super) fn second_on_picture(&mut self, cx: &mut Context<Self>) {
        let allowed = self.settings.as_ref().and_then(|s| s.second_screen);
        let open = self.second.display.is_some();
        if !self.second.take_resume(open, allowed) {
            return;
        }
        if let Some(setup) = self.second_setup() {
            self.second.update_display(cx, move |display, window, cx| {
                display.resume(setup, window, cx)
            });
        }
    }

    fn release_second_keys(&self, cx: &mut App) {
        if let Some(stream) = &self.current {
            stream.release_keys(cx);
        }
    }

    /// Shows `text` in the bottom pill for 3 s, replacing the capture hint.
    fn show_second_pill(&mut self, text: &'static str, cx: &mut Context<Self>) {
        self.clear_capture_hint();
        self.second.pill_generation += 1;
        let generation = self.second.pill_generation;
        self.second.pill = Some((text, generation, false));
        let executor = cx.background_executor().clone();
        self.second.pill_task = Some(cx.spawn(async move |this, cx| {
            executor.timer(Duration::from_millis(2800)).await;
            let _ = this.update(cx, |this, cx| {
                if let Some((_, current, fading)) = &mut this.second.pill {
                    if *current == generation {
                        *fading = true;
                        cx.notify();
                    }
                }
            });
            executor.timer(Duration::from_millis(200)).await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .second
                    .pill
                    .is_some_and(|(_, current, _)| current == generation)
                {
                    this.second.pill = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    /// A new capture hint replaces the pill: they share one slot.
    pub(super) fn clear_second_pill(&mut self) {
        self.second.pill = None;
        self.second.pill_task = None;
    }

    /// The picker and the pill, over the session window's picture.
    pub(super) fn second_overlays(
        &self,
        root: gpui_kit::Div,
        banner: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let theme = cx.theme().clone();
        let root = root.when_some(self.second.pill, |root, (text, generation, fading)| {
            root.child(session::notice_pill(
                text,
                session::capture_hint_bottom(banner),
                window.viewport_size().width,
                if fading {
                    session::HintFade::Out
                } else {
                    session::HintFade::In
                },
                generation,
            ))
        });
        let (Some(sides), Some(focus)) = (self.picker_sides(), self.second.picker_focus.clone())
        else {
            return root;
        };
        let pick = cx.weak_entity();
        let hover = cx.weak_entity();
        let dismiss = cx.weak_entity();
        let keys = cx.weak_entity();
        root.child(
            session::screen_placement(
                &theme,
                sides,
                self.second.hovered,
                std::rc::Rc::new(move |side, window, cx| {
                    let _ = pick.update(cx, |this, cx| this.pick_side(side, window, cx));
                }),
                std::rc::Rc::new(move |side, _, cx| {
                    let _ = hover.update(cx, |this, cx| {
                        if this.second.hovered != side {
                            this.second.hovered = side;
                            cx.notify();
                        }
                    });
                }),
                move |_, window, cx| {
                    let _ = dismiss.update(cx, |this, cx| this.dismiss_placement(window, cx));
                },
            )
            .track_focus(&focus)
            .on_key_down(move |event, window, cx| {
                let key = event.keystroke.key.clone();
                let _ = keys.update(cx, |this, cx| {
                    if key == "escape" {
                        this.dismiss_placement(window, cx);
                    } else if let Some(side) = session::placement_key(&key, sides) {
                        this.pick_side(side, window, cx);
                    }
                });
                cx.stop_propagation();
            }),
        )
    }
}

/// The Display 2 window.
pub(super) struct DisplayWindow {
    window: AnyWindowHandle,
    owner: (AnyWindowHandle, WeakEntity<SessionWindow>),
    side: Side,
    mode: DisplayMode,
    current: Option<NativeSession>,
    state: SecondState,
    streams: Streams,
    stream_events: Option<Task<()>>,
    applied: Option<DisplayRequest>,
    resize_task: Option<Task<()>>,
    move_task: Option<Task<()>>,
    fullscreen: bool,
    /// The Mac display this window was last seen on.
    mac_display: Option<DisplayId>,
}

impl DisplayWindow {
    fn new(
        owner: (AnyWindowHandle, WeakEntity<SessionWindow>),
        side: Side,
        setup: Option<StreamSetup>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            window: Window::window_handle(window),
            owner,
            side,
            mode: DisplayMode::MatchWindow,
            current: None,
            state: SecondState::Waiting,
            streams: Streams::default(),
            stream_events: None,
            applied: None,
            resize_task: None,
            move_task: None,
            fullscreen: window.is_fullscreen(),
            mac_display: window.display(cx).map(|display| display.id()),
        };
        let entity = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = entity.update(cx, |this, cx| this.closed_by_person(window, cx));
            true
        });
        cx.observe_window_bounds(window, |this, window, cx| {
            session_chrome(window, false);
            if this.fullscreen != window.is_fullscreen() {
                this.fullscreen = window.is_fullscreen();
                this.release_keys(cx);
            }
            this.resize(window, cx);
            this.follow_arrangement(window, cx);
        })
        .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            let (owner_window, session) = this.owner.clone();
            cx.defer(move |cx| {
                let _ = owner_window.update(cx, |_, _, cx| {
                    session.update(cx, |session, cx| session.set_second_active(active, cx))
                });
            });
        })
        .detach();
        session_chrome(window, false);
        if let Some(setup) = setup {
            this.open_stream(setup, window, cx);
        }
        this
    }

    fn open_stream(&mut self, setup: StreamSetup, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = second_mode(setup.mode);
        let request = self
            .mode
            .request(window.viewport_size(), window.scale_factor());
        let (events, mut incoming) = tokio::sync::mpsc::unbounded_channel();
        let stream = NativeSession::open(
            setup.url,
            window,
            cx,
            &setup.rt,
            SessionOptions {
                preferences: setup.preferences,
                dark: dark_appearance(window),
                muted: true,
                codec: setup.codec,
                display: request,
                stats: false,
                startup_grace: SOCKET_STARTUP_GRACE,
                waiting: "Connecting to Display 2…".into(),
                audio_output: None,
                audio_bitrate: None,
                role: StreamRole::Secondary(self.side),
            },
            events,
        );
        stream.set_input_settings(setup.input, window, cx);
        stream.set_clipboard_settings(setup.clipboard, window, cx);
        stream.set_mac_shortcuts(setup.mac_shortcuts, cx);
        self.current = Some(stream);
        self.applied = request;
        self.state = SecondState::Connecting;
        let generation = self.streams.begin();
        let handle = self.window;
        self.stream_events = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = incoming.recv().await {
                let updated = handle.update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| {
                        this.handle_event(generation, event, window, cx);
                        cx.notify();
                    })
                });
                if updated.map_or(true, |updated| updated.is_err()) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn handle_event(
        &mut self,
        generation: u64,
        event: SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.streams.is_current(generation) {
            return;
        }
        match event {
            SessionEvent::Video(true) => self.state = SecondState::Streaming,
            SessionEvent::Cursor(cursor) => {
                if let Some(stream) = &self.current {
                    stream.set_cursor(cursor, window, cx);
                }
            }
            SessionEvent::FingerScroll(enabled) => {
                if let Some(stream) = &self.current {
                    stream.allow_finger_scroll(enabled, cx);
                }
            }
            SessionEvent::ClipboardPolicy(policy) => {
                if let Some(stream) = &self.current {
                    stream.set_clipboard_policy(policy.for_secondary(), window, cx);
                }
            }
            SessionEvent::Killed(Kill::Superseded) => self.end(SecondState::Superseded, window, cx),
            SessionEvent::Killed(Kill::Refused(reason)) => {
                self.end(SecondState::Refused(reason), window, cx)
            }
            SessionEvent::Error(_) | SessionEvent::StartupTimeout => {
                self.end(SecondState::Ended, window, cx)
            }
            // Audio, stats, print, files, pads and the rest are the primary's.
            _ => {}
        }
    }

    /// Drops the stream: held keys are released first, then the close frame follows.
    fn stop_stream(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.streams.drop_current();
        self.stream_events = None;
        self.resize_task = None;
        self.applied = None;
        if let Some(stream) = self.current.take() {
            stream.release_focus(window, cx);
        }
    }

    fn end(&mut self, state: SecondState, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_stream(window, cx);
        self.state = state;
        cx.notify();
    }

    /// The primary stream ended: wait for it, with no stale picture.
    fn suspend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end(SecondState::Waiting, window, cx);
    }

    /// The primary's mode changed: resize to the new one now.
    fn set_mode(&mut self, mode: DisplayMode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        self.resize(window, cx);
    }

    fn resume(&mut self, setup: StreamSetup, window: &mut Window, cx: &mut Context<Self>) {
        if self.current.is_none() {
            self.open_stream(setup, window, cx);
        }
    }

    /// Ends the stream for good: the window is closing.
    fn shutdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.move_task = None;
        self.stop_stream(window, cx);
    }

    fn release_keys(&self, cx: &mut App) {
        if let Some(stream) = &self.current {
            stream.release_keys(cx);
        }
    }

    fn closed_by_person(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shutdown(window, cx);
        let handle = self.window;
        self.forward(false, cx, move |session, _, cx| {
            session.second_closed(handle, cx)
        });
    }

    /// Runs `f` on the session window, through its handle after this update (`ARCH-006`,
    /// `ARCH-015`), activating it first when `activate`.
    fn forward(
        &self,
        activate: bool,
        cx: &mut App,
        f: impl FnOnce(&mut SessionWindow, &mut Window, &mut Context<SessionWindow>) + 'static,
    ) {
        let (owner_window, session) = self.owner.clone();
        cx.defer(move |cx| {
            let _ = owner_window.update(cx, |_, window, cx| {
                if activate {
                    window.activate_window();
                }
                let _ = session.update(cx, |session, cx| f(session, window, cx));
            });
        });
    }

    /// Display 2 always resizes to its window (`r,WxH,display2`), 250 ms after it settles.
    fn resize(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.current.is_none() {
            return;
        }
        let request = self
            .mode
            .request(window.viewport_size(), window.scale_factor());
        if request == self.applied {
            self.resize_task = None;
            return;
        }
        let timer = cx.background_executor().timer(Duration::from_millis(250));
        self.resize_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, _| {
                if let (Some(stream), Some(request)) = (&this.current, request) {
                    stream.set_resolution(request);
                    this.applied = Some(request);
                }
            });
        }));
    }

    /// Moved onto another Mac display: once it settles there (500 ms), the side of the
    /// session window's display it is on now is sent, if it changed. Windows on the same
    /// Mac display keep the picked side.
    fn follow_arrangement(&mut self, window: &Window, cx: &mut Context<Self>) {
        let now = window.display(cx).map(|display| display.id());
        if now == self.mac_display {
            return;
        }
        self.mac_display = now;
        let Some(second) = now else {
            return;
        };
        let owner_window = self.owner.0;
        let timer = cx.background_executor().timer(Duration::from_millis(500));
        self.move_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let primary = owner_window
                .update(cx, |_, window, cx| {
                    window.display(cx).map(|display| display.id())
                })
                .ok()
                .flatten();
            let Some(primary) = primary.filter(|primary| *primary != second) else {
                return;
            };
            let side = side_between(global_rect(primary), global_rect(second));
            let _ = this.update(cx, |this, cx| {
                if this.mac_display != Some(second) || this.side == side {
                    return;
                }
                this.side = side;
                if let Some(stream) = &this.current {
                    stream.set_position(side, this.applied);
                }
                this.forward(false, cx, move |session, _, _| session.second_moved(side));
                cx.notify();
            });
        }));
    }

    /// Reconnect or Try Again: reopen once with the session's settings now.
    fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (owner_window, session) = self.owner.clone();
        let display = cx.weak_entity();
        let handle = self.window;
        self.state = SecondState::Connecting;
        cx.defer(move |cx| {
            // Only while the primary shows a picture: otherwise Display 2 waits for it.
            let setup = owner_window
                .update(cx, |_, _, cx| {
                    session.read_with(cx, |session, _| session.second_setup_when_streaming())
                })
                .ok()
                .and_then(Result::ok)
                .flatten();
            let _ = handle.update(cx, |_, window, cx| {
                let _ = display.update(cx, |display, cx| match setup {
                    Some(setup) => display.resume(setup, window, cx),
                    None => display.end(SecondState::Waiting, window, cx),
                });
            });
        });
        let _ = window;
        cx.notify();
    }

    fn close_from_banner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.closed_by_person(window, cx);
        window.remove_window();
    }
}

impl Render for DisplayWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let banner = banner_text(&self.state);
        let retry_label = match self.state {
            SecondState::Refused(_) => "Try Again",
            _ => "Reconnect",
        };
        let waiting = match self.state {
            SecondState::Waiting => Some("Waiting for the main display…"),
            SecondState::Connecting if self.current.is_none() => Some("Connecting to Display 2…"),
            _ => None,
        };
        let retry = cx.weak_entity();
        let close = cx.weak_entity();
        div()
            .size_full()
            .relative()
            .bg(rgb(0x000000))
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &ToggleFullScreen, window, cx| {
                this.release_keys(cx);
                window.toggle_fullscreen();
            }))
            .on_action(cx.listener(|this, _: &ToggleSecondDisplay, window, cx| {
                this.close_from_banner(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleSessionControls, _, cx| {
                this.release_keys(cx);
                this.forward(true, cx, |session, window, cx| {
                    session.open_controls_from_second(window, cx)
                });
            }))
            .on_action(cx.listener(|this, _: &ToggleFiles, _, cx| {
                this.forward(true, cx, |session, window, cx| {
                    session.toggle_files(window, cx)
                });
            }))
            .on_action(cx.listener(|this, _: &ToggleApps, _, cx| {
                this.forward(true, cx, |session, window, cx| {
                    session.toggle_apps(window, cx)
                });
            }))
            .on_action(cx.listener(|this, _: &ToggleStatsStrip, _, cx| {
                this.forward(true, cx, |session, _, cx| session.toggle_stats_strip(cx));
            }))
            // Capture is the primary's: it activates and captures there (`ARCH-032`).
            .on_action(cx.listener(|this, _: &TogglePointerCapture, _, cx| {
                this.release_keys(cx);
                this.forward(false, cx, |session, window, cx| {
                    session.request_capture(window, cx)
                });
            }))
            .on_action(cx.listener(|this, _: &DisconnectSession, _, cx| {
                this.forward(false, cx, |session, window, cx| {
                    session.disconnect(window, cx)
                });
            }))
            .when_some(self.current.as_ref(), |root, stream| {
                root.child(stream.view())
            })
            .when_some(waiting, |root, label| {
                root.child(session::waiting_overlay(label))
            })
            .when_some(banner, |root, text| {
                root.child(session::status_banner_labeled(
                    &theme,
                    text,
                    (
                        retry_label,
                        Box::new(move |_, window, cx| {
                            let _ = retry.update(cx, |this, cx| this.retry(window, cx));
                        }),
                    ),
                    (
                        "Close",
                        Box::new(move |_, window, cx| {
                            let _ = close.update(cx, |this, cx| this.close_from_banner(window, cx));
                        }),
                    ),
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> ScreenRect {
        ScreenRect { x, y, w, h }
    }

    fn id(n: u32) -> DisplayId {
        DisplayId::from(u64::from(n))
    }

    #[test]
    fn one_display_on_the_right_opens_there_without_asking() {
        let found = placements(
            rect(0., 0., 1512., 982.),
            &[(id(2), rect(1512., 0., 1920., 1080.))],
        );
        assert_eq!(single_side(&found), Some((Side::Right, id(2))));
    }

    #[test]
    fn displays_on_two_sides_offer_both() {
        let found = placements(
            rect(0., 0., 1512., 982.),
            &[
                (id(2), rect(-1920., 0., 1920., 1080.)),
                (id(3), rect(0., -1080., 1920., 1080.)),
            ],
        );
        assert_eq!(found, [Some(id(2)), None, Some(id(3)), None]);
        assert_eq!(single_side(&found), None);
    }

    #[test]
    fn no_other_display_offers_all_four_sides() {
        let found = placements(rect(0., 0., 1512., 982.), &[]);
        assert_eq!(found, [None; 4]);
        assert_eq!(single_side(&found), None);
    }

    #[test]
    fn a_display_offset_diagonally_counts_for_one_side_only() {
        // Below and to the right: right wins, as in the web's order.
        let found = placements(
            rect(0., 0., 1512., 982.),
            &[(id(2), rect(1600., 1000., 1920., 1080.))],
        );
        assert_eq!(found, [None, Some(id(2)), None, None]);
    }

    #[test]
    fn placements_use_global_display_origins() {
        // A display left of and above the main one has negative global origins.
        let found = placements(
            rect(0., 0., 1512., 982.),
            &[(id(2), rect(-2560., -400., 2560., 1440.))],
        );
        assert_eq!(single_side(&found), Some((Side::Left, id(2))));
        // GPUI's zero origins would put every display on top of the current one.
        let zero = placements(
            rect(0., 0., 1512., 982.),
            &[(id(2), rect(0., 0., 2560., 1440.))],
        );
        assert_eq!(zero, [None; 4]);
    }

    #[test]
    fn target_display_bounds_are_display_relative() {
        // The session window at (100, 80) on a display whose visible frame starts below
        // the menu bar; every value is relative to that display.
        let session = Bounds::new(point(px(100.), px(80.)), size(px(1200.), px(800.)));
        let visible = Bounds::new(point(px(0.), px(25.)), size(px(1512.), px(957.)));
        let beside = beside_bounds(session, visible, Side::Right);
        assert_eq!(beside.origin, point(px(128.), px(80.)));
        assert_eq!(beside.size, session.size);
    }

    #[test]
    fn beside_bounds_stay_inside_the_visible_frame() {
        let visible = Bounds::new(point(px(0.), px(25.)), size(px(1512.), px(957.)));
        let session = Bounds::new(point(px(400.), px(25.)), size(px(1200.), px(900.)));
        let beside = beside_bounds(session, visible, Side::Up);
        assert_eq!(beside.origin.y, px(25.));
        assert!(beside.origin.x + beside.size.width <= px(1512.));
        let huge = Bounds::new(point(px(0.), px(0.)), size(px(3000.), px(2000.)));
        assert_eq!(beside_bounds(huge, visible, Side::Left), visible);
    }

    #[test]
    fn side_follows_the_display_the_window_moved_to() {
        let main = rect(0., 0., 1512., 982.);
        assert_eq!(
            side_between(main, rect(1512., 0., 1920., 1080.)),
            Side::Right
        );
        assert_eq!(
            side_between(main, rect(-1920., 0., 1920., 1080.)),
            Side::Left
        );
        assert_eq!(side_between(main, rect(0., -1080., 1920., 1080.)), Side::Up);
        assert_eq!(
            side_between(main, rect(-200., 982., 1920., 1080.)),
            Side::Down
        );
    }

    #[test]
    fn same_display_keeps_the_picked_side() {
        // The arrangement is followed only across Mac displays: a move within the same
        // display has no other display to measure against, and the picked side stays.
        let host = SecondHost {
            wanted: Some(Side::Left),
            ..Default::default()
        };
        assert_eq!(host.wanted, Some(Side::Left));
    }

    #[test]
    fn secondary_mode_follows_primary_retina_or_match_window() {
        assert!(matches!(
            second_mode(DisplayMode::Retina),
            DisplayMode::Retina
        ));
        assert!(matches!(
            second_mode(DisplayMode::Fit),
            DisplayMode::MatchWindow
        ));
        assert!(matches!(
            second_mode(DisplayMode::Fixed(crate::transport::Resolution(800, 600))),
            DisplayMode::MatchWindow
        ));
    }

    #[test]
    fn session_is_key_while_either_window_is() {
        let mut key = SessionKey::default();
        assert!(!key.key());
        key.set_second(true);
        assert!(key.key());
        key.set_primary(true, true);
        key.set_second(false);
        assert!(key.key());
    }

    #[test]
    fn focus_moving_to_display_two_does_not_rest_pads() {
        // Primary resigns first: with Display 2 open the rest waits for the hand-off.
        let mut key = SessionKey {
            primary: true,
            second: false,
        };
        assert_eq!(key.set_primary(false, true), KeySync::AfterHandOff);
        // Display 2 becomes key within the grace: synced at once, still key.
        assert_eq!(key.set_second(true), KeySync::Now);
        assert!(key.key());
        // Without Display 2, a resign rests at once, as before.
        let mut alone = SessionKey {
            primary: true,
            second: false,
        };
        assert_eq!(alone.set_primary(false, false), KeySync::Now);
        assert!(!alone.key());
    }

    #[test]
    fn display_two_resumes_only_when_wanted_and_allowed() {
        let mut host = SecondHost::default();
        // Not wanted.
        assert!(!host.take_resume(true, Some(true)));
        host.wanted = Some(Side::Right);
        assert!(!host.take_resume(true, Some(false)));
        assert!(!host.take_resume(true, None));
        assert!(!host.take_resume(false, Some(true)));
        assert!(host.take_resume(true, Some(true)));
        // Once per primary stream.
        assert!(!host.take_resume(true, Some(true)));
    }

    #[test]
    fn display_two_resumes_when_allowed_after_the_picture() {
        let mut host = SecondHost {
            wanted: Some(Side::Right),
            ..Default::default()
        };
        // The picture came first, with the server not yet saying.
        assert!(!host.take_resume(true, None));
        // A later server_settings allows it: the Settings arm resumes once.
        assert!(host.take_resume(true, Some(true)));
        assert!(!host.take_resume(true, Some(true)));
    }

    #[test]
    fn a_primary_error_or_kill_suspends_display_two() {
        let mut host = SecondHost {
            wanted: Some(Side::Right),
            opening: true,
            ..Default::default()
        };
        assert!(host.take_resume(true, Some(true)));
        // The primary's Error arm (KILL routes through it) ends the primary for Display 2.
        assert!(host.primary_ended());
        assert!(host.layout.is_none());
        // Its next picture resumes Display 2 again.
        assert!(host.take_resume(true, Some(true)));
        // Nothing to suspend without a Display 2.
        assert!(!SecondHost::default().primary_ended());
    }

    #[test]
    fn a_bare_kill_reads_without_stray_punctuation() {
        assert_eq!(
            banner_text(&SecondState::Refused(String::new())).unwrap(),
            "Display 2 isn’t available."
        );
    }

    #[test]
    fn a_refused_display_two_retries_on_primary_reconnect() {
        let mut host = SecondHost {
            wanted: Some(Side::Right),
            ..Default::default()
        };
        assert!(host.take_resume(true, Some(true)));
        // The primary reconnects: its stream ended, and its next picture resumes again.
        host.resumed = false;
        host.layout = None;
        assert!(host.take_resume(true, Some(true)));
    }

    #[test]
    fn a_late_event_from_a_dropped_display_two_stream_is_ignored() {
        let mut streams = Streams::default();
        let first = streams.begin();
        assert!(streams.is_current(first));
        streams.drop_current();
        assert!(!streams.is_current(first));
        let second = streams.begin();
        assert!(streams.is_current(second));
        assert!(!streams.is_current(first));
    }

    #[test]
    fn notes_and_banners_read_as_designed() {
        let mut config = DisplayConfig::default();
        config.layouts.insert(
            "display2".into(),
            crate::transport::LayoutRect {
                x: 1920,
                y: 0,
                w: 1920,
                h: 1080,
            },
        );
        assert_eq!(
            note_text(&SecondState::Streaming, Side::Right, Some(&config)),
            "Screen 2 · Right · 1920 × 1080"
        );
        assert_eq!(
            note_text(&SecondState::Streaming, Side::Up, None),
            "Screen 2 · Connecting…"
        );
        assert_eq!(
            banner_text(&SecondState::Refused(
                "Second screens are disabled on this server.".into()
            ))
            .unwrap(),
            "Display 2 isn’t available. Second screens are disabled on this server."
        );
        assert_eq!(
            banner_text(&SecondState::Refused("No outputs".into())).unwrap(),
            "Display 2 isn’t available. No outputs."
        );
        assert_eq!(banner_text(&SecondState::Streaming), None);
    }
}
