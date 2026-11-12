//! Where Helios's files live: the volumes of the machine (internal and
//! external disks, cards and sticks, network shares), what each is and
//! what it is for (`docs/storage-backup-research.md`, section 2.1).
//!
//! - [`Volume`] is one volume: its identity ([`VolumeId`], found by
//!   [`Identity`]), the user's name for it, its [`VolumeKind`], capacity
//!   and free space, where it is mounted when online, the node a fixed disk
//!   belongs to, when and where it was last seen, and its [`Role`]s.
//! - [`Volumes`] lists what is mounted now; [`system`] is the machine's
//!   (Linux reads `/proc/mounts`, `/dev/disk` and `/sys`, see [`detect`];
//!   other systems list none for now, behind the same trait). Its
//!   [`Volumes::cards`] keeps the cards and cameras, for the import window.
//! - [`Watch`] polls a [`Volumes`] about once a second on a thread of its
//!   own and keeps the last list of cards, with those inserted since; it
//!   tells [`Watch::on_volumes`] listeners when a volume appears or goes.
//! - [`marker`] reads and writes the identity file `.helios/volume.json`.
//! - [`registry`] keeps the volumes Helios has seen in the vault, offline
//!   ones included.
//! - [`scan`] records them, links each location to its volume and says
//!   whether its file is present, offline or missing, on a thread of its
//!   own ([`scan::Scanner`]).
//! - [`exclude`] is the image map's blacklist: the places never recorded,
//!   by default per OS and by the user's `system.map.exclude`.
//! - [`strategy`] keeps the library's backup strategy, counts each photo's
//!   verified backup copies and says where Helios may write them.

pub mod detect;
pub mod exclude;
pub mod marker;
pub mod registry;
pub mod scan;
pub mod strategy;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use helios_vault::storage::VolumeId;

/// How a volume's id was found, strongest first: a marker file Helios
/// wrote wins over the filesystem's UUID, which wins over a share's
/// address; a weak id (label and size) only says "reinsert this card",
/// never counts a backup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Identity {
    Marker,
    Uuid,
    Share,
    Weak,
    /// An account or source of a non-local connector (`remote:…`): no
    /// mount point, its items fetched through the connector.
    Remote,
}

/// What a volume is. Only an internal disk is fixed: it belongs to its
/// node, and "offline" there means "that node is not running".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeKind {
    Internal,
    External,
    Removable,
    Share,
    Remote,
}

impl VolumeKind {
    /// Whether a volume of this kind stays in one machine.
    pub fn is_fixed(self) -> bool {
        self == Self::Internal
    }
}

/// What the user keeps on a volume (section 2.4). Helios writes only to
/// volumes with a library, archive, backup or scratch role, never to an
/// ingest one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Library,
    Archive,
    Backup,
    Ingest,
    Scratch,
}

impl Role {
    /// Every role, in the order the Storage page shows them.
    pub const ALL: [Role; 5] = [
        Role::Library,
        Role::Archive,
        Role::Backup,
        Role::Ingest,
        Role::Scratch,
    ];
}

/// When and on which node a volume was last seen, and where it was
/// mounted then.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastSeen {
    /// Seconds since the Unix epoch.
    pub at: u64,
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<PathBuf>,
}

/// A place files live that comes and goes as a whole.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Volume {
    pub id: VolumeId,
    pub identified_by: Identity,
    /// The user's name for it; its label until the user names it.
    pub name: String,
    /// The filesystem's label (or the folder it is mounted on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub kind: VolumeKind,
    #[serde(default)]
    pub roles: BTreeSet<Role>,
    /// The physical drive behind it, when the system says: two partitions
    /// of one disk name the same drive (and are not two copies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive: Option<String>,
    /// Its size in bytes, as last seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity: Option<u64>,
    /// Its free bytes, as last seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free: Option<u64>,
    /// Where it is mounted now; `None` when offline. Never stored: the
    /// registry keeps the last mount point in [`LastSeen`].
    #[serde(skip)]
    pub mount: Option<PathBuf>,
    /// The node a fixed disk belongs to; `None` for one that moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<LastSeen>,
    /// Whether it holds a `DCIM` folder: a card or a camera.
    #[serde(default)]
    pub has_dcim: bool,
    /// Kept away from the others (the user says): what an off-site copy
    /// needs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub off_site: bool,
    /// A RAID array (the user says; Helios cannot always tell). It still
    /// counts as one copy: its redundancy guards against a dead disk, not
    /// against deletion, corruption or theft.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub raid: bool,
}

impl Volume {
    /// Whether it is online (mounted here, now).
    pub fn online(&self) -> bool {
        self.mount.is_some()
    }

    /// A card or a camera: removable and holding `DCIM`.
    pub fn is_card(&self) -> bool {
        self.has_dcim && self.kind == VolumeKind::Removable
    }

    /// The roles a volume of its kind starts with: a card is an ingest
    /// source. (The library role goes to the volume holding the library
    /// folder, which only the registry knows.)
    pub fn default_roles(&self) -> BTreeSet<Role> {
        if self.is_card() {
            BTreeSet::from([Role::Ingest])
        } else {
            BTreeSet::new()
        }
    }

    /// The volume of one account or source of a non-local connector
    /// (`docs/design/import-connectors.md`, section 2.3): kind
    /// [`VolumeKind::Remote`], id `remote:<connector>:<source>`, never
    /// mounted, no role. Its items are sightings on it, their place the
    /// item id.
    pub fn remote(connector: &str, source: &str, label: &str) -> Self {
        Self {
            id: VolumeId(format!("remote:{connector}:{source}")),
            identified_by: Identity::Remote,
            name: label.to_string(),
            label: Some(label.to_string()),
            kind: VolumeKind::Remote,
            roles: BTreeSet::new(),
            drive: None,
            capacity: None,
            free: None,
            mount: None,
            node: None,
            last_seen: None,
            has_dcim: false,
            off_site: false,
            raid: false,
        }
    }

    /// The connector a remote volume belongs to, from its id.
    pub fn connector(&self) -> Option<&str> {
        if self.kind != VolumeKind::Remote {
            return None;
        }
        self.id.as_str().strip_prefix("remote:")?.split(':').next()
    }
}

/// A card or a camera as the import window shows it: a mounted volume
/// with a `DCIM` folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// Its label (the folder it is mounted on, for udisks).
    pub label: String,
    /// Where it is mounted.
    pub root: PathBuf,
    /// Its size in bytes, when the system says.
    pub capacity: Option<u64>,
}

impl Card {
    /// The card a volume is, if it is one and is mounted.
    pub fn of(volume: &Volume) -> Option<Self> {
        if !volume.is_card() {
            return None;
        }
        Some(Self {
            label: volume.label.clone().unwrap_or_else(|| volume.name.clone()),
            root: volume.mount.clone()?,
            capacity: volume.capacity,
        })
    }

    /// The volume a card stands for, weakly identified.
    pub fn volume(&self) -> Volume {
        let mut v = detect::weak(&self.label, self.capacity);
        v.kind = VolumeKind::Removable;
        v.mount = Some(self.root.clone());
        v.has_dcim = true;
        v.roles = v.default_roles();
        v
    }
}

/// Lists the volumes mounted now.
pub trait Volumes: Send + Sync {
    /// Every volume mounted now.
    fn volumes(&self) -> Vec<Volume>;

    /// The cards and cameras among them.
    fn cards(&self) -> Vec<Card> {
        self.cards_of(&self.volumes())
    }

    /// The cards and cameras among `volumes`, a list it gave.
    fn cards_of(&self, volumes: &[Volume]) -> Vec<Card> {
        volumes.iter().filter_map(Card::of).collect()
    }
}

/// A fixed list of cards, as given (tests, a tool).
impl Volumes for Vec<Card> {
    fn volumes(&self) -> Vec<Volume> {
        self.iter().map(Card::volume).collect()
    }

    fn cards(&self) -> Vec<Card> {
        self.clone()
    }

    fn cards_of(&self, _: &[Volume]) -> Vec<Card> {
        self.clone()
    }
}

/// A fixed list of volumes, as given.
impl Volumes for Vec<Volume> {
    fn volumes(&self) -> Vec<Volume> {
        self.clone()
    }
}

/// The machine's volumes.
pub fn system() -> Arc<dyn Volumes> {
    Arc::new(System)
}

struct System;

impl Volumes for System {
    #[cfg(target_os = "linux")]
    fn volumes(&self) -> Vec<Volume> {
        detect::scan(&detect::Roots::system(), &node())
    }

    #[cfg(not(target_os = "linux"))]
    fn volumes(&self) -> Vec<Volume> {
        Vec::new()
    }
}

/// This machine's name as a node: the kernel's host name, else
/// `HOSTNAME`, else `COMPUTERNAME` (Windows), else "localhost".
pub fn node() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_string())
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()))
        .or_else(|| std::env::var("COMPUTERNAME").ok().filter(|h| !h.is_empty()))
        .unwrap_or_else(|| "localhost".into())
}

/// A capacity as a card says it: decimal gigabytes ("64 GB"), megabytes
/// below one.
pub fn capacity_text(bytes: u64) -> String {
    const GB: f64 = 1e9;
    let gb = bytes as f64 / GB;
    if gb >= 1.0 {
        format!("{} GB", gb.round() as u64)
    } else {
        format!("{} MB", (bytes as f64 / 1e6).round() as u64)
    }
}

#[derive(Default)]
struct Seen {
    cards: Vec<Card>,
    /// Whether a first list came.
    listed: bool,
    /// Cards inserted since the first list, not taken yet.
    inserted: Vec<Card>,
}

/// Called when a volume appears or goes.
type Listener = Box<dyn Fn() + Send>;

/// The cards, polled on a thread of their own (about once a second), and
/// the volumes' comings and goings.
pub struct Watch {
    seen: Arc<Mutex<Seen>>,
    listeners: Arc<Mutex<Vec<Listener>>>,
    stop: Arc<AtomicBool>,
}

impl Watch {
    /// Starts polling `volumes`; `changed` is called (from the thread)
    /// when the list of cards changes, to ask the interface for a frame.
    pub fn start(volumes: Arc<dyn Volumes>, changed: impl Fn() + Send + 'static) -> Self {
        let seen: Arc<Mutex<Seen>> = Arc::default();
        let listeners: Arc<Mutex<Vec<Listener>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (s, l, halt) = (seen.clone(), listeners.clone(), stop.clone());
        let spawned = std::thread::Builder::new()
            .name("helios-cards".into())
            .spawn(move || {
                // Which volume is mounted where, as last polled.
                let mut mounted: Option<Vec<(VolumeId, PathBuf)>> = None;
                while !halt.load(Ordering::Relaxed) {
                    let all = volumes.volumes();
                    let now = volumes.cards_of(&all);
                    let mut mounts: Vec<(VolumeId, PathBuf)> = all
                        .into_iter()
                        .filter_map(|v| Some((v.id, v.mount?)))
                        .collect();
                    mounts.sort();
                    let came_or_went = mounted.as_ref().is_some_and(|m| *m != mounts);
                    mounted = Some(mounts);
                    let moved = {
                        let mut seen = s.lock().unwrap_or_else(PoisonError::into_inner);
                        let moved = !seen.listed || seen.cards != now;
                        if seen.listed {
                            let new: Vec<Card> = now
                                .iter()
                                .filter(|c| !seen.cards.iter().any(|o| o.root == c.root))
                                .cloned()
                                .collect();
                            seen.inserted.extend(new);
                        }
                        seen.cards = now;
                        seen.listed = true;
                        moved
                    };
                    if moved {
                        changed();
                    }
                    if came_or_went {
                        for listener in l.lock().unwrap_or_else(PoisonError::into_inner).iter() {
                            listener();
                        }
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            });
        if let Err(e) = spawned {
            log::warn!("cards: no thread to watch them: {e}");
        }
        Self {
            seen,
            listeners,
            stop,
        }
    }

    /// Calls `f` (from the watching thread) each time a volume appears,
    /// goes or changes mount point: the background scan's cue.
    pub fn on_volumes(&self, f: impl Fn() + Send + 'static) {
        self.listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::new(f));
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The cards mounted, as last polled.
    pub fn cards(&self) -> Vec<Card> {
        self.seen().cards.clone()
    }

    /// Whether a first list came.
    pub fn listed(&self) -> bool {
        self.seen().listed
    }

    /// The cards inserted since the last call (not those there at start).
    pub fn take_inserted(&self) -> Vec<Card> {
        std::mem::take(&mut self.seen().inserted)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capacity_reads_as_on_the_card() {
        assert_eq!(capacity_text(64_021_856_256), "64 GB");
        assert_eq!(capacity_text(512_000_000), "512 MB");
    }

    #[test]
    fn a_card_list_is_its_own_cards_and_weak_removable_volumes() {
        let cards = vec![Card {
            label: "EOS_DIGITAL".into(),
            root: "/run/media/u/EOS_DIGITAL".into(),
            capacity: Some(64_000_000_000),
        }];
        let volumes = cards.volumes();
        assert_eq!(volumes[0].identified_by, Identity::Weak);
        assert!(volumes[0].is_card());
        assert_eq!(volumes[0].roles, BTreeSet::from([Role::Ingest]));
        assert_eq!(Card::of(&volumes[0]).as_ref(), cards.first());
        // Through the trait's own filter too.
        assert_eq!(volumes.cards(), cards);
    }

    struct Shared(Mutex<Vec<Volume>>);

    impl Volumes for Shared {
        fn volumes(&self) -> Vec<Volume> {
            self.0.lock().unwrap().clone()
        }
    }

    #[test]
    fn the_watch_tells_when_a_volume_comes_or_goes() {
        let mut disk = detect::weak("Backup", Some(5));
        disk.kind = VolumeKind::External;
        disk.mount = Some("/media/Backup".into());
        let shared = Arc::new(Shared(Mutex::new(Vec::new())));
        let (listed, first) = std::sync::mpsc::channel();
        let watch = Watch::start(shared.clone(), move || {
            let _ = listed.send(());
        });
        first.recv_timeout(Duration::from_secs(10)).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        watch.on_volumes(move || {
            let _ = tx.send(());
        });
        shared.0.lock().unwrap().push(disk);
        rx.recv_timeout(Duration::from_secs(10))
            .expect("told it came");
        shared.0.lock().unwrap().clear();
        rx.recv_timeout(Duration::from_secs(10))
            .expect("told it went");
    }
}
