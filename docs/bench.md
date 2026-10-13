# The bench box

Helios runs its long jobs (full builds, the test suite, timings, the GPU
tests) on a **bench box**: a shared machine that runs each project's jobs
in fresh rootless containers, away from the development machine. The box
is self-service: Helios declares its image, its sources and its jobs in
[`tools/bench/Benchfile`](../tools/bench/Benchfile), and the box runs any
of them on request, inside an envelope of cores, memory, disk and time it
sets for every project.

The box's own documentation (the Benchfile format, its commands, its
degraded modes) lives in the box's repository; this page says what Helios
does with it.

## What runs where

| Where | What |
|-------|------|
| Locally | `cargo xtask check`, `cargo xtask lint`, the tests of the crate you change |
| CI (GitHub Actions) | the full gate on every push and pull request: lint, tests, doctests, survival, check-commits |
| The bench box | the same jobs on demand, on a pushed branch or on uncommitted code, plus timings on an otherwise idle machine and the tests on a real GPU |

Jobs of the Benchfile:

| Job | What it runs | Requests |
|-----|--------------|----------|
| `build` | `cargo build --workspace --all-targets` | 8 cores, 6 GB |
| `test` | `cargo nextest run --workspace --profile ci`, its JUnit report and a summary of the slowest tests in `out/` | 6 cores, 6 GB |
| `survival` | `cargo survival`: quit, `kill -9` and reopen | 4 cores, 4 GB |
| `check-commits` | `cargo xtask check-commits <base>` (`base=`, `origin/main` by default) | 8 cores, 6 GB |
| `gpu` | the GPU backend's tests on the box's NVIDIA GPU; the adapter used in `out/adapter.txt` | 4 cores, 4 GB, the GPU alone |
| `bench-masks` | `cargo xtask bench masks`, `masks.jsonl` in `out/` | 8 cores, 6 GB, the machine alone |
| `test-times` | `cargo xtask test-times --profile ci`, its history kept per branch | 8 cores, 6 GB, the machine alone |
| `raw-corpus` | the raw corpus checks (`helios-corpus`) on the `raw-corpus` volume; JUnit, a summary, the observed values and thumbnails in `out/`; `bless=yes` accepts new thumbnails | 4 cores, 4 GB |

The timing jobs hold the whole machine (`exclusive = ["machine"]`): nothing
else runs meanwhile, so their numbers compare from one run to the next.
Every other job runs without a GPU: the GPU backend and the GUI snapshot
tests use Mesa's lavapipe (software Vulkan), as in CI.

The `raw-corpus` volume holds the decoder regression corpus, the CC0 raws
listed in `tests/raw-corpus.toml`, mounted read-only by the `raw-corpus`
job. `provision raw-corpus branch=<branch>` fills it (with network, through
`tools/bench/fill-corpus.sh`) after the manifest changes; see
[raw-corpus.md](raw-corpus.md).

Each job gets its own checkout of the branch at `/work/helios` and vkdt's
module files at `/work/vkdt` (for the manifest round-trip test). Jobs run
without network: before each job, `cargo fetch --locked` downloads the
crates into a registry cache shared by all branches. The target directory
is a cache too, one per branch, used by one job at a time: a second job on
the same branch waits for the first.

## How to call it

Tell the tooling where the box is, in the environment or in the untracked
`tools/bench/local.env`:

```bash
HELIOS_BENCH_HOST=<host>               # or user@host; the user defaults to bench-helios
HELIOS_BENCH_KEY=~/.ssh/bench-helios   # the default
```

Then:

```bash
cargo xtask bench-remote test                    # the current branch, as GitHub has it
cargo xtask bench-remote build branch=main
cargo xtask bench-remote test --push             # this working tree, uncommitted changes included
cargo xtask bench-remote check-commits base=main
```

`bench-remote` asks the box whether it can serve (`box`), runs the job and
follows its log, then fetches the results to
`target/bench-remote/<id>/` (`log`, `status`, `request`, and the job's
`out/`) and prints a summary: the job, its id, its result and its
duration. The exit code is the job's.

Without `--push`, the box checks the branch out from GitHub: push it first
(`bench-remote` warns when the local branch differs from `origin`). With
`--push`, the tracked and untracked files of the working tree (not the
ignored ones) are sent to the box and the job runs on them; the target
cache used is the current branch's when GitHub has it, else main's. Each
checkout pushes under its own name (`helios-local-<id>`, a hash of its
path), so worktrees can push at the same time; two `--push` runs from the
same checkout still share it.
`check-commits` needs the git history, so it runs on a branch, not on
pushed code.

If the connection drops while a job runs, the job goes on: `bench-remote`
reattaches (`wait <id>`) a few times, and otherwise prints the id to
reattach to later.

The box's commands work directly too:

```bash
ssh -i ~/.ssh/bench-helios bench-helios@<host> box               # can it serve?
ssh -i ~/.ssh/bench-helios bench-helios@<host> list              # recent jobs
ssh -i ~/.ssh/bench-helios bench-helios@<host> fetch <id> > r.tar
ssh -t -i ~/.ssh/bench-helios bench-helios@<host> debug test branch=main   # a shell in the job's container
```

After changing the Benchfile, upload it:

```bash
ssh -i ~/.ssh/bench-helios bench-helios@<host> config benchfile < tools/bench/Benchfile
```

## Exit 75: the box could not serve

The box is a shared machine: it can be off, in maintenance, or without its
GPU. When it cannot serve a job, `bench-remote` prints
`bench box unavailable: <reason>` and exits with **75** (`EX_TEMPFAIL`):

- the box is unreachable (ssh exits 255: off, network down);
- the box is in maintenance;
- the GPU is absent or was lost during the run (the `gpu` job only);
- the box restarted under the job;
- the connection dropped and could not be reattached.

Exit 75 is never a test failure: retry later, or run the job locally or in
CI. Any other non-zero exit is the job's own failure (a build error, a
failing test), and its log is in `target/bench-remote/<id>/log`.
