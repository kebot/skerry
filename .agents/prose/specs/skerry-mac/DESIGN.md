# Skerry: design and architecture

Skerry (`crates/skerry-mac`, package `skerry-mac`) is a native Mac client for [SealSkin](https://docs.sealskin.app/latest/) and Selkies. It signs in to a SealSkin server with an application key, lists the applications and running sessions, and streams one session into a GPUI window over the Selkies WebSocket transport.

## Goals

- Feel like a Mac app: native windows, menus, full screen, Keychain, Retina, trackpad gestures and input methods.
- Talk to SealSkin and Selkies directly, with no embedded browser.
- Use hardware video decoding where the Mac supports it, and fall back to JPEG when it doesn't.
- Respect what the server allows: its codec list, setting ranges, locked settings and resize policy.

## How a session runs

1. **Sign in.** `api.rs` reads the application key and calls the SealSkin companion API to list applications and sessions.
2. **Launch or resume.** The API returns a session URL. `transport.rs` exchanges it for a session cookie and opens the Selkies WebSocket.
3. **Negotiate.** The client sends one `SETTINGS` message with the codec, initial resolution, display scale and audio options. The server answers with its capabilities, policy and encoder.
4. **Stream.** Video and audio arrive as binary frames. The transport decodes them on a Tokio task and publishes the latest picture to the GPUI view about 30 times a second.
5. **Interact.** The view turns mouse, keyboard, scroll and pinch events into Selkies commands. Session Controls sends settings changes back through the session window.
6. **Disconnect.** Dropping the session aborts the transport task. The remote session keeps running on the server.

## Modules

All code lives in `crates/skerry-mac/src/`.

### `api.rs`: credentials and the SealSkin API

- `Credential::parse` accepts keys of the form `ssk_<id>_<secret>@<host>[:port]` and requires an `https` origin with no path, query or user info.
- `Credential::startup` looks for a key in `SEALSKIN_APPLICATION_KEY`, then `.env` in the working directory, then Keychain (service `dev.kebot.sealskin`, account `application-key`). Keys from the environment or `.env` are never saved to Keychain.
- `Api` sends `Authorization: Bearer <key>` and `x-sealskin-app: other`, with redirects disabled:
  - `POST /companion/api/applications` and `GET /companion/api/sessions` load the library.
  - `POST /companion/api/launch/simple` launches an application.
  - `Api::session_url` rejects session URLs that leave the SealSkin origin.

### `transport.rs`: the Selkies WebSocket

- `socket_request` makes an unredirected `GET` to the session URL (which must carry `access_token` and end in `/`), expects `303 See Other` with a same-origin `Location` and a cookie, then opens `wss://…/api/websockets` with that cookie and `Origin`.
- `run` sends the initial `SETTINGS` message. It asks for the chosen codec with `encoderFallback`, 8-bit 4:2:0 video, the primary display, and an initial resolution taken from the window (`Resolution::from_window`, capped at 4096×2160 with even sizes).
- Binary frames:
  - **JPEG stripes** are decoded on the Tokio task and composited into a BGRA canvas.
  - **H.264, H.265 and AV1 packets** go to the hardware decoder in `video.rs`.
  - **Opus packets** go to `audio::Playback`.
- Text messages carry server settings, display capabilities, playback state, cursor images and encoder information.
- Timers:
  - Every 33 ms: publish a changed frame, send `CLIENT_FRAME_ACK <id> 0`, and flush coalesced pointer motion.
  - Every second: report stream statistics and measure WebSocket ping RTT.
  - Every second while keys are held: send `kh,<keys>` so the server doesn't release them.
- If hardware decoding stops producing frames and can't recover, the transport switches to JPEG when the server allows it, and otherwise reports an error.

### `video.rs`: hardware video decoding

- `Codec` lists JPEG, H.264, H.265 and AV1, with their wire names, labels and a `supported()` check against `VTIsHardwareDecodeSupported`.
- `Packet::parse` reads the Selkies video packet header. `Decoder` builds a `CMFormatDescription` from H.264/HEVC parameter sets or the AV1 sequence header, and runs a VideoToolbox decompression session that requires hardware acceleration.
- Decoded frames are IOSurface-backed `CVPixelBuffer`s (`Surface`), which the view paints directly with `window.paint_surface`.

### `settings.rs`: server settings and preferences

- `Settings` reads the server's numeric ranges and locks for video controls, drops controls the server doesn't support, and merges live updates with the session's starting values.
- `ScreenPolicy` reads whether the server allows resizing and UI scaling and turns local screen changes into Selkies messages.
- `Preferences` are saved per server as JSON in `~/Library/Application Support/SealSkin Native/`.

### `input.rs`: input options

- `InputSettings` holds the per-server switches: Command sends Control, Remote Cursor, Smooth Trackpad Scrolling and Use Mac Input Method.
- `RemoteChord` defines key combinations the Mac would otherwise intercept, such as Alt+Tab and Ctrl+Alt+Delete.

### `audio.rs`: Opus playback

- `Playback` opens the default output device at 48 kHz stereo `f32` and decodes Opus packets into a bounded queue (`48000 * 2 / 8` samples) to cap latency. Without a suitable device the session runs silently.

### `view.rs` and `view/pinch.rs`: the desktop view

- `NativeSession` owns the `Desktop` entity and aborts the transport task when dropped.
- `Desktop` paints a JPEG frame as a `RenderImage`, or a hardware frame as a surface, letterboxed in the window. It retires the previous `RenderImage` with `window.drop_image()` (see [gpui-notes.md](../gpui-notes.md)).
- Mouse, drag and wheel events are mapped to remote pixels and sent as `m,…` / `m2,…`. Keys and modifier changes become X11 keysyms (`kd,<sym>` / `ku,<sym>`). Combinations with Command stay with the Mac unless Command sends Control is on.
- Held keys, modifiers and buttons are released when the window loses focus or the session ends.
- `pinch.rs` installs an AppKit event monitor for trackpad magnification and turns pinches into Control-wheel zoom steps.

### `app.rs`, `app/controls.rs` and `app/display.rs`: windows

- `app.rs` runs a multi-threaded Tokio runtime next to `gpui::Application` and follows the Mac's light or dark appearance. It has three screens:
  1. **Connect:** **Paste Application Key**, with status and progress.
  2. **Library:** a sidebar with the server address and **Remove Connection**, and a main pane with **Applications** (**Launch**), **Running Sessions** (**Resume**) and **Refresh**.
  3. **Session:** the remote desktop fills the window.
- `controls.rs` draws **Session Controls** in a separate, movable child window: playback, input, screen settings, video codec and quality, stream diagnostics, full screen, reconnect and disconnect. It sends commands to the session window, which owns the stream and its settings.
- `display.rs` works out the Mac display's effective UI scale from its native panel size and logical size, so the remote desktop can match it, including fractional scaled modes.

## Scope

Supported now:

- Individual SealSkin sessions over the Selkies WebSocket transport.
- JPEG, plus H.264, H.265 and AV1 on Macs with hardware decoding.
- A remote resolution that follows the window, up to 4096×2160.

Not supported yet:

- WebRTC transport.
- Collaboration rooms, view-only sessions and player slots.
- Clipboard, file transfer, microphone, webcam, gamepads and printing.

[TODO.md](TODO.md) tracks this work.
