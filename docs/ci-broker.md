# CI broker

Our CI keeps getting longer. The CI broker is meant to save time by
choosing which tests a run needs, from what earlier runs saw:

- tests near the change, new tests, and tests that failed or flaked lately
  always run;
- tests that have stayed green for a long time come back at a rhythm;
- full runs stay nightly, before every release (only a full run
  promotes) and on demand.

Pushes to `next` run the selection instead of the whole suite: see "On
next".

It is built in steps:

| Step | What | State |
|---|---|---|
| B1 | The per-test history, written nightly to the `ci-history` branch | done |
| B2 | The always-run set: file-level `affected`, new or changed tests, recent failures, a smoke set | done |
| B3 | A shadow week: the broker records its choice and skips nothing, to measure what it would have missed | done: 64 runs, every first failure caught |
| B3+ | Pushes to `next` run the selection; the full suite nightly; only full runs promote; recent failures read by days | done |
| B4 | Confidence rhythm, seeded per commit | after the shadow week |
| B5 | A time budget per pull request | |
| B6 | Full runs nightly, at release and on demand; bisect-back tickets | |

The code lives in xtask: `xtask/src/ci_broker/` holds the format, the fold
and the report, with std and serde_json as its only dependencies, so it can
move to a shared crate when egui-pop adopts it. `xtask/src/ci_history.rs`
is the command, which reads the JUnit files and git.

## Where the history lives

The history lives on an orphan branch, `ci-history`, of this repository.
It holds no code; it only shares the repository:

- A nightly job writes it, one commit a night, so pushes never race.
- A pull request's job reads it with one shallow `git fetch`. That costs
  no API call.
- Artifacts alone expire, and reading each one costs an API call.
- The bench box cannot hold it, because GitHub runners cannot reach it. The
  box can feed it later, as a second `source`.

```
ci-history branch
├── summary.json          the per-test summary, folded from the day files
└── days/
    ├── 2026-11-19.jsonl  one line per CI run started that day (UTC)
    └── …
```

## History format

Format version 1.

### Day files: `days/YYYY-MM-DD.jsonl`

They are append-only: one JSON line per CI run, in the file of the day the
run started.

```json
{"format":1,"seq":42,"run":"19456781234","commit":"<full sha>","date":"2026-11-19T09:12:00Z","source":"ci","wall_s":1104.2,
 "tests":[["helios-core graph::tests::a_cycle_is_refused","pass",0.012],["helios-gui::it import::a_card_imports","flaky",3.4]]}
```

(Here on two lines; one line in the file.)

- `run` is the GitHub run id, `commit` its head commit, and `date` the time
  the run started.
- `seq` is the order in which the run was folded into the summary.
- `source` says which CI the run came from: `ci` is GitHub's ci.yml.
- `wall_s` is the nextest run's wall time. For a run with several reports
  (shards), it is the longest one.
- `tests` holds one entry per test: its id, its outcome and its time in
  seconds.
  - **The test id** is `<binary id> <test name>`, as nextest prints it. The
    binary id has no space in it.
  - **The outcome** is one of:
    - `pass`;
    - `fail`: it failed on every attempt;
    - `flaky`: it failed, then passed on a retry (nextest's `<flakyFailure>`);
    - `skip`: it was listed but not run.
  - When a run names a test in two reports, the worst outcome wins.

### `summary.json`

It is a small header, then one line per test, so a night's change reads as
a diff of the tests that ran.

```json
{
  "format": 1,
  "runs": 42,
  "last_run": {"run": "19456781234", "commit": "…", "date": "2026-11-19T09:12:00Z"},
  "ingested": ["…", "19456781234"],
  "tests": {
    "helios-core graph::tests::a_cycle_is_refused": {"runs":40,"passes":40,"failures":0,"flakes":0,"skips":0,"streak":40,"first_seen":"…","last_seen":42,"last_run":"…","last_failure":null,"last_flake":null,"median_s":0.012,"recent_s":[…]}
  }
}
```

| Field | Meaning |
|---|---|
| `runs` (top) | The runs folded so far. It is also the last run's `seq`. |
| `ingested` | The last 1000 run ids. A run in it is never folded twice. |
| `runs` | Runs that ran the test (skips aside). |
| `passes`, `failures`, `flakes`, `skips` | Counts per outcome. |
| `streak` | Passes in a row since the last failure or flake. |
| `first_seen` | The date of the first run that listed the test. |
| `last_seen` | The `seq` of the last run that listed it. |
| `last_run` | The date of the last run that ran it. |
| `last_failure`, `last_flake` | `{run, commit, date}`, or `null`. |
| `median_s`, `recent_s` | The median of its last 21 times. Passes and flakes count; a failure's time does not. |
| `shadow` (top) | The shadow week's runs and misses: see "Shadow week". Absent in a summary from before B3. |

**A renamed test** shows up as a new id with a fresh record. The old id
keeps its history. It is no longer current once no run among the last 10
has listed it, and the report then leaves it out. After 500 runs without it,
it leaves the summary altogether, but its lines stay in the day files.

**The summary is a pure fold of the day files.** If `summary.json` is lost
or doesn't read, rebuild it with `cargo xtask ci-history rebuild --history
DIR`. The rebuild replays the runs in `seq` order. With no history or an
unreadable one, the broker's later steps fall back to running every test:
missing data costs time, never safety.

## Commands

```sh
# One run's reports (several for a sharded run) into a checkout of the branch.
cargo xtask ci-history ingest --history DIR --run ID --commit SHA \
    --date 2026-11-19T09:12:00Z [--source ci] junit.xml...

# summary.json folded again from the day files.
cargo xtask ci-history rebuild --history DIR

# Read only: the slowest, the flakiest, and the tests never failed in N runs.
git fetch origin ci-history
cargo xtask ci-history report [--top N|all] [--min-runs N]   # reads origin/ci-history
cargo xtask ci-history report --history DIR

# The shadow week over the last N days (7): see "Shadow week".
cargo xtask ci-history report --shadow [--history DIR] [--days N]
```

`report` lists three things:
- the slowest current tests, by median time, with the total for all of them;
- the tests that flaked, highest flake rate first, each with the commit of
  its last flake;
- the tests that neither failed nor flaked in `--min-runs` runs or more (50
  by default), longest record first.

These lists are the input for B2's always-run set and B3's shadow week.

## The always-run set

`cargo xtask ci-broker select` prints the tests a change always runs, as
one nextest filter expression on stdout, and per source what it chose and
why on stderr. Pushes to `next` run it (see "On next").

```sh
cargo xtask ci-broker select [--base origin/next] [--history DIR] [--days 7] [--runs 20]
cargo xtask ci-broker select --failed-junit earlier-run/junit.xml   # repeatable
cargo xtask ci-broker select --list tests.txt --summary-md summary.md
cargo nextest run --workspace -E "$(cargo xtask ci-broker select)"
cargo xtask ci-broker select --explain 'helios-gui::it import::a_card_imports'
# a past commit, read from git alone (the map and the suites are today's)
cargo xtask ci-broker select --base 31b2a1f7^ --head 31b2a1f7
```

With a history, the summary ends with a measure: how many of the current
tests (those listed in its last 10 runs) the set holds.

The set is the union of four sources:

1. **affected**, at file level. The changed files are those `cargo xtask
   affected` reads: commits since the merge base with `--base`, the
   working tree and untracked files.
   - A path outside a crate follows `affected`'s rules. `Cargo.toml`,
     `Cargo.lock`, the toolchain, xtask, `.github/`, `.config/` and any
     path no rule knows select **every test**; data a crate reads selects
     that crate; docs select nothing.
   - A crate's integration test file, `tests/<target>.rs`, selects that
     test binary alone. Other files in `tests/` (data, shared helpers)
     select the crate.
   - Any other file of a crate (sources, manifest, `build.rs`) selects the
     crate and every crate depending on it, as `affected` does. A change
     in a crate the others share, such as helios-core, so selects nearly
     everything.
   - **helios-gui** is finer. Its integration tests are the modules of one
     binary, `it` (830 tests). A changed source file is mapped to the `it`
     modules that exercise it:
     - by name: `src/import_scan.rs` selects `import` and `import_scan`;
       `src/library/…` selects `library_*`. A file under `src/shell/` is
       mapped by its own name, since the shell reaches everything;
     - by `use` path: a module whose source names `helios_gui::import_scan`
       (or `helios_gui::shell::import_window` for a shell file);
     - by the checked-in override table, `OVERRIDES` in
       `xtask/src/ci_select.rs`, and `affected`'s `GUI_SUITES`.

     When no module is found, or for `lib.rs` and `main.rs`, the whole
     `it` binary runs. helios-gui's other tests (unit tests) and the crates
     depending on it run too. A changed `it` module selects itself and the
     modules naming it (`crate::<module>`: shared helpers).
   - **A feature crate** cut out of helios-gui (`helios-storage-ui`; the
     `FEATURES` table in `xtask/src/ci_select.rs`) **stops at the app
     crate** when an edit leaves its contract alone. It then selects:
     - the crate's own tests;
     - the unit tests of its mount in helios-gui (`src/shell/storage.rs`);
     - the `it` modules exercising it: those the mount maps to (as above),
       those naming the crate (`helios_storage_ui`) or a name helios-gui
       re-exports from it (`pub use helios_storage_ui::…`), those quoting
       one of its public string ids (its commands and bus points, such as
       `"library.storage"`), and the table's own override modules.

     It does not select all of helios-gui, nor the crates depending on it.
     The contract is read without compiling
     (`xtask/src/ci_broker/contract.rs`). It holds:
     - the file's `pub` items: signatures, not bodies, for functions and
       structs; the whole item for enums, traits, consts and their values;
     - its top-level consts and statics, public or not: point ids and
       contract tables such as `POINTS`;
     - its `impl` headers, and the associated consts, types and signatures
       of its trait impls (a bus point's `const ID`);
     - its derives and its crate attributes other than lint levels.

     Comments, function bodies, private items and lint attributes are not
     part of it. The idea comes from
     [`cargo public-api`](https://github.com/cargo-public-api/cargo-public-api)
     and [`cargo-semver-checks`](https://github.com/obi1kenobi/cargo-semver-checks),
     which diff a crate's public API between two versions. Both need a
     rustdoc JSON build, too slow before any build, so this check reads
     more than the API, never less.

     The crate selects like any other crate (itself, helios-gui and every
     crate depending on them) in these cases:
     - the edit changes that contract;
     - the file is not a source (`Cargo.toml`, data);
     - the file is new or deleted and has public items;
     - a crate other than helios-gui depends on the feature.

     A crate missing from the table selects like any crate: add a line
     when a feature leaves helios-gui.

     Measured on 2026-11-20 against the history's 3283 current tests:
     - a body edit in `helios-storage-ui` selects 155 tests instead of
       1167; so does 31b2a1f7, a lint attribute on its `lib.rs`;
     - an edit to a description in its `POINTS` table still selects 1167.
2. **changed tests**: the `#[test]` functions whose lines the diff adds or
   edits (`git diff -U0`), all of them for a new file. Mostly inside
   source 1 already; listed so `--explain` says it.
3. **recent failures**: every test that failed or flaked lately runs.
   - From the history's `summary.json` (`origin/ci-history` or `--history
     DIR`): the tests whose last failure or last flake is within the last
     `--days` days (7), counted back from the summary's newest run, or
     among its last `--runs` runs (20), whichever reaches further. Reading
     20 runs alone missed 6 tests in the shadow study: they had failed on
     an earlier commit of a red `next`, more than 20 runs (a few hours of
     `next`) before.
   - From runs the nightly history has not folded yet, `--failed-junit
     FILE` (repeatable): their JUnit reports' failed and flaky tests. CI
     passes `next`'s latest push runs and its last nightly run.
     A report that doesn't read is left out, and said.
   - With no history yet the summary part is skipped and the notes say so.
     A history that doesn't read runs every test.
4. **smoke**: `ci/smoke.txt`, about twenty fast, broad tests: `affected`'s
   critical set (a render, an export, round trips, survival) and a short
   journey through the desktop (first start, import, library, edit,
   settings, quit). A unit test checks each entry still exists.

The map is coarse on purpose. It may select too much; it should not
select too little, and B3 measures that: each test the shadow week sees
fail outside the set becomes an override line, or a reason to widen the
map. The full suite still runs on `next` after every merge.

The policy (`xtask/src/ci_broker/select.rs`) knows nothing of Helios:
scopes, the filter, `--explain`, the history and smoke sources. The
Helios map lives in `xtask/src/ci_select.rs`.

## On next

A push to `next` runs the selection, not the whole suite; the whole suite
runs nightly. Only a full run promotes `next` to `main` and releases. Our
own pull requests still run `pr-lint` alone, and their tests on the bench
box; a contributor's pull request from a fork runs the whole suite.

| Run | Tests | Promotes |
|---|---|---|
| A push to `next` | the broker's selection since the push's previous commit | no, unless it ran every test |
| A push to `next` whose commits carry the trailer `CI: full` | every test | yes |
| Nightly (`schedule`, 01:15 UTC), on `next`'s head | every test; skipped when a nightly run already tested that commit | yes |
| Manual (`workflow_dispatch`), on `next`'s head | every test | yes |
| A push to `main`, a pull request from a fork | every test | no |

The `test` job of `.github/workflows/ci.yml` decides in its step "full
suite or selection". On a push to `next`, it:

1. downloads the `junit` artifacts of `next`'s last 3 completed push runs
   and of the last nightly run, for the recent failures the nightly
   history has not folded yet;
2. lists every test of the workspace (`cargo nextest list --message-format
   oneline`), the run's denominator;
3. runs `cargo xtask ci-broker select --base <the push's previous commit>
   --committed --failed-junit … --list tests.txt --summary-md
   broker-summary.md`;
4. runs nextest with the filter it printed.

The job summary says what ran: "**N of M tests selected** (affected: …,
changed tests: …, recent failures: …, smoke: …)", with each source's notes
(which window of the history was read), and how to run everything. The
survival step is skipped: the smoke set holds the survival scenarios. Doc
tests and the GPU diffstat run as before: the selection does not cover them.

Every test runs instead, and the job summary says why, when:

- the run is not a push to `next` (nightly, manual, `main`, a fork's pull
  request);
- a pushed commit carries the trailer **`CI: full`** (a line of its own in
  the commit message, any case): the way to ask for a full run, and so a
  promotion, from a merge;
- there is no previous commit to compare with (a first or forced push);
- the tests do not list, the broker fails, or it selects everything
  (`all()`: a change to `Cargo.lock`, the toolchain, xtask, `.github/`, …).

Each run uploads a `ci-run` artifact, `{"format":1,"mode":"full"|"select",
"commit":…,"event":…}`: `commit` is the commit tested (a nightly run's own
head is `main`'s; it checks out `next`). promote.yml reads it: only a
green run whose `mode` is `full` promotes (`cargo xtask promote --run`,
docs/releases.md). ci-history.yml reads it for the nightly runs' commit.

The nightly run is skipped when `next` has not moved since the last nightly
run, as windows-tests.yml does: its `mark` job saves a cache entry
`ci-full-tested-<sha>` once it tested a commit, pass or fail, and its
`gate` job looks for it. A full run made by a push or by hand does not
count: the nightly run tests that commit again. Scheduled runs use
`main`'s copy of ci.yml: the nightly run starts once this reaches `main`.

A test the selection skips that breaks is caught by the next full run: the
nightly one at the latest, which then holds `main` back until it is green.
It then fails in the history, and every push selects it for 7 days.

## The nightly job

`.github/workflows/ci-history.yml` runs at 03:45 UTC, and also on manual
dispatch.

0. A scheduled run first asks its `gate` job, which takes seconds (no
   checkout, no toolchain), whether there is anything to ingest. It reads
   `last_run` in the branch's `summary.json` (GitHub's contents API), then
   looks among the completed ci.yml push runs on `next` of the last two
   days for one newer than it that uploaded `junit`. None: the `ingest`
   job is skipped, the run stays green and its summary says
   `skipped: no run on next to ingest since run <id>`. No branch yet, no
   `last_run`, or any API error: it ingests (the gate fails open). A manual
   run always ingests.
1. It lists the completed ci.yml push runs on `next` and its nightly and
   manual runs from the last two days, oldest first. A run's commit is its
   `ci-run` artifact's when it has one (a nightly run tests `next`, its own
   head is `main`'s).
2. For each run, it downloads the `junit` artifact that ci.yml's `test` job
   uploads. Failed runs upload one too. A run that stopped before its tests
   has none and is skipped.
3. It ingests each run. A run already in the history is left out, so
   overlapping windows count nothing twice.
4. It commits to `ci-history` as `github-actions[bot]` and adds the report
   to the job summary.

The job runs one at a time and is never cancelled. GITHUB_TOKEN's
`contents: write` is repository-wide, but the job only pushes
`refs/heads/ci-history`.

**The schedule starts once the workflow reaches `main`.** GitHub reads
scheduled workflows from the default branch, so the first nightly history
appears the night after `next` is promoted with this file. Before that, run
the workflow by hand on `next` if it is needed sooner.

## Shadow week

Before the broker skipped anything, it ran in shadow (step B3): CI ran
the full suite on every push to `next`, and recorded beside it what
`ci-broker select` would have chosen. The step and its `broker-shadow`
artifact stay on pushes to `next`; on a selective run every test it ran
is selected, so misses are now seen by the nightly full run instead.

The study of 64 runs (2026-11-19 to 11-21) found every one of the 20 first
failures selected. The 6 tests failing outside the selection had all
failed on an earlier commit already, and were missed only because recent
failures were read from the last 20 runs: they are now read over 7 days
and from runs not folded yet. Selective runs saved 29 % of test time;
`helios-gui::it` is most of what remains.

### On each push to next

The `test` job of `ci.yml` has a `broker shadow` step, after the tests
and their JUnit upload:

```sh
cargo xtask ci-broker select --base <the push's previous commit> --committed \
    --shadow broker-shadow.json --run <run id> --commit <sha> \
    --junit target/nextest/ci/junit.xml
```

- It runs on pushes to `next` only, after failed tests too, and never
  fails the job (`continue-on-error`). xtask is already built by the step
  before it, so it costs a few seconds.
- `--committed` reads the commits since the base, not the working tree:
  CI's checkout holds untracked files (vkdt) that would select everything.
- A first or forced push has no previous commit to compare with: no
  shadow for that run.
- The file is uploaded as the `broker-shadow` artifact:

```json
{
  "format": 1,
  "run": "19456781234",
  "commit": "<full sha>",
  "base": "<the push's previous sha>",
  "everything": false,
  "filter": "<the nextest filter select printed>",
  "tests": 4123,
  "selected": ["helios-gui::it import::a_card_imports", "…"],
  "changed": ["crates/helios-gui/src/import_scan.rs", "…"],
  "sources": [{"source": "affected", "filter": "…", "why": "…"}],
  "notes": [{"source": "recent failures", "note": "…"}]
}
```

`selected` lists the tests of the run's JUnit report that the set holds
(all of them when `everything`); `tests` counts those the report lists.

### Nightly

`ci-history.yml` downloads each run's `broker-shadow` artifact next to its
`junit` one and passes it to `ci-history ingest --shadow FILE`. A run
without one is ingested all the same, with its shadow skipped; an artifact
that doesn't read is skipped the same way.

For each run with a shadow, the ingest measures:

- `total` and `selected`: the tests the run ran (skips aside), and those
  of them selected;
- `total_s` and `selected_s`: the same in test-seconds, each test counted
  at its median time before the run (its own time when it has none yet).
  These are test-seconds, not wall time: nextest runs tests in parallel;
- **misses**: the tests that failed on every attempt in the full suite but
  were not selected. A flake (passed on its retry) is not a miss.

The day file line gets a `shadow` object with these fields, the base and
the changed files, so a rebuild finds them again. `summary.json` gets a
`shadow` section: `runs` and `misses` since the history began, `list` (the
last 500 misses, each with its run, commit, date, test and changed files)
and `recent` (the last 500 shadow runs, with their counts).

### The report

```sh
git fetch origin ci-history
cargo xtask ci-history report --shadow [--days 7]
```

It prints, over the last N days up to the last shadow run:

- the number of runs, the selected share (selected tests / tests run) and
  the test-seconds selection would have saved;
- each miss, with its run, commit and the files that changed in that run.
  A miss on a real break is fixed by an `OVERRIDES` line in
  `xtask/src/ci_select.rs` (from a changed file to the test's module), or
  by widening the map.

The nightly job adds the report to its job summary.

### When the week starts

The `broker shadow` step runs on the first push to `next` after this
change lands there. The nightly ingest that reads it runs from `main`'s
copy of `ci-history.yml`, so the shadow history starts once `next` is
promoted with it. Its first run reads the last two days of runs, so the
shadow runs of those two days are not lost.

### Go criteria for B4

The analysis aims for 80–90 % of regressions caught on the pull request,
the rest by the full runs on `next`. Skipping goes live (B4) when, over
7 days of shadow runs on `next`:

- **no miss on a real break**, or every miss on a real break is fixed by
  an `OVERRIDES` entry (or a wider map) that makes `select --explain`
  pick it for that run's change;
- flakes and infrastructure failures among the misses are named as such
  in the review, not counted;
- the selected share is low enough to be worth it: the report's saved
  test-seconds are the measure, read against today's test time.

The owner read the report on 2026-11-24 and turned the selection on for
pushes to `next`, with the whole suite nightly and only full runs promoting
(see "On next").
