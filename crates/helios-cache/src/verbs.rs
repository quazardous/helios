//! The cache service's node verbs, declared here (`helios_command`): what
//! the caches hold, and clearing them. The node answers them from this
//! process's [`crate::Service`].

use helios_command::schema::{or_null, string};
use helios_command::{CommandSpec, object};

/// The cache service's verbs.
pub fn verbs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("cache.stats", "Show what the caches hold", "Cache").read_only(),
        CommandSpec::new("cache.clear", "Clear the caches", "Cache").params(object(&[(
            "kind",
            or_null(string()),
            false,
        )])),
    ]
}
