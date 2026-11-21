//! Several Library views: the Results panel (`helios.vault`) opens as
//! many times as asked, as egui-pop panel instances (`helios.vault#2`,
//! `helios.vault#3`…). Each new one is a Library view of its own, as a
//! catalogue app's contact sheets are: its own gallery (the gallery id is
//! the instance's id), its Filters inside it, its own filter and cursor.
//! Moving in one never moves another; none follows another.
//!
//! The main explorer (`helios.filters` + `helios.vault`, the gallery
//! `main`) stays as it was.
//!
//! **Kept.** Each view's filter (folder and album included: they are
//! terms of it), cursor and how its results show are the instance's
//! payload ([`ViewState`]), which egui-pop keeps in the layout. The
//! layout is saved with the workspace (its layout copy) and at quit, so
//! a restart gives each view back as it was. Closing a view drops it and
//! its payload; the other views keep theirs. A view's state is view
//! state, never an undo step, and opening or closing one is layout.
//!
//! **On the bus.** A view opens through `pop.panel.open` (`instance:
//! "new"`), as the View menu's "New Library" does; an agent runs
//! [`OPEN_COMMAND`] or [`CLOSE_COMMAND`].

use std::collections::HashMap;

use egui::Ui;
use egui_pop::api::AppHandle;
use egui_pop::api::bus::{OpenPlace, PanelOpenAsk};
use egui_pop::model::ids::PanelId;
use egui_pop::views::library::filter::Filter;
use helios_template::ResultsDesc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::library::{GalleryOptions, Library, RESULTS_PANEL};

/// Opens a new Library view (`place`: `last`, the default, `here` or an
/// area's name); answers its id (`view`).
pub const OPEN_COMMAND: &str = "library.view.open";

/// Closes a Library view (`view`: its id, `helios.vault#<n>`); the
/// others keep their filter and cursor.
pub const CLOSE_COMMAND: &str = "library.view.close";

/// What a Library view keeps: its instance's payload.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewState {
    /// Its filter, the folder and album it is scoped to included.
    #[serde(skip_serializing_if = "Filter::is_empty")]
    pub filter: Filter,
    /// The photo under its cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// How its results show, when not the default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results: Option<ResultsDesc>,
}

/// Whether `id` is a Library view opened as another Results instance
/// (not the main explorer's Results).
pub fn is_view(id: &str) -> bool {
    id != RESULTS_PANEL && crate::library::gallery_of(id).as_deref() == Some(id)
}

/// The Library views open now, the main explorer's Results left out.
pub fn open_views(app: &AppHandle) -> Vec<String> {
    let kind = PanelId::from_static(RESULTS_PANEL);
    app.panel_instances(&kind)
        .into_iter()
        .map(|p| p.to_string())
        .filter(|p| is_view(p))
        .collect()
}

/// Opens a new Library view at `place` through `pop.panel.open`; its id,
/// or `None` while the workbench queued the ask (asked while drawing).
pub fn open(app: &AppHandle, place: OpenPlace, owner: &str) -> Result<Option<String>, String> {
    let before = open_views(app);
    app.open_panel(PanelOpenAsk::new(RESULTS_PANEL, place, owner).instance("new"))?;
    Ok(open_views(app).into_iter().find(|v| !before.contains(v)))
}

/// What [`OPEN_COMMAND`] does: `{ "view": <id> }`.
pub fn open_command(app: &AppHandle, args: &Value) -> Result<Value, String> {
    let place = match args.get("place").filter(|p| !p.is_null()) {
        None => OpenPlace::Last,
        Some(p) => {
            let name = p.as_str().ok_or("`place`: not a place's name")?;
            OpenPlace::parse(name)
                .ok_or_else(|| format!("`place`: no place `{name}` (`last`, `here` or an area)"))?
        }
    };
    match open(app, place, OPEN_COMMAND)? {
        Some(view) => Ok(json!({ "view": view })),
        None => Ok(json!({ "queued": true })),
    }
}

/// What [`CLOSE_COMMAND`] does: closes the view `view` (egui-pop's
/// `workbench.hide` step, as its tab's close button), its payload
/// dropped with it.
pub fn close_command(app: &AppHandle, args: &Value) -> Result<Value, String> {
    let view = args
        .get("view")
        .and_then(Value::as_str)
        .ok_or("`view`: a Library view's id (`helios.vault#<n>`) is missing")?;
    if !is_view(view) {
        return Err(format!(
            "`view`: `{view}` is not a Library view opened as another (`{RESULTS_PANEL}#<n>`)"
        ));
    }
    if !open_views(app).iter().any(|v| v == view) {
        return Err(format!("`view`: no Library view `{view}` is open"));
    }
    let id = PanelId::new(view).map_err(|e| e.to_string())?;
    match app.change_workbench(egui_pop::api::workbench::Change::Hide(id)) {
        Some(egui_pop::api::steps::Outcome::Done(Err(e))) => Err(e),
        Some(egui_pop::api::steps::Outcome::Vetoed { by, reason }) => {
            Err(format!("stopped by `{by}`: {reason}"))
        }
        _ => Ok(json!({ "view": view, "closed": true })),
    }
}

/// The Library views' drawing: each instance's gallery made from its
/// payload the first time it is drawn (new, restored or reopened), and
/// its payload written when its filter, cursor or results change.
#[derive(Default)]
pub struct Views {
    /// Each view's payload as last read or written: one that differs was
    /// set from outside (a restored layout, a reopened view).
    seen: HashMap<String, Value>,
    /// A cursor given back before the photo under it is read.
    pending: HashMap<String, String>,
}

impl Views {
    /// Draws the Library view `id` in `ui`.
    pub fn draw(&mut self, ui: &mut Ui, app: &AppHandle, id: &PanelId, library: &mut Library) {
        let key = id.as_str();
        // The views closed since: their galleries go.
        let open = open_views(app);
        let closed: Vec<String> = self
            .seen
            .keys()
            .filter(|k| !open.contains(k))
            .cloned()
            .collect();
        for k in closed {
            self.seen.remove(&k);
            self.pending.remove(&k);
            library.forget_gallery(&k);
        }
        let payload = app.instance_payload(id);
        if self.seen.get(key) != Some(&payload) {
            self.restore(key, &payload, library);
        }
        if let Some(cursor) = self.pending.get(key).cloned()
            && library.holds(&cursor)
        {
            library.with_gallery(key, |lib| lib.select_ids(std::slice::from_ref(&cursor)));
            self.pending.remove(key);
        }
        library.show_as(ui, &GalleryOptions::VIEW, key);
        let now = self.state_of(key, library);
        let now = serde_json::to_value(&now).unwrap_or(Value::Null);
        if now != payload
            && let Err(e) = app.set_instance_payload(id, now.clone())
        {
            log::warn!(target: "helios", "library view {key} not kept: {e}");
        }
        self.seen.insert(key.to_string(), now);
    }

    /// Makes the gallery `key` as `payload` says, from nothing.
    fn restore(&mut self, key: &str, payload: &Value, library: &mut Library) {
        library.forget_gallery(key);
        self.pending.remove(key);
        let state: ViewState = serde_json::from_value(payload.clone()).unwrap_or_else(|e| {
            if !payload.is_null() {
                log::warn!(target: "helios", "library view {key}: its kept state is not read: {e}");
            }
            ViewState::default()
        });
        library.with_gallery(key, |lib| {
            if !state.filter.is_empty() {
                lib.set_filter(state.filter.clone());
            }
            if let Some(results) = &state.results {
                lib.set_results(results);
            }
        });
        if let Some(cursor) = state.cursor {
            self.pending.insert(key.to_string(), cursor);
        }
    }

    /// What the view `key` keeps now.
    fn state_of(&self, key: &str, library: &mut Library) -> ViewState {
        let pending = self.pending.get(key).cloned();
        library.with_gallery(key, |lib| {
            let results = lib.results();
            ViewState {
                filter: lib.filter().clone(),
                cursor: pending.or_else(|| lib.cursor()),
                results: (results != ResultsDesc::default()).then_some(results),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_results_instances_past_the_first_are_views() {
        assert!(is_view("helios.vault#2"));
        assert!(is_view("helios.vault#12"));
        assert!(!is_view("helios.vault"));
        assert!(!is_view("helios.filters"));
        assert!(!is_view("helios.vault#x"));
        assert!(!is_view("helios.compare#2"));
    }

    #[test]
    fn a_view_s_state_reads_back_and_an_empty_one_is_an_empty_object() {
        assert_eq!(
            serde_json::to_value(ViewState::default()).unwrap(),
            json!({})
        );
        let state = ViewState {
            filter: Filter::parse("rating>=3"),
            cursor: Some("b".into()),
            results: None,
        };
        let back: ViewState =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(back, state);
    }
}
