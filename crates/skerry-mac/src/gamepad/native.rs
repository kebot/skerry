//! GameController and CoreHaptics (`ARCH-032`): connected pads, their readings, the Home
//! button's system gesture, and rumble. Main thread only; the only caller of either
//! framework. Readings leave as plain [`PadEvent`]s on a channel.

use super::{haptics_for, Haptic, Motor, PadId, RawPad, Rumble};
use block2::RcBlock;
use objc2::{
    msg_send,
    rc::Retained,
    runtime::{NSObjectProtocol, ProtocolObject},
    sel, AnyThread,
};
use objc2_core_haptics::{
    CHHapticEngine, CHHapticEvent, CHHapticEventParameter, CHHapticEventParameterIDHapticIntensity,
    CHHapticEventParameterIDHapticSharpness, CHHapticEventTypeHapticContinuous, CHHapticPattern,
    CHHapticPatternPlayer,
};
use objc2_foundation::{NSArray, NSNotification, NSNotificationCenter, NSOperationQueue, NSString};
use objc2_game_controller::{
    GCController, GCControllerButtonInput, GCControllerDidConnectNotification,
    GCControllerDidDisconnectNotification, GCControllerElement, GCDevice, GCExtendedGamepad,
    GCHapticsLocality, GCHapticsLocalityDefault, GCHapticsLocalityLeftHandle,
    GCHapticsLocalityRightHandle, GCSystemGestureState,
};
use std::{cell::RefCell, collections::HashMap, ptr::NonNull, rc::Rc};
use tokio::sync::mpsc;

/// What the pads did, for the `Gamepads` global.
#[derive(Clone, Debug, PartialEq)]
pub enum PadEvent {
    /// A pad with an extended gamepad profile connected: its name and whether it has
    /// haptics.
    Connected(PadId, String, bool),
    Changed(PadId, RawPad),
    Gone(PadId),
}

/// Every connected pad, and the connect / disconnect observers.
pub struct Controllers {
    inner: Rc<RefCell<Inner>>,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

struct Inner {
    pads: HashMap<PadId, NativePad>,
    events: mpsc::UnboundedSender<PadEvent>,
}

struct NativePad {
    controller: Retained<GCController>,
    haptics: Option<Haptics>,
    /// The Home button's gesture state before Skerry routed it.
    home_routed: bool,
}

fn pad_id(controller: &GCController) -> PadId {
    controller as *const GCController as usize as PadId
}

impl Controllers {
    /// Watches pads connecting and leaving, and adopts those already connected.
    pub fn start(events: mpsc::UnboundedSender<PadEvent>) -> Self {
        let inner = Rc::new(RefCell::new(Inner {
            pads: HashMap::new(),
            events,
        }));
        let center = NSNotificationCenter::defaultCenter();
        let queue = NSOperationQueue::mainQueue();
        let observe = |name: &NSString, connected: bool| {
            let inner = Rc::downgrade(&inner);
            let block = RcBlock::new(move |notification: NonNull<NSNotification>| {
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                // The center lends the notification for the duration of the block.
                let notification = unsafe { notification.as_ref() };
                let Some(object) = notification.object() else {
                    return;
                };
                let Ok(controller) = object.downcast::<GCController>() else {
                    return;
                };
                let Ok(mut inner) = inner.try_borrow_mut() else {
                    return;
                };
                if connected {
                    inner.adopt(controller);
                } else {
                    inner.remove(pad_id(&controller));
                }
            });
            // Delivered on the main queue; the center copies the block.
            unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    None,
                    Some(&queue),
                    &block,
                )
            }
        };
        let observers = unsafe {
            vec![
                observe(GCControllerDidConnectNotification, true),
                observe(GCControllerDidDisconnectNotification, false),
            ]
        };
        for controller in unsafe { GCController::controllers() }.iter() {
            inner.borrow_mut().adopt(controller);
        }
        Self { inner, observers }
    }

    /// Plays `rumble` on the pad, if it has haptics. A pad whose haptics fail is left
    /// without rumble.
    pub fn rumble(&self, id: PadId, rumble: &Rumble) {
        let mut inner = self.inner.borrow_mut();
        if let Some(pad) = inner.pads.get_mut(&id) {
            if let Some(haptics) = &mut pad.haptics {
                if !haptics.apply(rumble) {
                    pad.haptics = None;
                }
            }
        }
    }

    /// Stops every pad's rumble.
    pub fn stop_rumble(&self) {
        for pad in self.inner.borrow_mut().pads.values_mut() {
            if let Some(haptics) = &mut pad.haptics {
                haptics.stop();
            }
        }
    }

    /// Keeps the Home button for the session (`routed`), or gives it back to macOS
    /// (Launchpad, the Games overlay).
    pub fn route_home(&self, id: PadId, routed: bool) {
        if let Some(pad) = self.inner.borrow_mut().pads.get_mut(&id) {
            if pad.home_routed != routed {
                pad.home_routed = routed;
                set_home_gesture(&pad.controller, routed);
            }
        }
    }
}

impl Drop for Controllers {
    fn drop(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        for observer in &self.observers {
            // Each observer was registered with this center on the main thread.
            unsafe { center.removeObserver(observer.as_ref()) };
        }
        let mut inner = self.inner.borrow_mut();
        for (_, pad) in inner.pads.drain() {
            pad.release();
        }
    }
}

impl Inner {
    /// Starts reading a pad. Pads with no extended gamepad profile are ignored, and a pad
    /// already known (reported by both `controllers` and the notification) is adopted once.
    fn adopt(&mut self, controller: Retained<GCController>) {
        let id = pad_id(&controller);
        if self.pads.contains_key(&id) {
            return;
        }
        let Some(gamepad) = (unsafe { controller.extendedGamepad() }) else {
            return;
        };
        let name = unsafe { controller.vendorName() }
            .map(|name| name.to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Controller".into());
        let haptics = Haptics::new(&controller);
        let events = self.events.clone();
        // Captures only the sender and the id, never the pad: no retain cycle. `release`
        // clears it too.
        let handler = RcBlock::new(
            move |gamepad: NonNull<GCExtendedGamepad>, _: NonNull<GCControllerElement>| {
                // GameController lends the profile for the duration of the handler.
                let gamepad = unsafe { gamepad.as_ref() };
                let _ = events.send(PadEvent::Changed(id, read(gamepad)));
            },
        );
        unsafe { gamepad.setValueChangedHandler(RcBlock::as_ptr(&handler)) };
        let _ = self
            .events
            .send(PadEvent::Connected(id, name, haptics.is_some()));
        let _ = self.events.send(PadEvent::Changed(id, read(&gamepad)));
        self.pads.insert(
            id,
            NativePad {
                controller,
                haptics,
                home_routed: false,
            },
        );
    }

    fn remove(&mut self, id: PadId) {
        if let Some(pad) = self.pads.remove(&id) {
            pad.release();
            let _ = self.events.send(PadEvent::Gone(id));
        }
    }
}

impl NativePad {
    /// Clears the handler, stops rumble and gives the Home button back.
    fn release(mut self) {
        if let Some(gamepad) = unsafe { self.controller.extendedGamepad() } {
            unsafe { gamepad.setValueChangedHandler(std::ptr::null_mut()) };
        }
        if let Some(haptics) = &mut self.haptics {
            haptics.stop();
        }
        if self.home_routed {
            set_home_gesture(&self.controller, false);
        }
    }
}

fn set_home_gesture(controller: &GCController, routed: bool) {
    let Some(home) = (unsafe { controller.extendedGamepad() })
        .and_then(|gamepad| unsafe { gamepad.buttonHome() })
    else {
        return;
    };
    // macOS 11.3+; older systems keep Home for themselves.
    if !home.respondsToSelector(sel!(setPreferredSystemGestureState:)) {
        return;
    }
    let state = if routed {
        GCSystemGestureState::Disabled
    } else {
        GCSystemGestureState::Enabled
    };
    unsafe { home.setPreferredSystemGestureState(state) };
}

/// Reads every element of the profile.
fn read(gamepad: &GCExtendedGamepad) -> RawPad {
    let pressed =
        |button: &GCControllerButtonInput| f32::from(u8::from(unsafe { button.isPressed() }));
    let optional = |button: Option<Retained<GCControllerButtonInput>>| {
        button.map(|button| unsafe { button.isPressed() })
    };
    unsafe {
        let dpad = gamepad.dpad();
        let left = gamepad.leftThumbstick();
        let right = gamepad.rightThumbstick();
        RawPad {
            a: pressed(&gamepad.buttonA()),
            b: pressed(&gamepad.buttonB()),
            x: pressed(&gamepad.buttonX()),
            y: pressed(&gamepad.buttonY()),
            lb: pressed(&gamepad.leftShoulder()),
            rb: pressed(&gamepad.rightShoulder()),
            lt: gamepad.leftTrigger().value(),
            rt: gamepad.rightTrigger().value(),
            options: optional(gamepad.buttonOptions()),
            menu: Some(gamepad.buttonMenu().isPressed()),
            ls: optional(gamepad.leftThumbstickButton()),
            rs: optional(gamepad.rightThumbstickButton()),
            home: optional(gamepad.buttonHome()),
            dpad: [
                dpad.up().isPressed(),
                dpad.down().isPressed(),
                dpad.left().isPressed(),
                dpad.right().isPressed(),
            ],
            left: (left.xAxis().value(), left.yAxis().value()),
            right: (right.xAxis().value(), right.yAxis().value()),
        }
    }
}

/// A pad's haptic engines: left (strong) and right (weak) handles when it has both, else
/// one default engine. Each message gets a fresh player for its own duration.
struct Haptics {
    engines: Vec<(Motor, Retained<CHHapticEngine>)>,
    players: HashMap<Motor, Retained<ProtocolObject<dyn CHHapticPatternPlayer>>>,
}

impl Haptics {
    fn new(controller: &GCController) -> Option<Self> {
        if !controller.respondsToSelector(sel!(haptics)) {
            return None;
        }
        let haptics = unsafe { controller.haptics() }?;
        let localities = unsafe { haptics.supportedLocalities() };
        let has = |locality: &GCHapticsLocality| localities.containsObject(locality);
        let wanted: Vec<(Motor, &GCHapticsLocality)> = unsafe {
            if has(GCHapticsLocalityLeftHandle) && has(GCHapticsLocalityRightHandle) {
                vec![
                    (Motor::Left, GCHapticsLocalityLeftHandle),
                    (Motor::Right, GCHapticsLocalityRightHandle),
                ]
            } else {
                vec![(Motor::Default, GCHapticsLocalityDefault)]
            }
        };
        let mut engines = Vec::new();
        for (motor, locality) in wanted {
            // `createEngineWithLocality:` exists on macOS 11+, but objc2-game-controller
            // only binds it for iOS. Not an ownership family: the result is +0 and retained
            // here; `Option` absorbs nil.
            let engine: Option<Retained<CHHapticEngine>> =
                unsafe { msg_send![&*haptics, createEngineWithLocality: locality] };
            engines.push((motor, engine?));
        }
        Some(Self {
            engines,
            players: HashMap::new(),
        })
    }

    fn dual(&self) -> bool {
        self.engines.len() == 2
    }

    /// Plays `rumble`; `false` once the haptics have failed (logged once).
    fn apply(&mut self, rumble: &Rumble) -> bool {
        for step in haptics_for(rumble, self.dual()) {
            match step {
                Haptic::Stop(motor) => self.stop_motor(motor),
                Haptic::Play {
                    motor,
                    intensity,
                    seconds,
                } => {
                    if let Err(error) = self.play(motor, intensity, seconds) {
                        eprintln!("Skerry: rumble stopped working on this controller: {error}");
                        return false;
                    }
                }
            }
        }
        true
    }

    fn play(&mut self, motor: Motor, intensity: f32, seconds: f64) -> Result<(), String> {
        let Some((_, engine)) = self.engines.iter().find(|(m, _)| *m == motor) else {
            return Ok(());
        };
        unsafe {
            // Starting a running engine is a no-op; one macOS stopped starts again. A start
            // that races the asynchronous stop from `stop()` fails once, so it is retried.
            engine
                .startAndReturnError()
                .or_else(|_| engine.startAndReturnError())
                .map_err(|error| error.localizedDescription().to_string())?;
            let parameters = NSArray::from_retained_slice(&[
                CHHapticEventParameter::initWithParameterID_value(
                    CHHapticEventParameter::alloc(),
                    CHHapticEventParameterIDHapticIntensity,
                    intensity,
                ),
                CHHapticEventParameter::initWithParameterID_value(
                    CHHapticEventParameter::alloc(),
                    CHHapticEventParameterIDHapticSharpness,
                    0.5,
                ),
            ]);
            let event = CHHapticEvent::initWithEventType_parameters_relativeTime_duration(
                CHHapticEvent::alloc(),
                CHHapticEventTypeHapticContinuous,
                &parameters,
                0.,
                seconds,
            );
            let pattern = CHHapticPattern::initWithEvents_parameters_error(
                CHHapticPattern::alloc(),
                &NSArray::from_retained_slice(&[event]),
                &NSArray::new(),
            )
            .map_err(|error| error.localizedDescription().to_string())?;
            let player = engine
                .createPlayerWithPattern_error(&pattern)
                .map_err(|error| error.localizedDescription().to_string())?;
            player
                .startAtTime_error(0.)
                .map_err(|error| error.localizedDescription().to_string())?;
            self.players.insert(motor, player);
        }
        Ok(())
    }

    fn stop_motor(&mut self, motor: Motor) {
        if let Some(player) = self.players.remove(&motor) {
            let _ = unsafe { player.stopAtTime_error(0.) };
        }
    }

    /// Stops every player, then the engines, so nothing keeps the controller awake; the
    /// next rumble starts them again.
    fn stop(&mut self) {
        let motors: Vec<Motor> = self.players.keys().copied().collect();
        for motor in motors {
            self.stop_motor(motor);
        }
        for (_, engine) in &self.engines {
            // No completion handler; the engine stops asynchronously.
            unsafe { engine.stopWithCompletionHandler(std::ptr::null_mut()) };
        }
    }
}
