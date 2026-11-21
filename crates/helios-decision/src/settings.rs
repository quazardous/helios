//! The decisions' settings, declared here: how decision nodes are
//! resolved, and how many model calls they make at once.

use helios_config::{Concern, Kind, Setting};

/// The decision nodes' settings.
pub const SETTINGS: &[Setting] = &[
    Setting {
        key: "system.ai.calls",
        concern: Concern::Machine,
        label: "Model calls at once",
        about: "How many calls to AI models (decision nodes) run at the same time.",
        kind: Kind::Number {
            min: 1.0,
            max: 64.0,
            default: 2.0,
            unit: "calls",
            integer: true,
        },
    },
    Setting {
        key: "project.decisions.mode",
        concern: Concern::Project,
        label: "Decision nodes",
        about: "How decision nodes are resolved by default in this project.",
        kind: Kind::Choice {
            options: &["full_ai", "ai_assisted", "human"],
            default: "ai_assisted",
        },
    },
];
