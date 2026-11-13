//! The Google Photos plugin's program: the `import.source` service
//! (`docs/design/import-connectors.md`, 2.6) over JSON-RPC 2.0 on its
//! standard input and output, one message per line, as every Helios
//! plugin speaks (`docs/extensions.md`).
//!
//! Every call carries `auth`, the access token Helios renewed for the
//! account; the plugin keeps no token. Methods:
//!
//! | Method | Params | Result |
//! |---|---|---|
//! | `source.probe` | `auth` | `{}`: the token works for the Picker |
//! | `source.pick` | `auth`, `maxItems?` | `{session, pickerUri, expires}`: Helios opens `pickerUri` in the browser |
//! | `source.wait` | `auth`, `session`, `timeoutSecs?` | `{ready}`: polled until the person finished picking |
//! | `source.list` | `auth`, `session`, `cursor?`, `pageSize?` | `{items, next, complete}`, each item with its `download` |
//! | `source.fetch` | `auth`, `items` (`id`, `path`, `download`), `staging` | `{fetched: [{id, file, bytes, mime}], failed: [{id, code, message}]}` |
//! | `source.thumbnail` | `auth`, `item` (`id`, `download`), `maxSide`, `staging` | `{file, bytes}`: the item's preview, written into `staging` |
//! | `source.close` | `auth`, `session` | `{}`: the session deleted |
//! | `health.check` | `check`, `accounts` (`id`, `label`, `state`, `auth`) | `{findings}`: its `accounts` check |
//!
//! A token Google refuses answers -32010 ("sign-in needed"): Helios
//! offers to reconnect the account and nothing is retried.
//!
//! Its health check `accounts` (declared in its manifest) tries each
//! account Helios hands it, signed out ones aside: one whose token Google
//! takes is fine, one with no token or a refused one needs reconnecting
//! (the fix `import.reconnect`), one it could not try (offline) is said
//! as a warning without a fix.

use std::io::{BufRead, Write};
use std::path::Path;
use std::time::Duration;

use helios_connect::{Cancel, ConnectorError, FetchSink, Item};
use helios_plugin_google_photos::{API, Download, Picker, item_of};
use serde_json::{Value as Json, json};

const ID: &str = "io.github.quazardous.helios.google-photos";

/// A JSON-RPC error: code and message.
type Failure = (i64, String);

/// The code an error answers with.
fn code(e: &ConnectorError) -> i64 {
    match e {
        ConnectorError::AuthNeeded => -32010,
        ConnectorError::RateLimited { .. } => -32011,
        ConnectorError::Expired | ConnectorError::Gone => -32012,
        ConnectorError::Offline => -32013,
        ConnectorError::Cancelled => -32800,
        _ => -32000,
    }
}

fn failure(e: &ConnectorError) -> Failure {
    let message = match e {
        ConnectorError::AuthNeeded => {
            "Google Photos: sign-in needed, reconnect the account".to_string()
        }
        e => format!("Google Photos: {e}"),
    };
    (code(e), message)
}

fn send(msg: &Json) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{msg}");
    let _ = out.flush();
}

/// A message for people: its key in the plugin's catalogue, its English
/// and its arguments.
fn message(key: &str, english: &str, account: &str) -> Json {
    json!({
        "key": key,
        "english": english.replace("{ $account }", account),
        "args": { "account": account },
    })
}

/// The `accounts` check: one finding per account Helios hands, signed
/// out ones aside.
fn accounts_check(api: &str, params: &Json) -> Json {
    let accounts = params["accounts"].as_array().cloned().unwrap_or_default();
    let findings: Vec<Json> = accounts
        .iter()
        .filter(|a| a["state"] != "not-connected")
        .map(|a| {
            let id = a["id"].as_str().unwrap_or_default();
            let label = a["label"].as_str().unwrap_or(id);
            let tried = match a["auth"].as_str().filter(|t| !t.is_empty()) {
                None => Err(ConnectorError::AuthNeeded),
                Some(token) => {
                    let picker = Picker::new(api, token);
                    picker.create(Some(1)).map(|session| {
                        let _ = picker.delete(&session.id);
                    })
                }
            };
            match tried {
                Ok(()) => json!({
                    "about": id,
                    "level": "ok",
                    "message": message(
                        "google-photos-health-account-ok",
                        "{ $account } signs in",
                        label,
                    ),
                }),
                Err(ConnectorError::AuthNeeded) => json!({
                    "about": id,
                    "level": "warning",
                    "message": message(
                        "google-photos-health-account-reconnect",
                        "{ $account } needs reconnecting",
                        label,
                    ),
                    "detail": message(
                        "google-photos-health-account-reconnect-detail",
                        "Google no longer takes its sign-in (revoked or aged out): reconnect it to import from it again.",
                        label,
                    ),
                    "fix": {
                        "command": "import.reconnect",
                        "args": { "account": id },
                        "label": {
                            "key": "google-photos-health-fix-reconnect",
                            "english": "Reconnect",
                        },
                    },
                }),
                Err(e) => json!({
                    "about": id,
                    "level": "warning",
                    "message": message(
                        "google-photos-health-account-untried",
                        "{ $account } could not be checked",
                        label,
                    ),
                    "detail": {
                        "key": "",
                        "english": format!("Google Photos: {e}"),
                    },
                }),
            }
        })
        .collect();
    json!({ "findings": findings })
}

/// An item as the protocol carries it.
fn item_json(item: &Item, download: &Download) -> Json {
    json!({
        "id": item.id.0,
        "path": item.path,
        "taken": item.taken,
        "modified": item.modified,
        "kind": item.kind,
        "mime": item.mime,
        "metadata": item.metadata,
        "download": { "baseUrl": download.base_url, "video": download.video },
    })
}

/// A file name for a staged item: its id, nothing that leaves the
/// folder.
fn staged_name(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A file being written.
struct FileSink(std::fs::File);

impl FetchSink for FileSink {
    fn accept(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        self.0.write_all(chunk)
    }
}

struct Plugin {
    api: String,
}

impl Plugin {
    fn picker(&self, params: &Json) -> Result<Picker, Failure> {
        let token = params["auth"]
            .as_str()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| failure(&ConnectorError::AuthNeeded))?;
        Ok(Picker::new(&self.api, token))
    }

    fn session(params: &Json) -> Result<&str, Failure> {
        params["session"]
            .as_str()
            .ok_or_else(|| (-32602, "`session` is missing".into()))
    }

    fn fetch(&self, params: &Json) -> Result<Json, Failure> {
        let picker = self.picker(params)?;
        let staging = params["staging"]
            .as_str()
            .map(Path::new)
            .ok_or_else(|| (-32602, "`staging` is missing".to_string()))?;
        let items = params["items"]
            .as_array()
            .ok_or_else(|| (-32602, "`items` is missing".to_string()))?;
        let (mut fetched, mut failed) = (Vec::new(), Vec::new());
        for item in items {
            let id = item["id"].as_str().unwrap_or_default();
            let Some(base_url) = item["download"]["baseUrl"].as_str() else {
                failed.push(json!({"id": id, "code": -32602, "message": "no `download`"}));
                continue;
            };
            let download = Download {
                base_url: base_url.into(),
                video: item["download"]["video"].as_bool().unwrap_or(false),
            };
            let file = staging.join(staged_name(id));
            let written = std::fs::File::create(&file)
                .map_err(|e| ConnectorError::Fatal(format!("{}: {e}", file.display())))
                .and_then(|f| picker.download(&download, &mut FileSink(f), &Cancel::new()));
            match written {
                Ok(done) => fetched.push(json!({
                    "id": id, "file": file, "bytes": done.bytes, "mime": done.mime,
                })),
                Err(ConnectorError::AuthNeeded) => {
                    let _ = std::fs::remove_file(&file);
                    return Err(failure(&ConnectorError::AuthNeeded));
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&file);
                    let (code, message) = failure(&e);
                    failed.push(json!({"id": id, "code": code, "message": message}));
                }
            }
        }
        Ok(json!({ "fetched": fetched, "failed": failed }))
    }

    /// An item's preview, written into the staging folder: Google's
    /// `=w<n>-h<n>` of its `baseUrl`. An expired address answers -32012:
    /// Helios lists the picking again and asks once more.
    fn thumbnail(&self, params: &Json) -> Result<Json, Failure> {
        let picker = self.picker(params)?;
        let staging = params["staging"]
            .as_str()
            .map(Path::new)
            .ok_or_else(|| (-32602, "`staging` is missing".to_string()))?;
        let item = &params["item"];
        let id = item["id"].as_str().unwrap_or_default();
        let base_url = item["download"]["baseUrl"]
            .as_str()
            .ok_or_else(|| (-32602, "no `download`".to_string()))?;
        let download = Download {
            base_url: base_url.into(),
            video: item["download"]["video"].as_bool().unwrap_or(false),
        };
        let side = params["maxSide"].as_u64().unwrap_or(256).min(2048) as u32;
        let bytes = picker
            .thumbnail(&download, side, &Cancel::new())
            .map_err(|e| failure(&e))?;
        let file = staging.join(format!("{}.glance", staged_name(id)));
        std::fs::write(&file, &bytes)
            .map_err(|e| failure(&ConnectorError::Fatal(format!("{}: {e}", file.display()))))?;
        Ok(json!({ "file": file, "bytes": bytes.len() }))
    }

    fn handle(&mut self, method: &str, params: &Json) -> Result<Json, Failure> {
        match method {
            "initialize" => {
                if let Some(api) = params["settings"][format!("{ID}.api")].as_str() {
                    self.api = api.into();
                }
                Ok(json!({"name": "google-photos", "version": env!("CARGO_PKG_VERSION")}))
            }
            "shutdown" => Ok(Json::Null),
            "source.probe" => {
                let picker = self.picker(params)?;
                let session = picker.create(Some(1)).map_err(|e| failure(&e))?;
                let _ = picker.delete(&session.id);
                Ok(json!({}))
            }
            "source.pick" => {
                let max = params["maxItems"].as_u64().map(|n| n.min(2000) as u32);
                let session = self.picker(params)?.create(max).map_err(|e| failure(&e))?;
                Ok(json!({
                    "session": session.id,
                    "pickerUri": session.autoclose_uri(),
                    "expires": session.expires,
                }))
            }
            "source.wait" => {
                let deadline = params["timeoutSecs"].as_u64().map(Duration::from_secs);
                let ready = self
                    .picker(params)?
                    .wait(Self::session(params)?, deadline, &Cancel::new())
                    .map_err(|e| failure(&e))?;
                Ok(json!({ "ready": ready.is_some() }))
            }
            "source.list" => {
                let size = params["pageSize"].as_u64().unwrap_or(100) as usize;
                let (values, next) = self
                    .picker(params)?
                    .list(Self::session(params)?, params["cursor"].as_str(), size)
                    .map_err(|e| failure(&e))?;
                let items: Vec<Json> = values
                    .iter()
                    .filter_map(item_of)
                    .map(|(item, download)| item_json(&item, &download))
                    .collect();
                Ok(json!({ "items": items, "complete": next.is_none(), "next": next }))
            }
            "health.check" => match params["check"].as_str() {
                Some("accounts") => Ok(accounts_check(&self.api, params)),
                other => Err((-32602, format!("no check {other:?}"))),
            },
            "source.fetch" => self.fetch(params),
            "source.thumbnail" => self.thumbnail(params),
            "source.close" => {
                self.picker(params)?
                    .delete(Self::session(params)?)
                    .map_err(|e| failure(&e))?;
                Ok(json!({}))
            }
            _ => Err((-32601, format!("no method `{method}`"))),
        }
    }
}

fn main() {
    let mut plugin = Plugin { api: API.into() };
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
