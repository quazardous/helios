//! Quick open (`workbench.quickOpen`, `Ctrl+P`): one filtered list of
//! what can be gone to by name, beside egui-pop's command palette
//! (`Ctrl+Shift+P`), which holds the commands.
//!
//! Its sources:
//!
//! - **modules**: the nodes of the network shown, by name; choosing one
//!   selects it and brings the Parameters panel forward;
//! - **sub-workspaces**: the workspace's, by name; choosing one enters
//!   it (`subworkspace.enter`);
//! - **images**: the library's, by file name; choosing one opens it in
//!   the viewport, as a launch or a link would;
//! - **workspaces**: the recent ones; choosing one opens it in a window
//!   of its own.
//!
//! The list is the shell's picker (egui-pop's choice dialog: a filter
//! field, the arrows, Enter). Each entry's value names its item
//! (`module:<node>`, `subworkspace:<id>`, `image:<id>`,
//! `workspace:<path>`); the picker runs
//! `workbench.quickOpen.go` with it, which agents may run too.

use std::path::PathBuf;

use helios_core::Message;

use crate::dialogs::Choice;

/// Opens the list.
pub const COMMAND: &str = "workbench.quickOpen";
/// Goes to an item (`item`).
pub const GO: &str = "workbench.quickOpen.go";

/// Something quick open goes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A node of the network shown, by name.
    Module(String),
    /// A sub-workspace of the workspace, by id.
    Subworkspace(String),
    /// An image of the library, by its vault id.
    Image(String),
    /// A workspace directory.
    Workspace(PathBuf),
}

impl Item {
    /// The value its entry carries: `module:<node>`,
    /// `subworkspace:<id>`, `image:<id>`, `workspace:<path>`.
    pub fn value(&self) -> String {
        match self {
            Item::Module(n) => format!("module:{n}"),
            Item::Subworkspace(id) => format!("subworkspace:{id}"),
            Item::Image(id) => format!("image:{id}"),
            Item::Workspace(p) => format!("workspace:{}", p.display()),
        }
    }

    /// An entry's value read back.
    pub fn parse(value: &str) -> Result<Item, String> {
        let (kind, rest) = value.split_once(':').ok_or_else(|| {
            format!("`{value}`: not module:, subworkspace:, image: or workspace:")
        })?;
        if rest.is_empty() {
            return Err(format!("`{value}` names nothing"));
        }
        match kind {
            "module" => Ok(Item::Module(rest.into())),
            "subworkspace" => Ok(Item::Subworkspace(rest.into())),
            "image" => Ok(Item::Image(rest.into())),
            "workspace" => Ok(Item::Workspace(rest.into())),
            _ => Err(format!(
                "`{value}`: not module:, subworkspace:, image: or workspace:"
            )),
        }
    }
}

/// What quick open offers, gathered by the shell when it opens.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    /// The network's nodes: name, module (none for a source).
    pub modules: Vec<(String, Option<String>)>,
    /// The workspace's sub-workspaces: id, name.
    pub subworkspaces: Vec<(String, String)>,
    /// The library's images: id, file name.
    pub images: Vec<(String, String)>,
    /// The recent workspaces.
    pub workspaces: Vec<PathBuf>,
}

/// The picker's entries: the modules, then the sub-workspaces, then the
/// images, then the workspaces, each with what it is beside it in the interface's
/// language (`text` translates).
pub fn choices(sources: &Sources, text: impl Fn(Message) -> String) -> Vec<Choice> {
    let mut out = Vec::new();
    for (node, module) in &sources.modules {
        let detail = match module {
            Some(m) => {
                text(Message::new("gui-quick-open-module", "module { $module }").with("module", m))
            }
            None => text(Message::new("gui-quick-open-source", "source")),
        };
        out.push(Choice {
            value: Item::Module(node.clone()).value(),
            label: node.clone(),
            detail: Some(detail),
        });
    }
    for (id, name) in &sources.subworkspaces {
        out.push(Choice {
            value: Item::Subworkspace(id.clone()).value(),
            label: name.clone(),
            detail: Some(text(Message::new(
                "gui-quick-open-subworkspace",
                "sub-workspace",
            ))),
        });
    }
    for (id, name) in &sources.images {
        out.push(Choice {
            value: Item::Image(id.clone()).value(),
            label: name.clone(),
            detail: Some(text(Message::new("gui-quick-open-image", "image"))),
        });
    }
    for dir in &sources.workspaces {
        let name = dir
            .file_name()
            .map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into());
        out.push(Choice {
            value: Item::Workspace(dir.clone()).value(),
            label: name,
            detail: Some(text(
                Message::new("gui-quick-open-workspace", "workspace { $path }")
                    .with("path", dir.display().to_string()),
            )),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_name_their_item_and_read_back() {
        for item in [
            Item::Module("exposure".into()),
            Item::Subworkspace("skin".into()),
            Item::Image("0a1b".into()),
            Item::Workspace("/photos/2026".into()),
        ] {
            assert_eq!(Item::parse(&item.value()).unwrap(), item);
        }
        assert!(Item::parse("exposure").is_err());
        assert!(Item::parse("image:").is_err());
        assert!(Item::parse("node:x").is_err());
    }

    #[test]
    fn every_source_gives_its_entries_in_order() {
        let sources = Sources {
            modules: vec![
                ("source".into(), None),
                ("exposure".into(), Some("exposure".into())),
            ],
            subworkspaces: vec![("skin".into(), "Skin & eyes".into())],
            images: vec![("0a1b".into(), "beach.raf".into())],
            workspaces: vec!["/photos/2026".into()],
        };
        let c = choices(&sources, |m| m.english);
        let values: Vec<&str> = c.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(
            values,
            [
                "module:source",
                "module:exposure",
                "subworkspace:skin",
                "image:0a1b",
                "workspace:/photos/2026"
            ]
        );
        assert_eq!(c[1].detail.as_deref(), Some("module exposure"));
        assert_eq!(c[2].label, "Skin & eyes");
        assert_eq!(c[2].detail.as_deref(), Some("sub-workspace"));
        assert_eq!(c[4].label, "2026");
    }
}
