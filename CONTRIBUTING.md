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

## Linters and formatters

```bash
cargo xtask lint          # every check below
cargo xtask lint --fix    # apply the formatters first, then check
```

`cargo xtask lint` runs rustfmt and clippy, then each of these tools that
is installed, and names the missing ones (CI installs them all and runs
`cargo xtask lint --require-all`):

| Tool | Checks | Config | Install |
|------|--------|--------|---------|
| cargo-deny | licenses, RustSec advisories, sources | `deny.toml` | `cargo install --locked cargo-deny` |
| cargo-machete | unused dependencies | | `cargo install --locked cargo-machete` |
| taplo | TOML formatting | `taplo.toml` | `cargo install --locked taplo-cli` |
| typos | spelling | `_typos.toml` | `cargo install --locked typos-cli` |
| shellcheck | shell scripts | | your distribution's package |
| ruff | Python lint and format | `ruff.toml` | `pip install ruff` |

Clippy runs with the `pedantic` group on every crate (`[workspace.lints]`
in Cargo.toml, with the lints allowed workspace-wide and why); a lint
allowed in one place says why in a `reason`. Unsafe code is denied. A word
typos flags that is right (darktable or vkdt jargon) goes in `_typos.toml`.
A license cargo-deny rejects needs a decision before it goes in
`deny.toml`.

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
