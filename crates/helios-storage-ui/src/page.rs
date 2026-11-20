//! What the desktop keeps of storage ([`StoragePage`]): the volumes and
//! the strategy as last read, the owners of remote volumes, the one card
//! list, and the cards checked again once a backup ended.
//!
//! Everything here reaches the rest of the desktop through the bus: what
//! is read passes through `storage.read` ([`crate::read`]), a backup's end
//! and the cards found safe through `backup.ended` and `card.safe`
//! ([`crate::backup_end`]), the Settings app's reads through its read
//! kernel's facts ([`crate::VOLUMES_CHANGED`]). The one thing the desktop
//! lends is how a card safe to format is said to the person ([`Tell`]).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use egui_pop::api::app::AppHandle;
use egui_pop::api::bus::bus_contract::Bus;
use egui_pop::core::icon::Icon;
use helios_core::Message;
use helios_library::import::Card;
use helios_storage::strategy::{self, Destination, Strategy};
use helios_storage::{Role, Volume, VolumeKind};
use serde_json::json;

use crate::backup_end::{self, CardList};
use crate::link::StorageLink;
use crate::read as storage_read;

/// The waiting cards checked again: each one's path and what it is now.
type Checked = Vec<(std::path::PathBuf, Result<Card, String>)>;

/// A saved import source using an account, as the Storage page names the
/// account's volume by it: the account's id and the name given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceUse {
    /// The account the source reads, if any.
    pub account: Option<String>,
    /// The name the user gave the source.
    pub name: Option<String>,
}

/// Where the Storage page reads the saved sources from (the import
/// window keeps them).
pub trait SourceBook: Send + Sync {
    /// Every saved source, as [`SourceUse`]s.
    fn sources(&self) -> Vec<SourceUse>;
}

/// What names the owners of remote volumes, for the Storage page's rows:
/// the import accounts and the saved sources using them, once the
/// accounts are kept.
pub type Owners = Arc<
    Mutex<
        Option<(
            Arc<dyn helios_connect::account::AccountBook>,
            Arc<dyn SourceBook>,
        )>,
    >,
>;

/// How the desktop says to the person that a card is safe to format
/// (a toast, the Log, a notification): the one thing storage borrows from
/// the desktop besides the bus.
pub trait Tell {
    /// Says `message` as good news.
    fn success(&self, message: Message);
}

/// The volumes and the strategy as the link last answered them: shared
/// (`Rc`) between the desktop and `library.changed`'s local listener
/// `library.volumes`, which reads them again inside the pass
/// ([`refresh_volumes`]). What is read is passed through `storage.read`
/// ([`crate::read`]) to whoever shows it.
#[derive(Default)]
pub struct StorageRead {
    link: Option<Arc<dyn StorageLink>>,
    list: Vec<Volume>,
    /// The library's backup strategy, as last read.
    strategy: Option<Strategy>,
    error: Option<String>,
}

/// Reads the volumes again: the page's list, and the volumes passed
/// through `storage.read` on `bus` (the Library names them: a scan ran, a
/// disk came or went). What the link reads off the frame shows at a
/// coming one.
pub fn refresh_volumes(read: &RefCell<StorageRead>, bus: &dyn Bus) {
    let Some(link) = read.borrow().link.clone() else {
        return;
    };
    link.refresh();
    show_volumes(read, bus);
}

/// The volumes and the strategy as the link answers them now; the
/// volumes passed through `storage.read` on `bus`.
fn show_volumes(read: &RefCell<StorageRead>, bus: &dyn Bus) {
    let Some(link) = read.borrow().link.clone() else {
        return;
    };
    let volumes = link.volumes();
    // The import window's strategy line reads it too.
    let strategy = link.strategy();
    let mut read = read.borrow_mut();
    let mut readout = storage_read::Readout::default();
    match volumes {
        Ok(volumes) => {
            readout.volumes = Some(volumes.clone());
            read.list = volumes;
            read.error = None;
        }
        Err(e) => read.error = Some(e),
    }
    match strategy {
        Ok(s) => read.strategy = s,
        Err(e) => read.error = Some(e),
    }
    if let Some(e) = &read.error {
        log::warn!("storage: {e}");
    }
    drop(read);
    if readout.volumes.is_some() {
        storage_read::pass(bus, readout);
    }
}

/// What the desktop keeps of storage.
#[derive(Default)]
pub struct StoragePage {
    /// The volumes and the strategy as last read.
    pub read: Rc<RefCell<StorageRead>>,
    /// The accounts and sources behind remote volumes, read by
    /// `library.volumes` to show one row per account.
    pub owners: Owners,
    /// The cards an import left short of their backups, while inserted:
    /// the one list every reader reads ([`CardList`]).
    pub cards: CardList,
    /// The link's [`StorageLink::seq`] last shown.
    seen: u64,
    /// The waiting cards checked again off the frame, once a backup
    /// ended (`cards.safety`): each one's path and what it is now.
    checking: Rc<RefCell<Option<Receiver<Checked>>>>,
}

impl StoragePage {
    /// The library's backup strategy, as last read.
    pub fn strategy(&self) -> Option<Strategy> {
        self.read.borrow().strategy.clone()
    }

    /// The volumes known, online ones first.
    pub fn volumes(&self) -> Vec<Volume> {
        self.read.borrow().list.clone()
    }

    /// Reads the volumes again (see [`refresh_volumes`]).
    pub fn refresh_volumes(&self, bus: &dyn Bus) {
        refresh_volumes(&self.read, bus);
    }

    /// Each photo's backups read again (one read for the Library), for
    /// its badges and its `backup:` filters; what the link reads off the
    /// frame shows at a coming one.
    pub fn refresh_backups(&self, bus: &dyn Bus) {
        refresh_backups(&self.read, bus);
    }

    /// Reads and writes through `link` from now on: the volumes read
    /// again, passed through `storage.read` on `bus`.
    pub fn attach_link(&self, link: Arc<dyn StorageLink>, bus: &dyn Bus) {
        self.read.borrow_mut().link = Some(link);
        refresh_volumes(&self.read, bus);
    }

    /// The import window's strategy line, and whether it is the one
    /// without a strategy ("No backup set up:", the Storage page offered):
    /// "2 copies: Backup disk (plugged in), NAS (not seen for 3 days)".
    pub fn backup_line(&self, t: &dyn Fn(Message) -> String) -> (String, bool) {
        let read = self.read.borrow();
        backup_line(t, read.strategy.as_ref(), &read.list)
    }

    /// Storage's listeners of `backup.ended` and `card.safe`
    /// ([`crate::backup_end`]) on `app`'s bus, each in its declared
    /// place: the badges, the cards checked again, then `tell` says the
    /// cards safe and the Storage page's reads follow (the health check
    /// listens on its own). What the bus refused, said.
    pub fn listen_backup_points(&self, app: &AppHandle, tell: Rc<dyn Tell>) -> Vec<String> {
        let bus = app.bus();
        let (read, on) = (self.read.clone(), Arc::downgrade(&bus));
        let mut listened = vec![backup_end::listen_ended(
            &*bus,
            backup_end::BACKUPS,
            move |_, _| {
                if let Some(bus) = on.upgrade() {
                    refresh_backups(&read, &*bus);
                }
            },
        )];
        let (read, cards, checking) =
            (self.read.clone(), self.cards.clone(), self.checking.clone());
        let wake = app.clone();
        listened.push(backup_end::listen_ended(
            &*bus,
            backup_end::SAFETY,
            move |_, _| {
                let link = read.borrow().link.clone();
                if let Some(link) = link
                    && let Some(r) = check_cards(link, cards.waiting(), wake.egui())
                {
                    *checking.borrow_mut() = Some(r);
                }
            },
        ));
        listened.push(backup_end::listen_local(
            &*bus,
            backup_end::TOAST,
            move |safe, _| {
                for card in &safe.cards {
                    tell.success(
                        Message::new(
                            "gui-backup-card-safe",
                            "{ $card } is safe to format: every photo has its backups",
                        )
                        .with("card", card.label.clone()),
                    );
                }
            },
        ));
        let app = app.clone();
        listened.push(backup_end::listen_local(
            &*bus,
            backup_end::STORAGE,
            // The Storage page's card rows read again, from the list.
            move |_, _| read_fact(&app, crate::VOLUMES_CHANGED),
        ));
        listened
            .into_iter()
            .filter_map(Result::err)
            .map(|e| format!("{}: {e}", backup_end::ENDED))
            .collect()
    }

    /// Each frame: what the link read since it was last shown passes
    /// through `storage.read` on `app`'s bus, and the Settings app's reads
    /// of the volumes follow, their values kept while read again.
    pub fn follow_link(&mut self, app: &AppHandle) {
        let Some(link) = self.read.borrow().link.clone() else {
            return;
        };
        let seq = link.seq();
        if seq == self.seen {
            return;
        }
        self.seen = seq;
        let bus = app.bus();
        show_volumes(&self.read, &*bus);
        show_backups(&self.read, &*bus);
        read_fact(app, crate::VOLUMES_CHANGED);
    }

    /// Each frame: what the link read since shows ([`Self::follow_link`]),
    /// and the cards checked since a backup ended say whether they are
    /// safe to format: those found safe pass through `card.safe` on
    /// `app`'s bus, then are said on the journal. Whether some passed.
    pub fn frame(&mut self, app: &AppHandle) -> bool {
        self.follow_link(app);
        let checked = {
            let mut checking = self.checking.borrow_mut();
            let checked = match &*checking {
                Some(r) => match r.try_recv() {
                    Ok(checked) => checked,
                    Err(std::sync::mpsc::TryRecvError::Empty) => return false,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Vec::new(),
                },
                None => return false,
            };
            *checking = None;
            checked
        };
        // The list marked first: whoever reads it from now on (the
        // listeners below, the import window, the Storage page, the
        // health check) says the same.
        let mut safe = Vec::new();
        for (path, now) in checked {
            match now {
                Ok(now) => {
                    if self.cards.checked(&path, now.clone()) {
                        safe.push(now);
                    }
                }
                Err(e) => log::warn!("{}: {e}", path.display()),
            }
        }
        if safe.is_empty() {
            return false;
        }
        let labels: Vec<String> = safe.iter().map(|c| c.label.clone()).collect();
        let bus = app.bus();
        let reactions = backup_end::pass_safe(&*bus, backup_end::SafeCards { cards: safe });
        app.publish(
            "helios",
            None,
            backup_end::SAFE,
            backup_end::said(&reactions.ran, json!({ "cards": labels })),
        );
        true
    }
}

/// A backup job ended: passed through `backup.ended` on `app`'s bus (the
/// badges read again, each card still inserted checked again off the
/// frame: [`StoragePage::frame`] passes those found safe through
/// `card.safe`), then said on the journal.
pub fn backup_ended(app: &AppHandle) {
    let bus = app.bus();
    let reactions = backup_end::pass_ended(&*bus, backup_end::BackupEnd::default());
    app.publish(
        "helios",
        None,
        backup_end::ENDED,
        backup_end::said(&reactions.ran, json!({})),
    );
}

/// Whether the Settings app is open on its Storage or Backup scope.
pub fn page_open(app: &AppHandle) -> bool {
    app.service(&egui_pop::api::admin::ADMIN).is_some_and(|a| {
        a.is_open() && matches!(a.shown().0.as_deref(), Some("storage" | "backup"))
    })
}

/// A fact the Settings app's reads follow (`storage.volumes.changed`):
/// the values it makes out of date are read again, shown meanwhile.
fn read_fact(app: &AppHandle, name: &str) {
    if let Some(k) = app
        .service(&egui_pop::api::admin::ADMIN)
        .and_then(|a| a.reads())
    {
        k.event(name);
    }
}

/// A destination's words: any disk of a role, or one disk by name.
pub(crate) fn destination_text(
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
pub(crate) fn kind_of(kind: VolumeKind) -> (Icon, Message) {
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

/// When a volume was last seen, as a day in the system's time zone
/// (`2026-11-20`), as the Library says its days.
pub(crate) fn day_of(at: u64) -> String {
    i64::try_from(at)
        .ok()
        .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
        .map_or_else(
            || "?".into(),
            |utc| {
                utc.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d")
                    .to_string()
            },
        )
}

/// The strategy's line ([`StoragePage::backup_line`]) from a strategy and
/// the volumes known.
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

/// Each photo's backups read again (one read for the Library), for its
/// badges and its `backup:` filters; what the link reads off the frame
/// shows at a coming one.
fn refresh_backups(read: &RefCell<StorageRead>, bus: &dyn Bus) {
    let link = read.borrow().link.clone();
    if let Some(link) = link {
        link.refresh();
    }
    show_backups(read, bus);
}

/// Each photo's backups as the link answers them now, passed through
/// `storage.read` on `bus`.
fn show_backups(read: &RefCell<StorageRead>, bus: &dyn Bus) {
    let Some(link) = read.borrow().link.clone() else {
        return;
    };
    match link.statuses() {
        Ok(statuses) => {
            let read = read.borrow();
            let badges = read.strategy.is_some();
            let rule = strategy::rule(&read.list, read.strategy.as_ref());
            drop(read);
            let backups = storage_read::BackupsRead {
                statuses,
                badges,
                rule,
            };
            storage_read::pass(
                bus,
                storage_read::Readout {
                    volumes: None,
                    backups: Some(backups),
                },
            );
        }
        Err(e) => log::warn!("backups: {e}"),
    }
}

/// `cards` checked again through `link` on a thread of their own (the
/// check reads the vault), `wake`'s context asked a pass once it is done; where the
/// result comes, or none when there is nothing to check.
fn check_cards(
    link: Arc<dyn StorageLink>,
    cards: Vec<Card>,
    wake: egui_pop::egui::Context,
) -> Option<Receiver<Checked>> {
    if cards.is_empty() {
        return None;
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
            wake.request_repaint();
        });
    match spawned {
        Ok(_) => Some(checked),
        Err(e) => {
            log::warn!("cards: no thread to check them: {e}");
            None
        }
    }
}
