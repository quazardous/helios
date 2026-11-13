//! Parameter widgets, generated from value descriptors and bound to the
//! value store (see `docs/widgets.md`). A widget never holds its value: it
//! reads the store every frame and writes through it, as the user.
//!
//! - **Through the controls layer** (`helios_controls`,
//!   `docs/design/controls.md`). A node's parameters are a card: each
//!   parameter has the control patched to it (exposure's `exposure` and
//!   `black`), else its implicit control, built from its descriptor. The
//!   card is resolved in the controls' context (the panel's box and the
//!   density, shared with the `controls.*` commands), laid out by the slot
//!   engine and drawn by its managed renderers; what the framework cannot
//!   draw (a curve, a colour wheel, a list, a text field, a mask group's
//!   operator) is drawn here, as an integrated part.
//! - A float or an int is egui-pop's slider with its control's steps and
//!   detents: a drag is one gesture (one undo step, agents kept off
//!   meanwhile; Ctrl finer, Shift coarser, Escape gives it up), the arrows
//!   step, a double click resets, Enter types a value or an expression
//!   (units allowed, `,` or `.`); a typed value past the soft range is
//!   forced up to the hard one, and says so. The wheel scrolls the panel:
//!   a slider takes it only with the focus or while Alt is held. Steps in
//!   a row (a held arrow, a turned wheel) are one undo step. Each finished
//!   change runs egui-pop's `slider.change` step, then the
//!   `controls.change` step, so a plugin can veto, snap or follow it.
//! - A bool is a toggle, a choice a list (the arrows step it), a text a
//!   field. A list of numbers folds into a summary (`12 points` for a
//!   path's points, else `32 values`) that opens under the card, a point
//!   a line, so a long list never pushes the next rows out of view.
//! - A mask group's operator is named by its effect (union, intersection,
//!   …), each with the Masks panel's preview; its per-port lists
//!   (opacity, invert) read a line per wired port under the card, the
//!   member's name and its value in words ("spot: opacity 30 %").
//! - Marks, in the gutters of each part: changed from its default (a dot
//!   that resets), held by someone else's gesture, just moved by another
//!   origin (who, in the tooltip).
//! - The widget's id comes from the path, and so does its picture's name
//!   (`param:<path>`); the context menu resets, copies the path or the
//!   value, and pastes: commands, like everything else. While a widget
//!   has the focus, Edit › Copy and Paste (Ctrl+C, Ctrl+V) copy its
//!   parameter and paste onto it (`crate::clipboard::param`).

use std::collections::BTreeMap;
use std::sync::Mutex;

use egui::{Sense, Ui};
use egui_pop::api::sliders;
use egui_pop::core::icon::{Icon, Tone};
use egui_pop::core::slider::{Change as SliderChange, Slider, Verdict};
use egui_pop::core::ui::UiPop as _;
use egui_pop::viewport::change::{Cause as ToolCause, Change as ToolChange};
use egui_pop::viewport::curve::CurveEditor;
use egui_pop::viewport::wheel::{ColorWheel, WheelValue};
use helios_controls::commands::SharedContexts;
use helios_controls::context::CardBox;
use helios_controls::flow::Cause;
use helios_controls::render::{self, CardEvent, Drawings, Dressing, Edit, Marks};
use helios_controls::resolve::Values;
use helios_controls::slot::{Fallback, Integration};
use helios_controls::{ContextKeys, Control, HeliosRegistry as _, Registry, Surface};
use helios_core::{Message, ParamWidget, Value};
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, Entry, GestureId, Set, Store, Type};
use serde_json::json;

use crate::dialogs::{Level, Request, request};

/// The panel.
pub const PANEL: &str = "helios.params";

/// The node whose parameters the panel shows (a node name; empty or absent:
/// every node). The graph editor sets it.
pub const SELECTION: &str = "view/graph/selection";

/// How long a change by someone else stays marked, in seconds.
const MOVED_SECONDS: f64 = 2.0;

/// Steps further apart than this, in seconds, are undo steps of their own.
const STEPS_GAP: f64 = 0.5;

/// Each mask group's members by port, by the group's node name: what its
/// per-port lists are about (see [`crate::mask_tree::members_by_port`]).
pub type Members = BTreeMap<String, Vec<Option<String>>>;

/// The mask group parameter a path names (`operator`, `opacity`,
/// `invert`) and its group's node, when it is one.
fn group_param<'a>(d: &Descriptor, path: &'a str) -> Option<(&'a str, &'a str)> {
    if d.module.as_deref() != Some("mask_combine") {
        return None;
    }
    let (node, name) = path.strip_prefix("node/")?.rsplit_once('/')?;
    Some((node, name))
}

/// What the widgets remember between frames: never values.
#[derive(Default)]
pub struct Params {
    /// The sequence number of each path when last seen, and when another
    /// origin moved it.
    seen: BTreeMap<String, (u64, Option<f64>)>,
    gesture: Option<(String, GestureId)>,
    /// Steps in a row on one path (keys, the wheel): one group, and when
    /// its last step was.
    steps: Option<(String, GestureId, f64)>,
    /// The application, for the parameters' context menu.
    app: Option<egui_pop::api::AppHandle>,
    /// The controls' context the cards resolve in, shared with the
    /// `controls.*` commands (`controls.context` reads the box the panel
    /// set).
    contexts: SharedContexts,
    /// Text typed into a field, by path, until the field is left.
    typing: Option<(String, String)>,
}

/// The journal event a parameter's picker button publishes (`path`): a
/// picker tool answers it by sampling the image.
pub const PICKER_EVENT: &str = "param.picker";

/// Parses a typed number or expression as the sliders do: units after it
/// are ignored, a comma is a decimal separator.
pub fn parse_typed(text: &str) -> Option<f64> {
    egui_pop::core::slider::parse_typed(text)
}

/// A value as shown in English: display scale, precision, unit.
pub fn shown(d: &Descriptor, v: &Value) -> String {
    shown_in("en", d, v)
}

/// A value as shown in a language: display scale, precision with the
/// language's decimal separator, unit. Exposures are signed (`+0.70 EV`),
/// as darktable shows them.
pub fn shown_in(locale: &str, d: &Descriptor, v: &Value) -> String {
    let with_unit = |s: String| match d.unit.as_deref() {
        Some("%") => {
            let gap = egui_pop::i18n::format::separators(locale).percent;
            format!("{s}{}%", if gap.is_empty() { " " } else { gap })
        }
        Some(u) => format!("{s} {u}"),
        None => s,
    };
    match v {
        Value::Float(x) => {
            let x = x * d.display_scale;
            let s = crate::i18n::fixed(locale, x, d.precision as usize);
            let signed = d.unit.as_deref() == Some("EV") && !s.starts_with('-') && x > 0.0;
            with_unit(if signed { format!("+{s}") } else { s })
        }
        Value::Int(x) => with_unit(x.to_string()),
        Value::Bool(b) => if *b { "on" } else { "off" }.into(),
        Value::Choice(c) | Value::Text(c) => c.clone(),
        Value::List(l) => format!("{l:?}"),
        Value::Ints(l) => format!("{l:?}"),
    }
}

/// A node parameter's text in the language of `ctx`: the message `key`
/// (derived from its module and name), else its English.
fn translated(ctx: &egui::Context, key: Option<helios_i18n::MessageId>, english: &str) -> String {
    key.and_then(|k| crate::i18n::of(ctx)?.get(&k.key(), &[]))
        .unwrap_or_else(|| english.to_string())
}

/// The parameter's name: the last part of its path.
fn param_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// A parameter's label in the language of `ctx` (`param-<module>-<name>`'s
/// `.label`), else the descriptor's English.
pub fn label(ctx: &egui::Context, path: &str, d: &Descriptor) -> String {
    let key = d
        .module
        .as_deref()
        .map(|m| helios_i18n::ids::param_label(m, param_name(path)));
    translated(ctx, key, &d.label)
}

/// A parameter's description in the language of `ctx`, else its English.
pub fn description(ctx: &egui::Context, path: &str, d: &Descriptor) -> String {
    let key = d
        .module
        .as_deref()
        .map(|m| helios_i18n::ids::param_description(m, param_name(path)));
    translated(ctx, key, &d.description)
}

/// One value of a choice parameter in the language of `ctx`, else as it is.
pub fn choice(ctx: &egui::Context, path: &str, d: &Descriptor, value: &str) -> String {
    let key = d
        .module
        .as_deref()
        .map(|m| helios_i18n::ids::param_choice(m, param_name(path), value));
    translated(ctx, key, value)
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

/// The node a parameter's path is of, and the parameter's name.
fn node_param(path: &str) -> Option<(&str, &str)> {
    path.strip_prefix("node/")?.rsplit_once('/')
}

/// The control a parameter is edited by: the one patched to it, else its
/// implicit one. A parameter this panel draws itself (a text, a list
/// with no editor, a mask group's operator) has its implicit control
/// integrated on the panel, its value type's rows as its fallback.
fn control_of(registry: &Registry, path: &str, d: &Descriptor, operator: bool) -> Control {
    if let Some(c) = registry.control_for(path, d) {
        return c.clone();
    }
    let mut c = helios_controls::implicit(path, d);
    let own = operator
        || d.ty == Type::Text
        || (matches!(d.ty, Type::Floats | Type::Ints) && d.widget.is_none());
    if own {
        let integrated = Integration::integrated(Fallback::ValueType);
        for v in &mut c.faces {
            *v = v
                .clone()
                .integrated(Surface::Panel, integrated.clone())
                .integrated(Surface::Dock, integrated.clone())
                .integrated(Surface::Float, integrated.clone());
        }
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
    /// A mask group's per-port lines, for its per-port lists.
    ports: Option<Vec<(String, String)>>,
    /// A mask group's operator.
    operator: bool,
}

/// What goes under a card once it is drawn.
enum Below {
    /// An open list's values, so many per line.
    Values(Box<Descriptor>, Value, usize),
    /// A mask group's per-port lines.
    Ports(Vec<(String, String)>),
}

/// The card's host: Helios's labels, choices, picker, hooks, marks and
/// touches on what the framework draws, and its own drawings.
struct Host<'p> {
    params: &'p mut Params,
    store: &'p Mutex<Store>,
    /// By the card's key (the parameter's name).
    rows: BTreeMap<String, Row>,
    below: Vec<Below>,
}

impl Host<'_> {
    /// A widget's touches: its picture's name, the Edit commands while
    /// focused, its context menu (on `menu_on` when not on itself).
    fn touch(
        &self,
        ctx: &egui::Context,
        path: &str,
        response: &egui::Response,
        menu_on: Option<&egui::Response>,
    ) {
        crate::name_rect(ctx, &format!("param:{path}"), response.rect);
        if let Some(app) = &self.params.app {
            if response.has_focus() {
                // The Edit commands copy it and paste onto it.
                crate::clipboard::param::focus(app, path, response.id);
            }
            egui_pop::api::menus::context_menu_with(
                menu_on.unwrap_or(response),
                app,
                &crate::plugins::PARAM,
                json!({ "path": path }),
            );
        }
    }
}

impl Drawings for Host<'_> {
    fn draw(
        &mut self,
        ui: &mut Ui,
        key: &str,
        _control: &Control,
        rect: egui::Rect,
        _value: &mut helios_controls::value::Value,
    ) -> Option<Vec<Edit>> {
        let row = self.rows.get(key)?;
        let (path, entry, operator) = (row.path.clone(), row.entry.clone(), row.operator);
        let ports = row.ports.clone();
        let d = &entry.descriptor;
        let ctx = ui.ctx().clone();
        let label = label(&ctx, &path, d);
        let (response, menu_on) = match (d.ty, &entry.value) {
            (Type::Choice, Value::Choice(c)) if operator => {
                let (r, l) = operator_choice(ui, self.store, &path, d, c, &label);
                (r, Some(l))
            }
            (_, Value::List(_) | Value::Ints(_)) if ports.is_some() => {
                self.below.push(Below::Ports(ports.unwrap_or_default()));
                (ui.label(&label), None)
            }
            (Type::Floats, Value::List(l))
                if d.widget == Some(ParamWidget::Curve) && l.len() >= 4 =>
            {
                (
                    self.params.curve(ui, self.store, &path, &entry, l, rect),
                    None,
                )
            }
            (Type::Floats, Value::List(l))
                if d.widget == Some(ParamWidget::Wheel) && l.len() >= 3 =>
            {
                (
                    self.params.wheel(ui, self.store, &path, &entry, l, rect),
                    None,
                )
            }
            (_, v @ (Value::List(_) | Value::Ints(_))) => {
                let (r, open) = list_row(ui, &path, d, &label, v);
                if let Some(per_line) = open {
                    self.below.push(Below::Values(
                        Box::new(d.clone()),
                        entry.value.clone(),
                        per_line,
                    ));
                }
                (r, None)
            }
            (Type::Text, Value::Text(t)) => {
                (self.params.text(ui, self.store, &path, t, &label), None)
            }
            _ => return None,
        };
        self.touch(&ctx, &path, &response, menu_on.as_ref());
        Some(Vec::new())
    }

    fn label(&mut self, ctx: &egui::Context, key: &str, _: &Control) -> Option<String> {
        let row = self.rows.get(key)?;
        Some(label(ctx, &row.path, &row.entry.descriptor))
    }

    fn choice(&mut self, ctx: &egui::Context, key: &str, _: &Control, c: &str) -> Option<String> {
        let row = self.rows.get(key)?;
        if row.operator {
            return Some(crate::i18n::text(ctx, crate::mask_tree::operator_effect(c)));
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
        let picker = crate::i18n::text(
            ctx,
            Message::new("gui-param-picker", "Pick { $label } from the image")
                .with("label", label(ctx, &row.path, d)),
        );
        slider.quad(Icon::Picker, &picker)
    }

    /// A parameter with Helios's own context menu (its Reset, Copy,
    /// Paste…) keeps egui-pop's value menu off, so a right click opens
    /// that menu alone.
    fn value_menu(&mut self, key: &str, _: &Control) -> bool {
        !(self.rows.contains_key(key) && self.params.app.is_some())
    }

    fn verdict(&mut self, key: &str, control: &Control, c: &SliderChange) -> Verdict {
        let Some(row) = self.rows.get(key) else {
            return Verdict::Go;
        };
        control_verdict(control, &row.path, self.params.app.as_ref(), c)
    }

    fn marks(&mut self, key: &str) -> Option<Marks> {
        let row = self.rows.get(key)?;
        Some(Marks {
            changed: row.entry.value != row.entry.descriptor.default,
            held: row.held.as_ref().map(origin_name),
            moved: row.moved.as_ref().map(origin_name),
        })
    }

    fn dress(&mut self, ui: &mut Ui, dressing: Dressing<'_>) {
        let Some(row) = self.rows.get(dressing.key) else {
            return;
        };
        let ctx = ui.ctx().clone();
        let path = row.path.clone();
        if let Some(at) = dressing.changed {
            crate::name_rect(&ctx, &format!("param:{path}:changed"), at);
        }
        if let Some(at) = dressing.moved {
            crate::name_rect(&ctx, &format!("param:{path}:moved"), at);
        }
        let Some(response) = dressing.response else {
            // Drawn here: touched as it was drawn.
            return;
        };
        // A list's own response keeps its popup: a context menu on it
        // shares the popup's id and closes the list on the click that
        // opens it. Its label takes the menu; a list with none, none.
        let is_list = matches!(
            dressing.control.value,
            helios_controls::model::ValueSpec::OneOf { .. }
        );
        if is_list && dressing.label.is_none() {
            crate::name_rect(&ctx, &format!("param:{path}"), response.rect);
        } else {
            self.touch(&ctx, &path, response, dressing.label);
        }
        if dressing.control.value.number().is_some() {
            let d = &row.entry.descriptor;
            let shown_label = label(&ctx, &path, d);
            let locale = crate::i18n::locale(&ctx);
            let about = description(&ctx, &path, d);
            let _ = response.clone().on_hover_text(format!(
                "{}\n{}\n{}",
                if about.is_empty() {
                    &shown_label
                } else {
                    &about
                },
                crate::i18n::text(
                    &ctx,
                    Message::new("gui-param-default", "{ $path } · default { $value }")
                        .with("path", path.as_str())
                        .with("value", shown_in(&locale, d, &d.default)),
                ),
                gesture_hint(&ctx),
            ));
        }
    }
}

/// What a slider's change becomes, once its control's detents held it:
/// egui-pop's `slider.change` step (`app`'s), then, once done, the
/// `controls.change` step; a veto or a redirect of either wins.
fn control_verdict(
    control: &Control,
    path: &str,
    app: Option<&egui_pop::api::AppHandle>,
    c: &SliderChange,
) -> Verdict {
    let mut change = *c;
    if let Some(app) = app {
        let mut hook = sliders::hook(app, path);
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
        let steps = crate::controls::steps()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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

impl Params {
    /// The widgets' memory, resolving cards in `contexts` (the ones the
    /// `controls.*` commands share).
    pub fn new(contexts: SharedContexts) -> Self {
        Self {
            contexts,
            ..Self::default()
        }
    }

    /// The controls' context the panel's cards resolve in.
    pub fn contexts(&self) -> &SharedContexts {
        &self.contexts
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
    fn end_steps(&mut self, store: &Mutex<Store>, now: f64) {
        if self
            .steps
            .as_ref()
            .is_some_and(|(_, _, last)| now - last > STEPS_GAP)
            && let Some((_, g, _)) = self.steps.take()
        {
            store.lock().expect("value store lock").end(g);
        }
    }

    /// The application whose declared context menu the widgets open.
    pub fn set_app(&mut self, app: egui_pop::api::AppHandle) {
        self.app = Some(app);
    }

    /// The marks of a path this frame: (moved by, at what time).
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

    /// One parameter alone: a card of its own, in the width available.
    pub fn param_ui(&mut self, ui: &mut Ui, store: &Mutex<Store>, path: &str) {
        let Some((node, _)) = node_param(path) else {
            return;
        };
        let node = node.to_owned();
        let keys = self.keys_for(ui, false);
        self.card_ui(
            ui,
            store,
            &node,
            &format!("{path}#alone"),
            &[path],
            &Members::new(),
            keys,
        );
    }

    /// The context keys a card resolves in, its box the width available
    /// at the density. The panel's own (`shared`) also sets the shared
    /// box and the hand (a gesture open, the pointer over it), so the
    /// `controls.*` commands see what it shows.
    fn keys_for(&self, ui: &Ui, shared: bool) -> ContextKeys {
        let mut c = self
            .contexts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    /// previous resolution), drawn by the managed renderers and this
    /// panel's [`Host`], their changes written to the store as the user.
    #[allow(
        clippy::too_many_arguments,
        reason = "the card's node, id, parameters, members and keys, and where to draw"
    )]
    fn card_ui(
        &mut self,
        ui: &mut Ui,
        store: &Mutex<Store>,
        node: &str,
        id: &str,
        paths: &[&str],
        members: &Members,
        keys: ContextKeys,
    ) {
        let now = ui.input(|i| i.time);
        let ctx = ui.ctx().clone();
        let global = crate::controls::registry();
        let mut registry = (**global).clone();
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
                let d = &entry.descriptor;
                let group = group_param(d, path);
                let operator = group.is_some_and(|(_, name)| name == "operator");
                let ports = group
                    .filter(|(_, name)| matches!(*name, "opacity" | "invert"))
                    .and_then(|(node, _)| members.get(node))
                    .map(|m| port_lines(&ctx, &entry.value, m));
                let control = control_of(global, path, d, operator);
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
                        ports,
                        operator,
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
        if items.is_empty() {
            return;
        }
        let card = helios_controls::card::Card::new(items);
        let placement = {
            let mut c = self
                .contexts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
                    ui.colored_label(ui.visuals().error_fg_color, e);
                    return;
                }
            }
        };
        let mut host = Host {
            params: self,
            store,
            rows,
            below: Vec::new(),
        };
        let out = ui
            .push_id(("helios.params.card", id), |ui| {
                render::card(ui, &registry, &placement, &mut shown, &mut host)
            })
            .inner;
        let Host { rows, below, .. } = host;
        for b in below {
            match b {
                Below::Values(d, v, per_line) => list_values(ui, &d, &v, per_line),
                Below::Ports(lines) => {
                    ui.indent("ports", |ui| {
                        for (line, hint) in lines {
                            ui.add(egui::Label::new(line).wrap()).on_hover_text(hint);
                        }
                    });
                }
            }
        }
        self.apply_card(&ctx, store, node, &rows, out);
    }

    /// Writes what a card changed this frame, as the user, and runs its
    /// requests (a reset, a picker, a reveal).
    fn apply_card(
        &mut self,
        ctx: &egui::Context,
        store: &Mutex<Store>,
        node: &str,
        rows: &BTreeMap<String, Row>,
        out: render::CardOutput,
    ) {
        let mut numbers: BTreeMap<&str, Vec<render::Change>> = BTreeMap::new();
        for e in &out.edits {
            let Some(row) = rows.get(e.key()) else {
                continue;
            };
            match e {
                Edit::Number { key, change } => {
                    numbers.entry(key.as_str()).or_default().push(*change);
                }
                Edit::Set { to, done, .. } => {
                    let to = helios_controls::values::helios_value(to.clone());
                    if *done {
                        let mut s = store.lock().expect("value store lock");
                        write(&mut s, &row.path, Change::To(to), None, false);
                    }
                }
            }
        }
        for (key, changes) in numbers {
            let row = &rows[key];
            let d = &row.entry.descriptor;
            let shown_label = label(ctx, &row.path, d);
            self.apply(ctx, store, &row.path, d, &shown_label, &changes);
        }
        for e in out.events {
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
                    let mut c = self
                        .contexts
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    c.moments
                        .entry(node.to_owned())
                        .or_default()
                        .revealed
                        .insert(key);
                }
                CardEvent::RevealAll => {
                    let mut c = self
                        .contexts
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    c.moments.entry(node.to_owned()).or_default().reveal_all = true;
                }
            }
        }
    }

    /// Writes a number's changes this frame to the store, as the user: a
    /// drag's (or a scrub's) frames as one gesture ended by its release,
    /// steps in a row as one group, a typed value forced past the soft
    /// range (and said so), a drag given up leaving no trace.
    fn apply(
        &mut self,
        ctx: &egui::Context,
        store: &Mutex<Store>,
        path: &str,
        d: &Descriptor,
        shown_label: &str,
        changes: &[render::Change],
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
                    let note =
                        write(&mut s, path, Change::To(value(c.to)), None, force).and_then(|a| {
                            match (force, a.clamped) {
                                (_, helios_values::Clamped::Hard) => Some(
                                    Message::new(
                                        "gui-param-held",
                                        "{ $label }: held to { $value }",
                                    )
                                    .with("label", shown_label)
                                    .with(
                                        "value",
                                        shown_in(&crate::i18n::locale(ctx), d, &a.value),
                                    ),
                                ),
                                (true, _) => Some(
                                    Message::new(
                                        "gui-param-past-range",
                                        "{ $label }: past its usual range",
                                    )
                                    .with("label", shown_label),
                                ),
                                _ => None,
                            }
                        });
                    if let Some(n) = note {
                        request(ctx, Request::Notify(Level::Info, n));
                    }
                }
            }
        }
    }

    /// A text field: its label, then the field; what is typed is kept
    /// here until the field is left, then written as the user.
    fn text(
        &mut self,
        ui: &mut Ui,
        store: &Mutex<Store>,
        path: &str,
        stored: &str,
        label: &str,
    ) -> egui::Response {
        let mut text = match &self.typing {
            Some((p, t)) if p == path => t.clone(),
            _ => stored.to_owned(),
        };
        let l = ui.label(label);
        let r = ui
            .add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width()))
            .labelled_by(l.id);
        if r.changed() {
            self.typing = Some((path.to_owned(), text.clone()));
        }
        if r.lost_focus() {
            if self.typing.as_ref().is_some_and(|(p, _)| p == path) {
                self.typing = None;
            }
            if text != stored {
                let mut s = store.lock().expect("value store lock");
                write(&mut s, path, Change::To(Value::Text(text)), None, false);
            }
        }
        r
    }

    /// A curve (`ParamWidget::Curve`) in its slot: its label, then
    /// egui-pop's curve editor over the list's points, each finished
    /// change through the `curve.change` step, written as the user (a
    /// drag as one undo step).
    fn curve(
        &mut self,
        ui: &mut Ui,
        store: &Mutex<Store>,
        path: &str,
        entry: &Entry,
        list: &[f64],
        rect: egui::Rect,
    ) -> egui::Response {
        let d = &entry.descriptor;
        let label = label(ui.ctx(), path, d);
        let mut points = curve_points(list);
        let defaults = match &d.default {
            Value::List(l) if l.len() >= 4 => curve_points(l),
            _ => egui_pop::viewport::curve::identity(),
        };
        let row = ui.spacing().interact_size.y;
        let side = rect.width().min(rect.height() - row).max(48.0);
        let app = self.app.clone();
        let out = ui
            .vertical(|ui| {
                ui.label(&label);
                let mut editor = CurveEditor::new(&mut points, &label)
                    .default_points(defaults)
                    .size(egui::vec2(side, side));
                if let Some(n) = d.count {
                    editor = editor.max_points(n as usize / 2);
                }
                if let Some(app) = &app {
                    editor = editor.on_change(egui_pop::imaging::curve(app, path));
                }
                editor.show(ui)
            })
            .inner;
        self.apply_tool(store, path, d, &out.changes, &out.response, |p| {
            Value::List(
                p.iter()
                    .flat_map(|[x, y]| [f64::from(*x), f64::from(*y)])
                    .collect(),
            )
        });
        out.response
    }

    /// A colour wheel (`ParamWidget::Wheel`) in its slot: its label, then
    /// egui-pop's offset wheel and its luminance slider over the list's
    /// hue (in turns), chroma and luminance, each finished change through
    /// the `wheel.change` step, written as the user (a drag as one undo
    /// step).
    fn wheel(
        &mut self,
        ui: &mut Ui,
        store: &Mutex<Store>,
        path: &str,
        entry: &Entry,
        list: &[f64],
        rect: egui::Rect,
    ) -> egui::Response {
        let d = &entry.descriptor;
        let label = label(ui.ctx(), path, d);
        let mut value = wheel_value(list);
        let default = match &d.default {
            Value::List(l) if l.len() >= 3 => wheel_value(l),
            _ => WheelValue::default(),
        };
        let row = ui.spacing().interact_size.y;
        // Room for the label above and the luminance slider below.
        let side = rect.width().min(rect.height() - 2.0 * row).max(40.0);
        let app = self.app.clone();
        let out = ui
            .vertical(|ui| {
                ui.label(&label);
                let mut wheel = ColorWheel::offset(&mut value, &label)
                    .default(default)
                    .size(side);
                if let Some(app) = &app {
                    wheel = wheel.on_change(egui_pop::imaging::wheel(app, path));
                }
                wheel.show(ui)
            })
            .inner;
        self.apply_tool(store, path, d, &out.changes, &out.response, |w| {
            Value::List(vec![
                f64::from(w.hue) / 360.0,
                f64::from(w.chroma),
                f64::from(w.luminance),
            ])
        });
        out.response
    }

    /// Writes what an image tool changed this frame, as the user: a drag's
    /// frames as one gesture ended by its release (one undo step, other
    /// origins kept off meanwhile), keys in a row as one group, any other
    /// finished change as a step of its own. A drag given up (Escape, a
    /// veto at its release) leaves no trace.
    fn apply_tool<T>(
        &mut self,
        store: &Mutex<Store>,
        path: &str,
        d: &Descriptor,
        changes: &[ToolChange<T>],
        response: &egui::Response,
        value: impl Fn(&T) -> Value,
    ) {
        let now = response.ctx.input(|i| i.time);
        let mut s = store.lock().expect("value store lock");
        let held = |g: &Option<(String, GestureId)>| {
            g.as_ref().filter(|(p, _)| p == path).map(|(_, g)| *g)
        };
        for c in changes {
            let v = value(&c.to);
            if !c.done {
                let g = held(&self.gesture).unwrap_or_else(|| {
                    let g = s.begin_gesture(Origin::User, &format!("drag {}", d.label));
                    self.gesture = Some((path.into(), g));
                    g
                });
                write(&mut s, path, Change::To(v), Some(g), false);
            } else if let Some(g) = held(&self.gesture) {
                self.gesture = None;
                write(&mut s, path, Change::To(v), Some(g), false);
                s.end(g);
            } else if matches!(c.cause, ToolCause::Key | ToolCause::Access) {
                self.step(&mut s, path, Change::To(v), &d.label, now);
            } else {
                write(&mut s, path, Change::To(v), None, false);
            }
        }
        if response.drag_stopped()
            && let Some(g) = held(&self.gesture)
        {
            self.gesture = None;
            s.cancel(g);
        }
    }

    /// The Parameters panel: the selected node's values, or every node's,
    /// grouped by node in the manifest's order; inside a sub-workspace
    /// (the store's scope), its nodes' only. Each node is a card of the
    /// controls layer (see the module's notes). A node whose parameters
    /// name groups (a macro's exposed ones) shows them by section, a
    /// heading per group that folds as a list does (see [`sections`]),
    /// each section a card. A mask group's per-port lists say which
    /// member each value is for, from `members`.
    pub fn panel_ui(
        &mut self,
        ui: &mut Ui,
        store: &Mutex<Store>,
        selection: Option<&str>,
        members: &Members,
    ) {
        self.end_steps(store, ui.input(|i| i.time));
        let paths: Vec<(String, String, Option<String>)> = {
            let s = store.lock().expect("value store lock");
            let mut entries = s.list("node/");
            // In the order the manifests declare them.
            entries.sort_by_key(|(_, e)| e.order);
            entries
                .into_iter()
                .filter_map(|(p, e)| {
                    let (node, _) = node_param(p)?;
                    Some((node.to_string(), p.to_string(), e.descriptor.group.clone()))
                })
                .filter(|(node, _, _)| selection.is_none_or(|s| s == node))
                .filter(|(node, _, _)| s.in_scope(node))
                .collect()
        };
        if paths.is_empty() {
            ui.weak("No parameters: nothing is shown yet");
            return;
        }
        ui.scroll_area(
            egui::ScrollArea::vertical().id_salt("helios.params"),
            |ui| {
                let keys = self.keys_for(ui, true);
                let mut start = 0;
                while start < paths.len() {
                    let node = &paths[start].0;
                    let end = paths[start..]
                        .iter()
                        .position(|(n, _, _)| n != node)
                        .map_or(paths.len(), |i| start + i);
                    ui.add_space(4.0);
                    ui.strong(node);
                    // A module's own header (the chart profile's choice).
                    crate::calibrate::header_ui(ui, store, node);
                    // The Camera look entry's (fit, switch, scores).
                    crate::calibrate::look_header_ui(ui, store, node);
                    let rows: Vec<(&str, Option<&str>)> = paths[start..end]
                        .iter()
                        .map(|(_, p, g)| (p.as_str(), g.as_deref()))
                        .collect();
                    for (group, paths) in sections(&rows) {
                        if let Some(group) = group
                            && !section_row(ui, node, group)
                        {
                            continue;
                        }
                        // The node's own card keeps the node's name (what
                        // `controls.*` resolves); a section's is named after it.
                        let id = group.map_or_else(|| node.clone(), |g| format!("{node}#{g}"));
                        self.card_ui(ui, store, node, &id, &paths, members, keys.clone());
                    }
                    start = end;
                }
            },
        );
    }
}

/// A node's parameters by section: the ungrouped ones first, then each
/// group in the order of its first parameter, its parameters in their
/// order (`docs/design/macros.md`, "The exposed-parameter editor").
pub fn sections<'a>(rows: &[(&'a str, Option<&'a str>)]) -> Vec<(Option<&'a str>, Vec<&'a str>)> {
    let mut out: Vec<(Option<&str>, Vec<&str>)> = Vec::new();
    if rows.iter().any(|(_, g)| g.is_none()) {
        out.push((None, Vec::new()));
    }
    for (path, group) in rows {
        match out.iter_mut().find(|(g, _)| g == group) {
            Some((_, paths)) => paths.push(path),
            None => out.push((*group, vec![path])),
        }
    }
    out
}

/// A section's heading: a chevron and the group's name, a click on
/// either folds or unfolds it (open at first). Returns whether it is
/// open.
fn section_row(ui: &mut Ui, node: &str, group: &str) -> bool {
    let ctx = ui.ctx().clone();
    let id = egui::Id::unique(("helios.params.section", node, group));
    let open = ctx.data(|m| m.get_temp::<bool>(id)).unwrap_or(true);
    let hint = crate::i18n::text(
        &ctx,
        if open {
            Message::new("gui-params-fold-section", "Fold { $group }")
        } else {
            Message::new("gui-params-unfold-section", "Unfold { $group }")
        }
        .with("group", group),
    );
    let icon = if open {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    };
    let clicked = ui
        .horizontal(|ui| {
            let chevron = ui
                .add(icon.tone(Tone::Muted).label(&hint).sense_click())
                .on_hover_text(&hint);
            let title = ui
                .add(egui::Label::new(egui::RichText::new(group).strong()).sense(Sense::click()))
                .on_hover_text(&hint);
            chevron.clicked() || title.clicked()
        })
        .inner;
    if clicked {
        ctx.data_mut(|m| m.insert_temp(id, !open));
    }
    open
}

/// A mask group's operator: a list of the operators by their effect
/// (union, intersection, …), each after the Masks panel's preview of it,
/// the chosen one's preview before the list. Returns the list's response
/// and its label's, where the context menu goes.
fn operator_choice(
    ui: &mut Ui,
    store: &Mutex<Store>,
    path: &str,
    d: &Descriptor,
    current: &str,
    label: &str,
) -> (egui::Response, egui::Response) {
    use crate::mask_tree::{operator_effect, operator_glyph, operator_hint};
    let ctx = ui.ctx().clone();
    let effect = |o: &str| crate::i18n::text(&ctx, operator_effect(o));
    let mut picked = current.to_string();
    operator_glyph(ui, current, &operator_hint(&ctx, current));
    let r = egui::ComboBox::from_id_salt(path)
        .selected_text(effect(current))
        .show_ui(ui, |ui| {
            for o in &d.choices {
                ui.horizontal(|ui| {
                    operator_glyph(ui, o, &operator_hint(&ctx, o));
                    ui.selectable_value(&mut picked, o.clone(), effect(o));
                });
            }
        })
        .response
        .on_hover_text(operator_hint(&ctx, current));
    egui_pop::core::a11y::name(&r, egui::Role::ComboBox, label);
    let l = ui.label(label);
    if picked != current {
        let mut s = store.lock().expect("value store lock");
        write(&mut s, path, Change::To(Value::Choice(picked)), None, false);
    }
    (r, l)
}

/// A mask group's per-port list (opacities or inversions) as lines, one
/// per wired port: the member's name and its value in words, as the
/// Masks panel's rows say them, with the port in the line's hint.
fn port_lines(ctx: &egui::Context, v: &Value, members: &[Option<String>]) -> Vec<(String, String)> {
    use crate::mask_tree::{inverted_words, opacity_words};
    let words = |port: usize| match v {
        Value::List(l) => opacity_words(l.get(port).copied().unwrap_or(1.0)),
        Value::Ints(l) => inverted_words(l.get(port).is_some_and(|x| *x != 0)),
        _ => Message::plain(""),
    };
    let lines: Vec<(String, String)> = members
        .iter()
        .enumerate()
        .filter_map(|(port, m)| {
            let member = m.as_deref()?;
            let line = crate::i18n::text(
                ctx,
                Message::new("gui-params-port-member", "{ $member }: { $value }")
                    .with("member", member)
                    .with("value", crate::i18n::text(ctx, words(port))),
            );
            let hint = crate::i18n::text(
                ctx,
                Message::new(
                    "gui-params-port-hint",
                    "Port { $port } of the group; the Masks panel edits it on { $member }'s row",
                )
                .with("port", port + 1)
                .with("member", member),
            );
            Some((line, hint))
        })
        .collect();
    if lines.is_empty() {
        let none = crate::i18n::text(ctx, Message::new("gui-params-no-members", "no members yet"));
        return vec![(none.clone(), none)];
    }
    lines
}

/// A curve's points from its list (`x0, y0, x1, y1, …`).
pub fn curve_points(list: &[f64]) -> Vec<[f32; 2]> {
    list.chunks_exact(2)
        .map(|p| [p[0] as f32, p[1] as f32])
        .collect()
}

/// A wheel's value from its list (hue in turns, chroma, luminance).
pub fn wheel_value(list: &[f64]) -> WheelValue {
    let at = |i: usize| list.get(i).copied().unwrap_or(0.0) as f32;
    WheelValue {
        hue: (at(0) * 360.0).rem_euclid(360.0),
        chroma: at(1),
        luminance: at(2),
    }
}

/// How many values a line of an open list holds, when they are not
/// points.
const VALUES_PER_LINE: usize = 8;

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

/// A list's row: its label, then a summary (`12 points`) that a click on
/// it or on the chevron before it opens or closes. A list folded keeps
/// its row as narrow as the others. Returns the summary's response and,
/// open, how many values a line holds.
fn list_row(
    ui: &mut Ui,
    path: &str,
    d: &Descriptor,
    label: &str,
    v: &Value,
) -> (egui::Response, Option<usize>) {
    let len = match v {
        Value::List(l) => l.len(),
        Value::Ints(l) => l.len(),
        _ => 0,
    };
    let (summary, per_line) = list_items(d, path, len);
    let ctx = ui.ctx().clone();
    let summary = crate::i18n::text(&ctx, summary);
    let id = egui::Id::unique(("helios.params.open", path));
    let open = ctx.data(|m| m.get_temp::<bool>(id)).unwrap_or(false);
    let (icon, hint) = if open {
        (
            Icon::ChevronDown,
            Message::new("gui-params-hide-values", "Hide the values"),
        )
    } else {
        (
            Icon::ChevronRight,
            Message::new("gui-params-show-values", "Show the values"),
        )
    };
    let hint = crate::i18n::text(&ctx, hint);
    let chevron = ui
        .add(icon.tone(Tone::Muted).label(&hint).sense_click())
        .on_hover_text(&hint);
    ui.label(label);
    let r = ui
        .add(egui::Label::new(egui::RichText::new(&summary).weak()).sense(Sense::click()))
        .on_hover_text(&hint);
    if chevron.clicked() || r.clicked() {
        ctx.data_mut(|m| m.insert_temp(id, !open));
    }
    (r, open.then_some(per_line))
}

/// An open list's values under its row: `per_line` values a line (a
/// point), numbered from 1, wrapped to the panel's width.
fn list_values(ui: &mut Ui, d: &Descriptor, v: &Value, per_line: usize) {
    let locale = crate::i18n::locale(ui.ctx());
    let (values, fraction): (Vec<f64>, usize) = match v {
        Value::List(l) => (l.clone(), d.precision as usize),
        Value::Ints(l) => (l.iter().map(|&x| x as f64).collect(), 0),
        _ => return,
    };
    ui.indent("values", |ui| {
        for (i, line) in values.chunks(per_line.max(1)).enumerate() {
            let line: Vec<String> = line
                .iter()
                .map(|x| crate::i18n::fixed(&locale, *x, fraction))
                .collect();
            ui.add(egui::Label::new(format!("{}: {}", i + 1, line.join("  "))).wrap());
        }
    });
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

/// A slider's gestures, as the pointer profile gives them: the finer and
/// coarser modifiers, and when the wheel steps it.
fn gesture_hint(ctx: &egui::Context) -> String {
    let g = egui_pop::core::slider::gestures(ctx);
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let keys = |m: egui::Modifiers| egui::ModifierNames::NAMES.format(&m, mac);
    let wheel = if g.wheel_steps {
        Message::new("gui-param-hint-wheel-over", "the wheel over it")
    } else if g.wheel.any() {
        Message::new(
            "gui-param-hint-wheel-focused-or",
            "the wheel once focused or with { $keys }",
        )
        .with("keys", keys(g.wheel))
    } else {
        Message::new("gui-param-hint-wheel-focused", "the wheel once focused")
    };
    crate::i18n::text(
        ctx,
        Message::new(
            "gui-param-hint",
            "Drag to change ({ $fine } finer, { $coarse } coarser), arrows to step, { $wheel }; double click resets; Enter types a value or an expression (x is the value)",
        )
        .with("fine", keys(g.fine))
        .with("coarse", keys(g.coarse))
        .with("wheel", wheel),
    )
}
