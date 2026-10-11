# Contributing to Helios

Helios is written in English: code, comments, docs, commit messages and
changelog. Everything in the repository must make sense to an outside
reader: no private paths, no links or ticket numbers they cannot follow.

## Build and test

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all --check
cargo xtask check
```

Rust first (see ROADMAP.md, section 1): new code is Rust unless Rust is not
practical. Ask before adding a dependency whose license is not compatible
with the GPLv3.

Tests never sleep and never touch a real photo library: they wait on
events and run in their own directories (see docs/testing-strategy.md).

## Commits

One purpose per commit, and every commit builds and passes its tests. The
message says what changes and why, in plain sentences.

## Changelog

The changelog is not a commit log: it tells a user what changed for them
and why it matters. It is written as **fragments**, one file per
user-visible change in `changelog.d/`, added in the same commit as the
change:

```text
kind: fixed

Exports to a full disk now stop with a clear message instead of leaving a
half-written file.
```

- `kind` is one of `breaking`, `added`, `changed`, `deprecated`, `removed`,
  `fixed`, `security`, or `internal` for a change with nothing to tell a
  user (it satisfies the check and never reaches the changelog).
- One to three sentences, in plain language: the effect for a user, not the
  implementation. No file names, no private paths, no ticket numbers; a
  public link is fine.
- Name the file after the change (`full-disk-export.md`); sections list
  fragments in file name order.

Good: "A wait that times out now says which events happened instead."
Not a changelog entry: "Refactor engine.rs to use a HashMap" (that is a
commit message), "Fix #42" (a reader cannot follow it).

`cargo xtask changelog` previews the next release. `cargo xtask check`
validates the fragments; with `--base origin/main` it also reports code
changes that come without a fragment.

## Versions and releases

One version for the whole workspace (`workspace.package.version` in
Cargo.toml), following Semantic Versioning, and derived from the
fragments: `breaking` bumps the major version (the minor one before 1.0),
`added`, `changed`, `deprecated` and `removed` bump the minor version,
`fixed` and `security` alone bump the patch version.

`cargo xtask release` writes the new section at the top of CHANGELOG.md,
bumps the version and removes the fragments. It does not commit or tag:
review the diff, run `cargo check` to refresh Cargo.lock, then commit
"Release X.Y.Z" and tag `vX.Y.Z`.
