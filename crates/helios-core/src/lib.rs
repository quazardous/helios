//! The domain-agnostic core of Helios.
//!
//! A module is a pure computation with a declared surface (its [`Manifest`]).
//! The framework owns everything else: where data lives, what is computed and
//! when, and what is reported. See `docs/module-contract.md` and
//! `docs/architecture.md` at the root of the repository.
//!
//! - [`image`]: image buffers and regions, addressed by content.
//! - [`params`] and [`manifest`]: named, typed parameters and what a module declares.
//! - [`module`]: the contract a module implements.
//! - [`graph`]: a network of nodes.
//! - [`network`]: a node standing for a network (macros, multi-kernel modules).
//! - [`engine`]: evaluates a network, never computing the same content twice.
//! - [`bus`]: typed events for orchestration, with a journal.
//! - [`scheduler`]: named, cancellable background jobs; waiting until idle.
//! - [`jobs`]: how background work goes, as `job.*` events people follow.
//! - [`describe`]: every entity reports its state as structured data.

pub mod bus;
pub mod describe;
pub mod engine;
pub mod error;
pub mod graph;
pub mod image;
pub mod jobs;
pub mod manifest;
pub mod message;
pub mod module;
pub mod network;
pub mod params;
pub mod scheduler;
pub mod status;

pub use bus::{Bus, Envelope, Event};
pub use describe::Describe;
pub use engine::{Counters, Engine, FaultKind};
pub use error::Error;
pub use graph::{Graph, LayoutHint, NodeId, passed_input};
pub use image::{ContentHash, Image, Region};
pub use manifest::{
    Family, Manifest, ModuleKind, Needs, ParamHint, ParamKind, ParamSpec, ParamWidget, PixelFormat,
    PortKind, PortSpec, Role,
};
pub use message::{Arg, Message};
pub use module::{
    Accelerator, AcceleratorError, Context, DeviceImage, Extent, Failure, GpuJob, GpuKernel,
    Memory, Module, ModuleRef,
};
pub use network::{Composite, Expansion, Inner, InnerNode, Macro, Network, Wire};
pub use params::{Params, Value};
pub use scheduler::{CancelToken, JobId, JobSpec, JobState, Priority, Scheduler};
pub use status::{Load, NodeState, NodeStatus, NodeType};
