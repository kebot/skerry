---
name: skerry
kind: responsibility
id: 067NC4KG01SKERRY0000000001
---

# Skerry Native Mac Client

<!-- compose:status=settled provenance=compose-managed -->

### Goal

Provide a native macOS client (`crates/skerry-mac`, `gpui-kit` `0.10.0`) for SealSkin and Selkies that signs in with an application key, lists and launches remote applications and running sessions, and streams remote desktops over the Selkies transport with hardware-accelerated VideoToolbox decoding, Core Audio playback, and native macOS input and window integration.

### Requires

- `rust-workspace`: Rust implementation in `crates/skerry-ui/src/**`, `crates/skerry-mac/src/**`, and `Cargo.toml`
- `video-and-screen`: video decoding, resolution, HiDPI, playback, and diagnostics truth from `video-and-screen.prose.md`
- `launcher-and-sessions`: SealSkin companion API, launch options, session lifecycle, and storage truth from `launcher-and-sessions.prose.md`
- `desktop-and-collaboration`: macOS input, clipboard, file transfer, audio/video devices, printing, and collaboration truth from `desktop-and-collaboration.prose.md`

### Maintains

- `system-architecture`: verified module boundaries across `crates/skerry-mac/src/`:
  - `api.rs`: `Credential::parse` (`ssk_<id>_<secret>@<host>[:port]`, HTTPS-only), `Credential::startup` (`SEALSKIN_APPLICATION_KEY` -> `.env` -> macOS Keychain `dev.kebot.sealskin`), and `Api` (`POST /companion/api/applications`, `GET /companion/api/sessions`, `POST /companion/api/launch/simple`, same-origin `session_url`).
  - `transport.rs`: `socket_request` (unredirected `GET` -> `303 See Other` cookie exchange -> `wss://.../api/websockets`), initial `SETTINGS` handshake, 33 ms frame publish + `CLIENT_FRAME_ACK`, 1 s stream stats/ping RTT, and `kh,<keys>` held-key keepalive.
  - `video.rs`: `Codec` (JPEG, H.264, H.265, AV1), `VTIsHardwareDecodeSupported`, Annex B / parameter-set / AV1 sequence-header parsing, and IOSurface-backed `CVPixelBuffer` (`kCVPixelFormatType_420YpCbCr8BiPlanarFullRange`) painted via `window.paint_surface`.
  - `settings.rs`: server setting ranges/locks intersection, `ScreenPolicy`, and per-server JSON preferences in `~/Library/Application Support/SealSkin Native/`.
  - `input.rs`: `InputSettings` (Command sends Control, Remote Cursor, Smooth Trackpad Scrolling, Use Mac Input Method) and `RemoteChord` (Alt+Tab, Ctrl+Alt+Delete).
  - `audio.rs`: 48 kHz stereo `f32` Opus `Playback` with bounded latency queue (`48000 * 2 / 8` samples).
  - `view.rs` & `view/pinch.rs`: `NativeSession`, `Desktop` canvas (`RenderImage` with `window.drop_image()` retirement for JPEG; `paint_surface` for hardware frames), mouse/wheel/keyboard/IME dispatch, and AppKit magnification monitor.
  - `crates/skerry-ui`: pure `gpui-kit` `0.10.0` UI library (`components.rs`, `connect.rs`, `library.rs`, `controls.rs`, `session.rs`) with zero networking/macOS-platform dependencies, plus a standalone interactive preview workbench (`examples/preview.rs`, launched via `cargo run -p skerry-ui --example preview`) covering Connect, Library, Session Controls, and Session Overlays across Light/Dark/System themes.
  - `app.rs`, `app/controls.rs`, & `app/display.rs`: multi-threaded Tokio runtime beside `gpui_kit::application()`, Connect/Library/Session screens delegated to `skerry-ui` + `Theme::sync_system_appearance`, movable Session Controls child window (`cmd-shift-m`, opened via `cx.defer` with `Owner { window: AnyWindowHandle, launcher: Entity<Launcher> }`), and native display UI scale detection.
- `domain-readiness`:
  - `video-and-screen`: Milestone 1 + H.264/H.265/AV1 settled; second display, WebRTC, and VP8/VP9/advanced encoder options open on the frontier.
  - `launcher-and-sessions`: simple launch/resume settled; launch options, session stop/previews, open links/files, library search/favorites, persistent storage, account recovery, and cluster origins open on the frontier.
  - `desktop-and-collaboration`: keyboard/cursor/trackpad/IME settled; clipboard, file transfers, audio output device selection, printing, mic/webcam, gaming, and sharing open on the frontier.

Facets:
- `architecture` — module boundaries and GPUI/Tokio/VideoToolbox invariants
- `video-and-screen` — transport, codecs, display resolution, HiDPI, playback, and diagnostics
- `launcher-and-sessions` — SealSkin API, launching, session management, storage, and cluster origins
- `desktop-and-collaboration` — macOS input, clipboard, file transfers, media devices, printing, and sharing

Material: structural changes to `crates/skerry-mac/src/**`, newly resolved or changed `ARCH-*` constraints, and postcondition pass/fail transitions.
Immaterial: build timestamps, local target cache state, and formatting-only edits.

Postconditions:
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --check` pass.
- No embedded browser is used; Skerry talks directly to SealSkin and Selkies.
- Session URLs and application keys are never written to preferences files; environment/`.env` keys are never persisted to Keychain.

### Continuity

input-driven

### Invariants

- `crates/skerry-mac/Cargo.toml` uses `gpui-kit = "0.10.0"` (initialized via `gpui_kit::init(cx)` with `gpui_kit::assets::Assets`; windows opened via `gpui_kit::open_window` are wrapped in `gpui_kit::base::Root`).
- Cross-window handles must store `(AnyWindowHandle, Entity<T>)` instead of calling `window.window_handle().downcast::<T>()` (since `gpui_kit::open_window` mounts `Root`), and child windows whose initial render reads `Launcher` must be opened via `cx.defer(...)` outside `Launcher` update closures to avoid re-entrant entity lease panics.
- The multi-threaded Tokio `Runtime` is created in `main` before `Application::run` and kept alive for the lifetime of the app; all network I/O runs on Tokio and communicates with GPUI via `mpsc` / `watch` channels.
- JPEG frames decoded off the main thread into `RenderImage` must retire previous frames via `window.drop_image(previous)` inside the paint closure to prevent texture leaks (~10 MB/s at 60 fps).
- Hardware-decoded VideoToolbox frames use IOSurface-backed `CVPixelBuffer`s painted directly with `window.paint_surface` with zero copy.

### Execution

```prose
let result = call implement-feature
  target: target
return result
```
