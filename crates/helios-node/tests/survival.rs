//! Survival scenarios for a node keeping its edit in a vault: the node is
//! killed (`kill -9`, no clean shutdown) after changes made through the
//! control endpoint, or in the middle of an export, then started again on
//! the same vault. The history, its end (the head version), the ratings,
//! the labels and the metadata must be what the last answered change left.
//!
//! The runs of the workflow level kept in a vault survive the same way:
//! a node killed in the middle of a run resumes it when it starts again,
//! each file written once; a second node takes over the runs of a dead
//! one; a paused run stays paused; finished runs go after their retention
//! or when forgotten; a run of a newer format is listed read-only; items a
//! crash left without their `submit` row are entered.
//!
//! These scenarios gate a release: `cargo survival` runs them alone.

#![cfg(all(unix, debug_assertions))]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use helios_vault::fields::{Command as Field, LABELS, RATING};
use helios_vault::{ImageId, Vault};
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

/// A picture through an exposure node, and a decision answered by a human.
const NETWORK: &str = r#"{
  "sources": [{ "name": "src", "file": "photo.png" }],
  "modules": [
    { "name": "exp", "module": "exposure" },
    { "name": "bright", "module": "decide_exposure" }
  ],
  "connections": [
    { "from": "src", "port": "image", "to": "exp", "input": "input" },
    { "from": "src", "port": "image", "to": "bright", "input": "input" }
  ],
  "decisions": { "bright": "human" }
}"#;

struct Sandbox {
    dir: PathBuf,
    node: Option<Child>,
}

impl Sandbox {
    /// A directory with the network, its picture, and a vault where the
    /// picture is already rated 4 and labelled red.
    fn new(name: &str) -> (Self, ImageId) {
        let dir =
            std::env::temp_dir().join(format!("helios-survival-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("network.json"), NETWORK).unwrap();
        image::RgbImage::from_fn(24, 12, |x, y| {
            image::Rgb([(x * 10) as u8, (y * 20) as u8, 90])
        })
        .save(dir.join("photo.png"))
        .unwrap();
        let vault = Vault::new(SqliteStore::open(&dir.join("vault.db")).unwrap());
        let image = vault.import_file(&dir.join("photo.png"), 1).unwrap();
        vault
            .apply(
                &[
                    Field::set(image, RATING, json!(4)),
                    Field::add(image, LABELS, "red"),
                    Field::set(image, "title", json!("harbour")),
                ],
                "user",
                None,
                1,
            )
            .unwrap();
        (Self { dir, node: None }, image)
    }

    fn start(&mut self) {
        let mut node = Command::new(env!("CARGO_BIN_EXE_helios-node"))
            .arg("--graph")
            .arg(self.dir.join("network.json"))
            .arg("--socket")
            .arg(self.dir.join("node.sock"))
            .arg("--control")
            .arg(self.dir.join("control.sock"))
            .arg("--vault")
            .arg(self.dir.join("vault.db"))
            .env("HELIOS_DEBUG_CONTROL", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(node.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(line.starts_with("ready"), "{line}");
        self.node = Some(node);
    }

    /// `kill -9`: the node gets no chance to save anything more.
    fn kill(&mut self) {
        let mut node = self.node.take().unwrap();
        node.kill().unwrap();
        node.wait().unwrap();
    }

    fn run(&self, endpoint: &str, socket: &str, args: &[&str]) -> (i32, Value) {
        let out = Command::new(env!("CARGO_BIN_EXE_helios"))
            .arg(endpoint)
            .arg("--socket")
            .arg(self.dir.join(socket))
            .args(args)
            .output()
            .unwrap();
        (
            out.status.code().unwrap(),
            serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
        )
    }

    fn control(&self, args: &[&str]) -> Value {
        let (code, answer) = self.run("control", "control.sock", args);
        assert_eq!(code, 0, "{args:?}: {answer}");
        answer
    }

    fn saved_events(&self) -> Vec<Value> {
        let (_, events) = self.run("inspect", "node.sock", &["events"]);
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "edit.saved")
            .map(|e| e["payload"].clone())
            .collect()
    }

    fn vault(&self) -> Vault<SqliteStore> {
        Vault::new(SqliteStore::open(&self.dir.join("vault.db")).unwrap())
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if let Some(mut node) = self.node.take() {
            let _ = node.kill();
            let _ = node.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The labels of an image's versions, oldest first, and its one head.
fn history(vault: &Vault<SqliteStore>, image: ImageId) -> (Vec<String>, helios_vault::Version) {
    let heads = vault.heads(&image).unwrap();
    assert_eq!(heads.len(), 1, "one line of history");
    let labels = vault
        .history(&heads[0].id)
        .unwrap()
        .into_iter()
        .map(|v| v.label.unwrap_or_default())
        .collect();
    (labels, heads[0].clone())
}

fn exposure(edit: &Value, node: &str) -> f64 {
    edit["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == node)
        .unwrap()["params"]["exposure"]["float"]
        .as_f64()
        .unwrap()
}

/// What the user set on the image before any edit is still there.
fn assert_fields_kept(vault: &Vault<SqliteStore>, image: ImageId) {
    let fields = vault
        .fields_of(&[image], &[RATING, LABELS, "title"])
        .unwrap()
        .remove(0);
    assert_eq!(fields.get(RATING), Some(&json!(4)));
    assert_eq!(fields.get(LABELS), Some(&json!(["red"])));
    assert_eq!(fields.get("title"), Some(&json!("harbour")));
}

#[test]
fn survival_a_node_killed_keeps_every_change_it_answered() {
    let (mut s, image) = Sandbox::new("killed");
    s.start();
    let set = s.control(&["set-param", "exp", "exposure", "1.5"]);
    assert!(set["saved"].is_string(), "{set}");
    let decided = s.control(&["decision-decide", "bright", "exposure", "2"]);
    assert!(decided["saved"].is_string(), "{decided}");
    // Each save is on the bus, with its step: read, never waited for.
    let saved = s.saved_events();
    assert_eq!(
        saved.iter().map(|e| e["step"].clone()).collect::<Vec<_>>(),
        [json!(1), json!(2)]
    );
    assert_eq!(saved[1]["version"], decided["saved"]);
    s.kill();

    let vault = s.vault();
    let (labels, head) = history(&vault, image);
    assert_eq!(labels, ["set exp.exposure", "decision bright decided"]);
    assert_eq!(head.id.hex(), decided["saved"].as_str().unwrap());
    // Each version names who made it: the control client set the
    // parameter, the user (as the request says) decided.
    let origins: Vec<String> = vault
        .history(&head.id)
        .unwrap()
        .into_iter()
        .map(|v| v.origin)
        .collect();
    assert_eq!(origins, ["control", "user"]);
    assert_eq!(saved[1]["origin"], "user");
    assert!((exposure(&head.edit, "exp") - 1.5).abs() < 1e-9);
    assert_eq!(
        head.edit["decisions"]["nodes"]["bright"]["answers"][0]["values"]["exposure"]["float"], 2.0,
        "{}",
        head.edit
    );
    assert_fields_kept(&vault, image);
    drop(vault);

    // Started again on the same vault: the node is where the kill left it.
    s.start();
    let again = s.control(&["set-param", "exp", "exposure", "1.5"]);
    assert!(
        again["saved"].is_null(),
        "nothing changed, nothing saved: {again}"
    );
    let list = s.control(&["decisions"]);
    assert_eq!(list["decisions"]["bright"]["status"], "answered", "{list}");
    // The next change continues the history.
    let next = s.control(&["set-param", "exp", "exposure", "0.5"]);
    let (labels, head) = history(&s.vault(), image);
    assert_eq!(labels.len(), 3);
    assert_eq!(head.id.hex(), next["saved"].as_str().unwrap());
}

#[test]
fn survival_a_node_killed_during_an_export_keeps_its_edit_and_leaves_no_file() {
    let (mut s, image) = Sandbox::new("export");
    s.start();
    let set = s.control(&["set-param", "exp", "exposure", "0.5"]);
    // The export is asked for and held in the queue: the kill lands while
    // it is under way.
    s.control(&["pause"]);
    let out = s.dir.join("out.png");
    let export = s.control(&[
        "export",
        "exp",
        "--format",
        "png",
        "--to",
        out.to_str().unwrap(),
    ]);
    assert!(export["job"].is_u64(), "{export}");
    s.kill();

    assert!(!out.exists(), "no half-written export");
    let leftovers: Vec<_> = std::fs::read_dir(&s.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("out"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    let vault = s.vault();
    let (labels, head) = history(&vault, image);
    assert_eq!(labels, ["set exp.exposure"]);
    assert_eq!(head.id.hex(), set["saved"].as_str().unwrap());
    assert!((exposure(&head.edit, "exp") - 0.5).abs() < 1e-9);
    assert_fields_kept(&vault, image);
    drop(vault);

    // Restarted, it exports the saved edit.
    s.start();
    s.control(&[
        "export",
        "exp",
        "--format",
        "png",
        "--to",
        out.to_str().unwrap(),
    ]);
    let (code, _) = s.run("inspect", "node.sock", &["wait-idle"]);
    assert_eq!(code, 0);
    assert!(out.exists());
}

/// A synthetic network a run exports each item through.
const RUN_NETWORK: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}"#;

/// How long a run of a few tiny pictures may take, a take-over included
/// (a node looks for runs to take over every 5 s).
const RUN_DEADLINE: Duration = Duration::from_mins(1);

/// Nodes working runs over one vault, each named (`a`, `b`) with sockets
/// of its own; killed and removed when dropped.
struct Runs {
    dir: PathBuf,
    nodes: BTreeMap<String, Child>,
}

impl Runs {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("helios-runs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("network.json"), RUN_NETWORK).unwrap();
        Self {
            dir,
            nodes: BTreeMap::new(),
        }
    }

    fn start(&mut self, who: &str) {
        let mut node = Command::new(env!("CARGO_BIN_EXE_helios-node"))
            .arg("--graph")
            .arg(self.dir.join("network.json"))
            .arg("--socket")
            .arg(self.dir.join(format!("{who}-node.sock")))
            .arg("--control")
            .arg(self.dir.join(format!("{who}-control.sock")))
            .arg("--vault")
            .arg(self.dir.join("vault.db"))
            .env("HELIOS_DEBUG_CONTROL", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(node.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(line.starts_with("ready"), "{line}");
        self.nodes.insert(who.to_string(), node);
    }

    /// `kill -9`.
    fn kill(&mut self, who: &str) {
        let mut node = self.nodes.remove(who).unwrap();
        node.kill().unwrap();
        node.wait().unwrap();
    }

    /// A control request to node `who`, answered whatever it says.
    fn ask(&self, who: &str, args: &[&str]) -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_helios"))
            .arg("control")
            .arg("--socket")
            .arg(self.dir.join(format!("{who}-control.sock")))
            .args(args)
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    /// A control request that must succeed.
    fn control(&self, who: &str, args: &[&str]) -> Value {
        let answer = self.ask(who, args);
        assert_eq!(answer["ok"], true, "{args:?}: {answer}");
        answer
    }

    fn picture(&self, name: &str) -> PathBuf {
        let path = self.dir.join("in").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbImage::from_fn(6, 3, |x, y| {
            image::Rgb([(x * 40) as u8, (y * 80) as u8, 60])
        })
        .save(&path)
        .unwrap();
        path
    }

    /// Submits `images` to node `who`, written to `out`: the run's id.
    fn submit(&self, who: &str, out: &Path, images: &[&Path]) -> String {
        let mut args = vec!["workflow-submit", "--format", "png", "--to"];
        args.push(out.to_str().unwrap());
        args.extend(images.iter().map(|p| p.to_str().unwrap()));
        self.control(who, &args)["run"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The runs node `who` lists.
    fn runs(&self, who: &str) -> Vec<Value> {
        self.control(who, &["workflow-runs"])["runs"]
            .as_array()
            .unwrap()
            .clone()
    }

    /// Run `run` as node `who` lists it (null when it does not).
    fn run(&self, who: &str, run: &str) -> Value {
        self.runs(who)
            .into_iter()
            .find(|r| r["id"] == run)
            .unwrap_or(Value::Null)
    }

    /// Run `run` once `ready` holds for it; fails past the deadline with
    /// what it saw, saying what it waited for (`what`).
    fn until(&self, who: &str, run: &str, what: &str, ready: impl Fn(&Value) -> bool) -> Value {
        let start = Instant::now();
        loop {
            let found = self.run(who, run);
            if ready(&found) {
                return found;
            }
            assert!(
                start.elapsed() < RUN_DEADLINE,
                "run {run} not {what} on {who} in {RUN_DEADLINE:?}: {found}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// The items of a run at its export, by item id.
    fn items(&self, who: &str, run: &str) -> BTreeMap<String, Value> {
        self.control(who, &["workflow-items", run])["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| (i["item"].as_str().unwrap().to_string(), i.clone()))
            .collect()
    }

    /// The vault, opened while no node runs.
    fn vault(&self) -> Arc<Vault<SqliteStore>> {
        Arc::new(Vault::new(
            SqliteStore::open(&self.dir.join("vault.db")).unwrap(),
        ))
    }

    /// Changes a run's document in the vault, while no node runs.
    fn edit_run(&self, run: &str, change: impl FnOnce(&mut Value)) {
        let vault = self.vault();
        let mut doc = vault.workflow_document("runs", run).unwrap().unwrap();
        change(&mut doc);
        vault
            .set_workflow_documents(&[("runs".into(), run.into(), Some(doc))])
            .unwrap();
    }
}

impl Drop for Runs {
    fn drop(&mut self) {
        for node in self.nodes.values_mut() {
            let _ = node.kill();
            let _ = node.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The names of the files in `dir`, sorted.
fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn running(run: &Value) -> u64 {
    run["nodes"]["exp"]["running"].as_u64().unwrap_or(0)
}

#[test]
fn survival_a_run_killed_mid_batch_resumes_and_writes_each_file_once() {
    let mut s = Runs::new("resume");
    s.start("a");
    let pics: Vec<PathBuf> = ["a.png", "b.png", "c.png", "d.png"]
        .iter()
        .map(|n| s.picture(n))
        .collect();
    let out = s.dir.join("out");
    let run = s.submit("a", &out, &[&pics[0], &pics[1]]);
    s.until("a", &run, "finished", |r| r["state"] == "finished");
    let before = s.items("a", &run);
    // Two more, claimed but held in the scheduler: the kill lands while
    // they are under way.
    s.control("a", &["pause"]);
    assert_eq!(s.submit("a", &out, &[&pics[2], &pics[3]]), run);
    s.until("a", &run, "exporting", |r| running(r) >= 1);
    s.kill("a");
    assert_eq!(files(&out), ["a.png", "b.png"], "nothing more written");

    s.start("a");
    let done = s.until("a", &run, "finished again", |r| r["state"] == "finished");
    assert_eq!(done["nodes"]["exp"]["done"], 4, "{done}");
    assert_eq!(done["nodes"]["exp"]["failed"], 0, "{done}");
    let after = s.items("a", &run);
    for (id, item) in &after {
        assert_eq!(item["state"], "done", "{item}");
        // A row given back at the restart counts no attempt.
        assert_eq!(item["row"]["attempts"], 1, "exported once: {item}");
        if let Some(old) = before.get(id) {
            assert_eq!(item["row"]["since"], old["row"]["since"], "not redone");
        }
    }
    assert_eq!(files(&out), ["a.png", "b.png", "c.png", "d.png"]);
}

#[test]
fn survival_a_second_node_takes_over_the_runs_of_a_dead_one() {
    let mut s = Runs::new("takeover");
    s.start("a");
    let pics = [s.picture("a.png"), s.picture("b.png")];
    let out = s.dir.join("out");
    s.control("a", &["pause"]);
    let run = s.submit("a", &out, &[&pics[0], &pics[1]]);
    s.until("a", &run, "exporting", |r| running(r) >= 1);

    // While `a` lives, `b` lists its run and leaves it alone.
    s.start("b");
    let seen = s.run("b", &run);
    assert_eq!(seen["read_only"], true, "{seen}");
    assert_eq!(seen["reason"], "elsewhere", "{seen}");
    let refused = s.ask("b", &["workflow-items", &run]);
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(
        refused["message"]["key"], "workflow-run-elsewhere",
        "{refused}"
    );

    // `a` dies: `b` takes the run over and finishes it.
    s.kill("a");
    let done = s.until("b", &run, "taken over and finished", |r| {
        r["read_only"] == false && r["state"] == "finished"
    });
    assert_eq!(done["nodes"]["exp"]["done"], 2, "{done}");
    for item in s.items("b", &run).values() {
        assert_eq!(item["row"]["attempts"], 1, "{item}");
    }
    assert_eq!(files(&out), ["a.png", "b.png"]);
}

#[test]
fn survival_a_paused_run_stays_paused_after_a_restart() {
    let mut s = Runs::new("paused");
    s.start("a");
    let pics = [s.picture("a.png"), s.picture("b.png")];
    let out = s.dir.join("out");
    s.control("a", &["pause"]);
    let run = s.submit("a", &out, &[&pics[0], &pics[1]]);
    s.until("a", &run, "exporting", |r| running(r) >= 1);
    s.kill("a");
    s.edit_run(&run, |doc| doc["state"] = json!("paused"));

    s.start("a");
    let paused = s.run("a", &run);
    assert_eq!(paused["state"], "paused", "{paused}");
    // Given time to claim, it claims nothing.
    std::thread::sleep(Duration::from_secs(1));
    let still = s.run("a", &run);
    assert_eq!(still["state"], "paused", "{still}");
    assert_eq!(running(&still), 0, "{still}");
    assert_eq!(still["nodes"]["exp"]["done"], 0, "{still}");
    assert!(files(&out).is_empty());
}

#[test]
fn survival_finished_runs_go_after_their_retention_or_when_forgotten() {
    let mut s = Runs::new("retention");
    s.start("a");
    let pic = s.picture("a.png");
    let (one, two) = (s.dir.join("one"), s.dir.join("two"));
    let old = s.submit("a", &one, &[&pic]);
    let recent = s.submit("a", &two, &[&pic]);
    s.until("a", &old, "finished", |r| r["state"] == "finished");
    s.until("a", &recent, "finished", |r| r["state"] == "finished");
    s.kill("a");
    // Finished long ago: past the 30 days kept.
    s.edit_run(&old, |doc| doc["finished"] = json!(1));

    s.start("a");
    assert!(s.run("a", &old).is_null(), "removed at the start");
    assert_eq!(s.run("a", &recent)["state"], "finished");
    let forgotten = s.control("a", &["workflow-forget", &recent]);
    assert_eq!(forgotten["forgotten"], true, "{forgotten}");
    assert!(s.runs("a").is_empty());
    let again = s.ask("a", &["workflow-forget", &recent]);
    assert_eq!(again["message"]["key"], "workflow-no-run", "{again}");
    s.kill("a");

    // Nothing is kept of either: run, owner, items, rows.
    let vault = s.vault();
    for run in [&old, &recent] {
        assert_eq!(vault.workflow_document("runs", run).unwrap(), None);
        assert_eq!(vault.workflow_document("owners", run).unwrap(), None);
        assert!(
            vault
                .workflow_documents(&format!("{run}/items"))
                .unwrap()
                .is_empty()
        );
        let rows = helios_vault::journal::VaultJournal::new(vault.clone(), run);
        assert!(helios_journal::Store::rows(&rows).is_empty());
    }
    // The files written stay.
    assert_eq!(files(&one), ["a.png"]);
    assert_eq!(files(&two), ["a.png"]);
}

#[test]
fn survival_a_run_of_a_newer_format_is_listed_read_only() {
    let mut s = Runs::new("format");
    s.start("a");
    let pic = s.picture("a.png");
    let out = s.dir.join("out");
    let run = s.submit("a", &out, &[&pic]);
    s.until("a", &run, "finished", |r| r["state"] == "finished");
    s.kill("a");
    s.edit_run(&run, |doc| doc["format"] = json!(99));

    s.start("a");
    let listed = s.run("a", &run);
    assert_eq!(listed["read_only"], true, "{listed}");
    assert_eq!(listed["reason"], "newer-format", "{listed}");
    assert_eq!(listed["format"], 99, "{listed}");
    let items = s.ask("a", &["workflow-items", &run]);
    assert_eq!(items["message"]["key"], "workflow-newer-format", "{items}");
    let out_arg = out.to_str().unwrap();
    let submitted = s.ask(
        "a",
        &[
            "workflow-submit",
            "--format",
            "png",
            "--to",
            out_arg,
            pic.to_str().unwrap(),
        ],
    );
    assert_eq!(
        submitted["message"]["key"], "workflow-newer-format",
        "{submitted}"
    );
    s.kill("a");
    // Left as it was.
    assert_eq!(
        s.vault().workflow_document("runs", &run).unwrap().unwrap()["format"],
        99
    );
}

#[test]
fn survival_items_left_without_their_submit_row_are_entered_at_the_start() {
    let mut s = Runs::new("entered");
    s.start("a");
    let first = s.picture("a.png");
    let late = s.picture("late.png");
    let out = s.dir.join("out");
    let run = s.submit("a", &out, &[&first]);
    s.until("a", &run, "finished", |r| r["state"] == "finished");
    s.kill("a");
    // What a crash between a submission's two transactions leaves: the
    // item's document, without its `submit` row.
    let id = helios_node::workflow::item_id(&late);
    s.vault()
        .set_workflow_documents(&[(
            format!("{run}/items"),
            id.clone(),
            Some(json!({
                "id": id,
                "path": late,
                "output": out.join("late.png"),
                "order": 1,
            })),
        )])
        .unwrap();

    s.start("a");
    let done = s.until("a", &run, "finished with the late item", |r| {
        r["state"] == "finished" && r["nodes"]["exp"]["done"] == 2
    });
    assert_eq!(done["nodes"]["exp"]["failed"], 0, "{done}");
    assert_eq!(files(&out), ["a.png", "late.png"]);
}
