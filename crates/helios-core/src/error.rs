use std::fmt;

/// Errors of the core. Cloneable, because a failed computation is cached
/// like a successful one: asking again for the same content gives the same
/// answer without running it again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A parameter is unknown, of the wrong type or out of range.
    Param {
        module: String,
        param: String,
        reason: String,
    },
    /// A port name does not exist on a module.
    Port { module: String, port: String },
    /// A node id does not exist in the graph.
    Node(usize),
    /// A node input is not connected.
    Unconnected { node: String, port: String },
    /// A connection would create a cycle.
    Cycle { from: String, to: String },
    /// A module broke its contract (wrong output count, region or channels).
    Contract { module: String, reason: String },
    /// A module failed while preparing or processing.
    Module { module: String, reason: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Param {
                module,
                param,
                reason,
            } => write!(f, "{module}: parameter `{param}`: {reason}"),
            Error::Port { module, port } => write!(f, "{module}: no port `{port}`"),
            Error::Node(id) => write!(f, "no node #{id}"),
            Error::Unconnected { node, port } => {
                write!(f, "{node}: input `{port}` is not connected")
            }
            Error::Cycle { from, to } => {
                write!(f, "connecting {from} to {to} would create a cycle")
            }
            Error::Contract { module, reason } => {
                write!(f, "{module} broke its contract: {reason}")
            }
            Error::Module { module, reason } => write!(f, "{module}: {reason}"),
        }
    }
}

impl std::error::Error for Error {}
