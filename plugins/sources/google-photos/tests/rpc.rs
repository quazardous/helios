//! The plugin's program over JSON-RPC against a fake Photos Picker API:
//! pick, wait, list with paging, fetch the originals into the staging
//! folder (a video through `=dv`), close the session; a refused token
//! answers "sign-in needed" (-32010).

mod fake_picker;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use fake_picker::{FakePicker, TOKEN};
use serde_json::{Value, json};

const ID: &str = "io.github.quazardous.helios.google-photos";

struct Plugin {
    child: std::process::Child,
    lines: mpsc::Receiver<String>,
    next: u64,
}

impl Plugin {
    fn start(api: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_helios-google-photos"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut plugin = Self {
            child,
            lines,
            next: 0,
        };
        plugin
            .call(
                "initialize",
                json!({ "settings": { format!("{ID}.api"): api } }),
            )
            .unwrap();
        plugin
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.next += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params});
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(30))
            .expect("the plugin answers within 30 s");
        let answer: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(answer["id"], self.next);
        match answer.get("error") {
            Some(e) => Err(e.clone()),
            None => Ok(answer["result"].clone()),
        }
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_program_picks_waits_lists_fetches_and_closes() {
    let fake = FakePicker::start(
        2,
        vec![
            ("m1", "IMG_1.jpg", false, b"one".to_vec()),
            ("m2", "IMG_2.jpg", false, b"two".to_vec()),
            ("v3", "VID_3.mp4", true, b"three".to_vec()),
        ],
    );
    let mut plugin = Plugin::start(&fake.base);
    let picked = plugin
        .call("source.pick", json!({ "auth": TOKEN, "maxItems": 50 }))
        .unwrap();
    assert_eq!(picked["session"], "s1");
    assert!(
        picked["pickerUri"]
            .as_str()
            .unwrap()
            .ends_with("/pick/s1/autoclose")
    );
    let waited = plugin
        .call(
            "source.wait",
            json!({ "auth": TOKEN, "session": "s1", "timeoutSecs": 20 }),
        )
        .unwrap();
    assert_eq!(waited["ready"], true);

    let mut items = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let page = plugin
            .call(
                "source.list",
                json!({ "auth": TOKEN, "session": "s1", "cursor": cursor }),
            )
            .unwrap();
        items.extend(page["items"].as_array().unwrap().iter().cloned());
        if page["complete"] == true {
            break;
        }
        cursor = page["next"].clone();
    }
    assert_eq!(items.len(), 3);
    assert_eq!(items[2]["kind"], "video");
    assert_eq!(items[2]["download"]["video"], true);

    let staging = std::env::temp_dir().join(format!("helios-gphotos-rpc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).unwrap();
    let fetched = plugin
        .call(
            "source.fetch",
            json!({ "auth": TOKEN, "items": items, "staging": staging }),
        )
        .unwrap();
    assert_eq!(fetched["failed"], json!([]));
    let files = fetched["fetched"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(std::fs::read(staging.join("v3")).unwrap(), b"three");
    assert_eq!(files[0]["bytes"], 3);
    let _ = std::fs::remove_dir_all(&staging);

    plugin
        .call("source.close", json!({ "auth": TOKEN, "session": "s1" }))
        .unwrap();
    let requests = fake.requests();
    assert!(requests.contains(&"GET /media/v3=dv".to_string()));
    assert_eq!(requests.last().unwrap(), "DELETE /v1/sessions/s1");
}

#[test]
fn a_refused_token_answers_sign_in_needed() {
    let fake = FakePicker::start(1, vec![]);
    let mut plugin = Plugin::start(&fake.base);
    let error = plugin
        .call("source.pick", json!({ "auth": "revoked" }))
        .unwrap_err();
    assert_eq!(error["code"], -32010);
    assert!(error["message"].as_str().unwrap().contains("reconnect"));
    let error = plugin.call("source.probe", json!({})).unwrap_err();
    assert_eq!(error["code"], -32010, "no token at all");
    assert_eq!(fake.requests().len(), 1, "one refused call, no retry");
}

/// Its health check tries each account Helios hands it: one Google
/// takes is fine, a refused token or none needs reconnecting, a signed
/// out one is left alone.
#[test]
fn its_accounts_check_says_which_account_needs_reconnecting() {
    let fake = FakePicker::start(1, vec![]);
    let mut plugin = Plugin::start(&fake.base);
    let answer = plugin
        .call(
            "health.check",
            json!({
                "check": "accounts",
                "accounts": [
                    {"id": "google-photos-1", "label": "Google Photos — a@example.com", "state": "connected", "auth": TOKEN},
                    {"id": "google-photos-2", "label": "Google Photos — b@example.com", "state": "connected", "auth": "revoked"},
                    {"id": "google-photos-3", "label": "Google Photos", "state": "needs-attention", "auth": null},
                    {"id": "google-photos-4", "label": "Google Photos", "state": "not-connected", "auth": null},
                ],
            }),
        )
        .unwrap();
    let findings = answer["findings"].as_array().unwrap();
    let got: Vec<(&str, &str, &str)> = findings
        .iter()
        .map(|f| {
            (
                f["about"].as_str().unwrap(),
                f["level"].as_str().unwrap(),
                f["message"]["english"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                "google-photos-1",
                "ok",
                "Google Photos — a@example.com signs in"
            ),
            (
                "google-photos-2",
                "warning",
                "Google Photos — b@example.com needs reconnecting"
            ),
            (
                "google-photos-3",
                "warning",
                "Google Photos needs reconnecting"
            ),
        ]
    );
    assert_eq!(findings[1]["fix"]["command"], "import.reconnect");
    assert_eq!(findings[1]["fix"]["args"]["account"], "google-photos-2");
    assert!(findings[0].get("fix").is_none());
    let error = plugin
        .call("health.check", json!({ "check": "quota" }))
        .unwrap_err();
    assert_eq!(error["code"], -32602);
}
