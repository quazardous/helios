//! Storage: what the shell keeps of the volumes the vault knows
//! (`helios_storage::registry`), the library's backup strategy
//! (`helios_storage::strategy`), each photo's backups and the cards
//! waiting for theirs.
//!
//! The shell holds no vault: the host gives a [`StorageLink`]
//! ([`VaultStorage`] over the vault and the machine's volumes, tests
//! their own). The same link names the volumes of offline photos in the
//! Library ([`crate::library::Library::set_volumes`]).
//!
//! The Storage page is the Settings app's Storage and Backup scopes
//! (`crate::config::manifest`), read and written through the commands of
//! [`super::storage_admin`]. `library.storage` opens it at Storage ›
//! Volumes; `{"volume": id}` marks that volume and scrolls to it (the
//! health check's free-space fix does).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::toasts::{TOASTS, Toast};
use egui_pop::core::icon::Icon;
use helios_core::Message;
use helios_library::import::Card;
use helios_shell::commands::Origin;
use helios_storage::strategy::{self, BackupStatus, Destination, Scope, Strategy};
use helios_storage::{Role, Volume, VolumeId, VolumeKind, Volumes, registry};
use helios_vault::{Store, Vault};
use serde_json::Value;

use super::Shell;

/// Opens the Settings app at Storage › Volumes; `volume` (an id) marks
/// that volume and scrolls to it.
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

/// What the shell keeps of storage.
#[derive(Default)]
pub(crate) struct StoragePage {
    link: Option<Arc<dyn StorageLink>>,
    volumes: Vec<Volume>,
    /// The library's backup strategy, as last read.
    strategy: Option<Strategy>,
    error: Option<String>,
    /// The cards an import left short of their backups: told safe to
    /// format once a backup job makes them so, while still inserted.
    waiting: Vec<Card>,
    /// The same, for the health check's thread.
    shared_waiting: Arc<Mutex<Vec<Card>>>,
}

impl Shell {
    /// Reads and writes volumes through `link`: the Storage page, and the
    /// names of offline photos' volumes in the Library.
    pub fn set_storage(&mut self, link: Arc<dyn StorageLink>) {
        self.register_storage_commands(&link);
        self.core.storage.link = Some(link);
        self.refresh_volumes();
    }

    /// Reads the volumes again: the page's list, and the volumes the
    /// Library names (a scan ran, a disk came or went).
    pub fn refresh_volumes(&mut self) {
        let Some(link) = self.core.storage.link.clone() else {
            return;
        };
        match link.volumes() {
            Ok(volumes) => {
                self.focus.library.borrow_mut().set_volumes(&volumes);
                self.core.storage.volumes = volumes;
                self.core.storage.error = None;
            }
            Err(e) => self.core.storage.error = Some(e),
        }
        // The import window's strategy line reads it too.
        match link.strategy() {
            Ok(s) => self.core.storage.strategy = s,
            Err(e) => self.core.storage.error = Some(e),
        }
        if let Some(e) = &self.core.storage.error {
            log::warn!("storage: {e}");
        }
    }

    /// Each photo's backups read again (one read for the Library), for
    /// its badges and its `backup:` filters.
    pub fn refresh_backups(&mut self) {
        let Some(link) = self.core.storage.link.clone() else {
            return;
        };
        match link.statuses() {
            Ok(statuses) => {
                let marks = statuses
                    .iter()
                    .map(|(id, s)| (id.clone(), crate::library::BackupMark::of(s)))
                    .collect();
                let badges = self.core.storage.strategy.is_some();
                self.focus.library.borrow_mut().set_backups(marks, badges);
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
            self.core.storage.waiting.retain(|c| c.path != card.path);
            self.core.storage.waiting.push(card);
        }
        self.share_waiting();
    }

    /// The cards waiting for their backups, as the health check's thread
    /// reads them (kept current by the shell).
    pub fn waiting_cards(&self) -> Arc<Mutex<Vec<Card>>> {
        self.core.storage.shared_waiting.clone()
    }

    fn share_waiting(&self) {
        let mut shared = self
            .core
            .storage
            .shared_waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        shared.clone_from(&self.core.storage.waiting);
    }

    /// The cards still waiting for their backups.
    pub fn cards_waiting_for_backups(&self) -> Vec<String> {
        self.core
            .storage
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
        let Some(link) = self.core.storage.link.clone() else {
            return;
        };
        let waiting = std::mem::take(&mut self.core.storage.waiting);
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
                    if let Some(toasts) = self.core.app.handle().service(&TOASTS) {
                        toasts.notify(Toast::success(&text));
                    }
                    self.core
                        .log
                        .borrow_mut()
                        .push(crate::dialogs::Level::Info, message);
                    self.notes.push(text);
                }
                Ok(_) => self.core.storage.waiting.push(card),
                Err(e) => {
                    log::warn!("{}: {e}", card.label);
                    self.core.storage.waiting.push(card);
                }
            }
        }
        self.share_waiting();
    }

    /// The import window's strategy line, and whether it is the one
    /// without a strategy ("No backup set up:", the Storage page offered):
    /// "2 copies: Backup disk (plugged in), NAS (not seen for 3 days)".
    pub fn import_backup_line(&self) -> (String, bool) {
        backup_line(
            &|m| self.text(m),
            self.core.storage.strategy.as_ref(),
            &self.core.storage.volumes,
        )
    }

    /// Whether the Settings app is open on its Storage or Backup scope.
    pub fn storage_page_open(&self) -> bool {
        let app = self.core.app.handle();
        app.service(&egui_pop::api::admin::ADMIN).is_some_and(|a| {
            a.is_open() && matches!(a.shown().0.as_deref(), Some("storage" | "backup"))
        })
    }

    /// The library's backup strategy, as last read.
    pub fn storage_strategy(&self) -> Option<&Strategy> {
        self.core.storage.strategy.as_ref()
    }

    /// The volumes known, online ones first.
    pub fn storage_volumes(&self) -> &[Volume] {
        &self.core.storage.volumes
    }

    /// `library.storage`: opens the Settings app at Storage › Volumes,
    /// on `volume` when given.
    pub(super) fn open_storage(&mut self, args: &Value) -> Result<Value, String> {
        self.refresh_volumes();
        let path = match args["volume"].as_str() {
            Some(v) => format!("storage/volumes/{v}"),
            None => "storage/volumes".into(),
        };
        self.run(
            egui_pop::api::admin::CMD_OPEN.as_str(),
            serde_json::json!({ "path": path }),
            Origin::User,
        )
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
pub(super) fn destination_text(
    t: &dyn Fn(Message) -> String,
    d: &Destination,
    volumes: &[Volume],
) -> String {
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

/// A kind's icon and words.
pub(super) fn kind_of(kind: VolumeKind) -> (Icon, Message) {
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
pub fn role_text(role: Role) -> Message {
    match role {
        Role::Library => Message::new("gui-storage-role-library", "Library"),
        Role::Archive => Message::new("gui-storage-role-archive", "Archive"),
        Role::Backup => Message::new("gui-storage-role-backup", "Backup"),
        Role::Ingest => Message::new("gui-storage-role-ingest", "Ingest"),
        Role::Scratch => Message::new("gui-storage-role-scratch", "Scratch"),
    }
}

/// When a volume was last seen, as a day.
pub(super) fn day_of(at: u64) -> String {
    i64::try_from(at)
        .ok()
        .and_then(crate::library::day_text)
        .unwrap_or_else(|| "?".into())
}

/// The strategy's line ([`Shell::import_backup_line`]) from a strategy
/// and the volumes known.
pub fn backup_line(
    t: &dyn Fn(Message) -> String,
    strategy: Option<&Strategy>,
    volumes: &[Volume],
) -> (String, bool) {
    let Some(s) = strategy else {
        return (
            t(Message::new("gui-import-backup-none", "No backup set up:")),
            true,
        );
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let online = strategy::destinations_online(s, volumes);
    let disks: Vec<String> = volumes
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
                    Some(0) => Message::new("gui-import-backup-today", "{ $disk } (seen today)"),
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
            t(m.with("disk", disk))
        })
        .collect();
    let disks = if disks.is_empty() {
        t(Message::new(
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
    (t(line.with("disks", disks)), false)
}
