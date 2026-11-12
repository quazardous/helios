# Extensions: modules and plugins

Status: **decided** (2026-10-11). It answers: what separates a
plugin from a module, and does Helios need both? darktable paths are
relative to its source tree.

## What others do

| System | Kinds | Processing extended by third parties? | Trust |
|---|---|---|---|
| darktable | processing modules (`src/iop`), utility modules (`src/libs`), Lua scripts | no: modules build in-tree, a library whose version differs from darktable's is refused (`src/common/module_api.h`); Lua cannot touch the pipe | none for Lua (full `io`, `os`) |
| vkdt | modules only: files (`params`, `connectors`, `params.ui`), shaders, optional C | in-tree | none |
| VS Code | extensions, declared by contribution points (commands, keybindings, configuration, views), loaded on activation events, in a separate process | — | workspace trust limits the workspace, not installed extensions ([docs](https://code.visualstudio.com/docs/editing/workspaces/workspace-trust)) |
| Blender | extensions (add-ons, with a manifest and declared permissions since 4.2) and nodes; node groups are reusable data | custom nodes are not open to add-ons in the evaluators | permissions are disclosed, not enforced; review is the safeguard ([manual](https://docs.blender.org/manual/en/4.2/advanced/extensions/getting_started.html)) |
| AVS/Express | modules, macros, applications: everything is an object of the network, UI included | yes | none |
| GIMP | GEGL operations (reusable, non-destructive) and plug-ins (separate processes) | yes, as operations | crash isolation by process |
| Lightroom Classic | plug-ins for menus, export and publish services, metadata | no | — |

Lessons:

- Every system with a large third-party ecosystem separates pure
  processing from extending the application.
- darktable mixes GUI code into processing modules, which blocks their use
  without a GUI; AVS shows the same tangle at scale.
- A binary interface tied to the exact application version shuts third
  parties out of processing (darktable).
- Scripting interfaces that break on minor versions, and updaters that move
  scripts past the installed application, hurt users (darktable's script
  manager).
- Permissions that are only declared are theater (VS Code, Blender): either
  enforce them or call them disclosure.
- Hard-bound keys and untyped settings pile up (darktable's free
  `plugins/*` configuration keys).

## Proposal: two kinds, one manifest

**Module**: a pure, deterministic unit of a network, as in the module
contract ([module-contract.md](module-contract.md)): ports, a typed
parameter schema with versioned migrations, no IO, no UI code, no
commands of its own. What a surface needs to show it is declared (widget
hints, a node header, parameter actions a shortcut may target). Modules run
headless anywhere, and can be sandboxed (WebAssembly, GPU kernels) without
asking anyone to trust them.

**Plugin**: everything that extends the application: panels, views,
commands, importers and exporters, vault fields, AI resolvers, sync
backends. A plugin is the **unit of packaging**: it may ship modules.

**Saved sub-networks** (macros, node groups) are user data, not a third
kind of code.

## The manifest

One declarative format, read before any code loads (as VS Code's
contribution points):

- **identity**: id (reverse domain), version, license, the Helios versions
  it works with, and the version of each interface it uses (the module
  contract and the application interface are versioned apart);
- **modules**: names, parameter schema versions, ports;
- **commands**: id, title, category, the condition under which they apply;
- **keybindings**: suggestions only, with a `when` context; the framework
  resolves conflicts and the user owns the map (see
  [shortcuts-research.md](shortcuts-research.md)); parameters of modules
  are actions with elements and effects, so a key can reach any slider
  without the plugin choosing one;
- **settings**: typed by a schema, under the plugin's id, with defaults and
  a page; no free configuration keys (see "The configuration contract"
  below);
- **panels and views**: area, the views they belong to, a `when` context;
- **services**: importers, exporters, vault fields, resolvers, sync
  backends;
- **activation events**: code loads when first needed;
- **capabilities**: network, file scopes, other processes, secrets, GPU,
  AI models, each with its reason;
- **text**: a title and a description in English, translated with the
  commands' titles in the plugin's own catalogues beside the manifest
  (`i18n/<locale>/*.ftl`, keys `plugin-<id>.title`, `.description`,
  `command-<command id>.title`, and `<id>-…` for its own messages).
  `helios_i18n::plugins` loads them through egui-pop's prefix-owned
  catalogues: a key that is neither the plugin's nor one of its declared
  commands' is refused, so a plugin never rewrites another's text.

## The configuration contract

What a plugin, a module or a connector lets the person configure is
declared once, as data (`helios_core::config`), at three levels: the
plugin itself (`[settings.<key>]`), each account it connects
(`[account.settings.<key>]`) and each source it reads
(`[source.settings.<key>]`). Settings renders the first two, the import
window the third; built-in connectors (folder, card) declare theirs in
code with the same types (`ConnectorProvider::config`).

```toml
[settings.quality]
type = "int"            # bool, int, float, string, choice, path, secret
default = 90
min = 1                 # int and float: min, max, step
max = 100
step = 1
label = "Quality"       # English; the key's name when absent
help = "The JPEG quality of the previews it asks for"
group = "Previews"      # the heading it is shown under
order = 0               # its place in its group, then by name

[settings.mode]
type = "choice"
choices = ["auto", "custom"]
default = "auto"

[settings.cache]
type = "path"
path = "folder"         # or "file"
visible_when = { mode = "custom" }   # a list: any of its values

[account.settings.token]
type = "secret"         # kept in the keyring, never shown back: no default
label = "Access token"
```

`about`, which older manifests give, is read as `help`; a key without a
default is unset until the person sets one. Labels and help are
translated in the plugin's catalogues as the message
`setting-<id>-<key>` (`account-setting-…`, `source-setting-…`) with the
attributes `label` and `help`. A contract gives a JSON Schema of its
values, checked by the same checker as the node's command params, and the
manifest is refused when a declaration is wrong: an unknown type, a
default that does not fit its key (type, bounds, choices), a secret with a
default, a choice without choices, a path without its kind, bounds on a
key that is not a number, `visible_when` naming a key the level does not
declare, account settings from a plugin without `[[connect]]`, or source
settings without the `import.source` or `importer` service.

## Packaging and trust

- A signed archive: the manifest, the plugin's code, its modules.
- A plugin's modules are held to the module contract alone and run
  sandboxed, so a module can be used without trusting the plugin around it.
- Capabilities are enforced: plugins run out of process and reach Helios
  through brokered interfaces. Until that exists, the manifest calls them
  what they are, a disclosure.
- One reviewed registry, plus self-hosted ones, so forks are not split from
  their extensions.

## Proof of concept

`crates/helios-plugin` runs external plugins out of process, behind a
broker. It covers what is needed to judge the design, not the packaging
(no signed archives, no registry).

**Protocol.** A plugin is a directory holding `helios-plugin.toml` and any
executable that speaks JSON-RPC 2.0 over its standard input and output,
one message per line (the shape of LSP and MCP, without their headers).
The broker reads the manifest first and refuses it, before anything runs,
when the id is not a reverse domain, the Helios version is outside the
range it gives, an interface or its version is unknown, a capability is
unknown or comes without a reason, an activation event names nothing the
plugin declares, or its configuration contract is wrong (a setting's
default that does not fit its type, among others). The
manifest's command runs in the plugin's directory (a program found there
is run from there, any other is looked up on `PATH`).

The broker starts a plugin on its first activation event
(`onStartup`, `onService:<service>`, `onCommand:<id>`) and sends
`initialize` with the Helios version, the interface versions it speaks,
the capabilities granted and the plugin's settings (`<id>.<key>`, typed
by the manifest); the plugin answers, then gets `initialized`. Calls wait
at most a timeout, after which the plugin gets `$/cancelRequest`. The
plugin may send `$/progress` and `$/log` notifications; a request from the
plugin is answered "method not found", since Helios offers plugins nothing
but the calls it makes. A plugin left idle, disabled, or whose grants
change is sent `shutdown` then `exit`, and killed if it lingers; so is
every plugin when Helios exits.

A crash costs that plugin alone: the call in flight fails at once with the
plugin's last line of standard error, a `Crashed` notice goes out, and the
next call restarts it, after a wait that doubles with each crash in a row.

Three services: `importer` (see "Importers" below), `metadata.read` (an image path in, fields out, each with the
tag it came from: camera, lens, exposure, GPS, keywords…) and `device`
(`device.list`, `device.info`, `device.capture`, `device.download`, with
progress). The shell commands are `plugin.list`, `plugin.capabilities`,
`plugin.enable`, `plugin.disable`, `plugin.grant` (`granted: false` takes
the capability back), `plugin.revoke`, `metadata.read`,
`device.list`, `device.info`, `device.capture`, `device.download`, and the
commands plugins contribute, each run in its plugin.

**What is enforced: crash isolation plus brokered data.** Each capability
is either *brokered*, where the broker controls the access and checks it
on every call, or *ambient*, where the plugin's process could act on its
own:

- `images.read` (brokered): a plugin is handed only the path of the image
  it is asked about, after the broker resolved it; without the grant the
  call is refused and the plugin not even started. Read-only is what it is
  told, not yet what the system enforces.
- `source.read` (brokered): an importer is handed only the file or
  folder the user picked for it, checked first against what the importer
  declares; picking it is the grant, for that one scan (a portal's
  pattern), so no standing grant is kept.
- `folder.write` (brokered): a download goes to a fresh staging folder;
  the broker keeps only the plain files the plugin reports inside it,
  moves them into the destination itself and removes the rest.
- `process.spawn`, `device.usb`, `network`, `secrets`, `gpu`, `ai.models`
  (ambient): the broker cannot police them, so it refuses to start the
  plugin until the user grants each one.

**Asking, and remembering the answer.** The user grants capabilities
when they are first needed: an action of a plugin that lacks some of the
capabilities it declares (connecting a Google Photos account) opens a
consent dialog instead of running, with the plugin's name and id and each
capability it asks for, with the reason its manifest gives. Allow grants
them all and runs the action again; Don't allow runs nothing and says
where to allow them later. `source.read` is never asked: picking the
source is its grant.

The grants are kept in the user's settings layer (the node's `user`
layer, so they follow the person), as the table `plugin-grants`: each
plugin's id with the capabilities granted. The broker is given them at
start and writes them back through the node each time one is granted or
taken back. A capability the plugin no longer declares is not granted,
so one a newer version of its manifest adds is asked for again; the
grants of a plugin no longer installed are dropped when the grants are
next written. Settings > Plugins lists every plugin's capabilities with
a switch each (`plugin.capabilities`, `plugin.grant`); taking one back
stops the plugin.

A plugin's process still runs with the user's rights: it could read or
write other files, or run programs it was not granted. Holding the process
itself to its capabilities (namespaces, portals, seccomp) is the next
step; until then the ambient capabilities are a disclosure the user must
accept, not a guarantee.

Examples live in `plugins/examples/`: `exif-exiftool` (Python, standard
library alone, wraps `exiftool` for `metadata.read`) and `gphoto2-camera`
(Rust, a workspace member, wraps `gphoto2` for `device`: list, info,
capture, download with progress; its tests run against a fake `gphoto2`
put first on the plugin's `PATH`, no camera needed).

## Importers

An importer lists what a source holds, a folder or a file such as
another application's catalogue, with what it knows of each photo. Helios
imports the files itself, through its library, so a plugin never writes
the library. Each importer is an entry under **File > Import from →** on
the desktop (the submenu shows only when one is installed); choosing it
asks for the source with egui-pop's path field, and where the photos go
as File > Import… does (see docs/library.md).

**Manifest.** The plugin serves `importer` (interface version 1, or 2:
see below),
declares `source.read`, and one `[[importers]]` table per entry:

```toml
services = ["importer"]
activation = ["onImporter:ratings-folder"]   # or onService:importer

[compat]
helios = ">=0.58.0, <0.59.0"
interfaces = { protocol = 1, importer = 1 }

[[importers]]
id = "ratings-folder"            # unique in the plugin
label = "Folder with ratings.json"
source = "folder"                # what the user picks: "folder" or "file"
holds = ["ratings.json"]         # a folder: names it must hold
# patterns = ["*.lrcat"]         # a file: the names it accepts (`*` globs)
# anywhere = true                # a catalogue: images may lie outside the source

[[capabilities]]
name = "source.read"
reason = "Reads the JPEGs of the folder picked and its ratings.json"
```

The manifest is refused when importers come without the service or the
capability, two share an id, an id or a label is empty, `patterns` are
given for a folder or `holds` for a file, or `onImporter:<id>` names no
importer. The label is translated in the plugin's catalogues under
`<plugin id>-importer-<importer id>` (both written with every character
other than an ASCII letter or digit as `-`):

```text
org-example-ratings-folder-importer-ratings-folder = Dossier avec ratings.json
```

**Before the plugin starts**, the broker checks the source: it exists, is
a folder holding each name of `holds`, or a file whose name matches one
of `patterns`. A source that is not one the importer reads is refused
without starting anything.

**`importer.scan`** (Helios to the plugin), with the scan timeout
(10 minutes by default, not the 30 s of other calls):

```json
{"jsonrpc": "2.0", "id": 7, "method": "importer.scan", "params": {
  "importer": "ratings-folder",
  "source": "/photos/trip",
  "kind": "folder",
  "progress": "org.example.ratings-folder#scan-3"
}}
```

`source` is absolute, symbolic links resolved. The answer:

```json
{"jsonrpc": "2.0", "id": 7, "result": {
  "images": [
    {"path": "a.jpg", "rating": 5, "labels": ["red"], "tags": ["trip"], "title": "Low tide"},
    {"path": "/photos/trip/b.jpg", "rating": 2, "rejected": true},
    {"path": "c.jpg"}
  ],
  "warnings": ["ratings.json names gone.jpg, which the folder does not hold"]
}}
```

| Key | Type | Applied as |
|---|---|---|
| `path` | text, required | absolute, or relative to the source folder (to the folder of a source file) |
| `rating` | 0 to 5 | set where the photo has none |
| `rejected` | true or false | set where the photo has none (stars kept beside it) |
| `labels` | `red`, `yellow`, `green`, `blue`, `purple` | each added |
| `tags` | non-empty texts | each added |
| `title`, `description` | text | set where the photo has none |

Other keys are ignored, so a later version can add some. Helios checks
every image before importing anything:

- **refused** (left out, listed with why): no path, a path that does not
  exist, is not a plain file or not an image Helios reads, a path listed
  twice, a path outside the source (a symbolic link is followed first),
  unless the importer declares `anywhere`, as a catalogue naming photos
  where they are must;
- **dropped** (the image kept, a warning said): a value that does not fit
  its field, such as a rating of 7 or an unknown label.

Then Helios imports the files that passed as any import does (referenced
or copied, duplicates by content, each photo's darktable sidecars as
versions) and applies the values under the importer's name
(`importer:<plugin>/<importer>` in the audit trail), before the
sidecars: the plugin's values are its application's truth, a sidecar may
lag behind. Nothing the library holds is overwritten. A duplicate left
out gets no values.

**Progress.** The plugin may send
`{"method": "$/progress", "params": {"token": "<progress>", "done": 1, "total": 3, "message": "a.jpg"}}`;
the scan is a job of its own (kind `importer.scan`) in the jobs centre,
its progress the plugin's. The import that follows is the library's
`import` job.

**Errors.** A JSON-RPC error answers the scan:

| Code | Meaning |
|---|---|
| -32001 | not a source this importer reads; the message is shown to the user |
| -32602 | invalid parameters |
| -32800 | the request was cancelled |
| any other | the scan failed; the message is shown |

A scan that fails imports nothing; the error goes to the Log panel and a
toast.

**Cancellation.** Cancelling the scan's job sends the plugin
`{"method": "$/cancelRequest", "params": {"id": 7}}` and stops waiting at
once: the import ends cancelled before its first file. The plugin should
stop and may answer -32800; any later answer to that id is ignored, and
the plugin stays running for the next call. Cancelling the import job
afterwards stops it between two files, as for any import.

### Importer interface version 2

A plugin declares `importer = 2` in `compat.interfaces`; Helios accepts
1 and 2, and a version 1 importer runs as above, unchanged. Version 2
adds two things.

**Paged answers.** Helios adds `"pageSize": 500` to `importer.scan`'s
params. The plugin sends its images in pages, notifications carrying the
scan's progress token, in order, before its answer:

```json
{"jsonrpc": "2.0", "method": "importer/page", "params": {
  "token": "<progress>", "images": [ … up to pageSize images … ],
  "done": 500, "total": 100000
}}
```

then answers with the count of pages sent (and `total` when known):
`{"pages": 200, "total": 100000, "warnings": [ … ]}`. An answer may also
carry a last `images` list. Helios checks that every page announced
came, checks each image as in version 1 (a path listed twice is refused
across pages too), and shows each page's `done`/`total` as the scan's
progress. The plugin looks for `$/cancelRequest` before each page, so a
cancel stops it between pages. A plugin may still answer without pages
(an `images` list, no `pages`), as in version 1.

**Edits as history.** An image may carry `edits`, one per darktable
duplicate (`version`, 0 for the original), each with `from` (`library`
or `xmp`), `why` it was chosen (said in the import's log), and either:

- `xmp`: the absolute path of the duplicate's sidecar, read as Helios
  reads any darktable sidecar; or
- `history`: the edit itself, in the shape of
  `helios_xmp::Sidecar::from_history`:

```json
"history": {
  "end": 7,
  "iop_order_version": 5, "iop_order_list": "rawprepare,0,…",
  "items": [{"num": 0, "operation": "exposure", "modversion": 6,
             "enabled": true, "params": "<hex>", "multi_name": "",
             "multi_priority": 0, "blendop_version": 14,
             "blendop_params": "<hex>"}],
  "masks": [{"num": 4, "id": 1, "type": 1, "name": "circle 1",
             "version": 6, "count": 1, "points": "<hex>"}]
}
```

`params`, `blendop_params` and `points` are darktable's bytes in a
sidecar's own encodings (hex, or `gz` + base64 of zlib data), decoded by
the same function, so a history and the `.xmp` spelling the same edit
map onto the same Helios chain, report included. An image with `edits`
gets those as its versions (`import:xmp`, with darktable's duplicate
number), in place of the sidecars found beside it; one without keeps
Helios's own sidecar lookup. An edit with neither a `version` nor a
`history` or absolute `xmp` is dropped with a warning.

**Report.** The scan's line goes to the Log panel: how many photos were
found and left out, each one left out with why, and the plugin's
warnings; then the import's end as usual, with how many photos took
values from the importer.

**What a plugin cannot do through it.** It never writes the library or
the photos; it is handed one path, and what it names is checked. As
other plugins it still runs with the user's rights (see "What is
enforced" above). Not in version 1: views (another application's
folders or film rolls), stacks, edits other than sidecars, and paging
for very large catalogues; see docs/darktable-library-import.md for
what darktable's catalogue would need.

**Shipped importers.** Official importers live in the repository under
`plugins/importers/` and are Rust programs, workspace members built
beside Helios's own executable (one binary per system, nothing else to
install; decided 2026-11-07). Their manifests and catalogues are
compiled into Helios (`helios_plugin::bundled`); the desktop writes them
out once, with a copy of each program found beside it, into Helios's
cache folder (`~/.cache/helios/plugins/<version>-<digest>/`), read as
built-in after `<exe>/../share/helios/plugins` (a copy installed there
wins). A program built for Windows may carry `.exe` where the manifest's
command does not. A test checks that every manifest and catalogue of
their folders is bundled. `cargo xtask release` moves their
`compat.helios` range with each release. The first is
`darktable-library` (see docs/darktable-library-import.md); the second,
`lightroom-catalog`, reads a Lightroom Classic `.lrcat` (stars, rejects,
labels, keywords, collections as tags; the schema it assumes is listed in
its crate documentation). The first source, under `plugins/sources/`,
is `google-photos`: the `import.source` service over Google's Photos
Picker API, with its connection steps declared in `[[connect]]` (an
`oauth` step, then a `test`); see docs/google-photos.md. Beside them, a
source plugin lists how the import window browses its sources,
`browsers = ["picker"]` (any of `tree`, `folder`, `collections`,
`timeline`, `picker`, in the order offered; `picker` when absent): see
docs/design/import-connectors.md, section 3.9.

The test plugin `crates/helios-plugin/tests/fixtures/ratings-folder`
(Python, standard library alone) reads a folder of JPEGs and the
ratings, labels, tags and titles of its `ratings.json`; the broker's
tests and the desktop's import tests run it.
