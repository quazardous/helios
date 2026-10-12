# Raw corpus: decoder and camera-name regression set (draft)

Companion to `../tests/raw-corpus.toml` (59 files, 21 maker labels, 19 container
formats). Answers pitfalls 18 (decoder regressions on upstream bumps) and 20
(camera identity normalisation) of `docs/darktable-pitfalls/library-files.md`.
(The brief said items 7 and 8, but in that file those cover XMP sync and
startup scans. The corpus items are 18 and 20, and "Top 5" point 4.)

Data gathered 2026-10-12 from metadata only. No raw file was downloaded.

## 1. Licence

raw.pixls.us licenses each file separately. Most files are CC0, but not all.

- The upload form on https://raw.pixls.us/ requires this declaration:
  "I declare that I own full rights to this file and I hereby release it
  under the [CC0] license into the public domain." The link points to
  https://creativecommons.org/share-your-work/public-domain/cc0/.
- The repository table is fed by
  `https://raw.pixls.us/json/getrepository.php?set=all` (DataTables JSON).
  Each row has a licence column. On 2026-10-12 the 2016 rows were:
  - 1870 rows: "Creative Commons 0 - Public Domain" (links to
    `creativecommons.org/publicdomain/zero/1.0/`).
  - 146 rows: "Creative Commons - Attribution, Non-Commercial, ShareAlike 4.0".
    These are mostly old imports from rawsamples.ch, for example Nikon D1,
    FinePix F700, Sigma SD9/SD10, Kodak Z1015 IS and several Panasonic TZ71.
  - `?set=noncc0` lists the bodies whose samples are not CC0. The home page
    links it as "View only the non-cc0 samples".
- The same JSON publishes a **sha256** and a rounded size for every file. The
  manifest copies them, so no entry needs "to compute on fetch" for the hash.
  Record the exact byte size on first fetch.
- `github.com/pixlsus/raw-pixls-us` returns 404 (GitHub API). The site
  offers full mirrors as `https://raw.pixls.us/data.lfs.git` (git LFS) and
  `data.annex.git` (git-annex).

**Rule for the corpus: CC0 only.** CC BY-NC-SA would put a non-commercial
clause into a GPLv3 project's test data, so the four NC candidates were
replaced with CC0 files:

| NC candidate | CC0 replacement |
|---|---|
| Nikon D1 | D1X |
| FinePix F700 | FinePix S2Pro |
| Sigma SD9 | Sigma DP1 |
| Kodak Z1015 IS | Kodak P712 |

The fetch tool should still read the licence column again and refuse a
non-CC0 row, because licences on the site can change.

## 2. Selection rationale

These rules guided the choices:

- **One file per decoder path, not per body.** The set covers every rawler
  decoder module Helios can meet: crw, cr2, cr3, nef, arw, raf, orf, rw2,
  pef, dng, tfr (3FR/FFF), iiq, mos, srw, mrw, kdc and x3f. Within a format
  it covers the compression variants:
  - **Nikon:** lossy type 1, lossless, small NEF, and High Efficiency.
  - **Sony:** ARW1, ARW2 curve, uncompressed, and lossless LJPEG including
    the odd 2x2 MCU variant.
  - **Fujifilm:** SuperCCD, X-Trans uncompressed, lossless and lossy
    compressed, Bayer, and GFX.
  - **Canon:** CR2 RAW and sRAW, and CR3 RAW, C-RAW and dual-pixel.
  - **Panasonic:** v4 and v8 compression.
  - **DNG:** lossless JPEG, lossy JPEG, 8-bit linearised, LinearRaw
    (ProRAW), monochrome, and a camera DNG with a rare 1-component LJPEG.
- **Known trouble first.** The set includes the bodies darktable broke:
  - GFX 100, which rendered purple in darktable 5.0.
  - Olympus E-410 and E-510, which broke in darktable 5.4.
  It also includes files raw.pixls.us explicitly keeps for unusual data:
  - Nikon 1 J1 with its unique Huffman table.
  - DMC-LX1 with an odd LJPEG tile width.
  - ILCE-7RM5 with 2x2 MCU tiles.
  - X100S with a 1-component LJPEG.
  - D1X with non-square pixels.
- **Monochrome:** Leica M Monochrom (Typ 246) DNG and Pentax K-3 Mark III
  Monochrome PEF.
- **Old formats:** EOS D30 CRW, Minolta Dynax 7D and Alpha-7 MRW, Kodak KDC,
  Nikon D1X and D100.
- **Phones:** Apple iPhone 12 Pro ProRAW and Google Pixel 7 Pro DNG.
- **Same shot, two containers:** Pentax K-r as both PEF and DNG. The camera
  name must come out identical for both.
- **Expected failures (3), so a regression shows up as a change in either
  direction:**
  - Nikon Z50_2 "DX HE" (High Efficiency). rawler 0.8 explicitly returns
    `NEF compression HighEfficency is not supported`.
  - Sigma DP1 X3F. rawler has an X3F parser but no Sigma camera
    definitions, so `check_supported` fails.
  - "HASSELBLAD Lunar" ARW, a rebadged NEX-7 that neither rawler nor
    rawspeed lists.

  Each of these must fail cleanly with an error. A panic or a wrong image
  counts as a failure.
- **Smallest equivalent file.** Sizes run from 2.85 MB to 100.5 MB.
  - **Core tier** (53 files, about 729 MB): runs on every rawler bump.
  - **Heavy tier** (6 files over 30 MB, about 328 MB): runs on releases.
    The files are GFX 100, H3D, H5D-40, ILCE-7RM5, DC-S5M2 and K-3 III Mono.
- **Not covered:** raw.pixls.us has **no JPEG XL DNG** (DNG 1.7, newer ProRAW
  and Pixel files). rawler 0.8 has a jxl-oxide path for compression 52546
  that this corpus cannot test, so it needs a separate source.
  Nikon HE\* and Z8/Z9 HE are covered only by the one HE expected-failure.
  No Phase One IIQ L sample was small enough. H 25 IIQ S covers IIQ.

### Test assertions per file (suggested)

For each file, the corpus test should assert:

- The decoder name and rawler version.
- Width, height and cpp.
- The CFA pattern after Helios shifts it to the crop.
- The crop and active area.
- Black and white levels, and WB coefficients.
- Orientation.
- `make`/`model` as reported by rawler, and the canonical id after the
  mapping table.
- A hash of the decoded mosaic.

Store the expected values next to the manifest and regenerate them only
deliberately when rawler is bumped. This follows the
`.analyze.yaml` + `.digest.txt` pattern described in section 3.

## 3. rawler's own corpus (do not duplicate)

rawler 0.8.0 (crates.io; `Cargo.lock` pins 0.8.0) ships golden data, not
samples:

- `tests/supported_rawdb_sets.txt` has 811 make/model/set entries.
  `tests/rawdb/mod.rs` is generated by `rawdb_regen_tests.py`, with one test
  per file, behind the `rawdb` feature.
- `data/testdata/rawdb/<Make>/<Model>/<set>/` holds 1614 `*.analyze.yaml`
  files. Each has the file size, the file MD5, raw params (crop, active
  area, black and white levels, WB), the clean make and model, and EXIF.
  Next to them are MD5 digests of:
  - raw pixels (`.digest.txt`),
  - the full development (`.digest.full.txt`),
  - the preview,
  - the thumbnail.
- The raw files come from **rawdb.dnglab.org**
  (`https://rawdb.dnglab.org/api/download/<make>/<model>/<path>`), not from
  raw.pixls.us. The cache is set with `RAWDB_CACHE` ("~100 GiB"), and
  `RAWDB_API_KEY` is optional. That site does not state a licence on its
  front page.
- rawler wraps `decode` in `catch_unwind`, so a decoder panic becomes an
  error. Aborts and running out of memory are not caught.
- rawler also has stubs to watch:
  - The `arw6` decompressor (Sony compression 32766) is `unimplemented!()`.
    It is unreachable today because no camera TOML declares mode `arw6`.
  - The DNG decoder has a `todo!()` for unknown PhotometricInterpretation
    values.

What overlaps and what Helios adds:

- 50 of the 59 picks are bodies that rawler already tests on its own files,
  so their pixel decode is covered upstream.
- The Helios corpus checks what rawler does not:
  - Helios's own CFA-to-crop shift (pitfall 18).
  - Camera names against darktable and rawspeed (pitfall 20).
  - Same-shot PEF/DNG consistency.
  - Behaviour of unsupported files.
  - Bodies rawler does not test: D6 small NEF, Z 6_2, D-LUX (Typ 109),
    K-3 III Monochrome, Ricoh GR, Sigma DP1, Lunar, iPhone 12 Pro and
    Pixel 7 Pro.
- Do not copy rawler's digests. Its source files are different files.

## 4. Camera names: darktable/rawspeed vs rawler

**darktable (rawspeed).** The EXIF strings are read by exiv2, trailing
spaces are trimmed, and then `dt_rawspeed_lookup_makermodel` runs:

- It calls `CameraMetaData::getCamera(make, model)` against `cameras.xml`,
  where each `<Camera make= model=>` key holds the raw EXIF strings.
- On a hit, darktable stores three names:
  - `canonical_make` from `<ID make=>`,
  - `canonical_model` from `<ID model=>`,
  - `canonical_alias` from `<Alias id=>`, or the model itself.

  Regional names (Kiss, Rebel, Maxxum and so on) are `<Aliases>` of one
  `<Camera>`, so they share the canonical model and differ only in the
  alias. Noise profiles, presets and camera styles key on canonical
  maker/model.
- Bodies marked `supported="no"`, such as CR3 bodies decoded through LibRaw,
  still carry an `<ID>`. If rawspeed has no entry, darktable tries the LibRaw
  `modelMap` (CR3). If that also misses, it passes the EXIF strings through
  unchanged.

**rawler.** Each camera is a `data/cameras/<maker>/*.toml` with:

- `make`/`model`: the EXIF strings, matched after `trim_end`.
- `clean_make`/`clean_model`: the names it reports.
- `model_aliases = [[exif, clean], ...]`.

An alias produces the **alias's own clean model** as the reported model.
There is no shared canonical name and no separate alias field. For DNG,
rawler reuses the clean names when the body is in its catalog. Otherwise it
reports the raw EXIF make/model (`dng.rs`), with no trimming of company
suffixes.

**Measured on the actual data.** rawler 0.8.0 TOMLs were compared with the
rawspeed `cameras.xml` in the darktable fork (rawspeed submodule commit
c835b05, 2026-07-28), joining on the exact EXIF make/model:

- rawler has 857 keys and rawspeed has 947. 732 keys exist in both.
- Of those 732, **140 report a different name**:
  - **116 are alias cases.** rawler reports the regional name, while rawspeed
    reports the canonical body name with the regional name as alias.
  - **24 differ outright** (some differ in make, some in model).

The full list is in `meta/name_diffs.txt`. Examples:

| EXIF make / model | rawler reports | rawspeed canonical (alias) |
|---|---|---|
| Canon / Canon EOS Kiss Digital N | Canon / EOS Kiss Digital N | Canon / EOS 350D (EOS Kiss Digital N) |
| Canon / Canon EOS 1500D | Canon / EOS 1500D | Canon / EOS 2000D (EOS 1500D) |
| KONICA MINOLTA / ALPHA-7 DIGITAL | Minolta / ALPHA-7 | Minolta / Dynax 7D (Alpha-7 Digital) |
| KONICA MINOLTA / DYNAX 7D | Minolta / DYNAX 7D | Minolta / Dynax 7D |
| NIKON CORPORATION / NIKON Z 6_2 | Nikon / Z 6 2 | Nikon / Z 6_2 |
| NIKON CORPORATION / NIKON Z50_2 | Nikon / Z 50 II | Nikon / Z50_2 |
| OM Digital Solutions / OM-1MarkII | OM Digital Solutions / OM-1 Mark II | OM System / OM-1 Mark II |
| FUJIFILM / GFX100S | Fujifilm / GFX 100S | Fujifilm / GFX100S |
| Panasonic / DC-FZ10002 | Panasonic / DC-FZ1000M2 | Panasonic / DC-FZ10002 |
| Hasselblad / Hasselblad H3D | Hasselblad / H4D-H3D | Hasselblad / H3D |
| Phase One A/S / H 25 | Phase One / H 25 | Phase One / H25 |
| EASTMAN KODAK COMPANY / KODAK EASYSHARE Z1015 IS DIGITAL CAMERA | Kodak / EasyShare Z1015 IS | Kodak / Z1015 IS |
| NIKON / E8800 | Nikon / Coolpix E8800 | Nikon / E8800 |
| LEICA / V-LUX 4 | Leica / V-LUX 4 | Leica / V-Lux 4 |
| RICOH IMAGING COMPANY, LTD. / GR (DNG) | RICOH IMAGING COMPANY, LTD. / GR (EXIF passthrough, not in rawler data) | Ricoh / GR |
| SIGMA / SIGMA fp (DNG) | SIGMA / SIGMA fp (passthrough) | Sigma / fp |

Other naming traps found while building the manifest:

- **Trailing padding.** EXIF strings carry trailing spaces. Examples:
  `OLYMPUS IMAGING CORP.  `, `E-410           `, and `PENTAX K-3 Mark III
  Monochrome` followed by about 30 spaces. Both libraries trim the end, and
  Helios must do the same before any lookup.
- **Different EXIF source tags.** The two libraries can disagree even on the
  key. For the Leaf Aptus 22, rawspeed keys on `Creo/Leaf` /
  `Leaf Aptus 22(LF3779     )/Hasselblad H1`, while rawler keys on `Leaf` /
  `Leaf Aptus 22`.
- **raw.pixls.us names its own way.** Usually it uses rawspeed canonical
  names (`Z 6_2`, `Dynax 7D`, `OM System`). Sometimes it uses the alias
  (`EOS Kiss Digital N`, `Maxxum 7D`), the raw EXIF string
  (`KODAK P712 ZOOM DIGITAL CAMERA`), or a doubled make
  (`Hasselblad H4D`, `Canon PowerShot A650`). Its labels are not an identity
  source.
- **Missing bodies.** A body that neither library knows, such as
  HASSELBLAD Lunar, gets no canonical name at all. The camera-identity table
  needs an "unknown, EXIF as-is" state that is visible in the UI.

**Consequence for Helios.** Neither name can be used as the key on its own.
The identity table should map four inputs to one canonical id:

- the raw EXIF make/model (trimmed),
- rawler's clean name,
- rawspeed's canonical make/model plus alias,
- any aliases.

Seed it by joining the two catalogs on EXIF strings, as the comparison
script does. Test it with every entry of darktable's `noiseprofiles.json`,
plus every corpus file's `exif_*`, `rawler_name` and `rawspeed_name`. In the
manifest, `rawler_name` and `rawspeed_name` are predictions from the catalog
data. The corpus test must assert the names rawler really reports at decode
time.

## Files in this folder

- `../tests/raw-corpus.toml`: the manifest (59 `[[file]]` entries, all CC0, with sha256
  from raw.pixls.us).
- `meta/repo_all.json`: the raw.pixls.us repository JSON.
  `meta/all.tsv` is the same data flattened.
- `meta/select.tsv` lists the selection rules, and `meta/chosen.tsv` holds
  the resolved rows.
- `meta/exif/*.txt` holds the raw.pixls.us exiv2 dumps (text) for the
  selected files. `meta/exifnames.tsv` has the exact EXIF strings.
- `meta/name_diffs.txt` holds the rawler vs rawspeed name divergences.
- `scripts/`: the parse, resolve, compare-names and generate scripts
  (Python, standard library only).
