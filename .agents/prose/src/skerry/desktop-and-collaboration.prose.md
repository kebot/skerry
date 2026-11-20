---
name: desktop-and-collaboration
kind: responsibility
id: 067NC4KG01SKERRY0000000004
---

# Desktop Integration and Collaboration Domain

<!-- compose:status=partially-settled provenance=compose-managed -->
<!-- compose:unresolved id=ARCH-GAP-DC-01 concept="clipboard-sync" question="Bridge Selkies clipboard messages to NSPasteboard honoring inbound/outbound server policy, local direction switches, Unicode, focus changes, and loop-prevented follow-up rich-text/image sync." -->
<!-- compose:unresolved id=ARCH-GAP-DC-02 concept="file-transfers" question="Add in-session Selkies file upload/download with native macOS Open/Save dialogs, progress, cancel, and permission checks alongside a live stream." -->
<!-- compose:unresolved id=ARCH-GAP-DC-03 concept="audio-devices-and-quality" question="Handle macOS default output device changes, format resampling when 48 kHz f32 is unavailable, sleep/wake recovery, and server-allowed audio bitrate selection." -->
<!-- compose:unresolved id=ARCH-GAP-DC-04 concept="remote-printing" question="Receive remote PDF print jobs with macOS Save and Print actions and optional auto-open preference while honoring read-only/disabled print policies." -->
<!-- compose:unresolved id=ARCH-GAP-DC-05 concept="microphone-and-webcam" question="Negotiate audio/video uplink formats, request macOS capture permissions on demand, show active capture indicators, and stop capture on disconnect." -->
<!-- compose:unresolved id=ARCH-GAP-DC-06 concept="gaming-controls" question="Add relative pointer capture with escape shortcut, physical gamepad forwarding with slot/rumble support, and custom chords." -->
<!-- compose:unresolved id=ARCH-GAP-DC-07 concept="session-sharing-and-rooms" question="Support Selkies view-only and player-slot roles and evaluate SealSkin collaboration rooms (room API, token handoff, control transfer)." -->
<!-- compose:unresolved id=ARCH-GAP-DC-08 concept="multitouch-trackpad" question="Keep raw multitouch contact forwarding deferred until gesture-to-display latency in Chrome matches native macOS pinch/pan and contact release on focus loss is proven." -->

### Goal

Integrate the remote Selkies desktop seamlessly with macOS input, trackpad gestures, IME composition, `NSPasteboard` clipboard, native file dialogs, Core Audio devices, PDF printing, media capture, gamepads, and multi-user sharing.

### Requires

- `input-view-and-media-source`: `crates/skerry-mac/src/input.rs`, `crates/skerry-mac/src/view.rs`, `crates/skerry-mac/src/view/pinch.rs`, `crates/skerry-mac/src/audio.rs`, `crates/skerry-mac/src/transport.rs`, and `crates/skerry-mac/src/app/controls.rs`

### Maintains

- `keyboard-cursor-trackpad-and-ime` (settled; raw multitouch deferred in `ARCH-GAP-DC-08`):
  - Forwards mouse, drag, wheel (`m,...` / `m2,...`), and X11 keysyms (`kd,<sym>` / `ku,<sym>`) with 1-second `kh,<keys>` keepalive and automatic release of held keys/modifiers/buttons on focus loss or session end.
  - Exposes per-server **Input** controls: **Command sends Control**, **Remote Cursor** (server cursor shape/visibility updates), **Smooth Trackpad Scrolling** (precise two-finger deltas with end-of-scroll event, enabled by default), **Use Mac Input Method** (local GPUI marked-text composition committed via `co,end,<text>`), and **Send Key Combination** (Alt+Tab, Ctrl+Alt+Delete).
  - Translates AppKit trackpad magnification (`view/pinch.rs`) into Control-wheel zoom steps over the remote picture.
- `clipboard-integration` (unresolved: `ARCH-GAP-DC-01`):
  - Deliberate and optional seamless text/rich-text/image copy and paste between `NSPasteboard` and Selkies clipboard messages within server inbound/outbound policy.
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

### Continuity

input-driven
