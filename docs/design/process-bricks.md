# Process bricks: the functions modules are made of

Status: **study accepted** (2026-11-10). The four choices are decided in
§9; where a decision changes a proposal above, §9 wins.

**The request.** Process bricks are the sub-layer of the box (node)
diagram: the functions modules provide, in effect the public SDK. A
module should be an **assembly** of registered process bricks and
controls, so people, plugins or an AI can build a module from a toolbox
of described bricks plus the controls catalogue
([controls.md](controls.md)), instead of writing a `process()` in Rust.

**Terms used below.**

- A **brick** is a registered, described, pure image function: typed
  inputs, typed outputs, parameters. It has no card, no controls and no
  place of its own in an edit's history.
- A **module** is what a person adds to an image and sees in the Modules
  panel and the graph: a node with a header (family, role), a card of
  controls, a version recorded in the edit.
- An **assembly** is a module described as data: a small graph of bricks
  plus its card of controls, with the module's parameters patched to the
  bricks' parameters.
- A **sealed module** is a module written in Rust as today, with its own
  `process()`. Sealed modules stay allowed.
- **Kernel**, **composite** and **macro** keep the meaning of
  [module-contract.md](../module-contract.md), "Composites".

## 1. What exists

Read on 2026-11-10 on `next`.

| Piece | Where | What it gives bricks |
|---|---|---|
| Module contract | `helios_core::Module`, `Manifest` | a manifest as data (params, `PortSpec`s, context, `Needs`, kind, family, role); `prepare_at`, `input_region`, `output_extent`, `extent`, `process`; an optional WGSL variant. Region and union planning, the content-keyed cache, bypass and GPU residency are built. |
| Composites and macros | `helios_core::network`, `Macro::expose` | a node expanded at planning into inner modules, each its own step, key and cache entry; inner parameters exposed one to one, one name linking several. |
| Bricks in all but name | `modules/vkdt/src/guided.rs`, `modules/masks` | `vkdt_guided1..3`, `vkdt_blur`, `vkdt_blurs`, `vkdt_resample` wired by `guided_filter()` into `vkdt_contrast`; the masks crate's small modules (`mask_parametric`, `mask_combine`, `mask_feather`, `mask_blend`...) rebuild darktable's blending as a sub-network. |
| Composite files | `helios-template`, `CompositeFile`, `Catalogue` | a network as versioned JSON (`nodes`, `connections`, `exits`, `exposed`, `workshop`), checked against the modules known by name. |
| Registry and MCP | `helios_node::registry()`, `helios_describe` (entity `modules`) | a flat map of names to modules, readable by agents. |
| Controls | egui_hud (its own repository), `crates/helios-controls` | controls as data, `Patch { module, param, control, jack }`, implicit controls from descriptors, `controls.catalogue` and `controls.describe`. |
| Test kit | `helios_testkit::assert_contract_of` | defaults, range ends, adversarial inputs, tiled equals untiled, half scale; on composites too. |
| Plugins | `crates/helios-plugin`, [extensions.md](../extensions.md) | out of process, JSON-RPC over pipes, manifest first, capabilities brokered or disclosed; official ones in Rust under `plugins/<category>/<name>`. None contributes a module yet. |

So most of the machinery is there. What is missing is the layer's
**identity**: bricks are not told apart from modules, they have no
namespace or stable SDK promise, ports carry only images, module
parameters reach inner parameters only one to one (no derivation), and a
composite file cannot carry the card of controls a module needs.

## 2. Survey

### 2.1 What Helios modules do inside

A representative set: the darktable ports (`rawprepare`, `whitebalance`,
`highlights`, `demosaic`, `colorin`, `channelmixerrgb`, `exposure`,
`sigmoid`, `flip`, `blur`), the masks sub-network, and the vkdt ports
(`colour`, `grade`, `filmcurv`, `usm`, `contrast`).

| Module | Recurring steps inside |
|---|---|
| rawprepare | per-photosite affine, the coefficient picked by CFA position |
| whitebalance | per-photosite multiply by CFA colour |
| highlights (clip) | per-photosite clamp against a threshold from context |
| demosaic | CFA interpolation (specialised, full resolution only) |
| colorin | one 3x3 matrix from the camera's context |
| channelmixerrgb | matrix to CAT16 LMS, von Kries scaling, gamut compression, clip, matrix back |
| exposure | per-channel affine whose coefficients derive from EV and black |
| sigmoid | parameter solve (no pixels), per-channel curve, desaturate negatives, hue restore |
| flip | geometric remap of the plane |
| blur | separable Gaussian, radius scaled by the render scale |
| masks | shape rasterisation, per-channel ranges multiplied, luminance range, fold of members by operator, guided feather, blur, levels curve, mix through the mask |
| vkdt colour | matrix, CAT16 adaptation, exposure, saturation, clip, fused in one kernel |
| vkdt grade | CDL per pixel, weighted by luminance zones on a log scale |
| vkdt filmcurv | per-channel curve, then the middle channel restored (hue preservation) |
| vkdt usm | five-tap blur, detail boosted under a threshold |
| vkdt contrast | luminance, guided filter (box means in a pyramid, linear model, resample), combine |

The same handful of steps comes back. darktable shows the same pattern
at scale (counted by name in `src/iop/*.c` of the fork, roughly): the
blending layer (`develop/blend.c`) serves 69 modules; Gaussian or box
filters are called by 19, the bilateral grid by 9, guided or EIGF
filters by 7, the working-profile helpers (`dt_ioppr_*`, luminance from
the profile) by 23, colour space conversions by 21, curve helpers by 16,
interpolation (resampling) by 11 and histograms by 22. Each lives in
`src/common/` as a C helper with no description, called from imperative
code, so it is shared code but not a brick: nobody outside a module can
compose it.

### 2.2 Candidate bricks

Costs per output pixel, `c` the channels, `r` a radius in pixels; "kind"
is what fusion and planning care about (section 5.3).

| Brick | Inputs | Outputs | Parameters | Kind, cost |
|---|---|---|---|---|
| affine | image | image | per-channel scale and offset | pointwise, ~2c ops |
| matrix3 | image (3 channels used) | image | 3x3 matrix, or a value port | pointwise, 9 mul-add |
| colour convert | image in space A | image in space B | from, to (Rec.2020, XYZ, LMS CAT16, Jzazbz, dt UCS, log2) | pointwise, 10 to 60 ops |
| luminance | image | mask-like image (1 channel) | weights, or the working profile's | pointwise, 3 mul-add |
| curve apply | image or 1-channel image, curve | same shape | per channel or on luminance, with hue restore option | pointwise, one LUT read or a closed form |
| clamp / gamut compress | image | image | limits, compression knee | pointwise |
| range mask | image | mask | channel, four range points, feather, invert | pointwise |
| shape mask | none (the plane) | mask | shape geometry in image coordinates | generator, declares an `extent` |
| mask fold | n optional masks | mask | operator, opacity and inversion each | pointwise |
| mix | a, b, mask | image | opacity, mode (normal, multiply...) | pointwise |
| Gaussian blur | image | image | sigma in full-resolution pixels | neighbourhood, separable, ~4r·c or recursive O(c) |
| box mean | image | image | radius | neighbourhood, O(c) with running sums |
| guided filter | image, guide | image | radius, epsilon | composite: box means, linear model, combine |
| resample | image | image at another scale | factor, filter (box, Catmull-Rom, Lanczos) | resampling, changes resolution |
| histogram | image | statistics (bins per channel) | bins, scale (linear or stops), range | reduction, full extent (analysis) |
| percentile / mean | statistics or image | scalar | which statistic | reduction, then tiny |
| curve solve | scalars | curve | a model (sigmoid, film, spline) and its knobs | value only, no pixels |
| geometric remap | image | image | a mapping (flip, rotate, crop, distortion) | geometry, declares `output_extent` |

The last group (curve solve, the derivation of exposure's scale from
EV) is not image work at all: it is what darktable's `commit_params`
does. It belongs in the brick layer as **value bricks** (section 5.1),
or modules built from bricks cannot express sigmoid or exposure.

### 2.3 How other tools expose sub-module building blocks

- **darktable**: no sub-module layer. An IOP is one `process()` (plus
  `process_cl`), calling shared helpers in `src/common`; blending is
  applied by the framework after the module, the only composition there
  is. Parameters are introspected, so presets and the MCP can read them,
  but a helper cannot be wired by anyone outside the C code.
- **vkdt**: the closest prior art. A module's `create_nodes` builds a
  small graph of GPU compute nodes (each a GLSL shader with declared
  connectors), shared kernels in `pipe/modules/shared` and helpers in
  `api.h` (blur, guided filter, local Laplacian) used by many modules.
  The node graph is a run-time product of code, not data a user edits,
  and the shared kernels carry no description beyond their connectors.
- **Nuke and Natron**: two levels. Gizmos (Nuke) and PyPlugs (Natron)
  are groups of nodes saved as one, with chosen knobs exposed, which is
  Helios's macro. Below the node level, Nuke's BlinkScript lets a user
  write a per-pixel kernel compiled for CPU or GPU; the nodes themselves
  are C++ plugins (NDK). The lesson: a node group is cheap to share, a
  kernel language fills the gap groups cannot.
- **Blender**: the compositor's nodes are fixed C++ operations; node
  groups package them. Its GPU compositor compiles runs of pixel-wise
  nodes into one shader operation and runs buffered nodes (blurs,
  denoise) as their own passes, which is the fusion rule this note
  proposes. Geometry nodes go further: small, typed "field" nodes
  (values evaluated lazily per element) assembled into node groups that
  users ship as assets, with the group interface as the public face.
  Fields are the clearest model of parameter-like data flowing through
  the same graph as heavy data.
- **OpenFX**: a C plugin API between hosts and effects. An effect
  describes itself (clips, parameters, whether it supports tiles,
  render scale and multiple resolutions), then answers actions such as
  `getRegionsOfInterest` and `render`. It standardises the node level,
  not bricks: an effect is opaque inside. Its description-first,
  host-owns-everything stance is what Helios's manifest already does.
- **GEGL / GIMP**: operations are classes by shape (point filter, point
  composer, area filter, source, sink, meta), each with
  `get_bounding_box` and `get_required_for_output` for region planning.
  Meta operations build an inner graph of other operations and are the
  standard way to make a high-level filter from small ones. The base
  classes by shape are the useful lesson: a point filter declares only a
  per-pixel function and the framework does the rest.
- **Halide** (not a photo tool, but the reference for fusion): the
  algorithm (what each stage computes) is written apart from the
  schedule (tiling, fusion, vectorisation, GPU), so the same small stages
  run fused without being rewritten.

**Patterns.** (1) Every system that lets third parties compose
processing has a description-first unit (OpenFX, GEGL, vkdt connectors).
(2) Packaging groups as one node is universal (gizmos, PyPlugs, node
groups, meta operations, Helios macros). (3) Small stages are only fast
when the engine fuses pointwise runs (Blender, Halide). (4) Parameter
derivation lives either in code (darktable's `commit_params`, vkdt's C
`commit_params`) or as data flowing in the graph (geometry nodes'
fields, AVS's parameter ports); only the second is open to non-coders.

## 3. What a brick is

A brick is a **module under the module contract** whose manifest says
it is a brick (option 1 of choice Q1 below): the planning, the cache,
the GPU path and the test kit work on it unchanged. What it adds is a
stronger description and a stability promise.

### 3.1 Typed ports

Today's `PortKind` has `Image`, `AnyImage`, `SameAs` and a declared but
unused `Buffer`. Bricks need:

| Port type | Content | Notes |
|---|---|---|
| `image` | interleaved pixels, `channels`, `format` (f32, f16...), `layout` (`rgba`, `rggb`, `y`) and **`space`** | `space` is new: `rec2020-linear` (the working space), `camera`, `xyz-d50`, `lms-cat16`, `jzazbz`, `log2`, `any`, `same-as:<port>` |
| `mask` | an `image` with one channel, layout `y`, values in [0, 1] by convention | already the masks crate's convention (`mask_port`) |
| `scalar`, `vector(n)`, `matrix3` | small numbers | carried as a value, keyed by content, read whole |
| `curve` | a 1D LUT or a closed form with its parameters | one per channel or shared |
| `lut3` | a 3D LUT (size, domain) | |
| `stats` | histogram bins, moments, percentiles | the "value port" of the full-extent analysis note |

A connection is checked on kind, channels and space. A mismatch of
space is never converted silently: the assembler (or the AI) inserts a
`colour convert` brick, which then shows in the graph and costs what it
costs. Precision is declared per port; the engine's default stays f32,
and an f16 port is a promise the brick tolerates half floats, not a
conversion.

Any brick parameter may be **bound to a value port** of the same type
(AVS's parameter ports): exposure's scale comes from a scalar computed
by an analysis brick, or by a value brick from the module's parameters.

### 3.2 Parameters and descriptors

Brick parameters are `ParamSpec`s, as today: typed, ranged, defaulted,
with a description and the scale rule (a radius in full-resolution
pixels). A brick's parameters are **not shown to people by default**:
the module's parameters are, through its controls. But because each is
described, the controls layer can still build an implicit control for
it (`egui_hud::registry::implicit`), which is what the graph
surface's "opened" view and an AI's draft use.

### 3.3 Determinism and purity

Unchanged from the contract: no IO, no global state, no threads of its
own, no logging; the same plan and inputs give the same outputs. A brick
declares its **tolerance**: `bitwise` (integer-like steps, masks folds),
or a float tolerance across hardware. GPU results are keyed apart, as
today. Randomness (grain, dithering) takes a seed parameter derived
from the image's identity, never a clock.

### 3.4 Tiling, ROI and support

Each brick declares its support as the contract does (`Needs::overlap`,
`input_region` for anything else, `output_extent` with
`Needs::geometry`, `extent` for generators, `full_resolution` for the
mosaic) plus its **shape**, the GEGL lesson:

- `pointwise`: output pixel depends on the same input pixel only;
- `neighbourhood(r)`: a bounded support, scaled with the render scale;
- `resampling(factor)`: changes resolution, regions mapped by the factor;
- `reduction`: reads its input's whole extent at a declared scale and
  outputs a value (the full-extent analysis note);
- `geometry`: maps the plane;
- `generator`: no image input.

The shape is what fusion and the planner use, and what the test kit
checks: a `pointwise` brick must give the same bits on any tiling with
zero overlap.

### 3.5 CPU and GPU

The Rust CPU code is the reference; a WGSL variant is optional and
trusted within the declared tolerance, as today. Two additions:

- a pointwise brick may give its per-pixel function as a **WGSL
  snippet** (a function body over a pixel and its uniforms) instead of
  a whole kernel, so fusion can paste several into one shader
  (section 5.3);
- the accelerator's limit of one input per GPU step (noted for
  `guided3` and `combine`) becomes a brick capability (`gpu_inputs`)
  the planner reads, and lifting it is a slice of its own.

### 3.6 Versioning and history

An edit records **module** parameters and the module's version, never
the brick graph. Replay must render an old edit as it rendered then
(ROADMAP 2.1, engine version drift; 2.8, rule 6). Therefore:

- a brick id carries a **major version** (`core.blur.gaussian@1`); any
  change of output beyond its tolerance is a new major, and the old one
  stays registered, frozen, as long as a shipped module version names it;
- an assembled module's version pins the brick versions it names; a new
  module version may move to a new brick, with a parameter migration if
  needed, as a sealed module would;
- minor versions add optional parameters with defaults that keep the old
  output, and new optional ports;
- a cached step's key already includes the manifest's name and version,
  so two brick versions never share content by mistake.

Freezing is what makes the SDK a promise: a plugin assembly written
against `core.mix@1` renders the same in five years.

### 3.7 Cost hints

Declared, then corrected by measure (the scheduler already chooses by
measured speeds):

- `ops_per_pixel` (order of magnitude), `passes`, `memory_factor` (the
  buffers it holds per output pixel), the shape above;
- `fusable` (pointwise with a WGSL snippet and a Rust closure form);
- whether it is scale invariant and how good its approximation is on a
  reduced preview (the contract's existing wish).

The planner uses them for tile sizes and device choice; the AI uses
them to prefer a cheap assembly; the graph surface shows them in the
node's load.

### 3.8 An accessible, AI-readable description

Each brick's manifest carries, in English and translated through the
i18n catalogues as other shipped text:

- a one-line **purpose** and a paragraph of **what it does** (the
  formula when there is one);
- **when to use it** and **when not to** (use `box mean` for guided
  filters, not as a visual blur);
- per port and parameter: meaning, unit, space, typical values;
- **keywords** and the **family** (tone, colour, detail, mask, geometry,
  analysis, value);
- **examples**: a tiny assembly using it, which doubles as a test;
- its provenance (darktable helper, vkdt kernel, Helios's own) and
  licence notes (vkdt ports are BSD-2-Clause as well as GPLv3).

This is the same shape as a control's `purpose` and
`controls.describe`, so one reader serves both.

## 4. Registration and discovery

### 4.1 The registry

A `BrickRegistry` sits beside the node's module registry and the
template `Catalogue` (which checks composite files against it). Built
in Rust for core bricks (`register(ModuleRef)`), filled from plugin
manifests for plugin bricks (section 6.1). Registering checks the
manifest: a namespaced id with a major version, a description, every
port typed, a shape, and the contract tests passing in the test suite
of the crate that ships it.

### 4.2 Namespaces

| Namespace | Holds | Promise |
|---|---|---|
| `core.*` | Helios's general bricks (affine, matrix3, blur, mix, histogram...) | the SDK: frozen per major |
| `dt.*` | bricks extracted from darktable helpers, faithful to them (`dt.blend.normal`, `dt.gaussian`) | parity with darktable within its thresholds |
| `vkdt.*` | vkdt's shared kernels (`vkdt.guided1`...) | parity with `vkdt-cli` |
| `exp.*` | experimental bricks | none: may change or vanish; an assembly naming one is marked experimental |
| `<reverse-domain>/*` | a plugin's bricks (`org.example.lut3d`) | the plugin's own, held to the contract |

Module names stay as today (`exposure`, `vkdt_contrast`); a module and a
brick may share a word but never an id. The existing kernels
(`vkdt_guided1`, `mask_combine`) get brick ids as aliases, keeping their
current names for saved networks.

### 4.3 Capabilities a brick declares

`cpu`, `gpu` (and `gpu_inputs`), `tiles`, `fusable`, `scale-invariant`,
`deterministic: bitwise | tolerance`, `context` (the metadata fields it
reads, typed by the domain), `full-resolution`, `geometry`,
`reduction`. A plugin brick adds where its code runs (section 6.1). The
assembler refuses an assembly whose target surface lacks a capability
(a web client without GPU gets the CPU path or a refusal, never a
silent fallback that differs beyond tolerance).

### 4.4 How an AI lists and reads them

As controls are read today, the same three levels, as commands and MCP
verbs:

- `bricks.catalogue` (`helios_bricks_catalogue`): every brick, its id,
  family, purpose line, shape, ports in one line; filter by family,
  keyword, port type;
- `bricks.describe` (`helios_bricks_describe {id}`): the full
  description, ports, parameters with ranges, cost, capabilities,
  examples, versions available;
- `helios_describe` with entity `bricks`, next to `modules`, for a
  snapshot.

The answers are plain JSON produced from the manifests, so they are the
same in the desktop, the node and a test, and the catalogue's JSON is
frozen by a snapshot test (a changed description is a reviewed diff).

## 5. Assembly

### 5.1 A module is a small graph of bricks plus a card

An assembled module is a **composite file** with two more blocks:

```json
{
  "format": 1, "id": "helios.exposure-bricks", "version": 1,
  "module": { "family": "tone", "role": "general", "context": ["exposure_bias"] },
  "params": [
    { "name": "exposure", "type": "float", "min": -18, "max": 18, "default": 0 },
    { "name": "black", "type": "float", "min": -1, "max": 1, "default": 0 }
  ],
  "values": [
    { "name": "white", "brick": "core.value.exp2@1", "in": { "x": "-exposure" } },
    { "name": "scale", "brick": "core.value.affine-inv@1",
      "in": { "white": "white", "black": "black" } }
  ],
  "nodes": [
    { "name": "apply", "brick": "core.affine@1",
      "params": { "offset": "-black", "scale": "scale" } }
  ],
  "connections": [ { "from": "@", "port": "input", "to": "apply", "input": "input" } ],
  "exits": [ { "from": "apply", "port": "output", "output": "output" } ],
  "controls": [ { "control": "exposure", "jack": "value", "param": "exposure" } ]
}
```

(The block names are illustrative; the shape follows `CompositeFile`.)

- `params` are the module's own parameters, the ones an edit records.
- `values` is a **value graph**: value bricks (pure functions on
  scalars, vectors, curves, matrices) evaluated at prepare time, before
  any pixel, from the module's parameters and its context. This replaces
  `commit_params` and `prepare` for assembled modules. Expressions stay
  references (`-exposure` is a value brick `neg` written short), so the
  file stays data an AI can produce and a schema can check.
- `nodes` are image bricks; each parameter is a constant, a module
  parameter, or a value.
- `controls` patches the module's parameters to controls, as a sealed
  module's `Patch`es do; a parameter without a patch gets its implicit
  control. The card is laid out by the controls layer as for any module.

Today's macro `exposed` list is the one-to-one special case of `params`
plus `values`: a macro is an assembly with no value graph and no card of
its own.

### 5.2 Below the node diagram

On the graph surface a module is **one node**; its bricks are inside,
the way a composite is today (folded by default, opened in place, the
inner steps listed in the engine's status as `module/brick`). Exposed at
the node level:

- the module's ports (its `inputs` and `outputs`), parameters and card;
- its header (family, role, state, load: the sum of its bricks' cost);
- bypass of the whole module.

Not exposed by default: brick parameters, the value graph and inner
wiring. Opening the node shows them read-only for a shipped module
(changing a shipped module's inside would fork it); "Make editable"
copies it into the workspace as a new module id, as macros' editing does.

### 5.3 When a brick graph is collapsed or compiled

The engine already plans each inner node as a step with its own cache
entry. That is right for neighbourhoods and reductions, wasteful for a
run of pointwise bricks (each writes a full buffer). The proposal:

1. **Plan as today**, then **fuse** maximal runs of `fusable` pointwise
   bricks with one consumer each into one step: on the CPU one loop
   calling each brick's per-pixel function, on the GPU one WGSL shader
   pasting their snippets (Blender's GPU compositor rule).
2. A fused step is keyed by the list of its bricks' keys, so a fused and
   an unfused render share their inputs and outputs, not the
   intermediates nobody reads.
3. A node whose intermediate a person looks at (the Masks panel's
   selected node, a probe in the graph) is a fusion boundary.
4. Fusion is an optimisation the test kit checks: fused equals unfused
   bit for bit on the CPU, within tolerance on the GPU.

A sealed module stays the escape hatch when fusion cannot reach the
speed of hand-written code (demosaic, the local Laplacian).

## 6. Plugins and AI

### 6.1 What an out-of-process plugin can contribute

| Contribution | What it is | Runs where | Cost |
|---|---|---|---|
| assembled modules | composite files naming registered bricks, with params, values and controls | in Helios's engine; no plugin code at render | nothing beyond the bricks; reviewable as data |
| WGSL bricks | a pointwise or neighbourhood kernel with its manifest | on the GPU, which sandboxes it | needs a CPU reference: a WGSL-on-CPU path, or the brick is GPU-only and marked so |
| code bricks | a brick whose `process` runs in the plugin's process | out of process | pixel transfer per tile (below) |
| sealed modules | a whole module in the plugin's process | out of process | the same transfer, once per module instead of per brick |

**Serialisation cost.** The plugin protocol is JSON-RPC over pipes. A
24-megapixel image in f32 RGBA is about 384 MB; base64 in JSON adds a
third. Per edit, that is unusable. A code brick needs **shared memory**
(a memory-mapped file or `memfd` handed over the broker, the region and
layout in the JSON message, the pixels never in it) and works on tiles:
the cost is then a round trip per tile (tens to hundreds of
microseconds) and a copy into and out of the engine's buffers, which
breaks GPU residency (the image comes back to the host). This is the
cost a plugin brick pays; bricks in the middle of a fused GPU run are
the worst case.

**Sandbox.** Assembled modules need none: they are data checked against
the registry. WGSL bricks are sandboxed by the GPU driver and checked
for bounded loops by the WGSL validator. Code bricks run under the
broker's capabilities with none granted (no network, no files): a
brick only gets its tiles. Until the process itself is held
(namespaces, seccomp), this is a disclosure, as extensions.md says.

### 6.2 How an AI builds a module

The same path a person takes, as verbs:

1. **Intent.** The request in words ("soften skin but keep pores").
2. **Read the toolbox.** `bricks.catalogue`, then `bricks.describe` for
   the candidates; `controls.catalogue` for the card.
3. **Draft.** An assembly file: bricks, wiring, the value graph, the
   module's parameters with ranges and defaults, control patches.
4. **Validate** (`module.validate`, pure, no pixels): schema, every id
   known, port kinds, channels and spaces agree, no cycle, every brick
   parameter fed, every module parameter used, ranges inside the
   bricks' hard ranges, context fields available, capabilities met on
   the target, cost estimate under a budget. Each problem is a
   diagnostic with a path, which the AI fixes and resubmits.
5. **Test** (`module.test`): the test kit's contract on the assembly
   (defaults, range ends, adversarial inputs, tiled equals untiled, half
   scale near the full render reduced), renders on the shipped test
   charts at the range ends, and the module's own examples. The report
   is JSON with images by content key.
6. **Show.** A render on the open image (a sandbox subworkspace, as
   `helios_sandbox_start` gives), before and after, with the card.
7. **Confirm and save.** Saved to the workspace as a versioned
   composite file; publishing to the user's library asks the user first,
   as macros' publishing does today.

The **test kit for authors** (human, plugin or AI) is that list behind
two verbs, plus fixtures: a few synthetic charts (ramps, a colour
checker, noise, a hard edge), a small set of real raws from the corpus,
and golden renders for every shipped module.

## 7. Risks

**Performance: fusion against a graph of small kernels.** Each unfused
step writes and reads a full intermediate: four pointwise bricks on a
24-megapixel f32 image move about 3 GB of memory where one fused loop
moves 0.8 GB. Without fusion an assembled exposure through a mask
would be several times slower than the sealed one. Fusion of pointwise
runs recovers most of it; neighbourhood bricks stay separate passes, as
in vkdt and Blender. The cache helps interaction (a slider moving
recomputes only the steps after it) but costs memory per
intermediate; fused steps keep it bounded. Measure before deciding:
the masks bench (58 mask nodes) is the stress case already there.

**Keeping darktable equivalence.** Modules mirroring darktable are
checked against `darktable-cli` within Delta E thresholds. Rebuilding
them from bricks changes float operation order, which can move a pixel
past a threshold (a gamut clip, a sigmoid's black). The rule: a
darktable module is rebuilt from bricks only when the brick form equals
the sealed form (bit for bit on the CPU, or within the module's parity
thresholds with the parity test passing), and the sealed form stays
until then. `dt.*` bricks are extracted from the sealed code, not
rewritten, so both forms share their numbers.

**Stability of the SDK surface.** Once plugins and saved assemblies name
bricks, every brick is a promise. Mitigations: a small `core.*` set at
first; `exp.*` for anything unproven; majors frozen and kept; the
catalogue's JSON under a snapshot test; a brick deprecated only by
marking it (still rendering, hidden from new drafts). The cost is code
kept forever; vkdt and darktable carry the same cost in `legacy_params`.

**Granularity drift.** Too fine (an `add` brick, a `multiply` brick)
and assemblies become programs, slow and hard to read; too coarse and
bricks are modules again. The test: a brick is worth registering when
two modules use it, or when it is the natural unit a person or an AI
names ("guided filter", "mix through a mask").

**Value graph as a hidden language.** Derivations can grow into a
programming language. Keep value bricks pure, small, typed, and listed
like image bricks; anything beyond them is a sealed module.

## 8. Proposal and slices

### 8.1 A minimal first brick set

Ten image bricks and a handful of value bricks, all from code that
exists:

- image: `core.affine`, `core.matrix3`, `core.luminance`,
  `core.curve.apply`, `core.mix` (from `mask_blend`), `core.mask.range`
  (from `mask_parametric`), `core.mask.fold` (from `mask_combine`),
  `core.blur.gaussian` (from `blur`), `core.box.mean`,
  `core.resample`; `vkdt.guided1..3` as aliases of the existing kernels,
  and `core.guided` as their assembly;
- analysis: `core.histogram` (from `helios-scopes`' counts), when the
  full-extent analysis note is decided;
- value: `core.value.{neg, add, mul, exp2, log2, affine-inv, clamp}`.

### 8.2 The first module rebuilt from bricks: exposure

darktable's `exposure` in manual mode: `white = 2^-exposure`,
`scale = 1 / (white - black)`, `out = (in - black) * scale`, black
pulled below white, and the camera's bias taken back with
`compensate_bias`. It exercises every new piece at small risk: a value
graph with a context input (the bias, its trust rule as a `clamp`
brick), one image brick, a control patch (the `exposure` control exists),
and a parity test that already exists for the sealed module. Proof:
the assembly equals the sealed `exposure` bit for bit on the CPU, passes
`assert_contract_of`, and its card is the sealed module's card. Second
proof, once that holds: the same exposure through a parametric mask
(`core.mask.range` into `core.mix`), checked against darktable's
blending parity, which is what 69 darktable modules do.

### 8.3 Slices

1. **Brick identity.** Manifest fields: `layer` (brick or module),
   namespaced id with major version, shape, description block,
   capabilities; the `BrickRegistry`; the existing vkdt kernels and
   mask modules registered as bricks under aliases; `bricks.catalogue`,
   `bricks.describe`, `helios_describe` entity `bricks`; a snapshot test
   of the catalogue. No behaviour change.
2. **Typed values.** Value ports (`scalar`, `vector`, `matrix3`,
   `curve`) in the engine, keyed by content, read whole; the `space` tag
   on image ports, checked at connection; parameters bound to value
   ports.
3. **Assemblies.** The composite file's `module`, `params`, `values`
   and `controls` blocks, schema and serde together; `module.validate`;
   assembled modules registered in the node's registry like sealed
   ones; `exposure` assembled, equal to the sealed one.
4. **Fusion.** Pointwise runs fused on the CPU, then on the GPU from
   WGSL snippets; fused equals unfused in the test kit; the masks
   bench and an export benchmark before and after.
5. **Author kit.** `module.test`, the charts and golden renders, the
   AI path end to end in a sandbox subworkspace (draft, validate, test,
   show, save), publishing behind the user's confirmation.
6. **Plugins.** Assembled modules shipped by a plugin as data (in its
   manifest's `modules`); later, code bricks over shared memory, after
   a measure of the tile round trip.

Slices 1 and 2 are useful alone (an AI can read the toolbox; analysis
nodes get their value ports). Each is one branch, merged to `next`
with its tests.

## 9. Decisions (2026-11-10)

**Q1. Contracts: one module type declares the contracts it offers.**
A module is not either a brick or a node: one module can offer several
**contracts**, each the way it is used.

- **brick**: a step inside an assembly (§3, §5);
- **node**: an element of the node diagram, with its card of controls
  and its exposed ports (§5.2);
- **process**: a step of a batch, an export or a job, with job inputs
  and outputs;
- later ones as they come (a mask source, an analysis…).

The manifest lists them: `provides: [brick, node, process]`, each with
its own ports and description. Every contract goes through the same
`Module` core (parameters, regions, cache, GPU, bypass, the test kit);
a contract adds only what is its own (a brick its shape and tolerance,
a node its card and exposed ports, a process its job inputs and
outputs). The registry indexes modules by contract, so
`bricks.catalogue` and `nodes.catalogue` are views of one registry. An
assembly written as data declares its contracts the same way. This
replaces the study's first wording, "a module marked `layer: brick`"
(§3, §4.1).

**Q2. A value graph.** A module's parameters reach brick parameters
through a graph of registered value bricks, evaluated at prepare time
and written as data in the assembly (§5.1). A derivation (exposure's
scale, sigmoid's constants) is itself a small described brick an AI
can assemble and a schema can check. (Left to the implementer by the
owner; the recommended option.)

**Q3. Forms: no restriction, the implementation declares them.** What a
plugin may contribute is not restricted by policy. Each implementation
declares the **forms** its process is available in, a second axis next
to the contracts:

- an assembly written as data;
- Rust code (CPU);
- a WGSL kernel (GPU);
- code run out of process over shared memory;
- later ones.

A process may offer several forms. The engine picks a form per run and
per context (interactive preview, export, a parity test). A missing form
is visible, not forbidden: a GPU-only process cannot serve where a CPU
path is required (a CPU-only export, the darktable parity tests), and
the registry says so; an out-of-process form states its transfer cost.
"The CPU is the reference" becomes a property: a process with a CPU form
can be the reference, one without it cannot. This replaces §6.1's
order (data first, out-of-process code later) with declared forms;
§8.3's plugin slice still comes after the tile round trip is measured.

**Q4. darktable-mirroring modules move to bricks only at equality.**
Each is rebuilt from bricks once its brick form equals the sealed one,
bit for bit on the CPU or within its parity thresholds with the parity
test green; the sealed form stays until then. Exposure is the first;
demosaic may stay sealed for good. (Left to the implementer by the
owner; the recommended option.)

## 10. Sources

Read on 2026-11-10.

- Helios: [module-contract.md](../module-contract.md),
  [extensions.md](../extensions.md), [controls.md](controls.md),
  [control-contexts.md](control-contexts.md), [macros.md](macros.md),
  [workshops.md](workshops.md); `crates/helios-core` (`module.rs`,
  `manifest.rs`, `network.rs`), `crates/helios-node`,
  `crates/helios-template`, egui_hud,
  `crates/helios-controls`, `crates/helios-testkit`, `modules/*`.
- darktable: `src/develop/blend.c`, `src/common/{gaussian,bilateral,guided_filter,eigf,box_filters,interpolation,iop_profile,curve_tools,histogram,colorspaces_inline_conversions}.*`,
  counts over `src/iop/*.c`.
- vkdt: `src/pipe/modules/api.h`, `src/pipe/modules/shared/`, and
  [vkdt-study.md](../vkdt-study.md).
- GEGL: [operation API](https://gegl.org/gegl-operation.h.html)
  (`get_bounding_box`, `get_required_for_output`, operation base
  classes).
- OpenFX: [properties by object](https://openfx.readthedocs.io/en/doc/Reference/ofxPropertiesByObject.html)
  (tiles, render scale, regions of interest).
- Blender: [GPU (realtime) compositor](https://docs.blender.org/manual/en/latest/compositing/realtime_compositor.html),
  [node previews for shader operations](https://projects.blender.org/blender/blender/pulls/109250)
  (pixel-wise nodes compiled into shader operations).
- Nuke gizmos and BlinkScript, Natron PyPlugs, Halide's algorithm and
  schedule split: vendor and project documentation, paraphrased.
