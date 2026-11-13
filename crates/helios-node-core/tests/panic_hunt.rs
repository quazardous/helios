//! The panic hunt: every command the headless node lists runs with hostile
//! params, generated from its schema: wrong types, out-of-range numbers,
//! missing and extra fields, unknown ids, huge and odd strings, deep
//! nesting. Agents send malformed requests; each must end in an answer.
//!
//! Each call runs on a fresh node (a seeded memory vault), on a thread of
//! its own (the in-process transport's), and is waited for at most
//! [`DEADLINE`]. A call fails the hunt when it panics (a panic hook counts
//! every panic, on any thread), hangs past its deadline, or is refused yet
//! announces a change on the node's bus.
//!
//! Strings that look like paths name nothing on disk, so no call reads or
//! writes the machine's files.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_inspect::client::Client;
use helios_node_core::endpoint::{self, VaultService};
use helios_vault::{Image, ImageId, MemoryStore, Store, Vault, parse_hash};
use serde_json::{Map, Value, json};

/// How long one call may run.
const DEADLINE: Duration = Duration::from_secs(4);

/// The photos in the hunt's vault.
const PHOTOS: usize = 3;

/// Stands for the cursor a call's setup opened.
const OPENED_CURSOR: &str = "@opened-cursor";

/// A path that names nothing on any machine.
const NOWHERE: &str = "/nonexistent-helios-panic-hunt/photo.raw";

/// Every panic seen while the hunt runs, with where it happened.
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn panics() -> std::sync::MutexGuard<'static, Vec<String>> {
    PANICS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Counts every panic, then reports it as Rust would.
fn count_panics() {
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(ToString::to_string)
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        panics().push(format!("{message} (at {at})"));
        report(info);
    }));
}

fn image_id(n: usize) -> ImageId {
    ImageId(parse_hash(&format!("{n:064x}")).expect("a hash"))
}

/// The vault every call starts from: a few photos in its Library.
fn seeded() -> MemoryStore {
    let mut store = MemoryStore::default();
    for n in 1..=PHOTOS {
        let image = Image {
            id: image_id(n),
            size: 100,
            partial: parse_hash(&format!("{:064x}", n + 1_000_000)).expect("a hash"),
        };
        store.add_to_library(&image).expect("a photo added");
    }
    store
}

/// A fresh node on a copy of `store`, and its bus.
fn node(store: &MemoryStore) -> (Client, Arc<Bus>) {
    let bus = Arc::new(Bus::new());
    let vault = Arc::new(Vault::new(store.clone()).with_bus(bus.clone()));
    let service = VaultService::new(vault, bus.clone(), Jobs::new(bus.clone()), "panic-hunt");
    let client = Client::in_process(Arc::new(service)).with_origin("agent:panic-hunt");
    (client, bus)
}

/// Odd strings: empty, blank, control characters, huge, bidi and emoji,
/// numbers as text, format patterns, a path to nowhere, a known image id
/// and spoiled forms of it, and the words some commands take.
fn strings(known: &str) -> Vec<String> {
    vec![
        String::new(),
        " ".into(),
        "\0".into(),
        "x".repeat(1 << 16),
        "\u{202e}é🙂\u{fffd}".into(),
        "-1".into(),
        "18446744073709551616".into(),
        "NaN".into(),
        "%s{}${x}{{{date".into(),
        NOWHERE.into(),
        known.into(),
        known.to_uppercase(),
        known[..10].into(),
        format!("{known}0"),
        "auto".into(),
        "none".into(),
    ]
}

/// A value of every JSON kind, each hostile in its own way.
fn hostile(known: &str) -> Vec<Value> {
    let mut values = vec![
        Value::Null,
        json!(true),
        json!(0),
        json!(1),
        json!(-1),
        json!(i64::MIN),
        json!(i64::MAX),
        json!(u64::MAX),
        json!(0.5),
        json!(-0.5),
        json!(1e308),
        json!(-1e308),
    ];
    values.extend(strings(known).into_iter().map(Value::String));
    let mut deep = json!([]);
    for _ in 0..64 {
        deep = json!([deep]);
    }
    values.extend([
        json!([]),
        json!([null]),
        json!([-1, u64::MAX, 0.5]),
        json!([known, "", NOWHERE, "\0"]),
        json!(vec![known; 2_000]),
        json!([[0, 0], [0, 0], [0, 0], [0, 0]]),
        json!([[1e308, -1e308], [-1, -1], [u64::MAX, 0], [0.5, 0.5]]),
        json!([{}]),
        json!([{ "image": known, "field": "rating", "value": u64::MAX }]),
        json!([{ "image": known, "field": "", "op": "add" }]),
        json!([{ "image": known, "field": "tags", "op": "remove", "value": "\0" }]),
        json!([{ "image": "", "field": "rating", "value": 1 }]),
        json!({}),
        json!({ "": null }),
        json!({ "zzz": [1] }),
        json!({ "order": { "by": "\0", "descending": "yes" } }),
        json!({ "min_rating": i64::MIN, "captured_from": i64::MAX, "captured_before": i64::MIN }),
        json!({ "presence": ["gone"], "folder": "" }),
        deep,
    ]);
    values
}

/// A value its schema takes, plain: the first of an enum, the least
/// number, `known` for a string, the fewest items, an object with its
/// required properties.
fn typical(schema: &Value, known: &str) -> Value {
    if let Some(first) = schema["enum"].as_array().and_then(|e| e.first()) {
        return first.clone();
    }
    let kind = match &schema["type"] {
        Value::String(kind) => kind.as_str(),
        Value::Array(kinds) => kinds
            .iter()
            .filter_map(Value::as_str)
            .find(|k| *k != "null")
            .unwrap_or("null"),
        _ => "object",
    };
    match kind {
        "string" => json!(known),
        "integer" => json!(schema["minimum"].as_i64().unwrap_or(0).max(1)),
        "number" => json!(schema["minimum"].as_f64().unwrap_or(0.0)),
        "boolean" => json!(false),
        "array" => {
            let n = schema["minItems"].as_u64().unwrap_or(1);
            let item = typical(&schema["items"], known);
            Value::Array((0..n).map(|_| item.clone()).collect())
        }
        "object" => {
            let required: Vec<&str> = schema["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let fields: Map<String, Value> = schema["properties"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(name, _)| required.contains(&name.as_str()))
                .map(|(name, s)| {
                    let value = if name == "cursor" {
                        json!(OPENED_CURSOR)
                    } else {
                        typical(s, known)
                    };
                    (name.clone(), value)
                })
                .collect();
            Value::Object(fields)
        }
        _ => Value::Null,
    }
}

/// One call of the hunt.
struct Case {
    method: String,
    params: Value,
}

impl Case {
    fn new(method: &str, params: Value) -> Self {
        Self {
            method: method.into(),
            params,
        }
    }
}

/// The calls for command `id` with params `schema`: its params not an
/// object, empty, plain, with an extra field or an odd origin, then each
/// property in turn given every hostile value, and each required one
/// left out.
fn cases_of(id: &str, schema: &Value, known: &str) -> Vec<Case> {
    let base = typical(schema, known);
    let mut cases: Vec<Case> = [
        json!("params"),
        json!(7),
        json!([]),
        json!({}),
        base.clone(),
    ]
    .into_iter()
    .map(|params| Case::new(id, params))
    .collect();
    let with = |name: &str, value: Value| {
        let mut params = base.clone();
        params[name] = value;
        Case::new(id, params)
    };
    cases.push(with("zzz", json!(1)));
    for origin in ["", "robot", "agent:", "\0", "agent:\u{202e}"] {
        cases.push(with("_meta", json!({ "origin": origin })));
    }
    cases.push(with("_meta", json!("origin")));
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    for name in schema["properties"]
        .as_object()
        .into_iter()
        .flat_map(Map::keys)
    {
        for value in hostile(known) {
            cases.push(with(name, value));
        }
        if required.contains(&name.as_str()) {
            let mut params = base.clone();
            if let Some(fields) = params.as_object_mut() {
                fields.remove(name);
            }
            cases.push(Case::new(id, params));
        }
    }
    cases
}

/// The calls on the node's own verbs: `commands.list` and `command.run`
/// with odd params, and methods it does not know.
fn verb_cases(ids: &[String], known: &str) -> Vec<Case> {
    let mut cases = Vec::new();
    for value in hostile(known) {
        for field in ["filter", "window", "enabled_only"] {
            cases.push(Case::new("commands.list", json!({ field: value })));
        }
        cases.push(Case::new("command.run", json!({ "id": value })));
        for field in ["scope", "window", "origin", "params"] {
            cases.push(Case::new(
                "command.run",
                json!({ "id": "library.stacks.list", field: value }),
            ));
        }
        if let Value::String(method) = &value {
            cases.push(Case::new(method, json!({})));
        }
    }
    for id in ids {
        for scope in ["window", "node", "\0"] {
            cases.push(Case::new(
                "command.run",
                json!({ "id": id, "scope": scope, "params": {} }),
            ));
        }
    }
    cases
}

/// A short form of `params` for the report.
fn shown(params: &Value) -> String {
    let text = params.to_string();
    if text.chars().count() > 160 {
        format!("{}…", text.chars().take(160).collect::<String>())
    } else {
        text
    }
}

/// How a call ended.
enum Outcome {
    Answered,
    Refused,
    /// A panic, a hang, or a refusal that changed the vault.
    Wrong(String),
}

/// Runs `case` on a fresh node.
fn run(store: &MemoryStore, case: &Case) -> Outcome {
    let (client, bus) = node(store);
    let mut params = case.params.clone();
    if params["cursor"] == OPENED_CURSOR {
        let opened = client
            .call("cursor.open", json!({}))
            .wait_timeout(DEADLINE)
            .and_then(Result::ok);
        params["cursor"] = opened.map_or(json!(1), |a| a["cursor"].clone());
    }
    let heard = bus.subscribe();
    let before = panics().len();
    let answer = client
        .call(&case.method, params.clone())
        .wait_timeout(DEADLINE);
    let call = format!("`{}` {}", case.method, shown(&params));
    if let Some(first) = panics().get(before) {
        return Outcome::Wrong(format!("{call}: panicked: {first}"));
    }
    match answer {
        None => Outcome::Wrong(format!("{call}: no answer within {DEADLINE:?}")),
        Some(Ok(_)) => Outcome::Answered,
        Some(Err(_)) => {
            let changed: Vec<&str> = std::iter::from_fn(|| heard.try_recv().ok())
                .map(|e| e.kind)
                .filter(|kind| *kind != "control.command")
                .collect();
            if changed.is_empty() {
                Outcome::Refused
            } else {
                Outcome::Wrong(format!("{call}: refused, yet announced {changed:?}"))
            }
        }
    }
}

#[test]
fn every_command_answers_hostile_params_without_panicking() {
    count_panics();
    let store = seeded();
    let known = image_id(1).hex();
    let (client, _) = node(&store);
    let listed = client
        .call("commands.list", json!({}))
        .wait_timeout(DEADLINE)
        .expect("commands.list answers")
        .expect("commands.list succeeds");
    let commands = listed["commands"].as_array().expect("a list of commands");
    assert_eq!(
        commands.len(),
        endpoint::specs().len(),
        "the hunt walks every command the node lists"
    );
    let ids: Vec<String> = commands
        .iter()
        .filter_map(|c| c["id"].as_str().map(String::from))
        .collect();
    let mut cases: Vec<Case> = commands
        .iter()
        .flat_map(|c| cases_of(c["id"].as_str().unwrap_or_default(), &c["params"], &known))
        .collect();
    cases.extend(verb_cases(&ids, &known));

    let (mut answered, mut refused, mut failures) = (0, 0, Vec::new());
    for case in &cases {
        match run(&store, case) {
            Outcome::Answered => answered += 1,
            Outcome::Refused => refused += 1,
            Outcome::Wrong(what) => failures.push(what),
        }
    }
    println!(
        "panic hunt: {} calls on {} commands: {answered} answered, {refused} refused, {} wrong",
        cases.len(),
        ids.len(),
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} of {} calls went wrong:\n{}",
        failures.len(),
        cases.len(),
        failures
            .iter()
            .take(40)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    );
}
