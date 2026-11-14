//! Generated tools (`helios-mcp --tools generated`, [`ToolSet::Generated`]):
//! one MCP tool per command of the node's registry, made from it rather
//! than written by hand. A tool's name is `helios_cmd_` and the command's
//! id with every character a tool name does not take turned into `_`
//! (`library.stacks.list` is `helios_cmd_library_stacks_list`); its
//! description comes from the command's label and category; its input
//! schema is the command's params schema, plus the server's own arguments,
//! which start with `_`: `_vault`, `_node` and `_window` pick the node as
//! `helios_command_run`'s `vault`, `node` and `window` do, and
//! `_request_id` keys a change as its `request_id` does. Its annotations say
//! what the registry knows: its label as `title`, and `readOnlyHint`.
//!
//! A call checks the params against the command's schema first, then runs
//! as `helios_command_run` with that id and `scope: "node"` would: the same
//! caps, roots, request key and "outcome unknown". The commands come from
//! the registries compiled in (the node's verbs and a vault's commands),
//! sorted by id, so the list is the same on every start; a desktop's window
//! commands depend on its open windows and stay reached through
//! `helios_command_run`.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use helios_node_core::commands::{CommandSpec, Need, validate};
use serde_json::{Map, Value, json};

use super::{REQUEST_ID, TOOLS};

/// What every generated tool's name starts with.
pub const PREFIX: &str = "helios_cmd_";

/// The longest tool name MCP clients take.
pub const LONGEST_NAME: usize = 64;

/// The most generated tools offered; past it the last ids (by id) are left
/// out, still reached through `helios_command_run`.
pub const MOST_TOOLS: usize = 120;

/// The longest description; a longer one is cut, with a pointer to
/// `helios_commands_list`.
pub const LONGEST_DESCRIPTION: usize = 1024;

/// The server's own arguments of a generated tool, beside the command's
/// params.
const VAULT: &str = "_vault";
const NODE: &str = "_node";
const WINDOW: &str = "_window";
const KEY: &str = "_request_id";

/// A generated tool: its name and its command.
pub struct Generated {
    pub name: String,
    pub spec: CommandSpec,
}

/// The tool name of command `id`: [`PREFIX`] and the id, every character
/// but ASCII letters, digits, `_` and `-` turned into `_`.
#[must_use]
pub fn tool_name(id: &str) -> String {
    let id: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{PREFIX}{id}")
}

/// Every command a generated tool runs: the node's registry, then a
/// vault's commands it does not hold, by id.
fn commands() -> BTreeMap<String, CommandSpec> {
    let mut all = BTreeMap::new();
    for spec in crate::commands::registry()
        .specs()
        .cloned()
        .chain(helios_node_core::endpoint::specs())
    {
        all.entry(spec.id.clone()).or_insert(spec);
    }
    all
}

/// The generated tools, sorted by name, at most [`MOST_TOOLS`].
pub fn tools() -> &'static [Generated] {
    static TOOLS_MADE: OnceLock<Vec<Generated>> = OnceLock::new();
    TOOLS_MADE.get_or_init(|| {
        let mut made: Vec<Generated> = commands()
            .into_values()
            .map(|spec| Generated {
                name: tool_name(&spec.id),
                spec,
            })
            .collect();
        made.sort_by(|a, b| a.name.cmp(&b.name));
        made.truncate(MOST_TOOLS);
        made
    })
}

/// The generated tool named `name`.
pub fn find(name: &str) -> Option<&'static Generated> {
    tools().iter().find(|t| t.name == name)
}

impl Generated {
    /// Its description: the command's label and category, what it needs,
    /// and how it runs; cut at [`LONGEST_DESCRIPTION`].
    #[must_use]
    pub fn description(&self) -> String {
        let s = &self.spec;
        let mut text = format!("{} ({}). Runs command `{}`", s.label, s.category, s.id);
        text.push_str(" as helios_command_run would, as the agent (origin agent:mcp): its params are checked against its schema first.");
        if s.needs.contains(&Need::Vault) {
            text.push_str(" Needs a vault's node: give `_vault` (or `_node`).");
        }
        if s.needs.contains(&Need::Window) {
            text.push_str(" Needs a desktop window holding the workspace.");
        }
        text.push_str(if s.read_only {
            " Only reads."
        } else {
            " A change: give `_request_id` again after an outcome unknown, never resend it blindly."
        });
        cut(text)
    }

    /// Its input schema: the command's params schema and the server's own
    /// arguments.
    #[must_use]
    pub fn input_schema(&self) -> Value {
        let mut schema = match &self.spec.params {
            Value::Object(o) => Value::Object(o.clone()),
            _ => json!({}),
        };
        schema["type"] = json!("object");
        if !schema["properties"].is_object() {
            schema["properties"] = json!({});
        }
        let text = |description: &str| json!({ "type": "string", "description": description });
        let properties = schema["properties"]
            .as_object_mut()
            .expect("an object just set");
        properties.insert(
            VAULT.into(),
            text("The vault whose node runs the command; started on it when no node holds it"),
        );
        properties.insert(NODE.into(), text("The node's command endpoint (a socket path or tcp://HOST:PORT), instead of `_vault`; else HELIOS_NODE, else the control socket"));
        properties.insert(
            WINDOW.into(),
            text("The desktop window it runs in; main when left out"),
        );
        if !self.spec.read_only {
            properties.insert(
                KEY.into(),
                json!({
                    "type": "string",
                    "minLength": 1,
                    "maxLength": helios_inspect::request_ids::LONGEST,
                    "description": "This change's key. Given again after an outcome unknown, the node applies the change once and answers `already_applied`. A new one when left out.",
                }),
            );
        }
        schema
    }

    /// Its annotations: what the registry knows.
    #[must_use]
    pub fn annotations(&self) -> Value {
        json!({ "title": self.spec.label, "readOnlyHint": self.spec.read_only })
    }

    /// Its entry in `tools/list`.
    #[must_use]
    pub fn listed(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description(),
            "inputSchema": self.input_schema(),
            "annotations": self.annotations(),
        })
    }

    /// The arguments of `helios_command_run` this call stands for, once its
    /// params fit the command's schema.
    pub fn run_arguments(&self, arguments: &Value) -> Result<Value, String> {
        let mut params = match arguments {
            Value::Object(o) => o.clone(),
            Value::Null => Map::new(),
            _ => return Err(format!("{}: its arguments are an object", self.name)),
        };
        let mut run = json!({ "id": self.spec.id, "scope": "node" });
        for (own, run_as) in [
            (VAULT, "vault"),
            (NODE, "node"),
            (WINDOW, "window"),
            (KEY, REQUEST_ID),
        ] {
            if let Some(v) = params.remove(own) {
                run[run_as] = v;
            }
        }
        let params = Value::Object(params);
        validate(&self.spec.params, &params).map_err(|e| {
            format!(
                "{}: params: {e} (its schema: helios_commands_list, filter `{}`)",
                self.name, self.spec.id
            )
        })?;
        run["params"] = params;
        Ok(run)
    }
}

/// `text` cut at [`LONGEST_DESCRIPTION`] bytes, on a character boundary,
/// with a pointer to the whole.
fn cut(text: String) -> String {
    if text.len() <= LONGEST_DESCRIPTION {
        return text;
    }
    let tail = "... (cut: helios_commands_list has it whole)";
    let mut end = LONGEST_DESCRIPTION - tail.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{tail}", &text[..end])
}

/// The `helios_command_run` tool generated tools run through.
pub(super) fn run_tool() -> &'static super::Tool {
    TOOLS
        .iter()
        .find(|t| t.name == "helios_command_run")
        .expect("the generic run tool is declared")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether `name` is a tool name every MCP client takes.
    fn mcp_valid(name: &str) -> bool {
        (1..=LONGEST_NAME).contains(&name.len())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    #[test]
    fn the_generated_tools_are_the_registry_s_commands_each_named_once() {
        let all = commands();
        assert!(
            all.len() <= MOST_TOOLS,
            "{} commands, over the cap: raise it or page the list",
            all.len()
        );
        assert_eq!(tools().len(), all.len());
        let mut names: Vec<&str> = tools().iter().map(|t| t.name.as_str()).collect();
        let sorted = names.clone();
        names.sort_unstable();
        assert_eq!(names, sorted, "sorted by name");
        names.dedup();
        assert_eq!(names.len(), tools().len(), "every name once");
        for tool in tools() {
            assert!(mcp_valid(&tool.name), "{}", tool.name);
            assert!(
                TOOLS.iter().all(|t| t.name != tool.name),
                "{} is a hand-written tool's name",
                tool.name
            );
            assert!(all.contains_key(&tool.spec.id));
            assert!(tool.description().len() <= LONGEST_DESCRIPTION);
            let schema = tool.input_schema();
            assert_eq!(schema["type"], "object", "{}", tool.name);
            let properties = schema["properties"].as_object().unwrap();
            for required in schema["required"].as_array().into_iter().flatten() {
                assert!(
                    properties.contains_key(required.as_str().unwrap()),
                    "{}: `{required}` required, not a property",
                    tool.name
                );
            }
            // The server's own arguments never shadow a command's param.
            for name in tool.spec.params["properties"]
                .as_object()
                .into_iter()
                .flat_map(Map::keys)
            {
                assert!(!name.starts_with('_'), "{}: `{name}`", tool.name);
            }
            serde_json::to_string(&tool.listed()).unwrap();
            assert_eq!(tool.annotations()["readOnlyHint"], tool.spec.read_only);
        }
        assert!(find("helios_cmd_library_stacks_list").is_some());
        assert_eq!(tool_name("job.cancel"), "helios_cmd_job_cancel");
    }

    #[test]
    fn a_call_runs_its_command_as_the_generic_run_and_bad_params_are_refused() {
        let tool = find("helios_cmd_library_stacks_dissolve").unwrap();
        let run = tool
            .run_arguments(&json!({ "stack": "s1", "_vault": "/v.helios", "_request_id": "k1" }))
            .unwrap();
        assert_eq!(
            run,
            json!({ "id": "library.stacks.dissolve", "scope": "node", "params": { "stack": "s1" },
                    "vault": "/v.helios", "request_id": "k1" })
        );
        let request = run_tool().request(&run).unwrap();
        assert_eq!(request["_meta"][REQUEST_ID], "k1");
        let e = tool.run_arguments(&json!({})).unwrap_err();
        assert!(e.contains("`stack` is missing"), "{e}");
        assert!(
            e.starts_with("helios_cmd_library_stacks_dissolve: params"),
            "{e}"
        );
    }

    #[test]
    fn a_long_description_is_cut_with_a_pointer() {
        let text = cut("é".repeat(LONGEST_DESCRIPTION));
        assert!(text.len() <= LONGEST_DESCRIPTION);
        assert!(text.ends_with("helios_commands_list has it whole)"));
    }
}
