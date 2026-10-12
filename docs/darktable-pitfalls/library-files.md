# darktable pitfalls: library, files and cameras

Scope: SQLite library, XMP sidecars, import, metadata, film rolls, duplicates,
camera support (rawspeed, LibRaw, CR3, HEIC, JPEG XL), noise profiles, Lua API.
Sources: darktable-org/darktable issues and PRs read on 2026-10-12 (threads
fetched with `gh issue view --comments`). Helios state checked read-only in
`crates/helios-xmp`, `helios-vault`, `helios-vault-sqlite`, `helios-load`,
`helios-journal`, `ROADMAP.md` and `docs/NEEDS.md`. Anything not read in a
thread or in the code is marked *(guess)*.

---

### 1. Edits only reach storage on view change: lost on crash
- **darktable**: the history stack lived in memory and was written to the
  database (and the XMP) only when leaving the darkroom or switching image.
  A crash lost the whole session ("lost 1 h of retouching"). Maintainers
  hesitated for performance reasons ("I still prefer losing last edit than
  having a slow UI"); a commenter pointed out that sidecars must be written to
  a temp file then renamed, or a crash during the write corrupts them.
- **Evidence**: https://github.com/darktable-org/darktable/issues/7830
- **Helios exposure**: at risk. The vault has a `versions` table and field
  commands, but nothing yet says when an in-progress edit is persisted.
- **Recommendation**: persist every committed edit step (slider release,
  module toggle) as a small append in the vault, inside a transaction; it is
  tiny text, so cost is negligible with WAL. Write any XMP export through
  temp file + fsync + rename. Add a crash test: kill the process mid-edit,
  reopen, last committed step is there.

### 2. Silent database corruption, propagated into the user's own backups
- **darktable**: a user found `library.db` malformed (`integrity_check`
  failures) only after days of work; the daily backups of the last two days
  were corrupt too. A maintainer noted that most darktable writes were not
  transactional, so unclean exits could damage the file. PR #6348 then added
  SQLite backup-API snapshots, a `quick_check` at startup and restore from
  the last good snapshot (snapshots are renamed into place only once
  complete).
- **Evidence**: https://github.com/darktable-org/darktable/issues/4728 ,
  https://github.com/darktable-org/darktable/pull/6348 ,
  https://github.com/darktable-org/darktable/pull/12107
- **Helios exposure**: at risk. `helios-vault-sqlite` sets only
  `PRAGMA foreign_keys = ON`: no WAL, no `synchronous`, no integrity check, no
  snapshot.
  *Addressed since*: WAL, `synchronous=NORMAL` (FULL for the snapshots), an
  explicit busy timeout and one transaction per multi-statement write
  (`Store::begin` / `commit` / `rollback` in the contract). A
  clean-shutdown marker; after an unclean shutdown `quick_check` runs
  before use, and a failure opens the vault read-only with the latest
  snapshot named. Rotating snapshots via the backup API, checked before
  they are renamed into place. Tested with a process killed while writing
  and a file corrupted at a page. Not done: snapshots on a schedule (the
  call exists, nothing schedules it), and `integrity_check` instead of
  `quick_check` before a snapshot.
- **Recommendation**: open with `journal_mode=WAL`, `synchronous=NORMAL`
  (FULL for the catalog if cheap enough), explicit transactions for every
  multi-row write; `quick_check` on open after an unclean shutdown (a
  clean-shutdown flag), full `integrity_check` before taking a snapshot;
  rotating snapshots via the backup API, verified before they replace older
  ones. Put this in the store contract so every driver must survive it.

### 3. Schema migrations that break on real-world drift
- **darktable**: v53 to v54 failed for some users because a view (`v_images`)
  still referenced a column dropped by an older migration (v38 added
  `cameras.name`, v39 removed it, the view was never recreated); developers
  could not reproduce on their own fresh databases. v33 froze or failed on
  images whose film roll no longer existed (orphan rows); v21 to v30 failed on
  macOS because the REGEXP function was missing in that SQLite build.
- **Evidence**: https://github.com/darktable-org/darktable/issues/17363 ,
  https://github.com/darktable-org/darktable/pull/17364 ,
  https://github.com/darktable-org/darktable/issues/7891 ,
  https://github.com/darktable-org/darktable/issues/8020
- **Helios exposure**: at risk (early). The SQLite schema is a
  `CREATE TABLE IF NOT EXISTS` string with no `user_version`; the first
  schema change will need a migration framework.
  *Addressed since*: numbered migrations and `user_version`, each step in a
  transaction that drops every view and trigger first and recreates the
  current ones last; foreign keys are off while it runs and the orphans
  found afterwards are reported. Tested on old, current, newer and messy
  databases (orphans, a stale view and trigger).
- **Recommendation**: version the schema (`PRAGMA user_version`), one numbered
  migration per change, each in a transaction; drop and recreate every view
  and trigger at the end of each migration instead of patching them; run an
  orphan and FK check before migrating. Test migrations against a corpus of
  old databases (including deliberately dirty ones: orphans, missing rows),
  not only fresh ones. Use bundled SQLite (already the case with rusqlite
  `bundled`) so functions do not vary by platform.

### 4. Long migrations with no progress, no backup and no way back
- **darktable**: a `library.db` upgrade took 17 minutes with no feedback (it
  looked hung); another took 1 min 15 s on 50,000 images and 273,000 tag
  links. Backup before upgrade was requested in 2019 and is now done as
  `library.db-pre-<version>` copies, which users are told to restore by hand;
  there is no downgrade path (an older darktable refuses a newer database).
- **Evidence**: https://github.com/darktable-org/darktable/issues/8887 ,
  https://github.com/darktable-org/darktable/issues/7891 ,
  https://github.com/darktable-org/darktable/issues/2216 ,
  https://github.com/darktable-org/darktable/issues/15963
- **Helios exposure**: at risk (no migration yet). ROADMAP rule 6 already
  says migrations are explicit operations, which is the right base.
  *Addressed since*: a verified snapshot (backup API, then `quick_check` on
  the copy) before any migration, named in the error when a step fails;
  progress reported as a `vault.migration_progress` event (snapshot pages,
  then steps); a database newer than the binary is refused with a clear
  message, never downgraded. Not done: progress inside one long step, and
  the 200k-image budget test.
- **Recommendation**: snapshot before any migration and name it in the UI;
  report progress (rows done / total) as a scheduler job; budget-test a
  migration on a synthetic 200k-image catalog. Decide the downgrade story now:
  either keep migrations additive so an older Helios can still open the
  catalog read-only, or refuse with a clear message and the snapshot path.

### 5. Stale lock files and single-process locking
- **darktable**: a crash leaves `library.db.lock` / `data.db.lock` behind;
  darktable refuses to start and tells users to delete them by hand (kept on
  purpose, because two instances corrupted the database before). The CLI
  shares the lock: with a real library it cannot run while the GUI is open,
  so scripts use `--library :memory:`; flatpak and native builds disagree on
  stale-lock handling.
- **Evidence**: https://github.com/darktable-org/darktable/issues/8239 ,
  https://github.com/darktable-org/darktable/issues/5900 ,
  https://github.com/darktable-org/darktable/issues/19585
- **Helios exposure**: at risk for multi-process use (GUI, CLI, NAS agent,
  workers in `helios-journal`). Nothing in the vault handles concurrent
  processes yet.
  *Addressed since*: no lock file; WAL and a 5 s busy timeout, write
  transactions taken at once (`BEGIN IMMEDIATE`); a test runs two vaults
  and a reader on one file without a busy error.
- **Recommendation**: do not build a pid lock file. Rely on SQLite's own
  locking with WAL and a `busy_timeout`, so several readers and one writer
  coexist; if exclusive access is needed, use an OS advisory lock (released
  by the kernel when the process dies). Make the CLI a client of the same
  vault, not a second owner.

### 6. Sidecar written for every imported image, and later silently removed
- **darktable**: import wrote an XMP next to every file, even untouched JPEGs
  ("thousands of new xmp files despite no edits"), partly as a safety copy of
  the database. PR #9681 added an "after edit" mode; in 5.0 that mode deleted
  sidecars of images with no "real" edit (only auto-applied presets), which
  users read as sidecars not being written; this was not in the release
  notes.
- **Evidence**: https://github.com/darktable-org/darktable/issues/4450 ,
  https://github.com/darktable-org/darktable/pull/9681 ,
  https://github.com/darktable-org/darktable/issues/18295
- **Helios exposure**: handled in design (XMP is an exchange format generated
  on export, ROADMAP / NEEDS). At risk if Helios ever writes back into a
  darktable user's folders.
- **Recommendation**: never write next to originals by default; if a
  "write darktable-compatible XMP" option exists, make it explicit, per
  folder, never delete a file Helios did not create, and log every write.

### 7. XMP/database sync by timestamps ping-pongs
- **darktable**: "look for updated XMP files on startup" compares the
  database write time with the XMP mtime. A maintainer reading the crawler
  found it "really broken": when the database was newer it rewrote the XMP,
  which gave the XMP a newer mtime, so the next start flagged it again. The
  conflict dialog offered only per-file "keep DB / keep XMP" choices, without
  range selection or "keep newer".
- **Evidence**: https://github.com/darktable-org/darktable/issues/12257 ,
  https://github.com/darktable-org/darktable/issues/10164 ,
  https://github.com/darktable-org/darktable/issues/18253 ,
  https://github.com/darktable-org/darktable/issues/17512
- **Helios exposure**: partly handled. Identity is content-addressed and
  ROADMAP rejects last-writer-wins; but reading darktable XMP edited on
  another machine (the multi-workstation need) will meet the same problem.
- **Recommendation**: detect changes by content hash of the sidecar, never
  by mtime; store the hash of the last XMP imported or written per image;
  an external change becomes a new version (no merge), shown in a batch view
  with "keep newer / keep both" per selection.

### 8. Scanning every sidecar at startup on large or remote libraries
- **darktable**: the startup XMP check stats and reads every sidecar before
  the window is usable; on 60,000 images on a NAS this takes minutes, behind
  a splash screen that could not be moved; no cancel. The maintainers say it
  must finish before editing so thumbnails and history are not stale.
- **Evidence**: https://github.com/darktable-org/darktable/issues/14686 ,
  https://github.com/darktable-org/darktable/issues/18209 ,
  https://github.com/darktable-org/darktable/pull/17202
- **Helios exposure**: at risk for the repair scan and any sidecar watch:
  `helios-vault` repair already reads little (size, partial hash), but runs
  as one call.
- **Recommendation**: make scans scheduler jobs (named, cancellable, with
  progress), per folder, lazily prioritised by what the user is looking at;
  mark per image "verified at" so only stale entries are rechecked; prefer
  filesystem notifications where available.

### 9. Identity = absolute path + film roll: moving folders breaks everything
- **darktable**: images are keyed by film roll folder + filename. Moving or
  renaming folders outside darktable produces "skulls"; working from an
  external drive on two machines needs identical mount paths or the manual
  "update path to files" on the folder tree; relative paths were rejected
  because the library can span several roots. Deleted files stay as rows
  until a script purges them.
- **Evidence**: https://github.com/darktable-org/darktable/issues/17512 ,
  https://github.com/darktable-org/darktable/issues/18253 ,
  https://github.com/darktable-org/darktable/issues/9748 ,
  https://github.com/darktable-org/darktable/issues/7891
- **Helios exposure**: handled by design and in code: identity is the BLAKE3
  content hash, locations are data, repair relinks by hash
  (`helios-vault`).
- **Recommendation**: keep it. Still to cover: files whose bytes change
  without a content change (XMP written into a DNG, already noted in
  ROADMAP 2.3), and the darktable importer must map film-roll paths to
  locations and survive missing files (record them as offline, do not drop).

### 10. Metadata precedence bug: read nothing, write back the empty copy
- **darktable**: a 2020 commit tied sidecar metadata import to an unrelated
  option; with the default configuration no metadata was read from XMP, and
  the next sidecar write saved the image without it, erasing titles,
  descriptions and so on in the files. Import order (EXIF, then IPTC, then
  XMP, each overwriting) is load-bearing and was not protected by tests.
- **Evidence**: https://github.com/darktable-org/darktable/issues/4776
- **Helios exposure**: handled in principle (field-level writes, rule 1;
  `helios-xmp` reports what it does not carry). At risk wherever an importer
  writes a full record.
- **Recommendation**: importers emit field commands, never whole records;
  a field absent from the source means "unknown", not "empty"; precedence
  order (EXIF < IPTC < XMP < user) is a tested table.

### 11. "Refresh EXIF" overwrites user-set ratings
- **darktable**: new metadata fields in 4.8 required "refresh EXIF" on old
  images, which also reset star ratings set in darktable, without warning
  (documented as "working as designed"). Cameras disagree on whether 0 means
  "no rating" or "rated 0", so the importer cannot tell.
- **Evidence**: https://github.com/darktable-org/darktable/issues/17056
- **Helios exposure**: at risk (no EXIF refresh yet).
- **Recommendation**: keep camera-provided values and user values as
  separate fields (or provenance per field); a refresh updates only
  camera-provided fields; a new extracted field is backfilled lazily by a
  job, not by asking users to press a destructive button.

### 12. Exif field choice regressions (lens identification)
- **darktable**: since 5.0, Nikon Z bodies with some third-party lenses show
  the lens as "253" because darktable prefers `Exif.NikonLd4.LensIDNumber`
  over `Exif.Photo.LensModel`; lensfun then finds nothing. The stored value is
  wrong in the database until the user refreshes metadata or reimports; a CR2
  lens ID regression (4.4.1) is similar.
- **Evidence**: https://github.com/darktable-org/darktable/issues/18665 ,
  https://github.com/darktable-org/darktable/issues/14882 (title only, not read)
- **Helios exposure**: at risk once lens correction lands; `helios-load`
  takes camera data from rawler, lens data path not seen *(guess: not yet
  implemented)*.
- **Recommendation**: keep the raw candidate tags with the image, derive the
  lens name by a versioned rule, and re-derive when the rule changes instead
  of storing only the result. Regression corpus: one sample per maker/lens
  family with expected lens string.

### 13. Legacy parameter migration that silently changes the picture
- **darktable**: old filmic RGB v3/v4 edits opened in 4.8/4.9 with NaN
  contrast and wrong white target (18.45 % read into the wrong field),
  bisected to the "incremental IOP update" commit; it appeared and vanished
  with unrelated printf changes (undefined behaviour), and only on some
  platforms. Also: 2.6 edits with several instances of a module were imported
  in reverse pipe order (`multi_priority` inverted, fixed only for new
  imports, "no hope" for already migrated ones); old edits got the white
  balance reset when the chromatic adaptation default changed.
- **Evidence**: https://github.com/darktable-org/darktable/issues/17420 ,
  https://github.com/darktable-org/darktable/issues/15489 ,
  https://github.com/darktable-org/darktable/issues/3477 ,
  https://github.com/darktable-org/darktable/pull/3486 ,
  https://github.com/darktable-org/darktable/issues/7145
- **Helios exposure**: partly handled: `helios-xmp/src/legacy.rs` ports
  `legacy_params` step by step and tests each step byte for byte against
  darktable. At risk: `sidecar.rs` reads `multi_priority` and `history_end`,
  but no `iop_order` / `iop_order_version` handling was found in
  `helios-xmp` *(from grep; instance order may not match darktable's)*.
  *Addressed since* (2026-10-12): the pipe order is read
  ([pipeline.md](pipeline.md), pitfall 1). An item Helios cannot apply (a
  module it does not have, a params version it cannot migrate, a blend or
  mask, a second instance of a single-instance module, an entry it cannot
  read) is kept in the imported version with its raw attributes and the
  sidecar's text, reported in `not_applied` with its name, version and
  reason, and summed up in one warning per import; it never aborts the
  import, never moves `history_end`, and a masked item is never applied to
  the whole image. Not done: rejecting NaN or out-of-range params after a
  migration. A corpus of seven real sidecars written by darktable 3.1 to
  5.7 (`tests/darktable-sidecars`) checks what each import keeps, the pipe
  order against the order darktable-cli runs (its pipe debug output), and
  the render against darktable's where Helios has the modules. No 2.6
  sidecar yet (`xmp_version` 2 and 3 are covered by synthetic tests only).
- **Recommendation**: keep the byte-for-byte legacy tests and add render
  tests on real old sidecars (2.6, 3.x, 4.x) through the parity bench;
  read darktable's `iop_order_version` / `iop_order_list` and test a
  multi-instance sidecar from 2.6; reject NaN/out-of-range params after
  migration with a report entry instead of rendering them.

### 14. Preset and style migration crashes at startup
- **darktable**: a 5.3 master crashed at startup for users with old crop
  presets: migrating a preset ran `legacy_params`, which dereferenced
  `self->dev`, only set in the darkroom. Judged too dangerous to release.
- **Evidence**: https://github.com/darktable-org/darktable/issues/19958
- **Helios exposure**: handled by contract if module migrations stay pure
  functions of the bytes (no hidden IO, rule 2); at risk if a migration needs
  image dimensions.
- **Recommendation**: legacy conversion functions take only params (and an
  explicit, optional context); presets and styles are migrated lazily on use
  and a failure leaves the preset untouched with a report, never a crash.

### 15. History end and history consistency not persisted
- **darktable**: in 2026 master, the selected history end was not remembered
  after quitting (and history above it was partly lost); bisection pointed at
  a fix for a "compress history stack" freeze, then at a large PR merged just
  after. Earlier, "altered" status was a hard-coded SQL list of modules
  considered non-edits, inconsistent between history and module order.
- **Evidence**: https://github.com/darktable-org/darktable/issues/21943 ,
  https://github.com/darktable-org/darktable/issues/4313
- **Helios exposure**: partly handled: `helios-xmp` reads `history_end`;
  Helios versions are a DAG, so "end" is a ref, not an index.
- **Recommendation**: store the selected position as an explicit ref in the
  vault, written in the same transaction as the history; define "altered" as
  "differs from the defaults the engine would compute", computed, not a
  module list.

### 16. Duplicates (versions) as sidecar filename patterns
- **darktable**: duplicates are stored as `name_01.ext.xmp`,
  `name_02.ext.xmp`... and found on reimport by filename patterns; on Windows
  the patterns were passed to `FindFirstFileW`, which does not support them,
  so reimport saw only the first duplicate. Version numbers could collide
  with existing ones after importing old photos, which overwrote a previous
  export. Camera names like `YYYY_MM_DD_9999_1.cr3` are confused with
  darktable's own duplicate scheme.
- **Evidence**: https://github.com/darktable-org/darktable/issues/3978 ,
  https://github.com/darktable-org/darktable/issues/7935 ,
  https://github.com/darktable-org/darktable/issues/19591
- **Helios exposure**: handled for Helios's own versions (`versions` table
  with ids and parents). At risk in the darktable importer: no handling of
  `_NN` duplicate sidecars was found in `helios-xmp` *(from grep)*.
  *Addressed since* (2026-10-12): `helios_xmp::files` lists the folder and
  parses names, no glob: base, `_NN` duplicates (the suffix exactly as
  darktable prints it, so `IMG_0001` is not duplicate 1 of `IMG`), case
  variants, and orphans whose image is gone; a name that is both an
  image's own sidecar and another's duplicate is settled by
  `xmpMM:DerivedFrom`. `helios batch --sidecar --vault` and
  `helios import-darktable` keep each sidecar as its own version of the
  image (the duplicate number is part of the edit, so equal edits are
  never merged); only the base gives the rating. Exports still name files
  from a template, not from version ids.
- **Recommendation**: the darktable importer must enumerate duplicates by
  listing the folder and parsing names (not globbing), cross-checked with
  `xmpMM:DerivedFrom` (already read in `sidecar.rs`); export names derive from
  version ids, never from a counter that can be reused.

### 17. Import failures on network and virtual filesystems
- **darktable**: in 4.6, import from gvfs/SMB paths and recursive import
  reported "N images imported" while importing nothing, or skipped
  subfolders; broken in two consecutive stable releases. Importing several
  folders at once crashed (Lua `post-import` event and style application
  involved). Import from a tethered camera produced corrupted files between
  2.6.2 and 2.6.3.
- **Evidence**: https://github.com/darktable-org/darktable/issues/16091 ,
  https://github.com/darktable-org/darktable/issues/9338 ,
  https://github.com/darktable-org/darktable/issues/3924
- **Helios exposure**: at risk: the NAS agent and remote libraries are core
  goals.
- **Recommendation**: import reports counts it can prove (rows written,
  bytes hashed); verify a copy by hashing it before recording it; run the
  import test suite on a FUSE/SMB mount in CI (a local Samba container) and
  on a nested tree.

### 18. Decoder regressions from upstream raw library updates
- **darktable**: after 5.0, Fujifilm GFX 100 raws were purple: a rawspeed
  crop change (odd crop) combined with a CFA pattern reset in
  `applyCorrections()` after the crop; 5.4 broke Olympus E-410/E-510 ORF.
  The same thread notes the loader selection had just changed (magic-number
  dispatch, #17368), and that `cameras.xml` updates cannot ship separately
  because decoder versions are not tracked per camera.
- **Evidence**: https://github.com/darktable-org/darktable/issues/18073 ,
  https://github.com/darktable-org/darktable/pull/18242 ,
  https://github.com/darktable-org/darktable/issues/20011 ,
  https://github.com/darktable-org/darktable/pull/17368
- **Helios exposure**: at risk: `helios-load` decodes with rawler, a single
  upstream dependency, and the CFA is shifted to the crop in Helios code.
- **Recommendation**: a decoder regression corpus (one file per camera
  family from raw.pixls.us) with stored hashes of the decoded mosaic, black,
  white, CFA and crop; run it on every rawler bump. Record the decoder name
  and version with each image so a decode change is a visible event, not a
  silent new look of old edits.

### 19. New formats blocked by metadata and decoder dependencies (CR3, HEIC)
- **darktable**: CR3 support (158 comments, years) waited on exiv2's
  ISOBMFF support, held back by legal caution; it came via LibRaw for decoding
  plus exiv2 for metadata. LibRaw lags new bodies (EOS R6 Mark II compressed
  CR3 decoded with staircase artefacts until a LibRaw update). HEIC needed
  exiv2 ISOBMFF plus libheif; some iPhone HDR HEIC crashed darktable in the
  libheif loader (wrong row stride with transformations ignored).
- **Evidence**: https://github.com/darktable-org/darktable/issues/2170 ,
  https://github.com/darktable-org/darktable/issues/15955 ,
  https://github.com/darktable-org/darktable/issues/4989 ,
  https://github.com/darktable-org/darktable/issues/13045
- **Helios exposure**: at risk. rawler covers CR3 *(not verified for every
  body)*; HEIC/AVIF/JPEG XL readers are not in `helios-load` (it lists JPEG,
  PNG, TIFF). Licences matter (GPLv3 project, CLAUDE.md asks before adding
  incompatible dependencies).
- **Recommendation**: loaders are plugins with a declared format list and a
  test file each; read untrusted files in a sandboxed or panic-isolated
  worker; prefer Rust readers, fall back to LibRaw via FFI only per format,
  with a licence check. Publish a supported-camera list generated from the
  regression corpus.

### 20. Camera identity normalisation and noise profiles
- **darktable**: noise profiles generated by the noise tools were not found
  because they embed the EXIF maker/model while darktable looks them up by
  rawspeed's normalised names from `cameras.xml`; DNG-only bodies (Pentax,
  Ricoh) have their own aliases. Profiles are contributed per camera by users
  and each new body waits for one ("generic poissonian" fallback).
- **Evidence**: https://github.com/darktable-org/darktable/issues/8992 ,
  https://github.com/darktable-org/darktable/issues/11791 (title only) ,
  https://github.com/darktable-org/darktable/issues/14574 (title only)
- **Helios exposure**: at risk: Helios gets maker/model from rawler, whose
  normalisation may differ from rawspeed's, and darktable's noise profiles,
  presets and camera styles are keyed by rawspeed names *(guess on the
  mismatch rate; not measured)*.
- **Recommendation**: one camera-identity table mapping (rawler names,
  rawspeed names, EXIF names, aliases) to a canonical id; test that every
  entry of darktable's `noiseprofiles.json` resolves; expose the fallback
  ("no profile for this body") in the UI.

### 21. Lua API changes break user scripts
- **darktable**: the scripts repository requires a minimum API version; a
  flatpak with an older darktable could not start the script manager ("Lua
  API version 9.3.0 or later required"), fixed by checking out an older
  branch by hand. `register_event` arguments changed in 3.6. A documented
  behaviour (initialize returning an empty table) crashed darktable. Lua
  scripts appear in many import crash reports.
- **Evidence**: https://github.com/darktable-org/darktable/issues/16995 ,
  https://github.com/darktable-org/darktable/issues/9338 ,
  https://github.com/darktable-org/darktable/issues/14803 ,
  https://github.com/darktable-org/darktable/issues/9547
- **Helios exposure**: at risk once plugins exist (`helios-plugin`); ROADMAP
  2.7 already wants a declarative manifest read before code.
- **Recommendation**: semver the plugin API, declare the required range in
  the manifest, have the installer pick a compatible plugin version, and run
  plugins in a process where a crash cannot take the catalog down.

### 22. Background write-back of a stale copy reverts user changes
- **darktable**: a rating set while the full image loaded in the background
  was lost: the loader copied the whole image struct, decoded the raw, then
  wrote the stale copy back to the database.
- **Evidence**: https://github.com/darktable-org/darktable/issues/22569
- **Helios exposure**: handled in design (ROADMAP 2.8 rules 1 and 3, vault
  field commands with an audit trail).
- **Recommendation**: keep a contract test: a decode job and a rating
  command race on the same image; the rating must survive in every store
  driver.

---

## Top 5 for Helios now

1. **SQLite durability baseline** (pitfalls 2, 5): WAL, `busy_timeout`,
   transactions, `quick_check` after unclean shutdown, verified snapshots,
   no pid lock file. The SQLite driver has none of this yet and it is cheap
   to add before data accumulates.
2. **Schema versioning and migration harness** (3, 4): `user_version`,
   numbered transactional migrations, views recreated, pre-migration
   snapshot, tests on old and dirty databases, an explicit downgrade answer.
3. **darktable XMP import fidelity** (13, 16): `iop_order` / multi-instance
   order and `_NN` duplicate sidecars are not handled in `helios-xmp` today;
   add old-sidecar render tests (2.6 to 5.x) to the parity bench.
4. **Decoder and camera-identity regression corpus** (18, 20): hash the
   decoded mosaic, CFA, crop and levels per body on every rawler bump; map
   rawler names to darktable's (rawspeed) names for noise profiles and
   presets.
5. **Persist each edit step and detect external changes by hash**
   (1, 7): no session-only state, no mtime-based sync; an external sidecar
   change becomes a new version.
