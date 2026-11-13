//! macOS's habits (docs/design/windows.md, section 7, D2 and D7): the
//! menus in the system's global menu bar, and Helios kept running with no
//! window, the Dock icon bringing the main window back.
//!
//! The menus' layout is [`crate::native_menu`]'s, tested on every
//! platform; this module only turns it into an `NSMenu` (with `muda`) and
//! runs what is chosen in it. The main window's pass goes on while the
//! window is hidden (winit keeps delivering its redraws), so the menu's
//! choices and the Dock's reopen are taken there.
//!
//! The bar follows the window in front: every window's pass notes when
//! it is the focused one and wakes the main window's pass, which lays the
//! bar out again in that window's context and runs what is chosen there.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use egui_pop::model::ids::CommandId;
use egui_pop::model::when::WhenContext;
use muda::accelerator::Accelerator;
use muda::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use serde_json::Value as Json;

use crate::native_menu::{self, Bar, Entry, Role, Standard};
use crate::shell::Shell;

/// The menu entries chosen since the last pass (muda's ids).
static CHOSEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The Dock icon was clicked: the main window comes back.
static REOPEN: AtomicBool = AtomicBool::new(false);
/// The window in front (the focused viewport), as its own pass saw it.
static FRONT: Mutex<Option<egui::ViewportId>> = Mutex::new(None);

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
    /// The context the bar was laid out in: the one its choices run in.
    context: WhenContext,
    laid_out_at: Option<f64>,
    /// The window in front the bar was laid out for.
    laid_out_for: Option<egui::ViewportId>,
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
            follow_front(ctx);
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
            let _ = shell.run_in(
                command.as_str(),
                args,
                helios_shell::commands::Origin::User,
                &self.context,
            );
        }
        let front = FRONT
            .lock()
            .ok()
            .and_then(|f| *f)
            .unwrap_or(egui::ViewportId::ROOT);
        let now = ctx.input(|i| i.time);
        if self.laid_out_for == Some(front)
            && self
                .laid_out_at
                .is_some_and(|at| now - at < REFRESH_SECONDS)
        {
            return;
        }
        self.laid_out_at = Some(now);
        self.laid_out_for = Some(front);
        let handle = shell.app().handle().clone();
        // The window in front's group: an application window, or the
        // main window's (its own and its detached panels'); a window
        // closed since falls back to the main window's.
        let group = handle.window_group(front);
        let window = handle.window_of_group(group);
        self.context = native_menu::window_context(
            &handle.when_context(),
            window.as_str(),
            egui_pop::core::overlay::modal_open_in(ctx, group),
        );
        let source = native_menu::App {
            app: &handle,
            ctx,
            name: "Helios",
        };
        let bar = native_menu::build(&source, &egui_pop::api::app::MENU_MAIN, &self.context);
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
                let keys = accelerator
                    .as_deref()
                    .and_then(|a| a.parse::<Accelerator>().ok());
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

/// Notes the window in front from every window's pass, and wakes the
/// main window's pass when it changed: the bar follows it. A window's
/// pass runs when it gains the focus.
fn follow_front(ctx: &egui::Context) {
    ctx.on_begin_pass(
        "helios.native_menu.front",
        std::sync::Arc::new(|ui: &mut egui::Ui| {
            let ctx = ui.ctx();
            if ctx.input(|i| i.viewport().focused) != Some(true) {
                return;
            }
            let here = ctx.viewport_id();
            let Ok(mut front) = FRONT.lock() else {
                return;
            };
            if *front != Some(here) {
                *front = Some(here);
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        }),
    );
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
    #[allow(unsafe_code)] // AppKit: one method added to the delegate class through the Objective-C runtime
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
