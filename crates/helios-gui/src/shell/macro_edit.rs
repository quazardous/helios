//! Macros on the desktop (`macro.*`, see `docs/design/macros.md`): a
//! macro made from nodes, opened in place, folded, dissolved and moved to
//! a newer version, each one undo step that moves the open image's values
//! with the network (`Shell::workspace_step_moving`), so Ctrl+Z gives back
//! the nodes, the files and the image's sliders at once. The steps
//! themselves are `helios_template::macros`', the same for agents on the
//! files when no desktop holds the workspace.

use helios_core::Message;
use helios_shell::commands::Origin;
use helios_template::format::Exposed;
use helios_template::macros::{Step, Values};
use helios_template::workspace::State;
use serde_json::{Value, json};

use super::{Shell, lock};

/// Which nodes are selected in the graph, several at once (a JSON list of
/// node names): what `macro.make` packages by default. `view/graph/
/// selection` stays the one node the Parameters and Masks panels follow.
pub const SELECTED: &str = "view/graph/selected";

/// The macros' commands, with their titles.
pub(super) const COMMANDS: &[(&str, &str)] = &[
    (
        "macro.make",
        "Make a macro of nodes (`nodes`, the selected ones by default; `name`, `expose`, `dry_run`)",
    ),
    (
        "macro.open",
        "Open a macro in place, its nodes shown (`node`, the selected one by default; `dry_run`)",
    ),
    (
        "macro.fold",
        "Fold an opened macro back into one node (`node`; `dry_run`)",
    ),
    (
        "macro.toggle",
        "Open the selected macro, or fold the one the selected node is in",
    ),
    (
        "macro.dissolve",
        "Give a macro way to its nodes, as ordinary ones (`node`; `dry_run`)",
    ),
    (
        "macro.upgrade",
        "Move a macro to a newer version of its file (`node`, `version`; `dry_run`)",
    ),
];

/// The nodes selected in the graph, as [`SELECTED`] holds them.
pub fn selected_nodes(store: &helios_values::Store) -> Vec<String> {
    match store.get(SELECTED).map(|e| &e.value) {
        Some(helios_core::Value::Text(t)) => serde_json::from_str(t).unwrap_or_default(),
        _ => Vec::new(),
    }
}

impl Shell {
    /// The open image's values that differ from the workspace's network
    /// (`node/<node>/<param>`, `bypass/<node>`): what a macro step moves.
    /// None without an image open.
    pub(super) fn image_values(&self, state: &State) -> Result<Values, String> {
        let mut values = Values::new();
        if self.viewport.borrow().graph().is_none() {
            return Ok(values);
        }
        let built = state.bundle.build(&self.workspace_catalogue())?;
        let graph = &built.graph;
        let store = lock(&self.values);
        for (path, entry) in store.list("node/") {
            let Some((node, param)) = path
                .strip_prefix("node/")
                .and_then(|rest| rest.rsplit_once('/'))
            else {
                continue;
            };
            let Some(id) = graph.find(node) else {
                continue;
            };
            let network = graph
                .params(id)
                .ok()
                .flatten()
                .and_then(|p| p.get(param).cloned());
            if network.is_some() && network.as_ref() != Some(&entry.value) {
                values.insert(path.to_string(), entry.value.clone());
            }
        }
        for (path, entry) in store.list(helios_values::BYPASS) {
            let node = &path[helios_values::BYPASS.len()..];
            let Some(id) = graph.find(node) else {
                continue;
            };
            let network = graph.is_bypassed(id).unwrap_or(false);
            if entry.value.as_bool().is_some_and(|on| on != network) {
                values.insert(path.to_string(), entry.value.clone());
            }
        }
        Ok(values)
    }

    /// The node a macro command acts on: `node`, else the selected one.
    fn macro_target(&self, args: &Value) -> Result<String, String> {
        if let Some(n) = args["node"].as_str().filter(|s| !s.is_empty()) {
            return Ok(n.to_string());
        }
        crate::modules::selected(&lock(&self.values)).ok_or_else(|| "no macro is selected".into())
    }

    /// Runs one of [`COMMANDS`].
    pub(super) fn macro_command(
        &mut self,
        id: &str,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        let catalogue = self.workspace_catalogue();
        match id {
            "macro.make" => {
                let mut nodes: Vec<String> = match &args["nodes"] {
                    Value::Null => Vec::new(),
                    v => serde_json::from_value(v.clone()).map_err(|e| format!("nodes: {e}"))?,
                };
                if nodes.is_empty() {
                    // The nodes selected in the graph, else the one.
                    let store = lock(&self.values);
                    nodes = selected_nodes(&store);
                    if nodes.is_empty() {
                        nodes.extend(crate::modules::selected(&store));
                    }
                }
                if nodes.is_empty() {
                    return Err("no node is selected: select the nodes to package".into());
                }
                let Some(name) = args["name"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                else {
                    return self.macro_name_form(&nodes).map(|()| Value::Null);
                };
                let expose: Option<Vec<Exposed>> = match &args["expose"] {
                    Value::Null => None,
                    v => Some(
                        serde_json::from_value(v.clone()).map_err(|e| format!("expose: {e}"))?,
                    ),
                };
                let label = format!("make macro {name}");
                let answer = self.macro_step(&label, args, origin, |now, values| {
                    now.macro_made(&catalogue, &nodes, &name, expose.as_deref(), values)
                })?;
                // The macro is what one looks at now.
                if let Some(node) = answer["node"].as_str()
                    && !args["dry_run"].as_bool().unwrap_or(false)
                {
                    self.select_node(node);
                }
                Ok(answer)
            }
            "macro.open" => {
                let node = self.macro_target(args)?;
                self.macro_step(&format!("open {node}"), args, origin, |now, values| {
                    now.macro_opened(&catalogue, &node, values)
                })
            }
            "macro.fold" => {
                let node = self.macro_target(args)?;
                // A node of an opened macro folds it.
                let node = node.split('.').next().unwrap_or(&node).to_string();
                self.macro_step(&format!("fold {node}"), args, origin, |now, values| {
                    now.macro_folded(&catalogue, &node, values)
                })
            }
            "macro.toggle" => {
                let node = self.macro_target(args)?;
                let state = self.workspace_state().ok_or("no workspace is open")?;
                let head = node.split('.').next().unwrap_or(&node);
                let open = state
                    .bundle
                    .template
                    .network
                    .nodes
                    .iter()
                    .find(|n| n.name == head)
                    .is_some_and(|n| n.open);
                let id = if open { "macro.fold" } else { "macro.open" };
                self.macro_command(id, &json!({ "node": head }), origin)
            }
            "macro.dissolve" => {
                let node = self.macro_target(args)?;
                self.macro_step(&format!("dissolve {node}"), args, origin, |now, values| {
                    now.macro_dissolved(&catalogue, &node, values)
                })
            }
            "macro.upgrade" => {
                let node = self.macro_target(args)?;
                let version = match &args["version"] {
                    Value::Null => None,
                    v => Some(
                        v.as_u64()
                            .and_then(|v| u32::try_from(v).ok())
                            .ok_or("version: a whole number")?,
                    ),
                };
                self.macro_step(&format!("upgrade {node}"), args, origin, |now, values| {
                    now.macro_upgraded(&catalogue, &node, version, values)
                })
            }
            other => Err(format!("no macro command `{other}`")),
        }
    }

    /// A macro step as one undo step moving the image's values; a dry run
    /// answers what it would do and changes nothing.
    fn macro_step(
        &mut self,
        label: &str,
        args: &Value,
        origin: &Origin,
        make: impl Fn(&State, &Values) -> Result<Step, String>,
    ) -> Result<Value, String> {
        let now = self.workspace_state().ok_or("no workspace is open")?;
        let values = self.image_values(&now)?;
        if args["dry_run"].as_bool().unwrap_or(false) {
            let mut answer = make(&now, &values)?.answer;
            answer["dry_run"] = json!(true);
            return Ok(answer);
        }
        let mut answer = Value::Null;
        self.workspace_step_moving(label, origin, |now| {
            let step = make(now, &values)?;
            answer = step.answer;
            Ok((step.state, step.values))
        })?;
        answer["dry_run"] = json!(false);
        Ok(answer)
    }

    /// Selects one node (`view/graph/selection` and [`SELECTED`]).
    fn select_node(&self, node: &str) {
        let mut store = lock(&self.values);
        for (path, value) in [
            (crate::params::SELECTION, node.to_string()),
            (SELECTED, json!([node]).to_string()),
        ] {
            let _ = store.set(helios_values::Set::new(
                path,
                helios_values::Change::To(helios_core::Value::Text(value)),
                Origin::User,
            ));
        }
    }

    /// The macro's name, asked; the answer runs `macro.make` on `nodes`.
    fn macro_name_form(&mut self, nodes: &[String]) -> Result<(), String> {
        let f = crate::dialogs::Form {
            title: self.text(Message::new("gui-macro-make-title", "Make a macro")),
            fields: vec![crate::dialogs::Field {
                key: "name".into(),
                descriptor: helios_values::Descriptor::text(
                    &self.text(Message::new("gui-macro-name", "name")),
                    "",
                ),
                value: helios_core::Value::Text(String::new()),
            }],
            verb: self.text(Message::new("gui-macro-make-verb", "Make")),
            command: "macro.make".into(),
        };
        self.form_with(f, json!({ "nodes": nodes }))
    }
}
