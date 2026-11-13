# The library: import, duplicates and stacks

The vault keeps images by content (an image *is* the BLAKE3 hash of its
file) and where their copies are as data (see [persistence.md](persistence.md)
and the `helios-vault` crate). The library operations of `helios-library`
work on top of it: they bring files in, measure what makes two photos
near duplicates, and group photos into stacks. They run the same from the
command line (`helios import`, `helios stacks`, see [cli.md](cli.md)) and
from the desktop (the `library.*` commands).

Under way (2026-11-08): the vault records every image Helios sees on
every known volume, and import becomes a flag plus the copies the backup
strategy asks for; see [design/image-map.md](design/image-map.md).

## The Library and the map

An image is **in the Library** when its flag says so, with when and by
what it was set (an import, the user, an agent, `migrated` for the
images a vault held before the flag existed). Every read of the Library
leaves the other images out: its listing, queries and cursors, stacks
(a member out of the Library is not shown; a stack left with fewer than
two is not shown at all), every location the scan and the Storage page
count, backup status and backup jobs, and "safe to format" (a card file
whose image is out of the Library is not held by it). An image out of
the Library keeps its marks, versions and sightings; putting it in or
out is one undo step of the library, which moves the flag only.

**Import is a flag.** An import records each file it brings (its image
if unknown, off the Library, and its sighting), then sets the flag, with
when and the import's job as what set it (`library_by: import-12`).
Recording an image (`Store::add_image`) never puts it in the Library:
only an import does (the CLI's `helios import`, importer plugins,
darktable imports, files dropped or opened). A file added where it is
and seen before, unchanged since (the same size and modification time,
its content known), is flagged without being read again. Copies follow
the placement: "Add where they are" makes none, "Copy into the Library"
makes one by the verified copy, and the strategy's backups come after
(on import) from the backup job; each copy is a sighting where it
landed. Duplicates and "in your Library" are lookups on the flag: an
image only seen, or whose import was undone, is no duplicate.

The vault also keeps **sightings**: one file at one place (a volume and
its path there, or an absolute path on no volume yet), with its size and
modification time, and what reading it told: its partial hash, its image
once fully hashed, its capture time, camera and lens, when it was first
and last seen and last verified, its presence, and what listed it. A
location is the sighting of a file whose image is known. Imports, scans
and backups write them, and so does every source the import window
lists, folders and cards (`seen_by: import`) and files dropped or pasted
(`drop`): each file's place (the volume holding it and its path there),
size and modification time, the partial hash when the "in your Library"
prefilter read it anyway, then the capture time and camera its glance
tells. Nothing is read for the map alone. A file listed again unchanged
keeps what was read of it; changed in place, it is read again, except a
Library file, left to the background job. Rows are written off the
interface thread, a few hundred per transaction
(`helios_library::map::Recorder`). `system.map.record` says what is kept:
`listings` (the default), `everywhere`, or `import` (listings keep
nothing). File dialogs record once egui-pop's file browser has a source.

**The `map` job** reads what the map knows, one low-priority job in the
jobs centre (`helios_library::map_job`): first the files in view in the
import window (their partial hash), then Library files never verified
by their full hash, then the files of volumes just plugged in, then
duplicates to confirm (same size and partial hash as an image: the full
hash links them to it), then every other file without a partial hash.
It never reads a blacklisted place nor a file changed since it was
listed. It waits while an import, an export, a batch, a backup or a
render runs, and on battery (Linux reads `/sys/class/power_supply`;
Windows and macOS count as on mains for now); it can be paused and
cancelled in the jobs centre, and starts again when there is something
new to read or an hour later. It prunes the map (below) at most once an
hour. Its entry shows while it has work and ends once it has none.

**Forget a volume or a folder** drops its sightings not in the Library
and marks the folder `forgotten` in the `folders` table: listings and
scans leave it out until it is listed again on purpose
(`Vault::forget`, `Vault::folder_listed`). The commands
`library.forgetFolder` (`path`) and `library.forgetVolume` (`volume`, its
id) do it, from a source's menu in the import window ("Forget this
folder") and a volume's "Forget what was seen here" on the Storage page;
the volume itself stays known. The upkeep
(`Vault::prune_sightings`) drops sightings not in the Library missing
for longer than `system.map.keepMissing` (30 days), then the ones seen
longest ago past `system.map.limit` (1,000,000); Library sightings are
never dropped.

**The blacklist** keeps places out of the map: never recorded, never
thumbnailed into the cache. By default (`system.map.defaults`, on):
Helios's own data (`.helios/`, `.helios-part/`, its data and cache
folders), folders tagged `CACHEDIR.TAG`, version control and build trees
(`.git`, `.hg`, `.svn`, `node_modules`, `__pycache__`, `.venv`), hidden
folders and files, NAS thumbnails (`@eaDir`, `.@__thumb`), trash folders,
other photo applications' caches and libraries (`*.lrdata`,
`*.photoslibrary`, Capture One's `Cache`, darktable's `mipmaps-*`), and
the system's folders: on Linux `/proc`, `/sys`, `/dev`, `/run`, `/tmp`,
`/var`, `/usr`, `/etc`, `/boot`, `/snap`, `/opt`, `~/.cache`,
`~/.local/share`, `~/.var`; on Windows `C:\Windows`, `Program Files`,
`ProgramData`, `%APPDATA%`, `%LOCALAPPDATA%` and every volume's
`System Volume Information` and `$Recycle.Bin`; on macOS `/System`,
`/Library`, `/Applications`, `/private`, `/usr`, `/opt`, `~/Library` and
every volume's `.Spotlight-V100`, `.fseventsd`,
`.DocumentRevisions-V100`, `.Trashes`. A folder excluded takes its
subfolders with it.

`system.map.exclude` adds rules, as a JSON list:
`[{"glob": "**/Private/**"}, {"glob": "Archive/Clients/**", "volume":
"uuid:5A3C-11F0"}, {"glob": "!**/.photos/**"}]`. A glob matches the
absolute path, or with `volume` the path inside that volume; `**` crosses
folders, `*` and `?` stay in one, case is ignored on Windows and macOS,
and a glob covers what the folder it names holds. A glob starting with
`!` re-includes what a default excludes; the last rule that matches
wins (`helios_storage::exclude`).

## The default library

`helios-gui` opens a library at every launch: the vault file given with
`--vault FILE`, else the default library, `library.db` in Helios's data
directory (`$XDG_DATA_HOME/helios/library.db`, else
`~/.local/share/helios/library.db`; `%LOCALAPPDATA%\helios\library.db`
on Windows, `~/Library/Application Support/helios/library.db` on macOS),
made with its folder on the first launch. The import window's choices
are kept per library file.

When the library cannot be opened (an error reading it, a read-only
disk), a toast says why and Helios runs without one: the import window
shows "No library is open" with the reason, its Import button is off,
and the import commands refuse with the same reason.

## Import

```bash
helios import --vault library.db /media/card                      # a mounted card, referenced
helios import --vault library.db --copy --into ~/Photos /media/card
helios import --vault library.db --copy --rename '{date}-{seq}' /media/card
helios import --vault library.db --duplicates keep ~/Pictures/2024-06-trip
```

**Sources.** A folder and its subfolders, a single file, or a mounted card:
a folder holding `DCIM` is read from there. A phone speaking MTP is not a
source yet; mount it as a folder first.

**Placement.**

- `--reference` (the default): files stay where they are, the vault records
  where it saw them. A file already known at that place is only verified.
- `--copy`: files are copied into the library folder, as
  `YYYY/YYYY-MM-DD/<name>` by capture date (`undated/` without one), or as
  `--layout` says: `month` (`YYYY/YYYY-MM/`), `year` (`YYYY/`) or `flat`
  (the library folder itself). The
  library folder is given once with `--into DIR` and remembered in the
  vault. The source is read once: the copy is hashed as it is written
  (the hash is the file's identity, so duplicates are found before the
  copy takes a name), then read back and takes its name only when it
  holds the same content; until then it waits in the library folder's
  hidden `.helios-part/` folder, where its signals are measured too.
  Nothing is ever overwritten: a name taken by
  another file gets `-1`, `-2`..., a name holding the same content is used
  as it is. The copy keeps the original's modification date. The source is
  left as it was.

**Renaming** (`--rename PATTERN`, copies only: a referenced file stays as it
is). Fields: `{date}` (`YYYYMMDD`), `{time}` (`HHMMSS`), `{year}`, `{month}`,
`{day}`, `{camera}` (the model), `{maker}`, `{frame}` (the camera's frame
number), `{stem}` (the original name without its extension), `{seq}` (the
file's place in this import, from 1, four digits) and `{seq:N}`. The
original extension is kept. A field without a value writes `unknown`.

**Sidecars.** A copy takes its XMP sidecars along, renamed with it as
darktable names them: the base `NAME.EXT.xmp` and each duplicate
`NAME_NN.EXT.xmp` (matched as `helios import-darktable` matches them). A
sidecar whose new name is taken is left out and listed, never written
over.

**darktable edits.** The desktop's imports (and `library.import`, unless
`darktable` is false) also keep each file's darktable sidecars as versions
of its image, as `helios import-darktable` does (the same code,
`helios_library::darktable`): the base sidecar and each `_NN` duplicate
become first-edit versions side by side, never merged, and only the base
gives the image its rating. They are read where the file came from, so a
copy gets them too. Each file's line lists them under `darktable`, with
their import report (items not applied, modes substituted) or their
error; the report counts `edits` kept, `edits_reported` and
`edits_failed`. Importing again adds nothing for a sidecar already kept.
A sidecar whose image is not imported (an orphan) is left to `helios
import-darktable`, which keeps it in the vault's import record.

**One job.** An import reports as a job (see [jobs.md](jobs.md)), so the
desktop shows it in the jobs centre: `job.started` (kind `import`, id
`import-<n>`, its title and the number of files), one `job.progress` per
file (`done` of `total`, the file at hand as its `item`, the files failed
and skipped so far), a `job.log` line per file brought in, a
`job.item_failed` per file that failed, then `job.finished`, or
`job.failed` when it cannot start (a missing source, no library folder for
a copy). A file that fails does not stop the others.

**Safe to format.** At the end of a copy, each card read (a source
holding `DCIM`) is checked by content hash: a photo counts when the
library holds its image with a file in the library folder (a copy
verified as it was written, or one made before). The report's `cards`
lists each card (`path`, `label`, `photos`, `missing`,
`safe_to_format`) and the job's log says "EOS_DIGITAL: safe to format"
or "EOS_DIGITAL: 2 photos not in the Library" (a file that failed, a
duplicate of a referenced file left out, a file not reached). Helios
never formats a card itself.

**A lost source pauses.** A card taken out or unmounted (its `DCIM`
gone), or a folder moved away, mid-import does not fail every file left:
the job pauses (`job.paused`, its reason also a log line: "Reinsert
EOS_DIGITAL to go on", the card's name being its mount folder's), looks
for the source again every quarter of a second, and resumes
(`job.resumed`) once it is back, the file under way tried again. A paused
import can still be cancelled.

**Cancel.** The jobs centre's Cancel (`job.cancel` with the job's id), or
Ctrl-C for `helios import`, stops the import before its next file. A copy
under way stops too, and its hidden part (in `.helios-part/`) is removed: no
half-written file is left. The files done stay imported; the job ends
`job.cancelled` and the report says `"cancelled": true`.

The CLI prints one JSON line per file (`path`, `id`, `status`, and
`duplicate`, `to`, `sidecars`, `signals`, `error` when they apply; a file
stopped by a cancel says `cancelled`), then `{"report": ...}`; it exits
with 1 when a file failed, 130 when interrupted.

## Exact duplicates

The content hash of the whole file is computed for every file (streamed,
never held in memory), unless the file was seen before unchanged and its
content is known. A file is an exact duplicate when its image is in the
Library (a lookup on the flag) (`"duplicate": {"of": "library", "at": [...]}`), or when an
earlier file of the same import had it (`{"of": "source", "first": ...}`).
`--duplicates` says what to do:

| Policy | What happens |
|--------|--------------|
| `skip` (default) | left out: status `skipped` |
| `keep` | imported as one more copy of the same image: status `located` (a copy is made with `--copy`) |
| `ask` | left out and listed (status `asked`), for whoever started the import to answer, by importing those files again with `skip` or `keep` |

Nothing is ever deleted, whatever the policy. A file that changed (same
path, other content) is a new image, except the file of a Library photo:
changed by another application, it stays that photo, with its marks and
edits; the thumbnail re-check records its new content, makes its
thumbnail again and marks it "changed outside" until acknowledged on the
Health page (`library.changedOutside.acknowledge`).

## Near-duplicate signals

Exact duplicates share a hash; near duplicates (a burst, a bracket, a
re-encode) do not. Each import measures, for each image, and keeps in the
vault (namespace `signals`, one document per image):

- `phash`: a 64-bit difference hash of the embedded preview of a raw (or of
  the picture itself, or of a small decode of the mosaic when a raw has no
  preview): the preview is reduced to a 9 x 8 grey thumbnail and each bit
  says whether a cell is darker than its right neighbour. Near-identical
  frames differ by a few bits; a change of exposure moves few of them;
  another scene differs by about half. `hashed` says what was hashed;
- `taken`: the capture time from EXIF (`DateTimeOriginal` and its
  sub-seconds), read as the camera's clock (only differences are used);
  the file's modification time when there is none (`taken_from`: `exif` or
  `file`);
- `exposure_time`, `aperture`, `iso`, `exposure_bias`, `maker`, `model`;
- `frame`: the camera's frame number (EXIF `ImageNumber`), else the digits
  the file name ends with (`frame_from`);
- `burst`: a burst identifier, for cameras that write one (none read yet:
  it waits for maker notes);
- `sharpness`: the variance of the Laplacian of a grey thumbnail at most
  512 pixels a side.

## Stacks

A stack gathers images that are one shot in practice: its members, the one
shown on top, and whether it is shown expanded or collapsed. An image is in
one stack at most. Making or dissolving a stack never touches its images.

```bash
helios stacks --vault library.db list
helios stacks --vault library.db create IMG_0001.CR2 IMG_0002.CR2 --top IMG_0002.CR2
helios stacks --vault library.db top STACK IMAGE
helios stacks --vault library.db expand STACK      # or collapse
helios stacks --vault library.db dissolve STACK
```

An image is named by its id or by a file the vault knows.

## Suggesting stacks: `stacks.suggest`

`stacks.suggest` is a library decision: it asks a **resolver** for stacks
over a selection (every image when none is given), and records the
proposal. Nothing is applied until a person accepts.

```bash
helios stacks --vault library.db suggest                 # proposal: decision stacks.suggest-1
helios stacks --vault library.db accept stacks.suggest-1 # every group becomes a stack
helios stacks --vault library.db accept stacks.suggest-2 --groups 0,2
helios stacks --vault library.db decline stacks.suggest-3
helios stacks --vault library.db decisions
```

The default resolver, `rules`, goes through the images in capture order.
A frame continues the group of the one before it when:

- they come from the same camera (when both say), and
- they are the same burst, or were taken within `--window` seconds (2 by
  default), or, when neither has a time, have consecutive frame numbers;
  and
- their perceptual hashes differ by at most `--distance` bits (10 of 64),
  or by at most 18 when their exposures differ by 0.3 stop or more (a
  bracket changes the picture more than a burst).

A group of two or more is proposed, as a `burst` or a `bracket`, with a
reason, and the sharpest member (by the variance of the Laplacian) as its
proposed top. Images already in a stack, or without signals, are left out
of a suggestion and listed in `left_out`.

The decision is kept in the vault (`library/decisions`) with the
vocabulary of decision nodes (see [module-contract.md](module-contract.md)):
the proposal (the groups, the resolver's model and version, its
rationale, when), then the answer: `accepted` when every group is taken,
`corrected` when only some are (`--groups`), each with who answered and
the model they answered; a declined suggestion is recorded as a human
answer with no stacks. Each accepted group becomes a stack made by
`decision:<id>`, its proposed best on top. A decision is answered once;
asking again makes a new one. Changes are announced on the bus
(`library.decision`, `vault.stack_changed`).

Resolvers are picked by name (`--resolver NAME`): `rules` is the only one
today. A resolver that calls a model plugs in under its own name, through
the same interface (`helios_library::stacks::StackResolver`), when the AI
models work brings them.

## On the desktop

With a vault open, the desktop declares the library commands, run from the
palette or by an agent:

| Command | Arguments |
|---------|-----------|
| `library.import` | `path` (or `paths`, several files and folders as one import); `copy`, `into`, `rename`, `duplicates` as on the command line; `darktable` (`false` leaves the sidecars out). Runs in the background as a job, in the jobs centre and cancelled from it; the library is filled again when it ends |
| `library.importSource` | `path` (a card or folder; the import dialog asks for it when absent); `copy`, `darktable`, `remember` (see below). The import window's Import runs it per source ticked |
| `library.importWindow` | `source` (the card or folder it opens on). Opens the import window (see `docs/design/import-window.md`): cards and cameras first, recent folders, favourites, "Choose a folder…". File > Import… (Ctrl+Shift+I), the empty library's "Import photos…" and the toast of a card inserted run it |
| `library.importFiles` | `paths`; `copy` (`true`: into the library folder, `false`: referenced); `ask` (`true`: without an import source, the import dialog asks first); `darktable`. An import placed, renamed and with duplicates handled as the workspace's import source says, unless `copy` is given: what a paste of files and a drop on the window ask for |
| `library.stacks.suggest` | `images` (ids; every image when absent), `resolver` |
| `library.stacks.accept` | `decision`, `groups` (all when absent) |
| `library.stacks.decline` | `decision` |
| `library.stacks.list` | |
| `library.stacks.dissolve` | `stack` |

### Importing from the desktop

Importing is always within reach, whatever the workspace (decided
2026-11-06):

- **File > Import…** (Ctrl+Shift+I, as in Lightroom) asks for a card or a
  folder;
- the empty **Library** and **Filmstrip** panels show **Import photos…**;
- files and folders **dropped on the window** are imported (the window
  says "Drop to import into the library" while they are dragged over it);
- a paste of files (Edit > Paste) imports them, as before.

A workspace whose template has an import source imports as it says. Without
one, the import dialog asks where the photos go: **referenced in place**
(the default: the files stay where they are) or **copied into the library
folder**. **Remember for this workspace** makes the answer the workspace's
import source (a `folder` source, duplicates skipped; one Edit > Undo
step), so the next import does not ask; outside a workspace there is
nothing to remember it in, and the box is not shown. The dialog's
**Read darktable edits** box (on by default; an import source's
`darktable: false` turns it off) keeps each photo's `.xmp` sidecars as
versions (see "darktable edits" above).

**Undoing an import (2026-11-07).** Each import that changed the Library,
finished or cancelled, is one step of the Library's history, labelled
"Import 124 photos" (the photos added or found at a new place). Edit ›
Undo (Ctrl+Z) with the Library, the Filmstrip or the Duplicates panel
shown takes it back when it is the newest step there (an edit made after
it is undone first); the History panel lists it under the `library`
scope. Undo takes the photos the import brought out of the Library
(their flag cleared) with the values and darktable edits it gave them,
and on photos the Library already had, the values an importer gave. What
Helios saw stays: the images, their sightings and measured signals, so
importing them again reads nothing it knows. Redo sets the flag and the
values again; "Remove from Library" clears the flag the same way. **No file is touched**: what was referenced stays where it is, and
the copies the import made stay in the library folder, sidecars
included. A later import of the same photos finds those copies there by
their hash and does not copy them again. A photo edited since the import
(a version saved) or found at another place since is kept, and a value
changed since is not set back. The steps last for the session.

When an import ends, a short toast says how many photos and darktable
edits came in. The darktable edits not kept whole (items not applied,
modes substituted, sidecars that could not be read) are listed in the
**Log** panel, one line per sidecar with its report, and the toast then
points there.

**File > Import from →** lists the importers that plugins declare (the
submenu is shown only when one is installed). Choosing one asks for what
it reads, a folder or a file, and where the photos go as above; the
plugin then lists the photos and what it knows of each (rating, reject,
colour labels, tags, title, description), and Helios imports them itself,
as any import: referenced or copied, duplicates and darktable sidecars as
usual. The values the importer gives are applied before the sidecars and
never overwrite what the library holds: a rating, reject, title or
description is set only where the photo has none, labels and tags are
added. What the plugin found, left out (a path outside the folder, a
file that is not an image) and warned about goes to the **Log** panel.
The scan is a job of its own in the jobs centre, cancelled there. See
docs/extensions.md, "Importers", for the protocol.

Helios ships one importer: **File > Import from → darktable library**.
Pick darktable's configuration folder (`~/.config/darktable` on Linux,
the one holding `library.db` and `data.db`); every photo file the library
holds comes in with its stars, reject, colour labels, tags (the whole
`places|France|Lyon` path as one tag) and title and description, and its
darktable sidecars as versions as for any import. The library is only
read, never written, and while darktable is running it is read from a
copy taken when the scan starts. Photos whose file is not on disk (a
drive not plugged in) are left out and counted in the **Log** panel: run
the import again once the drive is back. darktable 4.0 to 5.6 libraries
are read; see docs/darktable-library-import.md, "Status: implemented
(v1)".

## Culling on the desktop

The library panels are egui-pop's photo library widgets (its
`docs/library.md`) over the vault:

- **Library**: the search field, the sort, the marks of the image under
  the cursor (stars, colour labels, pick or reject, tags) and a
  virtualised thumbnail grid. Thumbnails are the raws' embedded previews (a picture itself),
  read in the background, the cells in view first, and shown as they
  arrive.
- **Filmstrip**: the same images in one row, sharing the grid's
  selection.
- **Compare** (closed by default): the two to four images selected side
  by side, each drawn from its edit, zoom and pan synced (a toggle); mark
  one best, then **Keep the best** picks it and rejects the others. One
  photo is active, outlined: a click or its name above makes another
  active, and its stars, colour labels and flag show above the photos.
- **The Compare place** (2026-11-11): the Library's **Compare** button,
  `C` or `library.compare` on two to four selected photos opens them in
  a place of their own (`helios.comparison`, the photos named in its
  payload, so the workspace reopens on them): the Compare panel in the
  centre, the filmstrip under it, as Lightroom's Survey view. The arrow
  keys move the active photo, which is the selection there, so the
  culling keys mark it. The comparison (its photos, the active one, the
  best, zoom and pan) belongs to the window's focus, not to the library
  view, so it can become a window of its own. File > New workspace
  offers "Compare", a workspace that opens there.

**Sorting** (2026-11-11, see [design/library.md](design/library.md),
L3): the grid shows the photos by capture time, oldest first, unless
sorted otherwise. The Library toolbar's menu sorts by capture time,
import time, file name, rating, last edit or file size, and the arrow
beside it turns the order around; `library.sort` does the same for an
agent (`by`, `descending`; without `by` it answers the sort in force).
The vault sorts, in the query that reads the library: photos without a
value (never taken by a camera, never edited) come last either way, and
ties go by file name for the capture time, then by content, so the
order is stable. A stack's members follow its top photo. The sort is the
window's, kept with its workspace (`library.sort` in its template, one
Edit > Undo step each) and back when the workspace opens again. A mark
does not move a photo while culling: the order follows the next read
(an import, a scan, a sort).

Marks go to the vault as fields: `rating`, `labels`, `picked` and
`rejected` (a reject wins over a pick; taking either back keeps the
stars), and `tags`. Each mark runs egui-pop's library step
(`library.rate`, `library.flag`, `library.label`, `library.tag`), from a
key, the grid, the marks bar or an agent's `library.*` command, so a
plugin may veto or redirect it; what ran is what is written. A mark
applies to the selection, else to the image under the cursor.

**Culling keys**, while a library panel has the focus outside the
darkroom, or while culling (Lightroom's, and darktable's R and F2 to F5),
each a command a profile or a template may rebind (the whole map is in
[gui.md](gui.md), "Keyboard shortcuts"):

| Keys | Command |
|------|---------|
| 0 to 5 | `vault.rate.N` |
| P, X, U | `vault.flag.pick`, `vault.flag.reject`, `vault.flag.none` |
| R | `vault.reject` (toggled) |
| 6, 7, 8, 9 | `vault.label.red`, `.yellow`, `.green`, `.blue` |
| F2, F3, F4, F5 | `vault.label.yellow`, `.green`, `.blue`, `.purple` |
| S | `vault.stack.toggle` |
| B | `library.quick.toggle` (the quick collection) |
| Ctrl+Shift+K | `library.filter.switch` (the recent filters; not while typing) |
| Left, Right | `vault.previous`, `vault.next` |
| Backspace, Space (culling only) | `vault.previous`, `vault.next` |

`vault.autoAdvance` (or the Auto-advance box) moves the cursor to the
next image after each mark, as Lightroom's Caps Lock mode. The grid's
own keys (arrows, Shift and Ctrl with them, Space, Ctrl+A, Enter, + and
-) work as egui-pop describes; it reads no culling key itself, so a key
taken away stays away; Enter or a double click opens the image
in the viewport.

**Stacks.** A stack shows collapsed in the grid and the filmstrip: its
top image with a count and a chevron; the chevron, S or
`vault.stack.toggle` expands it (its members follow the top image), and
the vault keeps it so. The **Stacks** panel (closed by default) lists the
`stacks.suggest` proposals waiting: each group (a burst or a bracket, why
on hover) with its images, the one proposed on top starred, and a box to
take it. **Accept all** records `accepted`, **Accept the checked**
`corrected` (only the groups checked become stacks), **Decline** a human
answer with no stacks. **Suggest stacks** asks the `rules` resolver over
the images selected (two or more), else over every image; **Show**
selects a group's images, to look at them in the grid or the Compare
panel.

**Duplicates.** The **Duplicates** panel (closed by default) lists the
exact duplicates each import from the desktop found: the file, what it
duplicates (the library's copies, or the earlier file of the same
import), and what the import's policy did with it (skipped, or kept as
one more copy). With `ask`, each one waits for an answer there, **Keep**
or **Skip** (or all of them at once), and a note says so when the import
asks: keeping imports that file again with `keep`, placed and renamed as
its import said; skipping leaves it untouched. Nothing is deleted.

**The intake template.** The built-in workspace template **Import, cull,
de-duplicate** (see [templates.md](templates.md)) sets all of it up for
the first step of a shoot: the grid central with Compare and the image
(the loupe) as its tabs, the Stacks and Duplicates panels beside it, the
filmstrip under; a card copied into the library and renamed
(`{date}-{seq}`), its duplicates asked about, stacks proposed after each
import; auto-advance on; the culling keys also while the image has the
focus (there they replace the mask tools' keys).

**Copy and paste (2026-10-28).** While the grid or the filmstrip has
the focus, Edit › Copy (Ctrl+C) copies the images selected, or the one
under the cursor (`application/x-helios.images+json`: each image's id,
its file name and where its file is); other applications get the file
names, one per line. Edit › Paste (Ctrl+V) there imports, after a
confirmation, what the library lacks: the images copied that it does not
hold (the same file hash), from their files, or the files and folders
another application's text names (absolute paths or `file://` URIs, one
per line, as file managers copy them). An image the library holds
already is a reference: nothing is copied, and a paste with nothing to
import is refused with the reason. The import is `library.importFiles`,
one job like any import, placed as the workspace's import source says;
paste special (Ctrl+Shift+V) chooses **Import the files where they
are** or **Copy the files into the library folder**. Each import is one
undo step (see "Undoing an import" below).

**Files and images with other applications (2026-10-28).** Edit › Copy
files (`library.copyFiles`) puts the files of the images selected on the
system clipboard as a file manager copies them (`text/uri-list` on Linux,
a file list on Windows and macOS), paths whole: the user asked for the
files, where a plain copy gives their names only. Images without a file
at hand are left out and said. Edit › Paste from other applications
(`library.pasteFiles`, with a dry run) reads what another application
copied: files a file manager copied are imported as above; an image (a
browser's or an editor's "Copy image") is saved as a PNG file and
imported, copied into the library folder. Both are asked first. A text
pasted with Ctrl+V that names no file (a file manager that gives names
only) takes the files the system clipboard holds beside it. Only the
user copies to and pastes from other applications: an agent neither
reads the system clipboard nor writes over it.

**Search.** The whole filter is one vault query, run in the statement
that opens the library's cursor, the same on every vault driver: the
search field's rating, labels (any of them), flags, tags (all of them)
and dates, and in its text Helios's own keys:

- `camera:<model>`, `lens:<lens>`, `type:<cr3>`: the capture fields,
  exact, served by the vault's index on text fields; a value with a
  space is quoted (`camera:"Z 8"`); several of one key take any;
- `folder:<path>`: a photo with a copy in that folder or below, present
  or not;
- `presence:present`, `presence:offline`, `presence:missing`;
- `backup:met` (or `ok`), `backup:short` (short, unverified or with no
  backup), `backup:unverified`, `backup:none`, measured in the query
  against the library's strategy and backup volumes.

The other words are looked for, whatever their case (accents
included), in the name the Library shows (the first present copy's file
name, else the start of the photo's id) and in its tags. A date is the
**capture day** by the camera's clock (`date:2024-06-01..2024-06-30`
takes both days whole), served by the vault's index; a photo whose
capture is not read yet matches no date. A filter shows stacks
flattened.

**Facet counts.** While a filter is in force, lines under the Library's
toolbar count its photos per value of each facet: rating (exact stars),
colour label, flag, camera, lens, file type, presence and, with a
strategy, backup state. They come in one more vault query, whatever
the size of the library; a click on a value adds it to the filter
(`rating>=N` for stars).

**Capture metadata.** Each photo's capture time, camera maker and
model, lens, focal length, aperture, shutter, ISO, width, height, EXIF
orientation and file type are read-only vault fields (`captured`,
`capture_offset`, `captured_from`, `camera_make`, `camera_model`,
`lens`, `focal_length`, `aperture`, `shutter`, `iso`, `width`,
`height`, `orientation`, `file_type`), read at import without decoding
a pixel. The capture time is the camera's clock (EXIF
`DateTimeOriginal`) read as UTC, in seconds, with the clock's offset in
minutes when the file says it (`OffsetTimeOriginal`); a file with no
EXIF time takes its modification time, in UTC (`captured_from: file`).
Photos imported before these fields existed are filled by a background
job of kind `capture` in the jobs centre, started with Helios: it reads
each photo's first present copy, or, for one on an unplugged disk, what
the import kept (its signals, its sightings); it keeps its progress in
the vault, resumes where it stopped, and does nothing once the whole
Library is done. No command changes these fields and they leave no
audit row: the file is their record.

## Storage

Where files live is described by **volumes** (the `helios-storage` crate;
design in [storage-backup-research.md](storage-backup-research.md)): an
internal or external disk, a card or stick, a network share, later a
remote store through a plugin. Each volume has:

- an **id** and how it was found, strongest first: a marker file Helios
  wrote (`.helios/volume.json` at its root), the filesystem's UUID, a
  share's address (`smb://nas/photos`), else its label and size ("weak":
  enough to say "reinsert EOS_DIGITAL", never to count a backup);
- a name (the user's, its label until then), a kind (internal, external,
  removable, share, remote), its capacity and free space, where it is
  mounted when online, and the physical drive behind it (two partitions
  of one disk are one drive);
- the **node** an internal disk belongs to (the machine's host name),
  and when and on which node it was **last seen**;
- whether it holds `DCIM` (a card or a camera), and its **roles**:
  library, archive, backup, ingest, scratch. A card starts as ingest, the
  disk holding the library folder as library; the user sets them on the
  Storage page;
- whether it is **off site** and whether it is a **RAID**, both set by
  the user. A RAID counts as one copy: it survives a dead disk, not a
  deletion, corruption or theft.

On Linux the volumes are read from `/proc/mounts`, `/dev/disk/by-uuid`,
`/dev/disk/by-label`, `/dev/disk/by-id` and `/sys/block`, with free space
from `statvfs`; Windows and macOS list none yet. The import window's cards
are the removable volumes holding `DCIM`.

The volumes Helios has seen are kept in the vault (documents
`storage/volumes`): each scan updates what it saw and when, and a volume
not mounted stays known, offline, never dropped. The marker file is read
whenever present and written only when the user asks for it, never by a
scan.

Each location (a copy of a photo) names the **volume** it is on and its
path inside it, beside its absolute path. A background scan, at start,
when a volume is plugged in or out and every five minutes, records the
volumes and puts each location on its volume (one recorded before
volumes existed keeps its absolute path until then). The path inside the
volume is what counts: a disk mounted at another place, or given another
drive letter, still finds every photo on it.

## Missing files

A location is **present** (its file is there), **offline** (its volume
is not plugged in, or is an internal disk of another computer sharing
the vault) or **missing** (its volume is online, or it is on none, and
the file is not there). A photo is present when one copy is, offline
when none is but one is offline, missing otherwise. An offline photo is
not missing: its marks, tags and edits stay, and nothing is removed or
relinked because a disk is unplugged; plugging it in makes it present
again. `repair` (relinking moved files by content) checks present copies
only and never looks for an offline photo elsewhere.

In the Library, an offline photo's thumbnail carries an **offline**
badge; hovering it says "On Backup-2, last seen 2026-10-28 on studio".
A missing photo carries a **missing** badge. Opening an offline photo
(or the darkroom reopening one at start) says "Plug in Backup-2 to open
this photo", with the day and the computer it was last seen on, instead
of a bare "no copy present". When a scan changes where copies are (a disk
plugged in or out), the Library reads its photos again, so the badges
follow at once.

Reading the Library never touches the files. It goes through a cursor on
the vault, 2,000 photos at a time, each window one query that brings the
photos' marks and their copies together: the presence and the volume to
plug in are what the scans stored, the file's size and date those its
copy was last seen with. Once shown, the Library is brought up to date
by the cursor's changes: after an import, a scan or an undo it reads only
the photos added and those whose copies moved, and nothing at all when
the library did not change. At 100,000 photos the read is a few dozen
queries, never one per photo (see [performance.md](performance.md)).

### The Storage page

File > Storage… (`library.storage`) opens the Settings app at Storage ›
Volumes: a table of every volume the vault knows, those online first,
then the offline ones (`library.volumes`). Each row shows:

- its **name**, edited in place (Enter or leaving the field saves it,
  `library.volumeSet`);
- its state and kind, and a bar of its space used with its free space;
- its **roles** as toggles: Library, Archive, Backup, Ingest, Scratch
  (`library.volumeRole`);
- **Off site** and **RAID**, the user's flags (`library.volumeSet`);
- how many copies of photos the library holds on it, and whether the
  backup strategy copies to it;
- for an offline volume when and on which computer it was last seen, for
  an internal disk the computer it belongs to.

Every change is saved to the vault at once and kept by later scans. A
volume known only by its label and size (a weak identity), plugged in,
offers **Mark this disk…**: a confirmation explains what will be written,
and only once confirmed is the identity file `.helios/volume.json`
written at its root (`library.markVolume`). A card is never offered it:
Helios never writes to a card. "Scan now" scans it; "Forget…" asks
before dropping what the image map knows of its files not in the
Library. `library.storage` takes `{"volume": id}` to mark that row and
scroll to it, as the health check's free-space fix does.

## Backups

The library has one backup **strategy**, or none (a new library's
default). It is kept in the vault (documents `storage/strategies`, key
`library`) and set in the Settings app at Backup › Strategy, each part
saved at once (`library.setBackupStrategy`): how many verified **copies** each
photo needs, the **destinations** (any backup disk, any archive disk, or
one disk), whether one copy must be **off site**, and its **triggers**:
after each import, when a destination disk is plugged in, and by hand
("Back up now", `library.backupNow`). Per-shoot, per-import and per-card
strategies come later.

**How copies count** (`helios_storage::strategy::status`, giving `have`,
`want`, `off_site_ok` and `unverified`): a photo's backup copies are its
locations on volumes with the backup or archive role, present or offline,
whose copy was read back and matched by hash (recorded in
`storage/verified`). They count once per physical drive: two partitions
of one disk, or a RAID, are one copy. A volume known only by its label
and size never counts, and the library copy is not a backup copy.

**The backup job** (`helios_library::backup`, kind `backup`) copies each
photo short of its strategy from its library copy (never from a card) to
`Helios Backup/` on the destination, under the photo's path in the
library. Each copy is written to a hidden part file with its hash computed
while copying, read back and checked, then given its name; a name taken
by another file gets `-1`, `-2`..., nothing is overwritten, and a file
there already holding the photo is verified and kept. The copy is recorded
as a location (the disk, its path inside it, present, verified now).
Helios writes only to disks with a backup, archive or scratch role, never
to a card or an ingest disk, and only inside `Helios Backup/` or
`Helios Archive/`. A destination unplugged mid-run pauses the job ("Plug
in BACKUP_A to go on") until it is back, at any mount point; the job can
be cancelled, its copy under way removed. One run goes at a time.

**Safe to format**: with a strategy, a card is safe to format only when
every photo on it is in the Library and has its verified backup copies;
otherwise the import says "EOS_DIGITAL: in the Library, 3 photos short of
their backups: keep the card". Without one, the card is "safe to format (1
copy, in the Library: no backup set up)". A card left short is checked
again each time a backup job ends (`helios_library::import::card_safety`,
from the card's files and their images' locations), while it is still
inserted: once its photos have their backups, a toast says "EOS_DIGITAL is
safe to format: every photo has its backups".

**The import window's strategy line**, under the choices, says what
backups the photos will get: "2 copies: Backup disk (plugged in), NAS (not
seen for 3 days)", each disk the strategy copies to with whether it is
plugged in; without a strategy, "No backup set up:" and a button opening
the Storage page.

**Per photo**: with a strategy, each thumbnail of the Library has a badge
for its backups: "backed up" (its verified copies meet the strategy),
"backup short" (fewer verified copies than asked, or none off site when
one must be), "unverified" (copies on backup disks not checked by hash
yet) or "no backup". They are read once for the whole Library each time it
is filled (`helios_storage::strategy::statuses`). The search field takes
`backup:met`, `backup:short` (short, unverified or with no backup),
`backup:unverified` and `backup:none` (no backup copy at all, strategy or
not), run in the vault's query (see "Search").

### Catalogue snapshots

The catalogue (the vault database: ratings, tags, edits, collections) is
not beside the photos, so Helios keeps **snapshots** of it
(`helios_vault_sqlite::snapshot`, `helios_gui::catalogue`). A snapshot is
a copy made through SQLite's backup API from a connection of its own, in
one read transaction: consistent while Helios writes. It is written under
a part name, checked (it opens read-only, has a schema version and passes
`PRAGMA quick_check`), synced, then renamed: a snapshot that exists was
verified. They live next to the catalogue, in `<file>.snapshots/`, named
`<sequence>-v<schema>-<reason>.db`.

- **When:** every `catalogue.snapshotEvery` hours (daily by default; a
  thread looks every ten minutes), before a schema migration (the vault
  does it), and by hand ("Snapshot now" in the Settings app at Storage ›
  Catalogue,
  `library.catalogue.snapshot`).
- **Where:** next to the catalogue, and copied to each disk of the backup
  strategy plugged in, under `Helios Backup/catalogue/<library>/` (the
  catalogue file's stem). A copy is written to a part file, read back and
  checked like the snapshot, then named; one already there and whole is
  kept.
- **Retention:** `catalogue.snapshotsKept` (five by default) in each
  place, the oldest removed only once a new one is in place, never the
  last one.
- **Restore:** the Settings app's Storage › Catalogue lists the snapshots
  next to the catalogue and on the disks plugged in
  (`library.catalogue.snapshots`); "Restore…" asks first, then
  `library.catalogue.restore` checks the snapshot, asks the vault to put
  it in place and closes Helios, which starts again. At the opening, the
  snapshot is checked again, copied next to the catalogue and checked
  once more; the current catalogue (with its log) is kept aside as
  `<file>.before-restore-<time>`, then the copy takes its name. A step
  that fails leaves the catalogue as it was.
- **Health:** `catalogue.snapshots` reports the newest one, here or on a
  disk plugged in, read back at each run: none, or one that no longer
  reads back, is a problem; older than seven days, a warning.

## Not yet

- Phones over MTP as a source (mount them as folders).
- An interactive answer to `--duplicates ask` on the command line (the
  files are listed; the desktop asks, in its Duplicates panel).
- The comparison draws thumbnails at their size (256 pixels).
- Burst identifiers from maker notes; capture times of pictures (JPEG,
  TIFF) from their EXIF, which fall back to the file's date.
- Cancelling an import halfway (the job runs to its end).
- Ctrl+V with files or an image only on the system clipboard (no text
  beside them, as Windows' Explorer and browsers copy): egui-winit
  reads text only on the key and drops it when there is none, so
  nothing reaches Helios; Edit › Paste from other applications does it.
  Copy files gives the files only, no text beside them, and egui-pop's
  own copy still writes text only: both wait for egui-pop's system
  clipboard backend, which writes several formats at once. Collections,
  projects and stacks are not paste targets yet.
