---
name: desktop-and-collaboration
kind: responsibility
id: 067NC4KG01SKERRY0000000004
---

# Desktop Integration and Collaboration Domain

<!-- compose:status=partially-settled provenance=compose-managed -->
<!-- compose:unresolved id=ARCH-GAP-DC-14 concept="session-file-mount" question="Future scope: mount the session's files in Finder through a File Provider extension, a local WebDAV server, or a local NFS server. Not started; revisit after the Files window ships." -->
<!-- compose:unresolved id=ARCH-GAP-DC-15 concept="remote-rename-move" question="Delete is settled through SealSkin home storage (ARCH-024). Neither Selkies 715df7c nor SealSkin has a rename or move file route, and Cleanroom or direct Selkies sessions have no delete at all; revisit when a server offers them." -->
<!-- compose:unresolved id=ARCH-GAP-DC-05 concept="microphone-and-webcam" question="Negotiate audio/video uplink formats, request macOS capture permissions on demand, show active capture indicators, and stop capture on disconnect." -->
<!-- compose:unresolved id=ARCH-GAP-DC-07 concept="session-sharing-and-rooms" question="Support Selkies view-only and player-slot roles and evaluate SealSkin collaboration rooms (room API, token handoff, control transfer)." -->
<!-- compose:unresolved id=ARCH-GAP-DC-08 concept="multitouch-trackpad" question="Keep raw multitouch contact forwarding deferred until gesture-to-display latency in Chrome matches native macOS pinch/pan and contact release on focus loss is proven." -->

### Goal

Integrate the remote Selkies desktop seamlessly with macOS input, trackpad gestures, IME composition, `NSPasteboard` clipboard, native file dialogs, Core Audio devices, PDF printing, media capture, gamepads, and multi-user sharing.

### Requires

- `input-view-and-media-source`: `crates/skerry-mac/src/input.rs`, `crates/skerry-mac/src/view.rs`, `crates/skerry-mac/src/view/pinch.rs`, `crates/skerry-mac/src/audio.rs`, `crates/skerry-mac/src/transport.rs`, and `crates/skerry-mac/src/app/controls.rs`

### Maintains

- `keyboard-cursor-trackpad-and-ime` (settled; raw multitouch deferred in `ARCH-GAP-DC-08`):
  - Forwards mouse, drag, wheel (`m,...` / `m2,...`), and X11 keysyms (`kd,<sym>` / `ku,<sym>`) with 1-second `kh,<keys>` keepalive and automatic release of held keys/modifiers/buttons on focus loss or session end.
  - Key release before close (settled: `ARCH-026`, was `ARCH-GAP-DC-09`):
    - When a stream ends, the transport sends `ku` for every held key and then a WebSocket close frame, with a 1 s abort watchdog as fallback. A stream dropped while connecting never sends `SETTINGS`.
    - `kr` is never sent.
    - Bookkeeping lives in the pure `KeyLedger` (`view/keys.rs`): Command never maps to Super or Meta, and keys are released on blur and before Session Controls, Files, or full screen.
    - `SKERRY_INPUT_TRACE=1` traces input messages with printable keys redacted.
  - Exposes per-server **Input** controls: **Command sends Control**, **Remote Cursor** (server cursor shape/visibility updates), **Smooth Trackpad Scrolling** (precise two-finger deltas with end-of-scroll event, enabled by default), **Use Mac Input Method** (local GPUI marked-text composition committed via `co,end,<text>`), and **Send Key Combination** (Alt+Tab, Ctrl+Alt+Delete).
  - Translates AppKit trackpad magnification (`view/pinch.rs`) into Control-wheel zoom steps over the remote picture.
- `clipboard-integration` (settled: `ARCH-017`):
  - ⌘C/⌘X/⌘V stay on the Mac unless **Command sends Control** is on; in a session, Control+C/X/V copy, cut, and paste.
  - Speaks the Selkies web client's clipboard protocol (`cw`/`cb` and multipart `cws`/`cbs` out; `clipboard*` with `clipboard_secret` / `clipboard_reply,cr` in) via `clipboard.rs`, and reads/writes the general `NSPasteboard` directly via `pasteboard.rs` (text, HTML + text as the flavour envelope, PNG; TIFF converted to PNG).
  - **Seamless** (default, from `clipboard_seamless`): the Mac clipboard is sent when a session window becomes key, when the remote desktop gains focus, and just before a forwarded paste keystroke (Control+V, Shift+Insert), only when `NSPasteboard.changeCount` moved and the content differs from the last synced value. Session copies are written to the Mac at once. Nothing is read while Skerry is in the background.
  - **Deliberate** (Seamless off): Session Controls → Clipboard's **Send Mac Clipboard** and **Copy Session Clipboard** (`cr`) move content on demand.
  - Per-server **Send to Session**, **Receive from Session**, and **Seamless** switches (`Preferences.clipboard`) live within `clipboard_in_enabled`, `clipboard_out_enabled`, `enable_binary_clipboard`, and a locked `clipboard_seamless`.
  - Session secrets are written with `org.nspasteboard.ConcealedType` and taken back after 60 s, or once the session's clipboard moves on, while the Mac pasteboard still holds them.
- `in-session-file-transfers` (settled 2026-11-22: `ARCH-023`, `ARCH-024`; mount `ARCH-GAP-DC-14` and rename/move `ARCH-GAP-DC-15` open):
  - `selkies.rs` (`SelkiesClient`) speaks the Selkies HTTP file routes on the session origin with the SealSkin session cookie `transport.rs` already obtains: `GET api/file-manager/browse`, `POST api/file-manager/mkdir`, `GET api/files/<path>` (with `Range` to resume), and `POST api/upload` (plain at or under 64 MiB, otherwise 64 MiB slices with `X-Upload-Id`/`-Offset`/`-Total`/`-Final`, streamed from disk). It sends no `Origin` header.
  - A **Files** window per session (`skerry-ui` view, `skerry-mac` wiring) shows the file-manager folder as a sortable list with breadcrumbs, Back/Forward, Refresh, New Folder, multi-select, keyboard navigation, Quick Look, Open (download to a cache, then open with the default app), Upload…, Download…, and a transfers list with progress and Cancel.
  - Into the session: Finder drops onto the window and ⌘V of Finder file copies (`public.file-url` on `NSPasteboard`) upload into the folder shown; folders are walked on the Mac and recreated with `mkdir`.
  - Out of the session: dragging rows to Finder writes `NSFilePromiseProvider` items, and Skerry downloads each into the folder Finder names; ⌘C downloads the selection to a staging folder first, then puts the real file URLs on the pasteboard. A folder is downloaded file by file into a matching folder, not as the server's zip.
  - **Delete** (⌘⌫, confirmed in a native alert) goes through `SealSkinClient` to the session's home storage (`/Desktop/<path>`), offered only for a SealSkin session with a home whose `Desktop` lists the same names as the Selkies root (`ARCH-024`).
  - Upload and download controls follow `server_settings.file_transfers`; the server also refuses view-only credentials (Skerry connects as the primary client, so it does not model a viewer role).
- `audio-devices-and-media-uplinks` (output settled in `ARCH-030`; mic/webcam open in `ARCH-GAP-DC-05`):
  - Audio output (settled: `ARCH-030`): each session has its own `skerry-audio` thread. Output follows the macOS default, or a chosen **Output Device** saved per server by UID that falls back while missing or failing. The stream opens at 48 kHz f32 with Core Audio converting. Skerry rebuilds after errors, stalls, or sleep/wake, with backoff and a probe that leaves the fallback playing. **Audio Bitrate** sends `SETTINGS {audio_bitrate, picked}` and is hidden or locked as the server says.
  - Unresolved frontier (`ARCH-GAP-DC-05`): microphone and webcam capture uplinks with macOS permission prompts and disconnect teardown.
- `printing-gaming-and-sharing` (unresolved: `ARCH-GAP-DC-04`, `ARCH-GAP-DC-06`, `ARCH-GAP-DC-07`):
  - Gaming (settled: `ARCH-032`):
    - **Capture Pointer** (⌃⌥⇧Z, the Input card, the View menu) locks and hides the Mac cursor, sends relative `m2` motion with `p,1`, and releases on the chord, blur, child windows, full screen, and disconnect. The transport always sends `p,0` on close.
    - GameController pads take slots 0–3 per Selkies session, with W3C mapping, changes only, and a 100 ms `js,h` heartbeat.
    - Pads send only while the window is key, with CoreHaptics rumble, all gated by `gamepad_enabled`.
  - Printing (settled: `ARCH-033`):
    - `print_document` jobs are fetched once from `GET /api/print/{name}`, capped at 1 GiB and checked as PDF, into a cache.
    - Each job shows a notice with Print… (the PDFKit print sheet on the session window), Open, and Save…, and the Printing card lists the session's jobs.
    - **Open Print Dialog Automatically** (per server) opens only while the window is key, and after a reconnect only the newest job under 5 minutes old.
    - The cache is cleaned on remove, on close, on quit, and after 24 h. `printing_enabled` hides it all.
  - Remote PDF print job queue (`ARCH-GAP-DC-04`), relative mouse capture and gamepad slots/rumble (`ARCH-GAP-DC-06`), and view-only / player-slot / SealSkin collaboration rooms (`ARCH-GAP-DC-07`).
- `web-sidebar-panels` (Shortcuts settled in `ARCH-031`, Apps in `ARCH-035`; Pelorus excluded in `ARCH-035`): the web sidebar's Apps, Shortcuts, and Pelorus panels. `ARCH-020` maps every web panel to its owner.
  - Apps (settled: `ARCH-035`):
    - A per-session **Apps** window drives `apps,{op}` (install, remove, update, open, cancel) with a live job console. It is shown only when `command_enabled` is on and the server answers `list` within 5 s.
    - The catalog comes from GitHub with no credentials, or from the session's local repository.
    - Icons are checked and then rasterised in a sandboxed `--skerry-svg-worker` child process.
    - Pelorus is not surfaced.
  - Shortcuts (settled: `ARCH-031`): a Session Controls card lists `shortcuts::CHORDS`. **Mac shortcuts active** (server default and lock honoured, saved per server) hands those chords to the session as Ctrl chords when off, and ⌘Q always stays local. When ⌘ lifts, any held key that is physically up is released (HID key state).
  - Shortcuts (settled: `ARCH-031`): a Session Controls card lists `shortcuts::CHORDS`. **Mac shortcuts active** (server default and lock honoured, saved per server) hands those chords to the session as Ctrl chords when off, and ⌘Q always stays local. When ⌘ lifts, any held key that is physically up is released (HID key state).

Facets:
- `input-and-ime` — keyboard, cursor, trackpad pan/pinch, IME composition, and gaming input
- `clipboard-files-and-print` — `NSPasteboard`, native file transfers, and PDF printing
- `media-and-sharing` — Core Audio output selection, mic/webcam uplinks, and multi-user session sharing

Material: input protocol encoding, macOS AppKit/CoreAudio/NSPasteboard bridges, permission handling, and server policy enforcement.
Immaterial: individual pointer motion coordinates and audio buffer fill levels.

Postconditions:
- Held keys, modifiers, and mouse buttons are always released on window focus loss or session disconnect.
- Local macOS Command shortcuts remain local unless **Command sends Control** is enabled.
- Clipboard, file transfer, print, and media capture strictly obey server inbound/outbound and read-only policies.
- Clipboard content never crosses unchanged twice, never echoes back what Skerry itself wrote (`changeCount` and signature), and is never read from the Mac pasteboard while no session window is key, except by the deliberate button.

### Continuity

input-driven
