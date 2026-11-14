//! `helios-mcp`: an MCP server (stdio) over a running node's endpoints.
//!
//!     helios-mcp [--socket ADDRESS] [--control ADDRESS] [--token-file FILE]
//!                [--read-root DIR]... [--write-root DIR] [--tools full|compact]
//!
//! `--tools compact` offers only the generic pair over the command
//! registry (`helios_commands_list`, `helios_command_run`) and the sandbox
//! pair, for an agent with a short context; `full`, the default, every tool.
//!
//! With a root, the paths an agent gives a tool are confined: read under a
//! read root or the write root, written under the write root, refused
//! elsewhere (`helios_node::roots`). Without any, they are not confined.
//!
//! An address is a Unix socket path or `tcp://HOST:PORT` (with the node's
//! session token, from `--token-file` or `HELIOS_TOKEN`).
//!
//! The sockets default to `$HELIOS_SOCKET` and `$HELIOS_CONTROL_SOCKET`.
//! Started without any, it waits for `helios_sandbox_start`.
//! A request is at most 1 MiB, a wait 10 minutes and a batch 256 steps;
//! an answer's lists and text are cut to their caps with a "truncated"
//! note, an image over 2048 px or 5 MiB is left out with a note, and text from photos' metadata or
//! files is marked as untrusted data (`helios_node::caps`).
//! Tools are listed by `tools/list`; see `helios_node::mcp`. The
//! `helios_ai_*` tools need no node: they set up and tune the AI models of
//! this machine (`docs/ai-models.md`).

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use helios_inspect::Address;
use helios_node::caps::{Message, read_message, too_large};
use helios_node::mcp::{Sockets, ToolSet, handle};
use helios_node::roots::Roots;
use serde_json::{Value, json};

fn main() -> ExitCode {
    let mut inspect = std::env::var("HELIOS_SOCKET").ok();
    let mut control = std::env::var("HELIOS_CONTROL_SOCKET").ok();
    let mut token = std::env::var("HELIOS_TOKEN").ok();
    let mut read_roots = Vec::new();
    let mut write_root: Option<PathBuf> = None;
    let mut tools = ToolSet::Full;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--socket", Some(p)) => inspect = Some(p),
            ("--control", Some(p)) => control = Some(p),
            ("--read-root", Some(d)) => read_roots.push(PathBuf::from(d)),
            ("--write-root", Some(d)) if write_root.is_none() => {
                write_root = Some(PathBuf::from(d));
            }
            ("--tools", Some(set)) if ToolSet::parse(&set).is_some() => {
                tools = ToolSet::parse(&set).unwrap_or_default();
            }
            ("--token-file", Some(f)) => match std::fs::read_to_string(&f) {
                Ok(t) => token = Some(t.trim().to_string()),
                Err(e) => {
                    eprintln!("helios-mcp: cannot read {f}: {e}");
                    return ExitCode::from(2);
                }
            },
            _ => {
                eprintln!(
                    "usage: helios-mcp [--socket ADDRESS] [--control ADDRESS] [--token-file FILE] [--read-root DIR]... [--write-root DIR] [--tools full|compact]"
                );
                return ExitCode::from(2);
            }
        }
    }
    let roots = match Roots::new(&read_roots, write_root.as_deref()) {
        Ok(roots) => roots,
        Err(e) => {
            eprintln!("helios-mcp: {e}");
            return ExitCode::from(2);
        }
    };
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
        roots,
        tools,
    };
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout();
    // A request over the cap is refused whole, never parsed.
    while let Ok(Some(message)) = read_message(&mut stdin) {
        let line = match message {
            Message::Line(line) => line,
            Message::TooLarge(size) => {
                if writeln!(stdout, "{}", too_large(size)).is_err() || stdout.flush().is_err() {
                    break;
                }
                continue;
            }
        };
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
