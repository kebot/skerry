# Skerry Agent Instructions

Always use **OpenProse** (`.agents/prose/`) to plan, track, verify, and evolve Skerry's architecture and product features. Do not create standalone `docs/`, `TODO.md`, `DESIGN.md`, `GLOSSARY.md`, or separate markdown ticket folders.

## Canonical Source of Truth

All architecture, settled module contracts, GPUI invariants, and feature backlog frontiers live in `.agents/prose/`:

- **System Root & GPUI Invariants:** `.agents/prose/src/skerry/index.prose.md`
- **Domain Responsibilities & Feature Frontiers (`ARCH-GAP-*`):**
  - `.agents/prose/src/skerry/video-and-screen.prose.md`
  - `.agents/prose/src/skerry/launcher-and-sessions.prose.md`
  - `.agents/prose/src/skerry/desktop-and-collaboration.prose.md`
- **Feature Builder Function:** `.agents/prose/src/skerry/implement-feature.prose.md`
- **Package Verification Test Suite:** `.agents/prose/src/skerry/skerry.test.prose.md`
- **Decision & Constraint Lattice (`ARCH-*` & `ARCH-GAP-*`):** `.agents/prose/architecture/decisions.md`
- **Derived Visual Map (gitignored):** `.agents/prose/architecture/view.html`
- **OpenProse Skill Specification:** `.agents/skills/open-prose/SKILL.md`

## Workflow Rules

1. **Before Planning or Changing Behavior (`prose compose`):**
   - Activate the `open-prose` skill (`.agents/skills/open-prose/SKILL.md`).
   - Read `.agents/prose/src/skerry/index.prose.md`, the relevant domain `.prose.md` contract, and `.agents/prose/architecture/decisions.md`.
   - Use `prose compose` semantics: provide a brief mental-model sync, ask at most 1–3 high-leverage multiple-choice questions on the active frontier, and materialize answers directly into `.prose.md` contracts (`Maintains`, `Invariants`, `Postconditions`, or `<!-- compose:unresolved id=ARCH-GAP-... -->` markers), `skerry.test.prose.md`, `architecture/decisions.md`, and `architecture/view.html`.
2. **When the User Changes Their Mind:**
   - Surface the blast radius (affected `ARCH-*` decisions, downstream contracts, tests, and Rust modules).
   - Mark the previous `ARCH-*` decision in `.agents/prose/architecture/decisions.md` as `superseded by ARCH-xxx` with rationale rather than deleting history.
   - Update the affected `.prose.md` contract region and `skerry.test.prose.md` assertions in the same turn.
3. **When Implementing a Feature (`prose run` / `implement-feature`):**
   - Follow the invariants in `.agents/prose/src/skerry/index.prose.md` (GPUI 0.2.2 `runtime_shaders`, Tokio runtime lifetime, `window.drop_image` for JPEG frames, zero-copy `window.paint_surface` for VideoToolbox NV12 surfaces, and strict SealSkin/Selkies origin and credential policies).
   - Verify all Rust changes with:
     ```sh
     cargo test --workspace
     cargo clippy --workspace --all-targets -- -D warnings
     cargo fmt --check
     ```
   - Once verified, promote the feature in its domain `.prose.md` contract from `unresolved` to `settled`, remove its `<!-- compose:unresolved -->` marker, and record the settled `ARCH-*` decision in `.agents/prose/architecture/decisions.md`.
