//! The workflow level in a node process: a batch submitted to the node
//! becomes a **run**, worked through the node journal (`helios-journal`),
//! as `docs/workflow-level.md` decides.
//!
//! - A run's journal workflow is **derived from the network**: its nodes
//!   are the graph nodes that queue (the export, and the decision nodes
//!   it depends on), each after its nearest queueing ancestors through any
//!   number of process nodes, and after a root `submit` when it has none
//!   ([`derive`]). Process nodes never queue: they are computed inside the
//!   render of the export.
//! - A run's id follows from the network and the export settings
//!   ([`run_id`]), so submitting them again extends the same run.
//! - An item is a file, named by the hash of its absolute path
//!   ([`item_id`]), so a submission reads no image.
//! - [`runs::Workflows`] keeps the runs of the node process, works them
//!   by claim and conclude, and answers the `workflow.*` requests.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use helios_core::Graph;
use helios_decision::Mode;
use helios_journal::{Node as JournalNode, Workflow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod runs;

/// The root every item of a run is `done` at once it entered the run.
pub const SUBMIT: &str = "submit";

/// What a queueing graph node does for an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Queue {
    /// The batch export: render the item, write its file.
    Export,
    /// A decision, answered by a model or a person depending on its mode.
    Decision { mode: Mode },
}

/// A graph node, as the derivation sees it.
#[derive(Clone, Debug)]
pub struct GraphNode {
    pub name: String,
    /// The nodes feeding its inputs, by name.
    pub inputs: Vec<String>,
    /// Its mode, when it is a decision node.
    pub decision: Option<Mode>,
}

/// How a failure is retried, as the journal takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryDesc {
    /// Attempts in all.
    pub limit: u32,
    pub delay_ms: u64,
    /// Doubling after each attempt.
    pub exponential: bool,
}

/// A node of a run's journal workflow: a queueing graph node, with its
/// settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueueNode {
    pub name: String,
    /// Its nearest queueing ancestors, or [`SUBMIT`].
    pub parents: Vec<String>,
    #[serde(flatten)]
    pub queue: Queue,
    /// Leases out at once for this graph node, across the runs of a node
    /// process; 0 for a node no worker takes (a person answers).
    pub concurrency: usize,
    pub lease_ms: u64,
    pub retry: RetryDesc,
}

/// How long a lease holds before it is given back: a worker renews it
/// while its job runs.
pub const LEASE_MS: u64 = 60_000;

impl QueueNode {
    /// A queueing node with the defaults of its kind: an export as many
    /// at once as the scheduler has `workers`, three attempts from 2 s
    /// doubling; a model call two at once, four attempts from 5 s
    /// doubling; a person's answer no worker and no retry.
    pub fn with_defaults(name: &str, parents: Vec<String>, queue: Queue, workers: usize) -> Self {
        let (concurrency, retry) = match queue {
            Queue::Export => (
                workers.max(1),
                RetryDesc {
                    limit: 3,
                    delay_ms: 2_000,
                    exponential: true,
                },
            ),
            Queue::Decision { mode: Mode::Human } => (
                0,
                RetryDesc {
                    limit: 1,
                    delay_ms: 0,
                    exponential: false,
                },
            ),
            Queue::Decision { .. } => (
                2,
                RetryDesc {
                    limit: 4,
                    delay_ms: 5_000,
                    exponential: true,
                },
            ),
        };
        Self {
            name: name.into(),
            parents,
            queue,
            concurrency,
            lease_ms: LEASE_MS,
            retry,
        }
    }
}

/// The graph nodes of `graph`, decision nodes with their mode from
/// `decisions`.
pub fn graph_nodes(graph: &Graph, decisions: &BTreeMap<String, Mode>) -> Vec<GraphNode> {
    graph
        .ids()
        .filter_map(|id| {
            let name = graph.name(id).ok()?.to_string();
            let inputs = graph
                .input_nodes(id)
                .ok()?
                .into_iter()
                .flatten()
                .filter_map(|i| graph.name(i).ok().map(str::to_string))
                .collect();
            let decision = decisions.get(&name).copied();
            Some(GraphNode {
                name,
                inputs,
                decision,
            })
        })
        .collect()
}

/// The queueing nodes of a run exporting `export`, in an order where each
/// comes after its parents: the export, and the decision nodes it depends
/// on (a decision that does not reach the export does not change it, and
/// is left out). The parents of each are its nearest queueing ancestors,
/// through any number of process nodes, else [`SUBMIT`].
pub fn derive(nodes: &[GraphNode], export: &str, workers: usize) -> Result<Vec<QueueNode>, String> {
    let by_name: BTreeMap<&str, &GraphNode> = nodes.iter().map(|n| (n.name.as_str(), n)).collect();
    if !by_name.contains_key(export) {
        return Err(format!("no node named `{export}`"));
    }
    if by_name.contains_key(SUBMIT) {
        return Err(format!(
            "a node is named `{SUBMIT}`, the name a run gives its root: rename it"
        ));
    }
    let queues = |n: &GraphNode| n.name == export || n.decision.is_some();
    // The nearest queueing ancestors of a node.
    let nearest = |start: &GraphNode| -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut todo: Vec<&str> = start.inputs.iter().map(String::as_str).collect();
        while let Some(name) = todo.pop() {
            if !seen.insert(name) {
                continue;
            }
            let Some(node) = by_name.get(name) else {
                continue;
            };
            if queues(node) {
                found.insert(name.to_string());
            } else {
                todo.extend(node.inputs.iter().map(String::as_str));
            }
        }
        found
    };
    let mut parents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut todo = vec![export.to_string()];
    while let Some(name) = todo.pop() {
        if parents.contains_key(&name) {
            continue;
        }
        let found = nearest(by_name[name.as_str()]);
        todo.extend(found.iter().cloned());
        parents.insert(name, found);
    }
    // Parents first; between nodes free to go, the network's order.
    let mut placed: Vec<String> = Vec::new();
    while placed.len() < parents.len() {
        let next = nodes.iter().find(|n| {
            parents.get(&n.name).is_some_and(|ps| {
                !placed.contains(&n.name) && ps.iter().all(|p| placed.contains(p))
            })
        });
        match next {
            Some(n) => placed.push(n.name.clone()),
            None => return Err("the network's queueing nodes form a cycle".into()),
        }
    }
    Ok(placed
        .into_iter()
        .map(|name| {
            let node = by_name[name.as_str()];
            let queue = match node.decision {
                Some(mode) if name != export => Queue::Decision { mode },
                _ => Queue::Export,
            };
            let ps: Vec<String> = match &parents[&name] {
                ps if ps.is_empty() => vec![SUBMIT.to_string()],
                ps => ps.iter().cloned().collect(),
            };
            QueueNode::with_defaults(&name, ps, queue, workers)
        })
        .collect())
}

/// The journal workflow of a run's queueing nodes: [`SUBMIT`], then each
/// node after its parents, with its lease and retries.
pub fn journal_workflow(nodes: &[QueueNode]) -> Result<Workflow, String> {
    let mut all = vec![JournalNode::new(SUBMIT)];
    for n in nodes {
        let parents: Vec<&str> = n.parents.iter().map(String::as_str).collect();
        all.push(
            JournalNode::new(&n.name)
                .after(&parents)
                .lease(n.lease_ms)
                .retry(n.retry.limit, n.retry.delay_ms, n.retry.exponential),
        );
    }
    Workflow::new(all)
}

/// The item id of a file outside the library: `path:` and the first 32
/// hex digits of the BLAKE3 hash of its absolute path. Reads no image.
pub fn item_id(path: &Path) -> String {
    let hash = blake3::hash(path.as_os_str().as_encoded_bytes());
    format!("path:{}", &hash.to_hex()[..32])
}

/// What a run keeps of the network it renders: each node with its
/// module, parameters, bypass and inputs, in the network's order.
pub fn snapshot(graph: &Graph) -> Value {
    let nodes: Vec<Value> = graph
        .ids()
        .map(|id| {
            let manifest = graph.manifest(id).ok().flatten();
            let inputs: Vec<Option<String>> = graph
                .input_nodes(id)
                .unwrap_or_default()
                .into_iter()
                .map(|i| i.and_then(|i| graph.name(i).ok().map(str::to_string)))
                .collect();
            json!({
                "name": graph.name(id).unwrap_or_default(),
                "module": manifest.map(|m| m.name),
                "params": graph.params(id).ok().flatten(),
                "bypassed": graph.is_bypassed(id).unwrap_or(false),
                "inputs": inputs,
            })
        })
        .collect();
    json!({ "nodes": nodes })
}

/// A run's id: the first 16 hex digits of the BLAKE3 hash of the canonical
/// JSON of its network snapshot and export settings, with `salt` for a
/// run asked again (`again`).
pub fn run_id(snapshot: &Value, settings: &Value, salt: Option<u64>) -> String {
    let canonical = json!({ "network": snapshot, "export": settings, "salt": salt });
    let bytes = serde_json::to_vec(&canonical).expect("JSON values serialize");
    blake3::hash(&bytes).to_hex()[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, inputs: &[&str], decision: Option<Mode>) -> GraphNode {
        GraphNode {
            name: name.into(),
            inputs: inputs.iter().map(|s| (*s).to_string()).collect(),
            decision,
        }
    }

    #[test]
    fn process_nodes_are_cut_through_and_the_root_is_submit() {
        // source -> decide_exposure -> exposure -> export
        let nodes = [
            node("src", &[], None),
            node("bright", &["src"], Some(Mode::FullAi)),
            node("exp", &["bright"], None),
            node("out", &["exp"], None),
            // A decision that does not reach the export is left out.
            node("aside", &["src"], Some(Mode::Human)),
        ];
        let derived = derive(&nodes, "out", 2).unwrap();
        let names: Vec<_> = derived.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["bright", "out"]);
        assert_eq!(derived[0].parents, [SUBMIT]);
        assert_eq!(derived[0].queue, Queue::Decision { mode: Mode::FullAi });
        assert_eq!(derived[0].retry.limit, 4);
        assert_eq!(derived[1].parents, ["bright"]);
        assert_eq!(derived[1].queue, Queue::Export);
        assert_eq!(derived[1].concurrency, 2);
        assert_eq!(
            derived[1].retry,
            RetryDesc {
                limit: 3,
                delay_ms: 2_000,
                exponential: true
            }
        );
        journal_workflow(&derived).unwrap();
    }

    #[test]
    fn a_node_after_two_decisions_joins_both() {
        let nodes = [
            node("src", &[], None),
            node("a", &["src"], Some(Mode::Human)),
            node("b", &["src"], Some(Mode::AiAssisted)),
            node("mix", &["a", "b"], None),
        ];
        let derived = derive(&nodes, "mix", 1).unwrap();
        let names: Vec<_> = derived.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "mix"]);
        assert_eq!(derived[2].parents, ["a", "b"]);
        assert_eq!(derived[0].concurrency, 0, "a person answers, no worker");
        let flat = derive(&nodes[..1], "src", 1).unwrap();
        assert_eq!(flat[0].parents, [SUBMIT]);
        assert!(derive(&nodes, "nope", 1).is_err());
    }

    #[test]
    fn ids_are_stable_and_depend_on_what_they_name() {
        let a = item_id(Path::new("/photos/a.png"));
        assert!(a.starts_with("path:") && a.len() == 37, "{a}");
        assert_eq!(a, item_id(Path::new("/photos/a.png")));
        assert_ne!(a, item_id(Path::new("/photos/b.png")));
        let net = json!({ "nodes": [] });
        let one = run_id(&net, &json!({ "out": "/x" }), None);
        assert_eq!(one.len(), 16);
        assert_eq!(one, run_id(&net, &json!({ "out": "/x" }), None));
        assert_ne!(one, run_id(&net, &json!({ "out": "/y" }), None));
        assert_ne!(one, run_id(&net, &json!({ "out": "/x" }), Some(1)));
    }
}
