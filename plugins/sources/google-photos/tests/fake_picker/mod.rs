//! A fake Photos Picker API on a local port: one session, polled
//! `polls` times before the picking is done, its items listed two a page,
//! their originals served at `<baseUrl>=d` (`=dv` for a video), every
//! call checked for the bearer token `good`. It records each request.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

pub const TOKEN: &str = "good";

pub struct FakePicker {
    pub base: String,
    pub requests: Arc<Mutex<Vec<String>>>,
}

/// One picked item: id, file name, video or not, bytes.
pub type Picked = (&'static str, &'static str, bool, Vec<u8>);

impl FakePicker {
    pub fn start(polls: u32, items: Vec<Picked>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        let host = base.clone();
        let polled = Arc::new(AtomicU32::new(0));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let target = parts.next().unwrap_or("").to_string();
                let (mut auth, mut length) = (String::new(), 0usize);
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap_or((&h, ""));
                    match k.trim().to_ascii_lowercase().as_str() {
                        "authorization" => auth = v.trim().to_string(),
                        "content-length" => length = v.trim().parse().unwrap_or(0),
                        _ => {}
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                log.lock().unwrap().push(format!("{method} {target}"));
                let (status, kind, payload) = if auth == format!("Bearer {TOKEN}") {
                    route(&method, &target, &host, polls, &polled, &items)
                } else {
                    (
                        401,
                        "application/json",
                        json!({"error": {"code": 401, "status": "UNAUTHENTICATED"}})
                            .to_string()
                            .into_bytes(),
                    )
                };
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&payload);
                let _ = stream.flush();
            }
        });
        Self {
            base: format!("{base}/v1"),
            requests,
        }
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn item_json(host: &str, (id, name, video, _): &Picked) -> Value {
    json!({
        "id": id,
        "createTime": "2024-05-01T10:20:30Z",
        "type": if *video { "VIDEO" } else { "PHOTO" },
        "mediaFile": {
            "baseUrl": format!("{host}/media/{id}"),
            "mimeType": if *video { "video/mp4" } else { "image/png" },
            "filename": name,
            "mediaFileMetadata": { "width": "32", "height": "24",
                "cameraMake": "Google", "cameraModel": "Pixel 8" }
        }
    })
}

fn route(
    method: &str,
    target: &str,
    host: &str,
    polls: u32,
    polled: &AtomicU32,
    items: &[Picked],
) -> (u16, &'static str, Vec<u8>) {
    let ok = |v: Value| (200, "application/json", v.to_string().into_bytes());
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let param = |k: &str| {
        query
            .split('&')
            .find_map(|p| p.strip_prefix(&format!("{k}=")))
            .map(String::from)
    };
    match (method, path) {
        ("POST", "/v1/sessions") => ok(json!({
            "id": "s1",
            "pickerUri": format!("{host}/pick/s1"),
            "pollingConfig": { "pollInterval": "0.01s", "timeoutIn": "30s" },
            "expireTime": "2030-01-01T00:00:00Z",
            "mediaItemsSet": false,
        })),
        ("GET", "/v1/sessions/s1") => {
            let done = polled.fetch_add(1, Ordering::SeqCst) + 1 >= polls;
            if done {
                ok(json!({ "id": "s1", "pickerUri": format!("{host}/pick/s1"),
                    "mediaItemsSet": true }))
            } else {
                ok(json!({ "id": "s1", "pickerUri": format!("{host}/pick/s1"),
                    "mediaItemsSet": false,
                    "pollingConfig": { "pollInterval": "0.01s", "timeoutIn": "30s" } }))
            }
        }
        ("DELETE", "/v1/sessions/s1") => ok(json!({})),
        ("GET", "/v1/mediaItems") => {
            if param("sessionId").as_deref() != Some("s1") {
                return (404, "application/json", b"{}".to_vec());
            }
            if polled.load(Ordering::SeqCst) < polls {
                return (
                    400,
                    "application/json",
                    br#"{"error":{"status":"FAILED_PRECONDITION"}}"#.to_vec(),
                );
            }
            // Two a page, whatever is asked: the service may return fewer.
            let start: usize = param("pageToken").and_then(|t| t.parse().ok()).unwrap_or(0);
            let page: Vec<Value> = items
                .iter()
                .skip(start)
                .take(2)
                .map(|i| item_json(host, i))
                .collect();
            let mut answer = json!({ "mediaItems": page });
            if start + 2 < items.len() {
                answer["nextPageToken"] = json!((start + 2).to_string());
            }
            ok(answer)
        }
        ("GET", p) if p.starts_with("/media/") => {
            let rest = &p["/media/".len()..];
            let found = items.iter().find(|(id, _, video, _)| {
                rest == format!("{id}={}", if *video { "dv" } else { "d" })
            });
            match found {
                Some((.., bytes)) => (200, "application/octet-stream", bytes.clone()),
                None => (404, "text/plain", b"no such media".to_vec()),
            }
        }
        _ => (404, "text/plain", b"not found".to_vec()),
    }
}
