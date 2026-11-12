//! A Helios node without a window (docs/design/node-client-split.md): it
//! owns the vault ([`node::LocalNode`]), flushes it on its own schedule,
//! watches the volumes of the machine it runs on ([`volumes`]) and runs the
//! work that outlives a window ([`jobs`]: the backups and the health
//! check), the check's inputs gathered by [`health::HealthInputs`]. The
//! desktop embeds one in its process; `helios-node --vault` runs the same
//! node headless.

pub mod health;
pub mod jobs;
pub mod node;
pub mod quit;
pub mod volumes;
