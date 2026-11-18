use crate::transport::{self, Frame, Input, SessionEvent};
use gpui::{
    canvas, div, point, prelude::*, px, rgb, size, AnyView, App, Bounds, Context, Corners,
    DispatchPhase, Entity, FocusHandle, HitboxBehavior, KeyDownEvent, KeyUpEvent, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta,
    ScrollWheelEvent, Window,
};
use reqwest::Url;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use tokio::{
    runtime::Handle,
    sync::{mpsc, watch},
};

pub struct NativeSession {
    view: Entity<Desktop>,
    input: mpsc::UnboundedSender<Input>,
    task: tokio::task::JoinHandle<()>,
}

impl NativeSession {
    pub fn open(
        address: Url,
        window: &mut Window,
        cx: &mut App,
        runtime: &Handle,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        let (frame_tx, frames) = watch::channel(Frame::default());
        let (input, commands) = mpsc::unbounded_channel();
        let (pointer, motion) = watch::channel(None);
        let view = cx.new(|cx| Desktop::new(input.clone(), pointer, frames, window, cx));
        window.focus(&view.read(cx).focus);
        let task = runtime.spawn(async move {
            if let Err(message) = transport::run(address, frame_tx, commands, motion, &events).await
            {
                let _ = events.send(SessionEvent::Error(message.into()));
            }
        });
        Self { view, input, task }
    }

    pub fn view(&self) -> AnyView {
        self.view.clone().into()
    }

    pub fn set_audio(&self, enabled: bool) {
        let _ = self.input.send(Input::Audio(enabled));
    }

    pub fn release_focus(&self, window: &mut Window, cx: &mut App) {
        self.view.update(cx, |desktop, _| {
            desktop.release_input();
            if let Some(image) = desktop.painted.borrow_mut().take() {
                let _ = window.drop_image(image);
            }
        });
    }
}

impl Drop for NativeSession {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Desktop {
    focus: FocusHandle,
    frame: Frame,
    painted: Rc<RefCell<Option<Arc<gpui::RenderImage>>>>,
    bounds: Option<Bounds<Pixels>>,
    input: mpsc::UnboundedSender<Input>,
    pointer: watch::Sender<Option<String>>,
    buttons: u8,
    position: (u16, u16),
    keys: HashMap<String, u32>,
    modifiers: Modifiers,
    scroll: (f32, f32),
}

impl Desktop {
    fn new(
        input: mpsc::UnboundedSender<Input>,
        pointer: watch::Sender<Option<String>>,
        mut frames: watch::Receiver<Frame>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        cx.on_blur(&focus, window, |desktop, _, _| desktop.release_input())
            .detach();
        cx.observe_window_activation(window, |desktop, window, _| {
            if !window.is_window_active() {
                desktop.release_input();
            }
        })
        .detach();
        cx.spawn(async move |desktop, cx| {
            while frames.changed().await.is_ok() {
                let frame = frames.borrow_and_update().clone();
                if desktop
                    .update(cx, |desktop, cx| {
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
            frame: Frame::default(),
            painted: Rc::default(),
            bounds: None,
            input,
            pointer,
            buttons: 0,
            position: (0, 0),
            keys: HashMap::new(),
            modifiers: Modifiers::default(),
            scroll: (0.0, 0.0),
        }
    }

    fn command(&self, text: String) {
        let _ = self.input.send(Input::Command(text));
    }

    fn mouse_position(&mut self, point: Point<Pixels>, clamp: bool) -> bool {
        let Some(bounds) = self
            .bounds
            .filter(|b| b.size.width > px(0.) && b.size.height > px(0.))
        else {
            return false;
        };
        if !clamp && !bounds.contains(&point) {
            return false;
        }
        let x = (point.x - bounds.origin.x) / bounds.size.width;
        let y = (point.y - bounds.origin.y) / bounds.size.height;
        self.position = (
            (x * f32::from(self.frame.width))
                .clamp(0., f32::from(self.frame.width.saturating_sub(1))) as u16,
            (y * f32::from(self.frame.height))
                .clamp(0., f32::from(self.frame.height.saturating_sub(1))) as u16,
        );
        true
    }

    fn mouse_state(&self) -> String {
        format!(
            "m,{},{},{},0",
            self.position.0, self.position.1, self.buttons
        )
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
        self.pointer.send_replace(None);
        self.command(self.mouse_state());
    }

    fn scroll(&mut self, event: &ScrollWheelEvent) {
        let (x, y) = match event.delta {
            ScrollDelta::Lines(p) => (p.x, p.y),
            ScrollDelta::Pixels(p) => (f32::from(p.x) / 40., f32::from(p.y) / 40.),
        };
        self.scroll.0 += x;
        self.scroll.1 += y;
        let (x, y) = (self.scroll.0.trunc(), self.scroll.1.trunc());
        self.scroll.0 -= x;
        self.scroll.1 -= y;
        for (delta, positive, negative) in [(x, 64, 128), (y, 16, 8)] {
            if delta == 0. {
                continue;
            }
            let mask = self.buttons | if delta > 0. { positive } else { negative };
            let amount = (delta.abs() as u32).min(16);
            self.command(format!("m2,0,0,{mask},{amount}"));
            self.command(format!("m2,0,0,{},0", self.buttons));
        }
    }

    fn modifiers(&mut self, next: Modifiers) {
        for (current, next, key) in [
            (self.modifiers.shift, next.shift, 0xffe1),
            (self.modifiers.control, next.control, 0xffe3),
            (self.modifiers.alt, next.alt, 0xffe9),
        ] {
            if current != next {
                self.command(format!("{},{key}", if next { "kd" } else { "ku" }));
            }
        }
        self.modifiers = next;
    }

    fn release_input(&mut self) {
        for (_, key) in std::mem::take(&mut self.keys) {
            self.command(format!("ku,{key}"));
        }
        self.modifiers(Modifiers::default());
        if self.buttons != 0 {
            self.buttons = 0;
            self.pointer.send_replace(None);
            self.command(self.mouse_state());
        }
    }
}

impl Render for Desktop {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let desktop = cx.entity();
        let frame = self.frame.clone();
        let empty = frame.image.is_none();
        let painted = self.painted.clone();
        let surface = canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, cx| {
                let target = fit(bounds, frame.width, frame.height);
                desktop.update(cx, |desktop, _| desktop.bounds = target);
                let v = desktop.clone();
                let hb = hitbox.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && hb.is_hovered(window) {
                        v.update(cx, |desktop, _| {
                            if desktop.mouse_position(event.position, false) {
                                window.focus(&desktop.focus);
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
                        v.update(cx, |desktop, _| {
                            let dragging = desktop.buttons != 0;
                            if (hovered || dragging)
                                && desktop.mouse_position(event.position, dragging)
                            {
                                desktop.pointer.send_replace(Some(desktop.mouse_state()));
                            }
                        });
                    }
                });
                let v = desktop.clone();
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                        v.update(cx, |desktop, _| {
                            if desktop.mouse_position(event.position, false) {
                                desktop.pointer.send_replace(None);
                                desktop.command(desktop.mouse_state());
                                desktop.scroll(event);
                            }
                        });
                    }
                });
                if let (Some(target), Some(image)) = (target, &frame.image) {
                    let _ = window.paint_image(target, Corners::default(), image.clone(), 0, false);
                }
                if let Some(old) = painted.replace(frame.image.clone()) {
                    if !frame
                        .image
                        .as_ref()
                        .is_some_and(|new| Arc::ptr_eq(new, &old))
                    {
                        let _ = window.drop_image(old);
                    }
                }
            },
        )
        .size_full();
        div()
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|desktop, event: &KeyDownEvent, _, cx| {
                if event.keystroke.modifiers.platform {
                    return;
                }
                desktop.modifiers(event.keystroke.modifiers);
                if let Some(key) = keysym(&event.keystroke.key, event.keystroke.key_char.as_deref())
                {
                    desktop.keys.insert(event.keystroke.key.clone(), key);
                    desktop.command(format!("kd,{key}"));
                    cx.stop_propagation();
                }
            }))
            .on_key_up(cx.listener(|desktop, event: &KeyUpEvent, _, cx| {
                if let Some(key) = desktop.keys.remove(&event.keystroke.key) {
                    desktop.command(format!("ku,{key}"));
                    cx.stop_propagation();
                }
            }))
            .on_modifiers_changed(cx.listener(
                |desktop, event: &gpui::ModifiersChangedEvent, _, _| {
                    desktop.modifiers(event.modifiers);
                },
            ))
            .child(surface)
            .when(empty, |view| {
                view.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(0xb6bdcc))
                        .child("Waiting for the remote display…"),
                )
            })
    }
}

fn fit(bounds: Bounds<Pixels>, width: u16, height: u16) -> Option<Bounds<Pixels>> {
    if width == 0 || height == 0 {
        return None;
    }
    let scale = (f32::from(bounds.size.width) / f32::from(width))
        .min(f32::from(bounds.size.height) / f32::from(height));
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
