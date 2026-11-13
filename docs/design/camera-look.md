# A camera look fitted to the embedded JPEG

Status: **design accepted, steps 2 and 3a built, the command built**
(2026-11-13). The plan
(study, a headless fit, then a module off by default) is accepted with the
recommended answer to each question of section 8 (two modules, a 192 px
proxy, the chart ruling the colour, the table inline, the fitter in
`helios-calibrate`). Step 2, the headless fit, is in
`helios_calibrate::look`, its follow-up (the hue/saturation table, the
Delta E 2000 refinement, near-black frames) included (section 5,
"Built"); step 3a's bricks and modules are in (section 6, "Built"), and
so is the command `look.camera.fit` ("Built: the command"); step 3b's
user side is to do.

Related notes: [calibration.md](calibration.md) (the chart profile,
the accurate reference this note sits next to),
[../raw-development.md](../raw-development.md) (the raw chain against
darktable's), [../camera-profiles-research.md](../camera-profiles-research.md)
(where camera colour data comes from), [process-bricks.md](process-bricks.md)
(bricks and assemblies), [import-connectors.md](import-connectors.md)
(section 6, what follows a camera body).

## In short

- **The problem.** A raw opened with neutral defaults does not look like
  the picture the camera showed on its screen: darker or flatter, other
  hues, other saturation. "My raw doesn't look like the JPEG" is the most
  common complaint about darktable, and Helios promises a familiar start.
- **The idea**, from LightCraft: every raw carries the camera's own
  rendering of the same frame, its embedded JPEG preview. Fit a small,
  smooth model from the raw's scene-linear pixels to the preview's, per
  file, check it on pixels the fit never saw, and use it only when it
  passes strict gates.
- **The model:** a 3×3 matrix (neutrals kept), then a hue/saturation
  table, then a tone curve and a chroma curve. Every output pixel still
  comes from the sensor data; the JPEG only supplies correspondences.
- **The loss:** Delta E 2000 between the look's render and the preview,
  on a held-out third of the pixels taken in spatial blocks.
- **The gates** refuse a preview that is too small, too compressed or
  framed differently; a scene with too little colour or contrast, or too
  many clipped pixels; a picture style the model cannot follow
  (black-and-white, local effects); and any fit that does not beat
  Helios's default render clearly on the held-out pixels.
- **In Helios:** an optional **camera look**, off by default, fitted on
  demand (or at import, by a per-camera choice) from the preview
  `helios-load` already reads; the fit's numbers are copied into the
  edit, so a render never refits and stays the same. The chart profile
  stays the accurate reference; the camera look is the familiar start.

## 1. The problem

A camera's JPEG is a rendering: the maker's colour matrix and hue
twists, a contrast curve with a shoulder, saturation that falls off in
the highlights and rises in the shadows, often a picture style
("Standard", "Vivid", a film simulation) and sometimes local tone
mapping (Sony DRO, Nikon Active D-Lighting, Canon Auto Lighting
Optimizer). A raw developer starts elsewhere: the standard matrix
(Adobe's DNG matrices through rawler), a white balance, an exposure
default and a generic tone mapper (sigmoid at darktable's defaults in
Helios). The colours are defensible, often more accurate, but they are
not what the photographer saw on the back of the camera, and they
differ from one maker to the next in ways a generic curve cannot know.

Two answers exist elsewhere:

- **Per-camera profiles** that imitate the maker's styles (Adobe's
  "Camera Standard" DCPs, built by Adobe for each body and style). They
  need a lab, per body, and are not open.
- **Fitting the embedded JPEG** of each file: no lab, no per-body data,
  and the picture style the photographer chose comes along. This is the
  route LightCraft takes and the one studied here.

A chart profile ([calibration.md](calibration.md)) answers another
question, "what were the real colours", and stays the reference for
accurate work (reproduction, product shots). The camera look answers
"start me where the camera was".

## 2. LightCraft's method

Source: [LightCraft](https://github.com/storytold/lightcraft) (MIT or
Apache-2.0), its note
[`docs/camera-preview-colour.md`](https://github.com/storytold/lightcraft/blob/a75497d63405b448b03227ebcffbb08fca18ef48/docs/camera-preview-colour.md)
and the engine's
[`camera_preview` module](https://github.com/storytold/lightcraft/tree/a75497d63405b448b03227ebcffbb08fca18ef48/crates/engine/src/camera_preview)
(`proxy.rs`, `pairs.rs`, `colour.rs`, `hue_sat.rs`, `tone.rs`,
`look_fit.rs`, `mono.rs`), read at commit `a75497d`. Ideas only: no code
is taken, and Helios writes its own. In LightCraft the look is used
only when the camera has no measured colour matrix (most of the formats
it decodes); it is the default start there, not an option.

### 2.1 Pairs

- A **sensor proxy** of at most 96×96 pixels is developed from the raw
  (white balance, black and white levels, active crop), and the
  embedded JPEG is decoded (DCT scaling, colour managed, in the colour
  space the raw's Exif names when the JPEG is untagged) and reduced to
  the same size. Each proxy pixel is one pair, sensor RGB against JPEG
  RGB, with no pixel-exact registration needed at that size.
- Excluded from the colour pairs: near-black JPEG pixels (luminance
  under 0.015), JPEG highlights (0.85 and up), non-finite values, and
  **clipped sensor pixels** (any channel at 0.99 of white under the
  proxy pixel: a clipped sky goes magenta through a matrix while the
  JPEG shows it white, which pulled the matrix toward grey and the
  chroma curve down).
- **Lens-corrected JPEGs** (Nikon Z, Fujifilm, Panasonic, many compacts)
  do not line up with the uncorrected sensor near the edges: the sensor
  proxy is also warped by the file's own correction tables and the
  better-aligned of the two is kept, and a second attempt fits only the
  pixels **away from edges** (where a 3×3 neighbourhood spans a
  luminance ratio under 3).

### 2.2 Model

1. A ridge-regularised 3×3 **chromaticity** matrix (on luminance-
   normalised RGB, coefficients bounded by 8), so the matrix does not
   try to encode the tone curve.
2. A **hue/saturation table** in the layout of DNG's
   `ProfileHueSatMap`: 72 hues × 5 saturations × 5 values, each node a
   hue shift, a saturation scale and a value scale, applied in linear
   ProPhoto RGB. Fitted to the residual after the matrix with Gaussian
   kernels (2.5° of hue, 0.15 of saturation, 0.12 of encoded value),
   weighted by saturation, shrunk toward the identity where samples are
   few, the zero-saturation row left as the identity so neutrals stay
   neutral, and luminance restored (the tone curve owns it). A matrix
   alone rendered a lime shirt olive (16° off); the value axis matters
   (dark yellows turned toward orange while brighter ones turned toward
   green in the same frame).
3. A **tone curve** on luminance, fitted by **quantile matching** (the
   median of each 1/32 of sorted scene luminances paired with the median
   of the same 1/32 of sorted JPEG luminances): monotone by
   construction, needs no exact registration, keeps black, extends past
   the observed highlights with a bounded shoulder. Its steepest segment
   is bounded (an elasticity of 4) so shadows' noise is not amplified.
   An older conditional-median fit is tried only when no quantile look
   passes.
4. A **chroma curve**: 8 nodes over display luminance scaling chroma at
   constant luminance, since cameras saturate shadows more (up to 1.4×)
   and bleach highlights (0.1 to 0.3× above 0.85). Kept only when it
   lowers the held-out error.

Matrix alone and matrix + table are both completed (tone, then chroma)
and compared on the held-out pixels; the better one is kept.

### 2.3 The held-out check

Every third pair (by index) is held out of every fit. The completed
look must, on those pairs:

- cut the squared error (linear display values) to **below 0.7** of the
  neutral fallback's (camera RGB taken as sRGB, the default tone map);
- have a per-channel **RMS error of at most 0.10** (0.055 rejected half
  the public samples tried, which were still 1.5 to 3× better than the
  fallback).

Choices between candidates (matrix or matrix + table, chroma curve or
not) are made in gamma-encoded values, where a missed dark shirt weighs
more than a slightly missed bright rock.

### 2.4 LightCraft's gates

Read before porting; each one answers a failure seen on real files.

| Gate | Threshold | What it catches |
|---|---|---|
| Preview size | long edge at least 10 % of the raw's and at least 384 px | DNGs carrying only a thumbnail |
| Aspect ratio | the two proxies agree within 2 % | previews cropped to another ratio |
| Structure | log-luminance correlation (5×5 blurred) at least 0.85, best shift at most one proxy pixel | a preview of another frame, a badly misaligned one |
| Enough pairs | at least 256 usable pairs | tiny or mostly excluded frames |
| Enough colour | at least 5 % of samples with a linear channel spread above 0.05; input covariance nonsingular with a bounded condition number | grey scenes, where a matrix is underdetermined |
| Tone range | brightest 1/32 at least 1.2× the darkest 1/32 in scene luminance; under 1.5×, the curve must also render the bright areas no worse than the fallback | near-uniform frames, curves extrapolated from a narrow range |
| Highlight clipping | the fitted curve may clip at most 0.1 % more of the proxy than both the plain render and the preview | curves that blow highlights the camera kept |
| Steepness | tone curve elasticity at most 4 | noise amplified in the shadows |
| Held-out score | below 0.7 of the fallback's squared error, per-channel RMS at most 0.10 | everything else that fits badly |
| Related at all | Spearman rank correlation of luminance at least 0.5 (partial and tone-only looks) | a curve matched to the brightness of an unrelated picture |
| Monochrome | every JPEG pixel but one in a thousand has its channels within 0.002 | black-and-white picture styles: tone only, no colour fit |

When the full look fails, a **partial look** is tried: the tone curve,
the chroma curve and, when the scene has colour enough, the matrix at
half weight (half identity), no table; it is kept only when it beats
the fallback by the same 0.7 in linear and in gamma-encoded values and
passes the rank check. Black-and-white JPEGs get a tone curve alone.
Near-black frames get a tone-only fit with lower floors and stricter
counts. LightCraft reports, on 1,781 public files that render, 1,027
full looks and a median CIEDE2000 to the camera JPEG of about 3 for the
files it changed; the neutral fallback's median is 13.6.

Two things LightCraft does that Helios should not copy as such:

- **It refits when its fitter changes** (a look version bump refreshes
  previews, and edits may render differently once a source is decoded
  again). Helios keeps an edit's render fixed (section 3.3).
- **Its baseline is weak.** Its neutral fallback (camera RGB as sRGB) is
  about one stop too dark and keeps a third of the colour, so "0.7 of
  the fallback" is an easy bar. Helios's default render already uses a
  standard matrix and sigmoid; the bar is set against that render
  (section 4.4).

## 3. How it maps onto Helios

### 3.1 What Helios has

| Area | Where | Use here |
|---|---|---|
| Embedded previews | `helios_load::preview_oriented` (the largest preview rawler finds, with the raw's orientation tag; previews are stored as the sensor is, not turned), `helios_load::preview_bytes` and `crates/helios-load/src/sparse.rs` (a raw read without its sensor data) | the JPEG side of the pairs, cheap to read even from a connector |
| Preview colour | `helios_load::working` reads a raw's preview as sRGB; `crates/helios-load/src/colour.rs` already reads Exif `ColorSpace` and the interoperability index `R03` (Adobe RGB) for pictures | a gap: the fit must read the preview in the space the raw's Exif names (step 2) |
| Raw front | `templates/composites/helios.raw-front@1.helios-composite.json` (`rawprepare`, `whitebalance`, `highlights`, `demosaic`, `colorin`) | the sensor side of the pairs, rendered small |
| Default render | `channelmixerrgb` (CAT16 from the as-shot light), `exposure`, `sigmoid` at darktable's defaults | the baseline the look must beat |
| Colour maths | `helios_photo::compare` (`lab`, `delta_e_2000`, tested on Sharma, Wu and Dalal's pairs), `helios_photo::color` (`Mat3`, `camera_to_working`) | the loss and the matrices |
| Calibration | `helios-calibrate` (`lab`, `solver`: the neutral-keeping parametrisation `P = I + U Nᵀ`, Levenberg-Marquardt on Delta E 2000; `run`, `request`), the planned `core.matrix3@1` brick and `chartprofile` module (calibration.md, slice 6) | the matrix stage, its constraint and its solver; the brick |
| Camera defaults | calibration.md section 6 (`calibration/defaults`, keyed by camera, `@serial` optional) and import-connectors.md section 6 (what follows a body) | "start raws of this camera from the camera look" |

### 3.2 Where the look sits

The look has a **colour half** (scene-referred) and a **tone half**
(scene to display), and they belong in different places of the chain:

- **Colour:** the matrix and the hue/saturation table act on the
  working space right after `colorin`, where the chart profile acts,
  and keep the neutral axis, so `channelmixerrgb`'s white balance
  workflow is unchanged.
- **Tone:** the tone curve and the chroma curve map scene luminance to
  display, which is `sigmoid`'s job. With the look's tone on, it takes
  sigmoid's place (sigmoid bypassed); exposure still feeds it, so the
  exposure slider moves the photo along the camera's curve.

Proposed (Q1): two modules, `cameracolour` and `cameratone`, filled
together from one fit and shown together as one **Camera look** entry
in the module list, with a toggle per half. Each is an assembly of
bricks: `core.matrix3@1` (shared with `chartprofile`), a new
`core.huesat@1` (the DNG hue/saturation map semantics, CPU and WGSL),
and a new `core.curve1d@1` for the luminance and chroma curves.

With a **chart profile** active, the chart rules the colour half: the
camera look's colour half is off and its header says so; the tone half
can stay on top of the chart's colours (Q3). A camera without a
standard matrix (rare with rawler, common in LightCraft's case) is
where the colour half helps most: the fit then starts from the
identity.

### 3.3 Deterministic, stored in the edit

- **Fit once, store the numbers.** Modules do no IO (design rule 2):
  the fit runs in a command or a job, and its result (matrix, table,
  curves) is copied into the two modules' parameters, as `chartprofile`
  copies a profile. A render reads only parameters; it never reads the
  preview and never refits. A Helios that later fits better changes no
  existing edit; refitting is an explicit action, one undo step, and the
  fitter's version is stored beside the numbers.
- **Same input, same bits.** The fit is a pure function of the two
  proxies and the clip mask: fixed proxy size and box filter, f64
  arithmetic in a fixed order (no reduction whose order depends on
  threads), a deterministic held-out split, no random initialisation,
  fixed iteration caps. The same raw and preview give the same
  parameters on every machine and every run; a test asserts it.
- **Provenance.** The parameters keep the fitter version, the preview's
  hash and size, the held-out score and the baseline's, so the header
  can say what it was fitted to and how well, without the preview.
- **Size.** The table is 72 × 5 × 5 nodes of three numbers (5,400
  numbers); inline in the edit, rounded to four decimals, that is about
  30 KB per version. Q4 weighs that against a smaller grid or a
  content-addressed data input.

### 3.4 What the user sees

- Nothing by default: raws open as they do today.
- **Camera look** in the Color family; enabling it runs the fit (a
  second or less: two proxies of a few thousand pixels) and shows, in
  its header, one of:
  - "Fitted to the camera's JPEG: Delta E 2.4 (default 9.8)", the two
    held-out means, rounded;
  - "Tone only: the camera's JPEG is black and white";
  - "Partial: tone and chroma, colours damped";
  - "Not fitted: <reason>", the gate that refused, in words ("the
    embedded preview is too small", "the preview is framed differently
    from the raw", "too much of the picture is clipped", "the camera's
    picture style uses local effects"), and the modules stay at the
    identity.
- **Compare with the camera's JPEG**: the before/after view gains the
  embedded preview as the "before".
- **Amount** (0 to 100 %): blends the matrix and the table toward the
  identity and the curves toward the default tone map, so a user can
  keep part of the camera's rendering.
- **Refit** in the module's menu (after a crop or a white balance
  change the fit is still valid, since it is fitted on the as-shot
  render; refitting is for a newer fitter).
- **Per camera:** "Start raws of this camera from the camera look", a
  camera default in the sense of calibration.md section 6, applied at
  import (section 6, step 3b).
- **Agents:** `look.camera.fit {image, dry_run}` answers the report
  (gates, scores, the parameters) and, without the dry run, applies it.

## 4. The fit

### 4.1 Inputs

- **Sensor side:** the raw front at the photo's as-shot parameters
  (white balance, highlights, demosaic, `colorin`), then
  `channelmixerrgb` and `exposure` at their defaults, rendered at proxy
  size (Q2: 96 px as LightCraft, or 192): scene-linear working RGB. A
  clip mask: a proxy pixel is clipped when any sensor sample under it
  is at 0.99 of `highlights`' clip level or above (calibration's
  sampler reads clipping the same way).
- **Preview side:** the largest embedded preview, decoded in the space
  the raw's Exif names (sRGB, or Adobe RGB with `ColorSpace` 2 or
  interoperability index `R03`), linearised, converted to the working
  space, turned by nothing (preview and sensor share the stored
  orientation) and box-reduced to the same proxy after cropping to the
  raw's active area's aspect.
- **Lens correction:** when the raw carries the camera's correction and
  the preview is corrected, the sensor proxy is warped first; until
  Helios reads makers' correction tables, the away-from-edges attempt
  covers those files.

### 4.2 Model and stages

In the working space (linear Rec.2020), with `x` a sensor-side pixel:

1. **Matrix** `P`, 3×3, neutral axis kept (`P n = n`, `n` the as-shot
   neutral, calibration's `P = I + U Nᵀ` with six unknowns),
   chromaticity-normalised as LightCraft does so the tone stays out of
   it, ridge-regularised toward the identity, coefficients bounded.
   Solved by linear least squares, then refined on Delta E 2000 with
   the calibration solver's Levenberg-Marquardt.
2. **Hue/saturation table** on the residual, in the DNG layout (72 hues,
   5 saturations, 5 values), Gaussian kernels and shrinkage toward the
   identity as in 2.2, the zero-saturation row the identity, luminance
   restored.
3. **Tone curve** on luminance by quantile matching (32 quantiles,
   with extra dark knots when the frame has enough deep shadows), keeping
   black, monotone, with a bounded shoulder past the highest observed
   value and a bounded steepness; applied as a luminance ratio so the
   hue is kept (sigmoid's "preserve hue" way).
4. **Chroma curve**, 8 nodes over display luminance, a chroma ratio at
   constant luminance.

Candidates: matrix alone, matrix + table; each completed by 3 and 4,
with and without the chroma curve. The kept one has the lowest held-out
loss; a stage is kept only when it lowers it.

### 4.3 Loss

**Delta E 2000** between the look's display render and the preview, in
Lab (D65, `helios_photo::compare`), over the held-out pairs: the
reported score is the mean, with the 95th percentile alongside. Fitting
inside each stage uses the stage's natural least squares (linear
algebra for the matrix and the curves, kernel regression for the
table), then the matrix is refined on Delta E 2000; the choice between
candidates and every gate use Delta E 2000 on the held-out pairs. This
measures what the user compares (colours as seen) and weighs dark
errors fairly, which is what LightCraft's gamma-encoded comparison was
reaching for.

### 4.4 The held-out split

LightCraft holds out every third pair by index. On a proxy, neighbouring
pixels are nearly the same colour, so an interleaved split says little
about fitting a region the fit never saw. Helios holds out **blocks**:
the proxy is cut into 8×8-pixel tiles and every tile whose index (row
major) is 0 modulo 3 is held out, a third of the area in a fixed
pattern that does not depend on the content. The split is computed
once, before any exclusion, so the same pixel is held out in every
attempt (all pixels, away from edges, tone only). A held-out set left
with fewer than 64 usable pairs refuses the fit.

### 4.5 Gates: when to refuse

Thresholds start at LightCraft's where it measured them; the ones Helios
adds are set in step 2 on synthetic pairs and checked on the raw corpus
([../raw-corpus.md](../raw-corpus.md)). Each refusal has a reason the
header shows in words.

**The preview**

- None, or its long edge under 10 % of the raw's or under 384 px.
- Aspect ratio more than 2 % from the raw's active area.
- **Too compressed:** an estimated JPEG quality under 75, from the
  preview's quantisation tables (a Helios addition: heavy blocking on a
  small preview biases the table and the chroma curve).
- Structure: log-luminance correlation under 0.85, or a best shift of
  more than one proxy pixel (another frame, a misaligned preview).

**The scene**

- Fewer than 256 usable pairs, or under 64 held out.
- **Clipping:** more than half of the proxy clipped on the sensor side,
  or more than 5 % of the preview at 255 in any channel where the
  sensor is not clipped (the camera clipped what the raw kept; the
  curve's shoulder cannot be learnt from it). Clipped pixels are always
  left out of training.
- Colour: under 5 % of samples with a channel spread above 0.05, or an
  ill-conditioned input covariance: no matrix and no table, the partial
  look may still apply.
- Tone range: brightest 1/32 under 1.2× the darkest 1/32 in scene
  luminance: no tone curve.

**The camera's picture style**

- **Monochrome** (every pixel but one in a thousand grey within 0.002):
  tone curve only, colour left as Helios's default.
- **Unrelated** (Spearman rank correlation of luminance under 0.5):
  refused, whatever the score.
- **Local effects:** a maker note naming a picture effect a global
  model cannot follow (toy camera, miniature, soft focus, posterisation,
  HDR painting, partial colour) refuses the fit. Local tone mapping
  (Sony DRO, Nikon Active D-Lighting, Canon Auto Lighting Optimizer) and
  film simulations do not: the fit is attempted, the header names the
  style ("fitted to a JPEG with DRO on"), and the held-out score decides.
  Reading these tags is per maker and grows over time; an unknown tag is
  ignored, and a style the model cannot follow still fails the score.

**The fit**

- Held-out mean Delta E 2000 not below **0.7 of the default render's**
  (Helios's own baseline, section 2.4) or above an absolute **6**
  (provisional, set in step 2): refused, or the partial look is tried.
- Highlight clipping: the look may clip at most 0.1 % more of the proxy
  than both the default render and the preview.
- Tone curve steepness above 4 after smoothing: the curve is repaired
  (smoothed window) or refused.
- Non-finite values anywhere: refused.

The **partial look** (tone curve, chroma curve, matrix at half weight
when there is colour enough, no table) is tried when the full look
fails, with the same 0.7 bar and the rank check. These are acceptance
gates for a starting look, not a claim of colour accuracy: the chart
profile is that.

## 5. Step 2: the headless fit (one agent, one PR)

A pure module, `helios_calibrate::look` (Q5), with no IO and no GUI.

- **Types:** `LookInputs { sensor: proxy, preview: proxy, clipped: mask,
  neutral: [f64; 3], baseline: proxy }` (the default render at proxy
  size, for the bar); `LookFit { version, matrix, table, tone, chroma,
  kind: Full | Partial | ToneOnly, report }`; `Refusal { gate, detail }`
  with one variant per gate in 4.5; `Report { held_out: { mean, p95 },
  baseline: { mean, p95 }, pairs, held_out_pairs, attempt }`.
- **Functions:** `split(width, height)` (the block split), `pairs`
  (exclusions and the clip mask), `fit_matrix`, `fit_table`,
  `fit_tone`, `fit_chroma`, `fit(inputs) -> Result<LookFit, Refusal>`
  trying all pixels then away from edges, full then partial; `apply`
  (the reference CPU render of a fit, used by the tests and later by
  the bricks' tests).
- **Synthetic pairs, in the test kit:** a scene of flat patches,
  gradients and noise in scene-linear working RGB; a known "camera"
  (a matrix keeping neutrals, a hue twist on a few hues, an S curve with
  a shoulder, chroma falling off in the highlights) renders it into a
  preview, then 8-bit sRGB quantisation and a little noise.
- **Tests:**
  1. the known camera is found back: held-out mean Delta E under 1, the
     matrix within 1 % per coefficient on a matrix-only camera;
  2. the table earns its place: with a hue twist, matrix + table beats
     matrix alone on the held-out pairs; without one, the table is
     dropped;
  3. neutrals stay neutral (`P n = n` to 1e-12, the table's zero-
     saturation row the identity);
  4. the tone curve is monotone, keeps black, respects the steepness
     bound;
  5. one test per gate, each refusing where expected and nowhere else:
     tiny preview, aspect off, shifted preview, too few pairs, grey
     scene, flat scene, heavy clipping, monochrome preview (tone only),
     shuffled preview (unrelated), a vignette or a local brightening
     drawn into the preview (fails the score), a fit worse than the
     baseline;
  6. the split: fixed for a size, a third of the area, the same pixels
     in every attempt;
  7. determinism: two runs give bit-identical `LookFit`s, serialised.
- **Also in this step:** the raw's preview read in the space its Exif
  names (`helios-load`), with a test on an Adobe RGB tagged sample.
- **Not in this step:** bricks, modules, commands, maker-note styles
  (the gate's hook exists, with an empty list), lens warping.

If it runs over the hour, it lands the matrix, tone curve, split, gates
and tests 1, 3 to 7, and leaves the table and the chroma curve (test 2)
to a follow-up of the same size.

### Built (2026-11-13)

`helios_calibrate::look` holds the split, the pairs, the matrix (`P = I +
C Bᵀ`, `B` a basis orthogonal to the neutral, six unknowns solved by
ridge least squares of the luminance-normalised render along the
preview's chromaticity), the tone curve (quantile matching, a three-knot
smoothing then a refusal past the steepness bound, a shoulder toward 1
past the last knot), the chroma curve (8 nodes, kept when it lowers the
held-out mean by 2 %), `fit`, `apply`, `estimate_quality` (IJG tables)
and every gate of 4.5 with its words (`Gate::words`). Inputs and choices
made while building it:

- `LookInputs` also carries the preview's own clip mask (a channel at
  255 under the proxy pixel) and a `Source` (the raw's and the preview's
  sizes, the estimated JPEG quality, a named local effect): the caller
  reads them from the files.
- The colour pairs leave out preview luminances (linear) under 0.015 and
  from 0.85, and pixels at 255 in the preview; the matrix weighs each
  pair by the preview's luminance (a dark 8-bit pixel's chromaticity is
  the least certain); the tone curve and the chroma curve use the
  highlights too.
- Gates run in this order: inputs, preview (size, aspect, quality, local
  effect), pair counts, sensor then preview clipping, tone range,
  Spearman, structure (a shift of up to 3 pixels searched), then the
  attempts: all pixels then away from edges, each full then partial; the
  first refusal is the one reported. A grey scene gives a partial look
  without a matrix (`Gate::NotEnoughColour` in the report's notes), a
  black-and-white preview a tone-only look scored on lightness alone.
- The follow-up (fitter version 2) added the three stages left out:
  - **Hue/saturation table** (`look::table`, `fit_table`): DNG's layout
    and node order (72 hues × 5 saturations × 5 values; hue shift,
    saturation scale, value scale), Gaussian kernels of 2.5°, 0.15 and
    0.12, samples weighted by saturation, shrunk toward the identity, the
    zero-saturation row the identity, luminance restored. Three choices
    differ from LightCraft's: HSV is taken in the working space with each
    channel divided by the neutral (not in linear ProPhoto), so the
    neutral the matrix keeps has saturation 0 whatever it is; the value
    axis reads the scene-linear value as `sqrt(v / (1 + v))`, since the
    table sits before the tone curve where values pass 1; and every value
    scale is 1, the luminance being restored after the table. The table
    is fitted for full looks only, on the training pairs after the
    matrix, and kept only when the completed look (tone and chroma
    curves refitted) lowers the held-out mean by 2 % (test 2).
  - **Delta E 2000 refinement** (`refine_matrix`): the linear matrix is
    refined with the chart solver's Levenberg-Marquardt and its
    neutral-keeping parametrisation (`solver::levenberg_marquardt`,
    `assemble` and `orthogonal_basis`, now shared, the look's own copy
    of the basis gone), on at most 1,024 training pairs picked evenly and
    30 steps. Each render is scaled to the preview's luminance before
    Delta E 2000: the lightness is the tone curve's, so the matrix stays
    a colour stage, as the linear fit is. The refined matrix is kept when
    its completed look has the lower held-out mean (`Report::refined`).
  - **Near-black frames**: when the median preview luminance of the
    training-side pairs is under 0.015, the pairs are taken down to 0.001,
    the counts are doubled (512 pairs, 128 held out) and the look is tone
    only, scored on lightness, with `Gate::NearBlack` in its notes.
- The look and the calibration profile (`helios_calibrate::profile`, the
  `chartprofile` module) stay two things. A profile is measured on a
  chart, kept in the vault by its own format version and applied to
  every photo it is chosen for; a look is fitted per file to the camera's
  own rendering and copied into that file's edit with the fitter's
  version. Both act right after `colorin` and keep the neutral; with a
  profile on, the look's colour half (matrix and table) is off and its
  tone half (tone and chroma curves) may run on top of the profile's
  colours (Q3). They share the solver and Delta E 2000; the look compares
  renders in `helios_photo::compare`'s D65 Lab, the profile the chart's
  D50 references in `lab`.
- `helios_load` reads a raw's embedded preview in the space the raw's
  Exif names (Adobe RGB for `ColorSpace` 2, or 0xFFFF with `R03`).

## 6. Step 3: the module and its hooks

Two agents, two PRs, in order.

**3a. The look in the chain.** Depends on calibration's slice 6
(`core.matrix3@1`); if it has not landed, 3a builds that brick as
calibration.md describes it and calibration reuses it.

- Bricks `core.huesat@1` and `core.curve1d@1`, CPU and WGSL, tested
  against `helios_calibrate::look::apply` (equal within 1e-5), tiled
  equals untiled, GPU equals CPU.
- Modules `cameracolour` (after `colorin`, in a version 2 of the raw
  front, bypassed by default) and `cameratone` (in sigmoid's slot of
  the develop template, bypassed by default; when on, sigmoid is
  bypassed), their parameters the fit's numbers, the fitter version and
  the provenance of 3.3, and the amount.
- The command `look.camera.fit {image, dry_run = true}`: renders the two
  proxies through the photo's own chain, fits, answers the report or
  the refusal; without the dry run, writes both modules' parameters in
  one undo step. A node control request and its MCP tool
  (`helios_look_camera_fit`), the desktop command, the command line's
  `helios control look-camera-fit`.
- Tests: identity parameters change no pixel (darktable parity holds
  with the look off); on a synthetic raw and preview, the command's
  render matches the fit's held-out score; undo restores the edit; a
  refusal writes nothing.

### Built: 3a (2026-11-13)

The look's two halves are in the chain, off; the command is not.

- **Bricks.** The colour half reuses calibration's `core.huesat@1` (the
  matrix, then the DNG table, which is `core.matrix3@1`'s step bit for
  bit without a table): no new colour brick. The tone half is a new
  `core.curve1d@1` (`helios_bricks::curve1d`, CPU and WGSL): the tone
  curve as a luminance ratio (knots from black, linear, the look's
  shoulder past the last knot), then the chroma curve (8 nodes over
  display luminance), clamped to [0, 1]; at most 64 knots.
- **Modules.** `cameracolour` (`matrix`, `lut`, `amount`, `fitter`,
  `preview`) and `cameratone` (`tone`, `chroma`, `fitter`, `preview`,
  `held_out`, `baseline`). Defaults change no pixel.
- **In the chain.** "Develop one image" version 4 lists `cameracolour`
  after `chartprofile` and `cameratone` between `exposure` and `sigmoid`,
  both bypassed: renders keep version 3's bits. The builder's raw front
  (a raw opened in a picture template) appends `cameracolour` bypassed
  after `chartprofile`, as it does the profile. There is no raw front
  version 2: the network builder inserts modules since calibration.
- **In the edit** (`helios_calibrate::look::edit`): `with_look` copies a
  `Look` into both nodes, turns `cameratone` on and `sigmoid` off, and
  `cameracolour` on unless a chart profile is on (Q3 A);
  `calibrate`'s `with_profile` turns `cameracolour` off; `without_look`
  turns both off and `sigmoid` back on.
- **Tests.** The two modules render fitted looks (full with table and
  chroma, matrix only, tone only) as `look::apply` within 1e-5; v4 with
  the look off renders v3's bits; Q3 A on the edit; the brick's contract
  and its uniforms' size against the WGSL. As for the other bricks, the
  GPU form is validated by naga, not run against the CPU on a device.

Left to the command (3a's last part, done with 3b):

- `look.camera.fit` (node request, MCP tool, desktop command, command
  line), rendering the two proxies through the photo's chain.
- **Where the sensor side is read.** Section 4.1 renders the sensor
  proxy after `channelmixerrgb` and `exposure`, but `cameracolour` sits
  right after `colorin`, before them. The command must fit the colour
  half on what `cameracolour` sees (the `colorin` output) or the chain
  must move it; the tone half sees `exposure`'s output, as fitted.
- **Templates without `cameratone`.** `with_look` turns `sigmoid` off;
  a template showing the edit without `cameratone` (develop preview,
  export) would then render scene-linear. Those templates need the node
  (or the builder inserts it before `sigmoid`) before the command writes
  looks.
- The tone half's `amount` (blending toward the default tone map spans
  two nodes).

### Built: the command (2026-11-13)

The two open points of 3a are settled, and the command is in.

- **Where the sensor side is read: the module moved.** `cameracolour`
  now sits right after `channelmixerrgb` (before `flip` and `exposure`),
  in "Develop one image" version 4 and in the builder's raw front, and
  the sensor proxy is colour calibration's output. Fitting at `colorin`'s
  output instead cannot be made right: the as-shot cast is still there,
  and CAT16 is not diagonal in working RGB, so a hue/saturation table
  fitted after it cannot be carried before it. Exposure is a gain
  between the two halves: the command reads it off the two renders and
  multiplies the tone curve's scene axis by it
  (`look::render::for_chain`), which renders exactly what the fit
  scored. With a chart profile on, the colour half is off anyway (Q3 A),
  so it need not sit next to `chartprofile`. Version 4 keeps its number:
  no look could be written before the command, and both nodes are
  bypassed, so no render changes; 3a's bit-identity tests stay as they
  were.
- **Templates without `cameratone`: the builder adds it.** A network
  built with a raw that has a `sigmoid` node and no `cameratone` gets
  the catalogue's `cameratone`, bypassed, on sigmoid's input ("Develop
  (preview)", "Batch export", "Compare", and any template a user
  writes). An edit with a look (sigmoid off, the tone half on) then
  renders for display in every template; built without a raw, a network
  is as written. `with_look` keeps turning `sigmoid` off.
- **The command.** `look.camera.fit {images, dry_run}` (node-core's
  calibration commands, so the node's registry, `commands.list` and
  `command.run`, the CLI's `helios run look.camera.fit` and the MCP pair
  `helios_commands_list` / `helios_command_run` reach it, and the
  desktop registers it as a command run through its node). For each
  image: the raw rendered at 192 px through the chain
  (`look::render::prepare`: the edit's raw front and `colorin` over the
  template's defaults, colour calibration from the camera's light,
  exposure +0.7 EV, sigmoid for the baseline; the clip mask from the
  raw front's levels at full resolution; the preview box-reduced to the
  same proxy, a proxy pixel clipped where a preview pixel reaches 255 in
  sRGB), `look::fit`, then `with_look` with the preview's hash. Every
  image written is one library step together; a refusal is answered
  per image with its gate, its words and its numbers, and writes
  nothing; a picture is answered as "not a raw". The answer gives each
  look's kind, held-out and baseline scores, notes, and whether its
  colour half is on.
- **Not yet read from the files:** the preview's JPEG quality (the
  decoded preview carries no quantisation tables; the gate stays, fed
  nothing) and maker-note styles.

Left for 3b: the Camera look entry and its header (the toggle, "Fit to
the camera's JPEG", the report's held-out Delta E against the default's,
the refusal's words), the amount (the tone half's spans two nodes), the
compare-with-JPEG view, Refit, the per-camera default at import.

### Built: the header (2026-11-13)

The first slice of 3b: the Camera look entry's header, on the open photo.

- **Where.** In the Parameters panel, under the `cameratone` node (the
  entry), as the chart profile's header sits under `chartprofile`.
- **What it says.** "Fitted to the camera's JPEG: Delta E 2.4 (default
  9.8)" from the node's `held_out` and `baseline`; "Not fitted: <the
  gate's words>" after a refusal (kept in the transient
  `view/look/refusal`, cleared when another photo opens); "Not fitted
  yet" before any fit; with a chart profile on and the look on, "Colour
  half off: the chart profile rules the colours" (Q3 A).
- **What it does.** A switch (on: `cameratone` on, `sigmoid` off,
  `cameracolour` on unless a chart profile is on; off: both halves off,
  `sigmoid` on, as `with_look` / `without_look` do) and "Fit to the
  camera's JPEG", "Refit" once fitted. Both run the desktop command
  `look.camera.choose {look: fit | on | off}`: the fit goes through the
  node (`look.camera.fit` as a dry run, whose answer now carries each
  node's parameters, `params.cameracolour` and `params.cameratone`), and
  the numbers are written into the open edit's value store, one undoable
  step of the edit, not of the library. The image fitted is the one the
  chart profile's offer names (`image`, set when a raw opens).
- **The amount.** The colour half's `amount` is `cameracolour`'s own
  slider, under its node. The tone half's is still to come (it blends
  toward the default tone map, which spans two nodes).

Left for the next slices: the tone half's amount, compare with the
camera's JPEG (the before view showing the embedded preview), the fit
of a selection from the library's menus (the command takes `images`
already), the per-camera default at import, and the "Camera look"
preset in the module's preset menu.

**3b. The user's side and the hooks.**

- The Camera look entry and its header (section 3.4), the amount, the
  compare-with-JPEG before view, Refit; a kittest scenario drives the
  toggle through accesskit and reads the header's words.
- The per-camera default: a camera default rule (calibration.md section
  6's keying, camera model then optional serial) "start from the camera
  look"; at import, the import job queues the fit for raws of that
  camera after their preview is read (`preview_bytes` already keeps
  the sensor data out), applying it as the photo's first version; a
  refusal leaves the default render and is listed in the import's
  report.
- Templates: the develop template ships the two modules bypassed;
  a preset "Camera look" in the module's preset menu turns both on and
  fits.
- With a chart profile active, the colour half is off and says so
  (Q3).

## 7. Out of scope, for later

- Lens-correction warping of the sensor proxy (needs makers' tables).
- A maker-note reader for picture styles beyond the refusal list.
- Fitting a per-camera look from many files (a "camera standard"
  profile learnt from the library), then exporting it as a DCP.
- Using the look as the start of a chart-free "match this JPEG" for
  out-of-camera JPEGs shot with the raw (RAW + JPEG pairs).

## 8. Open questions

### Q1. One module or two

- **A.** One `cameralook` module after `colorin` doing everything,
  tone included, before `channelmixerrgb`.
- **B.** Two modules, colour after `colorin` and tone in sigmoid's
  slot, shown as one entry (this note).
- **C.** Colour only; the tone half becomes a sigmoid preset fitted to
  the JPEG (contrast, skew, targets).

Recommendation: **B**. Tone belongs after exposure, where the display
map is; A would put a display curve before the white balance and the
exposure. C is lighter but sigmoid's few parameters cannot follow a
camera's toe and shoulder, which is most of the visible difference.

### Q2. Proxy size

- **A.** 96 × 96, as LightCraft (its numbers apply).
- **B.** 192 × 192 (four times the pairs, better for the table's 1,800
  nodes, still a fraction of a second).

Recommendation: **B**, with step 2's tests run at both; LightCraft
already uses 192 for its camera-profile pairs.

### Q3. With a chart profile

- **A.** The chart rules the colour; the look's tone may stay on top.
- **B.** The look is refused while a chart profile is active.
- **C.** Both apply, the look fitted on top of the chart's colours.

Recommendation: **A**. The chart is the accurate reference and should
not be bent toward the JPEG; the camera's tone curve is independent of
it and is what most users miss.

### Q4. Where the table lives

- **A.** Inline in the edit, rounded (about 30 KB per version).
- **B.** A content-addressed data input in the vault, the edit holding
  its hash (as calibration's later LUT).
- **C.** A smaller grid (36 × 3 × 3) inline.

Recommendation: **A** now, **B** when calibration's residual LUT brings
data inputs; C would lose the value axis LightCraft found it needs.

### Q5. Where the fitter lives

- **A.** In `helios-calibrate`, as a `look` module.
- **B.** A new crate, `helios-camera-look`.

Recommendation: **A**. It shares Lab, Delta E, the neutral-keeping
matrix and the Levenberg-Marquardt solver; both are "fit colour from a
reference".
