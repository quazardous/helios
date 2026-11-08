//! The Storage page (Library > Storage…, `library.storage`): every volume
//! the vault knows (`helios_storage::registry`), online ones first, then
//! offline ones. Each shows its name (edited in place), kind, capacity and
//! free space, roles (toggles), the off-site and RAID flags the user sets,
//! the node a fixed disk belongs to, when and where an offline one was
//! last seen, and how many photos the library holds on it. A weakly
//! identified volume offers "Mark this disk…", which writes the identity
//! file only after the user reads why and confirms. Every edit is saved
//! to the registry at once.
//!
//! The shell holds no vault: the host gives a [`StorageLink`]
//! ([`VaultStorage`] over the vault and the machine's volumes, tests
//! their own). The same link names the volumes of offline photos in the
//! Library ([`crate::library::Library::set_volumes`]).
//!
//! `library.storage` takes `{"volume": id}` to open the page on one volume
//! (the health check's free-space fix does).
//!
//! Above the volumes, the library's backup strategy
//! (`helios_storage::strategy`): none, or how many verified copies, on
//! which disks (any backup disk, any archive disk, or one disk), whether
//! one must be off site and when copies are made; saved at once. "Back up
//! now" runs `library.backupNow`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use egui::Context as Egui;
use egui_pop::api::toasts::{TOASTS, Toast};
use egui_pop::core::icon::Icon;
use helios_core::Message;
use helios_library::import::Card;
use helios_shell::commands::Origin;
use helios_storage::strategy::{self, BackupStatus, Destination, Scope, Strategy, Trigger};
use helios_storage::{Identity, Role, Volume, VolumeId, VolumeKind, Volumes, registry};
use helios_vault::{Store, Vault};
use serde_json::Value;

use super::Shell;

/// Opens the Storage page; `volume` (an id) puts that volume first.
pub const COMMAND: &str = "library.storage";

/// Backs up now what is below the library's strategy.
pub const BACKUP_NOW: &str = "library.backupNow";

/// What the Storage page reads and writes.
pub trait StorageLink: Send + Sync {
    /// Every volume known: those mounted now (with their mount point and
    /// fresh free space), then the others, offline.
    fn volumes(&self) -> Result<Vec<Volume>, String>;
    /// How many copies of photos the library holds on each volume.
    fn photos(&self) -> Result<HashMap<VolumeId, usize>, String>;
    /// Saves what the user set (name, roles, off site, RAID).
    fn save(&self, volume: &Volume) -> Result<Volume, String>;
    /// Writes the identity file on a mounted volume (the user asked).
    fn mark(&self, volume: &Volume) -> Result<Volume, String>;
    /// The library's backup strategy; `None` when there is none.
    fn strategy(&self) -> Result<Option<Strategy>, String> {
        Ok(None)
    }
    /// Keeps (or with `None` removes) the library's backup strategy.
    fn set_strategy(&self, _strategy: Option<&Strategy>) -> Result<(), String> {
        Err("no backup strategy here".into())
    }
    /// Every image's backup status against the library's strategy, by id
    /// (`helios_storage::strategy::statuses`): one read for the whole
    /// Library.
    fn statuses(&self) -> Result<HashMap<String, BackupStatus>, String> {
        Ok(HashMap::new())
    }
    /// Whether a card an import read is safe to format now
    /// (`helios_library::import::card_safety`, from its files and their
    /// images).
    fn card_safety(&self, card: &Card) -> Result<Card, String> {
        Ok(card.clone())
    }
}

/// The link over a vault and the volumes mounted.
pub struct VaultStorage<S: Store> {
    vault: Arc<Vault<S>>,
    mounted: Arc<dyn Volumes>,
    /// What a marker names as the library that wrote it.
    library: String,
}

impl<S: Store> VaultStorage<S> {
    pub fn new(
        vault: Arc<Vault<S>>,
        mounted: Arc<dyn Volumes>,
        library: impl Into<String>,
    ) -> Self {
        Self {
            vault,
            mounted,
            library: library.into(),
        }
    }
}

/// `known` with what is mounted now: online ones first, each group by
/// name (as the registry sorts them).
pub fn merge(known: Vec<Volume>, mounted: &[Volume]) -> Vec<Volume> {
    let (mut online, offline): (Vec<Volume>, Vec<Volume>) = known
        .into_iter()
        .map(|mut v| {
            if let Some(m) = mounted.iter().find(|m| m.id == v.id) {
                v.mount.clone_from(&m.mount);
                v.free = m.free.or(v.free);
                v.capacity = m.capacity.or(v.capacity);
            }
            v
        })
        .partition(Volume::online);
    online.extend(offline);
    online
}

impl<S> StorageLink for VaultStorage<S>
where
    S: Store + Send + 'static,
{
    fn volumes(&self) -> Result<Vec<Volume>, String> {
        Ok(merge(
            registry::known(&self.vault)?,
            &self.mounted.volumes(),
        ))
    }

    fn photos(&self) -> Result<HashMap<VolumeId, usize>, String> {
        let mut counts = HashMap::new();
        for (_, location) in self.vault.all_locations()? {
            if let Some(v) = location.volume {
                *counts.entry(v).or_default() += 1;
            }
        }
        Ok(counts)
    }

    fn save(&self, volume: &Volume) -> Result<Volume, String> {
        registry::save(&self.vault, volume)
    }

    fn mark(&self, volume: &Volume) -> Result<Volume, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        registry::mark(&self.vault, volume, &self.library, now)
    }

    fn strategy(&self) -> Result<Option<Strategy>, String> {
        strategy::load(&self.vault, &Scope::Library)
    }

    fn set_strategy(&self, strategy: Option<&Strategy>) -> Result<(), String> {
        strategy::save(&self.vault, &Scope::Library, strategy)
    }

    fn statuses(&self) -> Result<HashMap<String, BackupStatus>, String> {
        let s = strategy::load(&self.vault, &Scope::Library)?;
        let volumes = registry::known(&self.vault)?;
        Ok(strategy::statuses(&self.vault, &volumes, s.as_ref())?
            .into_iter()
            .map(|(id, (status, _))| (id.hex(), status))
            .collect())
    }

    fn card_safety(&self, card: &Card) -> Result<Card, String> {
        helios_library::import::card_safety(
            &self.vault,
            &card.root,
            &card.path,
            &card.label,
            card.files.clone(),
        )
    }
}

/// The page's state, kept by the shell.
#[derive(Default)]
pub(crate) struct StoragePage {
    link: Option<Arc<dyn StorageLink>>,
    open: bool,
    volumes: Vec<Volume>,
    photos: HashMap<VolumeId, usize>,
    /// The names being typed, by volume.
    names: HashMap<VolumeId, String>,
    /// The volume asked for (the health check's fix): shown first.
    focus: Option<VolumeId>,
    /// The volume whose "Mark this disk" explanation is shown.
    marking: Option<VolumeId>,
    /// The library's backup strategy, as last read.
    strategy: Option<Strategy>,
    error: Option<String>,
    /// The cards an import left short of their backups: told safe to
    /// format once a backup job makes them so, while still inserted.
    waiting: Vec<Card>,
    /// The same, for the health check's thread.
    shared_waiting: Arc<Mutex<Vec<Card>>>,
}

/// What the page's controls asked.
enum Asked {
    Save(Volume),
    AskMark(VolumeId),
    Mark(Volume),
    /// "Forget what was seen here" (`library.forgetVolume`).
    Forget(VolumeId),
    CancelMark,
    Strategy(Option<Strategy>),
    BackupNow,
}

impl Shell {
    /// Reads and writes volumes through `link`: the Storage page, and the
    /// names of offline photos' volumes in the Library.
    pub fn set_storage(&mut self, link: Arc<dyn StorageLink>) {
        self.storage.link = Some(link);
        self.refresh_volumes();
    }

    /// Reads the volumes again: the page's list, and the volumes the
    /// Library names (a scan ran, a disk came or went).
    pub fn refresh_volumes(&mut self) {
        let Some(link) = self.storage.link.clone() else {
            return;
        };
        match link.volumes() {
            Ok(volumes) => {
                self.library.borrow_mut().set_volumes(&volumes);
                self.storage.volumes = volumes;
                self.storage.error = None;
            }
            Err(e) => self.storage.error = Some(e),
        }
        if self.storage.open {
            match link.photos() {
                Ok(p) => self.storage.photos = p,
                Err(e) => self.storage.error = Some(e),
            }
        }
        // The import window's strategy line reads it too.
        match link.strategy() {
            Ok(s) => self.storage.strategy = s,
            Err(e) => self.storage.error = Some(e),
        }
        let known: Vec<VolumeId> = self.storage.volumes.iter().map(|v| v.id.clone()).collect();
        self.storage.names.retain(|id, _| known.contains(id));
    }

    /// Each photo's backups read again (one read for the Library), for
    /// its badges and its `backup:` filters.
    pub fn refresh_backups(&mut self) {
        let Some(link) = self.storage.link.clone() else {
            return;
        };
        match link.statuses() {
            Ok(statuses) => {
                let marks = statuses
                    .iter()
                    .map(|(id, s)| (id.clone(), crate::library::BackupMark::of(s)))
                    .collect();
                let badges = self.storage.strategy.is_some();
                self.library.borrow_mut().set_backups(marks, badges);
            }
            Err(e) => log::warn!("backups: {e}"),
        }
    }

    /// The cards an import read, from its report (`cards`): those short
    /// of their backups wait for a backup job to end.
    pub(super) fn wait_for_backups(&mut self, report: &Value) {
        for card in report["cards"].as_array().into_iter().flatten() {
            if card["safe_to_format"] == true
                || card["missing"].as_u64().unwrap_or(0) > 0
                || card["short"].as_u64().unwrap_or(0) == 0
            {
                continue;
            }
            let Some(card) = card_of(card) else {
                continue;
            };
            self.storage.waiting.retain(|c| c.path != card.path);
            self.storage.waiting.push(card);
        }
        self.share_waiting();
    }

    /// The cards waiting for their backups, as the health check's thread
    /// reads them (kept current by the shell).
    pub fn waiting_cards(&self) -> Arc<Mutex<Vec<Card>>> {
        self.storage.shared_waiting.clone()
    }

    fn share_waiting(&self) {
        let mut shared = self
            .storage
            .shared_waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        shared.clone_from(&self.storage.waiting);
    }

    /// The cards still waiting for their backups.
    pub fn cards_waiting_for_backups(&self) -> Vec<String> {
        self.storage
            .waiting
            .iter()
            .map(|c| c.label.clone())
            .collect()
    }

    /// A backup job ended: the badges read again, and each card still
    /// inserted that waited for its backups checked again; one now safe
    /// to format says so ("`EOS_DIGITAL` is safe to format: every photo
    /// has its backups"). A card taken out is no longer followed.
    pub(super) fn backup_ended(&mut self) {
        self.refresh_backups();
        let Some(link) = self.storage.link.clone() else {
            return;
        };
        let waiting = std::mem::take(&mut self.storage.waiting);
        for card in waiting {
            if !card.path.is_dir() {
                continue;
            }
            match link.card_safety(&card) {
                Ok(now) if now.safe_to_format => {
                    let message = Message::new(
                        "gui-backup-card-safe",
                        "{ $card } is safe to format: every photo has its backups",
                    )
                    .with("card", now.label.clone());
                    let text = self.text(message.clone());
                    if let Some(toasts) = self.app.handle().service(&TOASTS) {
                        toasts.notify(Toast::success(&text));
                    }
                    self.log
                        .borrow_mut()
                        .push(crate::dialogs::Level::Info, message);
                    self.notes.push(text);
                }
                Ok(_) => self.storage.waiting.push(card),
                Err(e) => {
                    log::warn!("{}: {e}", card.label);
                    self.storage.waiting.push(card);
                }
            }
        }
        self.share_waiting();
    }

    /// The import window's strategy line, and whether it is the one
    /// without a strategy ("No backup set up:", the Storage page offered):
    /// "2 copies: Backup disk (plugged in), NAS (not seen for 3 days)".
    pub fn import_backup_line(&self) -> (String, bool) {
        let Some(s) = &self.storage.strategy else {
            return (
                self.text(Message::new("gui-import-backup-none", "No backup set up:")),
                true,
            );
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let online = strategy::destinations_online(s, &self.storage.volumes);
        let disks: Vec<String> = self
            .storage
            .volumes
            .iter()
            .filter(|v| !v.is_card() && s.destinations.iter().any(|d| d.takes(v)))
            .map(|v| {
                let disk = v.name.clone();
                let m = if online.iter().any(|o| o.id == v.id) {
                    Message::new("gui-import-backup-plugged", "{ $disk } (plugged in)")
                } else if v.online() {
                    Message::new(
                        "gui-import-backup-unusable",
                        "{ $disk } (plugged in, cannot take copies)",
                    )
                } else {
                    match v
                        .last_seen
                        .as_ref()
                        .map(|l| now.saturating_sub(l.at) / 86_400)
                    {
                        None => Message::new("gui-import-backup-never", "{ $disk } (never seen)"),
                        Some(0) => {
                            Message::new("gui-import-backup-today", "{ $disk } (seen today)")
                        }
                        Some(1) => Message::new(
                            "gui-import-backup-one-day",
                            "{ $disk } (not seen for 1 day)",
                        ),
                        Some(days) => Message::new(
                            "gui-import-backup-days",
                            "{ $disk } (not seen for { $days } days)",
                        )
                        .with("days", days),
                    }
                };
                self.text(m.with("disk", disk))
            })
            .collect();
        let disks = if disks.is_empty() {
            self.text(Message::new(
                "gui-import-backup-no-disk",
                "no disk for them yet",
            ))
        } else {
            disks.join(", ")
        };
        let line = if s.copies == 1 {
            Message::new("gui-import-backup-one", "1 copy: { $disks }")
        } else {
            Message::new("gui-import-backup-copies", "{ $copies } copies: { $disks }")
                .with("copies", u64::from(s.copies))
        };
        (self.text(line.with("disks", disks)), false)
    }

    /// Whether the Storage page is open.
    pub fn storage_page_open(&self) -> bool {
        self.storage.open
    }

    /// The library's backup strategy, as the Storage page shows it.
    pub fn storage_strategy(&self) -> Option<&Strategy> {
        self.storage.strategy.as_ref()
    }

    /// The volumes the Storage page lists, in its order.
    pub fn storage_volumes(&self) -> &[Volume] {
        &self.storage.volumes
    }

    /// `library.storage`: opens the page, on `volume` when given.
    pub(super) fn open_storage(&mut self, args: &Value) {
        self.storage.open = true;
        self.storage.focus = args["volume"].as_str().map(|v| VolumeId(v.to_string()));
        self.refresh_volumes();
    }

    /// After the panels: the page, when open.
    pub(super) fn storage_window(&mut self, ctx: &Egui) {
        if !self.storage.open {
            return;
        }
        let t = |m: Message| crate::i18n::text(ctx, m);
        let mut open = true;
        let mut asked = None;
        let max = ctx.content_rect().size() - egui::vec2(32.0, 32.0);
        let page = &mut self.storage;
        // The volume asked for first, then online ones, then offline.
        let mut order: Vec<usize> = (0..page.volumes.len()).collect();
        if let Some(focus) = &page.focus {
            order.sort_by_key(|&i| &page.volumes[i].id != focus);
        }
        egui::Window::new(t(Message::new("gui-storage-title", "Storage")))
            .id(egui::Id::unique("helios.storage.window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(620.0, 480.0).min(max))
            .max_size(max)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(t(Message::new(
                        "gui-storage-intro",
                        "The disks Helios has seen. Name them and say what each is for: Helios writes only to library, archive, backup and scratch disks.",
                    )))
                    .small(),
                );
                if let Some(e) = &page.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.separator();
                strategy_ui(ui, &t, page.strategy.as_ref(), &page.volumes, &mut asked);
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if page.volumes.is_empty() {
                        ui.weak(t(Message::new(
                            "gui-storage-none",
                            "No disk known yet: the scan runs in the background.",
                        )));
                    }
                    for i in order {
                        let volume = &page.volumes[i];
                        let count = page.photos.get(&volume.id).copied().unwrap_or(0);
                        let marking = page.marking.as_ref() == Some(&volume.id);
                        let focused = page.focus.as_ref() == Some(&volume.id);
                        let name = page
                            .names
                            .entry(volume.id.clone())
                            .or_insert_with(|| volume.name.clone());
                        volume_ui(ui, &t, volume, name, count, marking, focused, &mut asked);
                    }
                });
            });
        if !open {
            self.storage.open = false;
            self.storage.marking = None;
        }
        let Some(asked) = asked else {
            return;
        };
        let Some(link) = self.storage.link.clone() else {
            return;
        };
        let done = match asked {
            Asked::Save(v) => link.save(&v).map(|_| ()),
            Asked::Strategy(s) => link.set_strategy(s.as_ref()),
            Asked::BackupNow => self.run(BACKUP_NOW, Value::Null, Origin::User).map(|_| ()),
            Asked::AskMark(id) => {
                self.storage.marking = Some(id);
                Ok(())
            }
            Asked::Forget(id) => self
                .run(
                    super::import_window::FORGET_VOLUME,
                    serde_json::json!({ "volume": id.as_str() }),
                    Origin::User,
                )
                .map(|_| ()),
            Asked::CancelMark => {
                self.storage.marking = None;
                Ok(())
            }
            Asked::Mark(v) => {
                self.storage.marking = None;
                link.mark(&v).map(|marked| {
                    // Its record moved to the marker's id.
                    self.storage.names.remove(&v.id);
                    if self.storage.focus.as_ref() == Some(&v.id) {
                        self.storage.focus = Some(marked.id);
                    }
                })
            }
        };
        if let Err(e) = done {
            self.warn(format!("storage: {e}"));
        }
        self.refresh_volumes();
        self.refresh_backups();
    }
}

/// A card from an import's report line (`helios_library::import::Card`
/// as it serializes).
fn card_of(v: &Value) -> Option<Card> {
    let files = v["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let path = std::path::PathBuf::from(f.get(0)?.as_str()?);
            let id = f
                .get(1)
                .and_then(Value::as_str)
                .and_then(|h| helios_vault::ImageId::from_hex(h).ok());
            Some((path, id))
        })
        .collect();
    Some(Card {
        path: v["path"].as_str()?.into(),
        label: v["label"].as_str()?.to_string(),
        photos: v["photos"].as_u64().unwrap_or(0),
        missing: v["missing"].as_u64().unwrap_or(0),
        want: v["want"]
            .as_u64()
            .and_then(|w| u8::try_from(w).ok())
            .unwrap_or(0),
        short: v["short"].as_u64().unwrap_or(0),
        safe_to_format: v["safe_to_format"] == true,
        root: v["root"].as_str()?.into(),
        files,
    })
}

/// A destination's words: any disk of a role, or one disk by name.
fn destination_text(t: &dyn Fn(Message) -> String, d: &Destination, volumes: &[Volume]) -> String {
    match d {
        Destination::Role(Role::Archive) => {
            t(Message::new("gui-storage-dest-archive", "Any archive disk"))
        }
        Destination::Role(_) => t(Message::new("gui-storage-dest-backup", "Any backup disk")),
        Destination::Volume(id) => volumes
            .iter()
            .find(|v| &v.id == id)
            .map_or_else(|| id.to_string(), |v| v.name.clone()),
    }
}

/// The library's backup strategy and its editor.
fn strategy_ui(
    ui: &mut egui::Ui,
    t: &dyn Fn(Message) -> String,
    strategy: Option<&Strategy>,
    volumes: &[Volume],
    asked: &mut Option<Asked>,
) {
    ui.strong(t(Message::new("gui-storage-backup", "Backup")));
    let mut on = strategy.is_some();
    if ui
        .checkbox(
            &mut on,
            t(Message::new(
                "gui-storage-backup-on",
                "Keep verified backup copies of every photo",
            )),
        )
        .changed()
    {
        *asked = Some(Asked::Strategy(on.then(Strategy::default)));
    }
    let Some(current) = strategy else {
        ui.weak(t(Message::new(
            "gui-storage-backup-none",
            "No backup set up: each photo has one copy, in the Library.",
        )));
        return;
    };
    let mut edited = current.clone();
    ui.horizontal_wrapped(|ui| {
        let mut copies = u32::from(edited.copies);
        let count = ui.add(egui::DragValue::new(&mut copies).range(1..=5));
        edited.copies = u8::try_from(copies).unwrap_or(1);
        // The words after the count name it and the disks' menu.
        let words = ui.label(t(Message::new("gui-storage-backup-copies", "copies on")));
        count.labelled_by(words.id);
        // v1 edits one destination; a strategy may hold several.
        let first = edited
            .destinations
            .first()
            .cloned()
            .unwrap_or(Destination::Role(Role::Backup));
        let mut chosen = first.clone();
        let mut choices = vec![
            Destination::Role(Role::Backup),
            Destination::Role(Role::Archive),
        ];
        choices.extend(
            volumes
                .iter()
                .filter(|v| strategy::may_back_up_to(v).is_ok())
                .map(|v| Destination::Volume(v.id.clone())),
        );
        egui::ComboBox::from_id_salt("helios.storage.backup.destination")
            .selected_text(destination_text(t, &chosen, volumes))
            .show_ui(ui, |ui| {
                for c in choices {
                    let text = destination_text(t, &c, volumes);
                    ui.selectable_value(&mut chosen, c, text);
                }
            })
            .response
            .labelled_by(words.id);
        if chosen != first {
            edited.destinations = vec![chosen];
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(
            &mut edited.off_site_required,
            t(Message::new(
                "gui-storage-backup-off-site",
                "One of them off site",
            )),
        );
        for (trigger, text) in [
            (
                Trigger::OnImport,
                Message::new("gui-storage-backup-on-import", "After each import"),
            ),
            (
                Trigger::OnConnect,
                Message::new(
                    "gui-storage-backup-on-connect",
                    "When a backup disk is plugged in",
                ),
            ),
        ] {
            let mut on = edited.triggers.contains(&trigger);
            if ui.checkbox(&mut on, t(text)).changed() {
                if on {
                    edited.triggers.insert(trigger);
                } else {
                    edited.triggers.remove(&trigger);
                }
            }
        }
    });
    if edited != *current {
        *asked = Some(Asked::Strategy(Some(edited)));
    }
    if ui
        .button(t(Message::new("gui-storage-backup-now", "Back up now")))
        .on_hover_text(t(Message::new(
            "gui-storage-backup-now-hint",
            "Copies the photos short of their backups to the disks plugged in, verified",
        )))
        .clicked()
    {
        *asked = Some(Asked::BackupNow);
    }
}

/// A kind's icon and words.
fn kind_of(kind: VolumeKind) -> (Icon, Message) {
    match kind {
        VolumeKind::Internal => (
            Icon::Home,
            Message::new("gui-storage-kind-internal", "Internal disk"),
        ),
        VolumeKind::External => (
            Icon::Save,
            Message::new("gui-storage-kind-external", "External disk"),
        ),
        VolumeKind::Removable => (
            Icon::File,
            Message::new("gui-storage-kind-removable", "Card or stick"),
        ),
        VolumeKind::Share => (
            Icon::Folder,
            Message::new("gui-storage-kind-share", "Network share"),
        ),
        VolumeKind::Remote => (
            Icon::Up,
            Message::new("gui-storage-kind-remote", "Remote store"),
        ),
    }
}

/// A role's words.
fn role_text(role: Role) -> Message {
    match role {
        Role::Library => Message::new("gui-storage-role-library", "Library"),
        Role::Archive => Message::new("gui-storage-role-archive", "Archive"),
        Role::Backup => Message::new("gui-storage-role-backup", "Backup"),
        Role::Ingest => Message::new("gui-storage-role-ingest", "Ingest"),
        Role::Scratch => Message::new("gui-storage-role-scratch", "Scratch"),
    }
}

/// When a volume was last seen, as a day.
fn day_of(at: u64) -> String {
    i64::try_from(at)
        .ok()
        .and_then(crate::library::day_text)
        .unwrap_or_else(|| "?".into())
}

/// One volume.
#[allow(clippy::too_many_arguments)]
fn volume_ui(
    ui: &mut egui::Ui,
    t: &dyn Fn(Message) -> String,
    volume: &Volume,
    name: &mut String,
    photos: usize,
    marking: bool,
    focused: bool,
    asked: &mut Option<Asked>,
) {
    let frame = egui::Frame::group(ui.style());
    let frame = if focused {
        frame.stroke(egui::Stroke::new(1.5, ui.visuals().selection.stroke.color))
    } else {
        frame
    };
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        let (icon, kind) = kind_of(volume.kind);
        let kind = t(kind);
        ui.horizontal(|ui| {
            ui.add(icon.label(&kind));
            let edit = ui.add(
                egui::TextEdit::singleline(name)
                    .id(egui::Id::unique(("helios.storage.name", volume.id.as_str())))
                    .desired_width(220.0),
            );
            let edit = edit.on_hover_text(t(Message::new(
                "gui-storage-name-hint",
                "Its name; Enter saves it",
            )));
            if edit.lost_focus() && name.trim() != volume.name {
                *asked = Some(Asked::Save(Volume {
                    name: name.clone(),
                    ..volume.clone()
                }));
            }
            let state = if volume.online() {
                t(Message::new("gui-storage-online", "online"))
            } else {
                t(Message::new("gui-storage-offline", "offline"))
            };
            ui.weak(format!("{kind} · {state}"));
        });
        // Capacity and free space.
        if let Some(capacity) = volume.capacity.filter(|&c| c > 0) {
            let free = volume.free.unwrap_or(0).min(capacity);
            let used = (capacity - free) as f32 / capacity as f32;
            let text = t(Message::new("gui-storage-free", "{ $free } free of { $capacity }")
                .with("free", helios_storage::capacity_text(free))
                .with("capacity", helios_storage::capacity_text(capacity)));
            ui.add(
                egui::ProgressBar::new(used)
                    .desired_width(ui.available_width().min(360.0))
                    .text(text),
            );
        }
        // Roles.
        ui.horizontal_wrapped(|ui| {
            ui.label(t(Message::new("gui-storage-roles", "For:")));
            for role in Role::ALL {
                let mut on = volume.roles.contains(&role);
                if ui.checkbox(&mut on, t(role_text(role))).changed() {
                    let mut v = volume.clone();
                    if on {
                        v.roles.insert(role);
                    } else {
                        v.roles.remove(&role);
                    }
                    *asked = Some(Asked::Save(v));
                }
            }
        });
        // The user's flags.
        ui.horizontal_wrapped(|ui| {
            let mut off_site = volume.off_site;
            if ui
                .checkbox(&mut off_site, t(Message::new("gui-storage-off-site", "Off site")))
                .on_hover_text(t(Message::new(
                    "gui-storage-off-site-hint",
                    "Kept away from the other disks (another building)",
                )))
                .changed()
            {
                *asked = Some(Asked::Save(Volume {
                    off_site,
                    ..volume.clone()
                }));
            }
            let mut raid = volume.raid;
            if ui
                .checkbox(&mut raid, t(Message::new("gui-storage-raid", "RAID")))
                .changed()
            {
                *asked = Some(Asked::Save(Volume {
                    raid,
                    ..volume.clone()
                }));
            }
            if volume.raid {
                ui.weak(t(Message::new(
                    "gui-storage-raid-note",
                    "A RAID counts as one copy: it survives a dead disk, not a deletion or a theft.",
                )));
            }
        });
        let mut facts = Vec::new();
        if volume.kind.is_fixed()
            && let Some(node) = &volume.node
        {
            facts.push(t(
                Message::new("gui-storage-node", "In { $node }").with("node", node.clone())
            ));
        }
        if !volume.online()
            && let Some(seen) = &volume.last_seen
        {
            facts.push(t(Message::new(
                "gui-storage-last-seen",
                "Last seen { $date } on { $node }",
            )
            .with("date", day_of(seen.at))
            .with("node", seen.node.clone())));
        }
        facts.push(t(
            Message::new(
                "gui-storage-photos",
                "Photos on it: { $count }",
            )
            .with("count", photos),
        ));
        ui.label(egui::RichText::new(facts.join(" · ")).small());
        // A weak identity: the identity file, on the user's say only (a
        // card is never written).
        if volume.identified_by == Identity::Weak && volume.online() && !volume.is_card() {
            if marking {
                ui.label(t(Message::new(
                    "gui-storage-mark-explain",
                    "This disk is known only by its label and size, so Helios could take another disk for it. Marking it writes a small file, .helios/volume.json, at its root: nothing else on the disk changes.",
                )));
                ui.horizontal(|ui| {
                    if ui
                        .button(t(Message::new(
                            "gui-storage-mark-confirm",
                            "Write the identity file",
                        )))
                        .clicked()
                    {
                        *asked = Some(Asked::Mark(volume.clone()));
                    }
                    if ui
                        .button(t(Message::new("gui-storage-mark-cancel", "Cancel")))
                        .clicked()
                    {
                        *asked = Some(Asked::CancelMark);
                    }
                });
            } else if ui
                .button(t(Message::new("gui-storage-mark", "Mark this disk…")))
                .on_hover_text(t(Message::new(
                    "gui-storage-mark-hint",
                    "Known only by its label and size: an identity file tells it apart",
                )))
                .clicked()
            {
                *asked = Some(Asked::AskMark(volume.id.clone()));
            }
        }
        if ui
            .button(t(Message::new(
                "gui-storage-forget",
                "Forget what was seen here",
            )))
            .on_hover_text(t(Message::new(
                "gui-storage-forget-hint",
                "Drops what the image map knows of this volume's files not in the Library; the volume stays known",
            )))
            .clicked()
        {
            *asked = Some(Asked::Forget(volume.id.clone()));
        }
    });
}
