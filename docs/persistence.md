# Persistence: where an edit step goes

Status as of 2026-10-12. The first part says where each edit step is
kept now; the second is the audit made before that work, kept for its
file and line references and its reasoning (line numbers drift as the
code moves). What was fixed since is marked in its lists.

## Summary

Every edit step a user or an agent makes is now written to the vault as it
is made, in one transaction, and a restart (even after `kill -9`) resumes
from the last step saved:

- **Parameters, undo and redo** in the desktop: each committed step of the
  value store (a write outside a gesture, a gesture's end, an undo, a redo)
  is saved as a version of the image's edit (`helios_vault::EditSession`);
  a drag is saved every second while it goes (`FLUSH_SECONDS`), as a
  version labelled "in progress". `helios-gui --vault FILE --image FILE`
  opens the image at its head. Edit > Undo and Redo (`Ctrl+Z`,
  `Ctrl+Shift+Z`, `Ctrl+Y`) are egui-pop's commands answered by Helios's
  host from these steps, so each one is a step saved as a version too;
  they are held while a drag is in progress.
- **Parameters and decisions through control or MCP**: a node started with
  `--vault` and a file source keeps that image's edit; each `set-param` and
  decision answer is saved before it is answered, the decision book inside
  the version, and a restart applies the head and replays the book.
- **Ratings, rejects, colour labels**: field commands (set, set if absent,
  add or remove a label), applied by the vault to the stored value in one
  transaction; a reject is its own field, so taking it back keeps the
  stars. The desktop shows what the vault holds after each write and on
  every `vault.field_changed` event, and says why when a write fails.
- **XMP import** (`helios batch --sidecar --vault FILE`,
  `helios import-darktable --vault FILE`): a version with origin
  `import:xmp` per sidecar (base and darktable duplicates), its import
  report inside, the darktable edit as it
  came (the sidecar's text, each history item with its raw params and
  whether it was applied), and the sidecar's rating only when the vault
  has none.

Every save is announced on the bus: `edit.saved` (with the session's step
number) for a version, `vault.field_changed` for a field. Tests wait on
those, never on time.

| Edit step | Held in memory | Persisted | Granularity | Bus event | `kill -9` loses |
|---|---|---|---|---|---|
| Parameter, GUI | value store, viewport graph | a version per committed step | the edit, as a version continuing the head | `edit.saved` | the gesture going on since its last save (at most a second of a drag) |
| Parameter, control / MCP | node graph | a version per change, before the answer | the edit | `edit.saved` | nothing answered |
| Undo / redo (history move) | value store stacks | a version per move | the edit | `edit.saved` | nothing (the stacks themselves are not rebuilt on restart) |
| Module add / remove / reorder | (no runtime verb exists) | n/a | n/a | n/a | n/a |
| Decision answer (AI or human) | decision `Book` | inside the version, per answer | the book | `edit.saved`, `decision.*` | nothing answered |
| Rating / reject / colour label, GUI | library, pending commands | vault `fields`, next frame | field command | `vault.field_changed` | at most the current frame |
| Title / description | — | only through `Vault::apply`, no caller | field | `vault.field_changed` if a bus is set | n/a |
| Tags | not modelled | n/a | n/a | n/a | n/a |
| XMP import | transient value in `helios batch` | with `--vault`: a version `import:xmp`, rating if absent | version, field | `vault.version_added` | nothing kept |
| Configuration layer (node `--vault`) | — | vault `documents`, once at start | whole document | none | nothing |
| Workflow journal rows | — | vault `documents` (`VaultJournal`) | whole row | — | per statement |

**History and its end.** A version's id is the hash of its image, its
parents and its edit; an image's history is the line of first parents back
from a head (`Vault::history`), and the end of the history is that head.
An undo is a new version whose values are the earlier ones, so the
history only grows. Which head opens: the newest (latest time, then the
longest history, then the greatest id); a session then continues the
version it saved last, so two writers on one image make two heads and
neither overwrites the other.

**Quitting.** The frame that sees the close request draws nothing: it
writes what the user changed since the last frame, and the window closes
after it. The vault closes once the window is gone, on a thread of its
own with a deadline (`helios_gui::quit`); past it, the process exits
without the clean-shutdown marker, and the next opening checks the file.
Every step is already in the vault by then: nothing saved waits for the
quit.

**Survival scenarios** gate a release (`cargo xtask release` runs them
first, CI runs them by name): `cargo survival` runs the desktop's (quit
and reopen; `kill -9` in the middle of a drag; `kill -9` after a panel
moved, its workspace's layout kept) and the node's (`kill -9` after
changes answered through control; `kill -9` during an export). After
each, the history, its end, the ratings, the labels and the metadata must
equal the last saved step.

## The audit (before the fixes)

### Paths

#### 1. Parameter change from the GUI

- Entry: a slider writes through `params.rs` `write()` (~l.80) → `Store::set`
  (`helios-values/src/lib.rs` ~l.675); `param.set` commands go the same way
  (`helios-values/src/commands.rs`).
- In-memory holders:
  - `helios_values::Store` (`helios-values/src/lib.rs` ~l.435): `entries`
    (current values), `undo` / `redo` stacks, open `gestures`, `dirty` set;
    shared as `Shell::values` (`helios-gui/src/shell.rs` ~l.196).
  - The viewport's edited graph (`Edited.graph`, `helios-gui/src/viewport.rs`),
    brought up to date by `Viewport::sync` → `helios_values::sync`
    (`shell.rs` ~l.1910, `helios-values/src/graph.rs` `sync`) → `Graph::set_param`
    (`helios-core/src/graph.rs` ~l.149), plus an `Arc<Graph>` copy for the
    tile source's render thread (`helios-gui/src/engine_source.rs`). The
    view (`view/viewport/*`) is transient: never undone, never saved.
- Persisted: never. The graph comes from `show_image` (`helios-gui/src/main.rs`)
  with default parameters, unrelated to any vault image or version; the
  library selection does not load an edit either.
- Event: `param.changed` (path, old, new, origin, seq) on the in-process bus.
  No event says "saved", because nothing saves.
- `kill -9`: every parameter change and the whole undo history are lost.
  A clean exit loses them too (`on_exit` saves only the layout, the
  profile pointer and the interface's own settings).

#### 2. Parameter change from the control endpoint / MCP

- Entry: MCP tool `helios_set_param` (`helios-node/src/mcp.rs` ~l.255) is
  forwarded unchanged to the control socket; `control::answer`, arm
  `ControlRequest::SetParam` (`helios-node/src/main.rs` ~l.459) takes the
  graph write lock and calls `Graph::set_param`.
- In-memory holders: `SharedGraph(RwLock<Graph>)` (`main.rs` ~l.111) inside
  `Node` (~l.117); `Graph.nodes[].params` (`helios-core/src/graph.rs` ~l.46).
  `submit_export_now` works on a clone and never writes back.
- Persisted: never. The network JSON given with `--graph` is read once and
  not rewritten; the node does not open a vault for edits (its `--vault` is
  used for configuration layers only, `main.rs` ~l.674-735).
- Granularity: field-level in memory (one parameter per request), but each
  write re-resolves the module's whole `Params` from the manifest.
- Event: `control.action` is published *before* the change is applied, so it
  announces an intent, not a result; there is no `param.changed` on the node.
- `kill -9`: every change since the node started. The control endpoint is
  debug-only, so today this mostly affects agents and tests.

#### 3. History move / history end

- Helios has no history stack per image. The closest things are:
  - the value store's undo / redo (`Store::undo` / `redo`, ~l.824-850,
    commands `param.undo` / `param.redo`): memory only, lost with the
    process;
  - the decision `Book` history (`helios-decision/src/lib.rs` ~l.188,
    `history()` ~l.290): every answer with who / model / inputs, in memory
    in the node (`Node.book`). The type is `Serialize` and documented as
    "stored with the edit", but no code stores it. `kill -9` loses every
    answer, including model calls that cost money and time.
  - darktable's `history_end`, read by `Sidecar` (`helios-xmp/src/sidecar.rs`)
    and applied by `Sidecar::effective()`: a read, never stored.
- Version graph (`helios-vault/src/versions.rs`): `Vault::add_version` writes
  one immutable row (`INSERT OR IGNORE`) and publishes `vault.version_added`
  after the insert. It is the right home for an edit step but has **no
  caller** outside tests.

#### 4. Module added / removed / reordered

- No runtime verb exists: `Graph` has `add_module` and `connect` only, used
  when a network description is loaded (`helios-node/src/lib.rs` `load_in`).
  There is no remove, no reorder, and no control or MCP verb for either.
  Nothing to lose today; when these verbs arrive they inherit the same gap
  as parameters (the network is memory-only).

#### 5. Ratings, colour labels, tags, metadata

- Entry: shell commands `vault.rate.N`, `vault.reject`, `vault.label.<c>`
  (`helios-gui/src/shell.rs` ~l.1064-1100, actions ~l.1687-1705) →
  `Library::rate` / `toggle_reject` / `toggle_label`
  (`helios-gui/src/library.rs` ~l.179-211) → `Library::change` (~l.149).
- In-memory holders:
  - `Library.entries` (`library.rs` ~l.45): a whole-record cache (rating,
    labels) filled **once** at start by `fill_library`
    (`helios-gui/src/main.rs` ~l.48) and never refreshed;
  - `Library.changes` (~l.51): pending field changes, coalesced per
    (image, field).
- Persisted: on the next frame, `App::ui` (`main.rs` ~l.140-161) takes the
  changes and calls `Vault::apply` (`helios-vault/src/catalog.rs` ~l.103) on
  the UI thread. `apply` validates the whole batch, then for each field:
  `store.field` (read old), `store.set_field` (upsert), `store.add_audit`.
- Granularity: field-level (`fields` table keyed by image and name). Good:
  a rating write cannot touch labels. But `labels` is one field holding a
  set, so toggling one colour rewrites the whole set (see races below).
- Transactions: one per batch (since the audit). `apply` runs the image
  checks, the old-value reads, the field writes and their audit rows in one
  store transaction (`Store::begin` / `commit` / `rollback`; `BEGIN
  IMMEDIATE` in the SQLite driver), and publishes its events after the
  commit. The SQLite driver runs in WAL mode, `synchronous=NORMAL`, with an
  explicit 5 s busy timeout.
- Event: `vault.field_changed` is published after each write, but only when
  the vault has a bus; the GUI builds it with `Vault::new` (no bus), so
  nothing is announced. The catalog version used by cursors is a per-process
  counter: another process writing the same file is invisible to it.
- Failure: if `apply` fails (busy database, unknown image), the change has
  already been taken out of `Library.changes` and the grid already shows the
  new value: the change is dropped with a note, and the screen lies until
  restart.
- `kill -9`: before the next frame, the pending change (at most one frame of
  input). A batch being applied is lost whole: no field changes without its
  audit row, no part of a batch is kept.
- Tags: not modelled (`FIELDS` declares rating, labels, title, description).
  Title and description have no writer in any surface.

#### 6. XMP import

- Entry: `helios batch --sidecar` → `sidecar_params`
  (`helios-node/src/batch.rs` ~l.110) → `Sidecar::parse` + `helios_xmp::import`
  (`helios-xmp/src/import.rs` ~l.205).
- In-memory holders: `Imported` (nodes + report), then a per-image
  `BTreeMap<String, Params>` sent with the `export` request and applied to a
  clone of the graph (`submit_export_now`).
- Persisted: never. `Imported::edit()` is documented as the edit of a
  version with origin `import:xmp`, but no code calls `add_version`. The
  sidecar's `xmp:Rating` is parsed and dropped (no field write). The
  `Report` (what could not be carried over) is printed on stdout only.
- `kill -9`: nothing durable is lost because nothing durable is made; the
  next run re-imports from the sidecar.

#### 7. Other durable writes (for completeness)

- Configuration layers: `Vault::set_config_layer` (`helios-vault/src/settings.rs`)
  writes one whole TOML document, once, when a layer file is first imported
  (`helios-node/src/main.rs` ~l.704). Whole-document, last writer wins.
- Workflow journal (`helios-vault/src/journal.rs`): one document per
  (item, node), whole-row writes, one statement each; the interface panics
  on storage errors rather than continuing on unkept rows.
- GUI layout, workspace profile pointer: written in `on_exit` only; a kill
  loses the session's layout changes (fixed for the layout: saved as it
  changes). Recent workspaces: written at start.
- The interface's settings (window place, focus mode, the user's keys),
  kept by egui-pop: two versioned documents (`settings`, `keymap`) under
  `~/.config/helios/ui/`, written whole in `on_exit` only.
- The bus journal (`helios-core/src/bus.rs` ~l.45) is a bounded in-memory
  ring, not a durable log.

### Read-modify-write sequences that can erase a concurrent write

1. **Colour labels toggled from a stale cache** (fixed: `label.add` and
   `label.remove` commands, `Op::Add` / `Op::Remove`) (`Library::toggle_label`,
   `library.rs` ~l.198). The new set is computed from `Library.entries`,
   read once at start, and written whole. A label added meanwhile by another
   process (a node, a script, a future XMP sync) is erased. Exactly
   darktable's "metadata wiped on refresh" shape. Fix: label add / remove
   commands applied by the vault on the stored set, or a compare-and-set on
   the field.
2. **Reject toggled from a stale cache** (fixed: `rejected` is its own
   field; schema migration 3 moves a stored `-1`) (`toggle_reject` ~l.187): decides
   between `-1` and `0` from the cached rating, and un-rejecting writes `0`,
   which erases the star rating the image had before (reject and rating
   share one field). darktable separated the reject flag for this reason.
3. **The GUI never refreshes after its own failed write** (fixed: the
   touched images are read back after every write, kept or refused): the shown value
   and the stored value diverge; the next toggle starts from the shown
   value and writes it back.
4. **`Vault::apply` read-then-write** (fixed since the audit): the old
   value is read inside the batch's write transaction, taken at once, so
   another process cannot replace it between the read and the write; a
   test runs two vaults on one file and checks that the audit trail is one
   chain.
5. **Parameter writes re-resolve the whole module** (saves now go
   through versions, so the two copies become two heads) (`Graph::set_param`
   clones `params`, sets one, resolves all): harmless today because the
   graph lock serialises writers, but `submit_export_now` clones the graph
   and `helios_values::sync` pushes only dirty paths, so a parameter changed
   on the node by an agent and the same module edited in the GUI are two
   unrelated copies; whichever one is saved first (once saving exists)
   will overwrite the other unless saves go through versions.
6. **Configuration layers** are whole documents: two writers of the same
   layer (two nodes on one vault) lose each other's keys. Low risk today
   (written once).
7. **Version heads** are safe by construction: an edit never overwrites,
   it adds a version with parents; concurrent edits become two heads.

### Where each step must be persisted

1. **Make `Vault::apply` one transaction** (done). Add a transaction (or a
   `Store::apply_batch`) to the store contract: old-value read, field
   write and audit row commit together; the whole batch commits or none
   does. Publish `vault.field_changed` after commit only. Set WAL and an
   explicit busy timeout in the SQLite driver.
2. **Give the GUI's vault a bus, and refresh the library from it** (done,
   except moving the write off the UI thread). Apply
   `vault.field_changed` from any writer to `Library.entries`; on a failed
   `apply`, put the shown value back and keep the change for a retry.
   Move the write off the UI thread through the scheduler.
3. **Replace whole-set label writes by commands** (done) (`label.add`,
   `label.remove`) applied by the vault on the stored set, and split reject
   from rating (or make reject a separate field) so un-rejecting restores
   the stars.
4. **Persist an edit as a version on every committed step** (done; the
   "working" ref is the session's own last version, not stored). Bind a surface
   to an image: on open, load the image's head version into the graph; on
   each gesture end, undo / redo, control `set_param`, decision answer and
   (later) module add / remove / reorder, write `Version::new(image,
   [head], edit, origin, now)` with `add_version`. Coalesce a drag into its
   gesture's end; the version id is content-addressed, so a retried write
   is idempotent. A durable "working" ref (the current head per surface)
   lets a restart resume exactly where the kill happened.
5. **Store decisions with the edit** (done). Serialise the `Book` into the version
   (as its doc says), so an answer, its model and its inputs survive.
6. **Turn an XMP import into a version** (done, the report inside the
   version's edit rather than a separate document) with origin `import:xmp`, keeping
   the `Report` as a document, and write the sidecar rating as a field
   command (only if the vault has none, never overwriting a user rating
   with a sidecar's).
7. **Save the GUI layout on change** (done: once it changed and no
   button is held, egui-pop's `layout.save` step), not only in `on_exit`.

### Tests to write

Status: the survival scenarios run in `helios-gui/tests/survival.rs` and
`helios-node/tests/survival.rs` (`cargo survival`); the field-write
contract tests in `helios-vault/src/contract.rs` and
`helios-vault-sqlite/tests/durability.rs`. Not written yet: the decision
scenario with a model (the node's covers a human answer), the XMP one
with a kill (the batch test covers the import), the fault injection
between `set_field` and `add_audit` (covered by the audit-row failure),
and the shared catalog version for a second handle's cursor.

Survival scenarios (spawn the real process, `kill -9`, reopen):

- GUI: rate an image, kill after the next frame, reopen: rating present,
  one audit row. Kill before the frame: the rating is absent and nothing
  else changed.
- GUI: drag a slider, release, kill, reopen the same image: the parameter
  is the released value (needs step 4).
- Node: `set_param` over control, `kill -9`, restart on the same vault:
  the head version carries the value.
- Decision: accept a model's proposal, kill, restart: the answer, its model
  and its history are there; the model is not called again.
- XMP: import a sidecar, kill, reopen: a version with origin `import:xmp`
  exists with the report.
- Fault injection in the store (contract suite): fail after `set_field`
  and before `add_audit`; the field must be unchanged (no write without its
  audit). Fail in the middle of a two-field batch: neither field changed.

Field-write contract tests (in `helios-vault/src/contract.rs`, run by every
driver):

- Two vault handles on one file (two processes for SQLite): A writes the
  rating, B writes labels; both survive, each with its own audit row.
- A label added by B is not erased when A toggles another label from a
  stale read (fails today through `toggle_label`).
- Reject then un-reject keeps the previous star rating.
- A decode job and a rating command race on the same image; the rating
  survives in every store (the darktable#22569 scenario).
- An equal write produces no row, no audit, no event; a failed batch
  produces no event.
- `vault.field_changed` is published only after commit, and a second
  handle's cursor sees the change (catalog version shared through storage,
  not a per-process counter).
- Writing the same version twice is one row and one event; two edits on
  the same parent give two heads, neither overwritten.
