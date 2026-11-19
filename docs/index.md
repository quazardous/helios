# Documents

The design and research documents of Helios. The crates and modules are described in [crates.md](./crates.md).

- [ROADMAP.md](../ROADMAP.md): vision, concepts, phases and open questions.
- [NEEDS.md](./NEEDS.md): the roadmap checked against real user needs,
  competitors and measurements.
- [module-contract.md](./module-contract.md): draft of what an
  image processing module is in Helios: isolated, pure, testable alone.
- [architecture.md](./architecture.md): draft of how the parts of
  Helios communicate: data on graph edges, commands to owners, a central bus
  for orchestration.
- [testing-strategy.md](./testing-strategy.md): draft of how Helios
  is tested and sandboxed, designed so that an AI agent can build it.
- [releases.md](./releases.md): where releases come from, what
  they contain and how to check their checksums.
- [bench.md](./bench.md): the bench box, a remote runner for full
  builds, the test suite, timings and GPU tests (`cargo xtask bench-remote`).
- [ci-broker.md](./ci-broker.md): the CI broker that chooses which tests
  a run needs; its per-test history on the `ci-history` branch.
- [performance.md](./performance.md): the performance budgets
  (thumbnails first) and the dated measurements against them.
- [ui-index.md](./ui-index.md): the "use these" index, generated: a UI
  need to the call that makes it, Helios's presets first, then egui-pop's
  widgets. Read it before writing UI code.
- [gui.md](./gui.md): GUI design notes: dockable panels, the node
  header, the queue panel, the graph view.
- [ux-research.md](./ux-research.md): what users ask of darktable's
  interface and its equivalents, real workflows, and scenarios for the GUI.
- [vkdt-study.md](./vkdt-study.md): how vkdt modules describe
  themselves and run, mapped onto the Helios module contract, with its
  generated [inventory](./vkdt-inventory.md).
- [iop-inventory.md](./iop-inventory.md): what darktable's image
  operation modules already describe about themselves (introspection, widgets,
  tiling, OpenCL, scale awareness), as a baseline for module normalization.

