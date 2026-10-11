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
}

impl ParamKind {
    pub fn default_value(&self) -> Value {
        match self {
            ParamKind::Float { default, .. } => Value::Float(*default),
            ParamKind::Int { default, .. } => Value::Int(*default),
            ParamKind::Bool { default } => Value::Bool(*default),
            ParamKind::Choice { default, .. } => Value::Choice(default.clone()),
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
        }
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
}

/// A named input or output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PortSpec {
    pub name: String,
    #[serde(flatten)]
    pub kind: PortKind,
}

impl PortSpec {
    pub fn image(name: &str, channels: u32) -> Self {
        Self {
            name: name.into(),
            kind: PortKind::Image { channels },
        }
    }

    pub fn channels(&self) -> u32 {
        match self.kind {
            PortKind::Image { channels } => channels,
        }
    }
}

/// What a module needs to compute.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Needs {
    /// Pixels read around each output pixel, on every side.
    pub overlap: u32,
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
