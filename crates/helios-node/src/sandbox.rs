//! Sandboxes: a node with its own directory, settings and sockets, created,
//! started, stopped and removed in one command each.
//!
//! A sandbox directory holds `sandbox.json` (its marker and state), the
//! network, and one file per configuration layer. Its sockets live in a
//! short runtime directory derived from the sandbox path, because Unix
//! socket paths are limited to about 108 bytes. A sandbox is never created
//! over a directory that already holds something else, and is only removed
//! when it carries its marker: tools never point at real data.
//!
//! Where Unix sockets do not exist (Windows), the node serves loopback TCP
//! instead, its session token in the sandbox directory; `sandbox.json`
//! then records the addresses it bound (`tcp`).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use helios_core::ContentHash;
use helios_inspect::Address;
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
    /// The loopback TCP endpoints the node bound, where Unix sockets do
    /// not exist: they replace `inspect` and `control`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp: Option<Tcp>,
}

/// A node's TCP endpoints (`tcp://127.0.0.1:PORT`) and its session token's file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tcp {
    pub inspect: String,
    pub control: Option<String>,
    pub token_file: PathBuf,
}

impl State {
    /// How a client reaches the node's inspection endpoint.
    pub fn inspect_address(&self) -> Result<Address, String> {
        match &self.tcp {
            None => Ok(Address::Unix(self.inspect.clone())),
            Some(tcp) => tcp_address(&tcp.inspect, &tcp.token_file),
        }
    }

    /// How a client reaches the node's control (or batch) endpoint.
    pub fn control_address(&self) -> Result<Address, String> {
        match &self.tcp {
            None => Ok(Address::Unix(self.control.clone())),
            Some(Tcp {
                control: Some(control),
                token_file,
                ..
            }) => tcp_address(control, token_file),
            Some(_) => Err("the sandbox node serves no control endpoint".into()),
        }
    }
}

fn tcp_address(address: &str, token_file: &Path) -> Result<Address, String> {
    let token = std::fs::read_to_string(token_file)
        .map_err(|e| format!("{}: {e}", token_file.display()))?;
    Address::parse(address, Some(token.trim().to_string()))
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
        tcp: None,
    };
    let longest = state
        .inspect
        .as_os_str()
        .len()
        .max(state.control.as_os_str().len());
    if cfg!(unix) && longest > MAX_SOCKET_PATH {
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
#[cfg(unix)]
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

/// Whether the node of this sandbox runs: it answers on its endpoint with
/// the session token (a reused port or pid is never mistaken for it).
#[cfg(not(unix))]
fn is_our_node(_pid: u32, state: &State) -> bool {
    state.inspect_address().is_ok_and(|address| {
        helios_inspect::request_within(
            &address,
            &serde_json::json!({ "cmd": "state" }),
            std::time::Duration::from_secs(2),
        )
        .is_ok_and(|answer| answer["ok"] == true)
    })
}

/// Whether a process still runs (Windows: `tasklist` lists it).
#[cfg(not(unix))]
fn process_runs(pid: u32) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
}

/// Starts the sandbox's node (`node_bin` is the `helios-node` executable)
/// and returns once it serves. The control endpoint is served in debug builds.
pub fn start(dir: &Path, node_bin: &Path) -> Result<State, String> {
    launch(dir, node_bin, false, None)
}

/// Starts the sandbox's node like [`start`], with the batch endpoint
/// (`load-source`, `export`, `cancel`, the runs' `workflow.*`) on the
/// control socket instead of the debug one: what `helios batch` needs, in
/// every build. With `runs_vault`, the node keeps its runs there
/// (`--runs-vault`), so they outlive it.
pub fn start_batch(
    dir: &Path,
    node_bin: &Path,
    runs_vault: Option<&Path>,
) -> Result<State, String> {
    launch(dir, node_bin, true, runs_vault)
}

/// Creates the runtime directory, open to this user only: the sockets in
/// it are the node's way in. Changing its mode also proves it is ours (only
/// its owner may), so a directory planted by another user is refused.
fn private_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(())
}

fn launch(
    dir: &Path,
    node_bin: &Path,
    batch: bool,
    runs_vault: Option<&Path>,
) -> Result<State, String> {
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut state = read_state(&dir)?;
    if let Some(pid) = state.pid
        && is_our_node(pid, &state)
    {
        return Err(format!("the sandbox node already runs (pid {pid})"));
    }
    private_dir(&runtime_dir(&dir))?;
    // Unix sockets, or loopback TCP (ports picked by the node) with a
    // session token where they do not exist.
    let tcp = !cfg!(unix);
    let token_file = dir.join("token");
    let endpoint = |path: &Path| -> std::ffi::OsString {
        if tcp {
            "tcp://127.0.0.1:0".into()
        } else {
            path.as_os_str().to_owned()
        }
    };
    let mut command = Command::new(node_bin);
    command
        .arg("--graph")
        .arg(dir.join("network.json"))
        .arg("--socket")
        .arg(endpoint(&state.inspect))
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
    if let Some(vault) = runs_vault {
        let vault = std::path::absolute(vault).map_err(|e| format!("{}: {e}", vault.display()))?;
        command.arg("--runs-vault").arg(vault);
    }
    if tcp {
        command.arg("--token-file").arg(&token_file);
    }
    let control = batch || cfg!(debug_assertions);
    if batch {
        command.arg("--batch-control").arg(endpoint(&state.control));
    } else if control {
        command
            .arg("--control")
            .arg(endpoint(&state.control))
            .env("HELIOS_DEBUG_CONTROL", "1");
    }
    // No console window of its own on Windows (the node is a console
    // program started from a window or a terminal), and its own process
    // group: a Ctrl-C in the terminal is not for it.
    #[cfg(windows)]
    {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::os::windows::process::CommandExt::creation_flags(
            &mut command,
            CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW,
        );
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", node_bin.display()))?;
    let mut reader = BufReader::new(child.stdout.take().expect("piped"));
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    let Some(inspect) = line.trim().strip_prefix("ready ").map(str::to_string) else {
        let _ = child.wait();
        let log = std::fs::read_to_string(dir.join("node.log")).unwrap_or_default();
        return Err(format!("the node did not start: {}", log.trim()));
    };
    if tcp {
        // Then the control endpoint's address, `control ADDRESS` or
        // `batch-control ADDRESS`.
        let mut control_address = None;
        if control {
            let mut line = String::new();
            reader.read_line(&mut line).map_err(|e| e.to_string())?;
            control_address = line
                .trim()
                .split_once(' ')
                .map(|(_, address)| address.to_string());
        }
        state.tcp = Some(Tcp {
            inspect,
            control: control_address,
            token_file,
        });
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
        // SIGTERM; Windows has none for a program without a window: the
        // process is ended.
        #[cfg(unix)]
        let mut kill = Command::new("kill");
        #[cfg(unix)]
        kill.arg(pid.to_string());
        #[cfg(not(unix))]
        let mut kill = Command::new("taskkill");
        #[cfg(not(unix))]
        kill.args(["/F", "/PID", &pid.to_string()]);
        kill.stdout(Stdio::null())
            .status()
            .map_err(|e| e.to_string())?;
        // The node is not our child: watch it go, bounded.
        for _ in 0..200 {
            #[cfg(unix)]
            let runs = is_our_node(pid, &state);
            #[cfg(not(unix))]
            let runs = process_runs(pid);
            if !runs {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    let _ = std::fs::remove_file(&state.inspect);
    let _ = std::fs::remove_file(&state.control);
    state.tcp = None;
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
