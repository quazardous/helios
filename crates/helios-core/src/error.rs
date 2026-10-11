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
    /// A failure injected on purpose (testing the error paths). Transient:
    /// never cached, so asking again computes again.
    Fault { node: String, what: String },
    /// A decision node has no recorded answer yet. Transient: once it is
    /// answered, asking again renders.
    Pending { node: String },
    /// A render scale outside (0, 1], as written.
    Scale(String),
    /// A result was asked in an accelerator's memory of an engine that has
    /// none.
    NoAccelerator,
}

impl Error {
    /// Whether asking again may give another answer: such errors are not cached.
    pub fn is_transient(&self) -> bool {
        matches!(self, Error::Fault { .. } | Error::Pending { .. })
    }
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
            Error::Fault { node, what } => write!(f, "{node}: injected fault ({what})"),
            Error::Scale(scale) => write!(f, "render scale {scale} is not in (0, 1]"),
            Error::NoAccelerator => write!(f, "the engine has no accelerator"),
            Error::Pending { node } => write!(f, "{node}: decision pending (no recorded answer)"),
        }
    }
}

impl std::error::Error for Error {}
