//! An MCP server over the inspect and control endpoints of a node.
//!
//! Every tool maps one to one onto a request of [`helios_inspect::Request`]
//! or [`helios_inspect::control::ControlRequest`]: the tool's arguments
//! become the request's fields, sent to the node's socket, and the node's
//! answer comes back as the tool result. An agent drives Helios with the
//! same verbs a script uses through the `helios` CLI.
//!
//! Transport: JSON-RPC 2.0, one message per line on stdin and stdout.

use std::path::PathBuf;

use helios_inspect::{Address, request_at};

use serde_json::{Map, Value, json};

/// Which endpoint a tool talks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Inspect,
    Control,
    /// Handled by the server itself: starting and stopping a sandbox.
    Sandbox,
}

/// A tool: its name, the request it sends, and its arguments.
pub struct Tool {
    pub name: &'static str,
    pub endpoint: Endpoint,
    /// The `cmd` of the request.
    pub cmd: &'static str,
    pub description: &'static str,
    /// Properties of the input schema, by name.
    pub params: &'static [Param],
}

pub struct Param {
    pub name: &'static str,
    pub schema: fn() -> Value,
    pub description: &'static str,
    /// Filled in when the caller leaves it out; `None` when required.
    pub default: Option<fn() -> Value>,
}

fn integer() -> Value {
    json!({ "type": "integer", "minimum": 0 })
}

fn string() -> Value {
    json!({ "type": "string" })
}

fn region() -> Value {
    json!({
        "type": "object",
        "properties": {
            "x": { "type": "integer" }, "y": { "type": "integer" },
            "width": { "type": "integer", "minimum": 1 }, "height": { "type": "integer", "minimum": 1 }
        },
        "required": ["x", "y", "width", "height"]
    })
}

fn param_value() -> Value {
    json!({
        "description": "One of {\"float\": n}, {\"int\": n}, {\"bool\": b}, {\"choice\": \"s\"}",
        "type": "object",
        "minProperties": 1,
        "maxProperties": 1,
        "properties": {
            "float": { "type": "number" }, "int": { "type": "integer" },
            "bool": { "type": "boolean" }, "choice": { "type": "string" }
        }
    })
}

fn fault_kind() -> Value {
    json!({ "type": "string", "enum": ["compute", "source_read"] })
}

fn zero() -> Value {
    json!(0)
}

fn ten_seconds() -> Value {
    json!(10_000)
}

fn output_port() -> Value {
    json!("output")
}

const SINCE: Param = Param {
    name: "since",
    schema: integer,
    description: "Only events after this sequence number",
    default: Some(zero),
};

const TIMEOUT: Param = Param {
    name: "timeout_ms",
    schema: integer,
    description: "How long to wait, in milliseconds",
    default: Some(ten_seconds),
};

const NODE: Param = Param {
    name: "node",
    schema: string,
    description: "Node name in the network",
    default: None,
};

const DIR: Param = Param {
    name: "dir",
    schema: string,
    description: "Sandbox directory (created by `helios sandbox new` or by this tool)",
    default: None,
};

/// Every tool. Tests check each node tool maps onto a request type.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "helios_sandbox_start",
        endpoint: Endpoint::Sandbox,
        cmd: "start",
        description: "Starts a sandboxed node (creating the sandbox if the directory does not exist) and points this server at it",
        params: &[DIR],
    },
    Tool {
        name: "helios_sandbox_stop",
        endpoint: Endpoint::Sandbox,
        cmd: "stop",
        description: "Stops a sandboxed node",
        params: &[DIR],
    },
    Tool {
        name: "helios_state",
        endpoint: Endpoint::Inspect,
        cmd: "state",
        description: "Every entity's state snapshot (graph, engine, scheduler, node, config, bus)",
        params: &[],
    },
    Tool {
        name: "helios_describe",
        endpoint: Endpoint::Inspect,
        cmd: "describe",
        description: "One entity's state snapshot; a miss lists the entities that exist",
        params: &[Param {
            name: "entity",
            schema: string,
            description: "graph, engine, scheduler, node, config, modules or bus",
            default: None,
        }],
    },
    Tool {
        name: "helios_events",
        endpoint: Endpoint::Inspect,
        cmd: "events",
        description: "Bus events after a sequence number",
        params: &[SINCE],
    },
    Tool {
        name: "helios_wait_event",
        endpoint: Endpoint::Inspect,
        cmd: "wait_event",
        description: "Waits for the first event of a kind after a sequence number; on timeout, says what happened instead",
        params: &[
            Param {
                name: "kind",
                schema: string,
                description: "Event kind, e.g. node.render_done",
                default: None,
            },
            SINCE,
            TIMEOUT,
        ],
    },
    Tool {
        name: "helios_wait_idle",
        endpoint: Endpoint::Inspect,
        cmd: "wait_idle",
        description: "Waits until the scheduler has nothing queued or running; on timeout, shows what is left",
        params: &[TIMEOUT],
    },
    Tool {
        name: "helios_render",
        endpoint: Endpoint::Control,
        cmd: "render",
        description: "Renders a node's output over a region, as a scheduler job (debug control)",
        params: &[
            NODE,
            Param {
                name: "port",
                schema: string,
                description: "Output port",
                default: Some(output_port),
            },
            Param {
                name: "region",
                schema: region,
                description: "Region in image coordinates",
                default: None,
            },
        ],
    },
    Tool {
        name: "helios_set_param",
        endpoint: Endpoint::Control,
        cmd: "set_param",
        description: "Changes one parameter of a module node, checked against its manifest (debug control)",
        params: &[
            NODE,
            Param {
                name: "param",
                schema: string,
                description: "Parameter name",
                default: None,
            },
            Param {
                name: "value",
                schema: param_value,
                description: "New value",
                default: None,
            },
        ],
    },
    Tool {
        name: "helios_cancel",
        endpoint: Endpoint::Control,
        cmd: "cancel",
        description: "Cancels a scheduler job (debug control)",
        params: &[Param {
            name: "job",
            schema: integer,
            description: "Job id",
            default: None,
        }],
    },
    Tool {
        name: "helios_pause",
        endpoint: Endpoint::Control,
        cmd: "pause",
        description: "Stops starting jobs; running ones finish (debug control)",
        params: &[],
    },
    Tool {
        name: "helios_resume",
        endpoint: Endpoint::Control,
        cmd: "resume",
        description: "Starts jobs again (debug control)",
        params: &[],
    },
    Tool {
        name: "helios_fault",
        endpoint: Endpoint::Control,
        cmd: "fault",
        description: "Makes a node's next computation or source read fail once, to test error paths (debug control)",
        params: &[
            NODE,
            Param {
                name: "kind",
                schema: fault_kind,
                description: "compute or source_read",
                default: None,
            },
        ],
    },
];

impl Tool {
    pub fn input_schema(&self) -> Value {
        let properties: Map<String, Value> = self
            .params
            .iter()
            .map(|p| {
                let mut schema = (p.schema)();
                schema["description"] = json!(p.description);
                (p.name.to_string(), schema)
            })
            .collect();
        let required: Vec<&str> = self
            .params
            .iter()
            .filter(|p| p.default.is_none())
            .map(|p| p.name)
            .collect();
        json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
    }

    /// The request this tool sends for these arguments, defaults filled in.
    pub fn request(&self, arguments: &Value) -> Result<Value, String> {
        let given = arguments.as_object().cloned().unwrap_or_default();
        if let Some(unknown) = given
            .keys()
            .find(|k| !self.params.iter().any(|p| p.name == *k))
        {
            return Err(format!("{}: unknown argument `{unknown}`", self.name));
        }
        let mut request = Map::new();
        request.insert("cmd".into(), json!(self.cmd));
        for p in self.params {
            match (given.get(p.name), p.default) {
                (Some(v), _) => {
                    request.insert(p.name.into(), v.clone());
                }
                (None, Some(default)) => {
                    request.insert(p.name.into(), default());
                }
                (None, None) => {
                    return Err(format!("{}: missing argument `{}`", self.name, p.name));
                }
            }
        }
        Ok(Value::Object(request))
    }
}

/// The sockets the server talks to, and how it starts sandboxes.
pub struct Sockets {
    pub inspect: Option<Address>,
    pub control: Option<Address>,
    /// The `helios-node` executable.
    pub node_bin: PathBuf,
}

fn sandbox(sockets: &mut Sockets, tool: &Tool, request: &Value) -> Value {
    let dir = PathBuf::from(request["dir"].as_str().unwrap_or_default());
    let result = match tool.cmd {
        "start" => {
            let created = if dir.join("sandbox.json").exists() {
                Ok(())
            } else {
                crate::sandbox::create(&dir, None).map(|_| ())
            };
            created
                .and_then(|()| crate::sandbox::start(&dir, &sockets.node_bin))
                .map(|state| {
                    sockets.inspect = Some(Address::Unix(state.inspect.clone()));
                    sockets.control =
                        cfg!(debug_assertions).then(|| Address::Unix(state.control.clone()));
                    serde_json::to_value(state).expect("state serializes")
                })
        }
        _ => crate::sandbox::stop(&dir).map(|ran| json!({ "stopped": ran })),
    };
    match result {
        Ok(answer) => tool_result(
            serde_json::to_string_pretty(&answer).expect("JSON values serialize"),
            false,
        ),
        Err(e) => tool_result(e, true),
    }
}

fn send(address: &Address, request: &Value) -> Result<Value, String> {
    request_at(address, request).map_err(|e| format!("cannot reach the node at {address}: {e}"))
}

fn tool_result(text: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn call(sockets: &mut Sockets, params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or_default();
    let Some(tool) = TOOLS.iter().find(|t| t.name == name) else {
        return tool_result(format!("no tool `{name}`"), true);
    };
    let request = match tool.request(&params["arguments"]) {
        Ok(r) => r,
        Err(e) => return tool_result(e, true),
    };
    if tool.endpoint == Endpoint::Sandbox {
        return sandbox(sockets, tool, &request);
    }
    let socket = match (tool.endpoint, &sockets.inspect, &sockets.control) {
        (Endpoint::Sandbox, _, _) => unreachable!("handled above"),
        (Endpoint::Inspect, Some(path), _) | (Endpoint::Control, _, Some(path)) => path,
        (Endpoint::Inspect, None, _) => {
            return tool_result(
                "no node yet: pass --socket or call helios_sandbox_start".into(),
                true,
            );
        }
        (Endpoint::Control, _, None) => {
            return tool_result(
                "no control socket: pass --control or call helios_sandbox_start (debug builds)"
                    .into(),
                true,
            );
        }
    };
    match send(socket, &request) {
        Ok(answer) => tool_result(
            serde_json::to_string_pretty(&answer).expect("JSON values serialize"),
            answer["ok"] != true,
        ),
        Err(e) => tool_result(e, true),
    }
}

/// Answers one JSON-RPC message; `None` for notifications.
pub fn handle(sockets: &mut Sockets, message: &Value) -> Option<Value> {
    let id = message.get("id").cloned()?;
    let result = match message["method"].as_str().unwrap_or_default() {
        "initialize" => Ok(json!({
            "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "helios", "version": env!("CARGO_PKG_VERSION") },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": TOOLS.iter().map(|t| json!({
                "name": t.name, "description": t.description, "inputSchema": t.input_schema(),
            })).collect::<Vec<_>>()
        })),
        "tools/call" => Ok(call(sockets, &message["params"])),
        other => Err(json!({ "code": -32601, "message": format!("method `{other}` not found") })),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_inspect::Request;
    use helios_inspect::control::ControlRequest;

    /// An example call of each tool, with its required arguments only.
    fn example(tool: &Tool) -> Value {
        let args: Map<String, Value> = tool
            .params
            .iter()
            .filter(|p| p.default.is_none())
            .map(|p| {
                let v = match p.name {
                    "entity" | "kind" if tool.cmd == "fault" => json!("compute"),
                    "region" => json!({ "x": 0, "y": 0, "width": 4, "height": 4 }),
                    "value" => json!({ "float": 1.0 }),
                    "job" => json!(3),
                    _ => json!("exp"),
                };
                (p.name.to_string(), v)
            })
            .collect();
        Value::Object(args)
    }

    #[test]
    fn every_tool_maps_onto_a_request_type() {
        for tool in TOOLS.iter().filter(|t| t.endpoint != Endpoint::Sandbox) {
            let request = tool.request(&example(tool)).unwrap();
            let parsed = match tool.endpoint {
                Endpoint::Inspect => serde_json::from_value::<Request>(request.clone()).map(|_| ()),
                Endpoint::Control => {
                    serde_json::from_value::<ControlRequest>(request.clone()).map(|_| ())
                }
                Endpoint::Sandbox => unreachable!("filtered out"),
            };
            assert!(
                parsed.is_ok(),
                "{}: {request} does not parse: {parsed:?}",
                tool.name
            );
        }
    }

    #[test]
    fn every_request_has_a_tool() {
        let inspect = ["state", "describe", "events", "wait_event", "wait_idle"];
        let control = ["render", "set_param", "cancel", "pause", "resume", "fault"];
        for cmd in inspect.iter().chain(&control) {
            assert!(TOOLS.iter().any(|t| t.cmd == *cmd), "no tool for `{cmd}`");
        }
        let node_tools = TOOLS
            .iter()
            .filter(|t| t.endpoint != Endpoint::Sandbox)
            .count();
        assert_eq!(node_tools, inspect.len() + control.len());
    }

    #[test]
    fn arguments_are_checked_and_defaults_filled() {
        let wait = TOOLS
            .iter()
            .find(|t| t.name == "helios_wait_event")
            .unwrap();
        let request = wait.request(&json!({ "kind": "node.idle" })).unwrap();
        assert_eq!(
            request,
            json!({ "cmd": "wait_event", "kind": "node.idle", "since": 0, "timeout_ms": 10000 })
        );
        assert!(
            wait.request(&json!({}))
                .unwrap_err()
                .contains("missing argument `kind`")
        );
        assert!(
            wait.request(&json!({ "kind": "x", "oops": 1 }))
                .unwrap_err()
                .contains("unknown argument `oops`")
        );
        assert_eq!(wait.input_schema()["required"], json!(["kind"]));
    }

    #[test]
    fn notifications_get_no_answer_and_unknown_methods_an_error() {
        let mut sockets = Sockets {
            inspect: Some(Address::Unix(PathBuf::from("/nonexistent"))),
            control: None,
            node_bin: PathBuf::from("helios-node"),
        };
        assert!(
            handle(
                &mut sockets,
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
            )
            .is_none()
        );
        let answer = handle(
            &mut sockets,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "nope" }),
        )
        .unwrap();
        assert_eq!(answer["error"]["code"], -32601);
        let control = handle(&mut sockets, &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "helios_pause" } })).unwrap();
        assert_eq!(control["result"]["isError"], true);
    }
}
