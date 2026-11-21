---
name: launcher-and-sessions
kind: responsibility
id: 067NC4KG01SKERRY0000000003
---

# Launcher and Sessions Domain

<!-- compose:status=provisional provenance=compose-managed -->
<!-- compose:unresolved id=ARCH-GAP-LS-01 concept="launch-options-and-progress" question="Storage (auto/cleanroom), language, Wayland, and collaborative mode are settled in the Launch Dialog (ARCH-013). Still open: named homes, timezone, GPU/VPN options, stage progress and cold-pull timeout recovery, and one-click remembered launch." -->
<!-- compose:unresolved id=ARCH-GAP-LS-02 concept="session-management" question="Thumbnails and Stop with a data-loss confirmation are settled (ARCH-013). Still open: Send File from the launcher." -->
<!-- compose:unresolved id=ARCH-GAP-LS-03 concept="open-links-and-files" question="Accept pasted URLs in the omnibar and local files via native macOS mechanisms (File -> Open... Cmd+O, window drag-and-drop, and .app bundle CFBundleDocumentTypes / on_open_urls Finder 'Open With' handler — no web-style 'Upload a file' omnibar button), filter apps by declared URL/extension support, and launch with open-or-stage progress via SealSkin 0.5.0 /api/launch/url|file|file_path, or hand a URL to a running session via POST /api/sessions/<id>/open_url; send only http, https, ftp, or space-free magnet addresses." -->
<!-- compose:unresolved id=ARCH-GAP-LS-04 concept="launcher-library" question="Search (apps and sessions), app icons, and category filters are settled (ARCH-013). Still open: recents, favorites and hidden apps (where they are saved, and how a hidden app is shown again), per-app saved launch preferences, and running-session reuse matching app + storage." -->
<!-- compose:unresolved id=ARCH-GAP-LS-05 concept="persistent-storage" question="Integrate SealSkin storage APIs for homes, shared files (browse/search/mkdir/upload/download/launch-stored-file), and permitted public links with password/expiry/revocation." -->
<!-- compose:unresolved id=ARCH-GAP-LS-06 concept="account-and-key-recovery" question="Display signed-in account, effective capabilities, and usage limits; offer browser wake flow for dormant keys and replacement prompt for expired/revoked keys." -->
<!-- compose:unresolved id=ARCH-GAP-LS-07 concept="cluster-and-session-origins" question="Support Automatic/pool/node placement and validate trusted per-session origins and token handoff while keeping application keys strictly on the control-plane origin." -->
<!-- compose:unresolved id=ARCH-GAP-LS-08 concept="multi-window-sessions" question="Keep the Launcher in its own window and open every launched or resumed remote session in its own dedicated gpui-kit macOS window (including multiple concurrent sessions of the same app), with per-session Session Controls and independent teardown." -->
<!-- compose:unresolved id=ARCH-GAP-LS-09 concept="native-mac-app-presence" question="Give remote applications first-class native macOS app presence (separate Dock icon, Cmd+Tab entry, and optionally Spotlight); decide in a future compose turn between generated .app shims, automatic per-app helper processes, or both, and how relaunch behaves when a session is already running." -->
<!-- compose:unresolved id=ARCH-GAP-LS-10 concept="launcher-polish" question="Disconnect Server forgets the saved key with no confirmation; at wide windows (~2540 pt) about 75 % of the Library is empty; Refresh and Copy link give little lasting feedback; 'Started … ago' only updates on render. Add a confirm (or rename to Sign Out), a max content width or denser grid, and transient success toasts?" -->
<!-- compose:unresolved id=ARCH-GAP-LS-11 concept="control-plane-hardening" question="Icon fetches send the Bearer application key to any same-origin `logo` path, which shares an origin with container-served session paths (matters once per-session origins land, ARCH-GAP-LS-07); preference and key writes fsync on the main thread; Library render clones the app list once and the session list twice per frame. Use an unauthenticated client (or an allow-listed prefix) for logos, move writes to the Tokio runtime, and borrow instead of cloning?" -->

### Goal

Manage the complete SealSkin control-plane lifecycle: credential authentication and recovery, application discovery and organization, configurable launching with startup progress, active session supervision (resume, preview, send file, stop), persistent storage management, and cluster/session-origin routing.

### Requires

- `api-and-launcher-source`: `crates/skerry-mac/src/api.rs`, `crates/skerry-mac/src/app.rs`, and `crates/skerry-mac/src/settings.rs`

### Maintains

- `credential-and-connect` (settled baseline; recovery open in `ARCH-GAP-LS-06`):
  - Parses `ssk_<id>_<secret>@<host>[:port]`, enforces HTTPS with no path/query/userinfo, loads from `SEALSKIN_APPLICATION_KEY` -> `.env` -> `application_key` in `~/.config/skerry/config.toml` (saved only after a successful Connect from the key field, `0600` file in a `0700` folder, other settings and comments in the file preserved, removed on **Disconnect Server**; the macOS Keychain is never read or written, `ARCH-011`), and authenticates with `Authorization: Bearer <key>` and `x-sealskin-app: other`.
  - Minimum server version (settled, `ARCH-009`): before loading the library on Connect or Refresh, `Api::check_version` reads `GET /companion/api/ui/version` and refuses any server older than `0.5.0` (`MINIMUM_SEALSKIN_VERSION`), a `0.5.0` pre-release, an unparseable version, or a missing route. A refused server leaves Skerry on the Connect screen with an error naming the minimum and found versions. This also applies on Refresh: the Library and its textures are dropped, and Connect with an empty key field checks the same server again (`LibraryError::Refused`, decided 2026-11-21); the library is not loaded and the key is not saved to `~/.config/skerry/config.toml`.
  - Unresolved frontier (`ARCH-GAP-LS-06`): show account limits/capabilities, dormant-key browser wake flow, and expired/revoked key replacement.
- `application-library-and-launch` (Library, search, icons, and Launch Dialog settled in `ARCH-013`; remaining options & organization open in `ARCH-GAP-LS-01`, `ARCH-GAP-LS-03`, `ARCH-GAP-LS-04`):
  - Loads library via `POST /companion/api/applications` and launches via `POST /companion/api/launch/simple` with `home_name`, `language`, `wayland_mode`, and `launch_in_room_mode` from the **Launch Dialog** (Storage: Auto/Cleanroom, Language, Collaborative Session, Wayland Mode, Copy link). Escape, the close button, or a backdrop click dismiss it. The backdrop occludes the Library, so a backdrop click never also activates what is underneath.
  - Search filters applications and running sessions by name; category chips filter applications. App icons come from `GET /companion/api/app_icon/<id>` (meta apps) or the same-origin `logo` path, decoded with size limits and downscaled to 256 px.
  - Favorite / Hide are not shown until they are saved somewhere (`ARCH-GAP-LS-04`); the Launch Dialog never offers a control that does nothing.
  - Unresolved frontier (`ARCH-GAP-LS-01`): named homes, timezone, GPU/VPN selection, startup progress stages, and one-click remembered launch.
  - Unresolved frontier (`ARCH-GAP-LS-03`): open pasted URLs and local files in compatible remote apps.
  - Unresolved frontier (`ARCH-GAP-LS-04`): recents, favorites, hidden apps, per-app launch preferences, and matching session reuse.
- `running-sessions-supervision` (list/resume/disconnect, thumbnails, and Stop settled in `ARCH-013`; Send File open in `ARCH-GAP-LS-02`):
  - Lists sessions via `GET /companion/api/sessions` and resumes over Selkies WebSocket; disconnecting leaves the remote session running. Session cards show a thumbnail (`GET /companion/api/sessions/<id>/screenshot`, downscaled to 960 px), the app icon, start time, a GPU pill, and a storage pill ("Cleanroom" when the session has no home).
  - **Stop** (labelled button) asks for confirmation that explains the remote app ends and cleanroom data is deleted, then calls the session stop API; Escape or Cancel keeps the session.
  - A just-launched session's WebSocket is retried for up to 45 s while it answers 404/502/503/504 or refuses connections (`ARCH-014`).
  - Unresolved frontier (`ARCH-GAP-LS-02`): Send File from the launcher.
- `launcher-polish` (unresolved: `ARCH-GAP-LS-10`) and `control-plane-hardening` (unresolved: `ARCH-GAP-LS-11`): open findings from QA 2026-11-21.
- `persistent-storage-and-shares` (unresolved: `ARCH-GAP-LS-05`):
  - Manage SealSkin homes, shared files, direct remote file opening without re-upload, and public share links (password, expiry, revocation).
- `cluster-and-session-origins` (unresolved: `ARCH-GAP-LS-07`):
  - Automatic/pool/node placement, storage move evaluation, and validated token handoff to isolated per-session origins without leaking the control-plane application key.
- `multi-window-sessions` (design settled in `ARCH-010`; implementation unresolved: `ARCH-GAP-LS-08`):
  - Always-separate-windows model: the Launcher stays in its own window while every launched or resumed session opens in its own dedicated native macOS (`gpui-kit` `Root`-wrapped) window, allowing multiple concurrent sessions—including multiple sessions of the same application—side by side.
- `native-mac-app-presence` (in scope via `ARCH-010`; mechanism & relaunch behavior unresolved: `ARCH-GAP-LS-09`):
  - First-class native macOS app identity per remote application (separate Dock icon, `Cmd+Tab` entry, and optionally Spotlight). Delivery mechanism (generated `.app` shims vs. automatic per-app helper processes vs. both) and relaunch behavior when a session is already running are deferred to a future compose turn.

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
- Once `ARCH-GAP-LS-08` is implemented, closing a session window disconnects only that session and never stops the remote container or closes the Launcher or other open session windows.

### Continuity

input-driven
