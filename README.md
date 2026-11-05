# rb

A native macOS browser, written in Rust with [GPUI](https://gpui.rs/), for managing remote browsers together with your agent. It shows no local web content: it mirrors a remote Chromium (a [CloakBrowser Manager](https://github.com/CloakHQ/CloakBrowser-Manager) Profile, or any CDP endpoint) and sends your mouse and keyboard back.

- One window for all Connections, with an Arc/Zen-style vertical Sidebar: pinned Tabs, Tab Groups that can span Connections, and a colour and shape per Connection.
- Mirrors the Active Tab over the Chrome DevTools Protocol (CDP screencast). No audio.
- Local CDP proxy so an external Agent (Playwright, Puppeteer, Playwright MCP, browser-use) can drive the same browser without ever seeing the Manager token.
- JS dialogs are answered in a local modal, popups open as Tabs, permission prompts are denied by default.

See [docs/DESIGN.md](docs/DESIGN.md) for the agreed design, [GLOSSARY.md](GLOSSARY.md) for the vocabulary, [docs/adr/](docs/adr/) for decisions, and [docs/running.md](docs/running.md) for setup.

## Run

```sh
mkdir -p ~/.config/rb && chmod 700 ~/.config/rb
# create ~/.config/rb/config.toml (see docs/running.md), then:
chmod 600 ~/.config/rb/config.toml
cargo run -p rb-app
```

## Build a macOS app

```sh
scripts/bundle.sh        # produces dist/rb.app (ad-hoc signed, not notarised)
```

The GPUI dependency uses the `runtime_shaders` feature, so the Xcode Metal toolchain is not needed to build.

## Layout

| Path | Contents |
|------|----------|
| `crates/cdp` | CDP client, screencast, input, reconnect supervisor, dialogs, Agent proxy, fake CDP server for tests |
| `crates/manager` | CloakBrowser Manager REST client |
| `crates/store` | `config.toml` and `state.toml` models, Tab Group and pin model |
| `crates/app` | The GPUI application |
| `design/` | Static HTML mock of the UI |

## Develop

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Work breakdown and status: [TASK.md](TASK.md).
