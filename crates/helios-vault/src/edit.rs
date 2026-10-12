//! An image's edit, saved step by step as versions.
//!
//! A surface (the desktop, a node driven by an agent) that edits an image
//! holds an [`EditSession`]: on open it continues the image's head, and
//! each committed step (the end of a gesture, an undo, a redo, a parameter
//! set by an agent, a decision answered) is saved at once as a version
//! continuing the previous one, in one transaction. A crash or a `kill -9`
//! loses at most the step being made.
//!
//! **Which head.** Versions form a graph; two surfaces editing at once
//! make two heads, and neither overwrites the other. A session opens the
//! newest head (the latest `at`, then the longest history, then the
//! greatest id) and from then on continues the version it saved last, not
//! whatever another writer added meanwhile: concurrent edits become two
//! heads, as versions intend. Its history is the line of first parents
//! back from the head ([`Vault::history`]).
//!
//! **The edit.** A version's edit is a network of named parameters:
//!
//! ```json
//! { "modules": [{ "name": "exp", "module": "exposure", "params": { "exposure": { "float": 1.0 } } }],
//!   "decisions": { … } }
//! ```
//!
//! The network's shape (its sources and connections) belongs to the
//! surface; a version carries the parameters of its modules by name, and
//! the decision book when there is one. An imported sidecar's version
//! also carries its connections and its import report.
//!
//! Every save is announced on the vault's bus as `edit.saved`, with the
//! session's step number: a test waits for that event, it never sleeps.

use std::collections::BTreeMap;

use helios_core::{Event, Graph, ModuleKind, Params};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{ImageId, Store, Vault, Version, VersionId, versions};

/// The parameters of a graph's modules, by node name, with each node's
/// module: what a version keeps of a surface's network. Sources and
/// decision nodes are left out (a decision's answer is in its book).
pub fn of_graph(graph: &Graph) -> Value {
    let mut modules = BTreeMap::new();
    for id in graph.ids() {
        let Ok(Some(manifest)) = graph.manifest(id) else {
            continue;
        };
        if manifest.kind == ModuleKind::Decision {
            continue;
        }
        let (Ok(name), Ok(params)) = (graph.name(id), graph.params(id)) else {
            continue;
        };
        modules.insert(
            name.to_string(),
            (manifest.name.clone(), params.unwrap_or_default()),
        );
    }
    from_modules(&modules)
}

/// An edit from each node's module and parameters, by node name: the
/// same edit whatever order the nodes come in.
pub fn from_modules(modules: &BTreeMap<String, (String, Params)>) -> Value {
    let modules: Vec<Value> = modules
        .iter()
        .map(|(name, (module, params))| json!({ "name": name, "module": module, "params": params }))
        .collect();
    json!({ "modules": modules })
}

/// The parameters an edit gives each module, by node name.
pub fn modules(edit: &Value) -> Result<BTreeMap<String, (Option<String>, Params)>, String> {
    let mut out = BTreeMap::new();
    for m in edit["modules"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let name = m["name"]
            .as_str()
            .ok_or_else(|| format!("a module of the edit has no name: {m}"))?;
        let params: Params = if m["params"].is_null() {
            Params::new()
        } else {
            serde_json::from_value(m["params"].clone())
                .map_err(|e| format!("module `{name}`: {e}"))?
        };
        out.insert(
            name.to_string(),
            (m["module"].as_str().map(String::from), params),
        );
    }
    Ok(out)
}

/// Sets a graph's parameters to an edit's, by node name; returns what was
/// left out (a node the graph lacks, or runs another module in).
pub fn apply_to_graph(graph: &mut Graph, edit: &Value) -> Result<Vec<String>, String> {
    let mut skipped = Vec::new();
    for (name, (module, params)) in modules(edit)? {
        let Some(id) = graph.find(&name) else {
            skipped.push(format!("`{name}`: no such node"));
            continue;
        };
        let running = graph
            .manifest(id)
            .map_err(|e| e.to_string())?
            .map(|m| m.name);
        if module.is_some() && running != module {
            skipped.push(format!(
                "`{name}`: runs {}, the edit has {}",
                running.unwrap_or_default(),
                module.unwrap_or_default()
            ));
            continue;
        }
        for (param, value) in params.iter() {
            graph
                .set_param(id, param, value.clone())
                .map_err(|e| format!("`{name}.{param}`: {e}"))?;
        }
    }
    Ok(skipped)
}

/// An edit step was saved as a version.
#[derive(Clone, Debug, Serialize)]
pub struct EditSaved {
    pub image: String,
    pub version: String,
    /// The session's step number: 1 for its first save.
    pub step: u64,
    pub label: String,
    pub origin: String,
    /// Saved while a gesture is still going (a drag), not at its end.
    pub in_progress: bool,
}

impl Event for EditSaved {
    const KIND: &'static str = "edit.saved";
}

/// A surface's edit of one image, saved as versions.
#[derive(Clone, Debug)]
pub struct EditSession {
    image: ImageId,
    origin: String,
    head: Option<VersionId>,
    /// The edit of `head`, to skip saving the same edit twice.
    saved: Option<Value>,
    /// The head is a save made during a gesture.
    in_progress: bool,
    steps: u64,
}

impl EditSession {
    /// Opens an image's edit as `origin` (`user`, `control`…): the
    /// session continues the image's newest head, returned with it (none
    /// for an image never edited).
    pub fn open<S: Store>(
        vault: &Vault<S>,
        image: ImageId,
        origin: &str,
    ) -> Result<(Self, Option<Version>), String> {
        if vault.image(&image)?.is_none() {
            return Err(format!("no image {}", image.hex()));
        }
        let all = vault.versions(&image)?;
        // The newest head: the latest time, then (steps come faster than
        // seconds) the longest history, then the greatest id.
        let mut head = None;
        let mut best = None;
        for v in versions::heads(&all) {
            let key = (v.at, vault.history(&v.id)?.len(), v.id);
            if best.as_ref().is_none_or(|b| key > *b) {
                best = Some(key);
                head = Some(v.clone());
            }
        }
        Ok((
            Self {
                image,
                origin: origin.into(),
                head: head.as_ref().map(|v| v.id),
                saved: head.as_ref().map(|v| v.edit.clone()),
                in_progress: false,
                steps: 0,
            },
            head,
        ))
    }

    pub fn image(&self) -> ImageId {
        self.image
    }

    /// The version this session continues.
    pub fn head(&self) -> Option<VersionId> {
        self.head
    }

    /// How many versions this session saved.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// Saves a committed step: a version continuing the head, unless the
    /// edit is the head's already (a step that changed nothing). A step
    /// ending a gesture already saved in progress is saved even when equal,
    /// so the history names the finished gesture.
    pub fn save<S: Store>(
        &mut self,
        vault: &Vault<S>,
        edit: Value,
        label: &str,
        now: u64,
    ) -> Result<Option<Version>, String> {
        let force = self.in_progress;
        self.write(vault, edit, label, now, force, false)
    }

    /// Saves a gesture still in progress (a drag), so a crash during a
    /// long gesture loses at most what changed since: nothing when the edit
    /// is the head's already.
    pub fn save_in_progress<S: Store>(
        &mut self,
        vault: &Vault<S>,
        edit: Value,
        label: &str,
        now: u64,
    ) -> Result<Option<Version>, String> {
        self.write(
            vault,
            edit,
            &format!("{label} (in progress)"),
            now,
            false,
            true,
        )
    }

    fn write<S: Store>(
        &mut self,
        vault: &Vault<S>,
        edit: Value,
        label: &str,
        now: u64,
        force: bool,
        in_progress: bool,
    ) -> Result<Option<Version>, String> {
        if !force && self.saved.as_ref() == Some(&edit) {
            return Ok(None);
        }
        let version = Version::new(
            self.image,
            self.head.into_iter().collect(),
            edit,
            &self.origin,
            now,
        )
        .labelled(label);
        vault.add_version(&version)?;
        self.head = Some(version.id);
        self.saved = Some(version.edit.clone());
        self.in_progress = in_progress;
        self.steps += 1;
        if let Some(bus) = &vault.bus {
            bus.publish(
                "vault",
                None,
                &EditSaved {
                    image: self.image.hex(),
                    version: version.id.hex(),
                    step: self.steps,
                    label: label.into(),
                    origin: self.origin.clone(),
                    in_progress,
                },
            );
        }
        Ok(Some(version))
    }
}
