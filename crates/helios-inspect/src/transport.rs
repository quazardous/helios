//! Where an endpoint listens and how a client reaches it.
//!
//! Two transports carry the same JSON lines:
//! - a **Unix socket** (a path): reachable only through the file system;
//! - **loopback TCP** (`tcp://127.0.0.1:PORT`), for systems without Unix
//!   sockets and for paths too long for one. Only loopback addresses are
//!   accepted, and every connection must first present the session token
//!   (`{"auth": "<token>"}`), which the server writes to a file readable by
//!   its user only.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// Where a server listens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listen {
    Unix(PathBuf),
    /// Port 0 picks a free port.
    Tcp(SocketAddr),
}

/// How a client reaches a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Address {
    Unix(PathBuf),
    Tcp { addr: SocketAddr, token: String },
}

const TCP: &str = "tcp://";

impl Listen {
    /// A path, or `tcp://HOST:PORT` with a loopback host.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.strip_prefix(TCP) {
            None => Ok(Listen::Unix(PathBuf::from(text))),
            Some(rest) => {
                let addr: SocketAddr = rest
                    .parse()
                    .map_err(|_| format!("`{text}` is not tcp://HOST:PORT"))?;
                if !addr.ip().is_loopback() {
                    return Err(format!("`{text}`: only loopback addresses are served"));
                }
                Ok(Listen::Tcp(addr))
            }
        }
    }
}

impl Address {
    /// A path, or `tcp://HOST:PORT`, which needs the session token.
    pub fn parse(text: &str, token: Option<String>) -> Result<Self, String> {
        match text.strip_prefix(TCP) {
            None => Ok(Address::Unix(PathBuf::from(text))),
            Some(rest) => Ok(Address::Tcp {
                addr: rest
                    .parse()
                    .map_err(|_| format!("`{text}` is not tcp://HOST:PORT"))?,
                token: token.ok_or(format!(
                    "`{text}` needs the session token (--token-file or HELIOS_TOKEN)"
                ))?,
            }),
        }
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Address::Unix(path) => write!(f, "{}", path.display()),
            Address::Tcp { addr, .. } => write!(f, "{TCP}{addr}"),
        }
    }
}

/// A server bound and serving.
#[derive(Clone, Debug)]
pub struct Bound {
    /// What a client passes to [`Address::parse`].
    pub address: String,
}

/// A fresh session token: 256 bits from the standard library's randomly
/// keyed hashers (seeded by the operating system), as hex.
pub fn new_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    (0..4u8)
        .map(|i| {
            let mut h = RandomState::new().build_hasher();
            h.write_u8(i);
            format!("{:016x}", h.finish())
        })
        .collect()
}

/// Writes a token where only the current user can read it.
pub fn write_token(path: &std::path::Path, token: &str) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(token.as_bytes())
}

/// Serves JSON lines: each line parsed as an `R` and answered by `handler`.
/// TCP needs `token`, which every connection presents first.
pub fn serve_at<R, H>(listen: &Listen, token: Option<&str>, handler: H) -> io::Result<Bound>
where
    R: DeserializeOwned,
    H: Fn(R) -> Value + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    match listen {
        #[cfg(unix)]
        Listen::Unix(path) => {
            use std::os::unix::net::UnixListener;
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            let listener = UnixListener::bind(path)?;
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let handler = handler.clone();
                    thread::spawn(move || {
                        if let Ok(reader) = stream.try_clone() {
                            converse(reader, stream, None, &*handler);
                        }
                    });
                }
            });
            Ok(Bound {
                address: path.display().to_string(),
            })
        }
        #[cfg(not(unix))]
        Listen::Unix(_) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Unix sockets are not available here: use tcp://127.0.0.1:0",
        )),
        Listen::Tcp(addr) => {
            let token = token
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "a TCP endpoint needs a session token",
                    )
                })?
                .to_string();
            let listener = TcpListener::bind(addr)?;
            let bound = listener.local_addr()?;
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (handler, token) = (handler.clone(), token.clone());
                    thread::spawn(move || {
                        if let Ok(reader) = stream.try_clone() {
                            converse(reader, stream, Some(&token), &*handler);
                        }
                    });
                }
            });
            Ok(Bound {
                address: format!("{TCP}{bound}"),
            })
        }
    }
}

/// One connection: an optional authentication line, then requests.
fn converse<R, H>(reader: impl Read, mut writer: impl Write, token: Option<&str>, handler: &H)
where
    R: DeserializeOwned,
    H: Fn(R) -> Value,
{
    let mut lines = BufReader::new(reader).lines();
    if let Some(token) = token {
        let presented = lines
            .next()
            .and_then(Result::ok)
            .and_then(|l| serde_json::from_str::<Value>(&l).ok())
            .and_then(|v| v["auth"].as_str().map(str::to_string));
        if presented.as_deref() != Some(token) {
            let _ = writeln!(
                writer,
                "{}",
                json!({ "ok": false, "error": "authentication failed: wrong or missing session token" })
            );
            return;
        }
        if writeln!(writer, "{}", json!({ "ok": true })).is_err() {
            return;
        }
    }
    for line in lines {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<R>(&line) {
            Ok(request) => handler(request),
            Err(e) => json!({ "ok": false, "error": format!("bad request: {e}") }),
        };
        if writeln!(writer, "{response}").is_err() {
            return;
        }
    }
}

/// Sends one request and reads the response.
pub fn request_at<R: Serialize>(address: &Address, request: &R) -> io::Result<Value> {
    let line = serde_json::to_string(request).expect("requests always serialize");
    match address {
        #[cfg(unix)]
        Address::Unix(path) => {
            let mut stream = std::os::unix::net::UnixStream::connect(path)?;
            writeln!(stream, "{line}")?;
            read_answer(stream)
        }
        #[cfg(not(unix))]
        Address::Unix(_) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Unix sockets are not available here",
        )),
        Address::Tcp { addr, token } => {
            let mut stream = TcpStream::connect(addr)?;
            writeln!(stream, "{}", json!({ "auth": token }))?;
            let mut reader = BufReader::new(stream.try_clone()?);
            let mut first = String::new();
            reader.read_line(&mut first)?;
            let auth: Value = serde_json::from_str(&first)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            if auth["ok"] != true {
                return Ok(auth);
            }
            writeln!(stream, "{line}")?;
            let mut answer = String::new();
            reader.read_line(&mut answer)?;
            serde_json::from_str(&answer).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }
    }
}

fn read_answer(stream: impl Read) -> io::Result<Value> {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
