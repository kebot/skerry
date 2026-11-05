//! Application entry: one window with the [`Root`] view.

use gpui::{
    px, size, App, AppContext, Application, Bounds, Focusable, KeyBinding, TitlebarOptions,
    WindowBounds, WindowOptions,
};
use tokio::runtime::{Handle, Runtime};

use crate::startup::Startup;
use crate::state::{Quit, ToggleSidebar};
use crate::views::Root;

/// A multi-thread tokio runtime for all CDP and Manager I/O. The caller keeps
/// it alive for the whole program (see docs/gpui-notes.md: dropping it inside
/// the `Application::run` closure kills every CDP task).
pub fn build_runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime")
}

/// Run the app until the window closes or Cmd+Q. Blocks the main thread.
/// `rt` must outlive this call.
pub fn run(startup: Startup, rt: Handle) {
    Application::new().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-b", ToggleSidebar, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
        let mut startup = Some(startup);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("rb".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let root = cx.new(|cx| Root::new(startup.take().expect("once"), rt.clone(), cx));
                window.focus(&root.read(cx).focus_handle(cx));
                root
            },
        )
        .expect("open window");
        cx.activate(true);
    });
}
