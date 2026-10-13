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

## Test times

CI runs the tests with [cargo-nextest](https://nexte.st/) (one process per
test, `cargo install --locked cargo-nextest`), and the doctests with
`cargo test --doc`, which nextest does not run:

```bash
cargo nextest run --workspace     # what CI runs, with --profile ci
cargo xtask test-times            # the same, timed and recorded
cargo xtask test-times --top 30 --profile ci
```

`cargo xtask test-times` keeps one line per run in
`target/test-times/history.jsonl` and prints the slowest tests, the time
per crate, and the tests that took more than 50 % and more than 0.5 s
longer than the median of their last five runs. CI writes the 15 slowest
tests to the job summary and keeps the JUnit report as an artifact.

nextest warns about a test still running after 10 s and stops it after
120 s (`.config/nextest.toml`). When the suite takes more than 3 minutes
locally, or one test more than 5 s, open an optimisation pass: find what
the slow tests wait for rather than raising a limit.

No `sleep` in a test, not even in a polling loop: wait on the event that
says the work is done (a channel, a condition variable, a bus event), with
a generous deadline that only a hang reaches. A sleep makes the test slow
when the machine is idle and flaky when it is loaded.

## Benches

```bash
cargo xtask bench masks                 # the mask stress network
cargo xtask bench masks --modules 4     # a longer chain
```

Each mode (full, tiled, proxy, edit) runs in its own release process and
prints one JSON line: time, peak memory, steps computed and shared, and
content keys planned. The lines are appended to
`target/bench/masks.jsonl` with the date and commit. See docs/masks.md.

## Bench box

Long jobs (the full build, the test suite, the timings, the tests on a
real GPU) also run on a shared bench box, on a pushed branch or on the
working tree as it is:

```bash
cargo xtask bench-remote test                  # the current branch, as GitHub has it
cargo xtask bench-remote test --push           # this working tree
cargo xtask bench-remote bench-masks branch=main
```

Exit 75 means the box could not serve the job (off, in maintenance, its
GPU gone), never a test failure. See docs/bench.md.

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

An optional pre-commit hook runs the fast checks (rustfmt and
`cargo xtask check`) before each commit:

```bash
ln -s ../../tools/pre-commit .git/hooks/pre-commit
```

## Commits

One purpose per commit, and every commit builds and passes its tests. The
message says what changes and why, in plain sentences.

Before pushing a series of commits, agents included, check each one:

```bash
cargo xtask check-commits                 # @{upstream}..HEAD, else origin/main..HEAD
cargo xtask check-commits HEAD~3..HEAD    # a given range
cargo xtask check-commits --full          # every fast test on every commit
```

Every commit of the range but the last (run the full gate on that one)
is checked out in a temporary worktree under `target/` and goes through
`cargo build --workspace --all-targets`, clippy with `-D warnings`, the
tests that commit touches and `cargo xtask check`. The tests are chosen
as `cargo xtask affected` chooses them, from the commit's own diff
against its parent: the fast tests (nextest without the parity tests,
the headless GUI tests and the masks stress test) of the crates it
changes and of their dependents, plus the critical set. A commit that
changes `Cargo.lock`, xtask, CI or another path that runs everything
runs every fast test and the critical set; `--full` runs every fast test
on every commit. The walk stops at the first broken commit and names it,
and the report says, per commit, what ran and why. It is capped at the
20 newest commits; the older ones are listed as skipped. CI runs the
same walk on each push and pull request.

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

`cargo xtask release` first runs the survival scenarios (`cargo
survival`: the desktop and a node quit or killed with `kill -9`, then
reopened, keep every edit step, rating and label saved; see
docs/persistence.md) and refuses to release when one fails. It then
writes the new section at the top of CHANGELOG.md,
bumps the version and removes the fragments. It does not commit or tag:
review the diff, run `cargo check` to refresh Cargo.lock, then commit
"Release X.Y.Z" and tag `vX.Y.Z`.

A merge to main is a release (decided 2026-10-13): each branch merged
with user-facing fragments is released right after it lands, so the
version a build reports says what it contains. The bench box prepares
it (`cargo xtask bench-remote release branch=main`: survival, the
CHANGELOG section, the bump and Cargo.lock, as `out/release.patch`);
apply the patch on main, commit "Release X.Y.Z", tag `vX.Y.Z` and push
both.
