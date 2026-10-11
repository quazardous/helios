# darktable-helios: rules for agents

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
- Do not commit or push unless asked: stop at a staged change and a proposed
  commit message.
