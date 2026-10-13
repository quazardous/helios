//! `helios batch`: a network applied to many files, each exported to a file.
//!
//! A client of a node's control and inspect endpoints with no logic of its
//! own: for each image it gives the network's source the file
//! (`load_source`), asks for an export (`export`, a scheduler job holding
//! its destination) and waits for the job's end on the bus. Up to `jobs`
//! exports are asked at once; the node's scheduler runs them side by side.
//! It starts its own node in a temporary sandbox, serving the batch
//! endpoint (those three verbs only, in every build) on a private socket,
//! or attaches to a running node's batch or debug control endpoint.
//!
//! Each image gets one JSON line on stdout: its path, the file written, how
//! long it took, and the error if it failed, in English (`error`) and by
//! key when it has one (`message`, a [`Message`] to show translated). The
//! others go on after a failure.
//!
//! With `--sidecar`, each image's base darktable sidecar is applied; its
//! folder is listed once to find it and its duplicates
//! ([`helios_xmp::files`]). With `--vault FILE` as well, every sidecar of
//! the image is kept in that vault ([`keep_sidecar`]): the image, each
//! darktable edit (base and duplicates) as a version with origin
//! `import:xmp`, and the base's rating when the vault has none. On an
//! interrupt, the exports asked for are cancelled: a file is written whole
//! or not at all, never in part.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use helios_core::{Message, Params};
use helios_inspect::control::{ControlRequest, FileFormat};
use helios_inspect::{Address, Request, request_at};
use serde_json::{Value, json};

pub use crate::darktable::{IMPORT_XMP, keep_sidecar, read_sidecar};
use crate::darktable::{Listings, keep_file};

/// What `helios batch` was asked.
#[derive(Clone, Debug)]
pub struct Options {
    /// The network file; required unless attached to a node.
    pub network: Option<PathBuf>,
    pub out: PathBuf,
    pub format: FileFormat,
    pub quality: u8,
    /// Exports asked for at once.
    pub jobs: usize,
    /// The output file name: `{stem}` and `{ext}` are replaced.
    pub name: String,
    /// Apply each image's darktable sidecar on top of the network.
    pub sidecar: bool,
    /// Keep each sidecar applied in this vault (see [`keep_sidecar`]).
    pub vault: Option<PathBuf>,
    /// The source given each image; the network's only source by default.
    pub source: Option<String>,
    /// The node exported; the network's first export, else its only sink.
    pub node: Option<String>,
    /// A running node to attach to, instead of starting one.
    pub attach: Option<(Address, Address)>,
    pub images: Vec<PathBuf>,
}

/// The extension of a format's files.
pub fn extension(format: FileFormat) -> &'static str {
    match format {
        FileFormat::Jpeg => "jpg",
        FileFormat::Png => "png",
        FileFormat::Tiff => "tif",
    }
}

/// The output paths of the images, in order: the name template filled in,
/// and `-1`, `-2`... before the extension when the name is taken, by an
/// earlier image of the batch or by a file already there.
pub fn output_paths(
    images: &[PathBuf],
    out: &Path,
    template: &str,
    ext: &str,
    exists: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut taken = BTreeSet::new();
    images
        .iter()
        .map(|image| {
            let stem = image
                .file_stem()
                .map_or_else(|| "image".into(), |s| s.to_string_lossy().into_owned());
            let name = template.replace("{stem}", &stem).replace("{ext}", ext);
            let (base, dot_ext) = match name.rsplit_once('.') {
                Some((b, e)) if !b.is_empty() => (b.to_string(), format!(".{e}")),
                _ => (name.clone(), String::new()),
            };
            let mut n = 0;
            loop {
                let candidate = if n == 0 {
                    out.join(&name)
                } else {
                    out.join(format!("{base}-{n}{dot_ext}"))
                };
                if !taken.contains(&candidate) && !exists(&candidate) {
                    taken.insert(candidate.clone());
                    break candidate;
                }
                n += 1;
            }
        })
        .collect()
}

/// What a sidecar changes in the network: parameters by node name, and
/// what was left out. A module of the edit goes to the one node of the
/// network running that module; one the network does not run, or runs
/// more than once, is left out and named.
pub fn sidecar_params(
    imported: &helios_xmp::Imported,
    modules: &BTreeMap<String, String>,
) -> (BTreeMap<String, Params>, Value) {
    let mut params: BTreeMap<String, Params> = BTreeMap::new();
    let (mut applied, mut skipped) = (Vec::new(), Vec::new());
    for n in &imported.nodes {
        let mut running = modules.iter().filter(|(_, m)| **m == n.module);
        match (running.next(), running.next()) {
            (Some((node, _)), None) if !params.contains_key(node) => {
                let target = params.entry(node.clone()).or_default();
                for (k, v) in n.params.iter() {
                    target.set(k, v.clone());
                }
                applied.push(json!({ "operation": n.operation, "node": node }));
            }
            (None, _) => skipped.push(json!({ "operation": n.operation, "reason": format!("the network has no `{}` node", n.module) })),
            _ => skipped.push(json!({ "operation": n.operation, "reason": format!("several `{}` nodes or instances", n.module) })),
        }
    }
    let mut report = json!({ "applied": applied, "skipped": skipped, "import": imported.report });
    if let Some(warning) = imported.report.summary() {
        report["warning"] = json!(warning.english);
        report["warning_message"] = json!(warning);
    }
    (params, report)
}

/// The network a batch's own node runs: files made absolute (the network
/// is copied into the sandbox), its own renders and exports left out.
fn sandbox_network(path: &Path) -> Result<(String, Option<String>), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut network: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let base = std::path::absolute(path)
        .map_err(|e| e.to_string())?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if let Some(sources) = network["sources"].as_array_mut() {
        for s in sources {
            if let Some(file) = s["file"].as_str() {
                s["file"] = json!(base.join(file));
            }
        }
    }
    let first_export = network["exports"][0]["node"].as_str().map(str::to_string);
    if let Some(map) = network.as_object_mut() {
        map.remove("renders");
        map.remove("exports");
    }
    Ok((
        serde_json::to_string_pretty(&network).expect("JSON values serialize"),
        first_export,
    ))
}

/// A node the batch talks to; its own sandbox is removed when dropped.
struct Node {
    inspect: Address,
    control: Address,
    sandbox: Option<PathBuf>,
}

impl Drop for Node {
    fn drop(&mut self) {
        if let Some(dir) = &self.sandbox {
            let _ = crate::sandbox::clean(dir);
        }
    }
}

impl Node {
    fn call(address: &Address, body: &Value) -> Result<Value, Message> {
        let answer = request_at(address, body).map_err(|e| {
            Message::new(
                "batch-node-unreachable",
                "cannot reach the node at { $address }: { $error }",
            )
            .with("address", address.to_string())
            .with("error", e.to_string())
        })?;
        if answer["ok"] == true {
            Ok(answer)
        } else {
            // The node's message when it has a key, else its English.
            let keyed = serde_json::from_value::<Message>(answer["message"].clone()).ok();
            Err(match (keyed, answer["error"].as_str()) {
                (Some(m), _) => m,
                (None, Some(e)) => Message::plain(e),
                (None, None) => Message::new("batch-node-refused", "the node refused"),
            })
        }
    }

    fn control(&self, request: &ControlRequest) -> Result<Value, Message> {
        Self::call(
            &self.control,
            &serde_json::to_value(request).expect("requests serialize"),
        )
    }

    fn inspect(&self, request: &Request) -> Result<Value, Message> {
        Self::call(
            &self.inspect,
            &serde_json::to_value(request).expect("requests serialize"),
        )
    }
}

/// Starts a node in a fresh sandbox (`node_bin` is `helios-node`), serving
/// the batch endpoint on a socket in a directory private to this run: the
/// debug control endpoint is not needed, so this works in release builds.
fn own_node(
    network: &Path,
    jobs: usize,
    node_bin: &Path,
) -> Result<(Node, Option<String>), String> {
    let (text, first_export) = sandbox_network(network)?;
    let dir = std::env::temp_dir().join(format!("helios-batch-{}", std::process::id()));
    if dir.join("sandbox.json").exists() {
        crate::sandbox::clean(&dir)?;
    }
    crate::sandbox::create(&dir, Some(&text))?;
    let started = std::fs::write(
        dir.join("global.toml"),
        format!("[system.scheduler]\nworkers = {jobs}\n"),
    )
    .map_err(|e| e.to_string())
    .and_then(|()| crate::sandbox::start_batch(&dir, node_bin));
    match started {
        Ok(state) => Ok((
            Node {
                inspect: Address::Unix(state.inspect),
                control: Address::Unix(state.control),
                sandbox: Some(dir),
            },
            first_export,
        )),
        Err(e) => {
            let _ = crate::sandbox::clean(&dir);
            Err(e)
        }
    }
}

/// The source and the node a batch uses, from the node's graph.
fn pick(
    graph: &Value,
    options: &Options,
    first_export: Option<String>,
) -> Result<(String, String), Message> {
    let nodes = graph["nodes"].as_array().cloned().unwrap_or_default();
    let names = |kind: &str| -> Vec<String> {
        nodes
            .iter()
            .filter(|n| n["kind"] == kind)
            .filter_map(|n| n["name"].as_str().map(str::to_string))
            .collect()
    };
    let source = match &options.source {
        Some(s) => s.clone(),
        None => match names("source").as_slice() {
            [one] => one.clone(),
            [] => {
                return Err(Message::new("batch-no-source", "the network has no source"));
            }
            many => {
                return Err(Message::new(
                    "batch-several-sources",
                    "the network has several sources ({ $names }): say --source NAME",
                )
                .with("names", many.join(", ")));
            }
        },
    };
    let read: BTreeSet<u64> = nodes
        .iter()
        .flat_map(|n| n["inputs"].as_array().cloned().unwrap_or_default())
        .filter_map(|i| i[0].as_u64())
        .collect();
    let sinks: Vec<String> = nodes
        .iter()
        .filter(|n| n["kind"] == "module" && !n["id"].as_u64().is_some_and(|id| read.contains(&id)))
        .filter_map(|n| n["name"].as_str().map(str::to_string))
        .collect();
    let node = match options.node.clone().or(first_export) {
        Some(n) => n,
        None => match sinks.as_slice() {
            [one] => one.clone(),
            [] => source.clone(),
            many => {
                return Err(Message::new(
                    "batch-several-ends",
                    "the network ends in several nodes ({ $names }): say --node NAME",
                )
                .with("names", many.join(", ")));
            }
        },
    };
    Ok((source, node))
}

/// One image on its way.
struct Pending {
    image: PathBuf,
    output: PathBuf,
    started: Instant,
    sidecar: Value,
}

fn line(p: &Pending, error: Option<&Message>) -> Value {
    let mut line = json!({
        "path": p.image,
        "output": if error.is_none() { json!(p.output) } else { Value::Null },
        "duration_ms": p.started.elapsed().as_millis() as u64,
        "error": error.map(|e| &e.english),
        "sidecar": p.sidecar,
    });
    if let Some(e) = error.filter(|e| e.is_keyed()) {
        line["message"] = json!(e);
    }
    line
}

/// The key of a node that cannot be reached (as `Node::call` writes it):
/// it stops the whole batch.
pub const UNREACHABLE: &str = "batch-node-unreachable";

fn cancelled_message() -> Message {
    Message::new("batch-cancelled", "cancelled")
}

/// How a batch ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every image was written.
    Done,
    /// At least one image failed; the others were written.
    Failed,
    /// Interrupted: what was asked was cancelled.
    Interrupted,
}

/// Runs a batch, printing a JSON line per image through `emit`. Errors
/// that stop the whole batch (no node, a network that does not fit) come
/// back as `Err`.
pub fn run(
    options: &Options,
    node_bin: &Path,
    interrupted: &Arc<AtomicBool>,
    mut emit: impl FnMut(&Value),
) -> Result<Outcome, Message> {
    std::fs::create_dir_all(&options.out).map_err(|e| {
        Message::new("batch-cannot-create", "cannot create { $path }: { $error }")
            .with("path", &options.out)
            .with("error", e.to_string())
    })?;
    let out = std::path::absolute(&options.out).map_err(|e| Message::plain(e.to_string()))?;
    let (node, first_export) = match (&options.attach, &options.network) {
        (Some((inspect, control)), network) => {
            let first = match network {
                Some(path) => sandbox_network(path)?.1,
                None => None,
            };
            let node = Node {
                inspect: inspect.clone(),
                control: control.clone(),
                sandbox: None,
            };
            (node, first)
        }
        (None, Some(network)) => own_node(network, options.jobs, node_bin)?,
        (None, None) => {
            return Err(Message::new(
                "batch-needs-network",
                "batch needs --network FILE, or a node to attach to",
            ));
        }
    };
    let graph = node.inspect(&Request::Describe {
        entity: "graph".into(),
    })?["describe"]
        .clone();
    let (source, target) = pick(&graph, options, first_export)?;
    let modules: BTreeMap<String, String> = graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            Some((
                n["name"].as_str()?.to_string(),
                n["module"].as_str()?.to_string(),
            ))
        })
        .collect();
    let images: Vec<PathBuf> = options
        .images
        .iter()
        .map(|i| std::path::absolute(i).unwrap_or_else(|_| i.clone()))
        .collect();
    let outputs = output_paths(
        &images,
        &out,
        &options.name,
        extension(options.format),
        Path::exists,
    );

    let vault = match (&options.vault, options.sidecar) {
        (Some(path), true) => Some(helios_vault::Vault::new(
            helios_vault_sqlite::SqliteStore::open(path)
                .map_err(|e| Message::plain(format!("{}: {e}", path.display())))?,
        )),
        (Some(_), false) => {
            return Err(Message::new(
                "batch-vault-needs-sidecar",
                "--vault keeps sidecars: it needs --sidecar",
            ));
        }
        (None, _) => None,
    };

    let mut sidecars = Sidecars {
        vault: vault.as_ref(),
        listings: Listings::default(),
    };
    let mut failed = false;
    let mut cursor = 0;
    let mut running: BTreeMap<u64, Pending> = BTreeMap::new();
    let mut next = 0;
    let mut cancelled = false;
    while next < images.len() || !running.is_empty() {
        if interrupted.load(Ordering::SeqCst) && !cancelled {
            cancelled = true;
            for job in running.keys() {
                let _ = node.control(&ControlRequest::Cancel { job: *job });
            }
            for image in &images[next..] {
                emit(&json!({
                    "path": image,
                    "output": null,
                    "duration_ms": 0,
                    "error": cancelled_message().english,
                    "message": cancelled_message(),
                    "sidecar": null,
                }));
            }
            next = images.len();
        }
        while !cancelled && running.len() < options.jobs.max(1) && next < images.len() {
            let pending = Pending {
                image: images[next].clone(),
                output: outputs[next].clone(),
                started: Instant::now(),
                sidecar: Value::Null,
            };
            next += 1;
            match submit(
                &node,
                options,
                &mut sidecars,
                &source,
                &target,
                &modules,
                pending,
            ) {
                Ok((job, seq, pending)) => {
                    if cursor == 0 {
                        cursor = seq;
                    }
                    running.insert(job, pending);
                }
                Err(refused) => {
                    let (pending, error) = *refused;
                    failed = true;
                    emit(&line(&pending, Some(&error)));
                }
            }
        }
        if running.is_empty() {
            continue;
        }
        // Bounded, so an interrupt is seen soon.
        let ended = node.inspect(&Request::WaitEvent {
            kind: "scheduler.job_ended".into(),
            since: cursor,
            timeout_ms: 200,
        });
        let Ok(ended) = ended else {
            if let Err(e) = ended
                && e.key == UNREACHABLE
            {
                return Err(e);
            }
            continue;
        };
        let event = &ended["event"];
        cursor = event["seq"].as_u64().unwrap_or(cursor);
        let Some(pending) = event["payload"]["job"]
            .as_u64()
            .and_then(|j| running.remove(&j))
        else {
            continue;
        };
        match event["payload"]["state"].as_str() {
            Some("done") => emit(&line(&pending, None)),
            Some("cancelled") => emit(&line(&pending, Some(&cancelled_message()))),
            _ => {
                failed = true;
                // The job's message by key, else its English.
                let error = serde_json::from_value::<Message>(event["payload"]["message"].clone())
                    .unwrap_or_else(|_| match event["payload"]["error"].as_str() {
                        Some(e) => Message::plain(e),
                        None => Message::new("batch-failed", "failed"),
                    });
                emit(&line(&pending, Some(&error)));
            }
        }
    }
    Ok(if cancelled {
        Outcome::Interrupted
    } else if failed {
        Outcome::Failed
    } else {
        Outcome::Done
    })
}

/// Where a batch finds sidecars, and the vault it keeps them in.
struct Sidecars<'a> {
    vault: Option<&'a helios_vault::Vault<helios_vault_sqlite::SqliteStore>>,
    listings: Listings,
}

/// Gives the source the image and asks for its export: the job and the
/// sequence number of the request, or why the image cannot be exported.
fn submit(
    node: &Node,
    options: &Options,
    sidecars: &mut Sidecars,
    source: &str,
    target: &str,
    modules: &BTreeMap<String, String>,
    mut pending: Pending,
) -> Result<(u64, u64, Pending), Box<(Pending, Message)>> {
    let mut params = BTreeMap::new();
    let vault = sidecars.vault;
    if options.sidecar {
        let files = match sidecars.listings.sidecars(&pending.image) {
            Ok(files) => files,
            Err(e) => return Err(Box::new((pending, Message::plain(e)))),
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut warn = |path: &Path, warning: &str| {
            // Once per import, beside the image's JSON line.
            eprintln!("helios batch: {}: {warning}", path.display());
        };
        // The base sidecar is the edit exported; the others are kept only.
        let (base, others) = match files.split_first() {
            Some((first, rest)) if first.version == 0 => (Some(first), rest),
            _ => (None, &files[..]),
        };
        let mut report = Value::Null;
        if let Some(base) = base {
            let path = &base.path;
            let read = std::fs::read_to_string(path)
                .map_err(|e| {
                    Message::new("batch-cannot-read", "cannot read { $path }: { $error }")
                        .with("path", path)
                        .with("error", e.to_string())
                })
                .and_then(|xml| {
                    let (sidecar, imported) = read_sidecar(&xml).map_err(Message::plain)?;
                    if let Some(warning) = imported.report.summary() {
                        warn(path, &warning.english);
                    }
                    let (params, mut report) = sidecar_params(&imported, modules);
                    if let Some(vault) = vault {
                        report["vault"] =
                            keep_sidecar(vault, &pending.image, 0, true, &sidecar, &imported, now)
                                .map_err(Message::plain)?;
                    }
                    Ok((params, report))
                });
            match read {
                Ok((p, mut r)) => {
                    r["path"] = json!(path);
                    report = r;
                    params = p;
                }
                Err(e) => {
                    let e = if e.is_keyed() {
                        e
                    } else {
                        Message::plain(format!("{}: {e}", path.display()))
                    };
                    return Err(Box::new((pending, e)));
                }
            }
        }
        if !others.is_empty() {
            let others: Vec<Value> = others
                .iter()
                .map(|file| match vault {
                    Some(vault) => keep_file(vault, &pending.image, file, false, now, &mut warn),
                    None => {
                        json!({ "path": file.path, "version": file.version, "exact": file.exact })
                    }
                })
                .collect();
            if report.is_null() {
                report = json!({});
            }
            report["duplicates"] = json!(others);
        }
        pending.sidecar = report;
    }
    let loaded = node.control(&ControlRequest::LoadSource {
        node: source.into(),
        path: pending.image.clone(),
    });
    if let Err(e) = loaded {
        return Err(Box::new((pending, e)));
    }
    let asked = node.control(&ControlRequest::Export {
        node: target.into(),
        port: "output".into(),
        region: None,
        path: pending.output.clone(),
        format: options.format,
        quality: Some(options.quality),
        params,
    });
    match asked {
        Ok(answer) => {
            let job = answer["job"].as_u64().unwrap_or_default();
            let seq = answer["seq"].as_u64().unwrap_or_default();
            Ok((job, seq, pending))
        }
        Err(e) => Err(Box::new((pending, e))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_image_says_why_in_english_and_by_key() {
        let p = Pending {
            image: "a.png".into(),
            output: "out/a.jpg".into(),
            started: Instant::now(),
            sidecar: Value::Null,
        };
        let failed = line(&p, Some(&cancelled_message()));
        assert_eq!(failed["error"], "cancelled");
        assert_eq!(failed["message"]["key"], "batch-cancelled");
        assert!(failed["output"].is_null());
        let plain = line(&p, Some(&Message::plain("broken.png: not a picture")));
        assert_eq!(plain["error"], "broken.png: not a picture");
        assert!(plain.get("message").is_none());
        assert!(line(&p, None)["error"].is_null());
    }

    #[test]
    fn names_that_collide_get_a_number() {
        let images: Vec<PathBuf> = ["a/x.cr2", "b/x.png", "c/x.nef", "y.png"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let out = Path::new("/out");
        let on_disk = |p: &Path| p == Path::new("/out/y.jpg");
        let paths = output_paths(&images, out, "{stem}.{ext}", "jpg", on_disk);
        let names: Vec<_> = paths.iter().map(|p| p.to_str().unwrap()).collect();
        assert_eq!(
            names,
            ["/out/x.jpg", "/out/x-1.jpg", "/out/x-2.jpg", "/out/y-1.jpg"]
        );
        let plain = output_paths(&images[..2], out, "{stem}", "jpg", |_| false);
        assert_eq!(plain, [out.join("x"), out.join("x-1")]);
    }
}
