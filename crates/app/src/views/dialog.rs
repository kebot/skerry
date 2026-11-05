//! The JS dialog modal: drawn locally over the Mirror for the Active Tab,
//! answered with `Connection::handle_dialog`. Child module of `views`.
//!
//! The modal does not hide when a button is pressed: it goes away when the
//! dialog leaves `Connection::pending_dialogs` (answered, closed by the page
//! or the Agent, navigation, Connection down). While it is shown, mouse input
//! is blocked by the backdrop and keys go to the modal (Enter = OK / Leave,
//! Escape = Cancel / Stay; a prompt's text field takes typing), never to the
//! Mirror.

use gpui::{
    div, prelude::*, px, AnyElement, Context, FocusHandle, KeyDownEvent, MouseButton,
    MouseDownEvent, SharedString, Window,
};
use rb_cdp::{Dialog, DialogKind};

use super::sidebar::badge;
use super::{DialogInput, Msg, Root};
use crate::dialog_model::{self, DialogKey};
use crate::sidebar_model::LineEdit;
use crate::theme::{hsla, Palette};

impl Root {
    /// Create or drop the modal state to match the Active Tab's pending
    /// dialog, and move keyboard focus to the modal (and back to the Mirror
    /// when it is gone).
    pub(super) fn sync_dialog_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cur = self
            .active_dialog()
            .map(|(ci, d)| ((ci, d.target_id.clone()), d));
        match (cur, self.dialog_input.as_ref().map(|d| d.key.clone())) {
            (Some((key, d)), shown) if shown.as_ref() != Some(&key) => {
                let edit = match dialog_model::view_for(&d).prompt {
                    Some(text) => LineEdit::with_text(&text),
                    None => LineEdit::default(),
                };
                let focus: FocusHandle = cx.focus_handle();
                window.focus(&focus);
                self.dialog_input = Some(DialogInput {
                    key,
                    edit,
                    focus,
                    busy: false,
                });
            }
            (None, Some(_)) => {
                self.dialog_input = None;
                self.focus_mirror = true;
            }
            _ => {}
        }
    }

    /// Answer the Active Tab's dialog (the modal stays until it is gone).
    pub(super) fn answer_dialog(&mut self, accept: bool, cx: &mut Context<Self>) {
        let Some((ci, d)) = self.active_dialog() else {
            return;
        };
        let Some(input) = &mut self.dialog_input else {
            return;
        };
        if input.busy {
            return;
        }
        input.busy = true;
        let resp = dialog_model::response(d.kind, accept, &input.edit.buf);
        let Some(conn) = self.conns[ci].conn.clone() else {
            return;
        };
        let (tx, gen, target) = (self.ui_tx.clone(), self.conns[ci].gen, d.target_id);
        self.rt.spawn(async move {
            let r = conn
                .handle_dialog(&target, resp)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::DialogAnswered(ci, gen, r));
        });
        cx.notify();
    }

    fn dialog_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        if ks.modifiers.platform {
            return; // app shortcuts pass through
        }
        let Some((_, d)) = self.active_dialog() else {
            return;
        };
        let Some(input) = &mut self.dialog_input else {
            return;
        };
        let out = dialog_model::key(
            d.kind,
            &mut input.edit,
            ks.key.as_str(),
            ks.key_char.as_deref(),
            ks.modifiers.control || ks.modifiers.function,
        );
        match out {
            DialogKey::Answer(accept) => self.answer_dialog(accept, cx),
            DialogKey::Edited => {}
            DialogKey::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The modal over the Mirror area, if the Active Tab has a pending dialog.
    pub(super) fn dialog_overlay(
        &mut self,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (ci, d): (usize, Dialog) = self.active_dialog()?;
        let input = self.dialog_input.as_ref()?;
        let (focus, busy, text) = (input.focus.clone(), input.busy, input.edit.buf.clone());
        let view = dialog_model::view_for(&d);
        let style = self.conn_styles.get(ci).cloned();
        let conn_name = self.conns[ci].name.clone();
        let is_prompt = d.kind == DialogKind::Prompt;

        let mut buttons = div().flex().justify_center().gap(px(8.)).mt(px(14.));
        for (n, b) in view.buttons.iter().copied().enumerate() {
            buttons = buttons.child(
                div()
                    .id(SharedString::from(format!("dialog-btn-{n}")))
                    .min_w(px(84.))
                    .py(px(4.))
                    .flex()
                    .justify_center()
                    .rounded(px(7.))
                    .cursor_pointer()
                    .when(b.primary, |d| {
                        d.bg(hsla(p.accent)).text_color(gpui::white())
                    })
                    .when(!b.primary, |d| d.bg(hsla(p.hover)).text_color(hsla(p.ink)))
                    .when(busy, |d| d.opacity(0.5))
                    .hover(|s| s.opacity(0.85))
                    .child(b.label)
                    .on_click(cx.listener(move |this, _, _, cx| this.answer_dialog(b.accept, cx))),
            );
        }
        let card = div()
            .id("dialog-card")
            .track_focus(&focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.dialog_key(ev, cx)))
            .w(px(320.))
            .p(px(18.))
            .rounded(px(12.))
            .bg(hsla(p.win))
            .border_1()
            .border_color(hsla(p.line))
            .shadow_lg()
            .flex()
            .flex_col()
            .items_center()
            .text_color(hsla(p.ink))
            .children(style.as_ref().map(|s| badge(s, p.dark, 44.)))
            .child(
                div()
                    .mt(px(10.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_size(px(13.))
                    .child(SharedString::from(view.title)),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(hsla(p.ink3))
                    .child(SharedString::from(conn_name)),
            )
            .child(
                div()
                    .mt(px(10.))
                    .max_h(px(180.))
                    .overflow_hidden()
                    .text_size(px(12.))
                    .text_color(hsla(p.ink2))
                    .child(SharedString::from(view.message)),
            )
            .when(is_prompt, |d| {
                d.child(
                    div()
                        .mt(px(10.))
                        .w_full()
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(hsla(p.accent))
                        .bg(hsla(p.field))
                        .text_size(px(12.))
                        .child(SharedString::from(format!("{text}|"))),
                )
            })
            .child(buttons);
        let f2 = focus.clone();
        Some(
            div()
                .id("dialog-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .bg(hsla(0x00000038))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_, _: &MouseDownEvent, window, _| window.focus(&f2)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}
