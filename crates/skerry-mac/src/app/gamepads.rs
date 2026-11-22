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
    });
    cx.spawn(async move |cx| {
        while let Some(event) = incoming.recv().await {
            let roster = !matches!(event, PadEvent::Changed(..));
            // The global first, then (outside its lease) the sessions that must hear of it.
            let targets = cx.update(|cx| {
                cx.update_global::<Gamepads, _>(|gamepads, _| gamepads.apply(event));
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

impl Gamepads {
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
        let lost = if active {
            self.active
                .replace((window, entity))
                .filter(|(previous, _)| *previous != window)
        } else {
            if self
                .active
                .as_ref()
                .is_some_and(|(current, _)| *current == window)
            {
                self.active = None;
            }
            None
        };
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
