//! Message ids, derived mechanically from stable ids.
//!
//! A command, a panel or a module parameter is known by an id that never
//! changes with its English label (`view.palette`, `helios.graph`, the
//! `exposure` parameter of the `exposure` module). Its message id in the
//! catalogs is derived from that id alone, so rewording English never
//! breaks a translation's key, and translating never touches an id that
//! keymaps, scripts or agents use.
//!
//! The scheme:
//!
//! | What | Message | Attribute |
//! |---|---|---|
//! | a command's title | `command-<id>` | `.title` |
//! | a panel's title | `panel-<id>` | `.title` |
//! | a parameter's label | `param-<module>-<name>` | `.label` |
//! | a parameter's description | `param-<module>-<name>` | `.description` |
//! | a choice's label | `param-<module>-<name>-choice-<value>` | (the value) |
//!
//! where every id is written with [`slug`]: each character other than an
//! ASCII letter or digit becomes `-` (`vault.rate.3` gives
//! `command-vault-rate-3`, `decide_exposure` gives `decide-exposure`). Two
//! ids that differ only by those characters would share a message: the
//! English catalogue's generation refuses them.
//!
//! The command and panel keys are the ones egui-pop derives for every
//! application (`egui_pop_core::strings`), so its menus, palette and
//! workbench find Helios's translations; a plugin adding its own
//! catalogue is held to its own keys by `egui_pop_i18n::catalog::owned_by`.

use std::fmt;

/// A message in a catalog, with the attribute looked up (if any).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageId {
    pub id: String,
    pub attr: Option<String>,
}

impl MessageId {
    /// A message's value.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            attr: None,
        }
    }

    /// One attribute of a message.
    pub fn attr(id: impl Into<String>, attr: &str) -> Self {
        Self {
            id: id.into(),
            attr: Some(attr.into()),
        }
    }

    /// The key as the checks name it: `id` or `id.attr`.
    pub fn key(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.attr {
            Some(a) => write!(f, "{}.{a}", self.id),
            None => f.write_str(&self.id),
        }
    }
}

/// A stable id written as a Fluent identifier's part: ASCII letters and
/// digits are kept, every other character becomes `-`.
pub fn slug(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The message of a command: `command-<id>`.
pub fn command(id: &str) -> String {
    format!("command-{}", slug(id))
}

/// A command's title.
pub fn command_title(id: &str) -> MessageId {
    MessageId::attr(command(id), "title")
}

/// The message of a panel: `panel-<id>`.
pub fn panel(id: &str) -> String {
    format!("panel-{}", slug(id))
}

/// A panel's title.
pub fn panel_title(id: &str) -> MessageId {
    MessageId::attr(panel(id), "title")
}

/// The message of a module's parameter: `param-<module>-<name>`.
pub fn param(module: &str, name: &str) -> String {
    format!("param-{}-{}", slug(module), slug(name))
}

/// A parameter's short label.
pub fn param_label(module: &str, name: &str) -> MessageId {
    MessageId::attr(param(module, name), "label")
}

/// A parameter's description.
pub fn param_description(module: &str, name: &str) -> MessageId {
    MessageId::attr(param(module, name), "description")
}

/// The label of one value of a choice parameter.
pub fn param_choice(module: &str, name: &str, value: &str) -> MessageId {
    MessageId::new(format!("{}-choice-{}", param(module, name), slug(value)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_derived_from_stable_ids() {
        assert_eq!(
            command_title("view.palette").key(),
            "command-view-palette.title"
        );
        assert_eq!(command_title("vault.rate.3").id, "command-vault-rate-3");
        assert_eq!(
            panel_title("helios.graph").key(),
            "panel-helios-graph.title"
        );
        assert_eq!(
            param_label("decide_exposure", "compensate_bias").key(),
            "param-decide-exposure-compensate-bias.label"
        );
        assert_eq!(
            param_description("exposure", "black").key(),
            "param-exposure-black.description"
        );
        assert_eq!(
            param_choice("demosaic", "method", "rcd").key(),
            "param-demosaic-method-choice-rcd"
        );
        assert_eq!(slug("a b/é"), "a-b--");
    }
}
