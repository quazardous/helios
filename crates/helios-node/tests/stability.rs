//! Stability scenarios for the headless node's own jobs (`helios-node
//! --vault`, [`VaultNode`]): a node process is killed (`kill -9`, no
//! clean stop) in the middle of a backup, an import, a health run or a
//! settings write, then started again on the same vault.
//!
//! - A backup killed while copying: no half-written file counts as a copy;
//!   the next run copies what is left, each photo once.
//! - An import killed mid-way: each photo is recorded once, with one copy
//!   in the library folder; no copy is left that the vault does not know
//!   once the import is done again.
//! - A health run killed: the next start runs one cleanly, no run is left
//!   "running".
//! - A settings layer write killed: the layer reads back as the old or the
//!   new text, never a broken one.
//!
//! The node runs in a child process: this test binary itself, started on
//! [`node_process`] with the scene to play in the environment, so the
//! scenarios give it disks and a health schedule of their own. Each child
//! prints what it does as JSON lines, which the scenarios read to time the
//! kill.
//!
//! Not run by CI (it is for compatibility breaks): the bench box's
//! `stability` job runs them (`cargo stability`, the `stability` feature),
//! on demand and before a release.

#![cfg(unix)]

mod common;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
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
use serde_json::{Value, json};

/// The scene a child process plays (unset: [`node_process`] does nothing).
const SCENE: &str = "HELIOS_STABILITY_SCENE";
/// The scenario's folder, for the child.
const DIR: &str = "HELIOS_STABILITY_DIR";
/// A number the scene takes (where a settings loop starts).
const ARG: &str = "HELIOS_STABILITY_ARG";
/// What marks a child's lines, among the test harness's own.
const PREFIX: &str = "stability: ";

/// How long a step of a scenario may take (a child starting, a job
/// ending, a line awaited).
const DEADLINE: Duration = Duration::from_secs(60);
/// A child left alive (its scenario gone) ends itself after this.
const ORPHAN: Duration = Duration::from_secs(300);

/// The photos of the backup and import scenarios.
const PHOTOS: usize = 16;
/// Their size: copies long enough for a kill to land in one.
const PHOTO_BYTES: usize = 2 << 20;
/// The library folder copies are imported into, in the scenario's folder.
const LIBRARY: &str = "libdisk/Library";

/// A scenario's folder, removed when dropped.
struct Place(PathBuf);

impl Place {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("helios-stability-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn vault_file(&self) -> PathBuf {
        self.0.join("vault.db")
    }

    /// The vault, opened while no node runs, and what its opening found.
    fn open(&self) -> (Vault<SqliteStore>, bool) {
        let store = SqliteStore::open(&self.vault_file()).unwrap();
        let unclean = store.report().unclean_shutdown;
        (Vault::new(store), unclean)
    }

    /// `PHOTOS` distinct files in `roll/`, as a card holds them.
    fn roll(&self) -> PathBuf {
        let roll = self.0.join("roll");
        std::fs::create_dir_all(&roll).unwrap();
        for k in 0..PHOTOS {
            let mut bytes = vec![0u8; PHOTO_BYTES];
            for (i, b) in bytes.iter_mut().enumerate() {
                *b = (i as u8) ^ (k as u8).wrapping_mul(37) ^ ((i >> 11) as u8);
            }
            bytes[..8].copy_from_slice(&(k as u64).to_le_bytes());
            std::fs::write(roll.join(format!("p{k:02}.raf")), bytes).unwrap();
        }
        roll
    }

    /// Starts a node process playing `scene`.
    fn node(&self, scene: &str, arg: u64) -> NodeProcess {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "node_process",
                "--ignored",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env(SCENE, scene)
            .env(DIR, &self.0)
            .env(ARG, arg.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        // Its stderr, kept to say why it ended.
        let errors = Arc::new(Mutex::new(String::new()));
        let mut err = child.stderr.take().unwrap();
        let kept = errors.clone();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut err, &mut text);
            kept.lock().unwrap().push_str(&text);
        });
        let (tx, lines) = std::sync::mpsc::channel();
        let raw = errors.clone();
        std::thread::spawn(move || {
            for line in out.lines() {
                let Ok(line) = line else { break };
                // Every line, cut short, kept to say why it ended.
                {
                    let mut kept = raw.lock().unwrap();
                    kept.extend(line.chars().take(300));
                    kept.push('\n');
                }
                // The first follows the harness's `test node_process ... `.
                if let Some((_, json)) = line.split_once(PREFIX)
                    && let Ok(value) = serde_json::from_str::<Value>(json)
                    && tx.send(value).is_err()
                {
                    break;
                }
            }
        });
        NodeProcess {
            scene: scene.into(),
            child: Some(child),
            lines,
            seen: Vec::new(),
            errors,
        }
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A node process and the lines it printed.
struct NodeProcess {
    scene: String,
    child: Option<Child>,
    lines: Receiver<Value>,
    /// Every line read so far.
    seen: Vec<Value>,
    /// What it wrote (stdout cut short, stderr).
    errors: Arc<Mutex<String>>,
}

impl NodeProcess {
    /// Reads lines until one satisfies `want`, at most [`DEADLINE`].
    fn until(&mut self, what: &str, want: impl Fn(&Value) -> bool) -> Value {
        let end = Instant::now() + DEADLINE;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if want(&line) {
                        return line;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!("{}: no {what} in {DEADLINE:?}", self.scene)
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let status = self.child.as_mut().map(|c| c.wait());
                    std::thread::sleep(Duration::from_millis(200));
                    panic!(
                        "{}: ended ({status:?}) before {what}: {:?}\n{}",
                        self.scene,
                        self.seen,
                        self.errors.lock().unwrap()
                    )
                }
            }
        }
    }

    /// `kill -9`, then the lines it printed before dying.
    fn kill(&mut self) {
        let mut child = self.child.take().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        self.drain();
    }

    /// Waits for the scene's end (its `done` line, then a clean exit).
    fn finish(mut self) -> Value {
        let done = self.until("done", |l| l["done"] == true);
        let mut child = self.child.take().unwrap();
        let end = Instant::now() + DEADLINE;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > end {
                let _ = child.kill();
                panic!("{}: did not exit in {DEADLINE:?}", self.scene);
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            status.success(),
            "{}: {status}\n{}",
            self.scene,
            self.errors.lock().unwrap()
        );
        done
    }

    /// Reads what is left of the output of a dead process.
    fn drain(&mut self) {
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(5)) {
            self.seen.push(line);
        }
    }
}

impl Drop for NodeProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Files below `dir`, part files (`.helios-part`) apart.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|n| n == ".helios-part") {
            continue;
        }
        if path.is_dir() {
            found.extend(files(&path));
        } else {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// Files left in the part folders below `dir`.
fn part_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, inside: bool, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            let part = inside || path.file_name().is_some_and(|n| n == ".helios-part");
            if path.is_dir() {
                walk(&path, part, found);
            } else if part {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, false, &mut found);
    found
}

/// Every location the vault records below `dir`: (image, path).
fn recorded_below(vault: &Vault<SqliteStore>, dir: &Path) -> Vec<(helios_vault::ImageId, PathBuf)> {
    let mut found = Vec::new();
    for image in vault.images().unwrap() {
        for l in vault.locations(&image.id).unwrap() {
            if l.path.starts_with(dir) {
                found.push((image.id, l.path));
            }
        }
    }
    found
}

/// Each recorded copy is whole: its file there, holding its image.
fn assert_recorded_copies_whole(vault: &Vault<SqliteStore>, dir: &Path) {
    for (id, path) in recorded_below(vault, dir) {
        assert!(
            !path.components().any(|c| c.as_os_str() == ".helios-part"),
            "a part file counted as a copy: {}",
            path.display()
        );
        let held = import::hash_file(&path)
            .unwrap_or_else(|e| panic!("a recorded copy is not there: {e}"));
        assert_eq!(held, id, "{} holds another file", path.display());
    }
}

/// Each file below `dir` holds a different image: none copied twice.
fn assert_each_copied_once(dir: &Path) -> usize {
    let found = files(dir);
    let mut ids: Vec<_> = found
        .iter()
        .map(|p| import::hash_file(p).unwrap())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), found.len(), "a photo copied twice: {found:?}");
    found.len()
}

#[test]
fn stability_a_backup_killed_while_copying_counts_no_half_copy_and_the_next_copies_each_photo_once()
{
    let p = Place::new("backup");
    let roll = p.roll();
    {
        // The library: the photos copied into a library disk's folder; a
        // backup disk; a strategy that backs up by hand only.
        let (vault, _) = p.open();
        let (lib, backup) = disks(&p.0);
        registry::record(&vault, &[lib.clone(), backup.clone()], "studio", 10, None).unwrap();
        for v in [&lib, &backup] {
            registry::save(&vault, v).unwrap();
        }
        let by_hand = Strategy {
            triggers: [Trigger::ByHand].into(),
            ..Strategy::default()
        };
        strategy::save(&vault, &Scope::Library, Some(&by_hand)).unwrap();
        let jobs = Jobs::new(Arc::new(Bus::new()));
        let report =
            import::import(&vault, &jobs, &[roll], &copy_options(&p.0), 1, &mut |_| {}).unwrap();
        assert_eq!(report.added, PHOTOS as u64);
    }
    let backup_dir = p.0.join("backup");

    // Killed while it copies: a few photos in.
    let mut node = p.node("backup", 0);
    node.until("a backup a few photos in", |l| {
        l["kind"] == "job.progress"
            && l["payload"]["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("backup-"))
            && l["payload"]["done"].as_u64().is_some_and(|d| d >= 3)
    });
    node.kill();
    let (vault, unclean) = p.open();
    assert!(unclean, "the vault was not closed: checked at the opening");
    let counted = backed_up(&vault);
    eprintln!(
        "killed after {counted} of {PHOTOS} copies counted; part files left: {:?}",
        part_files(&backup_dir)
    );
    assert!(counted < PHOTOS, "the kill landed after the backup ended");
    assert_recorded_copies_whole(&vault, &backup_dir);
    // A copy may have taken its name just before the kill, unrecorded:
    // whole, and the next run finds it.
    for file in files(&backup_dir) {
        let id = import::hash_file(&file).unwrap();
        assert!(
            vault.image(&id).unwrap().is_some(),
            "{} is no photo of the library",
            file.display()
        );
    }
    drop(vault);

    // Started again: the next run copies what is left.
    let done = p.node("backup", 0).finish();
    eprintln!("the next run: {done}");
    let (vault, unclean) = p.open();
    assert!(!unclean, "the node closed the vault");
    assert_eq!(backed_up(&vault), PHOTOS, "every photo has its copy");
    assert_recorded_copies_whole(&vault, &backup_dir);
    assert_eq!(
        assert_each_copied_once(&backup_dir),
        PHOTOS,
        "one copy each"
    );
    // No copy counted twice on the disk.
    assert_eq!(recorded_below(&vault, &backup_dir).len(), PHOTOS);
}

#[test]
fn stability_an_import_killed_mid_way_records_each_photo_once_and_leaves_no_unknown_copy() {
    let p = Place::new("import");
    p.roll();
    let library = p.0.join(LIBRARY);

    let mut node = p.node("import", 0);
    for _ in 0..3 {
        node.until("an import a few photos in", |l| !l["file"].is_null());
    }
    node.kill();
    let (vault, unclean) = p.open();
    assert!(unclean, "the vault was not closed: checked at the opening");
    let recorded = vault.images().unwrap().len();
    eprintln!(
        "killed after {recorded} of {PHOTOS} photos recorded, {} files in the library folder; part files left: {:?}",
        files(&library).len(),
        part_files(&p.0)
    );
    assert!(recorded < PHOTOS, "the kill landed after the import ended");
    assert_recorded_copies_whole(&vault, &library);
    drop(vault);

    // The same import again, through a node started again.
    let done = p.node("import", 0).finish();
    eprintln!("the import again: {done}");
    let (vault, unclean) = p.open();
    assert!(!unclean, "the node closed the vault");
    assert_eq!(vault.images().unwrap().len(), PHOTOS, "each photo recorded");
    assert_recorded_copies_whole(&vault, &library);
    let copies = recorded_below(&vault, &library);
    assert_eq!(copies.len(), PHOTOS, "one library copy each: {copies:?}");
    // Every file in the library folder is a copy the vault knows.
    assert_eq!(assert_each_copied_once(&library), PHOTOS, "no unknown copy");
}

#[test]
fn stability_a_health_run_killed_leaves_nothing_running_and_the_next_start_runs_one() {
    let p = Place::new("health");
    drop(p.open());

    // Runs back to back: the kill lands in one.
    let mut node = p.node("health-loop", 0);
    for _ in 0..3 {
        node.until("a health run", |l| {
            l["kind"] == "job.started"
                && l["payload"]["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("health-"))
        });
    }
    node.kill();
    let (vault, unclean) = p.open();
    assert!(unclean, "the vault was not closed: checked at the opening");
    drop(vault);

    let done = p.node("health-once", 0).finish();
    assert_eq!(done["running_at_start"], false, "{done}");
    assert_eq!(done["runs"], 1, "{done}");
    assert_eq!(done["findings_ran"], true, "{done}");
    assert_eq!(done["jobs_left"], json!([]), "{done}");
    let (_, unclean) = p.open();
    assert!(!unclean, "the node closed the vault");
}

#[test]
fn stability_a_settings_write_killed_leaves_the_old_or_the_new_layer() {
    let p = Place::new("settings");
    drop(p.open());
    let mut from = 0;
    for round in 0..3 {
        let mut node = p.node("settings-loop", from);
        // Some writes in, then killed while it writes.
        node.until("settings writes", |l| {
            l["wrote"].as_u64().is_some_and(|n| n >= from + 20)
        });
        node.kill();
        let last = node
            .seen
            .iter()
            .filter_map(|l| l["wrote"].as_u64())
            .max()
            .unwrap();
        let (vault, unclean) = p.open();
        assert!(unclean, "the vault was not closed: checked at the opening");
        let text = vault.config_layer("user").unwrap().expect("a user layer");
        let kept = round_of(&text).expect("the layer names its round");
        assert!(
            kept == last || kept == last + 1,
            "round {round}: the layer is {kept}, the last write answered {last}"
        );
        assert_eq!(text, layer_text(kept), "round {round}: the layer is whole");
        drop(vault);

        // The node started again reads it whole and writes on.
        let done = p.node("settings-read", 0).finish();
        assert_eq!(done["round"], kept, "{done}");
        assert_eq!(done["whole"], true, "{done}");
        let (vault, unclean) = p.open();
        assert!(!unclean, "the node closed the vault");
        assert_eq!(
            vault.config_layer("user").unwrap().as_deref(),
            Some(layer_text(AFTER).as_str())
        );
        eprintln!("round {round}: killed after write {last}, the layer kept {kept}");
        from = last + 100;
    }
}

/// The round a node started again writes.
const AFTER: u64 = 1_000_000;

/// The user layer of write `round`: TOML of some size, which says its
/// round.
fn layer_text(round: u64) -> String {
    let fill = "x".repeat(32 * 1024 + (round % 7) as usize * 4096);
    format!("[stability]\nround = {round}\nfill = \"{fill}\"\n")
}

fn round_of(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|l| l.strip_prefix("round = "))
        .and_then(|n| n.parse().ok())
}

/// The library disk and the backup disk, below `dir`.
fn disks(dir: &Path) -> (Volume, Volume) {
    (
        disk("uuid:lib", &dir.join("libdisk"), &[Role::Library]),
        disk(BACKUP_DISK, &dir.join("backup"), &[Role::Backup]),
    )
}

/// An import copying into the library folder.
fn copy_options(dir: &Path) -> import::Options {
    import::Options {
        placement: Placement::Copy {
            into: Some(dir.join(LIBRARY)),
        },
        signals: false,
        ..import::Options::default()
    }
}

/// Disks that answer slowly, as a busy machine's: a backup's photos take
/// a while each, so a kill lands in the run.
struct Slow(Vec<Volume>);

impl Volumes for Slow {
    fn volumes(&self) -> Vec<Volume> {
        std::thread::sleep(Duration::from_millis(15));
        self.0.clone()
    }
}

/// A child's line.
fn say(line: &Value) {
    println!("{PREFIX}{line}");
}

/// The node process the scenarios start and kill: plays the scene its
/// environment names, printing what it does.
#[test]
#[ignore = "a node process the stability scenarios start and kill"]
fn node_process() {
    let Ok(scene) = std::env::var(SCENE) else {
        return;
    };
    let dir = PathBuf::from(std::env::var(DIR).unwrap());
    let arg: u64 = std::env::var(ARG).unwrap().parse().unwrap();
    // Never outlives its scenario for long.
    std::thread::spawn(|| {
        std::thread::sleep(ORPHAN);
        std::process::exit(3);
    });

    let file = dir.join("vault.db");
    let vault = Vault::new(SqliteStore::open(&file).unwrap());
    let bus = Arc::new(Bus::new());
    let heard = bus.subscribe();
    std::thread::spawn(move || {
        for e in heard {
            if e.kind.starts_with("job.") || e.kind.starts_with("library.") {
                say(&json!({ "kind": e.kind, "payload": e.payload }));
            }
        }
    });
    let jobs = Jobs::new(bus.clone());
    let now = Instant::now();
    let none: Arc<dyn Volumes> = Arc::new(Vec::<Volume>::new());
    let (volumes, schedule, clock) = match scene.as_str() {
        "backup" => {
            let (lib, backup) = disks(&dir);
            let slow: Arc<dyn Volumes> = Arc::new(Slow(vec![lib, backup]));
            (
                slow,
                Schedule::new(now, Duration::from_hours(1), Duration::from_hours(1)),
                Duration::from_secs(1),
            )
        }
        // A run every few milliseconds.
        "health-loop" => (
            none,
            Schedule::new(now, Duration::ZERO, Duration::from_millis(5)),
            Duration::from_millis(2),
        ),
        // One run at once.
        "health-once" => (
            none,
            Schedule::new(
                now + Duration::from_millis(300),
                Duration::ZERO,
                Duration::from_hours(1),
            ),
            Duration::from_millis(20),
        ),
        _ => (
            none,
            Schedule::new(now, Duration::from_hours(1), Duration::from_hours(1)),
            Duration::from_secs(1),
        ),
    };
    let node = VaultNode::start(
        vault,
        &file,
        bus,
        &jobs,
        Options {
            volumes,
            schedule,
            clock,
        },
    );
    let link = node.link();
    let mut done = json!({ "done": true });

    match scene.as_str() {
        "backup" => {
            node.backups().back_up_now().unwrap();
            assert!(
                until(DEADLINE, || backed_up(node.vault()) == PHOTOS),
                "every photo backed up in {DEADLINE:?}"
            );
        }
        "import" => {
            let report = node
                .importer()
                .import(&[dir.join("roll")], &copy_options(&dir), &mut |line| {
                    say(&json!({ "file": line }));
                    std::thread::sleep(Duration::from_millis(30));
                })
                .unwrap();
            done["report"] = serde_json::to_value(&report).unwrap();
        }
        // Until killed.
        "health-loop" => loop {
            std::thread::sleep(Duration::from_millis(50));
        },
        "settings-loop" => {
            let settings = node.settings();
            for round in arg.. {
                settings.write("user", &layer_text(round)).unwrap();
                say(&json!({ "wrote": round }));
            }
        }
        "health-once" => {
            done["running_at_start"] = json!(node.health().running());
            assert!(
                until(DEADLINE, || node.health().runs() >= 1
                    && !node.health().running()),
                "a health run ended in {DEADLINE:?}"
            );
            done["runs"] = json!(node.health().runs());
            done["findings_ran"] = json!(node.health().findings().ran());
        }
        "settings-read" => {
            let text = node.settings().read("user").unwrap().unwrap_or_default();
            let round = round_of(&text);
            done["round"] = json!(round);
            done["whole"] = json!(round.is_some_and(|r| text == layer_text(r)));
            node.settings().write("user", &layer_text(AFTER)).unwrap();
        }
        other => panic!("no scene {other}"),
    }
    assert!(
        until(DEADLINE, || link.running_jobs().is_empty()),
        "the node's jobs ended in {DEADLINE:?}"
    );
    done["jobs_left"] = json!(link.running_jobs());
    let stopping = std::thread::spawn(move || node.stop());
    assert!(link.wait_stopped(DEADLINE), "the node stopped");
    stopping.join().unwrap();
    say(&done);
}
