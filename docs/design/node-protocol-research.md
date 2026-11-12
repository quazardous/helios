# The node protocol: prior art and a recommendation

Status: **research, not decided** (2026-11-12). It surveys how other
programs put one process in charge of the data and let several clients
reach it, and recommends a shape for Helios. The ROADMAP records a
decision once the owner settles it.

Related notes: [node-client-split.md](node-client-split.md) (what runs
where; choices A to E taken on 2026-11-12), [architecture.md](../architecture.md)
(channels: data, commands, events, queries; cursors),
[windows.md](windows.md) (one process per library), [undo.md](undo.md).

## 0. The question

The node owns the data: a `Vault<S: Store>` over the SQLite driver
(`helios-vault-sqlite`). It serves:

- an **inspect** endpoint (`helios-inspect`): `state`, `describe`, `events`,
  `wait_event`, `wait_idle`, `subscribe`; JSON lines,
  `{"cmd": ...}` in, `{"ok": true, ...}` or `{"ok": false, "error": ...}`
  out; `subscribe` turns the connection into an event stream with a
  `gap` flag and heartbeats;
- a **control** endpoint: `ControlRequest` verbs (exports, batches,
  workflow runs, `library.import`, `settings.layer.*`, caches, calibration),
  and since the command registry, `commands.list` (id, label, JSON Schema
  of the params, needs, whether it can run here) and `command.run` (params
  checked against the schema, an `origin` such as `user`, `cli`,
  `agent:NAME`);
- transports: a Unix socket, or loopback TCP with a session token
  presented first (`{"auth": ...}`).

Its clients are the desktop (with the node embedded in its process), the
headless `helios-node`, the `helios` CLI and `helios-mcp`. Some of them
still open the vault themselves: the desktop calls the shared `Vault` from
interface code, the CLI opens it for `import`, `helios-mcp` for some
tools, and the health check reads the database file with raw SQLite
connections. Two writers on one vault means the node's caches and
subscribers never hear of the other's changes, and interface code waiting
on the vault's lock stalls a frame.

The goal: **every client reaches the data through the node, by "a kind of
JSON-RPC"**, through one client abstraction that costs nothing when the
node is in the same process. This note looks at how others do it.

Two facts frame the answer:

- Helios already speaks **JSON-RPC 2.0** twice: the plugin broker talks to
  plugins in JSON-RPC 2.0 over their standard input (`helios-plugin`,
  `rpc.rs`), and MCP is JSON-RPC 2.0.
- Pixels do not travel on this channel. Thumbnails, previews and file
  bytes go by path or by a byte stream (node-client-split.md, H3 and H4);
  the bus never carries a payload (architecture.md). The protocol here
  carries commands, queries, pages of records and events.

## 1. Protocols of editors and tools

### Language Server Protocol (LSP)

- **Transport and format**: JSON-RPC 2.0 over stdio (most often), a pipe
  or a socket, each message framed by a `Content-Length` header
  ([specification 3.17](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/)).
- **Capabilities**: an `initialize` request carries the client's
  capabilities and answers the server's; neither side uses a feature the
  other did not declare. New features are added as optional capabilities,
  so old clients and servers keep working. Servers may also register
  capabilities dynamically (`client/registerCapability`).
- **Reads at scale**: results can stream as **partial results**
  (`partialResultToken`, sent by `$/progress`), and long work reports
  `workDoneProgress`. There is no cursor: a result is computed once and
  sent in pieces.
- **Events**: notifications (a JSON-RPC message without an id) both ways:
  `textDocument/didChange`, `textDocument/publishDiagnostics`.
- **Cancellation**: `$/cancelRequest {id}`, a notification; the server
  answers the cancelled request with error `RequestCancelled` (-32800), or
  with its result if it was already done. `ContentModified` (-32801) says
  a result became stale because the document changed.
- **Embedded case**: none in the protocol; editors run servers as child
  processes.
- **Failure**: the client restarts a crashed server and resends the open
  documents (VS Code's language client restarts a few times, then gives
  up).

**For Helios**: the capability handshake, cancellation as a notification
naming the request id, the two "stale" error codes, and progress tied to a
request by a token are proven, and fit a JSON-RPC 2.0 node.

### Debug Adapter Protocol (DAP)

- Not JSON-RPC: its own envelope (`seq`, `type`: `request`, `response`,
  `event`; `command`), with the same `Content-Length` framing as LSP
  ([overview](https://microsoft.github.io/debug-adapter-protocol/overview),
  [specification](https://microsoft.github.io/debug-adapter-protocol/specification)).
- `initialize` answers a flat `Capabilities` object
  (`supportsCancelRequest`, `supportsProgressReporting`...).
- Reads are paged: `variables` takes `start` and `count` for large
  collections ("indexed" and "named" children with counts announced
  first), `stackTrace` takes `startFrame` and `levels`.
- Events: `stopped`, `output`, `progressStart`/`progressUpdate`/`progressEnd`.
- Cancellation: a `cancel` request naming a `requestId` or a
  `progressId`.

**For Helios**: "announce the count, fetch windows by start and count" is
the cursor of architecture.md in its simplest form. DAP also shows the cost
of a home-made envelope: every client library has to be written for it,
where JSON-RPC 2.0 has libraries in every language.

### Xi editor

- A Rust core and front-ends in other languages (Swift, GTK, others),
  separate processes, JSON-RPC-like messages over stdio, the core pushing
  line updates to the front-end's cache
  ([front-end protocol](https://xi-editor.io/docs/frontend-protocol.html)).
- The author's retrospective is the strongest warning in this survey: "I
  now firmly believe that the process separation between front-end and
  core was not a good idea"; asynchrony made window resizing and word wrap
  race with edits, and scrolling needed the front-end to ask the core for
  text outside its cache, which took months to get right
  ([xi retrospective](https://raphlinus.github.io/xi/2020/06/27/xi-retrospective.html)).
  JSON itself was friction, not the cause.

**For Helios**: the interactive loop must not cross an asynchronous
boundary. Choice A (the desktop embeds its node, zero-copy viewport) and
the gesture exception of H1 (a slider drag is drawn locally and committed
as one step) are exactly the answers xi lacked. The node protocol is for
the library, commands and jobs, not for the frame.

### Neovim

- **Transport and format**: MessagePack-RPC over stdio, a Unix socket or
  named pipe, or TCP. `nvim --embed` runs Neovim as a child speaking RPC on
  stdio (how GUIs embed it); `--listen` serves an address, and every
  instance exports its own in `$NVIM` / `v:servername` for child processes
  ([API](https://neovim.io/doc/user/api.html),
  [remote](https://neovim.io/doc/user/remote.html)).
- **Versioning**: `nvim_get_api_info` returns the channel id and the API
  metadata: every function with its parameters and the API level it
  appeared at, `api_level`, `api_compatible` (the oldest level still
  served) and `api_prerelease`. Functions are added, never changed;
  removed ones go through a deprecation period. Clients generate bindings
  from this metadata.
- **Events**: notifications. `nvim_buf_attach` subscribes a channel to a
  buffer: `nvim_buf_lines_event` (with the buffer's `changedtick`) and
  `nvim_buf_detach_event`. The UI protocol (`nvim_ui_attach`) pushes
  redraw events in batches.
- **Batching**: `nvim_call_atomic` ran several calls as one, without other
  events between them (now deprecated in favour of running Lua on the
  server, which does the same).
- **Embedded case**: the same API is served to Lua and Vimscript in the
  process, without serialisation; `--embed` is the out-of-process form.

**For Helios**: the closest model. One API described by machine-readable
metadata (our `commands.list` with schemas), served in process without
serialisation and out of process over RPC, a per-instance address exported
to children, and a monotonic counter (`changedtick`) that lets a client
tell whether its cache is current.

### VS Code extension host

- Extensions run in a separate **extension host** process, which may run
  locally, on a remote machine (Remote SSH, containers, WSL) or in a web
  worker in the browser; the same `vscode` API is offered in each place,
  over an internal RPC protocol between the window and the host
  ([extension host](https://code.visualstudio.com/api/advanced-topics/extension-host),
  [remote extensions](https://code.visualstudio.com/api/advanced-topics/remote-extensions)).
- The RPC is typed by paired interfaces (`MainThread*` on the window's
  side, `ExtHost*` on the extension's side); proxies are generated from
  them, so code calls methods, not messages.

**For Helios**: one API with interchangeable transports, typed proxies on
both sides, and the remote case designed in from the start. Their
separation exists to protect the window from extensions; ours exists so
the data has one owner.

### Zed

- Remote development runs a headless Zed, the `remote-server` binary, on
  the remote machine; the source, language servers, tasks and the
  terminal run there, the interface stays local
  ([remote development](https://zed.dev/docs/remote-development)).
- Its messages are protocol buffers (`crates/proto` in the
  [Zed repository](https://github.com/zed-industries/zed)), carried over
  an SSH connection.
- **Version**: the server's version must match the client's exactly;
  Zed installs or uploads the matching binary on connect.
- **Start and reconnect**: each connection runs the server in proxy mode,
  which "will start the daemon if it is not running, and reconnect to it if
  it is".

**For Helios**: "connect, else start the daemon, then reconnect" is the
pattern for the CLI and MCP. Exact version matching is simple for one
vendor's client and server; Helios also has third-party agents and
scripts, so it needs compatibility across versions instead.

## 2. Data applications

### Syncthing

- A daemon owns its index database; the web GUI and any tool use a **REST
  API** over HTTP, authenticated by an `X-API-Key` header
  ([REST API](https://docs.syncthing.net/dev/rest.html)).
- **Events**: `GET /rest/events?since=N&limit=M&events=...`, a long poll
  that returns the events after `since`. Every event has an `id` that
  "always increases by 1": a jump says events were lost from the buffer,
  and the client must read the state again
  ([event API](https://docs.syncthing.net/dev/events.html),
  [GET /rest/events](https://docs.syncthing.net/rest/events-get.html)).
- **Reads at scale**: paged endpoints (`/rest/db/browse`, `/rest/db/need`
  with `page` and `perpage`).

**For Helios**: our `subscribe` with `since`, the `gap` flag and a
sequence number is the same design, already built.

### Jellyfin, Plex

- A server owns the library database; every client (web, TV, phone) uses
  its HTTP API, with an access token per device and user
  ([Jellyfin API](https://api.jellyfin.org/)). Lists are paged
  (`StartIndex`, `Limit`, with `TotalRecordCount`).
- Changes reach clients over a WebSocket (`LibraryChanged`,
  `UserDataChanged`, session messages): ids of what changed, which clients
  re-read.

**For Helios**: events carry ids, not records; clients re-read what they
show. A per-client token, revocable, for remote access.

### Immich

- A server (database, jobs, thumbnails, machine learning, possibly on
  another machine) and clients (web, mobile, CLI) over a REST API
  described by OpenAPI, from which the client SDKs are generated
  ([API](https://immich.app/docs/api/)).
- Live updates over a WebSocket (asset uploaded, deleted, updated).
- The mobile app keeps a **local database** of the timeline, filled and
  kept current by a sync stream from the server with acknowledged
  checkpoints, so it works offline and does not re-read the whole library.
- The CLI uploads through the same API with an API key; it never touches
  the server's database.

**For Helios**: the schema-first API with generated clients (our
`commands.list` schemas), and a client cache fed by a resumable stream:
the remote-node case of the cursor and cache design below.

### PhotoPrism

- A server indexes an originals folder; its web interface uses a REST API
  and receives updates over a WebSocket; sessions or app passwords
  authenticate ([developer guide](https://docs.photoprism.app/developer-guide/)).

### digiKam (no daemon, for contrast)

- Each digiKam process opens the database itself: SQLite on the local
  disk, or MariaDB (an internal server that lives with the application, or
  a remote server)
  ([database settings](https://docs.digikam.org/en/setup_application/database_settings.html)).
- SQLite on network media is "prohibited"; a shared remote database needs
  the same digiKam version everywhere, and different instances "cannot
  access a common database and collection at the same time".

**For Helios**: what happens without an owner process: the database is the
API, its schema is the protocol, and every client must be the same
version. That is the situation the node removes.

### Lightroom Classic and darktable (in-process SQLite)

- Lightroom Classic opens its catalogue (a SQLite file) in process and
  holds a `.lrcat.lock` file beside it; a catalogue on a network drive is
  not supported, and only one Lightroom opens it at a time
  ([Adobe: catalog FAQ](https://helpx.adobe.com/lightroom-classic/kb/catalog-faq-lightroom.html)).
- darktable opens `library.db` and `data.db` in process, guarded by lock
  files; scripts (Lua) run in the same process on the same database; a
  second darktable on the same library is refused.

**For Helios**: a lock file per vault is still needed (one owner), but
the owner is a node that others can talk to, not a process that shuts
others out.

## 3. Local daemons with a client API

### Docker Engine API

- A REST API over a Unix socket (`/var/run/docker.sock`), TCP with TLS, or
  SSH (`ssh://`, which runs `docker system dial-stdio` on the far side)
  ([Engine API](https://docs.docker.com/reference/api/engine/)).
- **Versioning**: the version is in the path (`/v1.47/containers/json`);
  the daemon serves a range of versions; the client asks `/_ping` and
  `/version` and negotiates down to the highest both speak.
- **Events**: `GET /events` streams JSON objects, filtered, with `since`
  and `until`; logs and attach stream too.
- **Auth**: the socket's file permissions only; access to it is
  root-equivalent. Remote access needs TLS with client certificates.

**For Helios**: a version range negotiated at connect, filtered event
streams with `since`, and the warning that a socket's permissions are its
whole security model.

### D-Bus and systemd

- D-Bus: typed method calls, signals (events) and properties, over a bus
  daemon or a peer-to-peer socket; objects describe themselves by
  introspection; `PropertiesChanged` signals let clients keep caches of
  properties current
  ([D-Bus specification](https://dbus.freedesktop.org/doc/dbus-specification.html)).
- **Activation**: a client calls a well-known name; if no process owns
  it, the bus starts the service from a `.service` file and queues the
  call. systemd's **socket activation** does the same for any socket: the
  socket exists before the service, and the first connection starts it
  ([systemd.socket](https://www.freedesktop.org/software/systemd/man/latest/systemd.socket.html)).
- Auth: the peer's credentials (uid, pid) checked by the bus, polkit for
  privileged actions.

**For Helios**: activation is the clean form of "the CLI finds or starts a
node": the caller never races the start. A cross-platform equivalent is a
lock file and a rendezvous file (below).

### Tailscale LocalAPI

- The daemon `tailscaled` serves an HTTP API over a Unix socket (a named
  pipe on Windows); the CLI and the GUIs are clients of it
  ([`ipn/localapi`](https://github.com/tailscale/tailscale/blob/main/ipn/localapi/localapi.go)).
- **Auth**: read and write permissions per connection (`PermitRead`,
  `PermitWrite`), decided from the connecting user; requests carrying an
  `Origin` or `Referer` header are refused, and non-loopback hosts too,
  so a web page cannot reach the API through the browser.
- **Events**: `watch-ipn-bus` streams state notifications as JSON, with a
  mask of what to send; the first message can carry the full state, then
  changes.

**For Helios**: read and write permission per connection, the browser
guard for any HTTP or WebSocket transport, and a watch stream that starts
with the state so a client never misses what happened before it attached.

### Ollama

- A server on `127.0.0.1:11434` (`OLLAMA_HOST` to change it), HTTP and
  JSON, responses streamed as newline-delimited JSON objects
  ([API](https://github.com/ollama/ollama/blob/main/docs/api.md)).
- The CLI is a client of the same API; on macOS and Windows it starts the
  desktop application when the server does not answer, on Linux the
  server runs as a service.
- No authentication: loopback only.

### Transmission

- An RPC over HTTP, JSON; since 4.1 it is **JSON-RPC 2.0** with named
  params only, the older envelope kept for old clients
  ([RPC specification](https://github.com/transmission/transmission/blob/main/docs/rpc-spec.md)).
- **Versioning**: `session_get` returns `rpc_version_semver`.
- **Auth**: HTTP Basic, plus a CSRF guard: a request without the current
  `X-Transmission-Session-Id` header gets HTTP 409 with the right id, and
  the client resends.
- No event stream: clients poll (`torrent_get` with `ids: "recently_active"`
  returns only what changed, and the ids removed).

**For Helios**: a mature project moved its home-made JSON RPC to JSON-RPC
2.0, keeping the old form alongside: the path proposed below.

### mpd

- A line-based text protocol over TCP or a Unix socket; the server greets
  with its protocol version (`OK MPD 0.24.0`)
  ([protocol](https://mpd.readthedocs.io/en/latest/protocol.html)).
- **Events**: the `idle` command blocks until a subsystem changes, and
  answers only *which* (`changed: database`, `changed: player`); the
  client then re-reads what it shows. `noidle` interrupts it.
- **Batching**: `command_list_begin` ... `command_list_end`.
- **Reads at scale**: `window START:END` on queue and search commands.
- Auth: an optional `password` command and per-permission levels.

**For Helios**: "events name what changed, the client re-reads" is the
simplest correct cache invalidation; mpd has used it for twenty years.

## 4. Rust crates for the transport

| Crate | What | Transports | In process | Licence |
|---|---|---|---|---|
| [jsonrpsee](https://github.com/paritytech/jsonrpsee) | JSON-RPC 2.0 client and server, async (tokio), subscriptions, batches, middleware | HTTP, WebSocket; IPC through `reth-ipc` (a Unix socket or named pipe server from the reth project) | `Methods::call` and `raw_json_request` run a method on the module "without having to spin up a server" ([docs](https://docs.rs/jsonrpsee-core/latest/jsonrpsee_core/server/struct.Methods.html)) | MIT |
| [tarpc](https://github.com/google/tarpc) | Rust-to-Rust RPC from a trait, serde, deadlines and cancellation propagated | any `Stream + Sink`; TCP, Unix sockets with serde codecs | `transport::channel::unbounded()`, an in-memory transport, no bytes | MIT |
| [tonic](https://github.com/hyperium/tonic) | gRPC, protobuf, HTTP/2, streaming both ways | TCP, Unix sockets | a `tokio::io::duplex` pair, still encoded | MIT |
| [capnp-rpc](https://github.com/capnproto/capnproto-rust) | Cap'n Proto: capabilities as objects, promise pipelining, a format read without parsing | any byte stream | a local capability is called directly, no serialisation | MIT |
| [interprocess](https://github.com/kotauskas/interprocess) | local sockets: Unix sockets on Unix, named pipes on Windows, one API, sync and tokio | local only | n/a | 0BSD or Apache-2.0 |
| [serde_json](https://github.com/serde-rs/json) (with its own envelope types) | what Helios does today | any | the dispatcher called directly | MIT or Apache-2.0 |

All of them are compatible with GPLv3 (MIT, 0BSD and Apache-2.0 are).

Notes:

- **tarpc, tonic, capnp-rpc** give typed calls and real in-memory paths,
  but their wire is not JSON (tonic, capnp) or not described outside Rust
  (tarpc). Helios's clients include an MCP server for agents, scripts, a
  future web page and plugins in any language; `commands.list` hands them
  JSON Schemas. A binary RPC would need a second, JSON face for them.
  Their speed buys nothing here: pixels do not cross this channel, and a
  page of 200 catalogue records is a few tens of kilobytes of JSON.
- **jsonrpsee** is the established JSON-RPC 2.0 crate, and its in-process
  call is the pattern wanted. But it brings tokio and an async server
  model to a node whose transport is synchronous, one thread per
  connection, with a session token and a subscribe stream already built.
  It earns its place when the node needs WebSocket (a web client), not
  before.
- **interprocess** is the missing piece for Windows: today's endpoint is a
  Unix socket or loopback TCP; named pipes would give Windows the same
  file-permission model.

## 5. Comparison

| | Transport | Format | Reads at scale | Events | Cancel | Versioning | Local auth | Remote auth | In process | Daemon gone |
|---|---|---|---|---|---|---|---|---|---|---|
| LSP | stdio, socket | JSON-RPC 2.0 | partial results | notifications | `$/cancelRequest` | capabilities at `initialize` | process ownership | n/a | no | client restarts it |
| DAP | stdio, socket | own envelope | `start`/`count` pages | events | `cancel` request | capabilities | process ownership | n/a | no | session ends |
| Xi | stdio | JSON-RPC-like | core pushes a line cache | notifications | n/a | n/a | n/a | n/a | no | n/a |
| Neovim | stdio, socket, pipe, TCP | msgpack-RPC | ranges (`nvim_buf_get_lines`) | `nvim_buf_attach`, UI events | n/a | `api_level`, metadata | socket permissions | none (tunnel) | yes, same API | client sees EOF |
| VS Code ext host | pipe, WebSocket, worker | own RPC | API-specific | API events | cancellation tokens | engine version per extension | process | remote server token | worker in the browser | host restarted |
| Zed remote | SSH | protobuf | project-specific | messages | yes | exact version match | SSH | SSH | n/a | proxy reconnects |
| Syncthing | HTTP | REST JSON | `page`/`perpage` | long poll, `since`, ids by 1 | n/a | in the path for some | API key | API key, TLS | no | GUI reconnects |
| Jellyfin, Plex | HTTP, WebSocket | REST JSON | `StartIndex`/`Limit` | WebSocket, ids changed | n/a | server version | token | token | no | clients retry |
| Immich | HTTP, WebSocket | REST JSON (OpenAPI) | paged, sync stream with acks | WebSocket | n/a | server version, generated SDK | API key | API key, session | no | app works on its local copy |
| digiKam | SQL | the schema | SQL | none | n/a | same version everywhere | file | MariaDB login | yes (the only mode) | n/a |
| Docker | Unix socket, TCP+TLS, SSH | REST JSON | filters | `/events` stream, `since` | close the connection | `/vX.Y/`, negotiated | socket permissions | TLS client certs | no | client errors |
| D-Bus | bus socket | D-Bus wire | properties, methods | signals | n/a | interface names | peer credentials, polkit | n/a | no | activation restarts |
| Tailscale LocalAPI | Unix socket, named pipe | HTTP JSON | n/a | `watch-ipn-bus` stream | close | in the client | peer user; read/write | n/a | in-process bus too | CLI errors |
| Ollama | loopback TCP | HTTP, NDJSON streams | n/a | streamed responses | close | n/a | loopback | none | no | CLI starts the app (macOS, Windows) |
| Transmission | HTTP | JSON-RPC 2.0 (4.1) | `ids`, `recently_active` | none (poll) | n/a | `rpc_version_semver` | Basic, session id | Basic, session id | no | client retries |
| mpd | TCP, Unix socket | text lines | `window` | `idle` (what changed) | `noidle` | greeting version | password | password | no | client reconnects |

## 6. Recommendation for Helios

### 6.1 Protocol: JSON-RPC 2.0, over the transports we have

Move both endpoints to **JSON-RPC 2.0**, one JSON object per line (no
`Content-Length` header: the framing Helios uses today, and the one MCP
over stdio uses).

- **Why**: plugins and MCP already speak it; libraries exist in every
  language (a script or a web page needs none of ours); request **ids**
  let one connection carry several requests at once and interleave
  **notifications** (events), where today `subscribe` takes over a whole
  connection; **batches** are part of the standard (an array of requests);
  error codes are standard (-32601 no such method, -32602 invalid params),
  with room for ours. Transmission made the same move in 4.1.
- **Method names**: the command ids (`library.import`, `settings.layer.get`,
  `workflow.submit`), `params` the object `commands.list` describes. The
  `command.run` indirection disappears on the wire: every method *is* a
  command, checked against its schema, run under its origin. A few
  methods are the protocol's own: `node.hello`, `commands.list`,
  `events.subscribe`, `events.unsubscribe`, `$/cancel`.
- **Errors**: `data` carries what our errors carry today (the reason, the
  last events seen on a wait's timeout). Our own codes, in the
  implementation-defined range: needs not met (no vault, no window),
  stale (the cursor's or record's version moved, as LSP's
  `ContentModified`), cancelled (as LSP's `RequestCancelled`), refused
  (permission).
- **Compatibility**: the endpoint accepts both forms for a while: a line
  with `"jsonrpc": "2.0"` is JSON-RPC, a line with `"cmd"` the current
  protocol, answered as today. Nothing breaks on the day of the switch;
  the old form is removed once every client in the tree speaks the new
  one.
- **Library**: our own envelope types over `serde_json` (a request, a
  response, a notification, an error: about two hundred lines), plugged
  into the existing transport and the command registry. jsonrpsee is the
  candidate the day a WebSocket transport is needed for a web client.

### 6.2 The client abstraction

One `Client`, two transports, the same dispatcher behind both.

```rust
/// What every client of a node uses: the desktop, the CLI, helios-mcp,
/// tests. The node behind it may be in this process or elsewhere.
pub trait Transport: Send + Sync {
    /// Sends one request; the answer arrives on the returned handle.
    fn call(&self, method: &str, params: Value, meta: CallMeta) -> Pending;
    /// Events whose kind starts with `prefix`, after `since`.
    fn subscribe(&self, prefix: &str, since: Option<u64>) -> Subscription;
    fn cancel(&self, id: RequestId);
}

pub struct CallMeta {
    pub origin: Origin,             // user, cli, control, agent:NAME, script:NAME
    pub undo_group: Option<String>, // steps undone together
    pub idempotency: Option<String>,// a retry runs the write once
}
```

- **In-process transport** (the desktop's embedded node, tests): `call`
  goes straight to the node's dispatcher, with the params as a
  `serde_json::Value` (or a typed request where both sides are Rust,
  skipping serialisation as Neovim's in-process API and jsonrpsee's
  `Methods::call` do). It goes through **the same checks** as a remote
  call: schema, needs, origin, undo group, the bus event after a write.
  The gain is the end of bypasses, not speed; a JSON round of a few
  kilobytes costs microseconds, far under a frame.
- **Socket transport**: the Unix socket (named pipe on Windows, through
  `interprocess`) for a local node, TCP with the token for a remote one
  (TLS later, node-client-split.md gap 8).
- **Typed layer on top**: a `NodeClient` with one method per command,
  generated or written from the registry's specs, as VS Code's proxies
  and Neovim's generated bindings are. The MCP server keeps using the
  untyped form (it forwards schemas to agents).
- **Never block the interface thread**: `Pending` is polled each frame or
  awaited off the interface thread. That is what removes the frame stalls
  on the vault's lock: the desktop never holds the vault, only answers.
- **No `Vault` outside the node**: the `xtask check` guard of the plan
  makes it a rule.

### 6.3 Reads: cursors, and a cache the bus invalidates

- **Cursors** for collections, as architecture.md designs them:
  `cursor.open {query, order, projection}` answers an id, a count and a
  version; `cursor.window {id, offset, len}` a page; `cursor.rank {id,
  image}`; `cursor.close`. A cursor not read for a while expires (a lease,
  so a client that went away does not hold memory); a client can reopen
  it from its query. DAP's `start`/`count`, mpd's `window`, Syncthing's
  and Jellyfin's pages are the same idea; ours adds the version.
- **Records** by id, in batches (`images.get {ids, projection}`): one
  request per screen, not per cell.
- **Client cache**: keyed by method and params, each entry with the vault
  version it was read at. Events say **what** changed (ids and a
  sequence), not the new records, as mpd's `idle`, Jellyfin's and Immich's
  WebSocket events do; the cache drops what they name and the view
  re-reads on its next frame. A cursor whose version moved answers
  "stale" with the deltas or a new count.
- **Gaps**: the subscription's `gap` flag (built) or a jump in sequence
  numbers (Syncthing) means "drop the whole cache and re-read". A changed
  node **instance** (the node restarted) means the same, and cursors are
  reopened.
- **Read your writes**: a write answers the sequence number of its event;
  the client knows its cache is current once it has seen that number.

### 6.4 Writes: commands with origin, undo group and idempotency

- Every write is a command (`command.run` today, the method name tomorrow)
  with an **origin** (built) and an optional **undo group**: steps that
  undo together (a batch of ratings, an agent's multi-step edit), as the
  edit session on the node (choice B) records them.
- **Gestures** stay on the client until they end: a slider drag is drawn
  locally and committed as one coalesced command (H1's exception, the xi
  lesson).
- **Idempotency key** on writes a client may retry after a dropped
  connection: the node remembers recent keys and answers the first result
  again instead of running twice.
- **Long work** is a job (built): the call answers the job's id at once;
  progress and the end come as `job.*` events, as LSP ties
  `workDoneProgress` to a token. `$/cancel {id}` cancels a request still
  running; `job.cancel` a job.

### 6.5 Versioning and capabilities

- `node.hello` first on every connection (replacing the bare
  `{"auth": ...}` line on TCP): the client says the protocol version it
  speaks, its name and origin, its token when needed; the node answers
  its **protocol version** (semver, as Transmission's
  `rpc_version_semver`), the oldest it still serves (Neovim's
  `api_compatible`), its **instance** id, its features (`vault`,
  `window`, `cursors`...), and the client's permissions (read, write).
- **Commands are the capabilities**: `commands.list` already says which
  exist, their schemas and whether they can run here and why not; a
  client asks before offering a feature (LSP's capabilities, Neovim's
  metadata). Each spec gains the protocol version it appeared at.
- **Rules**: commands and fields are added, never changed in meaning;
  an optional field may be added to params and answers; a removal goes
  through a deprecation (listed as deprecated in `commands.list` for at
  least one minor version). The tree's own clients and the node are
  always the same version; this matters for agents, scripts and a remote
  node of another release.

### 6.6 Auth, local and remote

- **Local socket**: in the user's runtime directory, mode 0600; check the
  peer's uid (`SO_PEERCRED` on Linux, `getpeereid` on BSD and macOS) as
  Tailscale and D-Bus do; read and write permissions per connection.
- **Loopback TCP**: the session token file (built), presented in
  `node.hello`.
- **Any HTTP or WebSocket transport** (a web client later): refuse
  requests with an `Origin` the node does not trust and any non-loopback
  `Host` (Tailscale's guard against a web page reaching a local API), or
  require a per-session token as Transmission does.
- **Remote**: TLS with a pinned key and a revocable per-client token
  (Jellyfin and Immich keep a token per device); an SSH tunnel meanwhile
  (Docker's `ssh://`, Zed).

### 6.7 How the CLI and helios-mcp find or start a node

1. **One owner per vault**: the node holds a lock on the vault
   (`.helios/node.lock`, an OS file lock released when the process dies,
   never a stale pid file), as Lightroom and darktable do, and writes a
   **rendezvous file** beside it (`.helios/node.json`: socket address,
   instance, protocol version, pid), or in the user's runtime directory
   keyed by the vault's id.
2. **A client** reads the rendezvous file, connects, says `node.hello`.
   The desktop's embedded node advertises its socket the same way, so the
   CLI and MCP reach the desktop holding the vault (Neovim's `$NVIM`
   handed to child processes is the same idea; a terminal opened from
   the desktop can get it in `HELIOS_NODE`).
3. **No node**: the client takes the start lock and spawns
   `helios-node --vault PATH --idle-exit 60s`, waits for its socket (a
   readiness line on its standard output, not a sleep), then connects. A
   second client racing it waits on the same lock and finds the node the
   first started (Zed's proxy mode, D-Bus activation, systemd socket
   activation). The short-lived node exits after a quiet minute with no
   client and no job.
4. **Explicit address**: `--node ADDRESS` or `HELIOS_NODE` (as
   `OLLAMA_HOST`, `DOCKER_HOST`) bypasses discovery, for a remote node.
5. On Linux, a systemd user socket unit can later start the node on the
   first connection; the rendezvous file stays the portable path.

### 6.8 Failure handling

- **Heartbeats** on streams (built) and a read timeout on calls.
- **Reconnect** with backoff; resubscribe from the last sequence seen;
  `gap` or a new instance drops the cache and reopens cursors.
- **Writes in flight** when the link drops: retried with their
  idempotency key, or reported as unknown to the user.
- **Jobs** survive the client (they are the node's); a client back from a
  drop reads `describe jobs` to catch up (built, the relay does it).
- **The embedded node** has no "node gone" case; when the last window of
  the desktop closes, the node either ends with it (today) or detaches
  and keeps running its jobs (node-client-split.md gap 1).

### 6.9 What not to do

- **No binary RPC** (tonic, capnp, tarpc) for this channel: agents,
  scripts, plugins and the web need JSON and schemas; pixels go on the
  data channel.
- **No interactive frame over the protocol** (xi's lesson): the viewport
  and gestures stay in the client's process.
- **No shared database as the API** (digiKam's situation): the schema
  stays private to the driver.
- **No REST**: a JSON-RPC method per command maps the command registry
  one to one; a REST layout would need a second naming for every verb.

## 7. Order of work

Fits the slices of the plan to route every client through the node:

1. The `xtask check` guard against vault access outside the node (burns
   down a list of exceptions).
2. JSON-RPC 2.0 envelope on the existing endpoints, both forms accepted;
   `node.hello` with version, instance and permissions.
3. The `Client` with its in-process and socket transports; the CLI and
   `helios-mcp` move to it, with discovery and the short-lived node.
4. Health check's integrity reads behind a driver function the node calls.
5. Cursors and the event-invalidated cache; the desktop moves to the
   client area by area.
6. Later: named pipes on Windows, TLS for remote, WebSocket for a web
   client (jsonrpsee then).
