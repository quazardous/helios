//! The node's own settings, declared by the node.

use helios_config::{Concern, Kind, Setting};

/// The node's own settings: its worker threads, its bus's event journal
/// and how long its finished batches are kept. `helios-node` reads them
/// when it starts the node.
pub const SETTINGS: &[Setting] = &[
    Setting {
        key: "system.scheduler.workers",
        concern: Concern::Machine,
        label: "Worker threads",
        about: "How many jobs run at once on this machine; 0 means one per CPU.",
        kind: Kind::Number {
            min: 0.0,
            max: 1024.0,
            default: 0.0,
            unit: "threads",
            integer: true,
        },
    },
    Setting {
        key: "system.events.journal",
        concern: Concern::Machine,
        label: "Event journal",
        about: "How many recent events the bus keeps for inspection and waits.",
        kind: Kind::Number {
            min: 16.0,
            max: 1_000_000.0,
            default: 1024.0,
            unit: "events",
            integer: true,
        },
    },
    Setting {
        key: "system.workflow.keep_days",
        concern: Concern::Project,
        label: "Finished batches kept",
        about: "How long a finished batch (a run of the workflow level) stays in the vault, failures included, before it is removed whole; 0 removes it when the node next starts.",
        kind: Kind::Number {
            min: 0.0,
            max: 3650.0,
            default: 30.0,
            unit: "days",
            integer: true,
        },
    },
];
