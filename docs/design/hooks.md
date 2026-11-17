# Hooks: fact events, hook points and contracts

Status: analysis (2026-11-15). Nothing here is built; the slices at the
end say what would be, in which order.

The owner's ask: re-read how Helios's modules depend on each other, in the
light of the event bus, so that modules stop holding strong dependencies on
one another. The inspiration is the owner's
[QuarKernel](https://github.com/quazardous/quarkernel) (TypeScript):
listeners with ids, `after` ordering and priority, one shared context per
emit, wildcard patterns, composite events, an awaitable emit that returns
each listener's error. The ideas are taken, not the code.

## In short

- Helios has **three ways for parts to meet**, and today only two have a
  name. The **fact bus** ([architecture.md](../architecture.md), "Events:
  the central bus") says what happened. **Typed contracts** (traits,
  command registries, manifests) say how to ask an owner. The third, a
  **hook point** where several modules contribute in order to one
  result, exists only in pieces: egui-pop's alter hooks and steps
  (`egui_pop::api::hooks`, `egui_pop::api::steps`), the controls'
  `controls.change` step, and many hand-written lists in Helios.
- The strongest couplings are not bus problems. They are **lists kept by
  the wrong crate**: `helios-health` holds every module's checks and a
  `Context` field per module; `helios-config` holds every module's
  settings; `helios-init` holds the library's first-run steps; three
  crates each list the processing modules; the desktop's `main.rs` starts
  the backups after an import by hand, in three places.
- **Recommendation:** one small **hook kernel**, UI-free, `Send + Sync`,
  with no async runtime: named hook points with a typed context, listeners
  with ids, `after`/`before` and priority, each listener's contribution
  kept apart and merged only when it succeeds, every error returned,
  every point and listener listable for agents. It sits **next to** the
  bus: a hook emit is a call with a result, a fact is published after.
  The kernel is generic, so it belongs in **egui-pop** as a crate with no
  egui dependency (egui-pop already has two ordering engines that should
  become one); the hook points belong to the Helios crate that owns each
  result.
- **No crate does this** (survey in section 4): own it, about the size of
  egui-pop's `steps` module, reusing its ordering rules.
- **First slice:** health. Each check moves to the module that owns what
  it watches and contributes through a `health.report` point;
  `helios-health` keeps the runner, the finding format and the report.

## 1. Inventory

Every place where a Helios crate depends on another to get something done,
on `next` as of 2026-11-15. "Coupling" says what a change costs today.

| # | Where | Kind | What it couples | Coupling today |
|---|---|---|---|---|
| 1 | `helios-health` (`lib.rs`: `Registry::builtin`, `Context`; `checks.rs`, `backup.rs`, `gpu.rs`, `plugins.rs`, `snapshots.rs`) and `helios-node-core/src/health.rs` (`HealthInputs::with_*`) | registry filled at start, in the registry's crate | the health crate knows storage, the library, backups, the GPU, plugins and snapshots; `Context` has one field per module; the node's `HealthInputs` has one `with_*` per module | **strong**: a module adding a check edits `helios-health` (its check, a `Context` field, `builtin`) and `helios-node-core` (a `with_*`). `helios-health` depends on `helios-storage` and `helios-vault` for that |
| 2 | `helios-gui/src/main.rs` (three calls to `backups.start(Trigger::OnImport, …)`) and `helios-node/src/vault_node.rs` (the `helios-after-import` thread on `library.imported`) | direct call across crates | the desktop's import paths know the storage strategy's trigger | **strong**: the node does it from a fact, the desktop by hand; a new "after import" reaction (the image map, a health re-run) means a fourth call |
| 3 | `helios-node/src/lib.rs` (`registry()`), `helios-gui/src/templates.rs` (`builtin`), `helios-template`, `helios-calibrate`; the `Cargo.toml` of `helios-gui` and `helios-node` (fourteen `helios-module-*` each) | hand-written lists | each processing module named in four crates | **strong**: a new module edits four lists that must agree |
| 4 | `helios-config/src/lib.rs` (`SETTINGS`, one array) | registry filled at start, in the registry's crate | every module's settings declared in the config crate | **medium**: `Schema::new(&[sets])` already takes several sets; only the habit of one array remains |
| 5 | `helios-init/src/lib.rs` (`Registry::builtin`, `library()`) | registry filled at start, in the registry's crate | the library's first-run steps live in the init crate | **medium**: same shape as health, smaller |
| 6 | `helios-gui/src/library.rs` (`helios_terms`, `query_of`), `helios-gui/src/smart_album.rs` (`KEYS`), `helios-vault` (`Query`: `backup`, `cull`, `presence`… one field per feature) | closed struct across crates | each search key is a field of the vault's query, parsed in the desktop | **strong**: a module adding a key (`backup:`) edits the vault's `Query`, the desktop's `Terms` and the smart album keys |
| 7 | Thumbnail badges ([thumbnail-badges.md](thumbnail-badges.md), sections 3 and 4) | registry (designed, slices 1 and 2 built) | modules declare `BadgeDecl`s over cached facts | **weak by design**: declarations, no per-cell callback |
| 8 | Desktop commands: `helios-shell/src/commands.rs` (`Registry`), filled by some fifty desktop files; egui-pop's `CommandSpec` and `Host::dispatch` | typed contract | a command goes to its owner | **right as it is** (architecture.md, "Commands: to the owner") |
| 9 | Node verbs: `helios-node/src/commands.rs` (`verbs()`, one list) and `helios-node/src/mcp.rs` (dispatch) | registry filled at start, in one crate | every verb's schema written in the node binary's crate | **medium**: the shape is right (`helios-node-core/src/commands.rs`), the list is central |
| 10 | Import connectors: `helios-connect` (`Connector`, `ConnectorProvider`, built-ins in `lib.rs`), plugin sources (`helios-plugin/src/source.rs`), wired in `helios-gui/src/main.rs` | trait implemented in other crates, plus a start-up list | sources behind one contract | **right as it is**: data-heavy (listing, fetch, change feeds); [import-connectors.md](import-connectors.md) section 2.2 |
| 11 | `helios-plugin` (depends on `helios-connect`, `helios-health`, `helios-shell`): `commands.rs`, `health.rs`, `source.rs`, `importer.rs` | translator into each registry | the plugin host knows every registry a manifest may feed | **medium**: each new manifest section (`[[badges]]`, filter keys) adds a dependency to the plugin host |
| 12 | `helios-gui/src/controls.rs` (`r.patch(helios_module_exposure::patches())`) and the `controls.change` step (`helios-controls/src/flow.rs`, on `egui_hud::flow`) | direct call into one module; a third step engine | the desktop names the module that has patches | **medium**: one module today, every module tomorrow |
| 13 | `helios-gui/src/masks.rs`, `mask_tree.rs`, `clipboard/mask.rs`, `params.rs` (`helios_module_masks::MAX_MEMBERS`, `shape::*`) | direct use of a module's internals | the desktop draws masks with the module's own types | **acceptable**: masks are an engine contract (hot path, data); see section 2 |
| 14 | `helios-gui/src/shell.rs` and `main.rs`: some thirty `shell.set_*` / `shell.follow_*` calls (`StorageLink`, `LibraryFolder`, `QueueSource`, closures) | traits defined by the consumer, implemented at the composition root | the shell asks for what it needs; `main.rs` provides it | **right in shape** (the consumer owns the trait); the cost is `main.rs`'s size, not the coupling |
| 15 | `helios-node-core/src/endpoint.rs` (`WindowCommands`, implemented by the desktop) | trait owned by its definer | the node reaches a desktop window | **right as it is** |
| 16 | Ad hoc subscriptions: `Persist` (vault bus, each frame), `Shell::engine_notices` (engine bus to toasts), `config/store.rs`, the import window's `volume_events`, node-side `volumes.rs`, `settings.rs`, `library.rs`, `job_promises.rs`, the AI service | fact bus | listeners by kind, each written its own way | **weak**: facts, as designed; the desktop's node-facing ones move onto the node-event relay's `NodeEvents::listen(id, patterns)` (in progress), which lists who listens to what |
| 17 | egui-pop's `Host` (`egui_pop::api::host`): dispatch, journal, store, history, contexts, clipboard | typed contract owned by egui-pop | the interface reaches the kernel | **right as it is** (ROADMAP, 2026-10-12) |
| 18 | egui-pop's alter hooks (`Hook::menu`, `palette`, `settings`; id, priority, `after`) and steps (`StepSpec`, before/run/after, traced) | hook points, UI thread only (`Rc`, JSON arguments) | features expose their lifecycle | **right in kind**; two engines, neither usable off the UI thread or by the node |
| 19 | `helios-ai` depends on `helios-extract`; `helios-resolve` on `helios-ai` | typed contract | extractors and resolvers on the AI service | **right as it is** ([extractors.md](extractors.md), section 6) |

## 2. Classification

Three channels, each with its test:

- **Fact event:** something happened, the publisher does not care who
  reacts and wants no answer. Goes on the bus (architecture.md: typed,
  past tense, recorded, network-capable, not a command channel).
- **Hook point:** an owner builds one result and lets others contribute,
  in an order that matters, and needs to know who failed. Goes through
  the hook kernel (section 3).
- **Typed contract:** a caller asks one owner for something data-heavy,
  hot or engine-level. Stays a Rust trait or a registry entry, defined by
  the crate that owns the concept, never by its callers.

| # | Becomes | Why |
|---|---|---|
| 1 Health checks | **hook point** `health.report` | Many owners contribute findings to one report, in a stable order, and one failing check must not hide the others: the runner's job today, done by hand. Each check moves to its owner; `Context`'s per-module fields become what each listener reads for itself |
| 2 After an import | **fact event** `library.imported` | Nobody answers; backups, the map and a health re-run are reactions. The desktop's in-process imports publish the same fact the node does, and the storage strategy listens |
| 3 Module lists | **typed contract**, one catalogue | The engine's hot path: `Module` (`helios-core/src/module.rs`) stays a trait. The lists become one catalogue crate the node, the desktop, templates and calibration read; no hook point (no order, no partial failure) |
| 4 Settings | **typed contract** (declarations by owner), collected through `settings.declare` | Each module declares its `Setting`s next to its code; the collect is a hook point only because plugins and modules come and go and the form's sections are altered (egui-pop's `Hook::settings` already does that part) |
| 5 Init steps | **hook point** `init.steps` | Contributions in order (`after` matters: the library folder before the first import), with requirements per module |
| 6 Search keys | **hook point** `library.search.keys` for the grammar; **typed contract** for execution | Parsing, suggesting and describing a key is a contribution (the key's owner says what `backup:short` means as a vault rule). Execution stays the vault's rules (`helios-vault` `rules`), one SQL query per window ([library-search.md](library-search.md), section 4); no listener runs per photo |
| 7 Badges | **hook point** `thumbs.badges`, collected at start; evaluation stays data | Declarations collected and alterable (hide, reorder); predicates over cached facts per cell, as [thumbnail-badges.md](thumbnail-badges.md) decided. A hot point refuses callback listeners |
| 8 Desktop commands | **typed contract**; a `command.run` step for before/after | A command has one owner. Observing, vetoing or extending a run is a step, which egui-pop already offers |
| 9 Node verbs | **typed contract**, declared by owner | Each owning crate registers its verbs (`helios-node-core`'s `Registry` takes them); `verbs()` stops being one list |
| 10 Connectors | **typed contract** | Data-heavy and stateful; [import-connectors.md](import-connectors.md) stands. Only their registration list goes through the catalogue of contributions |
| 11 Plugin host | **hook point registrations from manifests** | A manifest's `[[hooks]]` entries become out-of-process listeners on named points (section 3), so the plugin host stops depending on each feature crate for each new section |
| 12 Control patches | **typed contract** in the module catalogue; `controls.change` on the kernel | Patches are declarations a module ships, read from the catalogue (#3). The step moves onto the shared kernel when egui-pop's steps do |
| 13 Mask internals | **typed contract** | Engine data in a hot path; the module owns the types, the desktop reads them. Keep |
| 14 Shell setters | **typed contract** (consumer-owned) | Correct direction already. Where a setter only hands over a shared service, egui-pop's `ServiceToken` replaces it |
| 15 `WindowCommands` | **typed contract** | Keep |
| 16 Subscriptions | **fact events** | Keep; desktop listeners of node facts go through `NodeEvents::listen` |
| 17 egui-pop `Host` | **typed contract** | Keep |
| 18 egui-pop hooks and steps | **hook points**, re-based on the kernel | One ordering engine, one introspection format, usable by the node |
| 19 AI and extractors | **typed contract** | Keep |

So of nineteen places, five become hook points (1, 5, 6, 7, 11), one a
fact event (2), and the rest stay contracts; three of those contracts
(3, 4, 9) move their list to the owners.

## 3. The hook kernel

### What it does

A **hook point** is declared once by the crate that owns its result: an
id (`health.report`), a description, the type of its context, its kind
and its limits. A **listener** is registered under a point by any crate:
an id of its own (`storage.free-space`), an owner (a module or a plugin),
`after` and `before` ids, a priority. An **emit** runs the listeners in
order on one context and returns the context, each listener's error and a
trace.

### Rust sketch

```rust
/// A hook point: a type per point, so the context is checked at compile
/// time. Declared by the crate that owns the result.
pub trait Point: 'static {
    const ID: &'static str;
    /// What every listener reads (the emit's input).
    type Input: Send + Sync;
    /// What one listener contributes.
    type Part: Send;
}

/// The point's declaration, for introspection and limits.
pub struct PointSpec {
    pub id: &'static str,
    pub description: &'static str,
    pub owner: Owner,
    /// Alter: listeners edit the collected list in turn (egui-pop's
    /// alter hooks). Collect: each listener adds parts. Guard: a listener
    /// may stop the run with a reason (egui-pop's before-hooks).
    pub kind: Kind,
    /// Hot points (per frame, per cell) refuse deferred and
    /// out-of-process listeners.
    pub hot: bool,
    /// How long deferred listeners get before they count as failed.
    pub deadline: Duration,
}

pub struct Listener<P: Point> {
    pub id: ListenerId,          // "storage.free-space"
    pub owner: Owner,            // module or plugin id
    pub after: Vec<ListenerId>,
    pub before: Vec<ListenerId>,
    pub priority: i32,           // among unordered listeners, higher first
    pub run: Run<P>,
}

pub enum Run<P: Point> {
    /// Runs on the emitting thread.
    Sync(Arc<dyn Fn(&P::Input, &mut Parts<P::Part>) -> Result<Flow, HookError> + Send + Sync>),
    /// Runs off the emitting thread (a thread of the kernel's pool, or a
    /// plugin's RPC); its parts come back before the point's deadline.
    Deferred(Arc<dyn Fn(Arc<P::Input>) -> Pending<P::Part> + Send + Sync>),
}

pub enum Flow { Go, Stop(Message) }

pub struct Emitted<P: Point> {
    /// The parts, in listener order, each stamped with its listener.
    pub parts: Vec<(ListenerId, P::Part)>,
    /// Every listener that failed, panicked or missed the deadline.
    pub errors: Vec<(ListenerId, HookError)>,
    /// Set when a guard listener stopped the run.
    pub stopped: Option<(ListenerId, Message)>,
    pub trace: Trace,
}

impl Kernel {
    pub fn declare<P: Point>(&self, spec: PointSpec) -> Result<(), Refused>;
    pub fn listen<P: Point>(&self, l: Listener<P>) -> Result<(), Refused>;
    /// Drops every listener of an owner (a plugin uninstalled, a module
    /// switched off).
    pub fn forget(&self, owner: &Owner);
    /// Blocks until every listener answered or the deadline passed:
    /// worker threads and the node, never the interface thread.
    pub fn emit<P: Point>(&self, input: P::Input) -> Emitted<P>;
    /// The interface thread's form: returns at once, polled each frame
    /// like egui-pop's deferred `Reply`.
    pub fn emit_later<P: Point>(&self, input: P::Input) -> InFlight<P>;
    /// Every point, its listeners in resolved order, and their problems.
    pub fn points(&self) -> Vec<PointInfo>;
}
```

### Rules

- **Order.** Listeners are sorted by `after`/`before` first, then by
  priority (higher first), then by registration order, so the order is
  stable across runs. A missing `after` target is reported and ignored,
  as egui-pop's alter hooks do; a cycle is refused at `listen`, naming
  the listeners in it. The resolved order is computed when listeners
  change, never per emit.
- **Error boundary.** Each listener writes into its own `Parts` buffer;
  the buffer joins the result only when the listener returns `Ok`. A
  listener that errs, panics (caught with `catch_unwind`) or misses the
  deadline adds nothing and one entry to `errors`; the others run. So a
  failing module never leaves half a contribution behind and never stops
  the rest, which is QuarKernel's "error boundary" made per listener.
- **Shared context, typed.** QuarKernel's shared context per emit is
  split in two: `Input` (read by all, shared) and `Part` (one listener's
  output). An `after` listener that must see earlier parts declares it,
  and the point's kind is then `Alter`: listeners receive the list so far.
  No untyped bag of values.
- **Sync and deferred, no async runtime.** Helios runs on threads and
  jobs (`helios-core`'s `Jobs`), with no async runtime anywhere; the
  kernel does not bring one. A deferred listener returns a `Pending`
  (a one-shot channel); the kernel starts deferred listeners at once, runs
  the sync ones meanwhile, and waits for the deferred ones up to the
  point's deadline. Ordering holds across both: a listener `after` a
  deferred one waits for it.
- **Out of process.** A plugin declares `[[hooks]]` in
  `helios-plugin.toml` (`point`, `id`, `after`, `priority`); the plugin
  host registers a deferred listener that makes the RPC call. A hot point
  refuses it, which keeps [thumbnail-badges.md](thumbnail-badges.md)'s
  rule: a plugin is never called per cell.
- **Introspection.** `points()` gives each point's id, owner, kind and
  description, and its listeners in resolved order with owner, priority,
  `after`/`before`, problems (missing target, refused) and counters
  (runs, errors, last error, time spent). It is served by the inspect
  endpoint and MCP (`hooks.list`, `hooks.trace {point}`) and shown by
  egui-pop's debug bridge as a `hooks` collector, so an agent sees who
  contributes where and why a contribution is missing.
- **No wildcards on points.** A listener names one point: a contribution
  to "any point" has no typed context to fill. Wildcards stay on the bus
  (`NodeEvents::listen(id, ["storage.*"])`).
- **No composite events yet.** QuarKernel's `when([a, b])` waits for
  several facts. No case in this inventory needs it: the closest ("vault
  open and node linked") is state, read when needed. Added when a real
  case shows up.

### Next to the bus, not on it

| | Fact bus | Hook kernel |
|---|---|---|
| Question | what happened? | who adds what to this result? |
| Call | publish, no answer | emit, returns parts and errors |
| Tense | past | present: the result is being built |
| Order | publication order only | `after`/`before`, priority |
| Crosses processes | yes: relays, resume, resync | no: in process; plugins through their RPC listener |
| Recorded | the journal (`events --since`) | the trace ring and `hooks.list` |

A point's owner publishes a fact after an emit when others should know
the result exists (`health.reported` after `health.report`). A listener
error is also a fact (`hook.failed`, with point, listener, owner and
message), so the central log reads it like any other failure. The bus
never carries a hook emit, and a listener never takes an order from the
bus (architecture.md: "Not a command channel").

### egui-pop or Helios

- **egui-pop:** the kernel, as a crate with no egui dependency (the
  node must use it without pulling the interface in).
  egui-pop's alter hooks and steps are hook points of exactly this shape
  (id, priority, `after`, before/run/after, traced), on the interface
  thread only; re-basing them on the kernel leaves one ordering engine and
  one introspection format, and the debug bridge shows Helios's points and
  egui-pop's side by side. ROADMAP (2026-10-12) asks egui-pop to expose
  each feature's lifecycle as hooks; this is the same promise, made
  usable off the interface thread.
- **Helios:** every point its crates own (`health.report`,
  `init.steps`, `library.search.keys`, `thumbs.badges`,
  `settings.declare`), the listeners of its modules, the plugin host's
  `[[hooks]]` registration, and the inspect/MCP endpoints.
- If egui-pop's maintainers (us) later decide the kernel does not fit
  there, the same crate moves to Helios unchanged: nothing in it knows
  either side.

## 4. Survey: Rust crates

Licences as published on crates.io (2026-11-15); `deny.toml` allows MIT,
Apache-2.0, BSL-1.0 and the other permissive ones.

| Crate | Licence | What it gives | Fit |
|---|---|---|---|
| `bevy_ecs` 0.20 | MIT OR Apache-2.0 | system ordering (`before`/`after`, sets), events, observers | Right ordering ideas, but an ECS world and schedule: far too much to carry for hook points. Take the ordering vocabulary |
| `tower` 0.5 | MIT | `Service` + `Layer` middleware chains | Ordered by composition only, async-first, no ids, no introspection, no per-listener errors |
| `signals2` 0.3 | BSL-1.0 | signal/slot after boost::signals2: slot groups (ordering), combiners collecting results | Closest in spirit; low use (16k downloads), last release 2023, no ids for `after`, no deadline or deferred slots. Not adopted |
| `eventbus` 0.5 | AGPL-3.0+ | event bus after MinecraftForge, priorities | Licence not in `deny.toml`'s list and unmaintained since 2018. Excluded |
| `event-emitter-rs` 0.1, `typed-emitter` 0.2, `pharos` 0.5 | MIT/Apache, MIT, Unlicense | Node-style emitters, observer streams | Fan-out like the bus; no ordering, no result. Not for hook points |
| `event-listener` 5, `async-broadcast` 0.7 | MIT OR Apache-2.0 | notification and broadcast primitives | Below the level needed; the bus already covers fan-out |
| `inventory` 0.3, `linkme` 0.3 | MIT OR Apache-2.0 | registration at link time (distributed slices) | Would fill the module catalogue (#3) without a list; per-platform linker caveats (WebAssembly, where egui-pop's web gallery runs). Not now: an explicit `register(&mut Catalogue)` per crate is enough and visible |
| `petgraph` 0.8, `topo_sort` 0.4 | MIT OR Apache-2.0 | topological sort | The ordering is a few dozen lines over small lists; egui-pop's alter hooks already sort. Own it |
| `tracing-subscriber` 0.3 | MIT | ordered layers over spans and events | For the central log's sink, not for hook points |

**Own**, small: the kernel is about the size of egui-pop's `steps.rs`
plus ordering and the deferred wait, with no new dependency.

## 5. Migration

Strongest coupling first. Each slice builds and passes its tests alone,
and changes no behaviour a user sees.

### Slice 0: the kernel

The crate (in egui-pop, section 3): points, listeners, ordering, error
boundary with `catch_unwind`, deferred listeners with a deadline,
introspection, trace ring. Unit tests: order with `after`/`before` and
priority, a cycle refused, a missing target reported, a failing and a
panicking listener leaving the others' parts whole, a deferred listener
past its deadline. No user yet.

### Slice 1: health

Before:

```text
helios-health ──► helios-storage, helios-vault
  checks.rs, backup.rs, gpu.rs, plugins.rs, snapshots.rs   (every owner's checks)
  Context { volumes, library_folder, backup, gpu, plugins, snapshots, … }
helios-node-core/health.rs ── HealthInputs::with_gpu / with_plugins / with_snapshots / …
helios-plugin/health.rs ──► helios-health::Registry::add
```

After:

```text
helios-health          declares point `health.report` (Input: the run's
                       time and scope; Part: Finding), keeps Finding,
                       Declaration, the runner and the report
helios-storage    ──►  listens: free space, volumes, backups
helios-library    ──►  listens: library folder, changed outside, pairs
helios-gpu        ──►  listens: adapters, kernels
helios-node-core  ──►  listens: snapshots, database integrity
helios-plugin     ──►  registers each manifest check as a deferred listener
helios-health no longer depends on helios-storage or helios-vault
```

Each listener reads what it needs itself (the gathering moves with the
check), so `Context`'s per-module fields and `HealthInputs`'s `with_*`
go away. The report's order is the listeners' resolved order; a check
that fails shows as a warning finding naming its owner, as the runner
does today.

Where it stands (2026-11-16): done. The point is in
(`helios_health::HealthReport`, a `collect` emit; `Registry::add`
registers each check as a listener owned by its module,
`Registry::remove_owner` takes a module's checks away, and a listener's
failure is its check's "failed" problem). Each check lives in its owner's
crate, over its owner's state (`helios_health::Owned`), which the owner
gathers once per run (`helios_health::Gather`, cached in the run's
`Context`, which holds nothing else): `helios_storage::health` (free
space) and `helios_storage::health::backup`, `helios_library::health`
(integrity, the library, its folder, changed outside, pairs),
`helios_gpu::health` (adapter, kernels; it reads the GPU itself),
`helios_plugin::health::installed` (it probes the plugins itself) and
`helios_node_core::health::snapshots`. The node composes them
(`HealthInputs::checks`: the volumes mounted are read once per run and
shared), the desktop adds the GPU and the plugin host
(`helios_gui::shell::health_checks`). `HealthInputs::with_*` and
`Context`'s fields are gone, and `helios-health` depends on neither
`helios-storage` nor `helios-vault`.

### Slice 2: after an import

The desktop's in-process imports publish `library.imported` on the bus
they report to, as the node does; the storage strategy's
`OnImport` trigger listens to it on both sides (the node's
`helios-after-import` thread is the model). `main.rs`'s three
`backups.start` calls go. A fact, not a hook point: no answer is wanted.

Where it stands (2026-11-16): done on the desktop side, as hook points
after all, because the reactions' order matters (the undo step before the
Library is filled, the session before the photos are selected). Every
import path says its end through one `helios_gui::import_end::Publisher`
(the `library.import` command's thread, the photos chosen through a
remote connector in a service's page, and the node's promise through
`NodeWatch`). `helios-gui` declares its two points, `import.ended` and
`import.failed` (`import_end::POINTS`), on the one bus `main` builds and
hands to the shell; their listeners are named, owned and ordered
(`import_end::ORDER`, `FAILED_ORDER`), the host's `backups.on-import` and
`library.refill` among them. The shell passes the end through its point,
then says `import.ended` or `import.failed` on the journal with the
listeners that ran. The listeners that need the shell's own state (the
Log panel, the toast, the sessions) mark their place in the order and
the shell does their part right after, in that order: a bus listener is
`Send + Sync`, the shell's panels are not. The cameras' default looks
are put on by the publisher, before the point, on every path: they add
to the import's changes (its one undo step) and write the vault, which
is not done on the interface's thread. The node's own
`helios-after-import` thread is unchanged.

### Slice 2b: the library changed

The vault says its library changed four ways (`vault.library_changed`,
`vault.locations_changed`, `vault.changed_outside`, `vault.captures_set`)
and its stacks two (`vault.stack_changed`, `library.decision`); the
desktop used to turn the first four into a `locations_changed` flag,
while imports and undo set a separate `refill` flag, so an import or an
undo read the Library twice, and the albums compared the library
history's revision every frame.

Where it stands (2026-11-16): done. `helios_gui::library_changed`
declares two points on the one bus. `Persist` gathers what one frame
heard (the vault's events, and what the desktop says through its
`library_changed::Publisher`: `import.ended`'s `library.refill`, a
library step undone or redone) into one change (`kinds`: contents,
locations, captures, stacks; `by`: vault, import, history) and passes it
once through `library.changed`: `library.volumes`, `library.fill`,
`library.albums`, `library.stacks`, each running only when the change
concerns it. Once the Library read again is shown, the shell passes
`library.filled`, whose `library.select-imported` selects the photos the
last import brought, in that pass (no more polling). Both are said on
the journal with the listeners that ran (`ran`). As for the import's
points, the listeners need state that is not `Send + Sync` (the Library,
the reads through the node): they mark their place, and `Persist` (for
`library.changed`) or the shell (for `library.filled`) does their part
right after, in the order they ran.

Since 2026-11-17 the node's `library.imported` (an import through it
ended, its record in the import history; any client's) is a fifth kind,
`imports`, and `library.imports` (the shell's) reads the Library's
import sessions again on it. They were read every 10 seconds, a blocking
node request on the interface's thread; now an idle pass reads nothing.

### Slice 2c: volumes coming and going

The node's volume watch published `volume.mounted` / `volume.unmounted`
on the journal and also called callbacks in the order the desktop's
start registered them: the scan of where each photo is (which only
woke the scan's own thread), the image map's feed, the backups on
connect and the health check's counter. A backup on connect could read
the locations before the scan set them: plugging the library disk back
in skipped its photos as offline.

Where it stands (2026-11-17): done. `helios_node_core::volumes` declares
two points (`POINTS`), passed on the application's bus the watch is
handed at its start, after the journal's event and on the watch's
thread. Their listeners are named and placed (`MOUNTED_ORDER`,
`UNMOUNTED_ORDER`): `import.cards` before `library.scan`;
`library.scan` scans inside the pass (`Scanner::scan_now`, one scan at
a time with the scanner's thread); `backups.on-connect`,
`health.recheck` and `map.plugged` after it. The Storage page and the
Library's badges follow the scan through `library.changed`. The order is
a reviewed dump (`crates/helios-gui/tests/bus/volumes.txt`);
`VolumeWatch::on_change` is gone. The headless node registers
`backups.on-connect` without a scan (it runs none): the missing target
is reported and ignored.

### Slice 2d: a backup's end and the cards safe to format

A backup job's end was a flag the jobs' journal reader set
(`take_backups`); the shell then read the badges again and checked the
cards still inserted on a thread, polled each frame, and said the safe
ones itself (toast, Log, notes). The cards were kept twice, `waiting`
for the shell and `shared_waiting` for the health check's thread, copied
by hand after each change; neither the import window nor the Storage
page said whether a card could be formatted.

Where it stands (2026-11-17): done. `helios_gui::backup_end` declares two
points (`POINTS`) on the one bus. A backup's end passes once through
`backup.ended`: `storage.backups` (the badges read again), then
`cards.safety` (the cards still inserted checked again off the frame:
the check reads the vault). The cards are one list, `CardList`: the
import's `storage.cards-waiting` fills it, the check marks it, and the
import window's card rows, the Storage page's card rows (`card`,
`backupText`) and the health check's "Cards safe to format" all read it.
Once the check lands, the shell marks the list, then passes the cards
that became safe through `card.safe`: `cards.toast` (toast, Log),
`storage.cards` (the Storage page's reads out of date), `health.cards`
(the cards check run again, or once a run under way ends), in that
pass. Both are said on the journal with the listeners that ran (`ran`).
The order is a reviewed dump (`crates/helios-gui/tests/bus/backups.txt`).

### Slice 2e: an import account signed out or removed

Signing an account out wrote the account book and the keyring directly
(`sign_out`, then `forget_source` per source and `book.remove` for a
removal); the Settings app's rows followed `account.*` on the journal,
nothing else did. A choice of photos under way on the account (the
service's page, its thread waiting half an hour) was left running: it
went on after the sign-out, and a token found gone could write the
account back as "needs attention".

Where it stands (2026-11-17): done. `helios_gui::account_gone` declares
two points (`POINTS`), `account.signedOut` and `account.removed`, with
one listener order for both (`ORDER`): `import.cancel-picking` (the
picking cancelled, a sign-in running on the account given up, its
photos chosen and not imported given up), `accounts.credentials` (the
token revoked where the service can, the keyring's secrets forgotten,
the account written "not connected" or forgotten), `import.sources` (the
import window's Sources and Detail columns in the same pass, a removed
account's saved sources forgotten), `storage.owners` (the Storage page's
volumes named by their accounts), `settings.accounts` (Settings >
Accounts' rows) and `health.accounts` (the plugin checks handed the
accounts). `import.signOut` and `import.disconnect` check the request,
then pass the point once on the interface's thread; the pass is said on
the journal with the listeners that ran (`ran`). The order is a reviewed
dump (`crates/helios-gui/tests/bus/accounts.txt`).

### Slice 2f: an image opened or closed in the darkroom

Switching images saved the edit open only on one path: the desktop's
opening of a raw called `persist.close_image` first, an order that lived
in a comment ("what is drawn now is not saved into its edit"). Any other
opening (`Persist::open_image` called directly) replaced the edit with
the next image's, its last step lost; and the save's answer came at a
later frame, after the next image was already shown, so a failed save
could not keep the image open.

Where it stands (2026-11-17): done for the switch itself.
`helios_gui::image_open` declares two requests (`POINTS`):
`image.open` and `image.close`, which `Persist::open_image` and
`Persist::close_image` pass on the shell's bus before anything of the
next image loads. Their listeners are named and placed (`ORDER`):
`edit.save` first (the step not saved yet written through the node and
its answer waited for inside the pass: a refused write refuses the
request with its reason, and the image stays open, its edit marked
unsaved), then `history.scope` (the value store's steps and the
snapshot book cleared, so the next image's undo never walks into this
one's steps). A refusal stops the listeners after it. The journal says
`image.closed` (with `from`, `to` and `ran`), `image.opened`, or
`image.refused` (with who refused and why). The order is a reviewed
dump (`crates/helios-gui/tests/bus/darkroom.txt`).

#### What follows the image open (2026-11-17)

**Decision (2026-11-17): followers hold their own state behind a handle
and are local listeners of `image.opened` / `image.closed`; no per-pass
inbox.** The other way was an inbox: `image.opened`'s listeners only
record that they were reached, and the shell drains the inbox at a
declared point of its pass with `&mut Shell` in hand. Reasons for the
handle:

- **One framework, already used.** `image.open`'s own listeners and
  `library.changed`'s four (c80172a5) are local listeners holding the
  part of the shell they change (`Rc` / `Arc`); an inbox is the shape
  `library.changed` left ("listeners that only marked their place, then
  the persistence layer did each part"), because the order dump then
  showed names while the work ran elsewhere, in a hand-written order.
- **"Not on the bus = does not exist".** With handles, the listener *is*
  the work: the dump (`crates/helios-gui/tests/bus/darkroom.txt`) lists
  every follower, its owner and its `after`, and the journal's
  `followed` says which ran. With an inbox, the drain is one more
  hand-written list in the shell, and a follower missing from it would
  be listed yet never run.
- **No special case.** Each follower is registered the same way
  (`image_open::listen_follower_local(bus, id, run)`), in its place in
  `image_open::FOLLOWERS`; a new follower needs a handle on its own
  state, which is what the bus asks of every listener anyway.
- **Inside the pass.** The followers run before `open_image` returns, so
  nothing of the next frame sees a half-switched darkroom (before, the
  workshop learned the image at the next `Persist::frame`, a frame late).

The cost: what a follower changes must be reachable without `&mut
Shell`. Each follower's state that is not yet is moved behind a handle
the shell and the listener share (as `Shell::library_parts` did), never
reached through the shell from the listener.

Where it stands: `image.opened` (an image loaded) and `image.closed`
(closed with none after it, or an opening that failed to load) pass
once per switch, their followers in this order:
`snapshots.book` (the image's pinned snapshots and the parameters its
edit saves, into the snapshot book), `mask.marks` (the labels and locks
its edit was saved with, into the value store) and `workshop.photo` (the
workshop session's photo: it was compared at every `Persist::frame`,
`set_edited_image`, and no longer is). A switch to another image passes
`image.closed` with no followers (the next image's `image.opened`
follows), so each follower runs exactly once.

What the slice's list named and is **not** a follower of the image open,
so it stays where it is (read 2026-11-17):

- **The viewport's source** is the load itself, part of the request's
  act (`Shell::set_view_source`, also used for a file outside the
  library and for a raw's first paint); the **preview** follows the
  network shown each pass: pixels, the listed exception.
- **The panels that follow the photo** (`panel.state`): made stale when
  an opening is *asked* (`Shell::open_from_library`, before anything is
  loaded), then derived each pass from the preview's render status,
  which changes without any switch. Not an image compare.
- **The stand-in** (the large thumbnail drawn while a raw is made
  ready): asked with the opening, dropped once the render is complete:
  the same.
- **The display route** (W8, `display::follow_window`) follows the
  window's screen, not the image.
- **The title** is the workspace's name, not the image's.
- **Decisions and the mask tree** are derived from the network shown
  (`Decisions::refresh`, `MaskTree::follow`), which changes in place with
  every edit; a switch only resets the decisions' book, inside
  `set_view_source`. Following the network's changes (an event when the
  shown network changes) would be a slice of its own, not
  `image.opened`.

### Slice 2g: a workspace's state applied

Applying a workspace's state (it opened; an edit, a reset, an agent's
step; an undo or a redo of one; its files changed outside and taken in)
put its tools, library, layout and name in place, then remembered the
keys loaded when its tools changed. Four other parts compared the
state on their own, each pass: the sub-workspaces (by the store's
sequence number, at the start of the pass, before the state was
applied), the place's scope after them, the graph editor and the
Workshops panel (by the whole text, and only while their panel was
drawn: a hidden panel read a change late).

Where it stands (2026-11-17): done. `helios_gui::workspace_applied`
declares one point (`POINTS`), `workspace.applied`, passed once by
the shell once the state is applied, with what changed (`Changed`:
tools, library, network, nodes, sub-workspaces, composites, layout,
name; all of them when the workspace opened; `nodes` is what an image
goes through: the network's nodes and links, or the composites, not
the nodes' positions). Its listeners (`ORDER`) each
run only for the parts they read: `workspace.keys` (tools; the
shortcuts editor's follower then takes no step for keys an undo put
back), `contexts.subworkspaces` and `contexts.scope` (sub-workspaces;
the list, then the store's scope that reads it), `graph.template`
(network, sub-workspaces, composites), `workshops.composites`
(network, composites), `library.heads` (nodes: the Compare panel's
renders made again) and the host's `darkroom.network` (nodes: the image
shown goes through the new network in place, or is opened again). The pass is said on the journal with `changed`
and the listeners that ran (`ran`). The order is a reviewed dump
(`crates/helios-gui/tests/bus/workspace.txt`).

Left, and why: the shell still notices an undo or a redo of the state
by the store's sequence number of its path, once per pass (a number,
no longer the text): the value store has no path listener yet (ADR
0016 names it). The host's listener only marks the change: the image
is passed through the new network after the shell's pass, by the host,
which holds the image (2026-11-17). The workshop session (`follow_session`) follows the value
store's history, not the workspace's state.

### Slice 3: one module catalogue

One crate lists the processing modules and what each ships (its
`ModuleRef`, control patches, templates' defaults). `helios-node`'s
`registry()`, `helios-gui`'s `templates::builtin` and `controls.rs`,
`helios-template` and `helios-calibrate` read it; a new module edits
one list. A typed contract: no kernel needed.

### Slice 4: declarations by owner

`settings.declare` and `init.steps` points; `helios-config`'s `SETTINGS`
and `helios-init`'s `library()` move to their owners; node verbs
registered by their owning crates into `helios-node-core`'s `Registry`.

### Slice 5: search keys and badges

`library.search.keys`: each key's owner contributes its name, its
suggestions and its translation into a vault rule
(`helios-vault` `rules::Rule`), so `Query` stops growing a field per
feature (`backup`, `cull`, `presence` become contributed rules). Built
with [library-search.md](library-search.md)'s grammar slice.
`thumbs.badges`: the `BadgeDecl` registry of
[thumbnail-badges.md](thumbnail-badges.md) section 4, collected through
the kernel (a hot point: declarations only).

### Slice 6: one engine

egui-pop's alter hooks and steps, and the controls' `controls.change`
step, re-based on the kernel; plugin manifests gain `[[hooks]]`. The
debug bridge and `hooks.list` show every point in one place.

### The read kernel and the central log

- **The read kernel** (Settings › Storage: values drawn from a local
  cache, refreshed in the background, a spinner per value) is a
  **listener of facts**, not a hook point: it invalidates its keys on
  node events through `NodeEvents::listen("reads", ["storage.*",
  "volume.*", …])`, and each read's freshness and invalidating events are
  declared with its command (a typed contract, next to the command's
  schema). It uses the kernel in one place: a page whose rows come from
  several modules (the "In use" rows, one per cache kind and plugin)
  collects them through a point (`storage.usage.rows`), each row naming
  the read that fills it, so the page draws every label at once.
- **The central log and notifications** use all three channels. Posting,
  updating and withdrawing a notification are **commands** to the log's
  owner (one owner, an answer). A record's arrival is a **fact**
  (`log.recorded`), which the status bar's count and the window follow.
  How a record is shown (a toast, a badge, a row only) is a **hook
  point**, `log.present`: modules contribute rules in order (a module's
  own rule before the default, `after` the user's mutes), and the first
  decision wins. The kernel's own listener errors reach the log as
  `hook.failed` facts, so a failing module is seen once, with its owner,
  instead of being lost.

## Open questions (not decided)

- The crate's home: egui-pop (recommended) or Helios.
- The deadline for deferred listeners per point: one default (proposed
  2 s) with a per-point override, or always per point.
- Whether `Alter` points (listeners editing the list so far) and
  `Collect` points (parts merged at the end) stay two kinds, or `Collect`
  is an `Alter` that only appends.
