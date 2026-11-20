# Skerry

Skerry is a native Mac client for [SealSkin](https://docs.sealskin.app/latest/) and [Selkies](https://github.com/selkies-project/selkies). It lists the applications and running sessions on a SealSkin server, launches or resumes one, and streams it into a Mac window over the Selkies WebSocket transport. It is written in Rust with [GPUI](https://gpui.rs/).

- **Video:** H.264, H.265 (HEVC) and AV1 decoded in hardware with VideoToolbox, or JPEG decoded in Rust.
- **Audio:** stereo Opus played through Core Audio.
- **Input:** keyboard, mouse, smooth trackpad scrolling, pinch to zoom, and optional Mac input method composition.
- **Screen:** the remote desktop follows the window size, Retina scaling and the Mac display's UI scale.
- **Session Controls:** a separate panel (⌘⇧M) for playback, screen, video codec and quality settings, and stream diagnostics.

Disconnecting returns to the launcher and leaves the remote session running.

> The window title, bundle name (`SealSkin Native.app`), Keychain service and Application Support folder still use the old SealSkin Native name. They will change when the app itself is renamed.

## Quick start

1. Sign in to your SealSkin server's web app, open **Settings → Application Keys**, and create a key. Copy the whole value, including the server address after `@`.
2. Run Skerry and choose **Paste Application Key**. Skerry saves the key in macOS Keychain after it connects.

The server needs a trusted HTTPS certificate and must allow the **Other** client type.

For development, Skerry also reads `SEALSKIN_APPLICATION_KEY` from the environment or from `.env` in the working directory. A key supplied this way is used for that run only and is not saved to Keychain.

```sh
cargo run -p skerry-mac
scripts/bundle.sh   # produces dist/SealSkin Native.app (ad-hoc signed, not notarised)
```

GPUI is built with the `runtime_shaders` feature, so you don't need the Xcode Metal toolchain.

## Layout

| Path | Contents |
|------|----------|
| `crates/skerry-mac` | The Mac app: SealSkin API client, Selkies transport, video decoding, GPUI views, Core Audio playback |
| `design/skerry-mac/` | Static HTML mock-up of the UI |
| `.agents/prose/` | OpenProse contracts (`src/`), architecture decisions (`architecture/`), and agent-managed specs, roadmap and tickets (`specs/`) |
| `scripts/bundle.sh` | Builds a release `.app` bundle in `dist/` |

## Develop

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```
