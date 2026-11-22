---
name: implement-feature
kind: function
---

# Implement Skerry Architectural Concept

### Parameters

- `target`: an unresolved `ARCH-GAP-*` identifier, concept name, or feature request from `video-and-screen`, `launcher-and-sessions`, or `desktop-and-collaboration`

### Returns

- `summary`: resolved constraints, Rust modules changed in `crates/skerry-mac/`, verification evidence (`cargo test`, `cargo clippy`, `cargo fmt`), and updated OpenProse contracts

### Invariants

- Never introduce an embedded browser or bypass `sealskin.rs` / `selkies.rs` / `transport.rs` origin and credential checks.
- Respect `gpui-kit` `0.10.0` invariants in `index.prose.md` (`Root`-wrapped windows, deferred child-window opening, Tokio runtime lifetime, `window.drop_image` for JPEG, `window.paint_surface` for VideoToolbox surfaces).
- Every new control must handle server defaults, server-locked values, missing server capabilities, and reconnect persistence.

### Strategies

- Locate the target concept's `<!-- compose:unresolved id=ARCH-GAP-... -->` marker in `video-and-screen.prose.md`, `launcher-and-sessions.prose.md`, or `desktop-and-collaboration.prose.md`.
- If a design boundary is genuinely ambiguous, run `prose compose` to settle the constraint with the architect before writing code.
- Implement the change in `crates/skerry-mac/src/` with unit tests covering defaults, locks, bounds, and protocol parsing.
- Run `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --check`.
- Promote the resolved concept in its domain `.prose.md` contract from `unresolved` to `settled`, remove the `<!-- compose:unresolved -->` marker, and record the `ARCH-*` decision in `.agents/prose/architecture/decisions.md`.
