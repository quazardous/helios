//! `helios-mcp` in front of a running node: an MCP client lists the tools,
//! renders through control and observes through inspect, and drives a
//! run of the workflow level with its verbs.

#![cfg(all(unix, debug_assertions))]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

const NETWORK: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 16, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}"#;

struct Client {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Client {
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let message =
            json!({ "jsonrpc": "2.0", "id": self.next, "method": method, "params": params });
        writeln!(self.stdin, "{message}").unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let answer: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(answer["id"], self.next);
        answer
    }

    /// Calls a tool; returns whether it failed and the node's answer.
    fn tool(&mut self, name: &str, arguments: Value) -> (bool, Value) {
        let answer = self.call(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        let result = &answer["result"];
        let text = result["content"][0]["text"].as_str().unwrap();
        (
            result["isError"] == true,
            serde_json::from_str(text).unwrap_or(json!(text)),
        )
    }
}

#[test]
fn an_agent_drives_a_node_through_mcp() {
    let dir = std::env::temp_dir().join(format!("helios-mcp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("network.json"), NETWORK).unwrap();
    let mut node: Child = Command::new(env!("CARGO_BIN_EXE_helios-node"))
        .arg("--graph")
        .arg(dir.join("network.json"))
        .arg("--socket")
        .arg(dir.join("node.sock"))
        .arg("--control")
        .arg(dir.join("control.sock"))
        .env("HELIOS_DEBUG_CONTROL", "1")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(node.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();

    let mut server = Command::new(env!("CARGO_BIN_EXE_helios-mcp"))
        .arg("--socket")
        .arg(dir.join("node.sock"))
        .arg("--control")
        .arg(dir.join("control.sock"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut client = Client {
        stdin: server.stdin.take().unwrap(),
        stdout: BufReader::new(server.stdout.take().unwrap()),
        next: 0,
    };

    let init = client.call(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {} }),
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "helios");
    writeln!(
        client.stdin,
        "{}",
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
    )
    .unwrap();

    let tools = client.call("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"helios_render") && names.contains(&"helios_wait_idle"),
        "{names:?}"
    );

    let (failed, render) = client.tool(
        "helios_render",
        json!({ "node": "exp", "region": { "x": 0, "y": 0, "width": 16, "height": 8 } }),
    );
    assert!(!failed, "{render}");
    let (failed, _) = client.tool("helios_wait_idle", json!({}));
    assert!(!failed);
    let (_, node_state) = client.tool("helios_describe", json!({ "entity": "node" }));
    assert!(
        node_state["describe"]["renders"][0]["hash"].is_string(),
        "{node_state}"
    );

    let (_, modules) = client.tool("helios_describe", json!({ "entity": "modules" }));
    let exposure = &modules["describe"]["modules"]["exposure"];
    assert_eq!(
        exposure["params"][0]["name"], "exposure",
        "module schemas come from manifests"
    );
    assert_eq!(exposure["params"][0]["type"], "float");

    let (failed, refused) = client.tool(
        "helios_set_param",
        json!({ "node": "exp", "param": "exposure", "value": { "float": 99.0 } }),
    );
    assert!(failed, "an out-of-range value is an error result");
    assert!(refused["error"].as_str().unwrap().contains("outside"));
    let (failed, on) = client.tool("helios_node_bypass", json!({ "node": "exp", "on": true }));
    assert!(!failed, "{on}");
    assert_eq!(on["bypassed"], true, "{on}");
    let (failed, off) = client.tool("helios_node_bypass", json!({ "node": "exp", "on": false }));
    assert!(!failed, "{off}");
    assert_eq!(off["bypassed"], false, "{off}");
    let (failed, missing) = client.tool("helios_describe", json!({}));
    assert!(failed);
    assert_eq!(missing, json!("helios_describe: missing argument `entity`"));

    workflow_verbs(&mut client, &dir);

    drop(client);
    let _ = server.wait();
    let _ = node.kill();
    let _ = node.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The run of `client`'s node named `run`, once `ready` holds for it.
fn run_until(client: &mut Client, run: &str, ready: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_mins(1);
    loop {
        let (_, runs) = client.tool("helios_workflow_runs", json!({}));
        let found = runs["runs"]
            .as_array()
            .and_then(|r| r.iter().find(|r| r["id"] == run))
            .cloned()
            .unwrap_or(Value::Null);
        if ready(&found) {
            return found;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run {run} not ready: {runs}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// The verbs on a run, as MCP tools: an item moved to the top, the run
/// paused and resumed, an item cancelled then retried, the rows checked.
fn workflow_verbs(client: &mut Client, dir: &std::path::Path) {
    let images: Vec<String> = ["a.png", "b.png"]
        .iter()
        .map(|name| {
            let path = dir.join(name);
            image::RgbImage::from_fn(4, 4, |x, y| image::Rgb([(x * 50) as u8, (y * 50) as u8, 9]))
                .save(&path)
                .unwrap();
            path.to_str().unwrap().to_string()
        })
        .collect();
    let out = dir.join("out");
    // The exports wait in the scheduler's queue.
    let (failed, _) = client.tool("helios_pause", json!({}));
    assert!(!failed);
    let (failed, submitted) = client.tool(
        "helios_workflow_submit",
        json!({ "items": images, "out": out, "format": "png", "concurrency": 1 }),
    );
    assert!(!failed, "{submitted}");
    let run = submitted["run"].as_str().unwrap().to_string();
    run_until(client, &run, |r| r["nodes"]["exp"]["running"] == 1);

    let (failed, paused) = client.tool("helios_workflow_pause", json!({ "run": run }));
    assert!(!failed, "{paused}");
    assert_eq!(paused["state"], "paused", "{paused}");
    let (failed, moved) = client.tool(
        "helios_workflow_prioritize",
        json!({ "run": run, "items": [images[1]], "to": "top" }),
    );
    assert!(!failed, "{moved}");
    assert_eq!(moved["moved"], 1, "{moved}");
    let (failed, cancelled) = client.tool(
        "helios_workflow_cancel",
        json!({ "run": run, "items": [images[0]] }),
    );
    assert!(!failed, "{cancelled}");
    let (failed, resumed) = client.tool("helios_workflow_resume", json!({ "run": run }));
    assert!(!failed, "{resumed}");
    let (failed, _) = client.tool("helios_resume", json!({}));
    assert!(!failed);
    let done = run_until(client, &run, |r| r["state"] == "finished");
    assert_eq!(done["nodes"]["exp"]["done"], 1, "{done}");
    assert_eq!(done["nodes"]["exp"]["cancelled"], 1, "{done}");
    let (_, page) = client.tool("helios_workflow_items", json!({ "run": run }));
    let states: Vec<(&str, &str)> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["title"].as_str().unwrap(), i["state"].as_str().unwrap()))
        .collect();
    assert_eq!(
        states,
        [("b.png", "done"), ("a.png", "cancelled")],
        "{page}"
    );

    let (failed, retried) = client.tool("helios_workflow_retry", json!({ "run": run }));
    assert!(!failed, "{retried}");
    assert_eq!(retried["retried"], 1, "{retried}");
    let done = run_until(client, &run, |r| {
        r["state"] == "finished" && r["nodes"]["exp"]["done"] == 2
    });
    assert_eq!(done["nodes"]["exp"]["cancelled"], 0, "{done}");
    assert!(out.join("a.png").exists() && out.join("b.png").exists());
}
