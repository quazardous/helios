# Diagram items: what travels, registers, and sync tags

Status: **design** (2026-11-14). The analysis plan was accepted by the
owner on that date, and the model in section 2 is the owner's, laid down
in the same thread (the latest word wins). The slices at the end wait for
the owner's go.

It builds on [workflow-level.md](../workflow-level.md) (runs, journal
rows per item and graph node, `choice`), [extractors.md](extractors.md)
(extractor blocks on carrier nodes, definitions and uses, the per-image
cache, gather), [raw-jpeg-pairs.md](raw-jpeg-pairs.md) (a photo with
several files is one item), [calibration.md](calibration.md) and
[camera-look.md](camera-look.md) (things Helios already saves).
Aggregates over a set (`max`, `mean`, `count`, rank, merges) are a
separate analysis; this note only points to it.

## In short

- **The photo is the item.** It travels in an **envelope**: the photo
  (its files), the values tied to it (extracted keys, the branch it
  took) and its **sync tag** (its identity in the run). Values stay with
  their photo by default; nothing else travels along the edges.
- **Wait rule:** a node that needs keys K for an item runs once the
  item's envelope has K, computing K itself through its own extractor
  blocks (per-image cache) or waiting for them from upstream.
- **Registers** are special nodes that **desynchronize**: they keep a
  value or a photo, detached from the item it came from. Input and
  output have the same type; a fixed manual register has no input. A
  register with an input chooses what it keeps by **extractor blocks +
  a condition + a policy** (first, last, best by a key, ask), and forgets
  by a **reset scope** (run, set, never).
- **Broadcast rule:** at a node, a desynchronized input is taken as
  valid for every item arriving on its synchronized inputs. The journal
  records which register version each item used.
- **Sync tags:** a special metadata type, the item's identity. A
  **take-sync** node outputs only the tag; a **resync** node binds a
  desynchronized value to a tag, so it travels in that item's envelope.
- **Two kinds of data:** image values (computed from the photo, a
  cache, never "saved") and saved values (registers with a diagram or
  library scope, named, with provenance and history). Calibration
  profiles, camera looks and presets read as library registers.

## 1. Use cases

The question was whether any data must travel apart from the photo, or
whether the photo plus its per-image shared state (the extractor cache,
the run journal) is enough. The baseline is: only images travel, and a
node that needs a value attaches the same extractor block and reads the
cache (no second model call).

| # | Use case | Non-image data a node needs | Where it comes from | Baseline serves it? | Model piece |
|---|---|---|---|---|---|
| 1 | Cull an import | focus, eyes open, duplicates | extractor on the photo | yes | image values |
| 2 | Tag animals: router then update metadata | `animal`, and the branch taken | extractor upstream, router | values yes (cache); the branch no | envelope (branch path) |
| 3 | Export variants, router in "all" mode | which branch this copy is on | router | no (same photo on two branches) | envelope (branch path) |
| 4 | Camera look fit on a RAW+JPEG pair | the camera file of the same photo | the photo's files | yes | envelope (files) |
| 5 | Grey card: white balance for the photos of the same light | the card's measured white balance | another photo of the set | no | value register, per set |
| 6 | Colour chart profile applied to a session | the fitted matrix | a chart shot, often an older session | no | library register (calibration profiles) |
| 7 | Exposure matching, time-lapse deflicker | the reference frame's exposure or histogram | first frame of each burst | no | value register, first per set |
| 8 | Match a series to a reference frame's EXIF (ISO, lens, focal, time) | the reference's EXIF keys | a reference frame | no | value register |
| 9 | Dark frame or flat field correction | a photo (the dark or the flat) | a shot kept by camera and ISO | no | photo register, library scope |
| 10 | A folder's theme as context for captions | one value per folder | all captions of the folder | no | gather (aggregates note), output desynchronized per set |
| 11 | The user picks the reference mid-run | the picked photo or value | the user | no | register with the `ask` policy |
| 12 | Agent-built workshop with a target look set once | a value chosen before the run | an agent or the user | no | manual register (no input) |
| 13 | Crops or panels from one photo | parent identity | a split node | no | open question (section 9) |
| 14 | Panorama, HDR, contact sheet | many photos into one result | a set | no | aggregates note |

**Findings.** Values that are a function of the photo alone (1, 4) need
nothing beyond the cache, but the branch taken (2, 3) is a fact about
the item's path, not the photo, so a minimal envelope is needed anyway.
Every cross-image case (5 to 12) needs a value detached from the photo
it came from, applied to others: that is the register. Set-level
computations (10, 14) produce the same kind of detached value, so they
reuse the broadcast and resync rules; their maths belongs to the
aggregates note.

## 2. The model (decided)

### The envelope

The queued item is the photo. Its envelope holds:

- **the sync tag** (below);
- **the photo:** its id, the version the run reads, its files with
  their roles (`master`, `camera`; raw-jpeg-pairs.md section 9);
- **tied values:** each extracted key as `use.key` (the extractor use's
  name in this diagram, then the key), with its definition `id@version`
  and a reference to its cache entry. Values are references, not
  copies: the envelope stays small and the cache stays the one truth;
- **resynced values** (section 5), each with where it came from;
- **the branch path:** the choices taken so far (`router: dog`).

The envelope only ever carries what belongs to its photo. It is stored
as the run's item document (workflow-level.md section 2), extended; it
goes with the run.

### The wait rule

A node declares the keys it reads (a carrier's condition, an alter's
template, a module parameter bound to a key). For an item, the node is
claimable when the journal's join holds **and** the envelope has those
keys:

- keys from the node's **own extractor blocks** are computed when the
  node runs, through the per-image cache;
- keys from **upstream** are already in the envelope once the upstream
  node concluded for the item; the journal's join already orders this;
- keys from a **register** are not in the envelope: they follow the
  broadcast rule (section 3).

The diagram check refuses a key read with no producer on every path to
the node, and the same `use.key` produced on two branches that meet
again with different extractor uses (same use on both: one cache entry,
no conflict).

### Branches and joins

A router in "all" mode puts the same photo on several branches: the same
sync tag, a different branch path. Where branches meet again, the item
is joined by its sync tag (photo and run), its tied values are the
union (conflicts refused at the check, as above), and its branch path
keeps both. A branch that dropped the item (a filter) is `omitted` for
it, and the journal's existing join semantics apply.

## 3. Registers

A register is a node holding a **slot** of a declared type, detached
from any item.

### Types

- **Value:** any extractor key type (`text`, `label`, `number`,
  `integer`, `bool`, multi), or a structured value (a curve, a 3×3
  matrix, a profile, a set of module parameters), declared like keys so
  carriers and modules read them the same way.
- **Photo:** a photo reference, kept with a snapshot of its envelope at
  the time it was kept (its id, version, files and tied values). A
  reader gets the photo as a side input (a dark frame, a reference for a
  fit), not as a queued item.

Input and output have the same type. A register with an input lets every
item through unchanged (it is not a filter); it only watches.

### Filling

- **By condition:** the register carries extractor blocks (the same
  blocks as carriers) and a **condition** in the router expression
  language, evaluated per incoming item:
  `chart.detected and chart.confidence > 0.9`, `burst.index == 0`,
  `keywords contains "reference"`. A value register also names the
  key (or the structured result of a block) it keeps from the matching
  item.
- **By hand:** a **fixed manual register** has no input; the user picks
  the photo or types the value in the diagram, or from the Library
  ("Use as reference…" on a photo, which lists the registers of that
  type).
- **By an agent:** over MCP (section 7), with the same checks as a hand
  edit.

### Policy and settling

Among the matching items, the **policy** chooses:

| Policy | Keeps | Settled when |
|---|---|---|
| `first` | the first matching item, in queue order | at the first match |
| `last` | the latest matching item | when its set closes (every item of the set passed the register or was omitted) |
| `best` | the highest or lowest value of a key among the matches | when its set closes |
| `ask` | the user's pick among the matches, shown as candidates | when the user picks |

`best by a key` covers the "sharpest of the burst" case with no set
aggregate in the condition. A condition that needs the whole set
(`sharpness == max(set)`) is **not allowed**: conditions are per item;
set-wide expressions belong to the aggregates note.

A register's entry is **provisional** until settled. Readers read
**settled** entries by default, so a `best` reference does not change
under the items already processed; a reader may ask for `live` (the
current content, whatever its state), and the journal shows it did.

### Reset scope and sets

- **per run:** emptied at each run's start;
- **per set:** one entry per set, the set taken from the item's sync tag
  (the input batch by default, or a group key: folder, burst, capture
  session, a key expression). A reader gets the entry of the set its
  item belongs to;
- **never:** kept across runs; a new match replaces it under the policy
  (or not at all with **lock once set**).

### Life scope, provenance, history

- **Run:** lives in the run, goes with it.
- **Diagram:** kept with the diagram (not its version), shared by its
  runs.
- **Library:** named ("Studio daylight WB", "Z50 dark 1600 ISO"), listed
  in a **Saved values** list, readable by any diagram by name, optionally
  keyed by camera, lens or light.

Every entry carries its **provenance**: the source photo (id, version)
or "typed by hand", the rule or the user or agent who set it, the run,
the extractor uses and their versions, and when. It stays valid if the
source photo is deleted (the provenance then says so). Each change makes
a new **version** of the entry; history is kept, and a reader takes the
latest or **pins** a version (a library register read by a published
workshop pins it).

## 4. The broadcast rule

At a node, every desynchronized input (a register, a gather's result)
pairs with each item arriving on the node's synchronized inputs:

- it is read when the item is claimed, as the settled entry of the
  item's set (or live, if asked);
- the journal row records, for the item, each register read as
  `register / entry (set) @ version`, so a review can tell which items
  used which reference, and a retry can choose to keep or re-read it;
- two desynchronized inputs are both broadcast: one entry of each, no
  cross product (each register gives one entry per set).

### Edge cases

- **Empty register when items arrive.** Items wait at the reading node
  (the journal's `waiting`, labelled "waiting for reference"), never
  fail. If the register can no longer fill (everything upstream of it
  concluded for that set, no match), the waiting items conclude
  `skipped` with "no reference", unless the register declares a
  **default** (a value, or its library entry from an earlier run). An
  `ask` register keeps them waiting for the user.
- **A register changes mid-run** (a hand edit, an agent, a library
  register updated by another run): items claimed afterwards read the
  new version; items already concluded keep the one they used, and the
  journal shows both. Re-running the earlier items with the new value is
  an explicit retry.
- **A register's own set is still open** under `last` or `best`: readers
  wait for it to settle (settled read) or read it live.
- **Resyncing an already synchronized value** is refused when the
  diagram is checked: the value port of a resync node accepts only
  desynchronized values. To bind an item's value to another item, pass
  it through a register first, so the detachment is explicit and
  visible.
- **A register fed by a branch the item never takes**: it fills from the
  items that do; readers on other branches read it as any other.

## 5. Sync tags

The sync tag is a special metadata type: the item's identity in the run,
with no photo and no values. It holds the photo id and version, the run,
the sets the item belongs to (batch, folder, burst, session), its
position in the queue, and its branch path. Every synchronized item and
value carries it.

- **Take-sync** node: photo in, tag out. It drives another branch per
  item without carrying the photo (a per-item counter, a log, a call to
  an external service keyed by item).
- **Resync** node: a desynchronized value and a tag in, the value bound
  to that item out. From then on it travels in the item's envelope
  (marked "resynced from register X @ version"), and obeys the wait rule
  like any tied value. Its type is the value's type plus the tag.
- A gather's per-set result, resynced by each item's tag, reaches every
  item of its set: that is how set-level results come back to items.

## 6. Saved things read as registers

The existing stores stay as they are; each is read through a **library
register view**, read-only from a diagram, edited where it is edited
today:

- **Calibration profiles:** photo-derived structured values, keyed by
  camera and illuminant; the camera default rule and the
  nearest-illuminant pick are the register's read selection. A diagram
  reads "the profile for this item's camera and light" and the journal
  records which profile id it used.
- **Camera looks:** a look fitted on a photo from its own embedded
  preview is an image value (in the envelope, no register). A look
  fitted from several photos, or a per-camera default look, is a library
  register keyed by camera.
- **Presets** (sets of module parameters): library value registers
  filled by hand.

Whether those stores later become registers in storage too (one
namespace, one history) is left open (section 9).

## 7. What the run journal and the Activity detail show

- **Per item:** its path (nodes, branches taken), the values it gained
  and from which extractor use, the registers it read with their
  versions, and why it waits ("waiting for reference: Burst exposure,
  set 12").
- **Per register:** its current entries (thumbnail or value, set,
  source, when, by what), provisional or settled, its history, and for
  `ask` the candidates with a pick button.
- **A register event** in the run's journal each time an entry changes
  (set, version, source item, rule or user or agent), so the Activity
  detail of a run lists when the reference was taken.

## 8. Agents and MCP

- `registers.list` (scope, type, name), `.get` (entries, provenance),
  `.history`, `.set` (a value or a photo, checked like a hand edit),
  `.pick` (answers an `ask` register), `.clear`.
- `diagrams.item.get {run, item}`: an item's envelope (sync tag, tied
  values, branch path, registers read).
- The journal's existing reads show register events and the versions
  each item used. An agent building a workshop declares registers in
  the diagram like any node.

## 9. Open questions (not decided)

- **Derived items** (crops, panels from one photo): child items with
  their own sync tag and a parent id, or out of scope until a node needs
  them.
- Whether the stores of section 6 move into one register storage, or
  stay as views.
- Whether a diagram-scoped register is shared across the diagram's
  versions or reset by a new version.
- The group keys that define sets (folder, burst, session, expression):
  which come first; the journal's groups are not built yet.
- Where the envelope's pure types live: a module of `helios-journal`,
  of `helios-extract`, or a small crate of their own (decided in
  slice 1).

## 10. Slices

Each slice is one branch, building and passing its tests; code checked
locally, finalised on the box. They follow extractors.md slices 5 and 6
(diagram runs over library images, routers).

1. **Envelope and sync tag types.** Pure types: sync tag, envelope
   (photo, files, tied value references, branch path), serialisation.
   *Tests:* round trip; a golden JSON; the same photo on two branches
   has one identity and two branch paths.
2. **Wait rule and diagram check.** Nodes declare the keys they read;
   claimable only when the envelope has them; the check refuses a key
   with no producer and a conflicting key on joined branches. *Tests:*
   journal contract scenarios on the memory store; check refusals named
   by node and key.
3. **Register core (pure).** Slots, types, per-set entries, keep rule
   (condition, policy `first`/`last`/`best`), settling, reset scopes,
   versions and provenance, fake clock. *Tests:* first per set, best by
   a key, last settles at set close, lock once set, reset per run.
4. **Registers in runs.** Register nodes in the journal, the broadcast
   rule, the read recorded per item, empty register waiting then
   skipped or defaulted, a mid-run change. *Tests:* a run over synthetic
   items with a fake extractor: each item's row names the register
   version it used; an empty register skips with "no reference".
5. **Take-sync and resync nodes.** Typing; resync of a synchronized
   value refused; resynced values obey the wait rule. *Tests:* a gather
   result resynced reaches every item of its set; the refusal at check.
6. **Scopes and storage.** Run, diagram and library registers in the
   vault, Saved values, history, pinned reads. *Tests:* on the memory
   and SQLite stores; a library register outlives its run and its
   source photo.
7. **By hand and `ask`.** Manual registers, the `ask` policy waiting for
   a pick, "Use as reference…" in the Library, the register's view in
   the diagram. *Tests:* GUI tests (kittest, accesskit): pick a
   candidate, the waiting items run; one undo step per hand edit.
8. **Saved things as registers.** Read views over calibration profiles,
   per-camera looks and presets. *Tests:* a diagram reads the profile
   for an item's camera and light; the journal records the profile id.
9. **Activity detail.** Item path, values, registers read, register
   events. *Tests:* kittest on a finished run: the detail names the
   reference each item used.
10. **MCP verbs.** `registers.*`, `diagrams.item.get`. *Tests:* protocol
    tests per verb; an agent scenario sets a manual register, runs, and
    reads an item's envelope.
11. **Reference use cases as templates.** Grey card white balance per
    set, burst reference exposure, dark frame by camera and ISO, with
    fake extractors. *Tests:* each template end to end through the
    node's batch endpoint, with deadlines.

Slices 1–5 need no GUI and give agents a working model; 6–9 give it to
the photographer.
