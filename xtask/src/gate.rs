//! `cargo xtask gate <PR>…`: the tester's tool (docs/next.md, "The
//! tester"). It takes a batch of pull requests into `next`, merges them on
//! a temporary branch made from `next`, runs the union of their test
//! manifests on the bench box, and fast-forwards `next` to that branch
//! when every job passes.
//!
//! 1. Each pull request must target `next` and carry a valid manifest
//!    (`manifest.rs`); one that does not is left out with its reason.
//! 2. A temporary branch `gate/<n>` is made from `origin/next` and each
//!    pull request's branch merged into it (`--no-ff`, "Merge pull request
//!    #N from …"). A branch that does not merge cleanly is left out.
//! 3. The temporary branch is pushed and every job of the merged manifests
//!    runs on it with an `ext_job_id` of `gate-<pr>-<n>`, side by side (one
//!    build on the box).
//! 4. All green: `next` fast-forwards to the tested commit (refused if
//!    `next` moved meanwhile: run the gate again), GitHub marks the pull
//!    requests merged, and their branches and the temporary one are
//!    deleted. A red job prints its pull request, job and box id; `next`
//!    does not move, the temporary branch is deleted, and the tester fixes
//!    the pull request's own branch (mechanical) or sends it back, then
//!    gates again.
//! 5. Beside the jobs, the box builds the tested commit for the desktop
//!    (`build-gui`); a failed build is red. After a green round its programs
//!    are here, and `cargo xtask install-dev --from <dir>` installs them.
//!
//! The parts that need no network (the plan) are pure and tested; the
//! rest drives git, gh and `cargo xtask bench-remote`.

use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, ExitCode};

use crate::manifest::{self, Manifest};

pub const USAGE: &str = "gate <PR>…  (merge pull requests into next on a temporary branch, run their manifests on the box, fast-forward next when green)";

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
    /// `bench-remote` arguments: the job, its parameters, `branch=`, the id.
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

/// The temporary branch a batch is tested on.
pub fn temp_branch(numbers: &[u64]) -> String {
    let ids: Vec<String> = numbers.iter().map(u64::to_string).collect();
    format!("gate/{}", ids.join("-"))
}

/// The plan for `prs`, tested on `branch` (`benchfile` for the manifests'
/// jobs).
pub fn plan(prs: &[Pr], benchfile: &str, branch: &str) -> Plan {
    let mut plan = Plan::default();
    for pr in prs {
        if pr.base != "next" {
            plan.left_out
                .push((pr.number, format!("targets `{}`, not `next`", pr.base)));
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
                    args.push(format!("branch={branch}"));
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
            "headRefName,baseRefName,title,body",
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
    let n: Option<Vec<u64>> = args
        .iter()
        .map(|a| a.trim_start_matches('#').parse().ok())
        .collect();
    let numbers = match n {
        Some(n) if !n.is_empty() => n,
        _ => return None,
    };
    Some(match gate(root, &numbers) {
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

/// Uploads `file` as the box's Benchfile for `branch`.
fn upload_benchfile(root: &Path, branch: &str, file: &Path) -> Result<(), String> {
    let bench = crate::remote::Bench::configure(root)?;
    let input = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let status = bench
        .ssh(10, &["config", "benchfile", branch])
        .stdin(input)
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "the box refused the Benchfile for {branch} ({status})"
        ))
    }
}

/// Drops the Benchfile uploaded for `branch`.
fn forget_benchfile(root: &Path, branch: &str) -> Result<(), String> {
    let bench = crate::remote::Bench::configure(root)?;
    bench
        .ssh(10, &["config", "benchfile", "rm", branch])
        .status()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Runs the gate from the checkout `root`; the report either way.
fn gate(root: &Path, numbers: &[u64]) -> Result<String, String> {
    let benchfile = std::fs::read_to_string(root.join("tools/bench/Benchfile")).unwrap_or_default();
    let temp = temp_branch(numbers);
    // A worktree of its own, so the caller's checkout is left alone.
    let work = root.join("target/gate");
    git(root, &["fetch", "-q", "origin", "next", "main"])?;
    if !work.exists() {
        git(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                work.to_str().unwrap_or(""),
                "origin/next",
            ],
        )?;
    }
    git(&work, &["fetch", "-q", "origin", "next"])?;
    git(&work, &["checkout", "-q", "--detach", "origin/next"])?;
    let base = git(&work, &["rev-parse", "HEAD"])?;
    let mut report = String::new();

    let prs: Vec<Pr> = numbers.iter().map(|n| pr(*n)).collect::<Result<_, _>>()?;
    let mut plan = plan(&prs, &benchfile, &temp);

    // Merge the taken branches on the temporary branch.
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
                .push((p.number, format!("does not merge into next: {e}")));
        } else {
            merged.push(p.number);
        }
    }
    plan.take.retain(|n| merged.contains(n));
    plan.runs.retain(|r| merged.contains(&r.pr));
    for (n, why) in &plan.left_out {
        let _ = writeln!(report, "left out #{n}: {why}");
    }
    if plan.take.is_empty() {
        return Err(format!("{report}nothing to test"));
    }
    let tested = git(&work, &["rev-parse", "HEAD"])?;
    git(
        &work,
        &[
            "push",
            "-q",
            "-f",
            "origin",
            &format!("HEAD:refs/heads/{temp}"),
        ],
    )?;
    // The box reads the default Benchfile (main's) for a branch it has none
    // for: a job only next declares would be refused. Upload the tested
    // Benchfile under the temporary branch when it differs from main's.
    let tested_benchfile = git(&work, &["show", "HEAD:tools/bench/Benchfile"]).unwrap_or_default();
    let main_benchfile =
        git(&work, &["show", "origin/main:tools/bench/Benchfile"]).unwrap_or_default();
    let own_benchfile = tested_benchfile != main_benchfile;
    if own_benchfile {
        let path = work.join("tools/bench/Benchfile");
        upload_benchfile(root, &temp, &path)?;
    }
    let drop_temp = || {
        let _ = git(root, &["push", "-q", "origin", "--delete", &temp]);
        if own_benchfile {
            let _ = forget_benchfile(root, &temp);
        }
    };

    // Run the jobs side by side: spawned together, then waited on. The
    // desktop build of the tested commit runs beside them, so a green round
    // brings back the programs to install (`install-dev --from`).
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let build = Command::new(&exe)
        .args([
            "bench-remote",
            "build-gui",
            &format!("branch={temp}"),
            "ext_job_id=gate-build",
        ])
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let children: Vec<_> = plan
        .runs
        .iter()
        .enumerate()
        .map(|(i, r)| {
            // Spaced out: a burst of ssh connections is refused by the box's
            // sshd (MaxStartups), and the refused job is never submitted.
            if i > 0 {
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
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
                    // No line names the job: it was never submitted; say why.
                    .or_else(|| text.lines().rev().find(|l| !l.trim().is_empty()))
                    .unwrap_or("no output")
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

    // The desktop build: its outputs' folder when it built, else red.
    let desktop = match build.and_then(std::process::Child::wait_with_output) {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout).to_string()
                + &String::from_utf8_lossy(&o.stderr);
            text.lines()
                .filter_map(|l| l.split("results in ").nth(1))
                .last()
                .map(|dir| Path::new(dir.trim()).join("out"))
        }
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout).to_string()
                + &String::from_utf8_lossy(&o.stderr);
            let line = text
                .lines()
                .rev()
                .find(|l| l.contains("gate-build"))
                .unwrap_or("no output");
            let _ = writeln!(report, "desktop build RED: {line}");
            None
        }
        Err(e) => {
            let _ = writeln!(report, "desktop build RED: not started: {e}");
            None
        }
    };
    if red.is_empty() && desktop.is_none() {
        drop_temp();
        let _ = writeln!(
            report,
            "red: the desktop build of the batch — next stays at {}",
            &base[..8]
        );
        return Err(report);
    }

    if !red.is_empty() {
        red.dedup();
        drop_temp();
        let _ = writeln!(
            report,
            "red: {} — next stays at {}; fix the pull request's branch (mechanical) or send it back, then gate again",
            red.iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", "),
            &base[..8]
        );
        return Err(report);
    }
    // Green: next fast-forwards to what was tested, unless it moved meanwhile.
    git(&work, &["fetch", "-q", "origin", "next"])?;
    if git(
        &work,
        &["merge-base", "--is-ancestor", "origin/next", &tested],
    )
    .is_err()
    {
        drop_temp();
        return Err(format!(
            "{report}green, but next moved during the run: gate the same pull requests again"
        ));
    }
    git(
        &work,
        &["push", "-q", "origin", &format!("{tested}:refs/heads/next")],
    )?;
    let _ = writeln!(report, "green: next is now {}", &tested[..8]);
    drop_temp();
    for p in prs.iter().filter(|p| plan.take.contains(&p.number)) {
        let _ = git(root, &["push", "-q", "origin", "--delete", &p.branch]);
    }
    if let Some(dir) = &desktop {
        let _ = writeln!(
            report,
            "desktop build of next {}: cargo xtask install-dev --from {}",
            &tested[..8],
            dir.display()
        );
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
    fn the_plan_takes_valid_pull_requests_into_next_and_numbers_their_runs() {
        let prs = [
            pr(
                1,
                "next",
                "Tests:\n- test-crate crate=a test=lib\n- i18n-extract\nCoverage: x.\n",
            ),
            pr(2, "main", "Tests: none — docs\n"),
            pr(3, "next", "no manifest"),
            pr(4, "next", "Tests: none — docs\n"),
        ];
        let p = plan(&prs, BF, "gate/1-4");
        assert_eq!(p.take, [1, 4]);
        assert_eq!(p.left_out.len(), 2);
        assert!(p.left_out[0].1.contains("not `next`"));
        assert!(p.left_out[1].1.contains("manifest"));
        assert_eq!(p.runs.len(), 2);
        assert_eq!(
            p.runs[0].args,
            [
                "test-crate",
                "crate=a",
                "test=lib",
                "branch=gate/1-4",
                "ext_job_id=gate-1-1"
            ]
        );
        assert_eq!(p.runs[1].ext_job_id, "gate-1-2");
    }

    #[test]
    fn a_batch_is_tested_on_a_branch_named_after_its_pull_requests() {
        assert_eq!(temp_branch(&[187, 189]), "gate/187-189");
    }
}
