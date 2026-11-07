# Importing darktable's library

Status: study (2026-11-07), not implemented.

This study asks what it takes to read darktable's catalogue (`library.db`,
with `data.db` beside it) into the Helios library, as an importer plugin
over the importer protocol (File > Import from →, `importer.scan`). It
quotes darktable's source; paths are relative to darktable's source tree
(the `darktable-org/darktable` repository), and versions are checked
against its release tags up to `release-5.6.2`.

In short:

- Protocol v1 is enough for the bulk of what people care about: which
  files, their stars, rejects, colour labels, tags, title and description.
  The edits come in already, through the photos' `.xmp` sidecars, which
  Helios's ordinary import keeps as versions.
- Edits of photos without sidecars (darktable set to never write them,
  or "after edit" for duplicates never edited since), film rolls as views,
  and geotags set in darktable need protocol additions. The edits are the
  one worth doing first: an optional `darktable` block per image carrying
  the database rows faithfully, behind `importer` interface version 2,
  turned into versions by Helios (never by the plugin).
- The plugin is best written in Python with the standard library alone
  (`sqlite3`), as the in-tree `exif-exiftool` example.

## 1. Where the catalogue is

darktable keeps two SQLite files in its configuration folder
(`dt_loc_init_user_config_dir`, `src/common/file_location.c`: GLib's
`g_get_user_config_dir()` followed by `darktable`):

| System | Default folder |
|--------|----------------|
| Linux (and other XDG systems) | `~/.config/darktable` |
| Windows | `%LOCALAPPDATA%\darktable` (GLib's user config folder there) |
| macOS | `~/.config/darktable` by GLib's XDG default; not checked on a Mac |

- `library.db`: the images, their film rolls, edits, labels, tags
  attached, metadata. Its name and place can change: the `database`
  configuration key (a name relative to the folder, or an absolute path),
  or `--library <file>` on the command line, or `:memory:` (a throw-away
  library: nothing to import). `--configdir` moves the whole folder.
- `data.db`: the tag names, styles, presets, map locations and (since
  data schema 13) the metadata keys. It is **always** `data.db` in the
  configuration folder (`dt_database_init`, `src/common/database.c`), even
  when `--library` points elsewhere. Without it, `tagged_images` holds tag
  ids with no names.
- Backups and snapshots sit beside them: `library.db-pre-<version>` before
  a schema upgrade, `library.db-snp-<date>` snapshots. They are libraries
  too; a user might pick one on purpose, the plugin never picks one by
  itself.

So the source an importer asks for is the **configuration folder**
(`source = "folder"`, `holds = ["library.db"]`), which gives both files. A
library elsewhere, under another name, is out of v1 (see section 9).

## 2. The schema that matters

Columns are quoted from `_create_library_schema` and `_create_data_schema`
(`src/common/database.c`, "the state from 27 October 2024") and the
upgrade steps after them.

### library.db

| Table | Columns the importer reads | Notes |
|-------|---------------------------|-------|
| `db_info` | `key VARCHAR PRIMARY KEY, value VARCHAR` | `key = 'version'`: the schema version (section 7) |
| `film_rolls` | `id INTEGER PRIMARY KEY, access_timestamp INTEGER, folder VARCHAR(1024) NOT NULL` | one row per imported folder, `folder` absolute |
| `images` | `id`, `group_id`, `film_id`, `filename`, `version`, `max_version`, `flags`, `history_end`, `longitude`, `latitude`, `altitude`, `datetime_taken`, `position`, `import_timestamp`, `change_timestamp`, `export_timestamp`, `print_timestamp` | one row per darktable duplicate (section 4) |
| `color_labels` | `imgid INTEGER, color INTEGER` | unique on `(imgid, color)` |
| `tagged_images` | `imgid INTEGER, tagid INTEGER, position INTEGER` | `tagid` is `data.tags.id` |
| `meta_data` | `id INTEGER, key INTEGER, value VARCHAR` | `id` is the image id |
| `history` | `imgid, num, module, operation VARCHAR(256), op_params BLOB, enabled, blendop_params BLOB, blendop_version, multi_priority, multi_name VARCHAR(256), multi_name_hand_edited` | `module` is the module's params version |
| `masks_history` | `imgid, num, formid, form, name VARCHAR(256), version, points BLOB, points_count, source BLOB` | the drawn forms, per history item |
| `module_order` | `imgid INTEGER PRIMARY KEY, version INTEGER, iop_list VARCHAR` | pipe order |
| `history_hash` | `imgid INTEGER PRIMARY KEY, basic_hash BLOB, auto_hash BLOB, current_hash BLOB, mipmap_hash BLOB` | whether the edit was touched by hand |

The full path of an image is `film_rolls.folder` joined with
`images.filename`, through `images.film_id`. There is no file hash in the
library (identity is folder plus file name), so Helios hashes every file
it imports, as always.

Internal, never imported: `selected_images` (the current selection),
`harmony_guide` and `overlay` (darkroom display aids), the camera and EXIF
lookup tables (`makers`, `models`, `lens`, `cameras`, `whitebalance`,
`flash`, `exposure_program`, `metering_mode`) and the EXIF columns of
`images` (`exposure`, `aperture`, `iso`, `focal_length`, `maker_id`...),
which are a cache of what the file says; `thumb_timestamp`,
`thumb_maxmip`, `history_hash.mipmap_hash` (thumbnail cache state); the
`v_images` view.

### data.db

| Table | Columns | Notes |
|-------|---------|-------|
| `db_info` | as in `library.db` | the data schema version |
| `tags` | `id INTEGER PRIMARY KEY, name VARCHAR, synonyms VARCHAR, flags INTEGER` | `name` is the path, `|` between levels |
| `meta_data` (data schema 13, darktable 5.2) | `key INTEGER PRIMARY KEY, tagname VARCHAR, name VARCHAR, internal INTEGER, visible INTEGER, private INTEGER, display_order INTEGER` | what each `library.meta_data.key` means |
| `styles`, `style_items` | a style's history items | not imported (section 6) |
| `presets` | module presets, auto-apply rules | not imported |
| `locations` | map locations, keyed by a tag id | not imported |

Tags whose name starts with `darktable|` are darktable's own
(`darktable|format|cr2`, `darktable|changed`, `darktable|exported`...;
`src/common/tags.c` filters them with `name NOT LIKE 'darktable|%'`):
the importer skips them. `tags.flags` holds `DT_TF_CATEGORY` (1: a path
level that is not a keyword), `DT_TF_PRIVATE` (2) and `DT_TF_ORDER_SET`
(4) (`src/common/tags.h`).

## 3. Marks: stars, rejects, labels, tags, text

**Stars and rejects** live in `images.flags` (`src/common/image.h`,
`dt_image_flags_t`):

- bits 0 to 2 (`DT_VIEW_RATINGS_MASK = 0x7`, `src/common/ratings.h`): the
  stars, 0 to 5 (`DT_VIEW_DESERT` to `DT_VIEW_STAR_5`, `src/views/view.h`);
- bit 3 (`DT_IMAGE_REJECTED = 8`): rejected. The stars are kept beside it
  (`src/common/ratings.c` sets and clears the bit without touching them),
  exactly as Helios's separate `rejected` field;
- the other bits are file kinds and states (`DT_IMAGE_LDR = 32`,
  `DT_IMAGE_RAW = 64`, `DT_IMAGE_HDR = 128`, `DT_IMAGE_REMOVE = 256`,
  `DT_IMAGE_AUTO_PRESETS_APPLIED = 512`, `DT_IMAGE_LOCAL_COPY = 2048`,
  monochrome bits...): not marks.

Before library schema 29 (darktable 3.x), a reject was the rating value 6
(`DT_VIEW_REJECT`) in the low bits; step 28 → 29 moved it to bit 3
(`UPDATE main.images SET flags = (flags | 8) & ~7 WHERE (flags & 7) = 6`).
Every 4.x and 5.x library is past that step, but the plugin reads
`(flags & 7) == 6` as a reject too, at no cost. In XMP, darktable writes
`xmp:Rating` -1 for a reject (`dt_image_get_xmp_rating_from_flags`,
`src/common/image.c`), which loses the stars; the database does not.

| darktable | Helios field | Operation |
|-----------|--------------|-----------|
| `flags & 7` (0 to 5) | `rating` | set if absent |
| `flags & 8` | `rejected: true` | set if absent |
| `color_labels.color` 0, 1, 2, 3, 4 (`DT_COLORLABELS_RED`, `_YELLOW`, `_GREEN`, `_BLUE`, `_PURPLE`, `src/common/colorlabels.h`) | `labels`: `red`, `yellow`, `green`, `blue`, `purple` | added |
| `tagged_images` → `data.tags.name` | `tags` | added |
| `meta_data` key `Xmp.dc.title` | `title` | set if absent |
| `meta_data` key `Xmp.dc.description` | `description` | set if absent |

A rating of 0 is sent (it is darktable's explicit "no star"); Helios's
`rating` accepts 0 to 5. darktable has no "picked" flag: `picked` is left
alone.

**Tags.** A darktable tag is a path, `places|France|Lyon`. Helios's `tags`
field is a flat set of trimmed names. Two choices:

- the **leaf** (`Lyon`): readable, but `places|Lyon` and
  `people|Lyon` collapse into one tag and the hierarchy is lost;
- the **whole path** as one tag (`places|France|Lyon`): nothing is lost,
  the hierarchy can be rebuilt later, and it is what darktable writes to
  `Xmp.lr.hierarchicalSubject`; darktable's `Xmp.dc.subject` instead gets
  every level as a separate keyword (`dt_tag_get_list`), or the leaf only
  with `omit_tag_hierarchy`.

Recommended: **the whole path**, `|` kept as the separator, so a later
hierarchical tag view in Helios reads the same strings. Tags flagged
`DT_TF_CATEGORY` are attached like the others in darktable only when the
user attached them; they are sent as they are. Tag synonyms are not sent.

**Title and description.** `library.meta_data.key` is a number:

- up to data schema 12 (darktable 4.0 to 5.0), the enum
  `dt_metadata_t` of `src/common/metadata.h`: 0 `creator`, 1 `publisher`,
  2 `title`, 3 `description`, 4 `rights`, 5 `notes`
  (`Xmp.acdsee.notes`), 6 `version name` (`Xmp.darktable.version_name`),
  7 `image id` (internal), and from 4.9/5.0 8 `preserved filename`
  (`Xmp.xmpMM.PreservedFileName`);
- from data schema 13 (darktable 5.2), the rows of `data.meta_data`, keyed
  the same for those nine (the upgrade step inserts them with `key = i`),
  plus any key the user added; `tagname` says what it is.

The plugin reads `data.meta_data` when it exists and looks keys up by
`tagname` (`Xmp.dc.title`, `Xmp.dc.description`), else uses 2 and 3.
Creator, publisher, rights and notes have no Helios field yet: they are
dropped in v1 and listed once in the warnings (a count, not one line per
image). The version name belongs to the version (section 4).

## 4. Duplicates, groups and edits

**Duplicates.** Each `images` row is one darktable duplicate. Rows of the
same file share `film_id` and `filename` and differ by `version` (0 for
the original, `max_version` the highest given). Their sidecars are
`NAME.EXT.xmp` for version 0 and `NAME_NN.EXT.xmp` for the others
(`dt_image_path_append_version_no_db`, `src/common/image.c`), which is
what `helios_xmp::files` matches. Deleting version 0 in darktable leaves
the others with their numbers.

In Helios, a darktable duplicate is a version of the one image (ROADMAP
2.3, 2026-10-12 and 2026-10-29: a first-edit head per duplicate, never
merged, only the base giving the rating). So:

- the plugin sends **one entry per file**, not per row;
- the file's marks come from its **base row**, the lowest `version`
  (normally 0). Labels and tags of the other rows are **added** as well
  (adding loses nothing); a duplicate whose stars or reject differ from the
  base's is reported in the warnings, since Helios has one rating per
  image.

**Groups.** `images.group_id` is the id of the group's leader (an image
alone is its own leader); `src/common/grouping.c`. Duplicates are grouped
with their original by default. Helios's **stacks** are the nearest thing,
but a darktable group of duplicates is one Helios image already; only a
group spanning several files (a RAW and its JPEG, a burst grouped by hand)
would become a stack. Not in v1: stacks are made by accepted decisions,
and an importer could later propose one (`stacks.suggest`-like) rather
than create it.

**The edit of a duplicate** is its `history` rows (`imgid`, ordered by
`num`), the applied part being the first `images.history_end` rows (the
rest were undone and stay for redo), its `masks_history` rows, and its
`module_order` row (`version`: darktable's built-in pipe order number;
`iop_list`: a custom order as text, `NULL` unless the order is custom or
has several instances, `dt_ioppr_write_iop_order` in
`src/common/iop_order.c`). That is, item for item, what darktable writes
to a sidecar (`_exif_xmp_read_data` and `_set_xmp_dt_history`,
`src/common/exif.cc`): `history.module` becomes `darktable:modversion`,
`op_params` and `blendop_params` are encoded with `dt_exif_xmp_encode`
(hex, or `gz` plus base64 of zlib data when long), `history_end`,
`iop_order_version` and `iop_order_list` go on the description.

`history_hash` tells a hand edit from darktable's defaults:
`current_hash = basic_hash` is an untouched image, `= auto_hash` only
auto-applied presets, anything else "altered"
(`dt_history_hash_get_status`, `src/common/history.c`). It can spare
Helios a version for an image nobody edited; its blobs are darktable's own
digest, not something to recompute.

**Two ways in.**

1. **Sidecars exist** (darktable's default, `write_sidecar_files = on
   import`, `data/darktableconfig.xml.in`). Helios's ordinary import
   already keeps each `.xmp` as a version
   (`helios_library::darktable::keep_file_of`), report and all. Nothing
   new is needed: v1 covers it.
2. **No sidecar** (`never`; or `after edit`, which writes a sidecar only
   after the first hand edit and updates it after each editing session, so
   an untouched duplicate has none and a recent one may lag). The edit is
   only in the database.

For the second case, the question is whether Helios can take rows instead
of a file. It can, with little work: `helios_xmp::import_with(sidecar:
&Sidecar, layouts, frame)` takes the in-memory `Sidecar`
(`crates/helios-xmp/src/sidecar.rs`), not XML. Its fields map one to one:

| `Sidecar` / `HistoryItem` | From the database |
|---------------------------|-------------------|
| `history_end` | `images.history_end` |
| `iop_order_version`, `iop_order_list` | `module_order.version`, `module_order.iop_list` |
| `version_name` | `meta_data` key `Xmp.darktable.version_name` (6) |
| `rating` | from `flags` for the base, `None` for the others |
| `xmp_version` | `Some(5)` (`DT_XMP_EXIF_VERSION`): the rows are already in today's form |
| item `num`, `operation`, `enabled`, `modversion`, `multi_priority`, `multi_name` | `history.num`, `operation`, `enabled`, `module`, `multi_priority`, `multi_name` |
| item `params`, `blendop_params`, `blendop_version` | `op_params`, `blendop_params` (raw bytes, no decoding), `blendop_version` |
| item `iop_order` | `None` (only sidecars older than `xmp_version` 4 carry it) |
| item `attributes` | built in the sidecar's spelling (`darktable:operation`, `darktable:params` hex...), so the version can be written back as a sidecar |
| `masks` (`drawn::Form`) | `masks_history`: `num`, `formid` → `id`, `form` → `kind`, `name`, `version`, `points_count` → `count`, `points` |

What is missing on the Helios side: a constructor (`Sidecar::from_rows`
or the like) that fills these and skips `add_implicit_highlights`
(darktable already applied that rule when it read the old sidecar into its
database); keeping the rows themselves under the version's `darktable`
part in place of `darktable.sidecar` (the whole XML today); and
`multi_name_hand_edited` and `masks_history.source`, which have no field
in `HistoryItem` and `Form` and would ride in `attributes`.

This must happen **in Helios**, not in the plugin: the plugin never writes
the library, and building versions needs `helios_xmp`'s layouts and
reports.

**Recommended protocol addition** (`importer` interface version 2, an
optional key per image, ignored by a v1 Helios):

```json
{
  "path": "/photos/2024/IMG_0001.CR2",
  "rating": 3,
  "darktable": {
    "schema": {"library": 57, "data": 13},
    "versions": [
      {
        "version": 0,
        "name": "",
        "history_end": 12,
        "iop_order": {"version": 5, "list": null},
        "altered": true,
        "history": [
          {"num": 0, "operation": "rawprepare", "module": 2, "enabled": true,
           "params": "<base64>", "blendop_version": 14, "blendop_params": "<base64>",
           "multi_priority": 0, "multi_name": "", "multi_name_hand_edited": false}
        ],
        "masks_history": [
          {"num": 7, "formid": 1700000000, "form": 2, "name": "ellipse #1",
           "version": 6, "points": "<base64>", "points_count": 1, "source": "<base64>"}
        ]
      }
    ]
  }
}
```

Blobs in standard base64, never re-encoded or decoded by the plugin.
Helios then, for each version number, uses the sidecar file when its
ordinary import found one for that number, else these rows: never both, so
a duplicate does not get two heads for one edit. The other option, the
plugin writing sidecar-equivalent XML and pointing Helios at it, would put
a second copy of darktable's XMP writer in the plugin (encodings, mask
fields, the `gz` form) and need a place to write; rejected.

## 5. Film rolls as views

ROADMAP 2.3: folders become views, nothing is moved; 2.4: a film roll is
"just one imported folder". The vault has no views or collections yet, so
v1 sends none. With them, an optional `collections` list per image
(`[{"kind": "film_roll", "name": "/photos/2024/Lyon"}]`), or one top-level
list of collections each naming its images, behind `importer` version 2,
lets Helios rebuild darktable's film rolls (and later its tag-based
collections) as views. Helios already records each file's folder as its
location, so a "by folder" view can also be derived without the plugin.

## 6. What maps, what does not

| darktable | Helios target | Protocol | Why |
|-----------|---------------|----------|-----|
| `film_rolls.folder` + `images.filename` | the image's file, referenced or copied | v1 (`path`) | |
| stars (`flags & 7`) | `rating` | v1 | |
| reject (`flags & 8`) | `rejected` | v1 | |
| colour labels | `labels` | v1 | |
| tags (not `darktable|`) | `tags`, full path | v1 | |
| title, description | `title`, `description` | v1 | |
| creator, publisher, rights, notes, user keys | none yet | dropped (counted) | no field; a later vault field |
| preserved filename, image id | none | dropped | darktable's own bookkeeping |
| duplicates with sidecars | versions (heads) | v1 (the ordinary import) | already implemented |
| duplicates without sidecars | versions (heads) | v2 (`darktable.versions`) | rows → `Sidecar` in Helios |
| version name | the version's label | v2 (from sidecars: today) | |
| `history_hash` | skip untouched duplicates | v2 (`altered`) | |
| groups (`group_id`) | stacks | not yet | a proposal, not an import |
| film rolls | views | v2 (`collections`) | no views in the vault yet |
| geotag (`longitude`, `latitude`, `altitude`) | none yet | later | set by darktable's map, may differ from the file's GPS; needs a vault field |
| `datetime_taken` | none | dropped | Helios reads the capture time from the file; a date corrected in darktable is lost |
| EXIF columns, makers, lenses | none | dropped | a cache of the file |
| `images.position` (custom sort) | none | dropped | no manual order in Helios's grid yet |
| `import_timestamp`, `change_timestamp`, `export_timestamp`, `print_timestamp` | none | dropped | could feed an audit or "last edited" later |
| local copies (`DT_IMAGE_LOCAL_COPY`, `<cache>/img-<md5>.<ext>`) | none | dropped | the original path is sent; the cache copy is darktable's |
| `selected_images` | none | dropped | session state |
| styles, presets | none | not imported | they are darktable params blobs per module; a later "import styles" job, through the same `helios_xmp` decoding |
| map locations, tag synonyms | none | dropped | |

## 7. Schema versions

darktable stores its schema version in its own table, not in SQLite's
`PRAGMA user_version`: `SELECT value FROM db_info WHERE key = 'version'`,
in each file (`dt_database_init`, `src/common/database.c`). Current
constants: `CURRENT_DATABASE_VERSION_LIBRARY 57`,
`CURRENT_DATABASE_VERSION_DATA 13`. Per release tag:

| darktable | library | data |
|-----------|---------|------|
| 3.8 (for reference) | 34 | 9 |
| 4.0.x | 36 | 9 |
| 4.2.x | 37 | 9 |
| 4.4.x | 38 | 10 |
| 4.6.x | 47 | 10 |
| 4.8.x | 53 | 10 |
| 5.0.x | 56 | 10 |
| 5.2.x, 5.4.x, 5.6.x | 57 | 13 |

The odd tags (`release-4.1.0`, `-4.3.0`... `-5.7.0`) mark the start of a
development cycle and carry the schema of the release before them; a
development build in between may sit on a number no release has (51 and
52 were such numbers, see step `51 || 52`).

The upgrade steps that matter to the importer, between 36 and 57:

| Step | darktable | Change | Importer |
|------|-----------|--------|----------|
| 37 → 38 | 4.4 | `history.multi_name_hand_edited` added | read it when present |
| 38 → 39, 41 | 4.6 | `makers`, `models`, `lens`, `cameras` tables; `images` rebuilt without its text `maker`, `model`, `lens` and old columns (`sha1sum`, `histogram`...) | not read |
| 40 → 44 | 4.6 | `history_hash` gains then loses thumbnail columns | read `basic_hash`, `auto_hash`, `current_hash` only |
| 44 → 45 | 4.6 | `images.thumb_timestamp`, `thumb_maxmip` | not read |
| 46 → 48 | 4.6 | `harmony_guide`, `overlay` | not read |
| 48 → 53 | 4.8 | `selected_images` gains `num` | not read |
| 50 → 51 | 5.0 | `whitebalance`, `flash`... lookups | not read |
| 53 → 54 | 5.0 | `meta_data` rebuilt with a foreign key, orphans dropped | none |
| 56 → 57 | 5.2 | `images.flash_tagvalue` | not read |
| data 9 → 10 | 4.4 | `multi_name_hand_edited` in styles and presets | not read |
| data 12 → 13 | 5.2 | `data.meta_data` key definitions | use when present |

Everything the importer reads in `film_rolls`, `images`, `color_labels`,
`tagged_images`, `meta_data`, `history`, `masks_history`, `module_order`
and `data.tags` has the same columns from 36 to 57, except the two marked
"when present". The plugin therefore accepts library 36 to 57 and data 9
to 13, checks the columns it reads (`PRAGMA table_info`) rather than
trusting the number alone, and refuses anything else with a message that
names the version found and the range it reads ("this library was written
by a darktable newer than this importer knows (library schema 58)").
Before 36 (darktable 3.x), the user is asked to open the library once in
darktable 4.0 or later, which upgrades it.

## 8. Reading without harm

The importer must never change a user's catalogue.

- **Open read-only**: `file:<path>?mode=ro` with `uri=True`. Never
  `immutable=1` on the live file (SQLite would then ignore locks and any
  write darktable makes while we read); only on a private copy.
- **darktable running**: it creates `library.db.lock` and `data.db.lock`
  (`<database file>.lock`, `_lock_databases`, `src/common/database.c`)
  holding its process id, and removes them on exit. A lock whose process
  is gone is stale (a crash); darktable itself then takes it over. The
  plugin reads the pid and, when that process is alive, does not read the
  live files: it takes a **snapshot** first, through SQLite's backup API
  (Python's `Connection.backup`) from a read-only connection into memory,
  or into a private temporary file for a large library, then reads the
  snapshot. The backup is consistent and holds the read lock only for
  that copy, which a long scan would otherwise hold against darktable's
  writes. The answer's warnings say darktable was running and that changes
  made after the snapshot are not in the import. A stricter variant,
  refusing with "close darktable first", is the fallback where the backup
  fails.
- **Journals**: darktable sets `PRAGMA journal_mode = MEMORY` and
  `synchronous = OFF` (`dt_database_init`), so there is normally no `-wal`
  or `-shm` file. If one exists (a library opened by another tool in WAL
  mode), the backup API reads through it; the plugin never deletes, moves
  or checkpoints it.
- **No write capability needed**: the in-memory snapshot needs no scratch
  folder. The temporary-file path does; until the broker offers a private
  scratch folder, it stays a disclosed ambient need or the in-memory path
  alone is used (section 10).

## 9. The plugin

**Language: Python, standard library alone.** `sqlite3` reads both files,
`json` speaks the protocol, `base64` encodes blobs for v2; nothing to
build, as the `exif-exiftool` example. Rust would mean a workspace member
and a bundled SQLite for a job that is a few queries. Python's `sqlite3`
is built against the system's SQLite, which reads files from any SQLite 3.

**Manifest sketch:**

```toml
id = "io.github.quazardous.helios.darktable-library"
name = "darktable library"
title = "darktable library"
description = "Imports the photos of a darktable library with their stars, labels, tags and texts"
version = "0.1.0"
license = "GPL-3.0-or-later"
command = ["python3", "darktable_library.py"]
services = ["importer"]
activation = ["onImporter:darktable-library"]

[compat]
helios = ">=0.58.0, <0.59.0"   # as the examples: the releases it was tested with
interfaces = { protocol = 1, importer = 1 }

[[importers]]
id = "darktable-library"
label = "darktable library"
source = "folder"
patterns = []
holds = ["library.db"]
anywhere = true

[[capabilities]]
name = "source.read"
reason = "Reads library.db and data.db in the darktable folder you pick"
```

A library elsewhere or under another name (`--library`) needs a `file`
source, but `data.db` would then lie outside what `source.read` hands the
plugin: a later protocol addition (companion files, or the folder given
as well as the file). Until then the user picks a folder holding both.

**Scan, step by step** (`importer.scan`):

1. Check `kind` is `folder` and `library.db` is there; else error -32001.
   Note whether `data.db` is beside it (tags and data schema need it; its
   absence is a warning: tags are skipped).
2. Look for `library.db.lock`; when its process is alive, snapshot as in
   section 8. Open (the snapshot or the file) read-only and `ATTACH` the
   data file read-only as `data`.
3. Read both `db_info` versions; refuse unknown ones (-32001 with the
   message of section 7); check the columns read.
4. Count the files (`SELECT COUNT(DISTINCT film_id || '/' || filename)`)
   for the progress total; one `$/progress` every few hundred files.
5. Walk `images JOIN film_rolls` ordered by `film_id, filename, version`,
   in batches (`LIMIT`/`OFFSET` or by `id`), gathering per file the rows
   of all its versions, their labels (`color_labels`), tags
   (`tagged_images JOIN data.tags`, skipping `darktable|%`) and texts
   (`meta_data`).
6. Build each entry: `path` (folder joined with file name, by the host's
   rules), base row's `rating` and `rejected`, `labels` and `tags` of all
   rows, `title` and `description` when not empty.
7. Check cancellation between batches; on `$/cancelRequest`, answer
   -32800 and drop the snapshot.
8. Answer `{"images": [...], "warnings": [...]}`.

The plugin does not test files: Helios checks every path and reports the
missing ones. It does say, in one warning per film roll, when a roll's
whole folder is absent (a drive not mounted), which is more useful than a
thousand "missing" lines.

**Warnings** (short, counted): darktable running, snapshot taken; data
file missing, tags skipped; film rolls whose folder is absent; duplicates
whose stars differ from the base's; metadata kept nowhere (creator,
rights...: a count); `:memory:` or empty libraries.

**Tests.** A test builds small libraries from the `CREATE TABLE`
statements of each supported schema (36, 38, 47, 53, 56, 57; data 9, 10,
13), copied into the test as text from darktable's
`src/common/database.c` at each release tag, fills a few rows (two film
rolls, a file with two duplicates, a reject with stars, labels, nested
tags and a `darktable|` tag, a title, a missing folder), and checks the
answer. Others: an unknown version refused; a lock file with a live pid
(the test's own) snapshots, one with a dead pid does not; cancel between
batches; non-ASCII names. No real user's library is ever committed: it
holds paths, names and places.

## 10. Risks

- **Schema drift.** darktable changes its schema about once a release.
  The column checks and a refusal with a clear message contain it; each
  new release needs a look at its upgrade steps and a test schema.
- **Huge libraries.** 100,000 images or more in one JSON answer is tens
  of megabytes with tags, and v2 histories (blobs) multiply it. v1 can
  live with one answer; v2 should page: `importer.scan` answering a first
  batch and a `next` token, or `$/importer.images` notifications streamed
  under the progress token. To decide with v2.
- **Paths.** `film_rolls.folder` is absolute and native (`C:\...` on
  Windows). SQLite text is UTF-8; how darktable stores a Linux file name
  that is not valid UTF-8 was not checked: such a path may not resolve and
  is reported by Helios as missing.
- **Offline media.** Helios identifies an image by its content, so a file
  on an unplugged drive cannot be imported at all. It is reported; the
  import can be run again later, since every operation is idempotent
  (set-if-absent, added labels and tags, a sidecar already kept adds
  nothing).
- **darktable running.** Handled by the snapshot; the user is told the
  import reflects the library at that moment.
- **Duplicates without sidecars.** In v1 their edits are not imported at
  all: the warning says how many duplicates had no sidecar, so the user
  knows (or can have darktable write them, the "write sidecar files" button
  of the lighttable's history stack module, then import again). v2 closes it.
- **Hierarchy loss.** Avoided by keeping the full path as the tag; a flat
  leaf view is a Helios display choice, not an import loss.
- **The same marks from the database and from the XMP.** Both carry the
  rating and reject (the base sidecar through `keep_sidecar_of`, the
  database through the plugin's answer), both with set-if-absent, so
  **the first applied wins**. Recommended order: the **plugin's database
  marks first**, then the sidecars. The database is darktable's truth:
  with `after edit` the sidecar lags until the end of an editing session,
  it can be stale when sidecars were turned off for a while, and an XMP
  reject (-1) has lost the stars the database still holds. Labels and
  tags are added from the database only (Helios's sidecar path reads the
  rating alone), so their order does not matter. The importer job does
  so: it applies the answer's values to each image before keeping its
  sidecars (see docs/extensions.md, "Importers").
- **One edit, two carriers.** With v2, a duplicate kept once from its
  rows and later from a sidecar written meanwhile would become two heads,
  since the version's `darktable` part differs (rows against XML).
  Helios should recognise the same edit by its decoded history, not its
  carrier. Open.
- **Darktable's own tags as signals.** `darktable|changed` and
  `darktable|exported` are skipped as tags; they might later feed a "was
  edited in darktable" filter. Not decided.

## Open points

- `importer` interface version 2: `darktable.versions`, `collections`,
  paging. Which come first (recommended: `darktable.versions`, then
  paging, then `collections` once the vault has views).
- A `file` source with companion files, for libraries outside the
  configuration folder.
- Vault fields for creator, rights and geotags, which other importers
  (Lightroom, digiKam) would also fill.
