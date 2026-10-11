# darktable-helios

> A client/server, AI-first reinterpretation of darktable: the quality of
> darktable without having to learn darktable.

Helios builds on [darktable](https://www.darktable.org/)'s image processing
and turns it into a set of normalized, self-describing layers: a catalog
exposed as an API (the vault), a kernel that schedules work on any node
(laptop, GPU workstation, NAS, cloud), and several surfaces on top of a
single engine, including one where an AI assembles a small, pre-calibrated
workshop from an artistic intention.

**Status: design stage.** There is no code here yet. The first phase, a
catalog SDK inside darktable itself, happens in the
[darktable fork](https://github.com/quazardous/darktable) on the
`helios/phase0-catalog-sdk` branch, with its design in `doc/helios/PHASE0.md`.

## Documentation

- [ROADMAP.md](./ROADMAP.md): vision, concepts, phases and open questions.
- [docs/NEEDS.md](./docs/NEEDS.md): the roadmap checked against real user needs,
  competitors and measurements.
- [docs/iop-inventory.md](./docs/iop-inventory.md): what darktable's image
  operation modules already describe about themselves (introspection, widgets,
  tiling, OpenCL, scale awareness), as a baseline for module normalization.

## Tools

- `tools/iop_inventory.py`: regenerates the module inventory from a darktable
  checkout (`../darktable` by default, or `$DARKTABLE_SRC`).

Tooling that serves darktable itself (phase 0 SQL inventory, equivalence
bench) lives in the darktable fork under `tools/helios/`.

## License

GNU General Public License v3.0. See [LICENSE](./LICENSE). Helios links with
darktable, which is GPLv3.
