# rb

A native macOS browser, written in Rust with [GPUI](https://gpui.rs/), for managing remote browsers together with your agent. It shows no local web content: it mirrors a remote Chromium (a [CloakBrowser Manager](https://github.com/CloakHQ/CloakBrowser-Manager) Profile, or any CDP endpoint) and sends your mouse and keyboard back.

- One window for all Connections, with an Arc/Zen-style vertical Sidebar: pinned Tabs, Tab Groups that can span Connections, and a colour and shape per Connection.
- Mirrors the Active Tab over the Chrome DevTools Protocol (CDP screencast). No audio.
- Local CDP proxy so an external Agent (Playwright, Puppeteer, Playwright MCP, browser-use) can drive the same browser without ever seeing the Manager token (off by default; Tabs the Agent touched get a violet ✦ marker).
- JS dialogs are answered in a local modal, popups open as Tabs, permission prompts are denied by default.

See [docs/rb/DESIGN.md](docs/rb/DESIGN.md) for the agreed design, [GLOSSARY.md](GLOSSARY.md) for the vocabulary, [docs/rb/adr/](docs/rb/adr/) for decisions, [docs/rb/running.md](docs/rb/running.md) for setup, and [docs/gpui-notes.md](docs/gpui-notes.md) for shared GPUI notes.

## Run

```sh
mkdir -p ~/.config/rb && chmod 700 ~/.config/rb
# create ~/.config/rb/config.toml (see docs/rb/running.md), then:
chmod 600 ~/.config/rb/config.toml
cargo run -p rb-app
```

## Build a macOS app

```sh
scripts/bundle.sh        # produces dist/rb.app (ad-hoc signed, not notarised)
```

The GPUI dependency uses the `runtime_shaders` feature, so the Xcode Metal toolchain is not needed to build.

## SealSkin Native for macOS

SealSkin Native is a Rust and GPUI app for launching and resuming applications on a [SealSkin server](https://docs.sealskin.app/latest/). It connects directly to the Selkies WebSocket, decodes JPEG video in Rust, and draws it in GPUI. Keyboard and mouse events go directly to Selkies. Stereo Opus audio plays through Core Audio on a compatible output device. Disconnect returns to the launcher and leaves the remote session running.

Sign in to your server’s web app, create an application key in **Settings → Application Keys**, and copy the full value including the server address after `@`. In the Mac app, choose **Paste Application Key**. The app saves the key in macOS Keychain after connecting. The server must have a trusted HTTPS certificate and allow the **Other** client type.

For development, the app also reads `SEALSKIN_APPLICATION_KEY` from the environment or from `.env` in the working directory. Keys supplied this way are used for the run and aren’t written to Keychain.

This version supports individual sessions with the Selkies WebSocket transport and JPEG enabled. It requests 1280×720 and scales the remote picture to the local window. WebRTC, other video codecs, and collaboration rooms are outside this version’s scope.

See [docs/sealskin-native/DESIGN.md](docs/sealskin-native/DESIGN.md) for the architecture and [docs/sealskin-native/running.md](docs/sealskin-native/running.md) for setup, credentials, and usage.

```sh
cargo run -p sealskin-native-app
scripts/bundle.sh sealskin-native   # produces dist/SealSkin Native.app
```

## Layout

| Path | Contents |
|------|----------|
| `crates/cdp` | CDP client, screencast, input, reconnect supervisor, dialogs, Agent proxy, fake CDP server for tests |
| `crates/manager` | CloakBrowser Manager REST client |
| `crates/store` | `config.toml` and `state.toml` models, Tab Group and pin model |
| `crates/app` | The rb GPUI application |
| `crates/sealskin-native` | The SealSkin launcher and API client, Selkies transport, GPUI desktop view, and Core Audio playback |
| `design/` | Static HTML mock of the UI |

## Develop

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Work breakdown and status: [TASK.md](TASK.md).
