//! A node's command endpoint on its vault, and its rendezvous (see
//! `docs/protocol.md`, "Finding a node").
//!
//! [`VaultService`] answers the library's commands ([`crate::library`]),
//! the calibration's ([`crate::calibrate`]) and `library.import` over
//! JSON-RPC 2.0, each checked against its
//! schema, run by the origin its call names and announced on the node's
//! bus ([`CommandRan`]). [`advertise`] serves it on a socket of its own
//! (TCP with a session token where there are no Unix sockets) and writes
//! the vault's rendezvous file under the vault's lock, so the `helios`
//! CLI and `helios-mcp` reach this node instead of starting another. The
//! desktop advertises its embedded node this way while it holds the
//! vault; `helios-node` serves the same commands on its batch endpoint.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::jobs::Jobs;
use helios_core::{Bus, Event};
use helios_inspect::discovery::{self, Lock, Rendezvous};
use helios_inspect::rpc::{self, Call, Error, Service};
use helios_inspect::transport::{Connection, Listen};
use helios_library::import::Report;
use helios_vault::{Changes, Store, Vault};
use serde::Serialize;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, validate};
use crate::{calibrate, library};

/// Who publishes the endpoint's events on the bus.
pub const SOURCE: &str = "node";

/// The origin of a call that names none.
pub const CONTROL: &str = "control";

/// A command is run, by whom.
#[derive(Serialize)]
pub struct CommandRan<'a> {
    pub id: &'a str,
    pub origin: &'a str,
}

impl Event for CommandRan<'_> {
    const KIND: &'static str = "control.command";
}

/// An import done through the node ended: its report (the files it
/// added, located, whether it was cancelled...).
#[derive(Serialize)]
pub struct Imported<'a> {
    #[serde(flatten)]
    pub report: &'a Report,
}

impl Event for Imported<'_> {
    const KIND: &'static str = "library.imported";
}

/// The commands a [`VaultService`] runs.
pub fn specs() -> Vec<CommandSpec> {
    std::iter::once(library::import_spec())
        .chain(library::specs())
        .chain(calibrate::specs())
        .collect()
}

/// Runs command `id` of [`specs`] but the import on `vault`, in the node's
/// process: its params checked against its schema, the run announced on
/// `bus` ([`CommandRan`]) as made by `origin`. The desktop runs its
/// library's and calibration's commands through here, as the CLI and
/// agents reach them over the endpoint. Answers what the vault changed
/// too, for a history to undo.
///
/// # Errors
/// An unknown command, params its schema refuses, or the command's
/// refusal.
pub fn command<S: Store>(
    vault: &Vault<S>,
    bus: &Arc<Bus>,
    id: &str,
    params: &Value,
    origin: &str,
) -> Result<(Value, Changes), String> {
    let Some(spec) = specs()
        .into_iter()
        .find(|s| s.id == id && s.id != library::IMPORT)
    else {
        return Err(format!("no command `{id}` on the vault"));
    };
    let mut checked = params.clone();
    if let Some(fields) = checked.as_object_mut() {
        fields.remove("_meta");
    }
    validate(&spec.params, &checked).map_err(|e| format!("`{id}` refused its params: {e}"))?;
    bus.publish(SOURCE, None, &CommandRan { id, origin });
    run_on(vault, bus, id, &checked, origin)
}

/// Runs a library or calibration command whose params are checked.
fn run_on<S: Store>(
    vault: &Vault<S>,
    bus: &Arc<Bus>,
    id: &str,
    params: &Value,
    origin: &str,
) -> Result<(Value, Changes), String> {
    if calibrate::handles(id) {
        return calibrate::run(Some(vault), id, params, now())
            .map(|done| (done.answer, done.changes))
            .map_err(|m| m.english);
    }
    library::run(vault, bus, id, params, origin).map(|answer| (answer, Changes::default()))
}

/// Whether `origin` names who runs a call: `user`, `cli`, `control`,
/// `agent:NAME` or `script:NAME`.
fn is_origin(origin: &str) -> bool {
    matches!(origin, "user" | "cli" | "control")
        || ["agent:", "script:"]
            .iter()
            .any(|p| origin.strip_prefix(p).is_some_and(|name| !name.is_empty()))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The library's commands on a node's vault, as JSON-RPC 2.0.
pub struct VaultService<S: Store> {
    vault: Arc<Vault<S>>,
    bus: Arc<Bus>,
    jobs: Jobs,
    instance: String,
}

impl<S: Store> VaultService<S> {
    /// The commands on `vault`, its changes and imports announced on
    /// `bus`, its imports run as jobs on `jobs`; `node.hello` names the
    /// node `instance`.
    pub fn new(vault: Arc<Vault<S>>, bus: Arc<Bus>, jobs: Jobs, instance: &str) -> Self {
        Self {
            vault,
            bus,
            jobs,
            instance: instance.into(),
        }
    }

    fn run(&self, id: &str, params: &Value, call: &Call) -> Result<Value, String> {
        if id != library::IMPORT {
            return run_on(&self.vault, &self.bus, id, params, origin_of(params))
                .map(|(answer, _)| answer);
        }
        let paths: Vec<PathBuf> = params["paths"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .collect();
        let options = library::import_options(params, Some(call.cancel_flag()))?;
        let mut files = Vec::new();
        let report = helios_library::import::import(
            &self.vault,
            &self.jobs,
            &paths,
            &options,
            now(),
            &mut |line| files.push(line.clone()),
        )?;
        self.bus
            .publish(SOURCE, None, &Imported { report: &report });
        Ok(json!({ "report": report, "files": files }))
    }
}

/// The origin a call's params carry (`_meta.origin`), else [`CONTROL`].
fn origin_of(params: &Value) -> &str {
    params["_meta"]["origin"].as_str().unwrap_or(CONTROL)
}

impl<S: Store + Send + 'static> Service for VaultService<S> {
    fn call(&self, method: &str, params: Value, call: &Call) -> Result<Value, Error> {
        match method {
            "node.hello" => return Ok(rpc::hello(&self.instance, "batch", &["batch"])),
            "commands.list" => {
                let commands: Vec<Value> = specs()
                    .iter()
                    .map(|s| {
                        let mut c = serde_json::to_value(s).expect("specs serialize");
                        c["enabled"] = json!(true);
                        c
                    })
                    .collect();
                return Ok(json!({ "commands": commands }));
            }
            _ => {}
        }
        let Some(spec) = specs().into_iter().find(|s| s.id == method) else {
            return Err(Error::method_not_found(method));
        };
        let origin = origin_of(&params).to_string();
        if !is_origin(&origin) {
            return Err(Error::new(
                rpc::code::ORIGIN_REFUSED,
                format!(
                    "`{origin}` is not an origin: user, cli, control, agent:NAME or script:NAME"
                ),
            ));
        }
        let mut checked = params.clone();
        if let Some(fields) = checked.as_object_mut() {
            fields.remove("_meta");
        }
        validate(&spec.params, &checked)
            .map_err(|e| Error::invalid_params(format!("`{method}` refused its params: {e}")))?;
        self.bus.publish(
            SOURCE,
            None,
            &CommandRan {
                id: method,
                origin: &origin,
            },
        );
        match self.run(method, &params, call) {
            Ok(mut answer) => {
                answer["command"] = json!(method);
                answer["origin"] = json!(origin);
                Ok(answer)
            }
            Err(e) if call.is_cancelled() => {
                Err(Error::cancelled().with_data(json!({ "error": e })))
            }
            Err(e) => Err(Error::new(rpc::code::REFUSED, e)),
        }
    }

    /// An import stops before its next file.
    fn cancellable(&self, method: &str) -> bool {
        method == library::IMPORT
    }
}

/// A node advertised to the clients of its vault: its command endpoint
/// served, its rendezvous file written, the vault's lock held. Dropped,
/// the rendezvous file goes first, then the endpoint's socket and the
/// lock.
#[derive(Debug)]
pub struct Advertised {
    rendezvous: PathBuf,
    dir: PathBuf,
    address: String,
    _lock: Lock,
}

impl Advertised {
    /// The command endpoint's address.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The rendezvous file.
    pub fn rendezvous(&self) -> &Path {
        &self.rendezvous
    }
}

impl Drop for Advertised {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.rendezvous);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A private folder for the endpoint's socket and token: Unix socket
/// paths are short, and a vault's folder may be deep.
fn runtime_dir() -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let dir = std::env::temp_dir().join(format!("helios-node-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(dir)
}

/// Serves `service` on an endpoint of its own and writes the rendezvous
/// file of the vault at `vault`, under the vault's lock. `None` when
/// another node holds the vault: this one is not advertised.
pub fn advertise(
    vault: &Path,
    service: Arc<dyn Service>,
    instance: &str,
) -> Result<Option<Advertised>, String> {
    let files = discovery::files(vault);
    let lock = match Lock::try_take(&files.lock) {
        Ok(Some(lock)) => lock,
        Ok(None) => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", files.lock.display())),
    };
    let dir = runtime_dir()?;
    let served = (|| {
        let (listen, token) = if cfg!(unix) {
            (Listen::Unix(dir.join("commands.sock")), None)
        } else {
            let token = helios_inspect::new_token();
            let file = dir.join("token");
            helios_inspect::transport::write_token(&file, &token)
                .map_err(|e| format!("{}: {e}", file.display()))?;
            (Listen::Tcp(([127, 0, 0, 1], 0).into()), Some((token, file)))
        };
        // Lines that are not JSON-RPC are refused.
        let older = |_: Value, _: &mut Connection<'_>| {
            Some(json!({ "ok": false, "error": "this endpoint speaks JSON-RPC 2.0 only" }))
        };
        let bound = helios_inspect::transport::serve_rpc_at(
            &listen,
            token.as_ref().map(|(t, _)| t.as_str()),
            older,
            service,
        )
        .map_err(|e| format!("cannot serve the node's commands: {e}"))?;
        let rendezvous = Rendezvous {
            address: bound.address.clone(),
            // The commands' endpoint answers `node.hello` too.
            inspect: bound.address.clone(),
            token_file: token.map(|(_, file)| file),
            instance: instance.into(),
            protocol: rpc::PROTOCOL.into(),
            pid: std::process::id(),
            vault: std::path::absolute(vault).unwrap_or_else(|_| vault.to_path_buf()),
        };
        rendezvous
            .write(&files.rendezvous)
            .map_err(|e| format!("{}: {e}", files.rendezvous.display()))?;
        Ok::<_, String>(bound.address)
    })();
    match served {
        Ok(address) => Ok(Some(Advertised {
            rendezvous: files.rendezvous,
            dir,
            address,
            _lock: lock,
        })),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use helios_inspect::client::Client;

    use super::*;

    #[test]
    fn origins_are_checked() {
        for ok in ["user", "cli", "control", "agent:mcp", "script:x"] {
            assert!(is_origin(ok), "{ok}");
        }
        for bad in ["", "agent:", "robot"] {
            assert!(!is_origin(bad), "{bad}");
        }
    }

    #[test]
    fn the_service_runs_the_library_commands_and_announces_them() {
        let bus = Arc::new(Bus::new());
        let heard = bus.subscribe();
        let service = VaultService::new(
            Arc::new(Vault::new(helios_vault::MemoryStore::default())),
            bus.clone(),
            Jobs::new(bus.clone()),
            "test",
        );
        let client = Client::in_process(Arc::new(service)).with_origin("cli");
        assert_eq!(client.hello().unwrap()["endpoint"], "batch");
        let listed = client.request("library.stacks.list", json!({})).unwrap();
        assert_eq!(listed["stacks"], json!([]));
        assert_eq!(listed["origin"], "cli");
        let ran = heard.try_recv().unwrap();
        assert_eq!(ran.kind, "control.command");
        assert_eq!(ran.payload["origin"], "cli");
        let refused = client
            .request("library.edit.head", json!({ "image": 3 }))
            .unwrap_err();
        assert!(
            refused.message.contains("refused its params"),
            "{refused:?}"
        );
        let unknown = client.request("export", json!({})).unwrap_err();
        assert_eq!(unknown.code, rpc::code::METHOD_NOT_FOUND);
        let commands = client.request("commands.list", json!({})).unwrap();
        assert_eq!(
            commands["commands"].as_array().unwrap().len(),
            1 + library::IDS.len()
        );
    }
}
