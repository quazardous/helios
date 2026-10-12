# darktable pitfalls, and what they mean for Helios

An inventory, dated 2026-10-12, of the pitfalls darktable ran into, read
from its issues and pull requests (darktable-org/darktable): what went wrong
and why, the evidence, whether Helios is exposed, and what to do about it.
Helios ports darktable's modules and aims at parity, so darktable's
mistakes are the cheapest lessons it can get.

This is a snapshot, not a watch: it is refreshed when a theme comes back
into focus. Short references such as `#3249` or `PR 3294` point to
darktable-org/darktable. Statements the authors could not confirm from a
thread are marked as inference or guess, and a few links are cited from
their title only, as marked.

| Theme | File | Pitfalls |
|-------|------|---------:|
| Pixel pipeline and image processing | [pipeline.md](pipeline.md) | 21 |
| Colour management and display | [colour.md](colour.md) | 21 |
| GPU, performance and threading | [gpu-perf.md](gpu-perf.md) | 24 |
| Library, files and cameras | [library-files.md](library-files.md) | 22 |
| UI, platforms and project process | [ui-platforms.md](ui-platforms.md) | 21 |

Each entry gives darktable's root cause, two to five links, Helios's
exposure (handled, at risk, or not applicable) and a recommendation. Each
file ends with its own "Top 5 for Helios now".

## Found in Helios's code

Two defects came out of this inventory and were checked in the code:

- **XMP import order.** The import chains modules in the order they were
  last edited, while darktable processes them in its pipe order
  (`iop_order_version`, `iop_order_list`), which was never read
  ([pipeline.md](pipeline.md), pitfall 1). Fixed on 2026-10-12.
- **GPU limits.** The device is opened with wgpu's default limits (128 MiB
  per storage binding), while the scheduler admits jobs against the
  adapter's much larger limits, so a 24 MP f32 image passes the check and
  fails on the device ([gpu-perf.md](gpu-perf.md), pitfall 3).

## Priorities across themes

1. **Data safety.** The SQLite vault has no WAL, no busy timeout, no
   integrity check, no snapshots and no schema versioning; an edit should
   be saved at each step and survive a crash, with quit-and-reopen tests
   ([library-files.md](library-files.md), [ui-platforms.md](ui-platforms.md)).
2. **Import fidelity.** Pipe order, `_NN` duplicate sidecars, missing
   modules filled with today's defaults, render tests on old sidecars
   ([pipeline.md](pipeline.md), [library-files.md](library-files.md)).
3. **Colour in and out.** Embed an ICC profile on export and read embedded
   profiles on load; a per-window display target that declares the
   surface's encoding instead of applying a monitor profile; one gamut
   mapping shared by the viewport and export ([colour.md](colour.md)).
4. **GPU robustness.** Matching device limits, a per-step CPU fallback that
   is reported, bounded waits and device-loss recovery, one memory budget
   per device that counts the GUI's textures ([gpu-perf.md](gpu-perf.md)).
5. **Reduction and whole-image analysis.** Never reduce a raw or a
   gamma-encoded source before it is linear and demosaiced; design
   full-extent analysis nodes before porting modules that need whole-image
   statistics; test tiled against untiled with forced small tiles
   ([pipeline.md](pipeline.md)).
6. **Input and platforms.** The wheel scrolls the panel and does not change
   a slider unless it has focus; a GUI scenario suite with raw pointer,
   wheel and key events; per-window pixel ratio; signed macOS builds owned
   by the project ([ui-platforms.md](ui-platforms.md)).
7. **Process.** Every commit of a push builds and passes its tests, so
   `git bisect` works ([ui-platforms.md](ui-platforms.md)).
