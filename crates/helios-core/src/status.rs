//! The normalized status of a node: what every surface shows in a node's
//! header, the same for every node (see `docs/gui.md`).

use serde::Serialize;

use crate::graph::NodeId;
use crate::image::Region;
use crate::manifest::{Family, Role};

/// What a node is in a network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    Source,
    Process,
    Decision,
    Sink,
}

/// Where a node stands for a given render.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeState {
    /// Never asked for this render.
    Idle,
    /// Asked before, but its inputs or parameters changed since: the content
    /// it holds is not the one asked for now.
    Stale,
    /// Being computed.
    Running,
    /// Computed (or shared) and current.
    Done,
    /// Its computation failed; asking again gives the same error.
    Failed,
    /// A decision node waiting for its answer; what reads from it waits too.
    Pending,
}

/// How much a node has been asked and has worked, for one render region.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Load {
    pub requests: u64,
    pub computed: u64,
    /// Requests served by content already computed or in flight.
    pub shared: u64,
    pub running: u64,
}

/// A node's header.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NodeStatus {
    pub node: NodeId,
    pub name: String,
    #[serde(rename = "type")]
    pub node_type: NodeType,
    pub family: Family,
    pub role: Role,
    pub state: NodeState,
    pub region: Region,
    /// Short content key of what is asked now.
    pub key: String,
    pub load: Load,
    /// Where it runs: `cpu` for now; GPU and remote nodes come with the scheduler.
    pub runs_on: &'static str,
}
