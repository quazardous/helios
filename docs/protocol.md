# The node protocol

A Helios node (`helios-node`) serves three endpoints: **inspect**
(`--socket`, read-only, every build), **control** (`--control`, debug
builds with `HELIOS_DEBUG_CONTROL` set) and **batch** (`--batch-control`,
every build, the verbs a batch needs). Each is a Unix socket, or loopback
TCP behind a session token (`{"auth": "<token>"}` as the first line).
An endpoint serves at most 64 connections at once: one more gets
`{"ok": false, "error": "too many connections: …"}` and is closed.

All three speak **JSON-RPC 2.0**, one JSON object per line in each
direction. Why JSON-RPC, and what other projects do, is in
[design/node-protocol-research.md](design/node-protocol-research.md).

## Calls

```json
{"jsonrpc": "2.0", "id": 1, "method": "workflow.runs", "params": {}}
{"jsonrpc": "2.0", "id": 1, "result": {"seq": 12, "runs": [], "now": 1763000000000, "command": "workflow.runs", "origin": "control"}}
```

- **Method**: on the control and batch endpoints, a command's id, as
  `commands.list` lists it (`export`, `workflow.submit`,
  `library.import`); on the inspect endpoint, the inspection requests
  (`state`, `describe`, `events`, `wait_event`, `wait_idle`).
- **Params**: an object, by name, as the command's schema in
  `commands.list` describes it; checked against it before the command
  runs. `_meta.origin` names who runs a command (`user`, `cli`,
  `control`, `midi`, `agent:NAME`, `script:NAME`, `remote:NAME`;
  `control` when not given): a change to the edit is saved under it.
  `_meta.request_id` is the call's key: sent again with it, the call is
  applied once ([Request ids](#request-ids)).
- **Id**: a number or a string. Each call runs on a thread of its own, so
  one connection carries several calls at once; answers come in the order
  the calls end, matched by id. A call without an id is a notification:
  it runs, and nothing is answered.
- Batches (an array of calls) are not served yet: send one call per line,
  or several commands as one `command.batch` ([Command batches](#command-batches)).

## Protocol methods

| Method | Endpoint | What it does |
|---|---|---|
| `node.hello` | all | `protocol` (semver), `oldest` (the oldest version still served), `instance` (the node's, the same on its three endpoints; a restarted node has another), `endpoint` (`inspect`, `debug`, `batch`), `permissions` (`inspect`; `batch`; `batch` and `debug`), `server` (name and version) |
| `commands.list` | all | the capability list: on control and batch, every command with its schema, needs, `scope` (`node`), and whether it runs here, with `disabledReason` when it does not (`filter`, `enabled_only`); on inspect, its methods |
| `command.run` | control, batch, a vault's | `{id, params, origin}` (and on a desktop's, `window`, `scope`): runs a listed command by its id, its params checked against its schema, a greyed one refused with its reason; the same as calling the id as a method |
| `command.batch` | control, batch, a vault's | `{steps, stop_on_error, window, scope, origin}`: runs several commands in one call, each step as `command.run` runs it, and answers each step's result in order ([Command batches](#command-batches)) |
| `events.subscribe` | inspect | `{prefix, since}`: answers `{subscription, last_seq, gap, instance}`, then sends the events whose kind starts with `prefix` as notifications on the same connection |
| `events.unsubscribe` | inspect | `{subscription}`: stops it |
| `$/cancel` | all | `{id}`: stops a call still running |

Changes to the protocol are additive within a major version: a new
method, a new optional field. A client reads `node.hello` to know what it
talks to.

## Init steps

The setup steps modules declare (the library folder first, see
[module-contract.md](module-contract.md#init-steps-what-a-module-asks-first))
are read on a vault's endpoint:

- `init.steps`: every step in order, each with its `id`, `owner`,
  `title`, `why`, the settings it `fills`, its `command` when one fills
  it, `required`, its `done` test (`{"set": [settings]}`) and the steps it
  comes `after`; `requires` lists, by module, the steps it needs.
- `init.status` (`module`, optional): each step with `done`, and
  `missing`, by module, the required steps (with their dependencies) not
  done yet. An unknown `module` is refused.

- `init.set` (`values`: setting to value): fills steps by setting the
  settings they fill, each checked as its keeper does
  (`{"values": {"library.folder": "/home/me/Pictures/Helios"}}`: a full
  path, made if missing, written in, with room left); answers the steps
  with `done`. A setting no step fills, or a value refused, is an error.

An agent fills a step with `init.set`, or by running its command
(`init.libraryFolder` with a `path`, on a desktop), then reads
`init.status` again.

## Events

After `events.subscribe` answers, the node sends, on the same connection
and between the answers to other calls:

```json
{"jsonrpc": "2.0", "method": "events.event", "params": {"subscription": "…", "event": {"seq": 13, "kind": "job.ended", "payload": {}}}}
{"jsonrpc": "2.0", "method": "events.heartbeat", "params": {"subscription": "…", "last_seq": 13}}
```

The journaled events after `since` come first, then the new ones as they
are published. `gap` true says the journal no longer holds every event
after `since`: read the state again. A heartbeat comes after a second
without an event, so a client sees a dead link.

## Cancellation

`$/cancel {"id": N}` asks call `N` to stop. `wait_event` and `wait_idle`
stop at once and answer error `-32800`. A call that runs to its end
(every command so far) is answered `-32006` (not cancellable); a call no
longer running is answered `{"cancelled": false}`. Sent without an id,
`$/cancel` is a notification and answers nothing.

## Errors

```json
{"jsonrpc": "2.0", "id": 4, "error": {"code": -32001, "message": "only the debug control endpoint runs it (helios-node --control, debug builds)", "data": {"message": {"key": "node-command-debug-only", "english": "…"}}}}
```

`message` is English; `data.message`, when there, is the keyed message
(`{key, args, english}`) for a client to show translated. `data` also
carries what the older form's answer carried beside its error (the last
events seen on a wait's timeout, the entities known on a miss).

| Code | Meaning |
|---|---|
| -32700 | the line is not JSON |
| -32600 | not a valid request (wrong `jsonrpc`, a batch, an id still running, a request id given to another call) |
| -32601 | no such method or command |
| -32602 | params do not match the schema |
| -32603 | internal error |
| -32000 | the command ran and refused or failed |
| -32001 | the command is not served on this endpoint |
| -32002 | the command needs a vault, and the node keeps none |
| -32003 | the command needs a desktop window |
| -32004 | the origin is not one |
| -32005 | a wait ran out of time |
| -32006 | the call cannot be cancelled |
| -32007 | no such cursor: closed, or its lease ran out |
| -32008 | outcome unknown (set by the client, never sent by a node) |
| -32800 | the call was cancelled |

**Outcome unknown, never resend.** When the connection closes after a call
was sent and before its answer came, the client answers the call -32008
itself: the node may have run it. A client reads the state (the image's
parameters, the command's own list or state) before sending it again, and
never resends a change blindly. A call made on a connection already
closed was not sent (-32603). A call sent with a request id may instead be
sent again with the same one.

## Request ids

A client may give a call that changes something a key of its own,
`_meta.request_id` (a string of 1 to 128 bytes, unique to that call):

```json
{"jsonrpc": "2.0", "id": 7, "method": "command.run", "params": {"id": "theme.select", "params": {"name": "helios-light"}, "_meta": {"origin": "cli", "request_id": "3f9c0d2e8a41b7c6d5e4f3a2b1c0d9e8"}}}
```

Each endpoint remembers the keys of the calls it answered, with their
answers. When a key comes again, on any connection, for the same call (its
method and params, `_meta` aside), the node does not run it again: it
answers the remembered result, with `"already_applied": true` added (a
result that is not an object is held in `result`). So after an outcome
unknown (-32008), the client sends the call again with the same key and
reads from the answer whether it had already run.

- A key given again with another call is refused (-32600): a key names
  one call.
- A call that failed (an error answer) is not remembered: it changed
  nothing, and sent again it runs again.
- A key that comes again while its call still runs waits for that call
  and answers its result.
- The store is bounded and time-limited: an endpoint keeps at most 1,000
  keys, the oldest answered forgotten first, each for 10 minutes. A key
  sent again after that runs the call again: resend soon, or read the
  state.
- The older form takes the key too, as `"_meta": {"request_id": …}` on
  the line; an answer `"ok": true` is remembered, and given again with
  `"already_applied": true`.
- An in-process client (`Client::in_process`) calls the service directly:
  there is no connection to lose, and keys are not looked at.
- A step of a command batch takes its own key (its `request_id`), looked
  at as its `command.run`'s would be ([Command batches](#command-batches)).

`helios-mcp` sends a key with every tool that changes something (the
control tools that write, `helios_command_run`, `helios_command_batch`), a new one unless the
agent gives `request_id`, and names it after an outcome unknown. The `helios` CLI does
the same with `--request-id` ([cli.md](cli.md)).

## Command batches

`command.batch` runs several commands in one call: each step is
`{id, params, window, scope, request_id}` (only `id` required), run in
order as `command.run` would run it, by the batch's origin (`origin`,
else `_meta.origin`). A step without its own `window` or `scope` takes
the batch's.

```json
{"jsonrpc": "2.0", "id": 9, "method": "command.batch", "params": {"steps": [{"id": "theme.select", "params": {"name": "helios-light"}}, {"id": "library.stacks.dissolve", "params": {}}, {"id": "library.stacks.list"}], "_meta": {"origin": "cli"}}}
{"jsonrpc": "2.0", "id": 9, "result": {"steps": [{"id": "theme.select", "ok": true, "answer": {…}}, {"id": "library.stacks.dissolve", "ok": false, "error": {"code": -32602, "message": "…"}}, {"id": "library.stacks.list", "skipped": true}], "ran": 1, "failed": 1, "skipped": 1, "cancelled": false, "atomic": false}}
```

- **One result per step, in the order sent:** `ok: true` with its
  `answer`, `ok: false` with its `error` (as a call's), or
  `skipped: true`.
- **`stop_on_error`** (true when not given): the first failed step stops
  the batch, the steps after it are skipped. False, every step runs
  whatever the others did.
- **Not atomic** (`"atomic": false`): the steps that ran before a failed
  one stay applied. Read the answer to know which.
- **At most 256 steps;** more, an empty list or a step without `id` is
  refused (-32602) before any step runs.
- **Cancelled** (`$/cancel`): the running step ends, the rest are
  skipped, and `cancelled` is true.
- **Keys:** a step's `request_id` is its run's key, so a step sent again
  in a later batch with the same key is applied once and its result says
  `already_applied`. The batch may carry a key of its own
  (`_meta.request_id`): sent again, the batch's whole answer comes back.
  After an outcome unknown (-32008) on a batch, send it again with the
  same key, or read the state; never resend it blindly.
- The batch's answer is a success even when a step failed; a failed step
  is in its result, not in the call's error.

## Cursors

The library's photos are read through cursors on the vault's endpoint
(`VaultService`: the desktop's node, in-process clients), so a client
never pulls a whole collection.

| Method | Params | Answers |
|---|---|---|
| `cursor.open` | `query`, `lease` (seconds, 60 by default, at most 3600) | `cursor`, `count`, `version`, `lease` |
| `cursor.window` | `cursor`, `offset`, `len` (at most 10,000), `fields`, `locations` | `rows` (`id`, `fields`, and `locations` when asked), `count`, `version`, `current`, `stale` |
| `cursor.rank` | `cursor`, `image` | `rank` (null when not in it) |
| `cursor.refresh` | `cursor` | `changed`, `count`, `version`, `inserted`, `removed`, `replaced` |
| `cursor.close` | `cursor` | `closed` |

- **A snapshot.** `cursor.open` runs its query once and keeps the
  ordered ids on the node. Windows page that snapshot: a write made
  during the paging never shifts the pages, and each photo is read once.
  The fields of a window are read when it is read.
- **Stale, not wrong.** A window says the version its snapshot was taken
  at and the catalog's (`current`); `stale` when they differ. The client
  then calls `cursor.refresh` (the photos inserted and removed, and those
  whose copies moved) or reopens the cursor.
- **A lease.** A cursor not read for its lease is closed by the node;
  reading it then answers -32007, and the client reopens it from its
  query. `cursor.window` is cancellable (`$/cancel`): it stops between
  chunks of rows and the cursor stays open.
- **The query**, every term optional (none: the whole Library by id):
  `min_rating`, `label`, `labels`, `flags` (`pick`, `reject`, `none`),
  `tags`, `captured_from`, `captured_before`, `words`, `cameras`,
  `lenses`, `file_types`, `folder`, `album`, `presence` (`present`,
  `offline`, `missing`), `cull`, and `order` (`{"by": "captured",
  "descending": true}`; `by` one of `captured`, `imported`, `name`,
  `rating`, `edited`, `size`, `focus`, `id`), `rules` (a smart album's
  rule tree) with `now` (the clock its relative dates resolve against),
  and `backup` (`{"rule": …, "states": ["met", "short", "unverified",
  "none"]}`, the rule as `library.facets` takes it).
- **Locations.** A window asked with `"locations": true` gives each row
  where its copies are: `path`, `size`, `modified`, `last_verified`,
  `volume`, `rel_path`, `presence` (`present`, `offline`, `missing`), as
  the scans stored them; no file is read.

`library.facets` (`query`, `rule`) counts the photos of a query by facet
(`facets`: each facet, each value, how many photos); `rule` is the backup
rule the `backup` facet counts against (`{"volumes": [{"id", "drive",
"off_site"}], "copies", "off_site_required"}`), none: no `backup` facet.
`library.changed.list` lists the photos another application changed
(`images`: `image`, `at`), until acknowledged.

`library.fields.set` (`changes`: `{image, field, value, op}`) writes
photos' fields, audited under the call's origin and announced as
`vault.field_changed`. `op` is `set` when not given (a null value
clears the field), `set_if_absent` (only an empty field), `add` or
`remove` (the name `value` put in or taken out of a set field, such as
the colour labels, the others kept). `library.fields.get` (`images`,
`fields`, `locations`) reads those fields as stored: `rows` (`id`,
`fields`, and `locations` when asked), in the order asked.

`library.images.remove` (`images`) takes photos out of the Library
(Lightroom's "Remove from Catalog"): their Library flag only, set by
the call's origin; their files, fields, versions and sightings stay,
and an import finds them again by their hash. It answers `removed` (how
many were in it) and `changes`, the step as the vault records it, which
its revert puts back: the desktop's Ctrl+Z. Announced as
`vault.library_changed`, as an undo or a redo of a library step is.

`library.images.rotate` (`images`, `turn`: `left`, `right` or `reset`)
turns photos a quarter counter-clockwise or clockwise, or back to their
file's EXIF orientation (Lightroom's "Rotate Left" and "Rotate Right",
darktable's "reset rotation"): their `rotation` field (quarter turns
clockwise, on top of the file's orientation), set by the call's origin
with its audit row; no file is touched and no pixel decoded. A photo
named twice turns once, and a reset leaves an unturned photo out. It
answers `rotated` and `changes`, the step its revert undoes (the
desktop's Ctrl+Z), and announces each turn as `vault.field_changed`.

`library.images.duplicate` (`images`, `from`: `current`, the default,
or `original`) makes a new version of each photo, a new head beside the
others (Lightroom's "Create Virtual Copy", darktable's "duplicate" and
"duplicate virgin"): a copy of its newest head's edit, continuing the
same versions (it shares their history), or a first edit with no
modules. The copy's edit carries a number under `duplicate`, so two
copies of one edit stay two versions; it is labelled `Duplicate`, made
by the call's origin, and, being the newest head, is the one an edit
opens next. A photo named twice is copied once. It answers `duplicated`,
the new version of each photo (`versions`) and each photo's head count
(`heads`), and announces each copy as `vault.version_added`. Removing a
copy is not built yet. `library.versions.heads` (no params, read-only)
answers the head count of every photo with more than one, for the
grid's versions badge:

```json
{"id": 7, "method": "library.images.duplicate",
 "params": {"images": ["4f1c…"], "from": "current"}}
{"id": 7, "result": {"duplicated": 1, "versions": {"4f1c…": "9a02…"},
 "heads": {"4f1c…": 2}}}
{"id": 8, "method": "library.versions.heads", "params": {}}
{"id": 8, "result": {"heads": {"4f1c…": 2}}}
```

`library.images.trash` (`images`, `plan`, `job`) moves photos' files to
the system trash (Lightroom's "Delete from Disk", darktable's "delete
(trash)") as a journaled job (`helios_library::trash`). The job is kept
as the library document `trash/<job>` before any file moves: each file
with its outcome (`pending`, `trashed`, `refused`, `failed`, `missing`,
with the `reason`), written as it goes. Files on a memory card, on a
backup or archive volume, in a `Helios Backup/` or `Helios Archive/`
folder, or on a volume not plugged in are refused, with the reason. The
trashed files' locations are marked missing, and a photo with a file
trashed and none left to do leaves the Library (`vault.library_changed`;
not a library step: the system trash is the undo). It answers `job`,
`trashed`, `missing`, `refused` and `failed` (the files, with their
`reason`) and `removed`. With `plan`, it only says what it would do:
`images`, `files` (how many would go), `folders` and `refused`. With
`job`, it resumes a job kept: its pending and failed files.
`library.trash.jobs` (`job`) reads the jobs kept, newest first (`jobs`),
or one (`job`).

`library.documents.get` (`ns`, `key`) reads a library document
(`document`, null when there is none), such as the Library's pinned and
recent filters; `library.documents.set` (`ns`, `key`, `document`) keeps
one, a null `document` removing it. `library.folders` (`query`, the
whole Library when none) counts the photos by folder: `folders`, one
entry per volume (`volume`, null for files on no known volume) with its
`folders` (each folder's path and how many photos it holds).
`library.workshop.sessions.list` lists the workshop sessions' records
(`sessions`: `id`, `record`), and `library.workshop.sessions.set` (`id`,
`record`) keeps one, a null `record` dropping it.

`library.albums.list` reads the albums, smart albums and album sets
(`albums`: `kind`, `id`, `name`, `images`, a smart album's `rules`, the
`set` it is in, and for a smart album or a set the `count` of photos it
selects) and the quick collection (`quick`, image ids).
`library.albums.edit` makes one change, named by `op`: `create` (`id`,
`name`, `images`), `rename` (`id`, `name`), `delete` (`id`, `members`:
a set's albums forgotten too), `createSet` (`id`, `name`, `set`),
`move` (`id`, `set`, null for the top), `add` and `remove` (`id`,
`images`), `createSmart` (`id`, `name`, `rules`), `rules` (`id`,
`rules`), `quickToggle` (`images`), `quickClear` and `quickSave` (`id`,
`name`, `clear`). It answers what the step changed, as an undo history
keeps it (`changes`: the `images` with their rows `before` and `after`,
and the `documents` with theirs), the album's `name` (the new one for a
creation, the one it had otherwise), the `count` of photos the step put
in or took out and, for `quickToggle`, whether it put them in
(`added`).

```json
{"jsonrpc": "2.0", "id": 4, "method": "library.albums.edit", "params": {"op": "rename", "id": "trip-2024", "name": "Summer 2024"}}
{"jsonrpc": "2.0", "id": 4, "result": {"name": "Trip 2024", "count": 0, "added": true, "changes": {"images": [], "documents": [{"ns": "library/collections", "key": "trip-2024", "before": {"kind": "album", "name": "Trip 2024", "images": []}, "after": {"kind": "album", "name": "Summer 2024", "images": []}}]}}}
```

`library.changes.apply` walks such `changes` back (`forward` false: an
undo) or again (`forward` true: a redo), all or nothing, and answers
what it did: images `removed`, `restored`, `changed`, and `kept` as they
are (changed since, or gone).

`library.storage.get` reads the library's backup strategy (`strategy`,
null when none is set) and the volumes it knows (`volumes`, by name):
the desktop copies a catalogue snapshot to the strategy's disks plugged
in with them.

The Storage page reads and writes through `storage.*`, each on the
node's vault:

- `storage.volumes` reads the volumes the library knows (`volumes`, by
  name) and how many copies of photos each holds (`photos`, by volume
  id): `{"volumes": [{"id": "uuid:a", "name": "Backup", …}], "photos":
  {"uuid:a": 120}}`.
- `storage.statuses` reads every photo's backup status against the
  library's strategy, by image id: `{"statuses": {"0a…": {"have": 1,
  "want": 1, "off_site_ok": true, "unverified": 0}}}`.
- `storage.volume.save` (`volume`, as read) keeps its name, roles, off
  site and RAID flags and answers the volume kept;
  `{"volume": {"id": "uuid:a", "name": "Backup"}}`.
- `storage.volume.mark` (`volume`, `mount`, `library`) writes the
  identity file on a volume plugged in at `mount` and moves its record to
  the marker's id: `{"volume": {"id": "label:B"}, "mount": "/media/b",
  "library": "/photos"}`.
- `storage.strategy.set` (`strategy`, null removes it) keeps the
  library's backup strategy and answers `{"set": true}`.
- `storage.card.safety` (`root`, `path`, `label`, `files`: `[path, image
  id or null]` for each file read on the card) answers the card with
  `safe_to_format` computed now: `{"root": "/photos", "path":
  "/media/card", "label": "EOS_DIGITAL", "files":
  [["/media/card/DCIM/a.cr3", null]]}`.

The desktop sends the writes from a thread of their own, in the order
made, then reads `library.storage.get`, `storage.volumes` and
`storage.statuses` again; a frame shows what was last read.

`calibrate.offer` (`camera`, and `image` when a photo is open) reads
what a photo's calibration header offers: the camera's `profiles` as
`calibrate.profiles.list` lists them, its default camera look's
`look_default` (`preview`, null without one) and the file the image is
read from (`path`, null when none is at hand). `calibrate.open`
(`image`, `camera`, `light`: the as-shot light in CIE xy, or null) is
what the desktop calls as a photo opens: a raw never edited gets its
camera's default profile, saved as its first edit by the call's origin.
It answers the `offer` and the `default`: its `outcome`, `applied`
(with the `profile` and its `name`), `refused` (with the `reason` and
its `key`: no as-shot light, the nearest light too far), `edited` (the
photo has an edit already) or `none` (no default for the camera).

`library.edit.head` (`image`) reads an image's newest edit: `head`, the
whole version (`id`, also as `version`, `image`, `parents`, `edit`,
`origin`, `at`, `label`, `meta`), null for an image never edited.
`library.edit.add` writes one step of an edit session a client keeps on
its side (its versions' ids are computed from their content):
`version`, in the form `head` has, `step`, the session's step number,
and `inProgress`, a step saved during a gesture. It refuses a version
whose parents are not written, and announces the step as `edit.saved`.
`library.snapshots.get` (`image`) reads an image's snapshots, oldest
first (`snapshots`: `id`, `name`, `origin`, `at`, the `version` each is
pinned to, null for the image with no edit, and that version's `edit`,
or an `error` when it cannot be read); `library.snapshots.set` (`image`,
`snapshots` in that form, without `edit`) replaces them, an empty list
removing them.

```json
{"jsonrpc": "2.0", "id": 5, "method": "library.edit.add", "params": {"version": {"id": "9f…", "image": "0a…", "parents": ["41…"], "edit": {"modules": []}, "origin": "user", "at": 1700000000, "label": "set exposure"}, "step": 2, "inProgress": false}}
{"jsonrpc": "2.0", "id": 5, "result": {"image": "0a…", "version": "9f…"}}
```

The desktop's library writes its fields, its stacks' expansion, its
filter history, its workshop sessions' records, its albums, its photos
taken out, the steps of the edit shown and its snapshots this
way, on an in-process client of its embedded node: the calls run off
the UI thread, one write after the other in the order made, and their
answers are picked up at the next frames. Its grid reads the same way:
the photos through a cursor kept open for an hour (refreshed when the
catalog moves, read whole again when its sort changes or the cursor is
gone), the search filter's photos through a cursor of their own and its
counts through `library.facets`, each read on a thread of its own and
shown at a coming frame, through a `ReadCache` fed by the bus's events.
The Folders section's counts and the filter history are read through
the node too, and shown at the frame their answer comes.

### Worker reads

The desktop's worker threads read through the node too, never the vault,
each call made on the thread that needs it (an export's render, a
thumbnail of a head version, an import window's scan):

- `library.version.get` (`version`) reads one version, in the form
  `library.edit.head` answers it, null when there is none of that id:
  an export renders each photo's version so.
- `export.plan` (`images`, `settings`: the export's format, folder,
  pattern and clash rule; `node`, the graph node rendered) plans an
  export over the library and answers its `request`: each image's file,
  its head version and its output name, decided now.
- `import.known` reads what an import's prefilter compares a card with:
  the library's images as `partials` (`[size, partial hash]`) and their
  present copies as `present` (`[image, path, size, modified]`).
- `import.recorded` (`folder`) reads what the image map saw under a
  folder whose volume is offline: the image files' `sightings`, on the
  known volume last mounted above it, and that `mount` (null when the
  folder is on no known volume).

```json
{"jsonrpc": "2.0", "id": 6, "method": "import.recorded", "params": {"folder": "/media/card/DCIM"}}
{"jsonrpc": "2.0", "id": 6, "result": {"mount": "/media/card", "sightings": [{"volume": "uuid:a", "rel_path": "DCIM/IMG_0001.CR3", "path": "/media/card/DCIM/IMG_0001.CR3", "size": 24000000, "modified": 1700000000, "partial": "5e…", "image": null, "taken": 1699990000, "camera": "Canon EOS R6", "presence": "present", "seen_by": "import"}]}}
```

A thumbnail of a photo's head version reads `library.edit.head` through
the desktop's read cache.

## Promises

A long piece of work answers at once with a **promise**: a handle to its
result to come, not the result. The caller (a window, the CLI, an agent
over MCP) keeps the id, may go away, and reads the result once the
promise is settled. Promises are durable and shared: the vault keeps
them (they survive a restart), and every client of the node sees them.

```json
{"id": "promise-7", "owner": "import-promise-7", "kind": "import", "label": "Import 2,000 photos", "request": {}, "state": "running", "done": 120, "total": 2000, "note": "IMG_0120.CR3", "cancel_asked": false, "attempts": 1, "created": 1763000000, "updated": 1763000042}
```

- **States:** `pending`, then `running` (`done`, `total`, null while not
  known, and a short `note`), then settled once: `done` (`result`, the
  work's own answer), `failed` (`reason`, and `retryable`: running it
  again may work) or `cancelled`. A settled promise never changes again,
  except a failed one retried, which goes back to `pending`.
- **Its owner** is the job its work reports under (`job.*`, the jobs
  centre); `kind` names the work, so a node knows how to run it again.

| Method | Params | Answers |
|---|---|---|
| `promises.get` | `id` | `promise` |
| `promises.list` | `states`, `owner`, `kind`, `limit` (at most 1,000) | `promises`, newest first, and `total` |
| `promises.wait` | `id`, `timeout_ms` (at most 600,000) | `promise`, `settled`: answers when it settles or at the timeout, whichever comes first |
| `promises.cancel` | `id` | `promise`: its work is asked to stop (its job is cancelled too), and settles it `cancelled`; one with no runner on the node is settled `cancelled` at once |
| `promises.retry` | `id` | `promise`, pending again with one more `attempts`; refused unless it failed `retryable` and this node runs its kind |

- **One way to wait on anything long.** `promises.wait` answers at the
  timeout with `settled: false`: wait again. It is cancellable
  (`$/cancel`) and stops at once. An agent reaches it through
  `helios_command_run`; the 10-minute cap is the MCP wait cap.
- **Events.** `promise.progress` (created, progressing, retried) and
  `promise.done` (settled, once), each with the promise as its payload,
  on the node's bus.
- **After a stop or a crash.** As the node starts, a promise left
  `pending` or `running` with no runner is taken up again when its kind
  can resume (its work skips what it already did), with one more
  `attempts`; any other is settled `failed` with the reason
  `interrupted`, `retryable` when the node knows its kind. No promise
  stays running with nobody working on it.
- The desktop shows a promise as one line (`helios_gui::promise_line`):
  what it stands for, its progress, then its outcome, with Cancel while
  it runs and Retry when it failed and may be retried.

**The library's import by rule** answers with one
([design/import-slow-sources.md](design/import-slow-sources.md)):
`library.import` with a `source` (an absolute folder or card) and a
`rule` (`all`, the default; `new`, what the library does not know; or
`{"items": [absolute paths]}`), beside the import's choices (`copy`,
`into`, `rename`, `layout`, `duplicates`, `signals`, `darktable`: each
photo's darktable sidecars kept as versions, off when absent,
`camera_look`, and a saved source's `filter` and `apply`),
answers `{"promise": …}` of kind `import` at once. Its work lists the
source through its connector (`note`: "listing: N found"), then imports
each file (`done` of `total`, the file's name as `note`), and settles
`done` with `{report, failed: [{path, error}], history, camera_looks}`:
the import's report, the files that failed and why, the import
history's record id. Cancelled, it stops before its next file and keeps
what it imported. The kind resumes after a stop: the same import runs
again and skips what the library already holds. `library.import` with
`paths` still answers once done with its report. The desktop hands the
person's imports (the import window's Import, Import again, a drop, File
› Import) to its node this way, with the paths as the rule's `items`, and
follows the promises in its status bar; an agent's, a script's or an
automatic import there, and an importer plugin's, still run on the
desktop's own `library.import` command.

## A client cache

Every answer of the vault's endpoint carries `seq`: the bus's sequence
once the call ran. Every event of a write is at or below it.
`helios_inspect::cache::ReadCache` keeps the answers of the reads it
knows (`cursor.window`, `cursor.rank`, `library.edit.head`,
`library.stacks.list`, `library.stacks.decisions`, the settings layers'
reads), keyed by method and params, and follows the node's events
(`events.subscribe` on its inspect endpoint) to drop what they name:

| Event | Drops |
|---|---|
| `vault.field_changed`, `vault.version_added`, `edit.saved` | the reads naming its `image` |
| `vault.stack_changed`, `library.decision` | the stacks' reads |
| `settings.layerChanged` | the settings' reads |
| `library.imported`, `vault.locations_changed`, `vault.library_changed`, `vault.captures_set`, `vault.changed_outside`, `node.ready` | everything |

- An answer read at a sequence older than an event already heard is
  checked against that event before it is kept.
- A `gap`, a subscription that ends, or a node of another `instance`
  (restarted) empties the cache.
- **Read your writes**: a writer waits until the cache has seen its
  answer's `seq` (`wait_seen`); from then on, the cache shows the write.
- A cached window keeps the `stale` it was read with: `cursor.refresh`
  is never cached, and is how a client learns its snapshot moved.

## Finding a node

One node owns a vault at a time. It holds the vault's lock
(`.helios/NAME.node.lock` beside the vault file `NAME`, an OS file lock
released when its process ends, even killed). A second node started on
the same vault (one taking over the runs of a dead one) works but is not
advertised. Once it serves, a node started with `--vault`
and `--batch-control` writes its rendezvous file, `.helios/NAME.node.json`:

```json
{"address": "/run/…/batch.sock", "inspect": "/run/…/inspect.sock", "instance": "…", "protocol": "1.0.0", "pid": 4242, "vault": "/photos/library.db"}
```

`address` is the endpoint running the vault's commands, `inspect` the
read-only one, `token_file` (TCP endpoints only) where the session token
is. The node removes the file as it stops. A file left by a killed node is
told apart by the lock: nobody holds it.

The desktop's embedded node does the same while it holds its library
(`helios_node_core::endpoint::advertise`): it serves the library's
commands (`library.import`, `library.importDarktable`,
`library.keepSidecar`, `library.edit.head`, `library.edit.save`,
`library.edit.add`, `library.snapshots.*`, `library.stacks.*`, plus `node.hello`, `commands.list` and
`command.run`) on a private socket (loopback TCP behind a session token where there are no Unix
sockets), writes the rendezvous file, whose `inspect` names that same
endpoint, and removes it as Helios quits. The CLI and `helios-mcp` then
reach the desktop's node instead of starting another.

That endpoint lists the desktop's window commands too: `commands.list`
(`window`: the window evaluated, `main` by default) gives the node's
commands (`scope: "node"`) then the window's (`scope: "window"`, the
egui-pop commands offered to agents), each with `enabled` and, when
greyed, `disabledReason`, the reason its menu entry gives ("Only in the
Helios window", "Does not apply here (…)"). `command.run` (`id`,
`params`, `origin`, `window`, `scope` to pick one where both have the
id) runs a window command on the interface's thread in that open window,
announced on the bus as `control.command` with its origin, and refuses
it with that reason where it does not apply.

The `helios commands` and `helios run` CLI verbs and `helios-mcp`'s
generic pair (`helios_commands_list`, `helios_command_run`) are these two
methods on the node a client finds this way: every command, the node's
and a desktop's window commands, reached without a tool or a verb of its
own, run as `cli` or `agent:mcp`.
`helios-mcp --tools generated` also offers each command of the registries
compiled in as a tool of its own (`helios_cmd_<id>`), its params schema as
the tool's input schema; a call is that `command.run` with `scope: "node"`
([cli.md](cli.md)).

A client (`helios_node::connect`):

1. uses `--node ADDRESS` or `HELIOS_NODE` when given (the token of a TCP
   endpoint in `HELIOS_TOKEN`), and the rendezvous file otherwise;
2. with no node holding the vault, takes the start lock
   (`.helios/NAME.start.lock`), starts `helios-node --vault FILE
   --batch-control … --idle-exit SECONDS`, waits for its `ready` line and
   its rendezvous file, then connects. A client racing it waits on the
   lock and finds the node the first one started;
3. the node started so exits once no client has been connected and no job
   has run for `--idle-exit` seconds.

In a Rust process, `helios_inspect::client::Client` is the client: over a
socket (`Client::connect`) or in process (`Client::in_process`, the
endpoint's own service called directly, with the same checks: schema,
origin, the bus event of a change). `Client::call` returns at once with a
`Pending` to poll or wait on; `_meta.origin` (and `_meta.undo_group`) go
with every call. Windows named pipes are not served yet: there the
endpoints are loopback TCP behind the token.

## The older form

A line without `"jsonrpc"` is the older protocol, answered as before:
`{"cmd": "state"}` answers `{"ok": true, …}` or `{"ok": false, "error":
…}`, and `{"cmd": "subscribe"}` turns the connection into a stream. The
`helios` CLI still sends it ([cli.md](cli.md)); it goes once every client
in the tree speaks JSON-RPC.
