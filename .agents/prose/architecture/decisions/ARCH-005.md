# ARCH-005: Product & Protocol Boundaries (SealSkin 0.4.2 & Selkies 715df7c)

- **Status:** settled — SealSkin 0.4.2 version baseline superseded by `ARCH-008`, itself superseded by `ARCH-009` (the product/protocol boundaries below remain in force)
- **Decision:**
  - **Operator vs. Client Settings:** Capture backends, GPU selection (*per-launch GPU choice superseded by `ARCH-016`: the operator still defines which GPUs exist, and the user picks among those at launch, as the web launcher does*), TLS, TURN credentials, audit/recording policy, and server administration (users, groups, app stores, templates, cluster nodes, audit logs) remain operator/web-dashboard responsibilities. Skerry only requests session changes permitted by server policy.
  - **Control-Plane vs. In-Session:** SealSkin companion API owns application keys, catalog, launching, persistent homes/shares, and stopping sessions. Selkies WebSocket owns live stream settings, input, clipboard, in-session file transfer, printing, and media uplinks. *(Amended by `ARCH-023`: in-session file transfer is Selkies' HTTP file routes on the session origin, not the WebSocket.)*
  - **Native Presentation:** Skerry uses native macOS windows, menus, and Session Controls (`cmd-shift-m`). Browser-specific layouts (Sidebar/Wish/Edge), CSS translucency, browser extensions, and touch-only overlays are excluded.
- **Affected Contracts:** `src/skerry/index.prose.md`, `src/skerry/video-and-screen.prose.md`, `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/desktop-and-collaboration.prose.md`
