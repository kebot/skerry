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
  - `sealskin.rs` (`SealSkinClient`, the control plane, `ARCH-024`; was `api.rs`/`Api`): `Credential::parse` (`ssk_<id>_<secret>@<host>[:port]`, HTTPS-only), `Credential::startup` (`SEALSKIN_APPLICATION_KEY` -> `.env` -> `application_key` in `~/.config/skerry/config.toml`, written 0600 in a 0700 folder; never the macOS Keychain, `ARCH-011`), and `Api` (`MINIMUM_SEALSKIN_VERSION = 0.5.0` gate via `GET /companion/api/ui/version` before any other call, `POST /companion/api/applications`, `GET /companion/api/sessions`, `POST /companion/api/admin/status`, `GET`/`POST /companion/api/homedirs`, `POST /companion/api/launch/simple`, same-origin `session_url`, and home storage `GET /companion/api/files/list/<home>` / `POST …/files/delete/<home>` + `delete_status`); `launch_options.rs`: the Launch Dialog's choices as SealSkin's web launcher makes them (`ARCH-016`). `launch_progress.rs`: follows each launch by `launch_id` through the progress route without double launches (`ARCH-027`).
  - `selkies.rs` (`SelkiesClient`, one Selkies session's HTTP file routes, `ARCH-023`/`ARCH-024`; was `files.rs`/`Access`) and `app/{files_window,files_model,promise}.rs` (the per-session Files window, its browsing model, and `NSFilePromiseProvider` drags).
  - `transport.rs`: `socket_request` (unredirected `GET` -> `303 See Other` cookie exchange -> `wss://.../api/websockets`), initial `SETTINGS` handshake, 33 ms frame publish + `CLIENT_FRAME_ACK`, 1 s stream stats/ping RTT, and `kh,<keys>` held-key keepalive.
  - `video.rs`: `Codec` (JPEG, H.264, H.265, AV1), `VTIsHardwareDecodeSupported`, Annex B / parameter-set / AV1 sequence-header parsing, and IOSurface-backed `CVPixelBuffer` (`kCVPixelFormatType_420YpCbCr8BiPlanarFullRange`) painted via `window.paint_surface`.
  - `settings.rs`: server setting ranges/locks intersection, `ScreenPolicy`, and per-server JSON preferences in `~/Library/Application Support/Skerry/` (with read fallback to legacy `~/Library/Application Support/SealSkin Native/`), loaded field by field so a newer build's keys survive (`ARCH-026`, `ARCH-040`).
  - `persist.rs`: the single FIFO write queue on Tokio for preferences and the key. Its in-memory copy serves every read, and it flushes within 1 s on quit (`ARCH-026`).
  - `view/keys.rs`: `KeyLedger`, the pure held-key bookkeeping that releases keys before close, on blur, and before child windows (`ARCH-026`).
  - `shortcuts.rs`: `CHORDS` and the single ordered `key_bindings()`, with the `NoAction` suppression layer last under `SessionDesktop` (`ARCH-031`).
  - `clipboard.rs` & `pasteboard.rs`: Selkies clipboard protocol (`encode`, `Receiver`, `Policy` / `ClipboardSettings` / `Effective`) and the general `NSPasteboard` bridge (`change_count`, `read`, `write`, concealed secrets) (`ARCH-017`).
  - `input.rs`: `InputSettings` (Command sends Control, Remote Cursor, Smooth Trackpad Scrolling, Use Mac Input Method) and `RemoteChord` (Alt+Tab, Ctrl+Alt+Delete).
  - `audio.rs`: 48 kHz stereo f32 Opus playback on a per-session `skerry-audio` thread (the only place cpal/Core Audio is called), which follows the default or a chosen output device and recovers with backoff. The latency queue is bounded at `48000 * 2 / 8` samples (`ARCH-030`).
  - `view.rs` & `view/pinch.rs`: `NativeSession`, `Desktop` canvas (`RenderImage` with `window.drop_image()` retirement for JPEG; `paint_surface` for hardware frames), mouse/wheel/keyboard/IME dispatch, and AppKit magnification monitor.
  - `crates/skerry-ui`: pure `gpui-kit` `0.10.0` UI library (`components.rs`, `connect.rs`, `library.rs`, `launch_dialog.rs`, `controls.rs`, `session.rs`) with zero networking/macOS-platform dependencies, plus a standalone interactive preview workbench (`examples/preview.rs`, launched via `cargo run -p skerry-ui --example preview`) covering Connect, Library, Session Controls, and Session Overlays across Light/Dark/System themes.
  - `app.rs`, `app/session_window.rs`, `app/controls.rs`, & `app/display.rs`: multi-threaded Tokio runtime beside `gpui_kit::application()`, the Library window (`Launcher`: Connect/Library screens delegated to `skerry-ui` + `Theme::sync_system_appearance`, reopened with `cmd-0` or the Dock), one `SessionWindow` per open session tracked in the `OpenWindows` global (`ARCH-022`), movable Session Controls child window per session (`cmd-shift-m`, opened via `cx.defer` with `Owner { window: AnyWindowHandle, session: Entity<SessionWindow> }`), and native display UI scale detection.
- `macos-release-pipeline` (`ARCH-040`):
  - `scripts/bundle.sh` produces ad-hoc signed `dist/Skerry.app` (`dev.kebot.skerry`, `assets/icons/Skerry.icns`), `dist/Skerry-<version>-<arch>.dmg`, `dist/Skerry-<version>-<arch>.zip`, and `dist/SHA256SUMS`, with optional `CODESIGN_IDENTITY` and notarization env-var hooks.
  - `.github/workflows/release-macos.yml` builds and publishes macOS `arm64` releases on `v*` tags (GitHub Actions and Forgejo Actions compatible).
  - `Casks/skerry.rb` (in-repo Homebrew tap) and `README.md` provide Homebrew Cask (`brew tap kebot/skerry https://github.com/kebot/skerry && brew install --cask kebot/skerry/skerry`) and direct DMG (`xattr -cr /Applications/Skerry.app`) installation.
- `domain-readiness`:
  - `video-and-screen`: Milestone 1, H.264/H.265/AV1 (continuity on Selkies 2.3.0 in `ARCH-012`), JPEG stripe decode (`ARCH-034`), stats overlay (`ARCH-031`), and Second Display (`ARCH-039`) settled; cross-display drag (`ARCH-GAP-VS-07`), WebRTC (`ARCH-GAP-VS-02`), VP8/VP9/advanced encoder options (`ARCH-GAP-VS-03`), and stream-toggle tile row (`ARCH-GAP-VS-05`) deferred.
  - `launcher-and-sessions`: launch/resume, Library search/icons/categories/favorites/hidden/recents, Launch Dialog (`ARCH-016`), startup progress & one-click launch (`ARCH-027`), open links/files (`ARCH-028`), account menu & key recovery (`ARCH-029`), multi-window sessions (`ARCH-022`), direct Selkies servers (`ARCH-036`), and Storage window (`ARCH-037`) settled; public share links (`ARCH-GAP-LS-14`), cluster origins (`ARCH-GAP-LS-07`), and native Mac app presence (`ARCH-GAP-LS-09`) deferred.
  - `desktop-and-collaboration`: keyboard/cursor/trackpad/IME, key release before close (`ARCH-026`), clipboard sync (`ARCH-017`), file transfers (`ARCH-023`/`ARCH-024`), audio output devices & bitrate (`ARCH-030`), Shortcuts (`ARCH-031`), pointer capture & gamepads (`ARCH-032`), remote PDF printing (`ARCH-033`), Apps catalog (`ARCH-035`), and microphone/camera uplinks (`ARCH-038`) settled; multichannel surround audio (`ARCH-GAP-DC-17`) open on the frontier, with session file mount (`ARCH-GAP-DC-14`), remote rename/move (`ARCH-GAP-DC-15`), mic echo cancellation (`ARCH-GAP-DC-16`), session sharing/rooms (`ARCH-GAP-DC-07`), and raw multitouch (`ARCH-GAP-DC-08`) deferred/blocked.

Facets:
- `architecture` — module boundaries and GPUI/Tokio/VideoToolbox invariants
- `video-and-screen` — transport, codecs, display resolution, HiDPI, playback, and diagnostics
- `launcher-and-sessions` — SealSkin API, launching, session management, storage, and cluster origins
- `desktop-and-collaboration` — macOS input, clipboard, file transfers, media devices, printing, and sharing

Material: structural changes to `crates/skerry-mac/src/**`, newly resolved or changed `ARCH-*` constraints, and postcondition pass/fail transitions.
Immaterial: build timestamps, local target cache state, and formatting-only edits.

Postconditions:
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --check` pass.
- No embedded browser is used; Skerry talks directly to SealSkin and Selkies. The one exception is the Apps catalog fetch from the Selkies web client's GitHub raw URL, which uses a separate client with no credentials, is size-capped, and is HTTPS only (`ARCH-035`).
- Session URLs and application keys are never written to preferences files; environment/`.env` keys are never persisted to `~/.config/skerry/config.toml`, and Skerry never reads or writes the macOS Keychain (`ARCH-011`). Direct Selkies credentials and certificate pins are the one other secret kept there, in `[[selkies]]` entries (`ARCH-036`).

### Continuity

input-driven

### Invariants

- `crates/skerry-mac/Cargo.toml` uses `gpui-kit = "0.10.0"` (initialized via `gpui_kit::init(cx)` with `gpui_kit::assets::Assets`; windows opened via `gpui_kit::open_window` are wrapped in `gpui_kit::base::Root`).
- Cross-window handles must store `(AnyWindowHandle, Entity<T>)` instead of calling `window.window_handle().downcast::<T>()` (since `gpui_kit::open_window` mounts `Root`), and child windows whose initial render reads another entity (session windows opened by the `Launcher`, Session Controls reading its `SessionWindow`) must be opened via `cx.defer(...)` outside that entity's update closures to avoid re-entrant entity lease panics.
- The multi-threaded Tokio `Runtime` is created in `main` before `Application::run` and kept alive for the lifetime of the app; all network I/O runs on Tokio and communicates with GPUI via `mpsc` / `watch` channels.
- JPEG stripes decoded off the main thread into per-stripe `RenderImage`s (`ARCH-034`) must retire replaced stripes via `window.drop_image` inside the paint closure, 4 paints later, to prevent texture leaks (~10 MB/s at 60 fps).
- Async or long-lived work on an entity that another window also renders (a `SessionWindow` is rendered by its Session Controls) captures `Window::window_handle(window)` and updates through that handle. `gpui-pre`'s `update_in` resolves the entity's *current* window instead (`ARCH-015`).
- Library images (app icons, session thumbnails) are GPU textures too: every replaced, removed, or stale image is retired with `window.drop_image` on the next Launcher render, and server images are decoded with size limits and downscaled before upload (`ARCH-014`).
- Hardware-decoded VideoToolbox frames use IOSurface-backed `CVPixelBuffer`s painted directly with `window.paint_surface` with zero copy.
- The application key is attached only to requests whose resolved path is under `/companion/api/`; every other fetch (logos, icons) uses a client with no key (`ARCH-026`).
- Preference and key files are written only by the `persist.rs` queue, never from the GPUI main thread (`ARCH-026`).
- TLS uses rustls with the ring provider only; aws-lc-rs must not enter the dependency tree (`ARCH-036`).
- Microphone and camera devices are opened only after a granted Start, and every stream end closes them (`ARCH-038`).
- Every frame a session socket sends goes through `transport::role::Wire`. On a Display 2 connection it always carries `displayId:"display2"` (`ARCH-039`).
- Skerry supports SealSkin `0.5.0` or newer (`ARCH-009`): it refuses to connect to a server whose `GET /companion/api/ui/version` reports an older or unparseable version, or that has no such route. Changing the minimum is a new superseding `ARCH-*` decision.

### Execution

```prose
let result = call implement-feature
  target: target
return result
```
