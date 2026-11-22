---
name: launcher-and-sessions
kind: responsibility
id: 067NC4KG01SKERRY0000000003
---

# Launcher and Sessions Domain

<!-- compose:status=provisional provenance=compose-managed -->
<!-- compose:unresolved id=ARCH-GAP-LS-14 concept="public-share-links" question="Storage window settled (ARCH-037). Still open: public share links with password, expiry, and revocation (L2)." -->
<!-- compose:unresolved id=ARCH-GAP-LS-07 concept="cluster-and-session-origins" question="Support Automatic/pool/node placement and validate trusted per-session origins and token handoff while keeping application keys strictly on the control-plane origin." -->
<!-- compose:unresolved id=ARCH-GAP-LS-09 concept="native-mac-app-presence" question="Give remote applications first-class native macOS app presence (separate Dock icon, Cmd+Tab entry, and optionally Spotlight); decide in a future compose turn between generated .app shims, automatic per-app helper processes, or both, and how relaunch behaves when a session is already running." -->

### Goal

Manage the complete SealSkin control-plane lifecycle: credential authentication and recovery, application discovery and organization, configurable launching with startup progress, active session supervision (resume, preview, send file, stop), persistent storage management, and cluster/session-origin routing.

### Requires

- `api-and-launcher-source`: `crates/skerry-mac/src/sealskin.rs`, `crates/skerry-mac/src/app.rs`, and `crates/skerry-mac/src/settings.rs`

### Maintains

- `credential-and-connect` (settled; account and recovery in `ARCH-029`):
  - Parses `ssk_<id>_<secret>@<host>[:port]`, enforces HTTPS with no path/query/userinfo, loads from `SEALSKIN_APPLICATION_KEY` -> `.env` -> `application_key` in `~/.config/skerry/config.toml` (saved only after a successful Connect from the key field, `0600` file in a `0700` folder, other settings and comments in the file preserved, removed on **Disconnect Server**; the macOS Keychain is never read or written, `ARCH-011`), and authenticates with `Authorization: Bearer <key>` and `x-sealskin-app: other`.
  - Minimum server version (settled, `ARCH-009`): before loading the library on Connect or Refresh, `Api::check_version` reads `GET /companion/api/ui/version` and refuses any server older than `0.5.0` (`MINIMUM_SEALSKIN_VERSION`), a `0.5.0` pre-release, an unparseable version, or a missing route. A refused server leaves Skerry on the Connect screen with an error naming the minimum and found versions. This also applies on Refresh: the Library and its textures are dropped, and Connect with an empty key field checks the same server again (`LibraryError::Refused`, decided 2026-11-21); the library is not loaded and the key is not saved to `~/.config/skerry/config.toml`.
  - Account and recovery (settled: `ARCH-029`):
    - The account menu shows `admin/status` (name, groups, rights, limits with meters, storage used, key source, and sleep time) and holds **Sign Out…**.
    - A dormant key gets **Sign In on the Web** (`<origin>/?wake=1`, never with the key in the URL), with a 5 s poll for up to 3 min.
    - A revoked or expired key returns to Connect for a new key while the old one stays saved. Disabled, client, inactive and held keys get their own notices.
    - Refusals on Refresh, Launch, and Stop all go through the same flow.
- `application-library-and-launch` (Library, search, icons, and Launch Dialog settled in `ARCH-013`; launch options settled in `ARCH-016`; progress & organization open in `ARCH-GAP-LS-01`, `ARCH-GAP-LS-03`, `ARCH-GAP-LS-04`):
  - Loads the library via `POST /companion/api/applications`, `GET /companion/api/sessions`, `POST /companion/api/admin/status` (launch rights, GPUs, VPNs; optional), and `GET /companion/api/homedirs` (with persistent storage), and launches via `POST /companion/api/launch/simple` from the **Launch Dialog**. Escape, the close button, or a backdrop click dismiss it. The backdrop occludes the Library, so a backdrop click never also activates what is underneath.
  - Launch options match SealSkin's web launcher (`ARCH-016`, `launch_options.rs`): **Storage** (Auto, Cleanroom, named homes; hidden without persistent storage; Auto creates and sends `auto-<app>`), **GPU** ("None" plus the GPUs that fit the app; hidden without GPU rights or a fitting GPU), **VPN** and **VPN endpoint** (endpoint only with two or more), searchable **Language** labelled with the Mac's locale names and defaulting to the locale nearest the Mac's, **Collaborative Session**, **Wayland Mode**, the Mac's time zone sent with every launch, and **Copy link** carrying the non-default options. Options are remembered per app after a successful launch and reused where they still hold.
  - Search filters applications and running sessions by name; category chips filter applications. App icons come from `GET /companion/api/app_icon/<id>` (meta apps) or the same-origin `logo` path, decoded with size limits and downscaled to 256 px.
  - Library organization (settled: `ARCH-027`):
    - Favorites, hidden apps and recents (up to 8) are saved per server in the preferences `library` key. The Library shows Running sessions, Favorites, Recent, then Applications.
    - Hidden apps leave All and the categories but still match a search, and a **Hidden (n)** chip lists them.
    - Favorite and Hide are offered in the Launch Dialog and in a tile right-click menu.
    - A running session with the same app, storage and Collaborative flag turns the dialog's primary button into **Open Running Session**.
  - Launch progress (settled: `ARCH-027`):
    - Every launch carries a `launch_id`, sent again as `X-Idempotency-Key`. A launching card follows `GET /companion/api/launch/progress/<id>` (Requesting, Pulling image, Starting, Connecting) for up to 960 s.
    - The card offers Keep Waiting / Retry on a stall and Retry / Dismiss on a failure, without ever starting a second container.
    - A session window that is still waiting offers Keep Waiting / Retry / Disconnect.
    - Tiles with remembered options launch in one click (hover ▶ or Option-click).
  - Open links and files (settled: `ARCH-028`):
    - A typed or pasted link offers **Open Here** in compatible running sessions (`POST /companion/api/sessions/<id>/open_url`) or **Launch & Open** in `url_support` apps (`launch/url`). Only http, https, ftp, and space-free magnet links are sent.
    - Files from ⌘O, drops, Finder Open With, or the Dock go to apps filtered by extension (`launch/file`, streamed in 1 MiB chunks) or to a running session's Desktop.
    - Nothing launches without a click.
- `running-sessions-supervision` (list/resume/disconnect, thumbnails, and Stop settled in `ARCH-013`; Send File open in `ARCH-GAP-LS-02`):
  - Lists sessions via `GET /companion/api/sessions` and resumes over Selkies WebSocket; disconnecting leaves the remote session running. Session cards show a thumbnail (`GET /companion/api/sessions/<id>/screenshot`, downscaled to 960 px), the app icon, start time, a GPU pill, and a storage pill ("Cleanroom" when the session has no home).
  - **Stop** (labelled button) asks for confirmation that explains the remote app ends and cleanroom data is deleted, then calls the session stop API; Escape or Cancel keeps the session.
  - A just-launched session's WebSocket is retried for up to 45 s while it answers 404/502/503/504 or refuses connections (`ARCH-014`).
  - Files from the launcher (settled: `ARCH-028`): each running-session card has a Files button that opens that session's Files window with no stream. It gets the session cookie through `transport::session_access`, without sending the application key, and there is one Files window per session.
- `launcher-polish-and-hardening` (settled: `ARCH-026`):
  - **Sign Out…** (was Disconnect Server) asks for confirmation. The wording names where the key came from (`config.toml`, environment, `.env`) and says open session windows stay connected. `application_key` is removed only when `config.toml` holds the active key.
  - The Library content is a centred column capped at 1200 pt. Success toasts ("Library refreshed", "Link copied") show for 3 s at the bottom centre. "Started … ago" updates every 30 s while the Library is visible.
  - The application key is attached only to requests under `/companion/api/`. Logos and icons use a client with no key.
  - Preference and key writes go through the single first-in-first-out write queue in `persist.rs` on Tokio. Its in-memory copy serves every read, it flushes within 1 s on quit, and an unreadable file is backed up before it is overwritten.
- `persistent-storage-and-shares` (settled: `ARCH-037`; public links open in `ARCH-GAP-LS-14`):
  - A per-server **Storage** window (⇧⌘I, Manage Storage…) browses Shared Files and homes through the Files pane on a SealSkin `FilesBackend`, and shows quota.
  - Homes can be created, and deleted only when no session uses them. Skerry never deletes Shared Files.
  - Transfers use 2 MiB chunks, resume only from a matching partial, and are idempotent on retry.
  - **Open in…** launches a stored Shared File in a compatible app via `launch/file_path`.
- `cluster-and-session-origins` (unresolved: `ARCH-GAP-LS-07`):
  - Automatic/pool/node placement, storage move evaluation, and validated token handoff to isolated per-session origins without leaking the control-plane application key.
- `direct-selkies-sessions` (settled: `ARCH-036`):
  - Connect's **Selkies** side reaches a standalone Selkies server with a link, Basic login or token, through `SelkiesClient` alone.
  - Credentials and the leaf-certificate SHA-256 pin live in `0600` `config.toml` `[[selkies]]` entries (at most 5).
  - The gate is WebSockets mode plus an authorised `api/sessions`.
  - The Library shows one Desktop card and a server menu. SealSkin-only features (catalog, launch, Stop, account, Delete) are hidden.
- `multi-window-sessions` (settled: `ARCH-010`, `ARCH-022`):
  - Resuming a session that is already open brings its window to the front. Skerry never opens a second connection to it.
  - The Library window can close while sessions stream. **Window → Library** (`cmd-0`) or a Dock-icon click reopens it. `cmd-q` disconnects every session and stops none.
  - A session window closes from its own buttons, which appear when the pointer enters the top-left corner, or from **Window → Disconnect Session**. `cmd-w` stays with the remote app.
  - Codec, input, clipboard, and video changes are saved per server for the next session. Other open windows keep theirs. Display mode, resolution, mute, and pause are per window.
  - Always-separate-windows model: the Launcher stays in its own window while every launched or resumed session opens in its own dedicated native macOS (`gpui-kit` `Root`-wrapped) window, allowing multiple concurrent sessions—including multiple sessions of the same application—side by side.
- `native-mac-app-presence` (in scope via `ARCH-010`; mechanism & relaunch behavior unresolved: `ARCH-GAP-LS-09`):
  - First-class native macOS app identity per remote application (separate Dock icon, `Cmd+Tab` entry, and optionally Spotlight). Delivery mechanism (generated `.app` shims vs. automatic per-app helper processes vs. both) and relaunch behavior when a session is already running are deferred to a future compose turn.
  - Prior art (research only, `ARCH-010` note): Pake compiles a Tauri app per site, which is too heavy for Skerry. Its unused prebuilt `runtime-template` points to a stub-`.app`-copy approach.

Facets:
- `auth-and-account` — config-file credentials (`~/.config/skerry/config.toml`), account limits, dormant/expired recovery, and cluster origin security
- `launcher` — application catalog, search/favorites, launch options, startup progress, and URL/file opening
- `sessions-and-storage` — running session previews/stop and SealSkin persistent homes/shares

Material: SealSkin companion API endpoints, credential/origin security policies, launcher UI states, and per-app launch preferences.
Immaterial: transient session timestamps and server polling intervals.

Postconditions:
- Skerry never loads a library from, launches on, or saves a key for a SealSkin server whose `GET /companion/api/ui/version` is older than `0.5.0` or cannot be read (`ARCH-009`).
- Application keys never leave the SealSkin control-plane origin and are never sent to untrusted redirects.
- Disconnecting never stops a remote session; stopping a session requires explicit user confirmation explaining cleanroom data loss.
- Closing a session window disconnects only that session and never stops the remote container or closes the Launcher or other open session windows.

### Continuity

input-driven
