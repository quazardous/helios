//! A node driven through the debug control endpoint and observed through
//! inspection, both only by the `helios` CLI: renders, parameter changes,
//! pause and resume, and error paths triggered by injected faults.

#![cfg(all(unix, debug_assertions))]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use serde_json::Value;

const NETWORK: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 32, "height": 16, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}"#;

struct Sandbox {
    dir: PathBuf,
    node: Child,
}

impl Sandbox {
    fn start(name: &str, control: bool) -> Self {
        let dir =
            std::env::temp_dir().join(format!("helios-control-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("network.json"), NETWORK).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_helios-node"));
        command
            .arg("--graph")
            .arg(dir.join("network.json"))
            .arg("--socket")
            .arg(dir.join("node.sock"))
            .arg("--control")
            .arg(dir.join("control.sock"))
            .stdout(Stdio::piped());
        if control {
            command.env("HELIOS_DEBUG_CONTROL", "1");
        } else {
            command.env_remove("HELIOS_DEBUG_CONTROL");
        }
        let mut node = command.spawn().unwrap();
        let mut line = String::new();
        BufReader::new(node.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        Self { dir, node }
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

    fn control(&self, args: &[&str]) -> (i32, Value) {
        self.run("control", "control.sock", args)
    }

    fn inspect(&self, args: &[&str]) -> (i32, Value) {
        self.run("inspect", "node.sock", args)
    }

    fn last_render(&self) -> Value {
        let (_, state) = self.inspect(&["describe", "node"]);
        state["describe"]["renders"]
            .as_array()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.node.kill();
        let _ = self.node.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const REGION: &str = "0,0,32,16";

#[test]
fn renders_parameters_and_pause_through_control() {
    let s = Sandbox::start("drive", true);
    let (code, answer) = s.control(&["render", "exp", "--region", REGION]);
    assert_eq!(code, 0, "{answer}");
    assert!(answer["job"].is_u64());
    assert_eq!(s.inspect(&["wait-idle"]).0, 0);
    let first = s.last_render();
    assert!(first["hash"].is_string(), "{first}");

    // A parameter change makes the node stale; rendering again makes it done.
    assert_eq!(s.control(&["set-param", "exp", "exposure", "1.5"]).0, 0);
    let (_, state) = s.inspect(&["describe", "node"]);
    assert_eq!(
        state["describe"]["headers"][0]["nodes"][1]["state"],
        "stale"
    );
    s.control(&["render", "exp", "--region", REGION]);
    s.inspect(&["wait-idle"]);
    assert_ne!(s.last_render()["hash"], first["hash"]);

    // Paused: the render waits, the wait times out and says what is queued.
    assert_eq!(s.control(&["pause"]).0, 0);
    s.control(&["render", "exp", "--region", "0,0,8,8"]);
    let (code, busy) = s.inspect(&["wait-idle", "--timeout", "100"]);
    assert_eq!(code, 1);
    assert_eq!(busy["scheduler"]["paused"], true);
    assert_eq!(busy["scheduler"]["queued"].as_array().unwrap().len(), 1);
    assert_eq!(s.control(&["resume"]).0, 0);
    assert_eq!(s.inspect(&["wait-idle"]).0, 0);

    // Every action is on the bus.
    let (_, events) = s.inspect(&["events"]);
    let actions = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "control.action")
        .count();
    assert_eq!(actions, 6);
}

#[test]
fn faults_trigger_error_paths_once() {
    let s = Sandbox::start("faults", true);
    assert_eq!(s.control(&["fault", "exp", "compute"]).0, 0);
    s.control(&["render", "exp", "--region", REGION]);
    s.inspect(&["wait-idle"]);
    assert_eq!(
        s.last_render()["error"],
        "exp: injected fault (computation)"
    );
    let (_, scheduler) = s.inspect(&["describe", "scheduler"]);
    assert_eq!(scheduler["describe"]["totals"]["failed"], 1);

    s.control(&["render", "exp", "--region", REGION]);
    s.inspect(&["wait-idle"]);
    assert!(
        s.last_render()["error"].is_null(),
        "a fault strikes once and is not cached"
    );

    assert_eq!(s.control(&["fault", "src", "source-read"]).0, 0);
    s.control(&["render", "exp", "--region", "0,0,4,4"]);
    s.inspect(&["wait-idle"]);
    assert_eq!(
        s.last_render()["error"],
        "src: injected fault (source read)"
    );

    let (code, refused) = s.control(&["set-param", "exp", "exposure", "99"]);
    assert_eq!(code, 1);
    assert!(refused["error"].as_str().unwrap().contains("outside"));
    assert_eq!(
        s.control(&["fault", "nope", "compute"]).1["error"],
        "no node named `nope`"
    );
}

#[test]
fn control_is_refused_without_the_environment_flag() {
    let dir = std::env::temp_dir().join(format!("helios-control-off-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("network.json"), NETWORK).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_helios-node"))
        .arg("--graph")
        .arg(dir.join("network.json"))
        .arg("--socket")
        .arg(dir.join("node.sock"))
        .arg("--control")
        .arg(dir.join("control.sock"))
        .env_remove("HELIOS_DEBUG_CONTROL")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("only when HELIOS_DEBUG_CONTROL is set"));
}
