# Masks

Status: **first step** (2026-10-11). How darktable's flexi masks map onto a
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

## From darktable's tree to a sub-network

- **Member** → a node with a one-channel output (`mask_from_luma` today;
  drawn shapes, parametric channels, raster and AI masks later), connected
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

Not ported yet: the details threshold (it needs darktable's Scharr detail
mask, an analysis of the raw), drawn shapes, parametric, raster and AI
members, and the preview bypass of a refinement.

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
