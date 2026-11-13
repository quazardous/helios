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
| Locally | `cargo fmt`, `cargo check --tests`, `cargo xtask check`; never clippy (the box lints first in every test job) |
| CI (GitHub Actions) | the full gate on every push to `next` and `main`, and on contributors' pull requests from forks: lint, tests, doctests, survival, check-commits. Our own pull requests run none: the box tests them and `next`'s run decides |
| The bench box | the same jobs on demand, on a pushed branch or on uncommitted code, plus timings on an otherwise idle machine and the tests on a real GPU |

Jobs of the Benchfile:

| Job | What it runs | Requests |
|-----|--------------|----------|
| `build` | `cargo build --workspace --all-targets` | 8 cores, 6 GB |
| `lint` | fast compile feedback: `cargo clippy --workspace --all-targets -- -D warnings`, `cargo xtask check`, then `cargo xtask i18n check` (its report in `out/i18n.txt`) | 8 cores, 6 GB |
| `test` | `cargo nextest run --workspace --profile ci`, its JUnit report and a summary of the slowest tests in `out/` | 6 cores, 6 GB |
| `test-crate` | a fix loop's run: `crate=` (required), `test=` (one test target, `test=lib` for the crate's unit tests) and `filter=` (a name filter, a test path such as `filter=undo::tests::redo`, or several names separated by spaces), stopped at the first failure, in the branch's target | 4 cores, 4 GB |
| `test-affected` | the tests a change touches since `base=` (`origin/main` by default), or the `crates=` given (as `--push` does), and the critical set: see "Running what a change touches"; the selection in `out/affected.json`, the JUnit report and the slowest tests in `out/` | 6 cores, 6 GB |
| `survival` | `cargo survival`: quit, `kill -9` and reopen | 4 cores, 4 GB |
| `stability` | `cargo stability`: the headless node (`helios-node --vault`) killed with `kill -9` in the middle of its own jobs (a backup copying, an import, a health run, a settings layer write), then started again on the same vault; and many clients on one node at once (`clients=`, 32 by default, for `seconds=`, 60 by default, `seed=`): each a real client over the node's socket, mixing reads, saves, stacks made and undone, imports and cancelled imports, disconnections, while subscribers follow the bus, then the vault checked against every acknowledged write; JUnit and a summary in `out/`, where each kill landed in the log, the stress report (calls per second, p50 and p99 latency, errors, the node's memory, threads and descriptors) in `out/many-clients.json`. Not run by CI, which is for compatibility breaks: run it on demand and before a release (the tests sit behind the `helios-node` feature `stability`, which no CI run enables) | 4 cores, 4 GB |
| `soak` | `cargo soak`: a moderate mix of clients (`clients=`, 8 by default, each pausing between calls) on one headless node for `minutes=` (60 by default, `seed=`), the same scenario as the many-clients run, the node sampled every `every=` seconds (30): memory, threads, descriptors, descriptors open on the vault's files, the size of its write-ahead log, what the caches hold, the bus's subscribers and journal, and how far its subscribers lag. Once warmed up, no measure may grow (the last third of the samples against the first, with a tolerance); no call may pass its 30 s deadline; the vault must hold exactly the acknowledged writes; the node must stop cleanly on SIGTERM. The series and each measure's trend in `out/soak.json`, a summary in the log. Not run by CI: on demand and before a release; a six-second variant is a normal test | 4 cores, 4 GB |
| `release` | the fallback when GitHub's CI is down (promotion releases otherwise): `cargo xtask release` (survival first), Cargo.lock refreshed; the change as `out/release.patch` (see CONTRIBUTING.md) | 5 cores, 6 GB |
| `i18n-extract` | the translation catalogues regenerated on a branch: `cargo xtask i18n extract`, then `i18n stamp fr`, then `i18n check` (its report in `out/i18n.txt`); the change as `out/i18n.patch`, applied on the branch with `git apply --index` and committed there | 5 cores, 6 GB |
| `dt-asan-rgbcurve` | darktable at `dt_ref=<sha>` (the fork) built with AddressSanitizer (recovering) and Lua, then the rgbcurve scroll scenario (`tools/bench/rgbcurve`: a wheel step away from every node on R, and a wheel with the zoom key held over a node) in a headless labwc (wbox); per case the history before and after, the ASan reports and screenshots in `out/<case>/`, `out/SUMMARY.txt` | 8 cores, 10 GB |
| `flatpak` | the Flatpak bundle (`packaging/flatpak/build.sh`) built from source with flatpak-builder, as the release workflow builds it, into `out/`, then installed and smoke-tested (`packaging/flatpak/smoke.sh`); the runtime, the SDK and the downloaded sources stay in the `flatpak` storage | 5 cores, 8 GB |
| `build-gui` | `helios-gui` in release, kept as the output `gui`, with the programs of the plugins Helios ships (the outputs `darktable-library`, `lightroom-catalog`, `google-photos`); `cargo xtask install-dev [--branch B]` installs the latest (`--from DIR` the outputs a gate brought back) outside the checkout (`helios-gui` and the programs beside it) with the "Helios Dev" launcher (see CONTRIBUTING.md, "A dev build on the desktop") | 5 cores, 6 GB |
| `egui-pop-gallery` | egui-pop's gallery in release (`egui_pop=` picks the branch), kept as the output `gallery`; `tools/bench/fetch-gallery.sh [branch]` fetches the latest into `target/release/egui-pop-gallery`, where the gallery's launcher points | 5 cores, 6 GB |
| `egui-pop-wbox` | egui-pop's gallery in release in a headless labwc (wbox), driven by a scenario of egui-pop's `tools/wbox` (`script=`, `titlebar.py` by default: hover after a move by the gallery's own title bar); its JSON lines in the log, its screenshots in `out/`. `compositor=mutter` runs the same scenario in a headless mutter (GNOME's compositor, egui-pop's `tools/wbox/mutter.py`: input through its RemoteDesktop D-Bus API, frames through ScreenCast and PipeWire), for bugs seen only on GNOME | 5 cores, 6 GB |
| `egui-pop-perf` | a timing job, after `egui-pop-perf-build` (its `first`, which builds it): egui-pop's performance gate, the gallery's `perf` example headless and deterministic (fixed frames, fixed input) on six heavy pages (`pages=` picks some, comma-separated): frame build and tessellation times, vertices, allocations, the heap's peak, one JSON line per page in `out/perf.jsonl`. Each full run is kept per egui-pop branch in the `egui-pop-perf` storage (`latest.jsonl`, `history.jsonl`) and compared with next's latest: a figure beyond its threshold is a warning in `out/regressions.txt`, never a failure. Run it on egui-pop's `next` after a merge to move the baseline | 4 cores, 4 GB, timing (the machine alone) |
| `quit-timing` | `helios-gui` in release in a headless labwc (wbox), the raw corpus as its library, closed `runs=` times in turn by Ctrl+Q, the compositor, SIGTERM and SIGINT, then once by SIGKILL; each step of the quit it marks (`HELIOS_QUIT_TRACE`), when its window went and when it exited, in `out/quit-timing.md`; `budget=<ms>` fails a window slower to go; a crash marker left by a close (none left by SIGKILL) fails the run | 5 cores, 6 GB |
| `import-close` | `helios-gui` in release in a headless labwc (wbox), the import window opened on the raw corpus and closed by each way in turn (`ways=`: cancel and compositor by default, escape, import, joined by `+`), beside the main window, then full screen over it (`covers=`: shown, covered), `runs=` times; from each close, the main window's next frame (`HELIOS_FRAME_TRACE`), when the import window went, the agent socket's next answer and the main thread's waits meanwhile, in `out/import-close.md`; `budget=<ms>` fails a close after which the main window draws later (`tools/bench/import-close.py`) | 5 cores, 6 GB |
| `window-audit` | `helios-gui` in release in a headless labwc (wbox), the import window opened on the raw corpus; from it, the folder picker, Escape, the command palette and a compositor close with the picker open, each checked to land in the import window (the multi-window checklist of `docs/design/windows.md`), in `out/window-audit.md` with a screenshot per probe (`tools/bench/window-audit.py`) | 5 cores, 6 GB |
| `start-timing` | `helios-gui` in release in a headless labwc (wbox), launched `runs=` times in turn bare (a workspace alone, as the launcher starts it), on the raw corpus's vault and on its first raw (`kinds=`, joined by `+`); each step of the start it marks (`HELIOS_START_TRACE`), its first frame presented and its first input answered (the script moves the pointer over it), from the process's start, in `out/start-timing.md`; `budget=<ms>` and `interactive=<ms>` fail a run slower to its first frame or to interactive | 5 cores, 6 GB |
| `masks-shots` | `helios-gui` in release in a headless labwc (wbox) at 1920×1080, the raw corpus's first image, driven through the command palette in each theme (`themes=`): the empty mask, then two parametric members and two shapes, then the mask display on the selected shape (`M`, then grey from the palette); the screenshots and each step's answers in `out/` (`tools/bench/masks-shots.py`). `doc=yes` draws the two masks pictures of [gui.md](gui.md) instead, at 1280×800 in the default theme on a synthetic picture: `out/masks-wbox.png` and `out/mask-tree-wbox.png`, to copy into `docs/gui/` | 5 cores, 6 GB |
| `scenarios` | Helios's scenarios (`crates/helios-gui/tests/scenarios/*.ron`) replayed live: `helios-gui` in release in a headless labwc (wbox), each scenario on a throwaway library with a folder of known photos, driven over its agent socket by the `scenarios` example (egui-pop's scenario runner), the invariants checked after every step; the shots, journals, failure reports and helios-gui's logs in `out/`, one line per scenario in `out/scenarios.jsonl`; `only=<name>[,<name>]` picks scenarios (`tools/bench/scenarios.py`) | 5 cores, 6 GB |
| `gui-snapshots` | helios-gui's snapshot tests (the import window's, or `test=<target>`'s) with `UPDATE_SNAPSHOTS=1`, on lavapipe (`update=force` rewrites every snapshot, not only those past their threshold); every PNG of `crates/helios-gui/tests/snapshots` in `out/snapshots/`, to review and commit | 5 cores, 6 GB |
| `check-commits` | `cargo xtask check-commits <base>` (`base=`, `origin/main` by default): each commit but the head builds, passes clippy and the tests it touches; `full=yes` runs every fast test on each; the per-commit report in `out/check-commits.md` | 8 cores, 6 GB |
| `gpu` | the GPU backend's tests on the box's NVIDIA GPU; the adapter used in `out/adapter.txt` | 4 cores, 4 GB, the GPU alone |
| `bench-masks` | `cargo xtask bench masks`, `masks.jsonl` in `out/` | 8 cores, 6 GB, the machine alone |
| `thumb-bench` | a timing job (the box's cap and cool-down apply), run after `thumb-bench-build`, which builds it: the thumbnail benchmark (`helios-gui`'s `thumb_bench` example, see [performance.md](performance.md)): per raw of the `raw-corpus` volume its embedded previews' cost, a 256 px cache entry's size and a 256 px render of its edit, then the library grid with the corpus repeated to each of `sizes=` images (1,000, 10,000 and 50,000 by default): first screen, scroll frame times, peak RSS, textures; `out/thumb-bench.jsonl` | 8 cores, 8 GB, timing (the machine alone) |
| `test-times` | `cargo xtask test-times --profile ci`, its history kept per branch | 8 cores, 6 GB, the machine alone |
| `mask-parity` | darktable's drawn masks against darktable-cli, built from a pinned upstream commit (see [masks.md](masks.md), "Parity with darktable"); JUnit, a summary and the masks in `out/` | 8 cores, 8 GB |
| `raw-corpus` | the raw corpus checks (`helios-corpus`) on the `raw-corpus` volume; JUnit, a summary, the observed values and thumbnails in `out/`; `bless=yes` accepts new thumbnails | 4 cores, 4 GB |
| `raw-develop` | a few raws of the `raw-corpus` volume and darktable's integration images opened in "Develop one image" as the desktop opens them (helios-gui's `develop_raw` tests), the middle of each rendered and compared with the node's raw path; JUnit in `out/` | 5 cores, 6 GB |
| `raw-parity` | a Bayer CR2, a Bayer NEF and an X-Trans RAF of the `raw-corpus` volume developed by "Develop one image" at its defaults (`helios-parity`'s `raw-parity`), against darktable-cli (the classic pin, the `darktable-build` mask-parity keeps) running the same modules (judged: darktable's thresholds, max at the 99.9th percentile) and its own defaults (measured); `summary.md`, `results.json`, JUnit and a side-by-side PNG per raw in `out/shots/`; `only=<id>` picks one raw (see [raw-development.md](raw-development.md)) | 6 cores, 8 GB |

The timing jobs hold the whole machine (`exclusive = ["machine"]`): nothing
else runs meanwhile, so their numbers compare from one run to the next.

The other test jobs share the box (`test`, `test-crate`, `test-affected`,
`release`): they run beside other jobs on a machine that may throttle, so a
frame-time budget there measures the load, not the code. Their Benchfile
entries set `HELIOS_SHARED_MACHINE=1`, and a test asserting a frame time
only warns under it (the rest of the test still asserts). The budget is
enforced where the machine is not shared: on CI, in `test-times` (the
whole suite, the machine alone) and in any local run. A frame-time test
reads the variable through a helper, as the Graph panel's
`within_budget`, and its name matches the nextest `alone` group
(`stays_fluid`, `frame_budget`, `frames_stay`).
Every other job runs without a GPU: the GPU backend and the GUI snapshot
tests use Mesa's lavapipe (software Vulkan), as in CI.

The `raw-corpus` volume holds the decoder regression corpus, the CC0 raws
listed in `tests/raw-corpus.toml`, mounted read-only by the `raw-corpus`,
`raw-develop`, `raw-parity` and `mask-parity` jobs (and the GUI timing and screenshot
jobs). `provision raw-corpus branch=<branch>` fills it
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
cargo xtask bench-remote queue                   # the box's queue: what runs, what waits and why
```

`bench-remote` asks the box whether it can serve (`box`), runs the job and
follows its log, then fetches the results to
`target/bench-remote/<id>/` (`log`, `status`, `request`, and the job's
`out/`) and prints a summary: the job, its id, its result and its
duration. The exit code is the job's. The results folder is the
`target/` of the checkout the command runs in (the git toplevel of the
current directory), whatever `CARGO_TARGET_DIR` says, so each worktree
finds its own jobs' results.

`help`, `--help` or `-h` anywhere prints the client's usage and queues
nothing. A job name the checkout's `tools/bench/Benchfile` does not
declare is refused here, with the list of its jobs, before anything goes
to the box.

Without `--push`, the box checks the branch out from GitHub: push it first
(`bench-remote` refuses to queue a branch origin does not have, or the
current branch while it has commits origin lacks). Before queueing it also
checks, when the box runs this checkout's code, that Cargo.lock is up to
date and that the changelog fragments are well formed (at most 600
characters): a few seconds here instead of a failed job on the box.
It also refuses a job missing a parameter its command needs. Before any job
that builds, the box itself runs the short `check` job (`egui-pop-check` for
egui-pop's jobs), declared as the job's `first` in the Benchfile and run
once per commit: `cargo clippy --tests --keep-going` on the crate the job
names, else the workspace, with no code generated and no link. Lints are
warnings rather than errors, so one pass shows every lint of every crate,
dependents and tests included; any warning or compile error makes the
check red. The job itself is queued only if that check passes:
**a compile error never takes a test job's place** (owner, 2026-11-08). The
check also warms the branch's target for the job that follows.
`first=none` on a run skips it; `HELIOS_BENCH_LOCAL_CHECK=1` also runs the
same `cargo check` here first. `HELIOS_BENCH_NO_PREFLIGHT=1` queues it anyway; never for a
compile error.

**Every failed job has a reason.** A failed job is recorded in
`$XDG_STATE_HOME/helios-bench/failures.jsonl` with its failing lines, and
`bench-remote` asks for the cause: the agent that queued it records it at
once with `cargo xtask bench-remote why <id> "<cause, and what is done
about it>"`. `cargo xtask bench-remote failures` lists the ones still
unexplained (`--all` for every one). A run meant to fail (a reproduction
before a fix) says so with `expect=fail` and is recorded as expected. With
`--push`, the tracked and untracked files of the working tree (not the
ignored ones) are sent to the box and the job runs on them, with
`branch=` the current branch (a detached HEAD is refused: give
`branch=<branch>` or check a branch out). The box still checks that branch
out from GitHub (its Benchfile, its target cache), so a branch origin
lacks is pushed there first (`git push -u origin <branch>`). The box's own
`check` before the job gets `src=` too (each `first` passes it), so it
type-checks the code sent, not the branch as GitHub has it. Each
checkout pushes under its own name (`helios-local-<id>`, a hash of its
path), so worktrees can push at the same time; two `--push` runs from the
same checkout still share it.
`check-commits` needs the git history, so it runs on a branch, not on
pushed code. `test-affected` needs it too, to diff against its base: with
`--push`, `bench-remote` computes the selection here, where git is, and
passes it as `crates=` (see "Running what a change touches").

Once the box has queued the job, `bench-remote` prints one line on the
box (its capacity, what running jobs hold, what this job asks for) and,
while the job waits, its place in the queue: its position, the jobs ahead,
why it waits and when it should start. The box is asked at most once a
minute and a line is printed only when it changes; nothing more once the
job runs. A box that does not answer these questions changes nothing to
the run.

If the connection drops while a job runs, the job goes on: `bench-remote`
reattaches (`wait <id>`) a few times, and otherwise prints the id to
reattach to later.

The box's commands work directly too. `-o IdentitiesOnly=yes` keeps ssh from offering every key the agent holds, which the box refuses as "Too many authentication failures":

```bash
ssh -o IdentitiesOnly=yes -i ~/.ssh/bench-helios bench-helios@<host> box               # can it serve? (then JSON)
ssh -o IdentitiesOnly=yes -i ~/.ssh/bench-helios bench-helios@<host> queue             # what runs, what waits and why
ssh -o IdentitiesOnly=yes -i ~/.ssh/bench-helios bench-helios@<host> health            # the same as JSON
ssh -o IdentitiesOnly=yes -i ~/.ssh/bench-helios bench-helios@<host> list              # recent jobs
ssh -o IdentitiesOnly=yes -i ~/.ssh/bench-helios bench-helios@<host> fetch <id> > r.tar
ssh -t -o IdentitiesOnly=yes -i ~/.ssh/bench-helios bench-helios@<host> debug test branch=main   # a shell in the job's container
```

After changing the Benchfile on a branch, upload it under that branch's
name. Jobs run with `branch=<branch>` then use it, and every other branch
keeps the default Benchfile:

```bash
ssh -o IdentitiesOnly=yes -i ~/.ssh/bench-helios bench-helios@<host> config benchfile <branch> < tools/bench/Benchfile
```

The default Benchfile is the one on `main`. Upload it, with no branch name,
only from `main`, once the change is merged:

```bash
ssh -o IdentitiesOnly=yes -i ~/.ssh/bench-helios bench-helios@<host> config benchfile < tools/bench/Benchfile
```

### Job names, priority and dependencies

Each job goes to the box under a name of its own, its `ext_job_id`:
`helios-<branch>-<job>`, such as `helios-wip-undo-lint`, or
`egui-pop-<egui_pop branch>-<job>` for egui-pop's jobs
(`egui-pop-feat-dock-gate` for `egui-pop-gate egui_pop=feat/dock`).
A job given parameters of its own (`crate=`, `test=`, `filter=`...)
gets a short hash of them after the job:
`helios-wip-undo-test-crate-1a2b3c4d`. The parameters that only say where
and how it runs (`branch=`, `egui_pop=`, `src=`, `priority=`, `after=`,
`after_ok=`) are left out of the hash, and so are `base=` and `crates=`: a
branch's `test-affected` stays one run, replaced by the next push's. Characters the box refuses become
`-`; a branch too long for the box's 64 characters is cut and a hash added.
`bench-remote` prints the name it uses; `ext_job_id=<name>` gives another
(made acceptable to the box the same way). **Submitting under a name the
box already holds replaces that job, queued or running** (it is
cancelled): the same job with the same parameters, re-queued after a
rebase or a new push, replaces its stale run, and the jobs waiting on the
name follow the new one. Two different runs on one branch (two crates, two
filters) have different names and run side by side; two runs of the very
same job that must both happen need `ext_job_id=` names of their own.

The box takes parameter values in `[A-Za-z0-9._/@+-]` only, so
`bench-remote` sends a `filter=` test path's `::` as `@` and its spaces as
`+`, and the test jobs (`test-crate`, `egui-pop-test`, `egui-pop-test-1p`)
turn them back. Raw `ssh ... run` calls write `filter=undo@tests@redo`
themselves.

The name stands for the job wherever the box takes a job id, as
`x:<name>`:

```bash
ssh … status x:helios-wip-undo-lint
ssh … cancel x:egui-pop-feat-dock-gate
cargo xtask bench-remote release after_ok=x:helios-wip-undo-lint   # only if it succeeded
cargo xtask bench-remote test after=x:helios-wip-undo-lint         # once it ended, any outcome
```

A job waiting on dependencies holds nothing on the box. When an `after_ok`
dependency does not succeed, the job never runs and ends with **exit 79**.
Dependencies are Helios's own jobs on the box (egui-pop's jobs included).

**egui-pop jobs first.** Every `egui-pop-*` job is submitted with
`priority=high`, ahead of every normal job on the box; the other jobs are
normal. `--priority high|normal` (or `priority=`) says otherwise:

```bash
cargo xtask bench-remote egui-pop-gate egui_pop=feat/dock                     # high
cargo xtask bench-remote egui-pop-gate egui_pop=feat/dock --priority normal  # normal
cargo xtask bench-remote release --priority high                            # a merge waits on it
```

The box keeps one high-priority job waiting per project: the last one
submitted (or `ssh … bump <id>`) takes it, and the one before goes back to
normal; the box says so.

### Chains

A branch's usual sequence goes to the box in one call, each step its own
job waiting on the ones before (`after_ok`):

```bash
cargo xtask bench-remote --chain egui-pop egui_pop=feat/dock   # catalogues, then snapshots, then the gate
cargo xtask bench-remote --chain helios                        # lint and test-affected side by side
cargo xtask bench-remote --chain helios --release branch=wip/x # ... then the release, once both succeeded
```

Every parameter goes to each step (`base=` and `crates=` to
`test-affected` alone), and
`--push` works as for a single job. Each step carries its usual name
(`egui-pop-feat-dock-snapshots`, `helios-wip-x-release`...): running one
step again alone, after a fix, replaces it in the chain, and the steps
after it wait on the new run. `bench-remote` then follows the steps in
turn as single jobs (their logs, results folders and summaries) and ends
with a table of their results. A step whose dependency failed never runs:
it ends with **79**. The exit code is the first failure's (a step's own
code, or 79), else 75 when a step could not be served, else 0.

egui-pop's chain runs at high priority, Helios's at normal; `--priority`
changes either. Since the box keeps one high-priority job per project, the
first step is submitted high and each later one is bumped
(`bump <id>`) when the steps before it have ended.

### Capacity and the queue

Jobs run side by side when their threads and memory fit in what the box
gives to jobs (`queue` and `box` say how much); a job with an `exclusive`
resource, such as the timing jobs holding the machine, runs alone. Memory,
more than threads, is what usually makes a job wait. Check
`cargo xtask bench-remote queue` before submitting: a long queue is a
reason to run a narrower job, or later. A queued job made stale by a
rebase or a new push is replaced when the job is submitted again under
its name (above), or cancelled with `ssh … cancel <id>`, rather than left
to run.

A job's `memory =` is what the queue counts, so it follows the measured
peak plus about 20 %; the container's hard limit is 2 GB above it. The
`memory peak` line of a job's log understates the peak (it missed the
links below); the figures here are the job cgroup's `memory.peak` (page
cache included) and the summed RSS of the top processes sampled every
2 s. What peaks is linking: each `rust-lld` of a test binary holds 1 to
1.5 GB, while rustc stays under 0.6 GB and a gallery test process under
1 GB. `CARGO_BUILD_JOBS` bounds how many link at once. Rust 1.95 already
links with lld on x86_64 Linux. Debug info stays at line tables (it is
already off for dependencies), since dropping it would rebuild every
target for little gain.

egui-pop's jobs, measured on 2026-11-07 (egui-pop `main`):

| Job | Before: reserved | Before: peak | After: settings | After: reserved |
| --- | --- | --- | --- | --- |
| `egui-pop-test` (gallery, one filter) | 5g | cgroup 5.0 GB (the limit, 0.2 GB swapped), 4.5 GB RSS in three links at once; 270 to 360 s | 2 build jobs | 4g |
| `egui-pop-snapshots` | 7g | cgroup 3.1 GB, 2.8 GB RSS in two links; 453 s (median 472 s) | 2 build jobs (at most two links already) | 4g |
| `egui-pop-gate` | 8g | cgroup 5.0 GB (the limit of a 3g probe, 0.2 GB swapped), 4.7 GB RSS in three links at once (the log line said 2.5 GB); 32 min, 23 to 44 min over 23 runs | 2 build jobs, timeout 75m | 4g |

The "before" peaks were measured with the build jobs at 3 for the test and
the gate, and at the default 5 for the snapshots. Two build jobs bound a
job to two links (about 3 GB) plus rustc, about 3.5 GB. The new
reservations have not been re-measured yet: if a job's `memory.peak` goes
past its reservation, raise it again. The hard limit (6 GB) is above even
the three-link peak. At 4g, `egui-pop-snapshots` and `egui-pop-test`
(8 GB, 9 threads) run side by side, and so do either of them and a Helios
`test-affected`.

#### Where the time went, 2026-11-07

The box's jobs that day, from `target/bench-remote/<id>/conditions.json`
(`seconds`) and the client's `bench: waiting` lines, about one a minute:

| job | runs | green | median | max | total | queued (min) |
|---|---|---|---|---|---|---|
| egui-pop-gate | 13 | 3 | 20.3 min | 44.4 min | 277 min | 35 |
| egui-pop-snapshots | 17 | 14 | 8.1 min | 10.7 min | 142 min | 171 |
| test-affected | 14 | 10 | 8.7 min | 17.1 min | 140 min | 23 |
| release | 6 | 6 | 12.7 min | 15.4 min | 79 min | 8 |
| egui-pop-catalogues | 13 | 13 | 0.8 min | 1.7 min | 12 min | 82 |
| egui-pop-test | 2 | 2 | 4.9 min | 5.3 min | 10 min | 2 |
| build-gui | 5 | 5 | 1.8 min | 2.6 min | 9 min | 42 |
| lint | 9 | 9 | 0.8 min | 1.5 min | 8 min | 45 |

- The gate was the biggest cost: 13 runs, 10 of them red, 277 minutes.
  It was used as a fix loop and failed late on a single-process leak.
  The box no longer runs it per branch (CI on `next` does), and it now
  runs under nextest and stops at the first failure.
- The snapshots waited longer in the queue (171 min) than they ran
  (142 min), and short jobs (catalogues, lint, build-gui) waited behind
  the big ones: memory is the bottleneck (see "Capacity" below and the
  memory work on the egui-pop jobs).
- On GitHub, egui-pop's lavapipe job took about 20 min: 3 min to build,
  16 min of tests (2,314), 1 min of doctests. It now runs as three
  shards side by side.

## Running what a change touches

A branch rarely needs the whole suite: `cargo xtask affected` says what a
change can break, and the box runs that.

```bash
cargo xtask affected                    # since origin/main: JSON on stdout, a summary on stderr
cargo xtask affected main --filter      # the nextest filter alone
cargo xtask affected --critical         # the critical set's filter alone
cargo xtask affected --crates helios-core+helios-gui --filter   # a selection given, no git needed
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

A source pushed with `--push` reaches the box without its `.git`, so the
box cannot diff it. `bench-remote` then computes the selection on the
client (against `base=`, `origin/main` by default) and gives it to
`test-affected` as `crates=`: `all` for everything, `none` for the
critical set alone, else the crates joined by `+`
(`crates=helios-core+helios-gui`). The job runs `cargo xtask affected
--crates <list>`, which takes the list as given, and the base is only a
label in its `affected.json`. This holds for `test-affected` alone,
`--affected` and `--chain helios`; a `crates=` given by hand is kept, and
works without `--push` too.

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

## When Ubuntu's archive is down

GitHub's Ubuntu runners install packages from Ubuntu's archive
(`archive.ubuntu.com`, its Azure mirror, `security.ubuntu.com`). When
Canonical has an outage, every `apt-get` step fails with "Temporary failure
resolving" or "Unable to fetch some archives", through no fault of the
code. One repository variable, `UBUNTU_ARCHIVE_DOWN`, tells the workflows:

```bash
gh variable set UBUNTU_ARCHIVE_DOWN --body true -R quazardous/helios   # on
gh variable delete UBUNTU_ARCHIVE_DOWN -R quazardous/helios            # off
```

While it is `true`, what can only work with the archive is skipped, not
failed, and each run says so with a warning and a line in its summary;
what has a fallback without the archive uses it at once:

| Workflow | Job or step | With the switch on |
|----------|-------------|--------------------|
| ci | `test`, `commits`: software Vulkan | no apt: lavapipe from the pinned Mesa build of gfx-rs/ci-build (checked by SHA-256) |
| ci | `lint`: shellcheck | unchanged: the runner image's own shellcheck, apt only if it is missing |
| release | `flatpak` | unchanged: it runs in Flathub's builder image, with no apt |
| parity | `darktable` | skipped |

Unset or `false`, the workflows behave as they always did. Skipped jobs
count as passed in a run's result, so a green `ci` on `next` still
promotes `main`.

The Linux tests still run on GitHub (lavapipe from the fallback Mesa);
no full gate is run on the bench box in their place. Only what is skipped
waits: darktable parity.
Delete the variable once `apt-get update` works on the runners again.

## Rules for agents (a shared box)

Several agents share one box, one queue and one disk quota. A job that hangs
or hoards blocks everyone for the length of its timeout. These rules come
from incidents on the box; every agent prompt points here.

**The next branch**: how a change reaches `next` (a pull request with its
test manifest; the tester's gate runs the jobs and merges) is in
[next.md](next.md), and the tester's own brief in [tester.md](tester.md).
Every agent follows them.

**The work cycle**
- Code the whole change with local checks only: `cargo fmt --check`, then
  `cargo check` and `cargo clippy` on the crates touched, with
  `CARGO_INCREMENTAL=0` (incremental caches grew to 20 GB and more). These
  type-check and lint without generating code, so they are quick and light.
  Local tests are not run.
- Once the code is finished and clean locally, finalise on the box: tests
  and snapshots (for egui-pop and egui_stack, CI on `next` runs the rest). The box confirms; it is not where errors are first
  looked for (a failing gate costs about seven minutes for what a local
  clippy shows in seconds).
- Box runs are queued by the tester alone (`cargo xtask gate`: a batch of
  pull requests' manifests on a `gate/…` branch), plus the wide runs
  (snapshots, release, the daily full suite) at fixed points. The tester
  does the waiting, so the agents writing code never wait on the box.

**Before queuing**
- Run the narrowest job first. A Helios change is tested with `test-crate
  crate=<crate> test=<target> filter=<name>` on the crates and targets it
  touched, and nothing wider: no `test-affected` and no `--affected` in a
  manifest ([next.md](next.md)). On Helios they reach
  nearly the whole suite (2026-11-08: 1,370 to 1,440 of 1,440 tests),
  which `next`'s CI already runs. `test-affected` and `test` are for the
  daily run and releases.
- A test that fails, then passes on nextest's retry (`retries = 1` in the
  `ci` profile), is reported FLAKY and the run stays green; each flaky test
  gets a ticket and a fix, never a lasting place in the suite.
- For egui-pop, a fix loop runs `egui-pop-test crate=<crate> test=<target>
  filter=<name>` (minutes); `test=lib` runs the crate's unit tests. A failure CI shows is reproduced with
  `egui-pop-test`.
- The box does not repeat what CI runs. egui-pop and egui_stack pull
  requests go to their `next` branch, whose CI runs every test; the box
  serves what CI cannot: a fix loop's narrow runs and snapshots regenerated
  to commit (`egui-pop-snapshots`). `egui-pop-gate` stays for a release or a
  by-hand check, not for each branch.
- The GPU and timing jobs (`gpu`, `gpu-intel`, `egui-pop-gpu-tiles`,
  `egui-pop-threading`, `egui-pop-perf`, `start-timing`, `quit-timing`, `test-times`, `thumb-bench`, the
  parity and bench jobs) run before a release only, queued by one agent
  given that job, never by the agents writing code. Helios's whole suite
  runs in `next`'s CI (nextest on the workspace, survival, Windows).
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
- Helios's and egui-pop's targets live on the hard disk (an egui-pop gate
  needs 25 to 35 GB); sccache, the registry and the images on the SSD
  (60 GB quota).
- Exit 75 with "needs about N GB": wait for space; do not free it by
  deleting others' storage.

**The development machine**
- Each worktree builds into its own `target/`: leave `CARGO_TARGET_DIR`
  unset there (`env -u CARGO_TARGET_DIR cargo …` when the shell exports
  one). Cargo names a workspace crate's artifacts after its path inside the
  workspace, so worktrees of one repository sharing a folder overwrite each
  other's builds, and cargo then reads another branch's build as fresh:
  false "not found" errors until a source file is touched. `bench-remote`
  warns when a linked worktree builds outside itself; its job results go
  to the worktree's own `target/bench-remote/` either way.
- Local work is check-only (see "The work cycle"), so a worktree's
  `target/` stays small: about 0.6 GB for a clippy of all of egui-pop's
  targets from scratch, in about 80 s. Full builds and tests belong on the
  box; a worktree used for them locally grew to 1 to 15 GB, and a few dozen
  of those once filled the disk.
- At most three agents compile at once.
- A worktree goes when its branch merges, after checking it holds no
  unpushed or uncommitted work; its box storage goes with it (`storage rm`).

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
