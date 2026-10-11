//! `helios`: the command line of Helios.
//!
//!     helios inspect [--socket PATH] state
//!     helios inspect [--socket PATH] describe ENTITY
//!     helios inspect [--socket PATH] events [--since SEQ]
//!     helios inspect [--socket PATH] wait-event KIND [--since SEQ] [--timeout MS]
//!     helios inspect [--socket PATH] wait-idle [--timeout MS]
//!
//! The socket defaults to `$HELIOS_SOCKET`. Output is JSON. Exit codes:
//! 0 answered, 1 the node answered with an error (a wait that timed out
//! prints what happened instead), 2 wrong usage, 3 the node cannot be reached.

use std::path::PathBuf;
use std::process::ExitCode;

use helios_inspect::{Request, request};

const USAGE: &str = "usage:
  helios inspect [--socket PATH] state
  helios inspect [--socket PATH] describe ENTITY
  helios inspect [--socket PATH] events [--since SEQ]
  helios inspect [--socket PATH] wait-event KIND [--since SEQ] [--timeout MS]
  helios inspect [--socket PATH] wait-idle [--timeout MS]";

fn usage(error: &str) -> ExitCode {
    eprintln!("helios: {error}\n{USAGE}");
    ExitCode::from(2)
}

fn number(value: Option<String>, flag: &str) -> Result<u64, String> {
    value
        .ok_or(format!("{flag} needs a value"))?
        .parse()
        .map_err(|_| format!("{flag} needs a number"))
}

fn parse(args: Vec<String>) -> Result<(PathBuf, Request), String> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("inspect") => {}
        Some(other) => return Err(format!("unknown command `{other}`")),
        None => return Err("no command".into()),
    }
    let mut socket = std::env::var_os("HELIOS_SOCKET").map(PathBuf::from);
    let mut words = Vec::new();
    let (mut since, mut timeout) = (0, 10_000);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => socket = Some(PathBuf::from(args.next().ok_or("--socket needs a path")?)),
            "--since" => since = number(args.next(), "--since")?,
            "--timeout" => timeout = number(args.next(), "--timeout")?,
            _ => words.push(arg),
        }
    }
    let socket = socket.ok_or("no socket: pass --socket PATH or set HELIOS_SOCKET")?;
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let request = match words.as_slice() {
        ["state"] => Request::State,
        ["describe", entity] => Request::Describe {
            entity: entity.to_string(),
        },
        ["events"] => Request::Events { since },
        ["wait-event", kind] => Request::WaitEvent {
            kind: kind.to_string(),
            since,
            timeout_ms: timeout,
        },
        ["wait-idle"] => Request::WaitIdle {
            timeout_ms: timeout,
        },
        [] => return Err("inspect needs a request".into()),
        other => return Err(format!("unknown request `{}`", other.join(" "))),
    };
    Ok((socket, request))
}

fn main() -> ExitCode {
    let (socket, req) = match parse(std::env::args().skip(1).collect()) {
        Ok(p) => p,
        Err(e) => return usage(&e),
    };
    match request(&socket, &req) {
        Ok(answer) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&answer).expect("JSON values serialize")
            );
            if answer["ok"] == true {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("helios: cannot reach the node at {}: {e}", socket.display());
            ExitCode::from(3)
        }
    }
}
