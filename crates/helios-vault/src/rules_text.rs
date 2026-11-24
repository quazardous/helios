//! The rules' text form (`docs/design/library-search.md`, section 4): one
//! grammar for the search field, smart albums, workspace queries and
//! `library.*` commands. [`parse_text`] reads it into a rule tree,
//! [`to_text`] writes a tree back; a tree with an `all` group at its top
//! reads back as it was written.
//!
//! ```text
//! query     = alt ( "OR" alt )*             several alts: any of them
//! alt       = term ( ws term )*            all terms hold
//! term      = [ "-" | "+" ] ( qualified | "#" tag | word | "(" query ")" )
//! qualified = key ( ":" [ op ] value | op value )
//! op        = ">=" | "<=" | ">" | "<" | "!=" | "=" | "~"
//! value     = bare | "\"" quoted "\"" | low ".." high
//! ```
//!
//! - Terms all hold, but one key repeated among the plain terms of one
//!   level means any of them for a key a photo has one value of, or
//!   one of a list (`label:red label:blue`; the [`ANY_KEYS`]); `+` keeps a
//!   term out of that (`+label:red +label:blue`: both labels).
//! - `-` negates a term (`-flag:reject`, `-rating:>=3` is `rating:<3`,
//!   `-(a OR b)` is a `none` group).
//! - `(a OR b)` is an `any` group, `(a b)` a nested `all` group.
//! - Values: a number or a range (`rating:3` is at least 3, `iso:<=800`,
//!   `f:1.4..2.8`, `shutter:<1/250`); a date or a range of whole days
//!   (`captured:2024-06`, `captured:2024-06-01..2024-06-30`,
//!   `captured:>=2024`), an instant (`captured:>2024-06-01T10:00:00`), a
//!   relative period (`captured:today`, `this-year`, `last:30d`); a text
//!   exact by default, `~` for contains (`camera:~Z`); `title` and
//!   `description` contain by default, `=` for exact.
//! - Aliases ([`ALIASES`]): `stars` for `rating`, `collection` for
//!   `album`, `date` for `captured`, `f` for `aperture`, `file_type` for
//!   `type`, `text` for a word; `from:` and `to:` take a day.
//! - An unknown key, a bad value or a stray character stays a word,
//!   searched in the photo's name and tags, as the Library always did.
//!   Unbalanced parentheses, and a tree past [`MAX_DEPTH`] or
//!   [`MAX_CONDITIONS`](crate::rules::MAX_CONDITIONS), are errors.
//!
//! The parser is written by hand: the grammar is small, and its leniency
//! (whatever is not a term is a word, never an error) is easier to say in
//! code than in a parser combinator's error recovery.

use serde_json::{Value, json};

use crate::capture::{civil, days_from_civil};
use crate::rules::{MAX_DEPTH, Match, Relative, Rule, RuleKind, Unit};

/// What went wrong reading a text, and where (a byte offset).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub at: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "at {}: {}", self.at, self.message)
    }
}

impl std::error::Error for ParseError {}

/// The text's keys and the rule field each names, as [`to_text`] writes
/// them.
pub const KEYS: &[(&str, &str)] = &[
    ("rating", "rating"),
    ("label", "label"),
    ("flag", "flag"),
    ("tag", "tag"),
    ("keyword", "keyword"),
    ("camera", "camera"),
    ("make", "make"),
    ("lens", "lens"),
    ("type", "file_type"),
    ("folder", "folder"),
    ("album", "album"),
    ("import", "import"),
    ("imported", "imported"),
    ("captured", "captured"),
    ("iso", "iso"),
    ("aperture", "aperture"),
    ("shutter", "shutter"),
    ("focal", "focal_length"),
    ("title", "title"),
    ("description", "description"),
    ("presence", "presence"),
    ("backup", "backup"),
    ("cull", "cull"),
    ("focus", "focus"),
    ("clip_white", "clip_white"),
    ("clip_black", "clip_black"),
    ("text", "text"),
];

/// Other names a key is typed with.
pub const ALIASES: &[(&str, &str)] = &[
    ("stars", "rating"),
    ("collection", "album"),
    ("date", "captured"),
    ("f", "aperture"),
    ("file_type", "type"),
    ("focal_length", "focal"),
];

/// The rule fields whose plain terms, repeated, mean any of them.
pub const ANY_KEYS: &[&str] = &[
    "label",
    "flag",
    "camera",
    "make",
    "lens",
    "file_type",
    "folder",
    "album",
    "import",
    "presence",
    "cull",
];

/// The rule field a typed key names (aliases included).
fn field_of(key: &str) -> Option<&'static str> {
    let key = ALIASES
        .iter()
        .find(|(a, _)| *a == key)
        .map_or(key, |(_, k)| k);
    KEYS.iter().find(|(k, _)| *k == key).map(|(_, f)| *f)
}

/// The key [`to_text`] writes for a rule field.
fn key_of(field: &str) -> &'static str {
    KEYS.iter()
        .find(|(_, f)| *f == field)
        .map_or("text", |(k, _)| k)
}

/// Reads a search text into a rule tree, an `all` group at its top.
///
/// # Errors
/// Unbalanced parentheses, groups nested past [`MAX_DEPTH`] or more than
/// [`MAX_CONDITIONS`](crate::rules::MAX_CONDITIONS) conditions.
pub fn parse_text(text: &str) -> Result<Rule, ParseError> {
    let mut p = Parser {
        s: text,
        at: 0,
        depth: 0,
    };
    let rule = match p.query()? {
        Alts::One(items) => Rule::group(Match::All, items),
        Alts::Many(alts) => Rule::group(Match::All, vec![Rule::group(Match::Any, alts)]),
    };
    if p.at < text.len() {
        return Err(ParseError {
            at: p.at,
            message: "a `)` closes no `(`".into(),
        });
    }
    Rule::parse(&rule.to_json()).map_err(|message| ParseError { at: 0, message })
}

/// Writes a rule tree as text, as [`parse_text`] reads it back. A tree
/// whose top is not an `all` group is written as the one term of one.
pub fn to_text(rule: &Rule) -> String {
    match rule {
        Rule::Group {
            mode: Match::All,
            rules,
        } => items_text(rules),
        other => term_text(other),
    }
}

enum Alts {
    One(Vec<Rule>),
    Many(Vec<Rule>),
}

struct Parser<'a> {
    s: &'a str,
    at: usize,
    depth: usize,
}

/// One term read, before the merging of repeated keys.
struct Item {
    rule: Rule,
    /// A plain positive condition on one of the [`ANY_KEYS`], not marked
    /// `+`: repeated, they merge into an `any` group.
    mergeable: bool,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.s[self.at..]
    }

    fn skip_ws(&mut self) {
        let t = self.rest();
        self.at += t.len() - t.trim_start().len();
    }

    fn error(&self, message: &str) -> ParseError {
        ParseError {
            at: self.at,
            message: message.into(),
        }
    }

    /// Whether the next chunk is the word `OR`.
    fn at_or(&self) -> bool {
        let t = self.rest();
        t.starts_with("OR")
            && t[2..]
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || c == ')')
    }

    /// Alternatives up to the end or a `)`.
    fn query(&mut self) -> Result<Alts, ParseError> {
        let mut alts = vec![self.alt()?];
        while self.at_or() {
            self.at += 2;
            alts.push(self.alt()?);
        }
        if alts.len() == 1 {
            return Ok(Alts::One(alts.pop().unwrap_or_default()));
        }
        Ok(Alts::Many(
            alts.into_iter()
                .filter(|items| !items.is_empty())
                .map(|mut items| {
                    if items.len() == 1 {
                        items
                            .pop()
                            .unwrap_or_else(|| Rule::group(Match::All, vec![]))
                    } else {
                        Rule::group(Match::All, items)
                    }
                })
                .collect(),
        ))
    }

    /// Terms up to the end, a `)` or an `OR`, repeated keys merged.
    fn alt(&mut self) -> Result<Vec<Rule>, ParseError> {
        let mut items: Vec<Item> = Vec::new();
        loop {
            self.skip_ws();
            if self.rest().is_empty() || self.rest().starts_with(')') || self.at_or() {
                break;
            }
            if let Some(item) = self.term()? {
                items.push(item);
            }
        }
        Ok(merge(items))
    }

    fn term(&mut self) -> Result<Option<Item>, ParseError> {
        let t = self.rest();
        let next_is_term = |c: Option<char>| {
            c.is_some_and(|c| !c.is_whitespace() && !matches!(c, ')' | '-' | '+'))
        };
        if t.starts_with('-') && next_is_term(t[1..].chars().next()) {
            self.at += 1;
            return Ok(self.term()?.map(|inner| Item {
                rule: negate(inner.rule),
                mergeable: false,
            }));
        }
        if t.starts_with('+') && next_is_term(t[1..].chars().next()) {
            self.at += 1;
            return Ok(self.term()?.map(|inner| Item {
                rule: inner.rule,
                mergeable: false,
            }));
        }
        if t.starts_with('(') {
            return self.group().map(Some);
        }
        let start = self.at;
        let segments = self.chunk();
        let raw = &self.s[start..self.at];
        if let Some(item) = qualified(&segments) {
            return Ok(Some(item));
        }
        let text = if segments.first().is_some_and(|s| s.quoted) {
            segments.iter().map(|s| s.text.as_str()).collect()
        } else {
            raw.to_string()
        };
        Ok(word(&text).map(|rule| Item {
            rule,
            mergeable: false,
        }))
    }

    fn group(&mut self) -> Result<Item, ParseError> {
        let open = self.at;
        self.at += 1;
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error(&format!("groups nest at most {MAX_DEPTH} deep")));
        }
        let rule = match self.query()? {
            Alts::One(items) => Rule::group(Match::All, items),
            Alts::Many(alts) => Rule::group(Match::Any, alts),
        };
        if !self.rest().starts_with(')') {
            return Err(ParseError {
                at: open,
                message: "a `(` is not closed".into(),
            });
        }
        self.at += 1;
        self.depth -= 1;
        Ok(Item {
            rule,
            mergeable: false,
        })
    }

    /// The text up to a space or a `)` outside quotes, cut into plain and
    /// quoted segments (an unclosed quote runs to the end).
    fn chunk(&mut self) -> Vec<Segment> {
        let mut out: Vec<Segment> = Vec::new();
        let mut chars = self.rest().char_indices().peekable();
        let base = self.at;
        let mut end = self.rest().len();
        let mut plain = String::new();
        while let Some((i, c)) = chars.next() {
            if c.is_whitespace() || c == ')' {
                end = i;
                break;
            }
            if c == '"' {
                if !plain.is_empty() {
                    out.push(Segment::plain(std::mem::take(&mut plain)));
                }
                // Backslashes are kept as typed (a Windows path), except a
                // run of them before a `"`: halved, and an odd one out
                // makes that `"` a character, not the end (the quoting
                // rule of Windows command lines, so `"C:\Photos\"` is
                // written `"C:\Photos\\"`).
                let mut quoted = String::new();
                let mut run = 0usize;
                for (_, q) in chars.by_ref() {
                    match q {
                        '\\' => run += 1,
                        '"' => {
                            quoted.extend(std::iter::repeat_n('\\', run / 2));
                            if run.is_multiple_of(2) {
                                run = 0;
                                break;
                            }
                            run = 0;
                            quoted.push('"');
                        }
                        q => {
                            quoted.extend(std::iter::repeat_n('\\', run));
                            run = 0;
                            quoted.push(q);
                        }
                    }
                }
                quoted.extend(std::iter::repeat_n('\\', run));
                out.push(Segment {
                    text: quoted,
                    quoted: true,
                });
                continue;
            }
            plain.push(c);
        }
        if !plain.is_empty() {
            out.push(Segment::plain(plain));
        }
        self.at = base + end;
        out
    }
}

struct Segment {
    text: String,
    quoted: bool,
}

impl Segment {
    fn plain(text: String) -> Segment {
        Segment {
            text,
            quoted: false,
        }
    }
}

/// Repeated plain terms of one of the [`ANY_KEYS`] become one `any`
/// group, where the first of them was.
fn merge(items: Vec<Item>) -> Vec<Rule> {
    let key = |i: &Item| match &i.rule {
        Rule::Condition(c) if i.mergeable => Some(c.field.name),
        _ => None,
    };
    let mut out: Vec<Rule> = Vec::new();
    let mut groups: Vec<(&'static str, usize)> = Vec::new();
    let counts = |k: &str| items.iter().filter(|i| key(i) == Some(k)).count();
    for item in &items {
        match key(item) {
            Some(k) if counts(k) > 1 => {
                if let Some((_, at)) = groups.iter().find(|(g, _)| *g == k) {
                    if let Rule::Group { rules, .. } = &mut out[*at] {
                        rules.push(item.rule.clone());
                    }
                } else {
                    groups.push((k, out.len()));
                    out.push(Rule::group(Match::Any, vec![item.rule.clone()]));
                }
            }
            _ => out.push(item.rule.clone()),
        }
    }
    out
}

/// A term negated: a condition by its opposite operator when it has one,
/// anything else in a `none` group.
fn negate(rule: Rule) -> Rule {
    if let Rule::Condition(c) = &rule {
        let opposite = match c.op {
            "is" => Some("is_not"),
            "is_not" => Some("is"),
            "has" => Some("lacks"),
            "lacks" => Some("has"),
            "in" => Some("not_in"),
            "not_in" => Some("in"),
            "=" => Some("!="),
            "!=" => Some("="),
            "<" => Some(">="),
            ">=" => Some("<"),
            ">" => Some("<="),
            "<=" => Some(">"),
            _ => None,
        };
        if let Some(op) = opposite
            && let Ok(r) = Rule::condition(c.field.name, op, c.value.clone())
        {
            return r;
        }
    }
    match rule {
        Rule::Group {
            mode: Match::Any,
            rules,
        } => Rule::group(Match::None, rules),
        Rule::Group {
            mode: Match::All,
            mut rules,
        } if rules.len() == 1 => Rule::group(Match::None, vec![rules.remove(0)]),
        other => Rule::group(Match::None, vec![other]),
    }
}

/// A word searched in the name and tags; none for a blank one.
fn word(text: &str) -> Option<Rule> {
    Rule::condition("text", "contains", json!(text)).ok()
}

/// A `key:value` (or `key>=value`, `#tag`) term, when the key is known
/// and the value fits it.
fn qualified(segments: &[Segment]) -> Option<Item> {
    let first = segments.first().filter(|s| !s.quoted)?;
    let tail: String = segments[1..].iter().map(|s| s.text.as_str()).collect();
    let value_quoted = segments.len() == 2 && segments[1].quoted;
    let (field, rest) = if let Some(tag) = first.text.strip_prefix('#') {
        ("tag", format!(":{tag}"))
    } else {
        let end = first
            .text
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(first.text.len());
        let key = first.text[..end].to_lowercase();
        let rest = first.text[end..].to_string();
        match key.as_str() {
            "from" | "to" => {
                let day = rest.strip_prefix(':')?.to_string() + &tail;
                let (start, end, _) = period(&day).filter(|p| p.2)?;
                let rule = if key == "from" {
                    Rule::condition("captured", ">=", json!(start))
                } else {
                    Rule::condition("captured", "<", json!(end))
                };
                return rule.ok().map(|rule| Item {
                    rule,
                    mergeable: false,
                });
            }
            _ => (field_of(&key)?, rest),
        }
    };
    // `key:` then an operator, or the operator straight after the key.
    let (op, plain) = if let Some(after) = rest.strip_prefix(':') {
        split_op(after)
    } else {
        let (op, plain) = split_op(&rest);
        op?;
        (op, plain)
    };
    let value = format!("{plain}{tail}");
    if value.is_empty() {
        return None;
    }
    let spec = crate::rules::rule_field(field).ok()?;
    let rule = condition(
        spec.name,
        spec.kind,
        op,
        &value,
        value_quoted && plain.is_empty(),
    )?;
    let mergeable = ANY_KEYS.contains(&spec.name)
        && matches!(&rule, Rule::Condition(c) if matches!(c.op, "is" | "has" | "in"));
    Some(Item { rule, mergeable })
}

/// An operator at the start of a value, and the value after it.
fn split_op(text: &str) -> (Option<&'static str>, &str) {
    for op in [">=", "<=", "!=", ">", "<", "=", "~"] {
        if let Some(rest) = text.strip_prefix(op) {
            return (Some(op), rest);
        }
    }
    (None, text)
}

/// The condition a key's operator and value make; `quoted`: the value
/// was typed in quotes whole (a text then, never a range).
fn condition(
    field: &'static str,
    kind: RuleKind,
    op: Option<&'static str>,
    value: &str,
    quoted: bool,
) -> Option<Rule> {
    let c = |op: &str, v: Value| Rule::condition(field, op, v).ok();
    let lower = value.to_lowercase();
    match kind {
        RuleKind::Number => {
            if !quoted && let Some((low, high)) = value.split_once("..") {
                if op.is_some() {
                    return None;
                }
                return match (number(low), number(high)) {
                    (Some(l), Some(h)) => c("between", json!([l, h])),
                    (Some(l), None) if high.is_empty() => c(">=", l),
                    (None, Some(h)) if low.is_empty() => c("<=", h),
                    _ => None,
                };
            }
            let mut n = number(value)?;
            let op = match op {
                None if field == "rating" => ">=",
                None => "=",
                Some("~") => return None,
                Some(op) => op,
            };
            // As the Library always read it, more stars than there are
            // is all of them.
            if field == "rating" && op == ">=" && n.as_f64().is_some_and(|v| v > 5.0) {
                n = json!(5);
            }
            c(op, n)
        }
        RuleKind::Date | RuleKind::Imported => date_condition(field, op, value),
        RuleKind::Text => {
            let contains_default = matches!(field, "title" | "description");
            let op = match op {
                None if contains_default => "contains",
                Some("~") => "contains",
                None | Some("=") => "is",
                Some("!=") => "is_not",
                Some(_) => return None,
            };
            let v = if field == "file_type" {
                lower
            } else {
                value.to_string()
            };
            c(op, json!(v))
        }
        RuleKind::Set | RuleKind::Keyword if op.is_none() => {
            let v = if field == "label" {
                lower
            } else {
                value.to_string()
            };
            c("has", json!(v))
        }
        RuleKind::Flag | RuleKind::Presence | RuleKind::Backup | RuleKind::Cull => {
            let v = if kind == RuleKind::Backup && lower == "ok" {
                "met".to_string()
            } else {
                lower
            };
            match op {
                None | Some("=") => c("is", json!(v)),
                Some("!=") => c("is_not", json!(v)),
                Some(_) => None,
            }
        }
        RuleKind::Folder | RuleKind::Album | RuleKind::Import if op.is_none() => {
            c("in", json!(value))
        }
        RuleKind::Words if op.is_none() => c("contains", json!(value)),
        _ => None,
    }
}

/// A number as typed (`3`, `2.8`, `1/250`), kept whole when it is one.
fn number(text: &str) -> Option<Value> {
    if let Ok(n) = text.parse::<i64>() {
        return Some(json!(n));
    }
    let n = match text.split_once('/') {
        Some((a, b)) => a.parse::<f64>().ok()? / b.parse::<f64>().ok()?,
        None => text.parse::<f64>().ok()?,
    };
    n.is_finite().then(|| json!(n))
}

/// A date's period `[start, end)` in seconds, and whether it is whole
/// days (`2024`, `2024-06`, `2024-06-01`) rather than an instant
/// (`2024-06-01T10:00:00`, `@<seconds>`: then `end` is `start`).
fn period(text: &str) -> Option<(i64, i64, bool)> {
    if let Some(raw) = text.strip_prefix('@') {
        let n = raw.parse::<i64>().ok()?;
        return Some((n, n, false));
    }
    let (day, time) = match text.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (text, None),
    };
    let parts: Vec<&str> = day.split('-').collect();
    let num = |s: &str, len: usize| (s.len() == len).then(|| s.parse::<i64>().ok()).flatten();
    let year = num(parts.first()?, 4)?;
    let month = parts.get(1).map_or(Some(0), |m| num(m, 2))?;
    let dayn = parts.get(2).map_or(Some(0), |d| num(d, 2))?;
    if parts.len() > 3 || (parts.len() > 1 && !(1..=12).contains(&month)) {
        return None;
    }
    if parts.len() > 2 && !(1..=31).contains(&dayn) {
        return None;
    }
    let days = |y: i64, m: i64, d: i64| days_from_civil(y, m as u32, d as u32) * 86_400;
    if let Some(time) = time {
        if parts.len() != 3 {
            return None;
        }
        let hms: Vec<&str> = time.split(':').collect();
        let [h, m, s] = hms[..] else { return None };
        let (h, m, s) = (num(h, 2)?, num(m, 2)?, num(s, 2)?);
        if h > 23 || m > 59 || s > 59 {
            return None;
        }
        let t = days(year, month, dayn) + h * 3600 + m * 60 + s;
        return Some((t, t, false));
    }
    Some(match parts.len() {
        1 => (days(year, 1, 1), days(year + 1, 1, 1), true),
        2 => {
            let (ny, nm) = if month == 12 {
                (year + 1, 1)
            } else {
                (year, month + 1)
            };
            (days(year, month, 1), days(ny, nm, 1), true)
        }
        _ => {
            let start = days(year, month, dayn);
            (start, start + 86_400, true)
        }
    })
}

fn date_condition(field: &'static str, op: Option<&'static str>, value: &str) -> Option<Rule> {
    let c = |op: &str, v: Value| Rule::condition(field, op, v).ok();
    if let Some(raw) = value.strip_prefix('@')
        && !value.contains("..")
    {
        let n = number(raw)?;
        return match op {
            None => c("=", n),
            Some("~") => None,
            Some(op) => c(op, n),
        };
    }
    if op.is_none() {
        let relative = match value.to_lowercase().as_str() {
            "today" => Some(Relative::Today),
            "this-year" | "this_year" => Some(Relative::ThisYear),
            v => v.strip_prefix("last:").and_then(|n| {
                let split = n.find(|c: char| !c.is_ascii_digit())?;
                let count: u32 = n[..split].parse().ok()?;
                let unit = match &n[split..] {
                    "d" | "days" => Unit::Days,
                    "w" | "weeks" => Unit::Weeks,
                    "m" | "months" => Unit::Months,
                    _ => return None,
                };
                Some(Relative::Last(count, unit))
            }),
        };
        if let Some(r) = relative {
            return c("within", r.to_json());
        }
        if let Some((low, high)) = value.split_once("..") {
            let low = if low.is_empty() {
                None
            } else {
                Some(period(low)?)
            };
            let high = if high.is_empty() {
                None
            } else {
                Some(period(high)?)
            };
            return match (low, high) {
                (Some(l), Some(h)) => {
                    let h = if h.2 { h.1 - 1 } else { h.1 };
                    c("between", json!([l.0, h]))
                }
                (Some(l), None) => c(">=", json!(l.0)),
                (None, Some(h)) => c("<", json!(if h.2 { h.1 } else { h.1 + 1 })),
                (None, None) => None,
            };
        }
    }
    let (start, end, whole) = period(value)?;
    if !whole {
        return match op {
            None => c("=", json!(start)),
            Some("~") => None,
            Some(op) => c(op, json!(start)),
        };
    }
    match op {
        None | Some("=") => c("between", json!([start, end - 1])),
        Some(">=") => c(">=", json!(start)),
        Some(">") => c(">=", json!(end)),
        Some("<") => c("<", json!(start)),
        Some("<=") => c("<", json!(end)),
        Some("!=") => c("between", json!([start, end - 1])).map(|r| negate_group(vec![r])),
        _ => None,
    }
}

fn negate_group(rules: Vec<Rule>) -> Rule {
    Rule::group(Match::None, rules)
}

// --- Writing -------------------------------------------------------------

/// A level's terms, space-separated: repeated plain terms of one of the
/// [`ANY_KEYS`] marked `+` (they would read back as any of them), an
/// `any` group of one key's plain terms written as those terms when
/// nothing else of that key is there.
fn items_text(rules: &[Rule]) -> String {
    let plain_key = |r: &Rule| match r {
        Rule::Condition(c) if mergeable(c) => Some(c.field.name),
        _ => None,
    };
    let count = |k: &str| rules.iter().filter(|r| plain_key(r) == Some(k)).count();
    let mut bare_anys: Vec<&str> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for r in rules {
        if let Some(k) = plain_key(r) {
            let t = term_text(r);
            out.push(if count(k) > 1 { format!("+{t}") } else { t });
            continue;
        }
        if let Rule::Group {
            mode: Match::Any,
            rules: members,
        } = r
            && members.len() > 1
            && let Some(k) = plain_key(&members[0])
            && members.iter().all(|m| plain_key(m) == Some(k))
            && count(k) == 0
            && !bare_anys.contains(&k)
        {
            bare_anys.push(k);
            out.extend(members.iter().map(term_text));
            continue;
        }
        out.push(term_text(r));
    }
    out.join(" ")
}

fn mergeable(c: &crate::rules::Condition) -> bool {
    ANY_KEYS.contains(&c.field.name) && matches!(c.op, "is" | "has" | "in")
}

/// One alternative of an `any` or `none` group.
fn alt_text(rule: &Rule) -> String {
    match rule {
        Rule::Group {
            mode: Match::All,
            rules,
        } if rules.len() > 1 => items_text(rules),
        other => term_text(other),
    }
}

fn term_text(rule: &Rule) -> String {
    match rule {
        Rule::Condition(c) => condition_text(c),
        Rule::Group {
            mode: Match::All,
            rules,
        } => format!("({})", items_text(rules)),
        Rule::Group {
            mode: Match::Any,
            rules,
        } => {
            if rules.is_empty() {
                "(OR)".into()
            } else if rules.len() == 1 {
                // One alternative alone reads back as an `all` group: a
                // trailing `OR` keeps it an `any`.
                format!("({} OR)", alt_text(&rules[0]))
            } else {
                let alts: Vec<String> = rules.iter().map(alt_text).collect();
                format!("({})", alts.join(" OR "))
            }
        }
        Rule::Group {
            mode: Match::None,
            rules,
        } => {
            if let [Rule::Condition(c)] = &rules[..]
                && !invertible(c.op)
            {
                return format!("-{}", condition_text(c));
            }
            if rules.is_empty() {
                return "-(OR)".into();
            }
            let alts: Vec<String> = rules.iter().map(alt_text).collect();
            format!("-({})", alts.join(" OR "))
        }
    }
}

fn invertible(op: &str) -> bool {
    !matches!(op, "contains" | "between" | "within")
}

fn condition_text(c: &crate::rules::Condition) -> String {
    let key = key_of(c.field.name);
    let negative = matches!(c.op, "is_not" | "lacks" | "not_in");
    let minus = if negative { "-" } else { "" };
    match c.field.kind {
        RuleKind::Number => {
            let op = match c.op {
                "between" => {
                    let v = c.value.as_array().cloned().unwrap_or_default();
                    return format!("{key}:{}..{}", v[0], v[1]);
                }
                "=" if c.field.name != "rating" => "",
                op => op,
            };
            format!("{key}:{op}{}", c.value)
        }
        RuleKind::Date | RuleKind::Imported => date_text(key, c),
        RuleKind::Text => {
            let default_contains = matches!(c.field.name, "title" | "description");
            let op = match (c.op, default_contains) {
                ("contains", true) | ("is" | "is_not", false) => "",
                ("contains", false) => "~",
                _ => "=",
            };
            format!("{minus}{key}:{op}{}", value_text(c.text()))
        }
        RuleKind::Set if c.field.name == "tag" && value_text(c.text()) == c.text() => {
            format!("{minus}#{}", c.text())
        }
        RuleKind::Words => {
            let t = c.text();
            if bare_ok(t)
                && !t.contains([':', '<', '>', '='])
                && t != "OR"
                && !t.starts_with(['-', '+', '#'])
            {
                t.to_string()
            } else {
                quote(t)
            }
        }
        _ => format!("{minus}{key}:{}", value_text(c.text())),
    }
}

fn date_text(key: &str, c: &crate::rules::Condition) -> String {
    if c.op == "within" {
        let v = match Relative::parse(&c.value) {
            Ok(Relative::Today) => "today".to_string(),
            Ok(Relative::ThisYear) => "this-year".to_string(),
            Ok(Relative::Last(n, unit)) => format!("last:{n}{}", &unit.name()[..1]),
            Err(_) => String::new(),
        };
        return format!("{key}:{v}");
    }
    if c.op == "between" {
        let v = c.value.as_array().cloned().unwrap_or_default();
        let low = when(&v[0], |t| t.rem_euclid(86_400) == 0);
        let high = when(&v[1], |t| (t + 1).rem_euclid(86_400) == 0);
        return format!("{key}:{low}..{high}");
    }
    let day_ok = matches!(c.op, ">=" | "<");
    let op = if c.op == "=" { "" } else { c.op };
    let v = when(&c.value, |t| day_ok && t.rem_euclid(86_400) == 0);
    format!("{key}:{op}{v}")
}

/// A time as its day when `as_day` holds, else as an instant (`@` and
/// the number when it is not whole seconds).
fn when(value: &Value, as_day: impl Fn(i64) -> bool) -> String {
    let Some(t) = value.as_i64() else {
        return format!("@{value}");
    };
    let (y, m, d) = civil(t.div_euclid(86_400));
    if as_day(t) {
        return format!("{y:04}-{m:02}-{d:02}");
    }
    let s = t.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

/// Whether a text is written as it is, without quotes.
fn bare_ok(text: &str) -> bool {
    !text.is_empty()
        && !text
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '(' | ')'))
}

/// A text value after `key:`: quoted when it holds a space, a quote, a
/// parenthesis, or starts like an operator.
fn value_text(text: &str) -> String {
    if bare_ok(text) && !text.starts_with(['<', '>', '=', '!', '~', '@']) {
        text.to_string()
    } else {
        quote(text)
    }
}

/// A text in quotes, read back as it was: a `"` and the backslashes
/// right before it, or before the closing quote, are escaped; any other
/// backslash is written as it is (`"C:\Photos\2024 trip"`).
fn quote(text: &str) -> String {
    let mut out = String::from('"');
    let mut run = 0usize;
    for ch in text.chars() {
        match ch {
            '\\' => run += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', 2 * run + 1));
                out.push('"');
                run = 0;
            }
            ch => {
                out.extend(std::iter::repeat_n('\\', run));
                out.push(ch);
                run = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', 2 * run));
    out.push('"');
    out
}

#[cfg(test)]
#[path = "rules_text_tests.rs"]
mod tests;
