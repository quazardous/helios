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
//! desktop's shell reads the bus).
//!
//! Each event then passes through its hook point on the application's
//! bus (`bus-contract`, handed to the watch at its start; egui-pop's
//! ADR 0029 and 0030): [`VolumeMounted`] (`volume.mounted`) and
//! [`VolumeUnmounted`] (`volume.unmounted`), on the watch's thread. Who
//! acts on a volume coming or going is a named listener in a declared
//! place ([`MOUNTED_ORDER`], [`UNMOUNTED_ORDER`]), never a callback in
//! the order of the desktop's start:
//!
//! | listener | points | what it does |
//! |---|---|---|
//! | [`CARDS`] | both, before the scan | the import window's cards and the card toast, woken at once |
//! | [`SCAN`] | both | the volumes and where each photo is scanned inside the pass; the Storage page and the Library's badges follow it (`library.changed`) |
//! | [`BACKUPS`] | mounted, after the scan | the strategy's "when a backup disk is plugged in", reading the locations the scan just set |
//! | [`HEALTH`] | both, after the scan | the health checks that follow the volumes run again |
//! | [`MAP`] | mounted, after the scan | the image map's job reads the volume plugged in |
//!
//! The desktop's node starts one ([`crate::node::LocalNode::watch_volumes`])
//! and stops it as it stops. A client machine runs one of its own later,
//! for the devices plugged into it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use bus_contract::{Bus as HookBus, HookError, ListenOpts, Point, PointDecl, PointKind, Refused};
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

/// The hook point a volume mounted passes through, once published on the
/// journal: its listeners in the order [`MOUNTED_ORDER`] declares.
pub struct VolumeMounted;

impl Point for VolumeMounted {
    const ID: &'static str = Mounted::KIND;
    type Input = Mounted;
    type Value = Reactions;
    const KIND: PointKind = PointKind::Event;
}

/// The hook point a volume gone passes through, once published on the
/// journal: its listeners in the order [`UNMOUNTED_ORDER`] declares.
pub struct VolumeUnmounted;

impl Point for VolumeUnmounted {
    const ID: &'static str = Unmounted::KIND;
    type Input = Unmounted;
    type Value = Reactions;
    const KIND: PointKind = PointKind::Event;
}

/// What a volume event's listeners did, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reactions {
    /// The listeners that ran, in their order.
    pub ran: Vec<String>,
}

/// How long a volume event's listeners may take, all told: the scan of
/// where each photo is runs inside the pass, on the watch's thread (never
/// the interface's).
pub const DEADLINE: Duration = Duration::from_mins(5);

/// The volume points, this crate's contract with the application's bus:
/// declared by [`declare`].
pub const POINTS: &[PointDecl] = &[
    PointDecl::of::<VolumeMounted>("A volume mounted: plugged in, or there at the first look")
        .deadline(DEADLINE),
    PointDecl::of::<VolumeUnmounted>("A volume gone: unplugged, or unmounted").deadline(DEADLINE),
];

/// The import window's cards and the card toast, woken at once (they read
/// the journal's `volume.*` events).
pub const CARDS: &str = "helios.import.cards";
/// The scan of the volumes and where each photo is (present, offline,
/// missing), done inside the pass: whoever runs after it reads the
/// locations it set. The Storage page and the Library's badges follow it
/// (the vault says the locations changed: `library.changed`).
pub const SCAN: &str = "helios.library.scan";
/// The strategy's "when a backup disk is plugged in": what is short goes
/// to it ([`crate::jobs::Backups::follow`]).
pub const BACKUPS: &str = "helios.backups.on-connect";
/// The health check runs the checks that follow the volumes again.
pub const HEALTH: &str = "helios.health.recheck";
/// The image map's job reads the volume plugged in.
pub const MAP: &str = "helios.map.plugged";
/// The desktop's read kernel: the values the volumes make out of date
/// (the Storage page's) are read again, after the scan set the locations.
pub const READS: &str = "helios.reads";

/// The listeners of [`VolumeMounted`], each with where it runs: the order
/// they run in.
pub const MOUNTED_ORDER: &[(&str, Place)] = &[
    (CARDS, Place::Before(SCAN)),
    (SCAN, Place::Free),
    (BACKUPS, Place::After(SCAN)),
    (HEALTH, Place::After(SCAN)),
    (MAP, Place::After(SCAN)),
    (READS, Place::After(SCAN)),
];

/// The listeners of [`VolumeUnmounted`], in order.
pub const UNMOUNTED_ORDER: &[(&str, Place)] = &[
    (CARDS, Place::Before(SCAN)),
    (SCAN, Place::Free),
    (HEALTH, Place::After(SCAN)),
    (READS, Place::After(SCAN)),
];

/// Where a listener runs among its point's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Wherever: it needs no other listener's work.
    Free,
    /// After this one.
    After(&'static str),
    /// Before this one.
    Before(&'static str),
}

/// Declares [`POINTS`] on `bus` (once; a second watch on the same bus
/// finds them there).
pub fn declare(bus: &dyn HookBus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios::node", "volumes: {refused}");
        }
    }
}

fn place_in(
    order: &[(&str, Place)],
    point: &str,
    id: &str,
    owner: &str,
) -> Result<ListenOpts, String> {
    let Some((_, place)) = order.iter().find(|(listener, _)| *listener == id) else {
        return Err(format!("`{id}` is not a listener of {point}"));
    };
    let opts = ListenOpts::new().owner(owner);
    Ok(match place {
        Place::Free => opts,
        Place::After(other) => opts.after(*other),
        Place::Before(other) => opts.before(*other),
    })
}

/// Registers the listener `id` of [`VolumeMounted`] (one of
/// [`MOUNTED_ORDER`]'s, owned by `owner`) on `bus`, in its declared place;
/// it runs on the watch's thread. One there already under `id` is
/// replaced.
///
/// # Errors
/// `id` is not in [`MOUNTED_ORDER`], or the bus refused it.
pub fn listen_mounted(
    bus: &dyn HookBus,
    id: &'static str,
    owner: &str,
    run: impl Fn(&Mounted) + Send + Sync + 'static,
) -> Result<(), String> {
    let opts = place_in(MOUNTED_ORDER, VolumeMounted::ID, id, owner)?;
    declare(bus);
    bus.unlisten(VolumeMounted::ID, id);
    bus.listen::<VolumeMounted>(id, opts, move |mounted, reactions| {
        run(mounted);
        reactions.ran.push(id.to_owned());
        Ok::<(), HookError>(())
    })
    .map_err(|e| e.to_string())
}

/// Registers the listener `id` of [`VolumeUnmounted`] (one of
/// [`UNMOUNTED_ORDER`]'s) on `bus`, as [`listen_mounted`] does.
///
/// # Errors
/// `id` is not in [`UNMOUNTED_ORDER`], or the bus refused it.
pub fn listen_unmounted(
    bus: &dyn HookBus,
    id: &'static str,
    owner: &str,
    run: impl Fn(&Unmounted) + Send + Sync + 'static,
) -> Result<(), String> {
    let opts = place_in(UNMOUNTED_ORDER, VolumeUnmounted::ID, id, owner)?;
    declare(bus);
    bus.unlisten(VolumeUnmounted::ID, id);
    bus.listen::<VolumeUnmounted>(id, opts, move |unmounted, reactions| {
        run(unmounted);
        reactions.ran.push(id.to_owned());
        Ok::<(), HookError>(())
    })
    .map_err(|e| e.to_string())
}

/// Registers the listener `id` on both points (it must be in both
/// orders), `run` hearing each as a [`Change`].
///
/// # Errors
/// `id` is not a listener of both points, or the bus refused it.
pub fn listen_both(
    bus: &dyn HookBus,
    id: &'static str,
    owner: &str,
    run: impl Fn(&Change) + Send + Sync + 'static,
) -> Result<(), String> {
    let run = Arc::new(run);
    let mounted = run.clone();
    listen_mounted(bus, id, owner, move |m| {
        mounted(&Change::Mounted(m.clone()));
    })?;
    listen_unmounted(bus, id, owner, move |u| {
        run(&Change::Unmounted(u.clone()));
    })
}

/// The scan of where each photo is, as [`SCAN`]: a volume plugged in or
/// gone is scanned inside the pass (not at the first look: the scan's own
/// first run reads those).
///
/// # Errors
/// The bus refused it.
pub fn listen_scan(
    bus: &dyn HookBus,
    scanner: helios_storage::scan::Scanner,
) -> Result<(), String> {
    listen_both(bus, SCAN, "helios.library", move |change| {
        if !change.at_start() {
            scanner.scan_now();
        }
    })
}

/// The order the volume points' listeners run in on `bus`.
pub fn order(bus: &dyn HookBus) -> String {
    bus_contract::order_dump(bus, &[VolumeMounted::ID, VolumeUnmounted::ID])
}

/// One volume event as the watch passed it through its point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heard {
    /// The point (`volume.mounted`, `volume.unmounted`).
    pub point: &'static str,
    pub id: VolumeId,
    /// Its listeners that ran, in order.
    pub ran: Vec<String>,
}

/// How many events [`VolumeWatch::heard`] keeps.
const HEARD_KEPT: usize = 64;

#[derive(Default)]
struct Seen {
    /// Whether a first look was taken.
    listed: bool,
    /// The volumes mounted, as last looked at.
    mounted: Vec<Volume>,
}

struct Shared {
    bus: Arc<Bus>,
    hooks: Arc<dyn HookBus>,
    seen: Mutex<Seen>,
    heard: Mutex<std::collections::VecDeque<Heard>>,
    stop: AtomicBool,
}

/// The volumes watched on a thread of their own, their comings and goings
/// published on a bus, then passed through their hook points. A handle: clones share the watch; it stops when
/// asked ([`VolumeWatch::stop`]) or once every handle is dropped.
#[derive(Clone)]
pub struct VolumeWatch(Arc<Shared>);

impl std::fmt::Debug for VolumeWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VolumeWatch").finish_non_exhaustive()
    }
}

impl VolumeWatch {
    /// Starts looking at `volumes` every `every`, publishing on `bus`,
    /// then passing each event through its point on `hooks` (the
    /// application's bus, the points declared there).
    pub fn start(
        volumes: Arc<dyn Volumes>,
        bus: Arc<Bus>,
        hooks: Arc<dyn HookBus>,
        every: Duration,
    ) -> Self {
        declare(&*hooks);
        let watch = Self(Arc::new(Shared {
            bus,
            hooks,
            seen: Mutex::default(),
            heard: Mutex::default(),
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

    /// One look at `volumes`: what came and went published, then passed
    /// through its point, its listeners in their declared order.
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
        for change in changes {
            let hooks = &*self.0.hooks;
            let (point, altered) = match &change {
                Change::Mounted(m) => {
                    self.0.bus.publish(SOURCE, None, m);
                    (
                        VolumeMounted::ID,
                        hooks.alter::<VolumeMounted>(m.clone(), Reactions::default()),
                    )
                }
                Change::Unmounted(u) => {
                    self.0.bus.publish(SOURCE, None, u);
                    (
                        VolumeUnmounted::ID,
                        hooks.alter::<VolumeUnmounted>(u.clone(), Reactions::default()),
                    )
                }
            };
            for failure in &altered.failures {
                log::warn!(target: "helios::node", "{point}: {failure}");
            }
            log::debug!(
                target: "helios::node",
                "{point} {}: {}",
                change.id().as_str(),
                altered.value.ran.join(", ")
            );
            let mut heard = self.0.heard.lock().unwrap_or_else(PoisonError::into_inner);
            if heard.len() == HEARD_KEPT {
                heard.pop_front();
            }
            heard.push_back(Heard {
                point,
                id: change.id().clone(),
                ran: altered.value.ran,
            });
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

    /// The application's bus the volume points are passed on.
    pub fn hooks(&self) -> &Arc<dyn HookBus> {
        &self.0.hooks
    }

    /// The last volume events passed through their points (the last
    /// 64), oldest first, with the listeners that ran.
    pub fn heard(&self) -> Vec<Heard> {
        self.0
            .heard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
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
    /// its identity, then passed through their points.
    #[test]
    fn the_watch_publishes_a_volume_coming_and_going() {
        let mut disk = detect::weak("Backup", Some(5));
        disk.kind = VolumeKind::External;
        disk.mount = Some("/media/Backup".into());
        let plugged = Arc::new(Plugged::default());
        let bus = Arc::new(Bus::new());
        let rx = bus.subscribe();
        let hooks: Arc<dyn HookBus> = Arc::new(egui_pop_hooks::Kernel::new());
        let (tx, heard) = std::sync::mpsc::channel();
        listen_both(&*hooks, CARDS, "test", move |c| {
            let _ = tx.send(c.clone());
        })
        .unwrap();
        let watch = VolumeWatch::start(plugged.clone(), bus, hooks, Duration::from_millis(10));
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

    /// Each listener runs in its declared place, whatever the order it
    /// was registered in; one not declared is refused.
    #[test]
    fn the_listeners_run_in_their_declared_order() {
        let bus = egui_pop_hooks::Kernel::new();
        let ran: Arc<Mutex<Vec<&str>>> = Arc::default();
        for id in [MAP, HEALTH, BACKUPS, SCAN, CARDS] {
            let ran = ran.clone();
            listen_mounted(&bus, id, "test", move |_| ran.lock().unwrap().push(id)).unwrap();
        }
        let mounted = Mounted {
            id: VolumeId("uuid:d".into()),
            mount: "/media/d".into(),
            name: "d".into(),
            card: None,
            at_start: false,
        };
        let altered = bus.alter::<VolumeMounted>(mounted, Reactions::default());
        assert!(altered.failures.is_empty());
        assert_eq!(*ran.lock().unwrap(), [CARDS, SCAN, MAP, HEALTH, BACKUPS]);
        assert_eq!(altered.value.ran, [CARDS, SCAN, MAP, HEALTH, BACKUPS]);
        assert!(listen_unmounted(&bus, BACKUPS, "test", |_| {}).is_err());
        assert!(listen_mounted(&bus, "nobody.knows", "test", |_| {}).is_err());
    }
}
