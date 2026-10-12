//! Helios as egui-pop plugins: its commands declared (run by the host,
//! see `host`), with their keys and menu entries; the system menu's lists
//! (profiles, recent workspaces), declared again when they may have
//! changed; and the areas, a block of egui-pop's main zone.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::app::{STEP_REDO, STEP_UNDO};
use egui_pop::api::steps::Before;
use egui_pop::api::zones::{self, Block};
use egui_pop::api::{Ctx, Plugin, PluginError};
use egui_pop::model::commands::CommandSpec;
use egui_pop::model::ids::{BlockId, CommandId, MenuId, PluginId};
use egui_pop::model::keymap::Layer;
use egui_pop::model::menus::MenuItem;
use helios_shell::commands::Registry;
use serde_json::json;

use crate::workbench::Workbench;

/// The system menu, opened by the logo (Alt+Space too).
pub const SYSTEM: MenuId = MenuId::from_static("helios.system");
pub const SYSTEM_RECENT: MenuId = MenuId::from_static("helios.system.recent");
pub const SYSTEM_PROFILES: MenuId = MenuId::from_static("helios.system.profiles");
pub const SYSTEM_THEMES: MenuId = MenuId::from_static("helios.system.themes");

pub const COMMANDS: PluginId = PluginId::from_static("helios.commands");
pub const LISTS: PluginId = PluginId::from_static("helios.lists");
pub const WORKBENCH: PluginId = PluginId::from_static("helios.workbench");
pub const HISTORY: PluginId = PluginId::from_static("helios.history");

/// What Helios declares beside its commands' registry entries: default
/// keys (with their `when`), menu entries, ticks, commands kept out of the
/// palette.
#[derive(Default)]
pub struct Declarations {
    /// Keys, command, `when`.
    pub keys: Vec<(String, String, String)>,
    pub menus: Vec<MenuItem>,
    /// A command's `checked` clause.
    pub checked: BTreeMap<String, String>,
    pub hidden: BTreeSet<String>,
}

/// A command's id, checked.
pub fn command_id(id: &str) -> Result<CommandId, PluginError> {
    CommandId::new(id).map_err(|e| PluginError::Failed(e.to_string()))
}

/// A command's title as a menu shows it: without the hint of its
/// arguments (`Open a workspace (`path`)`), which the palette keeps.
pub fn menu_title(title: &str) -> String {
    match title.rfind(" (`") {
        Some(i) if title.ends_with(')') => title[..i].to_string(),
        _ => title.to_string(),
    }
}

/// Every command of Helios's registry, declared to egui-pop: its menus,
/// palette, keymap and traces see them, and the host runs them.
pub struct Commands {
    pub registry: Arc<Mutex<Registry>>,
    pub declarations: Rc<RefCell<Declarations>>,
}

impl Plugin for Commands {
    fn id(&self) -> PluginId {
        COMMANDS
    }

    fn title(&self) -> String {
        "Helios".into()
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        let d = self.declarations.borrow();
        let specs: Vec<_> = self
            .registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .specs()
            .cloned()
            .collect();
        for spec in specs {
            let mut s = CommandSpec::new(command_id(&spec.id)?, &spec.title, &spec.category)
                .when(&spec.when);
            if let Some(c) = d.checked.get(&spec.id) {
                s = s.checked(c);
            }
            if d.hidden.contains(&spec.id) {
                s = s.hidden();
            }
            ctx.declare(s)?;
        }
        for (keys, command, when) in &d.keys {
            ctx.keybinding(keys, command_id(command)?, when, Layer::Default)?;
        }
        for item in &d.menus {
            ctx.menu_item(item.clone())?;
        }
        Ok(())
    }
}

/// Where the system menu's lists come from.
#[derive(Clone, Debug, Default)]
pub struct ListSource {
    pub profiles_dir: PathBuf,
}

/// The system menu's lists: the saved profiles (the one in use ticked)
/// and the recent workspaces. egui-pop's menus have no list filled when
/// a menu opens, so the shell connects this plugin again before showing
/// them.
pub struct Lists {
    pub source: Rc<RefCell<ListSource>>,
}

impl Plugin for Lists {
    fn id(&self) -> PluginId {
        LISTS
    }

    fn title(&self) -> String {
        "Helios lists".into()
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        let dir = self.source.borrow().profiles_dir.clone();
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .strip_suffix(".helios-profile")
                    .map(String::from)
            })
            .collect();
        if !names.contains(&"default".to_string()) {
            names.push("default".into());
        }
        names.sort();
        let use_profile = command_id("profile.use")?;
        for (i, n) in names.iter().enumerate() {
            let mut item = MenuItem::command(SYSTEM_PROFILES, "1", i as i32, use_profile.clone())
                .args(json!({ "name": n }))
                .label(n);
            if !n.contains('\'') {
                item = item.checked(&format!("heliosProfile == '{n}'"));
            }
            ctx.menu_item(item)?;
        }
        let open = command_id("workspace.open")?;
        let recent = crate::shell::recent_workspaces();
        if recent.is_empty() {
            // Greyed: a context key nothing sets.
            ctx.menu_item(
                MenuItem::command(SYSTEM_RECENT, "1", 0, open.clone())
                    .label("No recent workspace")
                    .when("heliosRecentWorkspace", true),
            )?;
        }
        for (i, p) in recent.iter().enumerate() {
            ctx.menu_item(
                MenuItem::command(SYSTEM_RECENT, "1", i as i32, open.clone())
                    .args(json!({ "path": p }))
                    .label(&p.display().to_string()),
            )?;
        }
        Ok(())
    }
}

/// Undo and redo wait for a change in progress: while a gesture (a drag)
/// holds values, egui-pop's `edit.undo` and `edit.redo` steps are vetoed,
/// with the reason, rather than undoing under the user's hand.
pub struct History {
    pub values: Arc<Mutex<helios_values::Store>>,
}

impl Plugin for History {
    fn id(&self) -> PluginId {
        HISTORY
    }

    fn title(&self) -> String {
        "Helios history".into()
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        for step in [STEP_UNDO, STEP_REDO] {
            let values = self.values.clone();
            ctx.before(step, move |_, _| {
                let v = values.lock().unwrap_or_else(PoisonError::into_inner);
                match v.open_gesture() {
                    Some(g) => Before::Veto(format!("{g} is in progress")),
                    None => Before::Go,
                }
            });
        }
        Ok(())
    }
}

/// The areas, drawn in egui-pop's main zone.
pub struct Areas {
    pub workbench: Rc<RefCell<Workbench>>,
}

impl Plugin for Areas {
    fn id(&self) -> PluginId {
        WORKBENCH
    }

    fn title(&self) -> String {
        "Helios areas".into()
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        let wb = self.workbench.clone();
        ctx.block(
            zones::MAIN,
            Block::add(BlockId::from_static("helios.areas"), 0, move |ui, _| {
                wb.borrow_mut().ui(ui);
            }),
        );
        Ok(())
    }
}
