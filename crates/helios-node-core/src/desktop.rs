//! The node's library as the desktop in its process uses it: the parts
//! the desktop wires (its service and client, the backups, the volume
//! scan, the import accounts, the image map, the health check's inputs)
//! and the operations its windows run (an import, the library folder, an
//! image opened from the library), built here on the node's side. The
//! desktop holds a [`Library`] and never the vault itself
//! (`cargo xtask check`, the desktop's rule).
//!
//! A [`Library`] comes from the node that owns the vault
//! ([`crate::node::LocalNode::library`]); tests and tools make one on a
//! store of their own ([`Library::on_store`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::jobs::Jobs;
use helios_core::{Bus, Message};
use helios_storage::Volumes;
use helios_vault::{ImageId, Location, Store, Vault};
use serde_json::Value;

use crate::endpoint::VaultService;
use crate::promises::Promises;

/// The node's library, for the desktop in its process.
pub struct Library<S: Store> {
    vault: Arc<Vault<S>>,
    bus: Arc<Bus>,
    promises: Option<Arc<Promises<S>>>,
}

impl<S: Store> Clone for Library<S> {
    fn clone(&self) -> Self {
        Self {
            vault: self.vault.clone(),
            bus: self.bus.clone(),
            promises: self.promises.clone(),
        }
    }
}

impl<S: Store + 'static> Library<S> {
    /// A library on `store` of its own, announcing its changes on `bus`
    /// (tests and tools; the desktop's comes from its node).
    pub fn on_store(store: S, bus: Arc<Bus>) -> Self {
        Self::on(Arc::new(Vault::new(store).with_bus(bus.clone())), bus)
    }

    /// The library on `vault`, opened by its owner, whose changes are
    /// announced on `bus`.
    pub fn on(vault: Arc<Vault<S>>, bus: Arc<Bus>) -> Self {
        Self {
            vault,
            bus,
            promises: None,
        }
    }

    /// Its promises (the imports by rule) run by `promises`, the node's.
    #[must_use]
    pub fn with_promises(mut self, promises: Arc<Promises<S>>) -> Self {
        self.promises = Some(promises);
        self
    }

    /// The vault, for tests and tools. The desktop's code does not call
    /// it: it reads and writes through the node's client and the
    /// operations below.
    pub fn vault(&self) -> &Arc<Vault<S>> {
        &self.vault
    }

    /// The bus the library's changes are announced on.
    pub fn bus(&self) -> &Arc<Bus> {
        &self.bus
    }

    /// The node's service on this library, as `instance`: what the
    /// desktop's in-process client and the endpoint advertised to the
    /// CLI and agents answer with.
    pub fn service(&self, instance: &str) -> VaultService<S> {
        let service = VaultService::new(
            self.vault.clone(),
            self.bus.clone(),
            Jobs::new(self.bus.clone()),
            instance,
        );
        match &self.promises {
            Some(promises) => service.with_promises(promises.clone()),
            None => service,
        }
    }

    /// The node's backups on this library, followed in `jobs`.
    pub fn backups(
        &self,
        jobs: Jobs,
        volumes: Arc<dyn Volumes>,
        warn: impl Fn(String) + Send + Sync + 'static,
    ) -> crate::jobs::Backups<S> {
        crate::jobs::Backups::new(self.vault.clone(), jobs, volumes, warn)
    }

    /// Starts the scan of the volumes and where each photo is
    /// (`helios_storage::scan`), the library folder read at each scan.
    pub fn scan_volumes(
        &self,
        volumes: Arc<dyn Volumes>,
        node: String,
        every: std::time::Duration,
        done: impl Fn(Result<helios_storage::scan::ScanReport, String>) + Send + 'static,
    ) -> helios_storage::scan::Scanner {
        let library = self.vault.clone();
        helios_storage::scan::Scanner::start(
            self.vault.clone(),
            volumes,
            node,
            move || {
                helios_library::import::library_root(&library)
                    .ok()
                    .flatten()
            },
            every,
            done,
        )
    }

    /// The import accounts, kept in the library (public facts only).
    pub fn accounts(&self) -> Arc<dyn helios_connect::account::AccountBook>
    where
        S: Send,
    {
        Arc::new(helios_connect::account::VaultAccounts(self.vault.clone()))
    }

    /// What records the listings the import window reads into the image
    /// map (`crate::map::recorder`).
    pub fn map_recorder(
        &self,
        settings: crate::map::MapSettings,
        helios: Vec<PathBuf>,
    ) -> impl Fn(&str) -> Option<helios_library::map::Recorder> + Send + Sync + 'static {
        crate::map::recorder(self.vault.clone(), settings, helios)
    }

    /// Starts the image map's job (`crate::map::ImageMap`).
    pub fn start_image_map(
        &self,
        jobs: Jobs,
        settings: crate::map::MapSettings,
        helios: Vec<PathBuf>,
        feed: helios_library::map_job::Feed,
        thumbs: Option<crate::map::ThumbCheck>,
    ) -> Arc<crate::map::ImageMap> {
        crate::map::ImageMap::start(self.vault.clone(), jobs, settings, helios, feed, thumbs)
    }

    /// The health check's inputs over this library and its `file`.
    pub fn health_inputs(
        &self,
        volumes: Arc<dyn Volumes>,
        file: Option<PathBuf>,
    ) -> crate::health::HealthInputs<S> {
        crate::health::HealthInputs::new(volumes, Some(self.vault.clone()), file)
    }

    /// Fills, in `jobs`, the capture fields of the photos imported before
    /// Helios kept them, then their culling signals; each says why it
    /// stopped to `warn`. Runs on the caller's thread.
    pub fn backfill(&self, jobs: &Jobs, warn: impl Fn(String)) {
        if let Err(e) = helios_library::capture::backfill(
            &self.vault,
            jobs,
            &helios_library::capture::Files,
            None,
        ) {
            warn(format!("capture backfill: {e}"));
        }
        if let Err(e) =
            helios_library::cull::backfill(&self.vault, jobs, &helios_library::cull::Files, None)
        {
            warn(format!("cull backfill: {e}"));
        }
    }

    /// The library folder, where chosen photos are copied when an import
    /// names none.
    pub fn folder(&self) -> Result<Option<PathBuf>, String> {
        helios_library::import::library_root(&self.vault)
    }

    /// Sets the library folder.
    pub fn set_folder(&self, dir: &Path) -> Result<(), String> {
        helios_library::import::set_library_root(&self.vault, dir)
    }

    /// Whether a camera has a default look.
    pub fn has_camera_looks(&self) -> bool {
        self.vault.look_defaults().is_ok_and(|d| !d.is_empty())
    }

    /// Imports `sources` (`helios_library::import::import`).
    pub fn import(
        &self,
        jobs: &Jobs,
        sources: &[PathBuf],
        options: &helios_library::import::Options,
        now: u64,
        emit: &mut impl FnMut(&Value),
    ) -> Result<helios_library::import::Report, String> {
        helios_library::import::import(&self.vault, jobs, sources, options, now, emit)
    }

    /// Imports the items a service's page chose
    /// (`helios_library::fetch::import_remote`).
    pub fn import_remote(
        &self,
        jobs: &Jobs,
        remote: &helios_library::fetch::Remote<'_>,
        options: &helios_library::import::Options,
        now: u64,
        emit: &mut impl FnMut(&Value),
    ) -> Result<helios_library::import::Report, String> {
        helios_library::fetch::import_remote(&self.vault, jobs, remote, options, now, emit)
    }

    /// Which of a service's `items` the library holds already.
    pub fn in_library(
        &self,
        source: &helios_connect::SourceRef,
        items: &[helios_connect::Item],
    ) -> Result<Vec<bool>, String> {
        helios_library::fetch::in_library(&self.vault, source, items)
    }

    /// The cameras' default looks on the raws an import brought in,
    /// inside its own step: how many.
    pub fn camera_looks_on_import(
        &self,
        changes: &mut helios_vault::Changes,
        now: u64,
    ) -> Result<usize, Message> {
        crate::calibrate::look::default_on_import(
            &self.vault,
            changes,
            now,
            &crate::calibrate::look::raw_camera,
        )
    }

    /// Clears the "changed outside Helios" mark of `images` (every photo
    /// when empty): how many.
    pub fn acknowledge_changed(&self, images: &[ImageId]) -> Result<usize, String> {
        self.vault.acknowledge_changed(images)
    }

    /// Forgets a folder in the image map: its files not in the library.
    pub fn forget_folder(
        &self,
        mounted: Vec<helios_storage::Volume>,
        path: &Path,
        now: u64,
    ) -> Result<usize, String> {
        let volumes = helios_storage::registry::merged(&self.vault, &mounted).unwrap_or(mounted);
        helios_library::map::forget_folder(&self.vault, &volumes, path, now)
    }

    /// Forgets a volume in the image map: its files not in the library.
    pub fn forget_volume(&self, volume: &str, now: u64) -> Result<usize, String> {
        let id = helios_vault::storage::VolumeId(volume.into());
        self.vault.forget(Some(&id), None, now)
    }

    /// Imports one file (an image opened by name): its id.
    pub fn import_file(&self, path: &Path, now: u64) -> Result<ImageId, String> {
        self.vault.import_file(path, now)
    }

    /// The copy of `image` to read: the library's, a card's only without
    /// one.
    pub fn file_to_read(&self, image: &ImageId) -> Result<Option<PathBuf>, String> {
        Ok(self.vault.file_to_read(image)?.map(|l| l.path))
    }

    /// Whether a copy of the image (by id) is present.
    pub fn image_present(&self, id: &str) -> bool {
        ImageId::from_hex(id)
            .ok()
            .and_then(|image| self.vault.locations(&image).ok())
            .is_some_and(|l| l.iter().any(Location::present))
    }
}
