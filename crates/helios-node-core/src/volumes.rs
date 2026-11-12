//! The node watches the volumes of the machine it runs on: cards, cameras
//! and disks coming and going (docs/design/node-client-split.md, row 16).
//!
//! A [`VolumeWatch`] polls a [`Volumes`] about once a second on a thread
//! of its own, with or without a window, and publishes what changed on
//! the node's bus: [`Mounted`] (`volume.mounted`) and [`Unmounted`]
//! (`volume.unmounted`), with the volume's identity and, for a card, the
//! card. The volumes there at the first look are published too, marked
//! [`Mounted::at_start`]. Its clients subscribe: the import window lists
//! the cards and a card inserted while it is closed raises a toast (the
//! desktop's shell reads the bus); the backups
//! start on a disk plugged in ([`crate::jobs::Backups::follow`]);
//! the volume scan and the image map hear of them through
//! [`VolumeWatch::on_change`].
//!
//! The desktop's node starts one ([`crate::node::LocalNode::watch_volumes`])
//! and stops it as it stops. A client machine runs one of its own later,
//! for the devices plugged into it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use helios_core::Bus;
use helios_core::bus::{Envelope, Event};
use helios_storage::{Card, Volume, VolumeId, Volumes};
use serde::{Deserialize, Serialize};

/// Who publishes the volume events on the bus.
pub const SOURCE: &str = "node.volumes";

/// How often the volumes are looked at.
pub const POLL: Duration = Duration::from_secs(1);

/// A volume mounted: plugged in, or there at the first look.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mounted {
    pub id: VolumeId,
    /// Where it is mounted.
    pub mount: PathBuf,
    /// Its name (its label until the user names it).
    pub name: String,
    /// The card it is, for a card or a camera.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<Card>,
    /// It was there at the watch's first look, not plugged in since.
    #[serde(default)]
    pub at_start: bool,
}

impl Event for Mounted {
    const KIND: &'static str = "volume.mounted";
}

/// A volume gone: unplugged, or unmounted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unmounted {
    pub id: VolumeId,
    /// Where it was mounted.
    pub mount: PathBuf,
    /// The card it was, for a card or a camera.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<Card>,
}

impl Event for Unmounted {
    const KIND: &'static str = "volume.unmounted";
}

/// A volume event, as heard from the watch or read off the bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Mounted(Mounted),
    Unmounted(Unmounted),
}

impl Change {
    /// The volume event `envelope` carries, if it is one.
    pub fn of(envelope: &Envelope) -> Option<Self> {
        if envelope.source != SOURCE {
            return None;
        }
        let payload = envelope.payload.clone();
        match envelope.kind {
            Mounted::KIND => serde_json::from_value(payload).ok().map(Self::Mounted),
            Unmounted::KIND => serde_json::from_value(payload).ok().map(Self::Unmounted),
            _ => None,
        }
    }

    /// The volume's id.
    pub fn id(&self) -> &VolumeId {
        match self {
            Self::Mounted(m) => &m.id,
            Self::Unmounted(u) => &u.id,
        }
    }

    /// The card, for a card or a camera.
    pub fn card(&self) -> Option<&Card> {
        match self {
            Self::Mounted(m) => m.card.as_ref(),
            Self::Unmounted(u) => u.card.as_ref(),
        }
    }

    /// The volume was there at the watch's first look: no coming or
    /// going, just the first list.
    pub fn at_start(&self) -> bool {
        matches!(self, Self::Mounted(m) if m.at_start)
    }
}

type Listener = Box<dyn Fn(&Change) + Send>;

#[derive(Default)]
struct Seen {
    /// Whether a first look was taken.
    listed: bool,
    /// The volumes mounted, as last looked at.
    mounted: Vec<Volume>,
}

struct Shared {
    bus: Arc<Bus>,
    seen: Mutex<Seen>,
    listeners: Mutex<Vec<Listener>>,
    stop: AtomicBool,
}

/// The volumes watched on a thread of their own, their comings and goings
/// published on a bus. A handle: clones share the watch; it stops when
/// asked ([`VolumeWatch::stop`]) or once every handle is dropped.
#[derive(Clone)]
pub struct VolumeWatch(Arc<Shared>);

impl std::fmt::Debug for VolumeWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VolumeWatch").finish_non_exhaustive()
    }
}

impl VolumeWatch {
    /// Starts looking at `volumes` every `every`, publishing on `bus`.
    pub fn start(volumes: Arc<dyn Volumes>, bus: Arc<Bus>, every: Duration) -> Self {
        let watch = Self(Arc::new(Shared {
            bus,
            seen: Mutex::default(),
            listeners: Mutex::default(),
            stop: AtomicBool::new(false),
        }));
        let weak: Weak<Shared> = Arc::downgrade(&watch.0);
        let spawned = std::thread::Builder::new()
            .name("helios-volumes".into())
            .spawn(move || {
                while let Some(shared) = weak.upgrade() {
                    if shared.stop.load(Ordering::SeqCst) {
                        break;
                    }
                    Self(shared).look(volumes.as_ref());
                    std::thread::sleep(every);
                }
            });
        if let Err(e) = spawned {
            log::warn!(target: "helios::node", "no thread to watch the volumes on: {e}");
        }
        watch
    }

    /// One look at `volumes`: what came and went published, then told to
    /// the listeners.
    fn look(&self, volumes: &dyn Volumes) {
        let mut now: Vec<Volume> = volumes
            .volumes()
            .into_iter()
            .filter(Volume::online)
            .collect();
        now.dedup_by(|a, b| a.id == b.id && a.mount == b.mount);
        let changes = {
            let mut seen = self.seen();
            let at_start = !seen.listed;
            let same = |a: &Volume, b: &Volume| a.id == b.id && a.mount == b.mount;
            let mut changes: Vec<Change> = seen
                .mounted
                .iter()
                .filter(|o| !now.iter().any(|n| same(o, n)))
                .map(|v| {
                    Change::Unmounted(Unmounted {
                        id: v.id.clone(),
                        mount: v.mount.clone().unwrap_or_default(),
                        card: Card::of(v),
                    })
                })
                .collect();
            changes.extend(
                now.iter()
                    .filter(|n| !seen.mounted.iter().any(|o| same(o, n)))
                    .map(|v| {
                        Change::Mounted(Mounted {
                            id: v.id.clone(),
                            mount: v.mount.clone().unwrap_or_default(),
                            name: v.name.clone(),
                            card: Card::of(v),
                            at_start,
                        })
                    }),
            );
            seen.mounted = now;
            seen.listed = true;
            changes
        };
        for change in &changes {
            match change {
                Change::Mounted(m) => self.0.bus.publish(SOURCE, None, m),
                Change::Unmounted(u) => self.0.bus.publish(SOURCE, None, u),
            };
            let listeners = self
                .0
                .listeners
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for listener in listeners.iter() {
                listener(change);
            }
        }
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.0.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The bus the volume events go to.
    pub fn bus(&self) -> &Arc<Bus> {
        &self.0.bus
    }

    /// The volume events published from now on (and every other event of
    /// the bus: [`Change::of`] keeps the volume ones).
    pub fn subscribe(&self) -> Receiver<Envelope> {
        self.0.bus.subscribe()
    }

    /// Calls `f`, from the watch's thread, with each volume event, right
    /// after it is published; the first list's included
    /// ([`Change::at_start`]).
    pub fn on_change(&self, f: impl Fn(&Change) + Send + 'static) {
        self.0
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::new(f));
    }

    /// The volumes mounted, as last looked at.
    pub fn mounted(&self) -> Vec<Volume> {
        self.seen().mounted.clone()
    }

    /// The cards and cameras mounted, as last looked at.
    pub fn cards(&self) -> Vec<Card> {
        self.seen().mounted.iter().filter_map(Card::of).collect()
    }

    /// Whether a first look was taken.
    pub fn listed(&self) -> bool {
        self.seen().listed
    }

    /// Stops watching (the node stopping).
    pub fn stop(&self) {
        self.0.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_storage::{VolumeKind, detect};
    use std::time::Instant;

    #[derive(Default)]
    struct Plugged(Mutex<Vec<Volume>>);

    impl Volumes for Plugged {
        fn volumes(&self) -> Vec<Volume> {
            self.0.lock().unwrap().clone()
        }
    }

    fn next(rx: &Receiver<Envelope>) -> Change {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let envelope = rx.recv_timeout(left).expect("a volume event in 10 s");
            if let Some(change) = Change::of(&envelope) {
                return change;
            }
        }
    }

    /// A disk plugged in, then unplugged: both published on the bus with
    /// its identity, and told to the listeners.
    #[test]
    fn the_watch_publishes_a_volume_coming_and_going() {
        let mut disk = detect::weak("Backup", Some(5));
        disk.kind = VolumeKind::External;
        disk.mount = Some("/media/Backup".into());
        let plugged = Arc::new(Plugged::default());
        let bus = Arc::new(Bus::new());
        let rx = bus.subscribe();
        let watch = VolumeWatch::start(plugged.clone(), bus, Duration::from_millis(10));
        let (tx, heard) = std::sync::mpsc::channel();
        watch.on_change(move |c| {
            let _ = tx.send(c.clone());
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !watch.listed() {
            assert!(Instant::now() < deadline, "no first look in 10 s");
            std::thread::sleep(Duration::from_millis(5));
        }
        plugged.0.lock().unwrap().push(disk.clone());
        let Change::Mounted(m) = next(&rx) else {
            panic!("mounted first")
        };
        assert_eq!((m.id.clone(), m.at_start), (disk.id.clone(), false));
        assert_eq!(m.mount, PathBuf::from("/media/Backup"));
        assert_eq!(
            heard.recv_timeout(Duration::from_secs(10)).unwrap().id(),
            &disk.id
        );
        plugged.0.lock().unwrap().clear();
        assert!(matches!(next(&rx), Change::Unmounted(u) if u.id == disk.id));
        assert!(watch.mounted().is_empty());
        watch.stop();
    }
}
