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
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] bypass NODE on|off
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] load-source NODE PATH
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] export NODE --format jpeg|png|tiff
//!                    [--quality Q] [--port PORT] [--region X,Y,W,H] [--set NODE.PARAM=VALUE]... --to PATH
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] cancel JOB
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] job-cancel ID
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] pause | resume
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] fault NODE compute|source-read
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] workflow-submit --format jpeg|png|tiff
//!                    [--quality Q] [--again] [--jobs N] --to DIR IMAGES...
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] workflow-runs | workflow-items RUN [NODE] | workflow-forget RUN
//!                    | workflow-cancel RUN [ITEMS...] [--node NODE] | workflow-retry RUN [ITEMS...] [--node NODE]
//!                    | workflow-prioritize RUN top|bottom ITEMS... | workflow-pause RUN | workflow-resume RUN
//!
//!     helios sandbox new DIR [--network FILE] | start DIR | stop DIR | clean DIR
//!
//!     helios batch --network FILE --out DIR [--format jpeg|png|tiff] [--quality Q] [--jobs N]
//!                  [--name TEMPLATE] [--sidecar] [--vault FILE] [--again] [--source NAME]
//!                  [--node NAME] [--socket ADDR --control ADDR [--token-file FILE]] IMAGES...
//!
//!     helios import-darktable --vault FILE FOLDERS-OR-IMAGES...
//!
//!     helios import --vault FILE [--reference | --copy [--into DIR]] [--rename PATTERN]
//!                   [--duplicates skip|keep|ask] [--no-signals] FOLDERS-CARDS-OR-FILES...
//!     helios stacks --vault FILE list | create IMAGE... [--top IMAGE] | dissolve STACK
//!                   | expand STACK | collapse STACK | top STACK IMAGE
//!                   | suggest [IMAGE...] [--resolver NAME] [--window S] [--distance N]
//!                   | accept DECISION [--groups N,N...] | decline DECISION | decisions
//!
//!     helios doctor display
//!     helios doctor ai
//!
//! `batch` applies a network to many images, each exported to a file in
//! `--out` (named by `--name`, `{stem}.{ext}` by default; a name already
//! taken gets `-1`, `-2`...). It submits them as a run of the workflow
//! level to its own node, started in a temporary sandbox, or to the node at
//! `--socket` and `--control`, and follows the run. With `--vault`, its own
//! node keeps the run in that vault: the same batch run again extends it
//! and writes nothing twice (`--again` starts a new run). `--sidecar`
//! applies each image's darktable XMP history on top of the network, and
//! with `--vault` keeps it there as a version with origin `import:xmp`,
//! with the sidecar's rating when the vault has none. It prints one JSON
//! line per image (`path`, `output`, `duration_ms`, `error`, `sidecar`) and
//! exits with 1 when an image failed (the others go on), 130 when
//! interrupted: Ctrl-C cancels the run and leaves no partial file, or,
//! with `--vault` and its own node, stops it to be finished by the same
//! batch run again. See `docs/cli.md`.
//!
//! `doctor display` says what the display offers the desktop: the session
//! (Wayland, X11 or none), on Wayland what the compositor's colour
//! management (`wp_color_management_v1`) supports, and what the viewport
//! draws there with the Wide-gamut preview setting on, and why. It asks
//! the compositor only; the running desktop's debug bridge says what its
//! window actually resolved.
//!
//! `doctor ai` reports the AI models of this machine as JSON: each
//! provider (local or remote, reachable, where its key is), the routing
//! map with why a capability or a prompt is unavailable, and the local
//! runtime's health (see `docs/ai-models.md`).
//!
//! `import-darktable` keeps a folder's darktable edits in a vault: every
//! image file, and a version per sidecar, duplicates (`IMG_0001_01.CR2.xmp`)
//! included; sidecars whose image is missing are listed as orphans. One
//! JSON line per image and per orphan; exit 1 when a sidecar could not be
//! kept.
//!
//! `import` brings image files into a vault as a library operation (see
//! `helios_library::import`): referenced in place, or copied into the
//! library folder and renamed by a pattern, their sidecars along; exact
//! duplicates skipped, kept or listed for an answer; near-duplicate
//! signals measured. One JSON line per file, then the report; exit 1
//! when a file failed, 130 when interrupted (Ctrl-C stops it before its
//! next file, a copy under way removed). `stacks` lists, makes and dissolves stacks, and
//! asks for stack suggestions (`helios_library::stacks`), which apply
//! only once accepted. IMAGE is an image's id or a file the vault knows.
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
use helios_inspect::control::{ControlRequest, FileFormat, MoveTo, RunItem};
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
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] bypass NODE on|off
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] load-source NODE PATH
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] export NODE --format jpeg|png|tiff
                 [--quality Q] [--port PORT] [--region X,Y,W,H] [--set NODE.PARAM=VALUE]... --to PATH
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] cancel JOB
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] job-cancel ID
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] pause | resume
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] fault NODE compute|source-read
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] workflow-submit --format jpeg|png|tiff
                 [--quality Q] [--again] [--jobs N] --to DIR IMAGES...
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] workflow-runs | workflow-items RUN [NODE] | workflow-forget RUN
                 | workflow-cancel RUN [ITEMS...] [--node NODE] | workflow-retry RUN [ITEMS...] [--node NODE]
                 | workflow-prioritize RUN top|bottom ITEMS... | workflow-pause RUN | workflow-resume RUN
  helios sandbox new DIR [--network FILE] | start DIR | stop DIR | clean DIR
  helios batch --network FILE --out DIR [--format jpeg|png|tiff] [--quality Q] [--jobs N]
               [--name TEMPLATE] [--sidecar] [--vault FILE] [--again] [--source NAME]
               [--node NAME] [--socket ADDR --control ADDR [--token-file FILE]] IMAGES...
  helios import-darktable --vault FILE FOLDERS-OR-IMAGES...
  helios import --vault FILE [--reference | --copy [--into DIR]] [--rename PATTERN]
                [--duplicates skip|keep|ask] [--no-signals] FOLDERS-CARDS-OR-FILES...
  helios stacks --vault FILE list | create IMAGE... [--top IMAGE] | dissolve STACK
                | expand STACK | collapse STACK | top STACK IMAGE
                | suggest [IMAGE...] [--resolver NAME] [--window S] [--distance N]
                | accept DECISION [--groups N,N...] | decline DECISION | decisions
  helios doctor display
  helios doctor ai";

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

/// The items a `workflow-*` verb names: ids (`path:…`) as given, files
/// made absolute; none for every item of the run.
fn run_items(words: &[&str]) -> Result<Option<Vec<String>>, String> {
    if words.is_empty() {
        return Ok(None);
    }
    words
        .iter()
        .map(|w| {
            if w.starts_with("path:") {
                Ok((*w).to_string())
            } else {
                absolute(w).map(|p| p.to_string_lossy().into_owned())
            }
        })
        .collect::<Result<_, _>>()
        .map(Some)
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
    let mut again = false;
    let mut at = None;
    let mut jobs = None;
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
            "--again" => again = true,
            "--node" => at = Some(args.next().ok_or("--node needs a node name")?),
            "--jobs" => {
                jobs = Some(usize::try_from(number(args.next(), "--jobs")?).unwrap_or(usize::MAX));
            }
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
                batch: None,
                as_mask: false,
            },
            ["set-param", node, param, v] => ControlRequest::SetParam {
                node: node.to_string(),
                param: param.to_string(),
                value: value(v),
            },
            ["bypass", node, on] => ControlRequest::NodeBypass {
                node: node.to_string(),
                on: match *on {
                    "on" => true,
                    "off" => false,
                    other => return Err(format!("bypass takes on or off, not `{other}`")),
                },
            },
            ["cancel", job] => ControlRequest::Cancel {
                job: job
                    .parse()
                    .map_err(|_| format!("`{job}` is not a job id"))?,
            },
            ["job-cancel", id] => ControlRequest::JobCancel { id: id.to_string() },
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
            ["workflow-submit", images @ ..] if !images.is_empty() => {
                ControlRequest::WorkflowSubmit {
                    title: None,
                    items: images
                        .iter()
                        .map(|i| absolute(i).map(RunItem::Path))
                        .collect::<Result<_, _>>()?,
                    out: to.ok_or("workflow-submit needs --to DIR")?,
                    format: format.ok_or("workflow-submit needs --format jpeg|png|tiff")?,
                    quality,
                    name: "{stem}.{ext}".into(),
                    node: None,
                    source: None,
                    again,
                    concurrency: jobs,
                }
            }
            ["workflow-runs"] => ControlRequest::WorkflowRuns,
            ["workflow-items", run] => ControlRequest::WorkflowItems {
                run: run.to_string(),
                node: None,
                state: None,
                offset: 0,
                limit: 100,
            },
            ["workflow-items", run, node] => ControlRequest::WorkflowItems {
                run: run.to_string(),
                node: Some(node.to_string()),
                state: None,
                offset: 0,
                limit: 100,
            },
            ["workflow-forget", run] => ControlRequest::WorkflowForget {
                run: run.to_string(),
            },
            ["workflow-cancel", run, items @ ..] => ControlRequest::WorkflowCancel {
                run: run.to_string(),
                items: run_items(items)?,
                node: at,
            },
            ["workflow-retry", run, items @ ..] => ControlRequest::WorkflowRetry {
                run: run.to_string(),
                items: run_items(items)?,
                node: at,
            },
            ["workflow-prioritize", run, to, items @ ..] if !items.is_empty() => {
                ControlRequest::WorkflowPrioritize {
                    run: run.to_string(),
                    items: run_items(items)?.unwrap_or_default(),
                    to: match *to {
                        "top" => MoveTo::Top,
                        "bottom" => MoveTo::Bottom,
                        other => {
                            return Err(format!(
                                "workflow-prioritize takes top or bottom, not `{other}`"
                            ));
                        }
                    },
                }
            }
            ["workflow-pause", run] => ControlRequest::WorkflowPause {
                run: run.to_string(),
            },
            ["workflow-resume", run] => ControlRequest::WorkflowResume {
                run: run.to_string(),
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
        vault: None,
        source: None,
        node: None,
        attach: None,
        again: false,
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
            "--again" => options.again = true,
            "--vault" => options.vault = Some(PathBuf::from(value()?)),
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
            ExitCode::from(if e.key == helios_node::batch::UNREACHABLE {
                3
            } else {
                1
            })
        }
    }
}

/// `helios import-darktable --vault FILE PATH...`: see `helios_node::darktable::import`.
fn import_darktable(args: &[String]) -> ExitCode {
    let mut vault = None;
    let mut paths = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--vault" => match args.next() {
                Some(v) => vault = Some(PathBuf::from(v)),
                None => return usage("--vault needs a value"),
            },
            flag if flag.starts_with("--") => return usage(&format!("unknown option `{flag}`")),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    let Some(vault) = vault else {
        return usage("import-darktable needs --vault FILE");
    };
    if paths.is_empty() {
        return usage("import-darktable needs folders or images");
    }
    let store = match helios_vault_sqlite::SqliteStore::open(&vault) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("helios: {}: {e}", vault.display());
            return ExitCode::from(1);
        }
    };
    let vault = helios_vault::Vault::new(store);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut out = std::io::stdout();
    let mut emit = |line: &serde_json::Value| {
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };
    let mut warn = |path: &std::path::Path, warning: &str| {
        eprintln!("helios import-darktable: {}: {warning}", path.display());
    };
    match helios_node::darktable::import(&vault, &paths, now, &mut emit, &mut warn) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("helios: {e}");
            ExitCode::from(1)
        }
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

type Library = helios_vault::Vault<helios_vault_sqlite::SqliteStore>;

fn open_vault(path: &std::path::Path) -> Result<Library, String> {
    helios_vault_sqlite::SqliteStore::open(path)
        .map(helios_vault::Vault::new)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn to_json<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).expect("library values serialize")
}

/// The options of `helios import`: the vault, the sources, how.
fn import_options(
    args: &[String],
) -> Result<(PathBuf, Vec<PathBuf>, helios_library::Options), String> {
    use helios_library::import::{Duplicates, Placement};
    let mut vault = None;
    let mut sources = Vec::new();
    let (mut copy, mut into) = (false, None);
    let mut options = helios_library::Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--vault" => vault = Some(PathBuf::from(value()?)),
            "--copy" => copy = true,
            "--reference" => copy = false,
            "--into" => {
                into = Some(PathBuf::from(value()?));
                copy = true;
            }
            "--rename" => {
                options.rename = Some(helios_library::rename::Pattern::parse(value()?)?);
            }
            "--duplicates" => options.duplicates = Duplicates::parse(value()?)?,
            "--no-signals" => options.signals = false,
            flag if flag.starts_with("--") => return Err(format!("unknown option `{flag}`")),
            _ => sources.push(PathBuf::from(arg)),
        }
    }
    if sources.is_empty() {
        return Err("import needs folders, cards or files".into());
    }
    if copy {
        options.placement = Placement::Copy { into };
    }
    Ok((vault.ok_or("import needs --vault FILE")?, sources, options))
}

/// `helios import`: see `helios_library::import`.
fn import(args: &[String]) -> ExitCode {
    let (vault, sources, mut options) = match import_options(args) {
        Ok(p) => p,
        Err(e) => return usage(&e),
    };
    let vault = match open_vault(&vault) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("helios: {e}");
            return ExitCode::from(1);
        }
    };
    // Ctrl-C stops the import before its next file, a copy under way
    // removed; the files done stay imported.
    let interrupted = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        if let Err(e) = signal_hook::flag::register(signal, interrupted.clone()) {
            eprintln!("helios: cannot watch for interrupts: {e}");
        }
    }
    options.cancel = Some(interrupted);
    let jobs = helios_core::jobs::Jobs::new(Arc::default());
    let mut out = std::io::stdout();
    let mut emit = |line: &serde_json::Value| {
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };
    match helios_library::import::import(&vault, &jobs, &sources, &options, now(), &mut emit) {
        Ok(report) => {
            emit(&serde_json::json!({ "report": report }));
            if report.cancelled {
                ExitCode::from(130)
            } else if report.failed == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("helios: {e}");
            ExitCode::from(1)
        }
    }
}

/// An image named on the command line: its id, or a file the vault knows.
fn image_named(vault: &Library, text: &str) -> Result<helios_vault::ImageId, String> {
    if let Ok(id) = helios_vault::ImageId::from_hex(text) {
        return Ok(id);
    }
    let path = std::path::absolute(text).map_err(|e| format!("{text}: {e}"))?;
    vault
        .at_path(&path)?
        .map(|(id, _)| id)
        .ok_or_else(|| format!("`{text}` is neither an image id nor a file the vault knows"))
}

/// The options of `helios stacks`.
#[derive(Default)]
struct StacksArgs {
    vault: Option<PathBuf>,
    words: Vec<String>,
    top: Option<String>,
    resolver: Option<String>,
    groups: Option<Vec<usize>>,
    /// Thresholds given for the rules, if any.
    rules: Option<helios_library::stacks::Rules>,
}

fn stacks_args(args: &[String]) -> Result<StacksArgs, String> {
    let mut parsed = StacksArgs::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--vault" => parsed.vault = Some(PathBuf::from(value()?)),
            "--top" => parsed.top = Some(value()?.clone()),
            "--resolver" => parsed.resolver = Some(value()?.clone()),
            "--window" => {
                parsed.rules.get_or_insert_with(Default::default).window = value()?
                    .parse()
                    .map_err(|_| "--window takes seconds".to_string())?;
            }
            "--distance" => {
                parsed.rules.get_or_insert_with(Default::default).distance = value()?
                    .parse()
                    .map_err(|_| "--distance takes a number of bits".to_string())?;
            }
            "--groups" => {
                parsed.groups = Some(
                    value()?
                        .split(',')
                        .map(|g| g.trim().parse::<usize>())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| "--groups takes numbers: 0,2".to_string())?,
                );
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option `{flag}`")),
            _ => parsed.words.push(arg.clone()),
        }
    }
    Ok(parsed)
}

/// `helios stacks`: see the module documentation.
fn stacks(args: &[String]) -> ExitCode {
    use helios_library::stacks::{self as suggest, Resolvers, RuleResolver};
    let parsed = match stacks_args(args) {
        Ok(p) => p,
        Err(e) => return usage(&e),
    };
    let Some(path) = &parsed.vault else {
        return usage("stacks needs --vault FILE");
    };
    let vault = match open_vault(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("helios: {e}");
            return ExitCode::from(1);
        }
    };
    let images = |names: &[&str]| -> Result<Vec<helios_vault::ImageId>, String> {
        names.iter().map(|n| image_named(&vault, n)).collect()
    };
    let words: Vec<&str> = parsed.words.iter().map(String::as_str).collect();
    let done: Result<serde_json::Value, String> = match words.as_slice() {
        ["list"] => vault.stacks().map(|s| to_json(&s)),
        ["create", members @ ..] if !members.is_empty() => images(members).and_then(|ids| {
            let top = parsed
                .top
                .as_deref()
                .map(|t| image_named(&vault, t))
                .transpose()?;
            vault
                .create_stack(&ids, top, "user", now())
                .map(|s| to_json(&s))
        }),
        ["dissolve", id] => vault.dissolve_stack(id).map(|s| to_json(&s)),
        ["expand", id] => vault.set_stack_expanded(id, true).map(|s| to_json(&s)),
        ["collapse", id] => vault.set_stack_expanded(id, false).map(|s| to_json(&s)),
        ["top", id, image] => image_named(&vault, image)
            .and_then(|i| vault.set_stack_top(id, &i))
            .map(|s| to_json(&s)),
        ["suggest", selection @ ..] => images(selection).and_then(|ids| {
            let resolvers = match &parsed.rules {
                Some(rules) => Resolvers::new().with(
                    suggest::RULES,
                    Arc::new(RuleResolver {
                        rules: rules.clone(),
                    }),
                ),
                None => Resolvers::default(),
            };
            let resolver = parsed.resolver.as_deref().unwrap_or(suggest::RULES);
            suggest::suggest(&vault, None, &resolvers, resolver, &ids, now())
                .map(|d| suggest::to_json(&d))
        }),
        ["accept", id] => {
            suggest::accept(&vault, None, id, parsed.groups.as_deref(), "user", now())
                .map(|d| suggest::to_json(&d))
        }
        ["decline", id] => {
            suggest::decline(&vault, None, id, "user", now()).map(|d| suggest::to_json(&d))
        }
        ["decisions"] => suggest::decisions(&vault).map(|d| to_json(&d)),
        _ => {
            return usage(
                "stacks takes: list | create IMAGE... | dissolve STACK | expand STACK | collapse STACK | top STACK IMAGE | suggest [IMAGE...] | accept DECISION | decline DECISION | decisions",
            );
        }
    };
    match done {
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

/// `helios doctor display`: what the display offers, as JSON; `helios
/// doctor ai`: the AI report.
fn doctor(args: &[String]) -> ExitCode {
    match args {
        [what] if what == "display" => {
            let offer = helios_display::probe();
            let report = serde_json::json!({
                "session": offer.session,
                "color_management": offer.color_management,
                "error": offer.error,
                "viewport": helios_display::drawn(&offer),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("JSON values serialize")
            );
            ExitCode::SUCCESS
        }
        [what] if what == "ai" => {
            match helios_ai::Ai::open(helios_ai::store::Paths::default_for_user()) {
                Ok(ai) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&ai.doctor()).expect("JSON values serialize")
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("helios: the AI service: {e}");
                    ExitCode::from(1)
                }
            }
        }
        _ => usage("doctor takes `display` or `ai`"),
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
    if args.first().is_some_and(|a| a == "import-darktable") {
        return import_darktable(&args[1..]);
    }
    if args.first().is_some_and(|a| a == "import") {
        return import(&args[1..]);
    }
    if args.first().is_some_and(|a| a == "stacks") {
        return stacks(&args[1..]);
    }
    if args.first().is_some_and(|a| a == "doctor") {
        return doctor(&args[1..]);
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
