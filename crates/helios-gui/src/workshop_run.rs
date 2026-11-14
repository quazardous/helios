//! A workshop run over a set of photos (`workshop.run`, experimental, see
//! `docs/design/workshops-on-the-fly.md`, section 4): the workshop
//! applied in the workspace (a node of its network) run on each photo of
//! a set, a job of the jobs API on a thread of its own.
//!
//! Per photo: its file through the workspace's network with its edit (the
//! head version), a proxy of what enters the workshop node, then
//! [`helios_template::run::photo`]: zones, targets aimed, junctions
//! answered from the workshop's decision book or by the AI within the
//! run's budget, the limits' check. A photo the check applies gets a
//! version continuing its head, labelled "workshop <name> (run <n>)",
//! its metadata holding each value's provenance and the flags; a held
//! photo is not written and is listed with its reasons; a stop ends the
//! run there. A photo whose edit would not change gets no version, so a
//! second run of the same set writes nothing and asks no model.
//!
//! The run is **one library step** ([`LibraryHistory`]): undo takes each
//! run's version out, so the photo's edit is its head before the run
//! again; a photo edited after the run keeps its later edit (its version
//! names the run's, which then stays).

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use helios_core::jobs::{Jobs, Progress, Report, Start};
use helios_core::{Engine, Graph, Message};
use helios_inspect::client::Client;
use helios_template::format::{Limits, OnBreach};
use helios_template::limits::{Action, Budget, Raised};
use helios_template::run::{self, Asker, Book, NoAi, Setup};
use helios_template::{Bundle, Catalogue};
use helios_vault::{Change, Changes, ImageId, Version, edit};
use serde_json::{Value, json};

use crate::library_history::LibraryHistory;

mod review;
pub use review::listing;

/// The kind of a run's job.
pub const JOB_KIND: &str = "workshop-run";

/// What a run is asked: the workshop node of the workspace's network,
/// the photos, and who asks.
#[derive(Clone)]
pub struct RunAsk {
    /// The workshop's node in the network.
    pub node: String,
    /// The workshop's composite, id@version.
    pub workshop: String,
    /// Its name, for the labels.
    pub name: String,
    /// The workspace's catalogue (the workshop's composite resolves in it).
    pub catalogue: Catalogue,
    /// The workspace: each photo's file goes through its network.
    pub bundle: Bundle,
    pub images: Vec<ImageId>,
    /// Over the workshop file's `on_breach`, for this run.
    pub on_breach: Option<OnBreach>,
    /// Who asked: `user`, `agent:<session>`.
    pub who: String,
}

/// A photo's file through the workspace's network.
pub type Load = Arc<dyn Fn(&Path, &Bundle) -> Result<Graph, String> + Send + Sync>;

/// Who answers the junctions of one run.
pub type Askers = Arc<dyn Fn() -> Box<dyn Asker + Send> + Send + Sync>;

/// A held (or stopping) photo and why.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Held {
    pub image: String,
    pub flags: Vec<Raised>,
}

/// What a run did.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Outcome {
    pub run: String,
    pub photos: usize,
    /// Photos given a version.
    pub written: Vec<String>,
    /// Photos whose edit already held the run's values.
    pub unchanged: Vec<String>,
    pub held: Vec<Held>,
    /// Written, each with at least one flag, and its flags.
    pub flagged: Vec<Held>,
    /// The photo the run stopped at (`on_breach: stop`).
    pub stopped: Option<Held>,
    pub failed: Vec<(String, String)>,
    pub cancelled: bool,
    /// AI calls made, and junctions the book answered.
    pub calls: u64,
    pub from_book: u64,
    #[serde(skip)]
    pub changes: Changes,
}

/// Runs workshops over photos of a library, reading and writing it
/// through the node (`workshop.*`, `docs/protocol.md`, "Workshop runs"):
/// a run's thread makes the calls, never the frame.
pub struct Runner {
    node: Arc<Client>,
    engine: Arc<Engine>,
    jobs: Jobs,
    history: LibraryHistory,
    load: Load,
    askers: Askers,
    /// One run at a time per workshop version: they share its book.
    busy: Mutex<Vec<String>>,
    started: AtomicU64,
}

fn seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The run step's label: "Workshop “Backlit” on 24 photos".
pub fn label(name: &str, count: usize) -> egui_pop::api::history::Label {
    let m = Message::new(
        "gui-history-workshop-run",
        "Workshop “{ $name }” on { $count } photos",
    )
    .with("name", name)
    .with("count", count);
    egui_pop::api::history::Label {
        text: m.english.clone(),
        key: Some(m.key),
        args: [
            ("name".to_string(), name.to_string()),
            ("count".to_string(), count.to_string()),
        ]
        .into(),
    }
}

impl Runner {
    /// Runs over the library `node` serves, rendering on `engine` (the desktop's), reported
    /// to `jobs`, each run a step of `history`. Files load through the
    /// workspace's network ([`crate::templates::develop_file`]); no AI
    /// answers junctions until one is given ([`Runner::with_askers`]).
    pub fn new(
        node: Arc<Client>,
        engine: Arc<Engine>,
        jobs: Jobs,
        history: LibraryHistory,
    ) -> Self {
        Self {
            node,
            engine,
            jobs,
            history,
            load: Arc::new(|path, bundle| {
                crate::templates::develop_file(path, bundle).map(|(g, _)| g)
            }),
            askers: Arc::new(|| Box::new(NoAi)),
            busy: Mutex::default(),
            started: AtomicU64::new(0),
        }
    }

    /// Photos' files loaded by `load` instead.
    #[must_use]
    pub fn with_load(mut self, load: Load) -> Self {
        self.load = load;
        self
    }

    /// Junctions answered by what `askers` makes, one per run.
    #[must_use]
    pub fn with_askers(mut self, askers: Askers) -> Self {
        self.askers = askers;
        self
    }

    /// Calls `method` on the node and waits for its answer: on a run's
    /// thread, or a command's, never the frame's.
    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.node.request(method, params).map_err(|e| e.message)
    }

    /// A workshop version's decision book, as the library keeps it.
    fn book_doc(&self, workshop: &str) -> Result<Option<Value>, String> {
        let read = self.call("workshop.book.get", json!({ "workshop": workshop }))?;
        Ok(Some(read["book"].clone()).filter(|b| !b.is_null()))
    }

    /// Writes a workshop version's decision book.
    fn set_book(&self, workshop: &str, book: &Value) -> Result<(), String> {
        self.call(
            "workshop.book.set",
            json!({ "workshop": workshop, "book": book }),
        )
        .map(drop)
    }

    /// Writes (or with `None` drops) a run's record.
    fn set_record(&self, run: &str, record: Option<&Value>) -> Result<(), String> {
        self.call(
            "workshop.record.set",
            json!({ "run": run, "record": record }),
        )
        .map(drop)
    }

    /// Writes `version`: the photo's rows before and after it.
    fn add_version(&self, version: &Version) -> Result<Change, String> {
        let version = serde_json::to_value(version).map_err(|e| e.to_string())?;
        let added = self.call("workshop.version.add", json!({ "version": version }))?;
        serde_json::from_value(added["change"].clone()).map_err(|e| format!("a change: {e}"))
    }

    fn book(&self, workshop: &str) -> Result<(Book, u64), String> {
        let doc = self.book_doc(workshop)?.unwrap_or(Value::Null);
        let runs = doc["runs"].as_u64().unwrap_or(0);
        let book = if doc.is_null() {
            Book::default()
        } else {
            serde_json::from_value(doc).map_err(|e| format!("the decision book: {e}"))?
        };
        Ok((book, runs))
    }

    /// What enters the workshop on `image` (a proxy), the values it holds
    /// there, and the graph with the photo's edit; with its head version
    /// and its file's name.
    #[allow(clippy::type_complexity)]
    fn input(
        &self,
        ask: &RunAsk,
        image: &ImageId,
    ) -> Result<
        (
            helios_core::Image,
            helios_core::Params,
            Graph,
            Option<Version>,
            Option<String>,
        ),
        String,
    > {
        let (graph, head, current, name) = self.opened(ask, image)?;
        let id = graph
            .find(&ask.node)
            .ok_or_else(|| format!("the photo's network has no `{}`", ask.node))?;
        let (from, port) = graph
            .input_ports(id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .next()
            .flatten()
            .ok_or_else(|| format!("nothing feeds `{}`", ask.node))?;
        let extent = graph
            .extent(from)
            .map_err(|e| e.to_string())?
            .ok_or("the photo has no extent")?;
        let (scale, region) = crate::preview::plan(extent);
        let input = self
            .engine
            .render_at(&graph, from, &port, region, scale)
            .map_err(|e| e.to_string())?;
        Ok((input, current, graph, head, name))
    }

    /// The dry run: the photos, `J` (junctions per photo), how many photos
    /// the decision book already answers (the cache's share in `A`), and
    /// the AI calls the run would make at most.
    ///
    /// # Errors
    /// The workshop does not build or the book cannot be read.
    pub fn plan(&self, ask: &RunAsk) -> Result<Value, String> {
        let (_, block, _) = Self::workshop(ask)?;
        let (book, runs) = self.book(&ask.workshop)?;
        let j = block.junctions.len();
        let mut answered = 0usize;
        let mut unreadable = Vec::new();
        if j > 0 {
            for image in &ask.images {
                match self.input(ask, image) {
                    Ok((input, ..)) => {
                        if run::answered(&book, &ask.workshop, &block.junctions, &input) {
                            answered += 1;
                        }
                    }
                    Err(e) => unreadable.push(json!({ "image": image.hex(), "error": e })),
                }
            }
        }
        let asked = (ask.images.len() - answered - unreadable.len()) * j;
        let limits = block.limits.clone().unwrap_or_default();
        let max = Budget::for_run(&limits, j, ask.images.len()).max;
        Ok(json!({
            "workshop": ask.workshop,
            "node": ask.node,
            "photos": ask.images.len(),
            "J": j,
            "A": if ask.images.is_empty() { 0.0 } else { asked as f64 / ask.images.len() as f64 },
            "answered_by_book": answered,
            "calls": asked.min(usize::try_from(max).unwrap_or(usize::MAX)),
            "max_ai_calls": max,
            "runs": runs,
            "unreadable": unreadable,
            "dry_run": true,
        }))
    }

    fn workshop(
        ask: &RunAsk,
    ) -> Result<
        (
            helios_core::Macro,
            helios_template::format::Workshop,
            Limits,
        ),
        String,
    > {
        let r = helios_template::network::composite_ref(&ask.workshop)
            .ok_or_else(|| format!("`{}` is not id@version", ask.workshop))?;
        let file = ask
            .catalogue
            .get_composite(&r)
            .ok_or_else(|| format!("unknown composite `{r}`"))?;
        let block = file
            .workshop
            .clone()
            .ok_or_else(|| format!("`{r}` is not a workshop"))?;
        let built = ask.catalogue.build_composite(&r, 0)?;
        let mut limits = block.limits.clone().unwrap_or_default();
        if let Some(b) = ask.on_breach {
            limits.on_breach = b;
        }
        Ok((built, block, limits))
    }

    /// Starts the run as a job on a thread of its own: its id at once.
    /// One run of a workshop version at a time.
    ///
    /// # Errors
    /// The workshop does not build, or a run of it goes on.
    pub fn start(self: &Arc<Self>, ask: RunAsk) -> Result<Value, String> {
        Self::workshop(&ask)?;
        {
            let mut busy = self.busy.lock().unwrap_or_else(PoisonError::into_inner);
            if busy.contains(&ask.workshop) {
                return Err(format!(
                    "a run of {} goes on: wait for it to end",
                    ask.workshop
                ));
            }
            busy.push(ask.workshop.clone());
        }
        let n = self.started.fetch_add(1, Ordering::Relaxed) + 1;
        let job = format!(
            "{JOB_KIND}-{}-{n}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis())
        );
        let title = Message::new("gui-workshop-run-job", "Workshop “{ $name }”")
            .with("name", ask.name.as_str())
            .english;
        let report = self.jobs.start(
            Start::new(&job, &title, JOB_KIND)
                .total(ask.images.len() as u64)
                .node(&ask.node),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        report.on_cancel(move || flag.store(true, Ordering::Relaxed));
        let me = self.clone();
        let photos = ask.images.len();
        let id = job.clone();
        std::thread::Builder::new()
            .name("workshop-run".into())
            .spawn(move || {
                let done = me.run(&ask, &id, &cancel, Some(&report));
                me.busy
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retain(|w| *w != ask.workshop);
                match done {
                    Ok(o) if o.cancelled => report.cancelled(),
                    Ok(o) => report.finished_with(json!({
                        "written": o.written.len(),
                        "held": o.held,
                        "flagged": o.flagged.len(),
                        "calls": o.calls,
                    })),
                    Err(e) => report.failed(&e),
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(json!({ "run": job, "photos": photos, "dry_run": false }))
    }

    /// Runs `ask` here, its progress told to `report`; `cancel` stops it
    /// before the next photo. What was written is one library step.
    ///
    /// # Errors
    /// The workshop does not build, or the node refuses a read or write.
    pub fn run(
        &self,
        ask: &RunAsk,
        id: &str,
        cancel: &AtomicBool,
        report: Option<&Report>,
    ) -> Result<Outcome, String> {
        let (built, block, limits) = Self::workshop(ask)?;
        let (mut book, runs) = self.book(&ask.workshop)?;
        let n = runs + 1;
        let setup = Setup {
            engine: &self.engine,
            catalogue: &ask.catalogue,
            workshop: &ask.workshop,
            built: &built,
            block: &block,
            limits: &limits,
        };
        let mut budget = Budget::for_run(&limits, block.junctions.len(), ask.images.len());
        let mut asker = (self.askers)();
        let mut out = Outcome {
            run: id.to_string(),
            photos: ask.images.len(),
            ..Outcome::default()
        };
        let version_label = format!("workshop {} (run {n})", ask.name);
        let ranges = helios_template::workshop::ranges(&built);
        let mut record: Vec<Value> = Vec::new();
        let total = ask.images.len() as u64;
        for (i, image) in ask.images.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                out.cancelled = true;
                break;
            }
            let hex = image.hex();
            if let Some(r) = report {
                r.progress(Progress::new().counts(i as u64, total).item(&hex));
            }
            let photo = self
                .input(ask, image)
                .and_then(|(input, current, graph, head, name)| {
                    let p = run::photo(
                        &setup,
                        &input,
                        &current,
                        &mut budget,
                        &mut book,
                        &mut *asker,
                    )?;
                    Ok((
                        p,
                        current,
                        graph,
                        head,
                        input.content_hash().to_string(),
                        name,
                    ))
                });
            let (p, current, graph, head, input, name) = match photo {
                Ok(x) => x,
                Err(e) => {
                    if let Some(r) = report {
                        r.item_failed(&hex, &e);
                    }
                    record.push(json!({ "image": hex, "state": "failed", "error": e }));
                    out.failed.push((hex, e));
                    continue;
                }
            };
            out.calls += p.calls;
            out.from_book += p.from_book;
            let held = Held {
                image: hex.clone(),
                flags: p.checked.flags.clone(),
            };
            let params = run::params_of(&built, &current, &p.checked.values);
            let mut entry = json!({
                "image": hex,
                "name": name,
                "input": input,
                "flags": p.checked.flags,
                "provenance": p.provenance,
                "before": exposed(&ranges, &current),
                "after": exposed(&ranges, &params),
                "head": head.as_ref().map(|h| h.id.hex()),
            });
            match p.action() {
                Action::Stop => {
                    entry["state"] = json!("stopped");
                    record.push(entry);
                    out.stopped = Some(held);
                    break;
                }
                Action::Hold => {
                    entry["state"] = json!("held");
                    record.push(entry);
                    out.held.push(held);
                    continue;
                }
                Action::Apply => {}
            }
            let edit = edit_with(head.as_ref(), &graph, &ask.node, &params)?;
            if head.as_ref().is_some_and(|h| h.edit == edit) {
                entry["state"] = json!("unchanged");
                record.push(entry);
                out.unchanged.push(hex);
                continue;
            }
            let meta = json!({ "workshop_run": {
                "run": id,
                "workshop": ask.workshop,
                "node": ask.node,
                "provenance": p.provenance,
                "flags": p.checked.flags,
            }});
            let at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let version = Version::new(
                *image,
                head.iter().map(|h| h.id).collect(),
                edit,
                &ask.who,
                at,
            )
            .labelled(&version_label)
            .with_meta(Some(meta));
            entry["state"] = json!("applied");
            entry["version"] = json!(version.id.hex());
            record.push(entry);
            let change = self.add_version(&version)?;
            out.changes.images.push(change);
            if !held.flags.is_empty() {
                out.flagged.push(held);
            }
            out.written.push(hex);
        }
        if let Some(r) = report {
            r.progress(Progress::new().counts(total, total));
        }
        let mut doc = serde_json::to_value(&book).map_err(|e| e.to_string())?;
        doc["runs"] = json!(n);
        self.set_book(&ask.workshop, &doc)?;
        let doc = json!({
            "run": id,
            "n": n,
            "workshop": ask.workshop,
            "node": ask.node,
            "name": ask.name,
            "who": ask.who,
            "on_breach": limits.on_breach,
            "calls": out.calls,
            "from_book": out.from_book,
            "cancelled": out.cancelled,
            "photos": record,
        });
        self.set_record(id, Some(&doc))?;
        out.changes.documents.push(helios_vault::DocumentChange {
            ns: helios_vault::workshops::RUNS.into(),
            key: id.into(),
            before: None,
            after: Some(doc),
        });
        self.history.record(
            label(&ask.name, out.written.len()),
            out.changes.clone(),
            seconds(),
        );
        Ok(out)
    }
}

/// The workshop's exposed values in `params`, by name.
fn exposed(
    ranges: &std::collections::BTreeMap<String, helios_template::workshop::Range>,
    params: &helios_core::Params,
) -> Value {
    ranges
        .keys()
        .filter_map(|n| {
            params
                .get(n)
                .and_then(helios_core::Value::as_f64)
                .map(|v| (n.clone(), json!(v)))
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// The photo's edit with the workshop node's parameters set to `params`:
/// its head's edit (or the graph's, for a photo never edited), the node's
/// entry replaced or added.
fn edit_with(
    head: Option<&Version>,
    graph: &Graph,
    node: &str,
    params: &helios_core::Params,
) -> Result<Value, String> {
    let mut edit = match head {
        Some(h) => h.edit.clone(),
        None => edit::of_graph(graph),
    };
    let module = graph
        .find(node)
        .and_then(|id| graph.manifest(id).ok().flatten())
        .map(|m| m.name)
        .ok_or_else(|| format!("no node `{node}`"))?;
    let params = serde_json::to_value(params).map_err(|e| e.to_string())?;
    if !edit["modules"].is_array() {
        edit["modules"] = json!([]);
    }
    let modules = edit["modules"].as_array_mut().expect("an array");
    if let Some(entry) = modules.iter_mut().find(|m| m["name"] == node) {
        entry["params"] = params;
    } else {
        modules.push(json!({ "name": node, "module": module, "params": params }));
        modules.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    }
    Ok(edit)
}
