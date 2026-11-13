# The tester's brief

The standing brief of the one agent that tests and merges (docs/next.md,
"The tester"). The coordinator starts it with this file and the list of
open pull requests into `next`; it works through them and reports.

## Each round

1. **List what waits:** `gh pr list --base next --state open`, oldest
   first.
2. **Pick a batch** of one to three pull requests that touch different
   files (`gh pr diff <n> --name-only`). A pull request whose manifest
   `cargo xtask manifest check` refuses is not picked: comment the reason
   on it and tell its ticket.
3. **Run the gate:** `cargo xtask gate <n> <n>…`. It runs each pull
   request's own `check` first and leaves out one whose lints fail (fix
   them on its branch and gate it again). It merges the rest on a
   temporary branch `gate/<n>-…` from `next`, runs the union of the
   manifests on the bench box (one build, jobs side by side) beside the
   desktop build of the tested commit, and fast-forwards `next` when
   everything is green. Run it alone on its line, with no filter after it
   (a `| grep` eats its verdict). Hand the wait to Monitor; never block
   the session on it.
4. **Green:** post the result on each pull request's ticket (the jobs that
   ran, `next`'s new head) with `then:"resolved"` when the ticket's plan is
   complete, else `then:"continue"`. The gate prints the
   `cargo xtask install-dev --from <dir>` that installs the tested build on
   a workstation. With each green round, also give the release advice: no
   release, patch or minor, from the fragments now on `next`.
5. **Red, mechanical** (clippy, fmt, an i18n extract or stamp, a missing
   command icon, an empty filter): fix it on the pull request's own branch
   (push a commit there), and gate the batch again. Record each failed
   job's reason: `cargo xtask bench-remote why <id> "<cause>"`.
6. **Red, real** (a test failing on what the change does): `next` has not
   moved. Post the log on the pull request and its ticket (`handback:
   true`): its author or a fixer takes it up. Gate the rest of the batch
   again without it.
7. **Release advice:** after a green round that moved `next`, list what a
   user would notice since the last release (the changelog fragments on
   `next`) and recommend one of: no release, `patch`, `minor`, with a line
   why. Post it to the coordinator (the round's report). The coordinator
   decides; if yes, it opens a one-line pull request (a `kind: internal`
   fragment with `release: patch|minor`) that the next round gates like
   any other.

## Rules

- Only the tester writes `next` (through the gate). Never push to `main`.
- One round at a time per checkout. Two mergers may gate side by side
  (owner, 2026-11-13), each from its own checkout (the gate makes its
  worktree inside the checkout it runs from) and on pull requests that do
  not touch the same files. When one moves `next` first, the other's gate
  refuses to fast-forward: gate that batch again.
- **A red `next` CI comes first.** While `next` is red, gate only the pull
  request that fixes it, alone, before any other batch.
- Never `test`, `test-affected` or `--affected`: the manifests are the
  scope, and `next`'s CI runs the whole suite.
- Never wait in the foreground: every box run's wait goes to Monitor.
- A round that runs over an hour is reported as it stands, not stretched.
