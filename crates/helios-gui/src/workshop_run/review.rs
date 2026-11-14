//! The review of a workshop run (`workshop.review`, experimental, see
//! `docs/design/workshops-on-the-fly.md`, section 4, "The review"): the
//! run's record (`workshops/runs` in the vault) listed flagged photos
//! first, and what a person or their agent does from it.
//!
//! - **accept** a held (or stopping) photo: written with the values the
//!   run found for it; the user's alone;
//! - **revert** a written photo: the run's version taken out, its edit
//!   before the run current again; refused when a later edit continues
//!   it; the user's alone;
//! - **fix** a photo: the workshop's values set by hand, a version with
//!   provenance `user` for them, their flags cleared; anyone, its author
//!   recorded;
//! - **decide** a junction left to its fallback: the answer recorded in
//!   the decision book (who decided it with it), so a re-run of the photo
//!   takes it from there; anyone, its author recorded;
//! - **rerun** the held, stopped and decided photos: a new run of them;
//!   the user's alone.
//!
//! Each change is one library step (the photo's rows, the run's record
//! and the book), undone as the run is.

use std::sync::Arc;

use helios_core::Message;
use helios_template::run::{Book, Recorded};
use helios_vault::{Change, Changes, DocumentChange, ImageId, ImageRows, Version, edit};
use serde_json::{Value, json};

use super::{RunAsk, Runner, edit_with, seconds};

/// The flags a junction left to its fallback raises: it can be decided.
const PENDING: [&str; 3] = ["low_confidence", "fallback", "ai_budget"];

/// States of a photo given a version by the run or its review.
const WRITTEN: [&str; 3] = ["applied", "accepted", "fixed"];

fn flags_of(p: &Value) -> Vec<&Value> {
    p["flags"]
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

/// Whether the review shows the photo among the flagged ones.
fn needs_review(p: &Value) -> bool {
    !flags_of(p).is_empty() || matches!(p["state"].as_str(), Some("held" | "stopped" | "failed"))
}

/// The junctions of a photo still left to their fallback.
fn pending(p: &Value) -> Vec<Value> {
    flags_of(p)
        .into_iter()
        .filter(|f| f["flag"].as_str().is_some_and(|k| PENDING.contains(&k)))
        .filter_map(|f| {
            let name = f["name"].as_str()?;
            p["decided"][name].is_null().then(|| {
                json!({
                    "image": p["image"],
                    "junction": name,
                    "flag": f["flag"],
                    "value": p["after"][name],
                })
            })
        })
        .collect()
}

/// The photos a re-run takes: held, stopped, or with a junction decided
/// since the run.
fn to_rerun(p: &Value) -> bool {
    p["rerun"].is_null()
        && (matches!(p["state"].as_str(), Some("held" | "stopped"))
            || p["decided"].as_object().is_some_and(|d| !d.is_empty()))
}

/// The review of `record`: the counts, the photos needing review first
/// (only those raising `flag` when given), the junctions to decide and
/// the photos a re-run takes.
pub fn listing(record: &Value, flag: Option<&str>) -> Value {
    let photos: Vec<Value> = record["photos"].as_array().cloned().unwrap_or_default();
    let state = |s: &str| photos.iter().filter(|p| p["state"] == s).count();
    let mut by_flag = serde_json::Map::new();
    for p in &photos {
        for f in flags_of(p) {
            if let Some(k) = f["flag"].as_str() {
                let n = by_flag.get(k).and_then(Value::as_u64).unwrap_or(0);
                by_flag.insert(k.to_string(), json!(n + 1));
            }
        }
    }
    let written = photos
        .iter()
        .filter(|p| p["state"].as_str().is_some_and(|s| WRITTEN.contains(&s)))
        .count();
    let (mut first, rest): (Vec<Value>, Vec<Value>) =
        photos.iter().cloned().partition(needs_review);
    first.extend(rest);
    if let Some(flag) = flag {
        first.retain(|p| flags_of(p).iter().any(|f| f["flag"] == flag));
    }
    json!({
        "run": record["run"],
        "n": record["n"],
        "workshop": record["workshop"],
        "node": record["node"],
        "name": record["name"],
        "counts": {
            "photos": photos.len(),
            "flagged": photos.iter().filter(|p| needs_review(p)).count(),
            "written": written,
            "held": state("held"),
            "stopped": state("stopped"),
            "unchanged": state("unchanged"),
            "reverted": state("reverted"),
            "failed": state("failed"),
            "calls": record["calls"],
            "from_book": record["from_book"],
            "by_flag": by_flag,
        },
        "photos": first,
        "pending": photos.iter().flat_map(pending).collect::<Vec<_>>(),
        "rerun": photos.iter().filter(|p| to_rerun(p)).map(|p| p["image"].clone()).collect::<Vec<_>>(),
    })
}

/// A review step's label: "Workshop “Backlit”: a photo accepted".
fn step_label(m: Message, name: &str) -> egui_pop::api::history::Label {
    let m = m.with("name", name);
    egui_pop::api::history::Label {
        text: m.english.clone(),
        key: Some(m.key),
        args: [("name".to_string(), name.to_string())].into(),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Runner {
    /// `workshop.review {run, action, image, flag, values, junction,
    /// value}` asked by `who` (`user`, `agent:<session>`); `ask` names the
    /// workshop applied in the workspace, which accept, fix and rerun
    /// need. Answers the review after the action (a rerun: its new run).
    ///
    /// # Errors
    /// No such run or photo, an action the photo's state refuses, or an
    /// agent asking what is the user's alone.
    pub fn review(
        self: &Arc<Self>,
        args: &Value,
        who: &str,
        ask: Option<RunAsk>,
    ) -> Result<Value, String> {
        let run = args["run"].as_str().ok_or("`run`: the run's id")?;
        let record =
            Some(self.call("workshop.record.get", json!({ "run": run }))?["record"].take())
                .filter(|r| !r.is_null())
                .ok_or_else(|| format!("no run `{run}` in this library"))?;
        let action = args["action"].as_str().unwrap_or("list");
        if matches!(action, "accept" | "revert" | "rerun") && who != "user" {
            return Err(format!(
                "`{action}` is the user's alone: an agent fixes values or decides junctions, the user accepts, reverts and re-runs"
            ));
        }
        let ask = || -> Result<RunAsk, String> {
            let ask = ask.clone().ok_or(
                "the workshop is not applied in the workspace open: open it to change its photos",
            )?;
            if ask.workshop != record["workshop"] {
                return Err(format!(
                    "the workshop applied is {}, the run's {}",
                    ask.workshop, record["workshop"]
                ));
            }
            Ok(ask)
        };
        if action == "rerun" {
            let images: Vec<ImageId> = listing(&record, None)["rerun"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| ImageId::from_hex(v.as_str()?).ok())
                .collect();
            if images.is_empty() {
                return Err("no photo to re-run: none held, stopped or decided".into());
            }
            let mut ask = ask()?;
            ask.images = images;
            ask.who = who.to_string();
            if ask.on_breach.is_none() {
                ask.on_breach = serde_json::from_value(record["on_breach"].clone()).ok();
            }
            let started = self.start(ask.clone())?;
            let mut doc = record.clone();
            for p in doc["photos"].as_array_mut().into_iter().flatten() {
                if to_rerun(p) {
                    p["rerun"] = started["run"].clone();
                }
            }
            self.set_record(run, Some(&doc))?;
            return Ok(started);
        }
        if action != "list" {
            let image = args["image"].as_str().ok_or("`image`: the photo's id")?;
            let i = record["photos"]
                .as_array()
                .and_then(|a| a.iter().position(|p| p["image"] == image))
                .ok_or_else(|| format!("photo `{image}` is not in run `{run}`"))?;
            let id = ImageId::from_hex(image)?;
            let mut doc = record.clone();
            let (message, mut changes) = match action {
                "accept" => (
                    Message::new(
                        "gui-history-workshop-accept",
                        "Workshop “{ $name }”: a photo accepted",
                    ),
                    self.accept(&ask()?, &mut doc, i, id, who)?,
                ),
                "revert" => (
                    Message::new(
                        "gui-history-workshop-revert",
                        "Workshop “{ $name }”: a photo reverted",
                    ),
                    self.revert_photo(&mut doc, i, id)?,
                ),
                "fix" => (
                    Message::new(
                        "gui-history-workshop-fix",
                        "Workshop “{ $name }”: a photo fixed",
                    ),
                    self.fix(&ask()?, &mut doc, i, id, who, &args["values"])?,
                ),
                "decide" => (
                    Message::new(
                        "gui-history-workshop-decide",
                        "Workshop “{ $name }”: a junction decided",
                    ),
                    self.decide(&mut doc, i, who, args)?,
                ),
                other => {
                    return Err(format!(
                        "`action`: `{other}` is not list, accept, revert, fix, decide or rerun"
                    ));
                }
            };
            self.set_record(run, Some(&doc))?;
            changes.documents.push(DocumentChange {
                ns: helios_vault::workshops::RUNS.into(),
                key: run.into(),
                before: Some(record),
                after: Some(doc.clone()),
            });
            let name = doc["name"].as_str().unwrap_or_default();
            self.history
                .record(step_label(message, name), changes, seconds());
            return Ok(listing(&doc, args["flag"].as_str()));
        }
        Ok(listing(&record, args["flag"].as_str()))
    }

    /// What the node answers of a photo (`workshop.photo`): its head,
    /// the file read, its name and its rows.
    fn photo(&self, image: &ImageId) -> Result<Value, String> {
        self.call("workshop.photo", json!({ "image": image.hex() }))
    }

    /// The photo's graph with its edit, its head, the workshop's values
    /// on it now, and its file's name.
    #[allow(clippy::type_complexity)]
    pub(super) fn opened(
        &self,
        ask: &RunAsk,
        image: &ImageId,
    ) -> Result<
        (
            helios_core::Graph,
            Option<Version>,
            helios_core::Params,
            Option<String>,
        ),
        String,
    > {
        let photo = self.photo(image)?;
        let head: Option<Version> =
            serde_json::from_value(photo["head"].clone()).map_err(|e| format!("a version: {e}"))?;
        let file = photo["file"]
            .as_str()
            .ok_or("no file of the photo is at hand")?;
        let name = photo["name"].as_str().map(str::to_string);
        let mut graph = (self.load)(std::path::Path::new(file), &ask.bundle)?;
        if let Some(v) = &head {
            edit::apply_to_graph(&mut graph, &v.edit)?;
        }
        let id = graph
            .find(&ask.node)
            .ok_or_else(|| format!("the photo's network has no `{}`", ask.node))?;
        let current = graph
            .params(id)
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        Ok((graph, head, current, name))
    }

    /// Writes `values` (by exposed name) on the photo as a version
    /// continuing its head; the photo's change.
    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        ask: &RunAsk,
        doc: &Value,
        image: ImageId,
        values: &std::collections::BTreeMap<String, f64>,
        meta: Value,
        how: &str,
        who: &str,
    ) -> Result<(Change, Version), String> {
        let (built, ..) = Self::workshop(ask)?;
        let (graph, head, current, _) = self.opened(ask, &image)?;
        let params = helios_template::run::params_of(&built, &current, values);
        let edit = edit_with(head.as_ref(), &graph, &ask.node, &params)?;
        let label = format!(
            "workshop {} (run {}, {how})",
            doc["name"].as_str().unwrap_or_default(),
            doc["n"]
        );
        let version = Version::new(image, head.iter().map(|h| h.id).collect(), edit, who, now())
            .labelled(&label)
            .with_meta(Some(json!({ "workshop_run": meta })));
        let change = self.add_version(&version)?;
        Ok((change, version))
    }

    fn accept(
        &self,
        ask: &RunAsk,
        doc: &mut Value,
        i: usize,
        image: ImageId,
        who: &str,
    ) -> Result<Changes, String> {
        let p = &doc["photos"][i];
        if !matches!(p["state"].as_str(), Some("held" | "stopped")) {
            return Err(format!(
                "only a held or stopped photo is accepted; this one is {}",
                p["state"]
            ));
        }
        let values = numbers(&p["after"]);
        let meta = json!({
            "run": doc["run"],
            "workshop": doc["workshop"],
            "node": doc["node"],
            "provenance": p["provenance"],
            "flags": p["flags"],
            "accepted": who,
        });
        let (change, version) = self.write(ask, doc, image, &values, meta, "accepted", who)?;
        let p = &mut doc["photos"][i];
        p["state"] = json!("accepted");
        p["version"] = json!(version.id.hex());
        Ok(Changes {
            images: vec![change],
            ..Changes::default()
        })
    }

    fn revert_photo(&self, doc: &mut Value, i: usize, image: ImageId) -> Result<Changes, String> {
        let p = &doc["photos"][i];
        if !p["state"].as_str().is_some_and(|s| WRITTEN.contains(&s)) {
            return Err(format!(
                "only a written photo is reverted; this one is {}",
                p["state"]
            ));
        }
        let version = p["version"]
            .as_str()
            .map(helios_vault::VersionId::from_hex)
            .transpose()?
            .ok_or("the photo's version is not recorded")?;
        let now: ImageRows = serde_json::from_value(self.photo(&image)?["rows"].take())
            .ok()
            .flatten()
            .ok_or("the photo is gone")?;
        if now.versions.iter().any(|v| v.parents.contains(&version)) {
            return Err(
                "the photo was edited after the run: its later edit continues the run's, undo that edit first"
                    .into(),
            );
        }
        let mut target = now.clone();
        target.versions.retain(|v| v.id != version);
        let changes = Changes {
            images: vec![Change {
                image,
                before: Some(now),
                after: Some(target),
            }],
            ..Changes::default()
        };
        let changed = serde_json::to_value(&changes).map_err(|e| e.to_string())?;
        self.call(
            "library.changes.apply",
            json!({ "changes": changed, "forward": true }),
        )?;
        let p = &mut doc["photos"][i];
        p["state"] = json!("reverted");
        p["version"] = Value::Null;
        Ok(changes)
    }

    fn fix(
        &self,
        ask: &RunAsk,
        doc: &mut Value,
        i: usize,
        image: ImageId,
        who: &str,
        values: &Value,
    ) -> Result<Changes, String> {
        let p = doc["photos"][i].clone();
        if matches!(p["state"].as_str(), Some("failed" | "reverted") | None) {
            return Err(format!("a {} photo has no values to fix", p["state"]));
        }
        let fixed = numbers(values);
        if fixed.is_empty() {
            return Err("`values`: the workshop's values by slider name, {\"lift\": 0.02}".into());
        }
        let (built, ..) = Self::workshop(ask)?;
        let ranges = helios_template::workshop::ranges(&built);
        for (name, v) in &fixed {
            let r = ranges
                .get(name)
                .ok_or_else(|| format!("the workshop has no slider `{name}`"))?;
            if *v < r.min || *v > r.max {
                return Err(format!(
                    "`{name}`: {v} is outside its range {} to {}",
                    r.min, r.max
                ));
            }
        }
        let mut values = numbers(&p["after"]);
        values.extend(fixed.clone());
        let mut provenance = p["provenance"].clone();
        if !provenance.is_object() {
            provenance = json!({});
        }
        for name in fixed.keys() {
            provenance[name] = json!({ "kind": "user", "by": who });
        }
        let flags: Vec<Value> = flags_of(&p)
            .into_iter()
            .filter(|f| !f["name"].as_str().is_some_and(|n| fixed.contains_key(n)))
            .cloned()
            .collect();
        let meta = json!({
            "run": doc["run"],
            "workshop": doc["workshop"],
            "node": doc["node"],
            "provenance": provenance,
            "flags": flags,
        });
        let (change, version) = self.write(ask, doc, image, &values, meta, "fixed", who)?;
        let p = &mut doc["photos"][i];
        p["state"] = json!("fixed");
        p["version"] = json!(version.id.hex());
        p["provenance"] = provenance;
        p["flags"] = json!(flags);
        p["after"] = json!(values);
        Ok(Changes {
            images: vec![change],
            ..Changes::default()
        })
    }

    fn decide(
        &self,
        doc: &mut Value,
        i: usize,
        who: &str,
        args: &Value,
    ) -> Result<Changes, String> {
        let p = &doc["photos"][i];
        let junction = args["junction"].as_str().ok_or("`junction`: its name")?;
        if !pending(p).iter().any(|j| j["junction"] == junction) {
            return Err(format!(
                "`{junction}` is not a junction left to its fallback on this photo"
            ));
        }
        let value = args["value"]
            .as_f64()
            .ok_or("`value`: the junction's value (a zone's mask is not decided here yet)")?;
        let input = p["input"]
            .as_str()
            .ok_or("the photo's input is not recorded")?;
        let workshop = doc["workshop"].as_str().unwrap_or_default().to_string();
        let workshop = workshop.as_str();
        let before = self.book_doc(workshop)?;
        let mut book_doc = before.clone().unwrap_or_else(|| json!({}));
        let mut book: Book = serde_json::from_value(book_doc.clone()).unwrap_or_default();
        let key = Book::key_of(workshop, junction, input);
        book.answers.insert(
            key.clone(),
            Recorded {
                value: Some(value),
                confidence: 1.0,
            },
        );
        book.decided.insert(key, who.to_string());
        let runs = book_doc["runs"].clone();
        book_doc = serde_json::to_value(&book).map_err(|e| e.to_string())?;
        if !runs.is_null() {
            book_doc["runs"] = runs;
        }
        self.set_book(workshop, &book_doc)?;
        doc["photos"][i]["decided"][junction] = json!({ "value": value, "by": who });
        Ok(Changes {
            documents: vec![DocumentChange {
                ns: helios_vault::workshops::BOOKS.into(),
                key: workshop.into(),
                before,
                after: Some(book_doc),
            }],
            ..Changes::default()
        })
    }
}

/// The numbers of a JSON object, by name.
fn numbers(v: &Value) -> std::collections::BTreeMap<String, f64> {
    v.as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| v.as_f64().map(|f| (k.clone(), f)))
        .collect()
}
