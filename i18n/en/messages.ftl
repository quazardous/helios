# Generated from Helios's declarations and code by `cargo xtask i18n extract`:
# do not edit. Translations live in i18n/<locale>/ under the same names.

batch-cancelled = cancelled

batch-cannot-create = cannot create { $path }: { $error }

batch-cannot-read = cannot read { $path }: { $error }

batch-failed = failed

batch-needs-network = batch needs --network FILE, or a node to attach to

batch-no-source = the network has no source

batch-node-refused = the node refused

batch-node-unreachable = cannot reach the node at { $address }: { $error }

batch-several-ends = the network ends in several nodes ({ $names }): say --node NAME

batch-several-sources = the network has several sources ({ $names }): say --source NAME

batch-vault-needs-sidecar = --vault keeps sidecars: it needs --sidecar

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

export-no-format = no format for { $path }

export-not-a-file = { $path } is not a file path

export-png-bits = PNG has 8 or 16 bits, not { $bits }

export-write-failed = cannot write { $path }: { $error }

gui-about-crashes = Crash reports: { $path }

gui-about-logs = Log file: { $path }

gui-confirmed-by-policy = { $verb }: confirmed by policy

gui-copied = copied { $text }

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

gui-gesture-in-progress = { $gesture } is in progress

gui-import-form-title = Import a card or a folder

gui-import-path = card or folder

gui-import-verb = Import

gui-job-batch-done = Batch finished

gui-job-batch-failed = Batch failed

gui-job-export-done = Export finished

gui-job-export-failed = Export failed

gui-library-auto-advance = Auto-advance

gui-library-compare-hint = Select two to four images to compare them

gui-library-count = { $shown } of { $total }

gui-library-empty = No images: import a folder into the vault

gui-library-file = { $size } · modified { $day }

gui-library-images = Images

gui-library-import = Import…

gui-library-keep-best = Keep the best: pick it, reject the others

gui-library-no-cursor = No image under the cursor

gui-library-none = No images

gui-library-not-saved = Not saved, the library shows what the vault holds: { $error }

gui-library-search = Search images

gui-library-sync = Sync zoom and pan

gui-library-tags = Tags

gui-masks-add = Add…

gui-masks-add-hint = Add a shape or a parametric member to the selected group. With the image focused: C a circle, E an ellipse, G a gradient, P a path.

gui-masks-beside-refinement = nothing goes beside a refinement: drop on a member or into a group

gui-masks-empty = The mask is empty: { $module } applies everywhere. Add a shape or a parametric member to limit it.

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

gui-masks-show = Show { $name }: its opacity full

gui-masks-take-out = Take out

gui-masks-take-out-group = Take the group { $name } out of the mask?

gui-masks-take-out-group-detail = Its members go with it, and taking out is not an undo step yet.

gui-masks-take-out-hint = Take the selected member out of the mask (Delete, the tree focused). Not an undo step yet.

gui-masks-take-out-verb = Take out

gui-masks-tree-hint = F2 renames the selected row. Drag a row to reorder it or into a group. Right-click a row for its menu: it never deletes anything.

gui-masks-unlock = Unlock { $name }

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

gui-param-hint = Drag to change (Ctrl finer, Shift coarser), arrows to step, the wheel once focused or with Alt; double click resets; Enter types a value or an expression (x is the value)

gui-param-past-range = { $label }: past its usual range

gui-param-picker = Pick { $label } from the image

gui-params-hide-values = Hide the values

gui-params-points = { $count } points

gui-params-show-values = Show the values

gui-params-values = { $count } values

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

gui-quick-open-image = image

gui-quick-open-module = module { $module }

gui-quick-open-source = source

gui-quick-open-title = Go to a module, an image or a workspace

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

gui-template-keys-error = template { $name }: keys: { $error }

gui-theme-exported = theme exported to { $path }

gui-theme-problem = theme { $name }: { $problem }

gui-theme-unknown = theme { $name }: no theme `{ $name }`: { $default } is used instead

gui-vault-unreadable = Cannot read the vault: { $error }

gui-window-frame-error = window frame: { $error }

node-batch-only = the batch endpoint only answers load-source, export, cancel, job.cancel, batch_start and batch_end

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

xmp-not-applied-group = { $labels } ({ $reason })

xmp-reason-blend = blending not supported: { $what }

xmp-reason-blending = blending not supported

xmp-reason-extra-instance = a second instance of a single-instance module

xmp-reason-mask = drawn mask not supported: { $why }

xmp-reason-mask-without-image = a drawn mask, read without its image

xmp-reason-unknown-blend-version = blend params v{ $version } not supported

xmp-reason-unknown-module = no Helios module

xmp-reason-unreadable = unreadable: { $error }

xmp-reason-unsupported-version = version not supported (reads { $known })
