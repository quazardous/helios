//! The batch endpoint (`helios-node --batch-control`), served in every
//! build: a socket only its user opens, which answers `load-source`,
//! `export`, `cancel`, `job.cancel` and the batch job's verbs, and refuses
//! the debug verbs.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde_json::Value;

const NETWORK: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}"#;

/// A node serving the batch endpoint in its own directory; killed and
/// removed when dropped.
struct Node {
    dir: PathBuf,
    child: Child,
}

impl Node {
    fn start(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "helios-batch-endpoint-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("network.json"), NETWORK).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_helios-node"))
            .arg("--graph")
            .arg(dir.join("network.json"))
            .arg("--socket")
            .arg(dir.join("node.sock"))
            .arg("--batch-control")
            .arg(dir.join("batch.sock"))
            .env_remove("HELIOS_DEBUG_CONTROL")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(line.starts_with("ready "), "{line}");
        Self { dir, child }
    }

    fn call(&self, endpoint: &str, socket: &str, args: &[&str]) -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_helios"))
            .arg(endpoint)
            .arg("--socket")
            .arg(self.dir.join(socket))
            .args(args)
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn batch(&self, args: &[&str]) -> Value {
        self.call("control", "batch.sock", args)
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn picture(path: &Path) {
    image::RgbImage::from_fn(6, 3, |x, y| {
        image::Rgb([(x * 40) as u8, (y * 80) as u8, 60])
    })
    .save(path)
    .unwrap();
}

#[test]
fn the_batch_endpoint_answers_only_what_a_batch_needs() {
    let node = Node::start("scope");
    let mode = std::fs::metadata(node.dir.join("batch.sock"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "only its user opens the socket");
    for refused in [vec!["pause"], vec!["set-param", "exp", "exposure", "1.0"]] {
        let answer = node.batch(&refused);
        assert_eq!(answer["ok"], false, "{answer}");
        assert!(
            answer["error"].as_str().unwrap().contains("only answers"),
            "{answer}"
        );
        assert_eq!(answer["message"]["key"], "node-batch-only", "{answer}");
    }
    let pic = node.dir.join("in.png");
    picture(&pic);
    // A file that is not a picture: refused by key, its English naming it.
    let junk = node.dir.join("junk.png");
    std::fs::write(&junk, b"not a picture").unwrap();
    let refused = node.batch(&["load-source", "src", junk.to_str().unwrap()]);
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["message"]["key"], "node-load-refused", "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("junk.png"),
        "{refused}"
    );
    let loaded = node.batch(&["load-source", "src", pic.to_str().unwrap()]);
    assert_eq!(loaded["ok"], true, "{loaded}");
    let out = node.dir.join("out.png");
    let asked = node.batch(&[
        "export",
        "exp",
        "--format",
        "png",
        "--to",
        out.to_str().unwrap(),
    ]);
    assert_eq!(asked["ok"], true, "{asked}");
    let job = asked["job"].as_u64().unwrap().to_string();
    let cancel = node.batch(&["cancel", &job]);
    // Cancelled in time, or already ended: never refused by the scope.
    assert!(
        cancel["ok"] == true || cancel["error"].as_str().unwrap().contains("already ended"),
        "{cancel}"
    );
    assert_eq!(
        node.call("inspect", "node.sock", &["wait-idle"])["ok"],
        true
    );
}

#[test]
fn the_batch_endpoint_takes_no_tcp_address() {
    let dir =
        std::env::temp_dir().join(format!("helios-batch-endpoint-tcp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("network.json"), NETWORK).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_helios-node"))
        .arg("--graph")
        .arg(dir.join("network.json"))
        .arg("--socket")
        .arg(dir.join("node.sock"))
        .arg("--batch-control")
        .arg("tcp://127.0.0.1:0")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("Unix socket"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `job.cancel` on the batch endpoint: a batch job cancelled by its id
/// says so on the node's bus and refuses the exports asked for after.
#[test]
fn a_batch_job_is_cancelled_by_its_id() {
    use helios_inspect::control::{ControlRequest, FileFormat};

    let node = Node::start("job-cancel");
    let control =
        |r: &ControlRequest| helios_inspect::request(&node.dir.join("batch.sock"), r).unwrap();
    let started = control(&ControlRequest::BatchStart {
        title: "Batch 2 images".into(),
        total: 2,
    });
    assert_eq!(started["ok"], true, "{started}");
    let id = started["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("batch-"), "{id}");
    let cancel = node.batch(&["job-cancel", &id]);
    assert_eq!(cancel["ok"], true, "{cancel}");
    let pic = node.dir.join("in.png");
    picture(&pic);
    let loaded = node.batch(&["load-source", "src", pic.to_str().unwrap()]);
    assert_eq!(loaded["ok"], true, "{loaded}");
    let out = node.dir.join("late.png");
    let refused = control(&ControlRequest::Export {
        node: "exp".into(),
        port: "output".into(),
        region: None,
        path: out.clone(),
        format: FileFormat::Png,
        quality: None,
        params: std::collections::BTreeMap::new(),
        batch: Some(id.clone()),
    });
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["message"]["key"], "batch-cancelled", "{refused}");
    let ended = control(&ControlRequest::BatchEnd {
        id: id.clone(),
        failed: 0,
        cancelled: false,
    });
    assert_eq!(ended["ok"], true, "{ended}");
    assert!(!out.exists());
    let events = node.call("inspect", "node.sock", &["events"]);
    let jobs: Vec<(&str, &str)> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"].as_str().unwrap().starts_with("job."))
        .map(|e| {
            (
                e["kind"].as_str().unwrap(),
                e["payload"]["id"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        jobs,
        [
            ("job.started", id.as_str()),
            ("job.cancelling", id.as_str()),
            ("job.cancelled", id.as_str())
        ]
    );
    // A job the node does not run: refused, never out of scope.
    let unknown = node.batch(&["job-cancel", "export-999"]);
    assert_eq!(unknown["ok"], false, "{unknown}");
    assert!(
        unknown["error"]
            .as_str()
            .unwrap()
            .contains("no running job"),
        "{unknown}"
    );
}
