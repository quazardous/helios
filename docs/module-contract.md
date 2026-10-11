# Module contract (draft)

Status: **draft, not decided** (2026-10-11). This page defines what an image
processing module is in Helios, so that every module can be written, tested
and replaced on its own. See ROADMAP section 2.1, "Isolated modules, layered
framework".

## Principle

A module is a **pure computation with a declared surface**. Everything it
reads is an input, everything it produces is an output, and the framework
owns everything else: files, catalog, caches, threads, GPU, scheduling,
logging. A module that needs something the contract does not offer gets the
contract extended, never a side channel.

The code is horizontal for features: adding a module adds a folder and
touches nothing else. It is vertical for the framework: module contract,
graph, scheduler, vault, surfaces, each layer only knowing the one below.

## Layers: a general core, a photo domain

Photo editing is one application of image processing (ROADMAP section 1).
The contract is therefore domain-agnostic:

- **core**: data types, ports, parameters, graph, regions, scheduling. No
  notion of raw file, EXIF, camera or color science.
- **domains**: typed vocabularies and modules built on the core. The photo
  domain defines raw buffers, camera metadata (EXIF), color spaces and the
  darktable and vkdt modules. Other domains (video, scientific imaging) may
  come later.
- **applications and surfaces**: a photo editor, a batch exporter, an AI
  workshop, each a network of domain modules.

A module depends on the core and on the domains it uses, never the reverse.

## Two lineages, one contract

Helios's long-term goal is to make darktable and vkdt meet (ROADMAP section
2.1). The contract is where they meet, so it must be able to describe a
module coming from either side:

| | darktable | vkdt | contract |
|---|---|---|---|
| graph | linear chain (pixelpipe) | DAG, several sources and sinks, `feedback` connectors for multi-frame work | DAG; a chain is a special case; feedback to be decided |
| module | one `process()` per module, CPU (OpenMP) plus OpenCL | a module creates one or more GPU compute nodes (GLSL) | a module may expand into a subgraph of nodes |
| parameters | C struct bytes, introspected, `legacy_params` | text file `params` (`exposure:float:1:0.0`), `params.ui` | named, typed parameters with explicit migrations |
| ports | one input, one output, implicit raster masks | text file `connectors` (`input:read:*:*`) | declared image, analysis and metadata ports |
| region | `roi_in` / `roi_out`, tiling callbacks | `modify_roi_in` / `modify_roi_out` | `input_region`, declared overlap |

The Rust CPU implementation is the reference; a GPU variant (vkdt-style
shaders) is the "best effort" part of "Rust first" and is checked against
it. The manifest should be translatable to and from vkdt's text files, so a
vkdt module can be described without rewriting it.

## The design reference: AVS

The model behind the contract comes from scientific imaging: AVS (Advanced
Visual Systems), where an application is a network of modules assembled in
a visual editor (ROADMAP section 1). darktable and vkdt are the two code
lineages to unify; AVS is the shape they are unified into. What the contract
takes from it:

- **Typed data flowing between modules, not only images.** AVS passes
  general data structures (fields: images, volumes, meshes, values) along
  typed connections. Helios ports carry images, but also histograms, masks,
  statistics and metadata, each with a declared type.
- **Parameters are ports too.** In AVS any parameter can be connected to the
  output of another module. In Helios, an exposure value can be set by the
  user or driven by an analysis node (darktable's deflicker, vkdt's
  `autoexp`) with the same module.
- **Macros: a network packaged as one module**, exposing only the parameters
  chosen by its author. This is what a Helios workshop is (ROADMAP section
  2.5): real modules, pre-calibrated, with only the useful sliders.
- **The network is the application.** A saved network is a reusable workflow;
  the darktable surface, the graph surface and the AI surface are different
  editors of the same network.

## Why: what darktable's `exposure` hides

darktable's `exposure` module (`src/iop/exposure.c`) looks like a one-line
formula, `out = (in - black) * scale`, but its real surface is wider than its
parameters:

| hidden dependency in darktable | in the Helios contract |
|---|---|
| reads EXIF exposure bias and highlight preservation from `dev->image_storage` | declared **context inputs** (image metadata fields) |
| deflicker mode reads the raw histogram of the image | a declared **analysis input** produced by another node |
| `process()` multiplies `pipe->dsc.processed_maximum`, shared pipe state | a declared **metadata output**, never a mutation |
| `commit_params()` turns user parameters into processing values using the image | a pure `prepare(params, context) -> plan` step |
| parameters stored as raw C struct bytes, versioned with `legacy_params` | named, typed parameters; migrations are explicit functions |

Each line is a reason why darktable modules cannot be tested alone today.

## What a module declares (manifest)

- **Identity**: name, version, short description.
- **Header**: what kind of node it makes (process, decision, sink), its
  family (tone, color, detail, geometry, merge, mask, AI, IO) and its role
  (general purpose or specialised). With the state and load the engine
  adds, this is the node header every surface shows (see [gui.md](gui.md)).
- **Parameters**: named and typed (float, int, bool, enum, curve, ...), with
  default, range and description, plus whether a value depends on the image
  scale (a radius in pixels) and how it scales.
- **Ports**: image inputs and outputs (pixel format, color space, channels);
  analysis inputs and outputs (histograms, masks, statistics); metadata
  outputs (processed maximum, ...).
  Any parameter may also be bound to a port of the right type (AVS style).
- **Composition**: a module may be a macro, a subgraph of other modules
  with a chosen subset of their parameters exposed.
- **Context inputs**: the metadata fields it reads, by name, typed by a
  domain (for the photo domain: EXIF bias, camera matrix, ...).
- **Computation needs**: neighbourhood it reads around each output pixel
  (overlap), memory factor, whether it can be tiled, whether it is scale
  invariant and how good its approximation is on a reduced preview.
- **Geometry**: whether it changes the image region (crop, distortion) and
  the mapping between output and input regions.
- **Migrations**: one function per version step, from version N parameters to
  version N+1, named and tested.

The manifest is data (readable without running the module), so a surface,
the scheduler, the AI or a test generator can use it.

## What a module implements

```text
prepare(params, context) -> plan          pure, cheap, no pixels
prepare_at(params, context, scale) -> plan     the same, for a render at a scale
input_region(plan, output_region) -> input_region
process(plan, inputs, output_region) -> outputs     pure, no IO, no globals
```

- `process` gets pixel buffers for the requested region and returns output
  buffers. It owns no threads; data parallelism is offered by the framework.
- No access to files, the catalog, the GUI, global configuration or other
  modules. No logging: the framework records what ran, with which plan.
- Deterministic: the same plan and inputs give the same outputs (up to a
  declared floating-point tolerance on different hardware).
- **Render scale.** A render can ask for a region at a scale (1 is full
  resolution, 0.5 half of it): regions are then in pixels at that scale,
  and sources come reduced by the framework (each pixel the mean of the
  full-resolution pixels it covers). A module whose parameters are sizes
  in pixels (a blur radius) states them in full-resolution pixels and turns
  them into pixels at the render's scale in `prepare_at`; its overlap
  follows, since `input_region` gets the plan. A preview then looks like
  the full render reduced. Modules working pixel by pixel ignore the scale.
  Renders at different scales are cached apart. Modules that only make
  sense at full resolution (rawprepare, demosaic: a reduced mosaic mixes
  its colors) get their preview from a demosaiced proxy instead, as
  darktable's preview pipe does.

## Decision nodes: where the algorithm alone is not enough

A network must be able to contain an **AI junction**: a point where a fixed
algorithm cannot decide and a judgment is needed (is the sky blown out? which
of these masks is the subject? does this render match the intent?). This is
a node kind of its own, distinct from ML inference: a denoising or
segmentation network with fixed weights is an ordinary, deterministic module.

A decision node:

- takes ordinary inputs (images, previews, statistics, an intention) and
  produces ordinary outputs: parameter values (through parameter ports), a
  mask, a choice between branches, or a verdict that loops back to an
  earlier node;
- is resolved in one of three **modes**, through the same contract:
  - **full AI**: the model decides and the network goes on; the human can
    review and override afterwards;
  - **AI-assisted**: the model proposes (one answer or ranked options) and
    a human confirms, picks or corrects before the network goes on;
  - **human**: a human decides, with no AI involved.

  The mode is a setting of the node, which a project or a run can override
  (a batch export in full AI, a client job in AI-assisted). With no AI
  available, a node falls back to human mode or to a declared default;
- is **recorded, not recomputed**. Its answer is stored as data in the edit
  (who decided: the model, a human, or a human accepting or correcting the
  model; the model and its version; the inputs it saw), so the network
  re-renders deterministically and offline from the recorded answer. Asking
  again is an explicit operation that creates a new version of the edit,
  never a side effect of a render;
- is scheduled like any expensive unit of work: it may run remotely, take
  seconds, be pending (downstream nodes show a proxy or wait), or be
  cancelled.

This keeps the rule that modules are pure (ROADMAP section 2.8, rules 2
and 6): the non-deterministic part is
the resolution of the node, which is outside the render and lands in the
edit like any user action.

How it is built:

- in the core, a decision node is a module of kind `decision` whose
  parameters are its answer. Until one is recorded it is **pending**: a
  render through it fails with a transient `Pending` error (nothing read,
  computed or cached), and its status says `pending`. It is answered
  through `Graph::answer`, never edited with `set_param`, so every change to
  it goes through the book below;
- `helios-decision` holds the **book** of an edit: each node's mode, every
  answer with its version, who decided, the model and its version, and the
  content keys of the inputs it saw. A model answer is the answer in full AI
  mode, a proposal in AI-assisted mode, and refused in human mode; a human
  may always decide, which in full AI mode is the review. Asking again
  withdraws the answer in force and keeps the history. A stored book
  replays into a fresh graph with no model, and an answer given on other
  inputs is flagged for review, never recomputed;
- the mode set on a node holds unless a project or a run (or a user, for
  themselves) sets `project.decisions.mode`; a global preference only fills
  in for nodes that set none.

## How a module is tested

Every module is testable alone, with no darktable and no framework running:

1. **Unit tests**: `prepare` and `process` on small synthetic images.
2. **Contract tests**, generated from the manifest, at full and half
   scale: defaults and range ends do not produce NaN or infinity, renders
   are reproducible, the declared overlap is enough (tiled render equals
   untiled render). A module with a size in pixels also checks that its
   half-scale render is close to its full render reduced. Migrations
   round-trip once modules have versions to migrate.
3. **Parity with darktable**: for a module ported from darktable, the same
   image and parameters rendered by `darktable-cli` with only that module
   active, compared within a Delta E threshold. darktable is the oracle.

## Language

Rust (ROADMAP section 1: Rust first, best effort for the rest). One module =
one crate or one folder in a modules crate, to be decided with the first
module. GPU variants are a later question; the CPU Rust implementation stays
the reference.

## Open questions

- Pixel buffer type: layout (interleaved RGBA float like darktable,
  planar), alignment, and how regions and tiles are expressed.
- Granularity of context inputs: individual EXIF fields, or a typed image
  description shared by all modules.
- Manifest format: Rust types with derived description, or a separate file
  (vkdt uses plain text files `params`, `connectors`, `params.ui`). Either
  way it must round-trip with vkdt's files.
- Decision nodes: how a choice between branches is expressed in the graph
  (a switch node, or conditional edges), and what a recorded answer
  contains (model, prompt, inputs by content hash).
- Feedback connectors (vkdt's cycles for multi-frame work): in the graph
  model from the start, or later.
- Data parallelism: which primitives the framework offers (`rayon`-style
  parallel iterators over rows or tiles).
- Where GUI hints live (slider ranges, soft limits, grouping): in the
  manifest or in a separate surface description.

## First steps

1. A minimal framework: load a module, give it an image region and
   parameters, get the output.
2. Port `exposure` (manual mode first, then the context inputs, then
   deflicker as an analysis input) with its parity test against
   `darktable-cli`.
3. Port a module with a neighbourhood and a scale-dependent radius (a blur)
   to exercise overlap, tiling and scale invariance.
4. Describe one vkdt module (its `exposure`) with the contract, to check the
   manifest covers the other lineage before it sets.
