//! `cargo xtask gate <PR>…`: the tester's tool (docs/next.md, "The
//! tester"). It takes pull requests into `devel`, merges them into
//! `devel`, runs the union of their test manifests on the bench box, and
//! fast-forwards `next` to `devel` when every job passes.
//!
//! 1. Each pull request must target `devel` and carry a valid manifest
//!    (`manifest.rs`); one that does not is left out with its reason.
//! 2. `devel` is fetched, brought up to `next` if the promotion bot moved
//!    `next` (a merge, pushed), and each branch merged into it (`--no-ff`, the
//!    pull request's title as the message). A branch that does not merge
//!    cleanly is left out; the others go on.
//! 3. `devel` is pushed: GitHub marks the pull requests merged.
//! 4. Every job of the merged manifests runs with `branch=devel` and an
//!    `ext_job_id` of `gate-<pr>-<n>`, side by side (one build on the box).
//! 5. All green: `next` fast-forwards to the tested commit (refused if
//!    `next` is not its ancestor), and the merged branches are deleted.
//!    A red job prints its pull request, job and box id; nothing moves,
//!    and the tester fixes `devel` (mechanical) or reverts the guilty
//!    merge (`git revert -m 1`) and runs the gate again with no number
//!    (`cargo xtask gate --retest`), which re-runs the jobs of every merge
//!    on `devel` since `next`.
//!
//! The parts that need no network (the plan) are pure and tested; the
//! rest drives git, gh and `cargo xtask bench-remote`.

use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, ExitCode};

use crate::manifest::{self, Manifest};

pub const USAGE: &str = "gate <PR>… | gate --retest  (merge pull requests into devel, run their manifests on the box, fast-forward next when green)";

/// A pull request the gate takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pr {
    pub number: u64,
    pub branch: String,
    pub base: String,
    pub title: String,
    pub body: String,
}

/// One box run the gate starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub pr: u64,
    pub ext_job_id: String,
    /// `bench-remote` arguments: the job, its parameters, `branch=devel`,
    /// the id.
    pub args: Vec<String>,
}

/// What the gate does with a batch, decided before anything moves: the
/// pull requests it takes with their runs, and the ones it leaves out with
/// why.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub take: Vec<u64>,
    pub runs: Vec<Run>,
    pub left_out: Vec<(u64, String)>,
}

/// The plan for `prs` (`benchfile` for the manifests' jobs).
pub fn plan(prs: &[Pr], benchfile: &str) -> Plan {
    let mut plan = Plan::default();
    for pr in prs {
        if pr.base != "devel" {
            plan.left_out
                .push((pr.number, format!("targets `{}`, not `devel`", pr.base)));
            continue;
        }
        match manifest::parse(&pr.body, benchfile) {
            Err(problems) => plan
                .left_out
                .push((pr.number, format!("manifest: {}", problems.join("; ")))),
            Ok(Manifest::Nothing { .. }) => plan.take.push(pr.number),
            Ok(Manifest::Jobs { jobs, .. }) => {
                plan.take.push(pr.number);
                for (n, job) in jobs.iter().enumerate() {
                    let ext = format!("gate-{}-{}", pr.number, n + 1);
                    let mut args = vec![job.name.clone()];
                    args.extend(job.params.iter().map(|(k, v)| format!("{k}={v}")));
                    args.push("branch=devel".into());
                    args.push(format!("ext_job_id={ext}"));
                    plan.runs.push(Run {
                        pr: pr.number,
                        ext_job_id: ext,
                        args,
                    });
                }
            }
        }
    }
    plan
}

/// The pull request numbers merged on `devel` since `next`, read from the
/// merge commits' subjects GitHub and the gate write
/// ("Merge pull request #N …" or "… (#N)").
pub fn merged_since(log: &str) -> Vec<u64> {
    let mut out = Vec::new();
    for line in log.lines() {
        let num = line
            .split_once("Merge pull request #")
            .map(|(_, r)| r)
            .or_else(|| line.rsplit_once("(#").map(|(_, r)| r))
            .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|d| d.parse().ok());
        if let Some(n) = num
            && !out.contains(&n)
        {
            out.push(n);
        }
    }
    out
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
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

fn pr(number: u64) -> Result<Pr, String> {
    let out = Command::new("gh")
        .args([
            "pr",
            "view",
            &number.to_string(),
            "--json",
            "headRefName,baseRefName,title,body,state",
        ])
        .output()
        .map_err(|e| format!("cannot run gh: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "#{number}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("#{number}: {e}"))?;
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    Ok(Pr {
        number,
        branch: s("headRefName"),
        base: s("baseRefName"),
        title: s("title"),
        body: s("body"),
    })
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let retest = matches!(args, ["--retest"]);
    let numbers: Vec<u64> = if retest {
        Vec::new()
    } else {
        let n: Option<Vec<u64>> = args
            .iter()
            .map(|a| a.trim_start_matches('#').parse().ok())
            .collect();
        match n {
            Some(n) if !n.is_empty() => n,
            _ => return None,
        }
    };
    Some(match gate(root, &numbers, retest) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(report) => {
            eprintln!("{report}");
            ExitCode::from(1)
        }
    })
}

/// Runs the gate from the checkout `root`; the report either way.
fn gate(root: &Path, numbers: &[u64], retest: bool) -> Result<String, String> {
    let benchfile = std::fs::read_to_string(root.join("tools/bench/Benchfile")).unwrap_or_default();
    // A worktree of its own on `devel`, so the caller's checkout is left alone.
    let work = root.join("target/gate-devel");
    git(root, &["fetch", "-q", "origin", "devel", "next"])?;
    if !work.exists() {
        git(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                work.to_str().unwrap_or(""),
                "origin/devel",
            ],
        )?;
    }
    git(&work, &["checkout", "-q", "--detach", "origin/devel"])?;
    let mut report = String::new();
    // The promotion bot commits releases on next: bring devel up to it, so
    // next can always fast-forward to what is tested.
    if git(
        &work,
        &["merge-base", "--is-ancestor", "origin/next", "HEAD"],
    )
    .is_err()
    {
        git(
            &work,
            &[
                "merge",
                "-q",
                "--no-edit",
                "-m",
                "Merge next into devel",
                "origin/next",
            ],
        )
        .map_err(|e| format!("devel does not take next's commits: {e}"))?;
        git(&work, &["push", "-q", "origin", "HEAD:refs/heads/devel"])?;
        let _ = writeln!(report, "devel brought up to next");
    }

    let prs: Vec<Pr> = if retest {
        let log = git(&work, &["log", "--format=%s", "origin/next..HEAD"])?;
        merged_since(&log)
            .into_iter()
            .map(pr)
            .collect::<Result<_, _>>()?
    } else {
        numbers.iter().map(|n| pr(*n)).collect::<Result<_, _>>()?
    };
    let mut plan = plan(
        &prs.iter()
            .map(|p| {
                // a retest takes merged pull requests: their base was devel
                let mut p = p.clone();
                if retest {
                    p.base = "devel".into();
                }
                p
            })
            .collect::<Vec<_>>(),
        &benchfile,
    );

    // Merge the taken branches into devel.
    if !retest {
        let mut merged = Vec::new();
        for p in prs.iter().filter(|p| plan.take.contains(&p.number)) {
            git(&work, &["fetch", "-q", "origin", &p.branch])?;
            let msg = format!(
                "Merge pull request #{} from {}\n\n{}",
                p.number, p.branch, p.title
            );
            if let Err(e) = git(&work, &["merge", "-q", "--no-ff", "-m", &msg, "FETCH_HEAD"]) {
                let _ = git(&work, &["merge", "--abort"]);
                plan.left_out
                    .push((p.number, format!("does not merge into devel: {e}")));
            } else {
                merged.push(p.number);
            }
        }
        plan.take.retain(|n| merged.contains(n));
        plan.runs.retain(|r| merged.contains(&r.pr));
        if !merged.is_empty() {
            git(&work, &["push", "-q", "origin", "HEAD:refs/heads/devel"])?;
        }
    }
    for (n, why) in &plan.left_out {
        let _ = writeln!(report, "left out #{n}: {why}");
    }
    if plan.take.is_empty() {
        return Err(format!("{report}nothing to test"));
    }
    let tested = git(&work, &["rev-parse", "HEAD"])?;

    // Run the jobs side by side.
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // spawned together, then waited on: the box runs them side by side
    let children: Vec<_> = plan
        .runs
        .iter()
        .map(|r| {
            Command::new(&exe)
                .arg("bench-remote")
                .args(&r.args)
                .current_dir(root)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|c| c.and_then(std::process::Child::wait_with_output))
        .collect();
    let mut red = Vec::new();
    for (r, out) in plan.runs.iter().zip(children) {
        let (ok, line) = match out {
            Ok(o) => {
                let text = String::from_utf8_lossy(&o.stdout).to_string()
                    + &String::from_utf8_lossy(&o.stderr);
                let line = text
                    .lines()
                    .rev()
                    .find(|l| l.contains(&r.ext_job_id))
                    .unwrap_or("")
                    .to_string();
                (o.status.success() && !line.contains(" exit "), line)
            }
            Err(e) => (false, e.to_string()),
        };
        let _ = writeln!(
            report,
            "#{} {}: {}",
            r.pr,
            if ok { "ok" } else { "RED" },
            line
        );
        if !ok {
            red.push(r.pr);
        }
    }

    if !red.is_empty() {
        red.dedup();
        let _ = writeln!(
            report,
            "red: {} — fix devel (mechanical) or revert the guilty merge (`git revert -m 1`), then `cargo xtask gate --retest`; next stays at origin/next",
            red.iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Err(report);
    }
    // Green: next fast-forwards to what was tested.
    if git(
        &work,
        &["merge-base", "--is-ancestor", "origin/next", &tested],
    )
    .is_err()
    {
        return Err(format!(
            "{report}green, but next is not an ancestor of devel: rebase devel on next first"
        ));
    }
    git(
        &work,
        &["push", "-q", "origin", &format!("{tested}:refs/heads/next")],
    )?;
    let _ = writeln!(report, "green: next is now {}", &tested[..8]);
    if !retest {
        for p in prs.iter().filter(|p| plan.take.contains(&p.number)) {
            let _ = git(root, &["push", "-q", "origin", "--delete", &p.branch]);
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BF: &str = "[jobs.test-crate]\ncommand = \"\"\"cargo test -p ${crate} ${test:+--test $test} ${filter:-} ${branch}\"\"\"\n[jobs.i18n-extract]\ncommand = \"\"\"x\"\"\"\n";

    fn pr(number: u64, base: &str, body: &str) -> Pr {
        Pr {
            number,
            branch: format!("feat/{number}"),
            base: base.into(),
            title: format!("Change {number}"),
            body: body.into(),
        }
    }

    #[test]
    fn the_plan_takes_valid_pull_requests_and_numbers_their_runs() {
        let prs = [
            pr(
                1,
                "devel",
                "Tests:\n- test-crate crate=a test=lib\n- i18n-extract\nCoverage: x.\n",
            ),
            pr(2, "next", "Tests: none — docs\n"),
            pr(3, "devel", "no manifest"),
            pr(4, "devel", "Tests: none — docs\n"),
        ];
        let p = plan(&prs, BF);
        assert_eq!(p.take, [1, 4]);
        assert_eq!(p.left_out.len(), 2);
        assert!(p.left_out[0].1.contains("not `devel`"));
        assert!(p.left_out[1].1.contains("manifest"));
        assert_eq!(p.runs.len(), 2);
        assert_eq!(
            p.runs[0].args,
            [
                "test-crate",
                "crate=a",
                "test=lib",
                "branch=devel",
                "ext_job_id=gate-1-1"
            ]
        );
        assert_eq!(p.runs[1].ext_job_id, "gate-1-2");
    }

    #[test]
    fn the_merges_since_next_are_read_from_their_subjects() {
        let log = "Merge pull request #12 from x/y\nfix: a thing\nMerge pull request #14 from z\nSquashed change (#15)\nMerge pull request #12 from x/y\n";
        assert_eq!(merged_since(log), [12, 14, 15]);
    }
}
