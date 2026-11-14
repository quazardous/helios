//! `helios-node --vault`: one node owns the vault, runs its jobs (the
//! volume watch, the backups, the health check) and closes the vault
//! cleanly when it stops, in process or on SIGTERM; an import through
//! the node starts the strategy's after-import backup; its settings
//! layers are read and written through its control endpoint; the saved
//! sources' automatic imports run with no window.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{BACKUP_DISK, backed_up, disk, until};

use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_health::runner::Schedule;
use helios_library::import::{self, Placement};
use helios_node::vault_node::{Options, VaultNode};
use helios_storage::strategy::{self, Scope, Strategy, Trigger};
use helios_storage::{Role, Volume, Volumes, registry};
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

/// Volumes plugged in and out by the test.
#[derive(Default)]
struct Plugged(std::sync::Mutex<Vec<Volume>>);

impl Volumes for Plugged {
    fn volumes(&self) -> Vec<Volume> {
        self.0.lock().unwrap().clone()
    }
}

/// Distinct photo bytes: each its own content.
fn photo(path: &Path, seed: u8) {
    let bytes: Vec<u8> = (0..20_000u32)
        .map(|i| (i as u8).wrapping_mul(seed) ^ seed)
        .collect();
    std::fs::write(path, bytes).unwrap();
}

/// The headless node runs the saved sources' automatic imports: a card
/// plugged in is copied with its own choices, a file settled in a watched
/// folder is added where it is, both recorded as `auto:<source>` in the
/// import history; a source whose first run was never confirmed is held
/// and said on the bus, not imported.
#[test]
fn the_node_runs_the_saved_sources_automatic_imports_with_their_own_origin() {
    use helios_library::auto_import::Timing;
    use helios_storage::VolumeKind;

    let p = Place::new("auto");
    let file = p.0.join("vault.db");
    let (card, hot, fresh, into) = (
        p.0.join("EOS_DIGITAL"),
        p.0.join("hot"),
        p.0.join("fresh"),
        p.0.join("photos"),
    );
    let dcim = card.join("DCIM/100CANON");
    for dir in [&dcim, &hot, &fresh] {
        std::fs::create_dir_all(dir).unwrap();
    }
    photo(&dcim.join("IMG_0001.RAF"), 1);
    photo(&dcim.join("IMG_0002.RAF"), 2);
    let vault = Vault::new(SqliteStore::open(&file).unwrap());
    let doc = format!(
        "[[source]]\nconnector = \"card\"\nname = \"Camera\"\npath = \"/media/elsewhere\"\n\
         volume = \"uuid:card\"\nauto = \"plugged\"\nconfirmed = \"2026-11-13\"\n\n\
         [source.choices]\ninto = {into:?}\nlayout = \"flat\"\n\n\
         [[source]]\nconnector = \"folder\"\nname = \"Hot\"\npath = {hot:?}\n\
         auto = \"new-files\"\nconfirmed = \"2026-11-13\"\n\n\
         [[source]]\nconnector = \"folder\"\nname = \"Fresh\"\npath = {fresh:?}\nauto = \"new-files\"\n"
    );
    vault
        .set_config_layer("user/import-sources", Some(&doc))
        .unwrap();
    let bus = Arc::new(Bus::new());
    let jobs = Jobs::new(bus.clone());
    let plugged = Arc::new(Plugged::default());
    let volumes: Arc<dyn Volumes> = plugged.clone();
    let mut node = VaultNode::start(
        vault,
        &file,
        bus.clone(),
        &jobs,
        Options {
            volumes,
            schedule: Schedule::new(
                Instant::now(),
                Duration::from_hours(1),
                Duration::from_hours(1),
            ),
            clock: Duration::from_secs(1),
        },
    );
    node.run_automatic_imports(Timing {
        poll: Duration::from_millis(50),
        settle: Duration::from_millis(200),
    });
    let link = node.link();

    // The card plugged in, mounted elsewhere than it was saved at: known
    // by its volume, its photos copied into its own folder.
    let mut volume = disk("uuid:card", &card, &[]);
    (volume.kind, volume.has_dcim) = (VolumeKind::Removable, true);
    plugged.0.lock().unwrap().push(volume);
    assert!(
        until(Duration::from_secs(30), || node
            .vault()
            .images()
            .unwrap()
            .len()
            == 2),
        "the card imported in 30 s"
    );
    assert!(
        into.join("IMG_0001.RAF").is_file(),
        "copied into its folder"
    );

    // A file settled in the watched folder: added where it is.
    photo(&hot.join("a.raf"), 3);
    // One in the folder never confirmed: held, not imported.
    photo(&fresh.join("b.raf"), 4);
    assert!(
        until(Duration::from_secs(30), || node
            .vault()
            .images()
            .unwrap()
            .len()
            == 3),
        "the hot folder's file imported in 30 s"
    );
    assert!(
        until(Duration::from_secs(30), || bus.since(0).iter().any(|e| {
            e.kind == "import.automatic.held"
                && e.payload["source"] == "Fresh"
                && e.payload["verdict"] == "confirm"
        })),
        "the unconfirmed source held and said in 30 s"
    );
    assert!(
        until(Duration::from_secs(30), || link.running_jobs().is_empty()),
        "the imports ended in 30 s"
    );
    assert_eq!(
        node.vault().images().unwrap().len(),
        3,
        "Fresh not imported"
    );
    let mut origins: Vec<String> = helios_library::import_history::list(node.vault(), None)
        .unwrap()
        .into_iter()
        .map(|r| r.origin)
        .collect();
    origins.sort();
    assert_eq!(origins, ["auto:Camera", "auto:Hot"]);

    let stopping = std::thread::spawn(move || node.stop());
    assert!(
        link.wait_stopped(Duration::from_secs(30)),
        "the node stopped in 30 s"
    );
    stopping.join().unwrap();
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

/// Runs `helios` with `args` on the vault's node: exit code, stdout,
/// stderr.
#[cfg(unix)]
fn helios_cli(args: &[&str], idle: &str) -> (i32, String, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_helios"))
        .args(args)
        .env("HELIOS_IDLE_EXIT", idle)
        .env_remove("HELIOS_NODE")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// The journaled events of the node whose inspect endpoint is `inspect`.
#[cfg(unix)]
fn node_events(inspect: &str) -> Vec<serde_json::Value> {
    let (code, out, err) = helios_cli(&["inspect", "--socket", inspect, "events"], "2");
    assert_eq!(code, 0, "{out}{err}");
    let answer: serde_json::Value = serde_json::from_str(&out).unwrap();
    answer["events"].as_array().cloned().unwrap_or_default()
}

/// Whether process `pid` still runs.
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The binary: a node on a vault, started with no network, writes its
/// rendezvous file beside the vault; `helios import --vault` finds it and
/// imports through it (the node's bus hears it, the command's origin
/// `cli`); a second node on the vault is not advertised; the file goes
/// when the node stops.
#[cfg(unix)]
#[test]
fn the_cli_imports_through_the_node_holding_the_vault() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    use helios_inspect::discovery::{Rendezvous, files};

    let p = Place::new("cli-through-node");
    let roll = p.0.join("roll");
    std::fs::create_dir_all(&roll).unwrap();
    std::fs::write(roll.join("p0.raf"), vec![7u8; 20_000]).unwrap();
    let file = p.0.join("vault.db");
    let inspect = p.0.join("node.sock");
    let start = || {
        Command::new(env!("CARGO_BIN_EXE_helios-node"))
            .arg("--socket")
            .arg(&inspect)
            .arg("--batch-control")
            .arg(p.0.join("batch.sock"))
            .arg("--vault")
            .arg(&file)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let mut node = start();
    let mut ready = String::new();
    BufReader::new(node.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.starts_with("ready "), "{ready}");
    let rendezvous_file = files(&file).rendezvous;
    assert!(
        until(Duration::from_secs(10), || rendezvous_file.exists()),
        "the rendezvous file is written in 10 s"
    );
    let rendezvous = Rendezvous::read(&rendezvous_file).unwrap().unwrap();
    assert_eq!(rendezvous.pid, node.id());
    assert_eq!(
        rendezvous.address,
        p.0.join("batch.sock").display().to_string()
    );
    assert_eq!(rendezvous.protocol, helios_inspect::rpc::PROTOCOL);

    // Another node on the same vault (one taking over runs) works, but is
    // not advertised: the rendezvous still names the owner.
    let mut second = Command::new(env!("CARGO_BIN_EXE_helios-node"))
        .arg("--socket")
        .arg(p.0.join("second.sock"))
        .arg("--batch-control")
        .arg(p.0.join("second-batch.sock"))
        .arg("--vault")
        .arg(&file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(second.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.starts_with("ready "), "{ready}");
    let _ = second.kill();
    let second = second.wait_with_output().unwrap();
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("another node holds this vault"),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        Rendezvous::read(&rendezvous_file).unwrap().unwrap().pid,
        node.id()
    );

    let (code, out, err) = helios_cli(
        &[
            "import",
            "--wait",
            "--vault",
            file.to_str().unwrap(),
            roll.to_str().unwrap(),
        ],
        "2",
    );
    assert_eq!(code, 0, "{out}{err}");
    let report: serde_json::Value = serde_json::from_str(out.lines().last().unwrap()).unwrap();
    assert_eq!(report["report"]["added"], 1, "{out}");
    let events = node_events(inspect.to_str().unwrap());
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "library.imported" && e["payload"]["added"] == 1),
        "the node's bus heard the import: {events:?}"
    );
    assert!(
        events.iter().any(|e| e["kind"] == "control.command"
            && e["payload"]["id"] == "library.import"
            && e["payload"]["origin"] == "cli"),
        "run by the CLI: {events:?}"
    );
    // The CLI started no node of its own.
    assert!(!files(&file).rendezvous.with_extension("log").exists());

    let _ = Command::new("kill")
        .args(["-TERM", &node.id().to_string()])
        .status();
    let end = Instant::now() + Duration::from_secs(30);
    while node.try_wait().unwrap().is_none() {
        if Instant::now() > end {
            let _ = node.kill();
            panic!("the node did not stop in 30 s after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!rendezvous_file.exists(), "the rendezvous file is removed");
}

/// The binary: with no node on the vault, two `helios import` at once
/// start one node and share it; it exits once idle, its rendezvous file
/// removed, and `helios stacks` then starts another.
#[cfg(unix)]
#[test]
fn with_no_node_the_cli_starts_one_shared_and_it_exits_when_idle() {
    use helios_inspect::discovery::{Rendezvous, files};

    let p = Place::new("cli-starts-node");
    let file = p.0.join("vault.db");
    let rolls: Vec<PathBuf> = (0..2u8)
        .map(|k| {
            let roll = p.0.join(format!("roll{k}"));
            std::fs::create_dir_all(&roll).unwrap();
            std::fs::write(roll.join(format!("p{k}.raf")), vec![k + 1; 20_000]).unwrap();
            roll
        })
        .collect();
    let imports: Vec<_> = rolls
        .iter()
        .map(|roll| {
            let (file, roll) = (file.clone(), roll.clone());
            std::thread::spawn(move || {
                helios_cli(
                    &[
                        "import",
                        "--wait",
                        "--vault",
                        file.to_str().unwrap(),
                        roll.to_str().unwrap(),
                    ],
                    "3",
                )
            })
        })
        .collect();
    for import in imports {
        let (code, out, err) = import.join().unwrap();
        assert_eq!(code, 0, "{out}{err}");
    }
    let rendezvous_file = files(&file).rendezvous;
    let rendezvous = Rendezvous::read(&rendezvous_file)
        .unwrap()
        .expect("the started node's rendezvous file");
    let events = node_events(&rendezvous.inspect);
    let imported = events
        .iter()
        .filter(|e| e["kind"] == "library.imported")
        .count();
    assert_eq!(imported, 2, "one node ran both imports: {events:?}");

    assert!(
        until(Duration::from_secs(30), || !alive(rendezvous.pid)),
        "the node exited once idle, in 30 s"
    );
    assert!(!rendezvous_file.exists(), "its rendezvous file is removed");
    let again = SqliteStore::open(&file).unwrap();
    assert!(!again.report().unclean_shutdown, "the vault closed cleanly");
    assert_eq!(Vault::new(again).images().unwrap().len(), 2);

    let (code, out, err) = helios_cli(&["stacks", "--vault", file.to_str().unwrap(), "list"], "1");
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&out).unwrap(),
        serde_json::json!([])
    );
}

/// The desktop's embedded node, played in process: its vault owned, its
/// commands advertised (`helios_node_core::endpoint::advertise`). The
/// CLI finds its rendezvous and runs through it, starting no node of its
/// own; the rendezvous file and the lock go when it stops.
#[cfg(unix)]
#[test]
fn the_cli_reaches_the_desktops_node_through_its_rendezvous() {
    use helios_inspect::discovery::{Lock, Rendezvous, files};
    use helios_node_core::endpoint::{VaultService, advertise};

    let p = Place::new("desktop-rendezvous");
    let roll = p.0.join("roll");
    std::fs::create_dir_all(&roll).unwrap();
    std::fs::write(roll.join("p0.raf"), vec![9u8; 20_000]).unwrap();
    let file = p.0.join("vault.db");
    let vault = Arc::new(Vault::new(SqliteStore::open(&file).unwrap()));
    let bus = Arc::new(Bus::new());
    let heard = bus.subscribe();
    let service = VaultService::new(
        vault.clone(),
        bus.clone(),
        Jobs::new(bus.clone()),
        "desktop-test",
    );
    let advertised = advertise(&file, Arc::new(service), "desktop-test")
        .unwrap()
        .expect("nobody else holds the vault");
    let node_files = files(&file);
    let rendezvous = Rendezvous::read(&node_files.rendezvous)
        .unwrap()
        .expect("the desktop's rendezvous file");
    assert_eq!(rendezvous.pid, std::process::id());
    assert_eq!(rendezvous.instance, "desktop-test");
    assert_eq!(rendezvous.address, advertised.address());
    assert!(Lock::held(&node_files.lock).unwrap());
    // A second advertiser on the same vault is not advertised.
    let other = VaultService::new(vault.clone(), bus.clone(), Jobs::new(bus.clone()), "x");
    assert!(advertise(&file, Arc::new(other), "x").unwrap().is_none());

    let (code, out, err) = helios_cli(
        &[
            "import",
            "--wait",
            "--vault",
            file.to_str().unwrap(),
            roll.to_str().unwrap(),
        ],
        "2",
    );
    assert_eq!(code, 0, "{out}{err}");
    let report: serde_json::Value = serde_json::from_str(out.lines().last().unwrap()).unwrap();
    assert_eq!(report["report"]["added"], 1, "{out}");
    let (code, out, err) = helios_cli(&["stacks", "--vault", file.to_str().unwrap(), "list"], "2");
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&out).unwrap(),
        serde_json::json!([])
    );
    // The desktop's node ran both, for the CLI, and imported the file.
    let ran: Vec<(String, String)> = heard
        .try_iter()
        .filter(|e| e.kind == "control.command")
        .map(|e| {
            (
                e.payload["id"].as_str().unwrap().to_string(),
                e.payload["origin"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        ran,
        [
            ("library.import".to_string(), "cli".to_string()),
            ("library.stacks.list".to_string(), "cli".to_string()),
        ]
    );
    assert_eq!(vault.images().unwrap().len(), 1);
    // The CLI started no node: no log of one, the rendezvous still ours.
    assert!(!node_files.rendezvous.with_extension("log").exists());
    assert_eq!(
        Rendezvous::read(&node_files.rendezvous)
            .unwrap()
            .unwrap()
            .pid,
        std::process::id()
    );

    drop(advertised);
    assert!(!node_files.rendezvous.exists(), "the rendezvous is removed");
    assert!(!Lock::held(&node_files.lock).unwrap(), "the lock is let go");
}
