//! The JSON Schemas of Helios's commands' arguments, as egui-pop's
//! `CommandSpec::schema` takes them.
//!
//! egui-pop checks a run's arguments against its command's schema before
//! the command is asked, for every origin, and refuses a mismatch on the
//! journal (`command.refused`, `by: "pop.command.args"`); an agent hears
//! the refusal at once, and MCP publishes the schema as the tool's
//! `inputSchema`. `plugins::declared` applies [`schema`].
//!
//! The schemas are written loose on purpose: each names the arguments
//! the command reads and the type of those it reads one way only, never
//! `additionalProperties: false`. A run the application itself makes is
//! checked too, so a schema stricter than its handler would refuse the
//! user's own clicks. An argument whose handler takes more than one shape
//! is left without a type.
//!
//! The first schemas are those of the commands agents call most (the
//! scenarios', quick open's), and of every destructive and private
//! command; a test lists the commands with arguments still without one,
//! each with its reason.

use serde_json::{Map, Value as Json, json};

/// An object schema with these properties, `required` ones first.
fn object(required: &[&str], props: &[(&str, Json)]) -> Json {
    let props: Map<String, Json> = props
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    let mut s = json!({ "type": "object", "properties": props });
    if !required.is_empty() {
        s["required"] = json!(required);
    }
    s
}

/// Any value: an argument read in more than one shape.
fn any() -> Json {
    json!({})
}

fn string() -> Json {
    json!({ "type": "string" })
}

fn string_or_null() -> Json {
    json!({ "type": ["string", "null"] })
}

fn boolean() -> Json {
    json!({ "type": "boolean" })
}

fn integer() -> Json {
    json!({ "type": "integer" })
}

/// Photo ids; absent (or null), the selection.
fn images() -> Json {
    json!({ "type": ["array", "null"] })
}

/// Absolute paths.
fn paths() -> Json {
    json!({ "type": "array", "items": { "type": "string" } })
}

/// The schema of `command`'s arguments, when it has one.
pub fn schema(command: &str) -> Option<Json> {
    Some(match command {
        // The scenarios' and quick open's.
        "param.get" | "param.reset" | "library.openFolder" | "library.scanFolder" => {
            object(&["path"], &[("path", string())])
        }
        "param.set" => object(
            &["path"],
            &[
                ("path", string()),
                ("value", any()),
                ("step", integer()),
                ("coarse", integer()),
                ("reset", boolean()),
                ("top", boolean()),
                ("bottom", boolean()),
                ("force", boolean()),
                ("if_seq", integer()),
            ],
        ),
        "workbench.quickOpen.go" => object(&["item"], &[("item", string())]),
        "library.importFiles" => object(
            &["paths"],
            &[
                ("paths", paths()),
                ("copy", boolean()),
                ("into", any()),
                ("layout", any()),
                ("rename", any()),
                ("duplicates", any()),
                ("ask", boolean()),
                ("darktable", boolean()),
                ("camera_look", any()),
            ],
        ),
        "library.importSource" => object(
            &[],
            &[
                ("path", string_or_null()),
                ("copy", boolean()),
                ("darktable", boolean()),
                ("remember", boolean()),
            ],
        ),
        "library.importWith" => object(
            &[],
            &[
                ("plugin", string()),
                ("importer", string()),
                ("source", string_or_null()),
                ("copy", boolean()),
                ("darktable", boolean()),
            ],
        ),
        // Folders and files (private).
        "library.folder" | "library.chooseFolder" | "init.libraryFolder" | "workspace.open" => {
            object(&[], &[("path", string_or_null())])
        }
        "init.apply" => object(&[], &[("library.folder", string())]),
        "calibrate.open" => object(
            &[],
            &[
                ("path", string_or_null()),
                ("corners", any()),
                ("camera", any()),
            ],
        ),
        "workspace.new" => object(&[], &[("template", any()), ("path", string_or_null())]),
        "workspace.parts" | "library.importWindow" => object(&[], &[("source", string_or_null())]),
        "profile.export" => object(
            &[],
            &[
                ("parts", any()),
                ("this_machine", boolean()),
                ("path", string_or_null()),
            ],
        ),
        // Accounts (private; sign out and disconnect destructive).
        "import.signOut" | "import.reconnect" => object(&["account"], &[("account", any())]),
        "import.disconnect" => object(
            &["account"],
            &[
                ("account", any()),
                ("sources", json!({ "enum": ["keep", "forget"] })),
            ],
        ),
        "import.history.list" => object(&[], &[("limit", integer())]),
        "import.sources.add" => object(
            &["connector"],
            &[
                ("connector", string()),
                ("path", string_or_null()),
                ("volume", any()),
                ("account", any()),
                ("name", string_or_null()),
                ("profile", any()),
                ("everyday", any()),
                ("auto", any()),
                ("params", any()),
            ],
        ),
        // Albums.
        "library.collection.create" => object(
            &[],
            &[
                ("name", string_or_null()),
                ("images", images()),
                ("kind", any()),
                ("rules", any()),
                ("set", any()),
            ],
        ),
        "library.collection.delete" => object(&["id"], &[("id", any()), ("members", boolean())]),
        "library.collection.remove" => object(&["id"], &[("id", any()), ("images", images())]),
        "library.collection.rename" => {
            object(&["id"], &[("id", any()), ("name", string_or_null())])
        }
        "library.quick.saveAs" => object(
            &[],
            &[
                ("id", any()),
                ("name", string_or_null()),
                ("clear", boolean()),
            ],
        ),
        "library.quick.create" => object(
            &[],
            &[
                ("name", string_or_null()),
                ("images", images()),
                ("target", boolean()),
            ],
        ),
        "library.quick.rename" => object(&["id"], &[("id", any()), ("name", string_or_null())]),
        "library.quick.delete" | "library.quick.target" => object(&["id"], &[("id", any())]),
        "library.quick.toggle" => object(
            &[],
            &[("id", any()), ("images", images()), ("add", boolean())],
        ),
        "library.quick.add" | "library.quick.remove" => {
            object(&[], &[("id", any()), ("images", images())])
        }
        "library.quick.list" => object(&[], &[]),
        "library.quick.move" => object(
            &["from"],
            &[("from", string()), ("id", any()), ("images", images())],
        ),
        "library.quick.open" => object(&[], &[("id", any()), ("place", string_or_null())]),
        // Filters.
        "library.filter.apply" => object(&[], &[("query", any()), ("filter", any())]),
        "library.filter.pin" => object(
            &[],
            &[("query", any()), ("filter", any()), ("pinned", boolean())],
        ),
        // Keywords.
        "library.keyword" | "library.keyword.list" => object(&[], &[("path", string_or_null())]),
        "library.keyword.add" | "library.keyword.remove" => {
            object(&["keyword"], &[("keyword", string()), ("images", images())])
        }
        "library.keyword.merge" => {
            object(&["from"], &[("from", string()), ("into", string_or_null())])
        }
        "library.keyword.rename" => {
            object(&["from"], &[("from", string()), ("to", string_or_null())])
        }
        // Marks and metadata.
        "library.marks.copy" => object(&[], &[("image", any())]),
        "library.marks.paste" => object(
            &[],
            &[
                ("images", images()),
                ("marks", any()),
                ("mode", json!({ "enum": ["merge", "overwrite"] })),
            ],
        ),
        "library.metadata.set" => object(
            &[],
            &[
                ("fields", json!({ "type": "object" })),
                ("keywords", json!({ "type": "object" })),
                ("labels", json!({ "type": "object" })),
                ("images", images()),
            ],
        ),
        "library.metadata.template.apply" => {
            object(&["name"], &[("name", string()), ("images", images())])
        }
        "library.metadata.template.delete" => object(&["name"], &[("name", string())]),
        "library.metadata.template.save" => object(
            &["name"],
            &[("name", string()), ("fields", any()), ("keywords", any())],
        ),
        // Photos deleted, removed or reset (destructive).
        "library.trash" | "library.deleteRejected" => {
            object(&[], &[("images", images()), ("confirmed", boolean())])
        }
        "library.removeVersion" => object(
            &[],
            &[
                ("image", any()),
                ("version", any()),
                ("confirmed", boolean()),
            ],
        ),
        "library.export" => object(
            &[],
            &[
                ("images", images()),
                ("scope", json!({ "enum": ["selected", "shown"] })),
                ("format", json!({ "enum": ["jpeg", "png", "tiff"] })),
                ("quality", any()),
                ("bits", any()),
                ("long_edge", any()),
                ("space", any()),
                ("folder", string_or_null()),
                ("pattern", any()),
                ("clash", json!({ "enum": ["number", "overwrite", "skip"] })),
                ("remember", boolean()),
                ("ask", boolean()),
            ],
        ),
        // Other photo commands.
        "library.compare"
        | "library.duplicate"
        | "library.duplicateOriginal"
        | "library.rotateLeft"
        | "library.rotateRight"
        | "library.remove"
        | "library.resetRotation" => object(&[], &[("images", images())]),
        "library.fitCameraLook" => object(&[], &[("images", images()), ("default", any())]),
        "library.show" => object(&[], &[("ids", images())]),
        "library.sort" => object(&[], &[("by", string_or_null()), ("descending", boolean())]),
        "library.collection" | "library.quick.clear" => object(&[], &[("id", any())]),
        "library.importSession" => object(&[], &[("id", any()), ("select", boolean())]),
        "library.importAutomatic.pause" => object(&[], &[("on", boolean())]),
        "library.select.similar" => object(&[], &[("by", any())]),
        "library.view.open" => object(&[], &[("place", string_or_null())]),
        "library.view.close" => object(&["view"], &[("view", string())]),
        // The network (destructive).
        "node.reset" => object(&[], &[("node", any())]),
        "mask.member.delete" => object(&[], &[("member", any())]),
        "network.remove" => object(&["nodes"], &[("nodes", json!({ "type": "array" }))]),
        "network.disconnect" => object(
            &["connections"],
            &[("connections", json!({ "type": "array" }))],
        ),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The commands of the English catalogue (generated from Helios's
    /// declarations), each with the arguments its title names in
    /// backticks.
    fn catalogue() -> Vec<(String, Vec<String>)> {
        let ftl = include_str!("../../../i18n/en/commands.ftl");
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for line in ftl.lines() {
            if let Some(id) = line.strip_prefix("command-") {
                let id = id.trim_end_matches(" =").trim().replace('-', ".");
                out.push((id, Vec::new()));
            } else if let Some(title) = line.trim().strip_prefix(".title = ")
                && let Some((_, args)) = out.last_mut()
            {
                args.extend(title.split('`').skip(1).step_by(2).map(String::from));
            }
        }
        out
    }

    /// Commands that take arguments and have no schema yet, by id or by
    /// prefix (`library.stack.`), and why. A command taking `dry_run` is
    /// left out by the test itself: Helios declares `dry_run` as its
    /// arguments' example, which egui-pop reads.
    const NO_SCHEMA_YET: &[(&str, &str)] = &[
        (
            "library.filter.recent",
            "takes no arguments: its title names its answer's fields",
        ),
        (
            "library.metadata.get",
            "takes no arguments: its title names its answer's fields",
        ),
        (
            "library.storage",
            "the storage page, being reworked; its schema comes with it",
        ),
        (
            "workshop.",
            "experimental: its arguments change with each slice",
        ),
        (
            "controls.",
            "the controls layer, still being shaped (`controls.describe` gives its own schemas)",
        ),
        (
            "mask.",
            "mask edits beyond deletion: a schema per shape, later",
        ),
        ("network.", "network edits beyond removal, later"),
        ("node.", "node edits beyond reset, later"),
        ("subworkspace.", "sub-workspace edits, later"),
        ("workspace.", "workspace edits beyond opening, later"),
        ("viewport.", "a view, later"),
        ("compare.", "a view, later"),
        ("view.", "a view, later"),
        (
            "helios.",
            "panel placement (`moveTo`, `float`), egui-pop's panel arguments",
        ),
        (
            "keymap.",
            "the keymap's package names, checked by the keymap",
        ),
        ("library.stack.", "stack edits, later"),
        ("library.files.", "file pairing, later"),
        (
            "library.collection.",
            "the rule editor and smart albums, later",
        ),
        ("vault.", "marks on the selection, later"),
        ("import.", "import steps beyond accounts and sources, later"),
        ("param.", "value commands beyond get, set and reset, later"),
        ("health.", "the health page's reads and fixes, later"),
        ("workbench.", "workbench steps beyond quick open, later"),
        ("decision.", "the decision book, later"),
        ("dialog.", "answers to Helios's own dialogs, later"),
        ("activity.", "a view, later"),
        ("notice.", "a view, later"),
    ];

    fn excepted(id: &str) -> bool {
        NO_SCHEMA_YET.iter().any(|(p, _)| {
            if p.ends_with('.') {
                id.starts_with(p)
            } else {
                id == *p
            }
        })
    }

    #[test]
    fn a_command_with_arguments_has_a_schema_or_a_reason() {
        let catalogue = catalogue();
        assert!(catalogue.len() > 200, "the catalogue was read");
        let missing: Vec<_> = catalogue
            .iter()
            .filter(|(_, args)| !args.is_empty() && !args.iter().any(|a| a == "dry_run"))
            .map(|(id, _)| id.as_str())
            .filter(|id| schema(id).is_none() && !excepted(id))
            .collect();
        assert!(
            missing.is_empty(),
            "these commands take arguments: give them a schema in \
             schemas::schema, or a reason in NO_SCHEMA_YET: {missing:?}"
        );
    }

    #[test]
    fn a_per_call_destructive_rule_names_a_value_its_schema_allows() {
        for (id, arg, value) in crate::destructive::DESTRUCTIVE_IF {
            let s = schema(id).unwrap_or_else(|| panic!("{id} has no schema"));
            let allowed = &s["properties"][arg]["enum"];
            assert!(
                allowed
                    .as_array()
                    .is_some_and(|a| a.contains(&json!(value))),
                "{id}: `{arg}: {value}` is not a value its schema allows ({allowed})"
            );
        }
    }

    #[test]
    fn every_destructive_command_with_arguments_has_a_schema() {
        let catalogue = catalogue();
        let missing: Vec<_> = crate::destructive::DESTRUCTIVE
            .iter()
            .filter(|id| {
                catalogue.iter().any(|(c, args)| {
                    c == *id && !args.is_empty() && !args.iter().any(|a| a == "dry_run")
                })
            })
            .filter(|id| schema(id).is_none())
            .collect();
        assert!(
            missing.is_empty(),
            "destructive without a schema: {missing:?}"
        );
    }

    #[test]
    fn every_schema_is_one_egui_pop_enforces_and_names_a_command() {
        let catalogue = catalogue();
        for (id, _) in &catalogue {
            if let Some(s) = schema(id) {
                assert!(egui_pop::core::debug::is_schema(&s), "{id}: {s}");
                // An empty call (null, checked as `{}`) passes unless an
                // argument is required.
                let required = s["required"].as_array().map_or(0, Vec::len);
                assert_eq!(
                    egui_pop::core::schema::check(&s, &json!({})).is_ok(),
                    required == 0,
                    "{id}"
                );
            }
        }
    }

    #[test]
    fn a_wrong_argument_is_refused_and_a_right_one_passes() {
        let trash = schema("library.trash").unwrap();
        let check = egui_pop::core::schema::check;
        assert!(check(&trash, &json!({ "images": ["a1"], "confirmed": true })).is_ok());
        assert!(check(&trash, &json!({ "images": "a1" })).is_err());
        assert!(check(&trash, &json!({ "confirmed": "yes" })).is_err());
        let set = schema("param.set").unwrap();
        assert!(
            check(
                &set,
                &json!({ "path": "node/exposure/exposure", "value": 1.0 })
            )
            .is_ok()
        );
        assert!(check(&set, &json!({ "value": 1.0 })).is_err());
    }
}
