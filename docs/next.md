# The `devel` → `next` protocol

How every change reaches Helios, for people and agents alike. The same
protocol holds in egui-pop, egui_stack, egui_fit and egui_hud.

> **In effect from the day `cargo xtask gate` lands.** Until then, authors
> still merge their own pull requests into `next` after their narrow box
> jobs pass (the protocol below, minus the tester).

Three branches, three roles:

| Branch | Who writes it | What it holds |
|---|---|---|
| `devel` | the **tester** only | pull requests merged to be tested together |
| `next` | the tester (fast-forward) and the promotion bot | what passed the box tests; CI runs on each push |
| `main` | the promotion bot only | what passed `next`'s CI, with its releases |

## For a change (an author's whole job)

1. **Branch from `origin/devel`,** in a worktree of its own, building into
   its own `target/` (`env -u CARGO_TARGET_DIR`). Never from `main`; never
   `git stash`.
2. **Code with light local checks only:** `cargo fmt`, `cargo check --tests`
   on the crates touched (`CARGO_INCREMENTAL=0`), `typos`, and
   `cargo update -w --offline` (commit `Cargo.lock` if it changed).
   No local tests, no local clippy, no release build: a local hook refuses
   them. Changelog fragments: at most 600 characters, for user-visible
   changes only, with a `kind:` line and **no `release:` line** (the
   coordinator decides releases, below).
3. **Open a pull request into `devel`** with a **test manifest** in its
   body: the narrow bench-box jobs that prove the change, and what they
   prove.

   ```text
   Tests:
   - test-crate crate=helios-gui test=library_filter
   - test-crate crate=helios-vault test=lib filter=albums_nest
   - i18n-extract
   Coverage: the drop rules on an album, a smart album and a set.
   ```

   - One job per line, as `cargo xtask bench-remote <job> …` takes it,
     without `branch=` (the tester gives it).
   - `filter=` names **one** test: two names select none, and the job
     passes empty.
   - Never `test`, `test-affected` or `--affected`: the whole suite is
     `next`'s CI.
   - A change with nothing to run says `Tests: none — <why>`.
   - `cargo xtask manifest check <file>` (or the body on stdin) checks it.
4. **Stop.** Post the step on the ticket and end. **Never merge, never run
   or wait for box jobs**: the tester does. Delete the worktree and its
   `target/`; the branch stays until the tester merges or returns it.

Sizing: one author, one pull request, one small slice, under an hour.

## The tester

One agent, with a standing brief ([tester.md](tester.md)), takes the pull requests waiting on
`devel` in order (first in, first out):

1. Picks a batch of one to three that do not touch the same files, and
   merges them into `devel` (rebasing a branch that falls behind).
2. Runs the union of their manifests on the bench box: one build, the jobs
   side by side (`cargo xtask gate`). The box runs `check` and clippy first.
3. **Green:** fast-forwards `next` to `devel`, deletes the merged branches,
   posts the result on each ticket.
4. **Mechanical red** (clippy, fmt, an i18n stamp or extract, a missing
   command icon, an empty filter): fixes it on `devel` and runs that job
   again.
5. **Real red** (a test failing on what the change does): finds the pull
   request whose jobs failed (or reverts one at a time), reverts it from
   `devel`, and sends it back to its author with the log; the others go on
   to `next`.
6. After a green batch, says what reached `next` that a user would notice
   since the last release, and recommends: no release, a patch or a minor
   one. **The coordinator decides**; a release is a one-line fragment
   (`kind: internal` and `release: patch|minor`) through the same gate,
   and the promotion of `next` tags and publishes it (docs/releases.md).

Every failed box job gets its reason at once:
`bench-remote why <id> "<cause, and what is done about it>"`.

## What tests run where

| Where | What | When |
|---|---|---|
| Local | fmt, check, typos | while coding |
| Bench box, narrow | the manifests' jobs (clippy first) | the tester, per batch on `devel` |
| `next`'s CI | the whole suite, survival, the commits walk, lint, Windows | every push of `next` |
| Bench box, wide | `test-affected`, `test`, GPU, timing, `stability` | daily, before a release, on demand |

CI looks for regressions and backward-compatibility breaks. Load, soak and
stress runs (`stability`) never go into CI.

## What happens on `next`

- Every push runs the whole CI. Runs are not cancelled: the newest push
  waits behind the run going, and each green run is promoted.
- When CI passes, `promote.yml` promotes that exact commit: when a
  fragment asks for a release (or one has waited 7 days) it commits
  "Release X.Y.Z" on top of it (version, CHANGELOG, Cargo.lock), tags and
  publishes it; then it fast-forwards `main`, and `next` if it has not
  moved.
- **One watcher** (the coordinating agent) follows CI on `next`. A red run
  gets **one fixing agent first**, before any other work: it reproduces the
  failure, adds the test that would have caught it, fixes it, and opens its
  pull request into `devel` like any change. A bug the owner reports is
  handled the same way: agents reproduce and validate it themselves and
  never ask the owner to test.
- External contributors' pull requests (from forks) target `next` and run
  the full CI as usual.

See also [bench.md](bench.md) for the box's jobs and its rules, and
[releases.md](releases.md) for promotion and tags.
