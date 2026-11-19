# Running SealSkin Native

## 1. Prerequisites

- A [SealSkin server](https://docs.sealskin.app/latest/) with a trusted HTTPS certificate.
- Your account must allow the **Other** client type for application keys.
- Individual sessions must support the Selkies WebSocket transport with JPEG enabled, or H.264 enabled when running the hardware decoding prototype.

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
cargo run -p sealskin-native-app
```

Build a macOS `.app` bundle (ad-hoc signed, not notarised):

```sh
scripts/bundle.sh sealskin-native   # produces dist/SealSkin Native.app
```

The GPUI dependency uses `runtime_shaders`, so the Xcode Metal toolchain does not need to be installed to build (see [gpui-notes.md](../gpui-notes.md)).

JPEG is the default. To try H.264 hardware decoding, start the app with this setting and open a session that allows `h264enc`.

```sh
SEALSKIN_VIDEO_CODEC=h264 'dist/SealSkin Native.app/Contents/MacOS/SealSkin Native'
```

Check **Client decoder** in **Stream Diagnostics** to see what the Mac is decoding. The client requests JPEG if H.264 is unavailable or cannot recover and the server allows JPEG. [Benchmark and verification](tickets/h264-video.md)

## 4. Using the app

- **Applications / Running Sessions:** click **Launch** to start a new session or **Resume** to attach to an existing one. Click **Refresh** to reload the list.
- **Session Controls:** press **⌘⇧M**, choose **View → Session Controls**, or click **•••** when it appears in the space beside the picture. The controls open in a separate, movable macOS panel with collapsible **Screen Settings**, **Video**, and **Stream Diagnostics** sections. Press **Esc** or **⌘W** to close the panel and return focus to the session.
  - **Mute / Unmute:** controls remote audio when a compatible output is available. Reconnect preserves your mute choice.
  - **Pause / Resume Video:** stops or restarts video delivery while keeping the session open.
  - **Video:** shows stream diagnostics and server-supported JPEG controls. Changes are saved per server. **Reset Video Settings** restores the defaults announced when the session opened.
  - **Screen Settings** has resolution presets, manual width and height, window matching, and HiDPI. **Scale Locally** fits the picture to the window. UI scaling and forced alignment depend on what the server advertises. Diagnostics show the resolution Selkies delivers.
  - **Full Screen:** toggles macOS full-screen mode. You can also press **⌃⌘F**.
  - **Reconnect:** tears down and re-opens the Selkies stream for the current session.
  - **Disconnect:** closes the local stream and returns to the launcher, leaving the remote session running on the server.
- **Keyboard & mouse:** mouse, scroll wheel, and non-`Cmd` keys are forwarded to the remote application. `Cmd+Q` quits the app.

## 5. Limitations

- **Transport and codecs** are limited to Selkies WebSockets with JPEG or opt-in, full-frame 8-bit H.264 on Macs with VideoToolbox hardware decode.
- **Initial size:** the client requests `1280×720`. The server can constrain later resize requests. The client scales and letterboxes the received picture to fit the available area.
- **Individual sessions only:** collaboration rooms are not supported in this version.
- **Audio:** requires a macOS output device supporting 48 kHz stereo `f32` audio; otherwise the session runs without audio.
