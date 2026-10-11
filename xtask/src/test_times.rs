//! `cargo xtask test-times`: how long each test takes, a history of runs,
//! and the tests getting slower.
//!
//! - `test-times [--profile P] [--top N]` runs the workspace's tests with
//!   cargo-nextest (profile `default` unless told), reads the JUnit report
//!   nextest writes to `target/nextest/<P>/junit.xml`, appends the run to
//!   `target/test-times/history.jsonl` (one JSON line per run: date, git
//!   commit, each test's name, duration and status), then prints the N
//!   slowest tests (15 by default), the total time per crate, and the tests
//!   that got slower than usual.
//! - `test-times --from-junit FILE [--top N] [--summary]` reads an existing
//!   report instead, without running or recording anything; `--summary`
//!   prints Markdown (CI appends it to the job summary).
//!
//! A test got slower when it took more than 50 % and more than 0.5 s longer
//! than the median of its last five recorded runs (with at least three of
//! them). The whole suite taking 20 % longer than its median is reported
//! too. Both are warnings: timings vary from one machine and load to the
//! next.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde_json::{Value, json};

pub const USAGE: &str =
    "test-times [--profile P] [--top N] | test-times --from-junit FILE [--top N] [--summary]";

/// How many earlier runs the median looks at, and how many it needs.
const WINDOW: usize = 5;
const MIN_RUNS: usize = 3;
/// A test is slower when it is above `median * RATIO` and `median + MARGIN_S`.
const RATIO: f64 = 1.5;
const MARGIN_S: f64 = 0.5;
/// The suite is slower when its total is above `median * SUITE_RATIO`.
const SUITE_RATIO: f64 = 1.2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Passed,
    Failed,
    Skipped,
    /// Failed, then passed on a retry.
    Flaky,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Status::Passed => "passed",
            Status::Failed => "failed",
            Status::Skipped => "skipped",
            Status::Flaky => "flaky",
        }
    }
}

/// One test of a run.
#[derive(Clone, Debug, PartialEq)]
pub struct Case {
    /// The binary, as nextest names it: `crate`, `crate::test_file` or
    /// `crate::bin/name`.
    pub binary: String,
    pub name: String,
    pub seconds: f64,
    pub status: Status,
}

impl Case {
    /// The crate the test belongs to.
    fn krate(&self) -> &str {
        self.binary.split("::").next().unwrap_or(&self.binary)
    }

    /// The name nextest prints: `binary test::path`.
    fn full_name(&self) -> String {
        format!("{} {}", self.binary, self.name)
    }
}

/// A JUnit report: the tests and the run's wall time, when given.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub cases: Vec<Case>,
    pub wall_seconds: Option<f64>,
}

/// Reads a JUnit report as cargo-nextest writes it. Only what the timings
/// need: `testsuites`' time, each `testcase`'s name, class name and time,
/// and whether it holds a failure, an error, a skip or a flaky failure.
pub fn parse_junit(xml: &str) -> Result<Report, String> {
    let mut report = Report::default();
    let mut open: Option<Case> = None;
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        rest = &rest[start..];
        // Content that is not markup: skip it whole.
        for (opening, closing) in [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")] {
            if rest.starts_with(opening) {
                let end = rest
                    .find(closing)
                    .ok_or_else(|| format!("unterminated `{opening}`"))?;
                rest = &rest[end + closing.len()..];
            }
        }
        if !rest.starts_with('<') || rest.starts_with("<!") || rest.starts_with("<?") {
            continue;
        }
        let end = tag_end(rest).ok_or("unterminated tag")?;
        let tag = &rest[1..end];
        rest = &rest[end + 1..];

        if let Some(name) = tag.strip_prefix('/') {
            if name.trim() == "testcase" {
                let case = open.take().ok_or("`</testcase>` without `<testcase>`")?;
                report.cases.push(case);
            }
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let (element, attributes) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
        let attr = |key: &str| attribute(attributes, key);
        match element {
            "testsuites" => {
                report.wall_seconds = attr("time").and_then(|t| t.parse().ok());
            }
            "testcase" => {
                if open.is_some() {
                    return Err("a `<testcase>` inside another".into());
                }
                let name = attr("name").ok_or("a `<testcase>` without a name")?;
                let case = Case {
                    binary: attr("classname").unwrap_or_default(),
                    seconds: attr("time").and_then(|t| t.parse().ok()).unwrap_or(0.0),
                    status: Status::Passed,
                    name,
                };
                if self_closing {
                    report.cases.push(case);
                } else {
                    open = Some(case);
                }
            }
            "failure" | "error" => {
                if let Some(case) = &mut open {
                    case.status = Status::Failed;
                }
            }
            "skipped" => {
                if let Some(case) = &mut open {
                    case.status = Status::Skipped;
                }
            }
            "flakyFailure" | "flakyError" => {
                if let Some(case) = open.as_mut().filter(|c| c.status == Status::Passed) {
                    case.status = Status::Flaky;
                }
            }
            _ => {}
        }
    }
    if open.is_some() {
        return Err("a `<testcase>` never closed".into());
    }
    Ok(report)
}

/// The index of the `>` closing the tag `s` starts with, outside quotes.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if q == c => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

/// The unescaped value of `key="…"` among a tag's attributes.
fn attribute(attributes: &str, key: &str) -> Option<String> {
    let mut rest = attributes;
    loop {
        rest = rest.trim_start();
        let (name, after) = rest.split_once('=')?;
        let after = after.trim_start();
        let quote = after.chars().next()?;
        let value_end = after[1..].find(quote)? + 1;
        if name.trim() == key {
            return Some(unescape(&after[1..value_end]));
        }
        rest = &after[value_end + 1..];
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else { break };
        let entity = &rest[1..end];
        let c = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .map(|h| u32::from_str_radix(h, 16))
                .or_else(|| entity.strip_prefix('#').map(str::parse))
                .and_then(Result::ok)
                .and_then(char::from_u32),
        };
        if let Some(c) = c {
            out.push(c);
            rest = &rest[end + 1..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// One recorded run, as a line of the history.
fn history_line(report: &Report, date: &str, commit: &str, profile: &str) -> String {
    let tests: Vec<Value> = report
        .cases
        .iter()
        .map(|c| {
            json!({
                "name": c.full_name(),
                "duration_s": (c.seconds * 1000.0).round() / 1000.0,
                "status": c.status.name(),
            })
        })
        .collect();
    json!({
        "date": date,
        "commit": commit,
        "profile": profile,
        "total_s": report.wall_seconds,
        "tests": tests,
    })
    .to_string()
}

/// The earlier runs: for each test, its durations, oldest first; and the
/// suite's totals. Lines that do not read are ignored.
#[derive(Debug, Default)]
pub struct History {
    tests: BTreeMap<String, Vec<f64>>,
    totals: Vec<f64>,
}

impl History {
    pub fn parse(jsonl: &str) -> History {
        let mut history = History::default();
        for run in jsonl
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        {
            if let Some(total) = run["total_s"].as_f64() {
                history.totals.push(total);
            }
            for t in run["tests"].as_array().into_iter().flatten() {
                let (Some(name), Some(d)) = (t["name"].as_str(), t["duration_s"].as_f64()) else {
                    continue;
                };
                if t["status"].as_str() == Some("passed") {
                    history.tests.entry(name.to_string()).or_default().push(d);
                }
            }
        }
        history
    }
}

/// The median of the last `WINDOW` values, if there are `MIN_RUNS` of them.
fn recent_median(values: &[f64]) -> Option<f64> {
    if values.len() < MIN_RUNS {
        return None;
    }
    let mut last = values[values.len().saturating_sub(WINDOW)..].to_vec();
    last.sort_by(f64::total_cmp);
    let n = last.len();
    Some(if n % 2 == 1 {
        last[n / 2]
    } else {
        f64::midpoint(last[n / 2 - 1], last[n / 2])
    })
}

/// A test slower than usual: its time now and its recent median.
#[derive(Debug, PartialEq)]
pub struct Slower {
    pub name: String,
    pub seconds: f64,
    pub median: f64,
}

/// The passed tests of `report` that got slower than their recent median.
pub fn slower_tests(report: &Report, history: &History) -> Vec<Slower> {
    let mut slower: Vec<Slower> = report
        .cases
        .iter()
        .filter(|c| c.status == Status::Passed)
        .filter_map(|c| {
            let name = c.full_name();
            let median = recent_median(history.tests.get(&name)?)?;
            (c.seconds > median * RATIO && c.seconds > median + MARGIN_S).then_some(Slower {
                name,
                seconds: c.seconds,
                median,
            })
        })
        .collect();
    slower.sort_by(|a, b| (b.seconds - b.median).total_cmp(&(a.seconds - a.median)));
    slower
}

/// The suite's recent median total, when `total` is 20 % above it.
pub fn slower_suite(total: f64, history: &History) -> Option<f64> {
    recent_median(&history.totals).filter(|m| total > m * SUITE_RATIO)
}

/// The slowest tests, then the time per crate, then what got slower.
fn render(
    report: &Report,
    top: usize,
    slower: &[Slower],
    suite: Option<f64>,
    markdown: bool,
) -> String {
    let mut cases: Vec<&Case> = report.cases.iter().collect();
    cases.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
    let mut crates: BTreeMap<&str, (usize, f64)> = BTreeMap::new();
    for c in &report.cases {
        let e = crates.entry(c.krate()).or_default();
        e.0 += 1;
        e.1 += c.seconds;
    }
    let mut crates: Vec<_> = crates.into_iter().collect();
    crates.sort_by(|a, b| b.1.1.total_cmp(&a.1.1));
    let sum: f64 = report.cases.iter().map(|c| c.seconds).sum();

    let mut out = String::new();
    let wall = report
        .wall_seconds
        .map_or(String::new(), |w| format!(", {w:.1} s wall time"));
    if markdown {
        let _ = writeln!(out, "### Slowest tests\n");
        let _ = writeln!(
            out,
            "{} tests, {sum:.1} s of test time{wall}.\n",
            report.cases.len()
        );
        let _ = writeln!(out, "| Test | Time | Status |\n|------|-----:|--------|");
        for c in cases.iter().take(top) {
            let _ = writeln!(
                out,
                "| `{}` | {:.2} s | {} |",
                c.full_name(),
                c.seconds,
                c.status.name()
            );
        }
        let _ = writeln!(out, "\n| Crate | Tests | Time |\n|-------|------:|-----:|");
        for (name, (n, s)) in &crates {
            let _ = writeln!(out, "| {name} | {n} | {s:.2} s |");
        }
    } else {
        let _ = writeln!(
            out,
            "{} tests, {sum:.1} s of test time{wall}.\n",
            report.cases.len()
        );
        let _ = writeln!(out, "Slowest {}:", top.min(cases.len()));
        for c in cases.iter().take(top) {
            let status = if c.status == Status::Passed {
                String::new()
            } else {
                format!("  ({})", c.status.name())
            };
            let _ = writeln!(out, "{:>8.2} s  {}{status}", c.seconds, c.full_name());
        }
        let _ = writeln!(out, "\nPer crate:");
        for (name, (n, s)) in &crates {
            let _ = writeln!(out, "{s:>8.2} s  {name} ({n} tests)");
        }
    }
    if !slower.is_empty() {
        let _ = writeln!(out, "\nSlower than the median of their last {WINDOW} runs:");
        for s in slower {
            let _ = writeln!(
                out,
                "{:>8.2} s  {} (median {:.2} s)",
                s.seconds, s.name, s.median
            );
        }
    }
    if let (Some(median), Some(wall)) = (suite, report.wall_seconds) {
        let _ = writeln!(
            out,
            "\nwarning: the suite took {wall:.1} s, more than 20 % above its recent median of {median:.1} s."
        );
    }
    out
}

/// The current time, UTC, as `YYYY-MM-DDTHH:MM:SSZ`.
fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after 1970")
        .as_secs();
    let s = secs % 86_400;
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        crate::civil((secs / 86_400).cast_signed()),
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

fn commit(root: &Path) -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || "unknown".into(),
            |o| String::from_utf8_lossy(&o.stdout).trim().to_string(),
        )
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from)
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let (mut profile, mut top, mut junit, mut summary) = ("default", 15, None, false);
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match *a {
            "--profile" => profile = args.next()?,
            "--top" => top = args.next()?.parse().ok()?,
            "--from-junit" => junit = Some(PathBuf::from(args.next()?)),
            "--summary" => summary = true,
            _ => return None,
        }
    }
    if summary && junit.is_none() {
        return None;
    }

    let mut code = ExitCode::SUCCESS;
    let recorded = junit.is_none();
    let path = if let Some(path) = junit {
        path
    } else {
        let status = Command::new("cargo")
            .args(["nextest", "run", "--workspace", "--profile", profile])
            .args(["--no-fail-fast"])
            .current_dir(root)
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(_) => code = ExitCode::FAILURE,
            Err(e) => {
                eprintln!(
                    "cargo nextest did not run ({e}): install it with `cargo install --locked cargo-nextest`"
                );
                return Some(ExitCode::FAILURE);
            }
        }
        target_dir(root)
            .join("nextest")
            .join(profile)
            .join("junit.xml")
    };
    let report = match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|xml| parse_junit(&xml))
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            return Some(ExitCode::FAILURE);
        }
    };

    let (mut slower, mut suite) = (Vec::new(), None);
    if recorded {
        let dir = target_dir(root).join("test-times");
        let file = dir.join("history.jsonl");
        let history = History::parse(&std::fs::read_to_string(&file).unwrap_or_default());
        slower = slower_tests(&report, &history);
        suite = report.wall_seconds.and_then(|w| slower_suite(w, &history));
        let line = history_line(&report, &now(), &commit(root), profile);
        let written = std::fs::create_dir_all(&dir).and_then(|()| {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file)?;
            writeln!(f, "{line}")
        });
        if let Err(e) = written {
            eprintln!("{}: {e}", file.display());
        }
    }
    println!("\n{}", render(&report, top, &slower, suite, summary));
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JUNIT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="4" failures="1" errors="0" uuid="x" timestamp="2026-10-11T10:00:00Z" time="3.5">
    <testsuite name="helios-core" tests="2" disabled="0" errors="0" failures="0">
        <testcase name="graph::tests::a_cycle_is_refused" classname="helios-core" timestamp="t" time="0.012">
        </testcase>
        <testcase name="quoted &quot;name&quot; &amp; &lt;more&gt; &#233;" classname="helios-core" time="1.5"/>
    </testsuite>
    <testsuite name="helios-gui::viewport" tests="2" disabled="0" errors="0" failures="1">
        <!-- a <testcase> in a comment is not a test -->
        <testcase name="a_pan_renders_only_the_new_tiles" classname="helios-gui::viewport" time="2.25">
            <failure type="test failure">thread panicked at x &gt; y</failure>
            <system-out><![CDATA[<testcase name="fake"/>]]></system-out>
        </testcase>
        <testcase name="retried" classname="helios-gui::viewport" time="0.5">
            <flakyFailure type="test failure">first try</flakyFailure>
        </testcase>
        <testcase name="later" classname="helios-gui::viewport" time="0">
            <skipped/>
        </testcase>
    </testsuite>
</testsuites>
"#;

    #[test]
    fn a_nextest_junit_report_reads() {
        let r = parse_junit(JUNIT).unwrap();
        assert_eq!(r.wall_seconds, Some(3.5));
        let got: Vec<(&str, &str, f64, Status)> = r
            .cases
            .iter()
            .map(|c| (c.binary.as_str(), c.name.as_str(), c.seconds, c.status))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "helios-core",
                    "graph::tests::a_cycle_is_refused",
                    0.012,
                    Status::Passed
                ),
                (
                    "helios-core",
                    "quoted \"name\" & <more> \u{e9}",
                    1.5,
                    Status::Passed
                ),
                (
                    "helios-gui::viewport",
                    "a_pan_renders_only_the_new_tiles",
                    2.25,
                    Status::Failed
                ),
                ("helios-gui::viewport", "retried", 0.5, Status::Flaky),
                ("helios-gui::viewport", "later", 0.0, Status::Skipped),
            ]
        );
        assert_eq!(r.cases[2].krate(), "helios-gui");
    }

    #[test]
    fn a_broken_report_says_so() {
        assert!(parse_junit("<testsuites><testcase name=\"a\">").is_err());
        assert!(parse_junit("<testcase classname=\"x\"/>").is_err());
        assert!(parse_junit("<testsuites time=\"1").is_err());
        assert_eq!(parse_junit("").unwrap(), Report::default());
    }

    fn case(name: &str, seconds: f64) -> Case {
        Case {
            binary: "c".into(),
            name: name.into(),
            seconds,
            status: Status::Passed,
        }
    }

    /// A history where test `t` took each of `durations`, one run each.
    fn history(durations: &[f64]) -> History {
        let lines: Vec<String> = durations
            .iter()
            .map(|d| {
                history_line(
                    &Report {
                        cases: vec![case("t", *d)],
                        wall_seconds: Some(*d),
                    },
                    "2026-10-11T00:00:00Z",
                    "abc",
                    "default",
                )
            })
            .collect();
        History::parse(&(lines.join("\n") + "\nnot json\n"))
    }

    fn slower(now: f64, before: &[f64]) -> Vec<Slower> {
        let report = Report {
            cases: vec![case("t", now)],
            wall_seconds: None,
        };
        slower_tests(&report, &history(before))
    }

    #[test]
    fn a_test_is_slower_past_both_thresholds() {
        // 50 % and 0.5 s above the median of 1 s.
        assert_eq!(
            slower(1.6, &[1.0, 1.0, 1.0]),
            [Slower {
                name: "c t".into(),
                seconds: 1.6,
                median: 1.0
            }]
        );
        // Above 50 % but only 0.3 s: noise on a short test.
        assert!(slower(0.9, &[0.6, 0.6, 0.6]).is_empty());
        // Above 0.5 s but under 50 %: a long test varying.
        assert!(slower(12.0, &[10.0, 10.0, 10.0]).is_empty());
        // Not enough history yet.
        assert!(slower(5.0, &[1.0, 1.0]).is_empty());
    }

    #[test]
    fn the_median_is_of_the_last_five_runs() {
        // The old fast runs no longer count: the median is 3 s.
        assert!(slower(4.0, &[0.1, 0.1, 0.1, 3.0, 3.0, 3.0, 3.0, 3.0]).is_empty());
        // Median of the last five, one outlier ignored: 1 s.
        assert_eq!(
            slower(2.0, &[9.0, 1.0, 1.0, 9.0, 1.0, 1.0]).len(),
            1,
            "the median ignores an outlier"
        );
        assert_eq!(recent_median(&[1.0, 2.0, 3.0, 4.0]), Some(2.5));
    }

    #[test]
    fn failed_runs_do_not_set_the_baseline_and_are_not_flagged() {
        let run = |status, seconds| {
            history_line(
                &Report {
                    cases: vec![Case {
                        status,
                        ..case("t", seconds)
                    }],
                    wall_seconds: None,
                },
                "d",
                "c",
                "p",
            )
        };
        let lines: Vec<String> = [1.0, 1.0, 1.0]
            .map(|d| run(Status::Passed, d))
            .into_iter()
            .chain([0.01; 3].map(|d| run(Status::Failed, d)))
            .collect();
        let h = History::parse(&lines.join("\n"));
        assert_eq!(h.tests["c t"], [1.0, 1.0, 1.0]);
        let slow_failure = Report {
            cases: vec![Case {
                status: Status::Failed,
                ..case("t", 9.0)
            }],
            wall_seconds: None,
        };
        assert!(slower_tests(&slow_failure, &h).is_empty());
    }

    #[test]
    fn the_suite_is_slower_past_twenty_percent() {
        let h = history(&[10.0, 10.0, 10.0]);
        assert_eq!(slower_suite(12.5, &h), Some(10.0));
        assert_eq!(slower_suite(11.5, &h), None);
    }

    #[test]
    fn the_report_lists_the_slowest_first() {
        let r = parse_junit(JUNIT).unwrap();
        let text = render(&r, 2, &[], None, false);
        let a = text.find("2.25 s").unwrap();
        let b = text.find("1.50 s").unwrap();
        assert!(a < b, "{text}");
        assert!(!text.contains("a_cycle_is_refused"), "top 2 only: {text}");
        assert!(text.contains("helios-gui (3 tests)"), "{text}");
        let md = render(&r, 15, &[], None, true);
        assert!(md.contains(
            "| `helios-gui::viewport a_pan_renders_only_the_new_tiles` | 2.25 s | failed |"
        ));
    }
}
