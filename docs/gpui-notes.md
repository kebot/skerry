# GPUI notes (gpui 0.2.2, verified on this Mac)

## Setup and toolchain

- `crates/app/Cargo.toml`: `gpui = { version = "0.2.2", features = ["runtime_shaders"] }`.
- Without `runtime_shaders`, gpui's build script runs `xcrun metal` and fails with
  `cannot execute tool 'metal' due to missing Metal Toolchain`. Xcode 27 here has the toolchain
  component "uninstalled". Two fixes:
  1. (used) the `runtime_shaders` feature: Metal shaders are compiled at app start, no build-time
     toolchain needed. Slight startup cost; fine for dev. Drop it if the toolchain is installed.
  2. `xcodebuild -downloadComponent MetalToolchain` (downloads several hundred MB, changes system
     state; not run by us).
- Build time: cold build of gpui and deps about 25 s (debug, many cores); incremental rebuild of
  rb-app about 2 s. `cargo clippy` cold about 28 s.
- Sources for reference: `~/.cargo/registry/src/*/gpui-0.2.2/{examples,src}`. Examples used:
  `hello_world.rs`, `set_menus.rs`, `image_loading.rs`.

## App skeleton

```rust
Application::new().run(|cx: &mut App| {
    cx.bind_keys([KeyBinding::new("cmd-q", Quit, None), KeyBinding::new("cmd-b", ToggleSidebar, None)]);
    cx.on_action(|_: &Quit, cx| cx.quit());                  // global action handler
    cx.on_window_closed(|cx| if cx.windows().is_empty() { cx.quit() }).detach();
    cx.open_window(WindowOptions { titlebar: Some(TitlebarOptions { title: Some("rb".into()), ..Default::default() }),
                   ..Default::default() },
        |window, cx| { let root = cx.new(Root::new); window.focus(&root.read(cx).focus_handle(cx)); root }).unwrap();
    cx.activate(true);
});
```

- `actions!(rb, [Quit, ToggleSidebar]);` declares unit actions.
- `cx.new(...)` needs `use gpui::AppContext` in scope; `focus_handle` needs `Focusable`.
- View-level action: `div().key_context("Root").track_focus(&focus).on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {...; cx.notify();}))`.
  The view must be focused for the binding to dispatch (we focus it at window creation).
- Click: `.on_mouse_down(MouseButton::Left, cx.listener(|this, ev, window, cx| {...; cx.notify()}))`.
- Render fn: `fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement`.
- Optional children: `.children(option_element)`.

## Drawing a Frame (JPEG bytes) as an image

Simplest path (verified to compile): wrap bytes in `gpui::Image` and use the `img` element. gpui decodes
with the `image` crate and converts RGBA to BGRA itself.

```rust
use gpui::{img, Image, ImageFormat, ObjectFit, StyledImage, Styled, px};
self.frame = Some(Arc::new(Image::from_bytes(ImageFormat::Jpeg, jpeg_bytes)));
cx.notify();
// in render:
img(self.frame.clone().unwrap()).object_fit(ObjectFit::Contain).w(px(w)).h(px(h))
```

- `ObjectFit::Contain` gives scale + letterbox; the element bounds are the letterbox container, so the
  coordinate mapping helper (T6) must use the same fit math.
- Each new `Arc<Image>` gets a new id, so a new texture is uploaded per Frame; gpui's image cache
  (keyed by id) can grow with a stream of Frames. If memory grows, decode yourself and build
  `RenderImage::new(SmallVec<[image::Frame; 1]>)` (pixels must be BGRA), then call
  `window.drop_image(Arc<RenderImage>)` on the previous one. That needs a direct `image` dep (not added).
  `img(ImageSource::Render(arc_render_image))` draws it. Lower-level: `Window::paint_image(bounds, corner_radii,
  Arc<RenderImage>, frame_index, grayscale)` inside a custom element's paint.
- Decode JPEG off the main thread: `cx.background_spawn(async move { ... })` then update the view.

## Tokio and GPUI executors

gpui has its own executors (main-thread `ForegroundExecutor`, thread-pool `BackgroundExecutor`); futures
spawned by gpui are not inside a tokio runtime, so tokio I/O (tokio-tungstenite, reqwest) panics there.
Recommended bridge: own a tokio `Runtime` (create it in `main` before `Application::run`, keep it in a
`Global`/state), run all CDP and Manager I/O on it, and push results to the UI through a channel that the
gpui side awaits:

```rust
// UI side (compiles; `rx` is any async receiver, e.g. futures/async-channel/tokio mpsc recv().await)
cx.spawn(async move |this, cx| {            // this: WeakEntity<V>, cx: &mut AsyncApp
    while let Some(msg) = rx.recv().await {
        if this.update(cx, |v, cx| { v.apply(msg); cx.notify(); }).is_err() { break; } // entity dropped
    }
}).detach();
// tokio side
runtime.spawn(async move { tx.send(msg).await.ok(); });
```

- Tokio's `mpsc::Receiver::recv()` is runtime-agnostic (it only needs a waker), so it can be awaited in
  gpui's executor. Prefer bounded channels for Frames (drop or coalesce stale Frames; ack after decode).
- Input back to tokio: call `runtime.spawn(...)` or send on an `UnboundedSender` from event handlers.
- `cx.spawn` closures are `AsyncFnOnce(WeakEntity<T>, &mut AsyncApp)` (async closures, Rust 2021 with
  recent stable is fine). `cx.background_spawn(fut)` runs on gpui's pool (not tokio).

## Findings and caveats

- `Application::new().run(...)` blocks the main thread (macOS requires it); do tokio setup before it.
- Closing the last window does not quit by default; we register `on_window_closed` to quit.
- Cmd+Q: bound globally via `bind_keys` with no context, plus `on_action`.
- The binary window runs and has title "rb" (confirmed through System Events); the Cmd+Q keystroke test
  via osascript was not confirmed (likely no Accessibility permission for the terminal), so verify
  Cmd+Q, Cmd+B and click counter by hand once.
- `cargo` prints a future-incompat warning for `block v0.1.6` and `proc-macro-error2` (gpui deps); harmless.

## Mirror (T7): Frame textures, input, IME

Code: `crates/app/src/mirror/` (`mod.rs` view, `input_map.rs` pure mapping,
`backend.rs` `MirrorBackend`), demo: `cargo run -p rb-app --example mirror_demo`.

* **Texture growth, measured.** The Mirror decodes JPEG itself (image crate,
  off the main thread) into a BGRA `RenderImage`, paints it with
  `Window::paint_image` into the `FrameFit` dest rect, and calls
  `Window::drop_image(previous)` from the paint closure once a newer Frame is
  shown. Demo at 60 fps requested (1000x700 JPEG Frames, debug build, about
  40 s): RSS stayed flat at about 114-123 MB. Control run with the drop
  disabled (previous images leaked): RSS grew from 170 MB to 595 MB in
  about 36 s (roughly 10 MB/s). So the drop is required; each new
  `RenderImage` gets a new id and a new atlas tile. The simple `img(Image)` path was
  not measured separately; it caches by image id and has no per-Frame drop
  hook, so use the explicit path.
* `drop_image` needs a `Window`, so retired images go in a shared queue that
  the canvas paint closure drains.
* **Ack contract:** the `cx.spawn` loop receives a Frame, decodes on the
  background executor, updates the view (`cx.notify()`), and only then spawns
  `Frame::ack()` on the tokio runtime. Decode failures still ack.
* **Gotcha:** `Application::run`'s closure returns right after setup. A tokio
  `Runtime` moved into it is dropped and all CDP tasks die silently. Keep the
  runtime in `main`'s scope (alive until `run` returns) or a Global.
* **Mouse:** registered with `window.on_mouse_event` in paint (not div
  handlers) so drags outside the Mirror still arrive; presses/moves/wheel need
  `hitbox.is_hovered`, releases and drag moves do not. Letterbox bars: ignored;
  while a button is held: clamped to the remote viewport.
* **Wheel:** `ScrollWheelEvent` delta follows the macOS content-movement
  direction with the user's natural-scrolling setting already applied, so CDP
  deltas are simply negated (lines x 40 px). Not verified with a real trackpad.
* **Keyboard:** `on_key_down/up` on the focused Mirror. Cmd combinations are
  not forwarded and not stopped (app bindings such as Cmd+Q, Cmd+B and macOS
  key equivalents keep working). Mapped keys call `cx.stop_propagation()`.
  Modifier-only presses are not forwarded.
* **IME:** gpui exposes `EntityInputHandler` + `Window::handle_input`; the
  Mirror implements it and forwards committed text via `Input.insertText`
  (emoji picker, dictation). Because mapped printable keys are consumed in
  `on_key_down` (macOS calls the key handler before the IME), composition of
  typed keys (CJK input methods) does not start. Supporting it needs a mode
  that lets printable keys fall through to the IME; left as an open issue.
