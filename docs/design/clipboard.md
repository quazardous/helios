# Copy and paste on the Helios side: what is copied, how it is adapted, agents and nodes

Status: **proposed** (2026-10-18), to study. The owner accepted the
analysis's direction: an internal clipboard versus the system one, and
hooks that process and transform what is copied and pasted. Nothing is
built. The generic part (the contract between kernel and interface,
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
