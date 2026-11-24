//! Step B2 of the broker (docs/ci-broker.md): the always-run set.
//!
//! A selection is a list of [`Pick`]s, each one a [`Scope`] of tests, the
//! [`Source`] that asked for it and why. It prints as one nextest filter
//! expression, and answers "why does this test run?" ([`Selection::explain`]).
//!
//! The four sources:
//! - **affected**: the tests near the changed files (the caller maps the
//!   files; see `ci_select.rs` for Helios);
//! - **changed tests**: the `#[test]` functions a diff adds or edits
//!   ([`test_fns`], [`hunks`], [`touched_tests`]);
//! - **history**: the tests that failed or flaked in the last days and
//!   runs of the summary ([`recent_failures`]), and in runs not folded yet
//!   ([`failed_in`]);
//! - **smoke**: a short checked-in list ([`parse_smoke`]).
//!
//! Like the rest of `ci_broker`, it knows nothing of Helios.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use super::history::{Outcome, Stamp, Summary, TestResult};
use super::shadow::day_number;

/// Why a test is in the always-run set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// Near a changed file.
    Affected,
    /// A test the change adds or edits.
    ChangedTest,
    /// Failed or flaked lately.
    History,
    /// In the smoke list.
    Smoke,
}

impl Source {
    pub const ALL: [Source; 4] = [
        Source::Affected,
        Source::ChangedTest,
        Source::History,
        Source::Smoke,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Source::Affected => "affected",
            Source::ChangedTest => "changed tests",
            Source::History => "recent failures",
            Source::Smoke => "smoke",
        }
    }
}

/// A set of tests, as nextest names them: a binary id (`crate` for the
/// library's unit tests, `crate::target` for an integration test) and a
/// test name (`module::name`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// Every test.
    All,
    /// Every test of a package.
    Package(String),
    /// Every test of a package but those of one binary.
    PackageBut { package: String, binary: String },
    /// Every test of one binary.
    Binary(String),
    /// The tests of a binary whose name starts with `module::`.
    Module { binary: String, module: String },
    /// One test, by its full name.
    Test { binary: String, name: String },
    /// A test function found in a source file, whose full name is not
    /// known: in `binary` (or anywhere in `package` when `None`), a name
    /// starting with `prefix` and ending in `name`, any module between.
    Fn {
        package: String,
        binary: Option<String>,
        prefix: String,
        name: String,
    },
}

/// The package of a binary id.
pub fn package_of(binary: &str) -> &str {
    binary.split("::").next().unwrap_or(binary)
}

impl Scope {
    /// The nextest filter expression for the scope.
    pub fn filter(&self) -> String {
        match self {
            Scope::All => "all()".into(),
            Scope::Package(p) => format!("package(={p})"),
            Scope::PackageBut { package, binary } => {
                format!("(package(={package}) & !binary_id(={binary}))")
            }
            Scope::Binary(b) => format!("binary_id(={b})"),
            Scope::Module { binary, module } => {
                format!("(binary_id(={binary}) & test(/^{module}::/))")
            }
            Scope::Test { binary, name } => format!("(binary_id(={binary}) & test(={name}))"),
            Scope::Fn {
                package,
                binary,
                prefix,
                name,
            } => {
                let within = match binary {
                    Some(b) => format!("binary_id(={b})"),
                    None => format!("package(={package})"),
                };
                format!("({within} & test(/^{prefix}(.*::)?{name}$/))")
            }
        }
    }

    /// Whether the test `binary` `name` is in the scope.
    pub fn contains(&self, binary: &str, name: &str) -> bool {
        match self {
            Scope::All => true,
            Scope::Package(p) => package_of(binary) == p,
            Scope::PackageBut { package, binary: b } => {
                package_of(binary) == package && binary != b
            }
            Scope::Binary(b) => binary == b,
            Scope::Module { binary: b, module } => {
                binary == b
                    && name
                        .strip_prefix(module.as_str())
                        .is_some_and(|r| r.starts_with("::"))
            }
            Scope::Test { binary: b, name: n } => binary == b && name == n,
            Scope::Fn {
                package,
                binary: b,
                prefix,
                name: n,
            } => {
                let within = match b {
                    Some(b) => binary == b,
                    None => package_of(binary) == package,
                };
                within
                    && name.strip_prefix(prefix.as_str()).is_some_and(|rest| {
                        rest == n
                            || rest
                                .strip_suffix(n.as_str())
                                .is_some_and(|m| m.ends_with("::"))
                    })
            }
        }
    }

    /// Whether every test of `other` is in `self`, as far as the scopes
    /// say (false when unsure: the filter then keeps both).
    pub fn covers(&self, other: &Scope) -> bool {
        if self == other {
            return true;
        }
        match (self, other) {
            (Scope::All, _) => true,
            (_, Scope::All) => false,
            (Scope::Package(p), Scope::PackageBut { package, .. } | Scope::Fn { package, .. }) => {
                p == package
            }
            (Scope::Package(p), other) => other.binary().is_some_and(|b| package_of(b) == p),
            (Scope::PackageBut { package, binary }, other) => other
                .binary()
                .is_some_and(|b| package_of(b) == package && b != binary),
            (Scope::Binary(b), other) => other.binary() == Some(b.as_str()),
            (Scope::Module { binary, .. }, Scope::Test { binary: b, name }) => {
                b == binary && self.contains(b, name)
            }
            (
                Scope::Module { binary, module },
                Scope::Fn {
                    binary: Some(b),
                    prefix,
                    ..
                },
            ) => b == binary && prefix.starts_with(&format!("{module}::")),
            _ => false,
        }
    }

    /// The one binary the scope lies in, if it lies in one.
    fn binary(&self) -> Option<&str> {
        match self {
            Scope::Binary(b) | Scope::Module { binary: b, .. } | Scope::Test { binary: b, .. } => {
                Some(b)
            }
            Scope::Fn { binary, .. } => binary.as_deref(),
            _ => None,
        }
    }
}

/// One reason for some tests to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub source: Source,
    pub scope: Scope,
    pub why: String,
}

/// The always-run set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub picks: Vec<Pick>,
    /// What each source has to say beyond its picks (a source skipped,
    /// the history read...).
    pub notes: Vec<(Source, String)>,
}

impl Selection {
    pub fn add(&mut self, source: Source, scope: Scope, why: impl Into<String>) {
        self.picks.push(Pick {
            source,
            scope,
            why: why.into(),
        });
    }

    pub fn note(&mut self, source: Source, note: impl Into<String>) {
        self.notes.push((source, note.into()));
    }

    pub fn everything(&self) -> bool {
        self.picks.iter().any(|p| p.scope == Scope::All)
    }

    /// The scopes the filter needs: each one once, none another covers.
    pub fn scopes(&self) -> Vec<&Scope> {
        let mut all: Vec<&Scope> = self.picks.iter().map(|p| &p.scope).collect();
        all.sort();
        all.dedup();
        all.iter()
            .enumerate()
            .filter(|(i, s)| {
                !all.iter()
                    .enumerate()
                    .any(|(j, o)| j != *i && o.covers(s) && !(s.covers(o) && j > *i))
            })
            .map(|(_, s)| *s)
            .collect()
    }

    /// The nextest filter expression: `all()`, `none()` or the scopes
    /// joined by `|`.
    pub fn filter(&self) -> String {
        if self.everything() {
            return "all()".into();
        }
        let scopes = self.scopes();
        if scopes.is_empty() {
            return "none()".into();
        }
        scopes
            .iter()
            .map(|s| s.filter())
            .collect::<Vec<_>>()
            .join(" | ")
    }

    /// Whether the set holds the test `binary` `name`.
    pub fn selects(&self, binary: &str, name: &str) -> bool {
        self.picks.iter().any(|p| p.scope.contains(binary, name))
    }

    /// Of the history's current tests (listed in one of its last `recent`
    /// runs), how many the set holds: `(held, current)`. What a change
    /// would run, measured against what a full run runs.
    pub fn held_of(&self, summary: &Summary, recent: u64) -> (usize, usize) {
        let since = summary.runs.saturating_sub(recent);
        let current: Vec<&String> = summary
            .tests
            .iter()
            .filter(|(_, t)| t.last_seen > since)
            .map(|(id, _)| id)
            .collect();
        let held = current
            .iter()
            .filter(|id| {
                id.split_once(' ')
                    .is_some_and(|(b, n)| self.selects(b, n.trim()))
            })
            .count();
        (held, current.len())
    }

    /// The picks that select a test, named as nextest and the history name
    /// it: `<binary id> <test name>`.
    pub fn explain(&self, test: &str) -> Result<Vec<&Pick>, String> {
        let (binary, name) = test
            .split_once(' ')
            .ok_or_else(|| format!("`{test}`: name a test as `<binary id> <test name>`"))?;
        Ok(self
            .picks
            .iter()
            .filter(|p| p.scope.contains(binary, name.trim()))
            .collect())
    }

    /// The human summary: per source, its picks and its notes.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        if self.everything() {
            let _ = writeln!(out, "always-run set: every test");
        } else {
            let _ = writeln!(
                out,
                "always-run set: {} picks, {} filter terms",
                self.picks.len(),
                self.scopes().len()
            );
        }
        for source in Source::ALL {
            let picks: Vec<&Pick> = self.picks.iter().filter(|p| p.source == source).collect();
            let notes: Vec<&String> = self
                .notes
                .iter()
                .filter(|(s, _)| *s == source)
                .map(|(_, n)| n)
                .collect();
            let _ = writeln!(out, "{} ({}):", source.name(), picks.len());
            for n in notes {
                let _ = writeln!(out, "  note: {n}");
            }
            for p in picks {
                let _ = writeln!(out, "  - {}: {}", p.scope.filter(), p.why);
            }
        }
        out
    }
}

/// How far back the history source reads: every failure or flake of the
/// last `days` days (counted back from the summary's newest run), and of
/// the last `runs` runs folded, whichever reaches further. A count of runs
/// alone missed tests that had been failing for a while (`next` runs often:
/// 20 runs can be a few hours).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub days: i64,
    pub runs: usize,
}

impl Window {
    /// What the summary says the window read, for the notes.
    pub fn describe(self, summary: &Summary) -> String {
        let newest = summary
            .last_run
            .as_ref()
            .map_or("no run", |r| r.date.get(..10).unwrap_or(&r.date));
        format!(
            "read the failures of the last {} days (up to {newest}) and of the last {} of {} runs in the summary",
            self.days,
            summary.ingested.len().min(self.runs),
            summary.runs
        )
    }
}

/// The tests that failed or flaked within the window, as `(test id,
/// why)`, by id. A test's last failure is its newest one: when it lies
/// before the window, so do all the others.
pub fn recent_failures(summary: &Summary, window: Window) -> Vec<(String, String)> {
    let skip = summary.ingested.len().saturating_sub(window.runs);
    let recent: BTreeSet<&str> = summary
        .ingested
        .iter()
        .skip(skip)
        .map(String::as_str)
        .collect();
    let since = summary
        .last_run
        .as_ref()
        .and_then(|r| day_number(&r.date))
        .map(|d| d - window.days + 1);
    let in_window = |s: &&Stamp| {
        recent.contains(s.run.as_str())
            || since.is_some_and(|since| day_number(&s.date).is_some_and(|d| d >= since))
    };
    let short = |c: &str| c.chars().take(10).collect::<String>();
    summary
        .tests
        .iter()
        .filter_map(|(id, stats)| {
            let mut why = Vec::new();
            if let Some(f) = stats.last_failure.as_ref().filter(in_window) {
                why.push(format!(
                    "failed in run {} ({}, {})",
                    f.run,
                    short(&f.commit),
                    f.date
                ));
            }
            if let Some(f) = stats.last_flake.as_ref().filter(in_window) {
                why.push(format!(
                    "flaked in run {} ({}, {})",
                    f.run,
                    short(&f.commit),
                    f.date
                ));
            }
            (!why.is_empty()).then(|| (id.clone(), why.join("; ")))
        })
        .collect()
}

/// The tests that failed or flaked in runs the history has not folded yet
/// (`next`'s runs since the last night), as `(test id, why)`, by id. `from` names where the results come
/// from.
pub fn failed_in(results: &[TestResult], from: &str) -> Vec<(String, String)> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for t in results {
        let what = match t.outcome {
            Outcome::Fail => "failed",
            Outcome::Flaky => "flaked",
            Outcome::Pass | Outcome::Skip => continue,
        };
        out.entry(t.id.clone())
            .or_insert_with(|| format!("{what} in {from}"));
    }
    out.into_iter().collect()
}

/// The job summary of a selective run: how many of the run's tests (`all`,
/// as nextest lists them: `<binary id> <test name>`) the set holds, and per
/// source how many it brought, then `context` (what ran, how to run
/// everything) and each source's notes.
pub fn run_summary(selection: &Selection, all: &[String], context: &str) -> String {
    let mut out = String::new();
    let tests: Vec<(&str, &str)> = all
        .iter()
        .filter_map(|id| id.split_once(' '))
        .map(|(b, n)| (b, n.trim()))
        .collect();
    let total = tests.len();
    if selection.everything() {
        let why = selection
            .picks
            .iter()
            .find(|p| p.scope == Scope::All)
            .map_or("", |p| p.why.as_str());
        let _ = writeln!(
            out,
            "**All {total} tests run**: the change selects everything ({why})."
        );
        return out;
    }
    let selected = tests
        .iter()
        .filter(|(b, n)| selection.selects(b, n))
        .count();
    let reasons: Vec<String> = Source::ALL
        .iter()
        .filter_map(|source| {
            let n = tests
                .iter()
                .filter(|(b, n)| {
                    selection
                        .picks
                        .iter()
                        .any(|p| p.source == *source && p.scope.contains(b, n))
                })
                .count();
            (n > 0).then(|| format!("{}: {n}", source.name()))
        })
        .collect();
    let reasons = if reasons.is_empty() {
        "nothing selects a test".to_string()
    } else {
        reasons.join(", ")
    };
    let _ = writeln!(
        out,
        "**{selected} of {total} tests selected** ({reasons}; a test may have several reasons)."
    );
    let _ = writeln!(out, "\n{context}");
    if !selection.notes.is_empty() {
        out.push('\n');
    }
    for (source, note) in &selection.notes {
        let _ = writeln!(out, "- {}: {note}", source.name());
    }
    out
}

/// A test id of the history (`<binary id> <test name>`) as a scope.
pub fn test_scope(id: &str) -> Option<Scope> {
    let (binary, name) = id.split_once(' ')?;
    Some(Scope::Test {
        binary: binary.into(),
        name: name.trim().into(),
    })
}

/// The smoke list: one entry a line, `#` starts a comment.
/// - `<binary id> <test name>`: one test;
/// - `<binary id> <module>::`: a module's tests;
/// - `<binary id>` or `<binary id> *`: a whole binary.
pub fn parse_smoke(text: &str) -> Result<Vec<Scope>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut words = line.split_whitespace();
        let binary = words.next().unwrap_or_default().to_string();
        let scope = match (words.next(), words.next()) {
            (None | Some("*"), None) => Scope::Binary(binary),
            (Some(name), None) => match name.strip_suffix("::") {
                Some(module) => Scope::Module {
                    binary,
                    module: module.into(),
                },
                None => Scope::Test {
                    binary,
                    name: name.into(),
                },
            },
            _ => return Err(format!("line {}: `{line}`: too many words", i + 1)),
        };
        out.push(scope);
    }
    Ok(out)
}

/// A `#[test]` function in a source file: its name and its lines (from
/// its attribute to the line before the next test's, 1-based, inclusive).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestFn {
    pub name: String,
    pub first: usize,
    pub last: usize,
}

fn is_test_attr(line: &str) -> bool {
    let line = line.trim();
    line == "#[test]" || (line.starts_with("#[") && line.contains("::test"))
}

/// The test functions of a source file. Coarse on purpose: a test's span
/// runs to the next test, so a helper between two tests counts as the
/// first one's.
pub fn test_fns(source: &str) -> Vec<TestFn> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out: Vec<TestFn> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_test_attr(lines[i]) {
            i += 1;
            continue;
        }
        let first = i + 1;
        let name = lines[i + 1..].iter().find_map(|l| {
            let rest = l.trim().split("fn ").nth(1)?;
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then_some(name)
        });
        if let Some(prev) = out.last_mut() {
            prev.last = first - 1;
        }
        if let Some(name) = name {
            out.push(TestFn {
                name,
                first,
                last: lines.len(),
            });
        }
        i += 1;
    }
    out
}

/// The lines a `git diff -U0` touched in the new side of each file, as
/// `(path, [(first, last)])`. A deletion touches the lines around it.
pub fn hunks(diff: &str) -> Vec<(String, Vec<(usize, usize)>)> {
    let mut out: Vec<(String, Vec<(usize, usize)>)> = Vec::new();
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            let path = path.strip_prefix("b/").unwrap_or(path);
            out.push((path.to_string(), Vec::new()));
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            let Some(new) = rest.split_whitespace().find(|w| w.starts_with('+')) else {
                continue;
            };
            let mut parts = new[1..].split(',');
            let start: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
            let count: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
            let range = if count == 0 {
                (start.max(1), start + 1)
            } else {
                (start, start + count - 1)
            };
            if let Some((_, ranges)) = out.last_mut() {
                ranges.push(range);
            }
        }
    }
    out.retain(|(p, _)| p != "/dev/null");
    out
}

/// The tests of `source` whose lines a range touches.
pub fn touched_tests(source: &str, ranges: &[(usize, usize)]) -> Vec<String> {
    test_fns(source)
        .into_iter()
        .filter(|t| ranges.iter().any(|(a, b)| *a <= t.last && *b >= t.first))
        .map(|t| t.name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci_broker::history::TestStats;

    fn test(binary: &str, name: &str) -> Scope {
        Scope::Test {
            binary: binary.into(),
            name: name.into(),
        }
    }

    #[test]
    fn scopes_match_and_cover_as_nextest_would() {
        let module = Scope::Module {
            binary: "helios-gui::it".into(),
            module: "import".into(),
        };
        assert!(module.contains("helios-gui::it", "import::a_card"));
        assert!(!module.contains("helios-gui::it", "import_scan::a_card"));
        assert!(module.covers(&test("helios-gui::it", "import::a_card")));
        assert!(Scope::Package("helios-gui".into()).covers(&module));
        let but = Scope::PackageBut {
            package: "helios-gui".into(),
            binary: "helios-gui::it".into(),
        };
        assert!(!but.covers(&module));
        assert!(but.contains("helios-gui", "shell::tests::x"));
        let f = Scope::Fn {
            package: "helios-core".into(),
            binary: None,
            prefix: String::new(),
            name: "a_cycle".into(),
        };
        assert!(f.contains("helios-core", "graph::tests::a_cycle"));
        assert!(!f.contains("helios-core", "graph::tests::not_a_cycle"));
        assert_eq!(
            f.filter(),
            "(package(=helios-core) & test(/^(.*::)?a_cycle$/))"
        );

        let mut s = Selection::default();
        s.add(
            Source::Smoke,
            test("helios-gui::it", "import::a_card"),
            "smoke",
        );
        s.add(Source::Affected, module.clone(), "near");
        s.add(Source::History, module, "again");
        assert_eq!(
            s.filter(),
            "(binary_id(=helios-gui::it) & test(/^import::/))"
        );
        assert_eq!(s.explain("helios-gui::it import::a_card").unwrap().len(), 3);
        assert!(s.explain("no-space").is_err());
        assert_eq!(Selection::default().filter(), "none()");
    }

    #[test]
    fn the_smoke_list_reads_tests_modules_and_binaries() {
        let text = "# smoke\nhelios-core::engine renders_a_chain\n\
                    helios-gui::it survival::   # a module\nhelios-node::survival *\n";
        assert_eq!(
            parse_smoke(text).unwrap(),
            vec![
                test("helios-core::engine", "renders_a_chain"),
                Scope::Module {
                    binary: "helios-gui::it".into(),
                    module: "survival".into()
                },
                Scope::Binary("helios-node::survival".into()),
            ]
        );
        assert!(parse_smoke("a b c").is_err());
    }

    #[test]
    fn a_diff_names_the_tests_it_touches() {
        let source =
            "use x;\n\n#[test]\nfn one() {\n    a();\n}\n\n#[test]\nfn two() {\n    b();\n}\n";
        assert_eq!(
            test_fns(source),
            vec![
                TestFn {
                    name: "one".into(),
                    first: 3,
                    last: 7
                },
                TestFn {
                    name: "two".into(),
                    first: 8,
                    last: 11
                },
            ]
        );
        let diff = "diff --git a/t.rs b/t.rs\n--- a/t.rs\n+++ b/t.rs\n@@ -9,0 +10,1 @@\n+    c();\n\
                    diff --git a/gone.rs b/gone.rs\n--- a/gone.rs\n+++ /dev/null\n@@ -1,3 +0,0 @@\n";
        let h = hunks(diff);
        assert_eq!(h, vec![("t.rs".to_string(), vec![(10, 10)])]);
        assert_eq!(touched_tests(source, &h[0].1), vec!["two".to_string()]);
        assert!(touched_tests(source, &[(1, 1)]).is_empty());
    }

    fn stamp(run: &str, date: &str) -> Option<Stamp> {
        Some(Stamp {
            run: run.into(),
            commit: "0123456789abcdef".into(),
            date: date.into(),
        })
    }

    #[test]
    fn recent_failures_read_the_last_runs_and_days() {
        let mut summary = Summary {
            runs: 30,
            ingested: (1..=30).map(|i| i.to_string()).collect(),
            last_run: stamp("30", "2026-11-21T18:00:00Z"),
            ..Summary::default()
        };
        summary.tests.insert(
            "helios-gui::it a::flaky".into(),
            TestStats {
                last_flake: stamp("29", "2026-11-21T09:00:00Z"),
                ..TestStats::default()
            },
        );
        summary.tests.insert(
            "helios-core b::old".into(),
            TestStats {
                last_failure: stamp("3", "2026-11-10T09:00:00Z"),
                ..TestStats::default()
            },
        );
        let by_runs = Window { days: 0, runs: 20 };
        let recent = recent_failures(&summary, by_runs);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].0, "helios-gui::it a::flaky");
        assert!(recent[0].1.starts_with("flaked in run 29 (0123456789"));
        assert_eq!(
            recent_failures(&summary, Window { days: 0, runs: 30 }).len(),
            2
        );
        // 11-10 is 12 days before the newest run: out of a week, in two
        assert_eq!(
            recent_failures(&summary, Window { days: 7, runs: 20 }).len(),
            1
        );
        assert_eq!(
            recent_failures(&summary, Window { days: 12, runs: 20 }).len(),
            2
        );
    }

    /// The shadow study's misses: a test failing for days on a busy `next`
    /// fell out of the last 20 runs while it was still failing. Read by
    /// days, it stays selected for as long as its last failure is recent.
    #[test]
    fn a_test_failing_for_days_stays_selected_past_the_last_runs() {
        let mut summary = Summary {
            runs: 88,
            ingested: (1..=88).map(|i| i.to_string()).collect(),
            last_run: stamp("88", "2026-11-21T20:00:00Z"),
            ..Summary::default()
        };
        summary.tests.insert(
            "helios-gui::it import::still_red".into(),
            TestStats {
                last_failure: stamp("60", "2026-11-20T08:00:00Z"),
                ..TestStats::default()
            },
        );
        assert!(recent_failures(&summary, Window { days: 0, runs: 20 }).is_empty());
        let week = Window { days: 7, runs: 20 };
        assert_eq!(recent_failures(&summary, week).len(), 1);
        assert!(
            week.describe(&summary)
                .contains("last 7 days (up to 2026-11-21)")
        );
        assert!(week.describe(&summary).contains("last 20 of 88 runs"));
    }

    #[test]
    fn failures_of_runs_not_folded_yet_are_named_once() {
        let r = |id: &str, outcome| TestResult {
            id: id.into(),
            outcome,
            seconds: 1.0,
        };
        let got = failed_in(
            &[
                r("helios-core a::red", Outcome::Fail),
                r("helios-core a::green", Outcome::Pass),
                r("helios-gui::it b::shaky", Outcome::Flaky),
                r("helios-core a::red", Outcome::Fail),
                r("helios-core a::off", Outcome::Skip),
            ],
            "next's run 7",
        );
        assert_eq!(
            got,
            vec![
                (
                    "helios-core a::red".to_string(),
                    "failed in next's run 7".to_string()
                ),
                (
                    "helios-gui::it b::shaky".to_string(),
                    "flaked in next's run 7".to_string()
                ),
            ]
        );
    }

    #[test]
    fn the_run_summary_counts_the_selected_tests_and_their_reasons() {
        let all: Vec<String> = [
            "helios-gui::it import::a",
            "helios-gui::it import::b",
            "helios-gui::it undo::c",
            "helios-core d::e",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let mut s = Selection::default();
        s.add(
            Source::Affected,
            Scope::Module {
                binary: "helios-gui::it".into(),
                module: "import".into(),
            },
            "near",
        );
        s.add(Source::Smoke, test("helios-core", "d::e"), "smoke");
        s.add(Source::History, test("helios-gui::it", "import::b"), "red");
        s.note(Source::History, "read the last 7 days");
        let md = run_summary(&s, &all, "The nightly run tests everything.");
        assert!(
            md.starts_with("**3 of 4 tests selected** (affected: 2, recent failures: 1, smoke: 1;"),
            "{md}"
        );
        assert!(md.contains("\nThe nightly run tests everything.\n"), "{md}");
        assert!(
            md.contains("- recent failures: read the last 7 days"),
            "{md}"
        );
        s.add(Source::Affected, Scope::All, "Cargo.lock changed");
        let md = run_summary(&s, &all, "");
        assert!(
            md.starts_with(
                "**All 4 tests run**: the change selects everything (Cargo.lock changed)"
            ),
            "{md}"
        );
    }

    #[test]
    fn the_set_is_measured_against_the_current_tests() {
        let mut summary = Summary {
            runs: 30,
            ..Summary::default()
        };
        for (id, seen) in [
            ("helios-gui::it import::a", 30),
            ("helios-gui::it undo::b", 29),
            ("helios-gui c::d", 30),
            ("helios-gui::it import::gone", 3),
        ] {
            summary.tests.insert(
                id.into(),
                TestStats {
                    last_seen: seen,
                    ..TestStats::default()
                },
            );
        }
        let mut s = Selection::default();
        s.add(
            Source::Affected,
            Scope::Module {
                binary: "helios-gui::it".into(),
                module: "import".into(),
            },
            "near",
        );
        assert_eq!(s.held_of(&summary, 10), (1, 3));
    }
}
