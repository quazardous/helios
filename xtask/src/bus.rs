//! `bus --check`: no Helios library crate constructs the bus it declares
//! points or listens on. The bus is a contract (`bus-contract`) the
//! application supplies: the desktop's and the node's `main` build one and
//! hand it to egui-pop (`App::builder().bus`) and to each Helios crate at
//! construction (egui-pop's ADR 0030, "the bus is a contract the
//! application supplies").
//!
//! A construction is `Kernel::new(`, `Kernel::with_default_deadline(` or
//! `Kernel::default(` in a crate's `src/` (the workspace's `crates/`,
//! `modules/` and `plugins/`), outside comments. The binaries
//! (`src/main.rs`, `src/bin/`), the tests (`tests/`, an inline test module
//! from its top-level `#[cfg(test)]` to the end of its file, the test kit
//! crate) may: they are applications, or stand for one.
//!
//! It also checks that every typed fact of the node's fact bus
//! (`helios_core::Bus`) is declared: each `impl Event for T` outside tests
//! has its `FactDecl::of::<T>` in a contract table. A kind published as
//! JSON is checked when it is published, on a bus that declared its
//! tables (`Bus::declare`).
//!
//! And it holds the desktop to its listener tables: a hand-rolled
//! subscription to a fact bus (`.subscribe()`, `.subscribe_kinds(`) in
//! `crates/helios-gui/src`, outside comments and tests, is in a file
//! listed in `xtask/allow/bus.toml` with its reason. The desktop's
//! followers of node facts are named listeners of the application bus's
//! points, or of the node listener table (`node_events`); the list holds
//! the documented exceptions and only shrinks (an entry whose file no
//! longer subscribes is reported).
//!
//! And it holds Helios's points to the bus-contract naming rule (its
//! README, "Naming points"): every point a Helios crate declares
//! (`impl Point for T` with its `const ID`) is under `helios.`, so
//! egui-pop's `pop.*` and `egui_stack`'s `stack.*` share the bus with it.
//! A point named after a node fact kind (`const ID = E::KIND`) is held to
//! it through the kind, below. The listeners are
//! held to it by helios-gui's `every_point_and_listener_of_helios_has_its_prefix`,
//! on the bus the desktop builds.
//!
//! So are the fact kinds (`helios_core::FactDecl`): each kind a crate
//! declares, the `const KIND` of an `impl Event for T` or the first
//! argument of `FactDecl::json(` / `FactDecl::typed::<T>(` (a literal, or
//! a `const` of the crate, by name), is under `helios.`. A node of an
//! older release names them without it; the relay maps those names
//! (`helios_core::legacy_kinds`), the bus never holds one.
//!
//! `bus --timings <file>` reads the bus debug collector saved by a run
//! (the journey's `bus-timings-*.json`) and prints, per point, its passes
//! (count, p50, p95, max), its budget and the passes over it, and its
//! slowest listeners: "who costs what". It reports and never fails on a
//! budget (no gate).

use std::fs;
use std::path::Path;
use std::process::ExitCode;

pub const USAGE: &str = "bus --check | bus --timings <file>";

/// How many listeners `--timings` names per point.
const SLOWEST: usize = 3;

/// The folders holding the workspace's crates, from the root.
const FOLDERS: &[&str] = &["crates", "modules", "plugins"];

/// Crates that stand for an application in tests: they may build a bus.
const TEST_CRATES: &[&str] = &["crates/helios-testkit/"];

/// The desktop's sources, held to their listener tables.
const DESKTOP: &str = "crates/helios-gui/src";

/// The prefix of every point Helios declares.
pub const PREFIX: &str = "helios.";

/// The desktop's exceptions, relative to the workspace root.
pub const ALLOW_LIST: &str = "xtask/allow/bus.toml";

/// A hand-rolled subscription to a fact bus.
const SUBSCRIPTIONS: &[&str] = &[".subscribe()", ".subscribe_kinds("];

const CONSTRUCTORS: &[&str] = &[
    "Kernel::new(",
    "Kernel::with_default_deadline(",
    "Kernel::default(",
];

/// Runs the check: `cargo xtask bus --check`.
pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    if let ["--timings", file] = args {
        return Some(timings(Path::new(file)));
    }
    if args != ["--check"] {
        return None;
    }
    let mut found = problems(root);
    let subscriptions = subscriptions(root);
    found.extend(judge_subscriptions(root, &subscriptions));
    let named = point_names(root);
    found.extend(named.problems.iter().cloned());
    let kinds = fact_kinds(root);
    found.extend(kinds.problems.iter().cloned());
    let facts = facts(root);
    found.extend(
        facts
            .undeclared
            .iter()
            .map(|e| format!("{e}: a fact in no FACTS table")),
    );
    if found.is_empty() {
        println!("no library crate constructs a bus (the application supplies it)");
        println!(
            "points declared under `{PREFIX}`: {} of {}",
            named.points.len(),
            named.points.len()
        );
        println!(
            "fact kinds named under `{PREFIX}`: {} of {}",
            kinds.kinds.len(),
            kinds.kinds.len()
        );
        println!(
            "typed fact kinds declared in a contract table: {} of {}",
            facts.events.len(),
            facts.events.len()
        );
        let files: std::collections::BTreeSet<&str> = subscriptions
            .iter()
            .map(|s| s.split(':').next().unwrap_or(""))
            .collect();
        println!(
            "hand-rolled bus subscriptions on the desktop: {} in {} files, each listed in {ALLOW_LIST}",
            subscriptions.len(),
            files.len()
        );
        return Some(ExitCode::SUCCESS);
    }
    for f in &found {
        eprintln!("bus: {f}");
    }
    eprintln!(
        "bus: {} problem(s): a bus built in a library crate (take it from the application), \
         a fact no table declares (add its `FactDecl::of`), a point or a fact kind \
         without its `{PREFIX}` prefix, or a desktop subscription \
         to a bus outside {ALLOW_LIST} (listen on the point or the node table instead)",
        found.len()
    );
    Some(ExitCode::from(1))
}

/// `bus --timings <file>`: the report, or why the file cannot be read.
fn timings(file: &Path) -> ExitCode {
    let read = fs::read_to_string(file)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string()));
    match read {
        Ok(snapshot) => {
            print!("{}", timings_report(&snapshot));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("bus --timings: {}: {e}", file.display());
            ExitCode::from(2)
        }
    }
}

/// A duration in µs as the report writes it (`850 µs`, `4.2 ms`).
fn time(us: u64) -> String {
    if us < 1000 {
        format!("{us} µs")
    } else {
        format!("{:.1} ms", us as f64 / 1000.0)
    }
}

/// The report of a bus collector's snapshot (`{"points": [...]}`, or the
/// whole debug snapshot holding it under `bus`): each point that ran a
/// pass, the slowest first by its passes' p95, with its budget, the
/// passes over it and its slowest listeners; then the points over budget.
pub fn timings_report(snapshot: &serde_json::Value) -> String {
    use std::fmt::Write as _;
    let bus = snapshot.get("bus").unwrap_or(snapshot);
    let u =
        |v: &serde_json::Value, k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
    let mut points: Vec<&serde_json::Value> = bus
        .get("points")
        .and_then(serde_json::Value::as_array)
        .map(|p| p.iter().filter(|p| p["timing"].is_object()).collect())
        .unwrap_or_default();
    let mut out = String::new();
    if points.is_empty() {
        out.push_str("no timings: the bus offers none, or no point ran a pass\n");
        return out;
    }
    points.sort_by_key(|p| {
        (
            std::cmp::Reverse(u(&p["timing"], "p95_us")),
            p["point"].to_string(),
        )
    });
    let mut over = Vec::new();
    for p in &points {
        let t = &p["timing"];
        let name = p["point"].as_str().unwrap_or("?");
        let budget = p.get("budget_us").and_then(serde_json::Value::as_u64);
        let _ = write!(
            out,
            "{name}: {} passes, p50 {}, p95 {}, max {}",
            u(t, "count"),
            time(u(t, "p50_us")),
            time(u(t, "p95_us")),
            time(u(t, "max_us")),
        );
        if let Some(b) = budget {
            let n = u(t, "over_budget");
            let _ = write!(out, "; budget {}, {n} over", time(b));
            if n > 0 {
                over.push(format!(
                    "{name} ({n} of {} passes over {})",
                    u(t, "count"),
                    time(b)
                ));
            }
        }
        out.push('\n');
        let mut listeners: Vec<&serde_json::Value> = p["listeners"]
            .as_array()
            .map(|l| l.iter().filter(|l| l["timing"].is_object()).collect())
            .unwrap_or_default();
        listeners.sort_by_key(|l| {
            (
                std::cmp::Reverse(u(&l["timing"], "p95_us")),
                std::cmp::Reverse(u(&l["timing"], "max_us")),
            )
        });
        for l in listeners.iter().take(SLOWEST) {
            let t = &l["timing"];
            let _ = writeln!(
                out,
                "  {}: {} runs, p95 {}, max {}",
                l["id"].as_str().unwrap_or("?"),
                u(t, "count"),
                time(u(t, "p95_us")),
                time(u(t, "max_us")),
            );
        }
    }
    if over.is_empty() {
        out.push_str("no point went over its budget\n");
    } else {
        let _ = writeln!(out, "over budget: {}", over.join(", "));
    }
    out
}

/// Every construction outside the allowed places, as `path:line: text`;
/// a folder that cannot be read is one too.
pub fn problems(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for folder in FOLDERS {
        let dir = root.join(folder);
        if dir.is_dir()
            && let Err(e) = crates_in(root, &dir, &mut found)
        {
            found.push(e);
        }
    }
    found
}

/// The crates under `dir` (a crate is a folder with a `Cargo.toml`), at
/// any depth (`plugins/<category>/<name>`), each `src/` scanned.
fn crates_in(root: &Path, dir: &Path, found: &mut Vec<String>) -> Result<(), String> {
    for path in sorted(dir)? {
        if !path.is_dir() || path.ends_with("target") {
            continue;
        }
        if path.join("Cargo.toml").is_file() {
            let src = path.join("src");
            if src.is_dir() {
                scan(root, &src, found)?;
            }
        } else {
            crates_in(root, &path, found)?;
        }
    }
    Ok(())
}

fn sorted(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    Ok(entries)
}

fn scan(root: &Path, dir: &Path, found: &mut Vec<String>) -> Result<(), String> {
    for path in sorted(dir)? {
        if path.is_dir() {
            if !path.ends_with("bin") {
                scan(root, &path, found)?;
            }
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if rel.ends_with("/src/main.rs") || TEST_CRATES.iter().any(|t| rel.starts_with(t)) {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|e| format!("{rel}: {e}"))?;
        found.extend(constructions(&rel, &text));
    }
    Ok(())
}

/// The desktop's hand-rolled subscriptions to a bus, as
/// `path:line: text`, comments and inline test modules left out.
pub fn subscriptions(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let dir = root.join(DESKTOP);
    if sources(&dir, &mut files).is_err() {
        return vec![format!("{DESKTOP}: cannot be read")];
    }
    let mut found = Vec::new();
    for path in files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        found.extend(subscriptions_in(&rel, &text));
    }
    found
}

/// One file's subscriptions to a bus, up to its inline test module.
pub fn subscriptions_in(rel: &str, text: &str) -> Vec<String> {
    text.lines()
        .enumerate()
        .take_while(|(_, line)| !line.starts_with("#[cfg(test)]"))
        .filter(|(_, line)| {
            let code = line.split("//").next().unwrap_or("");
            SUBSCRIPTIONS.iter().any(|s| code.contains(s))
        })
        .map(|(n, line)| format!("{rel}:{}: {}", n + 1, line.trim()))
        .collect()
}

/// What the allow-list says of `found`: a subscription in a file it does
/// not list, an entry whose file no longer subscribes, or the list
/// unreadable.
fn judge_subscriptions(root: &Path, found: &[String]) -> Vec<String> {
    let allow = match fs::read_to_string(root.join(ALLOW_LIST))
        .map_err(|e| e.to_string())
        .and_then(|t| crate::vault_access::parse_allow_list(&t))
    {
        Ok(list) => list,
        Err(e) => return vec![format!("{ALLOW_LIST}: {e}")],
    };
    let file_of = |s: &String| s.split(':').next().unwrap_or("").to_string();
    let mut problems: Vec<String> = found
        .iter()
        .filter(|s| !allow.iter().any(|a| a.path == file_of(s)))
        .map(|s| format!("{s}: a subscription to a bus no listener table holds"))
        .collect();
    problems.extend(
        allow
            .iter()
            .filter(|a| !found.iter().any(|s| file_of(s) == a.path))
            .map(|a| {
                format!(
                    "{ALLOW_LIST}: `{}` no longer subscribes to a bus: remove its entry",
                    a.path
                )
            }),
    );
    problems
}

/// The typed facts of the workspace and those no table declares.
#[derive(Debug, Default)]
pub struct Facts {
    /// Every `impl Event for T`, as `path:line: T`.
    pub events: Vec<String>,
    /// Those with no `FactDecl::of::<T>` anywhere.
    pub undeclared: Vec<String>,
}

/// Reads every crate's `src/` (binaries included, tests left out) for
/// typed facts and the declarations of them.
pub fn facts(root: &Path) -> Facts {
    let mut events = Vec::new();
    let mut declared = std::collections::BTreeSet::new();
    let mut files = Vec::new();
    for folder in FOLDERS {
        let dir = root.join(folder);
        if dir.is_dir() {
            let _ = sources(&dir, &mut files);
        }
    }
    for path in files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if !rel.contains("/src/") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let (found, decls) = facts_in(&rel, &text);
        events.extend(found);
        declared.extend(decls);
    }
    let undeclared = events
        .iter()
        .filter(|e| {
            let ty = e.rsplit(' ').next().unwrap_or("");
            !declared.contains(ty)
        })
        .cloned()
        .collect();
    Facts { events, undeclared }
}

/// Every `.rs` file under `dir`, `target/` left out.
fn sources(dir: &Path, files: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    for path in sorted(dir)? {
        if path.is_dir() {
            if !path.ends_with("target") && !path.ends_with("tests") {
                sources(&path, files)?;
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

/// One file's typed facts (`path:line: T` for each `impl Event for T`)
/// and the types its `FactDecl::of::<T>` declare, up to its inline test
/// module.
pub fn facts_in(rel: &str, text: &str) -> (Vec<String>, Vec<String>) {
    let mut events = Vec::new();
    let mut declared = Vec::new();
    for (n, line) in text
        .lines()
        .enumerate()
        .take_while(|(_, line)| !line.starts_with("#[cfg(test)]"))
    {
        let code = line.split("//").next().unwrap_or("").trim();
        if code.starts_with("impl")
            && let Some((head, ty)) = code.split_once("Event for ")
            && (head.ends_with(' ') || head.ends_with("::"))
        {
            events.push(format!("{rel}:{}: {}", n + 1, type_name(ty)));
        }
        for (i, _) in code.match_indices("FactDecl::of::<") {
            let ty = &code[i + "FactDecl::of::<".len()..];
            declared.push(type_name(ty).to_string());
        }
    }
    (events, declared)
}

/// Helios's points and those without their prefix.
#[derive(Debug, Default)]
pub struct PointNames {
    /// Every point with a name, as `path:line: id`.
    pub points: Vec<String>,
    /// Those not under [`PREFIX`].
    pub problems: Vec<String>,
}

/// Reads every crate's `src/` (tests left out) for the points it
/// declares and their names.
pub fn point_names(root: &Path) -> PointNames {
    let mut files = Vec::new();
    for folder in FOLDERS {
        let dir = root.join(folder);
        if dir.is_dir() {
            let _ = sources(&dir, &mut files);
        }
    }
    let mut names = PointNames::default();
    for path in files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if !rel.contains("/src/") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (at, id) in point_names_in(&rel, &text) {
            if !id.starts_with(PREFIX) {
                names
                    .problems
                    .push(format!("{at}: point `{id}` is not under `{PREFIX}`"));
            }
            names.points.push(format!("{at}: {id}"));
        }
    }
    names
}

/// One file's named points, up to its inline test module: each
/// `impl … Point for T` with its `const ID`, a string or a constant of the
/// file, as `(path:line, id)`. An id taken from a fact kind
/// (`E::KIND`, `Mounted::KIND`) is a node fact's, not named here.
pub fn point_names_in(rel: &str, text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text
        .lines()
        .take_while(|line| !line.starts_with("#[cfg(test)]"))
        .collect();
    let constant = |name: &str| {
        lines.iter().find_map(|line| {
            let code = line.split("//").next().unwrap_or("").trim();
            let rest = code
                .strip_prefix("pub ")
                .or_else(|| code.strip_prefix("pub(crate) "))
                .or_else(|| code.strip_prefix("pub(super) "))
                .unwrap_or(code);
            let rest = rest.strip_prefix("const ")?.strip_prefix(name)?;
            let value = rest.strip_prefix(": &str = ")?;
            Some(value.strip_prefix('"')?.split('"').next()?.to_string())
        })
    };
    let mut found = Vec::new();
    let mut in_point = false;
    for (n, line) in lines.iter().enumerate() {
        let code = line.split("//").next().unwrap_or("").trim();
        if code.starts_with("impl") && code.contains("Point for ") {
            in_point = true;
            continue;
        }
        if code.starts_with('}') {
            in_point = false;
            continue;
        }
        if !in_point {
            continue;
        }
        let Some(value) = code
            .strip_prefix("const ID: &'static str = ")
            .and_then(|v| v.strip_suffix(';'))
        else {
            continue;
        };
        let id = if let Some(literal) = value.strip_prefix('"') {
            Some(literal.trim_end_matches('"').to_string())
        } else if value.contains("::") {
            None
        } else {
            constant(value)
        };
        if let Some(id) = id {
            found.push((format!("{rel}:{}", n + 1), id));
        }
    }
    found
}

/// Helios's fact kinds and those without their prefix.
#[derive(Debug, Default)]
pub struct FactKinds {
    /// Every fact kind declared, as `path:line: kind`.
    pub kinds: Vec<String>,
    /// Those not under [`PREFIX`], or whose name cannot be read.
    pub problems: Vec<String>,
}

/// Reads every crate's `src/` (tests left out) for the fact kinds it
/// declares, each name read from its literal or from a `const` of the
/// crate.
pub fn fact_kinds(root: &Path) -> FactKinds {
    let mut files = Vec::new();
    for folder in FOLDERS {
        let dir = root.join(folder);
        if dir.is_dir() {
            let _ = sources(&dir, &mut files);
        }
    }
    // Each crate's sources, by the crate's folder (the part before `/src/`).
    let mut crates: std::collections::BTreeMap<String, Vec<(String, String)>> =
        std::collections::BTreeMap::new();
    for path in files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let Some((krate, _)) = rel.split_once("/src/") else {
            continue;
        };
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        crates
            .entry(krate.to_string())
            .or_default()
            .push((rel, text));
    }
    let mut found = FactKinds::default();
    for sources in crates.values() {
        let consts: Vec<Const<'_>> = sources
            .iter()
            .flat_map(|(rel, text)| {
                str_consts(text)
                    .into_iter()
                    .map(move |(module, name, value)| Const {
                        file: rel.as_str(),
                        module,
                        name,
                        value,
                    })
            })
            .collect();
        for (rel, text) in sources {
            for (at, name) in fact_kinds_in(rel, text) {
                let kind = match name {
                    KindName::Literal(kind) => Some(kind),
                    KindName::Path(path) => resolve(&path, rel, &consts),
                };
                match kind {
                    Some(kind) if kind.starts_with(PREFIX) => {
                        found.kinds.push(format!("{at}: {kind}"));
                    }
                    Some(kind) => {
                        found.kinds.push(format!("{at}: {kind}"));
                        found
                            .problems
                            .push(format!("{at}: fact kind `{kind}` is not under `{PREFIX}`"));
                    }
                    None => found.problems.push(format!(
                        "{at}: a fact kind whose name is not a literal nor a `const` of its crate"
                    )),
                }
            }
        }
    }
    found
}

/// How a fact kind's name is written where it is declared.
#[derive(Debug, PartialEq)]
pub enum KindName {
    /// A string literal.
    Literal(String),
    /// A constant, by its path (`facts::READY`, `STARTED`).
    Path(String),
}

/// One file's fact kinds, up to its inline test module: the `const KIND`
/// of each `impl … Event for T`, and the first argument of each
/// `FactDecl::json(` and `FactDecl::typed::<T>(`, as `(path:line, name)`.
pub fn fact_kinds_in(rel: &str, text: &str) -> Vec<(String, KindName)> {
    let lines: Vec<&str> = text
        .lines()
        .take_while(|line| !line.starts_with("#[cfg(test)]"))
        .collect();
    let mut found = Vec::new();
    let mut in_event = false;
    for (n, line) in lines.iter().enumerate() {
        let code = line.split("//").next().unwrap_or("").trim();
        let at = format!("{rel}:{}", n + 1);
        if code.starts_with("impl")
            && let Some((head, _)) = code.split_once("Event for ")
            && (head.ends_with(' ') || head.ends_with("::"))
        {
            in_event = true;
            continue;
        }
        if code.starts_with('}') {
            in_event = false;
        }
        if in_event
            && let Some(value) = code
                .strip_prefix("const KIND: &'static str = ")
                .and_then(|v| v.strip_suffix(';'))
        {
            let name = match value.strip_prefix('"') {
                Some(literal) => KindName::Literal(literal.trim_end_matches('"').into()),
                None => KindName::Path(value.to_string()),
            };
            found.push((at.clone(), name));
        }
        for opener in ["FactDecl::json(", "FactDecl::typed::<"] {
            for (i, _) in code.match_indices(opener) {
                let mut rest = &code[i + opener.len()..];
                if opener.ends_with('<') {
                    // Past the type's generics and the call's parenthesis.
                    let Some(open) = rest.find(">(") else {
                        continue;
                    };
                    rest = &rest[open + 2..];
                }
                // The argument on the next line when the call breaks there.
                let arg = if rest.trim().is_empty() {
                    lines.get(n + 1).map_or("", |l| l.trim())
                } else {
                    rest.trim()
                };
                let arg = arg.split(',').next().unwrap_or("").trim();
                let name = match arg.strip_prefix('"') {
                    Some(literal) => KindName::Literal(literal.trim_end_matches('"').into()),
                    None => KindName::Path(arg.to_string()),
                };
                found.push((at.clone(), name));
            }
        }
    }
    found
}

/// A `const NAME: &str = "…";` of a file: its inline module (`pub mod
/// facts { … }`) if any, its name and its value.
type StrConst = (Option<String>, String, String);

/// The `const NAME: &str = "…";` of one file, any visibility, up to its
/// inline test module.
fn str_consts(text: &str) -> Vec<StrConst> {
    let mut found = Vec::new();
    // The inline module the line is in, and how deep its braces go.
    let mut module: Option<(String, usize)> = None;
    for line in text
        .lines()
        .take_while(|line| !line.starts_with("#[cfg(test)]"))
    {
        let code = line.split("//").next().unwrap_or("").trim();
        let opens = code
            .strip_prefix("pub ")
            .unwrap_or(code)
            .strip_prefix("mod ")
            .and_then(|rest| rest.strip_suffix(" {"));
        if module.is_none()
            && let Some(name) = opens
        {
            module = Some((name.to_string(), 0));
        }
        if let Some(c) = code.split_once("const ").map(|(_, c)| c)
            && let Some((name, value)) = c.split_once(": &str = \"")
            && let Some(value) = value.split('"').next()
        {
            let inside = module.as_ref().map(|(m, _)| m.clone());
            found.push((inside, name.trim().to_string(), value.to_string()));
        }
        if let Some((_, depth)) = module.as_mut() {
            *depth = (*depth + code.matches('{').count()).saturating_sub(code.matches('}').count());
            if *depth == 0 {
                module = None;
            }
        }
    }
    found
}

/// A `const NAME: &str` of a crate, where it is.
#[derive(Debug)]
pub struct Const<'a> {
    /// Its file, from the workspace root.
    pub file: &'a str,
    /// The inline module it is in, if any.
    pub module: Option<String>,
    pub name: String,
    pub value: String,
}

/// The value of the constant `path` names, from the crate's `consts`:
/// the one of the module the path names (`promises::DONE` in
/// `promises.rs`, `facts::READY` in a `mod facts`), else of `rel` itself,
/// else the crate's only one of that name.
fn resolve(path: &str, rel: &str, consts: &[Const<'_>]) -> Option<String> {
    let mut segments = path.rsplit("::");
    let name = segments.next()?;
    let module = segments.next();
    let named: Vec<&Const<'_>> = consts.iter().filter(|c| c.name == name).collect();
    let stem = |file: &str| {
        file.rsplit('/')
            .next()
            .and_then(|f| f.strip_suffix(".rs"))
            .map(str::to_string)
    };
    let by_module = module.and_then(|m| {
        named
            .iter()
            .find(|c| c.module.as_deref() == Some(m) || stem(c.file).as_deref() == Some(m))
            .map(|c| c.value.clone())
    });
    by_module
        .or_else(|| {
            named
                .iter()
                .find(|c| c.file == rel && c.module.is_none())
                .map(|c| c.value.clone())
        })
        .or_else(|| match named.as_slice() {
            [only] => Some(only.value.clone()),
            _ => None,
        })
}

/// The last segment of the type path `ty` starts with, its generics and
/// what follows left out: `endpoint::CommandRan<'static>>(` is `CommandRan`.
fn type_name(ty: &str) -> &str {
    let end = ty
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
        .unwrap_or(ty.len());
    ty[..end].rsplit("::").next().unwrap_or("")
}

/// The constructions in one file's text, comments skipped, up to its
/// inline test module (a top-level `#[cfg(test)]`).
pub fn constructions(rel: &str, text: &str) -> Vec<String> {
    text.lines()
        .enumerate()
        .take_while(|(_, line)| !line.starts_with("#[cfg(test)]"))
        .filter_map(|(n, line)| {
            let code = line.split("//").next().unwrap_or("");
            CONSTRUCTORS
                .iter()
                .any(|c| {
                    code.match_indices(c).any(|(i, _)| {
                        // `ReadKernel::new(` is not the kernel.
                        !code[..i]
                            .chars()
                            .next_back()
                            .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
                    })
                })
                .then(|| format!("{rel}:{}: {}", n + 1, line.trim()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_library_constructing_a_kernel_is_found_its_tests_are_not() {
        let text = "let bus = Arc::new(egui_pop_hooks::Kernel::new());\n\
                    let k = Kernel::default();\n\
                    // Kernel::new() in a comment\n\
                    let r = ReadKernel::new(x);\n\
                    let d = Kernel::with_default_deadline(t);\n\
                    #[cfg(test)]\n\
                    mod tests { fn bus() { Kernel::new(); } }\n";
        assert_eq!(
            constructions("crates/x/src/lib.rs", text),
            [
                "crates/x/src/lib.rs:1: let bus = Arc::new(egui_pop_hooks::Kernel::new());",
                "crates/x/src/lib.rs:2: let k = Kernel::default();",
                "crates/x/src/lib.rs:5: let d = Kernel::with_default_deadline(t);",
            ]
        );
    }

    #[test]
    fn a_typed_fact_is_found_with_its_declaration() {
        let text = "impl Event for Ready {\n\
                    impl helios_core::Event for Progress {\n\
                    impl Event for CommandRan<'_> {\n\
                    impl SomeEvent for NotOne {\n\
                    pub const FACTS: &[FactDecl] = &[FactDecl::of::<endpoint::CommandRan<'static>>(\"x\")];\n\
                    #[cfg(test)]\n\
                    impl Event for Ping {\n";
        let (events, declared) = facts_in("crates/x/src/lib.rs", text);
        assert_eq!(
            events,
            [
                "crates/x/src/lib.rs:1: Ready",
                "crates/x/src/lib.rs:2: Progress",
                "crates/x/src/lib.rs:3: CommandRan",
            ]
        );
        assert_eq!(declared, ["CommandRan"]);
    }

    #[test]
    fn every_typed_fact_of_the_workspace_is_declared() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let facts = facts(root);
        assert_eq!(facts.undeclared, Vec::<String>::new());
        assert!(facts.events.len() >= 30, "{facts:?}");
    }

    #[test]
    fn a_desktop_subscription_is_found_outside_comments_and_tests() {
        let text = "let rx = bus.subscribe();\n\
                    // bus.subscribe() in a comment\n\
                    let n = engine.bus().subscribe_kinds(&[K], None);\n\
                    let (a, c) = self.subscribe(seq)?;\n\
                    #[cfg(test)]\n\
                    mod tests { fn t() { bus.subscribe(); } }\n";
        assert_eq!(
            subscriptions_in("crates/helios-gui/src/x.rs", text),
            [
                "crates/helios-gui/src/x.rs:1: let rx = bus.subscribe();",
                "crates/helios-gui/src/x.rs:3: let n = engine.bus().subscribe_kinds(&[K], None);",
            ]
        );
    }

    #[test]
    fn a_point_s_name_is_read_from_its_literal_or_its_constant() {
        let text = "pub const OPEN: &str = \"helios.image.open\";\n\
                    impl Point for Open {\n\
                    const ID: &'static str = OPEN;\n\
                    }\n\
                    impl bus_contract::Point for Screen {\n\
                    const ID: &'static str = \"display.window_screen\";\n\
                    }\n\
                    impl<E: Event> Point for FactPoint<E> {\n\
                    const ID: &'static str = E::KIND;\n\
                    }\n\
                    #[cfg(test)]\n\
                    mod tests { impl Point for T { const ID: &'static str = \"t\"; } }\n";
        assert_eq!(
            point_names_in("crates/x/src/lib.rs", text),
            [
                (
                    "crates/x/src/lib.rs:3".to_string(),
                    "helios.image.open".to_string()
                ),
                (
                    "crates/x/src/lib.rs:6".to_string(),
                    "display.window_screen".to_string()
                ),
            ]
        );
    }

    #[test]
    fn every_point_of_the_workspace_is_under_helios() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let named = point_names(root);
        assert_eq!(named.problems, Vec::<String>::new());
        assert!(named.points.len() >= 20, "{named:?}");
    }

    #[test]
    fn a_fact_kind_is_read_from_its_literal_its_constant_or_the_next_line() {
        let text = "pub const STARTED: &str = \"helios.job.started\";\n\
                    impl Event for Ready {\n\
                    const KIND: &'static str = \"node.ready\";\n\
                    }\n\
                    const FACTS: &[FactDecl] = &[\n\
                    FactDecl::json(STARTED, \"{ id }\", \"x\"),\n\
                    FactDecl::json(\n\
                    \"gui.event\",\n\
                    FactDecl::typed::<promises::Promise>(promises::DONE, \"x\"),\n\
                    ];\n\
                    #[cfg(test)]\n\
                    impl Event for Ping { const KIND: &'static str = \"test.ping\"; }\n";
        assert_eq!(
            fact_kinds_in("crates/x/src/lib.rs", text),
            [
                (
                    "crates/x/src/lib.rs:3".to_string(),
                    KindName::Literal("node.ready".into())
                ),
                (
                    "crates/x/src/lib.rs:6".to_string(),
                    KindName::Path("STARTED".into())
                ),
                (
                    "crates/x/src/lib.rs:7".to_string(),
                    KindName::Literal("gui.event".into())
                ),
                (
                    "crates/x/src/lib.rs:9".to_string(),
                    KindName::Path("promises::DONE".into())
                ),
            ]
        );
        let at = |file, module: Option<&str>, name: &str, value: &str| Const {
            file,
            module: module.map(str::to_string),
            name: name.into(),
            value: value.into(),
        };
        let consts = [
            at("crates/x/src/lib.rs", None, "STARTED", "helios.job.started"),
            at("crates/x/src/promises.rs", None, "DONE", "promise.done"),
            at("crates/x/src/other.rs", None, "DONE", "helios.other"),
            at(
                "crates/x/src/states.rs",
                Some("facts"),
                "READY",
                "helios.ready",
            ),
            at("crates/x/src/jobs.rs", None, "READY", "job.ready"),
        ];
        assert_eq!(
            resolve("facts::READY", "crates/x/src/lib.rs", &consts).as_deref(),
            Some("helios.ready")
        );
        assert_eq!(
            str_consts(
                "pub mod facts {\n    pub const READY: &str = \"r\";\n}\nconst B: &str = \"b\";\n"
            ),
            [
                (
                    Some("facts".to_string()),
                    "READY".to_string(),
                    "r".to_string()
                ),
                (None, "B".to_string(), "b".to_string()),
            ]
        );
        assert_eq!(
            resolve("STARTED", "crates/x/src/lib.rs", &consts).as_deref(),
            Some("helios.job.started")
        );
        assert_eq!(
            resolve("promises::DONE", "crates/x/src/lib.rs", &consts).as_deref(),
            Some("promise.done")
        );
        assert_eq!(resolve("DONE", "crates/x/src/lib.rs", &consts), None);
    }

    #[test]
    fn an_unprefixed_fact_kind_is_refused() {
        let dir = std::env::temp_dir().join(format!("xtask-bus-kinds-{}", std::process::id()));
        let src = dir.join("crates/x/src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "impl Event for Ready {\n\
             const KIND: &'static str = \"node.ready\";\n\
             }\n\
             const DONE: &str = \"helios.x.done\";\n\
             const FACTS: &[FactDecl] = &[FactDecl::json(DONE, \"{}\", \"x\")];\n",
        )
        .unwrap();
        let found = fact_kinds(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(found.kinds.len(), 2, "{found:?}");
        assert_eq!(
            found.problems,
            ["crates/x/src/lib.rs:2: fact kind `node.ready` is not under `helios.`"]
        );
    }

    #[test]
    fn every_fact_kind_of_the_workspace_is_under_helios() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let kinds = fact_kinds(root);
        assert_eq!(kinds.problems, Vec::<String>::new());
        assert!(kinds.kinds.len() >= 70, "{kinds:?}");
    }

    #[test]
    fn the_desktop_s_subscriptions_are_the_listed_exceptions() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let found = subscriptions(root);
        assert_eq!(judge_subscriptions(root, &found), Vec::<String>::new());
    }

    #[test]
    fn the_workspace_holds_none() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert_eq!(problems(root), Vec::<String>::new());
    }

    #[test]
    fn the_timings_report_names_the_slowest_and_the_budgets_breached() {
        let snapshot = serde_json::json!({"bus": {"points": [
            {"point": "quiet", "listeners": [], "timing": null},
            {"point": "library.changed", "budget_us": 4000,
             "timing": {"count": 20, "p50_us": 300, "p95_us": 5200, "max_us": 9000, "over_budget": 2},
             "listeners": [
                {"id": "fill", "timing": {"count": 20, "p95_us": 100, "max_us": 200}},
                {"id": "albums", "timing": {"count": 20, "p95_us": 5000, "max_us": 8000}},
                {"id": "never", "timing": null}
             ]},
            {"point": "image.opened", "budget_us": 4000,
             "timing": {"count": 2, "p50_us": 900, "p95_us": 900, "max_us": 900, "over_budget": 0},
             "listeners": [{"id": "marks", "timing": {"count": 2, "p95_us": 900, "max_us": 900}}]}
        ]}});
        assert_eq!(
            timings_report(&snapshot),
            "library.changed: 20 passes, p50 300 µs, p95 5.2 ms, max 9.0 ms; budget 4.0 ms, 2 over\n\
             \x20 albums: 20 runs, p95 5.0 ms, max 8.0 ms\n\
             \x20 fill: 20 runs, p95 100 µs, max 200 µs\n\
             image.opened: 2 passes, p50 900 µs, p95 900 µs, max 900 µs; budget 4.0 ms, 0 over\n\
             \x20 marks: 2 runs, p95 900 µs, max 900 µs\n\
             over budget: library.changed (2 of 20 passes over 4.0 ms)\n"
        );
        assert_eq!(
            timings_report(&serde_json::json!({"points": []})),
            "no timings: the bus offers none, or no point ran a pass\n"
        );
    }
}
