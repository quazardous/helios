//! JSON-RPC 2.0 on a node's endpoints: the envelope types, the error
//! codes, and what a method sees of its call.
//!
//! One JSON object per line, as the older `{"cmd": …}` form. A line with
//! `"jsonrpc": "2.0"` is JSON-RPC; any other line is the older form,
//! answered as before. Each call runs on a thread of its own, so one
//! connection carries several calls at once and the event notifications
//! of its subscriptions; answers come back in the order they finish,
//! matched to their call by `id`. See `docs/protocol.md`.

use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The JSON-RPC version every message names.
pub const JSONRPC: &str = "2.0";

/// The version of the node's protocol (its methods and their fields),
/// semver: changes are additive within a major version.
pub const PROTOCOL: &str = "1.0.0";

/// The oldest protocol version a node still serves.
pub const OLDEST: &str = "1.0.0";

/// The method that cancels a call still running.
pub const CANCEL: &str = "$/cancel";

/// Error codes: JSON-RPC's own, then Helios's, in the range the
/// specification leaves to implementations (-32000 to -32099), and the
/// request-cancelled code of the Language Server Protocol.
pub mod code {
    /// The line is not JSON.
    pub const PARSE_ERROR: i64 = -32700;
    /// The JSON is not a valid request (or is a batch, not served yet).
    pub const INVALID_REQUEST: i64 = -32600;
    /// No such method on this endpoint.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// The params do not match the method's schema.
    pub const INVALID_PARAMS: i64 = -32602;
    /// The node failed in a way the request did not cause.
    pub const INTERNAL_ERROR: i64 = -32603;
    /// The method ran and refused or failed: its reason in the message.
    pub const REFUSED: i64 = -32000;
    /// The command exists but this endpoint does not run it (a debug-only
    /// command on the batch endpoint).
    pub const DISABLED: i64 = -32001;
    /// The command needs a vault, and the node keeps none.
    pub const NEEDS_VAULT: i64 = -32002;
    /// The command needs a desktop window, and none holds the workspace.
    pub const NEEDS_WINDOW: i64 = -32003;
    /// The origin named for the call is not one.
    pub const ORIGIN_REFUSED: i64 = -32004;
    /// A wait ran out of time; `data` says what happened instead.
    pub const TIMED_OUT: i64 = -32005;
    /// `$/cancel` named a call that cannot be cancelled.
    pub const NOT_CANCELLABLE: i64 = -32006;
    /// The call was cancelled before it ended.
    pub const CANCELLED: i64 = -32800;
}

/// A request's id: a number or a string (`null` is accepted, as the
/// specification allows, and answered as such).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    Text(String),
    Null,
}

/// A call: answered when it has an `id`, a notification when not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// Absent for a notification; `Some(Id::Null)` for `"id": null`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub id: Option<Id>,
}

/// Keeps `"id": null` apart from no id at all.
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Id>, D::Error> {
    Id::deserialize(d).map(Some)
}

impl Request {
    /// A call answered by its `id`.
    pub fn call(id: impl Into<Id>, method: &str, params: Value) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            method: method.into(),
            params: Some(params),
            id: Some(id.into()),
        }
    }

    /// Reads a request from a JSON value, or says why it is not one, with
    /// the id to answer when it could be read.
    pub fn parse(value: Value) -> Result<Self, (Id, Error)> {
        let id = value
            .get("id")
            .and_then(|id| Id::deserialize(id).ok())
            .unwrap_or(Id::Null);
        let request: Request = serde_json::from_value(value)
            .map_err(|e| (id.clone(), Error::new(code::INVALID_REQUEST, e.to_string())))?;
        if request.jsonrpc != JSONRPC {
            return Err((
                id,
                Error::new(code::INVALID_REQUEST, "`jsonrpc` must be \"2.0\""),
            ));
        }
        if request.params.as_ref().is_some_and(|p| !p.is_object()) {
            return Err((
                id,
                Error::new(code::INVALID_PARAMS, "params are an object, by name"),
            ));
        }
        Ok(request)
    }
}

impl From<i64> for Id {
    fn from(n: i64) -> Self {
        Id::Number(n)
    }
}

impl From<&str> for Id {
    fn from(s: &str) -> Self {
        Id::Text(s.into())
    }
}

/// An error object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Error {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl Error {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(code::METHOD_NOT_FOUND, format!("no method `{method}`"))
    }

    pub fn invalid_params(error: impl std::fmt::Display) -> Self {
        Self::new(code::INVALID_PARAMS, format!("invalid params: {error}"))
    }

    pub fn cancelled() -> Self {
        Self::new(code::CANCELLED, "cancelled")
    }
}

/// What a call came to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    #[serde(rename = "result")]
    Result(Value),
    #[serde(rename = "error")]
    Error(Error),
}

/// The answer to a call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Id,
    #[serde(flatten)]
    pub outcome: Outcome,
}

impl Response {
    pub fn new(id: Id, outcome: Result<Value, Error>) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            id,
            outcome: match outcome {
                Ok(v) => Outcome::Result(v),
                Err(e) => Outcome::Error(e),
            },
        }
    }
}

/// A message without an id, from the node: an event, a heartbeat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
}

impl Notification {
    pub fn new(method: &str, params: Value) -> Self {
        Self {
            jsonrpc: JSONRPC.into(),
            method: method.into(),
            params,
        }
    }
}

/// The error code a refusal's message key stands for.
pub fn code_for(key: &str) -> i64 {
    match key {
        "node-command-unknown" => code::METHOD_NOT_FOUND,
        "node-command-params" => code::INVALID_PARAMS,
        "node-command-debug-only" | "node-batch-only" => code::DISABLED,
        "node-command-needs-vault" | "node-no-settings" => code::NEEDS_VAULT,
        "node-command-needs-window" => code::NEEDS_WINDOW,
        "node-command-origin" => code::ORIGIN_REFUSED,
        _ => code::REFUSED,
    }
}

/// Turns an answer of the older form (`{"ok": true, …}` or `{"ok": false,
/// "error": …, …}`) into a result or an error: the result is the answer
/// without `ok`; the error's message is `error`, its code follows the
/// refusal's message key ([`code_for`]), and its `data` holds the other
/// fields.
pub fn from_answer(answer: Value) -> Result<Value, Error> {
    let Value::Object(mut fields) = answer else {
        return Ok(answer);
    };
    let ok = fields.remove("ok");
    if ok != Some(Value::Bool(false)) {
        return Ok(Value::Object(fields));
    }
    let message = match fields.remove("error") {
        Some(Value::String(s)) => s,
        Some(other) => other.to_string(),
        None => "refused".into(),
    };
    let code = fields
        .get("message")
        .and_then(|m| m["key"].as_str())
        .map_or(code::REFUSED, code_for);
    let mut error = Error::new(code, message);
    if !fields.is_empty() {
        error = error.with_data(Value::Object(fields));
    }
    Err(error)
}

/// `node.hello`'s answer: the protocol's version and the oldest still
/// served, the node instance, the endpoint and what its client may do.
pub fn hello(instance: &str, endpoint: &str, permissions: &[&str]) -> Value {
    json!({
        "protocol": PROTOCOL,
        "oldest": OLDEST,
        "instance": instance,
        "endpoint": endpoint,
        "permissions": permissions,
        "server": { "name": "helios", "version": env!("CARGO_PKG_VERSION") },
    })
}

/// Where a connection's lines go, shared by its calls.
#[derive(Clone)]
pub struct Sink(Arc<Mutex<Box<dyn Write + Send>>>);

impl Sink {
    pub(crate) fn new(out: impl Write + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Box::new(out))))
    }

    /// Sends one line; fails once the client has gone.
    pub fn send(&self, value: &impl Serialize) -> io::Result<()> {
        let line = serde_json::to_string(value).map_err(io::Error::other)?;
        let mut out = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        writeln!(out, "{line}")?;
        out.flush()
    }

    /// Sends a notification.
    pub fn notify(&self, method: &str, params: Value) -> io::Result<()> {
        self.send(&Notification::new(method, params))
    }
}

/// What runs once a connection closes.
pub(crate) type Closers = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;

/// What a method sees of its call.
pub struct Call {
    id: Option<Id>,
    cancelled: Arc<AtomicBool>,
    sink: Sink,
    closers: Closers,
    after: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
}

impl Call {
    /// The call's id; `None` for a notification.
    pub fn id(&self) -> Option<&Id> {
        self.id.as_ref()
    }

    /// Whether `$/cancel` asked this call to stop: a cancellable method
    /// looks at it while it waits, and answers [`Error::cancelled`].
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// The connection, to send notifications on.
    pub fn sink(&self) -> Sink {
        self.sink.clone()
    }

    /// Runs `f` once the connection closes.
    pub fn on_close(&self, f: impl FnOnce() + Send + 'static) {
        self.closers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::new(f));
    }

    /// Runs `f` on the call's thread once its answer is sent: a
    /// subscription streams its notifications from there, after the
    /// answer that names it.
    pub fn after_reply(&self, f: impl FnOnce() + Send + 'static) {
        self.after
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::new(f));
    }
}

/// What answers an endpoint's JSON-RPC calls.
pub trait Service: Send + Sync {
    /// Runs one call: `params` is an object (empty when not given).
    fn call(&self, method: &str, params: Value, call: &Call) -> Result<Value, Error>;

    /// Whether `$/cancel` can stop a call of `method` while it runs.
    fn cancellable(&self, method: &str) -> bool {
        let _ = method;
        false
    }
}

/// The calls of one connection still running: their cancel flags, and
/// whether their method looks at it.
#[derive(Clone, Default)]
pub(crate) struct Running(Arc<Mutex<HashMap<Id, Flag>>>);

/// A running call's cancel flag, and whether its method looks at it.
type Flag = (Arc<AtomicBool>, bool);

/// Handles one JSON-RPC line of a connection.
pub(crate) fn dispatch(
    value: Value,
    service: &Arc<dyn Service>,
    sink: &Sink,
    closers: &Closers,
    running: &Running,
) {
    if value.is_array() {
        let _ = sink.send(&Response::new(
            Id::Null,
            Err(Error::new(
                code::INVALID_REQUEST,
                "batches are not served: send one call per line",
            )),
        ));
        return;
    }
    let request = match Request::parse(value) {
        Ok(request) => request,
        Err((id, error)) => {
            let _ = sink.send(&Response::new(id, Err(error)));
            return;
        }
    };
    let params = request.params.unwrap_or_else(|| json!({}));
    if request.method == CANCEL {
        let outcome = cancel(&params, running);
        if let Some(id) = request.id {
            let _ = sink.send(&Response::new(id, outcome));
        }
        return;
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    if let Some(id) = &request.id {
        let mut calls = running.0.lock().unwrap_or_else(PoisonError::into_inner);
        if calls.contains_key(id) {
            drop(calls);
            let _ = sink.send(&Response::new(
                id.clone(),
                Err(Error::new(
                    code::INVALID_REQUEST,
                    "a call with this id is still running",
                )),
            ));
            return;
        }
        calls.insert(
            id.clone(),
            (cancelled.clone(), service.cancellable(&request.method)),
        );
    }
    let call = Call {
        id: request.id,
        cancelled,
        sink: sink.clone(),
        closers: closers.clone(),
        after: Mutex::new(Vec::new()),
    };
    let (service, running, method) = (service.clone(), running.clone(), request.method);
    std::thread::spawn(move || {
        let outcome = service.call(&method, params, &call);
        if let Some(id) = &call.id {
            running
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(id);
            if call.sink.send(&Response::new(id.clone(), outcome)).is_err() {
                return;
            }
        }
        let after = std::mem::take(&mut *call.after.lock().unwrap_or_else(PoisonError::into_inner));
        for f in after {
            f();
        }
    });
}

/// `$/cancel {id}`: the call is told to stop when its method looks; it
/// then answers [`code::CANCELLED`].
fn cancel(params: &Value, running: &Running) -> Result<Value, Error> {
    let id = params
        .get("id")
        .and_then(|id| Id::deserialize(id).ok())
        .ok_or_else(|| Error::invalid_params("`$/cancel` takes the `id` of a call"))?;
    let calls = running.0.lock().unwrap_or_else(PoisonError::into_inner);
    match calls.get(&id) {
        None => Ok(json!({ "cancelled": false, "reason": "no call with this id is running" })),
        Some((_, false)) => Err(Error::new(
            code::NOT_CANCELLABLE,
            "this call cannot be cancelled: it runs to its end",
        )),
        Some((flag, true)) => {
            flag.store(true, Ordering::SeqCst);
            Ok(json!({ "cancelled": true }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_have_the_standard_shape() {
        let call = Request::call(1, "state", json!({}));
        assert_eq!(
            serde_json::to_value(&call).unwrap(),
            json!({ "jsonrpc": "2.0", "method": "state", "params": {}, "id": 1 })
        );
        let ok = Response::new(Id::Text("a".into()), Ok(json!(3)));
        assert_eq!(
            serde_json::to_value(&ok).unwrap(),
            json!({ "jsonrpc": "2.0", "id": "a", "result": 3 })
        );
        let err = Response::new(Id::Null, Err(Error::method_not_found("x")));
        let wire = serde_json::to_value(&err).unwrap();
        assert_eq!(wire["error"]["code"], -32601);
        assert_eq!(wire["id"], Value::Null);
        assert_eq!(serde_json::from_value::<Response>(wire).unwrap(), err);
    }

    #[test]
    fn a_null_id_is_not_a_notification() {
        let with_null =
            Request::parse(json!({ "jsonrpc": "2.0", "method": "m", "id": null })).unwrap();
        assert_eq!(with_null.id, Some(Id::Null));
        let without = Request::parse(json!({ "jsonrpc": "2.0", "method": "m" })).unwrap();
        assert_eq!(without.id, None);
        let (id, error) =
            Request::parse(json!({ "jsonrpc": "2.0", "method": "m", "params": [1], "id": 4 }))
                .unwrap_err();
        assert_eq!((id, error.code), (Id::Number(4), code::INVALID_PARAMS));
    }

    #[test]
    fn older_answers_become_results_and_coded_errors() {
        assert_eq!(
            from_answer(json!({ "ok": true, "seq": 4 })),
            Ok(json!({ "seq": 4 }))
        );
        let error = from_answer(json!({
            "ok": false,
            "error": "the node keeps no vault",
            "message": { "key": "node-command-needs-vault", "english": "the node keeps no vault" },
        }))
        .unwrap_err();
        assert_eq!(error.code, code::NEEDS_VAULT);
        assert_eq!(error.message, "the node keeps no vault");
        assert_eq!(
            error.data.unwrap()["message"]["key"],
            "node-command-needs-vault"
        );
        assert_eq!(
            from_answer(json!({ "ok": false, "error": "no" }))
                .unwrap_err()
                .code,
            code::REFUSED
        );
    }
}
