//! Mirror: the GPUI view that shows the Active Tab's Frames and captures
//! input for it.
//!
//! * Frames are decoded off the main thread into BGRA `RenderImage`s and
//!   painted scaled and letterboxed ([`FrameFit`]) into the view's bounds.
//!   The previous image is dropped from gpui's sprite atlas after every new
//!   Frame, so a long stream does not grow texture memory (see
//!   docs/gpui-notes.md).
//! * A Frame is acknowledged only after the view has taken it and requested
//!   a repaint; at most one Frame is in flight.
//! * Mouse, wheel and keyboard input is mapped to CDP by [`input_map`] and sent
//!   through [`MirrorBackend::input`]. Cmd combinations are never forwarded.

mod backend;
pub mod input_map;

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    canvas, div, point, prelude::*, px, rgb, size, App, Bounds, Context, Corners, DispatchPhase,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, HitboxBehavior,
    IntoElement, KeyDownEvent, KeyUpEvent, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Render, RenderImage, ScrollWheelEvent, UTF16Selection, Window,
};
use rb_cdp::{FrameFit, FrameMetadata, MouseButton as CdpButton, MouseEvent, Point};
use smallvec::SmallVec;
use tokio::sync::mpsc::UnboundedSender;

pub use backend::{MirrorBackend, MirrorInput};

/// Decode JPEG bytes to a BGRA image ready for gpui.
pub fn decode_jpeg(data: &[u8]) -> Option<Arc<RenderImage>> {
    let img = image::load_from_memory_with_format(data, image::ImageFormat::Jpeg).ok()?;
    let mut rgba = img.into_rgba8();
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2); // RGBA -> BGRA, as gpui expects
    }
    Some(Arc::new(RenderImage::new(SmallVec::from_elem(
        image::Frame::new(rgba),
        1,
    ))))
}

pub struct Mirror {
    focus: FocusHandle,
    image: Option<Arc<RenderImage>>,
    meta: FrameMetadata,
    /// Images replaced by newer ones, dropped from the atlas during paint.
    retired: Rc<RefCell<Vec<Arc<RenderImage>>>>,
    /// Bounds of the Mirror in window coordinates, set during prepaint.
    bounds: Rc<Cell<gpui::Bounds<Pixels>>>,
    input: UnboundedSender<MirrorInput>,
    /// Bitmask of mouse buttons pressed over the Mirror (see `MouseButton::mask`).
    held: u32,
    /// Keys whose press was forwarded; a key-up without it (for example the
    /// Enter that was pressed in the address field) is not forwarded.
    keys_down: std::collections::HashSet<String>,
    /// IME composition in progress (display only).
    marked: Option<String>,
    frames_shown: u64,
}

impl Mirror {
    pub fn new(backend: MirrorBackend, cx: &mut Context<Self>) -> Self {
        let MirrorBackend {
            mut frames,
            input,
            runtime,
        } = backend;
        cx.spawn(async move |this, cx| {
            while let Some(mut frame) = frames.recv().await {
                let data = std::mem::take(&mut frame.data);
                let decoded = cx
                    .background_executor()
                    .spawn(async move { decode_jpeg(&data) })
                    .await;
                let meta = frame.metadata;
                let alive = this
                    .update(cx, |m, cx| {
                        if let Some(img) = decoded {
                            m.set_frame(img, meta, cx);
                        }
                    })
                    .is_ok();
                // Ack only after the view has taken the Frame and requested a
                // repaint (also on decode failure, so the stream never stalls).
                runtime.spawn(async move {
                    let _ = frame.ack().await;
                });
                if !alive {
                    break;
                }
            }
        })
        .detach();
        Mirror {
            focus: cx.focus_handle(),
            image: None,
            meta: FrameMetadata::default(),
            retired: Rc::default(),
            bounds: Rc::default(),
            input,
            held: 0,
            keys_down: Default::default(),
            marked: None,
            frames_shown: 0,
        }
    }

    fn set_frame(&mut self, img: Arc<RenderImage>, meta: FrameMetadata, cx: &mut Context<Self>) {
        if let Some(old) = self.image.replace(img) {
            self.retired.borrow_mut().push(old);
        }
        self.meta = meta;
        self.frames_shown += 1;
        cx.notify();
    }

    /// Number of Frames shown so far.
    pub fn frames_shown(&self) -> u64 {
        self.frames_shown
    }

    fn fit(&self) -> FrameFit {
        let b = self.bounds.get();
        FrameFit::for_frame(
            &self.meta,
            f32::from(b.size.width),
            f32::from(b.size.height),
        )
    }

    fn local(&self, p: gpui::Point<Pixels>) -> Point {
        let o = self.bounds.get().origin;
        Point::new(f32::from(p.x - o.x), f32::from(p.y - o.y))
    }

    fn send(&self, msg: MirrorInput) {
        let _ = self.input.send(msg);
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, hovered: bool, window: &mut Window) {
        if !hovered {
            return;
        }
        // Only presses inside the Frame (not the letterbox bars) count.
        let Some(p) = input_map::map_position(&self.fit(), self.local(ev.position), false) else {
            return;
        };
        window.focus(&self.focus);
        let button = input_map::cdp_button(ev.button);
        self.held |= button.mask();
        let msg = MouseEvent::pressed(p.x as f64, p.y as f64, button, ev.click_count as u32)
            .with_buttons(self.held)
            .with_modifiers(input_map::cdp_modifiers(&ev.modifiers));
        self.send(MirrorInput::Mouse(msg));
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent) {
        let button = input_map::cdp_button(ev.button);
        if self.held & button.mask() == 0 {
            return; // press did not start on the Mirror
        }
        let p =
            input_map::map_position(&self.fit(), self.local(ev.position), true).unwrap_or_default();
        self.held &= !button.mask();
        let msg = MouseEvent::released(p.x as f64, p.y as f64, button, ev.click_count as u32)
            .with_buttons(self.held)
            .with_modifiers(input_map::cdp_modifiers(&ev.modifiers));
        self.send(MirrorInput::Mouse(msg));
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, hovered: bool) {
        let dragging = self.held != 0;
        if !dragging && !hovered {
            return;
        }
        let Some(p) = input_map::map_position(&self.fit(), self.local(ev.position), dragging)
        else {
            return;
        };
        let mut msg = MouseEvent::moved(p.x as f64, p.y as f64)
            .with_buttons(self.held)
            .with_modifiers(input_map::cdp_modifiers(&ev.modifiers));
        if self.held & CdpButton::Left.mask() != 0 {
            msg.button = CdpButton::Left;
        } else if self.held & CdpButton::Right.mask() != 0 {
            msg.button = CdpButton::Right;
        } else if self.held & CdpButton::Middle.mask() != 0 {
            msg.button = CdpButton::Middle;
        }
        self.send(MirrorInput::Mouse(msg));
    }

    fn scroll(&mut self, ev: &ScrollWheelEvent, hovered: bool) {
        if !hovered {
            return;
        }
        let Some(p) = input_map::map_position(&self.fit(), self.local(ev.position), false) else {
            return;
        };
        let (dx, dy) = input_map::wheel_deltas(&ev.delta);
        let msg = MouseEvent::wheel(p.x as f64, p.y as f64, dx, dy)
            .with_buttons(self.held)
            .with_modifiers(input_map::cdp_modifiers(&ev.modifiers));
        self.send(MirrorInput::Mouse(msg));
    }

    fn key_down(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        if let Some(k) = input_map::key_down_events(&ev.keystroke) {
            self.keys_down.insert(ev.keystroke.key.clone());
            self.send(MirrorInput::Key(k));
            // Handled: keep it away from the IME and from app shortcuts.
            cx.stop_propagation();
        }
    }

    fn key_up(&mut self, ev: &KeyUpEvent, cx: &mut Context<Self>) {
        if !self.keys_down.remove(&ev.keystroke.key) {
            return;
        }
        if let Some(k) = input_map::key_up_event(&ev.keystroke) {
            self.send(MirrorInput::Key(k));
            cx.stop_propagation();
        }
    }
}

impl Focusable for Mirror {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// IME / composed text. Keystrokes we map ourselves never reach the IME
/// (their events are stopped), so this receives text that does not come from
/// a mapped key: emoji picker, dictation, press-and-hold accents, and
/// composition if the platform started one. Composition itself (for example
/// CJK input methods) is therefore not supported for typed keys; see
/// docs/gpui-notes.md.
impl EntityInputHandler for Mirror {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| 0..utf16_len(m))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        if !text.is_empty() {
            self.send(MirrorInput::InsertText(text.to_string()));
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!new_text.is_empty()).then(|| new_text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Anchor the candidate window at the Mirror's top-left.
        Some(Bounds::new(element_bounds.origin, size(px(1.), px(20.))))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for Mirror {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view: Entity<Mirror> = cx.entity();
        let image = self.image.clone();
        let meta = self.meta;
        let retired = self.retired.clone();
        let bounds_cell = self.bounds.clone();
        let focus = self.focus.clone();

        let surface = canvas(
            move |bounds, window, _cx| {
                bounds_cell.set(bounds);
                window.insert_hitbox(bounds, HitboxBehavior::Normal)
            },
            move |bounds, hitbox, window, cx| {
                window.handle_input(&focus, ElementInputHandler::new(bounds, view.clone()), cx);

                let v = view.clone();
                let hb = hitbox.clone();
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        let hovered = hb.is_hovered(window);
                        v.update(cx, |m, _| m.mouse_down(ev, hovered, window));
                    }
                });
                let v = view.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, _window, cx| {
                    if phase == DispatchPhase::Bubble {
                        v.update(cx, |m, _| m.mouse_up(ev));
                    }
                });
                let v = view.clone();
                let hb = hitbox.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        let hovered = hb.is_hovered(window);
                        v.update(cx, |m, _| m.mouse_move(ev, hovered));
                    }
                });
                let v = view.clone();
                let hb = hitbox;
                window.on_mouse_event(move |ev: &ScrollWheelEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        let hovered = hb.is_hovered(window);
                        v.update(cx, |m, _| m.scroll(ev, hovered));
                    }
                });

                if let Some(img) = image {
                    let fit = FrameFit::for_frame(
                        &meta,
                        f32::from(bounds.size.width),
                        f32::from(bounds.size.height),
                    );
                    if !fit.is_empty() {
                        let r = fit.dest_rect();
                        let dest = Bounds::new(
                            point(bounds.origin.x + px(r.x), bounds.origin.y + px(r.y)),
                            size(px(r.width), px(r.height)),
                        );
                        let _ = window.paint_image(dest, Corners::default(), img, 0, false);
                    }
                }
                // The previous Frames are no longer painted: free their textures.
                for old in retired.borrow_mut().drain(..) {
                    let _ = window.drop_image(old);
                }
            },
        )
        .size_full();

        let status = if self.image.is_none() {
            Some("Waiting for a Frame...".to_string())
        } else {
            self.marked.as_ref().map(|m| format!("composing: {m}"))
        };

        div()
            .key_context("Mirror")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.key_down(ev, cx)))
            .on_key_up(cx.listener(|this, ev: &KeyUpEvent, _, cx| this.key_up(ev, cx)))
            .relative()
            .size_full()
            .bg(rgb(0x0b0b0c))
            .child(surface)
            .children(status.map(|s| {
                div()
                    .absolute()
                    .left(px(12.))
                    .bottom(px(8.))
                    .text_color(rgb(0x9a9a9a))
                    .child(s)
            }))
    }
}
