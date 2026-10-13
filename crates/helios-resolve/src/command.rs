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
//!  "brief": "warm, soft skin", "mode": "full_ai",
//!  "question": {}, "region": {"x": 0, "y": 0, "width": 6000, "height": 4000},
//!  "out": "/tmp/…"}
//! ```
//!
//! Inputs are sRGB PNG files (the model's view, not the working data) in a
//! directory removed once the program has answered; `out` is that
//! directory, where the program may write files it answers with. The
//! question is the node's (plain JSON values: an AI object mask's
//! `{"kind": "prompt", "prompt": "the red car", "resolver": ""}`), the
//! region the part of the image the inputs show, in full-resolution
//! pixels. The answer:
//!
//! ```json
//! {"options": [{"gain": 2.0}], "confidence": 0.8,
//!  "rationale": "…", "model": {"name": "…", "version": "…"}}
//! ```
//!
//! Option values are plain JSON, typed by the answer's declarations. An
//! empty `options` list is a refusal: `rationale` says why.
//!
//! **A mask** (a decision whose answer has a `mask` parameter: the AI
//! object member) is answered with a PNG, its luminance the mask (0 to
//! 255), at the inputs' resolution or any other up to 1024 a side, laid
//! over the request's region:
//!
//! ```json
//! {"options": [{"mask": {"png": "mask.png"}}], "confidence": 0.9,
//!  "rationale": "…", "model": {"name": "…", "version": "…"}}
//! ```
//!
//! `png` is a file (relative to `out`, or absolute); `{"png_base64": "…"}`
//! or a plain base64 string give it inline. The node's `area` and
//! `confidence` are filled from the request's region and the answer's
//! confidence.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use helios_core::Params;
use helios_decision::Model;
use helios_export::{Format, encode};
use serde_json::{Value, json};

use crate::{MASK, Request, Resolution, Resolver, mask_answer, params_from_json, plain_json};

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
pub fn request_json(request: &Request, dir: &Path) -> Result<Value, String> {
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
    let mut body = body(request, inputs);
    body["out"] = json!(dir);
    Ok(body)
}

/// The request as JSON with its inputs inline (`png_base64`), for an
/// endpoint over HTTP.
pub fn request_inline(request: &Request) -> Result<Value, String> {
    let mut inputs = Vec::new();
    for input in &request.inputs {
        let png = encode(&input.image, &Format::Png { bits: 8 })?;
        let r = input.image.region();
        inputs.push(json!({
            "key": input.key,
            "width": r.width,
            "height": r.height,
            "png_base64": STANDARD.encode(png),
        }));
    }
    Ok(body(request, inputs))
}

fn body(request: &Request, inputs: Vec<Value>) -> Value {
    json!({
        "node": request.node,
        "module": request.module,
        "description": request.description,
        "answer": request.answer,
        "inputs": inputs,
        "brief": request.brief,
        "mode": request.mode,
        "question": plain_json(&request.question),
        "region": request.region,
    })
}

/// A resolution from the JSON an external model wrote.
pub fn resolution_json(request: &Request, v: &Value) -> Result<Resolution, String> {
    resolution_json_in(request, v, None)
}

/// The PNG bytes a mask answer gives: a file (relative to `dir`), or
/// base64 inline.
fn mask_png(m: &Value, dir: Option<&Path>) -> Result<Vec<u8>, String> {
    let inline = |b64: &str| {
        STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("the mask is not base64: {e}"))
    };
    if let Some(b64) = m
        .as_str()
        .or_else(|| m.get("png_base64").and_then(Value::as_str))
    {
        return inline(b64);
    }
    let file = m
        .get("png")
        .and_then(Value::as_str)
        .ok_or("a mask is {\"png\": file}, {\"png_base64\": …} or a base64 string")?;
    let path = match dir {
        Some(d) => d.join(file),
        None => PathBuf::from(file),
    };
    std::fs::read(&path).map_err(|e| format!("the mask {}: {e}", path.display()))
}

/// One option of an answer: plain JSON typed by the declarations, a mask
/// read from its PNG.
fn option(
    request: &Request,
    o: &Value,
    confidence: f64,
    dir: Option<&Path>,
) -> Result<Params, String> {
    let mut o = o.clone();
    let mask = if request.wants_mask() {
        o.as_object_mut().and_then(|m| m.remove(MASK))
    } else {
        None
    };
    let mut params = params_from_json(&request.answer, &o)?;
    if let Some(m) = mask {
        let answer = mask_answer(request, &mask_png(&m, dir)?, confidence)?;
        for (name, value) in answer.iter() {
            if name == MASK || params.get(name).is_none() {
                params.set(name, value.clone());
            }
        }
    }
    Ok(params)
}

/// [`resolution_json`] for a program whose files live in `dir`.
pub fn resolution_json_in(
    request: &Request,
    v: &Value,
    dir: Option<&Path>,
) -> Result<Resolution, String> {
    let confidence = v
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let options = v
        .get("options")
        .and_then(Value::as_array)
        .ok_or("the answer has no `options` list")?
        .iter()
        .map(|o| option(request, o, confidence, dir))
        .collect::<Result<Vec<_>, _>>()?;
    let model = v.get("model").ok_or("the answer has no `model`")?;
    let text = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Ok(Resolution {
        options,
        confidence,
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
            resolution_json_in(request, &answer, Some(&dir))
        })();
        let _ = std::fs::remove_dir_all(&dir);
        result
    }
}
