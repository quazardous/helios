//! The link the Storage page reads and writes through ([`StorageLink`]),
//! and [`NodeStorage`], the link over the desktop's node and the
//! machine's volumes, read and written off the frame.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};

use helios_inspect::client::Client;
use helios_library::import::Card;
use helios_storage::strategy::{BackupStatus, Strategy};
use helios_storage::{Volume, VolumeId, Volumes, registry};
use serde_json::{Value, json};

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
/// (`library.storage.get` and the `storage.*` commands). The volumes
/// mounted are read on the link's thread too, with the node's answers:
/// asking each for its free space may take seconds (a network drive).
///
/// Nothing here waits for the node on the caller's thread: the reads
/// answer what was last read, and a thread of its own runs the writes in
/// the order made, then reads the volumes, the strategy and the backups
/// again ([`StorageLink::refresh`] asks the same). Each read that comes
/// back moves [`StorageLink::seq`], so the shell shows it at a coming
/// frame.
pub struct NodeStorage {
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
    /// The volumes mounted on this machine, as last read (their free
    /// space asked of each: slow on a network drive).
    mounted: Vec<Volume>,
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
        let (weak, c, m) = (Arc::downgrade(node), cached.clone(), mounted.clone());
        let spawned = std::thread::Builder::new()
            .name("storage-node".into())
            .spawn(move || work(&weak, &*m, &c, &queue));
        if let Err(e) = spawned {
            log::warn!("storage: no thread to reach the node: {e}");
        }
        let link = Self {
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
fn work(node: &Weak<Client>, mounted: &dyn Volumes, cached: &Cached, queue: &Receiver<Job>) {
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
                    read.mounted = mounted.volumes();
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

fn from<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| e.to_string())
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
    let photos: HashMap<String, usize> = from(field(&volumes, "photos")?)?;
    Ok(Read {
        known: from(field(&stored, "volumes")?)?,
        strategy: from(field(&stored, "strategy")?)?,
        photos: photos
            .into_iter()
            .map(|(id, n)| (VolumeId(id), n))
            .collect(),
        statuses: from(field(&statuses, "statuses")?)?,
        mounted: Vec::new(),
        seq: 0,
    })
}

impl StorageLink for NodeStorage {
    fn volumes(&self) -> Result<Vec<Volume>, String> {
        let read = self.read();
        Ok(registry::merge(read.known.clone(), &read.mounted))
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

/// A card from an import's report line (`helios_library::import::Card`
/// as it serializes).
pub fn card_of(v: &Value) -> Option<Card> {
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
