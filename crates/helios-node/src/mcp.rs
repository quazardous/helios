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
//! The `helios_workspace_*` tools are answered by the server itself too:
//! a workspace's parts listed, edited, reset to its template, parts of
//! another added as a copy, saved as a template, each with a dry run,
//! each step undone by `helios_workspace_undo`. When a desktop holds the
//! workspace open (it says so among the workspace's files,
//! `workspace::HOLDER`), the request goes to it: the edit is one step of
//! its Edit > Undo, the agent's, seen at once, and its next save keeps it;
//! the calls the agent gives one `group` name are one step labelled with
//! it (the answer carries the group's number). Otherwise
//! they work on the workspace's files (`helios_template::workspace::tools`),
//! which a desktop opening the workspace later reads, and one that has
//! it open without having said so takes in as an undo step. Each answer
//! says which (`via`: `desktop` or `files`).
//!
//! The `helios_context_*` tools are egui-pop's `context.*` commands for a
//! workspace: they read its places and modes (the library, the darkroom
//! on an image, its network, a sub-workspace; culling, mask editing,
//! compare) and move
//! them, through the same machine and frames as the desktop
//! (`helios_template::contexts`), each with `if_rev` and a dry run. Like
//! the workspace tools they go to the desktop holding the workspace,
//! whose window moves at once; otherwise they work on the stack the
//! workspace keeps beside its layout, what a desktop opening it comes
//! back to. `helios_context_hold` and `helios_context_release` hold the
//! stack of the desktop holding the workspace (a hold lives in its
//! window), so without one they are refused.
//!
//! The clipboard tools (`helios_clipboard`, `helios_copy`,
//! `helios_paste`) are egui-pop's `clipboard.*` commands on the desktop
//! holding a workspace, reached the same way: the agent works in a slot
//! of its own (`agent:mcp`), never in what the user copied unless the
//! user allows it, copies from or pastes into the desktop's endpoints
//! (a parameter, the graph, the masks, the library), with a dry run.
//! Without a desktop holding the workspace they are refused: the
//! clipboard is the desktop's (a node's own slots, kept in the vault,
//! are not built).
//!
//! The `helios_history_*` tools are egui-pop's `history.*` commands
//! (list, read, undo, redo, jump, each with `if_rev` and a dry run) on
//! the desktop holding a workspace open, run there as an agent through
//! the same steps as the user's Ctrl+Z and History panel, in the agent's
//! own lane: it walks only its own steps, and `helios_history_undo` with a
//! `group` undoes one of its batches alone, even under later steps that
//! left its values alone. The histories live in that window, so without
//! one they are refused.
//!
//! `helios_commands_list` and `helios_command_run` are the generic pair
//! over the command registry of a vault's node (`commands.list`,
//! `command.run`): every command, the node's and a desktop's window
//! commands, listed with its params' schema, `enabled` and
//! `disabledReason`, and run by id as `agent:mcp`, without a tool of its
//! own. The hand-written tools stay where they add what the pair cannot:
//! typed arguments and documentation for the processing node's verbs, and
//! the server's own (sandbox, AI, workspace files).
//!
//! Outcome unknown, never resend: when the connection to the node closes
//! after a request was sent and before its answer came, the tool's error
//! starts with "outcome unknown" (`-32008` on the generic pair). The
//! request may have been applied; the agent reads the state before
//! sending it again, and never resends a change blindly. "cannot reach
//! the node" means it was not sent.
//!
//! Caps and untrusted text (`crate::caps`): a request is at most 1 MiB,
//! a `limit` at most 1,000 items; an answer's lists and text are cut to
//! their caps with a "truncated, N more" note after the JSON, and text
//! from photos' metadata or files (titles, tags, camera strings, paths)
//! is named in a note as data, never instructions.
//!
//! Transport: JSON-RPC 2.0, one message per line on stdin and stdout.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_ai::Ai;
use helios_inspect::{Address, request_at};

use crate::caps;
use crate::roots::{Access, Roots};

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
    /// Handled by the server itself, on a workspace's files.
    Workspace,
    /// `commands.list` and `command.run` on the node of a vault (or the
    /// one `node` names, else the control socket): the generic pair over
    /// its command registry, a desktop's window commands included.
    Commands,
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

fn corners() -> Value {
    json!({
        "type": "array",
        "minItems": 4,
        "maxItems": 4,
        "items": { "type": "array", "minItems": 2, "maxItems": 2, "items": { "type": "number" } }
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

fn hundred() -> Value {
    json!(100)
}

fn name_pattern() -> Value {
    json!("{stem}.{ext}")
}

fn run_items() -> Value {
    json!({
        "type": "array",
        "minItems": 1,
        "items": { "anyOf": [
            { "type": "string" },
            { "type": "object", "properties": {
                "path": { "type": "string" },
                "params": node_params()
            }, "required": ["path"] }
        ] }
    })
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

fn optional_number() -> Value {
    json!({ "type": ["number", "null"] })
}

fn review_action() -> Value {
    json!({ "type": ["string", "null"], "enum": ["list", "accept", "revert", "fix", "decide", "rerun", null] })
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

fn optional_objects() -> Value {
    json!({ "type": ["array", "null"], "items": { "type": "object" } })
}

fn objects() -> Value {
    json!({ "type": "array", "items": { "type": "object" } })
}

fn object() -> Value {
    json!({ "type": "object" })
}

fn anything() -> Value {
    json!({})
}

fn boolean() -> Value {
    json!({ "type": "boolean" })
}

fn no() -> Value {
    json!(false)
}

fn yes() -> Value {
    json!(true)
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

fn session_action() -> Value {
    json!({ "type": "string", "enum": ["read", "keep", "close"] })
}

fn read() -> Value {
    json!("read")
}

fn move_to() -> Value {
    json!({ "type": "string", "enum": ["top", "bottom"] })
}

/// A run's id, for the `workflow.*` tools.
const RUN: Param = Param {
    name: "run",
    schema: string,
    description: "The run's id",
    default: None,
};

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

const WORKSPACE: Param = Param {
    name: "workspace",
    schema: string,
    description: "The workspace's directory, an absolute path",
    default: None,
};

const CONFIG: Param = optional(
    "config",
    optional_string,
    "Helios's configuration directory; the user's when null",
);

const DRY_RUN: Param = Param {
    name: "dry_run",
    schema: boolean,
    description: "Say what would change, write nothing",
    default: Some(no),
};

const SLOT: Param = optional(
    "slot",
    optional_string,
    "The clipboard slot: the agent's own (`agent:mcp`) when null; `user` is refused unless the user allows it",
);

const MACRO_NODE: Param = Param {
    name: "node",
    schema: string,
    description: "The macro's node in the network",
    default: None,
};

const MACRO_IMAGE: Param = optional(
    "image",
    optional_string,
    "With no desktop holding the workspace: the image (its id in the vault) whose values move with the step, saved as a version of its edit; with `vault`",
);

const MACRO_VAULT: Param = optional("vault", optional_string, "The vault file holding `image`");

const ENDPOINTS: &str = "helios.param (the focused parameter), helios.graph (the graph view's selection, the workspace's network), helios.masks (the mask edited), helios.library (the grid's selection, the library)";

const HISTORY_SCOPE: Param = optional(
    "scope",
    optional_string,
    "The history (`edit`: the image's edit); where Ctrl+Z goes when null",
);

const HISTORY_IF_REV: Param = optional(
    "if_rev",
    optional_integer,
    "Refused unless the history is still at this revision (from helios_history_read)",
);

const GROUP: Param = optional(
    "group",
    optional_string,
    "A name for this request's changes on the desktop: calls with the same name make one undo step, labelled with it, while no other step comes between; each call is its own step when null",
);

const SOURCE: Param = Param {
    name: "source",
    schema: string,
    description: "A template's id (built in or the user's), a template bundle's directory, or a workspace's directory",
    default: None,
};

/// Every tool. Tests check each node tool maps onto a request type.
/// Where the generic command pair goes: a vault's node, or one by address.
const COMMANDS_VAULT: Param = optional(
    "vault",
    optional_string,
    "The vault whose node runs the commands (a desktop holding it serves its window commands too); started on it when no node holds it",
);
const COMMANDS_NODE: Param = optional(
    "node",
    optional_string,
    "The node's command endpoint (a socket path or tcp://HOST:PORT, with HELIOS_TOKEN), instead of `vault`; else HELIOS_NODE, else the control socket",
);

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "helios_commands_list",
        endpoint: Endpoint::Commands,
        cmd: "commands.list",
        description: "Every command of the node's registry, generated rather than one tool each: id, label, category, the JSON schema of its params, keys, `scope` (node, or window for a desktop's window commands), `enabled` and, when greyed, `disabledReason` (\"Only in the Helios window\"). Run one with helios_command_run",
        params: &[
            COMMANDS_VAULT,
            COMMANDS_NODE,
            optional(
                "window",
                optional_string,
                "The desktop window the window commands are evaluated in (main, import, settings...); main when left out",
            ),
            optional(
                "filter",
                optional_string,
                "Keeps the commands whose id, label or category holds every word",
            ),
            Param {
                name: "enabled_only",
                schema: boolean,
                description: "Only the commands that can run now",
                default: Some(no),
            },
        ],
    },
    Tool {
        name: "helios_command_run",
        endpoint: Endpoint::Commands,
        cmd: "command.run",
        description: "Runs a command of helios_commands_list by its id, as the agent (origin agent:mcp): its params are checked against its schema, a greyed one is refused with its reason, a window command runs in that desktop window as its menu would",
        params: &[
            COMMANDS_VAULT,
            COMMANDS_NODE,
            Param {
                name: "id",
                schema: string,
                description: "The command's id, as listed",
                default: None,
            },
            Param {
                name: "params",
                schema: object,
                description: "The command's params, an object its listed schema accepts",
                default: Some(empty_object),
            },
            optional(
                "window",
                optional_string,
                "The desktop window a window command runs in; main when left out",
            ),
            optional(
                "scope",
                optional_string,
                "node or window, when an id is in both (library.import): the node's wins when left out",
            ),
        ],
    },
    Tool {
        name: "helios_workspace_parts",
        endpoint: Endpoint::Workspace,
        cmd: "workspace.parts",
        description: "The parts of a workspace (network, layout, keys, toolbox, library), or of another workspace or template given as `source`",
        params: &[
            WORKSPACE,
            optional(
                "source",
                optional_string,
                "A template's id or directory, or a workspace's directory; the workspace itself when null",
            ),
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workspace_state",
        endpoint: Endpoint::Workspace,
        cmd: "workspace.state",
        description: "A workspace as it stands, whole: its name, description and icon, its network, tools, library and layout",
        params: &[WORKSPACE, CONFIG],
    },
    Tool {
        name: "helios_workspace_edit",
        endpoint: Endpoint::Workspace,
        cmd: "workspace.edit",
        description: "Edits a workspace as one undoable step: `changes` sets its name, description, icon, network, tools, library or layout, written as in a template; checked first",
        params: &[
            WORKSPACE,
            Param {
                name: "changes",
                schema: object,
                description: "The parts changed: name, description, icon, network, tools, library (null for none), layout",
                default: None,
            },
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workspace_reset",
        endpoint: Endpoint::Workspace,
        cmd: "workspace.reset",
        description: "Resets a workspace to the template it was made from, as one undoable step; its name, description and icon stay",
        params: &[WORKSPACE, GROUP, DRY_RUN, CONFIG],
    },
    Tool {
        name: "helios_workspace_import_parts",
        endpoint: Endpoint::Workspace,
        cmd: "workspace.import_parts",
        description: "Copies parts of another workspace or template into a workspace, as one undoable step: the network as an opened composite under a free name, panels, keys, tools, library settings. A dry run returns the changes and the conflicts; each conflict needs a side in `picks` (ours or theirs) before it applies. Nothing records where the parts came from",
        params: &[
            WORKSPACE,
            SOURCE,
            Param {
                name: "parts",
                schema: strings,
                description: "network, layout, keys, toolbox, library; every part when empty",
                default: Some(empty_list),
            },
            Param {
                name: "picks",
                schema: object,
                description: "The side kept for each conflict, by its id (view, panel:<id>, key:<keys>, library): ours or theirs",
                default: Some(empty_object),
            },
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_network_delegate",
        endpoint: Endpoint::Workspace,
        cmd: "network.delegate",
        description: "Delegates nodes of a workspace's network to a new sub-workspace, as one undoable step: a named scope (one network, nothing changes in the render), nested in the innermost sub-workspace holding all the nodes, the ones beside it whose nodes are all given moving inside. Nodes crossing a sub-workspace's edge are refused. Answers its id (a slug of the name, never reused), its parent and the sub-workspaces moved (adopted)",
        params: &[
            WORKSPACE,
            Param {
                name: "name",
                schema: string,
                description: "The sub-workspace's name, in the user's words",
                default: None,
            },
            Param {
                name: "nodes",
                schema: strings,
                description: "Nodes of the network (a node's name, or an opened composite's inner node: retouch.mask)",
                default: None,
            },
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_macro_make",
        endpoint: Endpoint::Workspace,
        cmd: "macro.make",
        description: "Packages nodes of a workspace's network as one macro node, as one undoable step: a composite file `user.<slug of the name>@1` joins the workspace, its inputs the wires reaching the nodes, its outputs those read outside or shown; the parameters the image changed are exposed (or those in `expose`), their values moved to the macro. Refused for a source, a bypassed node, a node outside that reads the nodes and feeds them, nodes crossing a sub-workspace's edge. Through the desktop, the open image's values move in the same undo step; on the files, those of `image` in `vault`, saved as a version of its edit",
        params: &[
            WORKSPACE,
            Param {
                name: "nodes",
                schema: strings,
                description: "The nodes to package; through the desktop, the ones selected in the graph when empty",
                default: Some(empty_list),
            },
            Param {
                name: "name",
                schema: string,
                description: "The macro's name, in the user's words",
                default: None,
            },
            optional(
                "expose",
                optional_objects,
                "The parameters exposed: each `name`, `node`, `param`, and optionally `label`, `description`, `unit`, `group`, `min`, `max`, `soft`, `options`, `default`; the ones the image changed when null",
            ),
            MACRO_IMAGE,
            MACRO_VAULT,
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_macro_open",
        endpoint: Endpoint::Workspace,
        cmd: "macro.open",
        description: "Opens a macro in place, as one undoable step: its inner nodes join the network as `<node>.<inner>`, wired as inside; its exposed values keep their paths. Refused for a module, a macro already open or bypassed",
        params: &[
            WORKSPACE,
            MACRO_NODE,
            MACRO_IMAGE,
            MACRO_VAULT,
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_macro_fold",
        endpoint: Endpoint::Workspace,
        cmd: "macro.fold",
        description: "Folds an opened macro back into one node, as one undoable step. What the image changed inside that the macro's file does not hold (an inner parameter it does not expose, an inner node bypassed) is a change to the macro itself, settled by `changes`: `version` saves it as the file's next version (a file is never rewritten), this node pinned to it while the macro's other nodes keep theirs, offered as `updates`; `expose` makes the changed parameters exposed ones of that next version, their values kept on the image; `discard` takes them out; `keep` leaves it open. Without `changes`, a changed macro is refused on the files and asked in a dialog on the desktop",
        params: &[
            WORKSPACE,
            MACRO_NODE,
            optional(
                "changes",
                optional_string,
                "What to do with the changes made while open: `version`, `expose`, `discard` or `keep`; asked (desktop) or refused (files) when null and the macro changed",
            ),
            MACRO_IMAGE,
            MACRO_VAULT,
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_macro_dissolve",
        endpoint: Endpoint::Workspace,
        cmd: "macro.dissolve",
        description: "Gives a macro way to its inner nodes as ordinary ones, as one undoable step: named as inside when free, wired as the macro was, its exposed values written to them; its file stays in the workspace",
        params: &[
            WORKSPACE,
            MACRO_NODE,
            MACRO_IMAGE,
            MACRO_VAULT,
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_macro_upgrade",
        endpoint: Endpoint::Workspace,
        cmd: "macro.upgrade",
        description: "Moves a macro to another version of its file (the newest when `version` is null), as one undoable step: its values carried by name, clamped into a narrower range or dropped, the answer says which. Refused while it is open. Every macro step's answer lists the `updates` on offer: the nodes pinned at an older version than the newest",
        params: &[
            WORKSPACE,
            MACRO_NODE,
            optional(
                "version",
                optional_integer,
                "The version to move to; the newest the catalogue has when null",
            ),
            MACRO_IMAGE,
            MACRO_VAULT,
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_macro_interface",
        endpoint: Endpoint::Workspace,
        cmd: "macro.interface",
        description: "Gives a macro the parameters it shows, as one undoable step: `exposed` is the whole interface, in the order shown. The interface is saved as the next version of the macro's file (a shipped macro becomes a user's copy), this node pinned to it, other nodes keeping theirs. Values follow the inner parameters: a renamed one keeps its value, one no longer exposed is written into the file. Refused while the macro is open, or when an entry does not fit its inner parameter (a range wider than the inner one, options not among its own)",
        params: &[
            WORKSPACE,
            MACRO_NODE,
            Param {
                name: "exposed",
                schema: objects,
                description: "The parameters shown: each `name`, `node` (an inner node), `param`, and optionally `label`, `description`, `unit`, `group` (a section), `min`, `max` (within the inner range), `soft` ([lo, hi]), `options` (a subset of a choice's), `default` (tagged by kind: {\"float\": 0.5}); one name on several entries links them",
                default: None,
            },
            MACRO_IMAGE,
            MACRO_VAULT,
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_macro_publish",
        endpoint: Endpoint::Workspace,
        cmd: "macro.publish",
        description: "Publishes a workspace's macro into the user's library (`<config>/composites/`), where every workspace adds it from the graph's Add menu: its file at the version its node pins (or the composite `id`, `id@version`), with the macros it nests. Never undone (the file stays; removing it from the folder takes it back) and never rewritten: a version already there is refused, a change to the macro is published as its next version. Needs the user's confirmation: through the desktop, a dialog asks them and the answer says `asked` (nothing is written until they accept); on the files, only a dry run answers. Refused for a module, a macro open, a composite Helios ships",
        params: &[
            WORKSPACE,
            optional(
                "node",
                optional_string,
                "The macro node whose version is published; the selected one on the desktop when null and `id` is null",
            ),
            optional(
                "id",
                optional_string,
                "Or a composite of the workspace by id (its newest version) or `id@version`",
            ),
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workshop_apply",
        endpoint: Endpoint::Workspace,
        cmd: "workshop.apply",
        description: "Experimental (interface.experimental.workshops, refused with that key when off): applies a workshop, a macro whose file carries an intent and relative targets, after a node of the workspace's network, as one undoable step: its node (named after it) reads that node's output and every reader of it the workshop's output fits reads the workshop instead, the view too when it showed that output; its targets (a median brightness, highlights, clipping) are aimed on a 256 px proxy of the open image and their values are the image's, in the same step. Refused for a macro without a workshop block, a workshop with other than one input and one output, and on the files: it needs the desktop holding the workspace, with an image open to aim",
        params: &[
            WORKSPACE,
            Param {
                name: "workshop",
                schema: string,
                description: "The workshop, `id@version` (helios.lift-midtones@1)",
                default: None,
            },
            optional(
                "after",
                optional_string,
                "The node whose output it reads; the node the image is shown from when null",
            ),
            Param {
                name: "aim",
                schema: boolean,
                description: "Aim its targets on the open image; false applies it at its defaults",
                default: Some(yes),
            },
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workshop_aim",
        endpoint: Endpoint::Workspace,
        cmd: "workshop.aim",
        description: "Experimental (interface.experimental.workshops, refused with that key when off): aims an applied workshop's targets again on the open image, as one value step on the image's edit (nothing in the workspace changes): what feeds the workshop's node is rendered on a 256 px proxy and each target's parameter is solved there, the node's other values as they are now. Answers the values and, per target, whether it was reached. Refused for a node that is not a workshop, without an image open, and on the files: it needs the desktop holding the workspace",
        params: &[
            WORKSPACE,
            optional(
                "node",
                optional_string,
                "The workshop's node; the selected one when null",
            ),
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workshop_save",
        endpoint: Endpoint::Workspace,
        cmd: "workshop.save",
        description: "Experimental (interface.experimental.workshops, refused with that key when off): saves a macro as a workshop, as one undoable step: its next version (a user's macro n+1, a copy under `user.` for a shipped one) carries the intent, keywords and relative targets, the node pinned to it, the image's values kept. Each target is on an exposed number parameter: a measure (luma.median and luma.high in stops around 1.0, clip.high in %) and the value the image should measure at the workshop's output. Publishing it to the library stays helios_macro_publish, which asks the user. Refused on the files: it needs the desktop holding the workspace",
        params: &[
            WORKSPACE,
            optional(
                "node",
                optional_string,
                "The macro's node; the selected one when null",
            ),
            Param {
                name: "intent",
                schema: string,
                description: "What it is for, in English (Darker, for moody shots)",
                default: None,
            },
            optional("keywords", optional_strings, "English words it is found by"),
            optional(
                "targets",
                optional_objects,
                "The relative targets, in the order they are aimed: [{\"param\": \"brightness\", \"measure\": \"luma.median\", \"value\": -2.5}]",
            ),
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workshop_suggest",
        endpoint: Endpoint::Workspace,
        cmd: "workshop.suggest",
        description: "Experimental (interface.experimental.workshops, refused with that key when off): finds workshops (macros with an intent and relative targets) in the workspace's catalogue, shipped, the user's library and the workspace's own, by what the user wants: a word matches a word of a workshop that starts with it; ranked by where the first match is (the name, then the keywords, then the intent), then by how many words matched. Each with `matched`, where it comes `from` and the nodes it is `applied` as. Refused on the files: it needs the desktop holding the workspace",
        params: &[
            WORKSPACE,
            Param {
                name: "intent",
                schema: string,
                description: "What the user wants, in English words (brighter midtones); empty lists every workshop",
                default: None,
            },
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workshop_context",
        endpoint: Endpoint::Workspace,
        cmd: "workshop.context",
        description: "Experimental (interface.experimental.workshops, refused with that key when off): what an agent reads to design a workshop for the user's words (the design session the user started in the Workshops panel, else `intent`): the open image's measures on a 256 px proxy (median, highlights, clipping, contrast, whole and in two candidate zones it may reuse), the module catalogue with each parameter's range and description, the bricks' descriptions, the workshops the words already find, and how to write the draft. Pixels leave only when the user switched workshops.ai.send_proxy on for this session (asked with `proxy`); otherwise the answer says so. Helios stores no key and calls no model: you answer with helios_workshop_design. Refused on the files: it needs the desktop holding the workspace",
        params: &[
            WORKSPACE,
            optional(
                "intent",
                optional_string,
                "The user's words; the design session's when null",
            ),
            Param {
                name: "proxy",
                schema: boolean,
                description: "Ask for the proxy as a PNG (base64); given only when the user allowed it",
                default: Some(no),
            },
            optional(
                "modules",
                optional_strings,
                "Only these modules of the catalogue (all when null)",
            ),
            optional(
                "after",
                optional_string,
                "The node the workshop would read; the node the image is shown from when null",
            ),
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workshop_design",
        endpoint: Endpoint::Workspace,
        cmd: "workshop.design",
        description: "Experimental (interface.experimental.workshops, refused with that key when off): posts a workshop draft for the user's words, checked by Helios without any AI on a proxy of the open image, then applied as one undoable step. The check answers `check`: schema and catalogue (refused with each problem: an unknown module, a value out of range, a target on an unknown zone), each slider rendered at both ends (`dropped` when it changes nothing, `narrowed` when an end clips), each zone's share (`empty`, `whole`), and `notes` in sentences; a refused draft is not applied, post it again. Applied: the workshop saved as user.<slug of its name>@1, inserted after the node the image is shown from, its targets aimed, its node selected; a new draft of the same session replaces the former one. Refused on the files: it needs the desktop holding the workspace, with an image open",
        params: &[
            WORKSPACE,
            Param {
                name: "draft",
                schema: object,
                description: "{\"intent\": the words, \"composite\": a composite file with a workshop block (zones, targets; format, id, version, inputs and outputs may be left out), \"rationale\": {\"workshop\", \"nodes\", \"exposed\", \"zones\", \"targets\": a sentence per name}}",
                default: None,
            },
            optional(
                "intent",
                optional_string,
                "The user's words; the design session's when null",
            ),
            Param {
                name: "apply",
                schema: boolean,
                description: "Apply it when the check passes; false only checks it",
                default: Some(yes),
            },
            optional(
                "replace",
                optional_string,
                "The node of a former design to replace; the session's when null",
            ),
            optional(
                "after",
                optional_string,
                "The node it reads; the node the image is shown from when null",
            ),
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workshop_session",
        endpoint: Endpoint::Workspace,
        cmd: "workshop.session",
        description: "Experimental (interface.experimental.workshops, refused with that key when off): the record of the workshop session going on. Recording is always on: designing or applying a workshop starts a session, and each value step on its node is a step of the record (`steps`: `seq`, `author` user or agent:<id>, `kind` design, apply, set, propose, accept, override, reject, undo, redo, the `aim` words, the `changes` param, old and new, and the zone `measures` before and after on the proxy); `net` is each parameter's value after the whole session. A session the user does not keep is dropped when it ends; only the user keeps it. Refused on the files: it needs the desktop holding the workspace",
        params: &[
            WORKSPACE,
            Param {
                name: "action",
                schema: session_action,
                description: "read (the record), keep (the user's alone) or close (ends it: dropped unless kept)",
                default: Some(read),
            },
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workshop_propose",
        endpoint: Endpoint::Workspace,
        cmd: "workshop.propose",
        description: "Experimental (interface.experimental.workshops, refused with that key when off): proposes values for the workshop being piloted, while the user refines it (a session starts when a workshop is designed or applied; helios_workshop_session reads it). Nothing is set: the values are shown as ghost values on the workshop's sliders, and the user accepts one (one undo step), overrides it by moving the slider, or rejects it; the proposal and each answer are steps of the session's record, with their author. Refused for a slider the workshop lacks or a value outside its range. Refused on the files: it needs the desktop holding the workspace",
        params: &[
            WORKSPACE,
            Param {
                name: "values",
                schema: object,
                description: "The proposed values, by the workshop's slider names: {\"backlight_lift\": 0.8}",
                default: None,
            },
            optional(
                "aim",
                optional_string,
                "What the proposal answers, in words (the user's ask: the climber's face is too dark)",
            ),
            optional(
                "session",
                optional_string,
                "The session it is for; the one going on when null",
            ),
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workshop_generalise",
        endpoint: Endpoint::Workspace,
        cmd: "workshop.generalise",
        description: "Experimental (interface.experimental.workshops, refused with that key when off): \"Make it automatic\" for the workshop session going on. From the values the session ended with on its test photo, one row per slider it set: a relative target where a measure explains the value (`target`, with every candidate tried and its error as evidence), else a fixed value (`fixed`), or kept as AI (`ai`, a value the AI proposed that nothing explains); and one row per zone the AI draws: a fitted parametric mask (`parametric`, its overlap per photo) or kept as AI. Each row has a `default` and a `note`. `J` is the junctions left, `A` the AI calls expected per photo. The rows are shown in the Workshops panel. `rows` amends the choices; `confirm` writes the workshop's next version (the targets, the fixed values as the sliders' defaults, the fitted zones) as one undoable step and keeps the session: the user's alone, an agent proposes. A row left as AI cannot be written yet. Refused on the files: it needs the desktop holding the workspace, with an image open",
        params: &[
            WORKSPACE,
            optional(
                "rows",
                optional_object,
                "The choices to amend, by slider or zone name: {\"backlight_lift\": \"target\", \"haze\": \"fixed\", \"sky\": \"parametric\"}; each row's default when null",
            ),
            Param {
                name: "confirm",
                schema: boolean,
                description: "Write the next version with these choices (the user's alone; refused for an agent)",
                default: Some(no),
            },
            optional(
                "session",
                optional_string,
                "The session it is for; the one going on when null",
            ),
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workshop_review",
        endpoint: Endpoint::Workspace,
        cmd: "workshop.review",
        description: "Experimental (interface.experimental.workshops, refused with that key when off): the review of a workshop run the user started on the desktop. Lists the run's photos, those to review first (flagged, held, stopped, failed), each with its `state` (applied, held, stopped, unchanged, failed, accepted, reverted, fixed), its `flags`, each value's `provenance` (aimed, ai, fallback, fixed, user) and its values `before` and `after`; `counts` (photos, flagged, held, AI calls, per flag); `pending`, the junctions left to their fallback; `rerun`, the photos a re-run takes. Actions on one photo (`image`), each one undo step: `decide` answers a pending junction (`junction`, `value`), recorded in the workshop's decision book with you as its author, so a re-run takes it; `fix` sets the workshop's values by hand (`values`), a new version with provenance user, their flags cleared. `accept` (a held photo), `revert` (a written one) and `rerun` are the user's alone. No event announces a pending junction yet: read `pending` after a run. Refused on the files: it needs the desktop holding the workspace",
        params: &[
            WORKSPACE,
            optional(
                "run",
                optional_string,
                "The run's id (workshop.run's answer); the last one started on the desktop when null",
            ),
            optional(
                "action",
                review_action,
                "list (the default), decide, fix; accept, revert and rerun are the user's",
            ),
            optional("image", optional_string, "The photo's id, for an action"),
            optional(
                "flag",
                optional_string,
                "Only the photos raising this flag (low_confidence, fallback, target_missed…)",
            ),
            optional("junction", optional_string, "decide: the junction's name"),
            optional("value", optional_number, "decide: the junction's value"),
            optional(
                "values",
                optional_object,
                "fix: the workshop's values by slider name: {\"lift\": 0.02}",
            ),
            CONFIG,
        ],
    },
    Tool {
        name: "helios_network_connect",
        endpoint: Endpoint::Workspace,
        cmd: "network.connect",
        description: "Connects an output of a workspace's network to an input, as one undoable step: the input's wire (and the wires in `replaces`) removed first. Refused when a port is not there, the types differ or the wire would close a loop",
        params: &[
            WORKSPACE,
            Param {
                name: "from",
                schema: string,
                description: "The node (or source) whose output feeds",
                default: None,
            },
            Param {
                name: "port",
                schema: string,
                description: "Its output (a source's is `image`)",
                default: None,
            },
            Param {
                name: "to",
                schema: string,
                description: "The node fed",
                default: None,
            },
            Param {
                name: "input",
                schema: string,
                description: "Its input",
                default: None,
            },
            Param {
                name: "replaces",
                schema: objects,
                description: "Other wires taken out first, each {from, port, to, input}",
                default: Some(empty_list),
            },
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_network_disconnect",
        endpoint: Endpoint::Workspace,
        cmd: "network.disconnect",
        description: "Removes wires of a workspace's network, as one undoable step; every one must be there",
        params: &[
            WORKSPACE,
            Param {
                name: "connections",
                schema: objects,
                description: "The wires, each {from, port, to, input}",
                default: None,
            },
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_network_move",
        endpoint: Endpoint::Workspace,
        cmd: "network.move",
        description: "Moves nodes of a workspace's network in the graph view, as one undoable step",
        params: &[
            WORKSPACE,
            Param {
                name: "moves",
                schema: objects,
                description: "Each {node, to: [x, y]}, in the graph's units",
                default: None,
            },
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_network_add",
        endpoint: Endpoint::Workspace,
        cmd: "network.add",
        description: "Adds nodes to a workspace's network, unconnected, as one undoable step: a module by its name, a composite by `id@version`, or a copy of a node (its module or composite and its parameters). Answers the names given (`added`), made unique (exposure_2)",
        params: &[
            WORKSPACE,
            Param {
                name: "nodes",
                schema: objects,
                description: "Each {module}, {composite} or {copy_of}, with `at`: [x, y] in the graph view",
                default: None,
            },
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_network_remove",
        endpoint: Endpoint::Workspace,
        cmd: "network.remove",
        description: "Removes nodes from a workspace's network with their wires, as one undoable step; a source (the workspace's image) and the node the viewport shows stay",
        params: &[
            WORKSPACE,
            Param {
                name: "nodes",
                schema: strings,
                description: "The nodes' names",
                default: None,
            },
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_network_paste",
        endpoint: Endpoint::Workspace,
        cmd: "network.paste",
        description: "Pastes copied nodes into a workspace's network, as one undoable step: a name the network holds already is made unique, the wires rewritten with the new names, a wire coming in wired again when the node it came from is there, the places shifted. Answers the names given (`added`) and those changed (`renamed`)",
        params: &[
            WORKSPACE,
            Param {
                name: "clip",
                schema: object,
                description: "A `helios.nodes` payload, as Edit > Copy in the graph view makes it: {nodes, connections, inputs, positions}",
                default: None,
            },
            optional(
                "parts",
                optional_strings,
                "The nodes taken by name, and `inputs` for the wires coming in; all when null",
            ),
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_subworkspace_delete",
        endpoint: Endpoint::Workspace,
        cmd: "subworkspace.delete",
        description: "Deletes a sub-workspace of a workspace, as one undoable step: its nodes stay in the network, the sub-workspaces nested in it move up to its parent, and its id is never given again",
        params: &[
            WORKSPACE,
            Param {
                name: "id",
                schema: string,
                description: "The sub-workspace's id",
                default: None,
            },
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workspace_save_as_template",
        endpoint: Endpoint::Workspace,
        cmd: "workspace.save_as_template",
        description: "Saves a workspace as a template among the user's: a review of what goes in (network, layout, keys, toolbox, library), what is stripped (images, edit history, absolute paths, secrets, embedded data) and what is asked on create (paths, the import source); validated, versioned, with its English and French text. A dry run returns the review only",
        params: &[
            WORKSPACE,
            Param {
                name: "id",
                schema: string,
                description: "The template's id: lowercase words joined by '.' or '-' (user.wedding)",
                default: None,
            },
            Param {
                name: "name",
                schema: string,
                description: "Its name, in English",
                default: None,
            },
            Param {
                name: "description",
                schema: string,
                description: "The scenario it is made for, in English",
                default: None,
            },
            optional("icon", optional_string, "A short generic icon name"),
            optional(
                "version",
                optional_integer,
                "Its version; 1 when null, and above the one saved to replace it",
            ),
            optional(
                "translations",
                optional_object,
                "Its name and description in other languages, by locale: {\"fr\": {\"name\": …, \"description\": …}}",
            ),
            optional(
                "without",
                optional_strings,
                "Parts left out: keys, toolbox, library",
            ),
            optional(
                "ask",
                optional_strings,
                "The values asked on create, by id; every one proposed when null",
            ),
            Param {
                name: "overwrite",
                schema: boolean,
                description: "Replace a template of that id whatever its version",
                default: Some(no),
            },
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_workspace_undo",
        endpoint: Endpoint::Workspace,
        cmd: "workspace.undo",
        description: "Undoes the last step these tools made on a workspace",
        params: &[WORKSPACE, CONFIG],
    },
    Tool {
        name: "helios_context_read",
        endpoint: Endpoint::Workspace,
        cmd: "context.read",
        description: "A workspace's places and modes (the desktop's that has it open, else what a desktop opening it comes back to): the place path (the library, the darkroom on an image, its network, a sub-workspace), the modes on (culling, mask editing, compare), the revision, every frame it may hold, and its sub-workspaces (`id`, `name`, `parent`)",
        params: &[WORKSPACE, CONFIG],
    },
    Tool {
        name: "helios_context_request",
        endpoint: Endpoint::Workspace,
        cmd: "context.request",
        description: "Any transition of a workspace's places and modes, as egui-pop's context.request takes it: `request` with `op` enter (`frame`, `payload`), leave, jump (`depth`), go (`places`), set_mode or push_mode (`mode`), pop_mode, clear_mode (`group`) or back; refused when the stack is no longer at `if_rev`; a dry run answers without writing",
        params: &[
            WORKSPACE,
            Param {
                name: "request",
                schema: object,
                description: "The transition: {\"op\": \"enter\", \"frame\": \"helios.library\"}, {\"op\": \"back\"}...",
                default: None,
            },
            optional(
                "if_rev",
                optional_integer,
                "Refused unless the stack is still at this revision",
            ),
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_context_go",
        endpoint: Endpoint::Workspace,
        cmd: "context.go",
        description: "Goes to a place path of a workspace: `places`, each a `frame` (helios.library, helios.darkroom, helios.workspace, helios.sub-workspace) with its `payload` (the darkroom's {\"image\": id} or {\"file\": path}, the workspace's key, a sub-workspace's {\"id\": id} under its parent's)",
        params: &[
            WORKSPACE,
            Param {
                name: "places",
                schema: objects,
                description: "The path from the root: [{\"frame\": \"helios.library\"}, {\"frame\": \"helios.darkroom\", \"payload\": {\"image\": \"…\"}}]",
                default: None,
            },
            optional(
                "if_rev",
                optional_integer,
                "Refused unless the stack is still at this revision",
            ),
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_context_set_mode",
        endpoint: Endpoint::Workspace,
        cmd: "context.set_mode",
        description: "Sets a mode of a workspace (`mode`: helios.culling, helios.compare...), pushes it for a moment (`push`), or turns a group off (`group`: orientation, tool, compare; no `mode`)",
        params: &[
            WORKSPACE,
            optional("mode", optional_string, "The mode to turn on"),
            Param {
                name: "push",
                schema: boolean,
                description: "Pushed (popped by back) rather than set",
                default: Some(no),
            },
            optional(
                "group",
                optional_string,
                "The group to turn off, without `mode`",
            ),
            optional(
                "if_rev",
                optional_integer,
                "Refused unless the stack is still at this revision",
            ),
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_clipboard",
        endpoint: Endpoint::Workspace,
        cmd: "clipboard.state",
        description: "What a clipboard slot holds on the desktop that has the workspace open: its type, title, formats, revision and what was stripped from it; with `target` (an endpoint), every way it fits there, each with its preview; with `payload`, the item's data too",
        params: &[
            WORKSPACE,
            SLOT,
            optional(
                "target",
                optional_string,
                "An endpoint to fit the item to: helios.param, helios.graph, helios.masks or helios.library",
            ),
            Param {
                name: "payload",
                schema: boolean,
                description: "Answer the item's data too",
                default: Some(no),
            },
            CONFIG,
        ],
    },
    Tool {
        name: "helios_copy",
        endpoint: Endpoint::Workspace,
        cmd: "clipboard.copy",
        description: "Copies into a clipboard slot on the desktop holding the workspace: from an endpoint's selection (`source`), or an item given whole (`format` and `data`: a Helios type such as application/x-helios.param+json, or text/plain; a `title`). `cut` deletes what was copied, as one undo step. A dry run says what would be copied and left out",
        params: &[
            WORKSPACE,
            optional(
                "source",
                optional_string,
                "The endpoint copied from: helios.param, helios.graph, helios.masks or helios.library; null with `format` and `data`",
            ),
            optional(
                "format",
                optional_string,
                "The item's format, when it is given whole",
            ),
            optional("data", anything, "The item's data, as its format holds it"),
            optional("title", optional_string, "What the item is called"),
            Param {
                name: "cut",
                schema: boolean,
                description: "Delete what was copied (an endpoint that can)",
                default: Some(no),
            },
            SLOT,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_paste",
        endpoint: Endpoint::Workspace,
        cmd: "clipboard.paste",
        description: "Pastes a clipboard slot into an endpoint of the desktop holding the workspace (`target`), in the richest format it accepts or the one given, through a `transform`, only some `parts` (paste special's); refused unless the clipboard is still at `if_rev`; one undo step. A dry run answers the plan, the transform, what would be stripped and lost, and the target's changes, without making them. A paste into the graph or the masks is applied at the desktop's next frame: its answer is the plan",
        params: &[
            WORKSPACE,
            Param {
                name: "target",
                schema: string,
                description: ENDPOINTS,
                default: None,
            },
            optional(
                "format",
                optional_string,
                "The format pasted; the richest the target accepts when null",
            ),
            optional(
                "transform",
                optional_string,
                "A transform to go through (helios.mask-as-nodes)",
            ),
            optional(
                "parts",
                optional_strings,
                "Only these parts (paste special's); all when null",
            ),
            optional(
                "if_rev",
                optional_integer,
                "Refused unless the clipboard is still at this revision",
            ),
            SLOT,
            GROUP,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_context_hold",
        endpoint: Endpoint::Workspace,
        cmd: "context.hold",
        description: "Holds the places and modes of the desktop holding a workspace open (egui-pop's context.hold), so that nobody moves them while an agent works: a transition touching the held groups is refused with `reason` until the hold is released (helios_context_release) or the window closes. Its `token`. Refused without such a desktop",
        params: &[
            WORKSPACE,
            optional(
                "groups",
                optional_strings,
                "The groups held: `place` for the places, a mode group (orientation, tool, compare); all of them when null",
            ),
            Param {
                name: "reason",
                schema: string,
                description: "Why, as a refused transition says it: \"an agent is retouching this image\"",
                default: None,
            },
            CONFIG,
        ],
    },
    Tool {
        name: "helios_context_release",
        endpoint: Endpoint::Workspace,
        cmd: "context.release",
        description: "Releases a hold of helios_context_hold on the desktop holding a workspace open (`token`); whether it was held",
        params: &[
            WORKSPACE,
            Param {
                name: "token",
                schema: integer,
                description: "The hold's token, as helios_context_hold answered it",
                default: None,
            },
            CONFIG,
        ],
    },
    Tool {
        name: "helios_history_list",
        endpoint: Endpoint::Workspace,
        cmd: "history.list",
        description: "The histories of the desktop holding a workspace open (egui-pop's history.list): each scope (id, title, local, durable, cap), whether it is active, its revision, head and counts, and where Undo and Redo go now with what they would do",
        params: &[WORKSPACE, CONFIG],
    },
    Tool {
        name: "helios_history_read",
        endpoint: Endpoint::Workspace,
        cmd: "history.read",
        description: "A history's steps on the desktop holding a workspace open (egui-pop's history.read): `undo`, oldest first, and `redo`, the next one last, each with its id, label, text, origin and place; the revision and head; its `snapshots` (id, name, the step it was taken at, and `pending`: true for one kept only at the edit's next save, lost if the desktop quits first)",
        params: &[WORKSPACE, HISTORY_SCOPE, CONFIG],
    },
    Tool {
        name: "helios_history_undo",
        endpoint: Endpoint::Workspace,
        cmd: "history.undo",
        description: "Undoes the last step of a history on the desktop holding a workspace open, as the user's Ctrl+Z would (egui-pop's history.undo), when that step is this agent's own (the user's or another's is refused); with `group`, undoes that batch of this agent's alone, even under later steps, refused when one of them changed the same values. Refused unless the history is still at `if_rev`; a dry run says what it would undo",
        params: &[
            WORKSPACE,
            HISTORY_SCOPE,
            optional(
                "group",
                optional_integer,
                "One of this agent's batches (a step's `group` from helios_history_read, or the `group` an edit answered): undone alone",
            ),
            HISTORY_IF_REV,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_history_redo",
        endpoint: Endpoint::Workspace,
        cmd: "history.redo",
        description: "Redoes the last undone step of a history on the desktop holding a workspace open (egui-pop's history.redo), when it is this agent's own; refused unless the history is still at `if_rev`; a dry run says what it would redo",
        params: &[WORKSPACE, HISTORY_SCOPE, HISTORY_IF_REV, DRY_RUN, CONFIG],
    },
    Tool {
        name: "helios_history_jump",
        endpoint: Endpoint::Workspace,
        cmd: "history.jump",
        description: "Brings a history on the desktop holding a workspace open to one of its steps (`to`, a step's id from helios_history_read; 0: the start) by undos or redos, all or nothing (egui-pop's history.jump), when every step walked is this agent's own; refused unless the history is still at `if_rev`; a dry run says which steps it would walk",
        params: &[
            WORKSPACE,
            HISTORY_SCOPE,
            Param {
                name: "to",
                schema: integer,
                description: "The step's id; 0: before the first step",
                default: None,
            },
            HISTORY_IF_REV,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_history_undo_step",
        endpoint: Endpoint::Workspace,
        cmd: "history.undo_step",
        description: "Undoes one step of a history on the desktop holding a workspace open alone, even under later steps (`step`, a step's id from helios_history_read; egui-pop's history.undo_step, the History panel's \"Undo this step\"), when it is this agent's own; refused when a later step changed the same values, and unless the history is still at `if_rev`; the step goes on the redo list; a dry run says whether it would go",
        params: &[
            WORKSPACE,
            HISTORY_SCOPE,
            Param {
                name: "step",
                schema: integer,
                description: "The step's id",
                default: None,
            },
            HISTORY_IF_REV,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_history_snapshot",
        endpoint: Endpoint::Workspace,
        cmd: "history.snapshot",
        description: "Takes a snapshot of a history on the desktop holding a workspace open (egui-pop's history.snapshot): its current state under `name` (none: numbered). Nothing is saved for it: the image's edit, at its next save, pins it to that version, after which it survives a quit; until then it is pending. Refused during a drag and unless the history is still at `if_rev`; a dry run says what it would take",
        params: &[
            WORKSPACE,
            HISTORY_SCOPE,
            optional(
                "name",
                optional_string,
                "The snapshot's name; none: \"Snapshot <n>\"",
            ),
            HISTORY_IF_REV,
            DRY_RUN,
            CONFIG,
        ],
    },
    Tool {
        name: "helios_history_restore",
        endpoint: Endpoint::Workspace,
        cmd: "history.restore",
        description: "Brings a history on the desktop holding a workspace open back to one of its snapshots (`snapshot`, its id from helios_history_read; egui-pop's history.restore) as one new step of this agent's, which an undo takes back. Refused during a drag and unless the history is still at `if_rev`; a dry run says the step it would make",
        params: &[
            WORKSPACE,
            HISTORY_SCOPE,
            Param {
                name: "snapshot",
                schema: integer,
                description: "The snapshot's id",
                default: None,
            },
            HISTORY_IF_REV,
            DRY_RUN,
            CONFIG,
        ],
    },
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
            description: "graph, engine, scheduler, node, config, modules (with the shipped composites and their workshop blocks) or bus",
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
        name: "helios_node_bypass",
        endpoint: Endpoint::Control,
        cmd: "node.bypass",
        description: "Bypasses a module node, or brings it back: bypassed, it keeps its parameters and renders as if absent, its output the input it passes through. Saved with the edit as a step (debug control)",
        params: &[
            NODE,
            Param {
                name: "on",
                schema: boolean,
                description: "True to bypass, false to bring it back",
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
            Param {
                name: "as_mask",
                schema: boolean,
                description: "A one-channel port (a mask node's `mask`) written as its values, 0 black and 1 white, as the desktop's mask display shows it in grey",
                default: Some(no),
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
        name: "helios_job_cancel",
        endpoint: Endpoint::Control,
        cmd: "job.cancel",
        description: "Cancels a job people follow by its id (`export-3`, `batch-1`, from the job.* events)",
        params: &[Param {
            name: "id",
            schema: string,
            description: "The job's id",
            default: None,
        }],
    },
    Tool {
        name: "helios_cache_stats",
        endpoint: Endpoint::Control,
        cmd: "cache.stats",
        description: "What the node's caches hold: each tier's budget and use; per scope, kind and tier the entries, bytes and statistics (hits, misses, evictions)",
        params: &[],
    },
    Tool {
        name: "helios_cache_clear",
        endpoint: Endpoint::Control,
        cmd: "cache.clear",
        description: "Clears the node's caches (debug control): every kind, or `kind` alone (`render.step`); answers the bytes freed",
        params: &[Param {
            name: "kind",
            schema: string,
            description: "The kind to clear; every kind when left out",
            default: Some(null),
        }],
    },
    Tool {
        name: "helios_calibrate_chart",
        endpoint: Endpoint::Control,
        cmd: "calibrate.chart",
        description: "Measures a colour chart shot (a decoded picture: TIFF, PNG, JPEG, or a raw measured on its sensor data) and solves its colour correction, a dry run that writes nothing unless `save` is set: per patch the measured value, the reference and Delta E 2000 before and after, with its status; mean, max and worst patches; the orientation found, the light estimated from the neutral patches, the exposure offset and the matrices. With `save` (a node with a vault), the result is kept as a calibration profile and its id answered under `profile` (debug control)",
        params: &[
            optional(
                "path",
                optional_string,
                "Absolute path of the chart shot, on the node's machine (or give `image`)",
            ),
            optional(
                "image",
                optional_string,
                "The chart shot in the node's vault: its id or a path the vault knows; a raw is then measured with its edit's raw-front parameters",
            ),
            Param {
                name: "corners",
                schema: corners,
                description: "The centres of the chart's four corner patches, [x, y] in the picture's pixels as stored (a raw's: its sensor data's), in any order",
                default: None,
            },
            optional(
                "chart",
                optional_string,
                "The chart's id: xrite.cc24.2014 (the default) or xrite.cc24.2000",
            ),
            Param {
                name: "exclude",
                schema: strings,
                description: "Patch names left out of the solve (B3)",
                default: Some(empty_list),
            },
            Param {
                name: "dry_run",
                schema: boolean,
                description: "Report only, write nothing (`save` writes the profile)",
                default: Some(yes),
            },
            Param {
                name: "save",
                schema: boolean,
                description: "Keep the result as a calibration profile in the vault",
                default: Some(no),
            },
            optional(
                "name",
                optional_string,
                "The saved profile's name; the camera and the light when left out",
            ),
        ],
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
        name: "helios_workflow_submit",
        endpoint: Endpoint::Control,
        cmd: "workflow.submit",
        description: "Submits a batch as a run of the workflow level: each item exported through the network, claimed under a lease and retried on failure, followed per queueing node (describe workflow). The same network and settings again extend the same run, leaving done items done; each output path is decided now, once. Answers the run's id, its workflow and the counts new, already there, merged",
        params: &[
            optional(
                "title",
                optional_string,
                "The run's title, as the jobs centre shows it",
            ),
            Param {
                name: "items",
                schema: run_items,
                description: "The image files, absolute paths on the node's machine; an item may be {path, params} to change parameters for it only",
                default: None,
            },
            Param {
                name: "out",
                schema: string,
                description: "Absolute path of the folder the files are written to",
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
                name: "name",
                schema: string,
                description: "Output file name, `{stem}` and `{ext}` replaced; a name taken gets -1, -2...",
                default: Some(name_pattern),
            },
            optional(
                "node",
                optional_string,
                "The node exported; the network's first export, else its only end, when null",
            ),
            optional(
                "source",
                optional_string,
                "The source given each item; the network's only source when null",
            ),
            Param {
                name: "again",
                schema: boolean,
                description: "A new run even if the same one exists",
                default: Some(no),
            },
            optional(
                "concurrency",
                optional_integer,
                "Exports of this run at once, counted for it alone; the export node's concurrency when null",
            ),
        ],
    },
    Tool {
        name: "helios_workflow_runs",
        endpoint: Endpoint::Control,
        cmd: "workflow.runs",
        description: "The runs of the node: id, title, state, durable, and per queueing node the counts by status, waiting, upstream, held and leases out",
        params: &[],
    },
    Tool {
        name: "helios_workflow_items",
        endpoint: Endpoint::Control,
        cmd: "workflow.items",
        description: "One page of a run's items at a queueing node, in the order they are worked, with their rows (status, attempts, error) and output paths",
        params: &[
            Param {
                name: "run",
                schema: string,
                description: "The run's id",
                default: None,
            },
            optional(
                "node",
                optional_string,
                "A queueing node; the export when null",
            ),
            optional(
                "state",
                optional_string,
                "Only the items in this state: waiting, upstream, held, running, scheduled, asking (waiting for a person), done, skipped, failed, cancelled, omitted",
            ),
            Param {
                name: "offset",
                schema: integer,
                description: "Items skipped",
                default: Some(zero),
            },
            Param {
                name: "limit",
                schema: integer,
                description: "Items in the page",
                default: Some(hundred),
            },
        ],
    },
    Tool {
        name: "helios_workflow_forget",
        endpoint: Endpoint::Control,
        cmd: "workflow.forget",
        description: "Removes a run that is not at work (finished, or paused with nothing running), with its items and rows, from the vault too; nothing is kept of it. Refused for a run another live node works",
        params: &[RUN],
    },
    Tool {
        name: "helios_workflow_cancel",
        endpoint: Endpoint::Control,
        cmd: "workflow.cancel",
        description: "Cancels a run's items at a queueing node: rows not concluded become cancelled (running exports stopped first, a file is written whole or not at all), items not yet there are never claimed, done rows stay done. Every item and every node when null; the whole run cancelled ends its job cancelled",
        params: &[
            RUN,
            optional(
                "items",
                optional_strings,
                "Item ids (path:...) or absolute paths; every item of the run when null",
            ),
            optional(
                "node",
                optional_string,
                "A queueing node; every one when null",
            ),
        ],
    },
    Tool {
        name: "helios_workflow_retry",
        endpoint: Endpoint::Control,
        cmd: "workflow.retry",
        description: "Retries a run's failed and cancelled items at a queueing node: their rows are scheduled again, due now, with the full allowance of attempts. A finished run is worked again; a paused run stays paused",
        params: &[
            RUN,
            optional(
                "items",
                optional_strings,
                "Item ids (path:...) or absolute paths; every item of the run when null",
            ),
            optional(
                "node",
                optional_string,
                "A queueing node; every one when null",
            ),
        ],
    },
    Tool {
        name: "helios_workflow_prioritize",
        endpoint: Endpoint::Control,
        cmd: "workflow.prioritize",
        description: "Moves a run's items before (top) or after (bottom) every other item of the run in the order they are claimed; items running, or concluded at the export, stay where they are",
        params: &[
            RUN,
            Param {
                name: "items",
                schema: strings,
                description: "Item ids (path:...) or absolute paths",
                default: None,
            },
            Param {
                name: "to",
                schema: move_to,
                description: "top: claimed next; bottom: claimed last",
                default: None,
            },
        ],
    },
    Tool {
        name: "helios_workflow_pause",
        endpoint: Endpoint::Control,
        cmd: "workflow.pause",
        description: "Pauses a run: nothing more is claimed, running items finish. A paused run stays paused after the node restarts",
        params: &[RUN],
    },
    Tool {
        name: "helios_workflow_resume",
        endpoint: Endpoint::Control,
        cmd: "workflow.resume",
        description: "Resumes a paused run: its items are claimed again",
        params: &[RUN],
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
    /// Whether the tool changes something: its request then carries a
    /// key (`_meta.request_id`, the `request_id` argument or a new one),
    /// so it may be sent again after an unknown outcome and is applied
    /// once (`helios_inspect::request_ids`).
    pub fn changes(&self) -> bool {
        match self.endpoint {
            Endpoint::Control => !CONTROL_READS.contains(&self.cmd),
            Endpoint::Commands => self.cmd == "command.run",
            Endpoint::Inspect | Endpoint::Sandbox | Endpoint::Ai | Endpoint::Workspace => false,
        }
    }

    pub fn input_schema(&self) -> Value {
        let mut properties: Map<String, Value> = self
            .params
            .iter()
            .map(|p| {
                let mut schema = (p.schema)();
                schema["description"] = json!(p.description);
                (p.name.to_string(), schema)
            })
            .collect();
        if self.changes() {
            properties.insert(
                REQUEST_ID.into(),
                json!({
                    "type": "string",
                    "minLength": 1,
                    "maxLength": helios_inspect::request_ids::LONGEST,
                    "description": "This change's key. Given again after an outcome unknown, the node applies the change once and answers `already_applied`. A new one when left out.",
                }),
            );
        }
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
        if let Some(unknown) = given.keys().find(|k| {
            !self.params.iter().any(|p| p.name == *k) && !(self.changes() && *k == REQUEST_ID)
        }) {
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
        if self.changes() {
            let key = match given.get(REQUEST_ID) {
                None => helios_inspect::request_ids::new_request_id(),
                Some(key) => key
                    .as_str()
                    .ok_or_else(|| format!("{}: `{REQUEST_ID}` is a string", self.name))?
                    .to_string(),
            };
            request.insert("_meta".into(), json!({ REQUEST_ID: key }));
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
    /// The folders the agent may read and write (`crate::roots`).
    pub roots: Roots,
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
        Ok(answer) => answer_result(answer, false),
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
                .and_then(|state| {
                    sockets.inspect = Some(state.inspect_address()?);
                    sockets.control = if cfg!(debug_assertions) {
                        Some(state.control_address()?)
                    } else {
                        None
                    };
                    Ok(serde_json::to_value(state).expect("state serializes"))
                })
        }
        _ => crate::sandbox::stop(&dir).map(|ran| json!({ "stopped": ran })),
    };
    match result {
        Ok(answer) => answer_result(answer, false),
        Err(e) => tool_result(e, true),
    }
}

/// The argument that keys a change (`Tool::changes`).
const REQUEST_ID: &str = "request_id";

/// The control tools that only read: their requests carry no key.
const CONTROL_READS: &[&str] = &[
    "cache.stats",
    "decisions",
    "workflow.runs",
    "workflow.items",
];

/// What an agent may do after an unknown outcome of `request`: send it
/// again with its key, when it has one.
fn resend_hint(request: &Value) -> String {
    request["_meta"][REQUEST_ID]
        .as_str()
        .map_or_else(String::new, |key| {
            format!(
                "; or send it again with `{REQUEST_ID}` \"{key}\": the node applies it once and answers `already_applied` if it ran"
            )
        })
}

/// Sends `request` to the node at `address`. A request sent whose answer
/// never came says its outcome is unknown, rather than that the node
/// could not be reached: the agent reads the state before resending it,
/// or resends it with its key.
fn send(address: &Address, request: &Value) -> Result<Value, String> {
    request_at(address, request).map_err(|e| {
        if helios_inspect::unanswered(&e) {
            format!(
                "outcome unknown at the node at {address}: {e}{}",
                resend_hint(request)
            )
        } else {
            format!("cannot reach the node at {address}: {e}")
        }
    })
}

/// A tool's text result (an error, a message), cut to the answer's cap.
fn tool_result(text: String, is_error: bool) -> Value {
    json!({ "content": caps::text_content(text), "isError": is_error })
}

/// A tool's JSON answer, cut to the caps and with its untrusted text
/// marked (`crate::caps`).
fn answer_result(answer: Value, is_error: bool) -> Value {
    json!({ "content": caps::answer_content(answer), "isError": is_error })
}

/// The workspace tools that only read the workspace's files.
const WORKSPACE_READS: &[&str] = &[
    "workspace.parts",
    "workspace.state",
    "context.read",
    "clipboard.state",
    "history.list",
    "history.read",
];

/// The workspace tools that write into the configuration directory.
const CONFIG_WRITES: &[&str] = &[
    "workspace.save_as_template",
    "macro.publish",
    "workshop.save",
];

/// Whether a text reads as a path rather than an id (a template's
/// `source`, a run's item): absolute, or holding a separator.
fn path_like(text: &str) -> bool {
    Path::new(text).is_absolute() || text.contains('/') || text.contains('\\')
}

/// Refuses a request naming a path outside the agent's roots: each path
/// argument with the access its tool makes. A command run through the
/// generic pair does not say what it does with a path, so every absolute
/// path in its params needs the write root.
fn confine(roots: &Roots, tool: &Tool, request: &Value) -> Result<(), String> {
    if !roots.is_confined() {
        return Ok(());
    }
    let cmd = tool.cmd;
    let check = |text: &str, access: Access| roots.check(Path::new(text), access).map(|_| ());
    for param in tool.params {
        let value = &request[param.name];
        // One key per arm, each said on its own, even where two give the
        // same access.
        #[allow(clippy::match_same_arms)]
        let access = match param.name {
            "path" if cmd == "export" => Access::Write,
            // A template's id or a workspace's folder; elsewhere a node or
            // an endpoint's name.
            "source" if tool.endpoint == Endpoint::Workspace && !cmd.starts_with("clipboard.") => {
                Access::Read
            }
            "path" | "file" => Access::Read,
            "dir" | "out" | "vault" => Access::Write,
            "workspace" if WORKSPACE_READS.contains(&cmd) => Access::Read,
            "workspace" => Access::Write,
            "config" if CONFIG_WRITES.contains(&cmd) => {
                // Left out, the user's own: written there only if granted.
                let config = value
                    .as_str()
                    .map_or_else(helios_template::workspace::config_dir, PathBuf::from);
                roots.check(&config, Access::Write)?;
                continue;
            }
            "config" => Access::Read,
            "items" => {
                for item in value.as_array().into_iter().flatten() {
                    let path = item.as_str().or_else(|| item["path"].as_str());
                    if let Some(path) = path.filter(|p| Path::new(p).is_absolute()) {
                        check(path, Access::Read)?;
                    }
                }
                continue;
            }
            "params" if cmd == "command.run" => {
                let mut paths = Vec::new();
                absolute_paths(value, &mut paths);
                for path in paths {
                    check(path, Access::Write)?;
                }
                continue;
            }
            _ => continue,
        };
        match value.as_str().filter(|s| !s.is_empty()) {
            Some(text) if param.name == "source" && !path_like(text) => {}
            Some(text) => check(text, access)?,
            None => {}
        }
    }
    Ok(())
}

/// Every string of `value` that is an absolute path, at any depth.
fn absolute_paths<'a>(value: &'a Value, into: &mut Vec<&'a str>) {
    match value {
        Value::String(s) if Path::new(s).is_absolute() => into.push(s),
        Value::Array(items) => items.iter().for_each(|v| absolute_paths(v, into)),
        Value::Object(map) => map.values().for_each(|v| absolute_paths(v, into)),
        _ => {}
    }
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
    if let Err(e) = caps::check_limit(&request) {
        return tool_result(format!("{}: {e}", tool.name), true);
    }
    if let Err(e) = confine(&sockets.roots, tool, &request) {
        return tool_result(format!("{}: {e}", tool.name), true);
    }
    if tool.endpoint == Endpoint::Sandbox {
        return sandbox(sockets, tool, &request);
    }
    if tool.endpoint == Endpoint::Ai {
        return ai_call(sockets, &request);
    }
    if tool.endpoint == Endpoint::Commands {
        return match commands_client(sockets, &request) {
            Ok(client) => commands_call(&client, &request),
            Err(e) => tool_result(e, true),
        };
    }
    if tool.endpoint == Endpoint::Workspace {
        return match workspace_call(&request, &sockets.node_bin) {
            Ok(answer) => answer_result(answer, false),
            Err(e) => tool_result(e, true),
        };
    }
    let socket = match (tool.endpoint, &sockets.inspect, &sockets.control) {
        (Endpoint::Sandbox | Endpoint::Ai | Endpoint::Workspace | Endpoint::Commands, _, _) => {
            unreachable!("handled above")
        }
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
        Ok(answer) => {
            let is_error = answer["ok"] != true;
            answer_result(answer, is_error)
        }
        Err(e) => tool_result(e, true),
    }
}

/// The node the generic command pair talks to: the one of `vault`, or
/// at `node` (else `HELIOS_NODE`), else the control socket.
fn commands_client(
    sockets: &Sockets,
    request: &Value,
) -> Result<helios_inspect::client::Client, String> {
    let vault = request["vault"].as_str().filter(|s| !s.is_empty());
    let node = request["node"].as_str().filter(|s| !s.is_empty());
    if vault.is_some() || crate::connect::explicit(node).is_some() {
        return crate::connect::connect(
            vault.map(std::path::Path::new),
            node,
            &sockets.node_bin,
            MCP_ORIGIN,
        )
        .map(|c| c.client);
    }
    let control = sockets
        .control
        .as_ref()
        .ok_or("no node: give `vault` or `node`, or pass --control or call helios_sandbox_start")?;
    helios_inspect::client::Client::connect(control)
        .map(|c| c.with_origin(MCP_ORIGIN))
        .map_err(|e| format!("cannot reach the node at {control}: {e}"))
}

/// `helios_commands_list` or `helios_command_run` (`request`, the tool's
/// request) answered by the node behind `client`; a run is the agent's
/// (`agent:mcp`).
pub fn commands_call(client: &helios_inspect::client::Client, request: &Value) -> Value {
    let pick = |keys: &[&str]| -> Value {
        let picked: Map<String, Value> = keys
            .iter()
            .filter(|k| !request[**k].is_null())
            .map(|k| ((*k).to_string(), request[*k].clone()))
            .collect();
        Value::Object(picked)
    };
    let (method, params) = if request["cmd"] == "command.run" {
        let mut params = pick(&["id", "params", "window", "scope"]);
        params["origin"] = json!(MCP_ORIGIN);
        ("command.run", params)
    } else {
        ("commands.list", pick(&["window", "filter", "enabled_only"]))
    };
    let pending = match request["_meta"][REQUEST_ID].as_str() {
        Some(key) if method == "command.run" => client.call_keyed(method, params, key),
        _ => client.call(method, params),
    };
    match pending.wait() {
        Ok(answer) => answer_result(answer, false),
        Err(e) if e.code == helios_inspect::rpc::code::OUTCOME_UNKNOWN => tool_result(
            format!("outcome unknown: {}{}", e.message, resend_hint(request)),
            true,
        ),
        Err(e) => tool_result(e.message, true),
    }
}

/// What a template may name, seen from here: this node's modules and the
/// composites Helios ships (panels and commands are the desktop's, not
/// checked).
pub fn workspace_catalogue() -> helios_template::Catalogue {
    let mut c = helios_template::Catalogue::new().modules(crate::registry());
    for file in helios_template::shipped::composites() {
        c.add_composite(file);
    }
    c
}

/// [`workspace_catalogue`] with the user's library of composites
/// (`<config>/composites/`, the macros published), after the shipped
/// ones.
pub fn workspace_catalogue_in(config: &std::path::Path) -> helios_template::Catalogue {
    helios_template::library::with_library(
        workspace_catalogue(),
        &helios_template::library::files(config),
    )
}

/// How long a workspace request waits for the desktop holding it: its
/// own deadline (it answers when it has not run the request by then)
/// and a margin.
const DESKTOP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(25);

/// A workspace request sent to the desktop that holds the workspace
/// open; `None` when none does (none said so, or the one that did is
/// gone or let it go), and the files are worked on instead.
fn through_desktop(config: &std::path::Path, request: &Value) -> Option<Result<Value, String>> {
    let dir = PathBuf::from(request["workspace"].as_str()?);
    let files = helios_template::workspace::Files::new(config, &dir);
    let holder = files.holder()?;
    let address = Address::parse(&holder.address, holder.token.clone()).ok()?;
    let mut wire = request.clone();
    if let Some(o) = wire.as_object_mut() {
        o.remove("config");
    }
    match helios_inspect::request_within(&address, &wire, DESKTOP_DEADLINE) {
        Ok(answer) if answer["ok"] == true => Some(Ok(answer["answer"].clone())),
        // Gone, or another process now on its TCP port.
        Ok(answer)
            if answer["gone"] == true
                || answer["error"]
                    .as_str()
                    .is_some_and(|e| e.starts_with("authentication failed")) =>
        {
            None
        }
        Ok(answer) => Some(Err(answer["error"]
            .as_str()
            .map_or_else(|| answer.to_string(), String::from))),
        // Nothing listens there: the desktop went without a word (killed).
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) =>
        {
            None
        }
        Err(e) => Some(Err(format!(
            "the desktop holding this workspace ({}) did not answer: {e}",
            holder.address
        ))),
    }
}

/// A `helios_workspace_*`, `helios_context_*` or `helios_history_*`
/// tool: through the desktop holding the workspace, else on its files
/// (the histories: refused). A macro step moving an image's values
/// reaches the node of its vault, started on `node_bin` when none holds
/// it.
fn workspace_call(request: &Value, node_bin: &std::path::Path) -> Result<Value, String> {
    use helios_template::workspace::{config_dir, tools};
    let config = request["config"]
        .as_str()
        .map_or_else(config_dir, PathBuf::from);
    if let Some(answer) = through_desktop(&config, request) {
        return answer;
    }
    let cmd = request["cmd"].as_str().unwrap_or_default();
    if cmd.starts_with("clipboard.") {
        return Err(
            "no desktop holds this workspace open: the clipboard is the desktop's (a node's own clipboard slots are not built yet)"
                .into(),
        );
    }
    let mut args = request.clone();
    if let Some(o) = args.as_object_mut() {
        o.remove("cmd");
        o.remove("config");
    }
    let catalogue = workspace_catalogue_in(&config);
    let mut answer = match cmd {
        "workspace.parts" => tools::parts_of(&config, &args),
        "workspace.state" => tools::state(&config, &args),
        "workspace.edit" => tools::edit(&config, &args, &catalogue),
        "workspace.reset" => tools::reset(&config, &args, &catalogue),
        "workspace.import_parts" => tools::import_parts(&config, &args, &catalogue),
        "workspace.save_as_template" => tools::save_as_template(&config, &args, &catalogue),
        "network.delegate" => tools::delegate(&config, &args, &catalogue),
        "macro.publish" => tools::publish(&config, &args, &catalogue),
        cmd if cmd.starts_with("macro.") => {
            let reach = |vault: &str| {
                crate::connect::connect(
                    Some(std::path::Path::new(vault)),
                    None,
                    node_bin,
                    MCP_ORIGIN,
                )
                .map(|c| c.client)
            };
            macro_on_files(&config, cmd, &args, &catalogue, &reach)
        }
        cmd if cmd.starts_with("workshop.") => Err(format!(
            "{cmd} is experimental and runs on the desktop holding the workspace (with interface.experimental.workshops on, an image open): no desktop holds it"
        )),
        cmd if helios_template::network::is_command(cmd) => {
            tools::network(&config, cmd, &args, &catalogue)
        }
        "subworkspace.delete" => tools::delete_subworkspace(&config, &args, &catalogue),
        "workspace.undo" => tools::undo(&config, &args),
        "context.read" => helios_template::contexts::tools::read(&config, &args),
        "context.request" => helios_template::contexts::tools::request(&config, &args),
        "context.go" => helios_template::contexts::tools::go(&config, &args),
        "context.set_mode" => helios_template::contexts::tools::set_mode(&config, &args),
        cmd if cmd.starts_with("history.") => {
            Err("the histories are the desktop's: no desktop holds this workspace open".into())
        }
        "context.hold" | "context.release" => {
            Err("holds live in the desktop's window: no desktop holds this workspace open".into())
        }
        other => Err(format!("no workspace tool `{other}`")),
    }?;
    if let Some(o) = answer.as_object_mut() {
        o.insert("via".into(), json!("files"));
    }
    Ok(answer)
}

/// The origin of what `helios-mcp` asks of a vault's node.
const MCP_ORIGIN: &str = "agent:mcp";

/// A macro step on a workspace's files (no desktop holds it). With an
/// `image` of the vault at `vault`, its values move with the step: the
/// step is worked out from the values its edit holds that differ from the
/// network, and a version of its edit is saved through the new network
/// (labelled after the step, made by the agent). The image's edit is
/// read and saved by the vault's node (`reach` connects to it:
/// `library.edit.head`, `library.edit.save`), never by opening the vault
/// here. No undo: the files' step undoes the workspace, not the image.
fn macro_on_files(
    config: &std::path::Path,
    cmd: &str,
    args: &Value,
    catalogue: &helios_template::Catalogue,
    reach: &dyn Fn(&str) -> Result<helios_inspect::client::Client, String>,
) -> Result<Value, String> {
    use helios_template::workspace::tools;
    let image = args["image"].as_str().filter(|s| !s.is_empty());
    let vault = args["vault"].as_str().filter(|s| !s.is_empty());
    let on_node = |node: &helios_inspect::client::Client, method: &str, params: Value| {
        node.request(method, params)
            .map_err(|e| format!("the vault's node: {}", e.message))
    };
    let opened = match (image, vault) {
        (Some(image), Some(vault)) => {
            let node = reach(vault)?;
            let head = on_node(&node, "library.edit.head", json!({ "image": image }))?;
            Some((node, image.to_string(), head["head"]["edit"].clone()))
        }
        (None, None) => None,
        _ => {
            return Err(
                "`image` and `vault` go together: the image's values move with the step".into(),
            );
        }
    };
    let head_edit = opened
        .as_ref()
        .and_then(|(_, _, edit)| (!edit.is_null()).then(|| edit.clone()));
    let before = tools::files(config, args)?.load()?;
    let values = match &head_edit {
        Some(edit) => edit_values(&before, catalogue, edit)?,
        None => helios_template::macros::Values::new(),
    };
    let (mut answer, step) = tools::macro_step(config, cmd, args, catalogue, &values)?;
    if let (Some((node, image, _)), Some(step)) = (opened, step) {
        let mut graph = step.state.bundle.build(catalogue)?.graph;
        if let Some(edit) = &head_edit {
            // What does not fit the new network (a node gone) is left.
            let _ = helios_vault::edit::apply_to_graph(&mut graph, edit);
        }
        for (path, value) in step
            .values
            .iter()
            .filter_map(|(p, v)| Some((p, v.clone()?)))
        {
            if let Some((node, param)) = path
                .strip_prefix("node/")
                .and_then(|rest| rest.rsplit_once('/'))
                && let Some(id) = graph.find(node)
            {
                graph
                    .set_param(id, param, value)
                    .map_err(|e| format!("{path}: {e}"))?;
            } else if let Some(node) = path.strip_prefix("bypass/")
                && let Some(id) = graph.find(node)
            {
                graph
                    .set_bypass(id, value.as_bool() == Some(true))
                    .map_err(|e| format!("{path}: {e}"))?;
            }
        }
        let mut edit = helios_vault::edit::of_graph(&graph);
        if let Some(head) = &head_edit {
            for key in ["decisions", "marks"] {
                if !head[key].is_null() {
                    edit[key] = head[key].clone();
                }
            }
        }
        let saved = on_node(
            &node,
            "library.edit.save",
            json!({ "image": image, "edit": edit, "label": cmd }),
        )?;
        answer["image_version"] = saved["version"].clone();
    }
    Ok(answer)
}

/// The values an image's edit holds that differ from a workspace's
/// network (`node/<node>/<param>`, `bypass/<node>`).
fn edit_values(
    state: &helios_template::workspace::State,
    catalogue: &helios_template::Catalogue,
    edit: &Value,
) -> Result<helios_template::macros::Values, String> {
    let graph = state.bundle.build(catalogue)?.graph;
    let mut values = helios_template::macros::Values::new();
    let bypassed = helios_vault::edit::bypassed(edit);
    for (node, (_, params)) in helios_vault::edit::modules(edit)? {
        let Some(id) = graph.find(&node) else {
            continue;
        };
        let network = graph.params(id).ok().flatten().unwrap_or_default();
        for (param, v) in params.iter() {
            if network.get(param).is_some_and(|n| n != v) {
                values.insert(format!("node/{node}/{param}"), v.clone());
            }
        }
        let on = bypassed.contains(&node);
        if on != graph.is_bypassed(id).unwrap_or(false) {
            values.insert(format!("bypass/{node}"), helios_core::Value::Bool(on));
        }
    }
    Ok(values)
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
                    "on" => json!(true),
                    "format" => json!("png"),
                    "path" => json!("/nonexistent/out.png"),
                    "items" if tool.cmd == "workflow.prioritize" => json!(["/nonexistent/a.png"]),
                    "items" => json!(["/nonexistent/a.png", { "path": "/nonexistent/b.png" }]),
                    "to" => json!("top"),
                    "corners" => json!([[10.0, 10.0], [90.0, 10.0], [90.0, 60.0], [10.0, 60.0]]),
                    _ => json!("exp"),
                };
                (p.name.to_string(), v)
            })
            .collect();
        Value::Object(args)
    }

    #[test]
    fn every_tool_maps_onto_a_request_type() {
        for tool in TOOLS.iter().filter(|t| {
            !matches!(
                t.endpoint,
                Endpoint::Sandbox | Endpoint::Ai | Endpoint::Workspace | Endpoint::Commands
            )
        }) {
            let request = tool.request(&example(tool)).unwrap();
            let parsed = match tool.endpoint {
                Endpoint::Inspect => serde_json::from_value::<Request>(request.clone()).map(|_| ()),
                Endpoint::Control => {
                    serde_json::from_value::<ControlRequest>(request.clone()).map(|_| ())
                }
                Endpoint::Sandbox | Endpoint::Ai | Endpoint::Workspace | Endpoint::Commands => {
                    unreachable!("filtered out")
                }
            };
            assert!(
                parsed.is_ok(),
                "{}: {request} does not parse: {parsed:?}",
                tool.name
            );
        }
    }

    #[test]
    fn a_limit_over_the_cap_is_refused_before_the_node() {
        let mut sockets = Sockets {
            inspect: None,
            control: None,
            node_bin: PathBuf::from("helios-node"),
            ai: None,
            roots: Roots::unconfined(),
        };
        let (is_error, answer) = tool(
            &mut sockets,
            "helios_workflow_items",
            json!({ "run": "r1", "limit": 5000 }),
        );
        assert!(is_error);
        assert_eq!(
            answer,
            json!(
                "helios_workflow_items: `limit` 5000 is over the cap of 1000 items: page with `offset`"
            )
        );
    }

    #[test]
    fn every_request_has_a_tool() {
        let inspect = ["state", "describe", "events", "wait_event", "wait_idle"];
        let control = [
            "render",
            "set_param",
            "node.bypass",
            "load_source",
            "export",
            "cancel",
            "job.cancel",
            "pause",
            "resume",
            "fault",
            "decisions",
            "decision_resolve",
            "decision_accept",
            "decision_decide",
            "decision_reask",
            "workflow.submit",
            "workflow.runs",
            "workflow.items",
            "workflow.forget",
            "workflow.cancel",
            "workflow.retry",
            "workflow.prioritize",
            "workflow.pause",
            "workflow.resume",
            "cache.stats",
            "cache.clear",
            "calibrate.chart",
        ];
        for cmd in inspect.iter().chain(&control) {
            assert!(TOOLS.iter().any(|t| t.cmd == *cmd), "no tool for `{cmd}`");
        }
        let node_tools = TOOLS
            .iter()
            .filter(|t| {
                !matches!(
                    t.endpoint,
                    Endpoint::Sandbox | Endpoint::Ai | Endpoint::Workspace | Endpoint::Commands
                )
            })
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
            roots: Roots::unconfined(),
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
            roots: Roots::unconfined(),
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
    fn an_agent_moves_a_workspace_s_places_and_modes_through_mcp() {
        let root = std::env::temp_dir().join(format!("helios-mcp-context-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (config, ws) = (root.join("config"), root.join("shoot"));
        let mut s = Sockets {
            inspect: None,
            control: None,
            node_bin: PathBuf::from("helios-node"),
            ai: None,
            roots: Roots::unconfined(),
        };
        let args = |extra: Value| {
            let mut a = json!({ "workspace": ws, "config": config });
            for (k, v) in extra.as_object().unwrap() {
                a[k] = v.clone();
            }
            a
        };
        let (failed, read) = tool(&mut s, "helios_context_read", args(json!({})));
        assert!(!failed, "{read}");
        assert_eq!(read["place"], "root");
        // The library, then the darkroom on an image: a dry run first.
        let places = json!([{ "frame": "helios.library" },
            { "frame": "helios.darkroom", "payload": { "image": "ab12" } }]);
        let (failed, dry) = tool(
            &mut s,
            "helios_context_go",
            args(json!({ "places": places, "dry_run": true })),
        );
        assert!(!failed && dry["dry_run"] == true, "{dry}");
        let (_, read) = tool(&mut s, "helios_context_read", args(json!({})));
        assert_eq!(read["rev"], 0, "a dry run writes nothing");
        let (failed, went) = tool(
            &mut s,
            "helios_context_go",
            args(json!({ "places": places })),
        );
        assert!(!failed, "{went}");
        // Culling set, compare pushed, then back pops compare.
        let (failed, _) = tool(
            &mut s,
            "helios_context_set_mode",
            args(json!({ "mode": "helios.culling", "if_rev": 1 })),
        );
        assert!(!failed);
        let (failed, _) = tool(
            &mut s,
            "helios_context_set_mode",
            args(json!({ "mode": "helios.compare", "push": true })),
        );
        assert!(!failed);
        let (failed, back) = tool(
            &mut s,
            "helios_context_request",
            args(json!({ "request": { "op": "back" } })),
        );
        assert!(!failed, "{back}");
        assert_eq!(back["left"], json!(["helios.compare"]));
        // A stale revision is refused, and says so.
        let (failed, stale) = tool(
            &mut s,
            "helios_context_request",
            args(json!({ "request": { "op": "leave" }, "if_rev": 1 })),
        );
        assert!(failed, "{stale}");
        let (_, read) = tool(&mut s, "helios_context_read", args(json!({})));
        assert_eq!(read["place"], "helios.darkroom");
        assert_eq!(read["state"]["modes"][0]["frame"], "helios.culling");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_history_and_hold_tools_need_the_desktop_holding_the_workspace() {
        let root = std::env::temp_dir().join(format!("helios-mcp-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut s = Sockets {
            inspect: None,
            control: None,
            node_bin: PathBuf::from("helios-node"),
            ai: None,
            roots: Roots::unconfined(),
        };
        let args = json!({ "workspace": root.join("shoot"), "config": root.join("config") });
        for name in [
            "helios_history_list",
            "helios_history_read",
            "helios_history_undo",
            "helios_history_redo",
            "helios_history_snapshot",
            "helios_history_restore",
            "helios_context_hold",
            "helios_context_release",
        ] {
            let mut args = args.clone();
            if name == "helios_context_hold" {
                args["reason"] = json!("an agent is retouching");
            }
            if name == "helios_context_release" {
                args["token"] = json!(1);
            }
            if name == "helios_history_restore" {
                args["snapshot"] = json!(1);
            }
            let (failed, answer) = tool(&mut s, name, args);
            assert!(failed, "{name}: {answer}");
            assert!(answer.to_string().contains("no desktop holds"), "{answer}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_agent_edits_a_workspace_through_mcp_with_dry_runs() {
        let root =
            std::env::temp_dir().join(format!("helios-mcp-workspace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = root.join("config");
        let portrait = helios_template::shipped::templates()
            .into_iter()
            .find(|b| b.template.id == "helios.portrait-retouch")
            .unwrap();
        let (dir, _) = helios_template::workspace::create(
            &portrait,
            &std::collections::BTreeMap::new(),
            &workspace_catalogue(),
            &root.join("work"),
            &config,
        )
        .unwrap();
        let mut s = Sockets {
            inspect: None,
            control: None,
            node_bin: PathBuf::from("helios-node"),
            ai: None,
            roots: Roots::unconfined(),
        };
        let args = |extra: Value| {
            let mut a = json!({ "workspace": dir, "config": config });
            for (k, v) in extra.as_object().unwrap() {
                a[k] = v.clone();
            }
            a
        };
        let nodes = |s: &mut Sockets| {
            let (failed, state) = tool(s, "helios_workspace_state", args(json!({})));
            assert!(!failed, "{state}");
            state["template"]["network"]["nodes"]
                .as_array()
                .unwrap()
                .len()
        };
        let (failed, parts) = tool(&mut s, "helios_workspace_parts", args(json!({})));
        assert!(!failed, "{parts}");
        assert_eq!(parts["parts"][0]["part"], "network");
        // An import's dry run: the conflict to pick, nothing written.
        let intake = json!({ "source": "helios.import-cull-dedupe" });
        let mut dry = intake.clone();
        dry["dry_run"] = json!(true);
        let (failed, diff) = tool(&mut s, "helios_workspace_import_parts", args(dry));
        assert!(!failed, "{diff}");
        assert_eq!(diff["unresolved"], json!(["view"]));
        assert_eq!(nodes(&mut s), 3);
        let (failed, _) = tool(
            &mut s,
            "helios_workspace_import_parts",
            args(intake.clone()),
        );
        assert!(failed, "a conflict without a side is refused");
        let mut picked = intake;
        picked["picks"] = json!({ "view": "ours" });
        let (failed, done) = tool(&mut s, "helios_workspace_import_parts", args(picked));
        assert!(!failed, "{done}");
        assert_eq!(nodes(&mut s), 4);
        // Saved as a template: reviewed first, then written.
        let template =
            json!({ "id": "user.agent", "name": "Agent's", "description": "Made by an agent" });
        let mut dry = template.clone();
        dry["dry_run"] = json!(true);
        let (failed, review) = tool(&mut s, "helios_workspace_save_as_template", args(dry));
        assert!(!failed && review["ok"] == true, "{review}");
        let bundle = helios_template::workspace::template_dir(&config, "user.agent");
        assert!(!bundle.exists());
        let (failed, written) = tool(&mut s, "helios_workspace_save_as_template", args(template));
        assert!(!failed, "{written}");
        assert!(bundle.is_dir());
        // Reset, its dry run first, then undone.
        let (failed, dry) = tool(
            &mut s,
            "helios_workspace_reset",
            args(json!({ "dry_run": true })),
        );
        assert!(!failed && dry["changed"] == true, "{dry}");
        assert_eq!(nodes(&mut s), 4);
        let (failed, _) = tool(&mut s, "helios_workspace_reset", args(json!({})));
        assert!(!failed);
        assert_eq!(nodes(&mut s), 3);
        let (failed, undone) = tool(&mut s, "helios_workspace_undo", args(json!({})));
        assert!(!failed, "{undone}");
        assert_eq!(nodes(&mut s), 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_clipboard_tools_need_a_desktop_holding_the_workspace() {
        let root =
            std::env::temp_dir().join(format!("helios-mcp-clipboard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut s = Sockets {
            inspect: None,
            control: None,
            node_bin: PathBuf::from("helios-node"),
            ai: None,
            roots: Roots::unconfined(),
        };
        let base = json!({ "workspace": root.join("work"), "config": root.join("config") });
        for (name, extra) in [
            ("helios_clipboard", json!({})),
            (
                "helios_copy",
                json!({ "format": "text/plain", "data": "hello" }),
            ),
            (
                "helios_paste",
                json!({ "target": "helios.library", "dry_run": true }),
            ),
        ] {
            let mut args = base.clone();
            for (k, v) in extra.as_object().unwrap() {
                args[k] = v.clone();
            }
            let (failed, answer) = tool(&mut s, name, args);
            assert!(failed, "{name}: {answer}");
            assert!(
                answer.to_string().contains("no desktop"),
                "{name}: {answer}"
            );
        }
        // Unknown arguments are refused before anything is sent.
        let mut args = base;
        args["payload_format"] = json!("png");
        let (failed, _) = tool(&mut s, "helios_clipboard", args);
        assert!(failed);
    }

    #[test]
    fn an_agent_edits_a_workspace_s_network_node_by_node_through_mcp() {
        let root = std::env::temp_dir().join(format!("helios-mcp-network-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = root.join("config");
        let portrait = helios_template::shipped::templates()
            .into_iter()
            .find(|b| b.template.id == "helios.portrait-retouch")
            .unwrap();
        let (dir, _) = helios_template::workspace::create(
            &portrait,
            &std::collections::BTreeMap::new(),
            &workspace_catalogue(),
            &root.join("work"),
            &config,
        )
        .unwrap();
        let mut s = Sockets {
            inspect: None,
            control: None,
            node_bin: PathBuf::from("helios-node"),
            ai: None,
            roots: Roots::unconfined(),
        };
        let args = |extra: Value| {
            let mut a = json!({ "workspace": dir, "config": config });
            for (k, v) in extra.as_object().unwrap() {
                a[k] = v.clone();
            }
            a
        };
        let network = |s: &mut Sockets| {
            let (failed, state) = tool(s, "helios_workspace_state", args(json!({})));
            assert!(!failed, "{state}");
            state["template"]["network"].clone()
        };
        let has = |net: &Value, name: &str| {
            net["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["name"] == name)
        };
        // An added module gets a free name; a dry run writes nothing.
        let add = json!({ "nodes": [{ "module": "exposure", "at": [40.0, 300.0] }] });
        let mut dry = add.clone();
        dry["dry_run"] = json!(true);
        let (failed, said) = tool(&mut s, "helios_network_add", args(dry));
        assert!(!failed, "{said}");
        assert_eq!(
            (&said["added"], &said["dry_run"], &said["via"]),
            (&json!(["exposure_2"]), &json!(true), &json!("files"))
        );
        assert!(!has(&network(&mut s), "exposure_2"));
        let (failed, added) = tool(&mut s, "helios_network_add", args(add));
        assert!(!failed, "{added}");
        assert!(has(&network(&mut s), "exposure_2"));
        // Wired between the source and the first exposure.
        let (failed, wired) = tool(
            &mut s,
            "helios_network_connect",
            args(
                json!({ "from": "source", "port": "image", "to": "exposure_2", "input": "input" }),
            ),
        );
        assert!(!failed, "{wired}");
        let (failed, wired) = tool(
            &mut s,
            "helios_network_connect",
            args(
                json!({ "from": "exposure_2", "port": "output", "to": "exposure", "input": "input" }),
            ),
        );
        assert!(!failed, "{wired}");
        let net = network(&mut s);
        let into_exposure: Vec<&Value> = net["connections"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["to"] == "exposure")
            .collect();
        assert_eq!(into_exposure.len(), 1, "the input's old wire went");
        assert_eq!(into_exposure[0]["from"], "exposure_2");
        // A loop is refused and changes nothing.
        let (failed, refused) = tool(
            &mut s,
            "helios_network_connect",
            args(
                json!({ "from": "exposure", "port": "output", "to": "exposure_2", "input": "input" }),
            ),
        );
        assert!(failed, "{refused}");
        assert_eq!(network(&mut s), net);
        // Moved, then removed with its wires; the node shown stays.
        let (failed, moved) = tool(
            &mut s,
            "helios_network_move",
            args(json!({ "moves": [{ "node": "exposure_2", "to": [0.0, 0.0] }] })),
        );
        assert!(!failed, "{moved}");
        let (failed, _) = tool(
            &mut s,
            "helios_network_remove",
            args(json!({ "nodes": ["retouch"] })),
        );
        assert!(failed, "the node the viewport shows stays");
        // Taken off the render path first (a node the view reads from
        // keeps every input wired), then removed.
        let (failed, rewired) = tool(
            &mut s,
            "helios_network_connect",
            args(json!({ "from": "source", "port": "image", "to": "exposure", "input": "input" })),
        );
        assert!(!failed, "{rewired}");
        let (failed, removed) = tool(
            &mut s,
            "helios_network_remove",
            args(json!({ "nodes": ["exposure_2"] })),
        );
        assert!(!failed, "{removed}");
        let net = network(&mut s);
        assert!(!has(&net, "exposure_2"));
        assert!(
            !net["connections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["from"] == "exposure_2" || c["to"] == "exposure_2")
        );
        // Copied nodes pasted: renamed on a clash, wired in again.
        let clip = json!({
            "nodes": [{ "name": "exposure", "module": "exposure" }],
            "inputs": [{ "from": "source", "port": "image", "to": "exposure", "input": "input" }],
        });
        let (failed, pasted) = tool(
            &mut s,
            "helios_network_paste",
            args(json!({ "clip": clip })),
        );
        assert!(!failed, "{pasted}");
        assert_eq!(pasted["added"], json!(["exposure_2"]));
        assert_eq!(pasted["renamed"]["exposure"], "exposure_2");
        assert!(
            network(&mut s)["connections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["from"] == "source" && c["to"] == "exposure_2")
        );
        // Each edit is one step an agent undoes.
        let (failed, undone) = tool(&mut s, "helios_workspace_undo", args(json!({})));
        assert!(!failed, "{undone}");
        assert_eq!(undone["undone"], "paste nodes");
        assert!(!has(&network(&mut s), "exposure_2"));
        let _ = std::fs::remove_dir_all(&root);
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
    #[test]
    fn a_macro_made_on_the_files_moves_an_image_s_values_into_a_version() {
        use helios_vault::Store as _;
        let root = std::env::temp_dir().join(format!("helios-mcp-macro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (config, ws) = (root.join("config"), root.join("photos"));
        std::fs::create_dir_all(&ws).unwrap();
        // An image whose edit sets the blank network's exposure, in the
        // vault of a node reached in this process.
        let id = helios_vault::ImageId(helios_core::ContentHash([7; 32]));
        let mut store = helios_vault::MemoryStore::default();
        store
            .add_to_library(&helios_vault::Image {
                id,
                size: 10,
                partial: helios_core::ContentHash([7; 32]),
            })
            .unwrap();
        let vault = Arc::new(helios_vault::Vault::new(store));
        let (mut session, _) = helios_vault::EditSession::open(&vault, id).unwrap();
        let edit = json!({ "modules": [{ "name": "exposure", "module": "exposure",
            "params": { "exposure": { "float": 1.0 } } }] });
        session
            .save(&vault, edit, "set exposure", "user", 1)
            .unwrap();
        let bus = Arc::new(helios_core::Bus::new());
        let heard = bus.subscribe();
        let service = Arc::new(helios_node_core::endpoint::VaultService::new(
            vault.clone(),
            bus.clone(),
            helios_core::jobs::Jobs::new(bus),
            "test",
        ));
        let asked = std::sync::Mutex::new(Vec::new());
        let reach = |file: &str| {
            asked.lock().unwrap().push(file.to_string());
            Ok(helios_inspect::client::Client::in_process(service.clone()).with_origin(MCP_ORIGIN))
        };
        let args = json!({
            "workspace": ws,
            "nodes": ["exposure"],
            "name": "Look",
            "image": id.hex(),
            "vault": "/photos/vault.db",
        });
        let answer =
            macro_on_files(&config, "macro.make", &args, &workspace_catalogue(), &reach).unwrap();
        assert_eq!(answer["node"], "look", "{answer}");
        assert!(answer["image_version"].is_string(), "{answer}");
        assert_eq!(*asked.lock().unwrap(), ["/photos/vault.db"]);
        // The node ran the read and the save, for the agent.
        let ran: Vec<(String, String)> = heard
            .try_iter()
            .filter(|e| e.kind == "control.command")
            .map(|e| {
                (
                    e.payload["id"].as_str().unwrap().to_string(),
                    e.payload["origin"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            ran,
            [
                ("library.edit.head".to_string(), "agent:mcp".to_string()),
                ("library.edit.save".to_string(), "agent:mcp".to_string()),
            ]
        );
        // The workspace's files hold the macro.
        let files = helios_template::workspace::tools::files(&config, &args).unwrap();
        let state = files.load().unwrap();
        assert!(
            state
                .bundle
                .template
                .network
                .nodes
                .iter()
                .any(|n| n.name == "look")
        );
        // The image's head: its value on the macro, the exposure gone,
        // saved by the agent.
        let head = vault.heads(&id).unwrap().remove(0);
        assert_eq!(head.origin, "agent:mcp");
        let modules = helios_vault::edit::modules(&head.edit).unwrap();
        assert!(!modules.contains_key("exposure"), "{}", head.edit);
        assert_eq!(
            modules["look"].1.get("exposure"),
            Some(&helios_core::Value::Float(1.0)),
            "{}",
            head.edit
        );
        // Without its vault, an image is refused.
        let e = macro_on_files(
            &config,
            "macro.open",
            &json!({ "workspace": ws, "node": "look", "image": id.hex() }),
            &workspace_catalogue(),
            &reach,
        )
        .unwrap_err();
        assert!(e.contains("go together"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_macro_is_published_only_with_the_user_s_confirmation() {
        let root = std::env::temp_dir().join(format!("helios-mcp-publish-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (config, ws) = (root.join("config"), root.join("photos"));
        std::fs::create_dir_all(&ws).unwrap();
        let call = |cmd: &str, args: Value| {
            let mut request = args;
            request["cmd"] = json!(cmd);
            request["config"] = json!(config);
            request["workspace"] = json!(ws);
            workspace_call(&request, std::path::Path::new("helios-node"))
        };
        call(
            "macro.make",
            json!({ "nodes": ["exposure"], "name": "Look" }),
        )
        .unwrap();
        let file = helios_template::library::dir(&config).join("user.look@1.helios-composite.json");
        // A dry run says where it would write, and writes nothing.
        let dry = call("macro.publish", json!({ "node": "look", "dry_run": true })).unwrap();
        assert_eq!(dry["composite"]["id"], "user.look", "{dry}");
        assert_eq!(dry["file"], json!(file.display().to_string()));
        assert_eq!(dry["undoable"], false);
        assert!(!file.exists());
        // On the files, no one is there to confirm it: refused.
        let e = call("macro.publish", json!({ "node": "look" })).unwrap_err();
        assert!(e.contains("confirmation"), "{e}");
        assert!(!file.exists());
        // Once in the library (the desktop wrote it), the workspace's
        // catalogue offers it, and publishing it again is refused.
        let state = helios_template::workspace::tools::files(&config, &json!({ "workspace": ws }))
            .unwrap()
            .load()
            .unwrap();
        let files = state
            .macro_publishable(&workspace_catalogue(), Some("look"), None)
            .unwrap();
        helios_template::library::publish(&config, &files, false).unwrap();
        assert!(file.exists());
        assert!(
            workspace_catalogue_in(&config)
                .get_composite(&files[0].reference())
                .is_some()
        );
        let e = call(
            "macro.publish",
            json!({ "id": "user.look", "dry_run": true }),
        )
        .unwrap_err();
        assert!(e.contains("in your library already"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_workshop_verbs_need_the_desktop() {
        let root =
            std::env::temp_dir().join(format!("helios-mcp-workshops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (config, ws) = (root.join("config"), root.join("photos"));
        std::fs::create_dir_all(&ws).unwrap();
        for (verb, cmd) in [
            ("helios_workshop_apply", "workshop.apply"),
            ("helios_workshop_aim", "workshop.aim"),
            ("helios_workshop_suggest", "workshop.suggest"),
            ("helios_workshop_save", "workshop.save"),
            ("helios_workshop_context", "workshop.context"),
            ("helios_workshop_design", "workshop.design"),
            ("helios_workshop_session", "workshop.session"),
            ("helios_workshop_propose", "workshop.propose"),
            ("helios_workshop_generalise", "workshop.generalise"),
            ("helios_workshop_review", "workshop.review"),
        ] {
            let tool = TOOLS.iter().find(|t| t.name == verb).unwrap();
            assert_eq!(tool.cmd, cmd);
            assert!(tool.description.starts_with("Experimental"), "{verb}");
            let e = workspace_call(
                &json!({
                "cmd": cmd,
                "config": config,
                "workspace": ws,
                "intent": "brighter",
                "workshop": "helios.lift-midtones@1",
                "draft": {},
                }),
                std::path::Path::new("helios-node"),
            )
            .unwrap_err();
            assert!(e.contains("interface.experimental.workshops"), "{e}");
            assert!(e.contains("no desktop holds it"), "{e}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A desktop's windows, as its node sees them: `theme.select` applies
    /// in `main`, `edit.undo` is greyed with the reason its menu gives.
    struct Windows(std::sync::Mutex<Vec<(String, String)>>);

    impl helios_node_core::endpoint::WindowCommands for Windows {
        fn list(
            &self,
            window: &str,
        ) -> Result<Vec<helios_node_core::endpoint::WindowCommand>, String> {
            let command =
                |id: &str, reason: Option<&str>| helios_node_core::endpoint::WindowCommand {
                    id: id.into(),
                    label: id.into(),
                    category: "Edit".into(),
                    description: String::new(),
                    params: json!({ "type": "object" }),
                    keys: Vec::new(),
                    icon: None,
                    read_only: false,
                    enabled: reason.is_none(),
                    disabled_reason: reason.map(String::from),
                };
            assert_eq!(window, "main");
            Ok(vec![
                command("edit.undo", Some("Does not apply here (canUndo)")),
                command("theme.select", None),
            ])
        }

        fn run(&self, id: &str, _: Value, origin: &str, _: &str) -> Result<Value, String> {
            if id == "edit.undo" {
                return Err("Does not apply here (canUndo)".into());
            }
            self.0.lock().unwrap().push((id.into(), origin.into()));
            Ok(json!({ "ran": id }))
        }
    }

    #[test]
    fn the_generic_pair_lists_and_runs_node_and_window_commands_as_the_agent() {
        use helios_core::Bus;
        use helios_core::jobs::Jobs;
        use helios_node_core::endpoint::VaultService;
        use helios_vault::{MemoryStore, Vault};

        let bus = Arc::new(Bus::new());
        let heard = bus.subscribe();
        let windows = Arc::new(Windows(std::sync::Mutex::default()));
        let service = VaultService::new(
            Arc::new(Vault::new(MemoryStore::default())),
            bus.clone(),
            Jobs::new(bus.clone()),
            "mcp-test",
        )
        .with_windows(windows.clone());
        let client = helios_inspect::client::Client::in_process(Arc::new(service));
        let tool = |name: &str, arguments: Value| {
            let tool = TOOLS.iter().find(|t| t.name == name).unwrap();
            let result = commands_call(&client, &tool.request(&arguments).unwrap());
            let text = result["content"][0]["text"].as_str().unwrap().to_string();
            (
                result["isError"] == true,
                serde_json::from_str(&text).unwrap_or(json!(text)),
            )
        };

        // The node's commands and the window's, in one list, each saying
        // whether it runs and why not.
        let (failed, listed) = tool("helios_commands_list", json!({}));
        assert!(!failed, "{listed}");
        let commands = listed["commands"].as_array().unwrap();
        let find = |id: &str| commands.iter().find(|c| c["id"] == id).unwrap().clone();
        assert_eq!(find("library.stacks.list")["scope"], "node");
        assert_eq!(find("library.stacks.list")["enabled"], true);
        let undo = find("edit.undo");
        assert_eq!(undo["scope"], "window");
        assert_eq!(undo["enabled"], false);
        assert_eq!(undo["disabledReason"], "Does not apply here (canUndo)");
        // `enabled_only` leaves the greyed window command out.
        let (_, matched) = tool("helios_commands_list", json!({ "filter": "edit.undo" }));
        let ids: Vec<&Value> = matched["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| &c["id"])
            .collect();
        assert_eq!(ids, [&json!("edit.undo")], "{matched}");
        let (_, only) = tool(
            "helios_commands_list",
            json!({ "filter": "edit.undo", "enabled_only": true }),
        );
        assert_eq!(only["commands"], json!([]), "{only}");

        // A window command runs as the agent, and the bus hears who.
        let (failed, ran) = tool(
            "helios_command_run",
            json!({ "id": "theme.select", "params": { "name": "helios-light" } }),
        );
        assert!(!failed, "{ran}");
        assert_eq!(ran["origin"], "agent:mcp");
        assert_eq!(
            *windows.0.lock().unwrap(),
            [("theme.select".to_string(), "agent:mcp".to_string())]
        );
        // So does a node command.
        let (failed, stacks) = tool("helios_command_run", json!({ "id": "library.stacks.list" }));
        assert!(!failed, "{stacks}");
        let ran: Vec<(String, String)> = heard
            .try_iter()
            .filter(|e| e.kind == "control.command")
            .map(|e| {
                (
                    e.payload["id"].as_str().unwrap().to_string(),
                    e.payload["origin"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            ran,
            [
                ("theme.select".to_string(), "agent:mcp".to_string()),
                ("library.stacks.list".to_string(), "agent:mcp".to_string()),
            ]
        );

        // A greyed one is refused with its reason, bad params with the
        // schema's.
        let (failed, refused) = tool("helios_command_run", json!({ "id": "edit.undo" }));
        assert!(failed);
        assert_eq!(refused, "Does not apply here (canUndo)");
        let (failed, refused) = tool(
            "helios_command_run",
            json!({ "id": "library.stacks.dissolve", "params": {} }),
        );
        assert!(failed);
        assert!(
            refused
                .as_str()
                .unwrap()
                .ends_with("params: `stack` is missing"),
            "{refused}"
        );
    }

    /// A node that reads one request, then closes without answering.
    #[cfg(unix)]
    fn drops_after_reading(name: &str) -> (PathBuf, std::thread::JoinHandle<()>) {
        use std::io::BufRead;
        let dir = std::env::temp_dir().join(format!("helios-mcp-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("node.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let node = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut line = String::new();
            let _ = std::io::BufReader::new(stream).read_line(&mut line);
        });
        (path, node)
    }

    #[cfg(unix)]
    #[test]
    fn a_change_sent_and_never_answered_is_outcome_unknown_not_unreachable() {
        let (path, node) = drops_after_reading("send");
        let request = json!({ "cmd": "set_param", "node": "exp", "param": "ev", "value": 1.0 });
        let error = send(&Address::Unix(path.clone()), &request).unwrap_err();
        node.join().unwrap();
        assert!(error.starts_with("outcome unknown"), "{error}");
        assert!(error.contains("never resend it blindly"), "{error}");
        let gone = send(&Address::Unix(path.clone()), &request).unwrap_err();
        assert!(gone.starts_with("cannot reach the node"), "{gone}");

        let (path, node) = drops_after_reading("run");
        let client = helios_inspect::client::Client::connect(&Address::Unix(path)).unwrap();
        let run = json!({ "cmd": "command.run", "id": "edit.undo" });
        let result = commands_call(&client, &run);
        node.join().unwrap();
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("outcome unknown"), "{text}");
    }

    #[test]
    fn changes_carry_a_key_and_reads_do_not() {
        let tool = |name: &str| TOOLS.iter().find(|t| t.name == name).unwrap();
        let set = tool("helios_set_param");
        assert!(set.changes());
        assert!(set.input_schema()["properties"][REQUEST_ID].is_object());
        let arguments = json!({ "node": "exp", "param": "ev", "value": 1.0 });
        let first = set.request(&arguments).unwrap();
        let key = first["_meta"][REQUEST_ID].as_str().unwrap();
        assert_ne!(
            set.request(&arguments).unwrap()["_meta"],
            first["_meta"],
            "a new key each"
        );
        let mut given = arguments.clone();
        given[REQUEST_ID] = json!("mine");
        assert_eq!(set.request(&given).unwrap()["_meta"][REQUEST_ID], "mine");
        assert!(resend_hint(&first).contains(key), "{}", resend_hint(&first));
        assert!(tool("helios_command_run").changes());
        for read in [
            "helios_commands_list",
            "helios_workflow_runs",
            "helios_state",
        ] {
            let read = tool(read);
            assert!(!read.changes(), "{}", read.name);
            assert!(read.input_schema()["properties"].get(REQUEST_ID).is_none());
        }
    }

    /// Counts the commands it runs.
    #[derive(Default)]
    struct Runs(std::sync::atomic::AtomicU32);

    impl helios_inspect::rpc::Service for Runs {
        fn call(
            &self,
            _: &str,
            _: Value,
            _: &helios_inspect::rpc::Call,
        ) -> Result<Value, helios_inspect::rpc::Error> {
            let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            Ok(json!({ "runs": n }))
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_command_run_sent_again_with_its_request_id_is_applied_once() {
        let dir = std::env::temp_dir().join(format!("helios-mcp-{}-keys", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("node.sock");
        let runs = Arc::new(Runs::default());
        helios_inspect::serve_rpc_at(
            &helios_inspect::Listen::Unix(path.clone()),
            None,
            |_: Value, _: &mut helios_inspect::Connection<'_>| None,
            runs.clone(),
        )
        .unwrap();
        let tool = TOOLS
            .iter()
            .find(|t| t.name == "helios_command_run")
            .unwrap();
        let arguments = json!({ "id": "theme.select", "request_id": "agent-1" });
        let run = || {
            // A new connection each time, as after an unknown outcome.
            let client =
                helios_inspect::client::Client::connect(&Address::Unix(path.clone())).unwrap();
            let result = commands_call(&client, &tool.request(&arguments).unwrap());
            assert_eq!(result["isError"], false, "{result}");
            let text = result["content"][0]["text"].as_str().unwrap().to_string();
            serde_json::from_str::<Value>(&text).unwrap()
        };
        assert_eq!(run(), json!({ "runs": 1 }));
        assert_eq!(run(), json!({ "runs": 1, "already_applied": true }));
        assert_eq!(runs.0.load(std::sync::atomic::Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
