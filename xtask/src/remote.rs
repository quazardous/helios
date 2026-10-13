//! `cargo xtask bench-remote <job> [key=value...] [--push]`: a job of
//! `tools/bench/Benchfile` on the bench box, its results fetched locally.
//!
//! 1. `box`: is the box there and serving? Unreachable or in maintenance,
//!    the run stops there.
//! 2. With `--push`, the working tree (tracked and untracked files, not the
//!    ignored ones) goes to the box with `push helios-local-<id>`, one name
//!    per checkout so parallel worktrees never overwrite each other, and the
//!    job runs on it (`src=helios-local-<id>`) instead of the pushed branch.
//! 3. `run <job> [key=value...]`, its log followed; `branch=` defaults to
//!    the current branch. On a dropped connection, `wait <id>` reattaches.
//! 4. `fetch <id>`, extracted to `target/bench-remote/<id>/` (the job's
//!    `log`, `status`, `request` and `out/`), then a summary.
//!
//! With `--affected` instead of a job, the jobs come from `cargo xtask
//! affected` (its `box_jobs`, computed here against `base=`, `origin/main`
//! by default): each one runs in turn as above, `base=` passed to
//! `test-affected` alone, and a table of their results ends the run. The
//! exit code is the first failure's, else 75 when a job could not be
//! served, else 0.
//!
//! Exit 75 (`EX_TEMPFAIL`) means the box could not serve the job: off,
//! unreachable (ssh exits 255), in maintenance, its GPU gone, or restarted
//! under the job. It prints "bench box unavailable: <reason>" and is never
//! a test failure. Otherwise the exit code is the job's.
//!
//! The box is `HELIOS_BENCH_HOST` (`user@host`, or `host` for the user
//! `bench-helios`) and the key `HELIOS_BENCH_KEY` (default
//! `~/.ssh/bench-helios`), from the environment or from the untracked
//! `tools/bench/local.env` (`KEY=value` lines). See docs/bench.md.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::test_times::target_dir;

pub const USAGE: &str = "bench-remote <job> [key=value...] [--push] | bench-remote --affected [base=REF] [key=value...] [--push]";

/// The exit code of a job the box could not serve.
pub const EX_TEMPFAIL: u8 = 75;

/// The name local code is pushed under, at `/work/<name>` on the box: one
/// per checkout (a hash of its path), so two worktrees pushing at once do not
/// overwrite each other's code.
fn pushed_name(root: &Path) -> String {
    // FNV-1a: stable across runs and Rust versions, unlike the std hasher
    let mut h: u32 = 0x811c_9dc5;
    for b in root.to_string_lossy().bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    format!("helios-local-{h:08x}")
}

/// The box's user when `HELIOS_BENCH_HOST` names a host only.
const DEFAULT_USER: &str = "bench-helios";

/// How many times a lost job is reattached to, and the pause before each.
const REATTACH: u32 = 5;
const REATTACH_PAUSE: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub job: String,
    pub params: Vec<(String, String)>,
    pub push: bool,
    /// The jobs come from `cargo xtask affected`: `job` is empty.
    pub affected: bool,
}

impl Request {
    fn param(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// A job or parameter name, as the box accepts it.
fn valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A parameter value, as the box accepts it: `[A-Za-z0-9._/@+-]`.
fn valid_value(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/@+-".contains(c))
}

/// A job name, as the box accepts it: `[a-z][a-z0-9-]*`.
fn valid_job(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn parse_args(args: &[&str]) -> Option<Request> {
    let mut job = None;
    let mut params: Vec<(String, String)> = Vec::new();
    let mut push = false;
    let mut affected = false;
    for a in args {
        if *a == "--push" {
            push = true;
        } else if *a == "--affected" {
            affected = true;
        } else if let Some((k, v)) = a.split_once('=') {
            if !valid_name(k) || !valid_value(v) || params.iter().any(|(p, _)| p == k) {
                return None;
            }
            params.push((k.to_string(), v.to_string()));
        } else if job.is_none() && valid_job(a) {
            job = Some((*a).to_string());
        } else {
            return None;
        }
    }
    // `src` is how --push points the job at the pushed code
    if push && params.iter().any(|(k, _)| k == "src") {
        return None;
    }
    let job = match (job, affected) {
        (Some(job), false) => job,
        (None, true) => String::new(),
        _ => return None,
    };
    Some(Request {
        job,
        params,
        push,
        affected,
    })
}

/// What the box said of itself: the first line of `box`.
#[derive(Debug, PartialEq, Eq)]
pub enum BoxState {
    Ok,
    Degraded(String),
    Maintenance(String),
}

pub fn box_state(first_line: &str) -> BoxState {
    let line = first_line.trim();
    if line == "ok" {
        BoxState::Ok
    } else if let Some(reason) = line.strip_prefix("maintenance:") {
        BoxState::Maintenance(reason.trim().to_string())
    } else {
        let reason = line.strip_prefix("degraded:").unwrap_or(line);
        BoxState::Degraded(reason.trim().to_string())
    }
}

/// What an ssh command's exit code means.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The command ended with this code.
    Exit(u8),
    /// The box could not serve the job (exit 75).
    Unavailable,
    /// ssh itself failed (exit 255, or killed): the box is off, the
    /// network is down, or the connection dropped.
    Lost,
}

pub fn classify(code: Option<i32>) -> Outcome {
    match code {
        Some(75) => Outcome::Unavailable,
        Some(255) | None => Outcome::Lost,
        Some(c) => Outcome::Exit(u8::try_from(c).unwrap_or(1)),
    }
}

/// The id in `bench-job: job <id> (job build branch=main) queued`.
pub fn queued_id(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("bench-job: job ")?;
    let id = rest.split_whitespace().next()?;
    valid_id(id).then_some(id)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The job's exit code in its last line, `bench-job: <id> exit <n>`.
pub fn final_exit(line: &str, id: &str) -> Option<i32> {
    line.strip_prefix("bench-job: ")?
        .strip_prefix(id)?
        .trim()
        .strip_prefix("exit ")?
        .trim()
        .parse()
        .ok()
}

/// The reason a job could not be served, from the lines the box printed:
/// its `unavailable:` message, else its last `bench:` line.
pub fn unavailable_reason(lines: &[String]) -> String {
    let unavailable = lines
        .iter()
        .rev()
        .find_map(|l| l.strip_prefix("bench-job: unavailable: "));
    let last = || {
        lines
            .iter()
            .rev()
            .find_map(|l| l.strip_prefix("bench: "))
            .filter(|l| !l.starts_with("cost") && !l.starts_with("memory"))
    };
    unavailable
        .or_else(last)
        .map_or_else(|| "no reason given".into(), |r| r.trim().to_string())
}

/// `user@host` from what the configuration gives.
pub fn destination(host: &str) -> String {
    if host.contains('@') {
        host.to_string()
    } else {
        format!("{DEFAULT_USER}@{host}")
    }
}

/// `KEY=value` lines; blank lines and `#` comments skipped, quotes removed.
pub fn read_env(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim();
        let l = l.strip_prefix("export ").unwrap_or(l);
        let (k, v) = l.split_once('=')?;
        (k.trim() == key && !l.starts_with('#'))
            .then(|| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
            .filter(|v| !v.is_empty())
    })
}

struct Bench {
    destination: String,
    key: Option<PathBuf>,
}

impl Bench {
    fn configure(root: &Path) -> Result<Bench, String> {
        let local = std::fs::read_to_string(root.join("tools/bench/local.env")).unwrap_or_default();
        let get = |key: &str| {
            std::env::var(key)
                .ok()
                .filter(|v| !v.is_empty())
                .or_else(|| read_env(&local, key))
        };
        let host = get("HELIOS_BENCH_HOST")
            .ok_or("no bench box: set HELIOS_BENCH_HOST (or write it in tools/bench/local.env)")?;
        let key = get("HELIOS_BENCH_KEY").map(PathBuf::from).or_else(|| {
            let home = std::env::var_os("HOME")?;
            let key = Path::new(&home).join(".ssh").join(DEFAULT_USER);
            key.exists().then_some(key)
        });
        Ok(Bench {
            destination: destination(&host),
            key,
        })
    }

    fn ssh(&self, connect_timeout: u32, args: &[&str]) -> Command {
        let mut c = Command::new("ssh");
        if let Some(key) = &self.key {
            c.arg("-i").arg(key);
        }
        c.args(["-o", "BatchMode=yes", "-o"])
            .arg(format!("ConnectTimeout={connect_timeout}"))
            .args([
                "-o",
                "ServerAliveInterval=30",
                "-o",
                "ServerAliveCountMax=4",
            ])
            .arg(&self.destination)
            .args(args);
        c
    }
}

fn unavailable(reason: &str) -> u8 {
    eprintln!("bench box unavailable: {reason}");
    EX_TEMPFAIL
}

/// Runs a box command, its output streamed to ours and kept.
fn follow(cmd: &mut Command) -> (Option<i32>, Vec<String>) {
    let mut child = match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return (Some(255), vec![format!("cannot run ssh: {e}")]),
    };
    let lines = Arc::new(Mutex::new(Vec::new()));
    let stderr = child.stderr.take().map(|err| {
        let lines = Arc::clone(&lines);
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                eprintln!("{line}");
                lines.lock().unwrap().push(line);
            }
        })
    });
    if let Some(out) = child.stdout.take() {
        let mut stdout = std::io::stdout();
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let _ = writeln!(stdout, "{line}");
            lines.lock().unwrap().push(line);
        }
    }
    let code = child.wait().ok().and_then(|s| s.code());
    if let Some(t) = stderr {
        let _ = t.join();
    }
    let lines = std::mem::take(&mut *lines.lock().unwrap());
    (code, lines)
}

/// Asks `box`; `Err` carries the reason it cannot serve.
fn check_box(b: &Bench) -> Result<(), String> {
    let out = b
        .ssh(5, &["box"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run ssh: {e}"))?;
    if classify(out.status.code()) == Outcome::Lost {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "unreachable ({}): {}",
            b.destination,
            why.lines().last().unwrap_or("ssh failed").trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    match box_state(text.lines().next().unwrap_or("")) {
        BoxState::Ok => Ok(()),
        BoxState::Maintenance(why) => Err(format!("the box is in maintenance: {why}")),
        // the box refuses a job that needs what is missing, with exit 75
        BoxState::Degraded(why) => {
            eprintln!("bench-remote: the box is degraded ({why}); trying anyway");
            Ok(())
        }
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The branch to run on when none is given: the current one, else main
/// when pushing (the checkout only needs a branch GitHub has).
fn default_branch(root: &Path, push: bool) -> Option<String> {
    let current = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| b != "HEAD");
    if !push {
        return current;
    }
    let on_github = |b: &String| {
        git(
            root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/remotes/origin/{b}"),
            ],
        )
        .is_some()
    };
    Some(current.filter(on_github).unwrap_or_else(|| "main".into()))
}

/// Warns when the box will check out the current branch but not what is
/// here: unpushed commits or uncommitted changes. Another branch, named
/// on purpose, runs as GitHub has it without a word.
fn warn_unpushed(root: &Path, branch: &str) {
    if git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).as_deref() != Some(branch) {
        return;
    }
    let local = git(root, &["rev-parse", "HEAD"]);
    let remote = git(
        root,
        &["rev-parse", &format!("refs/remotes/origin/{branch}")],
    );
    if local != remote || git(root, &["status", "--porcelain"]).is_some_and(|s| !s.is_empty()) {
        eprintln!(
            "bench-remote: the box runs origin/{branch} as GitHub has it, not this checkout \
             (push the branch first, or use --push)"
        );
    }
}

/// Sends the working tree, or an empty tar to remove it.
fn push(b: &Bench, root: &Path, empty: bool) -> Result<(), String> {
    let mut files = Vec::new();
    if !empty {
        let list = git(root, &["ls-files", "-z", "-co", "--exclude-standard"])
            .ok_or("git ls-files failed")?;
        files.extend(
            list.split('\0')
                .filter(|p| !p.is_empty() && root.join(p).is_file())
                .map(str::to_string),
        );
    }
    let mut tar = Command::new("tar")
        .args(["--null", "-T", "-", "-cf", "-"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run tar: {e}"))?;
    let mut stdin = tar.stdin.take().expect("piped");
    let writer = std::thread::spawn(move || {
        for f in files {
            let _ = stdin.write_all(f.as_bytes());
            let _ = stdin.write_all(b"\0");
        }
    });
    let out = b
        .ssh(10, &["push", &pushed_name(root)])
        .stdin(tar.stdout.take().expect("piped"))
        .output()
        .map_err(|e| format!("cannot run ssh: {e}"))?;
    let _ = writer.join();
    let _ = tar.wait();
    match classify(out.status.code()) {
        Outcome::Exit(0) => {
            print!("{}", String::from_utf8_lossy(&out.stdout));
            Ok(())
        }
        Outcome::Lost | Outcome::Unavailable => {
            Err(format!("push failed: unreachable ({})", b.destination))
        }
        Outcome::Exit(c) => Err(format!(
            "push failed (exit {c}): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

/// Fetches a job's results into `target/bench-remote/<id>/`.
fn fetch(b: &Bench, root: &Path, id: &str) -> Result<PathBuf, String> {
    let dir = target_dir(root).join("bench-remote").join(id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut ssh = b
        .ssh(10, &["fetch", id])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run ssh: {e}"))?;
    let extracted = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(&dir)
        .stdin(ssh.stdout.take().expect("piped"))
        .status();
    let fetched = ssh.wait().ok().and_then(|s| s.code());
    if fetched != Some(0) || !extracted.is_ok_and(|s| s.success()) {
        return Err(format!("could not fetch the results of {id}"));
    }
    Ok(dir)
}

fn summary(req: &Request, id: &str, code: u8, took: Duration, dir: Option<&Path>) {
    let params: Vec<String> = req.params.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let secs = took.as_secs();
    println!(
        "\nbench-remote: {} {} (job {id}): {} in {}m{:02}s",
        req.job,
        params.join(" "),
        if code == 0 {
            "ok".into()
        } else {
            format!("exit {code}")
        },
        secs / 60,
        secs % 60
    );
    let Some(dir) = dir else { return };
    println!("bench-remote: results in {}", dir.display());
    if let Some(warning) = std::fs::read_to_string(dir.join("conditions.json"))
        .ok()
        .and_then(|text| conditions_warning(&text))
    {
        println!("bench-remote: {warning}");
    }
    let mut files: Vec<String> = std::fs::read_dir(dir.join("out"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    if !files.is_empty() {
        println!("bench-remote: out/ has {}", files.join(", "));
    }
}

/// The warning to print when the box says a run's conditions were not
/// comparable (throttled, another tenant, ...): its timings must not be
/// compared with other runs. `None` for a comparable run or an unreadable file.
fn conditions_warning(json: &str) -> Option<String> {
    let c: serde_json::Value = serde_json::from_str(json).ok()?;
    if c.get("comparable")?.as_bool()? {
        return None;
    }
    let mut why: Vec<String> = c
        .get("flags")
        .and_then(|f| f.as_array())
        .map(|f| {
            f.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if let Some(n) = c.get("throttle_events").and_then(serde_json::Value::as_u64) {
        why.push(format!("{n} throttle events"));
    }
    if let Some(t) = c
        .pointer("/cpu_temp/max")
        .and_then(serde_json::Value::as_f64)
    {
        why.push(format!("cpu {t:.0} °C max"));
    }
    Some(format!(
        "NOT comparable ({}): do not compare this run's timings",
        if why.is_empty() {
            "the box says so".into()
        } else {
            why.join(", ")
        }
    ))
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let mut req = parse_args(args)?;
    let result = if req.affected {
        affected(root, &req)
    } else {
        remote(root, &mut req)
    };
    Some(match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => crate::fail(&[e]),
    })
}

/// The requests `--affected` makes of the jobs a selection names: the
/// request's parameters for each, `base=` for `test-affected` alone.
pub fn affected_requests(req: &Request, jobs: &[String]) -> Vec<Request> {
    jobs.iter()
        .map(|job| Request {
            job: job.clone(),
            params: req
                .params
                .iter()
                .filter(|(k, _)| k != "base" || job == "test-affected")
                .cloned()
                .collect(),
            push: req.push,
            affected: false,
        })
        .collect()
}

/// The exit code of a series of jobs: the first failure's, else 75 when
/// one could not be served, else 0.
pub fn combined(codes: &[u8]) -> u8 {
    codes
        .iter()
        .copied()
        .find(|c| *c != 0 && *c != EX_TEMPFAIL)
        .or_else(|| codes.iter().copied().find(|c| *c == EX_TEMPFAIL))
        .unwrap_or(0)
}

fn affected(root: &Path, req: &Request) -> Result<u8, String> {
    let base = req
        .param("base")
        .unwrap_or(crate::affected::DEFAULT_BASE)
        .to_string();
    let (selection, files) = crate::affected::compute(root, &base)?;
    eprint!("{}", selection.summary(&base, files));
    let mut req = req.clone();
    if req.param("base").is_none() {
        req.params.push(("base".into(), base));
    }
    let mut results = Vec::new();
    for mut job in affected_requests(&req, &selection.box_jobs()) {
        println!("\nbench-remote: --affected: {}", job.job);
        let code = remote(root, &mut job)?;
        results.push((job.job, code));
    }
    println!("\nbench-remote: --affected results");
    for (job, code) in &results {
        let result = match *code {
            0 => "ok".to_string(),
            EX_TEMPFAIL => "unavailable (75)".to_string(),
            c => format!("exit {c}"),
        };
        println!("  {job:<14} {result}");
    }
    let codes: Vec<u8> = results.iter().map(|(_, c)| *c).collect();
    Ok(combined(&codes))
}

fn remote(root: &Path, req: &mut Request) -> Result<u8, String> {
    let b = Bench::configure(root)?;
    if let Err(why) = check_box(&b) {
        return Ok(unavailable(&why));
    }
    if req.param("branch").is_none() {
        let branch = default_branch(root, req.push)
            .ok_or("no current branch (detached HEAD): give branch=<branch>")?;
        req.params.insert(0, ("branch".into(), branch));
    }
    let branch = req.param("branch").unwrap_or_default().to_string();
    if req.push {
        if let Err(e) = push(&b, root, false) {
            return Ok(unavailable(&e));
        }
        req.params.push(("src".into(), pushed_name(root)));
    } else {
        warn_unpushed(root, &branch);
    }

    let started = Instant::now();
    let mut words: Vec<String> = vec!["run".into(), req.job.clone()];
    words.extend(req.params.iter().map(|(k, v)| format!("{k}={v}")));
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let (mut code, mut lines) = follow(b.ssh(10, &words).stdin(Stdio::null()));
    let Some(id) = lines.iter().find_map(|l| queued_id(l)).map(str::to_string) else {
        return Ok(match classify(code) {
            Outcome::Unavailable => unavailable(&unavailable_reason(&lines)),
            Outcome::Lost => unavailable(&format!("unreachable ({})", b.destination)),
            // refused (above the envelope, unknown job...): the box said why
            Outcome::Exit(c) => c.max(1),
        });
    };
    let mut attempts = 0;
    while classify(code) == Outcome::Lost && attempts < REATTACH {
        attempts += 1;
        eprintln!(
            "bench-remote: lost the connection to job {id}; reattaching ({attempts}/{REATTACH})"
        );
        std::thread::sleep(REATTACH_PAUSE);
        let (c, more) = follow(b.ssh(10, &["wait", &id]).stdin(Stdio::null()));
        code = c;
        lines.extend(more);
    }
    if classify(code) == Outcome::Lost {
        return Ok(unavailable(&format!(
            "lost the connection during job {id}; it may still run: reattach later with `wait {id}`"
        )));
    }
    let took = started.elapsed();
    let exit = lines
        .iter()
        .rev()
        .find_map(|l| final_exit(l, &id))
        .map_or_else(|| classify(code), |c| classify(Some(c)));
    if req.push {
        let _ = push(&b, root, true);
    }
    let dir = match fetch(&b, root, &id) {
        Ok(dir) => Some(dir),
        Err(e) => {
            eprintln!("bench-remote: {e}");
            None
        }
    };
    Ok(match exit {
        Outcome::Unavailable => {
            summary(req, &id, EX_TEMPFAIL, took, dir.as_deref());
            unavailable(&unavailable_reason(&lines))
        }
        Outcome::Exit(c) => {
            summary(req, &id, c, took, dir.as_deref());
            c
        }
        Outcome::Lost => unreachable!("handled above"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_on_a_throttled_box_is_flagged() {
        assert_eq!(conditions_warning(r#"{"comparable": true}"#), None);
        assert_eq!(conditions_warning("not json"), None);
        let w = conditions_warning(
            r#"{"comparable": false, "flags": ["throttled"], "throttle_events": 737,
                "cpu_temp": {"max": 96.0}}"#,
        )
        .expect("a warning");
        assert!(
            w.contains("throttled") && w.contains("737") && w.contains("96 °C"),
            "{w}"
        );
    }

    #[test]
    fn each_checkout_pushes_under_its_own_name() {
        let a = pushed_name(Path::new("/w/helios"));
        assert_eq!(a, pushed_name(Path::new("/w/helios")), "stable");
        assert_ne!(a, pushed_name(Path::new("/w/helios/.claude/worktrees/x")));
        assert!(a.starts_with("helios-local-") && valid_value(&a));
    }

    fn req(job: &str, params: &[(&str, &str)], push: bool) -> Request {
        Request {
            job: job.into(),
            params: params
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
            push,
            affected: false,
        }
    }

    #[test]
    fn affected_takes_no_job_and_passes_the_base_to_its_test_job() {
        let r = parse_args(&["--affected", "base=main", "--push"]).expect("parses");
        assert!(r.affected && r.push && r.job.is_empty());
        assert_eq!(parse_args(&["--affected", "test"]), None, "no job with it");
        let jobs: Vec<String> = ["lint", "test-affected", "raw-corpus"]
            .iter()
            .map(|j| (*j).to_string())
            .collect();
        let reqs = affected_requests(&req("", &[("branch", "x"), ("base", "main")], false), &jobs);
        assert_eq!(
            reqs,
            vec![
                req("lint", &[("branch", "x")], false),
                req("test-affected", &[("branch", "x"), ("base", "main")], false),
                req("raw-corpus", &[("branch", "x")], false),
            ]
        );
    }

    #[test]
    fn a_series_exits_with_its_first_failure() {
        assert_eq!(combined(&[0, 0]), 0);
        assert_eq!(combined(&[0, 75, 0]), 75);
        assert_eq!(combined(&[75, 101, 1]), 101);
        assert_eq!(combined(&[]), 0);
    }

    #[test]
    fn arguments_parse() {
        assert_eq!(parse_args(&["test"]), Some(req("test", &[], false)));
        assert_eq!(
            parse_args(&["build", "branch=feature/x-1", "--push"]),
            Some(req("build", &[("branch", "feature/x-1")], true))
        );
        assert_eq!(
            parse_args(&["--push", "check-commits", "base=abc123"]),
            Some(req("check-commits", &[("base", "abc123")], true))
        );
        assert_eq!(parse_args(&[]), None, "a job is needed");
        assert_eq!(parse_args(&["--push"]), None);
        assert_eq!(parse_args(&["test", "build"]), None, "one job");
        assert_eq!(parse_args(&["test", "--fast"]), None, "unknown flag");
        assert_eq!(parse_args(&["test", "branch="]), None, "empty value");
        assert_eq!(parse_args(&["test", "branch=a b"]), None);
        assert_eq!(
            parse_args(&["test", "branch=a;rm"]),
            None,
            "shell characters"
        );
        assert_eq!(parse_args(&["test", "Branch=x"]), None, "lowercase keys");
        assert_eq!(parse_args(&["test", "branch=a", "branch=b"]), None, "twice");
        assert_eq!(
            parse_args(&["test", "src=x", "--push"]),
            None,
            "--push sets src"
        );
        assert_eq!(
            parse_args(&["test", "src=x"]),
            Some(req("test", &[("src", "x")], false))
        );
    }

    #[test]
    fn exit_codes_map() {
        assert_eq!(classify(Some(0)), Outcome::Exit(0));
        assert_eq!(classify(Some(1)), Outcome::Exit(1));
        assert_eq!(classify(Some(2)), Outcome::Exit(2));
        assert_eq!(classify(Some(75)), Outcome::Unavailable);
        assert_eq!(classify(Some(255)), Outcome::Lost);
        assert_eq!(classify(None), Outcome::Lost, "ssh killed by a signal");
        assert_eq!(classify(Some(300)), Outcome::Exit(1));
    }

    #[test]
    fn the_box_states_parse() {
        assert_eq!(box_state("ok\n"), BoxState::Ok);
        assert_eq!(
            box_state("degraded: gpu absent"),
            BoxState::Degraded("gpu absent".into())
        );
        assert_eq!(
            box_state("maintenance: new disk"),
            BoxState::Maintenance("new disk".into())
        );
        assert_eq!(box_state(""), BoxState::Degraded(String::new()));
    }

    #[test]
    fn job_lines_parse() {
        let id = "20261012-133225-3878";
        assert_eq!(
            queued_id("bench-job: job 20261012-133225-3878 (job build branch=main) queued"),
            Some(id)
        );
        assert_eq!(queued_id("bench: building image"), None);
        assert_eq!(final_exit(&format!("bench-job: {id} exit 0"), id), Some(0));
        assert_eq!(
            final_exit(&format!("bench-job: {id} exit 101"), id),
            Some(101)
        );
        assert_eq!(
            final_exit(&format!("bench-job: {id} exit 75"), id),
            Some(75)
        );
        assert_eq!(final_exit("bench-job: other-id exit 0", id), None);
        assert_eq!(final_exit(&format!("bench-job: {id} running"), id), None);
    }

    #[test]
    fn unavailable_reasons_come_from_the_box() {
        let lines = |v: &[&str]| v.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert_eq!(
            unavailable_reason(&lines(&[
                "bench-job: unavailable: the box is in maintenance: new disk"
            ])),
            "the box is in maintenance: new disk"
        );
        assert_eq!(
            unavailable_reason(&lines(&[
                "bench: started",
                "bench: gpu lost during the run"
            ])),
            "gpu lost during the run"
        );
        assert_eq!(unavailable_reason(&[]), "no reason given");
    }

    #[test]
    fn the_host_comes_from_the_configuration() {
        assert_eq!(destination("box.example"), "bench-helios@box.example");
        assert_eq!(destination("me@box.example"), "me@box.example");
        let env =
            "# the bench box\nexport HELIOS_BENCH_HOST=\"box.example\"\n\nHELIOS_BENCH_KEY=/k\n";
        assert_eq!(
            read_env(env, "HELIOS_BENCH_HOST").as_deref(),
            Some("box.example")
        );
        assert_eq!(read_env(env, "HELIOS_BENCH_KEY").as_deref(), Some("/k"));
        assert_eq!(read_env("#HELIOS_BENCH_HOST=x", "HELIOS_BENCH_HOST"), None);
        assert_eq!(read_env("HELIOS_BENCH_HOST=", "HELIOS_BENCH_HOST"), None);
    }
}
