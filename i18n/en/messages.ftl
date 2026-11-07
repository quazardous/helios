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

gui-caption-decisions = Decisions waiting

gui-caption-decisions-count = { $count } stack suggestions waiting for an answer

gui-caption-jobs = Show the jobs

gui-caption-profile = Profile in use: { $name }

gui-confirmed-by-policy = { $verb }: confirmed by policy

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

gui-import-path = card or folder

gui-import-remember = Remember for this workspace

gui-import-remember-help = The workspace's import source: the next import does not ask

gui-import-verb = Import

gui-import-window-add-favourite = Add to favourites

gui-import-window-all-types = All types

gui-import-window-cancel = Cancel

gui-import-window-card-inserted = { $card } inserted

gui-import-window-cards = Cards and cameras

gui-import-window-choices = Choices

gui-import-window-choices-card = A card is copied into the Library.

gui-import-window-choices-folder = A folder is added where it is.

gui-import-window-choose = Choose a folder…

gui-import-window-contents = Contents

gui-import-window-counts = { $photos } photos, { $new } new, { $size }

gui-import-window-empty = No photos in the sources ticked

gui-import-window-favourites = Favourites

gui-import-window-import = Import { $count }

gui-import-window-in-library = in your Library

gui-import-window-jpeg-only = JPEG only

gui-import-window-new-only = New only

gui-import-window-no-cards = No card inserted

gui-import-window-no-date = No date

gui-import-window-no-favourites = None yet

gui-import-window-none-ticked = Tick the photos to import

gui-import-window-nothing = Tick a card or a folder to import

gui-import-window-open = Import photos…

gui-import-window-pair = RAW+JPG

gui-import-window-raw-only = Raw only

gui-import-window-reading = Reading…

gui-import-window-recent = Recent

gui-import-window-remove-favourite = Remove from favourites

gui-import-window-select = Select:

gui-import-window-select-all = all

gui-import-window-select-day = day

gui-import-window-select-none = none

gui-import-window-sources = Sources

gui-import-window-title = Import photos

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

gui-library-compare-hint = Select two to four images to compare them

gui-library-count = { $shown } of { $total }

gui-library-empty = No photos yet: import a card or a folder, or drop photos on the window

gui-library-file = { $size } · modified { $day }

gui-library-images = Images

gui-library-import = Import…

gui-library-import-photos = Import photos…

gui-library-keep-best = Keep the best: pick it, reject the others

gui-library-no-cursor = No image under the cursor

gui-library-none = No images

gui-library-not-saved = Not saved, the library shows what was saved last: { $error }

gui-library-read = { $count } images in the library

gui-library-reading = Reading the library…

gui-library-search = Search images

gui-library-sync = Sync zoom and pan

gui-library-tags = Tags

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

gui-macro-iface-filter = Filter

gui-macro-iface-group = Group

gui-macro-iface-inner = Inner parameters

gui-macro-iface-label = Label

gui-macro-iface-linked = Linked to the first row named so: it is shown as that one is

gui-macro-iface-name = Name

gui-macro-iface-name-hint = The same name on two rows links them: one value sets both

gui-macro-iface-none = Nothing shown: tick inner parameters on the left

gui-macro-iface-options = Options kept

gui-macro-iface-pick = Select a parameter shown to set its name, range and default

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

gui-paste-import-copy = The files are copied into the library folder.

gui-paste-import-image = The image is saved as a PNG file and copied into the library folder.

gui-paste-import-reference = The files stay where they are.

gui-paste-import-source = The files are placed as the workspace's import source says.

gui-paste-import-title = Import the pasted files ({ $count })?

gui-paste-import-verb = Import

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

gui-recording-saved = recording saved to { $path }

gui-recording-stopped = recording stopped: { $error }

gui-scene-saved = viewport.scene saved to { $path }

gui-scopes-blue = Blue

gui-scopes-green = Green

gui-scopes-red = Red

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

gui-status-history = History

gui-status-history-detail = { $count } steps to undo

gui-status-history-redo = Redo

gui-status-history-undo = Undo

gui-status-images = { $count } images

gui-status-zoom = Zoom

gui-status-zoom-fill = Fill

gui-status-zoom-fit = Fit

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

gui-workshops-experimental = Experimental: may change; what it saves stays a macro

gui-workshops-find = Find

gui-workshops-find-hint = what you want: brighter, less contrast…

gui-workshops-library = Your library

gui-workshops-measure-clip = clipping

gui-workshops-measure-high = highlights

gui-workshops-measure-median = midtones

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
