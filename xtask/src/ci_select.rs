//! `cargo xtask ci-broker select`: the CI broker's always-run set for a
//! change (step B2, docs/ci-broker.md), printed as a nextest filter.
//!
//! The union of four sources (`ci_broker/select.rs`):
//!
//! 1. **affected**, at file level: each changed file is mapped to the
//!    tests near it, finer than `cargo xtask affected`'s crates.
//!    - The paths outside a crate follow `affected`'s rules: Cargo.lock,
//!      the toolchain, xtask, CI... select everything; data a crate reads
//!      selects that crate.
//!    - A crate's integration test file (`tests/<target>.rs`) selects its
//!      binary alone; any other file of the crate selects the crate and,
//!      like `affected`, every crate depending on it. A change in a crate
//!      many depend on (helios-core) so selects nearly everything.
//!    - helios-gui's integration tests are the modules of one binary,
//!      `it`. A changed source of helios-gui selects the `it` modules that
//!      exercise it ([`gui_modules`]): by name, by the paths they `use`,
//!      and by [`OVERRIDES`]. When none is found, the whole `it` runs:
//!      the map may select too much, never knowingly too little.
//! 2. **changed tests**: the `#[test]` functions the diff adds or edits.
//! 3. **recent failures**: the tests that failed or flaked in the last N
//!    runs of the `ci-history` summary; skipped, and said, while there is
//!    no history.
//! 4. **smoke**: the list in `ci/smoke.txt`.
//!
//! The filter goes to stdout, the summary per source to stderr.
//! `--explain '<binary id> <test name>'` says why one test runs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::ExitCode;

use crate::affected::{self, Graph, Rule};
use crate::ci_broker::history::Summary;
use crate::ci_broker::select::{
    Scope, Selection, Source, hunks, parse_smoke, recent_failures, test_scope, touched_tests,
};

pub const USAGE: &str =
    "ci-broker select [--base REF] [--history DIR] [--runs N] [--explain '<binary id> <test>']";

/// The base a change is computed against: pull requests go into `next`.
pub const DEFAULT_BASE: &str = "origin/next";
/// How many of the last runs the history source reads.
pub const DEFAULT_RUNS: usize = 20;
/// The smoke list, from the workspace root.
pub const SMOKE: &str = "ci/smoke.txt";

pub const GUI: &str = "helios-gui";
pub const GUI_DIR: &str = "crates/helios-gui";
pub const GUI_IT: &str = "helios-gui::it";
const GUI_IT_DIR: &str = "crates/helios-gui/tests/it";

/// helios-gui's source modules too big to map as one: a file under
/// `src/shell/` is mapped by its own name (`shell::import_window`).
const HUBS: [&str; 1] = ["shell"];

/// The checked-in corrections to the map: helios-gui source path prefixes
/// and the `it` modules that exercise them, beside those the names and
/// the `use` paths find. `affected`'s `GUI_SUITES` is read too. Add a
/// line when the shadow runs (B3) show a module the map missed.
pub const OVERRIDES: &[(&[&str], &[&str])] = &[
    // what each history row says is drawn by the history panels
    (
        &["crates/helios-gui/src/library_history"],
        &["history_panel", "history_preview"],
    ),
];

/// One changed file: its path, its content now (`None`: deleted) and the
/// lines the diff touched in it (`None`: the whole file is new).
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub path: String,
    pub content: Option<String>,
    pub lines: Option<Vec<(usize, usize)>>,
}

/// The history as read.
#[derive(Clone, Debug)]
pub enum History {
    /// No summary yet: the source is skipped.
    Missing(String),
    /// A summary that does not read: everything runs.
    Unreadable(String),
    Read(Box<Summary>),
}

/// What the selection reads, gathered from git and the files
/// ([`gather`]), or given by a test.
pub struct Input {
    pub graph: Graph,
    pub changes: Vec<Change>,
    /// helios-gui's `it` modules and their source.
    pub gui_suites: BTreeMap<String, String>,
    pub history: History,
    pub runs: usize,
    pub smoke: Vec<Scope>,
}

/// The unit a helios-gui source path names, as `(path, leaf)`: `library`
/// for `src/library/view.rs`, `shell::import_window` for
/// `src/shell/import_window.rs`. `None` for the crate's root files.
fn gui_unit(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix(GUI_DIR)?.strip_prefix("/src/")?;
    let mut parts = rest.split('/');
    let stem = |s: &str| s.trim_end_matches(".rs").to_string();
    let top = stem(parts.next()?);
    if ["lib", "main", "bin"].contains(&top.as_str()) {
        return None;
    }
    if HUBS.contains(&top.as_str())
        && let Some(sub) = parts.next()
    {
        let leaf = stem(sub);
        if leaf != "mod" {
            return Some((format!("{top}::{leaf}"), leaf));
        }
    }
    Some((top.clone(), top))
}

fn idents(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
}

/// Whether a test module's source names the unit by a `helios_gui::` path.
fn uses_unit(source: &str, unit: &str) -> bool {
    let segments: Vec<&str> = unit.split("::").collect();
    source.split("helios_gui::").skip(1).any(|after| {
        let span = after.split(';').next().unwrap_or("");
        let mut words = idents(span);
        if span.trim_start().starts_with('{') {
            // `helios_gui::{library, shell::x}`: every segment named
            let all: BTreeSet<&str> = words.collect();
            segments.iter().all(|s| all.contains(s))
        } else if words.next() != Some(segments[0]) {
            false
        } else {
            let rest: BTreeSet<&str> = words.collect();
            segments[1..].iter().all(|s| rest.contains(s))
        }
    })
}

/// The `it` modules that exercise a helios-gui source file, each with
/// why; empty when the map knows none (the caller then runs all of `it`).
pub fn gui_modules(path: &str, suites: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (prefixes, modules) in affected::GUI_SUITES.iter().chain(OVERRIDES) {
        if let Some(p) = prefixes.iter().find(|p| path.starts_with(**p)) {
            for m in *modules {
                out.entry((*m).to_string())
                    .or_insert_with(|| format!("override for {p}"));
            }
        }
    }
    let Some((unit, leaf)) = gui_unit(path) else {
        return out;
    };
    for (module, source) in suites {
        let named = *module == leaf
            || module
                .strip_prefix(leaf.as_str())
                .is_some_and(|r| r.starts_with('_'))
            || leaf
                .strip_prefix(module.as_str())
                .is_some_and(|r| r.starts_with('_'));
        if named {
            out.entry(module.clone())
                .or_insert_with(|| format!("named like `{unit}`"));
        } else if uses_unit(source, &unit) {
            out.entry(module.clone())
                .or_insert_with(|| format!("uses helios_gui::{unit}"));
        }
    }
    out
}

/// The `it` modules whose source names module `m` (`crate::m`, `super::m`):
/// shared test helpers.
fn suites_using(m: &str, suites: &BTreeMap<String, String>) -> Vec<String> {
    suites
        .iter()
        .filter(|(name, src)| {
            *name != m
                && (src.contains(&format!("crate::{m}")) || src.contains(&format!("super::{m}")))
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Source 1, for one file.
fn affected_file(input: &Input, path: &str, s: &mut Selection) {
    let src = Source::Affected;
    let whole = |s: &mut Selection, seeds: &[&str], why: &str| {
        let seeds: BTreeSet<String> = seeds.iter().map(|c| (*c).to_string()).collect();
        for c in input.graph.dependents(&seeds) {
            let why = if seeds.contains(&c) {
                why.to_string()
            } else {
                format!("{why}, a dependency of {c}")
            };
            s.add(src, Scope::Package(c), why);
        }
    };
    match affected::rule_for(path) {
        Some(Rule::Everything) => {
            return s.add(src, Scope::All, format!("{path} changes every build"));
        }
        Some(Rule::Crates(names)) => return whole(s, names, &format!("{path} is read by it")),
        Some(Rule::Nothing) => return,
        None => {}
    }
    let Some(owner) = input.graph.owner(path) else {
        return s.add(src, Scope::All, format!("{path}: no rule maps it"));
    };
    let name = owner.name.as_str();
    let rest = &path[owner.dir.len() + 1..];
    if let Some(test) = rest.strip_prefix("tests/") {
        let parts: Vec<&str> = test.split('/').collect();
        match parts.as_slice() {
            [file] if file.ends_with(".rs") => {
                let target = file.trim_end_matches(".rs");
                s.add(
                    src,
                    Scope::Binary(format!("{name}::{target}")),
                    format!("{path} changed"),
                );
            }
            ["it", "main.rs"] if name == GUI => {
                s.add(src, Scope::Binary(GUI_IT.into()), format!("{path} changed"));
            }
            ["it", module, ..] if name == GUI => {
                let module = module.trim_end_matches(".rs");
                s.add(
                    src,
                    Scope::Module {
                        binary: GUI_IT.into(),
                        module: module.into(),
                    },
                    format!("{path} changed"),
                );
                for user in suites_using(module, &input.gui_suites) {
                    s.add(
                        src,
                        Scope::Module {
                            binary: GUI_IT.into(),
                            module: user,
                        },
                        format!("uses the it module {module}, changed"),
                    );
                }
            }
            _ => s.add(
                src,
                Scope::Package(name.into()),
                format!("{path}: test data or helpers"),
            ),
        }
        return;
    }
    if name == GUI && rest.starts_with("src/") {
        let modules = gui_modules(path, &input.gui_suites);
        if modules.is_empty() {
            s.add(
                src,
                Scope::Binary(GUI_IT.into()),
                format!("{path}: no it module known for it"),
            );
        }
        for (module, why) in modules {
            s.add(
                src,
                Scope::Module {
                    binary: GUI_IT.into(),
                    module,
                },
                format!("{path}: {why}"),
            );
        }
        s.add(
            src,
            Scope::PackageBut {
                package: GUI.into(),
                binary: GUI_IT.into(),
            },
            format!("{path} changed (the crate's other tests)"),
        );
        let seeds = BTreeSet::from([GUI.to_string()]);
        for c in input.graph.dependents(&seeds) {
            if c != GUI {
                s.add(
                    src,
                    Scope::Package(c.clone()),
                    format!("{path} changed, a dependency of {c}"),
                );
            }
        }
        return;
    }
    whole(s, &[name], &format!("{path} changed"));
}

/// Source 2, for one file.
fn changed_tests(input: &Input, change: &Change, s: &mut Selection) {
    let (Some(content), true) = (&change.content, change.path.ends_with(".rs")) else {
        return;
    };
    let Some(owner) = input.graph.owner(&change.path) else {
        return;
    };
    let rest = &change.path[owner.dir.len() + 1..];
    let whole = [(1, usize::MAX)];
    let lines = change.lines.as_deref().unwrap_or(&whole);
    let names = touched_tests(content, lines);
    if names.is_empty() {
        return;
    }
    let (binary, prefix) = match rest
        .strip_prefix("tests/")
        .map(|t| t.split('/').collect::<Vec<_>>())
    {
        Some(parts) if owner.name == GUI && parts.len() >= 2 && parts[0] == "it" => {
            let module = parts[1].trim_end_matches(".rs");
            if module == "main" {
                (Some(GUI_IT.to_string()), String::new())
            } else {
                (Some(GUI_IT.to_string()), format!("{module}::"))
            }
        }
        Some(parts) if parts.len() == 1 => (
            Some(format!(
                "{}::{}",
                owner.name,
                parts[0].trim_end_matches(".rs")
            )),
            String::new(),
        ),
        _ => (None, String::new()),
    };
    let how = if change.lines.is_none() {
        "new file"
    } else {
        "added or edited"
    };
    for name in names {
        s.add(
            Source::ChangedTest,
            Scope::Fn {
                package: owner.name.clone(),
                binary: binary.clone(),
                prefix: prefix.clone(),
                name,
            },
            format!("{} ({how})", change.path),
        );
    }
}

/// The always-run set for the input.
pub fn select(input: &Input) -> Selection {
    let mut s = Selection::default();
    for change in &input.changes {
        affected_file(input, &change.path, &mut s);
        changed_tests(input, change, &mut s);
    }
    if input.changes.is_empty() {
        s.note(Source::Affected, "no file changed");
    }
    match &input.history {
        History::Missing(why) => s.note(Source::History, format!("skipped, no history yet: {why}")),
        History::Unreadable(why) => {
            s.add(
                Source::History,
                Scope::All,
                format!("the history does not read ({why}): every test runs"),
            );
        }
        History::Read(summary) => {
            let read = summary.ingested.len().min(input.runs);
            s.note(
                Source::History,
                format!(
                    "read the last {read} of {} runs in the summary",
                    summary.runs
                ),
            );
            for (id, why) in recent_failures(summary, input.runs) {
                match test_scope(&id) {
                    Some(scope) => s.add(Source::History, scope, why),
                    None => s.note(Source::History, format!("`{id}`: not a test id, left out")),
                }
            }
        }
    }
    for scope in &input.smoke {
        s.add(Source::Smoke, scope.clone(), SMOKE);
    }
    s
}

/// The input from git, the working tree and the history.
pub fn gather(
    root: &Path,
    base: &str,
    history: Option<&Path>,
    runs: usize,
) -> Result<Input, String> {
    let paths = affected::changed_paths(root, base)?;
    let merge_base = affected::git(root, &["merge-base", base, "HEAD"])?
        .into_iter()
        .next()
        .ok_or_else(|| format!("no merge base between {base} and HEAD"))?;
    let diff = std::process::Command::new("git")
        .args([
            "diff",
            "-U0",
            "--no-renames",
            "--no-color",
            &merge_base,
            "--",
            "*.rs",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let touched: BTreeMap<String, Vec<(usize, usize)>> =
        hunks(&String::from_utf8_lossy(&diff.stdout))
            .into_iter()
            .collect();
    let changes = paths
        .into_iter()
        .map(|path| {
            let content = std::fs::read_to_string(root.join(&path)).ok();
            // an untracked file is not in the diff: all of it is new
            let lines = touched.get(&path).cloned();
            Change {
                path,
                content,
                lines,
            }
        })
        .collect();
    let mut gui_suites = BTreeMap::new();
    let dir = root.join(GUI_IT_DIR);
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().is_some_and(|e| e == "rs") && stem != "main" {
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            gui_suites.insert(stem.to_string(), text);
        }
    }
    let history = read_history(root, history);
    let smoke_path = root.join(SMOKE);
    let smoke = std::fs::read_to_string(&smoke_path)
        .map_err(|e| format!("{SMOKE}: {e}"))
        .and_then(|t| parse_smoke(&t).map_err(|e| format!("{SMOKE}: {e}")))?;
    Ok(Input {
        graph: affected::workspace(root)?,
        changes,
        gui_suites,
        history,
        runs,
        smoke,
    })
}

fn read_history(root: &Path, dir: Option<&Path>) -> History {
    let text = match dir {
        Some(dir) => match std::fs::read_to_string(dir.join("summary.json")) {
            Ok(t) => t,
            Err(e) => return History::Missing(format!("{}/summary.json: {e}", dir.display())),
        },
        None => {
            let out = std::process::Command::new("git")
                .args(["show", "origin/ci-history:summary.json"])
                .current_dir(root)
                .output();
            match out {
                Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
                _ => {
                    return History::Missing(
                        "no summary.json on origin/ci-history (`git fetch origin ci-history`)"
                            .into(),
                    );
                }
            }
        }
    };
    match Summary::from_json(&text) {
        Ok(s) => History::Read(Box::new(s)),
        Err(e) => History::Unreadable(e),
    }
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let ["select", rest @ ..] = args else {
        return None;
    };
    let (mut base, mut history, mut runs, mut explain) = (DEFAULT_BASE, None, DEFAULT_RUNS, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--base" => base = *it.next()?,
            "--history" => history = Some(Path::new(*it.next()?)),
            "--runs" => runs = it.next()?.parse().ok()?,
            "--explain" => explain = Some(*it.next()?),
            _ => return None,
        }
    }
    let input = match gather(root, base, history, runs) {
        Ok(i) => i,
        Err(e) => return Some(crate::fail(&[format!("ci-broker select: {e}")])),
    };
    let selection = select(&input);
    if let Some(test) = explain {
        return Some(match selection.explain(test) {
            Ok(picks) if picks.is_empty() => {
                println!("{test}: not in the always-run set since {base}");
                ExitCode::SUCCESS
            }
            Ok(picks) => {
                println!("{test}: selected since {base} by");
                for p in picks {
                    println!("  - {}: {} [{}]", p.source.name(), p.why, p.scope.filter());
                }
                ExitCode::SUCCESS
            }
            Err(e) => crate::fail(&[e]),
        });
    }
    eprintln!(
        "ci-broker select since {base}: {} files changed",
        input.changes.len()
    );
    eprint!("{}", selection.summary());
    println!("{}", selection.filter());
    Some(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::affected::Crate;
    use crate::ci_broker::history::{Stamp, TestStats};

    fn graph() -> Graph {
        let c = |name: &str, deps: &[&str]| Crate {
            name: name.into(),
            dir: format!("crates/{name}"),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
        };
        Graph {
            crates: vec![
                c("helios-core", &[]),
                c("helios-library", &["helios-core"]),
                c("helios-gui", &["helios-core", "helios-library"]),
                c("helios-gui-agent", &["helios-gui"]),
            ],
        }
    }

    fn suites() -> BTreeMap<String, String> {
        let s = |k: &str, v: &str| (k.to_string(), v.to_string());
        BTreeMap::from([
            s("import", "use helios_gui::shell::App;\n"),
            s(
                "import_albums",
                "use helios_gui::shell::{App, import_albums::Albums};\n",
            ),
            s(
                "storage",
                "use helios_gui::{shell::App, import_scan::Scan};\n",
            ),
            s(
                "persist",
                "use helios_gui::persist::Store;\nuse crate::support::x;\n",
            ),
            s("support", "pub fn x() {}\n"),
            s("culling", "use helios_gui::library::Cull;\n"),
            s("undo", "use helios_gui::shell::App;\n"),
        ])
    }

    fn input(changes: Vec<Change>, history: History) -> Input {
        Input {
            graph: graph(),
            changes,
            gui_suites: suites(),
            history,
            runs: DEFAULT_RUNS,
            smoke: parse_smoke("helios-core::engine renders_a_chain\n").unwrap(),
        }
    }

    fn change(path: &str) -> Change {
        Change {
            path: path.into(),
            content: Some(String::new()),
            lines: Some(vec![]),
        }
    }

    fn missing() -> History {
        History::Missing("none".into())
    }

    fn module(m: &str) -> Scope {
        Scope::Module {
            binary: GUI_IT.into(),
            module: m.into(),
        }
    }

    fn scopes(s: &Selection, source: Source) -> Vec<Scope> {
        s.picks
            .iter()
            .filter(|p| p.source == source)
            .map(|p| p.scope.clone())
            .collect()
    }

    #[test]
    fn a_gui_module_change_selects_its_it_modules() {
        let s = select(&input(
            vec![change("crates/helios-gui/src/import_scan.rs")],
            missing(),
        ));
        let got = scopes(&s, Source::Affected);
        // by name (`import` is a prefix of `import_scan`) and by `use`
        assert!(got.contains(&module("import")));
        assert!(got.contains(&module("storage")));
        assert!(!got.contains(&module("undo")));
        assert!(!got.contains(&Scope::Binary(GUI_IT.into())));
        assert!(got.contains(&Scope::Package("helios-gui-agent".into())));
        assert!(!s.everything());
        let filter = s.filter();
        assert!(filter.contains("test(/^import::/)"), "{filter}");
        assert!(!filter.contains("package(=helios-core)"), "{filter}");

        // a shell file is mapped by its own name, a grouped `use` found
        let s = select(&input(
            vec![change("crates/helios-gui/src/shell/import_albums.rs")],
            missing(),
        ));
        let got = scopes(&s, Source::Affected);
        assert!(got.contains(&module("import_albums")));
        assert!(got.contains(&module("import")));
        assert!(!got.contains(&module("undo")));

        // the override table and the library suites `affected` knows
        let s = select(&input(
            vec![change("crates/helios-gui/src/library/view.rs")],
            missing(),
        ));
        assert!(scopes(&s, Source::Affected).contains(&module("culling")));

        // unsure: the whole `it`
        for path in [
            "crates/helios-gui/src/lib.rs",
            "crates/helios-gui/src/nobody_uses.rs",
        ] {
            let s = select(&input(vec![change(path)], missing()));
            assert!(
                scopes(&s, Source::Affected).contains(&Scope::Binary(GUI_IT.into())),
                "{path}"
            );
        }

        // a shared it helper selects the modules using it
        let s = select(&input(
            vec![change("crates/helios-gui/tests/it/support.rs")],
            missing(),
        ));
        assert!(scopes(&s, Source::Affected).contains(&module("persist")));
    }

    #[test]
    fn a_new_test_is_selected() {
        let source = "use x;\n\n#[test]\nfn old() {}\n\n#[test]\nfn brand_new() {\n    y();\n}\n";
        let edited = Change {
            path: "crates/helios-gui/tests/it/import.rs".into(),
            content: Some(source.into()),
            lines: Some(vec![(6, 9)]),
        };
        let added = Change {
            path: "crates/helios-library/tests/cull.rs".into(),
            content: Some(source.into()),
            lines: None,
        };
        let s = select(&input(vec![edited, added], missing()));
        let got = scopes(&s, Source::ChangedTest);
        assert_eq!(got.len(), 3, "{got:?}");
        assert!(
            s.explain("helios-gui::it import::brand_new")
                .unwrap()
                .iter()
                .any(|p| p.source == Source::ChangedTest)
        );
        assert!(
            !s.explain("helios-gui::it import::old")
                .unwrap()
                .iter()
                .any(|p| p.source == Source::ChangedTest)
        );
        assert!(
            s.explain("helios-library::cull old")
                .unwrap()
                .iter()
                .any(|p| p.source == Source::ChangedTest)
        );
        // the test file alone: its binary, not the crate or its dependents
        assert!(
            scopes(&s, Source::Affected).contains(&Scope::Binary("helios-library::cull".into()))
        );
        assert!(!scopes(&s, Source::Affected).contains(&Scope::Package("helios-library".into())));
    }

    #[test]
    fn a_cargo_lock_change_selects_everything() {
        for path in [
            "Cargo.lock",
            "xtask/src/main.rs",
            ".github/workflows/ci.yml",
            "somewhere/new.txt",
        ] {
            let s = select(&input(vec![change(path)], missing()));
            assert!(s.everything(), "{path}");
            assert_eq!(s.filter(), "all()");
        }
        // a shared crate selects every crate depending on it
        let s = select(&input(
            vec![change("crates/helios-core/src/graph.rs")],
            missing(),
        ));
        for c in [
            "helios-core",
            "helios-library",
            "helios-gui",
            "helios-gui-agent",
        ] {
            assert!(
                scopes(&s, Source::Affected).contains(&Scope::Package(c.into())),
                "{c}"
            );
        }
        // a doc selects nothing but the smoke set
        let s = select(&input(vec![change("docs/ci-broker.md")], missing()));
        assert_eq!(
            s.filter(),
            "(binary_id(=helios-core::engine) & test(=renders_a_chain))"
        );
    }

    #[test]
    fn a_flaky_test_in_the_history_is_selected() {
        let mut summary = Summary::default();
        summary.runs = 3;
        summary.ingested = ["1", "2", "3"].iter().map(|s| (*s).to_string()).collect();
        summary.tests.insert(
            "helios-gui::it undo::a_redo".into(),
            TestStats {
                last_flake: Some(Stamp {
                    run: "2".into(),
                    commit: "abc".into(),
                    date: "d".into(),
                }),
                ..TestStats::default()
            },
        );
        let s = select(&input(
            vec![change("docs/x.md")],
            History::Read(Box::new(summary)),
        ));
        let why = s.explain("helios-gui::it undo::a_redo").unwrap();
        assert_eq!(why.len(), 1);
        assert_eq!(why[0].source, Source::History);
        assert!(why[0].why.starts_with("flaked in run 2"));
        assert!(s.filter().contains("test(=undo::a_redo)"));
    }

    #[test]
    fn with_no_history_the_source_is_skipped_and_said() {
        let s = select(&input(vec![change("docs/x.md")], missing()));
        assert!(scopes(&s, Source::History).is_empty());
        assert!(
            s.summary().contains("note: skipped, no history yet"),
            "{}",
            s.summary()
        );
        // a history that does not read runs everything
        let s = select(&input(vec![], History::Unreadable("bad".into())));
        assert!(s.everything());
    }

    /// Each entry of the checked-in smoke list and of the override table
    /// still names a test that exists, so a rename cannot hollow them out.
    #[test]
    fn the_smoke_list_and_the_overrides_name_what_exists() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let text = std::fs::read_to_string(root.join(SMOKE)).unwrap();
        let smoke = parse_smoke(&text).unwrap();
        assert!((15..=30).contains(&smoke.len()), "{} entries", smoke.len());
        let file = |binary: &str, module: Option<&str>| {
            let (package, target) = binary.split_once("::").expect("an integration test");
            let dir = root.join("crates").join(package).join("tests");
            match (target, module) {
                ("it", Some(m)) => dir.join("it").join(format!("{m}.rs")),
                ("it", None) => dir.join("it/main.rs"),
                (t, _) => dir.join(format!("{t}.rs")),
            }
        };
        for scope in smoke {
            match scope {
                Scope::Test { binary, name } => {
                    let (module, f) = match name.rsplit_once("::") {
                        Some((m, f)) => (Some(m), f),
                        None => (None, name.as_str()),
                    };
                    let path = file(&binary, module);
                    let src = std::fs::read_to_string(&path)
                        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                    assert!(src.contains(&format!("fn {f}(")), "{binary} {name}");
                }
                Scope::Module { binary, module } => {
                    assert!(file(&binary, Some(&module)).exists(), "{binary} {module}")
                }
                Scope::Binary(binary) => assert!(file(&binary, None).exists(), "{binary}"),
                other => panic!("{other:?}"),
            }
        }
        for (prefixes, modules) in OVERRIDES {
            for p in *prefixes {
                let p = root.join(p);
                assert!(
                    p.exists() || p.with_extension("rs").exists(),
                    "{}",
                    p.display()
                );
            }
            for m in *modules {
                assert!(
                    root.join(GUI_IT_DIR).join(format!("{m}.rs")).exists(),
                    "{m}"
                );
            }
        }
    }
}
