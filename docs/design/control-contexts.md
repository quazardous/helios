# Control contexts: what changes what the controls show

Status: **accepted** (2026-11-09). The study's four **choices** (section
7, C1 to C4) were decided on 2026-11-09, each on its recommended option.
The additions to `helios-controls` (section 6.5) were **built in
slice 5** (2026-11-09): the context keys, tiers and rules
(`helios_controls::context`), `resolve` per card with its placements,
"More (n)" entry, anti-jump and reachability invariant
(`helios_controls::resolve`), and the `controls.context`,
`controls.find`, `controls.reveal`, `controls.reveal_all` commands with
placements in `controls.describe`. Drawing "More (n)" and the context
bar are later slices (controls note section 7). The layout they act on was decided on 2026-11-09 and lives in
[controls.md](controls.md): integration levels (3.1), size policies,
standard slots, orientation and dense packing (3.2), the slot engine,
`egui_fit` (section 4). This note uses those words and does not restate them.
`Mode` and `Representation` are defined in controls.md's terms; the
crate places controls in them through `resolve` (section 6.5).

**The question.** A control has a **mode** (where it lives: panel,
docked, floating, HUD, toolbar) and a **representation** (how much it
shows: full, compact, simplified). A group of controls (a case, or a
module's card) may hide some of its parts. Which situations, the
**contexts**, should change those, who decides, and how does a person
never lose a control because a context hid it?

**Short answer.** A small, typed set of six context keys, plus two
facts handed to every control with them: the **box** its card has and
the **density** (controls note 3.2). Contexts are split into **slow**
ones (task, screen space, detail level), which may fold parts and change
representations, and **fast** ones (tool, device, image state), which
may only add, grow or emphasise. Making a control smaller to fit its
box is not a context rule at all: it is the size policy's own
fallback. The person's saved layout
always beats a group's rules. A hidden part is **folded, never
removed**: it is counted in a "More" disclosure, found by search,
reached by keys, listed to agents with the reason it is folded. A part
whose value is not its default is never folded, and nothing moves under
the pointer or during a gesture.

## 1. What exists

Read on 2026-11-09, Helios's `next`.

| Piece | Where | What it gives |
|---|---|---|
| `Mode` | `helios_controls::model::Mode` | `Panel`, `Docked`, `Floating`, `Hud`, `Toolbar`; declared, not used. |
| `Representation` | `helios_controls::model::Representation` | `Full`, `Compact`, `Simplified`; the renderer takes one (`render::show(ui, control, representation, variant, value)`). |
| `Surface`, slots | `model`, `slot` | Boxed surfaces (panel, dock, toolbar, float) hold slots of ¼ to n base units (controls note 3.2); the HUD's `chip` and the image's `gizmo` are surface forms, not slots. `Footprint` gave way to slots in slice 4. |
| `Variant` | `model` | Swappable forms of one purpose. Each declares a size policy per representation (the slot engine's forms, full first), its orientations (preferred first), its chip and gizmo, and its integration level per surface with its managed fallback. The person picks one (`controls.variant.*` in the note, not built). |
| `card::layout` | `card` | A card or a case (controls and chrome, in order) laid out through `egui_fit` in a rack (its width in slot columns, the row height): each item's slot, orientation, representation and status. What `resolve` (section 6.5) hands the shown controls to. |
| `Readout.when` | `model` | A string, `"active"` today: the only "when" in the crate. |
| `Case`, `Part` | `model` | A composite's parts by name; no rule on them yet. |
| `Registry::describe` | `registry` | What `controls.describe` answers and the gallery card shows. |
| Context keys | `helios_gui::shell::set_context`, `helios_gui::keys` | egui-pop's `when` clauses over keys Helios sets: `place` (darkroom), `mode.orientation` (culling), `focusedPanel`, `maskTool`, `maskPoint`, `maskDisplay`, `heliosProfile`, `canvasActive`, `fullScreen`. Keys are scoped by them today. |
| The context stack | [context-stack.md](context-stack.md) | Places and modes, restored on reopen; a frame changes panels, keys and the toolbox. |
| The gallery | `crates/helios-controls-gallery` | One card per control, its representations side by side over one live value. The **bench** (decided 2026-11-09: controls go from the **shelf**, the gallery's list, to the bench, where a panel is composed; commands `controls.bench.*`) is planned, not on `next` yet; section 5 says what it needs. |
| Sections and overflow | `egui_pop_core::sections`, `egui_pop_api::toolbars` | A section's band with a disclosure mark and a **count** ("Live, 3"), folded and remembered, exposed to AccessKit; a toolbar's "More tools" overflow, its hidden tools marked `overflow` for agents. What a "More (n)" row reuses (section 6.3). |

**What today says.** The vocabulary to answer the question exists: modes,
representations and `when` clauses over context keys. What is missing is
which keys matter to controls, a rule a group can declare, a resolution
that turns keys plus rules plus the person's layout into a placement,
and a guarantee of reachability.

## 2. Candidate contexts

For each: what it is, how Helios could know it, how stable it is. The
stability column matters most: a context that flips every few seconds
and moves things around is the "panels jumping" complaint of section 4.

| Context | Values | How Helios knows it | Stability |
|---|---|---|---|
| **Task** | `cull`, `develop`, `retouch`, `export` | The place and mode on the context stack: culling is `mode.orientation == culling`; the darkroom is `place == darkroom`; retouching is a darkroom with a mask, brush or heal tool active; exporting is the export dialog or queue in focus. A person may also say it (a task switch, as Resolve's pages). | **Slow**: minutes. Changes by an explicit act (entering a place, a mode). |
| **Image kind and state** | kind `raw` or `picture`; `edited` or `untouched`; `masked` (the active node has a mask) | The image's source format (the vault knows it); the history's length beyond the default pipeline; the node's mask members. | **Per image**: changes when the filmstrip moves, and once when the first edit lands. Fast while culling (an image a second). |
| **Active tool** | none, `mask.circle`, `mask.gradient`, `crop`, `picker`, `brush`… | Already a key (`maskTool`); the active control and part (`control.active`, `control.part`, controls note 3.7). | **Fast**: seconds. Changes with every click on a handle. |
| **Screen space** | `narrow`, `regular`, `wide`; the target `desktop` or `web` | Derived from the box the panel actually has, counted in slot columns at the current density, not from the screen: a wide screen with a narrow dock is narrow. The web gallery is a build target, known at compile time. A second screen is a floating window's own box. | **Session**: changes on resize, dock drag, moving a window to another screen. Needs hysteresis: half a base unit. |
| **Density** | `comfortable`, `compact`, `touch` | A person setting; the device may **suggest** one (a touch screen suggests `touch`), never switch it alone. It scales the base unit (controls note 3.2, D1). | **Stable**: weeks. |
| **Detail level** | `essential`, `standard`, `expert` | Only the person: a setting, or a controls profile (darktable-like, Lightroom-like; controls note 3.9). Never inferred from behaviour. | **Stable**: weeks. |
| **Input device** | `pointer`, `pen`, `touch`, `midi`, `keys` | The last event's source (egui gives pointer kind and touch; pen via pressure where the platform reports it); a MIDI backend's connection (controls note 3.8). | **Very fast** for the last event; **stable** for "a MIDI controller is connected". |

**Kept out.** Time of day, the photo's subject (portrait, landscape), the
camera: tempting, but they guess at intent, and a guess that hides a
control is worse than no guess. They may feed *suggestions* (a recipe,
a workshop, ROADMAP 2.5), not the controls' layout.

## 3. What each context could change

Four things can change: the **mode** (where the control lives), the
**representation** (how much it shows), which **parts** of a group show,
and what the **HUD** offers. The matrix says what each context could
reasonably do, and, from section 4, what it should not.

It does not cover what the **size policy** already does (controls note
3.2): a control given a slot below its minimum switches by itself to a
smaller representation or to its managed fallback, and the slot engine
repacks the card. That is a physical effect of the box, the same in
every context, not a rule. Rules act on top of it: they say which parts
show and which representation is *wanted*; the size policy then says
what fits.

| Context | Mode | Representation | Parts of a group | HUD |
|---|---|---|---|---|
| Task `cull` | Develop cards leave the panel; a few controls (exposure, white balance, crop's straighten) may be **docked** as a quick strip, as Lightroom's Quick Develop. | Simplified (a nudge: −⅓, +⅓) | Only the quick parts | Read-outs of rating and flags, not of parameters |
| Task `develop` | Panel (the default) | Full | All the module's essentials, the rest folded per detail level | Summoned chip of the active control |
| Task `retouch` | The active tool's options go to a **toolbar** (as Affinity's context toolbar); the mask case stays in the panel | Full for the tool's parts, compact for the rest | The mask case's parts (feather, opacity, invert) first | Chips of the dragged part (feather, angle) |
| Task `export` | Panel controls read-only or out of the way | Simplified | None but what export uses (size, sharpening for output) | None |
| Image `raw` vs `picture` | — | — | Raw-only parts (demosaic, raw black and white points, highlight reconstruction method) folded on a picture, with the reason "not a raw" | — |
| Image `untouched` | — | — | — | May offer starting points (auto exposure, a preset) |
| Image `masked` | — | — | The node's mask parts shown in its card | Mask chips when a handle is active |
| Active tool | May raise the tool's case to the **toolbar** or the **HUD**; must not take it out of the panel | — | The selected part is **emphasised** (scrolled into view, highlighted), nothing folds | The tool's chips (controls note: summoned, Q2) |
| Space `narrow` | A block (1 × n) may show in a floating popover from its compact row | Nothing by rule: controls that no longer fit their preferred slot shrink through their size policy (above), compact before folding | Unchanged | Chip text shortened |
| Space `wide` / second screen | A rack may **float** on the other screen | Full; growable controls take the extra columns through their size policy | Unchanged | Unchanged |
| Target `web` | No floating windows; docked or panel only | Compact on phones | Unchanged | Touch-sized chips |
| Detail `essential` | — | — | Advanced parts folded under "More" | Fewer chips |
| Detail `expert` | — | Full | Everything shown | Unchanged |
| Device `pen` | — | Unchanged; hit areas grown (already 24 points minimum) | — | Chip placed away from the hand (above the nib, not under the palm) |
| Device `midi` | A bank strip may **dock** to show what the encoders hold (an added rack, in its own slots) | Unchanged in the panel | — | The chip of the turned encoder's control |
| Device `touch` | — | Unchanged; hit areas and chips grown. A smaller layout with bigger targets comes from the `touch` **density**, a person setting the device only suggests | — | Touch-sized chips |

**Reading the matrix.** Two kinds of change appear. Some **fold or move**
(task, detail level, image kind); others only **add, grow or emphasise**
(tool, device, image state). Space changes sizes only through the size
policy; as a slow key it may also fold parts (C1, decided
2026-11-09), once the smallest forms no longer fit. That split matches the stability column in
section 2 and is the core of the proposal (section 6.1).

## 4. Survey: how established apps adapt panels

From the vendors' documentation and the users' forums; paraphrased.
Complaints come from user forums and are anecdotal, but they recur.

### 4.1 Lightroom Classic: modules and solo mode

- **What it does.** The task is the top-level switch: Library, Develop,
  Map, Print… are **modules**, each with its own panels. In Develop, the
  right side is a fixed column of panels (Basic, Tone Curve, HSL…),
  which a person may reorder and hide (Customize Develop Panel, added in
  recent versions). **Solo mode** (right click a panel header) keeps
  only one panel open: opening another closes the previous. Masking has
  its own solo mode, set from its own headers. Library has a **Quick
  Develop** panel: a few develop values as step buttons, the culling
  task's simplified representation.
- **Praised.** Modules match the photographer's steps; Quick Develop
  lets a culler fix exposure without leaving the grid; solo mode tames a
  long column on a laptop.
- **Complained about.** Solo mode hides what one wants to compare: some
  users want to see several panels' changes at once. A panel opened in
  solo mode can open with its top cut off, so people ask it to scroll to
  the header it just opened. On tablets, scrolling the column closes
  panels by accident. Solo mode for masking is hard to find (a second,
  separate switch). Settings that seem to turn solo off and do not.
- **Lesson.** A context-driven fold must be visible (what is folded, by
  what), per area, and must keep the opened thing in view.

### 4.2 Capture One: tool tabs and workspaces

- **What it does.** Tools (Exposure, Levels, Color Editor…) live in
  **tool tabs** the person builds: add a tool to any tab from its menu,
  add or remove tabs, reorder by dragging the icon. Tabs can be shown
  compact (icons only). Any tool can **float**. The whole arrangement is
  saved as a **workspace**, picked from the Window menu; Capture One
  ships several, and a legacy one for the old layout. A "Quick" tab
  gathers favourites.
- **Praised.** Total control; a workspace per job (tethered shoot,
  culling, retouch) is the task context, chosen by the person.
- **Complained about.** The default layout changed between versions and
  people looked for tools that moved; floating tools pile up on small
  screens; building a workspace is work, so many never do.
- **Lesson.** The person's saved layout is the strongest signal; ship
  good defaults per task, and keep a way back to the shipped layout.

### 4.3 darktable: module groups and the quick access panel

- **What it does.** The darkroom's right panel shows **module groups**:
  sets of modules the person defines, with shipped presets (workflow:
  scene-referred, a minimal one, "search only"). A **search box** finds
  any processing module whatever group it is in. The **quick access
  panel** gathers single widgets from any module (exposure's slider,
  color balance's vibrance) in one place, by action path, moving the
  real widget so its keys and history stay. Modules may be hidden; the
  active ones are always listed in their own group.
- **Praised.** The quick access panel is the favourite of many: one
  screen for the everyday edit. Search is the escape hatch that makes
  aggressive grouping safe. Groups as presets let a beginner start small.
- **Complained about.** People lose the group icons or the search box
  through a GUI option and think modules are gone; a module removed from
  every group is hard to find again without search; a second click on a
  group icon showing "all modules" surprises people, who ask to turn it
  off.
- **Lesson.** Search must always be there (never behind a preference),
  and the "active, changed" list is a safety net that makes hiding
  harmless: a changed value is never invisible.

### 4.4 DaVinci Resolve: pages

- **What it does.** Seven **pages** (Media, Cut, Edit, Fusion, Color,
  Fairlight, Deliver), each a complete workspace for one task, switched
  from a bar at the bottom. Within a page, palettes (Color's wheels,
  curves, qualifiers, windows) switch in a fixed strip. Pages a person
  does not use can be hidden, and the page bar reduced to icons on small
  screens. Control surfaces map per page.
- **Praised.** Each page is calm and complete for its job; muscle memory
  is per page; hardware panels follow the page.
- **Complained about.** Newcomers do not know which page a feature lives
  on (the same timeline looks different on Cut and Edit); some features
  exist on two pages with small differences.
- **Lesson.** A task context can change a lot when the switch is
  explicit and named; it must not duplicate a control with different
  behaviour on two tasks.

### 4.5 Blender: workspaces and `poll`

- **What it does.** **Workspaces** (Layout, Modeling, Sculpting, Shading…)
  are tabs that rearrange editors and set a mode. Inside the Properties
  editor, every panel class has a `poll(context)` class method: the panel
  draws only when it returns true (an active object, a mesh, a light).
  Add-ons share a poll through a mix-in; the same pattern gates
  operators and gizmo groups.
- **Praised.** The properties editor never shows a light's settings on a
  mesh: context-sensitive panels feel tidy, and add-ons fit in.
  Workspaces are named tasks the person picks and edits.
- **Complained about.** "Where did the panel go": a panel that polls
  false leaves no trace, so a person who selected the wrong object, or
  is in the wrong mode, sees nothing and no reason. Workspaces switch the
  mode silently and surprise people coming back.
- **Lesson.** `poll` is the right model for *relevance* (a raw-only part
  on a picture), but a hidden thing needs a visible trace and a reason.

### 4.6 Patterns

1. **The task is an explicit switch** in every app (modules, pages,
   workspaces, tool tabs). None infers the task from what you do.
2. **The person's layout wins**, and every app ships one per task.
3. **Fast contexts (selection, tool) only gate relevance** (Blender's
   poll, Affinity's context toolbar), they do not rearrange.
4. **Search is the escape hatch** (darktable, Blender's F3, Resolve's
   effects search): aggressive hiding is safe only with it.
5. **The complaints are the same everywhere**: something hidden with no
   trace, something that moved, something cut off when it opened.

## 5. A small usage test plan

Run on the gallery's **bench** (decided 2026-11-09): a page of the
controls gallery where controls go from the **shelf** to the bench and
a panel is composed from them (a "develop basics" card of exposure,
black level, contrast, targets; a "gradient mask" case of feather,
angle, profile), with a context bar that sets the six keys, the box
(the bench's width, dragged) and the density by hand. Its commands are
`controls.bench.*`. The bench needs: the context bar, the resolution
(section 6.5) applied to the composed panel through the slot engine,
and every control reachable through AccessKit (already true of the
renderers' sliders).

### 5.1 Measures

- **Steps**: input actions from the task's start to the value set (a
  click, a key, a wheel notch burst, a typed value), counted from the
  journal.
- **Reach**: pointer travel between successive targets, from the
  AccessKit node bounds (the sum of distances between centres), and the
  Fitts index of each move (distance over target width).
- **Shift**: how far the next target moved when a context changed (the
  web's layout-shift idea): the displacement of every visible control's
  bounds across the change, weighted by its area.
- **Reachability**: for every context state, every control patched to the
  card is visible or one action away (section 6.4).
- **Time and errors** (humans only): seconds to done, wrong control
  touched, undo used.

### 5.2 Scenarios

| # | Scenario | Compares | Headless? |
|---|---|---|---|
| S1 | Set exposure to +0.70 EV, then feather to 40 px on a gradient | Full, compact, simplified; slider and scrub variants | **Agent**: steps and reach through `ui.*` pointer input on AccessKit bounds; values checked through `controls.describe` |
| S2 | A folded part is needed: set the gradient's profile to sigmoid at detail `essential` | Reaching it by "More", by search, by a key | **Agent**: steps per route; the invariant that it is one action away |
| S3 | Switch task `develop` → `retouch` → `develop` with the pointer over the panel | Rules on vs off; the "no move under the pointer" deferral | **Agent**: shift measured; the controls under the pointer must not move until it leaves |
| S4 | The same card at panel widths 260, 340, 480 points, at each density, and in the web build | Size policy alone vs size policy then folding (C1); gap filling on vs a plain flow | **Agent**: snapshots per width; reachability; no control below its minimum; logical order unchanged by packing |
| S5 | A changed value in a folded part: set a raw-only part, then switch to a picture and back | The "changed is never folded" rule | **Agent**: the part stays visible with its reason |
| S6 | Eight encoders on a bank, task switch | Bank follows the card vs fixed bank | **Agent**: synthetic `nudge` intents; the docked bank strip names what each encoder holds |
| S7 | First use: "make the sky darker with a gradient" with no hint | Detail `essential` vs `standard` vs `expert` | **Human**: think-aloud, time, wrong turns |
| S8 | Ten minutes of culling with quick develop, then developing | Docked quick strip vs none | **Human**: perceived flow, surprise when the panel changed |
| S9 | Pen tablet: mask feathering with the HUD | HUD chip placement rules | **Human**: palm occlusion, comfort; **agent** only checks the chip is inside the view and off the handle |
| S10 | A real MIDI controller on a develop session | Bank strip on vs off | **Human**: does the person know what each knob does without looking at the panel |

### 5.3 What agents run, what needs a human

- **Agents, headless, in CI** (egui-pop's agent bridge, the `controls`
  collector, scenarios with invariants): S1 to S6 counts, distances,
  shifts, reachability, snapshots per width and per context. These are
  regressions tests too: a rule that adds a step or breaks reachability
  fails the scenario.
- **Humans**: S7 to S10, and the judgement half of every scenario
  (clutter, surprise, trust). A small round is enough: five people, two
  of them darktable users, two Lightroom users, one new to raw editing;
  the gallery's web build, so no install. Each does S7 and S8 at two
  detail levels, in an order that alternates; short questions after each (was
  anything missing, did anything move by itself).
- **Numbers decide ties, people decide direction.** If headless measures
  show compact costs one step more than full, that is a fact; whether
  the calm is worth the step is for the human round.

## 6. Proposal

### 6.1 The context keys

Six keys, typed, published both to the controls' resolution and as egui-pop
context keys (so key bindings and menus may use them in `when`):

| Key | Type | Values | Class |
|---|---|---|---|
| `ctl.task` | enum | `cull`, `develop`, `retouch`, `export` | slow |
| `ctl.space` | enum | `narrow`, `regular`, `wide`, derived from the panel's box in slot columns at the current density, with half a base unit of hysteresis | slow |
| `ctl.detail` | enum | `essential`, `standard`, `expert` | slow |
| `ctl.image` | flags | `raw`, `edited`, `masked` | per image |
| `ctl.tool` | string | the active tool's id, empty for none | fast |
| `ctl.device` | enum | `pointer`, `pen`, `touch`, `keys`, `midi` | fast |

The target (`desktop` or `web`) is a build fact, not a key: rules may
test it, nothing changes it at run time.

Two more inputs go with the keys to every resolution, and to every
control as part of its context (controls note 3.2), but are not `when`
keys:

- **The box**: the size the card (or dock, or toolbar) has, in slot
  columns and rows of the base unit. `ctl.space` is its coarse class.
- **The density**: `comfortable`, `compact` or `touch`, a person setting
  the device may suggest. It scales the base unit, so it changes how
  many slot columns a box holds; it is slow (it changes only when the
  person changes it).

**The class rule** (C1, decided 2026-11-09). Slow keys may change a representation and fold
parts. Per-image keys may fold only by **relevance** (a raw-only part on
a picture), like Blender's `poll`, never by taste. Fast keys may only
**add** (a toolbar, HUD chips, a docked bank strip), **grow** (hit
areas, chips) and **emphasise** (scroll into view, highlight); they
never shrink, fold or move a panel control. This keeps the panel still
while the hand works. A control shrinking because its box shrank is
not a rule of any class: it is the size policy (controls note 3.2).

### 6.2 Who decides, and precedence

From the weakest to the strongest:

1. **The control's declaration**: its default representation per mode,
   its size policy and orientations (decided, controls note 3.2), and
   its parts' **tier** (`essential`, `standard`, `expert`).
2. **The group's rules**: a case or a module's card declares rules, each
   a `when` clause over the keys and an effect (a representation, parts
   folded or shown, a mode offer, HUD chips). Rules are data, evaluated
   by egui-pop's `when` evaluator, the one keys use.
3. **The profile or workspace layout**: a shipped or shared layout
   (darktable-like, Lightroom-like), per task when it says so.
4. **The person's saved layout**: order, a part pinned shown, a part
   hidden, a representation picked, a slot size and span, a pinned
   orientation, a pinned position, per card, optionally per task.
   **Always wins over rules**: a rule never folds what the person
   pinned, never shows what they hid. It never wins over the physical
   limits: a control is never given less than its minimum or an
   orientation it does not support; then it falls back (a smaller form,
   its managed fallback) and shows the reason ("Too narrow for a
   vertical fader").
5. **The moment**: the person opened "More" or revealed a part by
   search; it stays open until they leave the card or the task.

Two rules above all of these: a part whose value differs from its
default is **never folded** (darktable's safety net), and a part being
handled is never moved (not even by packing; controls note 3.2, D3).

### 6.3 Folded, never removed

A rule's effect on a part is one of `shown`, `emphasised`, `folded`. A
folded part:

- is **counted** in its group's **"More (n)"** disclosure row, which
  opens in place (no jump: the rows below move down, the pointer's row
  stays put). The row is egui-pop's: a section of
  `egui_pop_core::sections` with its count and disclosure mark (folded
  state remembered, AccessKit expanded state) in a panel or dock, and
  the toolbar's "More tools" overflow (`egui_pop_api::toolbars`) in a
  toolbar. Helios adds the reasons, not a new widget;
- is **found by search**: the command palette's controls search
  (`controls.find`) lists every control of the active node, shown or
  folded, and choosing one reveals and focuses it;
- is **reached by keys**: Tab enters "More"; one command,
  `controls.reveal_all` (bound per keymap), shows every part of the
  card for the moment;
- is **listed to agents**: `controls.describe` and the `controls`
  collector give each instance's placement and, for a folded part, the
  **reason** (the rule's `when` clause and its source: group, profile,
  detail level);
- shows its **reason on hover** of the "More" row ("Raw only",
  "Expert", "Folded while culling").

**Anti-jump.** A placement change that a context causes, a repack by
the slot engine included, is applied when the pointer is not over the
panel and no gesture is open; until then it waits (at most until the
pointer leaves). A part opened by the person keeps its row in view
(Lightroom's cut-off complaint). Packing fills gaps densely (controls
note 3.2, D3), so this rule is what keeps it from moving things under
the hand.

### 6.4 The invariant

> A context never moves or removes a control without a visible way
> back: for every reachable context state, every control patched to the
> active node is either shown, or one action away from a visible
> element that names it or counts it (its group's "More"), and always
> reachable by search, keys and agents.

Checked by a pure function over the resolution (below) for every
combination of the slow keys, in unit tests, and by scenario S2 on the
bench. A rule that would break it is a diagnostic at registration,
like a bad patch today.

### 6.5 Additions to `helios-controls`

Built in slice 5 (2026-11-09), as named below, with these differences:

- The context keys, `Tier`, `Rule`, `Density` and the box live in
  `helios_controls::context`; the box is `CardBox` (so it does not
  shadow Rust's `Box`) and carries its surface. `ctl.image` is
  published as `ctl.image` (the flags joined) and one flag key each
  (`ctl.image.raw`, `ctl.image.edited`, `ctl.image.masked`); the target
  is `ctl.target`.
- A rule's effect also has `inapplicable` (the parts that do not apply:
  the only thing an image-key rule may do) and `emphasise` and `grow`
  (what fast keys may do). `Card::check` and `Control::check` refuse a
  rule that breaks the class rule, tests an unknown key or names an
  unknown part.
- `Visibility` is `Shown`, `Folded { reason }` or
  `NotApplicable { reason }`; emphasis is a mark on the placement, not a
  visibility. A part that does not apply is named in "More", not
  counted. A part the slot engine cannot place is folded with a size
  reason.
- `resolve`, `Placement`, `Layout`, `reachable` and `find` live in
  `helios_controls::resolve`. `resolve` also takes the moment (the
  parts revealed, "show all", and the hand: a gesture open, the pointer
  in the panel, the part handled) and the previous resolution
  (hysteresis and anti-jump). `reachable` checks one resolution;
  `reachable_everywhere` sweeps every slow-key and image combination.
- The commands resolve a node's card (one item per parameter, the
  patched or implicit control) in a context the host may share
  (`commands::register_with`).
- Not built yet: `Readout.when` as a clause, placements in the
  `controls` collector, and the renderers' part (slices 6 to 8).

- **`model::ContextKeys`**: the six keys as a struct of enums
  (`Task`, `Space`, `Detail`, `ImageFlags`, the tool's id, `Device`),
  with `to_context(&self) -> Vec<(String, String)>` for egui-pop's
  context and a `Class` (slow, per image, fast) per key.
- **`model::Tier`** (`Essential`, `Standard`, `Expert`) and a `tier`
  field on `Part`; implicit controls get `Standard`.
- **`model::Rule`**: `{ when: String, source: RuleSource, effect: Effect }`,
  `Effect` being a representation, `fold: Vec<String>` and
  `show: Vec<String>` (part names), `offer: Option<Mode>`, `hud:
  Vec<String>`; `Case` gains `rules: Vec<Rule>`. `Control::check` refuses
  a rule that folds by a fast key.
- **`model::Visibility`** (`Shown`, `Emphasised`, `Folded { reason }`):
  no `Removed` variant, by design.
- **`model::Density`** (`Comfortable`, `Compact`, `Touch`) and
  **`model::Box`** (columns and rows in twelfths of the base unit).
- **`model::Placement`**: `{ mode, representation, integration, slot:
  { units, span }, orientation, position, parts: Vec<(String,
  Visibility)>, hud: Vec<String>, reason: Option<String> }`, the answer
  for one control: where and how big (from the slot engine), which way,
  managed or integrated, which parts show, and why it fell back if it
  did.
- **`registry::resolve(case, keys, box, density, layout, values) ->
  Vec<Placement>`**: a pure function that works **per card** (a case,
  or a module's card): it applies section 6.2's precedence to each
  control, with `values` to keep changed parts shown, then hands the
  shown controls' size policies, orientations and the layout's pins to
  the slot engine (controls note section 4), which packs them into the
  box. `Readout.when` becomes a `when` clause evaluated here.
- **`model::Layout`**: the person's saved layout per card
  (`controls.layout.<module>`, controls note 3.9) with an optional task
  selector: pinned, hidden, representation, order, slot size and span,
  pinned orientation, pinned position.
- **`registry::reachable(control, layout) -> Result<(), String>`**: the
  invariant over every slow-key combination.
- **Commands**: `controls.context` (read the keys, the box and the
  density; set them in tests and on the bench), `controls.find`, `controls.reveal`,
  `controls.reveal_all`; `controls.describe` and the collector gain
  `placement` with reasons.
- **Renderers**: `render::show` already takes a `Representation`; it
  gains the orientation and the slot, and a "More (n)" row for a case's
  folded parts, drawn with egui-pop's sections or toolbar overflow. `Mode`
  stays the host's job (`helios-gui` mounts docked, floating and toolbar
  racks); the crate only says which mode a placement offers.

## 7. Choices

All four were decided on 2026-11-09, each on its recommended option;
the options set aside are kept for the record.

**C1. Which contexts may fold parts. Decided 2026-11-09: B.**
- **Slow keys** (`ctl.task`, `ctl.space`, `ctl.detail`) may fold parts.
- **The image key** (`ctl.image`) folds only the parts that do not
  apply (a raw-only part on a picture), like Blender's `poll`, never by
  taste.
- **Fast keys** (`ctl.tool`, `ctl.device`) only add or emphasise. They
  may grow hit areas or chips, but never shrink, fold or move anything.
- **Size pressure is not a fold rule.** A representation or a slot
  stepping down because the box is too small is a physical effect of
  the size policy (controls note 3.2), the same under every context.
- The options were: A, no context folds (only the person's layout and
  detail level); B, as above; C, every context may fold, the tool
  included. B was recommended: every surveyed app switches its layout
  on an explicit task and gates relevance like Blender's `poll`, and
  the "jumping panels" complaints come from fast changes, which B rules
  out.

**C2. The detail level shipped by default (`ctl.detail`). Decided
2026-11-09: B.**
- The default `ctl.detail` is **`standard`**.
- Rare parts fold under "More (n)".
- A part whose value has been changed is always shown.
- The options were: A, `expert` (everything shown, folding opt-in); B,
  as above; C, `essential` for a new profile, raised by the person. B
  was recommended: darktable users expect to see a module's controls,
  the changed-is-never-folded rule keeps an imported edit fully
  visible, and `essential` is better offered by a profile than
  imposed.

**C3. The person's layout against a group's rules. Decided
2026-11-09: A.**
- The person's layout always wins: on order, visibility,
  representation, span, the orientation pin and the position pin.
- Rules apply only to the parts the person has not placed.
- The layout never pushes a control below its minimum or into an
  orientation it does not support. In that case the control falls back
  (a smaller form, its managed fallback), with a visible reason.
- The options were: A, as above; B, rules win within their context,
  the person's layout being the base outside it; C, one saved layout
  per task replaces rules altogether (Capture One's workspaces). A was
  recommended: it is the only one where a person who pinned a part
  never sees it vanish, and per-task layouts remain possible as a task
  selector on the layout, without dropping the rules' defaults.

**C4. How the screen space is measured. Decided 2026-11-09: A.**
- `ctl.space` derives from the panel's own box, counted in slot
  columns at the current density, with half a base unit of hysteresis.
- The density is a setting the person chooses (`comfortable`,
  `compact` or `touch`). The device may suggest one, never switch it.
- The options were: A, as above; B, per window or screen class (laptop,
  large, web); C, the density alone, with no automatic change of
  `ctl.space`. A was recommended: a wide screen with a narrow dock is
  narrow, only the panel's own box tells, counting in slot columns ties
  the key to what the slot engine can place, and the hysteresis stops
  flicker during a resize.

## 8. Sources

Read on 2026-11-09; vendor docs for what the apps do, forums for what
users say.

- Helios: [controls.md](controls.md), [context-stack.md](context-stack.md),
  [workshops.md](workshops.md); `crates/egui_hud`, `crates/helios-controls`,
  `crates/helios-controls-gallery`, `crates/helios-gui/src/keys.rs`.
- Lightroom Classic: [Workspace basics](https://helpx.adobe.com/lightroom-classic/desktop/workspace/workspace-basics.html);
  forum threads on solo mode: [several panels at once](https://community.adobe.com/t5/lightroom-classic-ideas/p-lightroom-classic-panel-scroll-bar-close-annoyance/idi-p/12248801/page/4),
  [scroll to the opened header](https://community.adobe.com/feature-requests-676/p-it-would-be-nice-if-when-using-solo-mode-on-the-side-panels-it-would-auto-scroll-down-to-the-selected-panel-header-as-often-i-find-the-top-of-the-selected-panel-cut-off-when-opened-1639454),
  [solo mode in masking](https://community.adobe.com/t5/lightroom-classic-discussions/solo-mode-not-working-in-masking-section-of-lrc/m-p/14194761);
  [Customize Develop Panel](https://lightroomkillertips.com/customize-develop-panel/).
- Capture One: [maximising the image area with a custom workspace](https://www.captureone.com/blog/maximizing-image-area-custom-workspace),
  [workflow ordering of tools](https://support.captureone.com/hc/de/community/posts/360012650297-Workflow-ordering-arranging-tools),
  [tool tabs customisation request](https://support.captureone.com/hc/de/community/posts/360012203177-Tool-Tabs-Customization-Request).
- darktable: [module groups](https://docs.darktable.org/usermanual/development/lo/darkroom/organization/module-groups/),
  [manage module layouts](https://docs.darktable.org/usermanual/4.2/en/darkroom/organization/manage-module-layouts/),
  [organization overview](https://docs.darktable.org/usermanual/5.6/en/darkroom/organization/overview/);
  `src/libs/modulegroups.c`; forum threads:
  [module group icons gone](https://discuss.pixls.us/t/darkroom-module-group-icons-tabs-is-gone-solved/16952),
  [show all modules on a second click](https://discuss.pixls.us/t/can-i-disable-show-all-modules-when-second-clicking-on-module-group-icon/56186).
- DaVinci Resolve: the Reference Manual, "Hide Pages You Don't Use";
  [pages explained](https://www.tourboxtech.com/en/news/davinci-resolve-pages-explained.html).
- Blender: [Panel](https://docs.blender.org/api/current/bpy.types.Panel.html)
  (`poll`, mix-ins), [GizmoGroup](https://docs.blender.org/api/current/bpy.types.GizmoGroup.html).
