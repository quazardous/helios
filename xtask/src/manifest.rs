//! `cargo xtask manifest check [FILE|-]`: the test manifest a pull request
//! into `devel` carries (docs/next.md, "For a change"). The tester runs the
//! jobs it lists on the bench box; this check refuses a manifest the box
//! would run wrong before anyone waits on it.
//!
//! The manifest is a block in the pull request's body:
//!
//! ```text
//! Tests:
//! - test-crate crate=helios-gui test=library_filter
//! - test-crate crate=helios-vault test=lib filter=albums_nest
//! - i18n-extract
//! Coverage: the drop rules on an album, a smart album and a set.
//! ```
//!
//! - `Tests:` starts the block; each following line `- <job> [key=value]…`
//!   is one bench-remote job, as `cargo xtask bench-remote <job> …` takes
//!   it. The block ends at a blank line or at the next `Word:` line.
//! - `Tests: none — <why>` declares a change with nothing to run (docs,
//!   a comment); the reason is required.
//! - `Coverage:` says in a sentence or two what the jobs prove. Required
//!   with jobs.
//! - A job must exist in `tools/bench/Benchfile` and carry its required
//!   parameters (`branch` excepted: the tester gives it).
//! - `filter=` names one test: a second name (a space, a `+` or a comma)
//!   makes the box's filter match nothing, so the job would pass empty.
//! - `test-affected`, `test` and `--affected` are refused: a change tests
//!   its own part; the whole suite is `next`'s CI.

use std::io::Read as _;
use std::path::Path;
use std::process::ExitCode;

pub const USAGE: &str = "manifest check [FILE|-]  (a pull request body; - or nothing reads stdin)";

/// One job of a manifest.
#[derive(Debug, PartialEq, Eq)]
pub struct Job {
    pub name: String,
    pub params: Vec<(String, String)>,
}

/// A manifest read from a pull request's body.
#[derive(Debug, PartialEq, Eq)]
pub enum Manifest {
    /// Jobs to run, with what they prove.
    Jobs { jobs: Vec<Job>, coverage: String },
    /// Nothing to run, and why.
    Nothing { why: String },
}

/// Jobs a change never asks for: they reach the whole suite.
const WIDE: [&str; 2] = ["test", "test-affected"];

/// The manifest in `body`, or every reason it is refused (`benchfile` is
/// `tools/bench/Benchfile`'s text, for the job names and their parameters).
pub fn parse(body: &str, benchfile: &str) -> Result<Manifest, Vec<String>> {
    let lines: Vec<&str> = body.lines().map(str::trim_end).collect();
    let Some(start) = lines
        .iter()
        .position(|l| l.trim_start().starts_with("Tests:"))
    else {
        return Err(vec![
            "no `Tests:` block: list the box jobs that prove the change, or `Tests: none — <why>`"
                .into(),
        ]);
    };
    let head = lines[start].trim_start()["Tests:".len()..].trim();
    let mut problems = Vec::new();
    if let Some(rest) = head.strip_prefix("none") {
        let why = rest
            .trim_start_matches(|c: char| c == '—' || c == '-' || c == ':' || c.is_whitespace());
        if why.is_empty() {
            return Err(vec![
                "`Tests: none` needs its reason: `Tests: none — <why>`".into(),
            ]);
        }
        return Ok(Manifest::Nothing {
            why: why.to_string(),
        });
    }
    if !head.is_empty() {
        problems.push(format!(
            "`Tests:` takes its jobs on the lines below it, not `{head}`"
        ));
    }
    let mut jobs = Vec::new();
    for line in &lines[start + 1..] {
        let t = line.trim();
        if t.is_empty() || is_header(t) {
            break;
        }
        let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) else {
            problems.push(format!("`{t}`: a job line starts with `- `"));
            continue;
        };
        match job(item, benchfile) {
            Ok(j) => jobs.push(j),
            Err(e) => problems.extend(e),
        }
    }
    if jobs.is_empty() && problems.is_empty() {
        problems.push("`Tests:` lists no job (use `Tests: none — <why>` when nothing runs)".into());
    }
    let coverage = lines
        .iter()
        .find_map(|l| l.trim_start().strip_prefix("Coverage:"))
        .map_or("", str::trim);
    if coverage.is_empty() {
        problems.push("no `Coverage:` line: say in a sentence what the jobs prove".into());
    }
    if problems.is_empty() {
        Ok(Manifest::Jobs {
            jobs,
            coverage: coverage.to_string(),
        })
    } else {
        Err(problems)
    }
}

/// `Word:` at the start of a line: the next block of the body.
fn is_header(line: &str) -> bool {
    line.split_once(':').is_some_and(|(w, _)| {
        !w.is_empty() && !w.contains(' ') && w.chars().next().is_some_and(char::is_alphabetic)
    }) && !line.starts_with("- ")
}

fn job(item: &str, benchfile: &str) -> Result<Job, Vec<String>> {
    let mut words = item.split_whitespace();
    let name = words.next().unwrap_or("").to_string();
    let mut problems = Vec::new();
    if WIDE.contains(&name.as_str()) || item.contains("--affected") {
        return Err(vec![format!(
            "`{item}`: a change tests its own part (`test-crate`, `egui-pop-test`…), never the whole suite"
        )]);
    }
    if let Some(why) = crate::remote::unknown_job(&name, benchfile) {
        return Err(vec![format!("`{item}`: {why}")]);
    }
    let mut params = Vec::new();
    for w in words {
        let Some((k, v)) = w.split_once('=') else {
            problems.push(format!("`{item}`: `{w}` is not `key=value`"));
            continue;
        };
        if k == "filter" && v.contains(['+', ',']) {
            problems.push(format!(
                "`{item}`: `filter={v}` names more than one test, which selects none; one name per job"
            ));
        }
        if k == "branch" {
            problems.push(format!("`{item}`: no `branch=`, the tester gives it"));
        }
        params.push((k.to_string(), v.to_string()));
    }
    for need in crate::remote::required_params(benchfile, &name) {
        if !params.iter().any(|(k, _)| *k == need) {
            problems.push(format!("`{item}`: `{name}` needs `{need}=`"));
        }
    }
    if problems.is_empty() {
        Ok(Job { name, params })
    } else {
        Err(problems)
    }
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let source = match args {
        ["check"] | ["check", "-"] => None,
        ["check", file] => Some(*file),
        _ => return None,
    };
    let body = match source {
        None => {
            let mut s = String::new();
            if std::io::stdin().read_to_string(&mut s).is_err() {
                eprintln!("cannot read the manifest from stdin");
                return Some(ExitCode::from(2));
            }
            s
        }
        Some(f) => match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("cannot read {f}: {e}");
                return Some(ExitCode::from(2));
            }
        },
    };
    let benchfile = std::fs::read_to_string(root.join("tools/bench/Benchfile")).unwrap_or_default();
    Some(match parse(&body, &benchfile) {
        Ok(Manifest::Jobs { jobs, .. }) => {
            println!("manifest OK: {} job(s)", jobs.len());
            ExitCode::SUCCESS
        }
        Ok(Manifest::Nothing { why }) => {
            println!("manifest OK: nothing to run ({why})");
            ExitCode::SUCCESS
        }
        Err(problems) => {
            for p in problems {
                eprintln!("{p}");
            }
            ExitCode::from(1)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BF: &str = "[jobs.test-crate]\ncommand = \"\"\"cargo test -p ${crate} ${test:+--test $test} ${filter:-} ${branch}\"\"\"\n[jobs.i18n-extract]\ncommand = \"\"\"x\"\"\"\n[jobs.test-affected]\ncommand = \"\"\"y\"\"\"\n[jobs.test]\ncommand = \"\"\"z\"\"\"\n";

    #[test]
    fn a_manifest_lists_its_jobs_and_what_they_prove() {
        let body = "Adds the thing.\n\nTests:\n- test-crate crate=helios-gui test=library_filter\n- test-crate crate=helios-vault test=lib filter=albums_nest\n- i18n-extract\nCoverage: the drop rules.\n";
        let Ok(Manifest::Jobs { jobs, coverage }) = parse(body, BF) else {
            panic!("{:?}", parse(body, BF))
        };
        assert_eq!(jobs.len(), 3);
        assert_eq!(jobs[1].params[2], ("filter".into(), "albums_nest".into()));
        assert_eq!(coverage, "the drop rules.");
    }

    #[test]
    fn nothing_to_run_needs_its_reason() {
        assert_eq!(
            parse("Tests: none — docs only\n", BF),
            Ok(Manifest::Nothing {
                why: "docs only".into()
            })
        );
        assert!(parse("Tests: none\n", BF).is_err());
    }

    #[test]
    fn what_the_box_would_run_wrong_is_refused() {
        let refused = |body: &str| parse(body, BF).unwrap_err().join("\n");
        assert!(refused("no block here").contains("no `Tests:` block"));
        assert!(
            refused("Tests:\n- test-crate crate=x filter=a+b\nCoverage: y\n")
                .contains("more than one test")
        );
        assert!(
            refused("Tests:\n- test-crate crate=x filter=a,b\nCoverage: y\n")
                .contains("more than one test")
        );
        assert!(
            refused("Tests:\n- test-affected\nCoverage: y\n").contains("never the whole suite")
        );
        assert!(refused("Tests:\n- test\nCoverage: y\n").contains("never the whole suite"));
        assert!(refused("Tests:\n- nope crate=x\nCoverage: y\n").contains("no job `nope`"));
        assert!(refused("Tests:\n- test-crate test=lib\nCoverage: y\n").contains("needs `crate=`"));
        assert!(
            refused("Tests:\n- test-crate crate=x branch=b\nCoverage: y\n")
                .contains("the tester gives it")
        );
        assert!(refused("Tests:\n- test-crate crate=x\n").contains("no `Coverage:`"));
        assert!(refused("Tests:\n\nCoverage: y\n").contains("lists no job"));
    }

    #[test]
    fn the_block_ends_at_a_blank_line_or_the_next_header() {
        let body = "Tests:\n- i18n-extract\nCoverage: strings.\n- test-crate crate=ignored\n";
        let Ok(Manifest::Jobs { jobs, .. }) = parse(body, BF) else {
            panic!()
        };
        assert_eq!(jobs.len(), 1);
    }
}
