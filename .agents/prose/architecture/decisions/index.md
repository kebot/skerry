# Skerry Architecture Decisions & Constraint Lattice

Canonical settled behavior lives in `.agents/prose/src/skerry/*.prose.md`. This directory holds supporting ADR records (`ARCH-NNN.md`) and the active/deferred frontier (`ARCH-GAP-*`). Read this index first and open individual `ARCH-NNN.md` files only when working on that area.

## Decisions (`ARCH-*`)

| ID | Title | Status |
|---|---|---|
| [`ARCH-001`](./ARCH-001.md) | Attached OpenProse Root & Harness Selection | settled |
| [`ARCH-002`](./ARCH-002.md) | Native OpenProse Responsibility Package Replaces Legacy `docs/` & Tickets | settled |
| [`ARCH-003`](./ARCH-003.md) | Settled Video & Screen Baseline (Milestone 1 + Hardware Codecs) | settled |
| [`ARCH-004`](./ARCH-004.md) | Settled Input, Trackpad Gestures & Local IME Baseline | settled |
| [`ARCH-005`](./ARCH-005.md) | Product & Protocol Boundaries (SealSkin 0.4.2 & Selkies 715df7c) | settled — SealSkin 0.4.2 version baseline superseded by `ARCH-008`, itself superseded b... |
| [`ARCH-006`](./ARCH-006.md) | gpui-kit UI Design System & Root-Wrapped Window Ownership | settled |
| [`ARCH-007`](./ARCH-007.md) | Pure `skerry-ui` Screen Library & Interactive Preview Workbench | settled |
| [`ARCH-008`](./ARCH-008.md) | Pinned SealSkin Server Version (0.5.0 Only) | superseded by `ARCH-009` — the user meant 0.5.0 as a minimum, not an exact version; kep... |
| [`ARCH-009`](./ARCH-009.md) | Minimum SealSkin Server Version (0.5.0 or Newer) | settled (pinned minimum) — supersedes `ARCH-008` |
| [`ARCH-010`](./ARCH-010.md) | Multi-Window Session Architecture & Native Mac App Presence Scope | settled (scope & multi-window model; implemented in `ARCH-022`, native Mac app presence... |
| [`ARCH-011`](./ARCH-011.md) | Application Key Stored in `~/.config/skerry/config.toml`, Not the macOS Keychain | settled — replaces the Keychain store (`dev.kebot.sealskin` / `application-key`) that `... |
| [`ARCH-012`](./ARCH-012.md) | H.265 and AV1 Predicted Frames on Selkies 2.3.0 Follow Continuity, Not the Reference Field | settled — amends the `ARCH-003` codec baseline for SealSkin 0.5 / Selkies 2.3.0 servers |
| [`ARCH-013`](./ARCH-013.md) | Settled Launcher Library, Launch Dialog, Thumbnails, and Stop Session | settled — partially settles `ARCH-GAP-LS-01`, `ARCH-GAP-LS-02`, `ARCH-GAP-LS-04` |
| [`ARCH-014`](./ARCH-014.md) | Session Resilience, Modal Hit-Testing, and Library Texture Lifecycle | settled |
| [`ARCH-015`](./ARCH-015.md) | Async Launcher Updates Resolve Their Window by Handle | settled |
| [`ARCH-016`](./ARCH-016.md) | Launch Dialog Options Match SealSkin's Web Launcher | settled — settles more of `ARCH-GAP-LS-01` (named homes, timezone, GPU, VPN) and the pe... |
| [`ARCH-017`](./ARCH-017.md) | Clipboard Sync Follows the Selkies Web Client, Triggered by Mac Focus | settled 2026-11-21 — settles `ARCH-GAP-DC-01` and `ARCH-GAP-DC-10`. |
| [`ARCH-018`](./ARCH-018.md) | Session Controls Look and Work Like the Selkies Web Sidebar | settled — design approved in the preview workbench on 2026-11-21; amends the Session Co... |
| [`ARCH-019`](./ARCH-019.md) | Stats Section Follows the Selkies Web Sidebar's Stats | settled — design approved in the preview workbench on 2026-11-21; amends the 1-second S... |
| [`ARCH-020`](./ARCH-020.md) | Selkies Web Sidebar Panel Inventory (Reference) | reference — records what each panel of Selkies' web sidebar holds, so future Session Co... |
| [`ARCH-021`](./ARCH-021.md) | Decoded Frames Are Presented as They Arrive, Not on a 30 Hz Tick | settled — amends the presentation pacing of `ARCH-003` (`codec-and-rate-control`) |
| [`ARCH-022`](./ARCH-022.md) | Multi-Window Session Behaviour (`ARCH-GAP-LS-08`) | settled (decided and verified live 2026-11-22). |
| [`ARCH-023`](./ARCH-023.md) | A Native Files Window per Session over Selkies' HTTP File Routes | settled 2026-11-22 (built, tests/Clippy/fmt pass, and confirmed working live by the per... |
| [`ARCH-024`](./ARCH-024.md) | Two Clients — `SealSkinClient` (Control Plane) and `SelkiesClient` (Session) — and Delete Through SealSkin Home Storage | settled 2026-11-21 — amends `ARCH-023` (delete was out of scope) and narrows `ARCH-GAP-... |
| [`ARCH-025`](./ARCH-025.md) | Frontier Build Plan of 2026-11-22 (PM Triage) | settled 2026-11-22 (planning decision; each task settles its own `ARCH-*` when it ships). |
| [`ARCH-026`](./ARCH-026.md) | Launcher Polish, Control-Plane Hardening, Session Controls Memory, and Key Release Before Close (Task A) | settled 2026-11-22. |
| [`ARCH-027`](./ARCH-027.md) | Favorites, Hidden Apps, Recents, Session Reuse, Launch Progress, and One-Click Launch (Task B) | settled 2026-11-22. |
| [`ARCH-028`](./ARCH-028.md) | Open Links and Files in Remote Apps, and Files from the Launcher (Task C) | settled 2026-11-22. |
| [`ARCH-029`](./ARCH-029.md) | Account Menu and Key Recovery (Task D) | settled 2026-11-22. Settles `ARCH-GAP-LS-06`. |
| [`ARCH-030`](./ARCH-030.md) | Audio Output Follows the Mac, with a Device Picker, Recovery, and Audio Bitrate (Task E) | settled 2026-11-22. Settles `ARCH-GAP-DC-03`. |
| [`ARCH-031`](./ARCH-031.md) | Stats Strip over the Stream and a Shortcuts Card (Task F) | settled 2026-11-22. |
| [`ARCH-032`](./ARCH-032.md) | Pointer Capture and Gamepads (Task G) | settled 2026-11-22. |
| [`ARCH-033`](./ARCH-033.md) | Remote Printing Through the macOS Print Dialog (Task H) | settled 2026-11-22. Settles `ARCH-GAP-DC-04`. |
| [`ARCH-034`](./ARCH-034.md) | JPEG Stripes as Their Own Images, Presented per Frame (Task I) | settled 2026-11-22. |
| [`ARCH-035`](./ARCH-035.md) | Apps Catalog Window; Pelorus Excluded (Task J) | settled 2026-11-22. |
| [`ARCH-036`](./ARCH-036.md) | Direct Selkies Servers, with No SealSkin in Front (Task K) | settled 2026-11-22. |
| [`ARCH-037`](./ARCH-037.md) | Storage Window for SealSkin Homes and Shared Files (Task L1; public links deferred) | L1 settled 2026-11-22. |
| [`ARCH-038`](./ARCH-038.md) | Microphone and Camera Uplinks (Tasks M1 and M2) | settled 2026-11-22: the microphone (M1) and then the camera (M2). |
| [`ARCH-039`](./ARCH-039.md) | Second Display Window (Task N) | settled 2026-11-22. |
| [`ARCH-040`](./ARCH-040.md) | Skerry App Identity & Free macOS Release Pipeline | settled |

## Open & Deferred Frontier (`ARCH-GAP-*`)

| ID | Owning Contract | Concept | Status | Summary |
|---|---|---|---|---|
| `ARCH-GAP-DC-17` | `desktop-and-collaboration` | `multichannel-surround-audio` | unresolved | Decode Selkies `pcmflux` multistream Opus surround (5.1 / 6ch, 7.1 / 8ch), open multichannel Core Audio streams with speaker layout mapping, and downmix for stereo/mono outputs. |
| `ARCH-GAP-DC-14` | `desktop-and-collaboration` | `session-file-mount` | future scope | Mount session files in Finder via a File Provider extension, a local WebDAV server, or a local NFS server. |
| `ARCH-GAP-DC-15` | `desktop-and-collaboration` | `remote-rename-move` | blocked (server) | Delete settled via SealSkin storage (`ARCH-024`); no server offers rename or move, and Cleanroom/direct-Selkies sessions cannot delete. |
| `ARCH-GAP-DC-16` | `desktop-and-collaboration` | `mic-echo-cancellation` | deferred (`ARCH-038`) | VoiceProcessingIO would have to own playback, which breaks `ARCH-030`'s output thread guarantees. |
| `ARCH-GAP-DC-07` | `desktop-and-collaboration` | `session-sharing-and-rooms` | deferred (`ARCH-025`) | Selkies view-only / player-slot roles and SealSkin collaboration room evaluation (against 0.5.0 rooms, whose media/cursor messages carry only the sender's alias). |
| `ARCH-GAP-DC-08` | `desktop-and-collaboration` | `multitouch-trackpad` | deferred | Revisit raw multitouch contact forwarding only when Chrome gesture-to-display latency matches native macOS pinch/pan. |
| `ARCH-GAP-LS-07` | `launcher-and-sessions` | `cluster-and-session-origins` | deferred (`ARCH-025`) | Automatic/pool/node placement and validated token handoff to isolated per-session origins without leaking control-plane keys. |
| `ARCH-GAP-LS-09` | `launcher-and-sessions` | `native-mac-app-presence` | deferred (`ARCH-025`) | Per-app native macOS presence (Dock icon, `Cmd+Tab`, optionally Spotlight); mechanism (`.app` shims vs. helper processes vs. both) and existing-session relaunch behavior deferred to future compose decision. |
| `ARCH-GAP-LS-14` | `launcher-and-sessions` | `public-share-links` | deferred (`ARCH-037`) | Public links with password, expiry and revoke, where the server permits (L2). |
| `ARCH-GAP-VS-02` | `video-and-screen` | `webrtc-transport` | deferred (`ARCH-025`) | Prototype WebRTC signaling, negotiated codecs, ICE/STUN/TURN, and data-channel input vs. direct WebSocket. |
| `ARCH-GAP-VS-03` | `video-and-screen` | `extended-codecs-and-tuning` | deferred (`ARCH-025`) | Evaluate VP8/VP9 decode paths, 10-bit, 4:4:4, Turbo, paint-over, and CPU encoder options. |
| `ARCH-GAP-VS-07` | `video-and-screen` | `cross-display-drag` | deferred (`ARCH-039`) | Map a drag that crosses from one display window into the other. |

## Settled & Excluded Gaps

| ID | Owning Contract | Concept | Status | Summary |
|---|---|---|---|---|
| `ARCH-GAP-VS-01` | `video-and-screen` | `second-display` | settled (`ARCH-039`) | A Display 2 window on a stamped second connection, with no audio, placed beside the session window. |
| `ARCH-GAP-VS-04` | `video-and-screen` | `jpeg-cpu-cost` | settled (`ARCH-034`) | Per-stripe RenderImages, NEON decode, present on the bottom stripe or after a 2 ms settle; about 27–30 % fewer cycles. |
| `ARCH-GAP-VS-05` | `video-and-screen` | `session-controls-ux` | settled except tile row (`ARCH-018`, `ARCH-026`) | Section memory, scroll, and dropdown shadow are settled. The web's stream-toggle tile row is deferred (`ARCH-025`), since every toggle in it is already in Session Controls. |
| `ARCH-GAP-VS-06` | `video-and-screen` | `stats-overlay` | settled (`ARCH-031`) | The web's over-the-stream strip, with no hitbox and saved per server; the floating gauges were not built. |
| `ARCH-GAP-LS-01` | `launcher-and-sessions` | `launch-options-and-progress` | settled (`ARCH-013`, `ARCH-016`, `ARCH-027`) | Launch options, plus staged progress via `launch_id`, idempotent retries, timeout recovery, and one-click remembered launch. |
| `ARCH-GAP-LS-02` | `launcher-and-sessions` | `session-management` | settled (`ARCH-013`, `ARCH-028`) | Thumbnails, confirmed Stop, and a Files button that opens a session's Files window with no stream. |
| `ARCH-GAP-LS-03` | `launcher-and-sessions` | `open-links-and-files` | settled (`ARCH-028`) | Omnibar link chooser, ⌘O/⌘L, drops, Finder Open With and Dock, `skerry:` scheme, launch/url, launch/file, open_url. |
| `ARCH-GAP-LS-04` | `launcher-and-sessions` | `launcher-library` | settled (`ARCH-013`, `ARCH-016`, `ARCH-027`) | Search, icons, categories, favorites, hidden apps, recents (saved per server), and reuse of a matching running session. |
| `ARCH-GAP-LS-05` | `launcher-and-sessions` | `persistent-storage` | settled except links (`ARCH-037`) | Storage window: homes, Shared Files, quota, transfers with resume, and Open in…. |
| `ARCH-GAP-LS-06` | `launcher-and-sessions` | `account-and-key-recovery` | settled (`ARCH-029`) | Account menu with limits and meters, plus dormant / revoked / expired / disabled / held key recovery that keeps the old key until a new one connects. |
| `ARCH-GAP-LS-08` | `launcher-and-sessions` | `multi-window-sessions` | settled (`ARCH-022`) | Always-separate-windows model (`ARCH-010`): Launcher stays in its own window; every launched/resumed session opens in its own native macOS window, supporting multiple concurrent sessions of the same app. |
| `ARCH-GAP-LS-10` | `launcher-and-sessions` | `launcher-polish` | settled (`ARCH-026`) | Sign Out… with confirmation, 1200 pt Library column, success toasts, live "Started … ago". |
| `ARCH-GAP-LS-11` | `launcher-and-sessions` | `control-plane-hardening` | settled (`ARCH-026`) | Key only on `/companion/api/`, single FIFO write queue off the main thread, cached Library items. |
| `ARCH-GAP-LS-13` | `launcher-and-sessions` | `direct-selkies-sessions` | settled (`ARCH-036`) | Connect to a standalone Selkies server: Basic or token login, a pinned self-signed certificate, up to 5 servers in `config.toml`, and a Desktop card. |
| `ARCH-GAP-DC-01` | `desktop-and-collaboration` | `clipboard-sync` | settled (`ARCH-017`) | Bridge Selkies clipboard messages to `NSPasteboard` honoring server direction policy, Unicode, focus changes, and loop prevention. |
| `ARCH-GAP-DC-02` | `desktop-and-collaboration` | `file-transfers` | settled (`ARCH-023`, `ARCH-024`) | Per-session Files window over Selkies' HTTP file routes: browse, New Folder, upload/download with progress and Cancel, Finder copy/paste and drag-and-drop both ways. |
| `ARCH-GAP-DC-03` | `desktop-and-collaboration` | `audio-devices-and-quality` | settled (`ARCH-030`) | Follows the macOS default, Output Device picker by UID, Core Audio conversion at 48 kHz, recovery with backoff, Audio Bitrate. |
| `ARCH-GAP-DC-04` | `desktop-and-collaboration` | `remote-printing` | settled (`ARCH-033`) | One-shot PDF fetch, notices with Print…/Open/Save…, the macOS print sheet, Open Print Dialog Automatically, and cache cleanup. |
| `ARCH-GAP-DC-05` | `desktop-and-collaboration` | `microphone-and-webcam` | settled (`ARCH-038`) | Mic: AudioToolbox Opus `0x02`. Camera: VideoToolbox H.264 `0x06` with adaptive rate. Both are TCC-safe, primary-only, and stop on every stream end. |
| `ARCH-GAP-DC-06` | `desktop-and-collaboration` | `gaming-controls` | settled (`ARCH-032`) | Pointer capture with ⌃⌥⇧Z and `m2` relative motion; GameController pads on slots 0–3 per session with rumble; custom chords dropped. |
| `ARCH-GAP-DC-09` | `desktop-and-collaboration` | `stray-super-key` | settled client-side (`ARCH-026`) | Key releases are sent before the socket closes; no `kr`; `KeyLedger`. A live input trace is still to be taken. |
| `ARCH-GAP-DC-10` | `desktop-and-collaboration` | `clipboard-shortcuts` | settled (`ARCH-017`) | ⌘C/⌘X/⌘V stay on the Mac while Command sends Control is off; sessions use Control+C/X/V. |
| `ARCH-GAP-DC-11` | `desktop-and-collaboration` | `apps-catalog` | settled (`ARCH-035`) | An Apps window per session over `apps,{op}`. Support is detected, the GitHub catalog is fetched with no credentials, and icons are rasterised in a sandboxed worker process. |
| `ARCH-GAP-DC-12` | `desktop-and-collaboration` | `session-shortcuts` | settled (`ARCH-031`) | A Shortcuts card listing `shortcuts::CHORDS`, with "Mac shortcuts active" handing them to the session as Ctrl chords; ⌘Q always stays local. |
| `ARCH-GAP-DC-13` | `desktop-and-collaboration` | `pelorus` | excluded (`ARCH-035`) | An agent loop that can act on the desktop and needs a model key; not surfaced to people. |
