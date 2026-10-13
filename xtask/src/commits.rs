//! `cargo xtask check-commits [--full] [RANGE]`: every commit of a series
//! builds, passes clippy and the tests it touches, so `git bisect` keeps
//! working.
//!
//! The range is `BASE..HEAD`, or `BASE` alone for `BASE..HEAD`; without one
//! it is the branch's upstream (`@{upstream}..HEAD`), else
//! `origin/main..HEAD`. A base that is all zeros (a first push) or that the
//! repository does not know (a force push) falls back to the merge base of
//! the head with `origin/main`.
//!
//! The last commit of the range is left out: the full gate runs on it
//! anyway. Of the others, the [`CAP`] newest are walked, oldest first, and
//! the older ones are listed as skipped. Each commit is checked out in one
//! detached worktree under `target/check-commits/`, removed at the end, so
//! the caller's checkout is never touched, and every commit builds in the
//! same target directory, incrementally. Per commit:
//!
//! 1. `cargo build --workspace --all-targets`
//! 2. `cargo clippy --workspace --all-targets -- -D warnings`
//! 3. the tests the commit touches: `cargo nextest run --workspace
//!    --profile ci -E` [`commit_filter`]. The selection is the one
//!    [`crate::affected`] makes of the commit's own diff, its first parent
//!    as the base and its own `cargo metadata` as the graph: the fast tests
//!    of the crates it touches and of their dependents, and the critical
//!    set. A commit that changes a path that runs everything (`Cargo.lock`,
//!    xtask, CI, a path no rule knows) runs every fast test and the
//!    critical set. With `--full`, every commit runs every fast test
//!    ([`FAST_TESTS`]), as before the selection existed.
//! 4. `cargo xtask check`
//!
//! The walk stops at the first failure and names the commit and the step.
//! The report gives, per commit, the tests selected and why, the result
//! and the time taken. It is printed at the end, and appended to
//! `GITHUB_STEP_SUMMARY` when that is set.

use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::Instant;

use crate::affected::{self, CRITICAL, Graph, Selection};
use crate::test_times::target_dir;

pub const USAGE: &str = "check-commits [--full] [BASE..HEAD | BASE]";

/// The most commits walked in one run.
pub const CAP: usize = 20;

/// The fast tests: all but the slow ones, which the full gate still runs
/// on the head:
/// - `helios-parity`: parity against darktable-cli and vkdt-cli, and
///   decoding real raw files;
/// - the integration tests of `helios-gui`: the shell drawn headless and
///   compared to snapshots;
/// - the masks stress network (`helios-module-masks`, test `stress`).
pub const FAST_TESTS: &str = "not (package(helios-parity) \
    | (package(helios-gui) & kind(test)) \
    | (package(helios-module-masks) & binary(stress)))";

const WORKTREE: &str = "target/check-commits/worktree";

/// A range as given on the command line: `(base, head)`.
pub fn parse_range(arg: Option<&str>, upstream: Option<&str>) -> Option<(String, String)> {
    let arg = match arg {
        Some(a) => a.to_string(),
        None => format!("{}..HEAD", upstream.unwrap_or("origin/main")),
    };
    if arg.contains("...") {
        return None;
    }
    let (base, head) = arg.split_once("..").unwrap_or((&arg, "HEAD"));
    let head = if head.is_empty() { "HEAD" } else { head };
    if base.is_empty() || head.contains("..") {
        return None;
    }
    Some((base.to_string(), head.to_string()))
}

/// The command line: `(full, range)`.
pub fn parse_args<'a>(args: &[&'a str]) -> Option<(bool, Option<&'a str>)> {
    let (mut full, mut range) = (false, None);
    for a in args {
        match *a {
            "--full" if !full => full = true,
            a if a.starts_with('-') || range.is_some() => return None,
            a => range = Some(a),
        }
    }
    Some((full, range))
}

/// A base a push event gives when there is no previous head.
pub fn is_null_sha(rev: &str) -> bool {
    !rev.is_empty() && rev.chars().all(|c| c == '0')
}

/// A commit of the range and the base its diff is taken against: its first
/// parent (a merge is checked against the line it was merged into), or
/// none for a root commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub parent: Option<String>,
}

/// The commits of `git rev-list --reverse --parents`: one per line, the
/// commit then its parents.
pub fn parse_history(listed: &str) -> Vec<Commit> {
    listed
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let sha = words.next()?.to_string();
            let parent = words.next().map(str::to_string);
            Some(Commit { sha, parent })
        })
        .collect()
}

/// The commits to walk and the ones skipped, from the range's commits
/// listed oldest first: the last one (the head) is left out, then the
/// `cap` newest are kept.
pub fn plan_walk<T: Clone>(commits: &[T], cap: usize) -> (Vec<T>, Vec<T>) {
    let Some((_, before_head)) = commits.split_last() else {
        return (Vec::new(), Vec::new());
    };
    let cut = before_head.len().saturating_sub(cap);
    (before_head[cut..].to_vec(), before_head[..cut].to_vec())
}

/// What one commit's diff selects: `affected`'s selection of the paths it
/// changes, or everything for a root commit (no parent to diff against).
pub fn select_commit(graph: &Graph, parent: Option<&str>, paths: &[String]) -> Selection {
    let mut selection = affected::select(graph, paths);
    if parent.is_none() {
        selection.everything = true;
        selection.reasons.insert(0, "a root commit".into());
        selection.crates = graph.crates.iter().map(|c| c.name.clone()).collect();
    }
    selection
}

/// The nextest filter of one commit: the fast tests of the crates its
/// selection names, and the critical set; every fast test and the critical
/// set when it selects everything; every fast test alone with `--full`.
pub fn commit_filter(selection: Option<&Selection>) -> String {
    let Some(s) = selection else {
        return FAST_TESTS.to_string();
    };
    let critical = format!("({CRITICAL})");
    if s.everything {
        return format!("({FAST_TESTS}) | {critical}");
    }
    if s.crates.is_empty() {
        return critical;
    }
    let packages: Vec<String> = s.crates.iter().map(|c| format!("package(={c})")).collect();
    format!("(({}) & ({FAST_TESTS})) | {critical}", packages.join(" | "))
}

/// What a commit runs and why, in a line.
pub fn describe(selection: Option<&Selection>) -> String {
    let Some(s) = selection else {
        return "every fast test (--full)".into();
    };
    if s.everything {
        return format!(
            "every fast test and the critical set, because of: {}",
            s.reasons.join(", ")
        );
    }
    if s.crates.is_empty() {
        return "the critical set alone: no crate touched".into();
    }
    let changed: Vec<&str> = s.changed.iter().map(String::as_str).collect();
    let dependents = s
        .crates
        .len()
        .saturating_sub(s.changed.iter().filter(|c| s.crates.contains(*c)).count());
    format!(
        "{} crate(s): changed {}, {} dependent(s); and the critical set",
        s.crates.len(),
        if changed.is_empty() {
            "none".to_string()
        } else {
            changed.join(", ")
        },
        dependents
    )
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// The base to walk from: `base` itself, or the merge base of the head
/// with `origin/main` when `base` is null or unknown.
fn resolve_base(root: &Path, base: &str, head: &str) -> Result<String, String> {
    let known =
        !is_null_sha(base) && git(root, &["cat-file", "-e", &format!("{base}^{{commit}}")]).is_ok();
    if known {
        return Ok(base.to_string());
    }
    let fallback = git(root, &["merge-base", "origin/main", head])?;
    eprintln!("xtask: base `{base}` unknown, walking from the merge base with origin/main");
    Ok(fallback)
}

/// One step of the check on a commit.
struct Step {
    name: &'static str,
    args: Vec<String>,
}

fn steps(filter: &str) -> Vec<Step> {
    let s = |name, args: &[&str]| Step {
        name,
        args: args.iter().map(std::string::ToString::to_string).collect(),
    };
    vec![
        s("cargo build", &["build", "--workspace", "--all-targets"]),
        s(
            "cargo clippy",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ),
        s(
            "tests",
            &[
                "nextest",
                "run",
                "--workspace",
                "--profile",
                "ci",
                "--no-tests",
                "pass",
                "-E",
                filter,
            ],
        ),
        s(
            "cargo xtask check",
            &["run", "--quiet", "--package", "xtask", "--", "check"],
        ),
    ]
}

/// Removes the walk's worktree, wherever a previous run left it.
fn remove_worktree(root: &Path, dir: &Path) {
    let dir = dir.to_string_lossy();
    let _ = git(root, &["worktree", "remove", "--force", &dir]);
    let _ = git(root, &["worktree", "prune"]);
}

/// How one commit went.
pub struct Outcome {
    /// `<short sha> <subject>`.
    pub line: String,
    /// The tests selected and why ([`describe`]).
    pub tests: String,
    /// The step that failed, if one did.
    pub result: Result<(), &'static str>,
    pub seconds: u64,
}

fn summary(walked: &[Outcome], skipped: &[String], full: bool) -> String {
    let mut out = String::from("## Commits of the series\n\n");
    if walked.is_empty() && skipped.is_empty() {
        out.push_str("No commit before the head: nothing to check.\n");
        return out;
    }
    let _ = writeln!(
        out,
        "Tests per commit: {}.\n",
        if full {
            "every fast test (--full)"
        } else {
            "what the commit touches, and the critical set"
        }
    );
    out.push_str("| Commit | Tests | Result | Time |\n|--------|-------|--------|------|\n");
    let mut total = 0;
    for o in walked {
        let result = match o.result {
            Ok(()) => "ok".to_string(),
            Err(step) => format!("**broken**: {step} failed"),
        };
        total += o.seconds;
        let _ = writeln!(
            out,
            "| {} | {} | {result} | {}m{:02}s |",
            o.line.replace('|', "\\|"),
            o.tests.replace('|', "\\|"),
            o.seconds / 60,
            o.seconds % 60
        );
    }
    let _ = writeln!(out, "\nTotal: {}m{:02}s.", total / 60, total % 60);
    if !skipped.is_empty() {
        let _ = writeln!(
            out,
            "\nSkipped, beyond the cap of {CAP} commits:\n\n```text\n{}\n```",
            skipped.join("\n")
        );
    }
    out
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let (full, arg) = parse_args(args)?;
    let upstream = git(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .ok();
    let (base, head) = parse_range(arg, upstream.as_deref())?;
    Some(match walk(root, &base, &head, full) {
        Ok(code) => code,
        Err(e) => crate::fail(&[e]),
    })
}

fn walk(root: &Path, base: &str, head: &str, full: bool) -> Result<ExitCode, String> {
    let head = git(
        root,
        &["rev-parse", "--verify", &format!("{head}^{{commit}}")],
    )?;
    let base = resolve_base(root, base, &head)?;
    let listed = git(
        root,
        &[
            "rev-list",
            "--reverse",
            "--parents",
            &format!("{base}..{head}"),
        ],
    )?;
    let commits = parse_history(&listed);
    let (walk, skipped) = plan_walk(&commits, CAP);
    let oneline = |sha: &str| {
        git(root, &["log", "-1", "--format=%h %s", sha]).unwrap_or_else(|_| sha.to_string())
    };
    let skipped: Vec<String> = skipped.iter().map(|c| oneline(&c.sha)).collect();
    for s in &skipped {
        eprintln!("xtask: skipped (beyond the cap of {CAP}): {s}");
    }
    eprintln!(
        "xtask: checking {} commit(s) before the head {}{}",
        walk.len(),
        &head[..head.len().min(12)],
        if full {
            ", every fast test on each"
        } else {
            ""
        }
    );

    let mut results = Vec::new();
    if !walk.is_empty() {
        let dir = root.join(WORKTREE);
        remove_worktree(root, &dir);
        let dir_s = dir.to_string_lossy().to_string();
        git(
            root,
            &[
                "worktree",
                "add",
                "--detach",
                "--quiet",
                &dir_s,
                &walk[0].sha,
            ],
        )?;
        let target = target_dir(root);
        for commit in &walk {
            let outcome = check_one(&dir, &target, commit, &oneline(&commit.sha), full);
            let broken = outcome.result.is_err();
            results.push(outcome);
            if broken {
                break;
            }
        }
        remove_worktree(root, &dir);
    }

    let report = summary(&results, &skipped, full);
    eprintln!("\n{report}");
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        use std::io::Write as _;
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(report.as_bytes()));
        if let Err(e) = written {
            eprintln!("xtask: cannot write the job summary: {e}");
        }
    }
    if let Some(Outcome {
        line,
        result: Err(step),
        ..
    }) = results.last()
    {
        eprintln!("xtask: commit {line} is broken: {step} failed");
        Ok(ExitCode::FAILURE)
    } else {
        eprintln!("xtask: every commit checked builds and passes its tests");
        Ok(ExitCode::SUCCESS)
    }
}

/// The selection of a commit checked out in `dir`: its diff against its
/// parent, mapped on its own workspace.
fn selection_at(dir: &Path, commit: &Commit) -> Result<Selection, String> {
    let paths: Vec<String> = match &commit.parent {
        Some(parent) => git(
            dir,
            &["diff", "--name-only", "--no-renames", parent, &commit.sha],
        )?
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect(),
        None => Vec::new(),
    };
    let graph = affected::workspace(dir)?;
    Ok(select_commit(&graph, commit.parent.as_deref(), &paths))
}

fn check_one(dir: &Path, target: &Path, commit: &Commit, line: &str, full: bool) -> Outcome {
    eprintln!("\nxtask: === {line}");
    let started = Instant::now();
    let outcome = |tests: String, result| Outcome {
        line: line.to_string(),
        tests,
        result,
        seconds: started.elapsed().as_secs(),
    };
    let checked_out = Command::new("git")
        .args(["checkout", "--quiet", "--detach", "--force", &commit.sha])
        .current_dir(dir)
        .status();
    if !checked_out.is_ok_and(|s| s.success()) {
        return outcome(String::new(), Err("git checkout"));
    }
    let selection = if full {
        None
    } else {
        match selection_at(dir, commit) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("xtask: {line}: {e}");
                return outcome(String::new(), Err("the selection"));
            }
        }
    };
    let tests = describe(selection.as_ref());
    eprintln!("xtask: {line}: tests: {tests}");
    if let Some(s) = selection
        .as_ref()
        .filter(|s| !s.everything && !s.crates.is_empty())
    {
        let list: Vec<&str> = s.crates.iter().map(String::as_str).collect();
        eprintln!("xtask: {line}: tested crates: {}", list.join(", "));
    }
    let filter = commit_filter(selection.as_ref());
    for step in steps(&filter) {
        eprintln!("xtask: {line}: {}", step.name);
        let status = Command::new("cargo")
            .args(&step.args)
            .current_dir(dir)
            .env("CARGO_TARGET_DIR", target)
            .status();
        if !status.is_ok_and(|s| s.success()) {
            return outcome(tests, Err(step.name));
        }
    }
    outcome(tests, Ok(()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::affected::Crate;

    fn r(base: &str, head: &str) -> (String, String) {
        (base.into(), head.into())
    }

    #[test]
    fn ranges_parse() {
        assert_eq!(parse_range(Some("a..b"), None), Some(r("a", "b")));
        assert_eq!(parse_range(Some("a.."), None), Some(r("a", "HEAD")));
        assert_eq!(parse_range(Some("main"), None), Some(r("main", "HEAD")));
        assert_eq!(parse_range(None, None), Some(r("origin/main", "HEAD")));
        assert_eq!(
            parse_range(None, Some("origin/topic")),
            Some(r("origin/topic", "HEAD"))
        );
        assert_eq!(parse_range(Some("a...b"), None), None, "no symmetric range");
        assert_eq!(parse_range(Some("..b"), None), None);
    }

    #[test]
    fn arguments_parse() {
        assert_eq!(parse_args(&[]), Some((false, None)));
        assert_eq!(parse_args(&["--full"]), Some((true, None)));
        assert_eq!(parse_args(&["a..b", "--full"]), Some((true, Some("a..b"))));
        assert_eq!(parse_args(&["--full", "main"]), Some((true, Some("main"))));
        assert_eq!(parse_args(&["a", "b"]), None, "one range");
        assert_eq!(parse_args(&["--fast"]), None);
        assert_eq!(parse_args(&["--full", "--full"]), None);
    }

    #[test]
    fn null_bases_are_recognised() {
        assert!(is_null_sha(&"0".repeat(40)));
        assert!(!is_null_sha("0a00"));
        assert!(!is_null_sha(""));
    }

    #[test]
    fn the_walk_leaves_the_head_out_and_keeps_the_newest() {
        let c = |n: usize| (0..n).map(|i| format!("c{i}")).collect::<Vec<_>>();
        assert_eq!(plan_walk::<String>(&[], 20), (vec![], vec![]));
        assert_eq!(plan_walk(&c(1), 20), (vec![], vec![]));
        assert_eq!(plan_walk(&c(3), 20), (c(2), vec![]));
        let (walk, skipped) = plan_walk(&c(25), 20);
        assert_eq!(walk.first().map(String::as_str), Some("c4"));
        assert_eq!(walk.last().map(String::as_str), Some("c23"));
        assert_eq!(skipped, c(4));
    }

    /// `git rev-list --reverse --parents base..head` of a series: a root
    /// commit, a plain one, a merge, and the head.
    const HISTORY: &str = "a1\nb2 a1\nc3 b2 f9\nd4 c3\n";

    #[test]
    fn each_commit_is_diffed_against_its_first_parent() {
        let commits = parse_history(HISTORY);
        let parents: Vec<(&str, Option<&str>)> = commits
            .iter()
            .map(|c| (c.sha.as_str(), c.parent.as_deref()))
            .collect();
        assert_eq!(
            parents,
            [
                ("a1", None),
                ("b2", Some("a1")),
                ("c3", Some("b2")),
                ("d4", Some("c3"))
            ],
            "a merge against the line it was merged into"
        );
        let (walk, _) = plan_walk(&commits, CAP);
        assert_eq!(walk.len(), 3, "the head is left to the full gate");
        assert!(parse_history("").is_empty());
    }

    /// core <- photo <- load; core <- gui; xtask stands alone.
    fn graph() -> Graph {
        let c = |name: &str, dir: &str, deps: &[&str]| Crate {
            name: name.into(),
            dir: dir.into(),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
        };
        Graph {
            crates: vec![
                c("helios-core", "crates/helios-core", &[]),
                c("helios-photo", "crates/helios-photo", &["helios-core"]),
                c("helios-load", "crates/helios-load", &["helios-photo"]),
                c("helios-gui", "crates/helios-gui", &["helios-core"]),
                c("xtask", "xtask", &[]),
            ],
        }
    }

    fn paths(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_commit_runs_the_fast_tests_of_what_it_touches_and_the_critical_set() {
        let s = select_commit(
            &graph(),
            Some("a1"),
            &paths(&["crates/helios-photo/src/x.rs"]),
        );
        assert_eq!(
            s.crates,
            BTreeSet::from(["helios-photo".to_string(), "helios-load".to_string()])
        );
        assert_eq!(
            commit_filter(Some(&s)),
            format!(
                "((package(=helios-load) | package(=helios-photo)) & ({FAST_TESTS})) | ({CRITICAL})"
            )
        );
        let d = describe(Some(&s));
        assert!(
            d.contains("2 crate(s): changed helios-photo, 1 dependent(s)"),
            "{d}"
        );
        assert!(d.contains("critical set"), "{d}");
    }

    #[test]
    fn a_commit_no_test_reads_runs_the_critical_set_alone() {
        let s = select_commit(
            &graph(),
            Some("a1"),
            &paths(&["docs/bench.md", "changelog.d/x.md"]),
        );
        assert_eq!(commit_filter(Some(&s)), format!("({CRITICAL})"));
        assert!(describe(Some(&s)).contains("critical set alone"));
        let s = select_commit(&graph(), Some("a1"), &[]);
        assert_eq!(
            commit_filter(Some(&s)),
            format!("({CRITICAL})"),
            "an empty commit"
        );
    }

    #[test]
    fn a_commit_that_runs_everything_runs_every_fast_test() {
        for p in ["Cargo.lock", "xtask/src/commits.rs", "a/new/place.txt"] {
            let s = select_commit(&graph(), Some("a1"), &paths(&["docs/x.md", p]));
            assert!(s.everything, "{p}");
            assert_eq!(
                commit_filter(Some(&s)),
                format!("({FAST_TESTS}) | ({CRITICAL})"),
                "{p}"
            );
            assert!(describe(Some(&s)).contains(p), "the reason names {p}");
        }
        let s = select_commit(&graph(), None, &[]);
        assert!(s.everything, "a root commit has nothing to diff against");
        assert_eq!(s.crates.len(), graph().crates.len());
        assert!(describe(Some(&s)).contains("a root commit"));
    }

    #[test]
    fn full_runs_every_fast_test_on_every_commit() {
        assert_eq!(commit_filter(None), FAST_TESTS);
        assert!(describe(None).contains("--full"));
    }

    #[test]
    fn the_summary_names_the_selection_the_broken_commit_and_the_skipped_ones() {
        let o = |line: &str, tests: &str, result, seconds| Outcome {
            line: line.into(),
            tests: tests.into(),
            result,
            seconds,
        };
        let s = summary(
            &[
                o(
                    "a1 first",
                    "the critical set alone: no crate touched",
                    Ok(()),
                    61,
                ),
                o("b2 second", "every fast test (--full)", Err("tests"), 5),
            ],
            &["z9 old".into()],
            false,
        );
        assert!(s.contains("| a1 first | the critical set alone: no crate touched | ok | 1m01s |"));
        assert!(s.contains(
            "| b2 second | every fast test (--full) | **broken**: tests failed | 0m05s |"
        ));
        assert!(s.contains("Total: 1m06s."));
        assert!(s.contains("z9 old"));
        assert!(summary(&[], &[], false).contains("nothing to check"));
    }
}
