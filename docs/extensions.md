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
  AI models, each with its reason.

## Packaging and trust

- A signed archive: the manifest, the plugin's code, its modules.
- A plugin's modules are held to the module contract alone and run
  sandboxed, so a module can be used without trusting the plugin around it.
- Capabilities are enforced: plugins run out of process and reach Helios
  through brokered interfaces. Until that exists, the manifest calls them
  what they are, a disclosure.
- One reviewed registry, plus self-hosted ones, so forks are not split from
  their extensions.
