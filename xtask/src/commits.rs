//! `cargo xtask check-commits [RANGE]`: every commit of a series builds and
//! passes the fast tests, so `git bisect` keeps working.
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
//! 1. `cargo check --workspace --all-targets`
//! 2. the fast tests: `cargo nextest run --workspace --profile ci -E`
//!    [`FAST_TESTS`]
//! 3. `cargo xtask check`
//!
//! The walk stops at the first failure and names the commit and the step.
//! When `GITHUB_STEP_SUMMARY` is set, the report is appended to it.

use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, ExitCode};

use crate::test_times::target_dir;

pub const USAGE: &str = "check-commits [BASE..HEAD | BASE]";

/// The most commits walked in one run.
pub const CAP: usize = 20;

/// The tests run on each commit: all but the slow ones, which the full
/// gate still runs on the head:
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

/// A base a push event gives when there is no previous head.
pub fn is_null_sha(rev: &str) -> bool {
    !rev.is_empty() && rev.chars().all(|c| c == '0')
}

/// The commits to walk and the ones skipped, from the range's commits
/// listed oldest first: the last one (the head) is left out, then the
/// `cap` newest are kept.
pub fn plan_walk(commits: &[String], cap: usize) -> (Vec<String>, Vec<String>) {
    let Some((_, before_head)) = commits.split_last() else {
        return (Vec::new(), Vec::new());
    };
    let cut = before_head.len().saturating_sub(cap);
    (before_head[cut..].to_vec(), before_head[..cut].to_vec())
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

fn steps() -> Vec<Step> {
    let s = |name, args: &[&str]| Step {
        name,
        args: args.iter().map(std::string::ToString::to_string).collect(),
    };
    vec![
        s("cargo check", &["check", "--workspace", "--all-targets"]),
        s(
            "fast tests",
            &[
                "nextest",
                "run",
                "--workspace",
                "--profile",
                "ci",
                "-E",
                FAST_TESTS,
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

fn summary(walked: &[(String, Result<(), &'static str>)], skipped: &[String]) -> String {
    let mut out = String::from("## Commits of the series\n\n");
    if walked.is_empty() && skipped.is_empty() {
        out.push_str("No commit before the head: nothing to check.\n");
        return out;
    }
    out.push_str("| Commit | Result |\n|--------|--------|\n");
    for (line, result) in walked {
        let result = match result {
            Ok(()) => "ok".to_string(),
            Err(step) => format!("**broken**: {step} failed"),
        };
        let _ = writeln!(out, "| {line} | {result} |");
    }
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
    let arg = match args {
        [] => None,
        [range] if !range.starts_with('-') => Some(*range),
        _ => return None,
    };
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
    Some(match walk(root, &base, &head) {
        Ok(code) => code,
        Err(e) => crate::fail(&[e]),
    })
}

fn walk(root: &Path, base: &str, head: &str) -> Result<ExitCode, String> {
    let head = git(
        root,
        &["rev-parse", "--verify", &format!("{head}^{{commit}}")],
    )?;
    let base = resolve_base(root, base, &head)?;
    let listed = git(root, &["rev-list", "--reverse", &format!("{base}..{head}")])?;
    let commits: Vec<String> = listed.lines().map(str::to_string).collect();
    let (walk, skipped) = plan_walk(&commits, CAP);
    let oneline = |sha: &str| {
        git(root, &["log", "-1", "--format=%h %s", sha]).unwrap_or_else(|_| sha.to_string())
    };
    let skipped: Vec<String> = skipped.iter().map(|c| oneline(c)).collect();
    for s in &skipped {
        eprintln!("xtask: skipped (beyond the cap of {CAP}): {s}");
    }
    eprintln!(
        "xtask: checking {} commit(s) before the head {}",
        walk.len(),
        &head[..head.len().min(12)]
    );

    let mut results = Vec::new();
    if !walk.is_empty() {
        let dir = root.join(WORKTREE);
        remove_worktree(root, &dir);
        let dir_s = dir.to_string_lossy().to_string();
        git(
            root,
            &["worktree", "add", "--detach", "--quiet", &dir_s, &walk[0]],
        )?;
        let target = target_dir(root);
        for sha in &walk {
            let line = oneline(sha);
            let result = check_one(&dir, &target, sha, &line);
            let broken = result.is_err();
            results.push((line, result));
            if broken {
                break;
            }
        }
        remove_worktree(root, &dir);
    }

    let report = summary(&results, &skipped);
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
    if let Some((line, Err(step))) = results.last() {
        eprintln!("xtask: commit {line} is broken: {step} failed");
        Ok(ExitCode::FAILURE)
    } else {
        eprintln!("xtask: every commit checked builds and passes the fast tests");
        Ok(ExitCode::SUCCESS)
    }
}

fn check_one(dir: &Path, target: &Path, sha: &str, line: &str) -> Result<(), &'static str> {
    eprintln!("\nxtask: === {line}");
    let checked_out = Command::new("git")
        .args(["checkout", "--quiet", "--detach", "--force", sha])
        .current_dir(dir)
        .status();
    if !checked_out.is_ok_and(|s| s.success()) {
        return Err("git checkout");
    }
    for step in steps() {
        eprintln!("xtask: {line}: {}", step.name);
        let status = Command::new("cargo")
            .args(&step.args)
            .current_dir(dir)
            .env("CARGO_TARGET_DIR", target)
            .status();
        if !status.is_ok_and(|s| s.success()) {
            return Err(step.name);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn null_bases_are_recognised() {
        assert!(is_null_sha(&"0".repeat(40)));
        assert!(!is_null_sha("0a00"));
        assert!(!is_null_sha(""));
    }

    #[test]
    fn the_walk_leaves_the_head_out_and_keeps_the_newest() {
        let c = |n: usize| (0..n).map(|i| format!("c{i}")).collect::<Vec<_>>();
        assert_eq!(plan_walk(&[], 20), (vec![], vec![]));
        assert_eq!(plan_walk(&c(1), 20), (vec![], vec![]));
        assert_eq!(plan_walk(&c(3), 20), (c(2), vec![]));
        let (walk, skipped) = plan_walk(&c(25), 20);
        assert_eq!(walk.first().map(String::as_str), Some("c4"));
        assert_eq!(walk.last().map(String::as_str), Some("c23"));
        assert_eq!(skipped, c(4));
    }

    #[test]
    fn the_summary_names_the_broken_commit_and_the_skipped_ones() {
        let s = summary(
            &[
                ("a1 first".into(), Ok(())),
                ("b2 second".into(), Err("fast tests")),
            ],
            &["z9 old".into()],
        );
        assert!(s.contains("| b2 second | **broken**: fast tests failed |"));
        assert!(s.contains("z9 old"));
        assert!(summary(&[], &[]).contains("nothing to check"));
    }
}
