//! Helios settings.
//!
//! **Declared once.** Every setting is a [`Setting`]: its key
//! (`section.key`), the options page and group that show it, its label and
//! description, its kind (bounds, choices, default) and its concern. Options
//! pages, get and set by key, and the check of a hand-edited file all come
//! from the declarations.
//!
//! **Layered.** A value comes from the highest layer that sets it, among
//! the layers its concern allows (lowest first):
//!
//! | concern | layers |
//! |---|---|
//! | machine (GPU, threads, journal size) | default, global, run |
//! | project (export targets, decision modes) | default, global, project, run |
//! | user taste (panels, layout) | default, global, project, user, run |
//!
//! `global` is this user on this machine, `project` belongs to a project and
//! is shared by everyone working on it, `user` follows the user on every
//! machine, `run` is one invocation (command line, environment). Setting a
//! value in a layer its concern does not allow is refused.
//!
//! **Origin.** Every effective value is reported with the layer it comes
//! from. A value set back to its default leaves its layer, so defaults can
//! evolve.
//!
//! **Settings are inputs.** This crate reads only what it is handed (texts
//! or paths given by the program's edge); components receive resolved
//! values, never read a file behind the caller's back.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use helios_core::Describe;
use serde_json::json;
use toml::{Table, Value as Toml};

/// Who a setting belongs to; decides which layers may set it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Concern {
    Machine,
    Project,
    User,
}

impl Concern {
    /// The layers that may set it, lowest precedence first (after defaults).
    pub fn layers(self) -> &'static [Layer] {
        match self {
            Concern::Machine => &[Layer::Global, Layer::Run],
            Concern::Project => &[Layer::Global, Layer::Project, Layer::Run],
            Concern::User => &[Layer::Global, Layer::Project, Layer::User, Layer::Run],
        }
    }

    fn name(self) -> &'static str {
        match self {
            Concern::Machine => "machine",
            Concern::Project => "project",
            Concern::User => "user",
        }
    }
}

/// Where a value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    Default,
    Global,
    Project,
    User,
    Run,
}

impl Layer {
    pub fn name(self) -> &'static str {
        match self {
            Layer::Default => "default",
            Layer::Global => "global",
            Layer::Project => "project",
            Layer::User => "user",
            Layer::Run => "run",
        }
    }
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What a setting takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Number {
        min: f64,
        max: f64,
        default: f64,
        unit: &'static str,
        integer: bool,
    },
    /// On or off, each state with its word (`shown` / `hidden`).
    Toggle {
        default: bool,
        on: &'static str,
        off: &'static str,
    },
    Choice {
        options: &'static [&'static str],
        default: &'static str,
    },
}

impl Kind {
    pub fn default_value(&self) -> Value {
        match *self {
            Kind::Number { default, .. } => Value::Number(default),
            Kind::Toggle { default, .. } => Value::Toggle(default),
            Kind::Choice { default, .. } => Value::Choice(default.to_string()),
        }
    }
}

/// A setting's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Number(f64),
    Toggle(bool),
    Choice(String),
}

impl Value {
    fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Number(v) => json!(v),
            Value::Toggle(v) => json!(v),
            Value::Choice(v) => json!(v),
        }
    }
}

/// One setting, declared once.
#[derive(Clone, Copy, Debug)]
pub struct Setting {
    /// Its name, `page.group.name`: the options page and the group on it
    /// that show it come from the name itself, so pages need no layout of
    /// their own. In a file, `[page.group]` then `name = value`.
    pub key: &'static str,
    pub concern: Concern,
    pub label: &'static str,
    pub about: &'static str,
    pub kind: Kind,
}

impl Setting {
    /// The options page that shows it: the first part of its name.
    pub fn page(&self) -> &'static str {
        self.key.split('.').next().expect("keys are not empty")
    }

    /// The group on its page: the middle of its name (`general` when the
    /// name has only two parts).
    pub fn group(&self) -> &'static str {
        let parts: Vec<&'static str> = self.key.split('.').collect();
        match parts.len() {
            0..=2 => "general",
            n => {
                let start = parts[0].len() + 1;
                let end = self.key.len() - parts[n - 1].len() - 1;
                &self.key[start..end]
            }
        }
    }

    /// The input a surface draws for it, in the same normalized shape as a
    /// module parameter (`type`, bounds, default, choices), so one widget
    /// library serves both.
    pub fn input(&self) -> serde_json::Value {
        match self.kind {
            Kind::Number {
                min,
                max,
                default,
                unit,
                integer,
            } => {
                json!({ "type": if integer { "int" } else { "float" }, "min": min, "max": max, "default": default, "unit": unit })
            }
            Kind::Toggle { default, on, off } => {
                json!({ "type": "bool", "default": default, "on": on, "off": off })
            }
            Kind::Choice { options, default } => {
                json!({ "type": "choice", "options": options, "default": default })
            }
        }
    }
}

/// The settings of the framework itself. Other crates declare theirs the
/// same way and add them to a [`Schema`].
pub const SETTINGS: &[Setting] = &[
    Setting {
        key: "system.cache.budget",
        concern: Concern::Machine,
        label: "Render cache",
        about: "Memory the render cache may use before evicting what was used least recently; 0 means a share of the machine's memory (the setting below).",
        kind: Kind::Number {
            min: 0.0,
            max: 1_048_576.0,
            default: 0.0,
            unit: "MiB",
            integer: true,
        },
    },
    Setting {
        key: "system.cache.share",
        concern: Concern::Machine,
        label: "Render cache share",
        about: "The share of the machine's memory the render cache may use when no size is set above; 0 means no limit.",
        kind: Kind::Number {
            min: 0.0,
            max: 90.0,
            default: helios_core::engine::DEFAULT_CACHE_SHARE,
            unit: "%",
            integer: false,
        },
    },
    Setting {
        key: "system.scheduler.workers",
        concern: Concern::Machine,
        label: "Worker threads",
        about: "How many jobs run at once on this machine; 0 means one per CPU.",
        kind: Kind::Number {
            min: 0.0,
            max: 1024.0,
            default: 0.0,
            unit: "threads",
            integer: true,
        },
    },
    Setting {
        key: "system.ai.calls",
        concern: Concern::Machine,
        label: "Model calls at once",
        about: "How many calls to AI models (decision nodes) run at the same time.",
        kind: Kind::Number {
            min: 1.0,
            max: 64.0,
            default: 2.0,
            unit: "calls",
            integer: true,
        },
    },
    Setting {
        key: "system.events.journal",
        concern: Concern::Machine,
        label: "Event journal",
        about: "How many recent events the bus keeps for inspection and waits.",
        kind: Kind::Number {
            min: 16.0,
            max: 1_000_000.0,
            default: 1024.0,
            unit: "events",
            integer: true,
        },
    },
    Setting {
        key: "system.workflow.keep_days",
        concern: Concern::Project,
        label: "Finished batches kept",
        about: "How long a finished batch (a run of the workflow level) stays in the vault, failures included, before it is removed whole; 0 removes it when the node next starts.",
        kind: Kind::Number {
            min: 0.0,
            max: 3650.0,
            default: 30.0,
            unit: "days",
            integer: true,
        },
    },
    Setting {
        key: "project.decisions.mode",
        concern: Concern::Project,
        label: "Decision nodes",
        about: "How decision nodes are resolved by default in this project.",
        kind: Kind::Choice {
            options: &["full_ai", "ai_assisted", "human"],
            default: "ai_assisted",
        },
    },
    Setting {
        key: "interface.panels.queue_when_empty",
        concern: Concern::User,
        label: "Queue panel without a queue",
        about: "What the queue panel does when the selected node has no queue.",
        kind: Kind::Toggle {
            default: true,
            on: "shown, empty",
            off: "hidden",
        },
    },
    Setting {
        key: "interface.graph.zoom",
        concern: Concern::User,
        label: "Graph view opens on",
        about: "Whether the graph view opens on the overview or the full detail.",
        kind: Kind::Choice {
            options: &["overview", "detail"],
            default: "detail",
        },
    },
];

/// The declared settings a program knows.
#[derive(Clone, Debug, Default)]
pub struct Schema {
    settings: Vec<&'static Setting>,
}

impl Schema {
    pub fn new(sets: &[&'static [Setting]]) -> Self {
        let settings: Vec<_> = sets.iter().flat_map(|s| s.iter()).collect();
        for s in &settings {
            assert!(
                s.key.split('.').count() >= 2 && s.key.split('.').all(|p| !p.is_empty()),
                "setting `{}` is not named `page.group.name`",
                s.key
            );
        }
        let mut keys: Vec<_> = settings.iter().map(|s| s.key).collect();
        keys.sort_unstable();
        assert!(
            keys.windows(2).all(|w| w[0] != w[1]),
            "a setting is declared twice"
        );
        Self { settings }
    }

    /// The framework's own settings.
    pub fn core() -> Self {
        Self::new(&[SETTINGS])
    }

    pub fn get(&self, key: &str) -> Option<&'static Setting> {
        self.settings.iter().copied().find(|s| s.key == key)
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static Setting> + '_ {
        self.settings.iter().copied()
    }
}

/// Something wrong with a value or a file, with where it was found.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub layer: Layer,
    pub key: String,
    pub reason: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} layer, `{}`: {}", self.layer, self.key, self.reason)
    }
}

/// Settings resolved through their layers.
#[derive(Clone, Debug)]
pub struct Config {
    schema: Schema,
    layers: BTreeMap<Layer, BTreeMap<String, Value>>,
}

impl Config {
    pub fn new(schema: Schema) -> Self {
        Self {
            schema,
            layers: BTreeMap::new(),
        }
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// The effective value of a declared setting, with the layer it comes from.
    pub fn get(&self, key: &str) -> Option<(Value, Layer)> {
        let setting = self.schema.get(key)?;
        for layer in setting.concern.layers().iter().rev() {
            if let Some(v) = self.layers.get(layer).and_then(|values| values.get(key)) {
                return Some((v.clone(), *layer));
            }
        }
        Some((setting.kind.default_value(), Layer::Default))
    }

    /// The effective number of a declared number setting.
    /// The render cache's budget in bytes: `system.cache.budget` when set
    /// (MiB), else `system.cache.share` of `memory` (bytes, 2 GiB when not
    /// known); `None` for no limit.
    pub fn cache_budget(&self, memory: Option<u64>) -> Option<usize> {
        match self.number("system.cache.budget") as usize {
            0 => helios_core::engine::memory_share(self.number("system.cache.share"), memory),
            mib => Some(mib * 1024 * 1024),
        }
    }

    pub fn number(&self, key: &str) -> f64 {
        match self.get(key) {
            Some((Value::Number(v), _)) => v,
            other => panic!("`{key}` is not a declared number setting ({other:?})"),
        }
    }

    pub fn toggle(&self, key: &str) -> bool {
        match self.get(key) {
            Some((Value::Toggle(v), _)) => v,
            other => panic!("`{key}` is not a declared toggle setting ({other:?})"),
        }
    }

    pub fn choice(&self, key: &str) -> String {
        match self.get(key) {
            Some((Value::Choice(v), _)) => v,
            other => panic!("`{key}` is not a declared choice setting ({other:?})"),
        }
    }

    /// What the layers below `layer` give for a setting.
    fn below(&self, setting: &Setting, layer: Layer) -> Value {
        for l in setting
            .concern
            .layers()
            .iter()
            .rev()
            .filter(|l| **l < layer)
        {
            if let Some(v) = self
                .layers
                .get(l)
                .and_then(|values| values.get(setting.key))
            {
                return v.clone();
            }
        }
        setting.kind.default_value()
    }

    /// Sets a value in a layer, after checking it. A value equal to what the
    /// layers below give leaves the layer, so it keeps following them (and
    /// defaults can evolve).
    pub fn set(&mut self, layer: Layer, key: &str, value: Value) -> Result<(), Problem> {
        let problem = |reason: String| Problem {
            layer,
            key: key.to_string(),
            reason,
        };
        let setting = self
            .schema
            .get(key)
            .ok_or_else(|| problem("unknown setting".into()))?;
        if !setting.concern.layers().contains(&layer) {
            let allowed: Vec<_> = setting.concern.layers().iter().map(|l| l.name()).collect();
            return Err(problem(format!(
                "a {} setting is set in: {}",
                setting.concern.name(),
                allowed.join(", ")
            )));
        }
        let value = check(setting, value).map_err(problem)?;
        let inherited = self.below(setting, layer);
        let values = self.layers.entry(layer).or_default();
        if value == inherited {
            values.remove(key);
        } else {
            values.insert(key.to_string(), value);
        }
        Ok(())
    }

    /// Sets a value given as text (command line, environment).
    pub fn set_str(&mut self, layer: Layer, key: &str, raw: &str) -> Result<(), Problem> {
        let problem = |reason: String| Problem {
            layer,
            key: key.to_string(),
            reason,
        };
        let setting = self
            .schema
            .get(key)
            .ok_or_else(|| problem("unknown setting".into()))?;
        let value = match setting.kind {
            Kind::Number { .. } => Value::Number(
                raw.trim()
                    .parse()
                    .map_err(|_| problem(format!("`{raw}` is not a number")))?,
            ),
            Kind::Toggle { on, off, .. } => match raw.trim() {
                "true" => Value::Toggle(true),
                "false" => Value::Toggle(false),
                w if w == on => Value::Toggle(true),
                w if w == off => Value::Toggle(false),
                _ => {
                    return Err(problem(format!(
                        "`{raw}` is not true, false, `{on}` or `{off}`"
                    )));
                }
            },
            Kind::Choice { .. } => Value::Choice(raw.trim().to_string()),
        };
        self.set(layer, key, value)
    }

    /// Removes a value from a layer.
    pub fn unset(&mut self, layer: Layer, key: &str) {
        if let Some(values) = self.layers.get_mut(&layer) {
            values.remove(key);
        }
    }

    /// Loads a layer from TOML text. All or nothing: on any problem the
    /// layer is left as it was and every problem is returned. Load the lower
    /// layers first: a value equal to what it inherits leaves its layer.
    pub fn load_str(&mut self, layer: Layer, text: &str) -> Result<(), Vec<Problem>> {
        let table: Table = text.parse().map_err(|e: toml::de::Error| {
            vec![Problem {
                layer,
                key: String::new(),
                reason: format!("not valid TOML: {}", e.message()),
            }]
        })?;
        let mut flat = Vec::new();
        let mut problems = Vec::new();
        flatten("", &table, &mut flat, layer, &mut problems);
        let mut next = self.clone();
        next.layers.remove(&layer);
        for (key, value) in flat {
            if let Err(p) = next.set(layer, &key, value) {
                problems.push(p);
            }
        }
        if problems.is_empty() {
            *self = next;
            Ok(())
        } else {
            Err(problems)
        }
    }

    /// Loads a layer from a file; a missing file is an empty layer.
    pub fn load_file(&mut self, layer: Layer, path: &Path) -> Result<(), Vec<Problem>> {
        match std::fs::read_to_string(path) {
            Ok(text) => self.load_str(layer, &text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.layers.remove(&layer);
                Ok(())
            }
            Err(e) => Err(vec![Problem {
                layer,
                key: String::new(),
                reason: format!("cannot read {}: {e}", path.display()),
            }]),
        }
    }

    /// A layer as TOML text, only the values it sets.
    pub fn layer_toml(&self, layer: Layer) -> String {
        let mut root = Table::new();
        for (key, value) in self.layers.get(&layer).into_iter().flatten() {
            let integer = matches!(
                self.schema.get(key).map(|s| s.kind),
                Some(Kind::Number { integer: true, .. })
            );
            let toml = match value {
                Value::Number(v) if integer => Toml::Integer(*v as i64),
                Value::Number(v) => Toml::Float(*v),
                Value::Toggle(v) => Toml::Boolean(*v),
                Value::Choice(v) => Toml::String(v.clone()),
            };
            let mut table = &mut root;
            let mut parts: Vec<&str> = key.split('.').collect();
            let leaf = parts.pop().expect("keys are not empty");
            for part in parts {
                table = table
                    .entry(part)
                    .or_insert_with(|| Toml::Table(Table::new()))
                    .as_table_mut()
                    .expect("sections are tables");
            }
            table.insert(leaf.to_string(), toml);
        }
        toml::to_string(&root).expect("tables always serialize")
    }
}

fn check(setting: &Setting, value: Value) -> Result<Value, String> {
    match (setting.kind, value) {
        (
            Kind::Number {
                min,
                max,
                integer,
                unit,
                ..
            },
            Value::Number(v),
        ) => {
            if !v.is_finite() || v < min || v > max {
                Err(format!("{v} outside [{min}, {max}] {unit}"))
            } else if integer && v.fract() != 0.0 {
                Err(format!("{v} is not a whole number"))
            } else {
                Ok(Value::Number(v))
            }
        }
        (Kind::Toggle { .. }, Value::Toggle(v)) => Ok(Value::Toggle(v)),
        (Kind::Choice { options, .. }, Value::Choice(v)) => {
            if options.contains(&v.as_str()) {
                Ok(Value::Choice(v))
            } else {
                Err(format!("`{v}` is not one of {}", options.join(", ")))
            }
        }
        (kind, value) => Err(format!("expected {}, got {value:?}", kind_name(&kind))),
    }
}

fn kind_name(kind: &Kind) -> &'static str {
    match kind {
        Kind::Number { .. } => "a number",
        Kind::Toggle { .. } => "true or false",
        Kind::Choice { .. } => "one of the choices",
    }
}

fn flatten(
    prefix: &str,
    table: &Table,
    out: &mut Vec<(String, Value)>,
    layer: Layer,
    problems: &mut Vec<Problem>,
) {
    for (k, v) in table {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            Toml::Table(t) => flatten(&key, t, out, layer, problems),
            Toml::Integer(i) => out.push((key, Value::Number(*i as f64))),
            Toml::Float(f) => out.push((key, Value::Number(*f))),
            Toml::Boolean(b) => out.push((key, Value::Toggle(*b))),
            Toml::String(s) => out.push((key, Value::Choice(s.clone()))),
            other => problems.push(Problem {
                layer,
                key,
                reason: format!("unsupported value {other}"),
            }),
        }
    }
}

impl Describe for Config {
    fn describe(&self) -> serde_json::Value {
        let settings: Vec<_> = self
            .schema
            .iter()
            .map(|s| {
                let (value, origin) = self.get(s.key).expect("declared");
                json!({
                    "key": s.key, "value": value.to_json(), "origin": origin.name(),
                    "concern": s.concern.name(), "page": s.page(), "group": s.group(), "label": s.label, "input": s.input(),
                })
            })
            .collect();
        let layers: serde_json::Map<_, _> = self
            .layers
            .iter()
            .map(|(l, v)| (l.name().to_string(), json!(v.len())))
            .collect();
        json!({ "settings": settings, "layers": layers })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config::new(Schema::core())
    }

    #[test]
    fn defaults_come_first_and_say_so() {
        let c = config();
        assert_eq!(
            c.get("system.events.journal"),
            Some((Value::Number(1024.0), Layer::Default))
        );
        assert_eq!(c.get("nope"), None);
    }

    #[test]
    fn the_cache_budget_is_a_share_of_memory_unless_a_size_is_set() {
        let mut c = config();
        let memory = Some(32 << 30);
        assert_eq!(c.cache_budget(memory), Some(8 << 30), "25 % by default");
        assert_eq!(c.cache_budget(None), Some(2 << 30), "memory not known");
        c.set_str(Layer::Global, "system.cache.share", "50")
            .unwrap();
        assert_eq!(c.cache_budget(memory), Some(16 << 30));
        c.set_str(Layer::Global, "system.cache.share", "0").unwrap();
        assert_eq!(c.cache_budget(memory), None, "no limit");
        c.set_str(Layer::Run, "system.cache.budget", "512").unwrap();
        assert_eq!(c.cache_budget(memory), Some(512 << 20), "a size wins");
    }

    #[test]
    fn the_highest_allowed_layer_wins_per_concern() {
        let mut c = config();
        c.set_str(Layer::Global, "interface.graph.zoom", "overview")
            .unwrap();
        c.set_str(Layer::Project, "interface.graph.zoom", "detail")
            .unwrap();
        assert_eq!(
            c.get("interface.graph.zoom").unwrap(),
            (Value::Choice("detail".into()), Layer::Project),
            "the default value still overrides a lower layer"
        );
        c.set_str(Layer::User, "interface.graph.zoom", "overview")
            .unwrap();
        assert_eq!(
            c.get("interface.graph.zoom").unwrap(),
            (Value::Choice("overview".into()), Layer::User)
        );
        c.set_str(Layer::User, "interface.graph.zoom", "detail")
            .unwrap();
        assert_eq!(
            c.get("interface.graph.zoom").unwrap().1,
            Layer::Project,
            "equal to what it inherits: it leaves the user layer"
        );

        c.set_str(Layer::Global, "project.decisions.mode", "human")
            .unwrap();
        c.set_str(Layer::Project, "project.decisions.mode", "full_ai")
            .unwrap();
        assert_eq!(
            c.get("project.decisions.mode").unwrap(),
            (Value::Choice("full_ai".into()), Layer::Project)
        );
        c.set_str(Layer::Run, "project.decisions.mode", "human")
            .unwrap();
        assert_eq!(c.get("project.decisions.mode").unwrap().1, Layer::Run);
    }

    #[test]
    fn a_layer_a_concern_does_not_allow_is_refused() {
        let mut c = config();
        let err = c
            .set_str(Layer::Project, "system.events.journal", "64")
            .unwrap_err();
        assert_eq!(err.reason, "a machine setting is set in: global, run");
        assert!(
            c.set_str(Layer::User, "project.decisions.mode", "human")
                .is_err()
        );
    }

    #[test]
    fn values_are_checked() {
        let mut c = config();
        for (key, raw) in [
            ("system.events.journal", "8"),
            ("system.events.journal", "100.5"),
            ("system.events.journal", "many"),
            ("project.decisions.mode", "coin"),
            ("interface.panels.queue_when_empty", "maybe"),
        ] {
            assert!(
                c.set_str(Layer::Run, key, raw).is_err(),
                "{key} = {raw} accepted"
            );
        }
        c.set_str(Layer::Run, "interface.panels.queue_when_empty", "hidden")
            .unwrap();
        assert!(!c.toggle("interface.panels.queue_when_empty"));
    }

    #[test]
    fn a_file_loads_all_or_nothing_and_reports_every_problem() {
        let mut c = config();
        c.load_str(Layer::Global, "[system.events]\njournal = 64\n")
            .unwrap();
        let problems = c
            .load_str(
                Layer::Global,
                "[system.events]\njournal = 4\n[interface]\nunknown = 1\n[interface.graph]\nzoom = \"overview\"\n",
            )
            .unwrap_err();
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert_eq!(
            c.number("system.events.journal"),
            64.0,
            "the layer kept its previous values"
        );
        let bad = c.load_str(Layer::Global, "journal = [").unwrap_err();
        assert!(bad[0].reason.starts_with("not valid TOML"));
    }

    #[test]
    fn a_layer_round_trips_through_toml() {
        let mut c = config();
        c.set_str(Layer::Global, "system.events.journal", "64")
            .unwrap();
        c.set_str(Layer::Global, "interface.graph.zoom", "overview")
            .unwrap();
        let text = c.layer_toml(Layer::Global);
        assert_eq!(
            text,
            "[interface.graph]\nzoom = \"overview\"\n\n[system.events]\njournal = 64\n"
        );
        let mut d = config();
        d.load_str(Layer::Global, &text).unwrap();
        assert_eq!(d.describe(), c.describe());
    }

    #[test]
    fn pages_and_groups_come_from_the_names() {
        let schema = Schema::core();
        let zoom = schema.get("interface.graph.zoom").unwrap();
        assert_eq!((zoom.page(), zoom.group()), ("interface", "graph"));
        assert_eq!(zoom.input()["type"], "choice");
        let journal = schema.get("system.events.journal").unwrap();
        assert_eq!(
            journal.input()["type"],
            "int",
            "the same shape as a module parameter"
        );
        let two = Setting {
            key: "a.b",
            ..*journal
        };
        assert_eq!((two.page(), two.group()), ("a", "general"));
        let deep = Setting {
            key: "a.b.c.d",
            ..*journal
        };
        assert_eq!(deep.group(), "b.c");
    }

    #[test]
    #[should_panic(expected = "is not named `page.group.name`")]
    fn a_flat_name_is_refused() {
        const FLAT: &[Setting] = &[Setting {
            key: "flat",
            concern: Concern::User,
            label: "",
            about: "",
            kind: Kind::Toggle {
                default: true,
                on: "on",
                off: "off",
            },
        }];
        Schema::new(&[FLAT]);
    }

    #[test]
    fn describe_lists_every_setting_with_its_origin() {
        let mut c = config();
        c.set_str(Layer::Run, "system.events.journal", "32")
            .unwrap();
        let d = c.describe();
        let journal = d["settings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["key"] == "system.events.journal")
            .unwrap();
        assert_eq!(
            (journal["value"].as_f64(), journal["origin"].as_str()),
            (Some(32.0), Some("run"))
        );
        assert_eq!(d["layers"]["run"], 1);
    }
}
