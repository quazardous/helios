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
| `next` | each **author** (its own pull request, after its box jobs and its `pr-lint` check), a merger for a backlog (`cargo xtask gate`), and the promotion bot | what passed the box tests and the cheap checks; CI runs on each push |
| `main` | the promotion bot only | what passed `next`'s CI, with its releases |

A pull request is merged only when its tests passed on the box and its
`pr-lint` check is green, so "merged" always means "on `next`".

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
   - The suites that draw what you touched are part of it, not only the
     tests you wrote: `cargo xtask affected origin/next` prints them as
     manifest lines (a change to the Library's sources lists
     `control_names`, `library_filter`, `library_gallery` and
     `library_window`). A suite that went red on `next` because a
     manifest missed it gets its line in `GUI_SUITES`
     (`xtask/src/affected.rs`).
   - A change with nothing to run says `Tests: none — <why>`.
   - **A change to a screen** (a widget, a colour, a font, a layout, a
     text) regenerates the reference images of every snapshot suite it
     touches on the box (`gui-snapshots` for helios-gui's,
     `snapshots-refresh` for every suite, with the images it rewrote under
     `out/snapshots/` at their paths), commits them, and lists those
     suites in its manifest (`test-crate crate=<suite>`), so the images it
     moves move in the same pull request. A suite missed here turns `next`'s
     `release` job red later; the release keeper's `snapshots-refresh`
     step catches it before the release, and names the pull request
     (docs/releases.md, "Snapshots before a release").
     helios-gui's suites cut their pictures to what they test (the
     dialog, the window, the workbench: `tests/it/shot.rs`), so a change
     to the shell's chrome (the title bar, the menu bar, the status bar)
     moves only the `chrome_snapshots` suite's pictures
     (`test=chrome_snapshots`). A new snapshot of the shell uses
     `shot::Shots`, never kittest's `snapshot`, which takes the whole
     window.
   - `cargo xtask manifest check <file>` (or the body on stdin) checks it.
4. **Prove it on the box yourself** (owner, 2026-11-13: the author who
   wrote the change runs its narrowest tests; it beat batching every pull
   request through one gate). The branch's `check`,
   `cargo xtask bench-remote check branch=<branch>` (clippy, every lint
   at once), and the manifest's jobs with `branch=<branch>`, in any
   order: test jobs no longer wait on a check first, and the manifest's
   tests of one crate go in one `test-crate` job (`test=a+b`, see
   bench.md, "Rules for agents"). Wait in the
   foreground with `jbx wait <id>`: a subagent's turn stays alive. Fix
   what is red on the branch and run that job again.
5. **Merge it yourself** when every job of the manifest is green on the
   box **and** the pull request's `pr-lint` check is green (since
   2026-11-15). `pr-lint` (`.github/workflows/pr-lint.yml`) is the one
   GitHub check our own pull requests run, on every push, in a few
   minutes: `cargo xtask lint` with every linter installed (rustfmt,
   clippy, deps-worlds, raw-egui, cargo-deny, cargo-machete, taplo, typos,
   shellcheck, ruff), `cargo doc` without a warning, `cargo xtask
   ui-index --check`, `cargo xtask i18n check`, `cargo xtask check
   --base origin/next` (a changelog fragment) and the English catalogue
   against the declarations. Its two jobs (`linters`, `catalogue`) run side by side and `pr-lint`
   sums them up. Look at it once, when the box jobs are
   done: `gh pr checks <n> --watch --fail-fast` (it returns when the
   check ends), or a single `gh pr checks <n>`; that is all the CI an
   author watches. Red: fix it on the branch, as for a box job. A pull
   request may not add raw egui widgets to helios-gui (`cargo xtask
   raw-egui`; after removing some, `--update` lowers the baseline).
   On a pull request the guards with a backlog (`raw-egui`, `bus`) run
   in their delta mode (`--base origin/next`): they check the merge base
   too and fail only on what the pull request adds, each finding known
   by its file and code, not its line, and print what went down as
   progress ("raw egui calls: 300 → 297 (−3)"); a push to `next` runs
   them in full. Run it locally with `cargo xtask raw-egui --check
   --base origin/next` or `cargo xtask bus --check --base origin/next`.
   The idea comes from darktable's `dt-lockcheck` diff mode
   ([darktable#23056](https://github.com/darktable-org/darktable/pull/23056)). The
   box's `lint` job runs a part of it before a push (clippy,
   `cargo xtask check`, `cargo xtask i18n check`); the linters, the
   documentation and the generated files are checked by `pr-lint` alone. Then
   merge the pull request
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
| Bench box, narrow | the branch's `check` (clippy) and the manifests' jobs | the author, per pull request; a merger, per batch on a `gate/…` branch |
| The pull request's `pr-lint` | the cheap checks, no test but the English catalogue's: every linter, `cargo doc` without a warning, the UI index, the catalogues, the changelog fragment | every push of our own pull request into `next` |
| `next`'s CI | the whole suite, survival, the commits walk (each merge of `next`, `--first-parent`: a pull request's own fix-up commits are not walked one by one; owner, 2026-11-13), lint, the Windows build | every push of `next` |
| `windows-tests` workflow | the workspace's tests on Windows | nightly on `next` (02:30 UTC), and on manual dispatch |
| `ci-history` workflow | no test: the JUnit reports of `next`'s runs folded into the per-test history, the `ci-history` branch ([ci-broker.md](ci-broker.md)) | nightly (03:45 UTC), and on manual dispatch |
| Bench box, wide | `test-affected`, `test`, GPU, timing, `stability` | daily, before a release, on demand |
| Bench box, `journey` | the first-user journey: a fresh profile's wizard, a card imported, the Library's count, the import session, the health fixes, Settings › Storage, quit and restart, a photo opened in the darkroom and exported, headless ([bench.md](bench.md)) | every push of `next` (queued by the watcher of `next`), and before a release |

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
- Helios `next` builds on egui-pop's and egui_hud's `next` by git
  revision. When either turns green, the coordinator runs `cargo xtask
  pin-bump --pr`, which opens the one pull request moving the pins; nobody
  writes pin pull requests by hand, and no slice waits for an egui-pop
  release. A release takes their published versions instead: the release
  keeper cuts them first ([releases.md](releases.md), "egui-pop and
  egui_hud: next follows next").
- External contributors' pull requests (from forks) target `next` and run
  the full CI as usual.

See also [bench.md](bench.md) for the box's jobs and its rules, and
[releases.md](releases.md) for promotion and tags.
