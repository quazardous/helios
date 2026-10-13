//! `helios doctor ai` on a machine whose runtime does not answer.

use std::process::Command;

use serde_json::Value;

#[test]
fn doctor_ai_says_what_is_unavailable_and_why() {
    let dir = std::env::temp_dir().join(format!("helios-doctor-ai-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("ai.toml"),
        "[routing.capabilities]\nvision = { provider = \"ollama\", model = \"qwen2.5vl:7b\" }\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_helios"))
        .args(["doctor", "ai"])
        .env("HELIOS_AI_DIR", &dir)
        // Nothing listens on the discard port: the runtime does not answer.
        .env("OLLAMA_HOST", "127.0.0.1:9")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let ollama = &report["providers"][0];
    assert_eq!(ollama["id"], "ollama");
    assert_eq!(ollama["url"], "http://127.0.0.1:9");
    assert_eq!(ollama["remote"], false);
    assert_eq!(ollama["reachable"], false);
    let caps = &report["routing"]["capabilities"];
    assert!(
        caps["vision"]["reason"]
            .as_str()
            .unwrap()
            .contains("does not answer"),
        "{caps}"
    );
    assert!(
        caps["embeddings"]["reason"]
            .as_str()
            .unwrap()
            .contains("nothing is routed")
    );
    assert_eq!(report["runtime"]["running"], false);
    assert_eq!(
        report["files"]["config"].as_str().unwrap(),
        dir.join("ai.toml").to_str().unwrap()
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
