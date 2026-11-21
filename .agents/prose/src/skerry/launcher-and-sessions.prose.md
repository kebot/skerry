---
name: launcher-and-sessions
kind: responsibility
id: 067NC4KG01SKERRY0000000003
---

# Launcher and Sessions Domain

<!-- compose:status=provisional provenance=compose-managed -->
<!-- compose:unresolved id=ARCH-GAP-LS-01 concept="launch-options-and-progress" question="Expose storage mode (automatic per-app, cleanroom, named home), language/timezone, Wayland/X11, and GPU/VPN options with stage progress and cold-pull timeout recovery while keeping one-click remembered launch." -->
<!-- compose:unresolved id=ARCH-GAP-LS-02 concept="session-management" question="Display server thumbnails for running sessions and add Send File and Stop (distinct from Disconnect, warning that Stop terminates remote process and cleanroom data)." -->
<!-- compose:unresolved id=ARCH-GAP-LS-03 concept="open-links-and-files" question="Accept pasted URLs and dropped/chosen files in the launcher, filter apps by declared URL/extension support, and launch with open-or-stage progress via SealSkin 0.5.0 /api/launch/url|file|file_path, or hand a URL to a running session via POST /api/sessions/<id>/open_url; send only http, https, ftp, or space-free magnet addresses." -->
<!-- compose:unresolved id=ARCH-GAP-LS-04 concept="launcher-library" question="Add search, app icons, recents, favorites, hidden apps, per-app saved launch preferences, and running-session reuse matching app + storage." -->
<!-- compose:unresolved id=ARCH-GAP-LS-05 concept="persistent-storage" question="Integrate SealSkin storage APIs for homes, shared files (browse/search/mkdir/upload/download/launch-stored-file), and permitted public links with password/expiry/revocation." -->
<!-- compose:unresolved id=ARCH-GAP-LS-06 concept="account-and-key-recovery" question="Display signed-in account, effective capabilities, and usage limits; offer browser wake flow for dormant keys and replacement prompt for expired/revoked keys." -->
<!-- compose:unresolved id=ARCH-GAP-LS-07 concept="cluster-and-session-origins" question="Support Automatic/pool/node placement and validate trusted per-session origins and token handoff while keeping application keys strictly on the control-plane origin." -->

### Goal

Manage the complete SealSkin control-plane lifecycle: credential authentication and recovery, application discovery and organization, configurable launching with startup progress, active session supervision (resume, preview, send file, stop), persistent storage management, and cluster/session-origin routing.

### Requires

- `api-and-launcher-source`: `crates/skerry-mac/src/api.rs`, `crates/skerry-mac/src/app.rs`, and `crates/skerry-mac/src/settings.rs`

### Maintains

- `credential-and-connect` (settled baseline; recovery open in `ARCH-GAP-LS-06`):
  - Parses `ssk_<id>_<secret>@<host>[:port]`, enforces HTTPS with no path/query/userinfo, loads from `SEALSKIN_APPLICATION_KEY` -> `.env` -> macOS Keychain, and authenticates with `Authorization: Bearer <key>` and `x-sealskin-app: other`.
  - Minimum server version (settled, `ARCH-009`): before loading the library on Connect or Refresh, `Api::check_version` reads `GET /companion/api/ui/version` and refuses any server older than `0.5.0` (`MINIMUM_SEALSKIN_VERSION`), a `0.5.0` pre-release, an unparseable version, or a missing route. A refused server leaves Skerry on the Connect screen with an error naming the minimum and found versions; the library is not loaded and the key is not saved to Keychain.
  - Unresolved frontier (`ARCH-GAP-LS-06`): show account limits/capabilities, dormant-key browser wake flow, and expired/revoked key replacement.
- `application-library-and-launch` (baseline simple launch settled; options & library organization open in `ARCH-GAP-LS-01`, `ARCH-GAP-LS-03`, `ARCH-GAP-LS-04`):
  - Loads library via `POST /companion/api/applications` and launches via `POST /companion/api/launch/simple`.
  - Unresolved frontier (`ARCH-GAP-LS-01`): storage mode (automatic, cleanroom, named home), language/timezone, Wayland/X11, GPU/VPN selection, startup progress stages, and one-click remembered launch.
  - Unresolved frontier (`ARCH-GAP-LS-03`): open pasted URLs and local files in compatible remote apps.
  - Unresolved frontier (`ARCH-GAP-LS-04`): search, icons, recents, favorites, hidden apps, and matching session reuse.
- `running-sessions-supervision` (baseline list/resume/disconnect settled; previews & stop open in `ARCH-GAP-LS-02`):
  - Lists sessions via `GET /companion/api/sessions` and resumes over Selkies WebSocket; disconnecting leaves the remote session running.
  - Unresolved frontier (`ARCH-GAP-LS-02`): session thumbnail previews, Send File from launcher, and explicit Stop with cleanroom data warning.
- `persistent-storage-and-shares` (unresolved: `ARCH-GAP-LS-05`):
  - Manage SealSkin homes, shared files, direct remote file opening without re-upload, and public share links (password, expiry, revocation).
- `cluster-and-session-origins` (unresolved: `ARCH-GAP-LS-07`):
  - Automatic/pool/node placement, storage move evaluation, and validated token handoff to isolated per-session origins without leaking the control-plane application key.

Facets:
- `auth-and-account` — Keychain credentials, account limits, dormant/expired recovery, and cluster origin security
- `launcher` — application catalog, search/favorites, launch options, startup progress, and URL/file opening
- `sessions-and-storage` — running session previews/stop and SealSkin persistent homes/shares

Material: SealSkin companion API endpoints, credential/origin security policies, launcher UI states, and per-app launch preferences.
Immaterial: transient session timestamps and server polling intervals.

Postconditions:
- Skerry never loads a library from, launches on, or saves a key for a SealSkin server whose `GET /companion/api/ui/version` is older than `0.5.0` or cannot be read (`ARCH-009`).
- Application keys never leave the SealSkin control-plane origin and are never sent to untrusted redirects.
- Disconnecting never stops a remote session; stopping a session requires explicit user confirmation explaining cleanroom data loss.

### Continuity

input-driven
