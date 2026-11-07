# Import: trades, tools and facets

Status: study (2026-11-07). Nothing here is decided; the proposals are
marked "not decided" until the owner settles them. It answers: is import
one window or several, what can be pluggable, and what must the first
version of the import window hold?

Import is Helios's front door. A photographer judges the application in
the minute after the card goes in: whether the photos show at once,
whether the ones already imported are recognised, whether anything can
be lost. This study looks at who imports what (section 1), how the
market does it (section 2), the facets import takes (section 3), what a
plugin may supply (section 4), how it fits the library's model
(section 5), and ends with a recommendation and a phased plan
(section 6) and a roadmap proposal (section 7).

Sources are linked where they are used. Vendor pages are marked as such
when they make claims about their own products; forum threads are
evidence of pain, not of fact.

## In short

- **One import window, several entry points.** Every trade starts the
  same way: a source, a grid of what it holds, a few choices, a job.
  Cards, folders added in place, drops and pastes are the same window
  with a different source and placement. Tethered capture, catalogue
  migration and watched folders are separate entry points that feed the
  same import pipeline; they are not modes of the window.
- **Facets differ by trade, the core does not.** Wedding wants a
  verified backup copy, sports wants speed and caption templates, studio
  wants a live session, travel wants GPS, archives want metadata
  templates, amateurs want phones and "just the new ones", migrants want
  their stars and edits. All of them want the same core: safe copies,
  new-only, duplicates by content, a background job.
- **Pluggable at the edges.** Plugins supply sources (phones, cloud
  exports, cameras, scanners, other catalogues), steps (metadata
  templates, code replacement, conversion, AI suggestions after import)
  and session layouts. The core owns the library, identity and
  duplicates, copying and verification, the backup copy, renaming and
  layout, jobs, progress, undo and the report. A plugin never writes the
  library or a destination file itself.
- **What must never go wrong:** a file lost or overwritten, a card
  formatted before its copies are verified, an import that silently
  skipped a file, a UI blocked during ingest.
- **v1 of the import window**: sources with cards detected first, a
  grid of embedded previews with new-only on and probable duplicates
  greyed, one choices panel (copy or add, where, how named, backup
  copy, duplicates, darktable edits), a single-read verified copy, a
  background job that pauses when a card is pulled, and a final "safe
  to format" answer.

## 0. What Helios has today

From docs/library.md, docs/extensions.md, docs/jobs.md and the code of
`helios-library` (import) and `helios-gui` (shell import, importers):

- `library.import` (CLI `helios import`): a folder, file or mounted card
  (a folder holding `DCIM`), referenced in place or copied into the
  library folder as `YYYY/YYYY-MM-DD/<name>`, renamed by a pattern
  (`{date}`, `{time}`, `{camera}`, `{frame}`, `{stem}`, `{seq}`...),
  sidecars along, exact duplicates by BLAKE3 skipped, kept or asked,
  near-duplicate signals measured, darktable sidecars kept as versions,
  run as a cancellable job with one line per file.
- A copy is written to a hidden `.helios-part`, hashed as it is written,
  and takes its name only when it matches the source. Nothing is
  overwritten.
- On the desktop: File > Import… (Ctrl+Shift+I), "Import photos…" in the
  empty Library, drop and paste; a short form asks for the path, copy or
  reference, darktable edits and "remember for this workspace". File >
  Import from → lists importer plugins; Helios ships the darktable
  library importer, a Rust program under `plugins/importers/`.
- Plugins: out of process, JSON-RPC 2.0 over standard input and output,
  a declarative `helios-plugin.toml`, brokered capabilities
  (`source.read`, `folder.write`, `images.read`) and ambient ones
  granted by the user (`device.usb`, `process.spawn`, `network`...). A
  `device` service exists (the gphoto2 example: list, info, capture,
  download into a staging folder).

Gaps this study found, which the recommendation addresses:

1. **No preview before importing.** The form takes a path and imports;
   the user never sees what the card holds, which files are new, which
   are already in the library.
2. **The source is read twice on a copy**: `hash_file` reads the whole
   source to know its identity, then `copy_verified` reads it again
   while copying. On a card reader this doubles the slowest part of the
   job. Hashing while copying (one read of the card, one of the copy to
   verify it) is enough.
3. **No backup copy on ingest.** Every pro ingest tool has one (section
   2); the roadmap's continuous backup is a later, library-wide answer.
4. **No card detection.** The user browses to the card; nothing lists
   mounted cards and devices first.
5. **A pulled card fails every remaining file** one by one instead of
   pausing the job.
6. **RAW+JPEG pairs** are two unrelated images (an open roadmap
   question).
7. **Undo is inconsistent**: the roadmap says undoing a copy-import
   removes the catalogue entries it added, never the files; library.md
   says "an import is not an undo step".
8. **No metadata on import** (creator, copyright, keywords), no capture
   time offset, no GPS track.

## 1. Trades and workflows

Volumes are orders of magnitude. Where a figure comes from a vendor it
is said; where none was found it is an estimate, marked so.

### 1.1 Events and weddings

- **Session.** One or two bodies with dual card slots writing the same
  frames to both cards, several cards per day, often a second shooter.
  At home the same evening: copy every card to a working drive and a
  backup drive, then cull, then edit over days. Many keep the cards
  unformatted until delivery
  ([PetaPixel](https://petapixel.com/2019/06/20/how-to-back-up-photos-as-a-wedding-photographer/),
  [Fstoppers](https://fstoppers.com/education/my-post-wedding-workflow-image-backup-and-cataloging-87643),
  [SLR Lounge](https://slrlounge.com/never-lose-an-image-with-this-file-backup-and-cloud-storage-guide/)).
- **Volume.** 3,000 to 5,000 frames for a full day (vendor figures:
  [Lexar](https://americas.lexar.com/wedding-photographers-backup-workflow/),
  Imagen); two shooters, two to eight cards.
- **Never.** Losing a file; formatting a card whose copy was not
  verified; mixing the second shooter's frames out of time order
  (cameras whose clocks differ by minutes).
- **First 30 seconds.** Both cards (or both slots' cards) seen at once;
  the count of new frames; a destination with the client or event name;
  "and a second copy to the backup drive" ticked by the remembered
  preset; then walk away.
- **Wants from import.** Several cards in one job, a verified backup
  copy, a job or event name in the folder and file names, a capture time
  offset per camera, and fast culling right after (AI culling tools sell
  on this: [Aftershoot](https://aftershoot.com/blog/cull-wedding-photos/),
  [Narrative](https://narrative.so/blog/4-hour-wedding-workflow), both
  vendor pages).

### 1.2 Sports and press

- **Session.** Bursts at 10 to 30 frames per second; at half time or
  during the game, ingest the card (or only the frames tagged in camera,
  or a running ingest while shooting), pick a dozen, caption with a
  template and code replacement (a typed code expands into a player's
  name), transmit by FTP to the desk or the agency, often within
  minutes. Editing is minimal. Photo Mechanic is the standard
  ([Camera Bits on a big sporting event](https://home.camerabits.com/how-photo-mechanic-is-used-at-a-sporting-event/),
  [ShotKit review](https://shotkit.com/photo-mechanic-review/),
  [newspaper workflows on Dgrin](https://dgrin.com/discussion/224223/work-flow-for-newspaper-sports-newspaper-photographers),
  [Scott Kelby](https://scottkelby.com/my-sports-photography-workflow-so-far/?amp=1)).
- **Volume.** Estimate, not sourced: 1,000 to 5,000 frames a match, of
  which tens are sent.
- **Never.** Waiting: a preview that takes seconds is a lost deadline; a
  caption with the wrong name; metadata dropped on the way out.
- **First 30 seconds.** Thumbnails of the card appear before anything is
  copied; the newest frames first; tagged or protected frames
  selectable; a caption template applied on ingest.
- **Wants.** Embedded previews only, cull on the card before copying,
  incremental ("only what is new since the last ingest") and live ingest
  ([Camera Bits: Live Ingest](https://docs.camerabits.com/support/solutions/articles/48000341503)),
  IPTC templates with variables and code replacement, FTP out. Helios is
  a raw editor; this trade will judge it on ingest and culling speed
  only.

### 1.3 Studio and tethered

- **Session.** Camera on a cable (or Wi-Fi), each frame lands on the
  computer within a second and is shown large, with a preset applied
  ("next capture adjustments"), named by a counter, into a session's
  Capture folder; the art director picks on screen. Capture One sessions
  set the standard: Capture, Selects, Output and Trash folders, next
  capture naming and location
  ([Capture One: sessions](https://support.captureone.com/hc/en-us/articles/360002554438),
  [next capture adjustments](https://support.captureone.com/hc/en-us/articles/360002556677),
  [Fstoppers](https://fstoppers.com/capture-one/how-set-your-capture-one-session-improve-your-tethered-shooting-workflow-250210)).
  Lightroom Classic tethers supported Canon, Fujifilm, Nikon and Sony
  bodies, and offers Auto Import from a watched folder for the rest
  ([Adobe: tethered](https://helpx.adobe.com/lightroom-classic/help/import-photos-tethered-camera.html),
  [Adobe: auto import](https://helpx.adobe.com/lightroom/help/import-photos-automatically.html)).
  darktable tethers through gphoto2
  ([manual](https://docs.darktable.org/usermanual/development/en/tethering/overview/)).
- **Volume.** Hundreds to a few thousand frames a day, one at a time.
- **Never.** A frame that does not arrive or arrives late; a dropped
  connection that loses frames (the camera's card is the safety net).
- **First 30 seconds.** Plug the camera, see it named, take a test shot,
  see it full screen with the session's look.
- **Wants.** A session (a workspace) with a capture folder, a counter,
  a look applied on arrival, and a fallback watched folder for cameras
  the driver does not support.

### 1.4 Landscape and travel

- **Session.** Days or weeks away; a laptop or none; several cameras
  (body, phone, drone, action camera); back home, cards and phone dumps
  into one trip; GPS tracks from a phone or logger matched to the frames
  by time, which needs the camera clock's time zone offset
  ([Adobe: map module tracklogs](https://helpx.adobe.com/lightroom-classic/desktop/maps/maps-module.html),
  [Kevin Lisota on time offsets](https://kevinlisota.photography/2019/12/geocoding-photo-locations-using-lightrooms-map-module-and-gps-data/)).
  Phones shoot HEIC or Apple ProRAW (a DNG,
  [Apple](https://support.apple.com/es-lamr/HT211965),
  [PetaPixel](https://petapixel.com/2020/11/13/apple-proraw-photos-will-be-12-bit-dngs-with-14-stops-of-dynamic-range/));
  drones write DNG with GPS and altitude in EXIF.
- **Volume.** Thousands per trip, brackets and panoramas included.
- **Never.** Wrong dates (a camera still on home time), a phone's photos
  imported twice (once from the phone, once from the cloud copy).
- **First 30 seconds.** All sources of the trip in one list; frames
  sorted by real capture time across devices; a clear count of what is
  new.
- **Wants.** Capture time offsets per camera, GPX matching, HEIC and
  DNG from phones, brackets grouped (Helios's stacks), the laptop and
  home library reconciled (the roadmap's replicas).

### 1.5 Archive and scanning

- **Session.** Film or prints scanned on a flatbed or with a camera and
  a copy stand; TIFF or DNG out; a roll or box at a time; metadata the
  file cannot know (film stock, film camera, original date, roll and
  frame numbers) written by hand or from folder names, usually with
  exiftool
  ([tagging film scans](https://www.eliseomartelli.it/blog/2025-11-16-tagging-film-scans),
  [exiftool forum](https://exiftool.org/forum/index.php?msg=79880),
  [pixls.us thread](https://discuss.pixls.us/t/which-metadata-fields-for-scanned-slides-negatives/28543)).
  Institutions package each item with XMP and checksums
  ([University of Melbourne digitisation lab](https://blogs.unimelb.edu.au/digitisation-lab/?p=457)).
- **Volume.** 36 frames a roll; archives of tens of thousands; large
  files (100 MB TIFF and more).
- **Never.** Rewriting the master file; losing the link between a file
  and its physical roll and frame; a capture date set to the scan date.
- **First 30 seconds.** A folder of scans; a metadata template (stock,
  camera, roll, date) applied to all; names keeping the roll and frame.
- **Wants.** Metadata templates with per-batch fields, an "original
  date" distinct from the scan date, roll and frame in names, checksums
  (Helios's content hash already is one), never writing into the file.

### 1.6 Amateurs

- **Session.** A phone first, maybe a camera; photos backed up to iCloud
  or Google Photos; once in a while "get everything off the phone" or a
  cloud export into a folder; mixed JPEG, HEIC, videos, screenshots,
  Live Photos; the same picture arriving twice by two routes.
  Google Takeout keeps dates in JSON sidecars whose names are
  inconsistent, so tools that read EXIF only get dates wrong
  ([google-photos-takeout-date-fixer](https://www.github.com/laurentlbm/google-photos-takeout-date-fixer),
  [an Immich migration log](https://dev-portfolio-4ew.pages.dev/logbook/2025-03-22-google-to-immich)).
- **Volume.** Tens of thousands accumulated; hundreds per dump.
- **Never.** Duplicates everywhere; lost dates; anything that asks a
  technical question first.
- **First 30 seconds.** Plug the phone, see "312 new photos", press
  Import. Apple Photos is the reference: already imported items are
  shown apart from new ones
  ([MacMost](https://macmost.com/the-surprisingly-robust-photos-import-feature.html)).
- **Wants.** New-only by default, duplicates hidden, phones over MTP or
  PTP, HEIC read, cloud exports understood, no choices needed.

### 1.7 Migration from another application

- **Session.** Once, large, nervous: tens or hundreds of thousands of
  photos, years of stars, labels, keywords, collections and edits. The
  first test is a small folder; trust comes from seeing one's own stars.
- **What travels.** Files and their marks travel well; edits do not
  (different engines): Capture One imports Lightroom catalogues into its
  catalogues only, with known gaps
  ([Capture One FAQ](https://support.captureone.com/hc/en-us/articles/360003692438-Migrating-from-Lightroom-FAQ));
  digiKam cannot read a Lightroom catalogue and relies on XMP written
  out by Lightroom
  ([KDE bug 406631](https://bugs.kde.org/show_bug.cgi?id=406631),
  [digiKam users list](https://mail.kde.org/pipermail/digikam-users/2018-January/025867.html));
  darktable users redo Lightroom edits by eye
  ([pixls.us](https://discuss.pixls.us/t/solved-importing-image-edits-from-lightroom-to-darktable/41169)).
  A Lightroom catalogue is SQLite with XMP in compressed blobs (same
  pixls.us thread).
- **Never.** Writing the other application's catalogue or files; a
  migration that cannot be run again for what was offline (the
  darktable importer already says so).
- **First 30 seconds.** Pick the catalogue, see how many photos and how
  many are reachable, run on a sample first.
- **Wants.** One importer per application (darktable done), marks and
  keywords first, collections as tags or albums, edits as far as
  meaningful, a report of what did not come.

### 1.8 Common ground

Across trades the same needs repeat: see first, copy safely, skip what
is known, name sensibly, run in the background, say exactly what
happened. The differences sit at the edges: where files come from, what
is added on the way (metadata, backup, conversion), and what happens
right after (culling, captioning, tethered display).

## 2. Market

### 2.1 Lightroom Classic

- **Sources**: cards, cameras, folders, phones; a source panel with
  devices on top; tethered capture separately; Auto Import from a
  watched folder
  ([Adobe: import](https://helpx.adobe.com/lightroom/help/import-photos-video-catalog.html)).
- **Options**: Copy as DNG, Copy, Move, Add; Make a Second Copy To
  (originals only, before any change); file renaming templates;
  destination organised by date or into one folder; Apply During
  Import: develop preset, metadata preset, keywords; import presets
  saving it all; previews to build (minimal, embedded and sidecar,
  standard, 1:1, smart previews)
  ([Adobe: import options](https://helpx.adobe.com/lightroom-classic/desktop/import-photos/photo-video-import-options.html),
  [Julieanne Kost](https://jkost.com/blog/2020/09/importing-photos-into-lightroom-classic.html)).
- **Duplicates**: "Don't import suspected duplicates", matched on
  metadata (name, capture time, size), not content: renamed files and
  DNG conversions escape it, and users report files shown as new that
  are not
  ([Adobe community](https://community.adobe.com/t5/lightroom-classic-discussions/quot-don-t-import-suspected-duplicates-quot-isn-t-working-properly/m-p/13350961),
  [DNG idea](https://community.adobe.com:443/t5/idea-lightroom-classic/p:-don't-import-duplicates-should-recognize-raw-to-dng-conversions/idi-p/14263750)).
- **Speed**: "Embedded & Sidecar" previews to cull right away
  ([Lightroom Killer Tips](https://lightroomkillertips.com/see-imported-images-fast-lightroom-classic/)).
- **Pain**: slow import thumbnails from cards (minutes for some bodies,
  a blank grid until done), direct card imports far slower than a copy
  then an import
  ([Z9 thread](https://community.adobe.com/t5/lightroom-classic-discussions/lr-classic-11-2-very-slow-presenting-nikon-z9-import-thumbnails-almost-unusable/m-p/14321346/highlight/true),
  [slow thumbnails](https://community.adobe.com/questions-675/thumbnails-are-slow-to-appear-in-the-import-screen-1551379));
  "Move" from a card leaving missing files when things go wrong
  ([thread](https://community.adobe.com/questions-675/why-are-my-imported-files-missing-958391));
  DNG conversion debated for years
  ([Adobe blog](https://blog.adobe.com/en/publish/2015/12/07/dng-pros-cons-and-myths),
  [against](https://lightroomkillertips.com/dont-convert-raw-images-dng/)).

### 2.2 Capture One

- **Sources**: cards, cameras, folders; tethered capture as a first-class
  mode; sessions browse folders without import, catalogues import
  ([Capture One: sessions and catalogues](https://support.captureone.com/hc/en-us/sections/360001078997-Catalogs-and-Sessions),
  [sessions 101](https://www.captureone.com/blog/sessions-101-discussing-the-concept)).
- **Options**: into the catalogue (managed) or referenced in place;
  backup on import to a second location; naming; metadata; styles
  applied on import; "include existing adjustments"; "exclude
  duplicates"
  ([forum on backup location](https://support.captureone.com/hc/fr/community/posts/24951914077853-Capture-One-creates-a-copy-of-the-photo-in-the-backup-folder)).
- **Sessions**: Capture, Selects, Output, Trash folders; next capture
  naming, location and adjustments; hot folders for other software.
- **Duplicates**: by "key metadata", unpredictable and very slow on
  large catalogues
  ([exclude duplicates thread](https://support.captureone.com/hc/fr/community/posts/360013801198-Exclude-Duplicates-on-Import),
  [performance](https://support.captureone.com/hc/fr/community/posts/360009763837-Import-Images-Exclude-Duplicates-Shocking-performance)).
- **Migration**: imports Lightroom catalogues (catalogues only), Aperture
  and Media Pro; adjustments approximated
  ([FAQ](https://support.captureone.com/hc/en-us/articles/360003692438-Migrating-from-Lightroom-FAQ)).
- **Pain**: duplicate check speed; two mental models (session or
  catalogue) to choose up front.

### 2.3 Photo Mechanic

- **Sources**: several cards at once; folders; live ingest of a folder
  being written by a tethered or wireless camera.
- **Options**: primary and secondary destinations (tertiary for live
  ingest), folder names with variables and a job name, rename,
  IPTC template applied on ingest, incremental ingest ("only new since
  the last ingest on this computer"), eject after ingest
  ([Camera Bits: ingesting](https://docs.camerabits.com/support/solutions/articles/48000207409),
  [Live Ingest](https://docs.camerabits.com/support/solutions/articles/48000341503)).
- **Duplicates**: by an incremental record of past ingests, not by
  content; you cannot see which frames are new before ingesting
  ([jmpeltier review](https://www.jmpeltier.com/importing-editing-metadata-photo-mechanic/)).
- **Speed**: reads embedded JPEG previews only, never decodes raws;
  contact sheets of the card before any copy; the reason the trade uses
  it ([Imagen comparison](https://imagen-ai.com/post/photo-mechanic-vs-lightroom-culling/),
  vendor; [Fstoppers on embedded previews](https://fstoppers.com/education/are-lightroom-ccs-embedded-previews-worth-using-204234)).
- **Pain**: no development; Plus edition's catalogue is newer; the
  user interface is dense.

### 2.4 darktable

- **Sources**: folders ("add to library") and cards or cameras ("copy &
  import"), tethering through gphoto2.
- **Options**: base directory, sub-directory and file name patterns
  with variables (`$(YEAR)$(MONTH)$(DAY)_$(JOBCODE)`), job code,
  metadata (creator, publisher, rights, tags) applied on import, "keep
  window open"
  ([manual: import](https://docs.darktable.org/usermanual/development/en/module-reference/utility-modules/lighttable/import)).
- **Duplicates**: by folder and file name only (the film roll); no
  content hash (see docs/darktable-library-import.md).
- **Speed**: thumbnails from embedded previews by preference.
- **Pain**: "copy & import" ignores existing sidecars
  ([pixls.us](https://discuss.pixls.us/t/solved-importing-image-edits-from-lightroom-to-darktable/41169));
  no backup copy; renamed or moved folders lose their link; the import
  dialog is a file browser, not a card view.

### 2.5 digiKam

- **Sources**: collections of folders (local, removable, network share)
  scanned in place; an import tool for cameras (gPhoto2, USB mass
  storage) and phones
  ([collections](https://docs.digikam.org/en/setup_application/collections_settings.html),
  [features](https://digikam.org/about/features)).
- **Options**: rename on the fly (the advanced rename tool), automatic
  albums by date or by file type, metadata templates, convert to DNG
  on import, "identify already downloaded items"
  ([advanced import](https://docs.digikam.org/en/import_tools/advanced_import.html)).
- **Duplicates**: by visual fingerprints (Haar wavelets) after the
  fact, with a similarity threshold and a rule to pick the reference
  ([similarity view](https://docs.digikam.org/en/left_sidebar/similarity_view.html)).
- **Pain**: MTP phones on Windows and device conflicts with the desktop's
  own services
  ([KDE bug 478003](https://bugs.kde.org/show_bug.cgi?id=478003),
  [users list](https://mail.kde.org/pipermail/digikam-users/2022-September/034057.html));
  metadata renaming of raws over gPhoto2 fails; a network collection
  cannot be shared by two clients at once.

### 2.6 ON1 Photo RAW

- **Sources**: cards and folders; also browses folders without import.
- **Options**: download, back up, rename, add metadata, apply develop
  presets, edit capture time, all saved as an import preset
  ([ON1 blog](https://www.on1.com/blog/on1-photo-raw-exposed-smart-import/),
  [Behind the Shutter](https://www.behindtheshutter.com/top-5-reasons-to-use-on1-photo-raw-2018-in-your-wedding-workflow/));
  duplicate avoidance announced
  ([ON1 blog](https://www.on1.com/blog/sneak-peek-avoid-importing-duplicates/)).
- **Pain**: documentation mostly in blog posts; two models (browse and
  catalogue) again.

### 2.7 DxO PhotoLab

- **Sources**: none as such: PhotoLab has no import; it browses folders
  and indexes them for search, a file copy is the user's job
  ([DxO forum](https://forum.dxo.com/t/how-do-i-import-from-camera-solved-you-cant-through-photolab/12611),
  [user guide](https://userguides.dxo.com/photolab/en/managing-images/)).
- **Pain**: no card ingest, no backup; indexing large libraries takes
  hours; renaming or moving outside the app breaks the database. It
  shows that "add in place" alone does not serve card users.

### 2.8 Excire Foto

- **Sources**: folders, added in place, then analysed.
- **Options**: AI keywording, aesthetic rating, faces, exact and
  near-duplicate search, AI culling in recent versions
  ([Photography News](https://www.photographynews.co.uk/news/we-tested-excire-fotos-ai-photo-organisation-and-it-actually-works/),
  [Andy Hutchinson's review](https://ahutchinson.substack.com/p/excire-2025-indepth-review-a-lightning)).
- **Pain**: analysis on import slows it down; culling needs a second
  import. Lesson: AI analysis must run after the files are safe, as a
  background job, never in the copy path.

### 2.9 Apple Photos

- **Sources**: cameras, phones, cards, folders and drops.
- **Options**: copy into the library (default) or keep referenced;
  "Import All New Items"; already imported items listed apart; delete
  from the device after import (asked)
  ([Apple: storage](https://support.apple.com/en-kz/guide/photos/pht1ed9b966d/mac),
  [Apple: import](https://support.apple.com/en-la/guide/photos/pht6c803201/9.0/mac/14.0),
  [MacMost](https://macmost.com/the-surprisingly-robust-photos-import-feature.html)).
- **Duplicates**: on import by already-imported tracking; afterwards a
  Duplicates album, by machine learning, near duplicates included,
  merged on request
  ([Apple](https://support.apple.com/102260)).
- **Pain**: referenced files not backed up with the library and
  confusing to delete; "delete after import" producing duplicates in
  user reports
  ([discussion](https://discussions.apple.com/thread/251653915)).

### 2.10 Google Photos

- **Sources**: phone backup, web uploads, a desktop uploader.
- **Options**: original quality or storage saver; nothing else
  ([Google](https://support.google.com/photos/answer/6220791?hl=en)).
- **Duplicates**: identical uploads skipped, by Google's own matching
  (no official description found).
- **Exit**: Takeout, with dates and descriptions in JSON sidecars named
  inconsistently, some files with none (about 3% in one migration
  report)
  ([migration guide](https://aicybr.com/blog/google-photos-to-immich-migration-guide)).
  For Helios it is a migration source, not a competitor.

### 2.11 Rapid Photo Downloader

- **Sources**: cards, cameras, phones, folders, several at once (Linux).
- **Options**: download subfolders and file names generated from
  metadata, job codes, a per-day counter; backup to several devices
  during download; a timeline grouping frames by time gaps; "only new"
  by remembering what was downloaded
  ([man page](https://dokk.org/manpages/debian/13/rapid-photo-downloader/rapid-photo-downloader.1.en),
  [documentation](https://damonlynch.net/rapid/documentation/help04.html)).
- **Lesson**: the best free ingest tool on Linux is a standalone
  application. It proves that the ingest step (cards, generation of
  names and folders, backups, timeline) is a product of its own; Helios
  needs that quality inside.

### 2.12 Summary table

| Tool | Card view before copy | Backup copy | Duplicates | Rename / layout | Metadata on import | Develop on import | Tether / watch |
|---|---|---|---|---|---|---|---|
| Lightroom Classic | yes, slow | yes | metadata guess | templates | presets, keywords | presets | both |
| Capture One | yes | yes | metadata guess, slow | templates | yes | styles | tether, sessions, hot folder |
| Photo Mechanic | yes, fastest | 2 to 3 destinations | incremental record | variables | IPTC templates | no | live ingest |
| darktable | file browser | no | path only | patterns, job code | yes | auto presets | tether |
| digiKam | camera view | no | fingerprints after | rename tool | templates | no | no |
| ON1 | yes | yes | yes | yes | yes | presets | no |
| DxO PhotoLab | no import | no | no | no | no | no | no |
| Excire | no | no | AI, after | no | AI keywords | no | no |
| Apple Photos | yes | no | already-imported, ML after | no | no | no | no |
| Google Photos | phone backup | cloud | identical skipped | no | no | no | no |
| Rapid Photo Downloader | yes | several | download record | generated | job codes | no | no |
| **Helios today** | no | no | **content hash** | pattern | no | darktable sidecars | gphoto2 plugin (example) |

Helios's one strong card is already in hand: identity by content. No
tool in the table detects duplicates by content at import time; they
guess by name, date and size, and users complain. Making the grid grey
out what the library holds, reliably, is a visible advantage.

## 3. Facets

| Facet | Who | Fits as | Why |
|---|---|---|---|
| Ingest from cards and mounted devices | everyone with a camera | **the import window** (core), its default | the front door; must be perfect and in core |
| Add folders in place | archives, migrants, people with organised disks | **the same window**, the source a folder, placement "add" | same grid, same choices minus naming |
| Drop and paste of files | everyone | **a shortcut** into the same window (or straight to the job when the workspace has a remembered import source) | already built; keep it quick |
| Phones (MTP, PTP), cameras over USB | amateurs, travel | **a source in the window**, supplied by a **plugin** (the `device` service, libmtp or gphoto2) | platform-specific, ambient capabilities (`device.usb`) |
| Cloud exports (Google Takeout, iCloud download) | amateurs, migrants | **File > Import from →**, an **importer plugin** | a one-off migration with sidecars of its own |
| Catalogue migration (darktable, Lightroom, Capture One, digiKam) | migrants | **File > Import from →**, one **importer plugin** each | exists for darktable; same protocol |
| Tethered capture | studio | **a separate entry point** (a "Tethered session" workspace template and File > Tethered capture), camera drivers as **plugins**, each frame imported by the core pipeline | live, one frame at a time, its own view; reuses the core per frame |
| Watched or hot folders | studio fallback, wireless or FTP transfers, scanners | **a setting of a library folder or a workspace** (separate entry point, Library settings), core | background, no window; needs polling on network shares |
| Scanners | archives | **a plugin source** (SANE, TWAIN) later; most users import scanned folders | rare, platform-specific |
| Live ingest during a shoot (sports) | press | **watched folder** plus the window's "new only", core | same mechanism as hot folders |
| Transmission out (FTP to an agency) | press | **an exporter plugin**, not import | it is export |

So: one window with sources and a placement choice, two other entry
points (Import from → for catalogues and exports, a tethered session),
one background facility (watched folders), and plugins for sources the
core should not carry.

## 4. Pluggability

### 4.1 The pipeline

An import, whatever starts it, is the same pipeline:

1. **Source**: list what it holds (files with name, size, modification
   time, capture time when cheap, a thumbnail).
2. **Select**: the grid; new-only; the user's picks.
3. **Fetch**: make each file readable as a local path (already true for
   cards and folders; a download into a staging folder for phones,
   cameras and cloud).
4. **Place**: copy (hashing while reading, verifying the copy) to the
   destination and the backup destination, named by the pattern and
   laid out by the layout; or record in place.
5. **Enrich**: metadata template, values from an importer, darktable
   sidecars, capture time offset, GPS from a track.
6. **Register**: the vault records the image, its locations, its
   fields, its signals.
7. **After**: stacks suggested, AI suggestions, a tethered view showing
   the newest frame.

### 4.2 What a plugin can supply

| Kind | Examples | What it returns | Brokering |
|---|---|---|---|
| **Source** | phones over MTP, cameras over PTP or vendor SDKs, a cloud account, a scanner | a list of items (id, name, size, times, a thumbnail), then files fetched into a staging folder | `device.usb`, `network` ambient; `folder.write` brokered staging (as `device.download` today) |
| **Importer** (exists) | darktable, Lightroom, Capture One, digiKam catalogues, Google Takeout | paths plus marks (rating, reject, labels, tags, title, description), later version 2 blocks (edits, GPS, collections) | `source.read`, as today |
| **Metadata step** | IPTC template with variables, code replacement, film metadata from folder names, GPX matching | field values per file, as importer values (set if absent, labels and tags added) | `images.read` for the file at hand |
| **Name fields** | extra variables for the rename pattern (a job's client code, a counter from an external system) | text values per file; the core still builds the name | none |
| **Convert step** | DNG conversion, HEIC to TIFF for old software | a derived file written into staging; the core imports it as a new image linked to its original | `folder.write` |
| **After-import resolver** | AI culling (blur, closed eyes, best of a burst), AI keywords | a decision proposal (as `stacks.suggest`), applied only when a person accepts | `ai.models`, `images.read` |
| **Session layout** | Capture One-like session folders, an agency's folder convention | a layout description (folders for captures, selects, output, trash) the core creates and uses | none |

### 4.3 What the core owns

- **The library**: identity by content hash, locations, fields,
  versions, the audit trail. No plugin writes it.
- **Duplicates**: exact by hash, probable by the prefilter shown in the
  grid, near-duplicate signals. A plugin can propose; only the core
  decides what "already in the library" means.
- **Copying and verification**: every byte written to a destination,
  the hidden part file, the verify, the backup copy, the never-overwrite
  rule. A plugin only writes into a staging folder the broker hands it.
- **Naming and layout**: the pattern language and its fields; plugins
  may add fields and layouts, not write names themselves.
- **Jobs, progress, cancel, pause on a lost source, the report, the Log
  panel, undo.**
- **The import window** and its choices; a plugin adds entries to its
  lists (sources, steps, layouts) through the manifest, not UI code
  (the roadmap's "90% without front code").

### 4.4 A manifest sketch (not decided)

Following the existing `helios-plugin.toml`: official plugins are Rust
(or C or C++ where a vendor SDK requires it), live under
`plugins/<category>/<name>`, run out of process and speak JSON-RPC.

```toml
# plugins/sources/mtp-devices/helios-plugin.toml
id = "io.github.quazardous.helios.mtp-devices"
name = "phones and tablets"
version = "0.1.0"
license = "GPL-3.0-or-later"
command = ["helios-mtp-devices"]
services = ["import.source"]
activation = ["onImportSource:mtp"]

[compat]
helios = ">=0.70.0, <0.71.0"
interfaces = { protocol = 1, "import.source" = 1 }

[[import.sources]]
id = "mtp"
label = "Phones and tablets"
detect = "devices"        # listed in the window's Sources while a device is present
fetch = "staging"         # files are downloaded into a staging folder ("path": readable in place)
thumbnails = true         # the plugin can give thumbnails without fetching whole files

[[capabilities]]
name = "device.usb"
reason = "Talks to the phones plugged in over USB (MTP)"

[[capabilities]]
name = "folder.write"
reason = "Downloads the photos you chose into the folder Helios hands over"
```

```toml
# plugins/steps/iptc-template/helios-plugin.toml (fragment)
services = ["import.step"]

[[import.steps]]
id = "iptc-template"
label = "Caption template"
stage = "metadata"        # metadata | name-fields | convert
settings = "template"     # the step's settings live in the import preset, typed by a schema below

[import.steps.schema]
caption = { type = "string", about = "Caption, with {variables} and =codes= replaced" }
codes = { type = "file", about = "A code replacement file (tab separated)" }
```

Methods, Helios to the plugin (sketch):

| Method | Params | Result |
|---|---|---|
| `source.detect` | `source` | devices or accounts present: `[{id, label, kind}]` |
| `source.list` | `source`, `device`, `progress` | items: `[{item, name, size, modified, taken?, kind}]`, streamed with `$/progress` |
| `source.thumbnail` | `device`, `items` | small JPEG bytes (base64) or a path in staging |
| `source.fetch` | `device`, `items`, `staging`, `progress` | the files written into staging, as `device.download` does |
| `step.apply` | `step`, `settings`, `files: [{path, fields}]` | `[{path, values}]`, values applied as importer values are |
| `step.convert` | `step`, `settings`, `path`, `staging` | a derived file in staging, with the relation (`derived_from`) |

The importer protocol stays as it is (`importer.scan`); its version 2
(darktable edits, GPS, collections) is already sketched in
docs/darktable-library-import.md.

### 4.5 Which steps are core, which are plugins

- **Core**: copy, verify, backup copy, rename pattern, date layout,
  duplicates, darktable sidecars, a plain metadata template (creator,
  copyright, keywords, a description), capture time offset per camera.
  These are needed by most trades and touch identity or safety.
- **Official plugins**: MTP phones, vendor camera drivers, catalogue
  importers, Takeout, code replacement and IPTC captions, GPX matching
  (could move to core if travel users ask), DNG conversion, AI culling
  resolvers, session layouts beyond the default.
- **Third-party plugins**: agency conventions, studio management
  systems, scanners, cloud services.

## 5. Fit with Helios's model

**Reference or copy.** Both stay, as today, under plain words: "Copy to
the library folder" and "Add where they are". Proposal: a source on
removable media (a card, a camera, a phone) defaults to copy, and "add
where they are" is offered only with a warning, because a referenced
card file is gone when the card is formatted (users of Apple Photos
report that it copies card imports even when set to reference, and
referenced files bring their own troubles:
[Apple discussion](https://discussions.apple.com/docs/DOC-10612)).
"Move" (copy then delete the source) is not offered for cards; for
folders on disk it may come later, after verification, as Lightroom
does.

**The hidden vault.** The user sees "Library" and "the library folder".
The import window never says vault, database or hash; it says "already
in your library", "new", "copied and checked".

**RAW+JPEG pairs.** Identity is the file's content, so a pair is two
images. Proposal (not decided): link them on import by a `pair` relation
(same folder, same stem, same capture time), show them as one cell with
a "RAW+JPEG" badge, as a collapsed stack whose top is the raw; the
choices panel offers "both", "raw only" or "JPEG only". This answers the
roadmap's open question without bending identity (Lightroom's
preference "treat JPEG files next to raw files as separate photos" is
the reference for the choice).

**Sidecars.** Carry along on a copy, by name rules: XMP (darktable's
`NAME.EXT.xmp` and duplicates, Lightroom's and Capture One's `NAME.xmp`),
Apple `.AAE` edits, WAV voice notes, Google Takeout `.json`. Reading
them is separate: XMP marks and darktable edits in core; Takeout JSON
and other formats by importer plugins. A sidecar is never edited in
place on the source.

**Duplicates by content hash.** Hash on the way in, in one read: the
copy is hashed while written from the source (one read of the card),
then the copy is re-read to verify. For the grid, a prefilter answers
"probably already in your library" before anything is hashed: the same
size and the same partial hash (first and last 64 KiB) as an image the
library holds, or the same name, size and capture time as a location
the vault already records; greyed as "probably in library", confirmed
or cleared by the full hash during the job. Content identity also
recognises a file renamed or moved since, which name-based tools miss.
A file that a tool rewrote (XMP written into a DNG) is a new hash: the
roadmap's fallback match on capture time, camera serial and size should
mark it "same photo, changed file" and ask.

**Undo.** Proposal: an import is one step of the library's history;
undoing it removes the entries it added (and the versions and values it
set), never a file. Copies made stay in the library folder and are
listed with a "Move copied files to the trash" action, asked. This
matches the roadmap and requires library.md's "an import is not an undo
step" to change.

**A card pulled out mid-import.** Today each remaining file fails.
Proposal: the job notices the source is gone (the mount point vanished
or reads fail with a device error), stops, keeps what is done (all
copies verified, the part file removed), and shows "Card removed: 1,204
of 3,410 imported. Put it back to continue." Re-inserting the same card
(same volume id) resumes; anything already imported is skipped by its
hash. Nothing is reported as failed for a pulled card.

**Safe to format.** At the end, a line per card: "Every photo of this
card is in your library (and on the backup drive)", computed by hash,
not by "the job finished". Helios never formats or erases a card.

**Network shares.** References on a share are fine (the path is a
last-seen hint, the hash is identity), but hashing reads every byte
over the network: the window must say so and the job must throttle.
Watched folders on a share need polling, since inotify and its kin do
not see changes made by other machines
([LWN](https://lwn.net/Articles/896055),
[csi-driver-smb discussion](https://github.com/kubernetes-csi/csi-driver-smb/discussions/641)).
Copies to a share follow the same part file, verify and rename, with a
sync before the rename. Several clients writing one library over a share
is the roadmap's replica model, not a shared SQLite file (digiKam
documents that its shared database does not support two clients at
once,
[collections](https://docs.digikam.org/en/setup_application/collections_settings.html)).

**Jobs.** An import is one job with steps (list, copy, verify, backup,
register, signals); a multi-card import is a parent with a child per
card; thumbnails and signals run as low-priority work that never holds
the copy back. AI analysis is a separate job after.

**Templates.** The workspace's import source (docs/templates.md) becomes
an **import preset**: placement, destination, layout, rename pattern,
backup destination, duplicates, metadata template, steps. The built-in
template "Import, cull, de-duplicate" carries one; a "Tethered session"
template would carry another.

## 6. Recommendation

### 6.1 Facet map

```text
                         +-------------------------------+
  File > Import...  ---> |         IMPORT WINDOW         |  <--- drop, paste
  empty Library     ---> | Sources | Grid | Choices      |
                         +-------------------------------+
                            |  cards, devices (core)
                            |  folders (core: copy or add)
                            |  phones, cameras, cloud (source plugins)
                            v
  File > Import from -> importer plugins (darktable, Lightroom,
                        Capture One, digiKam, Takeout) ---+
                                                           |
  Tethered session   -> device plugins, one frame at a    |
  (workspace template)  time                               |
                                                           v
  Watched folders    -> library setting, polling   ---> CORE PIPELINE
                                                      fetch, copy+verify,
                                                      backup, name, layout,
                                                      enrich (steps),
                                                      register, signals
                                                           |
                                                           v
                                         after: stacks, AI suggestions,
                                         culling (decisions, asked)
```

### 6.2 v1: the import window

What the window being designed must have, nothing more:

1. **Sources, cards first.** A left list: mounted cards and cameras
   (removable volumes holding `DCIM`, refreshed when one appears, named
   by their label and capacity), then recent and favourite folders, then
   "Choose a folder…". Several cards can be ticked for one import. A
   card inserted while the window is closed offers to open it (a toast,
   never a modal window).
2. **A grid of what the source holds**, from embedded previews and EXIF
   thumbnails read in the order they are seen, the first ones within a
   second; capture time order; "New only" on by default; probable
   duplicates greyed with "in your library" (prefilter), confirmed by
   hash during the job; RAW+JPEG pairs as one cell; a count line
   ("3,410 photos, 2,980 new, 61 GB"); select, deselect, select by day.
3. **One choices panel**, top to bottom: Copy to the library folder or
   Add where they are (copy forced for cards unless overridden);
   destination and layout with a live example path; rename pattern with
   a live example name; Backup copy to (off by default, remembered);
   Duplicates (skip by default, keep, ask); Read darktable edits;
   Remember as this workspace's import preset. Advanced choices folded.
4. **A single-read, verified copy** (hash while copying, verify the
   copy), the backup copy verified the same way.
5. **A background job**, the window closable at once; per-card child
   jobs; pause on a pulled card, resume on reinsertion; the existing
   cancel.
6. **The end**: a toast with counts, a "safe to format" line per card,
   the report in the Log panel, and the new photos selected in the
   Library, ready to cull.
7. **Undo** of an import as one library step (entries, never files).

### 6.3 Later, as core

- Metadata template on import (creator, copyright, keywords,
  description), with variables. (S)
- Capture time offset per camera, set in the window from one frame of
  known time. (S)
- Watched folders, with polling on network shares. (M)
- Pairs as stacks, burst grouping visible in the grid. (M)
- Incremental live ingest during a shoot (a watched card or folder plus
  new-only). (S, once watched folders exist)

### 6.4 Later, as plugins

- Phones over MTP and PTP (source). (M)
- Google Takeout and iCloud exports (importer, JSON sidecars). (M)
- Lightroom Classic catalogue (importer: SQLite, XMP blobs, collections
  as tags). (L)
- Capture One catalogue and sessions (importer). (L)
- digiKam database (importer). (M)
- IPTC captions with code replacement (step). (S)
- GPX matching (step). (S)
- DNG conversion (convert step, never replacing the original). (M)
- AI culling and keywords after import (resolvers, decisions). (L)
- Tethered sessions with vendor SDKs (device plugins, a session
  template). (L)
- Scanners (source). (M)

### 6.5 What Helios never does

- Format or erase a card, or delete from a device after import.
- Move files off a card ("Move" is copy plus delete).
- Overwrite a file, in a destination or a source.
- Write into originals on import (no XMP into raws or DNGs, no
  conversion in place).
- Let a plugin write the library or a destination file itself.
- Replace a raw by a DNG by default.
- Block the interface while importing, or analyse with AI in the copy
  path.
- Upload to a cloud as a side effect of importing.
- Call something a duplicate by name or date alone.

### 6.6 Phased plan

| Phase | Content | Size |
|---|---|---|
| 1. Pipeline fixes | single-read copy (hash while copying), pause and resume on a lost source, "safe to format" check by hash, backup destination verified, import as one undo step | M |
| 2. The import window | sources with card detection, the embedded preview grid with new-only and greyed duplicates (prefilter), pairs as one cell, the choices panel with live examples, import presets replacing the "remember" box | L |
| 3. Enrichment in core | metadata template, capture time offset per camera, pairs as stacks | M |
| 4. Source and step protocol | `import.source` and `import.step` interfaces in the broker and manifest, staging fetch reused from `device` | M |
| 5. First plugins | MTP phones, Google Takeout, IPTC caption step, GPX step | M each, S for the steps |
| 6. Migrations | Lightroom, Capture One, digiKam importers; importer version 2 (edits, GPS, collections) | L |
| 7. Live work | watched folders, tethered session template with the gphoto2 plugin, live ingest | L |
| 8. After import | AI culling and keywords as decision resolvers | L |

Phases 1 and 2 are the import window; 3 to 8 follow demand, and can be
reordered by trade (studio first means phase 7 earlier).

## 7. Roadmap proposal (not decided)

A short entry for ROADMAP.md, section 2.3, for the owner to settle:

> (2026-11-07, not decided) **Import: one window, pluggable at the
> edges** (docs/import-research.md). Cards, folders and drops share one
> import window: sources with cards detected first, a grid of embedded
> previews with new-only on and probable duplicates greyed, one choices
> panel (copy or add, destination, name, backup copy, duplicates,
> darktable edits) and a background job that pauses when a card is
> pulled and ends with a "safe to format" check by hash. The copy reads
> the source once. Catalogue migrations stay under File > Import from →,
> tethered capture is a session template, watched folders a library
> setting; all feed the same core pipeline. Plugins supply sources
> (phones, cameras, cloud, scanners), steps (captions, GPS, conversion)
> and layouts, and never write the library or a destination file. An
> import is one undo step that removes entries, never files.
> RAW+JPEG pairs are two images linked as a pair, shown as one.

## Sources

Vendor documentation:
[Adobe: import](https://helpx.adobe.com/lightroom/help/import-photos-video-catalog.html),
[Adobe: import options](https://helpx.adobe.com/lightroom-classic/desktop/import-photos/photo-video-import-options.html),
[Adobe: tethered](https://helpx.adobe.com/lightroom-classic/help/import-photos-tethered-camera.html),
[Adobe: auto import](https://helpx.adobe.com/lightroom/help/import-photos-automatically.html),
[Adobe: map module](https://helpx.adobe.com/lightroom-classic/desktop/maps/maps-module.html),
[Adobe blog: DNG](https://blog.adobe.com/en/publish/2015/12/07/dng-pros-cons-and-myths),
[Capture One: catalogues and sessions](https://support.captureone.com/hc/en-us/sections/360001078997-Catalogs-and-Sessions),
[Capture One: sessions](https://support.captureone.com/hc/en-us/articles/360002554438),
[Capture One: next capture adjustments](https://support.captureone.com/hc/en-us/articles/360002556677),
[Capture One: migrating from Lightroom](https://support.captureone.com/hc/en-us/articles/360003692438-Migrating-from-Lightroom-FAQ),
[Capture One: sessions 101](https://www.captureone.com/blog/sessions-101-discussing-the-concept),
[Camera Bits: ingesting](https://docs.camerabits.com/support/solutions/articles/48000207409),
[Camera Bits: live ingest](https://docs.camerabits.com/support/solutions/articles/48000341503),
[Camera Bits: a sporting event](https://home.camerabits.com/how-photo-mechanic-is-used-at-a-sporting-event/),
[darktable: import](https://docs.darktable.org/usermanual/development/en/module-reference/utility-modules/lighttable/import),
[darktable: tethering](https://docs.darktable.org/usermanual/development/en/tethering/overview/),
[digiKam: advanced import](https://docs.digikam.org/en/import_tools/advanced_import.html),
[digiKam: collections](https://docs.digikam.org/en/setup_application/collections_settings.html),
[digiKam: similarity](https://docs.digikam.org/en/left_sidebar/similarity_view.html),
[digiKam: features](https://digikam.org/about/features),
[ON1: smart import](https://www.on1.com/blog/on1-photo-raw-exposed-smart-import/),
[ON1: duplicates](https://www.on1.com/blog/sneak-peek-avoid-importing-duplicates/),
[DxO: managing images](https://userguides.dxo.com/photolab/en/managing-images/),
[Apple: storage and referenced files](https://support.apple.com/en-kz/guide/photos/pht1ed9b966d/mac),
[Apple: import](https://support.apple.com/en-la/guide/photos/pht6c803201/9.0/mac/14.0),
[Apple: Duplicates album](https://support.apple.com/102260),
[Apple: ProRAW](https://support.apple.com/es-lamr/HT211965),
[Google: backup quality](https://support.google.com/photos/answer/6220791?hl=en),
[Rapid Photo Downloader man page](https://dokk.org/manpages/debian/13/rapid-photo-downloader/rapid-photo-downloader.1.en),
[Rapid Photo Downloader documentation](https://damonlynch.net/rapid/documentation/help04.html).

Reviews, guides and practitioners:
[Julieanne Kost](https://jkost.com/blog/2020/09/importing-photos-into-lightroom-classic.html),
[Lightroom Killer Tips: fast previews](https://lightroomkillertips.com/see-imported-images-fast-lightroom-classic/),
[Lightroom Killer Tips: against DNG](https://lightroomkillertips.com/dont-convert-raw-images-dng/),
[Fstoppers: embedded previews](https://fstoppers.com/education/are-lightroom-ccs-embedded-previews-worth-using-204234),
[Fstoppers: Capture One sessions](https://fstoppers.com/capture-one/how-set-your-capture-one-session-improve-your-tethered-shooting-workflow-250210),
[Fstoppers: wedding backup](https://fstoppers.com/education/my-post-wedding-workflow-image-backup-and-cataloging-87643),
[PetaPixel: wedding backup](https://petapixel.com/2019/06/20/how-to-back-up-photos-as-a-wedding-photographer/),
[PetaPixel: ProRAW](https://petapixel.com/2020/11/13/apple-proraw-photos-will-be-12-bit-dngs-with-14-stops-of-dynamic-range/),
[SLR Lounge: backup](https://slrlounge.com/never-lose-an-image-with-this-file-backup-and-cloud-storage-guide/),
[ShotKit: Photo Mechanic](https://shotkit.com/photo-mechanic-review/),
[J.M. Peltier: Photo Mechanic](https://www.jmpeltier.com/importing-editing-metadata-photo-mechanic/),
[Scott Kelby: sports workflow](https://scottkelby.com/my-sports-photography-workflow-so-far/?amp=1),
[Dgrin: newspaper workflow](https://dgrin.com/discussion/224223/work-flow-for-newspaper-sports-newspaper-photographers),
[Behind the Shutter: ON1](https://www.behindtheshutter.com/top-5-reasons-to-use-on1-photo-raw-2018-in-your-wedding-workflow/),
[Photography News: Excire](https://www.photographynews.co.uk/news/we-tested-excire-fotos-ai-photo-organisation-and-it-actually-works/),
[Andy Hutchinson: Excire](https://ahutchinson.substack.com/p/excire-2025-indepth-review-a-lightning),
[MacMost: Photos import](https://macmost.com/the-surprisingly-robust-photos-import-feature.html),
[Kevin Lisota: geocoding](https://kevinlisota.photography/2019/12/geocoding-photo-locations-using-lightrooms-map-module-and-gps-data/),
[Eliseo Martelli: tagging film scans](https://www.eliseomartelli.it/blog/2025-11-16-tagging-film-scans),
[University of Melbourne digitisation lab](https://blogs.unimelb.edu.au/digitisation-lab/?p=457),
[LWN: network filesystem notifications](https://lwn.net/Articles/896055).

Vendor marketing (claims about their own products):
[Lexar: wedding backup](https://americas.lexar.com/wedding-photographers-backup-workflow/),
[Imagen: Photo Mechanic vs Lightroom](https://imagen-ai.com/post/photo-mechanic-vs-lightroom-culling/),
[Aftershoot: culling weddings](https://aftershoot.com/blog/cull-wedding-photos/),
[Narrative: wedding workflow](https://narrative.so/blog/4-hour-wedding-workflow).

Forums and bug trackers:
[Lightroom: suspected duplicates](https://community.adobe.com/t5/lightroom-classic-discussions/quot-don-t-import-suspected-duplicates-quot-isn-t-working-properly/m-p/13350961),
[Lightroom: duplicates and DNG](https://community.adobe.com:443/t5/idea-lightroom-classic/p:-don't-import-duplicates-should-recognize-raw-to-dng-conversions/idi-p/14263750),
[Lightroom: Z9 import thumbnails](https://community.adobe.com/t5/lightroom-classic-discussions/lr-classic-11-2-very-slow-presenting-nikon-z9-import-thumbnails-almost-unusable/m-p/14321346/highlight/true),
[Lightroom: slow import thumbnails](https://community.adobe.com/questions-675/thumbnails-are-slow-to-appear-in-the-import-screen-1551379),
[Lightroom: missing files after a move](https://community.adobe.com/questions-675/why-are-my-imported-files-missing-958391),
[Capture One: backup location](https://support.captureone.com/hc/fr/community/posts/24951914077853-Capture-One-creates-a-copy-of-the-photo-in-the-backup-folder),
[Capture One: exclude duplicates](https://support.captureone.com/hc/fr/community/posts/360013801198-Exclude-Duplicates-on-Import),
[Capture One: duplicate check performance](https://support.captureone.com/hc/fr/community/posts/360009763837-Import-Images-Exclude-Duplicates-Shocking-performance),
[DxO: no import from camera](https://forum.dxo.com/t/how-do-i-import-from-camera-solved-you-cant-through-photolab/12611),
[KDE bug 406631: Lightroom migration](https://bugs.kde.org/show_bug.cgi?id=406631),
[KDE bug 478003: MTP phone](https://bugs.kde.org/show_bug.cgi?id=478003),
[digiKam users: Android import](https://mail.kde.org/pipermail/digikam-users/2022-September/034057.html),
[digiKam users: Lightroom XMP](https://mail.kde.org/pipermail/digikam-users/2018-January/025867.html),
[pixls.us: Lightroom edits to darktable](https://discuss.pixls.us/t/solved-importing-image-edits-from-lightroom-to-darktable/41169),
[pixls.us: scan metadata](https://discuss.pixls.us/t/which-metadata-fields-for-scanned-slides-negatives/28543),
[exiftool forum: DSLR scans](https://exiftool.org/forum/index.php?msg=79880),
[Apple: referenced library](https://discussions.apple.com/docs/DOC-10612),
[Apple: delete after import](https://discussions.apple.com/thread/251653915),
[Takeout date fixer](https://www.github.com/laurentlbm/google-photos-takeout-date-fixer),
[Google to Immich log](https://dev-portfolio-4ew.pages.dev/logbook/2025-03-22-google-to-immich),
[Google Photos to Immich guide](https://aicybr.com/blog/google-photos-to-immich-migration-guide),
[csi-driver-smb: inotify on SMB](https://github.com/kubernetes-csi/csi-driver-smb/discussions/641).
