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
//!    - A feature crate cut out of helios-gui ([`FEATURES`]) stops at the
//!      app crate: an edit inside it that leaves its contract alone (its
//!      public API, its bus points: `ci_broker/contract.rs`) selects the
//!      crate and the helios-gui tests exercising it, not all of
//!      helios-gui and its dependents. A contract edit, its manifest, data
//!      or a crate depending on it beside helios-gui select as before.
//! 2. **changed tests**: the `#[test]` functions the diff adds or edits.
//! 3. **recent failures**: the tests that failed or flaked in the last N
//!    runs of the `ci-history` summary; skipped, and said, while there is
//!    no history.
//! 4. **smoke**: the list in `ci/smoke.txt`.
//!
//! The filter goes to stdout, the summary per source to stderr.
//! `--explain '<binary id> <test name>'` says why one test runs.
//!
//! The shadow week (step B3): `--committed --shadow FILE --run ID --commit
//! SHA --junit FILE` reads the commits since `--base` only (CI's working
//! tree holds untracked checkouts), and writes the `broker-shadow`
//! artifact: the tests of the run's JUnit report the set holds, with the
//! changed files and every pick (`ci_broker/shadow.rs`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::ExitCode;

use crate::affected::{self, Graph, Rule};
use crate::ci_broker::history::Summary;
use crate::ci_broker::select::{
    Scope, Selection, Source, hunks, parse_smoke, recent_failures, test_scope, touched_tests,
};
use crate::ci_broker::shadow::artifact_json;

pub const USAGE: &str = "ci-broker select [--base REF] [--history DIR] [--runs N] [--committed | --head REF] [--explain '<binary id> <test>' | --shadow FILE --run ID --commit SHA --junit FILE]";

/// The base a change is computed against: pull requests go into `next`.
pub const DEFAULT_BASE: &str = "origin/next";
/// How many of the last runs the history source reads.
pub const DEFAULT_RUNS: usize = 20;
/// A test of the history is current when one of its last N runs listed
/// it: the measure of a set counts those.
pub const CURRENT_RUNS: u64 = 10;
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

/// A feature crate: cut out of helios-gui, it reaches the desktop
/// through the bus and its contract tables alone (docs/crates.md), and
/// helios-gui mounts it.
pub struct Feature {
    /// The crate.
    pub name: &'static str,
    /// The helios-gui sources that mount it: the `it` modules exercising
    /// them ([`gui_modules`]) exercise the feature, and their unit tests
    /// run too.
    pub mounts: &'static [&'static str],
    /// `it` modules exercising it that neither the mounts nor the crate's
    /// name find (an override line, as in [`OVERRIDES`]).
    pub modules: &'static [&'static str],
}

/// The feature crates. An edit inside one that leaves its contract alone
/// selects the crate, the `it` modules exercising it and its mounts' unit
/// tests: it stops at the app crate. Add a line when a feature leaves
/// helios-gui; a crate missing here selects helios-gui and every crate
/// depending on it, as any crate does.
pub const FEATURES: &[Feature] = &[Feature {
    name: "helios-storage-ui",
    mounts: &["crates/helios-gui/src/shell/storage.rs"],
    modules: &[],
}];

/// One changed file: its path, its content now (`None`: deleted), its
/// content at the base (`None`: new, or not read) and the lines the diff
/// touched in it (`None`: the whole file is new).
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub path: String,
    pub content: Option<String>,
    pub old: Option<String>,
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
    /// Per feature crate, the names helios-gui re-exports from it
    /// ([`reexports`]): an `it` module naming one uses the feature.
    pub reexported: BTreeMap<String, BTreeSet<String>>,
    /// Per feature crate, its public string ids ([`string_ids`]): its
    /// commands and bus points. An `it` module quoting one uses the
    /// feature through the bus.
    pub ids: BTreeMap<String, BTreeSet<String>>,
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

fn is_rust(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|e| e == "rs")
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

/// The names a source re-exports from the crate `ident` (`pub use
/// ident::…;`), as the importer sees them: an alias when there is one,
/// else the path's last segment.
pub fn reexports(source: &str, ident: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for span in source.split("pub use ").skip(1) {
        let span = span.split(';').next().unwrap_or("");
        let Some(rest) = span.trim_start().strip_prefix(ident) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix("::") else {
            continue;
        };
        for piece in rest.split(['{', '}', ',']) {
            let piece = piece.trim();
            let name = match piece.split_once(" as ") {
                Some((_, alias)) => alias.trim(),
                None => piece.rsplit("::").next().unwrap_or("").trim(),
            };
            if !name.is_empty() && name != "self" && name != "_" {
                out.insert(name.to_string());
            }
        }
    }
    out
}

/// The values of a source's public string constants (`pub const X:
/// &str = "…";`): a feature's command names and bus point ids.
pub fn string_ids(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("pub const ")?;
            let (_, value) = rest.split_once(": &str = \"")?;
            let value = value.strip_suffix("\";")?;
            (!value.is_empty()).then(|| value.to_string())
        })
        .collect()
}

/// The tests an edit inside a feature crate selects when it leaves the
/// crate's contract alone; `Err(why)` when it may not stop there.
fn feature_edit(
    input: &Input,
    feature: &Feature,
    change: &Change,
    rest: &str,
) -> Result<Vec<(Scope, String)>, String> {
    let path = change.path.as_str();
    if !(rest.starts_with("src/") && is_rust(path)) {
        return Err("not a source file of the crate".into());
    }
    let others: Vec<&str> = input
        .graph
        .crates
        .iter()
        .filter(|c| c.name != GUI && c.deps.iter().any(|d| d == feature.name))
        .map(|c| c.name.as_str())
        .collect();
    if !others.is_empty() {
        return Err(format!("{} depend on it too", others.join(", ")));
    }
    if change.old.is_none() && change.content.is_none() {
        return Err("neither side of the file reads".into());
    }
    if crate::ci_broker::contract::changed(change.old.as_deref(), change.content.as_deref()) {
        return Err("its contract (public API or bus points) changed".into());
    }
    let mut out = vec![(
        Scope::Package(feature.name.into()),
        format!("{path} changed (contract unchanged)"),
    )];
    let mut modules: BTreeMap<String, String> = BTreeMap::new();
    for mount in feature.mounts {
        for (m, why) in gui_modules(mount, &input.gui_suites) {
            modules
                .entry(m)
                .or_insert_with(|| format!("exercises its mount {mount}: {why}"));
        }
        if let Some((unit, _)) = gui_unit(mount) {
            out.push((
                Scope::Module {
                    binary: GUI.into(),
                    module: unit,
                },
                format!("{path} changed: the unit tests of its mount {mount}"),
            ));
        }
    }
    let ident = feature.name.replace('-', "_");
    for (m, source) in &input.gui_suites {
        if idents(source).any(|w| w == ident) {
            modules
                .entry(m.clone())
                .or_insert_with(|| format!("names {ident}"));
        }
    }
    let names = input.reexported.get(feature.name);
    for (m, source) in &input.gui_suites {
        if let Some(name) = idents(source).find(|w| names.is_some_and(|n| n.contains(*w))) {
            modules
                .entry(m.clone())
                .or_insert_with(|| format!("uses {name}, re-exported from {ident}"));
        }
    }
    let ids = input.ids.get(feature.name);
    for (m, source) in &input.gui_suites {
        if let Some(id) =
            ids.and_then(|ids| ids.iter().find(|id| source.contains(&format!("\"{id}\""))))
        {
            modules
                .entry(m.clone())
                .or_insert_with(|| format!("quotes `{id}`, an id of {}", feature.name));
        }
    }
    for m in feature.modules {
        modules
            .entry((*m).to_string())
            .or_insert_with(|| "override for the feature".into());
    }
    if modules.is_empty() {
        out.push((
            Scope::Binary(GUI_IT.into()),
            format!("{path} changed: no it module known for {}", feature.name),
        ));
    }
    for (module, why) in modules {
        out.push((
            Scope::Module {
                binary: GUI_IT.into(),
                module,
            },
            format!("{path} changed: {why}"),
        ));
    }
    Ok(out)
}

/// Source 1, for one file.
fn affected_file(input: &Input, change: &Change, s: &mut Selection) {
    let path = change.path.as_str();
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
            [file] if is_rust(file) => {
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
    let mut why = format!("{path} changed");
    if let Some(feature) = FEATURES.iter().find(|f| f.name == name) {
        match feature_edit(input, feature, change, rest) {
            Ok(picks) => {
                for (scope, why) in picks {
                    s.add(src, scope, why);
                }
                return;
            }
            Err(e) => {
                why =
                    format!("{path} changed, a feature crate whose edit may reach helios-gui: {e}")
            }
        }
    }
    whole(s, &[name], &why);
}

/// Source 2, for one file.
fn changed_tests(input: &Input, change: &Change, s: &mut Selection) {
    let (Some(content), true) = (&change.content, is_rust(&change.path)) else {
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
        affected_file(input, change, &mut s);
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

/// The input from git, the working tree and the history. `at`: the
/// commits up to that ref alone, read from git, not the working tree (a
/// past commit measured); `committed`: the commits up to `HEAD`, the
/// files read from the working tree.
pub fn gather(
    root: &Path,
    base: &str,
    history: Option<&Path>,
    runs: usize,
    committed: bool,
    at: Option<&str>,
) -> Result<Input, String> {
    let tip = at.unwrap_or("HEAD");
    let committed = committed || at.is_some();
    let merge_base = affected::git(root, &["merge-base", base, tip])?
        .into_iter()
        .next()
        .ok_or_else(|| format!("no merge base between {base} and {tip}"))?;
    // `--committed`: the commits alone, not the working tree nor the
    // untracked files (CI's checkout holds a few: vkdt, reports).
    let head_arg = [tip];
    let head: &[&str] = if committed { &head_arg } else { &[] };
    let paths = if committed {
        affected::git(
            root,
            &["diff", "--name-only", "--no-renames", &merge_base, tip],
        )?
    } else {
        affected::changed_paths(root, base)?
    };
    let diff = std::process::Command::new("git")
        .args(["diff", "-U0", "--no-renames", "--no-color", &merge_base])
        .args(head)
        .args(["--", "*.rs"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let touched: BTreeMap<String, Vec<(usize, usize)>> =
        hunks(&String::from_utf8_lossy(&diff.stdout))
            .into_iter()
            .collect();
    let graph = affected::workspace(root)?;
    let show = |rev: &str, path: &str| {
        std::process::Command::new("git")
            .args(["show", &format!("{rev}:{path}")])
            .current_dir(root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let changes = paths
        .into_iter()
        .map(|path| {
            let content = match at {
                Some(at) => show(at, &path),
                None => std::fs::read_to_string(root.join(&path)).ok(),
            };
            // the base side, for the contract of a feature crate's file
            let in_feature = graph
                .owner(&path)
                .is_some_and(|c| FEATURES.iter().any(|f| f.name == c.name));
            let old = if in_feature && is_rust(&path) {
                show(&merge_base, &path)
            } else {
                None
            };
            // an untracked file is not in the diff: all of it is new
            let lines = touched.get(&path).cloned();
            Change {
                path,
                content,
                old,
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
    let mut reexported: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in rust_files(&root.join(GUI_DIR).join("src"))? {
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        for f in FEATURES {
            let names = reexports(&text, &f.name.replace('-', "_"));
            reexported.entry(f.name.into()).or_default().extend(names);
        }
    }
    let mut ids: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for f in FEATURES {
        let Some(c) = graph.crates.iter().find(|c| c.name == f.name) else {
            continue;
        };
        for path in rust_files(&root.join(&c.dir).join("src"))? {
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            ids.entry(f.name.into())
                .or_default()
                .extend(string_ids(&text));
        }
    }
    let history = read_history(root, history);
    let smoke_path = root.join(SMOKE);
    let smoke = std::fs::read_to_string(&smoke_path)
        .map_err(|e| format!("{SMOKE}: {e}"))
        .and_then(|t| parse_smoke(&t).map_err(|e| format!("{SMOKE}: {e}")))?;
    Ok(Input {
        graph,
        changes,
        gui_suites,
        reexported,
        ids,
        history,
        runs,
        smoke,
    })
}

/// The `.rs` files under `dir`, at any depth.
fn rust_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for entry in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                todo.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    Ok(out)
}

fn read_history(root: &Path, dir: Option<&Path>) -> History {
    let text = if let Some(dir) = dir {
        match std::fs::read_to_string(dir.join("summary.json")) {
            Ok(t) => t,
            Err(e) => return History::Missing(format!("{}/summary.json: {e}", dir.display())),
        }
    } else {
        let out = std::process::Command::new("git")
            .args(["show", "origin/ci-history:summary.json"])
            .current_dir(root)
            .output();
        let Some(out) = out.ok().filter(|o| o.status.success()) else {
            return History::Missing(
                "no summary.json on origin/ci-history (`git fetch origin ci-history`)".into(),
            );
        };
        String::from_utf8_lossy(&out.stdout).into_owned()
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
    let (mut committed, mut shadow, mut run_id, mut commit, mut junit) =
        (false, None, None, None, None);
    let mut at = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--base" => base = *it.next()?,
            "--history" => history = Some(Path::new(*it.next()?)),
            "--runs" => runs = it.next()?.parse().ok()?,
            "--explain" => explain = Some(*it.next()?),
            "--committed" => committed = true,
            "--head" => at = Some(*it.next()?),
            "--shadow" => shadow = Some(Path::new(*it.next()?)),
            "--run" => run_id = Some(*it.next()?),
            "--commit" => commit = Some(*it.next()?),
            "--junit" => junit = Some(Path::new(*it.next()?)),
            _ => return None,
        }
    }
    // The shadow artifact needs its run, commit and JUnit report.
    let shadow = match shadow {
        Some(file) => Some((file, run_id?, commit?, junit?)),
        None => None,
    };
    let input = match gather(root, base, history, runs, committed, at) {
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
    if let History::Read(summary) = &input.history {
        let (held, current) = selection.held_of(summary, CURRENT_RUNS);
        eprintln!(
            "measured: {held} of the {current} tests of a full run (listed in the history's last {CURRENT_RUNS} runs)"
        );
    }
    println!("{}", selection.filter());
    if let Some((file, run_id, commit, junit)) = shadow {
        let changed: Vec<String> = input.changes.iter().map(|c| c.path.clone()).collect();
        let written = std::fs::read_to_string(junit)
            .map_err(|e| format!("{}: {e}", junit.display()))
            .and_then(|xml| crate::test_times::parse_junit(&xml))
            .and_then(|report| {
                let tests: Vec<String> = report
                    .cases
                    .iter()
                    .map(|c| format!("{} {}", c.binary, c.name))
                    .collect();
                let json = artifact_json(run_id, commit, base, &selection, &changed, &tests);
                std::fs::write(file, json).map_err(|e| format!("{}: {e}", file.display()))?;
                Ok(tests.len())
            });
        match written {
            Ok(n) => eprintln!("broker shadow: {} ({n} tests listed)", file.display()),
            Err(e) => return Some(crate::fail(&[format!("ci-broker select --shadow: {e}")])),
        }
    }
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
                c("helios-storage-ui", &["helios-core", "helios-library"]),
                c(
                    "helios-gui",
                    &["helios-core", "helios-library", "helios-storage-ui"],
                ),
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
            s("health", "use helios_storage_ui::backup_end::HEALTH;\n"),
            s("volumes", "let c = helios_gui::shell::STORAGE_COMMAND;\n"),
            s(
                "import_menu",
                "shell.run(\"library.storage\", Value::Null);\n",
            ),
            s("undo", "use helios_gui::shell::App;\n"),
        ])
    }

    fn input(changes: Vec<Change>, history: History) -> Input {
        Input {
            graph: graph(),
            changes,
            gui_suites: suites(),
            reexported: BTreeMap::from([(
                "helios-storage-ui".to_string(),
                reexports(
                    "pub use helios_storage_ui::{COMMAND as STORAGE_COMMAND, NodeStorage};\n\
                     pub use helios_storage_ui::read as storage_read;\n",
                    "helios_storage_ui",
                ),
            )]),
            ids: BTreeMap::from([(
                "helios-storage-ui".to_string(),
                string_ids(
                    "pub const COMMAND: &str = \"library.storage\";\nconst NOT: &str = \"x\";\n",
                ),
            )]),
            history,
            runs: DEFAULT_RUNS,
            smoke: parse_smoke("helios-core::engine renders_a_chain\n").unwrap(),
        }
    }

    fn change(path: &str) -> Change {
        Change {
            path: path.into(),
            content: Some(String::new()),
            old: Some(String::new()),
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

    fn edited(path: &str, old: &str, new: &str) -> Change {
        Change {
            path: path.into(),
            content: Some(new.into()),
            old: Some(old.into()),
            lines: Some(vec![(2, 2)]),
        }
    }

    const PAGE: &str = "pub fn rows(n: u32) -> u32 {\n    n + 1\n}\n";

    #[test]
    fn an_internal_edit_in_a_feature_crate_stops_at_the_app_crate() {
        let path = "crates/helios-storage-ui/src/page.rs";
        let body = PAGE.replace("n + 1", "n + 2");
        let s = select(&input(vec![edited(path, PAGE, &body)], missing()));
        let got = scopes(&s, Source::Affected);
        // the crate's own tests
        assert!(got.contains(&Scope::Package("helios-storage-ui".into())));
        // the `it` modules exercising it: its mount by name, its name
        assert!(got.contains(&module("storage")), "{got:?}");
        assert!(got.contains(&module("health")), "{got:?}");
        // and through what helios-gui re-exports from it
        assert!(got.contains(&module("volumes")), "{got:?}");
        // and through its ids on the bus
        assert!(got.contains(&module("import_menu")), "{got:?}");
        // the unit tests of its mount in helios-gui
        assert!(got.contains(&Scope::Module {
            binary: GUI.into(),
            module: "shell::storage".into(),
        }));
        // not all of helios-gui, nor what depends on it
        for wide in [
            Scope::Package(GUI.into()),
            Scope::Binary(GUI_IT.into()),
            Scope::Package("helios-gui-agent".into()),
        ] {
            assert!(!got.contains(&wide), "{wide:?}");
        }
        assert!(!got.contains(&module("undo")));
        assert!(
            !s.filter().contains("package(=helios-gui)"),
            "{}",
            s.filter()
        );
        assert!(
            s.explain("helios-gui::it storage::a_volume")
                .unwrap()
                .iter()
                .any(|p| p.why.contains("contract unchanged") || p.why.contains("mount"))
        );
    }

    #[test]
    fn a_contract_edit_in_a_feature_crate_selects_helios_gui() {
        let points =
            "pub const POINTS: &[PointDecl] = &[\n    PointDecl::of::<Read>(\"a read\"),\n];\n";
        let wide = |c: Change| {
            let s = select(&input(vec![c], missing()));
            let got = scopes(&s, Source::Affected);
            for c in ["helios-storage-ui", "helios-gui", "helios-gui-agent"] {
                assert!(got.contains(&Scope::Package(c.into())), "{c}: {got:?}");
            }
            s
        };
        // a bus point declared in its contract table
        let s = wide(edited(
            "crates/helios-storage-ui/src/read.rs",
            points,
            &points.replace("),\n", "),\n    PointDecl::of::<Other>(\"b\"),\n"),
        ));
        assert!(
            s.picks.iter().any(|p| p
                .why
                .contains("contract (public API or bus points) changed")),
            "{}",
            s.summary()
        );
        // its public API
        wide(edited(
            "crates/helios-storage-ui/src/page.rs",
            PAGE,
            &PAGE.replace("n: u32", "n: u64"),
        ));
        // a new file, its manifest, its data: unsure, so wide
        wide(Change {
            old: None,
            content: Some(PAGE.into()),
            ..edited("crates/helios-storage-ui/src/more.rs", "", "")
        });
        wide(change("crates/helios-storage-ui/Cargo.toml"));
        // another crate depending on it: not a feature of helios-gui alone
        let mut i = input(
            vec![edited(
                "crates/helios-storage-ui/src/page.rs",
                PAGE,
                &PAGE.replace("n + 1", "n + 2"),
            )],
            missing(),
        );
        i.graph.crates.push(Crate {
            name: "helios-node".into(),
            dir: "crates/helios-node".into(),
            deps: vec!["helios-storage-ui".into()],
        });
        let s = select(&i);
        assert!(scopes(&s, Source::Affected).contains(&Scope::Package(GUI.into())));
    }

    #[test]
    fn re_exports_are_read_by_the_names_the_importer_sees() {
        let src = "pub use helios_storage_ui::backup_end;\n\
                   pub use helios_storage_ui::admin::{\n    MARK as MARK_COMMAND, rows,\n};\n\
                   pub use helios_storage_ui::{a::{b, c as d}, self};\n\
                   pub use helios_storage::Other;\nuse helios_storage_ui::Private;\n";
        let got: Vec<String> = reexports(src, "helios_storage_ui").into_iter().collect();
        assert_eq!(got, ["MARK_COMMAND", "b", "backup_end", "d", "rows"]);
    }

    /// Each feature crate exists, helios-gui alone depends on it, and its
    /// mounts and override modules exist.
    #[test]
    fn the_feature_table_names_what_exists() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let graph = affected::workspace(&root).unwrap();
        for f in FEATURES {
            assert!(graph.crates.iter().any(|c| c.name == f.name), "{}", f.name);
            let users: Vec<&str> = graph
                .crates
                .iter()
                .filter(|c| c.deps.iter().any(|d| d == f.name))
                .map(|c| c.name.as_str())
                .collect();
            assert_eq!(users, [GUI], "{}", f.name);
            for m in f.mounts {
                assert!(root.join(m).exists(), "{m}");
                assert!(gui_unit(m).is_some(), "{m}");
            }
            for m in f.modules {
                assert!(
                    root.join(GUI_IT_DIR).join(format!("{m}.rs")).exists(),
                    "{m}"
                );
            }
        }
    }

    #[test]
    fn a_new_test_is_selected() {
        let source = "use x;\n\n#[test]\nfn old() {}\n\n#[test]\nfn brand_new() {\n    y();\n}\n";
        let edited = Change {
            path: "crates/helios-gui/tests/it/import.rs".into(),
            content: Some(source.into()),
            old: None,
            lines: Some(vec![(6, 9)]),
        };
        let added = Change {
            path: "crates/helios-library/tests/cull.rs".into(),
            content: Some(source.into()),
            old: None,
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
        let mut summary = Summary {
            runs: 3,
            ingested: ["1", "2", "3"].iter().map(|s| (*s).to_string()).collect(),
            ..Summary::default()
        };
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
                    assert!(file(&binary, Some(&module)).exists(), "{binary} {module}");
                }
                Scope::Binary(binary) => {
                    assert!(file(&binary, None).exists(), "{binary}");
                }
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
