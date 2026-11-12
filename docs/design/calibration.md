# Camera colour calibration from a chart

Status: **plan accepted** (2026-11-11). The owner accepted the plan below
(find the chart, measure in linear camera space, solve a 3×3 matrix on
Delta E 2000, report, save a profile per camera and illuminant, an agent
command). Calibration gets its own **Calibration focus**, a workspace in
the sense of [windows.md](windows.md). What is not decided is listed in
section 10, each question with options and a recommendation. The inventory
describes the code on `next` as of that date.

Related notes: [../camera-profiles-research.md](../camera-profiles-research.md)
(where camera data comes from, DCP and ICC input profiles, data
packages), [../raw-development.md](../raw-development.md) (the raw chain
against darktable's), [controls.md](controls.md) (handles on the image),
[process-bricks.md](process-bricks.md) (bricks and assemblies).

## In short

- The user shoots a ColorChecker (Classic 24, or the 24-patch page of the
  Passport) under the scene's light, opens the shot in the Calibration
  focus and drags **four corner handles** onto the centres of the four
  corner patches. Each patch is sampled from its centre 60 %, and checked
  for clipping and uniformity.
- The patches are measured in **linear camera RGB**, the output of the
  raw front's `demosaic`, before `colorin`. The raw front exists today
  (`templates/composites/helios.raw-front@1.helios-composite.json`), so
  raws are measured exactly from the first slice; a picture (a TIFF or a
  JPEG) is measured from its decoded linear values and the report says
  the result is approximate.
- A **3×3 matrix** is solved by weighted least squares, then refined to
  minimise Delta E 2000 against X-Rite's post-2014 reference values,
  with the neutral axis kept. An optional small 3D LUT for the residual
  is off by default.
- The **report** gives Delta E per patch (mean, max, the worst three),
  before and after, and the chart is shown as measured and corrected,
  side by side.
- The **profile** is a vault document keyed by camera and illuminant,
  exported and imported as a file. A `chartprofile` module, right after
  `colorin`, applies it per image or as the camera's default.
- **Agents** run `calibrate.chart {image, corners}`; its dry run returns
  the report and saves nothing.
- The maths and the checks live in a new pure crate, `helios-calibrate`,
  tested headless on a synthetic chart that recovers a known matrix.

## 1. Survey

### darktable (the fork, GPL-3.0-or-later)

**Colour calibration's checker** (`src/iop/channelmixerrgb.c`):

- The user drags a box of four points (`box[4]`, line 155) over the chart;
  a homography maps it to the chart's ideal rectangle (`get_homography`,
  from `src/chart/common.c`, called at line 2571).
- `_extract_patches` (line 1486) averages each patch inside a radius
  scaled by a "safety margin" slider (`radius * hypotf(1, ratio) *
  safety_margin`, line 1497), with exposure normalised on the grey patch.
- `_extract_color_checker` (line 1702) estimates the illuminant from the
  neutral patches, then solves the 3×3 mix in LMS by **linear** weighted
  least squares (27 rows per patch block, `pseudo_solve_gaussian` from
  `src/iop/gaussian_elimination.h`, line 1903). Weights follow an
  "optimise for" choice: none, high or low saturation, skin, foliage, sky,
  average or maximum Delta E (lines 1848 to 1876).
- `_compute_patches_delta_E` (line 1348) is Delta E 2000; the report gives
  the mean and max before white balance, after it and after the mix, with
  words: very good up to 1.2, good up to 2.3, passable up to 3.4, else
  bad (line 1959).

**Reference data** (`src/common/colorchecker.h`): CIE Lab (D50) for the
X-Rite ColorChecker 24 before 2014 and after 2014 (dated 2015-03-28 in
the file), the Datacolor SpyderCheckr 24, 48 and Photo, each with the
grid size, patch radius, aspect ratio and the indexes of the grey, white
and black patches. Patches are stored column-major.

**darktable-chart** (`src/chart/`): a separate tool that reads ArgyllCMS
chart descriptions and reference files and fits a Lab style with thin
plate splines (`thinplate.c`), plus a tone curve. It is a look fitter,
not a camera matrix.

- **Take:** the 2014 reference values and the chart geometry (GPL-3, the
  numbers being X-Rite's published values), the quality words and
  thresholds (users know them), the weight presets as a later option.
- **Not take:** the linear-only fit (Helios refines on Delta E), the box
  around the chart edges (Helios puts the handles on patch centres,
  easier to place exactly).
- **Decision: own**, porting the data and the geometry; darktable's
  checker is the parity oracle for the linear stage.

### dcamprof (Anders Torger, GPL-3)

A command-line camera profiler: reads measured patches (from ArgyllCMS's
`scanin` or its own tools), solves a matrix (optionally constrained to
keep white), then an optional LUT (HueSatMap) with smoothing, and writes
DCP or ICC. Its documentation is the reference on trade-offs: a matrix
first, a gentle LUT only where the matrix fails, never overfit a 24-patch
chart, and keep neutral patches exact. Last release 1.0.6; its sources
are maintained only lightly.

- **Take:** the method (white-preserving matrix, then a smooth, bounded
  residual), its warnings in the interface, its DCP layout for a later
  export.
- **Decision: own** the solver; dcamprof is an **optional oracle** on the
  bench box (the same patches through dcamprof and through Helios, the
  matrices compared), never a runtime dependency. The research note's
  "chart profiler plugin wrapping dcamprof" (camera-profiles-research,
  5.3) is superseded for the matrix by this plan; a plugin may still
  wrap it later for DCP LUT profiles.

### ArgyllCMS (Graeme Gill, AGPL-3.0 for most of it)

`scanin` finds a chart in an image from a chart recognition template
(`.cht`) and writes patch values; `colprof` makes ICC profiles; the
reference files (`.cie`, CGATS) are the common exchange format.

- **Take:** the CGATS reading (a small text format) for user-supplied
  reference values (section 10, Q4), and `scanin`'s approach (edge
  correlation against a template) for the later automatic detection.
- **Licence:** AGPL-3.0 is compatible with GPL-3.0, but its network
  clause would reach a node serving over the network. Not linked, not
  shipped.
- **Decision: not adopted**; out-of-process oracle on the bench box only,
  like dcamprof.

### Rust crates (checked on crates.io, 2026-11-11)

| Crate | Version | Licence | What it offers | Decision |
|-------|---------|---------|----------------|----------|
| `palette` | 0.7.7 | MIT OR Apache-2.0 | Lab, XYZ, white points, chromatic adaptation, `ImprovedCiede2000` | **not adopted**: Helios already owns Lab and Delta E 2000 (`helios_photo::compare`) and its colour maths (`helios_photo::color`, `helios-colour`); a second colour type system for twenty lines of maths is not worth it |
| `empfindung` | 0.2.6 | MIT | Delta E 76, 94, 2000, CMC | **not adopted**: the own Delta E 2000 is tested against Sharma, Wu and Dalal's published pairs instead (no dependency) |
| `deltae` | 0.3.2 | MIT | the same | not adopted, same reason |
| `nalgebra` | 0.35.0 | Apache-2.0 | dense and small matrices, Cholesky, SVD | **not adopted** for the matrix: 9 unknowns, a 9×9 Cholesky and a small Levenberg-Marquardt are owned in about 200 lines with tests; reconsidered for the LUT (below) |
| `levenberg-marquardt` | 0.15.0 | MIT | LM on top of nalgebra | not adopted (pulls nalgebra for one loop) |
| `faer` | 0.24.4 | MIT | dense and sparse linear algebra | **candidate for the LUT slice**: a 9³ LUT is 2,187 unknowns, a sparse regularised system; own conjugate gradients first, `faer` if they are too slow |
| `argmin` | 0.11.0 | MIT OR Apache-2.0 | general optimisers | not adopted |
| `colorimetry` | 0.1.0 | MIT OR Apache-2.0 | spectral colorimetry, illuminants | not adopted now; watched for spectral reference data (a chart's spectra under any illuminant) |

Every licence above suits GPL-3.0 and is in `deny.toml`'s allow list.

### Reference values

X-Rite changed the ColorChecker's formulation in November 2014 and
published new Lab (D50, 2° observer) values; charts made since then
(Classic and the Passport's 24-patch page) use them. Helios ships the
post-2014 values, taken from darktable's `colorchecker.h`
(`xrite_24_2014`), whose source is X-Rite's publication, with the
pre-2014 values for older charts. The chart file records the source, the
date and the licence (GPL-3 as part of darktable's file; the values
themselves are X-Rite's measurements). Whether the Passport's page
matches the Classic within tolerance is to be verified against X-Rite's
documentation before the Passport is offered as its own entry; until
then it is offered as "ColorChecker 24 (2014 formulation)".

## 2. What Helios has

| Area | Files | Use here |
|------|-------|----------|
| Handle kit | egui-pop's viewport overlay (`egui_pop::viewport::overlay`: `Handle`, `HandleKind`, `Overlay`, `Shape`, `HIT_RADIUS`; hooks `before_handle`, `under_overlays`, `after_paint`), used by `crates/helios-gui/src/masks.rs` | the four corner handles and the drawn patch grid |
| Handles as controls | `crates/helios-controls/src/gizmo.rs` (`Gizmo`, `HandleSpec`, `Drag`: one store gesture per drag, one undo step, Escape cancels, Shift and Ctrl snapping, `mask.handle.move` for agents) | the corners declared as a gizmo of four point handles; the agents' moves go through the same `Drag` |
| Viewport | `crates/helios-gui/src/viewport.rs` (egui-pop's viewport on the engine, the display transform, view values at `view/viewport/*`) | the chart shot shown, zoomed on a corner while dragging |
| Raw chain | `templates/composites/helios.raw-front@1.helios-composite.json` (`rawprepare`, `whitebalance`, `highlights`, `demosaic`, `colorin`), `modules/*` | the measurement taps `demosaic`'s output |
| Input profile | `modules/colorin/src/lib.rs` (the camera's matrix from the context field `XYZ_TO_CAMERA`, rows normalised so camera white stays white), `crates/helios-photo/src/color.rs` (`camera_to_working`, `Mat3`, `mul`, `inverse`) | the base matrix the profile corrects |
| Colour calibration | `modules/channelmixerrgb/src/lib.rs` (CAT16 from the camera's illuminant, after `colorin`) | the adaptation the solver includes |
| Camera data | `crates/helios-photo/src/camera.rs` (`CameraInfo`: levels, white balance, matrix), `crates/helios-load/src/lib.rs` (rawler decode, the camera's name) | the profile's camera key, the white level for the clipping check |
| Delta E | `crates/helios-photo/src/compare.rs` (`lab` with a D65 white, `delta_e_2000`) | reused; a D50 Lab is added for the chart's frame |
| Colour tagging | `crates/helios-colour/src/` (`InputSpace`, ICC through moxcms) | the decoded-picture path |
| Scopes | `crates/helios-scopes/` (counts on linear working data) | the uniformity statistics follow its style |
| Bricks | `crates/helios-bricks/src/` (`BrickRegistry`, `Assembly`, `ValueBrick`, `core.affine@1` per channel, not 3×3) | the module as an assembly of a new `core.matrix3@1` brick and a value brick |
| Module system | `crates/helios-core/src/module.rs`, `manifest.rs` (`Manifest`, `ModuleKind`, `Family::Color`, declared `context` fields, `Needs`, `picker`) | the `chartprofile` module |
| Vault documents | `crates/helios-vault/src/lib.rs:307` (`document`, `set_document`, `documents` by namespace and key), SQLite table `documents` (`crates/helios-vault-sqlite/src/migrate.rs`); users: `stacks.rs`, `snapshots.rs`, `settings.rs`, `journal.rs` | profiles and camera defaults |
| Commands | `crates/helios-shell/src/commands.rs` (one registry for menus, keys, palette, agents, tests) | `calibrate.*` on the desktop |
| Node and MCP | `crates/helios-inspect/src/lib.rs` (`ControlRequest`, dotted names by `serde(rename)`), `crates/helios-node/src/mcp.rs` (one tool per request, underscores in names, dry runs on the workspace and context tools) | `calibrate.chart` as a control request and the `helios_calibrate_chart` tool |
| Foci | `crates/helios-gui/src/shell/focus.rs` (`Focus::new` from a `Seed`, `viewport_on(&core.engine)`), `shell/core.rs` | the Calibration focus on the core's engine |

**What is missing:** a 3×3 matrix brick, a D50 Lab, a homography, the
chart data, a sampler with checks, the solver, the profile store, the
module, the commands, the focus.

## 3. The Calibration focus

A focus of kind **calibrate**: one chart shot open, its corners, its
report, and the saved profiles. It renders on the core's one engine, so
the shot is not decoded twice when it is also open elsewhere.

### Entry points

- File > New Focus > Calibrate Camera Colour…
- The library's context menu on one photo: "Calibrate colour from this
  chart…" (opens the focus on it).
- The `chartprofile` module's header: "Make a profile…" (opens the focus
  on the image's camera, profiles list filtered to it).
- `calibrate.open {image?}` for keys, the palette and agents.

### Layout

| Area | What it shows |
|------|---------------|
| Centre | the viewport on the shot, with the four corner handles and the patch grid drawn over it; each patch's sampling square outlined, its colour the patch's status (accepted, clipped, not uniform, excluded) |
| Right: Chart | the chart type (ColorChecker 24, 2014 or before 2014), the orientation found, two 4×6 grids side by side, "Measured" (through the camera's standard matrix) and "Corrected" (through the profile), each patch with the reference in a small inset at its centre |
| Right: Report | the quality word and mean, max, the worst three patches; the table of the 24 patches (name, Delta E before and after, status), sortable; the illuminant found from the neutral patches (xy, and the correlated colour temperature when it is near daylight or a black body) |
| Right: Save | the camera (make, model, serial when known), the illuminant's name in the user's words ("Studio flash", "Tungsten 3200 K", proposed from the measured temperature), the profile's name, "Use as the default for this camera", Save |
| Left: Profiles | the saved profiles grouped by camera, then illuminant; the camera's default marked; rename, delete, export, import, set as default, open the chart shot it came from |
| Status bar | the measured space ("Camera RGB, raw" or "Decoded picture, approximate") |

### Flow

1. **Open the shot.** A raw is measured through the raw front; a picture
   through its decoded values, with the "approximate" banner.
2. **Place the corners.** The handles start on a centred rectangle of the
   image. The user drags each onto the **centre** of a corner patch
   (dark skin, bluish green, white, black on a Classic). Shift snaps a
   handle to the nearest bright or dark extreme of the patch under it
   (later), Ctrl moves it a tenth of the pointer (the masks' convention),
   Escape cancels a drag. The grid follows live.
3. **Orientation.** The chart may be shot rotated or upside down. The
   four rotations of the corner order (and the mirror, for a chart
   photographed in a mirror or flipped) are each solved; the one with
   the lowest mean Delta E is kept and the Chart panel says "seen
   rotated 180°". The user can override it.
4. **Checks.** Each patch is accepted, or flagged clipped (excluded) or
   not uniform (kept with a warning, excludable). A click on a patch in
   the grid or the table toggles its exclusion.
5. **Report.** Every corner move re-samples (means of small regions of
   one cached render) and re-solves (milliseconds): the report is live.
6. **Save.** The profile goes to the vault; with "Use as the default",
   the camera's default for that illuminant is set.
7. **Apply.** From the Profiles list: "Apply to the selected photos"
   (one undo step per image, the `chartprofile` module turned on with
   the profile's values) or "Use as the camera's default".

Corner positions are the focus's values (in the value store, like the
masks' editing state): a drag is one gesture and one undo step of the
focus, so Ctrl+Z puts a corner back.

Until egui-pop's multi-window API lands (windows.md, slices 2 and 4), the
focus opens as a **place** in the main window through the context stack
(`calibrate` place, Escape or Back leaves it), with the same panels; it
moves to its own window with no change to its content (section 10, Q6).

## 4. Measuring

### Where

- **Raw:** the raw front without `colorin`, rendered at scale 1 on the
  bounding box of the patches only: `rawprepare` (black and white
  levels), `whitebalance` in camera reference (D65) mode, `highlights`,
  `demosaic`. The pixels are white-balanced linear camera RGB `c`. The
  same module parameters as the image's edit are used, so the profile
  matches what the edit's `colorin` receives.
- **Picture:** the decoded linear Rec.2020 values (`helios-load`). The
  "camera RGB" is then the working space, `colorin`'s matrix the
  identity. A camera JPEG has passed through the camera's tone curve and
  gamut clipping, so the profile is approximate and marked so; a linear
  TIFF from another raw converter is better but still marked.

### Geometry

The four handles are the centres of the four corner patches, in the
coordinates of the measured image (the raw front's output, before any
geometry module; the focus maps them through the viewport). A homography
`H` (four point pairs, an 8-unknown linear system) maps chart coordinates
`(i, j)` (column, row of a patch, from 0) to image pixels. Each patch's
sampling area is the square of side `0.6 × patch side` around its
centre, mapped through `H` (a quadrilateral); the patch side relative to
the pitch comes from the chart's data.

### Statistics and checks per patch

For the pixels inside the sampling quadrilateral:

- **value:** the mean of each channel (a 5 % trimmed mean, so dust and
  hot pixels do not move it);
- **clipped:** any photosite of the area at or above the clip threshold
  `highlights` uses, read on `rawprepare`'s output (before white balance
  and demosaicing, where clipping is real); for a picture, any channel at
  or above 0.995 encoded. A clipped patch is excluded;
- **too dark:** a mean below 0.2 % of the white level (noise dominates);
  a warning, kept;
- **not uniform:** the coefficient of variation `σ/μ` of a channel above
  5 %, or the means of the area's two halves (along either axis) apart by
  more than 3 % (glare, a shadow, a misplaced corner); a warning, kept,
  excludable;
- **chart-wide:** the six neutral patches must be in decreasing order of
  luminance (else the corners or the orientation are wrong) and their
  chromaticities close (else the light is mixed); both are warnings in
  the report.

Thresholds are first guesses, tuned on real shots in slice 2; they live
in one place with their tests.

## 5. Solving

### Notation

- `c_k` (3-vector): patch `k`'s white-balanced camera RGB, measured.
- `M_s` (3×3): `colorin`'s standard matrix, camera RGB to working space
  (`helios_photo::color::camera_to_working`); the identity for a picture.
- `A` (3×3): the CAT16 adaptation `channelmixerrgb` applies, from the
  illuminant estimated on the neutral patches to D50 (the module's own
  function, so the solve and the render agree). At render, colour
  calibration adapts from its own illuminant ("as shot in camera" by
  default); the report shows that one beside the chart's and their
  distance, since the corrected render matches the report only as far
  as the two agree. Applying a profile can set colour calibration's
  illuminant to the chart's (a choice in the Apply action).
- `x_k = M_s c_k`: the patch in the working space before adaptation.
- `L_k`: the reference Lab (D50); `Lab(·)`: working RGB to Lab with the
  D50 white (the frame of the reference values).
- `w_k`: the patch's weight (1 by default, 0 when excluded).

The profile corrects the colours between `colorin` and colour
calibration: a 3×3 `P` such that `Lab(A P x_k) ≈ L_k`. What is stored is
the camera-space matrix `M_c = P M_s` (camera RGB to working space), so
the profile does not depend on the standard matrix it was measured
through (section 6).

### Exposure

The patches' overall level is not the profile's business: before
solving, the `x_k` are scaled by `s` so that the mean luminance of the
neutral patches matches their references'. `s` is reported (as an
exposure offset in EV) and not stored in the matrix; exposure stays the
exposure module's.

### Neutral axis

`P` keeps neutral colours neutral: with `n` the mean direction of the
neutral patches' `x_k` (the illuminant's colour in the working space),
the constraint is `P n = n`. Colour calibration's adaptation then turns
that neutral into D50 white as it does without a profile. This removes
3 of the 9 unknowns: `P = I + U Nᵀ`, with `N` a 3×2 orthonormal basis
of the plane orthogonal to `n` (so `Nᵀ n = 0` and `P n = n`) and `U` a
free 3×2 matrix, six unknowns.

### Stage 1: linear weighted least squares

Minimise `Σ_k w_k ‖A P x_k − A t_k‖²` where `t_k` is the reference
converted to the working space before adaptation (`A⁻¹` applied to its
D50 working RGB). It is linear in `U`: normal equations of size 6,
solved by Cholesky. This is darktable's stage, with the constraint, and
the parity point with its checker.

### Stage 2: Delta E 2000

Minimise `F(U) = Σ_k w_k ΔE00(Lab(A P(U) x_k), L_k)²` by
Levenberg-Marquardt from stage 1's `U`: a numerical Jacobian (central
differences, 24×6), damping `λ` adapted per step, stop when the relative
decrease of `F` is below 1e-10 or after 100 iterations. Delta E 2000 is
smooth except at hue wrap-around and zero chroma; the neutral patches
have near-zero chroma, which is why their error is held by the
constraint and their weight in `F` can stay ordinary.

### Optional residual LUT (off by default)

A 9×9×9 3D LUT `T` applied after `P` in the working space, solved as
`min Σ_k w_k ‖Lab(A T(P x_k)) − L_k‖² + μ ‖∇²T‖²`, the smoothness term
dominant (`μ` chosen so the LUT moves no patch by more than its own
residual), identity at the grid's neutral axis and decaying to identity
away from the chart's gamut. With 24 patches the LUT is mostly the
regulariser's: it is offered with a strength slider and the report shows
both results. Solved by conjugate gradients on the sparse system.

### Tests (headless, `helios-calibrate`)

1. **Recovers a known matrix.** Take a real camera's `M_true` (a
   rawler matrix through `camera_to_working`) and an illuminant (D65),
   and synthesise each `c_k` so that `A_true M_true c_k` is patch `k`'s
   reference in D50 working RGB. Solve with `M_s = M_true` perturbed
   (another camera's matrix, or `M_true` times a random matrix keeping
   white): the chain `A P M_s` equals `A_true M_true` within 1e-6 (up to
   the exposure scale), so `M_c = M_true` when the illuminant is found
   exactly, and every patch's Delta E is below 1e-4.
2. **With noise.** Gaussian noise of 0.5 % on `c_k`: mean Delta E below
   0.5, max below 1.5, the matrix within 1 % per coefficient.
3. **Neutral kept.** `P n = n` to 1e-12 on every solve.
4. **Excluded patches.** With a patch at weight 0, the solve ignores it:
   moving its `c_k` anywhere changes nothing.
5. **Orientation.** The corners given in each rotated order (and
   mirrored) are found back and reported.
6. **Stage 1 against darktable.** The linear stage without the
   constraint matches darktable's checker on the same patches
   (`channelmixerrgb`'s LMS mix converted to a working-space matrix)
   within 1e-4.
7. **Delta E 2000.** Sharma, Wu and Dalal's 34 published pairs, to four
   decimals (for `helios_photo::compare::delta_e_2000`, today tested
   only on simple cases).
8. **Homography.** The four corners map exactly; a synthetic chart drawn
   in perspective is sampled back to its patch values.

## 6. The profile

### Data model

Profiles live in the library's vault as documents
(`Store::document`, `set_document`, `documents`):

- **namespace `calibration/profiles`**, key the profile's id: the BLAKE3
  hash (hex) of its canonical JSON without the `name`, so the same
  measurement saved twice is one profile, as images are one by content.

```jsonc
{
  "format": 1,
  "name": "Studio flash, Classic 2025",
  "camera": { "make": "Nikon", "model": "Z 6", "key": "nikon z 6", "serial": null },
  "illuminant": { "label": "Studio flash", "xy": [0.3271, 0.3402], "cct": 5620, "kind": "daylight" },
  "chart": { "id": "xrite.cc24.2014", "reference": "X-Rite, post-2014 formulation, via darktable colorchecker.h" },
  "measured": { "space": "camera", "exact": true, "image": "<content id>", "corners": [[812.5, 640.0], [3301.0, 655.5], [3290.0, 2310.0], [820.0, 2296.5]], "excluded": [] },
  "base_matrix": [ /* the XYZ_TO_CAMERA it was measured with, 9 numbers */ ],
  "matrix": [ /* M_c, camera RGB to working space, 9 numbers */ ],
  "lut": null,
  "solver": { "version": 1, "stage": "de2000", "exposure_ev": 0.12 },
  "report": { "mean": 0.84, "max": 2.11, "worst": ["D1", "B4", "A2"], "patches": [ /* per patch before, after, status */ ] },
  "created": "2026-11-11T10:00:00Z"
}
```

- **namespace `calibration/defaults`**, key the camera key (the
  normalised name of [../raw-corpus.md](../raw-corpus.md), section 4, so
  regional names of one body share it; with `@serial` appended for a
  rule on one body): `{ "profiles": [<id>, …], "pick": "nearest" }`.
- **Sharing:** export writes `<name>.helios-chartprofile.json`, the
  same document without local paths (the image is named by its content
  id only); import checks the format and the camera key and adds it. A
  DCP export for other software is a later slice (section 10, Q2).

### Choosing a profile for a photo

A camera may have several profiles, one per illuminant. When a raw of
that camera opens with the camera default on, the profile picked is the
one whose illuminant `xy` is nearest to the photo's as-shot illuminant
(the one `channelmixerrgb` finds), if within 0.02 in CIE 1960 `uv`;
otherwise none, and the module's header says so ("no profile for this
light"). The choice is written in the edit's parameters, so an edit
renders the same after profiles change (section 7).

## 7. The `chartprofile` module

- **Place:** in the raw front (a version 2 of the composite) right after
  `colorin`, before `channelmixerrgb`; bypassed by default, so renders
  without a profile are unchanged and darktable parity holds.
- **Parameters:** `matrix` (9 numbers, `M_c`), `profile` (the id, for
  provenance and the header's name; never read at render), `lut` (off,
  or a reference to a LUT data input, a later slice). Modules do no IO
  (design rule 2): applying a profile copies its values into the
  parameters.
- **Context:** `XYZ_TO_CAMERA`, to compute `M_s`.
- **Plan:** `P = M_c M_s⁻¹`, so the module turns what `colorin` gives into
  what the profile's matrix gives; for a picture, `M_s = I`.
- **Built as an assembly:** a value brick computing `P` from the
  parameters and the context, and a new image brick `core.matrix3@1`
  (`out = P · in` on the colour channels, alpha kept, with its WGSL
  variant), which `colorin` and `channelmixerrgb` can later share.
- **Family:** Color; its header shows the profile's name and the quality
  word, and "Make a profile…" opens the Calibration focus.
- **Tests:** identity when `M_c = M_s`; on the synthetic chart, the
  render's patches give the solver's Delta E; tiled equals untiled; the
  GPU variant equals the CPU reference.

## 8. Commands, for agents and the desktop

Each command is a shell command (menus, keys, palette, tests) and a node
control request with its MCP tool (`helios_calibrate_*`). Every one that
writes has a dry run.

| Command | Arguments | Answer |
|---------|-----------|--------|
| `calibrate.chart` | `image` (content id), `corners` (four `[x, y]` in the measured image's pixels, any order), `chart` (default `xrite.cc24.2014`), `exclude` (patch names), `lut` (bool, default false), `dry_run` (default **true**), `save` (`{name, illuminant, default}`, used when `dry_run` is false) | the report: per patch (name, measured, reference, Delta E before and after, status), mean, max, worst three, orientation, illuminant, exposure offset, the matrix; and with `save`, the profile's id |
| `calibrate.profiles` | `camera?` | the profiles (id, name, camera, illuminant, quality, default) |
| `calibrate.apply` | `images`, `profile` (or `none`), `dry_run` | per image the parameters it would set; one undo step per image |
| `calibrate.default` | `camera`, `profiles` (or empty to clear), `dry_run` | the rule before and after |
| `calibrate.remove`, `calibrate.export`, `calibrate.import` | `profile` / `path` | the profile; removing one in use leaves edits unchanged (they hold their values) |
| `calibrate.open` | `image?` | opens the Calibration focus (desktop only) |

The default dry run makes the first call safe: an agent can try corner
positions, read the report and only then save. The node answers
`calibrate.chart` without a desktop; the desktop answers it when it holds
the library, so a profile saved by an agent shows at once in the
Profiles list.

## 9. Build slices

Each slice builds, passes its tests headless, and leaves the application
usable. Slices 1 to 5 need no GUI.

1. **Chart data and geometry** (`helios-calibrate`): the ColorChecker 24
   (2014 and before 2014) with source and licence notes, the D50 Lab,
   the homography, the sampling quadrilaterals. Tests 7 and 8.
   **Built** (2026-11-12): `chart` (both charts, row by row, spot-checked
   against darktable's values and patch centres; the patch side, 0.793
   of the pitch, derived from darktable's sampling square), `lab`,
   `homography`, `layout` (the four handles on the corner patches'
   centres, each patch's centre, sampling quadrilateral and pixels, with
   a plain mean and variance per channel that slice 2 makes robust).
   Test 7 is `helios_photo::compare`'s Sharma, Wu and Dalal test; test 8
   draws a chart in perspective and samples it back exactly, and a
   misplaced chart shows on every patch's variance.
2. **Sampler and checks:** the trimmed means, clipping, darkness and
   uniformity flags, the chart-wide checks, on synthetic chart images
   (a clipped patch, a gradient patch, a misplaced corner). Thresholds
   set here.
   **Built** (2026-11-12): `sampler` (5 % trimmed means with the count
   kept; the clipped fraction of each area read on `rawprepare`'s output
   at `highlights`' level, `clip × 0.987`, or 0.995 for a picture, the
   patch excluded above 1 %; too dark under 1 % of the white patch's
   luminance or 0.2 % of the white level; not uniform above a 5 %
   variation or halves 3 % apart; the neutrals' order, their
   chromaticities' spread (0.02) and cast (0.03)). The thresholds live in
   `sampler::THRESHOLDS`, each documented with the synthetic chart that
   sets it; the tests draw a clean chart (noise, hot pixels) that passes
   everything, then a clipped, a dark and a gradient patch, a misplaced
   corner, swapped and flipped corners and a colour cast, each caught
   where expected and nowhere else.
3. **Solver:** stage 1, the constraint, stage 2, orientation. Tests 1 to
   6, the report structure.
4. **`calibrate.chart` dry run:** the measurement graph (raw front
   without `colorin`, regions of the patches), the control request, the
   MCP tool, the shell command. Tested on a synthetic mosaic made from
   the synthetic chart (the inverse of the raw front, in the test kit)
   and on a decoded picture.
5. **Profiles in the vault:** the two namespaces, save, list, remove,
   export, import, the default rule and the nearest-illuminant pick;
   `calibrate.chart` with `save`. Tested on the in-memory and SQLite
   stores.
6. **`core.matrix3@1` and the `chartprofile` module,** the raw front
   version 2, `calibrate.apply` and the camera default applied at open.
   Module tests and a desktop scenario (apply, undo).
7. **The Calibration focus:** as a place in the main window first; the
   corner gizmo, the grid overlay, the Chart, Report, Save and Profiles
   panels. A kittest scenario drives the corners through accesskit and
   checks the report; snapshots of the panels. Moves to its own window
   with windows.md slice 4 or 7.
8. **Residual LUT** (off by default): the regularised fit, a LUT data
   input for the module, the strength slider.
9. **Later:** automatic chart detection (corners proposed, the handles
   kept for correction), the SpyderCheckr charts, two-illuminant
   interpolation (DCP-style), DCP export.

## 10. Open questions

### Q1. Where the correction sits relative to colour calibration

- **A.** After `colorin`, before `channelmixerrgb`, keeping the neutral
  axis; the white is still colour calibration's (this note).
- **B.** The profile includes the adaptation (solved to D50 directly);
  applying it sets colour calibration's illuminant to none.
- **C.** Inside `channelmixerrgb`, as darktable's checker writes its
  mix into the module's matrix.

Recommendation: **A**. The user's white balance workflow is unchanged,
one profile works across small changes of light, and darktable parity
holds with the module off. B is simpler but locks the white to the
chart's light; C ties Helios's profile to a darktable module's
parameters.

### Q2. Storage and sharing

- **A.** Vault documents, exported as a Helios JSON file (this note).
- **B.** A user data package (camera-profiles-research, 5.4), written as
  a DCP.
- **C.** A now, and a DCP export (and a data-package entry) as a later
  slice.

Recommendation: **C**. The vault keeps profiles with the library that
was calibrated and is what the plan accepted; a DCP export later lets
the data packages and other software read them.

### Q3. Keying and choosing

- **A.** Camera model and an illuminant label; the user picks per image.
- **B.** Camera model and serial, picked by hand.
- **C.** Camera model (serial optional), the default picking the profile
  of the nearest illuminant within a threshold, else none.

Recommendation: **C**, with the pick always shown and overridable.

### Q4. Reference values

- **A.** X-Rite's post-2014 values from darktable's `colorchecker.h`
  (and the pre-2014 ones).
- **B.** BabelColor's averages of many measured charts (their licence
  to check first).
- **C.** A and, later, the user's own measured values imported from a
  CGATS file (a spectrophotometer reading of their own chart).

Recommendation: **C**: A now, user values when someone asks.

### Q5. The neutral constraint

- **A.** Hard: `P n = n` exactly (this note).
- **B.** Soft: the neutral patches weighted up.
- **C.** None (darktable).

Recommendation: **A**; it is dcamprof's advice and makes the profile
compose with colour calibration's white.

### Q6. The focus before multi-window

- **A.** Wait for egui-pop's multi-window API (windows.md slices 2 and 4).
- **B.** A place in the main window now, its own window when the API
  lands, with the same panels.
- **C.** A modal dialog.

Recommendation: **B**.

### Q7. The residual LUT

- **A.** A 9³ 3D LUT, regularised, off by default (the accepted plan).
- **B.** A 2.5D hue and saturation map as in DCP's HueSatMap, better
  behaved outside the chart's gamut and exportable to DCP.
- **C.** No LUT.

Recommendation: **A** as accepted, revisited when the DCP export (Q2)
is built, where B would be the natural form.

### Q8. Did "focus calibration" mean autofocus fine-tuning?

The owner's comment "focus calibration" is read here as **calibration
in its own focus** (a workspace). It could also mean **autofocus
fine-tune calibration**: measuring whether a lens and body focus in
front of or behind the target, and by how much, to set the camera's AF
micro-adjustment.

- **A.** The Calibration focus only (this note).
- **B.** Autofocus fine-tuning only.
- **C.** Both: this note now, autofocus as a separate study.

Recommendation: **A**, the reading this note takes, for the owner to
confirm; C if autofocus was meant too.

What autofocus fine-tuning would take: a focus chart (a ruler or a
target at 45°, or a flat target shot through a series of AF
adjustments); a sharpness measure on edges (a slanted-edge MTF50 brick
over a vertical profile of the ruler, or on each shot of the series);
the AF adjustment value of each shot, from maker notes (not standard
Exif, per maker); a series view plotting sharpness against the
adjustment (or the sharp band's offset from the target in millimetres),
and a recommended value per lens and focal length. It is a diagnostic,
not a profile in the pipeline, and mirrorless bodies focusing on the
sensor rarely need it.
