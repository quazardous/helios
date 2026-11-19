//! The history format: one line per CI run in a day file, and the summary
//! per test folded from those lines (docs/ci-broker.md, "History format").
//!
//! The fold is incremental: `Summary::fold` adds one run, so a nightly job
//! reads the summary, folds the day's runs and writes it back. Folding
//! every day file from an empty summary (`Summary::rebuild`) gives the same
//! summary, so a lost or corrupted `summary.json` is rebuilt, never patched.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;

use serde_json::{Value, json};

use super::shadow::{Shadow, ShadowSummary};

/// The format version written in `summary.json` and each run line.
pub const FORMAT: u64 = 1;
/// How many recent durations a test keeps for its median.
pub const DURATIONS_KEPT: usize = 21;
/// How many run ids the summary remembers, so a run is never folded twice.
pub const RUNS_REMEMBERED: usize = 1000;
/// A test not seen for this many runs (renamed or removed) leaves the
/// summary; its lines stay in the day files.
pub const PRUNE_AFTER_RUNS: u64 = 500;

/// What happened to a test in one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    /// Failed on every attempt.
    Fail,
    /// Failed, then passed on a retry.
    Flaky,
    /// Listed, not run (ignored or filtered out).
    Skip,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "fail",
            Outcome::Flaky => "flaky",
            Outcome::Skip => "skip",
        }
    }

    pub fn parse(s: &str) -> Option<Outcome> {
        Some(match s {
            "pass" => Outcome::Pass,
            "fail" => Outcome::Fail,
            "flaky" => Outcome::Flaky,
            "skip" => Outcome::Skip,
            _ => return None,
        })
    }

    /// Which outcome wins when one run names a test twice (two reports).
    fn severity(self) -> u8 {
        match self {
            Outcome::Skip => 0,
            Outcome::Pass => 1,
            Outcome::Flaky => 2,
            Outcome::Fail => 3,
        }
    }
}

/// One test of one run. `id` is `<binary> <name>` as nextest prints it
/// (`helios-gui::it import::a_card_imports`): the binary id holds no space.
#[derive(Clone, Debug, PartialEq)]
pub struct TestResult {
    pub id: String,
    pub outcome: Outcome,
    pub seconds: f64,
}

/// One CI run: where it ran and every test it reported.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Run {
    /// The CI run's id (GitHub's run id), unique per repository.
    pub id: String,
    pub commit: String,
    /// When the run started, UTC, `YYYY-MM-DDTHH:MM:SSZ`.
    pub date: String,
    /// Which CI produced it (`ci`, later the box).
    pub source: String,
    pub wall_seconds: Option<f64>,
    pub tests: Vec<TestResult>,
    /// The fold order, set by `Summary::fold`: a rebuild replays the lines
    /// in that order whatever day file they sit in.
    pub seq: Option<u64>,
    /// What the broker would have selected (step B3), when the run
    /// uploaded its `broker-shadow` artifact.
    pub shadow: Option<Shadow>,
}

impl Run {
    /// A run from several reports (shards): a test named twice keeps its
    /// worst outcome and its longest time; the wall time is the longest.
    pub fn new(id: &str, commit: &str, date: &str, source: &str) -> Run {
        Run {
            id: id.into(),
            commit: commit.into(),
            date: date.into(),
            source: source.into(),
            ..Run::default()
        }
    }

    pub fn add(&mut self, result: TestResult) {
        if let Some(t) = self.tests.iter_mut().find(|t| t.id == result.id) {
            if result.outcome.severity() > t.outcome.severity() {
                t.outcome = result.outcome;
            }
            t.seconds = t.seconds.max(result.seconds);
        } else {
            self.tests.push(result);
        }
    }

    pub fn add_wall(&mut self, seconds: f64) {
        self.wall_seconds = Some(self.wall_seconds.map_or(seconds, |w| w.max(seconds)));
    }

    /// The day file the run goes to: `YYYY-MM-DD` of its date.
    pub fn day(&self) -> Result<&str, String> {
        let day = self.date.get(..10).unwrap_or_default();
        let b = day.as_bytes();
        let ok = b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b.iter()
                .enumerate()
                .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
        if ok {
            Ok(day)
        } else {
            Err(format!("`{}` is not a date (YYYY-MM-DD…)", self.date))
        }
    }

    /// The run as one line of a day file.
    pub fn to_line(&self) -> String {
        let tests: Vec<Value> = self
            .tests
            .iter()
            .map(|t| json!([t.id, t.outcome.name(), round_ms(t.seconds)]))
            .collect();
        let mut line = json!({
            "format": FORMAT,
            "seq": self.seq,
            "run": self.id,
            "commit": self.commit,
            "date": self.date,
            "source": self.source,
            "wall_s": self.wall_seconds.map(round_ms),
            "tests": tests,
        });
        if let Some(shadow) = &self.shadow {
            line["shadow"] = shadow.to_json();
        }
        line.to_string()
    }

    pub fn from_line(line: &str) -> Result<Run, String> {
        let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let s = |k: &str| v[k].as_str().map(str::to_string).ok_or(format!("no `{k}`"));
        let mut run = Run {
            id: s("run")?,
            commit: s("commit")?,
            date: s("date")?,
            source: s("source").unwrap_or_default(),
            wall_seconds: v["wall_s"].as_f64(),
            tests: Vec::new(),
            seq: v["seq"].as_u64(),
            shadow: match &v["shadow"] {
                Value::Null => None,
                s => Some(Shadow::from_json(s)?),
            },
        };
        for t in v["tests"].as_array().ok_or("no `tests`")? {
            let (Some(id), Some(outcome), Some(seconds)) = (
                t[0].as_str(),
                t[1].as_str().and_then(Outcome::parse),
                t[2].as_f64(),
            ) else {
                return Err(format!("a test that does not read: {t}"));
            };
            run.tests.push(TestResult {
                id: id.into(),
                outcome,
                seconds,
            });
        }
        Ok(run)
    }
}

fn round_ms(s: f64) -> f64 {
    (s * 1000.0).round() / 1000.0
}

/// Where a test was last seen doing something: a run, its commit, its date.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Stamp {
    pub run: String,
    pub commit: String,
    pub date: String,
}

impl Stamp {
    fn of(run: &Run) -> Stamp {
        Stamp {
            run: run.id.clone(),
            commit: run.commit.clone(),
            date: run.date.clone(),
        }
    }

    fn to_json(&self) -> Value {
        json!({"run": self.run, "commit": self.commit, "date": self.date})
    }

    fn from_json(v: &Value) -> Option<Stamp> {
        Some(Stamp {
            run: v["run"].as_str()?.into(),
            commit: v["commit"].as_str()?.into(),
            date: v["date"].as_str()?.into(),
        })
    }
}

/// One test's record in the summary.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TestStats {
    /// Runs that ran it (skips aside).
    pub runs: u64,
    pub passes: u64,
    pub failures: u64,
    pub flakes: u64,
    pub skips: u64,
    /// Passes in a row since its last failure or flake.
    pub streak: u64,
    /// The date of the first run that listed it.
    pub first_seen: String,
    /// The `seq` of the last run that listed it.
    pub last_seen: u64,
    /// The date of the last run that ran it.
    pub last_run: String,
    pub last_failure: Option<Stamp>,
    pub last_flake: Option<Stamp>,
    /// Its last `DURATIONS_KEPT` times (passes and flakes), oldest first.
    pub recent_seconds: Vec<f64>,
}

impl TestStats {
    pub fn median_seconds(&self) -> Option<f64> {
        let mut v = self.recent_seconds.clone();
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        let n = v.len();
        Some(if n % 2 == 1 {
            v[n / 2]
        } else {
            f64::midpoint(v[n / 2 - 1], v[n / 2])
        })
    }

    /// Flakes per run, 0 when it never ran.
    pub fn flake_rate(&self) -> f64 {
        if self.runs == 0 {
            0.0
        } else {
            self.flakes as f64 / self.runs as f64
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "runs": self.runs,
            "passes": self.passes,
            "failures": self.failures,
            "flakes": self.flakes,
            "skips": self.skips,
            "streak": self.streak,
            "first_seen": self.first_seen,
            "last_seen": self.last_seen,
            "last_run": self.last_run,
            "last_failure": self.last_failure.as_ref().map(Stamp::to_json),
            "last_flake": self.last_flake.as_ref().map(Stamp::to_json),
            "median_s": self.median_seconds(),
            "recent_s": self.recent_seconds,
        })
    }

    fn from_json(v: &Value) -> Result<TestStats, String> {
        let n = |k: &str| v[k].as_u64().ok_or(format!("no `{k}`"));
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        Ok(TestStats {
            runs: n("runs")?,
            passes: n("passes")?,
            failures: n("failures")?,
            flakes: n("flakes")?,
            skips: n("skips")?,
            streak: n("streak")?,
            first_seen: s("first_seen"),
            last_seen: n("last_seen")?,
            last_run: s("last_run"),
            last_failure: Stamp::from_json(&v["last_failure"]),
            last_flake: Stamp::from_json(&v["last_flake"]),
            recent_seconds: v["recent_s"]
                .as_array()
                .ok_or("no `recent_s`")?
                .iter()
                .filter_map(Value::as_f64)
                .collect(),
        })
    }
}

/// The summary: every test's record, and which runs are in it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Summary {
    /// Runs folded so far; the last one's `seq`.
    pub runs: u64,
    pub last_run: Option<Stamp>,
    /// The last `RUNS_REMEMBERED` run ids folded, oldest first.
    pub ingested: VecDeque<String>,
    pub tests: BTreeMap<String, TestStats>,
    /// The shadow runs (step B3): what selection would have missed.
    pub shadow: ShadowSummary,
}

impl Summary {
    pub fn contains(&self, run: &str) -> bool {
        self.ingested.iter().any(|r| r == run)
    }

    /// Folds one run in and sets its `seq`. A run already folded changes
    /// nothing and returns false.
    pub fn fold(&mut self, run: &mut Run) -> bool {
        self.fold_with(run, PRUNE_AFTER_RUNS)
    }

    fn fold_with(&mut self, run: &mut Run, prune_after: u64) -> bool {
        if self.contains(&run.id) {
            return false;
        }
        self.runs += 1;
        let seq = self.runs;
        run.seq = Some(seq);
        let stamp = Stamp::of(run);
        for t in &run.tests {
            let s = self.tests.entry(t.id.clone()).or_default();
            if s.first_seen.is_empty() {
                s.first_seen.clone_from(&run.date);
            }
            s.last_seen = seq;
            if t.outcome == Outcome::Skip {
                s.skips += 1;
                continue;
            }
            s.runs += 1;
            s.last_run.clone_from(&run.date);
            match t.outcome {
                Outcome::Pass => {
                    s.passes += 1;
                    s.streak += 1;
                }
                Outcome::Flaky => {
                    s.flakes += 1;
                    s.streak = 0;
                    s.last_flake = Some(stamp.clone());
                }
                Outcome::Fail => {
                    s.failures += 1;
                    s.streak = 0;
                    s.last_failure = Some(stamp.clone());
                }
                Outcome::Skip => unreachable!(),
            }
            // A failure's time says when it stopped, not how long it takes.
            if t.outcome != Outcome::Fail {
                s.recent_seconds.push(round_ms(t.seconds));
                let extra = s.recent_seconds.len().saturating_sub(DURATIONS_KEPT);
                s.recent_seconds.drain(..extra);
            }
        }
        self.tests
            .retain(|_, s| seq.saturating_sub(s.last_seen) < prune_after);
        self.shadow.fold(run);
        self.last_run = Some(stamp);
        self.ingested.push_back(run.id.clone());
        while self.ingested.len() > RUNS_REMEMBERED {
            self.ingested.pop_front();
        }
        true
    }

    /// The summary of every run of the day files, replayed in fold order
    /// (their `seq`; lines without one keep their place).
    pub fn rebuild(mut runs: Vec<Run>) -> Summary {
        runs.sort_by_key(|r| r.seq.unwrap_or(u64::MAX));
        let mut summary = Summary::default();
        for mut run in runs {
            summary.fold(&mut run);
        }
        summary
    }

    /// `summary.json`: a small header, then one line per test, so a day's
    /// change reads as a diff of the tests that ran.
    pub fn to_json(&self) -> String {
        let mut out = String::from("{\n");
        let _ = writeln!(out, "  \"format\": {FORMAT},");
        let _ = writeln!(out, "  \"runs\": {},", self.runs);
        let _ = writeln!(
            out,
            "  \"last_run\": {},",
            self.last_run.as_ref().map_or(Value::Null, Stamp::to_json)
        );
        let _ = writeln!(out, "  \"ingested\": {},", json!(self.ingested));
        let _ = writeln!(out, "  \"shadow\": {},", self.shadow.to_json());
        out.push_str("  \"tests\": {");
        for (i, (id, s)) in self.tests.iter().enumerate() {
            out.push_str(if i == 0 { "\n" } else { ",\n" });
            let _ = write!(out, "    {}: {}", json!(id), s.to_json());
        }
        out.push_str(if self.tests.is_empty() {
            "}\n}\n"
        } else {
            "\n  }\n}\n"
        });
        out
    }

    pub fn from_json(text: &str) -> Result<Summary, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if v["format"].as_u64() != Some(FORMAT) {
            return Err(format!("format {} is not {FORMAT}", v["format"]));
        }
        let mut tests = BTreeMap::new();
        for (id, t) in v["tests"].as_object().ok_or("no `tests`")? {
            tests.insert(
                id.clone(),
                TestStats::from_json(t).map_err(|e| format!("{id}: {e}"))?,
            );
        }
        Ok(Summary {
            runs: v["runs"].as_u64().ok_or("no `runs`")?,
            last_run: Stamp::from_json(&v["last_run"]),
            ingested: v["ingested"]
                .as_array()
                .ok_or("no `ingested`")?
                .iter()
                .filter_map(|r| r.as_str().map(str::to_string))
                .collect(),
            tests,
            shadow: ShadowSummary::from_json(&v["shadow"])?,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn run(id: &str, date: &str, tests: &[(&str, Outcome, f64)]) -> Run {
        let mut r = Run::new(id, &format!("c{id}"), date, "ci");
        for (name, outcome, seconds) in tests {
            r.add(TestResult {
                id: (*name).into(),
                outcome: *outcome,
                seconds: *seconds,
            });
        }
        r
    }

    #[test]
    fn a_run_line_reads_back() {
        let mut r = run(
            "7",
            "2026-11-19T10:00:00Z",
            &[
                ("b a", Outcome::Pass, 0.1234),
                ("b \"q\"", Outcome::Flaky, 1.0),
            ],
        );
        r.wall_seconds = Some(3.0);
        r.seq = Some(4);
        let back = Run::from_line(&r.to_line()).unwrap();
        assert_eq!(back.tests[0].seconds, 0.123);
        assert_eq!(back.tests[1].outcome, Outcome::Flaky);
        assert_eq!((back.seq, back.wall_seconds), (Some(4), Some(3.0)));
        assert_eq!(back.day(), Ok("2026-11-19"));
        assert!(Run::from_line("{}").is_err());
        let mut bad = r.clone();
        bad.date = "../../etc".into();
        assert!(bad.day().is_err());
    }

    #[test]
    fn a_test_in_two_reports_keeps_its_worst_outcome() {
        let r = run(
            "1",
            "2026-11-19",
            &[("t", Outcome::Pass, 1.0), ("t", Outcome::Fail, 0.5)],
        );
        assert_eq!(r.tests.len(), 1);
        assert_eq!(
            (r.tests[0].outcome, r.tests[0].seconds),
            (Outcome::Fail, 1.0)
        );
    }

    #[test]
    fn a_run_is_folded_once() {
        let mut s = Summary::default();
        let mut r = run("1", "2026-11-19", &[("t", Outcome::Pass, 1.0)]);
        assert!(s.fold(&mut r.clone()));
        assert!(!s.fold(&mut r));
        assert_eq!((s.runs, s.tests["t"].runs), (1, 1));
    }

    #[test]
    fn a_test_unseen_for_long_leaves_the_summary() {
        let mut s = Summary::default();
        s.fold_with(&mut run("1", "d", &[("old", Outcome::Pass, 1.0)]), 3);
        for i in 2..=3 {
            s.fold_with(
                &mut run(&i.to_string(), "d", &[("new", Outcome::Pass, 1.0)]),
                3,
            );
            assert!(s.tests.contains_key("old"), "kept at run {i}");
        }
        s.fold_with(&mut run("4", "d", &[("new", Outcome::Pass, 1.0)]), 3);
        assert!(!s.tests.contains_key("old"));
    }

    #[test]
    fn the_summary_reads_back_and_the_median_keeps_the_last_times() {
        let mut s = Summary::default();
        for i in 0..30 {
            let seconds = if i < 9 { 100.0 } else { f64::from(i % 3) };
            s.fold(&mut run(
                &i.to_string(),
                "d",
                &[("t", Outcome::Pass, seconds)],
            ));
        }
        let t = &s.tests["t"];
        assert_eq!(t.recent_seconds.len(), DURATIONS_KEPT);
        assert_eq!(t.median_seconds(), Some(1.0));
        let back = Summary::from_json(&s.to_json()).unwrap();
        assert_eq!(back, s);
        assert_eq!(
            Summary::from_json(&Summary::default().to_json()).unwrap(),
            Summary::default()
        );
        assert!(Summary::from_json("{\"format\": 99}").is_err());
    }
}
