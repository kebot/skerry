---
name: desktop-and-collaboration
kind: responsibility
id: 067NC4KG01SKERRY0000000004
---

# Desktop Integration and Collaboration Domain

<!-- compose:status=partially-settled provenance=compose-managed -->
<!-- compose:unresolved id=ARCH-GAP-DC-02 concept="file-transfers" question="Add in-session Selkies file upload/download with native macOS Open/Save dialogs, progress, cancel, and permission checks alongside a live stream." -->
<!-- compose:unresolved id=ARCH-GAP-DC-03 concept="audio-devices-and-quality" question="Handle macOS default output device changes, format resampling when 48 kHz f32 is unavailable, sleep/wake recovery, and server-allowed audio bitrate selection." -->
<!-- compose:unresolved id=ARCH-GAP-DC-04 concept="remote-printing" question="Receive remote PDF print jobs with macOS Save and Print actions and optional auto-open preference while honoring read-only/disabled print policies." -->
<!-- compose:unresolved id=ARCH-GAP-DC-05 concept="microphone-and-webcam" question="Negotiate audio/video uplink formats, request macOS capture permissions on demand, show active capture indicators, and stop capture on disconnect." -->
<!-- compose:unresolved id=ARCH-GAP-DC-06 concept="gaming-controls" question="Add relative pointer capture with escape shortcut, physical gamepad forwarding with slot/rumble support, and custom chords." -->
<!-- compose:unresolved id=ARCH-GAP-DC-07 concept="session-sharing-and-rooms" question="Support Selkies view-only and player-slot roles and evaluate SealSkin collaboration rooms (room API, token handoff, control transfer)." -->
<!-- compose:unresolved id=ARCH-GAP-DC-08 concept="multitouch-trackpad" question="Keep raw multitouch contact forwarding deferred until gesture-to-display latency in Chrome matches native macOS pinch/pan and contact release on focus loss is proven." -->
<!-- compose:unresolved id=ARCH-GAP-DC-09 concept="stray-super-key" question="QA 2026-11-21 saw the remote start menu open by itself during a codec switch while focus moved between apps, which suggests a Cmd/Super press reached the remote without its release. Reproduce, then check modifier release on blur and reconnect." -->

### Goal

Integrate the remote Selkies desktop seamlessly with macOS input, trackpad gestures, IME composition, `NSPasteboard` clipboard, native file dialogs, Core Audio devices, PDF printing, media capture, gamepads, and multi-user sharing.

### Requires

- `input-view-and-media-source`: `crates/skerry-mac/src/input.rs`, `crates/skerry-mac/src/view.rs`, `crates/skerry-mac/src/view/pinch.rs`, `crates/skerry-mac/src/audio.rs`, `crates/skerry-mac/src/transport.rs`, and `crates/skerry-mac/src/app/controls.rs`

### Maintains

- `keyboard-cursor-trackpad-and-ime` (settled; raw multitouch deferred in `ARCH-GAP-DC-08`):
  - Forwards mouse, drag, wheel (`m,...` / `m2,...`), and X11 keysyms (`kd,<sym>` / `ku,<sym>`) with 1-second `kh,<keys>` keepalive and automatic release of held keys/modifiers/buttons on focus loss or session end.
  - Exposes per-server **Input** controls: **Command sends Control**, **Remote Cursor** (server cursor shape/visibility updates), **Smooth Trackpad Scrolling** (precise two-finger deltas with end-of-scroll event, enabled by default), **Use Mac Input Method** (local GPUI marked-text composition committed via `co,end,<text>`), and **Send Key Combination** (Alt+Tab, Ctrl+Alt+Delete).
  - Translates AppKit trackpad magnification (`view/pinch.rs`) into Control-wheel zoom steps over the remote picture.
- `clipboard-integration` (settled: `ARCH-017`):
  - ⌘C/⌘X/⌘V stay on the Mac unless **Command sends Control** is on; in a session, Control+C/X/V copy, cut, and paste.
  - Speaks the Selkies web client's clipboard protocol (`cw`/`cb` and multipart `cws`/`cbs` out; `clipboard*` with `clipboard_secret` / `clipboard_reply,cr` in) via `clipboard.rs`, and reads/writes the general `NSPasteboard` directly via `pasteboard.rs` (text, HTML + text as the flavour envelope, PNG; TIFF converted to PNG).
  - **Seamless** (default, from `clipboard_seamless`): the Mac clipboard is sent when a session window becomes key, when the remote desktop gains focus, and just before a forwarded paste keystroke (Control+V, Shift+Insert), only when `NSPasteboard.changeCount` moved and the content differs from the last synced value. Session copies are written to the Mac at once. Nothing is read while Skerry is in the background.
  - **Deliberate** (Seamless off): Session Controls → Clipboard's **Send Mac Clipboard** and **Copy Session Clipboard** (`cr`) move content on demand.
  - Per-server **Send to Session**, **Receive from Session**, and **Seamless** switches (`Preferences.clipboard`) live within `clipboard_in_enabled`, `clipboard_out_enabled`, `enable_binary_clipboard`, and a locked `clipboard_seamless`.
  - Session secrets are written with `org.nspasteboard.ConcealedType` and taken back after 60 s, or once the session's clipboard moves on, while the Mac pasteboard still holds them.
- `in-session-file-transfers` (unresolved: `ARCH-GAP-DC-02`):
  - Native macOS Open/Save dialogs with transfer progress, cancellation, and folder/ZIP support over the Selkies session channel.
- `audio-devices-and-media-uplinks` (baseline 48 kHz Opus output settled; device selection & mic/webcam open in `ARCH-GAP-DC-03`, `ARCH-GAP-DC-05`):
  - Unresolved frontier (`ARCH-GAP-DC-03`): output device picker, sample-rate conversion, sleep/wake recovery, and audio bitrate controls.
  - Unresolved frontier (`ARCH-GAP-DC-05`): microphone and webcam capture uplinks with macOS permission prompts and disconnect teardown.
- `printing-gaming-and-sharing` (unresolved: `ARCH-GAP-DC-04`, `ARCH-GAP-DC-06`, `ARCH-GAP-DC-07`):
  - Remote PDF print job queue (`ARCH-GAP-DC-04`), relative mouse capture and gamepad slots/rumble (`ARCH-GAP-DC-06`), and view-only / player-slot / SealSkin collaboration rooms (`ARCH-GAP-DC-07`).

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
