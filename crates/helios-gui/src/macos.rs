//! macOS's habits (docs/design/windows.md, section 7, D2 and D7): the
//! menus in the system's global menu bar, and Helios kept running with no
//! window, the Dock icon bringing the main window back.
//!
//! The menus' layout is [`crate::native_menu`]'s, tested on every
//! platform; this module only turns it into an `NSMenu` (with `muda`) and
//! runs what is chosen in it. The main window's pass goes on while the
//! window is hidden (winit keeps delivering its redraws), so the menu's
//! choices and the Dock's reopen are taken there.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use egui_pop::model::ids::CommandId;
use muda::accelerator::Accelerator;
use muda::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use serde_json::Value as Json;

use crate::native_menu::{self, Bar, Entry, Role, Standard};
use crate::shell::Shell;

/// The menu entries chosen since the last pass (muda's ids).
static CHOSEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The Dock icon was clicked: the main window comes back.
static REOPEN: AtomicBool = AtomicBool::new(false);

/// How often the bar is laid out again (its states follow the context).
const REFRESH_SECONDS: f64 = 0.25;

/// The global menu bar, kept in step with the declared menus.
#[derive(Default)]
pub struct MenuBar {
    installed: bool,
    built: Option<Bar>,
    /// Kept alive while it is the application's menu.
    menu: Option<Menu>,
    commands: Vec<(CommandId, Json)>,
    laid_out_at: Option<f64>,
}

impl MenuBar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs in the main window's pass: what was chosen in the menu, the
    /// Dock's reopen, then the bar laid out again when its menus changed.
    pub fn pass(&mut self, shell: &mut Shell, ctx: &egui::Context) {
        if !self.installed {
            self.installed = true;
            let waker = ctx.clone();
            muda::MenuEvent::set_event_handler(Some(move |e: muda::MenuEvent| {
                if let Ok(mut chosen) = CHOSEN.lock() {
                    chosen.push(e.id.0);
                }
                waker.request_repaint();
            }));
            dock::follow_reopen(ctx.clone());
        }
        if REOPEN.swap(false, Ordering::SeqCst) {
            shell.reopen();
            ctx.request_repaint();
        }
        let chosen = CHOSEN
            .lock()
            .map(|mut c| std::mem::take(&mut *c))
            .unwrap_or_default();
        for id in chosen {
            let Some((command, args)) = id
                .strip_prefix('c')
                .and_then(|n| n.parse::<usize>().ok())
                .and_then(|n| self.commands.get(n).cloned())
            else {
                continue;
            };
            let _ = shell.run(command.as_str(), args, helios_shell::commands::Origin::User);
        }
        let now = ctx.input(|i| i.time);
        if self
            .laid_out_at
            .is_some_and(|at| now - at < REFRESH_SECONDS)
        {
            return;
        }
        self.laid_out_at = Some(now);
        let handle = shell.app().handle().clone();
        let source = native_menu::App {
            app: &handle,
            ctx,
            name: "Helios",
        };
        let bar = native_menu::build(
            &source,
            &egui_pop::api::app::MENU_MAIN,
            &handle.when_context(),
        );
        if self.built.as_ref() == Some(&bar) {
            return;
        }
        self.install(&bar);
        self.built = Some(bar);
    }

    /// Makes `bar` the application's menu.
    fn install(&mut self, bar: &Bar) {
        self.commands = bar
            .commands()
            .into_iter()
            .map(|(c, a)| (c.clone(), a.clone()))
            .collect();
        let menu = Menu::new();
        let mut next = 0usize;
        let app = Submenu::new("Helios", true);
        fill(&app, &bar.app.entries, &mut next);
        let _ = menu.append(&app);
        for m in &bar.menus {
            let sub = Submenu::new(&m.title, true);
            fill(&sub, &m.entries, &mut next);
            let _ = menu.append(&sub);
            match m.role {
                Role::Window => sub.set_as_windows_menu_for_nsapp(),
                Role::Help => sub.set_as_help_menu_for_nsapp(),
                Role::Plain => {}
            }
        }
        menu.init_for_nsapp();
        self.menu = Some(menu);
    }
}

/// Appends the entries, numbering the commands as [`Bar::commands`]
/// lists them.
fn fill(sub: &Submenu, entries: &[Entry], next: &mut usize) {
    for e in entries {
        let item: Box<dyn IsMenuItem> = match e {
            Entry::Command {
                label,
                enabled,
                checked,
                accelerator,
                ..
            } => {
                let id = format!("c{next}");
                *next += 1;
                let keys = accelerator.and_then(|a| a.parse::<Accelerator>().ok());
                match checked {
                    Some(on) => Box::new(CheckMenuItem::with_id(id, label, *enabled, *on, keys)),
                    None => Box::new(MenuItem::with_id(id, label, *enabled, keys)),
                }
            }
            Entry::Submenu(m) => {
                let inner = Submenu::new(&m.title, true);
                fill(&inner, &m.entries, next);
                Box::new(inner)
            }
            Entry::Separator => Box::new(PredefinedMenuItem::separator()),
            Entry::Standard(s) => Box::new(match s {
                Standard::Services => PredefinedMenuItem::services(None),
                Standard::Hide => PredefinedMenuItem::hide(None),
                Standard::HideOthers => PredefinedMenuItem::hide_others(None),
                Standard::ShowAll => PredefinedMenuItem::show_all(None),
                Standard::Minimize => PredefinedMenuItem::minimize(None),
                Standard::Zoom => PredefinedMenuItem::maximize(None),
                Standard::BringAllToFront => PredefinedMenuItem::bring_all_to_front(None),
            }),
        };
        let _ = sub.append(item.as_ref());
    }
}

/// The Dock icon clicked while Helios runs: winit's application delegate
/// does not answer `applicationShouldHandleReopen:hasVisibleWindows:`, so
/// the method is added to its class.
mod dock {
    use std::ffi::CStr;
    use std::sync::OnceLock;
    use std::sync::atomic::Ordering;

    use objc2::MainThreadMarker;
    use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
    use objc2::sel;
    use objc2_app_kit::NSApplication;

    static WAKER: OnceLock<egui::Context> = OnceLock::new();

    extern "C-unwind" fn should_handle_reopen(
        _this: *mut AnyObject,
        _cmd: Sel,
        _app: *mut AnyObject,
        _has_visible_windows: Bool,
    ) -> Bool {
        super::REOPEN.store(true, Ordering::SeqCst);
        if let Some(ctx) = WAKER.get() {
            ctx.request_repaint();
        }
        Bool::YES
    }

    /// Adds the reopen method to the application delegate's class, once.
    pub fn follow_reopen(ctx: egui::Context) {
        if WAKER.set(ctx).is_err() {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        let Some(delegate) = app.delegate() else {
            return;
        };
        let object: &AnyObject = (*delegate).as_ref();
        let class: &AnyClass = object.class();
        // BOOL is `B` on Apple silicon, `c` on Intel.
        let types: &CStr = if cfg!(target_arch = "aarch64") {
            c"B@:@B"
        } else {
            c"c@:@c"
        };
        // SAFETY: the function has the method's signature (self, _cmd,
        // the application, a BOOL; returns a BOOL), and the types string
        // says so; the class is registered and the method is added once.
        unsafe {
            let imp: Imp = std::mem::transmute::<
                extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, Bool) -> Bool,
                Imp,
            >(should_handle_reopen);
            objc2::ffi::class_addMethod(
                class as *const AnyClass as *mut AnyClass,
                sel!(applicationShouldHandleReopen:hasVisibleWindows:),
                imp,
                types.as_ptr(),
            );
            // AppKit reads what the delegate answers when it is set.
            app.setDelegate(Some(&delegate));
        }
    }
}
