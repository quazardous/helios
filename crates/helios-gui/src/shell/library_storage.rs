//! The Library's listener of what storage read (`storage.read`,
//! [`crate::storage_read`]): the volumes it names for offline photos, and
//! each photo's backup badge and `backup:` filter. Storage passes what it
//! read through the point; it does not write into the Library.

use super::Shell;
use crate::library::BackupMark;
use crate::storage_read;

impl Shell {
    /// `library.storage`, the Library's listener of `storage.read`.
    pub(super) fn listen_storage_read(&mut self) {
        let library = self.focus.library.clone();
        let listened = storage_read::listen_local(
            &*self.core.app.handle().bus(),
            storage_read::LIBRARY,
            move |read, _| {
                let mut library = library.borrow_mut();
                if let Some(volumes) = &read.volumes {
                    library.set_volumes(volumes);
                }
                if let Some(backups) = &read.backups {
                    let marks = backups
                        .statuses
                        .iter()
                        .map(|(id, s)| (id.clone(), BackupMark::of(s)))
                        .collect();
                    library.set_backup_rule(backups.rule.clone());
                    library.set_backups(marks, backups.badges);
                }
            },
        );
        if let Err(e) = listened {
            self.notes.push(format!("{}: {e}", storage_read::READ));
        }
    }
}
