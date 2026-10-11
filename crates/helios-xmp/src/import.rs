//! From a sidecar's history to Helios modules and parameters, with a report
//! of everything not carried over.

use helios_core::{Params, Value as Param};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::layout::{self, Layouts};
use crate::legacy;
use crate::sidecar::Sidecar;

/// A Helios module instance an edit asks for.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Node {
    /// darktable's operation and instance, for naming the node.
    pub operation: String,
    pub multi_name: String,
    /// The Helios module.
    pub module: String,
    pub params: Params,
}

/// What was not carried over, and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    /// Operations with no Helios module yet.
    pub unknown_modules: Vec<String>,
    /// Items whose params version has no layout here: `(operation, version,
    /// versions known)`. Migrating them is a step of its own.
    pub unsupported_versions: Vec<(String, u32, Vec<u32>)>,
    /// Older versions of modules taken for their presence only: their
    /// params were not read. `(operation, version)`.
    pub params_not_read: Vec<(String, u32)>,
    /// Items migrated from an older version: `(operation, from, to)`.
    pub migrated: Vec<(String, u32, u32)>,
    /// Items whose params could not be decoded.
    pub undecodable: Vec<(String, String)>,
    /// Values without a Helios counterpart: `(operation, field, value)`.
    pub dropped_fields: Vec<(String, String, Value)>,
    /// Disabled items, kept out of the network.
    pub disabled: Vec<String>,
}

/// An edit read from a sidecar.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Imported {
    /// In history order.
    pub nodes: Vec<Node>,
    pub report: Report,
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

/// Reads a sidecar's effective history into Helios modules.
pub fn import(sidecar: &Sidecar, layouts: &Layouts) -> Imported {
    let mut out = Imported::default();
    for item in sidecar.effective() {
        if !item.enabled {
            out.report.disabled.push(item.operation.clone());
            continue;
        }
        let known = layouts.versions(&item.operation);
        let Some(&current) = known.iter().max() else {
            out.report.unknown_modules.push(item.operation.clone());
            continue;
        };
        let layout = layouts
            .get(&item.operation, current)
            .expect("a known version");
        let params = if item.modversion == current {
            item.params.clone()
        } else if legacy::presence_only(&item.operation)
            && map(&item.operation, &Map::new(), &mut Report::default()).is_some()
        {
            out.report
                .params_not_read
                .push((item.operation.clone(), item.modversion));
            // An empty decode: the mapping reads nothing from these modules.
            let mapped = map(&item.operation, &Map::new(), &mut out.report).expect("mapped");
            out.nodes.push(Node {
                operation: item.operation.clone(),
                multi_name: item.multi_name.clone(),
                module: mapped.module.into(),
                params: mapped.params,
            });
            continue;
        } else {
            match legacy::upgrade(&item.operation, item.modversion, &item.params, current) {
                Some(Ok(bytes)) => {
                    out.report
                        .migrated
                        .push((item.operation.clone(), item.modversion, current));
                    bytes
                }
                Some(Err(e)) => {
                    out.report.undecodable.push((item.operation.clone(), e));
                    continue;
                }
                None => {
                    out.report.unsupported_versions.push((
                        item.operation.clone(),
                        item.modversion,
                        known,
                    ));
                    continue;
                }
            }
        };
        let decoded = match layout::decode(layout, &params) {
            Ok(Value::Object(fields)) => fields,
            Ok(_) => continue,
            Err(e) => {
                out.report.undecodable.push((item.operation.clone(), e));
                continue;
            }
        };
        let Some(mapped) = map(&item.operation, &decoded, &mut out.report) else {
            out.report.unknown_modules.push(item.operation.clone());
            continue;
        };
        for (name, value) in &decoded {
            let reported = out
                .report
                .dropped_fields
                .iter()
                .any(|(o, f, _)| o == &item.operation && f == name);
            if !mapped.used.contains(&name.as_str()) && !reported && !is_neutral(value) {
                out.report.dropped_fields.push((
                    item.operation.clone(),
                    name.clone(),
                    value.clone(),
                ));
            }
        }
        out.nodes.push(Node {
            operation: item.operation.clone(),
            multi_name: item.multi_name.clone(),
            module: mapped.module.into(),
            params: mapped.params,
        });
    }
    out
}
