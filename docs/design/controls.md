# Controls: one controller per parameter, three surfaces, any device

Status: **accepted** (2026-11-08), recommended answers to Q1 to Q10
(Q6 superseded on 2026-11-09). **Layout and integration decided
2026-11-09** (owner's decisions, each dated where it is written):

- two integration levels, managed and integrated (3.1);
- a size policy per control, the module laid out like an HTML form
  (3.2);
- standard slots on a base unit of one panel row, counted in twelfths
  (D1, 3.2);
- an orientation per control, picked by the engine from the slot's
  shape (D4, 3.2);
- the slot engine as a standalone crate (D2) with its own small
  quantized-grid core (D2b, section 4);
- dense, deterministic packing with gap filling (D3, 3.2);
- the gallery's composing area named the **bench** (D5, section 7);
- everything interactive or live is a control, static items are a
  case's chrome (3.1).

The context rules (`control-contexts.md`) are accepted too (2026-11-09,
its choices C1 to C4).

**Demo** (2026-11-10): [egui_hud's web demo](https://quazardous.github.io/egui_hud/)
(`demo/` in egui_hud's repository) shows the framework with no application behind
it: a mixer channel and a light declared as data, on the bench, which
moved into `egui_hud` (feature `bench`) so the gallery and the demo share
it.

**Own repository** (2026-11-11): egui_hud and its demo moved out of the
Helios workspace to <https://github.com/quazardous/egui_hud>, under MIT OR
Apache-2.0; Helios depends on it by git, pinned to a commit. Since
2026-11-11 it is on crates.io (0.1.0), as egui_fit is, and Helios takes
both from there.

**Slice 1 built** (2026-11-09): the `helios-controls` crate (model,
registry and `controls.describe` output, the value flow with the vetoable
`controls.change` step, implicit controls from descriptors), the
`exposure` control patched by the exposure module (steps, 0 EV detent,
fine mode, slider and scrub variants, HUD chip), the `controls.*`
commands and `controls` collector, and the controls gallery
(`crates/helios-controls-gallery`, native and web). **Slice 4 built**
(2026-11-09): `helios-controls` on `egui_fit`. Each variant
declares a size policy per representation (full, compact, simplified:
the engine's forms 0, 1, 2 in that order), its orientations and its
integration level per surface with its managed fallback; indicators
and a case's chrome are in the model; `card::layout` packs a card or a
case and maps each placement back. A rack counts its width in slot
columns and its height in rows, so a `1x1` slot is a panel row whatever
the column's width. **Slice 5 built** (2026-11-09): the context keys,
the box and the density, tiers on parts and rules on cards and cases
under the class rule, and `resolve` per card: the precedence, the
person's layout, never folding a changed value, the slot engine, a
placement per item with its reasons, the "More (n)" entry, anti-jump,
the reachability invariant, and the `controls.context`, `controls.find`,
`controls.reveal`, `controls.reveal_all` commands
(`control-contexts.md`, section 6.5). **Slice 9 built** (2026-11-10):
the darkroom's Parameters panel draws each node as a card through
`resolve` and `render::card`: each parameter gets its patched control
(`exposure`, and `black-level` now bound to exposure's `black`) or its
implicit one, the card resolved in the context the `controls.*`
commands share (`register_with`: the panel sets the box from its width
at the density, and the hand), its gestures, undo steps, marks, menus
and accessible names as before. The renderers gained a host's
dressing (`render::Drawings`: the label shown, a choice's words, a
slider's touches, a verdict on each change, the marks in gutters
beside a part, and the touches once drawn); Helios draws as integrated
parts what the framework has no form for (curves, wheels, lists, text,
a mask group's operator). Section 7 lists the slices left. Section 8 holds the questions, each with its
recommendation.

**The request.** Helios modules should not draw widgets. They declare
**controls**: specialised, configurable controllers, "a kind of
gamepad", that sit between a module's parameters and whatever the person
touches. One control renders on three surfaces (the module panel, a HUD
over the image, handles on the image) and takes input from any device
(pointer, wheel, keys, MIDI, a gamepad, a console like Loupedeck). Simple
controls compose into complex ones: a gradient mask is a line (two
points), a feather and an angle. Helios's modules and its plugins share
one catalogue, and plugins may add composites. The person customises how
each control looks and behaves (its variant, sensitivity, fine
mode, steps, bindings, the module's layout, favourites), stored as
settings. Every change goes through the control, so undo, gestures,
agents, accessibility and scripts all see the same thing.

This is the photo side of the administration framework
(egui-pop's `docs/design/admin-forms.md`, accepted 2026-11-08), and keeps
its spirit: a declarative model, no hand-drawn widgets, a curated set
plus registered exceptions, a model without egui and renderers apart.
Where the admin note has generic bricks, this note has **business
controls**: each is built for a purpose (exposure, feather, a gradient
mask, crop), and the generic parts (a slider, a knob, a handle, a
read-out) stay egui-pop's. The controls layer is standardised like an
audio rack: standard slots, standard connectors, cases that hold
smaller controls, variants swapped within a purpose, and one ergonomic
contract (section 3.2).

**Where it lives.** egui-pop is the visual layer underneath: widgets,
the viewport, canvases, overlay and handle drawing, the history
contract. The controls layer is Helios's: a crate of its own,
`helios-controls`, on top of egui-pop. Where egui-pop lacks a visual
primitive the controls need, section 4 lists it as a small egui-pop
addition; the controls themselves never move into egui-pop.

**Terms used below.**

- A **parameter** is a module's value in the value store
  (`node/<node>/<param>`), described by its descriptor.
- A **control** is a business controller built for one purpose (the
  exposure control, the feather control), patched to one or more
  parameters. It knows its value, units, ranges, steps, gestures,
  read-out and handles. It draws through egui-pop's visual parts.
- A **composite**, or **case**, is a control made of other controls (its
  **parts**), wired inside.
- A **face** is a way to draw a value type, shared by every control:
  its slot sizes per representation, its orientations, its chip. The
  library (`egui_hud::face`) has, for a bool, a checkbox, a switch, a
  push button and a lamp (read-only); for a number, a slider, a fader,
  a knob, an endless knob, a scrub field, a meter and a read-out (both
  read-only); for a one-of, a menu, segmented buttons and a radio list;
  for a curve, a colour and a text, their editors. A face fits values,
  not only a type: the bounded slider, fader and knob need a bounded
  number, the endless knob a number that wraps or is open, nudged by
  relative steps. A control (value and behaviour, nothing about
  drawing) may restrict and order the faces it accepts (a gain: fader
  then knob); otherwise it accepts every face that fits its value.
  Which one shows is presentation: the kit's default, the context (a
  slot too small for it takes the next accepted face that fits), or the
  person on the bench (2026-11-11). A **variant**, in older sections, is
  a face as one control takes it.
- A **surface** is where a control appears: the **panel** (the module's
  card), the **HUD** (read-outs and small controls over the image, near
  the pointer), the **handles** (gizmos on the image).
- A **mode** is where the host mounts a control on the panel side:
  `panel` (the module's card), `docked` (a dock of its own), `floating`
  (a window), `hud`, `toolbar`. Toolbars and docks are surfaces too,
  with their own slots (3.2).
- A **representation** is how much a control shows in its slot, the
  same variant either way: `full`, `compact` (labels inside, less
  chrome) or `simplified` (a nudge, a read-out). A slider and a knob
  are two variants, not two representations.
- An **orientation** is the direction a control is laid out along:
  horizontal or vertical. It is neither a variant nor a representation
  (3.2).
- A **slot** is the box the host gives a control: whole units of the
  base unit on each axis (3.2).
- An **indicator** is a control with read-only jacks: a histogram, a
  meter, a read-out.
- A case's **chrome** is what it draws that is not a control: its
  legend, separators, help text, icons. It has no value and no jack.
- A **renderer** draws a control on one surface with egui-pop widgets.
- A **device** is anything that produces input for a control (pointer,
  wheel, keys, MIDI, gamepad, console, an agent).
- A **gesture** is one continuous change (a drag, a held key, a knob
  turn), which ends in one undo step.

## 1. What exists

Read on 2026-11-08: Helios's `next`, egui-pop's `next`.

### 1.1 egui-pop: the visual layer

| Piece | Where | What it gives the controls | What it lacks |
|---|---|---|---|
| Value descriptors | `egui_pop_model::values::Descriptor` | Type (`Float`, `Int`, `Bool`, `Choice`, `Floats`, `Ints`, `Text`), label, default, hard and soft range, step, coarse step, unit, display scale, precision, choices, count, `Writers`. | Nothing about points, angles or how a value is shown on the image. |
| The value store and gestures | `egui_pop_model::values::Store`: `begin_gesture`, `end`, `GestureId`; `Set::gesture` | One gesture is one undo step, its paths held against other writers meanwhile; listeners told of each change. | Nothing: the controls write through it. |
| Bindings | `egui_pop_core::binding::Binding` (ADR 0009): `plain`, `bound`, `watch` | Any widget edits a store path as the user, a drag being a gesture. | One path per widget; a composite writes several paths in one gesture. |
| The parameter widget | `egui_pop_core::param::param` | The editor a descriptor calls for (slider, check box, drop-down, colour swatch, text). | One widget per type, no choice by the person. |
| Sliders | `egui_pop_core::slider::{Slider, RangeSlider}` (`docs/sliders.md`) | darktable's bauhaus row: soft and hard range, fine and coarse modifiers, typed expressions, reset, a quad button, logarithmic, gradient track, field states, AccessKit actions; `on_change` with `Change { from, to, cause, done }`, causes `Drag`, `Key`, `Wheel`, `Typed`, `Reset`, `Access`; `slider::Gestures` as data. | A scrub field is not there (`egui_hud::render::scrub` stands in). Since 2026-11-10: `Slider::vertical`, `Slider::compact` and `Slider::size` (down to 24 points), and `egui_pop_core::knob::Knob`. |
| The `slider.change` step | `egui_pop_api::sliders::hook` | Each finished change runs a step that hooks may veto or redirect. | Per slider, not per parameter. |
| Image tools | `egui_pop_viewport::{curve::CurveEditor, wheels, histogram, zoom, navigator}`, steps in `egui-pop-imaging` (`curve.change`, `wheel.change`…) | A curve editor, colour wheels, a histogram you drag, each with the same `Change` and `done` rule. | Nothing for a list of points or a mask shape. |
| The viewport | `egui_pop_viewport::{Viewport, ViewportState, View}` (`docs/viewport.md`) | Tiled image, pan and zoom, readout, before and after, display transform. | |
| Overlays and handles | `egui_pop_viewport::overlay::{Overlay, Shape, Handle, HandleKind}` | Shapes in image pixels (`Point`, `Line`, `Rect`, `Circle`, `Ellipse`, `Path`), handles of kinds `Move`, `Point`, `Corner`, `Edge`, `Size`, `Rotate`, `Feather`, `Control`, styles (`selected`, `faint`, `dashed`), labels; `Overlay::apply` for the standard shapes; `hit`, `hit_edge`. | Handles in image pixels only (no view-space handle for a HUD widget pinned to a point); no "a handle's value is a scalar" kind beyond the shape's own. |
| Handle signals | `egui_pop_viewport::Signal::{Handle, HandleClick, EdgeClick, HandleReset, HandleWheel}`, `Phase::{Begin, Update, End, Cancel}`; the `before_handle` hook (`HandleMove`) | A handle drag is one gesture with begin, update, end and cancel (Escape); a hook vetoes or snaps; double click resets; the wheel adjusts. | |
| Paint hooks | `under_overlays`, `after_paint` | A mask's tint under the overlays, anything above the handles. | A placed HUD element (anchored to a point, kept on screen) is not a primitive. |
| The canvas and its commands | `egui_pop_core::canvas::{mark_active, request, take}`, `egui_pop_viewport::navigation` (`docs/design/navigation.md`) | The canvas in use takes `view.fit`, `view.zoom_in`, `view.frame_selection`…; hand mode; the `canvasActive` context key. | The image viewport is not yet on the model (the node graph is). |
| Pointer profiles | `docs/design/pointer-gestures.md`, `egui_pop_api::pointer` | darktable, Adobe, Document, Blender, Trackpad profiles stored as a keymap layer; wheel device and speed; slider wheel. | Shape-specific wheel meanings (Shift + wheel is feather on a shape in darktable) are not in a profile. |
| Keys | the keymap (ADR 0007), user and workspace layers, the shortcuts editor | Every command has keys, layered, with conflicts said. | A key that nudges a control (not a command) has no place. |
| History | `egui_pop_api::history`, `egui_pop_model::history` (`docs/history.md`, ADR 0014) | Steps with an origin and a place, groups, the history panel, `history.*` for agents. | Nothing. |
| Agents | `egui-pop-agent`, `docs/design/agent-control.md` | Commands, collectors, the journal, scenarios, the `ui.*` side over eframe's inspection. | No collector of controls. |
| The admin framework | `egui_pop_model::admin`, `egui_pop_api::admin` | The pattern followed here: manifests as data, bricks, registered kinds, hooks that compute and never draw. | Pages only, no image surfaces (by design). |

### 1.2 Helios: how modules get their interface today

There is no darkroom crate: the darkroom is a place of `helios-gui`
(`contexts.rs`, the `DARKROOM` key scope in `keys.rs`).

| Piece | Where | Today |
|---|---|---|
| Engine descriptors | `helios_core::manifest::{Manifest, ParamSpec, ParamKind}` | A module's parameters: name, description, and a kind (`Float`, `Int`, `Bool`, `Choice`, `Floats`, `Ints`, `Text`) with its hard range and default. |
| Interface hints | `helios_core::manifest::{ParamHint, ParamWidget}`, `Module::param_hints` | Optional per parameter: soft range, step and coarse step, unit, display scale, precision, label, `logarithmic`, `picker`, a widget (`Curve` or `Wheel`) and a `group`. Only blur, exposure, sigmoid and most of `modules/masks` give hints. |
| Store descriptors | `helios_values::Descriptor` | The two merged, at `node/<node>/<param>`; also `bypass/<node>`, `view/…`, `settings/…`. |
| The generated panel | `helios_gui::params::Params::panel_ui`, `param_ui_with` | Parameters grouped by node, then by `group`. By type: egui-pop `Slider::from_descriptor` (`signed` when the unit is EV, `logarithmic`, the picker as the quad button), a check box, a combo box (a mask group's `operator` has its own), `CurveEditor` or `ColorWheel` for `Floats` hinted so, a folded list for other lists, a text field. Each row has marks: changed from the default (a click resets), held by another origin's gesture, moved lately by someone else, past the soft range. |
| Hand-drawn parts | `helios_gui::{modules, mask_tree, masks}` | The Modules panel and its bypass toggles; the Masks panel's tree; the on-image shapes. **No module draws its own interface.** |
| Store, gestures, undo | `helios_values::Store`: `Set { path, change, origin, force, gesture, if_seq }`, `begin_gesture`, `end`, `cancel`; `param.changed` on the bus; `helios_gui::history::HeliosHistory` (`docs/design/undo.md`) | One undo step per gesture or group; a gesture's paths refuse other origins; cancel leaves no step. An agent's request is a group, undoable alone. |
| The slider path | `helios_gui::params::apply`, `apply_tool` | A drag opens a gesture on its first frame and writes each frame; the release ends it; a drag stopped without release (Escape, a veto) cancels. Keys, the wheel and AccessKit actions step, and steps closer than 0.5 s (`STEPS_GAP`) merge into one undo step. A typed value past the soft range is written with `force`. Every finished change runs egui-pop's `slider.change` step first. Curves and wheels follow the same path. |
| Mask shapes | `modules/masks/src/shape.rs`: `MaskCircle`, `MaskEllipse`, `MaskGradient`, `MaskPath`, `outline(module, params) -> Outline` | The edge, the feather and the handles of each shape, in image pixels. Brushes (`brush.rs`) are parameters only. |
| On-image editing | `helios_gui::masks::Masks` (`signals`, `on_handle`, `drag_writes`) | egui-pop overlays from `outline`; transient state at `view/masks/{mask,shape,point,tool,draft}`; a handle drag is a gesture, and `drag_writes(module, handle, start, to)` turns a handle's place into parameter values (centre, radius, `radius_x` also sets the rotation, feather, the gradient's rotation and feather, a path's corners and mirrored Bézier controls). `before_handle` snaps (Shift: 15° or the axes; Ctrl: 0.1×). Double click resets, the wheel adjusts as one undo step. This is a composite written by hand, once per shape. Since 2026-11-11 the circle's, the ellipse's and the gradient's handles are declared in `helios_controls::gizmo` instead (section 7, slice 12). |
| Crop | none | No crop tool yet. |
| Colour picker | `params.rs`: the quad button publishes `param.picker {path}` | Nothing receives it yet. |
| Keys | `helios_gui::keys`, `keymaps.rs` (packages in `keymaps/*.json`), `pointer_choice.rs` | Commands only: C, E, G, P add a circle, ellipse, gradient, path. No key is bound to a parameter. The wheel-on-slider profile is Photoshop's by default, darktable's on offer. |
| MIDI, gamepad | `Origin::Midi` (`helios-shell`), mapped from egui-pop's `Device("midi")` | An origin only; no input code. No gamepad. |
| Agents | `helios_values::commands`: `param.list`, `param.get`, `param.set` (`value`, `step`, `coarse`, `reset`, `top`, `bottom`, `force`, `if_seq`), `param.reset`, `param.undo`, `param.undoAgent`, `param.redo`; `param.copyPath`, `param.copyValue`, `param.paste`; `mask.add.*`, `mask.select`, `mask.handle.move`, `mask.point.*`, `mask.member.*`, `mask.overlay.*` | Agents already set any parameter by path; masks have their own verbs. |
| Coordinates | `docs/masks.md` ("Image coordinates"), `helios_xmp::drawn::{Frame, exif_orientation, ellipse_rotation, gradient_rotation}`, `modules/flip` (`orientation`, -1 for EXIF, 0 to 7) | Shapes are stored in full-resolution pixels of the image the network reads; darktable's masks are converted into them through the photo's orientation on import (tests in `helios-xmp/tests/orientation.rs`). |

**What today says.** The generated panel is already declarative: a
module gives descriptors and hints, the GUI picks the widget. What is
missing is everything above one parameter: points, angles and shapes
that span several parameters (written by hand in `drag_writes`, per
shape), a variant the person chooses, any device but the
keyboard and pointer, a HUD, and a way for a plugin to bring a shape.
`ParamWidget::Curve`, `Wheel` and `picker` exist but no module uses them
yet.

## 2. Survey

### 2.1 darktable

Read in the darktable sources (the fork Helios builds on, whose classic
mask manager was replaced by a "flexi masks" panel; the rest is close to
upstream). egui-pop's pointer gestures study (`docs/design/pointer-gestures.md`,
section 4) lists darktable's wheel and modifier meanings on shapes and
curves.

- **Control to parameter.** `dt_bauhaus_slider_from_params(self, "name")`
  and `dt_bauhaus_combobox_from_params` (`src/develop/imageop_gui.c`)
  find the field by introspection (`$MIN`, `$MAX`, `$DESCRIPTION`,
  `"name[3]"` for an array element) and keep a raw pointer into the
  module's params (`dt_bauhaus_widget_set_field`). A change writes the
  field and calls `dt_iop_gui_changed`, which runs the module's
  `gui_changed` and adds a history item. Sliders have a soft range
  (shown) and a hard range (reachable by force: Ctrl+Shift, or typing);
  right click opens a popup that evaluates an expression
  (`dt_calculator_solve`).
- **Undo grouping.** The top history item is replaced while the same
  module instance is edited (`_dev_add_history_item_ext`), and changes to
  the same widget within `darkroom/undo/merge_same_secs` make one undo
  record (`src/develop/develop.c`).
- **Actions.** Every bauhaus widget becomes an action when it gets its
  label (`dt_action_define`), with **elements** (a slider's `value`,
  `button`, `force`, `zoom`) and **effects** (edit, up, down, reset, top,
  bottom, set; `src/gui/accelerators.c`). A shortcut (`dt_shortcut_t`)
  binds a key, a button or a **move** (scroll, a MIDI knob, a gamepad
  axis) to an action, element and effect, with a **speed** the person
  sets per binding. Ctrl ×0.1 and Shift ×10 are fallbacks, also data.
  Mapping mode binds by hovering a widget and pressing a key or moving a
  control.
- **Devices.** Input drivers register (`dt_register_input_driver`) and
  feed `dt_shortcut_key_press` and `dt_shortcut_move`, whose return is
  the new position for feedback: MIDI with LED rings and motorised
  faders (`src/libs/tools/midi.c`, PortMidi, Behringer BCF and BCR),
  gamepads (`src/libs/tools/gamepad.c`, SDL). Lua calls the same actions
  (`darktable.gui.action`).
- **Masks on the image.** Each shape has a table of functions
  (`dt_masks_functions_t`: `mouse_moved`, `button_pressed`,
  `mouse_scrolled`, `post_expose`, `get_points`, `get_distance`…,
  `src/develop/masks/*.c`). A drag back-transforms the pointer through
  the pipe (`dt_dev_distort_backtransform`), normalises it and writes the
  form; drawing transforms forward on the preview pipe. Selection syncs
  panel and image (`dt_masks_select_form`, `masks_selection_changed`).
- **Crop and picker.** Crop edits a box in preview space and commits on
  release or focus out, back-transformed through the modules before it
  (`_commit_box`, `src/iop/crop.c`). A colour picker is attached to a
  button or a slider's quad button (`dt_color_picker_new`); the module
  applies the sample itself (`color_picker_apply`).
- **Favourites.** The quick access panel (`src/libs/modulegroups.c`)
  names widgets by action path (`"exposure/exposure"`) and **moves the
  real widget** into its panel, so binding and history stay the same; a
  module with several instances shows it disabled.

**What to keep:** every widget an action with elements and effects,
devices as drivers over one dispatcher, speed per binding, favourites by
path. **What to avoid:** a raw pointer into params (no origin, no
gesture), per-shape event code written by hand, the forward and inverse
transforms only on the preview pipe.

### 2.2 Other applications

From their documentation and use. "Unconfirmed" marks what was not
checked against a source.

| App | Control to parameter | Customisation | Panel, HUD and handles |
|---|---|---|---|
| Lightroom Classic | One slider per parameter in Develop panels; the targeted adjustment tool (TAT) turns a drag on the image into moves of the tone curve or HSL sliders under the pointer's colour. Masks (linear and radial gradient, brush, AI selections) each own a set of the same local sliders. | Few: panel order (Customize Develop Panel), slider steps by Shift and keys (`+`/`-` on the selected slider, unconfirmed). Consoles (Loupedeck, MIDI through the third-party MIDI2LR plug-in) map to the SDK's develop parameters. | A gradient's three lines and pin, a radial's ellipse and handles, all mirror the mask's parameters; the panel's sliders update as you drag on the image. The TAT shows which slider moves by highlighting it. |
| Capture One | Tools (Exposure, HDR, Levels, Color Editor…) of sliders; tools placed freely in user-made tool tabs; "Quick" tab of favourites. | Tool tabs and their order, workspaces saved and shared; keyboard shortcuts for "adjust sliders"; native Loupedeck and Tangent support (unconfirmed for Tangent). | Layers have masks drawn on the image (brush, linear and radial gradient) with handles; the panel and the image are linked; a "cursor tools" bar picks what the pointer does. |
| DaVinci Resolve | Every grade control (lift, gamma, gain wheels, curves, qualifiers, windows) is one parameter of a node; the same parameter is reachable from the wheels, the numbers under them, the mouse on the viewer, and the Micro, Mini and Advanced control panels. | Panels map parameters per page by Blackmagic (fixed); the keyboard is customisable; third-party panels (Tangent) get a mapping file. | Power windows (circle, linear, polygon, curve, gradient) have on-viewer handles (inner, outer softness, rotation) that mirror the Window palette's numbers; tracking moves them. Same parameter, three surfaces: the closest model to this note. |
| Blender gizmos | A gizmo (`bpy.types.Gizmo`) is bound to a property with `target_set_prop`; a gizmo group (`GizmoGroup`) poll decides when it shows. Operators run on release, with redo ("Adjust last operation"). | Gizmo visibility per viewport; keymaps for every gizmo's modal (Ctrl snap, Shift precision). | Gizmos are drawn in 3D or 2D space, the property panel updates live; the property is the single truth. Add-ons write gizmos in Python: the extension model this note borrows. |
| Photoshop | Properties panel sliders; on-canvas: transform handles, the Gradient tool's live on-canvas gradient (since 2023: stops edited on the canvas), the Brush's Alt+right-drag HUD for size and hardness. "Scrubby sliders": drag a field's label. | Workspaces, keyboard shortcuts per tool; HUD colour picker style (strip or wheel) in Preferences. | On-canvas widgets mirror the Properties panel; the HUD appears while a modifier is held, near the cursor, and disappears on release. |
| Affinity Photo | Context toolbar under the menu shows the active tool's parameters; adjustment layers open a dialog of sliders; on-canvas handles for gradients (Gradient tool), transforms and live filters' centres. | Studio panels arrangement, keyboard shortcuts; the context toolbar is fixed per tool. | The gradient's stops are handles on a line on the canvas, the context toolbar's values follow. |

### 2.3 Patterns

- **One parameter, several surfaces, one truth.** Resolve's windows,
  Lightroom's masks, Blender's gizmos: the handle and the panel show the
  same value and either may change it. None keeps a copy.
- **The handle set of a mask is always the same few shapes**: a line
  with a feather band (gradient), an ellipse with a feather ring and a
  rotation handle (radial), a path, a brush. They decompose into points,
  lengths and angles: the owner's composition example.
- **A HUD is summoned, not permanent.** Photoshop's brush HUD lives while
  keys are held; darktable's shape wheel adjusts show the value in a
  toast-like read-out; Lightroom shows values only in the panel. Nobody
  keeps a permanent floating panel over the image.
- **Devices map to parameters through one registry.** darktable's action
  tree (keys, MIDI, gamepad), Resolve's panels, Lightroom's SDK used by
  MIDI2LR: hardware works when every parameter has a stable id and a
  range.
- **Customisation is per person, with workspaces for layout.** Capture
  One's tool tabs and workspaces, darktable's module groups and quick
  access panel, Lightroom's panel order. A variant (slider or
  knob) is rarely offered; sensitivity and keys are.
- **Commit on release.** Blender's operators, darktable's history item
  after the drag, Lightroom's history step per slider gesture: preview
  while moving, one step at the end.

## 3. The model

### 3.1 A control is built for a purpose

A control is **almost a business layer**. It is not a generic widget: it
is "the exposure control", "the feather control", "the gradient mask
control". It carries the domain meaning of its purpose:

- its **value** and units (EV, kelvin, image pixels, degrees), its hard
  and soft ranges, its steps and fine steps, its sensible default;
- its **gestures** for that purpose (exposure: drag on the image to
  scrub, double click to reset to 0 EV; white balance: pick a neutral
  area);
- its **HUD read-out** (exposure: "+0.67 EV"; white balance: "5200 K,
  tint +4"; feather: the band drawn and "48 px");
- its **handles**, when it has a place on the image (a gradient's line,
  a crop's box, a picker's area);
- its **panel row**, and its **intents** for devices (3.8).

The **customisation** happens within that purpose (3.9): a person may
show exposure as a knob, slow it down, bind it to a MIDI encoder, move
it to the top of the card. It stays the exposure control.

**Three layers, three words.**

| Layer | What it holds | Examples | Who extends it |
|---|---|---|---|
| egui-pop: **visual parts** | generic widgets and drawing, no meaning | slider, range slider, knob, scrub field, check box, drop-down, curve editor, colour wheel, overlay shapes and handles, a read-out | egui-pop, by issues (section 4) |
| `helios-controls`: **controls** | business controls, built from visual parts, configurable within their purpose, composable | exposure, feather, white balance, gradient mask, crop | Helios; plugins register more (Q5) |
| modules: **patches** | which control edits which node parameter | the exposure module patches `exposure` to its `exposure` parameter | each module and plugin |

Visual parts are not controls: a slider knows nothing of EV, a handle
nothing of feathers. A control picks the parts that suit its purpose
for each surface. Its variants are the forms it allows (a slider, a knob
or a scrub field for exposure; never a check box), and the person picks
one.

**What a control declares.**

```text
control  id: "exposure"                       # the purpose, stable
  value:   number  unit: EV  hard: -18..=18  soft: -3..=4
           step: 0.01  fine: 0.001  coarse: 1/3 stop  default: 0
  faces:   [slider*, knob, scrub]            # * the default; from the face library
  hud:     readout "{+0.00} EV"  when: active
  image:   scrub  axis: horizontal  speed: 1 EV per 200 pt
  gestures: double-click: reset;  wheel: step
  intents: nudge, set, reset
```

**Value types are data, not controls.** Underneath, every control's
value is one of a few types the model knows how to store, compare,
interpolate and map: `number`, `bool`, `one-of`, `point`, `angle`,
`range`, `colour`, `points` (a curve or a path), `box`. They describe
storage, and the catalogue never offers a bare value type as a control:
the person picks "the tone curve", never "a `points`". A type still has
a **standard managed renderer**, which is what an integrated control's
managed fallback (below) is drawn with: `points` as a list of numbered
rows (x and y per node), `colour` as a swatch with its channels as
sliders, `point` as two numbers. That renderer is part of the control
that uses it, under its purpose and its ergonomic contract.

**Who adds a control.** The catalogue is curated: Helios's controls are
decided in this note and its follow-ups, like the admin note's bricks.
A plugin may register its own control (section 3.3 for composites, Q5 for
how far), declared as data with the same fields, so it gets
customisation, devices, undo and agents for free. A control that needs a
visual part egui-pop lacks waits for that part, or ships as an
integrated control (below): its own renderer in Rust plus a managed
fallback. Only a control with **no** managed fallback is an exception,
registered and counted by the inspector.

**Two levels of integration (decided 2026-11-09).** A control matters by
what it controls (its patch to parameters) and by what it shows (its
read-out and feedback). Its look is secondary, and comes at one of two
levels:

- **Managed (docked).**
  - The module does not care about the look. It gives only placement
    information: the group (the case) and the order. The control's own
    size policy and orientations (3.2) say what slot it needs. A part's
    **tier** (essential, standard, expert) joins it, from the context
    study (`control-contexts.md`, accepted 2026-11-09): it decides what
    the detail level folds.
  - The host places the control (in a panel, a dock, a float, a HUD or a
    toolbar) and draws it with the standard renderer for its slot,
    orientation and representation.
  - This is the default, and most controls never leave it.
- **Integrated (graphical).**
  - The control brings its own drawing on a surface: a curve editor,
    colour wheels, a histogram you drag, a gradient's line on the image,
    a custom gizmo.
  - It is still bound by the same contract: the value flow (preview,
    commit, cancel), one gesture per undo step, focus, devices, agents
    and accessibility.
  - It also declares a managed fallback, its parameters as standard
    parts. That fallback is used wherever its own drawing cannot go: a
    compact or simplified representation, a folded group, a narrow
    panel, a device with no pointer.

The level is chosen **per surface** and belongs to the control's
renderer, not to the module: one control may be managed in the panel
and integrated on the image. An integrated control with no managed
fallback is the exception counted by the inspector (Q5).

**Everything a control (decided 2026-11-09, answer B).**

- Anything interactive, or showing live data, is a control. A
  histogram, a meter or a read-out is an **indicator**: a control whose
  jacks are read-only. It gets a slot, a size policy, a place in the
  order and an AccessKit node like any other.
- Static items (a legend, a separator, help text, an icon) are the
  case's **chrome**, as `legend` and the border are a `fieldset`'s. They
  have no value and no connector, and the case draws them.
- Nothing is drawn outside a control. Integrated controls draw by
  design, inside their slot or on their surface.
- There is no free egui zone: a module never gets a raw `Ui` to draw
  in.

### 3.2 The rack: slots, connectors, cases

The controls layer is a Lego layer, standardised the way audio racks
are: 19-inch rack units, Eurorack's HP widths, patch cables between
jacks. Five standards make it so.

**1. Standard slots (decided 2026-11-09).** The host does not hand out
arbitrary sizes. It places every control in a **slot** of a standard
size, the way Eurobox-style storage crates work: every size is a
multiple or a simple fraction of one base unit, so slots tile and nest
without gaps, and a growable control grows slot by slot. This replaces
the fixed footprints accepted on 2026-11-08 (Q6).

**Base unit and fractions (D1, decided 2026-11-09).**

- **The base unit** is one panel row, scaled by the density (a person
  setting: comfortable, compact, touch; see `control-contexts.md`).
- **The allowed sizes**, on each axis, are ¼, ⅓, ½, ⅔, ¾, 1 and n
  (whole multiples).
- **Exact arithmetic.** Thirds do not nest in halves, so the engine
  counts in twelfths of the unit. 12 is the least common multiple of 2,
  3 and 4. Every allowed size is a whole number of twelfths, and the
  sums stay exact.

The slots, width × height in base units:

| Slot | Size | Holds, for example |
|---|---|---|
| ¼, ⅓ | ¼ or ⅓ × 1 | an icon toggle, a pip, a two-digit number |
| ½ (the old `half`) | ½ × 1 | a toggle, a short choice, a knob, a small number |
| ⅔, ¾ | ⅔ or ¾ × 1 | a short slider next to a toggle |
| 1 (the old `row`) | 1 × 1 | a slider, a drop-down, a range |
| 1 × n (the old `block(n)`) | 1 × n, n from 2 to 8 | a colour wheel (4), a curve (6), a case's own block |
| k × n | k × n, on surfaces wider than one column | a strip of vertical faders, a wide curve |

Slots belong to the surfaces that lay controls out in a grid:

| Surface | Its grid |
|---|---|
| **Panel** (a module's card) | one column of slots in a narrow panel, more when the panel is wide (`ctl.space` in `control-contexts.md` counts the columns) |
| **Dock** (a rack docked on its own: favourites, a quick strip, a MIDI bank strip) | its own width in slot columns, often several |
| **Toolbar** | a single line of slots along the toolbar's axis, horizontal or vertical; what does not fit goes to egui-pop's "More tools" overflow |
| **Floating** (a rack in a window) | as a dock, at the window's width |

Two surfaces have no slots: the **HUD**, whose element is a **chip**
(one line of text, an icon, at most a nudge: "+0.67 EV", "Feather
48 px"), and the **image**, whose element is a **gizmo** (no box:
handles in the image's space). `chip` and `gizmo` are forms on those
surfaces, not slot sizes.

A module's card is a grid of slot columns, and the slot engine
(section 4) packs its controls into it: the size policies and
orientations are all the layout code knows. Cards are assembled from
controls that snap in, and the person reorders, hides, collapses or
moves them to another card or to favourites without any bespoke layout.
A variant may declare its own size policy (a knob prefers ½, the slider
it replaces 1).

**Size policy (decided 2026-11-09).** Within the slots of a surface, a
control is either **fixed** or **growable**, depending on how it is
drawn:

- A bitmap-like drawing keeps its size.
- A vector drawing can use more room, such as a wider curve or a bigger
  wheel.

The control declares its policy: a minimum, a preferred and a maximum
size for each axis (each an allowed size), an aspect ratio if it has
one, and the axis it grows along. The module gives only the order and
the group. The host then lays the module out the way a browser lays out
an HTML form:

- **Cases** are the containers, like `fieldset`; their chrome (legend,
  separators, help text) is the `legend`.
- **Controls** are the elements.
- **Size policies** drive the flow.

The slot a control is given, and the density, are part of the
**context** handed to the control, alongside the context keys in
`control-contexts.md`. A growable control grows to the next allowed
size that fits within its maximum, never to an in-between size. Below
its minimum, the control switches to a smaller representation, or to
its managed fallback (3.1). That resizing is a physical effect of the
size policy, not a context rule.

**Packing: a grid with gap filling (D3, decided 2026-11-09).** The
owner chose dense packing over an ordered flow that never reorders.
The design keeps it predictable:

- **Dense and deterministic**, in the manner of CSS
  `grid-auto-flow: dense`. Controls are placed in the declared order,
  or the person's order when they set one. A later control may move up
  into an earlier gap that it fits; nothing else changes the order.
  The same inputs (controls, policies, order, slot columns, density)
  always give the same placement.
- **A case is a group (2026-11-10).** Packing stays dense within a
  case, but never across its edges: a case starts on a new band of rows
  below what comes before it, its legend alone on the case's first band,
  and nothing after it moves up into its gaps, so a part never lands
  above its legend and a loose control never inside a case. The slot
  engine gives each case's band, for its frame; a person's order and
  pinned positions apply within it (a pin counts from the top of the
  case's parts).
- **Rows that mix thirds with halves or quarters may leave a gap.** The
  engine fills it with the first later control that fits; failing that,
  it gives the gap to a growable neighbour whose policy allows the size;
  failing that, it leaves the gap empty.
- **Logical order is not visual order.** Tab, AccessKit and the order
  of a MIDI bank follow the declared or person order, never the visual
  position, so a control moved up into a gap keeps its place for the
  keyboard, screen readers and encoders.
- **No jump under the hand.** A repack caused by a context change (a
  resize, a task switch, a part folded) waits until the pointer leaves
  the panel and no gesture is open (the anti-jump rule of
  `control-contexts.md`). A part being handled never moves.
- **The person can pin a position** in their layout (3.9); the engine
  packs around pinned controls.
- **Same slot, same swap.** Two controls that fit the same slot can be
  swapped, and the swap moves nothing else. Moving a control elsewhere,
  or changing its slot size, may repack the controls after it, under
  the rules above.
- **To be measured on the bench:** which slot sizes the shelf shows
  first, and how often gap filling moves a control in practice.

**Orientation (decided 2026-11-09).** Some controls prefer to be
vertical, some horizontal, and some work either way. A control declares:

- its preferred orientation: `horizontal`, `vertical` or `any`;
- the orientations it supports.

Some examples:

- A slider is horizontal in a panel row, and vertical in a mixer-like
  strip (a bank of faders for a MIDI controller, a narrow side dock).
- A tone curve or a colour wheel has no orientation, but keeps its
  aspect ratio.
- A gradient bar, or a histogram strip, is horizontal only.

**D4 (decided 2026-11-09): the engine picks the orientation
automatically from the slot's shape.**

1. It uses the preferred orientation when the slot allows it.
2. Otherwise it uses another supported orientation.
3. Otherwise it falls back to a smaller form (a smaller representation,
   or the managed fallback).

The person may **pin** an orientation for a control; the pin is stored
in their layout (3.9), and the engine then treats it as the only
supported one. A control is never turned into an orientation it does
not support.

Orientation is **not a variant**: it is the same variant laid out along
the other axis. A flipped control keeps its connector, its bindings and
the meaning of its gestures. Dragging up on a vertical slider does what
dragging right does on a horizontal one, and the arrow keys follow the
axis.

**2. Standard connectors.** Every control has a typed interface, its
**connector**:

- **jacks**: the values it reads and writes, each typed with its value
  type, unit and space (`number<EV>`, `point<image>`, `box<oriented>`);
  an indicator's jacks are read-only;
- **gestures** it accepts (drag, scrub, wheel, pick, typed value,
  reset) and **intents** from devices (3.8);
- **signals** it emits: `preview` (a value inside a gesture), `commit`
  (the gesture's end), `cancel`.

A module **patches** a control to its node's parameters by plugging each
jack into a parameter, like patch cables between modules and a mixer
(3.4). Patching checks the types: a `number<EV>` jack takes a float
parameter; a `point<image>` jack takes a `Floats` of two or two floats.

**3. Cases.** A composite is a **case**: it holds smaller controls and
wires them inside (3.3). The case has its own size policy, chrome and
connector;
from outside, a gradient mask is one control with jacks `anchor`,
`angle`, `feather` and `profile`, whatever it holds.

**4. Swapping within a purpose.** Two controls with the same purpose and
the same connector are **variants**, and are interchangeable with no
change to the module: the slider exposure and the knob exposure, the
gradient with a sigmoid band drawn or plain lines. The module patches
the purpose; the person picks the variant (3.9). That is where
customisation lives.

**5. The ergonomic contract.** The standard is not only pixels: a
slot guarantees how a control is handled, so muscle memory carries
from one module to the next. The renderers and the runtime in
`helios-controls` implement it once; a control's author declares
numbers (sensitivity, detents), never behaviour.

- **Same gestures for the same purpose, everywhere.** Drag and scrub
  (relative: a click never makes a value jump), wheel steps, Ctrl finer
  and Shift coarser (or the pointer profile's modifiers), double click
  resets, arrows step, Page Up and Page Down take the big step, Enter
  types a value or an expression, Escape cancels the gesture. One
  **response curve** per purpose: linear, logarithmic (radius, ISO-like
  values) or perceptual (exposure in stops), with the same acceleration
  for fast wheel and encoder turns everywhere.
- **Predictable placement and reach.** Placement is predictable
  because packing is deterministic and the logical order never changes
  under the hand (standard 1); within a slot, the parts of a control
  are always in the same places:
  - **Horizontal**, in a slot one unit wide: the label is on the start
    side inside the bar, the value on the end side, the modified mark
    before the label, the quad button (a picker, an on/off) at the end.
    A ½ slot keeps the same order in half the width; narrower slots
    (¼, ⅓) drop the label into the tooltip and keep the value.
  - **Vertical**: the label at the bottom, under the track (as on a
    mixer's fader strip), the value at the top, the modified mark next
    to the value, the quad button below the label. Up is more.
  - **Blocks** (n rows high) put their title row first with the same
    mark and reset.

  Frequent actions are near the pointer: the HUD chip appears next to
  the pointer or the handle being dragged, never in a corner; handles
  sit on the image where the hand already is, and the wheel over a
  handle adjusts what it shows.
- **Device ergonomics.** Each purpose maps onto physical controls in a
  standard way: a number onto an endless encoder (relative, with
  acceleration) or a fader (absolute over the soft range), its reset onto
  the encoder's push; a point onto a stick (rate control) or two
  encoders; an angle onto an encoder with detents; a choice onto
  buttons or an encoder with steps; a pick onto a button. A **rack**
  (a card, or the favourites) maps onto a **bank** of physical controls
  in its logical order (declared or the person's, not the visual one): eight controls, eight encoders, and the bank pages with
  the next card.
- **Feedback.** While handled, every control gives the same feedback: a
  live read-out in the HUD chip, the preview rendered from the gesture's
  values, the handle drawn active, then the commit as one undo step
  named after the control. A device with feedback (an LED ring, a
  motorised fader) follows the value, whoever changed it.
- **Fatigue and precision.** Sensitivity defaults by purpose (exposure:
  1 EV per 200 points of drag; angle: 1° per 4 points), a fine mode
  (×0.1), snapping and **detents**: a short resistance at a meaningful
  value (0 EV, the as-shot white balance, 0°, 90°), crossed by moving on,
  turned off with a modifier. Handles have a minimum hit area (24 points,
  today's `HIT_RADIUS`) whatever their drawn size, and keep it at any
  zoom.

What the person may override within the contract (sensitivity, the
modifiers, detents on or off) is in 3.9 and question Q7.

### 3.3 Composition: cases

Bigger controls are built from smaller ones. The gradient mask control
is a line control (two points), a feather control and an angle control,
plus its own profile choice. A composite is a control like the others:
it has its purpose ("the gradient mask"), its read-out, its handles,
and it is reused wherever that purpose appears (the masks module today, a
plugin's graduated filter tomorrow).

A composite declares:

- its **parts**: other controls (the feather control, the angle
  control), each possibly tuned for this use (a feather in image pixels
  up to 1000, soft to 300);
- its **own values**: what belongs to the whole (the gradient's
  profile, linear or sigmoid);
- its **derived parts**: a part shown and edited, but computed from
  others. The gradient stores an anchor, an angle and a feather; its line
  part is derived from anchor and angle (`derive`), and dragging one of
  the line's ends writes anchor and angle back (`apply`);
- its **renderings** per surface, each optional, with **fallback to the
  parts**: the gradient draws its own handles (the line, the feather
  band, the rotate knob together); in the panel it falls back to its
  parts' rows (the feather slider, the angle dial, the profile choice).
  A composite with no rendering at all still works in the panel, on
  devices and for agents, because its parts do.

**Value flow.** The node's parameters are the only truth. A part reads
its value from its binding or its `derive`; a write goes to its binding
or through `apply` to the composite's bindings. Every write of one
gesture, on any part, goes into the composite's one gesture (3.5), so
"move the gradient" is one undo step even when it changes three
parameters.

```text
control  id: "gradient-mask"
  parts:
    line:     line-handle  space: image         # derived from anchor, angle
    feather:  feather      max: 1000 px  soft: 0..=300
    angle:    angle        derived from line
    profile:  one-of [linear, sigmoid]          # its own value
  hud:      [feather, angle]   when: dragging
  image:    gradient-gizmo     # the line, the feather band, the rotate knob
  panel:    fallback           # the parts' rows
```

**Reuse.** The feather control is in the circle, the ellipse, the
gradient and the path; the exposure control is in the exposure module
and in a mask's own exposure (when a mask carries one). The ellipse mask
control is a centre point, two radii, an angle and a feather; a
vignette control would be the ellipse plus a strength.

### 3.4 Patching to node parameters

A module patches each jack of a control (or of a case's part) to a
parameter of the node it is shown for:

```text
bind: param "exposure"          # node/<node>/exposure
bind: param "center" index 0    # one element of a Floats parameter
bind: none                      # derived (3.3) or the composite's own draft
```

- **The node is not in the declaration.** A module declares its controls
  once; the controls are **instantiated** per node (`node/<id>`), so two
  instances of exposure have their own controls with the same ids.
- **Checked at resolution**, as manifests are: every patched parameter
  exists with a type that fits the jack, every control is in the
  catalogue. A mismatch is
  a diagnostic and the control is left out, never a panic.
- **Transforms**: a control may show a value in another unit (`transform`
  hook: stored to shown and back), for instance a radius stored in image
  pixels shown as a percentage of the width.
- **Many controls, one parameter** is allowed and expected: the
  exposure slider, a MIDI knob and a HUD read-out all bind the same
  parameter. They never hold a copy.

### 3.5 The value flow: preview, commit, cancel

Every change, from any surface or device, is the same sequence:

1. **Begin.** The control opens a gesture on the store
   (`helios_values::Store::begin_gesture`, origin user, agent, script or
   device), which holds
   the paths it writes. A composite opens one gesture for all its parts.
2. **Preview.** Each move writes the parameters inside the gesture. The
   engine renders the preview from the store, as today; listeners (the
   panel, the HUD, the handles, other devices) redraw from the store.
3. **Commit.** The gesture ends (`Store::end`): one undo step, labelled
   by the control ("Gradient: angle", "Exposure"), with its place on the
   context stack. The `controls.change` step runs once, with `control`,
   `node`, `from`, `to`, `cause` and `device`; before-hooks may veto
   (the values go back) or redirect (snap, clamp).
4. **Cancel.** Escape, a device's cancel, or a lost focus mid-gesture
   restores the values from before the gesture with no trace
   (`Store::cancel`).

Discrete inputs (a key press, a wheel notch, a MIDI note) are one-shot
gestures; repeated ones within a short window (a held key, a knob being
turned, wheel notches) merge into one gesture that ends after a pause
(today's `STEPS_GAP`, 0.5 s, made a setting). This is what Helios's
parameter rows already do for keys and the wheel; the controls make it
the rule for every device.

### 3.6 Coordinate spaces for handles

Handles place points, lengths and angles. Each `point` and `angle`
declares the space its parameter is stored in:

| Space | Meaning | Used by |
|---|---|---|
| `image` | full-resolution pixels of the image the network reads, before orientation (`docs/masks.md`, "Image coordinates"); x right, y down, angles clockwise from x | drawn mask shapes today |
| `oriented-image` | the same pixels after the photo's orientation (the `flip` node's turn) | crop, straighten, what the person sees as "up" |
| `normalised` | `[0, 1]` across the oriented image | parameters meant to survive a resize (a vignette's centre) |
| `view` | screen points of the viewport | HUD anchors only, never stored |

The orientation rules are the ones the darktable drawn-mask import
settled (2026-11-08): a shape on the raw is turned as the `flip` node
turns the image (its orientation, or the EXIF tag when it is unset, none
when it is disabled), and angles turn with it (`helios_xmp::drawn`,
`helios_library::darktable::import_for`). The controls layer owns one
**mapping** per node instance: the chain from the parameter's space to
the viewport's image pixels (orientation, then the distortions upstream
of the node: crop, lens, perspective), and its inverse. A handle is
drawn at `forward(param)` and a drag writes `inverse(pointer)`. When a
distortion upstream has no inverse, the handle is shown faint and
read-only, with the reason on hover.

Lengths (a radius, a feather) are stored in their space's pixels and
scaled by the mapping's local scale; angles are composed with the
orientation's turn and mirror (a mirrored image reverses the angle's
direction).

### 3.7 Focus and selection across surfaces

- **One active control.** The control last touched, from any surface or
  device, is the **active control**. The panel highlights its row, the
  HUD shows its read-out, the image shows its handles selected, devices
  without a fixed mapping (a "current value" knob, the arrow keys with
  the image focused) adjust it.
- **One active module instance**, as today (the module card that is
  expanded and focused). Its composites show their handles; other
  modules' handles are hidden, or faint when the setting says so.
- **Selection inside a composite.** A click on a handle selects that
  part; the panel scrolls its row into view and focuses it; the wheel
  over the image adjusts the selected part (darktable's habit: the wheel
  follows the pointer), with modifier meanings from the composite's
  declaration (Shift + wheel: feather, Ctrl + wheel: opacity) and the
  pointer profile.
- **Keyboard focus** stays egui-pop's: Tab moves through the panel's
  controls in their logical order (3.2, packing), not their visual one; with the image focused, Tab cycles the active composite's
  handles (each handle is an AccessKit node with its value and actions).
- The active control and part are published as context keys
  (`control.active`, `control.part`), so keys and menus can be scoped to
  them.

### 3.8 Devices

A device produces **intents**, not values:

| Intent | Meaning |
|---|---|
| `nudge(n)` | n steps up or down (a key, a wheel notch, a relative MIDI encoder, a d-pad) |
| `set(v)` | an absolute position in the control's range, 0 to 1 (a MIDI fader, a gamepad trigger) |
| `drag(dx, dy)` | a 2D relative move (a mouse, a stick, a trackball) for points |
| `reset` | back to the default |
| `begin`, `end`, `cancel` | gesture edges a device can say (a touch-sensitive knob) |
| `pick` | start a pick on the image (a picker button) |
| `next`, `previous` | move the active control or part |
| `bank(n)` | show the next or previous bank of a rack on a device's encoders |

Each purpose maps onto physical controls in its standard way (3.2, the
ergonomic contract; the per-control table in 5.3), so a device works
with no setup: a bank of encoders follows the active card. A **binding**
overrides or adds to that, mapping a device input (`key: E + wheel`, `midi: cc 21`,
`gamepad: right-stick-x`, `console: knob 3`) to a target and an intent:
the target is a control by path (`exposure/exposure`), a part
(`masks/gradient.feather`), or **the active control** (the "current
value" binding a console's big dial wants). The device layer applies
the control's sensitivity, fine and coarse modifiers, acceleration and
step, then calls the control. Devices never write parameters
themselves.

Device backends are Helios plugins behind one trait (`Device`: poll
inputs, receive feedback such as a motorised fader's position or an LED
ring). Keys, the pointer and the wheel come from egui-pop's input and
pointer profiles; MIDI, gamepads and consoles are backends added later
(question Q3).

### 3.9 Customisation as settings

Customisation is data stored in the settings, in layers, never code:

| What | Example | Key |
|---|---|---|
| Variant | exposure as a knob instead of a slider | `controls.variant.<control>`, `controls.variant.<module>.<control>` for one module |
| Sensitivity, fine, coarse, acceleration | drag speed ×0.5, fine ×0.1 | `controls.feel.<control>`, or `controls.feel.<module>.<control>` for one module |
| Steps and detents | exposure in steps of 1/3 EV; the 0 EV detent off | `controls.steps.<control>`, `controls.detents.<control>` |
| Bindings | MIDI cc 21 to exposure | `controls.bindings` (a list) |
| Layout | the module card's controls reordered, some hidden, some in a "More" group; per control, a slot size within its policy, a span (columns), a pinned orientation and a pinned position (decided 2026-11-09) | `controls.layout.<module>` |
| Favourites | a quick panel of controls picked from any module | `controls.favourites` |
| HUD | which read-outs show, where, when | `controls.hud.*` |

Layers follow the admin note's value scopes: `app` (this person), and
`workspace` (a layout shared with a workspace or a profile; darktable
module groups are this). A **controls profile** is a named set of these
values (darktable-like, Lightroom-like), loaded like a keymap profile,
with per-setting overrides on top (question Q1).

Customisation stays inside the purpose and the ergonomic contract: a
person changes numbers (speed, steps, detents) and picks variants, never
the gestures' meanings (double click always resets, Escape always
cancels). How far this goes is question Q7.

The declaration gives the defaults; settings only override. A plugin's
control gets the same customisation for free, because the setting keys
are derived from the control's path.

### 3.10 How agents drive controls

Agents use the same path as everyone (egui-pop's agent control design):

- **Commands** `controls.list` (read only: the controls of a node with
  their purposes, variants, jacks and what they are patched to, values,
  ranges, parts), `controls.set`
  (`{node, control, value}` or `{…, part, value}`: one gesture, origin
  agent, one undo step), `controls.nudge`, `controls.reset`, and
  `controls.gesture` for a scripted drag (`begin`, a list of values,
  `end`), which tests preview and cancel.
- **A collector** `controls` gives the instantiated controls, the active
  control and part, each handle's position in view space (so an agent can
  aim `ui.*` pointer input at a handle and check the parameter after),
  and the device bindings.
- **Scenarios** check invariants over controls: every handle maps back
  to its parameter within a pixel (`inverse(forward(p)) == p`), every
  gesture ends in one undo step, a cancelled gesture leaves the store as
  it was.

Scripts (Lua or a plugin) call the same commands.

## 4. Where code lives

egui-pop is the visual layer underneath. Helios's controls layer uses it
and never reaches past it into egui.

| Layer | Crate | Holds |
|---|---|---|
| Slot layout | a standalone crate in its own repository, dual MIT OR Apache-2.0, depending on neither egui nor Helios (`egui_fit`, at https://github.com/quazardous/egui_fit; D2, decided 2026-11-09) | slot packing over plain boxes: the base unit and its fractions in twelfths, size policies, orientations picked from the slot's shape, smaller forms by index, dense deterministic grid packing with spans and pinned positions (D3), growth slot by slot, hysteresis. It knows nothing of controls, values or drawing. egui_hud is its first user. egui_stack's sections and toolbar overflow, and egui-pop's admin forms, may adopt it once the bench has proved it. It is not published before then. |
| Visual | egui-pop (`egui-pop-core`, `egui-pop-viewport`, `egui-pop-api`) | widgets (slider, range, curve, wheels), the viewport, overlays and handles, the canvas and its commands, the history contract, keys and pointer profiles, the agent bridge. Unchanged in spirit: visual parts, no meaning. |
| Framework | `egui_hud` (https://github.com/quazardous/egui_hud, its own repository since 2026-11-11; renamed egui_hud, 2026-11-10), agnostic of Helios: no `helios-*` dependency, only `egui_fit`, `egui-pop-model`, serde, and egui-pop behind its `render` feature. Developed in the Helios workspace under GPL-3.0-or-later, then moved to its own repository under MIT OR Apache-2.0 (option B, decided 2026-11-09; moved 2026-11-11) | the modules `model`, `slot`, `card`, `context`, `resolve`, `registry` (controls, patches, implicit controls from a `ParamDescriptor`), `flow` (on the `ValueSource` and `ValueStore` traits, with `MemoryStore` for tests and benches), `commands` (the `controls.*` commands as plain functions a host declares) and `render`. The rows below name the modules it holds. |
| Helios side | `helios-controls` | the business catalogue (exposure…), the mapping from `helios_values::Descriptor` and `ParamWidget` to the framework's descriptor, the adapters from `helios_values::Store` to the store traits and from the shell's command registry to the `controls.*` commands, and re-exports of the framework's modules for Helios's callers. |
| Model | `egui_hud`, module `model` (no egui) | the catalogue of business controls, value types, size policies and the slots they ask for (module `slot`; they replaced `Footprint` in slice 4), orientations (preferred and supported), the integration level per surface (managed or integrated, with the managed fallback), indicators and a case's chrome, connectors, cases, declarations and their serialisable form, bindings, derive and apply, coordinate spaces and mappings, intents, customisation types, resolution and diagnostics. |
| Renderers | `egui_hud`, module `render` (on egui-pop) | per control, variant, surface, orientation and representation, under the ergonomic contract: the standard managed renderers (egui-pop sliders, range sliders, check boxes, drop-downs, swatches, knobs, value-type fallbacks), HUD read-outs and mini-controls, integrated renderers (curve, wheels, handles from composites on egui-pop overlays); the fallback rule. |
| Runtime | `egui_hud` (modules `flow` and `commands`), wired by `helios-controls` | instantiation per node, the active control, gestures, `controls.*` commands and steps, the collector, the devices trait, the settings bridge. |
| Patches | each module (`modules/<name>`), each plugin | which controls they use and how each jack is patched to a parameter, as Rust builders over the model (no egui, so engine crates stay free of the interface) or the same data as JSON (out-of-process plugins). Today's `ParamHint` becomes the implicit control's source, then gives way to patches. |
| Devices | `plugins/devices/<name>` | MIDI, gamepad, console backends (Rust, out of process, as Helios plugins are). |
| Hosting | `helios-gui` | mounts panels, the HUD and handles in the darkroom; drops its hand-drawn editors as modules migrate. |

**The slot engine's core (D2b, decided 2026-11-09).** The slot crate
owns a small **quantized-grid core**: placements in twelfths of the
base unit, on a grid of slot columns, packed densely (D3). It borrows
taffy's vocabulary so a reader of either recognises the other:
`min_size`, `size`, `max_size`, `aspect_ratio`, and a grow factor along
the growth axis. It does not wrap taffy: taffy's continuous flexbox and
grid solve a more general problem, and the slot rules (allowed sizes
only, growth slot by slot, orientation from the slot's shape, smaller
forms by index) would sit on top of it as much code as they take on
their own. A taffy backend stays possible later behind the same
vocabulary.

**Small egui-pop additions** the controls need (visual primitives only):

1. A **knob** (dial) widget, with the slider's `Change` and gestures, and
   a **scrub field** (a number whose label drags), both from the same
   descriptor as the slider.
2. **Anchored HUD elements**: a small panel drawn over the viewport,
   anchored to an image point or to the pointer, kept inside the view,
   faded when idle, out of the hit path except its own controls.
3. A **scalar handle** in `overlay`: a handle whose drag gives a signed
   distance along an axis (a feather band's edge), so composites need not
   recompute it from points.
4. A **mapping hook** on the viewport: overlays given in a space with a
   forward and an inverse function, instead of image pixels only.
5. The image viewport on egui-pop's navigation model (already planned
   there), so handles and `view.*` commands share the canvas.
6. **Detents** on the slider, the range slider and the knob: a soft
   stop at given values, crossed by moving on, off with a modifier, and
   a hook for the response curve (perceptual, logarithmic).

Each is an issue of its own in egui-pop, useful to any application.

## 5. The first catalogue

Drawn from today's modules (section 1.2) and the darktable modules
Helios ports next. Each control has a purpose. The slot column gives
the panel slot (3.2, its preferred size: `row` is 1, `half` is ½,
`block(n)` is 1 × n), then the HUD form and the image form; a dash
means none. Variants are the swappable forms of the same control; the
first is the default.

**Size policy and orientation, as a rule** (proposed, to be checked
on the bench; in place of three more columns; a control that departs from the rule says so in its row):

- **Growable along the width**: sliders, range sliders and drop-downs
  (from ½ to n columns). **Fixed**: toggles, buttons, knobs, dials.
- **Growable with an aspect ratio of 1**: `grade-wheel`, `tone-curve`,
  the chromaticity plot (they grow by whole blocks, keeping square).
- **Orientation**: sliders, range sliders and the knob's strip are
  `horizontal` by preference and support `vertical` (fader strips,
  narrow docks); `targets` and `luminance-range` are horizontal only,
  being drawn on a horizontal track; wheels, curves, toggles, knobs and
  dials are `any`. Composites take the orientation of their parts'
  flow.

### 5.1 Simple controls: slot and connector

| Control | Connector (jacks) | Slot | Variants | Used by |
|---|---|---|---|---|
| `exposure` | `value: number<EV>` | row / chip / scrub | slider, knob (½), scrub field | exposure; vkdt `exposure`; a mask's exposure later |
| `black-level` | `value: number<relative>` | row / chip / - | slider, knob | exposure `black` |
| `feather` | `width: number<px image>` | row / chip / band of its shape | slider, knob | circle, ellipse, gradient, path masks |
| `opacity` | `value: number<percent>` | row / chip / - | slider, knob | every mask member, mask groups |
| `invert` | `on: bool` | half / - / - | toggle | every mask member |
| `angle` | `value: angle<degrees>` | half / chip / rotate handle | dial, slider | ellipse, gradient; crop's straighten |
| `radius` | `value: number<px image>` | row / chip / size handle | slider (logarithmic), knob | circle; blur `radius` |
| `temperature-tint` | `temperature: number<K>`, `tint: number` | block(2) / chip / picker | two sliders on colour tracks, a 2D pad (later) | white balance; channel mixer's illuminant |
| `rgb-coefficients` | `rgb: number[3]` | block(3) / - / picker | three sliders | white balance `red`, `green`, `blue` |
| `chromaticity` | `xy: point<xy>` | block(2) / chip / picker | two sliders, a chromaticity plot (later) | channel mixer `x`, `y` |
| `illuminant` | `kind: one-of` | row / - / - | drop-down | channel mixer |
| `contrast` | `value: number` | row / chip / vertical scrub | slider, knob | sigmoid `contrast` |
| `targets` | `range: range<display %>` | row / chip / - | range slider | sigmoid `black_target`, `white_target` |
| `highlight-clip` | `value: number<share of white>` | row / chip / clipped area shown | slider | highlights `clip` |
| `orientation` | `turn: one-of(EXIF, 0..7)` | half / - / - | rotate and mirror buttons | flip |
| `demosaic-method` | `method: one-of` | row / - / - | drop-down | demosaic |
| `blend-operator` | `op: one-of` | row / - / - | drop-down with icons, segmented | mask groups' `operator` |
| `grade-wheel` | `offset: colour<hcl>` | block(4) / chip / - | colour wheel, three sliders | vkdt `grade` lift, gamma, gain, offset |
| `tone-curve` | `points: points<0..1>` | block(6) / chip / tone picked on the image | curve editor | curve modules (none yet; `ParamWidget::Curve`) |
| `luminance-range` | `range: range<luminance>` | row / chip / range shown | range slider on a luminance track | luma mask `low`, `high` |

### 5.2 Composites (cases)

| Control | Holds | Outside jacks | Slot | Used by |
|---|---|---|---|---|
| `circle-mask` | centre (`point<image>`), `radius`, `feather` | `center`, `radius`, `feather` | block(3) / chip / gizmo | masks `circle` |
| `ellipse-mask` | centre, two `radius`, `angle`, `feather`, proportional (`invert`-like toggle) | `center`, `radii`, `rotation`, `feather`, `proportional` | block(5) / chip / gizmo | masks `ellipse`; a vignette later |
| `gradient-mask` | line (two points, derived), `angle` (derived), `feather`, profile | `anchor`, `rotation`, `feather`, `profile` | block(3) / chip / gizmo | masks `gradient` |
| `path-mask` | points with Bézier controls, `feather`, closed | `points`, `feather`, `closed` | block(2) / chip / gizmo | masks `path` |
| `brush-mask` | strokes (points with radius, hardness, density) | `points` | block(3) / chip / gizmo | masks `brush` (no drawing tool yet) |
| `crop` | box (`box<oriented>`), `angle` (straighten), aspect, guides | `box`, `angle`, `aspect` | block(3) / chip / gizmo | the crop module (none yet) |
| `white-balance` | `temperature-tint`, `rgb-coefficients`, mode, `colour-picker` | `mode`, `rgb` (temperature and tint derived) | block(5) / chip / picker | whitebalance |
| `colour-picker` | an area (point or box), mean or median | `area`, and a link to the control it feeds | quad button / chip / gizmo | any control that allows a pick |

### 5.3 Ergonomics: gestures, detents, devices

| Control | Gestures (beyond the common grammar) | Detents and snapping | Response and sensitivity | Device mapping |
|---|---|---|---|---|
| `exposure` | scrub on the image (held E, drag right brighter, as darktable's `e;scroll`) | 0 EV | perceptual (stops), 1 EV per 200 pt, fine 0.01, big step 1/3 stop | encoder (push: reset); fader over the soft range |
| `black-level` | - | 0 | linear, fine 0.0001 | encoder |
| `feather` | Shift + wheel over its shape (darktable's) | 0 (hard edge) | linear in pixels, scaled to the zoom | encoder |
| `opacity` | Ctrl + wheel over its shape | 100 % | linear, 1 % a step | encoder; fader |
| `invert` | click | - | - | button |
| `angle` | drag the rotate handle; Ctrl + Shift + wheel over the shape | 0°, 45°, 90° (Shift snaps every 15°, as today) | 1° per 4 pt, fine 0.1° | encoder with detents |
| `radius` | wheel over the shape (×0.97 a notch, darktable's) | - | logarithmic | encoder |
| `temperature-tint` | pick a neutral area | as shot; D50 and D65 | mired-even (equal steps look equal) | two encoders; a stick for the 2D pad |
| `chromaticity` | pick | the illuminant's white point | linear in xy | two encoders or a stick |
| `contrast` | vertical scrub on the image (held key) | the module's neutral | linear | encoder |
| `targets` | each thumb its own focus | the defaults | linear | two encoders |
| `orientation` | buttons, `[` and `]` | the EXIF value | steps of 90° | buttons |
| choices (`illuminant`, `demosaic-method`, `blend-operator`) | wheel steps when focused | - | one choice a step | encoder with steps; buttons |
| `grade-wheel` | drag in the wheel; luminance on its ring | neutral (centre) | fine with Ctrl | stick (hue and chroma) and encoder (luminance) |
| `tone-curve` | drag a node, wheel moves it vertically, Ctrl + click adds at x | the identity line | fine with Ctrl | encoder on the selected node, next and previous on buttons |
| `gradient-mask` | drag the line (move), the ends (rotate and place), the band (feather); Shift + click switches profile | rotation at 0°, 90° | as its parts | stick moves the anchor, encoders for angle and feather |
| `ellipse-mask` | drag the centre, axes, rotate and feather handles; wheel radii, Shift + wheel feather | rotation at 0°, 90°; circle when the radii meet | as its parts | stick for the centre, encoders for the rest |
| `crop` | drag corners and edges; Shift around the centre; drag outside the box rotates; draw the horizon with the right button (darktable's) | the image's edges, the aspect's sizes, 0° | linear | stick moves, encoders for size and angle |
| `colour-picker` | click a point, drag a box | - | - | button picks at the HUD's centre |

Every handle keeps a 24-point hit area. Every control in a card is
reachable from a bank of eight encoders in the card's order.

### 5.4 The first ones to build

1. `exposure`: the smallest control with every surface (panel, HUD,
   scrub on the image), the ergonomic contract and the full value flow.
2. `feather` and `angle`: small, and parts of every mask.
3. `gradient-mask`: the first case, with derived parts; replaces
   `drag_writes` for the gradient.
4. `ellipse-mask`, then `circle-mask` and `path-mask`, retiring
   `drag_writes` shape by shape.
5. `crop`: the first control in the oriented space with an upstream
   mapping, and the first with guides.
6. `colour-picker` and `white-balance`: the picker event finally
   answered.

## 6. Migration order

**How today's generated panel maps.** Nothing breaks on day one. A
parameter with no control bound gets an **implicit control** built from
its descriptor and hints, exactly the row `param_ui_with` draws today
(slider, check box, combo box, curve, wheel, list). Modules then bind
business controls one by one; an implicit control is counted by the
inspector, so the remaining work is visible. The `group` hint becomes
the card's sections; the row marks (changed, held, moved by someone
else, past the soft range) become the panel rendering's, for every
control.

1. **Exposure.** `helios-controls` with the model, the runtime and the
   panel renderer; the implicit controls over `param_ui_with`; the
   exposure module binds `exposure` and `black-level`. Then the HUD
   read-out and the scrub on the image (a held key and a drag), then
   `controls.*` for agents. This proves the value flow on one parameter.
2. **A gradient or ellipse mask.** `feather`, `angle`, then
   `gradient-mask` with its derived line; the masks module binds it, and
   `helios_gui::masks` draws its handles through the control instead of
   `drag_writes`. Then `ellipse-mask`, `circle-mask`, `path-mask`.
   `mask.handle.move` becomes an alias of `controls.set` on the part.
3. **Crop.** A crop module (none exists), its `crop` control in the
   oriented space, with the mapping through the modules upstream.
4. **The rest of today's modules**, then devices (keys bound to
   controls first, MIDI and gamepads per Q3), then customisation pages
   in the admin framework (the controls' settings under Input).

## 7. Slices

Sizes: S is a day or less of agent work, M two or three days, L a week.

Slices 1 to 9 and 5b are done; slice 2 is this note's reconciliation with the
decisions of 2026-11-09. The slices after it were renumbered that day:
the slot engine, contexts and the bench come first, then the masks and
customisation slices of the 2026-11-08 plan.

| # | Slice | Size | Depends on |
|---|---|---|---|
| 1 | **Done** (2026-11-09). `helios-controls`: model, registry and `controls.describe`, the value flow with the `controls.change` step, implicit controls from descriptors, the `exposure` control patched by the exposure module, the `controls.*` commands and collector, the gallery card | M | none |
| 2 | **Done** (2026-11-09). This note and `control-contexts.md` reconciled with the decisions of 2026-11-09 | S | 1 |
| 3 | **Done** (2026-11-09). The slot crate v0, `egui_fit`: the twelfths grid, size policies, orientation from the slot's shape, dense deterministic packing, pinned positions, growth slot by slot, hysteresis; pure unit tests, no egui | M | none |
| 4 | **Done** (2026-11-09). `helios-controls` on the slot engine: size policies, orientations and integration levels in the model in place of `Footprint`; cards laid out through it | M | 3 |
| 5 | **Done** (2026-11-09). Contexts and `resolve` per card (case): the context keys, the box and the density in, a `Placement` per control out, through the slot engine; the reachability invariant. Built: `helios_controls::context` (keys, classes, tiers, rules), `helios_controls::resolve` (precedence, layouts, "More (n)", anti-jump, `reachable`, `find`), the `controls.context`, `controls.find`, `controls.reveal`, `controls.reveal_all` commands, placements in `controls.describe` | M | 4 |
| 5b | **Done** (2026-11-09). The agnostic framework extracted from `helios-controls` into `egui_hud` (option B): model, slot, card, context, resolve, registry, value flow, commands and render, on its own value type, parameter descriptor and store traits; `helios-controls` keeps the catalogue and the adapters | M | 5 |
| 6 | **Done** (2026-11-10). Renderers by representation and orientation, under the ergonomic contract. Built: `egui_hud::render::card` draws a card from its resolution (each shown part at its rect, its chrome, the "More (n)" row as egui-pop's section band with its count, the reasons on hover, a part revealed through `controls.reveal`; in a toolbar, the More tools glyph); `render::part` by placement (numbers as egui-pop's slider, full, compact or vertical, its knob, or a scrub read-out; a toggle, a drop-down, `points` as numbered rows, a `colour` as a swatch and channels, text); an integrated part drawn by the host's `Drawings` where its full form fits, else its fallback (a variant, the value type's renderer, a case's parts), curve and wheels covered. egui-pop gained the vertical slider, the knob, sliders down to 24 points and a standalone section band. The controls gallery draws through this path, with snapshots per view | M | 4 |
| 7 | **Done** (2026-11-10). Bench A: the gallery's shelf and bench, controls moved from the shelf to the bench (`controls.bench.*`) and packed live. Built: a bench page in the controls gallery; the shelf (the catalogue by purpose, an implicit control per value type, searchable, each with a painted preview of its form); the bench as one card of the composition (controls, and cases as a legend, help text, their parts and a separator) resolved through `resolve` and drawn by `render::card`; drops shown as a ghost of the slot they would take; a layout mode to move, select, group and remove; a width handle, the placements following with hysteresis; values in egui_hud's `MemoryStore` through the value flow, which gained undo and redo; the commands `controls.bench.add`, `remove`, `move`, `case`, `list`, `clear` | M | 5, 6 |
| 8 | **Done** (2026-11-10). Bench B: the context switches on the bench and the measures as agent scenarios. Built: a context bar over the bench setting every key (task, detail, image flags, tool, device, density) through the context the `controls.*` commands share, so `controls.context` and the bar agree, with the space the box's width gives; changes wait while the pointer is over the bench (anti-jump); a mode switch (panel, dock, toolbar, float with their slot grids; the HUD as chips, a control without one named, gizmos listed); a placement and reason pane (slot, orientation, representation, visibility, why, and the rule, layout or engine that decided); per control a tier and a variant, card rules (`controls.bench.part`, `rules`, `mode`); a bench saved and loaded as data with its context, mode, width and the person's layout as the `controls.layout.bench` setting (`controls.bench.save`, `load`); the scenarios' bench (`controls.bench.sample`); S1 to S6 as headless tests writing text reports (`tests/reports`). Open: a toolbar is not yet one line (the slot engine has no height limit), and S6 has no device abstraction, so its sweep is synthetic nudges | M | 7 |
| 9 | **Done** (2026-11-10). The darkroom panels through the engine: `param_ui_with` replaced by the managed renderers, no visible change beyond the packing; snapshots updated knowingly. Built: a card per node (per section of a macro's exposed parameters) resolved in the shared context and drawn by `render::card` with Helios as its host (`render::Drawings`: labels, choices, the picker, the `slider.change` and `controls.change` steps, marks, names, menus, focus); `black-level` bound to exposure's `black` | M | 5, 6 |
| 10 | egui-pop: knob and scrub field; anchored HUD element | M | none (egui-pop issues) |
| 11 | HUD and the exposure scrub on the image; the active control and its context keys | M | 9, 10 |
| 12 | `feather`, `angle`, `gradient-mask` with derived parts: integrated on the image plus a managed fallback in the panel; its part of `drag_writes` removed; scenario with invariants. **Handles built** (2026-11-11), with `circle-mask` from slice 13: `helios_controls::gizmo` declares each case's handles (the part each shows, the parameters it writes, its Shift snap, its double-click reset, its wheel) as one more surface of the controls its parameters have in the panel (patched, else implicit): a drag is one store gesture holding those parameters, then the `controls.change` step with each parameter's control and one undo step; a reset and a wheel turn the same; Escape cancels; `mask.handle.move` drags through it. The circle's, the ellipse's and the gradient's parts of `drag_writes` are gone; headless tests per handle (`helios-gui/tests/mask_handles.rs`) held the behaviour. Open: the `feather`, `angle` and `radius` business controls patched to the masks' parameters (egui_hud's patch has no per-patch range or default, and a mask's feather default differs per shape), the gradient's derived line part, the HUD chip (slice 11), the scenario with invariants. The ellipse's handles moved too (slice 13) | M | 11 |
| 13 | `ellipse-mask`, `circle-mask`, `path-mask`, each integrated on the image plus a managed fallback; `drag_writes` removed. `circle-mask`'s and `ellipse-mask`'s handles built with slice 12; the path's are left (its corners, Bézier handles and points added or removed) | M | 12 |
| 14 | Customisation as settings (variants, feel, steps, layout with slot sizes, spans and pins, favourites), profiles; pages declared in the admin framework | M | 9 |
| 15 | Keys bound to controls (nudge, reset, the active control) in the keymap | S | 11 |
| 16 | egui-pop: mapping hook on overlays, scalar handle; the image viewport on the navigation model | M | none (egui-pop issues) |
| 17 | Crop module and `crop` control, in the oriented space | L | 13, 16 |
| 18 | `colour-picker`, `white-balance`, `temperature-tint`, `chromaticity` | M | 12 |
| 19 | The device trait and a MIDI backend with feedback (if Q3 says v1) | M | 15 |
| 20 | Gamepad and console backends | M | 19 |
| 21 | Plugin-declared controls and composites over the plugin protocol | M | 13 |

## 8. Open questions

**Q1. How far customisation reaches (who it belongs to).**
- A. Per person (`app`) with an optional `workspace` layer for layouts
  and favourites; per-module overrides of a control's feel allowed.
- B. Per person only, one set everywhere.
- C. Per person, per workspace and per module instance (two exposure
  instances tuned apart).
- Recommended: **A**. A person's hands are the same in every workspace,
  so feel and bindings are theirs; a layout is often a workflow's
  (darktable's module groups), so workspaces may carry one. Per instance
  is rare and makes "why does this slider feel different" hard to answer.

**Q2. The HUD: always on or summoned.**
- A. Summoned: chips appear while a control is handled (any surface or
  device) and fade after; a held key shows the active card's chips.
- B. Always on: the person pins chips that stay over the image.
- C. Off by default, a toggle in the view menu.
- Recommended: **A**, with pinning (B) as a later option. Every app
  surveyed summons its HUD; a permanent layer over the photo hides the
  photo.

**Q3. Devices in the first version.**
- A. Keys, pointer and wheel in v1, with the `Device` trait and the
  intents defined (agents and tests drive them); MIDI as the first
  backend just after; gamepads and consoles later.
- B. MIDI in v1 too.
- C. MIDI, gamepad and a console in v1.
- Recommended: **A**. The intents and the standard mappings are what
  matter, and tests prove them without hardware; MIDI is then a plugin of
  a few hundred lines, as darktable's `midi.c` shows.

**Q4. How controls and patches are declared.**
- A. Rust builders only.
- B. Data files (TOML or JSON) only.
- C. Rust builders producing serialisable data; out-of-process plugins
  send the same data as JSON.
- Recommended: **C**, as the admin note's Q5: types and compile errors
  in process, data for plugins and agents.

**Q5. Who may add controls and cases.**
- A. Helios's catalogue is curated; plugins may add **cases** built from
  catalogue controls, and controls of their own as integrated controls
  (their renderer plus a managed fallback); a control with no managed
  fallback is an exception, registered and counted; people may not.
- B. Plugins may add any control; people may build cases in an editor.
- C. Catalogue only; plugins patch existing controls.
- Recommended: **A**. Cases keep the rack standard (their parts follow
  the ergonomic contract); a free-for-all brings back hand-drawn widgets;
  a case editor for people can come once the catalogue is stable.

**Q6. The set of footprints.** *Superseded on 2026-11-09 by standard
slots (3.2, D1 to D4): a grid of base units after all, counted in
twelfths, with dense packing. Kept below for the record.*
- A. `half`, `row`, `block(n)` in the panel, `chip` in the HUD, `gizmo`
  on the image.
- B. A grid: units of a quarter row, controls 1 to 4 units wide and 1 to
  n rows high (Eurorack's HP).
- C. `row` and `block(n)` only.
- Recommended: **A**. A darkroom panel is narrow: half and full rows
  cover every control in section 5, and a grid adds layout decisions
  people did not ask for.

**Q7. How much of the ergonomics a person may override.**
- A. Numbers only: sensitivity, acceleration, steps, detents on or off,
  the fine and coarse factors, variants; the gestures' meanings are
  fixed (double click resets, Escape cancels).
- B. As A, plus remapping gestures per control (the wheel over a shape
  does feather instead of size).
- C. Everything, gestures included, per control.
- Recommended: **A**, with modifier meanings left to the pointer
  profiles that already exist. The point of the standard is that one
  learns it once; gestures that change per control defeat it.

**Q8. Other modules' handles on the image.**
- A. Only the active module's handles; others hidden.
- B. Others shown faint, clickable to activate their module.
- C. A setting, A by default.
- Recommended: **C**. A is darktable's habit and keeps the photo clear;
  B helps with several masks, and a setting costs little.

**Q9. Where handle parameters are stored.**
- A. As today: image pixels of the image the network reads, mapped to
  the view through the orientation and the upstream distortions.
- B. In the oriented image's pixels.
- C. Normalised to the oriented image.
- Recommended: **A**. It is what masks and the darktable import already
  use (`docs/masks.md`), and a turn of the photo leaves the shapes where
  they were on it; crop is the exception and declares `oriented-image`.

**Q10. `param.*` and `controls.*`.**
- A. Both: `param.*` stays the raw path to a value; `controls.*` adds
  purposes, parts, derived values and gestures.
- B. `controls.*` only, `param.*` removed.
- C. `param.*` only, extended with parts.
- Recommended: **A**. Scripts and the clipboard address values by path,
  and should keep doing so; agents that act as a person use controls.

## 9. Sources

Links read from memory on 2026-11-08, not fetched again.

- darktable: the sources named in section 2.1 (`src/bauhaus/bauhaus.c`,
  `src/develop/imageop_gui.c`, `src/develop/develop.c`,
  `src/gui/accelerators.c`, `src/libs/tools/midi.c`,
  `src/libs/tools/gamepad.c`, `src/develop/masks/`, `src/iop/crop.c`,
  `src/gui/color_picker_proxy.c`, `src/libs/modulegroups.c`);
  [Shortcuts](https://docs.darktable.org/usermanual/development/en/preferences-settings/shortcuts/),
  [Drawn masks](https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/drawn/).
- Helios: `docs/shortcuts-research.md`, `docs/masks.md`,
  `docs/design/undo.md`, `docs/design/mask-display.md`.
- egui-pop: `docs/design/admin-forms.md`, `docs/design/pointer-gestures.md`,
  `docs/design/navigation.md`, `docs/design/agent-control.md`,
  `docs/viewport.md`, `docs/sliders.md`, `docs/image-tools.md`,
  `docs/history.md`, ADR 0009.
- Lightroom Classic: [Local adjustments and masking](https://helpx.adobe.com/lightroom-classic/help/masking.html);
  MIDI2LR ([github.com/rsjaffe/MIDI2LR](https://github.com/rsjaffe/MIDI2LR)).
- Capture One: [Customising tool tabs](https://support.captureone.com/hc/en-us/articles/360002571798)
  (unconfirmed address).
- DaVinci Resolve: the Reference Manual, "Power Windows" and "Control
  Panels" chapters ([training page](https://www.blackmagicdesign.com/products/davinciresolve/training)).
- Blender: [Gizmo](https://docs.blender.org/api/current/bpy.types.Gizmo.html),
  [GizmoGroup](https://docs.blender.org/api/current/bpy.types.GizmoGroup.html).
- Photoshop: [Gradient tool](https://helpx.adobe.com/photoshop/using/gradient-tool.html),
  the brush's on-canvas size and hardness (Alt + right drag; Ctrl +
  Option drag on macOS).
- Affinity Photo: [help](https://affinity.help/photo2/), the context
  toolbar and the Gradient tool.
