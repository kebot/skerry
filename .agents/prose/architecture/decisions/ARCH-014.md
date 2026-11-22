# ARCH-014: Session Resilience, Modal Hit-Testing, and Library Texture Lifecycle

- **Status:** settled
- **Decision:**
  - **WebSocket startup retry:** `transport::run` retries the Selkies WebSocket for up to 45 s (`SOCKET_STARTUP_GRACE`, every 1.5 s) on I/O errors and 404/502/503/504, reusing the cookie from the single `access_token` exchange. Other failures still fail at once.
  - **Recoverable session errors:** the session status banner has **Reconnect** and **Disconnect** buttons. The ⋯ Session Controls trigger also appears when the pointer enters the window's top-right corner, so it is reachable when the picture fills the window. The trigger occludes, so clicking it never also clicks the remote desktop. Reconnects for codec or HiDPI changes keep Session Controls open. cmd-w is deliberately not bound in a session, because remote apps need it (ctrl-w with **Command sends Control**).
  - **Modals:** the Launch Dialog and Stop confirmation backdrops `occlude()` the Library, and Escape dismisses them.
  - **Library textures:** like JPEG frames, app icons and session thumbnails are GPU textures that GPUI frees only through `window.drop_image`. Replaced, removed, stale, and forgotten images are queued in `Launcher::retired_images` and dropped on the next render. Downloads started for an older Library (`media_generation`) are discarded. Server images are decoded with limits (≤ 8192 px, ≤ 256 MB) and downscaled (icons 256 px, thumbnails 960 px).
  - **Force Aligned Resolution:** Selkies aligns its stored display size, not the client's desired size, so after toggling alignment Skerry re-sends its desired resolution, which the server then aligns (or restores exactly when alignment is off).
  - **Full screen:** leaving full screen re-applies the session chrome, so the restored title bar never covers the picture.
  - **Connect:** a failed connect keeps the typed key. When the saved key fails at startup, Connect with an empty field retries it. A malformed `config.toml` is reported as invalid TOML with "fix or delete" guidance.
- **Affected Contracts:** `src/skerry/index.prose.md`, `src/skerry/video-and-screen.prose.md`, `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-mac/src/{transport,view,app,api}.rs`, `crates/skerry-mac/src/app/controls.rs`, `crates/skerry-ui/src/{session,launch_dialog,library}.rs`
