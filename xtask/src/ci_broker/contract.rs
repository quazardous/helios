//! A Rust source file's contract, read without compiling it: whether an
//! edit can change what the crates depending on it see.
//!
//! The idea comes from `cargo public-api`
//! (<https://github.com/cargo-public-api/cargo-public-api>) and
//! `cargo-semver-checks` (<https://github.com/obi1kenobi/cargo-semver-checks>),
//! which compare a crate's public API between two versions. Both need a
//! nightly rustdoc JSON build of the crate, too slow for a selection made
//! before any build. This is a syntactic stand-in, coarse on purpose: it
//! reads more than the API, never less, so an edit it calls internal
//! touches no item another crate can name.
//!
//! The surface of a file ([`surface`]), comments left out, whitespace
//! ignored:
//! - every `pub` item (not `pub(crate)`, `pub(super)`, `pub(in …)`): the
//!   signature of a function, struct or union (up to its body), the whole
//!   of anything else (an enum, a trait, a const and its value, a `use`,
//!   a module);
//! - every top-level `const` and `static`, public or not: point ids and
//!   contract tables are often private constants a public one names;
//! - every `impl` header, and in a trait `impl` its associated consts,
//!   types and function signatures (a bus point's `const ID` lives there);
//! - `#[derive]`, `#[repr]`, `#[non_exhaustive]` and `#[macro_export]`
//!   attributes, crate attributes other than lint levels, the macros
//!   called at the top level (`macro_rules!`, `bitflags! {…}`).
//!
//! A `#[cfg(test)]` module is left out. Like the rest of `ci_broker`, it
//! knows nothing of Helios.

/// Whether the edit from `old` to `new` (`None`: the file is absent on
/// that side) changes the file's surface.
pub fn changed(old: Option<&str>, new: Option<&str>) -> bool {
    surface(old.unwrap_or("")) != surface(new.unwrap_or(""))
}

/// The source twice, line for line and char for char: `keep` without its
/// comments, `shape` without its comments nor the content of its string
/// and char literals (so brackets in strings do not count).
fn strip(src: &str) -> (Vec<Vec<char>>, Vec<Vec<char>>) {
    let c: Vec<char> = src.chars().collect();
    let (mut keep, mut shape) = (String::new(), String::new());
    let push = |keep: &mut String, shape: &mut String, ch: char, blank: bool| {
        keep.push(ch);
        shape.push(if blank && ch != '\n' { ' ' } else { ch });
    };
    let ident = |ch: char| ch.is_alphanumeric() || ch == '_';
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        let next = c.get(i + 1).copied();
        if ch == '/' && next == Some('/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && next == Some('*') {
            let mut depth = 0;
            while i < c.len() {
                if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if c[i] == '\n' {
                        push(&mut keep, &mut shape, '\n', false);
                    }
                    i += 1;
                }
            }
        } else if ch == 'r'
            && (i == 0 || !ident(c[i - 1]))
            && matches!(next, Some('"' | '#'))
            && c[i + 1..]
                .iter()
                .find(|x| **x != '#')
                .is_some_and(|x| *x == '"')
        {
            // a raw string: r"…", r#"…"#
            let hashes = c[i + 1..].iter().take_while(|x| **x == '#').count();
            let open = i + 1 + hashes;
            for x in &c[i..=open] {
                push(&mut keep, &mut shape, *x, false);
            }
            i = open + 1;
            while i < c.len() {
                if c[i] == '"'
                    && c[i + 1..]
                        .iter()
                        .take(hashes)
                        .filter(|x| **x == '#')
                        .count()
                        == hashes
                {
                    for x in &c[i..=i + hashes] {
                        push(&mut keep, &mut shape, *x, false);
                    }
                    i += hashes + 1;
                    break;
                }
                push(&mut keep, &mut shape, c[i], true);
                i += 1;
            }
        } else if ch == '"' {
            push(&mut keep, &mut shape, ch, false);
            i += 1;
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' && i + 1 < c.len() {
                    push(&mut keep, &mut shape, c[i], true);
                    i += 1;
                }
                push(&mut keep, &mut shape, c[i], true);
                i += 1;
            }
            if i < c.len() {
                push(&mut keep, &mut shape, '"', false);
                i += 1;
            }
        } else if ch == '\'' && (next == Some('\\') || c.get(i + 2) == Some(&'\'')) {
            // a char literal ('x', '\n', '\u{..}'), not a lifetime
            push(&mut keep, &mut shape, ch, false);
            i += 1;
            if c.get(i) == Some(&'\\') {
                push(&mut keep, &mut shape, '\\', true);
                i += 1;
            }
            if i < c.len() {
                push(&mut keep, &mut shape, c[i], true);
                i += 1;
            }
            while i < c.len() && c[i] != '\'' && c[i] != '\n' {
                push(&mut keep, &mut shape, c[i], true);
                i += 1;
            }
            if c.get(i) == Some(&'\'') {
                push(&mut keep, &mut shape, '\'', false);
                i += 1;
            }
        } else {
            push(&mut keep, &mut shape, ch, false);
            i += 1;
        }
    }
    let lines = |s: &str| s.split('\n').map(|l| l.chars().collect()).collect();
    (lines(&keep), lines(&shape))
}

/// How a captured item ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    /// At the bracket that closes the first one (an attribute).
    Attr,
    /// Before the first `{`, or at the first `;`, outside brackets (a
    /// signature).
    Sig,
    /// At the first `;` outside brackets, or at the `}` that closes them
    /// (a whole item).
    Item,
    /// At the first `,` outside brackets, `<>` included, or before the
    /// bracket that closes the enclosing one (a field).
    Field,
}

/// What a line starts, if the surface holds it.
fn classify(t: &str, top: bool, in_trait_impl: bool) -> Option<End> {
    const KEPT_ATTRS: [&str; 4] = ["#[derive", "#[repr", "#[non_exhaustive", "#[macro_export"];
    const LINTS: [&str; 5] = ["#![allow", "#![warn", "#![deny", "#![expect", "#![forbid"];
    if KEPT_ATTRS.iter().any(|a| t.starts_with(a)) {
        return Some(End::Attr);
    }
    if t.starts_with("#![") {
        return (!LINTS.iter().any(|a| t.starts_with(a))).then_some(End::Attr);
    }
    let first_words = |s: &str| -> Vec<String> {
        s.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '!'))
            .filter(|w| !w.is_empty())
            .take(5)
            .map(str::to_string)
            .collect()
    };
    let is_fn = |s: &str| {
        // `fn`, after qualifiers only: const, async, unsafe, extern "C"
        let mut words = s.split_whitespace();
        loop {
            match words.next() {
                Some("fn") => return true,
                Some("const" | "async" | "unsafe" | "extern" | "\"C\"" | "\"system\"") => {}
                _ => return false,
            }
        }
    };
    if let Some(rest) = t.strip_prefix("pub ") {
        if is_fn(rest) {
            return Some(End::Sig);
        }
        let name_len = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let after = rest[name_len..].trim_start();
        if name_len > 0 && after.starts_with(':') && !after.starts_with("::") {
            return Some(End::Field);
        }
        let w = first_words(rest);
        return Some(match w.first().map(String::as_str) {
            Some("struct" | "union") => End::Sig,
            _ => End::Item,
        });
    }
    if in_trait_impl {
        if is_fn(t) {
            return Some(End::Sig);
        }
        if t.starts_with("const ") || t.starts_with("type ") {
            return Some(End::Item);
        }
    }
    if t.starts_with("impl") && t[4..].starts_with(|c: char| c == '<' || c.is_whitespace())
        || t.starts_with("unsafe impl")
    {
        return Some(End::Sig);
    }
    if top && (t.starts_with("const ") || t.starts_with("static ") || is_macro_call(t)) {
        return Some(End::Item);
    }
    None
}

/// Whether a line calls a macro (`macro_rules!`, `bitflags! {`): at the
/// top level, it may define public items.
fn is_macro_call(t: &str) -> bool {
    let name_len = t
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
        .unwrap_or(t.len());
    name_len > 0 && t[name_len..].starts_with('!')
}

/// From line `i`, the item's text and the line it ends on.
fn capture(keep: &[Vec<char>], shape: &[Vec<char>], i: usize, end: End) -> (String, usize) {
    let mut out = String::new();
    let mut bal = 0i32;
    for (j, line) in shape.iter().enumerate().skip(i) {
        for (k, ch) in line.iter().enumerate() {
            let arrow = k > 0 && line[k - 1] == '-';
            let stop = match ch {
                '{' if end == End::Sig && bal == 0 => Some(k),
                '<' if end == End::Field => {
                    bal += 1;
                    None
                }
                '>' if end == End::Field && !arrow => {
                    bal -= 1;
                    None
                }
                ',' | ')' | ']' | '}' if end == End::Field && bal == 0 => Some(k),
                '(' | '[' | '{' => {
                    bal += 1;
                    None
                }
                ')' | ']' | '}' => {
                    bal -= 1;
                    (bal == 0 && (end == End::Attr || (end == End::Item && *ch == '}')))
                        .then_some(k + 1)
                }
                ';' if bal == 0 => Some(k + 1),
                _ => None,
            };
            if let Some(cut) = stop {
                out.extend(&keep[j][..cut]);
                return (out, j);
            }
        }
        out.extend(&keep[j]);
        out.push('\n');
    }
    (out, shape.len().saturating_sub(1))
}

/// The text without whitespace nor trailing commas, so a reflow by
/// rustfmt reads the same.
fn normalize(text: &str) -> String {
    let squeezed: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    squeezed
        .iter()
        .enumerate()
        .filter(|(k, c)| {
            !(**c == ',' && matches!(squeezed.get(k + 1), Some(')' | ']' | '}' | '>')))
        })
        .map(|(_, c)| *c)
        .collect()
}

/// The surface of a source file: each item it holds, whitespace removed,
/// in order.
pub fn surface(src: &str) -> Vec<String> {
    let (keep, shape) = strip(src);
    let mut out = Vec::new();
    let mut depth = 0i32;
    // the depths inside a trait impl's block, innermost last
    let mut trait_impls: Vec<i32> = Vec::new();
    // inside a `#[cfg(test)]` module: the depth it closes back to
    let mut skip_to: Option<i32> = None;
    let mut cfg_test = false;
    for i in 0..shape.len() {
        let line: String = keep[i].iter().collect();
        let t = line.trim();
        let before = depth;
        for ch in &shape[i] {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        if let Some(to) = skip_to {
            if depth <= to {
                skip_to = None;
            }
            continue;
        }
        if t.starts_with("#[cfg(test)]") {
            cfg_test = true;
            continue;
        }
        if cfg_test && !t.is_empty() && !t.starts_with("#[") {
            cfg_test = false;
            let rest = t.strip_prefix("pub ").unwrap_or(t);
            if rest.starts_with("mod ") && !t.ends_with(';') {
                if depth > before {
                    skip_to = Some(before);
                }
                continue;
            }
        }
        while trait_impls.last().is_some_and(|d| before < *d) {
            trait_impls.pop();
        }
        let in_trait_impl = trait_impls.last() == Some(&before);
        let Some(end) = classify(t, before == 0, in_trait_impl) else {
            continue;
        };
        let (text, _) = capture(&keep, &shape, i, end);
        let is_impl = t.starts_with("impl") || t.starts_with("unsafe impl");
        if is_impl
            && text
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|w| w == "for")
        {
            trait_impls.push(before + 1);
        }
        let text = normalize(&text);
        out.push(text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"//! A feature.
#![allow(clippy::implicit_hasher)]

use std::fmt;

pub mod admin;
mod link;

/// The id.
pub const READ: &str = "helios.storage.read";
const LIMIT: u32 = 3;

pub const POINTS: &[PointDecl] = &[
    PointDecl::of::<Read>("a read"),
];

#[derive(Clone, Debug)]
pub struct Page {
    pub open: bool,
    cache: Vec<u8>,
}

pub struct Read;

impl Point for Read {
    const ID: PointId = PointId::new(READ);
    type Input = ();
    fn check(&self) -> bool {
        true
    }
}

impl Page {
    pub fn new() -> Self {
        Self { open: false, cache: vec![] }
    }
    fn tidy(&mut self) {
        self.cache.clear();
    }
}

pub fn label(n: u32) -> String {
    format!("{n} {{}}")
}

fn helper() {}

#[cfg(test)]
mod tests {
    pub fn only_in_tests() {}
}
"#;

    fn edit(from: &str, to: &str) -> String {
        assert!(BASE.contains(from), "{from}");
        BASE.replacen(from, to, 1)
    }

    #[test]
    fn a_body_comment_or_private_edit_leaves_the_contract() {
        for new in [
            // function bodies
            edit("format!(\"{n} {{}}\")", "format!(\"{n} items\")"),
            edit("self.cache.clear();", "self.cache.truncate(0);"),
            edit(
                "Self { open: false, cache: vec![] }",
                "Self { open: true, cache: Vec::new() }",
            ),
            edit("        true\n", "        false\n"),
            // private items, comments, lint levels, a reflow
            edit("fn helper() {}", "fn helper() { let _ = 1; }"),
            edit("    cache: Vec<u8>,", "    cache: Vec<u16>,"),
            edit("/// The id.", "/// The id the storage reads."),
            edit("#![allow(clippy::implicit_hasher)]", ""),
            edit("mod link;", "mod link;\nmod more;"),
            edit(
                "pub fn label(n: u32) -> String {",
                "pub fn label(\n    n: u32,\n) -> String {",
            ),
            // inside the test module
            edit(
                "pub fn only_in_tests() {}",
                "pub fn only_in_tests(x: u8) {}",
            ),
        ] {
            assert!(!changed(Some(BASE), Some(&new)), "{new}");
        }
    }

    #[test]
    fn an_api_or_bus_point_edit_changes_the_contract() {
        for new in [
            // public signatures and fields
            edit("pub fn label(n: u32)", "pub fn label(n: u64)"),
            edit(
                "    pub fn new() -> Self {",
                "    pub fn new(open: bool) -> Self {",
            ),
            edit(
                "    pub open: bool,",
                "    pub open: bool,\n    pub shown: bool,",
            ),
            edit("fn helper() {}", "pub fn helper() {}"),
            edit("#[derive(Clone, Debug)]", "#[derive(Debug)]"),
            edit("pub mod admin;", "pub mod admin;\npub mod more;"),
            edit("use std::fmt;", "use std::fmt;\npub use link::Link;"),
            // the bus points: a point's id, the contract table, a private const
            edit("\"helios.storage.read\"", "\"helios.storage.read2\""),
            edit(
                "PointDecl::of::<Read>(\"a read\"),",
                "PointDecl::of::<Read>(\"a read\"),\n    PointDecl::of::<Other>(\"b\"),",
            ),
            edit(
                "const ID: PointId = PointId::new(READ);",
                "const ID: PointId = PointId::new(\"x\");",
            ),
            edit("    type Input = ();", "    type Input = u8;"),
            edit("const LIMIT: u32 = 3;", "const LIMIT: u32 = 4;"),
            // trait impls come and go
            edit(
                "pub struct Read;",
                "pub struct Read;\nimpl Default for Read { fn default() -> Self { Read } }",
            ),
            edit("#![allow(clippy::implicit_hasher)]", "#![no_std]"),
            // a macro at the top level may define public items
            edit("fn helper() {}", "fn helper() {}\nmake_points!(A, B);"),
        ] {
            assert!(
                changed(Some(BASE), Some(&new)),
                "{:?}\n{new}",
                surface(&new)
            );
        }
        // a file that comes or goes with public items
        assert!(changed(None, Some(BASE)));
        assert!(changed(Some(BASE), None));
        assert!(!changed(None, Some("fn private() {}\n")));
    }

    #[test]
    fn strings_and_comments_do_not_fool_the_brackets() {
        let src = "pub const A: &str = \"{ // not a comment\";\npub fn b() -> char { '{' }\n/* pub fn c() {} */\npub const R: &str = r#\"}\"#;\n";
        assert_eq!(
            surface(src),
            vec![
                "pubconstA:&str=\"{//notacomment\";".to_string(),
                "pubfnb()->char".into(),
                "pubconstR:&str=r#\"}\"#;".into(),
            ]
        );
    }
}
