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

calibrate-auto-no-light = image { $image } has no as-shot light to pick a profile by (camera { $camera })

calibrate-auto-no-profile = no calibration profile for camera { $camera } (image { $image })

calibrate-auto-too-far = no profile of camera { $camera } fits the light of image { $image }: the nearest, { $profile }, is { $distance } away in uv (at most 0.02)

calibrate-bad-image = the chart shot has no pixel or fewer than three channels

calibrate-bad-params = calibrate.chart: { $error }

calibrate-bad-profile = the calibration profile `{ $id }` cannot be read: { $error }

calibrate-default-cameras = the profiles of a default are all of camera { $camera }; `{ $id }` is of { $other }

calibrate-default-no-camera = clearing a camera's default needs `camera`

calibrate-export-failed = cannot write { $path }: { $error }

calibrate-image-not-at-hand = no file of image { $image } is at hand

calibrate-import-camera-key = the camera key “{ $key }” does not match the make and model “{ $make } { $model }”

calibrate-import-format = the profile's format is { $found }; this Helios reads formats { $expected }

calibrate-import-id = the profile's id is not a 64-digit hexadecimal hash

calibrate-import-illuminant = the light's chromaticity { $x }, { $y } is outside the diagram

calibrate-import-matrix = the profile's matrix is not a finite matrix that can be inverted

calibrate-import-not-profile = the file is not a Helios calibration profile

calibrate-import-nothing = calibrate.profiles.import needs `path` or `file`

calibrate-import-numbers = the profile's correction or camera matrix holds numbers that are not finite

calibrate-import-refused = the profile is refused: { $reasons }

calibrate-import-table = the profile's residual table is not one this Helios can apply

calibrate-import-unreadable = the profile cannot be read: { $error }

calibrate-import-unreadable-file = cannot read { $path }: { $error }

calibrate-load-failed = cannot read the chart shot: { $error }

calibrate-needs-vault = the node keeps no vault: saving and reading profiles need one (start it with --vault)

calibrate-no-camera-matrix = the camera has no usable colour matrix to measure the raw against

calibrate-no-neutral = the chart could not be solved at these corners: no neutral patch is kept

calibrate-no-profile = no calibration profile `{ $id }`

calibrate-no-shot = calibrate.chart needs `path` or `image`, the chart shot

calibrate-other-camera = the profile is for camera { $profile }, image { $image } is from { $camera }

calibrate-outside-image = the chart's patches fall outside the image

calibrate-photo-load-failed = cannot read the photo: { $error }

calibrate-raw-graph = the raw could not be developed for measuring: { $error }

calibrate-save-needed = a run that writes needs `save`: the profile is what it writes

calibrate-too-few-patches = the chart could not be solved at these corners: too few patches are kept

calibrate-unknown-chart = no chart `{ $chart }`

calibrate-unknown-command = no command `{ $id }`

calibrate-unknown-image = `{ $image }` is neither an image id nor a file the vault knows

calibrate-unknown-patch = the chart has no patch `{ $patch }`

calibrate-unsolved = the chart could not be measured and solved at these corners

calibrate-vault-error = the vault: { $error }

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

gui-account-add = Add account…

gui-account-another = Use another account…

gui-account-choose = Choose photos…

gui-account-connected = connected

gui-account-connecting = connecting…

gui-account-disconnect = Disconnect

gui-account-disconnect-explain = Helios asks the service to revoke its access, forgets this account's keys and tokens on this computer and removes the account. What was imported stays.

gui-account-disconnect-forget = Disconnect and forget its sources

gui-account-disconnect-forget-explain = Helios asks the service to revoke its access, forgets this account's keys and tokens on this computer, removes the account and forgets these sources: { $sources }. What was imported stays.

gui-account-disconnect-keep = Disconnect, keep its sources

gui-account-disconnect-keep-explain = Helios asks the service to revoke its access and forgets this account's keys and tokens on this computer. These sources stay, not connected, until you reconnect: { $sources }.

gui-account-disconnect-title = Disconnect { $account }?

gui-account-identity-missing = Reconnect to see the account

gui-account-manage = Manage accounts…

gui-account-menu = Account

gui-account-needs-attention = needs attention

gui-account-needs-reconnecting = needs reconnecting

gui-account-never-used = Never

gui-account-not-connected = not connected

gui-account-reconnect = Reconnect

gui-account-status-connecting = Connecting

gui-account-status-not-connected = Not connected

gui-account-status-ok = OK

gui-account-status-reconnect = Needs reconnecting

gui-account-status-refused = Access refused

gui-activity-failed = { $count } background tasks failed

gui-activity-importing = Importing { $done }/{ $total }

gui-activity-importing-start = Importing…

gui-activity-percent = { $label } { $percent } %

gui-activity-running = { $count } background tasks running

gui-activity-show = Show the background tasks

gui-album-not-changed = The album was not changed: { $error }

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

gui-boot-gpu = Opening the graphics device

gui-boot-interface = Building the interface

gui-boot-library-read = Reading the library

gui-boot-plugins = Reading the plugins

gui-caches-clear = Clear

gui-caches-clear-all = Clear caches

gui-caches-clear-all-ask = Thumbnails are read again from the files and images rendered again when shown: slower for a while. Photos, edits and the catalogue are not touched.

gui-caches-clear-all-title = Clear every cache?

gui-caches-none = No cache holds anything yet.

gui-caches-size = { $size }, { $entries } entries

gui-caches-size-of = { $size } of { $budget }, { $entries } entries

gui-caches-size-shown = { $size }: { $shown } shown, { $prefetched } read ahead

gui-caches-tier-disk = on disk

gui-caches-tier-ram = in memory

gui-caches-tier-vram = in video memory

gui-caches-total = Caches hold { $size }

gui-caches-use = In use

gui-calibrate-after = Delta E after: mean { $mean }, max { $max }

gui-calibrate-as-default = Use as the default for this camera

gui-calibrate-before = Delta E before: mean { $mean }, max { $max }

gui-calibrate-clipped = Clipped, left out: { $patches }

gui-calibrate-corner = Chart corner { $n }

gui-calibrate-corner-tip = Drag onto the centre of the corner patch

gui-calibrate-corners-undo = Put the corner back

gui-calibrate-default-set = Default set for the camera

gui-calibrate-deleted = Profile deleted

gui-calibrate-export-folder = Export to folder

gui-calibrate-exported = Exported to { $path }

gui-calibrate-import = Import the profile

gui-calibrate-import-file = Profile file to import

gui-calibrate-imported = Profile imported

gui-calibrate-is-default = The camera's default

gui-calibrate-light = Light: { $kelvin } K

gui-calibrate-name = Profile name

gui-calibrate-no-shot = Open a shot of a ColorChecker chart to make a profile

gui-calibrate-open-shot = Open

gui-calibrate-orientation = Chart seen turned by { $turns } quarter turns

gui-calibrate-patches = Patches

gui-calibrate-profiles = Profiles

gui-calibrate-profiles-none = No calibration profile yet

gui-calibrate-quality = Quality: { $quality }

gui-calibrate-report = Report

gui-calibrate-report-none = Place the four corners on the centres of the corner patches

gui-calibrate-residual-after = Delta E with the table: mean { $mean }, max { $max }

gui-calibrate-residual-dropped = Residual table: dropped

gui-calibrate-residual-held-out = Held-out Delta E: { $without } without the table, { $with } with it

gui-calibrate-residual-kept = Residual table: kept

gui-calibrate-save = Save the profile

gui-calibrate-save-heading = Make a profile

gui-calibrate-saved = Profile “{ $name }” saved

gui-calibrate-shot = Chart shot

gui-calibrate-title = Calibrate camera colour

gui-calibrate-worst = Worst patches: { $patches }

gui-calibration-default-applied = Calibration profile “{ $name }” applied: the default of { $camera }

gui-calibration-default-none = No calibration profile applied: { $reason }

gui-calibration-header-about = { $camera }, { $light }, quality { $quality }

gui-calibration-header-choose = Calibration profile

gui-calibration-header-empty = No calibration profile for this camera yet

gui-calibration-header-gone = Profile { $id } (not in the library)

gui-calibration-header-make = Make a profile…

gui-calibration-header-none = No profile

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

gui-choose-about = { $service } opens in your browser. Choose your photos, press Done there, then come back here.

gui-choose-chosen = The photos chosen show in the middle: tick the ones to import, then press Import.

gui-choose-closed = The { $service } page has closed. Open it again to choose your photos.

gui-choose-connect = Connect { $service }

gui-choose-connecting = Sign in in your browser, then come back here.

gui-choose-copy-link = Copy the link

gui-choose-identity = Reconnect once to show which account this is.

gui-choose-more-hint = Opens the page again: the photos chosen there are added to these

gui-choose-more-short = Choose more…

gui-choose-not-connected = This account is not connected.

gui-choose-not-open = The page did not open?

gui-choose-nothing = No photos were chosen.

gui-choose-open-again = Open it again

gui-choose-opening = Opening { $service }…

gui-choose-photos = Choose photos in { $service }…

gui-choose-reconnect = Reconnect { $account }

gui-choose-refused = ▲ { $service } no longer lets Helios read this account.

gui-choose-refused-why = This happens when access was removed in the account's settings at { $service }, or when the access Helios was given ran out.

gui-choose-signed-in = { $service } shows the photos of the account your browser is signed in to: { $account }.

gui-choose-try-again = Try again

gui-choose-unreachable = Could not reach { $service }: { $why }

gui-choose-waiting = Choose your photos in the browser, press Done there, then come back here.

gui-compare-first = Compare shows { $count } photos at most: the first { $count } are compared

gui-compare-rating = Active photo's rating

gui-compare-too-few = Select two or more photos to compare them

gui-confirmed-by-policy = { $verb }: confirmed by policy

gui-connect-browser = Sign in in your browser, then come back here.

gui-connect-cancel = Cancel

gui-connect-close = Close

gui-connect-copy-link = Copy link

gui-connect-done = { $account } is connected.

gui-connect-failed = Not connected: { $why }

gui-connect-link-hint = The page's address, to paste in your browser if it did not open.

gui-connect-next = Connect

gui-connect-open-again = Open again

gui-connect-secret-refused = A secret is typed by the person, never given by an agent

gui-connect-title = Connect { $source }

gui-connect-working = Connecting…

gui-consent-allow = Allow

gui-consent-asks = { $plugin } asks for your permission to:

gui-consent-declined = Nothing was changed: { $plugin } does not run without these permissions. You can allow them later in Settings > Plugins.

gui-consent-deny = Don't allow

gui-consent-from = Plugin { $id }

gui-consent-later = Helios remembers your answer; Settings > Plugins changes it.

gui-consent-title = Allow { $plugin }?

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

gui-editor-card = Card or camera

gui-editor-params = Parameters

gui-editor-params-refused = A parameter does not fit what the connector takes: { $why }

gui-editor-save = Save the source

gui-editor-title = Edit source

gui-end-changes = Saving your changes

gui-end-interface = Saving the interface settings

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

gui-health-owner-backup = Backups

gui-health-owner-catalogue = Catalogue

gui-health-owner-gpu = GPU

gui-health-owner-library = Library

gui-health-owner-other = Other checks

gui-health-owner-plugins = Plugins

gui-health-owner-storage = Storage

gui-health-status = Status

gui-health-summary-ok = All good

gui-health-summary-problems = Problems: { $problems }, warnings: { $warnings }

gui-health-summary-warnings = Warnings: { $count }

gui-health-undo-ignore = Show again

gui-history-agent-batch = Agent: { $count } commands

gui-history-album-add = Add { $count } photos to “{ $name }”

gui-history-album-delete = Delete album “{ $name }”

gui-history-album-move = Move “{ $name }” into a set

gui-history-album-move-out = Move “{ $name }” out of its set

gui-history-album-new = New album “{ $name }”

gui-history-album-new-smart = New smart album “{ $name }”

gui-history-album-remove = Remove { $count } photos from “{ $name }”

gui-history-album-rename = Rename album “{ $name }”

gui-history-album-rules = Edit the rules of “{ $name }”

gui-history-batch = { $label } ({ $count } values)

gui-history-calibration-apply = Apply the calibration profile “{ $name }”

gui-history-calibration-default = Set the calibration default of { $name }

gui-history-calibration-delete = Delete the calibration profile “{ $name }”

gui-history-calibration-import = Import the calibration profile “{ $name }”

gui-history-calibration-off = Turn the calibration profile off

gui-history-calibration-save = Save the calibration profile “{ $name }”

gui-history-camera-look-default = Change the default camera look of { $name }

gui-history-camera-look-fit = Fit the camera look to the camera's JPEG

gui-history-import = Import { $count } photos

gui-history-library-remove = Remove { $count } photos from the Library

gui-history-preview-applied = applied

gui-history-preview-bypassed = bypassed

gui-history-preview-param = { $label } ({ $node })

gui-history-quick-add = Add { $count } photos to the quick collection

gui-history-quick-clear = Clear the quick collection

gui-history-quick-remove = Remove { $count } photos from the quick collection

gui-history-quick-save = Save the quick collection as “{ $name }”

gui-history-rotate-left = Rotate { $count } photos left

gui-history-rotate-reset = Reset the rotation of { $count } photos

gui-history-rotate-right = Rotate { $count } photos right

gui-history-set-new = New album set “{ $name }”

gui-history-snapshot-restored = Restored “{ $name }”

gui-history-stack-group = Group { $count } photos into a stack

gui-history-stack-top = Move to top of stack

gui-history-stack-ungroup = Unstack

gui-history-undo = Undo

gui-history-user-batch = { $count } commands

gui-history-version-remove = Remove version

gui-history-version-switch = Switch version

gui-history-workshop-accept = Workshop “{ $name }”: a photo accepted

gui-history-workshop-decide = Workshop “{ $name }”: a junction decided

gui-history-workshop-fix = Workshop “{ $name }”: a photo fixed

gui-history-workshop-revert = Workshop “{ $name }”: a photo reverted

gui-history-workshop-run = Workshop “{ $name }” on { $count } photos

gui-image-not-reopened = The image left open could not be opened again: { $error }

gui-image-opened = { $name } is open

gui-image-opening = Opening { $name }…

gui-import-album-choose = Choose an album under the account, in Sources.

gui-import-album-failed = Could not read the album:

gui-import-album-reading = Reading the album…

gui-import-albums-failed = Could not read the albums:

gui-import-albums-more = More albums

gui-import-albums-none = No albums

gui-import-albums-reading = Reading the albums…

gui-import-apply = Apply During Import

gui-import-apply-blue = Blue

gui-import-apply-green = Green

gui-import-apply-keywords = Keywords

gui-import-apply-keywords-help = Added to every photo imported, comma-separated

gui-import-apply-label = Label

gui-import-apply-none = None

gui-import-apply-purple = Purple

gui-import-apply-rating = Rating

gui-import-apply-red = Red

gui-import-apply-yellow = Yellow

gui-import-auto = Import automatically

gui-import-auto-ask = { $files } new photos ({ $size }) on { $source }: more than an automatic import takes without asking

gui-import-auto-confirm = First automatic import from { $source }: check the photos, then Import confirms it

gui-import-auto-confirmed = Automatic import confirmed: from now on it runs on its own

gui-import-auto-failed = Automatic import from { $source }: { $error }

gui-import-auto-held = Automatic imports are paused: { $source } not imported

gui-import-auto-new-files = When new files appear

gui-import-auto-off = Off

gui-import-auto-open = Open

gui-import-auto-paused = Automatic imports paused

gui-import-auto-plugged = When plugged in

gui-import-auto-resume = Resume

gui-import-auto-resumed = Automatic imports resumed

gui-import-auto-started = Automatic import from { $source }: { $files } files

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

gui-import-browse = Browse

gui-import-browse-collections = Albums

gui-import-browse-every-folder = Every folder

gui-import-browse-folder = All photos

gui-import-browse-not-listed = Helios does not list this source's photos here yet.

gui-import-browse-picker = Choose in the service's page

gui-import-browse-timeline = By date

gui-import-browse-tree = Folder tree

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

gui-import-history = History

gui-import-history-added = { $count } added

gui-import-history-again = Import again

gui-import-history-by = By { $origin }

gui-import-history-counts = { $added } added, { $located } located, { $unchanged } unchanged, { $skipped } skipped, { $failed } failed

gui-import-history-failed = failed

gui-import-history-imports = Imports

gui-import-history-none = No import yet

gui-import-include = Include

gui-import-include-all = All photos

gui-import-include-at-least = At least

gui-import-include-at-most = At most

gui-import-include-day-help = A day as YYYY-MM-DD, by the file's date

gui-import-include-delete = Delete filter

gui-import-include-filter = Filter

gui-import-include-from = From

gui-import-include-jpeg = JPEG

gui-import-include-names = Names

gui-import-include-names-help = File name patterns, comma-separated: * any text, ? one character

gui-import-include-new-only = New photos only

gui-import-include-other = Other

gui-import-include-raw = Raw

gui-import-include-save = Save

gui-import-include-save-as = Save filter as…

gui-import-include-to = To

gui-import-include-video = Video

gui-import-metadata = { $photos } photos took their ratings, labels or tags from the importer

gui-import-no-library = No library is open: start Helios with one (--vault FILE) to import photos

gui-import-no-library-error = No library is open: { $error }

gui-import-no-library-folder = Choose a library folder first: imports copy photos into it (Helios menu › Set up Helios…).

gui-import-path = card or folder

gui-import-remember = Remember for this workspace

gui-import-remember-help = The workspace's import source: the next import does not ask

gui-import-verb = Import

gui-import-window-add = Add where they are

gui-import-window-add-favourite = Add to favourites

gui-import-window-add-source = Add a source…

gui-import-window-all-types = All types

gui-import-window-camera-look = Apply the camera's default look

gui-import-window-camera-look-help = Raws of a camera with a default camera look start with it, as their first version

gui-import-window-cancel = Cancel

gui-import-window-card-inserted = { $card } inserted

gui-import-window-card-note = A card's photos are copied: the card is formatted later, the copies stay.

gui-import-window-cards = Cards and cameras

gui-import-window-change-into = Choose where copies go

gui-import-window-choices = Choices

gui-import-window-choices-default = { $source }: the default choices

gui-import-window-choices-own = { $source }: its own choices

gui-import-window-choices-reset = Reset to defaults

gui-import-window-choices-reset-help = This source takes the default choices again

gui-import-window-choices-undo = Undo

gui-import-window-choose = Choose a folder…

gui-import-window-choose-on-the-right = Choose photos in the panel on the right: they show here.

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

gui-import-window-edit-source = Edit source…

gui-import-window-empty = No photos in the sources ticked

gui-import-window-example = e.g. { $example }

gui-import-window-favourites = Favourites

gui-import-window-folders = Folders

gui-import-window-forget = Forget this folder

gui-import-window-forget-hint = Drops what the image map knows of this folder's files not in the Library; it is not recorded again until you list it

gui-import-window-forget-source = Forget this source

gui-import-window-forget-source-hint = Takes it off this column, its name and choices with it; the photos and the library are untouched

gui-import-window-import = Import { $count }

gui-import-window-in-library = in your Library

gui-import-window-into = Into

gui-import-window-jpeg-only = JPEG only

gui-import-window-last-import = Last import { $when }

gui-import-window-layout-day = By day

gui-import-window-layout-flat = One folder

gui-import-window-layout-month = By month

gui-import-window-layout-year = By year

gui-import-window-library-folder = The Library folder

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

gui-import-window-own-choices = Its own import choices

gui-import-window-pair = RAW+JPG

gui-import-window-preset = Preset

gui-import-window-preset-default = Default

gui-import-window-preset-last-used = Last used

gui-import-window-preset-name = Preset name

gui-import-window-preset-save = Save

gui-import-window-raw-only = Raw only

gui-import-window-reading = Reading { $path }

gui-import-window-remove-favourite = Remove from favourites

gui-import-window-save-preset = Save as preset…

gui-import-window-select = Select:

gui-import-window-select-all = all

gui-import-window-select-day = day

gui-import-window-select-none = none

gui-import-window-services = Services

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

gui-init-library-folder-choose = Choose…

gui-init-library-folder-help = A folder on a disk with room for your photos; it is made if missing.

gui-init-wizard = Set up Helios

gui-init-wizard-about = A few choices before the first photos: each can be changed later.

gui-init-wizard-finish = Finish

gui-job-backup-done = Backup finished

gui-job-backup-failed = Backup failed

gui-job-batch-done = Batch finished

gui-job-batch-failed = Batch failed

gui-job-export-done = Export finished

gui-job-export-failed = Export failed

gui-job-health-done = Health check finished

gui-job-health-failed = Health check failed

gui-job-import-done = Import finished

gui-job-import-failed = Import failed

gui-job-other-done = Job finished

gui-job-other-failed = Job failed

gui-keys-from-template = template

gui-layout-not-saved = The layout was not saved: { $error }

gui-library-album-add = Add the selected photos

gui-library-album-delete = Delete the album

gui-library-album-move = Move to set…

gui-library-album-name = name

gui-library-album-new = New album…

gui-library-album-new-smart = New smart album…

gui-library-album-new-title = New album

gui-library-album-new-verb = Create

gui-library-album-remove = Remove the selected photos

gui-library-album-rename = Rename…

gui-library-album-rename-title = Rename the album

gui-library-album-rename-verb = Rename

gui-library-album-rules = Edit the rules…

gui-library-album-smart-tip = Smart album: its rules choose its photos

gui-library-albums = Albums

gui-library-albums-none = No album yet

gui-library-auto-advance = Auto-advance

gui-library-badge-backed-up = backed up

gui-library-badge-backup-short = backup short

gui-library-badge-backup-unverified = unverified

gui-library-badge-blown = blown highlights

gui-library-badge-changed-outside = changed outside

gui-library-badge-crushed = crushed shadows

gui-library-badge-missing = missing

gui-library-badge-no-backup = no backup

gui-library-badge-offline = offline

gui-library-badge-quick = quick

gui-library-badge-soft = soft focus

gui-library-badge-versions = versions

gui-library-cannot-open = The library cannot be opened: { $error }

gui-library-compare = Compare

gui-library-compare-hint = Select two to four images to compare them

gui-library-compare-tip = The selected photos side by side (C)

gui-library-count = { $shown } of { $total }

gui-library-cull-clipped = clipped

gui-library-cull-sharp = sharp

gui-library-cull-soft = soft

gui-library-cull-unmeasured = not measured

gui-library-delete-rejected = Delete Rejected…

gui-library-drop-add = Add { $count } photos

gui-library-drop-set = Not here: drop the photos on one of the set's albums

gui-library-drop-smart = Not here: a smart album's rules choose its photos

gui-library-duplicate = Duplicate

gui-library-duplicate-original = Duplicate Original

gui-library-empty = No photos yet: import a card or a folder, or drop photos on the window

gui-library-facet-backup = Backup

gui-library-facet-camera = Camera

gui-library-facet-cull = Culling

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

gui-library-not-duplicated = The photos were not duplicated: { $error }

gui-library-not-removed = The photos were not removed from the Library: { $error }

gui-library-not-rotated = The photos were not rotated: { $error }

gui-library-not-saved = Not saved, the library shows what was saved last: { $error }

gui-library-offline-on = On { $volume }, last seen { $date } on { $node }

gui-library-offline-unknown = On a disk that is not plugged in

gui-library-plug-in = Plug in { $volume } to open this photo (last seen { $date } on { $node })

gui-library-quick = Quick collection

gui-library-quick-clear = Clear the quick collection

gui-library-quick-save = Save as album…

gui-library-quick-save-clear = then clear the quick collection

gui-library-quick-save-title = Save the quick collection as an album

gui-library-quick-save-verb = Save

gui-library-quick-tip = B adds the selected photos or takes them out

gui-library-quick-toggle = Add or remove the selected photos

gui-library-read = { $count } images in the library

gui-library-reading = Reading the library…

gui-library-remove = Remove from Library

gui-library-remove-version = Remove Version…

gui-library-rotate-left = Rotate Left

gui-library-rotate-reset = Reset Rotation

gui-library-rotate-right = Rotate Right

gui-library-search = Search images

gui-library-set-delete = Delete the set, keep its albums

gui-library-set-delete-all = Delete the set and its albums

gui-library-set-hint = album set

gui-library-set-move-title = Move “{ $name }” to a set

gui-library-set-new = New set…

gui-library-set-new-in = New set inside…

gui-library-set-new-title = New album set

gui-library-set-none = No album set to move it to: make one with New set…

gui-library-set-tip = Album set: its albums' photos together

gui-library-set-toggle = Show or hide the albums of { $name }

gui-library-set-top = No set (the top)

gui-library-sort-ascending = Ascending: oldest, smallest or A first

gui-library-sort-captured = Capture time

gui-library-sort-descending = Descending: newest, largest or Z first

gui-library-sort-edited = Last edit

gui-library-sort-focus = Sharpness

gui-library-sort-id = Content

gui-library-sort-imported = Import time

gui-library-sort-name = File name

gui-library-sort-rating = Rating

gui-library-sort-size = File size

gui-library-sort-tip = Sort by

gui-library-stack-group = Group into Stack

gui-library-stack-not-changed = The stack was not changed: { $error }

gui-library-stack-top = Move to Top of Stack

gui-library-stack-ungroup = Unstack

gui-library-sync = Sync zoom and pan

gui-library-tags = Tags

gui-library-trash = Move to Trash…

gui-library-version-copy = Copy

gui-library-version-not-changed = The photo's versions were not changed: { $error }

gui-library-versions-strip = Versions

gui-library-volume-offline = { $volume } { $count }, offline, last seen { $date }

gui-log-clear = Clear

gui-log-empty = Nothing to report yet

gui-look-fit-selection-done = Camera look of { $camera } fitted from { $fitted } of { $photos } photos: Delta E { $held } (default { $baseline })

gui-look-fit-selection-job = Fit a camera look from { $count } photos

gui-look-fit-selection-none = No camera look fitted: { $reasons }

gui-look-header-chart = Colour half off: the chart profile rules the colours

gui-look-header-compare = Compare with the camera's JPEG

gui-look-header-compare-about = Shows the camera's embedded JPEG left of the split and the render right of it; drag the split to move it

gui-look-header-compare-no-jpeg = No camera JPEG to compare: the raw carries none

gui-look-header-compare-not-raw = No camera JPEG to compare: the photo is not a raw

gui-look-header-default-clear = Clear the default

gui-look-header-default-in-use = The default look of { $camera }: its raws are imported with it

gui-look-header-default-other = { $camera } has another default look: its raws are imported with that one

gui-look-header-default-save = Save as the camera's default

gui-look-header-default-save-about = Raws of { $camera } imported from now on start with this camera look; photos already in the library keep theirs

gui-look-header-fit = Fit to the camera's JPEG

gui-look-header-fit-about = Fits a matrix, a hue and saturation table and tone curves to the camera's embedded JPEG, checked on pixels it did not fit

gui-look-header-fitted = Fitted to the camera's JPEG: Delta E { $held } (default { $baseline })

gui-look-header-none = Not fitted yet: the look starts from the camera's own JPEG

gui-look-header-presets = Presets

gui-look-header-refit = Refit

gui-look-header-refused = Not fitted: { $reason }

gui-look-header-switch = Camera look

gui-look-preset-camera = Camera look

gui-look-preset-camera-about = The camera's default look, or a look fitted to the camera's JPEG when the camera has none

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

gui-notice-details = Details

gui-notice-failed = Something went wrong

gui-notice-no-details = No failure to show the details of

gui-oauth-client-built-in = Helios's client

gui-oauth-client-environment = From the environment

gui-oauth-client-none = None in this build: use your own

gui-oauth-client-own = Your own client

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

gui-pick-choose-folder = Choose the library folder…

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

gui-promise-cancel = Cancel

gui-promise-cancelled = Cancelled

gui-promise-cancelling = Cancelling…

gui-promise-counts = { $done } of { $total }

gui-promise-done = Done

gui-promise-failed = Failed: { $reason }

gui-promise-pending = Waiting to start

gui-promise-retry = Retry

gui-promise-so-far = { $done } so far

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

gui-recent-filters-none = No recent filter yet: the filters used in the Library are listed here

gui-recent-filters-pinned = pinned

gui-recent-filters-recent = recent

gui-recent-filters-smart = smart album

gui-recent-filters-title = Recent filters

gui-recording-saved = recording saved to { $path }

gui-recording-stopped = recording stopped: { $error }

gui-rule-add-condition = + Condition

gui-rule-add-group = + Group

gui-rule-and = and

gui-rule-apply = Apply

gui-rule-cancel = Cancel

gui-rule-down = Move down

gui-rule-field-album = album

gui-rule-field-aperture = aperture

gui-rule-field-camera = camera

gui-rule-field-captured = capture date

gui-rule-field-clip-black = crushed shadows (share)

gui-rule-field-clip-white = blown highlights (share)

gui-rule-field-description = description

gui-rule-field-file-type = file type

gui-rule-field-flag = flag

gui-rule-field-focal-length = focal length

gui-rule-field-focus = sharpness (focus)

gui-rule-field-folder = folder

gui-rule-field-iso = ISO

gui-rule-field-label = colour label

gui-rule-field-lens = lens

gui-rule-field-make = camera make

gui-rule-field-name = Field

gui-rule-field-rating = rating

gui-rule-field-shutter = shutter speed

gui-rule-field-tag = tag

gui-rule-field-title = title

gui-rule-high-name = Second value

gui-rule-match-all = All

gui-rule-match-any = Any

gui-rule-match-name = Rules to match

gui-rule-match-none = None

gui-rule-of-rules = of these rules

gui-rule-op-between = is between

gui-rule-op-contains = contains

gui-rule-op-eq = is

gui-rule-op-ge = is at least

gui-rule-op-gt = is above

gui-rule-op-has = has

gui-rule-op-in = is in

gui-rule-op-is = is

gui-rule-op-is-not = is not

gui-rule-op-lacks = lacks

gui-rule-op-le = is at most

gui-rule-op-lt = is below

gui-rule-op-name = Comparison

gui-rule-op-ne = is not

gui-rule-op-not-in = is not in

gui-rule-op-within = is in

gui-rule-period-last = the last

gui-rule-period-this-year = this year

gui-rule-period-today = today

gui-rule-redo = Redo

gui-rule-remove = Remove

gui-rule-title = The smart album's rules

gui-rule-undo = Undo

gui-rule-unit-days = days

gui-rule-unit-months = months

gui-rule-unit-name = Unit

gui-rule-unit-weeks = weeks

gui-rule-up = Move up

gui-rule-value-name = Value

gui-scene-saved = viewport.scene saved to { $path }

gui-scopes-blue = Blue

gui-scopes-green = Green

gui-scopes-red = Red

gui-settings-account-last-used = Last used

gui-settings-account-name = Account

gui-settings-account-rename = Name

gui-settings-account-rename-help = How this account is called in Helios; the address it signed in as stays beside it.

gui-settings-account-service = Service

gui-settings-account-source = Source

gui-settings-account-status = Status

gui-settings-account-used-by = Used by

gui-settings-account-why = Why

gui-settings-accounts = Accounts

gui-settings-accounts-about = The accounts Helios imports from. Their keys and tokens stay in this computer's keyring; the library keeps only their names.

gui-settings-accounts-add = Add an account

gui-settings-accounts-clients = Use my own OAuth client

gui-settings-accounts-list = Import accounts

gui-settings-accounts-no-service = No service with accounts is installed.

gui-settings-accounts-none = No account yet: add one below.

gui-settings-automatic-imports = Automatic imports

gui-settings-backup = Backup

gui-settings-caches = Caches

gui-settings-catalogue = Catalogue

gui-settings-catalogue-about = Verified snapshots of the catalogue (ratings, tags, edits): kept next to it and copied to the backup disks plugged in.

gui-settings-client-helios = Use Helios's client

gui-settings-client-id = Your client ID

gui-settings-client-secret = Your client secret

gui-settings-client-secret-help = Kept in the keyring and never shown again: type it again to change it.

gui-settings-client-used = Client used

gui-settings-clients-about = Helios signs in to these services with its own OAuth client. To use yours instead (a desktop application you registered with the service), give its client ID and secret: they stay in this computer's keyring. Sign in again afterwards.

gui-settings-clients-none = No service signing in through OAuth is installed.

gui-settings-decisions = Decisions

gui-settings-destinations = Destinations

gui-settings-display = Display

gui-settings-experimental = Experimental

gui-settings-focus-import = Import window

gui-settings-focus-library = { $window }: library { $library }

gui-settings-focus-main = Main window

gui-settings-focus-workspace = { $window }: workspace { $workspace }, library { $library }

gui-settings-focus-workspace-alone = { $window }: workspace { $workspace }

gui-settings-for-library = This library

gui-settings-for-library-about = while this library is open

gui-settings-for-workspace = This workspace

gui-settings-for-workspace-about = while this workspace is open

gui-settings-hardware = Hardware

gui-settings-health = Health

gui-settings-health-check = Health check

gui-settings-history = History

gui-settings-image-map = Image map

gui-settings-library = Library

gui-settings-plugin-granted = Allowed

gui-settings-plugin-no-capability = It asks for no permission.

gui-settings-plugin-reason = Why

gui-settings-plugin-refused = This plugin's settings are not shown: { $problems }

gui-settings-plugin-settings = Settings

gui-settings-plugins = Plugins

gui-settings-plugins-capabilities = Permissions

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

gui-smart-camera = camera (contains)

gui-smart-file-type = file type

gui-smart-flag = flag

gui-smart-folder = folder (absolute)

gui-smart-label = colour label

gui-smart-match = match

gui-smart-new-title = New smart album

gui-smart-rating = at least (stars)

gui-smart-rules-title = The smart album's rules

gui-smart-rules-verb = Apply

gui-smart-tag = tag

gui-smart-taken-from = taken from (YYYY-MM-DD)

gui-smart-taken-to = taken to (YYYY-MM-DD)

gui-snapshot-failed = snapshot failed: { $error }

gui-snapshot-saved = { $target } pictured to { $path }

gui-snapshot-unreadable = Snapshot “{ $name }” cannot be read: { $error }

gui-snapshots-not-saved = The snapshots were not saved: { $error }

gui-splash-closing = Closing Helios…

gui-splash-quit-now = Quit now

gui-splash-starting = Starting Helios…

gui-splash-still-working = Still working…

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

gui-stacks-top-why = Proposed on top: { $why }

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

gui-trash-detail = Their { $files } files in { $place } go to the system Trash, which can restore them; Undo cannot, and they leave the Library.

gui-trash-done = { $count } files moved to the Trash (job { $job })

gui-trash-failed = The files were not moved to the Trash: { $error }

gui-trash-folders = { $count } folders

gui-trash-nothing = No file can be moved to the Trash. { $why }

gui-trash-refused = { $count } files stay where they are: { $reason }

gui-trash-rejected-title = Delete { $count } rejected photos?

gui-trash-some-failed = { $count } failed: { $error }

gui-trash-title = Move { $count } photos to the Trash?

gui-trash-verb = Move to Trash

gui-vault-unreadable = Cannot read the library: { $error }

gui-version-remove-detail = This copy of the photo and the edits only it holds leave the Library; the file and the other versions stay. Undo brings it back.

gui-version-remove-title = Remove version { $number }?

gui-version-remove-verb = Remove Version

gui-window-frame-error = window frame: { $error }

gui-wizard-account = Account

gui-wizard-account-settings = Account settings

gui-wizard-add = Add the source

gui-wizard-back = Back

gui-wizard-card = Cards and cameras are listed on their own when they are inserted, with the default choices: there is nothing to add.

gui-wizard-choices = How its photos are imported. You can change these later in the Choices column.

gui-wizard-choose = Choose…

gui-wizard-folder = Folder

gui-wizard-kind = What kind of source is it?

gui-wizard-name = Source name

gui-wizard-name-help = How the source is listed in the Sources column

gui-wizard-next = Next

gui-wizard-no-account = No account yet

gui-wizard-no-folder = No folder there

gui-wizard-no-library = Open a library first: accounts are kept in it

gui-wizard-sign-in = Sign in to an account…

gui-wizard-title = Add a source

gui-workshop-run-job = Workshop “{ $name }”

gui-workshops-ai-left = AI per photo: { $junctions } junctions, { $calls } calls

gui-workshops-ai-none = AI per photo: none

gui-workshops-aim = Aim again

gui-workshops-aim-hint = Solve its targets again on the open image (one undo step)

gui-workshops-aimed = aimed: { $measure } at { $value } { $unit }

gui-workshops-applied = Applied: { $node }

gui-workshops-applied-hint = Select its node, to tune it below

gui-workshops-apply = Apply

gui-workshops-apply-hint = Insert { $name } after the node the image is shown from, aimed on the open image (one undo step)

gui-workshops-automatic = Make it automatic

gui-workshops-automatic-hint = Turn what you set into a workshop for other photos: a target where a measure explains a value, else the same value; you confirm each row

gui-workshops-confirm = Confirm

gui-workshops-confirm-hint = Write the workshop's next version with these choices, as one undo step; a row kept as AI is asked on each photo

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

gui-workshops-flag-ai-budget = AI budget spent

gui-workshops-flag-at-limit = at its limit

gui-workshops-flag-deviation = far from the test photos

gui-workshops-flag-fallback = no AI: fallback used

gui-workshops-flag-low-confidence = AI unsure

gui-workshops-flag-not-monotonic = could not aim

gui-workshops-flag-target-missed = target missed

gui-workshops-flag-unlike = unlike the test photos

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

gui-workshops-review-accept = Accept

gui-workshops-review-accept-hint = Write this photo with the values the run found, flags and all

gui-workshops-review-accepted = accepted

gui-workshops-review-applied = applied

gui-workshops-review-close = Close

gui-workshops-review-counts = { $photos } photos, { $flagged } to review, { $held } held; AI calls: { $calls }

gui-workshops-review-decide = Decide

gui-workshops-review-decide-hint = Answer this junction for this photo: recorded in the workshop's decision book, a re-run takes it

gui-workshops-review-failed = failed

gui-workshops-review-fixed = fixed by hand

gui-workshops-review-flagged-only = Only photos to review

gui-workshops-review-held = held

gui-workshops-review-last = Review the last run

gui-workshops-review-refresh = Refresh

gui-workshops-review-rerun = Re-run held and decided ({ $count })

gui-workshops-review-rerun-hint = Run the workshop again on the held photos and those with a junction decided: a new run, one undo step

gui-workshops-review-revert = Revert

gui-workshops-review-revert-hint = Take the run's version of this photo out: its edit before the run is current again

gui-workshops-review-reverted = reverted

gui-workshops-review-set = Set

gui-workshops-review-set-hint = Fix this value on this photo by hand: a new version, its flag cleared

gui-workshops-review-stopped = stopped the run

gui-workshops-review-title = Review: { $name }, run { $n }

gui-workshops-review-unchanged = unchanged

gui-workshops-review-value = { $slider }: { $before } before, { $after } after ({ $source })

gui-workshops-row-ai = Keep as AI

gui-workshops-row-fixed = Same value { $value }

gui-workshops-row-target = Same target

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

gui-workshops-source-ai = AI

gui-workshops-source-aimed = aimed

gui-workshops-source-fallback = fallback

gui-workshops-source-fixed = fixed

gui-workshops-source-user = set by hand

gui-workshops-unit-percent = %

gui-workshops-unit-stops = stops

gui-workshops-workspace = This workspace

gui-workshops-zone-parametric = Parametric mask

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

health-fix-choose-library-folder = Choose a folder…

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

health-library-folder = Library folder

health-library-folder-none = No library folder chosen

health-library-folder-none-detail = Imports that copy photos and picked photos have nowhere to go: they ask for a folder first.

health-library-folder-ok = A library folder is chosen

health-library-folder-ok-detail = { $folder }

health-library-folder-unwritable = The library folder cannot be written

health-library-folder-unwritable-detail = { $folder } is missing or read-only: imports cannot copy photos into it.

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

init-library-folder = Library folder

init-library-folder-why = Imports copy photos into it, and picked photos go there.

library-folder-no-room = This disk has { $free } MiB free: choose a folder with room for photos.

library-folder-not-absolute = Give the folder's full path.

library-folder-not-writable = Helios cannot write in this folder: { $why }

look-default-bad-params = the look's { $node } parameters: { $error }

look-default-no-camera = a camera's default look needs `camera`

look-default-not-fitted = no fitted camera look to keep: fit one to the camera's JPEG first

look-default-what = give one of `look`, `image` or `clear`

look-fit-cancelled = the camera look's fit was cancelled: nothing was written

look-load-failed = cannot read the photo: { $error }

look-mixed-cameras = the photos are of { $count } cameras ({ $cameras }): a camera look is fitted from photos of one camera; select one camera's photos

look-render-failed = cannot render the raw: { $error }

node-batch-only = the batch endpoint only answers load-source, export, cancel, job.cancel, batch_start, batch_end, the workflow.* requests, library.import, settings.layer.*, commands.list and command.run

node-boot-vault = Opening the library

node-command-debug-only = only the debug control endpoint runs it (helios-node --control, debug builds)

node-command-needs-vault = the node keeps no vault: start it with --vault

node-command-needs-window = no desktop window holds this workspace

node-command-origin = `{ $origin }` is not an origin: user, cli, control, agent:NAME or script:NAME

node-command-params = `{ $id }` refused its params: { $error }

node-command-unknown = no command `{ $id }`

node-edit-not-saved = done, but the edit was not saved: { $error }

node-end-jobs = Stopping background jobs

node-end-jobs-waiting = Waiting for { $count } running jobs to finish

node-end-vault = Saving and closing the library

node-job-unknown = job { $job } is unknown or already ended

node-load-refused = cannot load the image: { $error }

node-no-extent = `{ $name }` has no extent: give a region

node-no-node = no node named `{ $name }`

node-no-settings = the node keeps no settings: start it with --vault

node-no-vault = the node has no vault to import into: start it with --vault

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
