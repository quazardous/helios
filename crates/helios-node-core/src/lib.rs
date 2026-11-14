//! A Helios node without a window (docs/design/node-client-split.md): it
//! owns the vault ([`node::LocalNode`]), flushes it on its own schedule,
//! watches the volumes of the machine it runs on ([`volumes`]) and runs the
//! work that outlives a window ([`jobs`]: the backups and the health
//! check), the check's inputs gathered by [`health::HealthInputs`], and
//! keeps the person's and the libraries' settings layers ([`settings`]),
//! and says which init steps are done ([`init`]). Long work answers with
//! promises, durable handles to its result ([`promises`]).
//! Its verbs are named commands ([`commands`]), listed with their params'
//! schema and run by id. The desktop embeds one in its process; `helios-node --vault` runs the same
//! node headless.

pub mod calibrate;
pub mod commands;
pub mod cursor;
pub mod desktop;
pub mod endpoint;
pub mod health;
pub mod import_promise;
pub mod init;
pub mod job_promises;
pub mod jobs;
pub mod library;
pub mod map;
pub mod node;
pub mod phases;
pub mod promises;
pub mod quit;
pub mod reads;
pub mod settings;
pub mod volumes;
pub mod workshops;
