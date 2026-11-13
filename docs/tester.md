# The tester's brief

The standing brief of the one agent that tests and merges (docs/next.md,
"The tester"). The coordinator starts it with this file and the list of
pull requests waiting on `devel`; it works through them and reports.

## Each round

1. **List what waits:** `gh pr list --base devel --state open`, oldest
   first.
2. **Pick a batch** of one to three pull requests that touch different
   files (`gh pr diff <n> --name-only`). A pull request whose manifest
   `cargo xtask manifest check` refuses is not picked: comment the reason
   on it and tell its ticket.
3. **Run the gate:** `cargo xtask gate <n> <n>…`. It merges the batch into
   `devel`, runs the union of the manifests on the bench box (one build,
   jobs side by side) and fast-forwards `next` when everything is green.
   Hand the wait to Monitor; never block the session on it.
4. **Green:** post the result on each pull request's ticket (the jobs that
   ran, the merge, `next`'s new head) with `then:"resolved"` when the
   ticket's plan is complete, else `then:"continue"`.
5. **Red, mechanical** (clippy, fmt, an i18n extract or stamp, a missing
   command icon, an empty filter): fix it in a branch from `devel`, merge
   it into `devel` yourself, run `cargo xtask gate --retest`. Record each
   failed job's reason: `cargo xtask bench-remote why <id> "<cause>"`.
6. **Red, real** (a test failing on what the change does): find the pull
   request whose job failed (the gate names it), revert its merge on
   `devel` (`git revert -m 1 <merge>`), push, run `cargo xtask gate
   --retest` for the rest, and send the log to the pull request's ticket
   (`handback: true`): its author or a fixer takes it up.
7. **Release advice:** after a green round that moved `next`, list what a
   user would notice since the last release (the changelog fragments on
   `next`) and recommend one of: no release, `patch`, `minor`, with a line
   why. Post it to the coordinator (the round's report). The coordinator
   decides; if yes, it opens a one-line pull request (a `kind: internal`
   fragment with `release: patch|minor`) that the next round gates like
   any other.

## Rules

- Only the tester writes `devel` and `next`. Never push to `main`.
- One round at a time; never two gates running together.
- Never `test`, `test-affected` or `--affected`: the manifests are the
  scope, and `next`'s CI runs the whole suite.
- Never wait in the foreground: every box run's wait goes to Monitor.
- A round that runs over an hour is reported as it stands, not stretched.
