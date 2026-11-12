//! The node's backups and health check ([`helios_node_core::jobs`]), and
//! the window's command that starts a backup.

use std::sync::Arc;

use helios_vault::Store;

pub use helios_node_core::jobs::*;

/// Registers `library.backupNow` on `shell`: a window's command, the
/// copies run by the node's `backups`.
pub fn register_back_up_now<S: Store + 'static>(
    shell: &mut crate::Shell,
    backups: &Arc<Backups<S>>,
) -> Result<(), String>
where
    Backups<S>: Send + Sync,
{
    let backups = backups.clone();
    shell.register(
        helios_shell::commands::CommandSpec::new(
            crate::shell::BACKUP_NOW_COMMAND,
            "Back up now: copy the photos short of the library's backup strategy to its disks plugged in, verified, as a job",
            "Library",
        ),
        move |_, _| {
            backups.back_up_now()?;
            Ok(serde_json::json!({ "queued": true }))
        },
    )
}
