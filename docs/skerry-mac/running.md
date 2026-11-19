# Running SealSkin Native

## 1. Prerequisites

- A [SealSkin server](https://docs.sealskin.app/latest/) with a trusted HTTPS certificate.
- Your account must allow the **Other** client type for application keys.
- Individual sessions must support the Selkies WebSocket transport and allow a codec supported by the Mac.

## 2. Application key

Sign in to your SealSkin server's web app, open **Settings → Application Keys**, and create a key. Copy the full value, including the server address after `@` (for example `ssk_<id>_<secret>@seal.example`).

You can supply the key in either of two ways:

- **In the app (Keychain):** click **Paste Application Key**. After a successful connection, SealSkin Native stores the key in macOS Keychain (`dev.kebot.sealskin` / `application-key`). Use **Remove Connection** in the sidebar to delete it from Keychain.
- **Environment or `.env` (for development):** set `SEALSKIN_APPLICATION_KEY` in your environment or in `.env` in the working directory:

```sh
SEALSKIN_APPLICATION_KEY=ssk_id_secret@seal.example
```

Keys supplied via `SEALSKIN_APPLICATION_KEY` or `.env` are used for the current run only and are not written to Keychain.

## 3. Run and bundle

Run from source:

```sh
cargo run -p skerry-mac
```

Build a macOS `.app` bundle (ad-hoc signed, not notarised):

```sh
scripts/bundle.sh skerry-mac   # produces dist/SealSkin Native.app
```

The GPUI dependency uses `runtime_shaders`, so the Xcode Metal toolchain does not need to be installed to build (see [gpui-notes.md](../gpui-notes.md)).

H.264 is the preferred codec for servers without a saved codec choice. To switch codecs, open **Session Controls → Video → Codec** while connected. SealSkin reconnects to the same session and remembers the choice for this server. **Stream Diagnostics → Client decoder** shows the codec actually in use. SealSkin can use JPEG where the server allows it if H.264 is unavailable or hardware decoding cannot recover. [Codec support and verification](tickets/more-codecs.md)

## 4. Using the app

- **Applications / Running Sessions:** click **Launch** to start a new session or **Resume** to attach to an existing one. Click **Refresh** to reload the list.
- **Session Controls:** press **⌘⇧M**, choose **View → Session Controls**, or click **•••** when it appears in the space beside the picture. The controls open in a separate, movable macOS panel with collapsible **Input**, **Screen Settings**, **Video**, and **Stream Diagnostics** sections. Press **Esc** or **⌘W** to close the panel and return focus to the session.
  - **Mute / Unmute:** controls remote audio when a compatible output is available. Reconnect preserves your mute choice.
  - **Pause / Resume Video:** stops or restarts video delivery while keeping the session open.
  - **Input** has switches for **Command sends Control**, **Remote Cursor**, **Smooth Trackpad Scrolling**, and **Use Mac Input Method**. Smooth Trackpad Scrolling starts enabled. The other switches are opt-in. Changes apply immediately and are saved per server. **Send Key Combination** sends **Alt+Tab** or **Ctrl+Alt+Delete** to the remote desktop. The session controls, quit, and fullscreen shortcuts stay local.
  - **Video** shows the codecs supported by both the Mac and the server. It offers JPEG quality and still-image settings for JPEG, and server-supported CBR target bitrate or CRF quality for hardware video. Frame rate cap applies to either. **Reset Video Settings** restores the server’s tuning defaults and keeps the codec preference.
  - **Screen Settings** opens each session in **Match Window** with **HiDPI** enabled and **UI Scaling** set to **Match Mac Display**. The requested remote resolution and UI scale follow the window when it is resized or moves between displays. The UI scale uses the server’s closest allowed value. For a non-HiDPI scaled mode, SealSkin can derive a fractional UI scale from the panel’s native pixels and the Mac’s logical display size. Use **Reset to Window** after a manual resolution or UI scale change to resume following the window and Mac display. **Scale Locally** fits the picture to the window.
  - **Full Screen:** toggles macOS full-screen mode. You can also press **⌃⌘F**.
  - **Reconnect:** tears down and re-opens the Selkies stream for the current session.
  - **Disconnect:** closes the local stream and returns to the launcher, leaving the remote session running on the server.
- **Keyboard & mouse**. Mouse, scroll wheel, and non-`Cmd` keys are forwarded to the remote application by default. **Use Mac Input Method** keeps composition in macOS and sends the selected text to a supported Selkies server. **Smooth Trackpad Scrolling** uses precise two-finger scroll deltas when the server supports them and falls back to wheel steps. Pinch over the remote picture sends Control-wheel zoom. `Cmd+Q` quits the app.

## 5. Limitations

- **Transport and codecs** are limited to Selkies WebSockets with JPEG or full-frame 8-bit H.264, H.265 (HEVC), and AV1 on Macs with compatible VideoToolbox hardware decoding.
- **Remote resolution** follows the window up to the client’s 4096×2160 limit. The server may further constrain the resolution or lock UI scaling. Stream Diagnostics shows the resolution Selkies delivers.
- **Individual sessions only:** collaboration rooms are not supported in this version.
- **Audio:** requires a macOS output device supporting 48 kHz stereo `f32` audio; otherwise the session runs without audio.
