# RAW+JPEG pairs: one photo, several files

Status: **analysis** (2026-11-14). The model is decided by the owner
(2026-11-14): a RAW+JPEG pair is **one photo with two files**. This
supersedes the Library note's L-C6 decision (B, a stack made at import,
2026-11-11; see [library.md](library.md)): the stacking slice that was
being built for it stops, and its reusable parts move here (section 4).
The slices at the end wait for the owner's go.

## In short

- **A photo has files, each with a role:** the RAW is the **master**
  (the source of edits and versions), the camera's JPEG or HEIF is the
  **camera rendering**. Room for more roles: in-camera variants (crops,
  picture styles), attachments (voice memos, GPS tracks).
- **The photo's id is its master's content id**, so every field,
  version, stack, album and Library flag keyed on an image today stays
  as it is. The companion file stays a content-hashed image with its own
  locations, out of the Library on its own. A new table says which
  photo each companion file belongs to, and in which role.
- **Pairing** happens at import (the stopped slice's detection, reused)
  and after the fact ("Pair RAW+JPEG Files" / "Unpair", one undo step
  each). Photos already imported as two are merged by the same command,
  offered once.
- **Files, not photos,** for delete ("Delete JPEG Files (Keep RAW)…"),
  backups and "safe to format": every file of a photo counts.
- **Display:** one cell, a RAW+JPEG badge, the JPEG as the fast picture
  until the RAW is rendered, a toggle and a compare between the camera's
  JPEG and the edited RAW.
- **Export** renders the RAW by default, or copies the camera JPEG as is.
- **Workflows:** the item is the photo; its files ride in its envelope;
  a node or an extractor says which file role it reads.

## 1. What established applications do

| Application | Model | Notes |
|---|---|---|
| Lightroom Classic | One photo, the JPEG a sidecar (default); a preference, "Treat JPEG files next to raw files as separate photos", makes two | The sidecar moves, renames and deletes with the raw; it cannot be edited or exported from the catalogue; the grid shows "CR3+JPG" |
| Capture One | Two images; pairs can be stacked automatically at import, with a chosen file on top | Edits are per image; a stack is a grouping |
| darktable | Two images, grouped at import when they share a base name; the import dialog can ignore JPEGs | Group leader shown collapsed; each image has its own history |
| Photo Mechanic | One item for both files when "treat RAW+JPEG as a unit" is on | Tags, ratings, copies, renames and deletes apply to both files |
| digiKam | Two items; "group by file name", by hand or at import | Grouping is a display relation |
| Apple Photos | One asset with two resources; the JPEG is the original shown until "Use RAW as Original" | Delete removes both; exporting the unmodified original gives both files |
| Google Photos | Keeps both uploads; how a pair shows varies by client | No editing model for the pair |

Two families: **one object with several files** (Lightroom, Photo
Mechanic, Apple Photos) and **two objects grouped** (Capture One,
darktable, digiKam). The first is what users mean by "the photo"; the
second keeps one file per object, which is Helios's identity model.
The design below takes the first while keeping the second's identity
rule underneath: content ids stay per file, the photo groups them.

## 2. The model

- A **photo** is what the Library shows, marks, edits, stacks, puts in
  albums and flows through workflows. Its id is the content id of its
  **master** file.
- A **file** is a content-hashed image (`images`), with any number of
  **locations** (copies of the same bytes on several volumes, as
  today). Files of one photo are different contents; locations of one
  file are the same content. The words stay distinct in code and UI.
- A **role** says what a file is to its photo:

| Role | What | Edits | Shown |
|---|---|---|---|
| `master` | The RAW (or the only file) | yes: versions, heads | the rendered photo |
| `camera` | The camera's JPEG or HEIF of the same shot | no | fast picture, "camera JPEG" view |
| `variant` (later) | Another in-camera rendering: crop, picture style, several per photo, ranked | no | in the files list |
| `attachment` (later) | A voice memo, a GPS track, anything the shot carries | no | in the files list |

  XMP sidecars stay as today (kept beside a file, read on import): they
  are not contents of their own.
- **Marks and metadata belong to the photo**: rating, flag, labels,
  keywords, albums, rotation, title, captions. There is nothing to sync.
- **Edits belong to the master.** The camera file is read only.
- **A JPEG-only photo** has the JPEG as master. Pairing a RAW to it later
  makes the RAW the master (section 4's merge rules).

The cost of "the photo's id is its master's id": a change of master is
a re-key (pairing two photos, unpairing, deleting the RAW and keeping
the JPEG). Those are rare, explicit commands, each one undoable step,
run with the merge rules below. The alternative, a photo id apart from
any content, would re-key every table of the catalogue for every photo;
it is not worth it for pairs.

## 3. Storage

### The table

```sql
CREATE TABLE photo_files (
    image    TEXT PRIMARY KEY,   -- the companion file (an images.id)
    photo    TEXT NOT NULL,      -- the photo it belongs to (its master's images.id)
    role     TEXT NOT NULL,      -- camera, variant, attachment
    rank     INTEGER NOT NULL DEFAULT 0,
    added_at INTEGER NOT NULL,
    added_by TEXT NOT NULL       -- import, user, pair:<step>
);
CREATE INDEX photo_files_photo ON photo_files (photo);
```

- The master has no row: a photo with no row is a one-file photo, so
  nothing changes for photos without companions and the migration adds
  no row.
- `image` is the key: **a file belongs to one photo at most**. A file
  that is a photo's companion is never a master (the vault refuses).
- A companion file's own Library flag is off (`by: pair`). Every Library
  read already leaves out images not in the Library, so the companion
  disappears from the grid with no change to the queries.
- `remove_image` of a master detaches its companions first (they become
  plain images, out of the Library); of a companion, removes its row.
- The SQLite step is one numbered migration (table and index), nothing
  backfilled. Snapshots and restore carry the table like any other.

### The Store trait and the memory store

New `Store` calls, every engine checked by the vault's `contract`:

- `put_photo_file(photo, image, role, rank, at, by)`,
  `remove_photo_file(image)`;
- `photo_files(photo) -> Vec<PhotoFile>`, `photo_of(image) -> Option<(ImageId, Role)>`;
- `photo_files_of(ids) -> Vec<(ImageId, PhotoFile)>`: the windowed read's
  batch call, one query per window (the `library_scale` guard).

`MemoryStore` keeps a map from file to `(photo, role, rank)` and an index
from photo to files. The undo layer gets one more row kind (`Change`
for a photo-file row), so attaching and detaching join any step.

The vault adds `attach_file` and `detach_file` (one step each, audited),
`files(photo)` (the master first, then roles in order) and **`holds(image)`**:
true when the image is in the Library or is a file of a photo in it.
Every "is this file held by the Library" question asks `holds`, never
`in_library` (import's known check, the import window's `in_library`
fact, "safe to format", backups).

### Contract tests

- attach, read back by photo and by file, rank order;
- a file in two photos refused; a companion made master refused;
- removing a master detaches its companions; removing a companion drops its row;
- `photo_files_of` over a window returns only the window's rows;
- an attach and a detach reverted and replayed by the undo layer;
- a snapshot restored keeps the rows; an old database migrates with no row.

### Photos already imported as two

The SQL migration only adds the table. Merging existing pairs is the
"Pair RAW+JPEG Files" command (section 4) run over the whole Library:
after the upgrade, the health check finds the pairs (the same detection
as import) and offers "N RAW+JPEG pairs are shown as two photos: Pair
them", one undo step, a report of what each merge kept. Nothing is
merged without the user's word, because the merge chooses between marks.

**Merge rules** (the RAW becomes the photo, the JPEG its `camera` file):

| What | Rule |
|---|---|
| Single-valued marks (rating, flag, rotation, title, description) | The RAW's when set, else the JPEG's; a conflict (picked against rejected, two ratings) keeps the RAW's and is listed in the report |
| Set-valued marks (keywords, labels, albums) | Union |
| Stacks | The RAW keeps its stack; a JPEG in a stack is replaced there by the RAW when the RAW is in none, else removed from it |
| Edits | The RAW's versions are the photo's. The JPEG's versions stay in the catalogue under its id, hidden, and come back on Unpair; the report says how many |
| Library flag | The RAW's set (it takes the earlier date), the JPEG's cleared |
| Import history, audit | Kept per file; the merge is audited on both |

**Unpair** reverses it: the companion becomes a photo of its own, in the
Library, with a copy of the photo's marks and its own old versions.
"Delete RAW Files (Keep JPEG)" is an Unpair followed by trashing the RAW
(section 7).

## 4. Pairing

### At import

The stopped stacking slice's `pairs.rs` is reused as it is for
detection: same folder and, in order,
1. the same name apart from the extension, case ignored;
2. else, renamed copies: EXIF capture time within 0.5 s and the same
   camera, with no other candidate at that moment.

Additions:
- **HEIF/HIF** as a camera file (a `Heif` kind in the import profile's
  `Kind`, read by the loader for previews);
- **dual-slot cards:** the RAW on one card, the JPEG on the other. Same
  name and same time across the import's sources, not only the same
  folder, when both cards are in one import; across two imports, the
  after-the-fact command finds them (rule 2 without the folder);
- **several JPEGs per RAW** (in-camera crops, styles): the one with the
  same name is `camera`, the others `variant`, ranked by name. The first
  slices pair one JPEG and leave the others as photos.

The import's post-step (`pairs::brought`, already in the import's undo
step and its report) attaches instead of stacking. A file brought that
is already a companion counts as known.

### The import profile option

`stack_pairs` becomes **`pairs`**: `one_photo` (default), `raw_only`,
`jpeg_only`, and it stays threaded through the profile, the node's
`library.import` schema, automatic and rule imports and the profile
editor. `raw_only` and `jpeg_only` leave the other file where it is: on
a card, those files are listed as **left by choice** and do not stop
"safe to format" (the user chose to drop them); the card's report says
how many.

The option is **connector-declared**: the import window knows only
connectors. A connector whose items carry a `group` (the connector
contract's pair) declares the option; one that never sees pairs (a
cloud service of JPEGs) does not show it.

### After the fact

- **Pair RAW+JPEG Files** (`library.pairs.pair`): on the selection, or
  the whole Library from the health check; finds pairs by the rules
  above, or pairs exactly two selected photos (one RAW, one JPEG or
  HEIF) when told to. One undo step, the merge rules of section 3.
- **Unpair** (`library.pairs.unpair`): the selected photos' companion
  files become photos again. One undo step.

## 5. Display

- **One cell per photo.** A badge declared through the badge contract
  ([thumbnail-badges.md](thumbnail-badges.md)): text "R+J", bottom
  right, neutral, over a new cached fact `roles` (`{ fact = "roles",
  has = "camera" }`), filled by the windowed read's `photo_files_of`.
  The import window's "R+J" keeps its own fact (the item's group).
- **Filter:** `has:jpeg` (photos with a camera file) and `-has:jpeg`
  in the search field, through the vault query.
- **Fast picture:** the thumbnail reads the RAW's embedded preview when
  it is large enough (as today), else the camera file, DCT-scaled. The
  loupe shows the camera file until the RAW render is ready, then the
  render. When the RAW is offline and the JPEG present, the JPEG is
  shown, with the offline badge. The thumbnail cache stays keyed by the
  photo (its master's content and version).
- **Camera JPEG view:** a loupe toggle "Show Camera JPEG" (the file as
  the camera wrote it) and a Compare pair "Camera JPEG | Edited", for a
  photo with a `camera` file. The files list (info panel) shows each
  file with its role, kind, size and presence.
- **Camera look:** the fit reads the `camera` file when there is one
  (full size, the maker's own rendering) and the embedded preview
  otherwise. An in-camera crop frames differently: the fit's framing
  gate already refuses it.

## 6. Storage safety: backups and "safe to format"

- A photo is **backed up** when every file is: the strategy's status for
  a photo is the worst of its files' statuses; the per-photo badges and
  `backup:` filters follow.
- The backup job copies every file of each photo, not only the master.
- "Safe to format": a card file counts as held when `holds` says so, so
  a JPEG attached to a photo is held, and it must meet the strategy like
  the RAW. Files left by the profile's choice are counted apart.

## 7. Delete, remove, trash

- **Remove from Library** removes the photo; its companion files go with
  it (they were never in the Library on their own). Undo brings both.
- **Move to Trash…** trashes every file of the photo (every location of
  each, as today), through the same journaled job.
- **Delete JPEG Files (Keep RAW)…** (`library.images.trash` with
  `roles: ["camera"]`): trashes only the camera files of the selection,
  detaches them, and writes the detached rows into the trash job's
  document so a restore can attach them again. Asked first with the
  count, refused on cards and backups as today.
- **Delete RAW Files (Keep JPEG)…**: Unpair, then trash the RAWs; the
  question says the RAWs' edits go with them.
- Trash is not a history step (as today); detaching through Unpair is.

## 8. Export

The export settings gain a **source**: `render` (default: the master's
current head, as today) or `camera` ("Export camera JPEG as is": the
camera file's bytes copied, named by the pattern, its own extension).
A photo with no camera file under `camera`: its master is copied when
it is a JPEG with no edit, else rendered; the report counts each case.

## 9. Workflows and extractors

- **The item that flows is the photo** (one envelope per photo, the
  diagram-items model). Its envelope carries its files:
  `files: [{ role, image, kind }]`, so no node splits or joins a pair.
- A node or an extractor declares **which file it reads**: `master`,
  `camera`, or a preference (`fast`: the camera file, else the master's
  preview; `accurate`: the master). The run's read order then picks a
  location of that file, as `read_order` does today.
- Extractor results are cached by the **file's** content id and the
  extractor's version: valid whatever the pairing, and two photos never
  share a cache by accident.
- Routers get facts over files: `roles has camera`, `master kind raw`.
- The camera file is the reference a look fit reads: it is in the
  envelope, so no register is needed for it.

## 10. Node and MCP

- `library.*` reads of a photo (the windowed read, `library.fields.get`)
  add `files: [{ role, image, kind, ext, size, presence }]`, the master
  first.
- New commands: `library.pairs.pair`, `library.pairs.unpair`,
  `library.files.list` (a photo's files with their locations);
  `library.images.trash` gains `roles`; export gains `source`;
  `library.import` takes `pairs`.
- An agent addresses a photo by its id and a file by its role; a file's
  own id is there for reads, never needed to act.

## 11. Open questions (not decided)

1. Lightroom's "treat as separate photos" as a fourth profile choice
   (two photos, nothing linked). Recommended: not offered at first; Unpair
   covers the need.
2. The one-time merge of existing pairs: offered by the health check
   (recommended), or run on its own at the upgrade.
3. The JPEG's hidden versions after a merge: kept for Unpair
   (recommended), or offered as heads of the photo.
4. Variants (several JPEGs per RAW) in the grid: only in the files list
   (recommended), or as heads.
5. Metadata written back (sidecars, L14): to the RAW's sidecar only, or
   also into the JPEG.

## 12. Slices

Each slice is one PR, with its node command and its tests.

1. **Storage.** `photo_files` in the `Store` trait, `MemoryStore` and
   SQLite (one migration), the undo row kind, `attach_file`,
   `detach_file`, `files`, `holds`. Tests: the contract tests of
   section 3 on both engines; the migration from the current schema;
   a snapshot round trip.
2. **Pair and Unpair, and the merge of existing pairs.** The merge rules
   as a pure function over two photos' rows, `library.pairs.pair` and
   `.unpair`, one history step each; the health check's offer. Tests:
   the rules table case by case; undo restores two photos with their
   marks, stacks and albums; Unpair brings back the JPEG's versions.
   *Built 2026-11-14:* `helios_vault::pairs` (the rules, the vault's
   `pair` and `unpair`), `helios_library::pairs` (detection, reused from
   the stacking slice, HEIF included), the health check
   `library.raw-jpeg-pairs` whose fix runs the desktop's
   `library.files.pair` over the whole Library. A mark of no value (0
   stars, no flag, rotation 0, an empty text) counts as unset; albums
   and the quick collection follow the stacks' rule (the JPEG's place
   goes to the RAW).
3. **Pairing at import.** `pairs.rs` attaches instead of stacking;
   `pairs` replaces `stack_pairs` in the profile, the node schema,
   automatic and rule imports and the editor; HEIF; "left by choice".
   Tests: an import fixture (same name, renamed by time, HEIF, a lone
   JPEG), the import undone, the profile round trip, the node's
   rule-path import, a re-import counting the JPEG as known.
4. **Library read and display.** `files` in the windowed read, the
   `roles` fact, the "R+J" badge, the `has:jpeg` filter, the files list.
   Tests: `library_scale`'s query counts unchanged; a badge snapshot; the
   filter through the vault query.
5. **Fast picture and the camera view.** Thumbnail and loupe fallback to
   the camera file, the offline case, "Show Camera JPEG", Compare's
   pair; the camera-look fit reads the camera file. Tests: the source
   choice as a unit test; a snapshot of the toggle; the fit's input pick.
6. **Backups and "safe to format".** `holds` in card safety, per-photo
   status as the worst file, the job copying every file. Tests: a card
   fixture with a pair (JPEG short makes it unsafe), a profile choice
   leaving JPEGs, the backup job's copied list.
7. **Per-file trash.** `roles` on the trash job, the two commands, the
   restore re-attaching. Tests: the `Bin` folder fixture, the job
   replayed after a kill, refusals.
8. **Export source.** `render` or `camera` in the settings and the plan.
   Tests: the plan for a pair, a JPEG-only photo, a RAW-only photo.
9. **Workflows.** Files in the envelope and the file-role choice for
   nodes and extractors, once the diagram-items envelope exists. Tests:
   a run over a pair reading each role; the extractor cache keyed by
   file.
