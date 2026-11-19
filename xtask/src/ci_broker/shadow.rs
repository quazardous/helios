//! Step B3 of the broker (docs/ci-broker.md, "Shadow week"): what the
//! always-run set would have missed.
//!
//! On every push to `next`, CI runs the full suite and, beside it, records
//! the tests `ci-broker select` would have chosen (the `broker-shadow`
//! artifact, [`Artifact`]). The nightly ingest reads it next to the JUnit
//! report and keeps, per run, a [`Shadow`]: how much was selected and the
//! **misses**, the tests that failed in the full suite but were not
//! selected. The summary folds them into a [`ShadowSummary`], and
//! [`render`] reports the window.
//!
//! Like the rest of `ci_broker`, it knows nothing of Helios.

use std::collections::{BTreeSet, VecDeque};
use std::fmt::Write as _;

use serde_json::{Value, json};

use super::history::{Outcome, Run, Summary};
use super::select::Selection;

/// The format of the `broker-shadow` artifact.
pub const ARTIFACT_FORMAT: u64 = 1;
/// How many shadow runs, and how many misses, the summary keeps.
pub const SHADOW_KEPT: usize = 500;

/// The `broker-shadow` artifact one CI run uploads: what `select` chose.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Artifact {
    pub run: String,
    pub commit: String,
    /// The commit the change was computed from (the push's `before`).
    pub base: String,
    /// The selection holds `all()`: every test.
    pub everything: bool,
    /// The selected test ids (`<binary id> <test name>`), among the tests
    /// the run's JUnit report lists.
    pub selected: BTreeSet<String>,
    /// The files that changed since `base`.
    pub changed: Vec<String>,
}

impl Artifact {
    pub fn from_json(text: &str) -> Result<Artifact, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if v["format"].as_u64() != Some(ARTIFACT_FORMAT) {
            return Err(format!("format {} is not {ARTIFACT_FORMAT}", v["format"]));
        }
        let s = |k: &str| v[k].as_str().map(str::to_string).ok_or(format!("no `{k}`"));
        let list = |k: &str| -> Result<Vec<String>, String> {
            Ok(v[k]
                .as_array()
                .ok_or(format!("no `{k}`"))?
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect())
        };
        Ok(Artifact {
            run: s("run")?,
            commit: s("commit")?,
            base: s("base")?,
            everything: v["everything"].as_bool().ok_or("no `everything`")?,
            selected: list("selected")?.into_iter().collect(),
            changed: list("changed")?,
        })
    }
}

/// The `broker-shadow` artifact for a selection: `tests` are the ids
/// the run's JUnit report lists, of which those the selection holds go
/// under `selected`. `sources` keeps each pick (its source, filter and
/// why), for a human reading the artifact.
pub fn artifact_json(
    run: &str,
    commit: &str,
    base: &str,
    selection: &Selection,
    changed: &[String],
    tests: &[String],
) -> String {
    let selected: BTreeSet<&String> = tests
        .iter()
        .filter(|id| {
            id.split_once(' ')
                .is_some_and(|(b, n)| selection.selects(b, n.trim()))
        })
        .collect();
    let sources: Vec<Value> = selection
        .picks
        .iter()
        .map(|p| json!({"source": p.source.name(), "filter": p.scope.filter(), "why": p.why}))
        .collect();
    let notes: Vec<Value> = selection
        .notes
        .iter()
        .map(|(s, n)| json!({"source": s.name(), "note": n}))
        .collect();
    let v = json!({
        "format": ARTIFACT_FORMAT,
        "run": run,
        "commit": commit,
        "base": base,
        "everything": selection.everything(),
        "filter": selection.filter(),
        "tests": tests.len(),
        "selected": selected,
        "changed": changed,
        "sources": sources,
        "notes": notes,
    });
    serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
}

/// One run's shadow: written in its day file line, under `shadow`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Shadow {
    pub base: String,
    pub everything: bool,
    /// Tests the run ran (skips aside), and those of them selected.
    pub total: u64,
    pub selected: u64,
    /// The same, in test-seconds: each test's median time before this
    /// run (its own time when it has none yet).
    pub total_s: f64,
    pub selected_s: f64,
    /// Tests that failed on every attempt and were not selected.
    pub misses: Vec<String>,
    pub changed: Vec<String>,
}

fn round_ms(s: f64) -> f64 {
    (s * 1000.0).round() / 1000.0
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

impl Shadow {
    /// The run measured against what the artifact selected. `summary` is
    /// the history before the run, for the median times.
    pub fn measure(summary: &Summary, run: &Run, artifact: &Artifact) -> Shadow {
        let mut s = Shadow {
            base: artifact.base.clone(),
            everything: artifact.everything,
            changed: artifact.changed.clone(),
            ..Shadow::default()
        };
        for t in &run.tests {
            if t.outcome == Outcome::Skip {
                continue;
            }
            let seconds = summary
                .tests
                .get(&t.id)
                .and_then(super::history::TestStats::median_seconds)
                .unwrap_or(t.seconds);
            s.total += 1;
            s.total_s += seconds;
            if artifact.everything || artifact.selected.contains(&t.id) {
                s.selected += 1;
                s.selected_s += seconds;
            } else if t.outcome == Outcome::Fail {
                s.misses.push(t.id.clone());
            }
        }
        s.total_s = round_ms(s.total_s);
        s.selected_s = round_ms(s.selected_s);
        s
    }

    pub fn to_json(&self) -> Value {
        json!({
            "base": self.base,
            "everything": self.everything,
            "total": self.total,
            "selected": self.selected,
            "total_s": self.total_s,
            "selected_s": self.selected_s,
            "misses": self.misses,
            "changed": self.changed,
        })
    }

    pub fn from_json(v: &Value) -> Result<Shadow, String> {
        let n = |k: &str| v[k].as_u64().ok_or(format!("shadow: no `{k}`"));
        let f = |k: &str| v[k].as_f64().ok_or(format!("shadow: no `{k}`"));
        Ok(Shadow {
            base: v["base"].as_str().unwrap_or_default().into(),
            everything: v["everything"].as_bool().unwrap_or(false),
            total: n("total")?,
            selected: n("selected")?,
            total_s: f("total_s")?,
            selected_s: f("selected_s")?,
            misses: strings(&v["misses"]),
            changed: strings(&v["changed"]),
        })
    }
}

/// One shadow run in the summary: the counts, without the test lists.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ShadowRun {
    pub run: String,
    pub commit: String,
    pub date: String,
    pub base: String,
    pub everything: bool,
    pub total: u64,
    pub selected: u64,
    pub total_s: f64,
    pub selected_s: f64,
    pub misses: u64,
}

/// One miss: a test that failed in the full suite, outside the selection.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Miss {
    pub run: String,
    pub commit: String,
    pub date: String,
    pub test: String,
    /// The files that changed in that run, to write an `OVERRIDES` line.
    pub changed: Vec<String>,
}

/// The summary's `shadow` section.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ShadowSummary {
    /// Shadow runs folded, and misses found, since the history began.
    pub runs: u64,
    pub misses: u64,
    /// The last `SHADOW_KEPT` misses, oldest first.
    pub list: VecDeque<Miss>,
    /// The last `SHADOW_KEPT` shadow runs, oldest first.
    pub recent: VecDeque<ShadowRun>,
}

fn keep<T>(q: &mut VecDeque<T>) {
    while q.len() > SHADOW_KEPT {
        q.pop_front();
    }
}

impl ShadowSummary {
    pub fn fold(&mut self, run: &Run) {
        let Some(s) = &run.shadow else { return };
        self.runs += 1;
        self.misses += s.misses.len() as u64;
        for test in &s.misses {
            self.list.push_back(Miss {
                run: run.id.clone(),
                commit: run.commit.clone(),
                date: run.date.clone(),
                test: test.clone(),
                changed: s.changed.clone(),
            });
        }
        self.recent.push_back(ShadowRun {
            run: run.id.clone(),
            commit: run.commit.clone(),
            date: run.date.clone(),
            base: s.base.clone(),
            everything: s.everything,
            total: s.total,
            selected: s.selected,
            total_s: s.total_s,
            selected_s: s.selected_s,
            misses: s.misses.len() as u64,
        });
        keep(&mut self.list);
        keep(&mut self.recent);
    }

    /// The section as it sits in `summary.json`: one line per entry.
    pub fn to_json(&self) -> String {
        let lines = |items: Vec<Value>| {
            if items.is_empty() {
                return "[]".to_string();
            }
            let body: Vec<String> = items.iter().map(|v| format!("    {v}")).collect();
            format!("[\n{}\n  ]", body.join(",\n"))
        };
        let list = self
            .list
            .iter()
            .map(|m| {
                json!({"run": m.run, "commit": m.commit, "date": m.date, "test": m.test, "changed": m.changed})
            })
            .collect();
        let recent = self
            .recent
            .iter()
            .map(|r| {
                json!({
                    "run": r.run, "commit": r.commit, "date": r.date, "base": r.base,
                    "everything": r.everything, "total": r.total, "selected": r.selected,
                    "total_s": r.total_s, "selected_s": r.selected_s, "misses": r.misses,
                })
            })
            .collect();
        format!(
            "{{\"runs\": {}, \"misses\": {}, \"list\": {}, \"recent\": {}}}",
            self.runs,
            self.misses,
            lines(list),
            lines(recent)
        )
    }

    /// Absent (a summary from before B3): empty.
    pub fn from_json(v: &Value) -> Result<ShadowSummary, String> {
        if v.is_null() {
            return Ok(ShadowSummary::default());
        }
        let s = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
        let list = v["list"]
            .as_array()
            .ok_or("shadow: no `list`")?
            .iter()
            .map(|m| Miss {
                run: s(m, "run"),
                commit: s(m, "commit"),
                date: s(m, "date"),
                test: s(m, "test"),
                changed: strings(&m["changed"]),
            })
            .collect();
        let recent = v["recent"]
            .as_array()
            .ok_or("shadow: no `recent`")?
            .iter()
            .map(|r| ShadowRun {
                run: s(r, "run"),
                commit: s(r, "commit"),
                date: s(r, "date"),
                base: s(r, "base"),
                everything: r["everything"].as_bool().unwrap_or(false),
                total: r["total"].as_u64().unwrap_or(0),
                selected: r["selected"].as_u64().unwrap_or(0),
                total_s: r["total_s"].as_f64().unwrap_or(0.0),
                selected_s: r["selected_s"].as_f64().unwrap_or(0.0),
                misses: r["misses"].as_u64().unwrap_or(0),
            })
            .collect();
        Ok(ShadowSummary {
            runs: v["runs"].as_u64().ok_or("shadow: no `runs`")?,
            misses: v["misses"].as_u64().ok_or("shadow: no `misses`")?,
            list,
            recent,
        })
    }
}

/// Days since 1970-01-01 of a `YYYY-MM-DD…` date (Howard Hinnant's
/// `days_from_civil`).
pub fn day_number(date: &str) -> Option<i64> {
    let y: i64 = date.get(0..4)?.parse().ok()?;
    let m: i64 = date.get(5..7)?.parse().ok()?;
    let d: i64 = date.get(8..10)?.parse().ok()?;
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The shadow report over the last `days` days (up to the last shadow
/// run): the selected share, the time it would have saved, every miss
/// with the files its run changed.
pub fn render(summary: &Summary, days: i64) -> String {
    let mut out = String::new();
    let sh = &summary.shadow;
    let Some(last) = sh.recent.back().and_then(|r| day_number(&r.date)) else {
        let _ = writeln!(
            out,
            "No shadow run in the history yet (the `broker-shadow` artifact of next's CI runs)."
        );
        return out;
    };
    let since = last - days + 1;
    let in_window = |date: &str| day_number(date).is_some_and(|d| d >= since);
    let runs: Vec<&ShadowRun> = sh.recent.iter().filter(|r| in_window(&r.date)).collect();
    let misses: Vec<&Miss> = sh.list.iter().filter(|m| in_window(&m.date)).collect();
    let first = runs.first().map_or("", |r| r.date.get(..10).unwrap_or(""));
    let last_date = runs.last().map_or("", |r| r.date.get(..10).unwrap_or(""));
    let (total, selected) = runs
        .iter()
        .fold((0, 0), |(t, s), r| (t + r.total, s + r.selected));
    let (total_s, selected_s) = runs
        .iter()
        .fold((0.0, 0.0), |(t, s), r| (t + r.total_s, s + r.selected_s));
    let everything = runs.iter().filter(|r| r.everything).count();
    let pct = |a: f64, b: f64| if b > 0.0 { a * 100.0 / b } else { 0.0 };
    let _ = writeln!(
        out,
        "Shadow, last {days} days: {} runs, {first} to {last_date} ({} shadow runs and {} misses in all).",
        runs.len(),
        sh.runs,
        sh.misses
    );
    let _ = writeln!(
        out,
        "Selected: {selected} of {total} tests run ({:.1} %); {everything} runs selected every test.",
        pct(selected as f64, total as f64)
    );
    let _ = writeln!(
        out,
        "Test time: {selected_s:.0} of {total_s:.0} test-seconds selected; about {:.0} s saved ({:.1} %), from the median times.",
        total_s - selected_s,
        pct(total_s - selected_s, total_s)
    );
    let missed_runs: BTreeSet<&str> = misses.iter().map(|m| m.run.as_str()).collect();
    let _ = writeln!(
        out,
        "\nMisses: {} (failed in the full suite, not selected), in {} runs.",
        misses.len(),
        missed_runs.len()
    );
    for m in &misses {
        let short: String = m.commit.chars().take(10).collect();
        let _ = writeln!(out, "- {}  (run {}, {short}, {})", m.test, m.run, m.date);
        let shown: Vec<&str> = m.changed.iter().take(20).map(String::as_str).collect();
        let more = m.changed.len().saturating_sub(shown.len());
        let tail = if more > 0 {
            format!(", … {more} more")
        } else {
            String::new()
        };
        let _ = writeln!(out, "    changed: {}{tail}", shown.join(", "));
    }
    if !misses.is_empty() {
        let _ = writeln!(
            out,
            "\nA miss on a real break: add an `OVERRIDES` line (xtask/src/ci_select.rs) from a changed file to the test's module."
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci_broker::history::Outcome::{Fail, Flaky, Pass};
    use crate::ci_broker::history::tests::run;

    fn artifact(selected: &[&str]) -> Artifact {
        Artifact {
            run: "1".into(),
            commit: "c1".into(),
            base: "c0".into(),
            everything: false,
            selected: selected.iter().map(|s| (*s).to_string()).collect(),
            changed: vec!["crates/a/src/lib.rs".into()],
        }
    }

    #[test]
    fn a_failure_outside_the_selection_is_a_miss() {
        let r = run(
            "1",
            "2026-11-20T10:00:00Z",
            &[
                ("a x", Pass, 1.0),
                ("a broke", Fail, 2.0),
                ("b flaky", Flaky, 3.0),
                ("b broke", Fail, 4.0),
            ],
        );
        let s = Shadow::measure(&Summary::default(), &r, &artifact(&["a x", "a broke"]));
        assert_eq!((s.total, s.selected), (4, 2));
        assert_eq!((s.total_s, s.selected_s), (10.0, 3.0));
        // A flake passed on its retry: not a miss.
        assert_eq!(s.misses, ["b broke"]);
        let back = Shadow::from_json(&s.to_json()).unwrap();
        assert_eq!(back, s);

        let mut every = artifact(&[]);
        every.everything = true;
        let s = Shadow::measure(&Summary::default(), &r, &every);
        assert!(s.misses.is_empty());
        assert_eq!(s.selected, 4);
    }

    #[test]
    fn the_report_reads_the_window() {
        let mut summary = Summary::default();
        for (id, day, misses) in [
            ("1", "2026-11-01T10:00:00Z", vec!["old miss".to_string()]),
            ("2", "2026-11-20T10:00:00Z", vec![]),
            ("3", "2026-11-21T10:00:00Z", vec!["b broke".to_string()]),
        ] {
            let mut r = run(id, day, &[("a x", Pass, 1.0)]);
            r.shadow = Some(Shadow {
                base: "c0".into(),
                everything: false,
                total: 10,
                selected: 4,
                total_s: 100.0,
                selected_s: 30.0,
                misses,
                changed: vec!["crates/a/src/lib.rs".into()],
            });
            summary.fold(&mut r);
        }
        assert_eq!((summary.shadow.runs, summary.shadow.misses), (3, 2));
        let text = render(&summary, 7);
        assert!(text.contains("2 runs, 2026-11-20 to 2026-11-21"), "{text}");
        assert!(
            text.contains("Selected: 8 of 20 tests run (40.0 %)"),
            "{text}"
        );
        assert!(text.contains("about 140 s saved (70.0 %)"), "{text}");
        assert!(text.contains("Misses: 1 "), "{text}");
        assert!(text.contains("- b broke  (run 3, c3,"), "{text}");
        assert!(text.contains("changed: crates/a/src/lib.rs"), "{text}");
        assert!(!text.contains("old miss"), "{text}");
        assert!(render(&Summary::default(), 7).contains("No shadow run"));
        assert_eq!(day_number("1970-01-01"), Some(0));
        assert_eq!(
            day_number("2026-03-01"),
            Some(day_number("2026-02-28").unwrap() + 1)
        );
    }
}
