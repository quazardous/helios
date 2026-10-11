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

#[test]
fn exports_are_written_next_to_the_network_and_reported() {
    let network = r#"{
      "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 32, "height": 16, "channels": 4 } }],
      "modules": [{ "name": "exp", "module": "exposure", "params": { "exposure": { "float": 1.0 } } }],
      "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }],
      "exports": [
        { "node": "exp", "region": { "x": 0, "y": 0, "width": 32, "height": 16 }, "path": "out.png" },
        { "node": "exp", "region": { "x": 0, "y": 0, "width": 32, "height": 16 }, "path": "out.tif" },
        { "node": "exp", "region": { "x": 0, "y": 0, "width": 32, "height": 16 }, "path": "missing/out.jpg" }
      ]
    }"#;
    let sandbox = Sandbox::start("exports", network);
    let (code, idle) = sandbox.inspect(&["wait-event", "node.idle", "--timeout", "10000"]);
    assert_eq!(code, 0, "{idle}");
    assert_eq!(idle["event"]["payload"]["exports"], 3);
    assert_eq!(
        idle["event"]["payload"]["failed"], 1,
        "missing/ does not exist"
    );

    let (_, state) = sandbox.inspect(&["state"]);
    let states: Vec<_> = state["state"]["node"]["exports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["done", "done", "failed"]);
    assert!(sandbox.dir.join("out.png").is_file());
    assert!(sandbox.dir.join("out.tif").is_file());

    let (_, events) = sandbox.inspect(&["events"]);
    let written: Vec<_> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "export.written")
        .collect();
    assert_eq!(written.len(), 2);
    assert!(
        written
            .iter()
            .all(|e| e["payload"]["bytes"].as_u64().unwrap() > 0)
    );
}

#[test]
fn an_export_without_a_format_is_refused_at_load() {
    let network = r#"{
      "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 4, "height": 4, "channels": 4 } }],
      "exports": [{ "node": "src", "port": "image", "region": { "x": 0, "y": 0, "width": 4, "height": 4 }, "path": "out.webp" }]
    }"#;
    let dir = std::env::temp_dir().join(format!("helios-system-noformat-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("network.json"), network).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_helios-node"))
        .arg("--graph")
        .arg(dir.join("network.json"))
        .arg("--socket")
        .arg(dir.join("node.sock"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no format for"));
}

#[test]
fn files_are_sources_read_next_to_the_network() {
    let images = std::env::var_os("HELIOS_TEST_IMAGES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../darktable/src/tests/integration/images")
        });
    let raw = images.join("mire1.cr2");
    if !raw.is_file() {
        eprintln!("skipped: no test images in {}", images.display());
        return;
    }
    // A picture next to the network, written by a first node's export.
    let make = r#"{
      "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 16, "height": 8, "channels": 4 } }],
      "exports": [{ "node": "src", "port": "image", "region": { "x": 0, "y": 0, "width": 16, "height": 8 }, "path": "pic.png" }]
    }"#;
    let maker = Sandbox::start("make-picture", make);
    maker.inspect(&["wait-event", "node.idle", "--timeout", "10000"]);
    let picture = maker.dir.join("pic.png");
    assert!(picture.is_file());

    let network = format!(
        r#"{{
      "sources": [
        {{ "name": "raw", "file": {raw:?} }},
        {{ "name": "pic", "file": {picture:?} }}
      ],
      "modules": [{{ "name": "exp", "module": "exposure", "params": {{ "exposure": {{ "float": 1.0 }} }} }}],
      "connections": [{{ "from": "pic", "port": "image", "to": "exp", "input": "input" }}],
      "renders": [
        {{ "node": "raw", "port": "image", "region": {{ "x": 0, "y": 0, "width": 64, "height": 64 }} }},
        {{ "node": "exp", "port": "output", "region": {{ "x": 0, "y": 0, "width": 16, "height": 8 }} }}
      ]
    }}"#
    );
    let sandbox = Sandbox::start("files", &network);
    let (code, idle) = sandbox.inspect(&["wait-event", "node.idle", "--timeout", "30000"]);
    assert_eq!(code, 0, "{idle}");
    assert_eq!(idle["event"]["payload"]["failed"], 0, "{idle}");
    let (_, graph) = sandbox.inspect(&["describe", "graph"]);
    let nodes = graph["describe"]["nodes"].as_array().unwrap();
    assert_eq!(nodes[0]["channels"], 1, "a raw loads as its mosaic");
    assert_eq!(nodes[1]["channels"], 4, "a picture loads as RGBA");
    assert_eq!(nodes[1]["region"]["width"], 16);
}

#[test]
fn a_source_says_either_synthetic_or_file() {
    let dir = std::env::temp_dir().join(format!("helios-system-both-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("network.json"),
        r#"{ "sources": [{ "name": "s" }] }"#,
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_helios-node"))
        .arg("--graph")
        .arg(dir.join("network.json"))
        .arg("--socket")
        .arg(dir.join("node.sock"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("either `synthetic` or `file`"));
}

#[test]
fn a_node_whose_stdout_is_closed_keeps_serving() {
    let dir = std::env::temp_dir().join(format!("helios-system-closed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("network.json"), NETWORK).unwrap();
    // The reading end is gone before the node writes `ready`.
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let mut node = Command::new(env!("CARGO_BIN_EXE_helios-node"))
        .arg("--graph")
        .arg(dir.join("network.json"))
        .arg("--socket")
        .arg(dir.join("node.sock"))
        .stdout(writer)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Nothing tells this test when the node serves (that is what stdout was
    // for): wait for its socket, bounded, or for the node to die.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let served = loop {
        if let Some(status) = node.try_wait().unwrap() {
            break Err(format!("the node exited: {status}"));
        }
        let state = Command::new(env!("CARGO_BIN_EXE_helios"))
            .args(["inspect", "--socket"])
            .arg(dir.join("node.sock"))
            .args(["wait-event", "node.idle", "--timeout", "10000"])
            .output()
            .unwrap();
        if state.status.code() == Some(0) {
            break Ok(());
        }
        if std::time::Instant::now() > deadline {
            break Err("the node never served".into());
        }
        std::thread::yield_now();
    };
    let _ = node.kill();
    let output = node.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        served,
        Ok(()),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_raw_goes_through_the_pipe_to_a_file() {
    let images = std::env::var_os("HELIOS_TEST_IMAGES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../darktable/src/tests/integration/images")
        });
    let raw = images.join("mire1.cr2");
    if !raw.is_file() {
        eprintln!("skipped: no test images in {}", images.display());
        return;
    }
    // The modules read the camera's levels, balance and pattern from the
    // context the raw source gives them: nothing is passed by hand.
    let region = r#"{ "x": 0, "y": 0, "width": 256, "height": 128 }"#;
    let network = format!(
        r#"{{
      "sources": [{{ "name": "raw", "file": {raw:?} }}],
      "modules": [
        {{ "name": "prep", "module": "rawprepare" }},
        {{ "name": "wb", "module": "whitebalance" }},
        {{ "name": "hl", "module": "highlights" }},
        {{ "name": "demosaic", "module": "demosaic" }},
        {{ "name": "exp", "module": "exposure", "params": {{ "exposure": {{ "float": 1.0 }} }} }}
      ],
      "connections": [
        {{ "from": "raw", "port": "image", "to": "prep", "input": "input" }},
        {{ "from": "prep", "port": "output", "to": "wb", "input": "input" }},
        {{ "from": "wb", "port": "output", "to": "hl", "input": "input" }},
        {{ "from": "hl", "port": "output", "to": "demosaic", "input": "input" }},
        {{ "from": "demosaic", "port": "output", "to": "exp", "input": "input" }}
      ],
      "exports": [{{ "node": "exp", "region": {region}, "path": "out.png" }}]
    }}"#
    );
    let sandbox = Sandbox::start("raw-pipe", &network);
    let (code, idle) = sandbox.inspect(&["wait-event", "node.idle", "--timeout", "60000"]);
    assert_eq!(code, 0, "{idle}");
    assert_eq!(idle["event"]["payload"]["failed"], 0, "{idle}");
    let png = image::open(sandbox.dir.join("out.png")).unwrap().to_rgb8();
    assert_eq!(png.dimensions(), (256, 128));
    // Not black, not white: real light came through.
    let mean = png
        .pixels()
        .map(|p| p.0.iter().map(|v| *v as f64).sum::<f64>() / 3.0)
        .sum::<f64>()
        / (256.0 * 128.0);
    assert!(mean > 5.0 && mean < 250.0, "mean {mean}");
}
