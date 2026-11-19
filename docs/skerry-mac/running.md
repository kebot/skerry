# Running Skerry

## 1. What you need

- A Mac. Hardware H.264, H.265 or AV1 decoding is used when available; otherwise Skerry uses JPEG.
- A [SealSkin server](https://docs.sealskin.app/latest/) with a trusted HTTPS certificate.
- An account that allows the **Other** client type for application keys.
- Individual sessions that support the Selkies WebSocket transport and allow a codec the Mac can play.

## 2. Application key

Sign in to your SealSkin server's web app, open **Settings → Application Keys**, and create a key. Copy the whole value, including the server address after `@` (for example `ssk_<id>_<secret>@seal.example`).

Give Skerry the key in one of two ways:

- **In the app:** choose **Paste Application Key**. After it connects, Skerry saves the key in macOS Keychain (service `dev.kebot.sealskin`, account `application-key`). **Remove Connection** in the sidebar deletes it.
- **For development:** set `SEALSKIN_APPLICATION_KEY` in the environment or in `.env` in the working directory. Skerry uses the key for that run only and doesn't save it to Keychain.

```sh
SEALSKIN_APPLICATION_KEY=ssk_id_secret@seal.example
```

## 3. Build and run

Run from source:

```sh
cargo run -p skerry-mac
```

Build an app bundle (ad-hoc signed, not notarised):

```sh
scripts/bundle.sh   # produces dist/SealSkin Native.app
```

GPUI is built with `runtime_shaders`, so you don't need the Xcode Metal toolchain (see [gpui-notes.md](../gpui-notes.md)).

## 4. Using Skerry

### Library

- **Launch** starts a new session for an application.
- **Resume** attaches to a session that's already running.
- **Refresh** reloads both lists.

### Session Controls

Open the controls with **⌘⇧M**, **View → Session Controls**, or the **•••** button when there's room beside the picture. They open in a separate panel you can move. **Esc** or **⌘W** closes it and returns focus to the session.

- **Mute / Unmute** turns remote audio on or off. Reconnecting keeps your choice.
- **Pause / Resume Video** stops or restarts video while keeping the session open.
- **Input** has four switches, saved per server and applied at once:
  - **Command sends Control** (off by default)
  - **Remote Cursor** (off by default)
  - **Smooth Trackpad Scrolling** (on by default)
  - **Use Mac Input Method** (off by default)

  **Send Key Combination** sends **Alt+Tab** or **Ctrl+Alt+Delete**. Shortcuts for Session Controls, quit and full screen always stay on the Mac.
- **Screen Settings:** each session starts in **Match Window** with **HiDPI** on and **UI Scaling** set to **Match Mac Display**. The remote resolution and UI scale follow the window as you resize it or move it to another display, using the closest UI scale the server allows. On scaled, non-HiDPI display modes Skerry works out a fractional UI scale from the panel's native pixels. After changing the resolution or UI scale by hand, **Reset to Window** goes back to following the window. **Scale Locally** fits the picture to the window instead.
- **Video** lists the codecs both the Mac and the server support. JPEG has quality and still-image settings. Hardware codecs have a constant target bitrate or a CRF quality, depending on what the server offers. Frame rate cap applies to all codecs. **Reset Video Settings** restores the server's defaults and keeps your codec.
- **Stream Diagnostics** shows frame rate, bandwidth, decode time, round-trip time, the remote resolution and the decoder in use (**Client decoder**).
- **Full Screen** toggles full screen. **⌃⌘F** does the same.
- **Reconnect** closes and reopens the stream for the same session.
- **Disconnect** closes the stream and returns to the library. The session keeps running on the server.

### Codecs

With no saved choice for a server, Skerry prefers H.264. To switch, open **Session Controls → Video → Codec**. Skerry reconnects to the same session and remembers the codec for that server. If H.264 isn't available or hardware decoding can't recover, Skerry falls back to JPEG when the server allows it. See [Codec support and verification](tickets/more-codecs.md).

### Keyboard, mouse and trackpad

- Mouse, scroll wheel and keys without Command go to the remote application.
- **Use Mac Input Method** composes text in macOS and sends the result, on servers that support it.
- **Smooth Trackpad Scrolling** sends precise two-finger scroll deltas when the server supports them, and wheel steps otherwise.
- Pinching over the picture sends Control-wheel zoom.
- **⌘Q** quits Skerry.

## 5. Limitations

- **Transport:** Selkies WebSockets only. WebRTC isn't supported.
- **Codecs:** JPEG, or full-frame 8-bit H.264, H.265 and AV1 where VideoToolbox can decode them in hardware.
- **Resolution:** follows the window up to 4096×2160. The server can limit it further or lock UI scaling. Stream Diagnostics shows what Selkies actually delivers.
- **Sessions:** individual sessions only, no collaboration rooms.
- **Audio:** needs an output device that supports 48 kHz stereo `f32`. Otherwise the session runs without sound.
