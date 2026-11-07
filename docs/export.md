# Exporting photos

Status: **built** (2026-11-07): batch exports run on the desktop as
local jobs. Sending the same request to a node as a workflow run is
**not decided** (see "Remote later").

An export writes library photos to files: each one rendered as the
darkroom shows it, encoded in the format and colour space asked for, and
written atomically (see `helios-export`). One photo or a thousand, it is a
**batch**: a parent job and an export per photo.

## Asking for one

`library.export` asks for an export. It is in **File > Export…**
(`Ctrl+Shift+E`, as Lightroom's; `Ctrl+E` in the *darktable classic*
keymap package), in the Library's context menu (a secondary click on the
grid) and in the palette.

Which photos: `images` (library ids) when given, else `scope`:
`selected` (the selection, else the cursor's photo) or `shown` (every
photo the library shows: its search filter, the workspace's query). By
default the selection, else what the library shows.

Without a `folder` (or with `ask`), the export dialog asks, starting from
the workspace's export settings (`library.export` in its template, see
[templates.md](templates.md)) or the defaults:

| Field | What | Default |
|---|---|---|
| `scope` | `selected` or `shown` (not asked when `images` are given) | the selection, else `shown` |
| `format` | `jpeg`, `png` or `tiff` (16-bit) | `jpeg` |
| `quality` | JPEG's quality, 1 to 100 | 92 |
| `bits` | PNG's bits per channel, 8 or 16 | 8 |
| `long_edge` | the longer side at most, in pixels; 0 keeps the full size; never enlarged | 0 |
| `space` | `srgb`, `display-p3`, `adobe-rgb` or `rec2020`, tagged in the file | `srgb` |
| `folder` | where the files go, made when missing; `~/` is the home folder | asked |
| `pattern` | the file names, as an import renames: `{stem}`, `{date}`, `{time}`, `{year}`, `{month}`, `{day}`, `{camera}`, `{maker}`, `{frame}`, `{seq}`, `{seq:N}`; the extension follows the format | `{stem}` |
| `clash` | a name taken: `number` (`name-2.jpg`), `overwrite`, or `skip` the photo | `number` |
| `remember` | make these the workspace's export settings (one undo step) | off |

The version exported is each photo's **head** when the export is asked
for: its newest edit, as the darkroom opens it. Later edits do not change
an export under way.

## How it runs

1. **Planned** over the library (`helios_library::export::plan`): each
   photo's first copy present, its head version, and its output path,
   decided now and never again against the files on disk. The pattern's
   fields come from what the import measured of the photo (its capture
   date, camera, frame number) and its place in the batch (`{seq}`).
   Two photos never get one name; a name taken on disk is numbered,
   replaced or left out as `clash` says. A photo with no copy present is
   refused in the request (a failure); one left out by `skip` is counted
   as skipped. The result is a **batch request** (below).
2. **Run** on the desktop's own scheduler (`helios_export::batch::run`,
   driven by `helios_gui::export::Exporter`): every export is queued at
   once, and a quarter of the cores (one to four) render side by side,
   as a raw's render holds a lot of memory.
3. **Rendered** as the darkroom shows the photo: its file through the
   workspace's network, the version's edit applied by node name (a part
   of it this network has no place for is left out), the network's view
   node rendered whole, then scaled down to `long_edge`, converted to the
   colour space and encoded.

What people see:

- **The jobs centre**: a parent "Export N photos" (kind `batch`) with an
  export per photo under it, each with its render, encode and write
  steps. The parent counts the photos done, failed and skipped.
- **The Queue panel** of the node exported (the workspace's view node,
  selected for it when no node is): an item per photo, waiting, running,
  done, failed or cancelled.
- **Cancel**: on the parent, every export not ended is cancelled, queued
  or rendering, writing nothing; on one export, only that photo.
- **Failures**: each photo that fails is listed under the parent with
  its reason, and in the Log panel ("a.cr3: not exported: …"). The batch
  then ends failed ("1 of 12 photos failed"), the other files written.
- **The end**: a toast with the counts and **Open folder**
  (`library.openFolder`, the folder in the file manager), also on the
  finished parent in the jobs centre. Closing the window cancels the
  batches still running.

## The batch request

A batch is plain data, `helios_export::batch::BatchRequest`, as JSON:

```json
{
  "format": 1,
  "node": "sigmoid",
  "settings": {
    "output": { "format": "jpeg", "quality": 92, "space": "srgb", "long_edge": 2048 },
    "folder": "/home/me/Pictures/Helios export",
    "pattern": "{stem}",
    "clash": "number"
  },
  "items": [
    { "image": "5f0c…(64 hex digits)", "version": "9a1e…",
      "source": "/photos/2026/IMG_0042.CR3",
      "output": "/home/me/Pictures/Helios export/IMG_0042.jpg" },
    { "image": "77d2…", "source": "/photos/2026/IMG_0043.CR3",
      "output": "/home/me/Pictures/Helios export/IMG_0043.jpg",
      "skip": "its file exists" }
  ]
}
```

- `format` is the request's own format: a Helios reading a newer one
  refuses it, never guesses.
- `node` names the graph node rendered, for the queues.
- Each item says everything its export needs: the image (library id),
  the version (absent: never edited), where it is read and where it is
  written. `skip` and `error` mark the items left out or refused when
  the request was made.

The local runner takes the request and a way to render an item
(`batch::Render`); nothing in it depends on the desktop.

## Remote later

Not decided (2026-11-07): sending a desktop's export to a node, so a
long batch runs on another machine or outlives the window. The request is
shaped so that the same description could become a **workflow run**
(see [workflow-level.md](workflow-level.md), decided 2026-10-22), without
the desktop building it twice:

- **The run's export settings** are the request's `settings`; the node's
  `ExportSettings` (folder, format, quality, name pattern) would take the
  colour space and the size, or be replaced by these.
- **The run's items** are the request's items: an image of the library is
  named by its vault id, its output path decided at submission (as the
  workflow already requires, so a resumed export writes the file it would
  have written), its version carried as the item's edit.
- **The network snapshot** a run pins is the workspace's network: the
  desktop would send it with the request (the run's id is the hash of the
  network snapshot and the settings), and the node render each item as
  `helios_gui::export::render_item` does: the file through the network,
  the version's edit applied.
- **Following it**: the run is a job of kind `batch` with an export per
  image (`run-<n>`); the desktop's relay already shows a node's batch as
  a parent with its exports as children (`node:<name>/<id>`), in the
  jobs centre and the Queue panel, and sends Cancel to the node.

Open questions for that step: whether the node reads the files where the
desktop sees them (a shared library) or receives them; whether the
versions travel as their edits or the node reads the workspace's vault;
and which node a desktop sends to by default.
