//! A node that stands for a network of other nodes.
//!
//! Two things are networks behind one node: a **macro**, a sub-network a
//! user saved and packaged with a few chosen parameters ([`Macro`]), and a
//! **multi-kernel module**, one that declares in code the small network of
//! kernels it is made of (a guided filter: a few per-pixel kernels and
//! blurs at several resolutions). Both are a [`Composite`]: given its
//! parameters, it expands into a [`Network`] of inner nodes, each an
//! ordinary module (or a composite again), wired to the composite's inputs
//! and to each other.
//!
//! The engine expands a composite when it plans a render, never before:
//! inner nodes are planned like any node, each with its own content key,
//! its own region (propagated through the inner wiring from what the
//! composite's consumers read), its own GPU variant, cache entries and
//! tiles, and a node read by several inner consumers is computed once over
//! the union of their reads. An inner node no output depends on is never
//! planned. An inner step's content key is that of the same module with
//! the same parameters and inputs anywhere: a macro and the flat network it
//! packages share their content.

use std::sync::Arc;

use crate::brick::{BrickSpec, Contract};
use crate::error::Error;
use crate::image::Region;
use crate::manifest::{Manifest, ModuleKind, ParamHint, ParamSpec, PortSpec};
use crate::module::{Context, ModuleRef};
use crate::params::{Params, Value};

/// An inner node of a [`Network`], by position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Inner(pub usize);

/// Where an inner node's input, or a composite's output, comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Wire {
    /// The composite's own input, by index.
    Input(usize),
    /// An output port (by index) of an inner node.
    Node(Inner, usize),
}

/// An inner node: a module, its parameters and where its inputs come from.
#[derive(Clone)]
pub struct InnerNode {
    pub name: String,
    pub module: ModuleRef,
    /// Checked and completed with defaults.
    pub params: Params,
    /// For each input port of the module; `None` when unconnected.
    pub inputs: Vec<Option<Wire>>,
}

/// A small network behind one node: inner nodes, their wiring, and which
/// inner output each output of the composite is.
///
/// An inner node reads only the composite's inputs and nodes added before
/// it, so a network has no cycle by construction.
#[derive(Clone)]
pub struct Network {
    inputs: Vec<PortSpec>,
    outputs: Vec<PortSpec>,
    nodes: Vec<InnerNode>,
    exits: Vec<Option<Wire>>,
}

impl Network {
    /// An empty network with the ports of `manifest` (the composite's).
    pub fn new(manifest: &Manifest) -> Self {
        Self::with_ports(manifest.inputs.clone(), manifest.outputs.clone())
    }

    /// An empty network with these ports.
    pub fn with_ports(inputs: Vec<PortSpec>, outputs: Vec<PortSpec>) -> Self {
        let exits = vec![None; outputs.len()];
        Self {
            inputs,
            outputs,
            nodes: Vec::new(),
            exits,
        }
    }

    pub fn inputs(&self) -> &[PortSpec] {
        &self.inputs
    }

    pub fn outputs(&self) -> &[PortSpec] {
        &self.outputs
    }

    pub fn nodes(&self) -> &[InnerNode] {
        &self.nodes
    }

    /// The inner output each output of the composite is.
    pub fn exits(&self) -> &[Option<Wire>] {
        &self.exits
    }

    pub fn node(&self, inner: Inner) -> Result<&InnerNode, Error> {
        self.nodes.get(inner.0).ok_or(Error::Node(inner.0))
    }

    /// Adds an inner node; `params` are checked and completed with
    /// defaults. A decision node cannot be inside a network: its answer is
    /// recorded on a node of the graph.
    pub fn add(&mut self, name: &str, module: ModuleRef, params: &Params) -> Result<Inner, Error> {
        let manifest = module.manifest();
        if manifest.kind == ModuleKind::Decision {
            return Err(Error::Contract {
                module: manifest.name.clone(),
                reason: "a decision node cannot be inside a network".into(),
            });
        }
        let params = manifest.resolve(params)?;
        let inputs = vec![None; manifest.inputs.len()];
        self.nodes.push(InnerNode {
            name: name.into(),
            module,
            params,
            inputs,
        });
        Ok(Inner(self.nodes.len() - 1))
    }

    /// The composite's input `port`, to wire inner nodes to.
    pub fn input(&self, port: &str) -> Result<Wire, Error> {
        self.inputs
            .iter()
            .position(|p| p.name == port)
            .map(Wire::Input)
            .ok_or_else(|| Error::Port {
                module: "network".into(),
                port: port.into(),
            })
    }

    /// Output `port` of an inner node, to wire other nodes or an exit to.
    pub fn output(&self, node: Inner, port: &str) -> Result<Wire, Error> {
        let index = self.node(node)?.module.manifest().output_index(port)?;
        Ok(Wire::Node(node, index))
    }

    /// The channels of what a wire carries.
    fn channels(&self, wire: Wire) -> Result<u32, Error> {
        let spec = match wire {
            Wire::Input(i) => self.inputs.get(i).ok_or(Error::Node(i))?,
            Wire::Node(n, port) => {
                let m = self.node(n)?.module.manifest();
                m.outputs.get(port).ok_or_else(|| Error::Port {
                    module: m.name.clone(),
                    port: port.to_string(),
                })?
            }
        };
        spec.channels().ok_or_else(|| Error::Contract {
            module: "network".into(),
            reason: format!("port `{}` is not an image the engine runs yet", spec.name),
        })
    }

    /// Wires `from` to input `port` of inner node `to`. `from` is the
    /// composite's input or an output of a node added before `to`.
    pub fn connect(&mut self, from: Wire, to: Inner, port: &str) -> Result<(), Error> {
        let channels = self.channels(from)?;
        if let Wire::Node(n, _) = from
            && n >= to
        {
            return Err(Error::Cycle {
                from: self.node(n)?.name.clone(),
                to: self.node(to)?.name.clone(),
            });
        }
        let node = self.node(to)?;
        let manifest = node.module.manifest();
        let index = manifest.input_index(port)?;
        let expected = manifest.inputs[index]
            .channels()
            .ok_or_else(|| Error::Contract {
                module: manifest.name.clone(),
                reason: format!("port `{port}` is not an image the engine runs yet"),
            })?;
        if channels != expected {
            return Err(Error::Contract {
                module: manifest.name.clone(),
                reason: format!("input `{port}` takes {expected} channels, it is given {channels}"),
            });
        }
        self.nodes[to.0].inputs[index] = Some(from);
        Ok(())
    }

    /// Makes `from` the composite's output `port`.
    pub fn exit(&mut self, from: Wire, port: &str) -> Result<(), Error> {
        let channels = self.channels(from)?;
        let index = self
            .outputs
            .iter()
            .position(|p| p.name == port)
            .ok_or_else(|| Error::Port {
                module: "network".into(),
                port: port.into(),
            })?;
        if self.outputs[index].channels() != Some(channels) {
            return Err(Error::Contract {
                module: "network".into(),
                reason: format!(
                    "output `{port}` gives {:?} channels, it is given {channels}",
                    self.outputs[index].channels()
                ),
            });
        }
        self.exits[index] = Some(from);
        Ok(())
    }

    /// Adds a node reading `from` on its first input: the usual chain.
    pub fn chain(
        &mut self,
        name: &str,
        module: ModuleRef,
        params: &Params,
        from: Wire,
    ) -> Result<Inner, Error> {
        let port = module
            .manifest()
            .inputs
            .first()
            .map(|p| p.name.clone())
            .ok_or_else(|| Error::Contract {
                module: module.manifest().name.clone(),
                reason: "no input to chain to".into(),
            })?;
        let node = self.add(name, module, params)?;
        self.connect(from, node, &port)?;
        Ok(node)
    }

    /// Every output has an exit, every required inner input is wired.
    pub(crate) fn check(&self, composite: &str) -> Result<(), Error> {
        for (port, exit) in self.outputs.iter().zip(&self.exits) {
            if exit.is_none() {
                return Err(Error::Contract {
                    module: composite.into(),
                    reason: format!("network output `{}` has no exit", port.name),
                });
            }
        }
        for node in &self.nodes {
            for (spec, wire) in node.module.manifest().inputs.iter().zip(&node.inputs) {
                if wire.is_none() && !spec.optional {
                    return Err(Error::Unconnected {
                        node: format!("{composite}/{}", node.name),
                        port: spec.name.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// The region of each of the composite's inputs its outputs read to
    /// cover `output` (`None` for an input nothing reads), walking the
    /// inner wiring back from the exits: each inner node is read over the
    /// bounds of what its consumers ask of it.
    pub fn reads(
        &self,
        context: &Context,
        at: &Expansion,
        output: Region,
    ) -> Result<Vec<Option<Region>>, Error> {
        let mut asked: Vec<Option<Region>> = vec![None; self.nodes.len()];
        let mut inputs: Vec<Option<Region>> = vec![None; self.inputs.len()];
        let ask = |wire: Wire,
                   r: Region,
                   asked: &mut Vec<Option<Region>>,
                   inputs: &mut Vec<Option<Region>>| {
            let slot = match wire {
                Wire::Input(i) => &mut inputs[i],
                Wire::Node(n, _) => &mut asked[n.0],
            };
            *slot = Some(slot.map_or(r, |s| s.bounds(&r)));
        };
        for wire in self.exits.iter().flatten() {
            ask(*wire, output, &mut asked, &mut inputs);
        }
        for i in (0..self.nodes.len()).rev() {
            let Some(r) = asked[i] else { continue };
            let node = &self.nodes[i];
            let need = node.module.reads_in(&node.params, context, at, r)?;
            for wire in node.inputs.iter().flatten() {
                ask(*wire, need, &mut asked, &mut inputs);
            }
        }
        Ok(inputs)
    }
}

/// What a composite is expanded for: the render's scale and the extent of
/// the image it works on (its first input's source, at full resolution;
/// `None` when no source feeds it). A module whose sizes are fractions of
/// the image (vkdt's radii) turns them into pixels with both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Expansion {
    pub scale: f64,
    pub extent: Option<Region>,
}

impl Expansion {
    pub fn new(scale: f64, extent: Option<Region>) -> Self {
        Self { scale, extent }
    }

    /// The image's width in pixels at the render's scale.
    pub fn width(&self) -> Option<f64> {
        self.extent.map(|e| f64::from(e.width) * self.scale)
    }

    /// The image's height in pixels at the render's scale.
    pub fn height(&self) -> Option<f64> {
        self.extent.map(|e| f64::from(e.height) * self.scale)
    }
}

/// A node that stands for a network: it declares its surface like a
/// module (its manifest) and, given its parameters, the network it is.
///
/// Pure like a module: the same parameters, context and expansion give the
/// same network. The inner nodes compute; the composite never does.
pub trait Composite: Send + Sync + 'static {
    fn manifest(&self) -> Manifest;

    /// How its parameters are best shown (see [`crate::Module::param_hints`]).
    fn param_hints(&self) -> Vec<ParamHint> {
        Vec::new()
    }

    /// The network it is with these parameters (checked against the
    /// manifest and completed with defaults) and the declared context
    /// fields, for a render at `at`.
    fn expand(&self, params: &Params, context: &Context, at: &Expansion)
    -> Result<Network, String>;

    /// What it declares as a brick (see [`crate::Module::brick`]).
    fn brick(&self) -> Option<BrickSpec> {
        None
    }

    /// The ways it is used (see [`crate::Module::provides`]).
    fn provides(&self) -> Vec<Contract> {
        let mut provides = vec![Contract::Node, Contract::Process];
        if self.brick().is_some() {
            provides.insert(0, Contract::Brick);
        }
        provides
    }
}

/// One parameter of a macro, written into inner nodes' parameters.
#[derive(Clone, Debug, PartialEq)]
struct Exposed {
    spec: ParamSpec,
    /// How it is shown: the first inner parameter's hint, renamed, with
    /// the macro's own label, unit, soft range and group.
    hint: ParamHint,
    /// The inner nodes and parameters it sets.
    targets: Vec<(Inner, String)>,
}

/// How a macro shows one exposed parameter, beyond the inner parameter it
/// sets. Every field is optional: what is not given is the inner
/// parameter's (its description, its range, its hint's unit and soft
/// range, its saved value as default).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Exposure {
    /// A short label; the exposed name otherwise.
    pub label: Option<String>,
    pub description: Option<String>,
    /// The unit shown (`EV`, `%`, `px`).
    pub unit: Option<String>,
    /// The section it is shown under.
    pub group: Option<String>,
    /// A narrower hard range than the inner one, for a number or a list of
    /// numbers: never wider, a whole number for an integer.
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// The range a slider spans by default, within the hard range.
    pub soft: Option<(f64, f64)>,
    /// The options a choice keeps, in the order shown: a subset of the
    /// inner ones.
    pub options: Option<Vec<String>>,
    /// The default, within the narrowed range; the inner node's saved
    /// value otherwise (clamped into the narrowed range).
    pub default: Option<Value>,
}

/// A sub-network a user saved, packaged as one node exposing a few chosen
/// parameters of its inner nodes (AVS's macro, Helios's workshop). Every
/// other parameter keeps the value it was saved with.
#[derive(Clone)]
pub struct Macro {
    manifest: Manifest,
    network: Arc<Network>,
    exposed: Vec<Exposed>,
}

impl Macro {
    /// A macro over `network`, its ports the network's; `manifest` gives
    /// its identity (name, version, description, header). Its ports and
    /// parameters are replaced by the network's ports and the parameters
    /// exposed with [`Macro::expose`].
    pub fn new(mut manifest: Manifest, network: Network) -> Result<Self, Error> {
        network.check(&manifest.name)?;
        manifest.inputs.clone_from(&network.inputs);
        manifest.outputs.clone_from(&network.outputs);
        manifest.params = Vec::new();
        // What the inner nodes read of the context, the macro passes on.
        let mut context: Vec<String> = Vec::new();
        for node in &network.nodes {
            for field in &node.module.manifest().context {
                if !context.contains(field) {
                    context.push(field.clone());
                }
            }
        }
        manifest.context = context;
        // One inner node needing full resolution makes the whole macro
        // computed at full resolution, then reduced.
        manifest.needs.full_resolution = network
            .nodes
            .iter()
            .any(|n| n.module.manifest().needs.full_resolution);
        Ok(Self {
            manifest,
            network: Arc::new(network),
            exposed: Vec::new(),
        })
    }

    /// Exposes parameter `param` of inner node `node` as the macro's
    /// parameter `name`, with the inner node's saved value as default.
    /// Exposing one name on several inner nodes links them: one value sets
    /// them all (their kinds must agree, and the range is what all of them
    /// accept).
    pub fn expose(self, name: &str, node: Inner, param: &str) -> Result<Self, Error> {
        self.expose_as(name, node, param, &Exposure::default())
    }

    /// [`Macro::expose`], shown as `how` says: a label, a unit, a group, a
    /// narrower range, a soft range, the options kept, a default. A linked
    /// parameter takes its interface from its first entry: a later entry
    /// with one is refused.
    pub fn expose_as(
        mut self,
        name: &str,
        node: Inner,
        param: &str,
        how: &Exposure,
    ) -> Result<Self, Error> {
        let refused = |reason: String| Error::Param {
            module: self.manifest.name.clone(),
            param: name.into(),
            reason,
        };
        let inner = self.network.node(node)?;
        let m = inner.module.manifest();
        let mut spec = m.param(param).cloned().ok_or_else(|| Error::Param {
            module: m.name.clone(),
            param: param.into(),
            reason: "unknown parameter".into(),
        })?;
        spec.name = name.into();
        if let Some(value) = inner.params.get(param) {
            spec.kind = with_default(&spec.kind, value);
        }
        if let Some(i) = self.exposed.iter().position(|e| e.spec.name == name) {
            if *how != Exposure::default() {
                return Err(refused(
                    "a linked parameter's interface is given on its first entry".into(),
                ));
            }
            let e = &mut self.exposed[i];
            if std::mem::discriminant(&e.spec.kind) != std::mem::discriminant(&spec.kind) {
                return Err(refused("linked to parameters of another kind".into()));
            }
            e.spec.kind = linked(&e.spec.kind, &spec.kind).map_err(refused)?;
            e.hint.soft = within(e.hint.soft, &e.spec.kind);
            e.targets.push((node, param.into()));
            let kind = e.spec.kind.clone();
            if let Some(p) = self.manifest.params.iter_mut().find(|p| p.name == name) {
                p.kind = kind;
            }
            return Ok(self);
        }
        let mut hint = inner
            .module
            .param_hints()
            .into_iter()
            .find(|h| h.name == param)
            .unwrap_or_default();
        if name != param {
            // The inner label names the inner parameter, not this one.
            hint.label = None;
        }
        hint.name = name.into();
        narrow(&mut spec.kind, how).map_err(refused)?;
        if let Some(d) = &how.description {
            spec.description.clone_from(d);
        }
        if let Some(v) = &how.default {
            spec.kind
                .check(v)
                .map_err(|e| refused(format!("default: {e}")))?;
            spec.kind = with_default(&spec.kind, v);
        }
        if how.label.is_some() {
            hint.label.clone_from(&how.label);
        }
        if how.unit.is_some() {
            hint.unit.clone_from(&how.unit);
        }
        if how.group.is_some() {
            hint.group.clone_from(&how.group);
        }
        hint.soft = match how.soft {
            Some((lo, hi)) => {
                let Some((min, max)) = hard(&spec.kind) else {
                    return Err(refused("a soft range is for numbers".into()));
                };
                if !(lo.is_finite() && hi.is_finite() && min <= lo && lo < hi && hi <= max) {
                    return Err(refused(format!(
                        "soft range [{lo}, {hi}] not within [{min}, {max}]"
                    )));
                }
                Some((lo, hi))
            }
            None => within(hint.soft, &spec.kind),
        };
        self.manifest.params.push(spec.clone());
        self.exposed.push(Exposed {
            spec,
            hint,
            targets: vec![(node, param.into())],
        });
        Ok(self)
    }

    /// The network it packages, as saved.
    pub fn network(&self) -> &Network {
        &self.network
    }
}

/// The hard range of a number or a list of numbers.
#[allow(clippy::cast_precision_loss)]
fn hard(kind: &crate::manifest::ParamKind) -> Option<(f64, f64)> {
    use crate::manifest::ParamKind as K;
    match kind {
        K::Float { min, max, .. } | K::Floats { min, max, .. } => Some((*min, *max)),
        K::Int { min, max, .. } | K::Ints { min, max, .. } => Some((*min as f64, *max as f64)),
        _ => None,
    }
}

/// An inherited soft range kept within the hard one; dropped when nothing
/// of it is left.
fn within(soft: Option<(f64, f64)>, kind: &crate::manifest::ParamKind) -> Option<(f64, f64)> {
    let (lo, hi) = soft?;
    let Some((min, max)) = hard(kind) else {
        return Some((lo, hi));
    };
    let (lo, hi) = (lo.max(min), hi.min(max));
    (lo < hi).then_some((lo, hi))
}

/// `kind` narrowed as `how` says (its range, its options), its default
/// clamped into what is left. A range wider than the inner one, or given
/// to a kind it does not fit, is refused.
fn narrow(kind: &mut crate::manifest::ParamKind, how: &Exposure) -> Result<(), String> {
    use crate::manifest::ParamKind as K;
    if how.min.is_some() || how.max.is_some() {
        let Some((min, max)) = hard(kind) else {
            return Err("a range is for numbers".into());
        };
        let lo = how.min.unwrap_or(min);
        let hi = how.max.unwrap_or(max);
        if !(lo.is_finite() && hi.is_finite() && min <= lo && lo <= hi && hi <= max) {
            return Err(format!(
                "range [{lo}, {hi}] not within the inner [{min}, {max}]"
            ));
        }
        set_range(kind, lo, hi)?;
    }
    if let Some(kept) = &how.options {
        let K::Choice { options, default } = kind else {
            return Err("options are for a choice".into());
        };
        if kept.is_empty() {
            return Err("no option kept".into());
        }
        for (i, o) in kept.iter().enumerate() {
            if !options.contains(o) {
                return Err(format!("`{o}` is not one of {options:?}"));
            }
            if kept[..i].contains(o) {
                return Err(format!("`{o}` kept twice"));
            }
        }
        options.clone_from(kept);
        if !options.contains(default) {
            default.clone_from(&options[0]);
        }
    }
    Ok(())
}

/// Sets a number kind's hard range to `[lo, hi]` (within the old one),
/// clamping its default.
fn set_range(kind: &mut crate::manifest::ParamKind, lo: f64, hi: f64) -> Result<(), String> {
    use crate::manifest::ParamKind as K;
    #[allow(clippy::cast_possible_truncation)]
    let whole = |x: f64| {
        if x.fract() == 0.0 {
            Ok(x as i64)
        } else {
            Err(format!("{x} is not a whole number"))
        }
    };
    match kind {
        K::Float { min, max, default } => {
            (*min, *max) = (lo, hi);
            *default = default.clamp(lo, hi);
        }
        K::Floats {
            min, max, default, ..
        } => {
            (*min, *max) = (lo, hi);
            for d in default {
                *d = d.clamp(lo, hi);
            }
        }
        K::Int { min, max, default } => {
            (*min, *max) = (whole(lo)?, whole(hi)?);
            *default = (*default).clamp(*min, *max);
        }
        K::Ints {
            min, max, default, ..
        } => {
            (*min, *max) = (whole(lo)?, whole(hi)?);
            for d in default {
                *d = (*d).clamp(*min, *max);
            }
        }
        _ => return Err("a range is for numbers".into()),
    }
    Ok(())
}

/// What a linked parameter accepts: the range both kinds accept, the
/// options both keep (in the first one's order), the first one's default
/// clamped into that. Refused when nothing is left.
fn linked(
    first: &crate::manifest::ParamKind,
    other: &crate::manifest::ParamKind,
) -> Result<crate::manifest::ParamKind, String> {
    use crate::manifest::ParamKind as K;
    let mut kind = first.clone();
    if let (
        K::Choice { options, default },
        K::Choice {
            options: theirs, ..
        },
    ) = (&mut kind, other)
    {
        options.retain(|o| theirs.contains(o));
        if options.is_empty() {
            return Err("linked to a choice with no option in common".into());
        }
        if !options.contains(default) {
            default.clone_from(&options[0]);
        }
        return Ok(kind);
    }
    if let (Some((a, b)), Some((c, d))) = (hard(first), hard(other)) {
        let (lo, hi) = (a.max(c), b.min(d));
        if lo > hi {
            return Err(format!(
                "linked to a parameter whose range [{c}, {d}] misses [{a}, {b}]"
            ));
        }
        set_range(&mut kind, lo, hi)?;
    }
    Ok(kind)
}

/// `kind` with `value` as its default, when they agree.
fn with_default(kind: &crate::manifest::ParamKind, value: &Value) -> crate::manifest::ParamKind {
    use crate::manifest::ParamKind as K;
    let mut kind = kind.clone();
    match (&mut kind, value) {
        (K::Float { default, .. }, Value::Float(v)) => *default = *v,
        (K::Int { default, .. }, Value::Int(v)) => *default = *v,
        (K::Bool { default }, Value::Bool(v)) => *default = *v,
        (K::Choice { default, .. }, Value::Choice(v))
        | (K::Text { default, .. }, Value::Text(v)) => {
            default.clone_from(v);
        }
        (K::Floats { default, .. }, Value::List(v)) => default.clone_from(v),
        (K::Ints { default, .. }, Value::Ints(v)) => default.clone_from(v),
        _ => {}
    }
    kind
}

impl Composite for Macro {
    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        self.exposed.iter().map(|e| e.hint.clone()).collect()
    }

    fn expand(&self, params: &Params, _: &Context, _: &Expansion) -> Result<Network, String> {
        let mut network = (*self.network).clone();
        for e in &self.exposed {
            let Some(value) = params.get(&e.spec.name) else {
                continue;
            };
            for (inner, param) in &e.targets {
                let node = &mut network.nodes[inner.0];
                let mut changed = node.params.clone();
                changed.set(param, value.clone());
                node.params = node
                    .module
                    .manifest()
                    .resolve(&changed)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(network)
    }
}
