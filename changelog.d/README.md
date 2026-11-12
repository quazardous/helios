# Changelog fragments

One file per user-visible change, added in the same commit as the change.
The promotion of `next` to `main` releases them (`cargo xtask promote`,
the same as `cargo xtask release`): it assembles them into CHANGELOG.md,
bumps the version and removes them.
How to write one: see CONTRIBUTING.md, "Changelog".
