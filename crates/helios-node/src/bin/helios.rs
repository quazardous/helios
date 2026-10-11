//! `helios`: the command line of Helios.
//!
//!     helios inspect [--socket PATH] state
//!     helios inspect [--socket PATH] describe ENTITY
//!     helios inspect [--socket PATH] events [--since SEQ]
//!     helios inspect [--socket PATH] wait-event KIND [--since SEQ] [--timeout MS]
//!     helios inspect [--socket PATH] wait-idle [--timeout MS]
//!
//!     helios control [--socket PATH] render NODE [--port PORT] --region X,Y,W,H
//!     helios control [--socket PATH] set-param NODE PARAM VALUE
//!     helios control [--socket PATH] cancel JOB
//!     helios control [--socket PATH] pause | resume
//!     helios control [--socket PATH] fault NODE compute|source-read
//!
//! The inspect socket defaults to `$HELIOS_SOCKET`, the control socket to
//! `$HELIOS_CONTROL_SOCKET`. Output is JSON. Exit codes: 0 answered, 1 the
//! node answered with an error (a wait that timed out prints what happened
//! instead), 2 wrong usage, 3 the node cannot be reached.

use std::path::PathBuf;
use std::process::ExitCode;

use helios_core::{FaultKind, Region, Value};
use helios_inspect::control::ControlRequest;
use helios_inspect::{Request, request};

const USAGE: &str = "usage:
  helios inspect [--socket PATH] state
  helios inspect [--socket PATH] describe ENTITY
  helios inspect [--socket PATH] events [--since SEQ]
  helios inspect [--socket PATH] wait-event KIND [--since SEQ] [--timeout MS]
  helios inspect [--socket PATH] wait-idle [--timeout MS]
  helios control [--socket PATH] render NODE [--port PORT] --region X,Y,W,H
  helios control [--socket PATH] set-param NODE PARAM VALUE
  helios control [--socket PATH] cancel JOB
  helios control [--socket PATH] pause | resume
  helios control [--socket PATH] fault NODE compute|source-read";

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

/// `X,Y,W,H`.
fn region(text: &str) -> Result<Region, String> {
    let parts: Vec<&str> = text.split(',').collect();
    let bad = || format!("--region takes X,Y,W,H, got `{text}`");
    if parts.len() != 4 {
        return Err(bad());
    }
    Ok(Region::new(
        parts[0].trim().parse().map_err(|_| bad())?,
        parts[1].trim().parse().map_err(|_| bad())?,
        parts[2].trim().parse().map_err(|_| bad())?,
        parts[3].trim().parse().map_err(|_| bad())?,
    ))
}

/// A parameter value typed on the command line: a number, true or false, or a choice.
fn value(text: &str) -> Value {
    match text {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => text
            .parse()
            .map(Value::Float)
            .unwrap_or_else(|_| Value::Choice(text.into())),
    }
}

/// A request, serialized for its endpoint.
struct Call {
    socket: PathBuf,
    body: serde_json::Value,
}

fn parse(args: Vec<String>) -> Result<Call, String> {
    let mut args = args.into_iter();
    let command = args.next().ok_or("no command")?;
    let (env, what) = match command.as_str() {
        "inspect" => ("HELIOS_SOCKET", "inspect"),
        "control" => ("HELIOS_CONTROL_SOCKET", "control"),
        other => return Err(format!("unknown command `{other}`")),
    };
    let mut socket = std::env::var_os(env).map(PathBuf::from);
    let mut words = Vec::new();
    let (mut since, mut timeout) = (0, 10_000);
    let (mut port, mut area) = ("output".to_string(), None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => socket = Some(PathBuf::from(args.next().ok_or("--socket needs a path")?)),
            "--since" => since = number(args.next(), "--since")?,
            "--timeout" => timeout = number(args.next(), "--timeout")?,
            "--port" => port = args.next().ok_or("--port needs a name")?,
            "--region" => area = Some(region(&args.next().ok_or("--region needs X,Y,W,H")?)?),
            _ => words.push(arg),
        }
    }
    let socket = socket.ok_or(format!("no socket: pass --socket PATH or set {env}"))?;
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let body = if what == "inspect" {
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
        serde_json::to_value(request)
    } else {
        let request = match words.as_slice() {
            ["render", node] => ControlRequest::Render {
                node: node.to_string(),
                port,
                region: area.ok_or("render needs --region X,Y,W,H")?,
            },
            ["set-param", node, param, v] => ControlRequest::SetParam {
                node: node.to_string(),
                param: param.to_string(),
                value: value(v),
            },
            ["cancel", job] => ControlRequest::Cancel {
                job: job
                    .parse()
                    .map_err(|_| format!("`{job}` is not a job id"))?,
            },
            ["pause"] => ControlRequest::Pause,
            ["resume"] => ControlRequest::Resume,
            ["fault", node, kind] => ControlRequest::Fault {
                node: node.to_string(),
                kind: match *kind {
                    "compute" => FaultKind::Compute,
                    "source-read" => FaultKind::SourceRead,
                    other => {
                        return Err(format!("unknown fault `{other}`: compute or source-read"));
                    }
                },
            },
            [] => return Err("control needs an action".into()),
            other => return Err(format!("unknown action `{}`", other.join(" "))),
        };
        serde_json::to_value(request)
    };
    Ok(Call {
        socket,
        body: body.expect("requests always serialize"),
    })
}

fn main() -> ExitCode {
    let call = match parse(std::env::args().skip(1).collect()) {
        Ok(c) => c,
        Err(e) => return usage(&e),
    };
    match request(&call.socket, &call.body) {
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
            eprintln!(
                "helios: cannot reach the node at {}: {e}",
                call.socket.display()
            );
            ExitCode::from(3)
        }
    }
}
