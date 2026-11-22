# ARCH-013: Settled Launcher Library, Launch Dialog, Thumbnails, and Stop Session

- **Status:** settled — partially settles `ARCH-GAP-LS-01`, `ARCH-GAP-LS-02`, `ARCH-GAP-LS-04`
- **Decision:** The Library (search over apps and running sessions, category chips, app icons, session thumbnails with GPU and storage pills), the Launch Dialog (Auto/Cleanroom storage, language, Wayland, collaborative, Copy link with in-place "Copied" confirmation), and Stop with a data-loss confirmation are settled behaviour; the QA plan `skerry.qa.test.prose.md` covers them. Favorite and Hide stay hidden (`LaunchDialogProps::show_organize = false`) until favorites and hidden apps are saved and a hidden app can be shown again (`ARCH-GAP-LS-04`). The search placeholder no longer promises opening addresses (`ARCH-GAP-LS-03`).
- **Evidence:** QA run 2026-11-21 (L1–L3, D2, K1, K2 passed against SealSkin 0.5.x).
- **Affected Contracts:** `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/index.prose.md`
- **Affected Rust:** `crates/skerry-ui/src/{library,launch_dialog,session}.rs`, `crates/skerry-mac/src/{app,api}.rs`
