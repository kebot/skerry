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
- **Decision:** Skerry supports SealSkin `0.5.0` and every newer version (`api::MINIMUM_SEALSKIN_VERSION = (0, 5, 0)`). Before any other companion API call, `Api::check_version` reads `GET /companion/api/ui/version` and parses `version` as SemVer `MAJOR.MINOR.PATCH[-pre][+build]`. Skerry refuses to continue when the version is older than `0.5.0`, is a pre-release of `0.5.0` (e.g. `0.5.0-rc1`, which SemVer orders before `0.5.0`), is not a plain three-part version (e.g. `0.5`, `v0.5.0`), the route is missing (servers before 0.5.0), or the reply is unreadable. A refused server leaves Skerry on the Connect screen with an error naming the minimum and the found version; the library is not loaded, the `Api` is not stored, and the key is not saved to Keychain (since `ARCH-011`: to `~/.config/skerry/config.toml`). The same gate runs on every Refresh.
- **Rationale:** 0.5.0 is the first release with the behaviour these contracts rely on (`ARCH-008` lists it: per-origin session cookie exchange, alias-only room messages, launch address scheme rule, `open_url`, Selkies 2.3.0 images, forced hardening). A minimum rather than an exact pin lets operators take SealSkin patch and feature releases without waiting for a Skerry update.
- **Risk accepted:** Newer SealSkin releases are accepted without being verified. When a release changes a route or behaviour Skerry relies on, raising the minimum (or fixing Skerry) is a new decision that supersedes this one.
- **Refresh (decided 2026-11-21, was `ARCH-GAP-LS-12`):** Before, a refusal during Refresh showed the error and kept the old Library on screen. Now `Api::library` returns `LibraryError::Refused` for any version-gate refusal, and the Launcher goes back to the Connect screen, dropping the Library and its textures. Connect with an empty key field checks that server again. Network failures (`LibraryError::Failed`) still keep the Library and show the error. This has not been tested live, because no server older than 0.5.0 is available.
- **Open verification:** ~~Re-run `ARCH-003` codec QA on a 0.5.0 server~~ done in QA 2026-11-21 against SealSkin 0.5.x (JPEG, H.264, H.265, AV1; `ARCH-012`).
- **Affected Contracts:** `src/skerry/index.prose.md`, `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-mac/src/api.rs` (`MINIMUM_SEALSKIN_VERSION`, `parse_version`, `Api::check_version`, `Api::library`, `LibraryError`), `crates/skerry-mac/src/app.rs` (`Launcher::load`, `leave_server`)

## ARCH-010: Multi-Window Session Architecture & Native Mac App Presence Scope

- **Status:** settled (scope & multi-window model; implementation tracked in `ARCH-GAP-LS-08`, native Mac app presence mechanism deferred to `ARCH-GAP-LS-09`)
- **Decision:**
  - **Multi-window sessions (`ARCH-GAP-LS-08`):** Adopt an always-separate-windows model. The Launcher stays open in its own window, and every launched or resumed remote session opens in its own dedicated native macOS (`gpui-kit` `Root`-wrapped) window. Multiple concurrent sessions—including multiple sessions of the same remote application—can run side by side, each with independent stream/audio/controls lifecycle.
  - **Native Mac app presence (`ARCH-GAP-LS-09`):** Add per-app native macOS identity (separate Dock icon, `Cmd+Tab` entry, and optionally Spotlight) to Skerry's scope. The delivery mechanism (on-demand generated `.app` shims vs. automatic per-app helper processes vs. both) and relaunch behavior when a session for that app is already running are intentionally left open for a future compose decision.
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
| **Files** | Opens the file manager: browse the session's directories, upload and download (shown when the server allows either transfer) | — | `ARCH-GAP-DC-02` |
| **Printing** | Shown once a document arrives: auto-print switch and every printed document with Print and Save (`printDocument`, `printRequest`) | — | `ARCH-GAP-DC-04` |
| **Apps** | "Manage Apps" opens the proot-apps catalog by category with search, install / remove / update / launch, and each job's console; needs the server's `command_enabled` | — | `ARCH-GAP-DC-11` |
| **Sharing** | Copyable links for the viewer (`#shared`) and controller slots (`#player2`–`#player4`), each gated by its server setting | — | `ARCH-GAP-DC-07` |
| **Gamepads** | Gamepad input switch, touch gamepad switch, Rumble on/off (`gamepad_rumble`), live picture of each pad (`gamepadButtonUpdate`, `gamepadAxisUpdate`) | — | `ARCH-GAP-DC-06` |
| **Shortcuts** | The dashboard's own chords (Ctrl+Shift+F fullscreen, Ctrl+Shift+X gaming mode, Ctrl+Shift+M dashboard, Ctrl+Shift+G virtual gamepad, Ctrl+Shift+Left Click pointer lock) and a "Shortcuts active" switch that hands them to the session instead | `cmd-shift-m` opens Session Controls; Send Key Combination (Alt+Tab, Ctrl+Alt+Delete) in **Input** | `ARCH-GAP-DC-12` |
| **Pelorus** (first, when the server serves `/pelorus`) | A prompt against the server's Pelorus API, which describes the desktop as text (windows, positions, accessibility tree) for a caller without vision, with a pop-out page | — | `ARCH-GAP-DC-13` |
| Footer | "Cite our paper academically" link | — | Excluded |

- **Affected Contracts:** `src/skerry/video-and-screen.prose.md`, `src/skerry/desktop-and-collaboration.prose.md`

---

## Active & Deferred Constraint Frontier (`ARCH-GAP-*`)

| ID | Owning Contract | Concept | Status | Summary |
|---|---|---|---|---|
| `ARCH-GAP-VS-01` | `video-and-screen` | `second-display` | unresolved (backlog) | Open a second remote display in its own GPUI window following server display placement while primary session owns audio. |
| `ARCH-GAP-VS-02` | `video-and-screen` | `webrtc-transport` | unresolved (backlog) | Prototype WebRTC signaling, negotiated codecs, ICE/STUN/TURN, and data-channel input vs. direct WebSocket. |
| `ARCH-GAP-VS-03` | `video-and-screen` | `extended-codecs-and-tuning` | unresolved (backlog) | Evaluate VP8/VP9 decode paths, 10-bit, 4:4:4, Turbo, paint-over, and CPU encoder options. |
| `ARCH-GAP-LS-01` | `launcher-and-sessions` | `launch-options-and-progress` | partially settled (`ARCH-013`, `ARCH-016`) | Settled: storage (Auto/Cleanroom/named homes), GPU, VPN, language, timezone, Wayland, collaborative, remembered options, Copy link options. Open: startup progress stages and one-click remembered launch. |
| `ARCH-GAP-LS-02` | `launcher-and-sessions` | `session-management` | partially settled (`ARCH-013`) | Settled: thumbnails and confirmed Stop. Open: Send File. |
| `ARCH-GAP-LS-03` | `launcher-and-sessions` | `open-links-and-files` | unresolved (todo) | Open pasted URLs in the omnibar and local files via native macOS mechanisms (File → Open… `⌘O`, window drag-and-drop, and `.app` bundle `CFBundleDocumentTypes` / `on_open_urls` Finder "Open With" — no "Upload a file" omnibar button) via SealSkin 0.5.0 `POST /api/launch/url`, `/api/launch/file`, `/api/launch/file_path`, and `POST /api/sessions/<id>/open_url`; only `http`, `https`, `ftp`, or space-free `magnet` addresses are sent. |
| `ARCH-GAP-LS-04` | `launcher-and-sessions` | `launcher-library` | partially settled (`ARCH-013`, `ARCH-016`) | Settled: search (apps + sessions), app icons, category filters, per-app launch preferences. Open: recents, favorites, hidden apps, and matching running-session reuse. |
| `ARCH-GAP-LS-05` | `launcher-and-sessions` | `persistent-storage` | unresolved (todo) | SealSkin storage API integration for homes, shared files, direct remote file opening, and public share links. |
| `ARCH-GAP-LS-06` | `launcher-and-sessions` | `account-and-key-recovery` | unresolved (todo) | Show signed-in account, capabilities, and limits; dormant-key browser wake flow and expired/revoked key replacement. |
| `ARCH-GAP-LS-07` | `launcher-and-sessions` | `cluster-and-session-origins` | unresolved (todo) | Automatic/pool/node placement and validated token handoff to isolated per-session origins without leaking control-plane keys. |
| `ARCH-GAP-LS-08` | `launcher-and-sessions` | `multi-window-sessions` | unresolved (todo) | Always-separate-windows model (`ARCH-010`): Launcher stays in its own window; every launched/resumed session opens in its own native macOS window, supporting multiple concurrent sessions of the same app. |
| `ARCH-GAP-LS-09` | `launcher-and-sessions` | `native-mac-app-presence` | unresolved (backlog) | Per-app native macOS presence (Dock icon, `Cmd+Tab`, optionally Spotlight); mechanism (`.app` shims vs. helper processes vs. both) and existing-session relaunch behavior deferred to future compose decision. |
| `ARCH-GAP-DC-01` | `desktop-and-collaboration` | `clipboard-sync` | settled (`ARCH-017`) | Bridge Selkies clipboard messages to `NSPasteboard` honoring server direction policy, Unicode, focus changes, and loop prevention. |
| `ARCH-GAP-DC-02` | `desktop-and-collaboration` | `file-transfers` | unresolved (todo) | In-session Selkies file upload/download with native macOS Open/Save dialogs, progress, cancel, and folder/ZIP support. |
| `ARCH-GAP-DC-03` | `desktop-and-collaboration` | `audio-devices-and-quality` | unresolved (todo) | Output device selection, sample-rate conversion, sleep/wake recovery, and server-allowed audio bitrate controls. |
| `ARCH-GAP-DC-04` | `desktop-and-collaboration` | `remote-printing` | unresolved (todo) | Receive remote PDF print jobs with macOS Save and Print actions and optional auto-open preference. |
| `ARCH-GAP-DC-05` | `desktop-and-collaboration` | `microphone-and-webcam` | unresolved (todo) | Audio/video capture uplinks with macOS permission requests on demand, capture indicators, and disconnect teardown. |
| `ARCH-GAP-DC-06` | `desktop-and-collaboration` | `gaming-controls` | unresolved (todo) | Relative pointer capture with escape shortcut, physical gamepad forwarding with slot/rumble support, and custom chords. |
| `ARCH-GAP-DC-07` | `desktop-and-collaboration` | `session-sharing-and-rooms` | unresolved (todo) | Selkies view-only / player-slot roles and SealSkin collaboration room evaluation (against 0.5.0 rooms, whose media/cursor messages carry only the sender's alias). |
| `ARCH-GAP-DC-08` | `desktop-and-collaboration` | `multitouch-trackpad` | deferred | Revisit raw multitouch contact forwarding only when Chrome gesture-to-display latency matches native macOS pinch/pan. |
| `ARCH-GAP-VS-04` | `video-and-screen` | `jpeg-cpu-cost` | unresolved (todo) | JPEG mode uses 36–65 % CPU (software decode + full-canvas copy every 33 ms). |
| `ARCH-GAP-VS-05` | `video-and-screen` | `session-controls-ux` | partially settled (`ARCH-018`) | Settled: web-sidebar cards, dropdowns, sliders, switches. Open: Session Controls forgets section state/scroll; popovers lack shadow; the web's stream-toggle tile row (`ARCH-020`). |
| `ARCH-GAP-LS-10` | `launcher-and-sessions` | `launcher-polish` | unresolved (todo) | Disconnect Server confirmation, wide-window Library density, Refresh/Copy feedback, live "Started … ago". |
| `ARCH-GAP-LS-11` | `launcher-and-sessions` | `control-plane-hardening` | unresolved (todo) | No Bearer key on logo fetches, off-main-thread preference/key writes, fewer per-render clones. |
| `ARCH-GAP-DC-09` | `desktop-and-collaboration` | `stray-super-key` | unresolved (needs repro) | Remote start menu opened during a codec switch while focus moved; possible unreleased Cmd/Super. |
| `ARCH-GAP-DC-10` | `desktop-and-collaboration` | `clipboard-shortcuts` | settled (`ARCH-017`) | ⌘C/⌘X/⌘V stay on the Mac while Command sends Control is off; sessions use Control+C/X/V. |
| `ARCH-GAP-VS-06` | `video-and-screen` | `stats-overlay` | unresolved (backlog) | The web's "Show over the stream" strip (status marks, fps, video rate vs target, capture-to-sent, RTT over the picture, no pointer input) and its compact/detailed floating gauges (FPS, Mbps, RTT, CPU, GPU, MEM, VRAM half-circles) (`ARCH-020`). |
| `ARCH-GAP-DC-11` | `desktop-and-collaboration` | `apps-catalog` | unresolved (backlog) | The web's Apps panel: proot-apps catalog by category with search, install/remove/update/launch and job consoles, gated by the server's `command_enabled` (`ARCH-020`). |
| `ARCH-GAP-DC-12` | `desktop-and-collaboration` | `session-shortcuts` | unresolved (backlog) | A Shortcuts panel listing Skerry's own chords (`cmd-shift-m`, full screen, and any gaming-mode / pointer-lock chord from `ARCH-GAP-DC-06`) with a switch that hands them to the remote session instead, as the web's "Shortcuts active" (`ARCH-020`). |
| `ARCH-GAP-DC-13` | `desktop-and-collaboration` | `pelorus` | unresolved (backlog) | The web's Pelorus panel: a prompt against the server's `/pelorus/api`, which describes the desktop as text for a caller without vision; evaluate whether Skerry should surface it (`ARCH-020`). |
