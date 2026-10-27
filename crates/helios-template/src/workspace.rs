//! A workspace made from a template, as it is edited (see
//! `docs/templates.md`, "Editing a workspace").
//!
//! A workspace keeps its own files under `<config>/workspaces/<key>/`:
//!
//! - `template.helios-template/` ([`ORIGIN_DIR`]): the template it was
//!   made from, as it was: what a reset goes back to;
//! - `workspace.helios-template/` ([`STATE_DIR`]): the workspace as it
//!   stands (its network, tools, library and a copy of its layout),
//!   written as a bundle once it differs from its template;
//! - `workspace.json` ([`POINTER`]): its profile, the template it came
//!   from, and its own name, description and icon.
//!
//! Its layout is egui-pop's document,
//! `<config>/ui/workspaces/<key>/layout.json`.
//!
//! What a workspace is made of comes in named [`Part`]s: they are listed
//! ([`parts`]), copied from another workspace or template ([`import`],
//! with its conflicts), and a workspace is extracted back into a template
//! ([`review`] then [`save_template`]): what goes in, what is stripped
//! (images, edit history, absolute paths, credentials, embedded data) and
//! what becomes a parameter asked on create. Nothing keeps where an
//! imported part came from: an import is a copy.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use egui_pop_model::ids::PanelId;
use egui_pop_model::layout::Layout;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::bundle::{Bundle, EXTENSION, Text};
use crate::catalogue::Catalogue;
use crate::format::{
    Binding, CompositeFile, Connection, Endpoint, Exit, FORMAT, InnerDesc, NetworkDesc, NodeDesc,
    PORTS, Parameter, ParameterKind, Port, Template, slug, valid_id, valid_name,
};

/// Where a workspace keeps the template it was made from.
pub const ORIGIN_DIR: &str = "template.helios-template";
/// Where a workspace keeps what it is now, once edited.
pub const STATE_DIR: &str = "workspace.helios-template";
/// The workspace's pointer: its profile, its template, its name.
pub const POINTER: &str = "workspace.json";
/// Where the user's templates are, under the configuration directory.
pub const TEMPLATES_DIR: &str = "templates";
/// Where the steps an agent made on a workspace's files are kept, to be
/// undone.
pub const UNDO_DIR: &str = "undo";
/// How many of those are kept.
pub const UNDO_KEPT: usize = 20;
/// The id of the state a workspace not made from a template starts from.
pub const BLANK_ID: &str = "helios.workspace";

/// The user's configuration directory for Helios:
/// `$XDG_CONFIG_HOME/helios`, else `~/.config/helios`.
pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("helios")
}

/// A workspace directory as a file name: every character other than a
/// letter, a digit, `.` or `-` written `_`.
pub fn key(workspace: &Path) -> String {
    workspace
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Where a workspace's files are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Files {
    /// `<config>/workspaces/<key>`.
    pub own: PathBuf,
    /// egui-pop's layout document for it.
    pub layout: PathBuf,
    /// Its directory's name: its name until it is given one.
    pub dir_name: String,
}

impl Files {
    pub fn new(config: &Path, workspace: &Path) -> Self {
        let k = key(workspace);
        Files {
            own: config.join("workspaces").join(&k),
            layout: config
                .join("ui")
                .join("workspaces")
                .join(&k)
                .join("layout.json"),
            dir_name: workspace.file_name().map_or_else(
                || workspace.display().to_string(),
                |n| n.to_string_lossy().into(),
            ),
        }
    }

    /// `workspace.json`, an empty object when it is missing.
    pub fn pointer(&self) -> Json {
        std::fs::read_to_string(self.own.join(POINTER))
            .ok()
            .and_then(|t| serde_json::from_str::<Json>(&t).ok())
            .filter(Json::is_object)
            .unwrap_or_else(|| json!({}))
    }

    fn write_pointer(&self, pointer: &Json) -> Result<(), String> {
        write(&self.own.join(POINTER), &pointer.to_string())
    }

    /// The template the workspace was made from, if it was.
    pub fn origin(&self) -> Option<Result<Bundle, String>> {
        let dir = self.own.join(ORIGIN_DIR);
        dir.is_dir().then(|| Bundle::load(&dir))
    }

    /// Whether the workspace has files of its own (made from a template,
    /// or edited).
    pub fn exists(&self) -> bool {
        self.own.join(ORIGIN_DIR).is_dir() || self.own.join(STATE_DIR).is_dir()
    }

    /// The layout egui-pop last saved for the workspace.
    pub fn saved_layout(&self) -> Option<Layout> {
        let text = std::fs::read_to_string(&self.layout).ok()?;
        Layout::from_json(&text).ok()
    }

    /// The workspace as it stands: its edited state, else its template,
    /// else the blank network; the layout egui-pop saved when there is
    /// one; its own name, description and icon.
    pub fn load(&self) -> Result<State, String> {
        let mut state = self.load_on_disk()?;
        if let Some(layout) = self.saved_layout() {
            state.bundle.layout = layout;
        }
        Ok(state)
    }

    /// The workspace as its own files hold it: as [`Files::load`], the
    /// layout being the copy written with its state rather than
    /// egui-pop's live document. What a desktop compares with what it
    /// last wrote, to see a change made outside it.
    pub fn load_on_disk(&self) -> Result<State, String> {
        let state = self.own.join(STATE_DIR);
        let bundle = if state.is_dir() {
            Bundle::load(&state)?
        } else {
            match self.origin() {
                Some(b) => b?,
                None => blank(),
            }
        };
        let pointer = self.pointer();
        let text = |k: &str| pointer[k].as_str().map(String::from);
        Ok(State {
            meta: Meta {
                name: text("name").unwrap_or_else(|| self.dir_name.clone()),
                description: text("description").unwrap_or_default(),
                icon: text("icon"),
            },
            bundle,
        })
    }

    /// Writes the workspace's state: its bundle (in place of the one
    /// before, never half written) and its name, description and icon in
    /// its pointer; with `layout`, egui-pop's layout document too.
    pub fn save(&self, state: &State, layout: bool) -> Result<(), String> {
        let dir = self.own.join(STATE_DIR);
        let next = self.own.join(format!("{STATE_DIR}.next"));
        let _ = std::fs::remove_dir_all(&next);
        state.bundle.save(&next)?;
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::rename(&next, &dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut pointer = self.pointer();
        if pointer.get("profile").is_none() {
            pointer["profile"] = json!("default");
        }
        pointer["name"] = json!(state.meta.name);
        pointer["description"] = json!(state.meta.description);
        pointer["icon"] = state.meta.icon.as_ref().map_or(Json::Null, |i| json!(i));
        self.write_pointer(&pointer)?;
        if layout {
            write(&self.layout, &state.bundle.layout.to_json())?;
        }
        Ok(())
    }

    /// Writes `next` as the workspace's state, keeping `before` to be
    /// undone ([`Files::undo`]); what an agent's step on the files does.
    pub fn save_step(&self, before: &State, next: &State, label: &str) -> Result<(), String> {
        let dir = self.own.join(UNDO_DIR);
        let mut steps = undo_steps(&dir);
        let n = steps.last().map_or(1, |(n, _)| n + 1);
        let record = json!({
            "label": label,
            "state": before.to_json(),
            "after": comparable(next),
        });
        write(&dir.join(format!("{n:08}.json")), &record.to_string())?;
        steps.push((n, dir.join(format!("{n:08}.json"))));
        while steps.len() > UNDO_KEPT {
            let (_, old) = steps.remove(0);
            let _ = std::fs::remove_file(old);
        }
        self.save(next, true)
    }

    /// Undoes the last step an agent made on the files: the state before
    /// it is written back. Returns that step's label. Refused when the
    /// workspace changed since that step (a desktop edited it, or another
    /// writer): writing the state before it back would drop that change.
    pub fn undo(&self) -> Result<String, String> {
        let dir = self.own.join(UNDO_DIR);
        let (_, last) = undo_steps(&dir)
            .pop()
            .ok_or("nothing to undo in this workspace")?;
        let text =
            std::fs::read_to_string(&last).map_err(|e| format!("{}: {e}", last.display()))?;
        let record: Json = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let label = record["label"].as_str().unwrap_or_default().to_string();
        if let Some(after) = record.get("after")
            && *after != comparable(&self.load()?)
        {
            return Err(format!(
                "the workspace changed since the step `{label}` was made: undoing it would drop that change"
            ));
        }
        let state = State::from_json(&record["state"])?;
        self.save(&state, true)?;
        std::fs::remove_file(&last).map_err(|e| format!("{}: {e}", last.display()))?;
        Ok(label)
    }

    /// The desktop that holds the workspace open, as it said when it
    /// opened it; `None` when none said so (or the file is unreadable).
    /// It may have gone since without a word (killed): a holder that
    /// cannot be reached holds nothing.
    pub fn holder(&self) -> Option<Holder> {
        let text = std::fs::read_to_string(self.own.join(HOLDER)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Says that `holder` holds the workspace open.
    pub fn hold(&self, holder: &Holder) -> Result<(), String> {
        write(
            &self.own.join(HOLDER),
            &serde_json::to_string(holder).expect("a holder serializes"),
        )
    }

    /// Says that the desktop of process `pid` no longer holds the
    /// workspace, unless another one took it since.
    pub fn release(&self, pid: u32) {
        if self.holder().is_some_and(|h| h.pid == pid) {
            let _ = std::fs::remove_file(self.own.join(HOLDER));
        }
    }
}

/// Where a desktop says it holds a workspace open, among the
/// workspace's own files: the agents' tools go through it then, rather
/// than writing the files under it.
pub const HOLDER: &str = "desktop.json";

/// A desktop holding a workspace open: its process, and the endpoint its
/// workspace requests go to (a Unix socket's path, or `tcp://HOST:PORT`
/// with the session token).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub pid: u32,
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// A state as compared to tell whether a workspace changed: its layout
/// left out, which egui-pop saves on its own as the user moves panels.
fn comparable(state: &State) -> Json {
    let mut json = state.to_json();
    if let Some(o) = json.as_object_mut() {
        o.remove("layout");
    }
    json
}

/// The parts a desktop merges by when the files of the workspace it has
/// open change outside it ([`merge`]), each with where it lies in
/// [`State::to_json`].
const MERGED: &[(&str, &[&str])] = &[
    ("name", &["/meta/name"]),
    ("description", &["/meta/description"]),
    ("icon", &["/meta/icon"]),
    (
        "network",
        &[
            "/template/network",
            "/composites",
            "/template/subworkspaces",
            "/template/retired_subworkspaces",
            "/template/positions",
        ],
    ),
    ("tools", &["/template/tools"]),
    ("library", &["/template/library"]),
    ("layout", &["/layout"]),
    (
        "template",
        &[
            "/template/format",
            "/template/id",
            "/template/version",
            "/template/meta",
            "/template/parameters",
            "/texts",
        ],
    ),
];

/// What [`merge`] made.
#[derive(Clone, Debug, PartialEq)]
pub struct Reloaded {
    pub state: State,
    /// The parts taken from the files.
    pub taken: Vec<&'static str>,
    /// Among them, the parts the desktop had changed too: the files'
    /// version won.
    pub both: Vec<&'static str>,
}

/// A workspace's files changed under a desktop that has it open: `base`
/// is what the desktop last wrote or read there, `ours` what it shows,
/// `theirs` what the files hold now. Part by part ([`MERGED`]): a part
/// only the files changed is taken from them, a part only the desktop
/// changed is kept, and a part both changed differently is taken from
/// the files (listed in [`Reloaded::both`]; the desktop keeps its own
/// version one undo step back).
pub fn merge(base: &State, ours: &State, theirs: &State) -> Result<Reloaded, String> {
    let (b, o, t) = (base.to_json(), ours.to_json(), theirs.to_json());
    let mut out = o.clone();
    let (mut taken, mut both) = (Vec::new(), Vec::new());
    for (part, pointers) in MERGED {
        let at = |j: &Json| -> Vec<Option<Json>> {
            pointers.iter().map(|p| j.pointer(p).cloned()).collect()
        };
        let (pb, po, pt) = (at(&b), at(&o), at(&t));
        if pt == po || pt == pb {
            continue;
        }
        if po != pb {
            both.push(*part);
        }
        taken.push(*part);
        for (pointer, value) in pointers.iter().zip(pt) {
            put(&mut out, pointer, value);
        }
    }
    Ok(Reloaded {
        state: State::from_json(&out)?,
        taken,
        both,
    })
}

/// Sets (or, with `None`, removes) the value at a JSON pointer whose
/// parent is an object.
fn put(json: &mut Json, pointer: &str, value: Option<Json>) {
    let (parent, key) = pointer.rsplit_once('/').unwrap_or(("", pointer));
    if let Some(Json::Object(o)) = json.pointer_mut(parent) {
        match value {
            Some(v) => {
                o.insert(key.to_string(), v);
            }
            None => {
                o.remove(key);
            }
        }
    }
}

/// The undo records of a workspace, oldest first.
fn undo_steps(dir: &Path) -> Vec<(u64, PathBuf)> {
    let mut v: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
        .map(|r| {
            r.filter_map(|e| {
                let path = e.ok()?.path();
                let n = path.file_stem()?.to_str()?.parse().ok()?;
                Some((n, path))
            })
            .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// A workspace's own name, description and icon (a template's are its
/// scenario's).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// A workspace as it stands: its name, and a bundle holding its network,
/// tools, library and layout.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub meta: Meta,
    pub bundle: Bundle,
}

impl State {
    /// As one JSON value (what the desktop keeps as an undoable value).
    pub fn to_json(&self) -> Json {
        let texts: Vec<Json> = self
            .bundle
            .texts
            .iter()
            .map(|t| json!({ "locale": t.locale, "name": t.name, "source": t.source }))
            .collect();
        json!({
            "meta": self.meta,
            "template": self.bundle.template,
            "layout": serde_json::from_str::<Json>(&self.bundle.layout.to_json())
                .unwrap_or(Json::Null),
            "composites": self.bundle.composites,
            "texts": texts,
        })
    }

    /// Reads [`State::to_json`]'s value back.
    pub fn from_json(json: &Json) -> Result<Self, String> {
        let meta: Meta =
            serde_json::from_value(json["meta"].clone()).map_err(|e| format!("meta: {e}"))?;
        let template: Template = serde_json::from_value(json["template"].clone())
            .map_err(|e| format!("template: {e}"))?;
        let layout = Layout::from_json(&json["layout"].to_string())?;
        let mut bundle = Bundle::new(template, layout);
        bundle.composites = serde_json::from_value(json["composites"].clone())
            .map_err(|e| format!("composites: {e}"))?;
        for t in json["texts"].as_array().into_iter().flatten() {
            let field = |k: &str| t[k].as_str().unwrap_or_default().to_string();
            bundle.texts.push(Text {
                locale: field("locale"),
                name: field("name"),
                source: field("source"),
            });
        }
        Ok(State { meta, bundle })
    }

    /// The problems that keep it from being used: its bundle checked
    /// against `catalogue`, as a template is.
    pub fn problems(&self, catalogue: &Catalogue) -> Vec<String> {
        self.bundle.validate(catalogue).problems
    }

    /// The state with `changes` made: `name`, `description`, `icon`,
    /// `network`, `subworkspaces`, `positions`, `tools`, `library` (null
    /// for none) and `layout`, each written as in a template. Fields it
    /// does not know are refused. A sub-workspace left out is deleted
    /// (its id retired), and a node taken out of the network leaves
    /// every sub-workspace and the graph view's positions.
    pub fn edited(&self, changes: &Json) -> Result<State, String> {
        let object = changes.as_object().ok_or("changes are an object")?;
        let mut next = self.clone();
        for (k, v) in object {
            let bad = |e: serde_json::Error| format!("{k}: {e}");
            match k.as_str() {
                "name" => {
                    let name = v.as_str().ok_or("name: text")?.trim();
                    if name.is_empty() {
                        return Err("name: empty".into());
                    }
                    next.meta.name = name.into();
                }
                "description" => {
                    next.meta.description = v.as_str().ok_or("description: text")?.into();
                }
                "icon" => {
                    next.meta.icon = v
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from);
                }
                "network" => {
                    next.bundle.template.network =
                        serde_json::from_value(v.clone()).map_err(bad)?;
                }
                "subworkspaces" => {
                    next.bundle.template.subworkspaces =
                        serde_json::from_value(v.clone()).map_err(bad)?;
                }
                "positions" => {
                    next.bundle.template.positions =
                        serde_json::from_value(v.clone()).map_err(bad)?;
                }
                "tools" => {
                    next.bundle.template.tools = serde_json::from_value(v.clone()).map_err(bad)?;
                }
                "library" => {
                    next.bundle.template.library =
                        serde_json::from_value(v.clone()).map_err(bad)?;
                }
                "layout" => next.bundle.layout = Layout::from_json(&v.to_string())?,
                other => return Err(format!("`{other}` is not a part of a workspace")),
            }
        }
        next.bundle
            .template
            .follow_subworkspaces(&self.bundle.template);
        next.bundle.template.follow_positions();
        Ok(next)
    }

    /// The state with a new sub-workspace `name` over `nodes` (see
    /// [`Template::delegated`]), and what was done.
    pub fn delegated(&self, name: &str, nodes: &[String]) -> Result<(State, Json), String> {
        let (template, answer) = self.bundle.template.delegated(name, nodes)?;
        let mut next = self.clone();
        next.bundle.template = template;
        Ok((next, answer))
    }

    /// The state without the sub-workspace `id`, retired (see
    /// [`Template::without_subworkspace`]), and what was done.
    pub fn subworkspace_deleted(&self, id: &str) -> Result<(State, Json), String> {
        let (template, answer) = self.bundle.template.without_subworkspace(id)?;
        let mut next = self.clone();
        next.bundle.template = template;
        Ok((next, answer))
    }

    /// The state its template gives, built (the composites' panel hints
    /// applied to the layout); its own name, description and icon kept.
    pub fn reset(&self, origin: &Bundle, catalogue: &Catalogue) -> Result<State, String> {
        let mut bundle = origin.clone();
        bundle.bundle_composites(catalogue)?;
        bundle.layout = bundle.build(catalogue)?.layout;
        // The ids used before stay retired: the context stack or an undo
        // step may still name them.
        bundle.template.follow_subworkspaces(&self.bundle.template);
        Ok(State {
            meta: self.meta.clone(),
            bundle,
        })
    }
}

/// The network of a workspace not made from a template: an exposure
/// mixed into the image through a mask the viewport draws in (with no
/// shape yet the mask is 1 everywhere, so the exposure applies fully).
pub fn blank_network() -> NetworkDesc {
    serde_json::from_value(json!({
        "sources": ["source"],
        "nodes": [
            { "name": "exposure", "module": "exposure" },
            { "name": "mask", "module": "mask_combine" },
            { "name": "blend", "module": "mask_blend" }
        ],
        "connections": [
            { "from": "source", "port": "image", "to": "exposure", "input": "input" },
            { "from": "source", "port": "image", "to": "blend", "input": "input" },
            { "from": "exposure", "port": "output", "to": "blend", "input": "processed" },
            { "from": "mask", "port": "mask", "to": "blend", "input": "mask" }
        ],
        "view": { "node": "blend", "port": "output" }
    }))
    .expect("the blank network reads")
}

/// The state of a workspace not made from a template: [`blank_network`],
/// no tools, no library, an empty layout (the one shown is kept).
pub fn blank() -> Bundle {
    let template = Template {
        format: FORMAT,
        id: BLANK_ID.into(),
        version: 1,
        meta: crate::format::Meta {
            name: "Workspace".into(),
            scenario: "Develop an image: an exposure through a mask".into(),
            ..crate::format::Meta::default()
        },
        network: blank_network(),
        tools: crate::format::Tools::default(),
        library: None,
        parameters: Vec::new(),
        subworkspaces: Vec::new(),
        retired_subworkspaces: Vec::new(),
        positions: BTreeMap::new(),
    };
    Bundle::new(template, Layout::default())
}

/// Creates a workspace from a template, its parameters answered (by
/// id): the directory (made when missing), a copy of the template among
/// the workspace's own files ([`ORIGIN_DIR`], its composites bundled),
/// named in its pointer, and the built layout as egui-pop's document.
/// Refused, before anything is written, when the template does not
/// check against `catalogue`. Returns the directory as opened, and the
/// template built.
pub fn create(
    template: &Bundle,
    answers: &BTreeMap<String, Json>,
    catalogue: &Catalogue,
    workspace: &Path,
    config: &Path,
) -> Result<(PathBuf, crate::Built), String> {
    let mut copy = template.clone();
    copy.template = template.template.answered(answers)?;
    let report = copy.validate(catalogue);
    if !report.is_ok() {
        return Err(format!(
            "template {}: {}",
            template.template.id,
            report.problems.join("; ")
        ));
    }
    copy.bundle_composites(catalogue)?;
    let built = copy.build(catalogue)?;
    std::fs::create_dir_all(workspace).map_err(|e| format!("{}: {e}", workspace.display()))?;
    let workspace = workspace
        .canonicalize()
        .map_err(|e| format!("{}: {e}", workspace.display()))?;
    let files = Files::new(config, &workspace);
    copy.save(&files.own.join(ORIGIN_DIR))?;
    let mut pointer = files.pointer();
    if pointer.get("profile").is_none() {
        pointer["profile"] = json!("default");
    }
    pointer["template"] = json!({ "id": copy.template.id, "version": copy.template.version });
    files.write_pointer(&pointer)?;
    write(&files.layout, &built.layout.to_json())?;
    Ok((workspace, built))
}

/// The templates the user saved (`<config>/templates/*.helios-template`),
/// each read or why it could not be.
pub fn user_templates(config: &Path) -> Vec<Result<Bundle, String>> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(config.join(TEMPLATES_DIR))
        .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    dirs.retain(|d| d.is_dir() && d.to_string_lossy().ends_with(EXTENSION));
    dirs.sort();
    dirs.iter().map(|d| Bundle::load(d.as_path())).collect()
}

/// What parts come from: a template bundle's directory, a workspace's
/// directory, or a template's id (built in, then the user's).
pub fn source(config: &Path, what: &str) -> Result<Bundle, String> {
    let path = Path::new(what);
    if what.ends_with(EXTENSION) && path.is_dir() {
        return Bundle::load(path);
    }
    if path.is_absolute() {
        let files = Files::new(config, path);
        if !files.exists() && !path.is_dir() {
            return Err(format!("{what}: no workspace or template there"));
        }
        return files.load().map(|s| s.bundle);
    }
    crate::shipped::templates()
        .into_iter()
        .chain(user_templates(config).into_iter().filter_map(Result::ok))
        .find(|b| b.template.id == what)
        .ok_or_else(|| format!("no template `{what}`"))
}

/// A part of a workspace, importable on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Part {
    /// Its network, imported as an opened composite wired to the image.
    Network,
    /// Where its panels are.
    Layout,
    /// Its key bindings.
    Keys,
    /// Its tools.
    Toolbox,
    /// Its import source and culling settings.
    Library,
}

impl Part {
    pub const ALL: [Part; 5] = [
        Part::Network,
        Part::Layout,
        Part::Keys,
        Part::Toolbox,
        Part::Library,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Part::Network => "network",
            Part::Layout => "layout",
            Part::Keys => "keys",
            Part::Toolbox => "toolbox",
            Part::Library => "library",
        }
    }

    pub fn parse(name: &str) -> Result<Part, String> {
        Part::ALL
            .into_iter()
            .find(|p| p.name() == name)
            .ok_or_else(|| {
                format!("`{name}` is not a part (network, layout, keys, toolbox or library)")
            })
    }

    /// Parts by name; every part for an empty list.
    pub fn list(names: &[String]) -> Result<Vec<Part>, String> {
        if names.is_empty() {
            return Ok(Part::ALL.to_vec());
        }
        let mut parts: Vec<Part> = names
            .iter()
            .map(|n| Part::parse(n.as_str()))
            .collect::<Result<_, _>>()?;
        parts.sort();
        parts.dedup();
        Ok(parts)
    }
}

/// Where a panel is in a layout: an area, floating, or a window of its
/// own; `None` when it is not shown.
fn place_of(layout: &Layout, panel: &PanelId) -> Option<Json> {
    if !layout.shows(panel) {
        return None;
    }
    if let Some(area) = layout.area_of(panel) {
        return Some(json!({ "area": area }));
    }
    if let Some(f) = layout.floating_of(panel) {
        return Some(json!({ "floating": f.pos }));
    }
    Some(json!({ "window": true }))
}

/// What a workspace or template is made of, part by part, as data.
pub fn parts(bundle: &Bundle) -> Json {
    let t = &bundle.template;
    let net = &t.network;
    let panels: Vec<Json> = bundle
        .layout
        .placed()
        .into_iter()
        .filter_map(|p| {
            place_of(&bundle.layout, p).map(|mut at| {
                at["panel"] = json!(p.as_str());
                at
            })
        })
        .collect();
    json!([
        {
            "part": "network",
            "sources": net.sources,
            "nodes": net.nodes.iter().map(|n| json!({
                "name": n.name,
                "module": n.module,
                "composite": n.composite.as_ref().map(ToString::to_string),
            })).collect::<Vec<_>>(),
            "view": format!("{}.{}", net.view.node, net.view.port),
        },
        { "part": "layout", "panels": panels },
        { "part": "keys", "bindings": t.tools.keymap },
        { "part": "toolbox", "tools": t.tools.toolbox, "default": t.tools.default },
        { "part": "library", "library": t.library },
    ])
}

/// Which side a conflict keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// What the workspace has.
    Ours,
    /// What the import brings.
    Theirs,
}

/// Something an import cannot decide alone: the user (or the agent)
/// picks a side.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Conflict {
    /// What a pick names it by: `view`, `panel:<id>`, `key:<keys>`
    /// (`key:<keys>@<when>` under a condition), `library`.
    pub id: String,
    pub part: Part,
    /// In words.
    pub what: String,
    pub ours: Json,
    pub theirs: Json,
    /// The side picked, if one was.
    pub pick: Option<Side>,
}

/// An import worked out: the state it makes, what it changes, and its
/// conflicts.
#[derive(Clone, Debug, PartialEq)]
pub struct Merged {
    pub state: State,
    /// Each change, as data (`part`, `change`, `what`).
    pub changes: Vec<Json>,
    pub conflicts: Vec<Conflict>,
    /// The imported network's new node name, by the source's name.
    pub renamed: BTreeMap<String, String>,
    /// What was left behind (presets, panel hints).
    pub notes: Vec<String>,
}

impl Merged {
    /// The conflicts no side was picked for: an import is applied only
    /// without any.
    pub fn unresolved(&self) -> Vec<&Conflict> {
        self.conflicts.iter().filter(|c| c.pick.is_none()).collect()
    }

    /// As an agent or a review dialog reads it.
    pub fn to_json(&self) -> Json {
        json!({
            "changes": self.changes,
            "conflicts": self.conflicts,
            "unresolved": self.unresolved().iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
            "renamed": self.renamed,
            "notes": self.notes,
        })
    }
}

/// A node name from an id (`helios.portrait-retouch` → `portrait_retouch`),
/// free in `taken`.
fn free_name(id: &str, taken: &BTreeSet<String>) -> String {
    let last = id.rsplit('.').next().unwrap_or(id);
    let mut base: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if !base.starts_with(|c: char| c.is_ascii_lowercase()) {
        base = format!("imported_{base}");
    }
    if !valid_name(&base) {
        base = "imported".into();
    }
    if !taken.contains(&base) {
        return base;
    }
    let mut n = 2;
    loop {
        let name = format!("{base}_{n}");
        if !taken.contains(&name) {
            return name;
        }
        n += 1;
    }
}

/// The nodes of a network in an order where each reads only sources and
/// nodes before it; a cycle is refused.
fn ordered(net: &NetworkDesc) -> Result<Vec<NodeDesc>, String> {
    let names: BTreeSet<&str> = net.nodes.iter().map(|n| n.name.as_str()).collect();
    let mut placed: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    let mut left: Vec<&NodeDesc> = net.nodes.iter().collect();
    while !left.is_empty() {
        let before = left.len();
        left.retain(|n| {
            let ready = net
                .connections
                .iter()
                .filter(|c| c.to == n.name && names.contains(c.from.as_str()))
                .all(|c| placed.contains(&c.from));
            if ready {
                placed.insert(n.name.clone());
                out.push((*n).clone());
            }
            !ready
        });
        if left.len() == before {
            return Err("the network has a cycle".into());
        }
    }
    Ok(out)
}

/// A network made a composite file: its sources the composite's inputs,
/// its view its output, its nodes inside in order.
fn network_composite(
    source: &Bundle,
    id: &str,
    version: u32,
    catalogue: &Catalogue,
) -> Result<CompositeFile, String> {
    let net = &source.template.network;
    let catalogue = source.catalogue(catalogue);
    let view = net
        .nodes
        .iter()
        .find(|n| n.name == net.view.node)
        .ok_or_else(|| format!("the network's view `{}` is not a node", net.view.node))?;
    let out = match (&view.module, &view.composite) {
        (Some(m), _) => catalogue.get_module(m).and_then(|m| {
            m.manifest()
                .outputs
                .iter()
                .find(|p| p.name == net.view.port)
                .and_then(|p| {
                    Some(Port {
                        name: "output".into(),
                        channels: p.channels()?,
                        layout: p.layout.clone(),
                        optional: false,
                    })
                })
        }),
        (None, Some(c)) => catalogue.get_composite(c).and_then(|f| {
            f.outputs
                .iter()
                .find(|p| p.name == net.view.port)
                .map(|p| Port {
                    name: "output".into(),
                    ..p.clone()
                })
        }),
        (None, None) => None,
    }
    .ok_or_else(|| {
        format!(
            "the network's view `{}.{}` is not an image output this Helios knows",
            net.view.node, net.view.port
        )
    })?;
    let sources: BTreeSet<&str> = net.sources.iter().map(String::as_str).collect();
    Ok(CompositeFile {
        format: FORMAT,
        id: id.into(),
        version,
        name: source.template.meta.name.clone(),
        description: source.template.meta.scenario.clone(),
        inputs: net
            .sources
            .iter()
            .map(|s| Port {
                name: s.clone(),
                channels: 4,
                layout: Some("rgba".into()),
                optional: false,
            })
            .collect(),
        outputs: vec![out],
        nodes: ordered(net)?
            .into_iter()
            .map(|n| InnerDesc {
                name: n.name,
                module: n.module,
                composite: n.composite,
                params: n.params,
            })
            .collect(),
        connections: net
            .connections
            .iter()
            .map(|c| {
                if sources.contains(c.from.as_str()) {
                    Connection {
                        from: PORTS.into(),
                        port: c.from.clone(),
                        to: c.to.clone(),
                        input: c.input.clone(),
                    }
                } else {
                    c.clone()
                }
            })
            .collect(),
        exits: vec![Exit {
            from: net.view.node.clone(),
            port: net.view.port.clone(),
            output: "output".into(),
        }],
        exposed: Vec::new(),
        panel: None,
    })
}

fn binding_id(b: &Binding) -> String {
    if b.when.is_empty() {
        format!("key:{}", b.keys)
    } else {
        format!("key:{}@{}", b.keys, b.when)
    }
}

/// Copies parts of `source` into `target`: the network as an opened
/// composite wired to the workspace's first image (its nodes renamed
/// under one free name), the panels it shows where it shows them, its key
/// bindings, its tools, its library settings. What both sides set
/// differently is a conflict, decided by `picks` (by conflict id); the
/// result says what is left undecided. Nothing records where the parts
/// came from.
pub fn import(
    target: &State,
    source: &Bundle,
    parts: &[Part],
    picks: &BTreeMap<String, Side>,
    catalogue: &Catalogue,
) -> Result<Merged, String> {
    let mut state = target.clone();
    let mut m = Merged {
        state: target.clone(),
        changes: Vec::new(),
        conflicts: Vec::new(),
        renamed: BTreeMap::new(),
        notes: Vec::new(),
    };
    let mut conflict = |id: String, part: Part, what: String, ours: Json, theirs: Json| {
        let pick = picks.get(&id).copied();
        m.conflicts.push(Conflict {
            id,
            part,
            what,
            ours,
            theirs,
            pick,
        });
        pick
    };
    let mut changes = Vec::new();
    let mut notes = Vec::new();
    let mut renamed = BTreeMap::new();
    let t = &mut state.bundle.template;
    if parts.contains(&Part::Network) {
        let src = &source.template;
        let first = t
            .network
            .sources
            .first()
            .cloned()
            .ok_or("the workspace has no image to wire the network to")?;
        let mut taken: BTreeSet<String> = t.network.sources.iter().cloned().collect();
        taken.extend(t.network.nodes.iter().map(|n| n.name.clone()));
        let name = free_name(&src.id, &taken);
        // The composite's id, at a version no composite of the workspace
        // has yet.
        let id = format!("import.{}", slug(&src.id).to_ascii_lowercase());
        let id = if valid_id(&id) {
            id
        } else {
            "import.network".into()
        };
        let version = state
            .bundle
            .composites
            .iter()
            .filter(|c| c.id == id)
            .map(|c| c.version)
            .max()
            .map_or(1, |v| v + 1);
        let file = network_composite(source, &id, version, catalogue)?;
        let reference = file.reference();
        let inputs: Vec<String> = file.inputs.iter().map(|p| p.name.clone()).collect();
        // The composites its nodes use come along.
        let mut with = source.clone();
        with.bundle_composites(catalogue)?;
        for c in with.composites {
            if !state
                .bundle
                .composites
                .iter()
                .any(|x| x.reference() == c.reference())
            {
                state.bundle.composites.push(c);
            }
        }
        state.bundle.composites.push(file);
        state
            .bundle
            .composites
            .sort_by_key(CompositeFile::reference);
        let t = &mut state.bundle.template;
        t.network.nodes.push(NodeDesc {
            name: name.clone(),
            composite: Some(reference.clone()),
            open: true,
            ..NodeDesc::default()
        });
        for input in &inputs {
            t.network.connections.push(Connection {
                from: first.clone(),
                port: crate::build::SOURCE_PORT.into(),
                to: name.clone(),
                input: input.clone(),
            });
        }
        for n in &src.network.nodes {
            renamed.insert(n.name.clone(), format!("{name}.{}", n.name));
        }
        // Its sub-workspaces come along as copies, each under a new id.
        let copied = t.copy_subworkspaces(src, |n| format!("{name}.{n}"));
        for (from, id) in &copied {
            let s = t.subworkspace(id).expect("just copied");
            changes.push(json!({
                "part": "network",
                "change": "delegated",
                "what": id,
                "name": s.name,
                "nodes": s.nodes,
                "copy_of": from,
            }));
        }
        changes.push(json!({
            "part": "network",
            "change": "added",
            "what": name,
            "composite": reference.to_string(),
            "nodes": src.network.nodes.iter().map(|n| format!("{name}.{}", n.name)).collect::<Vec<_>>(),
        }));
        if !src.network.presets.is_empty() {
            notes.push("the network's presets are not imported".to_string());
        }
        if src.network.nodes.iter().any(|n| n.panel.is_some()) {
            notes.push("the network's panel hints are not imported".to_string());
        }
        let theirs = Endpoint {
            node: name.clone(),
            port: "output".into(),
        };
        let pick = conflict(
            "view".into(),
            Part::Network,
            "which output the viewport shows".into(),
            json!(format!("{}.{}", t.network.view.node, t.network.view.port)),
            json!(format!("{}.{}", theirs.node, theirs.port)),
        );
        if pick == Some(Side::Theirs) {
            t.network.view = theirs;
            changes.push(
                json!({ "part": "network", "change": "view", "what": format!("{name}.output") }),
            );
        }
    }
    let t = &mut state.bundle.template;
    if parts.contains(&Part::Keys) {
        for b in &source.template.tools.keymap {
            match t
                .tools
                .keymap
                .iter_mut()
                // A removal (`-<command>`, no keys) meets only itself.
                .find(|o| {
                    o.keys == b.keys
                        && o.when == b.when
                        && (!b.keys.is_empty() || o.command == b.command)
                }) {
                Some(o) if o.command == b.command => {}
                Some(o) => {
                    let pick = conflict(
                        binding_id(b),
                        Part::Keys,
                        format!("{} is bound twice", b.keys),
                        json!(o.command),
                        json!(b.command),
                    );
                    if pick == Some(Side::Theirs) {
                        o.command.clone_from(&b.command);
                        changes.push(json!({ "part": "keys", "change": "rebound", "what": b.keys, "command": b.command }));
                    }
                }
                None => {
                    t.tools.keymap.push(b.clone());
                    changes.push(json!({ "part": "keys", "change": "added", "what": b.keys, "command": b.command }));
                }
            }
        }
    }
    if parts.contains(&Part::Toolbox) {
        for tool in &source.template.tools.toolbox {
            if !t.tools.toolbox.contains(tool) {
                t.tools.toolbox.push(tool.clone());
                changes.push(json!({ "part": "toolbox", "change": "added", "what": tool }));
            }
        }
        if t.tools.default.is_none() && source.template.tools.default.is_some() {
            t.tools.default.clone_from(&source.template.tools.default);
        }
    }
    if parts.contains(&Part::Library)
        && let Some(theirs) = &source.template.library
    {
        match &t.library {
            None => {
                t.library = Some(theirs.clone());
                changes.push(json!({ "part": "library", "change": "added", "what": "library" }));
            }
            Some(ours) if ours == theirs => {}
            Some(ours) => {
                let pick = conflict(
                    "library".into(),
                    Part::Library,
                    "the library is set on both sides".into(),
                    json!(ours),
                    json!(theirs),
                );
                if pick == Some(Side::Theirs) {
                    t.library = Some(theirs.clone());
                    changes.push(
                        json!({ "part": "library", "change": "replaced", "what": "library" }),
                    );
                }
            }
        }
    }
    if parts.contains(&Part::Layout) {
        let layout = &mut state.bundle.layout;
        for panel in source.layout.placed() {
            let Some(theirs) = place_of(&source.layout, panel) else {
                continue;
            };
            let ours = place_of(layout, panel);
            if ours.as_ref() == Some(&theirs) {
                continue;
            }
            if let Some(ours) = ours {
                // One instance of a panel: it stays or it moves.
                let pick = conflict(
                    format!("panel:{panel}"),
                    Part::Layout,
                    format!("{panel} is shown on both sides"),
                    ours,
                    theirs.clone(),
                );
                if pick != Some(Side::Theirs) {
                    continue;
                }
            }
            if let Some(area) = source.layout.area_of(panel) {
                layout.place(panel, area);
            } else if let Some(f) = source.layout.floating_of(panel) {
                layout.float(panel, f.pos);
            } else {
                notes.push(format!(
                    "{panel} is in a window of its own there: not moved"
                ));
                continue;
            }
            changes.push(json!({ "part": "layout", "change": "placed", "what": panel.as_str(), "at": theirs }));
        }
    }
    m.state = state;
    m.changes = changes;
    m.notes = notes;
    m.renamed = renamed;
    Ok(m)
}

/// What "Save as template" is asked: the template's identity and text,
/// the parts that go in, the values asked on create.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extract {
    pub id: String,
    #[serde(default = "one")]
    pub version: u32,
    pub name: String,
    /// The scenario it is made for.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: Option<String>,
    /// The name and description in other languages, by locale.
    #[serde(default)]
    pub translations: BTreeMap<String, Translation>,
    /// The parts left out (keys, toolbox, library); the network and the
    /// layout always go in.
    #[serde(default)]
    pub without: Vec<Part>,
    /// The candidates made parameters, by id; every one when absent.
    #[serde(default)]
    pub ask: Option<Vec<String>>,
}

fn one() -> u32 {
    1
}

/// A template's name and scenario in one language.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Translation {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// Something a template never carries, and where it was.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Stripped {
    /// `images`, `history`, `path`, `credential` or `data`.
    pub what: String,
    /// Where it was, as a JSON pointer into `template.json` (empty for
    /// what is never in a workspace's files).
    pub at: String,
    pub detail: String,
}

/// "Save as template" worked out, before anything is written.
#[derive(Clone, Debug, PartialEq)]
pub struct Review {
    /// What goes in, part by part ([`parts`]).
    pub included: Json,
    pub stripped: Vec<Stripped>,
    /// What could be asked on create.
    pub candidates: Vec<Parameter>,
    /// The bundle it writes.
    pub bundle: Bundle,
    /// Why it cannot be written yet.
    pub problems: Vec<String>,
}

impl Review {
    pub fn to_json(&self) -> Json {
        json!({
            "id": self.bundle.template.id,
            "version": self.bundle.template.version,
            "included": self.included,
            "stripped": self.stripped,
            "candidates": self.candidates,
            "parameters": self.bundle.template.parameters,
            "template": self.bundle.template,
            "problems": self.problems,
            "ok": self.problems.is_empty(),
        })
    }
}

/// Whether a text looks like an absolute path on some machine.
fn absolute(text: &str) -> bool {
    let b = text.as_bytes();
    text.starts_with('/')
        || text.starts_with("~/")
        || text.starts_with("\\\\")
        || text.starts_with("file:")
        || (b.len() > 2
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'\\' | b'/'))
}

/// Whether a parameter's name says it holds a secret.
fn credential(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "password",
        "passwd",
        "secret",
        "token",
        "api_key",
        "apikey",
        "credential",
    ]
    .iter()
    .any(|w| n.contains(w))
}

/// Text longer than this is data (an image, a mask), never a setting.
pub const DATA_LENGTH: usize = 256;

/// A Fluent message line: the text on one line, its braces escaped.
fn fluent(key: &str, text: &str) -> String {
    let mut v = String::new();
    for c in text.chars() {
        match c {
            '{' => v.push_str("{\"{\"}"),
            '}' => v.push_str("{\"}\"}"),
            '\n' | '\r' => v.push(' '),
            c => v.push(c),
        }
    }
    format!("{key} = {}\n", v.trim())
}

/// Strips from a value leaving for another application (the system
/// clipboard) what "Save as template" strips from a template's
/// parameters: an absolute path becomes its file name, a value under a
/// key naming a secret and text longer than [`DATA_LENGTH`] are emptied.
/// Answers what went, one entry each ("a local path", "a secret",
/// "embedded data").
pub fn strip_for_outside(json: &mut Json) -> Vec<String> {
    let mut out = Vec::new();
    strip_value(json, None, &mut out);
    out
}

fn strip_value(json: &mut Json, key: Option<&str>, out: &mut Vec<String>) {
    match json {
        Json::String(text) if key.is_some_and(credential) && !text.is_empty() => {
            text.clear();
            out.push("a secret".into());
        }
        Json::String(text) if absolute(text) => {
            let name = text
                .trim_end_matches(['/', '\\'])
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or_default()
                .to_string();
            *text = name;
            out.push("a local path".into());
        }
        Json::String(text) if text.len() > DATA_LENGTH => {
            text.clear();
            out.push("embedded data".into());
        }
        Json::Array(items) => {
            for v in items {
                strip_value(v, key, out);
            }
        }
        Json::Object(map) => {
            for (k, v) in map.iter_mut() {
                strip_value(v, Some(k), out);
            }
        }
        _ => {}
    }
}

/// Works out "Save as template" for a workspace as it stands: its
/// network, layout and the parts not left out go in, under the new id
/// and text; images and edit history never are in a workspace's files;
/// absolute paths, secrets and embedded data are stripped from the
/// parameters; the import source and the paths become parameters asked on
/// create. The bundle is checked against `catalogue`.
pub fn review(state: &State, x: &Extract, catalogue: &Catalogue) -> Review {
    let mut problems = Vec::new();
    let mut stripped = vec![
        Stripped {
            what: "images".into(),
            at: String::new(),
            detail: "the library's images stay in the vault".into(),
        },
        Stripped {
            what: "history".into(),
            at: String::new(),
            detail: "each image's edit history stays in the vault".into(),
        },
    ];
    let mut t = state.bundle.template.clone();
    let old_slug = slug(&t.id);
    t.id.clone_from(&x.id);
    t.version = x.version;
    t.meta = crate::format::Meta {
        name: x.name.trim().into(),
        scenario: x.description.trim().into(),
        preview: None,
        icon: x.icon.clone().filter(|i| !i.trim().is_empty()),
        tags: t.meta.tags.clone(),
    };
    t.parameters.clear();
    // A new template starts its own ids afresh.
    t.retired_subworkspaces.clear();
    if x.without.contains(&Part::Network) || x.without.contains(&Part::Layout) {
        problems.push("the network and the layout always go in a template".into());
    }
    if x.without.contains(&Part::Keys) {
        t.tools.keymap.clear();
    }
    if x.without.contains(&Part::Toolbox) {
        t.tools.toolbox.clear();
        t.tools.default = None;
    }
    if x.without.contains(&Part::Library) {
        t.library = None;
    }
    if crate::shipped::templates()
        .iter()
        .any(|b| b.template.id == x.id)
    {
        problems.push(format!("id: `{}` is a built-in template's", x.id));
    }
    // The parameters' values: paths become parameters, secrets and data
    // go.
    let mut candidates = Vec::new();
    let mut json = serde_json::to_value(&t).unwrap_or_default();
    let mut walk = |base: &str, node: &str, values: &mut Json| {
        let Some(values) = values.as_object_mut() else {
            return;
        };
        for (param, value) in values.iter_mut() {
            let Some(text) = value.get("text").and_then(Json::as_str).map(String::from) else {
                continue;
            };
            if text.is_empty() {
                continue;
            }
            let at = format!("{base}/{param}/text");
            let (what, detail) = if credential(param) {
                ("credential", format!("{node}: {param}, a secret"))
            } else if absolute(&text) {
                candidates.push(Parameter {
                    id: format!("{node}_{param}")
                        .chars()
                        .map(|c| {
                            if c.is_ascii_alphanumeric() {
                                c.to_ascii_lowercase()
                            } else {
                                '_'
                            }
                        })
                        .collect(),
                    name: format!("{node}: {param}"),
                    at: at.clone(),
                    kind: ParameterKind::Path,
                    choices: Vec::new(),
                });
                ("path", format!("{node}: {param}, {text}"))
            } else if text.len() > DATA_LENGTH {
                ("data", format!("{node}: {param}, {} bytes", text.len()))
            } else {
                continue;
            };
            value["text"] = json!("");
            stripped.push(Stripped {
                what: what.into(),
                at,
                detail,
            });
        }
    };
    if let Some(nodes) = json
        .pointer_mut("/network/nodes")
        .and_then(Json::as_array_mut)
    {
        for (i, n) in nodes.iter_mut().enumerate() {
            let name = n["name"].as_str().unwrap_or_default().to_string();
            if let Some(params) = n.get_mut("params") {
                walk(&format!("/network/nodes/{i}/params"), &name, params);
            }
        }
    }
    if let Some(presets) = json
        .pointer_mut("/network/presets")
        .and_then(Json::as_array_mut)
    {
        for (i, p) in presets.iter_mut().enumerate() {
            if let Some(values) = p.get_mut("values").and_then(Json::as_object_mut) {
                for (node, params) in values.iter_mut() {
                    walk(&format!("/network/presets/{i}/values/{node}"), node, params);
                }
            }
        }
    }
    // An absolute path anywhere else is not the template's to keep.
    let mut elsewhere = Vec::new();
    find_paths(&json, "", &mut elsewhere);
    for at in elsewhere {
        problems.push(format!("{at}: an absolute path; take it out first"));
    }
    let mut t: Template = match serde_json::from_value(json) {
        Ok(t) => t,
        Err(e) => {
            problems.push(format!("template: {e}"));
            t
        }
    };
    if t.library.as_ref().and_then(|l| l.import.as_ref()).is_some() {
        candidates.push(Parameter {
            id: "import_source".into(),
            name: "Import source".into(),
            at: "/library/import/source".into(),
            kind: ParameterKind::Choice,
            choices: crate::format::IMPORT_SOURCES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        });
    }
    t.parameters = candidates
        .iter()
        .filter(|c| x.ask.as_ref().is_none_or(|ask| ask.contains(&c.id)))
        .cloned()
        .collect();
    if let Some(ask) = &x.ask {
        for id in ask {
            if !candidates.iter().any(|c| c.id == *id) {
                problems.push(format!("ask: `{id}` is not a value that can be asked"));
            }
        }
    }
    // Its text: English, the translations, and the old template's other
    // messages (its presets') under the new id.
    let new_slug = slug(&t.id);
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    let mut en = fluent(&format!("{new_slug}-name"), &t.meta.name);
    en.push_str(&fluent(&format!("{new_slug}-scenario"), &t.meta.scenario));
    for q in &t.parameters {
        en.push_str(&fluent(
            &format!("{new_slug}-parameter-{}", slug(&q.id)),
            &q.name,
        ));
    }
    texts.insert("en".into(), en);
    for (locale, tr) in &x.translations {
        let mut text = fluent(&format!("{new_slug}-name"), &tr.name);
        if !tr.description.trim().is_empty() {
            text.push_str(&fluent(&format!("{new_slug}-scenario"), &tr.description));
        }
        texts.insert(locale.clone(), text);
    }
    for old in &state.bundle.texts {
        for line in old.source.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let k = k.trim();
            let Some(rest) = k.strip_prefix(&format!("{old_slug}-")) else {
                continue;
            };
            if rest == "name" || rest == "scenario" {
                continue;
            }
            let text = texts.entry(old.locale.clone()).or_default();
            let _ = writeln!(text, "{new_slug}-{rest} = {}", v.trim());
        }
    }
    // Panels egui-pop declares itself (`egui_pop.*`, the jobs centre) are
    // the application's, never a template's.
    let mut layout = state.bundle.layout.clone();
    let foreign: Vec<PanelId> = layout
        .placed()
        .into_iter()
        .filter(|p| p.as_str().starts_with("egui_pop."))
        .cloned()
        .collect();
    for p in &foreign {
        layout.remove(p);
    }
    layout
        .hidden
        .retain(|p| !p.as_str().starts_with("egui_pop."));
    let mut bundle = Bundle::new(t, layout);
    bundle.composites.clone_from(&state.bundle.composites);
    bundle.texts = texts
        .into_iter()
        .map(|(locale, source)| Text {
            locale,
            name: "template.ftl".into(),
            source,
        })
        .collect();
    if let Err(e) = bundle.bundle_composites(catalogue) {
        problems.push(e);
    }
    if x.name.trim().is_empty() {
        problems.push("name: empty".into());
    }
    if x.description.trim().is_empty() {
        problems.push("description: empty".into());
    }
    problems.extend(bundle.validate(catalogue).problems);
    problems.dedup();
    Review {
        included: parts(&bundle),
        stripped,
        candidates,
        bundle,
        problems,
    }
}

/// The JSON pointers of the absolute paths in a value.
fn find_paths(json: &Json, at: &str, out: &mut Vec<String>) {
    match json {
        Json::String(s) if absolute(s) => out.push(at.to_string()),
        Json::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                find_paths(v, &format!("{at}/{i}"), out);
            }
        }
        Json::Object(map) => {
            for (k, v) in map {
                find_paths(v, &format!("{at}/{k}"), out);
            }
        }
        _ => {}
    }
}

/// Where a user template is written: `<config>/templates/<id>.helios-template`.
pub fn template_dir(config: &Path, id: &str) -> PathBuf {
    config.join(TEMPLATES_DIR).join(format!("{id}{EXTENSION}"))
}

/// Writes a reviewed template where the user's templates are. Refused
/// while the review has a problem, and over a template of the same id
/// unless its version is newer (or `overwrite`).
pub fn save_template(review: &Review, config: &Path, overwrite: bool) -> Result<PathBuf, String> {
    if !review.problems.is_empty() {
        return Err(review.problems.join("; "));
    }
    let t = &review.bundle.template;
    let dir = template_dir(config, &t.id);
    if dir.is_dir() && !overwrite {
        let there = Bundle::load(&dir).map_or(0, |b| b.template.version);
        if there >= t.version {
            return Err(format!(
                "a template `{}` at version {there} is there: give a newer version, or overwrite it",
                t.id
            ));
        }
    }
    let next = dir.with_extension("next");
    let _ = std::fs::remove_dir_all(&next);
    review.bundle.save(&next)?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&next, &dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, text))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// What an agent does to a workspace's files (the node's MCP tools):
/// each call takes the configuration directory and its arguments as
/// JSON, and answers JSON. A step that changes the workspace is kept to
/// be undone (`undo`); with `dry_run` nothing is written.
pub mod tools {
    use super::{
        BTreeMap, Catalogue, Extract, Files, Json, Part, Path, PathBuf, Side, import, json, parts,
        review, save_template, source,
    };

    fn text<'a>(args: &'a Json, key: &str) -> Result<&'a str, String> {
        args[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("`{key}` is missing"))
    }

    fn files(config: &Path, args: &Json) -> Result<Files, String> {
        let dir = PathBuf::from(text(args, "workspace")?);
        if !dir.is_absolute() {
            return Err("`workspace` is an absolute path".into());
        }
        Ok(Files::new(config, &dir))
    }

    fn dry(args: &Json) -> bool {
        args["dry_run"].as_bool().unwrap_or(false)
    }

    /// `{workspace, source?}`: the parts of a workspace, or of the
    /// source given (a workspace, a template's directory or id).
    pub fn parts_of(config: &Path, args: &Json) -> Result<Json, String> {
        let bundle = match args["source"].as_str().filter(|s| !s.is_empty()) {
            Some(s) => source(config, s)?,
            None => files(config, args)?.load()?.bundle,
        };
        Ok(json!({ "id": bundle.template.id, "parts": parts(&bundle) }))
    }

    /// `{workspace}`: the workspace as it stands, whole.
    pub fn state(config: &Path, args: &Json) -> Result<Json, String> {
        Ok(files(config, args)?.load()?.to_json())
    }

    /// `{workspace, source, parts?, picks?, dry_run}`: parts of the
    /// source copied into the workspace; refused while a conflict has no
    /// pick (a dry run lists them).
    pub fn import_parts(config: &Path, args: &Json, catalogue: &Catalogue) -> Result<Json, String> {
        let files = files(config, args)?;
        let before = files.load()?;
        let from = source(config, text(args, "source")?)?;
        let names: Vec<String> = serde_json::from_value(args["parts"].clone()).unwrap_or_default();
        let parts = Part::list(&names)?;
        let picks: BTreeMap<String, Side> = if args["picks"].is_null() {
            BTreeMap::new()
        } else {
            serde_json::from_value(args["picks"].clone()).map_err(|e| format!("picks: {e}"))?
        };
        let merged = import(&before, &from, &parts, &picks, catalogue)?;
        let mut answer = merged.to_json();
        let problems = merged.state.problems(catalogue);
        answer["problems"] = json!(problems);
        answer["dry_run"] = json!(dry(args));
        if dry(args) {
            return Ok(answer);
        }
        let unresolved = merged.unresolved();
        if !unresolved.is_empty() {
            let ids: Vec<&str> = unresolved.iter().map(|c| c.id.as_str()).collect();
            return Err(format!(
                "pick a side for each conflict first: {}",
                ids.join(", ")
            ));
        }
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        files.save_step(&before, &merged.state, "import parts")?;
        Ok(answer)
    }

    /// `workspace`, `id`, `name`, `description`, and optionally `icon`,
    /// `translations`, `without`, `ask`, `version`, `overwrite`,
    /// `dry_run`: the review, and the template written unless it is a dry
    /// run.
    pub fn save_as_template(
        config: &Path,
        args: &Json,
        catalogue: &Catalogue,
    ) -> Result<Json, String> {
        let state = files(config, args)?.load()?;
        let mut fields = args.clone();
        if let Some(o) = fields.as_object_mut() {
            for k in ["workspace", "dry_run", "overwrite", "config"] {
                o.remove(k);
            }
            o.retain(|_, v| !v.is_null());
        }
        let x: Extract = serde_json::from_value(fields).map_err(|e| e.to_string())?;
        let r = review(&state, &x, catalogue);
        let mut answer = r.to_json();
        answer["dry_run"] = json!(dry(args));
        if !dry(args) {
            let overwrite = args["overwrite"].as_bool().unwrap_or(false);
            let dir = save_template(&r, config, overwrite)?;
            answer["written"] = json!(dir);
        }
        Ok(answer)
    }

    /// `{workspace, dry_run}`: back to its template, as one step.
    pub fn reset(config: &Path, args: &Json, catalogue: &Catalogue) -> Result<Json, String> {
        let files = files(config, args)?;
        let before = files.load()?;
        let origin = files
            .origin()
            .ok_or("the workspace was not made from a template")??;
        let next = before.reset(&origin, catalogue)?;
        let answer = json!({
            "template": origin.template.id,
            "version": origin.template.version,
            "changed": before != next,
            "dry_run": dry(args),
        });
        if !dry(args) {
            files.save_step(&before, &next, "reset to template")?;
        }
        Ok(answer)
    }

    /// `{workspace, changes, dry_run}`: its name, description, icon,
    /// network, tools, library or layout changed, as one step; checked
    /// first.
    pub fn edit(config: &Path, args: &Json, catalogue: &Catalogue) -> Result<Json, String> {
        let files = files(config, args)?;
        let before = files.load()?;
        let next = before.edited(&args["changes"])?;
        let problems = next.problems(catalogue);
        let answer = json!({ "problems": problems, "dry_run": dry(args) });
        if dry(args) {
            return Ok(answer);
        }
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        files.save_step(&before, &next, "edit workspace")?;
        Ok(answer)
    }

    /// `{workspace, name, nodes, dry_run}`: a new sub-workspace over
    /// nodes of the network, as one step; checked first. The answer says
    /// its id, its parent and the sub-workspaces moved inside it.
    pub fn delegate(config: &Path, args: &Json, catalogue: &Catalogue) -> Result<Json, String> {
        let files = files(config, args)?;
        let before = files.load()?;
        let nodes: Vec<String> =
            serde_json::from_value(args["nodes"].clone()).map_err(|e| format!("nodes: {e}"))?;
        let (next, answer) = before.delegated(text(args, "name")?, &nodes)?;
        step(
            &files,
            &before,
            &next,
            answer,
            args,
            catalogue,
            "delegate nodes",
        )
    }

    /// `{workspace, id, dry_run}`: a sub-workspace deleted (its nodes
    /// stay in the network, its id is retired), as one step.
    pub fn delete_subworkspace(
        config: &Path,
        args: &Json,
        catalogue: &Catalogue,
    ) -> Result<Json, String> {
        let files = files(config, args)?;
        let before = files.load()?;
        let (next, answer) = before.subworkspace_deleted(text(args, "id")?)?;
        step(
            &files,
            &before,
            &next,
            answer,
            args,
            catalogue,
            "delete a sub-workspace",
        )
    }

    /// Answers a step worked out (with its `problems` and `dry_run`),
    /// written unless it is a dry run; refused with problems.
    fn step(
        files: &Files,
        before: &super::State,
        next: &super::State,
        mut answer: Json,
        args: &Json,
        catalogue: &Catalogue,
        label: &str,
    ) -> Result<Json, String> {
        let problems = next.problems(catalogue);
        answer["problems"] = json!(problems);
        answer["dry_run"] = json!(dry(args));
        if dry(args) {
            return Ok(answer);
        }
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        files.save_step(before, next, label)?;
        Ok(answer)
    }

    /// `{workspace}`: the last step an agent made undone.
    pub fn undo(config: &Path, args: &Json) -> Result<Json, String> {
        let label = files(config, args)?.undo()?;
        Ok(json!({ "undone": label }))
    }
}
