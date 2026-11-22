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
  - **Operator vs. Client Settings:** Capture backends, GPU selection (*per-launch GPU choice superseded by `ARCH-016`: the operator still defines which GPUs exist, and the user picks among those at launch, as the web launcher does*), TLS, TURN credentials, audit/recording policy, and server administration (users, groups, app stores, templates, cluster nodes, audit logs) remain operator/web-dashboard responsibilities. Skerry only requests session changes permitted by server policy.
  - **Control-Plane vs. In-Session:** SealSkin companion API owns application keys, catalog, launching, persistent homes/shares, and stopping sessions. Selkies WebSocket owns live stream settings, input, clipboard, in-session file transfer, printing, and media uplinks. *(Amended by `ARCH-023`: in-session file transfer is Selkies' HTTP file routes on the session origin, not the WebSocket.)*
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
- **Decision:** Skerry supports SealSkin `0.5.0` and every newer version (`api::MINIMUM_SEALSKIN_VERSION = (0, 5, 0)`). Before any other companion API call, `Api::check_version` reads `GET /companion/api/ui/version` and parses `version` as SemVer `MAJOR.MINOR.PATCH[-pre][+build]`. Skerry refuses to continue when the version is older than `0.5.0`, is a pre-release of `0.5.0` (e.g. `0.5.0-rc1`, which SemVer orders before `0.5.0`), is not a plain three-part version (e.g. `0.5`, `v0.5.0`), the route is missing (servers before 0.5.0), or the reply is unreadable. A refused server leaves Skerry on the Connect screen with an error naming the minimum and the found version; the library is not loaded, the `Api` is not stored, and the key is not saved to Keychain (since `ARCH-011`: to `~/.config/skerry/config.toml`). The same gate runs on every Refresh.
- **Rationale:** 0.5.0 is the first release with the behaviour these contracts rely on (`ARCH-008` lists it: per-origin session cookie exchange, alias-only room messages, launch address scheme rule, `open_url`, Selkies 2.3.0 images, forced hardening). A minimum rather than an exact pin lets operators take SealSkin patch and feature releases without waiting for a Skerry update.
- **Risk accepted:** Newer SealSkin releases are accepted without being verified. When a release changes a route or behaviour Skerry relies on, raising the minimum (or fixing Skerry) is a new decision that supersedes this one.
- **Refresh (decided 2026-11-21, was `ARCH-GAP-LS-12`):** Before, a refusal during Refresh showed the error and kept the old Library on screen. Now `Api::library` returns `LibraryError::Refused` for any version-gate refusal, and the Launcher goes back to the Connect screen, dropping the Library and its textures. Connect with an empty key field checks that server again. Network failures (`LibraryError::Failed`) still keep the Library and show the error. This has not been tested live, because no server older than 0.5.0 is available.
- **Open verification:** ~~Re-run `ARCH-003` codec QA on a 0.5.0 server~~ done in QA 2026-11-21 against SealSkin 0.5.x (JPEG, H.264, H.265, AV1; `ARCH-012`).
- **Affected Contracts:** `src/skerry/index.prose.md`, `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-mac/src/api.rs` (`MINIMUM_SEALSKIN_VERSION`, `parse_version`, `Api::check_version`, `Api::library`, `LibraryError`), `crates/skerry-mac/src/app.rs` (`Launcher::load`, `leave_server`)

## ARCH-010: Multi-Window Session Architecture & Native Mac App Presence Scope

- **Status:** settled (scope & multi-window model; implemented in `ARCH-022`, native Mac app presence mechanism deferred to `ARCH-GAP-LS-09`)
- **Decision:**
  - **Multi-window sessions (`ARCH-GAP-LS-08`):** Adopt an always-separate-windows model. The Launcher stays open in its own window, and every launched or resumed remote session opens in its own dedicated native macOS (`gpui-kit` `Root`-wrapped) window. Multiple concurrent sessions—including multiple sessions of the same remote application—can run side by side, each with independent stream/audio/controls lifecycle.
  - **Native Mac app presence (`ARCH-GAP-LS-09`):** Add per-app native macOS identity (separate Dock icon, `Cmd+Tab` entry, and optionally Spotlight) to Skerry's scope. The delivery mechanism (on-demand generated `.app` shims vs. automatic per-app helper processes vs. both) and relaunch behavior when a session for that app is already running are intentionally left open for a future compose decision.
  - **Prior art for `ARCH-GAP-LS-09` (research note, 2026-11-22; no decision):** [tw93/Pake](https://github.com/tw93/pake) (Tauri 2, system WKWebView) is a working "generated `.app` shim" example. Its `pake` CLI writes a per-app `src-tauri/.pake/tauri.conf.json` (name, URL, window, icon, `identifier` such as `com.pake.weekly`) and runs `tauri build` for each app. So every app is its own bundle with its own Dock icon and `Cmd+Tab` entry, but making one needs a Rust toolchain and recompiling. Its `Cargo.toml` also has an unused `runtime-template` feature: one prebuilt binary that reads its config from `Contents/Resources` at launch. That suggests a cheaper path for Skerry: copy a prebuilt stub `.app`, rewrite `Info.plist` (bundle id, name) and the `.icns`, and have the stub hand off to Skerry (e.g. a `skerry://` URL). The mechanism and the relaunch behaviour stay open.
  - **Protocol boundary:** Because Selkies streams a whole virtual framebuffer rather than rootless per-window surfaces, individual Linux windows opened inside a single remote application stay inside that session's virtual display (a second virtual monitor remains tracked in `ARCH-GAP-VS-01`).
- **Affected Contracts:** `src/skerry/index.prose.md`, `src/skerry/launcher-and-sessions.prose.md`

## ARCH-011: Application Key Stored in `~/.config/skerry/config.toml`, Not the macOS Keychain

- **Status:** settled — replaces the Keychain store (`dev.kebot.sealskin` / `application-key`) that `Credential::startup`, `Credential::save`, and `Credential::forget` used before; no earlier `ARCH-*` decision recorded the Keychain choice.
- **Decision:** After a successful Connect from the key field, Skerry saves the application key as `application_key = "ssk_…@host"` in `~/.config/skerry/config.toml`. Startup loads `SEALSKIN_APPLICATION_KEY` → `.env` → that file. The folder is created `0700`, and the file is replaced atomically through a temporary file created `0600` before any secret bytes are written. Writes go through `toml_edit`, so other settings and comments in the file survive; **Disconnect Server** removes only `application_key`. A non-string `application_key` or an unparseable file is reported on the Connect screen, and Skerry does not overwrite it. Skerry never reads or writes the macOS Keychain, and there is no migration from it (reading the old item would itself show a Keychain prompt). The `security-framework` dependency is removed.
- **Rationale:** Rebuilt and unsigned development binaries make macOS show a Keychain access prompt on every new build, which blocks startup until someone answers it and gets in the way of unattended QA. A user-private config file is the convention for CLI-adjacent tools and is easy to inspect and edit.
- **Risk accepted:** The key sits in plaintext on disk, guarded only by file permissions (`0600`/`0700`). Any process running as the user can read it, the same exposure as `.env`.
- **Verification:** `api::tests::saved_key_round_trips_through_a_private_config_toml`, `saving_the_key_keeps_other_settings_and_comments`, `malformed_saved_keys_are_reported_not_ignored`; live: Skerry connected to `sealskin.kebot.me` from `config.toml` with no Keychain prompt.
- **Affected Contracts:** `src/skerry/index.prose.md`, `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-mac/src/api.rs` (`Credential::startup`/`save`/`forget`, `config_file`, `read_saved_key`, `write_saved_key`), `crates/skerry-mac/Cargo.toml`, `crates/skerry-ui/src/connect.rs` (Connect copy)

## ARCH-012: H.265 and AV1 Predicted Frames on Selkies 2.3.0 Follow Continuity, Not the Reference Field

- **Status:** settled — amends the `ARCH-003` codec baseline for SealSkin 0.5 / Selkies 2.3.0 servers
- **Finding (QA 2026-11-21):** On SealSkin 0.5.x, H.265 and AV1 streamed at ~1 fps with "Rejected packets" climbing. Packet traces showed that H.264 packets carry `reference` = id of the previous emitted packet, but Selkies 2.3.0's `x265` and `svt-av1` packets carry `reference == id`. `video::Decoder` only accepted a P-frame whose reference equalled the last decoded id, so every H.265/AV1 P-frame was dropped and only the once-per-second recovery keyframes showed.
- **Decision:** `video::follows(last, id, reference)`: when `reference != id`, a predicted frame must reference the last decoded packet (H.264 chaining, unchanged). When `reference == id` (no chain information), it is accepted if the previous frame decoded. The WebSocket is ordered and lossless, and any decode failure still clears `last`, requests a keyframe, and waits for it.
- **Evidence:** live traces on `sealskin.kebot.me` went from H.265 3/22 and AV1 3/26 packets decoded to 21/21 and 26/26; unit test `video::tests::predicted_frames_follow_chained_or_unchained_references`.
- **Affected Contracts:** `src/skerry/video-and-screen.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-mac/src/video.rs` (`follows`, `Decoder::decode`)

## ARCH-013: Settled Launcher Library, Launch Dialog, Thumbnails, and Stop Session

- **Status:** settled — partially settles `ARCH-GAP-LS-01`, `ARCH-GAP-LS-02`, `ARCH-GAP-LS-04`
- **Decision:** The Library (search over apps and running sessions, category chips, app icons, session thumbnails with GPU and storage pills), the Launch Dialog (Auto/Cleanroom storage, language, Wayland, collaborative, Copy link with in-place "Copied" confirmation), and Stop with a data-loss confirmation are settled behaviour; the QA plan `skerry.qa.test.prose.md` covers them. Favorite and Hide stay hidden (`LaunchDialogProps::show_organize = false`) until favorites and hidden apps are saved and a hidden app can be shown again (`ARCH-GAP-LS-04`). The search placeholder no longer promises opening addresses (`ARCH-GAP-LS-03`).
- **Evidence:** QA run 2026-11-21 (L1–L3, D2, K1, K2 passed against SealSkin 0.5.x).
- **Affected Contracts:** `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/index.prose.md`
- **Affected Rust:** `crates/skerry-ui/src/{library,launch_dialog,session}.rs`, `crates/skerry-mac/src/{app,api}.rs`

## ARCH-014: Session Resilience, Modal Hit-Testing, and Library Texture Lifecycle

- **Status:** settled
- **Decision:**
  - **WebSocket startup retry:** `transport::run` retries the Selkies WebSocket for up to 45 s (`SOCKET_STARTUP_GRACE`, every 1.5 s) on I/O errors and 404/502/503/504, reusing the cookie from the single `access_token` exchange. Other failures still fail at once.
  - **Recoverable session errors:** the session status banner has **Reconnect** and **Disconnect** buttons. The ⋯ Session Controls trigger also appears when the pointer enters the window's top-right corner, so it is reachable when the picture fills the window. The trigger occludes, so clicking it never also clicks the remote desktop. Reconnects for codec or HiDPI changes keep Session Controls open. cmd-w is deliberately not bound in a session, because remote apps need it (ctrl-w with **Command sends Control**).
  - **Modals:** the Launch Dialog and Stop confirmation backdrops `occlude()` the Library, and Escape dismisses them.
  - **Library textures:** like JPEG frames, app icons and session thumbnails are GPU textures that GPUI frees only through `window.drop_image`. Replaced, removed, stale, and forgotten images are queued in `Launcher::retired_images` and dropped on the next render. Downloads started for an older Library (`media_generation`) are discarded. Server images are decoded with limits (≤ 8192 px, ≤ 256 MB) and downscaled (icons 256 px, thumbnails 960 px).
  - **Force Aligned Resolution:** Selkies aligns its stored display size, not the client's desired size, so after toggling alignment Skerry re-sends its desired resolution, which the server then aligns (or restores exactly when alignment is off).
  - **Full screen:** leaving full screen re-applies the session chrome, so the restored title bar never covers the picture.
  - **Connect:** a failed connect keeps the typed key. When the saved key fails at startup, Connect with an empty field retries it. A malformed `config.toml` is reported as invalid TOML with "fix or delete" guidance.
- **Affected Contracts:** `src/skerry/index.prose.md`, `src/skerry/video-and-screen.prose.md`, `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-mac/src/{transport,view,app,api}.rs`, `crates/skerry-mac/src/app/controls.rs`, `crates/skerry-ui/src/{session,launch_dialog,library}.rs`

## ARCH-015: Async Launcher Updates Resolve Their Window by Handle

- **Status:** settled
- **Finding (QA 2026-11-21, RT-BUG-1 / BUG-8 / RT-BUG-2):** In `gpui-pre 0.3.12`, `WeakEntity::update_in` and `AsyncWindowContext::update_window_entity` do not use the window the task was spawned in. They resolve the entity's *current window* (`App::with_window(entity_id)`), which is the last window that rendered or updated it. Session Controls renders the `Launcher`, so after it opened, the session-event task's `ScreenPolicy` / `ResizeAllowed` handlers ran `resize_display` against the 388×680 pt panel. That sent `r,776x1360` after codec reconnects, and Force Aligned Resolution turned it into 768×1360. It also made UI Scaling read the panel's display. Debug traces showed `resize viewport=388×680 from app.rs:786/792`.
- **Decision:** Long-lived or async work on an entity that another window renders must capture `Window::window_handle(window)` up front and run `handle.update(cx, |_, window, cx| entity.update(cx, …))`. Never use `update_in` for it. `Launcher::open` (session events) and `Launcher::launch_with_options` follow this. Force Aligned Resolution also re-requests the desired resolution after toggling (`ARCH-014`), so turning it off restores the exact size.
- **Evidence:** final retest on a 1267×1392 pt window (1966×2160 remote): H.264 → H.265 → AV1 → JPEG → H.264 with Session Controls open kept 1966×2160 with the stream running. Force Aligned Resolution on gave 1952×2160; off went back to 1966×2160 with no Reset. UI Scaling stayed at 200 % throughout. Run record: `runs/2026-11-21-qa/`.
- **Affected Contracts:** `src/skerry/index.prose.md` (Invariants)
- **Affected Rust:** `crates/skerry-mac/src/app.rs`

## ARCH-016: Launch Dialog Options Match SealSkin's Web Launcher

- **Status:** settled — settles more of `ARCH-GAP-LS-01` (named homes, timezone, GPU, VPN) and the per-app launch preferences of `ARCH-GAP-LS-04`; amends `ARCH-005` (per-launch GPU choice) and `ARCH-013` (Auto storage)
- **Finding (2026-11-21):** Skerry's dialog had a GPU field that was never filled, and its "Auto (per-app persistence)" storage sent no `home_name`, which the server treats as a temporary home for any non-meta app (`server/app/launch.py`): Auto sessions were really Cleanroom ones and lost their data.
- **Decision:** The Launch Dialog follows SealSkin's web launcher (`client/src/ui/home.js`, `lib/gpus.js`, `lib/homes.js`, `lib/languages.js`, at `selkies-project/sealskin@1ba09a7`) for a single SealSkin node, in the same order:
  - **Storage:** shown only when the user has `persistent_storage` and the app `home_directories`; Auto, Cleanroom, then the user's named homes (`GET /companion/api/homedirs`, without `_sealskin_shared_files` and `auto-*`; none for a meta app). Auto sends `home_name = auto-<app name>` (lower case, spaces/underscores → `-`, other characters dropped) after creating it with `POST /companion/api/homedirs` when it is missing (409 counts as present; a meta app's is made by the server). Without storage the launch sends `cleanroom`.
  - **GPU:** shown only when `settings.gpu` is on and a GPU from `POST /companion/api/admin/status` fits the app (`nvidia` driver → `nvidia_support`, any other driver → `dri3_support`); "None" first. Labels are `model (node)` or `node (driver)` where node is the device's last path part, so a Kubernetes PodTemplate reads `intel-gpu (i915)`. The id goes to `selected_gpu` unchanged (a render node on Docker, a template name on Kubernetes).
  - **VPN / VPN endpoint:** shown when `status.vpns` is not empty, "No VPN" first; the endpoint field shows when the chosen profile has two or more endpoints, and a new VPN picks its first endpoint. Sent as `vpn` and `vpn_endpoint`.
  - **Language:** the launcher's 261 session locales, searchable, labelled with the Mac's name for each (`NSLocale`): "German (Germany) · de_DE" (user's choice over raw ids). The default is the locale nearest the Mac's (`default_language`, the web launcher's rule).
  - **Timezone:** no control; every launch sends the Mac's time zone (`NSTimeZone`).
  - **Remembered options:** after a successful launch, every option is saved per app in the server's preferences file (`Preferences::launch`, `~/Library/Application Support/SealSkin Native/<server>.json`); the next dialog for that app starts from them where they still hold, else the defaults (Auto, no GPU, no VPN, nearest locale, Wayland on).
  - **Copy link:** `/app/<id>/` with only what differs from a default launch: `home` (not Auto), `gpu`, `vpn`, `vpn_endpoint`, `room=1`, `lang` (not the default), `wayland=0`.
  - A status the server won't give leaves GPU and VPN hidden and storage off; the Library still loads.
- **Out of scope:** SealSkin cluster choices (`auto:<kind>`, `<device>@<node>`, the Where field) stay with `ARCH-GAP-LS-07`; startup progress stages and one-click launch stay with `ARCH-GAP-LS-01`.
- **Design review:** the dialog was built in the preview workbench first (stories **Launch Dialog** and **Launch Dialog · Minimal**; `SKERRY_PREVIEW_STORY=<story id>` opens one) and approved in the Plannotator Inbox on 2026-11-21.
- **Evidence:** `launch_options::tests::*` and `api::tests::{decodes_the_status_launch_rights_gpus_and_vpns, launch_body_carries_only_the_chosen_options, deep_link_carries_the_launch_options}`; read-only check against `sealskin.kebot.me` (Kubernetes): the companion lane returns `settings`, five GPU options (`intel-gpu`/`i915` and four `cpu` node templates), no VPNs, and Auto homes only. A live launch from the new dialog has not been run yet.
- **Affected Contracts:** `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/index.prose.md`, `src/skerry/skerry.test.prose.md`, `src/skerry/skerry.qa.test.prose.md`
- **Affected Rust:** `crates/skerry-mac/src/launch_options.rs` (new), `crates/skerry-mac/src/{api,app,settings,main}.rs`, `crates/skerry-mac/Cargo.toml` (`NSLocale`, `NSTimeZone`), `crates/skerry-ui/src/launch_dialog.rs`, `crates/skerry-ui/examples/preview.rs`

## ARCH-017: Clipboard Sync Follows the Selkies Web Client, Triggered by Mac Focus

- **Status:** settled 2026-11-21 — settles `ARCH-GAP-DC-01` and `ARCH-GAP-DC-10`. Live: the person tested copy and paste in a session on 2026-11-21 and reported it works; the full QA list (`skerry.qa.test.prose.md` B1–B6) has not been run step by step.
- **Reference:** Selkies `715df7c` server `crates/selkies/src/clipboard/{verbs,outbound,flavors}.rs`, web core `web/selkies-web-core/lib/clipboard-sync.ts`, and `docs/usage.md#clipboard`. Skerry speaks the same wire protocol as the web client. Skerry's behaviour differs only where a browser's limits forced the web client's design.
- **Wire protocol (WebSocket text frames):**
  - *Mac → session:* text goes as `cw,<base64 UTF-8>`. A binary payload goes as `cb,<mime>,<base64>`. Payloads of 16383 raw bytes or more are split into multipart transfers: `cws,<id>,<size>` / `cwd,<id>,<base64>` / `cwe,<id>`, and `cbs,<id>,<mime>,<size>` / `cbd` / `cbe` for binary. Each transfer has its own id, and the server caps a transfer at 64 MiB. Rich text goes as the flavour envelope `application/x-selkies-clipboard-flavours`, a JSON object `{"text/html": …, "text/plain": …}`. The server accepts the envelope even when the binary clipboard is off. Images go as `image/png` and need `enable_binary_clipboard`.
  - *Session → Mac:* the server sends `clipboard,<base64>`, `clipboard_binary,<mime>,<base64>`, or, for large payloads, `clipboard_start,<mime>,<size>` + `clipboard_data,<base64>`… + `clipboard_finish`. It may send `clipboard_secret` (the next payload is a password-manager secret) and `clipboard_reply,cr` (the next payload answers our `cr` fetch) first. A new `clipboard_start` drops an unfinished payload. A payload whose size does not match its declared size is thrown away.
  - Skerry does not send `REQUEST_CLIPBOARD`. The web client needs it only because Firefox and Safari cannot write the clipboard without a user gesture. A native app can always write `NSPasteboard`, so the server's own change announcements are enough.
- **Policy:** `server_settings` sets the limits. `clipboard_in_enabled` controls Mac → session, `clipboard_out_enabled` controls session → Mac, `enable_binary_clipboard` controls images, and `clipboard_seamless` (value and lock) sets the starting mode. The server also enforces every limit itself. Within those limits, Session Controls has a **Clipboard** section with per-server switches, saved in `Preferences.clipboard`: **Send to Session**, **Receive from Session**, and **Seamless**. A direction the server refuses is not shown, and a locked Seamless reads "Locked".
- **Seamless (default):**
  - *Mac → session:* Skerry checks `NSPasteboard.changeCount` when a session window becomes key and when the remote desktop gains focus. If the count changed and the content differs from the last synced value, Skerry sends it. The same check runs again just before a paste keystroke is forwarded (V with Control held, or Shift+Insert). The socket is ordered and the server finishes clipboard writes before it handles the next message, so the paste finds the new content. Skerry never polls while it is in the background. A copy made in another Mac app reaches the session only when the person returns to a session window.
  - *Session → Mac:* an announced payload is written to `NSPasteboard` as soon as it arrives, even when Skerry is in the background.
  - *Loop prevention:* after writing the pasteboard, Skerry records the new `changeCount`, so the next focus check does not send that content back. Skerry also keeps one "current" signature, the latest value synced in either direction, as the web core does, and never re-sends unchanged content.
- **Deliberate (Seamless off):** nothing moves by itself, and incoming announcements are ignored. The Clipboard section's **Send Mac Clipboard** button reads the pasteboard and sends it. **Copy Session Clipboard** sends `cr` and writes the tagged reply to the pasteboard. These two buttons replace the web dashboard's text box.
- **Content mapping (`pasteboard.rs`, direct `objc2-app-kit` `NSPasteboard`):** gpui's clipboard API has no `changeCount`, no HTML, and no concealed type, so Skerry uses `NSPasteboard` directly.
  - Text is `public.utf8-plain-text`. Outgoing CRLF/CR line ends become LF, as in the web core.
  - `public.html` with its string goes out as the flavour envelope. An incoming envelope is written as both HTML and plain text. RTF-only copies go out as plain text.
  - `public.png` goes out as `image/png`, and TIFF-only images are converted to PNG first. An incoming `image/png` or `image/jpeg` is written under its own pasteboard type.
  - Finder file copies go out only as their plain-text names. Real files are `ARCH-GAP-DC-02`.
- **Secrets:** an incoming secret is written together with `org.nspasteboard.ConcealedType` (the nspasteboard.org convention), so clipboard managers skip it. It is cleared after 60 s (the web core's `SECRET_CLIPBOARD_TTL_MS`), or as soon as an empty secret or any later payload arrives, but only while `changeCount` still shows Skerry's write. Clipboard contents are never logged.
- **Shortcuts (decided 2026-11-21, was `ARCH-GAP-DC-10`):** ⌘C/⌘X/⌘V stay on the Mac while **Command sends Control** is off, as `ARCH-004` already had it (the user's choice, over mapping just those three to Ctrl or turning Command sends Control on by default). In a session, Control+C/X/V copy, cut, and paste. Control+V, Control+Shift+V, and Shift+Insert send the Mac clipboard first. ⌘ does the same once Command sends Control is on.
- **Risk accepted:** newer macOS versions may show a paste-privacy prompt the first time Skerry reads the pasteboard without a paste gesture (the focus check). The `changeCount` check keeps such reads to copies the person actually made.
- **Affected Contracts:** `src/skerry/desktop-and-collaboration.prose.md`, `src/skerry/index.prose.md`, `src/skerry/skerry.test.prose.md`, `src/skerry/skerry.qa.test.prose.md`
- **Evidence:** `clipboard::tests::*` and `pasteboard::tests::*` (private pasteboard round-trip); `cargo test --workspace`, Clippy, and fmt pass.
- **Affected Rust:** new `crates/skerry-mac/src/{clipboard,pasteboard}.rs`; `Cargo.toml` (`NSPasteboard`, `NSData`, image `tiff`); `transport.rs`, `view.rs`, `app.rs`, `app/controls.rs`, `settings.rs`; `crates/skerry-ui/src/controls.rs`

## ARCH-018: Session Controls Look and Work Like the Selkies Web Sidebar

- **Status:** settled — design approved in the preview workbench on 2026-11-21; amends the Session Controls layout of `ARCH-007`, partially settles `ARCH-GAP-VS-05`
- **Finding (2026-11-21):** Selkies' web sidebar (`selkies-dashboard/src/layouts/sidebar/sidebar.css`, `panels/settings.tsx`, `translations.ts`) shows each section as its own rounded card, 10px apart, with uppercase titles. Inside, every setting is a label above a full-width control: dropdowns for Encoder, Rate Control, UI Scaling, and Preset; sliders with the value in the label ("Frames per second (60 FPS)", "Video CRF (25)", "Video Bitrate (8 Mbps)"); switches for on/off settings; full-width 40px buttons. Skerry stacked flush sections and used segmented button rows, −/+ steppers, "Turn On/Off" buttons, and inline option lists.
- **Decision:** `skerry-ui::controls` follows the web sidebar for the settings Skerry already supports (no new settings):
  - **Cards:** sections inset 16px with a 10px gap; 1px border, 12px radius, 11×14px header, 4/14/14px body; titles upper case, chevron down when closed and up when open.
  - **Rhythm:** 14px between settings, 6px from a label to its control, labels semibold.
  - **Dropdowns:** Encoder (codec), Encoder Rate Control Mode ("CBR (Constant Bitrate)", "CRF (Constant Quality)"), UI Scaling, and Preset are full-width 40px dropdown buttons opening a popup menu with the current choice checked (no `SelectState`; the inline picker lists and `ControlsAction::TogglePicker` go away).
  - **Sliders:** Frames per second, Video Bitrate (Mbps), Video CRF, JPEG Base Quality, and Paintover Quality are `gpui-kit` sliders over the server's min..max. Their live value shows in the label while dragging, and the setting is requested once, on release. `skerry-ui` keeps each slider's state with `window.use_keyed_state`, so callers pass only values.
  - **Switches:** Use Paint-Overs, HiDPI, Force Aligned Resolution, and Input options; a server-locked one is a disabled switch.
  - **Buttons:** Set Manual Resolution and Reset to Window side by side at equal width; Scale Locally, Reset Video Settings, and key combinations full width.
  - Skerry keeps its own theme colors (no pink accent) and native-only rows (Preferred codec notice, Stream Diagnostics).
  - The Clipboard section (`ARCH-017`) takes the same switches (a server-locked Seamless is a disabled switch) and full-width Send Mac Clipboard / Copy Session Clipboard buttons.
- **Design review:** approved in the preview workbench (`cargo run -p skerry-ui --example preview`, Session Controls stories) on 2026-11-21.
- **Evidence:** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`; screenshots of the Hardware H.265 story. Dropdown menus and slider drags have not yet been exercised in a live session.
- **Affected Contracts:** `src/skerry/video-and-screen.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-ui/src/controls.rs`, `crates/skerry-ui/examples/preview.rs`, `crates/skerry-mac/src/app/controls.rs`

## ARCH-019: Stats Section Follows the Selkies Web Sidebar's Stats

- **Amended by `ARCH-031`:** the subscription is held while Stats is open or the stream strip is on, and history resets only when a subscription starts.

- **Status:** settled — design approved in the preview workbench on 2026-11-21; amends the 1-second Stream Diagnostics of `ARCH-003` (`playback-and-diagnostics`)
- **Finding (2026-11-21):** Selkies' web sidebar Stats (`selkies-dashboard/src/panels/stats.tsx` `StatsSection`, `stats-data.ts`, `selkies-web-core/lib/stream-stats{,-view}.ts`) shows four status rows, three history graphs, figure tiles, and host meters, and copies a plain-text report. Its numbers come from the server only while the stats are on screen: the page sends `_stats,1`, and the server answers once a second with `stream_stats` (`encode_ms`, `encoded_fps`, `pipeline_ms` with `pipeline_min_ms`/`pipeline_max_ms`, `send_ms` with its range, `video_mbps`, `peak_mbps`, `target_mbps` under CBR, `rtt_ms`, `throttled`, and the host's `cpu_percent`, `mem_used`/`mem_total`, `gpu_percent`, `gpu_mem_used`/`gpu_mem_total`). `stream_info` describes the encoder and capture path. Skerry showed nine label/value lines from its own counters and read only `stream_info.encoder`.
- **Decision:** The Session Controls section is titled **Stats** and draws, in order, what the web sidebar draws for a WebSockets session:
  - **Rows** with a status mark (good ✓ / warn ⚠ / neutral •), worded by the web's rules (`streamRows`): **Encoder** (encoder · codec 4:2:0/4:4:4 · 10-bit · Striped; GPU · node · driver; warns when hardware was expected and fell back, with the server's reason), **Capture** (X11/Wayland · Zero-copy or Readback; warns on a readback where zero-copy was available), **Decoder** (Hardware via VideoToolbox for H.264/H.265/AV1, Software for JPEG — neutral, since no hardware JPEG path is expected; codec · resolution), **Connection** (WebSockets; warns while the server is `throttled`). A copy button puts the web's plain-text report (`streamReport`) on the clipboard, with "SealSkin Native" in place of the user agent.
  - **Graphs**, 44px high, line plus 12% area, a midline, and the axis top in the corner: **FPS** (client, with the server's `encoded_fps` as a second series when it reports one; axis from the configured frame rate up), **Bandwidth** (received Mbps), **Latency** (ms; Skerry's own WebSocket ping, else the server's `rtt_ms`). History starts when the section opens, keeps 600 s, and is bucketed by maximum to 120 points.
  - **Tiles**, two columns: Encode, Capture to encoded and Capture to sent (with the server's least–most), Decode, Video rate (against the CBR target; warns over 110%), Peak 250 ms, Received, Lost frames (rejected packets), Key frame requests. A time with nothing measured this second shows the last one within 5 s, else "idle" when no frame moved. Skerry leaves out the tiles it does not measure (Arrival to screen, Audio buffer, Frames not shown) instead of showing dashes.
  - **Meters**: CPU and GPU as bars, memory and VRAM as "used / total GiB", when the server reports them.
  - **Subscription:** Skerry sends `_stats,1` while the Stats section is open in Session Controls and `_stats,0` when it closes or the panel closes; a reconnect re-sends it. Server figures older than 3 s are left out of a sample.
- **Out of scope:** the floating panel's gauges and the "Show over the stream" strip (`ARCH-GAP-VS-06`), and mic/webcam uplinks (`ARCH-GAP-DC-05`).
- **Design review:** approved in the preview workbench (Session Controls stories, which open with Stats expanded) on 2026-11-21.
- **Evidence:** `stats::tests::*` (7 tests) and the workspace checks. Not yet run against a live server: the `_stats,1` subscription, the server-side tiles, and the host meters are verified only against fixtures and Selkies `715df7c` source (`crates/selkies/src/ws/server.rs` `send_stream_stats`, `stats/mod.rs`, `ws/host.rs`).
- **Affected Contracts:** `src/skerry/video-and-screen.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-mac/src/{transport,app,stats}.rs` (`stats.rs` new), `crates/skerry-mac/src/app/controls.rs`, `crates/skerry-ui/src/{stats,controls,lib}.rs` (`stats.rs` new), `crates/skerry-ui/examples/preview.rs`

## ARCH-020: Selkies Web Sidebar Panel Inventory (Reference)

- **Status:** reference — records what each panel of Selkies' web sidebar holds, so future Session Controls work starts from it; each open row is owned by the `ARCH-GAP-*` named in it
- **Source:** Selkies `715df7c` web dashboard: the panel catalog `selkies-dashboard/src/panels/catalog.ts` (`PANELS` and the `STREAMS` toggles), the sidebar layout `layouts/sidebar/index.tsx` and `sidebar.css`, panel bodies under `panels/` (`settings.tsx`, `stats.tsx`, `clipboard.tsx`, `files.tsx`/`file-manager.tsx`, `printing.tsx`, `apps.tsx`, `sharing.tsx`, `gamepads.tsx`, `shortcuts.tsx`, `catalog-bodies.tsx`, `soft-keys.tsx`, `appearance.tsx`), and strings in `translations.ts`. A panel shows only where the server allows it (`renderable.*` from `server_settings`) and only on the primary display's page; a view-only (`#shared`) page has no dashboard.
- **The sidebar, top to bottom:**

| Web element | What it holds | Skerry today | Owner |
|---|---|---|---|
| Header | Wordmark; theme pill (dark/light, also sent as the desktop's color scheme); layout toggle (sidebar / toolbar / edge); translucency knob | Follows the Mac's appearance (`Theme::sync_system_appearance`) and sends the color scheme; no layout or translucency choice | Excluded: native presentation (`ARCH-005`) |
| Mode buttons | Fullscreen (Ctrl+Shift+F); gaming mode (Ctrl+Shift+X: hides the dashboard and locks the pointer); a touch client's keyboard and trackpad | Full Screen button and the window's own full screen | Gaming mode: `ARCH-GAP-DC-06`; touch buttons excluded (`ARCH-005`) |
| Stream tiles | Five toggles: video, audio, microphone, webcam, gamepad input, each shown only where the server allows it | Pause Video / Mute buttons in the Session Controls header | Icon-tile row: `ARCH-GAP-VS-05`; mic/webcam: `ARCH-GAP-DC-05`; gamepad: `ARCH-GAP-DC-06` |
| Soft keys | A touch client's held modifiers, missing keys, chord palette, trackpad switch | — | Excluded: touch-only overlay (`ARCH-005`) |
| **Video Settings** | Format card: Encoder, Webcam Encoder, FullColor 4:4:4, 10-bit; Performance card: Frames per second, Rate Control, Video Bitrate, Video CRF, Turbo, Base/Paintover JPEG quality, Use Paint-Overs, Paint-Overs CRF, Paint-Over Burst Frames, CPU Encoding, Multithreaded CPU Encoding | Encoder, FPS, Rate Control, Bitrate, CRF, JPEG Base/Paintover Quality, Use Paint-Overs (`ARCH-018`) | 4:4:4, 10-bit, Turbo, Paint-Over CRF/Burst, CPU encoding: `ARCH-GAP-VS-03`; Webcam Encoder: `ARCH-GAP-DC-05` |
| **Screen Settings** | Display card: Add Screen +, Anti-aliasing, Use CSS cursors, Raw pointer motion, Command sends Control, HiDPI, Force Aligned Resolution, UI Scaling; Resolution card: Preset, Width/Height, Set Manual Resolution, Reset to Window, Scale Locally | HiDPI, Force Aligned Resolution, UI Scaling, Preset, Width/Height, Set/Reset, Scale Locally (`ARCH-018`); Command sends Control lives in **Input** | Add Screen: `ARCH-GAP-VS-01`; Raw pointer motion: `ARCH-GAP-DC-06`; anti-aliasing and CSS cursors are browser rendering, excluded |
| **Audio Settings** | Quality card: Audio Bitrate slider (kbps choices); Devices card: Input (microphone) and Output (speaker) pickers, "not supported" where the browser cannot pick an output | Mute only; 48 kHz Opus to the default output | Bitrate and output device: `ARCH-GAP-DC-03`; input device: `ARCH-GAP-DC-05` |
| **Stats** | "Show over the stream" switch; status rows, graphs, tiles, meters, copy (`StatsSection`) | Rows, graphs, tiles, meters, copy (`ARCH-019`) | Strip and floating gauges: `ARCH-GAP-VS-06` |
| **Clipboard** | The server clipboard's text, editable (sent on blur, `clipboardUpdateFromUI`); image upload when binary clipboard is on (`enable_binary_clipboard`); masked-secret copy button | Clipboard section: Send to Session, Receive from Session, Seamless; Send Mac Clipboard / Copy Session Clipboard when Seamless is off; `NSPasteboard` sync on focus (`ARCH-017`). No editable text box: the Mac's own pasteboard is the clipboard | Settled: `ARCH-017` (`ARCH-GAP-DC-01`) |
| **Files** | Opens the file manager: browse the session's directories, upload and download (shown when the server allows either transfer) | A native **Files** window per session (⌘⇧F, Session Controls → Files): browse, transfers, Finder copy/paste and drag both ways, Delete via SealSkin storage (`ARCH-023`, `ARCH-024`) | Settled (`ARCH-GAP-DC-02`); mount `ARCH-GAP-DC-14`, rename/move `ARCH-GAP-DC-15` |
| **Printing** | Shown once a document arrives: auto-print switch and every printed document with Print and Save (`printDocument`, `printRequest`) | — | `ARCH-GAP-DC-04` | `desktop-and-collaboration` | `remote-printing` | settled (`ARCH-033`) | One-shot PDF fetch, notices with Print…/Open/Save…, the macOS print sheet, Open Print Dialog Automatically, and cache cleanup. |
| **Apps** | "Manage Apps" opens the proot-apps catalog by category with search, install / remove / update / launch, and each job's console; needs the server's `command_enabled` | — | `ARCH-GAP-DC-11` | `desktop-and-collaboration` | `apps-catalog` | settled (`ARCH-035`) | An Apps window per session over `apps,{op}`. Support is detected, the GitHub catalog is fetched with no credentials, and icons are rasterised in a sandboxed worker process. |
| **Sharing** | Copyable links for the viewer (`#shared`) and controller slots (`#player2`–`#player4`), each gated by its server setting | — | `ARCH-GAP-DC-07` |
| **Gamepads** | Gamepad input switch, touch gamepad switch, Rumble on/off (`gamepad_rumble`), live picture of each pad (`gamepadButtonUpdate`, `gamepadAxisUpdate`) | — | `ARCH-GAP-DC-06` | `desktop-and-collaboration` | `gaming-controls` | settled (`ARCH-032`) | Pointer capture with ⌃⌥⇧Z and `m2` relative motion; GameController pads on slots 0–3 per session with rumble; custom chords dropped. |
| **Shortcuts** | The dashboard's own chords (Ctrl+Shift+F fullscreen, Ctrl+Shift+X gaming mode, Ctrl+Shift+M dashboard, Ctrl+Shift+G virtual gamepad, Ctrl+Shift+Left Click pointer lock) and a "Shortcuts active" switch that hands them to the session instead | `cmd-shift-m` opens Session Controls; Send Key Combination (Alt+Tab, Ctrl+Alt+Delete) in **Input** | `ARCH-GAP-DC-12` | `desktop-and-collaboration` | `session-shortcuts` | settled (`ARCH-031`) | A Shortcuts card listing `shortcuts::CHORDS`, with "Mac shortcuts active" handing them to the session as Ctrl chords; ⌘Q always stays local. |
| **Pelorus** (first, when the server serves `/pelorus`) | A prompt against the server's Pelorus API, which describes the desktop as text (windows, positions, accessibility tree) for a caller without vision, with a pop-out page | — | `ARCH-GAP-DC-13` | `desktop-and-collaboration` | `pelorus` | excluded (`ARCH-035`) | An agent loop that can act on the desktop and needs a model key; not surfaced to people. |
| Footer | "Cite our paper academically" link | — | Excluded |

- **Affected Contracts:** `src/skerry/video-and-screen.prose.md`, `src/skerry/desktop-and-collaboration.prose.md`

## ARCH-021: Decoded Frames Are Presented as They Arrive, Not on a 30 Hz Tick

- **Amended by `ARCH-034`:** JPEG frames are presented on their bottom stripe or after a 2 ms settle, as per-stripe images, instead of an 8 ms full-canvas flush.

- **Status:** settled — amends the presentation pacing of `ARCH-003` (`codec-and-rate-control`)
- **Finding (2026-11-21):** the Stats FPS graph (`ARCH-019`) showed the client at 30 fps while the server encoded 60. `transport::run` handed frames to the window only on its 33 ms `present` tick, which the first client (`e0835a5`) used to gather a JPEG frame's stripes into one picture. A hardware frame decoded between ticks waited in `decoded`, and a second one replaced it unseen, so H.264/H.265/AV1 were shown, and counted, at 30 fps at most.
- **Decision:**
  - **Hardware codecs:** each VideoToolbox frame is a whole picture, so it goes to the window (`frames.send_replace`) and is counted the moment it decodes; the `decoded` hold is gone.
  - **JPEG:** stripes still gather into one canvas, now flushed by an 8 ms `jpeg_flush` tick while anything changed, so a 60 fps JPEG stream is not capped at 30. A frame is counted once per frame id, so stripes of one frame split across two flushes do not count twice.
  - The 33 ms tick keeps only what does not need frame rate: `CLIENT_FRAME_ACK` of the newest frame (Selkies needs one every 50 ms), pointer motion, and the hardware-decode expiry check.
- **Affected Contracts:** `src/skerry/video-and-screen.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-mac/src/transport.rs`

## ARCH-022: Multi-Window Session Behaviour (`ARCH-GAP-LS-08`)

- **Amended by `ARCH-039`:** a session may now have a second connection, its Display 2 window. "Never a second connection" still holds for resuming an open session.

- **Status:** settled (decided and verified live 2026-11-22). Settles `ARCH-GAP-LS-08` and builds on `ARCH-010`.
- **Decision:**
  - **Window ownership:** a `SessionWindow` entity (`app/session.rs`) owns one session's stream, settings, stats, display mode, resize, policies, and Session Controls. The `Launcher` owns only the Library and a list of open session windows `(AnyWindowHandle, WeakEntity<SessionWindow>, session_id)`. Session Controls' `Owner` pairs the session window's handle with `Entity<SessionWindow>` (`ARCH-006`). Session windows open through `cx.defer` outside `Launcher` updates.
  - **Resume of an already open session:** brings its existing window to the front. Skerry never opens a second connection to a session it is already showing.
  - **Closing the Library:** the Library window closes and session windows keep streaming. **Window → Library** (`cmd-0`) or a Dock-icon click (app reopen) opens it again. Skerry quits only on `cmd-q` or when the last window closes. `cmd-q` disconnects every session and stops none.
  - **Preferences across windows:** codec, input, clipboard, and video changes are saved per server for the next session that opens. Other open windows keep what they have. Display mode, resolution, mute, and pause are per window, and each session plays its own audio.
  - **Closing a session window:** the title bar and window buttons stay hidden over the picture (`ARCH-014`). Moving the pointer into the window's top-left corner reveals the close / minimize / zoom buttons, and **Window → Disconnect Session** closes the key session window. `cmd-w` stays with the remote app.
  - **Teardown:** closing a session window (or the status banner's **Disconnect**) ends only that session's stream, releases its held keys, closes its Session Controls, and refreshes the Library. It never calls the stop API.
- **Affected Contracts:** `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/index.prose.md`, `src/skerry/skerry.test.prose.md`, `src/skerry/skerry.qa.test.prose.md`

## ARCH-023: A Native Files Window per Session over Selkies' HTTP File Routes

- **Status:** settled 2026-11-22 (built, tests/Clippy/fmt pass, and confirmed working live by the person); `ARCH-GAP-DC-02` closed. Designed 2026-11-21. Amends `ARCH-005` (file transfer is HTTP, not the WebSocket). Opens `ARCH-GAP-DC-14` (mount, future scope) and `ARCH-GAP-DC-15` (delete/rename/move, blocked on the server).
- **Reference:** Selkies `715df7c` server `crates/selkies/src/files/{mod,upload,manager,confine,transfer,archive}.rs` and `http/auth.rs::is_origin_allowed`; web core `web/selkies-web-core/lib/file-upload.ts`; dashboard `web/selkies-dashboard/src/panels/{files,file-manager}.tsx`; `docs/usage.md#file-transfers`.
- **What the web client does:** everything goes over HTTP on the session origin, confined by the server to `FILE_MANAGER_PATH` (default `~/Desktop`). `POST /api/upload` takes the destination in `X-Upload-Path` (URL-encoded); files over 64 MiB go as sequential 64 MiB slices with `X-Upload-Id`, `X-Upload-Offset`, `X-Upload-Total`, and `X-Upload-Final: 1`, staged and renamed into place, idle transfers reaped after 1 h. `GET /api/file-manager/browse?path=` lists one folder (`{name, path, is_dir, size, mtime}`, refused over 50,000 entries); `POST /api/file-manager/mkdir` `{"path"}`; `GET /api/files/<path>` serves a file as an attachment with `Range`/`HEAD`; `GET /api/file-manager/zip?path=` packs a folder (≤ 8 GiB, ≤ 100,000 files) before sending. The server paces transfers against the stream. `server_settings.file_transfers` (`upload`, `download`) gates each direction; view-only credentials are refused both. The dashboard runs one upload batch at a time, has no cancel, and there is no session-initiated download.
- **Decision:**
  - **Transport:** a new `files.rs` reuses the SealSkin session cookie from `transport.rs::socket_request` and the session base URL, on reqwest with streaming bodies. It sends no `Origin` header (Selkies treats a request without one as same-origin, which `mkdir` needs). Uploads use the web client's exact plain/chunked shapes; downloads use `Range` to resume.
  - **Window:** a **Files** window per session (chosen over a Session Controls section or a sidebar), opened from the session (Session Controls and a shortcut) and closed with the session. UI in `skerry-ui`, wiring in `skerry-mac`, opened like Session Controls (`cx.defer`, `(AnyWindowHandle, Entity<T>)` per `ARCH-006`/`ARCH-015`).
  - **Native behaviour:** sortable list, breadcrumbs, Back/Forward, Refresh, New Folder, multi-select, keyboard navigation, Quick Look, Open with the default Mac app (download to a cache first), Upload… (`NSOpenPanel`, files and folders), Download… (`NSSavePanel` / choose folder), and a transfers list with progress and Cancel.
  - **Finder → session:** drops onto the window (GPUI `ExternalPaths`) and ⌘V of Finder file copies (`public.file-url`) upload into the folder shown. Folders are walked on the Mac and recreated with `mkdir`.
  - **Session → Finder:** drag uses `NSFilePromiseProvider` (GPUI's own external drag needs files that already exist locally), and Skerry downloads into the folder Finder names. ⌘C downloads the selection to a staging folder first and then writes real file URLs to `NSPasteboard` (chosen over drag-only and over a Save dialog), because Finder does not take file promises from the clipboard. Folders download file by file into a matching folder, not through the zip route, so progress starts at once, files can resume, and the zip limits do not apply.
  - **Policy:** controls follow `file_transfers`. Skerry connects as the primary client and does not model a viewer role; the server refuses view-only credentials itself.
  - **Out of scope:** delete, rename, and move (`ARCH-GAP-DC-15`: no server route, left out rather than added to Selkies for now); mounting in Finder via File Provider, WebDAV, or NFS (`ARCH-GAP-DC-14`).
- **QA 2026-11-21 (subagent, code + in-process fake server):** request shapes match Selkies `715df7c`. Fixed: list keys firing inside the New Folder field (`FilesList && !Input`), ⌘W leaving transfers running, `416` on a stale partial (now restarts from 0), resume measuring an unflushed partial, a drag promise left uncompleted when the window was gone, uploads overwriting a same-named file when the listing was stale (the job re-lists first), the list snapping back to the selection during transfers, `:` refused in names, drags starting on a 1 px wobble (4 pt threshold). Known, not fixed: a resume sends no validator, so a file changed between attempts could splice (the 1 s retry window makes it unlikely); ⌘C writes the pasteboard when the download ends even if something newer was copied; Shift-arrow after extending grows rather than shrinks; long names plus the partial suffix can exceed `NAME_MAX`. Not verified live: SealSkin's proxy for these routes, real Finder drag/paste, `qlmanage`.
- **Merged with multi-window (`ARCH-022`), 2026-11-22:** the Files window belongs to its `SessionWindow` (`app/session_window.rs`: `files_window`, `file_access`, `file_policy`, `toggle_files`), opens over that session's window, and closes with its teardown; each session window has its own Files window. Renumbered from `ARCH-018`/`ARCH-019` and `ARCH-GAP-DC-11`/`-12`/`LS-12` on the branch, since master took those IDs.
- **Risks to check live:** that SealSkin's proxy passes `api/upload`, `api/files/`, and `api/file-manager/*` and accepts its session cookie there (Skerry uses the cookie only for the WebSocket so far), and whether that cookie expires during a long session.
- **Affected Contracts:** `src/skerry/desktop-and-collaboration.prose.md`, `src/skerry/skerry.test.prose.md`; `ARCH-005`.

## ARCH-024: Two Clients — `SealSkinClient` (Control Plane) and `SelkiesClient` (Session) — and Delete Through SealSkin Home Storage

- **Status:** settled 2026-11-21 — amends `ARCH-023` (delete was out of scope) and narrows `ARCH-GAP-DC-15` to rename/move; opens `ARCH-GAP-LS-13` (direct Selkies sessions).
- **Asked by the person:** "we also want to support Selkies sessions, so we should be two api client, SelkiesClient and SealSkinClient", and agreed to deleting through SealSkin's storage route.
- **Decision:**
  - `crates/skerry-mac/src/sealskin.rs` holds `SealSkinClient` (was `api.rs` / `Api`): the companion API with the application key — catalog, launch, sessions, homes, and now home storage. `crates/skerry-mac/src/selkies.rs` holds `SelkiesClient` (was `files.rs` / `Access`): one Selkies session's HTTP routes, authorized today by the SealSkin session cookie and later by a standalone Selkies login (`ARCH-GAP-LS-13`). Neither depends on the other; the Files window takes a `FilesSource { selkies, policy, storage: Option<Storage { client: SealSkinClient, home }> }`, so it works with Selkies alone and gains Delete when SealSkin is there.
  - **Delete** uses SealSkin's persistent-storage route, not Selkies: `POST /companion/api/files/delete/<home>` `{"paths": [...]}` returns a `task_id`, polled with `GET /companion/api/files/delete_status/<task>` until `completed` or `error` (SealSkin `server/app/routers/files.py`). The companion lane takes a plain JSON body with the Bearer application key (probed live 2026-11-21: `GET /companion/api/files/list/auto-zen` answered 200).
  - **Path mapping:** SealSkin mounts the home at `/config` (`container_config_path`), the session's `~`, and Selkies' `FILE_MANAGER_PATH` defaults to `~/Desktop`, so a Files path `p` is `/Desktop/p` in the home. Selkies keeps `file_manager_path` from clients (`client_excluded`), so the window offers Delete only after the Selkies root and the home's `/Desktop` list the same names (upload/download staging files aside), checked on open and on Refresh. A Cleanroom session (no home) gets no Delete.
  - Delete is ⌘⌫ or the toolbar's trash button, always confirmed in a native warning alert naming the home; the folder reloads afterwards.
- **Live evidence:** the probe listed `homelab-layer1-hardware 2.png` in `auto-zen`'s `/Desktop`, the "keep both" copy of the person's own upload through the Files window, confirming both the upload path and the mapping.
- **Affected Rust:** `sealskin.rs` (renamed; `storage_names`, `delete_storage`, `has_storage`), `selkies.rs` (renamed), `app/files_window.rs` (`FilesSource`, `Storage`, `check_storage`, `confirm_delete`, `storage_path`, `same_folder`), `app/session_window.rs`, `app.rs`, `crates/skerry-ui/src/files.rs` (`can_delete`, `FilesAction::Delete`).
- **Affected Contracts:** `index.prose.md`, `desktop-and-collaboration.prose.md`, `launcher-and-sessions.prose.md`, `implement-feature.prose.md`, `skerry.test.prose.md`. Earlier decisions keep their original `api.rs` / `Api` names as history.
- **Design review 2026-11-22 (subagent, preview screenshots in Light and Dark):** first pass applied — one-line transfer rows in a collapsible "Transfers (n)" panel above the status bar (capped at about four rows); selection in the accent colour with white text, grey when the window is not key; Trash (not ⌫) at the toolbar's end and a folder-plus New Folder icon (both embedded through `skerry_ui::assets::SkerryAssets`, since gpui-kit's default bundle lacks them); header columns built like the rows so labels sit over values; the extension kept visible when a name truncates; Finder dates (`Today at 10:04`, `2 Nov 2026 at 08:05`) and `--` for folder sizes; error notices tinted with an icon and kept, information notices fading after 4 s or on navigation; Delete progress as a status-bar spinner; Try Again on a listing error; a loading spinner; drops onto a folder row upload into that folder; "Uploaded as “a 2.txt”" when keep-both renames; Delete alert copy ("This item will be deleted immediately from the persistent storage “X”. You can’t undo this action.") with Cancel as the cancel button; one name, **Files** (View menu, Session Controls button, window title `<app> — Files`, root crumb **Desktop**); blue folder icons; a primary Create button; plainer error wording. Second pass applied: a right-click menu on the list (Open, Quick Look, Download, Download To…, Copy, New Folder, Upload…, Paste, Delete…; a right-click on an unselected row selects it, on empty space clears the selection) and **File** (New Folder, Upload…, Download, Download To…, Open, Quick Look, Delete…, Close Window), **Edit** (Copy, Paste, Select All), and **Go** (Back, Forward, Enclosing Folder) menus bound to the window's actions, disabled where no Files window has focus. To check live: that the Edit menu's ⌘C/⌘V key equivalents never take those keys from a session window (GPUI handles the key window's keys before the menu). Left for later (low): type-to-select, Home/End/Page keys, a unified title bar, collapsing breadcrumbs, resizable columns.

## ARCH-025: Frontier Build Plan of 2026-11-22 (PM Triage)

- **Status:** settled 2026-11-22 (planning decision; each task settles its own `ARCH-*` when it ships).
- **Asked by the person:** "check the rest of missing feature, build them, decide for me", with every task run as PLAN → design → review → build → QA → code review in subagents and the main agent acting only as PM.
- **Process:** work lands on the integration branch `frontier-2026-11-22`; each task is built on its own branch in a worktree and merged into the integration branch only after its code review passes. Product questions inside a task are decided by the PM and recorded in that task's `ARCH-*` entry.
- **Build order (tasks, each one `ARCH-*` when settled):**
  - **A. Polish and hardening:** `ARCH-GAP-LS-10`, `ARCH-GAP-LS-11`, `ARCH-GAP-VS-05`, `ARCH-GAP-DC-09`.
  - **B. Library organization and quick launch:** `ARCH-GAP-LS-04` (favorites, hidden, recents, session reuse) and `ARCH-GAP-LS-01` (startup progress, one-click remembered launch).
  - **C. Open links and files, Send File:** `ARCH-GAP-LS-03`, `ARCH-GAP-LS-02`.
  - **D. Account and key recovery:** `ARCH-GAP-LS-06`.
  - **E. Audio devices and quality:** `ARCH-GAP-DC-03`.
  - **F. Stats overlay and Shortcuts panel:** `ARCH-GAP-VS-06`, `ARCH-GAP-DC-12`.
  - **G. Gaming controls:** `ARCH-GAP-DC-06`.
  - **H. Remote printing:** `ARCH-GAP-DC-04` (only if the pinned Selkies sends print jobs; otherwise recorded as blocked by the server).
  - **I. JPEG CPU cost:** `ARCH-GAP-VS-04`.
  - **J. Pelorus and Apps catalog:** `ARCH-GAP-DC-13`, `ARCH-GAP-DC-11`.
  - **K. Direct Selkies sessions:** `ARCH-GAP-LS-13`.
  - **L. Persistent storage and shares:** `ARCH-GAP-LS-05`.
  - **M. Microphone and webcam:** `ARCH-GAP-DC-05`.
  - **N. Second display:** `ARCH-GAP-VS-01`.
- **Deferred by the PM (not built in this plan):** `ARCH-GAP-VS-02` WebRTC (a second transport stack, needs a prototype before it can be designed); `ARCH-GAP-VS-03` VP8/VP9 software decode, 10-bit, and 4:4:4 (VideoToolbox has no hardware path; the hardware codecs already cover the need); `ARCH-GAP-DC-07` rooms and viewer roles (SealSkin 0.5.0 room messages carry only the sender alias, so control handoff can't be built cleanly); `ARCH-GAP-DC-08` raw multitouch (kept deferred on its own condition); `ARCH-GAP-DC-14` Finder mount (needs a signed File Provider extension); `ARCH-GAP-DC-15` rename and move (blocked: no server route); `ARCH-GAP-LS-07` cluster placement and per-session origins (no test cluster to verify against); `ARCH-GAP-LS-09` native per-app Dock presence (needs code-signing and a helper-process design of its own).
- **Rationale:** the order puts small, low-risk, client-only work first, then the launcher features people use every day, then in-session media and input. Tasks that need a second protocol stack, signing infrastructure, or a server we can't test against are deferred, so that what ships can be verified.

## ARCH-026: Launcher Polish, Control-Plane Hardening, Session Controls Memory, and Key Release Before Close (Task A)

- **Status:** settled 2026-11-22. Settles `ARCH-GAP-LS-10`, `ARCH-GAP-LS-11`, and `ARCH-GAP-DC-09` (client side). `ARCH-GAP-VS-05` is now settled except the web's stream-toggle tile row (`ARCH-020`).
- **Process:** plan → design → review (14 required changes) → build → QA (1 major, 5 minor) → code review (2 rounds, MERGE), all done by subagents (`ARCH-025`). Branch `task-a-polish`, merged into `frontier-2026-11-22`.
- **Launcher (`LS-10`):**
  - **Sign Out…** replaces **Disconnect Server** and asks for confirmation in a modal built like Stop Session (Cancel, then **Sign Out** in danger).
    - The wording depends on where the key came from. With `config.toml` it says the saved key is removed. With `SEALSKIN_APPLICATION_KEY` or `.env` it says Skerry doesn't store that key and will use it again next launch.
    - It says open session windows stay connected (`ARCH-022`).
    - It removes `application_key` whenever `config.toml` holds the active key, and ignores a Refresh or launch still in flight.
  - The Library content column is capped at 1200 pt and centred. Tiles and cards keep their sizes.
  - Toasts are a `skerry-ui` component: bottom centre, 3 s, a new one replaces the old, successes only. They read "Library refreshed" (manual Refresh) and "Link copied".
  - "Started … ago" updates every 30 s, only while the Library is visible and has sessions.
- **Hardening (`LS-11`):**
  - The application key is attached only to requests whose resolved path is under `/companion/api/`. Logos and `/app/<id>/icon.png` go through a second HTTP client that has no key. This is chosen on the resolved path in release builds too, and tested over raw TCP.
  - Preference and key writes go through one first-in-first-out write queue on Tokio (`persist.rs`). It is the only writer, with an in-memory copy that every reader uses, so reads always see queued writes. On quit it flushes within 1 s (`block_on` in `on_app_quit`).
  - Preferences load field by field. Unknown top-level keys from newer builds are written back. An unreadable file is copied to `<file>.bak-<timestamp>` before it is overwritten.
  - Library items are cached as `Rc` slices and rebuilt only when marked dirty. Every image retirement (`ARCH-014`) marks them dirty.
- **Session Controls (`VS-05`):**
  - Which sections are open is saved per server. Restoring Stats open subscribes as a click would.
  - Scroll position is kept per session window across closing the panel and reconnecting; it is not saved to disk.
  - Preset was already a dropdown (`ARCH-018`), so no picker changed.
  - Session Controls dropdowns use a `skerry-ui` `menu_dropdown` with a stronger, theme-aware shadow. Light: 0 8 24 at 16 % plus 0 2 6 at 8 %. Dark: 0 12 32 at 60 % plus a 1 px white 12 % ring, with GPUI blur at half the CSS value. Launch Dialog `Select` popups keep gpui-kit's own shadow.
- **Stray Super key (`DC-09`):**
  - Root cause in Skerry: `NativeSession::drop` aborted the transport task before the queued key releases were sent, so a key held during a codec switch stayed down on the remote.
  - Now the transport sends `ku` for every held key, then a WebSocket close frame. A 1 s abort watchdog is the fallback, and a cancel flag stops a stream that is dropped while still connecting from ever sending `SETTINGS`.
  - **No `kr` is sent anywhere.** Selkies' `reset_keyboard` releases Super/Meta/Control/Escape even when they aren't down, which could itself open a menu.
  - Key bookkeeping lives in a pure `KeyLedger` (`view/keys.rs`):
    - Command never maps to Super or Meta.
    - A keysym that Shift changes while held is released.
    - Keys are released on blur and before Session Controls, Files, or full screen.
  - `SKERRY_INPUT_TRACE=1` prints each input message, with printable keys redacted.
- **Corrections from review:** Selkies `disconnect()`/`connect()` reset only at server start and stop. An aborted socket (1006) leaves keys held until the server's roughly 2 s stale sweep, and a clean close runs `reset_keyboard`. Ctrl+Esc is a likelier trigger than Super.
- **Not verified live:** the screen was locked during QA, so nothing was checked visually (Light/Dark shots, dropdown shadow, toasts). The Sign Out run and the codec-switch input trace were checked in code and unit tests only. The Library loading, keyless logo fetches (a proxy saw no Authorization), and resilience to a corrupt preferences file were verified live.
- **Known, not fixed:** quitting still ends streams abruptly (1006), so the server holds keys for its grace period. The in-memory preferences stay ahead of disk after a failed write.
- **Affected Rust:** `persist.rs` (new), `view/keys.rs` (new), `sealskin.rs`, `settings.rs`, `app.rs`, `view.rs`, `transport.rs`, `app/session_window.rs`, `app/controls.rs`, `crates/skerry-ui/src/{library.rs,controls.rs,components.rs}`, `examples/preview.rs`.

## ARCH-030: Audio Output Follows the Mac, with a Device Picker, Recovery, and Audio Bitrate (Task E)

- **Status:** settled 2026-11-22. Settles `ARCH-GAP-DC-03`.
- **Process:** plan → design → review (12 required changes) → build → QA (3 minor, 1 trivial, all fixed) → code review (2 rounds, MERGE) by subagents (`ARCH-025`). Branch `task-e-audio`.
- **Decision:**
  - Each session has its own `skerry-audio` thread behind a `Backend` seam. Every cpal/Core Audio call runs on that thread, never on GPUI or Tokio. This is the one exception to "all I/O on Tokio". `Drop` sends a shutdown message and never joins.
  - **Following the default:** cpal 0.17's error callback does nothing for the default device, so a 1 s poll of the default device's UID rebuilds the stream when it changes.
  - **Output Device** dropdown in a new **Audio Settings** card (between Video Settings and Stats, collapsed by default):
    - It lists "System Default (<active name>)" and then the connected devices by name. The choice is saved per server by raw UID.
    - A saved device that is missing appears greyed and checked as "<name> (not connected)". Sound plays through the default until the device returns.
    - A chosen device that fails falls back with the note "X isn't responding. Playing through Y while Skerry keeps trying."
  - The stream always opens at 48 kHz f32 and Core Audio (AUHAL) converts. cpal never sets the device rate. Mono devices get an L+R downmix. There is no in-tree resampler.
  - **Recovery:** rebuild on a stream error, on a stall (render callbacks stop for 2 s, counted only while a stream exists, so mute and pause don't count), on a default change, or when the chosen device returns.
    - At most one rebuild per second, with 2–30 s exponential backoff.
    - Strikes clear only after 5 s of steady callbacks.
    - A retry of the chosen device opens a silent probe stream beside the playing fallback, and swaps only once the probe renders.
    - A single failed device listing keeps the last list.
    - The first recovery shows nothing; "Audio output stopped" appears only when nothing can play.
  - The 125 ms drop-oldest queue is kept across rebuilds.
  - **Audio Bitrate:** Selkies publishes `audio_bitrate` as an enum (bps strings in `allowed`, and a live value that can be a number). Skerry sends `SETTINGS,{"audio_bitrate":N,"picked":["audio_bitrate"]}`, as the Selkies web client does.
    - The dropdown reads "128 kbps".
    - It is hidden when the server doesn't send the setting, and shown disabled when locked or when only one value is allowed.
    - The choice is saved per server and re-sent when the session is ready, if it is still allowed.
  - Preferences (`AudioPreferences`, `ControlsSections.audio`) use struct-level `#[serde(default)]` and are written only through `persist.rs` (`ARCH-026`).
- **Evidence:** Selkies 715df7c `settings/table.rs`, `ws/server.rs:2294`, `selkies-ws-core.ts:6204`, and the dashboard's `utils.ts:184` visibility rule. cpal 0.17.3 `device.rs:916` (no error callback for the default device) and l.751 (sample rate is set only for input). Prior art: Parsec and Windows App on the Mac follow the macOS default with no picker, so the picker exists for parity with the Selkies web client.
- **QA (live, real Core Audio over the real transport against a fake WebSocket server):**
  - Audio started on 44.1 kHz speakers, and the bitrate pick went out as the exact `SETTINGS` text.
  - With a temporary aggregate device, the stream followed default changes in about 0.2 s. A chosen device that was also the default stayed put when the default moved, and a missing chosen device fell back and came back.
  - 15 open/close cycles leaked no threads.
- **Not verified:** real plug/unplug, sleep/wake, and mono hardware (all simulated); nothing on screen, because the screen was locked. "Sound Settings…" in the menu was deferred as scope creep.
- **Affected Rust:** `audio.rs`, `settings.rs`, `transport.rs`, `app/controls.rs`, `app/session_window.rs`, `crates/skerry-ui/src/{controls.rs,components.rs}`, `examples/preview.rs`.

## ARCH-027: Favorites, Hidden Apps, Recents, Session Reuse, Launch Progress, and One-Click Launch (Task B)

- **Status:** settled 2026-11-22. Settles `ARCH-GAP-LS-04` and `ARCH-GAP-LS-01`, and lifts the `ARCH-013` hold on Favorite and Hide.
- **Process:** plan → design → review (11 required changes) → build → QA (4 low, fixed) → code review (2 rounds, MERGE) by subagents (`ARCH-025`). Branch `task-b-library`, merged after Task E.
- **Organization (`LS-04`):**
  - SealSkin 0.5 has no per-user favorites, hidden or recents API: its web launcher keeps all three in browser storage (`home.js:113-116, 214-220`). Skerry saves them per server in a `library` key of the preferences JSON, through `persist.rs`.
    - One bad entry drops only that entry.
    - Lists are capped at 512 ids (Recent at 8) on load and on write.
    - Unknown app ids are kept, never pruned.
  - **Library order:** Running sessions, a Favorites row, a Recent row, then Applications. The rows show only on All with an empty search, as compact 120×112 pt tiles.
  - **Recent:** at most 8, newest first, recorded only after a successful launch. It leaves out favorites and hidden apps.
  - **Hidden apps:** they leave All, the categories and both rows, but still match a search. A **Hidden (n)** chip, last in the row, lists them. The chip is a filter enum, so it can't collide with a category named "Hidden", and it resets once nothing is hidden.
  - Favorite and Hide are offered both in the Launch Dialog (toggle buttons; the dialog stays open) and in a tile right-click menu. The menu items are "Launch with Last Options", "Launch Options…", "Add to/Remove from Favorites" and "Hide from/Show in Library"; a bare "Hide" would read as ⌘H.
  - **Session reuse** follows the web launcher (`open.js:240-249`): the oldest running session with the same app, the same resolved storage and the same Collaborative flag.
    - When one matches, the Launch Dialog shows **Open Running Session** as the primary button and **Launch New** as secondary, and updates as Storage or Collaborative change.
    - Sessions created locally carry their real `created_at`.
- **Launch progress (`LS-01`):**
  - Every launch sends a UUID `launch_id`, which is also the `X-Idempotency-Key`. The companion lane runs a POST at most once and caches its result for 600 s (`security.py:245-330`).
  - Skerry polls `GET /companion/api/launch/progress/<id>` (`progress.py`). The launch timeout is 960 s, since Kubernetes image pulls can take up to 900 s.
  - **Stages shown:** Requesting, Pulling image (Docker only), Starting (which covers `client`), and Connecting. They never move backwards.
  - A launching card shows first under Running sessions, with a checklist and the server's sentence; the app's tile shows the stage. There is no Cancel, because SealSkin has no cancel route.
  - Launches no longer lock the Library; one launch runs per app at a time.
  - **Follower rules (no second container):**
    - Progress decides, and a POST success always wins.
    - A launch is called lost only after its POST has ended.
    - A same-id Retry is offered only after the request was lost; a fresh-id Retry only once nothing is outstanding.
    - Results that arrive after Ready are ignored.
    - A 401 from progress waits for the outstanding POST.
    - A gateway 502 or 504 counts as a lost request.
  - **Recovery:** 180 s with no stage change before the session exists shows Keep Waiting / Retry. A failure shows "Couldn't launch {App}" with Retry and Dismiss.
  - **In the session window:** when the 45 s connection retry runs out, a banner offers Keep Waiting (5 more minutes), Retry and Disconnect. A single timed-out socket attempt counts as transient. A new stream clears the banner. Only Keep Waiting keeps the longer grace period.
  - A Refresh that was requested before a launch became ready keeps that launch's session.
  - Closing the Library window drops the launches it was following. The server finishes them, and the session appears under Running sessions next time.
- **One-click launch:** tiles with remembered options show a hover ▶ ("Launch with Last Options"), and Option-click does the same. It always starts a new session and rechecks the remembered options as the dialog does. A plain click still opens the dialog.
- **QA (live, `sealskin.kebot.me`, Kubernetes):**
  - Two POSTs with the same `launch_id` returned the same `session_id`, and the list grew by one. A third resend after ready came back cached in 31 ms.
  - Progress went 404 → starting → waiting → ready in 4.5 s, with no image stage on Kubernetes.
  - A copy of the Task A-era preferences file kept every key.
  - Not verified: anything on screen (screen locked), the 180 s stall prompt, a gateway 504 mid-launch, and closing the Library mid-launch.
- **Known:** a progress run that the server pruned after 900 s can still offer a fresh Retry.
- **Affected Rust:** `launch_progress.rs` (new), `sealskin.rs`, `settings.rs`, `launch_options.rs`, `transport.rs`, `app.rs`, `app/session_window.rs`, `view.rs`, `crates/skerry-ui/src/{library.rs,launch_dialog.rs,session.rs,components.rs}`, `examples/preview.rs`.

## ARCH-029: Account Menu and Key Recovery (Task D)

- **Status:** settled 2026-11-22. Settles `ARCH-GAP-LS-06`.
- **Process:** plan → design → review (10 required changes) → build → QA (2 medium, fixed) → code review (1 high, 1 medium, fixed; 2 rounds, MERGE) by subagents (`ARCH-025`). Branch `task-d-account`.
- **What SealSkin 0.5.0 gives a key:**
  - `POST /companion/api/admin/status` (`routers/admin.py:116`) is open to every user. It returns name, admin flag, groups, rights (storage, GPU, VPN), limits (sessions, session length, idle, CPUs, memory, storage), the hours allowance, `held`, and storage used (with `?storage=1`).
  - Key refusals are `401` with `detail` = `unknown` / `expired` / `dormant` / `disabled` / `client` (`security.py:489-513`).
  - A key's own expiry, name and last use are not available to the key (`/api/keys` answers web sign-ins only), so they are left out.
  - Only the built-in `admin` user never goes dormant.
- **Account menu:**
  - The Library header's right edge holds an account button (user icon, name, caret). It takes the place of Task A's header Sign Out (macOS convention and SealSkin's own profile card).
  - Its 300 pt popover shows: name with an Admin tag, server, groups; Storage/GPU/VPN chips; and limit rows in the web profile's order with 4 pt meters (warning at 80 %, danger at 100 %, with a caption). Storage uses binary GB, as `quota.py` does, and is measured only when the menu opens; only a failed measure is retried. The key's source and its sleep time come next.
  - The footer has **Open SealSkin in Browser** and **Sign Out…** (in red), which closes the popover and then opens Task A's confirmation (`ARCH-026`).
  - Admins see "Administrators have every capability and no limits." The key id appears only in a tooltip.
- **Recovery:**
  - **Dormant:** a panel with **Sign In on the Web** (opens `<origin>/?wake=1`, never with the key in the URL), Try Again, and Use a Different Key. Skerry checks again every 5 s for up to 3 minutes and when the window becomes active. The poll is a GPUI task the Launcher owns, cancelled on every exit; `WakeState` clears its in-flight mark whenever a poll starts or stops.
  - **Revoked or expired:** back to Connect with a red alert. The field is relabelled "New Application Key", empty and focused. The saved key stays in `config.toml` until a replacement connects. An empty Connect says "Paste a new application key." and sends nothing.
  - **Disabled, client not allowed, or inactive:** a yellow alert, and an empty field retries the current key. Use a Different Key keeps the current key as the retry target, and a network failure on retry doesn't lose it.
  - **Held:** read from `held: true` before any text match. It shows a panel with Open SealSkin in Browser and Try Again.
  - Any unrecognised 403 shows the server's own sentence under "SealSkin refused the request" and allows a retry. Quota or permission 403s show the server's sentence alone.
  - A key from the environment or `.env` gets a note: a pasted key is saved, but the environment key wins at the next launch.
  - A pasted key that is refused at first and connects later (wake poll, Try Again, empty Connect) is kept as a pending credential and saved on that success. Environment and `.env` keys are never saved.
  - Refusals on Refresh, Launch (including home creation and Task B's progress 401) and Stop all reach this flow. Session windows keep streaming; their Files window's Storage pane shows the refusal; a new key reaches a session window only after it is reopened.
- **Evidence:** SealSkin 0.5.0 `routers/admin.py`, `security.py:489-513`, `api.md:42`, `shell/host.js:253`, `ui/app.js:122-165`. Prior art: gh and Tailscale re-authenticate in the browser and keep the old credential until the new one works.
- **QA:**
  - Live and read-only: the real `admin/status` for the admin user gives the expected menu.
  - A fake HTTPS server with the real binary: every refusal code mapped to the right recovery state with one Library round, `config.toml` stayed byte-identical, and the key went only to the control-plane host.
  - Not verified: anything on screen (screen locked) and the wake flow in a real browser.
- **Affected Rust:** `app/account.rs` (new), `crates/skerry-ui/src/account.rs` (new), `sealskin.rs`, `app.rs`, `launch_progress.rs`, `app/files_window.rs`, `crates/skerry-ui/src/{library.rs,connect.rs,assets.rs}`, `examples/preview.rs`.

## ARCH-028: Open Links and Files in Remote Apps, and Files from the Launcher (Task C)

- **Status:** settled 2026-11-22. Settles `ARCH-GAP-LS-03` and `ARCH-GAP-LS-02`. Amends `ARCH-023`: a Files window can now be opened from the Library without a stream.
- **Process:** plan → design → review (17 required changes) → build → QA (4 minor, 2 trivial) → code review (1 medium; 2 rounds, MERGE), all by subagents (`ARCH-025`). Branch `task-c-open`, rebased over Task D.
- **Server facts** (sealskin `1ba09a7`):
  - Apps declare `url_support` and `extensions` (`models.py::Application`).
  - `POST /companion/api/launch/url` takes the `launch/simple` body plus `url`, checked against `^(?:(?:https?|ftp)://|magnet:\?)[^\s\x00-\x1f\x7f]+$`, at most 8192 characters.
  - `launch/file` works as `upload/initiate` → `upload/chunk` (base64, at most 8 MiB; Skerry sends 1 MiB) → `launch/file`.
  - `POST /companion/api/sessions/<id>/open_url` answers 200 `opened`, 409 when nothing took the link, and 404 when the session isn't the caller's.
  - The session cookie comes from a plain GET that answers 303, and the access token is not used up by it.
- **Links:**
  - Text typed or pasted into the Library search becomes a link by the web launcher's rule, plus `ftp` and space-free `magnet`. A bare host gets `https://`, and the scheme is lower-cased.
  - The chooser above the results offers **Open Here** for compatible running sessions (`open_url`) and **Launch & Open** for apps with `url_support`. If a session refuses, Skerry shows a notice and never launches anything by itself.
  - The chooser shows the real host and drops userinfo.
  - Return acts only on text the user typed that has a scheme or one of 32 common TLDs. Links from Finder, `skerry:`, drops or ⌘O always need a click.
  - ⌘L (Open Location…) works only while the Library is key.
- **Files:**
  - Files arrive by ⌘O (Library only), a drop on the Library or on a session card (blue ring and pill), Finder **Open With**, or the Dock icon. The bundle declares `public.data` at Alternate rank, so Skerry is never the default app, plus a `skerry://open?url=` scheme. Skerry doesn't register for http or https.
  - The chooser filters apps by extension, and shows every app when none matches. A running session gets **Copy to Desktop** (an upload through its Files window).
  - Uploads open the file once and stream 1 MiB chunks. They stop if the file changes, and Cancel aborts the Tokio task.
  - Files over 4 GiB and folders are refused, and several files go one at a time.
  - Errors map from the response status: 403 means storage is full, 413 too large, 429 too many uploads.
  - Uploads ride Task B's launching card. A long upload never shows the stall notice.
- **Launch & Open dialog:** it keeps its own copy of the target, so a link that arrives mid-dialog joins the queue instead of replacing it. It shows "Opens “report.pdf” · PDF · 2.4 MB", hides Copy link and B's Open Running Session, and its button reads **Launch & Open**.
- **Files from the launcher (`LS-02`):**
  - Each running-session card has an icon-only Files button. It opens that session's Files window with no stream window, using `transport::session_access`, which does the cookie exchange on its own client and never sends the application key.
  - There is one Files window per session, shared between the Library and the session window. Windows the Library opened close on Stop and on Sign Out.
  - With no stream, Skerry assumes both directions are allowed and turns one off if the server answers 403.
  - A session that is still starting reads "The session is still starting. Try again in a moment."
- **Refusals** from launch/url, launch/file, uploads and `open_url` go through `ARCH-029`'s key classification.
- **⌘O/⌘L in session windows:** these are bound only in the Library's key context, and the session desktop (the `SessionDesktop` key context) claims them, so a remote app gets them when Command sends Control is on. When it is off, they do nothing, like every other ⌘ key. File → Open… does nothing unless the Library is key.
- **QA (live):**
  - launch/url worked. launch/file sent a 2.6 MB file as 3 chunks and it reached the session.
  - `open_url` refusals mapped as expected (422 → refused, 404/409 → not taken). A successful `open_url` was not shown live: Zen answered 409 on its first-run screen.
  - `session_access` got the cookie with no WebSocket, and `SelkiesClient` listed and uploaded.
  - `plutil -lint` passed on the bundle.
  - Not verified: anything on screen, Finder Open With, Dock drops, and 413/429.
- **Known:** the TLD list includes `app` and `ai`, so "Zen.app" followed by Return opens `https://zen.app`. A key refusal clears the chooser.
- **Affected Rust:** `open_target.rs` (new), `sealskin.rs`, `transport.rs`, `selkies.rs`, `launch_progress.rs`, `app.rs`, `app/files_window.rs`, `app/session_window.rs`, `view.rs`, `crates/skerry-ui/src/{library.rs,launch_dialog.rs,files.rs,assets.rs}`, `scripts/bundle.sh`, `examples/preview.rs`.

## ARCH-031: Stats Strip over the Stream and a Shortcuts Card (Task F)

- **Status:** settled 2026-11-22. Settles `ARCH-GAP-VS-06` and `ARCH-GAP-DC-12`. Amends `ARCH-019`: Stats history now resets only when a subscription starts.
- **Process:** plan → design → review (7 required changes) → build → QA (4 trivial) → code review (3 rounds, MERGE) by subagents (`ARCH-025`). Branch `task-f-overlay-shortcuts`.
- **Stats strip (`VS-06`):**
  - A **Show over the stream** switch sits at the top of the Stats card, and View → **Toggle Stats Over Stream** does the same.
  - The strip copies the Selkies web client's "over the stream" strip:
    - four status marks (encoder, capture, decoder, connection), then fps, video Mbps against the target (amber above 1.1×), capture-to-sent ms, and RTT
    - a 55 % black pill centred 8 pt from the top, in every theme
    - tabular digits and fixed minimum widths so it doesn't jump
    - it wraps whole figures in narrow windows
  - It has no hitbox, so clicks pass through, including to the ⋯ button it draws over.
  - It is saved per server through `persist.rs`.
  - Only the strip was built. Moonlight, Parsec and Steam all use text overlays, and Selkies draws its gauges only in layouts Skerry doesn't follow.
  - `_stats,1` is sent while the Stats card is open or the strip is on (`StatsInterest`), and sent again after a reconnect.
  - `stats::strip_bottom` tells later overlays where to start below the strip.
- **Shortcuts (`DC-12`):**
  - A Shortcuts card after Input lists Skerry's own chords from one table (`shortcuts::CHORDS`): ⇧⌘M Session Controls, ⌃⌘F full screen, ⇧⌘F, ⌘0 Library, and ⌘Q "Always on the Mac". The glyphs are macOS menu glyphs in Apple's modifier order.
  - **Mac shortcuts active** follows the server's `keyboard_shortcuts` until the user changes it, then the saved per-server value. A server lock wins, shows as a disabled switch, and survives reconnects.
  - When it is off, those chords go to the session with ⌘ sent as Control. The key is sent as a down-up tap while Control stays held, so holding ⌘ never auto-repeats it. There is no escape chord: the View menu, the ⋯ button and ⌘Q always work, as in Jump Desktop and Windows App.
  - One ordered `app::key_bindings()` puts the global chords first, then Files, then the Library's ⌘O/⌘L (`ARCH-028`), then a `NoAction` suppression layer under the `SessionDesktop` key context last. gpui-pre 0.3.12 lets the last binding win at equal depth.
  - `passes_to_session` reuses the suppression bindings and always calls `stop_propagation`, so neither the menu nor AppKit's own ⌃⌘F item fires.
- **Key release (amends `ARCH-026`):**
  - AppKit sends no key-up while ⌘ is held. When ⌘ lifts, every key the ledger still holds is checked against the real HID key state (`CGEventSourceKeyState`) and released if it's up; keys still physically down stay held.
- **QA:**
  - A GPUI test window with the shipped key bindings, plus the real transport against a fake WebSocket server, showed:
    - chords skip their actions and arrive as Ctrl
    - ⌘Q stays local, and ⌘O/⌘L still work in the Library
    - the letter's `ku` comes before Control's
    - `_stats` ref-counting holds across 41 rapid flips and a reconnect
  - Not verified (screen locked): the strip's look, clicking through it, full screen, and AppKit's ⌃⌘F item live.
- **Affected Rust:** `shortcuts.rs` (new), `view/keys.rs`, `view.rs`, `stats.rs`, `app.rs`, `app/session_window.rs`, `app/controls.rs`, `crates/skerry-ui/src/{stats.rs,controls.rs,session.rs}`, `examples/preview.rs`.

## ARCH-033: Remote Printing Through the macOS Print Dialog (Task H)

- **Status:** settled 2026-11-22. Settles `ARCH-GAP-DC-04`.
- **Process:** plan → design → review (12 required changes) → build → QA (1 medium) → code review (3 medium; 2 rounds, MERGE) by subagents (`ARCH-025`). Branch `task-h-printing`.
- **Server facts (Selkies 715df7c):**
  - The server sends text JSON `{"type":"print_document","name","size_bytes"}` to the primary controller, and sends it again after `server_settings`.
  - `GET /api/print/{name}` deletes the file on a 200 before the body is sent (`transfer.rs:297`), so the fetch is one-shot.
  - Gated by `printing_enabled` (default on). SealSkin's `harden_container` turns it off.
- **Decision:**
  - **Fetch:** each job downloads immediately on Tokio through `SelkiesClient::print_document`.
    - The local file is created before the GET.
    - Never `Range`.
    - 30 s timeouts on the headers and on each chunk.
    - A 1 GiB cap, checked before the request, on Content-Length, and while streaming. An over-cap announcement shows **Too large** and is never fetched.
    - A `%PDF-` check as the bytes arrive.
    - Failures before a 200, including a SealSkin cookie 403, say "Reconnect to receive", because the document is still on the server.
    - No fetch starts once the window has closed.
  - **Print…:** the PDFKit print operation runs as a sheet on the session window, from a `cx.spawn` task with no `cx.update` around the AppKit call. It takes the `NSWindow` and checks again before opening that the window is visible, has no sheet, and is key. A damaged or locked PDF shows **Unreadable**, with Open as the main button. `objc2-pdf-kit` 0.3.2 was added; it is the only new crate.
  - **Notices:** stacked at top right, 340 pt wide, below the ⋯ button or below `ARCH-031`'s strip (`stats::strip_bottom`), with at most three shown. A Ready notice (Print… / Open / Save…) hides after 15 s. A name that failed and is announced again replaces the earlier failed job.
  - **Printing card:** placed after Clipboard and shown only while `printing_enabled` is on. It holds **Open Print Dialog Automatically** (default on, like the web client's `print_auto`, saved per server through `persist.rs`) and the session's jobs, with Print…, Open, Save… and Remove per row.
  - **Auto-open:** only when the session window or its sheet is key; otherwise the job waits for the window to become active (up to 5 min) and the notice says so.
    - PM decision: documents already waiting at connect (announced within 3 s of `server_settings`, aged by `Last-Modified`) each get a notice. Only the newest auto-opens, and only if it is less than 5 minutes old, so a reconnect never brings up a burst of print dialogs.
  - **Cache** (`~/Library/Caches/SealSkin Native/Print/`):
    - Remove and window close delete unopened files. A job under an open sheet survives.
    - Cmd-Q cleans every window's jobs synchronously in `on_app_quit`.
    - A startup sweep removes anything older than 24 h.
    - Closing with a sheet up ends the sheet and then removes the window, in one task.
- **Prior art:** Parallels RAS and Windows App on the Mac open the native print dialog without an extra step.
- **QA (live, real server):**
  - `lp` printed through Selkies came back as a PDF. A second fetch got 404.
  - A file over 1 GiB was listed as Too large and never fetched.
  - A reconnect with 6 waiting documents auto-opened only the newest fresh one.
  - The cleanup sweeps work in the real binary, and PDFKit rejects damaged, locked and no-print PDFs.
  - Not verified (screen locked): the notices, the card, and the print sheet on screen.
- **Affected Rust:** `print_jobs.rs` (new), `print_dialog.rs` (new), `selkies.rs`, `transport.rs`, `settings.rs`, `app.rs`, `app/session_window.rs`, `app/controls.rs`, `crates/skerry-ui/src/{session.rs,controls.rs}`, `examples/preview.rs`.

## ARCH-032: Pointer Capture and Gamepads (Task G)

- **Status:** settled 2026-11-22. Settles `ARCH-GAP-DC-06`. Custom chords were dropped, following `ARCH-031`.
- **Process:** plan → design → review (13 required changes) → build (G1 pointer capture, G2 gamepads) → QA (2 medium, 3 low) → code review (4 rounds; one high found in rounds 2 and 3, fixed) by subagents (`ARCH-025`). Branch `task-g-gaming`, merged after Task H.
- **Protocol (Selkies 715df7c):**
  - Relative motion is `m2,<dx>,<dy>,<mask>,<magnitude>,<seq>`, integers only, so Skerry carries fractions forward.
  - Pointer lock is `p,0|1` plus `SET_NATIVE_CURSOR_RENDERING,0|1`.
  - Gamepads use `js,c,<idx>,<b64 name>,<axes>,<buttons>`, `js,d`, `js,b`/`js,a`, and a `js,h` heartbeat. Mapping follows the W3C standard layout.
  - Rumble comes back as the system frame `rumble,<slot>,<strong>,<weak>,<ms>`.
  - Everything is gated by `gamepad_enabled`, read as `{value, locked}`.
- **Pointer capture (G1):**
  - **Capture Pointer** lives in the Input card and in the View menu. ⌃⌥⇧Z (Moonlight's chord) captures and releases. It joins `shortcuts::CHORDS` as always-local, and a held chord toggles only once. VMware's ⌃⌘ clashes with ⌃⌘F, and Parallels' ⌃⌥ is a common Linux chord.
  - **Capturing:**
    - An app-wide AppKit monitor (`view/capture.rs`, built like `pinch.rs`) sends raw deltas to the input channel as scaled `m2` with carry.
    - Skerry sends `p,1`, warps the cursor to the picture centre (primary-display flip), drops the first motion after the warp, dissociates the mouse from the cursor, and hides the cursor.
  - **Releasing:** on the chord, on blur or deactivation, on child windows (including a print sheet, `ARCH-033`), on full screen, on disconnect, and on drop.
    - Release shows "Pointer released". Errors release with no hint.
    - Modifiers are lifted before they are muted.
    - The transport sends `p,0`, `SET_NATIVE_CURSOR_RENDERING,0`, and `js,d` on close, because the server's cursor setting outlives the connection.
  - Server pointer-lock requests are not honoured: Selkies sends none.
  - Hint pill: "Pointer captured · ⌃⌥⇧Z releases it" at the bottom centre for 3 s.
- **Gamepads (G2):**
  - Apple's GameController framework (`objc2-game-controller` 0.3.2) and CoreHaptics (`objc2-core-haptics` 0.3.2; `createEngineWithLocality` is called through `msg_send!` because the crate gates it to iOS).
  - **Slots:** an app-wide table gives each pad the lowest free slot 0–3. Drivers are keyed by session (host + session id), because each Selkies session has its own slot table.
  - **Sending:** only changes are sent, plus `js,h` every 100 ms from the transport.
    - Pads send only while the session window is key, as in Moonlight. When the window loses key, its slots return to rest.
    - A window sends `js,d` only for the slots it drove.
  - **Rumble:** each message starts a fresh haptics player, capped at 5 s. Engines stop with their players, and a start that races a stop is retried once.
  - **Gamepads card:** four fixed slot rows, each with a numbered badge (Sending / Ready / Not sent), the name, and "· No rumble" when the pad has none. Battery display was cut.
  - Skerry drives up to four slots; the web client drives one.
- **Risks recorded:**
  - Every `p` change makes the server reconfigure its displays.
  - If the main thread hangs while the pointer is captured, the cursor stays frozen until ⌥⌘Esc.
  - In collaboration rooms (master token set), the server drops Skerry's `js,` messages (`ARCH-GAP-DC-07`).
- **QA:**
  - The app launched with GameController and CoreHaptics loaded and didn't crash.
  - A fake-server harness confirmed every message, the 100 ms heartbeat, and rumble reaching the haptics path.
  - Not verified (screen locked, no controller): capture and warp on screen, `kill -9` while captured, real pads and haptics, and the server's display restart.
- **Affected Rust:** `view/capture.rs` (new), `gamepad/` (new, `native.rs`), `view.rs`, `view/keys.rs`, `transport.rs`, `settings.rs`, `shortcuts.rs`, `app.rs`, `app/session_window.rs`, `app/controls.rs`, `crates/skerry-ui/src/{controls.rs,session.rs,assets.rs}`, `Cargo.toml`, `examples/preview.rs`.

## ARCH-034: JPEG Stripes as Their Own Images, Presented per Frame (Task I)

- **Status:** settled 2026-11-22. Settles `ARCH-GAP-VS-04`. Amends `ARCH-021`: JPEG frames are now presented on the bottom stripe, or after a 2 ms settle, not flushed every 8 ms. Amends `ARCH-003`: the JPEG path no longer composites into a full canvas.
- **Process:** plan → (no visible UI, so design N/A) → review (8 required changes) → build → QA (3 medium, 1 low, against targets) → fix round → code review (3 rounds, MERGE) by subagents (`ARCH-025`). Branch `task-i-jpeg`.
- **Findings:**
  - Since `ARCH-021`, the whole 11 MB canvas was cloned every 8 ms, and each flush built a new full-size RenderImage. That meant a new Metal texture and an 11 MB upload, even when one stripe changed.
  - VideoToolbox JPEG decoding is not hardware on Apple Silicon: it moves the work to VTDecoderXPCService. ImageIO was slower than zune-jpeg.
  - libjpeg-turbo needs cmake, which isn't available.
- **Decision:**
  - Each stripe becomes its own RenderImage. Only changed stripes are uploaded, and the canvas buffer is gone.
  - Decoding: zune-jpeg decodes to RGB with NEON into a reused scratch buffer, then one NEON pass makes BGRA. It compiles only on aarch64, with a scalar fallback elsewhere. Grayscale stripes are accepted.
  - Each stripe gets one padding row top and bottom, and every edge is snapped to whole device pixels, so there are no seams at any scale.
  - Replaced stripes are retired with `window.drop_image` 4 paints later, because GPUI does not wait for the GPU (`metal_renderer.rs`). Every exit (resize, codec switch, fallback, stream end, reconnect) goes through the retirer.
  - **Presentation:** a frame shows as soon as its bottom stripe arrives, since pixelflux sends stripes top to bottom. Otherwise it shows 2 ms after the last stripe, with the timer armed early to absorb tokio's rounding. The pacer resets on hardware frames, codec switch, and resize.
  - The transport's biased select runs input, gamepad heartbeat, key heartbeat, socket, then settle, so stripes never starve input.
  - `SKERRY_PRESENT_TRACE=1` logs each present's reason, delay, frame id, and stripe rows.
- **Measured** (QA's headless harness: the real transport against a fake server replaying real stripes; release builds; excludes GPUI/Metal, so the removed 11 MB texture upload per flush is extra savings on top):
  - Motion: instructions −27 %, cycles −27 to −30 %, CPU median about 27 % → 17–29 % (noisy).
  - Typing: cycles −38 %, and last-stripe-to-present p50 1.6 ms against a 0.7–1.9 ms baseline (target ≤ baseline + 1 ms, met).
  - Footprint 55 MB → 12–35 MB.
- **Not verified live** (screen locked): real-app CPU, WindowServer and VTDecoder deltas, GPU memory, visual seams and tearing, and the top-to-bottom stripe order on a live server (use the trace).
- **Affected Rust:** `transport.rs`, `view.rs`, `view/stripes.rs` (new), `Cargo.toml` (zune-jpeg 0.5).

## ARCH-035: Apps Catalog Window; Pelorus Excluded (Task J)

- **Status:** settled 2026-11-22. Settles `ARCH-GAP-DC-11`. Closes `ARCH-GAP-DC-13` (Pelorus) as excluded. Amends the index rule that Skerry talks only to SealSkin and Selkies: one keyless catalog fetch from GitHub is allowed.
- **Process:** plan → design → review (13 required changes) → build → QA (1 high: older Selkies; 1 medium: SVG CPU bombs) → code review (4 rounds; an entity bomb, a marker bomb, an ancestor-class bypass and a `url()` whitespace bypass found and fixed, then a worker process added) by subagents (`ARCH-025`). Branch `task-j-apps`.
- **Protocol (Selkies 715df7c, c72a9ef7 or newer):**
  - The client sends `apps,{"op":install|remove|update|launch|list|cancel|clear,...}` on the session WebSocket.
  - The server replies with the system actions `apps_state` (replaces the state) and `apps_job` (console `append`/`reset`), plus `command_error`.
  - Gated by `command_enabled`, and hidden by `ui_sidebar_show_apps`.
  - One shared `system_action` match in the transport routes these and `ARCH-032`'s rumble.
- **Decision:**
  - **Window:** one **Apps** window per session, like Files (`ARCH-023`): source list (Installed, All Apps, categories with counts, Activity), a 112×104 icon grid, and a 300 pt detail pane. Installed apps get Open, then Update and Remove…, with a confirm. The pane also has a monospace console with auto-scroll, Copy, and Cancel ("Canceling…", since cancel gets no reply). ⌘⌫ removes.
  - **Opening:** from a Session Controls header button and View → Apps, with no chord. It stays open across a reconnect, and teardown closes it.
  - **Support detection:** older Selkies drop the ops silently. Skerry sends `list` once per stream and waits 5 s for `apps_state`. If nothing comes, the button hides, the View item is disabled, and an open window shows "This session's Selkies is too old for the Apps catalog." A late reply re-enables it. There is no old-protocol fallback.
  - **Catalog:**
    - From the Selkies web client's GitHub raw URL: a separate HTTP client with no key, cookie or user agent, HTTPS only, at most 3 redirects, size-capped, 5 s timeout. This is the one sanctioned third-party fetch.
    - Or from the session's local repository (`api/apps/`, with the session cookie), whose generated entries may have no icon, description or category.
    - YAML is parsed under a budget.
  - **Icons (untrusted SVG):**
    - Refused outright: any `<!ENTITY` or internal DTD subset, `<marker>`, CSS escapes and `@import`, and `<use>` or element or depth counts over budget.
    - Removed rather than refused: filters, so drop shadows are dropped.
    - Checked: nested mask, clipPath and pattern references are resolved through attributes, `style=` and stylesheet classes on the element and its ancestors. `url()` is read exactly as svgtypes reads it, and anything unreadable counts as a reference.
    - SVGs up to 1 MiB.
    - Rasterising runs in a child process (`--skerry-svg-worker`, checked first in `main`). It has RLIMIT_CPU 2 s, a 3 s kill, at most 2 at a time, an empty environment, and only stdin and stdout. Output is size-capped and bounds-checked.
    - PNG icons are capped at 2048 px and 32 MB.
    - All 137 real catalog icons decode.
    - Textures retire through a queue drained in render, with a generation counter (`ARCH-014`).
- **Pelorus excluded (`DC-13`):** `/pelorus/api/run` is an agent loop of up to 50 steps that clicks, types and can run shell commands, so even a "describe" prompt can change the desktop. The panel is aimed at agents, and it needs a model key that Skerry would have to store. Revisit if VoiceOver-style reading of remote windows becomes a goal.
- **QA:**
  - Hostile inputs were refused in 0.03–25 ms.
  - The GitHub fetch got 161 apps in 218 ms with only `accept` and `host` headers.
  - The live server runs a Selkies older than c72a9ef7, so install, remove and the job console were not verified live, nor anything on screen (screen locked).
- **Affected Rust:** `apps.rs` (new), `app/apps_window.rs`, `transport.rs`, `app/session_window.rs`, `app/controls.rs`, `app.rs`, `main.rs`, `crates/skerry-ui/src/{apps.rs,controls.rs,assets.rs}`, `examples/preview.rs`, `tests/fixtures/proot-apps-metadata.yml`.

## ARCH-036: Direct Selkies Servers, with No SealSkin in Front (Task K)

- **Status:** settled 2026-11-22. Settles `ARCH-GAP-LS-13`. Amends `ARCH-011`: Selkies credentials may be kept in the same `0600` `config.toml`, never the Keychain. Amends `ARCH-009`: Selkies reports no version, so a capability gate replaces the version check.
- **Process:** plan → design → review (15 required changes) → build (live against a stock Selkies container) → QA (2 medium, against a local container) → code review (2 rounds, MERGE) by subagents (`ARCH-025`). Branch `task-k-selkies`.
- **Decision:**
  - **Connect card:**
    - A **SealSkin | Selkies** segmented switch. The Selkies side takes the browser link (a `#token=` fragment becomes the token, form-decoded), Username and Password, **Remember on this Mac**, and Recent Servers (up to 5).
    - Pasting a URL into the key field switches tabs.
    - Prior art: Windows App splits Add Workspace from Add PC, and Finder ⌘K and Screens use an address plus a login.
  - **Storage:**
    - Up to 5 `[[selkies]]` entries in `config.toml` (`url`, `username`, `password`, `token`, `certificate_sha256`), newest first, with a warning comment above them. One connection at a time.
    - The certificate pin is saved even when Remember is off.
    - `SKERRY_SELKIES_URL` and `SKERRY_SELKIES_CERT_SHA256` are read for QA and never saved.
  - **Auth:**
    - Basic is sent on HTTP requests and on the WebSocket upgrade.
    - A token goes as Bearer on HTTP, and on the WebSocket as the `selkies.token.` subprotocol plus the `selkies_token` cookie.
    - No Origin header.
    - Credentials go only to the configured origin, with redirects off.
    - A token from a pasted link is dropped when the address changes to another host.
  - **TLS:**
    - HTTPS only, plus plain HTTP to loopback. Userinfo is refused, and so is port 0.
    - The stock container makes its own certificate per install, so it is trusted once by the SHA-256 of the leaf certificate. The Mac's trust store and the hostname are checked first, and the pin is tried only when they fail. Signature checks stay with rustls.
    - A changed certificate is a hard stop, including on the session socket, with no startup retry.
    - rustls is ring-only (`default-features = false`, `builder_with_provider`). aws-lc-rs would make the SealSkin tungstenite connector panic, and `cargo tree -i aws-lc-rs` is empty.
  - **Gate (instead of a version):**
    - The open `GET /api/status` must report WebSockets mode. A WebRTC-mode server is refused with its own message.
    - A logged-in `GET /api/sessions` must answer 200.
    - `GET /api/tokens` tells secure mode apart, and a token is checked on its own with Bearer.
    - 401 bodies tell "needs password", "wrong password" and "needs token" apart.
    - A bad token shows up as Close 4001 after the upgrade, and is fatal, never retried.
  - **Library:**
    - One Desktop card (thumbnail from `api/screenshot`, login, an "n other pages connected" line, Files, and Open).
    - A server menu replaces the account menu: Address, Login, Security, Mode, Open in Browser (hidden for token-only servers), Disconnect, and Forget This Server… with a confirmation.
    - There is no catalog, launch, Stop, account, or Delete in Files (`ARCH-024`).
    - Stream, audio, clipboard, gamepads, printing, Files and Apps all work.
    - Connecting to Selkies clears SealSkin recovery, pending saves, and queued opens. A Refresh that fails on the network keeps the Library.
- **QA (live):** run against `selkies/desktop:latest-ubuntu26.04` on a remote host by the builder, and against a local Selkies 2.2.1 by QA. Basic login, Files, first-trust pin, pin mismatch after the container was recreated, wrong password, needs-token, bad token (no loop), WebRTC refused, and the `config.toml` rules all passed. No request reached another origin under redirects. Not verified: anything on screen (screen locked), and plain-HTTP localhost.
- **Affected Rust:** `tls.rs` (new), `config_store.rs` (moved out of `sealskin.rs`), `direct.rs`, `app/selkies_home.rs`, `selkies.rs`, `transport.rs`, `persist.rs`, `app.rs`, `app/session_window.rs`, `crates/skerry-ui/src/{connect.rs,selkies.rs,library.rs}`, `Cargo.toml`, `examples/preview.rs`.

## ARCH-038: Microphone Uplink (Task M1; camera follows as M2)

- **Status:** microphone settled 2026-11-22. `ARCH-GAP-DC-05` stays open for the camera (M2, being built) until M2 merges. Echo cancellation is deferred, as `ARCH-GAP-DC-16`.
- **Process:** plan → design → review (which cut echo cancellation and split the camera off) → build → QA (1 high, 1 medium) → code review (2 rounds, MERGE) by subagents (`ARCH-025`). Branch `task-m-mic`.
- **Protocol (Selkies 715df7c):**
  - Binary `0x02` plus one Opus packet: mono, 24 kHz, 20 ms.
  - The server sends `MICROPHONE_DISABLED` and `CAPTURE_DEMAND microphone <0|1>`.
  - Settings: `microphone_enabled` and `microphone_on_start` (true, false, or demand). "Not enabled but not locked" still allows the mic, as in the web client.
- **Decision:**
  - **Encoding:** macOS's own AudioToolbox Opus encoder (streaming `AudioConverterFillComplexBuffer` with a private "no data yet" status and per-packet descriptions), 32 kbps. It resamples and downmixes in the same step. No C library is added; the bindings would need cmake.
  - **Capture thread:** a per-session `skerry-mic` thread, following `ARCH-030`'s model. It exists only from a granted Start to Stop and opens the cpal device by UID.
  - **Device list:** comes from plain CoreAudio properties (`input_devices.rs`). Nothing touches the microphone or triggers a macOS privacy request before capture is asked for and granted.
  - **Starting:** on `on_start` true, on `CAPTURE_DEMAND`, or on the person's switch.
    - PM decision: a mic the server locks on starts like `on_start` (after permission), with the switch shown on and disabled.
    - The start policy runs only on the first `server_settings`.
    - Late permission answers are ignored.
    - PM decision: after a lock or `MICROPHONE_DISABLED`, the person's earlier "on" is not restored (the safe direction).
  - **Stopping:** one `stop()` runs on every stream end: Error, StartupTimeout, `end_stream` (Reconnect, close, disconnect), teardown, and quit.
    - The thread also stops itself when its sink closes, or when its frame channel stays full for 2 s.
    - A blocked thread is detached, never stacked.
    - A 10 s silence from the server (no message or pong) ends the stream, so a dead network can't keep the mic open.
  - **UI:**
    - A **Microphone & Camera** card after Audio, with a switch, a picker, a 5-segment level meter, the locked and denied states, and Open Privacy Settings.
    - An in-window badge (left 12, top 48/52/88, with no hitbox) shows while the device is open.
    - `NSMicrophoneUsageDescription` is in the bundle, and `bundle.sh` fails if the key is missing.
  - **Echo cancellation deferred (`ARCH-GAP-DC-16`):** VoiceProcessingIO would have to take over playback on the `ARCH-030` thread, losing its health checks, device-return swap and default-following, and its spike could pass falsely.
- **QA (fake-server harness, real AudioToolbox):**
  - `0x02` frames decode as 24 kHz mono, 480 samples, 53–123 B, and none are sent before ready.
  - `on_start`, demand, `MICROPHONE_DISABLED`, and locks all behave.
  - Every stream end closes the stream and ends `skerry-mic`, and 15 cycles leaked no threads.
  - Connecting causes no microphone privacy request (`preflight=true` only).
  - Not verified: real capture and the menu-bar indicator (permission was undetermined with the screen locked), the card on screen, a real Selkies with the mic enabled, and macOS 13 (Opus encoder availability). The encoder emits Hybrid TOC `0x68`.
- **Known:** a Start that lands while the old thread is still exiting stays at "Starting…" with no capture (safe). It is being fixed in M2.
- **Affected Rust:** `capture.rs`, `microphone.rs`, `permissions.rs`, `input_devices.rs`, `app/media_capture.rs` (all new), `transport.rs`, `app/session_window.rs`, `app/controls.rs`, `app.rs`, `crates/skerry-ui/src/{controls.rs,session.rs}`, `scripts/bundle.sh`, `Cargo.toml` (`objc2-av-foundation`).

## ARCH-037: Storage Window for SealSkin Homes and Shared Files (Task L1; public links deferred)

- **Status:** L1 settled 2026-11-22. `ARCH-GAP-LS-05` stays open only for public share links (L2), now tracked as `ARCH-GAP-LS-14`.
- **Process:** plan → design → review (17 required changes) → build → QA (1 medium) → code review (2 rounds, MERGE) by subagents (`ARCH-025`). Branch `task-l-storage`.
- **Server facts (SealSkin 0.5.0):**
  - There is no search route, so Skerry filters the folder shown, as the web UI does.
  - Transfers are 2 MiB base64 chunks, and uploads never overwrite.
  - `launch/file_path` opens only top-level Shared Files (`_sealskin_shared_files`) and refuses Cleanroom.
  - `DELETE /homedirs/_sealskin_shared_files` would wipe Shared Files.
  - A bad home name answers 409, not 422.
  - There is no upload-abort route, so a cancelled upload holds one of the 16 upload slots for 24 h.
- **Decision:**
  - **Window:** one **Storage** window per SealSkin server, hidden for direct Selkies servers (`ARCH-036`). It opens from a Library header button, **Manage Storage…** in the account menu, or Window → Storage ⇧⌘I (Library only).
    - The sidebar shows Shared Files, Homes, App Homes (with the `auto-` prefix hidden), and a quota meter.
    - The right side is the session Files pane, with its I/O behind `FilesBackend` (Selkies or SealSkin storage), so transfers, Quick Look, drags and ⌘C carry over.
    - The pane stays out of `OpenWindows.files`, and the window closes on Sign Out.
  - **Safety:**
    - No DELETE is ever sent for Shared Files or Cleanroom (names compared ignoring case, refused before any request).
    - Delete Home re-reads sessions and transfers both before and after the confirm, counting meta-app and in-flight launch homes, and refuses while a session uses the home.
    - Uploads into a home's `/Desktop/files` go to Shared Files, and mkdir there is refused.
    - Storage 403s (quota, storage off) never turn uploads off.
  - **Transfers:**
    - Streamed in chunks of at most 2 MiB. A retried upload finalize carries `X-Idempotency-Key = upload_id`.
    - A download cut off mid-body is treated as a network error and resumes.
    - A kept partial resumes only when its `com.skerry.partial` xattr (home, path, size, server mtime) matches the listing, and the final size is checked.
    - A folder whose listing is truncated (over 10,000 entries) fails rather than downloading part of it.
  - **Open in…:** Shared Files top level only, through `ARCH-028`'s chooser and Launch Dialog. It offers only apps with `home_directories`, or meta apps, then calls `launch/file_path` with a `launch_id` (`ARCH-027`).
  - **Public links (L2) deferred (`ARCH-GAP-LS-14`):** split off at review to keep L1 reviewable. A link would serve a copy of the file as it was when made, with password, expiry and revoke, where the server permits.
- **QA (live, harness):**
  - A QA home was created, filled and deleted.
  - Uploads: 2.6 MB in 0.48 s and 200 MB in 29.9 s (debug build), plus a nested folder, and cancel followed by upload.
  - Downloads ran at 21.5 MB/s with matching md5.
  - Quota matched `admin/status?storage=1`.
  - Open in… a PDF reached the session's `Desktop/files`.
  - Delete Home was refused while a session ran on the home.
  - A 571-request log had no Shared Files DELETE.
  - The session Files window still uploads and downloads (md5 checked).
  - Not verified: anything on screen.
- **Affected Rust:** `storage.rs`, `app/files_backend.rs`, `app/storage_window.rs` (new), `sealskin.rs`, `app/files_window.rs`, `app/account.rs`, `app.rs`, `crates/skerry-ui/src/{storage.rs,files.rs,library.rs,account.rs}`, `examples/preview.rs`.

## ARCH-039: Second Display Window (Task N)

- **Status:** settled 2026-11-22. Settles `ARCH-GAP-VS-01`. Amends `ARCH-022`: Skerry now opens a second connection to a session, for its `display2` only. Cross-window drag is deferred as `ARCH-GAP-VS-07`.
- **Process:** plan → design → review (12 required changes; drag deferred) → build → QA (1 medium) → code review (2 rounds, MERGE; merged after K, M1 and L1) by subagents (`ARCH-025`). Branch `task-n-display2`.
- **Protocol (Selkies 715df7c):**
  - A second `/api/websockets` connection claims `displayId:"display2"` with `displayPosition` in its first `SETTINGS`, and resizes are `r,WxH,display2`. Layout arrives in `DISPLAY_CONFIG_UPDATE`.
  - Any `SETTINGS` without `displayId` counts as the primary (`protocol.rs:280`, `handler.rs:779`), so one unstamped message from Display 2 would kill the main stream. The web client has this bug itself.
  - `KILL <reason>` comes with close code 1008. Gated by `second_screen`.
  - The server adds each display's offset to input. Display 2 gets no audio or stats, but it does get the print replay on connect.
- **Decision:**
  - **Transport:** `transport::run` gets a role (primary or secondary) instead of being forked, and is split into connect plus `stream()`.
    - Every outgoing frame goes through `transport/role.rs::Wire`. On Display 2 its only text send stamps, and overwrites, `displayId`, so a raw send doesn't compile. `send_binary` sends only on the primary (mic, `ARCH-038`).
    - The primary's bytes on the wire are unchanged.
  - **What Display 2 drops:** print events, audio and audio bitrate, stats, colour scheme, DPI, `js,`, mic, and incoming clipboard. Audio, stats, gamepad, print, file and Apps events never reach its window. It has no stats strip, print notices, capture hint or drop target.
  - **Window:** "{App} — Display 2", opened from **Add Screen +** in the Screen card or View → Second Display, shown when `second_screen` is true.
    - Placement follows the web dashboard's `addScreen`, using `CGDisplayBounds`, because GPUI display origins are always zero. With exactly one Mac display beside the session window, Display 2 opens there. Otherwise a side picker (Left, Right, Above, Below, with a diagram) asks.
    - Full screen is ⌃⌘F on whichever display it's on.
  - **Lifecycle:**
    - Display 2 resumes on the primary's first picture, including when `second_screen` becomes true later.
    - It goes to Waiting when the primary errors, is killed, or reconnects.
    - It closes with the primary, releasing keys and dropping its stream first, and ignores late events by generation.
    - Each stream has its own 10 s silence timeout.
    - On direct Selkies servers it uses the primary's `SessionTarget` (`ARCH-036`).
  - **KILL:** "Display 2 isn't available. {reason}" with Try Again and Close. A takeover of the primary reads "This session was opened somewhere else."
  - Pointer capture hands off to the primary. Gamepads stay on the primary's stream, with a 120 ms focus hand-off.
- **QA (live, `sealskin.kebot.me`, real transport, primary plus Display 2):**
  - Both streamed, with Display 2 placed at x=1024.
  - Every `SETTINGS` Display 2 sent carried `displayId` (resize, HiDPI, codec, position), and the primary was never killed.
  - Takeovers gave Superseded, and Display 2 resumed after the primary reconnected. Key releases went out before close, and no print, file or audio events reached Display 2.
  - SealSkin's proxy accepts the second WebSocket (R10).
  - Not verified: the GUI, placement across two Mac displays, and a server refusal.
- **Affected Rust:** `transport/role.rs` (new), `transport.rs`, `app/second_display.rs` (new), `app/session_window.rs`, `app/display.rs`, `app.rs`, `crates/skerry-ui/src/{session.rs,controls.rs,components.rs}`, `examples/preview.rs`.

---

## Active & Deferred Constraint Frontier (`ARCH-GAP-*`)

| ID | Owning Contract | Concept | Status | Summary |
|---|---|---|---|---|
| `ARCH-GAP-VS-01` | `video-and-screen` | `second-display` | settled (`ARCH-039`) | A Display 2 window on a stamped second connection, with no audio, placed beside the session window. |
| `ARCH-GAP-VS-07` | `video-and-screen` | `cross-display-drag` | deferred (`ARCH-039`) | Map a drag that crosses from one display window into the other. |
| `ARCH-GAP-VS-02` | `video-and-screen` | `webrtc-transport` | unresolved (backlog) | Prototype WebRTC signaling, negotiated codecs, ICE/STUN/TURN, and data-channel input vs. direct WebSocket. |
| `ARCH-GAP-VS-03` | `video-and-screen` | `extended-codecs-and-tuning` | unresolved (backlog) | Evaluate VP8/VP9 decode paths, 10-bit, 4:4:4, Turbo, paint-over, and CPU encoder options. |
| `ARCH-GAP-LS-01` | `launcher-and-sessions` | `launch-options-and-progress` | settled (`ARCH-013`, `ARCH-016`, `ARCH-027`) | Launch options, plus staged progress via `launch_id`, idempotent retries, timeout recovery, and one-click remembered launch. |
| `ARCH-GAP-LS-02` | `launcher-and-sessions` | `session-management` | settled (`ARCH-013`, `ARCH-028`) | Thumbnails, confirmed Stop, and a Files button that opens a session's Files window with no stream. |
| `ARCH-GAP-LS-03` | `launcher-and-sessions` | `open-links-and-files` | settled (`ARCH-028`) | Omnibar link chooser, ⌘O/⌘L, drops, Finder Open With and Dock, `skerry:` scheme, launch/url, launch/file, open_url. |
| `ARCH-GAP-LS-04` | `launcher-and-sessions` | `launcher-library` | settled (`ARCH-013`, `ARCH-016`, `ARCH-027`) | Search, icons, categories, favorites, hidden apps, recents (saved per server), and reuse of a matching running session. |
| `ARCH-GAP-LS-05` | `launcher-and-sessions` | `persistent-storage` | settled except links (`ARCH-037`) | Storage window: homes, Shared Files, quota, transfers with resume, and Open in…. |
| `ARCH-GAP-LS-14` | `launcher-and-sessions` | `public-share-links` | deferred (`ARCH-037`) | Public links with password, expiry and revoke, where the server permits (L2). |
| `ARCH-GAP-LS-06` | `launcher-and-sessions` | `account-and-key-recovery` | settled (`ARCH-029`) | Account menu with limits and meters, plus dormant / revoked / expired / disabled / held key recovery that keeps the old key until a new one connects. |
| `ARCH-GAP-LS-07` | `launcher-and-sessions` | `cluster-and-session-origins` | unresolved (todo) | Automatic/pool/node placement and validated token handoff to isolated per-session origins without leaking control-plane keys. |
| `ARCH-GAP-LS-08` | `launcher-and-sessions` | `multi-window-sessions` | settled (`ARCH-022`) | Always-separate-windows model (`ARCH-010`): Launcher stays in its own window; every launched/resumed session opens in its own native macOS window, supporting multiple concurrent sessions of the same app. |
| `ARCH-GAP-LS-09` | `launcher-and-sessions` | `native-mac-app-presence` | unresolved (backlog) | Per-app native macOS presence (Dock icon, `Cmd+Tab`, optionally Spotlight); mechanism (`.app` shims vs. helper processes vs. both) and existing-session relaunch behavior deferred to future compose decision. |
| `ARCH-GAP-DC-01` | `desktop-and-collaboration` | `clipboard-sync` | settled (`ARCH-017`) | Bridge Selkies clipboard messages to `NSPasteboard` honoring server direction policy, Unicode, focus changes, and loop prevention. |
| `ARCH-GAP-DC-02` | `desktop-and-collaboration` | `file-transfers` | settled (`ARCH-023`, `ARCH-024`) | Per-session Files window over Selkies' HTTP file routes: browse, New Folder, upload/download with progress and Cancel, Finder copy/paste and drag-and-drop both ways. |
| `ARCH-GAP-DC-14` | `desktop-and-collaboration` | `session-file-mount` | future scope | Mount session files in Finder via a File Provider extension, a local WebDAV server, or a local NFS server. |
| `ARCH-GAP-DC-15` | `desktop-and-collaboration` | `remote-rename-move` | blocked (server) | Delete settled via SealSkin storage (`ARCH-024`); no server offers rename or move, and Cleanroom/direct-Selkies sessions cannot delete. |
| `ARCH-GAP-DC-03` | `desktop-and-collaboration` | `audio-devices-and-quality` | settled (`ARCH-030`) | Follows the macOS default, Output Device picker by UID, Core Audio conversion at 48 kHz, recovery with backoff, Audio Bitrate. |
| `ARCH-GAP-DC-04` | `desktop-and-collaboration` | `remote-printing` | settled (`ARCH-033`) | One-shot PDF fetch, notices with Print…/Open/Save…, the macOS print sheet, Open Print Dialog Automatically, and cache cleanup. |
| `ARCH-GAP-DC-05` | `desktop-and-collaboration` | `microphone-and-webcam` | microphone settled (`ARCH-038`); camera in progress (M2) | Mic: AudioToolbox Opus `0x02` uplink, TCC-safe listing, stops on every stream end. Camera: VideoToolbox H.264 `0x06` uplink, being built. |
| `ARCH-GAP-DC-16` | `desktop-and-collaboration` | `mic-echo-cancellation` | deferred (`ARCH-038`) | VoiceProcessingIO would have to own playback, which breaks `ARCH-030`'s output thread guarantees. |
| `ARCH-GAP-DC-06` | `desktop-and-collaboration` | `gaming-controls` | settled (`ARCH-032`) | Pointer capture with ⌃⌥⇧Z and `m2` relative motion; GameController pads on slots 0–3 per session with rumble; custom chords dropped. |
| `ARCH-GAP-DC-07` | `desktop-and-collaboration` | `session-sharing-and-rooms` | unresolved (todo) | Selkies view-only / player-slot roles and SealSkin collaboration room evaluation (against 0.5.0 rooms, whose media/cursor messages carry only the sender's alias). |
| `ARCH-GAP-DC-08` | `desktop-and-collaboration` | `multitouch-trackpad` | deferred | Revisit raw multitouch contact forwarding only when Chrome gesture-to-display latency matches native macOS pinch/pan. |
| `ARCH-GAP-VS-04` | `video-and-screen` | `jpeg-cpu-cost` | settled (`ARCH-034`) | Per-stripe RenderImages, NEON decode, present on the bottom stripe or after a 2 ms settle; about 27–30 % fewer cycles. |
| `ARCH-GAP-VS-05` | `video-and-screen` | `session-controls-ux` | mostly settled (`ARCH-018`, `ARCH-026`) | Settled: web-sidebar cards, section memory per server, scroll per window, theme-aware dropdown shadow. Open: the web's stream-toggle tile row (`ARCH-020`). |
| `ARCH-GAP-LS-10` | `launcher-and-sessions` | `launcher-polish` | settled (`ARCH-026`) | Sign Out… with confirmation, 1200 pt Library column, success toasts, live "Started … ago". |
| `ARCH-GAP-LS-13` | `launcher-and-sessions` | `direct-selkies-sessions` | settled (`ARCH-036`) | Connect to a standalone Selkies server: Basic or token login, a pinned self-signed certificate, up to 5 servers in `config.toml`, and a Desktop card. |
| `ARCH-GAP-LS-11` | `launcher-and-sessions` | `control-plane-hardening` | settled (`ARCH-026`) | Key only on `/companion/api/`, single FIFO write queue off the main thread, cached Library items. |
| `ARCH-GAP-DC-09` | `desktop-and-collaboration` | `stray-super-key` | settled client-side (`ARCH-026`) | Key releases are sent before the socket closes; no `kr`; `KeyLedger`. A live input trace is still to be taken. |
| `ARCH-GAP-DC-10` | `desktop-and-collaboration` | `clipboard-shortcuts` | settled (`ARCH-017`) | ⌘C/⌘X/⌘V stay on the Mac while Command sends Control is off; sessions use Control+C/X/V. |
| `ARCH-GAP-VS-06` | `video-and-screen` | `stats-overlay` | settled (`ARCH-031`) | The web's over-the-stream strip, with no hitbox and saved per server; the floating gauges were not built. |
| `ARCH-GAP-DC-11` | `desktop-and-collaboration` | `apps-catalog` | settled (`ARCH-035`) | An Apps window per session over `apps,{op}`. Support is detected, the GitHub catalog is fetched with no credentials, and icons are rasterised in a sandboxed worker process. |
| `ARCH-GAP-DC-12` | `desktop-and-collaboration` | `session-shortcuts` | settled (`ARCH-031`) | A Shortcuts card listing `shortcuts::CHORDS`, with "Mac shortcuts active" handing them to the session as Ctrl chords; ⌘Q always stays local. |
| `ARCH-GAP-DC-13` | `desktop-and-collaboration` | `pelorus` | excluded (`ARCH-035`) | An agent loop that can act on the desktop and needs a model key; not surfaced to people. |
