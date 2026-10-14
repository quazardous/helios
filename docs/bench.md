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
| `lint` | fast compile feedback: `cargo clippy --workspace --all-targets -- -D warnings`, `cargo xtask check`, then `cargo xtask i18n check` (its report in `out/i18n.txt`) | 8 cores, 6 GB |
| `test` | `cargo nextest run --workspace --profile ci`, its JUnit report and a summary of the slowest tests in `out/` | 6 cores, 6 GB |
| `test-affected` | the tests a change touches since `base=` (`origin/main` by default) and the critical set: see "Running what a change touches"; the selection in `out/affected.json`, the JUnit report and the slowest tests in `out/` | 6 cores, 6 GB |
| `survival` | `cargo survival`: quit, `kill -9` and reopen | 4 cores, 4 GB |
| `release` | `cargo xtask release` (survival first), Cargo.lock refreshed; the change as `out/release.patch` (see CONTRIBUTING.md) | 5 cores, 6 GB |
| `build-gui` | `helios-gui` in release, kept as the output `gui`; `tools/bench/fetch-gui.sh [branch]` fetches the latest into `target/box/helios-gui`, which `packaging/linux/install.sh --dev --box` launches | 5 cores, 6 GB |
| `quit-timing` | `helios-gui` in release in a headless labwc (wbox), the raw corpus as its library, closed `runs=` times by Ctrl+Q and by the compositor; each step of the quit it marks (`HELIOS_QUIT_TRACE`), when its window went and when it exited, in `out/quit-timing.md`; `budget=<ms>` fails a window slower to go | 5 cores, 6 GB |
| `masks-shots` | `helios-gui` in release in a headless labwc (wbox) at 1920×1080, the raw corpus's first image, driven through the command palette in each theme (`themes=`): the empty mask, then two parametric members and two shapes; the screenshots and each step's answers in `out/` (`tools/bench/masks-shots.py`) | 5 cores, 6 GB |
| `check-commits` | `cargo xtask check-commits <base>` (`base=`, `origin/main` by default): each commit but the head builds, passes clippy and the tests it touches; `full=yes` runs every fast test on each; the per-commit report in `out/check-commits.md` | 8 cores, 6 GB |
| `gpu` | the GPU backend's tests on the box's NVIDIA GPU; the adapter used in `out/adapter.txt` | 4 cores, 4 GB, the GPU alone |
| `bench-masks` | `cargo xtask bench masks`, `masks.jsonl` in `out/` | 8 cores, 6 GB, the machine alone |
| `test-times` | `cargo xtask test-times --profile ci`, its history kept per branch | 8 cores, 6 GB, the machine alone |
| `mask-parity` | darktable's drawn masks against darktable-cli, built from a pinned upstream commit (see [masks.md](masks.md), "Parity with darktable"); JUnit, a summary and the masks in `out/` | 8 cores, 8 GB |
| `raw-corpus` | the raw corpus checks (`helios-corpus`) on the `raw-corpus` volume; JUnit, a summary, the observed values and thumbnails in `out/`; `bless=yes` accepts new thumbnails | 4 cores, 4 GB |

The timing jobs hold the whole machine (`exclusive = ["machine"]`): nothing
else runs meanwhile, so their numbers compare from one run to the next.
Every other job runs without a GPU: the GPU backend and the GUI snapshot
tests use Mesa's lavapipe (software Vulkan), as in CI.

The `raw-corpus` volume holds the decoder regression corpus, the CC0 raws
listed in `tests/raw-corpus.toml`, mounted read-only by the `raw-corpus`
and `mask-parity` jobs. `provision raw-corpus branch=<branch>` fills it
(with network, through `tools/bench/fill-corpus.sh`) after the manifest
changes; see [raw-corpus.md](raw-corpus.md).

Each job gets its own checkout of the branch at `/work/helios` and vkdt's
module files at `/work/vkdt` (for the manifest round-trip test), and
darktable at the pinned commits the mask parity builds (`/work/darktable`,
`/work/darktable-flexi`, with `/work/rawspeed`, `/work/whereami` and
`/work/libraw`, its submodules); the darktable-cli builds are kept in the
`darktable-build` storage, one per checkout, until the pin moves. Jobs run
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
cargo xtask bench-remote check-commits base=main full=yes
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

After changing the Benchfile on a branch, upload it under that branch's
name. Jobs run with `branch=<branch>` then use it, and every other branch
keeps the default Benchfile:

```bash
ssh -i ~/.ssh/bench-helios bench-helios@<host> config benchfile <branch> < tools/bench/Benchfile
```

The default Benchfile is the one on `main`. Upload it, with no branch name,
only from `main`, once the change is merged:

```bash
ssh -i ~/.ssh/bench-helios bench-helios@<host> config benchfile < tools/bench/Benchfile
```

## Running what a change touches

A branch rarely needs the whole suite: `cargo xtask affected` says what a
change can break, and the box runs that.

```bash
cargo xtask affected                    # since origin/main: JSON on stdout, a summary on stderr
cargo xtask affected main --filter      # the nextest filter alone
cargo xtask affected --critical         # the critical set's filter alone
cargo nextest run --workspace -E "$(cargo xtask affected --filter)"
cargo xtask bench-remote test-affected base=origin/main
cargo xtask bench-remote --affected     # every job the selection names, in turn
```

How the selection is made:

1. **The changed files**: everything since the merge base of the base
   and `HEAD`, uncommitted and untracked files included.
2. **Their crates**: each file belongs to the workspace crate whose
   directory holds it (`cargo metadata`, nothing built). Data outside the
   crates goes to the crates that read it: `i18n/` to the translation
   crates, `tests/masks/` to the parity harness, the raw corpus manifest
   and its thumbnails to `helios-corpus`, `packaging/` to the desktop
   (its icon), the example plugins to the plugin broker and the
   catalogues. Files no test reads (`docs/`, `changelog.d/`, the README
   and other top-level texts, the linters' settings, `proto/`, the
   research scripts of `tools/`) select no crate.
3. **Their dependents**: every crate that depends on a changed one,
   directly or not, by any kind of dependency (normal, dev, build).
4. **The critical set**, always: a smoke render (the engine renders a
   chain, an image exports to every format), a load and save round trip
   (an edit saved step by step and reopened at its head, a vault reopened
   with its images and history) and survival (the desktop and the node
   quit, killed with `kill -9`, reopened). It is a nextest filter, kept in
   `xtask/src/affected.rs` (`CRITICAL`) with a unit test checking each of
   its tests still exists.
5. **The image-data jobs**, by what they read: `raw-corpus` when the
   loader (`helios-load`), the camera description it hands out
   (`helios-photo`), the corpus checks, the manifest, the blessed
   thumbnails or the fill script change; `mask-parity` when the masks
   module, the XMP import (`helios-xmp`), the parity harness, the
   fixtures or the darktable build script change. These follow direct
   changes only: a change to `helios-core` tests every crate, but does
   not run the corpus.

**Everything runs** (the full `test` and `survival` jobs, both data jobs,
`all()` as the filter) when the workspace manifest, `Cargo.lock`, the
toolchain, xtask, CI (`.github/`), the nextest or cargo configuration, or
the Benchfile changes, and when a path matches no rule: a new top-level
file is never skipped by accident.

The box jobs a selection names come in its `box_jobs`: `lint` always,
`test-affected` (or `test` and `survival` for everything), then the data
jobs. `bench-remote --affected` runs them one after the other on the
current branch (or with `--push`), passes `base=` to `test-affected`, and
prints a table of their results; it exits with the first failure, else
75 when the box could not serve a job.

The full suite still runs on `main` after every merge (CI's gate): a
selection is a shortcut for a branch, not the last word.

### Each commit of a series

`cargo xtask check-commits` (and the `check-commits` job) uses the same
selection commit by commit, so a series stays bisectable without paying
the whole suite on each commit. Every commit of the range but its head
(the full gate runs on the head) is checked out in turn and goes
through:

1. `cargo build --workspace --all-targets`;
2. `cargo clippy --workspace --all-targets -- -D warnings`;
3. nextest on the commit's own selection: its diff against its first
   parent, mapped on its own workspace (`cargo metadata` at that
   commit), gives the crates it touches and their dependents; their
   fast tests run (nextest without the parity tests, the headless GUI
   tests and the masks stress test), plus the critical set. A commit
   that only touches files no test reads runs the critical set alone.
   A commit that changes a path that runs everything (`Cargo.lock`,
   xtask, CI, the Benchfile, a path no rule knows), or a root commit,
   runs every fast test and the critical set;
4. `cargo xtask check`.

`--full` (`full=yes` on the box) runs every fast test on every commit
instead, as before the selection existed. The walk stops at the first
broken commit. The report, printed at the end, appended to
`GITHUB_STEP_SUMMARY` in CI and written to `out/check-commits.md` on the
box, has one line per commit: what it ran and why (the crates it
changed and how many dependents, or the paths that asked for
everything), its result and its time.

### Reproducibility

What a run depends on, so two runs can be compared:

- **The build profile.** Tests build in the dev profile, with the
  optimization levels `Cargo.toml` sets per package (the decoders, the
  engine, the raw chain, the parity harness); the box adds no incremental
  state and line tables only. The `mask-parity` binary builds in release.
  Rust neither reorders nor fuses floating-point operations at any
  optimization level, so the profile changes speed, not results.
- **No fast-math on Helios's side.** No `RUSTFLAGS`, no
  `-C target-cpu=native`, no fast or algebraic float intrinsics in the
  code a parity job runs: its numbers must not depend on the machine.
  darktable, the reference, builds with its own release flags (which
  include `-ffast-math`) at pinned commits, on the box's image: its
  output moves only when a pin or the image's compiler does, and the
  thresholds absorb what fast-math changes in its last bits.
- **The data packs.** The raw corpus is a volume of CC0 files checked
  against the sha256 of `tests/raw-corpus.toml`; the mask fixtures and
  darktable's harvested masks are in `tests/masks/`; vkdt and darktable
  are checkouts at pinned commits. A refilled volume or a moved pin is a
  new baseline: say so in the commit.
- **The comparison.** `raw-corpus` and `mask-parity` judge images with
  one function (`helios_photo::compare`): identical decoded pixels pass
  at once; otherwise the mean, the 99.9th and 99.99th percentiles and the
  max of the Delta E 2000 are held to the job's thresholds together.
- **Known non-deterministic tests.** None are known to fail at random.
  Some depend on time and stay within generous bounds: the platform and
  exit tests of `helios-gui` poll with short sleeps, the survival
  scenarios kill a child process at a moment it reports, and the GPU
  tests run on whatever adapter the machine has (lavapipe on the box but
  for the `gpu` jobs), so their timings, not their results, vary. A test
  seen failing once without a cause is a bug to file, not a retry to add:
  the `ci` profile retries nothing.

## Rules for agents (a shared box)

Several agents share one box, one queue and one disk quota. A job that hangs
or hoards blocks everyone for the length of its timeout. These rules come
from incidents on the box; every agent prompt points here.

**Before queuing**
- Run the narrowest job first: `test-affected` (or one test) before `test`,
  `survival` or `check-commits`. A full run is the last step, not the first.
- A new test has a deadline on every wait (a child process, a window, a
  file, a socket reply) and fails fast. Tests run under nextest, which stops
  a test after 120 s; `cargo survival` runs under nextest too.
- After a job hangs or times out (exit 124), find why before queuing it
  again: the same run will block the box for the same time.
- A job of yours that hangs or is no longer wanted: stop it with
  `ssh … cancel <id>` (or `cancel --branch <branch>` for all of a branch's
  jobs) rather than letting it run to its timeout; its log and `out/` are
  kept. A timeout now stops the container too.
- One job of yours per branch at a time; never push to a branch while a job
  of yours on it is queued.

**Waiting**
- Wait with one blocking `ssh … wait <id>` per job. Never poll in a loop,
  and never `until ! pgrep -f "<pattern>"`: the waiter's own command line
  holds the pattern, so it never ends (the same for `pkill -f`).
- Pipe `bench-remote` only under `set -o pipefail`; a `| grep` turns its
  exit 75 into 0.
- Name logs after the branch: the scratchpad is shared.

**Space**
- Never delete box storage you did not create. After a branch with its own
  Benchfile merges, `config benchfile rm <branch>`: each live label keeps
  its image for 14 days.
- egui-pop's targets live on the hard disk (a gate needs 25 to 35 GB);
  Helios's targets, sccache and the images on the SSD (60 GB quota).
- Exit 75 with "needs about N GB": wait for space; do not free it by
  deleting others' storage.

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
