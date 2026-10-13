//! Stand-ins for tests and sandboxes: a model runtime answering like
//! Ollama and an OpenAI-compatible server would ([`FakeRuntime`]), and a
//! machine with scripted outputs ([`FakeSystem`]). No network, no
//! program run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use crate::local::System;
use crate::transport::{HttpCall, Transport};

/// A model runtime in memory. It serves Ollama's paths (`/api/...`) and
/// OpenAI's (`/v1/...`) on any host; a chat answers the reply set for its
/// model ([`FakeRuntime::reply`]), `{}` otherwise.
#[derive(Debug, Default)]
pub struct FakeRuntime {
    /// Installed models, with what `/api/show` says they do.
    models: Mutex<BTreeMap<String, Vec<String>>>,
    replies: Mutex<BTreeMap<String, String>>,
    calls: Mutex<Vec<HttpCall>>,
    down: AtomicBool,
    /// How many progress lines a pull streams before `success`.
    pull_steps: Mutex<u64>,
    /// A pull of this model fails with this error.
    pull_errors: Mutex<BTreeMap<String, String>>,
}

impl FakeRuntime {
    pub fn new() -> Self {
        let fake = FakeRuntime::default();
        *fake.pull_steps.lock().expect("fake lock") = 4;
        fake
    }

    /// Installs `name`, doing `capabilities` (Ollama's words: `completion`,
    /// `vision`, `embedding`).
    pub fn model(self, name: &str, capabilities: &[&str]) -> Self {
        self.models.lock().expect("fake lock").insert(
            name.into(),
            capabilities.iter().map(|c| (*c).to_string()).collect(),
        );
        self
    }

    /// What a chat with `model` answers.
    pub fn reply(self, model: &str, text: &str) -> Self {
        self.replies
            .lock()
            .expect("fake lock")
            .insert(model.into(), text.into());
        self
    }

    pub fn pull_steps(self, steps: u64) -> Self {
        *self.pull_steps.lock().expect("fake lock") = steps;
        self
    }

    pub fn pull_error(self, model: &str, error: &str) -> Self {
        self.pull_errors
            .lock()
            .expect("fake lock")
            .insert(model.into(), error.into());
        self
    }

    /// Stops answering, as a runtime that is not running.
    pub fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
    }

    /// Every call received, in order.
    pub fn calls(&self) -> Vec<HttpCall> {
        self.calls.lock().expect("fake lock").clone()
    }

    pub fn installed(&self) -> Vec<String> {
        self.models
            .lock()
            .expect("fake lock")
            .keys()
            .cloned()
            .collect()
    }

    fn answer(&self, call: &HttpCall) -> (u16, Value) {
        let body = call.body.clone().unwrap_or(Value::Null);
        let model = body["model"].as_str().unwrap_or_default().to_string();
        let models = self.models.lock().expect("fake lock").clone();
        let reply = || {
            self.replies
                .lock()
                .expect("fake lock")
                .get(&model)
                .cloned()
                .unwrap_or_else(|| "{}".into())
        };
        let missing = || {
            (
                404,
                json!({ "error": format!("model \"{model}\" not found, try pulling it first") }),
            )
        };
        match call.path() {
            "/api/version" => (200, json!({ "version": "0.0.0-fake" })),
            "/api/tags" => (
                200,
                json!({ "models": models.keys().map(|m| json!({ "name": m, "model": m, "size": 1_000_000 })).collect::<Vec<_>>() }),
            ),
            "/api/show" => match models.get(&model) {
                Some(caps) => (200, json!({ "capabilities": caps })),
                None => missing(),
            },
            "/api/chat" if models.contains_key(&model) => (
                200,
                json!({ "model": model, "message": { "role": "assistant", "content": reply() }, "done": true }),
            ),
            "/api/embed" if models.contains_key(&model) => {
                let n = body["input"].as_array().map_or(1, Vec::len);
                (
                    200,
                    json!({ "model": model, "embeddings": vec![vec![0.5, 0.25]; n] }),
                )
            }
            "/v1/models" => (
                200,
                json!({ "data": models.keys().map(|m| json!({ "id": m })).collect::<Vec<_>>() }),
            ),
            "/v1/chat/completions" if models.contains_key(&model) => (
                200,
                json!({ "model": model, "choices": [{ "message": { "content": reply() } }] }),
            ),
            "/api/chat" | "/api/embed" | "/v1/chat/completions" => missing(),
            other => (404, json!({ "error": format!("no route {other}") })),
        }
    }
}

impl Transport for FakeRuntime {
    fn send(&self, call: &HttpCall) -> Result<(u16, String), String> {
        self.calls.lock().expect("fake lock").push(call.clone());
        if self.down.load(Ordering::SeqCst) {
            return Err(format!("{}: connection refused", call.url));
        }
        let (status, body) = self.answer(call);
        Ok((status, body.to_string()))
    }

    fn stream(&self, call: &HttpCall, line: &mut dyn FnMut(&str) -> bool) -> Result<u16, String> {
        self.calls.lock().expect("fake lock").push(call.clone());
        if self.down.load(Ordering::SeqCst) {
            return Err(format!("{}: connection refused", call.url));
        }
        if call.path() != "/api/pull" {
            let (status, body) = self.answer(call);
            line(&body.to_string());
            return Ok(status);
        }
        let model = call
            .body
            .as_ref()
            .and_then(|b| b["model"].as_str())
            .unwrap_or_default()
            .to_string();
        if let Some(error) = self.pull_errors.lock().expect("fake lock").get(&model) {
            line(&json!({ "error": error }).to_string());
            return Ok(500);
        }
        let steps = *self.pull_steps.lock().expect("fake lock");
        let total = steps * 1000;
        if !line(&json!({ "status": "pulling manifest" }).to_string()) {
            return Ok(200);
        }
        for i in 1..=steps {
            let progress = json!({ "status": "pulling 1234abcd", "digest": "sha256:1234abcd", "total": total, "completed": i * 1000 });
            if !line(&progress.to_string()) {
                return Ok(200);
            }
        }
        self.models
            .lock()
            .expect("fake lock")
            .insert(model, vec!["completion".into()]);
        line(&json!({ "status": "success" }).to_string());
        Ok(200)
    }
}

/// A machine with scripted program outputs and files.
#[derive(Debug, Default)]
pub struct FakeSystem {
    pub os: String,
    pub arch: String,
    /// `program arg arg` → its output (`Err` for a failure).
    pub outputs: Mutex<BTreeMap<String, Result<String, String>>>,
    pub files: Mutex<BTreeMap<PathBuf, String>>,
    pub programs: Mutex<BTreeMap<String, PathBuf>>,
    pub vars: BTreeMap<String, String>,
    /// Every program run, as `program arg arg`, in order.
    pub ran: Mutex<Vec<String>>,
}

impl FakeSystem {
    /// A Linux machine with a home and nothing else.
    pub fn linux() -> Self {
        let mut vars = BTreeMap::new();
        vars.insert("HOME".into(), "/home/ada".into());
        FakeSystem {
            os: "linux".into(),
            arch: "x86_64".into(),
            vars,
            ..FakeSystem::default()
        }
    }

    /// `program args` answers `output`.
    pub fn output(self, command: &str, output: Result<&str, &str>) -> Self {
        self.outputs.lock().expect("fake lock").insert(
            command.into(),
            output.map(str::to_string).map_err(str::to_string),
        );
        self
    }

    pub fn file(self, path: &str, text: &str) -> Self {
        self.files
            .lock()
            .expect("fake lock")
            .insert(path.into(), text.into());
        self
    }

    /// `program` is on `PATH`.
    pub fn program(self, program: &str) -> Self {
        self.programs
            .lock()
            .expect("fake lock")
            .insert(program.into(), PathBuf::from("/usr/bin").join(program));
        self
    }

    pub fn ran(&self) -> Vec<String> {
        self.ran.lock().expect("fake lock").clone()
    }
}

impl System for FakeSystem {
    fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        let command = std::iter::once(program)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        self.ran.lock().expect("fake lock").push(command.clone());
        self.outputs
            .lock()
            .expect("fake lock")
            .get(&command)
            .cloned()
            .unwrap_or_else(|| Err(format!("`{command}`: not scripted")))
    }

    fn read(&self, path: &Path) -> Option<String> {
        self.files.lock().expect("fake lock").get(path).cloned()
    }

    fn exists(&self, path: &Path) -> bool {
        let files = self.files.lock().expect("fake lock");
        path == Path::new("/") || files.keys().any(|f| f.starts_with(path))
    }

    fn write(&self, path: &Path, text: &str) -> Result<(), String> {
        self.files
            .lock()
            .expect("fake lock")
            .insert(path.to_path_buf(), text.into());
        Ok(())
    }

    fn which(&self, program: &str) -> Option<PathBuf> {
        self.programs
            .lock()
            .expect("fake lock")
            .get(program)
            .cloned()
    }

    fn env(&self, name: &str) -> Option<String> {
        self.vars.get(name).cloned()
    }

    fn os(&self) -> String {
        self.os.clone()
    }

    fn arch(&self) -> String {
        self.arch.clone()
    }
}
