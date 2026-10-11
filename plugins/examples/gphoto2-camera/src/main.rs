//! A Helios plugin serving `device` with the gphoto2 program: list the
//! cameras, describe one, take a picture, download files. It speaks
//! JSON-RPC 2.0 over its standard input and output, one message per line
//! (see `docs/extensions.md`, "Proof of concept").
//!
//! It writes only into the folder each download hands it; the broker
//! moves the files on from there.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::Path;
use std::process::Command;

use serde_json::{Value as Json, json};

const ID: &str = "io.github.quazardous.helios.gphoto2-camera";

struct Plugin {
    gphoto2: String,
    granted: Vec<String>,
}

/// A JSON-RPC error: code and message.
type Failure = (i64, String);

fn send(msg: &Json) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{msg}");
    let _ = out.flush();
}

impl Plugin {
    /// Runs gphoto2 on a camera (`port`, or the first one found).
    fn run(&self, port: Option<&str>, args: &[&str]) -> Result<String, Failure> {
        let mut cmd = Command::new(&self.gphoto2);
        if let Some(port) = port {
            cmd.args(["--port", port]);
        }
        let out = cmd
            .args(args)
            .output()
            .map_err(|e| (-32001, format!("{}: {e}", self.gphoto2)))?;
        if !out.status.success() {
            return Err((
                -32002,
                format!(
                    "gphoto2 {}: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn list(&self) -> Result<Json, Failure> {
        let out = self.run(None, &["--auto-detect"])?;
        // A header, a line of dashes, then `<model>   <port>` per camera.
        let devices: Vec<Json> = out
            .lines()
            .skip_while(|l| !l.starts_with("---"))
            .skip(1)
            .filter_map(|l| {
                let l = l.trim_end();
                let (model, port) = l.rsplit_once(char::is_whitespace)?;
                Some(json!({"id": port, "port": port, "model": model.trim()}))
            })
            .collect();
        Ok(json!({ "devices": devices }))
    }

    fn info(&self, port: Option<&str>) -> Result<Json, Failure> {
        let out = self.run(port, &["--summary"])?;
        let summary: BTreeMap<&str, &str> = out
            .lines()
            .filter_map(|l| l.split_once(": "))
            .map(|(k, v)| (k.trim(), v.trim()))
            .collect();
        Ok(json!({"device": port, "summary": summary}))
    }

    fn capture(&self, port: Option<&str>) -> Result<Json, Failure> {
        let out = self.run(port, &["--capture-image"])?;
        let files: Vec<&str> = out
            .lines()
            .filter_map(|l| {
                l.strip_prefix("New file is in location ")?
                    .strip_suffix(" on the camera")
            })
            .collect();
        if files.is_empty() {
            return Err((-32004, format!("gphoto2 reported no new file: {out}")));
        }
        Ok(json!({ "files": files }))
    }

    /// The camera's files, by path, with the number gphoto2 gives each.
    fn numbers(&self, port: Option<&str>) -> Result<BTreeMap<String, String>, Failure> {
        let out = self.run(port, &["--list-files"])?;
        let mut folder = "";
        let mut numbers = BTreeMap::new();
        for line in out.lines() {
            if let Some((_, rest)) = line.split_once(" in folder '") {
                folder = rest.trim_end_matches(['\'', ':', '.']);
            } else if let Some(rest) = line.strip_prefix('#') {
                let mut words = rest.split_whitespace();
                if let (Some(n), Some(name)) = (words.next(), words.next()) {
                    numbers.insert(format!("{}/{name}", folder.trim_end_matches('/')), n.into());
                }
            }
        }
        Ok(numbers)
    }

    fn download(&self, params: &Json) -> Result<Json, Failure> {
        if !self.granted.iter().any(|c| c == "folder.write") {
            return Err((-32003, "folder.write is not granted".into()));
        }
        let port = params.get("device").and_then(Json::as_str);
        let folder = Path::new(
            params
                .get("folder")
                .and_then(Json::as_str)
                .ok_or((-32602, "`folder` is missing".to_string()))?,
        );
        let token = params.get("progress").cloned().unwrap_or(Json::Null);
        let wanted: Vec<&str> = params
            .get("files")
            .and_then(Json::as_array)
            .map(|f| f.iter().filter_map(Json::as_str).collect())
            .unwrap_or_default();
        let numbers = self.numbers(port)?;
        let mut files = Vec::new();
        for (i, camera) in wanted.iter().enumerate() {
            let n = numbers
                .get(*camera)
                .ok_or((-32005, format!("no file {camera} on the camera")))?;
            let name = camera.rsplit('/').next().unwrap_or(camera);
            let dest = folder.join(name);
            let dest_text = dest.to_string_lossy();
            self.run(port, &["--get-file", n, "--filename", &dest_text])?;
            files.push(json!({"camera": camera, "path": dest}));
            send(&json!({
                "jsonrpc": "2.0",
                "method": "$/progress",
                "params": {"token": token, "done": i + 1, "total": wanted.len(), "message": name},
            }));
        }
        Ok(json!({ "files": files }))
    }

    fn handle(&mut self, method: &str, params: &Json) -> Result<Json, Failure> {
        let port = params.get("device").and_then(Json::as_str);
        match method {
            "initialize" => {
                self.granted = params["capabilities"]["granted"]
                    .as_array()
                    .map(|g| {
                        g.iter()
                            .filter_map(|c| c.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(p) = params["settings"][format!("{ID}.gphoto2")].as_str() {
                    self.gphoto2 = p.into();
                }
                Ok(json!({"name": "gphoto2-camera", "version": env!("CARGO_PKG_VERSION")}))
            }
            "shutdown" => Ok(Json::Null),
            "device.list" => self.list(),
            "device.info" => self.info(port),
            "device.capture" => self.capture(port),
            "device.download" => self.download(params),
            _ => Err((-32601, format!("no method `{method}`"))),
        }
    }
}

fn main() {
    let mut plugin = Plugin {
        gphoto2: "gphoto2".into(),
        granted: Vec::new(),
    };
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let Ok(msg) = serde_json::from_str::<Json>(&line) else {
            continue;
        };
        let method = msg.get("method").and_then(Json::as_str).unwrap_or("");
        let Some(id) = msg.get("id") else {
            if method == "exit" {
                return;
            }
            continue; // initialized, $/cancelRequest
        };
        let params = msg.get("params").cloned().unwrap_or(Json::Null);
        let reply = match plugin.handle(method, &params) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => json!({
                "jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message},
            }),
        };
        send(&reply);
    }
}
