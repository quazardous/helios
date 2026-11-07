//! The desktop's batch exports, run where the library is (see
//! `docs/export.md`).
//!
//! The shell asks for an export ([`crate::shell::ExportAsk`]: images and
//! settings); the host hands it to an [`Exporter`], which
//!
//! 1. plans it over the library (`helios_library::export::plan`): each
//!    image's file and head version, its output name decided now, into a
//!    serialisable [`BatchRequest`];
//! 2. runs that request on its own scheduler
//!    (`helios_export::batch::run`), reporting to the shell's jobs: a
//!    parent job and an export per image, which name the workspace's
//!    view node, so its Queue panel lists them;
//! 3. renders each image as the darkroom shows it: the file through the
//!    workspace's network, the version's edit applied.
//!
//! What the batch says (failures, its end) goes back to the shell
//! ([`crate::Shell::export_sink`]), which logs it and toasts the end.

use std::sync::Arc;
use std::sync::mpsc::Sender;

use helios_core::jobs::Jobs;
use helios_core::{Bus, Engine, Image, Scheduler};
use helios_export::batch::{self, BatchRequest, Item, Line};
use helios_template::Bundle;
use helios_vault::{ImageId, Store, Vault, VersionId, edit};
use serde_json::json;

use crate::shell::{ExportAsk, OPEN_FOLDER};

/// How many images render side by side: a quarter of the cores, one to
/// four (a raw's render holds a lot of memory).
pub fn parallelism() -> usize {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 4).clamp(1, 4))
}

/// Renders an item as the darkroom shows it: its file through `bundle`'s
/// network, its version's edit applied. A raw goes through its sensor
/// data, never its embedded preview: a network that cannot develop it
/// fails the item, saying why.
pub fn render_item<S: Store>(
    vault: &Vault<S>,
    engine: &Engine,
    bundle: &Bundle,
    item: &Item,
) -> Result<Image, String> {
    let (mut graph, node) = crate::templates::develop_file(&item.source, bundle)?;
    if let Some(version) = &item.version {
        let id = VersionId::from_hex(version)?;
        let version = vault
            .version(&id)?
            .ok_or_else(|| format!("no version {} of the image", &item.image))?;
        // A part of the edit this network has no place for is left out,
        // as the darkroom does.
        edit::apply_to_graph(&mut graph, &version.edit)?;
    }
    let region = graph
        .extent(node)
        .map_err(|e| e.to_string())?
        .ok_or("the image has no extent")?;
    engine
        .render(&graph, node, "output", region)
        .map_err(|e| e.to_string())
}

/// Runs the desktop's exports over a library.
pub struct Exporter<S: Store> {
    vault: Arc<Vault<S>>,
    /// Taken when the exporter drops (see its `Drop`).
    scheduler: Option<Scheduler>,
    engine: Arc<Engine>,
    jobs: Jobs,
    lines: Sender<Line>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// The batches run, by job id, cancelled when the window closes.
    batches: std::sync::Mutex<Vec<String>>,
}

impl<S: Store + Send + 'static> Exporter<S> {
    /// Exports from `vault`, reported to `jobs`, what they say sent to
    /// `lines` (the shell's `export_sink`), `wake` asking the window for a
    /// frame after each.
    pub fn new(
        vault: Arc<Vault<S>>,
        jobs: Jobs,
        lines: Sender<Line>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::with_workers(vault, jobs, lines, wake, parallelism())
    }

    /// [`Exporter::new`] rendering `workers` images side by side.
    pub fn with_workers(
        vault: Arc<Vault<S>>,
        jobs: Jobs,
        lines: Sender<Line>,
        wake: impl Fn() + Send + Sync + 'static,
        workers: usize,
    ) -> Self {
        // Its own bus: the scheduler's and the engine's events are no
        // one's on the desktop; the jobs' go to the shell's.
        let own = Arc::new(Bus::new());
        Exporter {
            vault,
            scheduler: Some(Scheduler::new(own.clone(), workers.max(1))),
            engine: Arc::new(Engine::new(own)),
            jobs,
            lines,
            wake: Arc::new(wake),
            batches: std::sync::Mutex::default(),
        }
    }

    /// Plans an export over the library: the request it runs.
    pub fn plan(&self, bundle: &Bundle, ask: &ExportAsk) -> Result<BatchRequest, String> {
        let images = ask
            .images
            .iter()
            .map(|i| ImageId::from_hex(i))
            .collect::<Result<Vec<_>, _>>()?;
        let node = Some(bundle.template.network.view.node.clone());
        helios_library::export::plan(&self.vault, &images, ask.settings.clone(), node)
    }

    /// Plans and runs an export: its batch job's id.
    pub fn run(&self, bundle: &Bundle, ask: &ExportAsk) -> Result<String, String> {
        let request = self.plan(bundle, ask)?;
        self.run_request(bundle, &request, &ask.open_label)
    }

    /// Runs a planned request: its batch job's id.
    pub fn run_request(
        &self,
        bundle: &Bundle,
        request: &BatchRequest,
        open_label: &str,
    ) -> Result<String, String> {
        batch::make_folder(&request.settings.folder).map_err(|m| m.english)?;
        let render: batch::Render = {
            let (vault, engine, bundle) = (self.vault.clone(), self.engine.clone(), bundle.clone());
            Arc::new(move |item: &Item| render_item(&vault, &engine, &bundle, item))
        };
        let (lines, wake) = (self.lines.clone(), self.wake.clone());
        let said = move |line: Line| {
            let _ = lines.send(line);
            wake();
        };
        let actions = json!([{
            "title": open_label,
            "command": OPEN_FOLDER,
            "args": { "path": request.settings.folder },
        }]);
        let scheduler = self.scheduler.as_ref().expect("the scheduler lives");
        let id = batch::run(scheduler, &self.jobs, request, render, said, actions)?;
        self.batches.lock().expect("batches lock").push(id.clone());
        Ok(id)
    }

    /// Holds the exports queued: none starts until [`Exporter::resume`]
    /// (those under way go on).
    pub fn pause(&self) {
        if let Some(s) = &self.scheduler {
            s.pause();
        }
    }

    pub fn resume(&self) {
        if let Some(s) = &self.scheduler {
            s.resume();
        }
    }

    /// Whether every export ended, waiting up to `timeout` (tests, the
    /// window closing).
    pub fn wait(&self, timeout: std::time::Duration) -> bool {
        self.scheduler
            .as_ref()
            .is_none_or(|s| s.wait_until_idle(timeout))
    }
}

impl<S: Store> Drop for Exporter<S> {
    /// The window closes: the batches still running are cancelled (their
    /// exports queued never run, those rendering write nothing), and the
    /// scheduler, which waits for the renders under way, goes on a thread
    /// of its own so the quit does not wait for them.
    fn drop(&mut self) {
        for id in self.batches.lock().expect("batches lock").drain(..) {
            // A batch that ended refuses: nothing to do.
            let _ = self.jobs.cancel(&id);
        }
        if let Some(scheduler) = self.scheduler.take() {
            let _ = std::thread::Builder::new()
                .name("helios-export-stop".into())
                .spawn(move || drop(scheduler));
        }
    }
}
