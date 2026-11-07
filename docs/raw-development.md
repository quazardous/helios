# Raw development: where raws go, and how far from darktable

The goal: a raw opens, is edited and exports through a real raw chain
(its sensor data, demosaiced and coloured by Helios's modules), never
through the JPEG preview the camera embedded in it, with a default look
on a par with darktable's. This page is the audit of where that holds
today (step A), the parity measured against darktable-cli, and the plan
for the two steps that follow: raws through the raw chain by default (B)
and a darktable-level default look (C).

Line numbers are those of the commit that added this page.

## 1. Where a raw goes

Two entry points read a raw file, both in `helios-load`:

- `helios_load::load` (`crates/helios-load/src/lib.rs:92`): the sensor
  mosaic through rawler, one channel in raw units, cropped, with the
  camera's description (levels, filter pattern, matrix, white balance);
- `helios_load::preview` (`lib.rs:116`) and `helios_load::look`
  (`lib.rs:164`): the largest embedded preview, an sRGB JPEG taken to
  linear Rec.2020.

Which one a path takes is decided in one place on the desktop:
`helios_gui::templates::open_file` (`crates/helios-gui/src/templates.rs:85`).
When the workspace's network has a raw source (`raw_sources` in its
template, `NetworkDesc::reads_raw`,
`crates/helios-template/src/format.rs:398`) the file's mosaic is loaded
(`templates.rs:95`); otherwise a raw gets its embedded preview
(`templates.rs:111`). Of the built-in templates only "Develop one image"
(`templates/develop.helios-template`) has a raw source; "Develop
(preview)", "Batch export", "Import, cull and dedupe", "Portrait retouch"
and the blank workspace (`helios_template::workspace::blank_network`,
`crates/helios-template/src/workspace.rs:573`) are picture networks.

| Path | What a raw goes through | Where |
|------|-------------------------|-------|
| Library thumbnails (grid, filmstrip) | **embedded preview**, always; a picture is decoded. The edit is never shown in a thumbnail | `crates/helios-gui/src/main.rs:104-118` (`show_library`), scaled by `helios_gui::library::small` (`library.rs:440`) |
| Light table: Compare panel, culling, stacks | **embedded preview**: they draw the same thumbnails | `crates/helios-gui/src/library.rs:1334` (`compare_ui`) |
| Light table: the loupe (an image of the library opened in the viewport) | the workspace's network: **sensor data** in a "Develop one image" workspace, **embedded preview** in every other | `main.rs:467` (`open_from_library`) → `main.rs:329` (`image_graph`) → `templates.rs:85` |
| Opening a file in a workspace (darkroom) | the same rule | `main.rs:364` (`prepare`) → `image_graph` → `templates.rs:85` |
| "Develop one image" | **sensor data**: the node's raw chain, the same pixels as the node's (`crates/helios-gui/tests/develop_raw.rs`); a picture is refused, a raw that is not a mosaic (sRAW, LinearRaw DNG) refused too | `templates.rs:87-107`, `templates/develop.helios-template/template.json` |
| Saved edit reopened (darkroom) | the workspace's network, the version's edit applied; edit nodes the network has no place for are skipped | `crates/helios-gui/src/persist.rs:286`, `crates/helios-vault/src/edit.rs:136` |
| Batch export from the library | the **workspace's** network: **sensor data** in a "Develop one image" workspace, **embedded preview** in any other, the "Batch export" template included | `crates/helios-gui/src/export.rs:40-63` (`render_item` → `templates::open_file`) |
| Export from a workspace | the same (`library.export` is the only export; it takes the workspace's bundle) | `crates/helios-gui/src/shell/export.rs:208`, `main.rs:710-713` |
| CLI and node (`helios-node`, `helios batch`, `load-source`, workflow runs) | **sensor data**, always: a raw source is the mosaic with its camera; the network must hold the raw chain itself (no chain is added, no preview is taken) | `crates/helios-node/src/lib.rs:471` (`load_in`), `crates/helios-node/src/main.rs:597` (`load_source`), `crates/helios-node/src/workflow/runs.rs:1970` |
| darktable sidecars (`helios import-darktable`, import with darktable edits, `helios batch --sidecar`) | the history is mapped onto Helios's raw modules (`rawprepare`, `temperature` → `whitebalance`, `highlights`, `demosaic`, `colorin`, `exposure`, `sigmoid`, `crates/helios-xmp/src/import.rs:431`), kept as a version; it renders through sensor data only where the network has those nodes | `crates/helios-library/src/darktable.rs:34`, `crates/helios-library/src/import.rs:670-695` |
| darktable library importer (plugin) | metadata only (stars, flags, labels, tags, title): Helios imports the files itself, sidecars as versions as above; darktable's own thumbnails (mipmaps) are not read | `plugins/importers/darktable-library/src/lib.rs:1-16` |
| Import signals (near-duplicates, sharpness) | **embedded preview** (a small decode of the mosaic when there is none): enough for a hash, kept so | `crates/helios-library/src/signals.rs:281-287` |

### The known limit: a raw in a picture network exports from its preview

It happens in `templates::open_file` (`templates.rs:108-114`): a network
without a raw source is built with `Source::Picture`, and a raw file
gives it `helios_load::preview`. The batch exporter renders each item
through that same function (`export.rs:46`), so the "Batch export"
template, the blank workspace and every picture template export a raw at
the size and look of its camera JPEG (often 1620 px to full size,
already tone-mapped by the camera), then apply exposure and sigmoid a
second time on top.

Why: a picture network starts at `exposure` on a four-channel linear
Rec.2020 image. The mosaic is one channel in raw units, and nothing in a
picture network turns it into an image (levels, white balance,
demosaicing, the camera matrix are nodes of the raw chain), so the only
RGB image at hand was the preview. The darktable edits a raw carries make
it worse there: their raw items find no node (`edit.rs:140-143` skips
them), only the exposure and sigmoid apply, on top of the camera's JPEG.

## 2. The raw chain today, against darktable's defaults

darktable's default for a raw (5.x, the "scene-referred (sigmoid)"
workflow and the "modern" chromatic adaptation, both its configuration
defaults) runs, in pipe order: rawprepare, temperature (camera reference,
D65), highlights (inpaint opposed), demosaic (RCD on Bayer, Markesteijn
one pass on X-Trans), colorin (standard matrix, or the enhanced one where
it has it), color calibration (`channelmixerrgb`: CAT16 adaptation from
the shot's illuminant), exposure (+0.7 EV, black -0.000244, compensating
the camera's exposure bias), sigmoid (per channel, contrast 1.5), colorout
(sRGB), flip (the EXIF orientation), gamma. Lens correction, denoising
and sharpening are not applied by default.

| Stage | In Helios | Module or file | Helios default | darktable default |
|-------|-----------|----------------|----------------|-------------------|
| Decode | yes, rawler 0.8 (not rawspeed, not LibRaw); sRAW, LinearRaw DNG and monochrome refused as "not a mosaic" | `crates/helios-load/src/lib.rs:92` | rawler's recommended crop, filter pattern shifted to it | rawspeed (LibRaw for CR3), rawspeed's crop |
| Black and white levels | yes | `modules/rawprepare` | per 2 x 2 position, black measured on the masked photosites | the same; plus user overrides and the DNG gain map (not in Helios) |
| White balance | partly | `modules/whitebalance` | as shot (the camera's coefficients) | camera reference D65 here, the shot's white done by color calibration |
| Highlight reconstruction | clip only | `modules/highlights` | clip at 1.0 (x 0.987) | inpaint opposed |
| Demosaic, Bayer | yes | `modules/demosaic/src/rcd.rs` | RCD | RCD |
| Demosaic, X-Trans | yes | `modules/demosaic/src/xtrans.rs` | Markesteijn one pass | Markesteijn one pass |
| Camera matrix, input profile | standard matrix only | `modules/colorin` | the camera's matrix to linear Rec.2020 | the same, the enhanced matrix for some cameras, ICC input profiles |
| Colour calibration (CAT) | **no** (vkdt's `vkdt_colour` does a CAT16, not used by any template) | `modules/vkdt/src/colour.rs` | none | CAT16 from the shot's illuminant |
| Exposure | yes | `modules/exposure` | 0 EV, bias not compensated | +0.7 EV, bias compensated |
| Tone mapper | sigmoid, per-channel method | `modules/sigmoid` | darktable's defaults (contrast 1.5, skew 0, targets 100 % and 0.0152 %, hue 100 %) | sigmoid, the same defaults |
| Filmic | **no** (vkdt's film curve `vkdt_filmcurv` exists, unused) | | | the "filmic" workflow only |
| Lens correction | **no** | | | off by default |
| Denoise | **no** | | | off by default |
| Orientation (flip) | **no**: a portrait raw shows on its side | | | auto, from EXIF |
| Output profile | yes, at export and on screen | `helios_export::Output` (`crates/helios-export/src/lib.rs:436`), `helios-colour` | sRGB (Display P3, Adobe RGB, Rec.2020 offered) | sRGB |

GPU: of the raw chain, only exposure and sigmoid have GPU kernels
(`GpuKernel`); rawprepare, white balance, highlights, demosaic and colorin
run on the CPU (`helios-compute` routes a whole render to one device).

## 3. Parity on the bench box

The `raw-parity` job (`tools/bench/Benchfile`, `helios-parity`'s
`raw-parity` binary) develops three raws of the corpus with Helios's
"Develop one image" at its defaults and compares them, in Delta E 2000 as
linear Rec.2020 inside a 12-pixel margin (`helios_parity::compare`), with
darktable-cli built from the classic pin of the mask parity:

- **chain**: darktable running Helios's modules (no workflow; white
  balance as shot, highlights clipped, its default demosaic, standard
  matrix, sigmoid at its defaults), judged by darktable's own thresholds
  with the max at the 99.9th percentile (mean <= 0.77, p99.9 <= 2.3);
- **defaults**: darktable's own defaults, measured.

Each line gives the full-resolution difference and the one of 8 x 8 block
means (colour and tone without demosaicing detail), and the exposure gap
(log2 of darktable's mean luminance over Helios's). `out/shots/<id>.png`
puts Helios, darktable with Helios's chain and darktable's defaults side
by side.

Run of 2026-11-07 (box job `20261107-162158-0cb3`, darktable
5.7.0+1260~gca5a02038b), Delta E 2000:

| raw | Helios size | darktable size | shift | chain mean | chain p99.9 | chain 1/8 mean | chain | defaults mean | defaults p99.9 | defaults 1/8 mean | defaults EV |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Canon EOS 350D, CR2 (Bayer) | 3456x2304 | 3474x2314 | (-10, -5) | 0.720 | 1.253 | 0.715 | pass | 8.451 | 16.672 | 8.407 | +0.82 |
| Nikon D300, NEF 12-bit lossless (Bayer) | 4288x2848 | 4320x2868 | (-15, -10) | 0.258 | 1.810 | 0.149 | pass | 11.009 | 23.171 | 10.907 | +0.71 |
| Fujifilm X-Pro1, RAF (X-Trans) | 4896x3264 | 4952x3288 | (-28, -12) | 0.172 | 0.596 | 0.167 | pass | 9.672 | 33.398 | 9.227 | +0.65 |

What it says:

- **The ported chain matches darktable** on all three: the same modules
  give darktable's pixels within its own thresholds. The CR2's mean (0.72,
  close to the 0.77 bound, and as large at 1/8) is a colour offset, not
  demosaicing: the camera matrix or levels rawler gives differ a little
  from rawspeed's for that body. Worth a look in C6.
- **The default look is far**: a mean of 8.5 to 11 Delta E, the same with
  detail averaged out, so tone and colour, not sharpness. The exposure
  gap (+0.65 to +0.82 EV) is darktable's +0.7 EV default with the bias
  compensation; the rest is the white balance (darktable's colour
  calibration renders the NEF's grey background neutral, Helios's as-shot
  coefficients leave it cool) and the highlights (clip against inpaint
  opposed, in the RAF's sky).
- **Crops differ**: rawler's recommended crop is 18 to 56 pixels narrower
  than rawspeed's, and shifted. Drawn masks are placed in the sensor's
  frame (`helios_xmp::drawn::Frame`), so this does not move them, but an
  export is a little smaller than darktable's.

A first run with a 24-pixel alignment reach failed the RAF (its shift is
28 pixels): the reach is now 64.

## 4. Plan

### B: raws through the raw chain by default

Status (2026-11-07): steps 1 to 4 are done. The raw front is
`templates/composites/helios.raw-front@1.helios-composite.json`; a
template's `image_sources` take a picture or a raw (see
[templates.md](templates.md)); the built-in templates (at version 2) and
the blank workspace use them; the batch exporter renders through
`helios_gui::templates::develop_file`, which never takes a preview; and
`edit::apply_to_graph` maps darktable's `temperature` item onto the
`whitebalance` node (the import names items after darktable's operations,
so that item was skipped even in "Develop one image"). Step 5 is larger
than sized (see its entry).

Each step is one branch, built and tested on its own. Sizes: S a day or
less, M a few days, L a week or more.

1. **A raw front for any network** (M). A shipped composite,
   `helios.raw-front@1` (rawprepare, white balance, highlights, demosaic,
   colorin: the first five nodes of "Develop one image"), and a third
   source kind, "image" (a picture or a raw), in the template format: built
   with a raw, the source is the mosaic followed by the raw front; with a
   picture, the picture itself. Targets: `crates/helios-template/src/build.rs`
   (`Source`, `SourceKind`), `crates/helios-template/src/format.rs`
   (`source_kind`, `reads_raw`), `templates/composites/`,
   `templates/schema/`, `crates/helios-gui/src/templates.rs:85-117`
   (`open_file` gives a raw's mosaic to an "image" source instead of its
   preview). Tests: `crates/helios-template/tests`, and
   `crates/helios-gui/tests/develop_raw.rs` extended to a picture template.
2. **The built-in templates and the blank workspace read raws** (S). Their
   source becomes "image": "Develop (preview)", "Batch export", "Import,
   cull and dedupe", "Portrait retouch" (`templates/*/template.json`) and
   `blank_network` (`crates/helios-template/src/workspace.rs:573`).
   Workspaces made from an older copy keep theirs: the update is offered
   as for any template version. "Develop one image" and "Develop
   (preview)" may then merge into one template (not decided).
3. **Exports never take a preview** (S). With 1 and 2 the batch exporter
   (`crates/helios-gui/src/export.rs:46`) gets the sensor data in every
   built-in workspace; a network that still cannot take a raw (a user's
   picture-only template) fails the item with a reason instead of
   exporting the preview (`templates.rs:111` kept for display only).
   Test: `crates/helios-gui/tests` (a raw exported from "Batch export"
   matches "Develop one image" at the same settings), and the
   `raw-develop` job.
4. **darktable edits land on the raw chain** (S). With 1 the nodes the
   XMP import maps (`crates/helios-xmp/src/import.rs:431`) exist in every
   workspace, so `edit::apply_to_graph` stops skipping them
   (`crates/helios-vault/src/edit.rs:140`). Name the raw front's nodes as
   the import does (`rawprepare`, `whitebalance`...), or map them in
   `apply_to_graph`. Test: `crates/helios-library/tests` with a fixture
   sidecar.
5. **Orientation** (S). The loader reports the EXIF orientation
   (`crates/helios-load/src/lib.rs`, `Loaded`), a `flip` step (a module or
   the raw front's last node) applies it, masks follow it
   (`helios_xmp::drawn::Frame`). darktable applies it by default.
   **Re-sized M to L (2026-11-07)**: the engine assumes every module
   keeps the geometry it is given (`Graph::extent` follows the first
   input up to the source), so a quarter turn needs a geometry hook in
   `helios-core` (a module's output extent from its input's, and
   `input_region` mapping back), a flip module whose regions may start
   off the origin, the viewport and the export taking such an extent,
   and `Frame` gaining the orientation for darktable's masks. Rotating
   the mosaic at load time (with its filter pattern and per-position
   black levels) is the cheaper route, but gives no step to override.
6. **A loupe from the raw chain** (M). The viewport opened from the
   library already renders the workspace's network (`main.rs:467`); the
   Compare panel (`crates/helios-gui/src/library.rs:1334`) draws
   thumbnails: give it a render of each image's head version at the
   panel's size, through the engine and its cache, the preview drawn
   until it lands.
7. **Thumbnails from the edit** (L). Keep the embedded preview for the
   first paint (fast, `main.rs:104-118`), then replace each thumbnail with
   a small render of its head version through the workspace's chain,
   cached by image hash, version and chain version (a vault table or a
   cache folder in `helios-library`), regenerated when the edit changes.
   Needs a cheap render: demosaic at reduced size (darktable's preview
   pipe downsamples the mosaic first; `modules/demosaic` gains a
   half-size path, the engine asks for it through the region's scale).
8. **The raw chain on the GPU** (L, optional for B). rawprepare, white
   balance, highlights and colorin are per-pixel and easy to port to
   WGSL; demosaic (RCD, Markesteijn) is the large one. Until then a
   render with a raw front runs on the CPU (`helios-compute`).

The signals of an import keep the embedded preview (a hash wants speed,
not colour). ROADMAP: record B as decided when it is accepted.

### C: a default look on a par with darktable's

Ordered by what the parity says weighs most. Each step reruns
`raw-parity` and records its numbers here; the "defaults" columns are the
measure of C.

1. **Exposure as darktable's scene-referred default** (S). For a raw,
   exposure +0.7 EV, black -0.000244 and the camera's exposure bias
   compensated (0 EV for a monochrome sensor), as darktable's
   `reload_defaults` (`src/iop/exposure.c`). The exposure gap measured is
   +0.65 to +0.82 EV, the size of this default. Targets: the exposure
   node of `templates/develop.helios-template/template.json` and of the
   raw front (B1); `modules/exposure` keeps its identity defaults for
   pictures. Test: `raw-parity`'s EV column near 0.
2. **Orientation** (S): B5, if not done there.
3. **Colour calibration and white balance as darktable's modern
   default** (L). A `colorcal` module ported from `channelmixerrgb`'s
   chromatic adaptation (CAT16, the illuminant estimated from the shot's
   coefficients, "as shot to reference"), and the white balance's
   "camera reference" (D65) mode (`modules/whitebalance`, listed there as
   not ported). darktable's code is in `src/iop/channelmixerrgb.c` and
   `src/common/chromatic_adaptation.h`; vkdt's `vkdt_colour`
   (`modules/vkdt/src/colour.rs`) already has a CAT16 to reuse. A parity
   test in `crates/helios-parity/tests/` against darktable with that
   module alone. Expected to take most of the remaining colour gap (the
   grey background of the NEF: neutral in darktable, cool in Helios).
4. **Highlight reconstruction, inpaint opposed** (M). darktable's default
   method (`src/iop/hlreconstruct/opposed.c`) in `modules/highlights`,
   clip kept as a choice. Matters on skies and speculars; a parity test
   as the clip one.
5. **colorin's enhanced matrices and gamut clipping** (S to M), listed as
   not ported in `modules/colorin`.
6. **Corpus breadth** (M). Extend `raw-parity` to one file per decoder
   path of the corpus once 1 to 4 land, and look into any chain row that
   fails (crops and matrices that differ between rawler and rawspeed).
7. **Judge the defaults** (S). When the "defaults" rows reach darktable's
   thresholds (mean 0.77, p99.9 2.3) on the three raws, judge them in
   `raw-parity` (a failure fails the job), and add the job to the
   affected selection for `modules/*` and `helios-load`
   (`xtask/src/affected.rs`).

Not darktable defaults, so not in C: lens correction (lensfun or the
makers' embedded data), profiled denoise, sharpening, filmic and AgX as
other tone mappers. They are worth their own steps afterwards.
