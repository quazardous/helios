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
