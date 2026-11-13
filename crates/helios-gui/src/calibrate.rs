//! The desktop's calibration commands (`docs/design/calibration.md`,
//! section 8), run through the desktop's embedded node as the CLI and
//! agents run them over its endpoint (`helios_node_core::calibrate`):
//!
//! - `calibrate.chart`: a colour chart shot measured and its correction
//!   solved; with `save`, kept as a profile in the vault.
//! - `calibrate.profiles.list`, `.get`, `.delete`: the saved profiles.
//! - `calibrate.profiles.setDefault`: a camera's default profiles.
//! - `calibrate.apply`: a profile (or `auto`, or `none`) put on photos'
//!   edits, the `chartprofile` node.
//!
//! Each run is announced on the node's bus with who ran it
//! (`control.command`). A save, a deletion, a default set or an apply is
//! one step of the library's history, so Ctrl+Z takes it back. Without a library, the chart's dry
//! run is still answered; the profile commands say they need one.

use std::sync::Arc;

use egui_pop::api::history::Label;
use helios_core::{Bus, Message};
use helios_node_core::calibrate::{self as node, APPLY, CHART, DELETE, GET, LIST, SET_DEFAULT};
use helios_shell::commands::CommandSpec;
use helios_vault::{Changes, Store, Vault};
use serde_json::Value;

use crate::library_history::LibraryHistory;
use crate::shell::Shell;

/// Measures a chart shot and answers the report; saves it when asked.
pub const CHART_COMMAND: &str = CHART;

/// The vault the desktop's node owns and its bus.
pub struct Node<S: Store> {
    pub vault: Arc<Vault<S>>,
    pub bus: Arc<Bus>,
}

fn seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The history step of command `id` on the profile `name` (an apply
/// with none: the profile turned off; a default: of camera `name`).
pub fn label(id: &str, name: &str) -> Label {
    let m = if id == DELETE {
        Message::new(
            "gui-history-calibration-delete",
            "Delete the calibration profile “{ $name }”",
        )
    } else if id == APPLY && name.is_empty() {
        Message::new(
            "gui-history-calibration-off",
            "Turn the calibration profile off",
        )
    } else if id == APPLY {
        Message::new(
            "gui-history-calibration-apply",
            "Apply the calibration profile “{ $name }”",
        )
    } else if id == SET_DEFAULT {
        Message::new(
            "gui-history-calibration-default",
            "Set the calibration default of { $name }",
        )
    } else {
        Message::new(
            "gui-history-calibration-save",
            "Save the calibration profile “{ $name }”",
        )
    }
    .with("name", name);
    Label {
        text: m.english.clone(),
        key: Some(m.key),
        args: [("name".to_string(), name.to_string())].into(),
    }
}

/// The profile's name in an answer of `id` (an apply turning it off:
/// none; a default: its camera).
fn named(id: &str, answer: &Value) -> String {
    if id == APPLY {
        return answer["images"][0]["name"]
            .as_str()
            .unwrap_or_default()
            .to_string();
    }
    if id == SET_DEFAULT {
        return answer["camera"].as_str().unwrap_or_default().to_string();
    }
    let profile = if id == DELETE {
        &answer["deleted"]
    } else {
        &answer["profile"]
    };
    profile["name"].as_str().unwrap_or_default().to_string()
}

/// Runs command `id` through the node holding `vault`, by `who`; the
/// change it made recorded in `history`.
fn through_node<S: Store>(
    node: &Node<S>,
    history: &LibraryHistory,
    id: &str,
    args: &Value,
    who: &str,
) -> Result<Value, String> {
    let (answer, changes): (Value, Changes) =
        helios_node_core::endpoint::command(&node.vault, &node.bus, id, args, who)?;
    if !changes.is_empty() {
        history.record(label(id, &named(id, &answer)), changes, seconds());
    }
    Ok(answer)
}

/// Registers the calibration commands on the shell, run through `node`
/// (none without a library), their changes recorded in `history`.
///
/// # Errors
/// A command cannot be registered.
pub fn register<S: Store + Send + 'static>(
    shell: &mut Shell,
    node: Option<Node<S>>,
    history: LibraryHistory,
) -> Result<(), String> {
    let node = node.map(Arc::new);
    let specs = [
        (
            CHART,
            "Measures a colour chart shot and solves its correction: the report per patch, the light and the matrix; with `save`, keeps it as a calibration profile",
            false,
        ),
        (
            LIST,
            "Lists the calibration profiles (`camera`: one camera's)",
            true,
        ),
        (GET, "Shows a calibration profile (`profile`: its id)", true),
        (
            DELETE,
            "Deletes a calibration profile (`profile`: its id)",
            false,
        ),
        (
            SET_DEFAULT,
            "Sets a camera's default calibration profiles (`profiles`: their ids, empty with `camera` to clear)",
            false,
        ),
        (
            APPLY,
            "Applies a calibration profile to photos (`images`; `profile`: an id, `auto` for the camera's nearest light, or `none`)",
            false,
        ),
    ];
    for (id, label, read_only) in specs {
        let mut spec = CommandSpec::new(id, label, "Calibration");
        if read_only {
            spec = spec.read_only();
        }
        let (node, history) = (node.clone(), history.clone());
        shell.register(spec, move |args, origin| {
            let who = crate::persist::who(origin);
            match &node {
                Some(node) => through_node(node, &history, id, args, &who),
                None => node::run(None::<&Vault<S>>, id, args, 0)
                    .map(|done| done.answer)
                    .map_err(|m| m.english),
            }
        })?;
    }
    Ok(())
}
