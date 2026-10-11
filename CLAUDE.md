# Helios: rules for agents

- Public project: everything written to files (code, comments, docs, commit
  messages) is in English, with no private paths, tracker IDs or links an
  outside reader cannot follow. The user chats in French: answer in French.
- Read `ROADMAP.md` before starting, and the fork's `doc/helios/PHASE0.md` for
  phase 0. There is no handoff file: status lives in the ROADMAP checklists.
- Split: what concerns darktable directly (phase 0 design, SQL inventory,
  equivalence bench, wbox) lives in the fork; tools that calibrate the future
  (module inventory, needs research) live here.
- `ROADMAP.md` is the reference for vision and decisions. Keep it current:
  date every decision, mark ideas "not decided" until the user settles them.
- Phase 0 work happens in the darktable fork (sibling checkout `../darktable`,
  branch `helios/phase0-catalog-sdk`), under that repository's `AGENTS.md`
  and its local `AGENTS.local.md` (build, bench and checkout state).
- License is GPLv3 (Helios links with darktable); ask before adding a
  dependency with an incompatible license.
- Commit and push freely in this repository (2026-10-11): it is a new,
  AI-assisted project. Keep one purpose per commit, every commit building
  and passing its tests, with a changelog fragment for user-visible changes.
  The darktable fork keeps its own rules (see its `AGENTS.md`).
