//! `cargo xtask bench masks [ARGS...]`: the masks bench.
//!
//! Builds the `stress_bench` example of `helios-module-masks` in release
//! mode, runs it once per mode (full, tiled, proxy, edit), each in its own
//! process so its peak memory is its own, prints the JSON line each run
//! writes, and appends them, with the date and the git commit, to
//! `target/bench/masks.jsonl`. Extra arguments go to every run
//! (`--modules N`, `--width W`, `--height H`, `--tile T`).

use std::path::Path;
use std::process::{Command, ExitCode};

use serde_json::Value;

use crate::test_times::{commit, now, target_dir};

pub const USAGE: &str = "bench masks [--modules N] [--width W] [--height H] [--tile T]";

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
    let mut lines = Vec::new();
    for mode in MODES {
        let output = match Command::new(&binary).arg(mode).args(rest).output() {
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
