# rb: agreed design

Status: agreed in a design interview; no code written yet. Terms are defined in [GLOSSARY.md](../../GLOSSARY.md).

## Purpose

A native macOS browser, built with Rust and [GPUI](https://gpui.rs/), for managing remote CloakBrowser Profiles together with an external Agent. It shows no local web content: it mirrors a Remote Browser and sends input back. Saving local memory is **not** a goal.

## Architecture

- **Workspace:** `crates/cdp` (protocol client, no GPUI dependency), `crates/manager` (Manager REST client), `crates/store` (config and state files), and `crates/app` (GPUI UI). `tokio` and `tokio-tungstenite` inside `cdp`, bridged to GPUI with channels. Start with `gpui 0.2.2` from crates.io; pin a Zed git revision only if an API is missing.
- **Transport:** CDP only in v1, behind a trait so a video backend (WebRTC) can be added later. Attach per page target in flat mode (`Target.attachToTarget`) via the Manager's browser-level `/api/profiles/{id}/cdp`.
- **Mirroring:** `Page.startScreencast`, JPEG quality 60, acknowledged after decode. Only the Active Tab (global across Connections) streams. Frames are scaled and letterboxed. There is **no** `Emulation.setDeviceMetricsOverride`, to protect the anti-detect fingerprint and to leave the page unchanged for the Agent. Input coordinates are mapped back through the same scale.
- **Input:** mouse move, click, wheel; key down/up and text via `Input.insertText`. Every Cmd-combination is an app shortcut; everything else goes to the Active Tab. Deferred: clipboard, drag and drop, touch, file pickers, right-click menus. No audio.
- **Dialogs and popups:** JS dialogs get a local modal (`Page.handleJavaScriptDialog`). Popups become Tabs. Permission prompts are denied.
- **Resilience:** auto-reconnect with exponential backoff (1 s to 30 s). State is rebuilt from `Target.getTargets`. A stopped Profile shows a clear state and can be started through the Manager API.
- **Connections:** a Manager plus Profile, or a raw CDP URL. The Manager is `CloakHQ/CloakBrowser-Manager` (`Kamasik/…` is a stale fork). The app lists, launches, and stops Profiles; creating and editing Profiles stays in the Manager's web UI. Connections connect lazily on click and stay connected in the background. `auto_connect = true` connects at startup.
- **Agents:** external only. A per-Connection local CDP proxy (off by default, bound to `127.0.0.1`, no Manager token needed by the Agent) lets any CDP-speaking Agent attach, and lets the app mark Tabs the Agent touched. No arbitration, pause, approval gates, or action log in v1.

## UI

- One window for all Connections.
- A collapsible vertical Sidebar: one unified list of Tabs from all Connections, coloured per Connection. Tabs can be freely grouped (flat Tab Groups that may span Connections) and pinned.
- A slim top bar above the Mirror: back, forward, reload, address field.
- Tabs of a disconnected Connection appear as dimmed Placeholder Tabs, persisted so Groups and pins survive. Clicking one connects its Connection.
- Pinned Tabs sit at the top. Spaces, nested folders, split view, and auto-archive are out of scope.
- **Mock design (finished): `design/index.html` is the UI reference.** It resolves Q36 and Q37. The stripe proposal was rejected: it collided with the Group indent line, and colour was its only cue.
  - **Connection identity:** a small coloured dot on the right edge of every Tab, with a distinct shape per Connection (● ◆ ▲ ■) as the non-colour cue. The Connection's initial appears where there is room (chips, address bar, dialogs). The Sidebar background is faintly tinted with the Active Tab's Connection colour. The Active Tab is ringed in its Connection colour.
  - **Tab Groups:** an outlined pill header and indent line, from a reserved palette (violet, olive, stone) that excludes Connection hues. A collapsed Group shows mini markers of the Connections it holds.
  - **Connection status:** a chip row at the bottom of the Sidebar, one compact chip per Connection. The status dot differs by shape and motion: round green is connected, a spinning amber ring is reconnecting, a red square is Profile stopped, and a hollow grey ring is not connected. A ✦ marks an attached Agent. Hover shows a popover with the right action (Connect, Retry now, Start, Disconnect).
  - **New Tab:** a "+ New Tab" row at the top of the Sidebar that shows the target Connection. `Cmd+T` opens in the Active Tab's Connection. Chips also show a "+" on hover.
  - **Other states in the mock:** Placeholder Tabs (dimmed, italic), Agent marker (static "touched earlier" and pulsing "acting now"), a reconnecting Connection, a stopped Profile with a Start button, the Sidebar collapsed to an icon rail, and a JS dialog drawn locally over the Mirror.
  - **Still open (provisional defaults until the product owner answers):** (1) `Cmd+T` when the Active Tab's Connection is disconnected: connect it, but do not auto-start a stopped Profile. (2) Clicking a Placeholder Tab connects its Connection and makes that Tab Active. (3) More than about 8 Connections: overflow into a menu. (4) Group colours are assigned from the palette. (5) One Agent marker in v1. (6) Config validation warns about duplicate colours and initials, and does not fail.

## Config and state

- `~/.config/rb/config.toml`, mode `0600` (the app warns if looser): `[[connection]]` entries with `name`, `manager_url` + `profile_id` + `token`, or a raw `cdp_url`; optional `color`, `auto_connect`. Tokens live only here and are redacted in logs. No keychain.
- `~/.config/rb/state.toml`: local-only Tab Groups, pins, and Placeholder Tab data (last-known title and URL). Group assignments match by target ID, then by exact URL. The Remote Browser never sees them.

## Testing

A fake CDP server in `crates/cdp` tests (tab tracking, frame ack, reconnect, input mapping). Real-Chromium smoke tests later. No UI pixel tests.

## Manager facts (verified from source)

- Auth is one optional static `AUTH_TOKEN`: `Authorization: Bearer` header or `auth_token` cookie. If unset, everything is open.
- `/api/profiles/{id}/cdp` is a browser-level CDP WebSocket proxy. It does not limit the number of clients; each client gets its own upstream connection. `Browser.setDownloadBehavior` is browser-wide, last writer wins.
- Lifecycle API: `GET /api/profiles`, `POST /api/profiles/{id}/launch` (409 if running), `POST …/stop`, `GET …/status`.
- The browser is headed, on a per-profile KasmVNC X display. The Manager's own web UI is a VNC viewer, not a CDP client. There is no MCP or agent feature.

## To verify while building

- That Chromium accepts concurrent CDP sessions (UI plus Agent) on the same page in practice.
- That the minimal GPUI window builds on this machine's Xcode and Metal toolchain.

## ADRs to write when building starts

1. CDP screencast over VNC/WebRTC for mirroring (costs audio; gains a native UI and lightweight client).
2. Scale and letterbox Frames instead of overriding the viewport (protects the fingerprint; costs some sharpness).
3. Local CDP proxy as the Agent endpoint (token-free Agents and the Tab marker; costs proxying CDP traffic).

## Milestones

1. Walking skeleton: read config, connect to one Profile, list Tabs, show Frames, forward mouse and keyboard.
2. Tab Groups and pinning.
3. Reconnect.
4. JS dialogs and popups.
5. Multi-Connection Sidebar and the Agent proxy, after the mock design.
