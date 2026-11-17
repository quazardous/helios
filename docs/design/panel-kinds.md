# Panel kinds: what shows, what edits

Status: study (2026-11-17), options A, B and C for the owner; nothing
built.

The owner's ask (2026-11-17): one cannot tell which panels only show and
which change the render. It must be obvious at a glance. The example: the
histogram can be dragged to change the image, the scatter-like scopes
(waveform, parade) cannot, and nothing on screen says so.

This study lists every panel and window with each interactive area
classed, looks at how four established editors show the difference, and
proposes one mechanism drawn by the framework, never by each panel.

## 0. Words

An interactive area is one of three kinds:

- **display**: shows something, writes nothing (a scope's picture, the
  log, a header's status).
- **navigate**: moves what is shown without touching the photo: pan and
  zoom, the selection, a scope's mode, the mask display. Its values are
  transient view values (`view/...` in the value store), never an undo
  step of the edit.
- **edit**: writes the photo's edit, so the render changes: a parameter, a
  bypass, a mask handle, the histogram's drag. Each one is an undo step
  of the edit (`helios_values::Store`).

Some areas write data that is not the render: the Library's marks
(rating, flag, labels, tags), the import's choices, settings. They are
called **catalogue** areas below. They are not part of the owner's
question (they do not change how the photo looks) and they already look
like forms or buttons; they are listed so the inventory is complete.

A panel is then:

- **view**: only display and navigate areas;
- **control**: its purpose is editing the render (it has edit areas, and
  its display areas only serve them);
- **mixed**: a view panel with one or more edit areas inside.

## 1. Inventory

The panels are the ones Helios declares in `helios_panels_with`
(`crates/helios-gui/src/lib.rs`), plus the panels and windows the shell
opens. "From" says who draws the interactive part: egui-pop (`ep`),
egui_hud through the controls layer (`hud`, `helios-controls`, #4861) or
Helios itself (`helios`).

### 1.1 Darkroom panels

| Panel (id) | Area | Kind | From | Code |
|---|---|---|---|---|
| Image (`helios.viewport`) | the picture, before and after, pixel readout | display | ep `viewport` | `viewport.rs` |
| | pan, zoom, fit, before/after split | navigate | ep `viewport` | `viewport.rs` (`view/viewport/*`) |
| | the selected mask's shape handles (circle, ellipse, gradient) | edit | hud `helios_controls::gizmo` on ep overlays | `masks.rs` |
| | a path's points and Bézier handles | edit | helios (until its case moves to `gizmo`) on ep overlays | `masks.rs` |
| | the mask display (overlay, grey) | navigate | helios | `mask_display.rs` |
| **Panel kind** | | **mixed** | | |
| Scopes (`helios.scopes`) | Histogram / Waveform / Parade, Stops toggles | navigate | ep `selectable_label` | `scopes.rs` (`ui_with`) |
| | the histogram: drags the network's exposure (one undo step, `histogram.drag` step, Escape gives up) | edit when an `exposure` parameter exists, else display | ep `viewport::histogram` (`draggable`) | `scopes.rs` (`histogram_ui`, `drag_exposure`) |
| | the waveform, the parade | display | helios (a texture) | `scopes.rs` |
| **Panel kind** | | **mixed** | | |
| Navigator (`helios.navigator`) | zoom control, frame drag | navigate | ep `viewport::navigator` | `navigator.rs` |
| **Panel kind** | | **view** | | |
| Parameters (`helios.params`) | each parameter's control (slider, scrub, chip), its changed dot that resets, context menu | edit | hud cards (`helios_controls::render`) and ep sliders | `params.rs` |
| | curves, colour wheels, lists, text fields | edit | ep `viewport::curve::CurveEditor`, `viewport::wheel::ColorWheel`, drawn as integrated parts | `params.rs` |
| | a parameter's picker button (`param.picker`) | edit (arms a sampling tool) | helios | `params.rs` |
| | the module header's mark and Reset (`node.reset`) | display + edit | helios | `params.rs`, `edit_marks.rs` |
| **Panel kind** | | **control** | | |
| Modules (`helios.modules`) | a row selects its module (`view/graph/selection`) | navigate | ep list | `modules.rs` |
| | the Bypass toggle (`node.bypass`) | edit | ep toggle | `modules.rs` |
| | the changed mark after the name | display | helios | `modules.rs`, `edit_marks.rs` |
| **Panel kind** | | **control** | | |
| Masks (`helios.masks`) | the tree: selection | navigate | ep `TreeView` | `mask_tree.rs` |
| | operator, inversion, opacity, lock, adding and removing members | edit | ep `TreeView` rows | `mask_tree.rs` |
| | the mask display button | navigate | helios | `mask_display.rs` |
| **Panel kind** | | **control** | | |
| Graph (`helios.graph`) | nodes, wires, ports: add, remove, connect (`graph.*` steps, then `network.*`) | edit | ep `viewport::graph` | `graph.rs` |
| | a node's sliders | edit | ep | `graph.rs` |
| | selection, entering a sub-workspace, pan and zoom | navigate | ep | `graph.rs` |
| | the headers' engine status | display | ep | `graph.rs` |
| **Panel kind** | | **control** | | |
| Decisions (`helios.decisions`) | Accept, Correct (the answer's form), Accept all above | edit (the answer lets the render go on) | helios on ep forms | `decisions.rs` |
| **Panel kind** | | **control** | | |
| Workshops (`helios.workshops`, experimental) | describe, apply, the session's notes | edit (a workshop's steps write the open image) | helios | `workshops.rs`, `shell/workshop_session.rs` |
| **Panel kind** | | **control** | | |
| Queue (`helios.queue`) | the rows, the summary | display | helios | `queue.rs` |
| | Cancel (`job.cancel`) | catalogue (a job, not the render) | helios | `queue.rs` |
| **Panel kind** | | **view** | | |
| Log (`helios.log`) | the entries | display | helios | `log_panel.rs` |
| **Panel kind** | | **view** | | |
| History (egui-pop's history view, placed by #5202) | the steps listed | display | ep | `egui-pop-api/src/history_view.rs`, `history.rs` |
| | a click jumps (`edit.jump`), undo, redo | edit (moves the edit back or forth) | ep | `history.rs` |
| **Panel kind** | | **control** | | |

### 1.2 Library panels

| Panel (id) | Area | Kind | From | Code |
|---|---|---|---|---|
| Library (`helios.vault`) | the grid, browse sections, filter, facets | navigate | ep `views::library` | `library.rs`, `library/` |
| | marks: rating, flag, labels, tags, stacks; drag and drop into albums | catalogue | ep library steps (`library.rate`...) | `library.rs` |
| | "Fit a camera look from the selection" | edit, indirect (a job writes a camera look other photos render with) | helios | `shell/camera_look.rs` |
| Filmstrip (`helios.filmstrip`) | selection | navigate | ep `ThumbGrid` | `library.rs` (`filmstrip`) |
| | marks | catalogue | ep | `library.rs` |
| | the "edited" pill (#5199) | display | ep | `library.rs` |
| Compare (`helios.compare`) | the photos, synced zoom and pan, the active photo | navigate | ep `Compare` | `comparison.rs`, `library.rs` (`compare_ui`) |
| | best, culling keys | catalogue | ep | `comparison.rs` |
| Stacks (`helios.stacks`) | Accept all, Accept the checked, Decline, Suggest | catalogue | helios | `stack_review.rs` |
| Duplicates (`helios.duplicates`) | keep or skip, file by file | catalogue | helios | `duplicates.rs` |

All five are **view** for the render question.

### 1.3 Shell items and windows

| Item | Kind | Code |
|---|---|---|
| Status bar: library count, zoom, history length | display (the zoom's card: fit, fill, 100 %, navigate; the history's card: undo, redo, edit) | `status_items.rs` |
| Status bar: the edit's mark (#5199) | display, its card's "Reset all" edit, before/after navigate | `status_items.rs`, `edit_marks.rs` |
| Settings app (Appearance, theme editor, health, storage...) | catalogue; the wide-gamut preview changes the display, not the edit (navigate) | `config.rs`, `theme_editor.rs`, `shell/health.rs`, `shell/storage.rs`, `display.rs` |
| Import window, import wizard, init wizard | catalogue | `shell/import_window.rs`, `shell/import_wizard.rs`, `shell/init_wizard.rs` |
| Library focus, Activity focus | as the Library panel; jobs (catalogue) | `shell/library_focus.rs`, `shell/activity_focus.rs` |
| Calibration focus | the chart's four corner handles and the profile set as a camera's default: edit, indirect (other photos' render) | `shell/calibrate_focus.rs` |
| Export dialog | catalogue (writes files, not the edit) | `shell/export.rs` |
| Rule editor, macro and workspace editing | catalogue; macro steps move the open image's values with the network (edit) | `rule_editor_ui.rs`, `shell/macro_edit.rs`, `shell/workspace_edit.rs` |

### 1.4 What is shown today

- **Panel chrome** (egui-pop's workbench, tab titles through its
  `TabViewer::title`, `egui-pop-api/src/workbench.rs`) carries the title
  and icon of `PanelSpec` (`egui-pop-model/src/panels.rs`); it has no field
  saying what a panel does. Every panel looks the same.
- **The histogram** already behaves as a control when draggable
  (`egui-pop-viewport/src/histogram.rs`): `Sense::click_and_drag`, the
  `ResizeHorizontal` cursor on hover and while dragging, an outline on
  hover, the focus ring, and the accessible role `Slider`. When not
  draggable it is `Sense::hover`, no cursor, role unknown. **At rest the
  two look the same**: nothing tells the draggable histogram from the
  waveform beside it until the pointer is over it, and nothing says what
  the drag writes (exposure) or by how much.
- **Sliders, curves, wheels, mask handles** look editable by their shape;
  the controls layer adds the changed dot and the reset (#5199).
- **The waveform and the parade** are plain images (`egui::Image`): no
  cursor change, no hover; nothing says they are view-only either.
- **A mixed panel** (Scopes, Image) says nothing in its header.
- **Navigate areas** (pan, zoom, selection) are never undo steps; edit
  areas always are. The difference exists in the value store
  (`view/...` against the edit's paths) but is not drawn.

Note: darktable's waveform and parade are draggable (section 2.1);
Helios's are not, which matches the owner's expectation. Whether they
should be is a separate question (follow-up 6).

## 2. How other editors show it

### 2.1 darktable

- The **scopes** module: dragging the right part of the histogram
  changes the exposure, the left part the black level; in the waveform
  and the RGB parade, the upper part changes the exposure and the lower
  part the black level; the scroll wheel does the same; a double click
  resets. The vectorscope is not listed as adjustable. The manual
  describes no cursor or on-screen sign for these regions; the buttons of
  the module appear on hover only ([darktable manual, scopes][dt-scopes]).
- **Processing modules** carry a header with the on/off button, the
  reset and the presets menu; a mask toggle appears only when a mask is
  active; on-image overlays (crop guides, shape handles) show only while
  the module has the focus ([darktable manual, module header][dt-header]).
- So: editing is announced by the module's chrome (power button, reset)
  and by handles on the image; the scope's drag is a hidden power-user
  feature.

### 2.2 Lightroom Classic

- The **histogram** is editable: on hover a tonal region (blacks,
  shadows, exposure, highlights, whites) turns light grey and the name of
  the control and its current value are shown; dragging moves the
  matching Basic panel slider ([Adobe, tone control][lr-tone]; [Julieanne
  Kost, Basic panel and histogram][lr-kost]).
- The **Targeted Adjustment tool** is a small circular icon in the corner
  of the Tone Curve and HSL panels; once armed, dragging on the image
  edits that panel and the curve shows the point ([Julieanne Kost, Tone
  Curve panel][lr-tat]).
- So: an editable graph says what it edits on hover (region + name +
  value); edit tools are armed by an icon in the panel they edit.

### 2.3 Capture One

- Every **tool** has a title bar with a reset icon (Alt+click previews
  without the tool), a presets menu (`...`) and a help icon
  ([Capture One, icons][c1-icons]; [Thomas Fitzgerald, tool panel
  icons][c1-tf]).
- The **Histogram** tool is display-only; the **Levels** tool draws its
  own histogram with black, mid and white handles and pickers on it
  ([Image Alchemist, histograms in Capture One][c1-hist]; [Image
  Alchemist, Levels tool][c1-levels]).
- So: a graph that edits always shows **handles at rest**; a graph without
  handles is read-only. Edit tools carry a reset in their header, view
  tools do not.

### 2.4 Photoshop

- The **Histogram panel** is display-only. When it shows cached data
  rather than the document's current state it carries a warning triangle
  ("Cached Data Warning"); clicking it refreshes ([Adobe Photoshop CS4
  manual, histogram][ps-cache]; [Photoshop Training Channel][ps-ptc]).
- **Curves** (in the Properties panel) has an on-image adjustment tool,
  a hand icon in the panel; once armed, dragging on the image edits the
  curve and a dot on the curve follows the pixel ([Julieanne Kost,
  Curves][ps-kost]).
- So: view panels are plain; editing on the image goes through an armed
  tool named in the panel it edits; staleness is a header mark.

### 2.5 Patterns

1. **Handles at rest** mean editable (Capture One Levels, curves
   everywhere); a graph with no handles reads as a picture.
2. **Hover names what will change** (Lightroom: region, control name,
   value). Cursor alone is not enough: none of the docs relies on it.
3. **Edit chrome**: tools that edit have a reset (and in darktable an
   on/off) in their header; view panels have none (darktable, Capture
   One).
4. **Edit on the image is armed** from the panel it edits (Lightroom's
   and Photoshop's targeted tools, darktable's module focus).
5. **Staleness** is a header mark (Photoshop's cached data warning),
   which meets #5205.
6. None of the sources describes colouring control panels differently.

## 3. Proposal

### 3.1 A declared kind, drawn by the framework

- **Per panel**: `PanelSpec` (egui-pop) gets a `kind`: `View`, `Control`
  or `Mixed`, declared with the panel (`helios_panels_with`). Helios
  declares it; egui-pop's chrome draws it. No panel draws its own mark.
- **Per area**: every widget that writes says so to the framework, the
  way it already gives its rect a name (`crate::name_rect`). An egui-pop
  call registers the area with its kind and, for an edit area, **what it
  writes** (a label and the parameter path: "Exposure", `exposure`). The
  framework then draws the affordance: the cursor, the hover outline,
  the hover card ("Drag: Exposure +0.30 EV"), the rest marks, the
  accessible role and description.
- **Who draws what**:
  - egui-pop: the panel chrome (the tab's mark, its tooltip), the area
    registry, the hover card and the rest marks of its own widgets
    (histogram grips, curve points, the navigator's frame);
  - egui_hud: its controls and handles already are edit areas; they
    register through the same contract (cursor and hover from one place);
  - the controls layer (`helios-controls`, #4861): knows each control's
    parameter, so it gives the label and path; `gizmo` handles too;
  - Helios: only declarations (the panel kinds, the path the histogram
    drags). Helios draws no mark itself (widgets only in egui-pop or
    egui_hud).
- **An invariant** instead of a convention: a headless test fails when a
  write to the edit's paths comes from an area not declared `edit` (the
  value store knows the origin of each write, the registry knows the
  area under the gesture). A new editing area cannot ship unmarked.

### 3.2 What the bus carries

- `panel.describe` (or the panels collector) carries each panel's kind,
  and `ui.areas` each declared area: id, kind, rect, for an edit area its
  label and path. Agents and tests read which areas edit without
  screenshots; the invariant above reads it too.
- An edit area's gesture carries the area as its origin detail
  (`histogram.drag` already is a step id), so the step's label can say
  where the change came from.
- Nothing new is needed for the values: the writes stay value store
  writes, as today.

### 3.3 How it meets #5199, #5202 and #5205

- **#5199 (modified marks)**: the changed dot and Reset belong to edit
  areas only; a view panel never shows one. An edit area whose parameter
  differs from its default shows the same dot as its row in Parameters
  (the histogram's card: "Exposure +0.30 EV, changed", double click
  resets, as darktable). A control or mixed panel's header mark can carry
  the changed state of what it edits.
- **#5202 (history)**: each step from an edit area is labelled with the
  parameter and the area ("Exposure +0.3 EV, histogram"); navigate areas
  never add steps, which is what makes them safe to use and is the rule
  the hover card states.
- **#5205 (stale or updating)**: the same chrome carries the state: a
  panel bound to the selected photo dims its content and shows a busy
  mark in the header while it is computing. **Edit areas are inert while
  stale**: dragging a histogram of the previous photo would edit with a
  wrong picture, so the grips are hidden and the drag refused until the
  data is current. One header slot, two marks (kind, state).

## 4. Options

### A. Cursor and hover only

Every edit area gets the edit cursor, the hover outline and a hover card
naming what it writes; display areas keep the default cursor and no
hover. Panels' chrome unchanged.

- Cheap: the histogram already has the cursor and outline; adds the card
  and the registry.
- Does not meet "obvious at a glance": at rest, the histogram and the
  waveform still look the same; one must hover to find out.

### B. A, plus rest marks and a header mark (recommended)

A, and:

- edit areas show **grips at rest**: the histogram draws its exposure
  grip (and black grip if added) as small handles on its edges, like
  Capture One's Levels; curves and mask handles already have them;
  view-only scopes have none, so the difference is visible without
  hovering;
- control and mixed panels carry a **mark in their tab** (a small edit
  glyph, its tooltip: "Edits the photo" or, for a mixed panel, "The
  histogram edits the exposure"); view panels carry none;
- the header slot is shared with #5205's busy mark.

Meets the requirement with the patterns of section 2.5 (handles at
rest, hover naming the parameter, edit chrome in the header), no new
colours, no theme change beyond one glyph and one token.

### C. B, plus distinct chrome for control panels

B, and control panels get a tinted header or background (an accent
band), mixed panels a partial band around their edit areas.

- Strongest signal, but none of the sources describes it; adds theme
  tokens for every theme, noise on the most used panels (Parameters is
  always tinted, so the tint says little), and mixed panels look
  patched.

### Recommendation

**B.** A alone fails the owner's "at a glance"; C adds noise for little
over B. B follows what the established editors do (section 2.5) and stays in the
framework.

### Follow-up tickets (for B)

1. egui-pop: `PanelSpec::kind` (`View`, `Control`, `Mixed`) and the tab
   mark with its tooltip and accessible description, sharing the header
   slot with #5205's state mark. egui-pop issue.
2. egui-pop: the area registry (`kind`, label, path) with the cursor,
   hover outline, hover card and rest grips drawn from it; the histogram
   moves to it and draws its grip at rest. egui-pop issue.
3. egui_hud and `helios-controls`: controls and `gizmo` handles register
   as edit areas with their parameter's label and path (one cursor and
   hover rule for every handle).
4. Helios: declare the kind of every panel in `helios_panels_with`, the
   histogram's label and path in `scopes.rs`, and edit areas as inert
   while #5205's state is stale.
5. Helios bus: `panel.describe` carries the kind and `ui.areas` the
   declared areas; history labels name the area (with #5202); the
   headless invariant "every edit write comes from a declared edit area".
6. Decide separately (not decided): whether the waveform and the parade
   become editable as in darktable (upper part exposure, lower part
   black level), and whether the histogram gets darktable's black-level
   region. Under B they would show grips the same way.

## Sources

[dt-scopes]: https://docs.darktable.org/usermanual/development/en/module-reference/utility-modules/shared/scopes/
[dt-header]: https://docs.darktable.org/usermanual/development/en/darkroom/processing-modules/module-header/
[lr-tone]: https://helpx.adobe.com/lightroom-classic/desktop/help/tone-control-adjustment.html
[lr-kost]: https://jkost.com/blog/2022/02/applying-auto-tone-adjustments-in-lightroom-classic.html
[lr-tat]: https://jkost.com/blog/?p=22724
[c1-icons]: https://support.captureone.com/hc/en-us/articles/360009649218-Icons-in-Capture-One
[c1-tf]: https://thomasfitzgeraldphotography.com/blog/2021/8/capture-one-beginners-understanding-capture-one-tool-panel-icons
[c1-hist]: https://imagealchemist.net/histograms-in-capture-one/
[c1-levels]: https://imagealchemist.net/capture-one-levels-part-1/
[ps-cache]: https://www.manualsdir.com/manuals/753896/adobe-photoshop-cs4.html?page=163
[ps-ptc]: https://photoshoptrainingchannel.com/tips/cache-levels-histogram-panel/
[ps-kost]: https://jkost.com/blog/2019/10/working-with-the-curves-in-photoshop.html

- darktable user manual: [scopes][dt-scopes], [module header][dt-header].
- Adobe: [Lightroom Classic tone control][lr-tone] (the page refused
  automated reading; its content as quoted in search results);
  [Photoshop CS4 manual, refreshing the histogram][ps-cache].
- Capture One: [Icons in Capture One][c1-icons].
- Third-party: Julieanne Kost ([Basic panel and histogram][lr-kost],
  [Tone Curve panel][lr-tat], [Curves][ps-kost]), [Thomas
  Fitzgerald][c1-tf], Image Alchemist ([histograms][c1-hist],
  [Levels][c1-levels]), [Photoshop Training Channel][ps-ptc].
