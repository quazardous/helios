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
- **Envelope**: sequence number, time, source (layer, module, node, job,
  image), the id of the command that caused it, the event itself.
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

The journal lives in the vault. It comes with the scheduler: an in-memory
journal first, with a contract test suite every storage must pass (grampy's
driver contract is the model), then a vault-backed one.

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
