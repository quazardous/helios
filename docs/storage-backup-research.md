# Storage and backup: volumes, roles and strategies

Status: study (2026-11-07). Nothing here is decided; proposals are
marked "not decided" until the owner settles them. It answers: how does
Helios know the user's storage (internal and external disks, RAID and
NAS, USB sticks, SD cards), how does the user say what each one is for,
and how does the library keep photos backed up, with strategies that
may differ from one shoot or card to the next?

Three directions from the owner frame it (2026-11-07):

- Helios is in contact with all of a user's storage, and the user
  describes how the library uses each volume.
- Backup is not an import gesture. It is a library-level strategy, and
  strategies may differ per shoot or card: not every reportage is
  treated the same.
- The import only reminds the user to set up a strategy when there is
  none, with one dismissible line, never a blocking modal window.

The study looks at prior art (section 1), proposes a storage model
(section 2) and backup strategies (section 3), checks the fit with what
Helios has (section 4), and ends with a recommendation, a phased plan
and a roadmap proposal (section 5). Sources are linked where they are
used; vendor pages are marked when they speak of their own products,
forum threads are evidence of pain, not of fact.

## In short

- **The market splits backup in two and gets both halves partly
  wrong.** Ingest tools (Lightroom's "Make a Second Copy To", Capture
  One's "Backup to", Photo Mechanic's secondary destination, Rapid Photo
  Downloader's backup devices) make a copy once, at import, and then
  forget it: no status, no re-check, no notion of "this shoot has 2 of
  its 2 copies". General backup tools (Time Machine, restic, Borg,
  rsync) verify and schedule well but know nothing of photos, shoots or
  cards.
- **Helios already holds the hard part**: identity by content hash and
  a list of locations per image. A backup copy is one more location.
  "Backed up 2/2" is a count over locations, verified by hash, which no
  photo application can say today.
- **Volumes become first-class.** A volume has a stable identity
  (filesystem UUID, else a label and size, a share's server and name), a
  kind, its capacity and free space, and online or offline. Locations
  point at a volume plus a path inside it, so a drive letter or mount
  point that changes no longer loses photos (Lightroom's most common
  "missing photos" cause).
- **Offline is not missing.** A photo on an unplugged drive stays in the
  library, with its preview and edits, flagged "on BACKUP_2 (offline)".
  Today `repair` marks such a location not present, and thumbnails are
  read from the raw itself: both must change. A preview cache in the
  library is a v1 prerequisite.
- **Users give volumes roles** (library, archive, backup, ingest,
  scratch), several per volume. Helios writes only to volumes with a
  writing role, only in its own folder there, never to a card.
- **A strategy** names destinations (by role or by volume), how many
  verified copies, when it runs (on import, on close, on a schedule, by
  hand, when a destination is plugged in) and how copies are re-checked.
  The library has a default (which can be "none"); shoots, import
  sessions and cards can override it.
- **"Safe to format"** follows the card's strategy: every photo in the
  library folder and in every copy the strategy asks for, verified.
- **NAS and cloud.** A mounted share is a volume: the core copies to
  it. Protocols that are not mounted (SFTP, S3, B2, WebDAV) are
  destination plugins; the core still owns the copy, the verification
  and the status.
- **Catalogue backup is part of it.** The vault already takes verified
  SQLite snapshots; v1 copies the newest one to every backup volume,
  because Helios's edits live in the catalogue, not beside the photos.
- **v1**: volumes with identity and offline state, roles, one local
  backup destination, a library default strategy, verified copies run
  as a job, status per photo and per import, a filter, the import
  reminder, safe to format by strategy, catalogue snapshots copied
  along.

## 0. What Helios has today

From the code of `helios-vault`, `helios-vault-sqlite` and
`helios-library`, docs/library.md and docs/design/import-window.md:

- **Identity and locations.** An image is the BLAKE3 hash of its file.
  It has any number of `Location`s: `path`, `size`, `modified`,
  `last_verified` and `present`. The ROADMAP already says file copies
  are "just information", used for "redundancy count, offline
  availability and which drive has it" (section 2.3, 2026-10-11).
- **No volume.** A location is an absolute path; nothing records which
  disk it is on. `Vault::repair` marks a location not present when the
  file cannot be found, which is the same answer for "the drive is
  unplugged" and "the file was deleted".
- **One library folder.** A copy-import writes `YYYY/YYYY-MM-DD/<name>`
  under one library root (`library_root`, kept in the vault), hashing
  as it writes and verifying by a second read before the file takes its
  name. Nothing is overwritten.
- **Safe to format**, per card (`helios_library::import::Card`): every
  photo of the card has a present location under the library root, by
  hash. It knows nothing of backups.
- **A lost source pauses** the import job, and reinserting the card
  resumes it.
- **Undo of an import** removes the library entries it added (images,
  locations, fields, signals, versions) and never touches a file.
- **Card detection**: the import window branch adds a `Volumes` trait
  and a `Watch` poller (in `helios-gui`): removable volumes holding
  `DCIM`, with a label (the mount folder's name), a root and a
  capacity, read from `/proc/mounts` under `/run/media/$USER` and
  `/media` on Linux, none elsewhere yet.
- **Catalogue snapshots**: `helios-vault-sqlite` takes verified
  snapshots through SQLite's backup API (`quick_check`, synced, then
  renamed), five kept, next to the database, today before a schema
  migration.
- **No preview cache.** The grid reads embedded previews from the
  files themselves, in the background. A photo whose file is not
  reachable has no thumbnail.
- **No backup copy** of any kind: the import window's design leaves it
  "for later, in a ticket of its own", which this study is.

## 1. Prior art

### 1.1 The 3-2-1 rule

Three copies of the data, on two kinds of media, one of them off site.
It comes from a 2012 US-CERT paper, *Data Backup Options*
([CISA](https://www.cisa.gov/sites/default/files/publications/data_backup_options.pdf)),
and is the rule every photography backup guide starts from
([Backblaze](https://www.backblaze.com/blog/the-3-2-1-backup-strategy/),
vendor). Vendors now extend it against ransomware (3-2-1-1-0: one
immutable or air-gapped copy, zero errors on verification,
[NAKIVO](https://www.nakivo.com/blog/3-2-1-backup-rule-efficient-data-protection-strategy/),
vendor).

What Helios takes from it:

- the unit is **a count of copies on distinct volumes**, not "a backup
  was made once";
- "two media" and "off site" are properties of volumes, which only the
  user can state (Helios cannot see that two USB disks sit in the same
  bag);
- "0 errors" means a copy counts only once verified, and keeps counting
  only while re-checks pass.

### 1.2 Lightroom Classic

- **Second copy on import.** "Make a Second Copy To" copies the
  originals, as they came off the card, to another folder during import
  ([Adobe: import options](https://helpx.adobe.com/lightroom-classic/desktop/import-photos/photo-video-import-options.html)).
  The copy is not tracked afterwards: the catalogue does not know it
  exists, edits are not in it, and nothing says whether it was verified.
- **Catalogue backup.** A separate setting (never, each exit, daily,
  weekly, monthly) copies the catalogue file on exit, zipped, with an
  integrity test and an optimise; photos are not included
  ([Adobe: back up a catalog](https://helpx.adobe.com/lightroom/help/back-catalog.html),
  [Lightroom Queen](https://www.lightroomqueen.com/catalog-back-frequency-keep/)).
  Users end up with hundreds of old backups because nothing prunes them.
- **Offline volumes.** The Folders panel lists volumes with their free
  space; an unplugged volume's name is greyed and its folders carry a
  question mark; photos stay in the grid with their previews, and
  editing works on Smart Previews. "Find All Missing Photos" makes a
  collection of them, and "Find Missing Folder" relinks a folder at
  once
  ([Adobe: locate missing photos](https://helpx.adobe.com/lightroom-classic/desktop/manage-catalogs-and-files/locate-missing-photos.html),
  [Lightroom Killer Tips](https://lightroomkillertips.com/when-photos-go-missing/)).
- **Pain.** A drive letter that changes on Windows, or a volume renamed
  on macOS, makes everything on it "missing"
  ([Adobe community](https://community.adobe.com/t5/lightroom-classic-discussions/suddenly-2556-of-4206-of-my-image-files-in-lightroom-are-now-quot-missing-quot/td-p/13338402),
  [Lightroom Queen: missing fix](https://www.lightroomqueen.com/lightroom-photos-missing-fix/)):
  locations are paths, not volumes.

Lesson: offline handling is the model to follow (greyed, previews kept,
relink by folder); the second copy is the one to avoid (fire and
forget). Identity by volume, not by path, removes the most common pain.

### 1.3 Capture One

- **Sessions and catalogues.** A session is a folder per job (Capture,
  Selects, Output, Trash) with its own small database, carried with the
  job; a catalogue is a library across jobs, managed or referenced
  ([Capture One: catalogues and sessions](https://support.captureone.com/hc/en-us/sections/360001078997-Catalogs-and-Sessions),
  [sessions 101](https://www.captureone.com/blog/sessions-101-discussing-the-concept),
  vendor). Photographers use sessions exactly for "this job is treated
  apart": its own folder, its own backup, archived as a whole.
- **Backup on import.** "Backup to" writes one more copy during import,
  to a folder chosen in the import window; like Lightroom's, it is not
  tracked afterwards, and users confuse it with catalogue backups
  ([forum: backup location](https://support.captureone.com/hc/fr/community/posts/24951914077853-Capture-One-creates-a-copy-of-the-photo-in-the-backup-folder),
  [forum: change backup folder](https://support.captureone.com/hc/en-us/community/posts/360012225997-How-to-change-backup-folder?page=1)).
- **Catalogue backup** on a schedule, to a folder, as Lightroom
  ([Thomas Fitzgerald: restoring a catalogue](https://blog.thomasfitzgeraldphotography.com/blog/2021/2/how-to-restore-a-capture-one-catalogue-from-a-backup-and-recovering-a-corrupt-catalogue)).

Lesson: the session is the reason "per shoot" matters: a job is the
unit people back up, archive and hand over. Helios does not need two
kinds of library for it; a shoot can carry its own strategy.

### 1.4 Photo Mechanic

- **Primary and secondary ingest.** Each ingest names a primary and an
  optional secondary destination (a third for live ingest), both with
  folder variables and a job name; both are written during the copy
  ([Camera Bits: ingesting](https://docs.camerabits.com/support/solutions/articles/48000207409),
  [live ingest](https://docs.camerabits.com/support/solutions/articles/48000341503),
  vendor).
- **Incremental ingest** remembers what was copied per card, so the next
  ingest copies only the new frames: a card can be ingested several
  times a day, and each time both destinations grow.
- Once ingested, the secondary copy is not tracked: nothing records
  later whether it still exists or still matches the primary.

Lesson: event photographers want both copies written while the card is
in the reader, in one pass, and are used to that being part of ingest.
A strategy "on import" must therefore run right after the library copy,
from the library copy, not from the card a second time (the card is the
slowest link), unless the user asks for "copy from the card" to keep the
backup independent of the library disk.

### 1.5 Rapid Photo Downloader

- **Backup devices** are found automatically: any mounted volume holding
  a folder of a chosen name (the "backup identifier") is a backup
  destination, or fixed paths are given instead; photos and videos can
  go to different backup devices; backups are made during the download
  ([man page](https://man.archlinux.org/man/rapid-photo-downloader.1.en),
  [man page, 0.9](https://manpath.be/f35/1/rapid-photo-downloader)).
- **Pain**: users ask for named setups per backup drive
  ([pixls.us](https://discuss.pixls.us/t/rapid-photo-downloader-setups/27596)).

Lesson: the marker folder is a simple, portable way to recognise "this
is one of my backup disks" on any system, even when the filesystem has
no UUID (exFAT has a serial only, some network shares nothing). Helios
can use a marker file the same way, written only on volumes the user
gave a writing role.

### 1.6 digiKam

- **Collections** are root folders of three kinds: local, removable and
  network. A local collection is assumed always there; removable and
  network ones may be absent, and their albums are then shown as
  unavailable rather than dropped
  ([digiKam: collections](https://docs.digikam.org/en/setup_application/collections_settings.html)).
  Collections are identified by the volume's UUID where possible.
- **Pain.** A collection declared "local" on an external disk, or a
  network share that drops while digiKam runs, has made whole albums
  vanish from the database
  ([KDE bug 377849](https://bugs.kde.org/show_bug.cgi?id=377849),
  [users list, 2022](https://mail.kde.org/pipermail/digikam-users/2022-September/034094.html));
  macOS does not tell digiKam when a drive comes and goes
  ([KDE bug 470556](https://mail.kde.org/pipermail/kde-mac/2023-June/008706.html));
  users ask that thumbnails stay visible for removable collections
  ([users list, 2024](https://mail.kde.org/pipermail/digikam-users/2024-January/035791.html)).

Lesson: the kind of volume must not be a guess the user can get wrong
with data loss as the price. In Helios, nothing is ever removed from the
library because a volume is absent, whatever its kind: absent is
offline.

### 1.7 darktable

- **Film rolls** are folders: an image is a film roll path plus a file
  name. The collections module can tell darktable that a folder moved,
  or remove a film roll from the library
  ([manual: collections](https://docs.darktable.org/usermanual/4.6/en/module-reference/utility-modules/shared/collections/)).
- **Missing files** show a placeholder (a skull) in the lighttable;
  removing them is a manual clean-up, and a moved or renamed folder
  loses its link
  ([pixls.us: remove missing images](https://discuss.pixls.us/t/remove-missing-images-darktable/5308),
  [pixls.us: update a film roll](https://discuss.pixls.us/t/how-to-update-a-film-roll/24021)).
- No backup of photos; recent versions take snapshots of the library
  database on a schedule set in the preferences; edits also live in XMP
  sidecars beside the raws, so a plain file backup carries them.

Lesson: sidecars made darktable's edits survive a lost database. Helios
keeps edits in the vault, so a backup of photos without the catalogue
loses the work: catalogue backup is part of the strategy, not an
afterthought.

### 1.8 General backup tools

- **Time Machine**: hourly, automatic, to a local or network disk;
  network backups are verified periodically, and "Verify Backups" runs
  by hand ([Apple: verify your backup disk](https://support.apple.com/guide/mac-help/mh26840/mac));
  it reminds the user when a backup disk has not been seen for days.
- **restic**: content-addressed snapshots; `restic check` checks the
  repository structure, `--read-data` reads everything, and
  `--read-data-subset` reads a part (`n/t`, a percentage or a size), so
  a full verification can be spread over days
  ([restic: checking integrity](https://restic.readthedocs.io/en/stable/045_working_with_repos.html#checking-integrity-and-consistency),
  [man page](https://www.mankier.com/1/restic-check)).
- **Borg**: `borg check` and `--verify-data` (reads and decrypts every
  chunk); `--max-duration` splits a repository check into slices
  ([Borg: check](https://borgbackup.readthedocs.io/en/stable/usage/check.html)).
- **rsync-style mirrors**: `--checksum` compares by content rather than
  size and time; a mirror propagates deletions (`--delete`) and
  corruption unless used with care
  ([rsync man page](https://download.samba.org/pub/rsync/rsync.1)).

Lessons:

- verification is a scheduled, sliced job (a percentage per run), with
  a "last fully verified" date per destination;
- objects are immutable and deduplicated by content: a corrupted or
  encrypted file is a new hash, the old copy stays (the ROADMAP's
  continuous backup says the same);
- a backup must never behave like a sync: deleting from the library is
  not deleting from the backup;
- reminders for disks not seen for a while are part of the product.

### 1.9 What event and wedding photographers do

From the practitioners' guides already cited in docs/import-research.md
([PetaPixel](https://petapixel.com/2019/06/20/how-to-back-up-photos-as-a-wedding-photographer/),
[Fstoppers](https://fstoppers.com/education/my-post-wedding-workflow-image-backup-and-cataloging-87643),
[SLR Lounge](https://slrlounge.com/never-lose-an-image-with-this-file-backup-and-cloud-storage-guide/),
[Lexar](https://americas.lexar.com/wedding-photographers-backup-workflow/), vendor):

- **In the camera**: dual slots writing the same frames to two cards;
  the second card is the first backup.
- **Same evening**: every card copied to a working drive and a backup
  drive (often a RAID or NAS at home, a portable SSD on the road), then
  a cloud upload overnight.
- **Cards kept unformatted** until the copies are confirmed, often until
  delivery; some rotate a stock of cards so they never have to format
  early.
- **Per client**: a wedding is kept on more copies (and longer) than a
  family portrait or a personal walk; a delivered job is moved to an
  archive drive, often with a second archive off site, and the working
  drive is freed.
- **What goes wrong** in their accounts: a card formatted before the
  copy was complete, a backup drive that had failed silently, a NAS
  synced that propagated a deletion, a forgotten off-site drive not
  updated for months.

What they need from Helios, in their words: "is this wedding on two
drives?", "can I format these cards?", "which jobs are not in the cloud
yet?", "which drive has the 2019 archive?", "when did I last check the
off-site disk?".

### 1.10 Summary

| Tool | Copies at ingest | Tracked after | Verified | Per job | Offline volumes | Catalogue backup |
|---|---|---|---|---|---|---|
| Lightroom Classic | 1 extra | no | no | no | greyed, previews kept | on exit, scheduled |
| Capture One | 1 extra | no | no | sessions | greyed | scheduled |
| Photo Mechanic | 1 to 2 extra | no | no | job names | n/a | n/a |
| Rapid Photo Downloader | several devices | no | no | job codes | n/a | n/a |
| digiKam | no | no | no | no | removable or network collections | database file |
| darktable | no | no | no | no | placeholder | snapshot at start |
| Time Machine, restic, Borg | n/a | yes | yes, scheduled | no | reminders | n/a |
| **Helios (proposed)** | per strategy | **per photo, by hash** | **yes, re-checked** | **per shoot, import, card** | greyed, previews kept | snapshots copied along |

## 2. Storage model

### 2.1 Volumes

A **volume** is a place files live that can come and go as a whole: an
internal disk partition, an external disk, a RAID array as the system
sees it, a USB stick, an SD card, a mounted network share, and, through
a plugin, a remote store.

**Identity**, in order of preference, the first that exists:

1. the filesystem UUID (ext4, btrfs, XFS, APFS, NTFS have one; FAT and
   exFAT have a 32-bit serial, unique enough when paired with the
   label and size);
2. for a network share, the server and share name (`smb://nas/photos`,
   `nfs://nas:/volume1/photos`), plus the marker below;
3. a marker file Helios writes at the root of its own folder on the
   volume (`.helios/volume.json`: an id, the library it belongs to,
   when it was named), only on volumes the user gave a writing role
   (section 2.4); it also survives a reformat-and-restore of a backup
   disk and recognises one share under two mount paths;
4. failing all three (a read-only card), the label and capacity, marked
   "weak": good enough to say "reinsert EOS_DIGITAL", never used to
   count a backup.

A RAID array or a NAS volume is one volume to Helios: its redundancy
protects against a dead disk, not against deletion, corruption or theft,
so **a RAID counts as one copy**. Helios says so in the Storage page
when the user marks a volume as RAID (it cannot always tell).

**What Helios records per volume**: id, how it was identified (UUID,
share, marker, weak), the user's name for it (default: its label),
kind, roles, capacity and free space last seen, whether it is online,
where it is mounted now, when it was last seen, whether it is off site
(the user says), and the physical drive behind it when the system says
(so two partitions of one disk are not two copies).

**Owner direction (2026-11-07):**
- The user describes the disks. Detection only proposes, and the user names each volume and gives it its roles.
- A volume is either removable or fixed. A **fixed** disk belongs to its **node**, the machine it is mounted in. Its "offline" means "that node is not running", not "unplugged". Seen from another node, it is listed as belonging elsewhere, and Helios neither writes to it nor repairs it from there.
- Every volume shows when, and on which node, it was **last seen**: in the Storage page, and on its offline photos ("on Backup-2, last seen 2026-10-28 on the studio PC").
- When the physical identity is not enough (no UUID, a FAT card, a cloned disk), Helios **marks the disk with a small identity file** (point 3 above), with the user's leave. This settles open question 3.

So `Volume` gains `node` (for a fixed disk), `last_seen` (date and node) and the way it is identified (physical or marker).

**Kinds**: internal, external, removable (card, stick), network share,
remote (plugin). The kind sets defaults (a card gets the ingest role, an
internal disk the library role) and wording, never what happens to the
photos when the volume is absent.

**Detection per system** (not decided: which library or call wraps
each):

| | Linux | Windows | macOS |
|---|---|---|---|
| List and watch | udisks2 over D-Bus: `Block` (`IdUUID`, `IdLabel`, `HintSystem`, `Drive`) and `Filesystem` (mount points), with signals when a device comes or goes ([udisks2 API](https://storaged.org/doc/udisks2-api/latest/)); `/proc/mounts` and `/dev/disk/by-uuid` as the fallback the import window reads today | volume GUID paths (`GetVolumeNameForVolumeMountPoint`), serial and filesystem from `GetVolumeInformation`, arrival and removal by `WM_DEVICECHANGE` ([Microsoft: volume GUID path](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumenameforvolumemountpointw), [GetVolumeInformation](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumeinformationw), [WM_DEVICECHANGE](https://learn.microsoft.com/en-us/windows/win32/devio/wm-devicechange)) | Disk Arbitration: appeared and disappeared callbacks, the description's volume UUID, name, kind and whether it is removable or network ([Apple: Disk Arbitration](https://developer.apple.com/documentation/diskarbitration)) |
| Free space | `statvfs` | `GetDiskFreeSpaceEx` | `statfs` |
| Network shares | GVfs and kernel mounts appear as mounts; the share's server and name from the mount source | mapped drives and UNC paths | `smbfs` and `nfs` mounts |
| Removable media | udisks `Removable`, `HintSystem` false | `GetDriveType` = removable | the description's removable and ejectable flags |

Polling about once a second (as the import window's `Watch` does) is
the portable fallback; events are an optimisation, not a requirement.
digiKam's macOS bug (no events) shows why polling must stay.

### 2.2 Locations on volumes

A location today is an absolute path. Proposal (not decided): a
location also names its **volume** and its **path inside the volume**;
the absolute path stays as the "last seen" hint. When a volume comes
back at another mount point (another drive letter, `/run/media/x/DISK1`
instead of `/media/DISK`), every location on it is found again without a
repair scan. This is Lightroom's "Find Missing Folder" done by the
machine.

**Built (phase 2, 2026-11-07).** `Location` has `volume` (a
`VolumeId`, which moved down into `helios_vault::storage` so the vault
names it without depending on `helios-storage`), `rel_path` and
`presence`. Schema step 4 adds the columns `volume`, `rel_path` and
`presence` to `locations` (the old `present` flag becomes `present` or
`missing`, then is dropped); old rows keep their absolute path and wait,
volume and `rel_path` empty, for the next scan. The scan
(`helios_storage::scan`) runs on a thread of its own at start, when the
import window's `Watch` sees a volume come, go or change mount point, and
every five minutes. It records the volumes (`registry::record`), then
links each location on no volume: to the volume with the deepest mount
point above its path, an online one only when the file is there (so a
path under `/` whose disk is unplugged is not given to the system disk),
an offline one known to have been mounted there on this node as is. On
an online volume the absolute path is rebuilt from the volume's mount
point and `rel_path`, so a changed mount point loses nothing.

### 2.3 Online, offline, missing

A location has three states instead of `present` true or false:

- **present**: the volume is online and the file was there when last
  looked at;
- **offline**: its volume is not mounted; nothing is known to be wrong;
- **missing**: its volume is online and the file is not there (deleted,
  moved outside Helios); `repair` may relink it by hash.

What the library does with a photo whose locations are all offline:

- it stays in the grid with its **preview** (from a preview cache, see
  section 4.5), its marks and its edits; a small badge says "offline:
  BACKUP_2" on hover, as Lightroom greys a folder;
- marks, tags, ratings, versions and decisions all work: they are vault
  data;
- opening it for full-resolution editing says "Plug in BACKUP_2 to edit
  at full size" and offers the preview for culling;
- exporting it waits for the volume (the job pauses, as an import does
  for a pulled card);
- `repair` never touches locations on offline volumes, and nothing is
  ever removed from the library because a volume is absent.

**Built (phase 2, 2026-11-07).** `helios_vault::Presence` is `Present`,
`Offline` or `Missing`; the scan sets it. A location is offline when its
volume is not mounted, when it is a fixed (internal) disk whose node is
not this one, or when the registry no longer knows its volume; present
when its file is there; missing when its volume is online (or it is on
none) and the file is not there. `repair` checks present locations only:
an offline one is never marked missing, and an image with an offline
copy is not searched for. The Library's entries carry the image's
presence (present when a copy is, else offline when one is, else
missing); nothing draws it yet (phase 3's badge).

### 2.4 Roles

The user says what each volume is for. A volume has zero or more roles:

| Role | Meaning | Helios writes there |
|---|---|---|
| **Library** (working) | where the library folder lives; photos being culled and edited | yes, in the library folder |
| **Archive** | where finished shoots are moved to free the working disk | yes, in its Helios folder |
| **Backup** | destination of backup copies | yes, in its Helios folder |
| **Ingest** | cards, sticks, phones: sources only | **never** |
| **Scratch / cache** | previews, render caches, exports in progress | yes, evictable; never counts as a copy |
| none | a volume Helios knows of (it holds referenced photos) but was given no role | **never** |

Several roles per volume are normal: a home NAS can be both archive and
backup; a laptop's internal disk library and scratch. A volume with
the library role and the backup role does not count twice for a photo:
copies count per volume (section 3.2).

**Which volumes Helios may write to**: only those with a library,
archive, backup or scratch role, only inside the folder Helios owns
there (the library folder, or `Helios Backup/` and `Helios Archive/`
at the volume's root by default), and only by the rules the import
already follows (a part file, verified by hash before it takes its
name, never overwriting). Referenced photos are never written, moved or
renamed, whatever their volume's roles. A card is never written, not
even a marker.

## 3. Backup strategies

### 3.1 What a strategy says

A **strategy** is a named rule of the library ("Client work", "Personal",
"Wedding: 3 copies"):

- **destinations**: each one either a **role** ("any backup volume",
  so a rotation of two backup disks satisfies it with whichever is
  plugged in), a **volume** ("BACKUP_A"), or a **remote destination**
  given by a plugin ("the B2 bucket"), each with an optional folder
  layout (by default the library folder's own, `YYYY/YYYY-MM-DD/name`);
- **copies**: how many verified backup copies a photo needs, on
  distinct volumes, besides its library copy; optionally "one of them
  off site" (on a volume the user marked off site, or a remote one);
- **when** it runs, one or several triggers: **on import** (right after
  the library copy, from it, or from the card when the user prefers
  independent copies), **when a destination is plugged in** (the
  rotation case: the off-site disk comes back once a month and catches
  up), **on close** (before Helios exits, with a visible "Backing up 34
  photos, close anyway?" that never blocks), **on a schedule** (daily
  at a time, weekly), **by hand** ("Back up now");
- **verification**: a copy counts once read back and matched by hash
  (always, not a choice); **re-check** of existing copies on a schedule
  (for example every 90 days, a slice per run as restic's
  `--read-data-subset`), with "last fully verified" per destination;
- **what it covers**: originals always; sidecars Helios copied along;
  optionally exports (not decided).

A **library default strategy** applies to every photo without another
one. A new library's default is **none** (not decided: see the open
questions), which makes the import's reminder appear.

### 3.2 How a copy counts

- One copy per **volume**: two copies on the same volume (or two
  partitions of the same physical drive, when the system says) count as
  one.
- A copy counts when its location is **present or offline** and its
  **last verification** passed and is younger than the strategy's
  re-check period; an offline copy still counts (the off-site disk is
  supposed to be away), with its "last verified" date shown.
- Caches and scratch never count. The library copy is the working copy,
  not a backup: "backed up 2/2" counts backup copies only. A referenced
  photo's original (on a volume with no role) is its working copy.
- Copies are never overwritten and never deleted by a strategy. A
  library photo removed by the user keeps its backup copies; reclaiming
  them is the ROADMAP's separate, confirmed "reclaiming space" step.

### 3.3 Overrides per shoot, import session or card

Not every reportage is treated the same. A strategy can be assigned to:

- a **shoot** (ROADMAP section 2.4's project or session: "Martin
  wedding"), once shoots exist;
- an **import session** (one import, today's unit of the library's
  history), from the import window or afterwards;
- a **card** (by its volume id): "everything that comes from the
  wedding cards gets the 3-copy strategy" (cards are often dedicated
  per body or per kind of job);
- a **selection of photos**, by hand.

A photo's strategy is the first that applies: the photo's own, its
shoot's, its import session's, its card's, then the library default.
When a photo belongs to several shoots, the **strongest** wins (most
copies; off site beats not), never the weakest: a strategy can only add
protection.

### 3.4 Status, filter and report

**Per photo**: `backed up 2/2` (green), `1/2` (pending or short:
amber), `0/2` (red), `no strategy` (grey), `stale` (a copy past its
re-check date), `error` (a re-check failed: the copy differs from the
hash). Shown as a small badge in the grid (off by default, on in the
intake template), in full in the image's info: which volumes, when each
copy was verified, which are offline.

**Per shoot or import**: "Martin wedding: 3,412 photos, backed up 2/2,
off site 0/1, last verified 3 days ago."

**Filter**: the search field gets `backup:short`, `backup:ok`,
`backup:none`, `on:BACKUP_A`, `offline`: "show me what is not safe
yet", "show me what is on this drive".

**Report** (Storage page, and `helios storage report` on the command
line): per strategy and per shoot, the counts; per volume, its photos,
its free space, when it was last seen and last verified; a list of
volumes not seen for longer than a set time ("BACKUP_OFFSITE not seen
for 41 days"), as Time Machine reminds.

### 3.5 Safe to format

Today: every photo of the card has a verified copy in the library
folder. With strategies (not decided), a card is safe to format when
every photo of it meets **its strategy** (the card's own, or the
library default):

- strategy satisfied: "EOS_DIGITAL: safe to format (library + 2
  backups, verified)";
- library copy only, strategy not yet met (a backup disk not plugged
  in): "EOS_DIGITAL: in the Library, backup 0/1: keep the card until
  BACKUP_A is plugged in";
- no strategy: "EOS_DIGITAL: safe to format (1 copy, in the Library: no
  backup set up)", with the reminder link.

The check runs again when a backup job ends, so a card left in the
reader turns "safe" when the backup catches up, with a toast. Helios
still never formats a card.

### 3.6 The import's reminder

When the photos being imported have no strategy (the library default is
none and no override applies):

- one line in the import window's Choices panel, under the placement:
  "No backup set up: these photos will have one copy. Set up backup…"
  (opens the strategy editor) and a close button;
- closing it hides it for this library ("Remind me again" in the
  Storage page brings it back); it never blocks the Import button and is
  never a modal window;
- the end toast's "safe to format" line says "1 copy" as above;
- when a strategy exists, the line instead says what will happen:
  "Backup: Client work, 2 copies (BACKUP_A plugged in, NAS online)",
  with a menu to pick another strategy for this import (the import
  session override).

### 3.7 NAS and cloud destinations

- **A mounted share is a volume**: the core copies to it with the same
  part file, hash, verify and rename, plus a sync before the rename.
  Hashing reads every byte over the network: backup jobs are throttled,
  run at low priority, and pause (not fail) when the share drops.
- **A protocol that is not mounted** (SFTP, S3 and compatible stores,
  B2, WebDAV, a vendor cloud) is a **destination plugin**, out of
  process as every plugin (docs/extensions.md). Proposed service
  `storage.destination` (not decided):
  - `describe` (name, free space if known, whether it can give back a
    stored object's bytes and which checksums it computes);
  - `put` (the core hands a read-only path to the original, as
    `images.read` does, and a key: the content hash; the plugin
    uploads and returns the remote object's name and any checksum the
    store computed);
  - `get` (a range or the whole object, for verification and restore);
  - `list` (keys present, for re-checks and reports);
  - no `delete` in v3: removing remote copies is a later, confirmed
    step.

  The core owns the copy order, the queue, retries, the status and the
  verification: a remote copy counts once the core has read it back
  (`get`) and matched the BLAKE3 hash, or, when reading back costs
  money or time, once the store's own checksum (SHA-256 for S3, SHA-1
  for B2) matches one the core computed while reading the original,
  marked "verified by the store" in the status. Secrets go through the
  `secrets` capability, never in the strategy. Keys are content hashes,
  so a re-upload is a no-op and a remote copy can never be overwritten
  by a different file.
- A plugin wrapping rclone (MIT-licensed, run as a separate program)
  would reach dozens of stores at once; a plugin per store is the other
  way (not decided).

### 3.8 Relation to the ROADMAP

- **Continuous background backup** (ROADMAP 2.3, 2026-10-11) is this
  design's long-term form: "the redundancy policy" is a strategy,
  "replicated in the background" is the trigger "whenever a destination
  is available", "scrubbing" is the scheduled re-check, "tombstones"
  and "never like sync" are section 3.2's rules, "last verified
  reminders" are section 3.4's. Strategies add what the ROADMAP did not
  say: they differ per shoot, import or card, and the user names
  destinations by role.
- **Retention tiers** (essential, normal, non-essential) map to
  strategies too (not decided): a tier is a strategy with a reclaiming
  rule, which comes with reclaiming space, not before.
- **Placement and caching**: caches are scratch-role volumes and never
  count; "client images never leave the premises" is a strategy rule
  forbidding remote destinations (later).
- **Catalogue (vault) backup**: the vault already takes verified SQLite
  snapshots (five kept, next to the database). Proposal: a snapshot is
  also taken on close (when the library changed) and daily, and the
  newest one is copied, verified, to every backup-role volume online,
  into `Helios Backup/library/`, keeping a few per volume. Without it, a
  dead library disk loses every edit, rating and tag, since Helios
  writes them to the vault and not beside the photos. A plain-text
  manifest (hash and relative path per photo) is written next to each
  destination's copies, so a backup can be checked, and the photos
  found again, without Helios.

## 4. Fit with Helios

### 4.1 The vault's locations and hashes

- A backup copy is a `Location` like any other: nothing new is needed
  for "where are the copies". It needs the volume (section 2.2), the
  offline state (section 2.3), and the hash of the last verification
  (or simply: `last_verified` set only when the bytes read back match
  the image's id, which is already what a copy-import does).
- `repair` must skip offline volumes and use volume-relative paths; it
  then relinks only what really moved. (Built in phase 2: `repair` skips
  offline locations, and the volume scan, not `repair`, follows a moved
  mount point through `rel_path`.)
- Strategies, assignments, volumes and their roles are vault documents
  (namespaces `storage/volumes`, `storage/strategies`,
  `storage/assignments`), so they travel with the library and its
  snapshots, and a second workstation sees the same volumes (ROADMAP's
  multi-workstation model: each node sees which of them it has online).
- The per-photo strategy and status are derived, not stored as truth;
  a cached field (`backup` = `{have, want, stale}`) feeds the filter
  and the grid badge, recomputed when a location or an assignment
  changes.

### 4.2 The import's volume detection

The import window's `Volumes` trait and `Watch` poller (cards: mounted
removable volumes holding `DCIM`) are the seed. Proposal: move them out
of `helios-gui` into a small `helios-storage` crate (no GUI), extend
`Volume` with an id, a kind, free space and the drive behind it, list
every volume rather than only cards, and keep "cards" as a filter
(removable and holding `DCIM`). The import window, the Storage page,
the backup jobs and `repair` all read the same list. Not duplicated: the
import window keeps calling `cards()`.

### 4.3 Jobs

- A backup is a job of kind `backup` (docs/jobs.md): a parent per run,
  a child per destination, `job.progress` per photo, `job.paused` with
  "Plug in BACKUP_A to go on" when a destination goes away (the import's
  pause code, reused), `job.resumed`, cancellable, at low priority so
  it never slows an import or an export.
- It is **idempotent by hash**: a copy already present and verified on
  a destination is skipped, so a cancelled or crashed job just runs
  again.
- A backup "on import" is started by the import job when it ends (or
  per card as each child ends), reading from the library copy just
  verified; the import's "safe to format" line waits for it, and is
  updated when it ends.
- Re-checks are a job of kind `verify`, sliced (a percentage or a time
  budget per run).

### 4.4 Undo of imports

- Undoing an import removes its library entries and never a file
  (docs/library.md). The backup copies it caused are locations like the
  library copies: they leave the library with the import and come back
  with a redo; the files stay on the backup volume, as the library
  copies stay in the library folder.
- A later import of the same photos finds those backup copies by hash
  (the destination's manifest, or a scan) and does not copy them again.
- A strategy assignment made in the import window is part of the
  import's step: undoing the import removes it too. Assigning a
  strategy afterwards is one Library undo step of its own; the copies
  it triggered stay (undo never deletes files).

### 4.5 Previews for offline photos

Today thumbnails come from the files. Offline photos need a **preview
cache** in the library (proposal, not decided): the embedded preview
read at import, kept at the grid's largest size (about 512 pixels on
the long side, 30 to 60 KB as JPEG, so about 2 GB for 50,000 photos),
on the library volume or a scratch-role volume, keyed by image id. It
also speeds up the grid for photos on slow network shares. This is a
v1 prerequisite for offline volumes.

## 5. Recommendation

### 5.1 The model, sketched in Rust (not decided)

```rust
/// A place files live that comes and goes as a whole.
pub struct Volume {
    pub id: VolumeId,
    pub identified_by: Identity,      // Uuid, Share, Marker, Weak
    pub name: String,                 // the user's, default the label
    pub label: Option<String>,
    pub kind: VolumeKind,             // Internal, External, Removable, Share, Remote
    pub roles: BTreeSet<Role>,
    pub off_site: bool,               // the user says
    pub raid: bool,                   // the user says: counts as one copy anyway
    pub drive: Option<String>,        // the physical drive, when the system says
    pub capacity: Option<u64>,
    pub free: Option<u64>,            // last seen
    pub online: Option<PathBuf>,      // where it is mounted now, if it is
    pub last_seen: u64,
}

pub enum Role { Library, Archive, Backup, Ingest, Scratch }

/// Today's `Location`, with its volume and a three-state presence.
pub struct Location {
    pub volume: Option<VolumeId>,     // None: an old record, until relinked
    pub relative: PathBuf,            // inside the volume
    pub path: PathBuf,                // last seen, absolute
    pub size: u64,
    pub modified: u64,
    pub last_verified: u64,           // set only by a read-back matching the id
    pub state: Presence,              // Present, Offline, Missing
    pub purpose: Purpose,             // Working, Backup, Archive (never a cache)
}

pub struct Strategy {
    pub id: StrategyId,
    pub name: String,
    pub destinations: Vec<Destination>,
    pub copies: u8,                   // backup copies, on distinct volumes
    pub off_site: bool,               // one of them off site
    pub triggers: Vec<Trigger>,
    pub recheck: Option<Recheck>,
    pub from_card: bool,              // backup read from the card, not the library copy
}

pub enum Destination {
    Role(Role),                       // any online volume with this role
    Volume(VolumeId),
    Remote { plugin: String, target: String },
}

pub enum Trigger { OnImport, OnConnect, OnClose, Daily { at: u16 }, Weekly { day: u8, at: u16 }, Manual }

pub struct Recheck { pub every_days: u32, pub slice_percent: u8 }

/// What a strategy is attached to; the first that applies wins,
/// the strongest when a photo is in several shoots.
pub enum Scope { Photo(ImageId), Shoot(ShootId), Import(ImportId), Card(VolumeId), Library }

pub struct BackupStatus { pub have: u8, pub want: u8, pub off_site: bool, pub stale: u32, pub errors: u32 }
```

### 5.2 Surfaces

- **Storage page** (a panel in the Library workspace, also from
  Settings): every volume Helios has seen, online first, each with its
  name, kind, roles (toggles), free space bar, last seen, last verified,
  photo count; a volume not seen for long in amber. Actions: name it,
  set roles, mark off site or RAID, "Show its photos" (the `on:` filter),
  "Verify now", "Forget" (only when it holds no location). Under it, the
  catalogue backups: where, when, how many.
- **Strategy editor** (from the Storage page and the import's reminder):
  name, destinations (role or volume pickers, remote ones from
  plugins), copies, off site, when, re-check; a live sentence ("Each
  photo gets 2 verified copies on your backup disks, made after each
  import, re-checked every 90 days"); "Use as the library default".
  A first-run version is three choices: "One backup disk", "Two backup
  disks, one off site", "Set up later".
- **In the library**: the badge on thumbnails (optional), the info
  panel's copies list, the search filters, a "Backup" column in the
  import history; on an import, a shoot or a selection, "Backup
  strategy ▸" in the context menu.
- **In the import window**: the reminder line or the strategy line
  (section 3.6); the end toast's safe-to-format line by strategy.
- **Status bar**: one quiet item ("Backup: 312 photos waiting for
  BACKUP_A") when something is short, nothing when all is well.
- **Command line**: `helios storage volumes`, `helios storage report`,
  `helios backup run [--strategy]`, `helios backup verify`; library
  commands `storage.*` and `backup.*` for agents.

### 5.3 v1: what it must have

1. **Volumes in the core**: the import window's detection moved to a
   `helios-storage` crate, extended to every volume, with identity
   (UUID on Linux, marker file on writable-role volumes, weak otherwise),
   kind, capacity, free space, online state. Linux first; Windows and
   macOS behind the same trait.
2. **Locations on volumes**, three-state presence, `repair` skipping
   offline volumes, and a **preview cache** so offline photos keep
   their thumbnails.
3. **Roles** set in the Storage page; Helios writes only where a role
   allows.
4. **One local destination kind** (a volume with the backup role, or a
   mounted share), a **library default strategy** (possibly none),
   triggers on import, on connect and by hand.
5. **Verified copies** as a `backup` job: part file, hash, read back,
   rename, never overwrite; paused when the destination goes;
   idempotent.
6. **Status** per photo and per import, the filters, the badge, a
   simple report.
7. **The import's reminder** and **safe to format by strategy**.
8. **Catalogue snapshots** on close and daily, copied to each backup
   volume, with a manifest.

### 5.4 v2

- Overrides per shoot (once shoots exist), import session, card and
  selection, with the "strongest wins" rule.
- **Archive moves**: move a shoot from the working volume to an archive
  volume (copy, verify, then remove the working copy only after a
  confirmation, never dropping below the strategy's count).
- Off-site rules and "not seen for N days" reminders.
- The full report (per strategy, per shoot, per volume).

### 5.5 v3

- Schedules (daily, weekly) and **re-checks** sliced over time (the
  ROADMAP's scrubbing), with repair from a good copy.
- The `storage.destination` plugin service; first plugins: SFTP and an
  S3-compatible store (or an rclone wrapper).
- Restore: "promote a backup copy" when the working copy is lost or
  corrupted, and rebuilding a library from a destination's manifest and
  catalogue snapshot.
- Retention tiers and reclaiming space, with the ROADMAP's confirmed
  steps.

### 5.6 What Helios never does

- Format, erase or write to a card, even a marker.
- Write to a volume the user gave no writing role, or outside its own
  folder there; write, move or rename a referenced photo.
- Overwrite a file on any destination, or count an unverified copy.
- Delete a backup copy as a consequence of deleting a photo, an undo or
  a strategy change; propagate deletions as a sync would.
- Count two copies on one volume (or one drive), a RAID's redundancy,
  or a cache as backups.
- Remove a photo from the library because its volume is offline.
- Block the interface or an import with a backup, or ask in a modal
  window that a strategy be set.
- Upload anywhere that is not a destination of a strategy the user set,
  or store credentials outside the secrets store.
- Let a plugin decide that a copy is verified.

### 5.7 Phased plan

| Phase | Content | Size |
|---|---|---|
| 1. Volumes in the core | `helios-storage` crate from the import window's `Volumes`: every volume, identity (UUID, marker, weak), kind, free space, online state, polling and udisks events on Linux | M |
| 2. Locations on volumes | volume and relative path in `Location` (vault migration, old records relinked on the next scan), three-state presence, `repair` skipping offline volumes | M |
| 3. Preview cache | embedded previews kept at import, grid reads them first, offline badge | M |
| 4. Roles and Storage page | roles, names, off site, RAID flag; the page; write permission enforced in copy code | M |
| 5. Strategies v1 | model, library default, triggers on import, on connect and by hand, `backup` job with verify and pause, status field, filters, badge | L |
| 6. Import integration | reminder line, strategy line, safe to format by strategy, toast updated when the backup ends | S |
| 7. Catalogue backup | snapshots on close and daily, copied to backup volumes, manifest | S |
| 8. Overrides and report (v2) | per import, card, selection, then shoot; strongest wins; the report | M |
| 9. Archive moves (v2) | move a shoot to an archive volume, verified, confirmed | M |
| 10. Schedules and re-checks (v3) | daily and weekly triggers, sliced re-checks, repair from a good copy | M |
| 11. Destination plugins (v3) | `storage.destination` service; SFTP and S3-compatible (or rclone) plugins | L |
| 12. Windows and macOS detection | the same trait over volume GUIDs and Disk Arbitration | M each |

Phases 1 to 7 are v1. Phase 3 can come before 2 (it pays off on its
own, for network shares).

### 5.8 Open questions for the owner

1. **Default for a new library**: "none" with the reminder (the owner's
   direction as read here), or propose "one backup disk" at first run?
2. **Counting**: does "backed up 2/2" count backup copies only (as
   proposed), or every durable copy, the library copy included (3/3, as
   the 3-2-1 rule counts)?
3. **Marker files**: settled by the owner (2026-11-07). Helios marks a
   disk with an identity file when its physical identity is not enough,
   with the user's leave (section 2.1).
4. **Shoots**: they do not exist yet (ROADMAP 2.4). Is the import
   session the first "shoot" for overrides, or do shoots come first?
5. **Backup from the card or from the library copy**: settled by the
   owner (2026-11-07). Every copy must be guaranteed and verified (written
   to a part file, hashed, read back and checked before it counts). There
   is no "copy from the original" preference: it is too complicated to
   keep up in the interface. Backups are made from the library copy.
6. **Exports and catalogue**: should strategies also cover exports, and
   how many catalogue snapshots are kept per backup volume?
7. **Remote stores**: an rclone wrapper (many stores, one more program
   to install) or one plugin per store?
8. **Preview cache size and place**: 512 pixels long side by default,
   on the library volume?
9. **Platform order**: Linux first, then Windows or macOS?

## 6. Roadmap proposal (not decided)

A short entry for ROADMAP.md, section 2.3, for the owner to settle:

> (2026-11-07, not decided) **Storage and backup strategies**
> (docs/storage-backup-research.md). Helios knows every volume of the
> user (internal and external disks, RAID and NAS, sticks and cards) by
> a stable identity, with its kind, free space and online state;
> locations name their volume, and a photo on an offline volume stays in
> the library with its preview, flagged offline, never removed. The user
> gives volumes roles (library, archive, backup, ingest, scratch), and
> Helios writes only where a role allows, never to a card. Backup is a
> library strategy, not an import gesture: destinations by role or by
> volume, a number of verified copies on distinct volumes, triggers (on
> import, when a destination is plugged in, on close, scheduled, by
> hand) and re-checks; a library default (possibly none) and overrides
> per shoot, import or card, the strongest winning. Status per photo and
> per shoot ("backed up 2/2"), a filter and a report; "safe to format"
> follows the card's strategy; without a strategy the import shows one
> dismissible reminder line. Copies are never overwritten or deleted by
> a strategy, and a backup never behaves like a sync. Mounted shares are
> volumes; other remote stores are destination plugins, while the core
> owns the copy, the verification and the status. Catalogue snapshots
> are copied along. This is the first form of the continuous background
> backup above.

## Sources

Rules and general tools:
[CISA: Data Backup Options](https://www.cisa.gov/sites/default/files/publications/data_backup_options.pdf),
[Backblaze: 3-2-1](https://www.backblaze.com/blog/the-3-2-1-backup-strategy/) (vendor),
[NAKIVO: 3-2-1-1-0](https://www.nakivo.com/blog/3-2-1-backup-rule-efficient-data-protection-strategy/) (vendor),
[Apple: verify your backup disk](https://support.apple.com/guide/mac-help/mh26840/mac),
[restic: checking integrity](https://restic.readthedocs.io/en/stable/045_working_with_repos.html#checking-integrity-and-consistency),
[restic check man page](https://www.mankier.com/1/restic-check),
[Borg: check](https://borgbackup.readthedocs.io/en/stable/usage/check.html),
[rsync man page](https://download.samba.org/pub/rsync/rsync.1).

Photo applications:
[Adobe: import options](https://helpx.adobe.com/lightroom-classic/desktop/import-photos/photo-video-import-options.html),
[Adobe: back up a catalog](https://helpx.adobe.com/lightroom/help/back-catalog.html),
[Adobe: locate missing photos](https://helpx.adobe.com/lightroom-classic/desktop/manage-catalogs-and-files/locate-missing-photos.html),
[Lightroom Killer Tips: when photos go missing](https://lightroomkillertips.com/when-photos-go-missing/),
[Lightroom Queen: catalogue backup frequency](https://www.lightroomqueen.com/catalog-back-frequency-keep/),
[Lightroom Queen: missing photos](https://www.lightroomqueen.com/lightroom-photos-missing-fix/),
[Capture One: catalogues and sessions](https://support.captureone.com/hc/en-us/sections/360001078997-Catalogs-and-Sessions),
[Capture One: sessions 101](https://www.captureone.com/blog/sessions-101-discussing-the-concept),
[Thomas Fitzgerald: restoring a Capture One catalogue](https://blog.thomasfitzgeraldphotography.com/blog/2021/2/how-to-restore-a-capture-one-catalogue-from-a-backup-and-recovering-a-corrupt-catalogue),
[Camera Bits: ingesting](https://docs.camerabits.com/support/solutions/articles/48000207409),
[Camera Bits: live ingest](https://docs.camerabits.com/support/solutions/articles/48000341503),
[Rapid Photo Downloader man page](https://man.archlinux.org/man/rapid-photo-downloader.1.en),
[Rapid Photo Downloader man page, 0.9](https://manpath.be/f35/1/rapid-photo-downloader),
[digiKam: collections](https://docs.digikam.org/en/setup_application/collections_settings.html),
[darktable: collections](https://docs.darktable.org/usermanual/4.6/en/module-reference/utility-modules/shared/collections/).

System interfaces:
[udisks2 API](https://storaged.org/doc/udisks2-api/latest/),
[Microsoft: GetVolumeNameForVolumeMountPoint](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumenameforvolumemountpointw),
[Microsoft: GetVolumeInformation](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumeinformationw),
[Microsoft: WM_DEVICECHANGE](https://learn.microsoft.com/en-us/windows/win32/devio/wm-devicechange),
[Apple: Disk Arbitration](https://developer.apple.com/documentation/diskarbitration).

Practitioners:
[PetaPixel: wedding backup](https://petapixel.com/2019/06/20/how-to-back-up-photos-as-a-wedding-photographer/),
[Fstoppers: wedding backup](https://fstoppers.com/education/my-post-wedding-workflow-image-backup-and-cataloging-87643),
[SLR Lounge: backup](https://slrlounge.com/never-lose-an-image-with-this-file-backup-and-cloud-storage-guide/),
[Lexar: wedding backup](https://americas.lexar.com/wedding-photographers-backup-workflow/) (vendor).

Forums and bug trackers:
[Adobe community: photos missing](https://community.adobe.com/t5/lightroom-classic-discussions/suddenly-2556-of-4206-of-my-image-files-in-lightroom-are-now-quot-missing-quot/td-p/13338402),
[Capture One forum: backup location](https://support.captureone.com/hc/fr/community/posts/24951914077853-Capture-One-creates-a-copy-of-the-photo-in-the-backup-folder),
[Capture One forum: change backup folder](https://support.captureone.com/hc/en-us/community/posts/360012225997-How-to-change-backup-folder?page=1),
[pixls.us: Rapid Photo Downloader setups](https://discuss.pixls.us/t/rapid-photo-downloader-setups/27596),
[KDE bug 377849](https://bugs.kde.org/show_bug.cgi?id=377849),
[KDE bug 470556](https://mail.kde.org/pipermail/kde-mac/2023-June/008706.html),
[digiKam users: albums on a USB disk](https://mail.kde.org/pipermail/digikam-users/2022-September/034094.html),
[digiKam users: thumbnails and removable storage](https://mail.kde.org/pipermail/digikam-users/2024-January/035791.html),
[pixls.us: remove missing images](https://discuss.pixls.us/t/remove-missing-images-darktable/5308),
[pixls.us: update a film roll](https://discuss.pixls.us/t/how-to-update-a-film-roll/24021).
