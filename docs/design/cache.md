# One cache service

Status: slices 1 to 3 **accepted** (2026-11-11), with the defaults of
the open questions; the later slices stay proposed. Slice 1 is built
(`crates/helios-cache`). The inventory describes the code on `next` as of
that date.

ROADMAP 2026-10-11 decided that data access is a framework prerogative:
"identical photo data is never loaded twice in RAM or VRAM: buffers are
addressed by content, held once per memory, shared by reference, and
concurrent requests for the same content are merged", and that caches are
owned and evictable by the framework (ROADMAP, "Placement and caching";
[architecture.md](../architecture.md), "Data access"). ROADMAP 2026-10-12
added a memory ceiling by default for the render cache.

What exists instead is a dozen caches, each built where it was needed,
each with its own key, budget, eviction and setting, some with none. The
render engine's cache does what the decision asks, but only inside one
engine, and the desktop runs two engines. This note lists them all,
proposes one service that owns every tier, and orders the work to move
them into it without breaking anything.

## 1. Inventory

### The table

Tier: where the bytes are. Key: what an entry is found by. Budget: the
limit and its default. "None" means nothing bounds it.

| # | Cache | Tier | Key | Budget (default) | Eviction | Setting | Filled by / read by | Invalidation | Metrics | Tests |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | Render cache (`helios_core::engine::Cache`) | RAM (host `Content`), plus VRAM for device entries | content key of a step: hash of lineage (module, parameters, context, scale, inputs' lineages) and region; a wider region of the same lineage is cropped | one per engine: `Engine::with_budget`, 25 % of physical memory (`DEFAULT_CACHE_SHARE`, 2 GiB when unknown) | LRU over ready, unpinned entries after each render; a render pins its steps until their readers ran; device entries evicted first on a VRAM allocation failure (`evict_device`) | `system.cache.budget`, `system.cache.share` (helios-config, Caches page): **read by `helios-node` only**; the desktop's viewport hard-codes the default share; `compute.budget cache_mb` per device (helios-compute settings file) for the compute manager's engines | every render: viewport tiles, preview (scopes, navigator), head renders (Compare), mask display, graph panel statuses; export has **its own engine** | none needed (content keys); dropped on device loss | `Counters` (requests, computed, evictions, crops, uploads, readbacks...), `engine.cache_settled` on the bus (bytes, budget, entries, evicted) | `helios-core/tests/cache.rs`, `engine.rs`, `helios-gui/tests/zero_copy.rs` |
| 2 | Exporter's engine (`helios_gui::export::Exporter`) | RAM | same as 1 | **none** (`Engine::new`) | none: grows until the exporter is dropped | none | export jobs | none | its own bus, not shown | `helios-gui/tests/export.rs` |
| 3 | GPU device ledger (`helios_gpu::Ledger`) and engine device entries | VRAM | the render cache's key (entries), plus transfers | `helios_gpu::default_budget`: 4 × the largest binding, at most a quarter of RAM; `vram_mb` per device in helios-compute settings (applied by the compute manager only) | the engine evicts LRU device content no render pins, retries, else runs the step on the CPU | `compute.budget vram_mb` | engine (accelerated steps), viewport device tiles (`DeviceTile::keep` holds a union's buffer while shown) | device loss drops all | `Gpu::memory()` (used, budget), `Counters::fallbacks` | `helios-gpu` tests, `zero_copy.rs` |
| 4 | Compiled GPU pipelines (`helios_gpu::Gpu::pipelines`) | VRAM / driver | (kernel, entry point) | none (small, bounded by the kernel count) | never | none | kernels | device rebuild | none | indirect |
| 5 | Health check's kernel results (`helios_health::gpu`) | RAM | adapter key | one entry | replaced | none | Health page | another adapter | none | `helios-health` unit test |
| 6 | Viewport tile cache (egui-pop `viewport::tiles`) | VRAM (textures) or a bound device tile | tile key (side, scale, position, generation) | `tiles::DEFAULT_BUDGET` 256 MiB, upload budget 8 MiB a frame | least recently drawn, never one drawn this frame | none in Helios | the viewport, from `engine_source` | stale on any change of the graph shown | `ViewportState::transfers`, `Stats` | egui-pop's, `zero_copy.rs` |
| 7 | Preview render (`helios_gui::preview`, 256 px) | RAM (an `Image` from the render cache, kept by the preview) | the graph shown | one image | replaced on each graph change | none | preview thread / scopes (histogram), navigator | graph change | none | `history_preview.rs` |
| 8 | Thumbnail disk cache (`helios_thumbs::ThumbCache`) | disk: `.helios/thumbs` beside the catalogue | `Key { image: BLAKE3 of the file, version ("preview" or a version id), chain }` × size (256, 1024); provisional `p:<size>:<partial hash>` renamed later | `system.cache.thumbnails`, 2 GiB | LRU by file modification time, down to 90 % of the limit, on write | `system.cache.thumbnails` (Caches page) | the pool (`Loader`), the import window's cache reader, the image map re-check (`thumb_check`) / Library, filmstrip, Compare, loupe first frame, import window | `thumb_check` re-checks sightings (stat, partial hash, full hash) and makes a new entry when content changed; the old entry left to LRU | `usage()` (bytes, count) | `helios-thumbs/tests/thumbs.rs`, `times.rs`; `thumb_budget.rs` |
| 9 | Thumbnail textures (`helios_gui::thumb_textures::Resident`) | VRAM (egui textures) | image id | `system.cache.textures`, 512 MiB | drawn longest ago, never a cell in view; out-of-view arrivals over budget not uploaded | `system.cache.textures` | the pool / grid, filmstrip, Compare | forget on a new thumbnail | bytes, count (inside) | `thumb_budget.rs` |
| 10 | Large thumbnails (`Library::large_textures`) | VRAM | image id | **none** (freed when a render lands) | explicit | none | pool `Loader::large` / Compare | render lands | none | `thumbnails.rs` |
| 11 | Loupe stand-in (`Viewport::stand_in`) | VRAM | the image opened | one texture | replaced when the render is complete | none | `Shell::open_from_library` / viewport | render complete | none | `thumbnails.rs` |
| 12 | Head renders (`Library::Heads`) | VRAM (texture) + the render's `Image` in RAM via the render cache | image id + version + size | **none** | stale entries replaced; never evicted | none | `head_render::spawn` on the viewport's engine / Compare | generation per image (edit or network change) | none | `compare_heads.rs` |
| 13 | Import window pictures and textures (`import_browser`) | RAM (`ColorImage`) then VRAM | file path | **none** (a frame's upload count is capped) | removed when a path leaves the listing | none | scan glance (embedded preview), the library thumbnail cache / import grid | rescan | none | `import.rs`, `import_snapshots.rs` |
| 14 | Import scan memo (`import_scan`: `seen`, `by_size`) | RAM | path → (size, mtime); size → hashes | none, one scan | dropped with the scan | none | the scan | per scan | none | `import.rs` |
| 15 | Image map (sightings) | disk (the vault file) | path + size + mtime → content hash | `system.map.limit` | forgetting rules (`system.map.keepMissing`) | `system.map.*` | import scans, `thumb_check` / import window, thumbnail keys | re-check | rows | `helios-library` unit tests |
| 16 | Resolver memo (`helios_resolve::Cached`) | RAM, in `helios-node` | `Request::key` (pixels + brief) | **none** | never | none | AI and HTTP resolvers / decision nodes | none (same question, same answer) | none | `helios-resolve` tests |
| 17 | Compiled kits and themes (egui-pop `tokens::cache::Cache::for_app`) | disk: the platform cache directory under Helios's app id | source path + hash of contents + compiler version | none | stale entries replaced | none | egui-pop at start / theme loading | content or compiler change | none | egui-pop's |
| 18 | Bundled plugins (`helios_plugin::bundled`) | disk: `$XDG_CACHE_HOME/helios/plugins` | plugin name + version | none | rewritten on a new version | none | the desktop at start / plugin host | version change | none | `darktable_library.rs` (bundled importer) |
| 19 | AI runtime downloads (`helios_ai::store`) | disk: `$XDG_CACHE_HOME/helios/ai` | archive name | none | never | none | local model setup | none | none | `helios-ai` tests |
| 20 | darktable library copy (importer plugin) | disk: a temporary folder | none | one copy per scan | deleted when the scan ends | none | the importer | per scan | none | `darktable_library.rs` |

Not caches, but read like one: the vault itself (catalogue rows, read
straight from SQLite each time; no memo found in `helios-vault` or
`helios-vault-sqlite`); the thumbnail key's content identity, which is
the image's id. The node's jobs and the scheduler hold no pixel cache of
their own beyond the node's engine. Histograms and the mask display do
not cache: the histogram reads the preview (7), and the mask display
renders the mask's node through the render cache (1) and composites a
new buffer each time (on the GPU, `mask_display.wgsl`), held only by the
tiles that show it.

### Duplicates: the same pixels held twice

1. **One image on screen, up to six copies.** An image open in the loupe
   with Compare and the filmstrip showing it can be held as: its 256 px
   thumbnail texture (9), its 1024 px large texture (10) or the loupe's
   stand-in (11, the same 1024 px entry, decoded again when both ask for it), the render cache's
   preview image (7, 256 px, RAM), a head render (12, RAM through the
   render cache plus a texture), the viewport's tiles (6), and the
   render's own content in the render cache (1, RAM and VRAM). Some are
   different sizes of the same version and could be one entry read at
   several sizes (a mip); some are byte-identical (the large texture and
   the stand-in come from the same disk entry).
2. **One raw decoded by two engines.** Exporting the image open in the
   darkroom decodes its raw in the exporter's engine (2) as well as in the
   viewport's (1); neither sees the other, and the exporter's has no
   budget at all. Two engines of the same process break "held once per
   memory" and "concurrent requests merged".
3. **The embedded preview read twice.** The import window decodes a
   raw's embedded preview for its cell (13) while the library's pool may
   decode the same stream for the same file into the thumbnail cache (8);
   the import grid's cells are keyed by path, not content, so they cannot
   meet.
4. **Readbacks kept beside device entries.** By design (a device entry
   keeps its read-back copy, counted), but nothing lets the RAM budget
   drop the host copy while keeping the device one.
5. **Three render budgets.** `system.cache.budget` / `share` (read by the
   node only), helios-compute's per-device `cache_mb` (read by the compute
   manager only) and the viewport's hard-coded default share: the desktop
   shows a "Render cache" setting it does not apply. Likewise `vram_mb`
   reaches the compute manager's devices, not the desktop's.

### Gaps against the decision

| Decision | Today |
|---|---|
| Addressed by content | yes inside the render cache (1) and the thumbnail cache (8, by image id and version); by path in the import window (13); by image id only in the textures (9, 10, 12) |
| Held once per memory | per engine, not per process: two engines in the desktop; textures duplicate each other (above) |
| Shared by reference | inside one engine (`Arc<Content>`); between engines, never; across processes (node, plugins), never |
| Concurrent requests merged | single flight inside one engine (`OnceLock` cells); the thumbnail pool deduplicates its queue; nothing between the pool and the import reader, nor between engines |
| Memory ceiling by default | render cache yes (25 %); exporter none; head renders, large textures, import pictures, resolver memo none |
| Owned and evictable by the framework | each feature owns its cache; no single place can say what is held, or free it under pressure |
| Visible and clearable | four sizes on the Caches settings page, one of them not applied; no usage shown; no "Clear caches"; the XDG cache folders are invisible |

Other weaknesses found on the way: the render cache's eviction scans every
entry per victim (quadratic when a render frees many); the thumbnail
cache's eviction sorts every entry on each overflowing write; device
pressure and host pressure are one budget in the engine although they are
different memories.

## 2. Design

### What the service is

One cache service per process, `helios-cache` (a new crate under
`helios-core`'s dependencies so the engine can use it). It owns the bytes
of every tier, their accounting and their eviction. Features do not own
caches; they **register a kind** and ask the service for entries of that
kind.

```text
         kinds (registered)            scopes                tiers
   render.step  thumb.256  thumb.1024   shared (default)     VRAM per device
   decode.raw   texture.grid  ...       silo: library X      RAM
   resolve.answer  plugin.<id>.*        silo: plugin Y       disk (folder per scope)
                     \                     |                    /
                      +---- one index, one budget per tier ----+
                            per-kind shares, pins, priorities
                            single flight, statistics, events
```

### Entries

- **Key**: `(kind, content key)`. The content key is a BLAKE3 hash the
  kind computes from what defines the bytes, never from where they came
  from: a render step's lineage and region (as today), an image's id plus
  version plus chain plus size for a thumbnail, a request key for a
  resolver answer. A key built from a path and a time is not content: it
  is allowed only for kinds that say so (`identity: path`), which the
  statistics show, so the remaining ones are visible debt (the import
  window today).
- **Value**: a typed handle, shared by reference: `Arc<Content>` in RAM, a
  device buffer handle in VRAM (dropped when the last holder lets go), a
  file in the disk tier (read through the service, never by path). The
  service never copies a value to hand it out.
- **Derived entries**: an entry can name its source (`derived_from`), so a
  256 px thumbnail can be a view of the 1024 px one, or a device entry
  and its read-back copy can be one entry with two residencies. Eviction
  frees the residency, not the identity: a RAM copy can go while the VRAM
  one stays, and the reverse.

### Kinds

A kind is declared once, at registration:

```rust
CacheKind {
    id: "thumb.256",              // stable: settings, stats and folders use it
    tiers: &[Tier::Disk, Tier::Ram],
    identity: Identity::Content,  // or Identity::Path (debt, shown)
    cost: Cost::Decode,           // how dear a miss is: Cheap, Decode, Render, Network
    share: Share::Default,        // or a reserved minimum per tier
    persist: true,                // disk tier survives restarts
    version: 2,                   // bumped when the format changes: old entries are dropped
    scope: ScopeRule::Shared,     // or PerLibrary, PerUser, Private
}
```

Kinds expected from the inventory: `render.step` (1, 2), `decode.raw`
(the source step, split out so export and darkroom share it even when
their networks differ), `preview` (7), `thumb.256` and `thumb.1024` (8),
`texture.grid`, `texture.large`, `texture.head` (9, 10, 11, 12),
`import.glance` (13), `resolve.answer` (16), `kit.compiled` (17),
`plugin.bundle` (18), `ai.download` (19). The viewport's tiles (6) stay
in egui-pop, which is a library: Helios feeds it a budget from the
service and reads its usage (see "egui-pop" below).

### Scopes: shared pools and silos

One service does not mean one pool. Entries live in a **scope**; the
default scope is the process's shared pool, and a kind or a caller can
ask for its own:

- **Shared (default).** Content-addressed and shared by everyone in the
  process: what the decision asks. Render steps, decodes, previews and
  textures are shared: two consumers of the same content get the same
  entry.
- **Reserved share.** A kind in the shared pool with a guaranteed minimum
  per tier (`Share::Reserved { ram: 256 MiB }`): its entries cannot be
  evicted by other kinds below that floor, and it can grow above it like
  any other. For what must stay fast whatever else runs: the grid's
  textures in view, the loupe's render.
- **Silo.** A scope with its own budget and its own eviction, whose
  entries are never seen by other scopes even when keys are equal. The
  service still counts it, shows it, clears it and lets the global
  pressure policy shrink it (down to its floor). A silo is for:
  - **data that must not cross a boundary**: one library's thumbnails
    and maps (a client's library on a shared machine; the image map's
    privacy rule that its thumbnails stay with its vault); a user's data
    in a multi-user node;
  - **a third party's cache**: an out-of-process plugin gets a silo
    (`plugin.<id>`), so its bugs and its size cannot evict or read
    Helios's entries, and uninstalling it clears it;
  - **a guaranteed budget that must not be borrowed**: a node serving
    remote renders keeps a private silo for its jobs so the local
    desktop's browsing cannot starve them;
  - **stores whose eviction is not LRU**: downloads kept until an update
    (19), compiled files replaced on a new version (17, 18).

  The rule: shared by default; a silo only when isolation (privacy, a
  third party, a user) or a guaranteed, unborrowable budget is needed,
  and the reason is written in the kind's registration. A silo's keys
  are still content keys, so moving a kind between shared and silo is a
  configuration change, not a format change.

Scopes are hierarchical for accounting (`process / library:<id> /
thumb.256`), flat for lookup (a lookup names its scope).

### Tiers and budgets

- **One budget per tier**, per machine: RAM (default 25 % of physical
  memory, the current render default, 2 GiB when unknown), VRAM per
  device (`helios_gpu::default_budget`, as today), disk per scope folder
  (default 2 GiB for a library's thumbnails, as today; a total for the
  user cache folder).
- **Per-kind shares**: a kind may have a reserved floor and a ceiling as
  a share of its tier; by default neither, and kinds compete by cost.
  Today's settings map onto it: `system.cache.textures` becomes the
  ceiling of `texture.*` in VRAM; `system.cache.thumbnails` the disk
  budget of the library's thumbnail scope.
- **Silos** have their own budget, counted inside the tier's total: the
  tier's budget is the machine's promise, silos split it.
- The engine no longer has a budget of its own: it asks the service, so
  every engine of the process shares one RAM budget and one VRAM budget
  per device.

### Eviction: one policy

Cost-aware LRU (GreedyDual-Size style): each entry's priority is
`last use + cost of a miss / size`, where the miss cost is the kind's
declared class refined by the measured time to produce the entry (a
render step that took 800 ms outlives a 2 ms crop of the same size).
Eviction takes the lowest priority first, in an ordered structure (no
full scan per victim), until the tier is under 90 % of its budget, the
same hysteresis the thumbnail cache has.

Never evicted:

- **pinned** entries: a render's working set until its readers ran (the
  engine's current pins, moved into the service), and what is on screen
  (cells in view, tiles drawn this frame, the loupe's render);
- **in-flight** entries (being computed);
- a kind's **reserved floor**.

A pin is a guard object (dropped, it unpins), so a crashed render cannot
leak one. When nothing is evictable the request gets a "no room" answer
and the caller degrades as today (a step on the CPU, a texture not
uploaded, a thumbnail left out).

Pressure from outside the process (the operating system's low-memory
signal, a VRAM allocation failure) triggers the same eviction to a lower
target.

### Priorities and request merging

- **Single flight for every kind**, not only render steps: the first
  request for a key creates an in-flight cell; later requests wait on it
  (with the request's deadline: no wait is unbounded). This is the
  engine's `OnceLock` cell, generalised. Export and darkroom asking the
  same raw decode wait on one.
- **Priorities** belong to requests, not entries: `on screen`, `next`
  (prefetch from declared intent), `background`. The cache does not
  schedule work (the scheduler does); it tells the scheduler that a
  request merged into an in-flight one of lower priority, so the
  scheduler can raise it.
- **Cancellation**: a waiter can leave; the computation goes on while
  any waiter or pin remains, and is cancelled when none does.

### Persistence of the disk tier

- Each scope with a disk tier is a folder: the library's at
  `.helios/thumbs` beside the catalogue (kept: moving a library moves its
  thumbnails, and the privacy rule holds), the user's under the platform
  cache folder (`$XDG_CACHE_HOME/helios`, the platform's equivalent
  elsewhere), one subfolder per kind and version. Every cache folder gets
  a `CACHEDIR.TAG` (backup tools skip it; Helios's own image map already
  skips tagged folders).
- The index is rebuilt by listing the folder off the interface thread
  (as `ThumbCache::open_unlisted` does), last use from file times; no
  separate database to corrupt. A small journal of last uses may come
  later if listing a large folder is too slow at start (measured by
  `thumb-bench --start warm`).
- Writes go to a temporary name then a rename, so a crash leaves a whole
  entry or none. An entry that cannot be read is a miss, never an error.
- The disk tier is never synced and never backed up: the vault and its
  replicas hold durable data; caches are rebuilt (ROADMAP, "durable copies
  count for redundancy; caches are evictable and do not").

### Invalidation

Content keys make most invalidation unnecessary: an edit is a new
version, a new key; the old entry ages out. What remains:

- **identity changes**: a file whose content changed under the same
  path. `thumb_check` and the image map find it and tell the service
  (`forget(scope, kind, key)`) or simply ask a new key; path-keyed kinds
  are dropped on a sighting change.
- **format or code changes**: a kind's `version` (and, for renders, the
  module's version already in the lineage) makes old entries
  unreachable; the service deletes unreachable disk entries in the
  background.
- **device loss**: the VRAM tier of that device is dropped as a whole.
- **scope removal**: a library closed or forgotten, a plugin
  uninstalled: its silo is dropped (and its folder deleted when it is a
  user cache folder).
- **explicit**: "Clear caches" per kind, per scope or all; the image
  map's "Forget everything not in the Library" clears the matching
  thumbnails through the service.

### Statistics, events and collectors

- Per scope, kind and tier: entries, bytes, hits, misses, merged
  requests, evictions (by cause: budget, pressure, explicit), bytes
  written and read on disk, time spent producing misses. Counted
  atomically (the engine's `Counters` stay and read from it, so its
  tests keep asserting "rendering twice computes nothing").
- On the bus: `cache.settled` (replacing `engine.cache_settled`, with
  per-kind figures) after a burst of work, never per entry.
- An agent collector, `helios.cache`, with the same figures and the
  pinned entries, so agents and scenario tests can assert budgets
  ("scrolling 10,000 cells never exceeds the texture ceiling", "opening
  an image already exported decodes nothing"). A `cache.stats` and
  `cache.clear` command on the node, for headless runs and the bench.

### Settings: the Caches page

The existing `storage/caches` settings page becomes the service's page:

- **Sizes**: RAM budget (size or share of memory), VRAM per device (from
  the device list), disk per scope (this library's thumbnails, the user
  cache). Machine settings: they live in the machine's layer (`global`)
  and the compute settings file, never in a library or a synced profile,
  since another machine has other memory. `system.cache.budget` and
  `share` become the RAM tier's and **apply to the desktop**; the
  compute settings' `cache_mb` and `vram_mb` are the same values seen per
  device (one store, two views, not two settings).
- **Usage**: a bar per tier, split by kind, with the reserved floors and
  the silos; the figures live (the collector's).
- **Clear caches**: per kind and all, with what it costs ("thumbnails
  will be read again from the files: about N minutes"); clearing never
  touches pinned entries and never durable data.
- The Health check gets one line: the disk tier's folders have room, and
  the RAM budget is below what the machine can give.

### Processes: desktop, node, plugins

- **One service per process.** The desktop and a local `helios-node` are
  two processes with two RAM tiers; they share the **disk tier** (same
  folders; writes by rename, so two writers are safe, and each process's
  index sees the other's entries on a miss by checking the file) and,
  later, may share decoded buffers through shared memory (a RAM scope
  backed by a memory-mapped file), which is a later slice and only if a
  measurement shows the duplicate decode costs.
- **A remote node** keeps its own service; what crosses the network is a
  content key and, on a miss, the bytes (ROADMAP 2.2: the render runs
  where the raw is, the result travels). The local disk tier caches what
  came back under the same key.
- **Out-of-process plugins** never see the service's memory. They get a
  silo through the plugin protocol: `cache.get` / `cache.put` on their
  own scope, with a budget the host sets; large values are files in the
  silo's folder handed by path. A plugin's bundle folder (18) is a kind
  of that silo.

### The vault's role

The vault does not hold caches. It provides what the cache keys on and
what invalidates them: content identity (an image's id is its file's
hash; versions and chains are vault ids), the image map's sightings
(which say whether a file still is the content a key names), and events
(a version created, a sighting changed, a library forgotten) the service
listens to. Retention and reclaiming (ROADMAP, "Deleting becomes
reclaiming space") drop caches first: the reclaim job asks the service to
shrink its disk scopes before touching any durable copy.

### egui-pop

egui-pop's tile cache and textures are a library's internals and stay
there (egui-pop serves other apps). Helios gives them their budget from
the service's VRAM tier (`texture.tiles`, a ceiling) and reads their
usage for the statistics; no egui-pop change is needed beyond what it has
(a texture budget and stats). Its compiled-file cache (17) is pointed at
the service's user cache folder, so it is listed and cleared with the
rest.

## 3. Slices

Each slice keeps behaviour and existing tests, adds its own, and is
measured where speed is at stake (`thumb-bench`, the engine's
`stress_bench`).

1. **The service, the Caches page, and the thumbnail disk cache.** Create
   `helios-cache`: scopes, kinds, tiers (disk and RAM accounting first),
   one budget per tier, the cost-aware LRU with pins, single flight,
   statistics, the `helios.cache` collector, `cache.stats` /
   `cache.clear`. Move `ThumbCache` (8) onto it as the kinds `thumb.256`
   and `thumb.1024` in the library's scope (same folder, same file names,
   so existing caches stay valid), `CACHEDIR.TAG` added. Rework the
   Caches page: sizes, usage per kind, "Clear caches". Chosen first
   because it is the cache the user sees (start time, disk use), the only
   persistent one, already content-addressed and read by four features:
   it exercises the disk tier, scopes and clearing with the least risk to
   rendering. Risks: the listing at start (keep it off the interface
   thread; `thumbnails.rs` asserts startup does not list); eviction
   moving from "on write" to the service's policy (keep the 90 % target;
   `thumbs.rs` tests stay); `thumb_check`'s rename of provisional keys
   must stay atomic.
2. **The render cache in the RAM tier; one engine budget.** The engine
   takes its entries, pins and single flight from the service
   (`render.step`), keeping its `Counters` and `engine.cache_settled`
   (then `cache.settled`). The desktop applies `system.cache.budget` /
   `share` (today ignored). Risks: the engine's hot path (lock
   contention: shard the index by key); `helios-core/tests/cache.rs` and
   `engine.rs` must pass unchanged; `stress_bench` must not slow down.
3. **The exporter shares the process's cache.** Export renders through
   the shared `render.step` and a new `decode.raw` kind, so exporting
   the open image decodes nothing again and export has a budget. Risks:
   an export's working set evicting the darkroom's tiles (pin what is on
   screen; give export a priority below on-screen); a test that exports
   the open image and asserts `source_reads` unchanged.
4. **VRAM tier.** The device ledger reports to the service; device
   entries and their read-back copies become one entry with two
   residencies; `vram_mb` applies to the desktop's device. Risks:
   `zero_copy.rs` counts must hold; the fallback path on allocation
   failure.
5. **Textures as kinds.** `texture.grid` (9), `texture.large` (10),
   the loupe stand-in (11) and `texture.head` (12) under one VRAM
   ceiling (`system.cache.textures`), the large entry decoded once for
   Compare and the loupe; head renders evictable. Risks: the grid's
   "never a cell in view" rule becomes a pin; `thumb_budget.rs`,
   `compare_heads.rs` stay green.
6. **Import window on content keys.** The import grid's cells keyed by
   the image map's content identity when known, sharing `thumb.256`
   with the library; the glance becomes `import.glance` (path identity,
   shown as debt) until hashed. Risk: cells not hashed yet (keep the
   glance path).
7. **The small stores.** `resolve.answer` (16, bounded), `kit.compiled`
   (17), `plugin.bundle` (18), `ai.download` (19) registered in the user
   cache scope, so the page lists and clears them; the plugin protocol's
   silo (`cache.get` / `cache.put`). Risk: low; protocol change needs a
   plugin API version bump.
8. **Across processes (only if measured).** Shared-memory RAM scope
   between a local node and the desktop; the reclaim job shrinking disk
   scopes first. Risk: platform differences (Windows mapping), so only
   with a measurement that the duplicate decode matters.

## 4. Open questions

- Should the library's thumbnail scope stay beside the catalogue (moves
  with the library, privacy) or move to the user cache folder (no writes
  on a NAS library)? Default: beside the catalogue, as today, with a
  setting later if a NAS library makes it slow.
- Is `decode.raw` worth a kind of its own, or is the source step's
  `render.step` entry enough once export shares the engine's cache?
  Default: start with the shared `render.step` (slice 3) and split only
  if export and darkroom networks differ before the first node.
- A RAM budget per process or per machine when a local node runs beside
  the desktop? Default: per process, each taking its share, with the
  page showing both.
