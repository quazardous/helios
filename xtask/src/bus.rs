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
    let found = problems(root);
    if found.is_empty() {
        println!("no library crate constructs a bus (the application supplies it)");
        return Some(ExitCode::SUCCESS);
    }
    for f in &found {
        eprintln!("bus: {f}");
    }
    eprintln!(
        "bus: {} construction(s) of a bus in a library crate: take it from the application",
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
    fn the_workspace_holds_none() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert_eq!(problems(root), Vec::<String>::new());
    }
}
