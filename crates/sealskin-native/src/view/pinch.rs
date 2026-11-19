use super::PaintedDisplay;
use crate::transport::{Input, Pointer};
use block2::RcBlock;
use gpui::{point, px, Pixels, Point};
use objc2::{rc::Retained, runtime::AnyObject, MainThreadMarker};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventPhase, NSView, NSWindow};
use objc2_foundation::NSPoint;
use std::{
    cell::Cell,
    ptr::{null_mut, NonNull},
    rc::Rc,
};
use tokio::sync::mpsc;

pub(super) type Target = Rc<Cell<Option<(PaintedDisplay, u8)>>>;

pub(super) struct Monitor(Retained<AnyObject>);

impl Monitor {
    pub(super) fn install(
        window: Retained<NSWindow>,
        target: Target,
        raw: Rc<Cell<bool>>,
        input: mpsc::UnboundedSender<Input>,
    ) -> Option<Self> {
        let carry = Cell::new(0.);
        let handler = RcBlock::new(move |event: NonNull<NSEvent>| {
            let Some(main) = MainThreadMarker::new() else {
                return event.as_ptr();
            };
            // The AppKit monitor lends the event for the duration of this callback.
            let native = unsafe { event.as_ref() };
            let phase = native.phase();
            if phase.intersects(NSEventPhase::Began | NSEventPhase::Cancelled | NSEventPhase::Ended)
            {
                carry.set(0.);
            }
            let Some((display, buttons)) = target.get() else {
                return event.as_ptr();
            };
            let owner = native.window(main);
            if !window.isKeyWindow()
                || owner
                    .as_ref()
                    .is_some_and(|owner| !std::ptr::eq(&**owner, &*window))
            {
                return event.as_ptr();
            }
            let Some(view) = window.contentView() else {
                return event.as_ptr();
            };
            let location = if owner.is_some() {
                native.locationInWindow()
            } else {
                window.mouseLocationOutsideOfEventStream()
            };
            let Some(position) = display.position(view_position(&view, location), false) else {
                carry.set(0.);
                return event.as_ptr();
            };
            if raw.get() {
                return null_mut();
            }
            if !phase.intersects(NSEventPhase::Cancelled | NSEventPhase::Ended) {
                let (notches, next) = zoom_steps(carry.get(), native.magnification());
                carry.set(next);
                if notches != 0 {
                    let _ = input.send(Input::Zoom(
                        notches,
                        Pointer {
                            display: display.resolution,
                            position,
                            buttons,
                        },
                    ));
                }
            }
            null_mut()
        });
        // The local monitor runs on the AppKit main thread. AppKit copies the block.
        unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::Magnify, &handler)
        }
        .map(Self)
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        // This handle was returned by AppKit when the main-thread session opened.
        unsafe { NSEvent::removeMonitor(&self.0) };
    }
}

pub(super) fn view_position(view: &NSView, location: NSPoint) -> Point<Pixels> {
    let rect = view.bounds();
    let local = view.convertPoint_fromView(location, None);
    let x = local.x - rect.origin.x;
    let y = if view.isFlipped() {
        local.y - rect.origin.y
    } else {
        rect.origin.y + rect.size.height - local.y
    };
    point(px(x as f32), px(y as f32))
}

fn zoom_steps(carry: f64, magnification: f64) -> (i8, f64) {
    if !magnification.is_finite() || magnification <= -1. {
        return (0, carry);
    }
    let steps = carry + magnification.ln_1p() / 1.14_f64.ln();
    let whole = steps.trunc();
    (whole.clamp(-16., 16.) as i8, steps - whole)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_native_pinches_accumulate_in_both_directions() {
        let (steps, carry) = zoom_steps(0., 0.08);
        assert_eq!(steps, 0);
        assert_eq!(zoom_steps(carry, 0.08).0, 1);
        let (steps, carry) = zoom_steps(0., -0.08);
        assert_eq!(steps, 0);
        assert_eq!(zoom_steps(carry, -0.08).0, -1);
        assert_eq!(zoom_steps(carry, f64::NAN), (0, carry));
    }
}
