//! A plugin's capabilities asked for at the moment of need: "Add a
//! source…" on a source plugin that lacks some shows the consent dialog
//! (each capability with its manifest's reason); Allow grants them and
//! starts the connection, Don't allow and Escape start nothing; each
//! answer closes it. Settings > Plugins' commands list the capabilities and take one back.
//! Allow on a running plugin whose stop takes its whole grace never holds
//! a frame: the grant runs off the interface's thread.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use helios_connect::account::{MemoryAccounts, MemorySecrets};
use helios_gui::{Shell, headless, helios_panels};
use helios_plugin::{Broker, Policy, Scope};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

const PLUGIN: &str = "org.example.fake-source";
const REASON: &str = "Talks to the fake service";

fn until(h: &mut Harness<'_, Shell>, what: &str, done: impl Fn(&mut Harness<'_, Shell>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        h.step();
        if done(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("never got there: {what}\n{:#?}", h.root());
}

fn shown(h: &Harness<'_, Shell>, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

/// Whether the consent dialog is in the tree of the window it was shown
/// in (kittest embeds the import window in the main one, so the root
/// tree holds both).
fn consent_shown(h: &Harness<'_, Shell>) -> bool {
    shown(h, "Allow Fake source?")
        || shown(h, "Don't allow")
        || shown(h, REASON)
        || h.query_all_by_label_contains("allow them later in Settings > Plugins")
            .next()
            .is_some()
}

/// Steps frames and checks the consent dialog stays gone at each.
fn stays_closed(h: &mut Harness<'_, Shell>, what: &str) {
    for _ in 0..20 {
        h.step();
        assert!(
            !consent_shown(h),
            "{what}: the consent dialog is still shown\n{:#?}",
            h.root()
        );
    }
}

/// Clicks `label` once the dialog holding it has settled: a dialog whose
/// content changed (Don't allow's word replacing the question) is
/// centred again on its new size a frame later, and a click at the
/// button's place of the frame before misses it.
fn click_settled(h: &mut Harness<'_, Shell>, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut was = h.get_by_label(label).rect();
    loop {
        h.step();
        let now = h.get_by_label(label).rect();
        if now == was {
            break;
        }
        assert!(Instant::now() < deadline, "{label} never settled");
        was = now;
    }
    h.get_by_label(label).click();
}

fn run(h: &mut Harness<'_, Shell>, id: &str, args: Value) -> Result<Value, String> {
    let r = h.state_mut().run(id, args, Origin::User);
    h.run_steps(2);
    r
}

/// A source plugin that never needs to run: its connection's first step
/// is a form.
fn plugin_dir() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-consent-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("helios-plugin.toml"),
        format!(
            r#"
id = "{PLUGIN}"
name = "Fake source"
version = "1.0.0"
license = "GPL-3.0-or-later"
command = ["false"]
services = ["import.source"]
activation = ["onService:import.source"]

[compat]
helios = ">=0.0.1"
interfaces = {{ protocol = 1, "import.source" = 1 }}

[[connect]]
id = "who"
kind = "form"
schema = {{ fields = [{{ id = "user" }}] }}

[[capabilities]]
name = "network"
reason = "{REASON}"
"#
        ),
    )
    .unwrap();
    dir
}

fn world() -> (Arc<Broker>, Harness<'static, Shell>) {
    let mut broker = Broker::new(Policy::default());
    broker.add(&plugin_dir(), Scope::User).unwrap();
    let broker = Arc::new(broker);
    let mut shell = Shell::new(helios_panels(), None);
    for p in helios_plugin::source::providers(&broker) {
        shell.add_import_connector(Arc::new(p));
    }
    shell.set_plugins(broker.clone());
    shell.set_import_accounts(
        Arc::new(MemoryAccounts::default()),
        Arc::new(MemorySecrets::default()),
    );
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    (broker, h)
}

fn granted(broker: &Broker) -> bool {
    broker.list()["plugins"][0]["capabilities"][0]["granted"] == true
}

fn connection(h: &mut Harness<'_, Shell>) -> Value {
    h.state_mut()
        .run("import.connection", Value::Null, Origin::User)
        .unwrap()
}

#[test]
fn an_ungranted_capability_is_asked_for_and_allow_starts_the_connection() {
    let (broker, mut h) = world();
    run(
        &mut h,
        "import.addSource",
        json!({ "connector": "fake-source" }),
    )
    .unwrap();
    until(&mut h, "the consent dialog", |h| {
        shown(h, "Allow Fake source?") && shown(h, REASON)
    });
    assert!(connection(&mut h).is_null(), "held until the person allows");
    click_settled(&mut h, "Allow");
    until(&mut h, "the connection's form", |h| {
        connection(h)["state"] == "form"
    });
    assert!(granted(&broker));
    // The dialog is gone, and the action ran again exactly once: the
    // connection's account stays the one it started with.
    let account = connection(&mut h)["account"].clone();
    assert!(!account.is_null());
    stays_closed(&mut h, "after Allow");
    assert_eq!(
        connection(&mut h)["account"],
        account,
        "the action ran again"
    );
}

#[test]
fn dont_allow_starts_nothing_and_says_where_to_allow_later() {
    let (broker, mut h) = world();
    run(
        &mut h,
        "import.addSource",
        json!({ "connector": "fake-source" }),
    )
    .unwrap();
    until(&mut h, "the consent dialog", |h| shown(h, "Don't allow"));
    click_settled(&mut h, "Don't allow");
    until(&mut h, "the calm word", |h| {
        h.query_all_by_label_contains("allow them later in Settings > Plugins")
            .next()
            .is_some()
    });
    assert!(connection(&mut h).is_null());
    assert!(!granted(&broker));
    // Asked again later, it asks afresh.
    click_settled(&mut h, "Close");
    until(&mut h, "the dialog closed", |h| !consent_shown(h));
    stays_closed(&mut h, "after Close");
    assert!(connection(&mut h).is_null());
    run(
        &mut h,
        "import.addSource",
        json!({ "connector": "fake-source" }),
    )
    .unwrap();
    until(&mut h, "asked again", |h| shown(h, "Don't allow"));
    assert!(connection(&mut h).is_null());
}

#[test]
fn escape_closes_it_and_runs_nothing() {
    let (broker, mut h) = world();
    run(
        &mut h,
        "import.addSource",
        json!({ "connector": "fake-source" }),
    )
    .unwrap();
    until(&mut h, "the consent dialog", |h| shown(h, "Don't allow"));
    h.key_press(egui::Key::Escape);
    until(&mut h, "the dialog closed", |h| !consent_shown(h));
    stays_closed(&mut h, "after Escape");
    assert!(connection(&mut h).is_null());
    assert!(!granted(&broker));
}

#[test]
fn settings_plugins_lists_the_capabilities_and_takes_one_back() {
    let (broker, mut h) = world();
    let rows = run(&mut h, "plugin.capabilities", Value::Null).unwrap();
    let row = &rows["capabilities"][0];
    assert_eq!(row["plugin"], PLUGIN, "{rows}");
    assert_eq!(row["capability"], "network");
    assert_eq!(row["reason"], REASON);
    assert_eq!(row["granted"], false);
    let args = |on: bool| json!({ "plugin": PLUGIN, "capability": "network", "granted": on });
    run(&mut h, "plugin.grant", args(true)).unwrap();
    assert!(granted(&broker));
    // Granted, the connection starts at once: nothing asked.
    run(
        &mut h,
        "import.addSource",
        json!({ "connector": "fake-source" }),
    )
    .unwrap();
    until(&mut h, "the connection's form", |h| {
        connection(h)["state"] == "form"
    });
    run(&mut h, "plugin.grant", args(false)).unwrap();
    assert!(!granted(&broker));
}

/// A source plugin that runs, answers its handshake, and neither answers
/// `shutdown` nor leaves when told: its stop takes the broker's whole
/// grace, twice over.
fn slow_plugin_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-gui-consent-slow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("slow.py"),
        r#"import json, sys, time
for line in sys.stdin:
    message = json.loads(line)
    if message.get("method") == "initialize":
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {}}) + "\n")
        sys.stdout.flush()
    elif "id" in message and message.get("method") != "shutdown":
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": None}) + "\n")
        sys.stdout.flush()
time.sleep(30)
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("helios-plugin.toml"),
        format!(
            r#"
id = "org.example.slow-source"
name = "Slow source"
version = "1.0.0"
license = "GPL-3.0-or-later"
command = ["python3", "slow.py"]
services = ["import.source"]
activation = ["onService:import.source"]

[compat]
helios = ">=0.0.1"
interfaces = {{ protocol = 1, "import.source" = 1 }}

[[connect]]
id = "who"
kind = "form"
schema = {{ fields = [{{ id = "user" }}] }}

[[capabilities]]
name = "network"
reason = "Talks to the slow service"

[[capabilities]]
name = "images.read"
reason = "{REASON}"
"#
        ),
    )
    .unwrap();
    dir
}

#[test]
fn allow_on_a_plugin_slow_to_stop_holds_no_frame() {
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("python3 is not installed: skipped");
        return;
    }
    const SLOW: &str = "org.example.slow-source";
    let mut broker = Broker::new(Policy {
        grace: Duration::from_secs(2),
        ..Policy::default()
    });
    broker.add(&slow_plugin_dir(), Scope::User).unwrap();
    // Allowed to run, and running: Allow's grant stops it.
    broker.grant(SLOW, "network").unwrap();
    broker
        .call(SLOW, "onService:import.source", "ping", json!({}))
        .unwrap();
    assert!(broker.running(SLOW));
    let broker = Arc::new(broker);
    let mut shell = Shell::new(helios_panels(), None);
    for p in helios_plugin::source::providers(&broker) {
        shell.add_import_connector(Arc::new(p));
    }
    shell.set_plugins(broker.clone());
    shell.set_import_accounts(
        Arc::new(MemoryAccounts::default()),
        Arc::new(MemorySecrets::default()),
    );
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    run(
        &mut h,
        "import.addSource",
        json!({ "connector": "slow-source" }),
    )
    .unwrap();
    until(&mut h, "the consent dialog", |h| {
        shown(h, "Allow Slow source?") && shown(h, REASON)
    });
    click_settled(&mut h, "Allow");
    let allowed = Instant::now();
    // Each frame in time while the plugin stops; the connection starts
    // once the grant is done.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let at = Instant::now();
        h.step();
        let took = at.elapsed();
        assert!(took < Duration::from_millis(500), "a frame took {took:?}");
        if connection(&mut h)["state"] == "form" {
            break;
        }
        assert!(Instant::now() < deadline, "the connection never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    // The stop did take its grace: the frames above went on meanwhile.
    assert!(
        allowed.elapsed() >= Duration::from_secs(2),
        "the plugin stopped at once ({:?}): the test proves nothing",
        allowed.elapsed()
    );
    assert!(!broker.running(SLOW), "stopped by the grant");
    assert_eq!(
        broker.list()["plugins"][0]["capabilities"][1]["granted"],
        true
    );
}
