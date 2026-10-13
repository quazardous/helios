//! `cargo xtask affected [BASE] [--filter | --critical]`: what a change
//! touches, so a branch runs the tests and the box jobs it can break
//! rather than the whole suite (docs/bench.md, "Running what a change
//! touches").
//!
//! 1. The files changed since the merge base of `BASE` (`origin/main` by
//!    default) and `HEAD`, the working tree's changes and untracked files
//!    included.
//! 2. Each file is mapped to the workspace crate whose directory holds it
//!    (from `cargo metadata`), or by [`rules`] for the files outside a
//!    crate: data a crate reads (`i18n/`, `tests/masks/`...), files no test
//!    reads (`docs/`, `changelog.d/`...), and files that change every build
//!    (the workspace manifest, `Cargo.lock`, the toolchain, xtask, CI, the
//!    Benchfile). One of those, or a path no rule knows, selects
//!    **everything**.
//! 3. The crates that depend on a changed crate, directly or not, any kind
//!    of dependency (normal, dev, build), are affected too: the reverse
//!    dependency closure.
//! 4. The selection is a nextest filter: the affected packages, plus the
//!    [`CRITICAL`] set, always.
//! 5. The image-data jobs follow their inputs: [`RAW_CORPUS`] and
//!    [`MASK_PARITY`] name the crates and paths each one reads.
//!
//! The selection prints as JSON on stdout (`base`, `everything`, `reasons`,
//! `changed`, `crates`, `nextest_filter`, `box_jobs`) and as a summary on
//! stderr. `--filter` prints the nextest filter alone, for a shell;
//! `--critical` prints the critical set's filter alone.
//!
//! The full suite still runs on `main` after a merge: a selection is a
//! shortcut for a branch, never the last word.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, ExitCode};

use serde_json::{Value, json};

pub const USAGE: &str = "affected [BASE] [--filter | --critical]";

/// The base a selection is computed against when none is given.
pub const DEFAULT_BASE: &str = "origin/main";

/// The critical set: the tests every selection runs, whatever changed.
///
/// - a smoke render: the engine renders a chain, and an image exports to
///   every format;
/// - a load and save round trip: an edit saved step by step and reopened
///   at its head, a vault reopened with its images and history;
/// - survival: the desktop and the node quit, killed with `kill -9` and
///   reopened (the scenarios of `cargo survival`).
///
/// `CRITICAL_TESTS` lists the test functions by file, and a unit test
/// checks that each one still exists, so a rename cannot empty the set.
pub const CRITICAL: &str = "(package(=helios-core) & binary(=engine) & test(=renders_a_chain)) \
| (package(=helios-export) & binary(=export) & test(=every_format_decodes_to_the_right_size)) \
| (package(=helios-gui) & binary(=persist) & test(=each_edit_step_is_a_version_and_reopening_loads_the_head)) \
| (package(=helios-vault-sqlite) & binary(=contract) & test(=a_sqlite_vault_survives_reopening)) \
| ((package(=helios-gui) | package(=helios-node)) & binary(=survival))";

/// The files and test functions [`CRITICAL`] names (a whole binary for
/// survival: `None`).
#[cfg(test)]
const CRITICAL_TESTS: [(&str, Option<&str>); 6] = [
    (
        "crates/helios-core/tests/engine.rs",
        Some("renders_a_chain"),
    ),
    (
        "crates/helios-export/tests/export.rs",
        Some("every_format_decodes_to_the_right_size"),
    ),
    (
        "crates/helios-gui/tests/persist.rs",
        Some("each_edit_step_is_a_version_and_reopening_loads_the_head"),
    ),
    (
        "crates/helios-vault-sqlite/tests/contract.rs",
        Some("a_sqlite_vault_survives_reopening"),
    ),
    ("crates/helios-gui/tests/survival.rs", None),
    ("crates/helios-node/tests/survival.rs", None),
];

/// A box job that reads image data, and what it reads: the crates whose
/// change (not a dependent's) runs it, and the paths outside them.
pub struct DataJob {
    pub job: &'static str,
    pub crates: &'static [&'static str],
    pub paths: &'static [&'static str],
}

/// The raw corpus: the loader, the camera description it hands out, the
/// corpus checks, the manifest and its thumbnails.
pub const RAW_CORPUS: DataJob = DataJob {
    job: "raw-corpus",
    crates: &["helios-load", "helios-photo", "helios-corpus"],
    paths: &[
        "tests/raw-corpus.toml",
        "tests/raw-corpus.thumbs.toml",
        "tests/raw-corpus-thumbs/",
        "tools/bench/fill-corpus.sh",
    ],
};

/// The drawn-mask parity: the masks module, the XMP import, the parity
/// harness, the fixtures and the darktable build.
pub const MASK_PARITY: DataJob = DataJob {
    job: "mask-parity",
    crates: &["helios-module-masks", "helios-xmp", "helios-parity"],
    paths: &["tests/masks/", "tools/bench/build-darktable.sh"],
};

pub const DATA_JOBS: [DataJob; 2] = [RAW_CORPUS, MASK_PARITY];

/// What a path outside the workspace's crates means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// Every build and test may change: run everything.
    Everything,
    /// Read by these crates (their tests or their build).
    Crates(&'static [&'static str]),
    /// Read by no test: nothing to run for it (the lint job still checks
    /// it).
    Nothing,
}

/// The rules for paths outside a crate, first match wins: a prefix ending
/// in `/` matches a directory, any other an exact path.
pub fn rules() -> &'static [(&'static str, Rule)] {
    use Rule::{Crates, Everything, Nothing};
    &[
        // every build and every test
        ("Cargo.toml", Everything),
        ("Cargo.lock", Everything),
        ("rust-toolchain.toml", Everything),
        ("rust-toolchain", Everything),
        ("xtask/", Everything),
        ("tools/bench/Benchfile", Everything),
        (".github/", Everything),
        (".config/", Everything),
        (".cargo/", Everything),
        // data the crates read
        ("i18n/", Crates(&["helios-i18n", "helios-declarations"])),
        ("tests/masks/", Crates(&["helios-parity"])),
        ("tests/raw-corpus.toml", Crates(&["helios-corpus"])),
        ("tests/raw-corpus.thumbs.toml", Crates(&["helios-corpus"])),
        ("tests/raw-corpus-thumbs/", Crates(&["helios-corpus"])),
        ("tools/bench/fill-corpus.sh", Nothing),
        ("tools/bench/build-darktable.sh", Nothing),
        // the desktop embeds the application icon
        ("packaging/", Crates(&["helios-gui"])),
        // the example plugins outside the workspace, read by the broker's
        // tests and the plugins' translation catalogues
        (
            "plugins/examples/",
            Crates(&["helios-plugin", "helios-i18n"]),
        ),
        // read by no test
        ("docs/", Nothing),
        ("changelog.d/", Nothing),
        ("README.md", Nothing),
        ("CHANGELOG.md", Nothing),
        ("ROADMAP.md", Nothing),
        ("CONTRIBUTING.md", Nothing),
        ("CLAUDE.md", Nothing),
        ("LICENSE", Nothing),
        (".gitignore", Nothing),
        // outside the workspace: GUI prototypes build on their own
        ("proto/", Nothing),
        // linters' settings: the lint job reads them
        ("clippy.toml", Nothing),
        ("rustfmt.toml", Nothing),
        ("deny.toml", Nothing),
        ("taplo.toml", Nothing),
        ("ruff.toml", Nothing),
        ("_typos.toml", Nothing),
        // research scripts, not built
        ("tools/", Nothing),
    ]
}

fn rule_for(path: &str) -> Option<Rule> {
    rules().iter().find_map(|(prefix, rule)| {
        let hit = if prefix.ends_with('/') {
            path.starts_with(prefix)
        } else {
            path == *prefix
        };
        hit.then_some(*rule)
    })
}

/// A workspace crate: its package name, its directory relative to the
/// workspace root, and the workspace crates it depends on (any kind).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crate {
    pub name: String,
    pub dir: String,
    pub deps: Vec<String>,
}

/// The workspace's crates and their dependencies on one another.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub crates: Vec<Crate>,
}

impl Graph {
    /// From `cargo metadata --no-deps --format-version 1`: the members,
    /// and the dependencies with a `path` inside the workspace.
    pub fn from_metadata(json: &str) -> Result<Graph, String> {
        let v: Value = serde_json::from_str(json).map_err(|e| format!("cargo metadata: {e}"))?;
        let root = v["workspace_root"]
            .as_str()
            .ok_or("cargo metadata: no workspace_root")?;
        let root = root.trim_end_matches('/');
        let relative = |p: &str| -> Option<String> {
            let p = p.trim_end_matches('/');
            p.strip_prefix(root)
                .map(|r| r.trim_start_matches('/').to_string())
        };
        let packages = v["packages"]
            .as_array()
            .ok_or("cargo metadata: no packages")?;
        let mut crates = Vec::new();
        for p in packages {
            let name = p["name"].as_str().ok_or("a package without a name")?;
            let manifest = p["manifest_path"]
                .as_str()
                .ok_or("a package without a manifest")?;
            let dir = Path::new(manifest)
                .parent()
                .and_then(Path::to_str)
                .and_then(relative)
                .ok_or_else(|| format!("{name}: outside the workspace"))?;
            let deps = p["dependencies"]
                .as_array()
                .map(|deps| {
                    deps.iter()
                        .filter_map(|d| d["path"].as_str().and_then(relative))
                        .collect()
                })
                .unwrap_or_default();
            crates.push(Crate {
                name: name.to_string(),
                dir,
                deps,
            });
        }
        // dependencies named by directory, now by package
        let by_dir: BTreeMap<String, String> = crates
            .iter()
            .map(|c| (c.dir.clone(), c.name.clone()))
            .collect();
        for c in &mut crates {
            c.deps = c
                .deps
                .iter()
                .filter_map(|d| by_dir.get(d).cloned())
                .collect();
        }
        Ok(Graph { crates })
    }

    /// The crate whose directory holds `path`: the deepest one.
    pub fn owner(&self, path: &str) -> Option<&Crate> {
        self.crates
            .iter()
            .filter(|c| {
                !c.dir.is_empty()
                    && path
                        .strip_prefix(c.dir.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|c| c.dir.len())
    }

    /// `seeds` and every crate depending on one of them, directly or not.
    pub fn dependents(&self, seeds: &BTreeSet<String>) -> BTreeSet<String> {
        let mut out: BTreeSet<String> = seeds
            .iter()
            .filter(|s| self.crates.iter().any(|c| &c.name == *s))
            .cloned()
            .collect();
        let mut queue: VecDeque<String> = out.iter().cloned().collect();
        while let Some(name) = queue.pop_front() {
            for c in &self.crates {
                if c.deps.contains(&name) && out.insert(c.name.clone()) {
                    queue.push_back(c.name.clone());
                }
            }
        }
        out
    }
}

/// What a change selects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Run the whole suite and every job.
    pub everything: bool,
    /// Why everything runs: the paths that asked for it.
    pub reasons: Vec<String>,
    /// The crates changed directly, by their files or the data they read.
    pub changed: BTreeSet<String>,
    /// The changed crates and their dependents.
    pub crates: BTreeSet<String>,
    /// The image-data jobs to run.
    pub data_jobs: BTreeSet<&'static str>,
}

/// Maps changed paths to a selection.
pub fn select(graph: &Graph, paths: &[String]) -> Selection {
    let mut reasons = Vec::new();
    let mut changed = BTreeSet::new();
    let mut data_jobs = BTreeSet::new();
    for path in paths {
        for job in DATA_JOBS {
            if job.paths.iter().any(|p| {
                if p.ends_with('/') {
                    path.starts_with(p)
                } else {
                    path == p
                }
            }) {
                data_jobs.insert(job.job);
            }
        }
        // a rule first: xtask is a crate, but changes every run
        match rule_for(path) {
            Some(Rule::Everything) => reasons.push(path.clone()),
            Some(Rule::Crates(names)) => {
                changed.extend(names.iter().map(|n| (*n).to_string()));
            }
            Some(Rule::Nothing) => {}
            None => match graph.owner(path) {
                Some(c) => {
                    changed.insert(c.name.clone());
                }
                None => reasons.push(format!("{path} (no rule maps it)")),
            },
        }
    }
    for job in DATA_JOBS {
        if job.crates.iter().any(|c| changed.contains(*c)) {
            data_jobs.insert(job.job);
        }
    }
    let everything = !reasons.is_empty();
    let crates = if everything {
        graph.crates.iter().map(|c| c.name.clone()).collect()
    } else {
        graph.dependents(&changed)
    };
    if everything {
        data_jobs.extend(DATA_JOBS.iter().map(|j| j.job));
    }
    Selection {
        everything,
        reasons,
        changed,
        crates,
        data_jobs,
    }
}

impl Selection {
    /// The nextest filter: the affected packages and the critical set, or
    /// every test.
    pub fn nextest_filter(&self) -> String {
        if self.everything {
            return "all()".into();
        }
        let mut parts: Vec<String> = self
            .crates
            .iter()
            .map(|c| format!("package(={c})"))
            .collect();
        parts.push(format!("({CRITICAL})"));
        parts.join(" | ")
    }

    /// The box jobs to run, in the order to run them: the lint first (it
    /// compiles fastest), then the tests, then the image-data jobs.
    pub fn box_jobs(&self) -> Vec<String> {
        let mut jobs: Vec<String> = vec!["lint".into()];
        if self.everything {
            jobs.extend(["test".into(), "survival".into()]);
        } else {
            jobs.push("test-affected".into());
        }
        jobs.extend(
            DATA_JOBS
                .iter()
                .filter(|j| self.data_jobs.contains(&j.job))
                .map(|j| j.job.to_string()),
        );
        jobs
    }

    pub fn to_json(&self, base: &str) -> Value {
        json!({
            "base": base,
            "everything": self.everything,
            "reasons": self.reasons,
            "changed": self.changed,
            "crates": self.crates,
            "nextest_filter": self.nextest_filter(),
            "box_jobs": self.box_jobs(),
        })
    }

    pub fn summary(&self, base: &str, files: usize) -> String {
        let mut out = format!("affected since {base}: {files} files changed\n");
        if self.everything {
            let _ = writeln!(
                out,
                "  everything runs, because of: {}",
                self.reasons.join(", ")
            );
        } else {
            let list = |s: &BTreeSet<String>| {
                if s.is_empty() {
                    "none".to_string()
                } else {
                    s.iter().cloned().collect::<Vec<_>>().join(", ")
                }
            };
            let _ = writeln!(out, "  changed crates: {}", list(&self.changed));
            let _ = writeln!(
                out,
                "  tested crates ({}): {}",
                self.crates.len(),
                list(&self.crates)
            );
            let _ = writeln!(
                out,
                "  plus the critical set (smoke render, round trip, survival)"
            );
        }
        let _ = writeln!(out, "  box jobs: {}", self.box_jobs().join(", "));
        out
    }
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<String>, String> {
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
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// The paths changed since the merge base of `base` and `HEAD`: commits,
/// working tree and untracked files. A rename counts both its sides.
pub fn changed_paths(root: &Path, base: &str) -> Result<Vec<String>, String> {
    let merge_base = git(root, &["merge-base", base, "HEAD"])?
        .into_iter()
        .next()
        .ok_or_else(|| format!("no merge base between {base} and HEAD"))?;
    let mut paths: BTreeSet<String> =
        git(root, &["diff", "--name-only", "--no-renames", &merge_base])?
            .into_iter()
            .collect();
    paths.extend(git(root, &["ls-files", "--others", "--exclude-standard"])?);
    Ok(paths.into_iter().collect())
}

/// The workspace graph from `cargo metadata` (no dependency resolution:
/// no network, nothing built).
pub fn workspace(root: &Path) -> Result<Graph, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Graph::from_metadata(&String::from_utf8_lossy(&out.stdout))
}

/// The selection for `base`, with the number of files changed.
pub fn compute(root: &Path, base: &str) -> Result<(Selection, usize), String> {
    let paths = changed_paths(root, base)?;
    let graph = workspace(root)?;
    Ok((select(&graph, &paths), paths.len()))
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let (mut base, mut filter, mut critical) = (None, false, false);
    for a in args {
        match *a {
            "--filter" => filter = true,
            "--critical" => critical = true,
            a if a.starts_with('-') || base.is_some() => return None,
            a => base = Some(a),
        }
    }
    if filter && critical {
        return None;
    }
    if critical {
        println!("{CRITICAL}");
        return Some(ExitCode::SUCCESS);
    }
    let base = base.unwrap_or(DEFAULT_BASE);
    Some(match compute(root, base) {
        Ok((selection, files)) => {
            if filter {
                println!("{}", selection.nextest_filter());
            } else {
                eprint!("{}", selection.summary(base, files));
                println!(
                    "{}",
                    serde_json::to_string_pretty(&selection.to_json(base)).expect("JSON")
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => crate::fail(&[e]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// core <- photo <- load <- corpus; core <- xmp <- parity (dev: load);
    /// core <- masks; gui depends on xmp and masks; xtask stands alone.
    fn graph() -> Graph {
        let c = |name: &str, dir: &str, deps: &[&str]| Crate {
            name: name.into(),
            dir: dir.into(),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
        };
        Graph {
            crates: vec![
                c("helios-core", "crates/helios-core", &[]),
                c("helios-photo", "crates/helios-photo", &["helios-core"]),
                c(
                    "helios-load",
                    "crates/helios-load",
                    &["helios-core", "helios-photo"],
                ),
                c("helios-corpus", "crates/helios-corpus", &["helios-load"]),
                c("helios-xmp", "crates/helios-xmp", &["helios-core"]),
                c(
                    "helios-parity",
                    "crates/helios-parity",
                    &["helios-xmp", "helios-load"],
                ),
                c("helios-module-masks", "modules/masks", &["helios-core"]),
                c(
                    "helios-gui",
                    "crates/helios-gui",
                    &["helios-xmp", "helios-module-masks"],
                ),
                c("helios-i18n", "crates/helios-i18n", &["helios-core"]),
                c("xtask", "xtask", &["helios-i18n"]),
            ],
        }
    }

    fn paths(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    fn set(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn metadata_parses_into_a_graph_of_workspace_crates() {
        let json = r#"{
          "workspace_root": "/w/helios",
          "packages": [
            {"name": "helios-core", "manifest_path": "/w/helios/crates/helios-core/Cargo.toml",
             "dependencies": [{"name": "serde"}]},
            {"name": "helios-module-masks", "manifest_path": "/w/helios/modules/masks/Cargo.toml",
             "dependencies": [
               {"name": "helios-core", "path": "/w/helios/crates/helios-core"},
               {"name": "helios-module-exposure", "path": "/w/helios/modules/exposure", "kind": "dev"},
               {"name": "image"}]},
            {"name": "helios-module-exposure", "manifest_path": "/w/helios/modules/exposure/Cargo.toml",
             "dependencies": []}
          ]
        }"#;
        let g = Graph::from_metadata(json).unwrap();
        assert_eq!(g.crates.len(), 3);
        assert_eq!(g.crates[1].dir, "modules/masks");
        assert_eq!(
            g.crates[1].deps,
            vec!["helios-core", "helios-module-exposure"],
            "path dependencies by package name, dev ones included"
        );
        assert!(g.crates[0].deps.is_empty(), "registry crates left out");
        assert!(Graph::from_metadata("{}").is_err());
    }

    #[test]
    fn a_file_belongs_to_the_deepest_crate_holding_it() {
        let g = graph();
        assert_eq!(
            g.owner("crates/helios-load/src/lib.rs")
                .map(|c| c.name.as_str()),
            Some("helios-load")
        );
        assert_eq!(g.owner("crates/helios-loader/x.rs"), None, "a whole name");
        assert_eq!(g.owner("crates/helios-load"), None, "a file inside");
        let mut nested = graph();
        nested.crates.push(Crate {
            name: "inner".into(),
            dir: "crates/helios-load/inner".into(),
            deps: vec![],
        });
        assert_eq!(
            nested
                .owner("crates/helios-load/inner/src/lib.rs")
                .map(|c| c.name.as_str()),
            Some("inner")
        );
    }

    #[test]
    fn a_change_selects_its_crate_and_every_dependent() {
        let s = select(&graph(), &paths(&["crates/helios-photo/src/cfa.rs"]));
        assert!(!s.everything);
        assert_eq!(s.changed, set(&["helios-photo"]));
        assert_eq!(
            s.crates,
            set(&[
                "helios-photo",
                "helios-load",
                "helios-corpus",
                "helios-parity"
            ])
        );
        assert_eq!(s.data_jobs, BTreeSet::from(["raw-corpus"]));

        let s = select(&graph(), &paths(&["crates/helios-core/src/engine.rs"]));
        assert_eq!(s.crates.len(), 10, "every crate: xtask too, through i18n");
        assert!(s.crates.contains("helios-i18n") && s.crates.contains("xtask"));
        assert!(
            s.data_jobs.is_empty(),
            "the data jobs follow direct changes"
        );

        let s = select(&graph(), &paths(&["crates/helios-gui/src/app.rs"]));
        assert_eq!(s.crates, set(&["helios-gui"]), "a leaf alone");
    }

    #[test]
    fn the_data_jobs_follow_their_inputs() {
        let jobs = |p: &[&str]| select(&graph(), &paths(p)).data_jobs;
        assert_eq!(
            jobs(&["crates/helios-load/src/raw.rs"]),
            BTreeSet::from(["raw-corpus"])
        );
        assert_eq!(
            jobs(&["tests/raw-corpus.toml"]),
            BTreeSet::from(["raw-corpus"])
        );
        assert_eq!(
            jobs(&["tools/bench/fill-corpus.sh"]),
            BTreeSet::from(["raw-corpus"])
        );
        assert_eq!(
            jobs(&["modules/masks/src/lib.rs"]),
            BTreeSet::from(["mask-parity"])
        );
        assert_eq!(
            jobs(&["crates/helios-xmp/src/import.rs"]),
            BTreeSet::from(["mask-parity"])
        );
        assert_eq!(
            jobs(&["tests/masks/circle.xmp"]),
            BTreeSet::from(["mask-parity"])
        );
        assert_eq!(
            jobs(&["tools/bench/build-darktable.sh"]),
            BTreeSet::from(["mask-parity"])
        );
        assert!(jobs(&["crates/helios-gui/src/app.rs"]).is_empty());
        let s = select(&graph(), &paths(&["tests/masks/circle.xmp"]));
        assert_eq!(s.crates, set(&["helios-parity"]), "the fixtures' own tests");
    }

    #[test]
    fn some_files_select_everything() {
        for p in [
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "xtask/src/main.rs",
            "tools/bench/Benchfile",
            ".github/workflows/ci.yml",
            ".config/nextest.toml",
            "somewhere/new.txt",
            "tests/new-data.bin",
        ] {
            let s = select(&graph(), &paths(&["docs/bench.md", p]));
            assert!(s.everything, "{p}");
            assert_eq!(s.crates.len(), graph().crates.len(), "{p}");
            assert_eq!(s.nextest_filter(), "all()", "{p}");
            assert_eq!(s.data_jobs.len(), DATA_JOBS.len(), "{p}");
            assert_eq!(
                s.box_jobs(),
                ["lint", "test", "survival", "raw-corpus", "mask-parity"]
            );
        }
        let s = select(&graph(), &paths(&["somewhere/new.txt"]));
        assert_eq!(s.reasons, ["somewhere/new.txt (no rule maps it)"]);
        assert!(
            !select(&graph(), &paths(&["crates/helios-core/Cargo.toml"])).everything,
            "a crate's own manifest is the crate's"
        );
    }

    #[test]
    fn files_no_test_reads_select_the_critical_set_alone() {
        let s = select(
            &graph(),
            &paths(&[
                "docs/bench.md",
                "changelog.d/x.md",
                "README.md",
                "proto/gui/src/main.rs",
                "clippy.toml",
                "tools/iop_inventory.py",
            ]),
        );
        assert!(!s.everything && s.crates.is_empty() && s.data_jobs.is_empty());
        assert_eq!(s.nextest_filter(), format!("({CRITICAL})"));
        assert_eq!(s.box_jobs(), ["lint", "test-affected"]);
        let s = select(&graph(), &[]);
        assert_eq!(s.nextest_filter(), format!("({CRITICAL})"), "no change");
    }

    #[test]
    fn data_outside_the_crates_selects_its_readers() {
        let s = select(&graph(), &paths(&["i18n/fr/helios.ftl"]));
        assert_eq!(s.changed, set(&["helios-i18n", "helios-declarations"]));
        // helios-declarations is not in the fake graph: dropped from the
        // closure, xtask (a dependent of i18n) kept
        assert_eq!(s.crates, set(&["helios-i18n", "xtask"]));
    }

    #[test]
    fn the_filter_names_packages_exactly_and_adds_the_critical_set() {
        let s = select(&graph(), &paths(&["crates/helios-corpus/src/lib.rs"]));
        assert_eq!(
            s.nextest_filter(),
            format!("package(=helios-corpus) | ({CRITICAL})")
        );
        assert_eq!(s.box_jobs(), ["lint", "test-affected", "raw-corpus"]);
        let j = s.to_json("origin/main");
        assert_eq!(j["base"], "origin/main");
        assert_eq!(j["crates"], json!(["helios-corpus"]));
        assert_eq!(
            j["box_jobs"],
            json!(["lint", "test-affected", "raw-corpus"])
        );
        assert_eq!(j["everything"], false);
        assert!(s.summary("origin/main", 1).contains("helios-corpus"));
    }

    #[test]
    fn the_critical_tests_exist() {
        let root = crate::root();
        for (file, test) in CRITICAL_TESTS {
            let text =
                std::fs::read_to_string(root.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
            if let Some(test) = test {
                assert!(
                    text.contains(&format!("fn {test}()")),
                    "{file}: no `{test}`"
                );
                assert!(CRITICAL.contains(&format!("test(={test})")), "{test}");
            }
            let binary = Path::new(file).file_stem().unwrap().to_string_lossy();
            assert!(CRITICAL.contains(&format!("binary(={binary})")), "{file}");
        }
    }

    #[test]
    fn arguments_parse() {
        let root = crate::root();
        assert!(run(&root, &["a", "b"]).is_none(), "one base");
        assert!(run(&root, &["--fast"]).is_none());
        assert!(run(&root, &["--filter", "--critical"]).is_none());
        assert!(run(&root, &["--critical"]).is_some());
    }
}
