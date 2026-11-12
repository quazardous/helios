//! `cargo xtask promote <SHA> [--remote NAME]`: promote a commit CI passed
//! on `next` to `main`, the release of its changelog fragments included
//! (`.github/workflows/promote.yml` runs it; see docs/releases.md).
//!
//! It works on the exact commit CI validated, never on whatever `next`
//! points to when it runs:
//!
//! 1. The base is the validated commit itself or, when `main` already holds
//!    a release commit it lacks (made while `next` had moved on), the merge
//!    of that release into `next` whose first parent is the validated
//!    commit. Without such a merge, nothing is promoted: the next green run
//!    on `next` does it.
//! 2. With user-facing fragments in the base, the release is written as
//!    `cargo xtask release` writes it, Cargo.lock is refreshed
//!    (`cargo update --workspace`) and the result is committed as
//!    "Release X.Y.Z", the base its parent. The survival scenarios are not
//!    run again: CI ran them on the validated commit, and the release
//!    changes no code.
//! 3. The commit to promote must differ from the validated one by a version
//!    bump only ([`bump_problems`]): Cargo.toml's workspace version, the
//!    in-tree plugins' Helios range, CHANGELOG.md, the `AppStream`
//!    metadata, fragments removed, and in Cargo.lock nothing but the
//!    versions of the workspace's own packages. Anything else (a stale
//!    Cargo.lock that the refresh would rewrite, say) stops the promotion
//!    before any push, so code CI did not run never reaches `main`.
//! 4. `main` fast-forwards to it. `next` does too, if it still points at
//!    the base (a lease, in the same atomic push). If `next` has moved, the
//!    release is merged into it by a merge commit, so nothing pushed is
//!    rewritten and both branches carry the same version.
//!
//! Pushes made with the workflow's token start no workflow: the release
//! commit does not rerun CI, which already ran on the tree it bumps.

use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, ExitCode};

pub const USAGE: &str = "promote <SHA> [--remote NAME]";

/// What a promotion did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// `main` already holds the commit.
    AlreadyOnMain,
    /// Nothing pushed, and why.
    Skipped(String),
    /// `main` moved to `main`; `release` is the version released, if any.
    Promoted {
        main: String,
        release: Option<String>,
        next: Next,
    },
}

/// How `next` followed a promotion.
#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    /// It already held the promoted commit.
    Untouched,
    /// It pointed at the base and fast-forwarded with `main`.
    FastForwarded,
    /// It had moved: the release was merged into it by this commit.
    Merged(String),
    /// It had moved and the merge failed: someone merges `main` into it.
    MergeFailed(String),
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let (sha, remote) = match args {
        [sha] => (*sha, "origin"),
        [sha, "--remote", remote] => (*sha, *remote),
        _ => return None,
    };
    let outcome = promote(root, remote, sha, &crate::today(), &refresh_lock);
    let (text, ok) = match outcome {
        Ok(Outcome::AlreadyOnMain) => (format!("main already holds {sha}"), true),
        Ok(Outcome::Skipped(why)) => (format!("not promoted: {why}"), true),
        Ok(Outcome::Promoted {
            main,
            release,
            next,
        }) => {
            let mut text = format!("main is now {main}");
            if let Some(v) = release {
                let _ = write!(text, ", \"Release {v}\" on top of {sha}");
            }
            match next {
                Next::Untouched => text.push_str("; next already held it"),
                Next::FastForwarded => text.push_str("; next fast-forwarded with it"),
                Next::Merged(m) => {
                    let _ = write!(text, "; next had moved: the release merged into it as {m}");
                }
                Next::MergeFailed(why) => {
                    let _ = write!(
                        text,
                        "; next had moved and the release does not merge into it ({why}): merge main into next by hand"
                    );
                    return Some(report(&text, false));
                }
            }
            (text, true)
        }
        Err(e) => (e, false),
    };
    Some(report(&text, ok))
}

fn report(text: &str, ok: bool) -> ExitCode {
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
            let _ = writeln!(f, "{text}");
        }
    }
    if ok {
        println!("{text}");
        ExitCode::SUCCESS
    } else {
        eprintln!("::error::{text}");
        ExitCode::from(1)
    }
}

/// Cargo.lock follows the new workspace version.
fn refresh_lock(dir: &Path) -> Result<(), String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let ok = Command::new(cargo)
        .args(["update", "--workspace"])
        .current_dir(dir)
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        Ok(())
    } else {
        Err("`cargo update --workspace` failed".into())
    }
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

fn is_ancestor(dir: &Path, a: &str, b: &str) -> bool {
    git(dir, &["merge-base", "--is-ancestor", a, b]).is_ok()
}

/// Promotes the validated commit `sha` of `next` from the checkout `dir`;
/// `refresh` brings Cargo.lock in line with a new version.
pub fn promote(
    dir: &Path,
    remote: &str,
    sha: &str,
    date: &str,
    refresh: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<Outcome, String> {
    let fetch = |dir: &Path| {
        git(
            dir,
            &[
                "fetch",
                "-q",
                remote,
                &format!("+refs/heads/main:refs/remotes/{remote}/main"),
                &format!("+refs/heads/next:refs/remotes/{remote}/next"),
            ],
        )
    };
    fetch(dir)?;
    let main_ref = format!("{remote}/main");
    let next_ref = format!("{remote}/next");
    let sha = git(dir, &["rev-parse", &format!("{sha}^{{commit}}")])?;
    let main = git(dir, &["rev-parse", &main_ref])?;
    let next = git(dir, &["rev-parse", &next_ref])?;
    if is_ancestor(dir, &sha, &main) {
        return Ok(Outcome::AlreadyOnMain);
    }

    // The base: the validated commit, or the merge of main's release into
    // next made on top of it.
    let base = if is_ancestor(dir, &main, &sha) {
        sha.clone()
    } else {
        let merges = git(
            dir,
            &[
                "rev-list",
                "--merges",
                "--parents",
                &format!("{sha}..{next}"),
            ],
        )?;
        let found = merges.lines().find_map(|line| {
            let ids: Vec<&str> = line.split_whitespace().collect();
            (ids.len() == 3 && ids[1] == sha && is_ancestor(dir, &main, ids[0]))
                .then(|| ids[0].to_string())
        });
        match found {
            Some(merge) => merge,
            None => {
                return Ok(Outcome::Skipped(format!(
                    "main holds commits {sha} lacks, and next has no merge of them on top of it; the next green run promotes"
                )));
            }
        }
    };
    git(dir, &["checkout", "-q", "--detach", &base])?;

    // The release of the base's fragments.
    let release = match crate::plan(dir, date) {
        Err(problems) => return Err(problems.join("; ")),
        Ok(None) => None,
        Ok(Some(plan)) => {
            let v = crate::apply_release(dir, date, plan);
            refresh(dir)?;
            git(dir, &["add", "-A"])?;
            git(dir, &["commit", "-q", "-m", &format!("Release {v}")])?;
            Some(v)
        }
    };
    let target = git(dir, &["rev-parse", "HEAD"])?;

    // Only the validated tree and a version bump reach main.
    let problems = bump_problems_between(dir, &sha, &target)?;
    if !problems.is_empty() {
        return Err(format!(
            "the commit to promote is not {sha} plus a version bump, nothing pushed (if Cargo.lock is stale, refresh it in a commit of its own, so CI tests it): {}",
            problems.join("; ")
        ));
    }

    // main and next together, when next still points at the base.
    let together = next == base
        && git(
            dir,
            &[
                "push",
                "-q",
                "--atomic",
                remote,
                &format!("{target}:refs/heads/main"),
                &format!("--force-with-lease=refs/heads/next:{base}"),
                &format!("{target}:refs/heads/next"),
            ],
        )
        .is_ok();
    if together {
        let next = if target == base {
            Next::Untouched
        } else {
            Next::FastForwarded
        };
        return Ok(Outcome::Promoted {
            main: target,
            release,
            next,
        });
    }
    // A fast-forward: refused if main has moved on its own.
    git(
        dir,
        &["push", "-q", remote, &format!("{target}:refs/heads/main")],
    )?;

    // next has moved: it gets the release by a merge, retried when it
    // moves again meanwhile.
    let mut next_followed = Next::Untouched;
    for _ in 0..3 {
        fetch(dir)?;
        if is_ancestor(dir, &target, &next_ref) {
            next_followed = Next::Untouched;
            break;
        }
        git(dir, &["checkout", "-q", "--detach", &next_ref])?;
        let v = release.as_deref().unwrap_or("commits");
        if let Err(e) = git(
            dir,
            &[
                "merge",
                "-q",
                "--no-ff",
                "-m",
                &format!("Merge release {v} into next"),
                &target,
            ],
        ) {
            let _ = git(dir, &["merge", "--abort"]);
            next_followed = Next::MergeFailed(e);
            break;
        }
        let merge = git(dir, &["rev-parse", "HEAD"])?;
        if git(
            dir,
            &["push", "-q", remote, &format!("{merge}:refs/heads/next")],
        )
        .is_ok()
        {
            next_followed = Next::Merged(merge);
            break;
        }
        next_followed = Next::MergeFailed("next kept moving".into());
    }
    Ok(Outcome::Promoted {
        main: target,
        release,
        next: next_followed,
    })
}

/// [`bump_problems`] on the commits `from` and `to` of the checkout `dir`.
fn bump_problems_between(dir: &Path, from: &str, to: &str) -> Result<Vec<String>, String> {
    let diff = git(dir, &["diff", "--name-status", "--no-renames", from, to])?;
    let changes: Vec<(String, String)> = diff
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(s, p)| (s.to_string(), p.to_string()))
        .collect();
    let show = |rev: &str, path: &str| git(dir, &["show", &format!("{rev}:{path}")]).ok();
    Ok(bump_problems(&changes, &|p| show(from, p), &|p| {
        show(to, p)
    }))
}

/// What, in a change (`(status, path)` pairs as `git diff --name-status`
/// gives them, with the files' contents before and after), is more than a
/// version bump. Empty when it is one.
pub fn bump_problems(
    changes: &[(String, String)],
    old: &dyn Fn(&str) -> Option<String>,
    new: &dyn Fn(&str) -> Option<String>,
) -> Vec<String> {
    let new_version = new("Cargo.toml").and_then(|t| crate::read_version(&t).ok());
    let mut problems = Vec::new();
    for (status, path) in changes {
        let both = || old(path).zip(new(path));
        let fine = match (status.as_str(), path.as_str()) {
            ("D", p) => {
                p.starts_with("changelog.d/")
                    && Path::new(p).extension().is_some_and(|e| e == "md")
                    && p != "changelog.d/README.md"
            }
            ("M", "CHANGELOG.md") => true,
            ("M", p) if p == crate::METAINFO => true,
            ("M", "Cargo.toml") => both().is_some_and(|(o, n)| {
                match (crate::read_version(&o), crate::read_version(&n)) {
                    (Ok(ov), Ok(nv)) => crate::write_version(&o, ov, nv) == n,
                    _ => false,
                }
            }),
            ("M", "Cargo.lock") => match both() {
                Some((o, n)) => match lock_problem(&o, &n) {
                    None => true,
                    Some(why) => {
                        problems.push(format!("Cargo.lock: {why}"));
                        continue;
                    }
                },
                None => false,
            },
            ("M", p)
                if p.ends_with("/helios-plugin.toml")
                    && crate::PLUGIN_DIRS.iter().any(|d| p.starts_with(d)) =>
            {
                match (both(), new_version) {
                    (Some((o, n)), Some(v)) => crate::write_plugin_range(&o, v) == n,
                    _ => false,
                }
            }
            _ => false,
        };
        if !fine {
            problems.push(format!("{path} ({status}): not part of a version bump"));
        }
    }
    problems
}

/// Why a Cargo.lock change is more than the workspace's own versions
/// moving, if it is.
fn lock_problem(old: &str, new: &str) -> Option<String> {
    let blocks = |t: &str| -> Vec<String> { t.split("\n\n").map(str::to_string).collect() };
    let (old, new) = (blocks(old), blocks(new));
    if old.len() != new.len() {
        return Some("packages added or removed".into());
    }
    let without_version = |b: &str| -> String {
        b.lines()
            .filter(|l| !l.starts_with("version = "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    for (o, n) in old.iter().zip(&new) {
        if o == n {
            continue;
        }
        let name = o
            .lines()
            .find_map(|l| l.strip_prefix("name = "))
            .unwrap_or("the header")
            .trim_matches('"')
            .to_string();
        // A package of the workspace has no `source`: only its version moves.
        let local = !o.lines().any(|l| l.starts_with("source = "));
        if !local || without_version(o) != without_version(n) {
            return Some(format!("`{name}` changes beyond a workspace version"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const LOCK: &str = "version = 4\n\n[[package]]\nname = \"helios-a\"\nversion = \"0.1.0\"\ndependencies = [\n \"serde\",\n]\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\nsource = \"registry+https://example.org/index\"\n";

    fn ok_refresh(dir: &Path) -> Result<(), String> {
        let v = crate::read_version(&std::fs::read_to_string(dir.join("Cargo.toml")).unwrap())?;
        let lock = std::fs::read_to_string(dir.join("Cargo.lock")).unwrap();
        let mut out = Vec::new();
        let mut in_a = false;
        for line in lock.lines() {
            if line.starts_with("name = ") {
                in_a = line == "name = \"helios-a\"";
            }
            if in_a && line.starts_with("version = ") {
                out.push(format!("version = \"{}.{}.{}\"", v.0, v.1, v.2));
            } else {
                out.push(line.to_string());
            }
        }
        std::fs::write(dir.join("Cargo.lock"), out.join("\n") + "\n").unwrap();
        Ok(())
    }

    fn stale_refresh(dir: &Path) -> Result<(), String> {
        ok_refresh(dir)?;
        let lock = std::fs::read_to_string(dir.join("Cargo.lock")).unwrap();
        std::fs::write(
            dir.join("Cargo.lock"),
            lock.replace("version = \"1.0.0\"", "version = \"1.0.1\""),
        )
        .unwrap();
        Ok(())
    }

    fn sh(dir: &Path, args: &[&str]) -> String {
        git(dir, args).unwrap()
    }

    /// A bare repository with `main` and `next` at a first commit, and a
    /// clone of it to commit in.
    struct Repo {
        top: PathBuf,
        remote: PathBuf,
        work: PathBuf,
    }

    impl Repo {
        fn new(name: &str) -> Repo {
            let top =
                std::env::temp_dir().join(format!("xtask-promote-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&top);
            std::fs::create_dir_all(&top).unwrap();
            let remote = top.join("remote.git");
            sh(&top, &["init", "-q", "--bare", "remote.git"]);
            let work = top.join("work");
            sh(&top, &["init", "-q", "work"]);
            configure(&work);
            sh(
                &work,
                &["remote", "add", "origin", remote.to_str().unwrap()],
            );
            let files = [
                (
                    "Cargo.toml",
                    "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.1.0\"\n",
                ),
                ("Cargo.lock", LOCK),
                (
                    "CHANGELOG.md",
                    "# Changelog\n\n## [0.1.0] - 2026-01-01\n\n- first\n",
                ),
                ("changelog.d/README.md", "Fragments.\n"),
                (crate::METAINFO, "<releases>\n  </releases>\n"),
                (
                    "plugins/importers/x/helios-plugin.toml",
                    "[requires]\nhelios = \">=0.1.0, <0.2.0\"\n",
                ),
                ("src.txt", "code\n"),
            ];
            for (path, text) in files {
                write(&work, path, text);
            }
            sh(&work, &["add", "-A"]);
            sh(&work, &["commit", "-q", "-m", "first"]);
            sh(
                &work,
                &[
                    "push",
                    "-q",
                    "origin",
                    "HEAD:refs/heads/main",
                    "HEAD:refs/heads/next",
                ],
            );
            Repo { top, remote, work }
        }

        /// A commit on top of `next`, pushed to it.
        fn land(&self, path: &str, text: &str) -> String {
            sh(&self.work, &["fetch", "-q", "origin"]);
            sh(&self.work, &["checkout", "-q", "--detach", "origin/next"]);
            write(&self.work, path, text);
            sh(&self.work, &["add", "-A"]);
            sh(&self.work, &["commit", "-q", "-m", &format!("add {path}")]);
            sh(
                &self.work,
                &["push", "-q", "origin", "HEAD:refs/heads/next"],
            );
            sh(&self.work, &["rev-parse", "HEAD"])
        }

        /// The runner's checkout of a validated commit.
        fn checkout(&self, sha: &str) -> PathBuf {
            let dir = self.top.join(format!("ci-{}", &sha[..8]));
            let _ = std::fs::remove_dir_all(&dir);
            sh(
                &self.top,
                &[
                    "clone",
                    "-q",
                    self.remote.to_str().unwrap(),
                    dir.to_str().unwrap(),
                ],
            );
            configure(&dir);
            sh(&dir, &["checkout", "-q", "--detach", sha]);
            dir
        }

        fn branch(&self, name: &str) -> String {
            sh(&self.remote, &["rev-parse", name])
        }

        fn show(&self, rev: &str, path: &str) -> String {
            sh(&self.remote, &["show", &format!("{rev}:{path}")])
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.top);
        }
    }

    fn configure(dir: &Path) {
        for (k, v) in [
            ("user.name", "test"),
            ("user.email", "test@example.org"),
            ("commit.gpgsign", "false"),
        ] {
            sh(dir, &["config", k, v]);
        }
    }

    fn write(dir: &Path, path: &str, text: &str) {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    const ADDED: &str = "kind: added\n\nA new panel.\n";
    const FIXED: &str = "kind: fixed\n\nExports no longer stop.\n";

    #[test]
    fn fragments_make_a_release_commit_on_the_validated_commit() {
        let repo = Repo::new("release");
        let v = repo.land("changelog.d/panel.md", ADDED);
        let dir = repo.checkout(&v);
        let out = promote(&dir, "origin", &v, "2026-11-12", &ok_refresh).unwrap();
        let main = repo.branch("main");
        assert_eq!(
            out,
            Outcome::Promoted {
                main: main.clone(),
                release: Some("0.2.0".into()),
                next: Next::FastForwarded,
            }
        );
        assert_eq!(repo.branch("next"), main, "main and next stay equal");
        assert_eq!(
            sh(&repo.remote, &["rev-parse", "main^"]),
            v,
            "its parent is the validated commit"
        );
        assert_eq!(
            sh(&repo.remote, &["log", "-1", "--format=%s", "main"]),
            "Release 0.2.0"
        );
        // The release commit's content: a version bump and nothing else.
        let files = sh(&repo.remote, &["diff", "--name-status", &v, "main"]);
        let mut files: Vec<&str> = files.lines().collect();
        files.sort_unstable();
        assert_eq!(
            files,
            [
                "D\tchangelog.d/panel.md",
                "M\tCHANGELOG.md",
                "M\tCargo.lock",
                "M\tCargo.toml",
                &format!("M\t{}", crate::METAINFO),
                "M\tplugins/importers/x/helios-plugin.toml",
            ]
        );
        assert!(
            repo.show("main", "Cargo.toml")
                .contains("version = \"0.2.0\"")
        );
        assert!(
            repo.show("main", "Cargo.lock")
                .contains("name = \"helios-a\"\nversion = \"0.2.0\"")
        );
        assert!(
            repo.show("main", "CHANGELOG.md")
                .contains("## [0.2.0] - 2026-11-12\n\n### Added\n\n- A new panel.\n")
        );
        assert!(
            repo.show("main", "plugins/importers/x/helios-plugin.toml")
                .contains(">=0.2.0, <0.3.0")
        );
        // Promoting it again does nothing.
        assert_eq!(
            promote(&dir, "origin", &v, "2026-11-12", &ok_refresh).unwrap(),
            Outcome::AlreadyOnMain
        );
    }

    #[test]
    fn without_a_user_facing_fragment_the_commit_alone_is_promoted() {
        let repo = Repo::new("plain");
        repo.land("src.txt", "more code\n");
        let v = repo.land("changelog.d/refactor.md", "kind: internal\n\nRefactor.\n");
        let dir = repo.checkout(&v);
        let out = promote(&dir, "origin", &v, "2026-11-12", &ok_refresh).unwrap();
        assert_eq!(
            out,
            Outcome::Promoted {
                main: v.clone(),
                release: None,
                next: Next::Untouched,
            }
        );
        assert_eq!(repo.branch("main"), v);
        assert_eq!(repo.branch("next"), v);
        assert!(
            repo.show("main", "Cargo.toml")
                .contains("version = \"0.1.0\"")
        );
    }

    #[test]
    fn a_moved_next_gets_the_release_by_a_merge_and_is_promoted_after() {
        let repo = Repo::new("moved");
        let v = repo.land("changelog.d/panel.md", ADDED);
        let n = repo.land("changelog.d/export.md", FIXED);
        // CI passed on v; next has moved to n since.
        let dir = repo.checkout(&v);
        let out = promote(&dir, "origin", &v, "2026-11-12", &ok_refresh).unwrap();
        let main = repo.branch("main");
        let Outcome::Promoted {
            release: Some(ref r),
            next: Next::Merged(ref m),
            ..
        } = out
        else {
            panic!("{out:?}")
        };
        assert_eq!(r, "0.2.0");
        assert_eq!(
            sh(&repo.remote, &["rev-parse", "main^"]),
            v,
            "the validated commit, not next"
        );
        assert_eq!(&repo.branch("next"), m);
        assert_eq!(
            sh(&repo.remote, &["rev-list", "--parents", "-n1", m]),
            format!("{m} {n} {main}"),
            "next is merged with the release, not rewritten"
        );
        // CI then passes on n: its promotion takes the merge and releases
        // the fragment n added.
        let dir = repo.checkout(&n);
        let out = promote(&dir, "origin", &n, "2026-11-13", &ok_refresh).unwrap();
        let main = repo.branch("main");
        assert_eq!(
            out,
            Outcome::Promoted {
                main: main.clone(),
                release: Some("0.2.1".into()),
                next: Next::FastForwarded,
            }
        );
        assert_eq!(&sh(&repo.remote, &["rev-parse", "main^"]), m);
        assert_eq!(repo.branch("next"), main);
        let log = repo.show("main", "CHANGELOG.md");
        assert!(log.find("## [0.2.1]").unwrap() < log.find("## [0.2.0]").unwrap());
    }

    #[test]
    fn a_validated_commit_lacking_main_without_a_merge_is_skipped() {
        let repo = Repo::new("skip");
        let v = repo.land("changelog.d/panel.md", ADDED);
        let n = repo.land("src.txt", "more\n");
        // main got a release of v; next moved, without the merge back.
        let dir = repo.checkout(&v);
        sh(&dir, &["checkout", "-q", "--detach", &v]);
        write(
            &dir,
            "Cargo.toml",
            "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.2.0\"\n",
        );
        sh(&dir, &["commit", "-qam", "Release 0.2.0"]);
        sh(&dir, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
        let dir = repo.checkout(&n);
        let out = promote(&dir, "origin", &n, "2026-11-12", &ok_refresh).unwrap();
        assert!(matches!(out, Outcome::Skipped(_)), "{out:?}");
        assert_eq!(repo.branch("next"), n);
    }

    #[test]
    fn a_lock_change_beyond_versions_stops_before_any_push() {
        let repo = Repo::new("stale");
        let v = repo.land("changelog.d/panel.md", ADDED);
        let first = sh(&repo.remote, &["rev-parse", "main"]);
        let dir = repo.checkout(&v);
        let err = promote(&dir, "origin", &v, "2026-11-12", &stale_refresh).unwrap_err();
        assert!(
            err.contains("`serde` changes beyond a workspace version"),
            "{err}"
        );
        assert_eq!(repo.branch("main"), first);
        assert_eq!(repo.branch("next"), v);
    }

    #[test]
    fn only_a_version_bump_passes() {
        let files: std::collections::HashMap<&str, (&str, &str)> = [
            (
                "Cargo.toml",
                (
                    "[workspace.package]\nversion = \"0.1.0\"\n",
                    "[workspace.package]\nversion = \"0.2.0\"\n",
                ),
            ),
            ("src/lib.rs", ("a", "b")),
        ]
        .into();
        let old = |p: &str| files.get(p).map(|f| f.0.to_string());
        let new = |p: &str| files.get(p).map(|f| f.1.to_string());
        let c = |s: &str, p: &str| (s.to_string(), p.to_string());
        assert!(
            bump_problems(
                &[c("M", "Cargo.toml"), c("D", "changelog.d/a.md")],
                &old,
                &new
            )
            .is_empty()
        );
        assert_eq!(
            bump_problems(
                &[c("M", "src/lib.rs"), c("A", "changelog.d/b.md")],
                &old,
                &new
            )
            .len(),
            2
        );
        let lock_bumped = LOCK.replacen("version = \"0.1.0\"", "version = \"0.2.0\"", 1);
        assert_eq!(lock_problem(LOCK, &lock_bumped), None);
        assert!(lock_problem(LOCK, &LOCK.replace("1.0.0", "1.0.1")).is_some());
        assert!(lock_problem(LOCK, &LOCK.replace(" \"serde\",\n", "")).is_some());
    }
}
