# Generated from Helios's declarations and code by `cargo xtask i18n extract`:
# do not edit. Translations live in i18n/<locale>/ under the same names.

batch-asking = not exported: the run { $run } waits for a person to answer a decision

batch-cancelled = cancelled

batch-cannot-create = cannot create { $path }: { $error }

batch-cannot-read = cannot read { $path }: { $error }

batch-held = not exported: a step before the export failed or was cancelled

batch-needs-network = batch needs --network FILE, or a node to attach to

batch-no-source = the network has no source

batch-node-refused = the node refused

batch-node-unreachable = cannot reach the node at { $address }: { $error }

batch-run-gone = the run { $run } left the node

batch-run-paused = not exported: the run { $run } is paused

batch-several-ends = the network ends in several nodes ({ $names }): say --node NAME

batch-several-sources = the network has several sources ({ $names }): say --source NAME

batch-stopped = stopped before it was written: the same batch run again finishes it

batch-vault-attached = --vault with a running node only keeps sidecars: it needs --sidecar (the node keeps its runs in its own vault)

engine-failure-compile = compile

engine-failure-device-lost = device lost

engine-failure-dispatch = dispatch

engine-failure-other = other

engine-failure-out-of-memory = out of memory

engine-failure-timeout = timeout

engine-gpu-fell-back = { $module } ran on the CPU: its GPU version failed ({ $failure }: { $reason }).

engine-gpu-lost = The GPU ({ $gpu }) failed ({ $failure }: { $reason }) and could not be restarted ({ $why }); the rest of this session renders on the CPU.

engine-gpu-lost-recovered = The GPU ({ $gpu }) failed ({ $failure }: { $reason }); it was restarted and the render replayed.

export-cancelled = cancelled

export-channels = cannot export an image of { $channels } channels

export-encode-failed = cannot encode the image: { $error }

export-folder-failed = cannot make the folder { $folder }: { $error }

export-no-format = no format for { $path }

export-not-a-file = { $path } is not a file path

export-not-a-mask = a mask has one channel, not { $channels }

export-png-bits = PNG has 8 or 16 bits, not { $bits }

export-write-failed = cannot write { $path }: { $error }

gui-about-description = A client/server, AI-first image processing framework.

gui-account-connected = connected

gui-account-connecting = connecting…

gui-account-needs-attention = needs attention

gui-account-not-connected = not connected

gui-account-pick = Pick photos…

gui-account-reconnect = Reconnect

gui-account-sign-out = Sign out

gui-account-sign-out-explain = Helios forgets this account's keys and tokens on this computer. What was imported stays.

gui-account-sign-out-title = Sign out of { $account }?

gui-backup-card-safe = { $card } is safe to format: every photo has its backups

gui-backup-wizard = Set up backups

gui-backup-wizard-about = Helios keeps verified copies of every photo on your backup disks: choose how many, where, and when.

gui-backup-wizard-copies = Copies of each photo

gui-backup-wizard-copies-about = Each copy is on a different disk, besides the photo in the library. Two copies survive a disk failing.

gui-backup-wizard-destination = Copy to

gui-backup-wizard-destination-archive = Any disk with the archive role

gui-backup-wizard-destination-backup = Any disk with the backup role

gui-backup-wizard-destination-help = Give a disk its role on Storage › Volumes; one disk per copy.

gui-backup-wizard-finish = Use this strategy

gui-backup-wizard-off-site = Keep one of the copies off site

gui-backup-wizard-off-site-help = A disk kept elsewhere survives a fire or a theft: mark it off site on Storage › Volumes.

gui-backup-wizard-on-connect = When a backup disk is plugged in

gui-backup-wizard-on-import = After each import

gui-backup-wizard-open = Set up backups…

gui-backup-wizard-step-copies = Copies

gui-backup-wizard-step-destination = Disks

gui-backup-wizard-step-off-site = Off site

gui-backup-wizard-step-when = When

gui-backup-wizard-when-about = Back up now on the Backup page makes copies at any time.

gui-caches-clear = Clear

gui-caches-clear-all = Clear caches

gui-caches-clear-all-ask = Thumbnails are read again from the files and images rendered again when shown: slower for a while. Photos, edits and the catalogue are not touched.

gui-caches-clear-all-title = Clear every cache?

gui-caches-none = No cache holds anything yet.

gui-caches-size = { $size }, { $entries } entries

gui-caches-size-of = { $size } of { $budget }, { $entries } entries

gui-caches-tier-disk = on disk

gui-caches-tier-ram = in memory

gui-caches-tier-vram = in video memory

gui-caches-total = Caches hold { $size }

gui-caches-use = In use

gui-caption-decisions = Decisions waiting

gui-caption-decisions-count = { $count } stack suggestions waiting for an answer

gui-caption-jobs = Show the jobs

gui-caption-profile = Profile in use: { $name }

gui-catalogue-days = { $days } days ago

gui-catalogue-here = next to the catalogue

gui-catalogue-none = No snapshot yet.

gui-catalogue-on-disk = on { $disk }

gui-catalogue-one-day = 1 day ago

gui-catalogue-restore = Restore…

gui-catalogue-restore-ask = Restore the catalogue from the snapshot of { $when }? Changes made since are lost. The current catalogue is kept aside, and Helios closes and starts again on the snapshot.

gui-catalogue-restore-confirm = Restore and restart

gui-catalogue-restore-title = Restore the catalogue?

gui-catalogue-snapshot-now = Snapshot now

gui-catalogue-today = today

gui-compare-first = Compare shows { $count } photos at most: the first { $count } are compared

gui-compare-rating = Active photo's rating

gui-compare-too-few = Select two or more photos to compare them

gui-confirmed-by-policy = { $verb }: confirmed by policy

gui-connect-browser = Sign in in your browser, then come back here.

gui-connect-cancel = Cancel

gui-connect-close = Close

gui-connect-done = { $account } is connected.

gui-connect-failed = Not connected: { $why }

gui-connect-next = Connect

gui-connect-open-again = Open the page again

gui-connect-secret-refused = A secret is typed by the person, never given by an agent

gui-connect-title = Connect { $source }

gui-connect-working = Connecting…

gui-copied = copied { $text }

gui-copied-files = copied { $count } files

gui-copied-files-missing = copied { $count } files; { $missing } without a file left out

gui-copied-image = copied the image ({ $width } × { $height })

gui-copy-image-failed = the image was not copied: { $error }

gui-decisions-accept = Accept

gui-decisions-accept-above = Accept the { $count } above { $percent } %

gui-decisions-answer = Answer…

gui-decisions-cancel = Cancel

gui-decisions-confidence = Confidence: { $percent } %

gui-decisions-correct = Correct…

gui-decisions-correct-hint = Change the proposal before it is recorded

gui-decisions-count = { $count } decisions waiting

gui-decisions-mode-assisted = { $node }: a model proposes, you settle

gui-decisions-mode-full-ai = { $node }: decided by a model, you may answer instead

gui-decisions-mode-human = { $node }: answered by you

gui-decisions-none = No decision waiting for an answer

gui-decisions-other-options = { $count } other options

gui-decisions-proposal = { $model } proposes:

gui-decisions-record = Record

gui-decisions-threshold-hint = How sure the model must be for its proposal to be taken

gui-decisions-waiting-model = Waiting for the model

gui-decisions-why = Why: { $why }

gui-duplicates-asked = Keep it as one more copy, or skip it?

gui-duplicates-clear = Clear the answered

gui-duplicates-declined = Skipped

gui-duplicates-keep = Keep

gui-duplicates-keep-all = Keep all

gui-duplicates-kept = Kept as one more copy

gui-duplicates-located = Kept as one more copy, as the import's policy said

gui-duplicates-none = No duplicates found at import

gui-duplicates-of-library = Already in the library: { $at }

gui-duplicates-of-source = The same file as { $first }, earlier in this import

gui-duplicates-skip = Skip

gui-duplicates-skip-all = Skip all

gui-duplicates-skipped = Skipped, as the import's policy said

gui-duplicates-waiting = { $count } duplicates wait for an answer

gui-duplicates-waiting-note = { $count } duplicates wait for an answer in the Duplicates panel

gui-edit-does-not-fit = Part of the saved edit does not fit this network: { $part }

gui-edit-not-saved = The edit was not saved: { $error }

gui-export-bits = PNG bits per channel

gui-export-cancelled = The export was cancelled: { $written } photos written to { $folder }

gui-export-clash = when a file name is taken

gui-export-ended = { $written } photos exported to { $folder }

gui-export-ended-failed = { $written } photos exported to { $folder }; { $failed } failed: see the Log panel

gui-export-ended-skipped = { $outcome } ({ $skipped } left out: their file exists)

gui-export-failed = The export did not start: { $error }

gui-export-folder = folder

gui-export-form-title = Export photos

gui-export-format = format

gui-export-image-failed = { $image }: not exported: { $error }

gui-export-long-edge = long edge (pixels)

gui-export-long-edge-help = The longer side at most; 0 keeps the full size

gui-export-open-folder = Open folder

gui-export-pattern = file names

gui-export-pattern-help = As an import renames: { $fields }; the extension follows the format

gui-export-quality = JPEG quality

gui-export-quality-help = 1 to 100, for JPEG only

gui-export-queued = Exporting { $count } photos to { $folder }

gui-export-remember = Remember for this workspace

gui-export-remember-help = The workspace's export settings: the next export starts from them

gui-export-scope = photos

gui-export-scope-help = selected: the { $selected } selected; shown: the { $shown } the library shows

gui-export-space = colour space

gui-export-verb = Export

gui-gesture-in-progress = { $gesture } is in progress

gui-graph-add-macro = Macros › { $name }

gui-graph-label = The workspace's network

gui-graph-no-workspace = No workspace is open: its network shows here.

gui-graph-state-cached = cached

gui-graph-state-done = done

gui-graph-state-failed = failed

gui-graph-state-idle = idle

gui-graph-state-pending = waiting

gui-graph-state-running = running

gui-graph-state-stale = stale

gui-graph-wires = Wires

gui-graph-wires-curves = Curves

gui-graph-wires-orthogonal = Orthogonal

gui-graph-wires-straight = Straight

gui-health-about = Helios checks the backups, the disks, the GPU, the plugins and the catalogue snapshots in the background: a while after start, daily, and when a disk comes or goes.

gui-health-check-now = Check now

gui-health-checking = Checking…

gui-health-fix = Fix…

gui-health-fix-confirm = Fix it

gui-health-ignore = Ignore

gui-health-ignored-row = ignored

gui-health-not-yet = Not checked yet: the checks run in the background.

gui-health-status = Status

gui-health-summary-ok = All good

gui-health-summary-problems = Problems: { $problems }, warnings: { $warnings }

gui-health-summary-warnings = Warnings: { $count }

gui-health-undo-ignore = Show again

gui-history-batch = { $label } ({ $count } values)

gui-history-import = Import { $count } photos

gui-history-preview-applied = applied

gui-history-preview-bypassed = bypassed

gui-history-preview-param = { $label } ({ $node })

gui-history-snapshot-restored = Restored “{ $name }”

gui-history-undo = Undo

gui-image-not-reopened = The image left open could not be opened again: { $error }

gui-image-opened = { $name } is open

gui-image-opening = Opening { $name }…

gui-import-backup-copies = { $copies } copies: { $disks }

gui-import-backup-days = { $disk } (not seen for { $days } days)

gui-import-backup-never = { $disk } (never seen)

gui-import-backup-no-disk = no disk for them yet

gui-import-backup-none = No backup set up:

gui-import-backup-one = 1 copy: { $disks }

gui-import-backup-one-day = { $disk } (not seen for 1 day)

gui-import-backup-plugged = { $disk } (plugged in)

gui-import-backup-storage = Storage…

gui-import-backup-today = { $disk } (seen today)

gui-import-backup-unusable = { $disk } (plugged in, cannot take copies)

gui-import-cancelled = The import was cancelled: { $photos } photos imported

gui-import-copy = Copy the photos into the library folder

gui-import-copy-help = Off: the photos stay where they are, referenced in place

gui-import-darktable = Read darktable edits (.xmp sidecars)

gui-import-darktable-failed = { $sidecar }: not kept: { $error }

gui-import-darktable-help = Each sidecar, duplicates included, becomes a version of its photo

gui-import-darktable-report = { $sidecar }: { $report }

gui-import-drop = Drop to import into the library

gui-import-dropped-title = Import { $count } items

gui-import-ended = { $photos } photos imported, { $edits } darktable edits

gui-import-ended-reported = { $photos } photos imported, { $edits } darktable edits; { $reported } not kept whole: see the Log panel

gui-import-failed = The import failed: { $error }

gui-import-form-title = Import photos

gui-import-metadata = { $photos } photos took their ratings, labels or tags from the importer

gui-import-no-library = No library is open: start Helios with one (--vault FILE) to import photos

gui-import-no-library-error = No library is open: { $error }

gui-import-path = card or folder

gui-import-remember = Remember for this workspace

gui-import-remember-help = The workspace's import source: the next import does not ask

gui-import-verb = Import

gui-import-window-accounts = Accounts

gui-import-window-add = Add where they are

gui-import-window-add-favourite = Add to favourites

gui-import-window-add-source = Add a source…

gui-import-window-all-types = All types

gui-import-window-cancel = Cancel

gui-import-window-card-inserted = { $card } inserted

gui-import-window-card-note = A card's photos are copied: the card is formatted later, the copies stay.

gui-import-window-cards = Cards and cameras

gui-import-window-change-into = Choose where copies go

gui-import-window-choices = Choices

gui-import-window-choose = Choose a folder…

gui-import-window-contents = Contents

gui-import-window-copy = Copy into the Library

gui-import-window-counts = { $photos } photos, { $new } new, { $size }

gui-import-window-darktable = Read darktable edits

gui-import-window-darktable-help = Each .xmp sidecar, duplicates included, becomes a version of its photo

gui-import-window-delete-preset = Delete preset

gui-import-window-duplicates = Duplicates

gui-import-window-duplicates-ask = Ask

gui-import-window-duplicates-keep = Keep

gui-import-window-duplicates-skip = Skip

gui-import-window-empty = No photos in the sources ticked

gui-import-window-example = e.g. { $example }

gui-import-window-favourites = Favourites

gui-import-window-forget = Forget this folder

gui-import-window-forget-hint = Drops what the image map knows of this folder's files not in the Library; it is not recorded again until you list it

gui-import-window-import = Import { $count }

gui-import-window-in-library = in your Library

gui-import-window-into = Into

gui-import-window-jpeg-only = JPEG only

gui-import-window-layout-day = By day

gui-import-window-layout-flat = One folder

gui-import-window-layout-month = By month

gui-import-window-layout-year = By year

gui-import-window-library-folder = The Library folder

gui-import-window-menu-file = File

gui-import-window-menu-window = Window

gui-import-window-more = More

gui-import-window-more-note = Metadata templates and time offsets come later.

gui-import-window-name = Name

gui-import-window-name-custom = Custom

gui-import-window-name-date-camera = Date, time and camera

gui-import-window-name-date-seq = Date and number

gui-import-window-name-date-stem = Date and name

gui-import-window-name-original = Original name

gui-import-window-new-only = New only

gui-import-window-no-cards = No card inserted

gui-import-window-no-favourites = None yet

gui-import-window-no-sample = Tick a photo to see an example

gui-import-window-none-ticked = Tick the photos to import

gui-import-window-nothing = Tick a card or a folder to import

gui-import-window-offline = offline

gui-import-window-open = Import photos…

gui-import-window-pair = RAW+JPG

gui-import-window-preset = Preset

gui-import-window-preset-default = Default

gui-import-window-preset-last-used = Last used

gui-import-window-preset-name = Preset name

gui-import-window-preset-save = Save

gui-import-window-raw-only = Raw only

gui-import-window-reading = Reading { $path }

gui-import-window-recent = Recent

gui-import-window-remove-favourite = Remove from favourites

gui-import-window-save-preset = Save as preset…

gui-import-window-select = Select:

gui-import-window-select-all = all

gui-import-window-select-day = day

gui-import-window-select-none = none

gui-import-window-sources = Sources

gui-import-window-title = Import photos

gui-import-window-totals = { $photos } photos, { $size }

gui-import-window-types = Types

gui-importer-file = file

gui-importer-folder = folder

gui-importer-form-title = Import from { $importer }

gui-importer-refused = { $importer }: { $path } left out: { $why }

gui-importer-scanned = { $importer }: { $found } photos found in { $source }, { $refused } left out

gui-importer-warning = { $importer }: { $warning }

gui-job-batch-done = Batch finished

gui-job-batch-failed = Batch failed

gui-job-export-done = Export finished

gui-job-export-failed = Export failed

gui-keys-from-template = template

gui-layout-not-saved = The layout was not saved: { $error }

gui-library-auto-advance = Auto-advance

gui-library-badge-backed-up = backed up

gui-library-badge-backup-short = backup short

gui-library-badge-backup-unverified = unverified

gui-library-badge-changed-outside = changed outside

gui-library-badge-missing = missing

gui-library-badge-no-backup = no backup

gui-library-badge-offline = offline

gui-library-cannot-open = The library cannot be opened: { $error }

gui-library-compare = Compare

gui-library-compare-hint = Select two to four images to compare them

gui-library-compare-tip = The selected photos side by side (C)

gui-library-count = { $shown } of { $total }

gui-library-empty = No photos yet: import a card or a folder, or drop photos on the window

gui-library-facet-backup = Backup

gui-library-facet-camera = Camera

gui-library-facet-flag = Flag

gui-library-facet-label = Label

gui-library-facet-lens = Lens

gui-library-facet-presence = Presence

gui-library-facet-rating = Rating

gui-library-facet-type = Type

gui-library-file = { $size } · modified { $day }

gui-library-folder-import = Import new photos here

gui-library-folder-scan = Scan this folder

gui-library-folder-show = Show in file manager

gui-library-folders = Folders

gui-library-folders-elsewhere = Other folders

gui-library-images = Images

gui-library-import = Import…

gui-library-import-photos = Import photos…

gui-library-keep-best = Keep the best: pick it, reject the others

gui-library-missing-note = Missing: its file is not where it was, and no other copy is known

gui-library-no-cursor = No image under the cursor

gui-library-none = No images

gui-library-not-saved = Not saved, the library shows what was saved last: { $error }

gui-library-offline-on = On { $volume }, last seen { $date } on { $node }

gui-library-offline-unknown = On a disk that is not plugged in

gui-library-plug-in = Plug in { $volume } to open this photo (last seen { $date } on { $node })

gui-library-read = { $count } images in the library

gui-library-reading = Reading the library…

gui-library-search = Search images

gui-library-sort-ascending = Ascending: oldest, smallest or A first

gui-library-sort-captured = Capture time

gui-library-sort-descending = Descending: newest, largest or Z first

gui-library-sort-edited = Last edit

gui-library-sort-id = Content

gui-library-sort-imported = Import time

gui-library-sort-name = File name

gui-library-sort-rating = Rating

gui-library-sort-size = File size

gui-library-sort-tip = Sort by

gui-library-sync = Sync zoom and pan

gui-library-tags = Tags

gui-library-volume-offline = { $volume } { $count }, offline, last seen { $date }

gui-log-clear = Clear

gui-log-empty = Nothing to report yet

gui-macro-fold-detail = Changed: { $changes }

gui-macro-fold-discard = Discard

gui-macro-fold-discard-hint = Back to the macro's inside as it was

gui-macro-fold-expose = Expose them

gui-macro-fold-expose-hint = The parameters changed become the macro's own, their values kept on this image

gui-macro-fold-keep = Keep it opened

gui-macro-fold-keep-hint = Fold it later

gui-macro-fold-title = { $node } changed while open

gui-macro-fold-version = Save as version { $version }

gui-macro-fold-version-hint = The changes become the macro's, in { $file }; its other nodes keep their version

gui-macro-iface-default = Its own default

gui-macro-iface-default-value = Default

gui-macro-iface-description = Description

gui-macro-iface-down = Move down

gui-macro-iface-drive-add = Drive another parameter

gui-macro-iface-drive-remove = Stop driving it

gui-macro-iface-filter = Filter

gui-macro-iface-group = Group

gui-macro-iface-inner = Inner parameters

gui-macro-iface-label = Label

gui-macro-iface-linked = Linked to the first row named so: it is shown as that one is

gui-macro-iface-name = Name

gui-macro-iface-name-hint = The same name on two rows links them: one value sets both

gui-macro-iface-new-slider = New semantic slider

gui-macro-iface-new-slider-hint = One slider that moves several inner numbers, each along its curve

gui-macro-iface-none = Nothing shown: tick inner parameters on the left

gui-macro-iface-options = Options kept

gui-macro-iface-pick = Select a parameter shown to set its name, range and default

gui-macro-iface-point = x: the slider's value; y: the inner value there

gui-macro-iface-point-add = Add a point

gui-macro-iface-range = Range

gui-macro-iface-remove = Stop showing it

gui-macro-iface-shown = Shown by the macro

gui-macro-iface-soft = A soft range

gui-macro-iface-soft-range = Soft range

gui-macro-iface-title = Interface of { $node }

gui-macro-iface-unchanged = Nothing changed yet

gui-macro-iface-unit = Unit

gui-macro-iface-up = Move up

gui-macro-iface-verb = Save as { $file }

gui-macro-make-title = Make a macro

gui-macro-make-verb = Make

gui-macro-name = name

gui-macro-publish-detail = Writes { $composite } into { $library }, where every workspace adds it from the graph's Add menu. Publishing is not undone: Ctrl+Z leaves the file; remove it from that folder to take it back. A later change is published as a new version.

gui-macro-publish-title = Publish { $name } to your library?

gui-macro-publish-verb = Publish

gui-macro-published = { $composite } published to { $file }: undo does not take it back

gui-mask-display = Show mask

gui-mask-display-grey = the mask alone in grey

gui-mask-display-hint = Show { $mask } on the image, now { $mode }. This button, M with the image focused and the palette cycle a coloured overlay, the mask alone in grey, and off.

gui-mask-display-off = off

gui-mask-display-overlay = a coloured overlay

gui-masks-add = Add…

gui-masks-add-hint = Add a shape or a parametric member to the selected group. With the image focused: C a circle, E an ellipse, G a gradient, P a path.

gui-masks-beside-refinement = nothing goes beside a refinement: drop on a member or into a group

gui-masks-empty = The mask is empty: { $module } applies everywhere. Add a shape or a parametric member to limit it.

gui-masks-gesture-ctrl = Ctrl: fine moves

gui-masks-gesture-reset = double-click: reset

gui-masks-gesture-shift-angle = Shift: snap the angle to 15° steps

gui-masks-gesture-shift-axes = Shift: along the axes and diagonals

gui-masks-gesture-wheel = wheel: adjust

gui-masks-group-full = the group `{ $group }` holds { $count } members already

gui-masks-group-locked = the group `{ $group }` is locked: its members stay

gui-masks-hide = Hide { $name }: its opacity off

gui-masks-into-itself = a group cannot go into itself

gui-masks-invert = Invert

gui-masks-invert-hint = Invert the selected member: it then covers everything except what it covered (for the module's mask, the whole mask)

gui-masks-lock = Lock { $name } against moves and edits

gui-masks-locked = `{ $name }` is locked

gui-masks-menu-add-circle = Circle

gui-masks-menu-add-ellipse = Ellipse

gui-masks-menu-add-gradient = Gradient

gui-masks-menu-add-parametric = Parametric

gui-masks-menu-add-path = Path

gui-masks-menu-delete = Take out of the mask

gui-masks-menu-invert = Invert

gui-masks-menu-lock = Lock or unlock

gui-masks-menu-opacity = Opacity off or full

gui-masks-menu-rename = Rename…

gui-masks-none = No mask: the selected module is not masked

gui-masks-not-a-group = `{ $name }` is not a group: only a group takes members

gui-masks-nothing-beside-root = nothing goes beside the module's mask: drop inside it

gui-masks-of = Mask of { $module }

gui-masks-one-at-a-time = members move one at a time

gui-masks-op-difference = subtract from the first

gui-masks-op-exclusion = exclude

gui-masks-op-maximum = union

gui-masks-op-minimum = intersection

gui-masks-op-product = multiply

gui-masks-op-screen = soft union

gui-masks-op-sum = add

gui-masks-opacity = Opacity

gui-masks-opacity-hint = Turn the selected member's opacity off, or back to full (its row's eye does the same)

gui-masks-operator-hint = { $effect }: how the members fold, in port order (the operator { $operator })

gui-masks-refine-blur = blur

gui-masks-refine-feather = feather

gui-masks-refine-levels = levels

gui-masks-refinement-stays = `{ $name }` stays with what it refines

gui-masks-root-stays = the module's mask stays the root

gui-masks-row-hidden = hidden

gui-masks-row-inverted = inverted

gui-masks-row-not-inverted = not inverted

gui-masks-row-opacity = opacity { $percent } %

gui-masks-show = Show { $name }: its opacity full

gui-masks-take-out = Take out

gui-masks-take-out-group = Take the group { $name } out of the mask?

gui-masks-take-out-group-detail = Its members go with it, and taking out is not an undo step yet.

gui-masks-take-out-hint = Take the selected member out of the mask (Delete, the tree focused). Not an undo step yet.

gui-masks-take-out-verb = Take out

gui-masks-tree-hint = F2 renames the selected row. Drag a row to reorder it or into a group. Right-click a row for its menu: it never deletes anything.

gui-masks-unlock = Unlock { $name }

gui-modules-bypass = Bypass

gui-modules-bypass-hint = Bypass { $node }: the image renders as if it were absent, its parameters kept ({ $key })

gui-modules-fold-hint = Fold { $node } back into one node

gui-modules-macro = macro { $version }

gui-modules-macro-interface = Interface…

gui-modules-macro-interface-hint = Choose the parameters { $node } shows, saved as a new version

gui-modules-macro-publish = Publish…

gui-modules-macro-publish-hint = Copy this version of { $node } into your library, for every workspace's Add menu; asked first, and not undone

gui-modules-macro-update = Update to { $version }

gui-modules-macro-update-hint = Move { $node } to version { $version } of its macro, its values carried by name

gui-modules-make-macro = Make macro…

gui-modules-make-macro-hint = Package the modules selected as one node (Ctrl+G in the graph)

gui-modules-none = No module: open an image to see its network's modules

gui-modules-none-in-scope = No module of the sub-workspace { $name } in the image's network

gui-modules-open-hint = Open { $node }: its modules shown

gui-modules-restore-hint = Bring { $node } back: the image renders it again ({ $key })

gui-modules-save-workshop = Save as workshop…

gui-modules-save-workshop-hint = Give { $node } an intent and targets aimed per image, saved as a new version (experimental)

gui-new-workspace-created = workspace { $path } created from { $template }

gui-new-workspace-directory = directory

gui-new-workspace-form-title = New workspace: { $template }

gui-new-workspace-template = template

gui-new-workspace-title = New workspace from a template

gui-new-workspace-unknown = no template { $id }

gui-new-workspace-verb = Create

gui-no-dialogs = no dialogs to show this in

gui-open-workspace-directory = directory

gui-open-workspace-title = Open a workspace

gui-open-workspace-verb = Open

gui-opening = opening { $path }

gui-param-default = { $path } · default { $value }

gui-param-held = { $label }: held to { $value }

gui-param-hint = Drag to change ({ $fine } finer, { $coarse } coarser), arrows to step, { $wheel }; double click resets; Enter types a value or an expression (x is the value)

gui-param-hint-wheel-focused = the wheel once focused

gui-param-hint-wheel-focused-or = the wheel once focused or with { $keys }

gui-param-hint-wheel-over = the wheel over it

gui-param-past-range = { $label }: past its usual range

gui-param-picker = Pick { $label } from the image

gui-params-fold-section = Fold { $group }

gui-params-hide-values = Hide the values

gui-params-no-members = no members yet

gui-params-points = { $count } points

gui-params-port-hint = Port { $port } of the group; the Masks panel edits it on { $member }'s row

gui-params-port-member = { $member }: { $value }

gui-params-show-values = Show the values

gui-params-unfold-section = Unfold { $group }

gui-params-values = { $count } values

gui-pick-browser = Pick your photos in your browser, press Done, then come back here.

gui-pick-done = { $count } photos imported.

gui-pick-failed = Not imported: { $why }

gui-pick-importing = Importing { $count } photos…

gui-pick-nothing = Nothing was picked in time.

gui-pick-sign-in = The source refused the sign-in: reconnect the account.

gui-pick-starting = Opening the picking…

gui-pick-title = Pick photos in { $source }

gui-pointer-choice-close = Close the pointer profile notice

gui-pointer-choice-darktable = Use darktable's

gui-pointer-choice-keep = Keep Photoshop-style

gui-pointer-choice-text = Use darktable's mouse and wheel habits: the wheel changes the slider under the pointer and zooms the image. You can change this later in Settings › Input.

gui-pointer-choice-title = Coming from darktable?

gui-policy-ask = confirmations are asked again

gui-policy-yes = confirmations are answered yes by policy

gui-profile-applied = profile { $name } applied

gui-profile-exported = profile exported to { $path }

gui-profile-import-changes = profile { $name }: { $count } changes; profile.import.apply to apply them

gui-profile-keys-changed = profile { $name }: { $count } key bindings changed

gui-profile-keys-conflict = profile { $name }: keys in conflict: { $keys }

gui-profile-keys-error = profile { $name }: keys: { $error }

gui-profile-keys-skipped = profile { $name }: keys skipped: { $keys }

gui-profile-layout-error = profile { $name }: layout: { $error }

gui-profile-variants = profile { $name }: { $count } values from variants: { $variants }

gui-queue-cancel = Cancel

gui-queue-column-attempts = Attempts

gui-queue-column-item = Item

gui-queue-column-since = Since

gui-queue-column-state = State

gui-queue-column-where = Where

gui-queue-empty = Nothing queued at { $node }

gui-queue-here = This desktop

gui-queue-hours = { $n } h

gui-queue-minutes = { $n } min

gui-queue-no-selection = Select a node to see its queue

gui-queue-oldest-wait = Oldest wait: { $time }

gui-queue-on-node = Node { $name }

gui-queue-seconds = { $n } s

gui-queue-state-cancelled = Cancelled

gui-queue-state-cancelling = Cancelling

gui-queue-state-done = Done

gui-queue-state-failed = Failed

gui-queue-state-paused = Paused

gui-queue-state-running = Running

gui-queue-state-waiting = Waiting

gui-queue-summary = { $waiting } waiting, { $running } running, { $done } done, { $failed } failed

gui-queue-title = Queue of { $node }

gui-quick-open-image = image

gui-quick-open-module = module { $module }

gui-quick-open-source = source

gui-quick-open-subworkspace = sub-workspace

gui-quick-open-title = Go to a module, a sub-workspace, an image or a workspace

gui-quick-open-workspace = workspace { $path }

gui-quit-jobs-detail = { $count } jobs are running; quitting cancels them

gui-quit-title = Quit Helios?

gui-quit-verb = Quit

gui-recording-saved = recording saved to { $path }

gui-recording-stopped = recording stopped: { $error }

gui-scene-saved = viewport.scene saved to { $path }

gui-scopes-blue = Blue

gui-scopes-green = Green

gui-scopes-red = Red

gui-settings-account-name = Account

gui-settings-account-source = Source

gui-settings-account-state = State

gui-settings-account-why = Why

gui-settings-accounts = Accounts

gui-settings-accounts-about = The accounts Helios imports from. Their keys and tokens stay in this computer's keyring; the library keeps only their names.

gui-settings-accounts-list = Import accounts

gui-settings-accounts-none = No account yet: add one with "Add a source…" in the import window.

gui-settings-backup = Backup

gui-settings-caches = Caches

gui-settings-catalogue = Catalogue

gui-settings-catalogue-about = Verified snapshots of the catalogue (ratings, tags, edits): kept next to it and copied to the backup disks plugged in.

gui-settings-decisions = Decisions

gui-settings-destinations = Destinations

gui-settings-display = Display

gui-settings-experimental = Experimental

gui-settings-hardware = Hardware

gui-settings-health = Health

gui-settings-health-check = Health check

gui-settings-history = History

gui-settings-image-map = Image map

gui-settings-library = Library

gui-settings-snapshots = Snapshots

gui-settings-storage = Storage

gui-settings-strategy = Strategy

gui-settings-strategy-copies = Copies

gui-settings-strategy-destinations = On

gui-settings-strategy-online = Disks plugged in

gui-settings-volume-backup = Backup

gui-settings-volume-forget = Forget…

gui-settings-volume-forget-title = Forget what was seen on { $disk }?

gui-settings-volume-forget-verb = Forget

gui-settings-volume-kind = Kind

gui-settings-volume-name = Name

gui-settings-volume-photos = Photos

gui-settings-volume-scan = Scan now

gui-settings-volume-seen = Seen

gui-settings-volume-space = Space

gui-settings-volume-state = State

gui-settings-volumes = Volumes

gui-settings-volumes-none = No volume known yet: plug in a disk.

gui-settings-volumes-seen = Volumes

gui-settings-workbench = Workbench

gui-settings-workers = Workers

gui-size-bytes = { $size } B

gui-size-gib = { $size } GiB

gui-size-kib = { $size } KiB

gui-size-mib = { $size } MiB

gui-snapshot-failed = snapshot failed: { $error }

gui-snapshot-saved = { $target } pictured to { $path }

gui-snapshot-unreadable = Snapshot “{ $name }” cannot be read: { $error }

gui-snapshots-not-saved = The snapshots were not saved: { $error }

gui-stacks-accept-all = Accept all

gui-stacks-accept-checked = Accept the checked

gui-stacks-bracket = Bracket of { $count }

gui-stacks-burst = Burst of { $count }

gui-stacks-decline = Decline

gui-stacks-left-out = { $count } images left out

gui-stacks-none = No stack suggestion waiting

gui-stacks-proposal = { $groups } stacks proposed by { $resolver }

gui-stacks-show = Show

gui-stacks-suggest = Suggest stacks

gui-stacks-suggest-hint = Over the images selected (two or more), else over every image

gui-stacks-top = Proposed on top: the sharpest

gui-status-culled = { $picked } picked, { $rejected } rejected

gui-status-decisions = { $count } decisions wait for an answer

gui-status-decisions-show = Show the decisions

gui-status-health-problem = Health check: { $count } findings need attention, a problem among them

gui-status-health-show = Open the health check

gui-status-health-warning = Health check: { $count } findings need attention

gui-status-history = History

gui-status-history-detail = { $count } steps to undo

gui-status-history-redo = Redo

gui-status-history-undo = Undo

gui-status-images = { $count } images

gui-status-zoom = Zoom

gui-status-zoom-fill = Fill

gui-status-zoom-fit = Fit

gui-storage-backup-destination = takes copies

gui-storage-backup-now = Back up now

gui-storage-backup-off-site = One of them off site

gui-storage-backup-on = Keep verified backup copies of every photo

gui-storage-backup-on-connect = When a backup disk is plugged in

gui-storage-backup-on-import = After each import

gui-storage-dest-archive = Any archive disk

gui-storage-dest-backup = Any backup disk

gui-storage-forget-hint = Drops what the image map knows of this volume's files not in the Library; the volume stays known

gui-storage-free = { $free } free of { $capacity }

gui-storage-intro = The disks Helios has seen. Name them and say what each is for: Helios writes only to library, archive, backup and scratch disks.

gui-storage-kind-external = External disk

gui-storage-kind-internal = Internal disk

gui-storage-kind-remote = Remote store

gui-storage-kind-removable = Card or stick

gui-storage-kind-share = Network share

gui-storage-last-seen = Last seen { $date } on { $node }

gui-storage-mark = Mark this disk…

gui-storage-mark-confirm = Write the identity file

gui-storage-mark-explain = This disk is known only by its label and size, so Helios could take another disk for it. Marking it writes a small file, .helios/volume.json, at its root: nothing else on the disk changes.

gui-storage-mark-title = Mark { $disk }?

gui-storage-name-hint = Its name; Enter saves it

gui-storage-node = In { $node }

gui-storage-off-site = Off site

gui-storage-off-site-hint = Kept away from the other disks (another building)

gui-storage-offline = offline

gui-storage-online = online

gui-storage-raid = RAID

gui-storage-raid-note = A RAID counts as one copy: it survives a dead disk, not a deletion or a theft.

gui-storage-role-archive = Archive

gui-storage-role-backup = Backup

gui-storage-role-ingest = Ingest

gui-storage-role-library = Library

gui-storage-role-scratch = Scratch

gui-storage-strategy-none = No backup set up

gui-storage-summary = { $online } online, { $offline } offline

gui-subworkspace-gone = The sub-workspace { $id } is gone: back to where it was entered from

gui-template-keys-error = template { $name }: keys: { $error }

gui-theme-exported = theme exported to { $path }

gui-theme-problem = theme { $name }: { $problem }

gui-theme-unknown = theme { $name }: no theme `{ $name }`: { $default } is used instead

gui-vault-unreadable = Cannot read the library: { $error }

gui-window-frame-error = window frame: { $error }

gui-workshops-aim = Aim again

gui-workshops-aim-hint = Solve its targets again on the open image (one undo step)

gui-workshops-aimed = aimed: { $measure } at { $value } { $unit }

gui-workshops-applied = Applied: { $node }

gui-workshops-applied-hint = Select its node, to tune it below

gui-workshops-apply = Apply

gui-workshops-apply-hint = Insert { $name } after the node the image is shown from, aimed on the open image (one undo step)

gui-workshops-describe = Describe what you want

gui-workshops-describe-hint = backlit, the mountain and the sky too contrasted…

gui-workshops-design = Design a workshop

gui-workshops-design-cancel = Cancel

gui-workshops-design-hint = Ask your own agent (over MCP) for a workshop made for these words; Helios checks it on the open image and applies it as one undo step

gui-workshops-design-waiting = Waiting for your agent: it reads helios_workshop_context and posts a draft with helios_workshop_design

gui-workshops-designed = Designed: { $workshop }

gui-workshops-experimental = Experimental: may change; what it saves stays a macro

gui-workshops-find = Find

gui-workshops-find-hint = what you want: brighter, less contrast…

gui-workshops-ghost = { $param }: { $value } proposed by { $author }

gui-workshops-ghost-accept = Accept

gui-workshops-ghost-accept-hint = Set this value (one undo step); move the slider instead to override it

gui-workshops-ghost-reject = Reject

gui-workshops-library = Your library

gui-workshops-measure-clip = clipping

gui-workshops-measure-contrast = contrast

gui-workshops-measure-high = highlights

gui-workshops-measure-median = midtones

gui-workshops-measure-ratio = gap between zones

gui-workshops-no-image = Open an image to tune it

gui-workshops-none = No workshops in this workspace yet

gui-workshops-none-found = No workshop found for these words

gui-workshops-open = Open in graph

gui-workshops-save-intent = what it is for

gui-workshops-save-keywords = keywords, separated by commas

gui-workshops-save-target = { $param }: target

gui-workshops-save-target-now = { $param }: target (the image now: { $now })

gui-workshops-save-title = Save { $node } as a workshop

gui-workshops-save-value = { $param }: value (stops, or % for clip.high)

gui-workshops-save-verb = Save as { $file }

gui-workshops-send-proxy = Let the agent see a small preview of the photo (this session only)

gui-workshops-session = Session recorded: { $steps } steps

gui-workshops-session-keep = Keep

gui-workshops-session-keep-hint = Keep this session's record: who set what, why, and what the image measured. Not kept, it is dropped when the session ends

gui-workshops-session-kept = Kept

gui-workshops-shipped = Shipped with Helios

gui-workshops-unit-percent = %

gui-workshops-unit-stops = stops

gui-workshops-workspace = This workspace

gui-workspace-description = description

gui-workspace-edit-title = Edit the workspace

gui-workspace-edit-verb = Save

gui-workspace-icon = icon

gui-workspace-import-next = Review

gui-workspace-import-none = no part chosen

gui-workspace-import-parts = Parts to add from { $source }

gui-workspace-import-review = Add from { $source }: { $changes } changes, { $conflicts } to decide

gui-workspace-import-title = Add from a workspace or a template

gui-workspace-import-verb = Add

gui-workspace-library-advance = culling advances after each mark

gui-workspace-library-compare = opens in Compare, photos side by side

gui-workspace-library-darktable = read darktable edits (.xmp sidecars) on import

gui-workspace-library-duplicates = exact duplicates

gui-workspace-library-placement = placement

gui-workspace-library-query = opens on (as searched: flag:pick rating:3)

gui-workspace-library-rename = rename copies as

gui-workspace-library-source = import source

gui-workspace-library-stacks = suggest stacks after an import

gui-workspace-library-title = The workspace's library

gui-workspace-name = name

gui-workspace-not-saved = The workspace was not saved: { $error }

gui-workspace-reload-failed = The workspace's files changed outside Helios and could not be read: { $error }

gui-workspace-reloaded = The workspace's files changed outside Helios: { $parts } reloaded

gui-workspace-reloaded-both = The workspace's files changed outside Helios while you changed it too: { $both } taken from the files (Edit > Undo gives yours back)

gui-workspace-template-ask = ask on create: { $name }

gui-workspace-template-description-fr = description in French

gui-workspace-template-id = template id

gui-workspace-template-keys = with its keys

gui-workspace-template-library = with its library settings

gui-workspace-template-name-fr = name in French

gui-workspace-template-overwrite = replace a template of that id

gui-workspace-template-saved = template { $name } saved in { $path }

gui-workspace-template-stripped = Never kept: { $what }

gui-workspace-template-title = Save as template

gui-workspace-template-toolbox = with its tools

gui-workspace-template-version = version

health-card-short = { $card } is not safe to format yet

health-card-short-detail = Photos on it short of their backups: { $count }. Keep the card.

health-cards = Cards safe to format

health-changed-outside = Photos changed outside Helios

health-changed-outside-detail = Another application rewrote their files. They stay in your Library with their edits; their thumbnails were made again.

health-changed-outside-some = { $count } photos changed outside Helios

health-check-crashed = The check { $check } failed

health-check-crashed-detail = It stopped before it could say anything.

health-fix-acknowledge-changed = Acknowledge

health-fix-back-up-now = Back up now

health-fix-gpu-docs = How to choose the GPU

health-fix-library-folder = Open the library's folder

health-fix-no-destination = No backup disk is plugged in: open the Storage page

health-fix-plug-in = Plug in { $volume } to back up

health-fix-plugin-disable = Disable it

health-fix-plugin-folder = Open its folder

health-fix-plugin-update = Update it: open its folder

health-fix-set-strategy = Set up a backup strategy

health-fix-show-volume = Show it on the Storage page

health-fix-snapshots-folder = Open the snapshots' folder

health-fix-verify = Back up now, which verifies them

health-free-space = Free space

health-free-space-detail = { $free } free of { $capacity } ({ $percent } %)

health-free-space-full = { $volume } is almost full

health-free-space-low = { $volume } is running out of space

health-free-space-ok = { $volume } has room

health-gpu-adapter = The GPU

health-gpu-cpu = Helios renders on the CPU

health-gpu-cpu-detail = { $hardware } is present but was not opened: { $why }

health-gpu-kernel-cpu = { $module } falls back to the CPU

health-gpu-kernel-cpu-detail = Its GPU kernel does not compile on { $adapter }: { $error }

health-gpu-kernels = GPU kernels

health-gpu-kernels-ok = Every GPU kernel compiles ({ $count })

health-gpu-kernels-ok-detail = On { $adapter }

health-gpu-none-detail = No GPU was found.

health-gpu-ok = Helios renders on { $adapter }

health-gpu-ok-detail = { $used }

health-gpu-software = Helios renders on a software adapter, { $adapter }

health-gpu-software-detail = { $used } runs on the CPU while { $hardware } is present: choose the GPU in the settings.

health-library = The library

health-library-closed = The library cannot be opened

health-library-closed-detail = { $file }: { $error }

health-library-no-room = The library's disk is almost full

health-library-no-room-detail = { $free } free of { $capacity } on { $volume }: the library needs room to grow.

health-library-ok = The library opens and has room

health-library-ok-detail = { $file }

health-met = Photos backed up

health-met-ok = Every photo has its backups

health-met-ok-detail = Photos checked: { $count }

health-met-short = Photos short of their backups: { $count }

health-met-short-detail = Verified copies { $name } asks for per photo: { $copies }

health-plugin-crashed = { $plugin } does not start

health-plugin-crashed-detail = { $why }. Log: { $log }

health-plugin-disabled = { $plugin } is disabled

health-plugin-disabled-detail = Log: { $log }

health-plugin-not-granted = { $plugin } waits for a permission

health-plugin-not-granted-detail = It needs { $capability } before it can start.

health-plugin-ok = { $plugin } answers

health-plugin-ok-detail = Version { $version }, in { $dir }

health-plugin-unsupported = { $plugin } does not suit this Helios

health-plugin-unsupported-detail = { $problems }. Update it.

health-plugins = Plugins

health-seen = Backup disks seen lately

health-seen-ok = Every backup disk was seen lately

health-seen-ok-detail = Within the last { $limit } days

health-seen-stale = { $volume } was last seen { $days } days ago

health-seen-stale-detail = Its backups are not kept up to date while it stays away (more than { $limit } days).

health-snapshots = Catalogue snapshots

health-snapshots-broken = The last catalogue snapshot cannot be restored

health-snapshots-file-detail = { $file }, { $days } days old

health-snapshots-none-yet = No catalogue snapshots yet

health-snapshots-none-yet-detail = Helios does not take catalogue snapshots yet; the backups keep the photos.

health-snapshots-ok = The last catalogue snapshot is recent and restorable

health-snapshots-old = The last catalogue snapshot is { $days } days old

health-snapshots-unverified = No catalogue snapshot is verified

health-snapshots-unverified-detail = { $count } snapshots, none read back yet.

health-strategy = Backup strategy

health-strategy-none = No backup is set up

health-strategy-none-detail = Each photo has one copy, in the Library: a dead disk loses it.

health-strategy-set = Backups follow { $name }

health-strategy-set-detail = Verified copies per photo: { $copies }

health-unverified = Verified copies

health-unverified-detail = A copy counts once it is read back and matches its hash.

health-unverified-some = Photos with backup copies not verified: { $count }

health-vault-damaged = The catalogue is damaged

health-vault-damaged-detail = { $file }: { $first } ({ $count } complaints). Restore a catalogue snapshot.

health-vault-integrity = Catalogue integrity

health-vault-ok = The catalogue is sound

health-vault-ok-detail = { $file } passed SQLite's quick check

health-vault-unreadable = The catalogue cannot be checked

health-vault-unreadable-detail = { $file }: { $error }

node-batch-only = the batch endpoint only answers load-source, export, cancel, job.cancel, batch_start, batch_end and the workflow.* requests

node-edit-not-saved = done, but the edit was not saved: { $error }

node-job-unknown = job { $job } is unknown or already ended

node-load-refused = cannot load the image: { $error }

node-no-extent = `{ $name }` has no extent: give a region

node-no-node = no node named `{ $name }`

node-param-refused = cannot set { $node }.{ $param }: { $error }

node-path-not-absolute = { $path }: give an absolute path, the node does not know where you are

node-source-refused = `{ $name }` cannot take the image: { $error }

resolve-failed = { $node }: { $reason }

resolve-not-asked = { $node } was not asked: { $reason }

resolve-refused = { $node }: the model gave no answer ({ $reason })

workflow-newer-format = run `{ $run }` was kept by a newer Helios: it cannot be worked here

workflow-no-item = run `{ $run }` has no item `{ $item }`

workflow-no-items = a run needs at least one item

workflow-no-node = run `{ $run }` does not queue at `{ $node }`

workflow-no-run = no run `{ $run }`

workflow-other-network = run `{ $run }` was submitted to another network ({ $why }): it cannot be worked here

workflow-run-at-work = run `{ $run }` is still at work: let it finish, or cancel it, before forgetting it

workflow-run-elsewhere = run `{ $run }` is worked by another node ({ $owner })

xmp-blend-details = the mask's details threshold

xmp-blend-feathering = the mask's feathering

xmp-blend-mode = blend mode { $mode }

xmp-blend-parametric = a parametric mask

xmp-blend-raster = a raster mask

xmp-blend-space = blending in colour space { $space }

xmp-mask-empty-group = mask group { $id } is empty

xmp-mask-lost = the mask would lose { $what }

xmp-mask-missing = a mask group refers to shape { $id }, not in the history

xmp-mask-size = shape points of { $bytes } bytes, { $expected } expected

xmp-mask-too-deep = mask groups nested too deep

xmp-mask-too-many-points = a shape of { $count } points, at most { $max }

xmp-mask-unknown-type = an unknown shape type ({ $kind })

xmp-mask-unsupported = a { $form } shape, which Helios does not draw yet

xmp-mask-version = masks version { $version } not supported (reads 4 to 7)

xmp-not-applied = { $count } of { $items } history items not applied, kept for later: { $groups }

xmp-not-applied-and-substituted = { $not_applied }; { $substituted }

xmp-not-applied-group = { $labels } ({ $reason })

xmp-reason-blend = blending not supported: { $what }

xmp-reason-blending = blending not supported

xmp-reason-extra-instance = a second instance of a single-instance module

xmp-reason-mask = drawn mask not supported: { $why }

xmp-reason-mask-without-image = a drawn mask, read without its image

xmp-reason-unknown-blend-version = blend params v{ $version } not supported

xmp-reason-unknown-module = no Helios module

xmp-reason-unreadable = unreadable: { $error }

xmp-reason-unsupported-part = not supported yet: { $part }

xmp-reason-unsupported-version = version not supported (reads { $known })

xmp-substituted = { $count } applied with a substitute: { $items }

xmp-substituted-item = { $label } ({ $to } in place of { $from })
