# Workshops v1: an experimental surface on macros

Status: **accepted, being built** (2026-11-05). The owner decided on
2026-11-05 to build workshops v1 as an **experimental** feature,
without the mockup test with photographers that ROADMAP section 2.5 and
[NEEDS.md](../NEEDS.md) planned first, and accepted this plan with its
defaults. Built: steps 1 to 3 of the plan (the switch and the empty
panel, the workshop block in the format, aiming). Each choice has a
default; the open questions at the end are for the owner to settle.

## What a workshop is

ROADMAP section 2.5 names three things:

- a **recipe**: a diagnosis leading to a tool ("too contrasty with the
  blue sky" -> tone and a sky mask);
- a **workshop**: a customised recipe, a mini workspace of real modules,
  pre-calibrated for the photo, showing only the useful sliders
  (restricted ranges, tested at the extremes), saved and reused because
  it stores the logic (relative targets, zone detection), not absolute
  values;
- a **retouch**: a workshop applied to an image.

In AVS terms a workshop is a **macro**: a network of modules packaged
as one, exposing only chosen parameters
([module-contract.md](../module-contract.md), "Macros"). Macros are
built and released ([macros.md](macros.md), all six steps): made from a
selection, an interface editor (labels, narrowed ranges, soft ranges,
groups), folded and opened in place, versioned files, a user library.
So the half of a workshop that is "real modules with only the useful
sliders" exists today. What a macro does not have is the other half:

1. **an intent**: what the workshop is for, in the user's words, so it
   can be found by what one wants rather than by module names;
2. **relative targets**: a parameter whose value is found per image
   from a measure ("midtones at this brightness", "this much clipping
   at most"), so the same workshop gives each image its own value;
3. **a surface of its own**: a place to find, apply and tune
   workshops without the graph or the Modules panel.

Workshops v1 adds exactly these three, on top of macros, behind an
experimental switch.

## v1: what it includes and what it leaves out

**In v1:**

- A **workshop block** in a composite file: intent, keywords and
  relative targets. A workshop is a macro whose file has this block;
  everything else (ports, inner nodes, exposed parameters, versions,
  the user library, publishing) is the macro's, unchanged.
- **Relative targets** on exposed number parameters, measured with the
  scopes' counts on a small proxy render, solved by bisection over the
  parameter's narrowed range. Three measures only (see "Relative
  targets").
- **Saving a macro as a workshop** (`workshop.save`): a new version of
  the macro with the block, as the interface editor saves one.
- **Applying** (`workshop.apply`): the workshop's macro inserted after
  the node the image is shown from, its targets aimed on the open
  image, as one undo step. The applied node is the retouch.
- **Re-aiming** (`workshop.aim`): the targets solved again on the open
  image, as one undo step.
- **Finding by intent without a model** (`workshop.suggest`): the
  user's words matched against workshops' names, intents and keywords.
  This is the AI's place, kept as a stub.
- **The Workshops panel**, declared only when the experimental switch
  is on.
- **One shipped workshop**, so the panel is never empty and the tests
  have a fixture.
- **MCP verbs** for each command, refused with the reason when the
  switch is off.

**Left out of v1** (each a later step, none decided):

- **The AI designing a workshop** from an intent (a `workshop.design`
  decision through `helios-ai`, a prompt with an input and an output
  schema). v1's `workshop.suggest` only finds existing ones.
- **Composite semantic sliders** (one slider driving several inner
  parameters along a curve). Today one exposed name on several inner
  nodes links them to one value; that is what v1 offers.
- **Zone detection** (a target measured in a mask: "the sky", "the
  face"). It needs segmentation, which belongs to decision nodes and
  the AI models; v1 measures the whole image.
- **Aiming a series**: applying to many images, each aimed on its own,
  as a job (ux-research S2, "one look across the series"). v1 aims the
  open image; another image is aimed when it is opened and
  `workshop.aim` runs.
- **Recording a target's provenance** in the image's edit (that a value
  was aimed, at which target). v1 writes plain values.
- **The mockup test.** The owner chose to build first; measuring time
  saved stays a later question (open question 8).

## The data model

### A workshop is a macro with a block

A composite file (`<id>@<version>.helios-composite.json`, `CompositeFile`
in `helios-template/src/format.rs`) grows one optional field,
`workshop`:

```json
{
  "format": 1,
  "id": "helios.lift-midtones",
  "version": 1,
  "name": "Lift midtones",
  "description": "Brighter midtones, highlights kept",
  "inputs": [ { "name": "input", "channels": 4, "layout": "rgba" } ],
  "outputs": [ { "name": "output", "channels": 4, "layout": "rgba" } ],
  "nodes": [
    { "name": "exposure", "module": "exposure" },
    { "name": "tone", "module": "sigmoid" }
  ],
  "connections": [
    { "from": "@", "port": "input", "to": "exposure", "input": "input" },
    { "from": "exposure", "port": "output", "to": "tone", "input": "input" }
  ],
  "exits": [ { "from": "tone", "port": "output", "output": "output" } ],
  "exposed": [
    { "name": "brightness", "node": "exposure", "param": "exposure",
      "label": "Brightness", "unit": "EV", "min": -3.0, "max": 3.0 },
    { "name": "contrast", "node": "tone", "param": "contrast",
      "label": "Contrast", "min": 1.0, "max": 3.0, "soft": [1.2, 2.2] }
  ],
  "workshop": {
    "intent": "Brighter midtones without losing the highlights",
    "keywords": ["brighten", "lift", "midtones", "underexposed"],
    "targets": [
      { "param": "brightness", "measure": "luma.median",
        "value": -2.5, "at": "output" }
    ]
  }
}
```

- `intent` (English, free text) and `keywords` (English words) are
  what `workshop.suggest` matches; translations go through the i18n
  catalogues as other shipped texts do.
- `targets` lists relative targets, each on one **exposed** parameter
  (by its exposed name), at most one target per parameter.
- The block is optional, so files stay `format: 1` (the rule
  macros.md's open question 6 settled for the interface fields): an
  older Helios refuses a file with it (unknown field), which is the
  honest outcome; this Helios reads it whether the switch is on or not,
  and with the switch off a workshop works as the plain macro it is.
- The JSON schema (`templates/schema/composite.schema.json`) gains the
  block; schema and serde are tested together as for the interface.

### Relative targets

A target says what the image should measure, not what the parameter
should be. v1 has three measures, all from `helios-scopes`' luminance
histogram on the working data (linear Rec.2020):

| `measure` | what it is | unit of `value` |
|---|---|---|
| `luma.median` | the median luminance | stops around 1.0 (log2) |
| `luma.high` | the 99th percentile of luminance | stops around 1.0 |
| `clip.high` | the share of pixels above 1.0 | % |

- `at` is `output` (the workshop's output, default) or `input` (what
  enters it, for a target that is a fixed offset from the input:
  rarely needed, kept for symmetry and tests).
- **Aiming** solves each target in the file's order: the other exposed
  parameters at their current values, the target's parameter is
  searched by bisection over its narrowed range (`min`/`max`, else the
  inner hard range). Each probe renders the workshop on a proxy of the
  open image (long side 256 px, the input cached, so a probe costs the
  workshop's own steps). At most 12 probes; the result is rounded to the
  parameter's step.
- **Tested at the extremes**: the two ends are probed first. If the
  target is not between them, the nearer end is taken and the answer
  says so ("brightness: target not reached, kept at 3.0 EV"). If the
  measure does not move monotonically between the ends (both ends on
  the same side of a probe in the middle), the parameter keeps its
  current value and the answer says why.
- **Deterministic**: the same input (by content key), workshop version
  and targets give the same values; the answer is cached by those keys
  for the session.
- **Where it lives**: `helios_template::workshop::aim`, a pure function
  given a render closure (`Fn(&Params) -> Result<Image, String>`), the
  targets and the ranges. The desktop and the node pass a closure over
  their engine; tests pass synthetic images. No new crate.

### Parameters the user sees

The exposed parameters, exactly as the macro's interface defines them:
labels, units, narrowed and soft ranges, groups. A targeted parameter
is a normal slider; moving it by hand is allowed and kept (it is the
user's value now, until the next aim). The panel marks a targeted
slider with the target ("aimed: midtones at -2.5 stops").

### Saving and reuse

- **Saving** a workshop is saving a macro version: `workshop.save` on a
  macro node writes its next version with the block (the macro's
  `next` reference from `State::macro_interface`: a user's macro gets
  version n+1, a shipped one a copy of its own under `user.`), and pins
  the node to it, as the interface editor does.
- **Reuse** is the macro's: the workspace's composites, then
  `macro.publish` into the user's library (`<config>/composites/`),
  which every workspace's catalogue reads. Templates bundle the
  composites they use, workshops included.
- **What the image's edit keeps**: values only, under
  `node/<workshop node>/<exposed>`, as for any macro. The workspace pins
  the version. The logic (the targets) lives in the file, so another
  image re-aimed gets its own values, which is the point of "relative
  targets, not absolute values".

### Applying: where the node goes

`workshop.apply` (`workshop`: `id@version`, `after`: a node, by default
the node the viewport shows the image from, `aim`: true) is one pure
step on the workspace, `State::workshop_applied`, beside the macro
steps in `helios_template::macros`:

- the macro's node is added (named after the workshop, a free name) and
  inserted on the wires leaving `after`: every reader of `after`'s
  output that the workshop's input kind fits now reads the workshop;
- a workshop with more than one input is refused for v1 ("this
  workshop needs a mask input; add it from the graph");
- with `aim`, the targets are solved and their values are the step's
  values for the open image.

## The desktop

### The switch

- A setting, `interface.experimental.workshops`, a toggle, user taste
  concern (`Concern::User`), off by default, declared with Helios's
  other settings so it shows on the generated settings page under an
  "Experimental" group, and can be set in any layer the concern allows,
  the run layer included (one session with workshops, from the command
  line).
- Read when the shell starts: on, the Workshops panel is declared and
  the `workshop.*` commands are registered; off, neither exists, and
  the View menu shows nothing about them. Changing it says "takes
  effect at the next start" (open question 2).
- Off, files with a workshop block still load and render; the Modules
  panel and the graph show them as macros.

### Marked experimental

- The panel's title is "Workshops", and its header carries a line
  "Experimental: may change; what it saves stays a macro".
- The commands are in a command-palette category "Experimental"
  (`CommandSpec`'s category), so the palette shows the word.
- The changelog fragment of each step starts with "Experimental:".
- The MCP verbs say "experimental" in their descriptions and refuse with
  the setting's key when it is off.

### The Workshops panel

`helios.workshops`, end side (with Parameters), closed until opened,
icon `phosphor:flask`. Three parts, top to bottom:

1. **Find.** A text field: typing filters the list by name, intent and
   keywords (`workshop.suggest`, ranked: name, then keywords, then
   intent words). Empty, it lists every workshop in the workspace's
   catalogue (shipped, the user library, the workspace's own), grouped
   by where it comes from.
2. **The list.** One row per workshop: name, intent on a second line,
   version. Apply on the row (or Enter) runs `workshop.apply`. A
   workshop already applied to the network shows as applied and selects
   its node instead.
3. **The applied workshop** (when the selected node is a workshop, from
   the graph, the Modules panel or the list): its exposed parameters as
   the Parameters panel draws a macro's (by group), each targeted one
   marked with its target; Aim again (`workshop.aim`); Open in graph
   (`macro.open`); Remove (`network.remove`).

On a macro node without a block, the Modules panel's and the graph's
node menu gain **Save as workshop…** (only when the switch is on): a
form dialog (egui-pop's `Form`) with the intent, the keywords and, per
exposed number parameter, a target (none by default; a measure and a
value, the value prefilled with what the open image measures now).

### Commands and MCP verbs

| Command | Arguments | MCP verb |
|---|---|---|
| `workshop.suggest` | `intent` | `helios_workshop_suggest` |
| `workshop.apply` | `workshop`, `after`, `aim`, `dry_run` | `helios_workshop_apply` |
| `workshop.aim` | `node`, `dry_run` | `helios_workshop_aim` |
| `workshop.save` | `node`, `intent`, `keywords`, `targets`, `dry_run` | `helios_workshop_save` |

`helios_describe` lists each composite's workshop block. Agents use the
same steps and refusals as the user, as for macros.

## Undo and history

- `workshop.apply` is **one undo step** ("apply workshop Lift
  midtones"): the network change and the aimed values together, through
  `Shell::workspace_step_moving` as the macro steps do, so Ctrl+Z gives
  back the network and the image's sliders at once.
- `workshop.aim` is **one value step** ("aim Lift midtones") on the
  image's edit, in the History panel like a slider change; nothing in
  the workspace changes.
- `workshop.save` is one workspace step, as the interface editor's new
  version is; undo removes the file it added. Publishing stays
  `macro.publish`: never undone, asked first.
- Moving an applied workshop's sliders is ordinary value steps.
- A dry run of each answers what would change (node, values, file)
  without changing anything.

## Tests

Following the test cycle: fmt, check and clippy locally, the full runs
on the bench box.

- **Format** (`helios-template`): the block round-trips; the schema
  accepts and refuses the same files as serde; refusals: a target on a
  parameter that is not exposed, on a non-number, two targets on one
  parameter, an unknown measure.
- **Aiming** (`helios-template`, synthetic images, no engine): a ramp
  reaches `luma.median` within half a step in at most 12 probes; the
  same inputs give the same value twice; a target outside the range
  gives the nearer end and the note; a non-monotonic measure keeps the
  value and says why.
- **Pure steps** (`helios-template/tests`): `workshop_applied` inserts
  the node on every reader of `after`; refuses two-input workshops;
  the network after the step renders as the flat network the macro
  packages, with the same content keys (the macros' rule).
- **Desktop** (`helios-gui/tests`, headless, as `macros_desk.rs`):
  switch off: no panel, `workshop.*` unknown, MCP verbs refused with the
  key; switch on: apply is one undo step and Ctrl+Z restores network
  and values; aim is one value step; save writes version n+1 and undo
  removes it; suggest ranks name over keyword over intent.
- **Node** (`helios-node`): each MCP verb, on and off.
- **The shipped workshop**: aimed on two corpus images of different
  brightness, the two `brightness` values differ and both medians land
  on the target (a bench check on the box, not a unit test).
- **wbox**: the panel's screenshot in a headless wbox session, default
  features only.

## Plan

One commit per step, each building and passing its tests, each with a
changelog fragment ("Experimental: ..."), the docs it changes and its
MCP verbs. Sizes: S under 300 lines, M 300 to 800, L over 800, tests
included.

1. **The switch** (S). `interface.experimental.workshops` declared,
   read at start; an "Experimental" command category; an empty
   `helios.workshops` panel declared only when on, with its experimental
   header; headless tests on and off.
2. **The workshop block in the format** (S). `Workshop`, `Target`,
   `Measure` in `helios-template/src/format.rs`, the schema, the
   refusals; `helios_describe` lists the block.
3. **Aiming** (M). `helios_template::workshop::aim` over a render
   closure, the three measures from `helios-scopes`, bisection with the
   ends tested first, the notes; synthetic-image tests.
4. **The shipped workshop** (S). `helios.lift-midtones@1` in
   `templates/composites/`, in the desktop's catalogue; a test that it
   builds and aims.
5. **Apply** (M). `State::workshop_applied` (insertion after a node,
   the refusals), `workshop.apply` as a desktop step carrying the aimed
   values, the engine-backed render closure on a proxy, the MCP verb,
   undo tests.
6. **The panel** (M). Find, the list, the applied workshop's sliders
   and targets, Aim again (`workshop.aim`, its value step and verb),
   `workshop.suggest` and its verb; wbox screenshot.
7. **Save as workshop** (M). `workshop.save` as the macro's next
   version, the form dialog from the Modules panel and the graph's node
   menu, the target prefilled from the open image's measure, the verb,
   undo tests.

Steps 1 to 4 need no GUI beyond the empty panel and can land in any
order after 1; 5 to 7 in order. Total: about three S and four M.

After v1, in no fixed order and not decided: the AI designing a
workshop (`workshop.design`, a decision through `helios-ai`, tested with
its fake provider); aiming a series as a job; targets measured in a
zone (a mask from a decision node); composite semantic sliders;
recording an aim's provenance in the edit.

## Open questions

Each with the default proposed:

1. **Is "workshop" the user-facing name?** Yes, as ROADMAP uses it,
   with "retouch" kept internal (the applied node is just "the
   workshop" in the UI). Others: "Recipe", "Look".
2. **Switching on at run time or at the next start?** At the next
   start: the panel list and the command list are built once. Live
   switching would need egui-pop to add and remove a panel and
   commands while running.
3. **Where an applied workshop goes:** after the node the image is
   shown from. Other: before the display transform (sigmoid) when the
   network has one, so a workshop works on scene-referred data; that
   needs a role on nodes the network does not carry yet.
4. **Should the edit record that a value was aimed?** No for v1
   (plain values, as for macros' open question 10). Yes later would
   let a series re-aim only the values that were aimed, not the ones
   the user moved.
5. **Should applying aim by default?** Yes (`aim: true`); a workshop
   whose targets are not wanted is applied with `aim: false` from the
   MCP verb, or its sliders moved after.
6. **May an agent save workshops?** As for macros: save inside a
   workspace, yes (undoable); publish only with the user's confirmation
   in that session.
7. **The three measures:** `luma.median`, `luma.high`, `clip.high`.
   Others asked for early (saturation, white balance by grey world)
   would come as new measures, one each, without a format change.
8. **Measuring the time saved:** the hypothesis NEEDS.md wanted tested
   first is not tested by building. Proposed: once v1 is in, About's
   "report a problem" link and the issue list gather feedback from the
   few users who switch it on; no formal study unless the owner asks
   for one.
9. **When does it stop being experimental?** When the AI design step
   or the series step is built and the panel has been used on real
   series; the switch then defaults to on, and later goes away.
