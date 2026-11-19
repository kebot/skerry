use super::pinch;
use crate::transport::{Contact, Input};
use objc2::{
    define_class, msg_send,
    rc::{Retained, Weak},
    runtime::AnyObject,
    DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSEvent, NSResponder, NSTouchPhase, NSTouchType, NSTouchTypeMask, NSView, NSWindow,
};
use std::cell::{Cell, RefCell};
use tokio::sync::mpsc;

const MAX_CONTACTS: usize = 10;

struct State {
    enabled: Cell<bool>,
    target: pinch::Target,
    input: mpsc::UnboundedSender<Input>,
    next_id: Cell<u16>,
    active: RefCell<Vec<(Retained<AnyObject>, u16)>>,
}

impl State {
    fn clear(&self) {
        if !self.active.borrow().is_empty() {
            self.active.borrow_mut().clear();
            let _ = self.input.send(Input::Contacts(Vec::new()));
        }
    }

    fn update(&self, event: &NSEvent, view: &NSView) {
        let Some(window) = view.window().filter(|window| window.isKeyWindow()) else {
            self.clear();
            return;
        };
        if !self.enabled.get() {
            self.clear();
            return;
        }
        let Some((display, _)) = self.target.get() else {
            self.clear();
            return;
        };
        let point = pinch::view_position(view, window.mouseLocationOutsideOfEventStream());
        if self.active.borrow().is_empty() && display.position(point, false).is_none() {
            return;
        }
        let touches = event.touchesMatchingPhase_inView(NSTouchPhase::Touching, Some(view));
        let mut frame = Vec::new();
        let mut identities = Vec::new();
        let mut active = self.active.borrow_mut();
        for touch in touches
            .to_vec()
            .into_iter()
            .filter(|touch| touch.r#type() == NSTouchType::Indirect && !touch.isResting())
            .take(MAX_CONTACTS)
        {
            let Some((x, y)) = position(touch.normalizedPosition().x, touch.normalizedPosition().y)
            else {
                continue;
            };
            let identity = touch.identity();
            let id = active
                .iter()
                // Touch identities conform to NSCopying and are compared using NSObject equality.
                .find(|(known, _)| unsafe { msg_send![&**known, isEqual: &*identity] })
                .map_or_else(
                    || loop {
                        let id = self.next_id.get();
                        self.next_id.set(id.checked_add(1).unwrap_or(1));
                        if !active
                            .iter()
                            .chain(identities.iter())
                            .any(|(_, used)| *used == id)
                        {
                            break id;
                        }
                    },
                    |(_, id)| *id,
                );
            identities.push((identity, id));
            frame.push(Contact(id, x, y));
        }
        if !frame.is_empty() || !active.is_empty() {
            *active = identities;
            let _ = self.input.send(Input::Contacts(frame));
        }
    }
}

pub(super) fn position(x: f64, y: f64) -> Option<(u16, u16)> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let quantize = |value: f64| (value.clamp(0., 1.) * 32767.).round() as u16;
    Some((quantize(x), quantize(1. - y)))
}

struct ResponderState {
    input: State,
    view: Weak<NSView>,
}

define_class!(
    // AppKit delivers indirect touches through the view's responder chain.
    #[unsafe(super(NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ResponderState]
    struct SealSkinTrackpadResponder;

    impl SealSkinTrackpadResponder {
        #[unsafe(method(touchesBeganWithEvent:))]
        fn began(&self, event: &NSEvent) { self.contacts(event); }

        #[unsafe(method(touchesMovedWithEvent:))]
        fn moved(&self, event: &NSEvent) { self.contacts(event); }

        #[unsafe(method(touchesEndedWithEvent:))]
        fn ended(&self, event: &NSEvent) { self.contacts(event); }

        #[unsafe(method(touchesCancelledWithEvent:))]
        fn cancelled(&self, _: &NSEvent) { self.ivars().input.clear(); }
    }
);

impl SealSkinTrackpadResponder {
    fn contacts(&self, event: &NSEvent) {
        let state = self.ivars();
        if let Some(view) = state.view.load() {
            state.input.update(event, &view);
        }
    }
}

struct PreviousResponder {
    next: Option<Retained<NSResponder>>,
    touch_types: NSTouchTypeMask,
}

pub(super) struct Capture {
    view: Retained<NSView>,
    responder: Retained<SealSkinTrackpadResponder>,
    previous: RefCell<Option<PreviousResponder>>,
}

impl Capture {
    pub(super) fn install(
        window: &NSWindow,
        target: pinch::Target,
        input: mpsc::UnboundedSender<Input>,
    ) -> Option<Self> {
        let main = MainThreadMarker::new()?;
        let view = window.contentView()?;
        let allocated = SealSkinTrackpadResponder::alloc(main).set_ivars(ResponderState {
            input: State {
                enabled: Cell::new(false),
                target,
                input,
                next_id: Cell::new(1),
                active: RefCell::default(),
            },
            view: Weak::new(&*view),
        });
        // NSResponder's init returns the allocated responder with its Rust fields intact.
        let responder = unsafe { msg_send![super(allocated), init] };
        Some(Self {
            view,
            responder,
            previous: RefCell::default(),
        })
    }

    pub(super) fn set_enabled(&self, enabled: bool) {
        let mut previous = self.previous.borrow_mut();
        self.responder.ivars().input.enabled.set(enabled);
        if enabled {
            if previous.is_some() {
                return;
            }
            // The original responder is alive in the view's chain. Capture retains
            // both links until detach, and restores the view's original next responder.
            unsafe {
                let next = self.view.nextResponder();
                let touch_types = self.view.allowedTouchTypes();
                self.responder.setNextResponder(next.as_deref());
                self.view.setNextResponder(Some(&self.responder));
                self.view
                    .setAllowedTouchTypes(touch_types | NSTouchTypeMask::Indirect);
                *previous = Some(PreviousResponder { next, touch_types });
            }
        } else if let Some(previous) = previous.take() {
            self.clear();
            // The view and both responders are still retained while restoring the chain.
            unsafe {
                if self
                    .view
                    .nextResponder()
                    .is_some_and(|next| std::ptr::eq(&*next, &**self.responder))
                {
                    self.view.setNextResponder(previous.next.as_deref());
                }
                self.responder.setNextResponder(None);
            }
            self.view.setAllowedTouchTypes(previous.touch_types);
        }
    }

    pub(super) fn clear(&self) {
        self.responder.ivars().input.clear();
    }

    pub(super) fn has_contacts(&self) -> bool {
        !self.responder.ivars().input.active.borrow().is_empty()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.set_enabled(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn converts_trackpad_positions_to_top_left_wire_coordinates() {
        assert_eq!(position(0., 0.), Some((0, 32767)));
        assert_eq!(position(1., 1.), Some((32767, 0)));
        assert_eq!(position(0.5, 0.5), Some((16384, 16384)));
        assert_eq!(position(-0.1, 1.01), Some((0, 0)));
        assert_eq!(position(f64::NAN, 0.), None);
    }
}
