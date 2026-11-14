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
- Batches (an array of calls) are not served yet: send one call per line.

## Protocol methods

| Method | Endpoint | What it does |
|---|---|---|
| `node.hello` | all | `protocol` (semver), `oldest` (the oldest version still served), `instance` (the node's, the same on its three endpoints; a restarted node has another), `endpoint` (`inspect`, `debug`, `batch`), `permissions` (`inspect`; `batch`; `batch` and `debug`), `server` (name and version) |
| `commands.list` | all | the capability list: on control and batch, every command with its schema, needs, `scope` (`node`), and whether it runs here, with `disabledReason` when it does not (`filter`, `enabled_only`); on inspect, its methods |
| `command.run` | control, batch, a vault's | `{id, params, origin}` (and on a desktop's, `window`, `scope`): runs a listed command by its id, its params checked against its schema, a greyed one refused with its reason; the same as calling the id as a method |
| `events.subscribe` | inspect | `{prefix, since}`: answers `{subscription, last_seq, gap, instance}`, then sends the events whose kind starts with `prefix` as notifications on the same connection |
| `events.unsubscribe` | inspect | `{subscription}`: stops it |
| `$/cancel` | all | `{id}`: stops a call still running |

Changes to the protocol are additive within a major version: a new
method, a new optional field. A client reads `node.hello` to know what it
talks to.

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

`helios-mcp` sends a key with every tool that changes something (the
control tools that write, `helios_command_run`), a new one unless the
agent gives `request_id`, and names it after an outcome unknown. The `helios` CLI does
the same with `--request-id` ([cli.md](cli.md)).

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

The desktop's library writes its fields and its stacks' expansion this
way, on an in-process client of its embedded node: the calls run off
the UI thread, one write after the other in the order made, and their
answers are picked up at the next frames. Its grid reads the same way:
the photos through a cursor kept open for an hour (refreshed when the
catalog moves, read whole again when its sort changes or the cursor is
gone), the search filter's photos through a cursor of their own and its
counts through `library.facets`, each read on a thread of its own and
shown at a coming frame, through a `ReadCache` fed by the bus's events.

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
| `library.imported`, `vault.locations_changed`, `vault.captures_set`, `vault.changed_outside`, `node.ready` | everything |

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
`library.stacks.*`, plus `node.hello`, `commands.list` and
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
