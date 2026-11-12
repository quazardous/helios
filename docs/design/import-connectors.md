# Import connectors: sources beyond folders and cards

Status: design study (2026-11-11), **decided** (2026-11-11): the owner
accepted every recommendation, C1 to C19 (section 10). Slice S1 (the
connector trait and registry, folders and cards behind it) is the first
built.

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
- **Standards** (section 5): the protocols (PTP, MTP), the card layout
  (DCF) and the metadata (Exif, XMP, IPTC, MWG's reconciliation) are
  standard and adopted as they are; **no shared import-plugin interface
  exists**, so the connector trait, its plugin service and the hooks
  are Helios's own. Devices go through one back end per platform:
  libgphoto2 and libmtp on Linux, `Windows.Media.Import` and WPD on
  Windows, ImageCaptureCore on macOS.
- **Camera bodies** (section 6): a vault record per model and per
  serial carries the body's colour profile pointer, lens matching, a
  default style and import settings; an import attaches them by the
  device's or the files' serial, the body over the model, and never
  guesses between two bodies that write no serial.
- **Lifecycle and hooks** (section 7): ten named stages from session
  start to session end; observers read the bus, participants answer
  with data (annotations, skip, veto, follow-up jobs) inside the
  import's transaction, so undo covers them; plugins and agents reach
  the same stages. Filter, deduplication, tags, bodies, stacks, backup,
  the map job and thumbnails become subscribers.
- **Contributed connectors** (section 8): one registry; an in-tree
  module contributes a connector as folders and cards do, a plugin
  through `import.source`; module path first.

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
- **Who holds the refresh token** is choice C2, decided (2026-11-11):
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

### 2.4.1 Connecting: a declared process, not only OAuth

Some connectors need a connection process before they can list
anything, and OAuth is only one kind (owner, 2026-11-11). A connector
declares its process in its manifest (`[[connect]]`), as ordered steps
Helios drives itself, so every connector, built in or a plugin, gets
the same screens, the same states and the same tests:

| Step kind | What Helios shows and does | Examples |
|---|---|---|
| `oauth` | the system browser on the consent page, PKCE and loopback redirect (2.4) | Google Photos, Dropbox |
| `form` | a form built from the step's schema (the declarative admin forms of the settings framework), secrets marked so they go to the keyring | NAS (SMB server, share, user, password), S3 (endpoint, bucket, keys), WebDAV |
| `device` | an instruction to follow on the device and a wait for it, with a timeout and a "try again" | a phone's "Allow access to data" after unlocking, a camera's Wi-Fi pairing (PTP/IP) accepted on the camera |
| `trust` | a fingerprint to confirm once, remembered per account | a self-signed WebDAV or NAS certificate, a PTP/IP camera's GUID |
| `code` | a code shown on one side and typed on the other | a device-pairing flow, a second factor the service asks for |
| `test` | a call the connector answers to prove the connection (`source.probe`), with its error in plain words | every connector, as the last step |

- **States**, per account or device: *not connected*, *connecting*
  (which step), *connected*, *needs attention* (a token expired, a
  password changed, a device not trusted any more). A `sign-in needed`
  or `not trusted` error from `list` or `fetch` moves the account to
  *needs attention* and the import window offers "Reconnect", which
  replays only the steps that failed.
- **Where**: "Add a source…" in the import window runs the process for
  a new account; Settings > Accounts lists every account with its state
  and "Reconnect" / "Sign out". Folders and mounted cards declare no
  process.
- **Agents and plugins**: the process is data, so the node exposes it
  (`import.connect` with the steps and their state); an agent may start
  it and fill a `form` the user allows, never a secret on its own, and
  never an `oauth` consent or a `device` step, which are the user's.
- **Tests**: each step kind runs headless against fakes (a fake OAuth
  server, a fake device that accepts after N polls, a fake certificate),
  so a connector's process is tested like its listing.

### 2.4.2 The OAuth step, as built

`helios_connect::oauth::OAuth` runs an `oauth` step (`authorize`,
`token`, `scopes`, and optionally `revoke` and `client_id` in the
manifest):

1. A code verifier (32 random bytes, base64url) and its S256 challenge,
   and a random `state`.
2. A listener on `127.0.0.1` with a port the system picks; the redirect
   URI is `http://127.0.0.1:<port>` (Google's loopback flow for desktop
   apps; no out-of-band copy and paste).
3. The system browser opened on the authorisation URL (the import
   window's dialog shows it too). The first request on the loopback
   that carries a `code` or an `error` is the answer: a `state` other
   than Helios's fails the step, `access_denied` says the consent was
   refused. The page shown says to go back to Helios.
4. The code exchanged at the token endpoint with the verifier.

**Where tokens live.** The refresh token, the access token and its
expiry go to the account's `Secrets`: on the desktop the system
keyring (`helios_connect::keyring::SystemSecrets`, service
`helios-import`, entry `<account>/<name>`), reached through
`secret-tool` or `security` as `helios-ai` does for its keys, so no
native D-Bus dependency (Windows has no keyring support yet: secrets
last the session there). The account's vault
document holds only where tokens renew and are revoked, the client id
and the step's id, never a token.

**Using and renewing.** `OAuth::access_token` gives a connector a valid
access token, renewed through the refresh token a minute before it
expires (a rotated refresh token is kept). A renewal the service
refuses (`invalid_grant`: access revoked, token aged out) leaves the
account *needs attention* at its `oauth` step, so Reconnect replays the
sign-in only. **Signing out** posts the token to `revoke` when the
service has one (a failure there is logged, not blocking), then forgets
the secrets.

**Client.** A client is an id and, for Google's desktop clients, a
secret Google does not treat as confidential; both always come from one
source. In order (`OAuth::client`): the person's own, set in Settings >
Accounts ("Use my own OAuth client", `import.setOAuthClient`, kept in
the keyring under the account `oauth-client/<connector>`); the
environment, `HELIOS_<CONNECTOR>_CLIENT_ID` and `_CLIENT_SECRET` (the
connector id upper-cased, `-` as `_`: `HELIOS_GOOGLE_PHOTOS_CLIENT_ID`);
Helios's built-in client, compiled in from the same variables when the
release build had them (`oauth::built_in`, from the repository's
secrets); then the manifest's `client_id`. Without one the step fails
and says where to set one in Settings. A renewal uses the secret of the
source holding the client id the account signed in with.

**Tests** (`crates/helios-connect/tests/oauth_flow.rs`) run against a
fake authorisation server on a local port, the "browser" being an HTTP
client that follows its redirect: PKCE checked by the server, a forged
`state`, a refused consent, a missing client, the person's own client
used over the environment's, a renewal and a
refused one, signing out with revocation.

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
  there is no "reference" for a fetched source (decided in C3).
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
  owner meant this or the Library's view after import was option C8,
  decided (2026-11-11) for this reading.
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

### 3.6 Sources and their choices (as built)

The owner's direction (2026-11-12): the Choices column's choices belong
to each source, not to the window. A **source** is a saved object: a
connector instance (its type and its own parameters: a folder's path,
an account) and its own import choices (copy or add where they are,
Into, the folder pattern, Name, Duplicates, Read darktable edits).

- **Where.** The sources are one of the person's documents the node
  keeps beside their settings layer, `user/import-sources` (TOML, a
  `[[source]]` table each), read and written through the node's settings
  (`NodeSettings`), so every client of one node sees the same sources.
- **The column follows the selected source**: the folder ticked last,
  or the account clicked last. Its first line says whose choices they
  are ("trips: its own choices", "trips: the default choices").
  Changing a choice saves it on that source; changes close together
  are one step of the column's Undo; "Reset to defaults" gives the
  source the window's choices again. The window's choices stay the last
  used, per library, as before; a source without choices of its own
  follows them.
- **A card** stays an automatic source with the window's choices,
  unless saved through "Edit source…" (section 3.8): its own choices
  then follow it where it is mounted.
- **Choices a connector cannot honour are hidden.** A connector reached
  through an account (`caps.accounts`, Google Photos) is remote: its
  photos are always copied ("Add where they are" hidden) and carry no
  XMP sidecars ("Read darktable edits" hidden).
- **Import** runs each folder ticked with its own choices.

Still to come: automatic imports hung on a source.

### 3.7 Adding a source: the wizard (as built)

"Add a source…", under the Sources column, opens a wizard in the import
window: its body shows the steps, Back, Next and Cancel under them.

1. **The kind**, among the connectors installed: Folder, Cards and
   cameras, then those that sign in to an account (a plugin's, such as
   Google Photos), each with the one line its manifest gives
   (`description`). Those needing an account wait for a library open,
   where accounts are kept.
2. **Its setup.** A folder is typed or chosen (Next waits for one that
   exists). An account is picked among the connector's, or signed in to
   with "Sign in to an account…": the connection dialog runs over the
   wizard (section 2.4.1), after the plugin's consent when it lacks
   capabilities, and the account connected there is picked. Cards need
   nothing: they are listed on their own when inserted, so the wizard
   says so and stops there.
3. **The choices**: the Choices column's widgets, started from the
   window's choices, without those the connector cannot honour (section
   3.6); for a remote connector, copy is set and darktable edits are off,
   so a source never keeps a choice it cannot follow.
4. **A name**, the folder's or the account's to start with.

"Add the source" saves it (`Sources::add`, in place of a source of the
same folder or account) and selects it: a folder is ticked, an account
clicked, and the column shows its choices under its name. The sources
saved under a name have their own rows, under "Saved sources": a
folder's row ticks it, an account's selects it; a named folder is not
listed again under Recent or Favourites. Cancel, at any step, saves
nothing; an account signed in to on the way stays an account (Settings >
Accounts lists it).

### 3.8 Editing a source (as built)

"Edit source…", in a source's menu in the Sources column (a right click
on its row), opens the wizard's steps on that source, from its setup; its
kind is the source's and is not asked again.

1. **Its parameters**: the folder or the account, then the connector's
   own parameters, from its configuration contract
   (`ConnectorProvider::config()`, its `source` level): each key that is
   not one of the choices, by its type (a checkbox, a choice, a number,
   a text), hidden while its `visible_when` does not hold. Next waits
   for values the contract takes. They are kept on the source
   (`params`, a table of `[[source]]`).
2. **Its choices**, as in the wizard. Copy or add where they are is
   offered only when the connector honours both: a connector reached
   through an account, or whose source contract has no `copy` key
   (Google Photos), shows copy alone; a source kept with "add where they
   are" by an older document is saved copied.
3. **Its name**, empty for a source never saved: the folder's or the
   account's then.

"Save the source" puts it in place of the source edited (a folder moved
is ticked where it is now), one step of the column's Undo. A remembered
source, a recent folder or a card, is saved by it: that is how one is
promoted. **"Forget this source"**, in the same menu, takes a source off
the column: its name, choices and parameters, and a folder off Recent
and Favourites; it is one Undo step too, bringing all of it back. It
leaves the image map alone, which "Forget this folder" clears.

**The choices and the contract.** The local sources' contract
(`helios_connect::local::source_contract`) names the choices' keys
(`copy`, `into`, `layout`, `rename`, `duplicates`, `darktable`); the
window's `Choices` read and write those values
(`Choices::from_values`, `Choices::values`), and its defaults are the
contract's. Its stored form keeps its own field names (`copy_folders`),
so documents and presets already written still read.

Not yet: promoting an account listed under Accounts (only those saved
are edited); a contract key's label and help in the person's language
(the contract's English is shown).

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

## 5. Standards and frameworks

The owner's question (2026-11-11): do photo-import connector standards
already exist? Checked on 2026-11-11 against the sources listed at the
end. In short: **the protocols, the card layout and the metadata are
standard; the connector interface is not.** No open "media source
plugin" interface is shared by photo applications; each application
wraps the same protocols in its own code.

### 5.1 What each standard is, and what Helios does with it

| Standard or framework | What it is | Verdict for Helios |
|---|---|---|
| **PTP** (ISO 15740:2013, confirmed 2023), USB Still Image class | the protocol cameras speak over USB. A session gives `DeviceInfo` (Manufacturer, Model, DeviceVersion, **SerialNumber**), storages, object handles and `ObjectInfo` (file name, format, size, capture date, a thumbnail), and optionally `GetPartialObject` (a byte range) | **Adopt** its vocabulary in the device connector: the device identity is `DeviceInfo`'s make, model and serial; `ObjectInfo` fills `Item` before any fetch; `caps.resume` when the camera has `GetPartialObject`. Object handles are only valid within a session, so the item id is the storage plus the DCF path, not the handle. No hash: `claims: None` |
| **PTP/IP** (CIPA) | PTP over TCP/IP, cameras on Wi-Fi | **Later**, through libgphoto2 (which speaks it for some bodies). Pairing differs per maker; not worth a connector of its own now |
| **MTP** (USB-IF, version 1.1, April 2011) | Microsoft's extension of PTP for phones and players: object properties, and a 128-bit persistent unique object identifier (PUOID) | **Adopt** through libmtp on Linux and WPD on Windows: the PUOID, when the phone fills it, is the stable item id. No content hash; listings are slow; one client at a time |
| **DCF** (CIPA DC-009 and JEITA CP-3461B, version 2.0, 2010) | the card layout: `DCIM/` at the root, folders `100ABCDE` to `999ABCDE`, files `ABCD0001` to `ABCD9999`, one "DCF object" being the files sharing a base name (JPEG, RAW, THM) | **Adopt** directly, as `Scan` already starts at `DCIM`: the base-name rule fills `Item.group` (pairs and THM files), the folder and file numbers feed `{frame}`. Numbers wrap at 9999 and repeat across cards, so names are never identity (content is) |
| **Windows Portable Devices** (WPD, COM) and **WIA** | WPD is Windows's API for cameras, phones and players over PTP and MTP; WIA is the older imaging API, kept through a WIA-WPD compatibility layer. Windows 10 also has the **Photo Import API** (`Windows.Media.Import`): sources with Manufacturer, Model and SerialNumber, items with their siblings (RAW+JPEG) and sidecars | **Use per platform** on Windows, where libgphoto2 goes through libusb and fights the system driver (digiKam's maintainers say the same). `Windows.Media.Import` first (it already pairs siblings and sidecars), WPD where it falls short; both reachable from Rust through the `windows` crate (MIT or Apache-2.0). Whether a desktop process may call `Windows.Media.Import` is to be checked on a real machine. WIA: **not relevant** |
| **ImageCaptureCore** (macOS) | Apple's framework for cameras and phones: `ICDeviceBrowser`, `ICCameraDevice` (with `serialNumberString`, nil when the device gives none), files with thumbnails, sidecars and downloads | **Use per platform** on macOS: the system's own camera daemon claims PTP devices, which is why gphoto2 is fragile there. iPhones appear here with their camera roll |
| **libgphoto2** (LGPL-2.1+) | the open driver library for thousands of cameras, PTP and vendor protocols, PTP/IP for some | **Use** on Linux (as darktable, digiKam and Shotwell do), and as a fallback elsewhere |
| **GVfs** (`gvfsd-gphoto2`, `gvfsd-mtp` and their volume monitors) and **KIO** (`kio-extras`: the `mtp:/` and `camera:/` workers) | the GNOME and KDE desktops expose cameras and phones as virtual file systems, GVfs also through FUSE under `/run/user/<uid>/gvfs` | **Not as a connector.** Reading the FUSE path works as a folder but loses the device identity and holds the device; a desktop mount is mostly an **obstacle**: libgphoto2 and libmtp cannot open a device GVfs holds (darktable's manual asks to unmount first; Shotwell has the same issue open). The device connector says "held by the desktop: release it" with a button that unmounts through GIO |
| **Android MediaStore, Storage Access Framework, Photo Picker** | APIs for an application **running on the phone** | **Not relevant** to a desktop import: from a computer a phone is an MTP or PTP device (in PTP mode it shows only `DCIM` and `Pictures`). They matter only for a future Helios companion on Android, which would push through sync, not import |
| **Exif** (CIPA DC-008, JEITA CP-3451; version 3.0, 2023) | the camera's metadata. Since 2.3 (2010): `BodySerialNumber`, `LensMake`, `LensModel`, `LensSerialNumber`, `LensSpecification`, `CameraOwnerName` | **Adopt**: the body serial and the owner name identify a body (section 6); the lens fields feed lens matching. The loader already reads the serial (`helios_load::Shot::serial`); the lens fields are to add |
| **XMP** (ISO 16684-1; Adobe's specification part 3 for storage in files) | the metadata model and its serialisation; a sidecar is looked for beside the file, same base name. Adobe names it `NAME.xmp`, darktable `NAME.EXT.xmp` | **Adopt** as the vocabulary of `Item.metadata`: keys are XMP property names (`exif:DateTimeOriginal`, `tiff:Model`, `exifEX:BodySerialNumber`, `dc:subject`, `Iptc4xmpCore:Location`…) rather than names of our own, so connectors, importer values and the XMP writer speak one language (option C14). Both sidecar namings are read; Helios writes darktable's |
| **IPTC Photo Metadata** (version 2024.1, December 2024) | descriptive and rights metadata (creator, copyright, location shown, people, digital source type), as XMP and as the legacy IIM block | **Adopt** for what connectors bring that files lack (a Google description, an album's place): mapped to IPTC properties, written as importer values |
| **MWG guidelines** (Metadata Working Group, version 2.0, 2010; the group is inactive and the document survives in archives) | how to reconcile Exif, IPTC-IIM and XMP when they disagree: native Exif over its XMP copy, IIM over XMP when the IPTC digest shows IIM changed since, XMP otherwise; hierarchical keywords and face regions | **Adopt** as the read rule after a fetch, with one addition the guidelines lack: a connector's opinion ranks last, under all three (2.2) |
| **DNG and DCP** (Adobe) | DNG is the open raw format; a DCP is a camera profile, tied to a camera by `UniqueCameraModelRestriction` (and optionally `ProfileCalibrationSignature`) | **Adopt the keying**: Lightroom and Camera Raw list only the DCPs whose restriction matches the raw's camera model, and their **raw defaults** are set per camera model, optionally per **serial number** and per ISO. Helios's calibration already keys by model with `@serial` (calibration.md, section 6); section 6 below extends the same layering to everything a body carries |
| **lensfun** (library LGPL-3, database CC BY-SA 3.0) | the open database of lens corrections: a camera entry is maker and model as Exif writes them, a variant, a mount and a crop factor; a lens entry is maker, model, mount and the crop factor it was measured on | **Adopt** its matching: a body's camera key maps to a lensfun camera (mount, crop factor), which narrows the lens search; a body record may pin the lensfun camera when the name does not match (6.3) |

### 5.2 What other applications use

| Application | Cameras and phones | Profiles and lens data |
|---|---|---|
| **darktable** | libgphoto2 on every platform ("copy & import from camera", tethering); the camera must not be mounted by the desktop; folders otherwise | rawspeed matrices, ICC from a folder, lensfun and embedded corrections |
| **digiKam** | an Import tool over libgphoto2 on Linux and macOS, or USB mass storage; on Windows no native back end yet (a WIA one is planned in its bug tracker), so mass storage only | lensfun |
| **Shotwell** | libgphoto2 directly; conflicts with a GVfs mount (an open issue asks to go through GVfs instead) | none |
| **RawTherapee** | no import: a file browser over folders | DCP (matched by exact camera name), lensfun, LCP |
| **Lightroom Classic** | the operating system's device access (Adobe does not document which API); devices first in the source list | DCP per camera model; raw defaults per model, serial and ISO |

### 5.3 A shared import-plugin standard?

**None exists.** The nearest things are:

- **KIPI** (KDE Image Plugin Interface): plugins shared by digiKam,
  Gwenview and KPhotoAlbum, mostly export and processing; digiKam left it
  for its own DPlugins in 2019. Not an import-source interface, and not
  maintained.
- The **operating-system frameworks** (WPD and `Windows.Media.Import`,
  ImageCaptureCore): shared by every application on one platform, but
  each is one platform's API, not a plugin interface.
- **libgphoto2**: a library many applications link, not an interface
  they offer each other.
- **Lightroom Classic's SDK** is proprietary and has **no import hooks**
  at all (`catalog:addPhoto` imports one photo; a request for pre- and
  post-import hooks has been open on Adobe's forum since 2012; plugins
  such as Folder Watch poll).

So Helios defines its own **`Connector` trait** (2.2) and its plugin
form, the **`import.source` service** (2.6), and its own **lifecycle
hooks** (section 7). Nothing standard covers: paged listing with a
cursor and a change feed, claimed hashes and their check, accounts and
quotas, the item-to-image identity, or saved and automatic imports.

### 5.4 What the connector model adopts

- **Device identity** from PTP `DeviceInfo` (make, model, serial), the
  USB serial descriptor, `PhotoImportSource.SerialNumber` or
  `ICDevice.serialNumberString`, whichever the back end gives, carried
  on `SourceRef` as a new `device: Option<DeviceIdentity>` (make, model,
  serial, `via`). A saved import matches a device by it (3.1's
  `match: { serial }`), and a body record links it to the files' Exif
  serial (6.1).
- **DCF rules** for any source with a `DCIM` folder: groups by base
  name, `{frame}` from the file number, never identity from names.
- **Metadata as XMP properties**, reconciled by MWG's read rules, the
  connector's values ranked last (C14).
- **One back end per platform for devices**: libgphoto2 and libmtp on
  Linux; `Windows.Media.Import` then WPD on Windows; ImageCaptureCore on
  macOS. Each is one implementation of the same device connector, in a
  plugin (C13). Cameras that mount as a disk keep going through the card
  connector on every platform, which is the path most people use.
- **Profile keying as Adobe and lensfun do**: camera model first, serial
  as an optional refinement; never the serial alone.

## 6. Camera bodies: profiles that follow the camera

A connector that knows the camera (PTP `DeviceInfo`, or the Exif of the
first files) lets an import attach what belongs to that camera:

- the **colour profile** from a chart (calibration.md, section 6: keyed
  by camera model, optional serial, nearest illuminant);
- the **lens and camera matching** for lens corrections (lensfun, and the
  order of camera-profiles-research.md, 5.6);
- a **default style or preset** ("this body always gets X");
- **import settings**: backup strategy, rename rule, tags.

### 6.1 A body and its identity

- **Key**: the camera key (the normalised make and model of
  raw-corpus.md, section 4, as calibration uses) and, for one body, its
  serial: `nikon z 6` for every Z 6, `nikon z 6@6012345` for one.
- **Where the serial comes from**: Exif `BodySerialNumber` or the maker
  note's serial (what `helios_load::Shot::serial` reads today), PTP
  `DeviceInfo.SerialNumber`, the USB serial descriptor, or the platform
  API's serial. These strings need not be equal for one camera (a USB
  serial padded with zeros, a different number altogether), so a body
  keeps **every identifier it was seen with** (`ids`, each with its
  `via`). They are linked when they are seen together: a device
  connector reads `DeviceInfo`, the first file fetched from it carries
  an Exif serial, and both go on the same record.
- **New bodies appear by themselves.** The first import of a model or
  serial Helios has not seen creates its record, unnamed ("Nikon Z 6 ·
  …2345") and with no defaults: nothing changes for that import, and the
  summary notice says "New camera: Nikon Z 6 · …2345. Name it". No
  question during an import.

### 6.2 The record

The vault's documents, namespace `cameras/bodies`. A **model record**
(key without serial) holds what applies to every body of the model; a
**body record** (key with serial) holds what one body overrides.

```jsonc
{
  "format": 1,
  "camera": { "make": "Nikon", "model": "Z 6", "key": "nikon z 6" },
  "serial": "6012345",                     // null on a model record
  "name": "Z 6 B",                         // the person's name; {body} in rename patterns
  "ids": [
    { "via": "exif", "value": "6012345" },
    { "via": "ptp",  "value": "0006012345" }
  ],
  "tell": null,                            // how to tell it apart without a serial (6.4)
  "colour": { "profiles": "calibration/defaults" },   // a pointer: calibration stays the source of truth
  "lens": { "lensfun_camera": null, "prefer": "embedded" },
  "edit": { "style": null, "presets": [] },          // style or module presets by id and version
  "import": {
    "saved": null,                         // a saved import used when this body is the source
    "rename": "{date}-{body}-{seq}",
    "tags": ["gear|Z 6 B"],
    "backup": "strategy"
  },
  "retired": false
}
```

- **Colour** is not copied: the record points to
  `calibration/defaults`, whose keys already are `<camera key>` and
  `<camera key>@<serial>`. Its pick (nearest illuminant within 0.02 in
  `uv`) is unchanged.
- **Lens**: `lensfun_camera` pins lensfun's camera entry (maker, model,
  variant) when the Exif names do not match it, which gives the mount
  and crop factor for the lens search; `prefer` says whether embedded
  maker corrections or lensfun come first for this body.
- **Edit**: a style or module presets, by vault id and version, once
  styles live in the vault (ROADMAP 2.3). They are applied when the
  image's edit is **created**, as the first history steps, named "from
  Z 6 B", so an edit renders the same after the record changes (C12).
- **Import**: what a saved import would say, used when this body is the
  source and no saved import says otherwise.

**Precedence**, from strongest: the saved import or the window's choice
for this import, the body record, the model record, the library's
defaults. The Choices column shows where a value comes from ("rename:
from Z 6 B").

### 6.3 How it attaches at import

Through the lifecycle hooks of section 7, as one subscriber, the
`camera-bodies` hook:

- at **item discovered**, a device source's `DeviceIdentity` names the
  body for every item, before any fetch, so the window shows "Z 6 B" in
  the source and the rename example already uses `{body}`;
- at **fetched and verified**, the file's Exif (make, model, serial,
  owner name) confirms or finds the body, links new identifiers, and
  creates a record for a new body;
- at **before commit**, it annotates the image with the body link (a
  vault field `camera.body`, searchable as `body:"Z 6 B"`), the body's
  tags and importer values, and the **edit defaults** to apply at the
  edit's creation (profile choice, lens, style), which are recorded, not
  rendered.

Undoing the import takes all of it back. A card from a known body
matches by its files: the saved import of 3.1 gains
`match: { body: "nikon z 6@6012345" }`, true for a device with that
serial or a card whose first DCF files carry it.

### 6.4 Two bodies of the same model

- **Both write their serial** (most interchangeable-lens bodies): two
  body records under one model record. Each may override any field;
  what neither overrides comes from the model. A chart profile made on
  body A is saved as `@serialA` when the person ticks "only this body"
  (calibration's Save step), and as the model's default otherwise:
  sensors of one model differ little, and one profile usually serves
  both.
- **No serial in the files** (phones, some compacts and older bodies,
  JPEGs re-saved by an application): everything falls to the model
  record, and Helios **does not guess**. The person may set a `tell`
  rule on a body record, checked in this order: the Exif
  `CameraOwnerName` or `Artist` set in the camera's menu, the file-name
  prefix set in the camera (`ZB6_`), the device serial when it came over
  USB, or the card (its volume id). A rule that matches both bodies, or
  none, leaves the model record in charge and says so in the summary.
- **Moving a body's files to another** (a serial read wrong, a repaired
  body with a new board): "Merge into…" on the body record moves its
  identifiers and its images' links, one undo step.

## 7. Import lifecycle and hooks

The owner's requirement (2026-11-11): the import process must have a
lifecycle the rest of Helios can hook into. Today each after-import step
is called by hand from `helios_library::import` or the desktop glue
(stacks, backup trigger, map pause, thumbnails). This section names the
stages, says what a hook may do at each, and moves those steps onto it.

### 7.1 What Helios already has to build on

| Piece | Where | What the hooks take from it |
|---|---|---|
| The event bus | `helios_core::Bus` (`crates/helios-core/src/bus.rs`) | typed events in the past tense, a journal, `seq` and `cause`: the **observer** side of every stage is a bus event |
| Events for agents | `helios_events`, `helios_wait_event` (`crates/helios-node/src/mcp.rs`) | agents read import stages with the tools they already use |
| The scheduler | `helios_core::scheduler` (`JobSpec`, `Priority`: batch, prefetch, interactive), job kinds `import`, `backup`, `map`, `stacks.suggest` | **follow-up work** is a job request, never a thread a hook starts |
| The plugin broker | `crates/helios-plugin` (activation events `onStartup`, `onService:`, `onCommand:`, `onImporter:`; timeouts and `$/cancelRequest`; crash backoff) | out-of-process hooks: one more activation event and one more service |
| The audit trail and undo | `helios_vault` catalog (`AuditEntry`, `vault.field_changed`), the import's one undo step (`library_by: import-<n>`) | what a hook writes goes through the import's transaction and its undo step |

**Prior art.** darktable's Lua API has `pre-import` (the list of files,
which a script may change), `post-import-image` (per image, blocking,
handlers in sequence) and `post-import-film` (once, after the images).
Scripts there write to the database directly, so undo does not see them,
and a slow script stalls the import. Lightroom Classic has no import
hooks (5.3). Helios keeps darktable's three moments and adds the ones a
connector needs (listing, fetch, verify), with writes returned as data.

### 7.2 The stages

| # | Stage | A hook receives | A hook may |
|---|---|---|---|
| 1 | `session.started` | the session: id, machine, trigger (by hand, plugged, schedule…), sources, choices, saved import | observe; **veto** the session with a reason (a policy plugin) |
| 2 | `source.listed` (per page, then complete) | the source, the page's counts, the cursor | observe |
| 3 | `item.discovered` | the `Item` (id, path, size, metadata, claim, group), the source's `DeviceIdentity` | **annotate** (body, tags, values); **skip** with a reason |
| 4 | `item.decided` | the item and the core's verdict: wanted, already in the Library, probably in the Library, filtered out | **skip** a wanted item, or **want** one the filter dropped; annotate |
| 5 | `item.before_fetch` | the item, its destination name | skip (last chance before bytes move) |
| 6 | `item.fetched` | the item, the staged file's path (read-only), its BLAKE3, the claim check, the file's own metadata | annotate from the bytes (Exif serial, a detector's tags); **veto** the item |
| 7 | `item.before_commit` | the item, its image id, whether it is a duplicate and the policy's answer | annotate (tags, values, body link, edit defaults, stack hints); veto |
| 8 | `item.imported` | the image id, its location, the annotations applied | observe; **schedule** follow-up work |
| 9 | `batch.done` | the image ids of one vault transaction (a page) | observe; schedule |
| 10 | `session.ended` (done, failed or cancelled) | the report: counts, failures, safety of cards, cursor reached | observe; schedule |

Stages 3 to 8 are per item but **delivered in batches** (a page, up to
500 items), so a hook costs one call per page, not per file.

### 7.3 Two kinds of subscriber

- **Observers** read the bus: every stage publishes an event
  (`import.session_started`, `import.page_listed`, `import.batch_done`,
  `import.session_ended`…; per-item stages are summarised per batch with
  the item ids, so 50,000 items do not flood the journal). Asynchronous,
  never wait for, never slow the import. Anyone may observe: desktop
  panels, the Log, plugins, agents.
- **Participants** are called in the import job, synchronously, at
  stages 1 and 3 to 10, and **answer with data**: annotations, skip or
  veto with a reason, follow-up job requests. A participant never writes
  to the vault, the library folder or the source itself.

```rust
pub trait ImportHook: Send + Sync {
    fn describe(&self) -> HookInfo;          // id, stages, band, priority, deadline, required?
    fn on_items(&self, stage: Stage, session: &SessionInfo, items: &[HookItem], cancel: &Cancel)
        -> Result<Vec<Answer>, HookError>;   // one answer per item
    fn on_session(&self, stage: Stage, session: &SessionInfo, report: Option<&Report>)
        -> Result<SessionAnswer, HookError>;
}

pub enum Answer {
    Pass,
    Annotate(Vec<Annotation>),               // tags, values, body, edit defaults, stack hint
    Skip(String),                            // stages 3 to 5: not fetched, reported
    Want,                                    // stage 4: take an item the filter dropped
    Veto(String),                            // stages 6, 7: not imported, staged file removed
}
```

### 7.4 Order, time and failure

- **Order.** Participants run in three bands: `core.early` (the saved
  import's filter, deduplication), `extensions` (bodies, plugins,
  declarative rules), `core.late` (duplicate policy, history). Within a
  band, by declared priority, then by id, so the order is the same on
  every run. Annotations accumulate: tags and values add (add-only, as
  importer values already are); for a single-valued field (the body
  link) the earlier band wins, and a conflict is reported. A skip or a
  veto stops the item; later hooks see it as skipped.
- **Time.** Each participant declares a deadline per batch; the core
  caps it (in process: 2 s per batch; plugins: 10 s per batch, with
  `$/cancelRequest` when it passes). A hook past its deadline is
  treated as having answered `Pass` for that batch and the report says
  so.
- **Failure is isolated.** An error, a timeout, a panic (in process,
  caught at the call) or a plugin crash costs that hook's answer for
  that batch and nothing else: the item goes on as if the hook had
  passed (**fail open**). Three failures in a row suspend the hook for
  the session, with a line in the report and the Log. A hook may
  declare itself `required` (a compliance check): then its failure holds
  the batch's items as failed rather than imported without it (C16).
- **Undo covers it.** Annotations are applied by the import, in its own
  vault transaction, audited as `import:<hook id>` under
  `library_by: import-<n>`; undoing the import takes them back with the
  images. Follow-up jobs on vault state (stack proposals, edit defaults)
  are withdrawn by the undo; work outside the vault (a backup copy
  already written) is not undone, and the undo says so ("2 backup copies
  stay on Backup A").
- **Cancellation.** A cancelled session stops calling participants,
  runs `session.ended` (cancelled) for observers, and leaves what was
  committed, as the import does today.

### 7.5 Plugins and agents

- **Plugins** declare hooks in their manifest and get a new activation
  event, so a plugin starts only when an import reaches its stage:

  ```toml
  [[import.hooks]]
  id = "gps-from-track"
  stages = ["item.before_commit"]
  band = "extensions"
  priority = 100
  deadline_ms = 5000
  activation = "onImportStage:item.before_commit"
  ```

  The service `import.hook` (interface version 1) has one method,
  `hook.call {stage, session, items}`, answered with one `Answer` per
  item; the staged file is readable at stage 6 through the brokered
  `images.read` capability. Observers among plugins get the bus events
  as notifications.
- **Agents** (MCP and the node) observe with `helios_events` and
  `helios_wait_event` on the `import.*` kinds. An agent is too slow and
  too uncertain to sit inside an import, so it participates in two other
  ways (C17):
  - **declarative rules**: a saved query and the annotations or skip it
    leads to ("camera Z 6 and lens 105 mm: tag `macro`"), stored in the
    vault (`import/rules`) and run by a core participant; an agent
    proposes, dry-runs and saves them;
  - **a review pause**: a session started with `review: true` stops after
    stage 4 with the decided list; `import.review {session, answers}`
    amends it (skips, wants, tags), then the session goes on. A deadline
    ends the pause with the core's verdicts.

### 7.6 What becomes a subscriber

| Feature | Stage | How |
|---|---|---|
| Saved import's filter profile (3.1, C8) | 4 | core participant, `core.early`: wanted or filtered out |
| Deduplication before download (2.7) | 4 | core participant, `core.early`: in the Library, probably in it |
| Exact duplicates (skip, keep, ask) | 7 | core participant, `core.late`; `ask` holds the item for the Duplicates panel, as today |
| Camera bodies (section 6) | 3, 6, 7 | `camera-bodies` participant: body link, its tags and values, edit defaults |
| Colour profile, lens, style per body | 7 | annotations of the body hook, applied at the edit's creation (C12) |
| Saved import's tags and album | 7 | core participant: add-only importer values under `import:<saved id>` |
| Declarative rules (7.5) | 3 to 7 | core participant reading `import/rules` |
| Stack proposals | 10 | schedules `stacks.suggest` over the imported ids |
| Backup after import | 9, 10 | schedules the backup job when the strategy's trigger is `OnImport`; "safe to format" recomputed when it ends |
| The map job | 1, 3, 10 | pauses at 1 (as now), records sightings from discovered items, resumes at 10 |
| Cache warm-up | 8 | schedules thumbnails at `Prefetch` priority, the visible cells first, reusing the glance cache |
| Health | 10 | re-checks backups and cards |
| The history record (3.4) | 10 | core participant, `core.late`: writes `import/history` |

## 8. Contributing a connector

The owner's requirement (2026-11-11): a module, and if not too
complicated a plugin, must be able to contribute a new connector.

### 8.1 One registry, two ways in

Helios's image modules are pure (`helios_core::Module`: no IO, no
threads), so a connector is not an image module; it is a **sibling kind
of module** built the same way: a crate in the tree with a manifest
readable without running it and a trait implementation, listed in a
static registry, as `helios_node::registry()` lists image modules.

- **In process (a module).** A crate (for instance
  `crates/helios-connect-<name>`) exports a `ConnectorProvider`:

  ```rust
  pub trait ConnectorProvider: Send + Sync {
      fn manifest(&self) -> ConnectorManifest;            // data: readable without running
      fn create(&self, settings: &Settings, services: &ConnectorServices)
          -> Result<Box<dyn Connector>, ConnectorError>;
  }
  ```

  and `helios_connect::registry()` lists it beside `folder` and `card`.
  `helios-declarations` collects its texts as it collects modules', so
  its label is translated like any other.
- **Out of process (a plugin).** The manifest declares `[[sources]]`
  with the same fields as `ConnectorManifest`; when the plugin is
  enabled the broker registers a `PluginConnector` for each, in the same
  registry, speaking `import.source` (2.6) and starting the plugin on
  `onSource:<id>`. A disabled or removed plugin takes its connectors
  away; their sources stay in the map as offline volumes.

`ConnectorManifest`: id, version, label and description (text keys),
`caps` (2.2), what it needs (an account and its OAuth endpoints and
scopes, a USB device, the network), its settings schema, the platforms
it runs on, and the interface version.

### 8.2 In the import window

The Sources column lists the registry, not a fixed set: detected
sources (`caps.detect`) under Devices, signed-in accounts under their
connector, and "Add a source…" for connectors that need setting up (as
built, a wizard: section 3.7)
("Connect Google Photos…", "Add an S3 bucket…", with the settings form
built from the manifest's schema). The folder and card connectors are
two entries of the same registry, so nothing in the window is special
to them.

### 8.3 Shared and different

| | Module (in process) | Plugin (out of process) |
|---|---|---|
| Registry, manifest, caps | same | same |
| Lifecycle hooks (section 7) | same stages, same events | same |
| Verification | Helios's `FetchSink`: BLAKE3 and the claim, size check, part files | same, on the staged file |
| Bytes | written into the sink directly, hashed as they stream | written to the brokered staging folder, then read once by Helios |
| Credentials | short-lived access tokens from the account service, in process | the same tokens per call (`params.auth`) through the brokered `account` capability; the refresh token never leaves Helios either way (C2) |
| Isolation | none: in-tree code, reviewed and compiled in | its own process; a crash costs that call, restart with backoff |
| Cancel and time | the `Cancel` token | `$/cancelRequest` and the broker's timeouts |
| Who writes it | Helios's tree (and forks) | anyone; official ones under `plugins/sources/<name>` |

**The plugin path is not much harder**: the broker, staging, paged
notifications and crash handling exist for importers. What is harder is
not the plugin but what such connectors need: accounts (S7) and device
access. So the module path comes first (S1, where folder and card are
its first users), the plugin path second (S3). Third-party in-process
connectors are not offered: Rust has no stable ABI to load them, and
loading them would lose crash isolation (C18).

## 9. Slices

Each slice builds, passes its tests headless (no network, no device, no
owner), and leaves the application usable. The windows note's slice 4
(the Import focus) can land before or after S1; S1 does not touch the
window's layout.

1. **The trait, folders and cards behind it.** `Connector`, `Item`,
   `Page`, `Cursor` in a new crate (`helios-connect`), the contract
   suite, the connector registry and `ConnectorManifest` (8.1), and the
   folder and card connectors moved under it as its first two modules;
   `Scan` lists through them. Nothing visible changes. Tests: the contract
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
10. **Lifecycle and hooks, in process.** The stages of 7.2 as bus
    events and `ImportHook` participants, bands, deadlines, fail-open
    isolation, annotations applied in the import's transaction and taken
    back by its undo; today's after-import steps (stacks, backup trigger,
    map pause, history) moved onto it. Lands right after S2, before S4,
    so S4 to S6 are written as subscribers. Tests: a recording hook sees
    the stages in order; a slow hook, a failing one and a panicking one
    leave the import whole and are reported; undo removes a hook's tags.
11. **Hooks for plugins and agents.** The `import.hook` service and the
    `onImportStage:` activation event, `import/rules` with a dry run, the
    review pause and `import.review`. Tests: a test plugin annotating a
    batch, one crashing, one past its deadline; a rule dry-run; a session
    paused and amended through the node.
12. **Camera bodies.** `cameras/bodies` documents, body identity from
    Exif (serial, owner name) and from `DeviceIdentity`, the
    `camera-bodies` hook, the `camera.body` field and `body:` search,
    `{body}` in rename patterns, edit defaults applied at the edit's
    creation, Settings > Cameras (name, merge, `tell` rules), a saved
    import's `match: { body }`. Tests: two bodies of one model with
    serials get their own defaults; two without serials fall to the
    model until a `tell` rule separates them; a new body is created
    unnamed; undo removes the links.
13. **Device identity and platform back ends.** `DeviceIdentity` on
    `SourceRef` (PTP `DeviceInfo`, USB serial), the DCF grouping rule for
    every `DCIM` source, metadata keyed by XMP property names with MWG's
    read rules; then the device connector on `Windows.Media.Import`/WPD
    and on ImageCaptureCore, as plugins, each checked on a real machine
    by an agent before release. Tests: fixture `DeviceInfo` and DCF
    trees; MWG precedence on files whose Exif, IIM and XMP disagree.

## 10. Choices for the owner

Decided (2026-11-11): the owner accepted every recommendation, C1 to
C19. Each keeps its options in key order; the decision is the line
below them.

**C1. Google Photos: which route.**
- A. The Picker connector only: pick by hand, up to 2,000 per session,
  re-encoded bytes without GPS, no automatic import.
- B. A Google Takeout importer only: originals and GPS, a one-off
  migration, no live link.
- C. Both: Takeout for moving a library in, the Picker for "get these
  few photos from my phone's backup".

Decided (2026-11-11): **C**, Takeout first (it is what a migrating user needs and
it keeps originals), the Picker after S7.

**C2. Who holds an account's refresh token.**
- A. Helios: it runs the OAuth flow from the manifest, keeps the token
  in the keyring, hands the plugin a short-lived access token per call
  (a new brokered capability `account`).
- B. The plugin: it runs its own flow and keeps its token, under the
  ambient `secrets` and `network` capabilities.
- C. Nobody: sign in at each import.

Decided (2026-11-11): **A**: the refresh token never leaves Helios, revoking a
plugin revokes its access, and one Settings page lists every account.

**C3. A fetched item: copy, or reference the remote.**
- A. Always a verified local copy in the library folder; the remote is a
  sighting only, never a backup.
- B. Allow "reference remote": a preview and metadata in the Library,
  the original fetched on demand.
- C. As A, and a remote with exact originals (S3, a Helios server) may
  count as a backup copy.

Decided (2026-11-11): **A** now; C for exact-original remotes once scrubbing can
verify them (sync.md, section 7).

**C4. Where saved imports and the history live.**
- A. The vault's documents (`import/saved`, `import/history`), so they
  travel with the library and with sync; triggers name the machines they
  fire on.
- B. A per-machine settings file, as presets are today.
- C. In the workspace or its template.

Decided (2026-11-11): **A**; a template may still reference a saved import by id.

**C5. Presets and saved imports.**
- A. One notion, the saved import; a preset is a saved import without a
  source.
- B. Two notions: presets (choices) and saved imports (a source plus a
  preset).

Decided (2026-11-11): **A**: one menu, one word, Lightroom users find their
presets in it.

**C6. Automatic runs.**
- A. The first run confirmed in the window, later runs quiet with a
  summary notice and Undo; a question above a cap or for a new source.
- B. Always a toast asking "Import 312 new photos?".
- C. Fully silent, the history only.

Decided (2026-11-11): **A**.

**C7. What runs automatic imports.**
- A. The desktop while it is open (v1), the headless node later.
- B. The node from the start, as a background service.

Decided (2026-11-11): **A**: the node's service mode is not built yet, and an
import the person cannot see is the surprise C6 avoids.

**C8. What "filter profile" means.**
- A. A saved filter over the source's items, applied before fetching
  (types, dates, camera, name, the Library's query syntax).
- B. A Library view (smart album) the imported photos are shown in
  afterwards.
- C. Both, as two fields of the saved import.

Decided (2026-11-11): **A**; "show the import in this view" is the after-import
`select` field, extended later if wanted.

**C9. Items without a hash (Google, MTP) already probably in the
Library.**
- A. Shown "probably in your Library" and not fetched while "New only"
  is on; the person can tick them.
- B. Fetched anyway, deduplicated by BLAKE3 after download.

Decided (2026-11-11): **A** for rendered sources (fetching cannot prove anything
and costs quota), **B** for exact-original sources without a claim
(MTP), where the BLAKE3 settles it.

**C10. Where per-camera settings live.**
- A. A camera body record in the vault (`cameras/bodies`), model and
  serial, holding everything that follows the camera.
- B. In saved imports only: one saved import per body.
- C. Both: the body record holds what belongs to the camera (profiles,
  lens, style, default rename and tags); a saved import holds what
  belongs to the job (source, destination, filter) and wins when both
  set a field.

Decided (2026-11-11): **C**: a body's profile and style apply however its photos
arrive (card, cable, a folder), while a saved import stays a job.

**C11. Two bodies of the same model.**
- A. Model only: both share every default.
- B. Serial only: each body separate; files without a serial get
  nothing.
- C. Layered: model record for all, body record per serial over it;
  without a serial, the model applies unless the person sets a `tell`
  rule (owner name, file prefix, device, card); never a guess.

Decided (2026-11-11): **C**, as Lightroom's raw defaults (model, optionally
serial) and calibration's `@serial` already do.

**C12. When a body's edit defaults (profile, lens, style) apply.**
- A. At import: an edit is written for every imported image.
- B. Recorded at import, applied when the image's edit is created (first
  open or first render), then frozen in the edit.
- C. Live: the image follows the body record until the person touches
  the module.

Decided (2026-11-11): **B**: an import stays fast, an edit renders the same after
the record changes (as calibration's pick is written into the edit), and
"Apply body defaults again" is a command on a selection.

**C13. Device back ends per platform.**
- A. libgphoto2 everywhere.
- B. One back end per platform: libgphoto2 and libmtp on Linux,
  `Windows.Media.Import` then WPD on Windows, ImageCaptureCore on macOS,
  libgphoto2 as a fallback.
- C. Linux only for devices; elsewhere, cameras as mounted cards.

Decided (2026-11-11): **B**, in that order (Linux first); until a platform's
back end exists it is C there, which is what most users do anyway.

**C14. The vocabulary of `Item.metadata`.**
- A. Helios's own field names.
- B. XMP property names (`exif:`, `tiff:`, `exifEX:`, `dc:`,
  `Iptc4xmpCore:`…), reconciled after the fetch by MWG's read rules, the
  connector last.

Decided (2026-11-11): **B**: one vocabulary for connectors, importer values and
the XMP writer, and a plugin author reads a standard, not our docs.

**C15. Where a hook may block the import.**
- A. Nowhere: observers on the bus only, plus declarative rules.
- B. At every stage.
- C. Participants at the stages listed in 7.2, delivered per batch, with
  deadlines; observers everywhere.

Decided (2026-11-11): **C**.

**C16. A participant that fails or times out.**
- A. Fail open always: the item goes on as if the hook passed; reported.
- B. Fail closed always: the item is held as failed.
- C. Fail open by default; a hook declared `required` fails closed.

Decided (2026-11-11): **C**; `required` is shown on the hook in Settings, so a
hook that can stop imports is never a surprise.

**C17. How agents take part in an import.**
- A. Observers only.
- B. Observers, declarative rules they propose and save, and a review
  pause after the decision stage.
- C. Live participants over MCP at each stage.

Decided (2026-11-11): **B**: an agent's latency stays out of the import loop,
and what it decides is data the person can read.

**C18. Who can contribute a connector.**
- A. In-tree modules (Helios's own crates) and out-of-process plugins.
- B. As A, plus third-party dynamic libraries loaded in process.
- C. Plugins only, folders and cards excepted.

Decided (2026-11-11): **A**: in-tree modules give the core a test bed with no
broker, plugins give anyone else isolation; B has no stable ABI and
loses crash isolation.

**C19. The connector registry.**
- A. Its own registry (`helios_connect::registry()`), built the way
  `helios_node::registry()` is, with its own manifest type.
- B. A new kind in the image module registry, with a source "family".

Decided (2026-11-11): **A**: image modules are pure by contract and a connector
does IO; sharing conventions (manifest first, static registry, collected
texts) is enough.

## Sources

- [Google Photos APIs: updates (Library API scope removal, 31 March 2025)](https://developers.google.com/photos/support/updates)
- [Picker API: list and retrieve media items (`=d`, location, 60-minute base URLs)](https://developers.google.com/photos/picker/guides/media-items)
- [Picker API: sessions (pickerUri, polling, maxItemCount 2,000)](https://developers.google.com/photos/picker/reference/rest/v1/sessions)
- [Picker API: mediaItems (fields; no hash, no size)](https://developers.google.com/photos/picker/reference/rest/v1/mediaItems)
- [Google Photos API limits and quotas](https://developers.google.com/photos/overview/api-limits-quotas)
- [Adobe: import photos automatically (Lightroom Classic Auto Import)](https://helpx.adobe.com/lightroom/help/import-photos-automatically.html)
- [ISO 15740:2013, Picture Transfer Protocol (confirmed 2023)](https://www.iso.org/standard/63602.html); [IS&T: PTP standards and PTP-IP](https://www.imaging.org/IST/IST/Standards/PTP_Standards.aspx)
- [USB-IF: Media Transfer Protocol 1.1](https://usb.org/node/1677); [libmtp](https://android.googlesource.com/platform/external/libmtp/+/refs/heads/main/README)
- [Design rule for Camera File system (DCF, CIPA DC-009, JEITA CP-3461B)](https://en.wikipedia.org/wiki/Design_rule_for_Camera_File_system)
- [Windows Portable Devices: application overview](https://learn.microsoft.com/en-us/windows/win32/wpd_sdk/application-overview); [WIA camera support and the WIA-WPD layer](https://learn.microsoft.com/en-us/archive/blogs/wpdblog/wia-camera-support-in-windows-vista-part-1-of-2)
- [Windows.Media.Import](https://learn.microsoft.com/en-us/uwp/api/windows.media.import); [PhotoImportSource (Manufacturer, Model, SerialNumber)](https://learn.microsoft.com/en-us/uwp/api/Windows.Media.Import.PhotoImportSource); [Import media from a device](https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/import-media-from-a-device)
- [ImageCaptureCore: ICDevice (serialNumberString, UUIDString)](https://developer.apple.com/documentation/imagecapturecore/icdevice)
- [GVfs(7): the gphoto2 and MTP backends and volume monitors](https://man.archlinux.org/man/gvfs.7); [KIO and MTP](https://mail.kde.org/pipermail/kfm-devel/2015-December/016881.html); [Arch wiki: MTP](https://wiki.archlinux.org/title/MTP)
- [Android: MediaStore](https://developer.android.com/training/data-storage/shared/media); [Storage Access Framework](https://developer.android.com/guide/topics/providers/document-provider)
- [Adobe: XMP specifications (part 3, storage in files)](https://developer.adobe.com/xmp/docs/xmp-specifications/)
- [IPTC Photo Metadata Standard](https://iptc.org/standards/photo-metadata/iptc-standard/); [version 2024.1](https://iptc.org/news/photo-metadata-standard-updated-to-version-2024-1/)
- [Metadata Working Group](https://en.wikipedia.org/wiki/Metadata_Working_Group); [ExifTool's MWG tags (link to the archived guidelines)](https://exiftool.sourceforge.net/TagNames/MWG.html)
- [ExifTool: Exif tags (BodySerialNumber, LensModel, CameraOwnerName)](https://exiftool.org/TagNames/EXIF.html)
- [Adobe: raw defaults per camera model and serial number](https://helpx.adobe.com/au/lightroom-classic/desktop/help/raw-defaults.html); [a DCP matched by UniqueCameraModel (Adobe community report)](https://community.adobe.com/questions-563/xiaomi-17-ultra-telephoto-dng-is-not-correctly-identified-by-camera-raw-1630240)
- [lensfun: adding a camera (Exif maker and model, mount, crop factor)](https://lensfun.github.io/manual/latest/addcamera.html); [lens calibration (crop factor of the measuring body)](https://lensfun.github.io/calibration-tutorial/lens-calibration.html)
- [darktable: import module (copy & import from camera)](https://docs.darktable.org/usermanual/development/en/module-reference/utility-modules/lighttable/import/); [tethering through gphoto2](https://docs.darktable.org/usermanual/5.6/en/tethering/overview/)
- [darktable Lua: pre-import](https://docs.darktable.org/lua/stable/lua.api.manual/events/pre-import), [post-import-image](https://docs.darktable.org/lua/stable/lua.api.manual/events/post-import-image), [post-import-film](https://docs.darktable.org/lua/stable/lua.api.manual/events/post-import-film)
- [digiKam: no WIA back end yet on Windows](https://bugs.kde.org/show_bug.cgi?id=462225); [PTP and MTP cameras not supported on Windows, mass storage only](https://bugs.kde.org/show_bug.cgi?id=416392)
- [Shotwell: use GVfs rather than libgphoto2](https://gitlab.gnome.org/GNOME/shotwell/-/issues/1643)
- [Lightroom Classic: request for import hooks in the SDK](https://community.adobe.com/feature-requests-676/p-import-actions-666348); [Folder Watch plugin (polling)](https://regex.info/blog/lightroom-goodies/folder-watch)
- [KIPI, the KDE Image Plugin Interface](https://en.wikipedia.org/wiki/KIPI)
- Crates and licences as listed on crates.io (2026-11-11): `gphoto2`
  LGPL-2.1-only, `libmtp-rs` MIT, `opendal` Apache-2.0, `aws-sdk-s3`
  Apache-2.0, `keyring` MIT or Apache-2.0, `oauth2` MIT or Apache-2.0,
  `notify` CC0-1.0, `smb` MIT, `windows` MIT or Apache-2.0.
