//! A network described as JSON, and the modules a node knows by name.
//!
//! ```json
//! {
//!   "sources": [{ "name": "src", "synthetic": { "kind": "noise", "width": 64, "height": 48, "channels": 4, "seed": 1 } }],
//!   "modules": [{ "name": "exp", "module": "exposure", "params": { "exposure": { "float": 1.0 } } }],
//!   "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }],
//!   "renders": [{ "node": "exp", "port": "output", "region": { "x": 0, "y": 0, "width": 64, "height": 48 } }]
//! }
//! ```
//!
//! Sources are synthetic images for now: loading files is the framework's
//! job and comes with the photo domain.

pub mod mcp;

use std::collections::BTreeMap;

use helios_core::{Graph, ModuleRef, NodeId, Params, Region};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Description {
    #[serde(default)]
    pub sources: Vec<SourceDesc>,
    #[serde(default)]
    pub modules: Vec<ModuleDesc>,
    #[serde(default)]
    pub connections: Vec<ConnectionDesc>,
    #[serde(default)]
    pub renders: Vec<RenderDesc>,
}

#[derive(Debug, Deserialize)]
pub struct SourceDesc {
    pub name: String,
    pub synthetic: Synthetic,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Synthetic {
    Noise {
        width: u32,
        height: u32,
        channels: u32,
        seed: u64,
    },
    Ramp {
        width: u32,
        height: u32,
        channels: u32,
    },
}

#[derive(Debug, Deserialize)]
pub struct ModuleDesc {
    pub name: String,
    pub module: String,
    #[serde(default)]
    pub params: Params,
}

#[derive(Debug, Deserialize)]
pub struct ConnectionDesc {
    pub from: String,
    pub port: String,
    pub to: String,
    pub input: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RenderDesc {
    pub node: String,
    pub port: String,
    pub region: Region,
}

/// The modules this node can instantiate, by name.
pub fn registry() -> BTreeMap<&'static str, ModuleRef> {
    BTreeMap::from([("exposure", ModuleRef::new(helios_module_exposure::Exposure))])
}

/// A network ready to evaluate.
pub struct Loaded {
    pub graph: Graph,
    pub nodes: BTreeMap<String, NodeId>,
    pub renders: Vec<RenderDesc>,
}

impl Loaded {
    pub fn node(&self, name: &str) -> Result<NodeId, String> {
        self.nodes
            .get(name)
            .copied()
            .ok_or_else(|| format!("no node named `{name}`"))
    }
}

/// Builds the graph a description says; errors name what is wrong.
pub fn load(desc: Description) -> Result<Loaded, String> {
    let registry = registry();
    let mut graph = Graph::new();
    let mut nodes = BTreeMap::new();
    let mut add = |name: &str, id: NodeId| -> Result<(), String> {
        match nodes.insert(name.to_string(), id) {
            None => Ok(()),
            Some(_) => Err(format!("two nodes named `{name}`")),
        }
    };
    for s in &desc.sources {
        let image = match s.synthetic {
            Synthetic::Noise {
                width,
                height,
                channels,
                seed,
            } => helios_testkit::noise(Region::sized(width, height), channels, seed),
            Synthetic::Ramp {
                width,
                height,
                channels,
            } => helios_testkit::ramp(Region::sized(width, height), channels),
        };
        add(&s.name, graph.add_source(&s.name, image))?;
    }
    for m in &desc.modules {
        let module = registry.get(m.module.as_str()).ok_or_else(|| {
            format!(
                "module `{}` unknown; known: {}",
                m.module,
                registry.keys().copied().collect::<Vec<_>>().join(", ")
            )
        })?;
        let id = graph
            .add_module(&m.name, module.clone(), &m.params)
            .map_err(|e| e.to_string())?;
        add(&m.name, id)?;
    }
    let mut loaded = Loaded {
        graph,
        nodes,
        renders: desc.renders,
    };
    for c in &desc.connections {
        let (from, to) = (loaded.node(&c.from)?, loaded.node(&c.to)?);
        loaded
            .graph
            .connect(from, &c.port, to, &c.input)
            .map_err(|e| e.to_string())?;
    }
    for r in &loaded.renders {
        loaded.node(&r.node)?;
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Description {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn loads_the_documented_example() {
        let doc = r#"{
          "sources": [{ "name": "src", "synthetic": { "kind": "noise", "width": 64, "height": 48, "channels": 4, "seed": 1 } }],
          "modules": [{ "name": "exp", "module": "exposure", "params": { "exposure": { "float": 1.0 } } }],
          "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }],
          "renders": [{ "node": "exp", "port": "output", "region": { "x": 0, "y": 0, "width": 64, "height": 48 } }]
        }"#;
        let loaded = load(parse(doc)).unwrap();
        assert_eq!(loaded.nodes.len(), 2);
        assert_eq!(loaded.renders.len(), 1);
    }

    #[test]
    fn errors_say_what_is_wrong() {
        let unknown = load(parse(
            r#"{ "modules": [{ "name": "a", "module": "blur" }] }"#,
        ))
        .err()
        .unwrap();
        assert_eq!(unknown, "module `blur` unknown; known: exposure");
        let twice = load(parse(r#"{ "modules": [{ "name": "a", "module": "exposure" }, { "name": "a", "module": "exposure" }] }"#)).err().unwrap();
        assert_eq!(twice, "two nodes named `a`");
        let missing = load(parse(r#"{ "renders": [{ "node": "x", "port": "output", "region": { "x": 0, "y": 0, "width": 1, "height": 1 } }] }"#)).err().unwrap();
        assert_eq!(missing, "no node named `x`");
    }
}
