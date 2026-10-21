//! The workspace open on the desktop, kept live with its files (see
//! `docs/templates.md`, "Editing a workspace"):
//!
//! - **Agents go through the desktop.** While a desktop holds a workspace
//!   open, it serves the workspace's requests on an endpoint of its own
//!   (JSON lines, as a node's inspect endpoint: a Unix socket in the
//!   user's runtime directory, loopback TCP with a session token where
//!   there are none) and says where in the workspace's files
//!   (`workspace::HOLDER`). The node's MCP tools (`helios_workspace_*`)
//!   find it there and send their request rather than writing the files
//!   under the desktop: an edit, a reset or an import is one step of the
//!   desktop's Edit > Undo, a dry run answers from what the desktop
//!   shows. The requests are answered on the UI thread, at the next
//!   frame; one the desktop has not answered in [`ANSWER_DEADLINE`] is
//!   dropped and the agent told.
//! - **Files changed outside it are reloaded.** A thread watches the
//!   workspace's files and asks for a frame when they change; the desktop
//!   then compares them with what it last wrote there and, when they
//!   differ, takes the change in as one undo step, part by part
//!   (`workspace::merge`): a part only the files changed is taken, a part
//!   only the desktop changed is kept, and a part both changed is taken
//!   from the files, the desktop's own version one Edit > Undo back, and
//!   a warning says which. The same check runs before each of the
//!   desktop's own writes, so a save never writes over a change it has
//!   not seen.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use helios_core::Message;
use helios_shell::commands::Origin;
use helios_template::workspace::{self, Files, Holder, State};
use serde_json::{Value, json};

use super::{Shell, lock};

/// How long an agent's request waits for the desktop to answer it.
pub const ANSWER_DEADLINE: Duration = Duration::from_secs(20);
/// How often the watcher looks at the workspace's files.
const WATCH_EVERY: Duration = Duration::from_millis(500);
/// Where a change found in the files comes from, on the undo step.
const FILES: &str = "workspace files";
/// Who an agent's request is run as.
const AGENT: &str = "mcp";

/// An agent's request waiting for the UI thread.
struct Pending {
    request: Value,
    reply: Sender<Value>,
    /// The agent stopped waiting: not run.
    dropped: Arc<AtomicBool>,
}

/// What the endpoint's and the watcher's threads share with the shell.
#[derive(Default)]
struct Shared {
    pending: Mutex<VecDeque<Pending>>,
    arrived: Condvar,
    /// The watcher saw the files change.
    changed: AtomicBool,
    /// The desktop is closing: requests are sent back to the files.
    closed: AtomicBool,
    waker: Mutex<Option<egui::Context>>,
}

impl Shared {
    fn wake(&self) {
        if let Some(ctx) = lock(&self.waker).as_ref() {
            ctx.request_repaint();
        }
    }
}

/// The desktop's hold on its workspace: its endpoint and the watcher.
pub(super) struct Live {
    files: Files,
    holder: Option<Holder>,
    shared: Arc<Shared>,
}

/// Each endpoint of this process gets its own name.
static ENDPOINTS: AtomicU32 = AtomicU32::new(0);

/// Where a Unix endpoint goes: the user's runtime directory, else the
/// temporary one.
#[cfg(unix)]
fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let n = ENDPOINTS.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("helios-desktop-{}-{n}.sock", std::process::id()))
}

/// The files the watcher looks at, with when each last changed and its
/// size: the workspace's state and its pointer (its layout is egui-pop's
/// to save, and a change made outside comes with the state).
fn fingerprint(files: &Files) -> Vec<(PathBuf, Option<SystemTime>, u64)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(PathBuf, Option<SystemTime>, u64)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            match e.metadata() {
                Ok(m) if m.is_dir() => walk(&path, out),
                Ok(m) => out.push((path, m.modified().ok(), m.len())),
                Err(_) => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&files.own.join(workspace::STATE_DIR), &mut out);
    if let Ok(m) = std::fs::metadata(files.own.join(workspace::POINTER)) {
        out.push((
            files.own.join(workspace::POINTER),
            m.modified().ok(),
            m.len(),
        ));
    }
    out.sort();
    out
}

impl Live {
    /// Starts serving the workspace's requests and watching its files.
    /// Without an endpoint (none could be opened) the files are still
    /// watched, and agents write them.
    pub(super) fn start(files: &Files) -> (Live, Option<String>) {
        let shared = Arc::new(Shared::default());
        let (holder, problem) = match serve(&shared) {
            Ok(holder) => match files.hold(&holder) {
                Ok(()) => (Some(holder), None),
                Err(e) => (None, Some(e)),
            },
            Err(e) => (None, Some(e)),
        };
        let watched = files.clone();
        let watcher = Arc::downgrade(&shared);
        std::thread::spawn(move || {
            // A change is said once the files stay as they are for one
            // look: a save under way is never read half written.
            let mut last = fingerprint(&watched);
            let mut seen = last.clone();
            loop {
                std::thread::sleep(WATCH_EVERY);
                let Some(shared) = watcher.upgrade() else {
                    return;
                };
                if shared.closed.load(Ordering::Relaxed) {
                    return;
                }
                let now = fingerprint(&watched);
                if now != seen {
                    seen = now;
                } else if now != last {
                    last = now;
                    {
                        let _pending = lock(&shared.pending);
                        shared.changed.store(true, Ordering::Relaxed);
                        shared.arrived.notify_all();
                    }
                    shared.wake();
                }
            }
        });
        (
            Live {
                files: files.clone(),
                holder,
                shared,
            },
            problem,
        )
    }

    /// Where agents reach the desktop for this workspace.
    pub(super) fn holder(&self) -> Option<&Holder> {
        self.holder.as_ref()
    }

    /// Stops serving: the requests waiting are sent back to the files,
    /// and the workspace is no longer said to be held.
    pub(super) fn close(&mut self) {
        if self.shared.closed.swap(true, Ordering::Relaxed) {
            return;
        }
        for p in lock(&self.shared.pending).drain(..) {
            let _ = p.reply.send(gone());
        }
        if let Some(h) = self.holder.take() {
            self.files.release(h.pid);
            #[cfg(unix)]
            if h.token.is_none() {
                let _ = std::fs::remove_file(&h.address);
            }
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.close();
    }
}

/// The answer that sends an agent back to the files: no desktop holds
/// the workspace (any more).
fn gone() -> Value {
    json!({ "ok": false, "gone": true, "error": "the desktop no longer holds this workspace" })
}

/// Opens the endpoint: each request is queued for the UI thread, which
/// is asked for a frame, and answered once it ran (or dropped at the
/// deadline).
fn serve(shared: &Arc<Shared>) -> Result<Holder, String> {
    let queue = Arc::downgrade(shared);
    let handler = move |request: Value| -> Value {
        let Some(shared) = queue.upgrade() else {
            return gone();
        };
        if shared.closed.load(Ordering::Relaxed) {
            return gone();
        }
        let (reply, answer) = channel();
        let dropped = Arc::new(AtomicBool::new(false));
        lock(&shared.pending).push_back(Pending {
            request,
            reply,
            dropped: dropped.clone(),
        });
        shared.arrived.notify_all();
        shared.wake();
        drop(shared);
        answer.recv_timeout(ANSWER_DEADLINE).unwrap_or_else(|_| {
            dropped.store(true, Ordering::Relaxed);
            json!({ "ok": false, "error": format!(
                "the desktop holding this workspace did not answer within {} s (a window that draws no frame, minimized, answers once it draws again): the request was dropped",
                ANSWER_DEADLINE.as_secs()
            ) })
        })
    };
    #[cfg(unix)]
    {
        let path = socket_path();
        let bound =
            helios_inspect::transport::serve_at(&helios_inspect::Listen::Unix(path), None, handler)
                .map_err(|e| format!("the workspace's endpoint: {e}"))?;
        Ok(Holder {
            pid: std::process::id(),
            address: bound.address,
            token: None,
        })
    }
    #[cfg(not(unix))]
    {
        let token = helios_inspect::new_token();
        let listen = helios_inspect::Listen::Tcp(([127, 0, 0, 1], 0).into());
        let bound = helios_inspect::transport::serve_at(&listen, Some(&token), handler)
            .map_err(|e| format!("the workspace's endpoint: {e}"))?;
        Ok(Holder {
            pid: std::process::id(),
            address: bound.address,
            token: Some(token),
        })
    }
}

impl Shell {
    /// Where agents reach this desktop for its workspace, while it holds
    /// one open.
    pub fn workspace_holder(&self) -> Option<Holder> {
        self.edit.as_ref()?.live.as_ref()?.holder().cloned()
    }

    /// Waits until an agent's request waits for the next frame, at most
    /// `timeout`: what a headless desktop (a test, a survival scenario)
    /// runs its frames on.
    pub fn wait_workspace_request(&self, timeout: Duration) -> bool {
        self.wait_live(timeout, false)
    }

    /// Waits until the watcher saw the workspace's files change, at most
    /// `timeout`.
    pub fn wait_workspace_files(&self, timeout: Duration) -> bool {
        self.wait_live(timeout, true)
    }

    fn wait_live(&self, timeout: Duration, files: bool) -> bool {
        let Some(live) = self.edit.as_ref().and_then(|e| e.live.as_ref()) else {
            return false;
        };
        let deadline = Instant::now() + timeout;
        let mut pending = lock(&live.shared.pending);
        let waiting = |pending: &VecDeque<Pending>| {
            if files {
                !live.shared.changed.load(Ordering::Relaxed)
            } else {
                pending.is_empty()
            }
        };
        while waiting(&pending) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            pending = live
                .shared
                .arrived
                .wait_timeout(pending, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Lets the workspace go: agents write its files again.
    pub(super) fn release_workspace(&mut self) {
        if let Some(live) = self.edit.as_mut().and_then(|e| e.live.as_mut()) {
            live.close();
        }
    }

    /// Once per frame: the files looked at again when the watcher saw
    /// them change, then the agents' requests answered.
    pub(super) fn follow_workspace(&mut self, ctx: &egui::Context) {
        let Some(live) = self.edit.as_ref().and_then(|e| e.live.as_ref()) else {
            return;
        };
        let shared = live.shared.clone();
        {
            let mut waker = lock(&shared.waker);
            if waker.is_none() {
                *waker = Some(ctx.clone());
            }
        }
        if shared.changed.swap(false, Ordering::Relaxed) {
            self.check_workspace_files();
        }
        let pending: Vec<Pending> = lock(&shared.pending).drain(..).collect();
        for p in pending {
            if p.dropped.load(Ordering::Relaxed) {
                continue;
            }
            let answer = self.workspace_request(&p.request);
            let _ = p.reply.send(answer);
        }
    }

    /// What the workspace's files hold now, when it is not what the
    /// desktop last wrote or read there.
    pub(super) fn changed_outside(&self) -> Option<State> {
        let edit = self.edit.as_ref()?;
        let theirs = edit.files.load_on_disk().ok()?;
        (theirs.to_json() != edit.disk).then_some(theirs)
    }

    /// Takes in a change made to the workspace's files outside the
    /// desktop, as one undo step (see the module's documentation).
    /// Returns the parts taken from the files, and those the desktop had
    /// changed too; `None` when the files hold what the desktop wrote.
    pub fn check_workspace_files(&mut self) -> Option<(Vec<String>, Vec<String>)> {
        let theirs = self.changed_outside()?;
        Some(self.reload_workspace(theirs))
    }

    /// Merges `theirs` (the files) into what the desktop shows.
    pub(super) fn reload_workspace(&mut self, theirs: State) -> (Vec<String>, Vec<String>) {
        let names = |v: &[&str]| v.iter().map(ToString::to_string).collect::<Vec<_>>();
        let Some(edit) = self.edit.as_ref() else {
            return Default::default();
        };
        let base = edit.disk_state();
        let Some(ours) = self.workspace_state() else {
            return Default::default();
        };
        // The layout is egui-pop's to save, as it changes: the desktop
        // changed it when it differs from what egui-pop last saved, and
        // the files compare with that too.
        let saved = self.own_panels(self.layout_saved.clone().unwrap_or_else(|| self.layout()));
        let base = base.map(|mut b| {
            b.bundle.layout = saved;
            b
        });
        let mut compared = theirs.clone();
        compared.bundle.layout = self.own_panels(theirs.bundle.layout.clone());
        let merged = match base.and_then(|b| workspace::merge(&b, &ours, &compared)) {
            Ok(m) => m,
            Err(e) => {
                self.warn(
                    Message::new(
                        "gui-workspace-reload-failed",
                        "The workspace's files changed outside Helios and could not be read: { $error }",
                    )
                    .with("error", e),
                );
                return Default::default();
            }
        };
        if let Some(edit) = &mut self.edit {
            // The files as they are now: what the step below writes over.
            edit.disk = theirs.to_json();
        }
        if merged.taken.is_empty() {
            // Nothing new in the files; what the desktop shows goes there.
            if ours.to_json() != theirs.to_json() {
                self.write_workspace(&ours);
            }
            return Default::default();
        }
        let origin = Origin::Remote(FILES.into());
        let next = merged.state.clone();
        if let Err(e) =
            self.workspace_step_as_is("reload the workspace's files", &origin, |_| Ok(next))
        {
            self.warn(e);
        }
        let parts = merged.taken.join(", ");
        if merged.both.is_empty() {
            self.note(
                Message::new(
                    "gui-workspace-reloaded",
                    "The workspace's files changed outside Helios: { $parts } reloaded",
                )
                .with("parts", parts),
            );
        } else {
            self.warn(
                Message::new(
                    "gui-workspace-reloaded-both",
                    "The workspace's files changed outside Helios while you changed it too: { $both } taken from the files (Edit > Undo gives yours back)",
                )
                .with("both", merged.both.join(", ")),
            );
        }
        (names(&merged.taken), names(&merged.both))
    }

    /// Answers an agent's workspace request (the node's MCP tools, the
    /// same `cmd` and arguments as on the files), as the wire carries it:
    /// `{ok, answer}` or `{ok: false, error}`; `gone` when this desktop
    /// does not hold the workspace named.
    pub fn workspace_request(&mut self, request: &Value) -> Value {
        let held = self.edit.as_ref().map(|e| e.files.own.clone());
        let named = request["workspace"]
            .as_str()
            .map(|w| Files::new(&self.edit_config(), std::path::Path::new(w)).own);
        if held.is_none() || held != named {
            return gone();
        }
        match self.agent_workspace(request) {
            Ok(mut answer) => {
                if let Some(o) = answer.as_object_mut() {
                    o.insert("via".into(), json!("desktop"));
                }
                json!({ "ok": true, "answer": answer })
            }
            Err(error) => json!({ "ok": false, "error": error }),
        }
    }

    fn edit_config(&self) -> PathBuf {
        self.edit
            .as_ref()
            .map(|e| e.config.clone())
            .unwrap_or_default()
    }

    fn agent_workspace(&mut self, request: &Value) -> Result<Value, String> {
        let origin = Origin::Agent(AGENT.into());
        let dry = request["dry_run"].as_bool().unwrap_or(false);
        match request["cmd"].as_str().unwrap_or_default() {
            "workspace.parts" => {
                self.workspace_parts(request["source"].as_str().filter(|s| !s.is_empty()))
            }
            "workspace.state" => Ok(self
                .workspace_state()
                .ok_or("no workspace is open")?
                .to_json()),
            "workspace.edit" => {
                let now = self.workspace_state().ok_or("no workspace is open")?;
                let next = now.edited(&request["changes"])?;
                let problems = next.problems(&self.workspace_catalogue());
                let answer = json!({ "problems": problems, "dry_run": dry });
                if dry {
                    return Ok(answer);
                }
                if !problems.is_empty() {
                    return Err(problems.join("; "));
                }
                self.edit_workspace(&request["changes"], &origin)?;
                Ok(answer)
            }
            "workspace.reset" => self.reset_workspace(dry, &origin),
            "workspace.import_parts" => {
                let mut answer = self.import_workspace_parts(request, &origin)?;
                if answer.get("problems").is_none() {
                    answer["problems"] = json!([]);
                }
                Ok(answer)
            }
            "workspace.save_as_template" => self.save_workspace_as_template(request),
            "network.delegate" => self.delegate_nodes(request, &origin),
            "subworkspace.delete" => self.delete_subworkspace(request, &origin),
            cmd @ ("network.connect" | "network.disconnect" | "network.move" | "network.add"
            | "network.remove") => self.network_command(cmd, request, &origin),
            "workspace.undo" => self.agent_undo(&origin),
            cmd if cmd.starts_with("context.") => {
                crate::contexts::agent_tool(self.app.handle(), request, AGENT)
            }
            other => Err(format!("no workspace tool `{other}`")),
        }
    }

    /// An agent's undo on the desktop: the last step, when it is a
    /// workspace step an agent made; never the user's.
    fn agent_undo(&mut self, origin: &Origin) -> Result<Value, String> {
        let mut store = lock(&self.values);
        let last = store
            .undo_steps()
            .last()
            .ok_or("nothing to undo in this workspace")?;
        let agents = matches!(last.origin, Origin::Agent(_));
        let workspace = last
            .changes
            .iter()
            .all(|(p, _, _)| p == super::WORKSPACE_STATE);
        if !(agents && workspace) {
            return Err(format!(
                "the desktop's last step (`{}`) is not an agent's step on the workspace: undo it on the desktop",
                last.label
            ));
        }
        let label = store
            .undo(origin)
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        drop(store);
        self.sync_workspace();
        Ok(json!({ "undone": label }))
    }
}
