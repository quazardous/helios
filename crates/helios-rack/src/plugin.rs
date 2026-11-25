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
//! - **tools on the image**: each a [`ToolDecl`], its handles an
//!   `egui_hud` handles file (`egui_hud-handles/1`, the kinds that carry
//!   their maths: a point, a radius, a feather, an angle, a band) on the
//!   plugin's number values, with the tool's own range per handle. The
//!   viewport draws them over the image and owns their drags (a drag is
//!   one undo step, in the plugins' scope). A kind this Helios does not
//!   know is left out with its reason, the tool's other handles kept;
//!   `custom` (code) never comes from a file.
//!
//! Checked at resolution (controls.md 3.4): every part names a value
//! declared with a type that fits its control, and every control is in
//! the catalogue. A mismatch is a problem, said with its reason, and the
//! part is left out; the rest of the card stays. Never a panic.

use std::collections::BTreeMap;

use egui_hud::handle::{Handles, Params as HandleParams};
use egui_hud::model::{CaseItem, ValueSpec};
use egui_hud::registry::Patch;
use helios_controls::Registry;
use helios_core::Value;
use helios_values::{Descriptor, PLUGIN_PATHS, Type};
use serde::Deserialize;
use serde_json::Value as Json;

use crate::tools::{Tool, ToolHandle};

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
    /// Its tools on the image, in order.
    pub tools: Vec<PluginTool>,
    /// What was left out, each with its reason.
    pub problems: Vec<String>,
    /// Its tools' refusals, each `(tool, reason)`: a handle left out,
    /// or a tool left out whole. Each is among the problems too.
    pub refused_tools: Vec<(String, String)>,
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
        self.values.len()
            + self.parts.len()
            + self
                .tools
                .iter()
                .map(|t| t.tool.handles.len())
                .sum::<usize>()
    }

    /// Its tool named `name`.
    pub fn tool(&self, name: &str) -> Option<&PluginTool> {
        self.tools.iter().find(|t| t.tool.control == name)
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
        for t in decl.tools {
            let decl: ToolDecl = match serde_json::from_value(t) {
                Ok(d) => d,
                Err(e) => {
                    self.problems
                        .push(format!("{file}: a tool is not read: {e}"));
                    continue;
                }
            };
            let name = decl.name.clone();
            match self.tool_of(decl, &prefix) {
                Ok((tool, problems)) => {
                    let ok = !tool.tool.handles.is_empty();
                    for p in problems {
                        self.problems.push(format!("{file}: tool `{name}`: {p}"));
                        self.refused_tools.push((name.clone(), p));
                    }
                    if self.items() + tool.tool.handles.len() > MAX_ITEMS {
                        self.problems
                            .push(format!("{file}: more than {MAX_ITEMS} values and parts"));
                        return;
                    }
                    if ok {
                        self.tools.push(tool);
                    } else {
                        self.problems
                            .push(format!("{file}: tool `{name}`: no handle left, left out"));
                        self.refused_tools
                            .push((name, "no handle left, left out".into()));
                    }
                }
                Err(e) => {
                    self.problems.push(format!("{file}: tool `{name}`: {e}"));
                    self.refused_tools.push((name, e));
                }
            }
        }
    }

    /// A tool, checked: its name free, its texts the plugin's, its
    /// handles of kinds this Helios knows (the others left out, each
    /// with its reason) on number values the plugin declares, its ranges
    /// on handles it has.
    fn tool_of(&self, d: ToolDecl, prefix: &str) -> Result<(PluginTool, Vec<String>), String> {
        if !key_ok(&d.name) {
            return Err("a name is lowercase letters, digits, `-` and `_`, at most 64".into());
        }
        if self.tool(&d.name).is_some() {
            return Err("declared twice".into());
        }
        if !d.label.starts_with(prefix) {
            return Err(format!(
                "`{}` is not a message id of the plugin's catalogue (`{prefix}…`)",
                d.label
            ));
        }
        if let Some(w) = &d.when
            && self.value(w).is_none_or(|v| v.ty != Type::Bool)
        {
            return Err(format!("`when` names no switch value: `{w}`"));
        }
        if let Some(m) = &d.module
            && !key_ok(m)
        {
            return Err(format!("`module` names no module: `{m}`"));
        }
        let handles = Handles::from_json(&d.handles.to_string())?;
        let (known, skipped) = handles.split();
        let mut problems: Vec<String> = skipped.iter().map(ToString::to_string).collect();
        let mut tool = match &d.module {
            // The host's parameters: the nodes' own descriptors hold
            // them to their types and ranges, checked where the tool is
            // shown (a node of that module with those parameters).
            Some(m) => Tool::new(&d.name, m),
            None => Tool::plugin(&d.name, &self.plugin),
        };
        for h in known {
            let asked = Asked::default();
            h.position(&asked);
            let _ = h.drag(&asked, [0.0, 0.0]);
            let reads = asked.names.into_inner();
            let writes = h.writes();
            // It writes numbers; it may read a switch (a feather in
            // proportion while one is on).
            let bad = writes
                .iter()
                .find(|n| self.value(n).is_none_or(|v| v.ty != Type::Float))
                .or_else(|| {
                    reads.iter().find(|n| {
                        self.value(n)
                            .is_none_or(|v| !matches!(v.ty, Type::Float | Type::Bool))
                    })
                });
            if d.module.is_none()
                && let Some(bad) = bad
            {
                problems.push(format!(
                    "handle `{}`: `{bad}` is not a number value the plugin declares, left out",
                    h.name
                ));
                continue;
            }
            let mut th = ToolHandle::new(h.name.clone(), h);
            if let Some([lo, hi]) = d.ranges.get(&th.handle.name) {
                if lo < hi && lo.is_finite() && hi.is_finite() {
                    th = th.range(*lo, *hi);
                } else {
                    problems.push(format!("range of `{}`: `min` below `max`", th.handle.name));
                }
            }
            tool.handles.push(th);
        }
        for name in d.ranges.keys() {
            if tool.handle(name).is_none() && !skipped.iter().any(|s| &s.name == name) {
                problems.push(format!("a range names no handle: `{name}`"));
            }
        }
        Ok((
            PluginTool {
                label: d.label,
                when: d.when,
                tool,
            },
            problems,
        ))
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

/// A plugin's tool on the image, once checked.
#[derive(Clone, Debug)]
pub struct PluginTool {
    /// Its name for people, a message id of the plugin's catalogue.
    pub label: String,
    /// The switch value it is shown while on; `None`: always, while
    /// the plugin is enabled and a photo is open.
    pub when: Option<String>,
    /// Its handles, on the plugin's values ([`crate::tools::Target::Plugin`],
    /// its `module` the plugin's id) or on a host module's nodes
    /// ([`crate::tools::Target::Nodes`], its `module` that module's
    /// name); its `control` is its name.
    pub tool: Tool,
}

impl PluginTool {
    /// The host module whose nodes it edits, if it edits the host's
    /// parameters rather than the plugin's values.
    pub fn host(&self) -> Option<&str> {
        (self.tool.target == crate::tools::Target::Nodes).then_some(self.tool.module.as_str())
    }
}

/// By what they show: their names, labels, switches, handles (by name
/// and kind) and ranges (a handle's maths may be code, which has no
/// equality).
impl PartialEq for PluginTool {
    fn eq(&self, o: &Self) -> bool {
        let shown = |t: &PluginTool| {
            t.tool
                .handles
                .iter()
                .map(|h| (h.name().to_owned(), h.handle.kind.name(), h.range))
                .collect::<Vec<_>>()
        };
        self.label == o.label
            && self.when == o.when
            && self.tool.control == o.tool.control
            && self.tool.module == o.tool.module
            && shown(self) == shown(o)
    }
}

/// A tool as written: a name, its label, the switch that shows it, its
/// handles (an `egui_hud` handles file) and the tool's own range per
/// handle.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDecl {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub when: Option<String>,
    /// The host module whose nodes it edits (`mask_circle`): its handles
    /// then read and write those nodes' parameters, not the plugin's
    /// values, and the plugin needs the `edit.controls` capability,
    /// granted, for it to show.
    #[serde(default)]
    pub module: Option<String>,
    /// `{"format": "egui_hud-handles/1", "handles": [...]}`.
    pub handles: Json,
    /// By handle name: `[min, max]` for the first value it writes.
    #[serde(default)]
    pub ranges: BTreeMap<String, [f64; 2]>,
}

/// Parameters that answer 1.0 to every name, and keep the names asked:
/// what a handle reads.
#[derive(Default)]
struct Asked {
    names: std::cell::RefCell<Vec<String>>,
}

impl HandleParams for Asked {
    fn float(&self, name: &str) -> Option<f64> {
        self.names.borrow_mut().push(name.to_owned());
        Some(1.0)
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
    /// Its tools on the image, each read alone.
    #[serde(default)]
    tools: Vec<Json>,
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
    fn a_tool_on_a_host_module_edits_its_nodes_not_the_plugin_s_values() {
        let mut decl = camera();
        decl["tools"] = json!([{
            "name": "spot",
            "label": "org-example-camera-spot",
            "module": "mask_circle",
            "handles": {"format": "egui_hud-handles/1", "handles": [
                {"name": "centre", "kind": "point", "look": "move"},
                {"name": "radius", "kind": "radius", "radius": "radius"},
            ]},
        }, {
            "name": "bad",
            "label": "org-example-camera-bad",
            "module": "Not A Module",
            "handles": {"format": "egui_hud-handles/1", "handles": []},
        }]);
        let d = read(&decl);
        let t = d.tool("spot").expect("kept");
        assert_eq!(t.host(), Some("mask_circle"));
        assert_eq!(t.tool.target, crate::tools::Target::Nodes);
        assert_eq!(t.tool.handles.len(), 2, "{:?}", d.problems);
        assert!(d.tool("bad").is_none());
        assert!(
            d.problems
                .iter()
                .any(|p| p.contains("`module` names no module")),
            "{:?}",
            d.problems
        );
        // The refusal, by tool, for the doctor.
        assert!(
            d.refused_tools
                .iter()
                .any(|(tool, why)| tool == "bad" && why.contains("`module` names no module")),
            "{:?}",
            d.refused_tools
        );
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
