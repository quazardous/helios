//! The versions of egui-pop and egui_hud Helios builds on (docs/releases.md,
//! "egui-pop and egui_hud: next follows next").
//!
//! - On `next`, Helios builds on egui-pop's `next` and egui_hud's `next`,
//!   each by one git revision, in the root `Cargo.toml`'s one
//!   `[patch.crates-io]` table: every crate of a repository from the same
//!   revision, so the graph holds one egui-pop.
//! - `pin-bump [egui-pop=REV] [egui_hud=REV] [--pr]` moves those revisions
//!   to the given commits, or to the newest commit of each repository's
//!   `next` its CI passed on (one `gh run list` per repository). It
//!   rewrites the table, lets cargo move Cargo.lock for those crates only
//!   (`cargo metadata`: no `cargo update`, nothing else re-resolved), checks
//!   the graph holds one of each egui-family crate (`deps-worlds`' lock
//!   check, sources counted) and prints the pull request's body. With
//!   `--pr` it does so in a worktree of its own from `origin/next`, commits
//!   with a changelog fragment, pushes `chore/pin-bump-<revs>` and opens the
//!   pull request into `next`; a branch already pushed for the same
//!   revisions means there is nothing to do. The coordinator runs it when
//!   its watch sees egui-pop's (or egui_hud's) `next` turn green: no
//!   workflow polls another repository.
//! - `pins [--release]`: the revisions in use; with `--release`, the
//!   release guard: it fails while any crate of the quazardous organisation
//!   comes from git. `cargo xtask release` and the promotion apply it too:
//!   a release builds on published crates only, so the release keeper cuts
//!   egui-pop's and egui_hud's releases first.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub const USAGE: &str = "pin-bump [egui-pop=REV] [egui_hud=REV] [--pr] | pins [--release]";

/// Where the organisation's crates live, as Cargo.lock writes their git
/// sources.
const ORG: &str = "https://github.com/quazardous/";

/// A repository Helios follows on `next`: its name under [`ORG`], the CI
/// workflow whose green run names the commit to take, and which crates
/// (by name) come from it.
struct Repo {
    name: &'static str,
    workflow: &'static str,
    owns: fn(&str) -> bool,
}

const REPOS: &[Repo] = &[
    Repo {
        name: "egui-pop",
        workflow: "ci.yml",
        owns: |n| n == "egui-pop" || n.starts_with("egui-pop-"),
    },
    Repo {
        name: "egui_hud",
        workflow: "ci.yml",
        owns: |n| n == "egui_hud",
    },
];

/// A `[[package]]` of Cargo.lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub source: Option<String>,
}

pub fn packages(lock: &str) -> Vec<Package> {
    let mut out = Vec::new();
    let mut cur: Option<Package> = None;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            out.extend(cur.take());
            cur = Some(Package {
                name: String::new(),
                version: String::new(),
                source: None,
            });
            continue;
        }
        let Some(p) = cur.as_mut() else { continue };
        if let Some(v) = quoted(line, "name") {
            p.name = v;
        } else if let Some(v) = quoted(line, "version") {
            p.version = v;
        } else if let Some(v) = quoted(line, "source") {
            p.source = Some(v);
        }
    }
    out.extend(cur);
    out
}

fn quoted(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?.trim_start().strip_prefix('=')?;
    let v = rest.trim().strip_prefix('"')?.strip_suffix('"')?;
    Some(v.to_string())
}

/// The organisation's crates Cargo.lock takes from git: `(name, version,
/// repository, commit)`.
pub fn git_pins(lock: &str) -> Vec<(String, String, String, String)> {
    packages(lock)
        .into_iter()
        .filter_map(|p| {
            let src = p.source?;
            let rest = src.strip_prefix("git+")?.strip_prefix(ORG)?;
            let repo = rest.split(['?', '#']).next().unwrap_or(rest).to_string();
            let commit = rest.rsplit('#').next().unwrap_or(rest).to_string();
            Some((p.name, p.version, repo, commit))
        })
        .collect()
}

/// The release guard: what keeps a release from being cut on this lock,
/// one line per repository and commit. Empty when every crate of the
/// organisation is a published version.
pub fn release_problems(lock: &str) -> Vec<String> {
    let mut groups: Vec<((String, String), Vec<String>)> = Vec::new();
    for (name, version, repo, commit) in git_pins(lock) {
        let short = commit.get(..8).unwrap_or(&commit).to_string();
        let crate_ = format!("{name} {version}");
        match groups
            .iter_mut()
            .find(|(k, _)| *k == (repo.clone(), short.clone()))
        {
            Some((_, crates)) => crates.push(crate_),
            None => groups.push(((repo, short), vec![crate_])),
        }
    }
    groups
        .into_iter()
        .map(|((repo, short), crates)| {
            format!(
                "{repo} comes from git ({short}), not crates.io: {}. A release builds on published crates only: cut {repo}'s release first, ask for its version in Cargo.toml and drop its lines from [patch.crates-io] (docs/releases.md)",
                crates.join(", ")
            )
        })
        .collect()
}

/// The `[patch.crates-io]` lines of `manifest` that take a crate from
/// `repo`'s git, as `(line index, crate name)`.
fn patch_lines(manifest: &str, repo: &str) -> Vec<(usize, String)> {
    let url = format!("\"{ORG}{repo}\"");
    let mut inside = false;
    let mut out = Vec::new();
    for (i, line) in manifest.lines().enumerate() {
        let t = line.trim();
        if t.starts_with('[') {
            inside = t == "[patch.crates-io]";
            continue;
        }
        if inside
            && t.contains(&url)
            && let Some((name, _)) = t.split_once('=')
        {
            out.push((i, name.trim().to_string()));
        }
    }
    out
}

/// The revision each line of `repo` pins, in table order.
pub fn current_revs(manifest: &str, repo: &str) -> Vec<String> {
    let lines: Vec<&str> = manifest.lines().collect();
    patch_lines(manifest, repo)
        .into_iter()
        .filter_map(|(i, _)| rev_of(lines[i]))
        .collect()
}

/// What breaks "one revision per repository" in the manifest's
/// `[patch.crates-io]`: two crates of egui-pop from two commits are two
/// egui-pops. `cargo xtask check` runs it.
pub fn manifest_problems(manifest: &str) -> Vec<String> {
    REPOS
        .iter()
        .filter_map(|r| {
            let mut revs = current_revs(manifest, r.name);
            revs.sort();
            revs.dedup();
            (revs.len() > 1).then(|| {
                format!(
                    "Cargo.toml: [patch.crates-io] takes {} from {} revisions ({}): one revision per repository (`cargo xtask pin-bump`)",
                    r.name,
                    revs.len(),
                    revs.join(", ")
                )
            })
        })
        .collect()
}

fn rev_of(line: &str) -> Option<String> {
    let rest = &line[line.find("rev")? + 3..];
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

fn entry(name: &str, repo: &str, rev: &str) -> String {
    format!("{name} = {{ git = \"{ORG}{repo}\", rev = \"{rev}\" }}")
}

/// `manifest` with every `[patch.crates-io]` line of `repo` pinned at
/// `rev`. A repository with no line yet gets one per crate of `crates` (the
/// names Cargo.lock holds from it), at the end of the table, which is made
/// when there is none.
pub fn set_rev(manifest: &str, repo: &str, rev: &str, crates: &[String]) -> String {
    let mut lines: Vec<String> = manifest.lines().map(str::to_string).collect();
    let found = patch_lines(manifest, repo);
    if found.is_empty() {
        let new: Vec<String> = crates.iter().map(|c| entry(c, repo, rev)).collect();
        match lines.iter().position(|l| l.trim() == "[patch.crates-io]") {
            Some(start) => {
                // after the table's last entry, before the blank line that ends it
                let mut at = start + 1;
                while at < lines.len() && !lines[at].trim().starts_with('[') {
                    at += 1;
                }
                while at > start + 1 && lines[at - 1].trim().is_empty() {
                    at -= 1;
                }
                lines.splice(at..at, new);
            }
            None => {
                if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push("[patch.crates-io]".into());
                lines.extend(new);
            }
        }
    } else {
        for (i, name) in found {
            lines[i] = entry(&name, repo, rev);
        }
    }
    let mut out = lines.join("\n");
    if manifest.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// What moved in Cargo.lock besides the organisation's crates: `name
/// old -> new` lines (a dependency the new revision asks for, or drops).
pub fn other_changes(old: &str, new: &str) -> Vec<String> {
    let ours = |p: &Package| {
        REPOS.iter().any(|r| (r.owns)(&p.name))
            || p.source.as_deref().is_some_and(|s| s.contains(ORG))
    };
    let label = |p: &Package| format!("{} {}", p.name, p.version);
    let old: Vec<String> = packages(old)
        .iter()
        .filter(|p| !ours(p))
        .map(label)
        .collect();
    let new: Vec<String> = packages(new)
        .iter()
        .filter(|p| !ours(p))
        .map(label)
        .collect();
    let mut out: Vec<String> = old
        .iter()
        .filter(|p| !new.contains(p))
        .map(|p| format!("- {p} (removed)"))
        .collect();
    out.extend(
        new.iter()
            .filter(|p| !old.contains(p))
            .map(|p| format!("- {p} (added)")),
    );
    out
}

/// The pull request's body for moving the pins `moves` (`(repo, from,
/// to)`), with what else Cargo.lock moved.
pub fn pr_body(moves: &[(String, String, String)], others: &[String]) -> String {
    let mut body = String::from(
        "Helios `next` follows egui-pop and egui_hud `next` (docs/releases.md): the pins move to the newest commit their CI passed on.\n\n",
    );
    for (repo, from, to) in moves {
        if from.contains('.') {
            let _ = writeln!(body, "- {repo}: {from} -> {to}");
        } else {
            let _ = writeln!(
                body,
                "- {repo}: {from} -> {to} ({ORG}{repo}/compare/{from}...{to})"
            );
        }
    }
    if others.is_empty() {
        body.push_str("\nCargo.lock moved those crates only.\n");
    } else {
        body.push_str("\nCargo.lock also moved what the new revisions ask for:\n");
        for o in others {
            let _ = writeln!(body, "{o}");
        }
    }
    body.push_str(
        "\nTests:\n- check\n- lint\n- journey\nCoverage: the workspace builds and passes clippy on the new revisions, and the first-user journey runs on them; next's CI runs the whole suite.\n",
    );
    body
}

fn run_out(dir: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// The newest commit of `repo`'s `next` its CI passed on.
fn green_next(dir: &Path, repo: &Repo) -> Result<String, String> {
    let sha = run_out(
        dir,
        "gh",
        &[
            "run",
            "list",
            "-R",
            &format!("quazardous/{}", repo.name),
            "--workflow",
            repo.workflow,
            "--branch",
            "next",
            "--event",
            "push",
            "--status",
            "success",
            "--limit",
            "1",
            "--json",
            "headSha",
            "-q",
            ".[0].headSha",
        ],
    )?;
    if sha.len() < 8 {
        return Err(format!(
            "no green CI run on {}'s next: give {}=REV",
            repo.name, repo.name
        ));
    }
    Ok(sha[..8].to_string())
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The bump in the checkout `dir`: `(moves, body)`, nothing moved when
/// `moves` is empty.
fn bump(
    dir: &Path,
    revs: &[(&Repo, String)],
) -> Result<(Vec<(String, String, String)>, String), String> {
    let manifest_path = dir.join("Cargo.toml");
    let lock_path = dir.join("Cargo.lock");
    let manifest = read(&manifest_path)?;
    let old_lock = read(&lock_path)?;
    let mut text = manifest.clone();
    let mut moves = Vec::new();
    for (repo, rev) in revs {
        let current = current_revs(&text, repo.name);
        if !current.is_empty() && current.iter().all(|r| r == rev) {
            continue;
        }
        let crates: Vec<String> = {
            let mut c: Vec<String> = packages(&old_lock)
                .into_iter()
                .map(|p| p.name)
                .filter(|n| (repo.owns)(n))
                .collect();
            c.dedup();
            c
        };
        let from = current
            .first()
            .cloned()
            .unwrap_or_else(|| "crates.io".into());
        text = set_rev(&text, repo.name, rev, &crates);
        moves.push((repo.name.to_string(), from, rev.clone()));
    }
    if moves.is_empty() {
        return Ok((moves, String::new()));
    }
    write(&manifest_path, &text)?;
    // cargo resolves the changed patches and nothing else
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--quiet"])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !status.success() {
        write(&manifest_path, &manifest)?;
        write(&lock_path, &old_lock)?;
        return Err(
            "cargo metadata failed on the new revisions: Cargo.toml and Cargo.lock restored".into(),
        );
    }
    let new_lock = read(&lock_path)?;
    let duplicates = crate::deps_worlds::lock_duplicates(&new_lock);
    if !duplicates.is_empty() {
        write(&manifest_path, &manifest)?;
        write(&lock_path, &old_lock)?;
        return Err(format!(
            "the new revisions leave two of a crate in the graph (a dependency asks for another version than the pinned one; move that repository's next first), Cargo.toml and Cargo.lock restored: {}",
            duplicates.join("; ")
        ));
    }
    let body = pr_body(&moves, &other_changes(&old_lock, &new_lock));
    Ok((moves, body))
}

fn resolve_revs(
    dir: &Path,
    given: &[(&str, &str)],
) -> Result<Vec<(&'static Repo, String)>, String> {
    REPOS
        .iter()
        .map(|r| {
            let rev = match given.iter().find(|(k, _)| *k == r.name) {
                Some((_, v)) => (*v).to_string(),
                None => green_next(dir, r)?,
            };
            Ok((r, rev))
        })
        .collect()
}

fn pin_bump(root: &Path, given: &[(&str, &str)], pr: bool) -> Result<(), String> {
    let revs = resolve_revs(root, given)?;
    if !pr {
        let (moves, body) = bump(root, &revs)?;
        if moves.is_empty() {
            println!("The pins are already at those revisions: nothing to bump.");
        } else {
            println!("{body}");
        }
        return Ok(());
    }
    let tag: Vec<&str> = revs.iter().map(|(_, r)| r.as_str()).collect();
    let branch = format!("chore/pin-bump-{}", tag.join("-"));
    if !run_out(
        root,
        "git",
        &["ls-remote", "origin", &format!("refs/heads/{branch}")],
    )?
    .is_empty()
    {
        println!("{branch} is already pushed: its pull request carries this bump.");
        return Ok(());
    }
    run_out(root, "git", &["fetch", "-q", "origin", "next"])?;
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("helios");
    let tree: PathBuf = root.with_file_name(format!("{name}-pin-bump"));
    let tree_str = tree.to_str().ok_or("a worktree path that is not UTF-8")?;
    run_out(
        root,
        "git",
        &["worktree", "add", "-b", &branch, tree_str, "origin/next"],
    )?;
    let result = (|| {
        let (moves, body) = bump(&tree, &revs)?;
        if moves.is_empty() {
            println!("next already builds on those revisions: nothing to bump.");
            return Ok(false);
        }
        let fragment = format!("changelog.d/{}.md", branch.replace('/', "-"));
        write(
            &tree.join(&fragment),
            "kind: internal\n\nHelios `next` moves to the newest green commits of egui-pop and egui_hud `next`.\n",
        )?;
        run_out(
            &tree,
            "git",
            &["add", "Cargo.toml", "Cargo.lock", &fragment],
        )?;
        let summary: Vec<String> = moves.iter().map(|(r, _, to)| format!("{r} {to}")).collect();
        let title = format!("Pins: next follows {}", summary.join(", "));
        run_out(&tree, "git", &["commit", "-q", "-m", &title])?;
        run_out(&tree, "git", &["push", "-q", "-u", "origin", &branch])?;
        let url = run_out(
            &tree,
            "gh",
            &[
                "pr", "create", "--base", "next", "--head", &branch, "--title", &title, "--body",
                &body,
            ],
        )?;
        println!("{url}");
        Ok(true)
    })();
    let removed = run_out(root, "git", &["worktree", "remove", "--force", tree_str]);
    if matches!(result, Ok(false)) {
        let _ = run_out(root, "git", &["branch", "-D", &branch]);
    }
    result.and(removed).map(|_| ())
}

fn pins(root: &Path, release: bool) -> Result<bool, String> {
    let lock = read(&root.join("Cargo.lock"))?;
    if release {
        let problems = release_problems(&lock);
        for p in &problems {
            eprintln!("xtask: {p}");
        }
        if problems.is_empty() {
            println!(
                "Every crate of the organisation is a published version: a release may be cut."
            );
        }
        return Ok(problems.is_empty());
    }
    let manifest = read(&root.join("Cargo.toml"))?;
    for r in REPOS {
        let revs = current_revs(&manifest, r.name);
        match revs.first() {
            Some(rev) => println!("{}: git {rev} ({} crates)", r.name, revs.len()),
            None => println!("{}: crates.io", r.name),
        }
    }
    Ok(true)
}

pub fn run_bump(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let mut given = Vec::new();
    let mut pr = false;
    for a in args {
        if *a == "--pr" {
            pr = true;
        } else {
            let (k, v) = a.split_once('=')?;
            if !REPOS.iter().any(|r| r.name == k) || v.is_empty() {
                return None;
            }
            given.push((k, v));
        }
    }
    Some(match pin_bump(root, &given, pr) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::from(1)
        }
    })
}

pub fn run_pins(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let release = match args {
        [] => false,
        ["--release"] => true,
        _ => return None,
    };
    Some(match pins(root, release) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::from(1)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "[workspace]\nmembers = []\n\n[patch.crates-io]\n\
        egui-pop = { git = \"https://github.com/quazardous/egui-pop\", rev = \"d76e8d22\" }\n\
        egui-pop-core = { git = \"https://github.com/quazardous/egui-pop\", rev = \"d76e8d22\" }\n\
        \n[profile.dev]\ndebug = false\n";

    const LOCK: &str = "version = 4\n\n\
        [[package]]\nname = \"egui-pop\"\nversion = \"0.11.0\"\n\
        source = \"git+https://github.com/quazardous/egui-pop?rev=d76e8d22#d76e8d225320f4e8\"\n\n\
        [[package]]\nname = \"egui_hud\"\nversion = \"0.4.0\"\n\
        source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n\
        [[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n\
        source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n\
        [[package]]\nname = \"helios-core\"\nversion = \"0.90.0\"\n";

    #[test]
    fn pins_rev_rewrite_moves_every_line_of_the_repo_and_nothing_else() {
        let out = set_rev(MANIFEST, "egui-pop", "abcdef12", &[]);
        assert_eq!(out, MANIFEST.replace("d76e8d22", "abcdef12"));
        assert_eq!(current_revs(&out, "egui-pop"), ["abcdef12", "abcdef12"]);
        assert!(current_revs(&out, "egui_hud").is_empty());
    }

    #[test]
    fn pins_check_refuses_two_revisions_of_one_repo() {
        assert!(manifest_problems(MANIFEST).is_empty());
        let mixed = MANIFEST.replacen("d76e8d22", "abcdef12", 1);
        let p = manifest_problems(&mixed);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].contains("egui-pop from 2 revisions"), "{p:?}");
    }

    #[test]
    fn pins_rev_rewrite_adds_a_repo_to_the_one_table() {
        let out = set_rev(MANIFEST, "egui_hud", "b8d74871", &["egui_hud".into()]);
        let expected = MANIFEST.replace(
            "rev = \"d76e8d22\" }\n\n[profile",
            "rev = \"d76e8d22\" }\negui_hud = { git = \"https://github.com/quazardous/egui_hud\", rev = \"b8d74871\" }\n\n[profile",
        );
        assert_eq!(out, expected);
        assert_eq!(out.matches("[patch.crates-io]").count(), 1);
    }

    #[test]
    fn pins_rev_rewrite_makes_the_table_after_a_release() {
        let out = set_rev(
            "[workspace]\n",
            "egui_hud",
            "b8d74871",
            &["egui_hud".into()],
        );
        assert_eq!(
            out,
            "[workspace]\n\n[patch.crates-io]\negui_hud = { git = \"https://github.com/quazardous/egui_hud\", rev = \"b8d74871\" }\n"
        );
    }

    #[test]
    fn pins_one_graph_check_refuses_a_crate_from_git_and_crates_io() {
        let two = format!(
            "{LOCK}\n[[package]]\nname = \"egui-pop\"\nversion = \"0.11.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n"
        );
        assert!(crate::deps_worlds::lock_duplicates(LOCK).is_empty());
        let d = crate::deps_worlds::lock_duplicates(&two);
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].contains("`egui-pop`"), "{d:?}");
    }

    #[test]
    fn pins_release_guard_refuses_a_git_source() {
        let p = release_problems(LOCK);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(
            p[0].starts_with("egui-pop comes from git (d76e8d22), not crates.io: egui-pop 0.11.0."),
            "{p:?}"
        );
        let published = LOCK.replace(
            "git+https://github.com/quazardous/egui-pop?rev=d76e8d22#d76e8d225320f4e8",
            "registry+https://github.com/rust-lang/crates.io-index",
        );
        assert!(release_problems(&published).is_empty());
    }

    #[test]
    fn pins_other_changes_skip_the_organisations_crates() {
        let new = LOCK.replace("d76e8d22", "abcdef12").replace(
            "name = \"serde\"\nversion = \"1.0.0\"",
            "name = \"serde\"\nversion = \"1.0.1\"",
        );
        assert_eq!(
            other_changes(LOCK, &new),
            ["- serde 1.0.0 (removed)", "- serde 1.0.1 (added)"]
        );
        assert!(other_changes(LOCK, &LOCK.replace("d76e8d22", "abcdef12")).is_empty());
    }

    #[test]
    fn pins_pr_body_names_the_moves_and_the_tests() {
        let body = pr_body(
            &[("egui-pop".into(), "d76e8d22".into(), "abcdef12".into())],
            &[],
        );
        assert!(body.contains("- egui-pop: d76e8d22 -> abcdef12"), "{body}");
        assert!(body.contains("\nTests:\n- check\n"), "{body}");
    }
}
