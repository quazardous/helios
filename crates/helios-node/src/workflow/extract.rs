//! Extractor runs over library images (`docs/design/extractors.md`,
//! sections 1 and 8).
//!
//! An [`ExtractRun`] works a set of library images through extractor
//! nodes, each a queueing node of a journal, as the export runs of
//! [`super::runs`] do:
//!
//! - an item is a library image, named [`image_item`] (`image:` and its
//!   content hash), so a submission reads no image;
//! - each [`ExtractNode`] is one use of an LLM extractor, after the nodes
//!   it names (or after [`SUBMIT`]); a row per (image, node) concludes
//!   once, so a stopped or crashed run resumes where it was, and the
//!   cache makes a re-submission call no model again;
//! - before anything is entered, [`Ai::check_run`] checks every node's
//!   route: a node nothing serves refuses the submission, naming it;
//! - each call goes through [`Ai::run_extract`] with the run's cache (for
//!   a vault, `vault.extract_cache(Some(run))`), the use's overrides and
//!   the prompt's inputs; the caller's [`ImageSource`] chooses the image
//!   sent (the embedded preview, or the head version rendered small, as
//!   `image_version`);
//! - what happens is said on the bus as facts: `helios.extract.run` (submitted,
//!   refused, progress, finished, stopped) and `helios.extract.item` (one per
//!   concluded row). Nothing here draws.
//!
//! Removing the run's vault documents (`remove_run`) drops its run-only
//! results; kept ones stay on the photos.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};

use helios_ai::extract::ExtractCall;
use helios_ai::prompts::PromptRef;
use helios_ai::routing::Overrides;
use helios_ai::{Ai, Capability, Need, PromptCall, PromptFailure, RunCheck};
use helios_core::Bus;
use helios_extract::{ExtractorDef, ExtractorUse, Kind, ResultCache};
use helios_journal::{Journal, Node as JournalNode, Outcome, Status, Store, Workflow};
use serde::Serialize;
use serde_json::{Value, json};

use super::SUBMIT;

/// The prefix of a library image's item id.
pub const IMAGE: &str = "image:";

/// The source of the facts this module publishes.
const SOURCE: &str = "extract";

/// Attempts of a row in all; a failed call is tried again after 2 s,
/// then 4 s. A reply that does not fit is not tried again past the
/// corrective retry `run_extract` already made.
const ATTEMPTS: u32 = 3;
const RETRY_MS: u64 = 2_000;

/// The item id of a library image, from its content hash (64 hex
/// digits).
pub fn image_item(hex: &str) -> String {
    format!("{IMAGE}{hex}")
}

/// The image of an item id, when it names a library image.
pub fn image_of(item: &str) -> Option<&str> {
    item.strip_prefix(IMAGE)
}

/// An extractor node of a run: one use of an LLM extractor.
#[derive(Clone, Debug)]
pub struct ExtractNode {
    /// Its name in the diagram, unique in the run.
    pub name: String,
    /// The extractor nodes it waits for; empty: it follows [`SUBMIT`].
    pub after: Vec<String>,
    pub def: ExtractorDef,
    pub using: ExtractorUse,
    /// The prompt's input variables (language, a count...), the same for
    /// every image; part of the cache key.
    pub input: Value,
    /// The node's and diagram's route overrides.
    pub overrides: Overrides,
}

impl ExtractNode {
    /// A node using `def` pinned to its version, after [`SUBMIT`].
    pub fn new(name: &str, def: ExtractorDef, input: Value) -> Self {
        Self {
            name: name.into(),
            after: Vec::new(),
            using: ExtractorUse::of(&def),
            def,
            input,
            overrides: Overrides::default(),
        }
    }

    /// The helios-ai prompt it calls, and the capability it needs.
    fn prompt(&self) -> Result<(PromptRef, Capability), String> {
        let Kind::Llm(llm) = &self.def.kind else {
            return Err(format!(
                "node `{}`: `{}` is computed in code, not by a model",
                self.name, self.def.id
            ));
        };
        let Some(id) = llm.prompt_id.as_deref() else {
            return Err(format!(
                "node `{}`: `{}` has its own prompt text, which runs do not send yet; \
                 give it a helios-ai prompt (`prompt_id`)",
                self.name, self.def.id
            ));
        };
        let capability = match llm.capability {
            helios_extract::Capability::Vision => Capability::Vision,
            helios_extract::Capability::Chat => Capability::Chat,
        };
        Ok((PromptRef::parse(id)?, capability))
    }

    /// What the pre-run check needs of it.
    fn need(&self) -> Result<Need, String> {
        let (prompt, capability) = self.prompt()?;
        Ok(Need {
            node: Some(self.name.clone()),
            capability,
            prompt: Some(prompt.to_string()),
            overrides: self.overrides.clone(),
        })
    }

    /// The context inputs of the cache key: each input variable, as JSON.
    fn inputs(&self) -> BTreeMap<String, String> {
        self.input
            .as_object()
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.to_string())).collect())
            .unwrap_or_default()
    }
}

/// The image an extractor is sent, as the caller chose it.
pub struct SentImage {
    pub bytes: Vec<u8>,
    /// The head version rendered for the call, `None` for the embedded
    /// preview: part of the cache key.
    pub version: Option<String>,
}

/// Where a run reads the image of each item.
pub trait ImageSource {
    /// The image sent for `image` (its content hash).
    fn image(&self, image: &str) -> Result<SentImage, String>;
}

/// A run's counts, over its (image, node) rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Tally {
    /// Rows to conclude: images × nodes.
    pub total: u64,
    pub done: u64,
    /// Failed for good, or cancelled.
    pub failed: u64,
    /// Rows this process concluded from the cache, without a model call.
    pub hits: u64,
    /// Rows this process concluded after a model call.
    pub calls: u64,
}

/// How a call to [`ExtractRun::work`] ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Ended {
    /// No row is left open or claimable.
    Finished,
    /// Asked to stop: what concluded stays, the rest waits for the next
    /// call.
    Stopped,
    /// Rows wait for their retry delay.
    Waiting,
}

/// A run of extractor nodes over library images.
pub struct ExtractRun<S: Store> {
    pub id: String,
    nodes: Vec<ExtractNode>,
    journal: Journal<S>,
    /// Item ids, in the order they are worked.
    items: Vec<String>,
    known: BTreeSet<String>,
    hits: u64,
    calls: u64,
    /// Told the run's counts each time they are said on the bus.
    on_tally: Option<Box<dyn Fn(Tally) + Send>>,
}

/// The pre-run check of `nodes` over `images` images.
pub fn check(ai: &Ai, nodes: &[ExtractNode], images: usize) -> Result<RunCheck, String> {
    let needs = nodes
        .iter()
        .map(ExtractNode::need)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ai.check_run(&needs, Some(images as u64)))
}

fn workflow(nodes: &[ExtractNode]) -> Result<Workflow, String> {
    let mut all = vec![JournalNode::new(SUBMIT)];
    for n in nodes {
        if n.name == SUBMIT {
            return Err(format!(
                "a node is named `{SUBMIT}`, the name a run gives its root: rename it"
            ));
        }
        let parents: Vec<&str> = if n.after.is_empty() {
            vec![SUBMIT]
        } else {
            n.after.iter().map(String::as_str).collect()
        };
        all.push(
            JournalNode::new(&n.name)
                .after(&parents)
                .lease(super::LEASE_MS)
                .retry(ATTEMPTS, RETRY_MS, true),
        );
    }
    Workflow::new(all)
}

/// A failure another attempt would not change: the model answered, and
/// its reply did not fit even after the corrective retry.
fn for_good(failure: &PromptFailure) -> bool {
    !failure.attempts.is_empty() && failure.attempts.iter().all(|a| a.raw.is_some())
}

impl<S: Store> ExtractRun<S> {
    /// Opens the run `id` over `store`, where its rows are kept: a run
    /// opened again over the rows a stopped or crashed process left gives
    /// back their running rows and goes on where it was. `images` are
    /// the run's library images (content hashes), in the order they are
    /// worked; those not entered yet are entered.
    pub fn open(
        id: &str,
        nodes: Vec<ExtractNode>,
        images: &[String],
        store: S,
        now: u64,
    ) -> Result<Self, String> {
        let journal = Journal::new(workflow(&nodes)?, store);
        journal.recover(now);
        let mut run = Self {
            id: id.into(),
            nodes,
            journal,
            items: Vec::new(),
            known: BTreeSet::new(),
            hits: 0,
            calls: 0,
            on_tally: None,
        };
        run.add(images, now);
        Ok(run)
    }

    /// Checks every node's route, then opens the run: a node nothing
    /// serves refuses the submission before anything is entered, and the
    /// refusal is said on the bus.
    pub fn submit(
        ai: &Ai,
        bus: &Bus,
        id: &str,
        nodes: Vec<ExtractNode>,
        images: &[String],
        store: S,
        now: u64,
    ) -> Result<Self, String> {
        let checked = check(ai, &nodes, images.len());
        let refusal = match &checked {
            Ok(c) => c.refusal(),
            Err(e) => Some(e.clone()),
        };
        if let Some(why) = refusal {
            bus.publish_value(
                SOURCE,
                None,
                "helios.extract.run",
                json!({ "run": id, "state": "refused", "reason": why }),
            );
            return Err(why);
        }
        let run = Self::open(id, nodes, images, store, now)?;
        let leaves = checked.map(|c| c.leaves).unwrap_or_default();
        bus.publish_value(
            SOURCE,
            None,
            "helios.extract.run",
            json!({
                "run": id,
                "state": "submitted",
                "images": run.items.len(),
                "nodes": run.nodes.iter().map(|n| &n.name).collect::<Vec<_>>(),
                "leaves": leaves,
            }),
        );
        Ok(run)
    }

    /// Enters the `images` not in the run yet; returns how many.
    pub fn add(&mut self, images: &[String], now: u64) -> usize {
        let mut fresh: Vec<String> = Vec::new();
        for item in images.iter().map(String::as_str).map(image_item) {
            if self.known.insert(item.clone()) {
                fresh.push(item);
            }
        }
        self.items.extend(fresh.iter().cloned());
        let refs: Vec<&str> = fresh
            .iter()
            .map(String::as_str)
            .filter(|i| self.journal.row(i, SUBMIT).is_none())
            .collect();
        if !refs.is_empty()
            && let Ok(lease) = self.journal.claim(SUBMIT, refs.len(), &refs, now)
            && let Err(e) = self.journal.conclude(&lease, Outcome::Done, now)
        {
            log::warn!("extract run {}: {e}", self.id);
        }
        fresh.len()
    }

    /// The run's counts.
    pub fn tally(&self) -> Tally {
        let mut t = Tally {
            total: (self.items.len() * self.nodes.len()) as u64,
            hits: self.hits,
            calls: self.calls,
            ..Tally::default()
        };
        for n in &self.nodes {
            for item in &self.items {
                match self.journal.row(item, &n.name).map(|r| r.status) {
                    Some(Status::Done | Status::Skipped | Status::Omitted) => t.done += 1,
                    Some(Status::Failed | Status::Cancelled) => t.failed += 1,
                    _ => {}
                }
            }
        }
        t
    }

    /// Tells `hook` the run's counts each time they are said on the bus
    /// (each concluded row, and when the run stops or finishes): the
    /// runner reports them as its job's progress.
    pub fn on_tally(&mut self, hook: impl Fn(Tally) + Send + 'static) {
        self.on_tally = Some(Box::new(hook));
    }

    /// The row of `image` at `node`, for a client or a test.
    pub fn status(&self, image: &str, node: &str) -> Option<Status> {
        self.journal.row(&image_item(image), node).map(|r| r.status)
    }

    /// Cancels the rows not concluded: they are never claimed again.
    pub fn cancel(&self, now: u64) -> Result<usize, String> {
        let refs: Vec<&str> = self.items.iter().map(String::as_str).collect();
        self.journal
            .cancel(&refs, None, now)
            .map(|w| w.len())
            .map_err(|e| e.to_string())
    }

    fn publish(&self, bus: &Bus, state: &str) {
        let tally = self.tally();
        bus.publish_value(
            SOURCE,
            None,
            "helios.extract.run",
            json!({ "run": self.id, "state": state, "tally": tally }),
        );
        if let Some(hook) = &self.on_tally {
            hook(tally);
        }
    }

    /// Works the run's rows, one at a time, node after node: each claimed
    /// row's image is read from `source`, its extractor run through
    /// `cache`, and the row concluded (a failure retried by the node's
    /// retry). Checks `stop` between rows. `now` gives the time in
    /// milliseconds.
    pub fn work(
        &mut self,
        ai: &Ai,
        cache: &dyn ResultCache,
        source: &dyn ImageSource,
        bus: &Bus,
        stop: &AtomicBool,
        now: &dyn Fn() -> u64,
    ) -> Ended {
        loop {
            let mut worked = false;
            for index in 0..self.nodes.len() {
                loop {
                    if stop.load(Ordering::Relaxed) {
                        self.publish(bus, "stopped");
                        return Ended::Stopped;
                    }
                    let refs: Vec<&str> = self.items.iter().map(String::as_str).collect();
                    let name = self.nodes[index].name.clone();
                    let Ok(lease) = self.journal.claim(&name, 1, &refs, now()) else {
                        break;
                    };
                    let Some(item) = lease.items.first().cloned() else {
                        break;
                    };
                    worked = true;
                    let outcome = self.one(ai, cache, source, bus, index, &item);
                    if let Err(e) = self.journal.conclude(&lease, outcome, now()) {
                        log::warn!("extract run {}: {e}", self.id);
                    }
                    self.publish(bus, "progress");
                }
            }
            if !worked {
                break;
            }
        }
        let open = self.nodes.iter().any(|n| {
            self.items
                .iter()
                .any(|i| self.journal.row(i, &n.name).is_none_or(|r| r.status.open()))
        });
        if open {
            Ended::Waiting
        } else {
            self.publish(bus, "finished");
            Ended::Finished
        }
    }

    /// Runs one node on one item; says the row's end on the bus.
    fn one(
        &mut self,
        ai: &Ai,
        cache: &dyn ResultCache,
        source: &dyn ImageSource,
        bus: &Bus,
        index: usize,
        item: &str,
    ) -> Outcome {
        let node = &self.nodes[index];
        let fact = |outcome: &str, extra: Value| {
            let mut payload = json!({
                "run": self.id,
                "item": item,
                "node": node.name,
                "outcome": outcome,
            });
            if let (Some(p), Some(e)) = (payload.as_object_mut(), extra.as_object()) {
                p.extend(e.clone());
            }
            bus.publish_value(SOURCE, None, "helios.extract.item", payload);
        };
        let Some(image) = image_of(item) else {
            let why = format!("`{item}` is not a library image");
            fact("failed", json!({ "error": why }));
            return Outcome::FailedForGood(why);
        };
        let prepared = node.prompt().and_then(|(reference, _)| {
            let sent = source.image(image)?;
            let mut call = PromptCall::new(reference, node.input.clone());
            call.images = vec![sent.bytes];
            call.overrides = node.overrides.clone();
            call.extractor = Some(node.def.clone());
            call.purpose = format!("extract run {} node {}", self.id, node.name);
            call.run = Some(self.id.clone());
            Ok(ExtractCall {
                call,
                using: node.using.clone(),
                image: image.to_string(),
                image_version: sent.version,
                inputs: node.inputs(),
            })
        });
        let call = match prepared {
            Ok(c) => c,
            Err(why) => {
                fact("failed", json!({ "error": why }));
                return Outcome::FailedForGood(why);
            }
        };
        match ai.run_extract(call, cache) {
            Ok(x) => {
                fact(
                    "done",
                    json!({ "hit": x.hit, "key": x.result.key, "model": x.result.model }),
                );
                if x.hit {
                    self.hits += 1;
                } else {
                    self.calls += 1;
                }
                Outcome::Done
            }
            Err(failure) => {
                let why = failure.to_string();
                fact("failed", json!({ "error": why }));
                if for_good(&failure) {
                    Outcome::FailedForGood(why)
                } else {
                    Outcome::Failed(why)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use helios_ai::fake::{FakeRuntime, FakeSystem};
    use helios_ai::routing::Route;
    use helios_ai::secrets::MemorySecrets;
    use helios_ai::store::Paths;
    use helios_extract::{KeyDecl, KeyType};
    use helios_vault::journal::VaultJournal;

    use super::*;

    const CAPTION: &str =
        r#"{"caption": "A yellow disc on a gradient", "keywords": ["disc", "sky"]}"#;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("helios-extract-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ai(name: &str) -> (Ai, Arc<FakeRuntime>) {
        let runtime = Arc::new(
            FakeRuntime::new()
                .model("qwen2.5vl:7b", &["completion", "vision"])
                .model("gemma3:4b", &["completion", "vision"])
                .reply("qwen2.5vl:7b", CAPTION)
                .reply("gemma3:4b", "I think it is a disc."),
        );
        let system = FakeSystem::linux()
            .output(
                "df -Pk /",
                Ok("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/x 900000000 1 800000000 1% /\n"),
            )
            .output("id -u", Ok("1000\n"));
        let ai = Ai::with_parts(
            Paths::under(&dir(name)),
            runtime.clone(),
            Arc::new(MemorySecrets::default()),
            Arc::new(system),
        )
        .unwrap();
        ai.routing_set(
            Some(Capability::Vision),
            None,
            Some(Route::new("ollama", "qwen2.5vl:7b")),
        )
        .unwrap();
        (ai, runtime)
    }

    fn chats(runtime: &FakeRuntime) -> usize {
        runtime
            .calls()
            .iter()
            .filter(|c| c.path().ends_with("/chat") || c.path().ends_with("/completions"))
            .count()
    }

    fn caption(id: &str, keep: bool) -> ExtractorDef {
        let mut def: ExtractorDef = serde_json::from_value(json!({
            "id": id,
            "version": 1,
            "title": "Caption",
            "keep": keep,
            "llm": { "capability": "vision", "prompt_id": "image.describe@1" },
            "keys": [],
        }))
        .unwrap();
        def.keys = vec![
            KeyDecl::new("caption", KeyType::Text),
            KeyDecl::new("keywords", KeyType::Label)
                .finite(&["disc", "sky"])
                .multi(),
        ];
        def
    }

    fn node(name: &str, def: ExtractorDef) -> ExtractNode {
        ExtractNode::new(name, def, json!({ "language": "English", "keywords": 3 }))
    }

    /// Every image is the test card; the preview is sent.
    struct Cards;

    impl ImageSource for Cards {
        fn image(&self, _image: &str) -> Result<SentImage, String> {
            Ok(SentImage {
                bytes: helios_ai::images::test_card(32),
                version: None,
            })
        }
    }

    /// The test card, raising `stop` once `after` images were read.
    struct Stopping<'a> {
        read: std::sync::atomic::AtomicUsize,
        after: usize,
        stop: &'a AtomicBool,
    }

    impl ImageSource for Stopping<'_> {
        fn image(&self, image: &str) -> Result<SentImage, String> {
            if self.read.fetch_add(1, Ordering::Relaxed) + 1 >= self.after {
                self.stop.store(true, Ordering::Relaxed);
            }
            Cards.image(image)
        }
    }

    fn images(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{i:064x}")).collect()
    }

    fn vault() -> Arc<helios_vault::Vault<helios_vault::MemoryStore>> {
        Arc::new(helios_vault::Vault::new(
            helios_vault::MemoryStore::default(),
        ))
    }

    fn clock() -> impl Fn() -> u64 {
        let t = std::sync::atomic::AtomicU64::new(1_000);
        move || t.fetch_add(1, Ordering::Relaxed)
    }

    fn kinds(bus: &Bus, kind: &str) -> Vec<Value> {
        bus.since(0)
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.payload)
            .collect()
    }

    #[test]
    fn a_run_concludes_each_image_once_per_node_and_a_rerun_calls_no_model() {
        let (ai, runtime) = ai("once");
        let bus = Bus::new();
        let vault = vault();
        let nodes = vec![node("caption", caption("caption", true)), {
            let mut n = node("again", caption("caption.b", false));
            n.after = vec!["caption".into()];
            n
        }];
        let imgs = images(3);
        let store = VaultJournal::new(vault.clone(), "run-a");
        let mut run =
            ExtractRun::submit(&ai, &bus, "run-a", nodes.clone(), &imgs, store, 1_000).unwrap();
        let cache = vault.extract_cache(Some("run-a"));
        let stop = AtomicBool::new(false);
        let ended = run.work(&ai, &cache, &Cards, &bus, &stop, &clock());
        assert_eq!(ended, Ended::Finished);
        let t = run.tally();
        assert_eq!(
            (t.total, t.done, t.failed, t.calls, t.hits),
            (6, 6, 0, 6, 0)
        );
        assert_eq!(chats(&runtime), 6);
        assert_eq!(kinds(&bus, "helios.extract.item").len(), 6);
        let states: Vec<_> = kinds(&bus, "helios.extract.run")
            .iter()
            .map(|p| p["state"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(states.first().map(String::as_str), Some("submitted"));
        assert_eq!(states.last().map(String::as_str), Some("finished"));

        // The same run opened again: every row concluded, nothing called.
        let store = VaultJournal::new(vault.clone(), "run-a");
        let mut again = ExtractRun::open("run-a", nodes.clone(), &imgs, store, 2_000).unwrap();
        assert_eq!(again.tally().done, 6);
        again.work(&ai, &cache, &Cards, &bus, &stop, &clock());
        assert_eq!(chats(&runtime), 6);

        // Another run over the same images reads the cache: no call. A
        // run's results are read by others while its run document is
        // kept (the runs layer writes it).
        vault
            .set_workflow_documents(&[(
                "runs".into(),
                "run-a".into(),
                Some(json!({ "id": "run-a" })),
            )])
            .unwrap();
        let store = VaultJournal::new(vault.clone(), "run-b");
        let mut other = ExtractRun::open("run-b", nodes, &imgs, store, 3_000).unwrap();
        let cache_b = vault.extract_cache(Some("run-b"));
        other.work(&ai, &cache_b, &Cards, &bus, &stop, &clock());
        assert_eq!(other.tally().hits, 6);
        assert_eq!(chats(&runtime), 6);

        // Removing run-a drops its run-only results; the kept ones stay.
        assert_eq!(vault.run_extracts("run-a").unwrap().len(), 3);
        vault
            .workflow_transaction(|tx| tx.remove_run("run-a"))
            .unwrap();
        assert!(vault.run_extracts("run-a").unwrap().is_empty());
        assert_eq!(vault.kept_extracts("caption").unwrap().len(), 3);
    }

    #[test]
    fn a_stopped_run_resumes_without_calling_again_and_cancel_keeps_what_concluded() {
        let (ai, runtime) = ai("resume");
        let bus = Bus::new();
        let vault = vault();
        let nodes = vec![node("caption", caption("caption", false))];
        let imgs = images(4);
        let cache = vault.extract_cache(Some("run-r"));
        let store = VaultJournal::new(vault.clone(), "run-r");
        let mut run =
            ExtractRun::submit(&ai, &bus, "run-r", nodes.clone(), &imgs, store, 1_000).unwrap();
        // Stop after two rows, as a kill would: the second image read
        // raises the flag, which the next claim sees.
        let stop = AtomicBool::new(false);
        let source = Stopping {
            read: std::sync::atomic::AtomicUsize::new(0),
            after: 2,
            stop: &stop,
        };
        let ended = run.work(&ai, &cache, &source, &bus, &stop, &clock());
        assert_eq!(ended, Ended::Stopped);
        assert_eq!(run.tally().done, 2);
        assert_eq!(chats(&runtime), 2);
        drop(run);

        // Opened again (a restart): only what was left is called.
        let stop = AtomicBool::new(false);
        let store = VaultJournal::new(vault.clone(), "run-r");
        let mut run = ExtractRun::open("run-r", nodes.clone(), &imgs, store, 5_000).unwrap();
        assert_eq!(
            run.work(&ai, &cache, &Cards, &bus, &stop, &clock()),
            Ended::Finished
        );
        assert_eq!(run.tally().done, 4);
        assert_eq!(chats(&runtime), 4, "no image is called twice");

        // Cancel: what concluded stays, a new image is never worked.
        let mut more = imgs.clone();
        more.push(format!("{:064x}", 99));
        run.add(&more, 6_000);
        assert_eq!(run.cancel(6_000).unwrap(), 1);
        run.work(&ai, &cache, &Cards, &bus, &stop, &clock());
        let t = run.tally();
        assert_eq!((t.done, t.failed), (4, 1));
        assert_eq!(chats(&runtime), 4);
        assert_eq!(run.status(&imgs[0], "caption"), Some(Status::Done));
    }

    #[test]
    fn a_node_nothing_serves_refuses_the_submission_before_anything_runs() {
        let (ai, runtime) = ai("refuse");
        let bus = Bus::new();
        let mut n = node("animals", caption("animals", false));
        n.overrides.node = Some(helios_ai::routing::RouteOverride::route(
            "nowhere", "none:1b",
        ));
        let err = ExtractRun::submit(
            &ai,
            &bus,
            "run-x",
            vec![n],
            &images(2),
            helios_journal::MemoryStore::default(),
            1_000,
        )
        .err()
        .unwrap();
        assert!(err.contains("animals"), "{err}");
        assert_eq!(chats(&runtime), 0);
        let refused = kinds(&bus, "helios.extract.run");
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0]["state"], "refused");
    }

    #[test]
    fn a_reply_that_never_fits_fails_for_good_and_other_rows_go_on() {
        let (ai, runtime) = ai("bad");
        let bus = Bus::new();
        let vault = vault();
        let mut bad = node("bad", caption("bad", false));
        bad.overrides.node = Some(helios_ai::routing::RouteOverride::route(
            "ollama",
            "gemma3:4b",
        ));
        let good = node("good", caption("good", false));
        let store = VaultJournal::new(vault.clone(), "run-f");
        let mut run = ExtractRun::submit(
            &ai,
            &bus,
            "run-f",
            vec![bad, good],
            &images(2),
            store,
            1_000,
        )
        .unwrap();
        let cache = vault.extract_cache(Some("run-f"));
        let ended = run.work(&ai, &cache, &Cards, &bus, &AtomicBool::new(false), &clock());
        assert_eq!(ended, Ended::Finished);
        let t = run.tally();
        assert_eq!((t.done, t.failed), (2, 2));
        // Two tries per bad row (the corrective retry), none after.
        assert_eq!(chats(&runtime), 2 * 2 + 2);
        let failed = kinds(&bus, "helios.extract.item")
            .into_iter()
            .filter(|p| p["outcome"] == "failed")
            .count();
        assert_eq!(failed, 2);
    }

    mod runner {
        use std::sync::mpsc;

        use helios_core::jobs::Jobs;
        use helios_vault_sqlite::SqliteStore;

        use super::super::super::extract_runner::{ExtractRunner, ExtractState, ExtractSubmit};
        use super::*;

        fn runner(
            bus: &Arc<Bus>,
            vault: Option<crate::workflow::runs::SharedVault>,
        ) -> Arc<ExtractRunner> {
            Arc::new(ExtractRunner::new(
                bus.clone(),
                Jobs::new(bus.clone()),
                vault,
                "node-test",
            ))
        }

        fn ask(ai: Ai, source: Arc<dyn ImageSource + Send + Sync>, n: usize) -> ExtractSubmit {
            ExtractSubmit {
                title: None,
                nodes: vec![node("caption", caption("caption", true))],
                images: images(n),
                ai: Arc::new(ai),
                source,
            }
        }

        fn job_event(bus: &Bus, kind: &str) -> Vec<Value> {
            kinds(bus, kind)
                .into_iter()
                .filter(|p| p["id"].as_str().is_some_and(|i| i.starts_with("extract-")))
                .collect()
        }

        #[test]
        fn a_submitted_run_is_worked_as_a_job_and_writes_its_document() {
            let (ai, runtime) = ai("runner-done");
            let bus = Arc::new(Bus::new());
            let path = dir("runner-done").join("runs.sqlite");
            let vault = Arc::new(helios_vault::Vault::new(SqliteStore::open(&path).unwrap()));
            let runner = runner(&bus, Some(vault.clone()));
            let answer = runner.submit(ask(ai, Arc::new(Cards), 3)).unwrap();
            let id = answer["run"].as_str().unwrap().to_string();
            assert!(id.starts_with("extract-"), "{answer}");
            assert_eq!(answer["total"], 3);
            assert_eq!(answer["already"], false);
            runner.join(&id);

            let status = runner.status(&id).unwrap();
            assert_eq!(status["state"], json!(ExtractState::Finished));
            assert_eq!(status["tally"]["done"], 3);
            assert_eq!(status["tally"]["failed"], 0);
            assert_eq!(chats(&runtime), 3);

            // The job, which the node mirrors as a promise.
            let started = job_event(&bus, "helios.job.started");
            assert_eq!(started.len(), 1);
            assert_eq!(started[0]["kind"], "extract");
            assert_eq!(started[0]["total"], 3);
            let finished = job_event(&bus, "helios.job.finished");
            assert_eq!(finished.len(), 1, "{:?}", kinds(&bus, "helios.job.failed"));
            assert_eq!(finished[0]["result"]["tally"]["done"], 3);

            // The run document, which the export runner leaves alone.
            let doc = vault.workflow_document("runs", &id).unwrap().unwrap();
            assert!(crate::workflow::extract_runner::is_extract(&doc));
            assert_eq!(doc["state"], "finished");
            assert_eq!(doc["images"], 3);
            assert_eq!(doc["owner"], "node-test");
            assert_eq!(doc["nodes"][0]["extractor"], "caption");
            assert!(doc["finished"].is_u64());
        }

        /// Sends the test card once the test lets each image go.
        struct Gated(Mutex<mpsc::Receiver<()>>);

        impl ImageSource for Gated {
            fn image(&self, image: &str) -> Result<SentImage, String> {
                self.0
                    .lock()
                    .unwrap()
                    .recv()
                    .map_err(|_| "the test is gone".to_string())?;
                Cards.image(image)
            }
        }

        use std::sync::Mutex;

        #[test]
        fn cancelling_the_job_stops_the_run_and_keeps_what_concluded() {
            let (ai, _runtime) = ai("runner-cancel");
            let bus = Arc::new(Bus::new());
            let runner = runner(&bus, None);
            let (go, gate) = mpsc::channel();
            let answer = runner
                .submit(ask(ai, Arc::new(Gated(Mutex::new(gate))), 3))
                .unwrap();
            let id = answer["run"].as_str().unwrap().to_string();
            // The same run again, while at work: given back.
            let (again_ai, _) = super::ai("runner-cancel-again");
            let again = runner.submit(ask(again_ai, Arc::new(Cards), 3)).unwrap();
            assert_eq!(again["run"], id.as_str());
            assert_eq!(again["already"], true);

            runner.cancel(&id).unwrap();
            go.send(()).unwrap();
            runner.join(&id);

            let status = runner.status(&id).unwrap();
            assert_eq!(status["state"], json!(ExtractState::Cancelled));
            assert_eq!(status["tally"]["done"], 1, "{status}");
            assert_eq!(status["tally"]["failed"], 2, "{status}");
            assert_eq!(job_event(&bus, "helios.job.cancelled").len(), 1);
            assert!(job_event(&bus, "helios.job.finished").is_empty());
        }

        #[test]
        fn the_verb_submits_a_run_listed_with_its_items_beside_the_export_runs() {
            use helios_inspect::control::ExtractUse;

            use crate::workflow::runs::{Context, Workflows};

            let (ai, runtime) = ai("runner-verb");
            let bus = Arc::new(Bus::new());
            let path = dir("runner-verb").join("runs.sqlite");
            let vault = Arc::new(helios_vault::Vault::new(SqliteStore::open(&path).unwrap()));
            let mut catalog = helios_extract::Catalog::default();
            catalog.add(caption("caption", true)).unwrap();
            let runner = Arc::new(
                ExtractRunner::new(
                    bus.clone(),
                    Jobs::new(bus.clone()),
                    Some(vault.clone()),
                    "node-test",
                )
                .with_catalog(catalog),
            );
            let uses = vec![ExtractUse {
                name: "caption".into(),
                extractor: "caption@1".into(),
                after: Vec::new(),
                input: json!({ "language": "English", "keywords": 3 }),
                overrides: Value::Null,
            }];
            let items: Vec<String> = images(2).iter().map(|i| image_item(i)).collect();

            // No AI handed to the runner yet: refused, nothing entered.
            assert!(runner.request(None, &uses, &items).is_err());
            runner.set_engine(Arc::new(ai), Arc::new(Cards));
            // An extractor the catalogue does not know, a file item: refused.
            let mut unknown = uses.clone();
            unknown[0].extractor = "caption@9".into();
            assert!(runner.request(None, &unknown, &items).is_err());
            assert!(
                runner
                    .request(None, &uses, &["/a.png".to_string()])
                    .is_err()
            );
            assert!(job_event(&bus, "helios.job.started").is_empty());

            let answer = runner.request(Some("Captions"), &uses, &items).unwrap();
            let id = answer["run"].as_str().unwrap().to_string();
            assert!(id.starts_with("extract-"), "{answer}");
            assert_eq!(answer["total"], 2);
            runner.join(&id);
            assert_eq!(chats(&runtime), 2);

            // The job of kind extract, and the run document.
            let started = job_event(&bus, "helios.job.started");
            assert_eq!(started.len(), 1);
            assert_eq!(started[0]["kind"], "extract");
            assert_eq!(started[0]["id"], answer["job"]);
            let doc = vault.workflow_document("runs", &id).unwrap().unwrap();
            assert_eq!(doc["kind"], "extract");
            assert_eq!(doc["title"], "Captions");
            assert_eq!(doc["state"], "finished");

            // `workflow.runs` lists it beside the export runs.
            let workflows = Workflows::new(Context {
                bus: bus.clone(),
                engine: Arc::new(helios_core::Engine::new(bus.clone())),
                scheduler: Arc::new(helios_core::Scheduler::new(bus.clone(), 1)),
                jobs: Jobs::new(bus.clone()),
                vault: None,
                workers: 1,
                owner: "node-test".into(),
                keep_ms: 0,
            });
            workflows.attach_extracts(runner.clone());
            let runs = workflows.runs();
            let listed: Vec<&Value> = runs["runs"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["id"] == id.as_str())
                .collect();
            assert_eq!(listed.len(), 1, "{runs}");
            let run = listed[0];
            assert_eq!(run["kind"], "extract");
            assert_eq!(run["title"], "Captions");
            assert_eq!(run["state"], "finished");
            assert_eq!(run["items"], 2);
            assert_eq!(run["job"], answer["job"]);
            assert_eq!(run["read_only"], false);
            assert_eq!(run["nodes"]["caption"]["done"], 2, "{run}");
            assert_eq!(run["nodes"]["caption"]["waiting"], 0, "{run}");
            assert_eq!(run["nodes"]["caption"]["parents"], json!([SUBMIT]));

            // `workflow.items` pages its items, at its node.
            let page = workflows.items(&id, None, None, 0, 10).unwrap();
            assert_eq!(page["node"], "caption");
            assert_eq!(page["total"], 2);
            let listed: Vec<&str> = page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["item"].as_str().unwrap())
                .collect();
            assert_eq!(listed, items.iter().map(String::as_str).collect::<Vec<_>>());
            assert_eq!(page["items"][0]["state"], "done");
            assert_eq!(page["items"][0]["row"]["status"], "done");
            let second = workflows.items(&id, Some("caption"), None, 1, 10).unwrap();
            assert_eq!(second["items"].as_array().unwrap().len(), 1);
            assert_eq!(second["items"][0]["item"], items[1].as_str());
            let waiting = workflows
                .items(&id, Some("caption"), Some("waiting"), 0, 10)
                .unwrap();
            assert_eq!(waiting["total"], 0);
            assert!(workflows.items(&id, Some("nope"), None, 0, 10).is_err());
            assert!(workflows.items("extract-none", None, None, 0, 10).is_err());
        }

        /// The vault's promise of job `job`, once it satisfies `ok`.
        fn promise_until(
            vault: &crate::workflow::runs::SharedVault,
            job: &str,
            ok: impl Fn(&helios_vault::promises::State) -> bool,
        ) -> helios_vault::promises::Promise {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                let found = vault
                    .promises(&helios_vault::promises::Filter {
                        owner: Some(job.into()),
                        ..helios_vault::promises::Filter::default()
                    })
                    .unwrap();
                if let Some(p) = found.into_iter().next()
                    && ok(&p.state)
                {
                    return p;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "job `{job}`: its promise never got there"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        fn caption_use() -> Vec<helios_inspect::control::ExtractUse> {
            vec![helios_inspect::control::ExtractUse {
                name: "caption".into(),
                extractor: "caption@1".into(),
                after: Vec::new(),
                input: json!({ "language": "English", "keywords": 3 }),
                overrides: Value::Null,
            }]
        }

        fn captions_runner(
            bus: &Arc<Bus>,
            vault: &crate::workflow::runs::SharedVault,
        ) -> Arc<ExtractRunner> {
            let mut catalog = helios_extract::Catalog::default();
            catalog.add(caption("caption", true)).unwrap();
            Arc::new(
                ExtractRunner::new(
                    bus.clone(),
                    Jobs::new(bus.clone()),
                    Some(vault.clone()),
                    "node-test",
                )
                .with_catalog(catalog),
            )
        }

        #[test]
        fn the_verb_extracts_two_library_photos_through_the_opened_engine() {
            use crate::workflow::library_source::LibraryPreviews;

            let dir = dir("runner-library");
            let vault = Arc::new(helios_vault::Vault::new(
                SqliteStore::open(&dir.join("library.sqlite")).unwrap(),
            ));
            // Two photos in the library, two files.
            let photos: Vec<String> = [24, 40]
                .iter()
                .map(|side| {
                    let file = dir.join(format!("card-{side}.png"));
                    std::fs::write(&file, helios_ai::images::test_card(*side)).unwrap();
                    vault.import_file(&file, 1_000).unwrap().hex()
                })
                .collect();
            let bus = Arc::new(Bus::new());
            let mirror =
                helios_node_core::job_promises::JobPromises::start(vault.clone(), bus.subscribe());
            let runner = captions_runner(&bus, &vault);
            let (ai, runtime) = ai("runner-library");
            let ai = Arc::new(ai);
            let opened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            {
                let (ai, vault, opened) = (ai.clone(), vault.clone(), opened.clone());
                runner.set_opener(Box::new(move || {
                    opened.fetch_add(1, Ordering::SeqCst);
                    let source: Arc<dyn ImageSource + Send + Sync> =
                        Arc::new(LibraryPreviews::new(vault.clone()));
                    Ok((ai.clone(), source))
                }));
            }
            let items: Vec<String> = photos.iter().map(|i| image_item(i)).collect();
            let answer = runner.request(None, &caption_use(), &items).unwrap();
            let id = answer["run"].as_str().unwrap().to_string();
            assert_eq!(answer["total"], 2);
            runner.join(&id);

            // Both photos concluded, each with one model call.
            let status = runner.status(&id).unwrap();
            assert_eq!(status["state"], json!(ExtractState::Finished));
            assert_eq!(status["tally"]["done"], 2, "{status}");
            assert_eq!(status["tally"]["failed"], 0, "{status}");
            assert_eq!(chats(&runtime), 2);
            // Each result kept in the vault's cache, on its photo.
            for photo in &photos {
                let kept = vault.kept_extract("caption", photo).unwrap();
                assert!(kept.is_some(), "no caption kept for {photo}");
            }
            // The run document and the job's promise end done.
            let doc = vault.workflow_document("runs", &id).unwrap().unwrap();
            assert_eq!(doc["state"], "finished");
            let job = answer["job"].as_str().unwrap();
            let promise = promise_until(&vault, job, helios_vault::promises::State::is_settled);
            let helios_vault::promises::State::Done { result } = promise.state else {
                panic!("{promise:?}")
            };
            assert_eq!(result["done"], 2, "{result}");
            assert_eq!(promise.kind, "extract");

            // The engine was opened once, on the first request.
            let again = runner.request(Some("Again"), &caption_use(), &items[..1]);
            assert!(again.is_ok(), "{again:?}");
            assert_eq!(opened.load(Ordering::SeqCst), 1);
            runner.join(again.unwrap()["run"].as_str().unwrap());
            mirror.stop();
        }

        #[test]
        fn a_missing_route_or_engine_refuses_the_verb_with_its_reason() {
            let dir = dir("runner-refused");
            let vault = Arc::new(helios_vault::Vault::new(
                SqliteStore::open(&dir.join("library.sqlite")).unwrap(),
            ));
            let bus = Arc::new(Bus::new());
            let runner = captions_runner(&bus, &vault);
            let items = vec![image_item(&images(1)[0])];

            // No library: the node says so.
            runner.set_unavailable("this node has no library");
            let refused = runner.request(None, &caption_use(), &items).unwrap_err();
            assert!(format!("{refused:?}").contains("no library"), "{refused:?}");
            // The AI service that does not open: its reason, tried again
            // on the next request.
            runner.set_opener(Box::new(|| Err("the AI service: no settings".into())));
            let refused = runner.request(None, &caption_use(), &items).unwrap_err();
            assert!(
                format!("{refused:?}").contains("no settings"),
                "{refused:?}"
            );

            // No route for the vision capability: the pre-run check names
            // the node, nothing entered.
            let unrouted = Ai::with_parts(
                Paths::under(&super::dir("runner-unrouted")),
                Arc::new(FakeRuntime::new()),
                Arc::new(MemorySecrets::default()),
                Arc::new(FakeSystem::linux()),
            )
            .unwrap();
            runner.set_engine(Arc::new(unrouted), Arc::new(Cards));
            let refused = runner.request(None, &caption_use(), &items).unwrap_err();
            let reason = format!("{refused:?}");
            assert!(reason.contains("caption"), "{reason}");
            assert!(job_event(&bus, "helios.job.started").is_empty());
            assert!(vault.workflow_documents("runs").unwrap().is_empty());
        }
    }
}
