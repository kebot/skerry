# SealSkin Native Milestone 1 QA

Milestone 1 covers the six Video and Screen tickets in Review and the separate Session Controls panel. The live smoke test used the signed macOS app, a disposable Zen session on SealSkin 0.4.2, and an isolated local preferences directory. [Checklist](TODO.md)

| Check | Result |
| --- | --- |
| Launch and resume | The SealSkin API launched the disposable Zen session. The app listed it, resumed it, and rendered the remote desktop over JPEG/WebSocket. |
| Server settings and preferences | The live session displayed the advertised 60 fps cap and JPEG quality 40. The panel accepted JPEG quality 45, saved it locally, and showed 45 after reconnect. Automated tests cover bounds, locks, and unavailable settings. |
| Screen and input | Fixed 1920×1080 and Match Retina changed the streamed picture. The diagnostics reported 1966×2160 in Match Retina for the test window. Clicking the visible Zen control after closing the panel advanced the remote screen. |
| Diagnostics | The panel reported current resolution, client FPS, bitrate, decode time, rejected stripes, and WebSocket round-trip time. The default panel height was increased after QA found the Diagnostics heading clipped. |
| Playback and reconnect | Pause changed the button to Resume Video. Resuming restored video. The app remained muted after reconnecting. |
| Panel and fullscreen | macOS exposed the session and Session Controls as separate windows. The keyboard shortcut opened the panel, Escape dismissed it, and the remote desktop received clicks after focus returned. Full Screen expanded the session window and Session Controls reopened in the fullscreen space. |
| Build | The workspace tests, Clippy, formatting, and ad-hoc signature verification passed. |

Audible playback, remote light/dark appearance switching, and the server’s reset behavior after reconnect still need hands-on checks. The client publishes JPEG pictures on a 33 ms tick, so the per-session frame rate cap can exceed the client’s presentation rate. Hardware decoding remains tracked separately. [Playback](tickets/playback.md) · [HiDPI and appearance](tickets/hidpi-and-appearance.md) · [H.264](tickets/h264-video.md)
