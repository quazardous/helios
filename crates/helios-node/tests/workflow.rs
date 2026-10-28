//! Batches as runs of the workflow level (`workflow.submit`), through the
//! batch endpoint every build serves: each item exported once by the node,
//! re-submitting redoing nothing, output names fixed at submission, a
//! transient failure retried, a permanent one not, the workflow derived
//! from a network with a decision, whose items wait while it is unanswered,
//! and a finished run forgotten.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const NETWORK: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}"#;

/// A decision before the export, through a process node.
const DECIDED: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [
    { "name": "bright", "module": "decide_exposure" },
    { "name": "exp", "module": "exposure" }
  ],
  "connections": [
    { "from": "src", "port": "image", "to": "bright", "input": "input" },
    { "from": "bright", "port": "output", "to": "exp", "input": "input" }
  ],
  "decisions": { "bright": "full_ai" }
}"#;

/// How long a run of a few tiny pictures may take, retries included.
const DEADLINE: Duration = Duration::from_mins(1);

/// A node serving the batch endpoint (and the debug one when asked) in
/// its own directory; killed and removed when dropped.
struct Node {
    dir: PathBuf,
    child: Child,
}

impl Node {
    fn start(name: &str, network: &str, debug: bool) -> Self {
        let dir =
            std::env::temp_dir().join(format!("helios-workflow-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("network.json"), network).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_helios-node"));
        command
            .arg("--graph")
            .arg(dir.join("network.json"))
            .arg("--socket")
            .arg(dir.join("node.sock"))
            .arg("--batch-control")
            .arg(dir.join("batch.sock"))
            .stdout(Stdio::piped());
        if debug {
            command
                .arg("--control")
                .arg(dir.join("control.sock"))
                .env("HELIOS_DEBUG_CONTROL", "1");
        } else {
            command.env_remove("HELIOS_DEBUG_CONTROL");
        }
        let mut child = command.spawn().unwrap();
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

    fn picture(&self, name: &str) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbImage::from_fn(6, 3, |x, y| {
            image::Rgb([(x * 40) as u8, (y * 80) as u8, 60])
        })
        .save(&path)
        .unwrap();
        path
    }

    fn submit(&self, out: &Path, images: &[&Path], extra: &[&str]) -> Value {
        let mut args = vec!["workflow-submit", "--format", "png", "--to"];
        args.push(out.to_str().unwrap());
        args.extend(extra);
        args.extend(images.iter().map(|p| p.to_str().unwrap()));
        let answer = self.batch(&args);
        assert_eq!(answer["ok"], true, "{answer}");
        answer
    }

    /// The run, once finished; fails past the deadline with what it saw.
    fn finished(&self, run: &str) -> Value {
        self.until(run, "finished", |r| r["state"] == "finished")
    }

    /// The run, once `ready` holds for it; fails past the deadline with
    /// what it saw, saying what it waited for (`what`).
    fn until(&self, run: &str, what: &str, ready: impl Fn(&Value) -> bool) -> Value {
        let start = Instant::now();
        loop {
            let runs = self.batch(&["workflow-runs"]);
            let found = runs["runs"]
                .as_array()
                .and_then(|r| r.iter().find(|r| r["id"] == run))
                .cloned()
                .unwrap_or(Value::Null);
            if ready(&found) {
                return found;
            }
            assert!(
                start.elapsed() < DEADLINE,
                "run {run} not {what} in {DEADLINE:?}: {runs}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// The items of a run at its export.
    fn items(&self, run: &str) -> Vec<Value> {
        let page = self.batch(&["workflow-items", run]);
        assert_eq!(page["ok"], true, "{page}");
        page["items"].as_array().unwrap().clone()
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_run_exports_each_item_once_and_a_second_submit_redoes_nothing() {
    let node = Node::start("once", NETWORK, false);
    let images: Vec<PathBuf> = ["a.png", "b.png", "c.png"]
        .iter()
        .map(|n| node.picture(n))
        .collect();
    let refs: Vec<&Path> = images.iter().map(PathBuf::as_path).collect();
    let out = node.dir.join("out");
    let first = node.submit(&out, &refs, &[]);
    assert_eq!(first["new"], 3, "{first}");
    assert_eq!(first["durable"], false, "no vault, the run is in memory");
    let run = first["run"].as_str().unwrap().to_string();
    let done = node.finished(&run);
    assert_eq!(done["nodes"]["exp"]["done"], 3, "{done}");
    assert_eq!(done["nodes"]["exp"]["failed"], 0, "{done}");
    for item in node.items(&run) {
        assert_eq!(item["state"], "done", "{item}");
        assert_eq!(item["row"]["attempts"], 1, "exported once: {item}");
        assert!(
            Path::new(item["output"].as_str().unwrap()).exists(),
            "{item}"
        );
    }

    // The same network and settings: the same run, nothing redone.
    let again = node.submit(&out, &refs, &[]);
    assert_eq!(again["run"], run.as_str(), "{again}");
    assert_eq!(again["new"], 0, "{again}");
    assert_eq!(again["already"], 3, "{again}");
    // An image given twice is one item.
    let twice = node.submit(&out, &[refs[0], refs[0]], &[]);
    assert_eq!(twice["already"], 1, "{twice}");
    assert_eq!(twice["merged"], 1, "{twice}");
    let still = node.finished(&run);
    assert_eq!(still["nodes"]["exp"]["done"], 3, "{still}");
    for item in node.items(&run) {
        assert_eq!(item["row"]["attempts"], 1, "not redone: {item}");
    }
    // `--again` is a new run.
    let fresh = node.submit(&out, &refs[..1], &["--again"]);
    assert_ne!(fresh["run"], run.as_str(), "{fresh}");
    assert_eq!(fresh["new"], 1, "{fresh}");
    let state = node.call("inspect", "node.sock", &["describe", "workflow"]);
    assert_eq!(
        state["describe"]["runs"].as_array().map(Vec::len),
        Some(2),
        "{state}"
    );
}

#[test]
fn a_finished_run_is_forgotten_and_one_at_work_is_not() {
    let node = Node::start("forget", NETWORK, false);
    let pic = node.picture("a.png");
    let out = node.dir.join("out");
    let run = node.submit(&out, &[&pic], &[])["run"]
        .as_str()
        .unwrap()
        .to_string();
    node.finished(&run);
    let forgotten = node.batch(&["workflow-forget", &run]);
    assert_eq!(forgotten["ok"], true, "{forgotten}");
    let runs = node.batch(&["workflow-runs"]);
    assert_eq!(runs["runs"].as_array().map(Vec::len), Some(0), "{runs}");
    let again = node.batch(&["workflow-forget", &run]);
    assert_eq!(again["message"]["key"], "workflow-no-run", "{again}");
    // The file written stays.
    assert!(out.join("a.png").exists());

    // A run waiting for a decision is at work: not forgotten.
    let waiting = Node::start("forget-busy", DECIDED, false);
    let pic = waiting.picture("p.png");
    let run = waiting.submit(&waiting.dir.join("out"), &[&pic], &[])["run"]
        .as_str()
        .unwrap()
        .to_string();
    waiting.until(&run, "asking at the decision", |r| {
        r["nodes"]["bright"]["asking"] == 1
    });
    let refused = waiting.batch(&["workflow-forget", &run]);
    assert_eq!(
        refused["message"]["key"], "workflow-run-at-work",
        "{refused}"
    );
    assert_eq!(
        waiting.until(&run, "listed", |r| !r.is_null())["state"],
        "active"
    );
}

#[test]
fn output_names_are_fixed_at_submission() {
    let node = Node::start("names", NETWORK, false);
    let one = node.picture("one/x.png");
    let two = node.picture("two/x.png");
    let y = node.picture("y.png");
    let out = node.dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    // A file already there: the name is numbered once, at submission.
    std::fs::write(out.join("y.png"), b"taken").unwrap();
    let first = node.submit(&out, &[&one, &two, &y], &[]);
    let outputs: Vec<&str> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["output"].as_str().unwrap())
        .collect();
    let expected: Vec<PathBuf> = ["x.png", "x-1.png", "y-1.png"]
        .iter()
        .map(|n| out.join(n))
        .collect();
    let expected: Vec<&str> = expected.iter().map(|p| p.to_str().unwrap()).collect();
    assert_eq!(outputs, expected, "{first}");
    let run = first["run"].as_str().unwrap().to_string();
    node.finished(&run);
    // Submitted again with every file now on disk: the same names.
    let again = node.submit(&out, &[&one, &two, &y], &[]);
    let names: Vec<&str> = again["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["output"].as_str().unwrap())
        .collect();
    assert_eq!(names, expected, "{again}");
    assert_eq!(std::fs::read(out.join("y.png")).unwrap(), b"taken");
}

#[test]
fn a_file_that_is_not_a_picture_fails_once_for_good() {
    let node = Node::start("junk", NETWORK, false);
    let good = node.picture("good.png");
    let junk = node.dir.join("junk.png");
    std::fs::write(&junk, b"not a picture").unwrap();
    let out = node.dir.join("out");
    let submitted = node.submit(&out, &[&good, &junk], &[]);
    let run = submitted["run"].as_str().unwrap().to_string();
    let done = node.finished(&run);
    assert_eq!(done["nodes"]["exp"]["done"], 1, "{done}");
    assert_eq!(done["nodes"]["exp"]["failed"], 1, "{done}");
    let page = node.batch(&["workflow-items", &run]);
    let failed: Vec<&Value> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["state"] == "failed")
        .collect();
    assert_eq!(failed.len(), 1, "{page}");
    assert_eq!(failed[0]["row"]["attempts"], 1, "not retried: {page}");
    assert!(
        failed[0]["error"].as_str().unwrap().contains("junk.png"),
        "{page}"
    );
}

#[test]
fn the_workflow_of_a_network_with_a_decision_is_derived() {
    let node = Node::start("derived", DECIDED, false);
    let pic = node.picture("p.png");
    let out = node.dir.join("out");
    let submitted = node.submit(&out, &[&pic], &[]);
    let workflow = submitted["workflow"].as_array().unwrap();
    let names: Vec<&str> = workflow
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["bright", "exp"], "{submitted}");
    assert_eq!(workflow[0]["parents"], serde_json::json!(["submit"]));
    assert_eq!(workflow[0]["kind"], "decision");
    assert_eq!(workflow[0]["mode"], "full_ai");
    assert_eq!(workflow[1]["parents"], serde_json::json!(["bright"]));
    assert_eq!(workflow[1]["kind"], "export");
    // The network has no answer for `bright`: no export could render, so
    // the item waits for the answer and the export holds it, failing
    // nothing.
    let run = submitted["run"].as_str().unwrap().to_string();
    let asking = node.until(&run, "asking at the decision", |r| {
        r["nodes"]["bright"]["asking"] == 1
    });
    assert_eq!(asking["state"], "active", "{asking}");
    assert_eq!(asking["nodes"]["exp"]["upstream"], 1, "{asking}");
    assert_eq!(asking["nodes"]["exp"]["failed"], 0, "{asking}");
    let page = node.batch(&["workflow-items", &run, "bright"]);
    assert_eq!(page["items"][0]["state"], "asking", "{page}");
}

/// A decision the network answered (through the debug control endpoint)
/// before the batch is submitted: the item goes through it to the export.
#[cfg(debug_assertions)]
#[test]
fn an_answered_decision_lets_the_item_through_to_the_export() {
    let node = Node::start("answered", DECIDED, true);
    let answer = node.call(
        "control",
        "control.sock",
        &["decision-decide", "bright", "exposure", "0.5"],
    );
    assert_eq!(answer["ok"], true, "{answer}");
    let pic = node.picture("p.png");
    let out = node.dir.join("out");
    let submitted = node.submit(&out, &[&pic], &[]);
    let run = submitted["run"].as_str().unwrap().to_string();
    let done = node.finished(&run);
    assert_eq!(done["nodes"]["bright"]["done"], 1, "{done}");
    assert_eq!(done["nodes"]["exp"]["done"], 1, "{done}");
    let items = node.items(&run);
    assert!(Path::new(items[0]["output"].as_str().unwrap()).exists());
}

/// A fault is injected through the debug control endpoint only.
#[cfg(debug_assertions)]
#[test]
fn an_injected_fault_is_retried_then_succeeds() {
    let node = Node::start("fault", NETWORK, true);
    let pic = node.picture("p.png");
    let fault = node.call("control", "control.sock", &["fault", "exp", "compute"]);
    assert_eq!(fault["ok"], true, "{fault}");
    let out = node.dir.join("out");
    let submitted = node.submit(&out, &[&pic], &[]);
    let run = submitted["run"].as_str().unwrap().to_string();
    let done = node.finished(&run);
    assert_eq!(done["nodes"]["exp"]["done"], 1, "{done}");
    let items = node.items(&run);
    assert_eq!(
        items[0]["row"]["attempts"], 2,
        "failed once, then done: {items:?}"
    );
    assert!(Path::new(items[0]["output"].as_str().unwrap()).exists());
}
