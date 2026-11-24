//! What agents and scenarios read of Helios's state, as debug collectors:
//! a scenario checks `Expect(Value("helios.values.node/exposure/exposure",
//! 0.5))` instead of reading a command's result off the journal.
//!
//! - `helios.library`: the images the views show (`count`, `picked`,
//!   `rejected`), the selection (`selected`, `selection`), and the image
//!   under the cursor (`current`: its `id`, `name` and `path`), with the
//!   size of the image open in the viewport (`open_size`).
//! - `helios.values`: the image's parameters, path to value
//!   (`node/exposure/exposure`), what `param.get` reads.
//! - `controls`: every node parameter and the control that edits it
//!   (`helios_controls::commands::instances`).
//! - `helios.jobs`: the node's promises (every job, as the Activity focus
//!   lists them) counted by state (`running`, `finished`, `failed`,
//!   `cancelled`, `total`, `idle`), and each one's `id` (the promise's),
//!   `job` (the job it mirrors, when it does), `kind`, `title` and
//!   `state` (`list`).
//! - `helios.cache`: the cache service's tiers and kinds
//!   (`crate::caches::collector`).
//!
//! The library and the viewport exist once the shell is assembled, after
//! the collectors are declared: they are handed over through [`Late`].

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::{Ctx, Plugin, PluginError};
use egui_pop::core::debug::FnCollector;
use egui_pop::model::ids::PluginId;
use serde_json::{Map, Value as Json, json};

use crate::library::Library;
use crate::viewport::Viewport;

/// The plugin's id.
pub const PLUGIN: PluginId = PluginId::from_static("helios.agent_state");

/// What the shell hands over once assembled.
#[derive(Clone, Default)]
pub struct Late(Rc<RefCell<Option<Sources>>>);

impl Late {
    /// The shell's library, once assembled.
    pub fn library(&self) -> Option<Rc<RefCell<Library>>> {
        self.0.borrow().as_ref().map(|s| s.library.clone())
    }

    /// The shell's library and viewport, from now on.
    pub fn set(
        &self,
        library: Rc<RefCell<Library>>,
        viewport: Rc<RefCell<Viewport>>,
        activity: crate::activity::Followed,
    ) {
        *self.0.borrow_mut() = Some(Sources {
            library,
            viewport,
            activity,
        });
    }
}

#[derive(Clone)]
struct Sources {
    library: Rc<RefCell<Library>>,
    viewport: Rc<RefCell<Viewport>>,
    activity: crate::activity::Followed,
}

/// Declares the collectors.
pub struct AgentState {
    pub late: Late,
    pub values: Arc<Mutex<helios_values::Store>>,
}

impl Plugin for AgentState {
    fn id(&self) -> PluginId {
        PLUGIN
    }

    fn title(&self) -> String {
        "Agent state".into()
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        let late = self.late.clone();
        ctx.collector(
            FnCollector::new(
                "helios.library",
                "The library as the views show it: count, picked, rejected; the selection; the image under the cursor (id, name, path); the size of the image open in the viewport.",
            )
            .snapshot(move |_| {
                let Some(s) = late.0.borrow().clone() else {
                    return Json::Null;
                };
                let mut v = library_snapshot(&s.library.borrow());
                // A viewport busy (borrowed while it draws) is left out.
                v["open_size"] = s
                    .viewport
                    .try_borrow()
                    .ok()
                    .and_then(|vp| vp.image_size())
                    .map_or(Json::Null, |s| json!(s));
                v
            }),
        )?;
        let values = self.values.clone();
        ctx.collector(
            FnCollector::new(
                "helios.values",
                "The image's parameters, path to value (`node/exposure/exposure`): what `param.get` reads.",
            )
            .snapshot(move |_| {
                let store = values.lock().unwrap_or_else(PoisonError::into_inner);
                values_snapshot(&store)
            }),
        )?;
        let values = self.values.clone();
        ctx.collector(
            FnCollector::new(
                "controls",
                "Every node parameter and the control that edits it: path, node, control id, implicit (built from its descriptor) or patched, value and how the control shows it.",
            )
            .snapshot(move |_| {
                let store = values.lock().unwrap_or_else(PoisonError::into_inner);
                helios_controls::commands::instances(&store, crate::controls::registry())
            }),
        )?;
        ctx.collector(crate::caches::collector())?;
        let late = self.late.clone();
        ctx.collector(
            FnCollector::new(
                "helios.jobs",
                "The node's promises (every job: the Activity focus's list) counted by state (running, finished, failed, cancelled, total, idle), and each one's id (the promise), job (the job it mirrors, if any), kind, title and state.",
            )
            .snapshot(move |_| {
                late.0
                    .borrow()
                    .as_ref()
                    .map_or(Json::Null, |s| jobs_snapshot(&s.activity.promises()))
            }),
        )?;
        Ok(())
    }
}

/// `helios.library`'s snapshot, the viewport aside.
fn library_snapshot(lib: &Library) -> Json {
    let (count, picked, rejected) = lib.counts();
    let selection = lib.selection();
    let current = lib.selected().and_then(|i| lib.entries.get(i)).map_or(
        Json::Null,
        |e| json!({ "id": e.id, "name": e.name, "path": lib.file(&e.id) }),
    );
    json!({
        "count": count,
        "picked": picked,
        "rejected": rejected,
        "selected": selection.len(),
        "selection": selection,
        "current": current,
        "open_size": null,
    })
}

/// `helios.values`'s snapshot: every `node/` value.
fn values_snapshot(store: &helios_values::Store) -> Json {
    let map: Map<String, Json> = store
        .list("node/")
        .into_iter()
        .map(|(p, e)| (p.to_string(), helios_values::value_to_json(&e.value)))
        .collect();
    Json::Object(map)
}

/// `helios.jobs`'s snapshot from the node's promises: work with no end
/// (the image map) is listed, never counted as running.
fn jobs_snapshot(promises: &[helios_vault::promises::Promise]) -> Json {
    use helios_vault::promises::State;
    let count = |keep: &dyn Fn(&State) -> bool| promises.iter().filter(|p| keep(&p.state)).count();
    let running = promises.iter().filter(|p| crate::activity::runs(p)).count();
    json!({
        "idle": running == 0,
        "total": promises.len(),
        "running": running,
        "finished": count(&|s| matches!(s, State::Done { .. })),
        "failed": count(&|s| matches!(s, State::Failed { .. })),
        "cancelled": count(&|s| matches!(s, State::Cancelled)),
        "list": promises.iter().map(|p| json!({
            "id": p.id,
            "job": p.request[helios_node_core::job_promises::JOB],
            "kind": p.kind,
            "title": p.label,
            "state": p.state.name(),
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_are_counted_by_state() {
        use helios_vault::promises::{Promise, State};
        let promise = |id: &str, kind: &str, state: State| Promise {
            id: id.into(),
            owner: format!("{kind}-{id}"),
            kind: kind.into(),
            label: kind.into(),
            request: json!({ "job": format!("{kind}-1") }),
            state,
            cancel_asked: false,
            attempts: 1,
            paused: None,
            created: 1,
            updated: 1,
        };
        let running = State::Running {
            done: 1,
            total: Some(3),
            note: None,
        };
        let s = jobs_snapshot(&[
            promise("a", "import", running),
            promise("b", "export", State::Done { result: json!({}) }),
            promise(
                "c",
                "export",
                State::Failed {
                    reason: "no room".into(),
                    retryable: false,
                },
            ),
        ]);
        assert_eq!(s["total"], 3);
        assert_eq!(s["running"], 1);
        assert_eq!(s["finished"], 1);
        assert_eq!(s["failed"], 1);
        assert_eq!(s["idle"], false);
        assert_eq!(s["list"][0]["kind"], "import");
        assert_eq!(s["list"][0]["job"], "import-1");
        assert_eq!(s["list"][0]["state"], "running");
        assert_eq!(jobs_snapshot(&[])["idle"], true);
    }

    #[test]
    fn an_empty_library_counts_nothing() {
        let s = library_snapshot(&Library::default());
        assert_eq!(s["count"], 0);
        assert_eq!(s["selected"], 0);
        assert!(s["current"].is_null());
    }

    #[test]
    fn values_are_listed_by_path() {
        let s = values_snapshot(&helios_values::Store::default());
        assert!(s.as_object().is_some_and(Map::is_empty));
    }
}
