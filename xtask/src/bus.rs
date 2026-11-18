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

use std::fs;
use std::path::Path;
use std::process::ExitCode;

pub const USAGE: &str = "bus --check";

/// The folders holding the workspace's crates, from the root.
const FOLDERS: &[&str] = &["crates", "modules", "plugins"];

/// Crates that stand for an application in tests: they may build a bus.
const TEST_CRATES: &[&str] = &["crates/helios-testkit/"];

const CONSTRUCTORS: &[&str] = &[
    "Kernel::new(",
    "Kernel::with_default_deadline(",
    "Kernel::default(",
];

/// Runs the check: `cargo xtask bus --check`.
pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    if args != ["--check"] {
        return None;
    }
    let mut found = problems(root);
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
            "typed fact kinds declared in a contract table: {} of {}",
            facts.events.len(),
            facts.events.len()
        );
        return Some(ExitCode::SUCCESS);
    }
    for f in &found {
        eprintln!("bus: {f}");
    }
    eprintln!(
        "bus: {} problem(s): a bus built in a library crate (take it from the application), \
         or a fact no table declares (add its `FactDecl::of`)",
        found.len()
    );
    Some(ExitCode::from(1))
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
    fn the_workspace_holds_none() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert_eq!(problems(root), Vec::<String>::new());
    }
}
