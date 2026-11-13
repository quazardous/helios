//! The problem checks a plugin declares, run through its process (the
//! test plugin, `tests/fixtures/testee`): registered under the plugin,
//! their findings answered by it, a crash, a timeout or a garbled answer
//! turned into that check's failure, an unsound declaration refusing the
//! plugin.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use helios_health::{Check, Context, FAILED, Level, Registry, run_one};
use helios_plugin::health;
use helios_plugin::{Broker, Policy, Scope};

const TESTEE: &str = "org.example.testee";

/// The checks the test plugin declares: `fine` answers, the others fail
/// as their names say.
const CHECKS: &str = r#"
[[health]]
id = "fine"
title = "Testee disk"
watches = "The testee's disk and fan"
levels = ["ok", "warning"]
fixes = ["testee.repair"]

[[health]]
id = "crash"
title = "Crashes"
watches = "Nothing: it crashes"
levels = ["ok"]

[[health]]
id = "hang"
title = "Hangs"
watches = "Nothing: it never answers"
levels = ["ok"]

[[health]]
id = "garbled"
title = "Garbled"
watches = "Nothing: it answers what is not findings"
levels = ["ok"]

[[health]]
id = "rogue"
title = "Rogue"
watches = "Nothing: it answers a level it does not declare"
levels = ["ok"]
"#;

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-plugin-health-{name}-{}-{}",
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

fn python() -> bool {
    let ok = Command::new("python3").arg("--version").output().is_ok();
    if !ok {
        eprintln!("python3 is not installed: skipped");
    }
    ok
}

/// The test plugin's manifest, declaring `checks` (started on
/// `onHealth`, the `health` interface given).
fn manifest(checks: &str) -> String {
    let text = std::fs::read_to_string(fixture().join("helios-plugin.toml")).unwrap();
    let text = text
        .replace(
            "activation = [\"onService:metadata.read\"]",
            "activation = [\"onService:metadata.read\", \"onHealth\"]",
        )
        .replace(
            "\"metadata.read\" = 1 }",
            "\"metadata.read\" = 1, health = 1 }",
        );
    format!("{text}{checks}")
}

/// A copy of the test plugin with `manifest`, added to a new broker:
/// the broker, or the manifest's problems.
fn broker(name: &str, manifest: &str) -> Result<Arc<Broker>, Vec<String>> {
    let dir = scratch(name);
    std::fs::copy(fixture().join("testee.py"), dir.join("testee.py")).unwrap();
    std::fs::write(dir.join("helios-plugin.toml"), manifest).unwrap();
    let mut b = Broker::new(Policy {
        timeout: Duration::from_secs(10),
        backoff: Duration::ZERO,
        ..Policy::default()
    });
    b.add(&dir, Scope::User)?;
    Ok(Arc::new(b))
}

/// The plugin's check `id`, waiting `timeout` at most.
fn check(broker: &Arc<Broker>, id: &str, timeout: Duration) -> Arc<dyn Check> {
    let (_, c) = health::checks(broker, None)
        .into_iter()
        .find(|(_, c)| c.id() == format!("{TESTEE}.{id}"))
        .expect("the check is declared");
    Arc::new(c.timeout(timeout))
}

#[test]
fn a_plugin_declares_its_checks_and_they_run_through_its_process() {
    if !python() {
        return;
    }
    let broker = broker("declares", &manifest(CHECKS)).unwrap();
    let mut registry = Registry::default();
    assert_eq!(
        health::register(&mut registry, &broker, None),
        Vec::<String>::new()
    );
    let owners: Vec<(&str, Vec<&str>)> = registry
        .by_owner()
        .into_iter()
        .map(|(o, checks)| (o, checks.iter().map(|c| c.id()).collect()))
        .collect();
    assert_eq!(
        owners,
        [(
            TESTEE,
            vec![
                "org.example.testee.fine",
                "org.example.testee.crash",
                "org.example.testee.hang",
                "org.example.testee.garbled",
                "org.example.testee.rogue",
            ]
        )]
    );

    let found = run_one(
        &check(&broker, "fine", Duration::from_secs(10)),
        &Context::default(),
    );
    let got: Vec<(&str, Level, String)> = found
        .iter()
        .map(|f| (f.id.as_str(), f.level, f.message.to_string()))
        .collect();
    assert_eq!(
        got,
        [
            (
                "org.example.testee.fine/disk",
                Level::Warning,
                "The testee's disk is full".to_owned()
            ),
            (
                "org.example.testee.fine",
                Level::Ok,
                "The fan turns".to_owned()
            ),
        ]
    );
    // No `[[connect]]`: no accounts handed.
    assert_eq!(found[0].detail.to_string(), "accounts: 0");
    let fix = found[0].fix.as_ref().unwrap();
    assert_eq!(fix.command, "testee.repair");
    assert_eq!(fix.args["disk"], 1);
}

#[test]
fn a_crash_a_timeout_or_a_bad_answer_is_that_checks_failure() {
    if !python() {
        return;
    }
    let broker = broker("fails", &manifest(CHECKS)).unwrap();
    for id in ["crash", "hang", "garbled", "rogue"] {
        let started = Instant::now();
        let found = run_one(
            &check(&broker, id, Duration::from_secs(2)),
            &Context::default(),
        );
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "{id}: held to its time limit"
        );
        let got: Vec<(&str, Level, &str)> = found
            .iter()
            .map(|f| (f.id.as_str(), f.level, f.message.key.as_str()))
            .collect();
        let want = format!("{TESTEE}.{id}");
        assert_eq!(got, [(want.as_str(), Level::Problem, FAILED)], "{id}");
    }
    // The plugin answers again after its crash and its timeout.
    let found = run_one(
        &check(&broker, "fine", Duration::from_secs(10)),
        &Context::default(),
    );
    assert_eq!(found.len(), 2, "{found:?}");
}

#[test]
fn a_disabled_plugins_checks_say_nothing() {
    if !python() {
        return;
    }
    let broker = broker("disabled", &manifest(CHECKS)).unwrap();
    broker.set_enabled(TESTEE, false).unwrap();
    let found = run_one(
        &check(&broker, "fine", Duration::from_secs(10)),
        &Context::default(),
    );
    assert!(found.is_empty(), "{found:?}");
    assert!(!broker.running(TESTEE));
}

#[test]
fn an_unsound_declaration_refuses_the_plugin_with_its_reasons() {
    let refused = |text: &str| broker("refused", text).err().unwrap_or_default();
    let bad = r#"
[[health]]
id = "Disk Space"
title = " "
watches = ""
levels = []
fixes = ["open folder"]

[[health]]
id = "twice"
title = "Twice"
watches = "Twice"
levels = ["ok", "ok"]

[[health]]
id = "twice"
title = "Twice"
watches = "Twice"
levels = ["ok"]
"#;
    assert_eq!(
        refused(&manifest(bad)),
        [
            "health check `Disk Space`: a check id of lowercase letters, digits, `.`, `-` and `_`",
            "health check `Disk Space`: `watches` says what it watches",
            "health check `Disk Space`: `levels` names at least one level",
            "health check `Disk Space`: fix `open folder`: a command id such as `library.storage`",
            "health check `Disk Space`: a title",
            "health check `twice`: `levels` names Ok twice",
            "health check `twice` is declared twice",
        ]
    );
    let plain = std::fs::read_to_string(fixture().join("helios-plugin.toml")).unwrap();
    assert_eq!(
        refused(&format!("{plain}{CHECKS}")),
        [
            "`[[health]]` without the activation event `onHealth`",
            "`[[health]]` without the `health` interface version in `compat.interfaces`",
        ]
    );
    let unchecked = manifest("");
    assert_eq!(
        refused(&unchecked),
        ["activation event `onHealth` names nothing this plugin declares"]
    );
}
