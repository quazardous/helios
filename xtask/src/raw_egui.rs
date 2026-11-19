//! `cargo xtask raw-egui [--check | --update]`: a ratchet on raw egui
//! widgets in the desktop shell (`crates/helios-gui/src`).
//!
//! Helios makes no widgets: forms go through helios-ui's presets (over
//! egui-pop), visual modules through helios-rack (see
//! `docs/design/ui-responsibilities.md`). The shell still draws with
//! hundreds of raw egui calls, too many to move at once, so their number
//! per file is written down in `xtask/allow/raw-egui.toml` and may only go
//! down:
//!
//! - a file with more raw calls than its baseline, or a file with raw
//!   calls and no baseline, fails;
//! - a file with fewer raw calls than its baseline fails too, asking for
//!   `cargo xtask raw-egui --update`, so the baseline never stays high;
//! - `--update` lowers the baseline (and drops files down to zero); it
//!   never raises it.
//!
//! The count is syntactic, over the code with comments, strings and
//! `#[cfg(test)]` items blanked (test files are skipped). A raw call is:
//!
//! - a widget method on a `ui` receiver (`ui.label(..)`, `child_ui.button(..)`):
//!   [`UI_METHODS`];
//! - a hover text on anything (`.on_hover_text(..)` and its variants):
//!   [`HOVER_METHODS`], counted as `on_hover_text`;
//! - an egui widget type named in a path (`egui::Button::new(..)`, or
//!   `RichText::new(..)` in a file that imports it from egui):
//!   [`WIDGET_TYPES`].
//!
//! Layout calls (`ui.horizontal`, `ui.add`, `ui.group`…) are not counted:
//! the widget they hold is.
//!
//! `--check --base REF` (what a pull request's `pr-lint` runs) compares
//! the merge base with REF and the head call by call (see
//! [`crate::delta`], an idea from darktable's `dt-lockcheck` diff mode,
//! <https://github.com/darktable-org/darktable/pull/23056>): it fails only
//! on the calls the change adds, or a baseline it leaves stale, and prints
//! the count as progress ("raw egui calls: 300 → 297 (−3)").

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::vault_access::{strip_comments_and_strings, strip_tests};

pub const USAGE: &str = "raw-egui [--check [--base [REF]] | --update]";

/// The baseline, relative to the workspace root.
pub const BASELINE: &str = "xtask/allow/raw-egui.toml";

/// The folder counted, relative to the workspace root.
pub const SCANNED: &str = "crates/helios-gui/src";

/// Widget methods of `egui::Ui`, counted on a receiver named `ui` or
/// `*_ui`. Kind: `ui.<method>`.
pub const UI_METHODS: &[&str] = &[
    "label",
    "weak",
    "strong",
    "small",
    "monospace",
    "code",
    "heading",
    "colored_label",
    "button",
    "small_button",
    "checkbox",
    "radio",
    "radio_value",
    "selectable_label",
    "selectable_value",
    "toggle_value",
    "text_edit_singleline",
    "text_edit_multiline",
    "add_space",
    "separator",
    "spinner",
    "hyperlink",
    "hyperlink_to",
    "link",
    "collapsing",
    "menu_button",
];

/// Hover texts, counted on any receiver. Kind: `on_hover_text`.
pub const HOVER_METHODS: &[&str] = &[
    "on_hover_text",
    "on_hover_text_at_pointer",
    "on_disabled_hover_text",
];

/// egui widget types, counted where a path names them (`egui::Button`,
/// or the bare name in a file that imports it from egui). Kind:
/// `egui::<Type>`.
pub const WIDGET_TYPES: &[&str] = &[
    "Button",
    "TextEdit",
    "Label",
    "Checkbox",
    "RadioButton",
    "ComboBox",
    "Slider",
    "DragValue",
    "Grid",
    "RichText",
    "Frame",
    "Spinner",
    "ProgressBar",
    "CollapsingHeader",
    "Modal",
    "ScrollArea",
    "Separator",
    "Hyperlink",
];

/// Raw calls by kind (`ui.label`, `on_hover_text`, `egui::Button`).
pub type Kinds = BTreeMap<String, usize>;

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The identifiers of `code` with their char positions, in order.
fn idents(c: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if is_ident(c[i]) {
            let start = i;
            while i < c.len() && is_ident(c[i]) {
                i += 1;
            }
            out.push((start, i));
        } else {
            i += 1;
        }
    }
    out
}

/// The first char at or after `i` that is not white space.
fn skip_space(c: &[char], mut i: usize) -> usize {
    while i < c.len() && c[i].is_whitespace() {
        i += 1;
    }
    i
}

/// The last char position before `i` that is not white space.
fn back_space(c: &[char], mut i: usize) -> Option<usize> {
    while i > 0 {
        i -= 1;
        if !c[i].is_whitespace() {
            return Some(i);
        }
    }
    None
}

/// The widget types a file imports from egui (`use egui::RichText;`,
/// `use egui::{RichText, Ui};`), and the char ranges of its `use` items,
/// which name types without drawing anything.
fn imports(code: &str) -> (Vec<&'static str>, Vec<(usize, usize)>) {
    let c: Vec<char> = code.chars().collect();
    let mut names = Vec::new();
    let mut ranges = Vec::new();
    let words = idents(&c);
    for (n, (start, end)) in words.iter().enumerate() {
        if c[*start..*end] != ['u', 's', 'e'] {
            continue;
        }
        // An item starts after `;`, `{`, `}`, `]` or `)` (a `pub(crate)`),
        // or after `pub`: not `r#use`, not a field named `use`.
        let item = match back_space(&c, *start) {
            None => true,
            Some(p) => matches!(c[p], ';' | '{' | '}' | ']' | ')') || n > 0 && words[n - 1].1 > p,
        };
        if !item {
            continue;
        }
        let stop = (*end..c.len()).find(|i| c[*i] == ';').unwrap_or(c.len());
        ranges.push((*start, stop));
        let text: String = c[*end..stop].iter().collect();
        if text.trim_start().starts_with("egui::") {
            for ty in WIDGET_TYPES {
                let named = text.split(|ch: char| !is_ident(ch)).any(|word| word == *ty);
                if named && !names.contains(ty) {
                    names.push(*ty);
                }
            }
        }
    }
    (names, ranges)
}

/// The raw egui widget calls of one file, by kind; test code, comments
/// and strings are not counted.
pub fn count(text: &str) -> Kinds {
    let mut kinds = Kinds::new();
    for (kind, _) in calls(text) {
        *kinds.entry(kind).or_default() += 1;
    }
    kinds
}

/// One raw call, as the delta mode tells it apart: its file, its kind and
/// its line of code with white space collapsed. No line number, so a call
/// moved within its file is the same call.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Site {
    pub file: String,
    pub kind: String,
    pub snippet: String,
}

impl Site {
    fn key(&self) -> String {
        format!("{}\u{0}{}\u{0}{}", self.file, self.kind, self.snippet)
    }
}

/// The raw calls of one file (`rel`, relative to [`SCANNED`]), each with
/// its snippet.
pub fn sites(rel: &str, text: &str) -> Vec<Site> {
    let lines: Vec<&str> = text.lines().collect();
    calls(text)
        .into_iter()
        .map(|(kind, line)| Site {
            file: rel.to_string(),
            kind,
            snippet: lines
                .get(line)
                .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
                .unwrap_or_default(),
        })
        .collect()
}

/// The raw calls of one file: their kind and their line (from 0).
fn calls(text: &str) -> Vec<(String, usize)> {
    let code = strip_tests(&strip_comments_and_strings(text));
    let c: Vec<char> = code.chars().collect();
    let (imported, uses) = imports(&code);
    let words = idents(&c);
    let word = |n: usize| -> String { c[words[n].0..words[n].1].iter().collect() };
    // The line of each char: blanking keeps the newlines where they were.
    let mut line_of = Vec::with_capacity(c.len());
    let mut line = 0;
    for ch in &c {
        line_of.push(line);
        if *ch == '\n' {
            line += 1;
        }
    }
    let mut kinds = Vec::new();
    for n in 0..words.len() {
        let (start, end) = words[n];
        if uses.iter().any(|(a, b)| start >= *a && start < *b) {
            continue;
        }
        let name = word(n);
        let before = back_space(&c, start);
        let after = skip_space(&c, end);
        // The word just before, when only `sep` (and white space) lies
        // between the two.
        let previous = |sep: &[char]| -> Option<String> {
            let (_, prev_end) = *words.get(n.checked_sub(1)?)?;
            let between: Vec<char> = c[prev_end..start]
                .iter()
                .copied()
                .filter(|ch| !ch.is_whitespace())
                .collect();
            (between == sep).then(|| word(n - 1))
        };
        let called = c.get(after) == Some(&'(');
        if before.is_some_and(|p| c[p] == '.') && called {
            if HOVER_METHODS.contains(&name.as_str()) {
                kinds.push(("on_hover_text".into(), line_of[start]));
            } else if UI_METHODS.contains(&name.as_str())
                && previous(&['.']).is_some_and(|r| r == "ui" || r.ends_with("_ui"))
            {
                kinds.push((format!("ui.{name}"), line_of[start]));
            }
            continue;
        }
        if let Some(ty) = WIDGET_TYPES.iter().find(|ty| **ty == name) {
            let from_egui = previous(&[':', ':']).is_some_and(|p| p == "egui");
            let bare = before.is_none_or(|p| c[p] != ':') && imported.contains(ty);
            if from_egui || bare {
                kinds.push((format!("egui::{ty}"), line_of[start]));
            }
        }
    }
    kinds
}

/// Whether a path under the scanned folder is test code.
fn test_path(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    let dirs = &parts[..parts.len().saturating_sub(1)];
    dirs.contains(&"tests") || parts.last() == Some(&"tests.rs")
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

/// Every file of the scanned folder but test code, by its path relative
/// to that folder, with its text.
fn sources(root: &Path) -> Vec<(String, String)> {
    let base = root.join(SCANNED);
    let mut files = Vec::new();
    walk(&base, &mut files);
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let rel = path
            .strip_prefix(&base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if test_path(&rel) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            out.push((rel, text));
        }
    }
    out
}

/// Every file of the scanned folder that has raw calls, by its path
/// relative to that folder.
pub fn scan(root: &Path) -> BTreeMap<String, Kinds> {
    sources(root)
        .into_iter()
        .filter_map(|(rel, text)| {
            let kinds = count(&text);
            (!kinds.is_empty()).then_some((rel, kinds))
        })
        .collect()
}

/// Every raw call of the scanned folder.
pub fn scan_sites(root: &Path) -> Vec<Site> {
    sources(root)
        .into_iter()
        .flat_map(|(rel, text)| sites(&rel, &text))
        .collect()
}

/// What a change does to the raw calls (`--base`).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Delta {
    /// Raw calls on the base and on the head.
    pub before: usize,
    pub after: usize,
    /// How many raw calls the change adds.
    pub new: usize,
    /// The calls added, where the change adds some: in a file and of a
    /// kind with more calls added than removed. A call whose line was
    /// edited (one removed, one added, same file and kind) is not new.
    pub added: Vec<Site>,
}

/// Compares the raw calls of the base and of the head, each call known by
/// its file, kind and snippet (see [`crate::delta`]).
pub fn delta(base: &[Site], head: &[Site]) -> Delta {
    let added = crate::delta::added(base, head, Site::key);
    let removed = crate::delta::added(head, base, Site::key);
    let group = |s: &Site| (s.file.clone(), s.kind.clone());
    let mut balance: BTreeMap<(String, String), isize> = BTreeMap::new();
    for s in &added {
        *balance.entry(group(s)).or_default() += 1;
    }
    for s in &removed {
        *balance.entry(group(s)).or_default() -= 1;
    }
    let new = balance
        .values()
        .filter(|n| **n > 0)
        .map(|n| n.unsigned_abs())
        .sum();
    Delta {
        before: base.len(),
        after: head.len(),
        new,
        added: added
            .into_iter()
            .filter(|s| balance.get(&group(s)).is_some_and(|n| *n > 0))
            .collect(),
    }
}

/// The baseline problems of a tree but the raw calls gone up (the delta
/// reports those call by call): a stale or missing baseline.
fn baseline_problems(root: &Path) -> Vec<String> {
    problems(root)
        .into_iter()
        .filter(|p| !p.contains(" allowed:") && !p.contains("in a file that had none"))
        .collect()
}

/// `raw-egui --check --base REF`: the raw calls the change adds, and the
/// baseline problems it brings; the count printed as progress.
pub fn check_delta(root: &Path, base: &str) -> Result<Vec<String>, String> {
    let rev = crate::delta::merge_base(root, base)?;
    let tree = crate::delta::BaseTree::extract(root, &rev, &[SCANNED, BASELINE])?;
    let d = delta(&scan_sites(&tree.dir), &scan_sites(root));
    println!(
        "{}",
        crate::delta::progress("raw egui calls", d.before, d.after)
    );
    let mut out: Vec<String> = d
        .added
        .iter()
        .map(|s| {
            format!(
                "{SCANNED}/{}: new {}: `{}`: draw it through helios-ui (forms) or helios-rack (visual modules)",
                s.file, s.kind, s.snippet
            )
        })
        .collect();
    if d.new > 0 {
        out.push(format!("{} raw egui widget call(s) added", d.new));
    }
    let file = |p: &String| p.split(':').next().unwrap_or("").to_string();
    out.extend(crate::delta::added(
        &baseline_problems(&tree.dir),
        &baseline_problems(root),
        file,
    ));
    Ok(out)
}

fn totals(found: &BTreeMap<String, Kinds>) -> BTreeMap<String, usize> {
    found
        .iter()
        .map(|(file, kinds)| (file.clone(), kinds.values().sum()))
        .collect()
}

/// Reads the baseline: `"path" = count` lines under `[files]`.
pub fn parse_baseline(text: &str) -> Result<BTreeMap<String, usize>, String> {
    let mut out = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line == "[files]" {
            continue;
        }
        let bad = || format!("{BASELINE}:{}: expected `\"path\" = count`", i + 1);
        let (key, value) = line.split_once('=').ok_or_else(bad)?;
        let key = key
            .trim()
            .strip_prefix('"')
            .and_then(|k| k.strip_suffix('"'))
            .ok_or_else(bad)?;
        let value: usize = value.trim().parse().map_err(|_| bad())?;
        if value == 0 || out.insert(key.to_string(), value).is_some() {
            return Err(format!(
                "{BASELINE}:{}: `{key}` is listed twice or with no call",
                i + 1
            ));
        }
    }
    Ok(out)
}

const HEADER: &str = "\
# Raw egui widget calls per file of crates/helios-gui/src (see `cargo xtask
# raw-egui` and xtask/src/raw_egui.rs for what is counted). Helios makes no
# widgets: a file's count may only go down, and a file not listed here has
# none. Written by `cargo xtask raw-egui --update`, which only lowers it.
";

pub fn render_baseline(counts: &BTreeMap<String, usize>) -> String {
    let mut out = format!("{HEADER}\n[files]\n");
    for (file, n) in counts {
        if *n > 0 {
            let _ = writeln!(out, "\"{file}\" = {n}");
        }
    }
    out
}

/// What the comparison found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// Files with more raw calls than allowed: the ratchet's failures.
    pub up: Vec<String>,
    /// Files with fewer raw calls than the baseline says: it is stale.
    pub down: Vec<String>,
}

/// Compares the counts found with the baseline.
pub fn compare(found: &BTreeMap<String, usize>, baseline: &BTreeMap<String, usize>) -> Verdict {
    let mut verdict = Verdict::default();
    for (file, n) in found {
        match baseline.get(file) {
            None if *n > 0 => verdict.up.push(format!(
                "{SCANNED}/{file}: {n} raw egui widget call(s) in a file that had none: draw through helios-ui (forms) or helios-rack (visual modules)"
            )),
            Some(allowed) if n > allowed => verdict.up.push(format!(
                "{SCANNED}/{file}: {n} raw egui widget calls, {allowed} allowed: draw the new ones through helios-ui (forms) or helios-rack (visual modules)"
            )),
            Some(allowed) if n < allowed => verdict.down.push(format!(
                "{SCANNED}/{file}: {n} raw egui widget calls, down from {allowed}"
            )),
            _ => {}
        }
    }
    for (file, allowed) in baseline {
        if !found.contains_key(file) {
            verdict.down.push(format!(
                "{SCANNED}/{file}: no raw egui widget call left, down from {allowed}"
            ));
        }
    }
    verdict
}

/// The baseline `--update` writes: what was found where it is lower, the
/// old value elsewhere; never higher, never a new file.
pub fn lowered(
    found: &BTreeMap<String, usize>,
    baseline: &BTreeMap<String, usize>,
) -> BTreeMap<String, usize> {
    baseline
        .iter()
        .filter_map(|(file, allowed)| {
            let n = found.get(file).copied().unwrap_or(0).min(*allowed);
            (n > 0).then(|| (file.clone(), n))
        })
        .collect()
}

fn read_baseline(root: &Path) -> Result<Option<BTreeMap<String, usize>>, String> {
    match std::fs::read_to_string(root.join(BASELINE)) {
        Ok(text) => parse_baseline(&text).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{BASELINE}: {e}")),
    }
}

/// What `--check` reports: a count gone up, a stale baseline, or none.
pub fn problems(root: &Path) -> Vec<String> {
    let found = totals(&scan(root));
    let baseline = match read_baseline(root) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return vec![format!(
                "{BASELINE} is missing: `cargo xtask raw-egui --update` writes it"
            )];
        }
        Err(e) => return vec![e],
    };
    let verdict = compare(&found, &baseline);
    let mut out = verdict.up;
    if !verdict.down.is_empty() {
        out.extend(verdict.down);
        out.push(format!(
            "{BASELINE} is stale: run `cargo xtask raw-egui --update` and commit it"
        ));
    }
    out
}

/// The totals by kind and the files with the most raw calls.
fn report(found: &BTreeMap<String, Kinds>) {
    let mut by_kind = Kinds::new();
    for kinds in found.values() {
        for (kind, n) in kinds {
            *by_kind.entry(kind.clone()).or_default() += n;
        }
    }
    let total: usize = by_kind.values().sum();
    println!(
        "{total} raw egui widget calls in {} files of {SCANNED}",
        found.len()
    );
    let mut kinds: Vec<_> = by_kind.into_iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("\nBy kind:");
    for (kind, n) in kinds {
        println!("{n:>6}  {kind}");
    }
    let mut files: Vec<_> = totals(found).into_iter().collect();
    files.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("\nTop files:");
    for (file, n) in files.iter().take(10) {
        println!("{n:>6}  {file}");
    }
    println!();
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let fail = |problems: &[String]| {
        for p in problems {
            eprintln!("raw-egui: {p}");
        }
        ExitCode::from(1)
    };
    let (args, base) = crate::delta::take_base(args)?;
    match (args.as_slice(), base) {
        (["--check"], Some(base)) => Some(match check_delta(root, base) {
            Ok(problems) if problems.is_empty() => {
                println!("Raw egui widgets in {SCANNED}: none added since {base}.");
                ExitCode::SUCCESS
            }
            Ok(problems) => fail(&problems),
            Err(e) => fail(&[e]),
        }),
        ([] | ["--check"], None) => {
            if args.is_empty() {
                report(&scan(root));
            }
            let problems = problems(root);
            Some(if problems.is_empty() {
                println!("Raw egui widgets in {SCANNED}: none added, baseline up to date.");
                ExitCode::SUCCESS
            } else {
                fail(&problems)
            })
        }
        (["--update"], None) => {
            let found = totals(&scan(root));
            let (next, up) = match read_baseline(root) {
                // The first baseline is what is there today.
                Ok(None) => (found, Vec::new()),
                Ok(Some(baseline)) => (lowered(&found, &baseline), compare(&found, &baseline).up),
                Err(e) => return Some(fail(&[e])),
            };
            if let Err(e) = std::fs::write(root.join(BASELINE), render_baseline(&next)) {
                return Some(fail(&[format!("{BASELINE}: {e}")]));
            }
            let total: usize = next.values().sum();
            println!(
                "{BASELINE}: {total} raw egui widget calls in {} files.",
                next.len()
            );
            Some(if up.is_empty() {
                ExitCode::SUCCESS
            } else {
                fail(&up)
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(pairs: &[(&str, usize)]) -> Kinds {
        pairs.iter().map(|(k, n)| ((*k).to_string(), *n)).collect()
    }

    fn counts(pairs: &[(&str, usize)]) -> BTreeMap<String, usize> {
        kinds(pairs)
    }

    #[test]
    fn raw_egui_counts_ui_methods_types_and_hover_texts() {
        let text = r#"
            fn draw(ui: &mut egui::Ui, child_ui: &mut egui::Ui) {
                ui.label("a");
                ui.weak("b");
                child_ui.button("c").on_hover_text("d");
                ui
                    .add_space(4.0);
                ui.add(egui::Button::new("e"));
                let _ = egui::RichText::new("f").strong();
                ui.horizontal(|ui| { ui.separator(); });
            }
        "#;
        assert_eq!(
            count(text),
            kinds(&[
                ("ui.label", 1),
                ("ui.weak", 1),
                ("ui.button", 1),
                ("on_hover_text", 1),
                ("ui.add_space", 1),
                ("egui::Button", 1),
                ("egui::RichText", 1),
                ("ui.separator", 1),
            ])
        );
    }

    #[test]
    fn raw_egui_skips_comments_strings_tests_and_other_receivers() {
        let text = r#"
            // ui.label("in a comment")
            /* egui::Button::new */
            fn draw(ui: &mut egui::Ui, step: &Step) {
                let _ = "ui.label(x) egui::Slider";
                let _ = step.label();
                let _ = text.weak();
                let _ = my::Button::new();
                let _ = Button::new();
                ui.painter();
            }
            #[cfg(test)]
            mod tests {
                fn t(ui: &mut egui::Ui) { ui.label("x"); }
            }
        "#;
        assert_eq!(count(text), Kinds::new());
    }

    #[test]
    fn raw_egui_counts_a_type_imported_from_egui_but_not_its_import() {
        let text = r"
            use egui::{RichText, Ui};
            pub(crate) use egui::Label;
            fn draw(ui: &mut Ui) {
                let a = RichText::new(1);
                let b = Label::new(a);
                let c = other::RichText::new(2);
            }
        ";
        assert_eq!(
            count(text),
            kinds(&[("egui::RichText", 1), ("egui::Label", 1)])
        );
    }

    #[test]
    fn raw_egui_test_paths_are_skipped() {
        assert!(test_path("shell/tests.rs"));
        assert!(test_path("shell/tests/import.rs"));
        assert!(!test_path("shell/import_window.rs"));
    }

    #[test]
    fn raw_egui_compare_fails_a_count_going_up_and_a_new_file() {
        let baseline = counts(&[("a.rs", 3), ("b.rs", 2)]);
        let found = counts(&[("a.rs", 4), ("b.rs", 2), ("new.rs", 1)]);
        let verdict = compare(&found, &baseline);
        assert_eq!(verdict.up.len(), 2, "{verdict:?}");
        assert!(verdict.up[0].contains("a.rs: 4 raw egui widget calls, 3 allowed"));
        assert!(verdict.up[1].contains("new.rs: 1 raw egui widget call(s)"));
        assert!(verdict.down.is_empty());
    }

    #[test]
    fn raw_egui_compare_reports_a_stale_baseline() {
        let baseline = counts(&[("a.rs", 3), ("gone.rs", 2)]);
        let found = counts(&[("a.rs", 1)]);
        let verdict = compare(&found, &baseline);
        assert!(verdict.up.is_empty());
        assert_eq!(verdict.down.len(), 2, "{verdict:?}");
        assert_eq!(compare(&baseline, &baseline), Verdict::default());
    }

    #[test]
    fn raw_egui_update_only_lowers() {
        let baseline = counts(&[("a.rs", 3), ("b.rs", 2), ("gone.rs", 2)]);
        let found = counts(&[("a.rs", 1), ("b.rs", 5), ("new.rs", 1)]);
        assert_eq!(
            lowered(&found, &baseline),
            counts(&[("a.rs", 1), ("b.rs", 2)])
        );
    }

    #[test]
    fn raw_egui_baseline_round_trips_and_refuses_nonsense() {
        let baseline = counts(&[("shell/a.rs", 3), ("b.rs", 12)]);
        let text = render_baseline(&baseline);
        assert_eq!(parse_baseline(&text), Ok(baseline));
        assert!(parse_baseline("[files]\n\"a.rs\" = 0\n").is_err());
        assert!(parse_baseline("[files]\n\"a.rs\" = 1\n\"a.rs\" = 2\n").is_err());
        assert!(parse_baseline("[files]\na.rs = many\n").is_err());
    }

    const BEFORE: &str = r#"
fn draw(ui: &mut egui::Ui) {
    ui.label("one");
    ui.button("two");
    ui.weak("three");
}

fn other(ui: &mut egui::Ui) {
    ui.heading("four");
}
"#;

    fn delta_of(base: &str, head: &str) -> Delta {
        delta(&sites("a.rs", base), &sites("a.rs", head))
    }

    #[test]
    fn raw_egui_delta_a_branch_adding_one_call_gives_one_finding() {
        let head = BEFORE.replace(
            "    ui.weak(\"three\");\n",
            "    ui.weak(\"three\");\n    ui.small(\"new\");\n",
        );
        let d = delta_of(BEFORE, &head);
        assert_eq!((d.before, d.after, d.new), (4, 5, 1), "{d:?}");
        assert_eq!(
            d.added,
            [Site {
                file: "a.rs".into(),
                kind: "ui.small".into(),
                snippet: "ui.small(\"new\");".into(),
            }]
        );
    }

    #[test]
    fn raw_egui_delta_a_branch_removing_two_shows_the_decrease() {
        let head = BEFORE
            .replace("    ui.label(\"one\");\n", "")
            .replace("    ui.heading(\"four\");\n", "");
        let d = delta_of(BEFORE, &head);
        assert_eq!((d.before, d.after, d.new), (4, 2, 0), "{d:?}");
        assert!(d.added.is_empty());
        assert_eq!(
            crate::delta::progress("raw egui calls", d.before, d.after),
            "raw egui calls: 4 → 2 (−2)"
        );
    }

    #[test]
    fn raw_egui_delta_moving_code_gives_nothing() {
        // `other` moved above `draw`, re-indented, with lines added above.
        let head = r#"
// A comment that shifts every line.
fn other(ui: &mut egui::Ui) {
        ui.heading("four");
}

fn draw(ui: &mut egui::Ui) {
    ui.label("one");
    ui.button("two");
    ui.weak("three");
}
"#;
        assert_eq!(
            delta_of(BEFORE, head),
            Delta {
                before: 4,
                after: 4,
                new: 0,
                added: Vec::new()
            }
        );
    }

    #[test]
    fn raw_egui_delta_an_edited_call_is_not_new() {
        let head = BEFORE.replace("ui.label(\"one\")", "ui.label(\"uno\")");
        let d = delta_of(BEFORE, &head);
        assert_eq!((d.new, d.added.len()), (0, 0), "{d:?}");
    }

    #[test]
    fn raw_egui_committed_baseline_matches_the_tree() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert_eq!(problems(root), Vec::<String>::new());
    }
}
