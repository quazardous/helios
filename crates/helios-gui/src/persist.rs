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
//!   this process sharing its bus) to the library, and its stacks again
//!   on `vault.stack_changed`;
//! - runs the vault query of the library's search filter, and keeps a
//!   stack expanded or collapsed as the user left it;
//! - saves the edit of the image shown ([`Persist::open_image`]), with
//!   the labels and locks of its mask nodes (its `marks`), as a
//!   version each time the value store commits a step: a write outside a
//!   gesture, the end of a gesture (a drag is one step, saved once when
//!   it ends), an undo, a redo. While a gesture goes on, what it changed
//!   is saved every [`FLUSH_SECONDS`] as a version labelled "in progress",
//!   so a crash in the middle of a long drag loses at most that much of
//!   it. Each save is announced on the bus as `edit.saved`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use helios_core::{Bus, Envelope, Graph, Message, NodeId, Params};
use helios_vault::fields::{Command, LABELS, PICKED, Query, RATING, REJECTED, TAGS};
use helios_vault::{EditSession, ImageId, Store, Vault, Version, edit};
use serde_json::Value;

use crate::Shell;
use crate::dialogs::Level;
use crate::library::{Change, Entry};

/// The fields the library shows.
const SHOWN: [&str; 5] = [RATING, REJECTED, PICKED, LABELS, TAGS];

/// Who the vault's audit trail names for what the desktop writes.
pub const WHO: &str = "user";

/// How often a gesture still going (a drag) is saved, in seconds.
pub const FLUSH_SECONDS: f64 = 1.0;

pub struct Persist<S: Store> {
    vault: Arc<Vault<S>>,
    bus: Arc<Bus>,
    events: Receiver<Envelope>,
    editing: Option<Editing>,
}

/// The edit of the image shown.
struct Editing {
    session: EditSession,
    /// The module each node of the shown network runs, by node name.
    modules: BTreeMap<String, String>,
    /// The value store's committed steps when last saved.
    committed: u64,
    /// When the gesture going on was last saved (or started).
    flushed_at: Option<f64>,
}

impl<S: Store> Persist<S> {
    /// A vault on `store`, announcing its changes on `bus`.
    pub fn new(store: S, bus: Arc<Bus>) -> Self {
        let events = bus.subscribe();
        Self {
            vault: Arc::new(Vault::new(store).with_bus(bus.clone())),
            bus,
            events,
            editing: None,
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
            // The file's size and date, for the library's details.
            if let Some(meta) = path.as_ref().and_then(|p| std::fs::metadata(p).ok()) {
                entry.bytes = Some(meta.len());
                entry.modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .and_then(|d| i64::try_from(d.as_secs()).ok());
                entry.day = entry.modified.and_then(crate::library::day_text);
            }
            for field in SHOWN {
                entry.show(field, row.fields.get(field));
            }
            entries.push(entry);
            if let Some(path) = path {
                files.push((id, path));
            }
        }
        shell.set_library(entries);
        self.show_stacks(shell)?;
        Ok(files)
    }

    /// Shows the vault's stacks in the library.
    pub fn show_stacks(&self, shell: &mut Shell) -> Result<(), String> {
        let stacks = self.vault.stacks()?;
        shell.library_mut().set_stacks(&stacks);
        Ok(())
    }

    /// The images the vault holds for a query, in its order.
    fn select(&self, query: Query) -> Result<Vec<String>, String> {
        let cursor = self.vault.open_cursor(query)?;
        let rows = self.vault.fetch(cursor.id, 0, cursor.count, &[]);
        self.vault.close_cursor(cursor.id);
        Ok(rows?.into_iter().map(|r| r.id.hex()).collect())
    }

    /// Shows an image's network in the viewport with the image's saved
    /// edit: its newest head, returned (none for an image never edited).
    /// From then on each step is saved as a version continuing it.
    pub fn open_image(
        &mut self,
        shell: &mut Shell,
        image: ImageId,
        mut graph: Graph,
        node: NodeId,
        port: &str,
    ) -> Result<Option<Version>, String> {
        let (session, head) = EditSession::open(&self.vault, image, WHO)?;
        if let Some(head) = &head {
            for skipped in edit::apply_to_graph(&mut graph, &head.edit)? {
                shell.notify(
                    Level::Warning,
                    Message::new(
                        "gui-edit-does-not-fit",
                        "Part of the saved edit does not fit this network: { $part }",
                    )
                    .with("part", skipped),
                );
            }
        }
        let modules = graph
            .ids()
            .filter_map(|id| {
                let manifest = graph.manifest(id).ok()??;
                Some((graph.name(id).ok()?.to_string(), manifest.name))
            })
            .collect();
        shell.set_view_source(graph, node, port)?;
        let committed = {
            let mut store = shell.values.lock().expect("value store lock");
            // The labels and locks it was saved with; another image's go.
            let marks = head.as_ref().map_or(&Value::Null, |h| &h.edit["marks"]);
            crate::mask_tree::restore_marks(&mut store, marks);
            store.committed()
        };
        self.editing = Some(Editing {
            session,
            modules,
            committed,
            flushed_at: None,
        });
        Ok(head)
    }

    /// The edit being saved, if an image is open.
    pub fn session(&self) -> Option<&EditSession> {
        self.editing.as_ref().map(|e| &e.session)
    }

    /// Writes what the user changed and shows what other writers did;
    /// `now` is the time in seconds (since the epoch for a real clock).
    pub fn frame(&mut self, shell: &mut Shell, now: f64) {
        let at = now.max(0.0) as u64;
        let changes = shell.take_library_changes();
        if !changes.is_empty() {
            self.write(shell, changes, at);
        }
        self.save_edit(shell, now);
        let expansions = shell.library_mut().take_expansions();
        for (stack, expanded) in expansions {
            if let Err(e) = self.vault.set_stack_expanded(&stack, expanded) {
                shell.notify(Level::Warning, e);
            }
        }
        let query = shell.library_mut().take_query();
        if let Some(query) = query {
            match self.select(query) {
                Ok(ids) => shell.library_mut().set_matches(ids),
                Err(e) => shell.notify(
                    Level::Error,
                    Message::new("gui-vault-unreadable", "Cannot read the vault: { $error }")
                        .with("error", e),
                ),
            }
        }
        let mut stacks_changed = false;
        while let Ok(event) = self.events.try_recv() {
            if event.kind == "vault.stack_changed" {
                stacks_changed = true;
            } else if event.kind == "vault.field_changed" {
                let p = &event.payload;
                if let (Some(image), Some(field)) = (p["image"].as_str(), p["field"].as_str()) {
                    let new = Some(&p["new"]).filter(|v| !v.is_null());
                    shell.library_mut().show_field(image, field, new);
                }
            }
        }
        if stacks_changed && let Err(e) = self.show_stacks(shell) {
            shell.notify(Level::Error, e);
        }
    }

    /// Saves the edit when the value store committed a step, or while a
    /// gesture goes on, every [`FLUSH_SECONDS`].
    fn save_edit(&mut self, shell: &mut Shell, now: f64) {
        let Some(e) = &mut self.editing else {
            return;
        };
        let at = now.max(0.0) as u64;
        let (committed, label, gesture, current) = {
            let store = shell.values.lock().expect("value store lock");
            let mut modules: BTreeMap<String, (String, Params)> = BTreeMap::new();
            for (path, entry) in store.list("node/") {
                let Some((node, param)) = path
                    .strip_prefix("node/")
                    .and_then(|rest| rest.rsplit_once('/'))
                else {
                    continue;
                };
                if let Some(module) = e.modules.get(node) {
                    modules
                        .entry(node.to_string())
                        .or_insert_with(|| (module.clone(), Params::new()))
                        .1
                        .set(param, entry.value.clone());
                }
            }
            let mut current = edit::from_modules(&modules);
            // The Masks panel's labels and locks, kept with the edit.
            let marks = crate::mask_tree::marks(&store);
            if !marks.is_empty() {
                current["marks"] = Value::Object(marks);
            }
            (
                store.committed(),
                store.last_committed().to_string(),
                store.open_gesture().map(String::from),
                current,
            )
        };
        let saved = if committed == e.committed {
            let Some(gesture) = gesture else {
                e.flushed_at = None;
                return;
            };
            let since = *e.flushed_at.get_or_insert(now);
            if now - since < FLUSH_SECONDS {
                return;
            }
            e.flushed_at = Some(now);
            e.session
                .save_in_progress(&self.vault, current, &gesture, at)
        } else {
            e.committed = committed;
            e.flushed_at = None;
            e.session.save(&self.vault, current, &label, at)
        };
        if let Err(err) = saved {
            shell.notify(
                Level::Error,
                Message::new("gui-edit-not-saved", "The edit was not saved: { $error }")
                    .with("error", err),
            );
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
                Message::new(
                    "gui-library-not-saved",
                    "Not saved, the library shows what the vault holds: { $error }",
                )
                .with("error", e),
            );
        }
        // Kept or not, show what the vault holds now.
        if let Err(e) = self.show_stored(shell, &touched) {
            shell.notify(
                Level::Error,
                Message::new("gui-vault-unreadable", "Cannot read the vault: { $error }")
                    .with("error", e),
            );
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
