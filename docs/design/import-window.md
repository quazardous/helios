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
  - **The file browser** (egui-pop's `FileBrowser`, 2026-11-10; it was a
    grid of its own before):
    - its source (`crate::import_browser::ImportSource`) lists the photos
      of the sources ticked as the scans find them; a source whose
      volume is offline lists what the image map recorded there, marked
      "offline", never imported;
    - its modes: details, thumbnails (the default), filmstrip, columns,
      carousel, agenda by capture day, month or year; Ctrl+1 to Ctrl+6;
      the mode is kept for the window;
    - thumbnails read from embedded previews and EXIF thumbnails as they
      are found, else from the library's thumbnail cache (an offline
      photo), visible first, what scrolled away cancelled;
    - "New only" is on, and images already in the Library are dimmed
      with "in your Library" (a size and partial-hash prefilter, then
      confirmed by the full hash during the job);
    - a RAW+JPEG pair is one entry with a "RAW+JPG" badge;
    - folders or cards dropped on it from the system open as the
      sources.
  - **Above the browser:**
    - one line of counts;
    - a type filter (all, raw only, JPEG only);
    - select all, none or the focused photo's day.
  - **Selecting:** the browser's selection is what is imported, every
    photo at first: a click selects one, Ctrl+click adds or removes,
    Shift extends, Ctrl+A selects all, Space opens a quick look.
  - **Folders:** a folder source lists every image Helios reads
    (`IMAGE_EXTENSIONS`), subfolders included.
- **Detail** (right): the selected source's connector in detail
  (owner's design, 2026-11-14: three zones, sources left, photos in the
  middle, the connector's detail right). Top to bottom, each with a live
  example:
  - **The choices every source shares:**
    - **Duplicates**: skip (default), keep, or ask.
    - **Read darktable edits** (sidecars as versions), when the
      connector honours them.
    - **Apply the camera's default look**, when the library keeps a
      camera's default look: on by default, a raw of that camera starts
      with it as its first version.
    - **More** (folded): metadata template and time offset, once they
      exist.
  - **The connector's section**, under its name. Each connector fills it
    through one contract: what it declares (its browsers, its
    capabilities, its source contract) calls for each part, and no
    connector draws the column itself.
    - **The account** (an account's connector): its connection, and for
      a connector whose photos are chosen in its service's page (Google
      Photos), "Choose photos"; the photos chosen show in the middle.
    - **Copy into the Library**, or **Add where they are** (references
      in place). Copy is forced for a card unless the person changes it,
      with a note on why; a connector that cannot add in place always
      copies.
    - **Into**: destination and folder layout, with an example path.
    - **Name**: the rename pattern (presets and custom, using today's
      `{date}` `{time}` `{camera}` `{frame}` `{stem}` `{seq}`), with an
      example name.
    - **Parameters**: the connector's own (its source contract's keys
      that are not choices), drawn from the contract with Settings'
      widgets and kept on the source as they change.
  - **The backup line**, last.

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
  photos ticked and their size. Sources and Detail keep fixed widths
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
