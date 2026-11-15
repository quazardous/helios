//! `ui-index`: Helios's "use these" index, `docs/ui-index.md`, mapping a
//! need ("a search field", "an empty state") to the call that makes it in
//! Helios: first the Helios preset (`helios-ui`, later `helios-rack`), then
//! the egui-pop widget under it.
//!
//! Generated, never kept by hand, from `/// Need (group): ...` doc lines
//! on public items: those of Helios's crates (`crates/*`) and those of the
//! egui-pop crates in `Cargo.lock` (egui-pop writes its own index from the
//! same lines). A preset says the same need as the widget it wraps, so the
//! two meet on one row. The scanner is egui-pop's (`xtask/src/ui_index.rs`
//! there), kept in step by hand: it reads source lines, not a syntax tree.
//! `the_committed_index_is_current` fails when the file is stale, after a
//! new `Need` line here or an egui-pop bump.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

pub const USAGE: &str = "ui-index [--check]";

/// The groups a need may name, in the index's order, with their titles.
pub const GROUPS: &[(&str, &str)] = &[
    ("text", "Text and titles"),
    ("layout", "Spacing, cards and scrolling"),
    ("action", "Buttons and actions"),
    ("input", "Fields and choices"),
    ("state", "Loading, empty and progress"),
    ("status", "Status marks and badges"),
    ("icon", "Icons and glyphs"),
    ("help", "Tooltips, hover cards and notices"),
    ("dialog", "Dialogs, menus and pages"),
    ("data", "Lists, trees and tables"),
    ("image", "Images and imaging"),
];

/// One need line, resolved to its item.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    /// One of [`GROUPS`]' keys.
    pub group: String,
    /// What the need is, as the line says it, without its final period.
    pub need: String,
    /// How it is called: `ui.section_title(..)`, `Input::search(..)`, `EmptyState`.
    pub call: String,
    /// Its full path, through the facade when the crate is re-exported.
    pub path: String,
    /// The item's first doc sentence.
    pub summary: String,
    /// Where it is declared: `crates/<crate>/src/<file>.rs`.
    pub source: String,
}

/// `cargo xtask ui-index [--check]`.
pub fn run(root: &Path, args: &[&str]) -> Option<std::process::ExitCode> {
    let check = match args {
        [] => false,
        ["--check"] => true,
        _ => return None,
    };
    let result = generate(root).and_then(|text| {
        let path = root.join("docs/ui-index.md");
        if check {
            let old = fs::read_to_string(&path)
                .unwrap_or_default()
                .replace("\r\n", "\n");
            if old != text {
                return Err("docs/ui-index.md is stale: run `cargo xtask ui-index`".into());
            }
            println!("docs/ui-index.md is current");
            return Ok(());
        }
        fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("docs/ui-index.md written from the `/// Need` lines");
        Ok(())
    });
    Some(match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask ui-index: {e}");
            std::process::ExitCode::FAILURE
        }
    })
}

/// The index's text, from the workspace at `root`.
pub fn generate(root: &Path) -> Result<String, String> {
    let helios = scan_helios(root)?;
    let (version, egui_pop) = scan_egui_pop(root)?;
    Ok(render(&helios, &egui_pop, &version))
}

/// Every need line of Helios's crates (`crates/*/src`).
pub fn scan_helios(root: &Path) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    let mut crates: Vec<PathBuf> = fs::read_dir(root.join("crates"))
        .map_err(|e| format!("crates/: {e}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("src").is_dir())
        .collect();
    crates.sort();
    for dir in crates {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        let base = name.replace('-', "_");
        let rel = format!("crates/{name}");
        entries.extend(scan_crate(&dir, &base, &rel)?);
    }
    entries.sort();
    Ok(entries)
}

/// The host's target triple (`rustc -vV`).
fn host() -> Result<String, String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc)
        .arg("-vV")
        .output()
        .map_err(|e| format!("cannot run rustc: {e}"))?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(str::to_owned)
        .ok_or_else(|| "rustc -vV: no host line".into())
}

/// The egui-pop crates `Cargo.lock` resolves, as `cargo metadata` gives
/// them: (name, version, directory). Filtered to the host's platform, so
/// a machine that built only for itself has every source it needs.
fn egui_pop_packages(root: &Path) -> Result<Vec<(String, String, PathBuf)>, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let host = host()?;
    let out = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--locked"])
        .args(["--filter-platform", &host])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let v: Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let mut found = Vec::new();
    for p in v["packages"].as_array().into_iter().flatten() {
        let name = p["name"].as_str().unwrap_or_default();
        if !(name == "egui-pop" || name.starts_with("egui-pop-")) || p["source"].is_null() {
            continue;
        }
        let Some(dir) = p["manifest_path"]
            .as_str()
            .and_then(|m| Path::new(m).parent())
        else {
            continue;
        };
        let version = p["version"].as_str().unwrap_or_default();
        found.push((name.to_owned(), version.to_owned(), dir.to_path_buf()));
    }
    found.sort();
    Ok(found)
}

/// Every need line of the egui-pop crates in the lock, and the facade's
/// version.
pub fn scan_egui_pop(root: &Path) -> Result<(String, Vec<Entry>), String> {
    let packages = egui_pop_packages(root)?;
    let facade_dir = packages
        .iter()
        .find(|(n, _, _)| n == "egui-pop")
        .ok_or("no egui-pop in the lock")?;
    let version = facade_dir.1.clone();
    let facade = facade_names(&facade_dir.2.join("src/lib.rs"));
    let mut entries = Vec::new();
    for (name, version, dir) in &packages {
        let base = crate_path(name, &facade);
        let rel = format!("{name} {version}");
        entries.extend(scan_crate(dir, &base, &rel)?);
    }
    entries.sort();
    Ok((version, entries))
}

/// The facade's re-exports: `pub use egui_pop_core as core;` maps
/// `egui_pop_core` to `core`.
pub fn facade_names(lib: &Path) -> BTreeMap<String, String> {
    let text = fs::read_to_string(lib).unwrap_or_default();
    let mut names = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("pub use ") else {
            continue;
        };
        let Some((from, to)) = rest.trim_end_matches(';').split_once(" as ") else {
            continue;
        };
        if !from.contains("::") {
            names.insert(from.trim().to_owned(), to.trim().to_owned());
        }
    }
    names
}

/// A crate's path for users: `egui_pop::core` for a crate the facade
/// re-exports, its own name otherwise.
pub fn crate_path(package: &str, facade: &BTreeMap<String, String>) -> String {
    let ident = package.replace('-', "_");
    if ident == "egui_pop" {
        return ident;
    }
    match facade.get(&ident) {
        Some(short) => format!("egui_pop::{short}"),
        None => ident,
    }
}

/// Every need line of one crate: `dir` holds its `src/`, `base` is its
/// path for users, `rel` its directory as the index shows it.
pub fn scan_crate(dir: &Path, base: &str, rel: &str) -> Result<Vec<Entry>, String> {
    let src = dir.join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
    files.sort();
    let mut entries = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        if !text.contains("Need (") {
            continue;
        }
        let modules = module_path(&src, &file);
        let mut path = base.to_owned();
        for m in &modules {
            path.push_str("::");
            path.push_str(m);
        }
        let shown = format!(
            "{rel}/src/{}",
            file.strip_prefix(&src)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/")
        );
        entries.extend(scan_source(&text, &path, &shown)?);
    }
    Ok(entries)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The public modules leading to `file` from the crate's root. A module
/// declared without `pub` is left out: its items are re-exported by its
/// parent (`pub use glyph::Glyph`), so they are reached there.
fn module_path(src: &Path, file: &Path) -> Vec<String> {
    let Ok(rel) = file.strip_prefix(src) else {
        return Vec::new();
    };
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let last = parts.pop().unwrap_or_default();
    let stem = last.trim_end_matches(".rs");
    if stem != "mod" && stem != "lib" && stem != "main" {
        parts.push(stem.to_owned());
    }
    let mut out = Vec::new();
    let mut parent = src.to_path_buf();
    for (i, m) in parts.iter().enumerate() {
        let parent_file = if i == 0 {
            src.join("lib.rs")
        } else {
            let mod_rs = parent.join("mod.rs");
            if mod_rs.is_file() {
                mod_rs
            } else {
                parent.with_extension("rs")
            }
        };
        let text = fs::read_to_string(&parent_file).unwrap_or_default();
        let public = text.lines().any(|l| {
            let l = l.trim();
            l == format!("pub mod {m};") || l == format!("pub mod {m} {{")
        });
        if public {
            out.push(m.clone());
        }
        parent = parent.join(m);
    }
    out
}

/// The need lines of one source file. `path` is the file's module path,
/// `shown` the file as the index names it.
pub fn scan_source(text: &str, path: &str, shown: &str) -> Result<Vec<Entry>, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut entries = Vec::new();
    let mut docs: Vec<&str> = Vec::new();
    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if let Some(doc) = line.strip_prefix("///") {
            docs.push(doc.strip_prefix(' ').unwrap_or(doc));
            continue;
        }
        if line.starts_with("#[") || line.starts_with("#!") {
            continue;
        }
        let block = std::mem::take(&mut docs);
        let needs = needs_of(&block, shown, i)?;
        if needs.is_empty() {
            continue;
        }
        let Some((kind, name)) = declaration(line) else {
            return Err(format!(
                "{shown}:{}: a `Need` line is not followed by an item",
                i + 1
            ));
        };
        let (call, full) = if kind == "fn" && raw.starts_with(char::is_whitespace) {
            let owner = owner_of(&lines[..i])
                .ok_or_else(|| format!("{shown}:{}: no `impl` or `trait` above `{name}`", i + 1))?;
            let takes_self = signature(&lines[i..]).contains("self");
            let call = match (&owner, takes_self) {
                (Owner::Trait(t), true) if t.starts_with("Ui") => format!("ui.{name}(..)"),
                (_, true) => format!(".{name}(..)"),
                (_, false) => format!("{}::{name}(..)", owner.name()),
            };
            let full = format!("{path}::{}::{name}", owner.name());
            (call, full)
        } else if kind == "fn" {
            let module = path.rsplit("::").next().unwrap_or(path);
            (format!("{module}::{name}(..)"), format!("{path}::{name}"))
        } else {
            (name.clone(), format!("{path}::{name}"))
        };
        let summary = summary_of(&block);
        for (group, need) in needs {
            entries.push(Entry {
                group,
                need,
                call: call.clone(),
                path: full.clone(),
                summary: summary.clone(),
                source: shown.to_owned(),
            });
        }
    }
    Ok(entries)
}

enum Owner {
    Trait(String),
    Type(String),
}

impl Owner {
    fn name(&self) -> &str {
        match self {
            Owner::Trait(n) | Owner::Type(n) => n,
        }
    }
}

/// The `impl` or `trait` a method at the end of `above` belongs to.
fn owner_of(above: &[&str]) -> Option<Owner> {
    for line in above.iter().rev() {
        if line.starts_with(char::is_whitespace) || line.is_empty() {
            continue;
        }
        let l = line.trim_start_matches("pub ");
        if let Some(rest) = l.strip_prefix("trait ") {
            return Some(Owner::Trait(ident(rest)));
        }
        if let Some(rest) = l.strip_prefix("impl") {
            let rest = skip_generics(rest.trim_start());
            let ty = match rest.split_once(" for ") {
                Some((_, ty)) => ty,
                None => rest,
            };
            return Some(Owner::Type(ident(ty.trim())));
        }
        if l.starts_with('}') || l.starts_with("//") || l.starts_with('#') {
            continue;
        }
    }
    None
}

/// `rest` without a leading `<...>`.
fn skip_generics(rest: &str) -> &str {
    if !rest.starts_with('<') {
        return rest;
    }
    let mut depth = 0;
    for (i, c) in rest.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return rest[i + 1..].trim_start();
                }
            }
            _ => {}
        }
    }
    rest
}

/// The identifier `text` starts with (its last path segment).
fn ident(text: &str) -> String {
    let word: String = text
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
        .collect();
    let word = word.trim_end_matches(':');
    word.rsplit("::").next().unwrap_or(word).to_owned()
}

/// A function's signature, up to its body or its `;`.
fn signature(lines: &[&str]) -> String {
    let mut out = String::new();
    for l in lines {
        out.push_str(l.trim());
        out.push(' ');
        if l.contains('{') || l.trim_end().ends_with(';') {
            break;
        }
    }
    out
}

/// An item's kind and name, from its declaration line.
fn declaration(line: &str) -> Option<(String, String)> {
    let mut l = line;
    for prefix in ["pub(crate) ", "pub(super) ", "pub "] {
        l = l.strip_prefix(prefix).unwrap_or(l);
    }
    for prefix in ["const fn ", "async fn ", "unsafe fn "] {
        if let Some(rest) = l.strip_prefix(prefix) {
            return Some(("fn".into(), ident(rest)));
        }
    }
    for kind in ["fn", "struct", "enum", "trait", "type", "const", "static"] {
        if let Some(rest) = l.strip_prefix(kind).and_then(|r| r.strip_prefix(' ')) {
            return Some((kind.into(), ident(rest)));
        }
    }
    None
}

/// The `Need (group): need.` lines of a doc block.
fn needs_of(docs: &[&str], shown: &str, line: usize) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for d in docs {
        let Some(rest) = d.trim().strip_prefix("Need (") else {
            continue;
        };
        let Some((group, need)) = rest.split_once("):") else {
            return Err(format!("{shown}:{line}: write `Need (group): the need.`"));
        };
        if !GROUPS.iter().any(|(g, _)| *g == group) {
            let known: Vec<&str> = GROUPS.iter().map(|(g, _)| *g).collect();
            return Err(format!(
                "{shown}:{line}: unknown need group `{group}` (known: {})",
                known.join(", ")
            ));
        }
        let need = need.trim().trim_end_matches('.').trim();
        if need.is_empty() {
            return Err(format!("{shown}:{line}: an empty need"));
        }
        out.push((group.to_owned(), need.to_owned()));
    }
    Ok(out)
}

/// An item's first doc sentence, its links reduced to their text.
fn summary_of(docs: &[&str]) -> String {
    let mut para = String::new();
    for d in docs {
        let d = d.trim();
        if d.starts_with("Need (") {
            continue;
        }
        if d.is_empty() {
            if para.is_empty() {
                continue;
            }
            break;
        }
        if d.starts_with("```") {
            break;
        }
        if !para.is_empty() {
            para.push(' ');
        }
        para.push_str(d);
    }
    let para = unlink(&para);
    let end = sentence_end(&para);
    para[..end].trim().to_owned()
}

/// The end of the first sentence: a period followed by a space, outside
/// backticks and parentheses, or the whole text.
fn sentence_end(text: &str) -> usize {
    let mut code = false;
    let mut depth = 0i32;
    let bytes = text.as_bytes();
    for (i, c) in text.char_indices() {
        match c {
            '`' => code = !code,
            '(' if !code => depth += 1,
            ')' if !code => depth -= 1,
            '.' if !code && depth <= 0 => {
                let next = bytes.get(i + 1).copied();
                if next.is_none() || next == Some(b' ') {
                    return i + 1;
                }
            }
            _ => {}
        }
    }
    text.len()
}

/// `[`X`]` and `[text](link)` / `[text][ref]` reduced to their text.
fn unlink(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else {
            out.push_str(&rest[open..]);
            return out;
        };
        out.push_str(&after[..close]);
        let mut tail = &after[close + 1..];
        for (o, c) in [('(', ')'), ('[', ']')] {
            if tail.starts_with(o)
                && let Some(end) = tail.find(c)
            {
                tail = &tail[end + 1..];
                break;
            }
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

/// The index's Markdown: for each need, Helios's presets first, then the
/// egui-pop widgets (marked as under a preset when one answers the need).
pub fn render(helios: &[Entry], egui_pop: &[Entry], version: &str) -> String {
    let mut out = String::from(
        "# UI index: what to use for a need in Helios\n\n\
         <!-- Generated by `cargo xtask ui-index` from the `/// Need (group): ...`\n\
         doc lines of Helios's crates and of the egui-pop crates in Cargo.lock.\n\
         Do not edit: add or change a `Need` line on the item, then run the task\n\
         again. A test fails when this file is stale. -->\n\n\
         **Helios makes no widgets.** Before writing any egui call, find the need\n\
         here and use what it names: **the Helios preset first** (`helios_ui`,\n\
         later `helios_rack`); the egui-pop widget under it only where Helios has\n\
         no preset yet. A need missing from this page is a gap to fill in\n\
         egui-pop (then a preset here), not a reason to hand-draw it. See\n\
         [docs/design/ui-responsibilities.md](design/ui-responsibilities.md).\n\n\
         `ui.*` methods come from egui-pop's `UiPop` trait (`egui_pop::prelude`).\n\
         To add an entry, give the public item a doc line\n\
         `/// Need (group): a search field.`, a preset using the same words as\n\
         the widget it wraps; the group is one of: ",
    );
    let keys: Vec<String> = GROUPS.iter().map(|(g, _)| format!("`{g}`")).collect();
    out.push_str(&keys.join(", "));
    out.push_str(&format!(".\n\negui-pop in the lock: {version}.\n"));
    if egui_pop.is_empty() {
        out.push_str(
            "Its crates carry no `Need` lines yet: until the next egui-pop bump, see\n\
             [egui-pop's own index](https://github.com/quazardous/egui-pop/blob/next/docs/ui-index.md).\n",
        );
    }
    for (group, title) in GROUPS {
        let mut needs: BTreeMap<&str, (Vec<&Entry>, Vec<&Entry>)> = BTreeMap::new();
        for e in helios.iter().filter(|e| e.group == *group) {
            needs.entry(&e.need).or_default().0.push(e);
        }
        for e in egui_pop.iter().filter(|e| e.group == *group) {
            needs.entry(&e.need).or_default().1.push(e);
        }
        if needs.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "\n## {title}\n\n| Need | Use | What it is |\n|---|---|---|\n"
        ));
        for (need, (presets, widgets)) in needs {
            let mut first = true;
            let mut row = |call: String, e: &Entry| {
                let shown = if first { cell(need) } else { String::new() };
                first = false;
                out.push_str(&format!(
                    "| {shown} | {call}<br>`{}` | {} |\n",
                    cell(&e.path),
                    cell(&e.summary)
                ));
            };
            for e in &presets {
                row(format!("**`{}`** (Helios preset)", cell(&e.call)), e);
            }
            for e in &widgets {
                let call = if presets.is_empty() {
                    format!("`{}` (egui-pop)", cell(&e.call))
                } else {
                    format!("`{}` (egui-pop, under the preset)", cell(&e.call))
                };
                row(call, e);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_index_is_current() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the workspace root")
            .to_path_buf();
        let text = generate(&root).expect("the need lines parse");
        assert!(
            text.contains("helios_ui::search"),
            "the index lost helios-ui's presets"
        );
        let committed = fs::read_to_string(root.join("docs/ui-index.md"))
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            committed == text,
            "docs/ui-index.md is stale: run `cargo xtask ui-index` and commit it"
        );
    }

    fn entry(need: &str, call: &str) -> Entry {
        Entry {
            group: "input".into(),
            need: need.into(),
            call: call.into(),
            path: format!("p::{call}"),
            summary: "S.".into(),
            source: "f.rs".into(),
        }
    }

    #[test]
    fn a_preset_comes_before_the_widget_it_wraps() {
        let text = render(
            &[entry("a search field", "helios_ui::search(..)")],
            &[
                entry("a search field", "Input::search(..)"),
                entry("a slider", "Slider"),
            ],
            "0.11.0",
        );
        let preset = text
            .find("**`helios_ui::search(..)`** (Helios preset)")
            .expect("preset");
        let widget = text
            .find("`Input::search(..)` (egui-pop, under the preset)")
            .expect("widget under it");
        assert!(preset < widget);
        assert!(text.contains("`Slider` (egui-pop)<br>"));
    }

    #[test]
    fn a_method_and_a_function_get_their_calls() {
        let src = "\
pub trait UiPop: sealed::Sealed {
    /// A section's title.
    /// Need (text): a section title.
    fn section_title(&mut self, text: impl Into<String>) -> Response;
}

/// A search field over `query`: egui-pop's [`Input::search`].
///
/// Need (input): a search field.
pub fn search(ui: &mut Ui, query: &mut String) -> InputOutput {
    todo!()
}
";
        let got = scan_source(src, "helios_ui", "f.rs").expect("parses");
        let calls: Vec<(&str, &str)> = got
            .iter()
            .map(|e| (e.call.as_str(), e.path.as_str()))
            .collect();
        assert_eq!(
            calls,
            [
                ("ui.section_title(..)", "helios_ui::UiPop::section_title"),
                ("helios_ui::search(..)", "helios_ui::search"),
            ]
        );
        assert_eq!(
            got[1].summary,
            "A search field over `query`: egui-pop's `Input::search`."
        );
    }

    #[test]
    fn an_unknown_group_is_refused() {
        let src = "/// Need (stuff): a thing.\npub struct Thing;\n";
        let err = scan_source(src, "p", "f.rs").expect_err("unknown group");
        assert!(err.contains("unknown need group `stuff`"), "{err}");
    }
}
