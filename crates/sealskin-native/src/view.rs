use crate::app::ToggleSessionControls;
use crate::settings::{Key, ScreenChange};
use crate::transport::{
    self, Frame, Input, Picture, Pointer, Resolution, SessionEvent, SessionOptions,
};
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

const MENU_SIZE: Pixels = px(32.);
const MENU_MARGIN: Pixels = px(8.);

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
        options: SessionOptions,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        let (frame_tx, frames) = watch::channel(Frame::default());
        let (input, commands) = mpsc::unbounded_channel();
        let (pointer, motion) = watch::channel(None);
        let view = cx.new(|cx| Desktop::new(input.clone(), pointer, frames, window, cx));
        window.focus(&view.read(cx).focus);
        let task = runtime.spawn(async move {
            if let Err(message) =
                transport::run(address, frame_tx, commands, motion, options, &events).await
            {
                let _ = events.send(SessionEvent::Error(message.into()));
            }
        });
        Self { view, input, task }
    }

    pub fn view(&self) -> AnyView {
        self.view.clone().into()
    }

    pub fn focus(&self, window: &mut Window, cx: &App) {
        window.focus(&self.view.read(cx).focus);
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

    pub fn set_resolution(&self, size: Resolution, scale: Option<f32>) {
        let _ = self.input.send(Input::Resolution(size, scale));
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
    scale_locally: bool,
    frame: Frame,
    painted: Rc<RefCell<Option<Arc<gpui::RenderImage>>>>,
    painted_display: Option<PaintedDisplay>,
    input: mpsc::UnboundedSender<Input>,
    pointer: watch::Sender<Option<Pointer>>,
    buttons: u8,
    position: Option<(Resolution, (u16, u16))>,
    keys: HashMap<String, u32>,
    modifiers: Modifiers,
    scroll: (f32, f32),
}

impl Desktop {
    fn new(
        input: mpsc::UnboundedSender<Input>,
        pointer: watch::Sender<Option<Pointer>>,
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
                        if (desktop.frame.width, desktop.frame.height)
                            != (frame.width, frame.height)
                        {
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
            scale_locally: true,
            frame: Frame::default(),
            painted: Rc::default(),
            painted_display: None,
            input,
            pointer,
            buttons: 0,
            position: None,
            keys: HashMap::new(),
            modifiers: Modifiers::default(),
            scroll: (0.0, 0.0),
        }
    }

    fn command(&self, text: String) {
        let _ = self.input.send(Input::Command(text));
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
        self.pointer.send_replace(None);
        self.send_pointer();
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
        self.release_pointer();
    }

    fn release_pointer(&mut self) {
        self.pointer.send_replace(None);
        self.position = None;
        if std::mem::take(&mut self.buttons) != 0 {
            self.command("m2,0,0,0,0".into());
        }
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
        let menu = picture.and_then(|picture| menu_position(bounds, picture));
        let painted = self.painted.clone();
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
                desktop.update(cx, |desktop, _| desktop.painted_display = display);
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
                                desktop.pointer.send_replace(desktop.mouse_state());
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
                                desktop.send_pointer();
                                desktop.scroll(event);
                            }
                        });
                    }
                });
                let mut jpeg = None;
                if let (Some(display), Some(picture)) = (display, &frame.picture) {
                    match picture {
                        Picture::Jpeg(image) => {
                            let _ = window.paint_image(
                                display.bounds,
                                Corners::default(),
                                image.clone(),
                                0,
                                false,
                            );
                            jpeg = Some(image.clone());
                        }
                        Picture::Video(surface) => {
                            window.paint_surface(display.bounds, surface.pixel_buffer())
                        }
                    }
                }
                if let Some(old) = painted.replace(jpeg.clone()) {
                    if !jpeg.as_ref().is_some_and(|new| Arc::ptr_eq(new, &old)) {
                        let _ = window.drop_image(old);
                    }
                }
            },
        )
        .size_full();
        div()
            .relative()
            .size_full()
            .overflow_hidden()
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
            .when_some(menu, |view, position| {
                view.child(
                    div()
                        .id("session-controls-trigger")
                        .absolute()
                        .top(position.y)
                        .left(position.x)
                        .size(MENU_SIZE)
                        .rounded_md()
                        .bg(rgb(0x22262f))
                        .text_color(rgb(0xf1f3f6))
                        .hover(|button| button.bg(rgb(0x354258)))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child("•••")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ToggleSessionControls), cx)
                        }),
                )
            })
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
