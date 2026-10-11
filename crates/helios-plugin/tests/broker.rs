//! The broker against a test plugin (`tests/fixtures/testee`, Python with
//! its standard library alone): manifests, crashes, timeouts, capabilities.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use helios_plugin::{Broker, Notice, Policy, Scope};
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
