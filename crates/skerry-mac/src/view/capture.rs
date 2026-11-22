//! Capture Pointer (`ARCH-032`): relative `m2` motion with the Mac cursor frozen and hidden.
//!
//! [`Capture`] is the pure core: what `p`, `SET_NATIVE_CURSOR_RENDERING` and `m2` commands
//! each step sends. While captured, an AppKit local [`Monitor`] owns the whole pointer: it
//! swallows motion, buttons and wheel app-wide while the session window is key, and sends
//! straight to the transport's input channel, as `pinch.rs` does. It never calls into GPUI.
//! [`CursorLock`] warps, dissociates and hides the cursor, and undoes exactly that.

use super::PaintedDisplay;
use crate::transport::Input;
use block2::RcBlock;
use gpui_kit::{px, Pixels, Point, ScrollDelta};
use objc2::{rc::Retained, runtime::AnyObject, MainThreadMarker};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType, NSWindow};
use objc2_core_graphics::{
    CGAssociateMouseAndMouseCursorPosition, CGDisplayBounds, CGMainDisplayID,
    CGWarpMouseCursorPosition,
};
use objc2_foundation::{NSPoint, NSProcessInfo, NSRect};
use std::{
    cell::{Cell, RefCell},
    ptr::{null_mut, NonNull},
    rc::Rc,
};
use tokio::sync::mpsc;

/// The pure state of one captured pointer, shared by `Desktop` and the [`Monitor`].
#[derive(Debug, Default)]
pub(super) struct Capture {
    on: bool,
    /// The sub-pixel remainder of each axis, carried into the next motion (the web's
    /// `_quantizeRelative`).
    carry: (f64, f64),
    /// The buttons held while captured; `Desktop.buttons` stays 0 meanwhile.
    buttons: u8,
    /// Remote px per painted pt; `None` while no picture is painted.
    scale: Option<(f64, f64)>,
    /// The `NSEvent` clock when the cursor was locked. Motion older than this, and the first
    /// motion after it (AppKit can fold the warp's jump into it), is dropped.
    locked_at: f64,
    first_motion: bool,
    /// The wheel's fractional steps, as `Desktop::scroll` keeps them.
    scroll: (f32, f32),
    /// Settings the wheel follows: pixel deltas count 12 pt per step with trackpad
    /// scrolling on, 40 otherwise.
    pub(super) trackpad_scroll: bool,
}

/// One mouse event the [`Monitor`] sees.
#[derive(Clone, Copy, Debug)]
pub(super) enum Event {
    Motion { dx: f64, dy: f64, timestamp: f64 },
    Button { mask: Option<u8>, down: bool },
    Wheel(ScrollDelta),
}

/// What the [`Monitor`] does with an event.
#[derive(Debug, PartialEq)]
pub(super) enum Route {
    /// Eat it (GPUI never sees it) and send these commands.
    Swallow(Vec<String>),
    Pass,
}

impl Capture {
    /// `p,1` then `SET_NATIVE_CURSOR_RENDERING,1` (the web sends both on every lock
    /// change; the second is a no-op alias on the server). Empty when already on. The carry
    /// restarts, as the web's does on both edges.
    pub(super) fn engage(&mut self, scale: Option<(f64, f64)>, now: f64) -> Vec<String> {
        if self.on {
            return Vec::new();
        }
        self.on = true;
        self.carry = (0., 0.);
        self.buttons = 0;
        self.scroll = (0., 0.);
        self.scale = scale;
        self.locked_at = now;
        self.first_motion = true;
        vec!["p,1".into(), "SET_NATIVE_CURSOR_RENDERING,1".into()]
    }

    /// `p,0`, `SET_NATIVE_CURSOR_RENDERING,0`, then `m2,0,0,0,0` once if buttons were held.
    /// Idempotent.
    pub(super) fn release(&mut self) -> Vec<String> {
        if !self.on {
            return Vec::new();
        }
        self.on = false;
        self.carry = (0., 0.);
        self.scroll = (0., 0.);
        let mut commands = vec!["p,0".into(), "SET_NATIVE_CURSOR_RENDERING,0".into()];
        commands.extend(self.lift_buttons());
        commands
    }

    pub(super) fn is_on(&self) -> bool {
        self.on
    }

    /// The painted picture changed: the scale follows, and with no picture motion is
    /// dropped until one is painted again.
    pub(super) fn set_scale(&mut self, scale: Option<(f64, f64)>) {
        if scale.is_none() {
            self.carry = (0., 0.);
        }
        self.scale = scale;
    }

    /// The remote frame changed size while captured: held buttons are lifted, and motion
    /// waits for the next painted picture (the capture itself stays).
    pub(super) fn frame_changed(&mut self) -> Vec<String> {
        if !self.on {
            return Vec::new();
        }
        self.set_scale(None);
        self.lift_buttons()
    }

    fn lift_buttons(&mut self) -> Vec<String> {
        if std::mem::take(&mut self.buttons) != 0 {
            vec!["m2,0,0,0,0".into()]
        } else {
            Vec::new()
        }
    }

    /// A relative move in Mac points, scaled to remote pixels and quantized with the carry
    /// (`m2` takes integers only). `None` when it rounds to nothing.
    pub(super) fn motion(&mut self, dx: f64, dy: f64, timestamp: f64) -> Option<String> {
        if !self.on {
            return None;
        }
        if std::mem::take(&mut self.first_motion) || timestamp < self.locked_at {
            return None;
        }
        let (sx, sy) = self.scale?;
        let x = dx * sx + self.carry.0;
        let y = dy * sy + self.carry.1;
        let (qx, qy) = (x.round(), y.round());
        self.carry = (x - qx, y - qy);
        if !self.carry.0.is_finite() || !self.carry.1.is_finite() {
            self.carry = (0., 0.);
            return None;
        }
        let (qx, qy) = (qx as i64, qy as i64);
        (qx != 0 || qy != 0).then(|| format!("m2,{qx},{qy},{},0", self.buttons))
    }

    /// A button went down or up: `m2,0,0,{mask},0`, with no position.
    pub(super) fn button(&mut self, mask: u8, down: bool) -> String {
        if down {
            self.buttons |= mask;
        } else {
            self.buttons &= !mask;
        }
        button_command(self.buttons)
    }

    /// Where an event goes: while captured and the session window is key, motion, buttons
    /// and wheel are swallowed (app-wide, so a click over the floating Session Controls
    /// panel can't take key and drop the capture). Everything else passes.
    pub(super) fn route(&mut self, event: Event, session_key: bool) -> Route {
        if !self.on || !session_key {
            return Route::Pass;
        }
        Route::Swallow(match event {
            Event::Motion { dx, dy, timestamp } => {
                self.motion(dx, dy, timestamp).into_iter().collect()
            }
            Event::Button { mask, down } => mask
                .map(|mask| vec![self.button(mask, down)])
                .unwrap_or_default(),
            Event::Wheel(delta) => {
                wheel_pulses(&mut self.scroll, delta, self.trackpad_scroll, self.buttons)
            }
        })
    }
}

/// Buttons alone, as `m2` with no movement (Selkies' `static_relative`).
pub(super) fn button_command(buttons: u8) -> String {
    format!("m2,0,0,{buttons},0")
}

/// The Selkies button mask of an `NSEvent` button number.
pub(super) fn button_mask(number: isize) -> Option<u8> {
    match number {
        0 => Some(1),
        2 => Some(2),
        1 => Some(4),
        _ => None,
    }
}

/// Wheel steps as `m2` pulses (button 4/5 up/down, 6/7 left/right), carrying the fraction
/// in `scroll`. Pixel deltas count `12` pt per step with trackpad scrolling, `40` without.
pub(super) fn wheel_pulses(
    scroll: &mut (f32, f32),
    delta: ScrollDelta,
    trackpad_scroll: bool,
    buttons: u8,
) -> Vec<String> {
    let (x, y) = match delta {
        ScrollDelta::Lines(p) => (p.x, p.y),
        ScrollDelta::Pixels(p) => {
            let points_per_step = if trackpad_scroll { 12. } else { 40. };
            (
                f32::from(p.x) / points_per_step,
                f32::from(p.y) / points_per_step,
            )
        }
    };
    scroll.0 += x;
    scroll.1 += y;
    let (x, y) = (scroll.0.trunc(), scroll.1.trunc());
    scroll.0 -= x;
    scroll.1 -= y;
    let mut commands = Vec::new();
    for (delta, positive, negative) in [(x, 64, 128), (y, 16, 8)] {
        if delta == 0. || !delta.is_finite() {
            continue;
        }
        let mask = buttons | if delta > 0. { positive } else { negative };
        let amount = (delta.abs() as u32).min(16);
        commands.push(format!("m2,0,0,{mask},{amount}"));
        commands.push(button_command(buttons));
    }
    commands
}

/// Remote px per painted pt, on both axes.
pub(super) fn scale(display: PaintedDisplay) -> Option<(f64, f64)> {
    let width = f64::from(f32::from(display.bounds.size.width));
    let height = f64::from(f32::from(display.bounds.size.height));
    (width > 0. && height > 0.).then(|| {
        (
            f64::from(display.resolution.0) / width,
            f64::from(display.resolution.1) / height,
        )
    })
}

/// The Quartz global point (top-left origin, y down) of `centre`, a point in a view whose
/// screen rect is `content` (Cocoa: bottom-left origin, y up). Quartz flips with the height
/// of the **primary** display, the one at (0, 0), whichever screen the window is on.
pub(super) fn warp_point(content: NSRect, centre: Point<Pixels>, primary_height: f64) -> NSPoint {
    let x = content.origin.x + f64::from(f32::from(centre.x));
    let cocoa_y = content.origin.y + content.size.height - f64::from(f32::from(centre.y));
    NSPoint::new(x, primary_height - cocoa_y)
}

/// The capture's notice over the picture (`session::capture_hint`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureHint {
    Captured,
    Released,
    /// The chord or the menu fired before the picture appeared.
    Unavailable,
}

impl CaptureHint {
    /// The hint's words, the chord drawn from `shortcuts::CHORDS`, and how long it shows.
    pub fn text(self) -> (skerry_ui::session::CaptureHintText, std::time::Duration) {
        use skerry_ui::session::CaptureHintText;
        use std::time::Duration;
        let chord = crate::shortcuts::chord(crate::shortcuts::ChordId::CapturePointer);
        let (modifiers, key) = crate::shortcuts::glyphs(chord.keystroke);
        match self {
            CaptureHint::Captured => (
                CaptureHintText {
                    lead: "Pointer captured · ".into(),
                    chord: Some(format!("{modifiers}{key}").into()),
                    tail: " releases it".into(),
                },
                Duration::from_millis(3000),
            ),
            CaptureHint::Released => (
                CaptureHintText {
                    lead: "Pointer released".into(),
                    chord: None,
                    tail: "".into(),
                },
                Duration::from_millis(1500),
            ),
            CaptureHint::Unavailable => (
                CaptureHintText {
                    lead: "Capture Pointer is available once the picture appears".into(),
                    chord: None,
                    tail: "".into(),
                },
                Duration::from_millis(1500),
            ),
        }
    }
}

/// Why a capture ended. Only the person's own releases (and a deactivation that leaves the
/// window on screen) show the "Pointer released" hint; the status banner explains the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Release {
    Chord,
    FocusLost,
    Window { visible: bool },
    Disconnected,
    Closed,
}

impl Release {
    pub(super) fn hint(self) -> Option<CaptureHint> {
        match self {
            Release::Chord | Release::FocusLost | Release::Window { visible: true } => {
                Some(CaptureHint::Released)
            }
            Release::Window { visible: false } | Release::Disconnected | Release::Closed => None,
        }
    }
}

/// The `NSEvent` clock: seconds since the Mac started.
pub(super) fn event_clock() -> f64 {
    NSProcessInfo::processInfo().systemUptime()
}

/// The local monitor that owns the pointer while captured. Installed on capture, dropped on
/// release (its `Drop` removes it).
pub(super) struct Monitor(Retained<AnyObject>);

impl Monitor {
    pub(super) fn install(
        window: Retained<NSWindow>,
        capture: Rc<RefCell<Capture>>,
        input: mpsc::UnboundedSender<Input>,
    ) -> Option<Self> {
        let handler = RcBlock::new(move |event: NonNull<NSEvent>| {
            let Some(_main) = MainThreadMarker::new() else {
                return event.as_ptr();
            };
            // The AppKit monitor lends the event for the duration of this callback.
            let native = unsafe { event.as_ref() };
            let kind = native.r#type();
            let seen = if kind == NSEventType::MouseMoved
                || kind == NSEventType::LeftMouseDragged
                || kind == NSEventType::RightMouseDragged
                || kind == NSEventType::OtherMouseDragged
            {
                Event::Motion {
                    dx: native.deltaX(),
                    dy: native.deltaY(),
                    timestamp: native.timestamp(),
                }
            } else if kind == NSEventType::LeftMouseDown
                || kind == NSEventType::RightMouseDown
                || kind == NSEventType::OtherMouseDown
            {
                Event::Button {
                    mask: button_mask(native.buttonNumber()),
                    down: true,
                }
            } else if kind == NSEventType::LeftMouseUp
                || kind == NSEventType::RightMouseUp
                || kind == NSEventType::OtherMouseUp
            {
                Event::Button {
                    mask: button_mask(native.buttonNumber()),
                    down: false,
                }
            } else if kind == NSEventType::ScrollWheel {
                let raw = gpui_kit::point(
                    native.scrollingDeltaX() as f32,
                    native.scrollingDeltaY() as f32,
                );
                Event::Wheel(if native.hasPreciseScrollingDeltas() {
                    ScrollDelta::Pixels(raw.map(px))
                } else {
                    ScrollDelta::Lines(raw)
                })
            } else {
                return event.as_ptr();
            };
            let Ok(mut capture) = capture.try_borrow_mut() else {
                return event.as_ptr();
            };
            match capture.route(seen, window.isKeyWindow()) {
                Route::Pass => event.as_ptr(),
                Route::Swallow(commands) => {
                    for command in commands {
                        let _ = input.send(Input::Command(command));
                    }
                    null_mut()
                }
            }
        });
        let mask = NSEventMask::MouseMoved
            | NSEventMask::LeftMouseDragged
            | NSEventMask::RightMouseDragged
            | NSEventMask::OtherMouseDragged
            | NSEventMask::LeftMouseDown
            | NSEventMask::LeftMouseUp
            | NSEventMask::RightMouseDown
            | NSEventMask::RightMouseUp
            | NSEventMask::OtherMouseDown
            | NSEventMask::OtherMouseUp
            | NSEventMask::ScrollWheel;
        // The local monitor runs on the AppKit main thread. AppKit copies the block.
        unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &handler) }.map(Self)
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        // This handle was returned by AppKit when the capture engaged, on the main thread.
        unsafe { NSEvent::removeMonitor(&self.0) };
    }
}

/// The frozen, hidden Mac cursor. `Desktop::release_capture` is the only caller of
/// [`CursorLock::unlock`]; `Drop` is a backstop only. A panic in a GPUI handler aborts
/// the process, and WindowServer then restores the cursor with the connection.
pub(super) struct CursorLock {
    window: Retained<NSWindow>,
    hidden: Cell<bool>,
    /// The window's `acceptsMouseMovedEvents` before the lock, restored on unlock.
    accepted_moves: bool,
}

impl CursorLock {
    /// Warps the cursor to `centre` (a point in the window's content view, top-left
    /// origin), then associates once (ending macOS's ~250 ms local-event suppression after
    /// a warp), dissociates, and hides the cursor.
    pub(super) fn lock(window: Retained<NSWindow>, centre: Point<Pixels>) -> Self {
        let accepted_moves = window.acceptsMouseMovedEvents();
        // Moves still reach the monitor while the cursor is frozen.
        window.setAcceptsMouseMovedEvents(true);
        if let Some(view) = window.contentView() {
            let content = window.convertRectToScreen(view.convertRect_toView(view.bounds(), None));
            let primary = CGDisplayBounds(CGMainDisplayID());
            let target = warp_point(content, centre, primary.size.height);
            let _ = CGWarpMouseCursorPosition(target);
        }
        let _ = CGAssociateMouseAndMouseCursorPosition(true);
        let _ = CGAssociateMouseAndMouseCursorPosition(false);
        // `NSCursor hide` is balanced by `unhide` once, guarded by `hidden`.
        unsafe {
            let _: () = objc2::msg_send![objc2::class!(NSCursor), hide];
        }
        Self {
            window,
            hidden: Cell::new(true),
            accepted_moves,
        }
    }

    /// Re-associates the cursor and shows it again. Idempotent.
    pub(super) fn unlock(&self) {
        if !self.hidden.replace(false) {
            return;
        }
        let _ = CGAssociateMouseAndMouseCursorPosition(true);
        unsafe {
            let _: () = objc2::msg_send![objc2::class!(NSCursor), unhide];
        }
        self.window.setAcceptsMouseMovedEvents(self.accepted_moves);
    }
}

impl Drop for CursorLock {
    fn drop(&mut self) {
        self.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Resolution;
    use gpui_kit::{point, size, Bounds};
    use objc2_foundation::NSSize;

    fn painted(width: f32, height: f32, resolution: (u16, u16)) -> PaintedDisplay {
        PaintedDisplay {
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(width), px(height))),
            resolution: Resolution(resolution.0, resolution.1),
        }
    }

    /// Engaged at clock 10 with a 2× scale, its first motion already dropped.
    fn captured() -> Capture {
        let mut capture = Capture::default();
        capture.engage(Some((2., 2.)), 10.);
        assert_eq!(capture.motion(1., 1., 11.), None);
        capture
    }

    #[test]
    fn capture_sends_p1_then_native_cursor_rendering() {
        let mut capture = Capture::default();
        assert_eq!(
            capture.engage(None, 0.),
            ["p,1", "SET_NATIVE_CURSOR_RENDERING,1"]
        );
        assert!(capture.is_on());
    }

    #[test]
    fn engaging_twice_sends_nothing_the_second_time() {
        let mut capture = Capture::default();
        capture.engage(None, 0.);
        assert!(capture.engage(None, 0.).is_empty());
    }

    #[test]
    fn release_sends_p0_then_lifts_held_buttons() {
        let mut capture = captured();
        assert_eq!(capture.button(1, true), "m2,0,0,1,0");
        assert_eq!(
            capture.release(),
            ["p,0", "SET_NATIVE_CURSOR_RENDERING,0", "m2,0,0,0,0"]
        );
    }

    #[test]
    fn release_without_buttons_sends_no_m2() {
        let mut capture = captured();
        assert_eq!(capture.release(), ["p,0", "SET_NATIVE_CURSOR_RENDERING,0"]);
    }

    #[test]
    fn release_is_idempotent() {
        let mut capture = captured();
        capture.release();
        assert!(capture.release().is_empty());
        assert!(!capture.is_on());
    }

    #[test]
    fn release_lifts_buttons_once() {
        let mut capture = captured();
        capture.button(4, true);
        let lifts = |commands: Vec<String>| commands.iter().filter(|c| *c == "m2,0,0,0,0").count();
        assert_eq!(lifts(capture.release()), 1);
        assert_eq!(lifts(capture.release()), 0);
        // `Desktop.buttons` stays 0 while captured, so `release_pointer` lifts nothing.
        assert_eq!(capture.buttons, 0);
    }

    #[test]
    fn relative_motion_scales_points_to_remote_pixels() {
        let mut capture = Capture::default();
        capture.engage(scale(painted(640., 400., (1280, 800))), 0.);
        capture.motion(0., 0., 1.);
        assert_eq!(capture.motion(3., -2., 1.).as_deref(), Some("m2,6,-4,0,0"));
    }

    #[test]
    fn relative_motion_carries_subpixel_remainders() {
        let mut capture = Capture::default();
        capture.engage(Some((1., 1.)), 0.);
        capture.motion(0., 0., 1.);
        let sent: i64 = (0..5)
            .filter_map(|_| capture.motion(0.4, 0., 1.))
            .map(|command| command.split(',').nth(1).unwrap().parse::<i64>().unwrap())
            .sum();
        assert_eq!(sent, 2);
    }

    #[test]
    fn relative_motion_that_rounds_to_zero_sends_nothing() {
        let mut capture = captured();
        assert_eq!(capture.motion(0.1, -0.1, 12.), None);
    }

    #[test]
    fn a_non_finite_carry_restarts_from_zero() {
        let mut capture = captured();
        assert_eq!(capture.motion(f64::NAN, 1., 12.), None);
        assert_eq!(capture.carry, (0., 0.));
        assert_eq!(capture.motion(1., 1., 12.).as_deref(), Some("m2,2,2,0,0"));
    }

    #[test]
    fn buttons_while_captured_go_as_m2_without_position() {
        let mut capture = captured();
        assert_eq!(capture.button(1, true), "m2,0,0,1,0");
        assert_eq!(capture.button(4, true), "m2,0,0,5,0");
        assert_eq!(capture.motion(1., 0., 12.).as_deref(), Some("m2,2,0,5,0"));
        assert_eq!(capture.button(1, false), "m2,0,0,4,0");
    }

    #[test]
    fn first_motion_after_lock_is_dropped() {
        let mut capture = Capture::default();
        capture.engage(Some((1., 1.)), 10.);
        assert_eq!(capture.motion(5., 5., 11.), None);
        // Older than the lock: the warp's own jump.
        assert_eq!(capture.motion(5., 5., 9.), None);
        assert_eq!(capture.motion(5., 5., 11.).as_deref(), Some("m2,5,5,0,0"));
    }

    #[test]
    fn motion_without_a_painted_picture_sends_nothing() {
        let mut capture = captured();
        capture.set_scale(None);
        assert_eq!(capture.motion(5., 5., 12.), None);
        capture.set_scale(Some((1., 1.)));
        assert_eq!(capture.motion(5., 5., 12.).as_deref(), Some("m2,5,5,0,0"));
    }

    #[test]
    fn engage_resets_the_carry() {
        let mut capture = captured();
        capture.motion(0.2, 0.2, 12.);
        assert_ne!(capture.carry, (0., 0.));
        capture.release();
        capture.engage(Some((1., 1.)), 20.);
        assert_eq!(capture.carry, (0., 0.));
    }

    #[test]
    fn a_frame_size_change_lifts_buttons_and_pauses_motion() {
        let mut capture = captured();
        capture.button(1, true);
        assert_eq!(capture.frame_changed(), ["m2,0,0,0,0"]);
        assert!(capture.is_on());
        assert_eq!(capture.motion(5., 5., 12.), None);
        assert!(Capture::default().frame_changed().is_empty());
    }

    #[test]
    fn route_swallows_motion_buttons_and_wheel_only_while_captured_and_key() {
        let motion = Event::Motion {
            dx: 2.,
            dy: 0.,
            timestamp: 12.,
        };
        let down = Event::Button {
            mask: Some(1),
            down: true,
        };
        let wheel = Event::Wheel(ScrollDelta::Lines(point(0., 1.)));
        let mut idle = Capture::default();
        for event in [motion, down, wheel] {
            assert_eq!(idle.route(event, true), Route::Pass);
        }
        let mut capture = captured();
        for event in [motion, down, wheel] {
            assert_eq!(capture.route(event, false), Route::Pass);
        }
        assert_eq!(
            capture.route(motion, true),
            Route::Swallow(vec!["m2,4,0,0,0".into()])
        );
        assert_eq!(
            capture.route(down, true),
            Route::Swallow(vec!["m2,0,0,1,0".into()])
        );
        assert_eq!(
            capture.route(wheel, true),
            Route::Swallow(vec!["m2,0,0,17,1".into(), "m2,0,0,1,0".into()])
        );
        // A button Selkies has no mask for is still eaten.
        let back = Event::Button {
            mask: None,
            down: true,
        };
        assert_eq!(capture.route(back, true), Route::Swallow(Vec::new()));
    }

    #[test]
    fn warp_point_flips_cocoa_to_quartz_coordinates() {
        let primary = 1000.;
        let rect = |x, y, w, h| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        let centre = point(px(400.), px(300.));
        // On the primary display: top-left of the content is Cocoa (100, 900).
        let on_primary = warp_point(rect(100., 100., 800., 800.), centre, primary);
        assert_eq!((on_primary.x, on_primary.y), (500., 400.));
        // A secondary display above the primary: Cocoa y past its height, Quartz y negative.
        let above = warp_point(rect(0., 1200., 800., 600.), centre, primary);
        assert_eq!((above.x, above.y), (400., -500.));
        // A secondary display to the left: negative x passes through.
        let left = warp_point(rect(-1600., 200., 800., 600.), centre, primary);
        assert_eq!((left.x, left.y), (-1200., 500.));
    }

    #[test]
    fn hint_text_for_capture_and_release() {
        assert_eq!(Release::Chord.hint(), Some(CaptureHint::Released));
        assert_eq!(Release::FocusLost.hint(), Some(CaptureHint::Released));
        assert_eq!(
            Release::Window { visible: true }.hint(),
            Some(CaptureHint::Released)
        );
        assert_eq!(Release::Window { visible: false }.hint(), None);
        assert_eq!(Release::Disconnected.hint(), None);
        assert_eq!(Release::Closed.hint(), None);
        let (captured, shown) = CaptureHint::Captured.text();
        assert_eq!(
            format!(
                "{}{}{}",
                captured.lead,
                captured.chord.unwrap_or_default(),
                captured.tail
            ),
            "Pointer captured · ⌃⌥⇧Z releases it"
        );
        assert_eq!(shown.as_millis(), 3000);
        let (released, shown) = CaptureHint::Released.text();
        assert_eq!(released.lead, "Pointer released");
        assert!(released.chord.is_none());
        assert_eq!(shown.as_millis(), 1500);
        assert_eq!(
            CaptureHint::Unavailable.text().0.lead,
            "Capture Pointer is available once the picture appears"
        );
    }

    #[test]
    fn button_numbers_map_to_selkies_masks() {
        assert_eq!(button_mask(0), Some(1));
        assert_eq!(button_mask(1), Some(4));
        assert_eq!(button_mask(2), Some(2));
        assert_eq!(button_mask(3), None);
    }
}
