//! The AI service against a fake runtime and a fake machine: routing,
//! prompt versions and pins, trying a prompt, pulling with progress,
//! probing, the setup's confirmations, privacy and the doctor.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use helios_ai::extract::ExtractCall;
use helios_ai::fake::{FakeRuntime, FakeSystem};
use helios_ai::local::System;
use helios_ai::prompts::PromptRef;
use helios_ai::provider::{ProviderConfig, ProviderKind};
use helios_ai::routing::{Level, Overrides, Route, RouteFrom, RouteOverride};
use helios_ai::secrets::{MemorySecrets, Secrets};
use helios_ai::store::Paths;
use helios_ai::{Ai, Capability, Need, PromptCall, images};
use helios_extract::{ExtractorDef, ExtractorUse, KeyDecl, KeyType, MemoryCache};
use serde_json::{Value, json};
use std::time::Instant;

const CAPTION: &str =
    r#"{"caption": "A yellow disc on a gradient", "keywords": ["disc", "gradient"]}"#;

fn dir_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("helios-ai-{name}-{}", std::process::id()))
}

fn dir(name: &str) -> PathBuf {
    let dir = dir_path(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A Linux machine with room on its disk.
fn machine() -> FakeSystem {
    FakeSystem::linux()
        .output(
            "df -Pk /",
            Ok("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/x 900000000 100000000 800000000 12% /\n"),
        )
        .output("id -u", Ok("1000\n"))
}

struct Setup {
    ai: Ai,
    runtime: Arc<FakeRuntime>,
    secrets: Arc<MemorySecrets>,
    dir: PathBuf,
}

fn setup(name: &str, runtime: FakeRuntime, system: FakeSystem) -> Setup {
    let dir = dir(name);
    let runtime = Arc::new(runtime);
    let secrets = Arc::new(MemorySecrets::default());
    let ai = Ai::with_parts(
        Paths::under(&dir),
        runtime.clone(),
        secrets.clone(),
        Arc::new(system),
    )
    .unwrap();
    Setup {
        ai,
        runtime,
        secrets,
        dir,
    }
}

fn vision_runtime() -> FakeRuntime {
    FakeRuntime::new()
        .model("qwen2.5vl:7b", &["completion", "vision"])
        .model("gemma3:4b", &["completion", "vision"])
        .model("qwen3:8b", &["completion"])
        .reply("qwen2.5vl:7b", CAPTION)
        .reply("gemma3:4b", "I think it is a disc.")
}

fn describe() -> PromptRef {
    PromptRef::parse("image.describe").unwrap()
}

fn last_chat_model(runtime: &FakeRuntime) -> String {
    runtime
        .calls()
        .iter()
        .rev()
        .find(|c| c.path().ends_with("/chat") || c.path().ends_with("/completions"))
        .and_then(|c| c.body.as_ref())
        .map(|b| b["model"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default()
}

#[test]
fn a_prompt_route_wins_over_its_capability() {
    let s = setup("routing", vision_runtime(), machine());
    s.ai.routing_set(
        Some(Capability::Vision),
        None,
        Some(Route::new("ollama", "gemma3:4b")),
    )
    .unwrap();
    let card = images::test_card(64);
    let r =
        s.ai.try_prompt(&describe(), None, vec![card.clone()], None)
            .unwrap();
    assert_eq!(last_chat_model(&s.runtime), "gemma3:4b");
    assert!(!r.ok(), "gemma's answer is not JSON");

    s.ai.routing_set(
        None,
        Some("image.describe"),
        Some(Route::new("ollama", "qwen2.5vl:7b")),
    )
    .unwrap();
    let r =
        s.ai.try_prompt(&describe(), None, vec![card], None)
            .unwrap();
    assert_eq!(last_chat_model(&s.runtime), "qwen2.5vl:7b");
    assert!(r.ok(), "{:?}", r.errors);

    // The map is saved, and read back by the next service.
    let text = std::fs::read_to_string(s.dir.join("ai.toml")).unwrap();
    assert!(
        text.contains("image.describe") && text.contains("gemma3:4b"),
        "{text}"
    );
    assert!(s.ai.routing_set(None, None, None).is_err());
    assert!(
        s.ai.routing_set(
            Some(Capability::Chat),
            None,
            Some(Route::new("nobody", "m"))
        )
        .unwrap_err()
        .contains("no provider `nobody`")
    );
}

#[test]
fn trying_returns_the_raw_and_the_validated_answer_and_applies_nothing() {
    let s = setup("try", vision_runtime(), machine());
    let card = images::test_card(64);
    let unrouted =
        s.ai.try_prompt(&describe(), None, vec![card.clone()], None)
            .unwrap();
    assert!(
        unrouted.errors[0].contains("nothing is routed for vision"),
        "{unrouted:?}"
    );

    let good =
        s.ai.try_prompt(
            &describe(),
            Some(json!({ "language": "French", "keywords": 3 })),
            vec![card.clone()],
            Some(Route::new("ollama", "qwen2.5vl:7b")),
        )
        .unwrap();
    assert_eq!(good.raw.as_deref(), Some(CAPTION));
    assert_eq!(good.validated.as_ref().unwrap()["keywords"][0], "disc");
    assert!(!good.applied);
    let sent = s.runtime.calls().last().unwrap().clone();
    let body = sent.body.unwrap();
    assert!(
        body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("in French")
    );
    assert_eq!(
        body["format"]["required"][0], "caption",
        "the output schema is asked for"
    );
    assert_eq!(body["options"]["temperature"], 0.0);

    let bad =
        s.ai.try_prompt(
            &describe(),
            None,
            vec![card.clone()],
            Some(Route::new("ollama", "gemma3:4b")),
        )
        .unwrap();
    assert_eq!(bad.raw.as_deref(), Some("I think it is a disc."));
    assert!(bad.validated.is_none() && !bad.errors.is_empty());

    let wrong_input =
        s.ai.try_prompt(
            &describe(),
            Some(json!({ "language": "French" })),
            vec![card],
            Some(Route::new("ollama", "qwen2.5vl:7b")),
        )
        .unwrap();
    assert!(wrong_input.errors[0].contains("`keywords` is missing"));
    assert!(
        wrong_input.raw.is_none(),
        "an input that does not fit is not sent"
    );

    let no_image =
        s.ai.try_prompt(
            &describe(),
            None,
            Vec::new(),
            Some(Route::new("ollama", "qwen2.5vl:7b")),
        )
        .unwrap();
    assert!(no_image.errors[0].contains("takes 1 image(s)"));
}

#[test]
fn prompt_versions_are_added_and_a_pin_keeps_its_version() {
    let s = setup("prompts", vision_runtime(), machine());
    let shipped = s.ai.prompt(&describe()).unwrap();
    assert_eq!(shipped.version, 1);
    let mut mine = shipped.clone();
    mine.template = format!("{}\nBe terse.", shipped.template);
    mine.tuned_for = vec!["gemma3:4b".into()];
    let v2 = s.ai.set_prompt(mine.clone()).unwrap();
    assert_eq!(v2.version, 2);
    mine.template.push_str("\nVery terse.");
    let v3 = s.ai.set_prompt(mine).unwrap();
    assert_eq!(v3.version, 3);
    assert_ne!(v2.hash(), v3.hash());

    assert_eq!(
        s.ai.prompt(&describe()).unwrap().version,
        3,
        "unpinned: the latest"
    );
    let pinned = PromptRef::parse("image.describe@2").unwrap();
    assert_eq!(
        s.ai.prompt(&pinned).unwrap(),
        v2,
        "pinned: that version, unchanged"
    );
    assert_eq!(
        s.ai.prompt(&PromptRef::parse("image.describe@1").unwrap())
            .unwrap(),
        shipped
    );

    // Written as files, read back by the next service.
    let again = Ai::with_parts(
        Paths::under(&s.dir),
        s.runtime.clone(),
        s.secrets.clone(),
        Arc::new(machine()),
    )
    .unwrap();
    assert_eq!(again.prompt(&pinned).unwrap().template, v2.template);

    // A user file cannot replace a shipped version.
    std::fs::write(
        s.dir.join("prompts/image.describe@1.json"),
        serde_json::to_string(&shipped).unwrap(),
    )
    .unwrap();
    let third = Ai::with_parts(
        Paths::under(&s.dir),
        s.runtime.clone(),
        s.secrets.clone(),
        Arc::new(machine()),
    )
    .unwrap();
    let listed = third.prompts_list();
    assert!(
        listed["problems"][0]
            .as_str()
            .unwrap()
            .contains("cannot replace"),
        "{listed}"
    );
}

fn progress(ai: &Ai) -> Vec<Value> {
    ai.events(0)
        .into_iter()
        .filter(|e| e.kind == "job.progress")
        .map(|e| e.payload)
        .collect()
}

#[test]
fn a_pull_runs_as_a_job_with_progress_events() {
    let s = setup("pull", FakeRuntime::new().pull_steps(4), machine());
    let started = s.ai.pull(None, "qwen3:8b", None).unwrap();
    let job = started["job"].as_u64().unwrap();
    assert_eq!(
        started["disk"]["needed_mib"], 5_720,
        "the catalogue's size, and a tenth more"
    );
    let ended = s.ai.job_wait(job, Duration::from_secs(10));
    assert_eq!(ended["state"], "done", "{ended}");
    assert_eq!(ended["result"]["pulled"], "qwen3:8b");
    let events = progress(&s.ai);
    let percents: Vec<u64> = events
        .iter()
        .filter_map(|e| e["percent"].as_u64())
        .collect();
    assert_eq!(percents, vec![0, 25, 50, 75, 100, 100], "{events:?}");
    assert!(events.iter().all(|e| e["job"] == job));
    assert_eq!(events.last().unwrap()["status"], "success");
    assert!(s.runtime.installed().contains(&"qwen3:8b".to_string()));
}

#[test]
fn a_pull_checks_the_disk_first_and_reports_failures() {
    let small = FakeSystem::linux().output(
        "df -Pk /",
        Ok("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/x 9000000 8000000 1000000 90% /\n"),
    );
    let s = setup("pull-disk", FakeRuntime::new(), small);
    let err = s.ai.pull(None, "qwen2.5vl:7b", None).unwrap_err();
    assert!(err.contains("not enough disk"), "{err}");
    assert!(
        s.runtime.calls().is_empty(),
        "nothing was asked of the runtime"
    );

    let s = setup(
        "pull-fail",
        FakeRuntime::new().pull_error("nope:1b", "pull model manifest: file does not exist"),
        machine(),
    );
    let job = s.ai.pull(None, "nope:1b", Some(100)).unwrap()["job"]
        .as_u64()
        .unwrap();
    let ended = s.ai.job_wait(job, Duration::from_secs(10));
    assert_eq!(ended["state"], "failed", "{ended}");
    assert!(
        ended["result"]["error"]
            .as_str()
            .unwrap()
            .contains("file does not exist")
    );
}

#[test]
fn a_linux_machine_is_probed() {
    let system = machine()
        .file("/proc/meminfo", "MemTotal:       65536000 kB\n")
        .file(
            "/proc/cpuinfo",
            "processor\t: 0\nmodel name\t: Test CPU\nprocessor\t: 1\nmodel name\t: Test CPU\n",
        )
        .file("/etc/os-release", "PRETTY_NAME=\"Fedora Linux 44\"\n")
        .program("nvidia-smi")
        .output(
            "nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits",
            Ok("NVIDIA GeForce RTX 4070, 12282\n"),
        );
    let s = setup("probe", FakeRuntime::new(), system);
    let probe = s.ai.probe();
    assert_eq!(probe.os_name.as_deref(), Some("Fedora Linux 44"));
    assert_eq!(probe.cpu.as_deref(), Some("Test CPU"));
    assert_eq!(probe.threads, Some(2));
    assert_eq!(probe.ram_mib, Some(64000));
    assert_eq!(probe.gpus[0].vram_mib, Some(12282));
    assert!(probe.accelerated && !probe.root);
    assert_eq!(probe.budget_mib, 11053);
    assert!(probe.fits.contains(&"qwen2.5vl:7b".to_string()));
    assert!(!probe.fits.contains(&"qwen2.5vl:32b".to_string()));
    let recommended = s.ai.recommend();
    assert_eq!(
        recommended["capabilities"]["vision"]["recommended"],
        "qwen2.5vl:7b"
    );
    assert!(recommended["capabilities"]["segmentation"]["reason"].is_string());
}

#[test]
fn the_setup_runs_nothing_without_confirm_and_never_as_root() {
    let runtime = FakeRuntime::new();
    runtime.set_down(true);
    let archive = dir_path("setup").join("cache/ollama-linux-amd64.tgz");
    let system = machine().output(
        &format!(
            "curl -fL --proto =https --create-dirs -o {} https://ollama.com/download/ollama-linux-amd64.tgz",
            archive.display()
        ),
        Ok(""),
    );
    let s = setup("setup", runtime, system);
    let shown = s.ai.setup(&[]);
    assert_eq!(shown["ran"], false);
    let outcomes: Vec<&str> = shown["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["outcome"].as_str().unwrap())
        .collect();
    assert!(
        outcomes
            .iter()
            .all(|o| *o == "not_confirmed" || *o == "shown_only"),
        "{outcomes:?}"
    );
    let root_step = shown["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["root"] == true)
        .unwrap();
    assert_eq!(root_step["outcome"], "shown_only");
    assert!(shown["next"].as_str().unwrap().contains("download"));

    let confirmed = s.ai.setup(&["download".to_string()]);
    let download = &confirmed["steps"][0];
    assert_eq!(download["id"], "download");
    assert_eq!(download["outcome"], "ran", "{confirmed}");
    assert_eq!(confirmed["steps"][1]["outcome"], "not_confirmed");
}

#[test]
fn the_setup_refuses_everything_as_root() {
    let runtime = FakeRuntime::new();
    runtime.set_down(true);
    let system = FakeSystem::linux().output("id -u", Ok("0\n"));
    let s = setup("setup-root", runtime, system);
    let all: Vec<String> = [
        "download", "prepare", "unpack", "service", "reload", "start",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    let report = s.ai.setup(&all);
    assert_eq!(report["root"], true);
    assert_eq!(report["ran"], false);
    assert!(
        report["steps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["outcome"] == "refused")
    );
}

#[test]
fn a_remote_provider_gets_downscaled_images_and_its_key_stays_in_the_keyring() {
    let s = setup("remote", vision_runtime(), machine());
    let mut entry = ProviderConfig::new("cloud", ProviderKind::OpenAi);
    entry.url = Some("https://models.example.com/v1".into());
    let added = s.ai.add_provider(entry, Some("sk-secret")).unwrap();
    assert_eq!(added["remote"], true);
    assert_eq!(
        s.secrets.get("cloud").unwrap().as_deref(),
        Some("sk-secret")
    );
    let text = std::fs::read_to_string(s.dir.join("ai.toml")).unwrap();
    assert!(
        !text.contains("sk-secret") && text.contains("keyring"),
        "{text}"
    );

    let big = images::test_card(1600);
    let r =
        s.ai.try_prompt(
            &describe(),
            None,
            vec![big],
            Some(Route::new("cloud", "qwen2.5vl:7b")),
        )
        .unwrap();
    assert!(r.ok(), "{:?}", r.errors);
    let call = s.runtime.calls().last().unwrap().clone();
    assert!(
        call.headers
            .contains(&("Authorization".into(), "Bearer sk-secret".into()))
    );
    let url = call.body.unwrap()["messages"][1]["content"][1]["image_url"]["url"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, b64) = url.split_once(";base64,").unwrap();
    let sent = STANDARD.decode(b64).unwrap();
    assert_eq!(images::dimensions(&sent), Some((1024, 1024)));

    // Every call is logged, never with its key or its images.
    let log = std::fs::read_to_string(s.dir.join("calls.jsonl")).unwrap();
    let last: Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    assert_eq!(last["provider"], "cloud");
    assert_eq!(last["remote"], true);
    assert_eq!(last["prompt"], "image.describe@1");
    assert!(!log.contains("sk-secret") && !log.contains(b64));
    assert!(s.ai.events(0).iter().any(|e| e.kind == "ai.call"));
    assert!(
        s.ai.events(0)
            .iter()
            .any(|e| e.kind == "scheduler.job_ended")
    );

    let removed = s.ai.remove_provider("cloud").unwrap();
    assert_eq!(removed["removed"], "cloud");
    assert_eq!(s.secrets.get("cloud").unwrap(), None);
}

#[test]
fn the_doctor_says_why_a_capability_is_unavailable() {
    let s = setup("doctor", vision_runtime(), machine());
    s.ai.routing_set(
        Some(Capability::Vision),
        None,
        Some(Route::new("ollama", "qwen2.5vl:7b")),
    )
    .unwrap();
    s.ai.routing_set(
        Some(Capability::Chat),
        None,
        Some(Route::new("ollama", "llama3.2:3b")),
    )
    .unwrap();
    s.ai.routing_set(
        Some(Capability::Embeddings),
        None,
        Some(Route::new("ollama", "qwen3:8b")),
    )
    .unwrap();
    let report = s.ai.doctor();
    assert_eq!(report["providers"][0]["id"], "ollama");
    assert_eq!(report["providers"][0]["reachable"], true);
    assert_eq!(report["providers"][0]["remote"], false);
    let caps = &report["routing"]["capabilities"];
    assert_eq!(caps["vision"]["available"], true, "{caps}");
    assert!(
        caps["chat"]["reason"]
            .as_str()
            .unwrap()
            .contains("has no model `llama3.2:3b`")
    );
    assert!(
        caps["embeddings"]["reason"]
            .as_str()
            .unwrap()
            .contains("does not do embeddings")
    );
    assert!(
        caps["segmentation"]["reason"]
            .as_str()
            .unwrap()
            .contains("nothing is routed")
    );
    assert_eq!(
        caps["structured"]["available"], false,
        "chat stands in, and is not available"
    );
    assert_eq!(report["runtime"]["running"], true);
    assert_eq!(report["runtime"]["installed"], false);

    s.runtime.set_down(true);
    let down = s.ai.doctor();
    assert_eq!(down["providers"][0]["reachable"], false);
    assert!(
        down["routing"]["capabilities"]["vision"]["reason"]
            .as_str()
            .unwrap()
            .contains("does not answer")
    );
}

#[test]
fn the_bench_measures_local_models_and_fills_the_routing_on_request() {
    let runtime = vision_runtime().reply(
        "qwen3:8b",
        r#"{"adjustments": [{"target": "sky", "property": "saturation", "direction": "less", "strength": 0.5}]}"#,
    );
    let s = setup("bench", runtime, machine());
    let started =
        s.ai.bench(helios_ai::BenchRequest {
            prompts: vec!["image.describe".into(), "intent.adjustments".into()],
            apply: true,
            runs: 1,
            ..helios_ai::BenchRequest::default()
        })
        .unwrap();
    let ended =
        s.ai.job_wait(started["job"].as_u64().unwrap(), Duration::from_secs(20));
    assert_eq!(ended["state"], "done", "{ended}");
    let results = ended["result"]["results"].as_array().unwrap();
    let ok: Vec<(&str, &str)> = results
        .iter()
        .filter(|r| r["ok"] == true)
        .map(|r| (r["prompt"].as_str().unwrap(), r["model"].as_str().unwrap()))
        .collect();
    assert!(
        ok.contains(&("image.describe", "qwen2.5vl:7b")),
        "{results:?}"
    );
    assert!(
        ok.contains(&("intent.adjustments", "qwen3:8b")),
        "{results:?}"
    );
    assert!(!ok.contains(&("image.describe", "gemma3:4b")));
    let routing = s.ai.config().routing;
    assert_eq!(
        routing.prompts["image.describe"],
        Route::new("ollama", "qwen2.5vl:7b")
    );
    assert_eq!(
        routing.capabilities[&Capability::Vision],
        Route::new("ollama", "qwen2.5vl:7b")
    );
    assert!(!progress(&s.ai).is_empty());
}

#[test]
fn the_tools_answer_by_request() {
    let s = setup("tools", vision_runtime(), machine());
    let call = |request: Value| helios_ai::tools::call(&s.ai, &request, Vec::new());
    let listed = call(json!({ "cmd": "ai.providers.list" })).unwrap();
    assert_eq!(listed[0]["id"], "ollama");
    let models = call(json!({ "cmd": "ai.models.list", "provider": "ollama" })).unwrap();
    assert_eq!(models[0]["models"].as_array().unwrap().len(), 3);
    call(json!({ "cmd": "ai.routing.set", "capability": "vision", "provider": "ollama", "model": "qwen2.5vl:7b" }))
        .unwrap();
    let file = s.dir.join("card.png");
    std::fs::write(&file, images::test_card(32)).unwrap();
    let tried =
        call(json!({ "cmd": "ai.prompts.try", "prompt": "image.describe", "file": file })).unwrap();
    assert_eq!(tried["validated"]["caption"], "A yellow disc on a gradient");
    let set = call(json!({ "cmd": "ai.prompts.set", "prompt": {
        "id": "stacks.suggest", "capability": "vision", "images": 1,
        "template": "Which is best?", "output": { "type": "object" } } }))
    .unwrap();
    assert_eq!(set["written"], "stacks.suggest@1");
    assert!(call(json!({ "cmd": "ai.nope" })).is_err());
    for cmd in helios_ai::tools::CMDS {
        let answer = call(json!({ "cmd": cmd }));
        if let Err(e) = answer {
            assert!(!e.starts_with("no request"), "{cmd}: {e}");
        }
    }
}

#[test]
fn the_fake_machine_records_what_it_ran() {
    let system = machine();
    assert!(system.run("id", &["-u"]).is_ok());
    assert_eq!(system.ran(), vec!["id -u".to_string()]);
}

#[test]
fn a_prompt_that_is_not_an_object_is_refused() {
    let s = setup("prompt-shape", FakeRuntime::new(), machine());
    for prompt in [
        json!("image.describe"),
        json!(7),
        json!([1, 2]),
        json!(true),
    ] {
        let request = json!({ "cmd": "ai.prompts.set", "prompt": prompt });
        let err = helios_ai::tools::call(&s.ai, &request, Vec::new()).unwrap_err();
        assert!(
            err.contains("`prompt` must be an object"),
            "{prompt}: {err}"
        );
    }
}

#[test]
fn an_absurd_pull_size_is_refused() {
    let s = setup("pull-size", FakeRuntime::new(), machine());
    let request = json!({ "cmd": "ai.local.pull", "model": "x:1b", "size_mib": u64::MAX });
    let err = helios_ai::tools::call(&s.ai, &request, Vec::new()).unwrap_err();
    assert!(err.contains("is not a model's size"), "{err}");
    assert!(
        s.runtime.calls().is_empty(),
        "nothing was asked of the runtime"
    );
}

fn describe_call(overrides: Overrides) -> PromptCall {
    let mut call = PromptCall::new(describe(), json!({ "language": "English", "keywords": 3 }));
    call.images = vec![images::test_card(64)];
    call.overrides = overrides;
    call
}

fn log_lines(s: &Setup) -> Vec<Value> {
    std::fs::read_to_string(&s.ai.paths().log)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn a_run_override_wins_and_the_answering_route_is_returned_and_logged() {
    let s = setup("run-override", vision_runtime(), machine());
    s.ai.routing_set(
        Some(Capability::Vision),
        None,
        Some(Route::new("ollama", "gemma3:4b")),
    )
    .unwrap();
    let overrides = Overrides {
        run: Some(RouteOverride::model("qwen2.5vl:7b")),
        node: Some(RouteOverride::model("qwen3:8b")),
        diagram: None,
    };
    let run = s.ai.run_prompt_with(describe_call(overrides)).unwrap();
    assert_eq!(run.route, Route::new("ollama", "qwen2.5vl:7b"));
    assert_eq!(run.from, RouteFrom::Override(Level::Run));
    assert_eq!(run.answered, "qwen2.5vl:7b");
    assert_eq!(run.prompt, "image.describe@1");
    assert_eq!(run.attempts, 1);
    assert_eq!(run.value["keywords"][0], "disc");
    assert_eq!(last_chat_model(&s.runtime), "qwen2.5vl:7b");

    let log = log_lines(&s);
    let line = log.last().unwrap();
    assert_eq!(line["model"], "qwen2.5vl:7b");
    assert_eq!(line["answered"], "qwen2.5vl:7b");
    assert_eq!(line["prompt"], "image.describe@1");
    assert_eq!(line["attempt"], 1);
    assert_eq!(line["route_from"], json!({ "override": "run" }));
}

#[test]
fn a_bad_answer_is_retried_once_with_its_errors_then_fits() {
    let runtime = vision_runtime().replies("qwen2.5vl:7b", &["not json at all"]);
    let s = setup("run-retry", runtime, machine());
    s.ai.routing_set(
        Some(Capability::Vision),
        None,
        Some(Route::new("ollama", "qwen2.5vl:7b")),
    )
    .unwrap();
    let run =
        s.ai.run_prompt_with(describe_call(Overrides::default()))
            .unwrap();
    assert_eq!(run.attempts, 2);
    assert_eq!(run.from, RouteFrom::Capability(Capability::Vision));
    let chats: Vec<_> = s
        .runtime
        .calls()
        .into_iter()
        .filter(|c| c.path().ends_with("/chat"))
        .collect();
    assert_eq!(chats.len(), 2);
    let second = chats[1].body.as_ref().unwrap()["messages"][1]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(second.contains("Your answer did not fit:\n- "), "{second}");
    assert!(second.contains("Answer again with one JSON object"));
    let attempts: Vec<_> = log_lines(&s).iter().map(|l| l["attempt"].clone()).collect();
    assert_eq!(attempts, vec![json!(1), json!(2)]);
}

#[test]
fn two_bad_answers_report_both_tries() {
    let s = setup("run-fail", vision_runtime(), machine());
    let overrides = Overrides {
        diagram: Some(RouteOverride::route("ollama", "gemma3:4b")),
        ..Overrides::default()
    };
    let failure = s.ai.run_prompt_with(describe_call(overrides)).unwrap_err();
    assert_eq!(failure.route, Some(Route::new("ollama", "gemma3:4b")));
    assert_eq!(failure.attempts.len(), 2);
    for a in &failure.attempts {
        assert_eq!(a.raw.as_deref(), Some("I think it is a disc."));
        assert!(!a.errors.is_empty());
    }
    assert!(failure.to_string().contains("after the retry"), "{failure}");
    // `run_prompt` says the same, as text.
    s.ai.routing_set(
        Some(Capability::Vision),
        None,
        Some(Route::new("ollama", "gemma3:4b")),
    )
    .unwrap();
    let err =
        s.ai.run_prompt(
            &describe(),
            json!({ "language": "English", "keywords": 3 }),
            vec![images::test_card(64)],
        )
        .unwrap_err();
    assert!(err.starts_with("image.describe@1: "), "{err}");
}

fn caption_extractor() -> ExtractorDef {
    let mut def: ExtractorDef = toml::from_str(
        r#"
id = "caption"
version = 1
title = "Caption"
keys = []
[llm]
capability = "vision"
prompt_id = "image.describe@1"
"#,
    )
    .unwrap();
    def.keys = vec![
        KeyDecl::new("caption", KeyType::Text),
        KeyDecl::new("keywords", KeyType::Label)
            .finite(&["disc", "sky"])
            .multi(),
    ];
    def
}

#[test]
fn an_extractor_check_counts_the_values_it_dropped() {
    let s = setup("run-extract", vision_runtime(), machine());
    let mut call = describe_call(Overrides {
        node: Some(RouteOverride::route("ollama", "qwen2.5vl:7b")),
        ..Overrides::default()
    });
    call.extractor = Some(caption_extractor());
    let run = s.ai.run_prompt_with(call).unwrap();
    assert_eq!(run.from, RouteFrom::Override(Level::Node));
    let answer = run.answer.unwrap();
    assert_eq!(answer.get("keywords").len(), 1, "{answer:?}");
    assert_eq!(run.counts.dropped, 1, "`gradient` is not in the list");
    assert_eq!(run.attempts, 1);
}

fn extract_call(def: &ExtractorDef, image: &str, model: &str) -> ExtractCall {
    let mut call = describe_call(Overrides {
        node: Some(RouteOverride::route("ollama", model)),
        ..Overrides::default()
    });
    call.extractor = Some(def.clone());
    ExtractCall {
        call,
        using: ExtractorUse::of(def),
        image: image.into(),
        image_version: None,
        inputs: std::collections::BTreeMap::new(),
    }
}

#[test]
fn a_cached_extractor_result_calls_no_model_and_a_new_version_or_model_misses() {
    let runtime = vision_runtime().model("llava:7b", &["completion", "vision"]);
    let runtime = runtime.reply("llava:7b", CAPTION);
    let s = setup("run-extract-cache", runtime, machine());
    let cache = MemoryCache::default();
    let def = caption_extractor();

    let first =
        s.ai.run_extract(extract_call(&def, "img-a", "qwen2.5vl:7b"), &cache)
            .unwrap();
    assert!(!first.hit);
    assert_eq!(first.result.model, "ollama/qwen2.5vl:7b");
    assert_eq!(first.result.pinned(), "caption@1");
    assert_eq!(first.result.values["keywords"].len(), 1);
    assert_eq!(chats(&s), 1);

    // The same use again: from the cache, no call.
    let again =
        s.ai.run_extract(extract_call(&def, "img-a", "qwen2.5vl:7b"), &cache)
            .unwrap();
    assert!(again.hit);
    assert_eq!(again.result, first.result);
    assert_eq!(chats(&s), 1, "a second run calls no model");

    // Another image, a new version, another model: each a miss.
    let mut v2 = def.clone();
    v2.version = 2;
    for (d, image, model) in [
        (&def, "img-b", "qwen2.5vl:7b"),
        (&v2, "img-a", "qwen2.5vl:7b"),
        (&def, "img-a", "llava:7b"),
    ] {
        let x =
            s.ai.run_extract(extract_call(d, image, model), &cache)
                .unwrap();
        assert!(!x.hit, "{image} {} {model}", d.pinned());
    }
    assert_eq!(chats(&s), 4);

    // A failed answer stores nothing: the next try calls again.
    let failed =
        s.ai.run_extract(extract_call(&def, "img-c", "gemma3:4b"), &cache);
    assert!(failed.is_err());
    assert!(
        s.ai.run_extract(extract_call(&def, "img-c", "gemma3:4b"), &cache)
            .is_err()
    );
    assert_eq!(chats(&s), 8, "two tries each, nothing cached");
}

#[test]
fn kept_results_outlive_the_run_and_run_only_ones_do_not() {
    let s = setup("run-extract-keep", vision_runtime(), machine());
    let cache = MemoryCache::default();
    let mut def = caption_extractor();
    def.keep = true;
    let mut run_only = extract_call(&def, "img-b", "qwen2.5vl:7b");
    run_only.using.keep = Some(false);

    s.ai.run_extract(extract_call(&def, "img-a", "qwen2.5vl:7b"), &cache)
        .unwrap();
    s.ai.run_extract(run_only.clone(), &cache).unwrap();
    assert!(cache.kept("caption", "img-a").is_some());
    assert!(cache.kept("caption", "img-b").is_none());
    assert_eq!(chats(&s), 2);

    cache.end_run();
    let kept =
        s.ai.run_extract(extract_call(&def, "img-a", "qwen2.5vl:7b"), &cache)
            .unwrap();
    assert!(kept.hit, "kept data survives the run");
    let gone = s.ai.run_extract(run_only, &cache).unwrap();
    assert!(!gone.hit, "run-only data went with its run");
    assert_eq!(chats(&s), 3);
}

#[test]
fn the_check_before_a_run_names_what_nothing_can_answer() {
    let s = setup("needs", vision_runtime(), machine());
    s.ai.routing_set(
        Some(Capability::Vision),
        None,
        Some(Route::new("ollama", "qwen2.5vl:7b")),
    )
    .unwrap();
    let needs = [
        Need {
            node: None,
            capability: Capability::Vision,
            prompt: Some("image.describe@1".into()),
            overrides: Overrides::default(),
        },
        Need {
            node: None,
            capability: Capability::Vision,
            prompt: None,
            overrides: Overrides {
                node: Some(RouteOverride::model("llava:missing")),
                ..Overrides::default()
            },
        },
        Need {
            node: None,
            capability: Capability::Embeddings,
            prompt: None,
            overrides: Overrides::default(),
        },
        Need {
            node: None,
            capability: Capability::Vision,
            prompt: None,
            overrides: Overrides {
                run: Some(RouteOverride::route("nobody", "m")),
                ..Overrides::default()
            },
        },
    ];
    let checks = s.ai.check_needs(&needs);
    assert!(checks[0].available, "{:?}", checks[0]);
    assert_eq!(checks[0].route, Some(Route::new("ollama", "qwen2.5vl:7b")));
    assert!(!checks[1].available);
    assert!(
        checks[1]
            .reason
            .as_deref()
            .unwrap()
            .contains("has no model `llava:missing`"),
        "{:?}",
        checks[1]
    );
    assert!(
        checks[2]
            .reason
            .as_deref()
            .unwrap()
            .contains("nothing is routed for embeddings")
    );
    assert!(
        checks[3]
            .reason
            .as_deref()
            .unwrap()
            .contains("no provider `nobody`")
    );
    let tags = s
        .runtime
        .calls()
        .iter()
        .filter(|c| c.path() == "/api/tags")
        .count();
    assert_eq!(tags, 1, "each provider is asked for its models once");
}

#[test]
fn derived_extractor_schemas_stay_in_the_schema_subset() {
    let mut defs = helios_extract::builtins().extractors;
    let mut d = caption_extractor();
    d.keys.push(
        KeyDecl::new("mood", KeyType::Label)
            .semi_finite(&["calm"])
            .with_confidence(),
    );
    defs.push(d);
    for d in &defs {
        let s = helios_extract::answer_schema(d);
        assert!(helios_ai::schema::lint(&s).is_empty(), "{}", d.pinned());
    }
    // A fitting answer passes both the generic check and the extractor's.
    let d = defs.last().unwrap();
    let good =
        json!({ "caption": "a disc", "keywords": ["disc"], "mood": { "value": "calm", "p": 0.8 } });
    assert!(helios_ai::schema::validate(&helios_extract::answer_schema(d), &good).is_empty());
    assert!(helios_extract::validate(d, &good).is_ok());
}

/// A setup whose `ai.toml` is `toml` before the service opens.
fn setup_with(name: &str, runtime: FakeRuntime, toml: &str) -> Setup {
    let dir = dir(name);
    std::fs::write(dir.join("ai.toml"), toml).unwrap();
    let runtime = Arc::new(runtime);
    let secrets = Arc::new(MemorySecrets::default());
    let ai = Ai::with_parts(
        Paths::under(&dir),
        runtime.clone(),
        secrets.clone(),
        Arc::new(machine()),
    )
    .unwrap();
    Setup {
        ai,
        runtime,
        secrets,
        dir,
    }
}

fn add_cloud(s: &Setup) {
    let mut entry = ProviderConfig::new("cloud", ProviderKind::OpenAi);
    entry.url = Some("https://models.example.com/v1".into());
    s.ai.add_provider(entry, Some("sk-secret")).unwrap();
}

fn node(name: &str, capability: Capability, overrides: Overrides) -> Need {
    Need {
        node: Some(name.into()),
        overrides,
        ..Need::new(capability)
    }
}

fn chats(s: &Setup) -> usize {
    s.runtime
        .calls()
        .iter()
        .filter(|c| c.path().ends_with("/chat") || c.path().ends_with("/completions"))
        .count()
}

#[test]
fn a_run_is_refused_before_anything_runs_naming_each_node_nothing_serves() {
    let s = setup("run-check", vision_runtime(), machine());
    add_cloud(&s);
    s.ai.routing_set(
        Some(Capability::Vision),
        None,
        Some(Route::new("ollama", "qwen2.5vl:7b")),
    )
    .unwrap();
    let caption = Need {
        prompt: Some("image.describe@1".into()),
        ..node("caption", Capability::Vision, Overrides::default())
    };
    let remote = node(
        "tags",
        Capability::Vision,
        Overrides {
            node: Some(RouteOverride::route("cloud", "qwen2.5vl:7b")),
            ..Overrides::default()
        },
    );
    let missing = node(
        "missing",
        Capability::Vision,
        Overrides {
            node: Some(RouteOverride::model("llava:missing")),
            ..Overrides::default()
        },
    );
    let unserved = node(
        "blind",
        Capability::Vision,
        Overrides {
            run: Some(RouteOverride::route("ollama", "qwen3:8b")),
            ..Overrides::default()
        },
    );
    let unrouted = node("embed", Capability::Embeddings, Overrides::default());
    let unknown_prompt = Need {
        prompt: Some("image.nothing@1".into()),
        ..node("ghost", Capability::Vision, Overrides::default())
    };
    let needs = [
        caption.clone(),
        remote.clone(),
        missing,
        unserved,
        unrouted,
        unknown_prompt,
    ];
    let check = s.ai.check_run(&needs, Some(10));
    assert!(!check.ok);
    assert_eq!(check.refused.len(), 4, "{:?}", check.refused);
    for (name, why) in [
        ("missing", "has no model `llava:missing`"),
        ("blind", "does not do vision"),
        ("embed", "nothing is routed for embeddings"),
        ("ghost", "image.nothing"),
    ] {
        assert!(
            check
                .refused
                .iter()
                .any(|r| r.starts_with(&format!("node `{name}`")) && r.contains(why)),
            "{name}: {:?}",
            check.refused
        );
    }
    let refusal = s.ai.check_submission(&needs, Some(10)).unwrap_err();
    assert!(
        refusal.starts_with("the run cannot start: ")
            && refusal.contains("node `missing` (vision)")
            && refusal.contains("AI settings"),
        "{refusal}"
    );
    assert_eq!(chats(&s), 0, "a refused run calls no model");

    // What leaves the machine: the remote node, to whom, how many images.
    let ok =
        s.ai.check_submission(&[caption.clone(), remote], Some(10))
            .unwrap();
    assert_eq!(ok.nodes[0].remote, Some(false));
    assert_eq!(ok.nodes[1].remote, Some(true));
    assert_eq!(ok.leaves.len(), 1, "{:?}", ok.leaves);
    let leaving = &ok.leaves[0];
    assert_eq!(leaving.node.as_deref(), Some("tags"));
    assert_eq!(leaving.provider, "cloud");
    assert_eq!(
        leaving.url.as_deref(),
        Some("https://models.example.com/v1")
    );
    assert_eq!((leaving.images_per_item, leaving.images), (1, Some(10)));
    assert_eq!(leaving.image_side, 1024);
    assert_eq!(ok.remote_calls, Some(10));
    assert_eq!(ok.remote_ceiling, 500);

    // A stopped provider is reported before the run.
    s.runtime.set_down(true);
    let down = s.ai.check_submission(&[caption], None).unwrap_err();
    assert!(
        down.contains("node `caption` (vision, image.describe@1): `ollama` does not answer"),
        "{down}"
    );
}

#[test]
fn a_provider_at_its_limit_makes_the_next_call_wait_then_go() {
    let s = setup_with(
        "limit-wait",
        vision_runtime(),
        "[routing.capabilities]\nvision = { provider = \"ollama\", model = \"qwen2.5vl:7b\" }\n\
         [limits.providers.ollama]\nparallel = 2\n",
    );
    s.runtime.hold(true);
    std::thread::scope(|scope| {
        let calls: Vec<_> = (0..3)
            .map(|_| scope.spawn(|| s.ai.run_prompt_with(describe_call(Overrides::default()))))
            .collect();
        let deadline = Instant::now() + Duration::from_secs(10);
        while s.runtime.chatting() < 2 {
            assert!(Instant::now() < deadline, "two calls never started");
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(s.runtime.chatting(), 2, "the third call waits for a slot");
        s.runtime.hold(false);
        for call in calls {
            let run = call.join().unwrap();
            assert!(run.is_ok(), "{run:?}");
        }
    });
    assert_eq!(s.runtime.peak(), 2);
    assert_eq!(
        chats(&s),
        3,
        "the waiting call went on once a slot was free"
    );
}

#[test]
fn a_run_past_its_remote_ceiling_is_refused_and_local_calls_do_not_count() {
    let s = setup_with(
        "ceiling",
        vision_runtime(),
        "[routing.capabilities]\nvision = { provider = \"ollama\", model = \"qwen2.5vl:7b\" }\n\
         [limits]\nrun_remote_calls = 2\n",
    );
    add_cloud(&s);
    let call = |remote: bool| {
        let mut c = describe_call(if remote {
            Overrides {
                run: Some(RouteOverride::route("cloud", "qwen2.5vl:7b")),
                ..Overrides::default()
            }
        } else {
            Overrides::default()
        });
        c.run = Some("run-1".into());
        s.ai.run_prompt_with(c)
    };
    assert!(call(true).is_ok());
    assert!(call(false).is_ok(), "a local call is not counted");
    assert!(call(true).is_ok());
    let refused = call(true).unwrap_err().to_string();
    assert!(
        refused.contains("the run `run-1` reached its ceiling of 2 remote calls"),
        "{refused}"
    );
    assert_eq!(s.ai.run_remote_calls("run-1"), 2);
    let remote_chats = s
        .runtime
        .calls()
        .iter()
        .filter(|c| c.path().ends_with("/completions"))
        .count();
    assert_eq!(remote_chats, 2, "the refused call never left");
    s.ai.forget_run("run-1");
    assert!(call(true).is_ok());
}
