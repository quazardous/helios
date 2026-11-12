//! `helios-node --vault`: one node owns the vault, runs its jobs (the
//! volume watch, the backups, the health check) and closes the vault
//! cleanly when it stops, in process or on SIGTERM.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_health::runner::Schedule;
use helios_node::vault_node::{Options, VaultNode};
use helios_storage::{Volume, Volumes};
use helios_vault::Vault;
use helios_vault_sqlite::SqliteStore;

struct Place(PathBuf);

impl Place {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("helios-vault-node-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Waits until `done`, at most `deadline`; whether it came.
fn until(deadline: Duration, done: impl Fn() -> bool) -> bool {
    let end = Instant::now() + deadline;
    while !done() {
        if Instant::now() > end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

#[test]
fn the_node_runs_its_jobs_and_closes_the_vault_cleanly_when_stopped() {
    let p = Place::new("jobs");
    let file = p.0.join("vault.db");
    let vault = Vault::new(SqliteStore::open(&file).unwrap());
    vault.set_config_layer("user", Some("a = 1")).unwrap();
    let bus = Arc::new(Bus::new());
    let jobs = Jobs::new(bus.clone());
    let volumes: Arc<dyn Volumes> = Arc::new(Vec::<Volume>::new());
    let node = VaultNode::start(
        vault,
        &file,
        bus,
        &jobs,
        Options {
            volumes,
            // A health run at once, then none for an hour.
            schedule: Schedule::new(Instant::now(), Duration::ZERO, Duration::from_hours(1)),
            clock: Duration::from_millis(20),
        },
    );
    let link = node.link();

    assert!(
        until(Duration::from_secs(10), || node.watch().listed()),
        "the volume watch took its first look in 10 s"
    );
    assert!(
        until(Duration::from_secs(30), || node.health().runs() >= 1
            && !node.health().running()),
        "a scheduled health run ended in 30 s"
    );
    assert!(node.health().findings().ran(), "its findings are in");
    assert!(!node.backups().cancelling(), "the backups are on");
    // The vault is the node's: what a user of it writes goes through it.
    node.vault()
        .set_config_layer("user", Some("a = 2"))
        .unwrap();

    let stopping = std::thread::spawn(move || node.stop());
    assert!(
        link.wait_stopped(Duration::from_secs(30)),
        "the node stopped in 30 s"
    );
    stopping.join().unwrap();

    let again = SqliteStore::open(&file).unwrap();
    assert!(!again.report().unclean_shutdown, "the vault closed cleanly");
    assert_eq!(
        Vault::new(again).config_layer("user").unwrap().as_deref(),
        Some("a = 2")
    );
}

/// The binary: SIGTERM stops the node, which closes the vault cleanly
/// and exits with 0.
#[cfg(unix)]
#[test]
fn sigterm_stops_the_node_with_the_vault_closed_cleanly() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let p = Place::new("sigterm");
    let network = p.0.join("network.json");
    std::fs::write(
        &network,
        r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "noise", "width": 16, "height": 16, "channels": 4, "seed": 1 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}"#,
    )
    .unwrap();
    let file = p.0.join("vault.db");
    let socket = p.0.join("node.sock");
    let mut node = Command::new(env!("CARGO_BIN_EXE_helios-node"))
        .arg("--graph")
        .arg(&network)
        .arg("--socket")
        .arg(&socket)
        .arg("--vault")
        .arg(&file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(node.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.starts_with("ready "), "{ready}");

    let killed = Command::new("kill")
        .args(["-TERM", &node.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let end = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = node.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > end {
            let _ = node.kill();
            panic!("the node did not stop in 30 s after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut node.stderr.take().unwrap(), &mut stderr).unwrap();
    assert!(status.success(), "{status}: {stderr}");
    assert!(stderr.contains("the vault is closed"), "{stderr}");

    let again = SqliteStore::open(&file).unwrap();
    assert!(!again.report().unclean_shutdown, "the vault closed cleanly");
}
