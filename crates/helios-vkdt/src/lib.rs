//! vkdt's module files, read into Helios manifests and written back.
//!
//! A vkdt module describes itself in three text files (see
//! `docs/vkdt-study.md`):
//!
//! - `params`: `name:type:count:default...`, types `float`, `int`, `string`;
//! - `connectors`: `name:role:channels:format`, roles `read`, `write`,
//!   `source`, `sink`;
//! - `params.ui`: one widget per line, `param:widget:args...`, plus groups
//!   and separators.
//!
//! [`read`] turns them into a Helios [`Manifest`] and what the manifest has
//! no place for ([`Extras`]: the order of connectors, every widget line);
//! [`write`] gives the three files back. Slider ranges become the ranges of
//! their parameters. vkdt's files are read, never copied into Helios.

use helios_core::{
    Family, Manifest, ModuleKind, Needs, ParamKind, ParamSpec, PixelFormat, PortKind, PortSpec,
    Role,
};
use serde::Serialize;

/// One line of `params.ui`, as written.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Widget {
    /// The parameter, or `group`, `---`, `split`...
    pub param: String,
    /// The rest of the line, field by field (widget kind, then arguments).
    pub fields: Vec<String>,
}

/// What a vkdt module declares that a Helios manifest does not hold.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Extras {
    /// Connector names in file order (the manifest splits inputs and outputs).
    pub connector_order: Vec<String>,
    /// Every `params.ui` line, in order.
    pub widgets: Vec<Widget>,
    /// Comment lines, kept with the file they came from and their position.
    pub comments: Vec<(String, usize, String)>,
    /// Parameters whose line ends with a separator and no value after it.
    pub trailing_separator: Vec<String>,
}

/// The three files of a module; `None` for an absent one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Files {
    pub params: Option<String>,
    pub connectors: Option<String>,
    pub params_ui: Option<String>,
}

/// Content lines; comments go to `comments` with their position among the
/// file's non-blank lines, so they come back in place.
fn lines(file: &str, text: &str, comments: &mut Vec<(String, usize, String)>) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    for line in text.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        if t.trim_start().starts_with('#') {
            comments.push((file.into(), at, t.into()));
        } else {
            out.push(t.to_string());
        }
        at += 1;
    }
    out
}

fn channels_of(layout: &str) -> Option<u32> {
    match layout {
        "rgba" => Some(4),
        "rgb" => Some(3),
        "rg" => Some(2),
        "r" | "y" | "rggb" => Some(1),
        _ => None,
    }
}

fn format_of(text: &str) -> PixelFormat {
    match text {
        "f32" => PixelFormat::F32,
        "f16" => PixelFormat::F16,
        "ui8" => PixelFormat::Ui8,
        "ui16" => PixelFormat::Ui16,
        "ui32" => PixelFormat::Ui32,
        "*" => PixelFormat::Any,
        other => PixelFormat::Other(other.into()),
    }
}

fn format_text(f: &PixelFormat) -> String {
    match f {
        PixelFormat::F32 => "f32".into(),
        PixelFormat::F16 => "f16".into(),
        PixelFormat::Ui8 => "ui8".into(),
        PixelFormat::Ui16 => "ui16".into(),
        PixelFormat::Ui32 => "ui32".into(),
        PixelFormat::Any => "*".into(),
        PixelFormat::Other(s) => s.clone(),
    }
}

fn param(line: &str, ranges: &[(String, f64, f64)]) -> Result<ParamSpec, String> {
    let f: Vec<&str> = line.split(':').collect();
    if f.len() < 3 {
        return Err(format!("params: `{line}` is not name:type:count:defaults"));
    }
    let (name, kind, count) = (f[0], f[1], f[2]);
    let count: u32 = count
        .parse()
        .map_err(|_| format!("params: `{line}`: count `{count}`"))?;
    // A number list may end with a separator and nothing after it; for a
    // string, an empty last field is the (empty) value itself.
    let defaults = match (kind, &f[3..]) {
        ("float" | "int", [rest @ .., ""]) => rest,
        (_, all) => all,
    };
    let range = ranges
        .iter()
        .find(|(p, _, _)| p == name)
        .map(|(_, lo, hi)| (*lo, *hi));
    let floats = || -> Result<Vec<f64>, String> {
        defaults
            .iter()
            .map(|d| {
                d.parse()
                    .map_err(|_| format!("params: `{line}`: `{d}` is not a number"))
            })
            .collect()
    };
    let kind = match kind {
        "float" => {
            let d = floats()?;
            let (min, max) = range.unwrap_or((f64::MIN, f64::MAX));
            if count == 1 && d.len() == 1 {
                ParamKind::Float {
                    min: min.min(d[0]),
                    max: max.max(d[0]),
                    default: d[0],
                }
            } else {
                ParamKind::Floats {
                    count,
                    min: d.iter().copied().fold(min, f64::min),
                    max: d.iter().copied().fold(max, f64::max),
                    default: d,
                }
            }
        }
        "int" => {
            let d: Vec<i64> = defaults
                .iter()
                .map(|d| {
                    d.parse()
                        .map_err(|_| format!("params: `{line}`: `{d}` is not an int"))
                })
                .collect::<Result<_, _>>()?;
            let (min, max) = range.map_or((i64::MIN, i64::MAX), |(a, b)| {
                (a.floor() as i64, b.ceil() as i64)
            });
            if count == 1 && d.len() == 1 {
                ParamKind::Int {
                    min: min.min(d[0]),
                    max: max.max(d[0]),
                    default: d[0],
                }
            } else {
                ParamKind::Ints {
                    count,
                    min: d.iter().copied().fold(min, i64::min),
                    max: d.iter().copied().fold(max, i64::max),
                    default: d,
                }
            }
        }
        "string" => ParamKind::Text {
            capacity: count,
            default: defaults.join(":"),
        },
        other => return Err(format!("params: `{line}`: type `{other}`")),
    };
    Ok(ParamSpec {
        name: name.into(),
        description: String::new(),
        kind,
    })
}

fn connector(line: &str) -> Result<(PortSpec, bool), String> {
    let f: Vec<&str> = line.split(':').collect();
    let [name, role, channels, format] = f[..] else {
        return Err(format!(
            "connectors: `{line}` is not name:role:channels:format"
        ));
    };
    let (input, io) = match role {
        "read" => (true, false),
        "sink" => (true, true),
        "write" => (false, false),
        "source" => (false, true),
        other => return Err(format!("connectors: `{line}`: role `{other}`")),
    };
    let (kind, layout) = if let Some(port) = channels.strip_prefix('&') {
        (PortKind::SameAs { port: port.into() }, None)
    } else if channels == "ssbo" {
        (PortKind::Buffer, None)
    } else if channels == "*" {
        (PortKind::AnyImage, None)
    } else {
        match channels_of(channels) {
            Some(n) => (PortKind::Image { channels: n }, Some(channels.to_string())),
            None => (PortKind::AnyImage, Some(channels.to_string())),
        }
    };
    let mut port = PortSpec::image(name, 0);
    port.kind = kind;
    port.layout = layout;
    port.format = format_of(format);
    port.io = io;
    Ok((port, input))
}

/// A vkdt module, read.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Module {
    pub manifest: Manifest,
    pub extras: Extras,
}

/// Reads a module's files into a manifest and its extras.
pub fn read(name: &str, files: &Files) -> Result<Module, String> {
    let mut extras = Extras::default();
    let ui = files
        .params_ui
        .as_deref()
        .map(|t| lines("params.ui", t, &mut extras.comments))
        .unwrap_or_default();
    for line in &ui {
        let mut f: Vec<String> = line.split(':').map(str::to_string).collect();
        let param = f.remove(0);
        extras.widgets.push(Widget { param, fields: f });
    }
    // Slider ranges: param:slider:min:max.
    let ranges: Vec<(String, f64, f64)> = extras
        .widgets
        .iter()
        .filter(|w| w.fields.first().map(String::as_str) == Some("slider") && w.fields.len() >= 3)
        .filter_map(|w| {
            Some((
                w.param.clone(),
                w.fields[1].parse().ok()?,
                w.fields[2].parse().ok()?,
            ))
        })
        .collect();
    let params = files
        .params
        .as_deref()
        .map(|t| lines("params", t, &mut extras.comments))
        .unwrap_or_default()
        .iter()
        .inspect(|l| {
            let kind = l.split(':').nth(1).unwrap_or("");
            if l.ends_with(':') && l.split(':').count() > 4 && kind != "string" {
                extras
                    .trailing_separator
                    .push(l.split(':').next().unwrap_or("").to_string());
            }
        })
        .map(|l| param(l, &ranges))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{name}: {e}"))?;
    let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
    for line in files
        .connectors
        .as_deref()
        .map(|t| lines("connectors", t, &mut extras.comments))
        .unwrap_or_default()
    {
        let (port, input) = connector(&line).map_err(|e| format!("{name}: {e}"))?;
        extras.connector_order.push(port.name.clone());
        if input {
            inputs.push(port);
        } else {
            outputs.push(port);
        }
    }
    let sink = inputs.iter().any(|p| p.io) && outputs.is_empty();
    let manifest = Manifest {
        name: name.into(),
        version: 1,
        description: String::new(),
        params,
        inputs,
        outputs,
        context: vec![],
        needs: Needs::default(),
        kind: if sink {
            ModuleKind::Sink
        } else {
            ModuleKind::Process
        },
        family: Family::Other,
        role: Role::General,
    };
    Ok(Module { manifest, extras })
}

fn number(v: f64) -> String {
    format!("{v}")
}

fn param_line(p: &ParamSpec) -> String {
    let (kind, count, defaults): (&str, u32, Vec<String>) = match &p.kind {
        ParamKind::Float { default, .. } => ("float", 1, vec![number(*default)]),
        ParamKind::Floats { count, default, .. } => (
            "float",
            *count,
            default.iter().map(|v| number(*v)).collect(),
        ),
        ParamKind::Int { default, .. } => ("int", 1, vec![default.to_string()]),
        ParamKind::Ints { count, default, .. } => (
            "int",
            *count,
            default
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
        ),
        ParamKind::Text { capacity, default } => ("string", *capacity, vec![default.clone()]),
        ParamKind::Bool { default } => ("int", 1, vec![i64::from(*default).to_string()]),
        ParamKind::Choice { options, default } => (
            "int",
            1,
            vec![
                options
                    .iter()
                    .position(|o| o == default)
                    .unwrap_or(0)
                    .to_string(),
            ],
        ),
    };
    let mut fields = vec![p.name.clone(), kind.into(), count.to_string()];
    fields.extend(defaults);
    fields.join(":")
}

fn connector_line(p: &PortSpec, input: bool) -> String {
    let role = match (input, p.io) {
        (true, false) => "read",
        (true, true) => "sink",
        (false, false) => "write",
        (false, true) => "source",
    };
    let channels = match &p.kind {
        PortKind::SameAs { port } => format!("&{port}"),
        PortKind::Buffer => "ssbo".into(),
        PortKind::AnyImage => p.layout.clone().unwrap_or_else(|| "*".into()),
        PortKind::Image { channels } => p.layout.clone().unwrap_or_else(|| channels.to_string()),
    };
    format!("{}:{role}:{channels}:{}", p.name, format_text(&p.format))
}

fn with_comments(file: &str, body: Vec<String>, comments: &[(String, usize, String)]) -> String {
    let mut out = body;
    for (f, at, text) in comments {
        if f == file {
            let at = (*at).min(out.len());
            out.insert(at, text.clone());
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Writes a module back to its three files.
pub fn write(module: &Module) -> Files {
    let m = &module.manifest;
    let x = &module.extras;
    let params: Vec<String> = m
        .params
        .iter()
        .map(|p| {
            let mut line = param_line(p);
            if x.trailing_separator.contains(&p.name) {
                line.push(':');
            }
            line
        })
        .collect();
    let connectors: Vec<String> = x
        .connector_order
        .iter()
        .filter_map(|name| {
            if let Some(p) = m.inputs.iter().find(|p| &p.name == name) {
                Some(connector_line(p, true))
            } else {
                m.outputs
                    .iter()
                    .find(|p| &p.name == name)
                    .map(|p| connector_line(p, false))
            }
        })
        .collect();
    let ui: Vec<String> = x
        .widgets
        .iter()
        .map(|w| {
            let mut f = vec![w.param.clone()];
            f.extend(w.fields.iter().cloned());
            f.join(":")
        })
        .collect();
    let file = |name: &str, body: Vec<String>| {
        let has_comment = x.comments.iter().any(|(f, _, _)| f == name);
        (!body.is_empty() || has_comment).then(|| with_comments(name, body, &x.comments))
    };
    Files {
        params: file("params", params),
        connectors: file("connectors", connectors),
        params_ui: file("params.ui", ui),
    }
}
