# The node protocol

A Helios node (`helios-node`) serves three endpoints: **inspect**
(`--socket`, read-only, every build), **control** (`--control`, debug
builds with `HELIOS_DEBUG_CONTROL` set) and **batch** (`--batch-control`,
every build, the verbs a batch needs). Each is a Unix socket, or loopback
TCP behind a session token (`{"auth": "<token>"}` as the first line).

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
  `control`, `agent:NAME`, `script:NAME`; `control` when not given): a
  change to the edit is saved under it.
- **Id**: a number or a string. Each call runs on a thread of its own, so
  one connection carries several calls at once; answers come in the order
  the calls end, matched by id. A call without an id is a notification:
  it runs, and nothing is answered.
- Batches (an array of calls) are not served yet: send one call per line.

## Protocol methods

| Method | Endpoint | What it does |
|---|---|---|
| `node.hello` | all | `protocol` (semver), `oldest` (the oldest version still served), `instance` (the node's, the same on its three endpoints; a restarted node has another), `endpoint` (`inspect`, `debug`, `batch`), `permissions` (`inspect`; `batch`; `batch` and `debug`), `server` (name and version) |
| `commands.list` | all | the capability list: on control and batch, every command with its schema, needs, and whether it runs here (`filter`, `enabled_only`); on inspect, its methods |
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
| -32600 | not a valid request (wrong `jsonrpc`, a batch, an id still running) |
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
| -32800 | the call was cancelled |

## The older form

A line without `"jsonrpc"` is the older protocol, answered as before:
`{"cmd": "state"}` answers `{"ok": true, …}` or `{"ok": false, "error":
…}`, and `{"cmd": "subscribe"}` turns the connection into a stream. The
`helios` CLI still sends it ([cli.md](cli.md)); it goes once every client
in the tree speaks JSON-RPC.
