//! `helios batch` as a client of the workflow level's runs: the same batch
//! run again extends the same run and writes nothing twice (`--again`
//! starts a new one), a batch kept in a vault and interrupted is finished
//! by the same batch run again, and a run that waits for a person's answer
//! ends the batch instead of hanging. Every wait has a deadline.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const NETWORK: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}"#;

/// A decision nobody answers before the export.
const DECIDED: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 8, "height": 8, "channels": 4 } }],
  "modules": [
    { "name": "bright", "module": "decide_exposure" },
    { "name": "exp", "module": "exposure" }
  ],
  "connections": [
    { "from": "src", "port": "image", "to": "bright", "input": "input" },
    { "from": "bright", "port": "output", "to": "exp", "input": "input" }
  ],
  "decisions": { "bright": "full_ai" }
}"#;

/// How long a batch of a few pictures may take.
const DEADLINE: Duration = Duration::from_mins(1);

/// A fresh directory; removed when dropped.
struct Place(PathBuf);

impl Place {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("helios-batch-runs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn s(&self, name: &str) -> String {
        self.join(name).to_str().unwrap().to_string()
    }

    /// A picture of `width` by `height` at `name`.
    fn picture(&self, name: &str, width: u32, height: u32) -> PathBuf {
        let path = self.join(name);
        image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x * 7 % 256) as u8, (y * 11 % 256) as u8, 60])
        })
        .save(&path)
        .unwrap();
        path
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// A running `helios batch`: its stdout lines come through a channel, so
/// each read has a deadline; killed when dropped.
struct Batch {
    child: Child,
    lines: mpsc::Receiver<Value>,
    stderr: std::thread::JoinHandle<String>,
}

impl Batch {
    fn spawn(args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_helios"))
            .arg("batch")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                let value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
                if tx.send(value).is_err() {
                    return;
                }
            }
        });
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        Self {
            child,
            lines,
            stderr,
        }
    }

    /// The next line, within the deadline.
    fn line(&self) -> Value {
        self.lines
            .recv_timeout(DEADLINE)
            .expect("a line within the deadline")
    }

    fn interrupt(&self) {
        let sent = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(sent.success());
    }

    /// Every line left, the exit code and stderr, within the deadline.
    fn finish(mut self) -> (i32, Vec<Value>, String) {
        let until = Instant::now() + DEADLINE;
        let code = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status.code().unwrap_or(-1);
            }
            assert!(Instant::now() < until, "helios batch still runs");
            std::thread::sleep(Duration::from_millis(20));
        };
        let mut lines = Vec::new();
        // The reader ends with stdout, which the exit closed.
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(5)) {
            lines.push(line);
        }
        let stderr = std::mem::replace(&mut self.stderr, std::thread::spawn(String::new));
        (code, lines, stderr.join().unwrap())
    }
}

impl Drop for Batch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs `helios batch` to its end: exit code, lines, stderr.
fn batch(args: &[&str]) -> (i32, Vec<Value>, String) {
    Batch::spawn(args).finish()
}

fn line_of<'a>(lines: &'a [Value], path: &Path) -> &'a Value {
    lines
        .iter()
        .find(|l| l["path"] == json!(path))
        .unwrap_or_else(|| panic!("no line for {}: {lines:?}", path.display()))
}

/// A node serving the batch endpoint; killed when dropped.
struct BatchNode {
    node: Child,
    inspect: PathBuf,
    batch: PathBuf,
}

impl BatchNode {
    fn start(place: &Place, network: &str) -> Self {
        std::fs::write(place.join("node.json"), network).unwrap();
        let (inspect, batch) = (place.join("node.sock"), place.join("batch.sock"));
        let mut node = Command::new(env!("CARGO_BIN_EXE_helios-node"))
            .arg("--graph")
            .arg(place.join("node.json"))
            .arg("--socket")
            .arg(&inspect)
            .arg("--batch-control")
            .arg(&batch)
            .env_remove("HELIOS_DEBUG_CONTROL")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, rx) = mpsc::channel();
        let stdout = node.stdout.take().unwrap();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let line = rx.recv_timeout(DEADLINE).expect("the node starts");
        assert!(line.starts_with("ready "), "{line}");
        Self {
            node,
            inspect,
            batch,
        }
    }

    /// The batch arguments attaching to this node.
    fn attach(&self) -> [String; 4] {
        [
            "--socket".into(),
            self.inspect.to_str().unwrap().into(),
            "--control".into(),
            self.batch.to_str().unwrap().into(),
        ]
    }

    fn runs(&self) -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_helios"))
            .args(["control", "--socket", self.batch.to_str().unwrap()])
            .arg("workflow-runs")
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }
}

impl Drop for BatchNode {
    fn drop(&mut self) {
        let _ = self.node.kill();
        let _ = self.node.wait();
    }
}

#[test]
fn the_same_batch_again_extends_its_run_and_writes_nothing_twice() {
    let p = Place::new("again");
    let node = BatchNode::start(&p, NETWORK);
    let (a, b, c) = (
        p.picture("a.png", 8, 4),
        p.picture("b.png", 4, 8),
        p.picture("c.png", 6, 6),
    );
    let attach = node.attach();
    let run = |images: &[&Path], extra: &[&str]| {
        let mut args: Vec<&str> = attach.iter().map(String::as_str).collect();
        let out = p.s("out");
        args.extend(["--out", &out, "--format", "png"]);
        args.extend(extra);
        args.extend(images.iter().map(|i| i.to_str().unwrap()));
        let (code, lines, err) = batch(&args);
        assert_eq!(code, 0, "{err}\n{lines:?}");
        assert_eq!(lines.len(), images.len(), "{lines:?}");
        lines
    };
    let first = run(&[&a, &b], &[]);
    let modified = |name: &str| {
        std::fs::metadata(p.join("out").join(name))
            .unwrap()
            .modified()
            .unwrap()
    };
    let (a_at, b_at) = (modified("a.png"), modified("b.png"));
    // Again, with one more: the two already written are not written again.
    let second = run(&[&a, &b, &c], &[]);
    for image in [&a, &b] {
        assert_eq!(
            line_of(&second, image)["output"],
            line_of(&first, image)["output"]
        );
    }
    assert!(line_of(&second, &c)["error"].is_null());
    assert_eq!(files(&p.join("out")), ["a.png", "b.png", "c.png"]);
    assert_eq!((modified("a.png"), modified("b.png")), (a_at, b_at));
    let runs = node.runs();
    let all = runs["runs"].as_array().expect("runs");
    assert_eq!(all.len(), 1, "{runs}");
    assert_eq!(all[0]["items"], 3, "{runs}");
    assert_eq!(all[0]["title"], "Batch of 2 images", "{runs}");

    // `--again`: a new run, its names chosen against the files there.
    let third = run(&[&a], &["--again"]);
    assert_eq!(
        line_of(&third, &a)["output"],
        json!(p.join("out/a-1.png")),
        "{third:?}"
    );
    assert_eq!(node.runs()["runs"].as_array().unwrap().len(), 2);
}

#[test]
fn a_batch_kept_in_a_vault_and_interrupted_is_finished_by_running_it_again() {
    let p = Place::new("resume");
    std::fs::write(p.join("network.json"), NETWORK).unwrap();
    // Large enough that the batch is interrupted before its end.
    let images: Vec<PathBuf> = (0..6)
        .map(|i| p.picture(&format!("in{i}.png"), 900, 600))
        .collect();
    let (network, out, vault) = (p.s("network.json"), p.s("out"), p.s("vault.db"));
    let mut args = vec![
        "--network",
        &network,
        "--out",
        &out,
        "--format",
        "png",
        "--jobs",
        "1",
        "--vault",
        &vault,
    ];
    args.extend(images.iter().map(|i| i.to_str().unwrap()));

    let running = Batch::spawn(&args);
    let first = running.line();
    assert!(first["error"].is_null(), "{first}");
    running.interrupt();
    let (code, rest, err) = running.finish();
    let mut lines = vec![first];
    lines.extend(rest);
    assert_eq!(lines.len(), images.len(), "{err}\n{lines:?}");
    let stopped: Vec<&Value> = lines.iter().filter(|l| !l["error"].is_null()).collect();
    if code == 130 {
        // The node stopped with the batch: what it had not written waits.
        assert!(!stopped.is_empty(), "{lines:?}");
        assert!(
            stopped
                .iter()
                .all(|l| l["message"]["key"] == "batch-stopped"),
            "{lines:?}"
        );
    } else {
        // Every export ended before the interrupt was seen.
        assert_eq!(code, 0, "{err}");
        assert!(stopped.is_empty(), "{lines:?}");
    }
    let written_before = files(&p.join("out"));

    // The same batch again: it finishes the run, each file written once.
    let (code, again, err) = batch(&args);
    assert_eq!(code, 0, "{err}\n{again:?}");
    assert_eq!(again.len(), images.len(), "{again:?}");
    for (i, image) in images.iter().enumerate() {
        let line = line_of(&again, image);
        assert!(line["error"].is_null(), "{line}");
        assert_eq!(
            line["output"],
            json!(p.join("out").join(format!("in{i}.png")))
        );
    }
    let names: Vec<String> = (0..images.len()).map(|i| format!("in{i}.png")).collect();
    assert_eq!(files(&p.join("out")), names, "no file written twice");
    for name in written_before {
        assert!(names.contains(&name), "{name}");
    }
}

#[test]
fn a_run_that_waits_for_a_person_ends_the_batch_instead_of_hanging() {
    let p = Place::new("asking");
    std::fs::write(p.join("network.json"), DECIDED).unwrap();
    let (a, b) = (p.picture("a.png", 8, 4), p.picture("b.png", 4, 8));
    let (code, lines, err) = batch(&[
        "--network",
        &p.s("network.json"),
        "--out",
        &p.s("out"),
        "--format",
        "png",
        a.to_str().unwrap(),
        b.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "{err}\n{lines:?}");
    assert_eq!(lines.len(), 2, "{lines:?}");
    for image in [&a, &b] {
        let line = line_of(&lines, image);
        assert_eq!(line["message"]["key"], "batch-asking", "{line}");
        assert!(line["output"].is_null(), "{line}");
    }
    assert!(files(&p.join("out")).is_empty());
}
