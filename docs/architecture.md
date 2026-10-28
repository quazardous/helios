# Internal architecture: how parts communicate (draft)

Status: **draft, not decided** (2026-10-11). How the parts of Helios talk to
each other: which channel carries what. See ROADMAP sections 2.2 and 2.8.

## The darktable baseline

darktable's parts communicate through six channels, with no rule on which to
use when (measured on the fork, 2026-10-11):

| channel | what it is | size |
|---|---|---|
| control signals (`DT_SIGNAL_*`) | 51 global signals on one GObject; parameters untyped (GValue varargs), delivered on the GTK thread, no record of who emitted or why | 193 raises, 132 handlers, 71 files |
| view proxies (`darktable.view_manager->proxy`) | tables of function pointers through which views and libs call each other directly | about ten proxies |
| the global `darktable` struct | every part reaches every subsystem | `darktable.develop` 1196 uses, `.gui` 734, `.db` 647 |
| configuration as shared state | `darktablerc` keys used as global variables between components | 2070 `dt_conf_get/set` calls |
| the database as a channel | `memory.collected_images`, `selected_images` read by the whole GUI | see the fork's SQL inventory |
| direct coupling in the pixelpipe | a module looks up another one (`dt_iop_get_module*`), raster masks, writes to shared pipe state (`processed_maximum`) | ~40 sites |

Nothing says who owns a piece of state, so anyone writes anywhere: that is
how a background load silently reverted a rating
([darktable#22569](https://github.com/darktable-org/darktable/issues/22569)).

## Four kinds of communication, each with its channel

| kind | example | channel |
|---|---|---|
| **data** | pixels, histograms, masks, a parameter driven by an analysis | the edges of the graph; buffers stay where they are computed |
| **commands** | set a rating, run a network, cancel a job | addressed to the owner of the state, answered (accepted or refused) |
| **events** | rating changed, job finished, buffer ready, decision node resolved | the central bus (orchestration) |
| **queries** | state of a node, a page of a collection | `describe()`, vault cursors; pure reads |

### Data: not on the bus

Image data is large and its place matters more than its transport. A buffer
stays where it was computed and only moves when a consumer elsewhere needs
it: vkdt keeps the whole graph's buffers in GPU memory and only downloads
for display or output; a render on the node holding the raw ships only the
result (ROADMAP 2.2). The graph and the scheduler own data movement:
device memory, shared memory, tiles, network transfer verified by content
hash. The bus never carries a payload; at most a reference (buffer handle,
content hash) to it.

### GPU residency

(2026-10-11) An engine with an accelerator (a `Gpu` from `helios-gpu`)
keeps the result of each step it runs on the GPU in a GPU buffer (a
`DeviceImage`: region, channels, and a handle only its device reads). The
next GPU step reads that buffer directly. Images cross only where the other
side needs them: a host image (a source, a CPU step's result) is uploaded
when a GPU step reads it, and a GPU image is read back when a CPU step or
the caller of `Engine::render` reads it. A chain of GPU steps therefore
uploads its source once and reads back its result once; a CPU-only module
in the middle costs one readback and one upload more. The engine counts
both (`Counters::uploads`, `Counters::readbacks`), so tests assert them.

GPU results are cached like CPU ones, under the same content key (which
already names the accelerator) and within the same memory budget: their
bytes count, and the least recently used are evicted first, which frees
the GPU buffer once nothing else holds it. A read back copy is kept with
the GPU entry and counted too, so asking the same render again reads back
nothing. `Engine::render_device` answers the result in GPU memory without
reading it back (uploading it if the last step ran on the CPU), for a
viewport sharing the engine's device; `Gpu::buffer` gives the `wgpu`
buffer behind it (tightly packed `f32` pixels). It is shared with the
cache: read it, never write it.

### The viewport on the GPU

(2026-10-27) The desktop's viewport shows the engine's GPU results
without a copy. The window draws with the process's one device (see
below), and once its GPU path is attached (`Shell::attach_gpu`, the
first frame) the viewport's engine takes that device's `Gpu` as its
accelerator (`Engine::attach_accelerator`: the engine exists before the
window does). Its tile source (`helios_gui::engine_source`) then renders
each union of neighbouring tiles with `Engine::render_device_at` and
hands every tile as a view into the union's buffer: an egui-pop device
tile (`egui_pop::viewport::device::DeviceTile`), which the viewport's
shader reads in place, running the display transform there. A chain of
GPU steps shown on screen is thus read back zero times and uploaded
again zero times; only the source's pixels, which live on the host, go
up once a render. The counters say so: the engine's `Counters::uploads`
and `Counters::readbacks`, and the viewport's `ViewportState::transfers`
(device tiles are `bound`, no byte copied). The test
`crates/helios-gui/tests/zero_copy.rs` asserts them, and compares the
picture with the host path's.

Each tile keeps its union's `DeviceImage` (`DeviceTile::keep`) while the
viewport holds it, so the GPU's memory ledger counts what is on screen.
The host path stays: before the GPU path is installed, after a device
rebuild (the window keeps the device it opened with), on an adapter whose
fragment shaders cannot read storage buffers (WebGL 2), for a result the
viewport cannot bind (more than four channels), and for a tile the
viewport refuses when it is delivered (the device tile's fallback renders
it on the host, on the render thread). A tile refused later, when the
interface takes it in, is asked for again.
The mask display stays on the device too: the mask's node is rendered
there over the same union and scale, and a WGSL kernel
(`mask_display.wgsl`, run by `Gpu::composite`, a pixel-wise kernel over
two images of one region) lays it over the image into a new RGBA buffer,
which the tiles view. The host composite (`mask_display::composite`)
stays the reference: the kernel is checked against it within
`mask_display::GPU_TOLERANCE` (2e-4 relative), and it runs wherever the
host path does, or when the mask cannot be had on the device.

### GPU failures

(2026-10-12) A failing GPU never fails a render, and never gives wrong
pixels: the CPU code is the reference a fallback runs. Accelerator
failures are typed (`Failure`), and the engine answers each kind:

- a kernel that does not compile or run: that step is computed on the
  CPU, the rest of the chain stays on the GPU (`engine.gpu_fell_back` on
  the bus, `Counters::fallbacks`);
- an allocation over the device's memory budget: the engine evicts device
  content no render needs, least recently used first, and tries again;
  with nothing left to evict, the step runs on the CPU. The budget is one
  per device (`helios_gpu::default_budget`, from the device's binding
  limit and the machine's memory, or the device's `vram_mb` setting);
  every buffer the device holds is counted, transfers included;
- a wait past its deadline (no wait is unbounded) or a lost device:
  nothing on the device can be read back. The engine drops its device
  content, asks the accelerator to rebuild its device and replays the
  render; if the rebuild fails, or the rebuilt device fails too, the
  engine renders on the CPU for the rest of its life
  (`engine.gpu_lost`, with a message for the user; `Engine::gpu_off`).

Both events carry their `message` by key (`engine-gpu-fell-back`,
`engine-gpu-lost`, `helios_core::Message`); the desktop shows them as
toasts in its language, a fallback once per module.

`helios_gpu::Gpu::inject` provokes each failure on a real device, so the
tests cover them all without a broken driver.

The process has one wgpu instance and one device (`helios_gpu::shared`),
which the GUI's window draws with as well (see
[testing strategy](testing-strategy.md) for the Vulkan loader bug that
makes a second one dangerous). After a rebuild the engine computes on the
new device, but the window still holds the lost one.

### Commands: to the owner

Every piece of state has one owner (the vault for catalog data, the edit for
its parameters, the scheduler for jobs). A command goes to that owner, which
validates it, applies it field by field and records it in the audit trail
(design rules 1 and 4). The caller gets an answer. Commands are not
broadcast: a broadcast "please do X" leaves nobody responsible for X.

### Events: the central bus

**The bus serves orchestration, not data transfer** (2026-10-11, decided).
It carries the signals that coordinate the parts: state changed, job ready,
started, finished or cancelled, data available at a given handle, decision
node waiting for an answer. Messages are small and fast, with many
subscribers and publishers that do not know who listens. An event is a fact
in the past tense, emitted by the owner after a change.

- **Typed.** The core defines the envelope; each layer and domain defines
  its own event types (vault, scheduler, graph, photo...), instead of one
  global list that grows forever.
- **Envelope**: sequence number (`seq`), time, source (layer, module,
  node, job, image), the sequence number of the event that caused it
  (`cause`), the event's kind and its payload. The bus stamps the time
  when it publishes, in two forms that are plain integers on the wire:
  `mono_us`, microseconds since that bus was created on a monotonic clock
  (it follows `seq` and measures latency, `Bus::age`, but means nothing
  outside its bus), and `wall_ms`, Unix time in milliseconds (for logs
  and for relays between processes; the wall clock can jump, so it orders
  nothing). Readers that do not know the two fields ignore them.
- **Recorded.** The bus keeps a recent journal, readable through the
  inspect endpoint (`events --since <seq>`); tests wait on events instead
  of sleeping (see [testing strategy](testing-strategy.md)).
- **Network-capable.** Events are serializable; delivery is in memory inside
  a process and relayed between nodes, with resume from a sequence number
  and a resync signal after a disconnection or a restart, so a client never
  guesses whether it missed something.
- **Not a command channel.** Handlers react to facts (refresh a view,
  invalidate a cache, start a job through the scheduler); they do not take
  orders from the bus.

#### Which bus carries what

A node (`helios-node`) runs one bus for everything in its process: the
engine, the scheduler and its jobs, the vault, decisions and its own
`node.*` events, served by its inspect endpoint and read through it by
the MCP server, scripts and the desktop's relays. The AI service keeps a
bus of its own for its jobs (`ai.jobs.events`).

The desktop (`helios-gui`) runs three separate buses, as built today
(2026-10-14). A new service goes on the bus of the part it reports to;
events never cross from one desktop bus to another.

| Bus | Created by | Published on it | Read by |
|-----|------------|-----------------|---------|
| Shell bus | `Shell::builder` | egui-pop's events through `HeliosHost` (`gui.command.*`, `gui.dialog.answered`, `gui.toast.shown`, `gui.edit.*`, and any other name as `gui.event` with `name` and `data`: egui-pop's context stack's `context.*` once Helios's egui-pop pin includes it, the parameter picker); `job.*` from `helios_core::jobs::Jobs` (`Shell::jobs`: an import, an export, the GPU calibration); the jobs relay of each connected node (source `relay`: the node's `job.*` with ids `node:<name>/<id>`, `node.link`, `node.jobs_forgotten`) | egui-pop, which reads it back as its journal (`since`, `last_seq`): the jobs centre and its consoles, `Forgotten`, every plugin reading `events_since`; the host's waker thread, which asks for a frame on an event not of kind `gui.*` |
| Vault bus | `main.rs`, handed to `Persist` when a vault is open | the vault (`vault.field_changed`, `vault.stack_changed`, `vault.version_added`, `edit.saved`); the stack commands (`library.decision`) | `Persist`, each frame: it refreshes the library's fields and stacks |
| Viewport engine bus | `Viewport::default` | the viewport's engine (`engine.render_requested`, `engine.step_computed`, `engine.gpu_fell_back`, `engine.gpu_lost`, `engine.cache_settled`) | the shell's `engine_notices`, which turns a GPU fallback or loss into a toast |

So: something the interface or the jobs centre must see (a job, a
context change, a link going up or down) goes on the shell bus, through
`Shell::jobs` or egui-pop's `AppHandle::publish`; a change of catalog
data goes through the vault, on the vault bus; what the image's engine
says stays on the viewport engine's bus. A node's events reach the
desktop only through a relay, which republishes them on the shell bus
as new envelopes (new `seq`, `mono_us` and `wall_ms`).

### Queries: pure reads

Reading state never changes it (design rule 6). Every entity answers
`describe()`; collections are read through vault cursors. A view that needs
fresh data subscribes to the matching event and queries again.

## Data access: a framework prerogative

(2026-10-11, decided) Modules and services never access data themselves:
not files, not the catalog, not caches, not device memory. They declare what
they need; the framework fetches, places, caches and writes. This is the
only way to guarantee that **no read or write is ever wasted**, because only
the framework sees all the requests at once.

darktable shows the cost of the opposite: drawing a thumbnail could decode a
full raw and write the image record back; reading a thumbnail could upgrade
the history in the database; each part fetches what it needs when it needs
it, one image at a time.

### Reads

- **Declared needs.** A module declares its ports, regions and context
  fields; a view declares the fields it shows. The framework fetches exactly
  that: projection of fields, the region of the image (tiles), and the
  smallest representation that is good enough (embedded preview, mip level,
  proxy, or the full raw only when required).
- **Batched.** Requests are grouped: one call fills a page of thumbnails
  with all their fields, never one query per image (no N+1).
- **Never loaded twice.** The case to avoid above all is the same photo
  data loaded twice in RAM or VRAM. Every buffer (decoded raw, intermediate
  result, preview) is addressed by content hash and parameters, held once
  per memory (one copy in RAM, one per GPU) and shared read-only by
  reference between every consumer. Caches are owned and evictable by the
  framework.
- **In-flight requests are merged.** If a second job asks for content that
  is already being loaded or computed, it waits for that load instead of
  starting another one (single flight). Two views of the same image, a
  virtual copy and its original, an export and the darkroom on the same raw
  all share one decode; networks with the same first nodes and parameters
  share that common prefix.
- **Local.** Computation goes to the data when that is cheaper (ROADMAP
  2.2); buffers stay in place (GPU memory, shared memory) and are handed
  over as references, zero copy when possible.
- **Prefetch from declared intent, not guesses.** Prefetching is a job of
  the scheduler, driven by what a cursor or a network will need next
  (scroll direction, the next images of a batch), named and cancellable,
  never a hidden speculative load.

### Cursors

Collections are read through cursors (designed in the fork's phase 0): an
ordered snapshot, its count, a window fetched with a projection, the rank
of an image, a version with deltas of insertions and deletions, several
cursors at once, resolved server-side so a client never pulls a million ids.
Scrolling a 100,000-image collection costs one window fetch per screen.

### Writes

- **Field-level, single owner** (design rule 1): a write names the fields it
  changes, and each field has one owner.
- **Skipped when unchanged.** Writing a value equal to the stored one does
  nothing: no write, no event, no sidecar rewrite.
- **Coalesced.** Successive writes to the same fields (a slider being
  dragged) are merged before reaching storage; a batch of writes is one
  unit of work (one transaction, one event batch).
- **Never on a read path** (design rule 6).
- **Derived outputs written once.** Sidecars (XMP), thumbnails and exports
  are generated by the framework from the stored state when it changed,
  not by every component that touched the image.

### Locks: possible, but exceptional

Owning every access also lets the framework coordinate concurrent access.
The coordination that matters most is the one above: an entry per content
being loaded, so identical data is never loaded twice into RAM or VRAM. It
is internal to the framework and needs no lock visible to anyone.

Explicit locks are the exception: inside the graph, data is immutable and
addressed by content, and concurrent edits coexist as versions (ROADMAP
2.3). They are mainly needed at the **leaves of the network**, sinks that
write outside the framework's store (an export to JPEG files, a sidecar, an
upload): two exports to the same file must not race. Resource limits
(concurrent raw decodes, GPU memory, AI calls) are handled by the scheduler
the same way. A module never takes a lock itself; every lock is a lease with
an owner and an expiry, visible in `describe()` and on the bus.

### Budgets: efficiency as a test

The framework counts every read and write, by command and by job (it owns
them all, so counting is free). Tests assert budgets: opening a collection
is one cursor query plus one window fetch; scrolling a screen is one window
fetch; showing a thumbnail never decodes a raw when a preview exists; two jobs on
the same raw decode it once;
changing a rating is one field write and one event. A budget that breaks is
a failing test with the list of operations that exceeded it, which tells an
agent exactly what regressed.

## Two levels of execution

(2026-10-11) **Render level**: one network evaluated over a region. Pure,
content-addressed, shared results, single flight: no queue is needed, and
the engine reports each node's status (idle, stale, running, done, failed,
with its load) by comparing the planned content keys with what it holds.

**Workflow level**: one network applied to many items over time (412
images of a wedding, 3,200 to cull, a roll of 36, a batch export). Items are
often images but not always. This level needs work queues, and follows the
model of [grampy](https://github.com/quazardous/grampy), a workflow graph
over work queues kept in a storage:

- a **node journal** per (item, node): running, done, skipped, failed,
  omitted; workers claim with a lease and conclude;
- **joins** as data: all parents, k of n, a step that runs on a failure;
- **exclusive choices**: a decision node that picks a branch marks what only
  the other branches lead to as omitted;
- **retries** with a declared backoff, **leases** given back when a worker
  dies, **waits** settled by a durable signal (a human answering an
  AI-assisted decision node);
- **concurrency and rate limits** per node (GPU memory, AI calls, an export
  destination);
- **groups**: items worked together (a burst, a roll);
- **lanes**: an item that comes back while waiting merges with the waiting
  version and runs once (a slider dragged over a batch);
- items **pinned to the graph version** they started on, migrated all or
  nothing (engine version drift, ROADMAP 2.3);
- **monitoring** per node: claimable now, oldest waiting, longest running,
  retries due; this feeds the queue panel of the GUI.

The journal lives in the vault. The `helios-journal` crate has the first
part (2026-10-11): rows, claims with leases and tokens, joins (all or k of
n), exclusive choices with omission, retries with a growing delay, lease
expiry and per-node monitoring, over an in-memory store, and a contract
every store must pass (grampy's driver contract is the model). Groups,
lanes, waits and signals, rate limits and graph versions come next. A
vault-backed store followed (`helios_vault::journal::VaultJournal`). How
node processes run batches through it (runs, workers, control verbs,
monitoring, restart) is proposed in [workflow-level.md](workflow-level.md)
(2026-10-22, not decided).

## Configuration

(2026-10-11) One settings system for the whole framework, in the
`helios-config` crate.

- **Declared once**, after tvty's settings schema: each setting has a name,
  a label, a description, a kind (number with bounds and unit, toggle with
  its two words, choice) with its default, and a **concern**. Options pages,
  get and set by key, and the check of a hand-edited file come from the
  declarations.
- **Pages come from the naming structure**: a setting is named
  `page.group.name` (`interface.graph.zoom`), and that name alone places it
  on the options page `interface`, in the group `graph`; in a file it is
  `[interface.graph]` then `zoom = ...`. Nobody lays out a settings page by
  hand.
- **Normalized inputs**: a setting describes its input in the same shape as
  a module parameter (`type` float, int, bool or choice, bounds, default,
  choices), so the GUI draws settings and module parameters with the same
  widgets.
- **Layered**, after aiball's "russian doll": shipped defaults, then
  `global` (this user on this machine), `project` (shared by everyone on the
  project), `user` (follows the user on every machine), `run` (one
  invocation: command line, environment). The concern decides which layers
  may set a value: machine settings only in global and run, project settings
  up to project, user taste up to user. A value set in a layer its concern
  does not allow is refused.
- **Origin**: every effective value is reported with the layer it comes
  from (`helios inspect describe config`). A value equal to what the layers
  below give leaves its layer, so it keeps following them and defaults can
  evolve.
- **Files are TOML**, loaded all or nothing: a file with a problem changes
  nothing and every problem is reported at once.
- **Settings are inputs**: the program's edge says which files form which
  layer; components receive resolved values and never read a configuration
  file behind the caller's back. The user and project layers move into the
  vault later, so every node of a user sees the same ones.

## What replaces each darktable channel

| darktable | Helios |
|---|---|
| control signals | typed events on the bus |
| view proxies, direct calls between parts | commands to the owner, or events |
| global `darktable` struct | each part receives the services it uses, explicitly |
| configuration as shared state | settings are inputs, declared and passed in |
| database read by everyone | vault queries and cursors, change events |
| module-to-module coupling in the pipe | declared ports and graph edges |

## Open questions

- Delivery: synchronous handlers, an async runtime, or both (in-process
  fast path, queued path for slow handlers).
- Backpressure: what happens when a subscriber is slow (drop and resync,
  coalesce, or block).
- Journal size and retention, and whether part of it becomes the audit
  trail or stays separate.
- Transport between nodes (the protocol layer of phase 2).
