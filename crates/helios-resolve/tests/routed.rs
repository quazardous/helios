//! `Routed` through the AI service's routing map, against a fake runtime.

use std::sync::Arc;

use helios_ai::fake::{FakeRuntime, FakeSystem};
use helios_ai::routing::Route;
use helios_ai::secrets::MemorySecrets;
use helios_ai::store::Paths;
use helios_ai::{Ai, Capability};
use helios_core::{Image, Module, ParamSpec, Params, Region, Value};
use helios_decision::Mode;
use helios_resolve::{Input, Request, Resolver, Routed};

const ANSWER: &str =
    r#"{"options": [{"gain": 2.5}], "confidence": 0.7, "rationale": "a dark sky"}"#;

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
        brief: None,
        mode: Mode::FullAi,
        question: Params::new(),
        region: Region::sized(8, 6),
    }
}

fn ai(name: &str, runtime: Arc<FakeRuntime>) -> Arc<Ai> {
    let dir = std::env::temp_dir().join(format!("helios-resolve-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Arc::new(
        Ai::with_parts(
            Paths::under(&dir),
            runtime,
            Arc::new(MemorySecrets::default()),
            Arc::new(FakeSystem::linux()),
        )
        .unwrap(),
    )
}

#[test]
fn a_decision_without_a_resolver_goes_to_the_vision_route() {
    let runtime = Arc::new(
        FakeRuntime::new()
            .model("qwen2.5vl:7b", &["completion", "vision"])
            .reply("qwen2.5vl:7b", ANSWER),
    );
    let ai = ai("vision", runtime.clone());
    let routed = Routed::new().through(ai.clone());
    let unrouted = routed.resolve(&request()).unwrap_err();
    assert!(
        unrouted.contains("nothing is routed for vision"),
        "{unrouted}"
    );

    ai.routing_set(
        Some(Capability::Vision),
        None,
        Some(Route::new("ollama", "qwen2.5vl:7b")),
    )
    .unwrap();
    let r = routed.resolve(&request()).unwrap();
    assert_eq!(r.options[0].float("gain"), 2.5);
    assert_eq!(
        (r.model.name.as_str(), r.model.version.as_str()),
        ("qwen2.5vl:7b", "qwen2.5vl:7b")
    );
    let chat = runtime.calls().last().unwrap().clone();
    assert_eq!(chat.path(), "/api/chat");
    let body = chat.body.unwrap();
    assert_eq!(body["messages"][0]["images"].as_array().unwrap().len(), 1);
    assert_eq!(body["format"], "json");
    assert!(
        ai.events(0)
            .iter()
            .any(|e| e.kind == "ai.call" && e.payload["purpose"] == "decision sky")
    );
}

#[test]
fn a_named_capability_takes_its_route_and_a_mask_needs_segmentation() {
    let ai = ai("named", Arc::new(FakeRuntime::new()));
    let routed = Routed::new().through(ai);
    let manifest = helios_module_masks::MaskObject.manifest();
    let mask = Request {
        module: manifest.name,
        description: manifest.description,
        answer: manifest.params,
        question: Params::new()
            .with("kind", Value::Choice("sky".into()))
            .with("prompt", Value::Text(String::new()))
            .with("resolver", Value::Text("segmentation".into())),
        ..request()
    };
    let err = routed.resolve(&mask).unwrap_err();
    assert!(err.contains("nothing is routed for segmentation"), "{err}");
    let mut unknown = request();
    unknown.question = Params::new().with("resolver", Value::Text("oracle".into()));
    assert_eq!(
        routed.resolve(&unknown).unwrap_err(),
        "no resolver named `oracle`"
    );
}

#[test]
fn without_the_ai_service_nothing_is_guessed() {
    assert_eq!(
        Routed::new().resolve(&request()).unwrap_err(),
        "no resolver is configured"
    );
}
