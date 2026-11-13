//! The one client of a node: the desktop, the `helios` CLI, `helios-mcp`
//! and tests call a node's methods through a [`Client`], whatever carries
//! them (see `docs/protocol.md`).
//!
//! Two transports, the same calls:
//! - **in process** ([`Client::in_process`]): the call goes straight to the
//!   endpoint's [`Service`], the object a socket's calls reach, so it goes
//!   through the same checks (the command's schema, its origin, the undo
//!   group in `_meta`, the bus event of a change), without a socket;
//! - **socket** ([`Client::connect`]): JSON-RPC 2.0 over a Unix socket, or
//!   loopback TCP behind the session token.
//!
//! [`Client::call`] never blocks: it answers a [`Pending`], polled
//! ([`Pending::poll`]) from a thread that must not wait (an interface's),
//! or waited on ([`Pending::wait`]) from one that may (a command line's).
//! Several calls run at once on one client; each answer reaches its own
//! [`Pending`]. The notifications the node sends on the client's
//! connection (a subscription's `events.event`) reach
//! [`Client::notifications`].

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};

use crate::rpc::{self, Call, Closers, Error, Id, Outcome, Response, Service, Sink, code};
use crate::transport::{Address, Client as Connection, Closer};

/// What a call says about itself, sent as its params' `_meta`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallMeta {
    /// Who runs it: `user`, `cli`, `control`, `agent:NAME`, `script:NAME`.
    pub origin: Option<String>,
    /// Steps undone together carry the same group.
    pub undo_group: Option<String>,
}

impl CallMeta {
    /// Calls made by `origin`.
    pub fn origin(origin: &str) -> Self {
        Self {
            origin: Some(origin.into()),
            undo_group: None,
        }
    }

    /// `params` with this meta as its `_meta`, unless it has one.
    fn into_params(self, params: Value) -> Value {
        let mut params = match params {
            Value::Object(fields) => fields,
            Value::Null => serde_json::Map::new(),
            other => return other,
        };
        if !params.contains_key("_meta") && (self.origin.is_some() || self.undo_group.is_some()) {
            let mut meta = serde_json::Map::new();
            if let Some(origin) = self.origin {
                meta.insert("origin".into(), json!(origin));
            }
            if let Some(group) = self.undo_group {
                meta.insert("undo_group".into(), json!(group));
            }
            params.insert("_meta".into(), Value::Object(meta));
        }
        Value::Object(params)
    }
}

/// A call's answer, still to come.
pub struct Pending {
    id: i64,
    answer: Receiver<Result<Value, Error>>,
}

impl Pending {
    /// The call's id, for [`Client::cancel`].
    pub fn id(&self) -> i64 {
        self.id
    }

    /// The answer if it came, without waiting: `None` while the call runs.
    pub fn poll(&self) -> Option<Result<Value, Error>> {
        match self.answer.try_recv() {
            Ok(answer) => Some(answer),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(gone())),
        }
    }

    /// Waits for the answer.
    pub fn wait(self) -> Result<Value, Error> {
        self.answer.recv().unwrap_or_else(|_| Err(gone()))
    }

    /// Waits for the answer at most `timeout`: `None` if it has not come.
    pub fn wait_timeout(&self, timeout: Duration) -> Option<Result<Value, Error>> {
        match self.answer.recv_timeout(timeout) {
            Ok(answer) => Some(answer),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Err(gone())),
        }
    }
}

/// The error of a call whose connection went before it was answered.
fn gone() -> Error {
    Error::new(
        code::INTERNAL_ERROR,
        "the connection to the node closed before the call was answered",
    )
}

/// Where the notifications a client receives go, once someone listens.
type Notify = Arc<Mutex<Option<Sender<Value>>>>;

/// Hands a notification to whoever listens; false once the listener
/// went (no listener yet is not a failure: the notification is dropped).
fn forward(notify: &Notify, value: Value) -> bool {
    let mut to = notify.lock().unwrap_or_else(PoisonError::into_inner);
    if to.as_ref().is_some_and(|s| s.send(value).is_err()) {
        *to = None;
        return false;
    }
    true
}

/// The lines a call of this process writes as notifications, each handed
/// to the client's listener.
struct Lines {
    line: Vec<u8>,
    notify: Notify,
}

impl Write for Lines {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for &b in bytes {
            if b == b'\n' {
                let line = std::mem::take(&mut self.line);
                if let Ok(value) = serde_json::from_slice::<Value>(&line)
                    && !forward(&self.notify, value)
                {
                    // A stream sending to nobody stops.
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
            } else {
                self.line.push(b);
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What carries a [`Client`]'s calls.
trait Transport: Send + Sync {
    /// Sends call `id`; its answer goes to `answer`.
    fn call(&self, id: i64, method: &str, params: Value, answer: Sender<Result<Value, Error>>);
    /// Asks call `id` to stop.
    fn cancel(&self, id: i64);
}

/// A client of one node.
pub struct Client {
    transport: Arc<dyn Transport>,
    next: AtomicI64,
    meta: CallMeta,
    notify: Notify,
}

impl Client {
    /// A client of the endpoint `service` answers in this process: each
    /// call runs on a thread of its own, as a socket's do.
    pub fn in_process(service: Arc<dyn Service>) -> Self {
        let notify = Notify::default();
        Self::over(
            Arc::new(InProcess {
                service,
                running: Arc::default(),
                closers: Closers::default(),
                notify: notify.clone(),
            }),
            notify,
        )
    }

    /// A client of the endpoint at `address`, its connection kept open
    /// until the client is dropped.
    pub fn connect(address: &Address) -> io::Result<Self> {
        let notify = Notify::default();
        Ok(Self::over(
            Arc::new(Socket::connect(address, notify.clone())?),
            notify,
        ))
    }

    fn over(transport: Arc<dyn Transport>, notify: Notify) -> Self {
        Self {
            transport,
            next: AtomicI64::new(1),
            meta: CallMeta::default(),
            notify,
        }
    }

    /// The notifications the node sends this client from now on (each a
    /// JSON-RPC notification: `method` and `params`), such as a
    /// subscription's events. One listener at a time: a second call
    /// takes them over.
    pub fn notifications(&self) -> Receiver<Value> {
        let (send, receive) = mpsc::channel();
        *self.notify.lock().unwrap_or_else(PoisonError::into_inner) = Some(send);
        receive
    }

    /// Its calls made by `origin` (`_meta.origin`), unless a call names
    /// its own.
    pub fn with_origin(mut self, origin: &str) -> Self {
        self.meta.origin = Some(origin.into());
        self
    }

    /// Calls `method` with `params` (an object) and the client's meta;
    /// returns at once.
    pub fn call(&self, method: &str, params: Value) -> Pending {
        self.call_with(method, params, self.meta.clone())
    }

    /// As [`Client::call`], with this call's own meta.
    pub fn call_with(&self, method: &str, params: Value, meta: CallMeta) -> Pending {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (send, answer) = mpsc::channel();
        self.transport
            .call(id, method, meta.into_params(params), send);
        Pending { id, answer }
    }

    /// Calls `method` and waits for its answer: for a thread that may wait.
    pub fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        self.call(method, params).wait()
    }

    /// Asks a call still running to stop (`$/cancel`); a call that can
    /// stop answers [`code::CANCELLED`].
    pub fn cancel(&self, pending: &Pending) {
        self.transport.cancel(pending.id);
    }

    /// The node's `node.hello`: its protocol, instance, endpoint and what
    /// this client may do there.
    pub fn hello(&self) -> Result<Value, Error> {
        self.request("node.hello", json!({}))
    }
}

/// Calls answered by a [`Service`] of this process.
struct InProcess {
    service: Arc<dyn Service>,
    running: Arc<Mutex<HashMap<i64, Arc<AtomicBool>>>>,
    /// What calls asked to run when their client goes.
    closers: Closers,
    notify: Notify,
}

impl Transport for InProcess {
    fn call(&self, id: i64, method: &str, params: Value, answer: Sender<Result<Value, Error>>) {
        let cancelled = Arc::new(AtomicBool::new(false));
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, cancelled.clone());
        // Notifications sent on the call's connection reach the client's
        // listener, as a socket's do.
        let call = Call::detached(
            Some(Id::Number(id)),
            cancelled,
            Sink::new(Lines {
                line: Vec::new(),
                notify: self.notify.clone(),
            }),
            self.closers.clone(),
        );
        let (service, method, running) = (
            self.service.clone(),
            method.to_string(),
            self.running.clone(),
        );
        let params = match params {
            Value::Null => json!({}),
            other => other,
        };
        std::thread::spawn(move || {
            let outcome = service.call(&method, params, &call);
            running
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            let _ = answer.send(outcome);
            call.run_after();
        });
    }

    fn cancel(&self, id: i64) {
        if let Some(flag) = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
        {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

impl Drop for InProcess {
    fn drop(&mut self) {
        let closers =
            std::mem::take(&mut *self.closers.lock().unwrap_or_else(PoisonError::into_inner));
        for close in closers {
            close();
        }
    }
}

/// The calls waiting for their answer on a socket, by id.
type Waiting = Arc<Mutex<Option<HashMap<i64, Sender<Result<Value, Error>>>>>>;

/// Calls carried as JSON-RPC lines on one connection; a thread reads the
/// answers and hands each to its call.
struct Socket {
    writer: Mutex<Box<dyn Write + Send>>,
    waiting: Waiting,
    closer: Closer,
}

impl Socket {
    fn connect(address: &Address, notify: Notify) -> io::Result<Self> {
        let (reader, writer, closer) = Connection::connect(address)?.split()?;
        let waiting: Waiting = Arc::new(Mutex::new(Some(HashMap::new())));
        let read = waiting.clone();
        std::thread::Builder::new()
            .name("helios-client".into())
            .spawn(move || read_answers(reader, &read, &notify))?;
        Ok(Self {
            writer: Mutex::new(writer),
            waiting,
            closer,
        })
    }

    fn send(&self, value: &Value) -> io::Result<()> {
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        writeln!(writer, "{value}")?;
        writer.flush()
    }
}

/// Hands each answer read to its call, and each notification to the
/// client's listener; once the connection closes, every call still
/// waiting is told.
fn read_answers(mut reader: Box<dyn BufRead + Send>, waiting: &Waiting, notify: &Notify) {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("id").is_none() && value.get("method").is_some() {
            let _ = forward(notify, value);
            continue;
        }
        // Lines of another form are not answers.
        let Ok(response) = serde_json::from_value::<Response>(value) else {
            continue;
        };
        let Id::Number(id) = response.id else {
            continue;
        };
        let sender = waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
            .and_then(|w| w.remove(&id));
        if let Some(sender) = sender {
            let _ = sender.send(match response.outcome {
                Outcome::Result(value) => Ok(value),
                Outcome::Error(error) => Err(error),
            });
        }
    }
    // Dropping the senders tells their calls the connection went.
    waiting
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
}

impl Transport for Socket {
    fn call(&self, id: i64, method: &str, params: Value, answer: Sender<Result<Value, Error>>) {
        {
            let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(waiting) = waiting.as_mut() else {
                let _ = answer.send(Err(gone()));
                return;
            };
            waiting.insert(id, answer);
        }
        let request = serde_json::to_value(rpc::Request::call(id, method, params))
            .expect("requests serialize");
        if let Err(e) = self.send(&request) {
            let sender = self
                .waiting
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_mut()
                .and_then(|w| w.remove(&id));
            if let Some(sender) = sender {
                let _ = sender.send(Err(Error::new(
                    code::INTERNAL_ERROR,
                    format!("cannot send the call to the node: {e}"),
                )));
            }
        }
    }

    fn cancel(&self, id: i64) {
        // A notification: nothing answers it.
        let _ = self.send(&json!({
            "jsonrpc": rpc::JSONRPC, "method": rpc::CANCEL, "params": { "id": id },
        }));
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.closer.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Echoes its params; `slow` waits until cancelled.
    struct Echo;

    impl Service for Echo {
        fn call(&self, method: &str, params: Value, call: &Call) -> Result<Value, Error> {
            match method {
                "echo" => Ok(params),
                "slow" => {
                    while !call.is_cancelled() {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(Error::cancelled())
                }
                other => Err(Error::method_not_found(other)),
            }
        }

        fn cancellable(&self, method: &str) -> bool {
            method == "slow"
        }
    }

    #[test]
    fn in_process_calls_carry_the_meta_and_can_be_cancelled() {
        let client = Client::in_process(Arc::new(Echo)).with_origin("cli");
        let echoed = client.request("echo", json!({ "a": 1 })).unwrap();
        assert_eq!(echoed, json!({ "a": 1, "_meta": { "origin": "cli" } }));
        let slow = client.call("slow", json!({}));
        assert!(slow.poll().is_none(), "the call does not block");
        client.cancel(&slow);
        assert_eq!(slow.wait().unwrap_err().code, code::CANCELLED);
        assert_eq!(
            client.request("nope", json!({})).unwrap_err().code,
            code::METHOD_NOT_FOUND
        );
    }

    #[cfg(unix)]
    #[test]
    fn socket_calls_are_matched_to_their_answers() {
        let dir = std::env::temp_dir().join(format!("helios-client-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rpc.sock");
        crate::serve_rpc_at(
            &crate::Listen::Unix(path.clone()),
            None,
            |_: Value, _: &mut crate::Connection<'_>| None,
            Arc::new(Echo),
        )
        .unwrap();
        let client = Client::connect(&Address::Unix(path)).unwrap();
        let slow = client.call("slow", json!({}));
        let fast = client.call_with("echo", json!({ "b": 2 }), CallMeta::origin("agent:x"));
        assert_eq!(
            fast.wait().unwrap(),
            json!({ "b": 2, "_meta": { "origin": "agent:x" } })
        );
        client.cancel(&slow);
        assert_eq!(slow.wait().unwrap_err().code, code::CANCELLED);
        let _ = std::fs::remove_dir_all(dir);
    }
}
