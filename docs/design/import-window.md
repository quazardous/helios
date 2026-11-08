# The import window

Status: built (2026-11-07), see "Order of work". It builds on `docs/import-research.md`
(section 6.2, "v1: the import window") and replaces the import form
(File > Import…, "Import photos…" in an empty Library, drop and paste).

## Goal

Import is the first thing a person does in Helios. The default path is:
**insert a card, press Import.** It takes two clicks, every choice has a
sensible default, and nothing blocks while the photos copy.

## Layout

One window, resizable, opened by File > Import… (Ctrl+Shift+I), by the
empty Library's "Import photos…", by a card toast, or by dropping files
or folders on Helios (the window opens with them as its source).

```
┌ Import photos ─────────────────────────────────────────────────────────────┐
│ SOURCES            │ EOS_DIGITAL · 3,410 photos · 2,980 new · 61 GB        │ CHOICES                     │
│ ▣ EOS_DIGITAL 64GB │ [New only ✓] [All types ▾] [Select: all · none · day] │ ◉ Copy into the Library     │
│ ☐ SD_CARD 32GB     │ ── 2026-11-02 · 412 ─────────────────────────── [✓]   │ ○ Add where they are        │
│ ──────────────     │ ┌──┐┌──┐┌──┐┌──┐┌──┐┌──┐┌──┐┌──┐                        │                             │
│ Recent             │ │✓ ││✓ ││✓ ││░░││✓ ││✓ ││✓ ││✓ │   ░░ = in your Library  │ Into  Pictures/2026/…  [▸]  │
│  Pictures/Trip     │ └──┘└──┘└──┘└──┘└──┘└──┘└──┘└──┘   RAW+JPG: one cell    │  e.g. 2026/2026-11-02/      │
│  Downloads         │ ── 2026-11-03 · 2,998 ───────────────────────── [✓]   │       IMG_0412.CR3          │
│ Favourites         │ ┌──┐┌──┐┌──┐ …                                          │ Name  {stem}            [▾] │
│  NAS/Photos        │                                                       │                             │
│ ──────────────     │                                                       │                             │
│ Choose a folder…   │                                                       │ Duplicates  skip        [▾] │
│                    │                                                       │ ☐ Read darktable edits      │
│                    │                                                       │ ▸ More                      │
├────────────────────┴───────────────────────────────────────────────────────┴─────────────────────────────┤
│ Preset: Default ▾                                    2,980 photos, 54 GB          [Cancel]  [Import 2,980] │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **Sources** (left):
  - mounted cards and cameras, listed first: removable volumes holding
    `DCIM`, named by label and capacity, refreshed when one appears or
    goes;
  - then recent folders, favourites, and "Choose a folder…" (egui-pop's
    folder chooser);
  - several sources can be ticked for one import.

  When Helios starts, the window opens on the first card it finds, or
  else on the last source used.
- **Contents** (middle):
  - **The grid:**
    - thumbnails read from embedded previews and EXIF thumbnails as they
      are found, the first ones within a second;
    - grouped by capture day, each day with a check box and its count;
    - "New only" is on, and images already in the Library are greyed
      with "in your Library" (a size and partial-hash prefilter, then
      confirmed by the full hash during the job);
    - a RAW+JPEG pair is one cell with a "RAW+JPG" mark.
  - **Above the grid:**
    - one line of counts;
    - a type filter (all, raw only, JPEG only);
    - select all, none or by day.
  - **Selecting:** a click toggles a cell, Shift extends, and Space
    toggles the focused cell. Hovering a cell shows its name, time and
    size.
  - **Folders:** a folder source lists every image Helios reads
    (`IMAGE_EXTENSIONS`), subfolders included, shown as day groups.
- **Choices** (right), top to bottom, each with a live example:
  - **Copy into the Library**, or **Add where they are** (references in
    place). Copy is forced for a card unless the person changes it, with
    a note on why.
  - **Into**: destination and folder layout, with an example path.
  - **Name**: the rename pattern (presets and custom, using today's
    `{date}` `{time}` `{camera}` `{frame}` `{stem}` `{seq}`), with an
    example name.
  - **Duplicates**: skip (default), keep, or ask.
  - **Read darktable edits** (sidecars as versions).
  - **More** (folded): metadata template and time offset, once they
    exist.

  Built (2026-11-07): the choices reach the import as
  `library.importFiles`'s arguments (`copy`, `into`, `layout`, `rename`,
  `duplicates`, `darktable`) and are kept, as the last used, per library.
- **Footer:**
  - a preset menu, which replaces the "remember for this workspace" box
    (Default, plus any the person saved; "Save as preset…");
  - the totals;
  - Cancel, and the main button, which states its count ("Import 2,980").

  Built (2026-11-07): a preset is a named set of choices, kept beside the
  per-library choices (`import-presets.json`); applying one sets the
  Choices column, copy or add for folders included (a card is still
  copied unless switched). "Save as preset…" asks a name in place;
  "Delete preset" deletes the one the choices are. The totals count the
  photos ticked and their size. Sources and Choices keep fixed widths
  (at most a quarter of the window each); Contents takes the rest.

## After pressing Import

- The window closes at once. The job runs in the background, one child
  job per source, with progress in the status bar and the Jobs panel.
- New photos appear in the Library as they land and are selected at the
  end, ready to cull.
- A pulled card pauses its job ("Reinsert EOS_DIGITAL to go on"), and
  reinserting it resumes. Cancel stays as it is today.
- The end toast shows counts and, per card, "safe to format: every photo
  is in the Library and verified" (checked by hash). It never formats
  the card itself. The full report goes to the Log panel.
- Undo removes the import's entries from the Library as one step. It
  never touches files. Built (2026-11-07): the `library` scope of the
  history, one step per import, finished or cancelled (see
  docs/library.md, "Undoing an import").

## Pipeline changes this needs (study phase 1)

- Copying reads the card once: hash while copying, then verify the copy.
  A backup copy on import is left for later, in a ticket of its own.
- A lost source pauses the job instead of failing each file.
- The "safe to format" check by hash.
- Import becomes one undo step, which resolves the conflict between
  ROADMAP and library.md.

## Kept as it is

- `library.import` and its arguments, the CLI `helios import`, scripting.
- File > Import from → for importer plugins (the darktable library). They
  are catalogue migrations, not this window.
- The generic form stays only for a command run without its arguments
  outside these entry points (`library.importSource`, `library.importFiles`
  with `ask`, from a script or a plugin). Built (2026-11-07): files or
  folders dropped on Helios, or pasted, open the window with them as its
  sources (`library.importWindow` with `paths`): a folder is a source, a
  file its folder with only the files given ticked.

## Out of v1

These come later (study 6.3 and 6.4): metadata templates and time
offsets, stacks for pairs and bursts, watched folders, phones over MTP,
cloud exports, other catalogues, tethering, and AI after import.

## Order of work

Done (2026-11-07), in the order planned:

1. **Pipeline fixes:** single-read copy, pause on a lost source, safe to
   format, undo step, tested on the box against a fake card.
2. **The window:** sources with card detection; the grid with
   prefilter and pairs; the choices panel with live examples; presets;
   the job hand-off. Tested with snapshots (pop-dark, 1x and 2x: empty,
   a card and a folder added where it is,
   `crates/helios-gui/tests/import_snapshots.rs`, regenerated by the
   box's `gui-snapshots` job) and a kittest that opens the window on a
   fake card, imports, waits for the job, and finds the photos in the
   Library, selected, and Ctrl+Z taking them back as one step.
3. **Wiring:** File > Import…, the empty Library, drop and paste, and the
   card toast all open the window; the old form is gone from those
   paths.

Left for later: the grid's virtualisation and texture eviction for very
large cards, and keeping what the window lists between openings (proposed
in [image-map.md](image-map.md)).
