# The Library focus: managing photos

Status: **decided** (2026-11-11): the owner accepted every
recommendation of section 5 (L-C1 to L-C9) and asked to start slice L1.
The inventory describes the code on `next` as of that date.

The Library focus is how Helios manages its photos: browsing and finding
them, acting on many at once, their metadata, stacks and versions, where
their files live, and all of it at the scale of a working photographer's
archive (100,000 photos and more). It is one of the foci of
[windows.md](windows.md): the "Library" focus over the whole library
(P10), and the library view that every other focus shows, filtered to its
scope.

What already exists is described in [../library.md](../library.md)
(import, the image map, duplicates, stacks, culling, storage, backups),
[cache.md](cache.md) (the cache service),
[../storage-backup-research.md](../storage-backup-research.md) and
[../import-research.md](../import-research.md). This note does not repeat
them; it inventories, compares with the established applications, and
proposes the next slices.

## In short

- **Strong today:** import (referenced or copied, duplicates by content,
  darktable sidecars and `library.db` as versions and marks), the image
  map, volumes, offline and missing photos, backups with verified copies,
  "safe to format", catalogue snapshots, the health check, culling marks
  as undoable field commands, stacks proposed and accepted, a virtualised
  grid with thumbnails in view first.
- **Missing for a library:** sorting (the grid is in content-hash order),
  capture time and camera data as queryable fields, folders, collections
  and saved searches, a keyword tree, file operations (rename, move,
  delete, rotate), metadata viewing and writing back, versions in the
  grid, and a library read that does not touch every photo's row and
  file on each refresh.
- **The first slices** are the plumbing every other one needs: a library
  read that scales, capture metadata indexed, sorting, the filter as one
  vault query. Then the browse tree (folders, collections, saved
  searches), then the actions on many photos, then metadata and versions.

## 1. Inventory

### Where the code is

| Area | Files | State |
|------|-------|-------|
| Catalogue (vault) | `crates/helios-vault/src/lib.rs` (images, locations, import, repair), `fields.rs` (field specs, commands, `Query`, `Order`), `catalog.rs` (cursors, fetch, refresh, apply), `stacks.rs`, `versions.rs`, `sightings.rs` (the image map, the Library flag), `undo.rs`, `changed.rs` | built |
| SQLite engine | `crates/helios-vault-sqlite/src/lib.rs`, `migrate.rs` (tables `images`, `locations`, `fields` indexed on `(name, number)`, `versions`, `documents`, `sightings`, `folders`), `snapshot.rs` | built |
| Library operations | `crates/helios-library/src/import.rs`, `rename.rs` (rename patterns for import), `signals.rs` (capture time, exposure, frame number, perceptual hash), `stacks.rs` (`stacks.suggest`), `darktable.rs`, `map.rs` and `map_job.rs` (image map), `thumb_check.rs` (files changed outside), `backup.rs`, `export.rs` | built |
| Storage | `crates/helios-storage/src/` (`detect.rs` volumes, `registry.rs`, `scan.rs`, `strategy.rs` backups, `exclude.rs` blacklist, `marker.rs`) | built on Linux; Windows and macOS list no volumes yet |
| XMP | `crates/helios-xmp/src/` (darktable sidecars read: history, blend, drawn masks, iop order, legacy) | read only |
| Thumbnails | `crates/helios-thumbs/src/` (`read.rs` embedded previews, `cache.rs` disk cache keyed by content and version, `pool.rs` loader, in view first) | built |
| Cache service | `crates/helios-cache/src/` (`service.rs`, `lru.rs`, `flight.rs`) | slices 1 to 3 of [cache.md](cache.md) |
| Health | `crates/helios-health/src/backup.rs`, `checks.rs`, `snapshots.rs` | built |
| Desktop library | `crates/helios-gui/src/library.rs` (the view over egui-pop's library widgets: entries, filter to query, badges, thumbnails, Compare heads), `library_history.rs` (library undo), `persist.rs` (`read_library`, `fill_library`, stacks, decisions), `shell/focus.rs` (the library view is the focus's) | built |
| Widgets | egui-pop `egui-pop-views/src/library/` (`grid.rs` virtualised grid and filmstrip, `filter.rs` search field and chips, `marks.rs`, `tags.rs`, `compare.rs`, `plugin.rs` steps) | built, upstream crate |

### Browse and find

- **Grid and filmstrip:** egui-pop's virtualised `ThumbGrid` (a hundred
  thousand cells cost a screenful), shared selection, rubber band, `+`
  and `-` sizes, captions and overlays, Enter or a double click opens the
  image. Thumbnails come from the disk cache, else the raw's embedded
  preview, the cells in view first (`Library::set_on_view`).
- **Search field:** words (name or tag), `rating>=N`, `label:`, `flag:`,
  `tag:`/`#tag`, `date:a..b`, `backup:met|short|unverified|none`, chips.
  Rating, labels, flags and tags become a vault `Query`
  (`library::query_of`); words, dates and backup status are matched in
  the window over what was loaded.
- **Dates are the file's modification day**, not the capture time
  (`persist::read_library` reads `fs::metadata`). The capture time is
  measured at import (`helios_library::signals`), but stored in a
  signals document, not as a field a query can use.
- **Sort:** none for the user. `Order` has `Id` (the content hash: a
  random order to a person) and `RatingDesc`; the grid shows `Id`.
- **Collections and folders:** none. A workspace template's scope is a
  `QueryDesc` (rating, labels, flags, tags: `helios-template`), the only
  saved filter. No folder tree, no albums, no recent collections.
- **Saved searches:** none beyond the template's query.

### Act on many photos

| Action | State |
|--------|-------|
| Select | grid selection (click, Shift, Ctrl, rubber band, Ctrl+A in the grid); no select none, invert, "select same" commands |
| Rate, flag, colour label | built: `vault.rate.N`, `vault.flag.*`, `vault.reject`, `vault.label.*`, on the selection, auto-advance, one undo step each |
| Tag | built: tag input with completion; tags are flat strings (`places|France|Lyon` kept as one tag from darktable) |
| Title, description | fields exist in the vault (`fields.rs`), filled by importers; no editor in the window |
| Copy files out, paste files in | built (`library.copyFiles`, `library.pasteFiles`) |
| Export | built (`library.export`, one or many) |
| Remove from Library | in the vault (`Vault::set_in_library`, used by import undo); **no command** in the window |
| Rename, move, copy on disk | none after import (import renames by pattern: `helios_library::rename`) |
| Delete or trash | none |
| Rotate | none in the library; EXIF orientation is read for display (`helios-load/src/colour.rs`) |
| Copy and paste edits, marks | edits: the clipboard design ([clipboard.md](clipboard.md)); marks between photos: none |

### Metadata

- **Read:** camera, lens and capture time per sighting; rawler gives the
  camera metadata to the loader. No EXIF or IPTC panel; ISO, aperture,
  shutter, focal length are not stored for queries.
- **Keywords:** flat tag strings, no tree, no synonyms, no rename.
- **darktable sidecars:** read thoroughly at import (`helios-xmp`), each
  `.xmp` and `_NN.xmp` a version, `library.db` through the bundled
  importer plugin. Files changed outside Helios are detected
  (`thumb_check`, a "changed" badge).
- **Writing back:** nothing is written into or beside a photo. Marks and
  edits live in the catalogue only (backed by snapshots).

### Stacks, duplicates, versions

- **Exact duplicates:** one image per content hash; import skips, keeps
  or asks (Duplicates panel).
- **Near duplicates:** signals measured at import; `stacks.suggest`
  proposes bursts and brackets, a person accepts (Stacks panel). Stacks
  collapse in the grid (S toggles), one level, dissolvable.
- **Manual stacks:** `library.stacks.dissolve` exists, but no "group the
  selection" command (Ctrl+G in Lightroom and darktable).
- **Versions:** each edit step is a version; darktable duplicates are
  heads of the one image (2026-10-29: plain heads). The grid shows one
  cell per image; heads show only in Compare (`Library::Heads`). No
  "duplicate" (virtual copy) command.
- **RAW+JPEG pairs:** not decided (ROADMAP 2026-10-13); today two images.

### Storage

All built and documented in [../library.md](../library.md): volumes with
roles, offline and missing badges, `repair` relinking by content,
forgetting folders and volumes, the blacklist, the backup strategy and
job, per-photo backup badges and filters, "safe to format", catalogue
snapshots and restore, the Storage page and the health check's backup
and catalogue checks. Missing: a folder view of what is where, moving a
folder (relocation) from Helios, and "find this missing folder" for a
volume whose path changed in a way `repair` does not see.

### Performance at scale

- **The library read** (`persist::read_library`) fetches every row of
  the Library at once, then for each photo calls `Vault::locations` (one
  query) and `std::fs::metadata` (one stat, possibly on a network share
  or a sleeping disk), on the window's start and on every refresh (after
  an import, a scan that changed presence, a backup). At 100,000 photos
  that is 100,000 queries and 100,000 stats per refresh, and the whole
  `Vec<Entry>` rebuilt.
- **Matching in the window:** words, dates and backup status filter the
  in-memory entries; the backup statuses are read for the whole Library
  on each refresh (`strategy::statuses`).
- **The grid itself** is virtualised and holds textures under a budget
  (`system.cache.textures`); the disk thumbnail cache has a 2 GiB limit,
  content-addressed, with LRU. The cache service ([cache.md](cache.md))
  will own the tiers; its later slices (thumbnail textures as one entry
  at several sizes, the head renders under a budget) matter for long
  scrolls.
- **The vault** has cursors (`open_cursor`, `fetch` by range,
  `refresh` with a delta, `rank_of`): the building blocks for paging and
  incremental updates exist, unused by the window.

## 2. Established applications

What users expect, by application. Keys are the defaults; all are
rebindable in each application.

### darktable lighttable (the fork's sources)

- **Keys:** 0 to 5 stars, R reject (`src/libs/tools/ratings.c`); F1 to
  F5 colour labels red, yellow, green, blue, purple
  (`src/libs/tools/colorlabels.c`); Ctrl+A, Ctrl+Shift+A, Ctrl+I select
  all, none, invert (`src/libs/select.c`); Ctrl+D duplicate, Ctrl+Shift+D
  duplicate virgin, Ctrl+G group, Ctrl+Shift+G ungroup, Delete remove
  (`src/libs/image.c`); Ctrl+T tag, Alt+T redo last tag
  (`src/libs/tagging.c`); Ctrl+K collection history
  (`src/libs/collect.c`); X culling, F sticky preview, W full preview
  held, Ctrl+W with focus detection (`src/libs/tools/lighttable.c`);
  I show image information held; Home, End, Page Up and Down, with Shift
  to extend the selection (`src/views/lighttable.c`).
- **Collections:** a stack of rules, each a property and a pattern,
  combined with and, or, except: film roll, folder, file name, camera,
  lens, aperture, exposure, focal length, ISO, day, time, import, change,
  export and print dates, geotagging, aspect ratio, tag, colour label,
  metadata fields, grouping, local copy, history, module used, rating,
  white balance, flash, dimensions, duplicates and more
  (`src/common/collection.h`, `DT_COLLECTION_PROP_*`). Recent collections
  (ten by default) and presets act as saved searches. A separate
  "filters" module adds quick ranges (rating, colour, text search, date)
  and the sort order (several keys).
- **Selected images module:** remove, delete (to the system trash by
  default, `send_to_trash`), move, copy, create HDR, duplicate, rotate
  90° each way, reset rotation, copy locally and resync (for offline
  volumes), group, ungroup; metadata copy, paste (merge or overwrite) and
  clear for ratings, colours, tags, geotags and text; refresh EXIF.
- **Sidecars:** `write_sidecar_files` never, after edit, or on import
  (the default), `file.ext.xmp` with the whole history; a crawler finds
  sidecars newer than the database at start (`run_crawler_on_start`).
- **Best at:** query power (any property, combined), and the sidecar as
  an emergency backup of every edit.

### Lightroom Classic, Library module

- **Keys:** G grid, E loupe, C compare, N survey, D develop; 0 to 5
  stars, P pick, X reject, U unflag, 6 to 9 colour labels; Caps Lock
  auto-advance; Ctrl+' virtual copy; Ctrl+G stack, Ctrl+Shift+G unstack,
  S expand or collapse a stack; Ctrl+[ and Ctrl+] rotate; F2 rename;
  Backspace remove or delete (a dialog asks: remove from the catalogue,
  or delete from disk to the trash), Ctrl+Backspace delete rejected
  photos; B add to the target (quick) collection; Ctrl+K keywords;
  `\` the filter bar; Ctrl+F text search; Ctrl+S save metadata to file;
  Shift+Ctrl+Alt+C copy settings, etc.
- **Browse:** Folders panel (volumes with free space, folders with
  counts, missing folders marked with a question mark, "Find missing
  folder", "Synchronize folder", "Show in Explorer/Finder", folders moved
  by drag and drop on disk), Collections (manual collections, smart
  collections from rules, collection sets, one quick collection), the
  filter bar (text, attribute, metadata columns that narrow each other,
  presets), sort by capture time, added order, edit time, rating, pick,
  label, file name, file type, aspect, user order (in a folder or a
  manual collection).
- **Metadata:** EXIF shown, IPTC edited with templates, keyword list with
  hierarchy, synonyms and export flags, keyword sets, painter tool;
  "automatically write changes into XMP" off by default, Ctrl+S on demand;
  metadata conflicts badged when the file changed outside.
- **Best at:** the folder tree tied to the disk, smart collections, the
  quick collection, and the keyword hierarchy with synonyms.

### digiKam

- **Keys:** Ctrl+0 to Ctrl+5 stars; Alt+0 to Alt+3 pick labels (none,
  rejected, pending, accepted); Ctrl+Alt+0 to Ctrl+Alt+7 colour labels
  (none, red, orange, yellow, green, blue, magenta, grey) (digiKam's
  shortcut documentation).
- **Browse:** albums are folders on "collections" (local, removable,
  network, each with an online state), plus views by tags, labels,
  dates, timeline, map, people (face recognition), similarity search
  (sketch or image), and a duplicates finder; advanced search saved as
  searches.
- **Metadata:** reads and writes EXIF, IPTC and XMP in the file or a
  sidecar (configurable), a tags manager with hierarchy, rename and
  merge; versioning of edited files.
- **Best at:** search breadth (faces, similarity, map, timeline) and
  metadata writing done thoroughly.

### Capture One

- **Model:** sessions (a folder per shoot: Capture, Selects, Output,
  Trash) or catalogs; albums, smart albums, projects, groups; filters
  tool with counts per value; sort by many keys.
- **Keys:** 0 to 5 stars; colour tags with `-` red, `+` green, `*`
  yellow (other colours unbound by default); F2 new variant, F3 clone
  variant (vendor shortcut lists; defaults vary by version).
- **Variants:** several variants of one raw, each with its own
  adjustments and rating, a "primary" variant, "edit selected variants"
  to apply to many.
- **Best at:** sessions (a shoot is a self-contained folder), variants,
  and the filters tool counting each value.

### Photo Mechanic

- **Model:** a contact sheet over folders, no catalogue needed (the
  "Plus" edition adds one); speed is the product: previews from embedded
  JPEGs, culling at the camera's pace.
- **Keys:** T tags (a pick) a photo, 1 to 8 colour classes, a modifier
  with 1 to 5 for stars; Ctrl+I the IPTC stamp; variables and code
  replacements in every text field.
- **Ingest:** several destinations at once, rename by variables, IPTC
  template applied on ingest.
- **Metadata:** writes IPTC and XMP straight away (into JPEG, sidecars
  for raws), interoperable with every other application.
- **Best at:** ingest and captioning speed (templates, variables, code
  replacements), and no catalogue between the photographer and the
  files.

### LightCraft (a Rust Lightroom clone, reviewed 2026-11-12)

An open-source, pure-Rust, egui/wgpu copy of Lightroom (MIT OR Apache-2.0),
built by agents at a fast pace. Its catalogue lives entirely in memory, an
append-only journal plus snapshots, so it does not scale like the vault, but
its *library model* is a useful checklist:

- **Smart albums as rule trees**: a rule is `{field, op, value}` or a nested
  group with its own match mode (all, any, none). Each field has a kind: text,
  keywords (a list of names), number, date, album, a fixed choice, or a flag.
  The operators follow the kind: contains, is, starts with, empty or not;
  equal, less, greater, between; in the last N days; in an album or not.
- **Its fields, by group**: rating, flag, label and text at the top level;
  source (album, virtual copy, copy name, stacked); file (name, extension, path,
  kind, format, duration); dates (capture, import, edit); keywords and people
  (with counts); description (title, caption, alt text, creator, copyright and
  its status); camera (camera, lens, focal length, aperture, shutter, ISO,
  flash, exposure bias, body serial); location (place, city, state, country,
  has GPS); size (long and short edge, aspect, megapixels); develop (edited,
  cropped, treatment); assisted culling (sharpness, best of its group).
- **A keyword tree with synonyms**, rename and merge.
- **A field search syntax** (`rating:>3 iso:>800 camera:…`), and seven sort keys.

What Helios takes: the field list as a checklist for L4's keys and L6's smart
albums, the rule-tree shape and its operators per kind for L6, and synonyms,
rename and merge for L12 after the path tags (L-C7). What it does not take:
the in-memory catalogue; everything runs on the vault `Query` and cursors.

### What users carry from one to another

- Number keys for stars, a reject key, colour labels on keys, a pick
  flag: Helios has them (Lightroom's map, darktable's R and F-keys).
- A **folder tree** that mirrors the disk, with counts and offline
  folders shown: every application has one.
- **Saved queries**: darktable's presets and recent collections,
  Lightroom's smart collections, digiKam's saved searches, Capture One's
  smart albums.
- **A quick, temporary set** (Lightroom's quick collection, B), and
  **manual albums**.
- **Sort by capture time** as the default everywhere.
- **Delete goes to the system trash**, after a question that also offers
  "remove from the library only".
- **Metadata in sidecars** for interoperability, written on demand or
  automatically, never forced into raw files.
- **Copies of one photo** (virtual copy, duplicate, variant) shown as
  cells of their own in the grid, grouped with the original.

## 3. Gaps

Ordered by how much other work depends on them.

| # | Gap | Why it matters |
|---|-----|----------------|
| G1 | The library read touches every photo's locations and file on each refresh | 100k photos: slow start, slow refresh after each import or scan, stats on sleeping disks |
| G2 | No capture time, camera, lens, exposure as indexed fields | dates are wrong (file day), no sort, no camera or lens filter |
| G3 | No sort | the grid is in hash order: unusable past a few hundred photos |
| G4 | Words, dates, backup status matched in the window | the filter cannot run on a vault the window did not load, nor on a remote one |
| G5 | No folder view | the first thing users of every other application look for |
| G6 | No collections, albums, saved searches, quick collection | no way to keep a set or a question |
| G7 | No file operations (remove, trash, rename, move, rotate) | basic housekeeping needs another application |
| G8 | No keyword tree | darktable and Lightroom users bring hierarchies |
| G9 | No metadata panel, no IPTC editing, nothing written back | captioning trades (press, events) cannot work; no interoperability |
| G10 | Versions not in the grid, no duplicate command | darktable and Lightroom users expect a copy as a cell |
| G11 | No manual grouping (Ctrl+G), RAW+JPEG pairs undecided | stacks only from proposals |
| G12 | No relocation (moved folder, changed mount) beyond `repair` by content | slow re-hash of a whole archive where a path prefix would do |
| G13 | No library window of its own | the Library focus needs windows slice 4 |

## 4. The Library in the Focus model

[windows.md](windows.md) splits the process into a `Core` (one per
process) and `Focus` (one per window); the vault, the cache service and
the node connection are the host's. The library fits it this way:

| Part | Lives in | Holds |
|------|----------|-------|
| The rows | host (vault) and core | the catalogue, queried by cursors; a shared row cache of what any window shows (the marks, presence, capture data of the photos in some window's view) |
| Thumbnails | cache service (host) | content-addressed, one copy for every window |
| The library view | each focus | its scope (the focus's query), its browse state (folder, collection, filter, sort), its cursor on the vault, selection and cursor image, grid size, expanded stacks |
| Library steps | core today; per focus with windows slice 7 (P5) | the undo of marks, file operations, collection edits |
| Collections, saved searches, keywords | vault documents | shared by every window and every node |
| File operations | jobs (core, host) | tagged with the focus that started them (T7) |

So a **library window can exist on its own**: it is a focus whose scope
is the whole library, whose layout is the browse tree, the grid, the
filter bar and the metadata panel, and which opens the darkroom as a
place inside it or hands a selection to a new edit focus ("Edit in a new
window"). Every slice below keeps the browse state in the focus's
library view and the data in the vault, so slice 4 of windows.md (the
list of foci) finds nothing global to untangle.

## 5. Choices for the owner

Each has options A, B, C, and a recommendation. All nine were decided on
2026-11-11, each with its recommended option.

**L-C1. The grid's default order, and which time is "the date".**
- A. Capture time (EXIF `DateTimeOriginal` with its offset when present,
  else the file's time), oldest first; ties by file name.
- B. Import order (newest import first), as a camera roll.
- C. File name.

Recommended: **A**, as every application above; the sort menu offers
the others, kept per focus.

**Decided (2026-11-11): A.**

**L-C2. How photos are gathered.**
- A. Three kinds in one tree: **folders** (mirroring the disk, read from
  where copies are), **albums** (an explicit set the user fills) and
  **smart albums** (a saved filter), plus one **quick collection** (B
  adds or removes). Lightroom's and Capture One's model.
- B. darktable's model: everything is a rule-based query, with recent
  queries and presets; no explicit sets.
- C. Tags only: an album is a tag, a saved search is a tag query.

Recommended: **A**. Explicit sets are what people expect and what agents
can fill; smart albums are the saved searches; folders answer "where is
it". The filter grammar can grow darktable's properties over time.

**Decided (2026-11-11): A.**

**L-C3. Writing metadata back to files.**
- A. Never: the catalogue only, with its snapshots; exports embed the
  metadata.
- B. On command: "Save metadata to files" (Ctrl+S in the library, as
  Lightroom) writes a sidecar per photo; a library setting can make it
  automatic later.
- C. Automatic from the start: each change written to the sidecar by a
  background job, as darktable's default.

Recommended: **B**, then the automatic setting once the format is
proven. Whatever the option: **never write into raw files**; write the
standard fields (rating, label, title, description, keywords, IPTC core)
in a Helios sidecar `photo.ext.helios.xmp` or merge into an existing
`.xmp` without touching darktable's `darktable:` history (to be decided
in that slice; darktable must still read its own sidecar).

**Decided (2026-11-11): B.**

**L-C4. Deleting.**
- A. Two verbs: "Remove from Library" (the flag, undoable, no file
  touched) and "Move to Trash" (the system trash, asked first, journaled),
  plus "Delete rejected…" (Ctrl+Backspace). Never on a card, a backup or
  an archive volume.
- B. Helios's own trash folder per volume, emptied after a retention
  period (undo across sessions).
- C. Remove only; files are deleted outside Helios.

Recommended: **A**; the system trash gives the undo users know, and the
backups protect the rest.

**Decided (2026-11-11): A.**

**L-C5. Copies of one photo (virtual copies, darktable duplicates).**
- A. Each head of an image is a cell of its own in the grid, grouped
  with the image (a stack of versions), "Duplicate" (Ctrl+D or Ctrl+')
  makes a new head. Marks stay per image (fields are per image).
- B. One cell per image with a badge counting its heads; a version strip
  in the image's panel; "Duplicate" adds a head shown there.
- C. A as an option of the view ("Show versions as cells").

Recommended: **C with B as the default**: it keeps the 2026-10-29
decision (plain heads, no named variants) and the one-image-one-cell
grid, and darktable and Lightroom users get cells on request. If the
owner wants per-copy ratings (Lightroom, Capture One), marks must move
to versions: a larger change, to decide before slice L10.

**Decided (2026-11-11): C, with B as the default.**

**L-C6. RAW+JPEG pairs** (open since 2026-10-13).
- A. One image with two files (Lightroom's "treat as one").
- B. A stack made at import without asking (a pair is certain), the raw
  on top.
- C. Two images, nothing linked.

Recommended: **B**: an image is the hash of one file, so A breaks the
identity model; stacks already show collapsed and a filter flattens
them.

**Decided (2026-11-11): B.**

**L-C7. Keyword hierarchy.**
- A. Path tags (`places|France|Lyon`, as darktable and as imported
  today), a tree drawn from the separator; renaming a branch is one
  batch step.
- B. Keywords as entities (parent, synonyms, "include on export"), as
  Lightroom's keyword list.
- C. A now, B's synonyms and export flags as documents beside the path
  tags later.

Recommended: **C**.

**Decided (2026-11-11): C.**

**L-C8. File operations on disk (rename, move).**
- A. Helios renames and moves files and folders, sidecars along,
  journaled and undoable, inside volumes with the library or archive
  role.
- B. Only inside the library folder Helios lays out.
- C. None: the user does it outside and `repair` follows.

Recommended: **A**, with the same rules as the backup job (write only
where the roles allow, never on a card).

**Decided (2026-11-11): A.**

**L-C9. Browse state per window.**
- A. Each focus has its own folder, collection, filter and sort.
- B. One browse state for the whole process.

Recommended: **A**, as windows.md's model (the library view is the
focus's).

**Decided (2026-11-11): A.**

## 6. Slices

Small, ordered, each shippable and testable headless (vault and library
tests, kittest scenarios and snapshots, agent commands). "Focus" says
where the slice's state lives.

**L1. The library read at scale.** `read_library` reads rows, presence,
the offline volume and the first present path in one query joining
`locations` (no per-photo query); the file's size and date come from
the sightings the map keeps, never a `stat` in the read. The window
keeps a cursor and fetches what the grid shows plus a margin
(`fetch` by range, `refresh` deltas after writes) instead of rebuilding
every entry. Test: a vault test with 100,000 synthetic images asserting
one statement per read and a time deadline; a kittest scenario scrolling
the grid end to end. Focus: the cursor is the focus's; the row cache is
the core's.

**L2. Capture metadata as fields.** Capture time (with offset), camera,
lens, ISO, aperture, shutter, focal length, width, height, orientation
and file type become indexed read-only image fields, written at import
from what `signals` and the sightings already read; a background job
fills the existing Library (resumable, in the jobs centre). The search
field's dates use the capture day. Test: import fixtures, assert
fields; migration test for the backfill. Focus: none (vault).

**L3. Sort.** `Order` gains capture time, import time, file name,
rating, last edit, file size, with direction; a sort menu in the
Library panel's header and `library.sort` for agents; kept per focus and
in templates. Stacks keep members after their top. Test: vault order
tests; a snapshot of the menu; an agent scenario. Focus: per focus
(L-C1, L-C9).

**L4. One query for the filter.** Words, dates, backup status, camera,
lens, folder, file type, presence (`missing`, `offline`) run in the
vault query; egui-pop's `Filter` grammar gets the new keys (an upstream
change, worked around with Helios's own chips first). The facet counts
(how many per label, per camera) come with it. Test: query tests per
key; the window's filter equals the vault's answer on a fixture.
Focus: the filter is the focus's. Checklist for later keys (from
LightCraft's fields, §2): ISO, focal length, aperture, shutter, exposure
bias, flash, body serial, edited, cropped, has versions, keyword count,
has GPS, megapixels, aspect, import and edit dates.

Built (2026-11-11): every term is a `Query` field run in the cursor's
statement (`camera:`, `lens:`, `type:`, `folder:`, `presence:` and
`backup:` are Helios's own keys in the search text, quoted values
allowed); the facet counts (rating, label, flag, camera, lens, file
type, presence, backup state) are one store query, shown under the
toolbar while a filter is in force. Not moved yet: the window still
holds every entry of the Library (read in windows of 2,000), so the
grid's paging, the badges' backup marks and the tag completion still
read the whole Library.

Built (2026-11-13): the culling measures (focus, clipping) are read-only
fields (`focus`, `clip_white`, `clip_black`) copied from the signals
whenever they are written, so `cull:sharp|soft|clipped|unmeasured`,
the `cull` facet, the sharpness sort (L3, sharpest first) and smart
album rules on those fields run in the store. The soft threshold is an
absolute focus value, a first guess to tune on real photos.

**L5. Folders.** A Folders section in the Library panel: volumes, then
folders with counts, from the Library's locations; offline volumes
greyed with their "last seen"; a click scopes the grid
(`folder:` key from L4); "Show in file manager", "Scan this folder",
"Import new photos here". Test: tree built from a fixture vault; a
kittest click; offline state. Focus: the selected folder is the
focus's browse state.

Built (2026-11-12), first part: a Folders section in the Library panel
(collapsed; the vault counts when it opens, again after the photos
change): every folder holding a copy, with how many photos are there or
below (`Vault::folders`, one recursive query; each count is what the
`folder:` filter selects); chains of folders holding only the next one
are one row; a click scopes the grid with the `folder:` term, a second
click drops it.

Built (2026-11-12), second part: the volumes are the top level. The
vault counts each volume's folders apart (`Vault::folders` keyed by the
copies' volume; a photo with copies on two volumes counts on each), the
tree names each volume from the registry (its name, else its label),
with the photos it holds; volumes at hand first, then offline ones,
greyed and saying the day they were last seen; copies on no known volume
last, as "Other folders". An offline volume's folders still filter the
grid (the photos show from the thumbnail cache). Not built yet: the
row's menu (show in file manager, scan, import here), the selected
folder kept per focus and in templates, and a dock panel of its own.

Built (2026-11-12), third part: a right click on a folder or a volume
opens its menu through the shell's menus (`dialogs::context_menu`, the
entries commands): "Show in file manager" (`library.openFolder`), "Scan
this folder" (`library.scanFolder`: the image map's job lists that
folder and those below it, as a volume asked for on purpose) and
"Import new photos here" (`library.importWindow` on the folder); all
three greyed on an offline volume. The tree's click runs
`library.folder`, which scopes this focus's library and writes the
folder to the workspace's `library.folder` (one undo step, as the sort):
the workspace opens on it again, and a changed query keeps it. Not
built yet: a dock panel of its own for the tree.

**L6. Albums, smart albums, quick collection.** Vault documents
`library/collections` (L-C2): albums hold image ids, smart albums a
filter, stored as a rule tree: `{field, op, value}` rules and nested
groups with their own match mode (all, any, none), operators per field
kind (LightCraft's model, §2), compiled to one vault `Query` so a smart
album pages and counts like L4; the quick collection is one album; B toggles it; Ctrl+K recent
filters (darktable's collection history; Lightroom's Ctrl+K is keywords,
so the profile decides). Undoable steps; agent commands
`library.collection.*`. Test: vault documents, undo, a scenario that
makes an album from a selection. Focus: the collections are shared;
which one is shown is the focus's.

Built (2026-11-12), first part: **albums**. Each is a vault document in
`library/collections` (`{kind: "album", name, images}`, keyed by an id
made from its first name, kept when it is renamed; names unique whatever
the case); `Query::album` selects its photos in the cursor's statement
(the ids the document holds, `json_each` in SQLite). The Library panel's
Albums section lists them by name with how many photos of the Library
each holds; "New album…" asks a name and takes the selection; each row's
menu adds or removes the selected photos, renames, deletes. A click runs
`library.collection`, which scopes this focus's library with the
`collection:` term and writes the workspace's `library.collection` (one
undo step, as the folder). Each change is a command
(`library.collection.create`, `.rename`, `.delete`, `.add`, `.remove`,
`.list`) the host makes in the vault as one step of the library's
history, holding the document before and after (`Changes::documents`;
an undo leaves a document changed since alone).

Built (2026-11-13), second part: **smart albums**. A document
`{kind: "smart", name, rules}` in the same namespace, its `rules` a tree
(`helios-vault::rules`): groups `{match: all|any|none, rules: [...]}`
nested at most 8 deep, conditions `{field, op, value}` on rating, label,
tag, flag, folder, album, captured, camera, make, lens, file type,
title, description, ISO, focal length, aperture and shutter; the
operators follow the field's kind (numbers: `=`, `!=`, `<`, `<=`, `>`,
`>=`, `between`; sets: `has`, `lacks`; texts: `is`, `is_not`,
`contains`; the flag: `is`, `is_not`; folder and album: `in`,
`not_in`). An unknown field, an operator the kind does not take or a
wrong value is refused with where it is (`rules[1][0]: unknown field…`).
`Query::rules` holds a tree; each store compiles it with the rest of the
query (SQLite: one boolean expression of `EXISTS` sub-queries in the
cursor's statement, each condition 0 or 1 so `none` negates cleanly; the
memory store: the same rules per photo). `Query::album` naming a smart
album runs its rules in its place, so the `collection:` term, the
workspace's `library.collection`, facets and the folders tree work
unchanged. The Albums section marks a smart album (⚙) and shows its live
count (one query each, read again whenever the library's history
moves). "New smart album…" opens a form (name, all or any, stars,
colour label, flag, tag, camera, file type, folder, a range of days);
"Edit the rules…" opens it again when the tree is one group the form
shows, else says to set them with `rules`.
`library.collection.create` takes `kind: "smart"` and `rules` (or the
form's keys); `library.collection.rules` sets them, one undoable step;
photos are never added to or removed from a smart album.

Built (2026-11-13), third part: **the rule editor and relative dates**.
"Edit the rules…" (`library.collection.rules` without `rules`) opens a
dialog over the window the Library is in, on any tree: each group with
its all, any or none and its "+ Condition" and "+ Group", each
condition with its field, operator and value boxes by the field's kind
(a number, two bounds for `between`, a day for the capture date, a
choice for the flag, colour labels and albums), moved up or down or
removed. The draft keeps each condition as typed: one not valid shows
the vault's reason under it, and Apply waits until none is left. Each
edit is one step of the dialog's own undo (typing on in one box is one
step); Apply is one step of the library's history, as before. Agents
make the same edits with `library.collection.rules.edit` (`edit`, `at`
a path of indexes, the edit's own keys; `undo`, `redo`, `apply`,
`cancel`), which answers the draft, its faults and the rules when valid.
The tree does not fit the declarative forms (flat fields): the dialog is
drawn by hand, its words through the catalogues.
The capture date (`captured`) takes `within` a relative period,
`"today"`, `"this_year"` or `{"last": n, "unit": "days" | "weeks" |
"months"}` (the n days, weeks or months ending today, today included),
stored as written. A store resolves it when it runs the query
(`rules::resolve`): against `Query::now` when set (tests), else the
system clock moved by the local offset the application sets at its
start, so "today" is the local day, as cameras write local times. It
becomes a whole-day range `[start, end)` in the tree the store compiles,
so SQLite and the memory store select the same photos.

Built (2026-11-13), fourth part: **the quick collection** (Lightroom's,
B). One more document in `library/collections`, under the key `quick`
and of that kind, `{kind: "quick", images}`: no document yet is an
empty one; it is not listed with the albums, cannot be renamed or
deleted, and no album takes its key (a new "Quick" gets `quick-2`).
`Query::album` naming it selects its photos as an album's, so
`collection:quick` scopes the grid in both stores. B (bound like the
culling keys, so not while a text field is typed in, rebindable) runs
`library.quick.toggle`: when the quick collection holds every photo of
the selection they go out, else those it lacks go in; one step of the
library's history per press. The Albums section pins it first
("⚡ Quick collection" and its count, its menu: add or remove the
selection, "Save as album…", clear), and each photo in it shows a
"quick" badge in the grid. `library.quick.saveAs` (`name`, `clear`;
without `name`, a form with both) makes a plain album of it, emptied or
not, as one step; `library.quick.clear` empties it. The import window
shows files still to import, not Library photos, so B has nothing to
act on there.

Built (2026-11-13), fifth part: **the recent filters** (darktable's
collection history). A filter used in the Library (applied by
`library.filter.apply`, a facet clicked, a folder or an album chosen, or
typed in the search field and left two seconds) goes first of the recent
ones, once, ten at most; a pinned one leaves that list and stays until
unpinned (`library.filter.pin`, the filter in force by default). Both are
one library document (`library/filters`, key `history`: each filter as
the search field's structure), read when the Library opens and written
when they change, outside the library's history: they survive a restart
and follow the library, not the workspace. Ctrl+Shift+K (rebindable, in
the Library, not while a text field is typed in) opens the switcher in
the window it is asked from: the pinned filters, the recent ones, then
the smart albums (as their `collection:` term), each with what it is
beside it, filtered by typing; Enter applies the one chosen to the grid.
Ctrl+K, darktable's key, starts chords in Helios (Ctrl+K Z, Ctrl+K
Ctrl+S), hence Shift; Lightroom's Ctrl+K is keywords. Agents list them
with `library.filter.recent`. Not built yet: album sets, drag and drop
onto an album, keyboard undo in the rule dialog (its buttons only).

**L7. Selection and batch commands.** Select none, invert, select
similar (same day, same stack), Ctrl+Shift+A, Ctrl+I; copy and paste
marks (rating, labels, tags, title) between photos, merge or overwrite,
as darktable's metadata copy. Test: commands over a fixture; undo.
Focus: selection is the focus's.

**L8. Remove and trash.** "Remove from Library" as a command on the
selection (the flag, undoable: the vault does it already); "Move to
Trash…" and "Delete rejected…" (L-C4) through a journaled job, refused
on cards and backup volumes, the system trash per platform. Test: a
temporary folder fixture, the journal replayed after a kill, refusals.
Focus: a job tagged with its focus.

**L9. Rotate.** An orientation field on the image (or the version, to
decide in the slice), Ctrl+[ and Ctrl+] (and darktable's equivalents in
its profile), thumbnails and the loupe rotated without decoding again.
Test: field commands, a snapshot of a rotated cell.

**L10. Versions in the grid.** "Duplicate" makes a new head from the
current or the original edit; the grid shows heads per L-C5 (badge and
strip, cells as an option). Test: vault heads; a snapshot of both
views. Focus: the view option is the focus's.

**L11. Manual stacks and pairs.** Group the selection (Ctrl+G), ungroup
(Ctrl+Shift+G), set the top; RAW+JPEG pairs stacked at import (L-C6).
Test: stack commands; an import fixture with pairs.

**L12. Keyword tree.** A Keywords section: the tree from `|` paths with
counts, click to filter, drag on photos, rename and merge a branch as
one batch step (L-C7). After it, the "B extras": synonyms (a keyword
also found under its synonyms, exported to XMP as Lightroom does),
and keywords as entities that keep their identity through a rename. Test: tree from a fixture; a rename undone.

**L13. Metadata panel.** EXIF read-only (from L2 and the loader),
editable title, description, creator, copyright, keywords; templates
(Photo Mechanic's stamp) applied on the selection and at import. Test:
field commands; a template applied to many.

**L14. Writing back.** Per L-C3: "Save metadata to files", a sidecar
writer in `helios-xmp`, conflicts with files changed outside shown with
the existing badge, a read-back test against darktable's parser
behaviour (darktable still opens its own sidecar). Test: round trip on
fixtures; a sidecar darktable wrote stays readable by Helios's importer.

**L15. Rename, move, relocate.** Rename by the import's patterns
(`helios_library::rename`), move to a folder, sidecars along, journaled
(L-C8); "Find missing folder" relinks a path prefix in one step before
falling back to `repair` by content. Test: journal, undo, a moved
fixture folder found by prefix.

**L16. The Library window.** Window > Library (P10) opens the Library
focus on its own once windows slice 4 has the list of foci: its layout
is the browse tree, the grid, the filter bar and the metadata panel;
"Edit in a new window" hands the selection to an edit focus. Test: a
two-window kittest scenario (each window its own filter and selection,
one thumbnail cache).

L1 to L4 are plumbing and could ship as one milestone; L5 and L6 make
it a library people recognise; L7 to L11 are housekeeping; L12 to L15
serve the captioning trades and interoperability; L16 waits on the
windows work, but nothing before it blocks on it.

## Sources

- darktable sources (the fork): `src/common/collection.h`,
  `src/libs/collect.c`, `src/libs/select.c`, `src/libs/image.c`,
  `src/libs/tagging.c`, `src/libs/tools/ratings.c`,
  `src/libs/tools/colorlabels.c`, `src/libs/tools/lighttable.c`,
  `src/views/lighttable.c`, `data/darktableconfig.xml.in`
  (`write_sidecar_files`, `send_to_trash`,
  `plugins/lighttable/collect/history_max`, `run_crawler_on_start`).
- digiKam shortcuts: <https://docs.digikam.org/en/setup_application/shortcuts_settings.html>
- Capture One shortcut lists (defaults vary by version):
  <https://giggster.com/guide/capture-one-keyboard-shortcuts>,
  <https://alexonraw.com/capture-one-keyboard-shortcuts/>
- Lightroom Classic and Photo Mechanic: their published shortcut lists
  and manuals, as summarised in [../import-research.md](../import-research.md),
  [../shortcuts-research.md](../shortcuts-research.md) and
  [../storage-backup-research.md](../storage-backup-research.md).
