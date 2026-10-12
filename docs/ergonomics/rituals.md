# darktable's interaction rituals: inventory

Companion to [ergonomics.md](ergonomics.md), section 4.

Research date: 2026-10-12. Sources:
- darktable `master` source (snapshot of 2026-10-12).
- The development user manual at https://docs.darktable.org/usermanual/development/en/.
- A few GitHub issues.

Tags:
- **[code]**: read in the source, not confirmed by the manual.
- **[guess]**: inference.
- Untagged: confirmed by the manual. Where the manual and the code disagree, both are given.

URL shorthands:
- `M:` = https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/
- `S:` = https://github.com/darktable-org/darktable/blob/master/src/

---

## 0. Shared machinery (applies to every module picker)

**Sources:**
- S:gui/color_picker_proxy.c and S:gui/color_picker_proxy.h
- S:libs/colorpicker.c
- S:common/color_picker.c
- S:develop/pixelpipe_hb.c
- Manual: https://docs.darktable.org/usermanual/development/en/darkroom/processing-modules/module-controls/

**Declaration.** A module creates a picker with `dt_color_picker_new(module, flags, widget)`, or with `dt_color_picker_new_with_cst(..., cst)` to force a colour space. Results come back asynchronously through `color_picker_apply(module, picker_widget, pipe)`. The module then reads these buffers:
- `picked_color` (mean), `picked_color_min`, `picked_color_max`: module **input**.
- `picked_output_color*`: module **output**.

**Flags (S:gui/color_picker_proxy.h):**
- `POINT`, `AREA` or `POINT_AREA`.
- `DENOISE`: B-spline blur before sampling, 4-channel images only.
- `IO`: also samples the output. "all pickers sample input, only ones with this flag set sample output".

**Two UI forms:**
- A standalone toggle button with a pipette icon.
- A "quad" button embedded at the right end of a bauhaus slider. The picker then drives that slider.

**Ritual:**
- Point mode: click the icon, then click a pixel. A cross appears, and later clicks move it.
- Area mode: **ctrl+click or right-click** the icon, then drag a rectangle (you can redraw it or drag its corners).
  - Right-click on the image resets the area to the whole image.
  - The ctrl/right-click mode switch only matters for `POINT_AREA` pickers. `AREA`-only or `POINT`-only pickers ignore it. [code]
  - The manual says "Ctrl+clicking anywhere on the image will switch modes" when both modes exist.
- Click the icon again to turn the picker off.

**Hidden state and gotchas:**
- **One picker at a time, globally.** Turning one on silently turns the previous one off, in any module and including the global picker. There is "at most one editable sample, with one proxy, at one time in the center view". [code]
- **Activating a picker switches the module on** (`module->off` is set active) and gives it focus. [code]
- **Each picker remembers its own last point and box** (`pick_pos` / `pick_box`). The first use starts from a default box: the "98%" box the negadoctor manual mentions. The memory lasts for the GUI session only; it is not stored in params. [code]
- **Auto-off rules:**
  - Changing any widget of the module turns its picker off (`dt_iop_gui_changed` calls `dt_iop_color_picker_reset(module, TRUE)`).
  - Losing focus turns it off too.
  - Exception: pickers whose widget name is `"keep-active"`. These are the "show colour" pickers of colour zones, rgb curve, colour equalizer and the parametric mask. [code]
  - The global picker turns off on any focus change, unless "restrict scope to selection" is on. [code]
- **Where and when sampling happens:**
  - On the **preview pipe** (low resolution), in the module's working colour space (`default_colorspace`) unless the cst is forced. [code]
  - The result arrives later, when that pipe finishes. Modules re-read **modifier keys at apply time** (`dt_key_modifier_state()`), not at click time. See section 3. [code]
- **Picked values are one-shot.** They are copied into params at pick time. A later upstream change does not re-run the picker; the user must pick again. [code; implied by the manual for tone equalizer and filmic]
- **No standard way to show what was picked.** Some modules print L/h/c labels, some draw a line on a graph, some show nothing.

---

## 1. Module entries

### 1. Color calibration (channelmixerrgb)

M:color-calibration/ — S:iop/channelmixerrgb.c

**Ritual A: CAT illuminant picker (white balance).**
- Prerequisite: set the white balance module to "camera reference", or both modules warn.
- Open the CAT tab, then click the picker to the right of the illuminant colour patch. It is `AREA` only, so a plain click already gives a rectangle.
- Drag the rectangle over a neutral area, or over the whole image if nothing is neutral ("gray-world").
- The module then switches the illuminant to custom, daylight or Planckian. The visible sliders change: temperature, or hue/chroma.

**Ritual B: area colour mapping.**
- Expand "area color mapping".
- Set "area mode" to **measure** and pick the control sample. This fills the target L/h/c.
- On the next image the mode **auto-resets to correction**. Pick the sample again; the illuminant is solved so that the sample maps to the target.
- Keep "take channel mixing into account" the same in both steps.

**Ritual C: colour checker.**
1. Turn on lens correction first, for vignetting.
2. Expand "calibrate with a color checker" and choose the chart.
3. Drag the 4 corners of the on-canvas grid onto the chart's corner marks. Double-click resets the grid. [code]
4. Click refresh and read the ΔE report.
5. Click validate, or change "optimize for" and refresh again.
- Optionally copy the suggested normalization values into exposure.

**Ritual D: "detect from image surfaces/edges" (AI white balance).**
- These illuminant options exist in code (`DT_ILLUMINANT_DETECT_SURFACES/EDGES`), but they are **compiled out**: `// #define AI_ACTIVATED`.
- They are absent from the manual. When enabled, they run on the full pipe only. [code]

**Hidden state and gotchas:**
- **The CAT picker and area mapping share one picker and one code path** (`_auto_set_illuminant`).
  - In "correction" mode, the picked colour is mapped onto the mapping target (L/h/c).
  - The target is "sticky": it lives in darktablerc, not in params, and survives a module reset.
  - So if the target is not neutral (chroma ≠ 0), the "white balance" picker silently produces a cast.
  - The section label only shows "(active)".
  - Issue https://github.com/darktable-org/darktable/issues/18299 proposes a neutral-only WB picker plus a separate picker inside the mapping section.
- In "measure" mode the same picker writes the target and does **not** change params. One icon, two behaviours, chosen by a combobox that lives in another section.
- The picker reads module **input**, in pipe work RGB converted to XYZ. Reading after the channel mixer is a computed option ("take channel mixing into account"), not a second sample. [code]
- The checker overlay is only active while profiling is started, and it exists only on canvas. [code]

**Data needed:**
- A, B: the mean RGB of a region at module input, plus a target chromaticity (neutral by default). The output is the illuminant xy.
- C: a quadrilateral (4 image points) locating the chart. The module samples the patch centres, scaled by "patch scale".

**Statistic:** mean, area.

### 2. White balance (temperature)

M:white-balance/ — S:iop/temperature.c

**Ritual:**
- Click the pipette in the settings button row ("from image area"), then draw a rectangle over something neutral.
- The setting switches to "from image area" (`DT_IOP_TEMP_SPOT`).
- Moving temperature/tint or the coefficients switches it to "user modified".

**Hidden state and gotchas:**
- The picker runs with `IOP_CS_NONE`, so it samples **raw camera RGB** before demosaic (or RGB for non-raw files), depending on what the pipe holds. [code]
- Coefficients are normalised to green and clamped to [0,8]. [code]
- Only the coefficients are stored; temperature and tint are a camera-specific view of them.
- The modern workflow wants "camera reference" here, with WB done in color calibration instead. So there are two competing WB pickers in two modules.

**Data needed:** the mean camera RGB of a neutral region at module input. The output is 4 multipliers.

**Statistic:** mean, area only.

### 3. Exposure

M:exposure/ — S:iop/exposure.c

**Ritual A:**
- The quad picker on the exposure slider. Its tooltip: "set the exposure adjustment using the selected area".
- Draw an area; exposure is set so that the area's mean reaches the target lightness.

**Ritual B: area exposure mapping.**
- The same two-step measure/correction ritual as color calibration.
- Set "area mode" to measure and pick, which fills the target L.
- On the next image (mode resets to correction), pick again.

**Hidden state and gotchas:**
- Ritual A *is* ritual B: the same `_auto_set_exposure` branches on `spot_mode`. In measure mode the picker does not change exposure. [code]
- The target L is sticky in darktablerc and survives a module reset.
- The code assumes it sits before input colour profile ("assuming we are before colorin"): it converts with the *input* profile. The manual: for JPEG the L metrics are wrong unless the module is moved.
- The picked colour is converted to grey (a = b = 0); only luminance is used. [code]
- Automatic mode (raw only) uses a percentile and a target level, with no picker.

**Data needed:** the mean luminance of a region at module input, plus a target L*.

**Statistic:** mean, area.

### 4. Color zones

M:color-zones/ — S:iop/colorzones.c

**Ritual:**
- First choose **"select by"** (hue, lightness or chroma). Changing it resets all curves.
- Then choose the tab: the channel to modify.
- **Picker 1** ("show color", `POINT_AREA`, LCh, keep-active):
  - Click for point mode; ctrl+click or right-click for area mode.
  - It only *displays* the position (and the area range plus median line) on the graph's x axis.
- **Picker 2** ("create curve", `AREA`, LCh):
  - Drag an area and the current curve is **reset**.
  - 5 nodes are then placed at min−feather, min, mean, max, max+feather of the "select by" channel.
  - Hold **ctrl while dragging** for a raised curve, **shift** for a lowered one, nothing for a flat one.

**Hidden state and gotchas:**
- The modifier is read at apply time (`dt_key_modifier_state()` inside `color_picker_apply`), so it must be held while the sample is computed, i.e. during the drag. [code]
- The manual says the area picker draws a "median" line. The code computes the **mean**: `picked_color` plus min and max. [code — manual/code mismatch]
- Picker 2 is destructive (it resets the curve) with no confirmation.
- The edited channel (tab) and the selection channel ("select by") are two separate states the user must line up.

**Data needed:** the min, mean and max of the selection channel (L, C or h) over a region at module input. The output is a curve band.

**Statistic:** min/mean/max, area.

### 5. Tone equalizer

M:tone-equalizer/ — S:iop/toneequal.c

**Ritual:**
1. Expand the module and enable it. Scrolling over the image enables it if it is off. [code]
2. Open the **masking** tab and turn on "display exposure mask".
3. Click the **wand** on "mask exposure compensation". It centres the mask histogram on −4 EV (from the mask's first and last deciles).
4. Click the wand on "mask contrast compensation". This spreads the histogram.
5. Go back to the simple or advanced tab.
6. **Scroll over the image.** The node band matching the mask exposure under the cursor is raised or lowered.
   - Steps: shift+scroll = 1 EV (coarse), ctrl+scroll = 0.1 EV (fine), plain scroll = 0.25 EV. [code; the hinter text shows these, the manual is silent]
   - Hold **'a'** to pan or zoom the image instead.
7. Redo steps 2–4 whenever exposure (or anything upstream) changes.

**Hidden state and gotchas:**
- There is **no colour picker**. The "sample" is the cursor position: the mask value under the pointer, read from the module's own preview buffer.
- Scroll only works when the module **has focus**, buffers are valid, the pipe is idle and no mask editing is in progress. Otherwise the scroll is swallowed or falls through to zoom. [code]
- A wand on a *disabled* module only enables it and returns. The user has to click again. [code]
- If the preview is still computing, the wand logs "wait for the preview to finish recomputing" and does nothing. [code]
- Issue https://github.com/darktable-org/darktable/issues/17287: the mask-tuning loop (tab → mask display → wands → tab back) is repeated on nearly every image. The user asks to merge the tabs and to replace the exposure/contrast pair with black/white handles.

**Data needed:**
- The guided-filter luminance mask histogram, whole image. It is used for auto exposure/contrast compensation.
- A cursor position, mapped to mask EV, plus a signed step. The output is a Gaussian bump on the node curve centred at that EV.

**Statistic:** wands use deciles of the whole-image mask; scroll uses the point mask value.

### 6. Color balance rgb

M:color-balance-rgb/ — S:iop/colorbalancergb.c

**Ritual A (4 ways tab):**
- Each hue slider (global, shadows, mid-tones, highlights) has a quad picker (`AREA`).
- Drag an area. Hue is set to the **opponent** hue of the area (+180°), so as to neutralise a cast, and chroma is set from the area.
- Hold **ctrl** to take the area's own hue instead.

**Ritual B (masks tab):**
- White fulcrum picker: takes the **max** luminance of the area, in log2.
- Contrast grey fulcrum picker: takes the **mean** luminance.
- Set exposure first. "You should at least set the white fulcrum."

**Hidden state and gotchas:**
- The manual says "Ctrl+clicking on the picker". In code, ctrl is read in `color_picker_apply`, i.e. when the sample arrives.
- Because these pickers are `AREA`-only, ctrl+click on the icon only activates the picker. The ctrl that counts is the one held when the result is applied (during the canvas drag). [code]
- [guess] ctrl+click on the icon alone may not give the "current colour" result if ctrl is released before the preview finishes.
- Elsewhere, ctrl+click on a picker icon means "area mode". Here it means "same colour, not opponent".
- Each pick writes both H and C, overwriting a chroma the user may have tuned.
- Pickers read module input; the mask previews show output.

**Data needed:**
- A: the mean RGB of a region at input, as Ych hue and chroma.
- B: the max luminance (white fulcrum) and the mean luminance (grey fulcrum) of a region.

**Statistic:** mean (hues, grey), max (white); area.

### 7. Color equalizer

M:color-equalizer/ — S:iop/colorequal.c

**Ritual:**
- Choose the tab: hue, saturation or brightness.
- Then either:
  - Edit the graph: drag or scroll nodes (ctrl = slower, shift = faster), middle-click toggles the per-node sliders, alt+scroll on the graph changes tab, double-click resets.
  - Or **scroll over the image**. The band at the hue under the cursor is changed for the current tab.
    - ctrl+scroll gives a 10x finer step. **alt+scroll over the image cycles the tab.** [code; added by PR #21397]
    - 'a' + scroll/drag pans or zooms instead.

**Pickers:**
- "node placement" (`hue_shift`) quad picker: `POINT_AREA`, `DENOISE`, JzCzhz, keep-active.
  - Despite sitting on the "node placement" slider, it only **visualises** the picked hue on the graph.
  - Tooltip: "pick hue from image and visualize it, ctrl+click to select an area".
- "white level" picker (`AREA`): sets the white level from the **max** of the area.

**Hidden state and gotchas:**
- The canvas scroll is ignored while mask editing is active. It needs a preview buffer computed by the module; if the buffer is missing, a reprocess is triggered and the scroll falls through. [code]
- The hue under the cursor is read from the module's own preview buffer, i.e. module input in UCS hue. [code]
- A picker attached to a slider that does **not** set that slider is unique to this module.

**Data needed:**
- A cursor position mapped to a hue, plus a signed step for the current channel.
- Optionally the hue distribution of a region (display only).
- The max luminance of a region (white level).

**Statistic:** point hue for scroll; mean/min/max hue for the display; max for white level.

### 8. Retouch

M:retouch/ — S:iop/retouch.c, S:develop/masks/*.c

**Ritual:**
1. Choose the **algorithm** first: clone, heal, fill or blur.
   - ctrl+click an algorithm button = change the algorithm of the *selected* shape.
   - shift+click = make it the default.
2. Optionally set up wavelet scales:
   - Drag the bottom triangle of the scale bar to add scales; there is no decomposition by default.
   - Click a scale square to make it current. Shapes are added to the **current scale**.
3. Click a **shape** icon: circle, ellipse, path or brush. ctrl+click the icon = continuous creation; right-click on the image ends it.
4. For clone and heal, the **source** is placed before the target:
   - Hovering shows "+" at the default source.
   - **shift+click** = relative source (an offset kept for later shapes).
   - **ctrl+shift+click** = absolute source (a fixed image position).
   - Then click the target. Circle and ellipse also allow click-drag from target to source.
5. Fill: choose the mode (erase or colour) and set the fill colour (colour dialog, RGB, or picker).

**Shape gestures (shared with drawn masks, see entry 26):**
- Scroll = size, shift+scroll = feather, ctrl+scroll = opacity.
- ctrl+drag inside = rotate the target only; ctrl+shift+drag = rotate source and target together. [code: mouse_actions]
- Right-click = delete the shape.

**Hidden state and gotchas:**
- The fill-colour picker is `POINT | IO` and takes the **output** colour (`picked_output_color`). It guards against an infinite loop when the picked colour equals the current one. [code]
- Most pickers read input; this one reads output, so it samples the retouched result. [code]
- The relative/absolute source mode is a hidden, sticky GUI state (`source_pos_type`). It carries over to later shapes "unless the target location is changed first".
- Shapes belong to one scale. They are invisible or uneditable from other scales unless "show and edit shapes" is set. Cut/paste moves them between scales.
- The uncropped image is shown while the module has focus.

**Data needed:** per shape, a target geometry (circle, ellipse, path or brush), a source offset (clone/heal), an algorithm, a scale index, and a fill colour or blur radius.

**Statistic:** point, output.

### 9. Liquify

M:liquify/ — S:iop/liquify.c

**Ritual:**
- Choose a tool (point, line or curve). ctrl+click the icon for repeated placement.
- Point: click and drag; the drag sets the strength vector.
- Line or curve: click nodes, then right-click after the last one to finish. A second right-click exits continuous mode.

**Before or while placing a shape:**
- scroll = size
- **shift+scroll = strength**
- **ctrl+scroll = direction**

**Editing:**
- Node-edit tool: its icon, or right-click the image.
- Drag the centre to move. Drag the radius handle, the two hardness handles, or the strength arrowhead.
- ctrl+click the arrowhead cycles linear → grow → shrink.
- Click the centre to toggle the feathering controls.
- ctrl+click a node cycles autosmooth → cusp → smooth → symmetrical.
- ctrl+click a segment adds a node. ctrl+alt+click a segment toggles line/curve.
- Right-click a node or the centre removes it; right-click a path removes the path.
- ctrl+right-click a centre removes a node (manual).

**Hidden state and gotchas:**
- Limit of 100 nodes per instance.
- The uncropped image is shown.
- **The scroll modifiers mean something different from drawn masks.** See section 3.

**Data needed:** a list of warps. Each warp is a polyline or Bézier of nodes, each node with a centre, radius, two hardness values, a strength vector and a mode.

**Statistic:** none.

### 10. Crop

M:crop/ — S:iop/crop.c

**Ritual:**
- Expand or focus the module. The full uncropped image is shown.
- Drag the edges or corners to resize; shift+drag keeps the ratio. Drag inside to move.
- The hinter shows the modifiers: "move vertically: shift+drag", and ctrl for horizontal (per the manual).
- Choose the aspect in the combobox, or type "x:y".
- **Commit by giving focus to another module.** The parameters are written in `gui_focus(FALSE)` via `_commit_box`. [code]

**Hidden state and gotchas:**
- Nothing is visibly "applied" until focus leaves the module. The canvas shows a preview box. [code]
- Shift means "keep ratio" on edges but "constrain axis" when moving.
- Do rotation and perspective first.

**Data needed:** a rectangle in normalised image coordinates (after upstream distortions), plus an optional aspect constraint.

**Statistic:** none.

### 11. Rotate and perspective (ashift)

M:rotate-perspective/ (the slug `rotate-and-perspective` 404s) — S:iop/ashift.c

**Ritual A: horizon.**
- **right-drag** on the image draws a line that should be horizontal or vertical, and the rotation is set from it.
- This works even when the module is collapsed, unless another tool owns right-click (e.g. drawn-mask creation).

**Ritual B: perspective.** Expand the collapsed section, then choose a structure method:
- **Auto** ("get structure"): line detection. ctrl+click = edge enhancement, shift+click = detail enhancement, both = both.
  - Then curate the lines: click selects, right-click deselects, sweeping with the button held toggles many.
  - The wheel sets the sweep size; shift+drag selects in a rectangle and shift+right-drag deselects.
- **Rectangle**: drag 4 corners onto edges that should be vertical or horizontal.
- **Lines**: click-drag to draw lines. They are classified vertical (green) or horizontal (blue) automatically. Drag to edit; right-click to delete.

Then **click a fit button**: vertical, horizontal or both.
- ctrl+click = rotation only; shift+click = lens shift only; ctrl+shift+click (both) = rotation plus lens shift.
- Nothing is applied until a fit is clicked, and the fit must be clicked again after any structure edit.

**Hidden state and gotchas:**
- The fit needs at least `MINIMUM_FITLINES` lines **in each relevant direction**; otherwise it logs "not enough structure…".
- The fit can fail with "automatic correction failed, please correct manually". [code]
- Auto detection is random from run to run.
- Only `MAX_SAVED_LINES` drawn lines are saved to params. [code]
- Right-drag means "horizon" here, while elsewhere right-click means delete or cancel.
- Modifiers on *buttons* (fit, structure) change the algorithm. That is a pattern found nowhere else except retouch's algorithm buttons.

**Data needed:**
- A: one line segment (two points).
- B: a set of line segments, each tagged vertical or horizontal (≥ N per direction), or 4 corner points. Plus the choice of which parameters to fit (rotation, lens shift v/h, shear).

**Statistic:** none.

### 12. Lens correction

M:lens-correction/ — S:iop/lens.cc

**Ritual:**
- Choose the method first: Lensfun, embedded metadata, or only manual vignette.
- The controls shown depend on it; some are hidden behind "fine-tuning" or the "manual vignetting correction" buttons.
- The TCA override checkbox reveals the R/B sliders.

**Hidden state and gotchas:**
- Lens identification is read at import.
- "Use latest algorithm" is irreversible.
- Borders are filled by repeating edge pixels.

**Data needed:** EXIF or lens metadata (camera, lens, focal length, aperture, distance), or a profile choice. No image interaction.

**Statistic:** none. There are no pickers and no canvas gestures [code].

### 13. Vignetting

M:vignetting/ — S:iop/vignette.c

**Ritual:**
- With the module focused, an overlay shows the inner (start) and outer (fall-off) ellipses.
- Drag a node to change the size or feather; ctrl+drag keeps the ratio.
- Drag the centre to move it.
- The mouse_actions list says "ctrl+drag on center = move vignette". The code's centre move appears to be a plain drag. [code; possible mislabel, guess]
- The manual does not document the gestures.

**Data needed:** a centre (x, y), two radii (start, fall-off), a ratio or "auto ratio", and brightness/saturation.

**Statistic:** none.

### 14. Haze removal

M:haze-removal/ — S:iop/hazeremoval.c

**Ritual:** two sliders, strength and distance. No pickers and no gestures.

**Data needed:** none from the user beyond the two scalars. The haze estimate is internal, from the whole image [code: dark-channel prior].

### 15. Highlight reconstruction

M:highlight-reconstruction/ — S:iop/highlights.c

**Ritual:**
- Choose the method first.
- Turn on the visual toggles: clipping mask, segment outlines, candidate overlay.
- Then tune the threshold "until highlights stop being white, then lower slowly".

**Data needed:** none from the image interactively. The clipping threshold is relative to the white point. Visual feedback is via overlay toggles. The clipping mask is exported as a raster mask.

**Statistic:** none.

### 16. Denoise (profiled)

M:denoise-profiled/ — S:iop/denoiseprofile.c

**Ritual:**
- The profile is chosen automatically from Exif ISO.
- Choose the mode (NLM / wavelets, auto / advanced).
- Use wavelet curves (graph widget): left = coarse, right = fine.

**Gotchas:** the per-RGB-channel ritual needs an extra color calibration instance per channel ("time-consuming").

**Data needed:** a noise profile (camera + ISO), with no image interaction.

**Statistic:** none.

### 17. Filmic rgb

M:filmic-rgb/ — S:iop/filmicrgb.c

**Ritual:**
- Prerequisites: set exposure for the mid-tones first, then neutral WB, then denoise.
- Scene tab:
  - **White** relative exposure picker: max of the area → EV above grey.
  - **Black** relative exposure picker: min of the area → EV below grey, times the safety factor.
  - **Auto-tune** picker: both, plus grey if "custom middle-grey" is on.
  - A **grey** picker exists when "use custom middle-gray values" is ticked: mean → grey.
- All of these are `AREA | DENOISE`.

**Hidden state and gotchas:**
- Picking grey also shifts black and white by the same EV, so as to keep the range. [code]
- Black reads −16 EV when the measurement fails, and must then be set by hand.
- Auto-tune "usually fails for portraits and indoor scenes".
- The norm for grey depends on the `preserve_color` method; black and white always use max RGB. [code]
- Values are one-shot: redo the picks after an exposure change.

**Data needed:**
- The min and max of an RGB norm over a region at module input (denoised), optionally the mean for grey.
- The dynamic range scaling margin.

**Statistic:** min (black), max (white), mean (grey); area.

### 18. Sigmoid

M:sigmoid/ — S:iop/sigmoid.c

**Ritual:**
- Choose "color processing" first (per channel / rgb ratio); the primaries section only works per channel.
- Set exposure first.
- No pickers and no gestures.

**Data needed:** scalars only. It relies on the scene being exposed so that middle grey sits at 18%.

**Statistic:** none.

### 19. AgX

M:agx/ — S:iop/agx.c

**Ritual (documented order):**
1. exposure
2. preset
3. **auto tune levels** picker
4. **pivot relative exposure** picker (optional)
5. **pivot target output** picker (optional)
6. contrast
7. toe/shoulder
8. look

**Pickers (all `AREA | DENOISE`):**
- White relative exposure: max of the max channel.
- Black relative exposure: min of the min channel.
- Auto tune: both.
- Pivot x: mean luminance → pivot input.
- Pivot x/y: mean luminance → pivot input and output, "preserve the average brightness of the selected region".
- A camera "read exposure" button (no picker): reads black and white from the first enabled, unmasked exposure instance.

**Hidden state and gotchas:**
- The curve plot is "not interactive"; the pivot moves only via the pickers or sliders.
- The curve pivot pickers depend on the black/white range already set. Pick levels first. [code: uses `range_black_relative_ev`]
- Presets have hidden couplings (e.g. rotation reversal is inert until master reversal > 0).

**Data needed:** the min and max channel values of a region (levels); the mean luminance of a region (pivot).

**Statistic:** min, max, mean; area.

### 20. RGB levels

M:rgb-levels/ — S:iop/rgblevels.c

**Ritual:**
- Choose the mode (linked / independent); in independent mode choose the channel tab.
- Drag the 3 handles on the histogram.
- Or use the **black / grey / white pickers** (`POINT` only).
- Or click **auto** (whole image), or the **"auto region" toggle**: drag a rectangle on the image, right-click to cancel.

**Hidden state and gotchas:**
- **"Auto region" looks like a picker (pipette icon) but is not one.**
  - It is a plain toggle with its own `button_pressed` / `button_released` canvas code.
  - Its gestures differ: drag, right-click to cancel. It does not take part in the one-picker-at-a-time rule; the module turns it off by hand. [code]
- The point pickers use `*picked_color`, i.e. the **first component only**, as the level value. In linked mode the manual implies luminance. [code; guess about the effect on independent channels]
- A pick is rejected if it would cross another handle (black > grey, etc.). [code]

**Data needed:**
- Three scalars per channel: black, grey, white.
- A point sample for each picker, or region (or whole-image) statistics for auto.

**Statistic:** point value; auto uses a histogram over the region.

### 21. RGB curve / tone curve

M:rgb-curve/ and M:tone-curve/ — S:iop/rgbcurve.c and S:iop/tonecurve.c

**Ritual:**
- Choose the mode or colour space first (rgb curve: linked / independent; tone curve: Lab linked / separated, XYZ, RGB), then the channel tab.
- **Show-colour picker** (`POINT_AREA | IO`):
  - Click for point, ctrl+click or right-click for area.
  - It marks input and output on the graph and prints the numbers.
  - In rgb curve it is keep-active.
- **rgb curve only**, "create curve" picker (`AREA | IO`):
  - Drag an area: nodes are added at min, mean and max.
  - **ctrl held while dragging** = positive curve, **shift** = negative.
  - The modifier is read at apply time. [code]
- Graph gestures:
  - Click adds a node. Ctrl+click adds a node on the curve.
  - Drag a node out of the widget to delete it. Double-click resets.
  - Scroll over a node moves it. [code/manual curves page]

**Hidden state and gotchas:**
- Tone curve has **no** create-curve picker; rgb curve and colour zones do. [code]
- The tone curve graph is always shown in Lab.

**Data needed:** min/mean/max of the curve channel over a region, at input and output.

**Statistic:** min/mean/max; point or area.

### 22. Graduated density

M:graduated-density/ — S:iop/graduatednd.c

**Ritual:**
- With the module focused, an on-canvas line appears.
- Drag the line to move it; drag its end nodes to rotate.
- **ctrl+scroll on the line = density**; **shift+scroll = hardness**. [code: mouse_actions]
- The hue slider has a `POINT` quad picker that sets the filter hue from a pixel. [code]

**Gotchas:**
- The scroll modifiers (density/hardness) contradict the drawn-mask gradient on the same canvas (scroll = curvature, shift = compression, ctrl = opacity).
- The manual documents neither the scroll gestures nor the hue picker.

**Data needed:** a line (centre, angle), density in EV, hardness, and an optional hue from a point.

**Statistic:** point.

### 23. Negadoctor

M:negadoctor/ — S:iop/negadoctor.c

**Ritual:**
- First set the film stock (colour / B&W).
- Then go through the tabs top to bottom: film properties → corrections → print properties.
- Pickers (all `AREA`), **in order**:
  1. **Film base colour** (D min): mean over unexposed base only. The default box covers ~98% and must be dragged in.
  2. **D max**: min over the exposed image only (no base).
  3. **Scan exposure bias** (offset): max.
  4. **Shadows colour cast**: mean over a neutral shadow. Do it *before* step 5.
  5. **Highlights white balance**: mean over a neutral highlight.
  6. **Paper black**: max over the exposed image.
  7. **Print exposure**: min over the brightest highlights.

**Hidden state and gotchas:**
- **Every picker's formula uses the values set by earlier pickers** (`Dmin`, `D_max`, `offset`, `wb_low`), so the order is enforced only by the maths, not by the UI. Re-picking an early one silently invalidates the later ones. [code]
- Areas must exclude dust and borders.
- If the base is not in the frame, measure a blank-base shot and copy the setting across images.
- The working profile and downstream modules must be configured (no filmic or base curve, tone equalizer after).

**Data needed:** seven region statistics, each one a function of the earlier ones (a chain).

**Statistic:** mean, min and max per picker as listed; area.

### 24. Color look up table (colorchecker)

M:color-look-up-table/ — S:iop/colorchecker.c

**Ritual:**
- Pick a colour with the picker (`POINT_AREA`, Lab).
- The nearest patch is auto-selected in the patch combo. [code: nearest by ΔE to the picked mean]
- On the patch board:
  - Click a patch to select it.
  - **shift+click a patch** = replace its source with the picked colour.
  - **shift+click empty space** = add a patch.
  - Double-click = reset the patch. Right-click = delete it.
- Edit the target with the L/a/b/C sliders.

**Gotcha:** the picked colour is consumed by a *later* gesture on another widget (shift+click on the board), so you must pick before you shift+click.

**Data needed:** a set of (source Lab, target Lab) pairs. The source comes from a region mean.

**Statistic:** mean; point or area.

### 25. Color mapping

M:color-mapping/ — S:iop/colormapping.c

**Ritual:**
1. Open the source image and click "acquire as source".
2. Open the target image and click "acquire as target".
- The mapping applies only when both cluster sets exist.
- Changing the number of clusters wipes both, so acquire both again.

**Hidden state and gotchas:**
- The source clusters are stored in params or GUI state *across images*.
- This is the only cross-image ritual besides the mapping modes in color calibration and exposure.
- Acquisition is random and freezes the GUI for a few seconds.

**Data needed:** Lab cluster statistics (means and variances) of a whole reference image and of the target image. There is no region selection.

**Statistic:** k-means clusters, whole image.

### 26. Channel mixer rgb / monochrome

**Channel mixer rgb:**
- `M:channel-mixer-rgb/` 404s. Its successor is color calibration (entry 1).
- The deprecated "channel mixer" (M:channel-mixer/) has no pickers.
- The color calibration R/G/B/colorfulness/brightness/grey tabs are sliders only, with no pickers.

**Monochrome:** M:monochrome/ — S:iop/monochrome.c
- **Ritual:**
  - Drag the filter circle on the module's a/b colour-plane graph; scroll there changes the filter size.
  - Or use the picker (`AREA`, on the "highlights" slider): it sets the filter centre to the mean a/b and its size from the max−min a/b spread. [code]
- **Data needed:** a Lab a/b centre and a size.
- **Statistic:** mean plus range; area.

### 27. Drawn masks (blend section, all modules) and AI object mask

https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/drawn/ — S:develop/masks/{circle,ellipse,path,brush,gradient,object,masks}.c, S:develop/blend_gui.c

**Common ritual:**
- Expand the module, then choose blending mode "drawn" or "drawn & parametric".
- Click a shape icon. ctrl+click = continuous creation; right-click on the image ends it.
- After creation you are in edit mode. The "show and edit" icon re-enters it; ctrl+click it = restricted mode (no move or resize).
- Hold 'a' to pan or zoom instead of acting on the shape.

**Per shape:**

| Shape | Create | Scroll | Shift+scroll | Ctrl+scroll | Other |
|---|---|---|---|---|---|
| circle | click | size (over border: feather) | feather | opacity | — |
| ellipse | click | size | feather | opacity | ctrl+shift+scroll rotate; shift+click (manual: alt+click) toggle feather mode; drag nodes for eccentricity |
| path | click nodes (ctrl+click = sharp), **right-click to finish**, ≥3 nodes | grow/shrink | feather | opacity | ctrl+shift+scroll resize; ctrl+click node smooth/sharp; shift+click segment add node; right-click node delete; right-click handle reset |
| brush | drag | size | **hardness** | opacity | keys `{ }` hardness, `< >` opacity; pressure from tablet |
| gradient | click or click-drag (angle) | **curvature** | **compression** | opacity | drag pivot rotates; ctrl+drag rotates |
| object (AI) | click = foreground point, shift+click = background point, ctrl+shift+click = clear | smoothing | cleanup | opacity | **right-click = apply** (turns it into paths); needs an AI model; first click encodes the image |

- All shapes: right-click on a shape = delete it [code: masks.c].
- Clone shapes (retouch only): ctrl+drag rotates the shape or source; ctrl+shift+drag rotates both.

**Hidden state and gotchas:**
- Scrolling during *creation* also changes the defaults for the next shapes (stored per shape type in darktablerc: `DT_MASKS_CONF`).
- Scroll direction can be inverted by a preference.
- The ellipse feather toggle is shift+click in code mouse_actions and alt+click in the manual. [code/manual mismatch]
- Distorting modules turn circles into ellipses.
- The AI object cache is invalidated by crop or distortion changes.

**Data needed:** a group of shapes. Each shape is a geometry in image coordinates before distortion, plus a feather/hardness, an opacity, a combine mode (union, intersection…) and an inversion.

### 28. Parametric masks

https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/parametric/ — S:develop/blend_gui.c

**Ritual:**
- Choose blend mode "parametric".
- Choose the **channel tab**. The tabs depend on the module's blend colour space: Lab (L a b C h), display RGB (g R G B H S L) or scene RGB (g R G B Jz Cz hz).
- Drag the 4 markers on the input slider and/or the output slider. The output sliders are hidden until "show output channels" is enabled in the blend menu.
- Polarity button per slider.

**Pickers:**
- **Show colour** (`POINT_AREA | IO`, keep-active): click for point, ctrl+click or right-click for area. It shows values on all sliders.
- **Set range** (`AREA | IO`): drag an area and the **current tab's** input slider is set to [min, max] ± 1% feather.
  - **ctrl held while dragging** writes the *output* slider instead, but only if output channels are shown. Otherwise ctrl is silently ignored.
  - Hue wraparound is handled (uses an alternate hue range). [code]

**Hover keys (pointer over the slider):**
- `c` = show the channel.
- `m` = show the mask.
- `a` = toggle linear/log.

**Showmask button:** ctrl+click = mask, shift+click = channel.

**Gotchas:**
- Set range touches **one channel only**. Building a mask from a region means repeating it per tab.
- Issue https://github.com/darktable-org/darktable/issues/2047 asks for "pick to build mask" with union and subtraction.
- All channels multiply, so one channel at 0 zeroes the mask.
- The parametric invert also swaps the drawn & parametric combine mode.

**Data needed:** per channel, a trapezoid (4 values) on input and output. A picker gives min and max per channel over a region, at input or output.

**Statistic:** min/max (set range); mean/min/max shown.

### 29. Raster masks

https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/raster/

**Ritual:**
- Choose blend mode "raster mask", then choose the source in a combobox.
- The source module must be enabled and earlier in the pipe; disabling it removes the mask.

**Data needed:** a reference to another module instance's mask. There are no gestures.

### 30. Global color picker (utility)

https://docs.darktable.org/usermanual/development/en/module-reference/utility-modules/darkroom/global-color-picker/ — S:libs/colorpicker.c

**Ritual:**
- Click the picker for point; ctrl+click or right-click for area.
- Choose the statistic (mean/min/max) and the display space.
- "add" stores a live sample. Click its swatch to lock it.
- Right-click a sample to load it into the primary sample for editing; right-click again to save it.
- "restrict scope to selection" limits the histogram to the area, and also applies to module pickers.

**Hidden state and gotchas:**
- It samples **after the full pipe** in the histogram/display space, whereas module pickers sample module input or output in the module's space.
- It is turned off by any module focus change, unless "restrict" is on. [code]
- It shares the single-picker slot with module pickers.
- The user's chosen statistic does **not** affect module pickers: each module hard-codes mean, min or max. [code]

**Data needed:** none; it is a measurement tool.

**Statistic:** user-selectable mean/min/max; point or area.

---

## 2. Summary table: data needs by type

| Need type | Modules |
|---|---|
| Region mean colour (input) → solve for a parameter | color calibration (CAT, mapping), white balance, exposure, color balance rgb hues, negadoctor Dmin/WB, color LUT, monochrome |
| Region min / max of a norm | filmic, agx, color balance rgb white fulcrum, color equalizer white level, negadoctor Dmax/offset/black/exposure |
| Region min/mean/max of one channel → curve band | color zones, rgb curve, parametric set-range |
| Region sample shown only (no param write) | color zones, rgb/tone curve, color equalizer hue, parametric show-colour, global picker |
| Point value | rgb levels (×3), retouch fill (output), graduated density hue |
| Cursor-position probe + scroll delta | tone equalizer, color equalizer |
| Whole-image statistic via button (wand / auto) | tone equalizer wands, rgb levels auto, color mapping acquire, (compiled-out) CAT AI detect |
| Region for auto stats, outside the picker system | rgb levels auto region |
| On-canvas geometry | crop (rect), ashift (lines, quad, horizon line), vignette (centre + 2 radii), graduated density (line), color calibration checker (quad), retouch/drawn masks (shapes + source offset), liquify (warp paths with vectors) |
| Cross-image reference | color calibration and exposure mapping (sticky conf target), color mapping (source clusters) |
| Ordered chain of samples | negadoctor (7), agx (levels then pivot), filmic (grey shifts black/white), color checker (lens corr → chart → refresh → validate) |

---

## 3. Cross-module inconsistencies

**1. Ctrl on a picker icon has three meanings.**
- Generic: ctrl+click or right-click = **area mode** (POINT_AREA pickers: colour zones, curves, parametric, global).
- color balance rgb: ctrl = **take the picked hue instead of the opponent**.
- Modifiers held at **apply time** steer the result:
  - colour zones and rgb curve: ctrl = positive curve, shift = negative.
  - parametric set-range: ctrl = **output** slider.
- These apply-time modifiers are read when the asynchronous sample lands, not on the click. They work only while the key is held during the canvas drag. [code]

**2. Right-click has many meanings.**

| Context | Right-click means |
|---|---|
| picker icon | area mode |
| image, while a picker is active | reset the area to the whole image |
| drawn shape | delete it |
| path creation | finish the path |
| path node | delete the node |
| path handle | reset the curvature |
| AI object | **apply** |
| ashift | right-drag = horizon, right-click = deselect a segment |
| liquify | finish a line or curve, switch to node-edit, or delete |
| rgb levels auto region | cancel |
| colour LUT patch board | delete a patch |
| sliders | type-in popup |

**3. Scroll modifiers have different meanings.**

| Context | Scroll | Shift+scroll | Ctrl+scroll | Other |
|---|---|---|---|---|
| sliders | value | 10x coarse step | 10x fine step | — |
| tone equalizer canvas | 0.25 EV | 1 EV (coarse) | 0.1 EV (fine) | — |
| colour equalizer canvas | value | — | fine | **alt+scroll = change tab** |
| circle, ellipse, path masks | size | feather | opacity | — |
| brush | size | **hardness** | opacity | — |
| gradient | **curvature** | compression | opacity | — |
| liquify | size | **strength** | **direction** | — |
| graduated density | — | **hardness** | **density** | — |
| AI object | smoothing | cleanup | opacity | — |
| ashift auto mode | sweep brush size | — | — | — |

- The same physical canvas therefore changes meaning with the focused module.
- When the module has no handler, or its preconditions fail (no focus, pipe busy), scroll falls through to **image zoom**.

**4. The "pan instead" key is 'a'.**
- It is honoured by tone equalizer, colour equalizer, masks and retouch.
- In the parametric mask, 'a' over a slider means "linear/log toggle".

**5. Point vs area defaults are arbitrary per picker.**
- AREA-only: color calibration, white balance, exposure, filmic, agx, negadoctor, color balance rgb.
- POINT-only: rgb levels, retouch fill, graduated density hue, colorize, split-toning.
- POINT_AREA: colour zones show, curves show, colour equalizer, colour LUT, parametric show.
- The icon looks the same in every case; the ctrl/right-click switch is a no-op on single-mode pickers.

**6. Input vs output sampling is invisible.**
- Most pickers read module input.
- These also read output (`IO`): retouch fill (uses the output only), the curves, the parametric pickers.
- The global picker reads the final display output.
- Nothing in the UI says which one is used.

**7. The statistic is invisible and hard-coded.**
- The same rectangle gives the mean (exposure, WB, grey fulcrum), the max (white fulcrum, filmic white), the min (filmic black, negadoctor Dmax) or min/mean/max (zones, curves).
- The global picker's mean/min/max selector does not affect module pickers.
- The colour zones manual says "median"; the code uses the mean.

**8. Picker vs non-picker "pick" buttons.**
- rgb levels "auto region" uses a pipette icon but is its own toggle with its own canvas code. It does not obey the one-picker rule.
- The tone equalizer wands and filmic/agx "auto" sample implicitly (whole image or a picker area).
- The agx camera icon reads another module's params, not the image.
- Color mapping uses "acquire" buttons on the whole image.

**9. Display-only pickers vs writing pickers.**
- These look identical:
  - The colour equalizer quad on "node placement" only visualises.
  - The colour zones, curves and parametric "show colour" pickers only visualise.
  - Every other picker writes params.
- Some "keep-active" pickers survive slider edits and focus loss; others switch off at the first slider touch.

**10. Mode-dependent pickers.**
- The color calibration CAT picker and the exposure picker behave differently depending on an "area mode" combobox (measure vs correction) in another section.
- That combobox auto-resets on image change.
- The colour calibration CAT picker also maps toward a sticky, possibly non-neutral target (issue #18299).

**11. Commit semantics.**
- Crop commits on focus loss.
- ashift applies only after a "fit" click.
- Color mapping applies only after two acquisitions on two images.
- The colour checker applies after "validate".
- Everything else applies immediately on each sample or gesture.

**12. Modifiers on buttons change the algorithm.**
- ashift fit: ctrl = rotation only, shift = lens shift only.
- ashift structure: ctrl = edge enhancement, shift = detail enhancement.
- retouch algorithm: ctrl = apply to the selected shape, shift = set the default.
- Shape icons: ctrl = continuous creation.
- Show-and-edit: ctrl = restricted mode.
- Showmask: ctrl = mask, shift = channel.
- Module on/off: ctrl = focus.
- None of these is visible without the tooltip.

**13. Rotation gestures.**

| Context | Rotate with |
|---|---|
| ellipse | ctrl+shift+scroll |
| retouch and mask shapes | ctrl+drag |
| gradient | drag the pivot, or ctrl+drag |
| ashift | right-drag on the image (horizon) |
| graduated density | drag the end nodes |

**14. Ordering is implicit.** The manual states the order (e.g. negadoctor 1→7, agx levels then pivot, filmic after exposure, tone equalizer mask after exposure), but the UI never enforces or flags a stale earlier step.

**15. Manual vs code drift observed.**
- colour zones "median" (code: mean).
- Ellipse feather toggle: alt+click (manual) vs shift+click (code mouse_actions).
- color balance rgb "ctrl+click the picker": ctrl is read at apply time.
- Vignette centre move: plain drag in code, labelled ctrl+drag in mouse_actions.
- The colour equalizer on-canvas scroll and alt+scroll are in code; only plain scroll is in the manual.
- Tone equalizer shift/ctrl scroll steps are not in the manual.
- The AI WB detection is compiled out.
