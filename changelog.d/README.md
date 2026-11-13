# Changelog fragments

One file per user-visible change, added in the same commit as the change.
A fragment may ask for a release with a second line
`release: patch | minor | major` (`false` by default). The promotion of
`next` to `main` that carries such a fragment releases every pending one
(`cargo xtask promote`): it assembles them into CHANGELOG.md, bumps the
version, removes them, and the release is tagged and published. Fragments
that ask for none wait for one that does, or for 7 days.
How to write one: see CONTRIBUTING.md, "Changelog".
