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
  a page; no free configuration keys;
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
(no signed archives, no registry, grants held in memory only).

**Protocol.** A plugin is a directory holding `helios-plugin.toml` and any
executable that speaks JSON-RPC 2.0 over its standard input and output,
one message per line (the shape of LSP and MCP, without their headers).
The broker reads the manifest first and refuses it, before anything runs,
when the id is not a reverse domain, the Helios version is outside the
range it gives, an interface or its version is unknown, a capability is
unknown or comes without a reason, an activation event names nothing the
plugin declares, or a setting's default does not fit its type. The
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

Two services: `metadata.read` (an image path in, fields out, each with the
tag it came from: camera, lens, exposure, GPS, keywords…) and `device`
(`device.list`, `device.info`, `device.capture`, `device.download`, with
progress). The shell commands are `plugin.list`, `plugin.enable`,
`plugin.disable`, `plugin.grant`, `plugin.revoke`, `metadata.read`,
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
- `folder.write` (brokered): a download goes to a fresh staging folder;
  the broker keeps only the plain files the plugin reports inside it,
  moves them into the destination itself and removes the rest.
- `process.spawn`, `device.usb`, `network`, `secrets`, `gpu`, `ai.models`
  (ambient): the broker cannot police them, so it refuses to start the
  plugin until the user grants each one.

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
