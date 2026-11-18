use crate::api::{Api, Credential, Library, RemoteApp, RemoteSession};
use crate::transport::SessionEvent;
use crate::view::NativeSession;
use gpui::{
    actions, div, prelude::*, px, rgb, size, App, Application, Bounds, Context, Div, KeyBinding,
    Menu, MenuItem, SharedString, Stateful, TitlebarOptions, Window, WindowAppearance,
    WindowBounds, WindowOptions,
};
use tokio::runtime::Handle;

const APP_NAME: &str = "SealSkin Native";
const TOOLBAR_HEIGHT: f32 = 54.;

pub fn run() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("create SealSkin runtime");
    let handle = runtime.handle().clone();
    Application::new().run(move |cx: &mut App| {
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(vec![Menu {
            name: APP_NAME.into(),
            items: vec![MenuItem::action("Quit SealSkin Native", Quit)],
        }]);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1080.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(640.), px(420.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(APP_NAME.into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| cx.new(|cx| Launcher::new(handle.clone(), window, cx)),
        )
        .expect("open SealSkin window");
        cx.activate(true);
    });
}

actions!(sealskin, [Quit]);

struct Launcher {
    rt: Handle,
    api: Option<Api>,
    library: Option<Library>,
    current: Option<(RemoteSession, NativeSession)>,
    stream_events: Option<gpui::Task<()>>,
    audio: Option<bool>,
    busy: Option<String>,
    status: Option<String>,
}

impl Launcher {
    fn new(rt: Handle, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut launcher = Self {
            rt,
            api: None,
            library: None,
            current: None,
            stream_events: None,
            audio: None,
            busy: None,
            status: None,
        };
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        match Credential::startup() {
            Ok(Some(credential)) => launcher.connect(credential, false, cx),
            Ok(None) => {}
            Err(error) => launcher.status = Some(error),
        }
        launcher
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let result = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .ok_or_else(|| "Copy a SealSkin application key first.".to_string())
            .and_then(|text| Credential::parse(&text));
        match result {
            Ok(credential) => self.connect(credential, true, cx),
            Err(error) => self.status = Some(error),
        }
        cx.notify();
    }

    fn connect(&mut self, credential: Credential, save: bool, cx: &mut Context<Self>) {
        match Api::new(&credential) {
            Ok(api) => self.load(api, save.then_some(credential), true, cx),
            Err(error) => self.status = Some(error),
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(api) = self.api.clone() {
            self.load(api, None, false, cx);
        }
    }

    fn load(
        &mut self,
        api: Api,
        save: Option<Credential>,
        connecting: bool,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(
            if connecting {
                "Connecting…"
            } else {
                "Refreshing…"
            }
            .into(),
        );
        self.status = None;
        let client = api.clone();
        let pending = self.rt.spawn(async move { client.library().await });
        cx.spawn(async move |this, cx| {
            let result = pending
                .await
                .unwrap_or_else(|_| Err("The connection was interrupted.".into()));
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(library) => {
                        this.api = Some(api);
                        this.library = Some(library);
                        this.status = save.and_then(|credential| credential.save().err());
                    }
                    Err(error) => this.status = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn forget(&mut self, cx: &mut Context<Self>) {
        self.status = Credential::forget().err();
        self.api = None;
        self.library = None;
        cx.notify();
    }

    fn launch(&mut self, app: RemoteApp, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone().filter(|_| self.busy.is_none()) else {
            return;
        };
        self.status = None;
        self.busy = Some(format!("Launching {}…", app.name));
        let pending = self.rt.spawn(async move { api.launch(app).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = pending
                .await
                .unwrap_or_else(|_| Err("The launch was interrupted.".into()));
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(session) => {
                        if let Some(library) = &mut this.library {
                            library.sessions.insert(0, session.clone());
                        }
                        this.open(session, window, cx);
                    }
                    Err(error) => this.status = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open(&mut self, session: RemoteSession, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = &self.api else {
            return;
        };
        let (events, mut incoming) = tokio::sync::mpsc::unbounded_channel();
        let stream = api
            .session_url(&session.session_url)
            .map(|url| NativeSession::open(url, window, cx, &self.rt, events));
        match stream {
            Ok(stream) => {
                self.audio = None;
                self.status = None;
                window.set_window_title(&format!("{} — {APP_NAME}", session.app_name));
                self.current = Some((session, stream));
                self.stream_events = Some(cx.spawn(async move |this, cx| {
                    while let Some(event) = incoming.recv().await {
                        if this
                            .update(cx, |this, cx| {
                                match event {
                                    SessionEvent::Audio(enabled) => this.audio = Some(enabled),
                                    SessionEvent::Error(error) => {
                                        this.audio = None;
                                        this.status = Some(error);
                                    }
                                }
                                cx.notify();
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
            }
            Err(error) => self.status = Some(error),
        }
        cx.notify();
    }

    fn reconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((session, _)) = &self.current {
            let session = session.clone();
            self.close_stream(window, cx);
            self.open(session, window, cx);
        }
    }

    fn close_stream(&mut self, window: &mut Window, cx: &mut App) {
        self.stream_events = None;
        if let Some((_, stream)) = self.current.take() {
            stream.release_focus(window, cx);
        }
        self.audio = None;
        window.set_window_title(APP_NAME);
    }
}

#[derive(Clone, Copy)]
struct Colors {
    background: u32,
    sidebar: u32,
    panel: u32,
    ink: u32,
    muted: u32,
    edge: u32,
    accent: u32,
}

impl Colors {
    fn for_window(window: &Window) -> Self {
        match window.appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self {
                background: 0x191c22,
                sidebar: 0x14171d,
                panel: 0x22262f,
                ink: 0xf1f3f6,
                muted: 0xa3acbb,
                edge: 0x343b48,
                accent: 0x346ce0,
            },
            _ => Self {
                background: 0xf5f6f9,
                sidebar: 0xedeff4,
                panel: 0xffffff,
                ink: 0x182238,
                muted: 0x56647a,
                edge: 0xdde2ec,
                accent: 0x2658cb,
            },
        }
    }

    fn button(
        self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        primary: bool,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex_none()
            .px_3()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(rgb(if primary { self.accent } else { self.edge }))
            .bg(rgb(if primary { self.accent } else { self.panel }))
            .text_color(rgb(if primary { 0xffffff } else { self.ink }))
            .text_sm()
            .cursor_pointer()
            .hover(move |style| style.border_color(rgb(self.muted)))
            .child(label.into())
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Colors::for_window(window);
        let root = div()
            .size_full()
            .bg(rgb(colors.background))
            .text_color(rgb(colors.ink));
        if let Some((session, stream)) = &self.current {
            let view = stream.view();
            let mut toolbar = div()
                .h(px(TOOLBAR_HEIGHT))
                .flex_none()
                .w_full()
                .px_4()
                .flex()
                .items_center()
                .gap_2()
                .bg(rgb(colors.sidebar))
                .border_b_1()
                .border_color(rgb(colors.edge))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(session.app_name.clone()),
                );
            if let Some(enabled) = self.audio {
                toolbar = toolbar.child(
                    colors
                        .button("audio", if enabled { "Mute" } else { "Unmute" }, false)
                        .on_click(cx.listener(move |this, _, _, _| {
                            if let Some((_, stream)) = &this.current {
                                stream.set_audio(!enabled);
                            }
                        })),
                );
            }
            return root
                .bg(rgb(0x000000))
                .flex()
                .flex_col()
                .child(
                    toolbar
                        .child(
                            colors
                                .button("fullscreen", "Full Screen", false)
                                .on_click(|_, window, _| window.toggle_fullscreen()),
                        )
                        .child(colors.button("reconnect", "Reconnect", false).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.reconnect(window, cx);
                            }),
                        ))
                        .child(colors.button("disconnect", "Disconnect", false).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.close_stream(window, cx);
                                this.refresh(cx);
                            }),
                        )),
                )
                .when_some(self.status.clone(), |root, status| {
                    root.child(div().p_3().bg(rgb(colors.panel)).child(status))
                })
                .child(div().flex_1().min_h_0().child(view));
        }
        let (Some(api), Some(library)) = (&self.api, &self.library) else {
            return root.flex().items_center().justify_center().child(
                div().w(px(416.)).p_6().rounded_xl().border_1().border_color(rgb(colors.edge)).bg(rgb(colors.panel)).flex().flex_col().gap_4()
                    .child(div().text_2xl().font_weight(gpui::FontWeight::SEMIBOLD).child("Connect to SealSkin"))
                    .child(div().text_sm().text_color(rgb(colors.muted)).child("Copy an application key from Settings → Application Keys on your SealSkin server. SealSkin stores it in macOS Keychain after connecting."))
                    .when_some(self.status.clone(), |view, status| view.child(status))
                    .when_some(self.busy.clone(), |view, busy| view.child(busy))
                    .when(self.busy.is_none(), |view| view.child(colors.button("paste", "Paste Application Key", true)
                        .on_click(cx.listener(|this, _, _, cx| this.paste(cx))))));
        };
        let sidebar = div()
            .w(px(222.))
            .flex_none()
            .h_full()
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(colors.sidebar))
            .border_r_1()
            .border_color(rgb(colors.edge))
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(APP_NAME),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(colors.muted))
                    .truncate()
                    .child(api.server().to_string()),
            )
            .child(div().flex_1())
            .when(self.busy.is_none(), |view| {
                view.child(
                    colors
                        .button("forget", "Remove Connection", false)
                        .on_click(cx.listener(|this, _, _, cx| this.forget(cx))),
                )
            });
        let enabled = self.busy.is_none();
        let content = div()
            .id("library")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p_6()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Applications"),
                    )
                    .when(enabled, |view| {
                        view.child(
                            colors
                                .button("refresh", "Refresh", false)
                                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                        )
                    }),
            )
            .when_some(self.busy.clone(), |view, busy| {
                view.child(div().text_sm().text_color(rgb(colors.muted)).child(busy))
            })
            .when_some(self.status.clone(), |view, status| {
                view.child(div().text_sm().child(status))
            })
            .when(library.apps.is_empty(), |view| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(rgb(colors.muted))
                        .child("No applications are available for this account."),
                )
            })
            .children(library.apps.iter().map(|app| {
                let app = app.clone();
                row(colors, app.name.clone()).when(enabled, |view| {
                    view.child(
                        colors
                            .button(
                                SharedString::from(format!("launch-{}", app.id)),
                                "Launch",
                                true,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.launch(app.clone(), window, cx)
                            })),
                    )
                })
            }))
            .child(
                div()
                    .mt_4()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Running Sessions"),
            )
            .when(library.sessions.is_empty(), |view| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(rgb(colors.muted))
                        .child("No running sessions."),
                )
            })
            .children(library.sessions.iter().map(|session| {
                let session = session.clone();
                row(colors, session.app_name.clone()).when(enabled, |view| {
                    view.child(
                        colors
                            .button(
                                SharedString::from(format!("resume-{}", session.session_id)),
                                "Resume",
                                false,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open(session.clone(), window, cx)
                            })),
                    )
                })
            }));
        root.flex().child(sidebar).child(content)
    }
}

fn row(colors: Colors, name: String) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .px_4()
        .py_2()
        .min_h(px(48.))
        .rounded_lg()
        .border_1()
        .border_color(rgb(colors.edge))
        .bg(rgb(colors.panel))
        .child(div().min_w_0().truncate().child(name))
}
