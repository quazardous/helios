//! What lets a run outlive the node process that started it
//! (`docs/workflow-level.md`, "The store" and "Failure and restart"):
//!
//! - **owners**: the node process working a run says so in the vault, with
//!   a heartbeat; another one takes the run over only once that owner is
//!   dead: silent for [`DEAD_MS`], or, on the same machine, a process that
//!   is gone (so a node restarted after a crash resumes its runs at once);
//! - **formats**: a run's documents carry the format they were written
//!   in; an older one is migrated when the run is opened, step by step, and
//!   a newer one is left alone;
//! - **the network**: a run renders the network as it was at submission,
//!   rebuilt over the node's own network from the run's snapshot.

use std::path::Path;

use helios_core::{Graph, ModuleKind, Params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How often an owner says it is alive, in each run it holds.
pub const HEARTBEAT_MS: u64 = 10_000;

/// An owner silent this long is dead, and its runs are taken over.
pub const DEAD_MS: u64 = 60_000;

/// Who works a run: the document `workflow/owners/<run>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    /// The node instance (its inspect `instance`).
    pub instance: String,
    /// The machine, as it names itself.
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub pid: u32,
    /// When the process started, as the system counts it: tells a pid
    /// reused by another process.
    #[serde(default)]
    pub started: Option<u64>,
    /// When it last said it was alive.
    pub heartbeat: u64,
}

impl Owner {
    /// This process, as instance `instance`, alive at `now`.
    pub fn this(instance: &str, now: u64) -> Self {
        let pid = std::process::id();
        Self {
            instance: instance.to_string(),
            host: host(),
            pid,
            started: match probe(pid) {
                Probe::Alive(started) => started,
                Probe::Gone | Probe::Unknown => None,
            },
            heartbeat: now,
        }
    }

    /// Whether this owner is alive at `now`, as `me` sees it: itself; or
    /// heard from within [`DEAD_MS`], unless it ran on this machine and
    /// its process is gone.
    pub fn alive(&self, me: &Owner, now: u64) -> bool {
        if self.instance == me.instance {
            return true;
        }
        if now.saturating_sub(self.heartbeat) >= DEAD_MS {
            return false;
        }
        let here = !self.host.is_empty() && self.host == me.host && self.pid != 0;
        !(here && gone(self.pid, self.started))
    }
}

/// The machine's name: the kernel's, else `HOSTNAME`, else empty (then
/// only the heartbeat says whether an owner is alive).
fn host() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_string())
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_default()
}

/// What the system says of a process.
enum Probe {
    Gone,
    /// Running, with its start time when known.
    Alive(Option<u64>),
    /// This system does not say (no `/proc`).
    Unknown,
}

fn probe(pid: u32) -> Probe {
    if !Path::new("/proc/self/stat").exists() {
        return Probe::Unknown;
    }
    let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return Probe::Gone;
    };
    // After the command name, in parentheses: the state (field 3), and
    // the start time (field 22).
    let rest = text.rsplit_once(')').map_or("", |(_, r)| r);
    let fields: Vec<&str> = rest.split_whitespace().collect();
    if matches!(fields.first(), Some(&"Z" | &"X")) {
        return Probe::Gone;
    }
    Probe::Alive(fields.get(19).and_then(|f| f.parse().ok()))
}

/// Whether process `pid`, started at `started`, is gone.
fn gone(pid: u32, started: Option<u64>) -> bool {
    match probe(pid) {
        Probe::Gone => true,
        Probe::Alive(now) => started.is_some() && now.is_some() && now != started,
        Probe::Unknown => false,
    }
}

/// One migration: a run document and its item documents, from a format
/// to the next.
pub type Step = fn(&mut Value, &mut [Value]) -> Result<(), String>;

/// The migrations, the one at index `i` from format `i + 1` to `i + 2`.
/// None yet: format 1 is the first.
pub const STEPS: &[Step] = &[];

/// The format this node writes: raised by a change older code cannot read,
/// with the step that brings older runs to it.
pub const FORMAT: u64 = STEPS.len() as u64 + 1;

/// The format of a run document (1 when it says none).
pub fn format_of(doc: &Value) -> u64 {
    doc["format"].as_u64().unwrap_or(1)
}

/// Brings a run's documents to the format after `steps`, step by step:
/// whether anything changed. A format newer than `steps` know is refused.
pub fn migrate(steps: &[Step], doc: &mut Value, items: &mut [Value]) -> Result<bool, String> {
    let target = steps.len() as u64 + 1;
    let from = format_of(doc).max(1);
    if from > target {
        return Err(format!("format {from} is newer than {target}"));
    }
    for step in &steps[(from - 1) as usize..] {
        step(doc, items)?;
    }
    if from == target {
        return Ok(false);
    }
    doc["format"] = Value::from(target);
    Ok(true)
}

/// The network a run renders, rebuilt from its `snapshot` over `base`,
/// the node's own: the same nodes, modules and wiring, with the
/// snapshot's parameters, bypasses and answers. Refused when the networks
/// differ: the run was submitted to another network.
pub fn restore(base: &Graph, snapshot: &Value) -> Result<Graph, String> {
    let mut graph = base.clone();
    let names = super::runs::node_names(&graph);
    let nodes = snapshot["nodes"]
        .as_array()
        .ok_or("a snapshot without nodes")?;
    if nodes.len() != names.len() {
        return Err(format!(
            "{} nodes in the run's network, {} in the node's",
            nodes.len(),
            names.len()
        ));
    }
    for node in nodes {
        let name = node["name"].as_str().unwrap_or_default();
        let id = *names
            .get(name)
            .ok_or_else(|| format!("no node `{name}` in the node's network"))?;
        let Some(manifest) = graph.manifest(id).map_err(|e| e.to_string())? else {
            continue;
        };
        if node["module"].as_str() != Some(manifest.name.as_str()) {
            return Err(format!("`{name}` is another module in the node's network"));
        }
        let params: Params =
            serde_json::from_value(node["params"].clone()).map_err(|e| format!("`{name}`: {e}"))?;
        if manifest.kind == ModuleKind::Decision {
            graph.answer(id, &params).map_err(|e| e.to_string())?;
            if node["answered"] == false {
                graph.withdraw(id).map_err(|e| e.to_string())?;
            }
        } else {
            for (param, value) in params.iter() {
                graph
                    .set_param(id, param, value.clone())
                    .map_err(|e| e.to_string())?;
            }
        }
        let bypassed = node["bypassed"].as_bool().unwrap_or(false);
        if graph.is_bypassed(id).map_err(|e| e.to_string())? != bypassed {
            graph.set_bypass(id, bypassed).map_err(|e| e.to_string())?;
        }
    }
    if super::snapshot(&graph) != *snapshot {
        return Err("the node's network is wired otherwise".into());
    }
    Ok(graph)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn owner(instance: &str, heartbeat: u64) -> Owner {
        Owner {
            instance: instance.into(),
            host: "elsewhere".into(),
            pid: 1,
            started: None,
            heartbeat,
        }
    }

    #[test]
    fn an_owner_is_dead_once_silent_or_once_its_process_is_gone() {
        let me = Owner::this("me", 100_000);
        assert!(me.alive(&me, u64::MAX), "always alive to itself");
        // Elsewhere: only the heartbeat says.
        let other = owner("other", 100_000);
        assert!(other.alive(&me, 100_000 + DEAD_MS - 1));
        assert!(!other.alive(&me, 100_000 + DEAD_MS));
        // Here, a live process (this one, as another instance) is alive
        // while it beats.
        let twin = Owner {
            instance: "twin".into(),
            ..me.clone()
        };
        assert!(twin.alive(&me, 100_000));
        if matches!(probe(std::process::id()), Probe::Alive(_)) {
            // A pid no process has: dead at once, heartbeat or not.
            let ghost = Owner {
                pid: u32::MAX - 1,
                ..twin.clone()
            };
            assert!(!ghost.alive(&me, 100_000));
            // The same pid, started at another time: another process.
            if let Some(started) = me.started {
                let reused = Owner {
                    started: Some(started + 1),
                    ..twin
                };
                assert!(!reused.alive(&me, 100_000));
            }
        }
    }

    #[allow(clippy::unnecessary_wraps, reason = "the signature of a step")]
    fn rename_title(doc: &mut Value, items: &mut [Value]) -> Result<(), String> {
        doc["name"] = doc["title"].take();
        for item in items {
            item["rank"] = item["order"].clone();
        }
        Ok(())
    }

    fn refuse(_: &mut Value, _: &mut [Value]) -> Result<(), String> {
        Err("cannot".into())
    }

    #[test]
    fn older_runs_are_migrated_step_by_step_and_newer_ones_refused() {
        assert_eq!(FORMAT, 1);
        let mut doc = json!({ "format": 1, "title": "t" });
        assert_eq!(migrate(STEPS, &mut doc, &mut []), Ok(false));
        assert_eq!(doc, json!({ "format": 1, "title": "t" }));

        let steps: &[Step] = &[rename_title];
        let mut items = [json!({ "order": 3 })];
        assert_eq!(migrate(steps, &mut doc, &mut items), Ok(true));
        assert_eq!(doc, json!({ "format": 2, "title": null, "name": "t" }));
        assert_eq!(items[0]["rank"], 3);
        // At the target already: untouched.
        assert_eq!(migrate(steps, &mut doc, &mut items), Ok(false));
        // No format said: the first.
        let mut bare = json!({ "title": "b" });
        assert_eq!(migrate(steps, &mut bare, &mut []), Ok(true));
        assert_eq!(bare["format"], 2);
        // Newer than known, or a step that fails: refused.
        let mut newer = json!({ "format": 3 });
        assert!(migrate(steps, &mut newer, &mut []).is_err());
        let mut old = json!({ "format": 1 });
        assert!(migrate(&[refuse], &mut old, &mut []).is_err());
    }
}
