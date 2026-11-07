# Camera, lens and noise profiles: research

Status: **research, not decided** (2026-11-07). It answers: what data a
raw developer needs for each camera and lens, who maintains it and how
good it is, what photographers expect, how other editors ship it, and
whether Helios should carry profiles as plugins, as manifest files or as
something else.

**Owner direction (2026-11-07):** Helios tries to stay compatible with
darktable's profiles. It reads darktable's noise profiles
(`noiseprofiles.json`), its colour matrices, its lensfun use and its
per-camera data as they are, and a user's darktable data works in Helios
without conversion. The data packages of section 5 carry them in
darktable's own formats. Where Helios goes further (DCP input, quality
labels), it adds to darktable's data and never replaces its formats.

Not to be confused with [profiles-research.md](profiles-research.md),
which is about *user* profiles and themes. Here "profile" means data that
describes a camera body, a lens or a sensor's noise.

Where Helios stands today (see [raw-development.md](raw-development.md)):
rawler 0.8 decodes the raw and gives one colour matrix (D65 when it has
one) from its camera catalogue or the DNG's tags; `modules/colorin`
applies that matrix and nothing else. There is no lens correction, no
profiled denoise, no DCP or ICC input profile, and no way for a user to
add camera data.

## 1. What a raw developer needs for each camera and lens

### 1.1 Decoding and colour, per body

| Data | What it is for | Where it comes from |
|---|---|---|
| Black level, white level (per channel, per ISO for some bodies) | scaling raw units to 0..1; a wrong white level makes magenta highlights | the file's tags; corrected per body in rawspeed's [`cameras.xml`](https://github.com/darktable-org/rawspeed/blob/develop/data/cameras.xml) and rawler's [camera TOMLs](https://github.com/dnglab/dnglab/tree/main/rawler/data/cameras) |
| Crop (active area) | hides masked and garbage photosites | the same; rawler's crop is 18 to 56 px narrower than rawspeed's on the parity corpus (raw-development.md, section 3) |
| CFA layout | which colour each photosite sees (Bayer phase, X-Trans 6 x 6) | the same, shifted with the crop |
| Colour matrix (XYZ to camera) | the camera's colours to a known space | Adobe's matrices, read from DNGs written by Adobe's DNG Converter: rawspeed's `ColorMatrix` (1119 entries in `cameras.xml`), rawler's `color_matrix`, LibRaw's `adobe_coeff` table ([LibRaw](https://github.com/LibRaw/LibRaw), `src/tables/colordata.cpp`), RawTherapee's `dcraw_matrix` in [`camconst.json`](https://github.com/Beep6581/RawTherapee/blob/dev/rtengine/camconst.json). darktable's old `adobe_coeff.c` is gone: it now reads rawspeed's matrices |
| "Enhanced" matrices | a matrix fitted on a chart by contributors, better than Adobe's on some bodies | darktable's [`colormatrices.c`](https://github.com/darktable-org/darktable/blob/master/src/common/colormatrices.c) (about 100 bodies, mostly older) |
| White balance presets | daylight, cloudy, flash... per body | darktable's [`wb_presets.json`](https://github.com/darktable-org/darktable/blob/master/data/wb_presets.json) |

A single matrix is linear and tuned for one illuminant (D65). It is the
"just works" baseline: every serious open source developer has it for
every supported body, because it is cheap to get (one DNG conversion)
and the numbers are facts, not creative work.

### 1.2 DCP and ICC input profiles

**DCP** (DNG Camera Profile, defined by the [DNG
specification](https://helpx.adobe.com/camera-raw/digital-negative.html))
is the richer format, and the one most chart tools write:

- `ColorMatrix1/2` and `ForwardMatrix1/2` for two calibration
  illuminants (typically Standard Light A and D65; DNG 1.6 allows a
  third), with `CalibrationIlluminant1/2`;
- **dual-illuminant interpolation**: the white point of the shot is found
  from its as-shot neutral, and the two matrices are blended by inverse
  correlated colour temperature, iterating until the white converges. A
  tungsten shot gets the tungsten-measured matrix, a daylight shot the
  daylight one, and everything in between a blend;
- `ProfileHueSatMap` (a 2.5D table of hue shift, saturation and value
  scale per hue and saturation, per illuminant): non-linear corrections
  the matrix cannot make;
- `ProfileLookTable` and `ProfileToneCurve`: the "look" (a camera-maker
  JPEG match, a contrast curve), separate from the colorimetric part;
- `BaselineExposureOffset`, `DefaultBlackRender`, copyright and
  embed policy tags.

rawler already reads these tags from a DNG (it copies `ForwardMatrix*`
and `ProfileHueSatMapData*` in `decoders/dng.rs`); Helios uses none.

**ICC input profiles** are the other format: a matrix and curves, or a
LUT (`A2B0`). They have one illuminant (their PCS is D50) and no
separation of colorimetry from look. darktable's input colour profile
module takes ICC files from the user's folder
([manual](https://docs.darktable.org/usermanual/stable/en/module-reference/processing-modules/input-color-profile/));
Capture One's built-in profiles are ICC.

**LUT profiles** (an ICC LUT or a DCP with a dense HueSatMap) are more
accurate on the chart's patches but can twist colours outside the
chart's gamut and amplify noise in smooth gradients. The dcamprof
documentation ([Anders Torger](https://torger.se/anders/dcamprof.html))
is the reference on these trade-offs: it recommends a matrix plus a
gentle LUT, and warns against overfitting to a 24-patch target.

### 1.3 Lens correction

**lensfun** ([site](https://lensfun.github.io/),
[repository](https://github.com/lensfun/lensfun)) is the only open
database of measured lens corrections. Per lens, per mount family, in
XML:

- **distortion**: `poly3`, `poly5` or `ptlens` (Panorama Tools) radial
  polynomials, per focal length;
- **TCA** (lateral chromatic aberration): `linear` or `poly3` scaling of
  red and blue against green, per focal length;
- **vignetting**: the `pa` model, a radial polynomial per focal length,
  aperture and focus distance;
- the crop factor of the body the lens was measured on, so a full-frame
  measurement can be scaled to a smaller sensor (a *derived* correction,
  not a measured one).

Corrections measured by the maker travel **inside the raw** on many
recent systems, and darktable reads them (`src/iop/lens.cc`,
"embedded metadata" method; `src/common/exif.cc` recognises Sony, Fuji,
DNG, Olympus/OM and Panasonic):

- **DNG opcodes**: `WarpRectilinear` (distortion and TCA per plane),
  `FixVignetteRadial`, `GainMap` (also used for lens shading on phones);
  in the DNG specification;
- **Sony ARW**: distortion, chromatic aberration and vignetting spline
  tables in the makernote;
- **Fujifilm RAF**: geometric distortion, chromatic aberration and
  vignetting parameters;
- **Olympus/OM System ORF**: distortion and CA polynomials in the
  makernote;
- **Panasonic RW2**: distortion parameters.

Tag names and layouts are reverse-engineered; [ExifTool's tag
tables](https://exiftool.org/TagNames/) are the usual reference. On
mirrorless bodies whose lenses are designed to be corrected in software
(strong barrel distortion at the wide end, dark corners), the embedded
data is not optional: without it the image is visibly wrong.

Adobe's format is the **LCP** lens profile (an XMP file per lens, made
with Adobe Lens Profile Creator). Adobe ships a large set with Camera Raw
([supported lenses](https://helpx.adobe.com/camera-raw/kb/supported-lenses.html));
RawTherapee and ART can read LCP files the user has
([RawPedia, Lens/Geometry](https://rawpedia.rawtherapee.com/Lens/Geometry)).

### 1.4 Noise profiles

darktable's profiled denoise
([manual](https://docs.darktable.org/usermanual/stable/en/module-reference/processing-modules/denoise-profiled/))
models sensor noise as Poisson plus Gaussian: variance = a * signal + b,
per colour channel. [`noiseprofiles.json`](https://github.com/darktable-org/darktable/blob/master/data/noiseprofiles.json)
holds `a` and `b` per camera per ISO; between two measured ISOs the
module interpolates. Without a profile it falls back to a "generic
Poissonian" estimate, which works but is less precise.

How they are measured
([pixls.us article](https://pixls.us/articles/how-to-create-camera-noise-profiles-for-darktable/)):
the photographer shoots a defocused, evenly lit subject with a range of
tones (a gradient or a window through tracing paper), at every ISO of
the body, overexposed to cover the full range; darktable's
`darktable-gen-noiseprofile` script (in `tools/noise/`) fits `a` and `b`
from the raws and writes a JSON fragment to submit upstream. It takes an
afternoon, a tripod and the camera itself: nothing a maintainer can do
without the body.

In the darktable source used by Helios's bench: **438 models, 8682
ISO profiles**, against **947** camera keys in rawspeed (measured in
[raw-corpus.md](raw-corpus.md), section 4): fewer than half the
supported bodies have a noise profile.

## 2. Who maintains each source, and how good it is

| Source | Maintainers | Coverage and lag | Licence |
|---|---|---|---|
| Adobe DNG Converter, Camera Raw | Adobe, with makers' pre-release access | new bodies usually at or soon after release ([supported cameras](https://helpx.adobe.com/camera-raw/kb/camera-raw-plug-supported-cameras.html)); the matrices every open project copies come from here | proprietary application; DCPs it installs are Adobe's |
| rawspeed `cameras.xml` | darktable's rawspeed developers, from samples on [raw.pixls.us](https://raw.pixls.us/) | a body is added once someone uploads samples and a developer writes the entry; it reaches users with darktable's releases (two majors a year, plus point releases): weeks to months, longer for unusual formats ([camera support](https://www.darktable.org/resources/camera-support/)) | LGPL-2.1 |
| rawler (dnglab) camera TOMLs | dnglab's author, mostly one person | 857 keys at 0.8.0, 116 of them aliases of rawspeed bodies (raw-corpus.md); releases are irregular | LGPL-2.1 |
| LibRaw `adobe_coeff` | LibRaw LLC | broad, follows Adobe; snapshots more frequent than releases | LGPL-2.1 or CDDL-1.0 |
| RawTherapee `camconst.json`, DCPs | RawTherapee contributors | levels and matrices for many bodies; a few dozen contributor-made dual-illuminant DCPs ([RawPedia, colour management](https://rawpedia.pixls.us/Color_Management)) | GPL-3 |
| darktable `colormatrices.c` | darktable contributors | about 100 bodies, few recent ones; not maintained actively | GPL-3 |
| lensfun database | volunteers, a handful of maintainers | library 0.3.4 (July 2023) is the last release ([news](https://lensfun.github.io/news)); the data moves in git and is fetched with `lensfun-update-data`, but distributions ship it unevenly. Recent mirrorless lenses (Sony E, Nikon Z, Canon RF, Fuji X third-party primes) are often missing because each needs a volunteer to shoot calibration images ([example](https://discuss.pixls.us/t/canon-r5-c-provide-noise-profiles-and-lensfun/51228)) | library LGPL-3; database CC BY-SA 3.0 |
| darktable `noiseprofiles.json` | photographers who own the body, merged by darktable | fewer than half the supported bodies; new bodies wait for an owner to measure ([Sony A7 IV](https://discuss.pixls.us/t/no-noise-profiles-for-sony-a7iv/33018), [darktable 5.0](https://discuss.pixls.us/t/missing-darktable-5-0-camera-profiles/47625), [new camera on Windows](https://discuss.pixls.us/t/darktable-noise-profile-for-a-new-camera-on-windows/51235), [contributing](https://discuss.pixls.us/t/contribute-to-denoise-profiles/46123)) | GPL-3 (part of darktable) |
| Embedded lens corrections | the camera makers | every lens the body knows, from day one, including future firmware | the user's own file; formats reverse-engineered |
| DxO Optics Modules | DxO's lab | about 110,000 body and lens pairs ([report](https://www.digitalcameraworld.com/tech/software/dxos-optical-corrections-now-cover-an-unprecedented-110-230-camera-and-lens-combinations)) | proprietary |

What the table says:

- **Colour is the healthy part.** Matrices exist for nearly every body
  as soon as a decoder supports it, because they come with Adobe's
  DNGs. The bottleneck is the decoder, not the colour.
- **Lens data is the weak part for new gear**, and the embedded
  corrections cover precisely the gap lensfun leaves: recent mirrorless
  lenses on Sony, Fuji, OM, Panasonic and DNG. Canon and Nikon keep
  their corrections in forms darktable does not read yet; for them
  lensfun stays the only open source.
- **Noise profiles do not scale** with volunteers alone. A generic
  fallback that estimates noise from the image itself is the realistic
  answer for the long tail.
- **Lag comes from releases.** darktable ties all this data to its own
  release cycle; lensfun ties it to distributions. A data channel that
  updates without an application release removes most of the lag.

### What Helios can legally ship

Helios is GPLv3.

- rawspeed and rawler data (LGPL-2.1), LibRaw's table (LGPL-2.1):
  compatible; ship with their notices.
- darktable's `noiseprofiles.json`, `wb_presets.json`,
  `colormatrices.c` (GPL-3): compatible.
- RawTherapee's `camconst.json` and DCPs (GPL-3): compatible.
- lensfun's database (CC BY-SA 3.0): shippable with attribution; a
  converted or extended copy stays CC BY-SA. CC BY-SA 3.0 is not
  declared compatible with the GPL, so keep the database a separate data
  file under its own licence (as every distribution does), never
  compiled into the GPL code.
- **Adobe's DCPs** (Adobe Standard, Camera Matching) and **LCP
  profiles**: installed by Adobe's free applications, but no licence we
  know of allows redistributing them. Helios must not ship them; it may
  read them where the user installed them, as RawTherapee and ART do.
- The **DNG SDK** is not needed: the DCP tags and opcodes are documented
  in the DNG specification, and rawler reads them. Its licence is
  permissive but carries Adobe's patent terms
  ([summary](https://scancode-licensedb.aboutcode.org/adobe-dng-sdk));
  the DNG patent licence asks for a notice in products implementing the
  format, which Helios should add once it reads DNG opcodes.
- **Embedded corrections** belong to the user's file: no licence issue.
- **DxO, Capture One, Lightroom** profile data: proprietary, out of
  reach.

## 3. What photographers expect

- **"It just works" on a new camera.** The commonest pain on pixls.us is
  not bad colour but *nothing*: a body not decoded, then decoded without
  a noise profile, then a lens unknown to lensfun (threads above). The
  expectation is set by Lightroom, which supports most bodies on release
  day. Users accept a generic fallback if it is labelled; they do not
  accept having to wait six months or compile darktable.
- **Clear status.** Users ask in the forums whether their camera "has a
  profile" because the application does not say. darktable shows
  "generic Poissonian" in the denoise module and lens names in the lens
  module, but nothing summarises what is known about a body.
- **Custom colour profiles** for product, art reproduction and
  multi-camera shoots: a ColorChecker (24 or SG) or IT8 shot, turned
  into a profile with
  [dcamprof](https://torger.se/anders/dcamprof.html) (free software,
  command line), [Lumariver Profile Designer](https://www.lumariver.com/)
  (commercial, dcamprof's graphical sibling), Adobe DNG Profile Editor
  (free, not updated for years), Calibrite or X-Rite tools, or
  darktable's colour calibration chart method
  ([manual](https://docs.darktable.org/usermanual/stable/en/module-reference/processing-modules/color-calibration/)),
  which fits a matrix inside the edit rather than producing a reusable
  file. Pain points: tools write DCP while darktable reads only ICC;
  profiles stop matching when a camera's name differs slightly (RawPedia
  warns that DCP matching is case-sensitive on the exact model name);
  Capture One needs the file named after its own model naming scheme to
  list it ([Capture One support](https://support.captureone.com/hc/en-us/articles/360002409057)).
- **Per-camera defaults**: "always use my profile and my denoise preset
  for this body at ISO above 3200". Lightroom and Capture One both let a
  profile be the default per camera model (and per serial number in
  Lightroom).
- **Looks**: camera-matching renderings (the maker's JPEG colours, in
  Lightroom as "Camera Matching" profiles
  ([Adobe](https://helpx.adobe.com/ca/lightroom-classic/desktop/process-and-develop-photos/image-tone-color.html))
  and film looks (Fujifilm film simulations, Hald CLUT packs in
  RawTherapee and ART, spectral film models in
  [ART](https://artraweditor.github.io/SpectralFilmSimHowto)). Users
  file them under "profiles" because Lightroom does, but they are looks
  applied after colorimetry, often as 3D LUTs with a strength slider.

## 4. How others ship and update profiles

| Application | Colour | Lens | Noise | Updates | Custom |
|---|---|---|---|---|---|
| Lightroom, Camera Raw | DCP: Adobe Color, Adobe Standard, Camera Matching per body; Creative profiles (LUT based) for looks | LCP, Adobe's lab plus embedded data | AI denoise, no per-body profiles exposed | with each Camera Raw release, roughly every two months | DCP and LCP files in a user folder; XMP Creative profiles |
| Capture One | ICC per body, made in-house; a generic profile for bodies not yet profiled | in-house lens profiles; LCC for user-measured flat field | in-house | with releases | ICC import, default per camera |
| darktable | rawspeed matrices, enhanced matrices, ICC input profiles from a folder | lensfun (system library and data), embedded metadata | `noiseprofiles.json`, user file in the config folder | with releases; lensfun via the system | ICC, user noise profile JSON, chart fit in colour calibration |
| RawTherapee | DCP (shipped contributor DCPs, matched by exact name, user folder), ICC, `camconst.json` | lensfun, LCP | none (non-profiled denoise) | with releases | DCP, ICC, LCP in folders |
| ART | as RawTherapee, plus CLF and CTL LUTs, OCIO | lensfun, LCP | none | with releases | DCP, ICC, LUTs |
| DxO PhotoLab | in-house per body | **Optics Modules**: lab-measured per body and lens pair, downloaded on demand the first time a folder holds that pair ([DxO](https://www.dxo.com/news/optics-modules-november-2023/)) | DeepPRIME, trained per sensor | modules downloaded independently of the application | none for lenses |

Two patterns stand out:

- **DxO decouples data from releases**: the application asks a server
  for the module matching the body and lens it just saw, and caches it.
  Coverage grows weekly without a new application.
- **The open source editors couple data to releases** (darktable,
  RawTherapee, ART) or to the system (lensfun), and expose custom data
  as files dropped in a folder, matched by name, with no provenance and
  no indication of quality.

## 5. Recommendation for Helios

### 5.1 Profiles are data packages, not plugins

[extensions.md](extensions.md) separates modules (pure processing) from
plugins (code that extends the application). Profiles are neither: they
are **data**, read by modules (colorin, lens, denoise) and chosen by the
application. They need no capabilities, no process and no trust in code,
only provenance. So:

- **A third packaging form: the data package**, a folder or archive with
  a declarative manifest and payload files, read before any processing.
  It is the same idea as a template bundle: user-visible, versioned,
  schema-checked, never executed.
- **Plugins make data packages.** A plugin may measure, convert or fetch
  profiles (it has code and capabilities), but what it hands Helios is a
  data package. Modules never see a plugin.
- **Not a manifest per profile file** either: a package groups many
  profiles from one source under one licence and one version.

### 5.2 What the core ships

One versioned package per upstream source, payloads kept in their
upstream format where it is stable (so updates are a re-import, not a
conversion to re-check), with a generated index for matching:

| Package | Payload | Licence |
|---|---|---|
| `helios.data.matrices` | rawler's and rawspeed's levels, crops and D65 matrices, darktable's enhanced matrices, white balance presets | LGPL-2.1, GPL-3 |
| `helios.data.noise` | darktable's `noiseprofiles.json` | GPL-3 |
| `helios.data.lensfun` | lensfun's XML database | CC BY-SA 3.0 |
| `helios.data.dcp-open` (later) | RawTherapee's contributor DCPs, and Helios's own measured DCPs if a contributor programme starts | GPL-3 |

Each is built from a pinned upstream commit by a script in the
repository (`tools/data/`), with a test that every entry parses and
matches at least one alias. The core binary carries a copy (it works
offline); newer copies, downloaded or installed, win by version.

**Updates without an app release**: a signed index on Helios's site
lists the latest version of each package; Helios checks it at most daily
(opt-in, the setting asked at first run), downloads only packages whose
version and schema it supports, and keeps the previous one for rollback.
A user with no network keeps the bundled copy. Distribution packagers
can install packages in `<prefix>/share/helios/data/` as they do for
plugins.

Embedded corrections (DNG opcodes, maker notes) are **code** in the
loader (`helios-load`, or upstreamed to rawler), not data: they are
parsed from each file.

### 5.3 What is a plugin

- **A chart profiler**: from a chart shot (ColorChecker 24, SG, IT8),
  produce a DCP (matrix, dual illuminant if two shots are given, optional
  gentle HueSatMap) inside a user data package. A first version can wrap
  dcamprof out of process (it is free software); an own Rust fitter can
  follow. The chart detection can reuse darktable's colour calibration
  approach.
- **A noise profiler**: guide the shoot, fit `a` and `b` per ISO with
  the same model as darktable, write a user noise package, and offer to
  export the darktable JSON fragment for upstream.
- **Vendor importers**: find Adobe's installed DCP and LCP folders (or
  Capture One's ICCs) and register them as a *local* package that
  references the files in place, never copying them, so nothing
  proprietary is redistributed.
- **A lens calibration helper** for lensfun (shots in, lensfun XML
  out, with a link to submit upstream).

### 5.4 How a user adds a custom profile

- **Import a file**: drop or pick a `.dcp`, `.icc`/`.icm`, `.lcp`, a
  lensfun `.xml` or a noise JSON. Helios reads its identity (a DCP's
  `UniqueCameraModel`, an ICC's description, an LCP's lens), shows what
  it will match, lets the user correct the match, and writes it into the
  user's package (`~/.local/share/helios/data/user/`).
- **Run a chart workflow**: the profiler plugin, ending in the same
  place.
- **Set per-camera defaults**: "use this profile for this body", by
  model or by serial number, stored as a rule in the user package (see
  matching below), not as an edit preset.

Looks (camera matching, film) are **not** input profiles: they are
presets or LUT nodes applied after colorimetry, shipped as templates or
LUT packages. Mixing them into the input profile (as DCP `LookTable`
does) should be read, but Helios's own looks belong in the network,
with a strength.

### 5.5 Manifest sketch

```toml
# helios-data.toml
[package]
id = "org.lensfun.database"        # reverse domain, as plugins
version = "2026.11.01"             # date of the upstream snapshot
kind = ["lens"]                    # colour, lens, noise, wb
schema = 1                         # Helios data schema
license = "CC-BY-SA-3.0"
attribution = "The lensfun project and its contributors"
homepage = "https://lensfun.github.io/"

[provenance]
upstream = "https://github.com/lensfun/lensfun"
commit = "3f1c0e2"
built_by = "tools/data/lensfun.py"
signed_by = "helios-data-2026"     # key id; user packages are unsigned

[quality]
default = "measured"               # measured, derived, generic
# per entry overrides live in the payload index

[[payload]]
format = "lensfun-xml"             # dcp, icc, lcp, darktable-noise-json, rawspeed-xml, rawler-toml
files = ["db/*.xml"]

# A user package adds rules on top of matching
[[rule]]
match = { make = "Sony", model = "ILCE-7M4", serial = "1234567" }
colour = "dcp:studio-cc24.dcp"
noise = "builtin"                  # keep the shipped profile
```

The generated index (one per package, rebuilt on install) lists each
entry with its key, its quality and its source line:

```toml
[[entry]]
kind = "noise"
make = "SONY"; model = "ILCE-7M4"   # normalised names, as raw-corpus.md
iso = [100, 125, 160, 200]          # measured points
quality = "measured"
source = "noiseprofiles.json#/noiseprofiles/SONY/ILCE-7M4"
```

### 5.6 Matching and fallbacks

Keys are normalised the way [raw-corpus.md](raw-corpus.md) section 4
proposes for camera names (rawler's clean name, rawspeed's aliases,
regional names folded), so a profile for "ILCE-7M4" also matches the
same body sold under another name. Lenses match by mount and maker lens
id when the file has one, else by lensfun's name rules, then focal
length.

Order, first match wins, each choice shown to the user:

- **Colour**: user rule for this serial, user rule for this model,
  DCP or ICC from a user or vendor-local package, an open DCP package,
  the DNG's own profile tags, the enhanced matrix, the standard matrix
  (rawler, then rawspeed), a generic matrix (flagged, never silent).
- **Lens**: user rule, embedded maker data (exact for that lens and
  body), lensfun measured on this mount, lensfun scaled from another
  crop factor or a sibling mount (derived), none.
- **Noise**: user rule, measured profile for this body (ISO
  interpolated; an ISO outside the measured range is clamped and marked
  derived), a profile of an alias or same-sensor body (derived), the
  generic estimate from the image (generic).

Packages are layered: user over vendor-local over downloaded over
bundled; within a layer, a higher package version wins. The choice made
is recorded in the edit (package id, version, entry), so an edit
reopened after an update renders the same until the user accepts the
newer data, as darktable's history keeps module parameters.

### 5.7 How quality is shown

Three levels, the same words everywhere:

- **Measured**: from a chart, lab or noise shoot of this body or lens
  (lensfun measured, a noise profile, a user DCP, the maker's embedded
  data);
- **Derived**: adapted from other data (Adobe's single matrix, a lensfun
  entry scaled to another crop factor, a sibling body's noise profile,
  an ISO extrapolated);
- **Generic**: a fallback that knows nothing specific.

Shown as a small badge in the colorin, lens and denoise node headers,
and gathered in an image's **camera support** panel (body, lens, each
kind of data, its source and licence, and "how to improve this": the
chart workflow, the noise workflow, the upstream link to contribute).
The library can filter images whose data is generic, so a photographer
sees at once which shoots would gain from a profile.

### 5.8 Phased plan

Sizes: S a day or less, M a few days, L a week or more.

1. **Data package format and loader** (M): manifest schema, index,
   layering, matching on normalised names, the bundled
   `helios.data.matrices` built from rawler and rawspeed; colorin takes
   its matrix from the package. Unblocks raw-development.md C5 and the
   CR2 colour offset noted there.
2. **Quality badges and the camera support panel** (S, then M for the
   panel).
3. **DCP input in colorin** (L): matrices, forward matrices,
   dual-illuminant interpolation, HueSatMap, LookTable and tone curve
   optional; first from the DNG's own tags, then from files.
4. **Import a custom DCP or ICC** (S once 1 and 3 exist), with
   per-camera rules.
5. **Embedded lens corrections and a lens module** (L): DNG opcodes
   first (documented), then Sony, Fuji, OM, Panasonic as darktable reads
   them; the module needs the geometry support listed as not decided in
   the module manifest.
6. **lensfun package** (M on top of 5): parse the XML in Rust, the
   models in the lens module.
7. **Noise package and profiled denoise** (L, dominated by porting the
   denoise module), with the generic fallback.
8. **Update channel** (M): signed index, download, rollback, opt-in.
9. **Plugins** (each M): vendor importer for Adobe's DCP and LCP
   folders, chart profiler wrapping dcamprof, noise profiler.
10. **Looks** (M): LUT packages (`.cube`, Hald CLUT, CLF) as a LUT node
    with a strength, camera-matching looks as templates.

## 6. ROADMAP proposal (not decided)

To add to ROADMAP.md if accepted:

> - (2026-11-07, proposed, not decided) **Camera, lens and noise
>   profiles are data packages**
>   ([docs/camera-profiles-research.md](docs/camera-profiles-research.md)):
>   one versioned, signed package per upstream source (matrices from
>   rawler and rawspeed, darktable's noise profiles, lensfun), with a
>   declarative manifest (identity, licence, provenance, matching rules),
>   bundled with the core and updated without an application release.
>   Plugins make packages (chart profiler, noise profiler, importers of
>   the user's installed Adobe profiles) but modules only ever read data.
>   Every choice is labelled measured, derived or generic, and recorded
>   in the edit. Adobe's DCP and LCP files are read in place, never
>   shipped.
