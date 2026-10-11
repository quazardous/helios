//! A network described as JSON, and the modules a node knows by name.
//!
//! ```json
//! {
//!   "sources": [{ "name": "src", "synthetic": { "kind": "noise", "width": 64, "height": 48, "channels": 4, "seed": 1 } }],
//!   "modules": [{ "name": "exp", "module": "exposure", "params": { "exposure": { "float": 1.0 } } }],
//!   "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }],
//!   "renders": [{ "node": "exp", "port": "output", "region": { "x": 0, "y": 0, "width": 64, "height": 48 } }],
//!   "exports": [{ "node": "exp", "region": { "x": 0, "y": 0, "width": 64, "height": 48 }, "path": "out.png" }]
//! }
//! ```
//!
//! A source is a synthetic image, or a file read by the framework
//! (`{ "name": "src", "file": "IMG_0001.CR2" }`, relative to the network
//! file): a raw loads as its sensor mosaic, and every module that reads it,
//! directly or not, gets the camera's description as context; a picture
//! loads in the working space. An export writes a node's `output`
//! port (or the `port` given) to a file whose format follows its extension,
//! unless `format` says otherwise (`{ "format": "jpeg", "quality": 90 }`,
//! `{ "format": "png", "bits": 16 }`, `{ "format": "tiff" }`).

pub mod mcp;
pub mod sandbox;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use helios_core::{Graph, ModuleRef, NodeId, Params, Region};
use helios_export::Format;
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
    #[serde(default)]
    pub exports: Vec<ExportDesc>,
}

/// A source: a synthetic image, or a file read by the framework.
#[derive(Debug, Deserialize)]
pub struct SourceDesc {
    pub name: String,
    #[serde(default)]
    pub synthetic: Option<Synthetic>,
    /// A raw file or a picture, relative to the network file.
    #[serde(default)]
    pub file: Option<PathBuf>,
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

#[derive(Clone, Debug, Deserialize)]
pub struct ExportDesc {
    pub node: String,
    #[serde(default = "output_port")]
    pub port: String,
    pub region: Region,
    pub path: PathBuf,
    #[serde(default)]
    pub format: Option<Format>,
}

fn output_port() -> String {
    "output".into()
}

impl ExportDesc {
    /// The format asked for, else the one the path's extension names.
    pub fn format(&self) -> Result<Format, String> {
        self.format
            .clone()
            .or_else(|| Format::from_path(&self.path))
            .ok_or_else(|| format!("no format for {}: say `format`", self.path.display()))
    }
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
    pub exports: Vec<ExportDesc>,
}

impl Loaded {
    pub fn node(&self, name: &str) -> Result<NodeId, String> {
        self.nodes
            .get(name)
            .copied()
            .ok_or_else(|| format!("no node named `{name}`"))
    }
}

fn upstream(graph: &Graph, id: NodeId, seen: &mut BTreeSet<NodeId>) -> Result<(), String> {
    for src in graph
        .input_nodes(id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .flatten()
    {
        if seen.insert(src) {
            upstream(graph, src, seen)?;
        }
    }
    Ok(())
}

/// Builds the graph a description says, files read relative to the
/// current directory; errors name what is wrong.
pub fn load(desc: Description) -> Result<Loaded, String> {
    load_in(desc, Path::new("."))
}

/// [`load`] for a description whose files are relative to `base` (the
/// directory of the network file): sources and exports.
pub fn load_in(mut desc: Description, base: &Path) -> Result<Loaded, String> {
    for e in &mut desc.exports {
        e.path = base.join(&e.path);
    }
    let registry = registry();
    let mut graph = Graph::new();
    let mut nodes = BTreeMap::new();
    let mut add = |name: &str, id: NodeId| -> Result<(), String> {
        match nodes.insert(name.to_string(), id) {
            None => Ok(()),
            Some(_) => Err(format!("two nodes named `{name}`")),
        }
    };
    let mut cameras = BTreeMap::new();
    for s in &desc.sources {
        let synthetic = match (&s.synthetic, &s.file) {
            (Some(synthetic), None) => synthetic,
            (None, Some(file)) => {
                let loaded = helios_load::load(&base.join(file))?;
                let id = graph.add_source(&s.name, loaded.image);
                if let Some(camera) = loaded.camera {
                    cameras.insert(id, camera);
                }
                add(&s.name, id)?;
                continue;
            }
            _ => {
                return Err(format!(
                    "source `{}`: say either `synthetic` or `file`",
                    s.name
                ));
            }
        };
        let image = match *synthetic {
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
        exports: desc.exports,
    };
    for c in &desc.connections {
        let (from, to) = (loaded.node(&c.from)?, loaded.node(&c.to)?);
        loaded
            .graph
            .connect(from, &c.port, to, &c.input)
            .map_err(|e| e.to_string())?;
    }
    // A module reading one raw, directly or not, gets its camera as context.
    let ids: Vec<NodeId> = loaded.nodes.values().copied().collect();
    for id in ids {
        let mut seen = BTreeSet::new();
        upstream(&loaded.graph, id, &mut seen)?;
        let mut found = seen.iter().filter_map(|s| cameras.get(s));
        if let (Some(camera), None) = (found.next(), found.next()) {
            loaded
                .graph
                .set_context(id, camera.to_context())
                .map_err(|e| e.to_string())?;
        }
    }
    for r in &loaded.renders {
        loaded.node(&r.node)?;
    }
    for e in &loaded.exports {
        loaded.node(&e.node)?;
        e.format()?;
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
