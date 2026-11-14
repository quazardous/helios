# Import from slow or huge sources: the list first, import at once

Status: design study (2026-11-14), **not decided**: the slices at the end
wait for the owner.

The request (2026-11-14): some sources take long to load, Google Photos
above all, and some imports are huge (a card or a folder of 20,000 raws).
The middle zone of the import window then shows the list without
thumbnails, and the person can start the import without waiting for the
listing or the thumbnails to end.

This note builds on [import-window.md](import-window.md) (the window as
built), [import-connectors.md](import-connectors.md) (the connector
contract, sections 2.2 to 2.7), [google-photos-source.md](google-photos-source.md)
and [../performance.md](../performance.md) (the thumbnail measurements).
The window it applies to has three zones: the sources on the left (one
selected at a time), that source's photos in the middle, its detail and
choices on the right.

## In short

- **Where the time goes** is different per source, but the window waits
  for the same two things today: the **whole listing** before the first
  item (a folder or card is walked in full before page one; a Google pick
  is listed in full before anything is shown or imported), and an import
  that takes **the list as a value**, so it can only start once the
  listing is over and only imports what was listed.
- **Measured on local files**: listing is cheap (20,000 files in 84 ms,
  the first page after 15 ms), a thumbnail is not (40 to 60 ms of CPU a
  raw at the median, 120 to 250 ms at p95, about a fifth of the file
  read). For 20,000 raws that is 15 to 20 minutes of thumbnails on one
  thread, about 5 on four: they must stay lazy, and are already (the
  cells in view first).
- **The fixes**, in order of value: (1) the import becomes a job over
  "a source and a rule" (all, new only, a filter), which lists by itself
  and so can start at once and survive the window; (2) the middle zone
  gets explicit states (listing, list view without thumbnails, lazy
  thumbnails); (3) connectors declare their listing shape, thumbnail cost
  and limits so the window picks those states without a special case per
  source; (4) a resumed copy import skips what is already done without
  reading it.

## 1. Where the time goes, per source

### Measured: cards and folders

On the bench box, the raw corpus (38 raws, 19 formats), each file in the
page cache as on a local disk; a synthetic card of empty files named
`IMG_nnnnn.CR3` in folders of 200 for the listing. A run of
`helios-corpus` replaced by a throwaway measurement (not kept).

Box job of 2026-11-14, flagged **not comparable** by the box (CPU
throttled 7.5 % of the run, another job running): the times are
pessimistic, the byte counts exact.

| Measure | Result |
|---|---|
| Listing 2,000 files (folder connector, pages of 500) | first page 3 ms, all in 9 ms |
| Listing 20,000 files | first page 15 ms, all in 84 ms (about 240,000 items a second); the bare directory walk alone 3 ms |
| Glance as the import window reads it (`helios_load::glance_at`, smallest embedded preview, 256 px) | p50 39 ms, p95 245 ms, max 334 ms (37 of 38 raws; one Hasselblad ARW gives none) |
| The library's fast reader (`helios_thumbs::read`, smallest preview of at least 256 px) | p50 58 ms, p95 120 ms (38 of 38) |
| Bytes a preview read takes (`helios_load::sparse::read`) | 120 MB of 591 MB (20 %); p50 13 % a file; read whole (100 %) for CRW, RW2, RWL and an old ARW |

- **Listing is not the cost on a local disk.** The first page waits for
  the whole walk, but the walk of 20,000 files takes milliseconds.
- **A thumbnail is the cost**: 40 to 60 ms of CPU at the median, so 15 to
  20 minutes for 20,000 raws on one thread, 4 to 5 on four. Neither
  reader is much faster for glances: the old one wins at the median (it
  takes the camera's tiny thumbnail when there is one), the fast one at
  p95 (no floating point). Choosing per file is a later tuning, not a
  slice.
- **On a card the bytes matter too**: a fifth of each raw on average, so
  at a USB 3 reader's 90 MB/s about 35 ms of reading per raw, of the same
  order as the decode.

What these numbers do not cover: a real card reader or a network share,
where a directory walk is slower (a FAT directory, a round trip per
folder). The one-shot walk would show there first; it is noted, not
sliced, until a card shows it.

### Cards and folders: the code path

| Step | Where | Shape | Thread |
|---|---|---|---|
| Walk | `helios_connect::local::Local::files` | **one shot**: the whole tree is walked and sorted before the first page; pages are then cut from that list (500 items) | the window's scan thread |
| Items to the grid | `import_scan::Scan` | each page's items sent as they come | scan thread, then the interface |
| "In your Library" | `import_scan::Prefilter` | image map (path, size, time), then size, then partial hash | scan thread |
| Glances | `import_scan` glancers, `helios_load::glance_at` | lazy, the cells in view first, then read ahead while the glance cache has room; RAM and disk cache | 2 to 4 threads |
| Import | `helios_library::import::import` | takes **paths**: walks them again (`prepare`), then copies each file read once, hashed while written, read back | a thread of the desktop, a job of kind `import` |

- **Time to first item = the whole walk.** Milliseconds on a local disk
  (measured above); slower on a card or a network share (not measured).
- **Import during listing imports a snapshot**: "Import N" counts the
  ticked cells, so the files listed after the click are left out.
- **A second run of a copy import** read and staged every file again
  before the duplicate check dropped it (`Run::file`: the shortcut by the
  image map, `seen_before`, applied only to imports in place). Resuming
  a card copy of 20,000 raws after a crash cost the whole card again.
  Done (slice 1): a copy notes the source file it read in the map, and a
  file unchanged since (place, size, time) whose image is in the Library
  is left out without a read. A file changed since, or an image taken out
  of the Library, is read and copied again, as an import in place
  imports it again.

### Google Photos: the code path, and an estimate

Nobody can sign in to a Google account from the bench box, so these are
**estimates** from Google's documentation and the code, not measurements.
The plugin's tests run against a fake Picker on a local port
(`plugins/sources/google-photos/tests/fake_picker`); it times the code
path, not Google, so it was not used for numbers.

| Step | Calls | Estimate |
|---|---|---|
| Picking session | `POST sessions`, then `GET sessions/{id}` every `pollInterval` (a few seconds) until `mediaItemsSet` | the person's own time in Google's page; Helios waits |
| Listing | `GET mediaItems`, **100 items a page** | 2,000 items (the most a picking holds) = 20 requests, a few seconds |
| Thumbnails | one `GET baseUrl=w256-h256` per item | one HTTPS round trip each (about 100 to 300 ms); 2,000 items ≈ 17 min at the plugin's own ceiling of 120 requests a minute |
| Downloads | one `GET baseUrl=d` per item, **one at a time** | phone JPEGs of 2 to 5 MB: a few seconds each, so 2,000 items ≈ 1 to 3 h |
| Download addresses | `baseUrl` lasts 60 minutes | an import longer than that meets expired addresses (handled, below) |
| Access token | an hour | renewed per call (below) |

- **Limits.** The plugin declares 120 requests a minute and 10,000 a day
  (`helios-plugin.toml`), Helios's own ceiling: every install shares the
  OAuth client's project quota. The glance read-ahead stops at it
  (`import_remote::Throttle`); **downloads do not count against it** (the
  fetch loop only obeys a 429 with backoff, at most 5 minutes a wait).
  At 10,000 a day, thumbnails and downloads of one 2,000-item pick fit
  (4,000 requests); five picks a day do not.
- **Listing shape.** Paged (100), small: listing is not the problem.
  The problem is that `pick_and_list` lists every page, then
  `import_remote` takes the whole list: nothing shows before the last
  page, and nothing imports before it either.
- **Thumbnails are a remote fetch**, cheap for Google, costly for the
  quota. Shown only for the cells in view, they cost a screen (30 to 60
  requests) per scroll.
- **Token and addresses.** The plugin keeps no token: each call carries
  the account's access token, renewed by the host when it expired
  (`helios_plugin::source`). An expired download address (403) is
  `Expired`: the adapter lists the picking again for fresh addresses and
  fetches the item once more. Both already hold over a long import.
- **Resume.** No byte ranges (`caps.resume` false): a cut item is fetched
  again from zero. Between items, an item whose id already has an image
  in the Library is skipped before any byte is fetched (its sighting on
  the account's remote volume). A pick, though, lives one session: once
  the window or the app closes, the session's items are gone and the
  person picks again; those already imported are then skipped by id.

### Lightroom and darktable catalogues

The importer plugins read the catalogue's database and stream its images
in pages of 500 (`importer/page`, interface version 2). A catalogue of
50,000 images is read in seconds; the time is in the import of the files
it points to, which is a folder import. Their previews (`previews.lrdata`,
darktable's mipmaps) are not read: the files' own previews are. The
desktop waits for the whole scan before the import starts
(`importers::run`), the same "list, then import" shape.

## 2. Progressive listing

- **Folders and cards** list 20,000 files in under 0.1 s locally: the
  one-shot walk stays. Should a card or a share show a slow walk, the
  walk can send a page every 500 files or 200 ms instead (pages then
  sorted within a folder, which the grid sorts anyway).
- **A Google pick streams** its 100-item pages to the grid as they come.
- **The middle zone fills as items arrive**, with a count and "Still
  listing…"; sorting and grouping by day apply to what is there and
  settle when the listing ends.

## 3. The middle zone's states

| State | When | What it shows |
|---|---|---|
| **Listing** | items arriving | "Listing… 3,412 photos so far", a spinner, Cancel listing; the grid or list below fills |
| **List view** | the connector says thumbnails are costly or unavailable, or the source holds more than a threshold (5,000 by default) | rows of name, date taken, size, kind, "in your Library"; no picture; a switch to the grid stays |
| **Grid with lazy thumbnails** | thumbnails cheap, or the person switched | cells with a placeholder (the file kind and a frame); thumbnails for the cells in view, then the next screen; one scrolled away is dropped from the queue (it is today for local glances, by priority; a remote request in flight finishes) |
| **Ready** | listing complete | the count is final; "New only" and filters exact |
| **Importing** | a job runs for this source | a progress line in the zone (done / total or "so far"), each row or cell marked imported, skipped, failed as its line comes; the window may close |

"Import all" (or "Import new") is enabled **as soon as the source is
selected**: it does not need the listing. "Import N selected" needs the
items ticked, so it is enabled once some are listed, and imports exactly
those.

Before thumbnails, a row shows what every connector knows at listing:
**name** (the decorative path's file name), **date taken** (else
modified), **size** (when `caps.sizes`; Google gives none), **kind**
(raw, JPEG, video, sidecar pair). That is enough to select by day or by
name.

## 4. Starting without waiting

- **The import job takes a rule, not a list.** New request shape:
  `{source, rule}` where the rule is `all`, `new only`, a filter (days,
  kinds), or `items` (an explicit list, as today). The job lists the
  source itself through its connector, page by page, and imports each
  page as it lands. Its total is "so far" until the listing completes.
  The window's grid and the job share nothing but the source: the window
  may list for display while the job lists for import (for Google, the
  pick's pages are cached by the adapter, so the second listing costs no
  request).
- **It runs as the node's job.** Today an import already outlives the
  window (a thread of the desktop, a job in the Jobs panel) but holds its
  list in memory and ends with the app. Run through the node's
  `library.import` command with a rule, it needs nothing from the window,
  a headless node can run it too, and a window opened again on the same
  source shows its progress.
- **Resumable.** The job records each item done (its line, already
  emitted) and its cursor. A job started again on the same source skips:
  for a copy, the files whose sighting (path, size, time) already links
  to an image in the Library, before reading them (today only imports in
  place do that); for a fetched source, the items whose id has an image
  (already so). Dedupe by content hash stays the final word.
- **Partial results stay imported.** Each item is its own commit to the
  vault; a cancelled or failed job keeps what it imported, as today.

## 5. The connector contract: additions

`Caps` gains what the window needs to pick its states, read from the
manifest for plugins:

```rust
pub struct Caps {
    // … as today, and:
    /// How a listing arrives: `Stream` (pages as they are found; the
    /// count is unknown until the end), `Paged` (pages from a cursor,
    /// total known up front when `total` is), `OneShot` (everything at
    /// once, after a wait).
    pub listing: Listing,
    /// What a preview costs: `Embedded` (a read of the file's own small
    /// preview: cheap), `Decode` (a full decode: costly), `Remote` (a
    /// request to the service, counted against `limits`), `None`.
    pub thumbnail_cost: ThumbCost,
    /// It can import a rule without being listed by the window first
    /// (the job lists it itself): every connector whose `list` can run
    /// headless, so all of today's. `false` for a source whose listing
    /// needs the window (none yet).
    pub import_without_listing: bool,
}

pub struct Limits {
    pub per_minute: Option<u32>,
    pub per_day: Option<u32>,
    /// Downloads at once the service tolerates (1 by default).
    pub parallel: Option<u32>,
}
```

| Connector | `listing` | `thumbnail_cost` | `limits` | `import_without_listing` |
|---|---|---|---|---|
| Folder | OneShot (fast locally) | Embedded | none | yes |
| Card | OneShot | Embedded | none | yes |
| Google Photos | Paged | Remote | 120/min, 10,000/day, parallel 2 | yes, within the pick's session |
| Catalogue importers | Paged | Embedded (the files') | none | yes |

The window then decides without naming a connector: `Remote` or `None`
thumbnails, or a large count, start in the list view; `Stream` shows
"so far"; `limits` hold the read-ahead and the downloads to the same
token bucket per account.

## 6. Limits and failures

| Failure | Today | Wanted |
|---|---|---|
| Rate limit (429) mid-import | backoff, `Retry-After` honoured, at most 5 min, item retried | the same, plus the job's line says "Google asks to wait: going on at 14:05"; downloads counted in the per-account bucket with the glances |
| Token expired during a long import | renewed per call by the host | unchanged |
| Download address expired (60 min) | the pick listed again, the item fetched once more | unchanged |
| Refresh refused (access removed) | the account marked "needs attention", the import stops | the job pauses with "Reconnect Google Photos" instead of failing, and goes on after the reconnection (same session if still alive) |
| Daily quota met | 429 retried every 5 min | the job pauses until the next day's window and says so; the window shows the count imported |
| A card pulled out | the job pauses until it is back | unchanged |
| A crash or a quit | the job is gone; what was imported stays | a job started again skips what is done (section 4) |

## 7. Slices

1. **Copy imports skip what the map knows** (done). `Run::file` uses
   `seen_before` for copies too (the source file's sighting links to an
   image in the Library, unchanged): a resumed card copy reads nothing
   already done. The card's file is then a second location of the image;
   every reader of an image's file (export, cull, capture, a library
   view's thumbnails, the darkroom, calibration) picks through
   `helios_vault::ReadOrder`: the library folder's copy first, a card or
   ingest source last.
2. **Import by rule as a node job.** `library.import` takes `{source,
   rule}`; the job lists through the connector and imports page by page;
   "Import all" / "Import new" enabled at once; the window shows the job's
   progress and may close. Local sources first.
3. **Google Photos on the same job.** The pick's pages stream to the grid
   and the job; downloads go through the per-account bucket, two at a
   time; quota and refused-refresh pauses.
4. **Contract and middle-zone states.** `Caps::listing`,
   `thumbnail_cost`, `import_without_listing`, `Limits::parallel`, in the
   manifest; the list view (name, date, size, kind) chosen from them and
   from the count.
