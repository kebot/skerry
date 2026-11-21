use crate::components::list_row;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    scroll::ScrollableElement as _,
    Sizable as _, StyledExt as _, Theme,
};
use gpui_kit::{div, prelude::*, px, App, ClickEvent, Div, FontWeight, SharedString, Window};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppItem {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionItem {
    pub id: String,
    pub app_name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibraryProps {
    pub app_name: String,
    pub server: String,
    pub apps: Vec<AppItem>,
    pub sessions: Vec<SessionItem>,
    pub status: Option<String>,
    pub busy: Option<String>,
}

pub type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
pub type ItemHandler = Rc<dyn Fn(String, &mut Window, &mut App)>;

pub struct LibraryHandlers {
    pub on_refresh: ClickHandler,
    pub on_forget: ClickHandler,
    pub on_launch: ItemHandler,
    pub on_resume: ItemHandler,
}

pub fn render(theme: &Theme, props: &LibraryProps, handlers: LibraryHandlers) -> Div {
    let enabled = props.busy.is_none();
    let LibraryHandlers {
        on_refresh,
        on_forget,
        on_launch,
        on_resume,
    } = handlers;

    let sidebar = div()
        .w(px(222.))
        .flex_none()
        .h_full()
        .p_5()
        .v_flex()
        .gap_3()
        .bg(theme.sidebar)
        .border_r_1()
        .border_color(theme.border)
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child(props.app_name.clone()),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .truncate()
                .child(props.server.clone()),
        )
        .child(div().flex_1())
        .when(enabled, |view| {
            view.child(
                Button::new("forget")
                    .outline()
                    .label("Remove Connection")
                    .on_click(move |event, window, cx| on_forget(event, window, cx)),
            )
        });

    let content = div()
        .id("library")
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_y_scrollbar()
        .p_6()
        .v_flex()
        .gap_3()
        .child(
            div()
                .h_flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Applications"),
                )
                .when(enabled, |view| {
                    view.child(
                        Button::new("refresh")
                            .outline()
                            .small()
                            .label("Refresh")
                            .on_click(move |event, window, cx| on_refresh(event, window, cx)),
                    )
                }),
        )
        .when_some(props.busy.clone(), |view, busy| {
            view.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(busy),
            )
        })
        .when_some(props.status.clone(), |view, status| {
            view.child(div().text_sm().child(status))
        })
        .when(props.apps.is_empty(), |view| {
            view.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("No applications are available for this account."),
            )
        })
        .children(props.apps.iter().map(|app| {
            let app_id = app.id.clone();
            let on_launch = Rc::clone(&on_launch);
            list_row(theme, app.name.clone()).when(enabled, |view| {
                view.child(
                    Button::new(SharedString::from(format!("launch-{app_id}")))
                        .primary()
                        .small()
                        .label("Launch")
                        .on_click(move |_, window, cx| on_launch(app_id.clone(), window, cx)),
                )
            })
        }))
        .child(
            div()
                .mt_4()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Running Sessions"),
        )
        .when(props.sessions.is_empty(), |view| {
            view.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("No running sessions."),
            )
        })
        .children(props.sessions.iter().map(|session| {
            let session_id = session.id.clone();
            let on_resume = Rc::clone(&on_resume);
            list_row(theme, session.app_name.clone()).when(enabled, |view| {
                view.child(
                    Button::new(SharedString::from(format!("resume-{session_id}")))
                        .outline()
                        .small()
                        .label("Resume")
                        .on_click(move |_, window, cx| on_resume(session_id.clone(), window, cx)),
                )
            })
        }));

    div()
        .size_full()
        .bg(theme.background)
        .text_color(theme.foreground)
        .flex()
        .child(sidebar)
        .child(content)
}
