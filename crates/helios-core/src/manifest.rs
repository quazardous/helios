//! What a module declares about itself. A manifest is data: a surface, the
//! scheduler, an AI or a test generator can read it without running the module.

use serde::Serialize;

use crate::error::Error;
use crate::params::{Params, Value};

/// The type, range and default of a parameter.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ParamKind {
    Float {
        min: f64,
        max: f64,
        default: f64,
    },
    Int {
        min: i64,
        max: i64,
        default: i64,
    },
    Bool {
        default: bool,
    },
    Choice {
        options: Vec<String>,
        default: String,
    },
    /// A fixed number of floats; `default` may hold fewer values than
    /// `count`, as vkdt's params files do.
    Floats {
        count: u32,
        min: f64,
        max: f64,
        default: Vec<f64>,
    },
    /// A fixed number of whole numbers.
    Ints {
        count: u32,
        min: i64,
        max: i64,
        default: Vec<i64>,
    },
    /// Text of at most `capacity` bytes.
    Text {
        capacity: u32,
        default: String,
    },
}

impl ParamKind {
    pub fn default_value(&self) -> Value {
        match self {
            ParamKind::Float { default, .. } => Value::Float(*default),
            ParamKind::Int { default, .. } => Value::Int(*default),
            ParamKind::Bool { default } => Value::Bool(*default),
            ParamKind::Choice { default, .. } => Value::Choice(default.clone()),
            ParamKind::Floats { default, .. } => Value::List(default.clone()),
            ParamKind::Ints { default, .. } => Value::Ints(default.clone()),
            ParamKind::Text { default, .. } => Value::Text(default.clone()),
        }
    }

    /// Checks a value against the kind; `Err` says why it does not fit.
    fn check(&self, value: &Value) -> Result<(), String> {
        match (self, value) {
            (ParamKind::Float { min, max, .. }, Value::Float(v)) => {
                if !v.is_finite() {
                    Err(format!("{v} is not finite"))
                } else if v < min || v > max {
                    Err(format!("{v} outside [{min}, {max}]"))
                } else {
                    Ok(())
                }
            }
            (ParamKind::Int { min, max, .. }, Value::Int(v)) => {
                if v < min || v > max {
                    Err(format!("{v} outside [{min}, {max}]"))
                } else {
                    Ok(())
                }
            }
            (ParamKind::Bool { .. }, Value::Bool(_)) => Ok(()),
            (
                ParamKind::Floats {
                    count, min, max, ..
                },
                Value::List(v),
            ) => {
                if v.len() > *count as usize {
                    Err(format!("{} values, at most {count}", v.len()))
                } else if let Some(x) = v.iter().find(|x| !x.is_finite() || *x < min || *x > max) {
                    Err(format!("{x} outside [{min}, {max}]"))
                } else {
                    Ok(())
                }
            }
            (
                ParamKind::Ints {
                    count, min, max, ..
                },
                Value::Ints(v),
            ) => {
                if v.len() > *count as usize {
                    Err(format!("{} values, at most {count}", v.len()))
                } else if let Some(x) = v.iter().find(|x| *x < min || *x > max) {
                    Err(format!("{x} outside [{min}, {max}]"))
                } else {
                    Ok(())
                }
            }
            (ParamKind::Text { capacity, .. }, Value::Text(t)) => {
                if t.len() > *capacity as usize {
                    Err(format!("{} bytes, at most {capacity}", t.len()))
                } else {
                    Ok(())
                }
            }
            (ParamKind::Choice { options, .. }, Value::Choice(v)) => {
                if options.contains(v) {
                    Ok(())
                } else {
                    Err(format!("`{v}` is not one of {options:?}"))
                }
            }
            (kind, value) => Err(format!("expected {}, got {value:?}", kind.type_name())),
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            ParamKind::Float { .. } => "a float",
            ParamKind::Int { .. } => "an int",
            ParamKind::Bool { .. } => "a bool",
            ParamKind::Choice { .. } => "a choice",
            ParamKind::Floats { .. } => "a list of floats",
            ParamKind::Ints { .. } => "a list of ints",
            ParamKind::Text { .. } => "text",
        }
    }
}

/// How a parameter is best shown and moved, beyond what its kind fixes:
/// what an editor needs and the engine does not. Every field is optional;
/// a module gives hints only for the parameters that need them.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ParamHint {
    pub name: String,
    /// The range a slider spans by default, within the hard one (the
    /// kind's); going past it up to the hard range is asked explicitly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soft: Option<(f64, f64)>,
    /// One fine step, in the parameter's own unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// One coarse step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coarse: Option<f64>,
    /// The unit shown (`EV`, `%`, `px`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Shown value = value × scale (100 for a fraction shown in %).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_scale: Option<f64>,
    /// Decimals shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<u32>,
    /// A short label; the parameter's name otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Shown on a logarithmic bar: equal ratios take equal lengths (a
    /// radius, an ISO). Needs a soft range above zero.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub logarithmic: bool,
    /// The module offers a picker for this parameter (a value sampled
    /// from the image): a button at the slider's end asks for it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub picker: bool,
    /// Drawn by an image tool rather than its kind's widget: a curve or a
    /// colour wheel (see [`ParamWidget`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub widget: Option<ParamWidget>,
}

/// An image tool a list of floats is edited with, instead of its values
/// one by one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamWidget {
    /// A curve: points `x0, y0, x1, y1, …` in 0..1, sorted by `x`, two
    /// at least (the list's count is twice the most points).
    Curve,
    /// A colour wheel: hue in turns (0..1, 0 to the right,
    /// counter-clockwise), chroma (0 at the centre, 1 at the rim) and a
    /// luminance offset (-1..1); three values.
    Wheel,
}

impl ParamHint {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn soft(mut self, min: f64, max: f64) -> Self {
        self.soft = Some((min, max));
        self
    }

    pub fn steps(mut self, fine: f64, coarse: f64) -> Self {
        self.step = Some(fine);
        self.coarse = Some(coarse);
        self
    }

    pub fn unit(mut self, unit: &str) -> Self {
        self.unit = Some(unit.into());
        self
    }

    pub fn display(mut self, scale: f64, precision: u32) -> Self {
        self.display_scale = Some(scale);
        self.precision = Some(precision);
        self
    }

    pub fn label(mut self, label: &str) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Shown on a logarithmic bar.
    pub fn logarithmic(mut self) -> Self {
        self.logarithmic = true;
        self
    }

    /// The module offers a picker for this parameter.
    pub fn picker(mut self) -> Self {
        self.picker = true;
        self
    }

    /// Drawn by an image tool: a curve or a colour wheel.
    pub fn widget(mut self, widget: ParamWidget) -> Self {
        self.widget = Some(widget);
        self
    }
}

/// A named parameter.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParamSpec {
    pub name: String,
    pub description: String,
    #[serde(flatten)]
    pub kind: ParamKind,
}

impl ParamSpec {
    pub fn float(name: &str, description: &str, min: f64, max: f64, default: f64) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind: ParamKind::Float { min, max, default },
        }
    }
}

/// What flows through a port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PortKind {
    /// An image with a fixed number of interleaved channels.
    Image { channels: u32 },
    /// An image with any number of channels, decided when connected.
    AnyImage,
    /// An image shaped like another port of the same module (its channels
    /// and format), as vkdt's `&input`.
    SameAs { port: String },
    /// A storage buffer rather than an image (vkdt's `ssbo`).
    Buffer,
}

/// How the values of a port are stored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PixelFormat {
    /// 32-bit float, the engine's format.
    #[default]
    F32,
    F16,
    Ui8,
    Ui16,
    Ui32,
    /// Decided when connected.
    Any,
    /// A format the engine does not process (vkdt's `atom`, `bc1`, `tri`),
    /// kept by name.
    Other(String),
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// A named input or output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PortSpec {
    pub name: String,
    #[serde(flatten)]
    pub kind: PortKind,
    #[serde(skip_serializing_if = "is_default")]
    pub format: PixelFormat,
    /// What the channels mean, when named (`rgba`, `rggb` for a raw mosaic,
    /// `y`), beyond their count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// The port reads or writes outside the network (a file, a device):
    /// the input of a sink, the output of a source.
    #[serde(skip_serializing_if = "is_default")]
    pub io: bool,
    /// An input that may stay unconnected: the module then gets an empty
    /// image (no pixels, see [`Image::is_absent`]) at its place, and the
    /// render does not fail. Inputs are required by default.
    ///
    /// [`Image::is_absent`]: crate::Image::is_absent
    #[serde(skip_serializing_if = "is_default")]
    pub optional: bool,
}

impl PortSpec {
    pub fn image(name: &str, channels: u32) -> Self {
        Self {
            name: name.into(),
            kind: PortKind::Image { channels },
            format: PixelFormat::F32,
            layout: None,
            io: false,
            optional: false,
        }
    }

    /// The same port, allowed to stay unconnected (an input).
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    /// The channel count, when the port fixes it. Ports of other kinds
    /// describe foreign modules and are not connected by the engine yet.
    pub fn channels(&self) -> Option<u32> {
        match self.kind {
            PortKind::Image { channels } => Some(channels),
            _ => None,
        }
    }
}

/// What a module needs to compute.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Needs {
    /// Pixels read around each output pixel, on every side.
    pub overlap: u32,
    /// Its inputs must be at full resolution (a sensor mosaic: reducing it
    /// mixes the colours of its photosites). At a scale below 1 the engine
    /// computes the module, and what it reads, at full resolution over the
    /// pixels the render covers, then reduces its output.
    pub full_resolution: bool,
}

/// What kind of node a module makes in a network.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleKind {
    /// Turns inputs into outputs.
    #[default]
    Process,
    /// Where the algorithm alone is not enough: resolved by an AI model or a
    /// human, its answer recorded (see `docs/module-contract.md`).
    Decision,
    /// Writes outside the framework's store (an export, a sidecar, an upload).
    Sink,
}

/// The family a module belongs to, shown in its node header and used to
/// present modules by intent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    Tone,
    Color,
    Detail,
    Geometry,
    Merge,
    Mask,
    Ai,
    Io,
    #[default]
    Other,
}

/// Whether a module is the general-purpose tool for its job or a specialised one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    General,
    Specialised,
}

/// Everything a module declares about itself.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Manifest {
    pub name: String,
    pub version: u32,
    pub description: String,
    pub params: Vec<ParamSpec>,
    pub inputs: Vec<PortSpec>,
    pub outputs: Vec<PortSpec>,
    /// Context fields (metadata of the image being processed) the module reads.
    pub context: Vec<String>,
    pub needs: Needs,
    pub kind: ModuleKind,
    pub family: Family,
    pub role: Role,
}

impl Manifest {
    pub fn param(&self, name: &str) -> Option<&ParamSpec> {
        self.params.iter().find(|p| p.name == name)
    }

    pub fn input_index(&self, port: &str) -> Result<usize, Error> {
        self.inputs
            .iter()
            .position(|p| p.name == port)
            .ok_or_else(|| self.port_error(port))
    }

    pub fn output_index(&self, port: &str) -> Result<usize, Error> {
        self.outputs
            .iter()
            .position(|p| p.name == port)
            .ok_or_else(|| self.port_error(port))
    }

    fn port_error(&self, port: &str) -> Error {
        Error::Port {
            module: self.name.clone(),
            port: port.into(),
        }
    }

    /// The default value of every parameter.
    pub fn defaults(&self) -> Params {
        self.params.iter().fold(Params::new(), |p, spec| {
            p.with(&spec.name, spec.kind.default_value())
        })
    }

    /// `params` completed with defaults, after checking every value; unknown
    /// names, wrong types and out-of-range values are refused.
    pub fn resolve(&self, params: &Params) -> Result<Params, Error> {
        let mut out = self.defaults();
        for (name, value) in params.iter() {
            let spec = self
                .param(name)
                .ok_or_else(|| self.param_error(name, "unknown parameter".into()))?;
            spec.kind
                .check(value)
                .map_err(|reason| self.param_error(name, reason))?;
            out.set(name, value.clone());
        }
        Ok(out)
    }

    fn param_error(&self, param: &str, reason: String) -> Error {
        Error::Param {
            module: self.name.clone(),
            param: param.into(),
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            name: "test".into(),
            version: 1,
            description: String::new(),
            params: vec![ParamSpec::float("gain", "", 0.0, 2.0, 1.0)],
            inputs: vec![],
            outputs: vec![],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Other,
            role: Role::General,
        }
    }

    #[test]
    fn resolve_fills_defaults_and_checks_values() {
        let m = manifest();
        assert_eq!(m.resolve(&Params::new()).unwrap().float("gain"), 1.0);
        assert_eq!(
            m.resolve(&Params::new().with("gain", Value::Float(2.0)))
                .unwrap()
                .float("gain"),
            2.0
        );
        for bad in [
            Params::new().with("gain", Value::Float(3.0)),
            Params::new().with("gain", Value::Float(f64::NAN)),
            Params::new().with("gain", Value::Bool(true)),
            Params::new().with("other", Value::Float(1.0)),
        ] {
            assert!(
                matches!(m.resolve(&bad), Err(Error::Param { .. })),
                "{bad:?} accepted"
            );
        }
    }
}
