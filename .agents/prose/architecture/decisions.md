# Architecture Decisions & Constraint Lattice

## ARCH-001: Attached OpenProse Root & Harness Selection

- **Status:** settled
- **Mode:** bootstrap (`prose init`)
- **Decision:** Establish OpenProse as an attached repository root at `.agents/prose/` using the default `filesystem` state backend and in-session Prose Complete agent harness.
- **Rationale:** Skerry is an existing Rust/GPUI workspace (`Cargo.toml`, `crates/skerry-mac`). Using `.agents/prose/` keeps the Cargo workspace root clean while versioning contracts and architecture records in Git.
- **Affected Contracts:** `src/skerry/index.prose.md`

## ARCH-002: Native OpenProse Responsibility Package Replaces Legacy `docs/` & Tickets

- **Status:** settled
- **Decision:** Convert all legacy markdown documentation (`DESIGN.md`, `gpui-notes.md`, `TODO.md`, `running.md`, `milestone-1-qa.md`, and all 28 ticket files) directly into native OpenProse contracts under `.agents/prose/src/skerry/` and decision records in `.agents/prose/architecture/decisions.md`, deleting `docs/` and `.agents/prose/specs/`.
- **Rationale:** Following `std/ops/compose` invariants ("The directory package is the canonical persisted program. The working architecture proposes changes to it; it does not remain a parallel truth"), completed features become settled `### Maintains` clauses and postconditions, while unbuilt features become explicit `<!-- compose:unresolved id=ARCH-GAP-... -->` frontier markers on their owning domain responsibilities.
- **Affected Contracts:**
  - `src/skerry/index.prose.md`
  - `src/skerry/video-and-screen.prose.md`
  - `src/skerry/launcher-and-sessions.prose.md`
  - `src/skerry/desktop-and-collaboration.prose.md`
  - `src/skerry/implement-feature.prose.md`
  - `src/skerry/skerry.test.prose.md`

## ARCH-003: Settled Video & Screen Baseline (Milestone 1 + Hardware Codecs)

- **Status:** settled
- **Decision:** Video & Screen maintains server-capability intersection (`settings.rs`), JPEG + hardware VideoToolbox H.264/H.265/AV1 decoding (`video.rs`, painted via zero-copy `window.paint_surface`), JPEG `RenderImage` retirement via `window.drop_image`, window/Retina/UI-scale resolution negotiation (`display.rs`), explicit video pause and server-acknowledged audio mute (`audio.rs`), and 1-second Stream Diagnostics in Session Controls (`app/controls.rs`).
- **Evidence:** Verified on SealSkin 0.4.2 disposable Zen sessions across JPEG, H.264, H.265, and AV1 at 1280x720 and Match Retina 1966x2160 on Apple M5 Pro, plus automated unit tests, Clippy, and formatting checks.
- **Affected Contracts:** `src/skerry/video-and-screen.prose.md`

## ARCH-004: Settled Input, Trackpad Gestures & Local IME Baseline

- **Status:** settled
- **Decision:** Desktop Integration maintains per-server Input controls (Command sends Control, Remote Cursor, Smooth Trackpad Scrolling, Use Mac Input Method, Send Key Combination), AppKit magnification pinch-to-zoom (`view/pinch.rs`), automatic key/modifier/button release on focus loss, and local GPUI marked-text IME composition committed via `co,end,<text>`. Raw multitouch contact forwarding remains deferred (`ARCH-GAP-DC-08`) due to high Chrome latency observed in prototype testing.
- **Affected Contracts:** `src/skerry/desktop-and-collaboration.prose.md`

## ARCH-005: Product & Protocol Boundaries (SealSkin 0.4.2 & Selkies 715df7c)

- **Status:** settled — SealSkin 0.4.2 version baseline superseded by `ARCH-008`, itself superseded by `ARCH-009` (the product/protocol boundaries below remain in force)
- **Decision:**
  - **Operator vs. Client Settings:** Capture backends, GPU selection, TLS, TURN credentials, audit/recording policy, and server administration (users, groups, app stores, templates, cluster nodes, audit logs) remain operator/web-dashboard responsibilities. Skerry only requests session changes permitted by server policy.
  - **Control-Plane vs. In-Session:** SealSkin companion API owns application keys, catalog, launching, persistent homes/shares, and stopping sessions. Selkies WebSocket owns live stream settings, input, clipboard, in-session file transfer, printing, and media uplinks.
  - **Native Presentation:** Skerry uses native macOS windows, menus, and Session Controls (`cmd-shift-m`). Browser-specific layouts (Sidebar/Wish/Edge), CSS translucency, browser extensions, and touch-only overlays are excluded.
- **Affected Contracts:** `src/skerry/index.prose.md`, `src/skerry/video-and-screen.prose.md`, `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/desktop-and-collaboration.prose.md`

## ARCH-006: gpui-kit UI Design System & Root-Wrapped Window Ownership

- **Status:** settled
- **Decision:** Migrate `crates/skerry-mac` from raw `gpui` (`0.2.2`) to `gpui-kit` (`0.10.0`) (`gpui_kit::init(cx)`, `gpui_kit::assets::Assets`, `Theme::sync_system_appearance`, and `gpui_kit::component` primitives: `Button`, `Input`/`InputState`, `Switch`, `Label`, `Collapsible`, `IconName`, `ScrollableElement`). Because `gpui_kit::open_window` mounts `gpui_kit::base::Root` as each window's root view:
  - Cross-window references (e.g., `ControlPanel`'s `Owner`) must pair `AnyWindowHandle` with the target `Entity<Launcher>` instead of calling `window.window_handle().downcast::<Launcher>()`.
  - Child windows whose initial render reads `Launcher` state (such as `ControlPanel::open` invoked from `Launcher::toggle_controls`) must be opened inside `cx.defer(...)` after `Launcher`'s mutable entity lease ends, preventing re-entrant borrow panics when `window.activate_window()` triggers a synchronous render.
- **Rationale:** Adopts a unified, system-appearance-synced component library instead of custom buttons, inputs, switches, and collapsibles, while honoring `gpui-kit`'s `Root` wrapper and GPUI's single-writer entity lease rules.
- **Affected Contracts:** `src/skerry/index.prose.md`

## ARCH-007: Pure `skerry-ui` Screen Library & Interactive Preview Workbench

- **Status:** settled
- **Decision:** Extract all of Skerry's GPUI screens, overlays, and shared UI primitives into a dedicated workspace crate `crates/skerry-ui` (`components.rs`, `connect.rs`, `library.rs`, `controls.rs`, `session.rs`) that depends only on `gpui-kit = "0.10.0"`, with `crates/skerry-mac` delegating its Connect, Library, Session Controls panel, and Desktop overlays (status banner, IME preedit, controls trigger, waiting placeholder) to `skerry-ui`. Provide a standalone `examples/preview.rs` workbench (`cargo run -p skerry-ui --example preview`) with interactive fixture states (Connect default/busy/error, Library populated/empty/busy, Session Controls H.265/JPEG/server-locked, and Session Overlays), Light/Dark/System theme switching, and live callback event feedback.
- **Rationale:** Keeps Skerry on `gpui-kit 0.10.0` (avoiding `gpui-storybook 0.8.0`'s `gpui-kit 0.9.0` pin and `gpui-preview`'s incompatible `gpui 0.2.2` stack) while giving a fast, self-contained UI iteration loop with no Keychain auth, Tokio runtime, or SealSkin/Selkies connection required—and ensuring previewed changes are the exact code rendered by `skerry-mac`.
- **Affected Contracts:** `src/skerry/index.prose.md`

## ARCH-008: Pinned SealSkin Server Version (0.5.0 Only)

- **Status:** superseded by `ARCH-009` — the user meant 0.5.0 as a minimum, not an exact version; kept for history
- **Decision:** Skerry supports exactly one SealSkin server version: `0.5.0` (`api::SUPPORTED_SEALSKIN_VERSION`). Before any other companion API call, `Api::check_version` reads `GET /companion/api/ui/version` (`{"version", "bridge"}`, served by SealSkin's `ui.router`) and refuses to continue unless `version` is exactly `0.5.0`. On a mismatch, a missing route (servers before 0.5.0), or an unreadable reply, Skerry does not load the library, does not store the `Api`, does not save the key to Keychain, and stays on the Connect screen with an error naming the required and found versions. The same gate runs on every Refresh.
- **Rationale:** SealSkin 0.5.0 changed security-relevant behaviour Skerry relies on or will build on: the per-origin `access_token` → `303` + `Set-Cookie` session exchange, collaboration room messages carrying only aliases (no room tokens), the `http`/`https`/`ftp`/`magnet` launch address rule, the new `POST /api/sessions/<id>/open_url`, Selkies 2.3.0 base images (`h264enc-striped` removed, congestion control on by default), and hardening switches that force file transfer, printing, and window-manager features off. Pinning one exact version keeps each contract verified against the one server it was written for, not a range nobody has tested.
- **Changing the pin:** Moving to any other SealSkin version (including a patch release such as `0.5.1`) is a new `ARCH-*` decision that supersedes this one. It must re-read that release's notes against these contracts, update `SUPPORTED_SEALSKIN_VERSION` and its unit test, and re-run codec QA (`ARCH-003`) against the new server.
- **Open verification:** The `ARCH-003` codec QA evidence was recorded on SealSkin 0.4.2. Re-run it on a 0.5.0 server (Selkies 2.3.0 congestion control now adapts the WebSocket video rate).
- **Affected Contracts:** `src/skerry/index.prose.md`, `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-mac/src/api.rs` (`SUPPORTED_SEALSKIN_VERSION`, `Api::check_version`, `Api::library`)

## ARCH-009: Minimum SealSkin Server Version (0.5.0 or Newer)

- **Status:** settled (pinned minimum) — supersedes `ARCH-008`
- **Decision:** Skerry supports SealSkin `0.5.0` and every newer version (`api::MINIMUM_SEALSKIN_VERSION = (0, 5, 0)`). Before any other companion API call, `Api::check_version` reads `GET /companion/api/ui/version` and parses `version` as SemVer `MAJOR.MINOR.PATCH[-pre][+build]`. Skerry refuses to continue when the version is older than `0.5.0`, is a pre-release of `0.5.0` (e.g. `0.5.0-rc1`, which SemVer orders before `0.5.0`), is not a plain three-part version (e.g. `0.5`, `v0.5.0`), the route is missing (servers before 0.5.0), or the reply is unreadable. A refused server leaves Skerry on the Connect screen with an error naming the minimum and the found version; the library is not loaded, the `Api` is not stored, and the key is not saved to Keychain. The same gate runs on every Refresh.
- **Rationale:** 0.5.0 is the first release with the behaviour these contracts rely on (`ARCH-008` lists it: per-origin session cookie exchange, alias-only room messages, launch address scheme rule, `open_url`, Selkies 2.3.0 images, forced hardening). A minimum rather than an exact pin lets operators take SealSkin patch and feature releases without waiting for a Skerry update.
- **Risk accepted:** Newer SealSkin releases are accepted without being verified. When a release changes a route or behaviour Skerry relies on, raising the minimum (or fixing Skerry) is a new decision that supersedes this one.
- **Open verification:** Re-run `ARCH-003` codec QA on a 0.5.0 server; its recorded evidence is from SealSkin 0.4.2.
- **Affected Contracts:** `src/skerry/index.prose.md`, `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-mac/src/api.rs` (`MINIMUM_SEALSKIN_VERSION`, `parse_version`, `Api::check_version`, `Api::library`)

## ARCH-010: Multi-Window Session Architecture & Native Mac App Presence Scope

- **Status:** settled (scope & multi-window model; implementation tracked in `ARCH-GAP-LS-08`, native Mac app presence mechanism deferred to `ARCH-GAP-LS-09`)
- **Decision:**
  - **Multi-window sessions (`ARCH-GAP-LS-08`):** Adopt an always-separate-windows model. The Launcher stays open in its own window, and every launched or resumed remote session opens in its own dedicated native macOS (`gpui-kit` `Root`-wrapped) window. Multiple concurrent sessions—including multiple sessions of the same remote application—can run side by side, each with independent stream/audio/controls lifecycle.
  - **Native Mac app presence (`ARCH-GAP-LS-09`):** Add per-app native macOS identity (separate Dock icon, `Cmd+Tab` entry, and optionally Spotlight) to Skerry's scope. The delivery mechanism (on-demand generated `.app` shims vs. automatic per-app helper processes vs. both) and relaunch behavior when a session for that app is already running are intentionally left open for a future compose decision.
  - **Protocol boundary:** Because Selkies streams a whole virtual framebuffer rather than rootless per-window surfaces, individual Linux windows opened inside a single remote application stay inside that session's virtual display (a second virtual monitor remains tracked in `ARCH-GAP-VS-01`).
- **Affected Contracts:** `src/skerry/index.prose.md`, `src/skerry/launcher-and-sessions.prose.md`

---

## Active & Deferred Constraint Frontier (`ARCH-GAP-*`)

| ID | Owning Contract | Concept | Status | Summary |
|---|---|---|---|---|
| `ARCH-GAP-VS-01` | `video-and-screen` | `second-display` | unresolved (backlog) | Open a second remote display in its own GPUI window following server display placement while primary session owns audio. |
| `ARCH-GAP-VS-02` | `video-and-screen` | `webrtc-transport` | unresolved (backlog) | Prototype WebRTC signaling, negotiated codecs, ICE/STUN/TURN, and data-channel input vs. direct WebSocket. |
| `ARCH-GAP-VS-03` | `video-and-screen` | `extended-codecs-and-tuning` | unresolved (backlog) | Evaluate VP8/VP9 decode paths, 10-bit, 4:4:4, Turbo, paint-over, and CPU encoder options. |
| `ARCH-GAP-LS-01` | `launcher-and-sessions` | `launch-options-and-progress` | unresolved (todo) | Storage mode (auto/cleanroom/named home), locale/timezone, Wayland/X11, GPU/VPN, startup progress stages, and remembered 1-click launch. |
| `ARCH-GAP-LS-02` | `launcher-and-sessions` | `session-management` | unresolved (todo) | Session thumbnail previews, Send File, and Stop (distinct from Disconnect, with cleanroom data loss confirmation). |
| `ARCH-GAP-LS-03` | `launcher-and-sessions` | `open-links-and-files` | unresolved (todo) | Open pasted URLs in the omnibar and local files via native macOS mechanisms (File → Open… `⌘O`, window drag-and-drop, and `.app` bundle `CFBundleDocumentTypes` / `on_open_urls` Finder "Open With" — no "Upload a file" omnibar button) via SealSkin 0.5.0 `POST /api/launch/url`, `/api/launch/file`, `/api/launch/file_path`, and `POST /api/sessions/<id>/open_url`; only `http`, `https`, `ftp`, or space-free `magnet` addresses are sent. |
| `ARCH-GAP-LS-04` | `launcher-and-sessions` | `launcher-library` | unresolved (todo) | Search, app icons, recents, favorites, hidden apps, per-app launch preferences, and matching running-session reuse. |
| `ARCH-GAP-LS-05` | `launcher-and-sessions` | `persistent-storage` | unresolved (todo) | SealSkin storage API integration for homes, shared files, direct remote file opening, and public share links. |
| `ARCH-GAP-LS-06` | `launcher-and-sessions` | `account-and-key-recovery` | unresolved (todo) | Show signed-in account, capabilities, and limits; dormant-key browser wake flow and expired/revoked key replacement. |
| `ARCH-GAP-LS-07` | `launcher-and-sessions` | `cluster-and-session-origins` | unresolved (todo) | Automatic/pool/node placement and validated token handoff to isolated per-session origins without leaking control-plane keys. |
| `ARCH-GAP-LS-08` | `launcher-and-sessions` | `multi-window-sessions` | unresolved (todo) | Always-separate-windows model (`ARCH-010`): Launcher stays in its own window; every launched/resumed session opens in its own native macOS window, supporting multiple concurrent sessions of the same app. |
| `ARCH-GAP-LS-09` | `launcher-and-sessions` | `native-mac-app-presence` | unresolved (backlog) | Per-app native macOS presence (Dock icon, `Cmd+Tab`, optionally Spotlight); mechanism (`.app` shims vs. helper processes vs. both) and existing-session relaunch behavior deferred to future compose decision. |
| `ARCH-GAP-DC-01` | `desktop-and-collaboration` | `clipboard-sync` | unresolved (todo) | Bridge Selkies clipboard messages to `NSPasteboard` honoring server direction policy, Unicode, focus changes, and loop prevention. |
| `ARCH-GAP-DC-02` | `desktop-and-collaboration` | `file-transfers` | unresolved (todo) | In-session Selkies file upload/download with native macOS Open/Save dialogs, progress, cancel, and folder/ZIP support. |
| `ARCH-GAP-DC-03` | `desktop-and-collaboration` | `audio-devices-and-quality` | unresolved (todo) | Output device selection, sample-rate conversion, sleep/wake recovery, and server-allowed audio bitrate controls. |
| `ARCH-GAP-DC-04` | `desktop-and-collaboration` | `remote-printing` | unresolved (todo) | Receive remote PDF print jobs with macOS Save and Print actions and optional auto-open preference. |
| `ARCH-GAP-DC-05` | `desktop-and-collaboration` | `microphone-and-webcam` | unresolved (todo) | Audio/video capture uplinks with macOS permission requests on demand, capture indicators, and disconnect teardown. |
| `ARCH-GAP-DC-06` | `desktop-and-collaboration` | `gaming-controls` | unresolved (todo) | Relative pointer capture with escape shortcut, physical gamepad forwarding with slot/rumble support, and custom chords. |
| `ARCH-GAP-DC-07` | `desktop-and-collaboration` | `session-sharing-and-rooms` | unresolved (todo) | Selkies view-only / player-slot roles and SealSkin collaboration room evaluation (against 0.5.0 rooms, whose media/cursor messages carry only the sender's alias). |
| `ARCH-GAP-DC-08` | `desktop-and-collaboration` | `multitouch-trackpad` | deferred | Revisit raw multitouch contact forwarding only when Chrome gesture-to-display latency matches native macOS pinch/pan. |
