//! How the `helios` CLI and `helios-mcp` reach the node of a vault (see
//! `docs/protocol.md`, "Finding a node"): they never open the vault
//! themselves.
//!
//! 1. `--node ADDRESS` or `HELIOS_NODE` names an endpoint: it is used as
//!    it is (a TCP one with the token of `HELIOS_TOKEN`).
//! 2. Otherwise the vault's rendezvous file
//!    ([`helios_inspect::discovery`]) names the node holding it.
//! 3. With no node holding it, the client takes the vault's start lock,
//!    starts `helios-node --vault FILE --idle-exit SECONDS` on it, waits
//!    for its ready line and its rendezvous file, then connects. A client
//!    racing it waits on the same lock and finds the node the first one
//!    started. The node exits once idle (no client, no job).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use helios_inspect::client::Client;
use helios_inspect::discovery::{self, Lock, Rendezvous};

/// How long a node started for a client waits, idle, before it exits.
pub const IDLE_EXIT: Duration = Duration::from_mins(1);

/// The environment variable that changes [`IDLE_EXIT`], in seconds.
pub const IDLE_ENV: &str = "HELIOS_IDLE_EXIT";

/// The environment variable holding the session token of a TCP endpoint
/// named by `--node` or `HELIOS_NODE`.
pub const TOKEN_ENV: &str = "HELIOS_TOKEN";

/// How long a node may take to start.
const START_TIMEOUT: Duration = Duration::from_mins(1);

/// A client connected to a vault's node.
pub struct Connected {
    pub client: Client,
    /// The node's rendezvous, when found through the vault.
    pub rendezvous: Option<Rendezvous>,
    /// Whether this client started the node.
    pub started: bool,
}

/// The endpoint `--node` names, or else `HELIOS_NODE`.
pub fn explicit(flag: Option<&str>) -> Option<String> {
    flag.map(str::to_string)
        .or_else(|| std::env::var(discovery::ENV).ok())
        .filter(|a| !a.is_empty())
}

/// The idle time of a node started for a client: [`IDLE_EXIT`], or
/// `HELIOS_IDLE_EXIT` seconds.
pub fn idle_exit() -> Duration {
    std::env::var(IDLE_ENV)
        .ok()
        .and_then(|s| s.parse().ok())
        .map_or(IDLE_EXIT, Duration::from_secs)
}

/// Connects to the node of `vault` (or to `node`, when named), starting
/// `node_bin` on the vault when none holds it. Calls are made by `origin`.
pub fn connect(
    vault: Option<&Path>,
    node: Option<&str>,
    node_bin: &Path,
    origin: &str,
) -> Result<Connected, String> {
    if let Some(address) = explicit(node) {
        let address = helios_inspect::Address::parse(&address, std::env::var(TOKEN_ENV).ok())?;
        let client = Client::connect(&address)
            .map_err(|e| format!("cannot reach the node at {address}: {e}"))?;
        return Ok(Connected {
            client: client.with_origin(origin),
            rendezvous: None,
            started: false,
        });
    }
    let vault = vault.ok_or("no vault: give --vault FILE, or --node ADDRESS")?;
    let vault = std::path::absolute(vault).map_err(|e| format!("{}: {e}", vault.display()))?;
    let files = discovery::files(&vault);
    if let Some(found) = reach(&files.rendezvous) {
        return Ok(found.with(origin, false));
    }
    // One client starts the node; the others wait here and find it.
    let _starting =
        Lock::take(&files.start).map_err(|e| format!("{}: {e}", files.start.display()))?;
    if let Some(found) = reach(&files.rendezvous) {
        return Ok(found.with(origin, false));
    }
    let held = Lock::held(&files.lock).map_err(|e| format!("{}: {e}", files.lock.display()))?;
    if held {
        // Its node is starting, or serves no endpoint a client can use.
        let deadline = Instant::now() + START_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(found) = reach(&files.rendezvous) {
                return Ok(found.with(origin, false));
            }
            if !Lock::held(&files.lock).unwrap_or(true) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if Lock::held(&files.lock).unwrap_or(true) {
            return Err(format!(
                "{}: a node holds this vault but serves no command endpoint a client can reach",
                vault.display()
            ));
        }
    }
    // A rendezvous left by a node that was killed.
    let _ = std::fs::remove_file(&files.rendezvous);
    start(&vault, node_bin)?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Some(found) = reach(&files.rendezvous) {
            return Ok(found.with(origin, true));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the node started on {} wrote no rendezvous file",
                vault.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A node found through its rendezvous file.
struct Found {
    client: Client,
    rendezvous: Rendezvous,
}

impl Found {
    fn with(self, origin: &str, started: bool) -> Connected {
        Connected {
            client: self.client.with_origin(origin),
            rendezvous: Some(self.rendezvous),
            started,
        }
    }
}

/// Connects to the node a rendezvous file names, if it answers.
fn reach(file: &Path) -> Option<Found> {
    let rendezvous = Rendezvous::read(file).ok()??;
    let address = rendezvous.address().ok()?;
    let client = Client::connect(&address).ok()?;
    Some(Found { client, rendezvous })
}

/// A private folder for the started node's sockets: Unix socket paths are
/// short, and a vault's folder may be deep.
fn runtime_dir() -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let dir = std::env::temp_dir().join(format!("helios-node-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(dir)
}

/// Starts `node_bin` on `vault`, exiting when idle, and waits for its
/// ready line. Its log goes beside the vault (`.helios/NAME.node.log`).
fn start(vault: &Path, node_bin: &Path) -> Result<(), String> {
    let files = discovery::files(vault);
    let log = files.rendezvous.with_extension("log");
    let dir = runtime_dir()?;
    let tcp = !cfg!(unix);
    let endpoint = |name: &str| -> std::ffi::OsString {
        if tcp {
            "tcp://127.0.0.1:0".into()
        } else {
            dir.join(name).into_os_string()
        }
    };
    let mut command = Command::new(node_bin);
    command
        .arg("--vault")
        .arg(vault)
        .arg("--socket")
        .arg(endpoint("inspect.sock"))
        .arg("--batch-control")
        .arg(endpoint("batch.sock"))
        .arg("--idle-exit")
        .arg(idle_exit().as_secs().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // Not a pipe: nobody reads it once this process has exited.
        .stderr(Stdio::from(
            std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?,
        ));
    if tcp {
        command.arg("--token-file").arg(dir.join("token"));
    }
    // Its own process group: a Ctrl-C in the terminal is for the client,
    // not for the node, which outlives it.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
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
    if !line.starts_with("ready ") {
        let _ = child.wait();
        let said = std::fs::read_to_string(&log).unwrap_or_default();
        return Err(format!("the node did not start: {}", said.trim()));
    }
    // The node outlives this process; it exits once idle.
    std::mem::forget(child);
    Ok(())
}
