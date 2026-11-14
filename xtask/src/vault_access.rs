//! The vault access guard, part of `cargo xtask check`: only the vault's
//! SQLite driver (`helios-vault-sqlite`) and the node (`helios-node-core`)
//! open the vault file or a raw SQLite connection. Everyone else reaches
//! the data through the node.
//!
//! The guard reads the Rust sources under `crates/*/src` and
//! `plugins/*/*/src` and refuses `SqliteStore::open` (and `open_with`),
//! `rusqlite::Connection` and `Connection::open*`. A file-backed store only
//! comes from `SqliteStore::open*`, so refusing it also refuses
//! `Vault::new` on a file store; `Vault::new` on an in-memory store stays
//! free. Test code is exempt: `#[cfg(test)]` items, `tests/` folders and
//! fixture files. Comments and string literals are not code.
//!
//! The files that still do it are listed in `xtask/allow/vault-access.toml`,
//! each with its reason. The list may only shrink: an entry whose file no
//! longer opens anything is reported, so it is removed with the code.
//!
//! A second rule holds the desktop (`crates/helios-gui/src`) to the node's
//! client: no `Vault` and no `vault` in its code, so it neither names the
//! vault's type nor calls a vault it was handed. The desktop's files that
//! still do are listed in `xtask/allow/gui-vault.toml`, each with its
//! reason, and that list only shrinks too.

use std::path::{Path, PathBuf};

/// Crates allowed to open the vault and raw SQLite connections.
pub const ALLOWED_CRATES: &[&str] = &["helios-vault-sqlite", "helios-node-core"];

/// The allow-list, relative to the workspace root.
pub const ALLOW_LIST: &str = "xtask/allow/vault-access.toml";

/// What the guard refuses, matched against code with comments and
/// string literals removed.
const FORBIDDEN: &[&str] = &[
    "SqliteStore::open",
    "rusqlite::Connection",
    "Connection::open",
];

/// The desktop's sources, held to the node's client.
pub const GUI_SRC: &str = "crates/helios-gui/src";

/// The desktop's allow-list, relative to the workspace root.
pub const GUI_ALLOW_LIST: &str = "xtask/allow/gui-vault.toml";

/// The words the desktop's code may not hold: the vault's type, and a
/// vault in hand (a field, a variable, an accessor).
const GUI_FORBIDDEN: &[&str] = &["Vault", "vault"];

/// One allow-list entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allowed {
    pub path: String,
    pub reason: String,
}

/// One forbidden use: file (relative, `/`-separated), line, what matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    pub line: usize,
    pub what: &'static str,
}

/// Runs the guard over the workspace at `root`.
pub fn problems(root: &Path) -> Vec<String> {
    let allow = match std::fs::read_to_string(root.join(ALLOW_LIST)) {
        Ok(text) => match parse_allow_list(&text) {
            Ok(list) => list,
            Err(e) => return vec![format!("{ALLOW_LIST}: {e}")],
        },
        Err(e) => return vec![format!("{ALLOW_LIST}: {e}")],
    };
    let mut hits = Vec::new();
    for file in sources(root) {
        let rel = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        if exempt_path(&rel) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        hits.extend(scan(&rel, &text));
    }
    judge(&hits, &allow)
}

/// Runs the desktop's rule over the workspace at `root`.
pub fn gui_problems(root: &Path) -> Vec<String> {
    let allow = match std::fs::read_to_string(root.join(GUI_ALLOW_LIST)) {
        Ok(text) => match parse_allow_list(&text) {
            Ok(list) => list,
            Err(e) => return vec![format!("{GUI_ALLOW_LIST}: {e}")],
        },
        Err(e) => return vec![format!("{GUI_ALLOW_LIST}: {e}")],
    };
    let mut files = Vec::new();
    walk(&root.join(GUI_SRC), &mut files);
    files.sort();
    let mut hits = Vec::new();
    for file in files {
        let rel = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        if exempt_path(&rel) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        hits.extend(scan_gui(&rel, &text));
    }
    judge_gui(&hits, &allow)
}

/// The desktop's uses of the vault in one file, test code excepted: the
/// words `Vault` and `vault` as whole identifiers (`helios_vault::` and
/// strings such as `"helios.vault"` are not).
pub fn scan_gui(rel: &str, text: &str) -> Vec<Hit> {
    let code = strip_tests(&strip_comments_and_strings(text));
    let mut hits = Vec::new();
    for (i, line) in code.lines().enumerate() {
        if let Some(what) = GUI_FORBIDDEN.iter().find(|w| has_word(line, w)) {
            hits.push(Hit {
                path: rel.to_string(),
                line: i + 1,
                what,
            });
        }
    }
    hits
}

/// Whether `line` holds `word` as a whole identifier.
fn has_word(line: &str, word: &str) -> bool {
    let ident = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    line.match_indices(word).any(|(at, _)| {
        !ident(line[..at].chars().next_back()) && !ident(line[at + word.len()..].chars().next())
    })
}

/// [`judge`] for the desktop's rule.
pub fn judge_gui(hits: &[Hit], allow: &[Allowed]) -> Vec<String> {
    let mut problems: Vec<String> = hits
        .iter()
        .filter(|h| !allow.iter().any(|a| a.path == h.path))
        .map(|h| {
            format!(
                "{}:{}: `{}` in the desktop: read and write through the node's client (or, for now, add the file to {GUI_ALLOW_LIST} with its reason)",
                h.path, h.line, h.what
            )
        })
        .collect();
    problems.extend(
        allow
            .iter()
            .filter(|a| !hits.iter().any(|h| h.path == a.path))
            .map(|a| {
                format!(
                    "{GUI_ALLOW_LIST}: `{}` no longer uses the vault: remove its entry",
                    a.path
                )
            }),
    );
    problems
}

/// Turns the hits and the allow-list into problems: every hit outside the
/// list, and every list entry that no longer matches a hit.
pub fn judge(hits: &[Hit], allow: &[Allowed]) -> Vec<String> {
    let mut problems: Vec<String> = hits
        .iter()
        .filter(|h| !allow.iter().any(|a| a.path == h.path))
        .map(|h| {
            format!(
                "{}:{}: `{}` outside {}: go through the node (or, for now, add the file to {ALLOW_LIST} with its reason)",
                h.path,
                h.line,
                h.what,
                ALLOWED_CRATES.join(" and ")
            )
        })
        .collect();
    problems.extend(
        allow
            .iter()
            .filter(|a| !hits.iter().any(|h| h.path == a.path))
            .map(|a| {
                format!(
                    "{ALLOW_LIST}: `{}` no longer opens the vault or SQLite: remove its entry",
                    a.path
                )
            }),
    );
    problems
}

/// The Rust files under `crates/*/src` and `plugins/*/*/src`, sorted.
fn sources(root: &Path) -> Vec<PathBuf> {
    let mut srcs = Vec::new();
    for krate in subdirs(&root.join("crates")) {
        srcs.push(krate.join("src"));
    }
    for kind in subdirs(&root.join("plugins")) {
        for plugin in subdirs(&kind) {
            srcs.push(plugin.join("src"));
        }
    }
    let mut files = Vec::new();
    for src in srcs {
        walk(&src, &mut files);
    }
    files.sort();
    files
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, files);
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
}

/// Whether a relative path is outside the guard: an allowed crate, a
/// `tests/` folder or a fixture.
pub fn exempt_path(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.first() == Some(&"crates") && parts.get(1).is_some_and(|c| ALLOWED_CRATES.contains(c))
    {
        return true;
    }
    let dirs = &parts[..parts.len().saturating_sub(1)];
    if dirs.iter().any(|d| *d == "tests" || *d == "fixtures") {
        return true;
    }
    matches!(
        parts.last().copied(),
        Some("tests.rs" | "fixture.rs" | "fixtures.rs")
    )
}

/// The forbidden uses in one file, test code excepted.
pub fn scan(rel: &str, text: &str) -> Vec<Hit> {
    let code = strip_tests(&strip_comments_and_strings(text));
    let mut hits = Vec::new();
    for (i, line) in code.lines().enumerate() {
        for what in FORBIDDEN {
            if line.contains(what) {
                hits.push(Hit {
                    path: rel.to_string(),
                    line: i + 1,
                    what,
                });
                break;
            }
        }
    }
    hits
}

/// Replaces comments and string/char literals by spaces, keeping newlines
/// so line numbers still hold.
fn strip_comments_and_strings(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let blank = |ch: char| if ch == '\n' { '\n' } else { ' ' };
    let mut i = 0;
    while i < c.len() {
        let next = c.get(i + 1).copied();
        if c[i] == '/' && next == Some('/') {
            while i < c.len() && c[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c[i] == '/' && next == Some('*') {
            let mut depth = 0;
            while i < c.len() {
                if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(c[i]));
                    i += 1;
                }
            }
        } else if c[i] == 'r'
            && (next == Some('"') || next == Some('#'))
            && (i == 0 || !(c[i - 1].is_alphanumeric() || c[i - 1] == '_'))
        {
            // Raw string: r"..." or r#"..."#.
            let mut j = i + 1;
            let mut hashes = 0;
            while c.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if c.get(j) != Some(&'"') {
                out.push(c[i]);
                i += 1;
                continue;
            }
            j += 1;
            loop {
                if j >= c.len() {
                    break;
                }
                if c[j] == '"' && (1..=hashes).all(|k| c.get(j + k) == Some(&'#')) {
                    j += 1 + hashes;
                    break;
                }
                j += 1;
            }
            out.extend(c[i..j.min(c.len())].iter().map(|&ch| blank(ch)));
            i = j;
        } else if c[i] == '"' {
            out.push(' ');
            i += 1;
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' && i + 1 < c.len() {
                    out.push(' ');
                    out.push(blank(c[i + 1]));
                    i += 2;
                } else {
                    out.push(blank(c[i]));
                    i += 1;
                }
            }
            out.push(' ');
            i += 1;
        } else if c[i] == '\'' {
            // A char literal ('x', '\n', '\u{..}'), or a lifetime.
            if next == Some('\\') {
                let mut j = i + 2;
                while j < c.len() && c[j] != '\'' {
                    j += 1;
                }
                out.extend(std::iter::repeat_n(' ', j + 1 - i));
                i = j + 1;
            } else if c.get(i + 2) == Some(&'\'') {
                out.push_str("   ");
                i += 3;
            } else {
                out.push('\'');
                i += 1;
            }
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// Blanks every item behind `#[cfg(test)]` (a module, a function, a
/// `mod x;` declaration), keeping newlines. Expects comments and strings
/// already removed, so braces are code braces.
fn strip_tests(code: &str) -> String {
    let mut out: Vec<char> = code.chars().collect();
    let text: String = code.to_string();
    let mut from = 0;
    while let Some(off) = text[from..].find("#[cfg(test)]") {
        let start = from + off;
        // Char index of the byte offset `start`.
        let ci = text[..start].chars().count();
        let mut i = ci;
        let mut depth = 0usize;
        let mut end = out.len();
        while i < out.len() {
            match out[i] {
                '{' => depth += 1,
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = i + 1;
                        break;
                    }
                }
                ';' if depth == 0 => {
                    end = i + 1;
                    break;
                }
                _ => {}
            }
            i += 1;
        }
        for ch in &mut out[ci..end] {
            if *ch != '\n' {
                *ch = ' ';
            }
        }
        from = start + "#[cfg(test)]".len();
    }
    out.into_iter().collect()
}

/// Reads the allow-list: `[[allow]]` tables with `path` and `reason`.
pub fn parse_allow_list(text: &str) -> Result<Vec<Allowed>, String> {
    let mut list: Vec<Allowed> = Vec::new();
    let mut current: Option<(Option<String>, Option<String>)> = None;
    let finish = |cur: Option<(Option<String>, Option<String>)>,
                  list: &mut Vec<Allowed>|
     -> Result<(), String> {
        if let Some((path, reason)) = cur {
            let path = path.ok_or("an [[allow]] entry has no `path`")?;
            let reason = reason
                .filter(|r| !r.trim().is_empty())
                .ok_or_else(|| format!("`{path}` has no `reason`"))?;
            if list.iter().any(|a| a.path == path) {
                return Err(format!("`{path}` is listed twice"));
            }
            list.push(Allowed { path, reason });
        }
        Ok(())
    };
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[allow]]" {
            finish(current.take(), &mut list)?;
            current = Some((None, None));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected `key = \"value\"`", n + 1));
        };
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .ok_or_else(|| format!("line {}: the value must be a quoted string", n + 1))?
            .to_string();
        let Some(entry) = current.as_mut() else {
            return Err(format!("line {}: a key outside an [[allow]] entry", n + 1));
        };
        match key.trim() {
            "path" => entry.0 = Some(value),
            "reason" => entry.1 = Some(value),
            other => return Err(format!("line {}: unknown key `{other}`", n + 1)),
        }
    }
    finish(current.take(), &mut list)?;
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFFENDER: &str = "\
use helios_vault::Vault;
use helios_vault_sqlite::SqliteStore;

pub fn open(path: &std::path::Path) -> Vault<SqliteStore> {
    Vault::new(SqliteStore::open(path).unwrap())
}
";

    const TEST_ONLY: &str = "\
pub fn nothing() {}

#[cfg(test)]
mod tests {
    #[test]
    fn opens() {
        let s = helios_vault_sqlite::SqliteStore::open(path).unwrap();
        let c = rusqlite::Connection::open_in_memory().unwrap();
        if true { let _ = '{'; }
    }
}

pub fn after() {}
";

    fn allow(path: &str) -> Allowed {
        Allowed {
            path: path.into(),
            reason: "a reason".into(),
        }
    }

    #[test]
    fn an_offender_outside_the_allowed_crates_fails() {
        let path = "crates/helios-gui/src/open.rs";
        let hits = scan(path, OFFENDER);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].line, 5);
        assert_eq!(hits[0].what, "SqliteStore::open");
        let problems = judge(&hits, &[]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("crates/helios-gui/src/open.rs:5:"));
    }

    #[test]
    fn raw_connections_are_refused_too() {
        let src = "fn f() { let c = rusqlite::Connection::open(p); }\n\
                   fn g() { let c = Connection::open_with_flags(p, f); }\n";
        let hits = scan("plugins/x/y/src/lib.rs", src);
        assert_eq!(hits.len(), 2, "{hits:?}");
    }

    #[test]
    fn an_allow_listed_offender_passes() {
        let path = "crates/helios-gui/src/open.rs";
        let hits = scan(path, OFFENDER);
        assert!(judge(&hits, &[allow(path)]).is_empty());
    }

    #[test]
    fn a_test_module_passes() {
        let hits = scan("crates/helios-gui/src/open.rs", TEST_ONLY);
        assert!(hits.is_empty(), "{hits:?}");
        // Code after the test module is still read.
        let after = format!("{TEST_ONLY}fn late() {{ SqliteStore::open(p); }}\n");
        let hits = scan("crates/helios-gui/src/open.rs", &after);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].line, 14, "{hits:?}");
    }

    #[test]
    fn comments_and_strings_are_not_code() {
        let src = "// SqliteStore::open(p)\n/* rusqlite::Connection */\n\
                   const S: &str = \"Connection::open\";\nconst R: &str = r#\"SqliteStore::open\"#;\n";
        assert!(scan("crates/a/src/lib.rs", src).is_empty());
    }

    #[test]
    fn allowed_crates_tests_and_fixtures_are_exempt() {
        assert!(exempt_path("crates/helios-vault-sqlite/src/lib.rs"));
        assert!(exempt_path("crates/helios-node-core/src/node.rs"));
        assert!(exempt_path("crates/helios-gui/src/tests/open.rs"));
        assert!(exempt_path("crates/helios-gui/src/tests.rs"));
        assert!(exempt_path("plugins/importers/x/src/fixture.rs"));
        assert!(!exempt_path("crates/helios-gui/src/platform.rs"));
        assert!(!exempt_path("crates/helios-node/src/main.rs"));
    }

    #[test]
    fn a_stale_allow_list_entry_is_reported() {
        let path = "crates/helios-gui/src/open.rs";
        let hits = scan(path, "pub fn nothing() {}\n");
        let problems = judge(&hits, &[allow(path)]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("no longer opens"), "{problems:?}");
    }

    #[test]
    fn the_allow_list_parses_and_wants_reasons() {
        let list = parse_allow_list(
            "# comment\n[[allow]]\npath = \"a.rs\"\nreason = \"why\"\n\n[[allow]]\npath = \"b.rs\"\nreason = \"because\"\n",
        )
        .unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].path, "b.rs");
        assert!(parse_allow_list("[[allow]]\npath = \"a.rs\"\n").is_err());
        assert!(
            parse_allow_list("[[allow]]\npath = \"a.rs\"\nreason = \"x\"\n[[allow]]\npath = \"a.rs\"\nreason = \"y\"\n")
                .is_err()
        );
    }

    #[test]
    fn a_desktop_file_calling_the_vault_is_refused() {
        let path = "crates/helios-gui/src/panel.rs";
        let src = "\
use helios_vault::ImageId;

pub fn shown(persist: &Persist) -> usize {
    persist.vault().images().len()
}

const PANEL: &str = \"helios.vault\";
// a vault, in a comment
";
        let hits = scan_gui(path, src);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!((hits[0].line, hits[0].what), (4, "vault"));
        let problems = judge_gui(&hits, &[]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("crates/helios-gui/src/panel.rs:4:"));
        assert!(judge_gui(&hits, &[allow(path)]).is_empty());
        // The type named is refused too; test code is not read.
        let typed = "fn f<S: Store>(v: &Vault<S>) {}\n";
        assert_eq!(scan_gui(path, typed)[0].what, "Vault");
        assert!(scan_gui(path, TEST_ONLY).is_empty());
        // An entry whose file no longer uses the vault is reported.
        let problems = judge_gui(&[], &[allow(path)]);
        assert!(
            problems[0].contains("no longer uses the vault"),
            "{problems:?}"
        );
    }

    #[test]
    fn the_desktop_passes_with_its_allow_list() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let problems = gui_problems(root);
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn the_workspace_passes_with_its_allow_list() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let problems = problems(root);
        assert!(problems.is_empty(), "{problems:#?}");
    }
}
