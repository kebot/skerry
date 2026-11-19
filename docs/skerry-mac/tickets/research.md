# Selkies web feature research

This backlog covers the Selkies web dashboard and session features relevant to the native macOS client. The inventory is pinned to upstream commit `715df7c`. An earlier smoke test against our SealSkin 0.4.2 deployment confirmed WebSocket/JPEG streaming and input. Availability of the other features depends on the Selkies build in each application image and on the server’s policy. [Selkies settings at 715df7c](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/docs/settings.md) · [SealSkin Native design](../DESIGN.md)

## What the web client offers

| Area | Options and behavior | Native baseline |
|---|---|---|
| Transport and video format | WebSockets by default and WebRTC where offered. Encoder menu follows the server’s available codecs and the client decoder’s capabilities. The upstream list includes H.264, H.265, VP8, VP9, AV1, and WebSocket-only JPEG. Full color (4:4:4) and 10-bit appear where supported. | WebSocket with JPEG requested. |
| Video performance | Frame rate and follow-display refresh, CBR bitrate or CRF quality, Turbo mode, JPEG quality, still-screen paint-over controls, force CPU encoding, and CPU encoder threading. Some controls depend on codec, rate-control mode, or encoder backend. | Requests 30 fps and JPEG quality 75. |
| Display | Automatic resize, manual presets and custom dimensions, local scaling, anti-aliasing, HiDPI, remote UI scaling/DPI, aligned resolution, fullscreen, and an optional second display with placement controls. The remote desktop can follow light or dark appearance. | Requests 1280×720, fits the image locally, and supports macOS fullscreen. |
| Playback and capture | Per-client video/audio toggles, audio bitrate, speaker and microphone device selection, microphone and webcam toggles, and a webcam encoder selector. Server policy controls startup and on-demand capture. | Mute/unmute and stereo Opus playback on compatible 48 kHz output devices. |
| Input | Keyboard and mouse, keyboard-layout detection in the web core, optional Command-as-Control, cursor styles, pointer lock and raw movement, gaming mode, and a toggle for the web client’s shortcut chords. Touch clients get direct touch, a virtual trackpad with adjustable speed, an on-screen keyboard, and soft keys or custom chords. Physical and virtual gamepads can be forwarded, including rumble where supported. | Mouse, wheel, keyboard, and modifier forwarding. macOS Command shortcuts stay local. |
| Clipboard, files, and print | Clipboard direction switches, text, images, rich text, and seamless versus deliberate sync. Session file upload/download and a file manager with folders. Remote print jobs can be opened, printed, or saved as PDF. An application launcher is available where the session image provides its catalog. | Pending. The existing launcher lists SealSkin server applications. |
| Sharing | View-only and player-slot links, shared control, and server-enforced roles. Secure mode uses provisioned tokens for roles. SealSkin also has a collaboration room with its own API and access model. | Individual sessions only. |
| Diagnostics and appearance | Live FPS, bitrate, latency/RTT, encoder and pipeline statistics. Sidebar, Toolbar (Wish), and Edge layouts, theme, translucency, branding, and server-configurable panel visibility. | Local appearance follows macOS. No stream stats panel. |

[Dashboard settings controls](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/web/selkies-dashboard/src/panels/settings.tsx) · [Web client and layouts](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/docs/components/web-client.md) · [Usage and interaction](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/docs/usage.md) · [SealSkin 0.4.2 API](https://github.com/selkies-project/sealskin/blob/0.4.2/docs/content/api.md)

## Ownership and acceptance

- **Selkies deployment configuration** includes capture backends, GPU selection, server/TLS settings, TURN credentials, audit and recording policy, and capability limits. Treat those as operator settings. A native control may request a session change where the server permits one.
- **SealSkin API** owns saved application access, launching, homes and storage, and stopping sessions. The Selkies Apps panel is an in-session feature of supported images. Evaluate it separately from the existing SealSkin launcher.
- **Native presentation** uses macOS menus, toolbar controls, and a settings panel. Translate server panel-visibility settings where there is an equivalent control. Sidebar/Wish/Edge layouts, CSS translucency, and touch-only overlays can stay browser-specific.
- **Compatibility checks** for each new control should cover the configured default, a server-locked value, a missing capability, and reconnect. Verify effects on the actual application images before claiming support. Record observed wire values in test fixtures with credentials removed.

[Server settings](https://github.com/selkies-project/selkies/blob/715df7cdea9c6c21ae339f93f194f718a879dc41/docs/settings.md) · [SealSkin API](https://github.com/selkies-project/sealskin/blob/0.4.2/docs/content/api.md)
