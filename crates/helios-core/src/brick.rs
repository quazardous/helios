//! Contracts a module offers, and the brick contract
//! (`docs/design/process-bricks.md`).
//!
//! One module type can be used several ways, each a **contract**: a
//! **node** of the node diagram (its card, its exposed ports), a step of a
//! batch or an export (**process**), or a **brick**, a step inside an
//! assembly (a module written as data: a small graph of bricks plus a
//! card). Every contract goes through the same [`Module`] core; a contract
//! adds only what is its own. A brick adds a stable, namespaced id with a
//! major version, its shape, its tolerance and a description an agent
//! reads ([`BrickSpec`]).
//!
//! Independently, an implementation is available in **forms** ([`Form`]):
//! Rust code on the CPU, a WGSL kernel on the GPU, an assembly. A form
//! that is missing is visible, not forbidden: a process without a CPU form
//! cannot be the reference of a parity test.
//!
//! [`Module`]: crate::Module

use serde::Serialize;

/// A way a module is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Contract {
    /// A step inside an assembly.
    Brick,
    /// An element of the node diagram, with its card of controls.
    Node,
    /// A step of a batch, an export or a job.
    Process,
}

/// What an implementation runs as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Form {
    /// Rust code on the CPU: the form that can be a reference.
    Cpu,
    /// A WGSL kernel on the GPU.
    Gpu,
    /// A graph of other modules, written as data or in code.
    Assembly,
}

/// How an output pixel depends on the input: what planning, fusion and
/// the test kit rely on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    /// An output pixel depends on the same input pixel only.
    Pointwise,
    /// A bounded support around the pixel, scaled with the render scale.
    Neighbourhood,
    /// Changes the resolution.
    Resampling,
    /// Reads its input's whole extent and outputs a value.
    Reduction,
    /// Maps the plane.
    Geometry,
    /// No image input.
    Generator,
    /// No pixels at all: a function of numbers, evaluated before any pixel.
    Value,
}

/// How far two runs of a brick may differ.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Tolerance {
    /// The same bits on any machine, any tiling.
    Bitwise,
    /// Within this relative difference across hardware (the GPU form).
    Relative { max: f32 },
}

/// The colour space an image port's values are in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Space {
    /// Linear Rec.2020, the working space.
    Rec2020Linear,
    /// The camera's own space (before the input profile).
    Camera,
    /// Any space: the brick does not depend on it.
    Any,
    /// The same space as another port of the brick.
    SameAs(String),
}

/// What a brick port carries.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PortType {
    /// Interleaved pixels.
    Image { channels: u32, space: Space },
    /// One channel, values in [0, 1] by convention.
    Mask,
    /// One number.
    Scalar,
    /// True or false.
    Bool,
}

/// One input or output of a brick, described.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BrickPort {
    pub name: String,
    #[serde(flatten)]
    pub kind: PortType,
    pub description: String,
}

impl BrickPort {
    pub fn new(name: &str, kind: PortType, description: &str) -> Self {
        Self {
            name: name.into(),
            kind,
            description: description.into(),
        }
    }
}

/// The families bricks are presented by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BrickFamily {
    Tone,
    Colour,
    Detail,
    Mask,
    Geometry,
    Analysis,
    Value,
}

/// What a brick declares beyond its manifest: its identity, shape,
/// tolerance and an accessible description.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BrickSpec {
    /// Namespaced id, without the version: `core.affine`.
    pub id: String,
    /// Any change of output beyond the tolerance is a new major; an old
    /// major stays registered as long as something names it.
    pub major: u32,
    pub family: BrickFamily,
    pub shape: Shape,
    pub tolerance: Tolerance,
    /// One line.
    pub purpose: String,
    /// What it does, the formula when there is one.
    pub description: String,
    pub use_when: String,
    pub avoid_when: String,
    pub keywords: Vec<String>,
    /// Where it comes from (a darktable helper, a vkdt kernel, Helios's own).
    pub provenance: String,
    pub inputs: Vec<BrickPort>,
    pub outputs: Vec<BrickPort>,
}

impl BrickSpec {
    /// `id@major`, the name an assembly uses.
    pub fn versioned_id(&self) -> String {
        format!("{}@{}", self.id, self.major)
    }

    /// The problems that keep it from being registered: a namespaced id
    /// (`core.`, `dt.`, `vkdt.`, `exp.`, or a plugin's reverse domain
    /// followed by `/`), a major version, a description and described
    /// ports.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let namespaced = ["core.", "dt.", "vkdt.", "exp."]
            .iter()
            .any(|ns| self.id.len() > ns.len() && self.id.starts_with(ns))
            || self
                .id
                .split_once('/')
                .is_some_and(|(domain, rest)| domain.contains('.') && !rest.is_empty());
        if !namespaced {
            problems.push(format!("`{}` is not in a brick namespace", self.id));
        }
        if self.id.contains('@') || self.id.chars().any(char::is_whitespace) {
            problems.push(format!("`{}`: an id has no version and no space", self.id));
        }
        if self.major == 0 {
            problems.push(format!("`{}`: majors start at 1", self.id));
        }
        for (field, text) in [
            ("purpose", &self.purpose),
            ("description", &self.description),
            ("use_when", &self.use_when),
            ("provenance", &self.provenance),
        ] {
            if text.trim().is_empty() {
                problems.push(format!("`{}`: no {field}", self.id));
            }
        }
        for port in self.inputs.iter().chain(&self.outputs) {
            if port.description.trim().is_empty() {
                problems.push(format!("`{}`: port `{}` undescribed", self.id, port.name));
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, major: u32) -> BrickSpec {
        BrickSpec {
            id: id.into(),
            major,
            family: BrickFamily::Value,
            shape: Shape::Value,
            tolerance: Tolerance::Bitwise,
            purpose: "p".into(),
            description: "d".into(),
            use_when: "u".into(),
            avoid_when: String::new(),
            keywords: vec![],
            provenance: "Helios".into(),
            inputs: vec![BrickPort::new("x", PortType::Scalar, "a number")],
            outputs: vec![BrickPort::new("y", PortType::Scalar, "")],
        }
    }

    #[test]
    fn an_id_is_namespaced_and_versioned() {
        assert_eq!(spec("core.neg", 1).versioned_id(), "core.neg@1");
        assert_eq!(
            spec("core.neg", 1).problems(),
            ["`core.neg`: port `y` undescribed"]
        );
        for bad in ["neg", "core.", "core.neg@1", "org.example/"] {
            assert!(!spec(bad, 1).problems().is_empty(), "{bad}");
        }
        assert!(
            spec("org.example/lut", 2)
                .problems()
                .iter()
                .all(|p| p.contains("undescribed"))
        );
        assert!(
            spec("core.neg", 0)
                .problems()
                .iter()
                .any(|p| p.contains("majors"))
        );
    }
}
