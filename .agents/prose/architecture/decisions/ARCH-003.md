# ARCH-003: Settled Video & Screen Baseline (Milestone 1 + Hardware Codecs)

- **Status:** settled
- **Decision:** Video & Screen maintains server-capability intersection (`settings.rs`), JPEG + hardware VideoToolbox H.264/H.265/AV1 decoding (`video.rs`, painted via zero-copy `window.paint_surface`), JPEG `RenderImage` retirement via `window.drop_image`, window/Retina/UI-scale resolution negotiation (`display.rs`), explicit video pause and server-acknowledged audio mute (`audio.rs`), and 1-second Stream Diagnostics in Session Controls (`app/controls.rs`).
- **Evidence:** Verified on SealSkin 0.4.2 disposable Zen sessions across JPEG, H.264, H.265, and AV1 at 1280x720 and Match Retina 1966x2160 on Apple M5 Pro, plus automated unit tests, Clippy, and formatting checks.
- **Affected Contracts:** `src/skerry/video-and-screen.prose.md`
