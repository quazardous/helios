//! Many clients on one headless node (`helios-node --vault`), each a real
//! [`Client`] over the node's batch socket, all at once:
//!
//! - reads: `node.hello`, `commands.list`, `library.stacks.list`,
//!   `library.edit.head`, some sent several at once on one client;
//! - writes: `library.edit.save` on images every client shares, and a
//!   stack made then dissolved again (its undo) on the client's own images;
//! - imports of a small folder of the client's own, again and again (each
//!   photo is recorded once), and imports cancelled (`$/cancel`) as soon
//!   as they start;
//! - clients that disconnect and connect again, some with a call still
//!   running.
//!
//! Meanwhile subscribers on the node's inspect socket follow the bus
//! (`events.subscribe`). Checked:
//! - every call is answered (or cancelled) within [`DEADLINE`], and the
//!   whole run ends within its own deadline (a watchdog: no deadlock);
//! - no write is lost or duplicated: once the node stopped, each shared
//!   image's versions in the vault are exactly the saves acknowledged, the
//!   images are the base ones plus those the imports said they added, and
//!   no stack is left;
//! - each subscriber saw every save announced (`control.command`), once,
//!   in increasing order;
//! - the node's memory, threads and file descriptors stay bounded, and
//!   the descriptors and threads come back once the clients have gone.
//!
//! A report (calls, calls per second, latency percentiles, errors, the
//! node's memory and descriptors) is printed, and written to the file
//! `HELIOS_STRESS_REPORT` names. The run is deterministic for a seed
//! (what each client does, in which order), not its interleaving.
//!
//! The short variant (4 clients, a few seconds) is a normal test, so the
//! scenario does not rot. The full one (`HELIOS_STRESS_CLIENTS`, 32 by
//! default, for `HELIOS_STRESS_SECONDS`, 60 by default, seed
//! `HELIOS_STRESS_SEED`) sits behind the `stability` feature: the bench
//! box's `stability` job runs it, never CI.
//!
//! The soak is the same scenario, long and moderate: a few clients, each
//! pausing between its calls, for an hour by default
//! (`HELIOS_SOAK_MINUTES`, `HELIOS_SOAK_CLIENTS`, `HELIOS_SOAK_EVERY`
//! seconds between samples, `HELIOS_SOAK_SEED`). Every sample takes the
//! node's memory, threads and descriptors, its open vault files and the
//! size of the vault's write-ahead log, what its caches hold, the bus's
//! subscribers and journal, and how far each subscriber lags behind the
//! bus. Besides every check above, no measure may grow over the run: once
//! warmed up, the mean of the last third of the samples stays within a
//! tolerance of the first third's. The series is written to the file
//! `HELIOS_SOAK_REPORT` names and a summary printed. A short soak (a few
//! seconds, sampled twice a second) is a normal test; the long one sits
//! behind the `stability` feature and runs in the bench box's `soak` job.

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use helios_inspect::Address;
use helios_inspect::client::{Client, Pending};
use helios_inspect::rpc::{Error, code};
use helios_vault::Vault;
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

/// How long one call may take to be answered.
const DEADLINE: Duration = Duration::from_secs(30);

/// The images every client edits.
const SHARED: usize = 6;

/// The network the node renders (it renders nothing here: the scenario
/// is about its vault).
const NETWORK: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}"#;

/// One run's size.
#[derive(Clone, Copy, Debug)]
struct Scale {
    clients: usize,
    run: Duration,
    seed: u64,
    /// Pictures of a client's cancelled imports.
    big: usize,
    /// How long a client waits between two steps: none for the stress
    /// run, a moderate pace for the soak.
    pause: Duration,
}

/// A deterministic generator (splitmix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A picture whose first pixel says whose and which it is: no two alike.
fn picture(path: &Path, set: u8, owner: usize, k: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(32, 24, |x, y| {
        if (x, y) == (0, 0) {
            image::Rgb([set, owner as u8, k as u8])
        } else {
            image::Rgb([(x * 7) as u8 ^ k as u8, (y * 9) as u8 ^ owner as u8, set])
        }
    })
    .save(path)
    .unwrap();
}

/// The node's process, its folder, its sockets.
struct Node {
    dir: PathBuf,
    child: Child,
}

impl Node {
    /// `debug_control` also serves the debug control endpoint
    /// (`control.sock`), which only debug builds have: the soak reads the
    /// caches' sizes there.
    fn start(dir: &Path, debug_control: bool) -> Self {
        std::fs::write(dir.join("network.json"), NETWORK).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_helios-node"));
        if debug_control {
            command
                .arg("--control")
                .arg(dir.join("control.sock"))
                .env("HELIOS_DEBUG_CONTROL", "1");
        } else {
            command.env_remove("HELIOS_DEBUG_CONTROL");
        }
        let mut child = command
            .arg("--graph")
            .arg(dir.join("network.json"))
            .arg("--socket")
            .arg(dir.join("inspect.sock"))
            .arg("--batch-control")
            .arg(dir.join("batch.sock"))
            .arg("--vault")
            .arg(dir.join("vault.db"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(
                std::fs::File::create(dir.join("node.log")).unwrap(),
            ))
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        out.read_line(&mut ready).unwrap();
        assert!(
            ready.starts_with("ready "),
            "the node did not start: {ready}"
        );
        // What else it prints is read, so it never blocks on a full pipe.
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut out, &mut std::io::sink());
        });
        Self {
            dir: dir.to_path_buf(),
            child,
        }
    }

    fn batch(&self) -> Address {
        Address::Unix(self.dir.join("batch.sock"))
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Stops it as a service manager would (SIGTERM); whether it exited
    /// cleanly in time.
    fn stop(&mut self) -> bool {
        let sent = Command::new("kill")
            .args(["-TERM", &self.pid().to_string()])
            .status()
            .is_ok_and(|s| s.success());
        let end = Instant::now() + DEADLINE;
        while sent && Instant::now() < end {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.success();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// The end of its log, for a failure's message.
    fn log(&self) -> String {
        let log = std::fs::read_to_string(self.dir.join("node.log")).unwrap_or_default();
        let lines: Vec<&str> = log.lines().collect();
        lines[lines.len().saturating_sub(20)..].join("\n")
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A process's resident memory (KiB), threads and open descriptors.
#[derive(Clone, Copy, Debug, Default)]
struct Usage {
    rss_kib: u64,
    threads: u64,
    fds: u64,
}

impl Usage {
    fn of(pid: u32) -> Self {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        let field = |name: &str| {
            status
                .lines()
                .find_map(|l| l.strip_prefix(name))
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        };
        Self {
            rss_kib: field("VmRSS:"),
            threads: field("Threads:"),
            fds: std::fs::read_dir(format!("/proc/{pid}/fd")).map_or(0, |d| d.count() as u64),
        }
    }

    fn max(self, other: Self) -> Self {
        Self {
            rss_kib: self.rss_kib.max(other.rss_kib),
            threads: self.threads.max(other.threads),
            fds: self.fds.max(other.fds),
        }
    }

    fn json(self) -> Value {
        json!({ "rss_kib": self.rss_kib, "threads": self.threads, "fds": self.fds })
    }
}

/// What one client did and saw.
#[derive(Default)]
struct Tally {
    /// Whose: the clients end in any order.
    client: usize,
    calls: u64,
    /// Each answered call's time, in microseconds.
    latencies: Vec<u64>,
    /// Calls not answered within the deadline.
    unanswered: u64,
    /// Answers that should not have been, by call.
    errors: Vec<String>,
    /// Imports cancelled by `$/cancel` (answered as cancelled).
    cancelled: u64,
    /// Calls left running when their client disconnected.
    abandoned: u64,
    reconnects: u64,
    /// The versions saves were acknowledged with, by shared image.
    saved: BTreeMap<usize, BTreeSet<String>>,
    /// The saves answered: each was announced on the bus.
    saves_answered: u64,
    /// Photos the imports said they added.
    added: u64,
    /// An import answered without its report: what it added is unknown.
    unreported_imports: u64,
    stacks_made: u64,
    stacks_dissolved: u64,
}

/// A client's own pictures: the small folder it imports again and
/// again, and the bigger one whose imports it cancels.
struct Own {
    small: PathBuf,
    pair: [String; 2],
    big: PathBuf,
}

fn own(root: &Path, c: usize, big: usize) -> Own {
    let small = root.join(format!("c{c}")).join("small");
    let pair = [0, 1].map(|k| {
        let path = small.join(format!("s{k}.png"));
        picture(&path, 2, c, k);
        path.to_str().unwrap().to_string()
    });
    let bigger = root.join(format!("c{c}")).join("big");
    for k in 0..big {
        picture(&bigger.join(format!("b{k:03}.png")), 3, c, k);
    }
    Own {
        small,
        pair,
        big: bigger,
    }
}

/// One client's run, until `end` or `stop`.
#[allow(
    clippy::too_many_arguments,
    reason = "the run's parts, each named by the scenario"
)]
fn client_run(
    c: usize,
    address: &Address,
    shared: &[String],
    own: &Own,
    seed: u64,
    pause: Duration,
    end: Instant,
    stop: &AtomicBool,
) -> Tally {
    let mut rng = Rng(seed ^ (c as u64).wrapping_mul(0x2545_f491_4f6c_dd1d));
    let mut tally = Tally {
        client: c,
        ..Tally::default()
    };
    let origin = format!("agent:stress{c}");
    let connect = || {
        Client::connect(address)
            .unwrap_or_else(|e| panic!("client {c} cannot connect: {e}"))
            .with_origin(&origin)
    };
    let mut client = connect();
    // Waits for a call's answer, timed; `None` when it never came.
    let answer = |tally: &mut Tally, name: &str, pending: &Pending, at: Instant| {
        tally.calls += 1;
        if let Some(outcome) = pending.wait_timeout(DEADLINE) {
            tally.latencies.push(at.elapsed().as_micros() as u64);
            Some(outcome)
        } else {
            tally.unanswered += 1;
            tally
                .errors
                .push(format!("{name}: no answer in {}s", DEADLINE.as_secs()));
            None
        }
    };
    let call = |client: &Client, tally: &mut Tally, name: &str, params: Value| {
        let at = Instant::now();
        let pending = client.call(name, params);
        answer(tally, name, &pending, at)
    };
    let import = |tally: &mut Tally, outcome: Result<Value, Error>| match outcome {
        Ok(v) if v["report"].is_object() => {
            tally.added += v["report"]["added"].as_u64().unwrap_or(0);
            if v["report"]["cancelled"] == true {
                tally.cancelled += 1;
            }
        }
        Ok(v) => {
            tally.unreported_imports += 1;
            tally.errors.push(format!("library.import: {v}"));
        }
        Err(e) if e.code == code::CANCELLED => {
            tally.cancelled += 1;
            tally.unreported_imports += 1;
        }
        Err(e) => tally.errors.push(format!("library.import: {e:?}")),
    };

    // First the client's own folder, so its stack has its images.
    let paths = json!({ "paths": [own.small] });
    if let Some(outcome) = call(&client, &mut tally, "library.import", paths.clone()) {
        import(&mut tally, outcome);
    }
    let mut step = 0u64;
    while Instant::now() < end && !stop.load(Ordering::SeqCst) && tally.unanswered == 0 {
        step += 1;
        let roll = rng.below(100);
        match roll {
            // Reads, one at a time.
            0..25 => {
                let (name, params) = match rng.below(4) {
                    0 => ("node.hello", json!({})),
                    1 => ("commands.list", json!({ "filter": "library" })),
                    2 => ("library.stacks.list", json!({})),
                    _ => (
                        "library.edit.head",
                        json!({ "image": shared[rng.below(shared.len())] }),
                    ),
                };
                if let Some(Err(e)) = call(&client, &mut tally, name, params) {
                    tally.errors.push(format!("{name}: {e:?}"));
                }
            }
            // Reads, several at once on the one connection.
            25..35 => {
                let at = Instant::now();
                let reads: Vec<Pending> = (0..4)
                    .map(|k| {
                        client.call(
                            "library.edit.head",
                            json!({ "image": shared[(k + rng.below(shared.len())) % shared.len()] }),
                        )
                    })
                    .collect();
                for p in &reads {
                    if let Some(Err(e)) = answer(&mut tally, "library.edit.head", p, at) {
                        tally.errors.push(format!("library.edit.head: {e:?}"));
                    }
                }
            }
            // A save on a shared image.
            35..65 => {
                let image = rng.below(shared.len());
                let params = json!({
                    "image": shared[image],
                    "edit": { "stress": { "client": c, "step": step } },
                    "label": format!("client {c} step {step}"),
                });
                match call(&client, &mut tally, "library.edit.save", params) {
                    None => {}
                    Some(outcome) => {
                        tally.saves_answered += 1;
                        match outcome {
                            Ok(v) => match v["version"].as_str() {
                                Some(version) => {
                                    tally
                                        .saved
                                        .entry(image)
                                        .or_default()
                                        .insert(version.to_string());
                                }
                                None => tally.errors.push(format!("library.edit.save: {v}")),
                            },
                            Err(e) => tally.errors.push(format!("library.edit.save: {e:?}")),
                        }
                    }
                }
            }
            // A stack of the client's own pair, then its undo.
            65..75 => {
                let made = call(
                    &client,
                    &mut tally,
                    "library.stacks.create",
                    json!({ "images": own.pair }),
                );
                match made {
                    Some(Ok(v)) => {
                        tally.stacks_made += 1;
                        let id = v["stack"]["id"].as_str().unwrap_or_default().to_string();
                        match call(
                            &client,
                            &mut tally,
                            "library.stacks.dissolve",
                            json!({ "stack": id }),
                        ) {
                            Some(Ok(_)) => tally.stacks_dissolved += 1,
                            Some(Err(e)) => {
                                tally.errors.push(format!("library.stacks.dissolve: {e:?}"));
                            }
                            None => {}
                        }
                    }
                    Some(Err(e)) => tally.errors.push(format!("library.stacks.create: {e:?}")),
                    None => {}
                }
            }
            // The client's own folder imported again: nothing added.
            75..82 => {
                if let Some(outcome) = call(&client, &mut tally, "library.import", paths.clone()) {
                    import(&mut tally, outcome);
                }
            }
            // An import cancelled as soon as it is sent.
            82..90 => {
                let at = Instant::now();
                let pending = client.call("library.import", json!({ "paths": [own.big] }));
                if rng.below(2) == 0 {
                    std::thread::sleep(Duration::from_millis(rng.below(5) as u64));
                }
                client.cancel(&pending);
                if let Some(outcome) = answer(&mut tally, "library.import", &pending, at) {
                    import(&mut tally, outcome);
                }
            }
            // Away and back, now and then with a read still running.
            90..95 => {
                if rng.below(2) == 0 {
                    let _left = client.call("library.stacks.list", json!({}));
                    tally.abandoned += 1;
                }
                drop(std::mem::replace(&mut client, connect()));
                tally.reconnects += 1;
            }
            // A command run by its id.
            _ => {
                let params = json!({ "id": "library.stacks.list" });
                if let Some(Err(e)) = call(&client, &mut tally, "command.run", params) {
                    tally.errors.push(format!("command.run: {e:?}"));
                }
            }
        }
        if !pause.is_zero() {
            std::thread::sleep(pause);
        }
    }
    tally
}

/// What a bus subscriber saw.
#[derive(Default)]
struct Seen {
    /// Saves announced, by origin.
    saves: BTreeMap<String, u64>,
    events: u64,
    /// Events out of order, or a gap the subscription reported.
    disorder: Vec<String>,
}

/// Where a subscriber stands while it runs.
#[derive(Default)]
struct Following {
    /// The saves seen so far.
    saves: AtomicU64,
    /// The sequence number of the last event seen.
    last: AtomicU64,
}

/// Follows the bus's events whose kind starts with `prefix` on the
/// node's inspect socket until `stop`.
fn subscribe(
    socket: &Path,
    prefix: &str,
    stop: Arc<AtomicBool>,
    following: Arc<Following>,
) -> std::thread::JoinHandle<Seen> {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.set_read_timeout(Some(DEADLINE)).unwrap();
    writeln!(
        stream,
        "{}",
        json!({ "jsonrpc": "2.0", "id": 1, "method": "events.subscribe",
                "params": { "prefix": prefix } })
    )
    .unwrap();
    // The answer to the subscription first.
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let ack: Value = serde_json::from_str(&line).unwrap();
    assert!(ack["result"]["subscription"].is_string(), "{line}");
    // From now on a quiet stream still looks at `stop` now and then.
    reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    std::thread::spawn(move || {
        let mut seen = Seen::default();
        if ack["result"]["gap"] == true {
            seen.disorder
                .push("the subscription opened with a gap".into());
        }
        let mut last = 0u64;
        let mut buf = Vec::new();
        loop {
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break,
                Ok(_) if buf.ends_with(b"\n") => {}
                Ok(_) => continue,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    continue;
                }
                Err(_) => break,
            }
            let message: Value = serde_json::from_slice(&buf).unwrap_or(Value::Null);
            buf.clear();
            if message["method"] != "events.event" {
                continue;
            }
            let event = &message["params"]["event"];
            let seq = event["seq"].as_u64().unwrap_or(0);
            if seq <= last {
                seen.disorder.push(format!("event {seq} after {last}"));
            }
            last = seq;
            following.last.store(seq, Ordering::SeqCst);
            seen.events += 1;
            if event["kind"]
                .as_str()
                .is_some_and(|k| k.starts_with("control.command"))
                && event["payload"]["id"] == "library.edit.save"
            {
                let origin = event["payload"]["origin"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                *seen.saves.entry(origin).or_default() += 1;
                following.saves.fetch_add(1, Ordering::SeqCst);
            }
        }
        let _ = reader.read_to_end(&mut Vec::new());
        seen
    })
}

/// The `q` quantile of sorted `values`, in milliseconds.
fn quantile(sorted: &[u64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let at = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[at] as f64 / 1000.0
}

/// A soak's sampling: how often the node is measured.
#[derive(Clone, Copy, Debug)]
struct Soak {
    every: Duration,
}

/// The node measured once, `at_s` seconds into the run.
#[derive(Clone, Copy, Debug, Default)]
struct Sample {
    at_s: f64,
    usage: Usage,
    /// Descriptors open on the vault's files (the database, its
    /// write-ahead log and shared memory): the SQLite handles.
    vault_files: u64,
    wal_bytes: u64,
    /// What the caches hold, in bytes (debug builds only: the debug
    /// control endpoint serves `cache.stats`).
    cache_bytes: Option<u64>,
    bus_subscribers: u64,
    journal: u64,
    /// How many events the slowest subscriber is behind the bus.
    lag: u64,
}

impl Sample {
    fn json(&self) -> Value {
        json!({
            "at_s": self.at_s,
            "rss_kib": self.usage.rss_kib,
            "threads": self.usage.threads,
            "fds": self.usage.fds,
            "vault_files": self.vault_files,
            "wal_bytes": self.wal_bytes,
            "cache_bytes": self.cache_bytes,
            "bus_subscribers": self.bus_subscribers,
            "journal": self.journal,
            "lag": self.lag,
        })
    }
}

/// Measures the node `pid` serving the folder `root`; `inspect` and
/// `control` are clients of its inspect and debug control endpoints.
fn sample(
    pid: u32,
    root: &Path,
    at_s: f64,
    inspect: &Client,
    control: Option<&Client>,
    following: &[Arc<Following>],
) -> Sample {
    let vault = root.join("vault.db");
    let vault_files = std::fs::read_dir(format!("/proc/{pid}/fd")).map_or(0, |d| {
        d.filter_map(Result::ok)
            .filter_map(|e| std::fs::read_link(e.path()).ok())
            .filter(|target| {
                target
                    .as_os_str()
                    .to_string_lossy()
                    .starts_with(vault.to_string_lossy().as_ref())
            })
            .count() as u64
    });
    let wal_bytes = std::fs::metadata(root.join("vault.db-wal")).map_or(0, |m| m.len());
    let ask = |client: &Client, method: &str, params: Value| {
        client
            .call(method, params)
            .wait_timeout(DEADLINE)
            .unwrap_or_else(|| panic!("{method}: no answer in {}s", DEADLINE.as_secs()))
            .unwrap_or_else(|e| panic!("{method}: {e:?}"))
    };
    let bus = ask(inspect, "describe", json!({ "entity": "bus" }));
    let bus = &bus["describe"];
    let last_seq = bus["last_seq"].as_u64().unwrap_or(0);
    let slowest = following
        .iter()
        .map(|f| f.last.load(Ordering::SeqCst))
        .min()
        .unwrap_or(last_seq);
    let cache_bytes = control.map(|control| {
        let stats = ask(control, "cache.stats", json!({}));
        stats["tiers"].as_array().map_or(0, |tiers| {
            tiers.iter().filter_map(|t| t["used"].as_u64()).sum()
        })
    });
    Sample {
        at_s,
        usage: Usage::of(pid),
        vault_files,
        wal_bytes,
        cache_bytes,
        bus_subscribers: bus["subscribers"].as_u64().unwrap_or(0),
        journal: bus["journal"]["len"].as_u64().unwrap_or(0),
        lag: last_seq.saturating_sub(slowest),
    }
}

/// One measure's trend over a soak's samples: the first quarter is the
/// warm-up and left out; the rest is cut in thirds, and the mean of the
/// last third may not exceed the first third's by more than `tolerance`
/// (a function of that first mean). `None` with too few samples or when
/// the measure was not taken.
fn trend(
    samples: &[Sample],
    name: &str,
    measure: impl Fn(&Sample) -> Option<f64>,
    tolerance: impl Fn(f64) -> f64,
) -> Option<(Value, Option<String>)> {
    let steady: Vec<(f64, f64)> = samples[samples.len() / 4..]
        .iter()
        .map(|s| measure(s).map(|v| (s.at_s, v)))
        .collect::<Option<_>>()?;
    if steady.len() < 3 {
        return None;
    }
    let third = steady.len() / 3;
    let mean = |part: &[(f64, f64)]| part.iter().map(|(_, v)| v).sum::<f64>() / part.len() as f64;
    let first = mean(&steady[..third]);
    let last = mean(&steady[steady.len() - third..]);
    // The least-squares slope, for the report.
    let n = steady.len() as f64;
    let (mt, mv) = (
        steady.iter().map(|(t, _)| t).sum::<f64>() / n,
        steady.iter().map(|(_, v)| v).sum::<f64>() / n,
    );
    let num: f64 = steady.iter().map(|(t, v)| (t - mt) * (v - mv)).sum();
    let den: f64 = steady.iter().map(|(t, _)| (t - mt) * (t - mt)).sum();
    let per_hour = if den > 0.0 { num / den * 3600.0 } else { 0.0 };
    let (min, max) = steady
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), (_, v)| {
            (lo.min(*v), hi.max(*v))
        });
    let allowed = tolerance(first);
    let growth = last - first;
    let verdict = (growth > allowed).then(|| {
        format!(
            "{name} grew from {first:.0} to {last:.0} (allowed {allowed:.0}, {per_hour:.0} an hour)"
        )
    });
    Some((
        json!({
            "measure": name,
            "first_third": first,
            "last_third": last,
            "growth": growth,
            "tolerance": allowed,
            "per_hour": per_hour,
            "min": min,
            "max": max,
            "flat": verdict.is_none(),
        }),
        verdict,
    ))
}

/// The scenario at `scale`, in a folder of its own named `name`; with
/// `soak`, the node is sampled all along and no measure may grow.
#[allow(
    clippy::too_many_lines,
    reason = "one scenario from start to checks, in the order it runs"
)]
fn many_clients(name: &str, scale: Scale, soak: Option<Soak>) {
    struct Gone(PathBuf);
    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root = std::env::temp_dir().join(format!("helios-many-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let _gone = Gone(root.clone());

    // The shared images, imported through the node before the clients come.
    let base = root.join("shared");
    let shared: Vec<String> = (0..SHARED)
        .map(|k| {
            let path = base.join(format!("p{k}.png"));
            picture(&path, 1, 0, k);
            path.to_str().unwrap().to_string()
        })
        .collect();
    let owns: Vec<Own> = (0..scale.clients)
        .map(|c| own(&root, c, scale.big))
        .collect();
    let debug_control = soak.is_some() && cfg!(debug_assertions);
    let mut node = Node::start(&root, debug_control);
    {
        let setup = Client::connect(&node.batch()).unwrap();
        let report = setup
            .call("library.import", json!({ "paths": [base] }))
            .wait_timeout(DEADLINE)
            .expect("the shared images imported in time")
            .unwrap_or_else(|e| panic!("{e:?}\n{}", node.log()));
        assert_eq!(report["report"]["added"], SHARED, "{report}");
    }
    std::thread::sleep(Duration::from_millis(200));
    let pid = node.pid();
    let before = Usage::of(pid);

    // The bus followed by a few subscribers from the start.
    let stop = Arc::new(AtomicBool::new(false));
    // The soak's subscribers follow every event, so their lag is counted
    // in the bus's own sequence numbers.
    let (count, prefix) = match soak {
        Some(_) => (2, ""),
        None => (scale.clients.div_ceil(8), "control.command"),
    };
    let subscribers: Vec<_> = (0..count)
        .map(|_| {
            let following = Arc::new(Following::default());
            let handle = subscribe(
                &root.join("inspect.sock"),
                prefix,
                stop.clone(),
                following.clone(),
            );
            (handle, following)
        })
        .collect();
    // The node's usage sampled while the clients run; the soak's series.
    let peak = Arc::new(Mutex::new(before));
    let sampled = Arc::new(AtomicU64::new(0));
    let series: Arc<Mutex<Vec<Sample>>> = Arc::default();
    let started = Instant::now();
    let sampler = {
        let (stop, peak, sampled, series) =
            (stop.clone(), peak.clone(), sampled.clone(), series.clone());
        let following: Vec<Arc<Following>> = subscribers.iter().map(|(_, f)| f.clone()).collect();
        let root = root.clone();
        std::thread::spawn(move || {
            let clients = soak.map(|soak| {
                let inspect = Client::connect(&Address::Unix(root.join("inspect.sock")))
                    .expect("the sampler connects to the inspect socket");
                let control = debug_control.then(|| {
                    Client::connect(&Address::Unix(root.join("control.sock")))
                        .expect("the sampler connects to the debug control socket")
                });
                (soak, inspect, control)
            });
            let mut next = started;
            while !stop.load(Ordering::SeqCst) {
                let now = Usage::of(pid);
                let mut p = peak.lock().unwrap_or_else(PoisonError::into_inner);
                *p = p.max(now);
                sampled.fetch_add(1, Ordering::SeqCst);
                drop(p);
                if let Some((soak, inspect, control)) = &clients
                    && Instant::now() >= next
                {
                    let at_s = started.elapsed().as_secs_f64();
                    let s = sample(pid, &root, at_s, inspect, control.as_ref(), &following);
                    series
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(s);
                    next += soak.every;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };

    let end = started + scale.run;
    let (done, finished) = mpsc::channel();
    let owns = Arc::new(owns);
    let shared = Arc::new(shared);
    let workers = Arc::new(AtomicBool::new(false));
    for c in 0..scale.clients {
        let (done, owns, shared, workers, address) = (
            done.clone(),
            owns.clone(),
            shared.clone(),
            workers.clone(),
            node.batch(),
        );
        std::thread::spawn(move || {
            let tally = client_run(
                c,
                &address,
                &shared,
                &owns[c],
                scale.seed,
                scale.pause,
                end,
                &workers,
            );
            let _ = done.send(tally);
        });
    }
    drop(done);
    // The watchdog: every client ends within the run and one call's
    // deadline, or the node is deadlocked.
    let watchdog = end + DEADLINE * 2;
    let mut tallies = Vec::new();
    while tallies.len() < scale.clients {
        let left = watchdog.saturating_duration_since(Instant::now());
        if let Ok(tally) = finished.recv_timeout(left) {
            tallies.push(tally);
        } else {
            workers.store(true, Ordering::SeqCst);
            stop.store(true, Ordering::SeqCst);
            panic!(
                "deadlock: {} of {} clients still running {}s after the run's end\n{}",
                scale.clients - tallies.len(),
                scale.clients,
                (DEADLINE * 2).as_secs(),
                node.log()
            );
        }
    }
    let elapsed = started.elapsed();

    // The bus: every save announced to every subscriber.
    let mut expected: BTreeMap<String, u64> = BTreeMap::new();
    for t in &tallies {
        if t.saves_answered > 0 {
            expected.insert(format!("agent:stress{}", t.client), t.saves_answered);
        }
    }
    let total_saves: u64 = expected.values().sum();
    // Each save is announced before it is answered; the stream's last
    // events may still be on their way.
    let streamed = Instant::now() + Duration::from_secs(10);
    while subscribers
        .iter()
        .any(|(_, f)| f.saves.load(Ordering::SeqCst) < total_saves)
        && Instant::now() < streamed
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(200));
    stop.store(true, Ordering::SeqCst);
    sampler.join().unwrap();
    let views: Vec<Seen> = subscribers
        .into_iter()
        .map(|(s, _)| s.join().unwrap())
        .collect();
    let peak = *peak.lock().unwrap_or_else(PoisonError::into_inner);

    // Every client and subscriber gone: the node's descriptors and
    // threads come back.
    let settled = Instant::now() + Duration::from_secs(10);
    let mut after = Usage::of(pid);
    while (after.fds > before.fds + 4 || after.threads > before.threads + 4)
        && Instant::now() < settled
    {
        std::thread::sleep(Duration::from_millis(100));
        after = Usage::of(pid);
    }

    assert!(
        node.stop(),
        "the node stopped cleanly on SIGTERM\n{}",
        node.log()
    );

    // The vault, as the node left it.
    let vault = Vault::new(SqliteStore::open(&root.join("vault.db")).unwrap());
    let mut lost = Vec::new();
    for (k, path) in shared.iter().enumerate() {
        let (id, _) = vault
            .at_path(Path::new(path))
            .unwrap()
            .unwrap_or_else(|| panic!("{path} is in the vault"));
        let kept: BTreeSet<String> = vault
            .versions(&id)
            .unwrap()
            .iter()
            .map(|v| v.id.hex())
            .collect();
        let acked: BTreeSet<String> = tallies
            .iter()
            .flat_map(|t| t.saved.get(&k).into_iter().flatten().cloned())
            .collect();
        if kept != acked {
            lost.push(format!(
                "image {k}: {} versions kept, {} saves acknowledged; missing {:?}, extra {:?}",
                kept.len(),
                acked.len(),
                acked.difference(&kept).collect::<Vec<_>>(),
                kept.difference(&acked).collect::<Vec<_>>(),
            ));
        }
    }
    let images = vault.images().unwrap().len() as u64;
    let added: u64 = tallies.iter().map(|t| t.added).sum();
    let unreported: u64 = tallies.iter().map(|t| t.unreported_imports).sum();
    let ceiling = (SHARED + scale.clients * (2 + scale.big)) as u64;
    let stacks = vault.stacks().unwrap().len() as u64;
    let made: u64 = tallies.iter().map(|t| t.stacks_made).sum();
    let dissolved: u64 = tallies.iter().map(|t| t.stacks_dissolved).sum();

    // The report.
    let mut latencies: Vec<u64> = tallies.iter().flat_map(|t| t.latencies.clone()).collect();
    latencies.sort_unstable();
    let calls: u64 = tallies.iter().map(|t| t.calls).sum();
    let errors: Vec<String> = tallies.iter().flat_map(|t| t.errors.clone()).collect();
    let mut report = json!({
        "clients": scale.clients,
        "seconds": elapsed.as_secs_f64(),
        "seed": scale.seed,
        "calls": calls,
        "calls_per_s": calls as f64 / elapsed.as_secs_f64(),
        "latency_ms": {
            "p50": quantile(&latencies, 0.50),
            "p99": quantile(&latencies, 0.99),
            "max": quantile(&latencies, 1.0),
        },
        "errors": errors.len(),
        "unanswered": tallies.iter().map(|t| t.unanswered).sum::<u64>(),
        "saves": total_saves,
        "imports_cancelled": tallies.iter().map(|t| t.cancelled).sum::<u64>(),
        "photos_added": added,
        "stacks_made": made,
        "reconnects": tallies.iter().map(|t| t.reconnects).sum::<u64>(),
        "abandoned_calls": tallies.iter().map(|t| t.abandoned).sum::<u64>(),
        "subscribers": views.len(),
        "events_seen": views.iter().map(|s| s.events).collect::<Vec<_>>(),
        "node": {
            "before": before.json(),
            "peak": peak.json(),
            "after": after.json(),
            "samples": sampled.load(Ordering::SeqCst),
        },
    });
    let mut growing = Vec::new();
    if soak.is_some() {
        let samples = std::mem::take(&mut *series.lock().unwrap_or_else(PoisonError::into_inner));
        let kib = 1024.0;
        #[allow(clippy::type_complexity, reason = "a table of measures")]
        let measures: [(&str, fn(&Sample) -> Option<f64>, fn(f64) -> f64); 9] = [
            (
                "rss_kib",
                |s| Some(s.usage.rss_kib as f64),
                |base| 16.0 * 1024.0 + base / 10.0,
            ),
            ("threads", |s| Some(s.usage.threads as f64), |_| 4.0),
            ("fds", |s| Some(s.usage.fds as f64), |_| 4.0),
            ("vault_files", |s| Some(s.vault_files as f64), |_| 2.0),
            (
                "wal_bytes",
                |s| Some(s.wal_bytes as f64),
                |_| 8.0 * 1024.0 * 1024.0,
            ),
            (
                "cache_bytes",
                |s| s.cache_bytes.map(|b| b as f64),
                |base| 16.0 * 1024.0 * 1024.0 + base / 10.0,
            ),
            (
                "bus_subscribers",
                |s| Some(s.bus_subscribers as f64),
                |_| 2.0,
            ),
            (
                "journal",
                |s| Some(s.journal as f64),
                |_| helios_core::bus::JOURNAL as f64,
            ),
            ("lag", |s| Some(s.lag as f64), |_| 512.0),
        ];
        let mut trends = Vec::new();
        for (name, measure, tolerance) in measures {
            if let Some((t, verdict)) = trend(&samples, name, measure, tolerance) {
                println!(
                    "soak: {name:<16} first third {:>12.0}  last third {:>12.0}  range {:.0}..{:.0}  {:+.0} an hour  {}",
                    t["first_third"].as_f64().unwrap_or(0.0),
                    t["last_third"].as_f64().unwrap_or(0.0),
                    t["min"].as_f64().unwrap_or(0.0),
                    t["max"].as_f64().unwrap_or(0.0),
                    t["per_hour"].as_f64().unwrap_or(0.0),
                    if verdict.is_none() { "flat" } else { "GROWS" },
                );
                trends.push(t);
                growing.extend(verdict);
            }
        }
        report["peak_rss_mib"] = json!(peak.rss_kib as f64 / kib);
        report["samples"] = json!(samples.len());
        println!("soak: {report}");
        report["trends"] = json!(trends);
        report["series"] = json!(samples.iter().map(Sample::json).collect::<Vec<_>>());
        if let Some(file) = std::env::var_os("HELIOS_SOAK_REPORT") {
            let _ = std::fs::write(file, format!("{report:#}\n"));
        }
        assert!(
            samples.len() >= 4,
            "the soak took {} samples, too few for a trend",
            samples.len()
        );
    } else {
        println!("many-clients: {report}");
        if let Some(file) = std::env::var_os("HELIOS_STRESS_REPORT") {
            let _ = std::fs::write(file, format!("{report:#}\n"));
        }
    }

    assert!(
        errors.is_empty(),
        "{} calls answered wrongly, first ones: {:#?}",
        errors.len(),
        &errors[..errors.len().min(10)]
    );
    assert!(lost.is_empty(), "saves lost or duplicated: {lost:#?}");
    if unreported == 0 {
        assert_eq!(
            images,
            SHARED as u64 + added,
            "the vault holds the shared images and those the imports added, each once"
        );
    } else {
        assert!(
            images >= SHARED as u64 + added && images <= ceiling,
            "{images} images: between {} and {ceiling}",
            SHARED as u64 + added
        );
    }
    assert_eq!(stacks, made - dissolved, "every stack made was dissolved");
    assert_eq!(stacks, 0, "no stack left");
    for (k, seen) in views.iter().enumerate() {
        assert!(
            seen.disorder.is_empty(),
            "subscriber {k}: {:?}",
            seen.disorder
        );
        assert_eq!(
            seen.saves, expected,
            "subscriber {k} saw every save announced once, by its client"
        );
    }
    assert!(
        after.fds <= before.fds + 4,
        "the node's descriptors came back once the clients went: {} before, {} after",
        before.fds,
        after.fds
    );
    assert!(
        after.threads <= before.threads + 4,
        "the node's threads came back once the clients went: {} before, {} after",
        before.threads,
        after.threads
    );
    assert!(
        peak.rss_kib <= before.rss_kib + 512 * 1024,
        "the node's memory stayed bounded: {} KiB before, {} KiB at its peak",
        before.rss_kib,
        peak.rss_kib
    );
    assert!(
        growing.is_empty(),
        "the node grew over the soak: {growing:#?}"
    );
}

/// The scenario small and short, so it keeps working between stability
/// runs.
#[test]
fn four_clients_read_write_import_and_cancel_on_one_node() {
    many_clients(
        "short",
        Scale {
            clients: 4,
            run: Duration::from_secs(3),
            seed: 5005,
            big: 8,
            pause: Duration::ZERO,
        },
        None,
    );
}

/// The stress run of the bench box's `stability` job.
#[cfg(feature = "stability")]
#[test]
fn many_clients_on_one_node_for_a_minute() {
    let number = |name: &str, default: u64| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    many_clients(
        "full",
        Scale {
            clients: number("HELIOS_STRESS_CLIENTS", 32) as usize,
            run: Duration::from_secs(number("HELIOS_STRESS_SECONDS", 60)),
            seed: number("HELIOS_STRESS_SEED", 5005),
            big: 24,
            pause: Duration::ZERO,
        },
        None,
    );
}

/// The soak small and short, so it keeps working between soak runs.
#[test]
fn soak_short_four_clients_sampled_twice_a_second() {
    many_clients(
        "soak-short",
        Scale {
            clients: 4,
            run: Duration::from_secs(6),
            seed: 5005,
            big: 8,
            pause: Duration::from_millis(20),
        },
        Some(Soak {
            every: Duration::from_millis(500),
        }),
    );
}

/// The soak of the bench box's `soak` job: an hour by default.
#[cfg(feature = "stability")]
#[test]
fn soak_long_moderate_clients_on_one_node() {
    let number = |name: &str, default: u64| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    many_clients(
        "soak",
        Scale {
            clients: number("HELIOS_SOAK_CLIENTS", 8) as usize,
            run: Duration::from_secs(number("HELIOS_SOAK_MINUTES", 60) * 60),
            seed: number("HELIOS_SOAK_SEED", 5005),
            big: 24,
            pause: Duration::from_millis(100),
        },
        Some(Soak {
            every: Duration::from_secs(number("HELIOS_SOAK_EVERY", 30)),
        }),
    );
}
