kind: added

A profile can differ per system or per machine: a bigger font on the
laptop, another shortcut on macOS. Its parts may hold variants for a
system (`macos`, `linux`, `windows`), a high-density screen (`hidpi`) or
a machine named once in `machine.toml` in the config directory; Helios
applies those that match where it runs, the most specific last, and
`profile.resolved` says which values came from which variant. Variants
for other machines stay in the file, travel with an export and show in an
import's diff; `profile.export` with `this_machine` writes only this
machine's view.
