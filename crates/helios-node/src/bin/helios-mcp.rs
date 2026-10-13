//! `helios-mcp`: an MCP server (stdio) over a running node's endpoints.
//!
//!     helios-mcp [--socket ADDRESS] [--control ADDRESS] [--token-file FILE]
//!
//! An address is a Unix socket path or `tcp://HOST:PORT` (with the node's
//! session token, from `--token-file` or `HELIOS_TOKEN`).
//!
//! The sockets default to `$HELIOS_SOCKET` and `$HELIOS_CONTROL_SOCKET`.
//! Started without any, it waits for `helios_sandbox_start`.
//! Tools are listed by `tools/list`; see `helios_node::mcp`. The
//! `helios_ai_*` tools need no node: they set up and tune the AI models of
//! this machine (`docs/ai-models.md`).

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use helios_inspect::Address;
use helios_node::mcp::{Sockets, handle};
use serde_json::{Value, json};

fn main() -> ExitCode {
    let mut inspect = std::env::var("HELIOS_SOCKET").ok();
    let mut control = std::env::var("HELIOS_CONTROL_SOCKET").ok();
    let mut token = std::env::var("HELIOS_TOKEN").ok();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--socket", Some(p)) => inspect = Some(p),
            ("--control", Some(p)) => control = Some(p),
            ("--token-file", Some(f)) => match std::fs::read_to_string(&f) {
                Ok(t) => token = Some(t.trim().to_string()),
                Err(e) => {
                    eprintln!("helios-mcp: cannot read {f}: {e}");
                    return ExitCode::from(2);
                }
            },
            _ => {
                eprintln!(
                    "usage: helios-mcp [--socket ADDRESS] [--control ADDRESS] [--token-file FILE]"
                );
                return ExitCode::from(2);
            }
        }
    }
    let parse = |text: Option<String>| text.map(|t| Address::parse(&t, token.clone())).transpose();
    let (inspect, control) = match (parse(inspect), parse(control)) {
        (Ok(i), Ok(c)) => (i, c),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("helios-mcp: {e}");
            return ExitCode::from(2);
        }
    };
    // Without a socket, the server starts empty: `helios_sandbox_start`
    // starts a node and points it there.
    let node_bin = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(|d| d.join(format!("helios-node{}", std::env::consts::EXE_SUFFIX)))
        })
        .unwrap_or_else(|| PathBuf::from("helios-node"));
    let mut sockets = Sockets {
        inspect,
        control,
        node_bin,
        ai: None,
    };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&mut sockets, &message),
            Err(e) => Some(json!({
                "jsonrpc": "2.0", "id": null,
                "error": { "code": -32700, "message": format!("parse error: {e}") },
            })),
        };
        if let Some(answer) = answer
            && (writeln!(stdout, "{answer}").is_err() || stdout.flush().is_err())
        {
            break;
        }
    }
    ExitCode::SUCCESS
}
