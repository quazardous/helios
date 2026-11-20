//! A feature crate run without the desktop, for its tests: egui-pop's
//! application over Helios's command registry, the Settings app with the
//! feature's pages and a read kernel, the feature's contract tables
//! declared on the application's bus, and the feature's own state stepped
//! each frame. A feature whose tests run here reaches the rest of the
//! desktop only through the bus and its commands.
//!
//! A feature implements [`Feature`] (its registrations, its settings
//! pages, its points, its frame); [`harness`] draws it with
//! `egui_kittest`.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use egui_kittest::Harness;
use egui_pop::api::admin::model::{FORMAT, Manifest};
use egui_pop::api::bus::bus_contract::Bus;
use egui_pop::api::host::{Dispatch, Host, LocalHost, Reply, Waker};
use egui_pop::api::reads::{ReadKernel, ReadPolicy, ReadTransport};
use egui_pop::api::{App, AppHandle, Ctx, DefaultPlugins, Plugin, PluginError};
use egui_pop::core::binding::SharedStore;
use egui_pop::egui;
use egui_pop::model::commands::Origin as PopOrigin;
use egui_pop::model::ids::{CommandId, PluginId};
use helios_shell::commands::{CommandSpec, Origin, Registry};
use serde_json::Value;

/// A command's handler, as Helios's registry runs it.
pub type Handler = Box<dyn Fn(&Value, &Origin) -> Result<Value, String> + Send + Sync>;

/// Where a feature registers its commands.
pub type Register<'a> = dyn FnMut(CommandSpec, Handler) -> Result<(), String> + 'a;

/// A read-only command's read policy: the command, how long its answer
/// stays fresh, the facts that make it out of date.
pub type ReadPolicies = &'static [(&'static str, Option<Duration>, &'static [&'static str])];

/// A feature as the host runs it.
pub trait Feature {
    /// Its commands, registered before the application is built (the
    /// host declares them to egui-pop when it connects).
    fn register(&mut self, register: &mut Register<'_>, i18n: &egui_pop::i18n::I18n);

    /// Its pages in the Settings app, added to an empty manifest; its
    /// texts through `t`.
    fn settings(
        &self,
        manifest: Manifest,
        _t: &dyn Fn(helios_i18n::Message) -> String,
    ) -> Manifest {
        manifest
    }

    /// The read policies of the commands its pages read.
    fn reads(&self) -> ReadPolicies {
        &[]
    }

    /// Its points declared on the application's bus, and its listeners
    /// registered, once the application is built.
    fn connect(&mut self, app: &AppHandle, bus: &dyn Bus);

    /// Each frame, after the application is drawn.
    fn frame(&mut self, app: &AppHandle);
}

/// egui-pop's origin as Helios's.
fn to_helios(origin: &PopOrigin) -> Origin {
    match origin {
        PopOrigin::User => Origin::User,
        PopOrigin::Agent(n) => Origin::Agent(n.clone()),
        PopOrigin::Script(n) => Origin::Script(n.clone()),
        PopOrigin::Device(_) => Origin::Midi,
        PopOrigin::Remote(n) => Origin::Remote(n.clone()),
    }
}

/// The host: egui-pop's own commands run in-process, Helios's through the
/// registry; the journal in memory.
struct RegistryHost {
    registry: Arc<Mutex<Registry>>,
    local: LocalHost,
}

impl Host for RegistryHost {
    fn dispatch(&mut self, d: &Dispatch<'_>) -> Reply {
        if d.has_local() {
            return Reply::Done(d.run_local().unwrap_or_else(|| Err("no handler".into())));
        }
        let mut cx = helios_shell::when::Context::new();
        for (k, v, _) in d.context.iter() {
            cx.set(k, v);
        }
        Reply::Done(
            self.registry
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .run(d.command.as_str(), d.args.clone(), to_helios(d.origin), &cx),
        )
    }

    fn journal(&self) -> Option<&egui_pop::api::host::Journal> {
        self.local.journal()
    }

    fn journal_mut(&mut self) -> Option<&mut egui_pop::api::host::Journal> {
        self.local.journal_mut()
    }

    fn store(&self) -> SharedStore {
        self.local.store()
    }

    fn set_waker(&mut self, waker: Waker) {
        self.local.set_waker(waker);
    }
}

/// Every command of the registry declared to egui-pop, and the feature's
/// Settings pages.
struct Declare {
    registry: Arc<Mutex<Registry>>,
    manifest: Option<Manifest>,
}

impl Plugin for Declare {
    fn id(&self) -> PluginId {
        PluginId::from_static("helios.feature-host")
    }

    fn title(&self) -> String {
        "Feature host".into()
    }

    fn requires(&self) -> Vec<PluginId> {
        vec![PluginId::from_static("egui_pop.admin")]
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        let specs: Vec<CommandSpec> = self
            .registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .specs()
            .cloned()
            .collect();
        for spec in specs {
            let id = CommandId::new(&spec.id)
                .map_err(|e| PluginError::Failed(format!("{}: {e}", spec.id)))?;
            let mut s =
                egui_pop::model::commands::CommandSpec::new(id, &spec.title, &spec.category)
                    .when(&spec.when);
            if spec.read_only {
                s = s.read_only();
            }
            ctx.declare(s)
                .map_err(|e| PluginError::Failed(e.to_string()))?;
        }
        if let (Some(manifest), Some(admin)) = (
            self.manifest.take(),
            ctx.app().service(&egui_pop::api::admin::ADMIN),
        ) {
            admin.manifest(manifest);
        }
        Ok(())
    }
}

/// The read-only commands of the registry, read by the Settings app.
struct RegistryReads {
    registry: Arc<Mutex<Registry>>,
    policies: ReadPolicies,
}

impl ReadTransport for RegistryReads {
    fn serves(&self, command: &str) -> bool {
        self.policies.iter().any(|(c, _, _)| *c == command)
    }

    fn read(&self, command: &str, args: &Value) -> Result<Value, String> {
        let handler = self
            .registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .reader(command)
            .ok_or_else(|| format!("no read-only command `{command}`"))?;
        handler(args, &Origin::User)
    }
}

/// The application a feature runs in, and the feature.
pub struct Hosted<F> {
    pub app: App,
    pub feature: F,
}

impl<F: Feature> Hosted<F> {
    /// The application built around `feature`.
    pub fn new(mut feature: F) -> Self {
        let registry = Arc::new(Mutex::new(Registry::new()));
        let i18n_plugin = egui_pop::i18n::I18nPlugin::new()
            .catalogues(helios_i18n::CATALOGUES)
            .env(helios_i18n::ENV)
            .language("en");
        let i18n = i18n_plugin.i18n();
        {
            let mut r = registry.lock().unwrap_or_else(PoisonError::into_inner);
            feature.register(
                &mut |spec: CommandSpec, handler: Handler| r.register(spec, handler),
                &i18n,
            );
        }
        let t = |m: helios_i18n::Message| helios_i18n::message(&i18n, &m);
        let manifest = feature.settings(Manifest::new(FORMAT), &t);
        let host = RegistryHost {
            registry: registry.clone(),
            local: LocalHost::default(),
        };
        let app = App::builder()
            .name("Helios")
            .host(host)
            .plugins(DefaultPlugins)
            .plugin(egui_pop::api::admin::AdminPlugin)
            .plugin(Declare {
                registry: registry.clone(),
                manifest: Some(manifest),
            })
            .plugin(i18n_plugin)
            .build(&egui::Context::default())
            .unwrap_or_else(|e| panic!("the feature's application builds: {e}"));
        if let Some(admin) = app.handle().service(&egui_pop::api::admin::ADMIN) {
            let policies = feature.reads();
            let k = ReadKernel::new(Arc::new(RegistryReads { registry, policies }));
            for (command, fresh, events) in policies {
                let mut p = ReadPolicy::new().invalidated_by(events);
                p.fresh_for = *fresh;
                k.policy(command, p);
            }
            admin.attach_reads(Arc::new(k));
        }
        let handle = app.handle().clone();
        feature.connect(&handle, &*handle.bus());
        Self { app, feature }
    }

    /// One frame: the application's logic, what was asked before the
    /// drawing heard, the application drawn, then the feature's frame.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.app.logic(&ctx);
        self.app.handle().drain_asks();
        self.app.show(ui);
        self.feature.frame(self.app.handle());
    }

    /// Runs command `id` as the user does, its answer at once (a queued
    /// command answers `{"queued": true}`); refused, it is said as a
    /// warning toast, as the desktop says it.
    pub fn run(&mut self, id: &str, args: Value) -> Result<Value, String> {
        let id = CommandId::new(id).map_err(|e| e.to_string())?;
        let result = match self.app.run(&id, args, PopOrigin::User) {
            egui_pop::api::host::Reply::Done(r) => r,
            egui_pop::api::host::Reply::Later(_) => Ok(serde_json::json!({ "queued": true })),
        };
        if let (Err(e), Some(toasts)) = (
            &result,
            self.app.handle().service(&egui_pop::api::toasts::TOASTS),
        ) {
            toasts.notify(egui_pop::api::toasts::Toast::warning(e));
        }
        result
    }

    /// The application's handle.
    pub fn handle(&self) -> &AppHandle {
        self.app.handle()
    }
}

/// `feature` drawn offscreen at a size, its application built.
pub fn harness<'a, F: Feature + 'a>(feature: F, size: egui::Vec2) -> Harness<'a, Hosted<F>> {
    let hosted = Hosted::new(feature);
    let h = Harness::builder()
        .with_size(size)
        .build_ui_state(|ui, hosted: &mut Hosted<F>| hosted.ui(ui), hosted);
    h.ctx.all_styles_mut(|style| {
        style.visuals.text_cursor.blink = false;
        style.animation_time = 0.0;
    });
    h
}
