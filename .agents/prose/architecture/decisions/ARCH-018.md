# ARCH-018: Session Controls Look and Work Like the Selkies Web Sidebar

- **Status:** settled — design approved in the preview workbench on 2026-11-21; amends the Session Controls layout of `ARCH-007`, partially settles `ARCH-GAP-VS-05`
- **Finding (2026-11-21):** Selkies' web sidebar (`selkies-dashboard/src/layouts/sidebar/sidebar.css`, `panels/settings.tsx`, `translations.ts`) shows each section as its own rounded card, 10px apart, with uppercase titles. Inside, every setting is a label above a full-width control: dropdowns for Encoder, Rate Control, UI Scaling, and Preset; sliders with the value in the label ("Frames per second (60 FPS)", "Video CRF (25)", "Video Bitrate (8 Mbps)"); switches for on/off settings; full-width 40px buttons. Skerry stacked flush sections and used segmented button rows, −/+ steppers, "Turn On/Off" buttons, and inline option lists.
- **Decision:** `skerry-ui::controls` follows the web sidebar for the settings Skerry already supports (no new settings):
  - **Cards:** sections inset 16px with a 10px gap; 1px border, 12px radius, 11×14px header, 4/14/14px body; titles upper case, chevron down when closed and up when open.
  - **Rhythm:** 14px between settings, 6px from a label to its control, labels semibold.
  - **Dropdowns:** Encoder (codec), Encoder Rate Control Mode ("CBR (Constant Bitrate)", "CRF (Constant Quality)"), UI Scaling, and Preset are full-width 40px dropdown buttons opening a popup menu with the current choice checked (no `SelectState`; the inline picker lists and `ControlsAction::TogglePicker` go away).
  - **Sliders:** Frames per second, Video Bitrate (Mbps), Video CRF, JPEG Base Quality, and Paintover Quality are `gpui-kit` sliders over the server's min..max. Their live value shows in the label while dragging, and the setting is requested once, on release. `skerry-ui` keeps each slider's state with `window.use_keyed_state`, so callers pass only values.
  - **Switches:** Use Paint-Overs, HiDPI, Force Aligned Resolution, and Input options; a server-locked one is a disabled switch.
  - **Buttons:** Set Manual Resolution and Reset to Window side by side at equal width; Scale Locally, Reset Video Settings, and key combinations full width.
  - Skerry keeps its own theme colors (no pink accent) and native-only rows (Preferred codec notice, Stream Diagnostics).
  - The Clipboard section (`ARCH-017`) takes the same switches (a server-locked Seamless is a disabled switch) and full-width Send Mac Clipboard / Copy Session Clipboard buttons.
- **Design review:** approved in the preview workbench (`cargo run -p skerry-ui --example preview`, Session Controls stories) on 2026-11-21.
- **Evidence:** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`; screenshots of the Hardware H.265 story. Dropdown menus and slider drags have not yet been exercised in a live session.
- **Affected Contracts:** `src/skerry/video-and-screen.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Rust:** `crates/skerry-ui/src/controls.rs`, `crates/skerry-ui/examples/preview.rs`, `crates/skerry-mac/src/app/controls.rs`
