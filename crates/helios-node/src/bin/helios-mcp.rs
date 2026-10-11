//! `helios-mcp`: an MCP server (stdio) over a running node's endpoints.
//!
//!     helios-mcp --socket /path/to/node.sock [--control /path/to/control.sock]
//!
//! The sockets default to `$HELIOS_SOCKET` and `$HELIOS_CONTROL_SOCKET`.
//! Tools are listed by `tools/list`; see `helios_node::mcp`.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use helios_node::mcp::{Sockets, handle};
use serde_json::{Value, json};

fn main() -> ExitCode {
    let mut inspect = std::env::var_os("HELIOS_SOCKET").map(PathBuf::from);
    let mut control = std::env::var_os("HELIOS_CONTROL_SOCKET").map(PathBuf::from);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--socket", Some(p)) => inspect = Some(PathBuf::from(p)),
            ("--control", Some(p)) => control = Some(PathBuf::from(p)),
            _ => {
                eprintln!("usage: helios-mcp --socket PATH [--control PATH]");
                return ExitCode::from(2);
            }
        }
    }
    let Some(inspect) = inspect else {
        eprintln!("helios-mcp: no socket: pass --socket PATH or set HELIOS_SOCKET");
        return ExitCode::from(2);
    };
    let sockets = Sockets { inspect, control };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&sockets, &message),
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
