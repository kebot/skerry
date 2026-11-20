# GPUI notes (gpui 0.2.2, macOS)

Notes and pitfalls found while building on GPUI. Most were first measured in an earlier prototype and still apply to Skerry.

## Toolchain

- `crates/skerry-mac/Cargo.toml` uses `gpui = { version = "0.2.2", features = ["runtime_shaders"] }`.
- Without `runtime_shaders`, GPUI's build script runs `xcrun metal` and fails with `cannot execute tool 'metal' due to missing Metal Toolchain`. There are two fixes:
  1. **Used here:** `runtime_shaders`, which compiles the Metal shaders when the app starts. It adds a small startup cost and needs no build-time toolchain.
  2. `xcodebuild -downloadComponent MetalToolchain`, which downloads several hundred MB and changes system state.
- A cold debug build of GPUI and its dependencies takes about 25 s on a many-core Mac. Incremental rebuilds take a few seconds.
- `cargo` prints future-incompatibility warnings for `block v0.1.6` and `proc-macro-error2`, which come from GPUI's dependencies. They're harmless.
- The GPUI examples in `~/.cargo/registry/src/*/gpui-0.2.2/examples` are the best reference, especially `hello_world.rs`, `set_menus.rs` and `image_loading.rs`.

## App skeleton

```rust
Application::new().run(|cx: &mut App| {
    cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_window_closed(|cx| if cx.windows().is_empty() { cx.quit() }).detach();
    cx.open_window(
        WindowOptions { titlebar: Some(TitlebarOptions { title: Some("Skerry".into()), ..Default::default() }), ..Default::default() },
        |window, cx| { let root = cx.new(Root::new); window.focus(&root.read(cx).focus_handle(cx)); root },
    ).unwrap();
    cx.activate(true);
});
```

- `actions!(app, [Quit]);` declares unit actions.
- `cx.new(...)` needs `use gpui::AppContext`, and `focus_handle` needs `Focusable`.
- A view-level action needs `key_context`, `track_focus` and `on_action(cx.listener(...))`, and the view must be focused for its bindings to fire.
- `Application::run` blocks the main thread, as macOS requires. Do Tokio setup before it.
- Closing the last window doesn't quit by default. Register `on_window_closed` to quit.

## Tokio and GPUI

GPUI has its own executors. Futures it spawns don't run inside a Tokio runtime, so Tokio I/O (tokio-tungstenite, reqwest) panics there.

- Create a Tokio `Runtime` in `main`, before `Application::run`, and keep it alive for the life of the app. **Gotcha:** the closure passed to `Application::run` returns right after setup, so a runtime moved into it is dropped and every network task dies silently.
- Run all network I/O on the Tokio runtime and send results to the UI over a channel. Tokio's `mpsc` and `watch` receivers only need a waker, so GPUI tasks can await them:

```rust
cx.spawn(async move |this, cx| {
    while let Some(msg) = rx.recv().await {
        if this.update(cx, |v, cx| { v.apply(msg); cx.notify(); }).is_err() { break; }
    }
}).detach();
```

- For input going the other way, call `runtime.spawn(...)` or send on an unbounded channel from event handlers.
- Use bounded or `watch` channels for video, so stale frames are replaced rather than queued.

## Drawing video frames

### JPEG: `RenderImage` and `drop_image`

The simple path, `img(Arc<Image>)`, gives every frame a new image id and a new texture, and GPUI's image cache never releases them. Instead:

- Decode the JPEG yourself, off the main thread, into BGRA pixels and build `RenderImage::new(smallvec![image::Frame::new(pixels)])`.
- Paint it with `Window::paint_image` inside a `canvas`, using your own letterbox rectangle.
- Call `window.drop_image(previous)` once a newer frame is on screen. `drop_image` needs a `Window`, so queue retired images and drain the queue in the paint closure.

Measured at 60 fps with 1000×700 frames: memory stayed flat at about 120 MB with `drop_image`, and grew by about 10 MB/s without it.

### Hardware video: `paint_surface`

VideoToolbox can output IOSurface-backed, Metal-compatible `CVPixelBuffer`s in full-range NV12 (`kCVPixelFormatType_420YpCbCr8BiPlanarFullRange`). `window.paint_surface(bounds, pixel_buffer)` draws these directly, with no copy and no atlas tile, so there's nothing to drop.

## Input

- **Mouse:** register with `window.on_mouse_event` during paint rather than with `div` handlers, so drags that leave the picture still arrive. Presses, moves and wheel events need `hitbox.is_hovered`; releases and drag moves don't. Ignore the letterbox bars, and clamp to the remote picture while a button is held.
- **Wheel:** `ScrollWheelEvent` deltas already include the user's natural-scrolling setting and follow content movement.
- **Keyboard:** handle `on_key_down`/`on_key_up` on the focused view and call `cx.stop_propagation()` for keys you forward. Leave Command combinations alone so app bindings such as ⌘Q and macOS key equivalents keep working.
- **Input methods:** GPUI exposes `EntityInputHandler` and `Window::handle_input`. Because macOS calls the key handler before the input method, a view that consumes printable keys in `on_key_down` stops CJK composition from starting. To support it, let printable keys fall through to the input method and send the committed text instead. Skerry's **Use Mac Input Method** switch does this.
- **Trackpad gestures:** GPUI doesn't expose magnification events. Skerry installs an AppKit event monitor (`view/pinch.rs`) for pinch gestures.
