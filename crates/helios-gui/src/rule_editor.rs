//! The smart album's rule editor (`docs/design/library.md`, L6): any rule
//! tree ([`helios_vault::rules`]) as a draft the "Edit the rules…" dialog
//! changes one edit at a time (a condition or a group added, removed or
//! moved, a group's match switched, a condition's field, operator or
//! value set), each edit undone and redone apart. A condition is kept as
//! typed, so one not valid yet shows the vault's reason beside it, and
//! the tree is applied only once every condition is valid.
//!
//! Agents drive it with `library.collection.rules.edit` ([`Edit::parse`]).

use helios_vault::capture::{day_start, day_text};
use helios_vault::rules::{Match, Relative, Rule, RuleKind, Unit, rule_field};
use serde_json::{Value, json};

/// Where a rule is: its index in each group from the top one (the top
/// group: empty).
pub type Path = Vec<usize>;

/// What a `within` condition's period box holds.
pub const PERIODS: [&str; 3] = ["today", "this_year", "last"];

/// A condition as typed: its field and operator, and its value's boxes
/// as text (`high`: `between`'s second bound; `unit`: a `last` period's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub field: String,
    pub op: String,
    pub value: String,
    pub high: String,
    pub unit: String,
}

/// A rule of the draft tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Group { mode: Match, rules: Vec<Node> },
    Condition(Draft),
}

/// Whether a field of `kind` is a time its boxes write as days.
fn is_date(kind: RuleKind) -> bool {
    matches!(kind, RuleKind::Date | RuleKind::Imported)
}

impl Draft {
    /// A new condition: at least one star.
    pub fn new() -> Draft {
        Draft {
            field: "rating".into(),
            op: ">=".into(),
            value: "1".into(),
            high: String::new(),
            unit: Unit::Days.name().into(),
        }
    }

    fn kind(&self) -> Option<RuleKind> {
        rule_field(&self.field).ok().map(|f| f.kind)
    }

    /// The value as the vault reads it, from the boxes: numbers; days
    /// (`YYYY-MM-DD`) or seconds for a capture date; a period for
    /// `within`.
    pub fn value_json(&self) -> Result<Value, String> {
        let kind = self.kind().ok_or_else(|| self.field_error())?;
        let is_date = is_date(kind);
        let field = &self.field;
        let number = |t: &str, end: bool| -> Result<Value, String> {
            let t = t.trim();
            if is_date && let Some(day) = day_start(t) {
                // The last bound of a range of days takes the day whole.
                return Ok(json!(if end { day + 86_399 } else { day }));
            }
            if let Ok(n) = t.parse::<i64>() {
                return Ok(json!(n));
            }
            match t.parse::<f64>() {
                Ok(n) if n.is_finite() => Ok(json!(n)),
                _ if is_date => Err(format!("`{field}`: `{t}` is not a day (YYYY-MM-DD)")),
                _ => Err(format!("`{field}`: `{t}` is not a number")),
            }
        };
        Ok(match (kind, self.op.as_str()) {
            (RuleKind::Date | RuleKind::Imported, "within") => match self.value.as_str() {
                "last" => {
                    let n =
                        self.high.trim().parse::<u64>().map_err(|_| {
                            format!("`{field}`: `{}` is not a count", self.high.trim())
                        })?;
                    json!({ "last": n, "unit": self.unit })
                }
                period => json!(period),
            },
            (RuleKind::Number | RuleKind::Date | RuleKind::Imported, "between") => {
                json!([number(&self.value, false)?, number(&self.high, true)?])
            }
            (RuleKind::Number | RuleKind::Date | RuleKind::Imported, _) => {
                number(&self.value, false)?
            }
            _ => json!(self.value),
        })
    }

    fn field_error(&self) -> String {
        rule_field(&self.field).err().unwrap_or_default()
    }

    /// The condition, checked by the vault: its refusal is the reason.
    pub fn rule(&self) -> Result<Rule, String> {
        Rule::condition(&self.field, &self.op, self.value_json()?)
    }

    /// The boxes showing a condition of the vault's.
    pub fn of(c: &helios_vault::rules::Condition) -> Draft {
        let mut d = Draft {
            field: c.field.name.into(),
            op: c.op.into(),
            value: String::new(),
            high: String::new(),
            unit: Unit::Days.name().into(),
        };
        let date = is_date(c.field.kind);
        let shown = |v: &Value, end: bool| -> String {
            match v.as_f64() {
                Some(n) if date => {
                    let start = if end { n + 1.0 } else { n };
                    if start.rem_euclid(86_400.0) == 0.0 {
                        day_text(if end { n } else { start })
                    } else {
                        v.to_string()
                    }
                }
                Some(_) => v.to_string(),
                None => v.as_str().map(String::from).unwrap_or_default(),
            }
        };
        match c.op {
            "within" => match Relative::parse(&c.value) {
                Ok(Relative::Last(n, unit)) => {
                    d.value = "last".into();
                    d.high = n.to_string();
                    d.unit = unit.name().into();
                }
                Ok(Relative::Today) => d.value = "today".into(),
                Ok(Relative::ThisYear) => d.value = "this_year".into(),
                Err(_) => {}
            },
            "between" => {
                d.value = shown(&c.value[0], false);
                d.high = shown(&c.value[1], true);
            }
            _ => d.value = shown(&c.value, false),
        }
        d
    }

    /// Sets the field: the operator and value start again from the
    /// kind's first operator.
    fn set_field(&mut self, field: &str) {
        let before = self.kind();
        self.field = field.into();
        let kind = self.kind();
        if kind != before || kind.is_none() {
            self.op = kind.map(|k| k.ops()[0]).unwrap_or_default().to_string();
            self.value = match kind {
                Some(RuleKind::Flag) => "pick".into(),
                Some(RuleKind::Set) if field == "label" => "red".into(),
                _ => String::new(),
            };
            self.high.clear();
        }
    }

    /// Sets the operator; a value of another shape (a period, a range)
    /// starts again.
    fn set_op(&mut self, op: &str) {
        let shape = |op: &str| match op {
            "within" => 2,
            "between" => 1,
            _ => 0,
        };
        if shape(op) != shape(&self.op) {
            self.value = if op == "within" {
                "last".into()
            } else {
                String::new()
            };
            self.high = if op == "within" {
                "30".into()
            } else {
                String::new()
            };
        }
        self.op = op.into();
    }
}

impl Default for Draft {
    fn default() -> Self {
        Draft::new()
    }
}

impl Node {
    /// The draft of a tree.
    pub fn of(rule: &Rule) -> Node {
        match rule {
            Rule::Group { mode, rules } => Node::Group {
                mode: *mode,
                rules: rules.iter().map(Node::of).collect(),
            },
            Rule::Condition(c) => Node::Condition(Draft::of(c)),
        }
    }

    /// The tree, every condition valid; else the first fault, with where
    /// it is (as the vault says it).
    pub fn rule(&self) -> Result<Rule, String> {
        let tree = self.rule_at("rules")?;
        // The vault's own checks on the whole (depth, count).
        Rule::parse(&tree.to_json())
    }

    fn rule_at(&self, at: &str) -> Result<Rule, String> {
        match self {
            Node::Group { mode, rules } => Ok(Rule::group(
                *mode,
                rules
                    .iter()
                    .enumerate()
                    .map(|(i, r)| r.rule_at(&format!("{at}[{i}]")))
                    .collect::<Result<_, _>>()?,
            )),
            Node::Condition(d) => d.rule().map_err(|e| format!("{at}: {e}")),
        }
    }

    /// The rule at `path`.
    pub fn get(&self, path: &[usize]) -> Option<&Node> {
        match path.split_first() {
            None => Some(self),
            Some((i, rest)) => match self {
                Node::Group { rules, .. } => rules.get(*i)?.get(rest),
                Node::Condition(_) => None,
            },
        }
    }

    fn get_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        match path.split_first() {
            None => Some(self),
            Some((i, rest)) => match self {
                Node::Group { rules, .. } => rules.get_mut(*i)?.get_mut(rest),
                Node::Condition(_) => None,
            },
        }
    }

    fn depth(&self) -> usize {
        match self {
            Node::Group { rules, .. } => 1 + rules.iter().map(Node::depth).max().unwrap_or(0),
            Node::Condition(_) => 0,
        }
    }
}

/// One change to the draft.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// A new condition at the end of the group at `at`.
    AddCondition {
        at: Path,
    },
    /// A new empty `all` group at the end of the group at `at`.
    AddGroup {
        at: Path,
    },
    /// The rule at `at` removed (not the top group).
    Remove {
        at: Path,
    },
    /// The rule at `at` moved to index `to` of its group.
    Move {
        at: Path,
        to: usize,
    },
    /// The group at `at` combines its rules thus.
    Match {
        at: Path,
        mode: Match,
    },
    Field {
        at: Path,
        field: String,
    },
    Op {
        at: Path,
        op: String,
    },
    /// The value's boxes (`high` and `unit`: kept when not given).
    Value {
        at: Path,
        value: String,
        high: Option<String>,
        unit: Option<String>,
    },
}

impl Edit {
    /// An edit from a command's arguments: `edit` (`add_condition`,
    /// `add_group`, `remove`, `move` with `to`, `match` with `match`,
    /// `field` with `field`, `op` with `op`, `value` with `value`,
    /// `high`, `unit`) at `at`, a path of indexes (`[]`: the top group).
    pub fn parse(args: &Value) -> Result<Edit, String> {
        let at: Path = match args.get("at") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(list)) => list
                .iter()
                .map(|v| v.as_u64().map(|n| n as usize))
                .collect::<Option<_>>()
                .ok_or_else(|| format!("`at`: {} is not a list of indexes", args["at"]))?,
            Some(other) => return Err(format!("`at`: {other} is not a list of indexes")),
        };
        let text = |k: &str| -> Result<String, String> {
            match args.get(k) {
                Some(Value::String(s)) => Ok(s.clone()),
                Some(Value::Number(n)) => Ok(n.to_string()),
                Some(other) => Err(format!("`{k}`: {other} is not a text")),
                None => Err(format!("`{k}` is missing")),
            }
        };
        let kind = text("edit")?;
        Ok(match kind.as_str() {
            "add_condition" => Edit::AddCondition { at },
            "add_group" => Edit::AddGroup { at },
            "remove" => Edit::Remove { at },
            "move" => Edit::Move {
                at,
                to: args
                    .get("to")
                    .and_then(Value::as_u64)
                    .ok_or("`to`: an index")? as usize,
            },
            "match" => Edit::Match {
                at,
                mode: Match::parse(&text("match")?).ok_or("`match` is all, any or none")?,
            },
            "field" => Edit::Field {
                at,
                field: text("field")?,
            },
            "op" => Edit::Op {
                at,
                op: text("op")?,
            },
            "value" => Edit::Value {
                at,
                value: text("value")?,
                high: text("high").ok(),
                unit: text("unit").ok(),
            },
            other => {
                return Err(format!(
                    "`edit`: `{other}` is add_condition, add_group, remove, move, match, field, op, value, undo, redo, apply or cancel"
                ));
            }
        })
    }
}

/// The dialog's state: the album edited and its draft, with the edits'
/// history.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleEditor {
    /// The album's id and name.
    pub album: String,
    pub name: String,
    pub tree: Node,
    undo: Vec<Node>,
    redo: Vec<Node>,
    /// The value box typed in last: further typing there is the same
    /// step.
    typing: Option<Path>,
}

impl RuleEditor {
    pub fn new(album: &str, name: &str, rules: &Rule) -> RuleEditor {
        RuleEditor {
            album: album.into(),
            name: name.into(),
            tree: Node::of(rules),
            undo: Vec::new(),
            redo: Vec::new(),
            typing: None,
        }
    }

    /// Makes an edit, one step of the dialog's history (typing on in
    /// the same value box stays one step); a path that names no rule of
    /// the right sort is refused.
    pub fn edit(&mut self, edit: Edit) -> Result<(), String> {
        let typing = match &edit {
            Edit::Value { at, .. } => Some(at.clone()),
            _ => None,
        };
        let mut tree = self.tree.clone();
        apply(&mut tree, edit)?;
        if tree != self.tree {
            let same = typing.is_some() && typing == self.typing && !self.undo.is_empty();
            let before = std::mem::replace(&mut self.tree, tree);
            if !same {
                self.undo.push(before);
            }
            self.redo.clear();
        }
        self.typing = typing;
        Ok(())
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The last edit taken back; false with none.
    pub fn undo(&mut self) -> bool {
        self.typing = None;
        let Some(before) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.tree, before));
        true
    }

    pub fn redo(&mut self) -> bool {
        self.typing = None;
        let Some(after) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.tree, after));
        true
    }

    /// Each condition's fault, by where it is.
    pub fn faults(&self) -> Vec<(Path, String)> {
        let mut out = Vec::new();
        faults(&self.tree, &mut Vec::new(), &mut out);
        out
    }

    /// What an agent reads of the dialog: the draft as JSON (a condition
    /// as typed), each fault, and the tree when it is valid.
    pub fn state(&self) -> Value {
        json!({
            "id": self.album,
            "draft": draft_json(&self.tree),
            "faults": self.faults().into_iter()
                .map(|(at, why)| json!({ "at": at, "reason": why }))
                .collect::<Vec<_>>(),
            "rules": self.tree.rule().ok().map(|r| r.to_json()),
            "undo": self.can_undo(),
            "redo": self.can_redo(),
        })
    }
}

fn faults(node: &Node, at: &mut Path, out: &mut Vec<(Path, String)>) {
    match node {
        Node::Group { rules, .. } => {
            for (i, r) in rules.iter().enumerate() {
                at.push(i);
                faults(r, at, out);
                at.pop();
            }
        }
        Node::Condition(d) => {
            if let Err(e) = d.rule() {
                out.push((at.clone(), e));
            }
        }
    }
}

fn draft_json(node: &Node) -> Value {
    match node {
        Node::Group { mode, rules } => json!({
            "match": mode.name(),
            "rules": rules.iter().map(draft_json).collect::<Vec<_>>(),
        }),
        Node::Condition(d) => json!({
            "field": d.field, "op": d.op, "value": d.value, "high": d.high, "unit": d.unit,
        }),
    }
}

fn where_(at: &[usize]) -> String {
    format!("`at`: {at:?} names no rule")
}

fn group<'a>(tree: &'a mut Node, at: &[usize]) -> Result<&'a mut Vec<Node>, String> {
    match tree.get_mut(at) {
        Some(Node::Group { rules, .. }) => Ok(rules),
        Some(Node::Condition(_)) => Err(format!("`at`: {at:?} is a condition, not a group")),
        None => Err(where_(at)),
    }
}

fn draft<'a>(tree: &'a mut Node, at: &[usize]) -> Result<&'a mut Draft, String> {
    match tree.get_mut(at) {
        Some(Node::Condition(d)) => Ok(d),
        Some(Node::Group { .. }) => Err(format!("`at`: {at:?} is a group, not a condition")),
        None => Err(where_(at)),
    }
}

fn apply(tree: &mut Node, edit: Edit) -> Result<(), String> {
    match edit {
        Edit::AddCondition { at } => group(tree, &at)?.push(Node::Condition(Draft::new())),
        Edit::AddGroup { at } => {
            if at.len() + 2 > helios_vault::rules::MAX_DEPTH {
                return Err(format!(
                    "groups nest at most {} deep",
                    helios_vault::rules::MAX_DEPTH
                ));
            }
            group(tree, &at)?.push(Node::Group {
                mode: Match::All,
                rules: Vec::new(),
            });
        }
        Edit::Remove { at } => {
            let (i, parent) = at.split_last().ok_or("the top group stays")?;
            let rules = group(tree, parent)?;
            if *i >= rules.len() {
                return Err(where_(&at));
            }
            rules.remove(*i);
        }
        Edit::Move { at, to } => {
            let (i, parent) = at.split_last().ok_or("the top group stays")?;
            let rules = group(tree, parent)?;
            if *i >= rules.len() || to >= rules.len() {
                return Err(where_(&at));
            }
            let rule = rules.remove(*i);
            rules.insert(to, rule);
        }
        Edit::Match { at, mode: m } => match tree.get_mut(&at) {
            Some(Node::Group { mode, .. }) => *mode = m,
            Some(Node::Condition(_)) => {
                return Err(format!("`at`: {at:?} is a condition, not a group"));
            }
            None => return Err(where_(&at)),
        },
        Edit::Field { at, field } => {
            rule_field(&field)?;
            draft(tree, &at)?.set_field(&field);
        }
        Edit::Op { at, op } => draft(tree, &at)?.set_op(&op),
        Edit::Value {
            at,
            value,
            high,
            unit,
        } => {
            let d = draft(tree, &at)?;
            d.value = value;
            if let Some(h) = high {
                d.high = h;
            }
            if let Some(u) = unit {
                d.unit = u;
            }
        }
    }
    debug_assert!(tree.depth() <= helios_vault::rules::MAX_DEPTH);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &[usize]) -> Path {
        path.to_vec()
    }

    #[test]
    fn a_nested_tree_built_by_edits_is_the_rules_expected() {
        let empty = Rule::group(Match::All, Vec::new());
        let mut e = RuleEditor::new("best", "Best", &empty);
        let mut run = |edit: Edit| e.edit(edit).unwrap();
        run(Edit::AddCondition { at: at(&[]) });
        run(Edit::AddGroup { at: at(&[]) });
        run(Edit::Match {
            at: at(&[1]),
            mode: Match::Any,
        });
        run(Edit::AddCondition { at: at(&[1]) });
        run(Edit::Field {
            at: at(&[1, 0]),
            field: "camera".into(),
        });
        run(Edit::Op {
            at: at(&[1, 0]),
            op: "contains".into(),
        });
        run(Edit::Value {
            at: at(&[1, 0]),
            value: "Z".into(),
            high: None,
            unit: None,
        });
        run(Edit::AddCondition { at: at(&[1]) });
        run(Edit::Field {
            at: at(&[1, 1]),
            field: "captured".into(),
        });
        run(Edit::Op {
            at: at(&[1, 1]),
            op: "within".into(),
        });
        run(Edit::Value {
            at: at(&[1, 1]),
            value: "last".into(),
            high: Some("2".into()),
            unit: Some("weeks".into()),
        });
        run(Edit::AddGroup { at: at(&[1]) });
        run(Edit::Match {
            at: at(&[1, 2]),
            mode: Match::None,
        });
        run(Edit::AddCondition { at: at(&[1, 2]) });
        run(Edit::Field {
            at: at(&[1, 2, 0]),
            field: "flag".into(),
        });
        run(Edit::Value {
            at: at(&[1, 2, 0]),
            value: "reject".into(),
            high: None,
            unit: None,
        });
        run(Edit::AddCondition { at: at(&[]) });
        run(Edit::Field {
            at: at(&[2]),
            field: "captured".into(),
        });
        run(Edit::Op {
            at: at(&[2]),
            op: "between".into(),
        });
        run(Edit::Value {
            at: at(&[2]),
            value: "2024-06-01".into(),
            high: Some("2024-06-30".into()),
            unit: None,
        });
        // Reordered: the range first.
        run(Edit::Move {
            at: at(&[2]),
            to: 0,
        });
        let june = day_start("2024-06-01").unwrap();
        let end = day_start("2024-06-30").unwrap() + 86_399;
        let expected = json!({"match": "all", "rules": [
            {"field": "captured", "op": "between", "value": [june, end]},
            {"field": "rating", "op": ">=", "value": 1},
            {"match": "any", "rules": [
                {"field": "camera", "op": "contains", "value": "Z"},
                {"field": "captured", "op": "within", "value": {"last": 2, "unit": "weeks"}},
                {"match": "none", "rules": [
                    {"field": "flag", "op": "is", "value": "reject"},
                ]},
            ]},
        ]});
        assert_eq!(e.tree.rule().unwrap().to_json(), expected);
        assert!(e.faults().is_empty());
        // Read back: the same draft, days and periods shown as typed.
        let back = Node::of(&Rule::parse(&expected).unwrap());
        assert_eq!(back, e.tree);
        assert_eq!(e.state()["rules"], expected);
    }

    #[test]
    fn an_invalid_condition_says_why_and_the_tree_is_not_valid() {
        let mut e = RuleEditor::new("best", "Best", &Rule::group(Match::All, Vec::new()));
        e.edit(Edit::AddCondition { at: at(&[]) }).unwrap();
        e.edit(Edit::Value {
            at: at(&[0]),
            value: "9".into(),
            high: None,
            unit: None,
        })
        .unwrap();
        let faults = e.faults();
        assert_eq!(faults.len(), 1);
        assert_eq!(faults[0].0, at(&[0]));
        assert!(faults[0].1.contains("not in [0, 5]"), "{}", faults[0].1);
        let why = e.tree.rule().unwrap_err();
        assert!(why.starts_with("rules[0]:"), "{why}");
        assert!(e.state()["rules"].is_null());
        e.edit(Edit::Value {
            at: at(&[0]),
            value: "many".into(),
            high: None,
            unit: None,
        })
        .unwrap();
        assert!(e.faults()[0].1.contains("not a number"));
        // Paths that name nothing are refused, and change nothing.
        assert!(e.edit(Edit::Remove { at: at(&[]) }).is_err());
        assert!(e.edit(Edit::AddCondition { at: at(&[0]) }).is_err());
        assert!(e.edit(Edit::Remove { at: at(&[3]) }).is_err());
        assert!(
            e.edit(Edit::Field {
                at: at(&[0]),
                field: "colour".into()
            })
            .is_err()
        );
    }

    #[test]
    fn each_edit_is_undone_and_redone() {
        let start = Rule::parse(&json!({"match": "all", "rules": [
            {"field": "rating", "op": ">=", "value": 3},
        ]}))
        .unwrap();
        let mut e = RuleEditor::new("best", "Best", &start);
        let first = e.tree.clone();
        e.edit(Edit::Match {
            at: at(&[]),
            mode: Match::Any,
        })
        .unwrap();
        let second = e.tree.clone();
        e.edit(Edit::Remove { at: at(&[0]) }).unwrap();
        assert!(e.undo());
        assert_eq!(e.tree, second);
        assert!(e.undo());
        assert_eq!(e.tree, first);
        assert!(!e.undo());
        assert!(e.redo());
        assert_eq!(e.tree, second);
        e.edit(Edit::AddGroup { at: at(&[]) }).unwrap();
        assert!(!e.can_redo(), "a new edit drops what was undone");
        // An edit changing nothing is no step.
        let steps = e.undo.len();
        e.edit(Edit::Match {
            at: at(&[]),
            mode: Match::Any,
        })
        .unwrap();
        assert_eq!(e.undo.len(), steps);
        // Typing on in one box is one step.
        e.edit(Edit::AddCondition { at: at(&[]) }).unwrap();
        let before = e.tree.clone();
        for typed in ["4", "45", "4"] {
            e.edit(Edit::Value {
                at: at(&[2]),
                value: typed.into(),
                high: None,
                unit: None,
            })
            .unwrap();
        }
        assert!(e.undo());
        assert_eq!(e.tree, before);
    }

    #[test]
    fn edits_parse_from_a_command_s_arguments() {
        assert_eq!(
            Edit::parse(&json!({"edit": "value", "at": [1, 0], "value": 3})).unwrap(),
            Edit::Value {
                at: at(&[1, 0]),
                value: "3".into(),
                high: None,
                unit: None
            }
        );
        assert_eq!(
            Edit::parse(&json!({"edit": "match", "match": "none"})).unwrap(),
            Edit::Match {
                at: at(&[]),
                mode: Match::None
            }
        );
        assert!(Edit::parse(&json!({"edit": "shuffle"})).is_err());
        assert!(Edit::parse(&json!({"edit": "move", "at": [0]})).is_err());
    }
}
