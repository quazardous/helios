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
        bus.publish_value(
            SOURCE,
            None,
            "helios.extract.run",
            json!({ "run": self.id, "state": state, "tally": self.tally() }),
        );
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
}
