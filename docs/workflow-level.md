# The workflow level: nodes run items through the journal

Status: **decided** (2026-10-22). The plan to wire the workflow journal
into the node was accepted on 2026-10-22, and this design, with every
default below and the answers to the questions at the end, was accepted
the same day.

It extends [architecture.md](architecture.md), "Two levels of
execution", which states the model (after
[grampy](https://github.com/quazardous/grampy)), and builds on
[jobs.md](jobs.md) (how work reports) and [gui.md](gui.md), "Queue
panel" (what people see).

## Two meanings of "node"

This note needs both, so it names them apart:

- a **graph node** is a node of a network (`exposure`, `decide_exposure`,
  an export); the journal calls the steps of a workflow "nodes" too, and
  here a journal node is always named after the graph node it stands for;
- a **node process** is a running `helios-node`: what serves the inspect
  and control endpoints, the MCP server and `helios batch`.

"The node's inspect API" is the node process's; "which nodes queue" are
graph nodes.

## What exists today

Read from `main` at release 0.24.0.

- **`helios-journal`** keeps one row per (item, graph node): `running`,
  `scheduled` (failed, retry due), `done`, `skipped`, `failed`, `omitted`.
  A `Workflow` is a DAG of `Node`s declared after their parents, each
  with a join (`need` k of n, all by default), an exclusive `choice`, a
  `lease_ms` (60 s by default) and a `Retry` (`limit` attempts, `delay_ms`,
  `exponential`; one attempt by default). `claim(node, max, candidates,
  now)` takes the claimable ones among the candidates the caller gives,
  atomically, under one token; `conclude(lease, outcome, now)` writes all
  or nothing and refuses a lease no longer held; a branch omits the other
  children and what only they lead to; `expire(now)` gives back leases
  past their due time without counting the attempt; `monitor(now)` counts
  rows by status per node, with the oldest running and the retries due.
  Time is always passed in. Every store passes `contract::check`.
- **`helios_vault::journal::VaultJournal`** keeps rows as vault documents
  in the namespace `journal/<name>`, one document per (item, node), keyed
  `item␟node`. It passes the contract on the memory and SQLite vaults. It
  panics on a storage error rather than run on rows it did not keep.
- **Gaps in the journal for this work**, found while reading it:
  1. the token counter starts at 0 in each `Journal::new`: a journal
     reopened over rows left `running` by a dead process can hand out a
     token those rows still carry;
  2. `Store` has no transaction: a claim or a conclusion over several
     items, and the omissions of a branch, are several `put`s, which a
     crash can cut in the middle (the vault itself has `begin`, `commit`
     and `rollback`);
  3. the claim's atomicity is a mutex in one process: two processes over
     the same vault would both claim;
  4. a lease cannot be renewed, so a job longer than its lease is given
     back while it still runs;
  5. there is no cancelled state, no wait settled by a signal, and no
     way to put a failed row back by hand;
  6. `monitor` and `expire` read every row on each call.
- **`helios-node`** runs a batch as **jobs**, driven from outside:
  `helios batch` is a client with no logic of its own. It starts a
  sandboxed node (or attaches to one), opens a batch job (`batch_start`,
  held on a connection so the batch fails if the client dies), and for
  each image sends `load-source` then `export` (a scheduler job holding
  its destination, the network snapshotted when asked), keeping `--jobs`
  exports in flight (2 by default). It waits on `scheduler.job_ended`,
  prints a JSON line per image, and ends the batch. Nothing is durable:
  a crash loses where the batch was, and output names are chosen against
  the files already on disk, so a re-run names them differently.
- **Control and inspect**: the control requests include `export`,
  `cancel`, `job.cancel`, `batch_start`, `batch_end`, `pause`, `resume`,
  and the decision verbs (`decisions`, `decision_resolve`,
  `decision_accept`, `decision_decide`, `decision_reask`); the inspect
  side has `describe`, `events`, `wait_event`, `subscribe` (pushed events
  with a heartbeat) and `instance`. The MCP server maps each one to a
  tool, one to one. The batch endpoint answers a subset in every build;
  the debug control endpoint exists in debug builds only.
- **The desktop's jobs relay** (`helios_gui::relay`) follows a node
  process: `describe jobs` to catch up, then `subscribe` to `job.*`,
  republished as `node:<name>/<id>`; Cancel goes back as `job.cancel`; a
  silent link marks the jobs lost, then failed after a minute; a
  restarted node (another instance) fails its old jobs.
- **The Queue panel** (branch `wip/queue-panel`) reads a `QueueSource`:
  `queue(app, node) -> Option<NodeQueue>` and `act(app, id, Action)`.
  Its interim source, `JobsQueue`, lists the jobs whose `job.started`
  names the graph node. Its item states are waiting, running, paused,
  cancelling, done, failed and cancelled; its only action is Cancel,
  because no job can be retried or moved in its queue.
- **Decision cards** (branch `wip/decision-cards`) show the decisions of
  the open image waiting for an answer, with Accept and Correct.
- **The desktop runs no batch**: exports and batches run in node
  processes (see [gui.md](gui.md)).

## 1. Which graph nodes queue

**Decision.** A graph node queues when a batch needs it once per item
and its work can fail, wait or be rate-limited on its own:

1. **the batch export first**: the graph node a batch exports (today's
   `--node`, else the network's first export, else its only end), and
   later any module of kind `sink` (a sidecar written, an upload);
2. **then decision nodes**: every module of kind `decision`. In mode
   `human` and `ai_assisted` the row waits for a person (the new
   `waiting` state, section 4); in `full_ai` it is a model call, queued
   for its rate limit and retries;
3. **then others**, when a module asks for it: whole-image analysis, AI
   object masks, anything with a declared resource the scheduler limits.
   Not in the first steps.

Process nodes never queue: they are the render level, pure and computed
on demand inside the worker that renders the export.

**What it means in the graph.** A run's journal workflow is derived from
the network when the batch is submitted:

- its nodes are the queueing graph nodes, named as in the graph, plus a
  root `submit` that every item gets a `done` row at when it enters the
  run;
- the parents of a queueing node are its nearest queueing ancestors in
  the network, through any number of process nodes (a transitive cut),
  and `submit` when it has none: `source → decide_exposure → exposure →
  export` becomes `submit → decide_exposure → export`;
- a graph node with several queueing ancestors joins on all of them;
- a decision module whose manifest declares branches (a cull keeping or
  rejecting) is a journal `choice`, its branches the queueing nodes each
  output leads to. Today's decisions (`decide_exposure`) answer
  parameters and have no branches: they conclude `done`.

**What it means in the template.** Nothing by default: whether a node
queues follows from its kind. A network node may carry an optional
`queue` object that overrides the defaults of section 4:

```json
{ "name": "export", "module": "...",
  "queue": { "concurrency": 4, "lease_ms": 120000,
             "retry": { "limit": 3, "delay_ms": 2000, "exponential": true } } }
```

`queue` on a process node is refused when the template is checked
(schema and `Bundle::build`), as is a `need` larger than the node's
queueing parents.

*Reason.* People ask "where are my 412 images" at the export and "what
waits for me" at a decision; process nodes finish within a render and
have nothing to show. Deriving the workflow keeps one network, as the
templates already promise, with no second graph to keep in sync.

## 2. How a batch becomes journal rows

**A run.** A submitted batch is a **run**: a pinned workflow (the
derived DAG with its per-node settings), the network snapshot it
renders, its export settings (destination folder, format, quality, name
pattern) and its items. Its id is the first 16 hex digits of the BLAKE3
hash of the canonical JSON of the network snapshot and the export
settings. One journal per run, named by the run id.

**An item** is one image of the batch:

- an image that comes from the library (a selection, a collection) is
  named by its vault id (the content hash, 64 hex digits);
- a file given on the command line, which the node has not hashed, is
  named `path:` and the first 32 hex digits of the BLAKE3 hash of its
  absolute path, so a submission reads no image.

An item document keeps what the run needs to do it again after a crash:
the path at submission, **the output path decided at submission** (the
name pattern resolved once, collisions numbered then; never again against
the files on disk), its parameters from a sidecar if any, its priority
and the order it came in.

**Rows.** Submitting writes, in one vault transaction, the item document
and a `done` row at `submit` for each new item. Nothing else: the rows
at the queueing nodes are written by claims, as the journal does today.
An item with no row at a node is **upstream** while its parents have not
concluded, and **waiting** once it is claimable.

**Idempotence.** Submitting the same network and settings again gives
the same run id: items already in the run are left as they are (done
stays done, failed stays failed), new ones are added, and the answer
says how many of each. Doing again what is done needs a new run
(`--again`, which salts the id with the time) or a retry (section 4).
Two files of the same content from the library are one item, written
once, and the answer says so.

**Joins and choices.** A join shows as items upstream at the joining
node until enough parents conclude; a failed or cancelled parent holds
the item there for good, which the panel counts apart ("held by a
failure upstream"). A choice concludes with a branch: the journal writes
`omitted` rows for the other branches in the same transaction, and the
panel at an omitted node counts them ("took another branch") without
listing them.

*Reason.* A per-run journal pins the workflow (graph versions are not in
the journal yet), keeps `rows()` and `monitor` bounded by one batch, and
makes forgetting a batch one namespace. Fixing the output path at
submission is what makes a resumed export write the file it would have
written.

## 3. The store

**Decision.** The vault-backed store (`VaultJournal`), in the vault the
node process opens with `--vault` (the workspace's vault, the library's).
Three namespaces:

| Namespace | One document per | Holds |
|---|---|---|
| `workflow/runs` | run | title, created, owner (node instance), state (`active`, `paused`, `finished`), format version, the pinned workflow, the network snapshot, the export settings, the next token |
| `workflow/<run>/items` | item | path, output path, sidecar parameters, priority, order, decision answers |
| `journal/<run>` | (item, node) | the journal row, as today |

A node process started without `--vault` keeps its runs in a memory
store: the batch works as today and says `durable: false`; nothing
resumes after a crash. `helios batch --vault FILE` passes the vault to
the node it starts.

**One writer per run.** Only the node process that owns a run writes its
documents (the owner is in the run document, with its instance id). The
desktop never opens a run in the vault: it reads it through the node's
inspect API (section 5). A node that opens a vault and finds a run owned
by another live instance leaves it alone; one owned by a dead instance
(no heartbeat in the run document for a minute) is taken over.

**Transactions.** `helios_journal::Store` gains `begin` and `commit`
with empty defaults (the memory store needs none); `VaultJournal` maps
them to the vault's. `claim`, `conclude` (with its omissions), `expire`
and the control verbs each run in one transaction.

**Retention.** A run is **finished** when it has no row running,
scheduled or waiting and no item claimable. A finished run is kept 30
days (`system.workflow.keep_days`), then removed whole (run, items,
rows) when the node starts and once a day after. `workflow.forget` removes
one at once. A run with failures counts as finished too: its failures
are kept as long as the rest. Nothing is kept of what is forgotten; the
audit trail stays separate (it records what changed in images, not
how batches ran).

**Migration.** Rows and documents are JSON, so the vault's SQL schema
does not change. New fields are optional (`#[serde(default)]`). A run
document carries `format: 1`; a change that old code cannot read raises
it, and a node migrates an older run when it opens it, in one
transaction (all or nothing). A node meeting a newer format than it knows
lists the run read-only and refuses to work it. A run pins its workflow
and network snapshot, so a network edited in the workspace does not
reach a run under way; moving items to a newer graph version waits for
graph versions in the journal.

*Reason.* The vault is already where the node keeps edits and settings,
with a SQLite driver, WAL, snapshots before migrations and a contract
the journal store already passes. A separate journal file would be a
second database to back up and migrate.

## 4. Workers

### Claim and conclude

A **runner** in the node process works each active run. Each tick
(every 250 ms, and at once when a row changes):

1. `expire(now)`;
2. for each queueing node, in workflow order: while the node has fewer
   leases out than its concurrency, `claim(node, 1, candidates, now)`,
   the candidates being the run's items ordered by priority then order
   of submission;
3. each lease becomes a scheduler job: an export (the render of the
   snapshot with the item's parameters, then the encode and the write)
   at priority `Batch`, holding the destination; a model call for a
   decision. The job reports as today (`helios_core::jobs`), a child of
   the run's job;
4. when the job ends, the runner concludes the lease: `done`, `failed`
   with the job's message, `skipped` (an export whose item has no
   picture to write), or a branch.

The runner and the journal decide **what** runs; the scheduler still
decides **when** (its workers, its resources such as GPU memory and AI
calls). One item per claim: exports are heavy and an item is the unit
people follow.

**Concurrency per graph node** (across the runs of a node process):

| Node | Default | Reason |
|---|---|---|
| batch export | the scheduler's workers (2 in `helios batch`, as `--jobs` today) | the export is the bottleneck and already sized by `--jobs` |
| decision, `full_ai` or `ai_assisted` | 2 | model calls are rate-limited; the scheduler's `ai` resource still caps them |
| decision, `human` | no worker | the row waits for a person (below) |
| other | 1 | until a module says otherwise |

**Waiting for a person.** The journal gains a `waiting` status and two
calls: `wait(lease, now)` turns a held row into `waiting` (no lease, no
expiry, not claimable), and `signal(item, node, outcome, now)` concludes
a waiting row. A `human` decision's worker waits at once; an
`ai_assisted` one asks the model, keeps the proposal in the item
document, then waits. `decision_accept` and `decision_decide` with an
`item` (and a `run`) signal the row; the decision cards list a decision
node's waiting items. A `full_ai` decision concludes on the model's
answer, without waiting.

### Leases given back when a worker dies

- **Through expiry.** A lease lasts `lease_ms` (60 s by default). The
  journal gains `renew(lease, now)`, and the runner renews each lease
  it holds every 20 s while its job runs: an export of a large raw
  outlives the lease without losing it. A worker that stops renewing (a
  hung job, later a remote worker) is given back on the next `expire`,
  without counting the attempt. A job that ends after its lease was given
  back writes nothing (`StaleLease`, as today); its file is written whole
  or not at all, so a second worker writing the same output is safe.
- **Through process exit.** In-process workers die with the node
  process, and nobody renews their leases. When a node opens a run (at
  start, or taking it over), it first gives back every `running` row of
  the run, as `expire` does, without counting the attempt: no lease can
  be alive across a restart. The token counter is kept in the run
  document and starts above every token written, which closes gap 1.
- **A job that fails** (an error, a panic caught by the scheduler) is
  not a dead worker: it concludes `failed`, which counts as an attempt.

### Retries with backoff

| Node | Attempts | First delay | Growth |
|---|---|---|---|
| batch export | 3 | 2 s | doubling (2 s, 4 s) |
| decision, model call | 4 | 5 s | doubling (5, 10, 20 s) |
| decision, person | 1 | | none: an answer is not retried |
| other | 1 | | none |

A failure the worker knows is permanent (a file that is not a picture,
an unsupported format, a destination refused) concludes with a new
outcome, `failed for good`, which skips the remaining attempts. A
cancelled job is not a failure (below).

### Cancel, retry, reprioritise

New control requests, on the debug control endpoint and the batch
endpoint (every build), each an MCP tool of the same name with
underscores, and a `helios workflow` CLI verb:

| Request | Arguments | Effect |
|---|---|---|
| `workflow.submit` | `title`, `items` (paths or image ids), export settings, `again` | creates or extends a run; answers its id and the counts new, already there, merged |
| `workflow.runs` | | the runs: id, title, state, counts, durable |
| `workflow.items` | `run`, `node`, `state`, `offset`, `limit` (100 by default) | one page of a node's items with their rows |
| `workflow.cancel` | `run`, optional `items`, optional `node` | the rows not concluded become `cancelled` (new status, satisfies no child); running ones have their job cancelled first; items with no row get a `cancelled` row at their first queueing node, so they are never claimed |
| `workflow.retry` | `run`, optional `items`, optional `node` | `failed` and `cancelled` rows become `scheduled`, due now, with their attempts reset: a retry by hand gets the full allowance |
| `workflow.prioritize` | `run`, `items`, `to` (`top` or `bottom`) | sets the items' priority above (or below) every other item of the run; rows already running are not touched |
| `workflow.pause`, `workflow.resume` | `run` | stops claiming for the run (running items finish) or starts again; kept in the run document, so a paused run stays paused after a restart |
| `workflow.forget` | `run` | removes a run that is not active |

`job.cancel` on a run's job is `workflow.cancel` of the run, so the jobs
centre's Cancel keeps working. These verbs close the actions the Queue
panel left out: Cancel, Retry, and Move to top / Move to bottom.

*Reason.* Claim-one and per-node concurrency mirror what `--jobs` does
today, so the first step changes nothing a user sees except that the
batch survives. Retries are for what can pass on a second try: a
transient write or a rate limit, not a broken file.

## 5. Monitoring

**What the node process exposes.**

- `describe workflow`: per run, its state, owner, `durable`, and per
  queueing node what `monitor` gives today (counts by status, oldest
  running, retries due) plus `waiting` (claimable now), `upstream`,
  `held` (behind a failure), `oldest_waiting_since`, `concurrency`,
  `leases_out`, and `now` (the node's clock in milliseconds, so a
  reader converts `since` to its own clock).
- `workflow.items`: one page of a node's items (above), each with its
  row, title (the file name), output path and error.
- Events on the node's bus, which `subscribe` already pushes:
  `workflow.rows` (the rows one write changed: run, node, items with
  status, attempts, since, due, error), `workflow.run` (a run created,
  paused, resumed, finished, forgotten).

`monitor` stops reading every row: the runner keeps the counts per
(run, node) in memory, built once when the run opens and updated by its
own writes, and `describe` reads them. A budget test checks that a tick
over 3,200 items reads no more rows than it claims (closing gap 6).

**Relayed to the desktop.** The jobs relay subscribes to `workflow.`
as well as `job.`: it catches up with `describe workflow` (counts) and
republishes the events as `node:<name>` on the shell's bus, as it does
jobs. A run is also a job in the jobs centre (its children are the
exports running now, as a batch's are today), so progress and
notifications need nothing new.

**`QueueSource`, journal-backed.** `JournalQueue` replaces `JobsQueue`
for nodes that have runs (and `JobsQueue` stays for the rest):

- `queue(app, node)`: the summary from the counts the relay keeps; the
  rows from `workflow.items`, fetched a page at a time for the selected
  node only (off the UI thread, through the relay) and kept current by
  `workflow.rows`. Several runs at a node are listed together, each
  item titled with its file name and its run;
- states: no row and claimable → Waiting; `scheduled` → Waiting, with
  "retry at" and its attempts; `running` → Running; `waiting` → a new
  `Asking` state ("Waiting for a person"); `done` and `skipped` → Done
  (a new `Skipped` state shows the difference); `failed` → Failed;
  `cancelled` → Cancelled. Upstream, held and omitted items are counted
  in the summary, not listed;
- `act`: the actions become a set: Cancel, Retry (failed or cancelled),
  Move to top and Move to bottom (waiting), Answer (asking: opens the
  decision card). `act` sends the control request through the relay.

**A remote node process.** The desktop shows what the relay last knew:

- each item's place is `Place::Node(name)`; its times are converted from
  the node's clock with the `now` of the last `describe`;
- link lost: the node's running items show Paused ("node NAME is out of
  reach"), the actions are disabled with that reason, the counts stay as
  they were with a "last seen" time. Unlike jobs, items are **not**
  failed after a minute: the journal on the node is the truth, and the
  desktop catches up when the link comes back;
- a node that restarted (another instance) is caught up from `describe
  workflow`: its runs resumed (section 7), so nothing is failed on the
  desktop's side;
- a node given no control endpoint lists its queue read-only, and the
  panel says why the actions are missing.

*Reason.* The summary must be cheap at 3,200 items, and only the
selected node's rows are worth carrying over a link. The journal being
durable is what lets the desktop stop guessing when a link drops.

## 6. What stays a job

**Decision.** A unit of work stays a plain scheduler job, without a run,
when it is **one item that someone is waiting for now**, or **not an
item at all**:

- a single export (the desktop's Export, `helios export`, control
  `export` without a run);
- one-off renders (the viewport, a thumbnail, `render`);
- the library import, the GPU calibration, model downloads: they report
  as jobs, and losing them on a crash costs a restart, not a decision.

Everything that **applies a network to a set of items**, **must survive
a restart** or **waits on a person** is a run: `helios batch`, an export
of a selection, a cull. `helios batch` becomes a client of
`workflow.submit`, keeping its command line and its JSON lines.

The line is the item count and the waiting, not the size: an export of
one 100-megapixel image is a job; three small exports of a selection
are a run. A run's workers are still scheduler jobs; the
journal adds what to run next, not another executor.

*Reason.* A job is the right tool for one interactive thing (priority
`Interactive`, no bookkeeping). A run's cost (documents in the vault)
pays only when there are many items or a restart to survive.

## 7. Failure and restart

| What | What is lost | What resumes |
|---|---|---|
| A node process crashes (or `kill -9`) mid-batch, with a vault | the exports running then (their files are written whole or not at all) | at its next start on the same vault, each run it owned is opened: running rows are given back without counting the attempt, scheduled retries keep their due time, waiting rows keep waiting, and the run goes on, **unless it was paused**; `workflow.run` says `resumed` |
| The same, without a vault | the run | nothing: as today, the client is told the batch failed |
| `helios batch` (the client) dies | nothing | the run goes on in the node it attached to; a node it started itself in a sandbox stops with it, and with `--vault` the run resumes at the next `helios batch` with the same network and settings (same run id) |
| The desktop crashes | nothing | the node process is not touched; at its next start the relay catches up from `describe workflow` |
| The link drops | nothing | the relay reconnects every second and catches up |
| The vault cannot be written | the run stops at once (the store panics, as today) | the node exits with the error; on the next start the run resumes from its last transaction |
| A worker job panics or fails | that attempt | the row is retried per its node's retry, or stands failed |

`helios batch`'s own idle timeout and held connection stay for runs in
memory; a durable run no longer needs them to avoid a batch running
forever, since it finishes when its items do and can be cancelled.

## 8. Delivery steps

Each step is one branch, building and passing its tests, merged then
released. Code is written with local checks only and finalised on the
bench box (see [bench.md](bench.md)).

1. **Journal: what a lasting run needs** (`helios-journal`,
   `helios-vault`). `Store::begin`/`commit` (the vault's transactions),
   token counter seeded from the store, `renew`, `recover` (give back
   every running row at open), statuses `cancelled` and `waiting`,
   `wait` and `signal`, `cancel` and `retry` by hand, the
   `failed for good` outcome, per-node counts kept up to date. Tests: new
   contract scenarios run on the memory, memory-vault and SQLite-vault
   stores (a crash between two puts leaves nothing: a store wrapper that
   panics mid-transaction; reopened, the tokens never collide; a renewed
   lease does not expire; a cancelled parent holds its children; a
   signal concludes a waiting row and a second one is refused; a retry by
   hand resets the attempts); a budget test that a claim reads only its
   candidates.
2. **Runs in the node** (`helios-node`). Deriving the workflow from the
   network, the run and item documents, `workflow.submit`, `.runs`,
   `.items`, `describe workflow`, the runner over the batch export with
   leases and retries, the `workflow.rows` and `workflow.run` events, and
   the run as a job. Tests (in `tests/`, through the batch endpoint, with
   deadlines): a batch of synthetic images exports each once; re-submitted,
   nothing is redone and the counts say so; an injected fault (`fault`) is
   retried then succeeds; a permanent failure is not retried; the output
   names are fixed at submission; the derived workflow of a network with a
   decision and an export.
   *As delivered (2026-10-28):* items are files (`path:` ids); image ids
   from the library wait for a library-driven batch. Until step 6 gives
   decisions workers, a decision the network had answered at submission
   concludes `done` at once and the export renders that answer; an
   unanswered one (no export could render it) turns its rows `waiting`
   for a person, holding the export. A decision that
   does not reach the exported node is left out of the run. The item
   documents are written in one vault transaction, then the `submit` rows
   in the journal's own (a crash in between leaves items to enter again,
   step 3). A run of the vault not yet opened by the process is opened on
   its next submission, its running rows given back. `workflow.items`
   and `describe workflow` name a claimable item `waiting` and a row
   waiting for a person `asking`. A failure is for good when the file cannot be read as a
   picture or does not fit the network; any other is retried. The
   `helios control` CLI has `workflow-submit`, `workflow-runs` and
   `workflow-items`; `helios workflow` comes with step 5.
3. **Survival** (`helios-node`). Opening a run at start, recovery,
   take-over of a run whose owner is dead, retention and `forget`, format
   versions. Tests in `tests/survival.rs`: `kill -9` mid-batch then
   restart on the same vault finishes the batch with each file written
   once; a paused run stays paused; a run past its retention is removed;
   a run of a newer format is listed read-only.
   *As delivered (2026-10-28):* the owner of a run is a document of its
   own (`workflow/owners/<run>`: instance, machine, process id and start
   time, heartbeat), so the run document is not rewritten every
   heartbeat. The owner beats every 10 s; it is dead when silent for 60 s,
   or at once when it ran on the same machine and its process is gone (so
   a node restarted after a crash resumes its runs without waiting a
   minute). Taking a run over checks and writes the owner in one vault
   transaction (SQLite's write lock from the first statement), so two
   nodes cannot both take it; an owner that finds another owner in its
   run at its next beat (it was silent too long) lets the run go, its
   exports stopped without writing. A node opens the vault's runs at its
   start, before it says it is ready, and looks for runs to take over
   every 5 s. A run is rebuilt over the node's own network from its
   snapshot; one submitted to another network is listed read-only, as is
   one of a newer format or one another live node works (`read_only`,
   `reason` in `workflow.runs`). Opening a run also enters the items a
   crash left between their documents and their `submit` rows. A run
   document says when the run finished (`finished`); retention counts from
   then (a run finished before this step counts from when it is first
   opened). Migrations are steps from one format to the next, applied with
   the take-over in one transaction; none exist yet (format 1).
   `workflow.forget` came with this step (control, batch endpoint, MCP
   tool, `helios control workflow-forget`), refusing a run at work or
   another live node's; a run is paused only by its document until step
   5 brings `workflow.pause`.
4. **`helios batch` over runs** (`helios-node`). The client submits and
   follows a run; its JSON lines and exit codes unchanged; `--vault`
   makes it durable; `--again`. Tests: the existing `tests/batch.rs`
   pass unchanged, plus a resumed batch.
5. **Control verbs and MCP** (`helios-inspect`, `helios-node`).
   `workflow.cancel`, `.retry`, `.prioritize`, `.pause`, `.resume`,
   `.forget` on both endpoints, their MCP tools and CLI verbs,
   `job.cancel` of a run's job. Tests: each verb through the batch
   endpoint and through the MCP server (`tests/mcp.rs`), with the rows
   checked after.
6. **Decision nodes in runs** (`helios-node`, `helios-decision`). Workers
   for decision nodes, waits and signals, answers kept per item,
   `decision_accept` and `decision_decide` with `run` and `item`, the
   template's `queue` overrides and their checks. Tests: a `human`
   decision holds the export until answered; `ai_assisted` with a
   command resolver proposes then waits; `full_ai` with a resolver that
   fails twice is retried; a `queue` on a process node is refused.
7. **The relay and `JournalQueue`** (`helios-gui`, after
   `wip/queue-panel` merges). The relay follows `workflow.`, catches up,
   and converts clocks; `JournalQueue` with paging, the new states and
   the actions. Tests (headless, through wbox for pictures): the panel
   at a node with a run of 3,200 items reads one page; Retry, Cancel,
   Move to top reach the node and the rows change; a dropped link
   pauses the items without failing them; a restarted node is caught up.
8. **Choices** (`helios-core` manifests, `helios-node`). Decision modules
   that declare branches, derived as journal choices, omitted rows shown
   as counts. Tests: a cull network sends rejected items to its other
   branch and the export counts them as omitted. Last, because no
   module declares branches yet.

Steps 1 to 5 are the batch export end to end; 6 and 7 can run side by
side once 5 is merged; 8 waits for a branching decision module.

## Questions settled (2026-10-22)

Each one was decided with its default.

1. **Where does a batch started from the desktop run?** Default: in a
   node process the desktop starts for the workspace (as `helios batch`
   starts its own), so the batch outlives the window; not in the
   desktop's own process.
2. **Without a vault, does a batch still go through the journal?**
   Default: yes, in memory (`durable: false`), so there is one code
   path; it resumes nothing.
3. **Resume after a crash: at once or asked?** Default: at once, unless
   the run was paused, with a `resumed` event and a notification on the
   desktop.
4. **Re-submitting the same batch:** default, idempotent (done stays
   done, new items added); `--again` (and a checkbox in the desktop) for
   a fresh run.
5. **Item identity for files outside the library:** default, the hash of
   the absolute path (`path:`), so submission reads no image; the
   alternative is the content hash, exact across renames but reading
   every file first.
6. **Retention of finished runs:** default 30 days, failures included,
   then removed whole; no history kept.
7. **Retry defaults:** default, export 3 attempts from 2 s doubling;
   model calls 4 attempts from 5 s doubling; a person's answer never
   retried; a manual retry resets the attempts.
8. **Concurrency of the export:** default, the scheduler's workers (2 in
   `helios batch`), set per node with `queue.concurrency`.
9. **Reprioritising:** default, "Move to top" and "Move to bottom" per
   item within a run; no numeric priorities and no ordering between runs
   (older runs first).
10. **Decision nodes in batches with `full_ai`:** default, they queue
    too (for retries and rate limits), concluding without waiting.
11. **After a link drops, do a remote node's queue items fail on the
    desktop after a minute, as jobs do?** Default: no, they stay paused
    with a "last seen" time, since the node's journal is durable.
