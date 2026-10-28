//! The workspace's network edited node by node (`network.*`): connect,
//! disconnect, move, add, remove and paste (copied nodes, see
//! `crate::clipboard::nodes`), each one undoable workspace step
//! (written to the workspace's files at once, seen by agents), checked
//! like any edit of the workspace. The Graph panel's edits become these
//! commands; agents and scripts run them too, and the node's MCP tools
//! (`helios_network_*`) through the desktop holding the workspace. The
//! edits themselves are `helios_template::network`'s, the same on the
//! files when no desktop holds the workspace.

use helios_shell::commands::Origin;
use helios_template::network;
use serde_json::{Value, json};

use super::Shell;

/// The network's commands, with their titles.
pub(super) const COMMANDS: &[(&str, &str)] = network::COMMANDS;

impl Shell {
    /// Runs one of [`COMMANDS`] as one undo step; with `dry_run`, says
    /// what it would do (and the problems it would meet) and changes
    /// nothing.
    pub(super) fn network_command(
        &mut self,
        id: &str,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        let catalogue = self.workspace_catalogue();
        let label = network::label(id, args)?;
        if args["dry_run"].as_bool().unwrap_or(false) {
            let now = self.workspace_state().ok_or("no workspace is open")?;
            let (next, mut answer) = network::edited(&now, &catalogue, id, args)?;
            answer["problems"] = json!(next.problems(&catalogue));
            answer["dry_run"] = json!(true);
            return Ok(answer);
        }
        let answer = std::cell::RefCell::new(json!({}));
        self.workspace_step(&label, origin, |now| {
            let (next, a) = network::edited(now, &catalogue, id, args)?;
            let problems = next.problems(&catalogue);
            if !problems.is_empty() {
                return Err(problems.join("; "));
            }
            *answer.borrow_mut() = a;
            Ok(next)
        })?;
        Ok(answer.into_inner())
    }
}
