//! `cargo xtask bench masks [ARGS...]`: the masks bench.
//!
//! Builds the `stress_bench` example of `helios-module-masks` in release
//! mode, runs it once per mode (full, tiled, proxy, edit), each in its own
//! process so its peak memory is its own, prints the JSON line each run
//! writes, and appends them, with the date and the git commit, to
//! `target/bench/masks.jsonl`. Extra arguments go to every run
//! (`--modules N`, `--width W`, `--height H`, `--tile T`, `--budget MIB`).
//! Without `--shapes`, every mode runs twice: the network as it is, then
//! with a group of drawn shapes in every module's mask (`--shapes 1`);
//! `--shapes 0` or `--shapes 1` runs one of them.

use std::path::Path;
use std::process::{Command, ExitCode};

use serde_json::Value;

use crate::test_times::{commit, now, target_dir};

pub const USAGE: &str =
    "bench masks [--modules N] [--width W] [--height H] [--tile T] [--budget MIB] [--shapes 0|1]";

const MODES: [&str; 4] = ["full", "tiled", "proxy", "edit"];

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let ["masks", rest @ ..] = args else {
        return None;
    };
    if rest.len() % 2 != 0 {
        return None;
    }
    let built = Command::new("cargo")
        .args(["build", "--quiet", "--release", "--package"])
        .args(["helios-module-masks", "--example", "stress_bench"])
        .current_dir(root)
        .status();
    if !built.is_ok_and(|s| s.success()) {
        eprintln!("xtask: the masks bench did not build");
        return Some(ExitCode::FAILURE);
    }
    let binary = target_dir(root)
        .join("release")
        .join("examples")
        .join("stress_bench");
    let (date, commit) = (now(), commit(root));
    let variants: &[&[&str]] = if rest.contains(&"--shapes") {
        &[&[]]
    } else {
        &[&["--shapes", "0"], &["--shapes", "1"]]
    };
    let runs = variants
        .iter()
        .flat_map(|extra| MODES.iter().map(move |mode| (*mode, *extra)));
    let mut lines = Vec::new();
    for (mode, extra) in runs {
        let output = match Command::new(&binary)
            .arg(mode)
            .args(rest)
            .args(extra)
            .output()
        {
            Ok(o) if o.status.success() => o,
            Ok(o) => {
                eprintln!(
                    "xtask: bench mode {mode} failed: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                return Some(ExitCode::FAILURE);
            }
            Err(e) => {
                eprintln!("xtask: {}: {e}", binary.display());
                return Some(ExitCode::FAILURE);
            }
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let Ok(Value::Object(mut line)) = serde_json::from_str::<Value>(text.trim()) else {
            eprintln!("xtask: bench mode {mode} printed no JSON line: {text}");
            return Some(ExitCode::FAILURE);
        };
        println!("{}", text.trim());
        line.insert("date".into(), date.clone().into());
        line.insert("commit".into(), commit.clone().into());
        lines.push(Value::Object(line).to_string());
    }
    let dir = target_dir(root).join("bench");
    let file = dir.join("masks.jsonl");
    let written = std::fs::create_dir_all(&dir).and_then(|()| {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)?;
        for line in &lines {
            writeln!(f, "{line}")?;
        }
        Ok(())
    });
    if let Err(e) = written {
        eprintln!("{}: {e}", file.display());
        return Some(ExitCode::FAILURE);
    }
    eprintln!("Recorded in {}.", file.display());
    Some(ExitCode::SUCCESS)
}

/// Cargo's arguments and the example's, from xtask's: `--dev` drops
/// `--release`, a bare `--budgets` gets the committed file.
fn diffstat_args(args: &[&str]) -> (Vec<&'static str>, Vec<String>) {
    let mut cargo = vec!["run", "--quiet", "--release", "--locked", "--package"];
    let mut example = Vec::new();
    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        match *arg {
            "--dev" => cargo.retain(|a| *a != "--release"),
            "--budgets" => {
                example.push("--budgets".to_string());
                match it.peek() {
                    Some(path)
                        if Path::new(path)
                            .extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("toml")) =>
                    {
                        example.push((*path).to_string());
                        it.next();
                    }
                    _ => example.push(DIFFSTAT_BUDGETS.to_string()),
                }
            }
            a => example.push(a.to_string()),
        }
    }
    (cargo, example)
}

pub const DIFFSTAT_USAGE: &str = "engine-diffstat <fixture|all> [--module NAME] [--dump-diff DIR] \
     [--threshold T] [--tile N] [--json FILE] [--budgets [FILE]] [--cpu] [--dev]";

/// The budgets file `--budgets` reads when it is given no path.
const DIFFSTAT_BUDGETS: &str = "crates/helios-gpu/diffstat-budgets.toml";

/// `cargo xtask engine-diffstat ARGS...`: the bench fixtures' pipelines
/// run on the CPU and on the GPU, per module how far the GPU is from the
/// CPU (`helios-gpu`'s `diffstat` example, after darktable's
/// `--opencl-diffstat`). A release build: run it on the box (the `diffstat`
/// job), where the GPU is. `--dev` builds it in the dev profile instead,
/// which optimises the engine and the modules already (the workspace's
/// `[profile.dev.package]`) and reuses what a test run built: CI's gate.
/// `--budgets` with no path reads the committed budgets file.
pub fn diffstat(root: &Path, args: &[&str]) -> ExitCode {
    let (cargo, example) = diffstat_args(args);
    let status = Command::new("cargo")
        .args(cargo)
        .args(["helios-gpu", "--example", "diffstat", "--"])
        .args(example)
        .current_dir(root)
        .status();
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(s) => ExitCode::from(u8::try_from(s.code().unwrap_or(1)).unwrap_or(1)),
        Err(e) => {
            eprintln!("xtask: cargo: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffstat_dev_drops_release_and_a_bare_budgets_reads_the_committed_file() {
        let (cargo, example) = diffstat_args(&["all", "--budgets", "--dev"]);
        assert!(!cargo.contains(&"--release"));
        assert_eq!(example, ["all", "--budgets", DIFFSTAT_BUDGETS]);
        let (cargo, example) = diffstat_args(&["tone", "--budgets", "b.toml"]);
        assert!(cargo.contains(&"--release"));
        assert_eq!(example, ["tone", "--budgets", "b.toml"]);
    }
}
