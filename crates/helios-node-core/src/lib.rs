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
pub mod image_states;
pub mod import_profiles;
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

/// The fact kinds this crate publishes.
pub const FACTS: &[helios_core::FactDecl] = {
    use helios_core::FactDecl;
    use image_states::facts;
    const STATE: &str = "{ photo, development, state, size, priority, reason?, bytes? }";
    &[
        FactDecl::of::<endpoint::CommandRan<'static>>("A command is run, by whom"),
        FactDecl::of::<endpoint::CommandRefused<'static>>(
            "A command run refused, by whom, and why",
        ),
        FactDecl::of::<endpoint::Imported<'static>>("An import done through the node ended")
            .relayable(),
        FactDecl::of::<settings::LayerChanged>("A settings layer the node keeps changed")
            .relayable(),
        // Their typed points are this crate's own (`volumes::POINTS`).
        FactDecl::of::<volumes::Mounted>(
            "A volume mounted: plugged in, or there at the first look",
        )
        .fact_only(),
        FactDecl::of::<volumes::Unmounted>("A volume gone: unplugged, or unmounted").fact_only(),
        FactDecl::json(
            facts::REQUESTED,
            STATE,
            "An image state nobody computes yet was asked for",
        ),
        FactDecl::json(
            facts::COMPUTING,
            STATE,
            "An image state's computation started",
        ),
        FactDecl::json(facts::READY, STATE, "An image state is ready"),
        FactDecl::json(facts::FAILED, STATE, "An image state's computation failed"),
        FactDecl::json(
            facts::CANCELLED,
            STATE,
            "Every asker of an image state cancelled",
        ),
        FactDecl::json(facts::EVICTED, STATE, "A ready image state was let go"),
    ]
};
