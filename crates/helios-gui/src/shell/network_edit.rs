//! The workspace's network edited node by node (`network.*`): connect,
//! disconnect, move, add, remove and paste (copied nodes, see
//! `crate::clipboard::nodes`), each one undoable workspace step
//! (written to the workspace's files at once, seen by agents), checked
//! like any edit of the workspace. The Graph panel's edits become these
//! commands; agents and scripts run them too.

use helios_shell::commands::Origin;
use helios_template::workspace::State;
use serde_json::{Value, json};

use super::Shell;
use crate::graph;

/// The network's commands, with their titles.
pub(super) const COMMANDS: &[(&str, &str)] = &[
    (
        "network.connect",
        "Connect an output to an input (`from`, `port`, `to`, `input`, `replaces`)",
    ),
    (
        "network.disconnect",
        "Remove wires of the network (`connections`: each `from`, `port`, `to`, `input`)",
    ),
    (
        "network.move",
        "Move nodes in the graph view (`moves`: each `node`, `to`: [x, y])",
    ),
    (
        "network.add",
        "Add nodes to the network (`nodes`: each `module` or `copy_of`, `at`: [x, y])",
    ),
    (
        "network.remove",
        "Remove nodes from the network, with their wires (`nodes`)",
    ),
    (
        "network.paste",
        "Paste copied nodes into the network (`clip`: a `helios.nodes` payload; `parts`), renamed on a clash",
    ),
];

impl Shell {
    /// Runs one of [`COMMANDS`] as one undo step.
    pub(super) fn network_command(
        &mut self,
        id: &str,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        let catalogue = self.workspace_catalogue();
        let mut answer = json!({});
        let label = match id {
            "network.connect" => format!(
                "connect {}.{} to {}.{}",
                args["from"].as_str().unwrap_or_default(),
                args["port"].as_str().unwrap_or_default(),
                args["to"].as_str().unwrap_or_default(),
                args["input"].as_str().unwrap_or_default(),
            ),
            "network.disconnect" => "disconnect".to_string(),
            "network.move" => "move nodes".to_string(),
            "network.add" => "add nodes".to_string(),
            "network.remove" => "remove nodes".to_string(),
            "network.paste" => "paste nodes".to_string(),
            other => return Err(format!("no network command `{other}`")),
        };
        let added = std::cell::RefCell::new(Vec::new());
        let renamed = std::cell::RefCell::new(std::collections::BTreeMap::new());
        self.workspace_step(&label, origin, |now: &State| {
            let t = &now.bundle.template;
            let mut net = t.network.clone();
            let mut positions = t.positions.clone();
            match id {
                "network.connect" => {
                    graph::connect(&mut net, &catalogue, args)?;
                }
                "network.disconnect" => {
                    graph::disconnect(&mut net, args)?;
                }
                "network.move" => {
                    graph::place(&net, &mut positions, args)?;
                }
                "network.add" => {
                    *added.borrow_mut() = graph::add(&mut net, &mut positions, &catalogue, args)?;
                }
                "network.paste" => {
                    let clip = crate::clipboard::nodes::Clip::from_json(&args["clip"])?;
                    let parts: Option<Vec<String>> = match &args["parts"] {
                        Value::Null => None,
                        v => Some(
                            serde_json::from_value(v.clone())
                                .map_err(|e| format!("`parts`: {e}"))?,
                        ),
                    };
                    let out = crate::clipboard::nodes::paste(
                        &mut net,
                        &mut positions,
                        &clip,
                        parts.as_deref(),
                    )?;
                    *renamed.borrow_mut() = out.renamed;
                    *added.borrow_mut() = out.added;
                }
                _ => {
                    graph::remove(&mut net, args)?;
                }
            }
            let next = now.edited(&json!({ "network": net, "positions": positions }))?;
            let problems = next.problems(&catalogue);
            if problems.is_empty() {
                Ok(next)
            } else {
                Err(problems.join("; "))
            }
        })?;
        let added = added.into_inner();
        if !added.is_empty() {
            answer["added"] = json!(added);
        }
        let renamed = renamed.into_inner();
        if !renamed.is_empty() {
            answer["renamed"] = json!(renamed);
        }
        Ok(answer)
    }
}
