//! The broker against a test plugin (`tests/fixtures/testee`, Python with
//! its standard library alone): manifests, crashes, timeouts, capabilities.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use helios_plugin::{Broker, Notice, Policy, Probed, Scope, Told};
use helios_shell::commands::{Origin, Registry};
use helios_shell::when::Context;
use serde_json::{Value as Json, json};

const TESTEE: &str = "org.example.testee";
const READ: &str = "onService:metadata.read";

/// A directory of the test's own.
fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-plugin-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/testee")
}

/// The tests that run a plugin need Python; without it they say so.
fn python() -> bool {
    let ok = Command::new("python3").arg("--version").output().is_ok();
    if !ok {
        eprintln!("python3 is not installed: skipped");
    }
    ok
}

/// A copy of the test plugin, its manifest edited by `edit`.
fn testee_copy(name: &str, edit: impl Fn(String) -> String) -> PathBuf {
    let dir = scratch(name);
    for f in ["testee.py", "helios-plugin.toml"] {
        let text = std::fs::read_to_string(fixture().join(f)).unwrap();
        let text = if f == "testee.py" { text } else { edit(text) };
        std::fs::write(dir.join(f), text).unwrap();
    }
    dir
}

fn quick() -> Policy {
    Policy {
        timeout: Duration::from_secs(10),
        backoff: Duration::ZERO,
        ..Policy::default()
    }
}

fn testee(policy: Policy) -> Broker {
    let mut b = Broker::new(policy);
    b.add(&fixture(), Scope::Builtin).unwrap();
    b
}

fn manifest_problems(text: &str) -> Vec<String> {
    let dir = scratch("manifest");
    std::fs::write(dir.join("helios-plugin.toml"), text).unwrap();
    Broker::new(Policy::default())
        .add(&dir, Scope::User)
        .err()
        .unwrap_or_default()
}

const GOOD: &str = r#"
id = "org.example.good"
version = "1.0.0"
license = "MIT"
command = ["run.sh"]
services = ["metadata.read"]
activation = ["onService:metadata.read"]

[compat]
helios = ">=0.0.1"
interfaces = { protocol = 1, "metadata.read" = 1 }

[settings.depth]
type = "int"
default = 3

[[capabilities]]
name = "images.read"
reason = "reads the images it is asked about"
"#;

#[test]
fn manifests_are_checked_before_anything_runs() {
    assert_eq!(manifest_problems(GOOD), Vec::<String>::new());
    let refused = |from: &str, to: &str, expect: &str| {
        let problems = manifest_problems(&GOOD.replace(from, to));
        assert!(
            problems.iter().any(|p| p.contains(expect)),
            "{from} -> {to}: {problems:?}"
        );
    };
    refused(
        r#"helios = ">=0.0.1""#,
        r#"helios = ">=9.0.0""#,
        "works with Helios >=9.0.0",
    );
    refused(
        "protocol = 1",
        "protocol = 2",
        "interface `protocol` version 2",
    );
    refused(
        r#""images.read""#,
        r#""filesystem.all""#,
        "unknown capability `filesystem.all`",
    );
    refused(
        "reason = \"reads the images it is asked about\"",
        "reason = \"\"",
        "without a reason",
    );
    refused(r#""org.example.good""#, r#""good""#, "reverse domain");
    refused(
        r#"["run.sh"]"#,
        r#"["../escape.sh"]"#,
        "command `../escape.sh`",
    );
    refused("default = 3", "default = \"three\"", "setting `depth`");
    refused(
        r#"activation = ["onService:metadata.read"]"#,
        r#"activation = ["onService:device"]"#,
        "names nothing",
    );
    refused("license = \"MIT\"", "licence = \"MIT\"", "unknown field");
}

#[test]
fn discovery_keeps_refused_plugins_apart_and_lets_a_project_shadow_a_user() {
    let user = scratch("user");
    let project = scratch("project");
    for (root, name, text) in [
        (&user, "good", GOOD.to_string()),
        (&user, "bad", GOOD.replace("protocol = 1", "protocol = 7")),
        (&project, "good", GOOD.to_string()),
    ] {
        std::fs::create_dir_all(root.join(name)).unwrap();
        std::fs::write(root.join(name).join("helios-plugin.toml"), text).unwrap();
    }
    let mut b = Broker::new(Policy::default());
    b.discover(&[
        (Scope::User, user.clone()),
        (Scope::Project, project.clone()),
        (Scope::Builtin, scratch("missing").join("none")),
    ]);
    assert_eq!(b.ids().collect::<Vec<_>>(), ["org.example.good"]);
    let list = b.list();
    assert_eq!(list["plugins"][0]["scope"], "project");
    let rejected: Vec<String> = b.rejected().iter().map(|r| r.problems.join(" ")).collect();
    assert_eq!(rejected.len(), 2, "{rejected:?}");
    assert!(rejected.iter().any(|p| p.contains("protocol")));
    assert!(rejected.iter().any(|p| p.contains("shadowed")));
}

#[test]
fn a_call_needing_an_ungranted_capability_is_refused() {
    if !python() {
        return;
    }
    let b = testee(quick());
    let image = scratch("ungranted").join("a.png");
    std::fs::write(&image, b"not really").unwrap();
    let e = b.metadata_read(&image).unwrap_err();
    assert!(e.contains("`images.read`, not granted"), "{e}");
    assert!(!b.running(TESTEE), "refused before starting anything");
    b.grant(TESTEE, "images.read").unwrap();
    let answer = b.metadata_read(&image).unwrap();
    // The plugin was handed that one path, and told what it holds.
    assert_eq!(
        answer["fields"][0]["value"],
        json!(image.canonicalize().unwrap())
    );
    assert_eq!(answer["fields"][0]["plugin"], TESTEE);
    assert_eq!(answer["fields"][1]["value"], json!(["images.read"]));
    // Only capabilities it declares can be granted.
    assert!(
        b.grant(TESTEE, "network")
            .unwrap_err()
            .contains("does not declare")
    );
}

#[test]
fn an_ambient_capability_must_be_granted_before_the_plugin_runs() {
    if !python() {
        return;
    }
    let dir = testee_copy("ambient", |t| {
        t + "\n[[capabilities]]\nname = \"network\"\nreason = \"fetches lens profiles\"\n"
    });
    let mut b = Broker::new(quick());
    b.add(&dir, Scope::User).unwrap();
    b.grant(TESTEE, "images.read").unwrap();
    let e = b.call(TESTEE, READ, "echo", json!({})).unwrap_err();
    assert!(
        e.contains("`network` (fetches lens profiles), not granted"),
        "{e}"
    );
    assert!(!b.running(TESTEE));
    b.grant(TESTEE, "network").unwrap();
    b.call(TESTEE, READ, "echo", json!({})).unwrap();
}

#[test]
fn a_crash_costs_that_plugin_alone_and_it_restarts() {
    if !python() {
        return;
    }
    let other = testee_copy("other", |t| t.replace(TESTEE, "org.example.other"));
    let mut b = testee(quick());
    b.add(&other, Scope::User).unwrap();
    let notices = b.subscribe();
    let pid = |b: &Broker, id: &str| {
        b.call(id, READ, "echo", json!({})).unwrap()["pid"]
            .as_u64()
            .unwrap()
    };
    let first = pid(&b, TESTEE);
    let bystander = pid(&b, "org.example.other");

    let e = b.call(TESTEE, READ, "crash", json!({})).unwrap_err();
    assert!(e.contains("stopped during `crash`"), "{e}");
    assert!(
        e.contains("crashing on demand"),
        "the error carries its stderr: {e}"
    );
    let crashed = std::iter::from_fn(|| notices.recv_timeout(Duration::from_secs(10)).ok())
        .find(|n| matches!(n, Notice::Crashed { .. }))
        .expect("a crash notice");
    assert_eq!(
        crashed,
        Notice::Crashed {
            plugin: TESTEE.into(),
            status: "exit status: 3".into(),
            stderr: vec!["testee: crashing on demand".into()],
        }
    );

    let second = pid(&b, TESTEE);
    assert_ne!(first, second, "restarted");
    assert_eq!(pid(&b, "org.example.other"), bystander, "untouched");
}

#[test]
fn restarts_back_off_after_a_crash() {
    if !python() {
        return;
    }
    let b = testee(Policy {
        backoff: Duration::from_mins(1),
        ..quick()
    });
    b.call(TESTEE, READ, "echo", json!({})).unwrap();
    b.call(TESTEE, READ, "crash", json!({})).unwrap_err();
    let e = b.call(TESTEE, READ, "echo", json!({})).unwrap_err();
    assert!(
        e.contains("crashed 1 time(s) in a row; it restarts in"),
        "{e}"
    );
    assert_eq!(b.list()["plugins"][0]["crashes"], 1);
    // Enabling it again clears the wait.
    b.set_enabled(TESTEE, false).unwrap();
    b.set_enabled(TESTEE, true).unwrap();
    b.call(TESTEE, READ, "echo", json!({})).unwrap();
}

#[test]
fn a_call_times_out_and_the_plugin_lives_on() {
    if !python() {
        return;
    }
    let b = testee(Policy {
        timeout: Duration::from_millis(300),
        ..quick()
    });
    let e = b.call(TESTEE, READ, "hang", json!({})).unwrap_err();
    assert!(e.contains("no answer within 300 ms"), "{e}");
    let pid = b.pid(TESTEE).unwrap();
    let again = b.call(TESTEE, READ, "echo", json!({"n": 1})).unwrap();
    assert_eq!(again["params"]["n"], 1);
    assert_eq!(again["pid"], pid);
}

#[test]
fn idle_and_disabled_plugins_are_stopped() {
    if !python() {
        return;
    }
    let b = testee(Policy {
        idle: Duration::ZERO,
        ..quick()
    });
    let notices = b.subscribe();
    b.call(TESTEE, READ, "echo", json!({})).unwrap();
    assert!(b.running(TESTEE));
    b.stop_idle();
    assert!(!b.running(TESTEE));
    assert!(notices.try_iter().any(|n| n
        == Notice::Stopped {
            plugin: TESTEE.into(),
            why: "idle".into()
        }));

    b.set_enabled(TESTEE, false).unwrap();
    let e = b.call(TESTEE, READ, "echo", json!({})).unwrap_err();
    assert!(e.contains("disabled"), "{e}");
}

#[test]
fn the_broker_as_commands() {
    if !python() {
        return;
    }
    let b = Arc::new(testee(quick()));
    let mut registry = Registry::new();
    let skipped = helios_plugin::register(&mut registry, b.clone()).unwrap();
    assert!(skipped.is_empty());
    let cx = Context::new();
    let mut run = |id: &str, args: Json| registry.run(id, args, Origin::User, &cx);

    let list = run("plugin.list", json!({})).unwrap();
    let caps = &list["plugins"][0]["capabilities"][0];
    assert_eq!(caps["name"], "images.read");
    assert_eq!(caps["enforcement"], "brokered");
    assert_eq!(caps["granted"], false);

    let image = scratch("commands").join("b.png");
    std::fs::write(&image, b"x").unwrap();
    let image = image.to_string_lossy().to_string();
    assert!(run("metadata.read", json!({"image": image})).is_err());
    run(
        "plugin.grant",
        json!({"plugin": TESTEE, "capability": "images.read"}),
    )
    .unwrap();
    let read = run("metadata.read", json!({"image": image})).unwrap();
    assert_eq!(read["fields"][0]["source"], "testee");
    let e = run("device.list", json!({})).unwrap();
    assert_eq!(e["devices"], json!([]));
    run("plugin.disable", json!({"plugin": TESTEE})).unwrap();
    assert!(!b.running(TESTEE));
    assert_eq!(
        run("metadata.read", json!({"image": image})).unwrap_err(),
        "no plugin reads metadata"
    );
}

#[test]
fn a_probe_starts_a_plugin_hands_it_its_handshake_and_stops_it() {
    if !python() {
        return;
    }
    // A plugin that dies before it answers `initialize`.
    let dead = testee_copy("dead", |t| {
        t.replace(TESTEE, "org.example.dead").replace(
            r#"["python3", "testee.py"]"#,
            r#"["python3", "-c", "import sys; print('dying', file=sys.stderr); sys.exit(3)"]"#,
        )
    });
    let mut b = testee(quick());
    b.add(&dead, Scope::User).unwrap();
    let (probed, _) = b.probe(TESTEE).unwrap();
    assert_eq!(probed, Probed::Answers);
    assert!(!b.running(TESTEE), "a probe keeps nothing running");
    let (probed, log) = b.probe("org.example.dead").unwrap();
    assert!(
        matches!(probed, Probed::Failed(ref e) if e.contains("did not initialize")),
        "{probed:?}"
    );
    assert!(!log.is_empty(), "its error is its log");
    b.set_enabled(TESTEE, false).unwrap();
    assert_eq!(b.probe(TESTEE).unwrap().0, Probed::Disabled);
    assert!(b.probe("org.example.nothing").is_err());
}

#[test]
fn grants_go_to_their_keeper_and_come_back_on_the_next_start() {
    let kept: Arc<std::sync::Mutex<Option<helios_plugin::grants::Grants>>> = Arc::default();
    let b = testee(quick());
    let k = kept.clone();
    b.on_grants(move |g| *k.lock().unwrap() = Some(g.clone()));
    assert_eq!(b.missing(TESTEE).unwrap().len(), 1);
    b.grant(TESTEE, "images.read").unwrap();
    let grants = kept.lock().unwrap().clone().expect("the keeper was told");
    assert_eq!(grants[TESTEE], ["images.read".to_string()].into());
    // Through the person's settings layer, as the desktop keeps them.
    let layer = helios_plugin::grants::write("", &grants).unwrap();
    // The next start: a new broker, given them back.
    let again = testee(quick());
    again.load_grants(&helios_plugin::grants::read(&layer));
    assert!(again.missing(TESTEE).unwrap().is_empty());
    assert_eq!(
        again.list()["plugins"][0]["capabilities"][0]["granted"],
        true
    );
}

#[test]
fn a_capability_an_updated_manifest_adds_is_asked_for_again() {
    let mut grants = helios_plugin::grants::Grants::new();
    // Kept from the earlier version, with one it no longer declares.
    grants.insert(
        TESTEE.into(),
        ["images.read".to_string(), "gpu".to_string()].into(),
    );
    let dir = testee_copy("updated", |t| {
        t + "\n[[capabilities]]\nname = \"network\"\nreason = \"fetches lens profiles\"\n"
    });
    let mut b = Broker::new(quick());
    b.add(&dir, Scope::User).unwrap();
    b.load_grants(&grants);
    let missing: Vec<String> = b
        .missing(TESTEE)
        .unwrap()
        .into_iter()
        .map(|c| format!("{}: {}", c.name, c.reason))
        .collect();
    assert_eq!(missing, ["network: fetches lens profiles"]);
    assert_eq!(b.grants()[TESTEE], ["images.read".to_string()].into());
}

#[test]
fn revoking_a_capability_stops_the_plugin_and_is_kept() {
    if !python() {
        return;
    }
    let told = Arc::new(AtomicU64::new(0));
    let b = testee(quick());
    let t = told.clone();
    b.on_grants(move |_| {
        t.fetch_add(1, Ordering::Relaxed);
    });
    let mut registry = Registry::new();
    helios_plugin::register(&mut registry, Arc::new(b)).unwrap();
    let cx = Context::new();
    let mut run = |id: &str, args: Json| registry.run(id, args, Origin::User, &cx);
    let args = |on: bool| json!({"plugin": TESTEE, "capability": "images.read", "granted": on});
    run("plugin.grant", args(true)).unwrap();
    let rows = run("plugin.capabilities", Json::Null).unwrap();
    assert_eq!(rows["capabilities"][0]["granted"], true, "{rows}");
    assert_eq!(
        rows["capabilities"][0]["id"],
        format!("{TESTEE} images.read")
    );
    let list = run("plugin.list", Json::Null).unwrap();
    assert_eq!(list["plugins"][0]["running"], false);
    // A call starts it; taking the capability back stops it.
    let image = scratch("revoke").join("a.png");
    std::fs::write(&image, b"not really").unwrap();
    let image = image.to_string_lossy().to_string();
    run("metadata.read", json!({"image": image})).unwrap();
    assert_eq!(
        run("plugin.list", Json::Null).unwrap()["plugins"][0]["running"],
        true
    );
    run("plugin.grant", args(false)).unwrap();
    assert_eq!(
        run("plugin.list", Json::Null).unwrap()["plugins"][0]["running"],
        false
    );
    assert!(run("metadata.read", json!({"image": image})).is_err());
    assert_eq!(told.load(Ordering::Relaxed), 2, "granted, then revoked");
}

#[test]
fn own_settings_reach_the_plugin_at_start_and_with_each_call_and_a_change_notifies_or_restarts() {
    if !python() {
        return;
    }
    let dir = testee_copy("settings", |m| {
        m + r#"
[settings.quality]
type = "int"
default = 90
min = 1
max = 100

[settings.token]
type = "secret"

[settings.engine]
type = "choice"
choices = ["a", "b"]
default = "a"
restart = true
"#
    });
    let mut b = Broker::new(quick());
    b.add(&dir, Scope::User).unwrap();
    let notices = b.subscribe();
    // The person's values, as the desktop keeps them: the secret read
    // from the keyring.
    let given = Arc::new(std::sync::Mutex::new(serde_json::Map::new()));
    let set = |k: &str, v: Json| {
        given.lock().unwrap().insert(k.into(), v);
    };
    set("quality", json!(50));
    set("token", json!("hunter2-secret"));
    {
        let given = given.clone();
        b.on_values(move |id| {
            assert_eq!(id, TESTEE);
            given.lock().unwrap().clone()
        });
    }
    let plugin = |answer: &Json| answer["params"]["config"]["plugin"].clone();
    let echo = b.call(TESTEE, READ, "echo", json!({"n": 1})).unwrap();
    assert_eq!(echo["params"]["n"], 1, "the call's own params are kept");
    assert_eq!(
        plugin(&echo),
        json!({"quality": 50, "token": "hunter2-secret", "engine": "a"}),
        "with each call"
    );
    let told = b.call(TESTEE, READ, "told", json!({})).unwrap();
    let pid = told["pid"].clone();
    assert_eq!(
        told["told"][0]["settings"]["org.example.testee.quality"],
        50
    );
    assert_eq!(
        told["told"][0]["config"]["plugin"]["token"], "hunter2-secret",
        "at start"
    );

    // A change of a key without `restart`: the running plugin is told.
    set("quality", json!(70));
    assert_eq!(b.settings_changed(TESTEE).unwrap(), Told::Notified);
    let told = b.call(TESTEE, READ, "told", json!({})).unwrap();
    assert_eq!(told["pid"], pid, "not restarted");
    assert_eq!(told["told"][1]["config"]["plugin"]["quality"], 70);
    assert_eq!(b.settings_changed(TESTEE).unwrap(), Told::Nothing);

    // A value its contract refuses is left out: the default stands.
    set("quality", json!(500));
    assert_eq!(b.values(TESTEE).unwrap()["quality"], 90);

    // A key that says `restart`: stopped, started again with it.
    set("quality", json!(70));
    set("engine", json!("b"));
    assert_eq!(b.settings_changed(TESTEE).unwrap(), Told::Restarted);
    assert!(!b.running(TESTEE));
    let told = b.call(TESTEE, READ, "told", json!({})).unwrap();
    assert_ne!(told["pid"], pid, "restarted");
    assert_eq!(told["told"][0]["config"]["plugin"]["engine"], "b");

    // The secret went to the plugin and nowhere else the broker shows.
    b.call(TESTEE, READ, "crash", json!({})).unwrap_err();
    let shown: Vec<String> = notices.try_iter().map(|n| format!("{n:?}")).collect();
    assert!(!shown.is_empty());
    for text in shown.iter().chain([&b.list().to_string()]) {
        assert!(!text.contains("hunter2"), "the secret shows in {text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_source_plugin_gets_its_account_and_source_values_with_each_call() {
    use helios_connect::{AccountRef, Cancel, Connector as _};
    use helios_core::config::Level;
    use helios_plugin::source::SourceProvider;

    if !python() {
        return;
    }
    let dir = testee_copy("source-values", |m| {
        m.replace(
            r#"services = ["metadata.read"]
activation = ["onService:metadata.read"]"#,
            r#"services = ["metadata.read", "import.source"]
activation = ["onService:metadata.read", "onService:import.source"]"#,
        )
        .replace(
            r#""metadata.read" = 1 }"#,
            r#""metadata.read" = 1, "import.source" = 1 }"#,
        ) + r#"
[[connect]]
id = "who"
kind = "form"
schema = { fields = [{ id = "user" }] }

[account.settings.region]
type = "choice"
choices = ["eu", "us"]
default = "eu"

[account.settings.key]
type = "secret"

[source.settings.album]
type = "string"
default = "all"

[source.settings.limit]
type = "int"
min = 1
max = 10
"#
    });
    let mut b = Broker::new(quick());
    b.add(&dir, Scope::User).unwrap();
    let b = Arc::new(b);
    let provider = SourceProvider::new(b.clone(), TESTEE).expect("an import.source plugin");
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let values: helios_connect::ValuesFn = {
        let asked = asked.clone();
        Arc::new(move |level, id: &str, contract| {
            asked.lock().unwrap().push((level, id.to_owned()));
            let keys: Vec<&String> = contract.keys.keys().collect();
            match level {
                Level::Account => {
                    assert_eq!(keys, ["key", "region"]);
                    json!({"region": "us", "key": "hunter2-secret"})
                }
                // `limit` is out of its bounds: left out.
                _ => json!({"album": "Holidays", "limit": 99}),
            }
            .as_object()
            .unwrap()
            .clone()
        })
    };
    let connector = provider.connector(Arc::new(|_: &str| Ok("token".into())), Some(values));
    let account = AccountRef {
        connector: "testee".into(),
        id: "testee-1".into(),
    };
    connector.probe(Some(&account), &Cancel::new()).unwrap();
    let last = b
        .call(TESTEE, "onService:import.source", "told", json!({}))
        .unwrap();
    let probe = &last["last"]["source.probe"];
    assert_eq!(probe["auth"], "token");
    assert_eq!(
        probe["config"],
        json!({
            "plugin": {},
            "account": {"region": "us", "key": "hunter2-secret"},
            "source": {"album": "Holidays"},
        })
    );
    assert_eq!(
        *asked.lock().unwrap(),
        [
            (Level::Account, "testee-1".to_string()),
            (Level::Source, "testee-1".to_string())
        ]
    );
    assert!(!b.list().to_string().contains("hunter2"));
    let _ = std::fs::remove_dir_all(&dir);
}
