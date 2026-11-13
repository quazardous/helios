//! How a client finds the node holding a vault (see `docs/protocol.md`).
//!
//! One node owns a vault at a time: it holds the vault's lock, an OS file
//! lock released when its process ends, even killed. While it serves, it
//! keeps a rendezvous file beside the vault, in the `.helios` folder next
//! to the vault file, named after it (`library.db` has
//! `.helios/library.db.node.json`): its endpoint's address, where its
//! session token is, its instance and protocol version. It writes the file
//! once it serves, under the lock, and removes it as it stops.
//!
//! A client reads the file and connects. A file whose node is gone (killed
//! before removing it) is told apart by the lock: nobody holds it.
//! `HELIOS_NODE` (or a command's `--node`) names an endpoint directly and
//! bypasses the file.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::transport::Address;

/// The environment variable naming a node's endpoint, bypassing discovery.
pub const ENV: &str = "HELIOS_NODE";

/// The folder beside a vault where its node keeps its files.
pub const DIR: &str = ".helios";

/// The files of a vault's node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Files {
    /// The rendezvous file ([`Rendezvous`]).
    pub rendezvous: PathBuf,
    /// The vault's lock: held by the node owning the vault.
    pub lock: PathBuf,
    /// Held by a client while it starts a node, so two clients racing
    /// start one.
    pub start: PathBuf,
}

/// The node files of the vault at `vault`.
pub fn files(vault: &Path) -> Files {
    let vault = std::path::absolute(vault).unwrap_or_else(|_| vault.to_path_buf());
    let dir = vault
        .parent()
        .map_or_else(|| PathBuf::from(DIR), |d| d.join(DIR));
    let name = vault
        .file_name()
        .map_or_else(|| "vault".into(), |n| n.to_string_lossy().into_owned());
    Files {
        rendezvous: dir.join(format!("{name}.node.json")),
        lock: dir.join(format!("{name}.node.lock")),
        start: dir.join(format!("{name}.start.lock")),
    }
}

/// What a node serving a vault tells its clients.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rendezvous {
    /// The endpoint running the vault's commands (the batch endpoint): a
    /// Unix socket path or `tcp://HOST:PORT`.
    pub address: String,
    /// The read-only inspect endpoint (state, events).
    pub inspect: String,
    /// Where the session token is, for a TCP endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_file: Option<PathBuf>,
    /// The node instance, as `node.hello` names it.
    pub instance: String,
    /// The protocol version it speaks.
    pub protocol: String,
    /// Its process.
    pub pid: u32,
    /// The vault it holds.
    pub vault: PathBuf,
}

impl Rendezvous {
    /// Writes the file whole (a reader never sees half of it).
    pub fn write(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let partial = path.with_extension("json.partial");
        std::fs::write(
            &partial,
            serde_json::to_vec_pretty(self).map_err(io::Error::other)?,
        )?;
        std::fs::rename(&partial, path)
    }

    /// Reads the file; `None` when there is none.
    pub fn read(path: &Path) -> io::Result<Option<Self>> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The command endpoint's address, its token read when it needs one.
    pub fn address(&self) -> Result<Address, String> {
        endpoint(&self.address, self.token_file.as_deref())
    }

    /// The inspect endpoint's address.
    pub fn inspect_address(&self) -> Result<Address, String> {
        endpoint(&self.inspect, self.token_file.as_deref())
    }
}

/// An endpoint's address, a TCP one with the token read from `token_file`.
pub fn endpoint(address: &str, token_file: Option<&Path>) -> Result<Address, String> {
    let token = match token_file {
        Some(file) if address.starts_with("tcp://") => Some(
            std::fs::read_to_string(file)
                .map_err(|e| format!("{}: {e}", file.display()))?
                .trim()
                .to_string(),
        ),
        _ => None,
    };
    Address::parse(address, token)
}

/// An OS file lock, released when dropped or when the process ends.
#[derive(Debug)]
pub struct Lock {
    /// Held open: closing it releases the lock.
    _file: File,
}

impl Lock {
    fn open(path: &Path) -> io::Result<File> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
    }

    /// Takes the lock at `path` if nobody holds it.
    pub fn try_take(path: &Path) -> io::Result<Option<Self>> {
        let file = Self::open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(e),
        }
    }

    /// Takes the lock at `path`, waiting for whoever holds it.
    pub fn take(path: &Path) -> io::Result<Self> {
        let file = Self::open(path)?;
        file.lock()?;
        Ok(Self { _file: file })
    }

    /// Whether someone holds the lock at `path` (a node owns the vault).
    pub fn held(path: &Path) -> io::Result<bool> {
        if !path.exists() {
            return Ok(false);
        }
        Ok(Self::try_take(path)?.is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_sit_beside_the_vault_named_after_it() {
        let files = files(Path::new("/photos/library.db"));
        assert_eq!(
            files.rendezvous,
            Path::new("/photos/.helios/library.db.node.json")
        );
        assert_eq!(
            files.lock,
            Path::new("/photos/.helios/library.db.node.lock")
        );
    }

    #[test]
    fn a_rendezvous_round_trips_and_a_lock_is_held_once() {
        let dir = std::env::temp_dir().join(format!("helios-discovery-{}", std::process::id()));
        let files = files(&dir.join("v.db"));
        let rendezvous = Rendezvous {
            address: "/run/batch.sock".into(),
            inspect: "/run/inspect.sock".into(),
            token_file: None,
            instance: "i-1".into(),
            protocol: "1.0.0".into(),
            pid: 7,
            vault: dir.join("v.db"),
        };
        assert_eq!(Rendezvous::read(&files.rendezvous).unwrap(), None);
        rendezvous.write(&files.rendezvous).unwrap();
        assert_eq!(
            Rendezvous::read(&files.rendezvous).unwrap(),
            Some(rendezvous)
        );
        assert!(!Lock::held(&files.lock).unwrap());
        let held = Lock::try_take(&files.lock).unwrap().unwrap();
        assert!(Lock::held(&files.lock).unwrap());
        drop(held);
        assert!(!Lock::held(&files.lock).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }
}
