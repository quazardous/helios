# Generated from Helios's declarations and code by `cargo xtask i18n extract`:
# do not edit. Translations live in i18n/<locale>/ under the same names.

engine-failure-compile = compile

engine-failure-device-lost = device lost

engine-failure-dispatch = dispatch

engine-failure-other = other

engine-failure-out-of-memory = out of memory

engine-failure-timeout = timeout

engine-gpu-fell-back = { $module } ran on the CPU: its GPU version failed ({ $failure }: { $reason }).

engine-gpu-lost = The GPU ({ $gpu }) failed ({ $failure }: { $reason }) and could not be restarted ({ $why }); the rest of this session renders on the CPU.

engine-gpu-lost-recovered = The GPU ({ $gpu }) failed ({ $failure }: { $reason }); it was restarted and the render replayed.
