//! A node's command endpoint on its vault, and its rendezvous (see
//! `docs/protocol.md`, "Finding a node").
//!
//! [`VaultService`] answers the library's commands ([`crate::library`]),
//! the calibration's ([`crate::calibrate`]), the init steps' ([`crate::init`]) and `library.import` over
//! JSON-RPC 2.0, each checked against its
//! schema, run by the origin its call names and announced on the node's
//! bus ([`CommandRan`]). [`advertise`] serves it on a socket of its own
//! (TCP with a session token where there are no Unix sockets) and writes
//! the vault's rendezvous file under the vault's lock, so the `helios`
//! CLI and `helios-mcp` reach this node instead of starting another. The
//! desktop advertises its embedded node this way while it holds the
//! vault; `helios-node` serves the same commands on its batch endpoint.
//!
//! A desktop adds its windows' commands ([`WindowCommands`]): `commands.list`
//! lists the node's (`scope: "node"`) and a window's (`scope: "window"`)
//! in one list, each saying whether it can run there and why not, and
//! `command.run` runs either by its id, a window's on the window's own
//! thread, refused with its reason where it does not apply.

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
use crate::{calibrate, cursor, init, library, promises, reads, workshops};

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

/// A command of a desktop window, as that window lists it: its id, label
/// and category as the menus and the palette show them, the schema of its
/// params (an open object when it declares none: its label names its
/// arguments), and whether it applies in the window asked about, with the
/// reason when it does not (its menu entry's greyed tooltip).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WindowCommand {
    pub id: String,
    pub label: String,
    pub category: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub params: Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    pub enabled: bool,
    #[serde(rename = "disabledReason", skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

impl WindowCommand {
    /// Whether every word of `filter` is in its id, label or category.
    pub fn matches(&self, filter: &str) -> bool {
        let hay = format!("{} {} {}", self.id, self.label, self.category).to_lowercase();
        filter
            .split_whitespace()
            .all(|word| hay.contains(&word.to_lowercase()))
    }
}

/// The window a call names when it names none.
pub const MAIN_WINDOW: &str = "main";

/// A desktop's windows' commands, reached from the endpoint's threads:
/// the desktop answers on its interface's thread.
pub trait WindowCommands: Send + Sync {
    /// The commands, evaluated in window `window`'s context.
    ///
    /// # Errors
    /// No such window, or the interface did not answer.
    fn list(&self, window: &str) -> Result<Vec<WindowCommand>, String>;

    /// Runs command `id` in window `window` as `origin` (checked).
    ///
    /// # Errors
    /// An unknown command, one that does not apply there (its reason),
    /// no such window open, or the command's refusal.
    fn run(&self, id: &str, params: Value, origin: &str, window: &str) -> Result<Value, String>;
}

/// The commands a [`VaultService`] runs.
pub fn specs() -> Vec<CommandSpec> {
    std::iter::once(library::import_spec())
        .chain(library::specs())
        .chain(cursor::specs())
        .chain(promises::specs())
        .chain(calibrate::specs())
        .chain(init::specs())
        .chain(reads::specs())
        .chain(workshops::specs())
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
    let Some(spec) = specs().into_iter().find(|s| {
        s.id == id
            && s.id != library::IMPORT
            && s.id != library::AGAIN
            && !cursor::handles(s.id.as_str())
            && !promises::handles(s.id.as_str())
    }) else {
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
        return calibrate::run_as(Some(vault), id, params, origin, now())
            .map(|done| (done.answer, done.changes))
            .map_err(|m| m.english);
    }
    if init::handles(id) {
        return init::run(vault, id, params).map(|answer| (answer, Changes::default()));
    }
    if reads::handles(id) {
        return reads::run(vault, id, params).map(|answer| (answer, Changes::default()));
    }
    if workshops::handles(id) {
        return workshops::run(vault, id, params).map(|answer| (answer, Changes::default()));
    }
    library::run(vault, bus, id, params, origin).map(|answer| (answer, Changes::default()))
}

/// Whether `origin` names who runs a call: `user`, `cli`, `control`,
/// `midi`, `agent:NAME`, `script:NAME` or `remote:NAME`.
fn is_origin(origin: &str) -> bool {
    matches!(origin, "user" | "cli" | "control" | "midi")
        || ["agent:", "script:", "remote:"]
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
    windows: Option<Arc<dyn WindowCommands>>,
    cursors: cursor::Cursors,
    promises: Arc<promises::Promises<S>>,
}

impl<S: Store + 'static> VaultService<S> {
    /// The commands on `vault`, its changes and imports announced on
    /// `bus`, its imports run as jobs on `jobs`; `node.hello` names the
    /// node `instance`.
    pub fn new(vault: Arc<Vault<S>>, bus: Arc<Bus>, jobs: Jobs, instance: &str) -> Self {
        Self {
            promises: Arc::new(crate::import_promise::register(
                promises::Promises::new(vault.clone(), jobs.clone()),
                bus.clone(),
            )),
            vault,
            bus,
            jobs,
            instance: instance.into(),
            windows: None,
            cursors: cursor::Cursors::default(),
        }
    }

    /// Answers the promise commands with `promises`, the node's (with the
    /// kinds it runs), rather than one of its own (which runs imports
    /// only). An import by rule starts on them.
    #[must_use]
    pub fn with_promises(mut self, promises: Arc<promises::Promises<S>>) -> Self {
        self.promises = promises;
        self
    }

    /// Serves a desktop's windows' commands too.
    #[must_use]
    pub fn with_windows(mut self, windows: Arc<dyn WindowCommands>) -> Self {
        self.windows = Some(windows);
        self
    }

    /// The cursors its clients hold open.
    pub fn cursors(&self) -> &cursor::Cursors {
        &self.cursors
    }
}

impl<S: Store + Send + 'static> VaultService<S> {
    /// `commands.list`: the node's commands, then window `window`'s
    /// (`main` when not given), each with its scope; `filter` keeps those
    /// whose id, label or category holds its every word, `enabled_only`
    /// those that can run. A window that cannot answer leaves its
    /// commands out and says why (`window_error`).
    fn list(&self, params: &Value) -> Value {
        let filter = params["filter"].as_str().unwrap_or("");
        let enabled_only = params["enabled_only"].as_bool().unwrap_or(false);
        let window = params["window"].as_str().unwrap_or(MAIN_WINDOW);
        let mut commands: Vec<Value> = specs()
            .iter()
            .filter(|s| s.matches(filter))
            .map(|s| {
                let mut c = serde_json::to_value(s).expect("specs serialize");
                c["enabled"] = json!(true);
                c["scope"] = json!("node");
                c
            })
            .collect();
        let mut answer = json!({});
        if let Some(windows) = &self.windows {
            match windows.list(window) {
                Ok(listed) => commands.extend(
                    listed
                        .iter()
                        .filter(|c| c.matches(filter) && (c.enabled || !enabled_only))
                        .map(|c| {
                            let mut v = serde_json::to_value(c).expect("commands serialize");
                            v["scope"] = json!("window");
                            v["window"] = json!(window);
                            v
                        }),
                ),
                Err(e) => answer["window_error"] = json!(e),
            }
        }
        answer["commands"] = json!(commands);
        answer
    }

    /// `command.run`: command `id` with its `params`, as `origin` (or the
    /// call's `_meta.origin`). A node command runs as its method would; a
    /// window's (`scope: "window"`, or an id only a window knows) runs in
    /// window `window` (`main` when not given), refused with the reason
    /// it is greyed there.
    fn run_command(&self, params: &Value, call: &Call) -> Result<Value, Error> {
        let id = params["id"]
            .as_str()
            .ok_or_else(|| Error::invalid_params("`command.run` takes an `id`"))?;
        let origin = params["origin"]
            .as_str()
            .unwrap_or_else(|| origin_of(params))
            .to_string();
        let mut args = match &params["params"] {
            Value::Null => json!({}),
            v => v.clone(),
        };
        let scope = params["scope"].as_str();
        let node = specs().iter().any(|s| s.id == id);
        if scope != Some("window") && node {
            if let Some(fields) = args.as_object_mut() {
                fields.insert("_meta".into(), json!({ "origin": origin }));
            }
            return self.call(id, args, call);
        }
        let Some(windows) = self.windows.as_ref().filter(|_| scope != Some("node")) else {
            return Err(Error::method_not_found(id));
        };
        if !is_origin(&origin) {
            return Err(refused_origin(&origin));
        }
        let window = params["window"].as_str().unwrap_or(MAIN_WINDOW);
        self.bus.publish(
            SOURCE,
            None,
            &CommandRan {
                id,
                origin: &origin,
            },
        );
        let mut answer = windows
            .run(id, args, &origin, window)
            .map_err(|e| Error::new(rpc::code::REFUSED, e))?;
        if !answer.is_object() {
            answer = json!({ "result": answer });
        }
        answer["command"] = json!(id);
        answer["origin"] = json!(origin);
        answer["window"] = json!(window);
        Ok(answer)
    }

    fn run(&self, id: &str, params: &Value, call: &Call) -> Result<Value, String> {
        let origin = origin_of(params).to_string();
        // An import of the history run again is that import, as asked now.
        let again;
        let params = if id == library::AGAIN {
            again = library::again_params(&self.vault, params)?;
            &again
        } else if id != library::IMPORT {
            return run_on(&self.vault, &self.bus, id, params, origin_of(params))
                .map(|(answer, _)| answer);
        } else if crate::import_promise::by_rule(params) {
            // An import by rule answers at once with its promise.
            let promise = crate::import_promise::start(&self.promises, params, &origin)?;
            return Ok(json!({ "promise": promise }));
        } else {
            params
        };
        let paths: Vec<PathBuf> = params["paths"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .collect();
        if paths.is_empty() {
            return Err("`library.import` needs `paths`, or a `source` and a `rule`".into());
        }
        let mut options = library::import_options(params, Some(call.cancel_flag()))?;
        options.origin = Some(origin);
        let mut files = Vec::new();
        let mut report = helios_library::import::import(
            &self.vault,
            &self.jobs,
            &paths,
            &options,
            now(),
            &mut |line| files.push(line.clone()),
        )?;
        // The cameras' default looks on the raws brought in, as the
        // desktop's import puts them.
        let looks = if params["camera_look"] == false {
            0
        } else {
            crate::calibrate::look::default_on_import(
                &self.vault,
                &mut report.changes,
                now(),
                &crate::calibrate::look::raw_camera,
            )
            .map_err(|m| m.english)?
        };
        self.bus
            .publish(SOURCE, None, &Imported { report: &report });
        Ok(json!({ "report": report, "files": files, "camera_looks": looks }))
    }
}

/// A call refused for the origin it names.
fn refused_origin(origin: &str) -> Error {
    Error::new(
        rpc::code::ORIGIN_REFUSED,
        format!("`{origin}` is not an origin: user, cli, control, agent:NAME or script:NAME"),
    )
}

/// The origin a call's params carry (`_meta.origin`), else [`CONTROL`].
fn origin_of(params: &Value) -> &str {
    params["_meta"]["origin"].as_str().unwrap_or(CONTROL)
}

impl<S: Store + Send + 'static> Service for VaultService<S> {
    fn call(&self, method: &str, params: Value, call: &Call) -> Result<Value, Error> {
        match method {
            "node.hello" => return Ok(rpc::hello(&self.instance, "batch", &["batch"])),
            "commands.list" => return Ok(self.list(&params)),
            "command.run" => return self.run_command(&params, call),
            _ => {}
        }
        let Some(spec) = specs().into_iter().find(|s| s.id == method) else {
            return Err(Error::method_not_found(method));
        };
        let origin = origin_of(&params).to_string();
        if !is_origin(&origin) {
            return Err(refused_origin(&origin));
        }
        let mut checked = params.clone();
        if let Some(fields) = checked.as_object_mut() {
            fields.remove("_meta");
        }
        validate(&spec.params, &checked)
            .map_err(|e| Error::invalid_params(format!("`{method}` refused its params: {e}")))?;
        if cursor::handles(method) {
            // Reads, announced on no bus: a page turned is not a change.
            let mut answer = self.cursors.run(
                &self.vault,
                method,
                &checked,
                call,
                std::time::Instant::now(),
            )?;
            answer["command"] = json!(method);
            answer["seq"] = json!(self.bus.last_seq());
            return Ok(answer);
        }
        if promises::handles(method) {
            if spec.read_only {
                let mut answer = self.promises.run(method, &checked, call)?;
                answer["command"] = json!(method);
                answer["seq"] = json!(self.bus.last_seq());
                return Ok(answer);
            }
            self.bus.publish(
                SOURCE,
                None,
                &CommandRan {
                    id: method,
                    origin: &origin,
                },
            );
            let mut answer = self.promises.run(method, &checked, call)?;
            answer["command"] = json!(method);
            answer["origin"] = json!(origin);
            answer["seq"] = json!(self.bus.last_seq());
            return Ok(answer);
        }
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
                // The bus's sequence once it ran: every event of a write
                // is at or below it, so a client whose cache has seen it
                // shows the write (docs/protocol.md, "A client cache").
                answer["seq"] = json!(self.bus.last_seq());
                Ok(answer)
            }
            Err(e) if call.is_cancelled() => {
                Err(Error::cancelled().with_data(json!({ "error": e })))
            }
            Err(e) => Err(Error::new(rpc::code::REFUSED, e)),
        }
    }

    /// An import stops before its next file, a cursor window between
    /// chunks of rows, a promise's wait at once.
    fn cancellable(&self, method: &str) -> bool {
        method == library::IMPORT
            || method == library::AGAIN
            || method == "cursor.window"
            || method == "promises.wait"
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
        for ok in [
            "user",
            "cli",
            "control",
            "midi",
            "agent:mcp",
            "script:x",
            "remote:phone",
        ] {
            assert!(is_origin(ok), "{ok}");
        }
        for bad in ["", "agent:", "remote:", "robot"] {
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
                + cursor::IDS.len()
                + promises::IDS.len()
                + calibrate::specs().len()
                + init::specs().len()
                + reads::IDS.len()
                + workshops::IDS.len()
        );
    }

    /// A window with one command that applies and one that does not.
    struct Windows;

    impl WindowCommands for Windows {
        fn list(&self, window: &str) -> Result<Vec<WindowCommand>, String> {
            let command = |id: &str, reason: Option<&str>| WindowCommand {
                id: id.into(),
                label: id.into(),
                category: "View".into(),
                description: String::new(),
                params: json!({ "type": "object" }),
                keys: Vec::new(),
                icon: None,
                read_only: false,
                enabled: reason.is_none(),
                disabled_reason: reason.map(Into::into),
            };
            let export = (window != "main").then_some("Only in the Helios window");
            Ok(vec![command("view.zoom", None), command("export", export)])
        }

        fn run(&self, id: &str, _: Value, origin: &str, window: &str) -> Result<Value, String> {
            let listed = self.list(window)?;
            let c = listed.iter().find(|c| c.id == id).ok_or("no command")?;
            match &c.disabled_reason {
                Some(reason) => Err(reason.clone()),
                None => Ok(json!({ "ran": id, "as": origin })),
            }
        }
    }

    #[test]
    fn node_and_window_commands_are_one_list_and_run_by_id() {
        let bus = Arc::new(Bus::new());
        let heard = bus.subscribe();
        let service = VaultService::new(
            Arc::new(Vault::new(helios_vault::MemoryStore::default())),
            bus.clone(),
            Jobs::new(bus.clone()),
            "test",
        )
        .with_windows(Arc::new(Windows));
        let client = Client::in_process(Arc::new(service)).with_origin("agent:test");
        let listed = client.request("commands.list", json!({})).unwrap();
        let commands = listed["commands"].as_array().unwrap();
        let scoped = |id: &str| commands.iter().find(|c| c["id"] == id).unwrap().clone();
        assert_eq!(scoped("library.stacks.list")["scope"], "node");
        assert_eq!(scoped("view.zoom")["scope"], "window");
        assert_eq!(scoped("export")["enabled"], true);
        let import = client
            .request(
                "commands.list",
                json!({ "window": "import", "filter": "export" }),
            )
            .unwrap();
        let window: Vec<&Value> = import["commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["scope"] == "window")
            .collect();
        assert_eq!(window.len(), 1, "{import}");
        assert_eq!(window[0]["window"], "import");
        assert_eq!(window[0]["disabledReason"], "Only in the Helios window");
        let enabled = client
            .request(
                "commands.list",
                json!({ "window": "import", "enabled_only": true }),
            )
            .unwrap();
        assert!(
            !enabled["commands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["id"] == "export")
        );
        let refused = client
            .request(
                "command.run",
                json!({ "id": "export", "window": "import", "origin": "agent:test" }),
            )
            .unwrap_err();
        assert_eq!(refused.code, rpc::code::REFUSED);
        assert_eq!(refused.message, "Only in the Helios window");
        let ran = client
            .request(
                "command.run",
                json!({ "id": "view.zoom", "origin": "agent:test" }),
            )
            .unwrap();
        assert_eq!(ran["as"], "agent:test");
        assert_eq!(ran["window"], "main");
        let events: Vec<_> = std::iter::from_fn(|| heard.try_recv().ok()).collect();
        assert!(
            events.iter().any(|e| e.kind == "control.command"
                && e.payload["id"] == "view.zoom"
                && e.payload["origin"] == "agent:test"),
            "{events:?}"
        );
        // A node command runs through `command.run` as its method would.
        let stacks = client
            .request("command.run", json!({ "id": "library.stacks.list" }))
            .unwrap();
        assert_eq!(stacks["stacks"], json!([]));
        assert_eq!(stacks["origin"], "agent:test");
        let bad = client
            .request(
                "command.run",
                json!({ "id": "view.zoom", "origin": "robot" }),
            )
            .unwrap_err();
        assert_eq!(bad.code, rpc::code::ORIGIN_REFUSED);
    }
}
