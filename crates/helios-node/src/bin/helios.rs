//! `helios`: the command line of Helios.
//!
//!     helios inspect [--socket PATH|tcp://HOST:PORT --token-file FILE] state
//!     helios inspect [--socket PATH|tcp://HOST:PORT --token-file FILE] describe ENTITY
//!     helios inspect [--socket PATH|tcp://HOST:PORT --token-file FILE] events [--since SEQ]
//!     helios inspect [--socket PATH|tcp://HOST:PORT --token-file FILE] wait-event KIND [--since SEQ] [--timeout MS]
//!     helios inspect [--socket PATH|tcp://HOST:PORT --token-file FILE] wait-idle [--timeout MS]
//!
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] render NODE [--port PORT] --region X,Y,W,H
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] set-param NODE PARAM VALUE
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] load-source NODE PATH
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] export NODE --format jpeg|png|tiff
//!                    [--quality Q] [--port PORT] [--region X,Y,W,H] [--set NODE.PARAM=VALUE]... --to PATH
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] cancel JOB
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] pause | resume
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] fault NODE compute|source-read
//!
//!     helios sandbox new DIR [--network FILE] | start DIR | stop DIR | clean DIR
//!
//!     helios batch --network FILE --out DIR [--format jpeg|png|tiff] [--quality Q] [--jobs N]
//!                  [--name TEMPLATE] [--sidecar] [--source NAME] [--node NAME]
//!                  [--socket ADDR --control ADDR [--token-file FILE]] IMAGES...
//!
//! `batch` applies a network to many images, each exported to a file in
//! `--out` (named by `--name`, `{stem}.{ext}` by default; a name already
//! taken gets `-1`, `-2`...). It starts its own node in a temporary sandbox,
//! or drives the node at `--socket` and `--control`; `--sidecar` applies each
//! image's darktable XMP history on top of the network. It prints one JSON
//! line per image (`path`, `output`, `duration_ms`, `error`, `sidecar`) and
//! exits with 1 when an image failed (the others go on), 130 when
//! interrupted: Ctrl-C cancels the exports in flight and leaves no partial
//! file. See `docs/cli.md`.
//!
//! The inspect socket defaults to `$HELIOS_SOCKET`, the control socket to
//! `$HELIOS_CONTROL_SOCKET`. Output is JSON. Exit codes: 0 answered, 1 the
//! node answered with an error (a wait that timed out prints what happened
//! instead), 2 wrong usage, 3 the node cannot be reached.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use helios_core::{FaultKind, Params, Region, Value};
use helios_inspect::control::{ControlRequest, FileFormat};
use helios_inspect::{Address, Request, request_at};
use helios_node::batch::{Options, Outcome};

const USAGE: &str = "usage:
  helios inspect [--socket PATH|tcp://HOST:PORT --token-file FILE] state
  helios inspect [--socket PATH|tcp://HOST:PORT --token-file FILE] describe ENTITY
  helios inspect [--socket PATH|tcp://HOST:PORT --token-file FILE] events [--since SEQ]
  helios inspect [--socket PATH|tcp://HOST:PORT --token-file FILE] wait-event KIND [--since SEQ] [--timeout MS]
  helios inspect [--socket PATH|tcp://HOST:PORT --token-file FILE] wait-idle [--timeout MS]
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] render NODE [--port PORT] --region X,Y,W,H
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] set-param NODE PARAM VALUE
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] load-source NODE PATH
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] export NODE --format jpeg|png|tiff
                 [--quality Q] [--port PORT] [--region X,Y,W,H] [--set NODE.PARAM=VALUE]... --to PATH
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] cancel JOB
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] pause | resume
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] fault NODE compute|source-read
  helios sandbox new DIR [--network FILE] | start DIR | stop DIR | clean DIR
  helios batch --network FILE --out DIR [--format jpeg|png|tiff] [--quality Q] [--jobs N]
               [--name TEMPLATE] [--sidecar] [--source NAME] [--node NAME]
               [--socket ADDR --control ADDR [--token-file FILE]] IMAGES...";

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
            .map_or_else(|_| Value::Choice(text.into()), Value::Float),
    }
}

/// `jpeg`, `png` or `tiff`.
fn file_format(text: &str) -> Result<FileFormat, String> {
    match text {
        "jpeg" | "jpg" => Ok(FileFormat::Jpeg),
        "png" => Ok(FileFormat::Png),
        "tiff" | "tif" => Ok(FileFormat::Tiff),
        other => Err(format!("unknown format `{other}`: jpeg, png or tiff")),
    }
}

/// A path the node can use wherever it runs from.
fn absolute(path: &str) -> Result<PathBuf, String> {
    std::path::absolute(path).map_err(|e| format!("{path}: {e}"))
}

/// `NODE.PARAM=VALUE`: the node name may hold dots, the parameter not.
fn setting(text: &str) -> Result<(String, String, String), String> {
    let bad = || format!("--set takes NODE.PARAM=VALUE, got `{text}`");
    let (target, v) = text.split_once('=').ok_or_else(bad)?;
    let (node, param) = target.rsplit_once('.').ok_or_else(bad)?;
    if node.is_empty() || param.is_empty() {
        return Err(bad());
    }
    Ok((node.into(), param.into(), v.into()))
}

/// A request, serialized for its endpoint.
struct Call {
    socket: Address,
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
    let mut socket = std::env::var(env).ok();
    let mut token = std::env::var("HELIOS_TOKEN").ok();
    let mut words = Vec::new();
    let (mut since, mut timeout) = (0, 10_000);
    let (mut port, mut area) = ("output".to_string(), None);
    let (mut format, mut quality, mut to) = (None, None, None);
    let mut sets: BTreeMap<String, Params> = BTreeMap::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => {
                socket = Some(
                    args.next()
                        .ok_or("--socket needs a path or tcp:// address")?,
                );
            }
            "--token-file" => {
                let file = args.next().ok_or("--token-file needs a path")?;
                token = Some(
                    std::fs::read_to_string(&file)
                        .map_err(|e| format!("cannot read {file}: {e}"))?
                        .trim()
                        .to_string(),
                );
            }
            "--since" => since = number(args.next(), "--since")?,
            "--timeout" => timeout = number(args.next(), "--timeout")?,
            "--port" => port = args.next().ok_or("--port needs a name")?,
            "--format" => {
                format = Some(file_format(
                    &args.next().ok_or("--format needs jpeg, png or tiff")?,
                )?);
            }
            "--quality" => {
                let q = number(args.next(), "--quality")?;
                quality = Some(
                    u8::try_from(q)
                        .ok()
                        .filter(|q| (1..=100).contains(q))
                        .ok_or("--quality takes 1 to 100")?,
                );
            }
            "--to" => to = Some(absolute(&args.next().ok_or("--to needs a path")?)?),
            "--set" => {
                let (node, param, v) =
                    setting(&args.next().ok_or("--set needs NODE.PARAM=VALUE")?)?;
                sets.entry(node).or_default().set(&param, value(&v));
            }
            "--region" => area = Some(region(&args.next().ok_or("--region needs X,Y,W,H")?)?),
            _ => words.push(arg),
        }
    }
    let socket = socket.ok_or(format!("no socket: pass --socket PATH or set {env}"))?;
    let socket = Address::parse(&socket, token)?;
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
            ["load-source", node, path] => ControlRequest::LoadSource {
                node: node.to_string(),
                path: absolute(path)?,
            },
            ["export", node] => ControlRequest::Export {
                node: node.to_string(),
                port,
                region: area,
                path: to.ok_or("export needs --to PATH")?,
                format: format.ok_or("export needs --format jpeg|png|tiff")?,
                quality,
                params: sets,
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
            ["decisions"] => ControlRequest::Decisions,
            ["decision-resolve"] => ControlRequest::DecisionResolve,
            ["decision-accept", node] => ControlRequest::DecisionAccept {
                node: node.to_string(),
                option: 0,
                who: "user".into(),
            },
            ["decision-accept", node, option] => ControlRequest::DecisionAccept {
                node: node.to_string(),
                option: option
                    .parse()
                    .map_err(|_| format!("`{option}` is not an option number"))?,
                who: "user".into(),
            },
            ["decision-decide", node, param, v] => ControlRequest::DecisionDecide {
                node: node.to_string(),
                values: helios_core::Params::new().with(param, value(v)),
                who: "user".into(),
            },
            ["decision-reask", node] => ControlRequest::DecisionReask {
                node: node.to_string(),
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

/// The `helios-node` executable next to this one.
fn node_bin() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(|d| d.join(format!("helios-node{}", std::env::consts::EXE_SUFFIX)))
        })
        .unwrap_or_else(|| PathBuf::from("helios-node"))
}

/// The options of `helios batch`.
fn batch_options(args: &[String]) -> Result<Options, String> {
    let mut args = args.iter().cloned();
    let mut options = Options {
        network: None,
        out: PathBuf::new(),
        format: FileFormat::Jpeg,
        quality: 92,
        jobs: 2,
        name: "{stem}.{ext}".into(),
        sidecar: false,
        source: None,
        node: None,
        attach: None,
        images: Vec::new(),
    };
    let (mut out, mut socket, mut control) = (None, None, None);
    let mut token = std::env::var("HELIOS_TOKEN").ok();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--network" => options.network = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--format" => options.format = file_format(&value()?)?,
            "--quality" => {
                options.quality = u8::try_from(number(Some(value()?), "--quality")?)
                    .ok()
                    .filter(|q| (1..=100).contains(q))
                    .ok_or("--quality takes 1 to 100")?;
            }
            "--jobs" => {
                options.jobs = usize::try_from(number(Some(value()?), "--jobs")?)
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--jobs takes a number above 0")?;
            }
            "--name" => options.name = value()?,
            "--sidecar" => options.sidecar = true,
            "--source" => options.source = Some(value()?),
            "--node" => options.node = Some(value()?),
            "--socket" => socket = Some(value()?),
            "--control" => control = Some(value()?),
            "--token-file" => {
                let file = value()?;
                token = Some(
                    std::fs::read_to_string(&file)
                        .map_err(|e| format!("cannot read {file}: {e}"))?
                        .trim()
                        .to_string(),
                );
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option `{flag}`")),
            _ => options.images.push(PathBuf::from(arg)),
        }
    }
    options.out = out.ok_or("batch needs --out DIR")?;
    options.attach = match (socket, control) {
        (Some(s), Some(c)) => Some((
            Address::parse(&s, token.clone())?,
            Address::parse(&c, token)?,
        )),
        (None, None) => None,
        _ => return Err("attaching to a node needs both --socket and --control".into()),
    };
    if options.attach.is_none() && options.network.is_none() {
        return Err("batch needs --network FILE".into());
    }
    if options.images.is_empty() {
        return Err("batch needs images".into());
    }
    Ok(options)
}

/// `helios batch`: see the module documentation.
fn batch(args: &[String]) -> ExitCode {
    let options = match batch_options(args) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let interrupted = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        if let Err(e) = signal_hook::flag::register(signal, interrupted.clone()) {
            eprintln!("helios: cannot watch for interrupts: {e}");
            return ExitCode::from(1);
        }
    }
    let mut out = std::io::stdout();
    let emit = |line: &serde_json::Value| {
        // A closed stdout must not stop the batch.
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };
    match helios_node::batch::run(&options, &node_bin(), &interrupted, emit) {
        Ok(Outcome::Done) => ExitCode::SUCCESS,
        Ok(Outcome::Failed) => ExitCode::from(1),
        Ok(Outcome::Interrupted) => ExitCode::from(130),
        Err(e) => {
            eprintln!("helios: {e}");
            ExitCode::from(if e.starts_with("cannot reach") { 3 } else { 1 })
        }
    }
}

/// `helios sandbox new DIR [--network FILE] | start DIR | stop DIR | clean DIR`.
fn sandbox(args: &[String]) -> ExitCode {
    use helios_node::sandbox;

    let node_bin = node_bin();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        ["new", dir] => sandbox::create(dir.as_ref(), None)
            .map(|s| serde_json::to_value(s).expect("state serializes")),
        ["new", dir, "--network", file] => std::fs::read_to_string(file)
            .map_err(|e| format!("cannot read {file}: {e}"))
            .and_then(|network| sandbox::create(dir.as_ref(), Some(&network)))
            .map(|s| serde_json::to_value(s).expect("state serializes")),
        ["start", dir] => sandbox::start(dir.as_ref(), &node_bin)
            .map(|s| serde_json::to_value(s).expect("state serializes")),
        ["stop", dir] => {
            sandbox::stop(dir.as_ref()).map(|ran| serde_json::json!({ "stopped": ran }))
        }
        ["clean", dir] => {
            sandbox::clean(dir.as_ref()).map(|()| serde_json::json!({ "removed": dir }))
        }
        _ => {
            return usage(
                "sandbox takes: new DIR [--network FILE] | start DIR | stop DIR | clean DIR",
            );
        }
    };
    match result {
        Ok(answer) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&answer).expect("JSON values serialize")
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("helios: {e}");
            ExitCode::from(1)
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "sandbox") {
        return sandbox(&args[1..]);
    }
    if args.first().is_some_and(|a| a == "batch") {
        return batch(&args[1..]);
    }
    let call = match parse(args) {
        Ok(c) => c,
        Err(e) => return usage(&e),
    };
    match request_at(&call.socket, &call.body) {
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
            eprintln!("helios: cannot reach the node at {}: {e}", call.socket);
            ExitCode::from(3)
        }
    }
}
