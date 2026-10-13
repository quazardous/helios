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

xmp-not-applied = { $count } of { $items } history items not applied, kept for later: { $groups }

xmp-not-applied-group = { $labels } ({ $reason })

xmp-reason-blending = blending not supported

xmp-reason-extra-instance = a second instance of a single-instance module

xmp-reason-unknown-blend-version = blend params v{ $version } not supported

xmp-reason-unknown-module = no Helios module

xmp-reason-unreadable = unreadable: { $error }

xmp-reason-unsupported-version = version not supported (reads { $known })
