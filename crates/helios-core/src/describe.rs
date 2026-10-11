//! Every framework entity reports its state as structured data, so a test,
//! a script or an agent can observe it without reading logs or pixels.

/// A structured snapshot of an entity's state.
pub trait Describe {
    fn describe(&self) -> serde_json::Value;
}
