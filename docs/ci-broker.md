# CI broker

Our CI keeps getting longer. The CI broker is meant to save time by
choosing which tests a run needs, from what earlier runs saw:

- tests near the change, new tests, and tests that failed or flaked lately
  always run;
- tests that have stayed green for a long time come back at a rhythm;
- full runs stay nightly, before every release and on demand.

It is built in steps:

| Step | What | State |
|---|---|---|
| B1 | The per-test history, written nightly to the `ci-history` branch | done |
| B2 | The always-run set: file-level `affected`, new or changed tests, recent failures, a smoke set | next |
| B3 | A shadow week: the broker prints its choice and skips nothing, to measure what it would have missed | |
| B4 | Confidence rhythm, seeded per commit | |
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
```

`report` lists three things:
- the slowest current tests, by median time, with the total for all of them;
- the tests that flaked, highest flake rate first, each with the commit of
  its last flake;
- the tests that neither failed nor flaked in `--min-runs` runs or more (50
  by default), longest record first.

These lists are the input for B2's always-run set and B3's shadow week.

## The nightly job

`.github/workflows/ci-history.yml` runs at 03:45 UTC, and also on manual
dispatch.

1. It lists the completed ci.yml push runs on `next` from the last two days,
   oldest first.
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
