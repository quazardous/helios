//! `command.batch`: several commands run in one call, each with its own
//! result (`docs/protocol.md`, "Command batches").
//!
//! The params are `{steps, stop_on_error, window, scope, origin}`. Each
//! step is `{id, params, window, scope, request_id}`, run as
//! `command.run` would run it, in order, on the call's thread. The answer
//! holds one result per step, in the order sent:
//!
//! - `{"id", "ok": true, "answer"}` for a step that ran (with
//!   `"already_applied": true` when its key had run before);
//! - `{"id", "ok": false, "error": {code, message, data}}` for one that
//!   failed;
//! - `{"id", "skipped": true}` for one left unrun: after a failed step
//!   with `stop_on_error` (the default), or once the batch was cancelled.
//!
//! A batch is not atomic: the steps that ran before a failed one stay
//! applied (`"atomic": false` in the answer says so). It holds at most
//! [`MAX_STEPS`] steps; more are refused before any runs.
//!
//! Keys: a step's `request_id` is its `command.run`'s `_meta.request_id`,
//! so on a socket endpoint (`crate::request_ids`) a step sent again in a
//! later batch with the same key is applied once. The batch itself may
//! carry a key too: sent again, the whole answer comes back.

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::rpc::{Call, Error, Service};

/// The method.
pub const METHOD: &str = "command.batch";

/// The most steps one batch holds.
pub const MAX_STEPS: usize = 256;

/// Runs batch `params` with each step sent to `service` as `command.run`.
///
/// # Errors
/// Params that are not a batch: no `steps`, more than [`MAX_STEPS`], a
/// step without an `id`. A step's own failure is in its result instead.
pub fn run(service: &dyn Service, params: &Value, call: &Call) -> Result<Value, Error> {
    let steps = params["steps"]
        .as_array()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Error::invalid_params("`command.batch` takes `steps`, a list of commands")
        })?;
    if steps.len() > MAX_STEPS {
        return Err(Error::invalid_params(format!(
            "`steps` holds {} commands, over the cap of {MAX_STEPS} per batch: send the rest in another call",
            steps.len()
        )));
    }
    let stop_on_error = params["stop_on_error"].as_bool().unwrap_or(true);
    let mut runs = Vec::with_capacity(steps.len());
    for (n, step) in steps.iter().enumerate() {
        let id = step["id"].as_str().filter(|id| !id.is_empty());
        let Some(id) = id else {
            return Err(Error::invalid_params(format!(
                "step {n} of `command.batch` takes an `id`"
            )));
        };
        runs.push((id, command_run(id, step, params)?));
    }
    let (mut ran, mut failed, mut skipped) = (0, 0, 0);
    let mut stopped = false;
    let mut results = Vec::with_capacity(runs.len());
    for (id, run) in runs {
        if stopped || call.is_cancelled() {
            skipped += 1;
            results.push(json!({ "id": id, "skipped": true }));
            continue;
        }
        let key = run["_meta"]["request_id"].clone();
        let mut result = match service.call("command.run", run, call) {
            Ok(answer) => {
                ran += 1;
                let mut result = json!({ "id": id, "ok": true });
                if answer[crate::request_ids::ALREADY_APPLIED] == true {
                    result[crate::request_ids::ALREADY_APPLIED] = json!(true);
                }
                result["answer"] = answer;
                result
            }
            Err(error) => {
                failed += 1;
                stopped = stop_on_error;
                json!({ "id": id, "ok": false, "error": error })
            }
        };
        if !key.is_null() {
            result["request_id"] = key;
        }
        results.push(result);
    }
    Ok(json!({
        "steps": results,
        "ran": ran,
        "failed": failed,
        "skipped": skipped,
        "cancelled": call.is_cancelled(),
        "atomic": false,
    }))
}

/// The `command.run` params of `step`: its id, params, window and scope
/// (else the batch's), the batch's origin, and its key.
fn command_run(id: &str, step: &Value, batch: &Value) -> Result<Value, Error> {
    let mut run = Map::new();
    run.insert("id".into(), json!(id));
    run.insert(
        "params".into(),
        match &step["params"] {
            Value::Null => json!({}),
            v => v.clone(),
        },
    );
    for field in ["window", "scope"] {
        let value = match &step[field] {
            Value::Null => &batch[field],
            v => v,
        };
        if !value.is_null() {
            run.insert(field.into(), value.clone());
        }
    }
    if !batch["origin"].is_null() {
        run.insert("origin".into(), batch["origin"].clone());
    }
    let mut meta = Map::new();
    if let Some(origin) = batch["_meta"].get("origin") {
        meta.insert("origin".into(), origin.clone());
    }
    match &step["request_id"] {
        Value::Null => {}
        key => {
            meta.insert("request_id".into(), key.clone());
            // Checked as a call's key is.
            crate::request_ids::request_id(&json!({ "_meta": meta }))?;
        }
    }
    if !meta.is_empty() {
        run.insert("_meta".into(), Value::Object(meta));
    }
    Ok(Value::Object(run))
}

/// A service with `command.batch` added, its steps run on the service
/// itself: the in-process transport's, where keys are not looked at.
pub(crate) struct Batched(pub(crate) Arc<dyn Service>);

impl Service for Batched {
    fn call(&self, method: &str, params: Value, call: &Call) -> Result<Value, Error> {
        if method == METHOD {
            return run(&*self.0, &params, call);
        }
        self.0.call(method, params, call)
    }

    fn cancellable(&self, method: &str) -> bool {
        method == METHOD || self.0.cancellable(method)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::client::Client;
    use crate::rpc::code;

    /// Runs `command.run`: `fail` is refused, anything else answers its
    /// id and how many ran; it records each run.
    #[derive(Default)]
    struct Commands(Mutex<Vec<String>>);

    impl Service for Commands {
        fn call(&self, method: &str, params: Value, _: &Call) -> Result<Value, Error> {
            assert_eq!(method, "command.run");
            let id = params["id"].as_str().unwrap().to_string();
            let mut ran = self.0.lock().unwrap();
            ran.push(id.clone());
            if id == "fail" {
                return Err(Error::new(code::REFUSED, "refused"));
            }
            Ok(
                json!({ "command": id, "params": params["params"], "origin": params["origin"], "n": ran.len() }),
            )
        }
    }

    fn batch(steps: &[&str], stop_on_error: bool) -> (Arc<Commands>, Result<Value, Error>) {
        let commands = Arc::new(Commands::default());
        let client = Client::in_process(commands.clone());
        let steps: Vec<Value> = steps
            .iter()
            .map(|id| json!({ "id": id, "params": { "x": id } }))
            .collect();
        let answer = client.request(
            METHOD,
            json!({ "steps": steps, "stop_on_error": stop_on_error, "origin": "agent:test" }),
        );
        (commands, answer)
    }

    #[test]
    fn a_batch_of_mixed_commands_answers_each_step_in_order() {
        let (commands, answer) = batch(&["theme.select", "library.stacks.list"], true);
        let answer = answer.unwrap();
        assert_eq!(answer["ran"], 2);
        assert_eq!(answer["failed"], 0);
        assert_eq!(answer["atomic"], false);
        let steps = answer["steps"].as_array().unwrap();
        assert_eq!(steps[0]["id"], "theme.select");
        assert_eq!(steps[0]["answer"]["n"], 1);
        assert_eq!(steps[0]["answer"]["params"], json!({ "x": "theme.select" }));
        assert_eq!(steps[0]["answer"]["origin"], "agent:test");
        assert_eq!(steps[1]["id"], "library.stacks.list");
        assert_eq!(steps[1]["answer"]["n"], 2);
        assert_eq!(
            *commands.0.lock().unwrap(),
            ["theme.select", "library.stacks.list"]
        );
    }

    #[test]
    fn a_failed_step_stops_the_batch_or_not_as_asked() {
        let (commands, answer) = batch(&["a", "fail", "b"], true);
        let answer = answer.unwrap();
        let steps = answer["steps"].as_array().unwrap();
        assert_eq!(steps[0]["ok"], true);
        assert_eq!(steps[1]["ok"], false);
        assert_eq!(steps[1]["error"]["code"], code::REFUSED);
        assert_eq!(steps[2], json!({ "id": "b", "skipped": true }));
        assert_eq!(
            (
                answer["ran"].clone(),
                answer["failed"].clone(),
                answer["skipped"].clone()
            ),
            (json!(1), json!(1), json!(1))
        );
        assert_eq!(*commands.0.lock().unwrap(), ["a", "fail"]);

        let (commands, answer) = batch(&["a", "fail", "b"], false);
        let answer = answer.unwrap();
        assert_eq!(answer["steps"][2]["ok"], true, "{answer}");
        assert_eq!(answer["skipped"], 0);
        assert_eq!(*commands.0.lock().unwrap(), ["a", "fail", "b"]);
    }

    #[test]
    fn a_batch_over_the_cap_is_refused_before_any_step_runs() {
        let ids = vec!["a"; MAX_STEPS + 1];
        let (commands, answer) = batch(&ids, true);
        let error = answer.unwrap_err();
        assert_eq!(error.code, code::INVALID_PARAMS);
        assert!(
            error.message.contains("over the cap of 256"),
            "{}",
            error.message
        );
        assert!(commands.0.lock().unwrap().is_empty());
        // So is one without steps, or with a step without an id.
        let client = Client::in_process(Arc::new(Commands::default()));
        for params in [
            json!({}),
            json!({ "steps": [] }),
            json!({ "steps": [{ "params": {} }] }),
        ] {
            let error = client.request(METHOD, params).unwrap_err();
            assert_eq!(error.code, code::INVALID_PARAMS);
        }
    }

    #[test]
    fn a_step_carries_its_key_into_its_run() {
        let step = json!({ "id": "a", "request_id": "k1", "window": "import" });
        let batch = json!({ "window": "main", "scope": "window", "_meta": { "origin": "cli" } });
        let run = command_run("a", &step, &batch).unwrap();
        assert_eq!(
            run,
            json!({ "id": "a", "params": {}, "window": "import", "scope": "window",
                    "_meta": { "origin": "cli", "request_id": "k1" } })
        );
        let long = json!({ "id": "a", "request_id": "x".repeat(200) });
        assert!(command_run("a", &long, &json!({})).is_err());
    }
}
