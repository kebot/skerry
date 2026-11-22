---
name: video-and-screen
kind: responsibility
id: 067NC4KG01SKERRY0000000002
---

# Video and Screen Domain

<!-- compose:status=partially-settled provenance=compose-managed -->
<!-- compose:unresolved id=ARCH-GAP-VS-07 concept="cross-display-drag" question="Second display settled (ARCH-039). Deferred: mapping a drag that crosses between the two display windows." -->
<!-- compose:unresolved id=ARCH-GAP-VS-02 concept="webrtc-transport" question="Prototype WebRTC signaling, negotiated codecs, ICE/STUN/TURN, data-channel input, and SealSkin session handoff before adding a transport selector alongside WebSockets." -->
<!-- compose:unresolved id=ARCH-GAP-VS-03 concept="extended-codecs-and-tuning" question="Evaluate VP8/VP9 decode paths where VideoToolbox lacks hardware decode, plus 10-bit, 4:4:4, Turbo, paint-over, and CPU encoder tuning." -->
<!-- compose:unresolved id=ARCH-GAP-VS-05 concept="session-controls-ux" question="Section memory, scroll position, and dropdown shadow are settled (ARCH-026). Still open: the web sidebar's stream-toggle tile row (ARCH-020)." -->

### Goal

Deliver low-latency hardware-accelerated or JPEG-fallback remote desktop video, accurate Retina/display scaling, explicit playback controls, and live stream diagnostics in Session Controls while honoring all server capability limits and locks.

### Requires

- `transport-and-video-source`: `crates/skerry-mac/src/transport.rs`, `crates/skerry-mac/src/video.rs`, `crates/skerry-mac/src/settings.rs`, `crates/skerry-mac/src/audio.rs`, `crates/skerry-mac/src/view.rs`, `crates/skerry-mac/src/app/controls.rs`, and `crates/skerry-mac/src/app/display.rs`

### Maintains

- `server-settings-capability-model` (settled): parses `server_settings` and live updates for current values, defaults, allowed choices, numeric ranges, and locks; takes the intersection of server capabilities and native support; resolves per-server saved preferences on connect/reconnect without storing session URLs or keys.
- `codec-and-rate-control` (settled for JPEG, H.264, H.265, AV1; provisional for VP8/VP9/10-bit/4:4:4):
  - Requests `h264enc` by default when supported, or user-selected JPEG, H.264, H.265 (HEVC), or AV1 in **Session Controls -> Video Settings -> Encoder**.
  - Accepts H.264 predicted frames by reference chaining and H.265/AV1 predicted frames (Selkies 2.3.0 sends `reference == id`) by continuity from the last decoded frame (`ARCH-012`).
  - Converts Annex B / parameter sets / AV1 sequence headers into `CMFormatDescription`, requires hardware VideoToolbox decode into full-range NV12 `CVPixelBuffer` surfaces, and falls back to JPEG when hardware decode cannot recover and the server permits JPEG.
  - Exposes JPEG quality, hardware CBR target bitrate, CRF quality, frame-rate cap, and **Reset Video Settings** according to server policy.
  - Presents each hardware-decoded frame as it decodes, and presents JPEG frames on their bottom stripe or after a 2 ms settle (`ARCH-034`), so neither is capped at 30 fps; acks and pointer motion stay on the 33 ms tick (`ARCH-021`).
- `resolution-hidpi-and-appearance` (settled):
  - Supports **Match Window**, **Match Retina**, **Scale Locally** (fit picture independently of remote resolution), and fixed resolution presets up to 4096x2160 (even dimensions).
  - Negotiates `enable_resize`, `manual_resolution`, dimensions, `stream_resolution`, and effective macOS display UI scale (including fractional scaled modes) plus light/dark desktop appearance.
- `playback-and-diagnostics` (settled):
  - Explicit video pause/resume, server-acknowledged audio mute/unmute, startup audio policy handling, and clear unavailability reasons.
  - 1-second Stream Diagnostics reporting client FPS, received bitrate, decode time, rejected stripes, WebSocket ping RTT, remote resolution, and active client/server codec.
  - Web-sidebar Stats (settled: `ARCH-019`): the Stats section shows Encoder/Capture/Decoder/Connection status rows, FPS/Bandwidth/Latency history graphs, figure tiles, and host CPU/GPU/memory meters from the server's `stream_stats`, subscribed with `_stats,1` only while the section is open; a copy button copies a plain-text report.
- `second-display-window` (settled: `ARCH-039`; drag across windows deferred in `ARCH-GAP-VS-07`):
  - **Add Screen +** opens a Display 2 window on a second connection that claims `display2`. Every outgoing frame is stamped through `Wire`, so it can never take over the primary.
  - It is placed beside the session window using `CGDisplayBounds`, or with a side picker.
  - No audio, stats or print on Display 2. It waits while the primary reconnects and closes with it.
- `jpeg-cpu-cost` (settled: `ARCH-034`): each JPEG stripe is its own RenderImage (zune-jpeg with NEON to BGRA, padded and pixel-snapped, retired after 4 paints). A frame presents on its bottom stripe or after a 2 ms settle, and input arms come before the socket. Hardware codecs stay the default.
- `session-controls-ux` (mostly settled: `ARCH-026`; tile row open in `ARCH-GAP-VS-05`): which sections are open is saved per server, scroll position is kept per session window across closing the panel and reconnecting, and Session Controls dropdowns use `menu_dropdown` with a theme-aware shadow.
  - Web-sidebar look (settled: `ARCH-018`): sections are separate rounded cards 10px apart; settings are a label above a full-width dropdown, slider (value in the label, requested on release), switch, or button, as in the Selkies web sidebar.
- `stats-overlay` (settled: `ARCH-031`): **Show over the stream** (in the Stats card and the View menu) draws the web client's strip centred at the top of the picture: status marks, fps, video rate against the target, capture-to-sent, and RTT. It takes no pointer input and is saved per server. `_stats,1` is held while Stats is open or the strip is on.
- `webrtc-transport-evaluation` (unresolved: `ARCH-GAP-VS-02`): evaluate WebRTC signaling, ICE/STUN/TURN, negotiated codecs, and data-channel input against the direct WebSocket transport.

Facets:
- `codecs` — JPEG, H.264, H.265, AV1 hardware decode and rate control
- `screen` — resolution negotiation, Retina/UI scaling, appearance, and multi-display
- `diagnostics-and-playback` — pause/mute state and live stream telemetry
- `transport` — Selkies WebSocket and candidate WebRTC transport

Material: codec negotiation changes, VideoToolbox surface pipeline changes, server settings intersection logic, resolution/DPI scaling math, and diagnostics metrics.
Immaterial: transient FPS/RTT sample fluctuations during live runs.

Postconditions:
- Verified against Milestone 1 & codec QA baseline (SealSkin 0.4.2 disposable Zen session at 1280x720 and Match Retina 1966x2160 across JPEG, H.264, H.265, and AV1 on Apple Silicon).
- Every video or screen control honors configured server defaults, server-locked values, missing capabilities, and reconnect persistence.

### Continuity

input-driven
