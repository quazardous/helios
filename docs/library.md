# The library: import, duplicates and stacks

The vault keeps images by content (an image *is* the BLAKE3 hash of its
file) and where their copies are as data (see [persistence.md](persistence.md)
and the `helios-vault` crate). The library operations of `helios-library`
work on top of it: they bring files in, measure what makes two photos
near duplicates, and group photos into stacks. They run the same from the
command line (`helios import`, `helios stacks`, see [cli.md](cli.md)) and
from the desktop (the `library.*` commands).

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
  `YYYY/YYYY-MM-DD/<name>` by capture date (`undated/` without one). The
  library folder is given once with `--into DIR` and remembered in the
  vault. A copy is hashed as it is written and takes its name only when it
  holds the same content as the source; until then it is a hidden
  `.<name>.helios-part`. Nothing is ever overwritten: a name taken by
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
over. To keep the darktable edits as versions, run `helios
import-darktable` on the library folder afterwards.

**One job.** An import reports as a job (see [jobs.md](jobs.md)), so the
desktop shows it in the jobs centre: `job.started` (kind `import`, id
`import-<n>`, its title and the number of files), one `job.progress` per
file (`done` of `total`, the file at hand as its `item`, the files failed
and skipped so far), a `job.log` line per file brought in, a
`job.item_failed` per file that failed, then `job.finished`, or
`job.failed` when it cannot start (a missing source, no library folder for
a copy). A file that fails does not stop the others.

**Cancel.** The jobs centre's Cancel (`job.cancel` with the job's id), or
Ctrl-C for `helios import`, stops the import before its next file. A copy
under way stops too, and its hidden `.helios-part` is removed: no
half-written file is left. The files done stay imported; the job ends
`job.cancelled` and the report says `"cancelled": true`.

The CLI prints one JSON line per file (`path`, `id`, `status`, and
`duplicate`, `to`, `sidecars`, `signals`, `error` when they apply; a file
stopped by a cancel says `cancelled`), then `{"report": ...}`; it exits
with 1 when a file failed, 130 when interrupted.

## Exact duplicates

The content hash of the whole file is computed for every file (streamed,
never held in memory). A file is an exact duplicate when the library already
has its content (`"duplicate": {"of": "library", "at": [...]}`), or when an
earlier file of the same import had it (`{"of": "source", "first": ...}`).
`--duplicates` says what to do:

| Policy | What happens |
|--------|--------------|
| `skip` (default) | left out: status `skipped` |
| `keep` | imported as one more copy of the same image: status `located` (a copy is made with `--copy`) |
| `ask` | left out and listed (status `asked`), for whoever started the import to answer, by importing those files again with `skip` or `keep` |

Nothing is ever deleted, whatever the policy. A file that changed (same
path, other content) is a new image.

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
| `library.import` | `path`; `copy`, `into`, `rename`, `duplicates` as on the command line. Runs in the background as a job, in the jobs centre and cancelled from it; the library is filled again when it ends |
| `library.stacks.suggest` | `images` (ids; every image when absent), `resolver` |
| `library.stacks.accept` | `decision`, `groups` (all when absent) |
| `library.stacks.decline` | `decision` |
| `library.stacks.list` | |
| `library.stacks.dissolve` | `stack` |

## Culling on the desktop

The library panels are egui-pop's photo library widgets (its
`docs/library.md`) over the vault:

- **Vault**: the search field, the marks of the image under the cursor
  (stars, colour labels, pick or reject, tags) and a virtualised thumbnail
  grid. Thumbnails are the raws' embedded previews (a picture itself),
  read in the background and shown as they arrive.
- **Filmstrip**: the same images in one row, sharing the grid's
  selection.
- **Compare** (closed by default): the two to four images selected side
  by side, zoom and pan synced; mark one best, then **Keep the best**
  picks it and rejects the others.

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

**Search.** The search field's rating, labels, flags and tags become a
vault query (several labels: any of them; several tags: all of them);
its words (in the name or a tag) and dates (the file's day) are matched
on what the library holds. A filter shows stacks flattened.

## Not yet

- Phones over MTP as a source (mount them as folders).
- An interactive answer to `--duplicates ask` on the command line (the
  files are listed; the desktop asks, in its Duplicates panel).
- Thumbnails are read in the library's order, not the ones in view
  first; the comparison draws them at their size (256 pixels).
- Burst identifiers from maker notes; capture times of pictures (JPEG,
  TIFF) from their EXIF, which fall back to the file's date.
- Cancelling an import halfway (the job runs to its end).
