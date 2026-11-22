# ARCH-004: Settled Input, Trackpad Gestures & Local IME Baseline

- **Status:** settled
- **Decision:** Desktop Integration maintains per-server Input controls (Command sends Control, Remote Cursor, Smooth Trackpad Scrolling, Use Mac Input Method, Send Key Combination), AppKit magnification pinch-to-zoom (`view/pinch.rs`), automatic key/modifier/button release on focus loss, and local GPUI marked-text IME composition committed via `co,end,<text>`. Raw multitouch contact forwarding remains deferred (`ARCH-GAP-DC-08`) due to high Chrome latency observed in prototype testing.
- **Affected Contracts:** `src/skerry/desktop-and-collaboration.prose.md`
