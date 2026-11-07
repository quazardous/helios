# Performance budgets

What Helios promises about speed and memory, and where it stands. Each
budget is measured by a benchmark on the bench box (see [bench.md](bench.md));
each dated table is a baseline the next slices compare against.

## Thumbnails

The library grid, the filmstrip, the import grid and Compare all draw
thumbnails. The budgets below are proposed (2026-11-07, not decided):
they are the targets of the thumbnail work (one cache, virtualised grids,
a render queue).

| Budget | Target |
| --- | --- |
| First screen of library thumbnails | under 1 s at 10,000 images, under 2 s at 50,000 |
| Scrolling the library grid | 60 fps at 50,000 images: a frame's interface work under 16 ms (p95) |
| RAM | the process under 1 GB at 50,000 images while browsing the library |
| GPU texture memory | the thumbnails' textures under 512 MB, at any library size |
| Cache on disk | under 300 MB per 10,000 images at 256 px (about 30 KB an entry) |
| A thumbnail rendered from the edit | under 200 ms an image at 256 px, on the box's CPU |

The first screen is timed from the moment the library's images are known
(the vault's rows read) to the frame where every cell in view shows its
thumbnail. The vault's read is not in it: a vault holds each file once
(by content), so the benchmark cannot repeat the corpus in one.

### The benchmark

`helios-gui`'s `thumb_bench` example, run by the box job `thumb-bench`
(timing-class: the machine alone, before a release only):

```bash
cargo xtask bench-remote thumb-bench branch=main            # 1,000, 10,000 and 50,000
cargo xtask bench-remote thumb-bench sizes=10000 frames=120
```

- **decode**: for each raw of the corpus (the `raw-corpus` volume), its
  largest embedded preview read and scaled to 256 px as the library reads
  it today (`helios_load::preview`), its smallest one as the import grid
  reads it (`helios_load::glance`), the size of a 256 px JPEG (quality 85)
  of it, as a disk cache would hold it, and, for the first raws, a 256 px
  render of its edit through the default network
  (`head_render::render`). Each preview is read twice and the second read
  timed: the file is in the page cache, as on a local disk.
- **grid**: a library of N images made by repeating the corpus by
  reference (entries pointing at its files: nothing is copied), shown in
  the Library panel of a headless shell at 1920×1080. The thumbnails are
  read as `helios-gui` reads them: one thread, every image in the
  library's order. Measured: the first frame with cells, the first screen
  (every cell in view has its thumbnail), then 240 frames of the wheel
  scrolling the grid 400 points a frame (the frame's interface work, and
  the picture drawn by the harness's renderer, a software one on the box),
  the share of cells in view that had their thumbnail, the previews read
  per second, peak RSS and the thumbnails' textures. Each N runs in a
  process of its own.

The results are JSON lines in the job's `out/thumb-bench.jsonl`.

### Baseline, 2026-11-07

One run of `thumb-bench` on `perf/thumbnail-budgets` (box job
20261107-221017-14b9), before any of the thumbnail work. The box flagged
the run **not comparable**: its CPU was throttled for 33 % of the run
(100 °C), so the times below are pessimistic. The corpus held 38 raws, 33
of them with an embedded preview rawler reads. The Library panel of the
default layout showed 6 cells at 1920×1080.

| Budget | Target | 1,000 | 10,000 | 50,000 | Verdict |
| --- | --- | --- | --- | --- | --- |
| First screen | < 1 s (10k), < 2 s (50k) | not measured | not measured | not measured | **fail** (see below) |
| Scroll, interface work per frame | p95 < 16 ms at 50k | p95 6.4 ms, p99 14.3 ms, max 21.0 ms | p95 6.6 ms, p99 10.4 ms, max 14.5 ms | p95 6.7 ms, p99 10.5 ms, max 12.1 ms | pass (to be confirmed, see below) |
| Scroll, picture drawn (software renderer) | not judged | p50 80 ms | p50 78 ms | p50 79 ms | not judged |
| Peak RSS | < 1 GB at 50k | 1.51 GB | 1.46 GB | 1.42 GB | **fail** |
| Thumbnail textures | < 512 MB | 180 MB projected | 1.79 GB projected | 8.95 GB projected | **fail** from about 2,900 images |
| Cache on disk (256 px JPEG, quality 85) | < 300 MB per 10,000 | | 168 MB projected (16.8 KB an entry, p95 26 KB) | | pass (projected: no cache yet) |
| Thumbnail rendered from the edit, 256 px | < 200 ms | | 358 to 937 ms (p50 897 ms, 3 raws) | | **fail** |

What else the run measured:

- **Reading the previews** as the library reads them today
  (`helios_load::preview`, the largest preview): mean 360 ms an image,
  p50 123 ms, p95 1,066 ms, max 1,574 ms (a DNG whose "preview" is its
  full image). The library's reader thread managed 2.6 to 2.7 images a
  second: reading every thumbnail would take 6 min at 1,000 images,
  63 min at 10,000 and 5 h at 50,000, at every start (nothing is cached).
- **The smallest preview** (`helios_load::glance`, the import grid's):
  mean 307 ms, p50 103 ms, p95 973 ms. Even the small previews are
  decoded at up to 1920×1280 and converted to the working space in
  floating point before being scaled to 256 px.
- **A texture** is 179 KB (256 px RGBA8); none is ever freed.
- **Renders from the edit**: 3 of the first 6 raws rendered; the other 3
  (linear DNGs, a CR2 sRAW) are "not a mosaic" for the develop network.
  There is no reduced-size demosaic: the render demosaics at full size,
  then scales.

What it could not measure, and why:

- **First screen**: never reached within the 120 s deadline at any size.
  Five corpus files give no preview (three ORF, rawler finds none; a CRW;
  a Hasselblad ARW rawler cannot decode), and their cells stay empty
  forever, which the benchmark counted as "not shown yet". The benchmark
  now counts a file without a preview as done; the run with that fix was
  cancelled after an hour in the queue (a timing job waits for an empty
  box). With 6 cells in view and previews read in order, the first
  screen would take the reading of its first 6 images: about 0.7 to
  2 s from the means above, whatever the library's size.
- **The scroll's reach**: the cells in view had their thumbnail on 48 %
  of the frames while the grid scrolled 400 points a frame, which says
  the view did not travel far past the first few hundred images (the
  reader had read about 380). The frame times are those of a grid with
  50,000 images, but how far the wheel moved it is not recorded in this
  run (the fixed benchmark records it).
- **The vault's read** of the library's rows: not in the benchmark (a
  vault holds each file once, so the corpus cannot be repeated in one).

The bottlenecks, in order:

1. Previews are read whole and in floating point, one image at a time,
   for every image at every start: 2.7 images a second.
2. Thumbnail textures are never evicted: 179 KB each, past the 512 MB
   budget at about 2,900 images.
3. Previews are read in the library's order, not the view's: scrolling
   shows empty cells (the grid already reports the cells in view; the
   library does not use it yet).
4. Renders from the edit run the full-size chain: 0.4 to 0.9 s each.
5. Files without a readable preview keep an empty cell, with no render
   to replace it.

### The thumbnail work, step by step

**Step 2, one thumbnail cache: built (2026-11-07).** The `helios-thumbs`
crate, used by the library (`helios_gui::thumbnails`):

- **The fast first read** (`helios_thumbs::read`): the file's embedded
  JPEG streams are found by their markers and their frame headers read
  (a raw's thumbnail and previews, a JPEG's Exif thumbnail, the JPEG
  itself; lossless streams, a raw's mosaic, are left out). The smallest
  at least 256 px on its long side is decoded by `jpeg-decoder` with DCT
  scaling (1/2, 1/4 or 1/8 while decoding), straight to 8-bit sRGB, then
  scaled to 256 px. No floating point, no full-size decode, no colour
  conversion (previews are sRGB). When no JPEG stream is found, rawler's
  thumbnail, preview or full image, else the picture itself (PNG, TIFF).
- **The cache** (`ThumbCache`): JPEG files (quality 80) in `.helios/thumbs`
  beside the catalogue, 256 px for the grids and 1024 px for Compare's and
  the loupe's first paint. An entry's key is the image's id (the BLAKE3 of
  its file: content-addressed, a file imported again finds its entry),
  the version drawn (`preview` for the embedded preview) and the chain's
  version (0): step 4 writes renders under their version's key. A size
  limit (`system.cache.thumbnails`, 2 GiB by default) is kept by evicting
  the entries used least recently, down to 90 % of the limit; an entry's
  last use is its file's modification time across restarts.
- **The pool** (`Loader`): two to four threads (half the CPUs). Each image
  is looked up in the cache first, its file read only on a miss; a miss
  stores the 256 px entry, then queues the 1024 px one behind every
  thumbnail still to show. The grid's in-view hook moves the cells in
  view to the front of the queue each time they change. A photo with no
  copy at hand (offline, missing) shows its cached thumbnail; a file with
  nothing readable, or an offline photo never cached, shows a
  placeholder (its file type and a crossed frame) instead of an empty
  cell. Offline and missing photos carry a badge.
- **Measured**: `thumb_bench` reads the library through the pool, with a
  cold start (`--start cold`, an empty cache) and a warm one
  (`--start warm`, every entry cached), and its `decode` part times the
  fast read and a cache hit per raw (`fast_ms`, `cache_hit_ms`). Not run
  yet: `thumb-bench` is a timing job, run before a release.

Still open: textures are never freed (step 3, the virtualised grids, with
the texture budget), and thumbnails are still the embedded previews, not
renders of the edit (step 4, the render queue, which replaces cache
entries by renders under their version's key and takes over the pool's
ordering).
