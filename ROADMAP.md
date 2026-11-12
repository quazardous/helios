# Helios roadmap

Vision, settled concepts, phases and open questions for Helios. This is a
living document: decisions are dated, and an idea stays "not decided" until
it says otherwise.

Last updated: 2026-11-11.

---

## 1. Vision

**Helios** is a client/server, AI-first reinterpretation of
[darktable](https://www.darktable.org/). The motto: "the human-machine
interface is made obsolete by AI". darktable's learning wall is the problem
Helios solves: *the quality of darktable without having to learn darktable*.

The name comes from Niépce's heliography and the Helios-44 lens.

The project is called **Helios**, not darktable-helios (2026-10-11,
decided): a darktable prefix would present it as a darktable variant, while
Helios is an image processing framework where darktable and vkdt meet and
photo editing is the first application. The darktable fork keeps its name.

The logo (2026-10-11, decided; `docs/brand/logo.svg`) is an iris
diaphragm whose opening is a sun and whose blades' edges are its rays, all
in one light; every proportion comes from the golden ratio.

Underlying ambition (2026-10-10): **systematic normalization of the software
layers**. The reference is AVS (Advanced Visual Systems, scientific imaging),
where image processing workflows are built by assembling modules. Once every
layer is normalized, workflows become simple, and a better-written framework
lets the project move faster on all fronts: normalization is a multiplier,
not a feature.

**Photo editing is one application of image processing** (2026-10-11,
decided). The framework is a general image processing framework, AVS style;
photo editing (darktable's job) is the first application built on it, not
its definition. The core (data types, ports, graph, scheduler, vault) knows
nothing about raws, EXIF or film rolls: those belong to a photo domain layer
on top. Other applications (video, as vkdt already does, timelapse,
astrophotography, scientific imaging) stay possible without being built:
photo editing remains the only application that drives the work, per the
second-system safeguard below.

### Two repositories

1. **The darktable fork**
   ([quazardous/darktable](https://github.com/quazardous/darktable)):
   modifies darktable's libraries as needed, with justified **pull requests to
   upstream** when relevant. Minimal gap with upstream, a patch stack that
   must shrink. What cannot go upstream preferably goes here instead.
2. **Helios** (this repository): the client/server
   reinterpretation (kernel, vault, MCP, plugins, front). Strong divergence
   from darktable is accepted at this level.

Iconoclasm embraced, but a respectful method: darktable itself only receives
changes it can accept on their own merits.

### Constraints

- License: **GPLv3**, mandatory since Helios links with darktable.
- Language: **Rust first, best effort for the rest** (2026-10-11, decided).
  Everything new is written in Rust by default; another language is used
  only where Rust is not practical (the C bridge to darktable, GPU shaders,
  the web front), and that code is held to a lighter bar. A normalized
  interface becomes a stable boundary (C ABI) behind which one component at
  a time can be replaced, under an equivalence bench, without rewriting
  everything.
- Known friction: darktable module parameters are stored as raw bytes
  (database, XMP, pipe cache), so their memory layout is part of the
  interface; introspection is generated from the C structures; computation
  relies on OpenMP and OpenCL.
- Safeguard against the second-system effect: each normalization step must
  unlock a concrete need.

---

## 2. Concepts

### 2.1 One engine, several surfaces

(2026-10-10) **Module normalization is the foundation.** Each module
describes itself: its interface to the outside (parameters, widgets...) **and
its computation needs**. The framework and the scheduler then decide on
display and final render (proxy or exact, where, when); that is no longer the
module's problem.

The goal is **a single engine** (one technology: a graph of self-describing
modules) with **different surfaces** on top, not two engines side by side:

- a darktable surface keeping the linear workflow (a chain is a special case
  of a graph);
- a vkdt-style graph surface;
- an AI / workshops surface;
- a cloud surface (the technology must be easy to reuse in a 100% cloud tool).

(2026-10-11, decided) **Two GUIs on one engine**: in the long run,
darktable's legacy GUI plugs onto Helios (darktable becomes a client of the
engine and the vault), and Helios has its own diagram-oriented GUI. The GUI
technology is **egui** (2026-10-11, decided), on wgpu, after two
prototypes measured side by side (docs/gui-prototype.md): cheaper per
frame for large graphs and float uploads, docking with tabs and a
serializable layout (egui_dock), widgets testable by label (AccessKit),
and the same code compiling to the web (WebAssembly, WebGPU) for the cloud
surface. Its widget library (viewport, graph editor, widgets generated
from manifests) and a modern desktop (dockable, sticky panels, saved
workspaces) come next.
**A separate UI library** (2026-10-11, decided): the widgets are
extracted into an egui component library published on its own,
`egui-pop` (tokens and themes, icons, the normalized dialogs and menus,
sliders and parameter widgets, the graph view), themable, accessible,
driven by agents and tested headless; Helios consumes it. MIT licence,
its own repository (github.com/quazardous/egui-pop), the name reserved on
crates.io; crate layout and phases in docs/ui-library-research.md
(2026-10-11, decided).
Until it provides their components, the GUI waits and the core goes on
(2026-10-11, decided): new surfaces (graph editor, layouts, settings,
tips) are built on egui-pop, not twice; the engine, the vault, modules,
GPU and parity work continue in parallel.
Translation goes into egui-pop too (2026-10-11, decided): an official
`egui-pop-i18n` plugin (Fluent catalogues, language fallback, key
extraction) built on egui-pop's public API, moved there from
helios-i18n under the MIT licence; Helios then uses it.
Done (2026-10-12): the desktop runs egui-pop-i18n (a Language setting,
egui-pop's own text and Helios's in English and French); helios-i18n
keeps only Helios's catalogues, its message ids and the English
generated from its declarations, and `cargo xtask i18n` calls egui-pop's
extractor and checks.
A generic image viewport goes into egui-pop too (2026-10-11, decided):
pan and zoom, tiled rendering fed by a tile source trait (proxy then exact
tiles), a display-transform hook, overlays with handles in image
coordinates, before/after views, a pixel readout, thumbnails and a
histogram from supplied data. Raw decoding, colour science, the engine and
the scopes' computation stay in Helios, which implements the tile source
and the display transform.
Helios's viewport runs on it (2026-10-12): the engine is its tile source,
a Rec.2020-to-sRGB matrix its display transform, the view kept in the
value store; the "before" of a comparison is the edit as it was opened
(not decided: the unedited image, or a snapshot taken by hand).
Drawn masks are edited on it (2026-10-12): the mask's shapes outlined,
the selected one's handles on egui-pop's overlay kit, a drag one undo
step, shapes created by `mask.*` commands and keys (see docs/gui.md).
Decided (2026-10-13): a change of the network (creating or deleting a
shape, adding, moving or taking out a member, rewiring) is an undo step,
one per structural edit, restoring the values it moved. Decided
(2026-10-14): each version of an image's edit keeps the whole network,
and an image keeps its own once edited.
The Masks panel (2026-10-13): the selected module's mask network as a
tree on egui-pop's `TreeView`, live through its handle, its selection
two-way with the viewport's; rename, invert and opacity are undo steps
of the store, moving and deleting members rewire the network (to become
undo steps, as decided above).
Decided (2026-10-21): the mask display shows the node selected in the
Masks panel, else the selected module's final mask, as a coloured
overlay (theme tokens) or the mask alone in grey; one transient view
value behind a command, the `M` key and a button, on through edits,
rendered by the same engine and cache (see docs/design/mask-display.md).
Decided (2026-10-22): the desktop's default pointer profile is Adobe's
(Lightroom's habits: the wheel scrolls the panel, a slider takes it once
clicked or focused), in place of darktable's (2026-10-21), whose wheel
over a slider changed values while scrolling (darktable pitfall 7). A
saved choice is kept; the first run offers darktable's profile in one
click, and that profile stays faithful to darktable (see docs/gui.md).
A node can be bypassed (2026-10-21, done): it keeps its parameters and
the engine plans through it, its output the input it passes through,
so the image and its content keys are those without it. One undo step
(`node.bypass`, Ctrl+/ on the selected module, the Modules panel's
toggle), saved with the edit; agents reach it on the desktop and through
a node's control (see docs/module-contract.md, "Bypass").
The desktop runs on egui-pop's workbench (2026-10-12): its window frame,
per-window scale and interface zoom, panels in areas with auxiliary
windows and arrangements (Helios's image-first layout a template), the
keymap changed at run time (profiles as keymap documents), and its
exit; layouts saved before migrate (see docs/gui.md).
egui-pop wraps the application's kernel; it is not the kernel (2026-10-12,
decided). egui-pop owns what is UI: command declarations (for menus, the
palette, the keymap and traces), focus, overlays, layout, theme and UI
settings. Command execution, domain state, events and persistence belong to
the application, behind a host contract (dispatch with a deferred result, an
event stream with sequence, source and cause, a settings store, a way to
wake the UI from another thread). egui-pop ships a minimal default host
(in-process handlers and a small bus) so a simple application has nothing
to write; Helios plugs its bus and its node into the contract.
egui-pop is not set in stone (2026-10-12, decided): it exists to help build
Helios, adapts to Helios's needs and keeps Helios's code clean, breaking
its API before 1.0 when that is the cleaner path.
egui-pop's mechanisms are open and tweakable by design (2026-10-12,
decided): egui-pop handles the plumbing (shortcuts, commands, layout,
windows) but every feature exposes its lifecycle at each step, so the
application can observe it (signals on the host's journal), veto or
redirect it (before hooks), replace it (run hooks with a sane default) or
extend it (after hooks). Each hook is optional and traced in the debug
bridge; egui-pop never hides a step the application may need to own (first
case: undo and redo, answered by Helios's version history).
Undo and history (2026-10-14, decided; egui-pop's ADR 0014 and
docs/design/undo.md): histories belong to the kernel, behind egui-pop's
`History` contract; the interface routes the keys to them and shows them.
One history per scope: the image's edit (durable, built on its versions,
its undo and redo lists rebuilt on reopen), the library (ratings, labels,
tags, stacks, imports: for the session only, rebuilt from the audit trail
later) and the workspace; a component that edits something of its own
(the theme editor, the settings window, the code editor) keeps a local
scope. Ctrl+Z goes to a focused text input, then a focused local scope,
then the newest step among the scopes active in the window. A gesture is
one step, an agent's request one group, and each version names who made
it. Each scope keeps 500 steps in memory, a number the host can change.
No "compress history" for now. The history panel hides the redo states a
new step dropped, with a toggle that shows them as "replaced". Undoing a
copy-import removes the catalogue entries it added, never the files.
Snapshots (2026-11-04, decided: option A): taking one forces no save;
it is pinned to the version the edit's next save leaves, kept with the
image in the vault from then on, shown pending until then and lost if
the app quits first; restoring one is a new step an undo takes back
(built: docs/design/undo.md).
Hard requirement (2026-10-11, decided): GPU acceleration on Linux, Windows
and macOS, as close to Vulkan as possible, so the viewport can show buffers
the engine computed on the GPU without copying them back. wgpu is
the shared GPU layer for the GUI and the engine's GPU modules (2026-10-11,
decided with egui: Vulkan on Linux and Windows, Metal on macOS, raw Vulkan
access for vkdt interop), at the version egui uses (wgpu 30 with egui 0.37).
A process has one wgpu instance and one device (2026-10-12), which the
window is handed by the engine's side, against a Vulkan loader crash (see
docs/testing-strategy.md); a second GPU of the same machine is then not
used while the first is open (not decided: whether to allow one device
per adapter once the loader fix is widespread). A failing GPU falls back
to the CPU per step, and a lost device is rebuilt once, else the session
continues on the CPU (see docs/architecture.md, "GPU failures").
**One window is one workspace** (2026-10-11, decided), as in VS Code:
opening another project opens another window, never a project tab.
Auxiliary windows (panels detached to another screen) belong to their
workspace's window; tabs live inside editor groups. The shell is a panel
framework from the start: fixed areas (editor groups, side and bottom
panels, activity and status bars), panels declared rather than hard-coded,
every action a named command reachable from menus, shortcuts, the palette,
agents and tests. Shortcuts follow darktable's where they fit a clear
credo (decided 2026-10-21, see docs/gui.md, "Keyboard shortcuts").
**The node and its windows** (2026-11-12, decided by the owner): the
headless core (the library, the vault, the jobs, the kernel) is the
Helios node, the same one that can run on a server; every OS window, the
main one included, is a GUI client view of it. Quitting is the node's
policy, not a window's: Ctrl+Q quits from any window, Ctrl+W closes one
window, Helios stays open while any of its windows exists, and closing
the last window follows the same rule as Ctrl+Q. Ctrl+Q stops the local
node only when the desktop started it; a remote node and its jobs keep
running. Every window has a menu bar with at least a Window menu. See
docs/design/windows.md, section 7.
**The node and client split** (2026-11-12, decided by the owner): locally
the node is a library embedded in the desktop process (zero copy kept),
behind the same API as a remote node, and a separate `helios-node`
process runs only for remote or headless use; edit sessions and undo
history live on the node, windows keep view state and gestures in
progress; thumbnail generation and its disk cache are the node's, textures
the client's; files are served by id, as a path when the client sees the
volume (checked by size, mtime and a partial hash, remapped per volume)
and as a stream otherwise, with a client resource agent for sources only
the client sees; secrets stay where the work runs, the client only hosts
the consent browser. Streamed node renders are not the default for a
remote darkroom. See docs/design/node-client-split.md.

**Helios makes darktable and vkdt meet** (2026-10-11, decided as the
long-term goal). darktable brings compatibility (edits, workflow, users) and
CPU computation; vkdt brings the graph and GPU speed. The meeting point is
the module contract: one description able to host a module coming from
either side, so both lineages run in the same graph. vkdt modules are
**ported**, not bridged (2026-10-11, decided): a Rust CPU reference and a
WGSL variant hand-translated from their GLSL, checked against `vkdt-cli` as
darktable's modules are against `darktable-cli`. Embedding libvkdt would
bring a second Vulkan device, which vkdt creates itself; a port keeps one
device, the engine's cache and tiling, and the CPU reference. **No vkdt
fork** (2026-10-11, decided): small fixes go upstream as pull requests,
and the question comes back only if a needed change is refused.
**Modules that expand into a network** (2026-10-13, done): more than
half of vkdt's modules are several kernels, and a macro is a network a
user packaged; both are one mechanism, a composite node the engine
expands when it plans a render, each inner kernel a step of its own (its
region propagated through the inner wiring, its content key, its GPU
variant, cache and tiles, union planning as for any node). Design in
[docs/module-contract.md](docs/module-contract.md), "Composites". First
port: vkdt's `contrast` (a guided filter, its blurs a cascade at lower
resolutions), tiled equal to untiled, its one-input kernels on the GPU,
and against `vkdt-cli` within darktable's thresholds away from the
border (Delta E 2000 inside a 16 to 32 pixel border of a 256x192 chart:
max 0.04 to 1.23, mean 0.02 to 0.12). Not decided: proposing upstream a
clamp of the guided filter's variance, which goes below zero next to
edges after the Catmull-Rom upsampling and makes the model singular
where it crosses `-eps` (a spot of 10 Delta E with the default edge
threshold, in vkdt as in the port); and an accelerator running kernels
of two inputs or more (`guided3`, `combine` run on the CPU today).

What darktable already offers to build on:

- parameter introspection (`DT_MODULE_INTROSPECTION`, ~93 modules);
- widgets bound to parameters (`dt_bauhaus_slider_from_params`, ~69 modules;
  the rest of `gui_init` is imperative);
- memory needs via `tiling_callback` (factor, maxbuf, overhead, overlap; ~36
  modules have their own);
- an OpenCL variant (`process_cl`, ~77 modules).

Computation and the GTK GUI live in the same file and the same library: they
must be separated for use without GTK.

**Isolated modules, layered framework** (2026-10-11, decided). The fork
untangles darktable's legacy; this repository starts writing the future
engine. Every piece of image processing is completely isolated and testable
on its own: no IO, no global state, no GUI, only declared inputs, parameters
and outputs. The code is therefore **horizontal for features** (many
independent modules side by side, adding one touches nothing else) and
**vertical for the framework** (a deep stack of layers: module contract,
graph, scheduler, vault, surfaces). Modules are written in Rust (section 1,
constraints); the module contract and the test oracle (darktable's own
render) are not decided yet; draft in
[docs/module-contract.md](docs/module-contract.md).

**Retouch on proxy, deferred exact render** (2026-10-10). From daily
newspaper press: retouching on thumbnails, then the same retouch replayed on
full-resolution images for the print RIP. Requires resolution-independent
parameters. darktable already does part of it (reduced preview pipe,
`dt_iop_roi_t.scale`, ~51 modules adapt their radii to the scale). To be
declared per module: scale invariance, approximation quality.

Modules are bound by the design rules of section 2.8: no hidden IO
(rule 2), pure reads (rule 6).

**Batch parallelism.** darktable exports sequentially, one image after the
other; parallelism only exists inside a render (OpenMP, OpenCL). Database
writes during export (tags, timestamps) are mixed with computation and must be
separated, as the catalog SDK does.

**Camera colour calibration from a chart** (2026-11-11, plan accepted).
A ColorChecker 24 shot under the scene's light, its four corners placed
with the masks' handle kit in a Calibration focus, the patches measured
in linear camera RGB before `colorin`, a 3×3 matrix solved on Delta E
2000 (an optional residual LUT off by default), a report per patch, and
a profile per camera and illuminant kept in the vault and applied by a
`chartprofile` module after `colorin`; agents run `calibrate.chart`
with a dry run. Design, slices and open questions:
[docs/design/calibration.md](docs/design/calibration.md).

### 2.2 Kernel and scheduler

- Content-addressed work units, offloadable anywhere (client, GPU PC by
  default, NAS, cloud), cancelable.
- Scheduler priorities: strict config rules > assignments >
  preferences/weighting > **auto mode based on measured execution speeds**.
  Every decision is explainable.
- The scheduler chooses between moving data and moving compute (render on the
  node holding the raw, ship only the result).
- **Adaptive visualization**: direct viewport, shared memory, tiles or a video
  stream depending on the case.
- No implicit background work: every job is named, cancellable and visible,
  and "wait until idle" is part of the API (section 2.8, rule 3).
- (2026-10-11, decided) **A central bus for orchestration, not for data.**
  Typed events coordinate the parts (state changed, job finished, buffer
  ready); image data travels on the graph edges and stays where it is
  computed (vkdt keeps it in GPU memory). Commands go to the owner of the
  state. Draft in [docs/architecture.md](docs/architecture.md).
- (2026-10-11, decided) **Data access is a framework prerogative.** Modules
  and services declare what they need and never read or write data
  themselves; the framework batches, projects, caches by content, prefetches
  from declared intent, coalesces writes and skips unchanged ones, so no
  read or write is wasted. Read and write budgets are checked by tests.
  Above all, identical photo data is never loaded twice in RAM or VRAM:
  buffers are addressed by content, held once per memory, shared by
  reference, and concurrent requests for the same content are merged.
  Explicit locks are exceptional (mainly exports and other sinks writing
  files), acquired by the scheduler as expiring leases, never by modules.
- (2026-10-12, decided) **A node is computed once per render, over the
  union of what its consumers read.** Planning collects the regions each
  consumer asks of a node, computes the node once over their union, and
  hands each consumer its crop; a step is no longer one exact region. (The
  stress network of 58 mask nodes took 386 steps, about 17 times the pixels
  of one pass, when each distinct region was its own step; done
  2026-10-12: 58 steps, 9 times faster, a sixth of the memory.)
- (2026-10-12, decided) **The cache has a memory ceiling by default.** A
  configurable budget (a share of RAM by default) with least-recently-used
  eviction; without it one 1536×1024 render held 3.9 GB, and tiles only
  bound memory once the cache has a budget. Done 2026-10-12: 25 % of
  memory by default, a render's working set pinned until read.
- (2026-10-12, proposed, not decided) **Whole-image statistics are
  analysis nodes.** A module needing them (haze removal, highlight
  reconstruction, auto-tune) reads a small value from an analysis node
  that reads its input's whole extent at a scale it declares, keyed
  without region or render scale, so preview, tiles and export agree; the
  value can be frozen as parameters. Design note and open questions:
  [docs/module-contract.md](docs/module-contract.md), "Full-extent
  analysis".
- (2026-10-12, proposed, not decided) **Modules declare their interaction
  needs; one ergonomics layer runs the rituals.** A module declares typed
  needs (a colour sampled from a point or an area, a target, a line or a set
  of lines, a region, a range picked from the image, a mask...) and how the
  gathered data becomes parameters; one layer runs the gestures the same way
  everywhere, per interaction profile (Lightroom, darktable, Helios). A tool
  the layer cannot serve writes its own ritual inside the same contract
  (state, undo, Esc, keyboard alternative, profile remapping, traces) and
  can be promoted to a shared one. Study: [docs/ergonomics/](docs/ergonomics/README.md).
- (2026-10-12, idea, not decided) **AI-defined rituals and a ritual
  editor.** Rituals become data composed only from registered primitives,
  scoped to a project and driving a process network; an AI can propose one
  on the fly, validated, dry-run and confirmed by a human before it is
  saved; a ritual editor edits and tries them. Until then the ritual
  contract stays serialisable and free of hidden state.
- (2026-10-22, decided) **Batches run through the workflow
  journal.** A batch submitted to a node process becomes a run: one
  journal row per item and queueing graph node (the export, then decision
  nodes), kept in the workspace's vault, worked by claim and conclude
  with renewed leases, retries with backoff, cancel, retry and
  reprioritise as control verbs and MCP tools, monitored per graph node
  and relayed to the desktop's Queue panel; a crashed node resumes its
  runs. Single exports and one-off renders stay jobs. Design note and
  open questions: [docs/workflow-level.md](docs/workflow-level.md).
- (2026-11-07, decided: option A) **Batch export on the desktop, as
  local jobs.** `library.export` (File > Export…, `Ctrl+Shift+E`, the
  Library's context menu) exports the selection or what the library
  shows: a parent job and an export per image on the desktop's own
  scheduler, in the jobs centre and the Queue panel, from a serialisable
  batch request (`helios_export::batch::BatchRequest`: settings, and each
  image with its head version and its output path decided when asked).
  Built (2026-11-07), with the "Batch export" template. Not decided:
  sending the same request to a node as a workflow run, which the request
  is shaped for ([docs/export.md](docs/export.md), "Remote later").

### 2.3 Vault: the catalog as an API

The SQL database becomes an API (the **vault**) with **cursors**. Phase 0 is
first an iso SDK in C inside the fork (see section 3).

**Storage is abstract, engines are drivers** (2026-10-11, decided). The vault
talks to a store contract only (`Store` and its contract suite); each
storage engine is a driver in its own crate that passes the contract.
SQLite (rusqlite, bundled) is the first driver, an in-memory store serves
tests. Nothing above the store names an engine, so another one (a
key-value store, a server database for a shared catalog) is one more
driver, not a rewrite.

**Identity by content, not by path.**

- (2026-10-10) Helios does not own the folders. Files stay wherever the user
  keeps them; identity is the content hash only; a **repair scan** relinks
  moved or renamed files by hash, and the path is just a "last seen" hint.
  darktable's `images` table has no file hash today (identity = film roll
  path + filename).
- Prefer a modern hash (BLAKE3 / SHA-256). Full hashing costs one read of the
  archive: prefilter by size + partial hash. Tools that write into the file
  (e.g. XMP written into a DNG) change the hash: consider hashing the image
  payload, or a fallback match on capture time + camera serial + size.
- Folders become views (queries, tags, projects, client deliveries), so
  nothing is ever moved. Counterweight: folders are trusted because they are
  transparent and outlive any app. Needed: a guaranteed exit (export or mount
  the views as plain folders, e.g. FUSE / WebDAV) and originals kept
  byte-identical.
- (2026-10-11) **File copies are just information**: an image identity
  carries a list of locations (path, volume, last verified), used for
  redundancy count, offline availability and "which drive has it". Not to be
  confused with darktable "duplicates", which are edit variants (versions).
  (2026-10-12, implemented; open to revision) Importing darktable edits,
  each sidecar of a photo (base, `_NN` duplicates, case variants) becomes
  a first-edit version of the one image, side by side as heads, never
  merged; only the base gives the rating. A history item Helios cannot
  apply is kept raw in the version, reported as not applied, and never
  applied to the whole image when it was masked. (2026-10-29, decided)
  Duplicates stay plain heads; a notion of named variants can come later if
  users ask for it. An orphan sidecar (its image gone) is kept in the
  vault's import record with its import report, not only listed. A step
  set to a mode Helios does not have is applied with the mode Helios has
  and reported as substituted (asked, run), never silently. Real old
  sidecars for the tests are fetched from darktable's test repository at a
  pinned commit, never committed here (no stated licence, personal
  metadata in one of them).
  Near-duplicates (RAW+JPEG pair, burst, re-encode) have different hashes and
  are relations, also data.
  (2026-10-13, implemented; open to revision) Import is a library
  operation run as a job: a folder or a mounted card, referenced or copied
  into a library folder laid out by capture date (the one place Helios
  writes files of its own), renamed by a pattern, sidecars along; exact
  duplicates (same content) are skipped by default, kept or listed for an
  answer, never deleted. Each image gets near-duplicate signals (a
  perceptual hash of its preview, capture time, exposure, frame number,
  sharpness). Bursts and brackets become **stacks** (members, a top,
  expanded or collapsed) only when a person accepts a `stacks.suggest`
  decision, whose default resolver is rule-based and whose resolver name
  is open to AI models. See docs/library.md. Not decided: RAW+JPEG pairs
  as stacks or as one image with two files; MTP sources.
  (2026-10-13, implemented) Culling runs on egui-pop's library widgets:
  the grid and filmstrip, Lightroom's culling keys as commands (with
  auto-advance), picks, rejects, labels and tags as vault fields written
  through egui-pop's library steps, the search filter as a vault query,
  a comparison of two to four images; stacks collapsed in the grid, the
  `stacks.suggest` proposals reviewed in a Stacks panel (accepted,
  corrected or declined), and the `ask` duplicates answered file by file
  in a Duplicates panel. The built-in template "Import, cull,
  de-duplicate" sets it up, its import source part of the template
  format. Not decided: whether stacks are proposed after every import by
  default (the template does, as a setting of its own).
  (2026-11-11, implemented) **Compare is a focus**: two to four photos
  side by side in a place of their own (Lightroom's Survey and Compare,
  darktable's culling), each drawn from its edit, zoom and pan
  synchronised by default, the arrows moving the active photo, its marks
  at hand; opened by `C` on a selection or File > New workspace >
  "Compare". Its state is the focus's, so it can become a window with
  the multi-window work.
  (2026-11-06, decided) **"Library" is the only word users see.** The
  vault is an internal notion: the normal interface never shows it (at
  most an advanced settings section may give its path and engine) and
  never says whether the library is local or remote. The Vault panel is
  the Library panel ("Bibliothèque" in French), with its strings, help,
  tips and menus; command ids keep their names. (2026-11-06, decided)
  **Importing is always within reach**: File > Import… (Ctrl+Shift+I, as
  Lightroom), "Import photos…" in the empty Library and Filmstrip, files
  dropped on the window; without an import source a short dialog asks
  where photos go (referenced in place by default, or copied into the
  library folder), "remember for this workspace" making the answer its
  import source. (2026-11-06, decided) **darktable edits come in with an
  import**: each sidecar (base and `_NN` duplicates) a version of its
  image, never merged, on by default with a checkbox; what was not
  applied or was substituted goes to the Log panel and a short toast.
  Later, not decided in detail: importer plugins under File > Import
  from →, and reading darktable's `library.db`.
  (2026-11-11, decided) **Import connectors**
  ([docs/design/import-connectors.md](docs/design/import-connectors.md)):
  a connector trait (paged listing, decorative path, metadata, claimed
  hash checked after download), folders and cards in process, Google
  Photos, phones and remote storage as source plugins; saved imports
  with filter, tags and triggers, automatic runs confirmed once, and a
  history with "Import again". Google Photos is reachable only through
  its Picker API since 2025 (hand-picked, no automatic import). The
  owner accepted every recommendation, C1 to C19 (2026-11-11); slice S1,
  the connector trait and registry with folders and cards behind it,
  comes first.
  (2026-11-11, decided) **The Library focus** (docs/design/library.md):
  the owner accepted the nine recommendations: capture time as the
  default order; folders, albums, smart albums and a quick collection;
  metadata written to sidecars on command, never into raw files; remove
  from Library and move to the system trash; one cell per image with
  copies shown on request; RAW+JPEG pairs as stacks made at import; path
  keywords first; file operations inside library and archive volumes;
  browse state per window. Slice L1 (the library read at scale) first.

**Sync model.** (2026-10-10) Take git's *model* rather than git itself:
content-addressed objects, a DAG of versions, refs, offline replicas,
integrity. Raws are large immutable blobs stored git-annex style (hash
pointer, separate object store), never in the history. Edits are small text
(named parameters) and fit a version DAG; ratings, labels and tags merge best
per field (CRDT-like), which requires field-level writes (section 2.8,
rule 1). Decided 2026-11-11, see below.
(2026-11-11, decided by the owner) The design note
[docs/design/sync.md](docs/design/sync.md) turns this model into
decisions, each with a default: a replica is a full vault plus partial
objects (previews everywhere); replicas exchange a change log per
replica by frontiers (version vectors), objects fetched on demand and
verified by hash, over a transport abstraction (a shared folder, the
local network with pairing, also over Tailscale, SSH: the user picks; owner, 2026-11-11);
fields merge per kind (last writer by
hybrid clock for single values, add-wins sets for labels and tags,
versions never merged); pins say which originals a machine keeps; backup
becomes continuous with scrubbing; reclaim never removes a copy without
live confirmation of the others. First target: a laptop and a
workstation, both offline at times, reconcile without losing anything.
Its questions are settled with the recommended answers: the shared-folder
transport first, then the network link; last writer wins for a single
value, the other kept in the history; only backup and archive roles count
as backups; the laptop keeps its imports until safe, its pins and the
previews; no developing without the original in v1; 30 days before
reclaim. Building is parked until sync is needed.

(2026-10-11) **Photos are not code.** No semantic merge: two concurrent edits
simply coexist as two versions, and the user keeps one or both.

(2026-10-12, implemented; open to revision) **Each edit step is a version.**
A surface saves every committed step (a gesture's end, an undo, a redo, a
parameter set by an agent, a decision answered) as a version continuing
the one it saved last, in one transaction, and a drag every second while
it goes; an undo is a new version, so the history is the line of first
parents and its end is the head. On open, the newest head (latest time,
then longest history). Ratings, rejects and labels are field commands
(set, set if absent, add, remove), never a record written back. Quit and
`kill -9` scenarios gate a release. See docs/persistence.md.

**Offline-first.** The travel laptop and the home workstation are both
replicas; today photographers reconcile them by hand (XMP last-writer-wins,
nothing for presets and styles).

**Placement and caching.** (2026-10-11) Multi-NAS and hybrid libraries are
not a special case: the framework moves a resource to where it is needed and
tracks every copy. Durable copies count for redundancy; caches are evictable
and do not. Policies to design: pinning (keep for travel), prefetch, transfer
verified by hash, bandwidth / egress cost, placement rules (e.g. client images
never leave the premises). Multi-workstation is the same model: each machine
is a node with replicas and caches.

**Continuous background backup.** (2026-10-11) The redundancy policy is
enforced all the time: anything below its target copy count is replicated in
the background, throttled, plus scrubbing (periodic hash verification, repair
from a good copy). It must not behave like sync: objects are immutable (a
ransomware-encrypted file is a new hash, the old object stays), deletions are
tombstones with a retention period, offline / offsite disks get "last
verified" reminders.

**Deleting becomes reclaiming space.** (2026-10-11) The logical intent ("I do
not want this image": tombstone, reversible, hidden from views) is separate
from physical reclamation, a gc-like policy run when space is needed: drop
caches first, then copies above the target count, then tombstoned objects past
retention. Erasing the last durable copy needs explicit confirmation.

**Retention tiers.** (2026-10-11) Essential (target copies, never reclaimed
automatically), normal, non-essential (reduced to fewer copies, then to a
proxy: preview + metadata + edits, raw dropped last). Defaults can follow
ratings and AI suggestions (blur, near duplicates). Dropping a raw is
irreversible, so it stays a visible, confirmed step.

**Engine version drift.** An edit records the module versions it was made
with. A newer engine renders old edits (legacy params); an older one cannot
render newer edits, so the edit is rendered on a capable node or the user is
asked to upgrade. Rendering an old edit never rewrites it: upgrading is an
explicit operation creating a new version (section 2.8, rule 6). Presets and
styles live in the vault so every node has the
same ones. This answers the objection raised by darktable maintainers on
multi-workstation setups
([darktable#18599](https://github.com/darktable-org/darktable/issues/18599)).

The NAS storage agent (index, embedded previews, versioned XMP, identity by
hash) is part of Helios; deploying a NAS is not.

### 2.4 Projects and sessions

- (2026-10-11) Photographers talk about a shoot / session / job (client,
  event, date), not film rolls; darktable's film roll is just one imported
  folder. Typical phases: ingest, cull, edit, export, deliver, archive.
  Reference: Capture One Sessions vs Catalogs. Not yet researched.
- darktable has **no "work in progress"**: no way to leave the desk as is over
  lunch. It always starts in the lighttable; it keeps the collection, its
  scroll offset and the selection, but undo and snapshots are lost on quit,
  and the image open in the darkroom, compare / culling sets, open modules and
  pickers are not restored.
- **Projects / contexts** as first-class objects, many in parallel (a personal
  side project, paid work and a niece's wedding at the same time), each with
  its own workspace state (saved, restored, resumable on another machine),
  settings (default modules, styles, export targets, watermark, copyright),
  retention and placement policies, visibility (personal vs client), and
  progress. Images can belong to several. darktable today: one config
  directory = one library and one set of preferences.

- (2026-10-13, decided) **A context stack** holds where the user is
  (places: a sub-workspace, a group, a mask sub-tree) and the modes that
  apply everywhere (a work orientation such as culling or retouch, a
  compare, "an agent is driving"). The kernel owns it; the display reads
  it and asks for transitions. It is written at each transition, so a
  workspace reopens exactly as it was left. egui-pop provides the
  contract and the projection, Helios the owner. See
  [docs/design/context-stack.md](docs/design/context-stack.md).
- (2026-10-13, decided) Importing from another workspace or template
  **copies** the parts (no link kept); **delegating** part of a network
  to a sub-workspace is a link, entered and left through the context
  stack.
- (2026-10-18, decided 2026-10-21) **Copy and paste** go through a clipboard the
  kernel owns: typed items (an edit or part of one, nodes, masks, a
  colour, library images, workspace parts) with readable fallbacks on the
  system clipboard, stripped of local paths and secrets when they leave
  the vault, and adapted on paste (module versions, image size, colour
  space, names). Each paste is one undo step. Proposed in
  [docs/design/clipboard.md](docs/design/clipboard.md), with egui-pop's
  ADR 0015.

### 2.5 Workshops and AI

- (2026-10-11, decided) **AI junctions in the network**: any processing
  network can contain a decision node, a point where the algorithm alone is
  not enough and a judgment sets parameters, picks a mask or a branch, or
  validates a render. Three resolution modes: full AI, AI-assisted (the AI
  proposes, a human confirms) and human. Its answer is recorded in the edit,
  so renders stay deterministic and asking again creates a new version.
  Design in [docs/module-contract.md](docs/module-contract.md).
- **Recipes** (diagnosis -> tool), **workshops** (a customized recipe),
  **retouches** (an applied workshop).
- In AVS terms, a workshop is a **macro**: a network of modules packaged as
  one, exposing only chosen parameters (see
  [docs/module-contract.md](docs/module-contract.md)). The engine runs
  macros (2026-10-13): a macro is a composite, expanded at planning like
  a multi-kernel module, and shares its content with the flat network it
  packages. Not decided: how a macro is saved with an edit and offered
  in the GUI (folded, opened, promoted from a stack).
- (2026-10-30, decided) **Macros on the desktop**: made from the
  graph's selection or the Modules panel (Ctrl+G), an editor for the
  exposed parameters (renamed, ranges narrowed, ordered, grouped),
  folded and opened in place on both surfaces, saved as a versioned
  composite file in the workspace then published, each action one undo
  step and an MCP verb; the ten defaults of
  [docs/design/macros.md](docs/design/macros.md) accepted, built in six
  steps. Steps 1 to 5 are built: the exposed parameters' interface in
  the composite format, the pure steps on a workspace, the desktop's
  steps (2026-11-03), the interface editor with groups in the
  Parameters panel (2026-11-03) and versions: a macro changed while
  open saved, exposed or discarded on folding, updates offered to its
  other nodes (2026-11-04). Step 6, publishing, is built (2026-11-04):
  `macro.publish` copies a macro's version into the user's library
  (`<config>/composites/`), which the graph's Add menu offers to every
  workspace; never undone, asked first, and an agent publishes only
  with the user's confirmation (open question 9's default).
- (2026-10-13) **Workspace templates**: a ready-to-use combination of
  tools, network and panel layout for one known scenario, shared as a
  `.helios-template` bundle like a plugin, schema-checked and built
  headless so an AI can later design one on demand (a `template.design`
  decision). Composites are saved as their own versioned files; a
  template pins a composite's version and a newer one is offered, never
  taken silently. First built-in: "Portrait retouch". A template's
  source may take a raw file's sensor data with its camera (2026-11-03);
  "Develop one image" develops a raw through the node's raw chain on the
  desktop, "Develop (preview)" stays for pictures. A template's library
  may open on a query, the picks only for instance (2026-11-03), and
  keep its export's settings: "Batch export" opens on the picks with the
  export ready (2026-11-07). Design in
  [docs/templates.md](docs/templates.md). The Masks panel, the Scopes and
  the Navigator open as floating palettes by default (2026-10-28).
  Not decided: the toolbox's
  widget, folding an opened composite back, a registry of shared
  templates, and whether the AI may write composite files.
- A workshop: the AI translates an artistic intention ("too contrasty with the
  blue sky") into a mini workspace: the real darktable modules, pre-calibrated
  for the photo, with only the useful sliders (restricted ranges, tested at
  the extremes), plus composite semantic sliders.
- Workshops are savable and reusable: they store the logic (relative targets,
  zone detection), not absolute values.
- Hypothesis to validate before building: mockup, a few photographers,
  measured time saved. "AI that learns your style" is already commoditized.
- (2026-11-05, decided) **Workshops v1 built as an experimental
  feature**, without the mockup test first. A workshop is a macro whose
  file carries an intent, keywords and relative targets (measures solved
  per image on a proxy render); a Workshops panel behind the
  `interface.experimental.workshops` switch finds, applies and re-aims
  them, each one undo step; the AI designing a workshop, zones and
  series come later. Design and plan, not decided in their details:
  [docs/design/workshops.md](docs/design/workshops.md). v1 built
  (2026-11-05): the switch, the format's block, aiming, Lift midtones,
  applying, the panel, saving a macro as a workshop.
- (2026-11-11, decided 2026-11-11) **Workshops made on the fly**: the AI
  creates the workshop from an intent (real modules pre-calibrated,
  only the useful sliders, semantic sliders), the user refines it on a
  test photo or two half by hand, the session is recorded and
  generalised ("Make it automatic": targets where a measure explains a
  value, deterministic rules where one fits, AI junctions only where
  needed, counted), then it runs on a set within limits the user sets,
  with a contact-sheet review. The owner accepted every recommended
  choice (C1 to C8) on 2026-11-11. Design, slices and choices:
  [docs/design/workshops-on-the-fly.md](docs/design/workshops-on-the-fly.md).

### 2.6 Collaboration

- Anchored messages and threads (image, zone, version, workshop); the AI is a
  participant that makes proposals (branches in the version graph).
- **Multi-profile** (photographer, retoucher, art director, client...), with
  layered taste (style guide > client > photographer > retoucher).
- Real gap on the studio side, but a different audience than current
  darktable users.

### 2.7 Plugins and front

- Plugins from the start: front, server or both, with a manifest and
  permissions. **90% without front code** thanks to a declarative interface
  specification, the same language for plugins, workshops and AI.
- Front: authority is server-side, but computation can happen anywhere (the
  client is a node; WebGPU / local engine; approximations replaced by the
  exact darktable render).
- (2026-10-11, decided) **Two kinds of extension, one manifest**
  ([docs/extensions.md](docs/extensions.md)):
  - a **module** is a pure, deterministic unit of a network under the
    module contract: no IO, no UI code, no commands, sandboxable;
  - a **plugin** extends the application (panels, views, commands,
    importers, exporters, services) and is the unit of packaging; it may
    ship modules;
  - saved sub-networks are user data, not a third kind.

  The declarative manifest is read before any code loads. Keybindings in
  it are suggestions only. Capabilities are enforced out of process, and
  called a disclosure until they are.

### 2.8 Design rules

(2026-10-11, not decided) Six rules drawn from a darktable debugging session:
a rating set from Lua was silently reverted one run in three, because a
full-image load started by a background job copied the whole image record,
decoded the raw, then wrote the stale copy back
([darktable#22569](https://github.com/darktable-org/darktable/issues/22569)).
Each rule removes one thing that made this hard to find.

1. **Field-level writes.** A writer declares the fields it owns and updates
   only those (patch or command), never "write back my copy". This is also
   what per-field merge between replicas needs (2.3).
2. **No hidden IO.** Modules never touch files, storage or the catalog; all
   IO goes through a framework layer that can be observed, mocked and
   serialized (2.1, [module contract](docs/module-contract.md)).
3. **No implicit background work.** Every unit of background work goes
   through the scheduler, named and cancellable, and the system exposes
   "is idle / wait until idle" for tests and scripts (2.2).
4. **Audit trail.** Writes record who, which fields, old and new value and
   from which job, available on demand, instead of patching in backtraces.
5. **Structured events, not free-text logs.** Events carry their source
   (module, job, image) and can be filtered; modules do not log directly.
6. **Pure reads.** Reading never changes stored data; migrations (legacy
   parameters, schema upgrades) are explicit operations that create a new
   version (2.3, engine version drift; decision nodes are recorded, not
   recomputed, for the same reason).

Rules 3 to 5 are framework requirements in the
[testing strategy](docs/testing-strategy.md): building Helios must be easy
for an AI agent, so the system is observable and waitable by construction.

---

## 3. Phases

### Phase 0: iso catalog SDK (in progress, in the fork)

All SQLite access in darktable goes through an in-process C SDK, with
strictly identical behavior. No network API. Branch
`helios/phase0-catalog-sdk` of the fork; design in the fork's
`doc/helios/PHASE0.md`.

Validated (2026-10-10): location `src/common/catalog/` with per-domain
`catalog_*.h` headers and a `sqlite/` backend; batch-write targets (`cursor`
first, `ids` for small cases; `selection` and `query` targets to be rediscussed
at step C); selection stays in the database during phase 0.

(2026-11-11, decided) Finish step A first, then start step B domain by
domain. The work stays in the fork: no upstream pull request, issue or
contact from now on. The domains already merged upstream stay there.

- [ ] **Step A: no SQL outside `common/`**, one domain per commit, checked
      by an equivalence bench (SQL dump + XMP, plus GUI captures) and a SQL
      inventory whose counter must go down. Counter (fork's
      `tools/helios/sql_inventory.py`): 235 sites on the upstream base of
      2026-10-09, 0 on 2026-11-07, then 15 after the rebase of 2026-11-07
      onto upstream master, which brought upstream's new flexi masks code
      (`src/develop/masks/`, `src/develop/masks_gui_presets.c`).
  - [x] 1. ratings and color labels (merged upstream, d846d4946)
  - [x] 2. metadata (merged upstream, cd6a056aa; the image information
        "various values" query in the fork, dd8f577b3)
  - [x] 3. tags (merged upstream, d304a8b90)
  - [x] 4. film rolls and images (merged upstream: 05868f989, a63c00c8b,
        62c225f30, 544a893f8, fecc1b00e)
  - [x] 5. locations, overlays (nothing to move: their SQL was already in
        `common/` on the base)
  - [x] 6. styles (merged upstream, 7f504191d)
  - [x] 7. presets (fork, 13 commits, the last 32ba57e9d)
  - [x] 8. history (fork: f55b1470a, 7fed4fa53, 5629e4ac6, 9e0912961)
  - [x] 9. selection and collection (fork, 20 commits, the last a90a1e396)
  - [ ] 10. the `darktable-mcp` bridge: its 9 sites are classed as an
        interface layer (the bridge runs on an injected database in its
        tests), so the counter leaves them out; moving them through
        `common/` is not decided
  - [ ] 11. upstream's flexi masks code, arrived with the rebase: 15 sites
        in presets (3), images (1), and masks history and its scratch
        images (11, critical, reinforced tests)
  - [ ] SQL inventory check (`sql_inventory.py --check`) added to CI once
        step A is done
  - [x] make the Lua bench deterministic (2026-10-11: ten consecutive runs
        byte-identical; see the fork's `doc/helios/PHASE0.md`)
  - [ ] performance bench: catalog operations on a synthetic 100,000-image
        library are timed (`perf.sh`, a bench box job); scrolling the
        lighttable is not
  - [ ] a GUI bench for each domain whose code path the Lua scenario
        misses: labels, metadata, tags and presets have one
  - dropped (2026-11-11 decision): reporting the "Lua as a library" race
    upstream, and contacting upstream with a first PR
- [ ] **Step B: SDK interface**: `common/` functions grouped behind
      `catalog_*.h`, SQL moved into the sqlite backend, transactions become
      `dt_catalog_batch_*`, the image cache fed through the SDK.
- [ ] **Step C: cursors, targets, query model**: cursors (ordered snapshot,
      count, fetch with projection, rank, version + delta, several at once)
      replacing `memory.collected_images`; a typed query model replacing the
      dynamic SQL of collections, producing the same SQL as today.

Upstream framing (until 2026-11-11): phase 0 was only acceptable upstream as
a cleanup (less SQL in the GUI), never as a step toward client/server.

### Phase 1: exposed parity

- Reuse the official `darktable-mcp` for "100% iso" parity.
- Inventory of IOP modules, libs, imageio and Lua scripts: keep / available /
  compatibility / outside MCP. First tool: the
  [IOP inventory](docs/iop-inventory.md).
- Delta E parity matrix against `darktable-cli`.

### Phase 2: this repository

(2026-10-11) Starts now, in parallel with phase 0, with the isolated module
contract (section 2.1).

- Layout: `protocol/`, `kernel/` (Rust), `engine/`, `vault/`, `mcp/`,
  `plugins/`, `front/`, `parity/`, `docs/`.
- A `remote` backend for the catalog SDK: the vault API with server-side
  cursors.

### Later phases

To be defined with the next normalized layers (module self-description,
scheduler, vault replication, surfaces).

---

## 4. Prior art

- **Official `darktable-mcp`** in darktable master: stdio JSON-RPC, module
  introspection (parameters by name, schemas), headless rendering with preview
  and statistics, history, styles, ratings. Do not redo what it does.
- **AI in darktable master**: ONNX backend, segmentation for object masks,
  restoration.
- **mmcc-xx/darktable**, branch `darktable-api`: a persistent headless engine
  (Unix socket, multi-client, web app). Could not be found again; to be
  confirmed.
- **w1ne/darktable-mcp**: sorting on previews, non-destructive XMP patching,
  tool schemas frozen by tests, SAM2.
- No client/server or NAS-oriented darktable exists, only containers that
  stream the GUI.
- Commercial references: Capture One (Sessions, multi-user LAN mode in beta),
  Lightroom Classic (no shared catalog).

---

## 5. Needs check (summary)

Checked against darktable issues, discuss.pixls.us and competitors
(2026-10-10). Evidence, sources and measurements: [docs/NEEDS.md](docs/NEEDS.md).

| Axis | Verdict |
|---|---|
| Client/server catalog, NAS, multi-workstation | the best-supported need, but outside upstream |
| Self-describing modules, headless / cloud engine | an architectural means, not an expressed need |
| Parallel batches | a real need on one machine; nobody asks for multi-machine |
| AI-first, intent -> workshop | a hypothesis to validate; v1 built as experimental first (2026-11-05) |
| Collaboration, roles | a real need of a different audience (studios) |
| Plugins | secondary (Lua already serves) |

---

## 6. Open questions

- **Primary target audience: solo photographer or small studios with
  clients?** This decides everything else (see section 5).
- Size of the test photo library and camera bodies (sizing, equivalence
  bench, performance).
- An existing photo library with darktable XMP / `library.db` to use as a
  corpus?
- darktable or vkdt as the engine base, and the format of the module
  description.
- The next normalized layers after the catalog SDK.
