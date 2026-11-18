# SealSkin Native: design and architecture

SealSkin Native (`crates/sealskin-native`, package `sealskin-native-app`) is a native macOS GPUI client for launching and resuming applications on a [SealSkin server](https://docs.sealskin.app/latest/).

## Purpose

Connect directly to a SealSkin server with an application key, list available applications and running sessions, and stream an individual remote application desktop over the Selkies WebSocket transport with JPEG video, mouse/keyboard input, and Opus audio.

## Architecture

All SealSkin Native code lives in `crates/sealskin-native/src/`:

- **`api.rs` — credentials and companion REST API**
  - `Credential::parse` validates application keys of the form `ssk_<id>_<secret>@<host>[:port]`, requiring an `https` origin with no path, query, or credentials in the URL.
  - `Credential::startup` checks `SEALSKIN_APPLICATION_KEY` in the environment, then `.env` in the working directory, and finally macOS Keychain (`dev.kebot.sealskin` / `application-key`). Env/`.env` keys are used for the current run only and are not saved to Keychain.
  - `Api` sends `Authorization: Bearer <key>` and `x-sealskin-app: other` with redirects disabled:
    - `POST /companion/api/applications` and `GET /companion/api/sessions` to load the library.
    - `POST /companion/api/launch/simple` to launch an application.
    - `Api::session_url` verifies that returned `session_url` paths stay on the same SealSkin origin.

- **`transport.rs` — Selkies WebSocket transport**
  - `socket_request` performs an unredirected `GET` on the session URL (which must carry `access_token` and end with `/`), expects `303 See Other` with a same-origin `Location` and `Set-Cookie`, and opens `wss://.../api/websockets` with the session `Cookie` and `Origin`.
  - Negotiates JPEG video via `SETTINGS` and requests an initial 1280×720 display with Opus redundancy disabled. It uses the server’s frame-rate and quality defaults when announced, with the previous 30 fps and quality 75 requests available as a compatibility fallback.
  - Incoming binary frames:
    - Type `3`: JPEG stripe at vertical offset `y`, decoded off-UI on the Tokio task and composited into a BGRA `RgbaImage` canvas (bounded to `4096×2160`).
    - Type `4`: non-JPEG video fallback; triggers an error after the 15 s video deadline if no JPEG arrives.
    - Type `1`: Opus audio packet forwarded to `audio::Playback`.
  - Incoming text messages handle stream resolution, server policy, and encoder information. Pause/resume and mute labels follow server playback messages. Audio startup respects the server policy and the local mute choice.
  - A one-second sample reports published-frame rate, received binary payload rate, JPEG stripe processing time, rejected stripes, and WebSocket ping RTT. Encoder information comes separately from the server.
  - Outgoing ticks: 33 ms presentation tick publishes changed `Frame` snapshots over a `watch` channel, sends `CLIENT_FRAME_ACK <id> 0`, and flushes coalesced pointer motion; 1 s heartbeat sends `kh,<keys>` while keys are held.

- **`settings.rs` — JPEG controls and preferences**
  - Reads numeric ranges and locks, filters unsupported controls, and reconciles live updates with the session’s initial defaults. Stores per-server preferences in Application Support.

- **`audio.rs` — Opus playback**
  - `Playback` opens the default `cpal` output device at 48 kHz stereo `f32` and decodes incoming Opus packets with `opus-decoder` into a bounded ring queue (`48000 * 2 / 8` samples) to cap latency.

- **`view.rs` — GPUI desktop view and input**
  - `NativeSession` owns the `Desktop` GPUI entity and aborts the background Tokio transport task on drop.
  - `Desktop` renders the latest `Arc<RenderImage>` scaled and letterboxed via `fit()`, and retires the previous image with `window.drop_image()` in the canvas paint closure (see [gpui-notes.md](../gpui-notes.md)).
  - Maps mouse move, press/release, drag, and wheel events into remote pixel coordinates and sends Selkies `m,...` / `m2,...` commands.
  - Maps key down/up and modifier changes to X11 keysyms (`kd,<sym>` / `ku,<sym>`). `Cmd` (`platform`) combinations are left for the macOS app (such as `Cmd+Q`).
  - Releases held keys, modifiers, and mouse buttons on focus loss, window deactivation, or disconnect.

- **`app.rs` — Launcher and session UI**
  - Runs a multi-threaded Tokio runtime alongside `gpui::Application`, respecting light/dark `WindowAppearance`.
  - Screens:
    1. **Connect:** **Paste Application Key** button (and status/busy feedback).
    2. **Library:** sidebar with server address and **Remove Connection**; main pane with **Applications** (**Launch**), **Running Sessions** (**Resume**), and **Refresh**.
    3. **Active session:** toolbar with playback controls, a Video panel with JPEG settings, stream diagnostics and sizing modes, plus Full Screen, Reconnect, and Disconnect.

## Scope and non-goals (current version)

- Supports individual sessions using the Selkies WebSocket transport with JPEG enabled.
- Requests 1280×720 initially and scales the remote picture to fit the local window.
- WebRTC, other video codecs, and collaboration rooms are outside this version's scope.
