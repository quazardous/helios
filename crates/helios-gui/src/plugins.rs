//! Helios as egui-pop plugins: its commands declared (run by the host,
//! see `host`), with their keys and menu entries; the system menu's lists
//! (profiles, recent workspaces), filled by list providers when the menu
//! opens; undo and redo held while a change is in progress; the
//! window's frame opening Helios's system menu. Helios's panels are
//! declared by `workbench`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::app::{STEP_REDO, STEP_UNDO};
use egui_pop::api::frame::{self, FramePlugin, STEP_SYSTEM_MENU};
use egui_pop::api::menus::{ListItem, popup_at};
use egui_pop::api::steps::Before;
use egui_pop::api::zones::{self, Block};
use egui_pop::api::{AppHandle, Ctx, Plugin, PluginError};
use egui_pop::model::commands::CommandSpec;
use egui_pop::model::ids::{BlockId, CommandId, MenuId, PluginId};
use egui_pop::model::keymap::Layer;
use egui_pop::model::menus::MenuItem;
use helios_core::Message;
use helios_shell::commands::{CommandSpec as HeliosSpec, Registry};
use serde_json::json;

/// The system menu, opened by the logo (Alt+Space too).
pub const SYSTEM: MenuId = MenuId::from_static("helios.system");
pub const SYSTEM_RECENT: MenuId = MenuId::from_static("helios.system.recent");
pub const SYSTEM_PROFILES: MenuId = MenuId::from_static("helios.system.profiles");
pub const SYSTEM_THEMES: MenuId = MenuId::from_static("helios.system.themes");
/// A parameter's context menu (its path given by the widget).
pub const PARAM: MenuId = MenuId::from_static("helios.param");

pub const COMMANDS: PluginId = PluginId::from_static("helios.commands");
pub const LISTS: PluginId = PluginId::from_static("helios.lists");
pub const HISTORY: PluginId = PluginId::from_static("helios.history");
pub const FRAME: PluginId = PluginId::from_static("helios.frame");

/// What Helios declares beside its commands' registry entries: default
/// keys (with their `when`), menu entries, ticks, icons, commands kept
/// out of the palette.
#[derive(Default)]
pub struct Declarations {
    /// Keys, command, `when`.
    pub keys: Vec<(String, String, String)>,
    pub menus: Vec<MenuItem>,
    /// A command's `checked` clause.
    pub checked: BTreeMap<String, String>,
    /// A command's icon, by name in egui-pop's icon registry: shown in
    /// menus, the palette and wherever the command appears.
    pub icons: BTreeMap<String, String>,
    pub hidden: BTreeSet<String>,
}

/// The icons of Helios's commands, from egui-pop's built-in set (the
/// Phosphor font stays off): only where the icon says what the command
/// does.
pub const COMMAND_ICONS: &[(&str, &str)] = &[
    ("param.undo", "undo"),
    ("param.redo", "redo"),
    ("param.reset", "reset"),
    ("workspace.open", "folder"),
    ("workspace.new", "plus"),
    ("workbench.keybindings", "keyboard"),
    ("workbench.about", "info"),
    ("workbench.fullscreen", "fullscreen"),
    ("theme.select", "theme"),
    ("mask.add.circle", "circle"),
    ("mask.member.delete", "delete"),
    ("mask.member.lock", "lock"),
    ("vault.flag.pick", "check"),
    ("vault.flag.reject", "close"),
    ("viewport.zoom.in", "plus"),
    ("viewport.zoom.out", "minus"),
    ("window.minimize", "window-minimize"),
    ("window.maximize", "window-maximize"),
    ("window.close", "window-close"),
];

/// A command's id, checked.
pub fn command_id(id: &str) -> Result<CommandId, PluginError> {
    CommandId::new(id).map_err(|e| PluginError::Failed(e.to_string()))
}

/// A command of Helios's registry as egui-pop declares it: its `when`,
/// and the tick and palette visibility Helios gives it.
pub fn declared(spec: &HeliosSpec, d: &Declarations) -> Result<CommandSpec, PluginError> {
    let mut s =
        CommandSpec::new(command_id(&spec.id)?, &spec.title, &spec.category).when(&spec.when);
    if let Some(c) = d.checked.get(&spec.id) {
        s = s.checked(c);
    }
    if let Some(icon) = d.icons.get(&spec.id) {
        s = s.icon(icon);
    }
    if d.hidden.contains(&spec.id) {
        s = s.hidden();
    }
    Ok(s)
}

/// Every command of Helios's registry, declared to egui-pop: its menus,
/// palette, keymap and traces see them, and the host runs them. A command
/// registered later is declared on its own (`Shell::register`).
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
            ctx.declare(declared(&spec, &d)?)?;
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

/// The lists of the system menu, filled by egui-pop's list providers.
pub const PROFILES_LIST: &str = "helios.profiles";
pub const RECENT_LIST: &str = "helios.recent";

/// The lists of Helios's menus, asked for each time a menu opens: the
/// saved profiles (the one in use ticked) and the recent workspaces in the
/// system menu, and the commands of a row's menu in the Masks panel under
/// their short labels, in the interface's language.
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
        ctx.menu_item(MenuItem::list(SYSTEM_PROFILES, "1", 0, PROFILES_LIST))?;
        ctx.menu_item(MenuItem::list(SYSTEM_RECENT, "1", 0, RECENT_LIST))?;
        let source = self.source.clone();
        let use_profile = command_id("profile.use")?;
        ctx.list_provider(PROFILES_LIST, move |_, cx| {
            let dir = source.borrow().profiles_dir.clone();
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
            names
                .iter()
                .map(|n| {
                    ListItem::new(n, use_profile.clone(), json!({ "name": n }))
                        .checked(cx.get("heliosProfile") == n)
                })
                .collect()
        });
        // A row's menu in the Masks panel: the commands act on the row
        // (its key comes with the menu's arguments), each under a verb.
        ctx.menu_item(MenuItem::list(
            crate::mask_tree::ROW_MENU,
            "1_member",
            0,
            crate::mask_tree::ROW_LIST,
        ))?;
        ctx.menu_item(MenuItem::list(
            crate::mask_tree::ROW_MENU_ADD,
            "1",
            0,
            crate::mask_tree::ROW_ADD_LIST,
        ))?;
        ctx.list_provider(crate::mask_tree::ROW_LIST, |app, _| {
            labelled(crate::mask_tree::row_entries(&app.egui()))
        });
        ctx.list_provider(crate::mask_tree::ROW_ADD_LIST, |app, _| {
            labelled(crate::mask_tree::add_entries(&app.egui()))
        });
        let open = command_id("workspace.open")?;
        ctx.list_provider(RECENT_LIST, move |_, _| {
            let recent = crate::shell::recent_workspaces();
            if recent.is_empty() {
                return vec![
                    ListItem::new("No recent workspace", open.clone(), json!(null)).disabled(),
                ];
            }
            recent
                .iter()
                .map(|p| {
                    ListItem::new(&p.display().to_string(), open.clone(), json!({ "path": p }))
                })
                .collect()
        });
        Ok(())
    }
}

/// Menu entries from labels and command ids, run without arguments of
/// their own (the menu's are given to them).
fn labelled<C: AsRef<str>>(entries: Vec<(String, C)>) -> Vec<ListItem> {
    entries
        .into_iter()
        .filter_map(|(label, id)| {
            let id = CommandId::new(id.as_ref()).ok()?;
            Some(ListItem::new(&label, id, serde_json::Value::Null))
        })
        .collect()
}

/// Undo and redo wait for a change in progress: while a gesture (a drag)
/// holds values, egui-pop's `edit.undo` and `edit.redo` steps are vetoed,
/// with the reason, rather than undoing under the user's hand.
pub struct History {
    pub values: Arc<Mutex<helios_values::Store>>,
    /// The catalogues a refusal is said through.
    pub i18n: egui_pop::i18n::I18n,
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
            let (values, i18n) = (self.values.clone(), self.i18n.clone());
            ctx.before(step, move |_, _| {
                let v = values.lock().unwrap_or_else(PoisonError::into_inner);
                match v.open_gesture() {
                    Some(g) => Before::Veto(helios_i18n::message(
                        &i18n,
                        &Message::new("gui-gesture-in-progress", "{ $gesture } is in progress")
                            .with("gesture", g.to_string()),
                    )),
                    None => Before::Go,
                }
            });
        }
        Ok(())
    }
}

/// The window's frame is egui-pop's (`FramePlugin`): its title bar, its
/// edges and the steps of its buttons. The start of the bar is Helios's:
/// the menu button, the logo and the application's name, in place of the
/// frame's own menu block. Its system menu (the bar's menu button, a
/// right click on the caption, `Alt+Space`) is Helios's, opened as a
/// popup where the frame asks: the `frame.system_menu` step is replaced.
/// Helios's status line is a block of egui-pop's status bar, beside its
/// jobs item; its global items (jobs, decisions, profile) are a block of
/// the bar's `CAPTION` zone, kept in full screen (`crate::caption`).
pub struct Frame {
    pub status: crate::jobs::Status,
}

/// Helios's status line in the status bar (keys pending, a recording,
/// the last note).
pub const BLOCK_STATUS: BlockId = BlockId::from_static("helios.frame.status");

/// The block at the start of the title bar: the menu button, the logo,
/// the application's name.
pub const BLOCK_LOGO: BlockId = BlockId::from_static("helios.frame.logo");

/// The logo, decoded once per context.
fn logo(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("helios.logo");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    let png = include_bytes!("../../../packaging/linux/icons/io.github.quazardous.Helios-64.png");
    let rgba = image::load_from_memory(png)
        .expect("the bundled logo decodes")
        .into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let picture = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    let t = ctx.load_texture("helios.logo", picture, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// Draws the start of the title bar.
fn caption_start(ui: &mut egui::Ui, app: &AppHandle) {
    frame::menu_button(ui, app);
    let side = crate::theme::Tokens::of(ui.ctx()).size.icon_toolbar;
    let logo = logo(ui.ctx());
    let image =
        ui.add(egui::Image::new((logo.id(), egui::vec2(side, side))).sense(egui::Sense::hover()));
    crate::name_rect(ui.ctx(), "helios.logo", image.rect);
    let name = app.name();
    if !name.is_empty() {
        ui.add(
            egui::Label::new(egui::RichText::new(name).strong())
                .selectable(false)
                .sense(egui::Sense::hover()),
        );
    }
}

impl Plugin for Frame {
    fn id(&self) -> PluginId {
        FRAME
    }

    fn title(&self) -> String {
        "Helios frame".into()
    }

    fn requires(&self) -> Vec<PluginId> {
        vec![FramePlugin::default().id()]
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        ctx.block(
            zones::CAPTION_START,
            Block::replace(frame::BLOCK_MENU, BLOCK_LOGO, caption_start),
        );
        ctx.block(
            zones::STATUS_BAR,
            Block::add(
                BLOCK_STATUS,
                100,
                crate::jobs::status_block(self.status.clone()),
            ),
        );
        ctx.block(
            zones::CAPTION,
            Block::add(
                crate::caption::BLOCK,
                100,
                crate::caption::block(self.status.clone()),
            ),
        );
        ctx.replace(STEP_SYSTEM_MENU, |app, args| {
            let x = args["x"].as_f64().unwrap_or(0.0) as f32;
            let y = args["y"].as_f64().unwrap_or(0.0) as f32;
            let open = popup_at(app, &SYSTEM, egui::pos2(x, y));
            Ok(json!({ "open": open, "menu": SYSTEM }))
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::COMMAND_ICONS;

    #[test]
    fn every_command_icon_is_a_built_in_name() {
        for (id, icon) in COMMAND_ICONS {
            assert!(
                egui_pop::core::icon::registry::is_known(icon),
                "{id}: `{icon}` is not in egui-pop's icon registry"
            );
        }
    }
}
