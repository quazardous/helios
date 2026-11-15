//! The Library's metadata commands (`docs/design/library.md`, L13): a
//! group edit of the selection's metadata (one step Undo takes back),
//! the selection's shared values read, and the metadata templates saved,
//! applied, deleted and listed.

use serde_json::{Map, Value, json};

use super::Shell;
use crate::library::{
    META_GET_COMMAND, META_SET_COMMAND, MetaEdit, TEMPLATE_APPLY_COMMAND, TEMPLATE_DELETE_COMMAND,
    TEMPLATE_LIST_COMMAND, TEMPLATE_SAVE_COMMAND,
};

/// The commands, with their titles (category Library).
pub(super) const COMMANDS: &[(&str, &str)] = &[
    (
        META_SET_COMMAND,
        "Set metadata on photos as a group (`fields`: title, description, creator, copyright, rating, flag; `keywords` and `labels`: `add`, `remove`; `images`: ids, else the selection); fields not named keep each photo's value; Ctrl+Z takes it back",
    ),
    (
        META_GET_COMMAND,
        "Read the selection's metadata: each field's shared value, or `mixed`; the labels and keywords all or some photos have",
    ),
    (
        TEMPLATE_SAVE_COMMAND,
        "Save a metadata template (`name`), from `fields` and `keywords` when given, else from what the selected photos share",
    ),
    (
        TEMPLATE_APPLY_COMMAND,
        "Apply a metadata template (`name`) to photos (`images`: ids, else the selection); Ctrl+Z takes it back",
    ),
    (
        TEMPLATE_DELETE_COMMAND,
        "Delete a metadata template (`name`)",
    ),
    (
        TEMPLATE_LIST_COMMAND,
        "List the metadata templates, with their fields and keywords",
    ),
];

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect()
}

impl Shell {
    /// Runs one of [`COMMANDS`].
    pub(super) fn library_meta_command(&mut self, id: &str, args: &Value) -> Result<Value, String> {
        let images = match args.get("images") {
            None | Some(Value::Null) => None,
            Some(Value::Array(list)) => Some(
                list.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect::<Vec<_>>(),
            ),
            Some(other) => return Err(format!("`images`: {other} is not a list of ids")),
        };
        let name = || {
            args.get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(String::from)
                .ok_or("`name`: no template named")
        };
        let mut lib = self.focus.library.borrow_mut();
        match id {
            META_SET_COMMAND => {
                let fields = match args.get("fields") {
                    None | Some(Value::Null) => Map::new(),
                    Some(Value::Object(m)) => m.clone(),
                    Some(other) => return Err(format!("`fields`: {other} is not an object")),
                };
                let edit = MetaEdit {
                    fields,
                    keywords_add: strings(args.pointer("/keywords/add")),
                    keywords_remove: strings(args.pointer("/keywords/remove")),
                    labels_add: strings(args.pointer("/labels/add")),
                    labels_remove: strings(args.pointer("/labels/remove")),
                    ..MetaEdit::default()
                };
                Ok(json!({ "photos": lib.set_meta(images, edit)? }))
            }
            META_GET_COMMAND => lib.meta_of_selection(),
            TEMPLATE_SAVE_COMMAND => {
                let given = match (args.get("fields"), args.get("keywords")) {
                    (None | Some(Value::Null), None | Some(Value::Null)) => None,
                    (fields, keywords) => Some((
                        fields
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default(),
                        strings(keywords),
                    )),
                };
                Ok(json!({ "templates": lib.save_template(&name()?, given)? }))
            }
            TEMPLATE_APPLY_COMMAND => {
                Ok(json!({ "photos": lib.apply_template(&name()?, images)? }))
            }
            TEMPLATE_DELETE_COMMAND => Ok(json!({ "templates": lib.delete_template(&name()?)? })),
            TEMPLATE_LIST_COMMAND => {
                let list = lib
                    .templates()
                    .ok_or("the templates are not read yet")?
                    .iter()
                    .map(crate::library::Template::to_json)
                    .collect::<Vec<_>>();
                Ok(json!({ "templates": list }))
            }
            other => Err(format!("no metadata command `{other}`")),
        }
    }
}
