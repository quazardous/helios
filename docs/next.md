# The `next` protocol

How every change reaches Helios, for people and agents alike. In every
repository (Helios, egui-pop, egui_stack, egui_fit, egui_hud) the author
merges into `next` after its own narrow box jobs pass.

> **Since 2026-11-13, evening:** authors test and merge their own pull
> requests again (owner: it worked better than one gate for every pull
> request). `cargo xtask gate` and a merger's brief ([tester.md](tester.md))
> stay for a backlog nobody merged.

Two branches, two writers:

| Branch | Who writes it | What it holds |
|---|---|---|
| `next` | each **author** (its own pull request, after its box jobs), a merger for a backlog (`cargo xtask gate`), and the promotion bot | what passed the box tests; CI runs on each push |
| `main` | the promotion bot only | what passed `next`'s CI, with its releases |

A pull request is merged only when its tests passed on the box, so
"merged" always means "on `next`".

## For a change (an author's whole job)

1. **Branch from `origin/next`,** in a worktree of its own, building into
   its own `target/` (`env -u CARGO_TARGET_DIR`). Never from `main`; never
   `git stash`.
2. **Code with light local checks only:** `cargo fmt`, `cargo check --tests`
   on the crates touched (`CARGO_INCREMENTAL=0`), `typos`, and
   `cargo update -w --offline` (commit `Cargo.lock` if it changed).
   No local tests, no local clippy, no release build: a local hook refuses
   them. Changelog fragments: at most 600 characters, for user-visible
   changes only, with a `kind:` line and **no `release:` line** (the
   coordinator decides releases, below).
3. **Open a pull request into `next`** with a **test manifest** in its
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
     without `branch=` (add it when you run them).
   - `filter=` names one test, or several joined with `+`
     (`filter=a+b`); a comma or a space between names is refused.
   - Never `test`, `test-affected` or `--affected`: the whole suite is
     `next`'s CI.
   - A change with nothing to run says `Tests: none — <why>`.
   - `cargo xtask manifest check <file>` (or the body on stdin) checks it.
4. **Prove it on the box yourself** (owner, 2026-11-13: the author who
   wrote the change runs its narrowest tests; it beat batching every pull
   request through one gate). First the branch's `check`:
   `cargo xtask bench-remote check branch=<branch>` (clippy, every lint
   at once). Then each manifest job with `branch=<branch>`. Wait in the
   foreground with `jbx wait <id>`: a subagent's turn stays alive. Fix
   what is red on the branch and run that job again.
5. **Merge it yourself** when every job is green: merge the pull request
   into `next` (`gh pr merge <n> --merge --delete-branch`). If `next`
   moved meanwhile, merge `origin/next` into the branch, check it again
   locally, and merge. Post the step on the ticket. Delete the worktree
   and its `target/`.
6. `next`'s full CI runs after the merge. A red run there gets a fixer at
   once, ahead of everything (below). `cargo xtask gate` stays for a
   backlog of pull requests nobody merged, run by a merger.

Sizing: one author, one pull request, one small slice, under an hour.

## A merger (for a backlog)

When pull requests pile up unmerged, a merger agent, with a standing
brief ([tester.md](tester.md)), takes them into `next` in order (first
in, first out):

1. Picks a batch of one to three that do not touch the same files.
2. Runs `cargo xtask gate <n>…`. First each pull request's own `check`
   (clippy, every lint at once) runs on its own branch, side by side; one
   that fails is left out of the batch with its lints, so it cannot turn
   the batch red. The others are merged on a temporary branch `gate/<n>-…`
   made from `next`, and the union of their manifests runs on the bench
   box on that branch (one build, the jobs side by side; the box runs
   `check` and clippy first).
3. **Green:** `next` fast-forwards to the tested commit, GitHub marks the
   pull requests merged, their branches and the temporary one are deleted;
   the tester posts the result on each ticket. Beside the tests the box
   builds the tested commit for the desktop (`build-gui`, red if it fails),
   and the gate brings its programs back: it prints the
   `cargo xtask install-dev --from <dir>` that installs them on a
   workstation at once.
4. **Mechanical red** (clippy, fmt, an i18n stamp or extract, a missing
   command icon, an empty filter): fixes it on the pull request's own
   branch and gates the batch again.
5. **Real red** (a test failing on what the change does): `next` has not
   moved and the temporary branch is gone; the tester sends the pull
   request back to its author (or a fixer) with the log, and gates the
   others again without it.
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
| Bench box, narrow | the manifests' jobs (clippy first) | the tester, per batch on a `gate/…` branch |
| `next`'s CI | the whole suite, survival, the commits walk (each merge of `next`, `--first-parent`: a pull request's own fix-up commits are not walked one by one; owner, 2026-11-13), lint, Windows | every push of `next` |
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
- **One watcher** (the coordinating agent) follows CI on `next`: a
  standing watch wakes it on every finished run, so a red run is seen
  within minutes, never hours. **A red `next` comes before everything**
  (owner, 2026-11-13). The merger (the release keeper, docs/releases.md)
  debugs it, as the one who knows what just reached `next` (owner, same
  day); a separate fixing agent starts only if the merger is already on
  another red. Whoever takes it drops everything else; it reproduces the
  failure, adds the test that would have caught
  it, fixes it, runs its box jobs with `--priority high` (one at a time:
  the box keeps one high-priority job waiting per project), and opens its
  pull request into `next` like any change. That pull request is gated
  **alone and first**; no other batch is gated while `next` is red, unless
  it carries the fix. A green `next` is what releases. A bug the owner reports is
  handled the same way: agents reproduce and validate it themselves and
  never ask the owner to test.
- External contributors' pull requests (from forks) target `next` and run
  the full CI as usual.

See also [bench.md](bench.md) for the box's jobs and its rules, and
[releases.md](releases.md) for promotion and tags.
