//! The broker: finds plugins by their manifest, starts each on its first
//! activation event, routes calls to it with a timeout, restarts it with
//! backoff when it crashes, stops it when idle or when Helios exits, and
//! holds it to its capabilities where the broker controls the access.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value as Json, json};

use crate::importer::{self, Scan};
use crate::manifest::{
    Enforcement, INTERFACES, ImporterDecl, MANIFEST, Manifest, Version, capability,
};
use crate::rpc::{CallError, Cancel, Event, Listener, Process};

/// How long things may take.
#[derive(Clone, Debug)]
pub struct Policy {
    /// The longest a call waits for its answer.
    pub timeout: Duration,
    /// The longest an importer's scan may take (a large catalogue).
    pub scan_timeout: Duration,
    /// The longest `initialize` may take.
    pub start_timeout: Duration,
    /// A plugin unused this long is stopped ([`Broker::stop_idle`]).
    pub idle: Duration,
    /// The wait before the first restart after a crash; it doubles with
    /// each crash in a row, up to `backoff_max`.
    pub backoff: Duration,
    pub backoff_max: Duration,
    /// How long a plugin asked to stop has before it is killed.
    pub grace: Duration,
    /// Added to every plugin's environment.
    pub env: Vec<(String, String)>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            timeout: Duration::from_secs(30),
            scan_timeout: Duration::from_mins(10),
            start_timeout: Duration::from_secs(10),
            idle: Duration::from_mins(5),
            backoff: Duration::from_secs(1),
            backoff_max: Duration::from_mins(1),
            grace: Duration::from_secs(1),
            env: Vec::new(),
        }
    }
}

/// Where a plugin was found; a more specific scope shadows a broader one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Builtin,
    User,
    Project,
}

/// The plugin directories, broadest first: built-in
/// (`<exe>/../share/helios/plugins`), the user's
/// (`$XDG_CONFIG_HOME/helios/plugins`, or `~/.config/helios/plugins`), and
/// the project's (`<project>/.helios/plugins`).
pub fn default_dirs(project: Option<&Path>) -> Vec<(Scope, PathBuf)> {
    let mut dirs = Vec::new();
    if let Some(exe) = std::env::current_exe().ok().and_then(|e| {
        e.parent()
            .map(|p| p.join("..").join("share").join("helios").join("plugins"))
    }) {
        dirs.push((Scope::Builtin, exe));
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    if let Some(config) = config {
        dirs.push((Scope::User, config.join("helios").join("plugins")));
    }
    if let Some(project) = project {
        dirs.push((Scope::Project, project.join(".helios").join("plugins")));
    }
    dirs
}

/// What a plugin says or suffers, for whoever listens
/// ([`Broker::subscribe`]): progress, logs, crashes.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Notice {
    Started {
        plugin: String,
    },
    Stopped {
        plugin: String,
        why: String,
    },
    /// `$/progress` from the plugin: `params` holds `token`, `done`,
    /// `total`, `message`.
    Progress {
        plugin: String,
        params: Json,
    },
    Log {
        plugin: String,
        message: String,
    },
    Crashed {
        plugin: String,
        status: String,
        stderr: Vec<String>,
    },
}

/// What [`Broker::probe`] found of a plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probed {
    /// It runs, or it started and answered its handshake (`initialize`).
    Answers,
    /// Turned off by the user: not started.
    Disabled,
    /// It needs this capability, not granted: not started.
    NotGranted(String),
    /// It did not start or did not answer its handshake: why.
    Failed(String),
}

/// A plugin directory whose manifest was refused.
#[derive(Clone, Debug, Serialize)]
pub struct Rejected {
    pub dir: PathBuf,
    pub scope: Scope,
    pub problems: Vec<String>,
}

struct State {
    enabled: bool,
    granted: BTreeSet<String>,
    process: Option<Arc<Process>>,
    crashes: u32,
    retry_at: Option<Instant>,
    last_used: Instant,
    in_flight: usize,
    last_error: Option<String>,
}

struct Slot {
    manifest: Manifest,
    dir: PathBuf,
    scope: Scope,
    state: Mutex<State>,
}

type Listeners = Arc<Mutex<Vec<Sender<Notice>>>>;

/// Who follows a call's `$/progress`, by its token.
type Watchers = Arc<Mutex<HashMap<String, Arc<dyn Fn(&Json) + Send + Sync>>>>;

fn announce(listeners: &Listeners, notice: Notice) {
    listeners
        .lock()
        .expect("listeners lock")
        .retain(|tx| tx.send(notice.clone()).is_ok());
}

pub struct Broker {
    version: Version,
    policy: Policy,
    plugins: BTreeMap<String, Slot>,
    rejected: Vec<Rejected>,
    listeners: Listeners,
    watchers: Watchers,
}

static NEXT: AtomicU64 = AtomicU64::new(0);

impl Broker {
    pub fn new(policy: Policy) -> Self {
        Broker {
            version: Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate's version"),
            policy,
            plugins: BTreeMap::new(),
            rejected: Vec::new(),
            listeners: Arc::default(),
            watchers: Arc::default(),
        }
    }

    pub fn version(&self) -> Version {
        self.version
    }

    /// Reads the manifests of every plugin directory under each of `dirs`
    /// (missing directories are skipped). Nothing runs yet.
    pub fn discover(&mut self, dirs: &[(Scope, PathBuf)]) {
        for (scope, dir) in dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut found: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.join(MANIFEST).is_file())
                .collect();
            found.sort();
            for p in found {
                let _ = self.add(&p, *scope);
            }
        }
    }

    /// Reads one plugin directory's manifest; a refused one is kept in
    /// [`Broker::rejected`] and its problems returned.
    pub fn add(&mut self, dir: &Path, scope: Scope) -> Result<String, Vec<String>> {
        let manifest = match Manifest::read(dir) {
            Ok(m) => m,
            Err(e) => return Err(self.reject(dir, scope, vec![e])),
        };
        let problems = manifest.problems(self.version);
        if !problems.is_empty() {
            return Err(self.reject(dir, scope, problems));
        }
        let id = manifest.id.clone();
        if let Some(old) = self.plugins.get(&id) {
            if old.scope >= scope {
                let why = format!("`{id}` is already provided by {}", old.dir.display());
                return Err(self.reject(dir, scope, vec![why]));
            }
            let (old_dir, old_scope) = (old.dir.clone(), old.scope);
            let why = format!("shadowed by {}", dir.display());
            self.reject(&old_dir, old_scope, vec![why]);
        }
        self.plugins.insert(
            id.clone(),
            Slot {
                manifest,
                dir: dir.to_path_buf(),
                scope,
                state: Mutex::new(State {
                    enabled: true,
                    granted: BTreeSet::new(),
                    process: None,
                    crashes: 0,
                    retry_at: None,
                    last_used: Instant::now(),
                    in_flight: 0,
                    last_error: None,
                }),
            },
        );
        Ok(id)
    }

    fn reject(&mut self, dir: &Path, scope: Scope, problems: Vec<String>) -> Vec<String> {
        self.rejected.push(Rejected {
            dir: dir.to_path_buf(),
            scope,
            problems: problems.clone(),
        });
        problems
    }

    pub fn rejected(&self) -> &[Rejected] {
        &self.rejected
    }

    pub fn manifest(&self, id: &str) -> Option<&Manifest> {
        self.plugins.get(id).map(|s| &s.manifest)
    }

    /// The directory a plugin was found in.
    pub fn dir(&self, id: &str) -> Option<&Path> {
        self.plugins.get(id).map(|s| s.dir.as_path())
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.plugins.keys().map(String::as_str)
    }

    /// Notices from now on.
    pub fn subscribe(&self) -> Receiver<Notice> {
        let (tx, rx) = channel();
        self.listeners.lock().expect("listeners lock").push(tx);
        rx
    }

    fn slot(&self, id: &str) -> Result<&Slot, String> {
        self.plugins
            .get(id)
            .ok_or_else(|| format!("no plugin `{id}`"))
    }

    /// Every plugin, as `plugin.list` shows it, and the refused ones.
    pub fn list(&self) -> Json {
        let plugins: Vec<Json> = self
            .plugins
            .iter()
            .map(|(id, slot)| {
                let st = slot.state.lock().expect("plugin lock");
                let m = &slot.manifest;
                json!({
                    "id": id,
                    "name": m.name,
                    "title": m.title(),
                    "description": m.description,
                    "version": m.version,
                    "license": m.license,
                    "scope": slot.scope,
                    "dir": slot.dir,
                    "enabled": st.enabled,
                    "running": st.process.as_ref().is_some_and(|p| p.alive()),
                    "crashes": st.crashes,
                    "last_error": st.last_error,
                    "services": m.services,
                    "activation": m.activation,
                    "commands": m.commands,
                    "importers": m.importers,
                    "settings": m.settings,
                    "capabilities": m.capabilities.iter().map(|c| {
                        let (enforcement, about) = capability(&c.name).expect("checked");
                        json!({
                            "name": c.name,
                            "reason": c.reason,
                            "about": about,
                            "enforcement": enforcement,
                            "granted": st.granted.contains(&c.name),
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({"plugins": plugins, "rejected": self.rejected})
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        let slot = self.slot(id)?;
        let mut st = slot.state.lock().expect("plugin lock");
        st.enabled = enabled;
        st.crashes = 0;
        st.retry_at = None;
        if !enabled {
            self.stop_locked(id, &mut st, "disabled");
        }
        Ok(())
    }

    /// Grants a capability the plugin declares. A running plugin is
    /// stopped, so it starts again told of its new grants.
    pub fn grant(&self, id: &str, capability: &str) -> Result<(), String> {
        let slot = self.slot(id)?;
        if !slot.manifest.declares(capability) {
            return Err(format!(
                "`{id}` does not declare the capability `{capability}`"
            ));
        }
        let mut st = slot.state.lock().expect("plugin lock");
        if st.granted.insert(capability.into()) {
            self.stop_locked(id, &mut st, "capabilities changed");
        }
        Ok(())
    }

    pub fn revoke(&self, id: &str, capability: &str) -> Result<(), String> {
        let slot = self.slot(id)?;
        let mut st = slot.state.lock().expect("plugin lock");
        if st.granted.remove(capability) {
            self.stop_locked(id, &mut st, "capabilities changed");
        }
        Ok(())
    }

    pub fn running(&self, id: &str) -> bool {
        self.plugins.get(id).is_some_and(|s| {
            let st = s.state.lock().expect("plugin lock");
            st.process.as_ref().is_some_and(|p| p.alive())
        })
    }

    /// The process id of a running plugin.
    pub fn pid(&self, id: &str) -> Option<u32> {
        let st = self.plugins.get(id)?.state.lock().expect("plugin lock");
        st.process.as_ref().filter(|p| p.alive()).map(|p| p.pid())
    }

    fn stop_locked(&self, id: &str, st: &mut State, why: &str) {
        if let Some(p) = st.process.take() {
            let was_alive = p.alive();
            p.stop(self.policy.grace);
            if was_alive {
                announce(
                    &self.listeners,
                    Notice::Stopped {
                        plugin: id.into(),
                        why: why.into(),
                    },
                );
            }
        }
    }

    /// Stops the plugins left unused longer than the policy's `idle`.
    pub fn stop_idle(&self) {
        for (id, slot) in &self.plugins {
            let mut st = slot.state.lock().expect("plugin lock");
            if st.process.is_some()
                && st.in_flight == 0
                && st.last_used.elapsed() >= self.policy.idle
            {
                self.stop_locked(id, &mut st, "idle");
            }
        }
    }

    /// Whether a plugin starts and answers its handshake, for the health
    /// check: one running already answers; otherwise it is started,
    /// handed `initialize`, then stopped again (a probe keeps nothing
    /// running). A disabled plugin, or one waiting for a capability, is
    /// not started. Also gives its last log lines (standard error of its
    /// last process, its last error).
    pub fn probe(&self, id: &str) -> Result<(Probed, Vec<String>), String> {
        let slot = self.slot(id)?;
        let mut st = slot.state.lock().expect("plugin lock");
        let mut log: Vec<String> = Vec::new();
        if let Some(p) = &st.process {
            if p.alive() {
                return Ok((Probed::Answers, log));
            }
            log.extend(p.stderr_tail());
        }
        if let Some(e) = &st.last_error {
            log.push(e.clone());
        }
        if !st.enabled {
            return Ok((Probed::Disabled, log));
        }
        for c in &slot.manifest.capabilities {
            if capability(&c.name).map(|(e, _)| e) == Some(Enforcement::Ambient)
                && !st.granted.contains(&c.name)
            {
                return Ok((Probed::NotGranted(c.name.clone()), log));
            }
        }
        match self.start(id, slot, &st) {
            Ok(p) => {
                p.stop(self.policy.grace);
                Ok((Probed::Answers, log))
            }
            Err(e) => {
                st.last_error = Some(e.clone());
                log.push(e.clone());
                Ok((Probed::Failed(e), log))
            }
        }
    }

    /// Starts the plugins that ask to run at startup.
    pub fn startup(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for (id, slot) in &self.plugins {
            if slot.manifest.activation.iter().any(|e| e == "onStartup") {
                let mut st = slot.state.lock().expect("plugin lock");
                if st.enabled
                    && let Err(e) = self.ensure(id, slot, &mut st, "onStartup")
                {
                    errors.push(e);
                }
            }
        }
        errors
    }

    /// The running process of a plugin, started for `event` if need be.
    fn ensure(
        &self,
        id: &str,
        slot: &Slot,
        st: &mut State,
        event: &str,
    ) -> Result<Arc<Process>, String> {
        if !st.enabled {
            return Err(format!("plugin `{id}` is disabled"));
        }
        if !slot.manifest.activation.iter().any(|e| e == event) {
            return Err(format!("plugin `{id}` does not start on `{event}`"));
        }
        // What the broker cannot hold a process to, the user must grant
        // before the process runs at all.
        for c in &slot.manifest.capabilities {
            if capability(&c.name).map(|(e, _)| e) == Some(Enforcement::Ambient)
                && !st.granted.contains(&c.name)
            {
                return Err(format!(
                    "plugin `{id}` needs the capability `{}` ({}), not granted: \
                     grant it with `plugin.grant`",
                    c.name, c.reason
                ));
            }
        }
        if let Some(p) = &st.process {
            if p.alive() {
                return Ok(p.clone());
            }
            // It ended on its own: count the crash, wait before the next
            // start, longer after each crash in a row.
            let tail = p.stderr_tail();
            st.process = None;
            self.back_off(st);
            st.last_error = Some(format!("crashed{}", tail_note(&tail)));
        }
        if let Some(at) = st.retry_at {
            let now = Instant::now();
            if at > now {
                return Err(format!(
                    "plugin `{id}` crashed {} time(s) in a row; it restarts in {} ms",
                    st.crashes,
                    (at - now).as_millis()
                ));
            }
        }
        match self.start(id, slot, st) {
            Ok(p) => {
                st.process = Some(p.clone());
                st.retry_at = None;
                announce(&self.listeners, Notice::Started { plugin: id.into() });
                Ok(p)
            }
            Err(e) => {
                self.back_off(st);
                st.last_error = Some(e.clone());
                Err(e)
            }
        }
    }

    /// One more crash in a row: the next start waits longer.
    fn back_off(&self, st: &mut State) {
        st.crashes += 1;
        let wait = self
            .policy
            .backoff
            .saturating_mul(1 << (st.crashes - 1).min(16))
            .min(self.policy.backoff_max);
        st.retry_at = Some(Instant::now() + wait);
    }

    fn start(&self, id: &str, slot: &Slot, st: &State) -> Result<Arc<Process>, String> {
        let m = &slot.manifest;
        let first = &m.command[0];
        let local = slot.dir.join(first);
        // a program built for Windows carries `.exe`, the manifest not
        let local_exe = slot
            .dir
            .join(format!("{first}{}", std::env::consts::EXE_SUFFIX));
        let program = if local.is_file() {
            local
        } else if local_exe.is_file() {
            local_exe
        } else {
            PathBuf::from(first)
        };
        let plugin = id.to_string();
        let listeners = self.listeners.clone();
        let watchers = self.watchers.clone();
        let listener: Listener = Arc::new(move |event| {
            let notice = match event {
                Event::Notification { method, params } => match method.as_str() {
                    "$/progress" => {
                        let watcher = params
                            .get("token")
                            .and_then(Json::as_str)
                            .and_then(|t| watchers.lock().expect("watchers lock").get(t).cloned());
                        if let Some(w) = watcher {
                            w(&params);
                        }
                        Notice::Progress {
                            plugin: plugin.clone(),
                            params,
                        }
                    }
                    // a page of a scan (importer interface 2): its
                    // scan's watcher only
                    "importer/page" => {
                        let watcher = params
                            .get("token")
                            .and_then(Json::as_str)
                            .and_then(|t| watchers.lock().expect("watchers lock").get(t).cloned());
                        if let Some(w) = watcher {
                            w(&params);
                        }
                        return;
                    }
                    "$/log" => Notice::Log {
                        plugin: plugin.clone(),
                        message: params
                            .get("message")
                            .and_then(Json::as_str)
                            .unwrap_or_default()
                            .into(),
                    },
                    _ => return,
                },
                Event::Exited { status, stderr } => Notice::Crashed {
                    plugin: plugin.clone(),
                    status,
                    stderr,
                },
            };
            announce(&listeners, notice);
        });
        let process = Process::spawn(
            &program,
            &m.command[1..],
            &slot.dir,
            &self.policy.env,
            listener,
        )
        .map_err(|e| format!("plugin `{id}` did not start: {e}"))?;
        let interfaces: BTreeMap<&str, &[u32]> = INTERFACES.iter().copied().collect();
        let settings = m.default_settings();
        let init = json!({
            "protocolVersion": 1,
            "helios": {"version": self.version.to_string()},
            "interfaces": interfaces,
            "plugin": {"id": id, "dir": slot.dir},
            "capabilities": {"granted": st.granted},
            "settings": settings,
        });
        match process.call("initialize", init, self.policy.start_timeout) {
            Ok(_) => {
                let _ = process.notify("initialized", json!({}));
                Ok(process)
            }
            Err(e) => {
                process.stop(self.policy.grace);
                Err(format!("plugin `{id}` did not initialize: {e}"))
            }
        }
    }

    /// Calls `method` on a plugin, started for `event` if need be.
    pub fn call(&self, id: &str, event: &str, method: &str, params: Json) -> Result<Json, String> {
        self.call_with(id, event, method, params, self.policy.timeout, None)
    }

    /// As [`Broker::call`], waiting at most `timeout`, given up early when
    /// `cancel` says so.
    pub fn call_with(
        &self,
        id: &str,
        event: &str,
        method: &str,
        params: Json,
        timeout: Duration,
        cancel: Option<&Cancel>,
    ) -> Result<Json, String> {
        self.stop_idle();
        let slot = self.slot(id)?;
        let process = {
            let mut st = slot.state.lock().expect("plugin lock");
            let p = self.ensure(id, slot, &mut st, event)?;
            st.in_flight += 1;
            st.last_used = Instant::now();
            p
        };
        let result = process.call_with(method, params, timeout, cancel);
        let mut st = slot.state.lock().expect("plugin lock");
        st.in_flight -= 1;
        st.last_used = Instant::now();
        match result {
            Ok(v) => {
                st.crashes = 0;
                Ok(v)
            }
            Err(CallError::Stopped(why)) => {
                let e = format!(
                    "plugin `{id}` stopped during `{method}` ({why}); it restarts on the next call"
                );
                st.last_error = Some(e.clone());
                Err(e)
            }
            Err(e) => {
                let e = format!("plugin `{id}`, `{method}`: {e}");
                st.last_error = Some(e.clone());
                Err(e)
            }
        }
    }

    /// The enabled plugins serving `service`.
    pub fn providers(&self, service: &str) -> Vec<&str> {
        self.plugins
            .iter()
            .filter(|(_, s)| {
                s.manifest.services.iter().any(|x| x == service)
                    && s.state.lock().expect("plugin lock").enabled
            })
            .map(|(id, _)| id.as_str())
            .collect()
    }

    /// Refuses unless the plugin declares a brokered capability and the
    /// user granted it.
    fn require(&self, id: &str, capability: &str) -> Result<(), String> {
        let slot = self.slot(id)?;
        if !slot.manifest.declares(capability) {
            return Err(format!(
                "plugin `{id}` does not declare the capability `{capability}`"
            ));
        }
        if !slot
            .state
            .lock()
            .expect("plugin lock")
            .granted
            .contains(capability)
        {
            return Err(format!(
                "plugin `{id}` needs the capability `{capability}`, not granted: \
                 grant it with `plugin.grant`"
            ));
        }
        Ok(())
    }

    /// The importers of the enabled plugins, with their plugin's id, in
    /// the plugins' order.
    pub fn importers(&self) -> Vec<(String, ImporterDecl)> {
        self.plugins
            .iter()
            .filter(|(_, s)| s.state.lock().expect("plugin lock").enabled)
            .flat_map(|(id, s)| {
                s.manifest
                    .importers
                    .iter()
                    .map(move |i| (id.clone(), i.clone()))
            })
            .collect()
    }

    /// Has a plugin's importer list what `source` holds (`importer.scan`),
    /// and checks its answer ([`importer::checked`]). `source` is held to
    /// what the importer reads before the plugin starts; it is the one
    /// path the plugin is handed: picking it is the grant of
    /// `source.read` for this call. The plugin's `$/progress` for this
    /// scan goes to `progress`; `cancel` gives the scan up.
    pub fn importer_scan(
        &self,
        plugin: &str,
        importer: &str,
        source: &Path,
        is_image: &dyn Fn(&Path) -> bool,
        cancel: Option<&Cancel>,
        progress: impl Fn(&Json) + Send + Sync + 'static,
    ) -> Result<Scan, String> {
        let slot = self.slot(plugin)?;
        let decl = slot
            .manifest
            .importer(importer)
            .ok_or_else(|| format!("plugin `{plugin}` has no importer `{importer}`"))?
            .clone();
        if !slot.manifest.declares("source.read") {
            return Err(format!(
                "plugin `{plugin}` does not declare the capability `source.read`"
            ));
        }
        let source = importer::check_source(&decl, source)?;
        let own = format!("onImporter:{importer}");
        let event = if slot.manifest.activation.contains(&own) {
            own
        } else {
            "onService:importer".to_string()
        };
        let token = format!("{plugin}#scan-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let paged = slot
            .manifest
            .compat
            .interfaces
            .get("importer")
            .is_some_and(|v| *v >= 2);
        // Pages as they come, checked once the scan answers.
        let pages: Arc<Mutex<Vec<Json>>> = Arc::default();
        {
            let pages = pages.clone();
            let watcher = move |p: &Json| {
                if let Some(images) = p.get("images") {
                    pages.lock().expect("pages lock").push(images.clone());
                    if p.get("done").is_some() {
                        progress(p);
                    }
                } else {
                    progress(p);
                }
            };
            self.watchers
                .lock()
                .expect("watchers lock")
                .insert(token.clone(), Arc::new(watcher));
        }
        let mut params = json!({
            "importer": importer,
            "source": source,
            "kind": decl.source.as_str(),
            "progress": token,
        });
        if paged {
            params["pageSize"] = json!(importer::PAGE_SIZE);
        }
        let answer = self.call_with(
            plugin,
            &event,
            "importer.scan",
            params,
            self.policy.scan_timeout,
            cancel,
        );
        self.watchers.lock().expect("watchers lock").remove(&token);
        let answer = answer?;
        let received = std::mem::take(&mut *pages.lock().expect("pages lock"));
        let mut scan = if paged && answer.get("pages").is_some() {
            let said = answer["pages"].as_u64().unwrap_or(0);
            if said != received.len() as u64 {
                return Err(format!(
                    "plugin `{plugin}`: the scan said {said} pages, {} came",
                    received.len()
                ));
            }
            let mut checked = importer::Pages::new(&decl, &source);
            for page in &received {
                let list = page
                    .as_array()
                    .ok_or_else(|| format!("plugin `{plugin}`: a page without an `images` list"))?;
                checked.page(list, is_image);
            }
            if let Some(last) = answer.get("images").and_then(Json::as_array) {
                checked.page(last, is_image);
            }
            checked.finish(&answer)
        } else {
            importer::checked(&answer, &decl, &source, is_image)
        }
        .map_err(|e| format!("plugin `{plugin}`: {e}"))?;
        scan.plugin = plugin.into();
        Ok(scan)
    }

    /// Runs a command a plugin contributes.
    pub fn execute(&self, id: &str, command: &str, args: Json) -> Result<Json, String> {
        self.call(
            id,
            &format!("onCommand:{command}"),
            "command/execute",
            json!({"command": command, "args": args}),
        )
    }

    /// Reads an image's metadata through every plugin serving
    /// `metadata.read`. Each plugin is handed that one path, and must hold
    /// `images.read`. Every field says which tag it came from (`source`)
    /// and which plugin read it.
    pub fn metadata_read(&self, image: &Path) -> Result<Json, String> {
        let image = image
            .canonicalize()
            .map_err(|e| format!("{}: {e}", image.display()))?;
        if !image.is_file() {
            return Err(format!("{} is not a file", image.display()));
        }
        let providers = self.providers("metadata.read");
        if providers.is_empty() {
            return Err("no plugin reads metadata".into());
        }
        let mut fields = Vec::new();
        let mut errors = Vec::new();
        for id in providers {
            let answer = self.require(id, "images.read").and_then(|()| {
                self.call(
                    id,
                    "onService:metadata.read",
                    "metadata.read",
                    json!({"image": image}),
                )
            });
            match answer.and_then(|a| checked_fields(id, &a)) {
                Ok(f) => fields.extend(f),
                Err(e) => errors.push(e),
            }
        }
        if fields.is_empty() && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(json!({"image": image, "fields": fields, "errors": errors}))
    }

    fn device_plugin<'a>(&'a self, plugin: Option<&'a str>) -> Result<&'a str, String> {
        let providers = self.providers("device");
        match plugin {
            Some(p) if providers.contains(&p) => Ok(p),
            Some(p) => Err(format!("plugin `{p}` serves no devices (or is disabled)")),
            None => providers
                .first()
                .copied()
                .ok_or_else(|| "no plugin serves devices".into()),
        }
    }

    /// The devices of every plugin serving `device`.
    pub fn device_list(&self) -> Result<Json, String> {
        let mut devices = Vec::new();
        let mut errors = Vec::new();
        for id in self.providers("device") {
            match self.call(id, "onService:device", "device.list", json!({})) {
                Ok(a) => {
                    for mut d in a
                        .get("devices")
                        .and_then(Json::as_array)
                        .cloned()
                        .unwrap_or_default()
                    {
                        if let Some(o) = d.as_object_mut() {
                            o.insert("plugin".into(), json!(id));
                            devices.push(d);
                        }
                    }
                }
                Err(e) => errors.push(e),
            }
        }
        Ok(json!({"devices": devices, "errors": errors}))
    }

    pub fn device_info(&self, plugin: Option<&str>, device: Option<&str>) -> Result<Json, String> {
        let id = self.device_plugin(plugin)?;
        self.call(
            id,
            "onService:device",
            "device.info",
            json!({"device": device}),
        )
    }

    /// Takes a picture and downloads it into `folder`.
    pub fn device_capture(
        &self,
        plugin: Option<&str>,
        device: Option<&str>,
        folder: &Path,
    ) -> Result<Json, String> {
        let id = self.device_plugin(plugin)?;
        // Checked before the shutter fires, not only at download.
        self.require(id, "folder.write")?;
        let captured = self.call(
            id,
            "onService:device",
            "device.capture",
            json!({"device": device}),
        )?;
        let files: Vec<String> = captured
            .get("files")
            .and_then(Json::as_array)
            .ok_or("the plugin's capture answer has no `files` list")?
            .iter()
            .filter_map(|f| f.as_str().map(String::from))
            .collect();
        let mut downloaded = self.device_download(Some(id), device, &files, folder)?;
        downloaded["captured"] = json!(files);
        Ok(downloaded)
    }

    /// Downloads files from a device into `folder`. The plugin writes into
    /// a fresh staging folder only; the broker checks each file it reports
    /// lies there, is a plain file, and moves it into `folder` itself.
    pub fn device_download(
        &self,
        plugin: Option<&str>,
        device: Option<&str>,
        files: &[String],
        folder: &Path,
    ) -> Result<Json, String> {
        let id = self.device_plugin(plugin)?;
        self.require(id, "folder.write")?;
        std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        let folder = folder
            .canonicalize()
            .map_err(|e| format!("{}: {e}", folder.display()))?;
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let staging = folder.join(format!(".helios-incoming-{}-{n}", std::process::id()));
        std::fs::create_dir(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
        let token = format!("{id}#{n}");
        let answer = self.call(
            id,
            "onService:device",
            "device.download",
            json!({"device": device, "files": files, "folder": staging, "progress": token}),
        );
        let result = answer.and_then(|a| collect(&a, &staging, &folder));
        let _ = std::fs::remove_dir_all(&staging);
        result.map(|(done, refused)| json!({"files": done, "refused": refused, "progress": token}))
    }
}

fn tail_note(tail: &[String]) -> String {
    tail.last().map(|l| format!(": {l}")).unwrap_or_default()
}

/// The fields of a `metadata.read` answer, each tagged with its plugin.
fn checked_fields(id: &str, answer: &Json) -> Result<Vec<Json>, String> {
    let list = answer
        .get("fields")
        .and_then(Json::as_array)
        .ok_or_else(|| format!("plugin `{id}`: the answer has no `fields` list"))?;
    list.iter()
        .map(|f| {
            let name = f.get("name").and_then(Json::as_str);
            let source = f.get("source").and_then(Json::as_str);
            match (name, source, f.get("value")) {
                (Some(name), Some(source), Some(value)) => Ok(json!({
                    "name": name, "value": value, "source": source, "plugin": id,
                })),
                _ => Err(format!(
                    "plugin `{id}`: a field needs `name`, `value` and `source`: {f}"
                )),
            }
        })
        .collect()
}

/// Moves the files a download reports from `staging` into `folder`,
/// refusing any that lies elsewhere or is not a plain file.
fn collect(answer: &Json, staging: &Path, folder: &Path) -> Result<(Vec<Json>, Vec<Json>), String> {
    let list = answer
        .get("files")
        .and_then(Json::as_array)
        .ok_or("the download answer has no `files` list")?;
    let staging = staging
        .canonicalize()
        .map_err(|e| format!("{}: {e}", staging.display()))?;
    let mut done = Vec::new();
    let mut refused = Vec::new();
    for f in list {
        let path = PathBuf::from(f.get("path").and_then(Json::as_str).unwrap_or_default());
        let plain = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file());
        let inside = path
            .canonicalize()
            .is_ok_and(|p| p.parent() == Some(staging.as_path()));
        if !plain || !inside {
            refused.push(json!({
                "path": path,
                "why": "not a plain file in the folder handed over",
            }));
            continue;
        }
        let name = path.file_name().expect("a file").to_string_lossy();
        let dest = free_name(folder, &name);
        std::fs::rename(&path, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        done.push(json!({"camera": f.get("camera"), "path": dest}));
    }
    Ok((done, refused))
}

/// `name` in `folder`, numbered if taken: `IMG_1.JPG`, `IMG_1-1.JPG`.
#[allow(
    clippy::maybe_infinite_iter,
    reason = "a folder holds finitely many files, so a free name is found"
)]
fn free_name(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s, format!(".{e}")),
        None => (name, String::new()),
    };
    (1..)
        .map(|i| folder.join(format!("{stem}-{i}{ext}")))
        .find(|p| !p.exists())
        .expect("a free name")
}

impl Drop for Broker {
    /// Helios exits: every plugin is asked to stop.
    fn drop(&mut self) {
        for (id, slot) in &self.plugins {
            let mut st = slot.state.lock().expect("plugin lock");
            self.stop_locked(id, &mut st, "Helios exits");
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn only_plain_files_in_the_staging_folder_are_kept() {
        let root =
            std::env::temp_dir().join(format!("helios-plugin-collect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (staging, folder) = (root.join("dest/.incoming"), root.join("dest"));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("good.jpg"), "picture").unwrap();
        std::fs::write(root.join("secret"), "not yours").unwrap();
        std::os::unix::fs::symlink(root.join("secret"), staging.join("link.jpg")).unwrap();
        std::fs::write(folder.join("good.jpg"), "taken").unwrap();
        let answer = json!({"files": [
            {"camera": "/a/good.jpg", "path": staging.join("good.jpg")},
            {"camera": "/a/link.jpg", "path": staging.join("link.jpg")},
            {"camera": "/a/secret", "path": root.join("secret")},
            {"camera": "/a/sneaky", "path": staging.join("../../secret")},
        ]});
        let (done, refused) = collect(&answer, &staging, &folder).unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0]["path"], json!(folder.join("good-1.jpg")));
        assert_eq!(refused.len(), 3);
        assert_eq!(
            std::fs::read_to_string(root.join("secret")).unwrap(),
            "not yours"
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("good.jpg")).unwrap(),
            "taken"
        );
    }
}
