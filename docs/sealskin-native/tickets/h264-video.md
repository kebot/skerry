# Prototype hardware-decoded H.264 over the existing Selkies WebSocket

Use VideoToolbox, verify codec negotiation and keyframe recovery after reconnect or dropped frames, and test presenting decoded surfaces directly in GPUI. Keep JPEG as a fallback where the server allows it. Compare CPU, memory, bandwidth, and motion quality at the same resolution and frame rate.

[WebSocket client implementation](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/web/selkies-web-core/selkies-ws-core.ts)

## Prototype results

A disposable Zen session sent full-frame H.264 (Constrained Baseline, 1280×720, 8-bit 4:2:0) over the existing Selkies WebSocket. Its 12-byte video header preceded Annex B access units. The first messages also included JPEG stripes from the previous session mode, so the native receiver must allow for a codec transition. FFmpeg with VideoToolbox requested decoded 72 captured H.264 frames on an Apple M5 Pro. This was a decode-only probe. In-app rendering, reconnect recovery, and a controlled performance comparison are still open.

GPUI 0.2.2 exposes `Window::paint_surface` for a `CVPixelBuffer`. Its macOS renderer currently expects full-range bi-planar 8-bit YUV. The captured stream reported limited range, so validate range and color conversion when selecting the decoder output format.

[Selkies web feature research](research.md)
