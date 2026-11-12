//! `helios-node --vault`: one node owns the vault, runs its jobs (the
//! volume watch, the backups, the health check) and closes the vault
//! cleanly when it stops, in process or on SIGTERM; an import through
//! the node starts the strategy's after-import backup; its settings
//! layers are read and written through its control endpoint.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_health::runner::Schedule;
use helios_library::import::{self, Placement};
use helios_node::vault_node::{Options, VaultNode};
use helios_storage::strategy::{self, Scope, Strategy, Trigger};
use helios_storage::{Identity, Role, Volume, VolumeId, VolumeKind, Volumes, detect, registry};
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

fn disk(id: &str, mount: &Path, roles: &[Role]) -> Volume {
    std::fs::create_dir_all(mount).unwrap();
    Volume {
        id: VolumeId(id.into()),
        identified_by: Identity::Uuid,
        name: id.into(),
        kind: VolumeKind::External,
        capacity: Some(1_000_000_000),
        free: Some(400_000_000),
        mount: Some(mount.to_path_buf()),
        roles: roles.iter().copied().collect(),
        ..detect::weak(id, None)
    }
}

const BACKUP_DISK: &str = "uuid:backup";

/// Photos with a copy on the backup disk.
fn backed_up(vault: &Vault<SqliteStore>) -> usize {
    vault
        .images()
        .unwrap()
        .iter()
        .filter(|i| {
            vault
                .locations(&i.id)
                .unwrap()
                .iter()
                .any(|l| l.volume.as_ref().is_some_and(|v| v.as_str() == BACKUP_DISK))
        })
        .count()
}

/// The headless node end to end, with no window: a library disk and a
/// backup disk of the strategy plugged in, an import through the node,
/// the after-import backup's copies recorded, a scheduled health run,
/// then a stop that leaves the vault closed cleanly.
#[test]
fn an_import_through_the_node_is_backed_up_then_the_health_check_runs_and_the_node_stops_cleanly() {
    const PHOTOS: u8 = 3;
    let p = Place::new("import");
    let file = p.0.join("vault.db");
    let vault = Vault::new(SqliteStore::open(&file).unwrap());
    let lib = disk("uuid:lib", &p.0.join("libdisk"), &[Role::Library]);
    let backup = disk(BACKUP_DISK, &p.0.join("backup"), &[Role::Backup]);
    registry::record(&vault, &[lib.clone(), backup.clone()], "studio", 10, None).unwrap();
    for v in [&lib, &backup] {
        registry::save(&vault, v).unwrap();
    }
    // Backed up after each import only: plugging a disk in or a person
    // asking would not start this one.
    let after_import = Strategy {
        triggers: [Trigger::OnImport].into(),
        ..Strategy::default()
    };
    strategy::save(&vault, &Scope::Library, Some(&after_import)).unwrap();
    let roll = p.0.join("roll");
    std::fs::create_dir_all(&roll).unwrap();
    for k in 0..PHOTOS {
        let bytes: Vec<u8> = (0..20_000u32)
            .map(|i| (i as u8).wrapping_mul(k + 1))
            .collect();
        std::fs::write(roll.join(format!("p{k}.raf")), bytes).unwrap();
    }

    let bus = Arc::new(Bus::new());
    let jobs = Jobs::new(bus.clone());
    // Both disks there from the start: no "plugged in" backup.
    let volumes: Arc<dyn Volumes> = Arc::new(vec![lib, backup]);
    let node = VaultNode::start(
        vault,
        &file,
        bus.clone(),
        &jobs,
        Options {
            volumes,
            // A health run soon after start, then none for an hour.
            schedule: Schedule::new(
                Instant::now(),
                Duration::from_millis(500),
                Duration::from_hours(1),
            ),
            clock: Duration::from_millis(20),
        },
    );
    let link = node.link();
    assert!(
        until(Duration::from_secs(10), || node.watch().listed()),
        "the volume watch took its first look in 10 s"
    );
    assert_eq!(
        backed_up(node.vault()),
        0,
        "nothing backed up before the import"
    );

    let options = import::Options {
        placement: Placement::Copy {
            into: Some(p.0.join("libdisk/Library")),
        },
        signals: false,
        ..import::Options::default()
    };
    let report = node
        .importer()
        .import(&[roll], &options, &mut |_| {})
        .unwrap();
    assert_eq!(report.added, u64::from(PHOTOS));
    assert!(
        bus.since(0)
            .iter()
            .any(|e| e.kind == "library.imported" && e.payload["added"] == u64::from(PHOTOS)),
        "the import is announced on the node's bus"
    );

    assert!(
        until(Duration::from_mins(1), || backed_up(node.vault())
            == usize::from(PHOTOS)),
        "the after-import backup copied every photo in a minute"
    );
    assert!(
        until(Duration::from_secs(30), || link.running_jobs().is_empty()),
        "the backup job ended in 30 s"
    );
    assert!(
        until(Duration::from_secs(30), || node.health().runs() >= 1
            && !node.health().running()),
        "a scheduled health run ended in 30 s"
    );
    assert!(node.health().findings().ran(), "its findings are in");

    let stopping = std::thread::spawn(move || node.stop());
    assert!(
        link.wait_stopped(Duration::from_secs(30)),
        "the node stopped in 30 s"
    );
    stopping.join().unwrap();

    let again = SqliteStore::open(&file).unwrap();
    assert!(!again.report().unclean_shutdown, "the vault closed cleanly");
    assert_eq!(
        backed_up(&Vault::new(again)),
        usize::from(PHOTOS),
        "the copies are recorded in the vault"
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

/// The binary: `helios control import` on the batch endpoint imports
/// into the vault the node owns and answers with the report; a node with
/// no vault refuses.
#[cfg(unix)]
#[test]
fn the_control_endpoint_imports_into_the_nodes_vault() {
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};

    let p = Place::new("control-import");
    let network = p.0.join("network.json");
    std::fs::write(
        &network,
        r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}"#,
    )
    .unwrap();
    let roll = p.0.join("roll");
    std::fs::create_dir_all(&roll).unwrap();
    std::fs::write(roll.join("p0.raf"), vec![7u8; 20_000]).unwrap();

    let start = |name: &str, vault: Option<&Path>| -> Child {
        let mut command = Command::new(env!("CARGO_BIN_EXE_helios-node"));
        command
            .arg("--graph")
            .arg(&network)
            .arg("--socket")
            .arg(p.0.join(format!("{name}.sock")))
            .arg("--batch-control")
            .arg(p.0.join(format!("{name}-batch.sock")))
            .env_remove("HELIOS_DEBUG_CONTROL")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(vault) = vault {
            command.arg("--vault").arg(vault);
        }
        let mut child = command.spawn().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert!(ready.starts_with("ready "), "{ready}");
        child
    };
    let import = |name: &str| -> serde_json::Value {
        let out = Command::new(env!("CARGO_BIN_EXE_helios"))
            .arg("control")
            .arg("--socket")
            .arg(p.0.join(format!("{name}-batch.sock")))
            .arg("import")
            .arg(&roll)
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null)
    };
    let stop = |mut child: Child| {
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
        let end = Instant::now() + Duration::from_secs(30);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > end {
                let _ = child.kill();
                panic!("the node did not stop in 30 s after SIGTERM");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    };

    let file = p.0.join("vault.db");
    let node = start("owner", Some(&file));
    let answer = import("owner");
    stop(node);
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["report"]["added"], 1, "{answer}");
    let again = SqliteStore::open(&file).unwrap();
    assert!(!again.report().unclean_shutdown, "the vault closed cleanly");
    assert_eq!(Vault::new(again).images().unwrap().len(), 1);

    let node = start("bare", None);
    let answer = import("bare");
    stop(node);
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(answer["message"]["key"], "node-no-vault", "{answer}");
}

/// The binary: the settings layers the node keeps are read and written
/// through its control endpoint (`helios control settings-get` and
/// `settings-set`, on the batch endpoint), each change heard by the
/// node's other clients (`settings.layerChanged` on its inspect
/// endpoint); a layer the node does not keep, or a node with no vault,
/// refuses.
#[cfg(unix)]
#[test]
fn the_control_endpoint_reads_and_writes_the_nodes_settings_layers() {
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};

    let p = Place::new("control-settings");
    let network = p.0.join("network.json");
    std::fs::write(
        &network,
        r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}"#,
    )
    .unwrap();
    let start = |name: &str, vault: Option<&Path>| -> Child {
        let mut command = Command::new(env!("CARGO_BIN_EXE_helios-node"));
        command
            .arg("--graph")
            .arg(&network)
            .arg("--socket")
            .arg(p.0.join(format!("{name}.sock")))
            .arg("--batch-control")
            .arg(p.0.join(format!("{name}-batch.sock")))
            .env_remove("HELIOS_DEBUG_CONTROL")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(vault) = vault {
            command.arg("--vault").arg(vault);
        }
        let mut child = command.spawn().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert!(ready.starts_with("ready "), "{ready}");
        child
    };
    let control = |name: &str, words: &[&str]| -> serde_json::Value {
        let out = Command::new(env!("CARGO_BIN_EXE_helios"))
            .arg("control")
            .arg("--socket")
            .arg(p.0.join(format!("{name}-batch.sock")))
            .args(words)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null)
    };
    let stop = |mut child: Child| {
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
        let end = Instant::now() + Duration::from_secs(30);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > end {
                let _ = child.kill();
                panic!("the node did not stop in 30 s after SIGTERM");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    };

    let file = p.0.join("vault.db");
    let node = start("owner", Some(&file));
    // Another client of the node follows its settings events.
    let (ack, mut heard) = helios_inspect::subscribe(
        &helios_inspect::Address::Unix(p.0.join("owner.sock")),
        "settings.",
        None,
    )
    .unwrap();
    assert_eq!(ack["ok"], true, "{ack}");
    heard
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();

    let empty = control("owner", &["settings-get", "user"]);
    let text = "[interface.graph]\nzoom = \"detail\"\n";
    let toml = p.0.join("user.toml");
    std::fs::write(&toml, text).unwrap();
    let set = control("owner", &["settings-set", "user", toml.to_str().unwrap()]);
    // The event, after the subscription's own lines (a heartbeat says
    // nothing).
    let event = loop {
        let line = heard.read().expect("the change heard in 30 s");
        if !line["event"].is_null() {
            break line["event"].clone();
        }
    };
    let read = control("owner", &["settings-get", "user"]);
    let refused = control("owner", &["settings-get", "global"]);
    let refused_set = control(
        "owner",
        &["settings-set", "interface", toml.to_str().unwrap()],
    );
    drop(heard);
    stop(node);

    assert_eq!(empty["ok"], true, "{empty}");
    assert!(empty["text"].is_null(), "{empty}");
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(event["kind"], "settings.layerChanged", "{event}");
    assert_eq!(event["payload"]["layer"], "user", "{event}");
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["text"], text, "{read}");
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused_set["ok"], false, "{refused_set}");
    // Kept in the vault, which closed cleanly.
    let again = SqliteStore::open(&file).unwrap();
    assert!(!again.report().unclean_shutdown, "the vault closed cleanly");
    assert_eq!(
        Vault::new(again).config_layer("user").unwrap().as_deref(),
        Some(text)
    );

    let node = start("bare", None);
    let answer = control("bare", &["settings-get", "user"]);
    stop(node);
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(answer["message"]["key"], "node-no-settings", "{answer}");
}
