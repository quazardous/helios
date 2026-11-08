# The image map: every image seen, import as a flag

Status: accepted (2026-11-08), with recommendations 1 to 7. Slice 1
built (2026-11-08): schema 5, the migration, the sighting and flag calls
of the `Store` (`put_sighting`, `sightings_of`, `sightings_like`,
`library_flag`, `set_library_flag`; `Vault::set_in_library` is an undo
step) and every Library read gated on the flag, with contract tests on
both stores. Slice 3 built (2026-11-08): import sets the flag
(`Vault::import_known`), `Store::add_image` records off the Library,
undo clears the flag keeping sightings and signals, duplicates and the
import window's "in your Library" are lookups. Slice 4 built in part
(2026-11-08): provisional keys and their renaming (`Key::provisional`,
`ThumbCache::rename`) and the re-check queue (`thumb_check`: on screen,
recently shown, then the Library; stat, partial, full hash; a changed
file's thumbnail made again; waits on battery and under load), fed by
the import window, the Library grid and Compare (their images' files,
from their sightings); the import window and dialogs on the shared
cache are left. Slice 5 built in part (2026-11-08): the map's job
scans a volume plugged in (`Feed::plugged`) or asked for
(`library.map.scanNow`), a folder at a time, known folders first, a
`stat` per file and no read, right after what is on screen (cheaper
than any read), pausable and giving way as its reads do;
`system.map.record` says which volumes (every one, those with a role,
none); a scan's end prunes to `system.map.limit`. A scan asked for by
name lists a volume forgotten whole again. Renames and moves, read
throttling on slow volumes, the wipe and the Storage page's view of the
map are left. Open question 6
decided (2026-11-08, option A of the owner's choice): a Library photo
whose file another application changed stays linked (section 3, "A file
changed in place"). Section 9 gives the order of work, section 10 the
questions as they were put.

The owner's request: "Helios's database records every image it sees on
every known volume. Import only means changing a flag, plus, when needed,
one or more physical copies to meet the replication strategies." And: "a
default blacklist per OS, plus settings".

In short:

- the vault keeps **images** (content identity, as today) and
  **sightings**: each file Helios has listed, on a volume, at a path, with
  what a listing tells cheaply (size, modification time) and what a read
  tells later (partial hash, full hash, capture time, camera, lens);
- an image is **in the Library** when its `in_library` flag is set, with
  when and by what. Today's locations become the sightings of library
  images;
- **import** sets the flag and makes the copies the strategy asks for
  (zero, one or several), through the verified copy job that exists;
- "in your Library", duplicates, "this photo is also on BACKUP_A" and the
  thumbnail of a file seen before become **lookups**, not reads;
- a **blacklist** (defaults per OS, plus `system.map.exclude`) keeps
  places out: never recorded, never thumbnailed.

## 1. Today

### The vault

`helios-vault` keeps an image as its content: `ImageId` is the BLAKE3
hash of the whole file (`helios_core::ContentHash`). The SQLite driver
(`helios-vault-sqlite`, schema in `src/migrate.rs`, `SCHEMA_VERSION` 4)
holds:

| Table | What it holds |
| --- | --- |
| `images (id, size, partial)` | one row per content: the full hash, the size, and `partial`, the BLAKE3 of the file's first and last 64 KiB and its size (`helios_vault::partial_hash`, `PARTIAL` = 64 KiB) |
| `locations (path PK, image, size, modified, last_verified, volume, rel_path, presence)` | where a copy of an image was seen; `presence` is `present`, `offline` or `missing` (migration 4, "locations on volumes") |
| `fields (image, name, value, number)` | marks and metadata (`rating`, `rejected`, `labels`, `tags`...) |
| `audit` | every field change, kept as a record |
| `versions (id, image, parents, edit, origin, at, label)` | edits, content-addressed (`VersionId`) |
| `documents (ns, key, value)` | everything else as JSON: `signals` (near-duplicate signals per image, with `taken`, `maker`, `model`), `storage/volumes`, `storage/strategies`, `storage/verified`, `library/decisions`, the library root |
| `vault_state` | small facts about the file (the clean-shutdown marker) |

The `Store` trait (`add_image`, `put_location`, `at_path`,
`all_locations`, `remove_location`, `remove_image`...) is what drivers
implement, checked by `helios_vault::contract`. `Location` carries
`path`, `size`, `modified`, `last_verified`, `volume: Option<VolumeId>`,
`rel_path` and `presence: Presence`; `Presence::of` gives an image's
presence from its locations. Every image row is a library image: the
vault has no notion of a file seen but not imported.

### Import

`helios_library::import::import` walks a source (`files_of`: a folder,
a file, a card from its `DCIM`), hashes every file whole (`hash_file`),
and per file either records a location where it is (reference) or copies
it into the library folder: the source read once, the copy hashed while
written in `.helios-part/`, read back, then named. Duplicates are found
by hash (`skip`, `keep`, `ask`). Signals and darktable sidecars are kept.
One import is one job and one undo step of the `library` scope; undo
removes the images and locations the import added, never a file
(docs/library.md, "Undoing an import").

The import window (`helios-gui/src/shell/import_window.rs`, design in
[import-window.md](import-window.md)) lists sources with
`import_scan::Scan`: a thread lists the files, then takes a **glance** at
each (`helios_load::glance`: the smallest embedded preview, the capture
time and the camera; it reads the whole file into memory but hashes
nothing). `import_scan::Prefilter` greys "in your Library" by size, then
by partial hash against the vault's `(size, partial)` pairs; the job
confirms it with the full hash. Nothing the window learns is kept: closing
it and opening it again lists, glances and prefilters everything again.

### Volumes and locations (#4830)

`helios-storage` lists the machine's volumes (`detect`, Linux only for
now), identifies each (`Identity`: marker file `.helios/volume.json`,
filesystem UUID, share address, else weak label and size), and keeps them
in the vault (`registry`, documents `storage/volumes`), offline ones
included. `scan::Scanner` runs at start, when `Watch` sees a volume come
or go, and every few minutes: it records the volumes, links each location
to its volume and path inside it, and sets `present`, `offline` or
`missing`. The path inside the volume is what counts: a new mount point or
drive letter loses nothing.

### Backups

A library has one `strategy::Strategy` (copies, `Destination`s,
`off_site_required`, `Trigger`s: `OnImport`, `OnConnect`, `ByHand`),
document `storage/strategies/library`. `strategy::status` counts a
photo's verified copies (`storage/verified`) on backup or archive volumes,
once per physical drive. The backup job (`helios_library::backup`,
`run`, `after_import`) copies from the library copy to `Helios Backup/`,
hash while copying, read back, then name, and records the copy as a
location.

### Thumbnails

`helios-thumbs`: `read` (the fast first read of the embedded JPEG),
`ThumbCache` (JPEG files in `.helios/thumbs` beside the vault file,
`cache_dir`, 256 and 1024 px, LRU under `system.cache.thumbnails`, 2 GiB)
keyed by `Key { image, version, chain }` (the image's full hash,
`preview` or a version id, the chain's version), and `Loader`, the pool,
cells in view first ([performance.md](../performance.md), "Thumbnails").
The import window does not use the cache: its cells are glanced, not yet
hashed, so they have no key.

### What this means

Helios already reads most of what a map needs, then forgets it: the
import window glances whole files and throws the bytes away, the
prefilter hashes in part and keeps nothing, the storage scan knows each
volume but records only library files. The map keeps what is read.

## 2. The model

### Images

`images` stays the table of contents: one row per full hash. It gains the
flag and its provenance:

| Column | Meaning |
| --- | --- |
| `in_library` | 0 or 1: whether the image is in the Library |
| `library_at` | when the flag was last set (or cleared) |
| `library_by` | what set it: an import's job id (`import-12`), `user` (a command), `agent:<name>`, `importer:<plugin>`, `migrated` |

An image is created as soon as a sighting's full hash is known, flag off.
Fields, versions, stacks and signals still hang on the image. Every
existing reader of the Library (catalog cursors, `Store::images`,
`select`, the grid, stacks, backup status) adds `in_library = 1`: the
map must never leak into the Library by accident. This is the main risk
of the change, and its contract tests check it (section 9).

### Sightings

A sighting is one file at one place, as last listed:

| Column | Meaning |
| --- | --- |
| `id` | row id |
| `volume`, `rel_path` | the place: the volume and the path inside it (today's `Location::volume` and `rel_path`) |
| `path` | the absolute path last resolved (a hint, as today) |
| `size`, `modified` | from the listing (a `stat`, no read) |
| `partial` | the partial hash, once read |
| `image` | the full hash (`images.id`), once read; `NULL` until then |
| `taken`, `camera`, `lens` | capture time, camera model and lens, once glanced |
| `first_seen`, `last_seen` | when this file was first and last listed here |
| `last_verified` | when its content was last read and matched (as today) |
| `presence` | `present`, `offline`, `missing` (as today) |
| `seen_by` | what listed it first: `import`, `dialog`, `drop`, `scan` |

A place holds one sighting: unique on `(volume, rel_path)`, or on `path`
for a sighting on no volume yet. A file that changes in place (same path,
another size or time) keeps its sighting; its `partial` and `image` are
cleared and read again (section 3).

A **folder** table keeps what is known per folder: `(volume, rel_dir,
last_listed, files, state)`, where `state` is `listed` or `forgotten`.
It gives scans their order (section 4) and makes "forget this folder"
stick.

### From locations to sightings

Today's `locations` are exactly the sightings of library images that have
a full hash. So:

- `Location` stays the vault's API type for a sighting with an image; the
  `Store`'s location calls read and write `sightings` where `image` is not
  null. `Presence::of`, repair, the scan and the backup job need no change
  in what they mean;
- the new calls are for sightings without an image (`put_sighting`,
  `sighting_at`, `sightings_under(volume, rel_dir)`, `sightings_like(size,
  partial)`) and for the flag (`set_in_library(id, on, by, at)`).

### Migration (schema 5)

One step, run in its transaction after the verified snapshot the migrator
already takes:

```sql
ALTER TABLE images ADD COLUMN in_library INTEGER NOT NULL DEFAULT 0;
ALTER TABLE images ADD COLUMN library_at INTEGER;
ALTER TABLE images ADD COLUMN library_by TEXT;
-- every image a vault holds today was imported
UPDATE images SET in_library = 1, library_by = 'migrated',
    library_at = (SELECT MIN(last_verified) FROM locations l WHERE l.image = images.id);
CREATE TABLE sightings (
    id            INTEGER PRIMARY KEY,
    volume        TEXT,
    rel_path      TEXT,
    path          TEXT NOT NULL,
    size          INTEGER NOT NULL,
    modified      INTEGER NOT NULL,
    partial       TEXT,
    image         TEXT REFERENCES images (id),
    taken         INTEGER,
    camera        TEXT,
    lens          TEXT,
    first_seen    INTEGER NOT NULL,
    last_seen     INTEGER NOT NULL,
    last_verified INTEGER,
    presence      TEXT NOT NULL DEFAULT 'present',
    seen_by       TEXT NOT NULL DEFAULT 'import'
);
INSERT INTO sightings (volume, rel_path, path, size, modified, partial, image,
                       first_seen, last_seen, last_verified, presence)
    SELECT l.volume, l.rel_path, l.path, l.size, l.modified, i.partial, l.image,
           l.last_verified, l.last_verified, l.last_verified, l.presence
    FROM locations l JOIN images i ON i.id = l.image;
DROP TABLE locations;
CREATE UNIQUE INDEX sightings_place ON sightings (volume, rel_path) WHERE volume IS NOT NULL;
CREATE UNIQUE INDEX sightings_loose ON sightings (path) WHERE volume IS NULL;
CREATE INDEX sightings_image ON sightings (image);
CREATE INDEX sightings_size ON sightings (size, partial);
CREATE TABLE folders (
    volume TEXT, rel_dir TEXT NOT NULL, last_listed INTEGER,
    files INTEGER, state TEXT NOT NULL DEFAULT 'listed',
    PRIMARY KEY (volume, rel_dir)
);
```

`taken` and `camera` of migrated sightings are filled from the image's
`signals` document by the first upkeep pass, not by the migration (it
stays SQL only). A `locations` view is not kept: no code outside the
driver reads the table. Undo steps recorded before the migration are
session-only, so none crosses it. The `MemoryStore` reference gets the
same model, and the contract tests run both.

The import record (`helios import-darktable`'s orphans) and every other
document stay as they are.

## 3. Identity and hashing

Four levels, each read only when the one before cannot decide:

1. **Unchanged** (no read): a listing gives the place, size and
   modification time. A sighting at the same place with the same size and
   time is the same file: its `image` and thumbnail stand. This is what
   makes a second listing of a card or folder instant.
2. **Size** (no read): a size no image and no sighting has is new
   content for sure.
3. **Partial hash** (128 KiB read): the first and last 64 KiB with the
   size, as `Image::partial` today. Same size and partial as a known image
   means "probably the same": enough to grey a cell or to reuse a
   thumbnail provisionally, never to count a backup or skip a copy.
4. **Full hash** (the whole file): the identity. Computed:
   - during an import, always (the copy is hashed while written, as
     today);
   - whenever the whole file is read anyway: a glance (`helios_load::glance`
     reads every byte already; BLAKE3 over a buffer in memory costs a few
     milliseconds against the read), a thumbnail's slow path, a backup;
   - in the background, by the upkeep queue (section 4), for sightings
     whose size and partial match an image (to confirm a duplicate), and
     for sightings on volumes with a role (library, archive, backup);
   - never, on its own, for a file on a card or a share that nothing has
     asked to read: listing a card costs a `stat` per file.

**Weak identities.** A sighting without a full hash is weak: it has no
image, no fields, and is never a backup copy or a reason to skip a copy.
It can still carry a thumbnail (keyed provisionally, section 7) and its
glance (time, camera, lens), so the import window shows it at once. A
volume with a weak identity (label and size) makes its sightings weak
too, as it does for backups today: "seen on EOS_DIGITAL" is said, never
counted.

**Renames and moves.** A rescan that finds a place gone and a new place
on the same volume with the same size and partial hash links them: the
new sighting takes the old one's `first_seen`, and the full hash is
checked in the background. Since marks hang on the image, not the
sighting, a library photo moved or renamed keeps everything with no
relink: the old sighting becomes `missing` until the full hash confirms,
then is dropped. Across volumes, a move is a sighting gone here and one
new there; the image is the same, so nothing is lost either way.
`Vault::repair` keeps its job for library images and becomes a user of
the same lookups.

**Same content on two volumes.** One image, two sightings. That is what a
backup copy is; it is also what a card already copied is, a download
folder holding an export, or two folders of the same shoot. The map
answers "where else is this?" with a query, and the Library shows it in
the photo's info ("also on BACKUP_A, last seen 2026-10-28").

**A file changed in place** (another tool wrote metadata into it): its
sighting's size or time changes and its partial hash is read again.

- **A file of a Library photo stays linked** (decided 2026-11-08). The
  re-check (section 7) reads its full hash and records it as the photo's
  content: the photo's id stays the hash it was imported with, since its
  marks, edits and history hang on it, and the document
  `content/<image id>` keeps the content its file holds now, every
  content it held before (hash, size, partial hash, from and until when)
  and the mark "changed outside Helios" with when it was found. No schema
  step: the documents table holds it (`helios_vault::changed`). The
  photo's thumbnail is made again under its own key, its larger entry
  dropped. The Library grid shows a "changed outside" badge, and the
  health check counts these photos ("N photos changed outside Helios")
  with an action, `library.changedOutside.acknowledge`, that clears the
  mark; the content and its history stay. A later re-check, and the
  map's verification of Library files, compare the file with the content
  recorded, not with the id.
- **Any other file**: its partial and image are cleared, and the new
  content is a new image, as before.

Left for later: thumbnails rendered from an edit (keyed by version and
chain) are not made again when the source changes, and the import
window's prefilter still knows the photo by the size and partial hash
it was imported with.

## 4. Recording

### What records

A sighting is written by every listing Helios makes of a place that is
not excluded (section 5):

| Listing | What it records |
| --- | --- |
| The import window's sources (`import_scan::Scan`) | every file it lists, then its glance (time, camera, preview) and, since the glance reads it whole, its full hash |
| File dialogs: egui-pop's `FileBrowser` through the source of #4844 | the image files of each folder shown: place, size, time; thumbnails as the dialog asks for them |
| Drops and pastes of files or folders | the files given, as the import window does for them |
| Volume scans (`helios-storage`'s `Scanner`, extended) | folders already listed, re-checked; then, by priority and per the setting, the rest of a volume (below) |

A listing writes its rows in batches (one transaction per folder, or per
few hundred files), off the interface thread, and never waits on the
vault to show what it found.

### Background scans

One upkeep queue, a job of kind `map` in the jobs centre, low priority,
in this order:

1. what is on screen (cells in view in the import window, a dialog, the
   Library): stat, then partial, then thumbnail;
2. library sightings: presence and changes (today's scan);
3. volumes plugged in just now (a card, a backup disk): their known
   folders, then the rest;
4. confirmations: full hashes for sightings whose partial matches an
   image;
5. the rest of known volumes, folder by folder, least recently listed
   first (the `folders` table), if the setting allows it.

It **pauses** while an import, an export, a backup or a render runs,
while the interface is busy (a frame budget missed), and **on battery**
(Linux `/sys/class/power_supply`, Windows `GetSystemPowerStatus`, macOS
`IOPSCopyPowerSourcesInfo`), resuming by itself. Reads are throttled on
removable and network volumes, and a volume going away stops its work at
once (no "missing" storm: its sightings go `offline`, as today).

### Size caps and forgetting

- Sightings of images not in the Library are capped (`system.map.limit`,
  default in open question 5); past it, the ones not seen for the longest
  are forgotten first. Library sightings are never capped.
- A sighting `missing` for longer than `system.map.keepMissing` (30 days)
  and not in the Library is dropped.
- **Forget a volume** (Storage page): drops its sightings not in the
  Library and their folders; the volume stays known unless removed.
- **Forget a folder** (from a dialog, the import window's sources, the
  Storage page): drops its sightings not in the Library and marks the
  folder `forgotten`, so scans skip it until it is listed again on
  purpose.

### Recording only on import

`system.map.record`: `everywhere` (listings and background scans of known
volumes), `listings` (what Helios is shown, no crawling), `import` (only
what is imported, as today; the import window still lists, but keeps
nothing it did not import). The default is open question 2.

## 5. The blacklist

A place is **excluded** when a default rule or `system.map.exclude`
matches it. An excluded place is never recorded and never thumbnailed
into the cache, even when the user opens it on purpose (a dialog shows
its files with plain icons, the import window shows them glanced but
keeps nothing). Its subfolders are excluded with it.

### Defaults

Everywhere:

- Helios's own data: the vault's folder (`.helios/`, its `thumbs`),
  `.helios-part/`, the data and cache directories, snapshots;
- folders tagged as caches by the Cache Directory Tagging spec
  (a `CACHEDIR.TAG` file: Cargo's `target/`, many build tools);
- version control and build trees: `.git`, `.hg`, `.svn`, `node_modules`,
  `__pycache__`, `.venv`;
- hidden folders (a name starting with `.`), the Synology and QNAP
  thumbnails (`@eaDir`, `.@__thumb`), trash folders (`.Trash-*`,
  `.Trashes`, `$RECYCLE.BIN`);
- other photo applications' caches and libraries, read only by their
  importers: Lightroom's `*.lrdata` (previews), Capture One's `Cache`
  folders in a catalogue or session, darktable's cache, Apple Photos'
  `*.photoslibrary`.

Linux: `/proc`, `/sys`, `/dev`, `/run`, `/tmp`, `/var`, `/usr`, `/etc`,
`/boot`, `/snap`, `/opt`, `~/.cache`, `~/.local/share` (Trash, Flatpak),
`~/.var`.

Windows: `C:\Windows`, `C:\Program Files`, `C:\Program Files (x86)`,
`C:\ProgramData`, `%APPDATA%`, `%LOCALAPPDATA%`, `System Volume
Information` and `$Recycle.Bin` on every volume.

macOS: `/System`, `/Library`, `/Applications`, `/private`, `/usr`,
`/opt`, `~/Library`, and on every volume `.Spotlight-V100`, `.fseventsd`,
`.DocumentRevisions-V100`, `.Trashes`.

### The setting

`system.map.exclude` (concern `Machine`: paths belong to a machine) is a
list of rules:

```json
[
  { "glob": "**/Private/**" },
  { "glob": "Archive/Clients/**", "volume": "uuid:5A3C-11F0" },
  { "glob": "!**/.photos/**" }
]
```

- a glob matches the path inside a volume (`volume` given) or the
  absolute path (`volume` absent: global); `**` crosses folders, matching
  is case-insensitive on Windows and macOS;
- a rule starting with `!` re-includes what a default excludes (a
  photographer's `~/.photos` folder);
- `system.map.defaults` (on by default) turns the default rules off as a
  whole, for whoever wants only their own.

The Storage page shows each volume's rules and offers "Exclude this
folder" from a folder's menu in dialogs and the import window's sources.
Adding a rule forgets what it now covers (as "forget a folder") and
removes its thumbnails from the cache.

## 6. Import as a flag

### What the import window does

The window does not change for the user; what it does underneath does:

- **Listing** a source records sightings. A source listed before shows
  at once: rows unchanged (level 1) keep their glance and thumbnail.
- **"In your Library"** is a lookup: the sighting's image has
  `in_library`. A sighting without a full hash yet is greyed on its
  partial match, as today, and confirmed during the job.
- **Duplicates** are lookups too: two sightings with one image in the
  source are one photo; an image in the Library is "already in your
  Library", with where its copies are.
- **Import** sets `in_library` (with `library_at` and `library_by` the
  job) on each image ticked, and makes the copies its placement and the
  strategy ask for.

### Copies: zero, one or several

| Placement | Copies made |
| --- | --- |
| **Add where they are** | zero: the flag only. Offered when the source is a lasting place (a fixed or archive disk); for a card it is refused unless the user switches it, as today |
| **Copy into the Library** | one: the library copy, by the verified copy (hash while copying, read back, then name) |
| Either, with a strategy triggered `OnImport` | plus each backup copy the strategy is short of, by the backup job (`helios_library::backup::after_import`), from the library copy, never from the card |

So "zero, one or several" is the placement plus `strategy::status`; no
new copy code is written. A copy made is a sighting of the image on its
volume (verified, `seen_by: import`), which is what `status` counts.
**The card default** stays as built: copy into the Library, then the
strategy's backups; "safe to format" is read off the map (every image on
the card in the Library, with its verified copies).

Images found already in the Library are not imported again; with
`duplicates: keep` a file of known content adds nothing but its sighting
(it is already one), so `keep` becomes "copy it anyway into the library
folder", and `ask` lists them as today.

### Undo

An import is one undo step, as today. Undo clears `in_library` on the
images the import flagged and puts back what it changed on images
already in the Library (values from importers, versions from sidecars),
as today. Sightings are facts, not changes: they stay, as do the copies
on disk, which a later import finds by their hash. **Remove from
Library** is the same as undo for chosen photos: the flag off, the files
untouched; whether their marks are kept is open question 7.

## 7. The shared thumbnail cache

One `ThumbCache` serves the Library, the filmstrip, Compare, the import
window and file dialogs.

**Keys.** `Key { image, version, chain }` stays. `image` is the content
identity: the full hash when known. A sighting with no full hash yet
uses a provisional key, `p:<size>:<partial>`, and its entry is renamed to
the full key when the hash comes (a cheap file rename in the cache). A
thumbnail rendered from an edit is keyed by its version and chain
(#4833 step 4), so edits and content changes never show a stale picture.

**Re-check.** The cache trusts a key only as long as the sighting it was
made from still matches. The upkeep queue (section 4), cells in view
first, re-checks in steps: size and time (a `stat`), then the partial
hash when they changed, then the full hash in the background. When the
content of a Library photo's file changed, the photo stays linked and
its thumbnail is made again under its own key (section 3, "A file
changed in place"). When another file's content changed, the sighting's
image is cleared, the old entry is left to the LRU (another sighting may
still use it) and a new thumbnail is made from the file.

**Virtualised grids (#4833 step 3).** The grid asks for the cells in
view; each cell is a row of a query (Library: images flagged; import
window and dialogs: sightings of a folder), a row gives a key, the key a
cache entry, the entry a texture under the texture budget. The re-check
queue is fed by the same "cells in view" hook as the loader. The import
grid's virtualisation, left for later in import-window.md, is this grid
over sightings.

## 8. Privacy

- **Local only.** The map lives in the vault file on this machine; no
  part of it is sent anywhere, and agents reach it only through the
  vault's commands, as the Library.
- **What is stored**: paths, sizes, times, hashes, capture time, camera
  and lens, and a 256 px thumbnail per image shown. Never GPS, faces or
  the pictures themselves for images not in the Library.
- **Wiping it**: "Forget everything not in the Library" (Storage page and
  a command) drops every sighting, image row and cache entry of images
  not in the Library, then compacts the vault; forget a volume or folder,
  `system.map.exclude`, and `system.map.record: import` keep it from
  growing again. A catalogue backup includes the map; its wipe applies
  to backups made after it.

## 9. Slices

Each slice merges into `next` on its own, with contract tests on both
stores.

1. **Vault schema and migration.** Schema 5 (section 2), the `Store`
   calls for sightings, folders and the flag, `MemoryStore` alike;
   `in_library = 1` added to every Library read, with contract tests
   that an image off the Library never shows in a query, a cursor, a
   stack, a backup count or "safe to format". No behaviour change.
2. **Recording.** The import window's scan, drops, and the `FileBrowser`
   source write sightings (glance and full hash included); the default
   blacklist and `system.map.exclude`; `system.map.record`; forget a
   folder or volume; the wipe.
3. **Import as a flag.** `import` sets the flag and copies by placement
   and strategy; "in your Library" and duplicates read the map; undo
   clears the flag; safe to format from the map.
4. **The thumbnail re-check.** Provisional keys and their renaming, the
   import window and dialogs on the shared cache, the re-check queue;
   this is what #4833 step 3 builds its grids on.
5. **Scans and upkeep.** The background queue with its priorities, pauses
   under load and on battery, renames and moves, caps and pruning, the
   Storage page's per-volume view of the map.

## 10. Open questions

Options in key order; the recommendation is marked.

1. **Where the map lives.**
   - A. In the library's vault, beside the images (recommended: one file
     to back up, one transaction for "flag on" and its sighting, and the
     owner's "Helios's database").
   - B. In a machine database shared by every library.
   - C. Both: library sightings in the vault, the rest per machine.
2. **What records by default** (`system.map.record`).
   - A. Everywhere: listings, and background scans of every known volume.
   - B. Listings, plus background scans of volumes with a role (library,
     archive, backup, ingest) (recommended: what the user shows Helios and
     what they told Helios is theirs; a crawl of a whole system disk is
     opt-in per volume).
   - C. Listings only.
3. **A blacklisted place imported on purpose.**
   - A. Refuse, with the rule that excludes it.
   - B. Import by copy only: the copy is recorded where it lands, nothing
     at the source (recommended).
   - C. Ask whether to lift the rule for that folder.
4. **The thumbnail cache's scope.**
   - A. Per library, beside the vault, as today (recommended: it follows
     the map's place).
   - B. Per machine, in the user's cache folder, shared by libraries.
   - C. Per library for library images, per machine for the rest.
5. **The cap on sightings not in the Library** (`system.map.limit`).
   - A. No cap; the Storage page shows the map's size.
   - B. One million sightings, the least recently seen forgotten first
     (recommended: about 300 MB of vault and thumbnails at most).
   - C. A cap per volume (100,000).
6. **A file changed in place** (metadata written by another tool).
   Decided 2026-11-08: a Library photo stays linked, its new content
   recorded and the photo marked until acknowledged (section 3); other
   files as A.
   - A. A new image; the old one keeps its marks, its sighting gone
     (recommended for now: what library.md says today, and safe).
   - B. The new content succeeds the old: marks and versions follow.
   - C. Ask, in the Duplicates panel.
7. **Marks of a photo removed from the Library.**
   - A. Kept on the image, flag off: importing it again brings them back
     (recommended; the wipe removes them).
   - B. Dropped with the flag, as Lightroom's Remove.
   - C. Ask each time.

## Prior art: seen but not imported, and duplicates

- **Lightroom Classic** knows only what was imported. Its catalogue
  references files by path; a file moved or a volume renamed makes photos
  "missing", found by "Find All Missing Photos" and relinked by folder
  ([Adobe: locate missing photos](https://helpx.adobe.com/lightroom-classic/desktop/manage-catalogs-and-files/locate-missing-photos.html)).
  "Don't Import Suspected Duplicates" compares the original file name,
  the EXIF capture time and the file size, not the content: renamed
  files slip through, reused names are blocked
  ([Adobe community: what makes a suspected duplicate](https://community.adobe.com/questions-675/what-determines-if-image-is-suspected-duplicate-in-import-dialog-937426),
  [tethered shots not detected](https://community.adobe.com/questions-675/tethered-vs-import-doesn-t-recognise-duplicates-938652)).
  Helios's content hash avoids both failures; the map adds memory of the
  files seen.
- **digiKam** has no import step for its collections: every image in a
  collection's folders is in its database, found by "Scan for New Items"
  (also at startup); duplicates are a maintenance tool over image
  fingerprints
  ([digiKam: maintenance tools](https://docs.digikam.org/en/maintenance_tools.html),
  [collections](https://docs.digikam.org/en/setup_application/collections_settings.html)).
  This is closest to "record everything seen", but without a flag: seen
  is imported, so pointing digiKam at a big disk fills the library.
- **darktable** knows images by film roll (a folder) and file name, with
  no content hash; a moved folder must be told, missing files show a
  placeholder and are removed by hand
  ([manual: collections](https://docs.darktable.org/usermanual/4.6/en/module-reference/utility-modules/shared/collections/),
  [pixls.us: remove missing images](https://discuss.pixls.us/t/remove-missing-images-darktable/5308)).
  The same file in two folders is two images.
- **Apple Photos** sorts a device's files into "Already Imported" and
  "New" and imports only the new, unless one is picked on purpose; a
  later "Duplicates" album finds near-identical photos already imported
  and merges them, keeping the best version
  ([MacMost: the import feature](https://macmost.com/the-surprisingly-robust-photos-import-feature.html),
  [MacMost: finding duplicates](https://macmost.com/finding-duplicate-photos-in-the-photos-app.html),
  [Apple Community: merging duplicates](https://discussions.apple.com/thread/256021424)).

Lessons: Apple's "already imported" is the behaviour to match, made
instant by the map; digiKam shows the cost of "seen means imported",
which the flag avoids; Lightroom and darktable show that path-based
identity is what makes files go missing, which content identity and
volume-relative places already fix.
