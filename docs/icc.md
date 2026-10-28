# ICC / colour-management library survey for Helios

Status: research note, 2026-10-12; the recommendation was accepted on
2026-10-27 (see "Decisions" at the end).

## What Helios needs

Context: the working space is linear Rec.2020 in f32; `unsafe` is denied in
Helios's own crates; licence GPL-3.0-or-later with the `deny.toml` allow-list
(MIT, Apache-2.0, BSD-2/3, Zlib, ISC, MPL-2.0, LGPL, ...). The colour pitfalls
note (items 9 to 11) asks for:

1. **Export tagging.** Embed an output profile in every JPEG (APP2), PNG
   (iCCP, plus cICP) and TIFF (tag 34675) for sRGB, Display P3, Adobe RGB
   (compatible), Rec.2020 and linear float; the profile must match the exact
   primaries and TRC Helios encodes with; `Exif ColorSpace = 0xFFFF` whenever
   the file is not sRGB.
2. **Input tagging.** Read embedded ICC v2 and v4 (matrix/TRC and LUT
   based), PNG cICP (and the legacy sRGB/gAMA/cHRM chunks), Exif ColorSpace
   and the interop index (`R03` = Adobe RGB); convert to linear Rec.2020 f32;
   fall back to sRGB only when nothing is tagged, and say so; never crash on a
   corrupt profile (fuzz).
3. **Speed and determinism.** Full images, SIMD or vectorisable loops;
   deterministic output for tests (bit-exact where possible, toleranced
   otherwise).
4. HDR (PQ/HLG) stays outside ICC (pitfall 11); cICP + mDCV/cLLI carry it.

### Where Helios stands today

- `helios-export` and `helios-load` both use `image` 0.25 (locked 0.25.10,
  features `jpeg`, `png`, `tiff`). The JPEG decoder is `zune-jpeg` 0.5, the
  PNG codec `png` 0.18.1, TIFF `tiff` 0.11.3.
- Export converts Rec.2020 to sRGB with Helios's own matrix and
  `srgb_encode`, then encodes 8- or 16-bit with no profile at all.
- Load applies `srgb_decode` + an sRGB-to-Rec.2020 matrix unconditionally.
- **`moxcms` 0.8.1 is already in the dependency tree**: `image` 0.25.10
  depends on it (default features) for its CICP colour-space conversions.

### What `image` 0.25.10 already offers

| Hook | JPEG | PNG | TIFF |
|---|---|---|---|
| `ImageDecoder::icc_profile()` | yes (multi-segment APP2 reassembled by zune-jpeg) | yes (iCCP, decompressed) | yes (tag 34675) |
| `ImageDecoder::exif_metadata()` | yes (raw APP1 blob) | yes (eXIf) | no (read the Exif IFD with `tiff` directly) |
| `ImageEncoder::set_icc_profile()` | yes (split into APP2 chunks, max 255 x ~64 KiB) | yes (iCCP) | yes (tag 34675) |
| `ImageEncoder::set_exif_metadata()` | yes | yes (eXIf) | no |
| cICP read / write | n/a | **no**: the `image` PNG decoder ignores cICP and the encoder cannot write it | n/a |

Other notes on `image`:

- `DynamicImage`/`ImageBuffer` carry a `Cicp` colour space and
  `apply_color_space` / `to_color_space` (backed by moxcms), but **decoders
  never set it**: every decoded buffer claims sRGB. It also only covers
  CICP-describable RGB spaces, not arbitrary ICC profiles. Helios should not
  rely on it.
- No EXIF parser: `exif_metadata()` returns raw bytes.

### What `png` 0.18.1 offers

- Read: `Info::coding_independent_code_points` (cICP), plus mDCV and cLLI
  (mastering display and content light level), `icc_profile`, `srgb`,
  `source_gamma`, `source_chromaticities`. Covered by the crate's own test
  `tests/bugfixes/cicp_pq.png`.
- Write: iCCP, sRGB, gAMA, cHRM and eXIf from `Info`, but **no cICP field on
  the write side**. `Writer::write_chunk(png::chunk::cICP, &[cp, tc, 0, 1])`
  is public and can be called after `write_header()` and before the image
  data, which is where the PNG 3rd edition requires cICP. Note: when `Info`
  has `srgb` set, `png` writes sRGB instead of gAMA/cHRM/iCCP, so leave it
  unset when embedding a profile.

## Candidates

| Crate | Licence (deny.toml) | Pure Rust / C dep | ICC read coverage | Profile writing | f32 transforms | Speed | Maintenance (2025-2026) | Notable users |
|---|---|---|---|---|---|---|---|---|
| **moxcms** 0.8.1 (latest 0.9.1) | BSD-3-Clause OR Apache-2.0: allowed | Pure Rust. `forbid(unsafe_code)` only when all SIMD features are off; the default features (enabled by `image`) use `unsafe` SIMD intrinsics inside the crate | v2 and v4; matrix/TRC with `curv` tables and `para` curves; `mft1`, `mft2`, `mAB `, `mBA ` LUTs; gray, CMYK, Lab, up to 16 inks; cicp tag | Yes: `ColorProfile::encode()`; built-ins `new_srgb`, `new_display_p3`, `new_adobe_rgb`, `new_bt2020`, `new_pro_photo_rgb`, `new_bt2020_pq/hlg`, `new_from_cicp`, ACES; colorant and TRC fields are public, so any primaries/white/TRC can be built. Always writes v4 (v4.3 when a cicp tag is present); stamps the current date in the header (non-deterministic bytes); profile ID left zero | Yes: `create_transform_f32`, extended range (values < 0 or > 1 kept); internal precision documented as about 14 bits | Runtime-dispatched SSE4.1/AVX2/AVX-512/NEON paths for 8/16-bit, fixed-point options; author's benchmarks put it ahead of lcms2 and skcms on RGB 8-bit | Very active: 0.7.x through 0.9.1 between 2025-11 and 2026-09; ~96 M downloads, ~48 M in the last 90 days | `image`, `jxl-oxide` (optional), typst, hayro (PDF), glycin (GNOME image loading), zune-imageprocs, yazi |
| **lcms2** 6.2.0 (+ lcms2-sys 4.0.7) | MIT (Little CMS itself MIT): allowed | **C dependency**: Little CMS built through `cc` (or a system library); FFI `unsafe` inside the crate | Reference implementation: everything (v2/v4, all LUT types, named colour, Lab, BPC, all intents) | Yes: `Profile::new_rgb`, tone curves, `icc()` serialisation, version selectable (v2 or v4) | Yes: `TYPE_RGB_FLT`, unbounded mode | Good; fast with optimisation plugins | Active: 6.1.1 (2025-07), 6.2.0 (2026-08); ~7 M downloads | darktable, GIMP, Krita, RawTherapee (all via the C library) |
| **qcms** 0.3.0 | MIT: allowed | Pure Rust, c2rust origin, still has `unsafe` | v2/v4 matrix/TRC, `mft1/mft2/mAB/mBA` | No serialisation (in-memory sRGB/gamma profiles only) | **No public f32 path**: RGB8/RGBA8/BGRA8/Gray8 | SIMD for 8-bit display output | crates.io release frozen since 2024-01; the live copy is vendored in Firefox | Firefox |
| skcms (reference) | BSD-3 (C++) | C++, no maintained Rust binding | Parses to matrix + parametric TRC, A2B/B2A fallback | No | f32 internally | Fast | Active in Chromium | Chromium, Skia, Android |
| **kolor** 0.1.9 | MIT OR Apache-2.0 | Pure Rust | No ICC; named spaces, matrices, transfer functions (f32, glam) | No | Yes (matrices) | n/a | Last release 2023-04, ~20 k downloads | Bevy experiments |
| **palette** 0.7.7 | MIT OR Apache-2.0 | Pure Rust | No ICC; typed colours, compile-time primaries/white | No | Yes | n/a | Active; very popular | many UI crates |
| **icc-profile** 0.0.6 | MIT OR Apache-2.0 | Pure Rust | Parser/dumper only | No | No | n/a | Experimental, ~15 k downloads | none notable |
| **jxl-color** 0.11 (in jxl-oxide) | MIT OR Apache-2.0 | Pure Rust | Detects primaries/TRC from an ICC; synthesises ICC from a JXL colour encoding | Yes, JXL-specific | own | n/a | Active with jxl-oxide | jxl-oxide |

## Recommendation: wrap moxcms (adopt it as the parser, writer and LUT engine; own the matrix/TRC path)

Reasoning:

- **Already paid for.** moxcms is in the tree through `image`; adding it as
  a direct dependency costs no new code, no new licence (BSD-3/Apache-2.0,
  already allowed) and no C toolchain. Pin the same minor as `image`
  (`moxcms = "0.8"` today) to avoid two copies; move to 0.9 when `image`
  does.
- **Coverage is complete enough.** v2/v4, matrix/TRC, parametric curves and
  all four LUT types parse; f32 extended-range transforms exist; profiles can
  be built and serialised. It is the de facto Rust CMS (image, typst, GNOME
  glycin, jxl-oxide), maintained at a fast pace.
- **lcms2 is the reference but not worth the C dependency.** It is the only
  option that beats moxcms on breadth (BPC, every intent, v2 writing), but it
  brings a C build, an FFI boundary and the memory-safety class of bug
  darktable already hit on corrupt profiles. Keep it as an *oracle* for tests
  (generate expected values offline with `transicc`/`tificc`), not as a
  dependency. Revisit only if a needed feature (soft proofing with BPC, CMYK
  print proofing) is missing from moxcms.
- **qcms, kolor, palette, icc-profile are out**: no f32 or no writer
  (qcms), no ICC at all (kolor, palette), or too young (icc-profile).
  Helios already has its own matrix code, so palette/kolor add nothing.
- **Why "wrap" and not plain adopt.** Two moxcms traits clash with Helios's
  rules:
  1. *Determinism.* The writer stamps the current date into the header, and
     the SIMD paths differ by CPU (feature unification with `image` forces the
     SIMD features on). Exports are content-hashed, so profile bytes must be
     stable, and pixel tests must not depend on the CPU.
  2. *Precision.* The f32 path is documented as roughly 14-bit and goes
     through tables. For matrix/TRC profiles (nearly all photographic
     inputs: sRGB, Adobe RGB, Display P3, ProPhoto, camera JPEGs) Helios can do
     exact, unbounded, bit-reproducible math itself, like skcms does.

  So the proposed split, in a small safe module (for instance
  `helios-photo::icc` or a `helios-colour` crate):

  - **Parse** with `moxcms::ColorProfile::new_from_slice`; on error, log,
    fall back to sRGB and flag the image ("corrupt ICC, assumed sRGB").
  - **Classify.** RGB matrix/TRC profile → extract the colorants (public
    `red_colorant`, ...) and the three `ToneReprCurve`s (`Parametric` or
    `Lut`) into a Helios `InputEncoding { to_xyz_d50: Mat3, trc: [Trc; 3] }`;
    evaluate it in Helios's own f32 loops (per-code-value LUT for 8/16-bit
    input, analytic curve for float input; then one 3x3 to Rec.2020 combined
    with D50→D65 adaptation). This path is bit-exact and autovectorises.
  - **LUT-based, gray, CMYK, Lab profiles** → `create_transform_f32` to a
    linear Rec.2020 destination built with moxcms (linear TRC,
    `allow_extended_range_rgb_xyz = true`), tested with tolerances
    (e.g. 1e-3) rather than bit equality.
  - **Write** profiles with moxcms from Helios's exact primaries and TRC,
    then normalise the header: fixed creation date (bytes 24-35), optionally
    the MD5 profile ID (bytes 84-99) so viewers can cache. One profile per
    output space, built once and cached (or generated at build time and
    committed, with a test that regenerates and compares).
  - **v2 vs v4.** moxcms writes v4 only. Every current browser and OS reads v4
    matrix/TRC profiles; if a v2 requirement shows up for old software, the
    CC0 Compact ICC Profiles (below) offer v2 files for the same spaces, or a
    ~150-line v2 matrix/TRC writer can be owned. Not decided.
  - Keep the PQ/HLG boundary transforms outside ICC (pitfall 11); moxcms's
    `new_bt2020_pq/hlg` must not be used for HDR rendering.

## Plumbing per format

### helios-export

Common: an `OutputSpace` (sRGB, Display P3, Adobe RGB compatible, Rec.2020
with BT.709/sRGB TRC (to decide), linear Rec.2020 for float TIFF) giving the
encode matrix + TRC used by `to_<space>` and the matching profile bytes and
cICP code points. A round-trip test per space: export, reload through
`helios-load`, compare in floats.

| Format | Profile | Other tags | How |
|---|---|---|---|
| JPEG | APP2 `ICC_PROFILE` chunks | Exif APP1 with ColorSpace = 1 (sRGB) or 0xFFFF; interop R03 is legacy, do not write | `JpegEncoder::set_icc_profile` + `set_exif_metadata` (Exif blob built by Helios or an Exif writer crate) |
| PNG | iCCP | cICP (primaries 1 = BT.709, 12 = P3-D65, 9 = BT.2020; transfer 13 = sRGB, 8 = linear, 16 = PQ, 18 = HLG; matrix 0; full range 1). Adobe RGB has no cICP code: iCCP only. Optionally eXIf | Drive `png` 0.18 directly (add it as a direct dependency, already in the tree): set `Info::icc_profile`, leave `srgb` unset, `write_header()`, then `write_chunk(chunk::cICP, ..)` (and mDCV/cLLI for HDR later), then the image data. `image`'s `PngEncoder` cannot write cICP |
| TIFF | tag 34675 | No Exif ColorSpace in the main IFD; if an Exif IFD is ever copied, force 0xFFFF for non-sRGB (the darktable linear Rec.2020 bug). 32-bit float TIFF for linear output | `TiffEncoder::set_icc_profile` covers 8/16-bit and `Rgb32F`; drive `tiff` directly if an Exif IFD or predictor/compression control is needed |

### helios-load

Order of precedence (proposal, matching browsers and the PNG 3rd edition):
cICP (PNG) > embedded ICC > PNG sRGB chunk > PNG gAMA+cHRM > Exif ColorSpace
1 → sRGB, or 0xFFFF with interop `R03` → Adobe RGB > untagged → sRGB with an
"assumed sRGB" flag in the image info.

| Format | ICC | cICP / chunks | Exif ColorSpace / interop |
|---|---|---|---|
| JPEG | `ImageDecoder::icc_profile()` | n/a | `exif_metadata()` raw blob → parse tags 0xA001 and interop 0x0001 (an Exif parser: `kamadak-exif`, BSD-2-Clause, allowed; or rawler's TIFF/Exif reader if exposed) |
| PNG | `icc_profile()` | Not available through `image`: open the stream with `png::Decoder` (or read the info first, then decode with `image`) to get `coding_independent_code_points`, `srgb`, `source_gamma`, `source_chromaticities`, mDCV/cLLI | `exif_metadata()` (eXIf) |
| TIFF | `icc_profile()` | n/a | Not exposed by `image`: read the Exif IFD (tag 34665) with `tiff` directly |

Then decode to f32 (8/16-bit or float samples, no premultiplication) and run
the `InputEncoding` (own matrix/TRC path) or the moxcms f32 transform for LUT
profiles. Embedded raw previews keep the current sRGB assumption unless the
preview JPEG carries its own tags.

Robustness: a `cargo fuzz` target feeding arbitrary bytes to the profile
classifier (moxcms already fuzzes its parser; Helios fuzzes its extraction
and fallback).

## Test fixtures

Prefer fixtures Helios generates and commits as its own work, with expected
values computed by an independent CMS; add third-party files only under
compatible licences.

- **Profiles, CC0: Compact ICC Profiles** (github.com/saucecontrol/Compact-ICC-Profiles).
  sRGB, Display P3, Rec.2020, ProPhoto, Rec.709, gray, and Adobe RGB
  compatible (ClayRGB-style, primaries within 1/216 of Adobe's), each in v2
  (table TRC, several sizes) and v4 (parametric). Ideal for both "read v2
  table TRC" and "read v4 parametric" tests, and as the v2 fallback for
  export if ever needed. Note the "max correctness" P3/Rec.2020 set uses a
  negative Z in red; test both sets.
- **Real-world and fuzz profiles, BSD-3: skcms `profiles/`** (Skia repo:
  `color.org/`, `mobile/`, `misc/`, `fuzz/`). Covers LUT (A2B) profiles,
  odd vendor profiles and known parser crashers: use for the robustness test
  and the "LUT path" tolerance test.
- **ICC v4 probe profile** (color.org `Probev1_ICCv4.icc`, LUT-based, renders
  different colours per intent): good to prove the LUT path is taken; check
  the ICC site terms before committing it, otherwise download in an ignored
  test.
- **cICP PNG**: the `png` crate's `tests/bugfixes/cicp_pq.png` (MIT/Apache)
  for parsing; Helios-made PNGs for sRGB/P3/Rec.2020 code points.
- **Images with known values (self-made).** Build a small patch chart
  (primaries, secondaries, greys, out-of-sRGB Rec.2020 colours, near-black
  toe values) in linear Rec.2020 f32; encode it as JPEG/PNG/TIFF in sRGB,
  Adobe RGB (compatible), Display P3 and Rec.2020 using Little CMS's
  command-line tools (`tificc`, `jpgicc`, `transicc` for the expected
  numbers) or ImageMagick with lcms. Commit the images (GPL-3 or CC0, own
  work) and the expected float values; this gives an independent oracle
  without making lcms2 a dependency. Add an Exif-only Adobe RGB JPEG
  (ColorSpace 0xFFFF, interop R03, no ICC), as cameras produce, to test the
  interop fallback; one can be made with `exiftool` on a self-made file.
- Avoid vendoring Apple's or Adobe's own profiles and the WebKit P3 demo
  images: their licences do not clearly allow redistribution.

## Decisions (2026-10-27)

The recommendation above was accepted (wrap moxcms), with these answers to
the questions it left open:

- **Rec.2020 export TRC: the BT.2020 curve** (the BT.709 camera curve, as
  BT.2020 defines it), not darktable's gamma 2.2. The ICC profile carries it
  as a parametric curve, PNG's cICP as transfer 1 (H.273 lists the same
  function as 1, 6, 14 and 15).
- **Profile version: v4 for PNG and TIFF, v2 for JPEG**, for older readers.
  moxcms writes the v4 profiles; Helios writes the v2 ones itself (a plain
  matrix/TRC display profile).
- **Exif writing: a small writer of Helios's own**, for the few tags it sets.
- **Where the code lives: a new `helios-colour` crate**, shared by export,
  load and later the display path.

What the first step built (`crates/helios-colour`, used by `helios-export`):

- Four output spaces: sRGB (the default), Display P3, Adobe RGB (1998) and
  Rec.2020. Their matrices come from the primaries and the D65 white in
  f64; the profiles' colorants are the same matrix adapted to the ICC D50
  white (Bradford), so the tag says exactly what the pixels are.
- Profiles have a fixed creation date (2026-01-01) and a zero profile ID:
  the same export gives the same bytes. The v4 profiles carry no cicp tag
  (an ICC v4.4 addition); PNG says the code points in its cICP chunk.
- JPEG: the v2 profile in APP2. PNG: the v4 profile in iCCP, plus cICP for
  every space but Adobe RGB, which has no code. TIFF: the v4 profile in tag
  34675.

What the second step built (`helios-colour`'s `InputSpace`, used by
`helios-load`):

- Pictures are read through their tags, the first usable one winning:
  PNG cICP, the embedded ICC profile, PNG sRGB, PNG gAMA (with cHRM when
  present), Exif `ColorSpace` (1 is sRGB; 2, or 0xFFFF with the interop
  index `R03`, is Adobe RGB). With none, sRGB is assumed.
- The load report (`helios_load::Colour`, also in the node's
  `load_source` answer) names the tag used and the space, and lists each
  tag skipped with why: a profile that cannot be read, a cICP curve not
  supported (PQ and HLG wait for HDR), a colour profile on a grey image.
  A broken tag never stops the load.
- Matrix/TRC profiles, cICP and the PNG chunks are converted by Helios in
  f64 (curves tabulated per code value for 8- and 16-bit files); RGB
  profiles with A2B tables go through moxcms's f32 transform to linear
  Rec.2020. Grey profiles apply their curve to a neutral. CMYK, Lab and
  device-link profiles are not supported: assumed sRGB, with the reason.
- TIFF profiles are read with a TIFF decoder of their own, not the one
  `ImageReader` makes: its default limits cap tag values at the pixel
  buffer's size and `icc_profile()` turns the error into "no profile", so
  small TIFFs would load untagged.
- Untagged pictures and raw previews now use the sRGB matrix derived from
  the xy primaries, the same as the exports, instead of the rounded
  0.4124564 one (a difference under 2.5e-4).

What the third step built (`helios-colour`'s `GamutMap`, used by
`helios-export`): the one gamut mapping.

- **Method: chroma reduction toward the neutral axis in Oklab**, the way
  CSS Color 4 maps colours for the web. A colour inside the output space is
  left exactly as it is (the mapping is colorimetric wherever it can be:
  exports keep matching their profile); so is one less than 5e-4 of its
  largest channel outside, but for that small clip (see below). A colour
  outside moves along a straight line in Oklab toward the grey of its own
  lightness, keeping its Oklab hue, to the last point of the line within
  that same margin of the space, then clipped, so that one just past the
  margin lands where one just inside it is clipped. A colour
  lighter than white goes to white, one darker than black to black. NaN
  counts as 0 and infinities as very large, so the result is always finite.
- Why this one: per-channel clipping, what exports did until now, shifts
  hue and lightness (darktable's pitfall 6). A soft compression that
  starts inside the gamut (ACES-style) would also move colours the file can
  hold, so an export would no longer say exactly what its profile tags. A
  constant-lightness, constant-hue reduction keeps both, is monotonic (more
  chroma in never gives less out), and is a published, widely used method.
  Oklab rather than CIELAB: its hue lines stay straight in the blues,
  where CIELAB turns them purple.
- A "close enough, clip" margin, far narrower than CSS's: a colour whose
  channels are less than 5e-4 of its largest channel outside [0, 1] is
  clipped, not mapped. ICC profiles round their colorants (s15Fixed16) and
  many state sRGB's primaries a little off, so a picture's edge colours
  come in up to 4e-4 outside its space (2e-5 from Helios's own v2
  profiles, 3.7e-4 from the common IEC 61966-2.1 sRGB one). The edge is
  hollow in Oklab around sRGB's blue: the straight line from grey to that
  blue leaves the space and only comes back at the blue itself, so a blue
  a hair outside was mapped to the line's first exit, and a loaded sRGB
  blue (0, 0, 254) was exported as (0, 48, 229). The margin keeps such
  colours where they are, and the search, which counts the same margin as
  inside, crosses that shallow hollow too.
- The margin is in proportion to the colour, because the profiles' error
  is: it is in their matrix. A fixed 5e-4 is a quarter of a dark blue's
  channels; the clip that ends its mapping then turned its hue by 0.07 rad
  and moved its lightness by 2e-3.
- What the margin costs, and why it cannot be avoided. Every mapped colour
  lands within the margin of the space and is clipped, so its lightness
  and hue move by what that clip moves them: under 5e-4 in lightness and
  3e-3 rad in hue on the unit tests' grid, instead of the line's exact
  values. A colour just inside the margin is clipped, and its clip has
  more chroma than the space holds at its exact lightness and hue (up to
  2e-4 more on the tests' chroma ramps). A colour just past the margin
  must have at least that chroma for the mapping to stay monotonic, so it
  cannot keep its exact lightness and hue either; nor can colours further
  out, since bringing them back to the exact line would lower their chroma
  again. Three other ways were measured and each breaks a rule: mapping
  strictly past the margin drops chroma at its edge (by 3e-3 with a fixed
  margin, 2e-4 with the proportional one); blending the
  clipped and the mapped colour across the margin drops it by up to 8e-4 and
  turns the reloaded blue to (0, 4, 253), the IEC profile's to
  (0, 43, 233); walking the line from the colour inward finds no inside
  point near the blue, which is inside only at its corner, and exports
  (0, 49, 228).
- Oklab's first matrix is scaled so that the D65 white Helios derives from
  its xy lands on the neutral axis exactly (a 1e-4 change from the
  published matrix, fitted to a white rounded differently).
- **One intent, no setting.** It is the only rendering Helios does at
  output, so no intent is offered that the engine would not implement
  (pitfall 6). A choice of method can be added later if a need shows up.
- Used by every export path: JPEG, PNG (8 and 16 bits), TIFF, and the
  clipboard's images, all through the same conversion.
- **Not yet on screen.** The viewport, the library's thumbnails and the
  mask display still clip each channel to sRGB: the screen side waits for a
  per-window display target, and then takes the same `GamutMap` with that
  display's space as its destination.
- Tests: colours inside unchanged, colours outside land inside, Oklab hue
  within 0.01 rad and lightness within 5e-4, chroma monotonic along ramps
  of constant lightness and hue, finite results for NaN, infinities and
  huge values, and every export format holding the same mapped values.
