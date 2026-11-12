# Node and client: what runs where

Status: **analysis, not decided** (2026-11-12). It describes the code on
`next` as of that date, classifies every subsystem, tests four hypotheses
and lists the follow-ups. The open choices are at the end; the ROADMAP
records a decision once the owner settles them.

The headless core of Helios is the **node**: the vault, the kernel (engine,
scheduler, jobs, the workflow journal) and the services around them. It can
run on the same machine as the windows or on a server. The **client** is
what draws: the desktop's windows today, a web page later, and an agent
through MCP. This note asks, subsystem by subsystem, what can live on a
node, local or remote, and what must not.

Related notes: [architecture.md](../architecture.md) (channels: data,
commands, events, queries), [cache.md](cache.md) (one cache service per
process), [windows.md](windows.md) (one window per focus, option C: one
process per library), [sync.md](sync.md) (replicas), [workflow-level.md](../workflow-level.md)
(runs on a node), [export.md](../export.md) ("Remote later"),
[import-connectors.md](import-connectors.md) (the connector trait).

## 0. Terms

- **Node**: the process (or library, see choice A) that owns durable state
  and runs work: `helios-node` today, the desktop's own in-process kernel
  today too.
- **Client**: a window, a web tab, an agent. It holds view state and the
  devices of the machine it runs on (screen, input, local disks and cards).
- **Local node**: on the client's machine. **Remote node**: elsewhere (a
  workstation reached from a laptop, a NAS, a server).
- **Resource agent**: a small service on the client's machine that serves
  what only that machine sees (a card, a folder) to a node, by stream.

## 1. Today in one picture

```text
 helios-gui process (one per library)            helios-node process (optional)
 ┌──────────────────────────────────────┐        ┌─────────────────────────────┐
 │ windows, egui-pop, input, display     │        │ engine + scheduler + jobs   │
 │ vault (SQLite, opened in process)     │  relay │ workflow runs (journal)     │
 │ engine (viewport, previews, export)   │◀──────▶│ vault (--vault, optional)   │
 │ jobs: import, export, backup, health  │  jobs  │ inspect + control endpoints │
 │ thumbnails pool + disk cache          │  only  │ (Unix socket, loopback TCP) │
 │ import window: cards, scans, glances  │        └──────────────▲──────────────┘
 │ values store, undo, workspace         │                       │ endpoints
 │ plugin broker, AI resolvers           │◀── agent socket ──┐   │
 └──────────────────────────────────────┘                     helios-mcp (stdio)
```

- **The desktop is its own node.** `helios-gui` opens the vault in process
  (`crates/helios-gui/src/main.rs`, `Persist` over `SqliteStore`), runs the
  engine the viewport draws from (`Shell::viewport().engine()`), and runs
  every job on its own scheduler (`Shell::jobs`): import, export, backups,
  the health check, GPU calibration.
- **`helios-node` is a headless node** (`crates/helios-node/src/main.rs`):
  a network from JSON, renders and exports, the workflow runs
  (`crates/helios-node/src/workflow/`), the inspect and control endpoints,
  settings layers kept in its vault with `--vault`. With `--vault` it runs
  the same `LocalNode` the desktop embeds (`crates/helios-node-core`,
  `crates/helios-node/src/vault_node.rs`): one owner of the vault, which
  the settings, the edit and the runs borrow, with the volume watch, the
  backups and the health check as its jobs, and a clean close on SIGTERM.
- **The link between them carries jobs only.** `--node NAME=INSPECT[,CONTROL]`
  starts a relay (`crates/helios-gui/src/relay.rs`) that mirrors a node's
  `job.*` events into the jobs centre and sends Cancel back. No catalogue
  query, thumbnail or render crosses it.
- **Transport is local only.** `helios-inspect` serves JSON lines on a Unix
  socket or on loopback TCP with a session token
  (`crates/helios-inspect/src/transport.rs`: "Only loopback addresses are
  accepted"). A remote node is reachable today only through a tunnel.
- **Remote compute units are probed, not used**
  (`crates/helios-compute/src/remote.rs`: no work is routed to a remote
  unit until the node renders on request).
- **Agents** reach a node through `helios-mcp`
  (`crates/helios-node/src/bin/helios-mcp.rs`), and the desktop through
  egui-pop's agent socket. The workspace, context, clipboard and history
  tools go to *the desktop holding the workspace*
  (`crates/helios-node/src/mcp.rs`, module docs): without a window, the
  history and clipboard tools are refused.

So the code already has the node boundary in two places (the relay, the
MCP routing), but the desktop is a complete node on its own, and its life
is the window's life.

## 2. Criteria

| Criterion | Question | Pulls toward |
|---|---|---|
| Data locality | Where are the bytes physically? A card or USB drive is seen only by the machine it is plugged into. | wherever the data is |
| Latency | Interactive (a slider, a brush, a cursor preview: under a frame, 16 ms) or batch (seconds to hours)? | interactive: client; batch: node |
| Bandwidth | What crosses the wire and at what size? A raw is 5 to 60 MB, a 1024 px preview 150 to 300 KB, a 256 px thumbnail 10 to 20 KB, a glance with sparse reads 4 to 35 % of the raw. | compute where the big bytes are |
| Hardware | Needs the GPU that draws, the colour-managed screen, input devices? | client |
| Secrets and trust | Which machine holds credentials? Is the node shared (several users, a server)? | the machine that does the work, never a shared node in clear |
| Survival | Must it go on when every window is closed (jobs, backups, watchers)? | node |
| Multi-client | Shared by several windows, clients or agents, or per window? | shared: node; per window: client |
| Offline | Must it work when the node is unreachable? | client (with a local node or a replica) |

## 3. Inventory and classification

Buckets: **node** only, **client** only, **either** (node by default, client
as an optimisation), **split** (a part on each side, with a protocol).

| # | Subsystem | Today: where, code | Touches | Deciding criteria | Bucket | Reason, protocol |
|---|---|---|---|---|---|---|
| 1 | Vault and catalogue | desktop process (`helios-gui/src/persist.rs`, `main.rs` opens and closes it); node with `--vault`; SQLite WAL, several processes may open it (`windows.md`) | the catalogue file, the vault bus | multi-client, survival | **node** | one owner per vault; clients query through cursors and subscribe to `vault.*` events (architecture.md, "Cursors"). Today two processes on one vault drift apart in memory. |
| 2 | Image map, volume registry, storage scan | desktop (`helios-library/src/map.rs`, `helios-storage/src/scan.rs`, `registry.rs`), poked on volume changes (`main.rs` `on_volumes_changed`) | the vault, mounted volumes | data locality, survival | **node**, with sightings from clients | the registry is catalogue data. Each machine's volumes are seen by that machine: a client reports its mounts (split with #16). |
| 3 | Engine, interactive render (viewport tiles, mask display, scopes' preview) | desktop (`engine_source.rs`, `preview.rs`, `mask_display.rs`), zero-copy GPU display (architecture.md, "The viewport on the GPU") | the raw's pixels, GPU memory | latency, hardware, bandwidth | **either** | the node by default when it is local (same process: zero copy); a client with the raw (local or fetched) and a GPU renders itself; a remote node streams tiles otherwise (choice F). |
| 4 | Engine, batch render (export, head renders, thumbnails of versions) | desktop (`export.rs` on the viewport's engine, `head_render.rs`); node (`ExportDesc`, workflow runs) | raws, output folders | survival, bandwidth | **node** | ROADMAP 2.2: render where the raw is, ship the result. |
| 5 | Previews and caches (render cache, preview, tiles, textures) | per process (`cache.md`, table rows 1, 6, 7, 9 to 12) | RAM, VRAM | latency, hardware | **either** (RAM/VRAM tiers on each side) | cache.md already says one service per process; a remote node's results land in the client's disk tier under the same content key. |
| 6 | Thumbnails: generation and disk cache | desktop: `helios-thumbs` (`ThumbCache` beside the catalogue, `Loader` pool), `helios-gui/src/thumbnails.rs` | raws' embedded previews, `.helios/thumbs` | multi-client, bandwidth | **node** (generation, keys, invalidation, pruning) | H2. The node answers `thumb.get(image, version, size)` with a path or bytes (H4). |
| 7 | Thumbnails: display | desktop (`thumb_textures.rs`, budget `system.cache.textures`) | VRAM | hardware | **client** | a texture is the client's; losing it costs a refetch. |
| 8 | Import: listing and glances | desktop import window (`import_scan.rs`, `import_browser.rs`, `GlanceCache` keyed by path, size, mtime) | the source's files, sparse reads (`helios-load/src/sparse.rs`) | data locality, latency | **split** | where the source is: a card on the client is listed and glanced there (resource agent); a folder the node sees is listed by the node. |
| 9 | Import: hash, copy, record, backup after import | desktop jobs (`helios-library/src/import.rs` `import`, `shell/import.rs`, `importers.rs`; `main.rs` `back_up`) | source files, library volumes, the vault | survival, multi-client | **split** (node stores; the bytes come from where they are) | the node runs the import job; for a client-only source it reads through the resource agent (`Connector::fetch`, ranged). |
| 10 | Connectors and sources (folders, cards, Google Photos, gphoto2) | desktop (`helios-connect`: `Access::InPlace(path)` or `Access::Fetch`; plugins via the broker) | local devices, the network, accounts | data locality, secrets | **split** by source: a card connector runs on the machine with the card; a cloud source (Google Photos) runs on the node | the `Connector` trait is already the protocol: `sources`, `list`, `thumbnail`, `fetch(offset)`. |
| 11 | Export (single and batch) | desktop: `Exporter` (`helios-gui/src/export.rs`), cancelled when the window closes (`impl Drop for Exporter`, line 210); node: workflow runs | the vault, raws, output folder | survival, data locality of the output | **node** (rendering, the run); output folder on the node's file system, or delivered to the client | export.md's request is already shaped as a workflow run. A destination only the client sees is a split: the node renders, the client writes. |
| 12 | Jobs and queue | desktop scheduler (`helios-core/src/jobs.rs`, `Shell::jobs`); node jobs relayed (`relay.rs`) | everything | survival, multi-client | **node** (state), **client** (the jobs centre, a view) | the relay is the protocol, already built: describe, subscribe from a seq, cancel, resync after a drop. |
| 13 | Workflow journal and runs | node (`helios-node/src/workflow/runs.rs`, `helios-journal`) | the vault | survival | **node** | built for it. |
| 14 | Workshops, MCP agents | `helios-mcp` over the node's endpoints; workspace, context, clipboard, history tools routed to the desktop holding the workspace (`mcp.rs`); `workshop.describe` in the desktop (`shell/workshop_design.rs`) | the workspace, the image's measures, the value store | multi-client, survival | **node** for what has meaning (workspace, history, design session); **client** for what the window shows | an agent should not need a window to undo its own step; today it does. |
| 15 | Settings | `helios-config` layers; the desktop reads files (`config.rs`, egui-pop Settings); the node keeps user and project layers in its vault (`--vault`) | config files, the vault | multi-client, offline | **split** by concern: machine (`global`) on each machine; `user` and `project` on the node; interface taste on the client | the concern already says which layer may hold a value (architecture.md, "Configuration"). |
| 16 | Removable volumes, cards, cameras | desktop: `helios_storage::Watch` in the import window (`shell/import_window.rs`), `system()` reads `/proc/mounts`, `/dev/disk`, `/sys` | the client machine's hardware | data locality, hardware | **either**: the machine where the device is plugged in | each machine runs a watcher; a client reports its volumes to the node (identity by `.helios/volume.json`, `helios-storage/src/marker.rs`). |
| 17 | Backups and scrubbing | desktop thread (`main.rs` `back_up`, `helios-library/src/backup.rs`), triggered after import or on a disk plugged in | volumes, the vault | survival | **node**; a client's disk as target is a split | ROADMAP: continuous background backup. A disk plugged into the client and not the node is reached through the resource agent (write direction), or is a node of its own. |
| 18 | Health check | desktop (`main.rs` `health_runner`, `helios-health`) | storage, the vault, GPU, plugins | survival, hardware | **split**: storage and vault checks on the node; GPU and display checks on each client | the GPU checked is the one that draws. |
| 19 | Accounts, OAuth tokens, keyring | `helios-connect` `StepKind::Oauth` (system browser, PKCE, loopback redirect), `Form` secrets to the keyring; `helios-ai/src/secrets.rs` (secret-tool, security); `helios-resolve` keys from the environment | the browser, the keyring | secrets and trust | **split**: consent in the client's browser; the token stored where the work runs (the node's secret store) | a loopback redirect lands on the machine of the browser, so the client relays the code to the node, or the device-code flow is used (`StepKind::Code`). |
| 20 | AI providers and resolvers | `helios-ai` service of the machine (in `helios-mcp`); resolvers in the node (`helios-resolve`, memo row 16 in cache.md) | the network, models, keys | secrets, hardware (local models) | **node** for decision nodes in runs; **either** for a local model on the client's GPU | providers are configured where they are called. |
| 21 | Plugins | broker started by the desktop (`main.rs`, `helios_plugin::Broker::new`), out of process, JSON-RPC | paths handed by the broker, staging folders | data locality, trust | **split** per manifest: ROADMAP 2.7 already says "front, server or both" | an importer of a client device runs on the client's machine; a service plugin runs beside the node. |
| 22 | Layout, workspace state, view state | desktop: egui-pop layout, `workspace.json` (`main.rs` `save_workspace`), `shell/workspace_live.rs`; the MCP server edits workspace files when no desktop holds them | config dir, workspace folder | multi-client | **split**: the workspace document (network, tools, context stack) on the node; layout, scroll, zoom, panel sizes on the client | H1. |
| 23 | Edit session, value store, undo history | desktop: `helios_values` store in the shell, `history.rs` (egui-pop history contract); steps saved as vault versions before the window closes (`docs/persistence.md`) | the vault | multi-client, survival | **node** (the store and its history); **client** (a gesture in progress, drawn before the node confirms) | H1's main exception: a drag or a brush stroke is drawn locally and committed as one coalesced step. |
| 24 | Input (keys, pointer, tablets, MIDI) | desktop: egui-pop keymaps (`keys.rs`, `keymaps.rs`), `pointer_choice.rs` | input devices | hardware, latency | **client** | always. Commands go to the node; key maps are user taste (a `user` setting). |
| 25 | Colour management, display profile | desktop: `display.rs`, `helios-display` (Wayland colour management), display transform in the viewport's shader | the screen, the compositor | hardware | **client** | the display transform runs where the pixels are shown; the node renders in a working or output space. Export colour (`helios-colour`) is node. |
| 26 | File dialogs | egui-pop's in-window dialogs and file browser (`egui_pop::api::files::browser`) over the client's file system | the client's file system | data locality | **split** | a dialog lists the file system of whoever will read the path: the client's for a client source, the node's (through a `files.list` query) for a node path. |
| 27 | Notifications, single instance, session end, logs | desktop `platform.rs` (egui-pop platform), `ending.rs` | the OS desktop | hardware | **client** | the node emits `job.*` events; the client decides to notify. |
| 28 | Clipboard | desktop (egui-pop clipboard, `clipboard.rs`); agent slots in the window | the OS clipboard | multi-client | **split**: the OS clipboard on the client; agents' slots on the node (mcp.rs says these are not built) | |
| 29 | Compute units and GPU calibration | `helios-compute` (local devices, settings never synced, remote units probed) | GPUs | hardware | **either**: each machine calibrates its own devices | already per machine. |

**Headline.** Node only: the vault, the image map and volume registry, jobs
and runs, batch renders and exports, thumbnail generation and its disk
cache, backups, the workspace document, the edit session and its history.
Client only: windows and view state, input, the display and its colour,
textures, notifications, the OS clipboard. Either: the interactive render
and its caches, local AI models, compute calibration, volume watching (the
machine with the device). Split: import and connectors (the source's side
reads, the node stores), accounts (consent on the client, token on the
node), settings (by concern), health, plugins (by manifest), file dialogs,
the clipboard.

## 4. The four hypotheses

### H1. Windows are views of the node

*The node holds everything that has meaning (catalogue, edits, jobs, import
state, selection as a document concept, history); a window holds only view
state (scroll, zoom, hover, panel sizes, focus, GPU textures).*

**What the code already does.**
- Ratings, labels, tags and edit steps are written to the vault before the
  window closes (`quit.rs` module docs, `docs/persistence.md`); the vault is
  already the source of truth for what has meaning.
- The context stack and workspace tools have a file form that works
  without a window (`helios_template::contexts`, `workspace::tools`), and
  the MCP server already falls back to it.
- Jobs on a node survive a closed window, and the relay catches up
  (`relay.rs`).

**What it breaks or requires.**
- The **value store and its undo history live in the window**
  (`history.rs`, `helios_values` in the shell). The MCP history and
  clipboard tools are refused without a desktop (`mcp.rs`). H1 moves the
  store to the node; the window keeps a gesture in progress.
- **The vault's life is the window's life.** `on_exit`
  (`main.rs`, line 1460) saves the interface's settings and keys; after
  `run_native` returns, a `Finisher` drops the vault with a deadline
  (`main.rs`, end of `main`). Closing the last window closes the catalogue.
- **Exports die with the window** (`impl Drop for Exporter`,
  `helios-gui/src/export.rs`, line 210: "the batches still running are
  cancelled").
- **Backups and the health check run on the window's threads**
  (`main.rs`, `back_up`, `health_runner`).
- **Selection** is a view state today (the library's selection in the
  shell). H1 makes a named selection (a set an agent or a job acts on) a
  node concept; the live selection under the cursor stays per window.
- **Exceptions to H1**, state that must stay in the window: a gesture in
  progress (a slider drag, a brush stroke, a mask handle), drawn before the
  node confirms it, committed as one coalesced write (architecture.md,
  "Writes: coalesced"); an unsent text field; the window's undo of
  view-only actions (panel layout).

**Cost.** Every edit becomes a command to the node and an event back.
Locally (in process, or over a Unix socket) that is microseconds to a
millisecond; remotely it is a round trip (1 ms on a LAN, 20 to 100 ms on a
WAN), hidden by drawing the gesture locally and reconciling when the step
lands.

**Verdict.** Holds, with the gesture exception. It is the precondition for
an agent working without a window and for the window closing without
closing the library.

### H2. Thumbnails: management central, display local

*The node owns generation, the disk cache (keys by content hash and
version), invalidation and pruning; the client keeps decoded images and
textures, which it can lose.*

**What the code already does.** The disk cache is content-addressed
(`helios-thumbs`: `Key { image: BLAKE3 of the file, version, chain }` ×
size), sits beside the catalogue (`.helios/thumbs`), and tolerates two
writers (writes by rename, cache.md "Processes"). The textures are a
separate client budget (`thumb_textures.rs`). The split is already there in
the data; only the process boundary is missing.

**What it breaks.** The `Loader` pool runs in the desktop and reads the
raws itself; a remote node means the pool moves to the node and the client
asks for pages of thumbnails (one request per screen, architecture.md
"Batched"). Provisional keys (`p:<size>:<partial hash>`) need a rename event
the client hears.

**Cost.** A 256 px JPEG is 10 to 20 KB: a screen of 100 cells is 1 to 2 MB,
under 20 ms on a gigabit LAN, about 0.3 s at 50 Mb/s. The client keeps a
disk tier of its own for a remote node (cache.md: "The local disk tier
caches what came back under the same key").

**Verdict.** Holds, and cheaply: it is mostly where the code is heading.

### H3. A file resource service that only streams

*A node service serves file content by id (content hash or source path):
byte ranges, thumbnails, previews, originals. It does not interpret them.
Everything that reads files goes through it.*

**What the code already does.**
- `helios_connect::Connector::fetch(item, offset, sink)` streams an item's
  bytes from an offset, and `helios_connect::ranged::Ranged` turns it into
  `Read + Seek`.
- `helios_load::sparse::read` reads a raw's previews and tags without its
  sensor data, over any `Read + Seek`: an import glance reads 4 to 35 % of
  the file (measured on the corpus, e.g. 4.4 MB of a 30.7 MB NEF).
- The vault knows every copy of an image (`Location`: volume, path,
  presence, last verified), so "by id" resolves to a present copy.

**What it breaks.** Today every reader opens paths directly: the engine's
loaders (`helios-load`), the thumbnail reader, the import scan, the export.
H3 puts one service between them and the files. Locally this costs a
function call if the service hands a path (H4); remotely it is the only
way a client gets bytes.

**Cost.** A raw is 5 to 60 MB. Fetching a 30 MB raw: 0.25 s on a gigabit
LAN, 2.4 s at 100 Mb/s, 12 s at a 20 Mb/s upload. A card of 1,000 such raws
is 30 GB: 4 minutes on a gigabit LAN, more than 3 hours at 20 Mb/s.
Ranged reads change the picture for everything but a full develop: a
glance is a few hundred KB to a few MB.

**Verdict.** Holds as the *remote* path and as the single place that
counts and verifies reads. It must not interpret content (that is the
loaders' job) and must not be a mandatory hop locally (H4).

### H4. Hybrid: a local file goes by path

*When the client can read the same file (same machine, shared mount), the
service answers with a path instead of bytes; the client checks it (size
and mtime, or hash) and falls back to the stream. The reverse direction: a
card seen only by the client is offered to the node as a stream by a small
resource agent on the client.*

**What the code already does.**
- `helios_connect::Access` is exactly this choice: `InPlace(PathBuf)` or
  `Fetch`.
- The image map records path + size + mtime → content hash
  (`helios-library/src/map.rs`), and the import prefilter checks size, then
  a partial hash, then the full hash (`import_scan.rs`, `Prefilter`). The
  verification ladder H4 needs exists.
- Volumes have identities (`.helios/volume.json`, `helios-storage`), so
  "the client sees the same file" can be decided per volume: the client
  reports the volumes it has mounted and where; the node answers a path
  only on a volume both see, remapped to the client's mount point (as
  Nuke's `--remap` and Deadline's path mapping do).
- The plugin broker hands a plugin only the paths it is asked about
  (`helios-plugin` module docs): the trust model for paths exists for
  plugins.

**What it breaks.** Nothing locally; it is how the desktop works now. For a
remote node it adds two protocols: the volume report (client → node) and
the resource agent (node → client requests, i.e. the client serves). The
resource agent is the `Connector` trait served over the wire: `sources`,
`list`, `thumbnail`, `fetch(offset)`.

**Trust.**
- *Node → client paths*: the client verifies before reading. Size and
  mtime are cheap but can be fooled by a copy that keeps them; a partial
  hash (first and last blocks) is cheap and catches almost every swap; the
  full hash is the only proof and costs a full read. The image map's ladder
  (size+mtime, partial hash, full hash on import) fits.
- *Client → node paths* are the dangerous direction. A node must never
  open a path a client names unless it lies under a volume or folder the
  node itself registered (the volume registry), after canonicalisation
  (no `..`, symlinks resolved, checked again at open time). A path from a
  client is a hint; the content hash is the identity. On a shared node
  (several users), a client may only name paths under the roots its user
  was granted.
- *Resource agent*: it serves only the sources its user offered (the card
  they ticked), read-only, for the session of the import, authenticated by
  the same token as the link.

**Verdict.** Holds, and it is the way to keep the local case free. The
reverse direction is the only clean answer to "the card is in the laptop,
the node is the workstation".

## 5. How others draw the line

| Tool | Where the catalogue and work live | Local devices | Secrets |
|---|---|---|---|
| **Lightroom Classic** | Catalogue and originals on the local machine; Adobe does not support a catalogue on a network drive. Sync sends Smart Previews, not originals, to the cloud. | imported on the machine | Adobe account on the machine |
| **Lightroom (cloud)** | Originals uploaded to Adobe's cloud; each device keeps a local cache and smart previews; edits sync both ways. The cloud is the node; every client renders locally from previews. | imported on any device, then uploaded | the account |
| **Capture One** | Catalogues (a database, referenced or managed files) or Sessions (a folder holding images, edits and previews side by side). A catalogue on a network share is possible but slow, one writer at a time. | tethering and import on the machine | licence on the machine |
| **darktable** | One process, one `library.db` on the machine, guarded by a lock file; XMP sidecars beside the raws. Multi-workstation use is the known gap ([darktable#18599](https://github.com/darktable-org/darktable/issues/18599)). | local | none |
| **vkdt** | Local, GPU-resident pipeline; thumbnails computed or read from disk on the machine. No client-server mode. In a related forum thread, streaming full renders from a server was doubted for interactive frame rates. | local | none |
| **Immich** | Server holds the database, thumbnails, encoded video, jobs; machine learning can run "on a separate machine". External libraries are server-side folders, mounted read-only. | the phone app uploads; the server never reads the phone's storage | server accounts; clients hold a session |
| **PhotoPrism** | Server indexes an originals folder; thumbnails and sidecars go to a separate storage folder, so originals can be read-only. | upload or a folder the server mounts | server accounts |
| **Blender (Flamenco)** | Farm needs shared storage, "a location that every computer in the farm can access"; or Shaman, which uploads job files into a content-addressed store. | the artist's machine submits | manager-side |
| **Nuke** | Frame Server renders on local or remote workers; paths cross with "the --remap command line argument". Farms (Deadline) map paths per OS and region. | shared storage | studio infrastructure |

**What they teach.**
- Every server-based tool (Immich, PhotoPrism, Lightroom cloud) makes the
  client **upload**: the server never reaches into the client's devices.
  H4's resource agent is the same idea made lazy (glance first, copy on
  import).
- Every render farm needs **shared storage or a content-addressed upload**
  (Flamenco's Shaman), plus **path remapping**. H3 + H4 are exactly that
  pair, with Helios's content hashes as the identity.
- Desktop editors (Lightroom Classic, darktable, vkdt) keep the
  interactive render on the client. Lightroom cloud keeps it there too, on
  previews. None streams interactive renders from a server. This argues
  for the interactive render being **either**, client by default when a
  raw or a smart preview is at hand (choice F not recommended).
- Capture One's Sessions (the edits and previews beside the images) are
  close to Helios's replicas and "the node holding the raw".

## 6. Gaps: node concerns in the client, and the reverse

| # | Gap | Where | Follow-up | Size |
|---|---|---|---|---|
| 1 | The vault's life is the window's: `on_exit` saves, then the vault is closed after `run_native` returns | `helios-gui/src/main.rs` (`on_exit`, `Finisher` at the end of `main`) | the node owns the vault; closing the last window detaches the client | M (after choice A) |
| 2 | Exports are cancelled when the window closes | `helios-gui/src/export.rs`, `impl Drop for Exporter` | batch exports become node runs (export.md "Remote later"); single exports node jobs | M |
| 3 | Backups and the health check run on the window's threads | `main.rs` (`back_up`, `health_runner`) | node jobs, health split (storage on the node, GPU on the client) | M |
| 4 | Import runs in the desktop's jobs and reads sources directly | `shell/import.rs`, `importers.rs`, `helios-library/src/import.rs` | a node import job reading through connectors; the client's card through a resource agent | L |
| 5 | Thumbnail pool in the desktop | `helios-gui/src/thumbnails.rs`, `helios-thumbs` `Loader` | node thumbnail service (`thumb.get` page requests, path or bytes) | M |
| 6 | Value store and undo history in the window; MCP history and clipboard refused without one | `history.rs`, `mcp.rs` | edit sessions and history on the node; the window keeps gestures | L |
| 7 | Workspace document written by two parties (the desktop holding it, the MCP server on files) | `shell/workspace_live.rs`, `helios_template::workspace::tools` | the node owns the workspace; the window holds its layout | M |
| 8 | Transport is loopback only | `helios-inspect/src/transport.rs` | an authenticated network transport (TLS, pinned key, token; the connector `Trust` step as UX) | M |
| 9 | The desktop's link to a node carries jobs only | `relay.rs` | a node client API: cursors, field writes, events by subscription, thumbnails, the file service | L |
| 10 | Remote compute units are not used | `helios-compute/src/remote.rs` | a node renders on request and reports its devices | M |
| 11 | Secrets are per machine with no rule for a remote node | `helios-ai/src/secrets.rs`, `helios-connect` (`Oauth`, `Form`) | a node secret store; consent relayed from the client's browser or device code | M |
| 12 | The plugin broker is started by the desktop | `main.rs` (`Broker::new`) | brokers on both sides, placed by the manifest's side | M |
| 13 | Settings: the desktop reads files, the node keeps `user`/`project` in its vault | `helios-gui/src/config.rs`, `helios-node` `--vault` | clients read `user` and `project` from the node; `global` stays per machine | S |
| 14 | Volume watching lives in the import window | `shell/import_window.rs` (`Watch`) | one watcher per machine, reporting volumes to the node | S |
| 15 | Two processes on one vault drift in memory | `windows.md`, "Helios today" | solved by one owner (gap 1) | (in 1) |
| 16 | Notifications only for the desktop's own jobs and relayed ones | `platform.rs` | the client notifies on any node `job.*` end | S |
| 17 | File dialogs list the client's file system only | egui-pop dialogs | a node `files.list` source for node paths | S |
| 18 | No resource agent | none | the `Connector` trait served by a client process (read-only, per session) | M |
| 19 | No path remapping between machines | none | volume report from the client; node answers paths on shared volumes remapped | S |

## 7. Choices

Decided by the owner on 2026-11-12: **A, B, C, D and E are taken; F is
not** (the client renders when it has the raw and a GPU; the node streams
renders only when it cannot). The choices as they were put:

- **A. Local topology** (recommended): the node is a library the desktop
  embeds in its process (one GPU device, zero-copy viewport, as windows.md
  option C decided), behind the same command, query and event API a remote
  node offers; a separate `helios-node` process only for remote or
  headless use. The alternative, a separate local node process always,
  costs the darkroom its zero copy (windows.md, option B).
- **B. Edit sessions and undo on the node** (recommended): H1 strict;
  windows keep view state and gestures in progress.
- **C. Thumbnail service on the node, display cache on the client**
  (recommended): H2.
- **D. File resource service with the path shortcut** (recommended): H3
  and H4, paths verified by size, mtime and partial hash, stream as the
  fallback, path remapping per volume, and a client resource agent for
  client-only sources.
- **E. Secrets where the work runs** (recommended): the node keeps tokens
  in its own secret store; the client only hosts the consent browser and
  relays the code (or the device-code flow).
- **F. Remote darkroom by streamed node renders** (not recommended as the
  default): the node renders interactive tiles and streams them. The
  recommended default instead is that the client renders when it has the
  raw (local, remapped or fetched once into its disk tier) and a GPU, and
  the node streams only when the client cannot (a web tab, a thin laptop);
  the scheduler's measured-speed mode (ROADMAP 2.2) chooses later.

Order of work if A to E are taken: gaps 1, 2, 3 (survival), then 13, 14,
16 (small), then 5 and 6, then the remote pieces (8, 9, 18, 19, 11).
