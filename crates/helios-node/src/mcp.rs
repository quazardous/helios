//! An MCP server over the inspect and control endpoints of a node.
//!
//! Every tool maps one to one onto a request of [`helios_inspect::Request`]
//! or [`helios_inspect::control::ControlRequest`]: the tool's arguments
//! become the request's fields, sent to the node's socket, and the node's
//! answer comes back as the tool result. An agent drives Helios with the
//! same verbs a script uses through the `helios` CLI.
//!
//! The `helios_ai_*` tools are answered by the server itself, through the
//! AI service of this machine (`helios-ai`, see `docs/ai-models.md`):
//! providers, routing, prompts and the local model runtime. They need no
//! node, except trying a prompt on a node's current image. Their requests'
//! `cmd` are the dotted names (`ai.providers.list`); the tool names use
//! underscores, which every MCP client accepts.
//!
//! Transport: JSON-RPC 2.0, one message per line on stdin and stdout.

use std::path::PathBuf;
use std::sync::Arc;

use helios_ai::Ai;
use helios_inspect::{Address, request_at};

use serde_json::{Map, Value, json};

/// Which endpoint a tool talks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Inspect,
    Control,
    /// Handled by the server itself: starting and stopping a sandbox.
    Sandbox,
    /// Handled by the server itself, through the AI service.
    Ai,
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

fn optional_region() -> Value {
    json!({ "anyOf": [region(), { "type": "null" }] })
}

fn file_format() -> Value {
    json!({ "type": "string", "enum": ["jpeg", "png", "tiff"] })
}

fn optional_quality() -> Value {
    json!({ "type": ["integer", "null"], "minimum": 1, "maximum": 100 })
}

fn node_params() -> Value {
    json!({
        "description": "Node name to parameter values",
        "type": "object",
        "additionalProperties": param_values()
    })
}

fn null() -> Value {
    Value::Null
}

fn empty_object() -> Value {
    json!({})
}

fn fault_kind() -> Value {
    json!({ "type": "string", "enum": ["compute", "source_read"] })
}

fn zero() -> Value {
    json!(0)
}

fn agent() -> Value {
    json!("agent")
}

fn param_values() -> Value {
    json!({
        "description": "Parameter name to value, each value one of {\"float\": n}, {\"int\": n}, {\"bool\": b}, {\"choice\": \"s\"}",
        "type": "object",
        "additionalProperties": param_value()
    })
}

const WHO: Param = Param {
    name: "who",
    schema: string,
    description: "Who decides, as the decision's history records it",
    default: Some(agent),
};

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

fn optional_string() -> Value {
    json!({ "type": ["string", "null"] })
}

fn optional_integer() -> Value {
    json!({ "type": ["integer", "null"], "minimum": 0 })
}

fn optional_strings() -> Value {
    json!({ "type": ["array", "null"], "items": { "type": "string" } })
}

fn strings() -> Value {
    json!({ "type": "array", "items": { "type": "string" } })
}

fn optional_object() -> Value {
    json!({ "type": ["object", "null"] })
}

fn object() -> Value {
    json!({ "type": "object" })
}

fn boolean() -> Value {
    json!({ "type": "boolean" })
}

fn no() -> Value {
    json!(false)
}

fn one() -> Value {
    json!(1)
}

fn empty_list() -> Value {
    json!([])
}

fn provider_kind() -> Value {
    json!({ "type": "string", "enum": ["ollama", "openai", "anthropic", "command"] })
}

fn optional_capability() -> Value {
    json!({ "type": ["string", "null"],
        "enum": ["chat", "vision", "embeddings", "structured", "segmentation", null] })
}

/// An optional argument, null when left out.
const fn optional(name: &'static str, schema: fn() -> Value, description: &'static str) -> Param {
    Param {
        name,
        schema,
        description,
        default: Some(null),
    }
}

const PROVIDER_ID: Param = Param {
    name: "id",
    schema: string,
    description: "Provider id (`ollama` is the built-in local one)",
    default: None,
};

const PROMPT: Param = Param {
    name: "prompt",
    schema: string,
    description: "Prompt id (`image.describe`), or `id@version` to pin one",
    default: None,
};

const VERSION: Param = optional(
    "version",
    optional_integer,
    "Prompt version; the latest when null",
);

const JOB: Param = Param {
    name: "job",
    schema: integer,
    description: "Job id, as the tool that started it said",
    default: None,
};

const FILE: Param = optional(
    "file",
    optional_string,
    "Absolute path of an image file (PNG, JPEG) on this machine",
);

const ROUTE_PROVIDER: Param = optional(
    "provider",
    optional_string,
    "Provider id; with `model`, overrides the routing map",
);

const ROUTE_MODEL: Param = optional("model", optional_string, "Model name at that provider");

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
        name: "helios_load_source",
        endpoint: Endpoint::Control,
        cmd: "load_source",
        description: "Gives a source node the image of a file (a raw or a picture), read by the node; the network stays (debug control)",
        params: &[
            NODE,
            Param {
                name: "path",
                schema: string,
                description: "Absolute path of the file, on the node's machine",
                default: None,
            },
        ],
    },
    Tool {
        name: "helios_export",
        endpoint: Endpoint::Control,
        cmd: "export",
        description: "Exports a node's output to a file as a scheduler job holding the destination; the file appears whole or not at all. The network is taken as it is now (debug control)",
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
                schema: optional_region,
                description: "Region in image coordinates; the node's whole extent when null",
                default: Some(null),
            },
            Param {
                name: "path",
                schema: string,
                description: "Absolute path of the file to write, on the node's machine",
                default: None,
            },
            Param {
                name: "format",
                schema: file_format,
                description: "File format",
                default: None,
            },
            Param {
                name: "quality",
                schema: optional_quality,
                description: "JPEG quality, 1 to 100 (92 when null)",
                default: Some(null),
            },
            Param {
                name: "params",
                schema: node_params,
                description: "Parameters changed for this export only, by node name",
                default: Some(empty_object),
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
    Tool {
        name: "helios_decisions",
        endpoint: Endpoint::Control,
        cmd: "decisions",
        description: "Lists the network's decisions: mode, status, the answer in force and its history, a pending proposal (debug control)",
        params: &[],
    },
    Tool {
        name: "helios_decision_resolve",
        endpoint: Endpoint::Control,
        cmd: "decision_resolve",
        description: "Asks the network's model about every decision waiting on one; full-AI answers are applied, AI-assisted ones become proposals (debug control)",
        params: &[],
    },
    Tool {
        name: "helios_decision_accept",
        endpoint: Endpoint::Control,
        cmd: "decision_accept",
        description: "Takes one option of a decision's proposal, 0 being the model's best (debug control)",
        params: &[
            NODE,
            Param {
                name: "option",
                schema: integer,
                description: "Option number, 0 for the best",
                default: Some(zero),
            },
            WHO,
        ],
    },
    Tool {
        name: "helios_decision_decide",
        endpoint: Endpoint::Control,
        cmd: "decision_decide",
        description: "Gives a decision's answer directly, checked against the decision module's parameters (debug control)",
        params: &[
            NODE,
            Param {
                name: "values",
                schema: param_values,
                description: "The answer's parameters",
                default: None,
            },
            WHO,
        ],
    },
    Tool {
        name: "helios_decision_reask",
        endpoint: Endpoint::Control,
        cmd: "decision_reask",
        description: "Withdraws a decision's answer so it is asked again; earlier answers stay in its history (debug control)",
        params: &[NODE],
    },
    Tool {
        name: "helios_ai_providers_list",
        endpoint: Endpoint::Ai,
        cmd: "ai.providers.list",
        description: "AI model providers in effect: kind, URL, capabilities, local or remote, where the key is (never the key)",
        params: &[],
    },
    Tool {
        name: "helios_ai_providers_add",
        endpoint: Endpoint::Ai,
        cmd: "ai.providers.add",
        description: "Adds or replaces an AI provider: ollama, openai (any OpenAI-compatible server: llama.cpp, LM Studio, vLLM, OpenAI) or anthropic by URL, or a command (segmentation). A remote provider receives downscaled images. Prefer `key_env`; `api_key` goes to the system keyring, never to a file",
        params: &[
            PROVIDER_ID,
            Param {
                name: "kind",
                schema: provider_kind,
                description: "Provider kind",
                default: None,
            },
            optional(
                "url",
                optional_string,
                "Base URL (`http://127.0.0.1:11434`, `http://localhost:8080/v1`); the kind's default when null",
            ),
            optional(
                "key_env",
                optional_string,
                "Environment variable holding the API key",
            ),
            optional(
                "api_key",
                optional_string,
                "The API key itself, stored in the system keyring",
            ),
            optional(
                "command",
                optional_strings,
                "A command provider's program and arguments",
            ),
            optional(
                "capabilities",
                optional_strings,
                "Narrows what the kind declares (chat, vision, embeddings, structured, segmentation)",
            ),
            optional("timeout_s", optional_integer, "Seconds a call may take"),
        ],
    },
    Tool {
        name: "helios_ai_providers_remove",
        endpoint: Endpoint::Ai,
        cmd: "ai.providers.remove",
        description: "Removes an AI provider, the routes to it and its keyring entry",
        params: &[PROVIDER_ID],
    },
    Tool {
        name: "helios_ai_providers_test",
        endpoint: Endpoint::Ai,
        cmd: "ai.providers.test",
        description: "Whether an AI provider answers, its models, and with `model` a short call timed",
        params: &[
            PROVIDER_ID,
            optional("model", optional_string, "A model to call"),
        ],
    },
    Tool {
        name: "helios_ai_models_list",
        endpoint: Endpoint::Ai,
        cmd: "ai.models.list",
        description: "Each AI provider's models, their capabilities and what they are routed for",
        params: &[optional("provider", optional_string, "Only this provider")],
    },
    Tool {
        name: "helios_ai_routing_get",
        endpoint: Endpoint::Ai,
        cmd: "ai.routing.get",
        description: "The AI routing map (capability or prompt to provider and model), each route checked, with why one is unavailable",
        params: &[],
    },
    Tool {
        name: "helios_ai_routing_set",
        endpoint: Endpoint::Ai,
        cmd: "ai.routing.set",
        description: "Routes a capability or a prompt to a provider and model; without provider and model, clears the route",
        params: &[
            optional("capability", optional_capability, "The capability routed"),
            optional(
                "prompt",
                optional_string,
                "Or the prompt id routed (wins over its capability)",
            ),
            ROUTE_PROVIDER,
            ROUTE_MODEL,
        ],
    },
    Tool {
        name: "helios_ai_prompts_list",
        endpoint: Endpoint::Ai,
        cmd: "ai.prompts.list",
        description: "Every AI prompt (shipped and user versions), its capability, versions and the models each was tuned for",
        params: &[],
    },
    Tool {
        name: "helios_ai_prompts_get",
        endpoint: Endpoint::Ai,
        cmd: "ai.prompts.get",
        description: "One AI prompt version whole: text, input and output schemas, example, tuned_for",
        params: &[PROMPT, VERSION],
    },
    Tool {
        name: "helios_ai_prompts_set",
        endpoint: Endpoint::Ai,
        cmd: "ai.prompts.set",
        description: "Writes a new user version of an AI prompt (the next number for its id; a version never changes once written)",
        params: &[Param {
            name: "prompt",
            schema: object,
            description: "The prompt: id, capability, template (with {{name}} slots), input and output JSON schemas, images, tuned_for, example, system, temperature, seed",
            default: None,
        }],
    },
    Tool {
        name: "helios_ai_prompts_try",
        endpoint: Endpoint::Ai,
        cmd: "ai.prompts.try",
        description: "Runs an AI prompt on a node's current image or an image file and returns the raw and the validated answer; applies nothing",
        params: &[
            PROMPT,
            VERSION,
            optional(
                "input",
                optional_object,
                "The prompt's input; its example when null",
            ),
            FILE,
            optional(
                "node",
                optional_string,
                "A node whose output is the image (exported through the control socket)",
            ),
            ROUTE_PROVIDER,
            ROUTE_MODEL,
        ],
    },
    Tool {
        name: "helios_ai_local_probe",
        endpoint: Endpoint::Ai,
        cmd: "ai.local.probe",
        description: "This machine for local AI models: GPU vendor and VRAM, RAM, CPU, free disk where models go, OS, the memory budget and the models that fit",
        params: &[],
    },
    Tool {
        name: "helios_ai_local_status",
        endpoint: Endpoint::Ai,
        cmd: "ai.local.status",
        description: "Whether the local runtime (Ollama) is installed and running, where, its version and models",
        params: &[],
    },
    Tool {
        name: "helios_ai_local_recommend",
        endpoint: Endpoint::Ai,
        cmd: "ai.local.recommend",
        description: "Local models per capability that fit this machine, best first, with their size and whether installed",
        params: &[],
    },
    Tool {
        name: "helios_ai_local_pull",
        endpoint: Endpoint::Ai,
        cmd: "ai.local.pull",
        description: "Downloads a model into the local runtime as a job, after checking the disk; follow it with helios_ai_jobs_events (job.progress) and helios_ai_jobs_wait, stop it with helios_ai_jobs_cancel",
        params: &[
            Param {
                name: "model",
                schema: string,
                description: "Model name (`qwen2.5vl:7b`)",
                default: None,
            },
            optional(
                "provider",
                optional_string,
                "An Ollama provider; the built-in one when null",
            ),
            optional(
                "size_mib",
                optional_integer,
                "The model's size, when the recommendations do not know it",
            ),
        ],
    },
    Tool {
        name: "helios_ai_local_bench",
        endpoint: Endpoint::Ai,
        cmd: "ai.local.bench",
        description: "Runs each AI prompt on each local model able to, as a job: latency and whether the answer validates; with `apply`, routes each prompt and unrouted capability to the fastest valid model",
        params: &[
            optional(
                "prompts",
                optional_strings,
                "Prompt references; every prompt when null",
            ),
            FILE,
            Param {
                name: "apply",
                schema: boolean,
                description: "Fill the routing map from the results",
                default: Some(no),
            },
            Param {
                name: "runs",
                schema: integer,
                description: "Runs per prompt and model (the median is kept)",
                default: Some(one),
            },
            optional(
                "providers",
                optional_strings,
                "Only these providers; every local one when null",
            ),
        ],
    },
    Tool {
        name: "helios_ai_local_setup",
        endpoint: Endpoint::Ai,
        cmd: "ai.local.setup",
        description: "The exact steps to install and start the local runtime (Ollama) for this user on this OS; runs only the steps whose id is in `confirm`, never one needing root, never as root",
        params: &[Param {
            name: "confirm",
            schema: strings,
            description: "Ids of the steps to run, after reading them",
            default: Some(empty_list),
        }],
    },
    Tool {
        name: "helios_ai_jobs_events",
        endpoint: Endpoint::Ai,
        cmd: "ai.jobs.events",
        description: "Events of the AI service after a sequence number: job.progress, ai.call, scheduler.job_*",
        params: &[SINCE],
    },
    Tool {
        name: "helios_ai_jobs_wait",
        endpoint: Endpoint::Ai,
        cmd: "ai.jobs.wait",
        description: "Waits for an AI job (a pull, a bench) to end; its state and result",
        params: &[JOB, TIMEOUT],
    },
    Tool {
        name: "helios_ai_jobs_cancel",
        endpoint: Endpoint::Ai,
        cmd: "ai.jobs.cancel",
        description: "Cancels an AI job (a pull, a bench)",
        params: &[JOB],
    },
    Tool {
        name: "helios_ai_doctor",
        endpoint: Endpoint::Ai,
        cmd: "ai.doctor",
        description: "The AI report of `helios doctor ai`: providers, routing, the local runtime's health, and why a capability is unavailable",
        params: &[],
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
    /// The AI service, opened at the first `helios_ai_*` call.
    pub ai: Option<Arc<Ai>>,
}

/// The current image of `node`: its output exported as a PNG through the
/// control socket, read once the node is idle.
fn node_image(sockets: &Sockets, node: &str) -> Result<Vec<u8>, String> {
    let (Some(control), Some(inspect)) = (&sockets.control, &sockets.inspect) else {
        return Err("a node's image needs the control socket: pass `file` instead".into());
    };
    let path =
        std::env::temp_dir().join(format!("helios-ai-try-{}-{node}.png", std::process::id()));
    let export = json!({ "cmd": "export", "node": node, "port": "output", "region": null,
        "path": path, "format": "png", "quality": null, "params": {} });
    let answer = send(control, &export)?;
    if answer["ok"] != true {
        return Err(format!("exporting `{node}`: {answer}"));
    }
    let idle = send(
        inspect,
        &json!({ "cmd": "wait_idle", "timeout_ms": 120_000 }),
    )?;
    if idle["ok"] != true {
        return Err(format!("waiting for the export of `{node}`: {idle}"));
    }
    let bytes = helios_ai::read_image(&path);
    let _ = std::fs::remove_file(&path);
    bytes
}

fn ai_call(sockets: &mut Sockets, request: &Value) -> Value {
    let ai = match &sockets.ai {
        Some(ai) => ai.clone(),
        None => match Ai::open(helios_ai::store::Paths::default_for_user()) {
            Ok(ai) => {
                let ai = Arc::new(ai);
                sockets.ai = Some(ai.clone());
                ai
            }
            Err(e) => return tool_result(format!("the AI service: {e}"), true),
        },
    };
    let mut images = Vec::new();
    if request["cmd"] == "ai.prompts.try"
        && request["file"].is_null()
        && let Some(node) = request["node"].as_str()
    {
        match node_image(sockets, node) {
            Ok(png) => images.push(png),
            Err(e) => return tool_result(e, true),
        }
    }
    match helios_ai::tools::call(&ai, request, images) {
        Ok(answer) => tool_result(
            serde_json::to_string_pretty(&answer).expect("JSON values serialize"),
            false,
        ),
        Err(e) => tool_result(e, true),
    }
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
    if tool.endpoint == Endpoint::Ai {
        return ai_call(sockets, &request);
    }
    let socket = match (tool.endpoint, &sockets.inspect, &sockets.control) {
        (Endpoint::Sandbox | Endpoint::Ai, _, _) => unreachable!("handled above"),
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
                    "values" => json!({ "exposure": { "float": 1.0 } }),
                    "job" => json!(3),
                    "format" => json!("png"),
                    "path" => json!("/nonexistent/out.png"),
                    _ => json!("exp"),
                };
                (p.name.to_string(), v)
            })
            .collect();
        Value::Object(args)
    }

    #[test]
    fn every_tool_maps_onto_a_request_type() {
        for tool in TOOLS
            .iter()
            .filter(|t| !matches!(t.endpoint, Endpoint::Sandbox | Endpoint::Ai))
        {
            let request = tool.request(&example(tool)).unwrap();
            let parsed = match tool.endpoint {
                Endpoint::Inspect => serde_json::from_value::<Request>(request.clone()).map(|_| ()),
                Endpoint::Control => {
                    serde_json::from_value::<ControlRequest>(request.clone()).map(|_| ())
                }
                Endpoint::Sandbox | Endpoint::Ai => unreachable!("filtered out"),
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
        let control = [
            "render",
            "set_param",
            "load_source",
            "export",
            "cancel",
            "pause",
            "resume",
            "fault",
            "decisions",
            "decision_resolve",
            "decision_accept",
            "decision_decide",
            "decision_reask",
        ];
        for cmd in inspect.iter().chain(&control) {
            assert!(TOOLS.iter().any(|t| t.cmd == *cmd), "no tool for `{cmd}`");
        }
        let node_tools = TOOLS
            .iter()
            .filter(|t| !matches!(t.endpoint, Endpoint::Sandbox | Endpoint::Ai))
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
            ai: None,
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

    #[test]
    fn every_ai_request_has_a_tool_and_names_suit_every_client() {
        let ai: Vec<&str> = TOOLS
            .iter()
            .filter(|t| t.endpoint == Endpoint::Ai)
            .map(|t| t.cmd)
            .collect();
        assert_eq!(ai, helios_ai::tools::CMDS);
        for tool in TOOLS {
            assert!(
                tool.name.len() <= 64
                    && tool
                        .name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "`{}`: tool names are [a-zA-Z0-9_-]",
                tool.name
            );
        }
    }

    /// A server whose AI service runs on a fake runtime and machine.
    fn ai_server(name: &str) -> (Sockets, Arc<helios_ai::fake::FakeRuntime>) {
        let dir = std::env::temp_dir().join(format!("helios-mcp-ai-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let runtime = Arc::new(
            helios_ai::fake::FakeRuntime::new()
                .model("qwen2.5vl:7b", &["completion", "vision"])
                .reply(
                    "qwen2.5vl:7b",
                    r#"{"focus": 4, "exposure": 3, "composition": 4, "defects": []}"#,
                ),
        );
        let system = helios_ai::fake::FakeSystem::linux()
            .output(
                "df -Pk /",
                Ok("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/x 900000000 1 800000000 1% /\n"),
            )
            .output("id -u", Ok("1000\n"));
        let ai = Ai::with_parts(
            helios_ai::store::Paths::under(&dir),
            runtime.clone(),
            Arc::new(helios_ai::secrets::MemorySecrets::default()),
            Arc::new(system),
        )
        .unwrap();
        let sockets = Sockets {
            inspect: None,
            control: None,
            node_bin: PathBuf::from("helios-node"),
            ai: Some(Arc::new(ai)),
        };
        (sockets, runtime)
    }

    fn tool(sockets: &mut Sockets, name: &str, arguments: Value) -> (bool, Value) {
        let answer = handle(
            sockets,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": name, "arguments": arguments } }),
        )
        .unwrap();
        let result = &answer["result"];
        let text = result["content"][0]["text"].as_str().unwrap();
        (
            result["isError"] == true,
            serde_json::from_str(text).unwrap_or(json!(text)),
        )
    }

    #[test]
    fn an_agent_sets_up_and_tries_a_local_model_through_mcp() {
        let (mut s, runtime) = ai_server("agent");
        let (failed, providers) = tool(&mut s, "helios_ai_providers_list", json!({}));
        assert!(!failed);
        assert_eq!(providers[0]["id"], "ollama");
        assert_eq!(providers[0]["remote"], false);

        let (failed, setup) = tool(&mut s, "helios_ai_local_setup", json!({}));
        assert!(!failed, "{setup}");
        assert_eq!(setup["ran"], false, "nothing runs without confirm");

        let (failed, pull) = tool(
            &mut s,
            "helios_ai_local_pull",
            json!({ "model": "qwen3:8b" }),
        );
        assert!(!failed, "{pull}");
        let (_, ended) = tool(&mut s, "helios_ai_jobs_wait", json!({ "job": pull["job"] }));
        assert_eq!(ended["state"], "done", "{ended}");
        let (_, events) = tool(&mut s, "helios_ai_jobs_events", json!({}));
        assert!(
            events["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"] == "job.progress")
        );
        assert!(runtime.installed().contains(&"qwen3:8b".to_string()));

        let (failed, _) = tool(
            &mut s,
            "helios_ai_routing_set",
            json!({ "capability": "vision", "provider": "ollama", "model": "qwen2.5vl:7b" }),
        );
        assert!(!failed);
        let card =
            std::env::temp_dir().join(format!("helios-mcp-ai-card-{}.png", std::process::id()));
        std::fs::write(&card, helios_ai::images::test_card(32)).unwrap();
        let (failed, tried) = tool(
            &mut s,
            "helios_ai_prompts_try",
            json!({ "prompt": "image.assess", "file": &card }),
        );
        assert!(!failed, "{tried}");
        assert_eq!(tried["validated"]["focus"], 4);
        assert_eq!(tried["applied"], false);

        let (failed, missing) = tool(
            &mut s,
            "helios_ai_prompts_try",
            json!({ "prompt": "image.assess", "node": "exp" }),
        );
        assert!(failed, "no control socket: {missing}");

        let (failed, doctor) = tool(&mut s, "helios_ai_doctor", json!({}));
        assert!(!failed);
        assert_eq!(
            doctor["routing"]["capabilities"]["vision"]["available"],
            true
        );
        let _ = std::fs::remove_file(card);
    }
}
