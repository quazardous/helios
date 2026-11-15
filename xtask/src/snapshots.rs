//! `cargo xtask snapshots filter | report | apply`: the release step that
//! brings the reference images (the snapshot tests' PNGs) up to date on
//! `next`, each change explained (docs/releases.md, "Snapshots before a
//! release").
//!
//! - `filter` prints the nextest filter of every snapshot suite: the
//!   workspace crates that commit images under `tests/snapshots/`. The box's
//!   `snapshots-refresh` job runs them with `UPDATE_SNAPSHOTS=1` in a
//!   **workspace** build, the features CI and the release build with, so a
//!   crate drawn differently alone than in the workspace cannot hide here.
//! - `report [--since REF] [--out DIR]`, after that run: every image the
//!   run rewrote or added (against `HEAD`), its pixels changed, and its
//!   **likely cause**: the pull requests merged since `REF` (the last
//!   `v*` tag by default) that touched what draws the suite (its crate and
//!   the workspace crates it depends on, the data they read, Cargo.lock,
//!   the toolchain, kittest.toml). An image no merged pull request
//!   explains is **unexplained** and the command exits 1: regenerating it
//!   blindly would hide a regression. With `--out`, the new images, their
//!   former versions (`.old.png`) and kittest's diffs (`.diff.png`) are
//!   copied under `DIR/snapshots/` at their paths in the repository, with
//!   the report as `DIR/snapshots.md` and `DIR/snapshots.json`.
//! - `apply DIR [--version V]` takes a `report --out` folder (the job's
//!   `out/`, as `bench-remote` fetched it) and opens the pull request: a
//!   branch `chore/snapshots-<V>` from the commit the job tested, in a
//!   worktree of its own beside the checkout, the images committed with a
//!   changelog fragment, pushed, and `gh pr create` into `next` with the
//!   report and a test manifest as body. It refuses a report with an
//!   unexplained image.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde_json::{Value, json};

use crate::affected::{self, Graph, Rule};

pub const USAGE: &str = "snapshots filter | snapshots report [--since REF] [--out DIR] | snapshots apply DIR [--version V]";

/// Paths outside a crate whose change may move every image.
const RENDER_GLOBAL: [&str; 4] = [
    "Cargo.lock",
    "rust-toolchain.toml",
    "rust-toolchain",
    "kittest.toml",
];

/// kittest's working files beside a snapshot, never committed.
const SCRATCH_SUFFIXES: [&str; 3] = [".new.png", ".old.png", ".diff.png"];

/// A snapshot suite: a crate committing images, and the crates it is
/// drawn with (itself and its workspace dependencies, any kind).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suite {
    pub name: String,
    pub dir: String,
    pub drawn_by: BTreeSet<String>,
}

impl Suite {
    /// Where its images live.
    pub fn images_dir(&self) -> String {
        format!("{}/tests/snapshots/", self.dir)
    }
}

/// The suites of the workspace, from its graph; `has_images` says whether
/// a crate's directory commits snapshots.
pub fn suites(graph: &Graph, has_images: impl Fn(&str) -> bool) -> Vec<Suite> {
    graph
        .crates
        .iter()
        .filter(|c| has_images(&c.dir))
        .map(|c| {
            let mut drawn_by = BTreeSet::from([c.name.clone()]);
            let mut todo = vec![c.name.clone()];
            while let Some(name) = todo.pop() {
                if let Some(k) = graph.crates.iter().find(|k| k.name == name) {
                    for d in &k.deps {
                        if drawn_by.insert(d.clone()) {
                            todo.push(d.clone());
                        }
                    }
                }
            }
            Suite {
                name: c.name.clone(),
                dir: c.dir.clone(),
                drawn_by,
            }
        })
        .collect()
}

/// The nextest filter running every suite.
pub fn filter(suites: &[Suite]) -> String {
    suites
        .iter()
        .map(|s| format!("package(={})", s.name))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Whether a change to `path` may move the images of `suite`.
pub fn draws(graph: &Graph, suite: &Suite, path: &str) -> bool {
    if path.ends_with(".png") && path.contains("/tests/snapshots/") {
        // an image regenerated draws nothing
        return false;
    }
    if RENDER_GLOBAL.contains(&path) {
        return true;
    }
    if let Some(owner) = graph.owner(path) {
        return suite.drawn_by.contains(&owner.name);
    }
    affected::rules().iter().any(|(prefix, rule)| {
        let hit = if prefix.ends_with('/') {
            path.starts_with(prefix)
        } else {
            path == *prefix
        };
        hit && matches!(rule, Rule::Crates(list) if list.iter().any(|c| suite.drawn_by.contains(*c)))
    })
}

/// A pull request merged into the branch since the base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merged {
    pub number: u32,
    pub title: String,
    pub sha: String,
    pub paths: Vec<String>,
}

/// The number of a GitHub merge commit's subject ("Merge pull request #N
/// from …"); other merges (a release merged back) are no pull request.
pub fn pr_number(subject: &str) -> Option<u32> {
    let rest = subject.strip_prefix("Merge pull request #")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// One image the regeneration changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changed {
    pub path: String,
    pub suite: String,
    /// `None` for an image `HEAD` did not have.
    pub pixels: Option<Diff>,
    pub causes: Vec<(u32, String)>,
}

impl Changed {
    pub fn explained(&self) -> bool {
        !self.causes.is_empty()
    }
}

/// How two images differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Diff {
    /// Same size: the pixels whose colour or alpha moved.
    Pixels(u64),
    /// Another size: `(old, new)` as width and height.
    Resized((u32, u32), (u32, u32)),
}

/// An image decoded as RGBA, 8 bits a channel.
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Decodes a PNG to RGBA8, whatever its colour type and depth.
pub fn decode(bytes: &[u8]) -> Result<Rgba, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let size = reader.output_buffer_size().ok_or("image too large")?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    let data = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("indexed colours left after expansion".into()),
    };
    Ok(Rgba {
        width: info.width,
        height: info.height,
        data,
    })
}

/// How `new` differs from `old`.
pub fn diff(old: &Rgba, new: &Rgba) -> Diff {
    if (old.width, old.height) != (new.width, new.height) {
        return Diff::Resized((old.width, old.height), (new.width, new.height));
    }
    let moved = old
        .data
        .chunks_exact(4)
        .zip(new.data.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    Diff::Pixels(moved as u64)
}

fn describe(diff: Option<Diff>) -> String {
    match diff {
        None => "new image".into(),
        Some(Diff::Pixels(n)) => n.to_string(),
        Some(Diff::Resized((ow, oh), (nw, nh))) => format!("resized {ow}×{oh} → {nw}×{nh}"),
    }
}

/// The report as Markdown: one row per image, then the unexplained ones.
pub fn markdown(changed: &[Changed], since: &str, head: &str, merged: &[Merged]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Snapshots regenerated on {head} (workspace build), against the pull requests merged since {since} ({} of them).\n",
        merged.len()
    );
    if changed.is_empty() {
        out.push_str("No image changed.\n");
        return out;
    }
    out.push_str("| Image | Pixels changed | Likely cause |\n|---|---|---|\n");
    for c in changed {
        let cause = if c.explained() {
            c.causes
                .iter()
                .map(|(n, t)| format!("#{n} {t}"))
                .collect::<Vec<_>>()
                .join("; ")
        } else {
            "**unexplained**".into()
        };
        let _ = writeln!(out, "| `{}` | {} | {cause} |", c.path, describe(c.pixels));
    }
    let unexplained: Vec<&Changed> = changed.iter().filter(|c| !c.explained()).collect();
    if !unexplained.is_empty() {
        let _ = writeln!(
            out,
            "\n**{} unexplained image(s)**: no pull request merged since {since} touched what draws their suite. \
Look at the `.diff.png` before anything: a regression is fixed in the code, not in the image.",
            unexplained.len()
        );
    }
    out
}

/// The report as JSON, which `apply` reads back.
pub fn to_json(changed: &[Changed], since: &str, head: &str, version: &str) -> Value {
    let images: Vec<Value> = changed
        .iter()
        .map(|c| {
            json!({
                "path": c.path,
                "suite": c.suite,
                "pixels": describe(c.pixels),
                "explained": c.explained(),
                "causes": c.causes.iter().map(|(n, t)| json!({"pr": n, "title": t})).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "head": head,
        "since": since,
        "version": version,
        "images": images,
        "unexplained": changed.iter().filter(|c| !c.explained()).count(),
    })
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

fn git_lines(root: &Path, args: &[&str]) -> Result<Vec<String>, String> {
    Ok(String::from_utf8_lossy(&git(root, args)?)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// The pull requests merged on the first-parent line since `since`, with
/// the paths each one brought.
fn merged_since(root: &Path, since: &str) -> Result<Vec<Merged>, String> {
    let range = format!("{since}..HEAD");
    let log = git(
        root,
        &[
            "log",
            "--first-parent",
            "--merges",
            "--format=%H%x1f%s%x1f%b%x1e",
            &range,
        ],
    )?;
    let mut merged = Vec::new();
    for entry in String::from_utf8_lossy(&log).split('\x1e') {
        let mut fields = entry.trim_start().splitn(3, '\x1f');
        let (Some(sha), Some(subject)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Some(number) = pr_number(subject) else {
            continue;
        };
        let title = fields
            .next()
            .and_then(|b| b.lines().map(str::trim).find(|l| !l.is_empty()))
            .unwrap_or(subject)
            .to_string();
        let paths = git_lines(
            root,
            &[
                "diff",
                "--name-only",
                "--no-renames",
                &format!("{sha}^1"),
                sha,
            ],
        )?;
        merged.push(Merged {
            number,
            title,
            sha: sha.to_string(),
            paths,
        });
    }
    Ok(merged)
}

/// The images under a suite the working tree changed or added.
fn changed_images(root: &Path, suite: &Suite) -> Result<Vec<String>, String> {
    let dir = suite.images_dir();
    let mut paths: BTreeSet<String> =
        git_lines(root, &["diff", "--name-only", "HEAD", "--", &dir])?
            .into_iter()
            .collect();
    paths.extend(git_lines(
        root,
        &["ls-files", "--others", "--exclude-standard", "--", &dir],
    )?);
    Ok(paths
        .into_iter()
        .filter(|p| p.ends_with(".png") && !SCRATCH_SUFFIXES.iter().any(|s| p.ends_with(s)))
        .filter(|p| root.join(p).exists())
        .collect())
}

fn workspace_version(root: &Path) -> String {
    std::fs::read_to_string(root.join("Cargo.toml"))
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| {
                l.strip_prefix("version = \"")
                    .map(|v| v.trim_end_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| "unknown".into())
}

fn the_suites(root: &Path) -> Result<(Graph, Vec<Suite>), String> {
    let graph = affected::workspace(root)?;
    let found = suites(&graph, |dir| {
        crate::deps_worlds::has_snapshots(&root.join(dir))
    });
    if found.is_empty() {
        return Err("no crate commits snapshots (tests/snapshots/*.png)".into());
    }
    Ok((graph, found))
}

fn copy_out(root: &Path, out: &Path, path: &str, old: Option<&[u8]>) -> Result<(), String> {
    let dest = out.join("snapshots").join(path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::copy(root.join(path), &dest).map_err(|e| format!("{path}: {e}"))?;
    let stem = path.trim_end_matches(".png");
    if let Some(old) = old {
        let o = out.join("snapshots").join(format!("{stem}.old.png"));
        std::fs::write(&o, old).map_err(|e| format!("{}: {e}", o.display()))?;
    }
    let diff = root.join(format!("{stem}.diff.png"));
    if diff.exists() {
        let d = out.join("snapshots").join(format!("{stem}.diff.png"));
        std::fs::copy(&diff, &d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    Ok(())
}

fn report(root: &Path, since: Option<&str>, out: Option<&Path>) -> Result<bool, String> {
    let since = match since {
        Some(s) => s.to_string(),
        None => git_lines(
            root,
            &[
                "describe",
                "--tags",
                "--abbrev=0",
                "--match",
                "v[0-9]*",
                "HEAD",
            ],
        )?
        .into_iter()
        .next()
        .ok_or("no v* tag before HEAD: give --since REF")?,
    };
    let head = git_lines(root, &["rev-parse", "HEAD"])?
        .into_iter()
        .next()
        .ok_or("no HEAD")?;
    let (graph, suites) = the_suites(root)?;
    let merged = merged_since(root, &since)?;
    let mut changed = Vec::new();
    for suite in &suites {
        let causes: Vec<(u32, String)> = merged
            .iter()
            .filter(|m| m.paths.iter().any(|p| draws(&graph, suite, p)))
            .map(|m| (m.number, m.title.clone()))
            .collect();
        for path in changed_images(root, suite)? {
            let old = git(root, &["show", &format!("HEAD:{path}")]).ok();
            let new = std::fs::read(root.join(&path)).map_err(|e| format!("{path}: {e}"))?;
            let pixels = match &old {
                Some(old) => Some(diff(&decode(old)?, &decode(&new)?)),
                None => None,
            };
            if pixels == Some(Diff::Pixels(0)) {
                // rewritten byte for byte differently, drawn the same
                continue;
            }
            if let Some(out) = out {
                copy_out(root, out, &path, old.as_deref())?;
            }
            changed.push(Changed {
                path,
                suite: suite.name.clone(),
                pixels,
                causes: causes.clone(),
            });
        }
    }
    let md = markdown(&changed, &since, &head, &merged);
    print!("{md}");
    if let Some(out) = out {
        std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
        std::fs::write(out.join("snapshots.md"), &md).map_err(|e| e.to_string())?;
        let value = to_json(&changed, &since, &head, &workspace_version(root));
        std::fs::write(
            out.join("snapshots.json"),
            serde_json::to_string_pretty(&value).map_err(|e| e.to_string())? + "\n",
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(changed.iter().all(Changed::explained))
}

/// The pull request's body: the report and a test manifest naming each
/// suite touched.
pub fn pr_body(report_md: &str, suites: &BTreeSet<String>) -> String {
    let mut body = String::from(report_md);
    body.push_str("\nTests:\n");
    for s in suites {
        let _ = writeln!(body, "- test-crate crate={s}");
    }
    body.push_str(
        "Coverage: each suite whose images change passes against them (its render features are the workspace's: deps-worlds).\n",
    );
    body
}

fn run_tool(dir: &Path, program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} {} failed", args.join(" ")))
    }
}

fn apply(root: &Path, dir: &Path, version: Option<&str>) -> Result<(), String> {
    let text = std::fs::read_to_string(dir.join("snapshots.json")).map_err(|e| {
        format!(
            "{}: {e} (the `out/` of a snapshots-refresh job)",
            dir.display()
        )
    })?;
    let report: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let images = report["images"]
        .as_array()
        .ok_or("no images in the report")?;
    if images.is_empty() {
        println!("No image changed: nothing to apply.");
        return Ok(());
    }
    if report["unexplained"].as_u64().unwrap_or(0) > 0 {
        return Err("the report has unexplained images: look at their diffs and fix the cause first (docs/releases.md)".into());
    }
    let head = report["head"].as_str().ok_or("no head in the report")?;
    let version = version
        .or_else(|| report["version"].as_str())
        .ok_or("no version: give --version")?;
    let branch = format!("chore/snapshots-{version}");
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("helios");
    let tree = root.with_file_name(format!("{name}-snapshots-{version}"));
    let tree_str = tree.to_str().ok_or("a worktree path that is not UTF-8")?;
    run_tool(
        root,
        "git",
        &["worktree", "add", "-b", &branch, tree_str, head],
    )?;
    let result = (|| {
        let mut suites = BTreeSet::new();
        for image in images {
            let path = image["path"].as_str().ok_or("an image without a path")?;
            suites.insert(image["suite"].as_str().unwrap_or_default().to_string());
            let dest = tree.join(path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::copy(dir.join("snapshots").join(path), &dest)
                .map_err(|e| format!("{path}: {e}"))?;
            run_tool(&tree, "git", &["add", "--", path])?;
        }
        let fragment = format!("changelog.d/snapshots-{version}.md");
        std::fs::write(
            tree.join(&fragment),
            "kind: internal\n\nThe reference images of the snapshot tests are regenerated before the release, each change explained by the pull request that caused it.\n",
        )
        .map_err(|e| e.to_string())?;
        run_tool(&tree, "git", &["add", "--", &fragment])?;
        let title = format!("Snapshots regenerated before the release after {version}");
        run_tool(&tree, "git", &["commit", "-q", "-m", &title])?;
        run_tool(&tree, "git", &["push", "-q", "-u", "origin", &branch])?;
        let md = std::fs::read_to_string(dir.join("snapshots.md")).unwrap_or_default();
        let body = dir.join("pr-body.md");
        std::fs::write(&body, pr_body(&md, &suites)).map_err(|e| e.to_string())?;
        let body_str = body.to_str().ok_or("a path that is not UTF-8")?.to_string();
        let created = run_tool(
            &tree,
            "gh",
            &[
                "pr",
                "create",
                "--base",
                "next",
                "--head",
                &branch,
                "--title",
                &title,
                "--body-file",
                &body_str,
            ],
        );
        let _ = std::fs::remove_file(&body);
        created
    })();
    run_tool(root, "git", &["worktree", "remove", "--force", tree_str])?;
    result
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let outcome = match args {
        ["filter"] => the_suites(root).map(|(_, s)| {
            println!("{}", filter(&s));
            true
        }),
        ["report", rest @ ..] => {
            let (mut since, mut out) = (None, None);
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match *a {
                    "--since" => since = Some(*it.next()?),
                    "--out" => out = Some(PathBuf::from(it.next()?)),
                    _ => return None,
                }
            }
            report(root, since, out.as_deref())
        }
        ["apply", dir] => apply(root, Path::new(dir), None).map(|()| true),
        ["apply", dir, "--version", v] => apply(root, Path::new(dir), Some(v)).map(|()| true),
        _ => return None,
    };
    Some(match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            eprintln!("xtask: unexplained snapshot changes: the release waits (docs/releases.md)");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::from(1)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::affected::Crate;

    fn graph() -> Graph {
        let k = |name: &str, dir: &str, deps: &[&str]| Crate {
            name: name.into(),
            dir: dir.into(),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
        };
        Graph {
            crates: vec![
                k(
                    "helios-gui",
                    "crates/helios-gui",
                    &["helios-ui", "helios-i18n"],
                ),
                k("helios-ui", "crates/helios-ui", &[]),
                k("helios-i18n", "crates/helios-i18n", &[]),
                k("helios-node", "crates/helios-node", &[]),
            ],
        }
    }

    fn gui_suite(g: &Graph) -> Suite {
        suites(g, |d| d == "crates/helios-gui").remove(0)
    }

    #[test]
    fn a_suite_is_drawn_by_its_crate_and_its_dependencies() {
        let g = graph();
        let s = gui_suite(&g);
        assert_eq!(
            s.drawn_by,
            BTreeSet::from([
                "helios-gui".into(),
                "helios-ui".into(),
                "helios-i18n".into()
            ])
        );
        assert_eq!(filter(&[s]), "package(=helios-gui)");
    }

    #[test]
    fn what_draws_a_suite_explains_its_images() {
        let g = graph();
        let s = gui_suite(&g);
        assert!(draws(&g, &s, "crates/helios-ui/src/theme.rs"));
        assert!(draws(&g, &s, "Cargo.lock"));
        assert!(draws(&g, &s, "i18n/en/gui.ftl"));
        assert!(!draws(&g, &s, "crates/helios-node/src/main.rs"));
        assert!(!draws(&g, &s, "docs/gui.md"));
        assert!(!draws(&g, &s, "xtask/src/main.rs"));
        assert!(!draws(&g, &s, "crates/helios-gui/tests/snapshots/a.png"));
    }

    #[test]
    fn only_pull_request_merges_count() {
        assert_eq!(
            pr_number("Merge pull request #353 from quazardous/feat/x"),
            Some(353)
        );
        assert_eq!(pr_number("Merge release 0.91.0 into next"), None);
        assert_eq!(pr_number("Fix a colour"), None);
    }

    fn png(width: u32, height: u32, data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut enc = png::Encoder::new(&mut bytes, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(data).unwrap();
        w.finish().unwrap();
        bytes
    }

    #[test]
    fn pixels_changed_are_counted() {
        let a = png(2, 1, &[0, 0, 0, 255, 9, 9, 9, 255]);
        let b = png(2, 1, &[0, 0, 0, 255, 9, 9, 8, 255]);
        let c = png(1, 1, &[0, 0, 0, 255]);
        let (a, b, c) = (
            decode(&a).unwrap(),
            decode(&b).unwrap(),
            decode(&c).unwrap(),
        );
        assert_eq!(diff(&a, &a), Diff::Pixels(0));
        assert_eq!(diff(&a, &b), Diff::Pixels(1));
        assert_eq!(diff(&a, &c), Diff::Resized((2, 1), (1, 1)));
    }

    #[test]
    fn the_report_names_causes_and_flags_the_unexplained() {
        let changed = vec![
            Changed {
                path: "crates/helios-gui/tests/snapshots/a.png".into(),
                suite: "helios-gui".into(),
                pixels: Some(Diff::Pixels(12)),
                causes: vec![(353, "Library gallery".into())],
            },
            Changed {
                path: "crates/helios-gui/tests/snapshots/b.png".into(),
                suite: "helios-gui".into(),
                pixels: None,
                causes: vec![],
            },
        ];
        let md = markdown(&changed, "v0.91.0", "abc", &[]);
        assert!(
            md.contains(
                "| `crates/helios-gui/tests/snapshots/a.png` | 12 | #353 Library gallery |"
            ),
            "{md}"
        );
        assert!(md.contains("| new image | **unexplained** |"), "{md}");
        assert!(md.contains("1 unexplained image(s)"), "{md}");
        let v = to_json(&changed, "v0.91.0", "abc", "0.91.0");
        assert_eq!(v["unexplained"], 1);
        assert!(markdown(&[], "v0.91.0", "abc", &[]).contains("No image changed."));
        let body = pr_body(&md, &BTreeSet::from(["helios-gui".to_string()]));
        assert!(
            body.contains("Tests:\n- test-crate crate=helios-gui\nCoverage:"),
            "{body}"
        );
    }
}
