//! External plugins, out of process (see `docs/extensions.md`, "Proof of
//! concept").
//!
//! A plugin is a directory holding a manifest, `helios-plugin.toml`, and
//! any executable that speaks JSON-RPC 2.0 over its standard input and
//! output, one message per line. The [`Broker`] reads the manifests
//! before anything runs, refuses those this Helios cannot honour, starts
//! a plugin on its first activation event, routes calls to it with a
//! timeout, and restarts it with backoff when it crashes: a crash costs
//! that plugin alone, and its caller gets an error.
//!
//! What is enforced is **crash isolation plus brokered data**: a plugin
//! is handed only the image paths it is asked about, writes downloads only
//! into a staging folder the broker checks and empties itself, and does
//! not start until the user grants every capability the broker cannot
//! police (running programs, USB, network). Holding the process itself to
//! those (namespaces, portals) comes later.
//!
//! - [`manifest`]: the manifest and its checks;
//! - [`rpc`]: one plugin process and its JSON-RPC line protocol;
//! - [`broker`]: discovery, activation, routing, capabilities, services;
//! - [`commands`]: the broker as shell commands.

pub mod broker;
pub mod commands;
pub mod manifest;
pub mod rpc;

pub use broker::{Broker, Notice, Policy, Scope, default_dirs};
pub use commands::register;
pub use manifest::{MANIFEST, Manifest};
