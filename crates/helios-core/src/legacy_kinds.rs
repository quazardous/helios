//! The fact kinds' names before the `helios.` prefix, kept for one
//! release so a desktop and a node of different releases still pair.
//!
//! Every fact kind Helios declares is named `helios.<subject>.<moment>`
//! (bus-contract's README, "Naming points"): its typed kinds are points of
//! the application's bus, shared with egui-pop's `pop.*` and `egui_stack`'s
//! `stack.*`. Up to Helios 0.92 they had no prefix (`vault.field_changed`,
//! `job.started`). Both sides of a node link translate, at the edge only,
//! so no bus and no journal ever holds an old name:
//!
//! - **A desktop reading an older node** maps each kind it reads from the
//!   node (an event, a relayed fact, a job `describe jobs` lists) to its
//!   new name ([`current`]) before anything hears it: the fact lands once,
//!   under its new name.
//! - **An older desktop reading a newer node** never says it reads
//!   prefixed names: the node then writes each kind of the streams it
//!   serves that desktop under its old name ([`legacy`]), and reads the
//!   prefix it asked for in the old names (`helios_inspect`'s
//!   `subscribe`, `prefixed`).
//!
//! Removed in Helios 0.94: from then on a desktop and a node pair only
//! from 0.93 on (ROADMAP, 2026-11-19).

/// What every fact kind Helios declares starts with.
pub const PREFIX: &str = "helios.";

/// The release that removes [`ALIASES`].
pub const REMOVED_IN: &str = "0.94.0";

/// Each fact kind's old name, then its name now: [`PREFIX`] followed by
/// the old one, but for `param.changed`, whose name with the prefix is
/// already the desktop's journal event for a step of values
/// (`helios.param.changed`): it is `helios.values.param_changed`.
pub const ALIASES: &[(&str, &str)] = &[
    ("engine.render_requested", "helios.engine.render_requested"),
    ("engine.step_computed", "helios.engine.step_computed"),
    ("engine.gpu_fell_back", "helios.engine.gpu_fell_back"),
    ("engine.gpu_lost", "helios.engine.gpu_lost"),
    ("engine.cache_settled", "helios.engine.cache_settled"),
    ("scheduler.job_queued", "helios.scheduler.job_queued"),
    ("scheduler.job_started", "helios.scheduler.job_started"),
    ("scheduler.job_ended", "helios.scheduler.job_ended"),
    ("scheduler.paused", "helios.scheduler.paused"),
    ("scheduler.idle", "helios.scheduler.idle"),
    ("job.started", "helios.job.started"),
    ("job.progress", "helios.job.progress"),
    ("job.item_failed", "helios.job.item_failed"),
    ("job.log", "helios.job.log"),
    ("job.cancelling", "helios.job.cancelling"),
    ("job.paused", "helios.job.paused"),
    ("job.resumed", "helios.job.resumed"),
    ("job.finished", "helios.job.finished"),
    ("job.failed", "helios.job.failed"),
    ("job.cancelled", "helios.job.cancelled"),
    ("ai.call", "helios.ai.call"),
    ("decision.answered", "helios.decision.answered"),
    ("decision.proposed", "helios.decision.proposed"),
    ("decision.pending", "helios.decision.pending"),
    ("export.written", "helios.export.written"),
    ("library.decision", "helios.library.decision"),
    ("control.command", "helios.control.command"),
    ("control.command.refused", "helios.control.command.refused"),
    ("library.imported", "helios.library.imported"),
    ("settings.layerChanged", "helios.settings.layerChanged"),
    ("volume.mounted", "helios.volume.mounted"),
    ("volume.unmounted", "helios.volume.unmounted"),
    ("image.state.requested", "helios.image.state.requested"),
    ("image.state.computing", "helios.image.state.computing"),
    ("image.state.ready", "helios.image.state.ready"),
    ("image.state.failed", "helios.image.state.failed"),
    ("image.state.cancelled", "helios.image.state.cancelled"),
    ("image.state.evicted", "helios.image.state.evicted"),
    ("vault.field_changed", "helios.vault.field_changed"),
    ("edit.saved", "helios.edit.saved"),
    ("vault.stack_changed", "helios.vault.stack_changed"),
    ("vault.version_added", "helios.vault.version_added"),
    ("vault.captures_set", "helios.vault.captures_set"),
    ("vault.changed_outside", "helios.vault.changed_outside"),
    ("vault.locations_changed", "helios.vault.locations_changed"),
    ("vault.library_changed", "helios.vault.library_changed"),
    ("promise.progress", "helios.promise.progress"),
    ("promise.done", "helios.promise.done"),
    (
        "vault.migration_progress",
        "helios.vault.migration_progress",
    ),
    ("import.automatic.held", "helios.import.automatic.held"),
    ("extract.run", "helios.extract.run"),
    ("extract.item", "helios.extract.item"),
    ("workflow.rows", "helios.workflow.rows"),
    ("workflow.run", "helios.workflow.run"),
    ("node.ready", "helios.node.ready"),
    ("node.render_done", "helios.node.render_done"),
    ("node.idle", "helios.node.idle"),
    ("control.action", "helios.control.action"),
    ("param.changed", "helios.values.param_changed"),
    ("gui.command.dispatched", "helios.gui.command.dispatched"),
    ("gui.command.done", "helios.gui.command.done"),
    ("gui.command.failed", "helios.gui.command.failed"),
    ("gui.dialog.answered", "helios.gui.dialog.answered"),
    ("gui.toast.shown", "helios.gui.toast.shown"),
    ("gui.edit.undone", "helios.gui.edit.undone"),
    ("gui.edit.redone", "helios.gui.edit.redone"),
    ("gui.edit.undo_failed", "helios.gui.edit.undo_failed"),
    ("gui.edit.redo_failed", "helios.gui.edit.redo_failed"),
    ("gui.history.changed", "helios.gui.history.changed"),
    ("gui.history.refused", "helios.gui.history.refused"),
    ("gui.event", "helios.gui.event"),
    ("node.jobs_forgotten", "helios.node.jobs_forgotten"),
    ("node.link", "helios.node.link"),
    ("node.resync", "helios.node.resync"),
];

/// The name now of the kind a peer of an older release calls `kind`;
/// `kind` itself when it is not an old name.
pub fn current(kind: &str) -> &str {
    ALIASES
        .iter()
        .find(|(old, _)| *old == kind)
        .map_or(kind, |(_, new)| new)
}

/// The name a peer of an older release knows the kind `kind` by; `kind`
/// itself when it had no other.
pub fn legacy(kind: &str) -> &str {
    ALIASES
        .iter()
        .find(|(_, new)| *new == kind)
        .map_or(kind, |(old, _)| old)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_new_name_is_the_old_one_prefixed_and_each_old_name_once() {
        for (old, new) in ALIASES {
            assert!(!old.starts_with(PREFIX), "{old}");
            if *old != "param.changed" {
                assert_eq!(new.strip_prefix(PREFIX), Some(*old), "{new}");
            }
        }
        let mut olds: Vec<_> = ALIASES.iter().map(|(old, _)| old).collect();
        olds.sort();
        olds.dedup();
        assert_eq!(olds.len(), ALIASES.len(), "an old name twice");
    }

    #[test]
    fn a_kind_maps_both_ways_and_others_stay() {
        assert_eq!(current("job.started"), "helios.job.started");
        assert_eq!(current("helios.job.started"), "helios.job.started");
        assert_eq!(current("pop.job.started"), "pop.job.started");
        assert_eq!(legacy("helios.vault.field_changed"), "vault.field_changed");
        assert_eq!(legacy("vault.field_changed"), "vault.field_changed");
        assert_eq!(legacy("helios.library.changed"), "helios.library.changed");
    }

    #[test]
    fn every_kind_this_crate_declares_has_its_old_name() {
        for decl in crate::bus::FACTS.iter().copied().flatten() {
            assert!(decl.kind.starts_with(PREFIX), "{}", decl.kind);
            assert_ne!(legacy(decl.kind), decl.kind, "{} has no alias", decl.kind);
        }
    }
}
