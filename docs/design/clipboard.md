# Copy and paste on the Helios side: what is copied, how it is adapted, agents and nodes

Status: **decided** (2026-10-21): the owner accepted this note with the
default of every open question; building follows its steps. Proposed on
2026-10-18. The owner accepted the analysis's direction: an internal clipboard versus the system one, and
hooks that process and transform what is copied and pasted. Building
started on 2026-10-27 (see "Built" at the end). The generic part (the contract between kernel and interface,
formats and negotiation, the copy and paste steps, sanitisers and
transforms, the Edit menu and keys, a survey of egui, of the clipboard
crates and of VS Code, browsers, GIMP, Krita, Blender, Lightroom and
darktable) is egui-pop's ADR 0015, "The clipboard: typed items owned by
the kernel, offered in several formats, hooked at each boundary"
(proposed), and its analysis. This note covers what Helios copies,
which transforms it needs, how agents and nodes take part, and the
migration.

## What exists today

Read from the code on `main` (2026-10-18), and from the darktable fork
(branch `helios/phase0-catalog-sdk`).

### In Helios

| What | Copy | Paste | Where | Gaps |
|---|---|---|---|---|
| A parameter's path | `param.copyPath`, the parameter's context menu | none | `crates/helios-gui/src/shell.rs:1886`, menu at :2404; `Action::Copy` (:110) handled at :3374 by `ctx.copy_text`, then a "copied" toast | text only; the ergonomics review proposes moving it under an advanced section |
| A parameter's value | `param.copyValue`, the same menu: `helios_values::value_to_json` | none | same | the value goes out as JSON text and cannot come back: no paste |
| Module parameters, all or selected | none | none | | the most frequent copy in a raw editor (darktable's Ctrl+C on the grid) is missing |
| Nodes, sub-networks | none | none | | egui-pop's graph widget has Duplicate only |
| Mask shapes and groups | none | none | | shapes are values (`node/<node>/<param>`) in full-resolution pixels of the image read (`docs/masks.md`, "Drawn shapes"): a copy to another image needs a transform |
| Library images | none | none | | an image is identified by its file's hash (`crates/helios-vault/src/lib.rs:47`); its paths live in the vault's `locations` table, not in edits |
| Styles, presets | none (no user styles yet) | none | | network presets exist inside templates (`crates/helios-template/src/format.rs:261`); ROADMAP 2.3: "Presets and styles live in the vault so every node has the same ones" |
| Workspace parts | `workspace.importParts`, a copy with a review | the review applies it | `crates/helios-gui/src/shell/workspace_edit.rs:56`, model in `crates/helios-template/src/workspace.rs` (`parts` :557, `import` :824, `review` :1194) | already a copy (decided 2026-10-13, see below), but from a file, not a clipboard |
| Save as template | an extraction that strips | | `workspace.rs:1143` (absolute paths), :1172 (`DATA_LENGTH = 256`) | the stripping rules a copy leaving the vault needs, already written |
| Agents (MCP, `crates/helios-node/src/mcp.rs`) | none | none | `helios_set_param` (:535), `helios_workspace_import_parts` (:364) | no copy, paste, style or sync verb |
| Node control endpoint | none | none | `crates/helios-node/src/main.rs` | the same |

The decision that makes workspace parts a copy (`docs/templates.md`,
"Editing a workspace", decided 2026-10-13): "the workspace is saved
continuously; 'Save as template…' is an explicit extraction; importing
is copying, with no provenance", and in its parts section: "Nothing
records where a part came from: imported twice, it is two copies." The
code says the same (`workspace.rs:22`), and so does ROADMAP 2.4
(2026-10-13: importing from another workspace or template copies the
parts, no link kept). A clipboard paste of a part
follows that rule: a paste is a copy, not a link.

### In darktable (the fork)

- **History copy and paste.** `dt_history_copy` (`src/common/history.c:2409`)
  copies nothing: it remembers the source image. `dt_history_copy_parts`
  (:2439) opens the "selective copy" dialog. The paste is
  `dt_history_copy_and_paste_on_image` (:916): it refuses source equal
  to target, keeps an undo snapshot, copies the module order, then
  **merges** (`_history_copy_and_paste_on_image_merge`, :591, module by
  module through `dt_history_merge_module_into_history`, :313, matched by
  operation and instance name, with the drawn forms the module uses,
  found by `_fill_used_forms`, :282, and duplicated) or **overwrites**
  (:755, the target's history and masks history replaced).
- **What is not copied.** `dt_history_module_skip_copy` (`history.h:108`)
  leaves out deprecated, hidden and `IOP_FLAGS_UNSAFE_COPY` modules
  unless the copy is selective: among them white balance
  (`src/iop/temperature.c:261`), lens correction (`lens.cc:282`), raw
  preparation (`rawprepare.c:107`), orientation (`flip.c:95`). These are
  the parameters that belong to one camera or one file.
- **Many targets, one undo.** `_control_paste_history_job_run`
  (`src/control/jobs/control_jobs.c:1786`) pastes on every selected
  image as one undo group, skips the image open in the darkroom, then
  writes the sidecars. Plain paste is always append
  (`dt_control_paste_history`, :2590); selective paste takes the mode
  from the dialog (`dt_control_paste_parts_history`, :2613;
  `src/gui/hist_dialog.c:233`).
- **Keys.** Ctrl+C, Ctrl+Shift+C, Ctrl+V, Ctrl+Shift+V on the
  thumbnail table (`src/dtgtk/thumbtable.c:3272`), the buttons in
  `src/libs/copy_history.c:201`.
- **Styles.** A saved partial history (`src/common/styles.c`), applied by
  `dt_styles_apply_to_image` (:1029); each item's parameters go through
  `dt_iop_legacy_params` when its module version differs (:754), and
  blend parameters through `dt_develop_blend_legacy_params`. Styles
  carry **no drawn forms**: a style with a masked module loses its mask.
- **Old parameters.** `dt_iop_legacy_params` (`src/develop/imageop.c:1521`)
  steps a module's parameters from version to version; it fails (1) or
  asks for the module's defaults (-1).
- **Masks.** Forms are C structs in coordinates normalised to the raw's
  full size (`src/develop/masks.h:146`, `DEVELOP_MASKS_VERSION 6`);
  duplicated by `dt_masks_dup_masks_form` (`masks.c:31`).
- **darktable-mcp** (`src/mcp/`): `list_styles`, `apply_style`
  (`dt_bridge.c:2220`), `save_style`, `import_style`, `get_history`;
  no copy or paste of a history.

### What the inventory says

1. Helios's only clipboard use is two strings out of a context menu;
   nothing comes back in.
2. What users of a raw editor copy most (an edit, or part of one, onto
   other images) has a reference implementation in the fork, with the
   right rules (unsafe modules left out, merge or overwrite, many
   targets as one undo), but bound to darktable's database.
3. Helios's coordinates are pixels of the image read, darktable's are
   normalised: a mask copied between images of different sizes must be
   transformed, which darktable never needed.
4. The stripping rules exist (Save as template) and the copy semantics
   of workspace parts are decided; a clipboard reuses both.
5. Helios's own legacy upgrade of darktable parameters
   (`crates/helios-xmp/src/legacy.rs`, `upgrade` at :160) covers four
   modules (white balance, highlights, exposure, sigmoid); the rest are
   "taken for their presence" (`presence_only`, :182).

## Recommendation in one paragraph

Helios's kernel is the owner of egui-pop's `Clipboard` contract: the
desktop's windows, its agents' sessions and, through the vault, a node
on the same library share typed items. Each item is a Helios type
(`application/x-helios.<type>+json`) with readable fallbacks (a
summary as text, a colour as hex, an image as PNG or its file); a copy
leaving the vault goes through the Save-as-template stripping rules,
and a paste runs the transforms that adapt it to its target: the module
version, the image's size, the colour space, the node names. Every
paste is one undo step of the target's scope (an image's edit, the
library, a workspace), and "paste an edit on 200 images" is one step of
each image, grouped as darktable's job is.

## What Helios copies

| Type | Payload | Fallbacks | Pasted onto | Parts (paste special) |
|---|---|---|---|---|
| `helios.params` (an edit, all or some modules) | per node: module, module version, parameters, marks; the mask sub-networks the nodes use; the source image's size and frame | text: "Exposure, Sigmoid, 2 masks (from IMG_0042)" | one or many images (grid, filmstrip), a node | the nodes, ticked by default except the unsafe ones; merge or overwrite |
| `helios.param` (one value) | path, kind, value, its module and version | the value as text (today's `param.copyValue`) | a parameter of the same kind | none |
| `helios.nodes` (nodes and a sub-network) | nodes with their parameters, the edges between them, the edges cut at the boundary named | text: the nodes' names | a network (graph view, masks panel) | which cut edges to reconnect |
| `helios.mask` (a shape, a group with its members) | the shape modules and the `mask_combine` chain, values, the source image's size and frame | text summary; an image of the mask (PNG, when asked) | a mask group, a module's mask input, another image's group | members |
| `helios.color` | value, colour space, white point | `#rrggbb` in sRGB, and `color(display-p3 …)` text | a colour parameter, a swatch | none |
| `helios.images` (library images) | image ids (file hashes), with the library they come from | text: file names; `text/uri-list` of the files only when the user asks "Copy files" | the library (another collection, a stack), a project; outside: the files | reference or file |
| `helios.style` / `helios.preset` | a style or preset's id and version in the vault, its content inline when it leaves the vault | text: its name | images, a node | as `helios.params` |
| `helios.parts` (workspace parts) | the parts as `workspace.parts` lists them | text summary | a workspace | the parts, with the same review and conflict sides as `workspace.importParts` |

**Unsafe parameters.** Modules that belong to one camera or one file
(white balance, lens correction, raw preparation, orientation and the
like) declare it in their manifest (`copy: per_image`), as darktable's
`IOP_FLAGS_UNSAFE_COPY`: a plain copy leaves them out and says so; a
selective copy lists them unticked.

**Library images: references, not files.** Within one vault an image is
its hash: a paste into another collection or project adds a reference,
never copies the file. Leaving the vault (another application) the
fallback is the file names; the files themselves go out only through
an explicit "Copy files" (`text/uri-list`), never by default, and a
paste of files from another application is an import (the same job,
the same placement and duplicates policy as `library.import`).

**Workspace parts** are pasted through the existing review: the
clipboard is one more source for `workspace.importParts`, beside a
template and a recent workspace. Importing stays a copy with no
provenance.

## Transforms Helios needs

| Transform | From → to | How | Lossy |
|---|---|---|---|
| Module version | `helios.params` of version N → the target engine's version M | Helios modules: the manifest's migration functions, one per version step (`docs/module-contract.md`, "Migrations"); darktable modules: through the fork's `dt_iop_legacy_params` and `dt_develop_blend_legacy_params` (as `dt_styles_apply_style_item` does), not Helios's partial `helios-xmp::legacy`; newer than the target: refused ("made with a newer engine", ROADMAP 2.3), never downgraded | when a migration drops a field: said in the toast |
| Mask across image sizes | `helios.mask` at the source's size and frame → the target's | positions scaled by width and height, radii and feathers by the smaller side, as darktable normalises (`helios-xmp::drawn`, `Frame`); a parameter declared scale-dependent in the manifest ("a radius in pixels") is scaled the same way; different aspect ratio: the shape keeps its relative place, paste special offers "keep pixel size" | an aspect change distorts an ellipse: said |
| Colour across colour spaces | `helios.color` in space A → the target parameter's space | through the colour engine's conversion (the same as the pipeline's), with chromatic adaptation when the white points differ; text in (`#hex`, `rgb()`, `color()`) is parsed as sRGB unless it names a space | out of gamut: clipped, said |
| Network renamed on clash | `helios.nodes` → a network holding the same names | names made unique (`blur` → `blur_2`), edges and parameter paths rewritten with them; the composites' rule (`<name>.<node>`) for a pasted composite | no |
| Local paths stripped | any item → the system clipboard, another vault, an agent outside the vault | the Save-as-template rules (`workspace.rs:1143`, :1172): absolute paths and secrets taken out of parameters, embedded data longer than 256 characters dropped from the text fallback, vault-local ids (image ids of images the other side cannot have, version ids) replaced by names; the item records what went | yes, by design: the toast lists it |
| Parameters onto another module | `helios.param` → a parameter of another module with the same kind | allowed when kinds match (a float onto a float), clamped to the target's range | when clamped: said |
| Paste on many images | `helios.params` → each selected image | one step per image under one group, the image open in the editor included (unlike darktable, it is the same edit service) | no |

## Agents and the node

- **The same steps.** Agents reach `clipboard.copy` and
  `clipboard.paste` through egui-pop's commands on the desktop, and the
  node exposes them as MCP tools: `helios_copy {source: {image, nodes?
  | params? | mask? | images?}, slot?}`, `helios_paste {target: {image
  | images | network | workspace}, format?, parts?, mode: merge |
  overwrite, if_rev, dry_run}`, `helios_clipboard {slot}` (what a slot
  holds, its formats, what was stripped).
- **Dry run.** `helios_paste {dry_run: true}` answers, per target, the
  transforms that would run (versions, scaling, renames), what would be
  left out (unsafe modules, stripped paths) and the changes as a diff of
  the edit, without writing a version. This mirrors
  `helios_workspace_import_parts`'s dry run.
- **Slots.** An agent's session has its own slot: it does not read or
  overwrite what the user copied unless the user allows it; the
  desktop shows "Paste from agent ‘retouch’" in paste special when an
  agent left an item for the user.
- **The node.** A headless node has no system clipboard: its slots are
  in the vault (a small document per slot, `clipboard/<slot>`), so a
  desktop and a node on the same vault can hand an item over (copy on
  the node, paste on the desktop, or the reverse). Items in the vault
  carry ids, never stripped, since they do not leave it.
- **Undo.** A paste by an agent is a group of its session (the undo
  note's rule): one version per image, labelled "Agent ‘<session>’:
  paste Exposure, Sigmoid".
- **The fork's MCP** keeps `apply_style`; a `paste_history` tool there
  is not needed once Helios's own exists.

## Flows

**K** is the kernel's clipboard service, **V** the vault, **UI**
egui-pop's projection.

### An edit pasted onto a selection in the grid

```
user : IMG_0042 focused in the grid, Ctrl+C
K    : item helios.params {nodes of its edit, versions, masks, size};
       white balance and lens left out (per_image), toast says so
user : selects 120 images, Ctrl+V
K    : per image: module versions migrated, masks scaled to its size;
       one version per image under group "Paste edit from IMG_0042"
UI   : one undo step in the library view; Ctrl+Z reverts the 120
```

### Selective paste with overwrite

```
user : Ctrl+Shift+V
UI   : paste special: the nodes (white balance unticked), merge or
       overwrite, the preview on the image shown
user : ticks Exposure and the "Face" mask, overwrite, confirms
K    : target's other nodes untouched (overwrite replaces only the
       nodes pasted, not the whole edit: see question 2)
```

### A mask group onto another image

```
user : masks panel, group "Face" selected, Ctrl+C on a 6000×4000 image
user : opens a 4000×6000 portrait, masks panel, Ctrl+V
K    : shapes scaled (positions by width and height, radii by the
       smaller side); toast "Face pasted; aspect differs, check the
       ellipses"; one step "Paste ‘Face’"; names made unique if needed
```

### A colour from a web page

```
user : copies "#3a6ea5" in a browser, right-click on a colour
       parameter, Paste
K    : external text → helios.color (sRGB) → the parameter's space;
       one step
```

### An agent copying between images with a dry run

```
agent: helios_copy {source: {image: A, nodes: ["exposure", "sigmoid"]}}
agent: helios_paste {target: {images: [B, C]}, dry_run: true}
K    : B: exposure v2 → v3 migrated; C: sigmoid newer than the engine →
       refused for C; no version written
agent: helios_paste {target: {images: [B]}, if_rev: …}
V    : one version on B, origin agent:<session>
```

### Copy to a chat

```
user : Ctrl+C on a node whose parameter holds "/srv/luts/film/lut.cube"
K    : system clipboard text: "lut3d (file: lut.cube)"; the path is
       stripped from the private payload too; toast "1 local path left
       out"; the internal item keeps it for pastes in this vault
```

## Migration

1. The two existing commands become `helios.param` items with a text
   fallback, behind egui-pop's `edit.copy` (no behaviour lost: the
   value still lands as text).
2. Once egui-pop's contract is built (ADR 0015, steps 2 and 3):
   `HeliosHost::clipboard()` with the kernel's service, the focus
   targets as sources and targets (grid and filmstrip: images and
   edits; parameters: a value; masks panel: shapes; graph: nodes).
3. `helios.params` copy and paste on images, merge mode, the
   `per_image` manifest flag, migration through the manifests; paste
   on many images as a group; the fork's legacy path for darktable
   modules.
4. Masks: the scaling transform, `helios.mask`, the masks panel's copy
   and paste; renames on clash for nodes.
5. Paste special: parts, merge or overwrite, preview.
6. Colours, library images (references), styles and presets once they
   exist in the vault, workspace parts as a source of the review.
7. Agents: `helios_copy`, `helios_paste`, `helios_clipboard` with dry
   run; slots in the vault for the node.

## Risks

- **A paste that differs from darktable's.** Merge rules (match by
  module and instance name, masks replaced by form id) decide what a
  user's edit becomes; Helios's are its own, so the equivalence bench
  gets a paste case: the same copy pasted by darktable and by Helios
  renders the same.
- **Scaling masks is approximate** across aspect ratios and crops; a
  paste always says what it changed, and paste special can keep pixel
  sizes.
- **Stripping too much or too little.** A secret in a parameter not
  named like one leaks; a path needed by the target is lost. The rules
  are the template's (tested there), and the item lists what went.
- **Old engines.** A node older than the item's modules refuses the
  paste; the desktop must show which node can apply it.
- **Big pastes.** 1000 images is a job, cancellable, with progress, each
  image its own version (as import is), not a frame-long loop.

## Open questions

Each with the default proposed:

1. **Plain Ctrl+V on images: append (merge) or overwrite?** Merge, as
   darktable's plain paste; overwrite in paste special.
2. **Overwrite replaces the whole edit or only the pasted nodes?** Only
   the pasted nodes, keeping the target's network otherwise; "replace
   the whole edit" is a separate choice in paste special (darktable's
   overwrite).
3. **Unsafe (per-image) modules in a plain copy?** Left out, with a
   toast saying so; ticked on demand in a selective copy.
4. **Paste onto the image open in the editor** from the grid? Included
   (one edit service), unlike darktable.
5. **Masks across sizes: relative or pixel?** Relative (darktable's
   normalisation); pixel size kept on demand.
6. **Library images pasted across vaults:** references when the target
   vault knows the file (same hash), an import job otherwise, asked
   first.
7. **A node's clipboard slots in the vault:** yes, one document per
   slot, kept a week then dropped.
8. **Agents reading the user's slot:** refused unless the user allows
   it for that session.
9. **Copy of a value to the system clipboard as JSON (today) or as a
   readable form ("+0.30 EV")?** Readable text by default; JSON stays
   in the private payload.

## Built

- **2026-10-27, `helios.param`** (migration step 1, with bug 5 fixed:
  `param.copyPath` and `param.copyValue` copied text that could never
  be pasted back). egui-pop's clipboard is connected with its default
  owner (`LocalClipboard`, in memory); Helios declares the type, its
  text fallback (the value as shown, `+0.30 EV`, or the path) and the
  `helios.outside` sanitiser, which applies the Save-as-template rules
  (`helios_template::workspace::strip_for_outside`) to what goes to the
  system clipboard. The focused parameter widget is an endpoint;
  `param.paste` pastes onto a named one (with a dry run). egui-pop's
  history group around a paste is a group of Helios's value store, so a
  paste is one undo step. Not yet: the kernel as the owner (slots in
  the vault, the node's), and the `helios_copy`, `helios_paste` and
  `helios_clipboard` MCP tools; agents use egui-pop's `clipboard.*`.
- **2026-10-27, `helios.nodes`** (from migration step 4: renames on
  clash). The graph view is an endpoint: a copy takes the selected
  nodes with their live parameters, the wires between them, the wires
  coming in and their places; a paste is `network.paste`, one workspace
  step, renaming on a clash (`exposure` → `exposure_2`), rewriting the
  wires and wiring the cut inputs again when their node is there. Paste
  special's parts are the nodes and `inputs`. The network is the
  shell's to edit, so the target answers its plan (the dry run) at once
  and the shell applies `network.paste` at the next frame. Not yet: an
  open composite's inner nodes, presets' values for the nodes copied,
  and a cut (the delete is not offered).
- **2026-10-27, `helios.mask`** (migration step 4: the scaling
  transform). The Masks panel and the image are endpoints: a copy takes
  the selected shape, or every shape of the mask edited, with the
  source image's size; a paste is `mask.paste`, the shapes added to the
  mask edited and scaled to the image open (positions by width and
  height, lengths by the smaller side, path and brush points the same,
  angles kept; a different aspect ratio said as a loss). The transform
  `helios.mask-as-nodes` makes nodes of shapes pasted into the graph
  view. Not yet: a group's structure (nested groups, the group's
  per-port opacity and inversion) is not copied, only its drawn shapes;
  "keep pixel size" in paste special; a pasted shape is not an undo
  step, as creating one is not.
- **2026-10-28, `helios.images`** (migration step 6: library images as
  references). The grid and the filmstrip are endpoints: a copy takes
  the images selected (the cursor's when none is), each its id (the
  file's hash), name and file; other applications get the names, one
  per line (the `helios.outside` sanitiser now strips a text of several
  lines line by line, so a long list is not taken for embedded data). A
  paste into the library follows question 6: an image the library holds
  (same hash) is a reference, nothing to do; the others are imported
  from their files, asked first, as `library.importFiles` (one
  `library.import` job of several paths, placed as the workspace's
  import source says). Paste special's parts are the table's "reference
  or file": **reference** imports the files where they are, **file**
  copies them into the library folder; both or neither leave it to the
  import source. Text another application copied (absolute paths or
  `file://` URIs, one per line) is the same import, asked first too: it
  writes the vault, and a stray paste of a path must not start a job.
  An import is a job, not an undo step. Not yet, and why: "Copy files"
  (`text/uri-list` for other applications) and pasting `text/uri-list`
  or `image/png` from them need egui-pop's system backend (ADR 0015,
  step 5), since the system clipboard carries text only today and the
  outside sanitiser turns paths into file names; a rendered image as
  PNG for the same reason; collections, projects and stacks as targets
  wait for those to exist; the item does not name the library it comes
  from (a vault has no identity yet), the hash is enough to tell a
  reference from an import.
- **2026-10-28, files and images with other applications** (the
  Helios side of the system backend). egui-pop's clipboard still writes
  and reads the system clipboard as text only, so what Helios needs
  beyond text goes through a seam of its own,
  `crate::clipboard::system::SystemClipboard`, over arboard (in the tree
  through egui-winit already; on Linux it speaks X11, bridged by Wayland
  compositors through XWayland), with an in-memory stand-in for tests.
  "Copy files" (`library.copyFiles`) puts the selected images' files on
  it as a file manager does (`text/uri-list`, a Windows file list, macOS
  file URLs); "Copy the image" (`viewport.copyImage`) renders the edited
  image whole and puts it there as 8-bit sRGB (PNG for other
  applications); "Paste from other applications" (`library.pasteFiles`)
  reads files or an image another application copied and pastes them
  through egui-pop's step as an external item, from a slot of their own
  (`system`) so what the user copied in Helios stays. The library's
  endpoint accepts `text/uri-list` and `image/png` (its data
  `{file, width, height}`: the image is written to a temporary PNG file
  first, then imported and copied into the library folder), and a text
  naming no file pasted by the user takes the files beside it on the
  system clipboard. Only the user copies to and pastes from other
  applications (question 8's rule, for the system clipboard). Not
  yet, and why: Ctrl+V when the system clipboard holds files or an image
  and no text (Windows' Explorer, browsers) never reaches Helios, since
  egui-winit drops the key when it finds no text; Copy files offers no
  text beside the files and egui-pop's own copy still writes only text,
  since writing several formats at once is the system backend's
  (egui-pop, ADR 0015 step 5); the primary selection is egui-pop's too.
- **2026-10-28, the agents' tools** (migration step 7, its desktop
  half). `helios_clipboard {slot?, target?, payload?}`,
  `helios_copy {source? | format, data, title?; cut?, slot?, dry_run}`
  and `helios_paste {target, format?, transform?, parts?, if_rev?,
  slot?, dry_run}` are node MCP tools that go, as the workspace and
  context tools do, to the desktop holding the workspace open, where
  they run egui-pop's `clipboard.offers`, `clipboard.read`,
  `clipboard.copy`, `clipboard.write` and `clipboard.paste` as the
  agent: its own slot (`agent:mcp`), the user's refused unless allowed
  (question 8), a paste one undo step, a dry run answering the plan,
  the transform, what is stripped and lost, and the target's changes.
  Sources and targets are the desktop's named endpoints, not the
  design's `{image, nodes?}` selectors: a copy takes the endpoint's
  selection. Not yet, and why: without a desktop the tools are refused,
  since the node's slots in the vault (question 7) wait for the kernel
  to own the clipboard (`HeliosHost::clipboard`, migration step 2);
  choosing what to copy by name (nodes, parameters) needs endpoints
  keyed per element (egui-pop #73); a paste into the graph or the masks
  answers its plan, its edit applied at the next frame (egui-pop #73
  again); "Paste from agent" in paste special is not offered yet.
