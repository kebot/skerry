# ARCH-002: Native OpenProse Responsibility Package Replaces Legacy `docs/` & Tickets

- **Status:** settled
- **Decision:** Convert all legacy markdown documentation (`DESIGN.md`, `gpui-notes.md`, `TODO.md`, `running.md`, `milestone-1-qa.md`, and all 28 ticket files) directly into native OpenProse contracts under `.agents/prose/src/skerry/` and decision records in `.agents/prose/architecture/decisions.md`, deleting `docs/` and `.agents/prose/specs/`.
- **Rationale:** Following `std/ops/compose` invariants ("The directory package is the canonical persisted program. The working architecture proposes changes to it; it does not remain a parallel truth"), completed features become settled `### Maintains` clauses and postconditions, while unbuilt features become explicit `<!-- compose:unresolved id=ARCH-GAP-... -->` frontier markers on their owning domain responsibilities.
- **Affected Contracts:**
  - `src/skerry/index.prose.md`
  - `src/skerry/video-and-screen.prose.md`
  - `src/skerry/launcher-and-sessions.prose.md`
  - `src/skerry/desktop-and-collaboration.prose.md`
  - `src/skerry/implement-feature.prose.md`
  - `src/skerry/skerry.test.prose.md`
