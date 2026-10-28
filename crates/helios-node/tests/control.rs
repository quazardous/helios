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
        Self::start_with(name, control, NETWORK)
    }

    fn start_with(name: &str, control: bool, network: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("helios-control-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("network.json"), network).unwrap();
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
    // Outside what was rendered: a region inside it would be cropped from
    // the cache, with no source read for the fault to strike.
    s.control(&["render", "exp", "--region", "32,16,4,4"]);
    s.inspect(&["wait-idle"]);
    assert_eq!(
        s.last_render()["error"],
        "src: injected fault (source read)"
    );

    let (code, refused) = s.control(&["set-param", "exp", "exposure", "99"]);
    assert_eq!(code, 1);
    assert!(refused["error"].as_str().unwrap().contains("outside"));
    let unknown = s.control(&["fault", "nope", "compute"]).1;
    assert_eq!(unknown["error"], "no node named `nope`");
    assert_eq!(unknown["message"]["key"], "node-no-node");
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

/// A decision answered by a model (a shell script here), then by a human.
const DECIDED: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 32, "height": 16, "channels": 4 } }],
  "modules": [{ "name": "bright", "module": "decide_exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "bright", "input": "input" }],
  "decisions": { "bright": "ai_assisted" },
  "brief": "bright and airy",
  "resolver": { "command": ["sh", "-c", "cat > /dev/null; echo '{\"options\":[{\"exposure\":1.5},{\"exposure\":0.5}],\"confidence\":0.8,\"rationale\":\"dark\",\"model\":{\"name\":\"script\",\"version\":\"1\"}}'"] }
}"#;

#[test]
fn a_model_proposes_and_a_human_accepts_through_control() {
    let s = Sandbox::start_with("decide", true, DECIDED);
    let (_, list) = s.control(&["decisions"]);
    assert_eq!(list["decisions"]["bright"]["mode"], "ai_assisted", "{list}");

    // Before any answer the decision blocks its render.
    s.control(&["render", "bright", "--region", REGION]);
    s.inspect(&["wait-idle"]);
    assert!(
        s.last_render()["error"]
            .as_str()
            .unwrap()
            .contains("pending")
    );

    let (code, resolved) = s.control(&["decision-resolve"]);
    assert_eq!(code, 0, "{resolved}");
    assert_eq!(resolved["report"]["proposed"][0], "bright", "{resolved}");
    let (_, list) = s.control(&["decisions"]);
    let proposal = &list["decisions"]["bright"]["proposal"];
    assert_eq!(proposal["options"].as_array().unwrap().len(), 2, "{list}");

    // The human takes the second option; the render goes through.
    let (code, accepted) = s.control(&["decision-accept", "bright", "1"]);
    assert_eq!(code, 0, "{accepted}");
    s.control(&["render", "bright", "--region", REGION]);
    s.inspect(&["wait-idle"]);
    assert!(s.last_render()["hash"].is_string(), "{}", s.last_render());
    let (_, list) = s.control(&["decisions"]);
    assert_eq!(list["decisions"]["bright"]["status"], "answered", "{list}");

    // Asked again, it waits again; a human answers directly.
    assert_eq!(s.control(&["decision-reask", "bright"]).0, 0);
    let (code, decided) = s.control(&["decision-decide", "bright", "exposure", "2"]);
    assert_eq!(code, 0, "{decided}");
    assert_eq!(decided["version"], 2, "{decided}");
}

#[test]
fn a_source_takes_a_file_and_exports_take_the_network_as_it_was() {
    let s = Sandbox::start("export", true);
    let picture = s.dir.join("in.png");
    image::RgbImage::from_fn(20, 10, |x, y| {
        image::Rgb([(x * 10) as u8, (y * 20) as u8, 90])
    })
    .save(&picture)
    .unwrap();
    let (code, loaded) = s.control(&["load-source", "src", picture.to_str().unwrap()]);
    assert_eq!(code, 0, "{loaded}");
    assert_eq!(loaded["source"]["region"]["width"], 20, "{loaded}");
    assert_eq!(loaded["source"]["raw"], false);
    // An untagged picture is read as sRGB, and the report says so.
    assert_eq!(
        loaded["source"]["colour"]["from"]["tag"], "untagged",
        "{loaded}"
    );
    assert_eq!(loaded["source"]["colour"]["space"], "sRGB (assumed)");

    // Queued while paused, then the network changes: the first export
    // still renders the network as it was asked.
    let out = |name: &str| s.dir.join(name).to_str().unwrap().to_string();
    assert_eq!(s.control(&["pause"]).0, 0);
    let (code, first) = s.control(&["export", "exp", "--format", "png", "--to", &out("a.png")]);
    assert_eq!(code, 0, "{first}");
    assert_eq!(first["region"]["height"], 10, "the whole extent: {first}");
    assert_eq!(s.control(&["set-param", "exp", "exposure", "2"]).0, 0);
    s.control(&["export", "exp", "--format", "png", "--to", &out("b.png")]);
    // Changed for one export only.
    s.control(&[
        "export",
        "exp",
        "--format",
        "png",
        "--set",
        "exp.exposure=0",
        "--to",
        &out("c.png"),
    ]);
    s.control(&[
        "export",
        "exp",
        "--format",
        "jpeg",
        "--quality",
        "50",
        "--to",
        &out("d.jpg"),
    ]);
    assert_eq!(s.control(&["resume"]).0, 0);
    assert_eq!(s.inspect(&["wait-idle"]).0, 0);
    let read = |name: &str| std::fs::read(s.dir.join(name)).unwrap();
    assert_ne!(read("a.png"), read("b.png"));
    assert_eq!(
        read("a.png"),
        read("c.png"),
        "exposure 0 is the network as it was"
    );
    let jpeg = image::load_from_memory(&read("d.jpg")).unwrap();
    assert_eq!((jpeg.width(), jpeg.height()), (20, 10));

    let (code, missing) = s.control(&["load-source", "src", "/nonexistent/x.png"]);
    assert_eq!(code, 1);
    assert!(
        missing["error"]
            .as_str()
            .unwrap()
            .contains("/nonexistent/x.png")
    );
    let (code, wrong) = s.control(&["load-source", "exp", picture.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(
        wrong["error"].as_str().unwrap().contains("not a source"),
        "{wrong}"
    );
    let (code, usage) = s.control(&["export", "exp", "--to", &out("e.png")]);
    assert_eq!(code, 2, "{usage}");
}
