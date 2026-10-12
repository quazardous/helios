# darktable pitfalls: pixel pipeline and image processing

Inventory of pitfalls darktable hit in its pixel pipeline (tiling, regions,
cache, history and module order, numerics, defaults, masks), read from
darktable-org/darktable issues and pull requests, and what they mean for
Helios. Research date: 2026-10-12. Sources were read with `gh` (bodies and
comment threads); statements marked *(inference)* are mine, not stated in a
thread. Helios state was read from `ROADMAP.md`, `docs/module-contract.md`,
`docs/masks.md`, `crates/helios-core`, `crates/helios-testkit`,
`crates/helios-xmp`, `crates/helios-parity`, `crates/helios-gpu` and
`modules/`.

---

### 1. History order is not pipe order

- **What went wrong.** darktable's pipe order is not the order of the
  history: since 3.0 every edit carries an `iop_order` per module, derived
  from an `iop_order_version` (a generated list) or a custom
  `iop_order_list`, because enabled default modules do not appear in the
  history at all (PR 1841). Instance ordering was even inverted between
  versions (`multi_priority` higher means *lower* in the stack for pre-3.0
  edits, PR 3486), and history copy/paste still loses the order of extra
  instances today (#22550). Orders are per edit and never silently rewritten
  (#8634: "the pipeline order is written in DB along with editing history").
- **Evidence.**
  https://github.com/darktable-org/darktable/pull/1841,
  https://github.com/darktable-org/darktable/pull/3486,
  https://github.com/darktable-org/darktable/issues/8634,
  https://github.com/darktable-org/darktable/issues/22550,
  https://github.com/darktable-org/darktable/pull/3908
- **Helios exposure: handled (fixed 2026-10-12).** The import chained
  the effective history in last-edited order, so "exposure, sigmoid,
  exposure (tweaked)" became `sigmoid -> exposure`. `helios-xmp` now reads
  `iop_order_version` / `iop_order_list` (and the per-item order of older
  sidecars), embeds darktable's order tables (`iop_order` module, checked
  against a darktable checkout), and imports in pipe order; modules the
  order does not place are reported in `Report::unordered`. A
  `darktable-cli` parity test covers two instances run against their
  history order.
- **Recommendation.** Rebuild the chain from darktable's order: read
  `iop_order_version` / `iop_order_list` from the sidecar, ship darktable's
  generated order tables (v3.0, v5.0 "scene-referred", legacy) as data, sort
  the effective items by it, and report an unknown version instead of
  guessing. Add a test with a history that touches a module twice around
  another one, compared against `darktable-cli`.

### 2. "Absent from history" means "today's default"

- **What went wrong.** Modules that are always on (white balance,
  highlights, ...) are often missing from old histories because they were at
  default. When darktable changed a default (inpaint opposed as the
  highlights default, modern chromatic adaptation, scene-referred +0.7 EV
  exposure), old edits silently rendered differently: the missing module was
  re-added with the *new* default. The fixes were a module version bump with
  the old default kept in `legacy_params`, plus explicit insertion of the
  old-default module when it is missing (#12650, #7145, PR 8599). The
  workflow defaults also broke on their own (#19660, #7271: white balance
  applied twice).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/12650,
  https://github.com/darktable-org/darktable/issues/7145,
  https://github.com/darktable-org/darktable/issues/19660,
  https://github.com/darktable-org/darktable/issues/7271
- **Helios exposure: handled for the bench, at risk for imports.** The
  parity bench pins `workflow=none`, `chromatic-adaptation=legacy` and
  `--apply-custom-presets false` (`helios-parity`). XMP import of real edits
  has no rule yet for modules missing from the history.
- **Recommendation.** Design rule: changing a default is a version bump,
  and a migration fills in the old default (ROADMAP rule 6 already says
  migrations are explicit). For imports, encode darktable's "implicit
  modules" per darktable version and workflow (from the sidecar's
  `darktable:xmp_version` and the writer version), and report every module
  Helios inserted. Helios edits store every value explicitly, never "absent
  means default".

### 3. Whole-image statistics computed from the region the module sees

- **What went wrong.** Modules that derive data from the image (haze
  removal's haze estimate, highlight reconstruction's colour propagation,
  filmic's auto-tune, tone equalizer curves) computed it from whatever
  region and scale they received. Darkroom (zoomed, cropped, preview
  downsampled) and export (full or downscaled early) then gave different
  results; haze removal took intermediate data from the preview pipe, which
  export does not have, and was proposed for hard deprecation because "there
  is no way to guarantee consistency between darkroom preview and export".
  Caching the estimate in the params was proposed (#18501).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/18384,
  https://github.com/darktable-org/darktable/issues/17358,
  https://github.com/darktable-org/darktable/issues/18501,
  https://github.com/darktable-org/darktable/issues/3022,
  https://github.com/darktable-org/darktable/issues/2845
- **Helios exposure: at risk.** `Module::process` sees only its input
  region, and the content key includes the region (`engine.rs::plan`), so a
  module computing statistics in `process` would give region-dependent
  output with nothing catching it. The contract mentions "analysis inputs"
  (`docs/module-contract.md`) but no ported module uses them yet.
- **Recommendation.** Before porting haze removal, highlights
  (segmentation/opposed), filmic/sigmoid auto modes or the tone equalizer:
  global data must be a separate analysis node evaluated on the full extent
  at a fixed, declared scale, whose result is either a port or a recorded
  parameter (the decision-node pattern). Add a testkit check: rendering a
  sub-region equals the same sub-region of a full render, for every module.

### 4. Auto-tuned parameters that keep tracking upstream changes

- **What went wrong.** Filmic's auto-tune picker stayed live: changing any
  earlier module silently changed filmic's black and white points (#3451).
  Pickers also read the preview pipe, whose lower resolution made values
  imprecise, worse with "reduce resolution of preview image" (#18459), and
  could read misaligned areas (#9920).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/3451,
  https://github.com/darktable-org/darktable/issues/18459,
  https://github.com/darktable-org/darktable/issues/9920
- **Helios exposure: handled by design, to keep.** Decision nodes record
  their answer and asking again creates a new version (ROADMAP 2.5,
  `Graph::answer`).
- **Recommendation.** Apply the same rule to every picker or "auto"
  button: the result is written as a parameter, read from a declared node
  output at a declared scale, never recomputed implicitly.

### 5. Tiling overlap declared apart from the algorithm

- **What went wrong.** darktable's `tiling_callback` declares overlap and
  memory separately from `process`. Denoise (profiled) wavelets kept the
  legacy number of scales in its tiling code, so tiles had too little
  overlap and a band appeared at the margin, reproducible only with the
  memory settings that forced tiling (#3249, #3284, fixed by PR 3294). The
  guided filter tiles internally with a radius-dependent size and produced
  blocky masks (#14215). Demosaic had to run untiled for dual demosaic,
  details mask and green equilibration until PR 19217.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/3249,
  https://github.com/darktable-org/darktable/pull/3294,
  https://github.com/darktable-org/darktable/issues/14215,
  https://github.com/darktable-org/darktable/pull/19217
- **Helios exposure: mostly handled.** `input_region` takes the plan, so
  overlap derives from the same values as the computation (e.g. `MaskFeather`
  grows by `2 * w`), and `helios-testkit::check_contract` asserts tiled ==
  untiled bit for bit. Limits: the test region is 37x23, so a module whose
  overlap exceeds the tile covers the whole image and never really tiles;
  parameters are varied one at a time (defaults, then each float at min and
  max), not in combination.
- **Recommendation.** Make the contract test choose a region of at least
  4x the largest overlap the parameter set produces, assert that at least
  two tiles were cut inside the image, and add combined extremes (all
  radii max). Forbid hidden internal tiling in modules: tiling belongs to
  the engine.

### 6. Every input of a step must be in its cache key

- **What went wrong.** Raster masks and the details mask were passed
  between modules beside the pipe, not as part of the cached data. Cached
  steps skipped the mask writer, so later modules lost their raster mask;
  it disappeared with `--disable-pipecache` (#14271). The fixes added the
  details hash to the cacheline hash (PR 14432) and repaired raster mask
  bookkeeping (PR 14533). Skipping a module only in some pipes also had to
  be mirrored in the hash (PR 15506).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/14271,
  https://github.com/darktable-org/darktable/pull/14432,
  https://github.com/darktable-org/darktable/pull/14533,
  https://github.com/darktable-org/darktable/pull/15506
- **Helios exposure: handled.** Masks are ordinary image edges
  (`docs/masks.md`); the step key hashes module, version, canonical params,
  context restricted to declared fields, region, scale, accelerator and
  input keys (`engine.rs::plan`).
- **Recommendation.** Keep it as an invariant with a test: perturb each
  key ingredient (one param, one context field, one input pixel, scale,
  region) and assert the key changes; and conversely, an undeclared context
  field must not change the output (modules cannot read it).

### 7. Cache validity checked by heuristics, and caches keyed to the viewport

- **What went wrong.** A size check added to cache hits compared against
  a cacheline's maximum historical size, so the full pipe could never hit
  its cache on large images (#13389). Raising the number of cachelines
  brought no clear benefit because OpenCL results were not written back and
  heavy modules were not prioritized (PR 12169). Demosaic cropped and scaled
  internally, so every pan or zoom missed the cache from demosaic on; PR
  22304 (2026) moved scaling to a separate module after a full-image
  demosaic, after a first attempt was dropped (PR 22236).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/13389,
  https://github.com/darktable-org/darktable/pull/12169,
  https://github.com/darktable-org/darktable/pull/22304,
  https://github.com/darktable-org/darktable/pull/22236,
  https://github.com/darktable-org/darktable/pull/5576
- **Helios exposure: partly handled.** Hits are decided by content hash
  only, and device results are cached under the same key. But the region is
  part of every key, so a viewport that requests arbitrary regions shares
  nothing between two pans *(inference from `engine.rs`)*.
- **Recommendation.** Have the viewport tile source request a fixed tile
  grid per scale (aligned origins), so pans and zooms reuse tiles, and add a
  budget test: "pan by half a tile computes only the new tiles". Consider
  cost-aware eviction (keep expensive steps such as demosaic).

### 8. Downscaling in the wrong place or the wrong space

- **What went wrong.** darktable downscales early for speed (preview
  downsampling, non-HQ export scaling at demosaic). Gamma-encoded inputs
  (JPEG, PNG) were rescaled before linearization, giving the classic gamma
  scaling error (#8410, #13335); non-HQ exports and the darkroom differed in
  colour because later modules saw a downscaled image (#17758); zoomed-out
  previews showed jagged lines, partly from Lab-space modules (#8016). A
  movable final-scale module was proposed and rejected for lack of proof
  (PR 13635).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/8410,
  https://github.com/darktable-org/darktable/issues/13335,
  https://github.com/darktable-org/darktable/issues/17758,
  https://github.com/darktable-org/darktable/issues/8016,
  https://github.com/darktable-org/darktable/pull/13635
- **Helios exposure: handled for loaded pictures (tested).**
  `Engine::render_at` reduces *sources* by a box mean in their own encoding
  (`Image::sample_at`) before any module runs, but `helios-load` removes the
  sRGB curve when it decodes a picture, so the mean is taken in linear
  light: a one-pixel black and white checkerboard at half scale gives
  linear 0.5, not the encoded mean (test `a_picture_is_reduced_in_linear_light`
  in `helios-load`). The engine does not know a source's encoding: a source
  built by hand from encoded values would still be averaged wrongly, so
  sources are linear by contract.
- **Recommendation.** Declare each source's encoding; reduce only linear
  data, or linearize first. Add a test with a 1-pixel black/white
  checkerboard in sRGB: half-scale render must equal the linear mean.

### 9. Downscaling a sensor mosaic

- **What went wrong.** darktable never box-averages a CFA: demosaic does
  the reduction itself (half-size passthrough, `clip_and_zoom_demosaic_*`),
  which coupled cropping and scaling with demosaicing and shaped all of
  items 5 and 7 (PR 19217, PR 22304). PR 22304 also notes that demosaicing
  only the requested region gave "inferior interpolators at the borders"
  visible when zooming in.
- **Evidence.**
  https://github.com/darktable-org/darktable/pull/22304,
  https://github.com/darktable-org/darktable/pull/19217,
  https://github.com/darktable-org/darktable/pull/4931
- **Helios exposure: real (tested).** The engine reduces the raw source at
  scale < 1 before `demosaic`, which does not use the scale; a 2x2 mean of a
  Bayer mosaic mixes R, G and B photosites, which `demosaic` then reads as a
  mosaic. Test `a_half_scale_render_mixes_photosites_before_demosaic` in
  `helios-module-demosaic`: a flat (0.8, 0.5, 0.2) scene through RGGB comes
  out grey (0.5, the mean of the four photosites) at half scale, while the
  full render reduced keeps its colour.
- **Recommendation.** Let a module declare that an input must be at full
  resolution (a "no reduction upstream" flag), set it on `demosaic` (and
  `rawprepare`, `highlights` in raw space), and scale after demosaic, as
  darktable 2026 does. Test: half-scale render of a raw close to the full
  render reduced (the contract already asks this for scale-dependent
  modules).

### 10. Drawn masks live in raw coordinates and cross every distortion

- **What went wrong.** Shapes are stored in input-image coordinates and
  pushed through each distorting module's `distort_transform` /
  `distort_backtransform` / `distort_mask`. Any missing or wrong transform
  misplaces masks: rawprepare's raw crop (#3904), crop at fixed ratio when
  zoomed (#19135, still open-ended: "a general problem with wrongly
  calculated positions"). Lens correction curves straight gradients; a
  "viewport space" mode was proposed and closed (PR 8673); samples were made
  distortion-aware (PR 8682).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/3904,
  https://github.com/darktable-org/darktable/issues/19135,
  https://github.com/darktable-org/darktable/issues/8634,
  https://github.com/darktable-org/darktable/pull/8682,
  https://github.com/darktable-org/darktable/pull/8673
- **Helios exposure: not yet, at risk when distortions are ported.**
  `modules/masks` has no drawn shapes yet and no distorting module exists.
- **Recommendation.** Before the first geometric module (crop, flip, lens,
  liquify), add to the contract a point transform (forward and back) with a
  round-trip property test, and decide which coordinate space shapes are
  stored in (darktable parity requires raw space). Every distorting module
  must implement both directions; the testkit checks they invert each other.

### 11. Integer regions from float scales

- **What went wrong.** ROIs are rounded to integers repeatedly at
  different scales. Crop at 1/4 preview downsampling could not reach the
  top-left corner; Aurélien Pierre suggested "integer rationals everywhere,
  instead of rounding floats to integers multiple times" (#5306). At some
  export sizes the left column was copied to the right (#12386). Mask
  offsets came from `round_smart()` mismatches (#3904).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/5306,
  https://github.com/darktable-org/darktable/issues/12386,
  https://github.com/darktable-org/darktable/issues/3904
- **Helios exposure: at risk.** Scale is an `f64` in the key and in
  `prepare_at` (radii `.round()`), and `sample_at` floors spans.
- **Recommendation.** Represent scale as a rational (or a power of two for
  the proxy), state one rounding rule in `Region`, and property-test
  `extent(scale)` and pixel mapping over many sizes (odd widths, primes).

### 12. Scale-dependent modules and their scaling rule

- **What went wrong.** Neighbourhood modules must convert sizes to the
  render scale; darktable's rules vary per module (the feather radius takes
  the square root of the scale, per `modules/masks/src/refine.rs`), and when
  they are approximate the zoomed darkroom does not match export (#8016,
  #3022). Sharpening before the final downscale was a known complaint behind
  PR 13635.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/8016,
  https://github.com/darktable-org/darktable/issues/3022,
  https://github.com/darktable-org/darktable/pull/13635
- **Helios exposure: partly handled.** `prepare_at` plus the "half scale
  close to full render reduced" contract check. `MaskFeather` scales its
  radius linearly while darktable uses the square root: a deliberate
  difference, documented only in a code comment.
- **Recommendation.** Record such deviations in one parity table
  (module, darktable rule, Helios rule, expected Delta E at proxy scales)
  so they are not "fixed" back by a later porter. Keep exact parity at
  scale 1 only.

### 13. Legacy parameter conversion is fragile

- **What went wrong.** Old filmic params went through
  `convert_to_spline_v3`, which wrote NaN or 0 latitude depending on the
  compiler: inlining under GCC 13.2 broke it, and adding a `printf` made it
  vanish (#15489, #17420, fixed by PR 17437 "don't inline"). Params are raw
  C struct bytes, so a version step can copy bytes into the wrong field;
  darktable later reworked legacy support for maintenance (#14608).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/15489,
  https://github.com/darktable-org/darktable/issues/17420,
  https://github.com/darktable-org/darktable/pull/17437,
  https://github.com/darktable-org/darktable/issues/14608
- **Helios exposure: partly handled.** `helios-xmp/src/legacy.rs`
  reproduces darktable's steps byte for byte (including the highlights v3
  to v4 quirk) and tests them against darktable's own conversions, but only
  for four modules.
- **Recommendation.** After every migration, validate values against the
  manifest ranges and report out-of-range or non-finite results instead of
  rendering them; build the expected outputs from a release build of
  darktable (the bug above only showed in some builds).

### 14. NaN and infinity spread downstream

- **What went wrong.** NaNs from OpenCL RCD crashed surface blur
  downstream (#10778, PR 10841); highlight reconstruction had to be fixed to
  "make sure we don't write NaNs to output" (PR 12931); the tone equalizer
  cursor showed "+nan EV" (#6572, title only). Norms of unbounded RGB with
  negatives made filmic + base curve black until an epsilon was added
  (#3865).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/10778,
  https://github.com/darktable-org/darktable/pull/12931,
  https://github.com/darktable-org/darktable/issues/3865,
  https://github.com/darktable-org/darktable/issues/6572
- **Helios exposure: partly handled.** The contract test asserts finite
  outputs at defaults, mins and maxes, but on the testkit's synthetic input,
  not on negatives, zeros, huge values or a NaN pixel.
- **Recommendation.** Add adversarial inputs to `check_contract`
  (negative channels, all-zero, 1e6, one NaN pixel) and state per module
  whether NaN in means NaN out or is repaired. Add an optional engine guard
  (debug builds, tests) that names the first step producing a non-finite
  value.

### 15. Unbounded scene-referred values: no clamping, but who handles negatives

- **What went wrong.** darktable keeps unbounded linear RGB on purpose and
  refused to clamp negatives in camera RGB ("clamping is a non-linear
  operation that destroys data", #4843), but several modules then broke on
  negatives or zeros (#3865 black image, #9076 channel mixer black patch,
  title only).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/4843,
  https://github.com/darktable-org/darktable/issues/3865,
  https://github.com/darktable-org/darktable/issues/9076
- **Helios exposure: at risk.** Ports carry `f32` with no declared domain.
- **Recommendation.** Declare a value domain per port (unbounded, >= 0,
  [0, 1]) in the manifest; the testkit feeds values outside the declared
  domain of the upstream port to check robustness.

### 16. Working colour space per module and placement in the pipe

- **What went wrong.** Lab modules placed before input colour profile
  (defringe) "do not do what is advertised" because there is no Lab before
  colorin (#8634); Lab-space modules contributed to zoomed-out artefacts
  (#8016); multiple tone mappers are allowed and a warning was refused
  (#22237).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/8634,
  https://github.com/darktable-org/darktable/issues/8016,
  https://github.com/darktable-org/darktable/issues/22237
- **Helios exposure: partly handled.** Ports have a layout; the parity
  bench isolates one module between linear Rec.2020 conversions.
- **Recommendation.** Declare the expected colour encoding (raw, camera
  RGB, linear working RGB, Lab, display) on ports, let the graph refuse or
  insert explicit conversions, and keep darktable's order as the default
  network only.

### 17. Partitioning that assumes the thread count

- **What went wrong.** Exports without OpenCL had a black band at the
  bottom: OpenCV, linked in, called `omp_set_dynamic`, so fewer OpenMP
  threads ran than darktable assumed and the last slices were never
  computed (#18127, 58 comments).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/18127
- **Helios exposure: at risk if modules split work by hand.** Modules have
  no threads of their own (module contract); the scheduler owns workers.
- **Recommendation.** Never derive work slices from a requested thread
  count; slice by data (rows, tiles) and let the pool schedule. Test: render
  with 1 worker and with N workers gives the same bits.

### 18. CPU and GPU paths diverge

- **What went wrong.** The same kernel gave amplified local contrast on
  one AMD driver out of four (#3756), corrupted previews (#7544), blocky
  feathered masks only with OpenCL (#15621); a new module was merged with a
  known CPU/GPU difference (#22389). darktable's integration tests compare
  CPU and GPU in Delta E.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/3756,
  https://github.com/darktable-org/darktable/issues/7544,
  https://github.com/darktable-org/darktable/issues/15621,
  https://github.com/darktable-org/darktable/issues/22389
- **Helios exposure: handled.** `helios-gpu` tests each GPU variant against
  the CPU reference within a tolerance, and accelerated steps get their own
  cache key.
- **Recommendation.** Keep the CPU path as the reference, run the backend
  suite on at least two adapters (one software, such as lavapipe, in CI),
  and report the tolerance per module.

### 19. Mask combination semantics drift from the documentation

- **What went wrong.** "Inclusive" combination of drawn and parametric
  masks did not match the documented maths; the maintainer confirmed it
  differs from the doc while matching older versions (#8172). History
  compression detached masks from their module and left thousands of
  undeletable shapes, slowing old edits (#6708).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/8172,
  https://github.com/darktable-org/darktable/issues/6708,
  https://github.com/darktable-org/darktable/issues/21905
- **Helios exposure: partly handled.** `mask_combine` follows darktable's
  data model (PR 22553, `docs/masks.md`).
- **Recommendation.** Port operators from darktable's code, not from its
  manual, with one parity test per operator and polarity; when code and
  doc disagree, note it in `docs/masks.md`. Vault rule: shapes are owned by
  the edit version, never orphaned by compaction.

### 20. Edit identity from hashing raw parameters

- **What went wrong.** The "altered" flag was broken and a history hash
  was added (#4313, PR 4322); hashing `op_params` is not stable because a
  legacy upgrade changes the bytes without changing the image.
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/4313,
  https://github.com/darktable-org/darktable/pull/4322
- **Helios exposure: handled for the cache, at risk for "altered"
  states.** Keys hash `manifest.version` and canonical params, which is
  right for caching; it would be wrong as an "edit changed" signal.
- **Recommendation.** Keep "edit changed" on the version DAG (a new
  version exists), not on content keys.

### 21. Precision of the final encoding

- **What went wrong.** Exporting to linear Rec.2020 at 8 bits banded
  skies; the answer was "use a non-linear profile" (#14793).
- **Evidence.**
  https://github.com/darktable-org/darktable/issues/14793
- **Helios exposure: at risk in `helios-export`** (not read in detail).
- **Recommendation.** Refuse or warn on linear transfer functions below 16
  bits; consider dithering on 8-bit output; test a smooth gradient for
  banding (count of distinct levels).

---

## Top 5 for Helios now

1. **Fix XMP import order** (pitfall 1): `Sidecar::effective()` plus
   `Imported::edit()` chain modules in last-edited order; read
   `iop_order_version` / `iop_order_list` and sort by darktable's pipe
   order, with a `darktable-cli` test.
2. **Stop reducing raws and gamma-encoded sources before they are linear
   and demosaiced** (pitfalls 8, 9): a "full-resolution input" declaration
   for demosaic and friends, and linear reduction of sources.
3. **Define global analysis before porting HLR, haze, filmic/sigmoid
   auto, tone equalizer** (pitfalls 3, 4): an analysis node on the full
   extent at a fixed scale, results recorded as parameters; testkit check
   "sub-region of full render == render of sub-region".
4. **Defaults policy** (pitfall 2): a default change is a version bump
   with a migration; import fills in darktable's implicit modules per
   version and reports them.
5. **Harden the contract tests** (pitfalls 5, 14, 15): regions large
   enough to really tile at maximum overlap, combined extreme params, and
   adversarial inputs (negatives, zeros, huge values, NaN).
