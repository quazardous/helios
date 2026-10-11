//! A node in a sandbox directory, driven and observed only through the
//! `helios` CLI, the way an agent or a script does it.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde_json::Value;

/// A throwaway directory and a running node; both go away on drop.
struct Sandbox {
    dir: PathBuf,
    node: Child,
}

impl Sandbox {
    fn start(name: &str, graph: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("helios-system-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("network.json"), graph).unwrap();
        let mut node = Command::new(env!("CARGO_BIN_EXE_helios-node"))
            .arg("--graph")
            .arg(dir.join("network.json"))
            .arg("--socket")
            .arg(dir.join("node.sock"))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        // The node says when it serves: no polling, no sleeping.
        let mut line = String::new();
        BufReader::new(node.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(line.starts_with("ready "), "node said {line:?}");
        Self { dir, node }
    }

    fn socket(&self) -> PathBuf {
        self.dir.join("node.sock")
    }

    /// Runs `helios inspect ARGS`; returns the exit code and the JSON answer.
    fn inspect(&self, args: &[&str]) -> (i32, Value) {
        let out = Command::new(env!("CARGO_BIN_EXE_helios"))
            .arg("inspect")
            .arg("--socket")
            .arg(self.socket())
            .args(args)
            .output()
            .unwrap();
        let json = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (out.status.code().unwrap(), json)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.node.kill();
        let _ = self.node.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const NETWORK: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 32, "height": 16, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure", "params": { "exposure": { "float": 1.0 } } }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }],
  "renders": [
    { "node": "exp", "port": "output", "region": { "x": 0, "y": 0, "width": 32, "height": 16 } },
    { "node": "exp", "port": "output", "region": { "x": 0, "y": 0, "width": 32, "height": 16 } }
  ]
}"#;

#[test]
fn a_node_is_driven_and_observed_through_the_cli() {
    let sandbox = Sandbox::start("render", NETWORK);

    // Renders are scheduler jobs: wait for the scheduler, not for a delay.
    let (code, idle) = sandbox.inspect(&["wait-idle", "--timeout", "10000"]);
    assert_eq!(code, 0, "{idle}");
    let (code, idle) = sandbox.inspect(&["wait-event", "node.idle", "--timeout", "10000"]);
    assert_eq!(code, 0, "{idle}");
    assert_eq!(idle["event"]["payload"]["failed"], 0);
    let (_, scheduler) = sandbox.inspect(&["describe", "scheduler"]);
    assert_eq!(scheduler["describe"]["totals"]["done"], 2);
    assert_eq!(scheduler["describe"]["recent"][0]["name"], "render exp");

    let (code, state) = sandbox.inspect(&["state"]);
    assert_eq!(code, 0);
    assert_eq!(state["state"]["node"]["idle"], true);
    let renders = state["state"]["node"]["renders"].as_array().unwrap();
    assert_eq!(renders.len(), 2);
    assert_eq!(
        renders[0]["hash"], renders[1]["hash"],
        "the same content twice"
    );
    // Budget: the second identical render computed nothing.
    assert_eq!(
        state["state"]["engine"]["counters"]["computed"], 2,
        "source region + exposure, once"
    );
    assert_eq!(state["state"]["graph"]["nodes"][1]["module"], "exposure");
    // The normalized header of every node of the first render.
    let header = &state["state"]["node"]["headers"][0]["nodes"];
    assert_eq!(header[0]["type"], "source");
    assert_eq!(header[1]["type"], "process");
    assert_eq!(header[1]["family"], "tone");
    assert_eq!(header[1]["state"], "done");
    assert_eq!(header[1]["load"]["shared"], 1);

    let (_, events) = sandbox.inspect(&["events"]);
    let kinds: Vec<&str> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.first(), Some(&"node.ready"));
    assert_eq!(kinds.last(), Some(&"node.idle"));
    assert!(kinds.contains(&"engine.step_computed"));
}

#[test]
fn failures_say_what_happened_instead() {
    let sandbox = Sandbox::start("failures", NETWORK);
    sandbox.inspect(&["wait-event", "node.idle"]);

    let (code, missed) = sandbox.inspect(&["wait-event", "node.never", "--timeout", "50"]);
    assert_eq!(code, 1);
    assert!(
        missed["recent"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "node.idle")
    );

    let (code, unknown) = sandbox.inspect(&["describe", "nope"]);
    assert_eq!(code, 1);
    assert!(
        unknown["entities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "engine")
    );

    let gone = Command::new(env!("CARGO_BIN_EXE_helios"))
        .args(["inspect", "--socket"])
        .arg(Path::new("/nonexistent/helios.sock"))
        .arg("state")
        .output()
        .unwrap();
    assert_eq!(gone.status.code(), Some(3));
    let usage = Command::new(env!("CARGO_BIN_EXE_helios"))
        .args(["inspect", "--socket", "x", "reboot"])
        .output()
        .unwrap();
    assert_eq!(usage.status.code(), Some(2));
}

#[test]
fn settings_come_from_the_layers_given_with_their_origin() {
    let dir = std::env::temp_dir().join(format!("helios-system-config-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("project")).unwrap();
    std::fs::write(
        dir.join("global.toml"),
        "[system.events]\njournal = 64\n[project.decisions]\nmode = \"human\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("project/helios.toml"),
        "[project.decisions]\nmode = \"full_ai\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("network.json"), NETWORK).unwrap();
    let mut node = Command::new(env!("CARGO_BIN_EXE_helios-node"))
        .arg("--graph")
        .arg(dir.join("network.json"))
        .arg("--socket")
        .arg(dir.join("node.sock"))
        .arg("--global")
        .arg(dir.join("global.toml"))
        .arg("--project")
        .arg(dir.join("project"))
        .args(["--set", "interface.graph.zoom=overview"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(node.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_helios"))
        .arg("inspect")
        .arg("--socket")
        .arg(dir.join("node.sock"))
        .args(["describe", "config"])
        .output()
        .unwrap();
    let _ = node.kill();
    let _ = node.wait();
    let _ = std::fs::remove_dir_all(&dir);
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    let origin = |key: &str| {
        let s = answer["describe"]["settings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["key"] == key)
            .unwrap();
        (
            s["value"].clone(),
            s["origin"].as_str().unwrap().to_string(),
        )
    };
    assert_eq!(
        origin("system.events.journal"),
        (serde_json::json!(64.0), "global".into())
    );
    assert_eq!(
        origin("project.decisions.mode"),
        (serde_json::json!("full_ai"), "project".into())
    );
    assert_eq!(
        origin("interface.graph.zoom"),
        (serde_json::json!("overview"), "run".into())
    );
    assert_eq!(origin("interface.panels.queue_when_empty").1, "default");
}

#[test]
fn a_setting_in_the_wrong_layer_is_refused() {
    let dir = std::env::temp_dir().join(format!("helios-system-badconf-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("project")).unwrap();
    std::fs::write(
        dir.join("project/helios.toml"),
        "[system.events]\njournal = 64\n",
    )
    .unwrap();
    std::fs::write(dir.join("network.json"), NETWORK).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_helios-node"))
        .arg("--graph")
        .arg(dir.join("network.json"))
        .arg("--socket")
        .arg(dir.join("n.sock"))
        .arg("--project")
        .arg(dir.join("project"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("a machine setting is set in: global, run")
    );
}

#[test]
fn a_bad_network_is_refused_with_a_reason() {
    let dir = std::env::temp_dir().join(format!("helios-system-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("bad.json"),
        r#"{ "modules": [{ "name": "a", "module": "blur" }] }"#,
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_helios-node"))
        .arg("--graph")
        .arg(dir.join("bad.json"))
        .arg("--socket")
        .arg(dir.join("n.sock"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("module `blur` unknown"));
}
