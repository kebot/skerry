//! Sidebar rendering and interaction wiring (the mock's Sidebar in GPUI).
//! Child module of `views`, so it can use the private fields of `Root`.

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    anchored, canvas, deferred, div, linear_color_stop, linear_gradient, point, prelude::*, px,
    AnyElement, Context, DragMoveEvent, Hsla, IntoElement, MouseButton, MouseDownEvent,
    PathBuilder, Pixels, Render, SharedString, Window,
};
use rb_store::{SidebarGroup, SidebarItem, SidebarTab};

use super::{Menu, Root};
use crate::agent_model::{self, AgentLine, Marker};
use crate::model;
use crate::multi;
use crate::sidebar_model::{self as sm, ChipAction, ChipStatus, ConnStyle, DropTarget, MenuKind};
use crate::theme::{self, hsla, Palette, Shape};

/// Drag payload: a Sidebar Tab (by stable id).
#[derive(Clone)]
pub(super) struct DragTab {
    id: String,
    label: String,
}

/// Drag payload: a Tab Group header (reorders Groups).
#[derive(Clone)]
pub(super) struct DragGroup {
    id: String,
    label: String,
}

/// The little card that follows the pointer while dragging.
struct DragGhost {
    label: String,
    p: Palette,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(10.))
            .h(px(26.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .bg(hsla(self.p.win))
            .border_1()
            .border_color(hsla(self.p.accent))
            .shadow_md()
            .text_color(hsla(self.p.ink))
            .text_size(px(13.))
            .child(SharedString::from(model::truncate(&self.label, 30)))
    }
}

/// A Connection's dot: colour plus a distinct shape (● ◆ ▲ ■).
pub(super) fn conn_dot(shape: Shape, color: Hsla, size: f32) -> impl IntoElement {
    match shape {
        Shape::Circle => div()
            .flex_none()
            .size(px(size))
            .rounded_full()
            .bg(color)
            .into_any_element(),
        Shape::Square => div()
            .flex_none()
            .size(px(size))
            .rounded(px(2.))
            .bg(color)
            .into_any_element(),
        Shape::Diamond | Shape::Triangle => canvas(
            |_, _, _| (),
            move |b, _, window, _| {
                let (x, y) = (f32::from(b.origin.x), f32::from(b.origin.y));
                let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
                let pts: Vec<(f32, f32)> = if shape == Shape::Diamond {
                    vec![
                        (x + w / 2., y),
                        (x + w, y + h / 2.),
                        (x + w / 2., y + h),
                        (x, y + h / 2.),
                    ]
                } else {
                    vec![(x + w / 2., y + 1.), (x + w, y + h - 1.), (x, y + h - 1.)]
                };
                let mut pb = PathBuilder::fill();
                pb.move_to(point(px(pts[0].0), px(pts[0].1)));
                for (px_, py_) in &pts[1..] {
                    pb.line_to(point(px(*px_), px(*py_)));
                }
                pb.close();
                if let Ok(path) = pb.build() {
                    window.paint_path(path, color);
                }
            },
        )
        .flex_none()
        .size(px(size + 2.))
        .into_any_element(),
    }
}

/// A spinning three-quarter ring (the "reconnecting" status dot).
fn spinner(color: Hsla, size: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            let t = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs_f32())
                .unwrap_or(0.);
            let a0 = (t % 1.0) * std::f32::consts::TAU;
            let a1 = a0 + std::f32::consts::PI * 1.5;
            let r = f32::from(b.size.width) / 2. - 1.;
            let c = (
                f32::from(b.origin.x) + f32::from(b.size.width) / 2.,
                f32::from(b.origin.y) + f32::from(b.size.height) / 2.,
            );
            let at = |a: f32| point(px(c.0 + r * a.cos()), px(c.1 + r * a.sin()));
            let mut pb = PathBuilder::stroke(px(2.));
            pb.move_to(at(a0));
            pb.arc_to(point(px(r), px(r)), px(0.), true, true, at(a1));
            if let Ok(path) = pb.build() {
                window.paint_path(path, color);
            }
            window.request_animation_frame();
        },
    )
    .flex_none()
    .size(px(size))
}

/// The status dot by shape and motion: round green, spinning amber ring,
/// red square, hollow grey ring.
fn status_dot(status: ChipStatus, p: &Palette) -> AnyElement {
    let ring = div()
        .absolute()
        .right(px(-4.))
        .bottom(px(-4.))
        .size(px(12.))
        .rounded_full()
        .bg(hsla(p.side))
        .flex()
        .items_center()
        .justify_center();
    match status {
        ChipStatus::Connected => ring
            .child(div().size(px(8.)).rounded_full().bg(hsla(p.ok)))
            .into_any_element(),
        ChipStatus::Reconnecting => ring.child(spinner(hsla(p.warn), 10.)).into_any_element(),
        ChipStatus::Stopped => ring
            .child(div().size(px(8.)).rounded(px(2.)).bg(hsla(p.bad)))
            .into_any_element(),
        ChipStatus::NotConnected => ring
            .child(
                div()
                    .size(px(8.))
                    .rounded_full()
                    .border_2()
                    .border_color(hsla(p.idle)),
            )
            .into_any_element(),
        ChipStatus::Failed => ring
            .child(
                div()
                    .size(px(8.))
                    .rounded_full()
                    .border_2()
                    .border_color(hsla(p.bad)),
            )
            .into_any_element(),
    }
}

fn display_title(t: &SidebarTab) -> String {
    if t.title.trim().is_empty() {
        if t.url.is_empty() {
            "New Tab".to_string()
        } else {
            model::shorten_url(&t.url, 40)
        }
    } else {
        t.title.clone()
    }
}

impl Root {
    pub(super) fn sidebar_view(
        &mut self,
        window: &mut Window,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = p.dark;
        let layout = self.layout();
        let rail = self.rail;
        let dragging = cx.has_active_drag();

        // Active Tab's Connection colour tints the whole Sidebar.
        let tint = self
            .active
            .as_ref()
            .and_then(|(c, _)| self.conn_styles.get(*c))
            .map(|s| s.color(dark));

        let mut scroll = div()
            .id("sidebar-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px(px(if rail { 6. } else { 8. }))
            .pb(px(8.))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.menu = Some(Menu {
                        kind: MenuKind::Sidebar,
                        pos: ev.position,
                    });
                    cx.notify();
                }),
            );

        scroll = scroll.child(self.new_tab_row(p, dark, cx));

        // Pinned Tabs.
        let pins: Vec<&SidebarTab> = layout
            .iter()
            .filter_map(|i| match i {
                SidebarItem::Tab(t) if t.pinned => Some(t),
                _ => None,
            })
            .collect();
        if !pins.is_empty() || dragging {
            let mut grid = div().flex().flex_row().flex_wrap().gap(px(6.)).pb(px(6.));
            for t in pins {
                grid = grid.child(self.pin_cell(t, p, dark, cx));
            }
            if dragging {
                grid =
                    grid.child(
                        div()
                            .id("pin-end")
                            .w(px(if rail { 46. } else { 58. }))
                            .h(px(44.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(hsla(if self.drop_hint == Some(DropTarget::PinnedEnd) {
                                p.accent
                            } else {
                                p.line
                            }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.))
                            .text_color(hsla(p.ink3))
                            .child("Pin")
                            .on_drag_move::<DragTab>(cx.listener(
                                |this, ev: &DragMoveEvent<DragTab>, _, cx| {
                                    if ev.bounds.contains(&ev.event.position) {
                                        this.set_drop_hint(DropTarget::PinnedEnd, cx);
                                    }
                                },
                            ))
                            .on_drop::<DragTab>(cx.listener(|this, d: &DragTab, _, cx| {
                                this.drop_tab(&d.id.clone(), cx)
                            })),
                    );
            }
            scroll = scroll.child(grid);
        }

        // Groups, then the "Tabs" label and ungrouped Tabs.
        let group_ids: Vec<String> = layout
            .iter()
            .filter_map(|i| match i {
                SidebarItem::Group(g) => Some(g.id.clone()),
                _ => None,
            })
            .collect();
        for item in &layout {
            if let SidebarItem::Group(g) = item {
                scroll = scroll.child(self.group_view(g, &group_ids, p, dark, cx));
            }
        }
        let has_groups = !group_ids.is_empty();
        let loose: Vec<&SidebarTab> = layout
            .iter()
            .filter_map(|i| match i {
                SidebarItem::Tab(t) if !t.pinned => Some(t),
                _ => None,
            })
            .collect();
        if !rail && (has_groups || !loose.is_empty() || dragging) {
            scroll = scroll.child(self.tabs_label(p, cx));
        }
        for t in loose {
            scroll = scroll.child(self.tab_row(t, p, dark, cx));
        }
        // Empty space below the list: drop here to append to the ungrouped Tabs.
        scroll = scroll.child(
            div()
                .id("sidebar-end")
                .flex_1()
                .min_h(px(28.))
                .on_drag_move::<DragTab>(cx.listener(|this, ev: &DragMoveEvent<DragTab>, _, cx| {
                    if ev.bounds.contains(&ev.event.position) {
                        this.set_drop_hint(DropTarget::LooseEnd, cx);
                    }
                }))
                .on_drop::<DragTab>(
                    cx.listener(|this, d: &DragTab, _, cx| this.drop_tab(&d.id.clone(), cx)),
                ),
        );

        let header = div()
            .flex_none()
            .h(px(34.))
            .px(px(8.))
            .flex()
            .items_center()
            .when(!rail, |d| d.justify_end())
            .when(rail, |d| d.justify_center())
            .child(
                div()
                    .id("sidebar-toggle")
                    .px(px(6.))
                    .rounded(px(5.))
                    .text_color(hsla(p.ink2))
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(p.hover)))
                    .child("☰")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.rail = !this.rail;
                        cx.notify();
                    })),
            );

        let chips = self.chips_row(p, dark, cx);
        let _ = window;
        div()
            .id("sidebar")
            .relative()
            .flex_none()
            .w(px(if rail { 58. } else { 272. }))
            .h_full()
            .flex()
            .flex_col()
            .bg(hsla(p.side))
            .border_r_1()
            .border_color(hsla(p.line))
            .children(tint.map(|c| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(hsla((c & 0xffffff00) | 0x33), 0.),
                        linear_color_stop(hsla(c & 0xffffff00), 0.75),
                    ))
            }))
            .child(header)
            .child(scroll)
            .child(chips)
            .into_any_element()
    }

    fn new_tab_row(&self, p: &Palette, dark: bool, cx: &mut Context<Self>) -> AnyElement {
        let target = self.target_connection().cloned();
        let rail = self.rail;
        div()
            .id("new-tab")
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .h(px(30.))
            .mb(px(8.))
            .rounded(px(7.))
            .bg(hsla(p.field))
            .text_color(hsla(p.ink))
            .cursor_pointer()
            .hover(|s| s.bg(hsla(p.hover)))
            .when(rail, |d| d.justify_center().px(px(0.)))
            .child(div().w(px(16.)).text_center().text_size(px(16.)).child("+"))
            .when(!rail, |d| {
                d.child(div().text_color(hsla(p.ink2)).child("New Tab"))
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(11.))
                            .text_color(hsla(p.ink3))
                            .children(target.map(|s| badge(&s, dark, 16.)))
                            .child("⌘T"),
                    )
            })
            .on_click(cx.listener(|this, _, _, cx| this.new_tab(cx)))
            .into_any_element()
    }

    fn tabs_label(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let hot = self.drop_hint == Some(DropTarget::LooseStart);
        div()
            .id("tabs-label")
            .flex()
            .items_center()
            .px(px(8.))
            .pt(px(10.))
            .pb(px(3.))
            .text_size(px(11.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(hsla(if hot { p.accent } else { p.ink3 }))
            .child(div().child("Tabs"))
            .child(
                div()
                    .id("new-group")
                    .ml_auto()
                    .px(px(6.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(p.hover)))
                    .child("+ Group")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let mut g = None;
                        this.edit_state(cx, |s| {
                            g = sm::apply_menu_action(s, &sm::MenuAction::NewGroup)
                        });
                        if let Some(g) = g {
                            this.start_rename(&g, window, cx);
                        }
                    })),
            )
            .on_drag_move::<DragTab>(cx.listener(|this, ev: &DragMoveEvent<DragTab>, _, cx| {
                if ev.bounds.contains(&ev.event.position) {
                    this.set_drop_hint(DropTarget::LooseStart, cx);
                }
            }))
            .on_drop::<DragTab>(
                cx.listener(|this, d: &DragTab, _, cx| this.drop_tab(&d.id.clone(), cx)),
            )
            .into_any_element()
    }

    /// One pinned Tab: a square cell with the favicon stand-in and the dot.
    fn pin_cell(
        &self,
        t: &SidebarTab,
        p: &Palette,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let live = self.is_live_target(&t.connection, t.target_id.as_deref());
        let dim = t.placeholder || !live;
        let active = live && self.is_active(&t.connection, t.target_id.as_deref());
        let alert = self.has_dialog_badge(&t.connection, t.target_id.as_deref());
        let agent = self.agent_marker(&t.connection, t.target_id.as_deref());
        let style = sm::style_of(&self.conn_styles, &t.connection);
        let ccol = style.map(|s| s.color(dark)).unwrap_or(p.idle);
        let id = t.id.clone();
        let title = display_title(t);
        let rail = self.rail;
        let hint = match &self.drop_hint {
            Some(DropTarget::Tab { id: h, after }) if *h == t.id && cx.has_active_drag() => {
                Some(*after)
            }
            _ => None,
        };
        let (id_click, id_menu, id_drag, id_move, id_drop) =
            (id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
        let _ = id_drop;
        div()
            .id(SharedString::from(format!("pin-{id}")))
            .relative()
            .w(px(if rail { 46. } else { 58. }))
            .h(px(44.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(if active { p.sel } else { p.field }))
            .when(active, |d| {
                d.border_1().border_color(hsla((ccol & 0xffffff00) | 0x8c))
            })
            .when(!active, |d| d.hover(|s| s.bg(hsla(p.hover))))
            .when(dim, |d| d.opacity(0.45))
            .cursor_pointer()
            .font_weight(gpui::FontWeight::BOLD)
            .text_size(px(15.))
            .text_color(hsla(p.ink2))
            .child(SharedString::from(sm::fav_text(&t.title, &t.url)))
            .children(alert.then(|| dialog_badge(p).absolute().left(px(5.)).top(px(4.))))
            .children(agent.shown().then(|| {
                agent_mark(agent, 12., p)
                    .absolute()
                    .left(px(4.))
                    .top(px(2.))
            }))
            .child(
                div()
                    .absolute()
                    .right(px(6.))
                    .bottom(px(6.))
                    .children(style.map(|s| conn_dot(s.shape, hsla(ccol), 10.))),
            )
            .when_some(hint, |d, after| {
                d.child(drop_bar_vertical(after, hsla(p.accent)))
            })
            .on_click(
                cx.listener(move |this, _, window, cx| this.activate_tab(&id_click, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.menu = Some(Menu {
                        kind: MenuKind::Tab(id_menu.clone()),
                        pos: ev.position,
                    });
                    cx.notify();
                }),
            )
            .on_drag(
                DragTab {
                    id: id_drag,
                    label: title,
                },
                {
                    let pal = *p;
                    move |d, _, _, cx| {
                        cx.new(|_| DragGhost {
                            label: d.label.clone(),
                            p: pal,
                        })
                    }
                },
            )
            .on_drag_move::<DragTab>(cx.listener(
                move |this, ev: &DragMoveEvent<DragTab>, _, cx| {
                    if ev.bounds.contains(&ev.event.position) {
                        let after = sm::is_after(
                            f32::from(ev.event.position.x - ev.bounds.origin.x),
                            f32::from(ev.bounds.size.width),
                        );
                        this.set_drop_hint(
                            DropTarget::Tab {
                                id: id_move.clone(),
                                after,
                            },
                            cx,
                        );
                    }
                },
            ))
            .on_drop::<DragTab>(
                cx.listener(|this, d: &DragTab, _, cx| this.drop_tab(&d.id.clone(), cx)),
            )
            .into_any_element()
    }

    /// One Tab row (ungrouped or inside a Group).
    fn tab_row(
        &self,
        t: &SidebarTab,
        p: &Palette,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let live = self.is_live_target(&t.connection, t.target_id.as_deref());
        let dim = t.placeholder || !live;
        let active = live && self.is_active(&t.connection, t.target_id.as_deref());
        let alert = self.has_dialog_badge(&t.connection, t.target_id.as_deref());
        let agent = self.agent_marker(&t.connection, t.target_id.as_deref());
        let style = sm::style_of(&self.conn_styles, &t.connection);
        let ccol = style.map(|s| s.color(dark)).unwrap_or(p.idle);
        let id = t.id.clone();
        let title = display_title(t);
        let rail = self.rail;
        let group = SharedString::from(format!("row-{id}"));
        let hint = match &self.drop_hint {
            Some(DropTarget::Tab { id: h, after }) if *h == t.id && cx.has_active_drag() => {
                Some(*after)
            }
            _ => None,
        };
        let (id_click, id_menu, id_close, id_drag, id_move) =
            (id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
        div()
            .id(SharedString::from(format!("tab-{id}")))
            .group(group.clone())
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .h(px(28.))
            .rounded(px(6.))
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .text_color(hsla(p.ink))
            .when(rail, |d| d.justify_center().gap(px(3.)).px(px(0.)))
            .when(active, |d| {
                d.bg(hsla(p.sel))
                    .border_color(hsla((ccol & 0xffffff00) | 0x8c))
                    .shadow_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
            })
            .when(!active, |d| d.hover(|s| s.bg(hsla(p.hover))))
            .when(dim, |d| d.opacity(0.5))
            .child(
                div()
                    .flex_none()
                    .size(px(16.))
                    .rounded(px(4.))
                    .bg(hsla(p.side2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(hsla(p.ink2))
                    .child(SharedString::from(sm::fav_text(&t.title, &t.url))),
            )
            .when(!rail, |d| {
                d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .when(dim, |d| d.italic())
                        .child(SharedString::from(title.clone())),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("close-{id_close}")))
                        .flex_none()
                        .size(px(16.))
                        .rounded(px(4.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(hsla(p.ink3))
                        .opacity(0.)
                        .group_hover(group.clone(), |s| s.opacity(1.))
                        .hover(|s| s.bg(hsla(p.hover)))
                        .child("×")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_tab(&id_close.clone(), cx);
                        })),
                )
            })
            .children(agent.shown().then(|| {
                if rail {
                    agent_mark(agent, 9., p)
                        .absolute()
                        .top(px(1.))
                        .right(px(2.))
                } else {
                    agent_mark(agent, 12., p)
                }
            }))
            .children(alert.then(|| dialog_badge(p)))
            .children(style.map(|s| conn_dot(s.shape, hsla(ccol), 9.)))
            .when_some(hint, |d, after| d.child(drop_bar(after, hsla(p.accent))))
            .on_click(
                cx.listener(move |this, _, window, cx| this.activate_tab(&id_click, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.menu = Some(Menu {
                        kind: MenuKind::Tab(id_menu.clone()),
                        pos: ev.position,
                    });
                    cx.notify();
                }),
            )
            .on_drag(
                DragTab {
                    id: id_drag,
                    label: title,
                },
                {
                    let pal = *p;
                    move |d, _, _, cx| {
                        cx.new(|_| DragGhost {
                            label: d.label.clone(),
                            p: pal,
                        })
                    }
                },
            )
            .on_drag_move::<DragTab>(cx.listener(
                move |this, ev: &DragMoveEvent<DragTab>, _, cx| {
                    if ev.bounds.contains(&ev.event.position) {
                        let after = sm::is_after(
                            f32::from(ev.event.position.y - ev.bounds.origin.y),
                            f32::from(ev.bounds.size.height),
                        );
                        this.set_drop_hint(
                            DropTarget::Tab {
                                id: id_move.clone(),
                                after,
                            },
                            cx,
                        );
                    }
                },
            ))
            .on_drop::<DragTab>(
                cx.listener(|this, d: &DragTab, _, cx| this.drop_tab(&d.id.clone(), cx)),
            )
            .into_any_element()
    }

    fn group_view(
        &self,
        g: &SidebarGroup,
        all_groups: &[String],
        p: &Palette,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let gc = hsla(theme::group_color(g.color.as_deref(), dark));
        let rail = self.rail;
        let gid = g.id.clone();
        let hot = self.drop_hint == Some(DropTarget::Group(g.id.clone())) && cx.has_active_drag();
        let renaming = self
            .rename
            .as_ref()
            .filter(|r| r.group == g.id)
            .map(|r| (r.edit.clone(), r.focus.clone()));

        let name: AnyElement = match renaming {
            Some((edit, focus)) => div()
                .id("rename-field")
                .track_focus(&focus)
                .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                    this.rename_key(ev, window, cx)
                }))
                .min_w(px(60.))
                .px(px(6.))
                .rounded(px(5.))
                .border_1()
                .border_color(gc)
                .bg(hsla(p.win))
                .text_size(px(12.))
                .text_color(hsla(p.ink))
                .child(SharedString::from(format!("{}|", edit.buf)))
                .into_any_element(),
            None => div()
                .px(px(6.))
                .rounded(px(5.))
                .border_1()
                .border_color(gc)
                .text_size(px(12.))
                .line_height(px(18.))
                .child(SharedString::from(g.name.clone()))
                .into_any_element(),
        };

        let (id_toggle, id_menu, id_drag, id_move, id_drop_group) = (
            gid.clone(),
            gid.clone(),
            gid.clone(),
            gid.clone(),
            gid.clone(),
        );
        let order = all_groups.to_vec();
        let my_group_id = gid.clone();
        let head = div()
            .id(SharedString::from(format!("ghead-{gid}")))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .py(px(3.))
            .rounded(px(6.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(gc)
            .cursor_pointer()
            .when(hot, |d| d.bg(hsla(p.hover)).border_1().border_color(gc))
            .hover(|s| s.bg(hsla(p.hover)))
            .when(rail, |d| d.justify_center())
            .child(div().w(px(10.)).text_size(px(10.)).child(if g.collapsed {
                "▸"
            } else {
                "▾"
            }))
            .when(!rail, |d| {
                d.child(name).child(
                    div()
                        .ml_auto()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(hsla(p.ink3))
                        .children(if g.collapsed {
                            g.connections
                                .iter()
                                .filter_map(|c| sm::style_of(&self.conn_styles, c))
                                .map(|s| {
                                    conn_dot(s.shape, hsla(s.color(dark)), 7.).into_any_element()
                                })
                                .collect::<Vec<_>>()
                        } else {
                            Vec::new()
                        })
                        .child(SharedString::from(format!(" {}", g.tab_count))),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                let id = id_toggle.clone();
                this.edit_state(cx, |s| {
                    if let Some(c) = s.group(&id).map(|g| g.collapsed) {
                        let _ = s.set_group_collapsed(&id, !c);
                    }
                });
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.menu = Some(Menu {
                        kind: MenuKind::Group(id_menu.clone()),
                        pos: ev.position,
                    });
                    cx.notify();
                }),
            )
            .on_drag(
                DragGroup {
                    id: id_drag,
                    label: g.name.clone(),
                },
                {
                    let pal = *p;
                    move |d, _, _, cx| {
                        cx.new(|_| DragGhost {
                            label: d.label.clone(),
                            p: pal,
                        })
                    }
                },
            )
            .on_drag_move::<DragTab>(cx.listener(
                move |this, ev: &DragMoveEvent<DragTab>, _, cx| {
                    if ev.bounds.contains(&ev.event.position) {
                        this.set_drop_hint(DropTarget::Group(id_move.clone()), cx);
                    }
                },
            ))
            .on_drop::<DragTab>(cx.listener(move |this, d: &DragTab, _, cx| {
                let _ = &id_drop_group;
                this.drop_tab(&d.id.clone(), cx)
            }))
            .on_drop::<DragGroup>(cx.listener(move |this, d: &DragGroup, _, cx| {
                let Some(to) = order.iter().position(|g| *g == my_group_id) else {
                    return;
                };
                let from_id = d.id.clone();
                this.edit_state(cx, |s| {
                    // Group order is State::groups order; indices there match
                    // the layout order of Groups.
                    let _ = s.move_group(&from_id, to);
                });
            }));

        let mut body = div()
            .flex()
            .flex_col()
            .ml(px(if rail { 4. } else { 12. }))
            .pl(px(if rail { 3. } else { 6. }))
            .border_l_2()
            .border_color(gc);
        for t in &g.tabs {
            body = body.child(self.tab_row(t, p, dark, cx));
        }
        div()
            .flex()
            .flex_col()
            .my(px(6.))
            .child(head)
            .when(!g.collapsed && !g.tabs.is_empty(), |d| d.child(body))
            .into_any_element()
    }

    /// The action a click on a chip performs (none for a healthy Connection).
    fn chip_primary(status: ChipStatus) -> Option<ChipAction> {
        match status {
            ChipStatus::Connected => None,
            ChipStatus::Reconnecting | ChipStatus::Failed => Some(ChipAction::RetryNow),
            ChipStatus::Stopped => Some(ChipAction::Start),
            ChipStatus::NotConnected => Some(ChipAction::Connect),
        }
    }

    /// One chip per Connection: badge, status dot, hover popover, click
    /// connects (or starts, retries). Beyond [`multi::MAX_CHIPS`] the rest go
    /// into an overflow menu behind a "..." chip.
    fn chips_row(&self, p: &Palette, dark: bool, cx: &mut Context<Self>) -> AnyElement {
        let rail = self.rail;
        let (shown, hidden) = multi::overflow_split(self.conn_styles.len());
        let mut row = div()
            .flex_none()
            .flex()
            .gap(px(8.))
            .p(px(10.))
            .border_t_1()
            .border_color(hsla(p.line))
            .when(rail, |d| d.flex_col().items_center().px(px(0.)))
            .when(!rail, |d| d.flex_row().flex_wrap().items_center());
        for (i, s) in self.conn_styles.iter().enumerate().take(shown) {
            let name = SharedString::from(format!("chip-{i}"));
            let status = self.chip_status_of(i);
            let off = !matches!(status, ChipStatus::Connected | ChipStatus::Reconnecting);
            let primary = Self::chip_primary(status);
            let tooltip_active = self.active.as_ref().is_some_and(|a| a.0 == i);
            let attached = self.agent_attached(i);
            let acting = attached
                && self.conns[i].agent.as_ref().is_some_and(|a| {
                    let now = std::time::Instant::now();
                    a.state.targets.keys().any(|t| {
                        agent_model::marker(Some(&a.state), Some(t), now) == Marker::Acting
                    })
                });
            row =
                row.child(
                    div()
                        .id(name.clone())
                        .group(name.clone())
                        .relative()
                        .p(px(2.))
                        .rounded(px(9.))
                        .cursor_pointer()
                        .hover(|st| st.bg(hsla(p.hover)))
                        .when(tooltip_active, |d| d.bg(hsla(p.field)))
                        .child(
                            div()
                                .when(off, |d| d.opacity(0.55))
                                .child(badge(s, dark, 26.)),
                        )
                        .child(status_dot(status, p))
                        .children(attached.then(|| {
                            div()
                                .absolute()
                                .left(px(-3.))
                                .top(px(-5.))
                                .size(px(14.))
                                .rounded_full()
                                .bg(hsla(p.side))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(agent_mark(
                                    if acting {
                                        Marker::Acting
                                    } else {
                                        Marker::Touched
                                    },
                                    12.,
                                    p,
                                ))
                        }))
                        .child(
                            div()
                                .id(SharedString::from(format!("chip-plus-{i}")))
                                .absolute()
                                .top(px(-6.))
                                .right(px(-6.))
                                .size(px(16.))
                                .rounded_full()
                                .bg(hsla(p.ink))
                                .text_color(hsla(p.win))
                                .flex()
                                .items_center()
                                .justify_center()
                                .opacity(0.)
                                .group_hover(name.clone(), |st| st.opacity(1.))
                                .child("+")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.new_tab_in(i, cx)
                                })),
                        )
                        .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                            this.chip_hover_changed(i, *h, cx)
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(a) = primary {
                                this.chip_action(i, a, cx);
                            }
                        })),
                );
        }
        if hidden > 0 {
            row = row.child(
                div()
                    .id("chip-overflow")
                    .size(px(30.))
                    .rounded(px(9.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(hsla(p.field))
                    .text_color(hsla(p.ink2))
                    .hover(|st| st.bg(hsla(p.hover)))
                    .child(SharedString::from(format!("+{hidden}")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.overflow_open = !this.overflow_open;
                        this.chip_open = None;
                        cx.notify();
                    })),
            );
        }
        row.into_any_element()
    }

    /// The overflow menu: one row per Connection past the chip row.
    fn overflow_menu(&self, p: &Palette, dark: bool, cx: &mut Context<Self>) -> AnyElement {
        let (shown, _) = multi::overflow_split(self.conn_styles.len());
        let mut panel = div()
            .id("chip-overflow-menu")
            .absolute()
            .left(px(if self.rail { 66. } else { 10. }))
            .bottom(px(54.))
            .w(px(248.))
            .py(px(4.))
            .rounded(px(10.))
            .bg(hsla(p.win))
            .border_1()
            .border_color(hsla(p.line))
            .shadow_lg()
            .occlude()
            .text_color(hsla(p.ink));
        for (i, s) in self.conn_styles.iter().enumerate().skip(shown) {
            let status = self.chip_status_of(i);
            let live_tabs = self.live_tab_count(&s.name);
            let pop = sm::popover(
                status,
                &self.link_of(i),
                live_tabs,
                self.tab_count(&s.name) - live_tabs,
                self.detail_of(i).as_deref(),
            );
            let primary = Self::chip_primary(status);
            panel = panel.child(
                div()
                    .id(SharedString::from(format!("overflow-{i}")))
                    .mx(px(4.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .hover(|st| st.bg(hsla(p.hover)))
                    .child(
                        div()
                            .relative()
                            .child(badge(s, dark, 20.))
                            .child(status_dot(status, p)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(SharedString::from(s.name.clone()))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(hsla(p.ink3))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(SharedString::from(pop.text)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(hsla(p.accent))
                            .child(primary.map(ChipAction::label).unwrap_or("Disconnect")),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.chip_action(i, primary.unwrap_or(ChipAction::Disconnect), cx)
                    })),
            );
        }
        deferred(panel).with_priority(1).into_any_element()
    }

    fn tab_count(&self, conn: &str) -> usize {
        self.state
            .tabs
            .iter()
            .filter(|t| t.connection == conn)
            .count()
    }

    fn live_tab_count(&self, conn: &str) -> usize {
        self.state
            .tabs
            .iter()
            .filter(|t| t.connection == conn && self.is_live_target(conn, t.target_id.as_deref()))
            .count()
    }

    /// Popover and context menu, drawn above everything else.
    pub(super) fn sidebar_overlays(
        &mut self,
        _window: &mut Window,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let dark = p.dark;
        if let Some(i) = self.chip_open {
            if let Some(s) = self.conn_styles.get(i).cloned() {
                out.push(self.popover_view(i, &s, p, dark, cx));
            }
        }
        if self.overflow_open {
            out.push(self.overflow_menu(p, dark, cx));
        }
        if let Some(menu) = &self.menu {
            let items = sm::menu_items(&menu.kind, &self.state);
            if items.is_empty() {
                self.menu = None;
            } else {
                let pos = menu.pos;
                let mut panel = div()
                    .id("context-menu")
                    .occlude()
                    .w(px(220.))
                    .py(px(4.))
                    .rounded(px(8.))
                    .bg(hsla(p.win))
                    .border_1()
                    .border_color(hsla(p.line))
                    .shadow_lg()
                    .text_size(px(13.))
                    .text_color(hsla(p.ink));
                for (n, it) in items.into_iter().enumerate() {
                    let action = it.action.clone();
                    let danger = it.danger;
                    panel = panel.child(
                        div()
                            .id(SharedString::from(format!("menu-{n}")))
                            .when(it.separator, |d| {
                                d.mt(px(4.))
                                    .border_t_1()
                                    .border_color(hsla(p.line))
                                    .pt(px(4.))
                            })
                            .child(
                                div()
                                    .mx(px(4.))
                                    .px(px(8.))
                                    .h(px(26.))
                                    .rounded(px(5.))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .cursor_pointer()
                                    .text_color(hsla(if danger { p.bad } else { p.ink }))
                                    .hover(|s| s.bg(hsla(p.hover)))
                                    .children(it.swatch.map(|c| {
                                        div()
                                            .size(px(10.))
                                            .rounded(px(3.))
                                            .bg(hsla(theme::group_color(Some(c.name()), dark)))
                                    }))
                                    .child(SharedString::from(it.label)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.menu_action(action.clone(), window, cx)
                            })),
                    );
                }
                let close = |this: &mut Root, cx: &mut Context<Root>| {
                    this.menu = None;
                    cx.notify();
                };
                out.push(
                    deferred(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .occlude()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| close(this, cx)),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| close(this, cx)),
                            )
                            .child(
                                anchored()
                                    .position(pos)
                                    .snap_to_window_with_margin(px(8.))
                                    .child(panel),
                            ),
                    )
                    .with_priority(2)
                    .into_any_element(),
                );
            }
        }
        out
    }

    fn popover_view(
        &self,
        i: usize,
        s: &ConnStyle,
        p: &Palette,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status = self.chip_status_of(i);
        let link = self.link_of(i);
        let live_tabs = self.live_tab_count(&s.name);
        let placeholders = self.tab_count(&s.name) - live_tabs;
        let detail = self.detail_of(i);
        let pop = sm::popover(status, &link, live_tabs, placeholders, detail.as_deref());
        let mut buttons = div().flex().gap(px(6.));
        for (n, a) in pop.actions.iter().copied().enumerate() {
            buttons = buttons.child(
                div()
                    .id(SharedString::from(format!("pop-btn-{n}")))
                    .px(px(12.))
                    .py(px(3.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(a.primary(), |d| {
                        d.bg(hsla(p.accent)).text_color(gpui::white())
                    })
                    .when(!a.primary(), |d| {
                        d.bg(hsla(p.hover)).text_color(hsla(p.ink))
                    })
                    .hover(|st| st.opacity(0.85))
                    .child(a.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.chip_action(i, a, cx))),
            );
        }
        let left = if self.rail { 66. } else { 10. };
        let agent_line = self.agent_line(i);
        let can_agent = !matches!(self.conns[i].phase, super::Phase::Problem(_));
        let copied = self.conns[i].agent_copied;
        let small_btn = |id: &'static str, label: &'static str, p: &Palette| {
            div()
                .id(id)
                .px(px(10.))
                .py(px(2.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(12.))
                .bg(hsla(p.hover))
                .text_color(hsla(p.ink))
                .hover(|st| st.opacity(0.85))
                .child(label)
        };
        let agent_section = match agent_line {
            AgentLine::Off if !can_agent => None,
            AgentLine::Off => Some(div().mt(px(8.)).child(
                small_btn("agent-enable", "Enable Agent endpoint", p).on_click(
                    cx.listener(move |this, _, _, cx| this.set_agent_enabled(i, true, cx)),
                ),
            )),
            other => {
                let mut sec = div()
                    .mt(px(8.))
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(hsla(p.line))
                    .flex()
                    .flex_col()
                    .gap(px(4.));
                let violet = hsla(theme::agent_color(dark));
                let head = |t: String| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_size(px(12.))
                        .child(div().text_color(violet).child("✦"))
                        .child(SharedString::from(t))
                };
                let mut btns = div().flex().gap(px(6.));
                match other {
                    AgentLine::On { endpoint, clients } => {
                        sec = sec
                            .child(head(format!(
                                "Agent endpoint · {clients} client{} attached",
                                if clients == 1 { "" } else { "s" }
                            )))
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .font_family("Menlo")
                                    .text_color(hsla(p.ink))
                                    .whitespace_nowrap()
                                    .child(SharedString::from(endpoint.clone())),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(hsla(p.ink3))
                                    .child(SharedString::from(agent_model::usage_hint(&endpoint))),
                            );
                        btns = btns.child(
                            small_btn(
                                "agent-copy",
                                if copied { "Copied" } else { "Copy endpoint" },
                                p,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.copy_endpoint(i, cx))),
                        );
                    }
                    AgentLine::Error(e) => {
                        sec = sec.child(
                            div()
                                .text_size(px(11.))
                                .text_color(hsla(p.warn))
                                .child(SharedString::from(e)),
                        );
                    }
                    _ => {
                        sec = sec.child(head("Agent endpoint · starting when connected".into()));
                    }
                }
                btns = btns.child(small_btn("agent-disable", "Disable", p).on_click(
                    cx.listener(move |this, _, _, cx| this.set_agent_enabled(i, false, cx)),
                ));
                Some(sec.child(btns))
            }
        };
        let wide = self.conns[i].agent.is_some();
        deferred(
            div()
                .id("chip-popover")
                .absolute()
                .left(px(left))
                .bottom(px(54.))
                .w(px(if wide { 340. } else { 248. }))
                .p(px(10.))
                .rounded(px(10.))
                .bg(hsla(p.win))
                .border_1()
                .border_color(hsla(p.line))
                .shadow_lg()
                .occlude()
                .text_color(hsla(p.ink))
                .on_hover(cx.listener(|this, h: &bool, _, cx| this.popover_hover_changed(*h, cx)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(badge(s, dark, 16.))
                        .child(SharedString::from(s.name.clone())),
                )
                .child(
                    div()
                        .my(px(4.))
                        .mb(px(8.))
                        .text_size(px(12.))
                        .text_color(hsla(p.ink2))
                        .child(SharedString::from(pop.text)),
                )
                .child(buttons)
                .children(agent_section),
        )
        .with_priority(1)
        .into_any_element()
    }
}

/// A filled square badge with the Connection's initial (colour and letter).
pub(super) fn badge(s: &ConnStyle, dark: bool, size: f32) -> AnyElement {
    let c = s.color(dark);
    div()
        .flex_none()
        .size(px(size))
        .rounded(px(if size > 20. { 7. } else { 4. }))
        .bg(hsla(c))
        .text_color(hsla(theme::on_color(c)))
        .flex()
        .items_center()
        .justify_center()
        .font_weight(gpui::FontWeight::BOLD)
        .text_size(px(if size > 20. { 13. } else { 10. }))
        .child(SharedString::from(s.initial.to_string()))
        .into_any_element()
}

/// Drop indicator above or below a row.
fn drop_bar(after: bool, color: Hsla) -> AnyElement {
    let bar = div()
        .absolute()
        .left(px(0.))
        .right(px(0.))
        .h(px(2.))
        .rounded_full()
        .bg(color);
    if after {
        bar.bottom(px(-2.)).into_any_element()
    } else {
        bar.top(px(-2.)).into_any_element()
    }
}

/// Drop indicator left or right of a pinned cell.
fn drop_bar_vertical(after: bool, color: Hsla) -> AnyElement {
    let bar = div()
        .absolute()
        .top(px(0.))
        .bottom(px(0.))
        .w(px(2.))
        .rounded_full()
        .bg(color);
    if after {
        bar.right(px(-4.)).into_any_element()
    } else {
        bar.left(px(-4.)).into_any_element()
    }
}

#[allow(dead_code)]
fn _px(_: Pixels) {}

/// A small amber "!" for a Tab that has a pending JS dialog.
fn dialog_badge(p: &Palette) -> gpui::Div {
    div()
        .flex_none()
        .size(px(14.))
        .rounded_full()
        .bg(hsla(p.warn))
        .text_color(hsla(0x1a1200ff))
        .flex()
        .items_center()
        .justify_center()
        .font_weight(gpui::FontWeight::BOLD)
        .text_size(px(10.))
        .child("!")
}

/// The Agent marker: a violet ✦, pulsing while the Agent is acting now and
/// static when it touched the Tab earlier (one marker, per the mock).
fn agent_mark(m: Marker, size: f32, p: &Palette) -> gpui::Div {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let alpha = agent_model::opacity(m, std::time::Duration::from_millis(ms));
    div()
        .flex_none()
        .text_size(px(size))
        .line_height(px(size))
        .text_color(hsla(theme::agent_color(p.dark)))
        .opacity(alpha)
        .child("✦")
        // Keeps frames coming while it pulses (the same trick as the spinner).
        .when(m == Marker::Acting, |d| {
            d.child(
                canvas(
                    |_, _, _| (),
                    |_, _, window, _| window.request_animation_frame(),
                )
                .absolute()
                .size(px(1.)),
            )
        })
}
