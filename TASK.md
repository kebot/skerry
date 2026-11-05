# TASK.md: rb work breakdown

Product manager's plan. Source of truth for scope: [docs/DESIGN.md](docs/DESIGN.md). Vocabulary: [GLOSSARY.md](GLOSSARY.md). UI reference: `design/` (static HTML mock, in progress).

## Rules for every task

- Stay inside the crate/paths the task names. Do not edit other tasks' files. If you need a change elsewhere, say so in your report.
- Use GLOSSARY.md terms in names and docs (Connection, Tab, Tab Group, Placeholder Tab, Active Tab, Mirror, Frame, Agent, Manager, Profile).
- Test without a real browser: use the fake CDP server (T1) or local fake HTTP servers. No UI pixel tests.
- Never log tokens. Do not commit to git. Do not add dependencies beyond what the task needs.
- Done means: `cargo test -p <crate>` and `cargo clippy -p <crate> -- -D warnings` pass, and the report lists what was built, how it was verified, and open issues.

## Workspace

```
crates/cdp      CDP client, flat sessions, screencast, input, fake CDP server (no GPUI)
crates/manager  CloakBrowser Manager REST client (list/launch/stop/status)
crates/store    config.toml and state.toml models (no GPUI)
crates/app      GPUI application
```

## Milestone 1: walking skeleton

| ID | Task | Crate / paths | Depends on | Status |
|----|------|---------------|------------|--------|
| T1 | CDP core: WebSocket connect (Bearer header), request/response matching, events, flat `Target.attachToTarget` sessions, `Target.getTargets` + target events, plus a reusable fake CDP server for tests | `crates/cdp` | none | done |
| T2 | Config and state: parse `config.toml` (`[[connection]]`, tokens, `color`, `auto_connect`), `0600` permission check, token redaction in `Debug`/logs; `state.toml` load/save for Groups, pins, Placeholder Tabs | `crates/store` | none | done |
| T3 | Manager client: `GET /api/profiles`, `POST …/launch` (handle 409, 402/403), `POST …/stop`, `GET …/status`, Bearer auth, fake HTTP server tests | `crates/manager` | none | done |
| T4 | GPUI hello-window: minimal window with `gpui 0.2.2`, verify it builds and runs on this Mac, note any toolchain steps | `crates/app` | none | done |
| T5 | Write ADRs 0001, 0002, 0003 in `docs/adr/` | `docs/adr/` | none | done |
| T6 | Screencast + input in `cdp`: `Page.startScreencast` (JPEG q60, ack after decode), Frame stream API; mouse move/click/wheel, key down/up, `Input.insertText`; coordinate mapping helper for scale and letterbox (no viewport override) | `crates/cdp` | T1 | done |
| T7 | Mirror view in `app`: draw Frames scaled and letterboxed, capture mouse and keyboard, map to CDP input; Cmd-combinations stay app shortcuts | `crates/app` | T4, T6 | done |
| T8 | Skeleton wiring: read `config.toml`, connect to one Connection, list Tabs in a basic Sidebar, set the Active Tab, show its Frames, forward input | `crates/app` | T2, T3, T6, T7 | done |

## Milestone 2: Tab Groups and pinning

| ID | Task | Depends on |
|----|------|------------|
| T9 | Group/pin model in `store`: flat Groups (name, colour, collapsed), pins, ordering, matching by target ID then exact URL | T2 |
| T10 | Sidebar UI: Tab list, Groups, pinning, drag to reorder and group, per the mock | T8, T9, mock |

## Milestone 3: reconnect and Placeholder Tabs

| ID | Task | Depends on |
|----|------|------------|
| T11 | Reconnect in `cdp`: exponential backoff 1–30 s, rebuild from `Target.getTargets`, restart screencast | T1, T6 |
| T12 | Placeholder Tabs: persist last-known title and URL, dimmed entries for disconnected Connections, connect on click | T9, T10, T11 |
| T13 | Stopped-Profile state: detect not-running, offer Start via the Manager client | T3, T11 |

## Milestone 4: dialogs and popups

| ID | Task | Depends on |
|----|------|------------|
| T14 | JS dialogs: `Page.javascriptDialogOpening` to local modal, `Page.handleJavaScriptDialog`; popups become Tabs; permission prompts denied | T8 |

## Milestone 5: multi-Connection and Agent

| ID | Task | Depends on |
|----|------|------------|
| T15 | Multi-Connection in one window: lazy connect, stay connected in background, `auto_connect`, Connection colour per Tab, status UI (per the mock) | T10, T12, mock |
| T16 | Agent CDP proxy: per-Connection local proxy on `127.0.0.1`, off by default, injects the Manager token, tags Tabs the Agent touched; fake-CDP tests | T1, T15 |
| T17 | Agent marker UI on Tabs and Connection chips | T16, mock |

## Open (product decisions)

- Q36 and Q37 (Connection vs Group colour, and the Connection status / new-Tab location) resolve in the mock at `design/`. Tasks marked "mock" wait for that decision.
- Verify during T1/T16: Chromium accepts concurrent CDP sessions (UI plus Agent) on the same page.

## Dispatch log

Wave 1 (parallel, separate crates): T1, T2, T3, T4, T5. All done; `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` pass.
Wave 2 (parallel, separate crates): T6 (`cdp`), T9 (`store`). T6 done (`cdp`: screencast with consumer-controlled ack, mouse/key/insertText input, `FrameFit` letterbox mapping; open: `offset_top` unused, check on a real browser). T9 done (47 `rb-store` tests pass; open: Tabs have no stable id yet, Groups always render before ungrouped Tabs). Wave 3 (parallel): T7 (`app`), T11 (`cdp`). T11 done (`Connection` supervisor: backoff 1–30 s, watchable state, `NotRunning` / `Failed` not retried, state and screencast rebuilt after reconnect; open: 4004/403 mappings unverified against a real Manager, `next_retry_in` is not a live countdown). T7 done (`app`: `Mirror` view with letterboxed Frames, input mapping, `MirrorBackend`, `examples/mirror_demo.rs`; measured: explicit `drop_image` per Frame is required, RSS flat at about 120 MB versus about 10 MB/s growth without it; open: CJK IME composition, wheel direction not verified on real hardware). Mock design finished (`design/index.html`, recorded in DESIGN.md). Wave 4: T8 (`app`) done (config -> Connection -> Sidebar Tabs -> Mirror -> input, address bar, new/close Tab, reconnect overlay, first-run screen, `docs/running.md`; verified only against the fake CDP server; open: untested against a real Manager, address field has no cursor movement, hidden macOS windows stop repainting). Wave 5 (parallel): T10 (`app` + `store`), T16 (`cdp`). T16 done (`AgentProxy`: 127.0.0.1 only, Origin/Host checks, optional path secret, token injected upstream, `AgentState` activity tracking; open: no real Playwright/Puppeteer run yet, concurrent sessions on one page unverified). T14a done (`cdp`: dialogs observed on all page Tabs via per-Tab observer sessions, `Connection::pending_dialogs` / `handle_dialog`, popups as Tabs with `opener_id`, default-deny permission policy; open: not tested on real Chromium, late denial race). T14b (app modal) is next after T10. T10 done (`store`: stable Tab ids; `app`: Sidebar per the mock with pins, Groups, drag and drop, context menu, chips and status dots, rail, shortcuts, persisted `state.toml`, Placeholder Tabs; open: Group rename editor has no cursor movement, Cmd+1..9 skips collapsed Groups, several keyboard paths verified only by unit tests). T14b, T15, T12, T13 done (`app`: one `rb_cdp::Connection` per configured Connection, lazy connect, single app-wide screencast, chip row with overflow menu, Profile stopped + Start, Placeholder Tab click-to-connect, dialog modal, popups as Tabs next to their opener; verified in a real window with three fake servers; open: prompt/beforeunload, popup path and overflow menu verified by unit tests only, nothing tried against a real Manager). T17 done (store: `agent_proxy`, `agent_proxy_port`, `agent_proxy_secret` per Connection; app: proxy lifecycle per Connection, endpoint + Copy in the chip popover, one ✦ marker on Tabs, pins, rail and chip; verified in a real window and with real Playwright against headless Chromium: connect, goto, newPage, concurrent session with the Mirror, 404 without secret, 403 with Origin; open: runtime enable/disable not saved to config, popover wide with an endpoint shown, Puppeteer/Playwright MCP/browser-use not tried). Agents must NOT edit TASK.md; the PM updates status cells (concurrent edits overwrote a cell in Wave 1).
