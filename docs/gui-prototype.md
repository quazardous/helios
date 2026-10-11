# GUI technology: egui or iced, measured

Two prototypes under `proto/` do the same work, so that the GUI technology
is chosen on facts (see `docs/gui.md` for the requirements):

- a **float viewport**: a 2048 x 1365 image in the linear working space,
  32-bit float, uploaded as an `Rgba32Float` texture and drawn by the same
  WGSL shader in both (`proto/shared/viewport.wgsl`), with pan and zoom;
  each exposure change renders a quarter-size **proxy** then the **exact**
  image off the GUI thread (`proto/scene`);
- **panels** (viewport, graph, parameters, stats) that can be moved, docked
  and opened in their own native window, the layout saved and restored;
- a **200-node graph** drawn in a panel, laid out by the core's hints.

Both run headless in wbox (`proto/wbox/bench.sh`), 400 frames each, the
exposure changing every 10 frames.

## Measures (2026-10-11)

| | egui 0.36 (eframe, wgpu 30) | iced 0.14 (wgpu 27) |
|---|---|---|
| frame time, p50 / p95 | 20.7 / 68.2 ms | 20.2 / 102.2 ms |
| graph panel (200 nodes), p50 / p95 | 0.07 / 0.26 ms | 2.35 / 20.0 ms |
| float texture upload, p50 / p95 | 12.6 / 79.8 ms | 67.8 / 229.2 ms |
| proxy render (CPU, shared) | 6.7 ms p50 | same code |
| prototype code | 627 lines | 851 lines |

The compositor renders offscreen without the host's GPU, so absolute times
are those of a software path; the comparison holds, the numbers do not.
Renders are the same code in both (the engine); exact renders are mostly
served from the cache, which is why their p50 is near zero.

## What each one took

| need | egui | iced |
|---|---|---|
| float viewport | a paint callback with our own wgpu pipeline; worked first time | the `shader` widget with our own pipeline; same shader, worked first time |
| docking | `egui_dock`: tabs, drag between leaves, splits, floating windows | `pane_grid`: drag, drop and resize panes; no tabs |
| native windows | egui viewports: a panel opened in its own window, docked back when closed | the `daemon` (one view per window): same result |
| saving the layout | `DockState` serializes as is (serde) | the pane tree mirrored by hand to serialize it |
| large graph | immediate painting: cheap per frame | `canvas` geometry rebuilt each frame (a cache would help only when nothing moves) |
| widgets by name (tests) | AccessKit tree, `egui_kittest` queries by label | `selector` and `tester` features; not every widget takes an id (sliders do not) |
| programming model | immediate mode: state in the app, little ceremony | Elm: messages and update; more code, explicit flow |

## Recommendation

**egui**, for Helios's first GUI: the cheaper per-frame cost of the graph
panel and of uploads, docking with tabs and a serializable layout out of
the box, testing by label through AccessKit, and a third less code for the
same result. iced's explicit message flow is attractive for a large
application, and both share wgpu with the engine; nothing here is out of
reach for iced, it simply costs more. The choice also sets the wgpu version
the GPU backend will use (wgpu 30 with egui 0.36).

Not measured yet: HDR output (neither toolkit exposes an HDR surface
today), behaviour on Windows and macOS, and frame times on a real GPU,
which wbox with a GPU-capable compositor would give.
