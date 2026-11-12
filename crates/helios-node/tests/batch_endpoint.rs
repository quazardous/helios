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

/// Where Unix sockets exist, the batch endpoint refuses TCP for that reason,
/// whether a token file is given or not (a missing token file is not the
/// reason, and a token file does not open TCP).
#[test]
fn the_batch_endpoint_takes_no_tcp_address() {
    let dir =
        std::env::temp_dir().join(format!("helios-batch-endpoint-tcp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("network.json"), NETWORK).unwrap();
    for token_file in [None, Some(dir.join("token"))] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_helios-node"));
        command
            .arg("--graph")
            .arg(dir.join("network.json"))
            .arg("--socket")
            .arg(dir.join("node.sock"))
            .arg("--batch-control")
            .arg("tcp://127.0.0.1:0")
            .stdin(Stdio::null());
        if let Some(file) = &token_file {
            command.arg("--token-file").arg(file);
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // A node that serves instead of refusing must not hang the test.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let out = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "token file {token_file:?}: {stderr}");
        assert!(
            stderr.contains("Unix socket"),
            "token file {token_file:?}: {stderr}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
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
        hold: false,
        idle_timeout_s: None,
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
        as_mask: false,
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

/// A node's jobs, followed from its inspect endpoint: pushed as they
/// happen (`subscribe`), and read whole (`describe jobs`) to catch up.
#[test]
fn a_node_s_jobs_are_pushed_and_described() {
    use helios_inspect::control::ControlRequest;
    use helios_inspect::{Address, Request};

    let node = Node::start("subscribe");
    let inspect = Address::Unix(node.dir.join("node.sock"));
    let (ack, mut stream) = helios_inspect::subscribe(&inspect, "job.", None).unwrap();
    assert_eq!(ack["ok"], true, "{ack}");
    let control =
        |r: &ControlRequest| helios_inspect::request(&node.dir.join("batch.sock"), r).unwrap();
    let started = control(&ControlRequest::BatchStart {
        title: "Batch 3 images".into(),
        total: 3,
        hold: false,
        idle_timeout_s: None,
    });
    let id = started["id"].as_str().unwrap().to_string();
    let first = next_job(&mut stream);
    assert_eq!(first["kind"], "job.started", "{first}");
    assert_eq!(first["payload"]["id"], id.as_str());
    assert_eq!(first["payload"]["total"], 3);

    let jobs = helios_inspect::request_at(
        &inspect,
        &Request::Describe {
            entity: "jobs".into(),
        },
    )
    .unwrap();
    let jobs = &jobs["describe"];
    assert_eq!(jobs["instance"], ack["instance"], "{jobs}");
    assert_eq!(jobs["running"][0]["started"]["id"], id.as_str(), "{jobs}");

    control(&ControlRequest::BatchEnd {
        id: id.clone(),
        failed: 0,
        cancelled: false,
    });
    let ended = next_job(&mut stream);
    assert_eq!(ended["kind"], "job.finished", "{ended}");
    assert_eq!(ended["payload"]["id"], id.as_str());
}

/// The next `job.*` event of a subscription, heartbeats skipped.
fn next_job(stream: &mut helios_inspect::Client) -> Value {
    loop {
        let line = stream.read().unwrap();
        if line.get("heartbeat").is_none() {
            return line["event"].clone();
        }
    }
}

/// A batch whose client dies without ending it does not stay running:
/// it ends when the connection it was held on closes, or, asked on no
/// held connection, once nothing was asked for it for its idle timeout.
#[test]
fn an_orphaned_batch_ends() {
    use helios_inspect::control::ControlRequest;
    use helios_inspect::{Address, Client};

    let node = Node::start("orphan");
    let inspect = Address::Unix(node.dir.join("node.sock"));
    let batch = Address::Unix(node.dir.join("batch.sock"));
    let (_, mut stream) = helios_inspect::subscribe(&inspect, "job.", None).unwrap();

    // Held: the client's connection drops, the batch ends.
    let mut client = Client::connect(&batch).unwrap();
    let held = client
        .request(&ControlRequest::BatchStart {
            title: "Batch 5 images".into(),
            total: 5,
            hold: true,
            idle_timeout_s: None,
        })
        .unwrap();
    let id = held["id"].as_str().unwrap().to_string();
    assert_eq!(next_job(&mut stream)["kind"], "job.started");
    drop(client);
    let ended = next_job(&mut stream);
    assert_eq!(ended["kind"], "job.failed", "{ended}");
    assert_eq!(ended["payload"]["id"], id.as_str());
    assert!(
        ended["payload"]["error"]
            .as_str()
            .unwrap()
            .contains("client went away"),
        "{ended}"
    );

    // Not held: it ends after its idle timeout.
    let asked = helios_inspect::request_at(
        &batch,
        &ControlRequest::BatchStart {
            title: "Batch 2 images".into(),
            total: 2,
            hold: false,
            idle_timeout_s: Some(1),
        },
    )
    .unwrap();
    let id = asked["id"].as_str().unwrap().to_string();
    assert_eq!(next_job(&mut stream)["kind"], "job.started");
    let ended = next_job(&mut stream);
    assert_eq!(ended["kind"], "job.failed", "{ended}");
    assert_eq!(ended["payload"]["id"], id.as_str());
    assert!(
        ended["payload"]["error"]
            .as_str()
            .unwrap()
            .contains("nothing asked for it in 1 s"),
        "{ended}"
    );
    // Ended: its client's late end is refused, never a second end.
    let late = helios_inspect::request_at(
        &batch,
        &ControlRequest::BatchEnd {
            id,
            failed: 0,
            cancelled: false,
        },
    )
    .unwrap();
    assert_eq!(late["ok"], false, "{late}");
}

/// One JSON-RPC call on `client`, with a deadline on its answer.
fn rpc(client: &mut helios_inspect::Client, id: i64, method: &str, params: Value) -> Value {
    let answer = client
        .request(
            &serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )
        .unwrap();
    assert_eq!(answer["id"], id, "{answer}");
    answer
}

#[test]
fn the_batch_endpoint_speaks_json_rpc() {
    use helios_inspect::rpc::code;
    use serde_json::json;

    let node = Node::start("jsonrpc");
    let connect = |socket: &str| {
        let client =
            helios_inspect::Client::connect(&helios_inspect::Address::Unix(node.dir.join(socket)))
                .unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        client
    };
    let mut batch = connect("batch.sock");

    // `node.hello` names the same instance as the inspect endpoint.
    let hello = rpc(&mut batch, 1, "node.hello", json!({}));
    let hello = &hello["result"];
    assert_eq!(hello["endpoint"], "batch");
    assert_eq!(hello["permissions"], json!(["batch"]));
    assert_eq!(hello["protocol"], helios_inspect::rpc::PROTOCOL);
    let inspect = rpc(&mut connect("node.sock"), 1, "node.hello", json!({}));
    assert_eq!(hello["instance"], inspect["result"]["instance"]);

    // The command list is the capability list.
    let list = rpc(
        &mut batch,
        2,
        "commands.list",
        json!({ "enabled_only": true }),
    );
    let ids: Vec<&str> = list["result"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["id"].as_str())
        .collect();
    assert!(ids.contains(&"workflow.runs"), "{ids:?}");
    assert!(!ids.contains(&"pause"), "{ids:?}");

    // A command by its id, under the origin `_meta` names.
    let runs = rpc(
        &mut batch,
        3,
        "workflow.runs",
        json!({ "_meta": { "origin": "script:test" } }),
    );
    assert_eq!(runs["result"]["origin"], "script:test", "{runs}");
    assert_eq!(runs["result"]["command"], "workflow.runs");

    // Refusals carry their code.
    let debug_only = rpc(&mut batch, 4, "pause", json!({}));
    assert_eq!(debug_only["error"]["code"], code::DISABLED, "{debug_only}");
    let unknown = rpc(&mut batch, 5, "no.such.command", json!({}));
    assert_eq!(
        unknown["error"]["code"],
        code::METHOD_NOT_FOUND,
        "{unknown}"
    );
    let params = rpc(&mut batch, 6, "workflow.items", json!({ "run": 3 }));
    assert_eq!(params["error"]["code"], code::INVALID_PARAMS, "{params}");
    let vault = rpc(
        &mut batch,
        7,
        "settings.layer.get",
        json!({ "layer": "user" }),
    );
    assert_eq!(vault["error"]["code"], code::NEEDS_VAULT, "{vault}");
    let origin = rpc(
        &mut batch,
        8,
        "workflow.runs",
        json!({ "_meta": { "origin": "nobody" } }),
    );
    assert_eq!(origin["error"]["code"], code::ORIGIN_REFUSED, "{origin}");

    // Nothing runs under that id any more.
    let cancel = rpc(&mut batch, 9, "$/cancel", json!({ "id": 3 }));
    assert_eq!(cancel["result"]["cancelled"], false, "{cancel}");

    // The older form, on the same connection.
    let old = batch.request(&json!({ "cmd": "workflow.runs" })).unwrap();
    assert_eq!(old["ok"], true, "{old}");
}
