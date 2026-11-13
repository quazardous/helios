//! The application's menus as a native menu bar sees them
//! (docs/design/windows.md, section 7, D7).
//!
//! On macOS the menus go to the system's global menu bar instead of being
//! drawn in the window. This module is the platform-neutral half: it reads
//! the menus egui-pop declares (`app.main` and its submenus, their `when`
//! and `checked` clauses, their lists) and lays them out as a native bar
//! does, with the application menu first (About, Settings…, Services,
//! Hide, Quit) and the standard entries of the Window menu. The macOS half
//! (`crate::macos`) turns a [`Bar`] into an `NSMenu` and runs the command
//! an entry names; it is rebuilt only when the [`Bar`] changes.
//!
//! The layout is plain data, so it is tested on every platform.

use egui_pop::api::AppHandle;
use egui_pop::api::menus::ListItem;
use egui_pop::model::ids::{CommandId, MenuId};
use egui_pop::model::menus::{Item, MenuItem, Shown, show_items};
use egui_pop::model::when::WhenContext;
use serde_json::Value as Json;

/// How deep submenus may nest before the rest is cut (a menu naming
/// itself as a submenu).
const MAX_DEPTH: u32 = 8;

/// egui-pop's About (`about::AboutPlugin`).
pub const ABOUT: CommandId = CommandId::from_static("about");
/// egui-pop's Settings window.
pub const SETTINGS: CommandId = egui_pop::api::settings_window::CMD_SETTINGS;
/// Helios's Quit, from any window (`workbench.quit`).
pub const QUIT: CommandId = CommandId::from_static("workbench.quit");
/// egui-pop's own Quit (File > Quit): the application menu's Quit takes
/// its place.
const POP_QUIT: CommandId = CommandId::from_static("app.quit");
/// Brings the main window back, or forward (Window > Main window): the
/// File menu's New Window on macOS.
pub const MAIN_WINDOW: CommandId = CommandId::from_static("helios.window.main");

/// An entry the system draws and runs itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standard {
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Zoom,
    BringAllToFront,
}

/// One entry of a native menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A command of the application, with its arguments.
    Command {
        command: CommandId,
        args: Json,
        label: String,
        enabled: bool,
        /// `Some` for a toggle or a radio item: whether it is ticked.
        checked: Option<bool>,
        /// The key the native menu answers itself (`CmdOrCtrl+Q`): only
        /// for the entries a platform's habits fix; the other keys stay
        /// egui-pop's keymap's, so a text field keeps its own.
        accelerator: Option<&'static str>,
    },
    Submenu(Menu),
    Separator,
    Standard(Standard),
}

/// What a menu is for the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Plain,
    /// The system lists the open windows in it.
    Window,
    /// The system adds its search field to it.
    Help,
}

/// A native menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub title: String,
    pub role: Role,
    pub entries: Vec<Entry>,
}

/// The whole bar: the application menu, then the application's menus.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub app: Menu,
    pub menus: Vec<Menu>,
}

impl Bar {
    /// Every command entry, depth first: what the macOS half numbers.
    pub fn commands(&self) -> Vec<(&CommandId, &Json)> {
        fn walk<'a>(entries: &'a [Entry], out: &mut Vec<(&'a CommandId, &'a Json)>) {
            for e in entries {
                match e {
                    Entry::Command { command, args, .. } => out.push((command, args)),
                    Entry::Submenu(m) => walk(&m.entries, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.app.entries, &mut out);
        for m in &self.menus {
            walk(&m.entries, &mut out);
        }
        out
    }
}

/// What the layout reads from the application.
pub trait Source {
    /// A menu's items, after the hooks.
    fn items(&self, menu: &MenuId) -> Vec<MenuItem>;
    /// A command's title as a menu shows it; `None` when not declared.
    fn title(&self, command: &CommandId) -> Option<String>;
    /// Whether a command applies (its own `when`).
    fn applies(&self, command: &CommandId, cx: &WhenContext) -> bool;
    /// A submenu's label in the interface's language.
    fn submenu_label(&self, menu: &MenuId, english: &str) -> String;
    /// A list's entries.
    fn list(&self, name: &str, cx: &WhenContext) -> Vec<ListItem>;
    /// A text of the application menu in the interface's language.
    fn word(&self, key: Word) -> String;
}

/// The application menu's own words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Word {
    About,
    Settings,
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    NewWindow,
    Minimize,
    Zoom,
    BringAllToFront,
}

impl Word {
    /// The English text (`name`: the application's).
    pub fn english(self, name: &str) -> String {
        match self {
            Word::About => format!("About {name}"),
            Word::Settings => "Settings…".into(),
            Word::Services => "Services".into(),
            Word::Hide => format!("Hide {name}"),
            Word::HideOthers => "Hide Others".into(),
            Word::ShowAll => "Show All".into(),
            Word::Quit => format!("Quit {name}"),
            Word::NewWindow => "New Window".into(),
            Word::Minimize => "Minimize".into(),
            Word::Zoom => "Zoom".into(),
            Word::BringAllToFront => "Bring All to Front".into(),
        }
    }
}

/// The commands the application menu holds: left out of the others.
fn in_app_menu(id: &CommandId) -> bool {
    *id == ABOUT || *id == SETTINGS || *id == QUIT || *id == POP_QUIT
}

/// Lays out the bar from `root` (egui-pop's `app.main`), in context `cx`.
pub fn build(source: &impl Source, root: &MenuId, cx: &WhenContext) -> Bar {
    let command = |id: CommandId, word: Word, accelerator| Entry::Command {
        label: source.word(word),
        enabled: source.applies(&id, cx),
        command: id,
        args: Json::Null,
        checked: None,
        accelerator,
    };
    let app = Menu {
        title: String::new(),
        role: Role::Plain,
        entries: vec![
            command(ABOUT, Word::About, None),
            Entry::Separator,
            command(SETTINGS, Word::Settings, Some("CmdOrCtrl+Comma")),
            Entry::Separator,
            Entry::Standard(Standard::Services),
            Entry::Separator,
            Entry::Standard(Standard::Hide),
            Entry::Standard(Standard::HideOthers),
            Entry::Standard(Standard::ShowAll),
            Entry::Separator,
            command(QUIT, Word::Quit, Some("CmdOrCtrl+KeyQ")),
        ],
    };
    let mut menus = Vec::new();
    for shown in show_items(&source.items(root), cx) {
        let Shown::Item {
            item: Item::Submenu { menu, label },
            enabled: true,
            ..
        } = shown
        else {
            continue;
        };
        let mut entries = entries(source, &menu, cx, 1);
        let role = if menu == egui_pop::api::windows::MENU_WINDOW {
            Role::Window
        } else if menu == egui_pop::api::app::MENU_HELP {
            Role::Help
        } else {
            Role::Plain
        };
        if menu == egui_pop::api::app::MENU_FILE {
            entries.insert(0, command(MAIN_WINDOW, Word::NewWindow, None));
            entries.insert(1, Entry::Separator);
        }
        if role == Role::Window {
            let mut standard = vec![
                Entry::Standard(Standard::Minimize),
                Entry::Standard(Standard::Zoom),
                Entry::Separator,
            ];
            standard.append(&mut entries);
            standard.push(Entry::Separator);
            standard.push(Entry::Standard(Standard::BringAllToFront));
            entries = standard;
        }
        let entries = tidy(entries);
        if entries.is_empty() {
            continue;
        }
        menus.push(Menu {
            title: source.submenu_label(&menu, &label),
            role,
            entries,
        });
    }
    Bar { app, menus }
}

/// A menu's entries, its groups separated.
fn entries(source: &impl Source, menu: &MenuId, cx: &WhenContext, depth: u32) -> Vec<Entry> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }
    let mut out = Vec::new();
    for shown in show_items(&source.items(menu), cx) {
        match shown {
            Shown::Separator => out.push(Entry::Separator),
            Shown::Item {
                item,
                enabled,
                checked,
                checkable,
                radio,
            } => match item {
                Item::Command { id, args, label } => {
                    if in_app_menu(&id) {
                        continue;
                    }
                    let Some(title) = source.title(&id) else {
                        continue;
                    };
                    out.push(Entry::Command {
                        enabled: enabled && source.applies(&id, cx),
                        label: label.unwrap_or(title),
                        command: id,
                        args,
                        checked: (checkable || radio).then_some(checked),
                        accelerator: None,
                    });
                }
                Item::Submenu { menu: sub, label } => {
                    let inner = tidy(entries(source, &sub, cx, depth + 1));
                    if !inner.is_empty() {
                        out.push(Entry::Submenu(Menu {
                            title: source.submenu_label(&sub, &label),
                            role: Role::Plain,
                            entries: if enabled {
                                inner
                            } else {
                                inner.into_iter().map(disabled).collect()
                            },
                        }));
                    }
                }
                Item::List { name } => {
                    for e in source.list(&name, cx) {
                        out.push(Entry::Command {
                            label: e.label,
                            enabled: enabled && e.enabled,
                            checked: e.checked.then_some(true),
                            command: e.command,
                            args: e.args,
                            accelerator: None,
                        });
                    }
                }
            },
        }
    }
    out
}

/// An entry greyed (a submenu whose item does not apply).
fn disabled(e: Entry) -> Entry {
    match e {
        Entry::Command {
            command,
            args,
            label,
            checked,
            accelerator,
            ..
        } => Entry::Command {
            command,
            args,
            label,
            enabled: false,
            checked,
            accelerator,
        },
        Entry::Submenu(mut m) => {
            m.entries = m.entries.into_iter().map(disabled).collect();
            Entry::Submenu(m)
        }
        other => other,
    }
}

/// No separator first, last or twice in a row (the entries between them
/// left out).
fn tidy(entries: Vec<Entry>) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::with_capacity(entries.len());
    for e in entries {
        if e == Entry::Separator && out.last().is_none_or(|l| *l == Entry::Separator) {
            continue;
        }
        out.push(e);
    }
    if out.last() == Some(&Entry::Separator) {
        out.pop();
    }
    out
}

/// The application, read through its handle.
pub struct App<'a> {
    pub app: &'a AppHandle,
    pub ctx: &'a egui::Context,
    /// The application's name, in the application menu's words.
    pub name: &'a str,
}

impl Source for App<'_> {
    fn items(&self, menu: &MenuId) -> Vec<MenuItem> {
        self.app.menu(menu)
    }

    fn title(&self, command: &CommandId) -> Option<String> {
        self.app
            .command(command)
            .map(|s| self.app.command_menu_title(&s))
    }

    fn applies(&self, command: &CommandId, cx: &WhenContext) -> bool {
        self.app.applies(command, cx)
    }

    fn submenu_label(&self, menu: &MenuId, english: &str) -> String {
        egui_pop::core::strings::menu_label(self.ctx, menu.as_str(), english)
    }

    fn list(&self, name: &str, cx: &WhenContext) -> Vec<ListItem> {
        self.app.list_items(name, cx)
    }

    fn word(&self, key: Word) -> String {
        key.english(self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_pop::api::app::{MENU_FILE, MENU_MAIN};
    use egui_pop::api::windows::MENU_WINDOW;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Fake {
        menus: HashMap<MenuId, Vec<MenuItem>>,
        lists: HashMap<String, Vec<ListItem>>,
    }

    impl Source for Fake {
        fn items(&self, menu: &MenuId) -> Vec<MenuItem> {
            self.menus.get(menu).cloned().unwrap_or_default()
        }
        fn title(&self, command: &CommandId) -> Option<String> {
            (command.as_str() != "missing").then(|| format!("T:{command}"))
        }
        fn applies(&self, command: &CommandId, _: &WhenContext) -> bool {
            command.as_str() != "never"
        }
        fn submenu_label(&self, _: &MenuId, english: &str) -> String {
            english.to_uppercase()
        }
        fn list(&self, name: &str, _: &WhenContext) -> Vec<ListItem> {
            self.lists.get(name).cloned().unwrap_or_default()
        }
        fn word(&self, key: Word) -> String {
            key.english("Helios")
        }
    }

    fn id(s: &str) -> CommandId {
        CommandId::new(s).unwrap()
    }

    fn menu(s: &str) -> MenuId {
        MenuId::new(s).unwrap()
    }

    fn labels(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                Entry::Command { label, .. } => label.clone(),
                Entry::Submenu(m) => format!("{}>", m.title),
                Entry::Separator => "-".into(),
                Entry::Standard(s) => format!("{s:?}"),
            })
            .collect()
    }

    fn sample() -> Fake {
        let edit = menu("app.menu.edit");
        let recent = menu("helios.recent");
        let mut f = Fake::default();
        f.menus.insert(
            MENU_MAIN,
            vec![
                MenuItem::submenu(MENU_MAIN, "1", 0, MENU_FILE, "File"),
                MenuItem::submenu(MENU_MAIN, "1", 1, edit.clone(), "Edit"),
                MenuItem::submenu(MENU_MAIN, "1", 2, MENU_WINDOW, "Window"),
                // Hidden: its `when` does not hold.
                MenuItem::submenu(MENU_MAIN, "1", 3, menu("hidden"), "Hidden")
                    .when("nothing == 'set'", false),
            ],
        );
        f.menus.insert(
            MENU_FILE,
            vec![
                MenuItem::command(MENU_FILE, "1_import", 0, id("import")),
                MenuItem::submenu(MENU_FILE, "1_import", 1, recent.clone(), "Recent"),
                MenuItem::command(MENU_FILE, "8_settings", 0, SETTINGS),
                MenuItem::command(MENU_FILE, "9_quit", 0, POP_QUIT),
                MenuItem::command(MENU_FILE, "9_quit", 1, QUIT),
            ],
        );
        f.menus.insert(
            recent.clone(),
            vec![MenuItem::list(recent, "1", 0, "recent")],
        );
        f.lists.insert(
            "recent".into(),
            vec![
                ListItem::new("a.jpg", id("open"), serde_json::json!({ "path": "a.jpg" }))
                    .checked(true),
            ],
        );
        f.menus.insert(
            edit.clone(),
            vec![
                MenuItem::command(edit.clone(), "1", 0, id("undo")),
                MenuItem::command(edit.clone(), "1", 1, id("never")),
                MenuItem::command(edit.clone(), "2", 0, id("missing")),
                MenuItem::command(edit.clone(), "3", 0, id("toggle")).checked("on == 'yes'"),
            ],
        );
        f.menus.insert(
            MENU_WINDOW,
            vec![MenuItem::command(MENU_WINDOW, "1", 0, MAIN_WINDOW)],
        );
        f
    }

    #[test]
    fn the_application_menu_comes_first_with_its_keys() {
        let bar = build(&sample(), &MENU_MAIN, &WhenContext::new());
        assert_eq!(
            labels(&bar.app.entries),
            [
                "About Helios",
                "-",
                "Settings…",
                "-",
                "Services",
                "-",
                "Hide",
                "HideOthers",
                "ShowAll",
                "-",
                "Quit Helios"
            ]
        );
        let quit = bar.app.entries.last().unwrap();
        assert!(matches!(
            quit,
            Entry::Command { command, accelerator: Some("CmdOrCtrl+KeyQ"), .. } if *command == QUIT
        ));
    }

    #[test]
    fn menus_follow_the_declared_bar_without_what_the_app_menu_holds() {
        let bar = build(&sample(), &MENU_MAIN, &WhenContext::new());
        let titles: Vec<_> = bar.menus.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, ["FILE", "EDIT", "WINDOW"]);
        // New Window first; Settings and both Quits gone to the app menu;
        // the separators they left behind tidied.
        assert_eq!(
            labels(&bar.menus[0].entries),
            ["New Window", "-", "T:import", "RECENT>"]
        );
        let Entry::Submenu(recent) = &bar.menus[0].entries[3] else {
            panic!("a submenu")
        };
        assert!(matches!(
            &recent.entries[0],
            Entry::Command { label, checked: Some(true), enabled: true, .. } if label == "a.jpg"
        ));
    }

    #[test]
    fn states_come_from_the_clauses() {
        let mut cx = WhenContext::new();
        let edit = |cx: &WhenContext| build(&sample(), &MENU_MAIN, cx).menus[1].clone();
        let m = edit(&cx);
        // An undeclared command is left out, its group with it.
        assert_eq!(labels(&m.entries), ["T:undo", "T:never", "-", "T:toggle"]);
        assert!(matches!(
            &m.entries[1],
            Entry::Command { enabled: false, .. }
        ));
        assert!(matches!(
            &m.entries[3],
            Entry::Command {
                checked: Some(false),
                ..
            }
        ));
        cx.set("on", "yes");
        assert!(matches!(
            &edit(&cx).entries[3],
            Entry::Command {
                checked: Some(true),
                ..
            }
        ));
    }

    #[test]
    fn the_window_menu_has_the_system_entries() {
        let bar = build(&sample(), &MENU_MAIN, &WhenContext::new());
        let window = &bar.menus[2];
        assert_eq!(window.role, Role::Window);
        assert_eq!(
            labels(&window.entries),
            [
                "Minimize",
                "Zoom",
                "-",
                "T:helios.window.main",
                "-",
                "BringAllToFront"
            ]
        );
    }

    #[test]
    fn commands_are_numbered_depth_first() {
        let bar = build(&sample(), &MENU_MAIN, &WhenContext::new());
        let ids: Vec<_> = bar.commands().iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(
            ids,
            [
                "about",
                "app.settings",
                "workbench.quit",
                "helios.window.main",
                "import",
                "open",
                "undo",
                "never",
                "toggle",
                "helios.window.main"
            ]
        );
    }

    #[test]
    fn a_menu_naming_itself_stops() {
        let mut f = Fake::default();
        let a = menu("a");
        f.menus.insert(
            MENU_MAIN,
            vec![MenuItem::submenu(MENU_MAIN, "1", 0, a.clone(), "A")],
        );
        f.menus.insert(
            a.clone(),
            vec![
                MenuItem::command(a.clone(), "1", 0, id("x")),
                MenuItem::submenu(a.clone(), "1", 1, a.clone(), "A"),
            ],
        );
        let bar = build(&f, &MENU_MAIN, &WhenContext::new());
        assert_eq!(bar.menus.len(), 1);
    }
}
