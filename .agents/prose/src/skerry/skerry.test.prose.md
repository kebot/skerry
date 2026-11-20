---
name: skerry-package-test
kind: test
subject: skerry
---

# Skerry Package Verification Suite

### Fixtures

- `workspace`: `crates/skerry-mac` Rust workspace and `.agents/prose/src/skerry/*.prose.md` contracts

### Expects

- The root responsibility `skerry` and domain responsibilities (`video-and-screen`, `launcher-and-sessions`, `desktop-and-collaboration`) have explicit `Requires`, `Maintains`, `Postconditions`, and `Continuity`.
- Every unsettled product feature from the former backlog is preserved as an explicit `<!-- compose:unresolved id=ARCH-GAP-... -->` marker and constraint entry in its owning domain responsibility.
- Every settled Milestone 1 and hardware codec feature is recorded as a settled `Maintains` clause with verification postconditions.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --check` succeed on the workspace.

### Expects Not

- Separate non-Prose markdown ticket directories (`docs/` or `.agents/prose/specs/skerry-mac/tickets/`) acting as a parallel source of truth outside `.prose.md` contracts and `architecture/decisions.md`.
- Storing session URLs or application keys inside per-server JSON preferences.
