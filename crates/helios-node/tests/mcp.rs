//! `helios-mcp` in front of a running node: an MCP client lists the tools,
//! renders through control and observes through inspect.

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

    drop(client);
    let _ = server.wait();
    let _ = node.kill();
    let _ = node.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
