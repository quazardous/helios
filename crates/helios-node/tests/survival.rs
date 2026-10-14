//! Survival scenarios for a node keeping its edit in a vault: the node is
//! killed (`kill -9`, no clean shutdown) after changes made through the
//! control endpoint, or in the middle of an export, then started again on
//! the same vault. The history, its end (the head version), the ratings,
//! the labels and the metadata must be what the last answered change left.
//!
//! These scenarios gate a release: `cargo survival` runs them alone.

#![cfg(all(unix, debug_assertions))]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

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
