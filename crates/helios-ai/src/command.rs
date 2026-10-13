//! A local program as a provider: segmentation, which no runtime serves
//! over a standard API yet (a SAM-family model behind a small script).
//!
//! The request goes to the program's standard input:
//!
//! ```json
//! {"model": "sam2-small", "prompt": "the sky",
//!  "image": {"mime": "image/png", "base64": "…"}}
//! ```
//!
//! and its answer comes back on standard output:
//!
//! ```json
//! {"png_base64": "…", "confidence": 0.9, "model": "sam2-small"}
//! ```
//!
//! or `{"error": "why"}`. The mask is a grayscale PNG, its luminance the
//! coverage.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::Capability;
use crate::provider::{
    ChatReply, ChatRequest, ModelInfo, ModelProvider, ProviderConfig, ProviderKind, SegmentReply,
    SegmentRequest, unsupported,
};

pub struct CommandProvider {
    id: String,
    argv: Vec<String>,
    capabilities: Vec<Capability>,
}

impl CommandProvider {
    pub fn new(config: &ProviderConfig) -> Self {
        CommandProvider {
            id: config.id.clone(),
            argv: config.command.clone(),
            capabilities: config.capabilities(),
        }
    }

    fn run(&self, input: &Value) -> Result<Value, String> {
        let (program, args) = self.argv.split_first().ok_or("no program")?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run `{program}`: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input.to_string().as_bytes())
                .map_err(|e| format!("`{program}`: {e}"))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| format!("`{program}`: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "`{program}` failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let answer: Value = serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("`{program}` did not answer JSON: {e}"))?;
        if let Some(error) = answer.get("error").and_then(Value::as_str) {
            return Err(format!("`{program}`: {error}"));
        }
        Ok(answer)
    }
}

/// Whether `program` can be run: a path that exists, or a name on `PATH`.
pub fn findable(program: &str) -> bool {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.exists();
    }
    std::env::var_os("PATH").is_some_and(|dirs| {
        std::env::split_paths(&dirs).any(|d| {
            d.join(program).exists()
                || d.join(format!("{program}{}", std::env::consts::EXE_SUFFIX))
                    .exists()
        })
    })
}

impl ModelProvider for CommandProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Command
    }

    fn capabilities(&self) -> Vec<Capability> {
        self.capabilities.clone()
    }

    fn remote(&self) -> bool {
        false
    }

    fn health(&self) -> Result<String, String> {
        let program = self.argv.first().ok_or("no program")?;
        if findable(program) {
            Ok(format!("`{program}` found"))
        } else {
            Err(format!("`{program}` is not found"))
        }
    }

    fn models(&self) -> Result<Vec<ModelInfo>, String> {
        Ok(vec![ModelInfo {
            name: "default".into(),
            size_bytes: None,
            capabilities: self.capabilities.clone(),
        }])
    }

    fn chat(&self, _: &ChatRequest) -> Result<ChatReply, String> {
        Err(unsupported(&self.id, Capability::Chat))
    }

    fn segment(&self, request: &SegmentRequest) -> Result<SegmentReply, String> {
        let answer = self.run(&json!({
            "model": request.model,
            "prompt": request.prompt,
            "image": { "mime": request.image.mime, "base64": STANDARD.encode(&request.image.bytes) },
        }))?;
        let png = answer["png_base64"]
            .as_str()
            .ok_or("the answer holds no `png_base64`")?;
        Ok(SegmentReply {
            png: STANDARD
                .decode(png)
                .map_err(|e| format!("the mask's base64: {e}"))?,
            confidence: answer["confidence"].as_f64().unwrap_or(0.5),
            model: answer["model"]
                .as_str()
                .unwrap_or(&request.model)
                .to_string(),
        })
    }
}
