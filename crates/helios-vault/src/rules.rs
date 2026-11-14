//! Smart albums' rules (`docs/design/library.md`, L6): a tree of
//! conditions on what the vault knows of a photo, compiled by each store
//! into its query ([`crate::Query::rules`]), so a smart album pages, counts
//! and facets like any filter.
//!
//! A tree is written in JSON as a group, `{"match": "all", "rules": [...]}`
//! (`all`, `any` or `none` of its rules), each rule a condition,
//! `{"field": "rating", "op": ">=", "value": 3}`, or a nested group. The
//! fields, their kinds and the operators each kind takes are in
//! [`RULE_FIELDS`]; anything else is refused with its reason
//! ([`Rule::parse`]).
//!
//! The capture date also takes relative values, `{"field": "captured",
//! "op": "within", "value": {"last": 30, "unit": "days"}}` (or `"today"`,
//! `"this_year"`): kept relative in the tree, they are resolved against a
//! clock each time a query runs ([`resolve`], [`Relative`]).

use std::borrow::Cow;
use std::sync::atomic::{AtomicI64, Ordering};

use serde::{Serialize, Serializer};
use serde_json::{Value, json};

use crate::capture;
use crate::fields::{self, FLAGS, FieldKind};

/// How deep groups may nest.
pub const MAX_DEPTH: usize = 8;
/// How many conditions a tree may hold.
pub const MAX_CONDITIONS: usize = 200;

/// How a group combines its rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Match {
    /// Every rule holds (none: every photo).
    All,
    /// One rule holds at least (none: no photo).
    Any,
    /// No rule holds (none: every photo).
    None,
}

impl Match {
    pub const ALL: [Match; 3] = [Match::All, Match::Any, Match::None];

    pub fn name(self) -> &'static str {
        match self {
            Match::All => "all",
            Match::Any => "any",
            Match::None => "none",
        }
    }

    pub fn parse(name: &str) -> Option<Match> {
        Match::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// What a rule field holds, which decides its operators and values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleKind {
    /// A number: `=`, `!=`, `<`, `<=`, `>`, `>=`, `between` (`[low,
    /// high]`, both kept). A photo without the field never matches, but
    /// for the rating, which counts as 0.
    Number,
    /// A set of names (colour labels, tags): `has`, `lacks`.
    Set,
    /// A text: `is`, `is_not`, `contains` (whatever the case). A photo
    /// without it is not any text.
    Text,
    /// The culling flag: `is`, `is_not` one of [`FLAGS`].
    Flag,
    /// A folder (an absolute path): a copy `in` it or below, or `not_in`.
    Folder,
    /// An album (its id): `in`, `not_in`. A smart album holds no photo
    /// here: its rules are written out instead.
    Album,
    /// A capture time (seconds, the camera's clock read as UTC): a
    /// number's operators, and `within` a [`Relative`] period.
    Date,
    /// An import session of the import history (its id, a record of
    /// [`IMPORT_HISTORY`]): `in`, `not_in`, the photos it brought into
    /// the Library or found again.
    Import,
    /// When a photo was imported (seconds since the epoch, UTC): a
    /// number's operators on the time of the sessions that hold it, and
    /// `within` a [`Relative`] period. A photo in no session never
    /// matches.
    Imported,
}

/// The store's documents namespace the import history keeps its records
/// in (the library documents' `import/history`,
/// `helios_library::import_history::NS`), each with the photos it
/// brought in (`images`) and its time (`when`).
pub const IMPORT_HISTORY: &str = "library/import/history";

impl RuleKind {
    /// The operators the kind takes.
    pub fn ops(self) -> &'static [&'static str] {
        match self {
            RuleKind::Number => &["=", "!=", "<", "<=", ">", ">=", "between"],
            RuleKind::Date | RuleKind::Imported => {
                &["=", "!=", "<", "<=", ">", ">=", "between", "within"]
            }
            RuleKind::Set => &["has", "lacks"],
            RuleKind::Text => &["is", "is_not", "contains"],
            RuleKind::Flag => &["is", "is_not"],
            RuleKind::Folder | RuleKind::Album | RuleKind::Import => &["in", "not_in"],
        }
    }
}

/// A field a rule may test: its name in rules, the vault field it reads
/// (empty for the flag, a folder and an album) and its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuleField {
    pub name: &'static str,
    pub field: &'static str,
    pub kind: RuleKind,
}

const fn f(name: &'static str, field: &'static str, kind: RuleKind) -> RuleField {
    RuleField { name, field, kind }
}

/// The fields rules test.
pub const RULE_FIELDS: &[RuleField] = &[
    f("rating", fields::RATING, RuleKind::Number),
    f("label", fields::LABELS, RuleKind::Set),
    f("tag", fields::TAGS, RuleKind::Set),
    f("flag", "", RuleKind::Flag),
    f("folder", "", RuleKind::Folder),
    f("album", "", RuleKind::Album),
    f("captured", capture::CAPTURED, RuleKind::Date),
    f("import", "", RuleKind::Import),
    f("imported", "", RuleKind::Imported),
    f("camera", capture::CAMERA_MODEL, RuleKind::Text),
    f("make", capture::CAMERA_MAKE, RuleKind::Text),
    f("lens", capture::LENS, RuleKind::Text),
    f("file_type", capture::FILE_TYPE, RuleKind::Text),
    f("title", "title", RuleKind::Text),
    f("description", "description", RuleKind::Text),
    f("iso", capture::ISO, RuleKind::Number),
    f("focal_length", capture::FOCAL_LENGTH, RuleKind::Number),
    f("aperture", capture::APERTURE, RuleKind::Number),
    f("shutter", capture::SHUTTER, RuleKind::Number),
    f("focus", crate::cull::FOCUS, RuleKind::Number),
    f("clip_white", crate::cull::CLIP_WHITE, RuleKind::Number),
    f("clip_black", crate::cull::CLIP_BLACK, RuleKind::Number),
];

/// The rule field of that name.
pub fn rule_field(name: &str) -> Result<&'static RuleField, String> {
    RULE_FIELDS.iter().find(|f| f.name == name).ok_or_else(|| {
        let known: Vec<_> = RULE_FIELDS.iter().map(|f| f.name).collect();
        format!("unknown field `{name}` (known: {})", known.join(", "))
    })
}

/// One test on a photo; built checked by [`Rule::parse`] or
/// [`Condition::new`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Condition {
    pub field: &'static RuleField,
    pub op: &'static str,
    /// For `between`, `[low, high]`; a number, a text, else.
    pub value: Value,
}

impl Condition {
    /// A condition, checked: the field known, the operator one its kind
    /// takes, the value of the right type.
    pub fn new(field: &str, op: &str, value: Value) -> Result<Condition, String> {
        let spec = rule_field(field)?;
        let op = *spec.kind.ops().iter().find(|o| **o == op).ok_or_else(|| {
            format!(
                "`{field}` takes no operator `{op}` (it takes {})",
                spec.kind.ops().join(", ")
            )
        })?;
        let value = check_value(spec, op, value)?;
        Ok(Condition {
            field: spec,
            op,
            value,
        })
    }

    /// The value as a number (not `between`).
    pub fn number(&self) -> f64 {
        self.value.as_f64().unwrap_or(0.0)
    }

    /// `between`'s bounds.
    pub fn bounds(&self) -> (f64, f64) {
        let b = |i: usize| self.value.get(i).and_then(Value::as_f64).unwrap_or(0.0);
        (b(0), b(1))
    }

    /// The value as a text.
    pub fn text(&self) -> &str {
        self.value.as_str().unwrap_or_default()
    }

    fn json(&self) -> Value {
        json!({ "field": self.field.name, "op": self.op, "value": self.value })
    }
}

/// Checks a condition's value against its field and operator, in the
/// form the stores compare.
fn check_value(spec: &RuleField, op: &str, value: Value) -> Result<Value, String> {
    let name = spec.name;
    let number = |v: &Value| -> Result<f64, String> {
        let n = v
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or_else(|| format!("`{name}`: {v} is not a number"))?;
        if let Ok(fields::FieldSpec {
            kind: FieldKind::Int { min, max },
            ..
        }) = fields::spec(spec.field)
            && !(*min as f64..=*max as f64).contains(&n)
        {
            return Err(format!("`{name}`: {n} is not in [{min}, {max}]"));
        }
        Ok(n)
    };
    let text = |v: &Value| -> Result<String, String> {
        v.as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from)
            .ok_or_else(|| format!("`{name}`: {v} is not a text"))
    };
    match spec.kind {
        RuleKind::Date | RuleKind::Imported if op == "within" => Relative::parse(&value)
            .map(Relative::to_json)
            .map_err(|e| format!("`{name}`: {e}")),
        RuleKind::Number | RuleKind::Date | RuleKind::Imported if op == "between" => {
            let pair = value
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or_else(|| format!("`{name}`: `between` takes [low, high], not {value}"))?;
            let (low, high) = (number(&pair[0])?, number(&pair[1])?);
            if low > high {
                return Err(format!("`{name}`: {low} is above {high}"));
            }
            Ok(json!([pair[0], pair[1]]))
        }
        RuleKind::Number | RuleKind::Date | RuleKind::Imported => {
            number(&value)?;
            Ok(value)
        }
        RuleKind::Set => {
            let item = text(&value)?;
            if let Ok(fields::FieldSpec {
                kind: FieldKind::Labels(allowed),
                ..
            }) = fields::spec(spec.field)
                && !allowed.contains(&item.as_str())
            {
                return Err(format!(
                    "`{name}`: `{item}` is not one of {}",
                    allowed.join(", ")
                ));
            }
            Ok(Value::from(item))
        }
        RuleKind::Text | RuleKind::Album | RuleKind::Import => Ok(Value::from(text(&value)?)),
        RuleKind::Flag => {
            let flag = text(&value)?;
            if !FLAGS.contains(&flag.as_str()) {
                return Err(format!(
                    "`{name}`: `{flag}` is not a flag ({})",
                    FLAGS.join(", ")
                ));
            }
            Ok(Value::from(flag))
        }
        RuleKind::Folder => {
            let path = text(&value)?;
            if !std::path::Path::new(&path).is_absolute() {
                return Err(format!("`{name}`: `{path}` is not an absolute path"));
            }
            Ok(Value::from(path))
        }
    }
}

/// A smart album's rules: a condition, or a group of rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rule {
    Group { mode: Match, rules: Vec<Rule> },
    Condition(Condition),
}

impl Rule {
    /// A group of rules.
    pub fn group(mode: Match, rules: Vec<Rule>) -> Rule {
        Rule::Group { mode, rules }
    }

    /// A condition, checked ([`Condition::new`]).
    pub fn condition(field: &str, op: &str, value: Value) -> Result<Rule, String> {
        Condition::new(field, op, value).map(Rule::Condition)
    }

    /// A tree from its JSON, checked: a group at the top, groups nested
    /// at most [`MAX_DEPTH`] deep, at most [`MAX_CONDITIONS`] conditions;
    /// the first fault found is the error, with where it is.
    pub fn parse(json: &Value) -> Result<Rule, String> {
        let mut count = 0;
        let rule = parse_at(json, 1, "rules", &mut count)?;
        if !matches!(rule, Rule::Group { .. }) {
            return Err("the rules must be a group: {\"match\": …, \"rules\": […]}".into());
        }
        Ok(rule)
    }

    /// Its JSON, as [`Rule::parse`] reads it.
    pub fn to_json(&self) -> Value {
        match self {
            Rule::Group { mode, rules } => json!({
                "match": mode.name(),
                "rules": rules.iter().map(Rule::to_json).collect::<Vec<_>>(),
            }),
            Rule::Condition(c) => c.json(),
        }
    }

    /// Both trees at once: `self` and `other` in one `all` group.
    pub fn and(self, other: Rule) -> Rule {
        Rule::group(Match::All, vec![self, other])
    }

    /// Whether the tree holds a relative date (`within`).
    pub fn is_relative(&self) -> bool {
        match self {
            Rule::Condition(c) => c.op == "within",
            Rule::Group { rules, .. } => rules.iter().any(Rule::is_relative),
        }
    }

    /// The tree with each relative date resolved against `now` (the
    /// camera's clock read as UTC, in seconds): `within` becomes the
    /// range `[start, end)` of [`Relative::range`], as two conditions.
    pub fn at(&self, now: i64) -> Rule {
        match self {
            Rule::Group { mode, rules } => Rule::Group {
                mode: *mode,
                rules: rules.iter().map(|r| r.at(now)).collect(),
            },
            Rule::Condition(c) if c.op == "within" => {
                let (mut start, mut end) =
                    Relative::parse(&c.value).map_or((0, 0), |r| r.range(now));
                // An import's time is UTC, not a camera's clock: the
                // local days, as UTC.
                if c.field.kind == RuleKind::Imported {
                    let offset = LOCAL_OFFSET.load(Ordering::Relaxed);
                    start -= offset;
                    end -= offset;
                }
                let bound = |op: &'static str, at: i64| {
                    Rule::Condition(Condition {
                        field: c.field,
                        op,
                        value: json!(at),
                    })
                };
                Rule::group(Match::All, vec![bound(">=", start), bound("<", end)])
            }
            other @ Rule::Condition(_) => other.clone(),
        }
    }

    /// Whether `test` holds for the tree, each condition answered by
    /// `test`: how the memory store reads a tree.
    pub fn holds(&self, test: &mut impl FnMut(&Condition) -> bool) -> bool {
        match self {
            Rule::Condition(c) => test(c),
            Rule::Group { mode, rules } => match mode {
                Match::All => rules.iter().all(|r| r.holds(test)),
                Match::Any => rules.iter().any(|r| r.holds(test)),
                Match::None => !rules.iter().any(|r| r.holds(test)),
            },
        }
    }
}

fn parse_at(json: &Value, depth: usize, at: &str, count: &mut usize) -> Result<Rule, String> {
    let Some(obj) = json.as_object() else {
        return Err(format!("{at}: {json} is not a rule"));
    };
    if obj.contains_key("match") || obj.contains_key("rules") {
        if depth > MAX_DEPTH {
            return Err(format!("{at}: groups nest at most {MAX_DEPTH} deep"));
        }
        if let Some(k) = obj.keys().find(|k| *k != "match" && *k != "rules") {
            return Err(format!("{at}: a group takes no `{k}`"));
        }
        let mode = obj
            .get("match")
            .and_then(Value::as_str)
            .and_then(Match::parse)
            .ok_or_else(|| format!("{at}: `match` is all, any or none"))?;
        let list = obj
            .get("rules")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{at}: `rules` is a list"))?;
        let rules = list
            .iter()
            .enumerate()
            .map(|(i, r)| parse_at(r, depth + 1, &format!("{at}[{i}]"), count))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Rule::Group { mode, rules });
    }
    if let Some(k) = obj
        .keys()
        .find(|k| !["field", "op", "value"].contains(&k.as_str()))
    {
        return Err(format!("{at}: a condition takes no `{k}`"));
    }
    *count += 1;
    if *count > MAX_CONDITIONS {
        return Err(format!("at most {MAX_CONDITIONS} conditions"));
    }
    let field = obj
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{at}: a condition needs a `field`"))?;
    let op = obj
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{at}: a condition needs an `op`"))?;
    let value = obj
        .get("value")
        .cloned()
        .ok_or_else(|| format!("{at}: a condition needs a `value`"))?;
    Condition::new(field, op, value)
        .map(Rule::Condition)
        .map_err(|e| format!("{at}: {e}"))
}

impl Serialize for Rule {
    fn serialize<Z: Serializer>(&self, s: Z) -> Result<Z::Ok, Z::Error> {
        self.to_json().serialize(s)
    }
}

/// A query as a store runs it: when its `album` is a smart album, the
/// album's rules in its place (with the query's own rules, if any); when
/// it is an album set, the union of its members as rules
/// ([`crate::collections::set_rule`]); `album` conditions naming a set
/// written out the same way. `docs` reads the collections' documents,
/// only when the query names an album. A smart album whose rules no
/// longer parse is an error. Relative dates are then resolved against
/// the query's clock (`Query::now`), else [`now`].
pub fn resolve(
    query: &crate::Query,
    docs: impl FnOnce() -> Result<Vec<(String, Value)>, String>,
) -> Result<Cow<'_, crate::Query>, String> {
    let mut resolved = smart(query, docs)?;
    if resolved.rules.as_ref().is_some_and(Rule::is_relative) {
        let at = resolved.now.unwrap_or_else(now);
        let q = resolved.to_mut();
        q.rules = q.rules.as_ref().map(|r| r.at(at));
    }
    Ok(resolved)
}

fn smart(
    query: &crate::Query,
    docs: impl FnOnce() -> Result<Vec<(String, Value)>, String>,
) -> Result<Cow<'_, crate::Query>, String> {
    use crate::collections::{SET, SMART, expand_sets, names_album, set_rule};
    let rules_name_album = query.rules.as_ref().is_some_and(names_album);
    if query.album.is_none() && !rules_name_album {
        return Ok(Cow::Borrowed(query));
    }
    let docs = docs()?;
    let mut q = Cow::Borrowed(query);
    if rules_name_album && let Some(own) = &query.rules {
        let expanded = expand_sets(own, &docs)?;
        if &expanded != own {
            q.to_mut().rules = Some(expanded);
        }
    }
    let Some(id) = &query.album else {
        return Ok(q);
    };
    let kind = docs
        .iter()
        .find(|(k, _)| k == id)
        .and_then(|(_, d)| d.get("kind").and_then(Value::as_str));
    if !matches!(kind, Some(SMART | SET)) {
        return Ok(q);
    }
    let Some(rules) = set_rule(id, &docs)? else {
        return Ok(q);
    };
    let mut q = q.into_owned();
    q.album = None;
    q.rules = Some(match q.rules.take() {
        Some(own) => own.and(rules),
        None => rules,
    });
    Ok(Cow::Owned(q))
}

/// The unit of a `last` period.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Days,
    Weeks,
    Months,
}

impl Unit {
    pub const ALL: [Unit; 3] = [Unit::Days, Unit::Weeks, Unit::Months];

    pub fn name(self) -> &'static str {
        match self {
            Unit::Days => "days",
            Unit::Weeks => "weeks",
            Unit::Months => "months",
        }
    }

    pub fn parse(name: &str) -> Option<Unit> {
        Unit::ALL.into_iter().find(|u| u.name() == name)
    }
}

/// The most a `last` period counts.
pub const MAX_LAST: u32 = 10_000;

/// A period relative to the day a query runs, by the capture clock's
/// days: `"today"`, `"this_year"`, or `{"last": n, "unit": "days" |
/// "weeks" | "months"}`, the `n` days (weeks, months) ending today, today
/// included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relative {
    Today,
    ThisYear,
    Last(u32, Unit),
}

impl Relative {
    /// The period a `within` value names.
    pub fn parse(value: &Value) -> Result<Relative, String> {
        let bad = || {
            format!(
                "`within` takes \"today\", \"this_year\" or {{\"last\": n, \"unit\": \"days\"}}, not {value}"
            )
        };
        match value {
            Value::String(s) if s == "today" => Ok(Relative::Today),
            Value::String(s) if s == "this_year" => Ok(Relative::ThisYear),
            Value::Object(o) => {
                if let Some(k) = o.keys().find(|k| *k != "last" && *k != "unit") {
                    return Err(format!("`within` takes no `{k}`"));
                }
                let n = o.get("last").and_then(Value::as_u64).ok_or_else(bad)?;
                if !(1..=u64::from(MAX_LAST)).contains(&n) {
                    return Err(format!("`last`: {n} is not in [1, {MAX_LAST}]"));
                }
                let unit = o
                    .get("unit")
                    .and_then(Value::as_str)
                    .and_then(Unit::parse)
                    .ok_or_else(|| "`unit` is days, weeks or months".to_string())?;
                Ok(Relative::Last(n as u32, unit))
            }
            _ => Err(bad()),
        }
    }

    pub fn to_json(self) -> Value {
        match self {
            Relative::Today => json!("today"),
            Relative::ThisYear => json!("this_year"),
            Relative::Last(n, unit) => json!({ "last": n, "unit": unit.name() }),
        }
    }

    /// The capture times `[start, end)` it covers on the day of `now`
    /// (seconds, the camera's clock read as UTC): whole days, up to the
    /// end of today. A month back from the 31st lands on its month's
    /// last day.
    pub fn range(self, now: i64) -> (i64, i64) {
        let today = now.div_euclid(86_400);
        let end = (today + 1) * 86_400;
        let start = match self {
            Relative::Today => today,
            Relative::ThisYear => {
                let (y, _, _) = capture::civil(today);
                capture::days_from_civil(y, 1, 1)
            }
            Relative::Last(n, Unit::Days) => today - i64::from(n) + 1,
            Relative::Last(n, Unit::Weeks) => today - 7 * i64::from(n) + 1,
            Relative::Last(n, Unit::Months) => {
                let (y, m, d) = capture::civil(today);
                let months = y * 12 + i64::from(m) - 1 - i64::from(n);
                let (y, m) = (months.div_euclid(12), months.rem_euclid(12) as u32 + 1);
                let first = capture::days_from_civil(y, m, 1);
                let next = if m == 12 {
                    capture::days_from_civil(y + 1, 1, 1)
                } else {
                    capture::days_from_civil(y, m + 1, 1)
                };
                // The same day `n` months back (its month's last at most),
                // then the day after.
                first + i64::from(d).min(next - first)
            }
        };
        (start * 86_400, end)
    }
}

/// The local clock's offset from UTC, in seconds ([`set_local_offset`]).
static LOCAL_OFFSET: AtomicI64 = AtomicI64::new(0);

/// Sets how far the local clock is from UTC (seconds, east positive), so
/// [`now`] reads the time as a camera set to local time would: an
/// application sets it once at its start.
pub fn set_local_offset(seconds: i64) {
    LOCAL_OFFSET.store(seconds, Ordering::Relaxed);
}

/// The time now as capture times count it: the system's clock, moved by
/// the local offset ([`set_local_offset`]).
pub fn now() -> i64 {
    let utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    utc + LOCAL_OFFSET.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_reads_back_as_written() {
        let json = json!({"match": "all", "rules": [
            {"field": "rating", "op": ">=", "value": 3},
            {"match": "any", "rules": [
                {"field": "label", "op": "has", "value": "red"},
                {"field": "captured", "op": "between", "value": [0, 86400]},
            ]},
        ]});
        let rule = Rule::parse(&json).unwrap();
        assert_eq!(rule.to_json(), json);
    }

    #[test]
    fn faults_are_refused_with_where_they_are() {
        let bad = |j: Value| Rule::parse(&j).unwrap_err();
        let e =
            bad(json!({"match": "all", "rules": [{"field": "colour", "op": "is", "value": 1}]}));
        assert!(
            e.contains("rules[0]") && e.contains("unknown field `colour`"),
            "{e}"
        );
        let e =
            bad(json!({"match": "all", "rules": [{"field": "rating", "op": "has", "value": 1}]}));
        assert!(e.contains("takes no operator `has`"), "{e}");
        let e = bad(json!({"match": "all", "rules": [{"field": "rating", "op": ">", "value": 9}]}));
        assert!(e.contains("not in [0, 5]"), "{e}");
        let e = bad(
            json!({"match": "all", "rules": [{"field": "label", "op": "has", "value": "pink"}]}),
        );
        assert!(e.contains("not one of"), "{e}");
        let e = bad(json!({"match": "some", "rules": []}));
        assert!(e.contains("all, any or none"), "{e}");
        let e = bad(json!({"field": "rating", "op": ">", "value": 1}));
        assert!(e.contains("must be a group"), "{e}");
        let e = bad(
            json!({"match": "all", "rules": [{"field": "folder", "op": "in", "value": "rel"}]}),
        );
        assert!(e.contains("absolute"), "{e}");
        let mut deep = json!({"match": "all", "rules": []});
        for _ in 0..MAX_DEPTH {
            deep = json!({"match": "all", "rules": [deep]});
        }
        assert!(bad(deep).contains("nest at most"));
        let e = bad(json!({"match": "all", "rules": [
            {"field": "captured", "op": "within", "value": {"last": 0, "unit": "days"}},
        ]}));
        assert!(e.contains("not in [1,"), "{e}");
        let e = bad(json!({"match": "all", "rules": [
            {"field": "rating", "op": "within", "value": "today"},
        ]}));
        assert!(e.contains("takes no operator `within`"), "{e}");
    }

    #[test]
    fn relative_dates_cover_whole_days_up_to_today() {
        let day = |t: &str| capture::day_start(t).unwrap();
        let now = day("2024-03-31") + 15 * 3600;
        let end = day("2024-04-01");
        assert_eq!(Relative::Today.range(now), (day("2024-03-31"), end));
        assert_eq!(Relative::ThisYear.range(now), (day("2024-01-01"), end));
        assert_eq!(
            Relative::Last(7, Unit::Days).range(now),
            (day("2024-03-25"), end)
        );
        assert_eq!(
            Relative::Last(2, Unit::Weeks).range(now),
            (day("2024-03-18"), end)
        );
        assert_eq!(
            Relative::Last(1, Unit::Months).range(now),
            (day("2024-03-01"), end),
            "from the day after a month back (Feb 29th)"
        );
        assert_eq!(
            Relative::Last(14, Unit::Months).range(now),
            (day("2023-02-01"), end),
            "January 31st 2023, then a day on"
        );
        let tree = Rule::parse(&json!({"match": "all", "rules": [
            {"field": "captured", "op": "within", "value": {"last": 7, "unit": "days"}},
        ]}))
        .unwrap();
        assert!(tree.is_relative());
        assert_eq!(
            tree.at(now).to_json(),
            json!({"match": "all", "rules": [{"match": "all", "rules": [
                {"field": "captured", "op": ">=", "value": day("2024-03-25")},
                {"field": "captured", "op": "<", "value": end},
            ]}]})
        );
    }
}
