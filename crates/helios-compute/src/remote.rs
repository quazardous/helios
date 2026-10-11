//! Remote units: helios-node instances reached over their socket.
//!
//! A node answers the inspection protocol: a probe asks it to `describe`
//! its modules, which says it is there and what it runs. A node does not
//! report its devices yet, so a remote unit's devices stay unknown, and no
//! work is routed to it until the node renders on request.

use helios_inspect::{Address, Request, request_at};
use serde::Serialize;

use crate::settings::RemoteSettings;

/// What is known of a remote unit.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RemoteState {
    /// Declared, not asked yet.
    #[default]
    NotProbed,
    /// It answered: the modules it runs.
    Reachable {
        modules: Vec<String>,
    },
    Unreachable {
        error: String,
    },
}

/// Asks a node what it runs.
pub fn probe(remote: &RemoteSettings) -> RemoteState {
    let token = match &remote.token_file {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(t) => Some(t.trim().to_string()),
            Err(e) => {
                return RemoteState::Unreachable {
                    error: format!("{}: {e}", path.display()),
                };
            }
        },
        None => None,
    };
    let address = match Address::parse(&remote.address, token) {
        Ok(a) => a,
        Err(error) => return RemoteState::Unreachable { error },
    };
    let request = Request::Describe {
        entity: "modules".into(),
    };
    match request_at(&address, &request) {
        Err(e) => RemoteState::Unreachable {
            error: format!("{address}: {e}"),
        },
        Ok(answer) if answer["ok"] != serde_json::Value::Bool(true) => RemoteState::Unreachable {
            error: format!("{address} answered: {}", answer["error"]),
        },
        Ok(answer) => RemoteState::Reachable {
            modules: answer["describe"]["modules"]
                .as_object()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
        },
    }
}
