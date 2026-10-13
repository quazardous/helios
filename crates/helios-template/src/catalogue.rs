//! What a template may name: modules, composites (each version), panels
//! and commands. Helios fills it with what it knows (the node its
//! registry, the desktop its modules, panels and commands); a template is
//! checked and built against it.

use std::collections::{BTreeMap, BTreeSet};

use egui_pop_model::panels::PanelSpec;
use helios_core::{Macro, Manifest, ModuleKind, ModuleRef, Network};
use serde_json::{Value as Json, json};

use crate::format::{CompositeFile, CompositeRef, InnerDesc, PORTS};

/// How deep composites may nest in one another.
pub const MAX_DEPTH: usize = 8;

/// Modules by name, composite files by id and version, and, when given,
/// the panels and commands a template may name.
#[derive(Clone, Default)]
pub struct Catalogue {
    modules: BTreeMap<String, ModuleRef>,
    composites: BTreeMap<String, BTreeMap<u32, CompositeFile>>,
    panels: Vec<PanelSpec>,
    /// `None`: commands are not checked (a node without a desktop).
    commands: Option<BTreeSet<String>>,
}

impl Catalogue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a module under its manifest's name.
    pub fn module(mut self, module: ModuleRef) -> Self {
        self.modules.insert(module.manifest().name.clone(), module);
        self
    }

    /// Adds modules under the names given (a node's registry).
    pub fn modules<'a>(mut self, modules: impl IntoIterator<Item = (&'a str, ModuleRef)>) -> Self {
        for (name, m) in modules {
            self.modules.insert(name.into(), m);
        }
        self
    }

    /// Adds a composite file (another of the same id and version replaces
    /// it).
    pub fn composite(mut self, file: CompositeFile) -> Self {
        self.add_composite(file);
        self
    }

    pub fn add_composite(&mut self, file: CompositeFile) {
        self.composites
            .entry(file.id.clone())
            .or_default()
            .insert(file.version, file);
    }

    /// The panels a layout may name.
    pub fn panels(mut self, panels: Vec<PanelSpec>) -> Self {
        self.panels = panels;
        self
    }

    /// The commands tools may name.
    pub fn commands(mut self, commands: impl IntoIterator<Item = String>) -> Self {
        self.commands = Some(commands.into_iter().collect());
        self
    }

    pub fn get_module(&self, name: &str) -> Option<&ModuleRef> {
        self.modules.get(name)
    }

    /// A composite file at one version.
    pub fn get_composite(&self, r: &CompositeRef) -> Option<&CompositeFile> {
        self.composites.get(&r.id).and_then(|v| v.get(&r.version))
    }

    /// The newest version of a composite, when it is newer than `r`'s.
    pub fn newer(&self, r: &CompositeRef) -> Option<u32> {
        self.composites
            .get(&r.id)
            .and_then(|v| v.keys().next_back().copied())
            .filter(|v| *v > r.version)
    }

    /// Every composite file, by id then version.
    pub fn composite_files(&self) -> impl Iterator<Item = &CompositeFile> {
        self.composites.values().flat_map(BTreeMap::values)
    }

    pub fn panel_specs(&self) -> &[PanelSpec] {
        &self.panels
    }

    /// Whether a command may be named (always when commands are not
    /// checked).
    pub fn knows_command(&self, id: &str) -> bool {
        self.commands.as_ref().is_none_or(|c| c.contains(id))
    }

    /// What a designer (a person, an AI) may use, as data: each module's
    /// manifest, each composite file, the panels and the commands.
    pub fn describe(&self) -> Json {
        let modules: Vec<&Manifest> = self.modules.values().map(ModuleRef::manifest).collect();
        let composites: Vec<&CompositeFile> = self.composite_files().collect();
        let panels: Vec<Json> = self
            .panels
            .iter()
            .map(|p| {
                json!({
                    "id": p.id.as_str(),
                    "title": p.title,
                    "area": p.area,
                    "areas": p.areas,
                })
            })
            .collect();
        json!({
            "modules": modules,
            "composites": composites,
            "panels": panels,
            "commands": self.commands,
        })
    }

    /// The module or composite a node names, a composite built as a
    /// macro.
    pub(crate) fn resolve(
        &self,
        module: Option<&str>,
        composite: Option<&CompositeRef>,
        depth: usize,
    ) -> Result<ModuleRef, String> {
        match (module, composite) {
            (Some(name), None) => self
                .modules
                .get(name)
                .cloned()
                .ok_or_else(|| format!("unknown module `{name}`")),
            (None, Some(r)) => self.build_composite(r, depth).map(ModuleRef::composite),
            (Some(_), Some(_)) => Err("names both a module and a composite".into()),
            (None, None) => Err("names neither a module nor a composite".into()),
        }
    }

    /// A composite file built as a macro.
    pub fn build_composite(&self, r: &CompositeRef, depth: usize) -> Result<Macro, String> {
        let file = self
            .get_composite(r)
            .ok_or_else(|| format!("unknown composite `{r}`"))?;
        build(file, self, depth).map_err(|e| format!("composite `{r}`: {e}"))
    }
}

/// A composite file as a macro: its inner nodes added in order, wired,
/// its exits set and its parameters exposed.
pub fn build(file: &CompositeFile, catalogue: &Catalogue, depth: usize) -> Result<Macro, String> {
    if depth >= MAX_DEPTH {
        return Err(format!("composites nest more than {MAX_DEPTH} deep"));
    }
    let inputs = file.inputs.iter().map(crate::format::Port::spec).collect();
    let outputs = file.outputs.iter().map(crate::format::Port::spec).collect();
    let mut network = Network::with_ports(inputs, outputs);
    let mut names = BTreeMap::new();
    for (i, n) in file.nodes.iter().enumerate() {
        let at = |e: String| format!("nodes[{i}] ({}): {e}", n.name);
        let InnerDesc {
            name,
            module,
            composite,
            params,
        } = n;
        if names.contains_key(name.as_str()) {
            return Err(at("a second node of that name".into()));
        }
        let m = catalogue
            .resolve(module.as_deref(), composite.as_ref(), depth + 1)
            .map_err(at)?;
        let inner = network
            .add(name, m, params)
            .map_err(|e| at(e.to_string()))?;
        names.insert(name.as_str(), inner);
    }
    let node = |name: &str| {
        names
            .get(name)
            .copied()
            .ok_or_else(|| format!("no node `{name}`"))
    };
    for (i, c) in file.connections.iter().enumerate() {
        let at = |e: String| format!("connections[{i}]: {e}");
        let from = if c.from == PORTS {
            network.input(&c.port).map_err(|e| at(e.to_string()))?
        } else {
            network
                .output(node(&c.from).map_err(at)?, &c.port)
                .map_err(|e| at(e.to_string()))?
        };
        let to = node(&c.to).map_err(at)?;
        network
            .connect(from, to, &c.input)
            .map_err(|e| at(e.to_string()))?;
    }
    for (i, x) in file.exits.iter().enumerate() {
        let at = |e: String| format!("exits[{i}]: {e}");
        let from = if x.from == PORTS {
            network.input(&x.port).map_err(|e| at(e.to_string()))?
        } else {
            network
                .output(node(&x.from).map_err(at)?, &x.port)
                .map_err(|e| at(e.to_string()))?
        };
        network
            .exit(from, &x.output)
            .map_err(|e| at(e.to_string()))?;
    }
    let manifest = Manifest {
        name: file.id.clone(),
        version: file.version,
        description: file.description.clone(),
        params: Vec::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        context: Vec::new(),
        needs: helios_core::Needs::default(),
        kind: ModuleKind::Process,
        family: helios_core::Family::Other,
        role: helios_core::Role::General,
    };
    let mut built = Macro::new(manifest, network).map_err(|e| e.to_string())?;
    for (i, x) in file.exposed.iter().enumerate() {
        let inner = node(&x.node).map_err(|e| format!("exposed[{i}]: {e}"))?;
        built = built
            .expose(&x.name, inner, &x.param)
            .map_err(|e| format!("exposed[{i}]: {e}"))?;
    }
    Ok(built)
}
