//! The app-wide gamepads (`ARCH-032`): connected pads, their Player slots, and which session
//! window is key. Each `SessionWindow` keeps its own `gamepad::Link` and syncs itself.
//!
//! Never updates a session inside `update_global` (the index invariant, `ARCH-015`): the
//! drain task updates the global, then the sessions, one after the other.

use super::session_window::{OpenWindows, SessionWindow};
use crate::gamepad::native::{Controllers, PadEvent};
use crate::gamepad::{Pad, PadState, Rumble, SlotTable};
use gpui_kit::{AnyWindowHandle, App, BorrowAppContext as _, Global, WeakEntity};

type SessionHandle = (AnyWindowHandle, WeakEntity<SessionWindow>);

pub(crate) struct Gamepads {
    native: Option<Controllers>,
    slots: SlotTable,
    /// In connection order.
    pads: Vec<Pad>,
    /// The key session window, if any: only it sends pads.
    active: Option<SessionHandle>,
    /// The window that last drove each slot of each server while key. Only it may send
    /// that slot's `js,d` (`ARCH-032`).
    drivers: Drivers<AnyWindowHandle>,
}

/// The window driving each `(server, slot)`. Slot tables are per Selkies server, so slot 0
/// on one server and slot 0 on another are different Players.
#[derive(Debug)]
pub(super) struct Drivers<W>(std::collections::HashMap<(String, u8), W>);

impl<W> Default for Drivers<W> {
    fn default() -> Self {
        Self(std::collections::HashMap::new())
    }
}

impl<W: Copy + PartialEq> Drivers<W> {
    fn drive(&mut self, window: W, server: &str, slots: &[u8]) {
        for &slot in slots {
            self.0.insert((server.to_string(), slot), window);
        }
    }

    /// Whether `window` still drives `slot` on `server` (no other window has since).
    fn drives(&self, window: W, server: &str, slot: u8) -> bool {
        self.0
            .get(&(server.to_string(), slot))
            .is_none_or(|driver| *driver == window)
    }
}

impl Global for Gamepads {}

/// Starts GameController and the task that applies its events.
pub(super) fn init(cx: &mut App) {
    let (events, mut incoming) = tokio::sync::mpsc::unbounded_channel();
    cx.set_global(Gamepads {
        native: Some(Controllers::start(events)),
        slots: SlotTable::default(),
        pads: Vec::new(),
        active: None,
        drivers: Drivers::default(),
    });
    cx.spawn(async move |cx| {
        while let Some(first) = incoming.recv().await {
            // Everything queued since, with each pad's readings folded to its latest: one
            // global update and one sync per batch, however fast a stick moves.
            let mut batch = vec![first];
            while let Ok(event) = incoming.try_recv() {
                batch.push(event);
            }
            let batch = coalesce(batch);
            let roster = batch
                .iter()
                .any(|event| !matches!(event, PadEvent::Changed(..)));
            // The global first, then (outside its lease) the sessions that must hear of it.
            let targets = cx.update(|cx| {
                cx.update_global::<Gamepads, _>(|gamepads, _| {
                    for event in batch {
                        gamepads.apply(event);
                    }
                });
                if roster {
                    cx.try_global::<OpenWindows>()
                        .map(|windows| {
                            windows
                                .sessions
                                .iter()
                                .map(|open| (open.window, open.entity.clone()))
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    cx.global::<Gamepads>()
                        .active
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                }
            });
            cx.update(|cx| {
                for (window, entity) in targets {
                    let _ = window.update(cx, |_, _, cx| {
                        entity.update(cx, |session, cx| {
                            session.sync_gamepads(cx);
                            if roster {
                                cx.notify();
                            }
                        })
                    });
                }
            });
        }
    })
    .detach();
}

/// Folds each pad's readings to its latest, keeping connects and disconnects in order (a
/// reading never moves across its own pad's connect or disconnect).
fn coalesce(events: Vec<PadEvent>) -> Vec<PadEvent> {
    let mut out: Vec<PadEvent> = Vec::with_capacity(events.len());
    for event in events {
        if let PadEvent::Changed(id, _) = &event {
            let earlier = out.iter().rposition(|seen| match seen {
                PadEvent::Changed(other, _) => other == id,
                PadEvent::Connected(other, ..) | PadEvent::Gone(other) => other == id,
            });
            if let Some(index) = earlier.filter(|&i| matches!(out[i], PadEvent::Changed(..))) {
                out[index] = event;
                continue;
            }
        }
        out.push(event);
    }
    out
}

/// The key window after `next` becomes key (`active`) or resigns, and the one that lost
/// key, if another.
fn hand_over<T>(
    current: &mut Option<T>,
    next: T,
    active: bool,
    same: impl Fn(&T, &T) -> bool,
) -> Option<T> {
    if active {
        let lost = current.take().filter(|previous| !same(previous, &next));
        *current = Some(next);
        lost
    } else {
        if current.as_ref().is_some_and(|current| same(current, &next)) {
            *current = None;
        }
        None
    }
}

impl Gamepads {
    /// The key window drives the slots it announced on its server.
    pub(super) fn drive(&mut self, window: AnyWindowHandle, server: &str, slots: &[u8]) {
        self.drivers.drive(window, server, slots);
    }

    /// Whether `window` still drives `slot` on `server`.
    pub(super) fn drives(&self, window: AnyWindowHandle, server: &str, slot: u8) -> bool {
        self.drivers.drives(window, server, slot)
    }

    fn apply(&mut self, event: PadEvent) {
        match event {
            PadEvent::Connected(id, name, rumble) => {
                if self.pads.iter().any(|pad| pad.id == id) {
                    return;
                }
                let slot = self.slots.assign(id);
                self.pads.push(Pad {
                    id,
                    name,
                    slot,
                    state: PadState::REST,
                    rumble,
                });
                self.route_home();
            }
            PadEvent::Changed(id, raw) => {
                if let Some(pad) = self.pads.iter_mut().find(|pad| pad.id == id) {
                    pad.state = PadState::from_raw(&raw);
                }
            }
            PadEvent::Gone(id) => {
                self.slots.free(id);
                self.pads.retain(|pad| pad.id != id);
                // A pad waiting for a Player takes the freed slot, in connection order.
                for pad in self.pads.iter_mut().filter(|pad| pad.slot.is_none()) {
                    pad.slot = self.slots.assign(pad.id);
                }
                self.route_home();
            }
        }
    }

    pub(crate) fn pads(&self) -> &[Pad] {
        &self.pads
    }

    /// The session window `window` became key (`active`) or resigned. Only records it,
    /// and returns the window that lost key so the caller can sync it later: the caller
    /// syncs itself, and must not be updated from here (it holds its own lease).
    pub(super) fn set_active(
        &mut self,
        window: AnyWindowHandle,
        entity: WeakEntity<SessionWindow>,
        active: bool,
    ) -> Option<SessionHandle> {
        let lost = hand_over(&mut self.active, (window, entity), active, |a, b| {
            a.0 == b.0
        });
        // Whatever played belonged to the window that was key.
        self.stop_rumble();
        self.route_home();
        lost
    }

    /// Plays `rumble` on the pad holding its slot.
    pub(crate) fn rumble(&self, rumble: &Rumble) {
        let pad = self
            .pads
            .iter()
            .find(|pad| pad.slot == Some(rumble.slot) && pad.rumble);
        if let (Some(native), Some(pad)) = (&self.native, pad) {
            native.rumble(pad.id, rumble);
        }
    }

    pub(crate) fn stop_rumble(&self) {
        if let Some(native) = &self.native {
            native.stop_rumble();
        }
    }

    /// Pads with a Player keep the Home button while a session window is key; macOS gets
    /// it back otherwise.
    fn route_home(&self) {
        let Some(native) = &self.native else {
            return;
        };
        let routed = self.active.is_some();
        for pad in &self.pads {
            native.route_home(pad.id, routed && pad.slot.is_some());
        }
    }

    #[cfg(test)]
    fn detached() -> Self {
        Self {
            native: None,
            slots: SlotTable::default(),
            pads: Vec::new(),
            active: None,
            drivers: Drivers::default(),
        }
    }
}

/// The pad ids in slot order, for tests.
#[cfg(test)]
fn slots_of(gamepads: &Gamepads) -> Vec<(crate::gamepad::PadId, Option<u8>)> {
    gamepads.pads.iter().map(|pad| (pad.id, pad.slot)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drivers_are_per_server_and_slot() {
        use crate::gamepad::{Gate, Link, Pad};
        let pad = |id| Pad {
            id,
            name: "Pad".into(),
            slot: Some(0),
            state: PadState::REST,
            rumble: false,
        };
        let key = Gate {
            ready_and_enabled: true,
            input_on: true,
            key: true,
        };
        // Window 1 on server A and window 2 on server B both drive slot 0.
        let mut drivers = Drivers::default();
        let (mut a, mut b) = (Link::default(), Link::default());
        a.sync(&[pad(7)], key);
        drivers.drive(1u8, "a.example", &a.announced_slots());
        b.sync(&[pad(7)], key);
        drivers.drive(2u8, "b.example", &b.announced_slots());
        // Closing window 1 sends js,d to its own server: nothing yielded.
        assert!(a
            .yield_to(&|slot| drivers.drives(1, "a.example", slot))
            .is_empty());
        assert_eq!(a.close(), ["js,d,0"]);
        // Window 2's slot stays its own.
        assert!(b
            .yield_to(&|slot| drivers.drives(2, "b.example", slot))
            .is_empty());
        assert_eq!(b.announced_slots(), [0]);
        // On the same server, a newer driver takes the slot over.
        drivers.drive(3u8, "a.example", &[0]);
        assert!(!drivers.drives(1, "a.example", 0));
        assert!(drivers.drives(2, "b.example", 0));
    }

    #[test]
    fn set_active_reports_the_window_that_lost_key() {
        let same = |a: &u8, b: &u8| a == b;
        let mut key = None;
        assert_eq!(hand_over(&mut key, 1, true, same), None);
        assert_eq!(hand_over(&mut key, 1, true, same), None);
        // Window 2 takes key: window 1 lost it.
        assert_eq!(hand_over(&mut key, 2, true, same), Some(1));
        // Window 1 resigning (late) changes nothing.
        assert_eq!(hand_over(&mut key, 1, false, same), None);
        assert_eq!(key, Some(2));
        assert_eq!(hand_over(&mut key, 2, false, same), None);
        assert_eq!(key, None);
    }

    #[test]
    fn readings_coalesce_per_pad_around_roster_events() {
        use crate::gamepad::RawPad;
        let reading = |a: f32| RawPad {
            a,
            ..RawPad::default()
        };
        let events = vec![
            PadEvent::Changed(1, reading(0.)),
            PadEvent::Changed(2, reading(0.)),
            PadEvent::Changed(1, reading(1.)),
            PadEvent::Gone(2),
            PadEvent::Connected(2, "Pad".into(), false),
            PadEvent::Changed(2, reading(1.)),
            PadEvent::Changed(1, reading(0.5)),
        ];
        assert_eq!(
            coalesce(events),
            [
                PadEvent::Changed(1, reading(0.5)),
                PadEvent::Changed(2, reading(0.)),
                PadEvent::Gone(2),
                PadEvent::Connected(2, "Pad".into(), false),
                PadEvent::Changed(2, reading(1.)),
            ]
        );
    }

    #[test]
    fn a_waiting_pad_takes_a_freed_slot() {
        let mut gamepads = Gamepads::detached();
        for id in 1..=5 {
            gamepads.apply(PadEvent::Connected(id, format!("Pad {id}"), false));
        }
        assert_eq!(slots_of(&gamepads)[4], (5, None));
        gamepads.apply(PadEvent::Gone(2));
        assert_eq!(
            slots_of(&gamepads),
            [(1, Some(0)), (3, Some(2)), (4, Some(3)), (5, Some(1))]
        );
        // A pad reported twice (controllers() and the notification) is kept once.
        gamepads.apply(PadEvent::Connected(1, "Pad 1".into(), false));
        assert_eq!(gamepads.pads().len(), 4);
    }
}
