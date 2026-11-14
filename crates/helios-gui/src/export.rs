//! The desktop's batch exports, run where the library is (see
//! `docs/export.md`).
//!
//! The shell asks for an export ([`crate::shell::ExportAsk`]: images and
//! settings); the host hands it to an [`Exporter`], which
//!
//! 1. plans it over the library through the node (`export.plan`): each
//!    image's file and head version, its output name decided now, into a
//!    serialisable [`BatchRequest`];
//! 2. runs that request on its own scheduler
//!    (`helios_export::batch::run`), reporting to the shell's jobs: a
//!    parent job and an export per image, which name the workspace's
//!    view node, so its Queue panel lists them;
//! 3. renders each image as the darkroom shows it: the file through the
//!    workspace's network, the version's edit applied, on the engine the
//!    desktop renders with ([`Exporter::with_engine`]): one cache and one
//!    budget for the process, so what the darkroom computed is not
//!    computed again and an export cannot grow memory without bound.
//!
//! What the batch says (failures, its end) goes back to the shell
//! ([`crate::Shell::export_sink`]), which logs it and toasts the end.
//!
//! The exports are the local node's jobs ([`crate::node::NodeJobs`]): the
//! host hands the exporter to the node ([`crate::node::Link::adopt`]), and
//! a window only starts exports and watches them in the Queue panel.
//! Closing that window leaves them running; the node's stop waits for
//! them, or cancels them when the quit was confirmed as cancelling jobs.

use std::sync::Arc;
use std::sync::mpsc::Sender;

use helios_core::jobs::Jobs;
use helios_core::{Bus, Engine, Image, Scheduler};
use helios_export::batch::{self, BatchRequest, Item, Line};
use helios_inspect::client::Client;
use helios_template::Bundle;
use helios_vault::{Version, edit};
use serde_json::{Value, json};

use crate::shell::{ExportAsk, OPEN_FOLDER};

/// How many images render side by side: a quarter of the cores, one to
/// four (a raw's render holds a lot of memory).
pub fn parallelism() -> usize {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 4).clamp(1, 4))
}

/// Renders an item as the darkroom shows it: its file through `bundle`'s
/// network, its version's edit applied. A raw goes through its sensor
/// data, never its embedded preview: a network that cannot develop it
/// fails the item, saying why. Its version is read through the node
/// (`library.version.get`), on the render's thread.
pub fn render_item(
    node_client: &Client,
    engine: &Engine,
    bundle: &Bundle,
    item: &Item,
) -> Result<Image, String> {
    let (mut graph, node) = crate::templates::develop_file(&item.source, bundle)?;
    if let Some(version) = &item.version {
        let read = node_client
            .request("library.version.get", json!({ "version": version }))
            .map_err(|e| e.message)?;
        let version: Version = match &read["version"] {
            Value::Null => None,
            row => Some(serde_json::from_value(row.clone()).map_err(|e| e.to_string())?),
        }
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

/// Runs the desktop's exports over a library, read through its node.
pub struct Exporter {
    node: Arc<Client>,
    /// Taken when the exporter drops (see its `Drop`).
    scheduler: Option<Scheduler>,
    engine: Arc<Engine>,
    jobs: Jobs,
    lines: Sender<Line>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// The batches run, by job id: what a quit cancels.
    batches: std::sync::Mutex<Vec<String>>,
}

impl Exporter {
    /// Exports from the library `node` reads, reported to `jobs`, what they say sent to
    /// `lines` (the shell's `export_sink`), `wake` asking the window for a
    /// frame after each; rendered on `engine`, the desktop's (the
    /// viewport's: its cache and its budget).
    pub fn new(
        node: Arc<Client>,
        jobs: Jobs,
        lines: Sender<Line>,
        wake: impl Fn() + Send + Sync + 'static,
        engine: Arc<Engine>,
    ) -> Self {
        Self::with_engine(node, jobs, lines, wake, parallelism(), engine)
    }

    /// An exporter rendering `workers` images side by side on an engine of
    /// its own, within the cache service's RAM budget.
    pub fn with_workers(
        node: Arc<Client>,
        jobs: Jobs,
        lines: Sender<Line>,
        wake: impl Fn() + Send + Sync + 'static,
        workers: usize,
    ) -> Self {
        let budget = helios_cache::Service::global()
            .budget(helios_cache::Tier::Ram)
            .map(|b| b as usize);
        let engine = Arc::new(Engine::with_budget(Arc::new(Bus::new()), budget));
        Self::with_engine(node, jobs, lines, wake, workers, engine)
    }

    /// An exporter rendering `workers` images side by side on `engine`.
    pub fn with_engine(
        node: Arc<Client>,
        jobs: Jobs,
        lines: Sender<Line>,
        wake: impl Fn() + Send + Sync + 'static,
        workers: usize,
        engine: Arc<Engine>,
    ) -> Self {
        // Its own bus: the scheduler's events are no one's on the
        // desktop; the jobs' go to the shell's.
        let own = Arc::new(Bus::new());
        Exporter {
            node,
            scheduler: Some(Scheduler::new(own, workers.max(1))),
            engine,
            jobs,
            lines,
            wake: Arc::new(wake),
            batches: std::sync::Mutex::default(),
        }
    }

    /// Hands the exporter to the node: its exports become the node's jobs,
    /// running on when the window that started them closes. The window
    /// keeps the handle returned, to start exports.
    pub fn adopted_by(self, node: &crate::node::Link) -> Arc<Self>
    where
        Self: Send + Sync,
    {
        let exporter = Arc::new(self);
        node.adopt(exporter.clone());
        exporter
    }

    /// The engine it renders with.
    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// Plans an export over the library through the node
    /// (`export.plan`): the request it runs.
    pub fn plan(&self, bundle: &Bundle, ask: &ExportAsk) -> Result<BatchRequest, String> {
        let params = json!({
            "images": ask.images,
            "settings": ask.settings,
            "node": bundle.template.network.view.node,
        });
        let planned = self
            .node
            .request("export.plan", params)
            .map_err(|e| e.message)?;
        serde_json::from_value(planned["request"].clone()).map_err(|e| e.to_string())
    }

    /// Plans and runs an export: its batch job's id.
    pub fn run(&self, bundle: &Bundle, ask: &ExportAsk) -> Result<String, String> {
        let request = self.plan(bundle, ask)?;
        self.run_request(bundle, &request, &ask.open_label)
    }

    /// Plans and runs an export on a thread of its own, so the frame that
    /// asked never waits for the node: `refused` hears why one did not
    /// start (its plan refused, its folder not made).
    pub fn start(
        self: &Arc<Self>,
        bundle: Bundle,
        ask: ExportAsk,
        refused: impl FnOnce(String) + Send + 'static,
    ) where
        Self: Send + Sync,
    {
        let exporter = self.clone();
        let spawned = std::thread::Builder::new()
            .name("helios-export-plan".into())
            .spawn(move || {
                if let Err(e) = exporter.run(&bundle, &ask) {
                    refused(e);
                }
            });
        if let Err(e) = spawned {
            log::warn!("export: no thread to plan it: {e}");
        }
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
            let (node, engine, bundle) = (self.node.clone(), self.engine.clone(), bundle.clone());
            Arc::new(move |item: &Item| render_item(&node, &engine, &bundle, item))
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

impl crate::node::NodeJobs for Exporter {
    fn running(&self) -> Vec<String> {
        let running = self.jobs.cancellable();
        let mut batches = self.batches.lock().expect("batches lock");
        batches.retain(|id| running.contains(id));
        batches.clone()
    }

    /// The batches still running cancelled: their exports queued never
    /// run, those rendering write nothing.
    fn cancel_all(&self) {
        for id in self.batches.lock().expect("batches lock").drain(..) {
            // A batch that ended refuses: nothing to do.
            let _ = self.jobs.cancel(&id);
        }
    }

    fn wait(&self, deadline: std::time::Duration) -> bool {
        Exporter::wait(self, deadline)
    }
}

impl Drop for Exporter {
    /// The last holder lets go (the node, as it stops): the scheduler,
    /// which waits for the renders under way (a cancelled batch's), goes
    /// on a thread of its own so the quit does not wait for them. Nothing
    /// is cancelled here: that is the node's quit's decision.
    fn drop(&mut self) {
        if let Some(scheduler) = self.scheduler.take() {
            let _ = std::thread::Builder::new()
                .name("helios-export-stop".into())
                .spawn(move || drop(scheduler));
        }
    }
}
