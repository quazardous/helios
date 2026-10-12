# Ergonomics: power without the protocol

Status: research notes (2026-10-12), not decided. Scope: the *ergonomics* of
photo-editing tools, and the *access* to them: what a user must know, set
up, aim at and remember before a tool does its job. darktable, GIMP and
RawTherapee can do almost anything; the complaint studied here is that they
ask for exact parameters, a strict sequence of steps, modes set up first and
knowledge of their internals. Helios ports darktable's modules and wants
their power without that cost.

This note builds on [darktable-pitfalls/ui-platforms.md](darktable-pitfalls/ui-platforms.md)
and does not repeat it: the wheel over sliders (its pitfall 7), hover against
selection (9), preferences bloat (19), lowercase labels (15), focus theft
(5) and keymaps across layouts (6) are covered there. It also builds on
[ux-research.md](ux-research.md) (tools by intent, one grammar, AI proposals
stay editable), [shortcuts-research.md](shortcuts-research.md) (the key map),
[widgets.md](widgets.md) (values and descriptors) and [masks.md](masks.md).

Sources: darktable-org/darktable issues read with `gh` (2026-10-12), the
darktable user manual, darktable's source (labels quoted from
`src/iop/*.c`), discuss.pixls.us threads, the GIMP manual and its old
Bugzilla and GitLab trackers, Adobe, DxO, Capture One and Snapseed
documentation or tutorials, and HCI references. Statements marked
**[opinion]** are design judgement; **[guess]** marks inference not checked
in a source. A full source list is at the end; the module-by-module ritual
inventory is in [rituals.md](rituals.md).

**In short.**
1. Thirteen anti-patterns (section 1), each with a testable Helios rule.
2. One interaction grammar for every tool (section 2): Esc, undo per step,
   right-click is a menu, double-click resets, Space pans, one "show what
   this affects" quasimode, one fine and one coarse modifier, the same
   handles on every shape.
3. Seven signature interactions (section 3): point at the picture, intent
   masks, sample to range, hover previews, show-affected and A/B, auto
   then adjust, ask by intent.
4. The core proposal (section 4): modules stop implementing rituals and
   **declare typed interaction needs**; one layer runs them (one picker,
   visible chips, no hidden modes, profile-remappable gestures); custom
   rituals implement the same contract; later, rituals become data that
   an AI can compose and a ritual editor can edit.
5. Interaction profiles (section 5): Lightroom, darktable and Helios
   packages over a new `Profile` keymap layer and a new gesture map.
6. Twelve places where Helios's current plans risk repeating the problem
   (section 6), and a review checklist (section 7).

---

## 1. Engineer's ergonomics: a taxonomy

Each anti-pattern has: what it is, evidence, what praised tools do instead,
and a **Helios rule**, written to be testable where possible.

### E1. Parameters that are the model's math

- **What.** The control exposes the algorithm's variables instead of the
  photographic effect. The user must learn the model to predict the result.
- **Evidence.** filmic rgb's parameters, as declared in its source:
  "white relative exposure", "black relative exposure", "dynamic range
  scaling", "target black luminance" (default `0.01517634`), "hardness",
  "linear region", "preserve chrominance" with choices "max RGB",
  "luminance Y", "RGB power norm", "RGB euclidean norm (legacy)", and a
  "color science" choice "v3 (2019)" to "v7 (2023)"
  ([filmicrgb.c](https://github.com/darktable-org/darktable/blob/master/src/iop/filmicrgb.c)).
  The manual explains white relative exposure as "the stops between scene
  middle gray and the scene luminance mapped to display white"
  ([filmic rgb](https://docs.darktable.org/usermanual/4.6/en/module-reference/processing-modules/filmic-rgb/)),
  and users open "ELI5" threads to understand it
  ([pixls](https://discuss.pixls.us/t/eli5-black-and-white-levels-in-filmic/21757/24)).
  Tone equalizer's mask is tuned with "mask exposure compensation" and
  "mask contrast compensation"; a user asked for min/max points instead
  because "as a user, I am most interested in spreading the mask's values
  from black to white"; the answer was that the maths must not change
  ([#17287](https://github.com/darktable-org/darktable/issues/17287)).
  RawTherapee exposes CIECAM, wavelet levels and Retinex as peers of
  exposure; its beginners' thread says "too many buttons that appear to all
  have the same ranking in utility"
  ([pixls](https://discuss.pixls.us/t/can-we-make-rawtherapee-look-less-complex-on-very-first-sight-for-brand-new-users/14105)).
- **Praised tools.** Lightroom's Basic panel speaks of the picture
  (Highlights, Shadows, Whites, Blacks); the curve model behind it is
  hidden. Snapseed lists Brightness, Ambiance, Highlights, Warmth.
- **Helios rule.** Every parameter shown by default has a label in image
  terms and a one-line effect ("brightens the brightest parts"). Algorithm
  variants and version choices ("v7 (2023)", "RGB power norm") are never a
  default-level control: they live in an *advanced* level or are fixed by
  the module version. **Test:** a lint over module declarations fails when
  a default-level parameter has no label or tooltip, when its label equals
  its internal name, or when a choice list contains a version or an
  algorithm name without the `advanced` level.

### E2. A protocol before the result

- **What.** The tool works only after preparatory steps the software could
  have done itself, in a fixed order.
- **Evidence.** Lens correction was not applied to new images unless the
  user created an auto-applied preset; a core developer asked "can we
  expect a 'normal' user to know about this?"
  ([#13408](https://github.com/darktable-org/darktable/issues/13408)).
  Tone equalizer's mask wands must be clicked "when initially activating
  the module, whenever I change the mask, whenever I apply a preset,
  whenever the exposure of the image changes", and tuning the mask is a
  four-step loop across two tabs
  ([#17287](https://github.com/darktable-org/darktable/issues/17287)).
  filmic's pickers need a neutral white balance first: "first neutralize
  any color cast in the color calibration or white balance module"
  ([manual](https://docs.darktable.org/usermanual/4.6/en/module-reference/processing-modules/filmic-rgb/)).
  GIMP's foreground select needs a rough outline, then Enter, then strokes,
  then Enter again; "it is not obvious that you have to hit Enter" was
  answered with a status-bar message
  ([commit, 2018](https://mail.gnome.org/archives/commits-list/2018-May/msg04065.html),
  [Bugzilla 499690](https://bugzilla.gnome.org/show_bug.cgi?id=499690)).
- **Praised tools.** Lightroom applies lens profiles from a checkbox and
  enables masks from one click on "Select Sky"
  ([Adobe](https://www.adobe.com/products/photoshop-lightroom/masking.html)).
  Capture One's Magic Brush: "draw a line or a doodle on the area you want
  masked" ([Capture One support](https://support.captureone.com/hc/en-us/articles/4404012811537)).
- **Helios rule.** Anything the engine can compute from the image is
  computed and shown as a starting point the user adjusts ("auto, then
  adjust"). A derived value (a mask's spread, a white point) recomputes when
  its inputs change unless the user pinned it, and the UI shows which.
  **Test:** for each module, enabling it on the reference corpus yields a
  visible, non-destructive result with no further action.

### E3. Hidden modes and invisible state

- **What.** The same gesture does different things depending on a state
  the screen does not show; when the result is "nothing happens", the user
  is stuck.
- **Evidence.** GIMP's manual has a whole chapter, "Getting unstuck", whose
  causes are almost all invisible state: "There is a floating selection",
  "The selection is hidden", "You are acting outside the selection", "The
  active drawable is not visible", "You are trying to act outside the
  layer", "You are trying to act on a layer group", "The image is in
  indexed color mode"
  ([GIMP manual](https://docs.gimp.org/2.10/en/gimp-getting-unstuck.html)).
  Floating selections were called "one of the most confusing things for
  Gimp newbies" ([Bugzilla 113477](https://bugzilla.gnome.org/show_bug.cgi?id=113477),
  WONTFIX for years; GIMP 3.0 finally pastes as a new layer,
  [MR 769](https://gitlab.gnome.org/GNOME/gimp/-/merge_requests/769)).
  Painting a layer instead of its mask happens because "the active
  drawable is the layer and not the layer mask"
  ([gimp-forum](https://www.gimp-forum.net/Thread-SOOOOOOOOOOO-close-in-cleaning-up-an-image?pid=39108));
  Photoshop users hit the same trap, so this is not GIMP-specific.
  In darktable, color calibration's white balance picker gives "strange
  results if the user is not aware that he has set a target color different
  from neutral"; the "active" mark "can easily be overlooked"
  ([#18299](https://github.com/darktable-org/darktable/issues/18299)).
  GIMP's path tool has Design, Edit and Move modes reached by Ctrl and Alt;
  Move "flicks back" to Design
  ([discourse.gnome.org](https://discourse.gnome.org/t/paths-and-the-transient-move-option/19085)).
- **Praised tools.** Lightroom's masking panel always lists the masks and
  shows which one is being edited, with an overlay (`O`).
- **Helios rule.** No sticky mode without a visible indicator *on the
  canvas* (not only in a panel) and an Esc exit. Prefer quasimodes (hold a
  key: the mode ends when the key is released, Raskin's "spring-loaded"
  modes, see [mode](https://en.wikipedia.org/wiki/Mode_(user_interface)))
  and tool sessions that end by themselves. When a gesture has no effect,
  say why at the cursor ("outside the layer: extend?"). **Test:** the UI
  state is data (`ui.mode`); a scenario test fails if a gesture produces no
  change and no message.

### E4. Modifier soup

- **What.** Functions are reachable only through modifier + click or
  modifier + wheel combinations, different in each tool, with no visible
  equivalent.
- **Evidence.** darktable's drawn masks: Ctrl+click a shape icon to keep
  drawing, right-click to stop; Shift+scroll feather, Ctrl+scroll opacity;
  Ctrl+click and drag to rotate; Shift+Ctrl+scroll to rotate an ellipse;
  Alt+click to toggle decay; Ctrl+click a node for a sharp corner;
  Shift+click a segment to add a node; right-click a node to delete it;
  hold `a` to pan instead of acting on the shape
  ([manual, drawn masks](https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/drawn/)).
  A pixls user cites "all those different keyboard modifiers for mouse
  clicks" and "inconsistency in using left and right mouse clicks" as
  reasons to turn away ([pixls](https://discuss.pixls.us/t/darktable-ui-work/56268)).
  In GIMP the path tool's modifiers were not even shown in its options
  ([Bugzilla 415796](https://bugzilla.gnome.org/show_bug.cgi?id=415796)),
  and Alt is captured by some window managers so Move mode is unreachable
  ([discourse.gnome.org](https://discourse.gnome.org/t/paths-and-the-transient-move-option/19085)).
- **Praised tools.** Snapseed has two gestures for all its sliders: swipe
  vertically to choose the parameter, horizontally to change it
  ([tutorial](https://iphonephotographyschool.com/snapseed/)). Lightroom
  uses one modifier meaning across panels (Alt: show what this affects, see
  section 2).
- **Helios rule.** Every modifier gesture has a visible equivalent (a
  handle, an on-canvas button, a menu entry). A canvas gesture uses at most
  one modifier, and a modifier keeps one meaning everywhere (section 2).
  No function needs Alt alone on Linux. While a tool is active, a hint bar
  near the cursor lists its gestures. **Test:** every gesture a ritual
  declares (section 4) has a command; the command palette reaches it.

### E5. The module, not the intent

- **What.** Tools are organised by implementation; finding "the shadows
  slider" means knowing the module that implements it.
- **Evidence.** "the fact I have to search for my highlight/shadow
  adjustment is really annoying"; answer: "it doesn't help to search in
  darktable for a lightroom function"
  ([#19084](https://github.com/darktable-org/darktable/issues/19084)).
  A basic channel mixer is a tab inside color calibration, which is also
  white balance; "it takes 3 clicks for someone who already knows how to
  locate the required module"; "I simply have no clue where the modules are
  located in the pipeline"
  ([pixls](https://discuss.pixls.us/t/another-dt-ui-discussion/58799)).
  Several modules do the same job: three, then five, tone mappers, and a
  warning was proposed because users enable two by mistake
  ([#22237](https://github.com/darktable-org/darktable/issues/22237);
  [pixls](https://discuss.pixls.us/t/on-module-proliferation/56435)).
  Module group buttons "look like tabs but behave like filter toggles"
  ([pixls](https://discuss.pixls.us/t/darktable-ui-work/56268)).
  darktable does carry search aliases per module (tone equalizer:
  "tone curve|tone mapping|relight|background light|shadows highlights",
  [toneequal.c](https://github.com/darktable-org/darktable/blob/master/src/iop/toneequal.c)),
  a good base that only helps users who already think to search.
- **Praised tools.** Lightroom's fixed panel order follows the edit
  (Basic, Tone Curve, HSL); its targeted adjustment tool lets the user point
  at the picture instead of finding a slider
  ([Adobe](https://pages.adobe.com/creativecloud/en/photography-plan/lightroom-classic/hsl-target-adjustment)).
  Photoshop's contextual task bar offers "next steps right on the canvas"
  ([Adobe community](https://community.adobe.com/t5/photoshop-beta-discussions/contextual-task-bars-now-in-photoshop-beta/td-p/13658736/page/9)),
  though users complain it gets in the way.
- **Helios rule.** The palette and the module search accept intent words
  and other tools' names (a synonym table per module, including Lightroom
  and Capture One vocabulary), and the canvas answers "what can I do here"
  (section 3, S1). One recommended module per job (`Role::General`), the
  alternatives one step away. **Test:** a list of 50 intents ("brighten
  shadows", "remove haze", "straighten") each reaches a working control in
  at most 2 actions from the canvas.

### E6. Pipeline knowledge required

- **What.** The user must reason about processing order, colour spaces or
  which stage owns a job.
- **Evidence.** A Reddit poster finds no way to build a workflow in the
  order users expect because "the module order is tied to backend order"
  (summarised from r/DarkTable in a search; the darktable FAQ confirms
  "very little workflow and tool knowledge can be transferred from other
  programs", [FAQ](https://www.darktable.org/about/faq/)). Two tone mappers
  "stack", so the warning text says "Make sure you know what you are doing,
  their tone curves stack" ([#22237](https://github.com/darktable-org/darktable/issues/22237)).
  The term "tone mapper", used everywhere in discussion, "is not mentioned
  or explained in the user manual or darktable interface" (same issue).
  Parametric mask channels depend on the module's colour space (Lab, display
  RGB or scene RGB give different channel sets)
  ([manual](https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/parametric/)).
- **Praised tools.** Lightroom and Capture One have a fixed internal order
  the user never sees.
- **Helios rule.** The engine owns order; the stack view shows modules in
  an editing order, not a processing order, and conflicts (two tone
  mappers) are prevented by declaration (an exclusive role), not by a
  warning. Mask channels are offered in perceptual terms (lightness,
  colour, saturation) whatever the module's working space. **Test:** no
  default-level control mentions a colour space or a pipeline position.

### E7. Precision demanded of the hand

- **What.** Small handles, exact positions, values reached by
  overshoot-and-correct.
- **Evidence.** darktable's slider fine control is a right-click pop-up
  where "the closer your mouse pointer is to the triangular marker the
  coarser the control"; "clicking anywhere in a slider's area sets its
  value" ([manual](https://docs.darktable.org/usermanual/development/en/darkroom/processing-modules/module-controls/)).
  Capture sharpening could only be compared by moving a slider to zero and
  back, "cumbersome and prone to under/overshooting"
  ([#19211](https://github.com/darktable-org/darktable/issues/19211)).
  The module on/off icon is "a tad too small"
  ([pixls](https://discuss.pixls.us/t/darktable-ui-work/56268)). Drawn masks
  become hard to use after flip and rotation
  ([#3218](https://github.com/darktable-org/darktable/issues/3218)).
  Parametric masks take four triangle markers per channel
  ([manual](https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/parametric/)).
- **Praised tools.** Generous targets (WCAG 2.2 sets 24 by 24 CSS pixels as
  a minimum, [target size](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html));
  Fitts's law: larger and closer targets are faster
  ([NN/g](https://www.nngroup.com/articles/fitts-law/)).
- **Helios rule.** Hit targets of at least 24 logical px for every handle
  and toggle (the drawn handle may be smaller, its hit area is not); a
  click on a slider track never jumps the value (a drag is relative);
  every value has an exact entry; on/off and A/B have keys. **Test:** a
  headless check measures the hit rectangles of every interactive node
  from AccessKit.

### E8. Blind adjustment: no preview before commitment

- **What.** The user must commit to see, or switch views to judge.
- **Evidence.** Tone equalizer: "switch to the masking tab, turn on mask
  display, fiddle, switch back to the advanced tab and try"
  ([#17287](https://github.com/darktable-org/darktable/issues/17287)).
  The suggested way to compare capture sharpening was snapshots or undo
  and redo ([#19211](https://github.com/darktable-org/darktable/issues/19211)).
  Each module has its own way to show what it affects: a mask display
  button in blending, keys `c` and `m` over a parametric slider, a
  "display highlight reconstruction mask" in filmic, a "display exposure
  mask" in tone equalizer (darktable sources and manual).
- **Praised tools.** Lightroom previews a preset in the loupe on hover
  (since 7.2; a setting can limit it to the navigator,
  [Adobe community](https://community.adobe.com/t5/lightroom-classic-discussions/disable-full-image-presets-in-latest-update/m-p/9777804)),
  and Alt while dragging a slider shows the affected area (Point Color:
  "hold Alt/Option while dragging a slider to show the affected area",
  [PhotoshopCAFE](https://photoshopcafe.com/remove-color-reflections-in-lightroom-classic-acr-with-new-point-color/)).
- **Helios rule.** Choices preview on hover (presets, styles, choice
  parameters, masks to pick); every module and every mask has the same
  "show what this affects" quasimode and the same A/B key. **Test:** a
  scenario hovers each preset of a module and checks a proxy render is
  requested and dropped on leave, with no history entry.

### E9. Error-intolerant flows

- **What.** No undo inside a multi-step tool, resets that do not reset,
  a wrong step that costs the whole sequence.
- **Evidence.** GIMP's foreground select had no undo while refining
  ([Bugzilla 499690](https://bugzilla.gnome.org/show_bug.cgi?id=499690)).
  darktable's color calibration area mapping is saved to the configuration
  and applied to each new edit; "the module's configuration won't reset
  when 'module reset' is pressed" ([#18299](https://github.com/darktable-org/darktable/issues/18299)).
  GIMP's paste anchored itself while the user tried to move it "at least
  40% of the time" ([GIMP #1215](https://gitlab.gnome.org/GNOME/gimp/-/issues/1215)).
- **Helios rule.** Each step of a multi-step tool is one undo step; Esc
  leaves a tool with no trace; reset restores every value the module owns,
  remembered tool settings included, and says what it reset. Hidden
  carry-over from one image to the next is forbidden: carried values are
  shown as "from the last image" until confirmed. **Test:** for each
  ritual, start, step, Esc leaves the history and the values unchanged.

### E10. Private sub-UIs per tool

- **What.** Each tool brings its own editor for a shared concept (masks,
  curves, pickers), so knowledge does not transfer between tools.
- **Evidence.** Classic darktable masks: "Disjoint mental models apply to
  shapes, parametric and raster masks", "the hard to understand
  include/exclude and positive/negative polarity", "Raster masks cannot be
  combined with anything else", the mask editor "sits all the way across the
  screen", "why ... are there 12!!! items in my mask manager??"
  ([pixls, Flexi masks RFC](https://discuss.pixls.us/t/rfc-builds-available-masking-panel-revamp-and-new-masking-possibilities/59675);
  [#21905](https://github.com/darktable-org/darktable/issues/21905)).
  "In darktable every module has its own masks"
  ([pixls](https://discuss.pixls.us/t/darktable-tips-from-captureone-users-wanted/57386?page=2)).
  Pickers differ per module (section 4).
- **Praised tools.** Lightroom's single masking panel, with Add, Subtract,
  Intersect on any mask type
  ([Adobe](https://www.adobe.com/vn_en/learn/lightroom-cc/web/advanced-lightroom-masking)).
- **Helios rule.** One mask editor, one picker, one curve editor, one shape
  editor, provided by the framework; modules declare what they need
  (section 4). Masks are named objects reusable across modules.

### E11. Ranges set by hand when they could be sampled

- **What.** The user must translate what they see ("these berries") into
  numeric ranges on channels.
- **Evidence.** "If you want to create a parametric mask, you have to move
  the slider manually for one (or more) channels"; the request to build it
  from areas picked on the image (include, add, subtract) was judged a good
  idea that "needs some thinking" and closed
  ([#2047](https://github.com/darktable-org/darktable/issues/2047)).
- **Praised tools.** DxO's U Point "automatically analyzes the image
  luminosity, contrast, and color" around a control point
  ([DxO](https://www.dxo.com/dxo-photolab/u-point/)); RawTherapee's RT-spot
  takes its reference from the average inside a circle
  ([RawPedia](https://rawpedia.rawtherapee.com/Local_Adjustments));
  Lightroom's Point Color samples a colour, then offers range sliders.
- **Helios rule.** Every range-like parameter (a channel range, a hue
  band, a luminance zone) can be set by sampling the image (point, area,
  stroke), adding to or subtracting from the sample, then refined by hand.

### E12. The manual as a prerequisite

- **What.** Features are discoverable only by reading documentation or
  watching videos; vocabulary comes from the code.
- **Evidence.** "Who is darktable not for: someone who doesn't want to read
  the manual" ([RFC #22024](https://github.com/darktable-org/darktable/issues/22024)).
  The shortcut search was "far from obvious" and "an extensive manual entry
  will be required" ([#9378](https://github.com/darktable-org/darktable/issues/9378)).
  The histogram's draggable exposure was "discovered by accident"
  ([#2041](https://github.com/darktable-org/darktable/issues/2041)).
  A second click on a module group tab, "which most users may not discover",
  changes its content ([pixls](https://discuss.pixls.us/t/darktable-ui-work/56268)).
- **Praised tools.** Recognition rather than recall: show options instead
  of requiring memory ([NN/g](https://www.nngroup.com/articles/recognition-and-recall/)).
- **Helios rule.** Every function is reachable from a visible element or
  the palette; tooltips say what a control does to the image; a tool shows
  its gestures while active. **Test:** a list of all commands and declared
  gestures, each with a visible path (menu, button, handle, hint), checked
  in CI.

### E13. Configuration as the answer

- **What.** A reasonable request is answered with "make a preset", "set up
  the quick access panel", "add a preference".
- **Evidence.** A beginner asking for a simple panel was pointed to the
  quick access panel and the beginner workflow preset
  ([#19084](https://github.com/darktable-org/darktable/issues/19084)); lens
  correction needed a hand-made auto-applied preset
  ([#13408](https://github.com/darktable-org/darktable/issues/13408)); "the
  link to the auto-apply workflow setting is not obvious"
  ([pixls](https://discuss.pixls.us/t/darktable-ui-work/56268)). (The
  preferences side is ui-platforms pitfall 19.)
- **Praised tools.** RawTherapee lets the user pick a complexity level per
  local adjustment tool (Basic, Standard, Advanced)
  ([pixls](https://discuss.pixls.us/t/local-adjustments-is-in-dev/18795)):
  progressive disclosure ([NN/g](https://www.nngroup.com/articles/progressive-disclosure/)).
- **Helios rule.** The defaults are the product: a fresh install, on the
  reference corpus, gives a good result with no configuration. Progressive
  disclosure is declared per parameter (`level: basic | standard |
  advanced`), not built by each user.

[opinion] The common root of E1 to E13: darktable's UI is generated from
the implementation (one module, one panel, its parameters as sliders, its
private gestures), and darktable's culture treats learning it as the price
of power. Helios's descriptor-driven UI can reproduce the same thing
faster unless the declarations carry *intent*, not only types.

---

## 2. Consistent reflexes: one interaction grammar

The owner's requirement: the same gesture or key means the same thing in
every tool. Below, the reflexes worth unifying, where darktable and GIMP
contradict themselves, and a proposed grammar.

### Where the tools contradict themselves

| Reflex | darktable | GIMP |
|---|---|---|
| Right-click | on a slider: fine-tune pop-up and typed entry; on a shape: delete it; while creating shapes: stop creating; on a path node: delete the node; on module header icons: other functions ([manual](https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/drawn/)) | context menu almost everywhere; anchors a floating layer from the layers list ([Bugzilla 127451](https://bugzilla.gnome.org/show_bug.cgi?id=127451)) |
| Wheel | over a slider: value; over the image: zoom; over a shape: size; over a brush node: size; over a gradient's line: curvature; in tone equalizer: brighten the tone under the cursor ([manual](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/tone-equalizer/)) | scroll or zoom depending on preferences |
| Shift | Shift+scroll: feather (shapes) or border width (paths); Shift+click: insert a path node, or select only feathering nodes ([manual](https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/drawn/)); AI mask: Shift+click subtracts ([5.6 notes](https://github.com/darktable-org/darktable/releases/tag/release-5.6.0)) | add to selection; constrain lines; in paths, a different set again |
| Ctrl | Ctrl+scroll: opacity; Ctrl+click a shape icon: continuous creation; Ctrl+click "edit mask" icon: restricted edit; Ctrl+drag: rotate; Ctrl+click a node: sharp corner; Ctrl+click the parametric picker: set output range | path tool: switch to Edit mode; selection: subtract |
| Alt | ellipse decay toggle | path tool: Move mode, often eaten by the window manager |
| Show what is affected | blending's mask button, `m`/`c` over parametric sliders, a filmic "display mask" toggle, tone equalizer's mask display: four mechanisms | quick mask toggle; Alt+click a mask thumbnail (Photoshop habit) |
| Commit / cancel | crop applies "when the iop loses focus" ([crop.c](https://github.com/darktable-org/darktable/blob/master/src/iop/crop.c)); right-click stops shape creation | Enter commits foreground select, crop and transforms; Esc cancels |
| Reset | double-click a slider label resets; module reset button; a reset that does not reset area mapping ([#18299](https://github.com/darktable-org/darktable/issues/18299)) | per-tool "reset" icon in tool options |
| On/off vs visibility | the eye icon means "show the mask", so it was refused for "on/off" ([#19211](https://github.com/darktable-org/darktable/issues/19211)) | the eye means visibility of a layer |
| Pan while a tool is active | hold `a` ([manual](https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/drawn/)) | Space-drag |

The ritual inventory (section 4.1 and its companion file) extends this
table: right-click has about a dozen meanings in darktable's darkroom,
Ctrl on a picker three, and Shift+scroll and Ctrl+scroll five or six each
depending on the focused module.

Lightroom, by contrast, keeps a few reflexes across panels: Alt while
dragging shows clipping (Whites, Blacks) or the affected area (Point Color,
masks); double-click a slider label resets it; `[` and `]` size every brush;
`O` shows any mask overlay [from Adobe's shortcut reference and tutorials;
key details to recheck against
[Adobe's list](https://helpx.adobe.com/lightroom-classic/desktop/introduction-to-lightroom-classic/keyboard-shortcuts.html)].

### The Helios grammar (proposal, not decided)

Invariants: they hold in every tool and every profile, because breaking
them breaks other reflexes.

1. **Esc** leaves the current tool session or cancels the gesture in
   progress, with no trace. Enter (or a click on the commit chip) commits a
   session that needs a commit; most do not (a session commits each step).
2. **Undo** undoes the last step, including a step inside a tool session.
3. **Drag on a value** (slider, handle, point on the canvas) is relative
   and is one undo step; a plain click never jumps a value.
4. **Double-click** a control or a handle: reset it to its default (or
   the auto value). A double-click on the canvas never resets anything.
5. **Space held**: pan the canvas, whatever the tool. (darktable's `a`
   becomes an alias in the darktable profile.)
6. **Show what this affects**: one quasimode, the same in every module:
   hold a key (proposed `Alt` while dragging, and a dedicated key such as
   `M` held) to show the mask, the affected area or the clipping of the
   control under the pointer.
7. **A/B**: one key toggles the current module (or mask, or step) on and
   off while held; another compares to a chosen "before".
8. **Fine and coarse adjustment** have one modifier each, everywhere, for
   drags, wheel and arrows (proposed: Shift coarser, as gui.md already says
   for sliders and as darktable's sliders and tone equalizer do; Ctrl/Cmd
   finer). On the canvas, a modifier never changes *which* quantity is
   adjusted (darktable's Shift+scroll is feather on a circle, hardness on a
   brush, compression on a gradient, strength in liquify): the handle under
   the pointer decides that (rule 11). The keys are profile-dependent; the
   existence of fine and coarse steps is invariant.
9. **Add / subtract** in any selection or mask: one modifier pair for every
   mask type (proposed Shift adds, Alt subtracts, as in Photoshop and
   Lightroom brushes); plus visible Add/Subtract buttons (E4).
10. **Right-click** opens a context menu of commands, nothing else. Never
    delete, never commit, never a hidden pop-up editor. (Typed entry is a
    double-click on the value text or Enter on a focused control.)
11. **Handles**: every shape (circle, ellipse, path, gradient, crop box,
    perspective line) has the same handle kinds with the same looks and
    behaviour: move (body), size (edge), feather (outer ring), rotate
    (a satellite handle), opacity (a chip). A wheel over a handle adjusts
    that handle's quantity, never something else.
12. **Wheel**: over the canvas, zoom (or pan, per profile); over a handle,
    its quantity; over a focused control, its value; otherwise scroll the
    panel (ui-platforms pitfall 7).
13. **Hover informs, click acts** (ui-platforms pitfall 9), extended:
    hover previews choices (E8), never commits them.

Where it lives: egui-pop should own the grammar as data (the gesture
vocabulary of section 4.5 and a resolver), so every widget and every ritual
asks "what does this input mean here" instead of reading raw modifiers.
**Test:** a scenario suite plays each grammar rule against each ritual and
widget kind.

---

## 3. Signature interactions

Seven interactions Helios should have from its first editing release, each
with what darktable's engine already allows and what the UI must add.

**S1. Point at the picture (targeted adjustment).** Hover the image: a
small readout says what is under the cursor ("bright sky, blue"). Drag up
or down on it: the active module changes the parameter that governs that
place (the tone band in a tone equalizer, the hue band in a colour
equalizer, the curve point in a curve). Without an active module, the drag
offers the two or three most likely adjustments in a radial chip.
*Engine today:* tone equalizer already does this with the wheel (hover
shows mask EV, scroll brightens that band,
[manual](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/tone-equalizer/));
rgb curve, color zones and the colour pickers already map a sampled
colour to a curve position. *UI to add:* a declared "point-to-parameter"
need (section 4) so any module can say "this pixel's value maps to this
parameter", the same drag in every module, and a readout. Lightroom's
targeted adjustment tool is the reference.

**S2. Intent masks, editable.** "Sky", "subject", "background", "this
object", then Add/Subtract/Intersect with any other mask kind, in one mask
panel. *Engine today:* darktable 5.6 ships SAM2.1/SegNext object masks
with foreground and background points
([release notes](https://github.com/darktable-org/darktable/releases/tag/release-5.6.0));
Helios's mask nodes already combine members with operators
([masks.md](masks.md)), and decision nodes record an AI-assisted answer.
*UI to add:* masks as named objects, one panel, operators shown as Add,
Subtract, Intersect (the flexi operators "screen", "exclusion" stay
advanced), and a refine brush that works on any mask kind.

**S3. Sample to range.** Click or brush on the picture to define a
parametric mask, a colour band or a luminance zone; Shift adds a sample,
Alt subtracts; the ranges and feathering are then sliders (E11). *Engine
today:* `mask_parametric` and `mask_from_luma` take ranges; area statistics
are a framework read. *UI to add:* the shared sampler (section 4).

**S4. Hover previews.** Hovering a preset, a style, a choice value or a
proposed mask previews it on the canvas (proxy render), without a history
entry. *Engine today:* content-addressed renders and proxy plans make a
throwaway render cheap; the scheduler must give the preview a high
priority and cancel it on leave. *UI to add:* a "tentative value" state in
the value store that renders but is never recorded.

**S5. Show what this affects, and A/B.** One quasimode for every module
and mask (grammar 6 and 7). *Engine today:* every mask is a port that can
be displayed; a node can be bypassed. *UI to add:* a viewport overlay mode
fed by the module's mask port or by a difference image, and a "bypass while
held" command.

**S6. Auto, then adjust.** Enabling a module starts from an automatic
estimate (exposure, white point, black point, mask spread, perspective),
shown as such ("auto"), adjustable, and recomputed when its inputs change
until the user touches it (E2). *Engine today:* the auto-tune pickers
compute these from image statistics. *UI to add:* the "auto / pinned"
state per value, shown by the widget.

**S7. Ask by intent.** The palette understands "remove haze", "warmer",
"straighten horizon", "brighten face", proposing modules, presets, masks
and workshops, with Lightroom and Capture One names as synonyms. *Engine
today:* darktable's `aliases()` and Helios's `Family` and `Role` in the
manifest. *UI to add:* a synonym table per module and a ranking that
prefers `Role::General`.

---

## 4. Declared interaction needs

Remapping keys is not enough. Many darktable modules carry their own
*ritual*: the private sequence of pickers, modes, tabs and modifier
gestures by which the user gives the module the data it needs. The rituals
differ from module to module, so nothing learned in one transfers to the
next (E10), and each one carries hidden state (E3). The proposal studied
here [owner's proposal, not decided]: **modules no longer implement
rituals; they declare what they need from the user (typed interaction
needs), and one ergonomics layer runs the rituals the same way everywhere,
per profile.**

### 4.1 Inventory of darktable's rituals

Thirty modules and tools were read in darktable's source (`src/iop/*.c`,
`src/gui/color_picker_proxy.c`, `src/develop/masks/*.c`,
`src/develop/blend_gui.c`) and in the development manual. The full
inventory, with each ritual step by step, links and code references, is
the companion file [rituals.md](rituals.md). The
condensed view (M = manual page under
`docs.darktable.org/usermanual/development/en/module-reference/processing-modules/`;
[code] = read in the source, not in the manual):

| Module | Ritual (condensed) | Hidden state, gotchas | What it actually needs |
|---|---|---|---|
| color calibration, CAT ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/color-calibration/)) | set white balance module to "camera reference"; CAT tab; picker; drag over neutral area | same picker and code path as area colour mapping; maps to a target kept in the configuration, survives reset, may be non-neutral ([#18299](https://github.com/darktable-org/darktable/issues/18299)); "measure" vs "correction" set in another section, auto-resets per image [code] | mean RGB of a region at input + a target chromaticity (neutral) → illuminant |
| color calibration, checker | enable lens correction; expand section; choose chart; drag 4 corners; refresh; read ΔE; validate | overlay exists only while profiling [code] | a quad locating the chart → patch samples → matrix |
| white balance ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/white-balance/)) | pipette; draw rectangle on neutral | samples raw camera RGB [code]; competes with color calibration's picker | mean camera RGB of a neutral region → multipliers |
| exposure ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/exposure/)) | picker on the slider; draw area; or the two-step measure/correction mapping across images | same function for both modes [code]; target L kept in configuration, survives reset | mean luminance of a region + target lightness → EV |
| color zones ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/color-zones/)) | choose "select by" (resets curves); choose tab; "show" picker (display only) or "create curve" picker; hold Ctrl/Shift *during* the drag for raised/lowered | modifier read when the asynchronous sample arrives [code]; create-curve resets the curve without asking; manual says median, code uses mean | min/mean/max of a channel over a region → curve band |
| tone equalizer ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/tone-equalizer/)) | enable; masking tab; mask display; exposure wand; contrast wand; back to tab; scroll on image (Shift coarse, Ctrl fine); redo after upstream changes | scroll works only with focus, valid buffers, idle pipe [code]; a wand on a disabled module only enables it [code] | whole-image mask histogram (auto spread) + cursor → band, signed step |
| color balance rgb ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/color-balance-rgb/)) | picker per hue slider: drag area → *opponent* hue; Ctrl: same hue; fulcrum pickers (max, mean) | Ctrl here means "same hue", elsewhere "area mode"; a pick overwrites tuned chroma | mean hue/chroma of a region; max and mean luminance |
| color equalizer ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/color-equalizer/)) | choose tab; scroll on image at a hue (Ctrl fine; Alt+scroll changes tab [code]) or edit graph; a picker on "node placement" only *visualises* | a picker on a slider that does not set it | cursor → hue band, signed step; max luminance (white level) |
| retouch ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/retouch/)) | choose algorithm (Ctrl/Shift on its buttons change scope); set wavelet scales; shape icon; Shift+click relative source, Ctrl+Shift+click absolute; click target | source mode sticky and hidden; shapes belong to one scale; fill picker reads module *output* [code] | per shape: target geometry, source offset, algorithm, scale, fill |
| liquify ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/liquify/)) | tool; click/drag; right-click to finish; Shift+scroll strength, Ctrl+scroll direction; Ctrl+click node types | scroll modifiers differ from masks; 100-node limit | list of warps: paths of nodes with radius, hardness, strength vector |
| crop ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/crop/)) | drag edges; Shift keeps ratio on edges but constrains axis when moving; aspect combo | applied only when focus leaves the module [code] | rectangle + aspect |
| rotate and perspective ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/rotate-perspective/)) | right-drag for horizon; or structure: auto (Ctrl/Shift on button = enhancement), rectangle or lines; curate lines; click a *fit* button (Ctrl/Shift = which parameters) | nothing applied until fit; needs N lines per direction; random detection; right-drag here = horizon | one line; or a tagged line set / 4 corners + which parameters to fit |
| lens correction | method first; fine-tuning hidden | needed an auto-apply preset to be on by default ([#13408](https://github.com/darktable-org/darktable/issues/13408)) | metadata only: no interaction |
| vignetting ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/vignetting/)) | drag centre and nodes; Ctrl keeps ratio | gestures undocumented | centre + two radii |
| haze removal | two sliders | — | nothing (estimate internal) |
| highlight reconstruction | method; overlay toggles; threshold "until highlights stop being white" | — | a threshold judged by looking |
| denoise profiled | profile from EXIF; mode | — | nothing |
| filmic rgb ([M](https://docs.darktable.org/usermanual/4.6/en/module-reference/processing-modules/filmic-rgb/)) | exposure first, neutral WB first; white picker (max), black (min), auto-tune, grey (mean) | grey pick shifts black and white [code]; black reads −16 EV on failure; one-shot: redo after exposure change | min/max/mean of a norm over a region |
| sigmoid, AgX | AgX: documented order exposure → preset → auto tune → pivot pickers → contrast | pivot pickers depend on levels already picked [code] | min/max of channels; mean luminance |
| rgb levels ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/rgb-levels/)) | handles; or black/grey/white point pickers; or "auto region" (drag, right-click cancels) | "auto region" looks like a picker but is not one (own canvas code, ignores one-picker rule) [code]; point pickers use the first channel only [code] | three scalars per channel |
| rgb curve / tone curve | mode then tab; "show" picker; rgb curve only: "create curve" with Ctrl/Shift during drag | tone curve has no create picker | min/mean/max of the curve channel |
| graduated density | drag line; Ctrl+scroll density, Shift+scroll hardness [code]; hue picker | contradicts the mask gradient's scroll meanings on the same canvas; undocumented | a line + density, hardness |
| negadoctor ([M](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/negadoctor/)) | seven pickers in order: film base, Dmax, offset, shadows cast, highlights WB, paper black, print exposure | each formula uses earlier picks; order enforced only by maths; re-picking an early one silently invalidates the rest [code] | a chain of seven region statistics |
| color look up table | pick; Shift+click a patch to replace its source; Shift+click empty space to add | the pick is consumed by a later gesture on another widget | pairs (source Lab, target Lab) |
| color mapping | "acquire as source" on one image, "acquire as target" on another | cross-image state; changing clusters wipes both | whole-image cluster statistics of two images |
| monochrome | drag filter circle on a/b plane, or picker | — | a/b centre + size |
| drawn masks ([M](https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/drawn/)) | blend mode "drawn"; shape icon (Ctrl continuous); right-click ends; per shape: scroll, Shift+scroll, Ctrl+scroll with different meanings (gradient: curvature, compression) | scrolling during creation changes defaults for the next shapes [code]; manual and code disagree on the ellipse feather toggle | shapes in image coordinates with feather, opacity, combine mode |
| AI object mask | click foreground, Shift+click background, Ctrl+Shift+click clear, **right-click applies** | first click encodes the image; cache invalidated by crop | points → mask |
| parametric masks ([M](https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/parametric/)) | blend mode; channel tab (set depends on colour space); 4 markers; "set range" picker per tab (Ctrl = output, ignored unless output sliders shown); hover keys `c`, `m`, `a` | one channel per pick; all channels multiply | per channel trapezoids from region min/max |
| raster masks | choose source module | source must be enabled and earlier | a reference to another mask |
| global color picker | point/area (Ctrl or right-click); statistic; add samples | samples after the full pipe; its statistic does not apply to module pickers [code] | a measurement, no parameter |

Shared machinery and its rules [code, `color_picker_proxy.c`]: **one
picker at a time** in the whole darkroom; activating a picker **switches
its module on**; each picker remembers its last area for the session only;
touching any slider of the module or moving focus **turns the picker off**,
except four "keep-active" display pickers; sampling happens on the preview
pipe and the result arrives later, so **modifiers are read when the sample
lands, not when the user clicks**; and picks are one-shot (an upstream
change does not re-run them). A module declares point/area, blur and
input/output sampling, but the statistic (mean, min, max) and whether the
picker writes or only shows are hard-coded per module.

Patterns that the inventory makes plain:

- **The same data, many rituals.** "Mean of a region at the module's
  input" is needed by color calibration, white balance, exposure, color
  balance rgb, negadoctor, the colour LUT and monochrome, with seven
  different sequences.
- **Ctrl on a picker has three meanings** (area mode; same hue instead of
  opponent; output slider or raised curve), right-click about a dozen
  (area mode, reset area, delete shape, finish path, delete node, reset
  handle, **apply** an AI mask, horizon, deselect, cancel, type a value),
  and the scroll modifiers change meaning with the focused module
  (inventory, section 3).
- **Commit rules differ**: on each gesture (most), on focus loss (crop),
  after a fit click (perspective), after two acquisitions on two images
  (colour mapping), after "validate" (colour checker).
- **Order is implicit**: negadoctor's seven pickers, AgX's levels before
  pivot, filmic after exposure; nothing marks a later step stale.
- **Display-only and writing pickers look identical.**

### 4.2 A vocabulary of typed needs

Derived from the inventory: what modules actually need, once the gestures
are stripped away. Fourteen kinds cover every module in 4.1 except liquify
(a custom ritual, 4.5). Geometry is always in normalised image coordinates
of the *unflipped, uncropped* image, so a mask or a line survives a later
crop, flip or rotation (the darktable bug
[#3218](https://github.com/darktable-org/darktable/issues/3218)).

| Need | Data produced | Constraints declared | Default gesture | Keyboard alternative |
|---|---|---|---|---|
| `sample` | a statistic of pixels: mean, median, min, max or a percentile, per channel, in a declared space, read at a declared port (the module's input by default) | shape allowed (point, area, stroke, whole image), statistic, space, port, several regions (add/subtract) or one | click: point (small radius); drag: area; Shift: add a region; Alt: subtract | a reticle moved by arrows, Enter samples, `+`/`-` grow the area |
| `target` | a reference value or colour the sample is mapped to | default (neutral grey, 18 % grey, a swatch), allowed sources (swatch, typed value, another sample, a reference image) | the chip opens a swatch; dragging from the canvas onto the chip samples it | typed values in the chip |
| `point` | one image position | inside the image or not | click, then drag the handle | arrows move it, Shift for larger steps |
| `line` | two positions (a direction and a length) | horizontal or vertical snapping | drag from start to end | two points in sequence |
| `line_set` | n lines tagged by orientation (vertical, horizontal) | at least n of each kind | drag each line; orientation guessed from the angle, togglable | add line command, then points |
| `region` | a rectangle or quad | aspect ratio, minimum size, inside the image | drag the box; edges and corners as handles | typed size and ratio |
| `ellipse` | centre, radii, rotation, feather | circle only or ellipse | drag from centre; the standard handles (section 2, rule 11) | typed values |
| `shapes` | a list of shapes (drawn mask elements, spots), each with handles | kinds allowed (brush, circle, ellipse, path, gradient), with or without a source (clone, heal) | the shape tool's drag; source by a second drag | commands for each kind |
| `range` | one or more channel ranges with feathering | channels offered in perceptual terms (lightness, colour, saturation), polarity | produced from `sample`s (add, subtract), then refined on a range bar | typed bounds |
| `point_to_param` | a mapping "pixel under the cursor → parameter coordinate" (a tone band, a hue band, a curve abscissa) | which parameter family, the readout to show | drag up or down on the image (S1); wheel over the image as a profile option | arrows on the hovered band |
| `mask` | a reference to a mask object | kinds allowed, one or several | choose or create in the mask panel; intent masks (S2) | the palette |
| `image` | another image, a second exposure or a reference look | same camera, same size, any | drop or pick from the filmstrip | the picker dialog |
| `choice` | one of N candidates rendered as previews (AI proposals, mask proposals, presets) | how many, whether "none" is allowed | hover previews, click picks | arrows and Enter |
| `scalar` | a single value set *by looking*: a threshold shown on the image (clipping, highlight reconstruction) | range, what to overlay while it moves | a slider with the "show what this affects" quasimode forced on while dragging | arrows |

The inventory, re-expressed:

- color calibration: `sample(mean, input)` + `target(neutral)`, advanced;
  checker: `region(quad)`; white balance and exposure: `sample(mean)` (+
  `target(18 % grey)` for exposure);
- filmic, AgX: `sample(max)` white, `sample(min)` black, `sample(mean)`
  grey or pivot, each with `auto: whole image`; AgX's pivot `after` the
  levels;
- color zones, rgb curve, parametric masks: `range` built from `sample`s
  (add, subtract) on perceptual channels, all channels at once;
- tone equalizer, color equalizer: `point_to_param`, plus an auto
  estimator for the mask spread;
- negadoctor: seven `sample`s chained by `after`, presented as a guided
  sequence the first time;
- crop: `region(aspect)`; rotate and perspective: `line` (horizon) or
  `line_set` with `auto: detect`; vignetting: `ellipse`; graduated
  density: `line`;
- retouch: `shapes(with_source)`; drawn masks: `shapes`; AI object mask:
  `mask` from `point`s (foreground, background); raster masks: `mask`;
- colour mapping: `image`; colour LUT: pairs of `sample` and `target`;
  highlight reconstruction: `scalar` with its clipping overlay;
- lens correction, denoise, haze removal, sigmoid: no need beyond
  parameters (and that is fine);
- liquify: `Custom` (4.5).

Every need also declares:

- **an auto estimator** when one exists (whole-image statistic, detected
  lines, EXIF), so the need starts *provided* (E2, S6);
- **how it is shown when not provided**: an empty chip in the module
  header with a one-line hint ("Click a neutral grey area"), never a
  silent default;
- **how it is edited later**: a provided geometric need stays as handles
  on the canvas whenever its module is focused; a sample keeps its region,
  so "pick again" re-reads the same place and moving the region updates it;
- **undo**: each provided or changed need is one undo step, named after
  the need ("White balance: sample neutral area").

### 4.3 How the unified layer runs them

- **One picker.** Every `sample` runs the same sampler: the same cursor,
  the same point/area switch (click or drag, no mode to set first), the
  same statistic chip (mean by default, median, min, max as declared), the
  same live readout, the same "pick again", the same add/subtract roles.
  The sampler shows *which port it reads* ("before this module") only when
  it is not the module's input [opinion: almost nobody needs to know].
- **Rules reversed from darktable's picker proxy.** Gesture roles are
  resolved at gesture time and travel with the request, never re-read when
  the asynchronous statistic lands; the statistic is declared and shown,
  not hard-coded and hidden; a display-only probe (a readout) looks
  different from a sample that writes; touching a slider does not silently
  end the ritual, Esc or completion does; starting a need on a disabled
  module enables it visibly, as one undo step.
- **Visible state.** Each module header shows its needs as chips: empty,
  auto, set by the user, stale. A stale chip means its port's content
  changed since the sample (the engine knows: content keys change). Only
  one ritual is active at a time; it is named in the hint bar with its
  gestures, and Esc ends it.
- **No hidden modes.** A ritual is a session started by a chip, a command
  or a canvas click on a handle, and ended by its own completion, Esc or
  focus moving to another module. No picker stays armed silently.
- **Preview.** While a ritual runs, its tentative value renders as a
  preview (S4) and becomes history only on a step or a commit.
- **Profiles.** Rituals declare gesture *roles*; the profile's gesture map
  binds roles to buttons, wheel and modifiers (section 5), so the same
  ritual feels like Lightroom in the Lightroom profile.
- **Sequencing.** By default, needs are provided in **free order**: the
  chips are all visible, and each is independent. A module may declare
  dependencies (`after: "film_base"`: negadoctor's later pickers read a
  result of the first) and the runner greys a chip until its dependency is
  provided, saying why. A module may also declare a **guided** sequence,
  which the runner presents as steps with Next and Back (a wizard for first
  use; the chips remain for later edits). Re-providing an early need marks
  the later ones stale; it never resets them silently.
- **Stale samples [option, not decided].** darktable stores the *result*
  of a picker (the parameters), so a change upstream silently invalidates
  it (tone equalizer's wands, filmic's levels). Helios could store the
  *sample* (region, statistic, port) and derive the parameters in the
  plan, so the derivation follows upstream changes. Trade-off: renders
  then depend on an image statistic (deterministic, but harder to export
  to darktable's XMP, which needs the derived values). A middle way: store
  both, keep the value, mark the chip stale and offer "update".
- **Agents.** Every need is reachable without a pointer: `need.list(node)`,
  `need.provide(node, need, value)`, `need.auto(node, need)`. A decision
  node in human mode (ROADMAP 2.5) is answered through the same needs
  (`choice`, `mask`, `region`), so humans and agents see the same thing.

### 4.4 Where it lives in Helios

It follows decisions already taken: modules are pure and declare their
surface (ROADMAP 2.1), "data access is a framework prerogative" (2.2),
and "egui-pop wraps the application's kernel; it is not the kernel"
(2.1, 2026-10-12).

- **The module declares, the framework gathers, the module derives.** A
  module lists its needs next to its parameter hints (editor-side, never
  read by the engine) and provides a **pure** derivation from the gathered
  data to a parameter patch (sample mean → illuminant; line set →
  rotation and lens shift). The framework reads the pixels a `sample` asks
  for, at the declared port, as any other data access: cached by content,
  at proxy scale for the preview and exact scale on commit. The module
  never sees a pointer.
- **The vocabulary** (need kinds, need values, image geometry, statistics)
  lives in Helios's module contract (`helios-core`, next to `ParamHint`),
  because modules depend on it and it is photographic (colour statistics,
  ports, image coordinates). The English labels and hints join the
  declarations (`helios-declarations`) so they are translated like
  parameters.
- **The machinery is generic and goes into egui-pop**: a `Ritual` trait,
  a session runner (one active ritual, Esc, steps as undo steps through the
  host's dispatch, traces into the debug bridge), gesture roles resolved by
  a gesture map with layers like the keymap, the handle kit and the hint
  bar on the viewport's overlays (the viewport with "overlays with handles
  in image coordinates" is already decided for egui-pop). egui-pop knows
  points, lines, regions and handles, not colours or modules.
- **Helios implements the photo rituals** on that trait (the sampler with
  statistics, the range builder, the line set, the shape editor), binds
  them to need kinds, and wires a ritual's output through
  `derive` → `param.set` on the value store (widgets.md), so history,
  origin and agent arbitration work unchanged.

A sketch [illustrative, not an API proposal yet]:

```rust
// helios-core: the vocabulary, next to ParamHint.
pub struct NeedSpec {
    pub id: &'static str,                 // "neutral", "film_base", "horizon"
    pub kind: NeedKind,
    pub label: &'static str,              // "Neutral area"
    pub hint: &'static str,               // "Click or drag over something grey"
    pub auto: Option<AutoEstimate>,       // WholeImage(Stat::Mean), DetectLines, ...
    pub after: &'static [&'static str],   // dependencies, for sequencing
    pub level: Level,                     // Basic, Standard, Advanced
}

pub enum NeedKind {
    Sample { shapes: Shapes, stat: Stat, space: Space, port: Port, multi: bool },
    Target { default: TargetValue, sources: Sources },
    Point, Line { snap: bool }, LineSet { min_vertical: u8, min_horizontal: u8 },
    Region { aspect: Option<Aspect> }, Ellipse { circle_only: bool },
    Shapes { kinds: ShapeKinds, with_source: bool },
    Range { channels: Channels },          // perceptual channels
    PointToParam { family: &'static str }, // "tone_band", "hue_band", "curve_x"
    Mask { kinds: MaskKinds }, Image { same_camera: bool },
    Choice { candidates: u8 }, Scalar { param: &'static str, overlay: Overlay },
    Custom { ritual: RitualId },           // see 4.5
}

pub trait Module {
    // ... manifest, param_hints, prepare, process as today ...

    /// What the module asks of the user, beyond sliders. Editor-side only.
    fn needs(&self) -> Vec<NeedSpec> { Vec::new() }

    /// Pure: turns a provided need into parameter changes.
    fn derive(&self, need: &str, value: &NeedValue, params: &Params, cx: &Context)
        -> Result<ParamPatch, String> { Err(format!("no need `{need}`")) }
}

// darktable's color calibration, white balance part:
fn needs(&self) -> Vec<NeedSpec> {
    vec![
        NeedSpec::sample("neutral", Stat::Mean, Port::Input)
            .label("Neutral area")
            .hint("Click or drag over something that should be grey")
            .auto(AutoEstimate::Illuminant),   // darktable's "AI detect"
        NeedSpec::target("target_colour", TargetValue::Neutral)
            .level(Level::Advanced),           // was the hidden "area color mapping"
    ]
}
```

The second need is the fix for
[#18299](https://github.com/darktable-org/darktable/issues/18299): the
target is a visible chip, advanced, shown as "not neutral" when changed,
instead of a hidden section that silently changes what the main picker
does.

### 4.5 The ritual contract, and custom rituals

When no shared ritual fits, a module brings its own, **inside** the ritual
machinery, not beside it (owner's precision): a custom ritual is a
registered implementation of the same contract, so it gets the same state
display, undo, Esc, keyboard alternatives, profile remapping, traces, tests
and sequencing with the module's other needs. Once a second module needs
it, it is promoted to a shared ritual.

**What a ritual receives**: the need it serves (spec and current value,
if any), the image geometry (size, the view transform to and from image
coordinates), read-only access to declared data (a statistic of a region,
a downscaled port image), and *gesture events by role*, never raw
pointer and modifier state.

**What it renders**: overlays in image coordinates through the handle kit
(standard handle kinds and sizes, section 2 rule 11), plus a hint (its
current step and the gestures available now).

**What it produces**: a `NeedValue` (serialisable), tentative while
previewing, committed as a step.

**Lifecycle**: `start` (from a chip, a command or a handle), `event`
(returns continue, preview, step or done), `cancel` (Esc: the runner
restores the value from before `start`), and `describe` (a serialisable
spec: id, need kind served, steps, gesture roles with default bindings,
labels). The runner, not the ritual, owns undo, Esc, focus, the hint bar
and traces.

```rust
// egui-pop: generic, knows geometry and gestures, not photography.
pub trait Ritual: Send + 'static {
    fn describe(&self) -> RitualSpec;               // serialisable, see 4.7
    fn start(&mut self, cx: &RitualCx, current: Option<&Value>) -> Flow;
    fn event(&mut self, cx: &RitualCx, ev: &RoleEvent) -> Flow;
    fn overlay(&self, cx: &RitualCx, out: &mut Overlay);   // handle kit only
    fn hint(&self) -> Hint;
}

pub enum Flow {
    Continue,
    Preview(Value),   // rendered as a tentative value, not in history
    Step(Value),      // one undo step, named by the spec
    Done(Value),
    Cancel,           // the runner restores the value from before start
}

pub struct RitualSpec {
    pub id: RitualId,                        // "helios.perspective.lines"
    pub serves: NeedKindId,                  // "line_set"
    pub roles: Vec<GestureRole>,             // declared, remappable
    pub steps: Vec<StepSpec>,                // labels, hints, optional order
}

pub struct GestureRole {
    pub id: &'static str,                    // "add_line"
    pub label: &'static str,                 // "Draw a line along an edge"
    pub default: PointerBinding,             // Drag(Primary) on Canvas
    pub command: CommandId,                  // keyboard / palette alternative
}
```

**Example: perspective lines as a custom ritual.** darktable's rotate and
perspective module (ashift) needs lines along edges that should be
vertical or horizontal, or a structure detection, then fits rotation and
lens shift (inventory 4.1). In Helios the module declares one need,
`line_set { min_vertical: 2, min_horizontal: 2 }` with
`auto: DetectLines`, and a pure `derive` that fits the correction. The
first version of its ritual is custom (`helios.perspective.lines`), with
roles `add_line` (drag on canvas), `select_line` (click a line),
`toggle_orientation` (a chip on the selected line; key `O` by default),
`delete_line` (Delete; context menu), `accept_detected` (a button over
the detected lines). The runner gives it Esc, an undo step per line, the
hint bar ("2 of 2 vertical lines; add horizontal lines or Apply"), the
remapping and the traces. When the lens correction or a future
"straighten" tool needs a line set, the ritual moves to the shared set
unchanged.

**Example: liquify.** Its need is a list of warps (point, line or curve,
each with radius and a strength vector). No shared kind fits, so the
module declares `Custom { ritual: "helios.liquify" }`. The custom ritual
implements the contract with roles `add_point`, `add_line`, `add_curve`,
`drag_strength`, `resize`, `delete_node`; the runner provides the rest.
Its path editing is the same as drawn mask paths: a candidate for the
first promotion (one shared `path` ritual for masks, liquify and
retouch).

### 4.6 Risks

- **Needs that do not fit.** Liquify, retouch's source and target pairs,
  and any future painting tool: custom rituals, as above. The risk is that
  every port becomes "custom" because it is faster; the review checklist
  asks for a reason.
- **Lowest common denominator.** A uniform sampler may lose a module's
  subtlety (darktable's filmic reads the max RGB over an area, color zones
  the hue at a point). The statistic and the space are declared to keep
  that, but the vocabulary will grow; keep it small and add a kind only
  when two modules need it.
- **darktable compatibility.** darktable's XMP stores only parameters.
  Needs that store their sample (4.3, option) need the derived values too,
  for export.
- **Performance.** Live preview of a sample on a full-resolution port is
  expensive; the sampler reads proxies while dragging and exact data on
  commit (ROADMAP "retouch on proxy, deferred exact render").
- **Gesture role explosion.** If each ritual invents roles, profiles
  cannot map them. Roles are drawn from a shared list (section 2) first;
  custom roles need a default binding *and* a command.
- **Over-wizarding.** Guided sequences help the first time and annoy
  later; the chips stay the primary surface and a guided sequence never
  blocks free order for someone who knows it.

### 4.7 Next stage: rituals as data, composed by an AI

Not to build now; the design must allow it. An AI defines a ritual on the
fly, inside a project, to drive a process network: "sample the sky, then
set the horizon line" feeding a mask node, a gradient and a rotation.

- **Rituals as data.** A ritual description is a composition of registered
  primitives (the need kinds, the shared rituals, the registered custom
  ones): steps in order or free, labels and hints, defaults, gesture roles,
  preview targets, and for each produced value **which node and parameter
  of the network it feeds** (directly, or through a module's `derive`).
  Code-only custom rituals stay possible; AI-made ones are compositions
  only, never code.
- **Scope and storage.** A ritual description belongs to a project:
  versioned with it in the vault, diffable, shareable as a file like a
  profile part. It targets a network (several nodes), not one module, and
  names nodes by stable ids.
- **Safety and quality.** Validation against the vocabulary and the
  network's declared parameters (types, ranges, ports); a dry run on the
  current image with previews; the same undo, Esc and traces; a human
  confirmation before a new ritual is saved. The agent uses the existing
  control surface (MCP, commands), nothing new.
- **What it implies now.** Keep `RitualSpec`, `NeedSpec` and `NeedValue`
  serialisable, with stable ids; no hidden state in a ritual (everything
  it holds is in its value or its spec); give every primitive a
  machine-readable description (`describe()`, the same JSON the agent
  surface lists), so an AI can discover and compose them.

```toml
# A project ritual, as data [illustrative].
[ritual]
id = "project.sky-and-horizon"
label = "Fix the sky and the horizon"
order = "guided"

[[ritual.step]]
need = "sample"            # a registered primitive
label = "Click the sky"
stat = "mean"
feeds = "node/sky_mask#range"      # through mask_parametric's derive

[[ritual.step]]
need = "line"
label = "Drag along the horizon"
feeds = "node/rotate#angle"
after = ["sample"]
```

### 4.8 A ritual editor

Since rituals are data, they can be edited.

- **Users**: power users and profile authors; the AI as a co-author whose
  proposals are reviewed there before saving.
- **What it edits**: the sequence of primitives (order, guided or free,
  dependencies), labels and hints, defaults and auto estimators, gesture
  bindings per profile, and the node and parameter each value feeds.
- **Try it live** on the current image, with the real runner: real undo,
  real Esc, real previews; leaving the editor discards the trial.
- **Validation inline**: an unknown primitive, a type mismatch with the
  parameter fed, a missing default binding or command, an unreachable step.
- **Diff** against the previous version (as profiles do on import), and
  export/import with the project or as a shareable file.
- **Built from egui-pop pieces** [planned ones]: the node graph editor for
  the step sequence and what each step feeds, the inspector for a step's
  spec, the debug bridge's traces to replay a trial.


---

## 5. Interaction profiles

The owner's requirement: people with habits are not disrupted. A
**Lightroom** profile, a **darktable** profile and the native **Helios**
profile; Capture One later. An interaction profile is not the same thing
as a Helios user profile (`crates/helios-profile`: a named set of parts a
person exports and imports); it is a *base* such a profile selects.

### What an interaction profile must cover

| Area | Lightroom profile | darktable profile | Cost |
|---|---|---|---|
| View keys | `G` grid, `E` loupe, `D` develop, `C` compare, `N` survey, `R` crop, `Q` spot removal, `W` white balance picker, `Tab`/`Shift+Tab` panels | `l` lighttable, `d` darkroom, `Tab`, `w` sticky preview | cheap: keymap |
| Rating, flags, labels | `0`–`5`, `P`/`X`/`U`, `6`–`9` colours, Caps Lock auto-advance | `0`–`5`, `r` reject, `F1`–`F5` colours | cheap: keymap |
| Compare | `\` before/after, `Y` side by side | snapshots; a held key in the Helios grammar | cheap: keymap + command |
| Brush and mask | `[` `]` size, `Shift+[` `]` feather, `O` overlay, `Shift+O` overlay colour, Alt for eraser, `Shift+W` masks [recheck keys] | wheel size, Shift+wheel feather (hardness), Ctrl+wheel opacity, `{` `}` hardness, `<` `>` opacity ([manual](https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/drawn/)) | medium: needs gesture bindings, not only keys |
| Modifiers | Alt-drag: show clipping or affected area; double-click label: reset | Ctrl/Shift on wheel: fine/coarse; right-click slider: fine pop-up and typing | medium: gesture map |
| Wheel | panel scrolls; canvas zoom by keys or click | wheel on slider changes it; wheel on image zooms with fixed stops ([#21027](https://github.com/darktable-org/darktable/issues/21027)) | medium: gesture map |
| Panel layout and order | Basic first, panels in edit order, tools strip above (crop, spot, red eye, masking) | module groups, quick access panel, history on the left | cheap: layout part |
| Terminology | "Exposure", "Highlights", "Texture", "Clarity", "Dehaze", "Color Mixer", "Masking" as names and as search synonyms | darktable module names ("tone equalizer", "color calibration", "diffuse or sharpen") | medium: a label set per profile over the same modules |
| Default tool behaviour | lens corrections on, auto-advance in culling, crop constrained to the original ratio | darktable's scene-referred defaults (exposure +0.7 EV, filmic or sigmoid) | **hard**: defaults that change the picture must stay explicit and per project |
| Workflow | develop sliders in a fixed order, global then local | module order, multiple instances, per-module masks | **hard**: not a profile matter; offered as views (stack, graph) and workshops |

**Cheap**: keys, labels and synonyms, panel layout, which tools appear in
the "basic" level. **Medium**: pointer gestures and modifier meanings,
because Helios has no binding model for them yet (below). **Hard**:
anything that changes the rendered image (defaults, module choice) or the
user's model of the edit (Lightroom's fixed panel vs darktable's modules):
a profile must never make the same edit render differently, so rendering
defaults belong to the project and its presets, not to the interaction
profile.

### Invariant vs profile-dependent reflexes

- **Invariant across profiles** (grammar of section 2): Esc cancels, undo
  covers ritual steps, right-click is a context menu, a click never jumps
  a value, double-click resets, hover never commits, the "show what this
  affects" and A/B quasimodes exist (their keys may vary), every gesture
  has a command.
- **Profile-dependent**: which key or modifier triggers each of those;
  fine/coarse modifier; wheel over canvas (zoom or pan); brush size keys
  vs wheel; rating, flag and colour keys; panel layout; labels and
  synonyms.
- **Conflicts to resolve**: darktable's right-click fine pop-up violates
  invariant 10; the darktable profile maps it to "typed entry on
  double-click of the value" and says so once [opinion: keep the invariant,
  accept the small disruption]. darktable's "wheel on a slider changes it"
  is ui-platforms pitfall 7: the darktable profile may keep it as an
  opt-in. Lightroom's Alt-for-eraser and the grammar's Alt-subtract agree.

### How profiles map onto Helios's mechanisms

What exists (read on 2026-10-12):

- egui-pop's keymap (`egui-pop-model/src/keymap.rs`) has four layers,
  `Suggestion < Default < Workspace < User`, with `when` clauses, removals
  (`"-id"`) and unbinding (`null`), `load(owner, layer, doc)` and
  `clear(layer)` ("before loading another profile"). It binds **keys
  only**: there is no pointer, wheel or modifier-gesture binding.
- `helios-profile` has a `Keymap` part (bindings with `Mod`), a `Layout`
  part and a `Settings` part, variants per system or machine, export and
  import with a diff per part, and `Profile::over(default)`.

Proposal [not decided]:

1. **An interaction profile is a package**, as the shortcuts research
   already says ("Keymaps are packages"): `helios.lightroom`,
   `helios.darktable`, `helios.native`. It contributes a keymap, a gesture
   map, a label set (labels and synonyms), a layout and the "basic level"
   module list.
2. **A new keymap layer `Profile`** between `Default` and `Workspace`
   (`Suggestion < Default < Profile < Workspace < User`). A user's own
   bindings stay on top whatever base they choose; switching base clears
   only the `Profile` layer. Without that layer, a base loaded into `User`
   would mix with personal bindings and a switch would lose them.
3. **A gesture map next to the keymap**, with the same layers and `when`
   clauses: bindings from *(pointer button or wheel, modifiers, target
   kind: handle, canvas, control)* to *gesture roles* declared by rituals
   (section 4: `adjust`, `fine`, `add`, `subtract`, `show_affected`,
   `size`, `feather`...). Rituals never read raw modifiers; they receive
   roles. This is what makes the grammar remappable and testable.
4. **`helios-profile` names its base**: a user profile gets `base =
   "helios.lightroom"` and keeps only the user's deltas in its `Keymap`
   part (and a new `Gestures` part). Import shows the base change in the
   diff like any other part.
5. **Labels per profile** are a Fluent catalogue overlay (`egui-pop-i18n`)
   keyed by module and parameter ids, plus synonyms for the palette, so the
   module ids stay stable and agents are unaffected.
6. **The conflict report** of the keymap (`conflicts()`) extends to
   gestures; the shortcuts editor shows "from profile Lightroom" as a
   source.

---

## 6. Where Helios risks repeating the problem

Read against ROADMAP.md, gui.md, widgets.md, masks.md, module-contract.md
and the code of `helios-core` (manifest), `helios-values` and egui-pop's
keymap. Each item names the anti-pattern it risks.

1. **Generated panels reproduce "one module, its sliders" (E1, E5).** The
   Parameters panel is generated from value descriptors (gui.md,
   "Parameters"). The descriptor has type, ranges, steps, unit and label,
   but nothing about *importance*, *grouping by effect*, *level* (basic or
   advanced) or the *meaning in image terms*. Porting filmic's parameters
   with their descriptors gives exactly darktable's filmic panel, faster.
   Add `level`, `group`, `effect` (a one-line tooltip) and `auto` to
   `ParamHint`/`Descriptor`.
2. **Labels fall back to internal names (E1, E12).** `ParamHint::label`
   is "a short label; the parameter's name otherwise". A missing hint shows
   `white_point_source`. Make a label mandatory for every parameter above
   the advanced level (lint, E1 test).
3. **Porting every module means porting the overlap (E5, E6).** The IOP
   inventory ports darktable's modules one by one, so filmic, sigmoid and
   AgX all arrive. `Role` (general or specialised) and `Family` exist in
   the manifest, which is the right start; add an *exclusive job* (`tone
   mapper`, `white balance`) so the engine prevents stacking instead of
   warning.
4. **Masks ported as darktable's tree (E10, E1).** masks.md maps flexi
   masks to nodes with the operators `maximum`, `screen`, `minimum`,
   `product`, `sum`, `difference`, `exclusion`, plus `mask_levels`
   "contrast and brightness". Correct as an engine; risky as a UI. The
   default mask UI should speak Add, Subtract, Intersect, Feather, Refine
   edge, with the others advanced.
5. **The graph view as the honest view (E6).** gui.md makes the graph and
   the stack two views of one network. Good for experts, but every
   feature that is *only* reachable in the graph (merges, a mask reused by
   two modules) asks for pipeline knowledge. Each needs a stack-level or
   canvas-level way in. gui.md's "explicit actions rather than precise
   gestures" in the graph is the right instinct (E7).
6. **Context menu "copy the path" (E12).** Useful for agents and scripts,
   but paths like `node/12/white_point_source` in a menu are engineer
   vocabulary. Keep them behind an advanced or developer section.
7. **Keys only (E4, section 5).** egui-pop's keymap binds keys; pointer
   gestures, wheel and modifiers have no declared meaning, so each widget
   and each ported module will read raw modifiers, which is how darktable
   ended with its per-tool modifier soup. A gesture map with roles (section
   5) should exist before the first canvas tool is ported.
8. **Wheel on sliders** (ui-platforms pitfall 7): still in gui.md.
9. **Workshops rely on AI to be simple (E1, E13).** ROADMAP 2.5 gives the
   simple surface ("only the useful sliders", "composite semantic
   sliders") to AI-built workshops. NEEDS.md says that hypothesis is
   unproven. The simple surface must exist without AI: declared levels,
   intent search, auto values and the signature interactions of section 3.
10. **Decision nodes' human mode has no UI design yet.** ROADMAP 2.5 and
    module-contract.md give decision nodes a human resolution mode ("picks
    a mask or a branch"). That is an interaction need (section 4) and
    should go through the same ritual layer, not a dialog per node kind.
11. **Settings generated from descriptors (E13).** gui.md makes settings
    pages generated and searchable; good. But rendering defaults (lens
    correction on, exposure +0.7 EV) must not become settings at all: they
    are project presets with visible names.
12. **Normalized dialogs everywhere.** gui.md's closed set of dialogs is
    good for consistency, but a tool that opens a Form for what could be a
    canvas gesture (a crop ratio, a line) moves the work off the picture
    [guess: a risk once modules are ported quickly]. Prefer canvas
    rituals; forms for things that have no place on the image.

---

## 7. Ergonomics checklist

For reviews of a module port, a panel, a widget or a ritual, and as
egui-pop guidance. A "no" needs a written reason.

**Labels and levels**
- [ ] Every parameter at the basic or standard level has a label in image
      terms and a one-line effect; none shows its internal name.
- [ ] Algorithm, version and colour-space choices are advanced or fixed.
- [ ] The module declares its job, role (general or specialised) and
      synonyms (including Lightroom and Capture One names).

**Starting point**
- [ ] Enabling the module gives a visible, sensible result with no other
      action; derived values are "auto" until touched, and shown as such.
- [ ] No step is asked of the user that the engine can do (E2).

**Interaction**
- [ ] Every value the module needs from the picture is a declared need
      (section 4); the module has no private gesture code, or its custom
      ritual is registered and implements the contract.
- [ ] Gestures use roles, not raw modifiers; each has a command and a
      visible equivalent; at most one modifier per canvas gesture.
- [ ] The grammar holds: Esc cancels with no trace, undo covers each step,
      right-click is a menu, a click does not jump a value, double-click
      resets, hover does not commit.
- [ ] Hit targets of 24 logical px or more; handles look and behave like
      every other handle.

**Seeing**
- [ ] "Show what this affects" and A/B work for this module and its mask.
- [ ] Choices (presets, modes, proposed masks) preview on hover.
- [ ] Any mode in progress is visible on the canvas, with its gestures in
      the hint bar.

**Forgiveness**
- [ ] Reset restores every value the module owns, remembered tool state
      included.
- [ ] Nothing carries over from the previous image silently.

**Discovery**
- [ ] The palette finds the module and its main actions by intent words.
- [ ] Nothing requires the manual to be found; the tooltip says what the
      control does to the image.

**Profiles**
- [ ] Keys and gestures come from the keymap and gesture map; the module
      works in the Helios, Lightroom and darktable profiles; profile
      switches never change a render.

---

## Sources

darktable issues and pull requests (github.com/darktable-org/darktable):
[#2041](https://github.com/darktable-org/darktable/issues/2041),
[#2047](https://github.com/darktable-org/darktable/issues/2047),
[#3218](https://github.com/darktable-org/darktable/issues/3218),
[#9378](https://github.com/darktable-org/darktable/issues/9378),
[#13408](https://github.com/darktable-org/darktable/issues/13408),
[#17287](https://github.com/darktable-org/darktable/issues/17287),
[#18299](https://github.com/darktable-org/darktable/issues/18299),
[#19084](https://github.com/darktable-org/darktable/issues/19084),
[#19211](https://github.com/darktable-org/darktable/issues/19211),
[#21027](https://github.com/darktable-org/darktable/issues/21027),
[#21905](https://github.com/darktable-org/darktable/issues/21905),
[#22024](https://github.com/darktable-org/darktable/issues/22024),
[#22237](https://github.com/darktable-org/darktable/issues/22237),
[release 5.6.0](https://github.com/darktable-org/darktable/releases/tag/release-5.6.0).

darktable manual and site:
[drawn masks](https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/drawn/),
[parametric masks](https://docs.darktable.org/usermanual/development/en/darkroom/masking-and-blending/masks/parametric/),
[module controls](https://docs.darktable.org/usermanual/development/en/darkroom/processing-modules/module-controls/),
[tone equalizer](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/tone-equalizer/),
[filmic rgb](https://docs.darktable.org/usermanual/4.6/en/module-reference/processing-modules/filmic-rgb/),
[FAQ](https://www.darktable.org/about/faq/).

discuss.pixls.us:
[darktable UI work](https://discuss.pixls.us/t/darktable-ui-work/56268),
[another dt UI discussion](https://discuss.pixls.us/t/another-dt-ui-discussion/58799),
[Flexi masks RFC](https://discuss.pixls.us/t/rfc-builds-available-masking-panel-revamp-and-new-masking-possibilities/59675),
[module proliferation](https://discuss.pixls.us/t/on-module-proliferation/56435),
[ELI5 filmic levels](https://discuss.pixls.us/t/eli5-black-and-white-levels-in-filmic/21757/24),
[tips from Capture One users](https://discuss.pixls.us/t/darktable-tips-from-captureone-users-wanted/57386?page=2),
[RawTherapee for new users](https://discuss.pixls.us/t/can-we-make-rawtherapee-look-less-complex-on-very-first-sight-for-brand-new-users/14105),
[RawTherapee local adjustments](https://discuss.pixls.us/t/local-adjustments-is-in-dev/18795).

GIMP:
[Getting unstuck](https://docs.gimp.org/2.10/en/gimp-getting-unstuck.html),
[Bugzilla 113477](https://bugzilla.gnome.org/show_bug.cgi?id=113477),
[Bugzilla 127451](https://bugzilla.gnome.org/show_bug.cgi?id=127451),
[Bugzilla 415796](https://bugzilla.gnome.org/show_bug.cgi?id=415796),
[Bugzilla 499690](https://bugzilla.gnome.org/show_bug.cgi?id=499690),
[foreground select commit](https://mail.gnome.org/archives/commits-list/2018-May/msg04065.html),
[GitLab #1215](https://gitlab.gnome.org/GNOME/gimp/-/issues/1215),
[MR 769](https://gitlab.gnome.org/GNOME/gimp/-/merge_requests/769),
[paths and the transient Move option](https://discourse.gnome.org/t/paths-and-the-transient-move-option/19085),
[layer vs mask](https://www.gimp-forum.net/Thread-SOOOOOOOOOOO-close-in-cleaning-up-an-image?pid=39108),
[auto-expanding layers](https://librearts.org/2023/11/gimp-autoexpand-layers/).

Other editors:
[Lightroom masking](https://www.adobe.com/products/photoshop-lightroom/masking.html),
[advanced Lightroom masking](https://www.adobe.com/vn_en/learn/lightroom-cc/web/advanced-lightroom-masking),
[HSL targeted adjustment](https://pages.adobe.com/creativecloud/en/photography-plan/lightroom-classic/hsl-target-adjustment),
[Lightroom Classic shortcuts](https://helpx.adobe.com/lightroom-classic/desktop/introduction-to-lightroom-classic/keyboard-shortcuts.html),
[preset hover preview](https://community.adobe.com/t5/lightroom-classic-discussions/disable-full-image-presets-in-latest-update/m-p/9777804),
[Point Color](https://photoshopcafe.com/remove-color-reflections-in-lightroom-classic-acr-with-new-point-color/),
[Photoshop contextual task bar](https://community.adobe.com/t5/photoshop-beta-discussions/contextual-task-bars-now-in-photoshop-beta/td-p/13658736/page/9),
[Capture One Magic Brush](https://support.captureone.com/hc/en-us/articles/4404012811537),
[DxO U Point](https://www.dxo.com/dxo-photolab/u-point/),
[RawPedia local adjustments](https://rawpedia.rawtherapee.com/Local_Adjustments),
[Snapseed](https://iphonephotographyschool.com/snapseed/).

HCI:
[progressive disclosure](https://www.nngroup.com/articles/progressive-disclosure/),
[recognition and recall](https://www.nngroup.com/articles/recognition-and-recall/),
[Fitts's law](https://www.nngroup.com/articles/fitts-law/),
[WCAG 2.2 target size](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html),
[modes and quasimodes](https://en.wikipedia.org/wiki/Mode_(user_interface)),
B. Shneiderman, "Direct Manipulation: A Step Beyond Programming
Languages", IEEE Computer 16(8), 1983.
