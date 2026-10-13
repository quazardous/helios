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

gui-confirmed-by-policy = { $verb }: confirmed by policy

gui-copied = copied { $text }

gui-edit-does-not-fit = Part of the saved edit does not fit this network: { $part }

gui-edit-not-saved = The edit was not saved: { $error }

gui-gesture-in-progress = { $gesture } is in progress

gui-library-file = { $size } · modified { $day }

gui-library-not-saved = Not saved, the library shows what the vault holds: { $error }

gui-masks-group-full = the group `{ $group }` holds { $count } members already

gui-masks-into-itself = a group cannot go into itself

gui-masks-not-a-group = `{ $name }` is not a group: only a group takes members

gui-masks-nothing-beside-root = nothing goes beside the module's mask: drop inside it

gui-masks-one-at-a-time = members move one at a time

gui-masks-root-stays = the module's mask stays the root

gui-no-dialogs = no dialogs to show this in

gui-open-workspace-directory = directory

gui-open-workspace-title = Open a workspace

gui-open-workspace-verb = Open

gui-opening = opening { $path }

gui-param-default = { $path } · default { $value }

gui-param-held = { $label }: held to { $value }

gui-param-hint = Drag or arrows to change, the wheel once focused or with Alt (Shift: finer drag, coarser steps); double click resets; Enter types a value

gui-param-not-a-number = { $label }: `{ $typed }` is not a number

gui-param-past-range = { $label }: past its usual range

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

gui-recording-saved = recording saved to { $path }

gui-recording-stopped = recording stopped: { $error }

gui-scene-saved = viewport.scene saved to { $path }

gui-size-bytes = { $size } B

gui-size-gib = { $size } GiB

gui-size-kib = { $size } KiB

gui-size-mib = { $size } MiB

gui-snapshot-failed = snapshot failed: { $error }

gui-snapshot-saved = { $target } pictured to { $path }

gui-theme-exported = theme exported to { $path }

gui-theme-problem = theme { $name }: { $problem }

gui-theme-unknown = theme { $name }: no theme `{ $name }`: { $default } is used instead

gui-vault-unreadable = Cannot read the vault: { $error }

gui-window-frame-error = window frame: { $error }

node-batch-only = the batch endpoint only answers load-source, export and cancel

node-edit-not-saved = done, but the edit was not saved: { $error }

node-job-unknown = job { $job } is unknown or already ended

node-load-refused = cannot load the image: { $error }

node-no-extent = `{ $name }` has no extent: give a region

node-no-node = no node named `{ $name }`

node-param-refused = cannot set { $node }.{ $param }: { $error }

node-path-not-absolute = { $path }: give an absolute path, the node does not know where you are

node-source-refused = `{ $name }` cannot take the image: { $error }

xmp-not-applied = { $count } of { $items } history items not applied, kept for later: { $groups }

xmp-not-applied-group = { $labels } ({ $reason })

xmp-reason-blending = blending not supported

xmp-reason-extra-instance = a second instance of a single-instance module

xmp-reason-unknown-blend-version = blend params v{ $version } not supported

xmp-reason-unknown-module = no Helios module

xmp-reason-unreadable = unreadable: { $error }

xmp-reason-unsupported-version = version not supported (reads { $known })
