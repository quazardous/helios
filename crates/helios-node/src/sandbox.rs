//! Sandboxes: a node with its own directory, settings and sockets, created,
//! started, stopped and removed in one command each.
//!
//! A sandbox directory holds `sandbox.json` (its marker and state), the
//! network, and one file per configuration layer. Its sockets live in a
//! short runtime directory derived from the sandbox path, because Unix
//! socket paths are limited to about 108 bytes. A sandbox is never created
//! over a directory that already holds something else, and is only removed
//! when it carries its marker: tools never point at real data.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use helios_core::ContentHash;
use serde::{Deserialize, Serialize};

const MARKER: &str = "sandbox.json";

/// Longest socket path accepted, under the Unix limit with a margin.
const MAX_SOCKET_PATH: usize = 100;

/// The network a sandbox starts with when none is given.
pub const DEFAULT_NETWORK: &str = r#"{
  "sources": [{ "name": "src", "synthetic": { "kind": "ramp", "width": 64, "height": 32, "channels": 4 } }],
  "modules": [{ "name": "exp", "module": "exposure" }],
  "connections": [{ "from": "src", "port": "image", "to": "exp", "input": "input" }]
}
"#;

/// What `sandbox.json` records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub inspect: PathBuf,
    pub control: PathBuf,
    /// The node's process while it runs.
    pub pid: Option<u32>,
}

fn read_state(dir: &Path) -> Result<State, String> {
    let text = std::fs::read_to_string(dir.join(MARKER))
        .map_err(|_| format!("{} is not a sandbox (no {MARKER})", dir.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", dir.join(MARKER).display()))
}

fn write_state(dir: &Path, state: &State) -> Result<(), String> {
    let text = serde_json::to_string_pretty(state).expect("state serializes");
    std::fs::write(dir.join(MARKER), text).map_err(|e| e.to_string())
}

/// The runtime directory of a sandbox's sockets: short, and unique per sandbox.
fn runtime_dir(dir: &Path) -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    let hash = ContentHash::of(dir.to_string_lossy().as_bytes());
    base.join(format!("helios-{}", hash.short()))
}

/// Creates a sandbox in `dir`, which must not exist or be empty.
pub fn create(dir: &Path, network: Option<&str>) -> Result<State, String> {
    if dir.join(MARKER).exists() {
        return Err(format!("{} is already a sandbox", dir.display()));
    }
    if dir.exists()
        && std::fs::read_dir(dir)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err(format!(
            "refusing to create a sandbox in {}: it exists and holds something else",
            dir.display()
        ));
    }
    std::fs::create_dir_all(dir.join("project")).map_err(|e| e.to_string())?;
    let dir = dir.canonicalize().map_err(|e| e.to_string())?;
    let runtime = runtime_dir(&dir);
    let state = State {
        inspect: runtime.join("node.sock"),
        control: runtime.join("control.sock"),
        pid: None,
    };
    let longest = state
        .inspect
        .as_os_str()
        .len()
        .max(state.control.as_os_str().len());
    if longest > MAX_SOCKET_PATH {
        return Err(format!(
            "socket path {} is too long for a Unix socket ({longest} bytes, at most {MAX_SOCKET_PATH}): set XDG_RUNTIME_DIR to a shorter directory",
            state.inspect.display()
        ));
    }
    let files = [
        ("network.json", network.unwrap_or(DEFAULT_NETWORK)),
        ("global.toml", ""),
        ("user.toml", ""),
        ("project/helios.toml", ""),
    ];
    for (name, text) in files {
        std::fs::write(dir.join(name), text).map_err(|e| e.to_string())?;
    }
    write_state(&dir, &state)?;
    Ok(state)
}

/// Whether a process runs and is the node of this sandbox (its command
/// line names the sandbox's socket): a reused pid is never mistaken for it.
fn is_our_node(pid: u32, state: &State) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| {
            String::from_utf8_lossy(&o.stdout).contains(&*state.inspect.to_string_lossy())
        })
}

/// Starts the sandbox's node (`node_bin` is the `helios-node` executable)
/// and returns once it serves. The control endpoint is served in debug builds.
pub fn start(dir: &Path, node_bin: &Path) -> Result<State, String> {
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut state = read_state(&dir)?;
    if let Some(pid) = state.pid
        && is_our_node(pid, &state)
    {
        return Err(format!("the sandbox node already runs (pid {pid})"));
    }
    std::fs::create_dir_all(runtime_dir(&dir)).map_err(|e| e.to_string())?;
    let mut command = Command::new(node_bin);
    command
        .arg("--graph")
        .arg(dir.join("network.json"))
        .arg("--socket")
        .arg(&state.inspect)
        .arg("--global")
        .arg(dir.join("global.toml"))
        .arg("--project")
        .arg(dir.join("project"))
        .arg("--user")
        .arg(dir.join("user.toml"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // Not a pipe: nobody reads it once this process has exited.
        .stderr(Stdio::from(
            std::fs::File::create(dir.join("node.log")).map_err(|e| e.to_string())?,
        ));
    // Its own process group: a Ctrl-C in the terminal that started it is
    // for that command, not for the node, which outlives it.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    if cfg!(debug_assertions) {
        command
            .arg("--control")
            .arg(&state.control)
            .env("HELIOS_DEBUG_CONTROL", "1");
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", node_bin.display()))?;
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("piped"))
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    if !line.starts_with("ready ") {
        let _ = child.wait();
        let log = std::fs::read_to_string(dir.join("node.log")).unwrap_or_default();
        return Err(format!("the node did not start: {}", log.trim()));
    }
    state.pid = Some(child.id());
    write_state(&dir, &state)?;
    // The node keeps running after this process exits; `stop` ends it.
    std::mem::forget(child);
    Ok(state)
}

/// Stops the sandbox's node if it runs; returns whether it did.
pub fn stop(dir: &Path) -> Result<bool, String> {
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut state = read_state(&dir)?;
    let Some(pid) = state.pid.take() else {
        return Ok(false);
    };
    let ran = is_our_node(pid, &state);
    if ran {
        Command::new("kill")
            .arg(pid.to_string())
            .status()
            .map_err(|e| e.to_string())?;
        // The node is not our child: watch it go, bounded.
        for _ in 0..200 {
            if !is_our_node(pid, &state) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    let _ = std::fs::remove_file(&state.inspect);
    let _ = std::fs::remove_file(&state.control);
    write_state(&dir, &state)?;
    Ok(ran)
}

/// Stops the node and removes the sandbox and its runtime directory. Only a
/// directory carrying the sandbox marker is ever removed.
pub fn clean(dir: &Path) -> Result<(), String> {
    stop(dir)?;
    let dir = dir.canonicalize().map_err(|e| e.to_string())?;
    read_state(&dir)?;
    let _ = std::fs::remove_dir_all(runtime_dir(&dir));
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())
}
