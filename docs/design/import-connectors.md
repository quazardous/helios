# Import connectors: sources beyond folders and cards

Status: design study (2026-11-11), **not decided**. Documentation only;
nothing here is built. Choices for the owner are in section 6, each
with options A, B, C and a recommendation.

The request (2026-11-11):

- import takes its input from several **connectors**, through a
  connector-provider notion: a local folder, Google Photos, others;
- a connector gives each item a **path** (mostly decorative), its
  **metadata**, the **hash** it claims for the image (checked again once
  the bytes are here), and anything else the import needs;
- an import can be **saved as a setting**, and a saved import can run
  **automatically**; it can carry a filter profile and tags to add;
- a **quick import** stays possible without saving anything, or from the
  **history** of past imports.

This note builds on [import-window.md](import-window.md) (the window as
built), [../import-research.md](../import-research.md) (sections 3 and
4: the facets and the pluggable pipeline, whose `import.source` sketch
this note turns into a contract), [../extensions.md](../extensions.md)
(plugins out of process, the importer protocol),
[windows.md](windows.md) (the Import focus is its slice 4),
[library.md](library.md) (choices L-C1 to L-C9), [sync.md](sync.md) (its
`SyncTransport` is a sibling abstraction) and [cache.md](cache.md).

## In short

- A **connector** lists a source page by page, gives each item a stable
  id, a decorative path, metadata, a size and a claimed hash when it has
  them, and fetches the bytes. Helios, never the connector, computes the
  BLAKE3 identity and checks the claim while the bytes land.
- **Folders and cards** become the first two connectors, in process,
  behind the same trait; everything that needs a network, a USB protocol
  or an account (Google Photos, phones over MTP, cameras over PTP, S3,
  SMB) is an **out-of-process plugin** under `plugins/sources/<name>`,
  speaking an `import.source` service.
- A remote item becomes a vault image only through a **verified local
  copy** in the library folder; the remote place is recorded as a
  sighting on a volume of kind `remote`, never as a backup.
- A **saved import** (source, choices, filter, tags, after-import steps,
  triggers) lives in the vault's documents; today's presets are saved
  imports without a source. Every import, saved or not, leaves a
  **history record**, with "Import again".
- **Google Photos today:** since 31 March 2025 a third-party application
  can read a user's library only through the **Picker API**: the user
  picks up to 2,000 items per session in Google's own page, the
  application downloads them for 60 minutes. Downloads strip location,
  are not guaranteed to be the uploaded bytes, carry no hash and no
  size. No automatic import, no change feed. Google Takeout remains the
  only route to the originals.

## 1. What exists today

### Where the code is

| Part | Files | What it does |
|---|---|---|
| The import window | `crates/helios-gui/src/shell/import_window.rs` | Sources column (cards, recent folders, favourites, "Choose a folder…"), Contents, footer with presets; opened by File > Import…, the empty Library, a card toast, a drop or a paste (`library.importWindow`) |
| Its choices | `crates/helios-gui/src/shell/import_choices.rs` | copy or add in place, Into, layout, Name, Duplicates, darktable edits; `Choices::args` becomes `library.importFiles`'s arguments; last used per library, presets in `import-presets.json` beside it |
| The file browser source | `crates/helios-gui/src/import_browser.rs` | `ImportSource`: an egui-pop `FileSource` over the ticked sources as scans find them, an offline volume's as the image map recorded it; `ImportThumbnails` reads glances, else the library's thumbnail cache |
| Scans and glances | `crates/helios-gui/src/import_scan.rs` | `Scan` walks a card from `DCIM` or a folder with its subfolders (`IMAGE_EXTENSIONS`), sends each file, then each one's glance (`helios_load::glance_at`: the smallest embedded preview and the capture time, never a raw decode), visible cells first; `Prefilter` ("in your Library": image map, then size, then partial hash); `Contents` (RAW+JPEG pairs as one cell); `Recorded` (an offline folder as last seen) |
| The glance cache | `import_scan::GlanceCache`, cache kind `import.glance` | RAM, keyed by path + size + mtime, 192 MiB LRU ([cache.md](cache.md), row 13) |
| Desktop import glue | `crates/helios-gui/src/shell/import.rs` | the workspace's import source, the "where to" dialog, the job's lines to the Duplicates panel, Log and toasts |
| Importer plugins on the desktop | `crates/helios-gui/src/importers.rs` | File > Import from →: `importer.scan` as a job, then the library import with the plugin's values |
| The import operation | `crates/helios-library/src/import.rs` | `Options` (placement, rename, layout, duplicates, signals, darktable, listed values, cancel); single-read copy hashed while written into `.helios-part/`, read back, never overwrite; sidecars; pause on a lost source; `Card` and `card_safety` ("safe to format") |
| Rename patterns | `crates/helios-library/src/rename.rs` | `{date}` `{time}` `{camera}` `{frame}` `{stem}` `{seq}`… |
| Exact duplicates | `import.rs` (`Duplicates`), `crates/helios-gui/src/duplicates.rs` | skip (default), keep, ask; the Duplicates panel answers `ask` file by file; nothing deleted |
| Content identity | `crates/helios-vault/src/lib.rs` (`ContentHash`, `partial_hash`) | an image is the BLAKE3 hash of one file; the partial hash is BLAKE3 of the first and last 64 KiB plus the size, a prefilter only |
| Sightings, Library flag | `crates/helios-vault/src/sightings.rs` | one file at one place (volume + path, size, mtime, partial hash, image once fully hashed, capture time, camera, `seen_by`); a location is a sighting whose image is known; "in the Library" is a flag set by an import, one undo step |
| Recording the map | `crates/helios-library/src/map.rs` | `Recorder`: what a listing found becomes sightings, in batches, never reading a file for the map alone |
| The map job | `crates/helios-library/src/map_job.rs` | low-priority job `map`: on-screen partial hashes, scans of plugged volumes, full hashes of Library sightings, duplicates to confirm; pauses during imports, on battery |
| Volumes and roles | `crates/helios-storage/src/lib.rs`, `detect.rs`, `registry.rs`, `scan.rs`, `marker.rs`, `exclude.rs` | `Volume` with `VolumeKind` and `Role` (library, archive, backup, ingest, scratch); Linux detection from `/proc/mounts`, `/dev/disk`, `/sys`; `Watch` polls once a second and tells listeners when a card appears; registry in the vault (`storage/volumes`); the map's blacklist |
| Backup strategy | `crates/helios-storage/src/strategy.rs`, `crates/helios-library/src/backup.rs` | one library strategy (`storage/strategies`), copies on distinct drives, `Trigger` (on import, on connect, by hand), `may_write` only on backup, archive or scratch volumes; the backup job copies from the library copy, verified |
| Health | `crates/helios-health/src/backup.rs` | strategy set and met, destinations seen lately, copies verified, cards safe to format |
| darktable library importer | `plugins/importers/darktable-library/`, `crates/helios-plugin/src/importer.rs` | importer interface version 2: paged `importer/page`, edits per image; bundled with Helios (`helios_plugin::bundled`) |
| Plugins | `crates/helios-plugin/src/{broker,rpc,manifest,importer}.rs` | JSON-RPC 2.0 over stdio, manifest checked before anything runs, activation events, crash isolation, brokered capabilities (`images.read`, `source.read`, `folder.write` into a staging folder) and ambient ones granted by the user (`process.spawn`, `device.usb`, `network`, `secrets`…) |
| A device plugin | `plugins/examples/gphoto2-camera/` | the `device` service (`device.list`, `info`, `capture`, `download` into staging), wrapping the `gphoto2` program; tests with a fake `gphoto2` |

### What the pieces already decide

- **Identity is content.** BLAKE3 of the whole file, computed while the
  copy is written; the copy takes its name only once read back and equal.
  A connector's claim can only ever be a hint checked against this.
- **Import is a flag** on images the vault may already know: "in your
  Library" and duplicates are lookups on the flag, so a source whose
  items were seen but not imported costs nothing to list again.
- **Placement** is reference or copy. Copy is forced for a card unless the
  person switches; a referenced file stays where it is.
- **Sightings are per place**, and a place is a volume plus a path. An
  offline volume's listing is the map's record of it.
- **Safe to format** is computed by hash, for cards only, at the end of a
  copy and again after each backup job.
- **Memory.** The last choices per library (a JSON file), presets beside
  them (`import-presets.json`), recent folders, favourites and the last
  source (another JSON file); a workspace's import source is part of its
  template (a `folder` source, duplicates skipped). None of this is in
  the vault, so none of it travels with the library or with sync.
- **History.** Each import is one undo step of the Library (`library`
  scope), for the session only. There is no persistent record of past
  imports beyond the job log and the audit trail's `library_by:
  import-<n>`.
- **Plugins.** Two shapes exist: `importer` (paths plus values, Helios
  imports the files) and `device` (list, download into a staging folder
  the broker owns). The research note sketched `import.source`
  (`source.detect`, `list`, `thumbnail`, `fetch`) without building it.
- **Automatic anything.** Only the backup job has triggers. No watched
  folder, no schedule, no automatic import.

## 2. The connector model

### 2.1 Vocabulary

| Word | Meaning | Example |
|---|---|---|
| **Connector** (provider) | code that knows one kind of source | folders, cards, Google Photos, MTP phones |
| **Account** | one identity on a connector, when it has any | `me@example.org` on Google Photos; none for a folder |
| **Source** | one place a connector lists, under an account | a folder, a card, a phone, a Picker session, a bucket prefix |
| **Item** | one file the source holds, with a stable id within the source | `DCIM/100CANON/IMG_0412.CR3`, a Google media item id |
| **Claim** | the hash a connector says an item has, in some algorithm | MD5, SHA-1, Dropbox content hash, a peer's BLAKE3 |
| **Staging** | the part file the bytes land in before they are an image | `.helios-part/` in the library folder |

The interface keeps its words: the window's column stays **Sources**;
accounts appear in Settings > Accounts and under their connector in the
Sources column. "Connector" is a developer word.

### 2.2 The trait

In-process connectors implement it directly; a plugin connector is one
adapter (`PluginConnector`) that implements it over JSON-RPC (2.5).
Sketch, not final names:

```rust
/// One kind of source.
pub trait Connector: Send + Sync {
    /// Id, label, and what it can do.
    fn describe(&self) -> ConnectorInfo;

    /// The sources present now under an account (cards plugged in, phones
    /// connected, a Picker session ready), for the Sources column. A
    /// connector without detection returns the sources it was given.
    fn sources(&self, account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError>;

    /// One page of a source's items, from the start (`from: None`) or
    /// after a cursor. The cursor is opaque, kept by Helios between runs.
    fn list(&self, source: &SourceRef, from: Option<&Cursor>, page: usize, cancel: &Cancel)
        -> Result<Page, ConnectorError>;

    /// What changed since a cursor, when `caps.changes` (a change feed).
    fn changes(&self, source: &SourceRef, since: &Cursor, cancel: &Cancel)
        -> Result<Page, ConnectorError> { Err(ConnectorError::Unsupported) }

    /// A small preview without fetching the file, when `caps.thumbnails`.
    fn thumbnail(&self, item: &ItemRef, max_side: u32) -> Result<Option<Vec<u8>>, ConnectorError>;

    /// How Helios reads the bytes: in place (a mounted folder) or fetched.
    fn access(&self, item: &ItemRef) -> Access;

    /// Writes the item's bytes into `sink`, from byte `offset` when the
    /// connector can resume (`caps.resume`). Helios owns the sink: it
    /// hashes as bytes arrive, in BLAKE3 and in the claim's algorithm.
    fn fetch(&self, item: &ItemRef, offset: u64, sink: &mut dyn FetchSink, cancel: &Cancel)
        -> Result<Fetched, ConnectorError>;
}

pub struct ConnectorInfo {
    pub id: String,                 // "folder", "card", "google-photos", "mtp"
    pub label: String,              // translated
    pub caps: Caps,
}

pub struct Caps {
    pub accounts: bool,             // needs sign-in (2.4)
    pub detect: bool,               // sources come and go (cards, phones)
    pub claims: Option<HashAlgo>,   // the hash it gives, if any
    pub sizes: bool,                // sizes known before fetch
    pub thumbnails: bool,
    pub resume: bool,               // fetch from an offset
    pub changes: bool,              // a change feed (2.3)
    pub originals: Originals,       // Exact, or Rendered (bytes may differ from the upload)
}

pub struct Page { pub items: Vec<Item>, pub next: Option<Cursor>, pub complete: bool }

pub struct Item {
    pub id: ItemId,                 // stable within the source; the resume and dedup key
    pub path: String,               // decorative: shown, used by {stem} and layouts, never trusted
    pub size: Option<u64>,
    pub modified: Option<Timestamp>,
    pub taken: Option<Timestamp>,   // capture time when the source knows it
    pub kind: ItemKind,             // photo, raw, video, sidecar
    pub mime: Option<String>,
    pub metadata: Metadata,         // camera, lens, exposure, dimensions, rating, album names…
    pub claim: Option<Claim>,       // (algorithm, digest) as the source states it
    pub revision: Option<String>,   // changes when the item's bytes change (an ETag)
    pub group: Option<String>,      // RAW+JPEG pairs and sidecars the source knows belong together
}

pub enum Access { InPlace(PathBuf), Fetch }

pub struct Fetched { pub bytes: u64, pub mime: Option<String> }

pub enum ConnectorError {
    Unsupported, NotFound, Gone,                  // the item went away
    Offline, AuthNeeded, Expired,                 // retry after sign-in or a re-list
    RateLimited { retry_after: Option<Duration> },
    Transient(String), Fatal(String),
}
```

Rules the core enforces, whatever the connector:

- **The path is decorative.** It names, it feeds `{stem}` and the
  example in the Choices column, it is shown in the Log. Identity never
  comes from it; a path with `..` or a separator outside the source is
  shown as given and used as a name only after the core sanitises it.
- **The claim is a hint, the BLAKE3 is the truth.** While the bytes
  land, the `FetchSink` computes BLAKE3 (the image id) and, when the item
  claims a hash, that algorithm too. A mismatch fails the item ("Google
  Photos: IMG_0412.jpg: the bytes received are not the ones announced"),
  is retried once from zero, then left in the report as failed; nothing
  half-verified becomes an image. A claim the core cannot compute (an
  unknown algorithm) is recorded and reported as unchecked.
- **Metadata from a connector is the source's opinion.** It fills the
  window (dates, camera, sizes) before any fetch; after the fetch, the
  file's own EXIF wins for every field the file carries, as the importer
  protocol already rules ("applied before the sidecars, never
  overwriting"). Values a file cannot carry (an album name, a Google
  description, a rating from a phone gallery) go in as importer values,
  under `import:<connector>` in the audit trail.
- **Size and time limits.** An item whose fetched size differs from its
  announced size fails like a hash mismatch; a fetch has a deadline
  scaled by size.

### 2.3 Listing, change feeds and the image map

A connector lists **pages** (500 items, as the importer protocol's
version 2) with a **cursor**. Helios keeps the last cursor per source in
the vault, so an incremental source (a change token, a page token, an
S3 `start-after`) is listed from where it stopped; a connector without
one lists from the start and the map makes the second listing cheap.

**Each account is a volume.** A remote source has no mount point, but it
has what a volume is for: an identity, a name, a last-seen date, online
or offline. Helios records each account (or each phone, each bucket) as a
volume of a new kind `remote` (with `VolumeKind::Remote { connector }`),
and each item as a **sighting** on it, its place the item id, its
decorative path kept beside it. That gives, for free:

- **"in your Library"** for remote items, through the same `Prefilter`
  lookup the import window does today;
- the **offline listing** of a source last seen (a phone not plugged in,
  no network), marked "offline, never imported" as for a card;
- the map's forgetting rules and blacklist (a remote folder excluded).

A remote volume has no role by default and may never take the backup or
archive role (`strategy::may_write` refuses it): Helios cannot verify
what the remote holds later, and a re-encoding service never holds the
original (option C3 below).

### 2.4 Accounts and credentials

- **OAuth 2.0 for installed applications**: authorisation code with PKCE
  and a loopback redirect (`http://127.0.0.1:<port>`), the system
  browser opened on the consent page; no embedded web view (Google
  refuses them). Scopes are the narrowest the connector declares.
- **Tokens in the system keyring**, never in the vault or a settings
  file: Secret Service on Linux (through the Secret portal inside a
  Flatpak), Keychain on macOS, Credential Manager on Windows. The
  `keyring` crate (MIT or Apache-2.0) covers the three. The vault keeps
  only the account's public facts (connector, display name, the
  volume id) so a second machine sees the account and asks to sign in
  there.
- **Who holds the refresh token** is an owner choice (C2). Recommended:
  Helios runs the flow from the plugin's manifest (`[[accounts]]`:
  authorisation and token endpoints, scopes, client id), keeps the
  refresh token, and hands the plugin a short-lived access token with
  each call (`params.auth`). The plugin never sees the refresh token, the
  broker can revoke it, and a new capability `account` (brokered) joins
  `source.read` and `folder.write`.
- **Client ids.** A public build needs Helios's own client id for each
  service. Google's photos scopes call for its app verification before
  more than a handful of test users; until then a user may enter their
  own client id in Settings > Accounts (as rclone does).
- **Signing out** deletes the keyring entry and, where the service
  offers it, revokes the token. Sightings stay.

### 2.5 Rate limits, cuts, offline

- **Rate limits.** `RateLimited { retry_after }` pauses the source's
  work, not the job's other sources: the job says "Google Photos asks to
  wait: going on at 14:05" (as a lost card says "Reinsert EOS_DIGITAL").
  The core keeps one token bucket per account, so two jobs on one
  account share the quota, with exponential backoff and jitter when no
  `Retry-After` is given.
- **A cut mid-fetch.** The part file and its offset are kept in
  `.helios-part/`; a connector with `caps.resume` continues from the
  offset (an HTTP `Range`), the sink having kept its hasher state, or else
  starts again. An expired download link (Google's 60 minutes) is
  `Expired`: the core lists the item again for a fresh link, once.
- **A cut between items.** The import job already records each file
  done; it gains the item id per line, so a resumed job (after a crash,
  the journal permitting, see windows.md T9) skips the items whose
  sightings already have an image.
- **Offline.** A lost network is a lost source: the job pauses ("Offline:
  going on when the network is back") and can be cancelled. A source
  listed while offline shows the map's record. Automatic imports wait.

### 2.6 In process or a plugin

| Connector | Where | Why |
|---|---|---|
| Local folder | **core, in process** | today's `Scan`; no capability to grant |
| Card, mounted camera or phone as a disk | **core** | today's `Watch` and `DCIM` rule; safe to format |
| Watched folder | **core** | a trigger on the folder connector (3.2) |
| Network share mounted by the system (SMB, NFS, sshfs, gvfs) | **core**, as a folder | `detect.rs` already knows network volumes |
| Dropbox, OneDrive, Google Drive, iCloud Drive folders | **core**, as folders | the sync clients make them folders (4.2) |
| Google Photos | **plugin** `plugins/sources/google-photos` | network, an account, a service that changes |
| Phones over MTP | **plugin** `plugins/sources/mtp-devices` | libmtp, USB, per-platform quirks |
| Cameras over PTP | **plugin**, the gphoto2 example moved to `plugins/sources/gphoto2-camera` | `device.usb`, `process.spawn` |
| S3-compatible, WebDAV, SFTP, SMB without mounting | **plugin** `plugins/sources/remote-storage` | network, credentials; one plugin over `opendal` |

This follows the plugin direction (official plugins in Rust, out of
process, under `plugins/<category>/<name>`, bundled with Helios as the
darktable-library importer is). In-process connectors give the core a
test bed with no broker, and the plugin adapter is tested against the
same contract suite.

**The `import.source` service** (interface version 1), the trait over
JSON-RPC, completing the research note's sketch:

| Method | Params | Result |
|---|---|---|
| `source.detect` | `connector`, `auth?` | `[{source, label, kind}]` |
| `source.list` | `source`, `cursor?`, `pageSize`, `auth?` | streamed `source/page` notifications `{items, next, complete}`, as `importer/page` |
| `source.changes` | `source`, `since`, `auth?` | as `source.list` |
| `source.thumbnail` | `items`, `maxSide`, `auth?` | a JPEG per item, written into the staging folder |
| `source.fetch` | `items` (with `offset`), `staging`, `auth?` | each file written into staging, `$/progress` per byte count; errors per item with the codes of 2.2 |

The plugin writes into the brokered staging folder (`folder.write`, as
`device.download` does); Helios then reads the staged file once, hashing
it (BLAKE3 and the claim) and moving it into place on the same
filesystem (the staging folder is created inside `.helios-part/`), so a
plugin fetch costs one write and one read, as a card copy does. A
connector that can stream through a pipe can come later.

### 2.7 From an item to an image

```
list ─► item (id, path, size?, claim?, metadata)
          │
          ├─ sighting on the account's volume (the map), "in your Library"?
          │      known item id with an image, or claim alias known  ─► skip (new only)
          │
          ├─ filter profile (3.1) ─► not wanted: listed, not fetched
          │
          ▼
       access: InPlace(path) ─► today's import (reference or copy)
               Fetch        ─► .helios-part/<id>.part, BLAKE3 + claim while writing
                                  ├─ mismatch ─► retry once, then failed
                                  ▼
                               image id known? ─► duplicate policy (skip, keep, ask)
                                  ▼
                               moved to its name in the library folder (layout, pattern)
                               location recorded; sighting on the remote volume linked
                               Library flag set by import-<n>; importer values; tags
```

- **A fetched item always becomes a local copy** in the library folder:
  there is no "reference" for a fetched source (recommended in C3).
  Placement in the Choices column shows "Copy into the Library", fixed,
  with a note on why.
- **Deduplication before download**, cheapest first:
  1. the item id's sighting already has an image in the Library: skip;
  2. the claim matches an **alias**: a new vault table `claims
     (algorithm, digest) → image`, filled each time a fetch checked a
     claim, and when a local file of the Library is hashed in an
     algorithm a connector uses (on demand, by the map job, for MD5 or
     SHA-1 only when a connector of that algorithm is set up);
  3. a claimed BLAKE3 (a Helios peer, a future Helios server) is the
     image id itself;
  4. no claim: a **probable** match by capture time, camera and
     dimensions against Library images, shown "probably in your Library"
     and, with "New only", not fetched. A probable match is never final:
     an item fetched anyway is deduplicated by its BLAKE3 as today.
- **Rendered sources** (`Originals::Rendered`, Google Photos) never
  match a card's original by hash: the same photo is a different file.
  Their items are linked to the Library image by the probable match
  above and imported only when the person asks; an item imported anyway
  is a near-duplicate, offered as a stack with the original
  (`stacks.suggest`).
- **Sidecars and pairs.** An item's `group` brings its sidecars and its
  pair along, as `Contents` pairs RAW and JPEG today.
- **Safe to format** extends from cards to any source with
  `caps.detect` whose items can be deleted by the user (a phone): "Pixel
  8: every photo is in the Library and verified". Helios never deletes
  from a source.

### 2.8 The sibling: sync's transport

`SyncTransport` ([sync.md](sync.md), section 4) moves Helios objects
between replicas that trust each other; a connector moves foreign files
into one replica. They share mechanics, not a trait: the verified part
file, the per-peer rate and resume handling, the keyring for whatever
needs a secret. The shared pieces go in one crate (the fetch sink, the
part-file writer, the backoff), used by both.

## 3. Saved imports, triggers and history

### 3.1 What a saved import holds

```jsonc
{
  "format": 1,
  "id": "card-to-library",
  "name": "Card to the Library",
  "source": {                         // absent: a preset, applied to whatever source is ticked
    "connector": "card",              // folder, card, google-photos, mtp…
    "account": null,
    "match": { "label": "EOS_DIGITAL" }   // or a volume id + path, a phone's serial, a bucket prefix
  },
  "choices": {                        // today's Choices, unchanged
    "copy": true, "into": "library", "layout": "day",
    "rename": "{date}-{seq}", "duplicates": "skip", "darktable": true
  },
  "filter": {                         // the filter profile: which items, before any fetch
    "types": ["raw", "jpeg"], "newOnly": true,
    "taken": { "after": "2026-01-01" },
    "query": "camera:\"EOS R6\" AND NOT name:*_preview*"
  },
  "add": {
    "tags": ["trips|Lyon"],
    "album": null,                    // L-C2 albums, once built
    "template": null                  // metadata template, once built
  },
  "after": { "backup": "strategy", "stacks": "suggest", "select": true },
  "triggers": [ { "on": "plugged" } ],
  "auto": { "enabled": false, "confirmed": null, "askAbove": { "photos": 2000, "bytes": "100 GB" } },
  "machines": ["<node id>"]           // where the triggers fire
}
```

- **The filter profile** is read here as a saved filter over the
  source's items, in the Library's query language (`helios_vault`'s
  `Query`, the search filter's syntax) applied to item metadata before
  any fetch, plus the window's type filter and "New only". Whether the
  owner meant this or the Library's view after import is option C8.
- **Tags** are added as importer values (add-only, under
  `import:<saved id>`), so undoing the import takes them back with it.
- **Backup after import** is the strategy's `OnImport` trigger today;
  a saved import may say "strategy" (default), "now, to this volume", or
  "no". Photo Mechanic's and Rapid Photo Downloader's second
  destination during ingest is a later option of the same field.
- **Rename rules** are today's pattern; nothing new.

### 3.2 Triggers

| Trigger | Mechanism | Notes |
|---|---|---|
| A card or volume plugged | `helios_storage::Watch::on_volumes` (exists) | matched by volume id, else label |
| A device connected | the plugin's `source.detect`, polled while the window or a trigger needs it | phones, cameras |
| A watched folder | the `notify` crate (CC0) on local disks; polling every N minutes on network shares, which raise no events | a file is taken once its size and mtime hold still for a few seconds (a transfer in progress) |
| A schedule | "every N hours", "daily at HH:MM", in the jobs scheduler's clock | missed runs run once at the next start |
| A change feed | `source.changes` polled on the schedule | only connectors with `caps.changes`; **not Google Photos** |

Who runs them: the desktop while it is open (v1); the headless node
later, so a studio machine ingests a hot folder with no window (C7).

### 3.3 Never a surprise

- **First run confirmed.** Turning "automatic" on runs the import once
  with its window open on the source, the filter applied and the counts
  shown; pressing Import confirms the setting (`auto.confirmed` takes
  the date). Changing the source, the destination or the copy mode
  clears the confirmation.
- **Afterwards, quietly, with a summary.** A notice "Card to the
  Library: 312 photos, 4.1 GB, EOS_DIGITAL safe to format" with "Show"
  (the photos selected) and "Undo"; the Log panel has the report; the
  history has the record.
- **Ask above a cap.** More photos or bytes than `askAbove`, a
  destination with less free space than the import plus a margin, or a
  source never seen before: the automatic run becomes a toast "312 new
  photos on EOS_DIGITAL: Import / Open / Not now".
- **One switch to pause them all** (status bar and Settings), and
  automatic imports pause on battery and during a render, as the map
  job does.
- Never delete or modify anything on a source; never format a card.

### 3.4 Quick import and history

- **Quick import is today's window.** Pick a source, press Import;
  nothing is saved but the last choices. The footer's preset menu becomes
  the saved imports menu, with "Save as…" (with or without its source).
- **Every import leaves a record** (`import/history`, one document per
  import job): when, which machine, connector and source (label and
  match), the settings used (a full copy, not a reference), the saved
  import it came from if any, counts (added, located, duplicates,
  failed, bytes), the cards' safety at the end, the cursor reached, and
  the job id that ties it to `library_by: import-<n>` and its undo step.
  The list of files is the audit trail's, not copied.
- **The window shows it.** A "Recent imports" group in the Sources
  column (as Lightroom's recent sources) and Window > Import history:
  each record offers **Import again** (same source and settings, "New
  only" on, which from a cursor means only what came since) and **Save
  as…**. Records are kept a year by default, the last 50 always.

### 3.5 Where they live

The vault's documents (option C4), one namespace each:

| Namespace | Key | Content |
|---|---|---|
| `import/saved` | saved import id | 3.1, with a `format` and `upgrade` as `storage/volumes` does |
| `import/history` | job id | 3.4 |
| `import/cursors` | source key | the last cursor per source, per machine |
| `import/accounts` | account id | connector, display name, its volume id; never a token |

The last used choices may stay a per-machine file, as now. Today's
`import-presets.json` and the workspace import source migrate once into
`import/saved` (a preset becomes a saved import without a source; a
workspace import source becomes a reference to one).

## 4. The connectors surveyed

### 4.1 Google Photos

**What changed in 2025.** On 31 March 2025 Google removed the
`photoslibrary.readonly`, `photoslibrary.sharing` and `photoslibrary`
scopes from the Library API; calls relying only on them return 403. The
Library API now lists, searches and returns only the albums and items
**the calling application created** (`photoslibrary.readonly.appcreateddata`);
uploading (`photoslibrary.appendonly`) stays. Reading a user's library
moves to the **Picker API**.

**The Picker API, as it works today.**

1. Helios creates a session (`photospicker.mediaitems.readonly`); the
   answer has a `pickerUri`, which must be opened in a browser (not an
   iframe; `/autoclose` closes the tab after picking).
2. The user picks items in Google Photos' own page. At most **2,000
   items** per session (`maxItemCount`, default and ceiling).
3. Helios polls the session (`pollingConfig` gives the interval and the
   timeout) until `mediaItemsSet` is true, then lists the picked items
   (paged).
4. Each item gives `id`, `createTime`, `type`, and a `mediaFile` with
   `baseUrl`, `mimeType`, `filename` and metadata (width, height, camera
   make and model, focal length, aperture, ISO, exposure time). **No
   hash, no size.**
5. Bytes: `baseUrl` + `=d` for a photo, `=dv` for a video, with the
   OAuth bearer token in the `Authorization` header. Base URLs last **60
   minutes**; the session and its items expire at `expireTime`.

**What the download is.** Google documents `=d` as keeping the Exif
metadata **except location**. Developers report on Google's issue
tracker that `=d` returns bytes that differ from the upload; whether a
RAW uploaded as original quality comes back as the RAW is not
documented, and has to be checked against a real account before the
plugin claims it. Photos uploaded in "Storage saver" are re-encoded by
Google in any case.

**Quotas.** Picker requests 100,000 per minute per project; media bytes
1,000,000 per minute per project (the Library API's older daily caps,
10,000 requests and 75,000 media-byte loads, do not apply to the
Picker). 429 answers ask for backoff.

**Feasible today:**

- a "Google Photos…" source in the window: sign in once, "Pick
  photos…" opens Google's page, the picked items appear in Contents
  with their thumbnails (`baseUrl=w256-h256`), are fetched and imported
  as copies, marked with Google's location loss;
- deduplication by item id (the same item picked twice) and by probable
  match against the Library (capture time, camera, dimensions);
- "Import again" re-opens a picker; nothing is remembered on Google's
  side.

**Not feasible today:**

- listing or watching the whole library, an automatic or scheduled
  import, a change feed;
- more than 2,000 items without the person picking again;
- originals known byte for byte, GPS, a hash before download;
- albums: the Picker gives items, not the user's albums.

**The other route: Google Takeout.** Takeout exports the library as
archives of the uploaded files with one JSON sidecar per photo (dates,
GPS, description, people), the dates often only in the JSON. It is the
only route to the originals and the location. It is an **importer
plugin** (`plugins/importers/google-takeout`): the user downloads and
unpacks the archives, Helios reads the folder, the JSON fills the values
the files lack. Option C1.

### 4.2 Others, briefly

| Source | How | Licence and crates | Feasible |
|---|---|---|---|
| Camera over USB (PTP) | libgphoto2, or the `gphoto2` program as the example plugin does | libgphoto2 LGPL-2.1+, the `gphoto2` crate (Rust bindings) LGPL-2.1-only: both usable from a GPLv3 program; the program route needs no linking | yes: move the example to `plugins/sources/gphoto2-camera`, `caps.detect`, `thumbnails`, no claim; most cameras also mount as a card |
| Phone (MTP) | libmtp | libmtp LGPL-2.1+; `libmtp-rs` MIT but unmaintained since 2020, so a thin binding of our own or the `mtp-*` programs | yes on Linux (and Windows through WPD later); no hashes, slow listings, one client at a time (the desktop's own MTP mount may hold the device). iPhones over USB speak PTP for the camera roll |
| NAS, SMB | mounted by the system: a folder (exists); unmounted: `smb` crate (MIT) or `opendal` | `opendal` Apache-2.0 (GPLv3-compatible) | yes, mounted first; the plugin when mounting is not wanted |
| S3-compatible | `opendal` or `aws-sdk-s3` (Apache-2.0) | as above | yes: list paged with `start-after`, sizes known, ETag = MD5 for single-part uploads only (a multipart ETag is not a hash), `Range` resume |
| iCloud Photos | no public API; third-party tools reverse-engineer the web service with the account's password and 2FA | n/a | **no** as a connector: the supported routes are iCloud for Windows' download folder, macOS Photos with "Download originals", or a folder a tool such as icloudpd fills; Helios reads those as folders |
| Dropbox, OneDrive, Google Drive | their sync clients make folders | n/a | yes, as folders. Beware online-only placeholders: reading one downloads it; the folder connector should detect them (Windows recall-on-access attributes, macOS File Provider) and treat them as `Fetch` items. Their APIs give hashes (Dropbox content hash, OneDrive SHA-1 or QuickXorHash, Drive MD5) for a plugin later |

### 4.3 Prior art

- **Lightroom Classic.** Import presets save the right panel (file
  handling, renaming, develop settings, metadata preset, keywords,
  destination) but **not the source**; Auto Import watches **one**
  folder, moving files into a destination with a rename, develop
  settings, metadata and keywords; the source list shows devices first.
  Lightroom (cloud) imports from a device in the background.
- **darktable.** "Add to library" (in place) and "copy & import"; a
  folder browser with places and recent folders; metadata and tags set
  in the dialog and remembered; no saved import sets; cameras through
  gphoto2 ("import from camera"), tethering in its own view.
- **digiKam.** A separate Import tool for cameras (gphoto2 or USB mass
  storage), "download new" by its own record of what was downloaded,
  rename, auto-rotate, album per date, an option to delete from the
  camera after download; configured cameras listed by name.
- **Photo Mechanic.** Ingest from several cards at once to a primary and
  a **secondary** destination (backup at ingest), rename, IPTC template,
  **incremental ingest** (skips what it ingested before, by its own
  record), auto-ingest on card insertion, eject when done; settings are
  remembered, not named.
- **Rapid Photo Downloader** (Linux): devices detected and downloaded
  in parallel, backup during download, job codes in names, "new only" by
  its own download history.

What Helios takes: devices first (all), a saved import with its source
(none of them name the source except Photo Mechanic's remembered state),
incremental by **content and item id** rather than by name and date
(all of them), backup after import by strategy, never delete from the
source (digiKam offers it; Helios does not).

## 5. Slices

Each slice builds, passes its tests headless (no network, no device, no
owner), and leaves the application usable. The windows note's slice 4
(the Import focus) can land before or after S1; S1 does not touch the
window's layout.

1. **The trait, folders and cards behind it.** `Connector`, `Item`,
   `Page`, `Cursor` in a new crate (`helios-connect`), the contract
   suite, and the folder and card connectors moved under it; `Scan`
   lists through them. Nothing visible changes. Tests: the contract
   suite on the folder connector over a temporary tree, the import
   window's existing kittests and snapshots unchanged.
2. **Fetch, verify, place.** The `FetchSink` (BLAKE3 plus MD5, SHA-1,
   SHA-256), part files with offsets, `Access::Fetch` in
   `helios_library::import` (copy forced), the `remote` volume kind and
   sightings by item id, the `claims` alias table, pre-download
   deduplication. Tests: an in-memory connector with sizes, claims, a
   wrong claim, a cut at half a file, a 429 with `Retry-After`, an
   expired link; assert images, locations, sightings, the report.
3. **`import.source` for plugins.** The service in `helios-plugin`
   (manifest `[[sources]]`, `source/page`, staging), `PluginConnector`,
   a test plugin serving a fake remote from a folder; the gphoto2
   example moved to `plugins/sources/` on this service. Tests: the
   contract suite through the broker, crash and cancel mid-fetch.
4. **Saved imports and history in the vault.** `import/saved`,
   `import/history`, `import/cursors`; migration of
   `import-presets.json` and workspace import sources; the footer's
   menu, "Save as…", Recent imports, "Import again";
   `helios import --saved <id>` on the CLI. Tests: document round trip
   and upgrade, migration from fixture files, a kittest importing twice
   from history with "new only".
5. **Filter profile and tags.** The query over item metadata before
   fetch, tags as importer values undone with the import. Tests: a
   fixture source with mixed cameras and dates; undo takes the tags back.
6. **Automatic imports.** Triggers (plugged, watched folder, schedule,
   change feed), first-run confirmation, caps, the summary notice, the
   global pause, desktop-only runner. Tests: a fake `Volumes`, a fake
   clock, a temporary watched folder with a file growing then still, the
   cap turning a run into a question.
7. **Accounts.** Settings > Accounts, the OAuth loopback flow with PKCE
   brokered by Helios, refresh tokens in the keyring, the `account`
   capability, sign-out and revoke. Tests: a local fake authorisation
   server; a mock keyring backend.
8. **Google Photos (Picker).** `plugins/sources/google-photos`: session,
   `pickerUri` opened in the browser, polling, paged items, thumbnails,
   `=d` fetch with expiry handling, the location-loss note, probable
   matches. Tests against a recorded fake of the Picker endpoints; one
   check against a real account before release, by an agent with a test
   account, to settle what `=d` returns for a RAW.
9. **Google Takeout importer**, then **MTP**, then **remote storage**
   (`opendal`: S3, WebDAV, SFTP, SMB) as plugins, in the order the owner
   picks.

## 6. Choices for the owner

Not decided. Each has options in key order and a recommendation.

**C1. Google Photos: which route.**
- A. The Picker connector only: pick by hand, up to 2,000 per session,
  re-encoded bytes without GPS, no automatic import.
- B. A Google Takeout importer only: originals and GPS, a one-off
  migration, no live link.
- C. Both: Takeout for moving a library in, the Picker for "get these
  few photos from my phone's backup".

Recommended: **C**, Takeout first (it is what a migrating user needs and
it keeps originals), the Picker after S7.

**C2. Who holds an account's refresh token.**
- A. Helios: it runs the OAuth flow from the manifest, keeps the token
  in the keyring, hands the plugin a short-lived access token per call
  (a new brokered capability `account`).
- B. The plugin: it runs its own flow and keeps its token, under the
  ambient `secrets` and `network` capabilities.
- C. Nobody: sign in at each import.

Recommended: **A**: the refresh token never leaves Helios, revoking a
plugin revokes its access, and one Settings page lists every account.

**C3. A fetched item: copy, or reference the remote.**
- A. Always a verified local copy in the library folder; the remote is a
  sighting only, never a backup.
- B. Allow "reference remote": a preview and metadata in the Library,
  the original fetched on demand.
- C. As A, and a remote with exact originals (S3, a Helios server) may
  count as a backup copy.

Recommended: **A** now; C for exact-original remotes once scrubbing can
verify them (sync.md, section 7).

**C4. Where saved imports and the history live.**
- A. The vault's documents (`import/saved`, `import/history`), so they
  travel with the library and with sync; triggers name the machines they
  fire on.
- B. A per-machine settings file, as presets are today.
- C. In the workspace or its template.

Recommended: **A**; a template may still reference a saved import by id.

**C5. Presets and saved imports.**
- A. One notion, the saved import; a preset is a saved import without a
  source.
- B. Two notions: presets (choices) and saved imports (a source plus a
  preset).

Recommended: **A**: one menu, one word, Lightroom users find their
presets in it.

**C6. Automatic runs.**
- A. The first run confirmed in the window, later runs quiet with a
  summary notice and Undo; a question above a cap or for a new source.
- B. Always a toast asking "Import 312 new photos?".
- C. Fully silent, the history only.

Recommended: **A**.

**C7. What runs automatic imports.**
- A. The desktop while it is open (v1), the headless node later.
- B. The node from the start, as a background service.

Recommended: **A**: the node's service mode is not built yet, and an
import the person cannot see is the surprise C6 avoids.

**C8. What "filter profile" means.**
- A. A saved filter over the source's items, applied before fetching
  (types, dates, camera, name, the Library's query syntax).
- B. A Library view (smart album) the imported photos are shown in
  afterwards.
- C. Both, as two fields of the saved import.

Recommended: **A**; "show the import in this view" is the after-import
`select` field, extended later if wanted.

**C9. Items without a hash (Google, MTP) already probably in the
Library.**
- A. Shown "probably in your Library" and not fetched while "New only"
  is on; the person can tick them.
- B. Fetched anyway, deduplicated by BLAKE3 after download.

Recommended: **A** for rendered sources (fetching cannot prove anything
and costs quota), **B** for exact-original sources without a claim
(MTP), where the BLAKE3 settles it.

## Sources

- [Google Photos APIs: updates (Library API scope removal, 31 March 2025)](https://developers.google.com/photos/support/updates)
- [Picker API: list and retrieve media items (`=d`, location, 60-minute base URLs)](https://developers.google.com/photos/picker/guides/media-items)
- [Picker API: sessions (pickerUri, polling, maxItemCount 2,000)](https://developers.google.com/photos/picker/reference/rest/v1/sessions)
- [Picker API: mediaItems (fields; no hash, no size)](https://developers.google.com/photos/picker/reference/rest/v1/mediaItems)
- [Google Photos API limits and quotas](https://developers.google.com/photos/overview/api-limits-quotas)
- [Adobe: import photos automatically (Lightroom Classic Auto Import)](https://helpx.adobe.com/lightroom/help/import-photos-automatically.html)
- Crates and licences as listed on crates.io (2026-11-11): `gphoto2`
  LGPL-2.1-only, `libmtp-rs` MIT, `opendal` Apache-2.0, `aws-sdk-s3`
  Apache-2.0, `keyring` MIT or Apache-2.0, `oauth2` MIT or Apache-2.0,
  `notify` CC0-1.0, `smb` MIT.
