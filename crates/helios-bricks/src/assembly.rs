//! Modules assembled from bricks, written as data.
//!
//! An assembly is a module described as JSON: its own parameters (the ones
//! an edit records), a **value graph** of value bricks deriving brick
//! parameters from them and from the image's context, a small graph of
//! image bricks, and the controls its parameters are patched to
//! (`docs/design/process-bricks.md`, section 5.1):
//!
//! ```json
//! {
//!   "format": 1, "name": "levels", "version": 1, "description": "...",
//!   "module": { "family": "tone", "role": "general", "context": [] },
//!   "params": [ { "type": "float", "name": "black", "description": "...",
//!                 "min": -1, "max": 1, "default": 0 } ],
//!   "inputs": [ { "name": "input", "channels": 4 } ],
//!   "outputs": [ { "name": "output", "channels": 4 } ],
//!   "values": [ { "name": "scale", "brick": "core.value.affine-inv@1",
//!                 "in": { "white": 1, "black": "black" } } ],
//!   "nodes": [ { "name": "apply", "brick": "core.affine@1",
//!                "params": { "origin": "black", "scale": "scale" } } ],
//!   "connections": [ { "from": "@", "port": "input", "to": "apply", "input": "input" } ],
//!   "exits": [ { "from": "apply", "port": "output", "output": "output" } ],
//!   "controls": [ { "control": "black-level", "jack": "value", "param": "black" } ]
//! }
//! ```
//!
//! An operand is a number, a flag, or a name: a module parameter, a value
//! defined above it, or `context:<field>` (a declared context field). `@`
//! is the module itself, for its inputs.
//!
//! [`Assembled`] checks an assembly against a [`BrickRegistry`] and is a
//! [`Composite`]: the engine expands it at planning into its image bricks,
//! the value graph evaluated first (only the values the bricks need, so a
//! context field behind an unselected branch may be absent).

use std::collections::BTreeMap;
use std::sync::Arc;

use helios_core::{
    Composite, Context, Expansion, Family, Inner, Manifest, ModuleKind, ModuleRef, Needs, Network,
    ParamHint, ParamKind, ParamSpec, Params, PortSpec, PortType, Role, Value, Wire,
};
use serde::Deserialize;

use crate::registry::{Brick, BrickRegistry};
use crate::values::ValueBrick;

/// An assembly as written.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assembly {
    pub format: u32,
    pub name: String,
    pub version: u32,
    pub description: String,
    pub module: ModuleBlock,
    pub params: Vec<ParamDecl>,
    pub inputs: Vec<PortDecl>,
    pub outputs: Vec<PortDecl>,
    #[serde(default)]
    pub values: Vec<ValueDecl>,
    pub nodes: Vec<NodeDecl>,
    pub connections: Vec<Connection>,
    pub exits: Vec<Exit>,
    #[serde(default)]
    pub controls: Vec<ControlDecl>,
}

/// The module's header.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleBlock {
    pub family: String,
    #[serde(default = "general")]
    pub role: String,
    /// Context fields it reads.
    #[serde(default)]
    pub context: Vec<String>,
}

fn general() -> String {
    "general".into()
}

/// A module parameter.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum ParamDecl {
    Float {
        name: String,
        description: String,
        min: f64,
        max: f64,
        default: f64,
    },
    Bool {
        name: String,
        description: String,
        default: bool,
    },
}

impl ParamDecl {
    fn spec(&self) -> ParamSpec {
        match self {
            ParamDecl::Float {
                name,
                description,
                min,
                max,
                default,
            } => ParamSpec::float(name, description, *min, *max, *default),
            ParamDecl::Bool {
                name,
                description,
                default,
            } => ParamSpec {
                name: name.clone(),
                description: description.clone(),
                kind: ParamKind::Bool { default: *default },
            },
        }
    }
}

/// An image port of the module.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortDecl {
    pub name: String,
    pub channels: u32,
}

/// A value: a value brick and its inputs.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueDecl {
    pub name: String,
    pub brick: String,
    #[serde(rename = "in")]
    pub inputs: BTreeMap<String, Operand>,
}

/// An image brick and its parameters; parameters not given keep the
/// brick's defaults.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDecl {
    pub name: String,
    pub brick: String,
    #[serde(default)]
    pub params: BTreeMap<String, Operand>,
}

/// A number, a flag, or a name (see the module documentation).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Operand {
    Number(f64),
    Flag(bool),
    Name(String),
}

/// An image wire: output `port` of node `from` (or the module's input
/// `port` when `from` is `@`) into input `input` of node `to`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub from: String,
    pub port: String,
    pub to: String,
    pub input: String,
}

/// Which node output is a module output.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exit {
    pub from: String,
    pub port: String,
    pub output: String,
}

/// A module parameter patched to a control's jack.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlDecl {
    pub control: String,
    pub jack: String,
    pub param: String,
}

impl Assembly {
    pub fn from_json(text: &str) -> Result<Self, Vec<String>> {
        serde_json::from_str(text).map_err(|e| vec![format!("assembly: {e}")])
    }
}

/// Where a brick parameter or a value input comes from, resolved.
#[derive(Clone, Debug)]
enum Ref {
    Const(Value),
    Param(String),
    Context(String),
    Value(usize),
}

struct ValueNode {
    name: String,
    brick: Arc<dyn ValueBrick>,
    /// In the brick's input order.
    inputs: Vec<Ref>,
}

enum Source {
    Input(usize),
    Node(usize, String),
}

struct ImageNode {
    name: String,
    module: ModuleRef,
    params: Vec<(String, Ref)>,
    inputs: Vec<(String, Source)>,
}

/// A checked assembly, a [`Composite`] the engine expands.
pub struct Assembled {
    manifest: Manifest,
    hints: Vec<ParamHint>,
    values: Vec<ValueNode>,
    nodes: Vec<ImageNode>,
    exits: Vec<(String, usize, String)>,
    controls: Vec<ControlDecl>,
}

impl Assembled {
    /// Parses and checks an assembly.
    pub fn from_json(text: &str, registry: &BrickRegistry) -> Result<Self, Vec<String>> {
        Self::new(&Assembly::from_json(text)?, registry)
    }

    /// Checks an assembly against the registry: every brick known, every
    /// operand resolved and of the type its port takes, every brick input
    /// fed, every module parameter used, the wiring sound. Each problem is
    /// a diagnostic with its path.
    pub fn new(assembly: &Assembly, registry: &BrickRegistry) -> Result<Self, Vec<String>> {
        let mut problems = Vec::new();
        if assembly.format != 1 {
            problems.push(format!("format: {} is not 1", assembly.format));
        }
        let family = family(&assembly.module.family).unwrap_or_else(|| {
            problems.push(format!(
                "module.family: `{}` is not a family",
                assembly.module.family
            ));
            Family::Other
        });
        let role = match assembly.module.role.as_str() {
            "general" => Role::General,
            "specialised" => Role::Specialised,
            other => {
                problems.push(format!("module.role: `{other}` is not a role"));
                Role::General
            }
        };
        let params: Vec<ParamSpec> = assembly.params.iter().map(ParamDecl::spec).collect();
        let mut used = vec![false; params.len()];
        for (i, p) in params.iter().enumerate() {
            if params[..i].iter().any(|q| q.name == p.name) {
                problems.push(format!("params[{i}]: `{}` is declared twice", p.name));
            }
        }
        let context = &assembly.module.context;

        // Resolves a name into a reference, with the type it carries.
        let resolve = |path: &str,
                       operand: &Operand,
                       values: &[ValueNode],
                       used: &mut Vec<bool>,
                       problems: &mut Vec<String>|
         -> Option<(Ref, PortType)> {
            match operand {
                Operand::Number(n) => Some((Ref::Const(Value::Float(*n)), PortType::Scalar)),
                Operand::Flag(b) => Some((Ref::Const(Value::Bool(*b)), PortType::Bool)),
                Operand::Name(name) => {
                    if let Some(field) = name.strip_prefix("context:") {
                        if context.iter().any(|c| c == field) {
                            return Some((Ref::Context(field.into()), PortType::Scalar));
                        }
                        problems.push(format!(
                            "{path}: context field `{field}` is not declared in module.context"
                        ));
                        return None;
                    }
                    if let Some(i) = params.iter().position(|p| &p.name == name) {
                        used[i] = true;
                        let kind = match params[i].kind {
                            ParamKind::Bool { .. } => PortType::Bool,
                            _ => PortType::Scalar,
                        };
                        return Some((Ref::Param(name.clone()), kind));
                    }
                    if let Some(i) = values.iter().position(|v| &v.name == name) {
                        let kind = values[i].brick.spec().outputs[0].kind.clone();
                        return Some((Ref::Value(i), kind));
                    }
                    problems.push(format!(
                        "{path}: `{name}` is no parameter, no value above it, no context field"
                    ));
                    None
                }
            }
        };

        let mut values: Vec<ValueNode> = Vec::new();
        for (i, decl) in assembly.values.iter().enumerate() {
            let path = format!("values[{i}] `{}`", decl.name);
            if params.iter().any(|p| p.name == decl.name)
                || values.iter().any(|v| v.name == decl.name)
            {
                problems.push(format!("{path}: the name is taken"));
            }
            let brick = match registry.get(&decl.brick) {
                Some(Brick::Value(b)) => b.clone(),
                Some(Brick::Image(_)) => {
                    problems.push(format!("{path}: `{}` is an image brick", decl.brick));
                    continue;
                }
                None => {
                    problems.push(format!("{path}: no value brick `{}`", decl.brick));
                    continue;
                }
            };
            let spec = brick.spec();
            for given in decl.inputs.keys() {
                if !spec.inputs.iter().any(|p| &p.name == given) {
                    problems.push(format!(
                        "{path}.in.{given}: `{}` has no such input",
                        spec.id
                    ));
                }
            }
            let mut inputs = Vec::new();
            for port in &spec.inputs {
                let at = format!("{path}.in.{}", port.name);
                let Some(operand) = decl.inputs.get(&port.name) else {
                    problems.push(format!("{at}: not fed"));
                    continue;
                };
                if let Some((r, kind)) = resolve(&at, operand, &values, &mut used, &mut problems) {
                    if kind != port.kind {
                        problems.push(format!(
                            "{at}: a {:?} is given, the input takes a {:?}",
                            kind, port.kind
                        ));
                    }
                    inputs.push(r);
                }
            }
            values.push(ValueNode {
                name: decl.name.clone(),
                brick,
                inputs,
            });
        }

        let mut nodes: Vec<ImageNode> = Vec::new();
        for (i, decl) in assembly.nodes.iter().enumerate() {
            let path = format!("nodes[{i}] `{}`", decl.name);
            if nodes.iter().any(|n| n.name == decl.name) || decl.name == "@" {
                problems.push(format!("{path}: the name is taken"));
            }
            let module = match registry.get(&decl.brick) {
                Some(Brick::Image(m)) => m.clone(),
                Some(Brick::Value(_)) => {
                    problems.push(format!("{path}: `{}` is a value brick", decl.brick));
                    continue;
                }
                None => {
                    problems.push(format!("{path}: no image brick `{}`", decl.brick));
                    continue;
                }
            };
            let mut node_params = Vec::new();
            for (name, operand) in &decl.params {
                let at = format!("{path}.params.{name}");
                let Some(spec) = module.manifest().param(name) else {
                    problems.push(format!("{at}: `{}` has no such parameter", decl.brick));
                    continue;
                };
                let takes = match spec.kind {
                    ParamKind::Bool { .. } => PortType::Bool,
                    ParamKind::Float { .. } => PortType::Scalar,
                    _ => {
                        problems.push(format!("{at}: only numbers and flags are fed yet"));
                        continue;
                    }
                };
                if let Some((r, kind)) = resolve(&at, operand, &values, &mut used, &mut problems) {
                    if kind != takes {
                        problems.push(format!(
                            "{at}: a {kind:?} is given, the parameter takes a {takes:?}"
                        ));
                    }
                    node_params.push((name.clone(), r));
                }
            }
            nodes.push(ImageNode {
                name: decl.name.clone(),
                module,
                params: node_params,
                inputs: Vec::new(),
            });
        }

        let inputs: Vec<PortSpec> = assembly
            .inputs
            .iter()
            .map(|p| PortSpec::image(&p.name, p.channels))
            .collect();
        let outputs: Vec<PortSpec> = assembly
            .outputs
            .iter()
            .map(|p| PortSpec::image(&p.name, p.channels))
            .collect();
        let node_index = |name: &str| nodes.iter().position(|n| n.name == name);
        // The channels of a source, or a problem.
        let source = |path: &str, from: &str, port: &str, nodes: &[ImageNode]| {
            if from == "@" {
                return inputs
                    .iter()
                    .position(|p| p.name == port)
                    .map(|i| (Source::Input(i), inputs[i].channels()))
                    .ok_or_else(|| format!("{path}: the module has no input `{port}`"));
            }
            let n = nodes
                .iter()
                .position(|n| n.name == from)
                .ok_or_else(|| format!("{path}: no node `{from}`"))?;
            let m = nodes[n].module.manifest();
            m.outputs
                .iter()
                .find(|p| p.name == port)
                .map(|p| (Source::Node(n, port.into()), p.channels()))
                .ok_or_else(|| format!("{path}: `{from}` has no output `{port}`"))
        };
        let mut wired = Vec::new();
        for (i, c) in assembly.connections.iter().enumerate() {
            let path = format!("connections[{i}]");
            let Some(to) = node_index(&c.to) else {
                problems.push(format!("{path}: no node `{}`", c.to));
                continue;
            };
            let (from, channels) = match source(&path, &c.from, &c.port, &nodes) {
                Ok(found) => found,
                Err(e) => {
                    problems.push(e);
                    continue;
                }
            };
            if let Source::Node(n, _) = &from
                && *n >= to
            {
                problems.push(format!(
                    "{path}: `{}` reads `{}`, which comes after it",
                    c.to, c.from
                ));
                continue;
            }
            let manifest = nodes[to].module.manifest();
            match manifest.inputs.iter().find(|p| p.name == c.input) {
                None => problems.push(format!("{path}: `{}` has no input `{}`", c.to, c.input)),
                Some(p) if p.channels() != channels => problems.push(format!(
                    "{path}: `{}` takes {:?} channels on `{}`, it is given {channels:?}",
                    c.to,
                    p.channels(),
                    c.input
                )),
                Some(_) => wired.push((to, c.input.clone(), from)),
            }
        }
        for (to, input, from) in wired {
            nodes[to].inputs.push((input, from));
        }
        for node in &nodes {
            for port in &node.module.manifest().inputs {
                let fed = node.inputs.iter().filter(|(i, _)| *i == port.name).count();
                if fed == 0 && !port.optional {
                    problems.push(format!(
                        "nodes `{}`: input `{}` not fed",
                        node.name, port.name
                    ));
                } else if fed > 1 {
                    problems.push(format!(
                        "nodes `{}`: input `{}` fed twice",
                        node.name, port.name
                    ));
                }
            }
        }
        let mut exits = Vec::new();
        for (i, e) in assembly.exits.iter().enumerate() {
            let path = format!("exits[{i}]");
            let Some(out) = outputs.iter().find(|p| p.name == e.output) else {
                problems.push(format!("{path}: the module has no output `{}`", e.output));
                continue;
            };
            match source(&path, &e.from, &e.port, &nodes) {
                Ok((Source::Node(n, port), channels)) if channels == out.channels() => {
                    exits.push((e.output.clone(), n, port));
                }
                Ok((Source::Node(..), channels)) => problems.push(format!(
                    "{path}: `{}` gives {:?} channels, it is given {channels:?}",
                    e.output,
                    out.channels()
                )),
                Ok((Source::Input(_), _)) => {
                    problems.push(format!("{path}: an exit comes from a node"));
                }
                Err(e) => problems.push(e),
            }
        }
        for out in &outputs {
            match exits.iter().filter(|(o, ..)| *o == out.name).count() {
                1 => {}
                0 => problems.push(format!("exits: output `{}` has no exit", out.name)),
                _ => problems.push(format!("exits: output `{}` has two", out.name)),
            }
        }
        for (i, c) in assembly.controls.iter().enumerate() {
            if !params.iter().any(|p| p.name == c.param) {
                problems.push(format!("controls[{i}]: no parameter `{}`", c.param));
            }
        }
        for (p, used) in params.iter().zip(&used) {
            if !used {
                problems.push(format!("params `{}`: never used", p.name));
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(Self {
            manifest: Manifest {
                name: assembly.name.clone(),
                version: assembly.version,
                description: assembly.description.clone(),
                params,
                inputs,
                outputs,
                context: context.clone(),
                needs: Needs::default(),
                kind: ModuleKind::Process,
                family,
                role,
            },
            hints: Vec::new(),
            values,
            nodes,
            exits,
            controls: assembly.controls.clone(),
        })
    }

    /// How its parameters are best shown (its card's soft ranges, steps
    /// and units).
    pub fn with_hints(mut self, hints: Vec<ParamHint>) -> Self {
        self.hints = hints;
        self
    }

    /// The controls its parameters are patched to.
    pub fn controls(&self) -> &[ControlDecl] {
        &self.controls
    }

    /// The value graph evaluated for these parameters and context: every
    /// value the image bricks read, by name. Values no brick reads are not
    /// evaluated.
    pub fn values(&self, params: &Params, context: &Context) -> Result<Params, String> {
        let mut memo = vec![None; self.values.len()];
        for node in &self.nodes {
            for (_, r) in &node.params {
                self.resolve(r, params, context, &mut memo)?;
            }
        }
        let mut out = Params::new();
        for (node, value) in self.values.iter().zip(memo) {
            if let Some(v) = value {
                out.set(&node.name, v);
            }
        }
        Ok(out)
    }

    fn resolve(
        &self,
        r: &Ref,
        params: &Params,
        context: &Context,
        memo: &mut Vec<Option<Value>>,
    ) -> Result<Value, String> {
        match r {
            Ref::Const(v) => Ok(v.clone()),
            Ref::Param(name) => params
                .get(name)
                .cloned()
                .ok_or_else(|| format!("parameter `{name}` missing")),
            Ref::Context(field) => context
                .get(field)
                .cloned()
                .ok_or_else(|| format!("context field `{field}` needed, absent")),
            Ref::Value(i) => self.value(*i, params, context, memo),
        }
    }

    fn value(
        &self,
        i: usize,
        params: &Params,
        context: &Context,
        memo: &mut Vec<Option<Value>>,
    ) -> Result<Value, String> {
        if let Some(v) = &memo[i] {
            return Ok(v.clone());
        }
        let node = &self.values[i];
        let v = node
            .brick
            .eval(&mut |k| {
                let r = node.inputs.get(k).ok_or("no such input")?;
                self.resolve(r, params, context, memo)
            })
            .map_err(|e| format!("value `{}`: {e}", node.name))?;
        memo[i] = Some(v.clone());
        Ok(v)
    }
}

impl Composite for Assembled {
    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        self.hints.clone()
    }

    fn expand(&self, params: &Params, context: &Context, _: &Expansion) -> Result<Network, String> {
        let mut memo = vec![None; self.values.len()];
        let mut network = Network::new(&self.manifest);
        let mut inner: Vec<Inner> = Vec::new();
        for node in &self.nodes {
            let mut node_params = Params::new();
            for (name, r) in &node.params {
                node_params.set(name, self.resolve(r, params, context, &mut memo)?);
            }
            let id = network
                .add(&node.name, node.module.clone(), &node_params)
                .map_err(|e| e.to_string())?;
            for (port, from) in &node.inputs {
                let wire = match from {
                    Source::Input(i) => Wire::Input(*i),
                    Source::Node(n, out) => {
                        network.output(inner[*n], out).map_err(|e| e.to_string())?
                    }
                };
                network.connect(wire, id, port).map_err(|e| e.to_string())?;
            }
            inner.push(id);
        }
        for (output, n, port) in &self.exits {
            let wire = network.output(inner[*n], port).map_err(|e| e.to_string())?;
            network.exit(wire, output).map_err(|e| e.to_string())?;
        }
        Ok(network)
    }
}

fn family(name: &str) -> Option<Family> {
    Some(match name {
        "tone" => Family::Tone,
        "color" | "colour" => Family::Color,
        "detail" => Family::Detail,
        "geometry" => Family::Geometry,
        "merge" => Family::Merge,
        "mask" => Family::Mask,
        "ai" => Family::Ai,
        "io" => Family::Io,
        "other" => Family::Other,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::{Bus, Engine, Graph, Image, Region};

    const LEVELS: &str = r#"{
      "format": 1, "name": "levels", "version": 1,
      "description": "Map a black level to zero",
      "module": { "family": "tone", "context": ["photo.gain"] },
      "params": [
        { "type": "float", "name": "black", "description": "the black level",
          "min": -1, "max": 0.5, "default": 0 },
        { "type": "bool", "name": "gain", "description": "apply the context's gain",
          "default": false }
      ],
      "inputs": [ { "name": "input", "channels": 4 } ],
      "outputs": [ { "name": "output", "channels": 4 } ],
      "values": [
        { "name": "inv", "brick": "core.value.affine-inv@1", "in": { "white": 1, "black": "black" } },
        { "name": "scale", "brick": "core.value.select@1",
          "in": { "when": "gain", "then": "context:photo.gain", "else": "inv" } }
      ],
      "nodes": [ { "name": "apply", "brick": "core.affine@1",
                   "params": { "origin": "black", "scale": "scale" } } ],
      "connections": [ { "from": "@", "port": "input", "to": "apply", "input": "input" } ],
      "exits": [ { "from": "apply", "port": "output", "output": "output" } ],
      "controls": [ { "control": "black-level", "jack": "value", "param": "black" } ]
    }"#;

    fn levels() -> Assembled {
        Assembled::from_json(LEVELS, &BrickRegistry::core()).unwrap()
    }

    fn render(module: ModuleRef, params: &Params, context: Context) -> Result<Image, String> {
        let input = Image::new(
            Region::sized(2, 1),
            4,
            vec![0.5, 1.0, 0.75, 0.2, 0.0, 0.25, 1.5, 1.0],
        );
        let mut g = Graph::new();
        let src = g.add_source("src", input.clone());
        let n = g
            .add_module("levels", module, params)
            .map_err(|e| e.to_string())?;
        g.set_context(n, context).unwrap();
        g.connect(src, helios_core::graph::SOURCE_PORT, n, "input")
            .unwrap();
        Engine::new(Arc::new(Bus::new()))
            .render(&g, n, "output", input.region())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn an_assembly_renders_through_its_bricks() {
        let module = ModuleRef::composite(levels());
        assert_eq!(module.forms(), [helios_core::Form::Assembly]);
        let params = Params::new().with("black", Value::Float(0.5));
        let out = render(module.clone(), &params, Context::new()).unwrap();
        assert_eq!(out.data(), &[0.0, 1.0, 0.5, 0.2, -1.0, -0.5, 2.0, 1.0]);
        // The context is read only on the branch that needs it.
        let gain = Params::new().with("gain", Value::Bool(true));
        let err = render(module.clone(), &gain, Context::new()).unwrap_err();
        assert!(err.contains("photo.gain"), "{err}");
        let out = render(
            module,
            &gain,
            Context::new().with("photo.gain", Value::Float(2.0)),
        )
        .unwrap();
        assert_eq!(out.data(), &[1.0, 2.0, 1.5, 0.2, 0.0, 0.5, 3.0, 1.0]);
    }

    #[test]
    fn the_value_graph_reads_only_what_bricks_need() {
        let assembled = levels();
        let params = assembled
            .manifest()
            .resolve(&Params::new().with("black", Value::Float(0.5)))
            .unwrap();
        let values = assembled.values(&params, &Context::new()).unwrap();
        assert_eq!(values.get("inv"), Some(&Value::Float(2.0)));
        assert_eq!(values.get("scale"), Some(&Value::Float(2.0)));
        assert_eq!(
            assembled.controls(),
            [ControlDecl {
                control: "black-level".into(),
                jack: "value".into(),
                param: "black".into(),
            }]
        );
    }

    #[test]
    fn it_passes_the_module_contract() {
        helios_testkit::assert_contract_of(&ModuleRef::composite(levels()), &Context::new());
    }

    fn problems(edit: impl FnOnce(&mut serde_json::Value)) -> Vec<String> {
        let mut json: serde_json::Value = serde_json::from_str(LEVELS).unwrap();
        edit(&mut json);
        match Assembled::from_json(&json.to_string(), &BrickRegistry::core()) {
            Ok(_) => Vec::new(),
            Err(p) => p,
        }
    }

    #[test]
    fn each_problem_is_named_with_its_path() {
        let has = |p: Vec<String>, what: &str| {
            assert!(p.iter().any(|e| e.contains(what)), "{what}: {p:?}");
        };
        has(
            problems(|j| j["values"][0]["brick"] = "core.value.nothing@1".into()),
            "values[0] `inv`: no value brick `core.value.nothing@1`",
        );
        has(
            problems(|j| j["nodes"][0]["brick"] = "core.value.neg@1".into()),
            "is a value brick",
        );
        has(
            problems(|j| j["values"][0]["in"]["black"] = "nobody".into()),
            "values[0] `inv`.in.black: `nobody` is no parameter",
        );
        has(
            problems(|j| {
                j["values"][0]["in"]
                    .as_object_mut()
                    .unwrap()
                    .remove("white");
            }),
            "values[0] `inv`.in.white: not fed",
        );
        has(
            problems(|j| j["values"][1]["in"]["when"] = "black".into()),
            "values[1] `scale`.in.when: a Scalar is given",
        );
        has(
            problems(|j| j["values"][1]["in"]["then"] = "context:photo.iso".into()),
            "is not declared in module.context",
        );
        has(
            problems(|j| j["values"][1]["in"]["else"] = "scale".into()),
            "`scale` is no parameter, no value above it",
        );
        has(
            problems(|j| j["values"][1]["in"]["when"] = false.into()),
            "params `gain`: never used",
        );
        has(
            problems(|j| j["connections"][0]["input"] = "guide".into()),
            "`apply` has no input `guide`",
        );
        has(
            problems(|j| j["exits"] = serde_json::json!([])),
            "exits: output `output` has no exit",
        );
        has(
            problems(|j| j["controls"][0]["param"] = "white".into()),
            "controls[0]: no parameter `white`",
        );
        has(
            problems(|j| j["surprise"] = 1.into()),
            "unknown field `surprise`",
        );
        assert!(problems(|_| {}).is_empty());
    }
}
