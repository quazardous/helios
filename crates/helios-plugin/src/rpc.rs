//! One plugin process, spoken to in JSON-RPC 2.0 over its standard input
//! and output, one message per line (the shape of LSP and MCP, without
//! their headers). A reader thread matches answers to calls and hands
//! notifications on; when the process ends, every waiting call fails at
//! once instead of waiting for its timeout.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};

/// What a plugin sends on its own, or how it ended.
#[derive(Clone, Debug)]
pub enum Event {
    Notification {
        method: String,
        params: Json,
    },
    /// The process ended without being asked to: `status` says how,
    /// `stderr` holds its last lines.
    Exited {
        status: String,
        stderr: Vec<String>,
    },
}

/// Why a call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum CallError {
    /// No answer within the time given; the plugin is told to cancel.
    Timeout(Duration),
    /// The process is gone (crashed, or stopped).
    Stopped(String),
    /// The plugin answered with an error.
    Rpc { code: i64, message: String },
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            CallError::Timeout(d) => write!(f, "no answer within {} ms", d.as_millis()),
            CallError::Stopped(why) => write!(f, "the plugin stopped: {why}"),
            CallError::Rpc { code, message } => write!(f, "{message} (error {code})"),
        }
    }
}

type Reply = Result<Json, CallError>;
pub type Listener = Arc<dyn Fn(Event) + Send + Sync>;

const STDERR_LINES: usize = 20;

pub struct Process {
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Mutex<HashMap<u64, Sender<Reply>>>,
    next: AtomicU64,
    alive: AtomicBool,
    stopping: AtomicBool,
    stderr: Mutex<VecDeque<String>>,
    pub started: Instant,
}

impl Process {
    /// Starts `program` in `dir`, with `env` added to the environment.
    pub fn spawn(
        program: &Path,
        args: &[String],
        dir: &Path,
        env: &[(String, String)],
        listener: Listener,
    ) -> Result<Arc<Process>, String> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(dir)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {e}", program.display()))?;
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let process = Arc::new(Process {
            stdin: Mutex::new(child.stdin.take()),
            child: Mutex::new(child),
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            alive: AtomicBool::new(true),
            stopping: AtomicBool::new(false),
            stderr: Mutex::new(VecDeque::new()),
            started: Instant::now(),
        });
        let p = process.clone();
        let (stderr_done, stderr_closed) = channel::<()>();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                p.note(line);
            }
            let _ = stderr_done.send(());
        });
        let p = process.clone();
        std::thread::spawn(move || {
            p.read(BufReader::new(stdout), &listener);
            // The last words on stderr say why it ended: wait for them a
            // moment (a child the plugin left behind may hold it open).
            let _ = stderr_closed.recv_timeout(Duration::from_secs(1));
            p.ended(&listener);
        });
        Ok(process)
    }

    fn read(&self, out: impl BufRead, listener: &Listener) {
        for line in out.lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(msg) = serde_json::from_str::<Json>(&line) else {
                self.note(format!("not a JSON-RPC message: {line}"));
                continue;
            };
            let id = msg.get("id").and_then(Json::as_u64);
            match (msg.get("method").and_then(Json::as_str), id) {
                // A request from the plugin: Helios offers it nothing
                // yet, every access goes through the services it serves.
                (Some(method), Some(_)) => {
                    let _ = self.write(&json!({
                        "jsonrpc": "2.0",
                        "id": msg["id"],
                        "error": {"code": -32601, "message": format!("Helios offers no `{method}` to plugins")},
                    }));
                }
                (Some(method), None) => listener(Event::Notification {
                    method: method.into(),
                    params: msg.get("params").cloned().unwrap_or(Json::Null),
                }),
                (None, Some(id)) => {
                    let reply = match msg.get("error") {
                        Some(e) => Err(CallError::Rpc {
                            code: e.get("code").and_then(Json::as_i64).unwrap_or(0),
                            message: e
                                .get("message")
                                .and_then(Json::as_str)
                                .unwrap_or("unknown error")
                                .into(),
                        }),
                        None => Ok(msg.get("result").cloned().unwrap_or(Json::Null)),
                    };
                    if let Some(tx) = self.pending.lock().expect("pending lock").remove(&id) {
                        let _ = tx.send(reply);
                    }
                }
                (None, None) => self.note(format!("not a JSON-RPC message: {line}")),
            }
        }
    }

    /// The output closed: the process is ending. Every waiting call fails.
    fn ended(&self, listener: &Listener) {
        self.alive.store(false, Ordering::SeqCst);
        let status = self.reap();
        let stderr = self.stderr_tail();
        let why = match stderr.last() {
            Some(last) => format!("{status}: {last}"),
            None => status.clone(),
        };
        for (_, tx) in self.pending.lock().expect("pending lock").drain() {
            let _ = tx.send(Err(CallError::Stopped(why.clone())));
        }
        if !self.stopping.load(Ordering::SeqCst) {
            listener(Event::Exited { status, stderr });
        }
    }

    /// How the process ended. One that closed its output but goes on is
    /// killed after a moment: the lock is never held while waiting.
    fn reap(&self) -> String {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut child = self.child.lock().expect("child lock");
            match child.try_wait() {
                Ok(Some(s)) => return s.to_string(),
                Err(e) => return e.to_string(),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    return match child.wait() {
                        Ok(s) => format!("{s} (closed its output, killed)"),
                        Err(e) => e.to_string(),
                    };
                }
                Ok(None) => {}
            }
            drop(child);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn note(&self, line: String) {
        let mut tail = self.stderr.lock().expect("stderr lock");
        if tail.len() == STDERR_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr
            .lock()
            .expect("stderr lock")
            .iter()
            .cloned()
            .collect()
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn pid(&self) -> u32 {
        self.child.lock().expect("child lock").id()
    }

    fn write(&self, msg: &Json) -> Result<(), String> {
        let mut stdin = self.stdin.lock().expect("stdin lock");
        let stdin = stdin.as_mut().ok_or("the plugin's input is closed")?;
        let mut line = msg.to_string();
        line.push('\n');
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.flush())
            .map_err(|e| e.to_string())
    }

    pub fn notify(&self, method: &str, params: Json) -> Result<(), String> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// Calls `method` and waits for its answer, at most `timeout`.
    pub fn call(&self, method: &str, params: Json, timeout: Duration) -> Reply {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        {
            let mut pending = self.pending.lock().expect("pending lock");
            // Checked under the lock the reader drains with: a call made
            // as the process ends fails now, not at its timeout.
            if !self.alive() {
                return Err(CallError::Stopped("it is not running".into()));
            }
            pending.insert(id, tx);
        }
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if let Err(e) = self.write(&msg) {
            self.pending.lock().expect("pending lock").remove(&id);
            return Err(CallError::Stopped(e));
        }
        match rx.recv_timeout(timeout) {
            Ok(reply) => reply,
            Err(RecvTimeoutError::Timeout) => {
                self.pending.lock().expect("pending lock").remove(&id);
                let _ = self.notify("$/cancelRequest", json!({"id": id}));
                Err(CallError::Timeout(timeout))
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err(CallError::Stopped("its answer was lost".into()))
            }
        }
    }

    /// Asks the plugin to stop (`shutdown`, then `exit`), and kills it if
    /// it has not within `grace`.
    pub fn stop(&self, grace: Duration) {
        self.stopping.store(true, Ordering::SeqCst);
        if self.alive() {
            let _ = self.call("shutdown", Json::Null, grace);
            let _ = self.notify("exit", Json::Null);
        }
        self.stdin.lock().expect("stdin lock").take();
        let deadline = Instant::now() + grace;
        loop {
            let mut child = self.child.lock().expect("child lock");
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                Ok(None) => {}
            }
            drop(child);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
