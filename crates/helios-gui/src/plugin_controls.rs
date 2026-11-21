//! A plugin's declared controls (`[ui] rack`, the `ui` interface): its
//! own values in the value store, a card of them in its section of
//! Settings > Plugins, and each committed change told to it.
//!
//! - **Values.** Each enabled plugin's values are declared at
//!   `plugin/<id>/<key>` with the descriptors its declaration gives
//!   (`helios_rack::plugin`), and handed to the broker, so the plugin
//!   gets them at `initialize`. Their steps are the plugins' history
//!   scope (`helios_values::HistoryScope::Plugin`), kept across photos.
//! - **The card.** The plugin's section holds a registered widget,
//!   [`WIDGET`], which the host draws through `helios-rack` as any
//!   parameter card: a drag is one undo step, Escape gives it up with no
//!   trace, steps in a row are one step. The plugin draws nothing.
//! - **Told.** The plugins' listener ([`PLUGINS`], after the journal's)
//!   gathers each surface change as the journal does
//!   (`helios.param.changed`) and, for a committed one (a step made, an
//!   undo, a redo), tells each plugin whose values moved through
//!   [`helios_plugin::Broker::ui_changed`]: `ui/changed {paths, values,
//!   cause}`, `cause` `edit`, `undo` or `redo`. A preview, a gesture's
//!   frames and a gesture given up tell nothing.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::app::AppHandle;
use egui_pop::api::bus::bus_contract::ListenOpts;
use helios_controls::HeliosRegistry as _;
use helios_plugin::Broker;
use helios_rack::plugin::{self, Declared};
use helios_rack::{Extra, Rack};
use helios_values::{HistoryScope, Store};
use serde_json::{Map, Value as Json};

use crate::values_changed::{self as vc, Change, Gather, ValuesChanged};

/// The admin widget kind drawing a plugin's card (`{"plugin": <id>}`).
pub const WIDGET: &str = "helios.plugin-controls";

/// The plugins' listener on the values' changes: tells each plugin its
/// committed changes.
pub const PLUGINS: &str = "helios.plugins.ui";

/// A plugin's rack declarations, read from its folder and checked against
/// the catalogue; `None` when it declares none.
pub fn read(broker: &Broker, id: &str) -> Option<Declared> {
    let m = broker.manifest(id)?;
    if m.ui.rack.is_empty() {
        return None;
    }
    let dir = broker.dir(id)?;
    let catalogue = helios_controls::Registry::standard();
    let mut d = Declared::new(id);
    for file in &m.ui.rack {
        match std::fs::read_to_string(dir.join(file)) {
            Ok(text) => d.read(file, &text, &catalogue),
            Err(e) => d.problems.push(format!("{file}: not read: {e}")),
        }
    }
    for p in &d.problems {
        log::warn!("plugin `{id}`'s controls: {p}");
    }
    Some(d)
}

/// Declares each enabled plugin's values in `values` (those not declared
/// yet), hands them to the broker, draws their cards in Settings
/// ([`WIDGET`]) and tells each plugin its committed changes on `surface`.
pub fn install(app: &AppHandle, broker: &Arc<Broker>, values: &Arc<Mutex<Store>>, surface: u64) {
    let mut cards = BTreeMap::new();
    for id in broker.ids() {
        let Some(d) = read(broker, id) else {
            continue;
        };
        if !broker.enabled(id) {
            continue;
        }
        let mut now = Map::new();
        {
            let mut s = values.lock().unwrap_or_else(PoisonError::into_inner);
            for (key, descriptor) in &d.values {
                let path = plugin::path(id, key);
                if s.get(&path).is_none() {
                    s.declare(&path, descriptor.clone());
                }
                if let Some(e) = s.get(&path) {
                    now.insert(key.clone(), plugin::to_json(&e.value));
                }
            }
        }
        if let Err(e) = broker.set_ui_values(id, now) {
            log::warn!("plugin `{id}`'s values: {e}");
        }
        cards.insert(id.to_owned(), d);
    }
    if cards.is_empty() {
        return;
    }
    let cards = Arc::new(cards);
    if let Some(admin) = app.service(&egui_pop::api::admin::ADMIN) {
        let (store, shown) = (values.clone(), cards.clone());
        admin.widget_kind(WIDGET, move |ui, app, options| {
            let Some(d) = options
                .get("plugin")
                .and_then(Json::as_str)
                .and_then(|id| shown.get(id))
            else {
                return;
            };
            card(ui, app, &store, d);
        });
    }
    listen(app, surface, broker.clone(), values.clone(), cards);
}

thread_local! {
    /// The rack the cards are drawn by: the interface's thread's (it
    /// speaks on the application's bus, which stays on that thread).
    static RACK: RefCell<Option<Rack>> = const { RefCell::new(None) };
}

/// A plugin's card, drawn by the rack in the width available.
fn card(ui: &mut egui_pop::egui::Ui, app: &AppHandle, store: &Mutex<Store>, d: &Declared) {
    RACK.with_borrow_mut(|rack| {
        let rack = rack.get_or_insert_with(|| {
            let mut r = Rack::new(
                helios_controls::commands::SharedContexts::default(),
                Arc::clone(crate::controls::registry()),
                Arc::clone(crate::controls::steps()),
            );
            r.set_app(app.clone());
            r
        });
        draw(ui, rack, store, d);
    });
}

fn draw(ui: &mut egui_pop::egui::Ui, rack: &mut Rack, store: &Mutex<Store>, d: &Declared) {
    let paths: Vec<&str> = d.parts.iter().map(|p| p.path.as_str()).collect();
    let extras: BTreeMap<String, Extra> = d
        .parts
        .iter()
        .filter_map(|p| {
            let control = p.control.clone()?;
            Some((
                p.path.clone(),
                Extra {
                    control: Some(control),
                    ..Extra::default()
                },
            ))
        })
        .collect();
    let keys = rack.keys(ui, false);
    let id = format!("plugin:{}", d.plugin);
    let _ = rack.card(ui, store, &d.plugin, &id, &paths, &extras, keys);
    rack.end_steps(store, ui.input(|i| i.time));
}

/// The plugins' line's heads: its newest undo step and its next redo.
fn heads(s: &Store) -> (Option<u64>, Option<u64>) {
    (
        s.undo_steps_of(HistoryScope::Plugin).last().map(|s| s.id),
        s.redo_steps_of(HistoryScope::Plugin).last().map(|s| s.id),
    )
}

/// What moved the plugins' line from `before` to `after`: an undo when
/// its newest step went to the redo list, a redo when its next redo came
/// back, else an edit.
pub fn cause(
    before: (Option<u64>, Option<u64>),
    after: (Option<u64>, Option<u64>),
) -> &'static str {
    if before.0.is_some() && after.1 == before.0 {
        "undo"
    } else if before.1.is_some() && after.0 == before.1 {
        "redo"
    } else {
        "edit"
    }
}

/// The plugins' listener ([`PLUGINS`], after the journal's) on
/// `surface`'s changes.
fn listen(
    app: &AppHandle,
    surface: u64,
    broker: Arc<Broker>,
    values: Arc<Mutex<Store>>,
    cards: Arc<BTreeMap<String, Declared>>,
) {
    let bus = app.bus();
    vc::declare(&*bus);
    bus.unlisten(vc::CHANGED, PLUGINS);
    let gather = RefCell::new(Gather::default());
    let last = RefCell::new(heads(
        &values.lock().unwrap_or_else(PoisonError::into_inner),
    ));
    let listened = bus.listen_local::<ValuesChanged>(
        PLUGINS,
        ListenOpts::new().owner(vc::BUS_OWNER).after(vc::JOURNAL),
        move |change: &Change, (): &mut ()| {
            if change.surface != surface {
                return Ok(());
            }
            let Some(said) = gather.borrow_mut().take(change) else {
                return Ok(());
            };
            if !said.step {
                return Ok(());
            }
            let mut told: BTreeMap<&str, Map<String, Json>> = BTreeMap::new();
            let why = {
                let s = values.lock().unwrap_or_else(PoisonError::into_inner);
                for path in &said.paths {
                    let Some((id, key)) = plugin::split(path) else {
                        continue;
                    };
                    let Some((id, _)) = cards.get_key_value(id) else {
                        continue;
                    };
                    if let Some(e) = s.get(path) {
                        told.entry(id.as_str())
                            .or_default()
                            .insert(key.to_owned(), plugin::to_json(&e.value));
                    }
                }
                let now = heads(&s);
                let why = cause(*last.borrow(), now);
                *last.borrow_mut() = now;
                why
            };
            for (id, changed) in told {
                if let Err(e) = broker.ui_changed(id, changed, why) {
                    log::warn!("{e}");
                }
            }
            Ok(())
        },
    );
    if let Err(e) = listened {
        log::error!(target: "helios", "{PLUGINS}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::cause;

    #[test]
    fn the_cause_is_read_from_the_plugins_line() {
        assert_eq!(cause((None, None), (Some(1), None)), "edit");
        assert_eq!(cause((Some(1), None), (None, Some(1))), "undo");
        assert_eq!(cause((None, Some(1)), (Some(1), None)), "redo");
        assert_eq!(cause((Some(1), Some(2)), (Some(3), None)), "edit");
    }
}
