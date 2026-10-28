# darktable pitfalls: colour management and display

Inventory for Helios (Rust, egui + wgpu, linear Rec.2020 working space),
built from darktable-org/darktable issues and PRs read on 2026-10-12.
Helios state checked in: `crates/helios-gui/src/viewport.rs` (display
transform: Rec.2020 -> sRGB primaries matrix, per-channel clamp, sRGB encode
in the shader), `crates/helios-photo/src/color.rs` (matrices, camera matrix
normalisation, sRGB TRC), `crates/helios-export/src/lib.rs` (sRGB only,
per-channel clamp, no ICC written), `crates/helios-load/src/lib.rs`
(pictures assumed sRGB), `crates/helios-scopes` (scopes on working data),
`crates/helios-core/src/image.rs` (`sample_at`: box mean on linear data).
"Guess" marks what was inferred and not read in a thread.

---

### 1. Wayland: double colour management (app converts, compositor converts again)
- What went wrong: darktable reads a monitor profile (colord / X atom) and
  converts the image to it, but on Wayland its surface declares no image
  description, so a colour-managing compositor (KWin, Hyprland, GNOME 50)
  treats the pixels as sRGB and converts them to the monitor again. It "worked"
  on GNOME only because GNOME's CM was itself incomplete (VCGT only). KDE
  dropped colord support because of exactly this. The fix in progress (PR
  #22491) declares Rec.2020/gamma 2.2 on dedicated subsurfaces through
  `color-management-v1` and lets the compositor do the display conversion;
  a KWin developer advises using `wp_color_management_surface_feedback_v1`
  (preferred description) rather than tracking outputs, and warns that only
  one colour-management surface may exist per wl_surface (GTK4 would collide).
- Evidence:
  https://github.com/darktable-org/darktable/issues/19977
  https://github.com/darktable-org/darktable/issues/19322
  https://github.com/darktable-org/darktable/issues/20688
  https://github.com/darktable-org/darktable/pull/22491
- Helios exposure: at risk later, fine today. The viewport converts to sRGB
  primaries and encodes sRGB, which is exactly what a compositor assumes for
  an undescribed surface, so today's output is correct (gamut-limited to sRGB).
  The danger is the planned "monitor profile later" (comment in
  `display_matrix()`): applying a monitor ICC on a Wayland surface that the
  compositor also manages reproduces #19977.
- Recommendation: model the display transform as "encode for the colour space
  the *surface* declares", never "for the monitor". On Wayland, apply no
  monitor ICC; either stay sRGB or declare a wide space (Rec.2020 linear or
  gamma 2.2, or extended-linear scRGB through a float swapchain) and let the
  compositor map. Pick the space from the surface-feedback preferred
  description. Only on X11/Windows/macOS-without-EDR does Helios itself apply
  a monitor profile. Make sure egui/winit/wgpu never creates a second
  colour-management object on the same surface.

### 2. Multi-monitor: the profile of the wrong screen
- What went wrong: on Windows darktable used the primary display's profile
  even when its window lived on a secondary monitor, for years (2.6 -> 5.x),
  because colord is the detection path and it does not handle several displays
  on Windows; exported JPEGs then "looked different". On Wayland, darktable did
  not get the window position from events, so it had to re-query on every
  window move (PR #19646). The histogram-profile menu listed "system display
  profile" twice with two displays, with no way to tell them apart.
- Evidence:
  https://github.com/darktable-org/darktable/issues/3619
  https://github.com/darktable-org/darktable/issues/14784
  https://github.com/darktable-org/darktable/pull/19646
  https://github.com/darktable-org/darktable/issues/3271
- Helios exposure: at risk. `display_matrix()` is a single global; ROADMAP
  plans auxiliary windows detached to another screen.
- Recommendation: the display transform is per window (per surface), keyed on
  the monitor that window is on, recomputed on move / monitor change / scale
  change events from winit, and shown by name in the UI ("display: DELL U27,
  P3"). Viewport tiles must not be re-rendered by the engine for this: only the
  shader uniform changes.

### 3. Display profile acquired asynchronously: race at start-up
- What went wrong: darktable asks colord for the window's profile with an
  asynchronous call; about 30 % of start-ups the UI and lighttable thumbnails
  were drawn before the callback returned, so thumbnails stayed
  non-colour-managed until a window resize. PR #22491 had to make the mipmap
  (thumbnail) cache "Wayland-safe" because cached thumbnails were stored
  already display-encoded.
- Evidence:
  https://github.com/darktable-org/darktable/issues/19776
  https://github.com/darktable-org/darktable/pull/22491
- Helios exposure: at risk once a real profile lookup exists, and for
  thumbnails (ROADMAP: thumbnails in the egui-pop viewport).
- Recommendation: keep caches (thumbnails, proxies, tiles) in the working
  space or tagged with the display-transform identity, never display-encoded;
  the display transform is applied at draw time only. Version the display
  state and redraw everything when it changes, including when the first real
  profile arrives after start.

### 4. Readouts taken after the display conversion
- What went wrong: darktable's global picker, histogram and over/under-exposure
  indicator are computed at the end of the preview pipe, after `colorout`
  converted to the display profile, and then converted to the "histogram
  profile". With a measured (LUT, non "well-behaved") monitor profile, values
  clip or shift: black can't reach 0, a neutral grey ramp reads R != G != B,
  saturated reds appear hue-shifted because negatives are clipped by the
  display TRC. The bug was declared "unfixable" without pipe branching and was
  only documented; scopes even changed with the display intent.
- Evidence:
  https://github.com/darktable-org/darktable/issues/3271
  https://github.com/darktable-org/darktable/issues/12310
  https://github.com/darktable-org/darktable/issues/12047
  https://github.com/darktable-org/darktable/issues/4936
- Helios exposure: handled. `helios-scopes` reads working data before the
  display transform, and the viewport keeps `viewport.scene` (float, before
  the transform) apart from `viewport.display`.
- Recommendation: keep this as a tested invariant: picker, scopes, clipping
  and gamut overlays read working-space floats (or a declared readout space
  converted from them), never display output. Add a test: changing the display
  transform must not change any readout. Show in the readout which space it
  is in, and allow negative values to be displayed.

### 5. Averaging and resampling in a non-linear encoding
- What went wrong: darktable's picker averaged display-encoded (gamma) values,
  so mean values of mixed areas were wrong; zoomed-out darkroom views of a
  checkerboard showed the wrong colour. Preview vs export colour shifts
  (noisy, underexposed images turning red/green) came from `finalscale`
  downscaling after `colorout`, i.e. in display encoding; fixed by a new module
  order (v3.1, renamed 5.0) placing `finalscale` before `colorout`. Non-raw
  inputs were also downscaled in gamma space until PR #20004 made the input
  scaling gamma-aware.
- Evidence:
  https://github.com/darktable-org/darktable/issues/12310
  https://github.com/darktable-org/darktable/issues/17758
  https://github.com/darktable-org/darktable/issues/13682
  https://github.com/darktable-org/darktable/pull/20004
  https://github.com/darktable-org/darktable/pull/17780
- Helios exposure: handled. Pictures are linearised at load, and
  `Image::sample_at` box-averages linear floats; the display encode is the
  last step in the shader.
- Recommendation: keep every resampling, mean and blur before the display
  encode, including for thumbnails and exported downsizes. Add a parity test
  with a 1-pixel R/G checkerboard at several zooms (the case in #12310).

### 6. Out-of-gamut: per-channel clipping, intents silently ignored
- What went wrong: for matrix + TRC profiles darktable uses its own fast path,
  where negative and >1 channels are clipped independently (hue shifts); with
  LittleCMS the "perceptual" intent falls back to near-colorimetric because
  the built-in profiles carry no AtoB/BtoA tables. Users chose intents that did
  nothing. Gamut handling ended up inside tone mappers (filmic v6/v7, sigmoid
  primaries, color calibration gamut mapping) instead of at output.
- Evidence:
  https://github.com/darktable-org/darktable/issues/11777
  https://github.com/darktable-org/darktable/issues/12047
  https://github.com/darktable-org/darktable/issues/14652
  https://github.com/darktable-org/darktable/pull/16917
- Helios exposure: half done (2026-10-28). Exports go through one gamut
  mapping, `helios-colour`'s `GamutMap` (chroma reduction toward the neutral
  axis in Oklab, see `docs/icc.md`); the viewport (`display()`, the shader)
  still clamps each channel to [0, 1] after the matrix, until it has a
  per-window display target to map into.
- Recommendation: decide explicitly (ROADMAP, dated) where gamut mapping
  lives: an explicit, visible gamut-compression node before output (and the
  same function in the viewport so screen = file), with per-channel clamp only
  as the last safety net. Do not expose rendering intents that the engine does
  not implement. Add a gamut-check overlay that reads working data against the
  target gamut.

### 7. Module output depends on the export/output profile
- What went wrong: filmic v6/v7 gamut-maps against the pipe's *output*
  profile, so exporting to linear Rec.2020 then converting to sRGB elsewhere
  differs from exporting to sRGB directly, and softproofing did not feed the
  proof profile to filmic. The same history renders differently depending on
  the destination, invisibly.
- Evidence:
  https://github.com/darktable-org/darktable/issues/14652
  https://github.com/darktable-org/darktable/issues/12047
- Helios exposure: at risk when porting filmic/sigmoid. In a graph engine with
  content hashing, a hidden dependency on "where the result goes" breaks both
  caching and reproducibility.
- Recommendation: make the target gamut an explicit input/parameter of any
  node that needs it (part of its content key), with a default that is
  visible; parity tests must pin the output profile darktable used.

### 8. Softproof semantics misunderstood (and partly wrong)
- What went wrong: users expected "softproof sRGB" to show what a
  non-colour-managed browser shows; in darktable it simulates a colour-managed
  conversion to the proof profile, so with a wide-gamut proof nothing changes,
  while the histogram silently switched to the softproof profile. The thread
  (#15313) shows repeated confusion; maintainers point users to set the
  *display* profile to sRGB to emulate unmanaged viewers.
- Evidence:
  https://github.com/darktable-org/darktable/issues/15313
  https://github.com/darktable-org/darktable/issues/3271
  https://github.com/darktable-org/darktable/issues/14652
- Helios exposure: not yet implemented (the shortcut is listed in
  docs/shortcuts-research.md).
- Recommendation: two distinct, named modes: "proof" (working -> proof
  profile with a real gamut mapping -> display) and "unmanaged viewer"
  (assume the file's numbers are shown as sRGB on this display). Scopes switch
  to the proof space only when the user asks, with the space named on screen.

### 9. Exported files: missing, inconsistent or contradictory colour tags
- What went wrong: darktable 3.0 changed the embedded sRGB profile (built via
  `cmsCreateRGBProfile` with prequantised primaries) and exports looked
  different in managed vs unmanaged viewers until it rebuilt profiles from pure
  primaries (#3501). Internal profiles round-tripped with a visible blue error
  (s15Fixed16 quantisation / white point handling, fixed by #4745). A linear
  Rec.2020 TIFF carried the raw's `Exif ColorSpace = sRGB` beside a correct
  ICC, so some viewers rendered it as sRGB.
- Evidence:
  https://github.com/darktable-org/darktable/issues/3501
  https://github.com/darktable-org/darktable/issues/4106
  https://github.com/darktable-org/darktable/issues/18919
  https://github.com/darktable-org/darktable/pull/8823
- Helios exposure: at risk. `helios-export` writes JPEG/PNG/TIFF through the
  `image` crate with no ICC profile at all and only sRGB; EXIF copying (if
  added later) could carry a wrong ColorSpace tag.
- Recommendation: always embed an ICC profile (and cICP where the format has
  it), generated from exact primaries and the exact TRC Helios uses, with a
  round-trip test (export -> load -> compare in floats). Set
  `Exif ColorSpace = Uncalibrated (0xFFFF)` whenever the file is not sRGB.
  Offer at least sRGB, Display P3, Adobe RGB, Rec.2020 and linear float outputs.

### 10. Pictures (JPEG/PNG/TIFF) assumed to be sRGB on input
- What went wrong: darktable's `colorin` reads embedded ICC profiles and
  Exif/interop tags; still, non-raw images used the wrong module order
  ("JPEG v3.0" order not auto-detected, exposure behaving in gamma space,
  #12310 comments), were scaled in gamma space (#20004), and a corrupted
  embedded ICC could crash lcms2 (#18977).
- Evidence:
  https://github.com/darktable-org/darktable/pull/20004
  https://github.com/darktable-org/darktable/pull/18977
  https://github.com/darktable-org/darktable/issues/12310
- Helios exposure: at risk. `helios-load` decodes pictures and embedded raw
  previews with `srgb_decode` + sRGB->Rec.2020 matrix unconditionally: an
  Adobe RGB JPEG or a ProPhoto/linear TIFF (Helios's own future exports) loads
  with wrong colours.
- Recommendation: read embedded ICC / cICP / Exif ColorSpace + interop index
  (R03 = Adobe RGB) and convert from that; fall back to sRGB only when nothing
  is tagged, and say so in the image info. Reject or fall back gracefully on a
  corrupt profile (fuzz the parser).

### 11. HDR transfer functions: missing luminance scaling and OOTF
- What went wrong: PQ images imported ~50x too dark and exported ~50x too
  bright because darktable expressed PQ as an ICC shaper curve where 1.0 =
  10000 nits instead of reference white = 203 nits; HLG lacked the
  luminance-dependent OOTF (system gamma 1.2), which three independent ICC
  curves cannot represent. Known since 2020 (#3880), fixed in 2026 (PR #22516).
  JPEG XL's synthesised ICC and PNG cICP handling also lost HDR signalling.
- Evidence:
  https://github.com/darktable-org/darktable/issues/20562
  https://github.com/darktable-org/darktable/pull/22516
  https://github.com/darktable-org/darktable/issues/3880
  https://github.com/darktable-org/darktable/issues/17710
- Helios exposure: not applicable yet (no HDR I/O), but NEEDS.md ranks HDR
  export third among user requests.
- Recommendation: implement PQ/HLG as explicit RGB-boundary transforms outside
  any ICC machinery, with reference white 203 nits (BT.2408) and HLG OOTF on
  luminance; carry cICP + mastering/CLL metadata; test against public
  reference images (the reporter's hdr-test-images set) in CI.

### 12. HDR / wide-gamut display blocked by the toolkit
- What went wrong: GTK3 + Cairo give darktable an 8-bit sRGB surface: no HDR
  viewing, and every HDR request is answered "after the GTK4 migration
  (person-years)". Contributors proposed native per-OS surfaces; Wayland
  colour management needed ~1200 lines of subsurface hacks just to keep the
  GTK UI in Rec.709 and the image in Rec.2020 (PR #22491). Discussion lists the
  needed display parameters: primaries, transfer, min/max luminance, SDR white
  level (Windows "HDR content brightness"), relative vs absolute HDR, and clip
  (not roll-off) for editing.
- Evidence:
  https://github.com/darktable-org/darktable/issues/18078
  https://github.com/darktable-org/darktable/issues/17710
  https://github.com/darktable-org/darktable/issues/20477
  https://github.com/darktable-org/darktable/pull/22491
- Helios exposure: at risk / opportunity. wgpu can present float surfaces
  (Rgba16Float, extended sRGB on Windows/macOS: guess, to verify against wgpu
  30), but docs/gui-prototype.md notes HDR output is not measured and egui
  draws its UI assuming sRGB.
- Recommendation: define now a `DisplayTarget` struct (primaries, transfer,
  SDR white nits, peak nits, bit depth, surface format) that the viewport
  shader takes as a uniform, even if the only implementation today is sRGB.
  Prototype an Rgba16Float / scRGB surface in wbox early; editing view clips at
  panel peak, and UI elements are drawn at SDR white.

### 13. OS HDR mode silently breaks SDR colour (Windows)
- What went wrong: with Windows HDR enabled, darktable showed a blue-green
  cast (about 10 % less red) in lighttable and darkroom while the picker read
  neutral; root cause outside darktable (the Windows HDR Calibration profile;
  workaround: Windows "ICC compatibility" option). Maintainers had no Windows
  HDR hardware, the report was closed as user error first.
- Evidence:
  https://github.com/darktable-org/darktable/issues/15967
  https://github.com/darktable-org/darktable/issues/3619
- Helios exposure: at risk on Windows: same SDR-in-HDR composition applies to
  a wgpu sRGB swapchain (guess).
- Recommendation: detect OS HDR mode and SDR white level on Windows (DXGI
  output desc, DISPLAYCONFIG_SDR_WHITE_LEVEL), show it in diagnostics, and
  prefer an scRGB float surface when HDR is on so Helios, not the OS ICC
  emulation, controls the mapping.

### 14. No diagnostics: colour bugs cannot be triaged
- What went wrong: almost every display-colour report ("not enough
  saturation", "export differs", "too red") turned into a long thread asking
  for `darktable-cmstest` output, profiles, and viewer settings; many ended as
  configuration problems, some as real bugs found only after weeks. cmstest
  only covered X11/colord until PR #22491 added Wayland diagnostics; on Windows
  users could not even find it.
- Evidence:
  https://github.com/darktable-org/darktable/issues/15804
  https://github.com/darktable-org/darktable/issues/14784
  https://github.com/darktable-org/darktable/issues/3271
  https://github.com/darktable-org/darktable/pull/22491
- Helios exposure: at risk (nothing exists yet), cheap to address.
- Recommendation: a `helios colour-diag` command and an About panel section
  listing per window: platform path (Wayland CM / X11 atom / colord / Windows
  / macOS), the surface format and declared colour space, the profile name and
  hash, HDR state and SDR white, and a verdict. Agents and bug templates ask
  for it. Include an on-screen test chart (grey ramp, primaries) as a shell
  command.

### 15. UI colours not colour-managed, separate from the image
- What went wrong: only the centre image went through the display profile;
  widget colours (hue sliders, histogram colours, picker swatches) were raw
  CSS sRGB, so on wide-gamut monitors they are oversaturated and a grey
  histogram turned pink on some profiles. Making profiles global for the GUI
  was proposed and auto-closed; PR #22491 had to colour-convert picker
  swatches, navigation thumbnail, overlays and clipping indicators one by one.
- Evidence:
  https://github.com/darktable-org/darktable/issues/4936
  https://github.com/darktable-org/darktable/issues/6945
  https://github.com/darktable-org/darktable/pull/22491
- Helios exposure: at risk: egui paints in sRGB; colour wheels, swatches and
  scope colours in egui-pop will show image colours.
- Recommendation: in egui-pop, any widget that shows an *image* colour
  (swatch, wheel, scope trace, mask overlay) takes it in a declared space and
  converts through the same `DisplayTarget` as the viewport; plain theme
  colours stay sRGB. If the whole window surface goes wide/HDR, convert egui's
  sRGB output once at composition rather than widget by widget.

### 16. Changing a default breaks old edits (white balance)
- What went wrong: old darktable stored only *changed* modules; when the
  "modern chromatic adaptation" preference made `temperature` default to
  camera reference (D65), every pre-3.0 edit whose history lacked a white
  balance entry silently re-rendered with a D65 white balance, including in
  lighttable thumbnails. Fixed (PR #8599) by inserting the legacy defaults for
  histories that lack an auto-applied module; images opened before the fix
  stayed wrong.
- Evidence:
  https://github.com/darktable-org/darktable/issues/7145
  https://github.com/darktable-org/darktable/pull/8599
  https://github.com/darktable-org/darktable/issues/11377
- Helios exposure: at risk for XMP import (helios-xmp) and for Helios's own
  history: helios-parity forces `chromatic-adaptation=legacy`, i.e. parity
  already depends on a darktable preference.
- Recommendation: an edit stores fully resolved parameters for every active
  node, never "default"; defaults only apply at creation. The XMP importer
  reproduces darktable's version-dependent auto-apply rules (darktable version,
  workflow and chromatic-adaptation preference), with fixtures from old
  versions.

### 17. Camera matrix source changes re-colour existing images
- What went wrong: switching from darktable's own `adobe_coeff` table to
  rawspeed's data (#11221) changed camera-reference coefficients, so to keep
  old edits darktable turned them into "user modified" white balance, which
  then confused users and tripped the "white balance applied twice" warning.
  DNG handling kept evolving: ForwardMatrix preferred per spec only since 2026
  (PR #22383, new imports only), CameraCalibration applied since 2024
  (PR #16624), and a DNG with an embedded matrix stopped being used when the
  camera was not in the supported list (#10848).
- Evidence:
  https://github.com/darktable-org/darktable/issues/11377
  https://github.com/darktable-org/darktable/pull/22383
  https://github.com/darktable-org/darktable/pull/16624
  https://github.com/darktable-org/darktable/issues/10848
- Helios exposure: at risk. `camera_to_working` takes the rawler
  `xyz_to_camera` matrix as given; an update of rawler's camera data changes
  every image's colour without any edit changing.
- Recommendation: record the matrix (values and source: embedded DNG
  ColorMatrix1/2 + illuminants, ForwardMatrix, CameraCalibration, decoder
  table) in the image's metadata in the vault at import, and use the recorded
  one for existing edits; a new source applies only on explicit "reset".
  Implement DNG dual-illuminant interpolation and ForwardMatrix per spec.

### 18. White balance applied twice / where the adaptation happens
- What went wrong: with the modern workflow, white balance in `temperature`
  (camera reference) plus chromatic adaptation in `color calibration` is the
  rule; any user WB in `temperature` combined with color calibration doubles
  the correction, hence a permanent warning. Some pre-colorin modules
  (highlight reconstruction, raw CA) need neutral channels, which led to the
  "as-shot to later D65" workflow (PR #15602) and a new `temperature` version.
- Evidence:
  https://github.com/darktable-org/darktable/issues/11377
  https://github.com/darktable-org/darktable/pull/15602
  https://github.com/darktable-org/darktable/issues/7145
- Helios exposure: at risk when color calibration is ported; today Helios
  applies as-shot multipliers (parity in legacy mode).
- Recommendation: a graph-level check (decision/lint) that flags two nodes
  both performing white balance / CAT on the same path; carry "the white
  balance currently applied" as context metadata between nodes so CAT nodes
  know it, rather than guessing.

### 19. Learned models fed working-space numbers as if they were sRGB
- What went wrong: darktable's AI denoise passed Rec.2020/ProPhoto RGB with
  only a gamma curve to a model trained on sRGB-primaried data, so pink turned
  teal; out-of-sRGB values were clamped to [0, 1], losing wide-gamut headroom.
- Evidence:
  https://github.com/darktable-org/darktable/pull/20813
- Helios exposure: at risk: ROADMAP plans an AI surface and AI-assisted
  modules on a linear Rec.2020 pipe.
- Recommendation: every model node declares its expected input space
  (primaries, transfer, range) in its manifest; the framework inserts the
  conversion and preserves out-of-range residuals (convert, run, add back what
  was clamped).

### 20. Printing: double colour management with printer profiles
- What went wrong: darktable converted to the printer ICC with LittleCMS but
  never told CUPS, which colour-managed again (`cm-calibration` false);
  macOS needed its own fix (PR #20721). Users also saw prints darken after a
  CUPS filter update.
- Evidence:
  https://github.com/darktable-org/darktable/pull/20721
  https://github.com/darktable-org/darktable/issues/19732
- Helios exposure: not applicable yet.
- Recommendation: when printing arrives, the print job carries an explicit
  "already converted" flag per platform and a test that prints to file and
  checks the numbers.

### 21. Hand-written colour constants and curves drift from the standards
- What went wrong: several colour bugs were constants: prequantised sRGB
  primaries (#3501), a wrong Rec.709 transfer curve used in AVIF export
  (PR #8823), round-trip error in internal profiles (#4106), PQ/HLG scaling
  (#20562). Each took months to identify because outputs only looked "a bit
  off".
- Evidence:
  https://github.com/darktable-org/darktable/issues/3501
  https://github.com/darktable-org/darktable/pull/8823
  https://github.com/darktable-org/darktable/issues/4106
  https://github.com/darktable-org/darktable/issues/20562
- Helios exposure: partly handled. `helios-photo::color` has inverse and
  white-point tests; matrices are 7-digit literals rather than derived from
  xy primaries.
- Recommendation: derive RGB<->XYZ matrices from xy primaries and white at
  build or test time (one function, Bradford/CAT02 for D50 ICC tags), test
  every TRC against published reference values (BT.709, sRGB, PQ, HLG) and
  each space round-trip at 1e-6 in f64.

---

## Top 5 for Helios now

1. **Embed colour tags in exports and read them on input** (pitfalls 9, 10):
   `helios-export` writes no ICC and `helios-load` assumes every picture is
   sRGB. Both are small, testable changes with a round-trip test, and they stop
   Helios from producing files other apps misread.
2. **A `DisplayTarget` abstraction per window, surface-declared, not
   monitor-applied** (pitfalls 1, 2, 3, 12): replace the global
   `display_matrix()` with per-window state (primaries, transfer, SDR white,
   peak, surface format), applied only in the shader, recomputed on monitor
   change. Design it so the Wayland path declares a colour space instead of
   applying a monitor profile.
3. **Decide gamut mapping explicitly** (pitfalls 6, 7): today both viewport and
   export clamp per channel. Put one gamut-compression function shared by
   viewport and export, and make any node that depends on the target gamut
   take it as an explicit, hashed input.
4. **Freeze colour inputs of an edit** (pitfalls 16, 17): store resolved
   parameters and the camera matrix (with its source) in the vault, so a
   rawler update or a default change never re-colours existing images; the XMP
   importer replays darktable's version-dependent defaults.
5. **Colour diagnostics from day one** (pitfall 14), plus the invariant test
   that readouts never depend on the display transform (pitfall 4, already
   true in `helios-scopes`: lock it with a test).
