//! Sandboxes through the `helios` CLI and through `helios-mcp`: create,
//! start, observe, stop, clean; and the refusals that keep real data safe.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn helios(args: &[&str]) -> (i32, Value, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_helios"))
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// A fresh path under the temporary directory; cleaned when dropped.
struct Place(PathBuf);

impl Place {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("helios-sb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        Self(path)
    }

    fn s(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        if self.0.join("sandbox.json").exists() {
            let _ = helios(&["sandbox", "clean", self.s()]);
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_sandbox_is_created_started_stopped_and_cleaned() {
    let place = Place::new("life");
    let (code, created, err) = helios(&["sandbox", "new", place.s()]);
    assert_eq!(code, 0, "{err}");
    assert!(
        created["inspect"].as_str().unwrap().len() <= 100,
        "short socket path"
    );
    for file in [
        "network.json",
        "global.toml",
        "user.toml",
        "project/helios.toml",
    ] {
        assert!(place.0.join(file).exists(), "{file}");
    }

    let (code, started, err) = helios(&["sandbox", "start", place.s()]);
    assert_eq!(code, 0, "{err}");
    let socket = started["inspect"].as_str().unwrap().to_string();
    assert!(started["pid"].is_u64());
    let (code, _, _) = helios(&["inspect", "--socket", &socket, "wait-idle"]);
    assert_eq!(code, 0);
    let (_, graph, _) = helios(&["inspect", "--socket", &socket, "describe", "graph"]);
    assert_eq!(
        graph["describe"]["nodes"][1]["module"], "exposure",
        "the default network"
    );
    let (code, _, err) = helios(&["sandbox", "start", place.s()]);
    assert_eq!(code, 1);
    assert!(err.contains("already runs"), "{err}");

    let (_, stopped, _) = helios(&["sandbox", "stop", place.s()]);
    assert_eq!(stopped["stopped"], true);
    assert_eq!(
        helios(&["inspect", "--socket", &socket, "state"]).0,
        3,
        "the node is gone"
    );
    let (_, again, _) = helios(&["sandbox", "stop", place.s()]);
    assert_eq!(again["stopped"], false);

    assert_eq!(helios(&["sandbox", "clean", place.s()]).0, 0);
    assert!(!place.0.exists());
}

#[test]
fn real_data_is_never_touched() {
    let place = Place::new("foreign");
    std::fs::create_dir_all(&place.0).unwrap();
    std::fs::write(place.0.join("library.db"), "precious").unwrap();
    let (code, _, err) = helios(&["sandbox", "new", place.s()]);
    assert_eq!(code, 1);
    assert!(err.contains("refusing to create a sandbox"), "{err}");
    let (code, _, err) = helios(&["sandbox", "clean", place.s()]);
    assert_eq!(code, 1);
    assert!(err.contains("is not a sandbox"), "{err}");
    assert_eq!(
        std::fs::read_to_string(place.0.join("library.db")).unwrap(),
        "precious"
    );
}

fn mcp_call(
    stdin: &mut impl Write,
    stdout: &mut impl BufRead,
    id: u64,
    name: &str,
    arguments: Value,
) -> (bool, String) {
    let message = json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
    writeln!(stdin, "{message}").unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let answer: Value = serde_json::from_str(&line).unwrap();
    let result = &answer["result"];
    (
        result["isError"] == true,
        result["content"][0]["text"].as_str().unwrap().to_string(),
    )
}

#[test]
fn an_agent_starts_its_own_sandbox_through_mcp() {
    let place = Place::new("mcp");
    let mut server = Command::new(env!("CARGO_BIN_EXE_helios-mcp"))
        .env_remove("HELIOS_SOCKET")
        .env_remove("HELIOS_CONTROL_SOCKET")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = server.stdin.take().unwrap();
    let mut stdout = BufReader::new(server.stdout.take().unwrap());

    let (failed, text) = mcp_call(&mut stdin, &mut stdout, 1, "helios_state", json!({}));
    assert!(failed && text.contains("no node yet"), "{text}");
    let (failed, text) = mcp_call(
        &mut stdin,
        &mut stdout,
        2,
        "helios_sandbox_start",
        json!({ "dir": place.s() }),
    );
    assert!(!failed, "{text}");
    let (failed, text) = mcp_call(&mut stdin, &mut stdout, 3, "helios_wait_idle", json!({}));
    assert!(!failed, "{text}");
    if cfg!(debug_assertions) {
        let region = json!({ "x": 0, "y": 0, "width": 8, "height": 8 });
        let (failed, text) = mcp_call(
            &mut stdin,
            &mut stdout,
            4,
            "helios_render",
            json!({ "node": "exp", "region": region }),
        );
        assert!(!failed, "{text}");
    }
    let (failed, text) = mcp_call(
        &mut stdin,
        &mut stdout,
        5,
        "helios_sandbox_stop",
        json!({ "dir": place.s() }),
    );
    assert!(!failed && text.contains("true"), "{text}");
    drop(stdin);
    let _ = server.wait();
    assert!(Path::new(&place.0).join("sandbox.json").exists());
}
