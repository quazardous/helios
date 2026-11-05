# Workspace templates

Status: **design note, format in code** (2026-10-13). The format, its
schema, validation, the headless build check and composite files live in
`crates/helios-template`; four built-in templates ("Portrait retouch",
"Import, cull, de-duplicate", "Develop one image" and "Develop
(preview)") ship as data under
`templates/`, and the desktop's "New workspace from template" creates a
workspace from them or from the user's own. A workspace is then edited, saved back as a
template, reset to its template, and given parts of another (see
"Editing a workspace"). What is marked "not decided" waits for the
owner.

## What a template is

A **workspace template** is a ready-to-use combination of tools, network
and panel layout, optimised for one known scenario ("retouch a portrait",
"cull a wedding", "scan negatives"). Picking one creates a workspace that
opens with the right network wired, the right panels docked and the right
tools at hand, so the photographer starts working instead of arranging.

Two readers matter from day one:

- **people**, who pick a template by its scenario, share it like a plugin,
  and tweak the workspace it made;
- **an AI**, which will later design a template on demand (the
  `template.design` decision, below). So the format is data checked by a
  schema, built and checked without a window, and every reference in it
  (a module, a composite, a panel, a command) is checked against what
  this Helios knows.

## The `.helios-template` bundle

A template is a directory, `<name>.helios-template/`, shared and installed
as a whole, like an external plugin:

```text
portrait-retouch.helios-template/
  template.json                 meta, network, tools (schema-checked)
  layout.json                   egui-pop's layout document
  i18n/<locale>/template.ftl    its text, one Fluent file per language
  composites/                   the composites it uses, bundled (optional)
    helios.masked-adjustment@1.helios-composite.json
  preview.png                   what it looks like (optional)
```

`template.json` carries a `format` number (1 today). A Helios reading a
format newer than its own refuses the template and says so, never guesses;
an older format is migrated when one exists. Its `version` is the
template's own, bumped by its author.

### `meta`

```json
"meta": {
  "name": "Portrait retouch",
  "scenario": "Retouch a portrait: brighten the subject through a mask …",
  "preview": "preview.png",
  "icon": "portrait",
  "tags": ["portrait", "masks"]
}
```

`name` and `scenario` are the English text. Translations live in the
bundle's Fluent files under keys derived from the template's id, the
same ownership rule as a plugin's catalogues
(`helios_i18n::plugins`): a template may only define messages starting
with its id's slug, so it never rewrites Helios's text.

```text
helios-portrait-retouch-name = Retouche de portrait
helios-portrait-retouch-scenario = Retoucher un portrait : …
```

### `network`

The starting graph, written like a node's network description
(`helios-node`), with composites referenced by id and version:

```json
"network": {
  "sources": ["source"],
  "nodes": [
    { "name": "exposure", "module": "exposure",
      "params": { "exposure": { "float": 0.5 } } },
    { "name": "retouch",
      "composite": { "id": "helios.masked-adjustment", "version": 1 },
      "open": true }
  ],
  "connections": [
    { "from": "source", "port": "image", "to": "exposure", "input": "input" },
    { "from": "source", "port": "image", "to": "retouch", "input": "input" },
    { "from": "exposure", "port": "output", "to": "retouch", "input": "processed" }
  ],
  "view": { "node": "retouch", "port": "output" },
  "presets": [
    { "id": "soft", "name": "Soft",
      "values": { "exposure": { "exposure": { "float": 0.3 } } } }
  ]
}
```

- `sources` are the images the workspace is given (the image opened in
  it); their output port is `image`. A source is given a picture in the
  working space, four channels (RGBA): a JPEG, PNG or TIFF, or a raw
  file's embedded preview. A source named in `raw_sources` (2026-11-03)
  is given a raw file's sensor data instead: its mosaic, one channel in
  raw units, and the camera's description (black and white levels,
  filter pattern, white balance coefficients, matrix) as the context of
  every node reading it, directly or not, as the node gives a raw
  source (`rawprepare` → `whitebalance` → `highlights` → `demosaic` →
  `colorin` turn it into the working space). A raw source refuses a
  picture and a picture source a raw: the desktop opens a raw's mosaic
  in a workspace whose network reads one, its embedded preview
  elsewhere, and refuses a picture in a network that reads a raw.
- A node is a `module` (by its manifest name) or a `composite` (by id and
  version), never both. A composite is placed folded (one node, the
  engine expands it when it renders) or, with `open`, opened: its inner
  nodes are put in the graph as ordinary nodes named
  `<instance>.<inner>`, so the panels that edit nodes (the Masks panel,
  the viewport's shapes, Parameters) reach them. Not decided: how a
  surface folds an opened composite back, and whether the graph keeps
  which nodes came from which composite.
- `params` are checked against the module's (or the composite's)
  manifest, completed with defaults.
- `bypassed: true` places a node bypassed: it keeps its parameters and
  renders as if absent (see module-contract.md, "Bypass"). An opened
  composite is not bypassed whole: its inner nodes are, one by one.
- `view` is the output the viewport shows.
- `presets` are named sets of parameter values by node name (for a
  composite, its exposed parameters). Building with a preset puts its
  values over the nodes' own.

### `subworkspaces`

Named scopes over nodes of the network (optional): a sub-workspace is a
part of the network delegated so that, entered, only its nodes are
shown and edited (see [design/context-stack.md](design/context-stack.md),
"Sub-workspaces"). There is still one network and one engine graph: a
scope changes nothing in the render.

```json
"subworkspaces": [
  { "id": "face", "name": "Face", "nodes": ["exposure", "retouch"] },
  { "id": "mask", "name": "Mask", "nodes": ["retouch.mask"], "parent": "face" }
],
"retired_subworkspaces": ["skin"]
```

- `id` is stable, a slug of the name it was first given (`face`,
  `face-2`), and never given twice in a workspace: a deleted one goes to
  `retired_subworkspaces`.
- `nodes` names every node in it, those of the sub-workspaces nested in
  it included: a node of the network, or an opened composite's inner
  node (`retouch.mask`); naming a composite takes its inner nodes in.
- `parent` nests it in another; its nodes are among its parent's, and
  the sub-workspaces under one parent share no node, so each node has
  one innermost sub-workspace.

Validation refuses an id that is not one, used twice or retired, an
empty name, a node the network does not have, an unknown parent or a
cycle of parents, and nodes outside the parent's or shared between
sub-workspaces side by side.

### `positions`

Where the graph view draws each node (optional), by node name: its
top-left corner in graph units (points at 100 %). It is a view, never
read by the render, kept with the workspace so a network arranged by
hand stays arranged; a node without a position is placed by its rank
(see [gui.md](gui.md), "Graph view").

```json
"positions": { "source": [0, 0], "exposure": [240, 0] }
```

A node taken out of the network takes its position with it; validation
refuses a position for a node the network does not have.

### `tools`

```json
"tools": {
  "toolbox": ["mask.add.circle", "mask.add.ellipse", "mask.add.path"],
  "default": "mask.add.path",
  "keymap": [{ "keys": "Shift+C", "command": "mask.add.circle" }]
}
```

The toolbox is a list of command ids (Helios's commands, so a key, the
palette, an agent and the toolbox reach the same actions); `default` is
one of them; `keymap` adds bindings, written like a profile's, loaded
into egui-pop's **workspace** keymap layer (over the defaults, under the
user's own keys) when the workspace opens. Not decided: the toolbox's
widget (today the workspace keeps the list and the keys only).

### `library`

What the workspace sets for the library (optional):

```json
"library": {
  "import": {
    "source": "card",
    "placement": "copy",
    "rename": "{date}-{seq}",
    "duplicates": "ask",
    "suggest_stacks": true
  },
  "query": { "flags": ["pick"], "min_rating": 2 },
  "auto_advance": true
}
```

- `import` is the workspace's import source, written as `library.import`
  takes its arguments (see [library.md](library.md)): `source` is `card`
  (a mounted card, read from its `DCIM`) or `folder`; `placement`
  `reference` (the default) or `copy`; `rename` a copy's pattern;
  `duplicates` `skip` (the default), `keep` or `ask`; `suggest_stacks`
  proposes stacks after each import (a `stacks.suggest` decision to
  review, nothing applied until accepted). The Vault panel then offers
  **Import…**, and `library.importSource` asks for the card or folder and
  imports it so. Validation refuses a word it does not know, and a
  renaming without a copy.
- `query` (2026-11-03) is what the library opens on, as the search
  field's criteria: `min_rating` (0 to 5 stars), `labels` (`red`,
  `yellow`, `green`, `blue`, `purple`: any of them), `flags` (`pick`,
  `reject`, `none`: any of them) and `tags` (all of them); all must hold.
  The workspace opens with them as the search field's filter, which the
  user may then change; words and dates are not kept. A query changed in
  the workspace (an edit, an undo, a reset) filters the library again;
  its other settings changing keep the search typed. Validation refuses
  a query with no criterion, a word it does not know, more than 5 stars
  and a tag with a space (the search field could not show it).
- `auto_advance` starts culling with auto-advance on.

### `parameters`

What is asked when a workspace is created from the template ("ask on
create", optional): each parameter names where its answer goes in
`template.json`, as a JSON pointer, and what it may be (`text`, `path`,
or `choice` with its `choices`). Without an answer the template's own
value stays.

```json
"parameters": [
  { "id": "import_source", "name": "Import source",
    "at": "/library/import/source", "kind": "choice",
    "choices": ["card", "folder"] }
]
```

Its name is translated under `<slug>-parameter-<id>`. Validation refuses
a pointer to nothing, to something other than text, or to what
identifies the template (`/id`, `/version`, `/format`).

### `layout`

`layout.json` is egui-pop's layout document (`kind: layout`, its own
schema version, migrated by egui-pop): which panel sits in which area, as
tabs and splits, which ones are closed. A template's layout names panels
by id: Helios's panels, and those egui-pop's plugins declare (the jobs
centre, `egui_pop.jobs`); validation refuses a panel neither declares,
or one placed in an area it may not live in.

## Composites as files

A composite (a macro, see [module-contract.md](module-contract.md),
"Composites") is saved as its own versioned file,
`<id>@<version>.helios-composite.json`:

```json
{
  "format": 1,
  "id": "helios.masked-adjustment",
  "version": 1,
  "name": "Masked adjustment",
  "description": "Mixes an adjustment into the image through a mask group",
  "inputs": [
    { "name": "input", "channels": 4, "layout": "rgba" },
    { "name": "processed", "channels": 4, "layout": "rgba" }
  ],
  "outputs": [{ "name": "output", "channels": 4, "layout": "rgba" }],
  "nodes": [
    { "name": "mask", "module": "mask_combine" },
    { "name": "feather", "module": "mask_feather" },
    { "name": "blend", "module": "mask_blend" }
  ],
  "connections": [
    { "from": "mask", "port": "mask", "to": "feather", "input": "mask" },
    { "from": "@", "port": "input", "to": "feather", "input": "guide" },
    …
  ],
  "exits": [{ "from": "blend", "port": "output", "output": "output" }],
  "exposed": [
    { "name": "operator", "node": "mask", "param": "operator" },
    { "name": "opacity", "node": "blend", "param": "opacity",
      "label": "Strength", "group": "Blend", "min": 0.2, "max": 1.0,
      "soft": [0.3, 0.9], "default": { "float": 0.8 } }
  ],
  "panel": { "panel": "helios.masks", "area": "start_side" }
}
```

- The sub-network is inner nodes in order (a node reads only the
  composite's inputs, written `@`, and nodes before it: no cycle by
  construction), their wiring and the exits.
- `exposed` turns inner parameters into the composite's own, in the
  order shown (one name on several inner nodes links them; the link
  accepts what all of them accept), each with its inner value as
  default.
- Each exposed parameter may say how it is shown, all optional: a
  `label`, a `description`, a `unit`, a `group` (a section heading;
  sections keep the order of their first parameter), `min` and `max`
  narrowing the inner hard range (never widening it), a `soft` range
  inside that, the `options` a choice keeps, and a `default` tagged by
  kind. What is not given is the inner parameter's: its hints (unit,
  steps, soft range, widget) follow it under the exposed name. A linked
  name gives these on its first entry. An interface that does not fit
  its inner parameter is refused when the composite is built; an older
  Helios refuses the fields it does not know.
- An inner node may itself be a composite, by id and version.
- **Version pinning.** A template keeps the composite version it was made
  with. When the catalogue holds a newer one, validation reports it as an
  update on offer; nothing changes until someone asks (the desktop, an
  agent or the author upgrades the pin, and the template's own version is
  bumped). A template whose pinned version is missing is refused, never
  built with another version.
- **Panel hints.** A composite may say which panel edits it and where that
  panel docks (`panel`): building a template docks that panel when its
  layout does not show it. A template's node may override the hint (its
  own `panel`), for instance to dock the Masks panel at the end side.
- **Workshop block** (experimental, 2026-11-05, see
  [design/workshops.md](design/workshops.md)). A composite may carry a
  `workshop`: its `intent` (English), `keywords`, and `targets`, each a
  relative target on one exposed number parameter (`param`, `measure`:
  `luma.median` or `luma.high` in stops around 1.0, `clip.high` in %,
  `value`, `at`: `output` by default or `input`, an offset from what the
  input measures). The file stays `format: 1`: an older Helios refuses
  the field. A target on a parameter that is not exposed, on one that is
  not a number, a second target on one parameter, or a `clip.high`
  outside 0 to 100 % is refused when the composite is built. With
  workshops switched off the composite is the macro it is.
  `helios_describe modules` lists the shipped composites with their
  blocks.

**Macros on a workspace** (`helios_template::macros`). A user's
composites are made from a workspace's nodes and live in its bundle,
as pure steps on the workspace's state, each given the open image's
values and giving back the next state, the values to move and an
answer:

- `macro_made`: nodes packaged as one node pinning a new file,
  `user.<slug of its name>@1`; the wires reaching them become its
  inputs, what is read outside or shown its outputs, and the
  parameters the image changed are exposed (or those named), their
  values moved to `node/<macro>/<exposed>`. Refused for a source, a
  bypassed node, a node outside reading the selection and feeding it,
  or nodes crossing a sub-workspace's edge.
- `macro_opened` and `macro_folded`: the node's `open` flag; values
  keep their paths. What the image changed inside that the file does not
  hold (`macro_changes`: inner parameters not exposed, inner nodes
  bypassed) is settled by folding's `Changes`: saved as the file's next
  version, exposed by that next version, or discarded; never written
  into the file pinned.
- `macro_dissolved`: the inner nodes back as ordinary ones, the
  exposed values written to them; the file stays in the bundle.
- `macro_upgraded`: the pin moved to a newer version, values carried
  by name. `Bundle::updates` lists the nodes pinned at an older version
  (what `validate` reports as updates on offer), without building.
- `macro_interface` and `macro_interfaced`: the interface a macro
  shows (its `exposed` list) and every inner parameter it could show;
  a new interface is saved as the next version of the file (a shipped
  macro becomes a user's copy, `user.<slug>@1`), the node pinned to it.
  Values follow the inner parameters, not the names: a renamed one keeps
  its value (clamped into a narrower range), one no longer exposed is
  written into the new file. Refused while the macro is open.
- `macro_publishable`: the files `macro.publish` copies into the user's
  library, the node's pinned version (or a composite by `id`) with the
  macros it nests that Helios does not ship.

Each keeps the content of every inner step, so nothing renders again.

**The user's library of composites** (`helios_template::library`,
`<config>/composites/<id>@<version>.helios-composite.json`): the macros
published from a workspace. It joins the catalogue after the shipped
composites (a shipped file is never replaced), and a workspace's own
bundled file stands over a library file of the same id and version. A
library file is never rewritten: publishing a version it holds is
refused, and a change is published as the next version. A macro added
to a workspace from the library is copied into its bundle
(`Bundle::carry_composites`), so the workspace builds without it.
Publishing is outside the workspace's undo; removing the file
unpublishes it.

The first composite file is `helios.masked-adjustment` (version 1): a
mask group (`mask_combine`), feathered along the image's edges
(`mask_feather`) and blended (`mask_blend`), exposing the group's
operator, the feather's radius and the blend's opacity. Its hint is the
Masks panel.

The shipped workshop (experimental) is `helios.lift-midtones` (version
1): an exposure then a sigmoid, exposing `brightness` (-3 to 3 EV) and
`contrast` (1 to 3), its `brightness` aimed so that the output's median
luminance sits at -2.5 stops. With workshops off it is a plain macro.

## Sharing like plugins

A template travels as its bundle, never as code: it references modules
and commands by name, and composites by id and version. The user's
templates live under `<config>/templates/<id>.helios-template/` ("Save
as template" writes there; installing a shared one is copying its
bundle there, not built yet as a command); "New workspace from template"
lists them after the built-in ones, its bundled composites join the
catalogue, its Fluent files are added with the plugin ownership rule. A template naming a module or command this Helios lacks
is refused with what is missing (a plugin to install), as a profile's
plugin references are listed before an import. Not decided: a registry
of shared templates, and signing.

## Validation and the headless build

`helios_template` checks a template in two steps, both without a
window:

1. **Validation** (`Bundle::validate`): the format, ids and names, every
   module, composite (and version), panel and command against the
   catalogue Helios gives, the tools (the default tool in the toolbox),
   the presets' nodes, the layout's panels and areas. The result lists
   every problem with where it is (`network.nodes[1] (retouch)`), and the
   composite updates on offer.
2. **Build** (`Bundle::build`): the network as a `helios_core::Graph`
   (sources filled with a placeholder of their kind, composites folded or
   opened, the view found, every required input connected on the render
   paths: the nodes the view and every sink read from) and the layout
   with the composites' panel hints applied. A test renders the result.
   A node off those paths may wait for its wires: the graph view adds a
   node, then wires it, one edit each; nothing renders it meanwhile, and
   asking for its output says which input is missing.

The JSON schemas (`templates/schema/template.schema.json` and
`composite.schema.json`, `helios_template::schema`) describe the files;
serde refuses unknown fields as the schemas do, and a test keeps them in
step with the types.

## Creating a workspace from a template

The desktop's **New workspace from template** (`workspace.new`) lists the
templates with their scenario, asks for the workspace's directory, then:

- copies the bundle into the workspace's own files
  (`<config>/workspaces/<key>/template.helios-template/`, its composites
  bundled) and records it in `workspace.json`;
- writes the built layout as the workspace's layout document, so egui-pop
  loads it when the workspace opens;
- opens the workspace in its window; there the template's keys go into
  the workspace keymap layer, and an image opened is the template's
  source, shown at its view.

The template's parameters are asked in the same form (`parameter.<id>`);
an answer of the wrong kind refuses the creation before anything is
written.

## Editing a workspace

A workspace made from a template is then the user's: it is edited, and
what it becomes is kept (decided 2026-10-13: the workspace is saved
continuously; "Save as template…" is an explicit extraction; importing
is copying, with no provenance). `helios_template::workspace` holds the
model; the desktop and agents drive it.

**Its files.** Under `<config>/workspaces/<key>/`:
`template.helios-template/` is the template it was made from, as it was
(what a reset goes back to); `workspace.helios-template/` is the
workspace as it stands, written as a bundle once it is edited (network,
tools, library, a copy of its layout); `workspace.json` holds its
profile, the template it came from, and its own name, description and
icon. Its live layout is egui-pop's document, saved as it changes. A
workspace not made from a template starts from the blank network (an
exposure through a mask).

**On the desktop.** The state is one value of the value store
(`workspace/state`), so each edit, reset or import is one step of
Edit > Undo, written to the files at once (a kill loses none: `cargo
survival`). The File menu has:

- **Edit the workspace** (`workspace.edit`): its name, description and
  icon in a form; agents pass `changes` (network, tools, library,
  layout, written as in a template), checked against the catalogue
  before they apply. Keys and tools set this way go into the
  workspace's keymap layer, over the defaults and under the user's own.
- **Keyboard shortcuts** (the settings window): keys change for every
  workspace (the user's, by default) or for this workspace. The
  workspace's are its own `tools.keymap`, one undo step written to its
  files; a key of the template taken away is kept as a removal
  (`{"keys": "", "command": "-mask.add.circle"}`, as a profile writes
  it). Each key says where it comes from: `template` (the template the
  workspace was made from set it), `workspace` or `user`.
- **The workspace's library** (`workspace.library`): the import source
  (or none), placement, renaming, duplicates, suggested stacks, the
  query it opens on (typed as in the search field, `flag:pick
  rating:3`; an agent may also pass it written as in a template) and
  culling's auto-advance, in a form.
- **Add from a workspace or a template…** (`workspace.importParts`):
  the source picked (a built-in or user template, a recent workspace),
  its parts chosen, then a review: the number of changes, and a side
  (ours or theirs) for each conflict, the workspace's by default.
- **Save as template…** (`workspace.saveAsTemplate`): the review form.
- **Reset to template** (`workspace.reset`): the template's network,
  tools, library and layout, built; the workspace's name stays.

When the network changes (a graph edit, a reset, an import, an undo or
a redo of any), the image shown goes through it in place: the same
source image, its values and its undo history kept, so the step that
changed the network is undone with the image open. A node it had keeps
the image's values; a new node starts from the network's, except for
values the step moved to it (`Shell::workspace_step_moving`, which
sets them in the same undo group as the network), or all of them after
an undo or a redo. The values of a node no longer in the network stay
in the value store for an undo that brings it back, and are not saved
with the edit. A network that cannot take the image shown (a raw
network for a picture) opens it again instead.

**Save as template.** A review says what goes in (the network and the
layout always; the keys, tools and library unless left out), what is
stripped, and what becomes a parameter; the template is then validated
and written, versioned (the same id at the same version is refused
unless replaced on purpose), with its English text and the translations
given (French in the form):

- images and each image's edit history are never in a workspace's
  files (they stay in the vault): the review says so;
- in node parameters, an absolute path, a secret (a parameter named
  like a password, a token or a key) and embedded data (text longer
  than 256 characters: a mask, an image) are taken out;
- the absolute paths and the import source become parameters asked on
  create, each one unless unticked;
- an absolute path anywhere else refuses the review until it is taken
  out; the old template's other messages (its presets') move to the new
  id.

**Parts.** A workspace or template is made of named parts, each copied
on its own (`workspace.parts` lists them):

| Part | Copied as | Conflict |
|---|---|---|
| network | an opened composite (`import.<source>`, at a version the workspace has not used) under one free node name, its inputs wired to the workspace's first image; its nodes renamed `<name>.<node>`; presets and panel hints left behind | `view`: which output the viewport shows |
| layout | each panel shown, where the source shows it | `panel:<id>`: a panel shown on both sides stays or moves |
| keys | each binding added | `key:<keys>` (`@<when>` under a condition): a chord bound to two commands |
| toolbox | the tools missing, in order | none |
| library | the source's settings when the workspace has none | `library`: set differently on both sides |

An import is refused while a conflict has no side picked; its dry run
lists the changes and the conflicts. Nothing records where a part came
from: imported twice, it is two copies. The network's sub-workspaces
come with it, each a copy under an id free in the workspace, its nodes
renamed with the network's (`<name>.<node>`), nested as they were.

**Sub-workspaces.** Delegating nodes to a sub-workspace and deleting
one are workspace steps like the others (one Edit > Undo step each,
with `dry_run`), on the desktop and for agents:

- `network.delegate {name, nodes}`: a new sub-workspace over nodes of
  the network. It is nested in the innermost sub-workspace holding all
  of them, and the sub-workspaces beside it whose nodes are all among
  them move inside it. Nodes crossing a sub-workspace's edge (some in
  it, some not) are refused. The answer gives its `id`, its `parent`
  and the sub-workspaces moved (`adopted`).
- `subworkspace.delete {id}`: the sub-workspace goes, its nodes stay in
  the network, the sub-workspaces nested in it move up to its parent
  (`moved`), and its id is retired.
- `workspace.edit` takes `subworkspaces` whole (as in a template); one
  left out is deleted and its id retired. A node taken out of the
  network (an edit, a composite closed) leaves every sub-workspace; a
  sub-workspace left empty stays until it is deleted.
- A reset to the template keeps the ids used so far retired; "Save as
  template" starts the new template's ids afresh.

Entering a sub-workspace is its place on the context stack,
`helios.sub-workspace` (see [gui.md](gui.md), "Sub-workspaces as
places"): its nodes only shown and edited, its own layout and keys.

**Agents.** The desktop's commands take their arguments whole, with
`dry_run`. The node's MCP server has the same as tools
(`helios_workspace_parts`, `_state`, `_edit`, `_reset`,
`_import_parts`, `_save_as_template`, `helios_network_delegate`,
`helios_subworkspace_delete`, the network edited node by node as the
graph view does it (`helios_network_connect`, `_disconnect`, `_move`,
`_add`, `_remove`, `_paste`), the macros (`helios_macro_make`,
`_open`, `_fold`, `_dissolve`, `_upgrade`, `_interface`; on the files, with an
`image` and its `vault` whose values move with the step, saved as a
version of its edit), each with `dry_run`, and
`helios_workspace_undo`); each answer says where it was made (`via`):

- **Through the desktop** (`via: desktop`), when a desktop holds the
  workspace open. The desktop says so in the workspace's files
  (`desktop.json`: its process and its endpoint, JSON lines as a node's
  inspect endpoint, on a Unix socket in the user's runtime directory or
  loopback TCP with a session token), and lets it go when it closes. The
  request is run at the desktop's next frame: an edit, a reset or an
  import is one step of its Edit > Undo, shown at once and written by
  the desktop, so its next save keeps it; a dry run answers from what
  it shows. `_undo` takes back the desktop's last step only when it is
  an agent's step on the workspace, never the user's. A desktop that
  has not answered in 20 s (a minimized window that draws no frame)
  drops the request and the agent is told.
- **On the files** (`via: files`), when no desktop holds it, or the one
  that said so is gone (killed: nothing listens at its endpoint). Each
  step is kept to be undone by `_undo`, which refuses once the files
  changed since that step (a desktop edited the workspace in between).

**Changed outside the desktop.** A desktop watches its workspace's files
and compares them with what it last wrote there. A change made outside
(another tool, a desktop that did not say it held it) is taken in as one
undo step, part by part (name, description, icon, network, tools,
library, layout and the rest of the template): a part only the files
changed is taken; a part only the desktop changed is kept; a part both
changed is taken from the files, the desktop's own one Edit > Undo
back, and a warning names it (decided 2026-10-14). For the layout,
which egui-pop saves as it changes, the desktop's change is what it has
not saved yet. The same check runs before each of the desktop's own
writes, so a save never writes over a change it has not seen.

**The network in a graph editor** (later): editing the workspace's
network on the desktop, rather than through `changes` or a template,
waits on the node graph view (see gui.md, "Graph view").

## The AI's `template.design` decision (later)

An AI designs a template as a **decision node** answers a question (see
[module-contract.md](module-contract.md), "Decision nodes"): the question
is the scenario in words ("retouch skin on headshots, quick"); the answer
is a `template.json` and a `layout.json`. What it is given:

- the JSON schemas, so its answer has the right shape;
- the **catalogue**: every module's manifest (parameters, ranges, ports),
  every composite file (with its exposed parameters and panel hint), the
  panels and the commands, as `helios_template::Catalogue` describes
  them;
- examples: the built-in templates.

Its answer goes through the same validation and headless build as a
template written by hand; a refused answer is sent back with the
problems, a bounded number of times. The accepted template is recorded
like any decision's answer (asking again makes a new one), and a human
may confirm it first (the AI-assisted mode). Not decided: whether the AI
may also write new composite files, and how a designed template is named
and shared.
