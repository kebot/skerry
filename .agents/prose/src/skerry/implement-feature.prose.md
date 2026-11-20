---
name: implement-feature
kind: function
---

# Implement Skerry Feature Ticket

### Parameters

- `ticket`: ticket filename or feature request under `.agents/prose/specs/skerry-mac/tickets/`

### Returns

- `summary`: implemented changes, verification output (`cargo test`, `cargo clippy`, `cargo fmt`), and updated spec paths in `.agents/prose/specs/`

### Strategies

- Read `.agents/prose/specs/skerry-mac/DESIGN.md` and `.agents/prose/specs/gpui-notes.md` before touching `crates/skerry-mac/src/`.
- Keep changes minimal and aligned with existing module boundaries (`api.rs`, `transport.rs`, `video.rs`, `settings.rs`, `input.rs`, `audio.rs`, `view.rs`, `app.rs`).
- Gate completion on passing `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --check`.
- Update `.agents/prose/specs/skerry-mac/TODO.md` and `.agents/prose/specs/skerry-mac/DESIGN.md` once verified.
