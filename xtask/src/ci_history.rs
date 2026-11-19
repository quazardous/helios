//! `cargo xtask ci-history`: the per-test history of CI runs, kept on the
//! orphan branch `ci-history` (docs/ci-broker.md).
//!
//! - `ci-history ingest --history DIR --run ID --commit SHA --date DATE
//!   [--source S] JUNIT...`: one CI run's nextest JUnit reports (several
//!   for a sharded run) appended to `DIR/days/<day>.jsonl`, and folded into
//!   `DIR/summary.json`. A run already in the summary is left out, so
//!   overlapping windows are safe. A test that passed on its retry counts
//!   as a flake.
//! - `ci-history rebuild --history DIR`: `summary.json` folded again from
//!   every day file, when it is lost or does not read.
//! - `ci-history report [--history DIR] [--top N|all] [--min-runs N]`, read
//!   only: the slowest tests, the flakiest, and those that never failed in
//!   N runs (50 by default). Without `--history` it reads `summary.json`
//!   from `origin/ci-history` (`git fetch origin ci-history` first).
//! - `ingest --shadow FILE` adds the run's `broker-shadow` artifact (step
//!   B3): the tests the broker would have selected, and so its misses. A
//!   file that is not there is skipped, and said.
//! - `report --shadow [--days N]`: the shadow week instead, over the last
//!   N days (7): the selected share, the time saved, each miss with the
//!   files its run changed.
//!
//! The format and the fold live in `ci_broker`, which knows nothing of
//! JUnit or git: this file is the glue.

use std::path::Path;
use std::process::{Command, ExitCode};

use crate::ci_broker::history::{Outcome, Run, Summary, TestResult};
use crate::ci_broker::report;
use crate::ci_broker::shadow::{self, Artifact, Shadow};
use crate::test_times::{self, Report, Status};

pub const USAGE: &str = "ci-history ingest --history DIR --run ID --commit SHA --date DATE [--source S] [--shadow FILE] JUNIT... | ci-history rebuild --history DIR | ci-history report [--history DIR] [--top N|all] [--min-runs N] | ci-history report --shadow [--history DIR] [--days N]";

const SUMMARY: &str = "summary.json";
const DAYS: &str = "days";

/// One CI run from its JUnit reports.
pub fn run_from_reports(
    id: &str,
    commit: &str,
    date: &str,
    source: &str,
    reports: &[Report],
) -> Run {
    let mut run = Run::new(id, commit, date, source);
    for report in reports {
        if let Some(w) = report.wall_seconds {
            run.add_wall(w);
        }
        for case in &report.cases {
            run.add(TestResult {
                id: format!("{} {}", case.binary, case.name),
                outcome: match case.status {
                    Status::Passed => Outcome::Pass,
                    Status::Failed => Outcome::Fail,
                    Status::Flaky => Outcome::Flaky,
                    Status::Skipped => Outcome::Skip,
                },
                seconds: case.seconds,
            });
        }
    }
    run
}

fn read_summary(dir: &Path) -> Result<Summary, String> {
    let path = dir.join(SUMMARY);
    match std::fs::read_to_string(&path) {
        Ok(text) => Summary::from_json(&text).map_err(|e| {
            format!(
                "{}: {e} (`cargo xtask ci-history rebuild` folds it again from the day files)",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Summary::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Written beside, then renamed: a job killed halfway leaves the old one.
fn write_summary(dir: &Path, summary: &Summary) -> Result<(), String> {
    let path = dir.join(SUMMARY);
    let tmp = dir.join(format!("{SUMMARY}.tmp"));
    std::fs::write(&tmp, summary.to_json())
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Appends the run to its day file and folds it into the summary, with
/// its shadow when the run uploaded one. False when the summary already
/// holds it (nothing written).
pub fn ingest(dir: &Path, mut run: Run, artifact: Option<&Artifact>) -> Result<bool, String> {
    let day = run.day()?.to_string();
    let mut summary = read_summary(dir)?;
    if summary.contains(&run.id) {
        return Ok(false);
    }
    if let Some(a) = artifact {
        if a.run != run.id {
            return Err(format!(
                "the shadow artifact is run {}'s, not run {}'s",
                a.run, run.id
            ));
        }
        run.shadow = Some(Shadow::measure(&summary, &run, a));
    }
    if !summary.fold(&mut run) {
        return Ok(false);
    }
    let days = dir.join(DAYS);
    let file = days.join(format!("{day}.jsonl"));
    std::fs::create_dir_all(&days)
        .and_then(|()| {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file)?;
            writeln!(f, "{}", run.to_line())
        })
        .map_err(|e| format!("{}: {e}", file.display()))?;
    write_summary(dir, &summary)?;
    Ok(true)
}

/// Every run of the day files, in file then line order.
fn read_days(dir: &Path) -> Result<Vec<Run>, String> {
    let days = dir.join(DAYS);
    let mut files: Vec<_> = match std::fs::read_dir(&days) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("{}: {e}", days.display())),
    };
    files.sort();
    let mut runs = Vec::new();
    for file in files {
        let text =
            std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        for (n, line) in text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            runs.push(
                Run::from_line(line).map_err(|e| format!("{}:{}: {e}", file.display(), n + 1))?,
            );
        }
    }
    Ok(runs)
}

pub fn rebuild(dir: &Path) -> Result<Summary, String> {
    let summary = Summary::rebuild(read_days(dir)?);
    write_summary(dir, &summary)?;
    Ok(summary)
}

/// The summary on `origin/ci-history`, without a checkout.
fn remote_summary(root: &Path) -> Result<Summary, String> {
    let out = Command::new("git")
        .args(["show", &format!("origin/ci-history:{SUMMARY}")])
        .current_dir(root)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(
            "no summary on origin/ci-history: `git fetch origin ci-history`, or pass --history DIR"
                .into(),
        );
    }
    Summary::from_json(&String::from_utf8_lossy(&out.stdout))
}

fn fail(message: &str) -> ExitCode {
    eprintln!("ci-history: {message}");
    ExitCode::FAILURE
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let (command, rest) = args.split_first()?;
    let (mut history, mut id, mut commit, mut date, mut source) = (None, None, None, None, "ci");
    let (mut options, mut files) = (report::Options::default(), Vec::new());
    let (mut shadow_file, mut shadow_report, mut days) = (None, false, 7);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--history" => history = Some(Path::new(*it.next()?)),
            "--run" => id = Some(*it.next()?),
            "--commit" => commit = Some(*it.next()?),
            "--date" => date = Some(*it.next()?),
            "--source" => source = it.next()?,
            "--top" => {
                options.top = match *it.next()? {
                    "all" => usize::MAX,
                    n => n.parse().ok()?,
                };
            }
            "--min-runs" => options.min_runs = it.next()?.parse().ok()?,
            "--shadow" if *command == "ingest" => shadow_file = Some(Path::new(*it.next()?)),
            "--shadow" if *command == "report" => shadow_report = true,
            "--days" => days = it.next()?.parse().ok()?,
            f if !f.starts_with("--") && *command == "ingest" => files.push(f),
            _ => return None,
        }
    }
    match *command {
        "ingest" => {
            let (dir, id, commit, date) = (history?, id?, commit?, date?);
            if files.is_empty() {
                return None;
            }
            let mut reports = Vec::new();
            for f in &files {
                match std::fs::read_to_string(f)
                    .map_err(|e| e.to_string())
                    .and_then(|xml| test_times::parse_junit(&xml))
                {
                    Ok(r) => reports.push(r),
                    Err(e) => return Some(fail(&format!("{f}: {e}"))),
                }
            }
            let artifact = match shadow_file {
                Some(f) if f.exists() => match std::fs::read_to_string(f)
                    .map_err(|e| e.to_string())
                    .and_then(|t| Artifact::from_json(&t))
                {
                    Ok(a) => Some(a),
                    // A shadow that does not read loses the shadow, not
                    // the run's history.
                    Err(e) => {
                        eprintln!(
                            "ci-history: {}: {e}; run {id}'s shadow skipped",
                            f.display()
                        );
                        None
                    }
                },
                Some(f) => {
                    println!(
                        "run {id}: no shadow artifact ({}), shadow skipped",
                        f.display()
                    );
                    None
                }
                None => None,
            };
            let run = run_from_reports(id, commit, date, source, &reports);
            let tests = run.tests.len();
            match ingest(dir, run, artifact.as_ref()) {
                Ok(true) if artifact.is_some() => {
                    println!("run {id} ({commit}, {date}): {tests} tests and the shadow ingested");
                }
                Ok(true) => println!("run {id} ({commit}, {date}): {tests} tests ingested"),
                Ok(false) => println!("run {id}: already in the history, left out"),
                Err(e) => return Some(fail(&e)),
            }
            Some(ExitCode::SUCCESS)
        }
        "rebuild" => match rebuild(history?) {
            Ok(s) => {
                println!("summary rebuilt: {} runs, {} tests", s.runs, s.tests.len());
                Some(ExitCode::SUCCESS)
            }
            Err(e) => Some(fail(&e)),
        },
        "report" => {
            let summary = match history {
                Some(dir) => read_summary(dir),
                None => remote_summary(root),
            };
            match summary {
                Ok(s) if shadow_report => {
                    print!("{}", shadow::render(&s, days));
                    Some(ExitCode::SUCCESS)
                }
                Ok(s) => {
                    print!("{}", report::render(&s, options));
                    Some(ExitCode::SUCCESS)
                }
                Err(e) => Some(fail(&e)),
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_1: &str = include_str!("../fixtures/ci-history/run-1.xml");
    const RUN_2: &str = include_str!("../fixtures/ci-history/run-2.xml");

    fn fixture(id: &str, date: &str, xml: &str) -> Run {
        let report = test_times::parse_junit(xml).unwrap();
        run_from_reports(id, &format!("sha{id}"), date, "ci", &[report])
    }

    struct TempDir(std::path::PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn temp(name: &str) -> TempDir {
        let dir =
            std::env::temp_dir().join(format!("xtask-ci-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    #[test]
    fn junit_reports_fold_into_the_summary() {
        let dir = temp("fold");
        let d = &dir.0;
        assert!(ingest(d, fixture("101", "2026-11-18T10:00:00Z", RUN_1), None).unwrap());
        assert!(ingest(d, fixture("102", "2026-11-19T09:00:00Z", RUN_2), None).unwrap());
        // The same run again (overlapping nightly windows): left out.
        assert!(!ingest(d, fixture("102", "2026-11-19T09:00:00Z", RUN_2), None).unwrap());

        let s = read_summary(d).unwrap();
        assert_eq!(s.runs, 2);
        // A pass, twice.
        let pass = &s.tests["helios-core graph::tests::a_cycle_is_refused"];
        assert_eq!((pass.runs, pass.passes, pass.streak), (2, 2, 2));
        assert_eq!(pass.median_seconds(), Some(0.3));
        assert!(pass.last_failure.is_none());
        // A failure on every attempt, then a pass on the retry: one
        // failure, one flake, no streak.
        let edge = &s.tests["helios-core graph::tests::an_edge_is_kept"];
        assert_eq!(
            (edge.failures, edge.flakes, edge.passes, edge.streak),
            (1, 1, 0, 0)
        );
        assert_eq!(edge.last_failure.as_ref().unwrap().commit, "sha101");
        assert_eq!(edge.last_flake.as_ref().unwrap().run, "102");
        // The failed attempt's time is not a duration.
        assert_eq!(edge.recent_seconds, [0.12]);
        // A renamed test: the new name starts its own record, the old one
        // keeps its history and is no longer current.
        let old = &s.tests["helios-gui::it import::the_old_name"];
        let new = &s.tests["helios-gui::it import::the_new_name"];
        assert_eq!(
            (old.runs, old.last_seen, new.runs, new.last_seen),
            (1, 1, 1, 2)
        );
        assert_eq!(new.first_seen, "2026-11-19T09:00:00Z");

        // One line per run, in its day's file.
        let day = std::fs::read_to_string(d.join("days/2026-11-19.jsonl")).unwrap();
        assert_eq!(day.lines().count(), 1);
        assert!(d.join("days/2026-11-18.jsonl").exists());

        // The summary is a pure fold of the day files.
        std::fs::write(d.join(SUMMARY), "garbage").unwrap();
        assert!(read_summary(d).unwrap_err().contains("rebuild"));
        assert_eq!(rebuild(d).unwrap(), s);
        assert_eq!(read_summary(d).unwrap(), s);
    }

    #[test]
    fn the_rebuild_replays_the_fold_order() {
        let dir = temp("order");
        let d = &dir.0;
        // A run of the 18th that finished late is folded after one of the
        // 19th: the rebuild keeps that order, not the files'.
        ingest(d, fixture("2", "2026-11-19T01:00:00Z", RUN_2), None).unwrap();
        ingest(d, fixture("1", "2026-11-18T23:00:00Z", RUN_1), None).unwrap();
        let s = read_summary(d).unwrap();
        assert_eq!(s.last_run.as_ref().unwrap().run, "1");
        assert_eq!(rebuild(d).unwrap(), s);
    }

    #[test]
    fn the_shadow_counts_misses_and_skips_a_run_without_one() {
        let dir = temp("shadow");
        let d = dir.0.to_str().unwrap();
        let root = Path::new(".");
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/ci-history");
        let xml = fixtures.join("run-1.xml");
        let ingest_cli = |id: &str, date: &str, shadow: &str| {
            let shadow = fixtures.join(shadow);
            let args = [
                "ingest",
                "--history",
                d,
                "--run",
                id,
                "--commit",
                &format!("sha{id}"),
                "--date",
                date,
                "--shadow",
                shadow.to_str().unwrap(),
                xml.to_str().unwrap(),
            ];
            run(root, &args)
        };
        // A run with a miss: `an_edge_is_kept` failed, outside the set.
        assert_eq!(
            ingest_cli("201", "2026-11-20T10:00:00Z", "shadow-miss.json"),
            Some(ExitCode::SUCCESS)
        );
        // The same failure, selected this time: no miss.
        assert_eq!(
            ingest_cli("202", "2026-11-21T10:00:00Z", "shadow-clean.json"),
            Some(ExitCode::SUCCESS)
        );
        // No shadow artifact: the run is ingested, its shadow skipped.
        assert_eq!(
            ingest_cli("203", "2026-11-22T10:00:00Z", "no-such-shadow.json"),
            Some(ExitCode::SUCCESS)
        );
        // An artifact of another run is refused.
        assert_eq!(
            ingest_cli("204", "2026-11-22T11:00:00Z", "shadow-clean.json"),
            Some(ExitCode::FAILURE)
        );

        let s = read_summary(&dir.0).unwrap();
        assert_eq!((s.runs, s.shadow.runs, s.shadow.misses), (3, 2, 1));
        let miss = &s.shadow.list[0];
        assert_eq!(
            (miss.run.as_str(), miss.test.as_str()),
            ("201", "helios-core graph::tests::an_edge_is_kept")
        );
        assert_eq!(miss.changed[0], "crates/helios-gui/src/import_scan.rs");
        let first = &s.shadow.recent[0];
        assert_eq!((first.selected, first.total, first.misses), (1, 3, 1));
        let second = &s.shadow.recent[1];
        assert_eq!((second.selected, second.misses), (2, 0));
        // The day line keeps the shadow: the rebuild finds it again.
        let day = std::fs::read_to_string(dir.0.join("days/2026-11-20.jsonl")).unwrap();
        assert!(day.contains("\"shadow\":"), "{day}");
        let day = std::fs::read_to_string(dir.0.join("days/2026-11-22.jsonl")).unwrap();
        assert!(!day.contains("\"shadow\":"), "{day}");
        assert_eq!(rebuild(&dir.0).unwrap(), s);

        let text = shadow::render(&s, 7);
        assert!(text.contains("Misses: 1 "), "{text}");
        assert!(
            text.contains("- helios-core graph::tests::an_edge_is_kept  (run 201"),
            "{text}"
        );
        assert!(
            text.contains("changed: crates/helios-gui/src/import_scan.rs"),
            "{text}"
        );
        assert_eq!(
            run(root, &["report", "--shadow", "--history", d, "--days", "3"]),
            Some(ExitCode::SUCCESS)
        );
    }

    #[test]
    fn the_command_line_ingests_and_reports() {
        let dir = temp("cli");
        let d = dir.0.to_str().unwrap();
        let root = Path::new(".");
        let f = dir.0.join("r.xml");
        std::fs::write(&f, RUN_1).unwrap();
        let f = f.to_str().unwrap();
        let args = [
            "ingest",
            "--history",
            d,
            "--run",
            "9",
            "--commit",
            "abc",
            "--date",
            "2026-11-19T00:00:00Z",
            f,
        ];
        assert_eq!(run(root, &args), Some(ExitCode::SUCCESS));
        assert_eq!(
            run(root, &["report", "--history", d, "--top", "all"]),
            Some(ExitCode::SUCCESS)
        );
        assert_eq!(
            run(root, &["ingest", "--history", d, f]),
            None,
            "run, commit and date are required"
        );
        let bad_date = [
            "ingest",
            "--history",
            d,
            "--run",
            "8",
            "--commit",
            "a",
            "--date",
            "../x",
            f,
        ];
        assert_eq!(run(root, &bad_date), Some(ExitCode::FAILURE));
    }
}
