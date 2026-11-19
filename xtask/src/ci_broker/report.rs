//! What the history says, read only: the slowest tests, the flakiest, and
//! those that never failed in many runs. The input of the always-run set
//! (B2) and of the shadow week (B3).

use std::fmt::Write as _;

use super::history::{Summary, TestStats};

/// A test counts as current when one of the last `ACTIVE_RUNS` runs listed
/// it; older ones were renamed or removed.
pub const ACTIVE_RUNS: u64 = 10;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// How many tests each list shows (`usize::MAX`: all).
    pub top: usize,
    /// "Never failed" needs at least this many runs.
    pub min_runs: u64,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            top: 20,
            min_runs: 50,
        }
    }
}

/// The current tests: listed by one of the last `ACTIVE_RUNS` runs.
pub fn active(summary: &Summary) -> impl Iterator<Item = (&str, &TestStats)> {
    let since = summary.runs.saturating_sub(ACTIVE_RUNS);
    summary
        .tests
        .iter()
        .filter(move |(_, s)| s.last_seen > since)
        .map(|(id, s)| (id.as_str(), s))
}

/// Slowest first, by median time.
pub fn slowest(summary: &Summary) -> Vec<(&str, &TestStats)> {
    let mut v: Vec<_> = active(summary)
        .filter(|(_, s)| s.median_seconds().is_some())
        .collect();
    v.sort_by(|a, b| {
        let median = |s: &TestStats| s.median_seconds().unwrap_or(0.0);
        median(b.1).total_cmp(&median(a.1)).then(a.0.cmp(b.0))
    });
    v
}

/// Tests that flaked, highest flake rate first.
pub fn flakiest(summary: &Summary) -> Vec<(&str, &TestStats)> {
    let mut v: Vec<_> = active(summary).filter(|(_, s)| s.flakes > 0).collect();
    v.sort_by(|a, b| {
        b.1.flake_rate()
            .total_cmp(&a.1.flake_rate())
            .then(b.1.flakes.cmp(&a.1.flakes))
            .then(a.0.cmp(b.0))
    });
    v
}

/// Tests run at least `min_runs` times with no failure and no flake, the
/// longest record first.
pub fn never_failed(summary: &Summary, min_runs: u64) -> Vec<(&str, &TestStats)> {
    let mut v: Vec<_> = active(summary)
        .filter(|(_, s)| s.runs >= min_runs && s.failures == 0 && s.flakes == 0)
        .collect();
    v.sort_by(|a, b| b.1.runs.cmp(&a.1.runs).then(a.0.cmp(b.0)));
    v
}

pub fn render(summary: &Summary, options: Options) -> String {
    let mut out = String::new();
    let current = active(summary).count();
    let last = summary.last_run.as_ref().map_or(String::new(), |l| {
        format!(", the last {} ({})", l.run, l.date)
    });
    let _ = writeln!(
        out,
        "{} runs{last}; {current} current tests ({} in the history).",
        summary.runs,
        summary.tests.len()
    );

    let slow = slowest(summary);
    let total: f64 = slow.iter().filter_map(|(_, s)| s.median_seconds()).sum();
    let _ = writeln!(
        out,
        "\nSlowest (median time; {total:.0} s for all current tests):"
    );
    for (id, s) in slow.iter().take(options.top) {
        let _ = writeln!(
            out,
            "{:>8.2} s  {id}  ({} runs)",
            s.median_seconds().unwrap_or(0.0),
            s.runs
        );
    }

    let flaky = flakiest(summary);
    let _ = writeln!(out, "\nFlakiest ({} tests passed on a retry):", flaky.len());
    for (id, s) in flaky.iter().take(options.top) {
        let last = s.last_flake.as_ref().map_or("", |l| l.commit.as_str());
        let _ = writeln!(
            out,
            "{:>6.1} %  {id}  ({} of {} runs, last at {last})",
            s.flake_rate() * 100.0,
            s.flakes,
            s.runs
        );
    }

    let green = never_failed(summary, options.min_runs);
    let _ = writeln!(
        out,
        "\nNever failed nor flaked in {} runs or more: {} of {current} tests.",
        options.min_runs,
        green.len()
    );
    for (id, s) in green.iter().take(options.top) {
        let _ = writeln!(out, "{:>8}  {id}", s.runs);
    }
    if green.len() > options.top {
        let _ = writeln!(
            out,
            "     …  {} more (`--top all`)",
            green.len() - options.top
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::history::Outcome::{Fail, Flaky, Pass};
    use super::super::history::tests::run;
    use super::*;

    fn summary() -> Summary {
        let mut s = Summary::default();
        for i in 0..60 {
            let flaky = if i % 4 == 0 { Flaky } else { Pass };
            let broke = if i == 30 { Fail } else { Pass };
            s.fold(&mut run(
                &i.to_string(),
                "2026-11-19",
                &[
                    ("b green", Pass, 0.1),
                    ("b slow", Pass, 9.0),
                    ("b flaky", flaky, 1.0),
                    ("b broke", broke, 2.0),
                ],
            ));
        }
        s
    }

    #[test]
    fn the_lists_rank_what_they_say() {
        let s = summary();
        let ids = |v: Vec<(&str, &TestStats)>| {
            v.iter()
                .map(|(id, _)| (*id).to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(slowest(&s)),
            ["b slow", "b broke", "b flaky", "b green"]
        );
        assert_eq!(ids(flakiest(&s)), ["b flaky"]);
        assert_eq!(ids(never_failed(&s, 50)), ["b green", "b slow"]);
        assert!(never_failed(&s, 61).is_empty());
        let text = render(
            &s,
            Options {
                top: 1,
                min_runs: 50,
            },
        );
        assert!(
            text.contains("25.0 %  b flaky  (15 of 60 runs, last at c56)"),
            "{text}"
        );
        assert!(text.contains("1 more"), "{text}");
    }

    #[test]
    fn a_test_gone_for_ten_runs_is_not_current() {
        let mut s = summary();
        for i in 60..70 {
            s.fold(&mut run(&i.to_string(), "d", &[("b green", Pass, 0.1)]));
        }
        let ids: Vec<&str> = active(&s).map(|(id, _)| id).collect();
        assert_eq!(ids, ["b green"]);
    }
}
