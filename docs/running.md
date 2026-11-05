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

## Known limitations

- One Connection per run (first, or the one you name). No Groups, pins,
  Placeholder Tabs, dialogs (a JS `alert` blocks the page until answered
  elsewhere), popups as Tabs, or Agent proxy yet.
- The address field is minimal: type, backspace, Cmd+V, Cmd+A; no cursor
  movement or selection. Back/forward are not disabled at the ends of history.
- Typed CJK composition does not work in the Mirror (see docs/gpui-notes.md).
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
