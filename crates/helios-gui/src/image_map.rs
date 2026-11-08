//! The image map in the interface: what the import window's sources list
//! is recorded (`docs/design/image-map.md`, section 4), as the settings
//! say (`system.map.record`, `system.map.defaults`, `system.map.exclude`).

use std::path::PathBuf;
use std::sync::Arc;

use helios_library::map::{RECORD, Record, Recorder};
use helios_storage::exclude::{Blacklist, Checker, DEFAULTS, EXCLUDE};
use helios_vault::{Store, Vault};

use crate::import_scan::MapRecorder;

/// The map's settings, read once.
#[derive(Clone, Debug)]
pub struct MapSettings {
    pub record: Record,
    pub defaults: bool,
    /// The JSON of `system.map.exclude`.
    pub exclude: String,
    /// `system.map.limit`, `system.map.keepMissing` and
    /// `system.map.record`, for the job.
    pub prune: helios_library::map_job::Settings,
}

/// The map's settings, from the resolved settings ([`crate::config`]).
pub fn settings(config: &helios_config::Config) -> MapSettings {
    MapSettings {
        record: Record::parse(&config.choice(RECORD)),
        defaults: config.toggle(DEFAULTS),
        exclude: config.text(EXCLUDE),
        prune: helios_library::map_job::Settings {
            limit: config.number(helios_library::map_job::LIMIT) as usize,
            keep_missing_days: config.number(helios_library::map_job::KEEP_MISSING) as u64,
            record: Record::parse(&config.choice(RECORD)),
        },
    }
}

/// The blacklist the settings make, Helios's own folders (`helios`) in
/// it; rules that do not read are left out, said in the log.
fn blacklist(settings: &MapSettings, helios: &[PathBuf]) -> Blacklist {
    Blacklist::here(helios.to_vec(), settings.defaults, &settings.exclude).unwrap_or_else(|e| {
        log::warn!("image map: {e}; the user's rules are left out");
        Blacklist::here(helios.to_vec(), settings.defaults, "").expect("no rule reads")
    })
}

/// Starts the image map's job on a thread of its own (`map` in the jobs
/// centre): it reads what `feed` says is on screen first, then the rest
/// in the note's order, giving way to imports, exports, backups and
/// renders on `jobs`' bus and to the battery. It scans the volumes `feed`
/// says were plugged in or asked for, as `system.map.record` allows.
pub fn start_job<S: Store + 'static>(
    vault: Arc<Vault<S>>,
    jobs: helios_core::jobs::Jobs,
    settings: MapSettings,
    helios: Vec<PathBuf>,
    feed: helios_library::map_job::Feed,
) {
    use helios_library::map_job::{BusLoad, Disk, Job, SystemPower};
    let checker = Checker::new(blacklist(&settings, &helios));
    let known = {
        let vault = vault.clone();
        move || {
            let mounted = helios_storage::system().volumes();
            helios_storage::registry::merged(&vault, &mounted).unwrap_or(mounted)
        }
    };
    let volumes = known();
    let load = BusLoad::new(jobs.bus());
    let spawned = std::thread::Builder::new()
        .name("image map".into())
        .spawn(move || {
            let stop = std::sync::atomic::AtomicBool::new(false);
            Job::new(
                vault,
                Box::new(Disk),
                checker,
                volumes,
                feed,
                settings.prune,
            )
            .volumes_from(known)
            .load(Box::new(load))
            .power(Box::new(SystemPower))
            .run(&jobs, &stop);
        });
    if let Err(e) = spawned {
        log::warn!("image map: the job did not start: {e}");
    }
}

/// Starts the shared thumbnail cache's re-check on a thread of its own
/// (`helios_library::thumb_check`): what `feed` says is on screen first,
/// then what was, then the Library's files, giving way as the map's job
/// does.
pub fn start_thumb_check<S: Store + 'static>(
    vault: Arc<Vault<S>>,
    jobs: &helios_core::jobs::Jobs,
    cache: Arc<helios_thumbs::ThumbCache>,
    settings: &MapSettings,
    helios: Vec<PathBuf>,
    feed: helios_library::map_job::Feed,
) {
    use helios_library::map_job::{BusLoad, Disk, SystemPower};
    use helios_library::thumb_check::Check;
    let checker = Checker::new(blacklist(settings, &helios));
    let mounted = helios_storage::system().volumes();
    let volumes = helios_storage::registry::merged(&vault, &mounted).unwrap_or(mounted);
    let load = BusLoad::new(jobs.bus());
    let spawned = std::thread::Builder::new()
        .name("thumbnail re-check".into())
        .spawn(move || {
            // The pool lists the cache's folder; the re-check waits for it.
            cache.list();
            let stop = std::sync::atomic::AtomicBool::new(false);
            Check::new(
                vault,
                Box::new(Disk),
                Box::new(cache),
                checker,
                volumes,
                feed,
            )
            .load(Box::new(load))
            .power(Box::new(SystemPower))
            .run(&stop);
        });
    if let Err(e) = spawned {
        log::warn!("thumbnail re-check: it did not start: {e}");
    }
}

/// The recorder the import window's scans use: each makes one on its
/// thread, over the volumes known now. `helios` names Helios's own
/// folders (the vault's, its data and cache), which are never recorded.
pub fn recorder<S: Store + 'static>(
    vault: Arc<Vault<S>>,
    settings: MapSettings,
    helios: Vec<PathBuf>,
) -> MapRecorder {
    Arc::new(move |seen_by: &str| {
        if !settings.record.listings() {
            return None;
        }
        let blacklist = blacklist(&settings, &helios);
        let mounted = helios_storage::system().volumes();
        let volumes = helios_storage::registry::merged(&vault, &mounted).unwrap_or(mounted);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Some(Recorder::new(
            Box::new(vault.clone()),
            Checker::new(blacklist),
            volumes,
            seen_by,
            now,
        ))
    })
}
