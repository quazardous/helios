//! A node served over loopback TCP: the CLI and the MCP server reach it with
//! the session token; without it, nothing gets in.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const NETWORK: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 16, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }],
  "renders": [{ "node": "exp", "port": "output", "region": { "x": 0, "y": 0, "width": 16, "height": 8 } }]
}"#;

fn helios(args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_helios"))
        .env_remove("HELIOS_TOKEN")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
    )
}

#[test]
fn a_node_over_tcp_is_reached_with_its_token_only() {
    let dir = std::env::temp_dir().join(format!("helios-tcp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("network.json"), NETWORK).unwrap();
    let token_file = dir.join("token");
    let mut command = Command::new(env!("CARGO_BIN_EXE_helios-node"));
    command
        .arg("--graph")
        .arg(dir.join("network.json"))
        .args(["--socket", "tcp://127.0.0.1:0"])
        .arg("--token-file")
        .arg(&token_file)
        .stdout(Stdio::piped());
    if cfg!(debug_assertions) {
        command
            .args(["--control", "tcp://127.0.0.1:0"])
            .env("HELIOS_DEBUG_CONTROL", "1");
    }
    let mut node = command.spawn().unwrap();
    let mut out = BufReader::new(node.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    let inspect = line.trim().strip_prefix("ready ").unwrap().to_string();
    assert!(inspect.starts_with("tcp://127.0.0.1:"), "{inspect}");
    let tf = token_file.to_str().unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&token_file).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "the token is readable by its user only"
        );
    }

    assert_eq!(
        helios(&[
            "inspect",
            "--socket",
            &inspect,
            "--token-file",
            tf,
            "wait-idle"
        ])
        .0,
        0
    );
    let (_, node_state) = helios(&[
        "inspect",
        "--socket",
        &inspect,
        "--token-file",
        tf,
        "describe",
        "node",
    ]);
    assert!(
        node_state["describe"]["renders"][0]["hash"].is_string(),
        "{node_state}"
    );

    let wrong = dir.join("wrong");
    std::fs::write(&wrong, "not-the-token").unwrap();
    let (code, refused) = helios(&[
        "inspect",
        "--socket",
        &inspect,
        "--token-file",
        wrong.to_str().unwrap(),
        "state",
    ]);
    assert_eq!(code, 1);
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("authentication failed")
    );
    let no_token = Command::new(env!("CARGO_BIN_EXE_helios"))
        .env_remove("HELIOS_TOKEN")
        .args(["inspect", "--socket", &inspect, "state"])
        .output()
        .unwrap();
    assert_eq!(
        no_token.status.code(),
        Some(2),
        "no token: a usage error that says so"
    );

    if cfg!(debug_assertions) {
        line.clear();
        out_line(&mut out, &mut line);
        let control = line.trim().strip_prefix("control ").unwrap().to_string();
        let (code, rendered) = helios(&[
            "control",
            "--socket",
            &control,
            "--token-file",
            tf,
            "render",
            "exp",
            "--region",
            "0,0,8,8",
        ]);
        assert_eq!(code, 0, "{rendered}");

        // The MCP server reaches the same node over TCP.
        let mut server = Command::new(env!("CARGO_BIN_EXE_helios-mcp"))
            .args([
                "--socket",
                &inspect,
                "--control",
                &control,
                "--token-file",
                tf,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = server.stdin.take().unwrap();
        let mut stdout = BufReader::new(server.stdout.take().unwrap());
        let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "helios_wait_idle", "arguments": {} } });
        writeln!(stdin, "{call}").unwrap();
        let mut answer = String::new();
        stdout.read_line(&mut answer).unwrap();
        let answer: Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(answer["result"]["isError"], false, "{answer}");
        drop(stdin);
        let _ = server.wait();
    }

    let _ = node.kill();
    let _ = node.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

fn out_line(out: &mut impl BufRead, line: &mut String) {
    out.read_line(line).unwrap();
}

#[test]
fn tcp_without_a_token_file_is_refused() {
    let dir = std::env::temp_dir().join(format!("helios-tcp-nofile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("network.json"), NETWORK).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_helios-node"))
        .arg("--graph")
        .arg(dir.join("network.json"))
        .args(["--socket", "tcp://127.0.0.1:0"])
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("needs --token-file"));
}
