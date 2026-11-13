//! The smart album form (`docs/design/library.md`, L6): the common
//! conditions as form fields, read into a rule tree
//! ([`helios_vault::rules`]) and back, for a new smart album. Its rules
//! are then edited, whatever their tree, in the rule editor
//! ([`crate::rule_editor`]).

use helios_vault::capture::day_start;
use helios_vault::rules::{Condition, Match, Rule};
use serde_json::{Value, json};

/// The form's keys, besides `name`.
pub const KEYS: [&str; 10] = [
    "match",
    "rating",
    "label",
    "flag",
    "tag",
    "camera",
    "file_type",
    "folder",
    "taken_from",
    "taken_to",
];

/// What a choice box holds when it asks nothing.
pub const ANY: &str = "any";

/// The match modes the form offers.
pub const MATCHES: [&str; 2] = ["all", "any"];

/// The form's values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SmartForm {
    /// One condition is enough (else all of them).
    pub any: bool,
    /// At least these stars (0: no condition).
    pub rating: i64,
    /// A colour label, or [`ANY`].
    pub label: String,
    /// A flag (`pick`, `reject`, `none`), or [`ANY`].
    pub flag: String,
    pub tag: String,
    /// In the camera's model, whatever the case.
    pub camera: String,
    pub file_type: String,
    /// A folder (absolute), its photos and those below.
    pub folder: String,
    /// Taken on or after this day (`YYYY-MM-DD`).
    pub taken_from: String,
    /// Taken on or before this day.
    pub taken_to: String,
}

impl SmartForm {
    /// The form asking nothing yet.
    pub fn of_default() -> SmartForm {
        SmartForm {
            label: ANY.into(),
            flag: ANY.into(),
            ..SmartForm::default()
        }
    }

    /// The labels' box: [`ANY`], then each colour.
    pub fn labels() -> Vec<&'static str> {
        let mut all = vec![ANY];
        if let Ok(spec) = helios_vault::fields::spec(helios_vault::fields::LABELS)
            && let helios_vault::fields::FieldKind::Labels(colours) = spec.kind
        {
            all.extend_from_slice(colours);
        }
        all
    }

    /// The flags' box: [`ANY`], then each flag.
    pub fn flags() -> Vec<&'static str> {
        let mut all = vec![ANY];
        all.extend_from_slice(&helios_vault::fields::FLAGS);
        all
    }

    /// Whether the arguments of a command hold one of the form's keys.
    pub fn in_args(args: &Value) -> bool {
        KEYS.iter().any(|k| args.get(k).is_some())
    }

    /// The form's values from a command's arguments (a key missing asks
    /// nothing).
    pub fn from_args(args: &Value) -> Result<SmartForm, String> {
        let text = |k: &str| -> Result<String, String> {
            match args.get(k) {
                None | Some(Value::Null) => Ok(String::new()),
                Some(Value::String(s)) => Ok(s.trim().to_string()),
                Some(other) => Err(format!("`{k}`: {other} is not a text")),
            }
        };
        let choice = |k: &str| text(k).map(|t| if t.is_empty() { ANY.into() } else { t });
        let any = match text("match")?.as_str() {
            "" | "all" => false,
            "any" => true,
            other => return Err(format!("`match`: `{other}` is all or any")),
        };
        let rating = match args.get("rating") {
            None | Some(Value::Null) => 0,
            Some(v) => v
                .as_i64()
                .ok_or_else(|| format!("`rating`: {v} is not a whole number"))?,
        };
        Ok(SmartForm {
            any,
            rating,
            label: choice("label")?,
            flag: choice("flag")?,
            tag: text("tag")?,
            camera: text("camera")?,
            file_type: text("file_type")?.to_lowercase(),
            folder: text("folder")?,
            taken_from: text("taken_from")?,
            taken_to: text("taken_to")?,
        })
    }

    /// The rule tree the form asks: one group of its conditions; each
    /// checked, a fault refused with its reason.
    pub fn rules(&self) -> Result<Rule, String> {
        let mut rules = Vec::new();
        let mut add = |field: &str, op: &str, value: Value| -> Result<(), String> {
            rules.push(Rule::condition(field, op, value)?);
            Ok(())
        };
        if self.rating != 0 {
            add("rating", ">=", json!(self.rating))?;
        }
        if self.label != ANY {
            add("label", "has", json!(self.label))?;
        }
        if self.flag != ANY {
            add("flag", "is", json!(self.flag))?;
        }
        if !self.tag.is_empty() {
            add("tag", "has", json!(self.tag))?;
        }
        if !self.camera.is_empty() {
            add("camera", "contains", json!(self.camera))?;
        }
        if !self.file_type.is_empty() {
            add("file_type", "is", json!(self.file_type))?;
        }
        if !self.folder.is_empty() {
            add("folder", "in", json!(self.folder))?;
        }
        let day = |k: &str, t: &str| {
            day_start(t).ok_or_else(|| format!("`{k}`: `{t}` is not a day (YYYY-MM-DD)"))
        };
        if !self.taken_from.is_empty() {
            add(
                "captured",
                ">=",
                json!(day("taken_from", &self.taken_from)?),
            )?;
        }
        if !self.taken_to.is_empty() {
            let end = day("taken_to", &self.taken_to)? + 86_400;
            add("captured", "<", json!(end))?;
        }
        let mode = if self.any { Match::Any } else { Match::All };
        Ok(Rule::group(mode, rules))
    }

    /// The form showing a tree, when it can: one `all` or `any` group of
    /// conditions the form has a box for, each once.
    pub fn of(rule: &Rule) -> Option<SmartForm> {
        let Rule::Group { mode, rules } = rule else {
            return None;
        };
        let mut form = SmartForm {
            any: match mode {
                Match::All => false,
                Match::Any => true,
                Match::None => return None,
            },
            label: ANY.into(),
            flag: ANY.into(),
            ..SmartForm::default()
        };
        let mut seen = Vec::new();
        for r in rules {
            let Rule::Condition(c) = r else {
                return None;
            };
            let Condition { field, op, .. } = c;
            let key = (field.name, *op);
            if seen.contains(&key) {
                return None;
            }
            seen.push(key);
            let text = c.text().to_string();
            match key {
                ("rating", ">=") => form.rating = c.number() as i64,
                ("label", "has") => form.label = text,
                ("flag", "is") => form.flag = text,
                ("tag", "has") => form.tag = text,
                ("camera", "contains") => form.camera = text,
                ("file_type", "is") => form.file_type = text,
                ("folder", "in") => form.folder = text,
                ("captured", ">=") => {
                    form.taken_from = helios_vault::capture::day_text(c.number());
                }
                ("captured", "<") => {
                    form.taken_to = helios_vault::capture::day_text(c.number() - 86_400.0);
                }
                _ => return None,
            }
        }
        Some(form)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_reads_into_rules_and_back() {
        let args = json!({
            "name": "Best", "match": "any", "rating": 4, "label": "red", "flag": "any",
            "camera": "Z", "taken_from": "2024-06-01", "taken_to": "2024-06-30",
        });
        let form = SmartForm::from_args(&args).unwrap();
        let rules = form.rules().unwrap();
        assert_eq!(rules.to_json()["match"], "any");
        assert_eq!(rules.to_json()["rules"].as_array().unwrap().len(), 5);
        assert_eq!(SmartForm::of(&rules), Some(form));
        let nested =
            Rule::parse(&json!({"match": "all", "rules": [{"match": "any", "rules": []}]}))
                .unwrap();
        assert_eq!(SmartForm::of(&nested), None, "a nested group has no form");
        let e = SmartForm::from_args(&json!({"label": "pink"}))
            .unwrap()
            .rules()
            .unwrap_err();
        assert!(e.contains("not one of"), "{e}");
        let e = SmartForm::from_args(&json!({"taken_from": "June"}))
            .unwrap()
            .rules()
            .unwrap_err();
        assert!(e.contains("not a day"), "{e}");
    }
}
