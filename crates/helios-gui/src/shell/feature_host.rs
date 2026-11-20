//! A feature run without the shell, for its tests: egui-pop's application
//! over Helios's command registry and host, the Settings app with
//! Helios's scopes (`crate::config::BridgePlugin`) and its read kernel,
//! the feature's contract tables declared on the application's bus, and
//! the feature's own state stepped each frame. No `Shell`: a feature
//! whose tests run here reaches the rest of the desktop only through
//! the bus and its commands, which is what it needs to leave this crate.
//!
//! A feature implements [`Feature`] (its registrations, its points, its
//! frame); [`harness`] draws it with egui_kittest the way
//! `super::headless::harness` draws a shell.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_pop::api::bus::bus_contract::Bus as ContractBus;
use egui_pop::api::{App, AppHandle, DefaultPlugins};
use egui_pop::model::ids::CommandId;
use helios_shell::commands::{CommandSpec, Registry};
use serde_json::Value;

use super::storage_admin::Handler;
use crate::host::HeliosHost;
use crate::plugins::{self, Declarations};

/// A feature as the host runs it.
pub(crate) trait Feature {
    /// Its commands, registered before the application is built (its
    /// plugins declare them when they connect).
    fn register(
        &mut self,
        register: &mut super::storage_admin::Register<'_>,
        i18n: &egui_pop::i18n::I18n,
    );

    /// Its points declared on the application's bus, and its listeners
    /// registered, once the application is built.
    fn connect(&mut self, bus: &dyn ContractBus);

    /// Each frame, after the application is drawn.
    fn frame(&mut self, app: &AppHandle);
}

/// The application a feature runs in, and the feature.
pub(crate) struct Hosted<F> {
    pub(crate) app: App,
    pub(crate) feature: F,
}

impl<F: Feature> Hosted<F> {
    /// The application built around `feature`.
    pub(crate) fn new(mut feature: F) -> Self {
        let registry = Arc::new(Mutex::new(Registry::new()));
        let i18n_plugin = egui_pop::i18n::I18nPlugin::new()
            .catalogues(helios_i18n::CATALOGUES)
            .env(helios_i18n::ENV)
            .language("en");
        let i18n = i18n_plugin.i18n();
        {
            let mut r = registry.lock().unwrap();
            feature.register(
                &mut |spec: CommandSpec, handler: Handler| r.register(spec, handler),
                &i18n,
            );
        }
        let host =
            HeliosHost::new(registry.clone(), crate::bus(), Arc::default()).with_i18n(i18n.clone());
        let app = App::builder()
            .name("Helios")
            .host(host)
            .plugins(DefaultPlugins)
            .plugin(egui_pop::api::admin::AdminPlugin)
            .plugin(crate::config::BridgePlugin {
                i18n: Some(i18n.clone()),
                plugins: crate::config::plugins::Shared::default(),
            })
            .plugin(plugins::Commands {
                registry: registry.clone(),
                declarations: Rc::new(RefCell::new(Declarations::default())),
            })
            .plugin(i18n_plugin)
            .build(&egui::Context::default())
            .unwrap_or_else(|e| panic!("the feature's application builds: {e}"));
        if let Some(admin) = app.handle().service(&egui_pop::api::admin::ADMIN) {
            admin.attach_reads(super::reads::kernel(registry));
        }
        feature.connect(&*app.handle().bus());
        Self { app, feature }
    }

    /// One frame: the application's logic, what was asked before the
    /// drawing heard, the application drawn, then the feature's frame.
    pub(crate) fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.app.logic(&ctx);
        self.app.handle().drain_asks();
        self.app.show(ui);
        self.feature.frame(self.app.handle());
    }

    /// Runs command `id` as the user does, its answer at once (a queued
    /// command answers `{"queued": true}`).
    pub(crate) fn run(&mut self, id: &str, args: Value) -> Result<Value, String> {
        let id = CommandId::new(id).map_err(|e| e.to_string())?;
        match self
            .app
            .run(&id, args, egui_pop::model::commands::Origin::User)
        {
            egui_pop::api::host::Reply::Done(r) => r,
            egui_pop::api::host::Reply::Later(_) => Ok(serde_json::json!({ "queued": true })),
        }
    }

    /// The application's handle.
    pub(crate) fn handle(&self) -> &AppHandle {
        self.app.handle()
    }
}

/// `feature` drawn offscreen at a size, its application built.
pub(crate) fn harness<'a, F: Feature + 'a>(feature: F, size: egui::Vec2) -> Harness<'a, Hosted<F>> {
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
