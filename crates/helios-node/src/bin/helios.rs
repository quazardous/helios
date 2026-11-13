//! `helios`: the command line of Helios.
//!
//!     helios --version
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
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] import FOLDERS-OR-IMAGES...
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] settings-get LAYER
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] settings-set LAYER TOML-FILE|-
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] pause | resume
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] fault NODE compute|source-read
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] calibrate-chart PICTURE X,Y X,Y X,Y X,Y
//!                    [--chart ID] [--exclude PATCH]...
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] workflow-submit --format jpeg|png|tiff
//!                    [--quality Q] [--again] [--jobs N] --to DIR IMAGES...
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] workflow-runs | workflow-items RUN [NODE] | workflow-forget RUN
//!                    | workflow-cancel RUN [ITEMS...] [--node NODE] | workflow-retry RUN [ITEMS...] [--node NODE]
//!                    | workflow-prioritize RUN top|bottom ITEMS... | workflow-pause RUN | workflow-resume RUN
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] commands [FILTER] [--enabled]
//!     helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] run ID [PARAMS-JSON]
//!
//!     helios sandbox new DIR [--network FILE] | start DIR | stop DIR | clean DIR
//!
//!     helios batch --network FILE --out DIR [--format jpeg|png|tiff] [--quality Q] [--jobs N]
//!                  [--name TEMPLATE] [--sidecar] [--vault FILE] [--again] [--source NAME]
//!                  [--node NAME] [--socket ADDR --control ADDR [--token-file FILE]] IMAGES...
//!
//!     helios import-darktable --vault FILE|--node ADDRESS FOLDERS-OR-IMAGES...
//!
//!     helios import --vault FILE|--node ADDRESS [--reference | --copy [--into DIR]] [--rename PATTERN]
//!                   [--layout day|month|year|flat] [--duplicates skip|keep|ask] [--no-signals] FOLDERS-CARDS-OR-FILES...
//!     helios stacks --vault FILE|--node ADDRESS list | create IMAGE... [--top IMAGE] | dissolve STACK
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
//! interrupted: Ctrl-C cancels the batch's images in the run and leaves no
//! partial file, or, with `--vault` and its own node, stops it to be
//! finished by the same batch run again. See `docs/cli.md`.
//!
//! `calibrate-chart` measures a colour chart shot (a decoded picture) and
//! solves its correction, a dry run that writes nothing: the four points
//! are the centres of the chart's corner patches in the picture's pixels,
//! in any order; it prints the report (see `docs/design/calibration.md`).
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
//! included; sidecars whose image is missing are kept in the vault as
//! they are, with their import report, and listed as orphans. One
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
//! `import`, `import-darktable` and `stacks` never open the vault: they
//! run on the node holding it (`helios_node::connect`), found through the
//! rendezvous file it keeps beside the vault, or named by `--node ADDRESS`
//! or `$HELIOS_NODE`. With no node holding the vault, the CLI starts one
//! (`helios-node --vault FILE --idle-exit SECONDS`, a minute, or
//! `$HELIOS_IDLE_EXIT` seconds), which exits once idle; several commands
//! at once share it. Exit code 3 when no node can be reached or started.
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
use helios_inspect::client::Client;
use helios_inspect::control::{ControlRequest, FileFormat, MoveTo, RunItem};
use helios_inspect::{Address, Request, request_at, rpc};
use helios_node::batch::{Options, Outcome};

const USAGE: &str = "usage:
  helios --version
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
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] import FOLDERS-OR-IMAGES...
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] settings-get LAYER
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] settings-set LAYER TOML-FILE|-
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] pause | resume
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] fault NODE compute|source-read
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] calibrate-chart PICTURE X,Y X,Y X,Y X,Y
                 [--chart ID] [--exclude PATCH]...
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] workflow-submit --format jpeg|png|tiff
                 [--quality Q] [--again] [--jobs N] --to DIR IMAGES...
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] workflow-runs | workflow-items RUN [NODE] | workflow-forget RUN
                 | workflow-cancel RUN [ITEMS...] [--node NODE] | workflow-retry RUN [ITEMS...] [--node NODE]
                 | workflow-prioritize RUN top|bottom ITEMS... | workflow-pause RUN | workflow-resume RUN
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] commands [FILTER] [--enabled]
  helios control [--socket PATH|tcp://HOST:PORT --token-file FILE] run ID [PARAMS-JSON]
  helios sandbox new DIR [--network FILE] | start DIR | stop DIR | clean DIR
  helios batch --network FILE --out DIR [--format jpeg|png|tiff] [--quality Q] [--jobs N]
               [--name TEMPLATE] [--sidecar] [--vault FILE] [--again] [--source NAME]
               [--node NAME] [--socket ADDR --control ADDR [--token-file FILE]] IMAGES...
  helios import-darktable --vault FILE|--node ADDRESS FOLDERS-OR-IMAGES...
  helios import --vault FILE|--node ADDRESS [--reference | --copy [--into DIR]] [--rename PATTERN]
                [--layout day|month|year|flat] [--duplicates skip|keep|ask] [--no-signals] FOLDERS-CARDS-OR-FILES...
  helios stacks --vault FILE|--node ADDRESS list | create IMAGE... [--top IMAGE] | dissolve STACK
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
/// A point `X,Y` in pixels.
fn point(text: &str) -> Result<[f64; 2], String> {
    let bad = || format!("`{text}` is not a point X,Y");
    let (x, y) = text.split_once(',').ok_or_else(bad)?;
    let x: f64 = x.trim().parse().map_err(|_| bad())?;
    let y: f64 = y.trim().parse().map_err(|_| bad())?;
    if x.is_finite() && y.is_finite() {
        Ok([x, y])
    } else {
        Err(bad())
    }
}

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
    let mut enabled_only = false;
    let mut at = None;
    let mut jobs = None;
    let mut sets: BTreeMap<String, Params> = BTreeMap::new();
    let (mut chart, mut exclude) = (None, Vec::new());
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
            "--enabled" => enabled_only = true,
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
            "--chart" => chart = Some(args.next().ok_or("--chart needs a chart id")?),
            "--exclude" => exclude.push(args.next().ok_or("--exclude needs a patch name")?),
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
            ["calibrate-chart", path, a, b, c, d] => ControlRequest::CalibrateChart {
                path: absolute(path)?,
                corners: [point(a)?, point(b)?, point(c)?, point(d)?],
                chart: chart.clone(),
                exclude: exclude.clone(),
                dry_run: true,
            },
            ["import", paths @ ..] if !paths.is_empty() => ControlRequest::LibraryImport {
                paths: paths
                    .iter()
                    .copied()
                    .map(absolute)
                    .collect::<Result<_, _>>()?,
                copy: false,
                into: None,
                rename: None,
                layout: None,
                duplicates: None,
                signals: None,
            },
            ["settings-get", layer] => ControlRequest::SettingsLayerGet {
                layer: layer.to_string(),
            },
            ["settings-set", layer, file] => ControlRequest::SettingsLayerSet {
                layer: layer.to_string(),
                text: if *file == "-" {
                    std::io::read_to_string(std::io::stdin())
                        .map_err(|e| format!("the standard input: {e}"))?
                } else {
                    std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?
                },
            },
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
            ["commands"] => ControlRequest::CommandsList {
                filter: None,
                enabled_only,
            },
            ["commands", filter] => ControlRequest::CommandsList {
                filter: Some(filter.to_string()),
                enabled_only,
            },
            ["run", id] => ControlRequest::CommandRun {
                id: id.to_string(),
                params: serde_json::json!({}),
                origin: "cli".into(),
            },
            ["run", id, params] => ControlRequest::CommandRun {
                id: id.to_string(),
                params: serde_json::from_str(params)
                    .map_err(|e| format!("run takes its params as JSON: {e}"))?,
                origin: "cli".into(),
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

/// A client of the node holding `vault` (or of the node `--node` or
/// `HELIOS_NODE` names), started on the vault when none holds it: the
/// CLI never opens the vault itself. Exit code 3 when it cannot.
fn vault_client(vault: Option<&std::path::Path>, node: Option<&str>) -> Result<Client, ExitCode> {
    match helios_node::connect::connect(vault, node, &node_bin(), "cli") {
        Ok(connected) => Ok(connected.client),
        Err(e) => {
            eprintln!("helios: {e}");
            Err(ExitCode::from(3))
        }
    }
}

/// Runs `method` on the node and waits for its answer; with `interrupt`,
/// Ctrl-C or SIGTERM asks the node to stop the call (`$/cancel`).
fn on_node(
    client: &Client,
    method: &str,
    params: serde_json::Value,
    interrupt: bool,
) -> Result<serde_json::Value, rpc::Error> {
    let pending = client.call(method, params);
    if !interrupt {
        return pending.wait();
    }
    let interrupted = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        if let Err(e) = signal_hook::flag::register(signal, interrupted.clone()) {
            eprintln!("helios: cannot watch for interrupts: {e}");
        }
    }
    let mut asked = false;
    loop {
        if let Some(answer) = pending.wait_timeout(std::time::Duration::from_millis(100)) {
            return answer;
        }
        if !asked && interrupted.load(std::sync::atomic::Ordering::SeqCst) {
            client.cancel(&pending);
            asked = true;
        }
    }
}

/// Prints one JSON line.
fn line(value: &serde_json::Value) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

/// A path given on the command line, made absolute for the node.
fn absolute_text(text: &str) -> Result<String, String> {
    std::path::absolute(text)
        .map(|p| p.display().to_string())
        .map_err(|e| format!("{text}: {e}"))
}

/// `helios import-darktable --vault FILE PATH...`: see
/// `helios_node::darktable::import`, run by the vault's node
/// (`library.importDarktable`).
fn import_darktable(args: &[String]) -> ExitCode {
    let (mut vault, mut node) = (None, None);
    let mut paths = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--vault" | "--node" => match args.next() {
                Some(v) if arg == "--vault" => vault = Some(PathBuf::from(v)),
                Some(v) => node = Some(v.clone()),
                None => return usage(&format!("{arg} needs a value")),
            },
            flag if flag.starts_with("--") => return usage(&format!("unknown option `{flag}`")),
            _ => match absolute_text(arg) {
                Ok(path) => paths.push(path),
                Err(e) => return usage(&e),
            },
        }
    }
    if vault.is_none() && helios_node::connect::explicit(node.as_deref()).is_none() {
        return usage("import-darktable needs --vault FILE");
    }
    if paths.is_empty() {
        return usage("import-darktable needs folders or images");
    }
    let client = match vault_client(vault.as_deref(), node.as_deref()) {
        Ok(c) => c,
        Err(code) => return code,
    };
    match on_node(
        &client,
        "library.importDarktable",
        serde_json::json!({ "paths": paths }),
        false,
    ) {
        Ok(answer) => {
            for file in answer["files"].as_array().into_iter().flatten() {
                line(file);
            }
            for w in answer["warnings"].as_array().into_iter().flatten() {
                eprintln!(
                    "helios import-darktable: {}: {}",
                    w["path"].as_str().unwrap_or_default(),
                    w["warning"].as_str().unwrap_or_default()
                );
            }
            if answer["kept"] == true {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("helios: {}", e.message);
            ExitCode::from(1)
        }
    }
}

/// The options of `helios import`: the vault, the node, and
/// `library.import`'s params.
fn import_options(
    args: &[String],
) -> Result<(Option<PathBuf>, Option<String>, serde_json::Value), String> {
    let (mut vault, mut node) = (None, None);
    let mut sources = Vec::new();
    let mut params = serde_json::Map::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--vault" => vault = Some(PathBuf::from(value()?)),
            "--node" => node = Some(value()?.clone()),
            "--copy" => {
                params.insert("copy".into(), true.into());
            }
            "--reference" => {
                params.insert("copy".into(), false.into());
            }
            "--into" => {
                params.insert("into".into(), absolute_text(value()?)?.into());
                params.insert("copy".into(), true.into());
            }
            "--rename" => {
                let pattern = value()?;
                helios_library::rename::Pattern::parse(pattern)?;
                params.insert("rename".into(), pattern.clone().into());
            }
            "--layout" => {
                let layout = value()?;
                helios_library::import::Layout::parse(layout)?;
                params.insert("layout".into(), layout.clone().into());
            }
            "--duplicates" => {
                let duplicates = value()?;
                helios_library::import::Duplicates::parse(duplicates)?;
                params.insert("duplicates".into(), duplicates.clone().into());
            }
            "--no-signals" => {
                params.insert("signals".into(), false.into());
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option `{flag}`")),
            _ => sources.push(serde_json::Value::from(absolute_text(arg)?)),
        }
    }
    if sources.is_empty() {
        return Err("import needs folders, cards or files".into());
    }
    if vault.is_none() && helios_node::connect::explicit(node.as_deref()).is_none() {
        return Err("import needs --vault FILE".into());
    }
    params.insert("paths".into(), sources.into());
    Ok((vault, node, serde_json::Value::Object(params)))
}

/// `helios import`: see `helios_library::import`, run by the vault's node
/// (`library.import`).
fn import(args: &[String]) -> ExitCode {
    let (vault, node, params) = match import_options(args) {
        Ok(p) => p,
        Err(e) => return usage(&e),
    };
    let client = match vault_client(vault.as_deref(), node.as_deref()) {
        Ok(c) => c,
        Err(code) => return code,
    };
    // Ctrl-C stops the import before its next file, a copy under way
    // removed; the files done stay imported.
    match on_node(&client, "library.import", params, true) {
        Ok(answer) => {
            for file in answer["files"].as_array().into_iter().flatten() {
                line(file);
            }
            let report = &answer["report"];
            line(&serde_json::json!({ "report": report }));
            if report["cancelled"] == true {
                ExitCode::from(130)
            } else if report["failed"].as_u64().unwrap_or(0) == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("helios: {}", e.message);
            ExitCode::from(1)
        }
    }
}

/// An image named on the command line: its id, or a file (made absolute
/// for the node, which knows it).
fn image_named(text: &str) -> Result<String, String> {
    if helios_vault::ImageId::from_hex(text).is_ok() {
        return Ok(text.to_string());
    }
    absolute_text(text)
}

/// The options of `helios stacks`.
#[derive(Default)]
struct StacksArgs {
    vault: Option<PathBuf>,
    node: Option<String>,
    words: Vec<String>,
    top: Option<String>,
    resolver: Option<String>,
    groups: Option<Vec<usize>>,
    window: Option<f64>,
    distance: Option<u32>,
}

fn stacks_args(args: &[String]) -> Result<StacksArgs, String> {
    let mut parsed = StacksArgs::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--vault" => parsed.vault = Some(PathBuf::from(value()?)),
            "--node" => parsed.node = Some(value()?.clone()),
            "--top" => parsed.top = Some(value()?.clone()),
            "--resolver" => parsed.resolver = Some(value()?.clone()),
            "--window" => {
                parsed.window = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--window takes seconds".to_string())?,
                );
            }
            "--distance" => {
                parsed.distance = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--distance takes a number of bits".to_string())?,
                );
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

/// `helios stacks`: see the module documentation; each run by the
/// vault's node (`library.stacks.*`), the answer's value printed.
fn stacks(args: &[String]) -> ExitCode {
    use serde_json::json;
    let parsed = match stacks_args(args) {
        Ok(p) => p,
        Err(e) => return usage(&e),
    };
    if parsed.vault.is_none() && helios_node::connect::explicit(parsed.node.as_deref()).is_none() {
        return usage("stacks needs --vault FILE");
    }
    let images = |names: &[&str]| -> Result<Vec<String>, String> {
        names.iter().map(|n| image_named(n)).collect()
    };
    let words: Vec<&str> = parsed.words.iter().map(String::as_str).collect();
    let call: Result<(&str, serde_json::Value, &str), String> = match words.as_slice() {
        ["list"] => Ok(("library.stacks.list", json!({}), "stacks")),
        ["create", members @ ..] if !members.is_empty() => images(members).and_then(|ids| {
            let top = parsed.top.as_deref().map(image_named).transpose()?;
            Ok((
                "library.stacks.create",
                json!({ "images": ids, "top": top }),
                "stack",
            ))
        }),
        ["dissolve", id] => Ok(("library.stacks.dissolve", json!({ "stack": id }), "stack")),
        ["expand", id] => Ok((
            "library.stacks.expand",
            json!({ "stack": id, "expanded": true }),
            "stack",
        )),
        ["collapse", id] => Ok((
            "library.stacks.expand",
            json!({ "stack": id, "expanded": false }),
            "stack",
        )),
        ["top", id, image] => image_named(image).map(|image| {
            (
                "library.stacks.top",
                json!({ "stack": id, "image": image }),
                "stack",
            )
        }),
        ["suggest", selection @ ..] => images(selection).map(|ids| {
            (
                "library.stacks.suggest",
                json!({ "images": ids, "resolver": parsed.resolver,
                        "window": parsed.window, "distance": parsed.distance }),
                "decision",
            )
        }),
        ["accept", id] => Ok((
            "library.stacks.accept",
            json!({ "decision": id, "groups": parsed.groups }),
            "decision",
        )),
        ["decline", id] => Ok((
            "library.stacks.decline",
            json!({ "decision": id }),
            "decision",
        )),
        ["decisions"] => Ok(("library.stacks.decisions", json!({}), "decisions")),
        _ => {
            return usage(
                "stacks takes: list | create IMAGE... | dissolve STACK | expand STACK | collapse STACK | top STACK IMAGE | suggest [IMAGE...] | accept DECISION | decline DECISION | decisions",
            );
        }
    };
    let (method, params, field) = match call {
        Ok(call) => call,
        Err(e) => {
            eprintln!("helios: {e}");
            return ExitCode::from(1);
        }
    };
    let client = match vault_client(parsed.vault.as_deref(), parsed.node.as_deref()) {
        Ok(c) => c,
        Err(code) => return code,
    };
    match on_node(&client, method, params, false) {
        Ok(answer) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&answer[field]).expect("JSON values serialize")
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("helios: {}", e.message);
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
    if let [flag] = args.as_slice()
        && (flag == "--version" || flag == "-V")
    {
        println!("helios {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
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
