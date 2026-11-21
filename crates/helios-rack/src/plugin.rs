//! A plugin's rack declarations (`[ui] rack` in its manifest, the `ui`
//! interface): its own values and the card of controls the host shows for
//! them (`docs/extensions.md`, "Declared controls").
//!
//! A declaration is data, read and checked before anything of the
//! plugin runs, and the host draws everything:
//!
//! - **values**: each plugin value is declared in the value store at
//!   `plugin/<id>/<key>` with a typed descriptor (a number, a whole
//!   number, a switch, a choice, a text), its texts message ids of the
//!   plugin's own catalogue;
//! - **the card**: an `egui_hud` [`Case`](egui_hud::model::Case) whose parts name the values by
//!   key, each edited by a control of Helios's catalogue or by
//!   [`IMPLICIT`], the value's own managed control (a slider for a
//!   number, a menu for a choice); `egui_hud` [`Patch`]es may add parts
//!   too. A plugin adds no control of its own (controls.md Q5 A).
//!
//! Checked at resolution (controls.md 3.4): every part names a value
//! declared with a type that fits its control, and every control is in
//! the catalogue. A mismatch is a problem, said with its reason, and the
//! part is left out; the rest of the card stays. Never a panic.

use std::collections::BTreeMap;

use egui_hud::model::{CaseItem, ValueSpec};
use egui_hud::registry::Patch;
use helios_controls::Registry;
use helios_core::Value;
use helios_values::{Descriptor, PLUGIN_PATHS, Type};
use serde::Deserialize;
use serde_json::Value as Json;

/// The newest rack declaration format this Helios reads.
pub const FORMAT: u32 = 1;

/// The largest rack declaration file, in bytes.
pub const MAX_BYTES: usize = 256 * 1024;

/// The most values and parts a plugin declares, all files together.
pub const MAX_ITEMS: usize = 200;

/// The control id of a value's own managed control, built from its
/// descriptor.
pub const IMPLICIT: &str = "implicit";

/// The store path of a plugin's value: `plugin/<id>/<key>`.
pub fn path(plugin: &str, key: &str) -> String {
    format!("{PLUGIN_PATHS}{plugin}/{key}")
}

/// The plugin and the key of a plugin value's path.
pub fn split(path: &str) -> Option<(&str, &str)> {
    path.strip_prefix(PLUGIN_PATHS)?.rsplit_once('/')
}

/// A part of the card: a plugin value and the control it is edited by.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub key: String,
    /// Its store path, `plugin/<id>/<key>`.
    pub path: String,
    /// A control of the catalogue; `None`: the value's own ([`IMPLICIT`]).
    pub control: Option<String>,
}

/// What a plugin declares for the rack, once checked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Declared {
    pub plugin: String,
    /// Its card's title, a message id of its catalogue.
    pub title: Option<String>,
    /// Its values, by key, in the order declared, each with its
    /// descriptor (texts are message ids of its catalogue).
    pub values: Vec<(String, Descriptor)>,
    /// The card's parts, in order.
    pub parts: Vec<Part>,
    /// What was left out, each with its reason.
    pub problems: Vec<String>,
}

impl Declared {
    /// Nothing declared for `plugin` yet.
    pub fn new(plugin: &str) -> Self {
        Self {
            plugin: plugin.into(),
            ..Self::default()
        }
    }

    /// The descriptor of a value, by key.
    pub fn value(&self, key: &str) -> Option<&Descriptor> {
        self.values.iter().find(|(k, _)| k == key).map(|(_, d)| d)
    }

    fn items(&self) -> usize {
        self.values.len() + self.parts.len()
    }

    /// Reads one declaration file (`file` names it in the problems) and
    /// adds what passes its checks against `catalogue`; what does not is
    /// left out with its reason.
    pub fn read(&mut self, file: &str, text: &str, catalogue: &Registry) {
        if text.len() > MAX_BYTES {
            self.problems
                .push(format!("{file}: {} bytes, at most {MAX_BYTES}", text.len()));
            return;
        }
        let decl: FileDecl = match serde_json::from_str(text) {
            Ok(d) => d,
            Err(e) => {
                self.problems.push(format!("{file}: not read: {e}"));
                return;
            }
        };
        if decl.version > FORMAT {
            self.problems.push(format!(
                "{file}: format {} is newer than this Helios reads ({FORMAT})",
                decl.version
            ));
            return;
        }
        let prefix = format!("{}-", helios_i18n::ids::slug(&self.plugin));
        for (key, v) in decl.values {
            if self.items() >= MAX_ITEMS {
                self.problems
                    .push(format!("{file}: more than {MAX_ITEMS} values and parts"));
                return;
            }
            match value(&key, v, &prefix) {
                Ok(_) if self.value(&key).is_some() => {
                    self.problems
                        .push(format!("{file}: value `{key}` is declared twice"));
                }
                Ok(d) => self.values.push((key, d)),
                Err(e) => self.problems.push(format!("{file}: value `{key}`: {e}")),
            }
        }
        if let Some(t) = decl.title {
            if t.starts_with(&prefix) {
                self.title = Some(t);
            } else {
                self.problems.push(format!(
                    "{file}: the title `{t}` is not a message id of the plugin's catalogue (`{prefix}…`)"
                ));
            }
        }
        let mut wanted: Vec<(String, String)> = Vec::new();
        if let Some(case) = decl.case {
            let items = case.get("items").and_then(Json::as_array);
            for item in items.into_iter().flatten() {
                match serde_json::from_value::<CaseItem>(item.clone()) {
                    Ok(CaseItem::Part(p)) => wanted.push((p.name, p.control)),
                    Ok(CaseItem::Chrome(_)) => self
                        .problems
                        .push(format!("{file}: the card's chrome is the host's, left out")),
                    Err(e) => self
                        .problems
                        .push(format!("{file}: an item of the card is not read: {e}")),
                }
            }
        }
        for p in decl.patches {
            match serde_json::from_value::<Patch>(p) {
                Ok(p) if p.module != self.plugin => self.problems.push(format!(
                    "{file}: patch {}/{}: a plugin patches its own values only",
                    p.module, p.param
                )),
                Ok(p) => {
                    let jack = catalogue
                        .get(&p.control)
                        .is_none_or(|c| c.connector.jacks.iter().any(|j| j.name == p.jack));
                    if jack {
                        wanted.push((p.param, p.control));
                    } else {
                        self.problems.push(format!(
                            "{file}: patch {}/{}: control `{}` has no jack `{}`",
                            p.module, p.param, p.control, p.jack
                        ));
                    }
                }
                Err(e) => self
                    .problems
                    .push(format!("{file}: a patch is not read: {e}")),
            }
        }
        for (key, control) in wanted {
            if self.items() >= MAX_ITEMS {
                self.problems
                    .push(format!("{file}: more than {MAX_ITEMS} values and parts"));
                return;
            }
            match self.part(&key, &control, catalogue) {
                Ok(part) => self.parts.push(part),
                Err(e) => self.problems.push(format!("{file}: part `{key}`: {e}")),
            }
        }
    }

    /// A part, checked: its value declared once on the card, its control
    /// in the catalogue with a value that fits.
    fn part(&self, key: &str, control: &str, catalogue: &Registry) -> Result<Part, String> {
        let d = self
            .value(key)
            .ok_or_else(|| format!("no value `{key}` is declared"))?;
        if self.parts.iter().any(|p| p.key == key) {
            return Err("already on the card".into());
        }
        let control = if control == IMPLICIT {
            None
        } else {
            let c = catalogue
                .get(control)
                .ok_or_else(|| format!("unknown control `{control}`"))?;
            let fits = match &c.value {
                ValueSpec::Number(_) => matches!(d.ty, Type::Float | Type::Int),
                ValueSpec::Bool { .. } => d.ty == Type::Bool,
                ValueSpec::OneOf { .. } => d.ty == Type::Choice,
                ValueSpec::Other { .. } => false,
            };
            if !fits {
                return Err(format!(
                    "control `{control}` does not fit a value of type {:?}",
                    d.ty
                ));
            }
            Some(control.to_owned())
        };
        Ok(Part {
            key: key.into(),
            path: path(&self.plugin, key),
            control,
        })
    }
}

/// A rack declaration file, as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDecl {
    version: u32,
    #[serde(default)]
    title: Option<String>,
    /// By key, each read alone so one bad value is left out alone.
    #[serde(default)]
    values: BTreeMap<String, Json>,
    /// An `egui_hud` case: its items read one by one.
    #[serde(default)]
    case: Option<Json>,
    #[serde(default)]
    patches: Vec<Json>,
}

/// A value as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValueDecl {
    #[serde(rename = "type")]
    ty: String,
    label: String,
    #[serde(default)]
    description: Option<String>,
    default: Json,
    #[serde(default)]
    min: Option<f64>,
    #[serde(default)]
    max: Option<f64>,
    #[serde(default)]
    step: Option<f64>,
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    precision: Option<u32>,
    #[serde(default)]
    logarithmic: bool,
    #[serde(default)]
    choices: Vec<String>,
}

/// Whether `key` may name a value: short, lowercase letters, digits,
/// `-` and `_`.
fn key_ok(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// A value's descriptor, checked.
fn value(key: &str, v: Json, prefix: &str) -> Result<Descriptor, String> {
    if !key_ok(key) {
        return Err("a key is lowercase letters, digits, `-` and `_`, at most 64".into());
    }
    let v: ValueDecl = serde_json::from_value(v).map_err(|e| e.to_string())?;
    for t in std::iter::once(&v.label).chain(&v.description) {
        if !t.starts_with(prefix) {
            return Err(format!(
                "`{t}` is not a message id of the plugin's catalogue (`{prefix}…`)"
            ));
        }
    }
    let range = || match (v.min, v.max) {
        (Some(lo), Some(hi)) if lo < hi && lo.is_finite() && hi.is_finite() => Ok((lo, hi)),
        _ => Err("a number needs `min` below `max`".to_string()),
    };
    let mut d = match v.ty.as_str() {
        "float" => {
            let (lo, hi) = range()?;
            let x = v.default.as_f64().ok_or("its default is not a number")?;
            let mut d = Descriptor::float(&v.label, lo, hi, x.clamp(lo, hi));
            if let Some(s) = v.step.filter(|s| *s > 0.0) {
                d.step = s;
                d.coarse = s * 10.0;
            }
            if let Some(p) = v.precision {
                d.precision = p.min(6);
            }
            d.logarithmic = v.logarithmic && lo > 0.0;
            d
        }
        "int" => {
            let (lo, hi) = range()?;
            let x = v
                .default
                .as_i64()
                .ok_or("its default is not a whole number")?;
            Descriptor::int(
                &v.label,
                lo as i64,
                hi as i64,
                x.clamp(lo as i64, hi as i64),
            )
        }
        "bool" => Descriptor::bool(
            &v.label,
            v.default
                .as_bool()
                .ok_or("its default is not true or false")?,
        ),
        "choice" => {
            let x = v.default.as_str().ok_or("its default is not a choice")?;
            if v.choices.is_empty() || !v.choices.iter().any(|c| c == x) {
                return Err(format!("its default `{x}` is not one of its choices"));
            }
            let choices: Vec<&str> = v.choices.iter().map(String::as_str).collect();
            Descriptor::choice(&v.label, &choices, x)
        }
        "text" => Descriptor::text(
            &v.label,
            v.default.as_str().ok_or("its default is not a text")?,
        ),
        other => return Err(format!("unknown type `{other}`")),
    };
    d.description = v.description.unwrap_or_default();
    d.unit = v.unit;
    Ok(d)
}

/// A value of the store as the plugin is told it (`ui/changed`).
pub fn to_json(v: &Value) -> Json {
    helios_values::value_to_json(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_controls::HeliosRegistry as _;
    use serde_json::json;

    const CAMERA: &str = "org.example.camera";

    fn read(decl: &Json) -> Declared {
        let mut d = Declared::new(CAMERA);
        d.read("ui/controls.json", &decl.to_string(), &Registry::standard());
        d
    }

    fn camera() -> Json {
        json!({
            "version": 1,
            "title": "org-example-camera-capture",
            "values": {
                "iso": {"type": "choice", "label": "org-example-camera-iso",
                        "choices": ["auto", "100", "400"], "default": "auto"},
                "shutter": {"type": "float", "label": "org-example-camera-shutter",
                            "min": 0.001, "max": 30.0, "default": 0.008, "unit": "s",
                            "logarithmic": true},
                "bias": {"type": "float", "label": "org-example-camera-bias",
                         "min": -3.0, "max": 3.0, "default": 0.0},
            },
            "case": {"items": [
                {"part": {"name": "iso", "control": "implicit"}},
                {"part": {"name": "shutter", "control": "implicit"}},
                {"part": {"name": "bias", "control": "exposure"}},
            ]},
        })
    }

    #[test]
    fn a_camera_s_controls_are_its_own_values_on_catalogue_controls() {
        let d = read(&camera());
        assert_eq!(d.problems, Vec::<String>::new());
        assert_eq!(d.values.len(), 3);
        let keys: Vec<_> = d.parts.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(keys, ["iso", "shutter", "bias"]);
        assert_eq!(d.parts[0].path, "plugin/org.example.camera/iso");
        assert_eq!(d.parts[2].control.as_deref(), Some("exposure"));
        assert!(d.value("shutter").unwrap().logarithmic);
        assert_eq!(split(&d.parts[1].path), Some((CAMERA, "shutter")));
    }

    #[test]
    fn an_unknown_control_or_a_misfit_is_left_out_and_the_rest_stays() {
        let mut decl = camera();
        decl["case"]["items"] = json!([
            {"part": {"name": "iso", "control": "hologram"}},
            {"part": {"name": "iso", "control": "exposure"}},
            {"part": {"name": "ghost", "control": "implicit"}},
            {"part": {"name": "shutter", "control": "implicit"}},
            {"wormhole": {}},
        ]);
        decl["patches"] = json!([
            {"module": "org.other", "param": "iso", "control": "exposure", "jack": "value"},
            {"module": CAMERA, "param": "bias", "control": "exposure", "jack": "value"},
        ]);
        decl["values"]["broken"] = json!({"type": "float", "label": "org-example-camera-x",
                                          "min": 2.0, "max": 1.0, "default": 0.0});
        decl["values"]["stolen"] = json!({"type": "bool", "label": "gui-ok", "default": true});
        let d = read(&decl);
        let keys: Vec<_> = d.parts.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(keys, ["shutter", "bias"]);
        let said = d.problems.join("\n");
        for reason in [
            "unknown control `hologram`",
            "control `exposure` does not fit a value of type Choice",
            "no value `ghost` is declared",
            "an item of the card is not read",
            "a plugin patches its own values only",
            "value `broken`: a number needs `min` below `max`",
            "value `stolen`: `gui-ok` is not a message id",
        ] {
            assert!(said.contains(reason), "{reason}\n{said}");
        }
    }

    #[test]
    fn a_newer_or_oversized_file_is_refused_whole() {
        let mut decl = camera();
        decl["version"] = json!(FORMAT + 1);
        let d = read(&decl);
        assert!(d.values.is_empty() && d.parts.is_empty());
        assert!(d.problems[0].contains("newer"), "{:?}", d.problems);
        let mut d = Declared::new(CAMERA);
        d.read(
            "big.json",
            &" ".repeat(MAX_BYTES + 1),
            &Registry::standard(),
        );
        assert!(d.problems[0].contains("at most"), "{:?}", d.problems);
        let many: serde_json::Map<String, Json> = (0..=MAX_ITEMS)
            .map(|i| {
                (
                    format!("v{i}"),
                    json!({"type": "bool", "label": "org-example-camera-v", "default": false}),
                )
            })
            .collect();
        let d = read(&json!({"version": 1, "values": many}));
        assert_eq!(d.values.len(), MAX_ITEMS);
        assert!(d.problems.iter().any(|p| p.contains("more than")));
    }
}
