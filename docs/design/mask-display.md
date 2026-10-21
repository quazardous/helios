# Mask display: seeing what a module or a mask node affects

Status: **decided** (2026-10-21): the owner accepted this note with the
default of every choice; building follows its plan. Proposed on
2026-10-14.

A mask is hard to judge from the image alone: a soft feather, an
intersection that leaves almost nothing, a parametric range that catches
the sky as well. darktable answers with a mask display toggle. Helios
needs the same, and its masks being nodes changes how it is built.

## What darktable does

From the darktable sources (5.x):

- **The toggle.** A "display mask" button in each module's blending
  section, next to "temporarily switch off the mask"
  (`src/develop/blend_gui.c`), and the mask indicator in the module's
  header (`src/develop/imageop.c`). Both set the module's
  `request_mask_display`; modules that do not support blending have
  neither.
- **What it shows.** The module's final mask only (drawn, parametric and
  raster together), never one shape or one group of it.
- **How it shows.** The final module (`src/iop/gamma.c`) composites:
  the image turned grey (a mix of luminance, preference
  `darkroom/ui/develop_mask_mix`, 0.3 by default), then blended towards
  a hard-coded yellow by the mask's value. The colour is not themeable.
  A second mode, the **channel display**, shows one blendif channel of
  the input or output (L, a, b, R, G, B, hue, chroma, …) in false colour
  or grey (preference `channel_display`), with the yellow overlay on top
  when both are asked.
- **How it is toggled.** A click toggles the overlay; Ctrl+click the
  overlay, Shift+click the channel display, both for both. On the
  parametric sliders, entering one with Shift or Ctrl shows the channel
  or the mask while the pointer stays, restored a second after it leaves;
  keys while a slider has focus: `c` channel, `C` channel kept after
  leaving, `m` mask, `a` the alternative display.
- **While editing.** It stays on while the module is edited (sliders,
  shape drags). It is turned off when the module loses focus, together
  with the mask edit mode; undo restores it.
- **Per module.** Some modules show their own mask views through the
  same pipe field: tone equalizer and color equalizer show their output
  unprocessed by later modules, retouch and color zones their own masks;
  color balance rgb and tone equalizer draw their own, exclusive with
  the toggle.
- **In the pipe.** The mask travels in the alpha channel: the blend
  writes it (`blendop.cl` on OpenCL, the blend functions on the CPU),
  later non-distorting modules are bypassed and copy it through,
  distorting modules still run so the mask stays aligned, and gamma
  draws it. The pipe cache is disabled for the run (two lines alternate,
  hashes invalidated), and only the full pipe shows it: never the
  preview pipe, never an export.
- **Shape outlines** are a separate system: cairo overlays of the mask
  edit mode, drawn over the center view, shown alongside the mask
  display.

## What changes in Helios

A mask is a node: a one-channel image on an output port
(`docs/masks.md`). Showing it is rendering one more port, through the
same engine and cache, and compositing it over the image in the viewport.
Nothing downstream is bypassed, nothing is carried in alpha, the cache
is not disabled, and any node of the tree can be shown, not only the
final mask.

## Choices

### 1. What is shown

- (a) The final mask of the selected module (what its `mask_blend`
  reads), as darktable does.
- (b) The output of the node selected in the Masks panel: a member, a
  group, a refinement, or the root.

**Decided (2026-10-21): (b), falling back to (a).** With nothing selected in the
tree, the root shows: the final mask. Selecting a member shows that
member alone, as it reaches its group (after its refinements, before the
group's opacity and inversion of it), which is what one checks when a
group's result surprises. The blend's own opacity is not applied: the
display says where the module acts, not how much.

### 2. How it is shown

- (a) An overlay: the image as it is, a colour laid over it by the
  mask's value.
- (b) The mask alone, in grey (0 black, 1 white).
- (c) Both, as modes of one toggle.

**Decided (2026-10-21): (c), the overlay first.** The overlay's colour and
opacity are theme tokens (`mask.overlay.color`, defaulting to the
theme's accent; `mask.overlay.opacity`, 0.6), never a hard-coded yellow,
so a theme can keep it visible on its own image surround. The image
under the overlay is dimmed towards grey as darktable does (a token
too, `mask.overlay.desaturate`), so the colour reads on colourful
images. The grey mode is the exact mask, for judging a feather's
falloff. darktable's channel display is left for later: it belongs with
the parametric member's editor, not the viewport toggle.

### 3. How it is toggled

**Decided (2026-10-21): all three, one value behind them.** The state is a view
value, `view/viewport/mask_display` (`off`, `overlay`, `grey`),
transient: never an undo step, never saved with the edit, like the
zoom.

- a command, `viewport.mask.display`, cycling off, overlay, grey (and
  `viewport.mask.display.off` to leave), in the palette and for agents;
- a key in the viewport, proposed `M` (darktable's mask key), checked
  against the keymap before it is taken;
- a button in the Masks panel's action row, an eye-on-mask icon, its
  hint naming the mode and the node shown.

### 4. Whether it stays on while editing

**Decided (2026-10-21): it stays on**, through handle drags, parameter changes,
an agent's edits and undo, and follows the tree's selection. It goes
off only when toggled, or when the selected module has no mask (it
comes back with the next module that has one, since the value is the
user's choice, not the module's). During a drag the mask renders as the
image does: the proxy first, then the exact tiles. Shape outlines stay
drawn over it.

### 5. How agents and screenshots read it

- **MCP.** `helios_render` gains an optional `node` and `port`, so an
  agent renders any mask node's output as a grey PNG without a window;
  the display value is set like any view value.
- **GUI pictures.** `gui.snapshot viewport.display` already pictures
  what is on screen, the overlay included; `viewport.scene` stays the
  image's float data.
- **masks-shots.** The bench job adds two pictures after its members
  are added: the overlay on the root, then grey on one member, through
  the palette as a person would.

**Decided (2026-10-21): all three**, the MCP argument first since it serves
tests too.

### 6. GPU path and preview pipe

- **Engine.** The viewport asks for a second port's tiles over the same
  regions and scale as the image's. The mask's nodes are already part
  of the image's render (the blend reads them), so their tiles are
  usually in the cache: showing the mask costs a lookup, not a render.
  On the GPU, the mask modules' WGSL variants keep the result in a
  device buffer (`Engine::render_device`), shown without a copy back.
- **Viewport.** egui-pop's viewport draws one tile source today. It
  needs an overlay layer: a second, one-channel source composited in its
  shader with a colour, an opacity and a desaturation, or drawn alone
  in grey. That is an egui-pop change, generic enough to belong there
  (a heat map or a focus peaking layer would use it too).
- **Geometry.** The mask is the module's input geometry. While Helios
  has no distorting module after a masked one, the layers align; when
  one comes (lens correction, crop), the overlay must go through the
  same transforms as the image, which a network does by running the
  mask through those nodes' geometry, not by an alpha trick.
- **Preview.** The navigator and the scopes keep measuring the image,
  never the mask, as darktable's preview pipe does. An export never
  shows it.

## Build plan

1. The view value and the command (`viewport.mask.display`), the node
   chosen as in choice 1, the Masks panel's button and the key; tests
   on the value and the chosen node.
2. egui-pop: the viewport's overlay layer (overlay and grey modes), with
   its own tests and pictures; then Helios's theme tokens for it.
3. Helios: the second tile source from the engine, the mask's port at
   the image's regions, GPU buffers kept on the device; a test that the
   overlay follows an edit and an undo.
4. `helios_render` with `node` and `port`; the masks-shots pictures.
5. Docs: `docs/gui.md` (the viewport and the Masks panel) and
   `docs/masks.md`.

## What is built (2026-10-21)

- **Choices 1 to 4** as decided: `crate::mask_display` in `helios-gui`
  (the value `view/viewport/mask_display`, the commands
  `viewport.mask.display` and `viewport.mask.display.off`, `M` in the
  image, the Masks panel's Show mask button), the node chosen from the
  Masks panel's network. `M` was free in the image's keys (`C`, `E`,
  `G`, `P` draw shapes there).
- **Choice 2's tokens**: `mask.overlay.color` (the accent),
  `mask.overlay.opacity` (0.6) and `mask.overlay.desaturate` (1: the
  image grey under the overlay, as darktable shows it), in each of
  Helios's themes. The colour is mixed in encoded sRGB, as darktable
  mixes its yellow; the grey mode is exact on an sRGB display.
- **Choice 6, first step**: the overlay is composited on the viewport's
  render thread (`EngineSource`), on the CPU tiles the engine renders
  for the image and the mask over the same unions and scale, before the
  display transform. It needed no egui-pop change. The overlay layer in
  egui-pop's viewport shader, with the mask kept on the device, stays
  the next step for the GPU path (build plan, steps 2 and 3).
- **Choice 5**: `helios_render` already took a `node` and a `port`,
  but answers a job, not a picture: the MCP argument is `as_mask` on
  `helios_export`, which writes a mask node's port as the grey mode
  shows it (0 black, 1 white) rather than as light. Pictures:
  `gui.snapshot viewport.display` shows the display, and
  `helios-gui --mask-display overlay|grey` starts with it on.
