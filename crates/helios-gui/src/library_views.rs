//! Several Library views: the Results panel (`helios.vault`) opens as
//! many times as asked, as egui-pop panel instances (`helios.vault#2`,
//! `helios.vault#3`…). Each new one is a Library view of its own, as a
//! catalogue app's contact sheets are: its own gallery (the gallery id is
//! the instance's id), its Filters inside it, its own filter and cursor.
//! Moving in one never moves another, unless it is linked: a view
//! linked to the main one ([`LINK_COMMAND`], the link toggle in its bar)
//! takes the main view's selection and cursor each time they change;
//! unlinked, it keeps its own.
//!
//! The main explorer (`helios.filters` + `helios.vault`, the gallery
//! `main`) stays as it was.
//!
//! **Kept.** Each view's filter (folder and album included: they are
//! terms of it), cursor and how its results show are the instance's
//! payload ([`ViewState`]), which egui-pop keeps in the layout. The
//! layout is saved with the workspace (its layout copy) and at quit, so
//! a restart gives each view back as it was. The main explorer's
//! selection and cursor go the same way: the main Results panel's
//! payload (instance 1 of `helios.vault`). Closing a view drops it and
//! its payload; the other views keep theirs. A view's state is view
//! state, never an undo step, and opening or closing one is layout.
//!
//! **On the bus.** A view opens through `pop.panel.open` (`instance:
//! "new"`), as the View menu's "New Library" does; an agent runs
//! [`OPEN_COMMAND`] or [`CLOSE_COMMAND`].
//!
//! **Quick-collection panels.** A view may show a quick collection
//! ([`QUICK_OPEN_COMMAND`], a quick collection's "Open in a panel"): its
//! source is that collection (its filter is `collection:<id>`, kept with
//! the rest of its state), its tab is titled with the collection's name,
//! its grid has no search, and the whole panel takes dropped photos. Photos dragged from one quick collection to
//! another move (`library.quick.move`); with Ctrl they are copied
//! (`library.quick.add`); from anywhere else they are added. As many
//! can be open as wanted, each with its own cursor, so sorting between
//! them ("this one I like, this one less") moves no other view's.
//!
//! **The culling strip.** A view may be laid out in one row
//! ([`OPEN_COMMAND`] with `strip: true`, placed under Compare by
//! default): a filmstrip of its own, linked to the main view when it
//! opens (its link toggle unlinks it). The arrows move its cursor, and
//! Space adds the photo under it to the comparison, or takes it out
//! when it is compared (`compare.add`, `compare.remove`). Its layout is
//! kept with the rest of its state.

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
/// area's name); answers its id (`view`). With `strip: true`, the view
/// is the culling strip: one row, linked to the main view, under
/// Compare (`bottom`) unless `place` says otherwise.
pub const OPEN_COMMAND: &str = "library.view.open";

/// Closes a Library view (`view`: its id, `helios.vault#<n>`); the
/// others keep their filter and cursor.
pub const CLOSE_COMMAND: &str = "library.view.close";

/// Opens a quick collection (`id`; none: the target) in a panel of its
/// own (`place`: `last`, the default, `here` or an area's name); answers
/// its id (`view`).
pub const QUICK_OPEN_COMMAND: &str = "library.quick.open";

/// Says a Library view's selection and cursor (`view`: its id,
/// `helios.vault#<n>`, or `main`; none: every view, the main one
/// first): `{ "view", "selection", "cursor" }`, or `{ "views": [...] }`.
/// What is kept with the layout reads back the same after a restart.
pub const SELECTION_COMMAND: &str = "library.view.selection";

/// Links a Library view to the main one, or unlinks it (`view`: its id;
/// `linked`: true or false, none: the other way round): a linked view
/// follows the main view's selection and cursor, an unlinked one keeps
/// its own. Kept with the view. Answers `{ "view", "linked" }`.
pub const LINK_COMMAND: &str = "library.view.link";

/// What a linked view follows: the main view.
const LINKED_TO: &str = "main";

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
    /// The photos selected in it, in order, when more than the one under
    /// its cursor (a lone cursor is its own selection).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub selection: Vec<String>,
    /// How its results show, when not the default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results: Option<ResultsDesc>,
    /// The quick collection it shows, for a quick-collection panel: its
    /// source, the filter scoped to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quick: Option<String>,
    /// The view it follows (`main`), when linked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Laid out in one row: the culling strip.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub strip: bool,
}

impl ViewState {
    /// A quick-collection panel's state: the collection `id` its source.
    pub fn of_quick(id: &str) -> ViewState {
        ViewState {
            filter: Filter::parse(&format!("collection:{}", crate::library::quoted(id))),
            quick: Some(id.to_string()),
            ..ViewState::default()
        }
    }
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

/// The place a command's `place` names (`fallback` when none).
fn place_or(args: &Value, fallback: OpenPlace) -> Result<OpenPlace, String> {
    match args.get("place").filter(|p| !p.is_null()) {
        None => Ok(fallback),
        Some(p) => {
            let name = p.as_str().ok_or("`place`: not a place's name")?;
            OpenPlace::parse(name)
                .ok_or_else(|| format!("`place`: no place `{name}` (`last`, `here` or an area)"))
        }
    }
}

/// The place a command's `place` names (`last` when none).
fn place_arg(args: &Value) -> Result<OpenPlace, String> {
    place_or(args, OpenPlace::Last)
}

/// What [`OPEN_COMMAND`] does: `{ "view": <id> }` (and `"strip": true`
/// for the culling strip).
pub fn open_command(app: &AppHandle, args: &Value) -> Result<Value, String> {
    let strip = match args.get("strip") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(v) => return Err(format!("`strip`: {v} is not true or false")),
    };
    if !strip {
        return match open(app, place_arg(args)?, OPEN_COMMAND)? {
            Some(view) => Ok(json!({ "view": view })),
            None => Ok(json!({ "queued": true })),
        };
    }
    let under = OpenPlace::parse("bottom").unwrap_or(OpenPlace::Last);
    let Some(view) = open(app, place_or(args, under)?, OPEN_COMMAND)? else {
        // Asked while drawing (a menu's entry): the workbench opens the
        // view next frame, and its first draw makes it the strip.
        app.egui()
            .data_mut(|d| d.insert_temp(strip_pending_id(), true));
        return Ok(json!({ "queued": true, "strip": true }));
    };
    let id = PanelId::new(&view).map_err(|e| e.to_string())?;
    app.set_instance_payload(&id, strip_state())?;
    Ok(json!({ "view": view, "strip": true }))
}

/// Where a strip asked while drawing waits for its view to open.
fn strip_pending_id() -> egui::Id {
    egui::Id::unique("helios.library.strip.pending")
}

/// What a culling strip keeps when it opens: one row, linked to the
/// main view.
pub fn strip_state() -> Value {
    let state = ViewState {
        strip: true,
        link: Some(LINKED_TO.to_string()),
        ..ViewState::default()
    };
    serde_json::to_value(state).unwrap_or(Value::Null)
}

/// The culling strip the default Compare layout opens under Compare
/// (`workbench::comparison`): the Results panel's second instance, the
/// first view after the main explorer's.
pub const DEFAULT_STRIP: &str = "helios.vault#2";

/// The culling strips open now.
pub fn open_strips(app: &AppHandle) -> Vec<String> {
    open_views(app)
        .into_iter()
        .filter(|v| {
            PanelId::new(v).is_ok_and(|id| {
                app.instance_payload(&id)
                    .get("strip")
                    .and_then(Value::as_bool)
                    == Some(true)
            })
        })
        .collect()
}

/// What [`QUICK_OPEN_COMMAND`] does for the quick collection `quick`
/// (the caller checked it exists): a new view whose source it is,
/// `{ "view": <id>, "id": <quick> }`.
pub fn open_quick_command(app: &AppHandle, args: &Value, quick: &str) -> Result<Value, String> {
    let Some(view) = open(app, place_arg(args)?, QUICK_OPEN_COMMAND)? else {
        return Err("the panel could not open now: ask again".into());
    };
    let id = PanelId::new(&view).map_err(|e| e.to_string())?;
    let state = serde_json::to_value(ViewState::of_quick(quick)).map_err(|e| e.to_string())?;
    app.set_instance_payload(&id, state)?;
    Ok(json!({ "view": view, "id": quick }))
}

/// The quick-collection panels open now, each with the quick collection
/// it shows.
pub fn quick_views(app: &AppHandle) -> Vec<(String, String)> {
    open_views(app)
        .into_iter()
        .filter_map(|v| {
            let id = PanelId::new(&v).ok()?;
            let state: ViewState = serde_json::from_value(app.instance_payload(&id)).ok()?;
            Some((v, state.quick?))
        })
        .collect()
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
    /// A selection and cursor given back before the photos are read.
    pending: HashMap<String, Kept>,
    /// The quick collection each quick-collection panel shows.
    quick: HashMap<String, String>,
    /// The views laid out in one row (culling strips).
    strips: std::collections::HashSet<String>,
    /// The tab title last given to each quick-collection panel: its
    /// collection's name, given again only when it changes.
    titled: HashMap<String, String>,
    /// The main view's selection and cursor last given to each linked
    /// view: given again only when they change.
    followed: HashMap<String, Kept>,
    /// The main view's payload as last read or written.
    main_seen: Option<Value>,
    /// The main view's selection and cursor given back before the
    /// photos are read.
    main_pending: Option<Kept>,
}

/// A payload without its link: what restoring a view looks at (a link
/// turned on or off restores nothing).
fn unlinked(payload: &Value) -> Value {
    let mut p = payload.clone();
    if let Some(o) = p.as_object_mut() {
        o.remove("link");
    }
    p
}

impl Views {
    /// Draws the Library view `id` in `ui`; `compared`: the photos the
    /// comparison holds, for a culling strip's Space.
    pub fn draw(
        &mut self,
        ui: &mut Ui,
        app: &AppHandle,
        id: &PanelId,
        library: &mut Library,
        compared: &[String],
    ) {
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
            self.quick.remove(&k);
            self.strips.remove(&k);
            self.titled.remove(&k);
            self.followed.remove(&k);
            library.forget_gallery(&k);
        }
        let mut payload = app.instance_payload(id);
        // A strip asked while drawing: the first new view drawn is it.
        if payload.is_null()
            && !self.seen.contains_key(key)
            && ui
                .ctx()
                .data_mut(|d| d.remove_temp::<bool>(strip_pending_id()))
                .unwrap_or(false)
        {
            payload = strip_state();
            if let Err(e) = app.set_instance_payload(id, payload.clone()) {
                log::warn!(target: "helios", "library view {key}: not made the strip: {e}");
            }
        }
        if self.seen.get(key).map(unlinked) != Some(unlinked(&payload)) {
            self.restore(key, &payload, library);
        }
        let linked = payload.get("link").and_then(Value::as_str) == Some(LINKED_TO);
        if linked {
            let main = library.with_gallery(crate::library::MAIN_GALLERY, |lib| Kept::of(lib));
            if self.followed.get(key) != Some(&main) {
                library.with_gallery(key, |lib| main.apply(lib));
                self.pending.remove(key);
                self.followed.insert(key.to_string(), main);
            }
        } else {
            self.followed.remove(key);
        }
        if let Some(kept) = self.pending.get(key).cloned()
            && kept.ready(library)
        {
            library.with_gallery(key, |lib| kept.apply(lib));
            self.pending.remove(key);
        }
        if !self.quick.contains_key(key) {
            link_toggle(ui, id, linked);
        }
        match self.quick.get(key).cloned() {
            Some(quick) => {
                let rect = ui.max_rect();
                if let Some(name) = quick_header(ui, library, &quick) {
                    self.retitle(app, id, name);
                }
                library.show_as(ui, &GalleryOptions::QUICK, key);
                library.quick_drop_target(ui, &quick, rect);
            }
            None if self.strips.contains(key) => {
                let label = crate::i18n::text(
                    ui.ctx(),
                    helios_core::Message::new("gui-library-culling-strip", "Culling filmstrip"),
                );
                let strip = library.strip_as(ui, key, &label);
                if strip.is_some_and(|r| r.has_focus()) {
                    let cursor = library.with_gallery(key, |lib| lib.cursor());
                    space_to_compare(ui, cursor, compared);
                }
            }
            None => library.show_as(ui, &GalleryOptions::VIEW, key),
        }
        let mut now = self.state_of(key, library);
        now.link = linked.then(|| LINKED_TO.to_string());
        let now = serde_json::to_value(&now).unwrap_or(Value::Null);
        if now != payload
            && let Err(e) = app.set_instance_payload(id, now.clone())
        {
            log::warn!(target: "helios", "library view {key} not kept: {e}");
        }
        self.seen.insert(key.to_string(), now);
    }

    /// Keeps the main explorer's selection and cursor as its Results
    /// panel's payload (`id`: `helios.vault`), with the layout as the
    /// other views' are, and gives them back once the photos are read.
    pub fn keep_main(&mut self, app: &AppHandle, id: &PanelId, library: &mut Library) {
        let main = crate::library::MAIN_GALLERY;
        let payload = app.instance_payload(id);
        if self.main_seen.as_ref() != Some(&payload) {
            let state: ViewState = serde_json::from_value(payload.clone()).unwrap_or_default();
            self.main_pending =
                (state.cursor.is_some() || !state.selection.is_empty()).then_some(Kept {
                    selection: state.selection,
                    cursor: state.cursor,
                });
        }
        if let Some(kept) = self.main_pending.clone()
            && kept.ready(library)
        {
            library.with_gallery(main, |lib| kept.apply(lib));
            self.main_pending = None;
        }
        let kept = match &self.main_pending {
            Some(k) => k.clone(),
            None => library.with_gallery(main, |lib| Kept::of(lib)),
        };
        let now = ViewState {
            selection: kept.selection_kept(),
            cursor: kept.cursor,
            ..ViewState::default()
        };
        let now = serde_json::to_value(&now).unwrap_or(Value::Null);
        if now != payload
            && let Err(e) = app.set_instance_payload(id, now.clone())
        {
            log::warn!(target: "helios", "the main Library view's selection not kept: {e}");
        }
        self.main_seen = Some(now);
    }

    /// Names a quick-collection panel's tab after its collection (not
    /// "Results 2"), through egui-pop's `pop.panel.retitle`; the title is
    /// kept with the layout. Asked again only when the name changes (a
    /// rename, another language).
    fn retitle(&mut self, app: &AppHandle, id: &PanelId, name: String) {
        if self.titled.get(id.as_str()) == Some(&name) {
            return;
        }
        match app.set_instance_title(id, Some(name.clone())) {
            Ok(()) => {
                self.titled.insert(id.to_string(), name);
            }
            Err(e) => log::warn!(target: "helios", "library view {id}: not titled: {e}"),
        }
    }

    /// Makes the gallery `key` as `payload` says, from nothing.
    fn restore(&mut self, key: &str, payload: &Value, library: &mut Library) {
        library.forget_gallery(key);
        self.pending.remove(key);
        self.quick.remove(key);
        self.strips.remove(key);
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
        if state.cursor.is_some() || !state.selection.is_empty() {
            self.pending.insert(
                key.to_string(),
                Kept {
                    selection: state.selection,
                    cursor: state.cursor,
                },
            );
        }
        if let Some(quick) = state.quick {
            self.quick.insert(key.to_string(), quick);
        }
        if state.strip {
            self.strips.insert(key.to_string());
        }
    }

    /// What the view `key` keeps now.
    fn state_of(&self, key: &str, library: &mut Library) -> ViewState {
        let pending = self.pending.get(key).cloned();
        let quick = self.quick.get(key).cloned();
        let strip = self.strips.contains(key);
        library.with_gallery(key, |lib| {
            let results = lib.results();
            let kept = pending.unwrap_or_else(|| Kept::of(lib));
            ViewState {
                filter: lib.filter().clone(),
                selection: kept.selection_kept(),
                cursor: kept.cursor,
                results: (results != ResultsDesc::default()).then_some(results),
                quick,
                link: None,
                strip,
            }
        })
    }
}

/// A view's selection and cursor, as kept or given back.
#[derive(Clone, Debug, Default, PartialEq)]
struct Kept {
    selection: Vec<String>,
    cursor: Option<String>,
}

impl Kept {
    /// The gallery in force's.
    fn of(lib: &Library) -> Kept {
        Kept {
            selection: lib.selection(),
            cursor: lib.cursor(),
        }
    }

    /// The selection as kept: none when it is the lone cursor.
    fn selection_kept(&self) -> Vec<String> {
        if self.selection.len() == 1 && self.cursor.as_ref() == self.selection.first() {
            Vec::new()
        } else {
            self.selection.clone()
        }
    }

    /// Whether the photos are read: the cursor's, or else the
    /// selection's first.
    fn ready(&self, library: &Library) -> bool {
        self.cursor
            .as_ref()
            .or(self.selection.first())
            .is_some_and(|id| library.holds(id))
    }

    /// Gives it back to the gallery in force; a lone cursor selects
    /// itself.
    fn apply(&self, lib: &mut Library) {
        if self.selection.is_empty() {
            lib.select_ids(&self.cursor.iter().cloned().collect::<Vec<_>>());
        } else {
            lib.select_kept(&self.selection, self.cursor.as_deref());
        }
    }
}

/// One view's answer to [`SELECTION_COMMAND`].
fn selection_of(view: &str, kept: &Kept) -> Value {
    json!({ "view": view, "selection": kept.selection, "cursor": kept.cursor })
}

/// What [`SELECTION_COMMAND`] does: the selection and cursor of the
/// view `view` (`main`: the main explorer's), or of every view.
pub fn selection_command(
    app: &AppHandle,
    library: &mut Library,
    args: &Value,
) -> Result<Value, String> {
    let main = crate::library::MAIN_GALLERY;
    let read =
        |library: &mut Library, gallery: &str| library.with_gallery(gallery, |lib| Kept::of(lib));
    let Some(view) = args.get("view").filter(|v| !v.is_null()) else {
        let mut views = vec![selection_of(main, &read(library, main))];
        for view in open_views(app) {
            views.push(selection_of(&view, &read(library, &view)));
        }
        return Ok(json!({ "views": views }));
    };
    let view = view.as_str().ok_or("`view`: a Library view's id")?;
    if view == main || view == RESULTS_PANEL {
        return Ok(selection_of(main, &read(library, main)));
    }
    if !open_views(app).iter().any(|v| v == view) {
        return Err(format!(
            "`view`: no Library view `{view}` is open (`main` or `{RESULTS_PANEL}#<n>`)"
        ));
    }
    Ok(selection_of(view, &read(library, view)))
}

/// The link toggle at the top of a Library view: the link icon, muted
/// unlinked and in the accent linked; named, and said pressed or not,
/// for screen readers. It runs [`LINK_COMMAND`].
fn link_toggle(ui: &mut Ui, id: &PanelId, linked: bool) {
    use egui_pop::core::icon::{IconWidget, Source as IconSource, Tone as IconTone};
    use helios_core::Message;
    let name = crate::i18n::text(
        ui.ctx(),
        Message::new("gui-library-view-link", "Follow the main view's selection"),
    );
    let (icon, tone) = if linked {
        ("phosphor:link", IconTone::Accent)
    } else {
        ("phosphor:link-break", IconTone::Muted)
    };
    let response = ui.add(
        IconWidget::new(IconSource::named(icon, "library bar"))
            .tone(tone)
            .label(&name)
            .selected(linked),
    );
    if response.clicked() {
        crate::dialogs::request(
            ui.ctx(),
            crate::dialogs::Request::Run(
                LINK_COMMAND.into(),
                json!({ "view": id.as_str(), "linked": !linked }),
                helios_shell::commands::Origin::User,
            ),
        );
    }
}

/// Space in a culling strip (Ctrl+Space toggles the selection, as in
/// the grid): the photo under its `cursor` added to the comparison, or
/// taken out when `compared` holds it, through the Compare commands.
fn space_to_compare(ui: &Ui, cursor: Option<String>, compared: &[String]) {
    let space = ui.input(|i| i.key_pressed(egui::Key::Space) && !i.modifiers.command);
    let Some(photo) = cursor.filter(|_| space) else {
        return;
    };
    let command = if compared.contains(&photo) {
        crate::comparison::REMOVE_COMMAND
    } else {
        crate::comparison::ADD_COMMAND
    };
    crate::dialogs::request(
        ui.ctx(),
        crate::dialogs::Request::Run(
            command.into(),
            json!({ "images": [photo] }),
            helios_shell::commands::Origin::User,
        ),
    );
}

/// What [`LINK_COMMAND`] does: the link kept in the view's payload.
pub fn link_command(app: &AppHandle, args: &Value) -> Result<Value, String> {
    let view = args
        .get("view")
        .and_then(Value::as_str)
        .ok_or("`view`: a Library view's id")?;
    if !open_views(app).iter().any(|v| v == view) {
        return Err(format!(
            "`view`: no Library view `{view}` is open (`{RESULTS_PANEL}#<n>`)"
        ));
    }
    let id = PanelId::new(view).map_err(|e| format!("`view`: {e}"))?;
    let mut payload = app.instance_payload(&id);
    if payload.get("quick").is_some_and(|q| !q.is_null()) {
        return Err(format!(
            "`view`: `{view}` shows a quick collection, which follows no other view"
        ));
    }
    let now = payload.get("link").and_then(Value::as_str) == Some(LINKED_TO);
    let linked = match args.get("linked") {
        None | Some(Value::Null) => !now,
        Some(Value::Bool(b)) => *b,
        Some(v) => return Err(format!("`linked`: {v} is not true or false")),
    };
    if !payload.is_object() {
        payload = json!({});
    }
    if let Some(o) = payload.as_object_mut() {
        if linked {
            o.insert("link".into(), json!(LINKED_TO));
        } else {
            o.remove("link");
        }
    }
    app.set_instance_payload(&id, payload)?;
    Ok(json!({ "view": view, "linked": linked }))
}

/// A quick-collection panel's head: the collection's name and how many
/// photos it holds, or that it was deleted. The name shown, for the tab.
fn quick_header(ui: &mut Ui, library: &Library, quick: &str) -> Option<String> {
    use egui_pop::core::ui::UiPop as _;
    use helios_core::Message;
    let ctx = ui.ctx().clone();
    let first = helios_vault::collections::QuickCollection {
        id: helios_vault::collections::QUICK.to_string(),
        name: None,
        images: Vec::new(),
    };
    let found = library
        .quick_collection(quick)
        .or((quick == helios_vault::collections::QUICK).then_some(&first));
    let Some(found) = found else {
        ui.muted(crate::i18n::text(
            &ctx,
            Message::new(
                "gui-library-quick-panel-gone",
                "This quick collection was deleted",
            ),
        ));
        return None;
    };
    let name = crate::library::quick_label(&ctx, found);
    let count = library.quick_count_of(quick);
    ui.horizontal(|ui| {
        ui.strong_text(format!("⚡ {name}"));
        ui.muted(crate::i18n::text(
            &ctx,
            Message::new("gui-library-quick-panel-count", "{ $count } photos").with("count", count),
        ));
    });
    Some(name)
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
            selection: vec!["a".into(), "b".into()],
            results: None,
            quick: None,
            link: Some(LINKED_TO.into()),
            strip: true,
        };
        let back: ViewState =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn a_lone_cursor_is_not_kept_as_a_selection_too() {
        let lone = Kept {
            selection: vec!["b".into()],
            cursor: Some("b".into()),
        };
        assert!(lone.selection_kept().is_empty());
        let several = Kept {
            selection: vec!["a".into(), "c".into()],
            cursor: Some("c".into()),
        };
        assert_eq!(several.selection_kept(), ["a", "c"]);
        let elsewhere = Kept {
            selection: vec!["a".into()],
            cursor: Some("c".into()),
        };
        assert_eq!(elsewhere.selection_kept(), ["a"]);
    }

    #[test]
    fn a_quick_collection_panel_s_state_is_scoped_to_it() {
        let state = ViewState::of_quick("picks");
        assert_eq!(state.filter.text, "collection:picks");
        let kept = serde_json::to_value(&state).unwrap();
        assert_eq!(kept["quick"], "picks");
        let back: ViewState = serde_json::from_value(kept).unwrap();
        assert_eq!(back, state);
    }
}
