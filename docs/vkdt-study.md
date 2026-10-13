# vkdt: how its modules describe themselves and run

Status: study (2026-10-11), read-only, on vkdt at revision `afc34256f`
(2026-10-07). Inventory: [vkdt-inventory.md](vkdt-inventory.md), generated
by `tools/vkdt_inventory.py`. Purpose: prepare the meeting of darktable and
vkdt in the Helios module contract (ROADMAP section 2.1,
[module-contract.md](module-contract.md)).

## In one paragraph

vkdt ([github.com/hanatos/vkdt](https://github.com/hanatos/vkdt), BSD-2-Clause
with some GPLv3 files) is a GPU-only (Vulkan) raw and video processor. Its
pipeline is a DAG of **modules**; each module expands into a sub-graph of
**nodes**, one compute shader each. A module is a folder of plain files
(`params`, `connectors`, `params.ui`, GLSL shaders) plus optional C callbacks
in `main.c`; a network is a text `.cfg` file. Everything is designed to stay
in GPU memory: one big allocation, buffers bound into it, only sinks
download.

## What a module is made of

| file | content | example |
|---|---|---|
| `params` | one line per parameter: `name:type:count:default...` (types float, int, string) | `exposure:float:1:0.0` |
| `connectors` | one line per port: `name:type:channels:format`; type read, write, source, sink (modify exists in code) | `input:read:*:*`, `output:write:&input:*` |
| `params.ui` | one line per widget: `param:widget[:min:max]:...`, groups, separators | `exposure:slider:-10:10` |
| `*.comp` | GLSL compute shaders, one per node | `main.comp` |
| `main.c` | optional callbacks (below) | |
| `readme.md` | the module's documentation | |

Channels are `rgba`, `rg`, `r`, `y`, `rggb` (a raw mosaic), `ssbo` (a storage
buffer) or `*` (any); `&input` means "the same as connector input". Formats
are `f16`, `f32`, `ui8`, `ui16`, `ui32`, `atom`, `bc1` (compressed thumbnails),
`tri` (geometry) or `*`.

Callbacks in `main.c` (share of the 99 modules, from the inventory):

| callback | role | share |
|---|---|---|
| `create_nodes` | build the module's sub-graph of nodes (several shaders, intermediate buffers) | 54 / 99 |
| `modify_roi_in` / `modify_roi_out` | region the module needs / produces | 48 / 99 |
| `commit_params` | translate parameters before upload (strings to floats, derived values) | 20 / 99 |
| `check_params` | react to a parameter change (may restructure the graph) | frequent |
| `read_source` / `write_sink` | IO of input and output modules | 16 / 13 |
| `animate` | change over time (video, simulations) | 2 / 99 |
| none | the module is only its files and one `main.comp` | 21 / 99 |

Feedback connectors (a buffer written on one frame and read on the next) are
wired in code (`dt_node_feedback`), by 6 modules (temporal denoising, neural
network training, simulations).

## A network

`.cfg` files are the graph itself, line by line:

```text
module:i-raw:main:43:400            # module:<name>:<instance>:<x>:<y> (x, y = GUI position)
connect:i-raw:main:output:denoise:01:input
param:llap:01:sigma:0.12
```

vkdt's default raw pipeline: i-raw, denoise, hilite, demosaic, crop, colour,
filmcurv, llap (local contrast), grade, then display sinks with a histogram.
It edits by writing these lines; there is no XMP and no database for the
edit itself (ratings and labels live in a small per-folder database).

## Library and command line

`src/lib/vkdt.h` pulls in the graph API: `dt_graph_init`,
`dt_graph_read_config_ascii`, `dt_graph_run`, `dt_graph_export`,
`dt_graph_cleanup`. `vkdt-cli` renders a `.cfg` headless. Embedding vkdt is
therefore possible at the graph level, but always on a Vulkan device it
creates (`qvk`), which matters for sharing GPU memory with Helios.

## Mapping onto the Helios contract

| vkdt | Helios today | gap |
|---|---|---|
| `params` (name, type, count, default) | `ParamSpec` (name, kind with range, default) | vkdt has counts (arrays, e.g. 4 white balance values) and string parameters; Helios has no array or string parameter kind yet |
| `params.ui` (widget, min, max, groups) | ranges live in the manifest; GUI hints not decided | ranges are UI hints in vkdt, validation bounds in Helios; groups and conditional visibility have no Helios equivalent |
| `connectors` (channels, format) | `PortSpec::Image { channels }`, f32 only | formats (f16, ui8, compressed), storage buffers (`ssbo`), geometry (`tri`), "same as input" (`&input`), wildcards |
| read / write / source / sink | inputs, outputs, `ModuleKind::Sink`; sources are framework-owned | vkdt sources are modules (`read_source`), Helios sources are loaders by design (data access is a framework prerogative) |
| module = sub-graph of nodes (`create_nodes`) | one `process` per module | the contract needs modules that expand into sub-networks: the macro concept (a node standing for a sub-network) is the same idea |
| `modify_roi_in` / `out` | `input_region`, declared overlap | `modify_roi_out` (a module changing its output size: crop, resize, lens) has no Helios equivalent yet; geometry is in the manifest's "not decided" list |
| `commit_params` | `prepare(params, context) -> plan` | equivalent |
| `check_params` (may rebuild nodes) | none | parameters that change the structure of the network |
| feedback connectors | none | multi-frame work: listed as an open question of the contract |
| GPU only, shaders | CPU reference in Rust | GPU variants of modules are planned; vkdt shaders are GLSL, wgpu speaks WGSL (and SPIR-V on Vulkan) |
| `.cfg` text network | JSON network description (helios-node) | straightforward conversion; vkdt stores GUI positions in the network, Helios keeps layout out of it (layout belongs to the user's workspace) |

## What this means for the next steps

1. **Manifest round-trip** is feasible for parameters and
   connectors once Helios adds: array parameters (count), string parameters,
   port formats beyond f32 (at least f16 and ui8), storage-buffer ports, and
   "same as input" port types. These are additions to the manifest, not
   changes to its principles.
2. **Sub-graph modules** are the biggest structural difference: more than
   half of vkdt's modules are several kernels. Helios should let a module
   declare a sub-network of nodes (its own or kernel-level), which is also
   what macros need. Done 2026-10-13: composites (module-contract.md,
   "Composites"), first used by the port of vkdt's `contrast`.
3. **Output size changes** (`modify_roi_out`) must enter the contract before
   geometry modules (crop, lens, resize) can be ported from either side.
4. **GPU sharing**: running vkdt itself (bridge) means using its Vulkan
   device or importing its buffers; porting its shaders to wgpu keeps one
   device. That choice comes with running vkdt modules.
5. vkdt's file formats are simple and stable enough that the converter
   needs no vkdt code, only the files, which keeps licensing simple (the
   BSD-2 module files are read, not copied into Helios).
