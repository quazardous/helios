//! Process bricks: the registered, described functions modules are made of
//! (`docs/design/process-bricks.md`).
//!
//! - An **image brick** is a module offering the brick contract
//!   ([`helios_core::Module::brick`]): a namespaced id with a major version,
//!   a shape, a tolerance, typed and described ports. It runs through the
//!   engine as any module (regions, cache, GPU, the test kit).
//! - A **value brick** ([`ValueBrick`]) is a pure function of numbers,
//!   evaluated before any pixel: what darktable's `commit_params` does in
//!   code, written as data.
//! - The [`BrickRegistry`] holds them by `id@major`, checks them when
//!   registered, and answers [`BrickRegistry::catalogue`] and
//!   [`BrickRegistry::describe`] as plain JSON.
//! - An [`Assembly`] is a module written as data: its parameters, a value
//!   graph, a small graph of image bricks and its control patches;
//!   [`Assembled`] checks one and is the composite the engine expands.

pub mod affine;
pub mod assembly;
pub mod registry;
pub mod values;

pub use affine::Affine;
pub use assembly::{Assembled, Assembly, ControlDecl};
pub use registry::{Brick, BrickRegistry};
pub use values::ValueBrick;
