# Replicas and sync: a laptop and a workstation that never lose anything

Status: **decided** (2026-11-11). The owner took every default and every
recommended answer of section 9 (Q1: the folder mailbox first, then the
network link). Building is parked until sync is needed; the slices of
section 8 are the plan when it is.
The inventory describes the code on `next` as of that date.

ROADMAP 2.3 took git's *model* for the library (2026-10-10): content
addressed objects, a graph of versions, refs, offline replicas, integrity;
raws stored git-annex style, beside the history and never in it; ratings,
labels and tags merged per field. It added that photos are not code (two
concurrent edits stay two versions, 2026-10-11), that every machine is a
node with replicas and caches, that backup runs all the time and never
behaves like sync, and that deleting is reclaiming space, never the last
copy. It left the model "not decided". This note turns it into decisions
and orders the work.

**The first target.** A travel laptop and a home workstation, each
offline at times, each importing, rating, tagging and editing on its own,
reconcile when they meet, and nothing is lost: no rating, no tag, no
edit, no file.

## 1. What exists

| Piece | Where | State |
|---|---|---|
| Identity by content (BLAKE3 of the file) | `crates/helios-vault/src/lib.rs` (`ImageId`, `Image`) | built |
| Copies as data: locations with volume, path on it, presence, last verified | `crates/helios-vault/src/lib.rs` (`Location`, `Presence`), `crates/helios-storage/src/scan.rs` | built |
| Volumes known offline, the node a fixed disk belongs to, last seen where | `crates/helios-storage/src/lib.rs` (`Volume`), `registry.rs`, kept in the vault (`crates/helios-vault/src/storage.rs`) | built |
| A file changed in place keeps its image; earlier contents kept | `crates/helios-vault/src/changed.rs` | built |
| Field commands (set, set if absent, add, remove), folded on the stored value, audited | `crates/helios-vault/src/fields.rs`, `catalog.rs` (`Vault::apply`) | built |
| Versions addressed by content, parents, heads, newest head on open | `crates/helios-vault/src/versions.rs`, `edit.rs` | built |
| Concurrent edits kept as heads side by side (darktable duplicates already land so) | `versions.rs` (`heads`), `contract.rs` | built |
| One store contract, two drivers (memory, SQLite), transactions | `crates/helios-vault/src/lib.rs` (`Store`), `contract.rs`, `crates/helios-vault-sqlite` | built |
| Backup strategy, copies counted per drive, verified by hash | `crates/helios-storage/src/strategy.rs` | built |
| The `backup` job: part file, hash, read back, rename, never overwrite, pause | `crates/helios-library/src/backup.rs` | built |
| Catalogue snapshots, verified, copied to backup disks, restore | `crates/helios-vault-sqlite/src/snapshot.rs`, `crates/helios-gui/src/catalogue.rs` | built |
| Storage page (volumes, roles, off site, RAID), backup badges and filters | `crates/helios-gui/src/shell/storage.rs`, `storage_admin.rs`, docs/library.md | built |
| Health checks: strategy set and met, destinations seen, copies verified, cards, snapshots | `crates/helios-health/src/backup.rs`, `snapshots.rs`, `checks.rs` | built |
| Cache service: kinds, tiers, budgets, LRU with pins, single flight | `crates/helios-cache` ([cache.md](cache.md)) | slices 1 to 3 built |
| Node control endpoints: JSON lines over a Unix socket or loopback TCP with a token | `crates/helios-inspect/src/transport.rs`, `crates/helios-node/src/main.rs`, `mcp.rs` | built, local only |
| Replica identity, change log, merge, exchange, object transfer, pins, reclaim, scrubbing | none | not started |

In short, as the earlier reanalysis found: **backup** is largely built;
**placement** is half built (every copy is tracked by hash and volume,
nothing says which copies a machine should hold); the **ground for
merges** is ready (writes are field commands, edits are content-addressed
versions); **replicas and exchange** are not started.

Two gaps in what exists matter here. The audit's sequence number is the
store's own (`AuditEntry::seq`), so it orders one replica's changes, not
two. And `Vault::apply` keeps the resulting value only: who won a field,
and when, is not kept beside it, which a merge needs.

## 2. Prior art, briefly

- **git** (Torvalds, 2005): objects by hash, a graph of commits, refs,
  fetch and push by "what do you have, what do I want". Borrowed: content
  addressing, union of immutable objects, the have/want exchange. Not
  borrowed: one tree per commit (a library is a million independent
  records, not one tree), branches as the user's notion.
- **git-annex** (Hess, 2010): large files kept out of the history, a
  location log saying which repository holds which key, `numcopies`, a
  `drop` that first checks live that enough other copies exist,
  "preferred content" expressions saying what a repository wants.
  Borrowed: all four (sections 3, 6, 7).
- **Syncthing**: devices exchange an index of files with per-device
  sequence numbers and version vectors, send only what changed since the
  last sequence seen, and reset an "index ID" when a device's database is
  rebuilt; device IDs from keys, explicit pairing. Borrowed: per-replica
  sequence numbers, frontiers as version vectors, the index ID on restore
  (section 4). Not borrowed: syncing whole files with last-writer-wins
  and conflict copies, which is what photographers get today when they
  sync XMP sidecars with it.
- **Lightroom (cloud sync and Classic's Smart Previews)**: one cloud
  catalogue, originals uploaded, devices keep proxies and fetch originals
  on demand, "make available offline"; Smart Previews let Classic develop
  a photo whose drive is unplugged. Borrowed: proxies everywhere and
  originals on demand, the "make available offline" verb (section 6).
  Not borrowed: a server as the only source of truth (no offline
  reconcile between two desktops), whole-record last-writer-wins.
- **darktable's XMP sidecars**: the edit and marks travel beside the
  raw, any tool can copy them, and the newer file wins on the next start
  (`look for updated XMP files`). Borrowed: the exit (the state can be
  written beside the files, ROADMAP 2.3). Not borrowed as a sync channel:
  a sidecar is one record written whole, so a rating on one machine and
  a tag on the other cannot both survive, and two edits become one.
- **CRDTs** (Shapiro, Preguiça, Baquero, Zawirski, 2011): a
  last-writer-wins register and an add-wins observed-remove set converge
  whatever order replicas exchange in. Borrowed for fields (section 5).
  Hybrid logical clocks (Kulkarni, Demirbas et al., 2014) order writes
  without trusting wall clocks.

## 3. What a replica is

**Decision.** A replica is **one vault plus the objects it can reach**.
The vault part is **full**: every replica holds every image's record, its
fields, every version of its edit, its stacks and the replicated
documents. The object part is **partial**: a replica holds the raws its
volumes carry (its own internal disks, the removable disks plugged in),
and every replica holds the previews (`thumb.256` for all, `thumb.1024`
for what it pins), so the whole Library browses, filters and culls
offline. Each replica has an id (a random UUID made when the vault is
created or cloned, document `sync/replica`) and a name (the computer's
host name, which `Volume::node` already uses).

Replicated documents are an explicit list: `storage/volumes` (the
volumes, so the laptop knows the workstation's disks), `storage/strategies`,
`storage/verified`, `content` (files changed in place), the user and
project settings layers, stacks, snapshots, presets and styles, pins
(section 6). Node-local, never replicated: sightings and folders (the
image map is what this machine's scans saw), workflow journals and runs
(a node's work), caches.

A location is replicated when it names a volume (the path inside the
volume is the same everywhere; the absolute path is recomputed from where
that volume is mounted here). A location on no volume yet stays local
until a scan links it. A copy on another computer's internal disk reads
as offline here, which `Presence` already says ("an internal disk of
another computer sharing the vault").

**Default.** Full catalogue, partial objects, previews everywhere.

**Rejected.**
- *Partial catalogues* (a laptop holding only some photos' records):
  merges would have to reason about records a replica never saw, and the
  catalogue is small (records and versions are text; a 200 000 photo
  library is a few hundred MB). Possible later as a pin on records, not
  needed for the first target.
- *One shared SQLite file on a NAS*: no offline work, and SQLite over a
  network file system is unsafe for writers on two machines.
- *A server as the source of truth*: no reconcile between two machines
  that meet without it. A server is welcome as one more replica that is
  always on (section 4).
- *git itself as the store*: one tree per commit over a million records,
  and raws must stay out of it; git-annex shows the raw side works, but
  its records would be files in a tree, losing queries and cursors.

**To build.** The replica id and name; the list of replicated
namespaces; cloning (section 4).

## 4. How two machines exchange

**Decision: a change log per replica, exchanged by frontiers.** Every
write to the replicated part (a field command, a version, a library flag,
a stack change, a replicated document, a location on a volume) appends a
**change** to the replica's log, in the same store transaction as the
write: the replica id, a counter increasing by one, a hybrid logical
clock stamp, who and which job (as the audit already records), and the
operation as given (the command, not only the value it produced). The
audit stays the user-facing record; the log is what replicas exchange.

**Refs are frontiers.** There are no branches. What a replica knows is a
**version vector**: for each replica, the highest counter received from
it. Each replica keeps, per peer, the vector it last saw from that peer
(documents `sync/peers/<replica>`). An image's heads stay what
`versions::heads` computes: nothing names them.

**Pull and push.** A session is:

1. `hello`: each side gives its replica id, its index ID (below) and its
   vector;
2. each side sends the changes the other lacks, per replica, in counter
   order (pull and push are the same request seen from either side);
3. the receiver applies them through the merge rules (section 5), each
   batch in one transaction, then raises its vector: a session cut halfway
   resumes where it stopped, and receiving a change twice changes nothing
   (versions are already idempotent by content; the counter guards the
   rest);
4. **objects** follow separately and lazily: the placement job (section
   6) asks `want` with the hashes it should hold, the other side answers
   which it has, `get` sends the bytes (whole or a range), and the
   receiver writes a part file, hashes while writing, reads it back,
   checks the BLAKE3 hash and renames it, as the backup job does. A
   received copy is a location like any other, recorded as verified.

**Index ID.** A replica restored from a snapshot, or cloned, takes a new
replica id, so its counters can never collide with changes it made
before the snapshot and already sent (Syncthing's index ID solves the
same problem). **Cloning** a new replica is: copy a verified catalogue
snapshot (what `snapshot.rs` already makes), give it a new id, then pull
what came after the snapshot's vector.

**Transport: an abstraction, several implementations.** The session
(`hello`, `changes`, `want`, `get`) is one protocol; *how* its bytes reach
the other machine is a `SyncTransport` chosen per peer, so the user picks
what works on their network and none is required. The session layer
never knows which one runs. A transport gives one of two shapes:

- a **link**: a duplex stream to a live peer (JSON lines, object bytes as
  a length-prefixed binary frame after a JSON header line, not inside
  JSON);
- a **mailbox**: a place both machines can write and read at different
  times, holding **bundles** (the changes after a vector, plus chosen
  objects, as git bundles do) and each side's last vector.

Implementations, each behind the same trait and tested against the same
contract (the session over an in-memory link and an in-memory mailbox):

- **Folder** (mailbox): any folder both machines see: a USB disk, a NAS
  share, a folder synced by Syncthing, Dropbox or OneDrive. Nothing to
  configure but a path, nothing listening, works offline by nature.
  Bundles are written to a part file and renamed, so a half-synced folder
  is never read as complete.
- **Local network** (link): the node's endpoint opened on the LAN with
  TLS, device keys and a pairing code shown on one machine and typed on
  the other, peers found by mDNS (Syncthing's model). No account, no
  server, no SSH.
- **Tailscale or another mesh VPN** (link): the same network link,
  reached at the peer's tailnet address (MagicDNS name) instead of found
  by mDNS, so two machines on different networks (home and travel) sync
  live with no port opened and no relay of ours; Tailscale's WireGuard
  already authenticates the machines, so pairing only binds the two
  Helios replicas. Nothing Tailscale-specific in the protocol: a peer
  address the user types or picks from `tailscale status`.
- **SSH** (link): `ssh <remote> helios sync-serve --stdio`, for users who
  already have SSH between their machines; authentication and encryption
  are SSH's, as git and git-annex do.
- Later, if wanted: a relay of our own, only if a mesh VPN does not cover
  the case.

The node's MCP server gets `helios_sync_status` and `helios_sync_now` so
an agent can see and start a session, whatever the transport.

**Offline.** Nothing waits for the other side. Every replica writes to
its own log and its own vault as today; a session happens when the peer
is reachable (by hand, at start, every few minutes while reachable) or,
with a mailbox, whenever the folder is there. The Health check reports
each known replica's last session ("studio: last synced 12 days ago, 340
photos only on this laptop").

**Default.** Change log with counters and HLC stamps; frontiers as
version vectors; the transport an abstraction (`SyncTransport`, link or
mailbox) with the folder mailbox built first and the local-network link
second (section 9); sessions at start, by hand, and every five minutes
while the peer answers.

**Rejected.**
- *Comparing whole states* (send every record, or a Merkle tree of them):
  simpler to start, but merging needs to know what each side did, not
  only what it holds; and a full scan of a large catalogue at every
  session costs more than reading a log after a counter.
- *git refs and branches per image*: they solve naming of lines of work,
  which Helios does not expose; heads already come from the graph.
- *Syncing the SQLite file or XMP sidecars with a file-sync tool*: one
  side's whole record wins, the other's changes are lost or left as a
  conflict copy.
- *One fixed transport* (SSH only, or the network only): SSH is hard to
  set up for many users, especially on Windows, and a network link fails
  where a shared folder works (owner, 2026-11-11: the transport is an
  abstraction with a choice of implementations).

**To build.** The log table and its contract tests on both drivers; the
HLC; the vectors and the session (in process first, between two vaults);
the `SyncTransport` trait with its contract tests; the folder mailbox;
the `sync` scope and the local-network link; the object frame; the
Health check.

## 5. Merges

**Decision: per field, by the field's kind, built on the existing
commands.** The commands already say what changes rather than "here is
my copy" (`fields::Op`: `Set`, `SetIfAbsent`, `Add`, `Remove`). Merging
gives each kind a rule that converges whatever the order of arrival:

| What | Kind | Rule |
|---|---|---|
| `rating`, `rejected`, `picked`, `title`, `description` | register | last writer wins by HLC stamp, ties by replica id; the value it replaced on the other side is kept in the audit as "replaced by sync from <replica>" |
| `labels`, `tags` | set | add-wins observed-remove set: a remove takes out only the adds it saw, so a tag added on the laptop survives a remove on the workstation made without seeing it. `Add` and `Remove` map one to one; a `Set` on a set field is logged as the removes of what it saw and the adds of what it puts |
| any register, `SetIfAbsent` | default | applies only while no plain write exists: a sidecar's rating imported on one side never beats a rating a person gave on the other |
| Library flag | register, add-wins | an image put back in the Library on one side and taken out on the other stays in: keeping beats removing |
| versions | grow-only set | the union; two concurrent edits are two heads, never merged (decided 2026-10-11). Open shows the newest head as today and says the photo has two edits from two computers; the user keeps one or both. Setting one aside is a mark on the version, itself replicated; no version is removed by sync |
| stacks | set of members, register for the top | add-wins membership; the top by last writer |
| `storage/verified` | grow-only | a verification is a fact: the newest time per copy wins |
| volumes | register per field | a name or role by last writer; `last_seen` the newest |
| strategies, settings layers, presets, styles | register per key | last writer wins per key; a style or macro file is never rewritten (a new version is a new key), so nothing is lost there |
| locations | register per (volume, path on it) | the newest verification wins; a copy seen missing on one side is missing until seen again |

To apply this, the store keeps, beside each field's value, the stamp of
the write that set it, and for set fields the adds still alive (each add
with its replica and counter). `Vault::apply` does that for local writes
too, so the local path and the remote path are one path. The rule that a
command equal to the stored value writes nothing stays for registers; for
sets an add of a name already there still records its add (it is what
keeps that name alive against a remove made elsewhere).

**Default.** As in the table: last writer wins for single values, with
the replaced value kept in the history; add-wins sets for labels and
tags; versions never merged.

**Rejected.**
- *Last writer wins on the whole record* (what an XMP sidecar does):
  loses concurrent changes to different fields.
- *Asking the user for every concurrent field change*: two marks on the
  same photo during a trip are common and almost never matter; asking
  would turn every reconcile into a chore. Kept as an option for ratings
  (section 9).
- *Merging edits by parameter* (taking the laptop's exposure and the
  workstation's crop): an edit is a whole; ROADMAP decided against
  semantic merges (2026-10-11).
- *Wall-clock time to order writes*: a laptop's clock off by a few
  minutes would decide which rating wins. The HLC keeps causality and
  stays close to wall time.

**To build.** The stamps and set adds in the store (a schema step on
SQLite, the memory store, the contract); `Vault::apply` writing them and
the change log; the receiving side applying remote changes by these
rules; a convergence test: two replicas, random concurrent writes,
exchanged in any order, end equal.

## 6. Placement and pinning

**Decision.** Which raws a replica holds is decided by **pins**, rules
that say "this computer keeps these photos' originals", in the spirit of
git-annex's preferred content. A pin is a Library query (a saved search,
a collection, a selection, later a shoot) or a list of images, kept as a
replicated document per replica (`sync/pins/<replica>`), so the
workstation sees what the laptop wants and holds.

**The travel laptop by default holds:**

1. every photo imported on it, until the photo has its verified backups
   (strategy met) and has reached another replica: it is the only copy
   during the trip;
2. what it pins ("Make available offline" on a selection or a
   collection, Lightroom's verb);
3. previews for everything else (`thumb.256`, already in the library's
   disk cache scope), so the whole Library browses, rates, tags and
   searches offline. A photo whose raw is not here can be marked but not
   developed: opening it says "Not on this computer: on studio (Library
   disk)" as offline photos already do.

**The workstation by default holds** everything its library volume and
disks hold today: it pins the whole Library.

**Placement job.** When a pin is added or a peer is reachable, a
background job (throttled, low priority, cancellable, through the
scheduler) asks peers for the pinned objects it lacks and writes them to
its library folder, laid out as import does. A pinned copy is durable: it
is a location on a library volume. It is **not** a backup copy (only
backup and archive roles count, `strategy::status`), so pinning never
makes a card safe to format.

**Cache.** An original fetched to open one photo once, without a pin, is
a cache entry: a kind `object.raw` in the cache service's disk tier,
user cache scope, evictable by its LRU, never counted as a copy and never
in the reclaim guard. Unpinning turns pinned copies into reclaimable ones
(section 7), never deletes them at once.

**Default.** Laptop: own imports until safe elsewhere, plus pins, plus
previews. Workstation: everything. "Make available offline" on the
selection and collections; a pinned collection follows its members.

**Rejected.**
- *Full mirror everywhere* (Syncthing's default): a laptop cannot hold a
  workstation's terabytes.
- *Previews only, nothing kept unless pinned* (Lightroom's cloud model):
  the photos of the trip itself would sit on the laptop as one copy
  that the system thinks it may drop.
- *Pins as a per-photo flag field*: a pin on a query follows new photos
  (a collection that grows), a flag does not; and a pin belongs to a
  replica, not to the photo.

**To build.** Pin documents and their queries; the placement job and its
`want`/`get` client; the `object.raw` kind; "Make available offline" and
"Free up space on this computer" commands; the offline photo's message
naming the replica.

## 7. Backup and reclaim

**Decision: backup stays what it is, made continuous.** The `backup`
job, its strategy, its verification and its write permissions do not
change. Continuous means three more triggers and one more job:

- **Whenever a destination is available**: a destination plugged in or
  a share reachable runs the job (exists: `Trigger::OnConnect`), and a
  change of what is short (a new import, a copy found missing) queues it
  again, throttled.
- **Copies made by other replicas count**: `storage/verified` and the
  locations replicate, so the laptop sees that the workstation backed up
  its trip; its badges and "safe to format" follow after a session.
- **Scrubbing**: a re-check job reads back a slice of copies each day
  (oldest verification first), records the new time, and marks a copy
  whose hash no longer matches as damaged; **repair** copies it again
  from a good copy. Nothing is overwritten: the damaged file is moved
  aside. A file changed in place is new content, not damage
  (`changed.rs`): its old content's copies stay, which is also what
  protects against ransomware (ROADMAP: "a ransomware-encrypted file is
  a new hash").
- **Never like sync**: removing a photo from the Library on one replica
  replicates as a flag, hides it on the other, and deletes no file
  anywhere. Files go only through reclaim, on the replica that holds
  them.

**Decision: reclaim is a job with a dry run, and never removes the last
copy.** "Free up space" on a volume, or the health check's free-space
fix, runs it. Its order is the ROADMAP's:

1. caches (the cache service shrinks its disk scopes; `object.raw`
   first);
2. copies not pinned on this replica, above the strategy's count;
3. photos out of the Library past their retention (tombstones), with
   a confirmation that names them.

A copy is removed only when, **at that moment**, enough other copies are
confirmed: on a volume online here, read back now; or on another replica
reachable now, which confirms holding the verified hash and keeps it for
the duration of the removal (git-annex's `drop` locks the remote copy the
same way). A copy known only from an old record of an offline replica
does not count: the job says "connect to studio to free 40 GB" instead.
Below the strategy's count, or for the last durable copy, it refuses;
dropping a raw for a proxy (retention tiers) stays a later, visible,
confirmed step.

**Default.** Backup triggers: after import, on connect, on any shortfall,
by hand. Scrub: every copy re-read at least once every 90 days, in daily
slices. Tombstone retention: 30 days. Reclaim: dry run shown first, live
confirmation required.

**Rejected.**
- *Propagating deletions* (sync semantics): one mistake or one infected
  machine would empty every disk.
- *Trusting recorded copies for reclaim*: a backup disk may have died in
  a drawer since it was last verified.
- *Reclaiming automatically when a disk fills*: dropping a copy is never
  silent; the free-space check proposes it, the user starts it.

**To build.** The shortfall trigger; the scrub job and repair; the
reclaim job with its dry run and its live confirmation over the `sync`
scope (`sync.hold` / `sync.release` on a hash); tombstone retention.

## 8. Build slices

Each slice is small, keeps every existing test, and is tested headless:
two vaults in one process first (memory and SQLite through the store
contract), then two `helios-node` processes over a socket pair, never a
real network or a second machine.

1. **Replica id and clock.** `sync/replica` document, the HLC, a new id
   on clone and on snapshot restore. Tests: the id survives reopening;
   a restored snapshot gets a new id; the clock never goes back.
2. **Change log.** Every replicated write appends its change in the same
   transaction; the store contract checks it on both drivers. Tests: a
   rolled back batch leaves no change; counters have no gaps; a no-op
   command on a register logs nothing.
3. **Merge rules.** Stamps and set adds in the store; applying a remote
   change by section 5's table. Tests: per rule, the classic cases (tag
   added here, removed there; two ratings; `SetIfAbsent` against a
   rating); a property test that random concurrent writes exchanged in
   any order converge on both drivers.
4. **Sessions in process.** Vectors, `hello`, changes after a vector,
   resume after a cut. The first target as a test: laptop and
   workstation diverge offline (imports, ratings, tags, two edits of one
   photo), reconcile, and every write is there on both, the two edits as
   two heads.
5. **The transport trait and the folder mailbox.** `SyncTransport`
   (link and mailbox), its contract run over in-memory fakes, then the
   folder implementation: bundles written as part files and renamed, the
   peer's vector read back; the MCP tools. Tests: two vaults syncing
   through a temporary folder; a bundle cut halfway is ignored.
   Later slices add the local-network link (TLS, pairing, mDNS) and the
   SSH link, each passing the same contract.
6. **Objects.** `want` / `get` with the binary frame, part file, hash,
   read back, rename; the received copy recorded as a verified location.
   Tests: a corrupted frame is refused and leaves nothing; a resumed
   transfer.
7. **Pins and placement.** Pin documents, the placement job, the default
   laptop rule, `object.raw` in the cache service. Tests: a pinned
   collection gains a photo and the job fetches it; unpinning keeps the
   files.
8. **Continuous backup and scrub.** The shortfall trigger, the scrub
   job, repair. Tests: a flipped byte is found and repaired from another
   copy; the damaged file is kept aside.
9. **Reclaim.** Dry run, the order, the live confirmation. Tests: every
   refusal (last copy, below strategy, peer offline, peer confirms then
   vanishes mid-removal).
10. **Faces.** Computers on the Storage page (each replica, last session,
    photos only there), the Health check, "Make available offline" and
    "Free up space" commands, the offline photo's message. Tests through
    accesskit, headless, as the Storage page's are.

Slices 1 to 4 reach the first target without a network; 5 and 6 make it
work between two machines; 7 to 10 make it comfortable.

## 9. Open questions for the owner

Each option is listed in order; the recommendation is marked.

1. **Which transport implementations first** (the transport is an
   abstraction either way, owner 2026-11-11).
   - A. The folder mailbox first (USB disk, NAS, a synced cloud folder:
     a path and nothing else), then the network link with pairing, which
     also runs over Tailscale (recommended: works for everyone at once,
     no network setup).
   - B. The local-network link first (live sync at home, and across
     networks through Tailscale), then the folder.
   - C. SSH first, for users who already have it, then the others.
2. **Two different ratings for one photo, set on two machines between
   two sessions.**
   - A. Last writer wins; the other value is in the photo's history
     (recommended: rare, cheap to undo, no chore).
   - B. Flag it as a conflict to review in a panel.
   - C. Keep the higher rating.
3. **Does a copy on another computer's library disk count as a backup
   copy?**
   - A. No: only volumes with the backup or archive role count, as today
     (recommended: one rule, and "safe to format" stays strict).
   - B. Yes, once verified, as one copy (the workstation is the trip's
     backup).
   - C. Only when the user gives that disk the backup role on the
     Storage page.
4. **What the laptop keeps by default.**
   - A. Its own imports until safe elsewhere, its pins, previews for the
     rest (recommended).
   - B. Everything that fits, newest first.
   - C. Previews only; originals only when pinned.
5. **Developing a photo whose original is not on this computer.**
   - A. Not in the first version: it can be rated, tagged and searched,
     not developed (recommended).
   - B. A proxy original (a reduced raw or linear DNG) kept for pinned or
     recent photos, as Lightroom's Smart Previews.
   - C. Rendered remotely on the replica that holds it, when reachable.
6. **Retention of photos taken out of the Library before reclaim may
   remove their files.**
   - A. 30 days (recommended).
   - B. 90 days.
   - C. Never automatic: reclaim lists them, the user confirms each time.
