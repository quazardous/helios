//! Menus are declared, never drawn by whoever fills them: a menu is an id
//! (`system`, `panel.tab`, `library.cell`) to which the shell, a panel or a
//! plugin contributes items. An item names a command (its title and keys
//! come from the command and the keymap, so a menu never shows a key that
//! does not work), or a submenu, or a list the shell fills when the menu
//! opens (the themes, the profiles). Items sit in groups, separated when
//! shown, and in an order within their group; a `when` clause hides or
//! greys an item, a `checked` clause ticks it.

use serde::Serialize;
use serde_json::Value;

use crate::when::{Context, When};

/// What an item is.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Item {
    /// A command, with the arguments it runs with.
    Command { id: String, args: Value },
    /// Another menu, shown under a label.
    Submenu { menu: String, label: String },
    /// Entries the shell makes when the menu opens (`themes`,
    /// `profiles`, `recent`).
    List { name: String },
}

/// One contribution to a menu.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MenuItem {
    pub menu: String,
    pub item: Item,
    /// Items of a group are shown together; groups are separated and
    /// ordered by name (`1_open`, `2_preferences`).
    pub group: String,
    pub order: i32,
    /// Where it applies; empty: everywhere.
    pub when: String,
    /// Greyed rather than hidden when `when` does not hold.
    pub grey: bool,
    /// When it shows ticked; empty: never.
    pub checked: String,
}

impl MenuItem {
    pub fn command(menu: &str, group: &str, order: i32, id: &str) -> Self {
        Self {
            menu: menu.into(),
            item: Item::Command {
                id: id.into(),
                args: Value::Null,
            },
            group: group.into(),
            order,
            when: String::new(),
            grey: true,
            checked: String::new(),
        }
    }

    pub fn submenu(menu: &str, group: &str, order: i32, sub: &str, label: &str) -> Self {
        Self {
            item: Item::Submenu {
                menu: sub.into(),
                label: label.into(),
            },
            ..Self::command(menu, group, order, "")
        }
    }

    pub fn list(menu: &str, group: &str, order: i32, name: &str) -> Self {
        Self {
            item: Item::List { name: name.into() },
            ..Self::command(menu, group, order, "")
        }
    }

    pub fn args(mut self, args: Value) -> Self {
        if let Item::Command { args: a, .. } = &mut self.item {
            *a = args;
        }
        self
    }

    pub fn when(mut self, when: &str, grey: bool) -> Self {
        self.when = when.into();
        self.grey = grey;
        self
    }

    pub fn checked(mut self, when: &str) -> Self {
        self.checked = when.into();
        self
    }
}

/// An item as a menu shows it in a context.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    Item {
        item: Item,
        enabled: bool,
        checked: bool,
    },
    Separator,
}

#[derive(Default)]
pub struct Menus {
    items: Vec<MenuItem>,
}

impl Menus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an item; its clauses must parse.
    pub fn contribute(&mut self, item: MenuItem) -> Result<(), String> {
        for clause in [&item.when, &item.checked] {
            When::parse(clause).map_err(|e| format!("menu `{}`: {e}", item.menu))?;
        }
        self.items.push(item);
        Ok(())
    }

    /// The items of a menu in a context: groups in order, separated,
    /// items hidden or greyed by their `when`.
    pub fn show(&self, menu: &str, cx: &Context) -> Vec<Shown> {
        let holds =
            |clause: &str| clause.is_empty() || When::parse(clause).is_ok_and(|w| w.eval(cx));
        let mut items: Vec<&MenuItem> = self.items.iter().filter(|i| i.menu == menu).collect();
        items.sort_by(|a, b| (&a.group, a.order).cmp(&(&b.group, b.order)));
        let mut out = Vec::new();
        let mut group: Option<&str> = None;
        for i in items {
            let applies = holds(&i.when);
            if !applies && !i.grey {
                continue;
            }
            if group.is_some_and(|g| g != i.group) {
                out.push(Shown::Separator);
            }
            group = Some(&i.group);
            out.push(Shown::Item {
                item: i.item.clone(),
                enabled: applies,
                checked: !i.checked.is_empty() && holds(&i.checked),
            });
        }
        out
    }

    /// Every menu id contributed to.
    pub fn ids(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.items.iter().map(|i| i.menu.as_str()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}
