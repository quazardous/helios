//! The node's commands (`commands.list`, `command.run`), through the
//! `helios` CLI: every control verb listed with its params' schema and
//! whether it can run here, a run answering as its request does, params
//! refused by their schema, and a command that cannot run saying why.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use serde_json::{Value, json};

const NETWORK: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}"#;

/// A node without a vault serving the batch endpoint, and the debug
/// endpoint where the build has one; killed and removed when dropped.
struct Node {
    dir: PathBuf,
    child: Child,
}

impl Node {
    fn start(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("helios-commands-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("network.json"), NETWORK).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_helios-node"));
        command
            .arg("--graph")
            .arg(dir.join("network.json"))
            .arg("--socket")
            .arg(dir.join("node.sock"))
            .arg("--batch-control")
            .arg(dir.join("batch.sock"))
            .stdout(Stdio::piped());
        if cfg!(debug_assertions) {
            command
                .arg("--control")
                .arg(dir.join("control.sock"))
                .env("HELIOS_DEBUG_CONTROL", "1");
        }
        let mut child = command.spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(line.starts_with("ready "), "{line}");
        Self { dir, child }
    }

    fn call(&self, socket: &str, args: &[&str]) -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_helios"))
            .arg("control")
            .arg("--socket")
            .arg(self.dir.join(socket))
            .args(args)
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    fn batch(&self, args: &[&str]) -> Value {
        self.call("batch.sock", args)
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An answer without what differs between two calls: its bus sequence,
/// the time it was given (`workflow.runs`), and the command and origin a
/// run adds.
fn bare(mut answer: Value) -> Value {
    if let Some(map) = answer.as_object_mut() {
        for key in ["seq", "now", "command", "origin"] {
            map.remove(key);
        }
    }
    answer
}

fn command<'a>(list: &'a Value, id: &str) -> &'a Value {
    list["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no command {id} in {list}"))
}

#[test]
fn the_list_has_every_verb_with_its_schema_and_state() {
    let node = Node::start("list");
    let list = node.batch(&["commands"]);
    assert_eq!(list["ok"], true, "{list}");
    let commands = list["commands"].as_array().unwrap();
    // Every command of the node's registry (its control verbs, the
    // library's and calibration's commands), once each: counted from the
    // registry, so a module that registers commands keeps this true.
    let listed: Vec<&str> = commands.iter().map(|c| c["id"].as_str().unwrap()).collect();
    let registered: Vec<&str> = helios_node::commands::registry()
        .specs()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(listed.len(), registered.len(), "{list}");
    for id in registered
        .iter()
        .copied()
        .chain(helios_node::library::IDS.iter().copied())
        .chain(helios_node::calibrate::PROFILE_IDS.iter().copied())
    {
        assert!(listed.contains(&id), "{id} missing from {list}");
    }
    for c in commands {
        assert_eq!(c["params"]["type"], "object", "{c}");
        assert!(c["label"].as_str().is_some_and(|l| !l.is_empty()), "{c}");
        assert!(c["enabled"].is_boolean(), "{c}");
    }
    let export = command(&list, "export");
    assert_eq!(export["enabled"], true, "{export}");
    assert_eq!(
        export["params"]["required"],
        json!(["node", "path", "format"])
    );
    // Not served by this endpoint: said so, by key.
    let pause = command(&list, "pause");
    assert_eq!(pause["enabled"], false, "{pause}");
    assert_eq!(
        pause["message"]["key"], "node-command-debug-only",
        "{pause}"
    );
    // Served, but the node has no vault.
    let import = command(&list, "library.import");
    assert_eq!(import["needs"], json!(["vault"]), "{import}");
    assert_eq!(import["enabled"], false, "{import}");
    assert_eq!(
        import["message"]["key"], "node-command-needs-vault",
        "{import}"
    );
    assert!(
        import["reason"].as_str().unwrap().contains("--vault"),
        "{import}"
    );
    // A filter and enabled_only narrow it.
    let workflow = node.batch(&["commands", "workflow", "--enabled"]);
    let ids: Vec<&str> = workflow["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 9, "{workflow}");
    assert!(ids.iter().all(|id| id.starts_with("workflow.")), "{ids:?}");
}

#[test]
fn a_run_answers_as_its_request_does() {
    let node = Node::start("same");
    let direct = node.batch(&["workflow-runs"]);
    let run = node.batch(&["run", "workflow.runs"]);
    assert_eq!(run["ok"], true, "{run}");
    assert_eq!(run["command"], "workflow.runs", "{run}");
    assert_eq!(run["origin"], "cli", "{run}");
    assert_eq!(bare(run), bare(direct));
    // A refusal too, by the same key and words.
    let direct = node.batch(&["job-cancel", "export-99"]);
    let run = node.batch(&["run", "job.cancel", r#"{"id":"export-99"}"#]);
    assert_eq!(run["ok"], false, "{run}");
    assert_eq!(bare(run), bare(direct));
}

#[test]
fn params_are_checked_against_the_schema() {
    let node = Node::start("params");
    for (params, error) in [
        (
            r#"{"node":"exp","format":"png"}"#,
            "params: `path` is missing",
        ),
        (
            r#"{"node":"exp","path":"/x.gif","format":"gif"}"#,
            r#"params.format: expected one of "jpeg", "png", "tiff""#,
        ),
        (
            r#"{"node":"exp","path":"/x.png","format":"png","speed":2}"#,
            "params: `speed` is not one of its params",
        ),
    ] {
        let answer = node.batch(&["run", "export", params]);
        assert_eq!(answer["ok"], false, "{answer}");
        assert_eq!(answer["message"]["key"], "node-command-params", "{answer}");
        assert!(
            answer["error"].as_str().unwrap().ends_with(error),
            "{answer} should end with {error}"
        );
    }
    let unknown = node.batch(&["run", "teleport"]);
    assert_eq!(
        unknown["message"]["key"], "node-command-unknown",
        "{unknown}"
    );
}

#[test]
fn a_command_that_cannot_run_says_why() {
    let node = Node::start("disabled");
    let import = node.batch(&["run", "library.import", r#"{"paths":["/photos"]}"#]);
    assert_eq!(import["ok"], false, "{import}");
    assert_eq!(
        import["message"]["key"], "node-command-needs-vault",
        "{import}"
    );
    let pause = node.batch(&["run", "pause"]);
    assert_eq!(pause["ok"], false, "{pause}");
    assert_eq!(
        pause["message"]["key"], "node-command-debug-only",
        "{pause}"
    );
}

/// On the debug endpoint every verb runs: a change to the edit answers its
/// undo group, the same through a command as through its request.
#[cfg(debug_assertions)]
#[test]
fn a_run_on_the_debug_endpoint_keeps_its_undo_group() {
    let node = Node::start("debug");
    let list = node.call("control.sock", &["commands", "--enabled"]);
    assert_eq!(list["commands"].as_array().unwrap().len(), 29, "{list}");
    let run = node.call(
        "control.sock",
        &[
            "run",
            "set_param",
            r#"{"node":"exp","param":"exposure","value":{"float":1.0}}"#,
        ],
    );
    assert_eq!(run["ok"], true, "{run}");
    assert_eq!(run["group"], "set exp.exposure", "{run}");
    assert_eq!(run["origin"], "cli", "{run}");
    let direct = node.call("control.sock", &["set-param", "exp", "exposure", "1.0"]);
    assert_eq!(bare(run), bare(direct));
    // The run is on the bus, with who ran it.
    let out = Command::new(env!("CARGO_BIN_EXE_helios"))
        .args(["inspect", "--socket"])
        .arg(node.dir.join("node.sock"))
        .arg("events")
        .output()
        .unwrap();
    let events: Value = serde_json::from_slice(&out.stdout).unwrap();
    let ran: Vec<&Value> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "control.command")
        .collect();
    assert_eq!(ran.len(), 1, "{events}");
    assert_eq!(
        ran[0]["payload"],
        json!({ "id": "set_param", "origin": "cli" })
    );
}

/// Runs `helios` with `args`: exit code, stdout, stderr.
fn helios(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_helios"))
        .args(args)
        .env_remove("HELIOS_NODE")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// The `control.command` events of the node: (id, origin).
fn ran(node: &Node) -> Vec<(String, String)> {
    let out = Command::new(env!("CARGO_BIN_EXE_helios"))
        .args(["inspect", "--socket"])
        .arg(node.dir.join("node.sock"))
        .arg("events")
        .output()
        .unwrap();
    let events: Value = serde_json::from_slice(&out.stdout).unwrap();
    events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "control.command")
        .map(|e| {
            (
                e["payload"]["id"].as_str().unwrap().to_string(),
                e["payload"]["origin"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// `helios commands` and `helios run` reach a running node by its address,
/// as `helios-mcp`'s generic pair does: the list says why a command is
/// greyed, a run is the CLI's (or the agent's), a greyed command and bad
/// params are refused with their reasons.
#[test]
fn the_cli_and_the_mcp_pair_list_and_run_commands_on_a_running_node() {
    let node = Node::start("surface");
    let address = node.dir.join("batch.sock");
    let address = address.to_str().unwrap();

    let (code, out, err) = helios(&["commands", "--node", address, "--filter", "pause"]);
    assert_eq!(code, 0, "{out}{err}");
    let list: Value = serde_json::from_str(&out).unwrap();
    let pause = command(&list, "pause");
    assert_eq!(pause["enabled"], false, "{pause}");
    assert_eq!(pause["scope"], "node", "{pause}");
    assert!(
        pause["disabledReason"]
            .as_str()
            .unwrap()
            .starts_with("only the debug control endpoint"),
        "{pause}"
    );

    let (code, out, err) = helios(&["run", "workflow.runs", "--node", address]);
    assert_eq!(code, 0, "{out}{err}");
    let run: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(run["origin"], "cli", "{run}");
    let (code, _, err) = helios(&["run", "pause", "--node", address]);
    assert_eq!(code, 1);
    assert!(err.contains("only the debug control endpoint"), "{err}");
    let (code, _, err) = helios(&[
        "run",
        "export",
        "--node",
        address,
        "--json",
        r#"{"node":"exp","path":"/x.gif"}"#,
        "--param",
        "format=gif",
    ]);
    assert_eq!(code, 1);
    assert!(
        err.contains(r#"params.format: expected one of "jpeg", "png", "tiff""#),
        "{err}"
    );
    // The camera look's fit, reached through the registry by name: this
    // node keeps no vault, so it is refused with that reason.
    let (code, _, err) = helios(&[
        "run",
        "look.camera.fit",
        "--node",
        address,
        "--json",
        r#"{"images":["/a.nef"],"dry_run":true}"#,
    ]);
    assert_eq!(code, 1);
    assert!(err.contains("--vault"), "{err}");

    // The MCP pair, on the same node.
    let mut sockets = helios_node::mcp::Sockets {
        inspect: None,
        control: None,
        node_bin: PathBuf::from(env!("CARGO_BIN_EXE_helios-node")),
        ai: None,
    };
    let mut tool = |name: &str, arguments: Value| {
        let answer = helios_node::mcp::handle(
            &mut sockets,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                     "params": { "name": name, "arguments": arguments } }),
        )
        .unwrap();
        let result = &answer["result"];
        let text = result["content"][0]["text"].as_str().unwrap().to_string();
        (
            result["isError"] == true,
            serde_json::from_str(&text).unwrap_or(json!(text)),
        )
    };
    let (failed, list) = tool(
        "helios_commands_list",
        json!({ "node": address, "enabled_only": true }),
    );
    assert!(!failed, "{list}");
    assert!(
        list["commands"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["enabled"] == true),
        "{list}"
    );
    let (failed, all) = tool("helios_commands_list", json!({ "node": address }));
    assert!(!failed, "{all}");
    let fit = command(&all, "look.camera.fit");
    assert_eq!(fit["needs"], json!(["vault"]), "{fit}");
    assert_eq!(fit["params"]["required"], json!(["images"]), "{fit}");
    let (failed, run) = tool(
        "helios_command_run",
        json!({ "node": address, "id": "workflow.runs" }),
    );
    assert!(!failed, "{run}");
    assert_eq!(run["origin"], "agent:mcp", "{run}");
    let (failed, refused) = tool(
        "helios_command_run",
        json!({ "node": address, "id": "pause" }),
    );
    assert!(failed);
    assert!(
        refused
            .as_str()
            .unwrap()
            .starts_with("only the debug control endpoint"),
        "{refused}"
    );

    assert_eq!(
        ran(&node),
        [
            ("workflow.runs".to_string(), "cli".to_string()),
            ("workflow.runs".to_string(), "agent:mcp".to_string()),
        ]
    );
}
