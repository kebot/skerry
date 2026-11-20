---
name: video-and-screen
kind: responsibility
id: 067NC4KG01SKERRY0000000002
---

# Video and Screen Domain

<!-- compose:status=partially-settled provenance=compose-managed -->
<!-- compose:unresolved id=ARCH-GAP-VS-01 concept="second-display" question="Follow server second-screen capability and display placement model in a dedicated GPUI window while keeping audio owned by the primary session view." -->
<!-- compose:unresolved id=ARCH-GAP-VS-02 concept="webrtc-transport" question="Prototype WebRTC signaling, negotiated codecs, ICE/STUN/TURN, data-channel input, and SealSkin session handoff before adding a transport selector alongside WebSockets." -->
<!-- compose:unresolved id=ARCH-GAP-VS-03 concept="extended-codecs-and-tuning" question="Evaluate VP8/VP9 decode paths where VideoToolbox lacks hardware decode, plus 10-bit, 4:4:4, Turbo, paint-over, and CPU encoder tuning." -->

### Goal

Deliver low-latency hardware-accelerated or JPEG-fallback remote desktop video, accurate Retina/display scaling, explicit playback controls, and live stream diagnostics in Session Controls while honoring all server capability limits and locks.

### Requires

- `transport-and-video-source`: `crates/skerry-mac/src/transport.rs`, `crates/skerry-mac/src/video.rs`, `crates/skerry-mac/src/settings.rs`, `crates/skerry-mac/src/audio.rs`, `crates/skerry-mac/src/view.rs`, `crates/skerry-mac/src/app/controls.rs`, and `crates/skerry-mac/src/app/display.rs`

### Maintains

- `server-settings-capability-model` (settled): parses `server_settings` and live updates for current values, defaults, allowed choices, numeric ranges, and locks; takes the intersection of server capabilities and native support; resolves per-server saved preferences on connect/reconnect without storing session URLs or keys.
- `codec-and-rate-control` (settled for JPEG, H.264, H.265, AV1; provisional for VP8/VP9/10-bit/4:4:4):
  - Requests `h264enc` by default when supported, or user-selected JPEG, H.264, H.265 (HEVC), or AV1 in **Session Controls -> Video -> Codec**.
  - Converts Annex B / parameter sets / AV1 sequence headers into `CMFormatDescription`, requires hardware VideoToolbox decode into full-range NV12 `CVPixelBuffer` surfaces, and falls back to JPEG when hardware decode cannot recover and the server permits JPEG.
  - Exposes JPEG quality, hardware CBR target bitrate, CRF quality, frame-rate cap, and **Reset Video Settings** according to server policy.
- `resolution-hidpi-and-appearance` (settled):
  - Supports **Match Window**, **Match Retina**, **Scale Locally** (fit picture independently of remote resolution), and fixed resolution presets up to 4096x2160 (even dimensions).
  - Negotiates `enable_resize`, `manual_resolution`, dimensions, `stream_resolution`, and effective macOS display UI scale (including fractional scaled modes) plus light/dark desktop appearance.
- `playback-and-diagnostics` (settled):
  - Explicit video pause/resume, server-acknowledged audio mute/unmute, startup audio policy handling, and clear unavailability reasons.
  - 1-second Stream Diagnostics reporting client FPS, received bitrate, decode time, rejected stripes, WebSocket ping RTT, remote resolution, and active client/server codec.
- `second-display-window` (unresolved: `ARCH-GAP-VS-01`): open a second remote display in its own GPUI window following the server's display placement model, coordinate boundaries, and fullscreen lifecycle while primary session owns audio.
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
