//! Storage: what the shell keeps of the volumes the vault knows
//! (`helios_storage::registry`), the library's backup strategy
//! (`helios_storage::strategy`), each photo's backups and the cards
//! waiting for theirs.
//!
//! The shell reaches no catalogue itself: the host gives a
//! [`StorageLink`] ([`NodeStorage`] over the desktop's node and the
//! machine's volumes, read and written off the frame). The same link names the volumes of offline photos in the
//! Library ([`crate::library::Library::set_volumes`]).
//!
//! The Storage page is the Settings app's Storage and Backup scopes
//! (`crate::config::manifest`), read and written through the commands of
//! [`super::storage_admin`]. `library.storage` opens it at Storage ›
//! Volumes; `{"volume": id}` marks that volume and scrolls to it (the
//! health check's free-space fix does).

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};

use egui_pop::api::toasts::{TOASTS, Toast};
use egui_pop::core::icon::Icon;
use helios_core::Message;
use helios_inspect::client::Client;
use helios_library::import::Card;
use helios_storage::strategy::{self, BackupStatus, Destination, Strategy};
use helios_storage::{Role, Volume, VolumeId, VolumeKind, Volumes, registry};
use serde_json::{Value, json};

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
    /// Reads everything again, off the caller's thread when the link
    /// reads through a node; [`StorageLink::seq`] moves once it is read.
    fn refresh(&self) {}
    /// Moves each time what the link answers changed: the shell shows it
    /// again at its next frame.
    fn seq(&self) -> u64 {
        0
    }
}

/// The link over the desktop's node and the volumes mounted
/// (`library.storage.get` and the `storage.*` commands).
///
/// Nothing here waits for the node on the caller's thread: the reads
/// answer what was last read, and a thread of its own runs the writes in
/// the order made, then reads the volumes, the strategy and the backups
/// again ([`StorageLink::refresh`] asks the same). Each read that comes
/// back moves [`StorageLink::seq`], so the shell shows it at a coming
/// frame.
pub struct NodeStorage {
    mounted: Arc<dyn Volumes>,
    /// What a marker names as the library that wrote it.
    library: String,
    node: Weak<Client>,
    cached: Arc<Cached>,
    jobs: Sender<Job>,
}

/// What [`NodeStorage`] last read, and the jobs not yet done.
#[derive(Default)]
struct Cached {
    read: Mutex<Read>,
    /// Jobs sent and not yet done, and the condition
    /// [`NodeStorage::settle`] waits on.
    pending: Mutex<usize>,
    done: Condvar,
}

#[derive(Default)]
struct Read {
    known: Vec<Volume>,
    photos: HashMap<VolumeId, usize>,
    strategy: Option<Strategy>,
    statuses: HashMap<String, BackupStatus>,
    seq: u64,
}

enum Job {
    Read,
    Save(Volume),
    Mark(Volume, String),
    Strategy(Option<Strategy>),
}

impl NodeStorage {
    /// The link over `node`, read once at once (off the caller's thread).
    pub fn new(node: &Arc<Client>, mounted: Arc<dyn Volumes>, library: impl Into<String>) -> Self {
        let (jobs, queue) = std::sync::mpsc::channel();
        let cached = Arc::new(Cached::default());
        let (weak, c) = (Arc::downgrade(node), cached.clone());
        let spawned = std::thread::Builder::new()
            .name("storage-node".into())
            .spawn(move || work(&weak, &c, &queue));
        if let Err(e) = spawned {
            log::warn!("storage: no thread to reach the node: {e}");
        }
        let link = Self {
            mounted,
            library: library.into(),
            node: Arc::downgrade(node),
            cached,
            jobs,
        };
        link.send(Job::Read);
        link
    }

    fn send(&self, job: Job) {
        *self
            .cached
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner) += 1;
        if self.jobs.send(job).is_err() {
            self.cached.finished(1);
        }
    }

    fn read(&self) -> std::sync::MutexGuard<'_, Read> {
        self.cached
            .read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits until every job sent is done, `timeout` at most: whether it
    /// was.
    pub fn settle(&self, timeout: std::time::Duration) -> bool {
        let pending = self
            .cached
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (left, _) = self
            .cached
            .done
            .wait_timeout_while(pending, timeout, |n| *n > 0)
            .unwrap_or_else(PoisonError::into_inner);
        *left == 0
    }

    /// Whether a job sent is not done yet.
    pub fn busy(&self) -> bool {
        *self
            .cached
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            > 0
    }
}

impl Cached {
    fn finished(&self, n: usize) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        *pending = pending.saturating_sub(n);
        if *pending == 0 {
            self.done.notify_all();
        }
    }
}

/// The link's thread: each batch of jobs waiting, the writes in order,
/// then one read of all the page shows.
fn work(node: &Weak<Client>, cached: &Cached, queue: &Receiver<Job>) {
    while let Ok(first) = queue.recv() {
        let batch: Vec<Job> = std::iter::once(first).chain(queue.try_iter()).collect();
        let n = batch.len();
        if let Some(node) = node.upgrade() {
            for job in batch {
                let written = match job {
                    Job::Read => Ok(Value::Null),
                    Job::Save(v) => node.request("storage.volume.save", json!({ "volume": v })),
                    Job::Mark(v, library) => node.request(
                        "storage.volume.mark",
                        json!({ "volume": v, "mount": v.mount, "library": library }),
                    ),
                    Job::Strategy(s) => {
                        node.request("storage.strategy.set", json!({ "strategy": s }))
                    }
                };
                if let Err(e) = written {
                    log::warn!("storage: {}", e.message);
                }
            }
            match read_all(&node) {
                Ok(mut read) => {
                    let mut kept = cached.read.lock().unwrap_or_else(PoisonError::into_inner);
                    read.seq = kept.seq + 1;
                    *kept = read;
                }
                Err(e) => log::warn!("storage: {e}"),
            }
        }
        cached.finished(n);
    }
}

/// The volumes known, the photos on each, the strategy and the backups,
/// read through `node`.
fn read_all(node: &Client) -> Result<Read, String> {
    let ask = |method: &str| node.request(method, json!({})).map_err(|e| e.message);
    let stored = ask("library.storage.get")?;
    let volumes = ask("storage.volumes")?;
    let statuses = ask("storage.statuses")?;
    let field = |v: &Value, what: &str| -> Result<Value, String> {
        v.get(what)
            .cloned()
            .ok_or_else(|| format!("no `{what}` read"))
    };
    fn from<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, String> {
        serde_json::from_value(v).map_err(|e| e.to_string())
    }
    let photos: HashMap<String, usize> = from(field(&volumes, "photos")?)?;
    Ok(Read {
        known: from(field(&stored, "volumes")?)?,
        strategy: from(field(&stored, "strategy")?)?,
        photos: photos
            .into_iter()
            .map(|(id, n)| (VolumeId(id), n))
            .collect(),
        statuses: from(field(&statuses, "statuses")?)?,
        seq: 0,
    })
}

impl StorageLink for NodeStorage {
    fn volumes(&self) -> Result<Vec<Volume>, String> {
        let known = self.read().known.clone();
        Ok(registry::merge(known, &self.mounted.volumes()))
    }

    fn photos(&self) -> Result<HashMap<VolumeId, usize>, String> {
        Ok(self.read().photos.clone())
    }

    /// Kept as `registry::save` keeps it and shown at once; written in
    /// turn.
    fn save(&self, volume: &Volume) -> Result<Volume, String> {
        let saved = {
            let mut read = self.read();
            let record = read
                .known
                .iter_mut()
                .find(|v| v.id == volume.id)
                .ok_or_else(|| format!("no volume {}", volume.id))?;
            let name = volume.name.trim();
            record.name = if name.is_empty() {
                record.label.clone().unwrap_or_else(|| record.name.clone())
            } else {
                name.to_string()
            };
            record.roles.clone_from(&volume.roles);
            record.off_site = volume.off_site;
            record.raid = volume.raid;
            let saved = Volume {
                mount: volume.mount.clone(),
                ..record.clone()
            };
            read.seq += 1;
            saved
        };
        self.send(Job::Save(saved.clone()));
        Ok(saved)
    }

    /// Written in turn by the node: the volume answered is the one asked,
    /// its new id read back with the others.
    fn mark(&self, volume: &Volume) -> Result<Volume, String> {
        self.send(Job::Mark(volume.clone(), self.library.clone()));
        Ok(volume.clone())
    }

    fn strategy(&self) -> Result<Option<Strategy>, String> {
        Ok(self.read().strategy.clone())
    }

    fn set_strategy(&self, strategy: Option<&Strategy>) -> Result<(), String> {
        {
            let mut read = self.read();
            read.strategy = strategy.cloned();
            read.seq += 1;
        }
        self.send(Job::Strategy(strategy.cloned()));
        Ok(())
    }

    fn statuses(&self) -> Result<HashMap<String, BackupStatus>, String> {
        Ok(self.read().statuses.clone())
    }

    /// Asks the node and waits for it: the shell calls it off its frame.
    fn card_safety(&self, card: &Card) -> Result<Card, String> {
        let node = self.node.upgrade().ok_or("the node is gone")?;
        let files: Vec<Value> = card
            .files
            .iter()
            .map(|(path, id)| json!([path, id.as_ref().map(helios_vault::ImageId::hex)]))
            .collect();
        let answer = node
            .request(
                "storage.card.safety",
                json!({ "root": card.root, "path": card.path, "label": card.label, "files": files }),
            )
            .map_err(|e| e.message)?;
        let mut now = card_of(&answer).ok_or("no card answered")?;
        now.files.clone_from(&card.files);
        Ok(now)
    }

    fn refresh(&self) {
        self.send(Job::Read);
    }

    fn seq(&self) -> u64 {
        self.read().seq
    }
}

/// The waiting cards checked again: each one's path and what it is now.
type Checked = Vec<(std::path::PathBuf, Result<Card, String>)>;

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
    /// The link's [`StorageLink::seq`] last shown.
    seen: u64,
    /// The waiting cards checked again off the frame, once a backup
    /// ended: each one's path and what it is now.
    checking: Option<Receiver<Checked>>,
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
    /// Library names (a scan ran, a disk came or went). What the link
    /// reads off the frame shows at a coming one.
    pub fn refresh_volumes(&mut self) {
        if let Some(link) = &self.core.storage.link {
            link.refresh();
        }
        self.show_volumes();
    }

    /// The volumes and the strategy as the link answers them now.
    fn show_volumes(&mut self) {
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
    /// its badges and its `backup:` filters; what the link reads off the
    /// frame shows at a coming one.
    pub fn refresh_backups(&mut self) {
        if let Some(link) = &self.core.storage.link {
            link.refresh();
        }
        self.show_backups();
    }

    /// Each photo's backups as the link answers them now.
    fn show_backups(&mut self) {
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
                let rule = strategy::rule(
                    &self.core.storage.volumes,
                    self.core.storage.strategy.as_ref(),
                );
                let mut library = self.focus.library.borrow_mut();
                library.set_backup_rule(rule);
                library.set_backups(marks, badges);
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
    /// inserted that waited for its backups checked again, off the frame
    /// ([`Shell::storage_frame`] says the result). A card taken out is no
    /// longer followed.
    pub(super) fn backup_ended(&mut self) {
        self.refresh_backups();
        let Some(link) = self.core.storage.link.clone() else {
            return;
        };
        self.core.storage.waiting.retain(|c| c.path.is_dir());
        self.share_waiting();
        let cards = self.core.storage.waiting.clone();
        if cards.is_empty() {
            return;
        }
        let (send, checked) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("card-safety".into())
            .spawn(move || {
                let now = cards
                    .iter()
                    .map(|c| (c.path.clone(), link.card_safety(c)))
                    .collect();
                let _ = send.send(now);
            });
        match spawned {
            Ok(_) => self.core.storage.checking = Some(checked),
            Err(e) => log::warn!("cards: no thread to check them: {e}"),
        }
    }

    /// Each frame: what the link read since shows, and the cards checked
    /// since a backup ended say whether they are safe to format.
    pub(super) fn storage_frame(&mut self) {
        if let Some(link) = &self.core.storage.link {
            let seq = link.seq();
            if seq != self.core.storage.seen {
                self.core.storage.seen = seq;
                self.show_volumes();
                self.show_backups();
            }
        }
        let checked = match &self.core.storage.checking {
            Some(r) => match r.try_recv() {
                Ok(checked) => checked,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Vec::new(),
            },
            None => return,
        };
        self.core.storage.checking = None;
        for (path, now) in checked {
            match now {
                Ok(now) if now.safe_to_format => {
                    self.core.storage.waiting.retain(|c| c.path != path);
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
                Ok(_) => {}
                Err(e) => log::warn!("{}: {e}", path.display()),
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
        self.open_settings_page(&path)
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
