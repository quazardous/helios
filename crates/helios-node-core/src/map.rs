//! The image map as the node's jobs (`docs/design/image-map.md`,
//! section 4): the map's job, the shared thumbnail cache's re-check and
//! the recorder the import window's scans write sightings with, each on
//! the node's vault, never a window's.
//!
//! - [`ImageMap::start`] runs the map's job (`map` in the jobs centre) on
//!   a thread of the node's: what its feed says is on screen first, then
//!   the rest, giving way to imports, exports, backups and renders and to
//!   the battery; and, given a thumbnail cache, the cache's re-check
//!   beside it ([`helios_library::thumb_check`]). Adopted by the node
//!   ([`crate::node::Link::adopt`]), both stop when the node stops, before
//!   the vault closes; a quit does not ask about them (they only keep the
//!   map current).
//! - Each stands in the node's promises while it runs, with no end and
//!   no total ([`crate::job_promises::Standing`]): the Activity focus
//!   lists them, the status bar does not count them. Each says there what
//!   it does (the file at hand, how many so far) or what it waits on
//!   (nothing new, another job, the battery, a pause); a watch flags, in
//!   the log and the note, one that says it works and then says nothing
//!   past [`crate::job_promises::STALLED`].
//! - [`recorder`] makes, on a scan's thread, the recorder of what an
//!   import window's sources list, over the volumes known then.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use helios_core::Message;
use helios_core::jobs::Jobs;
use helios_library::map::{Record, Recorder};
use helios_library::map_job::{BusLoad, Disk, Feed, Job, Status, SystemPower};
use helios_library::thumb_check::{Check, Thumbs};
use helios_storage::Volume;
use helios_storage::exclude::{Blacklist, Checker};
use helios_vault::{Store, Vault};

use crate::job_promises::{STALLED, Standing};
use crate::node::NodeJobs;

/// The kind of the thumbnail re-check's standing promise.
pub const THUMBS_KIND: &str = "thumbs";

/// What the map records and keeps (`system.map.*`).
#[derive(Clone, Debug)]
pub struct MapSettings {
    pub record: Record,
    /// Whether the default exclusions apply.
    pub defaults: bool,
    /// The JSON of `system.map.exclude`.
    pub exclude: String,
    /// `system.map.limit`, `system.map.keepMissing` and
    /// `system.map.record`, for the job.
    pub prune: helios_library::map_job::Settings,
}

/// The blacklist the settings make, Helios's own folders (`helios`) in
/// it; rules that do not read are left out, said in the log.
fn blacklist(settings: &MapSettings, helios: &[PathBuf]) -> Blacklist {
    Blacklist::here(helios.to_vec(), settings.defaults, &settings.exclude).unwrap_or_else(|e| {
        log::warn!(target: "helios::node", "image map: {e}; the user's rules are left out");
        Blacklist::here(helios.to_vec(), settings.defaults, "").expect("no rule reads")
    })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The volumes known now: those mounted, merged with the vault's.
fn known<S: Store>(vault: &Vault<S>) -> Vec<Volume> {
    let mounted = helios_storage::system().volumes();
    helios_storage::registry::merged(vault, &mounted).unwrap_or(mounted)
}

/// The thumbnail cache the re-check keeps, and what to do on its thread
/// before it starts (list the cache's folder).
pub type ThumbCheck = (Box<dyn Thumbs>, Box<dyn FnOnce() + Send>);

/// A standing promise as what a background loop says its status to.
struct Said<S: Store>(Standing<S>);

impl<S: Store> Status for Said<S>
where
    Standing<S>: Send,
{
    fn working(&mut self, done: u64, item: &str) {
        self.0.working(done, item);
    }

    fn waiting(&mut self, done: u64, why: &str) {
        self.0.waiting(done, why);
    }
}

/// The checks of the standing works' watches, each flagging its work when
/// it went silent while working (`job_promises::StandingWatch::check`).
type Watches = Arc<Mutex<Vec<Box<dyn Fn(Duration) -> bool + Send>>>>;

/// How often the watch looks at the standing works.
const WATCH_EVERY: Duration = Duration::from_secs(10);

/// The map's job and the thumbnail re-check, run by the node.
pub struct ImageMap {
    stop: Arc<AtomicBool>,
    /// Threads still running, and their ends heard.
    running: Arc<(Mutex<usize>, Condvar)>,
}

impl ImageMap {
    /// Starts the map's job on `vault`, reporting to `jobs`, and, with
    /// `thumbs` (the cache and what to do on its thread before the
    /// re-check: list the cache's folder), the thumbnail re-check. `feed`
    /// says what is on screen and which volumes were plugged in or asked
    /// for; `helios` names Helios's own folders, never recorded.
    pub fn start<S: Store + 'static>(
        vault: Arc<Vault<S>>,
        jobs: Jobs,
        settings: MapSettings,
        helios: Vec<PathBuf>,
        feed: Feed,
        thumbs: Option<ThumbCheck>,
    ) -> Arc<Self> {
        let map = Arc::new(Self {
            stop: Arc::default(),
            running: Arc::default(),
        });
        let watches: Watches = Arc::default();
        if let Some((cache, before)) = thumbs {
            let checker = Checker::new(blacklist(&settings, &helios));
            let volumes = known(&vault);
            let load = BusLoad::new(jobs.bus());
            let (vault, feed, watches) = (vault.clone(), feed.clone(), watches.clone());
            map.spawn("thumbnail re-check", move |stop| {
                // A promise with no end while it runs (Activity), saying
                // what the re-check does or waits on.
                let standing = Standing::start(vault.clone(), THUMBS_KIND, "Thumbnail re-check");
                let watch = standing.watch();
                lock(&watches).push(Box::new(move |after| watch.check(after)));
                // The pool lists the cache's folder; the re-check waits for it.
                standing.working(0, "Listing the thumbnail cache");
                before();
                Check::new(vault, Box::new(Disk), cache, checker, volumes, feed)
                    .load(Box::new(load))
                    .power(Box::new(SystemPower))
                    .status(Box::new(Said(standing)))
                    .run(stop);
            });
        }
        let checker = Checker::new(blacklist(&settings, &helios));
        let volumes = known(&vault);
        let load = BusLoad::new(jobs.bus());
        let listed = vault.clone();
        let watching = watches.clone();
        map.spawn("image map", move |stop| {
            // Pause and Resume on its promise pause its feed: what the
            // jobs centre's Pause did on the `map-N` job.
            let pausing = feed.clone();
            let standing = Standing::start_pausable(
                vault.clone(),
                helios_library::map_job::KIND,
                "Image map",
                move |paused| pausing.pause(paused),
            );
            let watch = standing.watch();
            lock(&watching).push(Box::new(move |after| watch.check(after)));
            standing.working(0, "Starting");
            Job::new(
                vault,
                Box::new(Disk),
                checker,
                volumes,
                feed,
                settings.prune,
            )
            .volumes_from(move || known(&listed))
            .load(Box::new(load))
            .power(Box::new(SystemPower))
            .status(Box::new(Said(standing)))
            .run(&jobs, stop);
        });
        // The watch: a standing work silent past STALLED while it says it
        // works is flagged (the log, its note).
        map.spawn("background watch", move |stop| {
            let mut looked = std::time::Instant::now();
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(200));
                if looked.elapsed() >= WATCH_EVERY {
                    looked = std::time::Instant::now();
                    for check in lock(&watches).iter() {
                        check(STALLED);
                    }
                }
            }
        });
        map
    }

    fn spawn(&self, name: &str, work: impl FnOnce(&AtomicBool) + Send + 'static) {
        let (stop, running) = (self.stop.clone(), self.running.clone());
        *running.0.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        let ended = running.clone();
        let spawned = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                work(&stop);
                let (count, woken) = &*ended;
                *count.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
                woken.notify_all();
            });
        if let Err(e) = spawned {
            log::warn!(target: "helios::node", "{name}: it did not start: {e}");
            let (count, _) = &*running;
            *count.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
        }
    }

    /// Asks both to stop and waits for them, at most `deadline`.
    fn halt(&self, deadline: Duration) -> bool {
        self.stop.store(true, Ordering::SeqCst);
        let (count, woken) = &*self.running;
        let count = count.lock().unwrap_or_else(PoisonError::into_inner);
        let (count, _) = woken
            .wait_timeout_while(count, deadline, |c| *c > 0)
            .unwrap_or_else(PoisonError::into_inner);
        *count == 0
    }
}

impl NodeJobs for ImageMap {
    /// None a quit would cancel: they only keep the map current.
    fn running(&self) -> Vec<String> {
        Vec::new()
    }

    fn name(&self) -> Message {
        Message::new("node-end-map", "Stopping the image map")
    }

    /// Both asked to stop; the node's stop waits for them
    /// ([`NodeJobs::wait`]).
    fn cancel_all(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    fn wait(&self, deadline: Duration) -> bool {
        // Only the node's stop waits: they never end by themselves.
        self.halt(deadline)
    }
}

/// The recorder the import window's scans use: each makes one on its
/// thread with `seen_by` (who saw the files), over the volumes known
/// then; none when `system.map.record` records no listing. `helios`
/// names Helios's own folders (the vault's, its data and cache), which
/// are never recorded.
pub fn recorder<S: Store + 'static>(
    vault: Arc<Vault<S>>,
    settings: MapSettings,
    helios: Vec<PathBuf>,
) -> impl Fn(&str) -> Option<Recorder> + Send + Sync + 'static {
    move |seen_by: &str| {
        if !settings.record.listings() {
            return None;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Some(Recorder::new(
            Box::new(vault.clone()),
            Checker::new(blacklist(&settings, &helios)),
            known(&vault),
            seen_by,
            now,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_vault::MemoryStore;

    fn settings(record: &str) -> MapSettings {
        MapSettings {
            record: Record::parse(record),
            defaults: true,
            exclude: String::new(),
            prune: helios_library::map_job::Settings {
                limit: 1000,
                keep_missing_days: 30,
                record: Record::parse(record),
            },
        }
    }

    #[test]
    fn the_node_stops_the_map_job_before_the_vault_closes() {
        let vault = Arc::new(Vault::new(MemoryStore::default()));
        let jobs = Jobs::new(Arc::new(helios_core::Bus::new()));
        let map = ImageMap::start(
            vault,
            jobs,
            settings("import"),
            Vec::new(),
            Feed::default(),
            None,
        );
        assert!(
            map.wait(Duration::from_secs(20)),
            "the map job did not stop"
        );
        assert!(map.running().is_empty());
    }

    #[test]
    fn a_recorder_is_made_only_when_listings_are_recorded() {
        let vault = Arc::new(Vault::new(MemoryStore::default()));
        let none = recorder(vault.clone(), settings("import"), Vec::new());
        assert!(none("import").is_none());
        let some = recorder(vault, settings("listings"), Vec::new());
        assert!(some("import").is_some());
    }
}
