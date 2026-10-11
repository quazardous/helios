# Helios roadmap

Vision, settled concepts, phases and open questions for Helios. This is a
living document: decisions are dated, and an idea stays "not decided" until
it says otherwise.

Last updated: 2026-10-11.

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
Hard requirement (2026-10-11, decided): GPU acceleration on Linux, Windows
and macOS, as close to Vulkan as possible, so the viewport can show buffers
the engine computed on the GPU without copying them back. wgpu is
the shared GPU layer for the GUI and the engine's GPU modules (2026-10-11,
decided with egui: Vulkan on Linux and Windows, Metal on macOS, raw Vulkan
access for vkdt interop), at the version egui uses (wgpu 30 with egui 0.36).

**Helios makes darktable and vkdt meet** (2026-10-11, decided as the
long-term goal). darktable brings compatibility (edits, workflow, users) and
CPU computation; vkdt brings the graph and GPU speed. The meeting point is
the module contract: one description able to host a module coming from
either side, so both lineages run in the same graph. How (porting, bridging,
contributing upstream to one or both) is not decided. No vkdt fork until a
change to vkdt itself is needed (proposed 2026-10-11, not decided).

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
  Near-duplicates (RAW+JPEG pair, burst, re-encode) have different hashes and
  are relations, also data.

**Sync model.** (2026-10-10) Take git's *model* rather than git itself:
content-addressed objects, a DAG of versions, refs, offline replicas,
integrity. Raws are large immutable blobs stored git-annex style (hash
pointer, separate object store), never in the history. Edits are small text
(named parameters) and fit a version DAG; ratings, labels and tags merge best
per field (CRDT-like), which requires field-level writes (section 2.8,
rule 1). Not decided.

(2026-10-11) **Photos are not code.** No semantic merge: two concurrent edits
simply coexist as two versions, and the user keeps one or both.

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
  [docs/module-contract.md](docs/module-contract.md)).
- A workshop: the AI translates an artistic intention ("too contrasty with the
  blue sky") into a mini workspace: the real darktable modules, pre-calibrated
  for the photo, with only the useful sliders (restricted ranges, tested at
  the extremes), plus composite semantic sliders.
- Workshops are savable and reusable: they store the logic (relative targets,
  zone detection), not absolute values.
- Hypothesis to validate before building: mockup, a few photographers,
  measured time saved. "AI that learns your style" is already commoditized.

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

- [ ] **Step A: no SQL outside `common/`**, one domain = one potential
      upstream PR, checked by an equivalence bench (SQL dump + XMP, plus GUI
      captures) and a SQL inventory whose counter must go down.
  - [x] 1. ratings and color labels (c442a88a8, upstream PR darktable-org/darktable#22572)
  - [x] 2. metadata (e14874d30, 2026-10-11)
  - [x] 3. tags (5d5763e27, 2026-10-11)
  - [ ] 4. film rolls and images
  - [ ] 5. locations, overlays
  - [ ] 6. styles
  - [ ] 7. presets
  - [ ] 8. history and masks (critical, reinforced tests)
  - [ ] 9. selection and collection
  - [ ] 10. the `darktable-mcp` bridge
  - [ ] SQL inventory check added to CI once step A is done
  - [ ] make the Lua bench deterministic (see the fork's
        `doc/helios/PHASE0.md`, known limits)
  - [ ] performance bench: open and scroll a collection of 100,000 images
  - [ ] a GUI bench for each domain whose code path the Lua scenario misses
  - [ ] report the "Lua as a library" race upstream
  - [ ] contact upstream (GitHub Discussions / pixls.us) with the first PR,
        disclosing AI assistance
- [ ] **Step B: SDK interface**: `common/` functions grouped behind
      `catalog_*.h`, SQL moved into the sqlite backend, transactions become
      `dt_catalog_batch_*`, the image cache fed through the SDK.
- [ ] **Step C: cursors, targets, query model**: cursors (ordered snapshot,
      count, fetch with projection, rank, version + delta, several at once)
      replacing `memory.collected_images`; a typed query model replacing the
      dynamic SQL of collections, producing the same SQL as today.

Upstream framing: phase 0 is only acceptable upstream as a cleanup (less SQL
in the GUI), never as a step toward client/server.

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
| AI-first, intent -> workshop | a hypothesis to validate |
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
