//! The panic hunt over `helios-mcp`: every tool `tools/list` gives is
//! called with hostile arguments built from its input schema (wrong
//! types, out-of-range numbers, missing and extra fields, huge and odd
//! strings, deep nesting), and the JSON-RPC envelope itself is spoiled.
//! Agents send malformed calls; each must end in a clean answer.
//!
//! The server runs as agents run it: its own process, talking JSON-RPC on
//! stdin and stdout to a node of its own (a small network, the control
//! socket on). Each tool gets a fresh node and server; one that dies is
//! started again. A call fails the hunt when the server or the node
//! panics (their stderr says so), when no answer comes within
//! [`DEADLINE`], when the answer is not a tool result (a JSON-RPC error
//! or a malformed result), or when a file appears in the server's working
//! folder or beside the roots' only file.
//!
//! No call touches the machine's files: the server is confined to a root
//! in the hunt's own folder, its home, configuration, caches and runtime
//! folder are there too, and every path it is given names nothing (a
//! folder that does not exist, or one under a file). The AI service has
//! no provider it can reach.
//!
//! The generators follow the node commands' hunt (`helios-node-core`'s
//! `panic_hunt`): the same kinds of hostile values, fitted to the tools'
//! schemas.

#![cfg(all(unix, debug_assertions))]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use helios_node::mcp::{Endpoint, TOOLS};
use serde_json::{Map, Value, json};

/// How long one call may run.
const DEADLINE: Duration = Duration::from_secs(8);

/// How long a node or a server may take to start.
const READY: Duration = Duration::from_secs(30);

/// The hunt stops after this many wrong calls: the report is long enough.
const MAX_WRONG: usize = 40;

/// The longest wait a hostile `timeout_ms` asks for: a longer one is a
/// wait the agent asked for, not a hang.
const LONGEST_WAIT_MS: f64 = 1_000.0;

/// A path that names nothing on any machine.
const NOWHERE: &str = "/nonexistent-helios-mcp-hunt/photo.raw";

/// The node's network: a source and one module.
const NETWORK: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 16, "height": 8, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}"#;

/// The arguments that name a file or a folder.
const PATH_ARGUMENTS: &[&str] = &[
    "path",
    "file",
    "dir",
    "out",
    "vault",
    "workspace",
    "config",
    "source",
];

/// What a process wrote on its stderr, read as it comes.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<String>>);

impl Log {
    fn text(&self) -> std::sync::MutexGuard<'_, String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn follow(&self, from: impl Read + Send + 'static) {
        let log = self.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(from).lines() {
                let Ok(line) = line else { break };
                let mut text = log.text();
                text.push_str(&line);
                text.push('\n');
            }
        });
    }

    fn len(&self) -> usize {
        self.text().len()
    }

    /// The first panic reported after byte `from`, with its message.
    fn panic_since(&self, from: usize) -> Option<String> {
        let text = self.text();
        let tail = text.get(from..)?;
        let at = tail.find("panicked at")?;
        Some(tail[at..].lines().take(2).collect::<Vec<_>>().join(" "))
    }

    fn last_lines(&self) -> String {
        let text = self.text();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(6)..].join(" | ")
    }
}

/// The hunt's own folder: the root the server is confined to, with one
/// file in it, the home and the working folders.
struct Lab {
    dir: PathBuf,
    root: PathBuf,
    cwd: PathBuf,
}

impl Lab {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("helios-hunt-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["root", "home", "cwd", "node-cwd", "tmp", "run", "bin"] {
            std::fs::create_dir_all(dir.join(sub)).expect("the hunt's folders");
        }
        std::fs::write(dir.join("root/file"), b"a file, not a folder").expect("the root's file");
        std::fs::write(dir.join("network.json"), NETWORK).expect("the network");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join("run"), std::fs::Permissions::from_mode(0o700))
                .expect("a private runtime folder");
        }
        Self {
            root: dir.join("root"),
            cwd: dir.join("cwd"),
            dir,
        }
    }

    /// An empty home: what a tool kept there (an AI provider added, a
    /// workspace's state) does not reach the next tool.
    fn fresh_home(&self) {
        let home = self.dir.join("home");
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::create_dir_all(&home);
    }

    /// A path inside the root that names nothing: it goes under a file.
    fn inside(&self) -> String {
        self.root.join("file/photo.raw").display().to_string()
    }

    /// Every variable the server and the node read, pointing into the
    /// hunt's folder; nothing else from the test's environment. `PATH` is
    /// an empty folder: no other program runs (a keyring's tool, a
    /// local AI runtime).
    fn environment(&self, command: &mut Command) {
        let home = self.dir.join("home");
        command
            .env_clear()
            .current_dir(&self.cwd)
            .env("PATH", self.dir.join("bin"))
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_RUNTIME_DIR", self.dir.join("run"))
            .env("TMPDIR", self.dir.join("tmp"))
            .env("HELIOS_AI_DIR", home.join("ai"))
            // The discard port: nothing answers there.
            .env("OLLAMA_HOST", "127.0.0.1:9")
            .env("HELIOS_IDLE_EXIT", "1");
    }

    fn socket(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn node(&self) -> Result<Node, String> {
        for name in ["node.sock", "control.sock"] {
            let _ = std::fs::remove_file(self.socket(name));
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_helios-node"));
        self.environment(&mut command);
        command
            .current_dir(self.dir.join("node-cwd"))
            .env("HELIOS_DEBUG_CONTROL", "1")
            .arg("--graph")
            .arg(self.dir.join("network.json"))
            .arg("--socket")
            .arg(self.socket("node.sock"))
            .arg("--control")
            .arg(self.socket("control.sock"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| format!("the node: {e}"))?;
        let log = Log::default();
        if let Some(stderr) = child.stderr.take() {
            log.follow(stderr);
        }
        let stdout = child.stdout.take().ok_or("the node's stdout")?;
        let (ready, started) = mpsc::channel();
        std::thread::spawn(move || {
            let mut lines = BufReader::new(stdout).lines();
            let _ = ready.send(matches!(lines.next(), Some(Ok(_))));
            // Read the rest, so the node never blocks on a full pipe.
            for _ in lines {}
        });
        let mut node = Node { child, log };
        match started.recv_timeout(READY) {
            Ok(true) => Ok(node),
            _ => {
                node.stop();
                Err(format!("the node did not start: {}", node.log.last_lines()))
            }
        }
    }

    fn server(&self) -> Result<Server, String> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_helios-mcp"));
        self.environment(&mut command);
        command
            .arg("--socket")
            .arg(self.socket("node.sock"))
            .arg("--control")
            .arg(self.socket("control.sock"))
            .arg("--read-root")
            .arg(&self.root)
            .arg("--write-root")
            .arg(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| format!("the server: {e}"))?;
        let log = Log::default();
        if let Some(stderr) = child.stderr.take() {
            log.follow(stderr);
        }
        let stdin = child.stdin.take().ok_or("the server's stdin")?;
        let stdout = child.stdout.take().ok_or("the server's stdout")?;
        let (lines, answers) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
        let mut server = Server {
            child,
            stdin,
            answers,
            log,
            next: 0,
        };
        let init = json!({ "protocolVersion": "2025-06-18", "capabilities": {} });
        match server.request("initialize", &init) {
            (_, Reply::Answer(answer)) if answer["result"]["serverInfo"]["name"] == "helios" => {
                Ok(server)
            }
            (_, reply) => {
                server.stop();
                Err(format!(
                    "the server did not start ({reply:?}): {}",
                    server.log.last_lines()
                ))
            }
        }
    }

    /// What appeared where no argument pointed: in the server's working
    /// folder, or in the root beside its file. Removed once seen.
    fn strays(&self) -> Vec<String> {
        let mut found = Vec::new();
        for (dir, keep) in [(&self.cwd, None), (&self.root, Some("file"))] {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                if Some(entry.file_name().to_string_lossy().as_ref()) == keep {
                    continue;
                }
                let path = entry.path();
                found.push(path.display().to_string());
                let _ = std::fs::remove_dir_all(&path);
                let _ = std::fs::remove_file(&path);
            }
        }
        if !self.root.join("file").is_file() {
            found.push("the root's file is gone".into());
            let _ = std::fs::write(self.root.join("file"), b"a file, not a folder");
        }
        found
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Node {
    child: Child,
    log: Log,
}

impl Node {
    fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What came back for one message.
#[derive(Debug)]
enum Reply {
    Answer(Value),
    /// Nothing within the deadline.
    Silent,
    /// The server closed its output: it exited.
    Gone,
    /// A line that is not JSON.
    Garbled(String),
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    answers: Receiver<String>,
    log: Log,
    next: u64,
}

impl Server {
    /// Sends one line and waits for one line back.
    fn send(&mut self, line: &str) -> Reply {
        if writeln!(self.stdin, "{line}").is_err() || self.stdin.flush().is_err() {
            return Reply::Gone;
        }
        match self.answers.recv_timeout(DEADLINE) {
            Ok(line) => serde_json::from_str(&line).map_or_else(
                |_| Reply::Garbled(shown(&Value::String(line))),
                Reply::Answer,
            ),
            Err(RecvTimeoutError::Timeout) => Reply::Silent,
            Err(RecvTimeoutError::Disconnected) => Reply::Gone,
        }
    }

    /// A request with a fresh id; the id and the reply.
    fn request(&mut self, method: &str, params: &Value) -> (Value, Reply) {
        self.next += 1;
        let id = json!(self.next);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        (id, self.send(&message.to_string()))
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Odd strings: empty, blank, control characters, huge, bidi and emoji,
/// numbers as text, format patterns, paths that name nothing, outside
/// the roots and inside them, a relative path, and the names the
/// network knows.
fn strings(inside: &str) -> Vec<String> {
    vec![
        String::new(),
        " ".into(),
        "\0".into(),
        "x".repeat(1 << 16),
        "\u{202e}é🙂\u{fffd}".into(),
        "-1".into(),
        "18446744073709551616".into(),
        "%s{}${x}{{{date".into(),
        NOWHERE.into(),
        inside.into(),
        "../../nowhere".into(),
        "exp".into(),
        "src".into(),
    ]
}

/// A value of every JSON kind, each hostile in its own way.
fn hostile(inside: &str) -> Vec<Value> {
    let mut values = vec![
        Value::Null,
        json!(true),
        json!(0),
        json!(-1),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(0.5),
        json!(1e308),
    ];
    values.extend(strings(inside).into_iter().map(Value::String));
    let mut deep = json!([]);
    for _ in 0..64 {
        deep = json!([deep]);
    }
    values.extend([
        json!([]),
        json!([null]),
        json!(["exp", "", NOWHERE, inside, "\0"]),
        json!(vec!["exp"; 2_000]),
        json!([[1e308, -1e308], [-1, -1], [u64::MAX, 0], [0.5, 0.5]]),
        json!([{}]),
        json!([{ "path": inside, "params": { "exp": { "exposure": { "float": 1e308 } } } }]),
        json!({}),
        json!({ "": null }),
        json!({ "float": 1e308 }),
        json!({ "int": u64::MAX, "bool": "no" }),
        json!({ "x": -1, "y": i64::MIN, "width": u64::MAX, "height": 0 }),
        json!({ "exp": { "exposure": { "choice": "\0" } } }),
        deep,
    ]);
    values
}

/// A string its argument takes, plain: a path inside the roots for a
/// path, else a name the network knows.
fn typical_string(name: &str, inside: &str) -> Value {
    if PATH_ARGUMENTS.contains(&name) {
        json!(inside)
    } else if name == "param" {
        json!("exposure")
    } else {
        json!("exp")
    }
}

/// A value its schema takes, plain: the first of an enum or of an
/// `anyOf`, the least number, a known name or a path inside the roots
/// for a string, the fewest items, an object with its required
/// properties.
fn typical(name: &str, schema: &Value, inside: &str) -> Value {
    if let Some(first) = schema["enum"].as_array().and_then(|e| e.first()) {
        return first.clone();
    }
    if let Some(first) = schema["anyOf"].as_array().and_then(|a| a.first()) {
        return typical(name, first, inside);
    }
    let kind = match &schema["type"] {
        Value::String(kind) => kind.as_str(),
        Value::Array(kinds) => kinds
            .iter()
            .filter_map(Value::as_str)
            .find(|k| *k != "null")
            .unwrap_or("null"),
        _ => "object",
    };
    match kind {
        "string" => typical_string(name, inside),
        "integer" if name == "timeout_ms" => json!(0),
        "integer" => json!(schema["minimum"].as_i64().unwrap_or(0).max(1)),
        "number" => json!(schema["minimum"].as_f64().unwrap_or(0.0)),
        "boolean" => json!(false),
        "array" => {
            let n = schema["minItems"].as_u64().unwrap_or(1);
            let item = typical(name, &schema["items"], inside);
            Value::Array((0..n).map(|_| item.clone()).collect())
        }
        "object" => Value::Object(
            required(schema)
                .into_iter()
                .map(|field| {
                    let value = typical(field, &schema["properties"][field], inside);
                    (field.to_string(), value)
                })
                .collect(),
        ),
        _ => Value::Null,
    }
}

fn required(schema: &Value) -> Vec<&str> {
    schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// The arguments a tool is called with: not an object, empty, plain,
/// with an unknown field, then each argument in turn given every hostile
/// value, and each required one left out. A tool that waits is given no
/// wait at all, and no hostile wait longer than [`LONGEST_WAIT_MS`].
fn cases_of(schema: &Value, inside: &str) -> Vec<Value> {
    let mut base = typical("", schema, inside);
    if !schema["properties"]["timeout_ms"].is_null() {
        base["timeout_ms"] = json!(0);
    }
    let mut cases = vec![
        json!("arguments"),
        json!(7),
        json!([]),
        Value::Null,
        json!({}),
        base.clone(),
    ];
    let with = |name: &str, value: Value| {
        let mut arguments = base.clone();
        arguments[name] = value;
        arguments
    };
    cases.push(with("zzz", json!(1)));
    let needed = required(schema);
    for name in schema["properties"]
        .as_object()
        .into_iter()
        .flat_map(Map::keys)
    {
        for value in hostile(inside) {
            let long_wait = value.as_f64().is_some_and(|ms| ms > LONGEST_WAIT_MS);
            if name == "timeout_ms" && long_wait {
                continue;
            }
            cases.push(with(name, value));
        }
        if needed.contains(&name.as_str()) {
            let mut arguments = base.clone();
            if let Some(fields) = arguments.as_object_mut() {
                fields.remove(name);
            }
            cases.push(arguments);
        }
    }
    cases
}

/// A short form of `value` for the report.
fn shown(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 160 {
        format!("{}…", text.chars().take(160).collect::<String>())
    } else {
        text
    }
}

/// Whether `reply` is a well-formed tool result for request `id`; its
/// `isError` when it is.
fn tool_result(id: &Value, reply: &Reply) -> Result<bool, String> {
    let answer = answer_to(id, reply)?;
    if let Some(error) = answer.get("error") {
        return Err(format!(
            "a JSON-RPC error instead of a tool result: {}",
            shown(error)
        ));
    }
    let result = &answer["result"];
    let content = result["content"].as_array().filter(|c| !c.is_empty());
    let texts = content.is_some_and(|items| {
        items
            .iter()
            .all(|item| item["type"] == "text" && item["text"].is_string())
    });
    match result["isError"].as_bool() {
        Some(is_error) if texts => Ok(is_error),
        _ => Err(format!("a malformed tool result: {}", shown(result))),
    }
}

/// The answer to request `id` (null for a message the server could not
/// read, which has no id), or why there is none.
fn answer_to<'a>(id: &Value, reply: &'a Reply) -> Result<&'a Value, String> {
    match reply {
        Reply::Answer(answer) if answer["jsonrpc"] == "2.0" && &answer["id"] == id => Ok(answer),
        Reply::Answer(answer) => Err(format!("an answer to another id: {}", shown(answer))),
        Reply::Silent => Err(format!("no answer within {DEADLINE:?}")),
        Reply::Gone => Err("the server exited".into()),
        Reply::Garbled(line) => Err(format!("an answer that is not JSON: {line}")),
    }
}

/// Whether `reply` is a clean JSON-RPC refusal for request `id`: an
/// error with an integer code and a message, or a result.
fn clean_answer(id: &Value, reply: &Reply) -> Result<bool, String> {
    let answer = answer_to(id, reply)?;
    match answer.get("error") {
        None if answer.get("result").is_some() => Ok(false),
        Some(error) if error["code"].is_i64() && error["message"].is_string() => Ok(true),
        _ => Err(format!(
            "neither a result nor a clean error: {}",
            shown(answer)
        )),
    }
}

/// The hunt's tally.
struct Hunt {
    lab: Lab,
    node: Option<Node>,
    server: Option<Server>,
    calls: usize,
    answered: usize,
    refused: usize,
    wrong: Vec<String>,
}

impl Hunt {
    fn new(name: &str) -> Self {
        Self {
            lab: Lab::new(name),
            node: None,
            server: None,
            calls: 0,
            answered: 0,
            refused: 0,
            wrong: Vec::new(),
        }
    }

    /// A node and a server running, started again when gone.
    fn ready(&mut self) -> Result<(), String> {
        if self.node.as_mut().is_none_or(Node::exited) {
            self.node = Some(self.lab.node()?);
        }
        if self.server.is_none() {
            self.server = Some(self.lab.server()?);
        }
        Ok(())
    }

    /// A fresh node, server and home, for the next tool.
    fn fresh(&mut self) {
        self.server = None;
        self.node = None;
        self.lab.fresh_home();
    }

    fn full(&self) -> bool {
        self.wrong.len() >= MAX_WRONG
    }

    /// Sends one message (`line`, the request with `id`); `judge` reads
    /// the reply. Records what went wrong, the panics of the server and
    /// the node first, and starts again what died.
    fn call(
        &mut self,
        what: &str,
        id: &Value,
        line: &str,
        judge: impl Fn(&Value, &Reply) -> Result<bool, String>,
    ) {
        if self.full() {
            return;
        }
        if let Err(e) = self.ready() {
            self.wrong.push(format!("{what}: {e}"));
            return;
        }
        let (Some(server), Some(node)) = (self.server.as_mut(), self.node.as_mut()) else {
            return;
        };
        let marks = (server.log.len(), node.log.len());
        let reply = server.send(line);
        let restart = matches!(reply, Reply::Silent | Reply::Gone | Reply::Garbled(_));
        let verdict = if let Some(panic) = server.log.panic_since(marks.0) {
            Err(format!("the server panicked: {panic}"))
        } else if let Some(panic) = node.log.panic_since(marks.1) {
            Err(format!("the node panicked: {panic}"))
        } else if node.exited() {
            Err(format!("the node exited: {}", node.log.last_lines()))
        } else {
            judge(id, &reply)
        };
        self.calls += 1;
        let strays = self.lab.strays();
        match verdict {
            _ if !strays.is_empty() => {
                self.wrong.push(format!("{what}: left {strays:?}"));
            }
            Ok(false) => self.answered += 1,
            Ok(true) => self.refused += 1,
            Err(e) => {
                self.wrong.push(format!("{what}: {e}"));
                // A panic may leave the server half-way: start it again.
                self.server = None;
            }
        }
        if restart {
            self.server = None;
        }
    }

    /// Calls `tool` with every case its schema gives.
    fn tool(&mut self, tool: &Value) {
        let name = tool["name"].as_str().unwrap_or_default();
        let inside = self.lab.inside();
        self.fresh();
        for arguments in cases_of(&tool["inputSchema"], &inside) {
            let id = json!(self.calls + 1);
            let message = json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": { "name": name, "arguments": arguments },
            });
            let what = format!("`{name}` {}", shown(&arguments));
            self.call(&what, &id, &message.to_string(), tool_result);
        }
    }

    /// Sends `message` as is, its `id` the one judged.
    fn message(&mut self, message: &Value, judge: impl Fn(&Value, &Reply) -> Result<bool, String>) {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        self.call(&shown(message), &id, &message.to_string(), judge);
    }

    /// Fails with the wrong calls, if any.
    fn report(&self, what: &str) {
        println!(
            "MCP panic hunt, {what}: {} calls, {} answered, {} refused, {} wrong",
            self.calls,
            self.answered,
            self.refused,
            self.wrong.len()
        );
        assert!(
            self.wrong.is_empty(),
            "{} calls went wrong (the hunt stops at {MAX_WRONG}):\n{}",
            self.wrong.len(),
            self.wrong.join("\n")
        );
    }
}

/// The tools `tools/list` gives whose endpoint is one of `endpoints`;
/// every tool the server lists is one of [`TOOLS`].
fn listed(hunt: &mut Hunt, endpoints: &[Endpoint]) -> Vec<Value> {
    hunt.ready().expect("a node and a server");
    let server = hunt.server.as_mut().expect("a server");
    let (id, reply) = server.request("tools/list", &json!({}));
    let answer = answer_to(&id, &reply).expect("tools/list answers");
    let tools = answer["result"]["tools"]
        .as_array()
        .expect("a list of tools")
        .clone();
    assert_eq!(tools.len(), TOOLS.len(), "the server lists every tool");
    tools
        .into_iter()
        .filter(|listed| {
            TOOLS
                .iter()
                .find(|t| listed["name"] == t.name)
                .is_some_and(|t| endpoints.contains(&t.endpoint))
        })
        .collect()
}

fn hunt_tools(what: &str, endpoints: &[Endpoint]) {
    let mut hunt = Hunt::new(what);
    let tools = listed(&mut hunt, endpoints);
    assert!(!tools.is_empty(), "{what}: no tool");
    for tool in &tools {
        hunt.tool(tool);
    }
    hunt.report(&format!("{} tools ({what})", tools.len()));
}

#[test]
fn the_node_tools_answer_hostile_arguments_without_panicking() {
    hunt_tools("node", &[Endpoint::Inspect, Endpoint::Control]);
}

#[test]
fn the_workspace_tools_answer_hostile_arguments_without_panicking() {
    hunt_tools("workspace", &[Endpoint::Workspace]);
}

#[test]
fn the_ai_sandbox_and_command_tools_answer_hostile_arguments_without_panicking() {
    hunt_tools(
        "server",
        &[Endpoint::Ai, Endpoint::Sandbox, Endpoint::Commands],
    );
}

/// The envelope spoiled: lines that are not JSON or too large, odd ids,
/// methods and params, a tool's name and arguments of every kind. Each
/// gets a clean answer; a notification gets none, and the server still
/// answers after it.
#[test]
fn a_spoiled_envelope_gets_a_clean_answer() {
    let mut hunt = Hunt::new("envelope");
    let inside = hunt.lab.inside();
    let null = Value::Null;
    for line in [
        "{".to_string(),
        "[]".into(),
        "\"tools/list\"".into(),
        "\u{feff}{}".into(),
        "\0".into(),
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"{}\"}}",
            "x".repeat(2 << 20)
        ),
    ] {
        hunt.call(&shown(&json!(line)), &null, &line, clean_answer);
    }
    for value in hostile(&inside) {
        hunt.message(
            &json!({ "jsonrpc": "2.0", "id": value, "method": "ping" }),
            clean_answer,
        );
        hunt.message(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": value }),
            clean_answer,
        );
        for method in ["initialize", "tools/list", "ping", "tools/call"] {
            hunt.message(
                &json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": value }),
                clean_answer,
            );
        }
        hunt.message(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": value } }),
            clean_answer,
        );
        hunt.message(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": value, "arguments": {} } }),
            tool_result,
        );
        hunt.message(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "helios_state", "arguments": value } }),
            tool_result,
        );
        // A notification: no answer, then a ping answered.
        if let Some(server) = hunt.server.as_mut() {
            let note = json!({ "jsonrpc": "2.0", "method": "tools/call", "params": value });
            let _ = writeln!(server.stdin, "{note}");
        }
        hunt.message(
            &json!({ "jsonrpc": "2.0", "id": "after", "method": "ping" }),
            clean_answer,
        );
    }
    hunt.report("the envelope");
}

#[test]
fn the_hunt_paths_name_nothing() {
    let lab = Lab::new("paths");
    for path in [NOWHERE.to_string(), lab.inside()] {
        assert!(!Path::new(&path).exists(), "{path}");
        let parent = Path::new(&path).parent().expect("a parent");
        assert!(!parent.is_dir(), "{path}: nothing can be created there");
    }
}
