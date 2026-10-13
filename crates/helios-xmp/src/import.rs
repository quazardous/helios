//! From a sidecar's history to Helios modules and parameters, with a report
//! of everything not carried over.
//!
//! An item Helios cannot apply (a module it does not have, a params
//! version it cannot read, a blend it does not do, an item it cannot read
//! at all) is never dropped and never aborts the import: it is reported in
//! [`Report::not_applied`] with its name, version and reason, and kept
//! with its raw params in the edit ([`Imported::darktable`]), so a later
//! Helios can apply it and the sidecar can be written back as it was.

use helios_core::{Arg, Message, Params, Value as Param};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::layout::{self, Layouts};
use crate::legacy;
use crate::sidecar::{HistoryItem, Sidecar};

/// A Helios module instance an edit asks for.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Node {
    /// The node's name in the network, unique in the edit: the operation,
    /// then the instance's name (see [`HistoryItem::instance_name`]), or
    /// its number when it has no name and is not the first.
    pub name: String,
    /// darktable's operation and instance.
    pub operation: String,
    pub multi_name: String,
    /// darktable's instance number (`multi_priority`), 0 for the first.
    pub instance: u32,
    /// The Helios module.
    pub module: String,
    pub params: Params,
}

/// A node's name for an item, unique among `taken`.
fn node_name(item: &HistoryItem, taken: &[Node]) -> String {
    let base = if !item.instance_name().is_empty() {
        format!("{} {}", item.operation, item.instance_name())
    } else if item.multi_priority > 0 {
        format!("{} {}", item.operation, item.multi_priority)
    } else {
        item.operation.clone()
    };
    // Two instances may share a name in darktable.
    let mut name = base.clone();
    let mut n = 1;
    while taken.iter().any(|t| t.name == name) {
        name = format!("{base} #{n}");
        n += 1;
    }
    name
}

/// Why a history item is kept without being applied.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reason {
    /// Helios has no module doing what this darktable module does.
    UnknownModule,
    /// Helios cannot read params of this version; it reads `known`.
    UnsupportedVersion { known: Vec<u32> },
    /// The item, or its params, could not be read.
    Unreadable { error: String },
    /// The item blends its result (a mask, an opacity, a blend mode),
    /// which Helios does not do yet: applied without it, it would change
    /// the whole image instead of a part.
    Blending { blendop_version: u32 },
    /// Blend params of a version Helios does not know how to read.
    UnknownBlendVersion { blendop_version: u32 },
    /// Another instance of a module darktable runs only once.
    ExtraInstance,
}

impl Reason {
    /// In a few words, for people, by key.
    pub fn describe(&self) -> Message {
        match self {
            Reason::UnknownModule => Message::new("xmp-reason-unknown-module", "no Helios module"),
            Reason::UnsupportedVersion { known } => {
                let known: Vec<String> = known.iter().map(|v| format!("v{v}")).collect();
                Message::new(
                    "xmp-reason-unsupported-version",
                    "version not supported (reads { $known })",
                )
                .with("known", known.join(", "))
            }
            Reason::Unreadable { error } => {
                Message::new("xmp-reason-unreadable", "unreadable: { $error }").with("error", error)
            }
            Reason::Blending { .. } => {
                Message::new("xmp-reason-blending", "blending not supported")
            }
            Reason::UnknownBlendVersion { blendop_version } => Message::new(
                "xmp-reason-unknown-blend-version",
                "blend params v{ $version } not supported",
            )
            .with("version", *blendop_version),
            Reason::ExtraInstance => Message::new(
                "xmp-reason-extra-instance",
                "a second instance of a single-instance module",
            ),
        }
    }
}

/// A history item in force that Helios kept without applying.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NotApplied {
    pub operation: String,
    /// darktable's instance number (`multi_priority`) and name.
    pub instance: u32,
    pub multi_name: String,
    /// The version of its params.
    pub version: u32,
    /// Its name for people: operation, instance and version.
    pub label: String,
    pub reason: Reason,
}

/// What was not carried over, and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    /// Items in force that Helios kept but did not apply, in pipe order.
    pub not_applied: Vec<NotApplied>,
    /// Older versions of modules taken for their presence only: their
    /// params were not read. `(operation, version)`.
    pub params_not_read: Vec<(String, u32)>,
    /// Items migrated from an older version: `(operation, from, to)`.
    pub migrated: Vec<(String, u32, u32)>,
    /// Values without a Helios counterpart: `(operation, field, value)`.
    pub dropped_fields: Vec<(String, String, Value)>,
    /// Disabled items, kept out of the network.
    pub disabled: Vec<String>,
    /// The pipe order the modules were put in: darktable's preset name, or
    /// `custom` for an order the sidecar spells out.
    pub pipe_order: String,
    /// Items the sidecar's order does not place, `(operation, instance)`:
    /// placed next to their module's other instances, or by the rank
    /// darktable's preset gives them.
    pub unordered: Vec<(String, u32)>,
    /// History items in force, disabled ones included.
    pub items: usize,
    /// Items darktable adds when it reads an older sidecar, which the file
    /// does not hold (see [`HistoryItem::inserted`]), by label.
    pub inserted: Vec<String>,
}

impl Report {
    /// One line saying what was not applied, grouped by reason, or `None`
    /// when everything in force was applied: what an import warns once.
    /// By key (`xmp-not-applied`), each reason too.
    pub fn summary(&self) -> Option<Message> {
        if self.not_applied.is_empty() {
            return None;
        }
        let mut groups: Vec<(Message, Vec<&str>)> = Vec::new();
        for item in &self.not_applied {
            let reason = item.reason.describe();
            match groups.iter_mut().find(|(r, _)| *r == reason) {
                Some((_, labels)) => labels.push(&item.label),
                None => groups.push((reason, vec![&item.label])),
            }
        }
        let groups = groups.into_iter().map(|(reason, labels)| {
            Message::new("xmp-not-applied-group", "{ $labels } ({ $reason })")
                .with("labels", labels.join(", "))
                .with("reason", reason)
        });
        Some(
            Message::new(
                "xmp-not-applied",
                "{ $count } of { $items } history items not applied, kept for later: { $groups }",
            )
            .with("count", self.not_applied.len())
            .with("items", self.items)
            .with("groups", Arg::list(groups, "; ")),
        )
    }
}

/// An edit read from a sidecar.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Imported {
    /// In darktable's pipe order.
    pub nodes: Vec<Node>,
    pub report: Report,
    /// The darktable edit as it came: the sidecar's text, and each history
    /// item with its raw params and what became of it (see
    /// [`Imported::edit`]). `Null` for an edit not read from a sidecar.
    pub darktable: Value,
}

fn float(fields: &Map<String, Value>, name: &str) -> Option<f64> {
    fields.get(name).and_then(Value::as_f64)
}

fn text<'a>(fields: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    fields.get(name).and_then(Value::as_str)
}

fn set(p: &mut Params, name: &str, v: Option<f64>) {
    if let Some(v) = v {
        p.set(name, Param::Float(v));
    }
}

/// One darktable module's decoded params turned into a Helios module's.
struct Mapped {
    module: &'static str,
    params: Params,
    /// Fields read into `params`, or knowingly replaced by something else
    /// (the camera's description).
    used: Vec<&'static str>,
}

/// Fields whose value carries nothing when it is its default.
fn is_neutral(value: &Value) -> bool {
    match value {
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::Bool(b) => !b,
        Value::String(s) => s.is_empty(),
        Value::Array(items) => items.iter().all(is_neutral),
        _ => false,
    }
}

fn map(operation: &str, f: &Map<String, Value>, report: &mut Report) -> Option<Mapped> {
    let mut p = Params::new();
    let (module, used): (&'static str, Vec<&'static str>) = match operation {
        "rawprepare" => (
            "rawprepare",
            // The crop and levels come from the raw's description in Helios.
            vec![
                "left",
                "top",
                "right",
                "bottom",
                "raw_black_level_separate",
                "raw_white_point",
            ],
        ),
        "temperature" => {
            p.set("mode", Param::Choice("user".into()));
            set(&mut p, "red", float(f, "red"));
            set(&mut p, "green", float(f, "green"));
            set(&mut p, "blue", float(f, "blue"));
            (
                "whitebalance",
                vec!["red", "green", "blue", "various", "preset"],
            )
        }
        "highlights" => {
            if text(f, "mode") != Some("DT_IOP_HIGHLIGHTS_CLIP") {
                report.dropped_fields.push((
                    operation.into(),
                    "mode".into(),
                    f.get("mode").cloned().unwrap_or(Value::Null),
                ));
            }
            set(&mut p, "clip", float(f, "clip"));
            ("highlights", vec!["mode", "clip"])
        }
        "demosaic" => ("demosaic", vec![]),
        "colorin" => ("colorin", vec!["type_work", "filename_work"]),
        "exposure" => {
            set(&mut p, "exposure", float(f, "exposure"));
            set(&mut p, "black", float(f, "black"));
            if let Some(b) = f.get("compensate_exposure_bias").and_then(Value::as_bool) {
                p.set("compensate_bias", Param::Bool(b));
            }
            (
                "exposure",
                vec!["mode", "exposure", "black", "compensate_exposure_bias"],
            )
        }
        "sigmoid" => {
            set(&mut p, "contrast", float(f, "middle_grey_contrast"));
            set(&mut p, "skew", float(f, "contrast_skewness"));
            set(&mut p, "white_target", float(f, "display_white_target"));
            set(&mut p, "black_target", float(f, "display_black_target"));
            set(&mut p, "preserve_hue", float(f, "hue_preservation"));
            if text(f, "color_processing") != Some("DT_SIGMOID_METHOD_PER_CHANNEL") {
                report.dropped_fields.push((
                    operation.into(),
                    "color_processing".into(),
                    f.get("color_processing").cloned().unwrap_or(Value::Null),
                ));
            }
            (
                "sigmoid",
                vec![
                    "middle_grey_contrast",
                    "contrast_skewness",
                    "display_white_target",
                    "display_black_target",
                    "hue_preservation",
                    "color_processing",
                ],
            )
        }
        _ => return None,
    };
    Some(Mapped {
        module,
        params: p,
        used,
    })
}

/// The mapped modules darktable runs once (`IOP_FLAGS_ONE_INSTANCE`).
fn single_instance(operation: &str) -> bool {
    matches!(
        operation,
        "rawprepare" | "temperature" | "highlights" | "demosaic" | "colorin"
    )
}

/// The mapped modules whose output darktable blends
/// (`IOP_FLAGS_SUPPORTS_BLENDING`); the others ignore their blend params.
fn blends(operation: &str) -> bool {
    matches!(operation, "highlights" | "exposure" | "sigmoid")
}

/// The newest blend params version darktable writes (`DEVELOP_BLEND_VERSION`).
const BLEND_VERSION: u32 = 14;

/// Whether an item's blend params leave its output as it is.
fn check_blend(item: &HistoryItem) -> Result<(), Reason> {
    let p = &item.blendop_params;
    // darktable reads all-zero blend params as its defaults: no blending.
    if !blends(&item.operation) || p.iter().all(|b| *b == 0) {
        return Ok(());
    }
    let blendop_version = item.blendop_version.unwrap_or(0);
    if !(1..=BLEND_VERSION).contains(&blendop_version) {
        return Err(Reason::UnknownBlendVersion { blendop_version });
    }
    // The first word is the blend mode up to version 4, the mask mode
    // since; zero means off in both (`DEVELOP_MASK_DISABLED`).
    let first = p
        .get(..4)
        .map(|w| u32::from_le_bytes(w.try_into().expect("4 bytes")))
        .ok_or_else(|| Reason::Unreadable {
            error: format!("blend params of {} bytes", p.len()),
        })?;
    if first == 0 {
        Ok(())
    } else {
        Err(Reason::Blending { blendop_version })
    }
}

/// An item in force turned into a Helios node, or why it cannot be.
fn apply(item: &HistoryItem, layouts: &Layouts, report: &mut Report) -> Result<Mapped, Reason> {
    if let Some(error) = &item.unreadable {
        return Err(Reason::Unreadable {
            error: error.clone(),
        });
    }
    let known = layouts.versions(&item.operation);
    let Some(&current) = known.iter().max() else {
        return Err(Reason::UnknownModule);
    };
    if map(&item.operation, &Map::new(), &mut Report::default()).is_none() {
        return Err(Reason::UnknownModule);
    }
    if item.multi_priority > 0 && single_instance(&item.operation) {
        return Err(Reason::ExtraInstance);
    }
    check_blend(item)?;
    let layout = layouts
        .get(&item.operation, current)
        .expect("a known version");
    let params = if item.modversion == current {
        item.params.clone()
    } else if legacy::presence_only(&item.operation) {
        report
            .params_not_read
            .push((item.operation.clone(), item.modversion));
        // An empty decode: the mapping reads nothing from these modules.
        return Ok(map(&item.operation, &Map::new(), report).expect("mapped"));
    } else {
        match legacy::upgrade(&item.operation, item.modversion, &item.params, current) {
            Some(Ok(bytes)) => {
                report
                    .migrated
                    .push((item.operation.clone(), item.modversion, current));
                bytes
            }
            Some(Err(error)) => return Err(Reason::Unreadable { error }),
            None => return Err(Reason::UnsupportedVersion { known }),
        }
    };
    let decoded = match layout::decode(layout, &params) {
        Ok(Value::Object(fields)) => fields,
        Ok(other) => {
            return Err(Reason::Unreadable {
                error: format!("params decode to {other}, not fields"),
            });
        }
        Err(error) => return Err(Reason::Unreadable { error }),
    };
    let mapped = map(&item.operation, &decoded, report).expect("mapped");
    for (name, value) in &decoded {
        let reported = report
            .dropped_fields
            .iter()
            .any(|(o, f, _)| o == &item.operation && f == name);
        if !mapped.used.contains(&name.as_str()) && !reported && !is_neutral(value) {
            report
                .dropped_fields
                .push((item.operation.clone(), name.clone(), value.clone()));
        }
    }
    Ok(mapped)
}

/// Where an imported edit comes from, as a version records it.
pub const ORIGIN: &str = "import:xmp";

impl Imported {
    /// The edit as a network after a source named `src`, the modules
    /// chained in pipe order (output to input), in the shape of a
    /// `helios-node` network description: what the vault keeps as a
    /// version (`Version::new(image, parents, imported.edit(), ORIGIN, now)`).
    /// The darktable edit it came from, items not applied included, is
    /// kept under `darktable` (see [`Imported::darktable`]).
    pub fn edit(&self) -> Value {
        let mut modules = Vec::new();
        let mut connections = Vec::new();
        let mut previous = ("src".to_string(), "image");
        for n in &self.nodes {
            modules.push(json!({
                "name": n.name,
                "module": n.module,
                "params": n.params,
            }));
            connections.push(json!({
                "from": previous.0,
                "port": previous.1,
                "to": n.name,
                "input": "input",
            }));
            previous = (n.name.clone(), "output");
        }
        let mut edit = json!({ "modules": modules, "connections": connections });
        if !self.darktable.is_null() {
            edit["darktable"] = self.darktable.clone();
        }
        edit
    }
}

/// What became of a history item.
fn item_json(item: &HistoryItem, status: &Value) -> Value {
    let attributes: Map<String, Value> = item
        .attributes
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    let mut out = json!({
        "num": item.num,
        "operation": item.operation,
        "instance": item.multi_priority,
        "multi_name": item.multi_name,
        "version": item.modversion,
        "enabled": item.enabled,
        "inserted": item.inserted,
        "attributes": attributes,
    });
    if let (Some(out), Some(status)) = (out.as_object_mut(), status.as_object()) {
        out.extend(status.clone());
    }
    out
}

/// Reads a sidecar's text and imports it (see [`import`]), the text kept
/// whole under `darktable.sidecar` so the edit can be written back or
/// read again by a later Helios.
pub fn import_text(xml: &str, layouts: &Layouts) -> Result<(Sidecar, Imported), String> {
    let sidecar = Sidecar::parse(xml)?;
    let mut imported = import(&sidecar, layouts);
    imported.darktable["sidecar"] = Value::String(xml.to_string());
    Ok((sidecar, imported))
}

/// Reads a sidecar's effective history into Helios modules, in the order
/// darktable runs them. Items Helios cannot apply are reported and kept
/// with their raw params, in their place: the import never stops at one.
pub fn import(sidecar: &Sidecar, layouts: &Layouts) -> Imported {
    let mut out = Imported::default();
    let index = |item: &HistoryItem| {
        sidecar
            .history
            .iter()
            .position(|i| std::ptr::eq(i, item))
            .expect("an item of the history")
    };
    let end = sidecar.history_end.unwrap_or(u32::MAX) as usize;
    // What became of each item, in history order.
    let mut status: Vec<Value> = (0..sidecar.history.len())
        .map(|i| json!({ "status": if i < end { "replaced" } else { "undone" } }))
        .collect();
    let arranged = sidecar.in_pipe_order();
    out.report.pipe_order = arranged.order.source;
    out.report.unordered = arranged.unordered;
    out.report.items = arranged.items.len();
    out.report.inserted = sidecar
        .history
        .iter()
        .filter(|i| i.inserted)
        .map(HistoryItem::label)
        .collect();
    for item in arranged.items {
        let at = index(item);
        if !item.enabled {
            out.report.disabled.push(item.operation.clone());
            status[at] = json!({ "status": "disabled" });
            continue;
        }
        match apply(item, layouts, &mut out.report) {
            Ok(mapped) => {
                let name = node_name(item, &out.nodes);
                status[at] = json!({ "status": "applied", "node": name });
                out.nodes.push(Node {
                    name,
                    operation: item.operation.clone(),
                    multi_name: item.multi_name.clone(),
                    instance: item.multi_priority,
                    module: mapped.module.into(),
                    params: mapped.params,
                });
            }
            Err(reason) => {
                status[at] = json!({ "status": "not_applied", "reason": reason });
                out.report.not_applied.push(NotApplied {
                    operation: item.operation.clone(),
                    instance: item.multi_priority,
                    multi_name: item.multi_name.clone(),
                    version: item.modversion,
                    label: item.label(),
                    reason,
                });
            }
        }
    }
    let items: Vec<Value> = sidecar
        .history
        .iter()
        .zip(&status)
        .map(|(item, status)| item_json(item, status))
        .collect();
    out.darktable = json!({
        "xmp_version": sidecar.xmp_version,
        "history_end": sidecar.history_end,
        "iop_order_version": sidecar.iop_order_version,
        "iop_order_list": sidecar.iop_order_list,
        "pipe_order": out.report.pipe_order,
        "history": items,
    });
    out
}
