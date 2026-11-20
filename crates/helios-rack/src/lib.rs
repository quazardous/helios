//! helios-rack: the frame of Helios's visual modules
//! (`docs/design/ui-responsibilities.md`, section 7).
//!
//! **It draws nothing.** A module declares its parameters (their
//! descriptors in the value store, the controls patched to them); the
//! rack resolves them as a card in the controls' context, `egui_hud` lays
//! the card out and chooses egui-pop's widgets for each part (sliders,
//! curves, colour wheels, folded lists, read-out lines, menus with
//! previews, text fields), and the rack binds what they change to the
//! value store, as the user:
//!
//! - a drag (or a scrub) is one gesture: one undo step, other origins
//!   kept off meanwhile; Escape gives it up without a trace;
//! - steps in a row (a held arrow, a turned wheel) are one undo step;
//! - a typed value past the soft range is forced up to the hard one, and
//!   said so (a note for the shell to show);
//! - an editor's value (a curve's points, a wheel's colour, a choice, a
//!   text) is written at the end of its gesture;
//! - each finished change runs egui-pop's `slider.change` step (or its
//!   `curve.change` / `wheel.change` one), then the `controls.change`
//!   step, so a plugin can veto, snap or follow it.
//!
//! What only the shell knows reaches the rack as data ([`Extra`]: a
//! choice's words and previews, read-out lines), and what the shell does
//! with the parts comes back as data ([`Output`]: touches for its names,
//! menus and clipboard, notes to show). The picker is a bus event.
//!
//! The rack also hosts the on-image tools ([`tools`]): a shape's handles
//! declared on `egui_hud`'s handle kinds ([`handle`]), each drag, double
//! click and wheel turn bound to the value store the same way.

pub mod tools;
mod words;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use egui_hud::render::{self, CardEvent, Drawings, Dressing, Edit, Editor, Folded, Marks};
use egui_pop::core::slider::{Change as SliderChange, Slider, Verdict};
use egui_pop::core::tooltip::TooltipExt as _;
use egui_pop::egui::{self, Ui};
use egui_pop::viewport::change::{Change as ToolChange, Verdict as ToolVerdict};
use egui_pop::viewport::wheel::WheelValue;
use helios_controls::commands::SharedContexts;
use helios_controls::context::CardBox;
use helios_controls::flow::{Cause, SharedSteps};
use helios_controls::model::ValueSpec;
use helios_controls::resolve::Values;
use helios_controls::{ContextKeys, Control, HeliosRegistry as _, Registry, Surface};
use helios_core::{Message, Value};
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, Entry, GestureId, Set, Store, Type};
use serde_json::json;

pub use egui_hud::handle;
pub use egui_hud::render::{Line, Preview};
pub use words::{
    choice, description, fixed, gesture_hint, label, locale, param_name, shown_in, text,
};

/// The journal event a parameter's picker button publishes (`path`): a
/// picker tool answers it by sampling the image.
pub const PICKER_EVENT: &str = "param.picker";

/// How long a change by someone else stays marked, in seconds.
const MOVED_SECONDS: f64 = 2.0;

/// Steps further apart than this, in seconds, are undo steps of their own.
const STEPS_GAP: f64 = 0.5;

/// How many values a line of an open list holds, when they are not
/// points.
const VALUES_PER_LINE: usize = 8;

/// What the shell adds to a parameter, as data: what the rack cannot
/// know of it (a mask group's operator by its effect, its members).
#[derive(Clone, Debug, Default)]
pub struct Extra {
    /// The words of each choice, by choice, instead of the parameter's
    /// own translations.
    pub choices: BTreeMap<String, String>,
    /// A preview before each choice, by choice: the part is a menu with
    /// previews.
    pub previews: BTreeMap<String, Preview>,
    /// Read-out lines instead of the value: the part only reads.
    pub lines: Option<Vec<Line>>,
}

/// A part the shell touches once drawn: its picture's name
/// (`param:<path>`), and, with `menu`, the Edit commands while it has the
/// focus and its context menu (on `menu_on` when not on itself).
#[derive(Debug)]
pub struct Touch {
    pub path: String,
    pub response: egui::Response,
    pub menu_on: Option<egui::Response>,
    pub menu: bool,
}

/// What a card asks of the shell after a frame.
#[derive(Debug, Default)]
pub struct Output {
    pub touches: Vec<Touch>,
    /// Pictures to name (a part's marks), by name.
    pub named: Vec<(String, egui::Rect)>,
    /// What to tell the person (a typed value held or forced).
    pub notes: Vec<Message>,
}

/// The rack's memory between frames: never values.
pub struct Rack {
    /// The sequence number of each path when last seen, and when another
    /// origin moved it.
    seen: BTreeMap<String, (u64, Option<f64>)>,
    gesture: Option<(String, GestureId)>,
    /// Steps in a row on one path (keys, the wheel): one group, and when
    /// its last step was.
    steps: Option<(String, GestureId, f64)>,
    /// The application: its bus (the picker, the change steps).
    app: Option<egui_pop::api::AppHandle>,
    /// The controls' context the cards resolve in, shared with the
    /// `controls.*` commands.
    contexts: SharedContexts,
    registry: Arc<Registry>,
    /// The `controls.change` step's hooks.
    hooks: SharedSteps,
    /// Text typed into a field, by path, until the field is left.
    typing: Option<(String, String)>,
}

/// The mask group parameter a path names (`operator`, `opacity`,
/// `invert`) and its group's node, when it is one.
pub fn group_param<'a>(d: &Descriptor, path: &'a str) -> Option<(&'a str, &'a str)> {
    if d.module.as_deref() != Some("mask_combine") {
        return None;
    }
    node_param(path)
}

/// The node a parameter's path is of, and the parameter's name.
pub fn node_param(path: &str) -> Option<(&str, &str)> {
    path.strip_prefix("node/")?.rsplit_once('/')
}

fn write(
    store: &mut Store,
    path: &str,
    change: Change,
    gesture: Option<GestureId>,
    force: bool,
) -> Option<helios_values::Applied> {
    let mut set = Set::new(path, change, Origin::User);
    set.gesture = gesture;
    set.force = force;
    store.set(set).ok()
}

/// The control a parameter is edited by: the one patched to it, else its
/// implicit one, its face chosen by what the shell adds (a menu with
/// previews, read-out lines) and by its value (a list with no editor
/// folds into a summary).
fn control_of(registry: &Registry, path: &str, d: &Descriptor, extra: Option<&Extra>) -> Control {
    if let Some(c) = registry.control_for(path, d) {
        return c.clone();
    }
    let mut c = helios_controls::implicit(path, d);
    let face = match extra {
        Some(e) if !e.previews.is_empty() && d.ty == Type::Choice => Some("preview-menu"),
        Some(e) if e.lines.is_some() && matches!(d.ty, Type::Floats | Type::Ints) => {
            Some("readout-lines")
        }
        _ if matches!(d.ty, Type::Floats | Type::Ints) && d.widget.is_none() => Some("folded-list"),
        _ => None,
    };
    if let Some(variant) = face
        .and_then(egui_hud::face::face)
        .map(|f| f.variant.clone())
    {
        let mut variant = variant;
        variant.chip = false;
        c.faces.retain(|v| v.id != variant.id);
        c.faces.insert(0, variant);
    }
    c
}

/// One parameter of a card, as this frame found it.
struct Row {
    path: String,
    entry: Entry,
    /// Held by another origin's gesture.
    held: Option<Origin>,
    /// Just moved by another origin.
    moved: Option<Origin>,
    extra: Option<Extra>,
}

fn origin_name(o: &Origin) -> String {
    match o {
        Origin::User => "you".into(),
        Origin::Agent(id) => format!("agent {id}"),
        Origin::Script(id) => format!("script {id}"),
        Origin::Midi => "a MIDI controller".into(),
        Origin::Remote(id) => format!("remote {id}"),
    }
}

/// What a list's summary counts, and how many values a line of it holds
/// open: a path's or a brush's points (their values each), else values.
fn list_items(d: &Descriptor, path: &str, len: usize) -> (Message, usize) {
    use helios_module_masks::shape::{BRUSH_POINT_VALUES, PATH_POINT_VALUES};
    let per_point = match (d.module.as_deref(), param_name(path)) {
        (Some("mask_path"), "points") => Some(PATH_POINT_VALUES),
        (Some("mask_brush"), "points") => Some(BRUSH_POINT_VALUES),
        _ => None,
    };
    match per_point {
        Some(n) => (
            Message::new("gui-params-points", "{ $count } points").with("count", len / n),
            n,
        ),
        None => (
            Message::new("gui-params-values", "{ $count } values").with("count", len),
            VALUES_PER_LINE,
        ),
    }
}

/// A list folded: its summary (`12 points`), its values a line each (a
/// point, numbered from 1), in the language of `ctx`.
fn folded(ctx: &egui::Context, path: &str, d: &Descriptor, v: &Value) -> Folded {
    let locale = locale(ctx);
    let (values, fraction): (Vec<f64>, usize) = match v {
        Value::List(l) => (l.clone(), d.precision as usize),
        Value::Ints(l) => (l.iter().map(|&x| x as f64).collect(), 0),
        _ => (Vec::new(), 0),
    };
    let (summary, per_line) = list_items(d, path, values.len());
    let lines = values
        .chunks(per_line.max(1))
        .enumerate()
        .map(|(i, line)| {
            let line: Vec<String> = line.iter().map(|x| fixed(&locale, *x, fraction)).collect();
            format!("{}: {}", i + 1, line.join("  "))
        })
        .collect();
    Folded {
        summary: text(ctx, summary),
        lines,
        show: text(
            ctx,
            Message::new("gui-params-show-values", "Show the values"),
        ),
        hide: text(
            ctx,
            Message::new("gui-params-hide-values", "Hide the values"),
        ),
    }
}

/// The card's host: Helios's words, verdicts and marks on what `egui_hud`
/// draws, and what the shell is to touch. No drawing of its own.
struct Host<'r> {
    app: Option<&'r egui_pop::api::AppHandle>,
    hooks: &'r SharedSteps,
    /// By the card's key (the parameter's name).
    rows: &'r BTreeMap<String, Row>,
    out: &'r mut Output,
}

impl Host<'_> {
    /// A tool's change through egui-pop's step for it (`curve.change`,
    /// `wheel.change`), on the application's bus.
    fn tool<T: Clone>(
        &self,
        key: &str,
        change: &ToolChange<T>,
        step: impl Fn(&egui_pop::api::AppHandle, &str, &ToolChange<T>) -> ToolVerdict<T>,
    ) -> ToolVerdict<T> {
        match (self.app, self.rows.get(key)) {
            (Some(app), Some(row)) => step(app, &row.path, change),
            _ => ToolVerdict::Allow,
        }
    }
}

impl Drawings for Host<'_> {
    fn draw(
        &mut self,
        _: &mut Ui,
        _: &str,
        _: &Control,
        _: egui::Rect,
        _: &mut helios_controls::value::Value,
    ) -> Option<Vec<Edit>> {
        None
    }

    fn label(&mut self, ctx: &egui::Context, key: &str, _: &Control) -> Option<String> {
        let row = self.rows.get(key)?;
        Some(label(ctx, &row.path, &row.entry.descriptor))
    }

    fn choice(&mut self, ctx: &egui::Context, key: &str, _: &Control, c: &str) -> Option<String> {
        let row = self.rows.get(key)?;
        if let Some(w) = row.extra.as_ref().and_then(|e| e.choices.get(c)) {
            return Some(w.clone());
        }
        Some(choice(ctx, &row.path, &row.entry.descriptor, c))
    }

    fn slider<'a>(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        _: &Control,
        slider: Slider<'a>,
    ) -> Slider<'a> {
        let Some(row) = self.rows.get(key) else {
            return slider;
        };
        let d = &row.entry.descriptor;
        if !d.picker {
            return slider;
        }
        let picker = text(
            ctx,
            Message::new("gui-param-picker", "Pick { $label } from the image")
                .with("label", label(ctx, &row.path, d)),
        );
        slider.quad(egui_pop::core::icon::Icon::Picker, &picker)
    }

    /// A parameter with Helios's own context menu (its Reset, Copy,
    /// Paste…) keeps egui-pop's value menu off, so a right click opens
    /// that menu alone.
    fn value_menu(&mut self, key: &str, _: &Control) -> bool {
        !(self.rows.contains_key(key) && self.app.is_some())
    }

    fn verdict(&mut self, key: &str, control: &Control, c: &SliderChange) -> Verdict {
        let Some(row) = self.rows.get(key) else {
            return Verdict::Go;
        };
        control_verdict(control, &row.path, self.app, self.hooks, c)
    }

    fn marks(&mut self, key: &str) -> Option<Marks> {
        let row = self.rows.get(key)?;
        Some(Marks {
            changed: row.entry.value != row.entry.descriptor.default,
            held: row.held.as_ref().map(origin_name),
            moved: row.moved.as_ref().map(origin_name),
        })
    }

    fn editor(&mut self, key: &str, _: &Control) -> Editor {
        let Some(row) = self.rows.get(key) else {
            return Editor::default();
        };
        let d = &row.entry.descriptor;
        Editor {
            default: Some(helios_controls::values::value(&d.default)),
            max_points: d.count.map(|n| n as usize / 2),
        }
    }

    fn curve_change(
        &mut self,
        key: &str,
        _: &Control,
        change: &ToolChange<Vec<[f32; 2]>>,
    ) -> ToolVerdict<Vec<[f32; 2]>> {
        self.tool(key, change, |app, path, c| {
            egui_pop::imaging::curve(app, path)(c)
        })
    }

    fn wheel_change(
        &mut self,
        key: &str,
        _: &Control,
        change: &ToolChange<WheelValue>,
    ) -> ToolVerdict<WheelValue> {
        self.tool(key, change, |app, path, c| {
            egui_pop::imaging::wheel(app, path)(c)
        })
    }

    fn folded(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        _: &Control,
        _: &helios_controls::value::Value,
    ) -> Option<Folded> {
        let row = self.rows.get(key)?;
        Some(folded(
            ctx,
            &row.path,
            &row.entry.descriptor,
            &row.entry.value,
        ))
    }

    fn lines(
        &mut self,
        _: &egui::Context,
        key: &str,
        _: &Control,
        _: &helios_controls::value::Value,
    ) -> Option<Vec<Line>> {
        self.rows.get(key)?.extra.as_ref()?.lines.clone()
    }

    fn preview(
        &mut self,
        _: &egui::Context,
        key: &str,
        _: &Control,
        choice: &str,
    ) -> Option<Preview> {
        self.rows
            .get(key)?
            .extra
            .as_ref()?
            .previews
            .get(choice)
            .cloned()
    }

    fn dress(&mut self, ui: &mut Ui, dressing: Dressing<'_>) {
        let Some(row) = self.rows.get(dressing.key) else {
            return;
        };
        let ctx = ui.ctx().clone();
        let path = row.path.clone();
        if let Some(at) = dressing.changed {
            self.out.named.push((format!("param:{path}:changed"), at));
        }
        if let Some(at) = dressing.moved {
            self.out.named.push((format!("param:{path}:moved"), at));
        }
        let Some(response) = dressing.response else {
            return;
        };
        declare_edit(ui, &path, response);
        // A list's own response keeps its popup: a context menu on it
        // shares the popup's id and closes the list on the click that
        // opens it. Its label takes the menu; a list with none, none.
        let is_list = matches!(dressing.control.value, ValueSpec::OneOf { .. });
        self.out.touches.push(Touch {
            path: path.clone(),
            response: response.clone(),
            menu_on: dressing.label.cloned(),
            menu: !(is_list && dressing.label.is_none()),
        });
        if dressing.control.value.number().is_some() {
            let d = &row.entry.descriptor;
            let shown_label = label(&ctx, &path, d);
            let about = description(&ctx, &path, d);
            let _ = response.clone().tip(format!(
                "{}\n{}\n{}",
                if about.is_empty() {
                    &shown_label
                } else {
                    &about
                },
                text(
                    &ctx,
                    Message::new("gui-param-default", "{ $path } · default { $value }")
                        .with("path", path.as_str())
                        .with("value", shown_in(&locale(&ctx), d, &d.default)),
                ),
                gesture_hint(&ctx),
            ));
        }
    }
}

/// Declares a parameter's control to egui-pop's area registry as an edit
/// area writing `path` (`docs/design/panel-kinds.md`): agents and tests
/// read it on the bus (`pop.ui.areas`). No control name is given, so the
/// framework draws no hover card over the row's own hover text.
fn declare_edit(ui: &Ui, path: &str, response: &egui::Response) {
    use egui_pop::core::areas::{self, AreaKind, AreaSpec};
    areas::register(
        ui,
        response,
        AreaSpec::new(format!("param:{path}"), AreaKind::Edit).path(path),
    );
}

/// What a slider's change becomes, once its control's detents held it:
/// egui-pop's `slider.change` step (`app`'s), then, once done, the
/// `controls.change` step; a veto or a redirect of either wins.
fn control_verdict(
    control: &Control,
    path: &str,
    app: Option<&egui_pop::api::AppHandle>,
    hooks: &SharedSteps,
    c: &SliderChange,
) -> Verdict {
    let mut change = *c;
    if let Some(app) = app {
        let mut hook = egui_pop::api::sliders::hook(app, path);
        match hook(&change) {
            Verdict::Veto => return Verdict::Veto,
            Verdict::Redirect(v) => change.to = v,
            Verdict::Go => {}
        }
    }
    if change.done {
        let event = helios_controls::flow::ChangeEvent {
            control: control.id.clone(),
            node: helios_controls::flow::node_of(path),
            path: path.into(),
            from: c.from,
            to: change.to,
            cause: render::cause_of(c.cause),
            device: "pointer".into(),
        };
        let steps = hooks.lock().unwrap_or_else(PoisonError::into_inner);
        match steps.run(&event) {
            helios_controls::flow::Verdict::Veto => return Verdict::Veto,
            helios_controls::flow::Verdict::Redirect(v) => change.to = v,
            helios_controls::flow::Verdict::Go => {}
        }
    }
    if change.to == c.to {
        Verdict::Go
    } else {
        Verdict::Redirect(change.to)
    }
}

impl Rack {
    /// A rack resolving cards in `contexts` (the ones the `controls.*`
    /// commands share), with the controls of `registry`, each finished
    /// change through `hooks` (the `controls.change` step).
    pub fn new(contexts: SharedContexts, registry: Arc<Registry>, hooks: SharedSteps) -> Self {
        Self {
            seen: BTreeMap::new(),
            gesture: None,
            steps: None,
            app: None,
            contexts,
            registry,
            hooks,
            typing: None,
        }
    }

    /// The controls' context the cards resolve in.
    pub fn contexts(&self) -> &SharedContexts {
        &self.contexts
    }

    /// The application whose bus the rack speaks on.
    pub fn set_app(&mut self, app: egui_pop::api::AppHandle) {
        self.app = Some(app);
    }

    /// The application, when set.
    pub fn app(&self) -> Option<&egui_pop::api::AppHandle> {
        self.app.as_ref()
    }

    /// A step of a slider (an arrow, a wheel notch), in the group of the
    /// steps just before it on the same path.
    fn step(&mut self, s: &mut Store, path: &str, change: Change, label: &str, now: f64) {
        let group = match &mut self.steps {
            Some((p, g, last)) if p == path && now - *last <= STEPS_GAP => {
                *last = now;
                *g
            }
            _ => {
                if let Some((_, g, _)) = self.steps.take() {
                    s.end(g);
                }
                let g = s.begin_group(Origin::User, &format!("step {label}"));
                self.steps = Some((path.into(), g, now));
                g
            }
        };
        write(s, path, change, Some(group), false);
    }

    /// Ends a row of steps once it has paused.
    pub fn end_steps(&mut self, store: &Mutex<Store>, now: f64) {
        if self
            .steps
            .as_ref()
            .is_some_and(|(_, _, last)| now - last > STEPS_GAP)
            && let Some((_, g, _)) = self.steps.take()
        {
            store.lock().expect("value store lock").end(g);
        }
    }

    /// The marks of a path this frame: who moved it, if another origin
    /// did just now.
    fn moved(&mut self, path: &str, e: &Entry, now: f64) -> Option<Origin> {
        let seen = self.seen.entry(path.into()).or_insert((e.seq, None));
        if seen.0 != e.seq {
            let by_other = e.origin.as_ref().is_some_and(|o| *o != Origin::User);
            seen.0 = e.seq;
            seen.1 = by_other.then_some(now);
        }
        match seen.1 {
            Some(at) if now - at < MOVED_SECONDS => e.origin.clone(),
            _ => None,
        }
    }

    /// The context keys a card resolves in, its box the width available
    /// at the density. The panel's own (`shared`) also sets the shared
    /// box and the hand (a gesture open, the pointer over it), so the
    /// `controls.*` commands see what it shows.
    pub fn keys(&self, ui: &Ui, shared: bool) -> ContextKeys {
        let mut c = self.contexts.lock().unwrap_or_else(PoisonError::into_inner);
        let card_box = CardBox::from_points(
            Surface::Panel,
            ui.available_width(),
            ui.clip_rect().height(),
            c.keys.density,
        );
        if !shared {
            return c.keys.clone().with_box(card_box);
        }
        c.keys.set_box(card_box);
        c.hand.gesture_open = self.gesture.is_some();
        c.hand.pointer_in_panel = ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|p| ui.clip_rect().contains(p));
        c.keys.clone()
    }

    /// One card: the parameters `paths` of `node` through the controls
    /// layer, resolved in `keys` (the shared reveals and hand, `id`'s
    /// previous resolution), drawn by `egui_hud`, their changes written to
    /// the store as the user. `extras` holds what the shell adds to a
    /// parameter, by path.
    #[expect(
        clippy::too_many_arguments,
        reason = "the card's node, id, parameters, extras and keys, and where to draw"
    )]
    pub fn card(
        &mut self,
        ui: &mut Ui,
        store: &Mutex<Store>,
        node: &str,
        id: &str,
        paths: &[&str],
        extras: &BTreeMap<String, Extra>,
        keys: ContextKeys,
    ) -> Output {
        let now = ui.input(|i| i.time);
        let ctx = ui.ctx().clone();
        let global = Arc::clone(&self.registry);
        let mut registry = (*global).clone();
        let mut items = Vec::new();
        let mut values = Values::new();
        let mut shown = BTreeMap::new();
        let mut rows = BTreeMap::new();
        {
            let s = store.lock().expect("value store lock");
            for path in paths {
                let Some(entry) = s.get(path).cloned() else {
                    continue;
                };
                let Some((_, name)) = node_param(path) else {
                    continue;
                };
                let held = s.held_by(path).filter(|o| **o != Origin::User).cloned();
                let extra = extras.get(*path).cloned();
                let control = control_of(&global, path, &entry.descriptor, extra.as_ref());
                let cid = control.id.clone();
                if registry.get(&cid).is_none() && registry.register(control).is_err() {
                    continue;
                }
                items.push(helios_controls::card::CardItem::control(name, &cid));
                values.insert(
                    name.to_owned(),
                    helios_controls::values::value(&entry.value).to_json(),
                );
                let mut v = helios_controls::values::value(&entry.value);
                if let (Some((p, t)), Value::Text(_)) = (&self.typing, &entry.value)
                    && p == path
                {
                    v = helios_controls::value::Value::Text(t.clone());
                }
                shown.insert(name.to_owned(), v);
                rows.insert(
                    name.to_owned(),
                    Row {
                        path: (*path).to_owned(),
                        entry,
                        held,
                        moved: None,
                        extra,
                    },
                );
            }
        }
        for row in rows.values_mut() {
            row.moved = self.moved(&row.path, &row.entry, now);
            if row.moved.is_some() {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(MOVED_SECONDS));
            }
        }
        let mut out = Output::default();
        if items.is_empty() {
            return out;
        }
        let card = helios_controls::card::Card::new(items);
        let placement = {
            let mut c = self.contexts.lock().unwrap_or_else(PoisonError::into_inner);
            let moment = c.moment(node);
            let resolved = helios_controls::resolve::resolve(
                &registry,
                &card,
                &keys,
                &c.layouts,
                &values,
                &moment,
                c.previous.get(id),
            );
            match resolved {
                Ok(p) => {
                    c.previous.insert(id.to_owned(), p.clone());
                    p
                }
                Err(e) => {
                    drop(c);
                    render::error_line(ui, &e);
                    return out;
                }
            }
        };
        let mut host = Host {
            app: self.app.as_ref(),
            hooks: &self.hooks,
            rows: &rows,
            out: &mut out,
        };
        let card_out = ui
            .push_id(("helios.params.card", id), |ui| {
                render::card(ui, &registry, &placement, &mut shown, &mut host)
            })
            .inner;
        self.apply_card(&ctx, store, node, &rows, card_out, &mut out);
        out
    }

    /// Writes what a card changed this frame, as the user, and runs its
    /// requests (a reset, a picker, a reveal).
    fn apply_card(
        &mut self,
        ctx: &egui::Context,
        store: &Mutex<Store>,
        node: &str,
        rows: &BTreeMap<String, Row>,
        card: render::CardOutput,
        out: &mut Output,
    ) {
        let mut numbers: BTreeMap<&str, Vec<render::Change>> = BTreeMap::new();
        for e in &card.edits {
            let Some(row) = rows.get(e.key()) else {
                continue;
            };
            match e {
                Edit::Number { key, change } => {
                    numbers.entry(key.as_str()).or_default().push(*change);
                }
                // Typing is kept here until the field is left, then
                // written as the user.
                Edit::Set {
                    to: helios_controls::value::Value::Text(t),
                    done,
                    ..
                } => {
                    if *done {
                        if self.typing.as_ref().is_some_and(|(p, _)| *p == row.path) {
                            self.typing = None;
                        }
                        if row.entry.value != Value::Text(t.clone()) {
                            let mut s = store.lock().expect("value store lock");
                            write(
                                &mut s,
                                &row.path,
                                Change::To(Value::Text(t.clone())),
                                None,
                                false,
                            );
                        }
                    } else {
                        self.typing = Some((row.path.clone(), t.clone()));
                    }
                }
                // A set value (a choice) is written once done.
                Edit::Set { to, done, .. } => {
                    let to = helios_controls::values::helios_value(to.clone());
                    if *done {
                        let mut s = store.lock().expect("value store lock");
                        write(&mut s, &row.path, Change::To(to), None, false);
                    }
                }
                // An editor's change (a curve's points, a wheel's colour):
                // a drag's frames as one gesture that its end closes (one
                // undo step), so the editor follows the drag.
                Edit::Tool {
                    to, cause, done, ..
                } => {
                    let to = helios_controls::values::helios_value(to.clone());
                    let d = row.entry.descriptor.clone();
                    self.tool_edit(ctx, store, &row.path, &d, to, *cause, *done);
                }
                // A drag given up: what it wrote is put back, no trace.
                Edit::Cancel { .. } => {
                    if let Some((p, g)) = self.gesture.take() {
                        if p == row.path {
                            store.lock().expect("value store lock").cancel(g);
                        } else {
                            self.gesture = Some((p, g));
                        }
                    }
                }
            }
        }
        for (key, changes) in numbers {
            let row = &rows[key];
            let d = &row.entry.descriptor;
            let shown_label = label(ctx, &row.path, d);
            self.apply(ctx, store, &row.path, d, &shown_label, &changes, out);
        }
        for e in card.events {
            match e {
                CardEvent::Reset(key) => {
                    if let Some(row) = rows.get(&key) {
                        let mut s = store.lock().expect("value store lock");
                        write(&mut s, &row.path, Change::Reset, None, false);
                    }
                }
                CardEvent::Quad(key) => {
                    if let (Some(row), Some(app)) = (rows.get(&key), &self.app) {
                        app.publish(
                            "helios.params",
                            None,
                            PICKER_EVENT,
                            json!({ "path": row.path }),
                        );
                    }
                }
                CardEvent::Reveal(key) => {
                    let mut c = self.contexts.lock().unwrap_or_else(PoisonError::into_inner);
                    c.moments
                        .entry(node.to_owned())
                        .or_default()
                        .revealed
                        .insert(key);
                }
                CardEvent::RevealAll => {
                    let mut c = self.contexts.lock().unwrap_or_else(PoisonError::into_inner);
                    c.moments.entry(node.to_owned()).or_default().reveal_all = true;
                }
            }
        }
    }

    /// Writes one change of an editor (a curve, a wheel), as the user: a
    /// drag's frames as one gesture ended by its release, other origins
    /// kept off meanwhile; keys in a row as one group; any other finished
    /// change as a step of its own.
    #[expect(
        clippy::too_many_arguments,
        reason = "the path, its descriptor, the value, its cause and whether it is done"
    )]
    fn tool_edit(
        &mut self,
        ctx: &egui::Context,
        store: &Mutex<Store>,
        path: &str,
        d: &Descriptor,
        v: Value,
        cause: egui_pop::viewport::change::Cause,
        done: bool,
    ) {
        use egui_pop::viewport::change::Cause as ToolCause;
        let now = ctx.input(|i| i.time);
        let mut s = store.lock().expect("value store lock");
        let held = self
            .gesture
            .as_ref()
            .filter(|(p, _)| p == path)
            .map(|(_, g)| *g);
        if !done {
            let g = held.unwrap_or_else(|| {
                let g = s.begin_gesture(Origin::User, &format!("drag {}", d.label));
                self.gesture = Some((path.into(), g));
                g
            });
            write(&mut s, path, Change::To(v), Some(g), false);
        } else if let Some(g) = held {
            self.gesture = None;
            write(&mut s, path, Change::To(v), Some(g), false);
            s.end(g);
        } else if matches!(cause, ToolCause::Key | ToolCause::Access) {
            self.step(&mut s, path, Change::To(v), &d.label, now);
        } else {
            write(&mut s, path, Change::To(v), None, false);
        }
    }

    /// Writes a number's changes this frame to the store, as the user: a
    /// drag's (or a scrub's) frames as one gesture ended by its release,
    /// steps in a row as one group, a typed value forced past the soft
    /// range (and said so), a drag given up leaving no trace.
    #[expect(
        clippy::too_many_arguments,
        reason = "the path, its descriptor and label, the changes, and the notes out"
    )]
    fn apply(
        &mut self,
        ctx: &egui::Context,
        store: &Mutex<Store>,
        path: &str,
        d: &Descriptor,
        shown_label: &str,
        changes: &[render::Change],
        out: &mut Output,
    ) {
        let value = |x: f64| match d.ty {
            Type::Int => Value::Int(x.round() as i64),
            _ => Value::Float(x),
        };
        let (lo, hi) = d.soft.or(d.hard).unwrap_or((f64::MIN, f64::MAX));
        let now = ctx.input(|i| i.time);
        let mut s = store.lock().expect("value store lock");
        let held = |g: &Option<(String, GestureId)>| {
            g.as_ref().filter(|(p, _)| p == path).map(|(_, g)| *g)
        };
        for c in changes {
            match c.cause {
                // A drag given up (Escape, or vetoed at its release): what
                // it wrote is put back and leaves no undo step.
                Cause::Drag | Cause::Scrub if c.cancelled => {
                    if let Some(g) = held(&self.gesture) {
                        self.gesture = None;
                        s.cancel(g);
                    }
                }
                // A drag's frames: one gesture, other origins kept off.
                Cause::Drag | Cause::Scrub if !c.done => {
                    let g = held(&self.gesture).unwrap_or_else(|| {
                        let g = s.begin_gesture(Origin::User, &format!("drag {}", d.label));
                        self.gesture = Some((path.into(), g));
                        g
                    });
                    write(&mut s, path, Change::To(value(c.to)), Some(g), false);
                }
                // Its release, as the step left it: one undo step.
                Cause::Drag | Cause::Scrub => match held(&self.gesture) {
                    Some(g) => {
                        self.gesture = None;
                        write(&mut s, path, Change::To(value(c.to)), Some(g), false);
                        s.end(g);
                    }
                    None => {
                        write(&mut s, path, Change::To(value(c.to)), None, false);
                    }
                },
                Cause::Key | Cause::Wheel | Cause::Access | Cause::Nudge => {
                    self.step(&mut s, path, Change::To(value(c.to)), &d.label, now);
                }
                Cause::Reset => {
                    write(&mut s, path, Change::Reset, None, false);
                }
                Cause::Set => {
                    write(&mut s, path, Change::To(value(c.to)), None, false);
                }
                // Typed past the soft range: forced, up to the hard one.
                Cause::Typed => {
                    let force = c.to < lo || c.to > hi;
                    let applied = write(&mut s, path, Change::To(value(c.to)), None, force);
                    let note = applied.and_then(|a| match (force, a.clamped) {
                        (_, helios_values::Clamped::Hard) => Some(
                            Message::new("gui-param-held", "{ $label }: held to { $value }")
                                .with("label", shown_label)
                                .with("value", shown_in(&locale(ctx), d, &a.value)),
                        ),
                        (true, _) => Some(
                            Message::new(
                                "gui-param-past-range",
                                "{ $label }: past its usual range",
                            )
                            .with("label", shown_label),
                        ),
                        _ => None,
                    });
                    out.notes.extend(note);
                }
            }
        }
    }
}
