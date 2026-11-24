//! The decisions' node verbs, declared here (`helios_command`): list a
//! network's decisions, ask the model, accept a proposal, give or
//! withdraw an answer. The node answers them over its network's book.

use helios_command::schema::{count, named, params, string, text};
use helios_command::{CommandSpec, object};

/// The decisions' verbs.
pub fn verbs() -> Vec<CommandSpec> {
    let node = || ("node", string(), true);
    let who = || {
        (
            "who",
            text("who decided: the run's origin when not given"),
            false,
        )
    };
    vec![
        CommandSpec::new("decisions", "List the network's decisions", "Decisions").read_only(),
        CommandSpec::new(
            "decision_resolve",
            "Ask the model about the waiting decisions",
            "Decisions",
        ),
        CommandSpec::new(
            "decision_accept",
            "Accept a decision's proposal",
            "Decisions",
        )
        .params(object(&[node(), ("option", count(), false), who()])),
        CommandSpec::new("decision_decide", "Give a decision's answer", "Decisions")
            .params(object(&[node(), ("values", params(), true), who()])),
        CommandSpec::new(
            "decision_reask",
            "Withdraw a decision's answer",
            "Decisions",
        )
        .params(named("node")),
    ]
}
