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
use std::sync::{Arc, PoisonError};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::rpc::{self, Closers, Running, Service, Sink, dispatch};

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
    use std::fmt::Write as _;
    use std::hash::{BuildHasher, Hasher};
    (0..4u8).fold(String::new(), |mut token, i| {
        let mut h = RandomState::new().build_hasher();
        h.write_u8(i);
        let _ = write!(token, "{:016x}", h.finish());
        token
    })
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

/// One client's connection, as a handler sees it: a request may answer
/// with more than one line ([`Connection::send`], a stream), and may ask
/// to be told when the client goes ([`Connection::on_close`]).
pub struct Connection<'a> {
    out: &'a Sink,
    closers: &'a Closers,
}

impl Connection<'_> {
    /// Sends one line now; fails once the client has gone.
    pub fn send(&mut self, value: &Value) -> io::Result<()> {
        self.out.send(value)
    }

    /// Runs `f` once the connection closes: the client hung up, or its
    /// process died.
    pub fn on_close(&mut self, f: impl FnOnce() + Send + 'static) {
        self.closers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::new(f));
    }
}

/// Serves JSON lines: each line parsed as an `R` and answered by `handler`.
/// TCP needs `token`, which every connection presents first.
pub fn serve_at<R, H>(listen: &Listen, token: Option<&str>, handler: H) -> io::Result<Bound>
where
    R: DeserializeOwned,
    H: Fn(R) -> Value + Send + Sync + 'static,
{
    serve_connections_at(listen, token, move |request: R, _: &mut Connection<'_>| {
        Some(handler(request))
    })
}

/// As [`serve_at`], the handler seeing the connection: it answers with
/// the line it returns, or sends its own lines and returns `None`.
pub fn serve_connections_at<R, H>(
    listen: &Listen,
    token: Option<&str>,
    handler: H,
) -> io::Result<Bound>
where
    R: DeserializeOwned,
    H: Fn(R, &mut Connection<'_>) -> Option<Value> + Send + Sync + 'static,
{
    serve_both(listen, token, handler, None)
}

/// As [`serve_connections_at`], a line with `"jsonrpc": "2.0"` answered
/// by `service` ([`crate::rpc`]), any other line by `handler` as before.
pub fn serve_rpc_at<R, H>(
    listen: &Listen,
    token: Option<&str>,
    handler: H,
    service: Arc<dyn Service>,
) -> io::Result<Bound>
where
    R: DeserializeOwned,
    H: Fn(R, &mut Connection<'_>) -> Option<Value> + Send + Sync + 'static,
{
    serve_both(listen, token, handler, Some(service))
}

fn serve_both<R, H>(
    listen: &Listen,
    token: Option<&str>,
    handler: H,
    service: Option<Arc<dyn Service>>,
) -> io::Result<Bound>
where
    R: DeserializeOwned,
    H: Fn(R, &mut Connection<'_>) -> Option<Value> + Send + Sync + 'static,
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
                    let (handler, service) = (handler.clone(), service.clone());
                    thread::spawn(move || {
                        if let Ok(reader) = stream.try_clone() {
                            converse(reader, stream, None, &*handler, service);
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
                    let service = service.clone();
                    thread::spawn(move || {
                        if let Ok(reader) = stream.try_clone() {
                            converse(reader, stream, Some(&token), &*handler, service);
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

/// The connections this process serves, open now, and when one last
/// sent a line or closed: what a node that exits when idle looks at.
static OPEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// How busy this process's endpoints are.
#[derive(Clone, Copy, Debug)]
pub struct Activity {
    /// Connections open on any endpoint.
    pub open: usize,
    /// When a connection last sent a line or closed; `None` before any.
    pub last: Option<std::time::Instant>,
}

/// This process's endpoints' [`Activity`].
pub fn activity() -> Activity {
    Activity {
        open: OPEN.load(std::sync::atomic::Ordering::SeqCst),
        last: *LAST.lock().unwrap_or_else(PoisonError::into_inner),
    }
}

fn touch() {
    *LAST.lock().unwrap_or_else(PoisonError::into_inner) = Some(std::time::Instant::now());
}

/// Counts a connection open while it lives.
struct Open;

impl Open {
    fn new() -> Self {
        OPEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        touch();
        Open
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        touch();
        OPEN.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// One connection: an optional authentication line, then requests; what
/// the handler asked to run at the end runs once the client has gone.
fn converse<R, H>(
    reader: impl Read,
    mut writer: impl Write + Send + 'static,
    token: Option<&str>,
    handler: &H,
    service: Option<Arc<dyn Service>>,
) where
    R: DeserializeOwned,
    H: Fn(R, &mut Connection<'_>) -> Option<Value>,
{
    let _open = Open::new();
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
    let sink = Sink::new(writer);
    let closers: Closers = Arc::default();
    let running = Running::default();
    for line in lines {
        let Ok(line) = line else { break };
        touch();
        if line.trim().is_empty() {
            continue;
        }
        let parsed = serde_json::from_str::<Value>(&line);
        // A JSON-RPC line: a batch, or an object naming `jsonrpc`; a line
        // that is not JSON is answered in the form it seems to use.
        let rpc = match &parsed {
            Ok(Value::Array(_)) => true,
            Ok(value) => value.get("jsonrpc").is_some(),
            Err(_) => line.contains("\"jsonrpc\""),
        };
        if let (true, Some(service)) = (rpc, &service) {
            match parsed {
                Ok(value) => dispatch(value, service, &sink, &closers, &running),
                Err(e) => {
                    let error = rpc::Error::new(rpc::code::PARSE_ERROR, e.to_string());
                    if sink
                        .send(&rpc::Response::new(rpc::Id::Null, Err(error)))
                        .is_err()
                    {
                        break;
                    }
                }
            }
            continue;
        }
        let mut connection = Connection {
            out: &sink,
            closers: &closers,
        };
        let response = match parsed.and_then(serde_json::from_value::<R>) {
            Ok(request) => handler(request, &mut connection),
            Err(e) => Some(json!({ "ok": false, "error": format!("bad request: {e}") })),
        };
        if let Some(response) = response
            && sink.send(&response).is_err()
        {
            break;
        }
    }
    let closers = std::mem::take(&mut *closers.lock().unwrap_or_else(PoisonError::into_inner));
    for close in closers {
        close();
    }
}

/// Sends one request and reads the response.
pub fn request_at<R: Serialize>(address: &Address, request: &R) -> io::Result<Value> {
    request_timed(address, request, None)
}

/// As [`request_at`], giving up (`TimedOut` or `WouldBlock`) when the
/// server takes longer than `timeout` to read the request or to answer.
pub fn request_within<R: Serialize>(
    address: &Address,
    request: &R,
    timeout: Duration,
) -> io::Result<Value> {
    request_timed(address, request, Some(timeout))
}

fn request_timed<R: Serialize>(
    address: &Address,
    request: &R,
    timeout: Option<Duration>,
) -> io::Result<Value> {
    let line = serde_json::to_string(request).expect("requests always serialize");
    match address {
        #[cfg(unix)]
        Address::Unix(path) => {
            let mut stream = std::os::unix::net::UnixStream::connect(path)?;
            stream.set_read_timeout(timeout)?;
            stream.set_write_timeout(timeout)?;
            writeln!(stream, "{line}")?;
            read_answer(stream)
        }
        #[cfg(not(unix))]
        Address::Unix(_) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Unix sockets are not available here",
        )),
        Address::Tcp { addr, token } => {
            let mut stream = match timeout {
                Some(t) => TcpStream::connect_timeout(addr, t)?,
                None => TcpStream::connect(addr)?,
            };
            stream.set_read_timeout(timeout)?;
            stream.set_write_timeout(timeout)?;
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

/// A connection kept open: requests in turn on it, or a stream read line
/// by line ([`crate::subscribe`]). A server that ties something to the
/// connection (a batch held by its client) sees it close when this is
/// dropped, or when the process dies.
pub struct Client {
    reader: BufReader<Stream>,
    writer: Stream,
}

impl Client {
    /// Connects, presenting the session token over TCP.
    pub fn connect(address: &Address) -> io::Result<Client> {
        let stream = match address {
            #[cfg(unix)]
            Address::Unix(path) => Stream::Unix(std::os::unix::net::UnixStream::connect(path)?),
            #[cfg(not(unix))]
            Address::Unix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Unix sockets are not available here",
                ));
            }
            Address::Tcp { addr, .. } => Stream::Tcp(TcpStream::connect(addr)?),
        };
        let mut client = Client {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
        };
        if let Address::Tcp { token, .. } = address {
            client.send(&json!({ "auth": token }))?;
            let auth = client.read()?;
            if auth["ok"] != true {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    auth["error"]
                        .as_str()
                        .unwrap_or("authentication failed")
                        .to_string(),
                ));
            }
        }
        Ok(client)
    }

    /// Sends one request and reads its answer.
    pub fn request<R: Serialize>(&mut self, request: &R) -> io::Result<Value> {
        self.send(request)?;
        self.read()
    }

    /// Sends one line.
    pub fn send<R: Serialize>(&mut self, request: &R) -> io::Result<()> {
        let line = serde_json::to_string(request).expect("requests always serialize");
        writeln!(self.writer, "{line}")?;
        self.writer.flush()
    }

    /// Reads the next line; the server closing the connection is an
    /// `UnexpectedEof` error, a read timeout a `WouldBlock` or `TimedOut`.
    pub fn read(&mut self) -> io::Result<Value> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the connection closed",
            ));
        }
        serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// How long [`Client::read`] waits for a line (`None`: for ever).
    pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> io::Result<()> {
        self.writer.set_read_timeout(timeout)
    }

    /// Closes the connection from another thread: a read blocked on it
    /// returns.
    pub fn closer(&self) -> io::Result<Closer> {
        Ok(Closer(self.writer.try_clone()?))
    }
}

impl Client {
    /// Its reading half and its writing half, for a client that reads on
    /// one thread and writes from others ([`crate::client`]).
    #[allow(
        clippy::type_complexity,
        reason = "the three halves, named by their use"
    )]
    pub(crate) fn split(
        self,
    ) -> io::Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>, Closer)> {
        let closer = Closer(self.writer.try_clone()?);
        Ok((Box::new(self.reader), Box::new(self.writer), closer))
    }
}

/// Closes a [`Client`]'s connection from elsewhere.
pub struct Closer(Stream);

impl Closer {
    pub fn close(&self) {
        self.0.shutdown();
    }
}

enum Stream {
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
    Tcp(TcpStream),
}

impl Stream {
    fn try_clone(&self) -> io::Result<Stream> {
        Ok(match self {
            #[cfg(unix)]
            Stream::Unix(s) => Stream::Unix(s.try_clone()?),
            Stream::Tcp(s) => Stream::Tcp(s.try_clone()?),
        })
    }

    fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.set_read_timeout(timeout),
            Stream::Tcp(s) => s.set_read_timeout(timeout),
        }
    }

    fn shutdown(&self) {
        let _ = match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.shutdown(std::net::Shutdown::Both),
            Stream::Tcp(s) => s.shutdown(std::net::Shutdown::Both),
        };
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.read(buf),
            Stream::Tcp(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.write(buf),
            Stream::Tcp(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.flush(),
            Stream::Tcp(s) => s.flush(),
        }
    }
}

fn read_answer(stream: impl Read) -> io::Result<Value> {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
