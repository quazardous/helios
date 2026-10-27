//! Helios behind egui-pop's host contract (`egui_pop::api::host`).
//!
//! egui-pop owns what is UI: command declarations, menus, the palette,
//! keys, focus, overlays. Running commands, keeping the journal and waking
//! the window belong to Helios, and [`HeliosHost`] plugs them in:
//!
//! - **commands** egui-pop hands over run on Helios's control surface,
//!   the command [`Registry`] that agents, scripts and the node reach as
//!   well; egui-pop's own commands (the palette, undo, the settings
//!   window) keep their in-process handlers. A handler that only queues
//!   work for the next frame takes a [`Resolver`] from [`Later`]: egui-pop
//!   then shows the command as running until the work is done, and never
//!   waits for it;
//! - **the journal** is Helios's [`Bus`]: egui-pop's events (a command
//!   dispatched, done or failed, a dialog answered, a toast shown) go on
//!   the bus beside the engine's, under `gui.*` kinds, with their sequence
//!   number, source and cause, and egui-pop reads the bus back (`since`,
//!   `last_seq`): the host keeps no journal of egui-pop's own;
//! - **the store** holds the interface's own settings (focus mode, window
//!   place, the user's keys); Helios's values (node parameters, the
//!   viewport) stay in `helios_values`;
//! - **undo and redo**: egui-pop's Edit > Undo and Redo (`Ctrl+Z`...) ask
//!   the host, which answers from Helios's value store
//!   ([`HeliosHost::with_values`]): its steps, their labels ("Undo drag
//!   exposure"), and an undo or redo as a step of its own, which the
//!   vault saves as a version (`persist`). The history groups egui-pop
//!   opens around a paste or a cut are groups of that store too, so a
//!   paste is one of its steps. The interface settings keep their own
//!   undo, in the settings window;
//! - **the waker**: a thread follows the bus and asks for a frame when an
//!   event comes from elsewhere (a render done, a node's status);
//! - **the layout's document**: each workspace keeps its own
//!   (`workspaces/<workspace>/layout`, beside the interface's settings),
//!   named by the [`Workspace`] the shell switches; egui-pop loads and
//!   saves it.

use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::host::{self, Dispatch, Event, Host, Reply, Resolver, Seq, Waker};
use egui_pop::core::binding::SharedStore;
use egui_pop::model::commands::Origin as PopOrigin;
use egui_pop::model::values::GestureId as PopGestureId;
use egui_pop::model::when::WhenContext;
use helios_core::Bus;
use helios_shell::commands::{Origin, Registry};
use helios_shell::when::Context;
use serde_json::{Value as Json, json};

/// Where a handler that finishes on a later frame takes its resolver: set
/// while the host runs a command, empty otherwise.
#[derive(Clone, Default)]
pub struct Later(Arc<Mutex<Option<Resolver>>>);

impl Later {
    /// The resolver of the command being run, if it has not been taken:
    /// whoever takes it gives the result later (the host replies
    /// `Reply::Later`).
    pub fn take(&self) -> Option<Resolver> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    fn set(&self, r: Option<Resolver>) -> Option<Resolver> {
        std::mem::replace(
            &mut *self.0.lock().unwrap_or_else(PoisonError::into_inner),
            r,
        )
    }
}

/// The workspace whose layout egui-pop loads and saves: its key (see
/// [`workspace_key`]), none for the interface's one layout.
#[derive(Clone, Debug, Default)]
pub struct Workspace(Arc<Mutex<Option<String>>>);

impl Workspace {
    /// The workspace's key, if one is open.
    pub fn get(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Opens a workspace (its key), or none.
    pub fn set(&self, key: Option<String>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = key;
    }

    /// The document the layout is kept in.
    pub fn layout_document(&self) -> String {
        match self.get() {
            Some(key) => format!("workspaces/{key}/layout"),
            None => host::LAYOUT_DOCUMENT.into(),
        }
    }
}

/// A workspace directory's key: its path, every character but letters,
/// digits, `.` and `-` turned to `_`, so it names one folder.
pub fn workspace_key(path: &std::path::Path) -> String {
    helios_template::workspace::key(path)
}

/// The bus kinds egui-pop's events go under, by egui-pop's name.
const KINDS: &[(&str, &str)] = &[
    ("command.dispatched", "gui.command.dispatched"),
    ("command.done", "gui.command.done"),
    ("command.failed", "gui.command.failed"),
    ("dialog.answered", "gui.dialog.answered"),
    ("toast.shown", "gui.toast.shown"),
    ("edit.undone", "gui.edit.undone"),
    ("edit.redone", "gui.edit.redone"),
    ("edit.undo_failed", "gui.edit.undo_failed"),
    ("edit.redo_failed", "gui.edit.redo_failed"),
];

/// The kind of an egui-pop event whose name is not in [`KINDS`]; its
/// name goes in the payload.
const OTHER: &str = "gui.event";

/// Helios as egui-pop's host.
pub struct HeliosHost {
    registry: Arc<Mutex<Registry>>,
    bus: Arc<Bus>,
    later: Later,
    store: SharedStore,
    waker: Arc<Mutex<Waker>>,
    /// Helios's values, whose steps undo and redo walk.
    values: Option<Arc<Mutex<helios_values::Store>>>,
    workspace: Workspace,
}

impl HeliosHost {
    /// A host running commands on `registry` and journaling on `bus`;
    /// starts the thread that wakes the window on the bus's events.
    pub fn new(registry: Arc<Mutex<Registry>>, bus: Arc<Bus>, store: SharedStore) -> Self {
        let waker: Arc<Mutex<Waker>> = Arc::default();
        let events = bus.subscribe();
        let wake = waker.clone();
        std::thread::Builder::new()
            .name("helios-gui-waker".into())
            .spawn(move || {
                // Ends when the bus is gone. The GUI's own events are
                // published on its thread, which is awake already. A burst
                // (a node's progress, relayed) asks for one frame.
                while let Ok(first) = events.recv() {
                    let mut from_elsewhere = !first.kind.starts_with("gui.");
                    for e in events.try_iter() {
                        from_elsewhere |= !e.kind.starts_with("gui.");
                    }
                    if from_elsewhere {
                        let w = wake.lock().unwrap_or_else(PoisonError::into_inner).clone();
                        w.wake();
                    }
                }
            })
            .expect("a thread for the GUI's waker");
        Self {
            registry,
            bus,
            later: Later::default(),
            store,
            waker,
            values: None,
            workspace: Workspace::default(),
        }
    }

    /// Undo and redo walk the steps of these values.
    pub fn with_values(mut self, values: Arc<Mutex<helios_values::Store>>) -> Self {
        self.values = Some(values);
        self
    }

    fn values(&self) -> Option<std::sync::MutexGuard<'_, helios_values::Store>> {
        self.values
            .as_ref()
            .map(|v| v.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The workspace whose layout egui-pop keeps (shared: the shell
    /// switches it).
    pub fn workspace(&self) -> Workspace {
        self.workspace.clone()
    }

    /// Where queuing handlers take their resolver.
    pub fn later(&self) -> Later {
        self.later.clone()
    }
}

/// egui-pop's origin as Helios's.
pub fn to_helios(origin: &PopOrigin) -> Origin {
    match origin {
        PopOrigin::User => Origin::User,
        PopOrigin::Agent(n) => Origin::Agent(n.clone()),
        PopOrigin::Script(n) => Origin::Script(n.clone()),
        PopOrigin::Device(_) => Origin::Midi,
        PopOrigin::Remote(n) => Origin::Remote(n.clone()),
    }
}

/// Helios's origin as egui-pop's.
pub fn to_pop(origin: &Origin) -> PopOrigin {
    match origin {
        Origin::User => PopOrigin::User,
        Origin::Agent(n) => PopOrigin::Agent(n.clone()),
        Origin::Script(n) => PopOrigin::Script(n.clone()),
        Origin::Midi => PopOrigin::Device("midi".into()),
        Origin::Remote(n) => PopOrigin::Remote(n.clone()),
    }
}

/// egui-pop's context as Helios's `when` context (depths dropped: Helios
/// clauses test values only).
pub fn context_of(cx: &WhenContext) -> Context {
    let mut out = Context::new();
    for (k, v, _) in cx.iter() {
        out.set(k, v);
    }
    out
}

impl Host for HeliosHost {
    fn dispatch(&mut self, d: &Dispatch<'_>) -> Reply {
        if d.has_local() {
            return Reply::Done(d.run_local().unwrap_or_else(|| Err("no handler".into())));
        }
        let (pending, resolver) = host::pending();
        self.later.set(Some(resolver));
        let result = self
            .registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .run(
                d.command.as_str(),
                d.args.clone(),
                to_helios(d.origin),
                &context_of(d.context),
            );
        let taken = self.later.set(None).is_none();
        match result {
            Ok(_) if taken => Reply::Later(pending),
            r => Reply::Done(r),
        }
    }

    fn publish(&mut self, source: &str, cause: Option<Seq>, name: &str, data: Json) -> Option<Seq> {
        Some(match KINDS.iter().find(|(n, _)| *n == name) {
            Some((_, kind)) => self.bus.publish_value(source, cause, kind, data),
            None => {
                self.bus
                    .publish_value(source, cause, OTHER, json!({ "name": name, "data": data }))
            }
        })
    }

    fn last_seq(&self) -> Option<Seq> {
        Some(self.bus.last_seq())
    }

    fn since(&self, seq: Seq) -> Option<Vec<Event>> {
        let events = self
            .bus
            .since(seq)
            .into_iter()
            .map(|e| {
                let (name, data) = if e.kind == OTHER {
                    (
                        e.payload["name"].as_str().unwrap_or(OTHER).to_string(),
                        e.payload["data"].clone(),
                    )
                } else {
                    (
                        e.kind.strip_prefix("gui.").unwrap_or(e.kind).to_string(),
                        e.payload,
                    )
                };
                Event {
                    seq: e.seq,
                    source: e.source,
                    cause: e.cause,
                    name,
                    data,
                }
            })
            .collect();
        Some(events)
    }

    fn store(&self) -> SharedStore {
        self.store.clone()
    }

    fn set_waker(&mut self, waker: Waker) {
        *self.waker.lock().unwrap_or_else(PoisonError::into_inner) = waker;
    }

    fn layout_document(&self) -> Option<String> {
        Some(self.workspace.layout_document())
    }

    fn can_undo(&self) -> bool {
        self.values().is_some_and(|v| !v.undo_steps().is_empty())
    }

    fn can_redo(&self) -> bool {
        self.values().is_some_and(|v| !v.redo_steps().is_empty())
    }

    fn undo_label(&self) -> Option<String> {
        self.values()?.undo_steps().last().map(|s| s.label.clone())
    }

    fn redo_label(&self) -> Option<String> {
        self.values()?.redo_steps().last().map(|s| s.label.clone())
    }

    fn undo(&mut self, origin: &PopOrigin) -> Result<Option<String>, String> {
        match self.values() {
            Some(mut v) => v.undo(&to_helios(origin)).map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    fn redo(&mut self, origin: &PopOrigin) -> Result<Option<String>, String> {
        match self.values() {
            Some(mut v) => v.redo(&to_helios(origin)).map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    // A paste or a cut is one step of Helios's values, the steps undo
    // walks: egui-pop's group is a group of that store, and a target's
    // writes carry it (`helios_values::GestureId` of the same number).
    fn begin_group(&mut self, origin: &PopOrigin, label: &str) -> Option<PopGestureId> {
        let g = self.values()?.begin_group(to_helios(origin), label);
        Some(PopGestureId(g.0))
    }

    fn end_group(&mut self, group: PopGestureId, keep: bool) {
        if let Some(mut v) = self.values() {
            let g = helios_values::GestureId(group.0);
            if keep {
                v.end(g);
            } else {
                v.cancel(g);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use egui_pop::api::{App, Ctx, Plugin, PluginError};
    use egui_pop::model::commands::CommandSpec;
    use egui_pop::model::ids::{CommandId, PluginId};
    use helios_shell::commands::CommandSpec as HeliosSpec;

    use super::*;

    struct Declares;

    impl Plugin for Declares {
        fn id(&self) -> PluginId {
            PluginId::from_static("test.declares")
        }

        fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
            for id in ["test.now", "test.later"] {
                ctx.declare(CommandSpec::new(CommandId::new(id).unwrap(), id, "Test"))?;
            }
            Ok(())
        }
    }

    fn app() -> (App, Arc<Bus>, mpsc::Receiver<Resolver>) {
        let registry = Arc::new(Mutex::new(Registry::new()));
        let bus = Arc::new(Bus::new());
        let host = HeliosHost::new(registry.clone(), bus.clone(), Arc::default());
        let later = host.later();
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        {
            let mut r = registry.lock().unwrap();
            r.register(
                HeliosSpec::new("test.now", "now", "Test"),
                |args, origin| Ok(json!({ "args": args, "origin": origin })),
            )
            .unwrap();
            r.register(
                HeliosSpec::new("test.later", "later", "Test"),
                move |_, _| {
                    let resolver = later.take().ok_or("no resolver")?;
                    tx.lock().unwrap().send(resolver).unwrap();
                    Ok(json!({ "queued": true }))
                },
            )
            .unwrap();
        }
        let app = App::builder()
            .host(host)
            .plugin(Declares)
            .build(&egui::Context::default())
            .unwrap();
        (app, bus, rx)
    }

    #[test]
    fn commands_run_on_helios_s_registry_with_their_origin() {
        let (mut app, bus, _) = app();
        let reply = app.run(
            &CommandId::from_static("test.now"),
            json!(3),
            PopOrigin::Agent("a1".into()),
        );
        assert_eq!(
            reply.now(),
            Some(Ok(
                json!({ "args": 3, "origin": { "kind": "agent", "id": "a1" } })
            ))
        );
        // On the bus: dispatched, then done caused by it.
        let events = bus.since(0);
        let kinds: Vec<_> = events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["gui.command.dispatched", "gui.command.done"]);
        assert_eq!(events[1].cause, Some(events[0].seq));
        assert_eq!(events[0].source, "agent:a1");
        // egui-pop reads them back under its names.
        let back = app.handle().events_since(0);
        assert_eq!(back[1].name, "command.done");
        assert_eq!(back[1].data["result"]["args"], json!(3));
        // The last event is the bus's, read through the host.
        assert_eq!(app.handle().last_event(), bus.last_seq());
        assert_eq!(app.handle().last_event(), events[1].seq);
    }

    #[test]
    fn a_queued_command_runs_until_its_result_arrives_without_holding_a_frame() {
        let (mut app, bus, resolvers) = app();
        let id = CommandId::from_static("test.later");
        assert!(matches!(
            app.run(&id, json!(null), PopOrigin::User),
            Reply::Later(_)
        ));
        assert!(app.handle().running(&id));
        let resolver = resolvers.try_recv().unwrap();
        std::thread::spawn(move || resolver.resolve(Err("the vault is read-only".into())))
            .join()
            .unwrap();
        assert_eq!(app.settle(), 1);
        assert!(!app.handle().running(&id));
        let last = app.handle().history().last().unwrap().clone();
        assert_eq!(last.result, Err("the vault is read-only".into()));
        assert_eq!(bus.since(0).last().unwrap().kind, "gui.command.failed");
    }

    #[test]
    fn other_events_keep_their_name_and_events_from_elsewhere_wake_the_window() {
        let (app, bus, _) = app();
        app.handle()
            .publish("app", None, "plugin.thing", json!({ "n": 1 }));
        let e = app.handle().events_since(0);
        assert_eq!(e[0].name, "plugin.thing");
        assert_eq!(e[0].data, json!({ "n": 1 }));
        assert_eq!(bus.since(0)[0].kind, OTHER);

        let bus = Arc::new(Bus::new());
        let mut host = HeliosHost::new(Arc::default(), bus.clone(), Arc::default());
        let (tx, woken) = mpsc::channel();
        let tx = Mutex::new(tx);
        host.set_waker(Waker::new(move || {
            let _ = tx.lock().unwrap().send(());
        }));
        // The GUI's own events do not wake it; an engine's does.
        host.publish("user", None, "command.dispatched", json!(null));
        bus.publish_value("engine", None, "engine.render", json!(null));
        woken
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("woken by the engine's event");
        assert!(woken.try_recv().is_err(), "woken once");
    }
}
