use super::pinch;
use crate::transport::{Contact, Input};
use objc2::{
    define_class, msg_send, rc::Retained, runtime::AnyObject, DefinedClass, MainThreadMarker,
    MainThreadOnly,
};
use objc2_app_kit::{
    NSEvent, NSGestureRecognizer, NSTouchPhase, NSTouchType, NSTouchTypeMask, NSView, NSWindow,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
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

define_class!(
    // The parent recognizer and all callbacks are restricted to the AppKit thread.
    #[unsafe(super(NSGestureRecognizer))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Rc<State>]
    struct SealSkinTrackpadRecognizer;

    impl SealSkinTrackpadRecognizer {
        #[unsafe(method(touchesBeganWithEvent:))]
        fn began(&self, event: &NSEvent) { self.contacts(event); }

        #[unsafe(method(touchesMovedWithEvent:))]
        fn moved(&self, event: &NSEvent) { self.contacts(event); }

        #[unsafe(method(touchesEndedWithEvent:))]
        fn ended(&self, event: &NSEvent) { self.contacts(event); }

        #[unsafe(method(touchesCancelledWithEvent:))]
        fn cancelled(&self, _: &NSEvent) { self.ivars().clear(); }
    }
);

impl SealSkinTrackpadRecognizer {
    fn contacts(&self, event: &NSEvent) {
        if let Some(view) = self.view() {
            self.ivars().update(event, &view);
        }
    }
}

pub(super) struct Capture {
    view: Retained<NSView>,
    recognizer: Retained<SealSkinTrackpadRecognizer>,
    previous_touch_types: NSTouchTypeMask,
}

impl Capture {
    pub(super) fn install(
        window: &NSWindow,
        target: pinch::Target,
        input: mpsc::UnboundedSender<Input>,
    ) -> Option<Self> {
        let main = MainThreadMarker::new()?;
        let view = window.contentView()?;
        let state = Rc::new(State {
            enabled: Cell::new(false),
            target,
            input,
            next_id: Cell::new(1),
            active: RefCell::default(),
        });
        let allocated = SealSkinTrackpadRecognizer::alloc(main).set_ivars(state);
        // The superclass initializer accepts nil target and action. The overrides read contacts without recognizing a gesture.
        let recognizer: Retained<SealSkinTrackpadRecognizer> = unsafe {
            msg_send![super(allocated), initWithTarget: Option::<&AnyObject>::None, action: Option::<objc2::runtime::Sel>::None]
        };
        recognizer.setAllowedTouchTypes(NSTouchTypeMask::Indirect);
        recognizer.setEnabled(false);
        let previous_touch_types = view.allowedTouchTypes();
        view.addGestureRecognizer(&recognizer);
        Some(Self {
            view,
            recognizer,
            previous_touch_types,
        })
    }

    pub(super) fn set_enabled(&self, enabled: bool) {
        if !enabled {
            self.clear();
        }
        self.recognizer.ivars().enabled.set(enabled);
        self.view.setAllowedTouchTypes(if enabled {
            self.previous_touch_types | NSTouchTypeMask::Indirect
        } else {
            self.previous_touch_types
        });
        self.recognizer.setEnabled(enabled);
    }

    pub(super) fn clear(&self) {
        self.recognizer.ivars().clear();
    }

    pub(super) fn has_contacts(&self) -> bool {
        !self.recognizer.ivars().active.borrow().is_empty()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.set_enabled(false);
        self.view.removeGestureRecognizer(&self.recognizer);
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
