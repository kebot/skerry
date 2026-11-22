# ARCH-006: gpui-kit UI Design System & Root-Wrapped Window Ownership

- **Status:** settled
- **Decision:** Migrate `crates/skerry-mac` from raw `gpui` (`0.2.2`) to `gpui-kit` (`0.10.0`) (`gpui_kit::init(cx)`, `gpui_kit::assets::Assets`, `Theme::sync_system_appearance`, and `gpui_kit::component` primitives: `Button`, `Input`/`InputState`, `Switch`, `Label`, `Collapsible`, `IconName`, `ScrollableElement`). Because `gpui_kit::open_window` mounts `gpui_kit::base::Root` as each window's root view:
  - Cross-window references (e.g., `ControlPanel`'s `Owner`) must pair `AnyWindowHandle` with the target `Entity<Launcher>` instead of calling `window.window_handle().downcast::<Launcher>()`.
  - Child windows whose initial render reads `Launcher` state (such as `ControlPanel::open` invoked from `Launcher::toggle_controls`) must be opened inside `cx.defer(...)` after `Launcher`'s mutable entity lease ends, preventing re-entrant borrow panics when `window.activate_window()` triggers a synchronous render.
- **Rationale:** Adopts a unified, system-appearance-synced component library instead of custom buttons, inputs, switches, and collapsibles, while honoring `gpui-kit`'s `Root` wrapper and GPUI's single-writer entity lease rules.
- **Affected Contracts:** `src/skerry/index.prose.md`
