# Mirror via CDP screencast, not VNC or WebRTC

The app Mirrors the Active Tab with CDP `Page.startScreencast` (JPEG Frames, quality 60) and sends input back over the same CDP connection, rather than embedding a VNC or WebRTC video of the Remote Browser's desktop. This keeps the Sidebar, Tab Groups and top bar native, keeps the client lightweight, and gives direct per-Tab control (only the Active Tab streams, input goes to one page target). The transport sits behind a trait so a video backend (WebRTC) can be added later.

**Costs accepted:** no audio; JPEG Frames add latency and compression artefacts compared with a video stream; native popups and OS-level UI (file pickers, right-click menus, permission bubbles) are not visible in the Mirror and are handled locally or deferred (JS dialogs get a local modal, popups become Tabs, permission prompts are denied).

## Considered Options

- **The Manager's noVNC/KasmVNC proxy.** Each Profile's headed browser runs on a KasmVNC display, so this works today and has audio and native popups. It mirrors the whole desktop, not one Tab, so the app could not offer a native Sidebar or per-Tab streaming, and the Manager's web UI is a VNC viewer, not a CDP client.
- **Neko-style WebRTC.** Better latency and audio, but needs a server-side component the Manager does not provide, plus a heavy client (WebRTC stack) and still streams a desktop, not a Tab.
- **RDP / SPICE.** Same desktop-level model and a heavier client, with nothing the Manager already exposes.
- **DOM mirroring.** Re-rendering the remote page locally is fragile, breaks on canvas, iframes and anti-bot pages, and would show local web content, which the app deliberately never does.
