# darktable sidecars from several eras

A small corpus of real darktable XMP sidecars, written by darktable itself
between 2020 and 2026, to check that Helios imports old edits as darktable
reads them: the history in force and its end, the pipe order, which modules
are applied and which are kept as not applied, and, where Helios has the
modules, the render (`crates/helios-xmp/tests/corpus.rs`,
`crates/helios-parity/tests/corpus.rs`).

## Where they come from

Every file is a byte-for-byte copy of a sidecar of darktable's integration
tests: first in darktable's own repository
(<https://github.com/darktable-org/darktable>, `src/tests/integration/`),
moved in January 2021 to <https://github.com/darktable-org/darktable-tests>.
The commit is the last one that changed the file; the darktable version is
the one in development at that date (the tests are written by running
darktable on the test image).

| File | Test | Last written | darktable | `xmp_version` | Pipe order | Image |
|------|------|--------------|-----------|---------------|------------|-------|
| `0001-exposure.xmp` | `0001-exposure/exposure.xmp` | 2020-04-25, darktable a2d96f72 | 3.1 (between 3.0 and 3.2) | 4 | legacy (1) | `mire1.cr2` |
| `0004-masks.xmp` | `0004-masks/masks.xmp` | 2020-04-26, darktable f3478b58 | 3.1 | 4 | v3.0 (2) | `mire1.cr2` |
| `0033-blending-modes-uniform.xmp` | `0033-blending-modes-uniform/blending-modes-uniform.xmp` | 2020-11-04, darktable be57e496 ("run integration tests with 3.2.1") | 3.2.1 | 4 | custom list, 5 exposure instances | `mire1.cr2` |
| `0035-multiple-modules.xmp` | `0035-multiple-modules/multiple-modules.xmp` | 2020-11-08, darktable 6390b4f2 | 3.3 (before 3.4) | 4 | v3.0 (2) | `mire1.cr2` |
| `0097-sigmoid.xmp` | `0097-sigmoid/sigmoid.xmp` | 2022-11-02, darktable-tests 723a9500 | 4.1 (before 4.2) | 5 | v3.0 (2) | `mire1.cr2` |
| `0151-sigmoid-no-hue-preservation.xmp` | `0151-sigmoid-no-hue-preservation/sigmoid-no-hue-preservation.xmp` | 2023-08-02, darktable-tests ec745142 | 4.5 (before 4.6) | 5 | v3.0 (2) | `mire1.cr2` |
| `0180-spektrafilm-simple.xmp` | `0180-spektrafilm-simple/spektrafilm-simple.xmp` | 2026-10-07, darktable-tests 3eaf0e64 | 5.7 (development) | 5 | v5.0 (4) | `hlrecovery.arw` |

What each one covers:

- `0001-exposure`: the oldest format here; old params versions migrated
  (temperature 3, highlights 2, exposure 5), darktable's legacy order.
- `0004-masks`: no highlights in the history, so darktable adds clip
  highlights when it reads it; an exposure with a drawn mask (kept, not
  applied), a mask manager item.
- `0033-blending-modes-uniform`: a custom `iop_order_list`, five exposure
  instances of which four blend (kept, not applied), two colorbalance
  instances.
- `0035-multiple-modules`: many modules Helios does not have (liquify,
  retouch, filmic, borders...), all kept.
- `0097-sigmoid`, `0151-sigmoid-no-hue-preservation`: the 4.x format,
  sigmoid version 1 migrated, a module edited twice, a disabled filmic.
- `0180-spektrafilm-simple`: the 5.x format and order, modules from
  darktable's scene-referred workflow presets (`_builtin_` instances).

## Images

The raws are not copied (15 to 21 MB each). The render test reads them from
darktable's integration tests, `HELIOS_TEST_IMAGES` or a darktable checkout
next to Helios's (`src/tests/integration/images`), and skips, saying so,
without them or without `darktable-cli`. `0180-spektrafilm-simple` needs
darktable 5.7 (5.6 has no spektrafilm and crashes on it); with an older
darktable it is skipped, saying so.

The render reference is darktable's render of the same history with the
items Helios does not apply turned off, linear Rec.2020 out, and RCD and
clip highlights in place of methods Helios does not have (the import
reports those). It is judged with darktable's integration thresholds
(Delta E 2000, the max at the 99.9th percentile, away from the edges).

## Licence

darktable is GPL-3.0-or-later, as is Helios. The files written before
January 2021 were part of darktable's repository under that licence. The
darktable-tests repository states no licence of its own; its files are
written by darktable's developers for darktable and are used here on the
same terms. `0001-exposure.xmp` carries the test image's own metadata
(author, CC-BY-SA rights, GPS position), as published by darktable.
