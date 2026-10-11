//! A resolver that is an external program: the request goes to its
//! standard input as JSON, its answer comes back on its standard output.
//! A local model run by a script, or any API the user's own script calls:
//! Helios holds no key for it.
//!
//! The request:
//!
//! ```json
//! {"node": "sky", "module": "how_bright", "description": "…",
//!  "answer": [{"name": "gain", "kind": {"float": {"min": 0, "max": 4, "default": 1}}, …}],
//!  "inputs": [{"key": "…", "width": 64, "height": 43, "png": "/tmp/…/0.png"}],
//!  "brief": "warm, soft skin", "mode": "full_ai"}
//! ```
//!
//! Inputs are sRGB PNG files (the model's view, not the working data) in a
//! directory removed once the program has answered. The answer:
//!
//! ```json
//! {"options": [{"gain": 2.0}], "confidence": 0.8,
//!  "rationale": "…", "model": {"name": "…", "version": "…"}}
//! ```
//!
//! Option values are plain JSON, typed by the answer's declarations.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command as Process, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use helios_decision::Model;
use helios_export::{Format, encode};
use serde_json::{Value, json};

use crate::{Request, Resolution, Resolver, params_from_json};

pub struct Command {
    program: PathBuf,
    args: Vec<String>,
}

impl Command {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Command {
            program: program.into(),
            args: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: &str) -> Self {
        self.args.push(arg.into());
        self
    }
}

/// A directory of its own for a call's images.
fn scratch() -> Result<PathBuf, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-resolve-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

/// The request as JSON, its inputs written as PNG files under `dir`.
pub fn request_json(request: &Request, dir: &std::path::Path) -> Result<Value, String> {
    let mut inputs = Vec::new();
    for (i, input) in request.inputs.iter().enumerate() {
        let path = dir.join(format!("{i}.png"));
        let png = encode(&input.image, &Format::Png { bits: 8 })?;
        std::fs::write(&path, png).map_err(|e| format!("{}: {e}", path.display()))?;
        let r = input.image.region();
        inputs.push(json!({
            "key": input.key,
            "width": r.width,
            "height": r.height,
            "png": path,
        }));
    }
    Ok(json!({
        "node": request.node,
        "module": request.module,
        "description": request.description,
        "answer": request.answer,
        "inputs": inputs,
        "brief": request.brief,
        "mode": request.mode,
    }))
}

/// A resolution from the JSON an external model wrote.
pub fn resolution_json(request: &Request, v: &Value) -> Result<Resolution, String> {
    let options = v
        .get("options")
        .and_then(Value::as_array)
        .ok_or("the answer has no `options` list")?
        .iter()
        .map(|o| params_from_json(&request.answer, o))
        .collect::<Result<Vec<_>, _>>()?;
    let model = v.get("model").ok_or("the answer has no `model`")?;
    let text = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Ok(Resolution {
        options,
        confidence: v
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .clamp(0.0, 1.0),
        rationale: text(v, "rationale"),
        model: Model {
            name: text(model, "name"),
            version: text(model, "version"),
        },
    })
}

impl Resolver for Command {
    fn resolve(&self, request: &Request) -> Result<Resolution, String> {
        let dir = scratch()?;
        let result = (|| {
            let body = request_json(request, &dir)?;
            let mut child = Process::new(&self.program)
                .args(&self.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("{}: {e}", self.program.display()))?;
            child
                .stdin
                .take()
                .expect("piped")
                .write_all(body.to_string().as_bytes())
                .map_err(|e| format!("{}: {e}", self.program.display()))?;
            let out = child
                .wait_with_output()
                .map_err(|e| format!("{}: {e}", self.program.display()))?;
            if !out.status.success() {
                return Err(format!(
                    "{} failed ({}): {}",
                    self.program.display(),
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            let answer: Value = serde_json::from_slice(&out.stdout)
                .map_err(|e| format!("{}: not a JSON answer: {e}", self.program.display()))?;
            resolution_json(request, &answer)
        })();
        let _ = std::fs::remove_dir_all(&dir);
        result
    }
}
