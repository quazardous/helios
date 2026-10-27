# Macros on the desktop: making, exposing, folding and saving them

Status: **proposed** (2026-10-27), not decided. Nothing in it is built.
Each choice below has a default; the open questions at the end are for
the owner to settle.

A macro is a network of modules packaged as one node, showing only the
parameters chosen for it (AVS's macro, Helios's workshop). The engine
runs them since 2026-10-13: a macro is a composite, expanded when a
render is planned. What is missing is everything a user touches: making
one from nodes, choosing what it shows, folding and opening it, saving
it and changing it later. This note proposes that side.

## What exists today

### In the core and the template crates

- **Composites in the engine** (`helios-core/src/network.rs`). The
  `Composite` trait declares a surface (a manifest) and, given its
  parameters, the `Network` it is. `Macro::new(manifest, network)`
  packages a network; `Macro::expose(name, inner, param)` makes an inner
  parameter the macro's own, the inner value its default, and one name
  exposed on several inner nodes links them (their kinds must agree).
  `ModuleRef::composite` puts it in a graph like a module. The engine
  expands it at planning: each inner node is a step with its own region,
  content key, GPU variant, cache and tiles, so a macro and the flat
  network it packages share their content (see
  [module-contract.md](../module-contract.md), "Composites").
- **What a macro does not carry yet.** `Exposed` keeps the inner
  parameter's whole kind (its hard range, its options): there is no
  label, no narrowed range, no soft range, no group. `Macro` gives no
  `param_hints`, so an exposed slider loses the inner module's unit,
  step, soft range and widget.
- **Composite files** (`helios-template/src/format.rs`, `CompositeFile`;
  schema `templates/schema/composite.schema.json`; see
  [templates.md](../templates.md), "Composites as files"). A composite
  is saved as `<id>@<version>.helios-composite.json`: ports, inner nodes
  in order (a node reads `@`, the composite's inputs, and nodes before
  it, so there is no cycle), connections, exits, `exposed`
  (`name`, `node`, `param`) and a panel hint. The first one ships:
  `templates/composites/helios.masked-adjustment@1.helios-composite.json`.
  `helios_template::catalogue::build` turns a file into a `Macro`.
- **Pinning** (`helios-template/src/catalogue.rs`, `Catalogue::newer`;
  `bundle.rs`). A template's node names a composite by id and version.
  A newer version in the catalogue is reported as an update on offer and
  never taken silently; a missing pinned version refuses the template.
- **Folded or opened** (`helios-template/src/build.rs`). A template's
  node with `open: true` is expanded into the graph as ordinary nodes
  named `<name>.<inner>`, wired as inside, so the Masks panel, the
  viewport's shapes and Parameters reach them. An opened composite is
  bypassed node by node, never whole. Not decided in templates.md: how
  a surface folds an opened composite back, and whether the graph keeps
  which nodes came from which composite (today only the `NodeDesc`
  remembers it, through its `composite` and `open` fields).
- **The workspace's composites**. A workspace keeps a copy of its
  template bundle, composites included (`Bundle::composites`,
  `<config>/workspaces/<key>/template.helios-template/composites/`);
  they join the catalogue the workspace is checked against. The
  catalogue the desktop builds (`helios-gui/src/templates.rs`) holds
  the shipped composites; there is no user library of composites.

### On the desktop

- **The Graph panel** (`helios-gui/src/graph.rs`, see
  [gui.md](../gui.md), "Graph view"). egui-pop's node graph editor over
  an adapter; each edit becomes a `network.*` command
  (`helios-gui/src/shell/network_edit.rs`: `connect`, `disconnect`,
  `move`, `add`, `remove`), one undoable workspace step written to the
  workspace's files at once. The editor selects several nodes
  (`GraphState::selection`), but Helios writes one to the shared value
  `view/graph/selection` (the node selected before when it still is,
  else the first). Non-principal nodes are grouped in stacks
  (`stack:<principal>`); a top sub-workspace is a group of its own.
- **The Modules panel** (`helios-gui/src/modules.rs`, 0.30.0). One row
  per module of the network shown, in the order they were added; a row
  selects its module; each row has a Bypass toggle (`bypass/<node>`, one
  undo step, saved with the image's edit). One row is selected at a
  time.
- **Workspace steps and undo** (`helios-gui/src/shell/workspace_edit.rs`).
  The workspace's state is one value of the store, `workspace/state`;
  `Shell::workspace_step` turns it into the next state as one labelled
  undo group and writes the files. `network.delegate` (delegate nodes to
  a sub-workspace: `name`, `nodes`, `dry_run`) is the closest model for
  making a macro: a selection of nodes, checked first, a dry run that
  says what would happen, one step.
- **Per-image values.** An image's edit holds values by path:
  `node/<node>/<param>` and `bypass/<node>`
  (`helios-values/src/graph.rs`). Node names are the keys, so any step
  that renames nodes must carry the values with it.
- **Agents.** The node's MCP server has `helios_network_delegate`,
  `helios_workspace_edit`, `helios_workspace_import_parts`,
  `helios_set_param`, `helios_node_bypass`
  (`helios-node/src/mcp.rs`); no verb makes or edits a composite.

### In egui-pop

- **Dialogs** (`egui-pop-api/src/dialogs.rs`): a `Form` of typed fields
  built from value descriptors, and a `Custom` dialog whose body is any
  component (the mount contract, ADR 0013), its values merged into the
  command's arguments, answered as a form by agents.
- **The tree** (`egui-pop-views/src/tree/`): egui-pop's own, with rename
  in place, keyboard and type-ahead, drag and drop gated by a step,
  AccessKit names.
- **Fields** (`egui-pop-model/src/field.rs`, `egui-pop-core/src/field.rs`):
  one state model (modified, required, invalid, disabled with a reason).
- **Sliders** (`egui-pop-core/src/slider.rs`, `slider/range.rs`): the
  slider with soft and hard ranges, and the `RangeSlider`, two thumbs on
  one bar, each its own focusable widget.
- **The graph editor** (`egui-pop-viewport/src/graph/`, its
  `docs/node-graph.md`): groups with a frame and a title that fold into
  a stack. It has no edit that turns a selection into a node and no
  way to enter a node: both are the application's, through its
  commands.

### What is missing

1. Making a macro from the graph's selection or the Modules panel.
2. Choosing, renaming, narrowing, ordering and grouping the exposed
   parameters, and a format that keeps it.
3. Folding and opening a macro on both surfaces, and dissolving one.
4. Saving it as a file in the workspace, changing it as a new version,
   publishing it beyond the workspace.
5. Undo of each of these as one step, with the image's values carried.
6. MCP verbs for agents.

## How established applications package node groups

**Blender node groups.** Ctrl+G groups the selected nodes into a group
node; the wires crossing the selection become the group's sockets,
shown inside as Group Input and Group Output nodes. Tab enters and
leaves the group (the rest of the tree is hidden meanwhile, a
breadcrumb shows the path); Ctrl+Alt+G ungroups. The interface (the
sidebar's Group tab) lists the sockets: rename, drag to reorder, a
default, a minimum and maximum per socket, and since 4.0 panels that
group sockets under a collapsible heading. A group is a data block
shared by every instance: editing it changes all of them (the number of
users is shown, and "make single user" copies it). Groups travel by
append or link from another file, and as assets in the Asset Browser.
No versions.

**Nuke Group and Gizmo.** Ctrl+G collapses the selection into a Group
node; Ctrl+Enter opens its inner graph in a tab of its own;
Ctrl+Alt+G expands it back into the parent graph. Parameters (knobs)
are exposed by linking inner knobs onto the group's properties panel
(drag with Ctrl, or Pick in Manage User Knobs), each with its own
label, range and tooltip, grouped under tabs and dividers. A group is
exported as a Gizmo, a `.gizmo` file on the plugin path that appears as
a node; a gizmo instance is closed (its insides are copied to a group to
edit them). Versioning is a studio convention, by file name.

**Houdini digital assets.** A selection is collapsed into a subnetwork
(Shift+C), and a subnetwork becomes a digital asset (HDA). Its Type
Properties dialog promotes inner parameters by dragging them onto the
asset's interface: each keeps a reference to the inner one, with its own
label, a range (lockable at either end), a default, and folders (tabs,
collapsible) to group them; order is by drag. The definition lives in an
`.hda` library file shared by every instance, which is locked (it
matches the definition) unless its contents are unlocked for editing,
after which "Save Node Type" writes the definition or "Match Current
Definition" throws the change away. Versions are part of the type name
(`studio::tool::2.0`): several versions coexist and an instance keeps
the one it was made with until it is changed.

**DaVinci Resolve Fusion macros.** The selected tools are made into a
macro from the context menu (Macro > Create Macro): the Macro Editor
lists every input of every tool with a checkbox to expose it, a name to
give it and its order. The macro is saved as a `.setting` file in the
Macros folder and appears in the Effects library as one tool. Finer
settings (each input's soft range `MinScale`/`MaxScale` and hard range
`MinAllowed`/`MaxAllowed`) are attributes in the `.setting` file, often
edited as text. Fusion also has Groups (Ctrl+G), which fold and expand
in place in the node view. A macro instance does not follow later
changes to its file: it is the copy made when it was added.

**darktable styles (the contrast).** A style is a saved list of
history items, made from an image's history with a choice of modules,
and applied to images in append or overwrite mode, where it flattens
into each image's history. It has no ports, no exposed parameters, no
folding and no versions (overwriting a style replaces it); it travels
as a `.dtstyle` XML file. It is what a Helios preset over several nodes
already is, not a macro.

**Where Helios follows and where it departs.**

- It follows Blender, Nuke and Fusion: make from a selection, the
  crossing wires become the ports, Ctrl+G (the key the keyboard plan
  already keeps for "group" in the graph, gui.md), Ctrl+Alt+G to
  dissolve.
- It follows Houdini and Fusion for the interface: inner parameters
  ticked or promoted, each renamed, ranged and ordered, grouped as
  Blender's panels and Houdini's folders.
- It follows Houdini for versions: a macro's file never changes once
  written; a change is a new version, and an instance keeps its version
  until someone upgrades it (the pinning templates already have). It
  departs from Blender's shared data block: an edit made for one image
  never silently changes another workspace's render.
- It departs from Blender's and Nuke's "enter the group": opening a
  macro expands it in place in a framed group, as Fusion's groups and
  Nuke's expand do, because the panels that edit nodes (Parameters,
  Masks, the viewport's shapes) must reach the inner nodes while the
  image stays in view.
- It departs from all of them in what folding costs: nothing. An inner
  step's content key is the same folded or flat, so making, folding,
  opening and dissolving a macro never renders anything again.

## The model

### 1. Making a macro

**From the graph.** With two nodes or more selected, Ctrl+G (the
node's menu: Make macro…) runs `macro.make` on the selection. **From
the Modules panel.** Rows become multi-selectable (Shift+click a run,
Ctrl+click one more), and the panel's menu offers Make macro… on the
rows selected; a stack in the graph (`stack:<principal>`) offers it on
the whole stack, which is gui.md's "a stack can be promoted to a
macro".

Both need a selection value with more than one node:
`view/graph/selected` (a list, transient), beside `view/graph/selection`,
which stays the one node Parameters and Masks follow. The graph editor
writes both; the Modules panel too.

What `macro.make` does (`nodes`, `name`, `id`, `expose`, `open`,
`dry_run`), as one workspace step:

- **Checks.** Every node is a module or a composite (no source); the
  selection is closed under paths (no wire leaves it and comes back,
  which would make a cycle once folded); a node of an opened macro is
  taken with its whole macro or not at all. A refusal says which node
  breaks which rule.
- **Ports.** Each distinct output outside the selection that the
  selection reads becomes one input of the macro, named after the first
  inner input it feeds (`input`, `mask`, then `input_2`); each output of
  a selected node read outside, or shown by the viewport, becomes one
  output, named after its port. Port kinds come from the manifests
  (`channels`, `layout`).
- **The file.** A `CompositeFile` at version 1, its inner nodes in the
  network's order with their names, the inner wiring, the exits, and the
  parameters the workspace holds for them as the inner values.
- **The network.** The selected nodes are replaced by one node named
  after the macro (a free name: `look`, `look_2`), wired as the
  selection was, placed where the selection's first node stood.
- **The image's values.** Values the open image's edit holds on the
  selected nodes move: an exposed one to `node/<macro>/<exposed>`, the
  others become the file's inner values (they are the macro's now).
  Bypassed nodes stay bypassed inside (their `bypass/<node>` becomes the
  inner node's saved bypass); a bypassed node cannot be the only node of
  a macro.
- **Exposed at first** (default): the parameters the open image changed
  from their defaults, by inner node, named `<node> <param>` when two
  would clash; the editor of section 2 opens next on the same step.
- **Opened or folded** (default): folded, as a template places it.

The answer gives the node's name, the file's id and version, the ports
made and the values moved, and a dry run gives the same without
changing anything, as `network.delegate` does.

### 2. The exposed-parameter editor

`macro.interface` (`node`, `exposed`, `dry_run`) opens a `Custom` dialog
of egui-pop, "Macro interface", on a macro node:

- **Left: the inner parameters**, an egui-pop tree, one branch per inner
  node, one row per parameter with a check box (exposed or not) and its
  current value. Typing filters it.
- **Right: the interface**, a second tree: groups are branches, exposed
  parameters are rows. Rename in place (F2, a double click), drag to
  reorder and to move between groups (the tree's drop, gated by its
  step), Delete to unexpose, New group. Dropping a parameter on an
  exposed one links them (the same value sets both), refused with the
  reason when their kinds differ.
- **Below: the row selected.** Its label, a description, a unit; for a
  number, the inner hard range drawn faint and a `RangeSlider` that
  narrows it, a soft range inside that, and the default (the inner value
  by default); for a choice, the options kept (a subset); for a bool or
  a text, the default only.
- **Accept** writes one step, "edit the interface of look". Since the
  dialog's values are merged into the command's arguments, an agent
  passes the same `exposed` list and needs no dialog.

The format grows optional fields on `exposed`, kept in the file and the
schema:

```json
"exposed": [
  { "name": "strength", "node": "blend", "param": "opacity",
    "label": "Strength", "group": "Look",
    "min": 0.2, "max": 0.8, "soft": [0.3, 0.7], "default": 0.5 },
  { "name": "feather", "node": "feather", "param": "radius",
    "unit": "px", "group": "Edges" }
]
```

- The order of the list is the order shown. `group` names a section;
  sections keep the order of their first parameter.
- `min`/`max` narrow the inner hard range and never widen it; values
  outside are clamped when an older edit is read. `options` keeps a
  subset of a choice.
- Hints not given are the inner module's (`Macro` forwards each
  exposed parameter's `ParamHint`, renamed), so the slider keeps its
  unit, step, logarithmic bar and widget.
- The Parameters panel shows a macro's parameters by section, a
  heading per group, folded and unfolded as its other lists are.

### 3. Folding and opening

- **Open** (`macro.open`, `node`): the node becomes `open: true`, its
  inner nodes in the graph as `<macro>.<inner>`, the mechanism templates
  have. The graph draws them in an egui-pop group `macro:<name>`,
  framed and titled with the macro's name and version; the group's title
  menu offers Fold, Interface…, Dissolve. Ctrl+Enter on a macro node
  opens it, on a node of an opened one folds it (Nuke's key for opening
  a group; Tab stays egui-pop's focus key).
- **Fold** (`macro.fold`, `node`): back to one node.
- **The Modules panel** shows a folded macro as one row with a
  disclosure arrow and its version (`look · macro 1`); opened, its inner
  modules are indented rows under that heading row. The arrow runs
  `macro.open` and `macro.fold`. The heading's Bypass bypasses a folded
  macro whole; opened, each inner row has its own, as the engine wants.
- **Values while opened.** An exposed parameter keeps its path
  `node/<macro>/<exposed>` in both states: opened, the inner node's
  slider for it is bound to that path and marked "exposed as strength",
  so an image's edit never changes keys when a macro is opened or
  folded. A non-exposed inner parameter changed while opened is written
  to `node/<macro>.<inner>/<param>`: a change to the macro itself, which
  folding must settle (section 5).
- **Dissolve** (`macro.dissolve`, `node`, Ctrl+Alt+G): the macro gives
  way to its nodes as ordinary ones (their names kept when free), its
  exposed values written to each inner parameter. The composite file
  stays in the workspace for its other users.

Opening and folding change the workspace's network (the `open` flag is
in it), so each is a workspace step, undoable.

### 4. Saving: the workspace, then the library and templates

- **In the workspace.** `macro.make` writes the composite file among the
  workspace's composites (`Bundle::composites`, written with the other
  files by the same step); the network's node pins it by id and
  version. The id is `user.<slug of the name>` (`user.warm-skin`), made
  unique in the catalogue; `helios.` stays the shipped ones'.
- **Into the user's library** (`macro.publish`, `node` or `id`): the
  file is copied into `<config>/composites/`, whose files join the
  desktop's catalogue after the shipped ones, so any workspace adds it
  from the graph's Add menu (a Macros section) and the Modules panel.
  Publishing writes a file outside the workspace: it is not an undo step
  and it says where it wrote.
- **Into templates.** "Save as template" already bundles the composites
  the network uses; nothing more is needed.
- **With the image's edit.** The edit keeps values only (as for any
  node): the macro's exposed values under `node/<macro>/<exposed>`.
  The network, and so the macro's version, is the workspace's.

### 5. Versioning when edited

- **A file is never rewritten.** `user.warm-skin@1` stays as it was
  once written; any change to the macro is version 2, a new file.
- **What changes a macro:** its inner network (nodes added, removed,
  rewired while opened), inner values changed while opened, and its
  interface (section 2). The interface editor saves a new version on
  Accept. Folding an opened macro that changed asks, in a dialog whose
  answer agents pass as `changes`:
  - **Save as version 2** (default): the file at version 2, this node
    pinned to it;
  - **Expose them**: the changed inner parameters become exposed ones
    (also a version 2), their values kept on this image;
  - **Discard**: back to version 1's inside;
  - **Keep it opened.**
- **Other users of the macro** (other nodes, other workspaces, templates)
  keep their version. The workspace's validation reports the newer
  version as an update on offer, as templates do; `macro.upgrade`
  (`node`, `version`) moves a node to it, as one step, carrying exposed
  values by name and saying which were dropped.
- **Linked instances are not in the model**: two nodes on one macro are
  two pins, not one shared data block (Blender's model), so an edit made
  while looking at one image never changes another's render unasked.

### 6. Undo

Each of `macro.make`, `macro.interface`, `macro.open`, `macro.fold`,
`macro.dissolve` and `macro.upgrade` is **one undo step**, labelled
("make macro look", "fold look"). A step changes two things together:
the workspace's state (network, positions, composites) and the open
image's values that move. `Shell::workspace_step` writes the first as
one store group; it gains the values to move in the same group, so
Ctrl+Z gives back the nodes, the files and the image's sliders at once.
Undoing `macro.make` removes the composite file it added (it is part of
the workspace's state); a published file is never removed by undo.

### 7. Agents: MCP verbs

Every desktop action is a command with its arguments whole, so agents
use the same steps (and the same refusals) as the user. On the node's
MCP server:

| Verb | Command | Arguments |
|---|---|---|
| `helios_macro_make` | `macro.make` | `nodes`, `name`, `id`, `expose`, `open`, `dry_run` |
| `helios_macro_interface` | `macro.interface` | `node`, `exposed`, `dry_run` |
| `helios_macro_open`, `helios_macro_fold` | `macro.open`, `macro.fold` | `node`, `changes` (`version`, `expose`, `discard`) |
| `helios_macro_dissolve` | `macro.dissolve` | `node`, `dry_run` |
| `helios_macro_upgrade` | `macro.upgrade` | `node`, `version`, `dry_run` |
| `helios_macro_publish` | `macro.publish` | `node` or `id` |

`helios_describe` already lists the catalogue's composites; it gains
each one's exposed parameters with their ranges and groups, and the
macros on offer for upgrade.

## Open questions

Each with the default proposed:

1. **What is exposed when a macro is made?** The parameters the open
   image changed from their defaults, the editor opened next. (Others:
   nothing, as Nuke and Houdini; everything, as Fusion's list before
   it is unticked.)
2. **Is `open` a network change or a view?** A network change (the
   flag templates already have), one undo step, saved in the workspace.
   A view would keep undo for edits only, but the panels would then
   reach inner nodes that the network does not name.
3. **Folding a changed macro:** a new version by default, Expose,
   Discard or Keep opened offered.
4. **The id of a user's macro:** `user.<slug>`, unique in the
   catalogue; the name stays free text.
5. **Narrowed hard ranges:** allowed (`min`/`max` within the inner's,
   a subset of a choice's options); older values outside are clamped on
   reading, with a note.
6. **Format version:** the new `exposed` fields are optional, so the
   files stay `format: 1`; an older Helios refuses them (unknown
   fields), which is the honest outcome.
7. **Keys:** Ctrl+G make, Ctrl+Alt+G dissolve, Ctrl+Enter open and
   fold, all in the graph's scope.
8. **Where the user library lives:** `<config>/composites/`, beside
   `<config>/templates/`; a shared registry stays a later question, as
   for templates.
9. **May an agent make and publish macros?** Make, edit and upgrade
   inside a workspace, yes (undoable, as any edit); publish to the user
   library only with the user's confirmation in that session. This also
   settles templates.md's "whether the AI may write composite files" for
   the workspace.
10. **Does an image's edit record the macro version its values were
    made with?** No: values only, as for modules; the workspace pins the
    version. An image opened in another workspace reads the exposed
    values by name.

## Build steps

One branch each, in order; each ships its commands, MCP verbs, tests
and the docs it changes (`docs/gui.md`, `docs/templates.md`,
`docs/module-contract.md`, `docs/cli.md`).

1. **The interface in the format** (`helios-core`, `helios-template`):
   `exposed` gains `label`, `description`, `unit`, `group`, `min`,
   `max`, `soft`, `options`, `default`; the schema follows; `Macro`
   narrows the kind and forwards the inner `ParamHint`s; tests on
   clamping, links and hints.
2. **Pure macro operations on a workspace** (`helios-template`):
   `Template::macro_made(nodes, name)` giving the next template, the
   composite file and the values to move; `macro_dissolved`,
   `macro_opened`, `macro_folded`, `macro_upgraded`; refusals (sources,
   paths leaving and coming back); headless tests that render before
   and after the same, with the same content keys.
3. **Desktop steps and undo** (`helios-gui`): `view/graph/selected`;
   the `macro.*` commands as workspace steps carrying the image's
   values in the same undo group; Ctrl+G, Ctrl+Alt+G, Ctrl+Enter; the
   graph's `macro:<name>` group and title menu; the Modules panel's
   multi-selection, heading rows and disclosure; the MCP verbs.
4. **The interface editor** (`helios-gui`, egui-pop's dialog, tree and
   range slider): `macro.interface`, and sections by group in the
   Parameters panel. Anything egui-pop lacks (a tree drop between two
   trees) goes there first, as its own change.
5. **Versions**: folding a changed macro, the dialog and its
   `changes`; updates on offer in the workspace; `macro.upgrade`.
6. **Publishing**: `<config>/composites/` in the catalogue, the Add
   menu's Macros section, `macro.publish`.
