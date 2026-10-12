# Masks

Status: **third step** (2026-10-12): groups, refinement, parametric,
raster and drawn shape members (circle, ellipse, gradient, path), and a
stress network with its bench. How darktable's flexi masks map onto a
Helios network, and what porting them taught about the core. The modules
live in `modules/masks` (crate `helios-module-masks`).

## Why masks are a crash test

darktable's flexi masks (data model in darktable's
`dev-doc/masks_data_model.md`, pull request darktable-org/darktable#22553)
give a module's mask as a tree of groups. Each group folds its visible
members in list order with one operator, then applies its refinement, then
its inversion, then its opacity. A member (a drawn shape, a parametric
channel, a raster mask, an AI object or another group) yields 0 to 1 per
pixel and carries its own opacity, inversion and element-scope refinement.

In darktable all of this runs inside one module's blend step. Helios has no
blend step: everything is a node. Porting the masks as a sub-network checks
that the contract (ports, parameters, regions, scale, cache) can carry a
real, nested, order-dependent structure without a side channel.

## The convention: a mask is a one-channel image

A mask is an ordinary `Image` with one channel, values 0 (the module does
nothing) to 1 (it applies fully). Mask ports are image ports with one
channel and layout `y` (`helios_module_masks::mask_port`), so they connect
only to one-channel outputs, and they tile, scale and cache like any image.
Values are not clamped on every port: operators clamp where darktable does
(minimum, sum, the blend).

## Modules

| module | ports | what it does |
|---|---|---|
| `mask_combine` | `in0`..`in7` (optional masks) → `mask` | a group: fold, group inversion, group opacity |
| `mask_feather` | `mask`, `guide` (optional, RGBA) → `mask` | guided-filter feathering; self-guided without a guide |
| `mask_blur` | `mask` → `mask` | Gaussian blur, radius in full-resolution pixels |
| `mask_levels` | `mask` → `mask` | contrast and brightness (darktable's tone curve), then inversion, then opacity |
| `mask_blend` | `input`, `processed` (RGBA), `mask` → `output` | `input + (processed - input) * mask * opacity` |
| `mask_from_luma` | `input` → `output`, `mask` | a member from a luminance range; passes the image through |
| `mask_parametric` | `input` → `output`, `mask` | a member from channel ranges (darktable's parametric mask); passes the image through |
| `mask_raster` | `mask` (optional) → `mask` | reuses another module's mask as a member (darktable's raster mask); 0 when unconnected |
| `mask_circle` | → `mask` | a drawn circle: centre, radius, feather |
| `mask_ellipse` | → `mask` | a drawn ellipse: centre, half-axes, rotation, feather (equidistant or proportional) |
| `mask_gradient` | → `mask` | a drawn linear gradient: a point of its line, rotation, transition width, linear or sigmoid |
| `mask_path` | → `mask` | a drawn closed path: up to 64 points with Bézier handles (a polygon when handles sit on their corners), feather |

## From darktable's tree to a sub-network

- **Member** → a node with a one-channel output (`mask_from_luma`,
  `mask_parametric`, `mask_raster`, `mask_circle`, `mask_ellipse`,
  `mask_gradient`, `mask_path` today; AI masks later), connected
  to a port of its group. Its opacity and inversion are the group's
  `opacity[i]` and `invert[i]` for that port: inversion first, then
  opacity, as darktable does.
- **Element-scope refinement** → refinement nodes on the member's branch,
  before the group's port.
- **Group** → a `mask_combine`. Its operator is `operator`; its members are
  its connected ports, in port order (the list order). A hidden or
  disabled member is a port left unconnected.
- **Group-scope refinement** → the group's own inversion and opacity left
  neutral on `mask_combine`, then the refinement nodes, closed by a
  `mask_levels` holding the group's contrast, brightness, inversion and
  opacity. The order fold → refinement → inversion → opacity is kept.
  darktable's choice of feathering before or after the blur is the order of
  the two nodes.
- **Nested group** → a `mask_combine` whose `mask` feeds a port of its
  parent.
- **The module's mask** → the root group's output into `mask_blend`, with
  the module's input and output.

```text
image -> mask_from_luma -mask-> mask_feather (guide: image) -> in0 \
shape -> mask_blur ------------------------------------------> in1  > mask_combine (screen)
mask_combine -> mask_blur -> mask_levels (group refinement) ----------> mask      \
image ----------------------------------------------------------------> input      > mask_blend
image -> the module --------------------------------------------------> processed /
```

## Operators

Members fold in port order, each `m = opacity · (invert ? 1 − v : v)`,
into `d`, which starts at 1 for minimum and product and at 0 otherwise.

| operator | step | darktable |
|---|---|---|
| `maximum` (default) | `max(d, m)` | union, flexi default |
| `screen` | `d + m − d·m` | `FLEXI_SCREEN` |
| `minimum` | `min(max(d, 0), max(m, 0))` | `FLEXI_MINIMUM` |
| `product` | `d · m` | `FLEXI_PRODUCT` |
| `sum` | `min(1, d + m)` | `FLEXI_SUM` |
| `difference` | first member copied, then `d · (1 − m)` where both are positive | `FLEXI_DIFFERENCE` |
| `exclusion` | `max((1 − d)·m, d·(1 − m))` where both are positive, else `max(d, m)` | `FLEXI_EXCLUSION`, classic's exclusion |

Exclusion is not associative: (0.6, 0.5, 0.9) gives 0.63, (0.9, 0.5, 0.6)
gives 0.33; the tests check both. A group with no connected member gives 1
(darktable's "no active mask element": the module applies fully).

## Drawn shapes

The four shape members read no image: each is a source of one channel,
`(invert ? 1 − v : v) · opacity` where `v` is the shape's value, like
every member. Their parameters are ordinary module parameters, so they
live in the value store (`node/<node>/<param>`): edits are undoable,
announced on the bus and saved with the network like any other value.

- **Image coordinates.** Positions and lengths are full-resolution pixels
  of the image the network reads (`x` from the left, `y` from the top;
  pixel `(i, j)` covers `[i, i + 1) × [j, j + 1)`). A render at scale `s`
  evaluates its pixel `(i, j)` at `((i + 0.5) / s, (j + 0.5) / s)`, so a
  shape stays where it was drawn at every zoom, crop and tile.
  Rotations are degrees from the x axis toward the y axis (clockwise on
  screen).
- **Falloff.** Circle and ellipse use darktable's quadratic falloff along
  each ray from the centre, `((t² − l²) / (t² − r²))²` between the edge
  `r` and the feather's end `t`; an ellipse's feather is either one width
  (`equidistant`, darktable's default) or `proportional` to each half-axis
  (`feather` is then its width along the longer one). The path falls off
  linearly with the distance outside it (darktable's path falloff), inside
  by the even-odd rule. The gradient goes from 0 to 1 across its line over
  `feather`, linearly or along an error function (darktable's sigmoid,
  whose `compression` is `feather / (2 · diagonal)`); rotation 0 is full
  above the line, 90 full left of it, as in darktable.
- **Anti-aliasing.** Each pixel is the mean of the profile over the
  pixel's width across the edge (a box filter along the distance, in
  closed form by Gauss-Legendre quadrature, exact for the polynomial
  profiles): a hard edge gets the pixel's coverage, so a hard circle's
  mask sums to its area within 0.2 %.
- **Paths.** A path's Bézier segments are cut into straight pieces within
  0.1 image pixel of the curve, independently of the render's scale (a
  preview draws the same polygon), at most 31 pieces a segment.
- **Extent.** Each shape but the gradient declares its bounding box plus
  its feather and half a render pixel as its extent (see
  `docs/module-contract.md`): a tile outside it is filled with the
  member's constant (0, or its opacity when inverted) without running the
  module. Inside, a far pixel's window lies on one side of the profile and
  gives the constant exactly, so filled and computed tiles agree bit for
  bit.
- **Tiles.** Each pixel depends on its own centre only (a path's rows
  find crossings and nearby sides per row, giving exactly what reading
  every side gives), so a tiled render equals the untiled one bit for
  bit; the tests check it for every shape at scales 1, 0.5, 0.37 and 0.25
  with tiles of 7, 32 and 64 pixels, and the group operators over shapes,
  tiled.
- **GPU.** Each shape has a WGSL kernel (`shapes.wgsl`: circle, ellipse,
  gradient; `path.wgsl`), run over the engine's two-dimensional grid as a
  module without inputs (it reads a one-pixel placeholder and learns its
  output's origin from the shape binding). The path kernel reads the
  polygon the CPU plan flattened (at most 64 × 31 points, in a 16 KiB
  uniform buffer) and every side for every pixel. GPU (`f32`) and CPU
  (`f64`) agree within 2·10⁻³ (`shape::GPU_TOLERANCE`), tested at several
  scales and regions.

Not ported: brushes, darktable's per-point path feather (one feather per
path here), the gradient's curvature, and clone shapes (retouch). The
viewport's handles that draw and move shapes are a separate step; what
they read and write is listed in the module's documentation
(`helios_module_masks::shape`).

## From darktable's drawn masks

`helios_xmp::drawn` reads a sidecar's `darktable:masks_history` (each
form's points are its C structs, hex or `gz` encoded like history params)
and maps each form onto a member, given a `Frame`: the size of the image
darktable's pipe starts from and where Helios's image starts in it (the
loader's crop). It reads masks versions 4 to 6 (darktable's current is 6).

| darktable form | Helios | carried over | lost |
|---|---|---|---|
| circle | `mask_circle` | centre, radius, feather (by the smaller side) | nothing |
| ellipse | `mask_ellipse` | centre, half-axes, rotation, feather, equidistant or proportional | nothing |
| gradient | `mask_gradient` | anchor, rotation, compression (`feather = 2 · compression · diagonal`), linear or sigmoid | the curvature (reported when not 0); `steepness`, which darktable does not use |
| path | `mask_path` | corners and both handles of up to 64 points | the feather per point: one mean feather (reported when they differ by more than 1 %); paths of more points are skipped |
| group | `mask_combine` | members in order, each one's inversion, opacity and operator (union and sum → `maximum`, intersection → `minimum`, difference, exclusion) | a group whose later members use different operators is a chain of two-member groups, which the importer builds |
| brush, clone, AI object | skipped, with why | | |

Every member lists its losses and every skipped form says why. Two
approximations hold for all: darktable computes a shape on a coarse grid
(up to 4 pixels) and interpolates, Helios computes every pixel; and
darktable distorts a shape through the modules before it (lens, crop,
rotation), which Helios does not do yet. Wiring the members into a
module's mask needs the module's blend parameters (its `mask_id`), not
decoded yet: the importer (`helios_xmp::import`) does not call this
mapping yet.

## Refinement

- `mask_blur`: Gaussian, sigma = radius × render scale, reach 3 sigma.
- `mask_feather`: guided filter (He, Sun and Tang), window radius =
  radius × render scale, two box means, so it reads twice the window
  around a tile. The guide is the luminance of the guide image × 10, with
  `eps` = 0.25, darktable's settings for scene-referred RGB. Differences
  from darktable: a luminance guide instead of a colour one (a 3×3
  covariance), and the window follows the scale linearly where darktable
  takes its square root, so a preview looks like the full render reduced.
- `mask_levels`: darktable's mask tone curve with an opacity of 1, skipped
  as there when contrast and brightness are both under 0.01.

Not ported yet:

- brushes, curved gradients, per-point path feathers and AI object
  members;
- the details threshold (it needs darktable's Scharr detail mask, an
  analysis of the raw);
- the parametric mask's display-referred spaces: darktable's Lab and RGB
  (display) blendif, with their HSL channels (`mask_parametric` covers
  the scene-referred RGB space: luminance, RGB and JzCzhz);
- the preview bypass of a refinement;
- the raster mask of a module disabled in the history (darktable keeps the
  last one computed; here the source is either connected or not).

## Stress network and bench

`helios_module_masks::stress` builds a chain of masked modules, each with
18 mask nodes (three groups using all seven operators, element and group
refinement, parametric, luminance and raster members, the raster member
reusing the previous module's mask), an exposure as the processed module,
and a blend. Three modules make 54 mask nodes. Its tests check that a
tiled render equals the full render bit for bit (output and each module's
mask) and that a one-value edit recomputes only the dirty branch: the
seven steps from the edited member to the output, seen in the bus's
`engine.step_computed` events and the engine's counters.

`cargo xtask bench masks` renders it once per mode, each in its own
release process: `full` (one render), `tiled` (256-pixel tiles), `proxy`
(scale 0.25) and `edit` (a render after one value changed), and appends
one JSON line per mode to `target/bench/masks.jsonl`. Three modules at
1536×1024, on a loaded machine (times are indicative):

| mode | time | peak memory | computed / asked | keys planned |
|---|---|---|---|---|
| full | 44 s | 3.9 GB | 386 / 533 | 386 |
| tiled | 48 s | 4.7 GB | 9264 / 12792 | 9264 |
| proxy | 3.1 s | 0.19 GB | 259 / 365 | 259 |
| edit | 2.0 s | 3.9 GB | 7 / 14 | 386 |

With union planning (finding 14), the same machine and load, before and
after on one run each:

| mode | time before | time after | peak before | peak after | computed before | computed after |
|---|---|---|---|---|---|---|
| full | 62.8 s | 7.0 s | 3.9 GB | 0.61 GB | 386 | 58 |
| tiled | 73.1 s | 8.2 s | 4.7 GB | 0.69 GB | 9264 | 1392 |
| proxy | 1.5 s | 0.32 s | 0.19 GB | 0.07 GB | 259 | 58 |
| edit | 1.4 s | 0.89 s | 4.0 GB | 0.66 GB | 7 | 7 |

A full render now computes each of the 58 nodes once; tiles compute each
node once per tile (24 × 58).

Under a cache budget (`--budget MIB`, finding 15), same machine:

| mode | no budget | 256 MiB | 64 MiB |
|---|---|---|---|
| full: peak, evictions | 0.61 GB, 0 | 0.40 GB, 45 | 0.29 GB, 55 |
| tiled: peak, evictions | 0.69 GB, 0 | 0.19 GB, 1101 | 0.09 GB, 1315 |
| proxy: peak | 0.07 GB | 0.07 GB | 0.07 GB |
| edit: computed | 7 | 11 | 56 |

Computed steps do not move in a single render: what a render still needs
is never evicted. The peak includes about 60 MB the process holds without
a cache (the picture, the network). Tiles stay close to the budget; one
full render does not, since its working set (each module's input, read by
nine nodes, and the masks waiting for their group) is larger than 64 MiB
and is pinned until read. An edit after a render under a small budget
recomputes what the budget evicted.

Planning before and after the memo (finding 12), one full render at 64×48:

| modules | keys before | keys after | time before | time after | peak before | peak after |
|---|---|---|---|---|---|---|
| 3 | 3009 | 386 | 523 ms | 471 ms | 23 MB | 22 MB |
| 4 | 29497 | 874 | 1573 ms | 1567 ms | 66 MB | 54 MB |
| 5 | 288969 | 1609 | 4249 ms | 3162 ms | 235 MB | 113 MB |
| 6 | 2830713 | 2591 | 18687 ms | 6746 ms | 1406 MB | 210 MB |

## Crash-test findings

What the port needed from the core, or shows it lacks:

1. **Variable arity.** The core had no way to leave an input unconnected:
   rendering failed with `Unconnected`. Added (additive): an input may be
   declared `optional`; unconnected, the module gets an empty image at its
   place (`Image::absent`, `Image::is_absent`) so inputs keep their port
   index, and the content key marks which ports are empty. A group still
   has a fixed maximum (8 members); a true variable arity (ports created
   on connection) would need the manifest to depend on the node, not the
   module. darktable nests at most 8 deep and puts no limit on members, so
   a bigger group is two groups today (only exact for associative
   operators: maximum, screen, minimum, product, sum).
2. **Per-member parameters are parallel arrays.** Opacity and inversion are
   `Floats` and `Ints` of length 8, indexed by port. They drift from the
   wiring (a member moved to another port keeps the wrong opacity), there
   is no list of bools, and an editor cannot show "the opacity of the
   member on `in3`". Parameters attached to a connection (an edge's own
   parameters) would fit darktable's model better.
3. **Order is the port order.** List order is part of the result
   (exclusion, difference), so reordering members is rewiring, which a
   surface must present as a reorder, not as two edits.
4. **Refinement order vs one node.** fold → refinement → inversion →
   opacity cannot be one `mask_combine` when refinement is delegated to
   separate modules; the group's tail moves to `mask_levels`. Two places
   hold "a group's inversion" depending on whether it is refined. A macro
   node (a network packaged as one module, the contract's composition)
   would hide this.
5. **No "skip me" output.** darktable skips an empty nested group in its
   parent; here an empty group outputs 1, which a parent folding with
   maximum takes as "everything". A surface must disconnect empty groups.
   An output that may be absent would mirror optional inputs.
6. **Channel count is fixed per port.** The existing `blur` takes 4
   channels only, so `mask_blur` repeats it for one channel. `AnyImage`
   ports exist in the manifest but the engine does not run them yet; with
   them one blur would serve images and masks.
7. **One input region for all inputs.** The feather's guide is read over
   the same region as the mask, which is what it needs; a module wanting
   different reaches per input would read more than it needs.
8. **Unused inputs are still computed.** The engine plans and runs every
   connected input before calling the module, so a member with opacity 0
   is still rendered. Hiding a member must disconnect it.
9. **Cache cost of many small nodes.** Every node caches its full output
   for each region and scale: a group of 8 members with element refinement
   holds about 25 one-channel buffers per tile, each a cache entry with its
   key, against one scratch buffer in darktable. One-channel buffers keep
   the bytes small (a quarter of an RGBA image), but the entry count grows
   with the tree; fusing pixel-wise chains (combine, levels, blend) into one
   step would cut both.
10. **Typed parameter access.** `Params` has a typed accessor for floats
    only; bools, choices and arrays are matched by hand in each module.
11. **GPU path.** The accelerator ran any module with a kernel and one
    input; a module whose input is optional is now left to the CPU, since
    the input may arrive empty. No mask module has a GPU variant yet.
12. **Planning was exponential in shared inputs.** The engine planned the
    graph as a tree: a node read by several consumers was planned again on
    every path, and a masked module reads its input from nine places, so
    each chained module multiplied planning by about ten (2.8 million keys
    for six modules). Fixed: planned steps are kept per node and region
    within one render and shared. Keys now equal the distinct steps.
13. **The cycle check was exponential too.** `Graph::connect` walked every
    path upstream of the new input; it now visits each node once.
14. **Every distinct region was a distinct step (fixed).** Steps and the
    cache were keyed by the exact region. Consumers with different reaches (a blur, a
    feather, a raster member reading another module's mask) ask the same
    node for different regions, and each one is computed apart: the stress
    network's 58 nodes take 386 steps for one full render, the source alone
    25, and the pixels computed are about 17 times one pass over the image.
    This is most of the full render's time and memory now. Planning each
    node once over the union of what its consumers read, and handing each a
    crop, would bring it close to one pass; it changes what a step is (its
    region is no longer the one asked for), so it is a decision of its own.
    Decided and done (2026-10-12): planning gathers, consumers first, the
    regions each node is read over and computes the node once over their
    union, each consumer reading a crop (a view into the same buffer, no
    copy). Reads far apart stay apart: two regions merge when their
    bounding box has no more pixels than the two apart, so a merge never
    computes more than it saves. The stress network never reaches that
    limit (its reads all overlap); it is a cost rule, not a tuned number,
    and a network with reads far apart (a lens correction, a retouch
    source) would tell whether a looser one pays. A step's key is its
    lineage (what it is made of, no region) and its region, so content
    asked over a region inside cached content of the same lineage is
    cropped from it: a tile after a full render computes nothing.
15. **Memory had no ceiling by default (fixed).** Without a budget the
    cache keeps every step's output: 3.9 GB for one full render of the
    stress network at 1536×1024, 4.7 GB in tiles (each tile's halos are
    steps of their own). Now the cache's budget is a share of the
    machine's memory by default (25 %, `system.cache.share`, or a size in
    `system.cache.budget`), least recently used content is evicted, and a
    render pins its steps' content until every step reading it has run,
    so nothing is computed twice within a render. The bench still runs
    without a budget by default, to measure what the network holds;
    `--budget` sets one. The budget bounds what stays between renders; a
    render's own working set can exceed it (see the table above), which
    tiles keep small.
16. **Tiles did not save work.** In 256-pixel tiles the network computed
    9264 steps (24 tiles, 386 each): halos grow with every blur and
    feather along the chain, and finding 14 multiplied them. With union
    planning a tile computes each node once (1392 steps); halos remain. Tiles bound a
    render's working set only once the cache has a budget.
