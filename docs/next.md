# The `next` protocol

How every change reaches Helios, for people and agents alike. The same
protocol holds in egui-pop, egui_stack, egui_fit and egui_hud.

`next` is the merge zone. Changes land there without having run the whole
suite, and the full CI runs once per push of `next`, never once per branch
or pull request. No one waits for tests that do not concern their change,
and the same suite never runs several times over. `main` only ever moves
by the promotion of a green `next`.

## For a change (an agent's whole job)

1. **Branch from `origin/next`,** in a worktree of its own, building into
   its own `target/` (`env -u CARGO_TARGET_DIR`). Never from `main`; never
   `git stash`.
2. **Code with light local checks only:** `cargo fmt`, `cargo check --tests`
   on the crates touched (`CARGO_INCREMENTAL=0`), `typos`, and
   `cargo update -w --offline` (commit `Cargo.lock` if it changed).
   No local tests, no local clippy, no release build: a local hook refuses
   them. Changelog fragments: at most 600 characters, for user-visible
   changes only.
3. **Test only your own part, on the bench box:**
   `cargo xtask bench-remote test-crate crate=<crate> test=<target> filter=<name>`
   for Helios, `egui-pop-test` for egui-pop, on the crates and targets the
   change touched. **Never `test-affected`, `--affected` or `test` before a
   merge:** they reach nearly the whole suite, which `next`'s CI runs anyway.
   The box runs `check` and clippy before any test job, so the narrow job
   lints the part too. One blocking wait per job, with a deadline; never end
   a session to "wait for a notification".
4. **Every failed box job gets its reason** at once:
   `bench-remote why <id> "<cause, and what is done about it>"`. A
   reproduction meant to fail runs with `expect=fail`.
5. **Rebase on `origin/next`, check the diff, merge, stop.**
   - `git diff --stat origin/next..HEAD` must list only the files the change
     meant to touch (a squash done with `git reset --soft` onto a newer
     `next` once reverted a whole merged change).
   - Open a pull request into `next` and merge it as soon as the narrow box
     jobs passed (`gh pr merge --merge`; egui_stack squashes). Our own pull
     requests run no CI: there is nothing to wait for.
   - Delete the branch (local and remote), the worktree and its `target/`.
   - **Never push to `main`.**
6. **Do not wait for CI.** Once the change is on `next`, the work is done.

Sizing: one agent, one pull request, one small slice, under an hour. A
slice that runs over lands what is done and writes the rest down.

## What tests run where

| Where | What | When |
|---|---|---|
| Local | fmt, check, typos | while coding |
| Bench box, narrow | `test-crate` / `egui-pop-test` on the part touched (clippy first) | before the merge |
| `next`'s CI | the whole suite, survival, the commits walk, lint, Windows | every push of `next` |
| Bench box, wide | `test-affected`, `test`, GPU, timing, `stability` | daily, before a release, on demand |

CI looks for regressions and backward-compatibility breaks. Load, soak and
stress runs (`stability`) never go into CI.

## What happens on `next`

- Every push runs the whole CI. Runs are not cancelled: the newest push
  waits behind the run going, and each green run is promoted.
- When CI passes, `promote.yml` promotes that exact commit: with
  user-facing fragments it commits "Release X.Y.Z" on top of it (version,
  CHANGELOG, Cargo.lock), then fast-forwards `main`, and `next` if it has
  not moved. Expect bot commits on `next`: rebase before merging.
- **One watcher** (the coordinating agent) follows CI on `next`. A red run
  gets **one fixing agent first**, before any other work: it reproduces the
  failure, adds the test that would have caught it, fixes it, and merges a
  follow-up into `next`. A bug the owner reports is handled the same way:
  agents reproduce and validate it themselves and never ask the owner to
  test.
- External contributors' pull requests (from forks) run the full CI as
  usual.

See also [bench.md](bench.md) for the box's jobs and its rules, and
[releases.md](releases.md) for promotion and tags.
