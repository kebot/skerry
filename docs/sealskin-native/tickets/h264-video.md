# Prototype hardware-decoded H.264 over the existing Selkies WebSocket

Use VideoToolbox, verify codec negotiation and keyframe recovery after reconnect or dropped frames, and test presenting decoded surfaces directly in GPUI. Keep JPEG as a fallback where the server allows it. Compare CPU, memory, bandwidth, and motion quality at the same resolution and frame rate.

[WebSocket client implementation](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/web/selkies-web-core/selkies-ws-core.ts)

## Prototype results

SealSkin Native can request `h264enc` from **Session Controls → Video → Codec**. It converts full-frame Annex B access units to length-prefixed samples, requires hardware VideoToolbox decode, and gives retained full-range NV12 surfaces directly to GPUI. JPEG remains the default and is the fallback when permitted by the server. [Running the app](../running.md)

A disposable Zen session on SealSkin 0.4.2 delivered 1280×720 x264 over the existing WebSocket. The client rendered it, recovered after pause and reconnect, and handled JPEG stripes queued before the codec switch. A QA build that rejected incoming H.264 packets requested keyframes and then switched back to JPEG after the 15-second recovery window. The hardware test checks black and white expansion from limited range and verifies that a missing reference invalidates deltas until a new keyframe arrives.

## Comparison on Apple M5 Pro

Both runs used the same disposable Zen session with its animated welcome screen, the same 1280×720 resolution and client window, and a requested 25 fps cap. Values are medians of ten one-second samples from a release build with temporary diagnostics logging. The JPEG run used the session’s default quality. Motion looked continuous in both runs and neither reported rejected packets. Color fidelity and motion quality still need a calibrated comparison across a wider range of content. GPUI 0.2.2 uses a fixed full-range BT.601 conversion for these surfaces.

| Client decoder | Published FPS | Received Mb/s | Process CPU | Process RSS |
| --- | ---: | ---: | ---: | ---: |
| H.264 VideoToolbox | 23.9 | 0.64 | 4.8% | 106.2 MiB |
| JPEG | 23.5 | 8.55 | 20.1% | 123.0 MiB |

[Selkies web feature research](research.md)
