# Running rb against a CloakBrowser Manager

## 1. Create the config

rb reads `~/.config/rb/config.toml` (override with `RB_CONFIG=/path/to/config.toml`).
It holds tokens, so keep it private:

```sh
mkdir -p ~/.config/rb
cat > ~/.config/rb/config.toml <<'TOML'
[[connection]]
name = "work"
manager_url = "https://manager.example.com"   # base URL, http or https
profile_id = "your-profile-id"
token = "your-manager-token"
TOML
chmod 600 ~/.config/rb/config.toml
```

If the file is readable by group or others, rb still starts and shows a
warning in the status bar. A missing file shows a first-run screen with this
example. A raw CDP endpoint works too: `cdp_url = "ws://host:9222/devtools/browser/..."`
(optional `token`).

## 2. Run

```sh
cargo run -p rb-app                         # first Connection in the file
cargo run -p rb-app -- --connection work    # or: RB_CONNECTION=work cargo run -p rb-app
```

Without a browser you can try the whole UI against the fake CDP server:
`cargo run -p rb-app --example skeleton_demo` (env `RB_DEMO_OUTAGE=15:8` drops
the connection at 15 s and refuses reconnects for 8 s).

What you get: Sidebar with the Connection's page Tabs (click one to make it the
Active Tab; only the Active Tab streams; `x` closes, `+ New Tab` opens a blank
one), address field (Enter navigates; `example.com` becomes `https://example.com`,
text with spaces becomes a search), back / forward / reload, the Mirror (click it
to focus; mouse, wheel and keys are forwarded; Cmd combinations stay with the
app), Cmd+B toggles the Sidebar, Cmd+Q quits. A stopped Profile shows "Profile
not running" with a Start button; a lost connection shows "Reconnecting..." over
the Mirror and recovers by itself; a bad token shows an auth error.

## Several Connections

Add one [[connection]] block per Remote Browser (name, manager_url +
profile_id + token, or cdp_url; optional color = "#rrggbb" and
auto_connect = true). All Connections share one Sidebar: each Tab has a
dot in its Connection's colour and shape. Connections connect lazily (click
the chip, a Placeholder Tab, or "+ New Tab" for it) and stay connected in
the background; auto_connect = true, --connection <name>, or a config with a
single Connection connects at startup. Only the Active Tab (one across all
Connections) streams. Chip row at the bottom of the Sidebar: round green =
connected, spinning amber ring = reconnecting, red square = Profile
stopped (Manager Connections; click or use the hover popover to Start),
red ring = failed (bad token, Manager unreachable; Retry), hollow grey =
not connected. More than 8 Connections overflow into a "+N" menu. Cmd+T opens
a blank Tab in the Active Tab's Connection (connecting it first if it is
down; a stopped Profile is never started automatically). Duplicate
Connection colours or initials show a warning in the status bar.

JS dialogs (alert, confirm, prompt, beforeunload) show a local modal over
the Mirror for the Active Tab; Enter = OK / Leave, Escape = Cancel / Stay.
Background Tabs with a dialog get an amber "!" in the Sidebar. A popup opens
as a Tab next to its opener, and becomes Active only if its opener was the
Active Tab.

Try it without browsers: cargo run -p rb-app --example multi_demo (three fake
Connections; control file path is printed, e.g. `echo "kill alpha 15" >>
<dir>/cmd`, `dialog beta B1 confirm Sure?`, `popup alpha A1 Popup`).

## Agents

An external Agent (Playwright, Puppeteer, Playwright MCP, browser-use) can
drive the same Remote Browser through a local proxy that rb runs per
Connection. It is **off by default**: nothing listens unless you enable it.

```toml
[[connection]]
name = "work"
# ...manager_url / profile_id / token, or cdp_url...
agent_proxy = true               # default false
agent_proxy_port = 9333          # optional; default 0 = a free port each run
agent_proxy_secret = "my-secret-1234"  # optional; default: random each run
```

Secret: at least 8 characters of `A-Z a-z 0-9 - _`. If it is not set, rb
generates a random one every run (never written to the config). Set a fixed
port and secret if the Agent needs a stable endpoint.

The proxy starts when the Connection is connected and stops on Disconnect
and quit. After a reconnect it restarts on the same port and secret and the
Agent markers are cleared (target ids can change when the browser
restarts). A port that is already in use shows a warning in the status bar
and the Connection keeps working without an Agent endpoint. You can also
turn it on or off at runtime: hover the Connection chip and use "Enable
Agent endpoint" / "Disable" (a runtime choice is not saved to the config).

The chip popover shows the endpoint, `http://127.0.0.1:PORT/SECRET`, a
"Copy endpoint" button and the two usual ways to use it:

    # Playwright (Python): browser = p.chromium.connect_over_cdp("<endpoint>")
    # Playwright MCP:      npx @playwright/mcp --cdp-endpoint <endpoint>

Puppeteer: use `ws://127.0.0.1:PORT/SECRET/devtools/browser/rb` as
`browserWSEndpoint`.

Agent marker: a violet ✦ on the Connection chip while at least one Agent
client is attached, and on every Tab (Sidebar row, pinned tile, rail) the
Agent sent commands to. It pulses while the Agent acted within the last 2 s
and stays static after that ("touched earlier"). When the Agent disconnects
the chip marker goes, the Tab marks stay until the Connection reconnects or
the proxy is disabled.

Security notes: it listens on 127.0.0.1 only; requests with an `Origin`
header (any web page in a browser) are refused with 403, and so are Host
headers other than 127.0.0.1 / localhost / ::1; the random path secret keeps
other local users and processes from guessing the endpoint (any local process
that knows the URL can drive the browser, so treat it like a token). The
Manager token is added by rb on its own upstream connection and is never
sent to, or visible to, the Agent. There is **no arbitration in v1**: you and
the Agent control the same Tabs at the same time, and an Agent that changes
emulation (`Emulation.set*Override`) changes what you see.

Without a browser, `cargo run -p rb-app --example multi_demo` runs `alpha`
with `agent_proxy = true` on port 39333 and plays the Agent on command
(`echo "agent attach" >> <dir>/cmd`, `agent nav A2 https://example.com/`,
`agent detach`).

## Known limitations

- One Connection per run (first, or the one you name). No Groups, pins,
  Placeholder Tabs, dialogs (a JS `alert` blocks the page until answered
  elsewhere), popups as Tabs, or Agent proxy yet.
- The address field is minimal: type, backspace, Cmd+V, Cmd+A; no cursor
  movement or selection. Back/forward are not disabled at the ends of history.
- Typed CJK composition does not work in the Mirror (see [gpui-notes.md](../gpui-notes.md)).
- Frames are scaled, the remote viewport is not resized (ADR 0002).
- Backoff can leave "Reconnecting..." showing for up to 30 s after the server
  is back; the Connection retries on its own schedule.
- A macOS window that is hidden behind others may stop repainting until it is
  shown again.

## First real run: what to check

1. `token` accepted by both the Manager REST API and the CDP WebSocket
   (`Authorization: Bearer`); a wrong one must show "Authentication failed".
2. Stop the Profile in the Manager: rb must show "Profile not running"; Start
   must launch it (HTTP 409 means already running and is fine) and then connect.
3. Does the Manager's CDP proxy pass `Target.setDiscoverTargets`,
   `Target.getTargets`, `Target.createTarget` / `closeTarget`,
   `Page.getNavigationHistory` and `Page.navigateToHistoryEntry` (back/forward)?
   Failures show in the status bar.
4. Concurrent CDP sessions: with another CDP client (for example Playwright or
   `chrome://inspect`) attached to the same page, do both keep working?
5. Does a page with `deviceWidth` different from the window letterbox correctly
   and do clicks land where the pointer is (watch the first few clicks)?
6. Wheel direction on a real trackpad; Frame rate and memory over a few minutes.
7. Kill the Manager or the network: "Reconnecting..." appears, and the same Tab
   is mirrored again afterwards.
