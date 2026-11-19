//! The delta mode of the guards with a backlog (`raw-egui --check --base
//! REF`, `bus --check --base REF`, `lint --base REF`): the guard runs on
//! the merge base and on the head, and only what the change adds is a
//! finding; a count that went down is printed as progress ("raw egui
//! calls: 300 → 297 (−3)"). An old finding no longer blocks an unrelated
//! pull request, and its author sees what they added, not the backlog.
//!
//! The idea comes from darktable's `dt-lockcheck` diff mode, which runs
//! its checker on the base and on the head of a pull request and reports
//! only the new findings
//! (<https://github.com/darktable-org/darktable/pull/23056>).
//!
//! A finding's identity survives line shifts: it is its file and a
//! normalised snippet (or message), never its line number, so code moved
//! within a file is not a new finding.
//!
//! The base is checked out of git (`git archive`) into a scratch folder
//! under `target/`, only the paths the guard reads; the working tree and
//! the index are not touched.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The ref a delta is taken against when none is given.
pub const DEFAULT_BASE: &str = "origin/next";

/// Reads `--base [REF]` (or `--base=REF`) out of `args`: the remaining
/// arguments, and the ref if `--base` was given ([`DEFAULT_BASE`] when it
/// has no value). `None` when `--base=` is empty.
pub fn take_base<'a>(args: &[&'a str]) -> Option<(Vec<&'a str>, Option<&'a str>)> {
    let mut rest = Vec::new();
    let mut base = None;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if *a == "--base" {
            base = Some(match it.peek() {
                Some(r) if !r.starts_with("--") => *it.next()?,
                _ => DEFAULT_BASE,
            });
        } else if let Some(r) = a.strip_prefix("--base=") {
            if r.is_empty() {
                return None;
            }
            base = Some(r);
        } else {
            rest.push(*a);
        }
    }
    Some((rest, base))
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

/// The merge base of `HEAD` with `base`: what the change is compared to.
pub fn merge_base(root: &Path, base: &str) -> Result<String, String> {
    git(root, &["merge-base", "HEAD", base])
}

/// The tree of `rev`, limited to `paths` (those it has), written to a
/// scratch folder. Dropped, the folder is removed.
pub struct BaseTree {
    pub dir: PathBuf,
}

impl Drop for BaseTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl BaseTree {
    pub fn extract(root: &Path, rev: &str, paths: &[&str]) -> Result<BaseTree, String> {
        let present: Vec<&str> = paths
            .iter()
            .copied()
            .filter(|p| git(root, &["cat-file", "-e", &format!("{rev}:{p}")]).is_ok())
            .collect();
        let short: String = rev.chars().take(12).collect();
        let dir = root
            .join("target")
            .join("xtask-delta")
            .join(format!("{short}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let tree = BaseTree { dir };
        if present.is_empty() {
            return Ok(tree);
        }
        let mut archive = Command::new("git")
            .args(["archive", "--format=tar", rev, "--"])
            .args(&present)
            .current_dir(root)
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run git archive: {e}"))?;
        let stdout = archive.stdout.take().ok_or("git archive: no output")?;
        let tar = Command::new("tar")
            .arg("-x")
            .arg("-C")
            .arg(&tree.dir)
            .stdin(stdout)
            .status()
            .map_err(|e| format!("cannot run tar: {e}"))?;
        let archived = archive.wait().map_err(|e| e.to_string())?;
        if !archived.success() || !tar.success() {
            return Err(format!("cannot extract {rev} into {}", tree.dir.display()));
        }
        Ok(tree)
    }
}

/// A finding's message without its line number: `path:12: text` becomes
/// `path: text`, so the same finding on a shifted line is the same.
pub fn without_line(message: &str) -> String {
    let mut parts = message.splitn(3, ':');
    if let (Some(path), Some(line), Some(rest)) = (parts.next(), parts.next(), parts.next())
        && !line.is_empty()
        && line.chars().all(|c| c.is_ascii_digit())
    {
        return format!("{path}:{rest}");
    }
    message.to_string()
}

/// The findings of `head` that `base` does not have, counted as a
/// multiset by their identity `key`: a finding present twice in the head
/// and once in the base is new once.
pub fn added<T: Clone>(base: &[T], head: &[T], key: impl Fn(&T) -> String) -> Vec<T> {
    let mut left: BTreeMap<String, usize> = BTreeMap::new();
    for b in base {
        *left.entry(key(b)).or_default() += 1;
    }
    let mut out = Vec::new();
    for h in head {
        match left.get_mut(&key(h)) {
            Some(n) if *n > 0 => *n -= 1,
            _ => out.push(h.clone()),
        }
    }
    out
}

/// `label: before → after (±n)`, or `label: n (unchanged)`.
pub fn progress(label: &str, before: usize, after: usize) -> String {
    if before == after {
        format!("{label}: {after} (unchanged)")
    } else if after < before {
        format!("{label}: {before} → {after} (−{})", before - after)
    } else {
        format!("{label}: {before} → {after} (+{})", after - before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_base_is_read_out_of_the_arguments() {
        assert_eq!(
            take_base(&["--check", "--base", "origin/next"]),
            Some((vec!["--check"], Some("origin/next")))
        );
        assert_eq!(
            take_base(&["--base=abc", "--check"]),
            Some((vec!["--check"], Some("abc")))
        );
        assert_eq!(take_base(&["--check"]), Some((vec!["--check"], None)));
        assert_eq!(
            take_base(&["--check", "--base"]),
            Some((vec!["--check"], Some(DEFAULT_BASE)))
        );
        assert_eq!(
            take_base(&["--base", "--check"]),
            Some((vec!["--check"], Some(DEFAULT_BASE)))
        );
        assert_eq!(take_base(&["--base="]), None);
    }

    #[test]
    fn delta_line_numbers_are_not_part_of_a_finding() {
        assert_eq!(
            without_line("crates/a/src/x.rs:12: foo.subscribe()"),
            "crates/a/src/x.rs: foo.subscribe()"
        );
        assert_eq!(
            without_line("xtask/allow/bus.toml: stale"),
            "xtask/allow/bus.toml: stale"
        );
    }

    #[test]
    fn delta_added_counts_as_a_multiset() {
        let base = ["a", "b", "b"];
        let head = ["b", "a", "b", "b", "c"];
        assert_eq!(added(&base, &head, |s| (*s).to_string()), ["b", "c"]);
        assert!(added(&head, &base, |s| (*s).to_string()).is_empty());
    }

    #[test]
    fn delta_progress_says_which_way() {
        assert_eq!(
            progress("raw egui calls", 300, 297),
            "raw egui calls: 300 → 297 (−3)"
        );
        assert_eq!(progress("x", 2, 3), "x: 2 → 3 (+1)");
        assert_eq!(progress("x", 2, 2), "x: 2 (unchanged)");
    }
}
