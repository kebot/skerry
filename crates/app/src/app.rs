//! Application entry: one window with the [`Root`] view.

use gpui::{
    px, size, App, AppContext, Application, Bounds, Focusable, KeyBinding, TitlebarOptions,
    WindowBounds, WindowOptions,
};
use tokio::runtime::{Handle, Runtime};

use crate::sidebar_model::{Shortcut, KEYMAP};
use crate::startup::Startup;
use crate::state::{
    CloseTab, FocusAddress, GoBack, GoForward, NewTab, Quit, Select1, Select2, Select3, Select4,
    Select5, Select6, Select7, Select8, Select9, ToggleSidebar,
};
use crate::views::Root;

/// Bind every app shortcut (see `sidebar_model::KEYMAP`). These are global:
/// they work whether the Mirror, the address field or the Sidebar has focus,
/// and are never forwarded to the page.
pub fn bind_shortcuts(cx: &mut App) {
    let bindings: Vec<KeyBinding> = KEYMAP
        .iter()
        .map(|(k, s)| match s {
            Shortcut::Quit => KeyBinding::new(k, Quit, None),
            Shortcut::ToggleSidebar => KeyBinding::new(k, ToggleSidebar, None),
            Shortcut::NewTab => KeyBinding::new(k, NewTab, None),
            Shortcut::CloseTab => KeyBinding::new(k, CloseTab, None),
            Shortcut::FocusAddress => KeyBinding::new(k, FocusAddress, None),
            Shortcut::Back => KeyBinding::new(k, GoBack, None),
            Shortcut::Forward => KeyBinding::new(k, GoForward, None),
            Shortcut::SelectTab(0) => KeyBinding::new(k, Select1, None),
            Shortcut::SelectTab(1) => KeyBinding::new(k, Select2, None),
            Shortcut::SelectTab(2) => KeyBinding::new(k, Select3, None),
            Shortcut::SelectTab(3) => KeyBinding::new(k, Select4, None),
            Shortcut::SelectTab(4) => KeyBinding::new(k, Select5, None),
            Shortcut::SelectTab(5) => KeyBinding::new(k, Select6, None),
            Shortcut::SelectTab(6) => KeyBinding::new(k, Select7, None),
            Shortcut::SelectTab(7) => KeyBinding::new(k, Select8, None),
            Shortcut::SelectTab(_) => KeyBinding::new(k, Select9, None),
        })
        .collect();
    cx.bind_keys(bindings);
}

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
        bind_shortcuts(cx);
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
                // Repaint when the system switches between light and dark.
                root.update(cx, |_, cx| {
                    cx.observe_window_appearance(window, |_, _, cx| cx.notify())
                        .detach();
                });
                window.focus(&root.read(cx).focus_handle(cx));
                root
            },
        )
        .expect("open window");
        cx.activate(true);
    });
}
