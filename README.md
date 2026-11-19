# SealSkin Native for macOS

SealSkin Native is a Rust and GPUI app for launching and resuming applications on a [SealSkin server](https://docs.sealskin.app/latest/). It connects directly to the Selkies WebSocket, decodes JPEG video in Rust, and draws it in GPUI. Keyboard and mouse events go directly to Selkies. Stereo Opus audio plays through Core Audio on a compatible output device. Disconnect returns to the launcher and leaves the remote session running.

Sign in to your server’s web app, create an application key in **Settings → Application Keys**, and copy the full value including the server address after `@`. In the Mac app, choose **Paste Application Key**. The app saves the key in macOS Keychain after connecting. The server must have a trusted HTTPS certificate and allow the **Other** client type.

For development, the app also reads `SEALSKIN_APPLICATION_KEY` from the environment or from `.env` in the working directory. Keys supplied this way are used for the run and aren’t written to Keychain.

This version supports individual sessions with the Selkies WebSocket transport and JPEG enabled. It requests 1280×720 and scales the remote picture to the local window. WebRTC, other video codecs, and collaboration rooms are outside this version’s scope.

See [docs/sealskin-native/DESIGN.md](docs/sealskin-native/DESIGN.md) for the architecture and [docs/sealskin-native/running.md](docs/sealskin-native/running.md) for setup, credentials, and usage. Shared GPUI notes are in [docs/gpui-notes.md](docs/gpui-notes.md).

The GPUI dependency uses the `runtime_shaders` feature, so the Xcode Metal toolchain is not needed to build.

```sh
cargo run -p sealskin-native-app
scripts/bundle.sh sealskin-native   # produces dist/SealSkin Native.app
```

## Layout

| Path | Contents |
|------|----------|
| `crates/sealskin-native` | The SealSkin launcher and API client, Selkies transport, GPUI desktop view, and Core Audio playback |
| `design/sealskin-native/` | Static HTML mock of the UI |
| `docs/` | Design notes, tickets, and shared GPUI notes |

## Develop

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```
