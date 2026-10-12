//! What the desktop keeps in the vault, and how it hears about changes.
//!
//! The shell itself holds no vault: [`Persist`] is the host's link
//! between them. Once per frame ([`Persist::frame`]) it
//!
//! - writes what the user changed in the library, as field commands in one
//!   batch; whether the batch is kept or refused, the touched images are
//!   read back and shown as the vault holds them, so a failed write never
//!   leaves the grid showing a value that is not stored (and a field
//!   another process changed meanwhile shows too);
//! - applies the vault's `vault.field_changed` events (from any writer in
//!   this process sharing its bus) to the library.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use helios_core::{Bus, Envelope};
use helios_vault::fields::{Command, LABELS, Query, RATING, REJECTED};
use helios_vault::{ImageId, Store, Vault};
use serde_json::Value;

use crate::Shell;
use crate::dialogs::Level;
use crate::library::{Change, Entry};

/// The fields the library shows.
const SHOWN: [&str; 3] = [RATING, REJECTED, LABELS];

/// Who the vault's audit trail names for what the desktop writes.
pub const WHO: &str = "user";

pub struct Persist<S: Store> {
    vault: Arc<Vault<S>>,
    bus: Arc<Bus>,
    events: Receiver<Envelope>,
}

impl<S: Store> Persist<S> {
    /// A vault on `store`, announcing its changes on `bus`.
    pub fn new(store: S, bus: Arc<Bus>) -> Self {
        let events = bus.subscribe();
        Self {
            vault: Arc::new(Vault::new(store).with_bus(bus.clone())),
            bus,
            events,
        }
    }

    pub fn vault(&self) -> &Arc<Vault<S>> {
        &self.vault
    }

    pub fn bus(&self) -> &Arc<Bus> {
        &self.bus
    }

    /// Fills the library with the vault's images; returns where each one
    /// can be read (its first present copy), for thumbnails.
    pub fn fill_library(&self, shell: &mut Shell) -> Result<Vec<(String, PathBuf)>, String> {
        let cursor = self.vault.open_cursor(Query::default())?;
        let rows = self.vault.fetch(cursor.id, 0, cursor.count, &SHOWN);
        self.vault.close_cursor(cursor.id);
        let mut entries = Vec::new();
        let mut files = Vec::new();
        for row in rows? {
            let path = self
                .vault
                .locations(&row.id)?
                .into_iter()
                .find(|l| l.present)
                .map(|l| l.path);
            let id = row.id.hex();
            let mut entry = Entry {
                id: id.clone(),
                name: path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map_or_else(|| id[..12].to_string(), |n| n.to_string_lossy().into()),
                ..Entry::default()
            };
            for field in SHOWN {
                entry.show(field, row.fields.get(field));
            }
            entries.push(entry);
            if let Some(path) = path {
                files.push((id, path));
            }
        }
        shell.set_library(entries);
        Ok(files)
    }

    /// Writes what the user changed and shows what other writers did;
    /// `now` is the time written in the audit trail, in seconds.
    pub fn frame(&mut self, shell: &mut Shell, now: u64) {
        let changes = shell.take_library_changes();
        if !changes.is_empty() {
            self.write(shell, changes, now);
        }
        while let Ok(event) = self.events.try_recv() {
            if event.kind == "vault.field_changed" {
                let p = &event.payload;
                if let (Some(image), Some(field)) = (p["image"].as_str(), p["field"].as_str()) {
                    let new = Some(&p["new"]).filter(|v| !v.is_null());
                    shell.library_mut().show_field(image, field, new);
                }
            }
        }
    }

    fn write(&self, shell: &mut Shell, changes: Vec<Change>, now: u64) {
        let touched: BTreeSet<String> = changes.iter().map(|c| c.image.clone()).collect();
        let commands: Result<Vec<Command>, String> = changes
            .into_iter()
            .map(|c| Ok(Command::with(ImageId::from_hex(&c.image)?, &c.field, c.op)))
            .collect();
        if let Err(e) = commands.and_then(|c| self.vault.apply(&c, WHO, None, now)) {
            shell.notify(
                Level::Warning,
                format!("Not saved, the library shows what the vault holds: {e}"),
            );
        }
        // Kept or not, show what the vault holds now.
        if let Err(e) = self.show_stored(shell, &touched) {
            shell.notify(Level::Error, format!("Cannot read the vault: {e}"));
        }
    }

    /// Shows the stored fields of some images of the library.
    fn show_stored(&self, shell: &mut Shell, images: &BTreeSet<String>) -> Result<(), String> {
        let ids = images
            .iter()
            .map(|h| ImageId::from_hex(h))
            .collect::<Result<Vec<_>, _>>()?;
        let rows = self.vault.fields_of(&ids, &SHOWN)?;
        let mut library = shell.library_mut();
        for (id, fields) in ids.iter().zip(rows) {
            for field in SHOWN {
                library.show_field(
                    &id.hex(),
                    field,
                    fields.get(field).filter(|v| !Value::is_null(v)),
                );
            }
        }
        Ok(())
    }
}
