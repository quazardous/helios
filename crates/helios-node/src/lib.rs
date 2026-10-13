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
//!
//! Decision nodes (modules of the decision kind, such as
//! `decide_exposure`) take a mode in `decisions` (`full_ai`, `ai_assisted`
//! or `human`), and the network may name the model that answers them:
//!
//! ```json
//! "decisions": { "bright": "ai_assisted" },
//! "brief": "bright and airy",
//! "resolver": { "command": ["my-model", "--json"] }
//! ```
//!
//! or `"resolver": { "http": { "api": "openai", "url": "https://…/chat/completions",
//! "model": "…", "key_env": "OPENAI_API_KEY" } }` (`api` may be `anthropic`,
//! or `helios` for an endpoint speaking the command's JSON, which can
//! answer masks). The key is read from that environment variable when a
//! model is called. Resolvers a decision names by its question's
//! `resolver` parameter (an AI object mask member) are listed by name:
//! `"resolvers": { "local": { "command": ["segment", "--json"] } }`.
//! `"resolver": "routing"` asks the provider and model this machine's AI
//! routing map names (set up through the `helios_ai_*` MCP tools, see
//! `docs/ai-models.md`). Nothing is configured by default: no model, no
//! network.

pub mod batch;
pub mod darktable;
pub mod mcp;
pub mod sandbox;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use std::sync::Arc;

use helios_core::{Context, Graph, ModuleKind, ModuleRef, NodeId, Params, Region};
use helios_decision::Mode;
use helios_export::Format;
use helios_photo::CameraInfo;
use helios_resolve::command::Command;
use helios_resolve::http::{Api, Http};
use helios_resolve::{Cached, Resolver, Routed};
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
    /// Each decision node's mode, by node name.
    #[serde(default)]
    pub decisions: BTreeMap<String, Mode>,
    /// What the network is for, in words: given to the models.
    #[serde(default)]
    pub brief: Option<String>,
    #[serde(default)]
    pub resolver: Option<ResolverDesc>,
    /// Resolvers a decision may name (its question's `resolver`).
    #[serde(default)]
    pub resolvers: BTreeMap<String, ResolverDesc>,
}

/// The resolver of a network: its default one, and the named ones behind
/// a [`Routed`] when there are any. `None` when it names none.
pub fn build_resolvers(
    default: Option<&ResolverDesc>,
    named: &BTreeMap<String, ResolverDesc>,
) -> Result<Option<Arc<dyn Resolver>>, String> {
    let default = default.map(ResolverDesc::build).transpose()?;
    if named.is_empty() {
        return Ok(default);
    }
    let mut routed = Routed::new();
    if let Some(d) = default {
        routed = routed.default_to(d);
    }
    for (name, desc) in named {
        routed = routed.named(name, desc.build().map_err(|e| format!("`{name}`: {e}"))?);
    }
    Ok(Some(Arc::new(routed)))
}

/// The model that answers decisions.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResolverDesc {
    /// A program and its arguments, speaking JSON (see `helios-resolve`).
    Command(Vec<String>),
    Http(HttpDesc),
    /// This machine's AI routing map (`"resolver": "routing"`): the
    /// provider and model routed for `vision`, `segmentation` for a mask,
    /// or the capability a decision names (see `docs/ai-models.md`).
    Routing,
}

#[derive(Clone, Debug, Deserialize)]
pub struct HttpDesc {
    pub api: HttpApi,
    pub url: String,
    pub model: String,
    #[serde(default)]
    pub key_env: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpApi {
    OpenAi,
    Anthropic,
    Helios,
}

impl ResolverDesc {
    /// The resolver this describes, behind a cache: the same pixels are
    /// never asked about twice.
    pub fn build(&self) -> Result<Arc<dyn Resolver>, String> {
        Ok(match self {
            ResolverDesc::Command(argv) => {
                let (program, args) = argv
                    .split_first()
                    .ok_or("`resolver.command` names no program")?;
                let mut c = Command::new(program);
                for a in args {
                    c = c.arg(a);
                }
                Arc::new(Cached::new(c))
            }
            ResolverDesc::Http(h) => {
                let api = match h.api {
                    HttpApi::OpenAi => Api::OpenAi,
                    HttpApi::Anthropic => Api::Anthropic,
                    HttpApi::Helios => Api::Helios,
                };
                let mut http = Http::new(api, &h.url, &h.model);
                if let Some(var) = &h.key_env {
                    http = http.key_from(var);
                }
                Arc::new(Cached::new(http))
            }
            ResolverDesc::Routing => {
                let ai = helios_ai::Ai::open(helios_ai::store::Paths::default_for_user())?;
                Arc::new(Cached::new(Routed::new().through(Arc::new(ai))))
            }
        })
    }
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
    BTreeMap::from([
        (
            "rawprepare",
            ModuleRef::new(helios_module_rawprepare::RawPrepare),
        ),
        (
            "whitebalance",
            ModuleRef::new(helios_module_whitebalance::WhiteBalance),
        ),
        (
            "highlights",
            ModuleRef::new(helios_module_highlights::Highlights),
        ),
        ("demosaic", ModuleRef::new(helios_module_demosaic::Demosaic)),
        ("colorin", ModuleRef::new(helios_module_colorin::ColorIn)),
        ("exposure", ModuleRef::new(helios_module_exposure::Exposure)),
        (
            "decide_exposure",
            ModuleRef::new(helios_module_exposure::DecideExposure),
        ),
        ("sigmoid", ModuleRef::new(helios_module_sigmoid::Sigmoid)),
        ("blur", ModuleRef::new(helios_module_blur::Blur)),
        (
            "vkdt_exposure",
            ModuleRef::new(helios_module_vkdt::VkdtExposure),
        ),
        ("vkdt_usm", ModuleRef::new(helios_module_vkdt::VkdtUsm)),
        (
            "vkdt_contrast",
            ModuleRef::composite(helios_module_vkdt::VkdtContrast),
        ),
        (
            "mask_combine",
            ModuleRef::new(helios_module_masks::MaskCombine),
        ),
        ("mask_blend", ModuleRef::new(helios_module_masks::MaskBlend)),
        (
            "mask_feather",
            ModuleRef::new(helios_module_masks::MaskFeather),
        ),
        ("mask_blur", ModuleRef::new(helios_module_masks::MaskBlur)),
        (
            "mask_levels",
            ModuleRef::new(helios_module_masks::MaskLevels),
        ),
        (
            "mask_from_luma",
            ModuleRef::new(helios_module_masks::MaskFromLuma),
        ),
        (
            "mask_parametric",
            ModuleRef::new(helios_module_masks::MaskParametric),
        ),
        (
            "mask_raster",
            ModuleRef::new(helios_module_masks::MaskRaster),
        ),
        (
            "mask_circle",
            ModuleRef::new(helios_module_masks::MaskCircle),
        ),
        (
            "mask_ellipse",
            ModuleRef::new(helios_module_masks::MaskEllipse),
        ),
        (
            "mask_gradient",
            ModuleRef::new(helios_module_masks::MaskGradient),
        ),
        ("mask_path", ModuleRef::new(helios_module_masks::MaskPath)),
        ("mask_brush", ModuleRef::new(helios_module_masks::MaskBrush)),
        (
            "mask_object",
            ModuleRef::new(helios_module_masks::MaskObject),
        ),
    ])
}

/// A network ready to evaluate.
pub struct Loaded {
    pub graph: Graph,
    pub nodes: BTreeMap<String, NodeId>,
    pub renders: Vec<RenderDesc>,
    pub exports: Vec<ExportDesc>,
    /// Every decision node with its mode (`human` unless the description says).
    pub decisions: BTreeMap<String, Mode>,
    pub brief: Option<String>,
    pub resolver: Option<ResolverDesc>,
    pub resolvers: BTreeMap<String, ResolverDesc>,
    /// The first source: decisions look at its region.
    pub frame: Option<NodeId>,
    /// The file of the first source, when it is read from one: the image
    /// whose edit a node with a vault keeps.
    pub frame_file: Option<PathBuf>,
    /// The camera of each source loaded from a raw file.
    pub cameras: BTreeMap<NodeId, CameraInfo>,
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

/// Gives every module reading one raw, directly or not, that raw's camera
/// as context; the others get none.
pub fn give_cameras(
    graph: &mut Graph,
    cameras: &BTreeMap<NodeId, CameraInfo>,
) -> Result<(), String> {
    let ids: Vec<NodeId> = graph.ids().collect();
    for id in ids {
        let mut seen = BTreeSet::new();
        upstream(graph, id, &mut seen)?;
        let mut found = seen.iter().filter_map(|s| cameras.get(s));
        let context = match (found.next(), found.next()) {
            (Some(camera), None) => camera.to_context(),
            _ => Context::new(),
        };
        graph.set_context(id, context).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// What a source was given by [`load_source`].
#[derive(Clone, Debug, serde::Serialize)]
pub struct SourceLoaded {
    /// BLAKE3 of the file.
    pub hash: String,
    pub region: Region,
    pub channels: u32,
    /// Whether the file is a raw, with a camera.
    pub raw: bool,
}

/// Gives the source `id` the image of a file: a raw with its camera (the
/// modules reading it get it as context) or a picture.
pub fn load_source(
    graph: &mut Graph,
    cameras: &mut BTreeMap<NodeId, CameraInfo>,
    id: NodeId,
    loaded: helios_load::Loaded,
) -> Result<SourceLoaded, String> {
    let answer = SourceLoaded {
        hash: loaded.hash,
        region: loaded.image.region(),
        channels: loaded.image.channels(),
        raw: loaded.camera.is_some(),
    };
    graph
        .set_source(id, loaded.image)
        .map_err(|e| e.to_string())?;
    match loaded.camera {
        Some(camera) => cameras.insert(id, camera),
        None => cameras.remove(&id),
    };
    give_cameras(graph, cameras)?;
    Ok(answer)
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
    let mut frame = None;
    let mut frame_file = None;
    for s in &desc.sources {
        let synthetic = match (&s.synthetic, &s.file) {
            (Some(synthetic), None) => synthetic,
            (None, Some(file)) => {
                let loaded = helios_load::load(&base.join(file))?;
                let id = graph.add_source(&s.name, loaded.image);
                if frame.is_none() {
                    frame_file = Some(base.join(file));
                }
                frame.get_or_insert(id);
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
        let id = graph.add_source(&s.name, image);
        frame.get_or_insert(id);
        add(&s.name, id)?;
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
    let mut decisions = BTreeMap::new();
    for m in &desc.modules {
        if registry[m.module.as_str()].manifest().kind == ModuleKind::Decision {
            let mode = desc.decisions.get(&m.name).copied().unwrap_or(Mode::Human);
            decisions.insert(m.name.clone(), mode);
        }
    }
    if let Some(name) = desc.decisions.keys().find(|n| !decisions.contains_key(*n)) {
        return Err(format!(
            "`decisions` names `{name}`, which is not a decision node"
        ));
    }
    let mut loaded = Loaded {
        graph,
        nodes,
        renders: desc.renders,
        exports: desc.exports,
        decisions,
        brief: desc.brief,
        resolver: desc.resolver,
        resolvers: desc.resolvers,
        frame,
        frame_file,
        cameras: BTreeMap::new(),
    };
    for c in &desc.connections {
        let (from, to) = (loaded.node(&c.from)?, loaded.node(&c.to)?);
        loaded
            .graph
            .connect(from, &c.port, to, &c.input)
            .map_err(|e| e.to_string())?;
    }
    loaded.cameras = cameras;
    give_cameras(&mut loaded.graph, &loaded.cameras)?;
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
            r#"{ "modules": [{ "name": "a", "module": "nope" }] }"#,
        ))
        .err()
        .unwrap();
        let known: Vec<_> = registry().keys().copied().collect();
        assert_eq!(
            unknown,
            format!("module `nope` unknown; known: {}", known.join(", "))
        );
        let twice = load(parse(r#"{ "modules": [{ "name": "a", "module": "exposure" }, { "name": "a", "module": "exposure" }] }"#)).err().unwrap();
        assert_eq!(twice, "two nodes named `a`");
        let missing = load(parse(r#"{ "renders": [{ "node": "x", "port": "output", "region": { "x": 0, "y": 0, "width": 1, "height": 1 } }] }"#)).err().unwrap();
        assert_eq!(missing, "no node named `x`");
    }
}
