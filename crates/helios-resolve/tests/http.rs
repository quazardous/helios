//! The HTTP resolver against a local stand-in server: no network.

// The resolver reads its API key from an environment variable, and setting
// one is unsafe since the 2024 edition. Each test sets its own variable, so
// no two threads race on the same one.
#![allow(unsafe_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use helios_core::*;
use helios_decision::Mode;
use helios_resolve::http::{Api, Http};
use helios_resolve::{Input, Request, Resolver};

/// Serves one request with `status` and `body`; hands back what it got.
fn server(status: u16, body: String) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/endpoint", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut head = String::new();
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
            head.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        let mut payload = vec![0; length];
        reader.read_exact(&mut payload).unwrap();
        tx.send(head + &String::from_utf8_lossy(&payload)).unwrap();
        let mut stream = stream;
        write!(
            stream,
            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    (url, rx)
}

fn request() -> Request {
    Request {
        node: "sky".into(),
        module: "how_bright".into(),
        description: "how much to brighten the sky".into(),
        answer: vec![ParamSpec::float("gain", "decided factor", 0.0, 4.0, 1.0)],
        inputs: vec![Input {
            key: "abc".into(),
            image: Image::from_fn(Region::sized(4, 3), 3, |x, _, _| x as f32 / 4.0),
        }],
        brief: Some("bright and airy".into()),
        mode: Mode::FullAi,
        question: Params::new(),
        region: Region::sized(8, 6),
    }
}

/// An AI object mask's request: its question, a mask to answer.
fn mask_request() -> Request {
    let manifest = helios_module_masks::MaskObject.manifest();
    Request {
        module: manifest.name,
        description: manifest.description,
        answer: manifest.params,
        question: Params::new()
            .with("kind", Value::Choice("sky".into()))
            .with("prompt", Value::Text(String::new()))
            .with("resolver", Value::Text(String::new())),
        ..request()
    }
}

const ANSWER: &str = r#"Here you go:
```json
{"options": [{"gain": 2.5}, {"gain": 2.0}], "confidence": 0.9, "rationale": "underexposed"}
```"#;

#[test]
fn an_openai_compatible_endpoint_answers() {
    let reply = serde_json::json!({
        "model": "vision-2026-09",
        "choices": [{"message": {"content": ANSWER}}]
    });
    let (url, got) = server(200, reply.to_string());
    unsafe { std::env::set_var("HELIOS_TEST_KEY_OPENAI", "sk-test") };
    let http = Http::new(Api::OpenAi, &url, "vision").key_from("HELIOS_TEST_KEY_OPENAI");
    let r = http.resolve(&request()).unwrap();
    assert_eq!(r.options.len(), 2);
    assert_eq!(r.options[0].float("gain"), 2.5);
    assert_eq!(r.confidence, 0.9);
    assert_eq!(
        (r.model.name.as_str(), r.model.version.as_str()),
        ("vision", "vision-2026-09")
    );
    let sent = got.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        sent.to_ascii_lowercase()
            .contains("authorization: bearer sk-test")
    );
    assert!(sent.contains("data:image/png;base64,"));
    assert!(sent.contains("bright and airy") && sent.contains("`gain`"));
}

#[test]
fn an_anthropic_endpoint_answers() {
    let reply = serde_json::json!({
        "model": "claude-vision",
        "content": [{"type": "text", "text": ANSWER}]
    });
    let (url, got) = server(200, reply.to_string());
    unsafe { std::env::set_var("HELIOS_TEST_KEY_ANTHROPIC", "ak-test") };
    let http = Http::new(Api::Anthropic, &url, "claude").key_from("HELIOS_TEST_KEY_ANTHROPIC");
    let r = http.resolve(&request()).unwrap();
    assert_eq!(r.options[0].float("gain"), 2.5);
    let sent = got.recv_timeout(Duration::from_secs(5)).unwrap();
    let (head, body) = sent.split_once("\r\n\r\n").unwrap();
    let head = head.to_ascii_lowercase();
    assert!(head.contains("x-api-key: ak-test"));
    assert!(head.contains("anthropic-version: 2023-06-01"));
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(
        body.pointer("/messages/0/content/0/source/media_type"),
        Some(&serde_json::json!("image/png"))
    );
}

#[test]
fn a_missing_key_is_named_before_any_call() {
    let http = Http::new(Api::OpenAi, "http://127.0.0.1:9/none", "m")
        .key_from("HELIOS_TEST_KEY_THAT_IS_NOT_SET");
    let err = http.resolve(&request()).unwrap_err();
    assert!(err.contains("HELIOS_TEST_KEY_THAT_IS_NOT_SET"), "{err}");
}

#[test]
fn an_error_status_is_reported_with_its_body() {
    let (url, _got) = server(401, r#"{"error": "bad key"}"#.into());
    let err = Http::new(Api::OpenAi, &url, "m")
        .resolve(&request())
        .unwrap_err();
    assert!(err.contains("401") && err.contains("bad key"), "{err}");
}

#[test]
fn a_helios_endpoint_answers_a_mask_inline() {
    let mask = helios_module_masks::object::encode_mask(2, 1, &[255, 0]).unwrap();
    let reply = serde_json::json!({
        "options": [{"mask": {"png_base64": mask}}],
        "confidence": 0.8,
        "rationale": "a clear sky",
        "model": {"name": "segmenter", "version": "1"}
    });
    let (url, got) = server(200, reply.to_string());
    let r = Http::new(Api::Helios, &url, "segmenter")
        .resolve(&mask_request())
        .unwrap();
    let answer = &r.options[0];
    assert_eq!(
        answer.get("mask"),
        Some(&Value::Text(mask)),
        "the mask is kept as the node reads it"
    );
    assert_eq!(answer.get("area"), Some(&Value::Ints(vec![0, 0, 8, 6])));
    assert_eq!(answer.float("confidence"), 0.8);
    let sent = got.recv_timeout(Duration::from_secs(5)).unwrap();
    let (_, body) = sent.split_once("\r\n\r\n").unwrap();
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["question"]["kind"], "sky");
    assert_eq!(body["region"]["width"], 8);
    assert!(body["inputs"][0]["png_base64"].is_string());
}

#[test]
fn a_chat_endpoint_is_not_asked_for_a_mask() {
    let err = Http::new(Api::OpenAi, "http://127.0.0.1:9/none", "m")
        .resolve(&mask_request())
        .unwrap_err();
    assert!(err.contains("cannot answer a mask"), "{err}");
}
