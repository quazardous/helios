//! The bench: controls moved from a shelf onto a card, composed and tried
//! live (`docs/design/controls.md`, section 7, slice 7;
//! `docs/design/control-contexts.md`, section 5).
//!
//! - The [`Shelf`] lists what can go on the bench: the registered
//!   business controls by purpose, then an implicit control per value type
//!   (a number, a toggle, a choice, a curve, a colour, a text), each built
//!   from a descriptor as a module's parameter would be.
//! - A [`Composition`] is what is on the bench, in its logical order:
//!   controls, and cases grouping some of them under a legend, with help
//!   text and a closing separator as the case's chrome. It splits into
//!   cards ([`Composition::sections`]): each run of loose controls, and
//!   each case as a card of its own (a `fieldset`), so dense packing never
//!   pulls a part above its legend. Each is laid out by the slot engine
//!   through `resolve`, as any module's card is, and they are stacked.
//! - [`Bench`] holds the composition, an in-memory value store
//!   ([`MemoryStore`]) with a value per control on the bench, the box's
//!   width, the previous resolution (for hysteresis), and runs the
//!   `controls.bench.*` commands ([`COMMANDS`]). Every value changes
//!   through the value flow: a gesture previews, commits one undo step or
//!   cancels with no trace.
//!
//! The drawing is in [`crate::bench_ui`].

use std::collections::{BTreeMap, BTreeSet};

use egui_hudget::commands::CommandInfo;
use egui_hudget::flow::{self, Cause, GestureOf, Outcome, Steps};
use egui_hudget::model::{Control, ValueSpec, ValueType};
use egui_hudget::resolve::Layouts;
use egui_hudget::slot::{Chrome, ChromeKind};
use egui_hudget::value::{MemoryStore, ParamDescriptor, ParamType, Value, ValueSource, ValueStore};
use egui_hudget::{CardPlacement, ContextKeys, Placement, Registry};
use helios_controls::card::{Card, CardItem, CardLayout, Rect as CardRect};
use helios_controls::context::{CardBox, Density};
use helios_controls::render;
use helios_controls::resolve::Moment;
use helios_controls::{HeliosRegistry as _, Surface};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

/// The bench's node: its values live at `node/bench/<key>`.
pub const NODE: &str = "bench";

/// The store path of a control on the bench.
pub fn path_of(key: &str) -> String {
    format!("node/{NODE}/{key}")
}

/// The narrowest and widest bench, in points.
pub const WIDTH_RANGE: (f32, f32) = (120.0, 960.0);

/// The bench's width at first: two slot columns.
pub const DEFAULT_WIDTH: f32 = 480.0;

/// Who writes from the bench's own widgets.
const POINTER: &str = "pointer";

/// One entry of the shelf.
#[derive(Clone, Debug, PartialEq)]
pub struct ShelfEntry {
    /// A short name, what a key on the bench starts with (`exposure`,
    /// `toggle`).
    pub name: String,
    /// The control's id in the bench's registry.
    pub control: String,
    pub label: String,
    /// The shelf's group: a purpose, or the implicit controls.
    pub group: String,
    /// One line: the control's purpose, or its value type.
    pub hint: String,
    /// The parameter a copy on the bench is declared with.
    pub descriptor: ParamDescriptor,
}

impl ShelfEntry {
    /// Whether it matches a search (any case): by its name, label, group
    /// or hint.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty()
            || [&self.name, &self.label, &self.group, &self.hint]
                .iter()
                .any(|s| s.to_lowercase().contains(&q))
    }
}

/// The group of the implicit controls on the shelf.
pub const IMPLICIT_GROUP: &str = "Implicit, by value type";

/// The purpose group of a business control on the shelf.
fn group_of(control: &Control) -> &'static str {
    match control.id.as_str() {
        helios_controls::catalogue::EXPOSURE => "Tone",
        _ => "Other purposes",
    }
}

/// A parameter declared for a business control's value.
fn descriptor_of(control: &Control) -> ParamDescriptor {
    match &control.value {
        ValueSpec::Number(n) => {
            let mut d = ParamDescriptor::float(&control.label, n.hard.0, n.hard.1, n.default);
            d.soft = Some(n.soft);
            d.step = n.step;
            d.coarse = n.coarse;
            d.unit.clone_from(&n.unit);
            d.precision = n.precision;
            d.display_scale = n.display_scale;
            if n.integer {
                d.ty = ParamType::Int;
                d.default = Value::Int(n.default.round() as i64);
            }
            d
        }
        ValueSpec::Bool { default } => {
            ParamDescriptor::new(ParamType::Bool, &control.label, Value::Bool(*default))
        }
        ValueSpec::OneOf { choices, default } => {
            let mut d = ParamDescriptor::new(
                ParamType::Choice,
                &control.label,
                Value::Choice(default.clone()),
            );
            d.choices.clone_from(choices);
            d
        }
        ValueSpec::Other { stored } => match stored {
            ValueType::Text => {
                ParamDescriptor::new(ParamType::Text, &control.label, Value::Text(String::new()))
            }
            _ => ParamDescriptor::new(ParamType::Floats, &control.label, Value::Floats(Vec::new())),
        },
    }
}

/// The implicit controls of the shelf: one parameter per value type, as a
/// module would declare it.
fn implicit_descriptors() -> Vec<(&'static str, &'static str, ParamDescriptor)> {
    let list = |label: &str, default: Vec<f64>, widget| {
        let mut d = ParamDescriptor::new(ParamType::Floats, label, Value::Floats(default));
        d.widget = Some(widget);
        d
    };
    let mut amount = ParamDescriptor::float("Amount", 0.0, 1.0, 0.5);
    amount.unit = Some("%".into());
    amount.display_scale = 100.0;
    amount.precision = 0;
    let mut count = ParamDescriptor::new(ParamType::Int, "Iterations", Value::Int(2));
    count.hard = Some((0.0, 8.0));
    count.step = 1.0;
    count.precision = 0;
    let mut method =
        ParamDescriptor::new(ParamType::Choice, "Method", Value::Choice("Filmic".into()));
    method.choices = vec!["Linear".into(), "Filmic".into(), "Sigmoid".into()];
    vec![
        ("number", "a number", amount),
        ("whole", "a whole number", count),
        (
            "toggle",
            "a toggle",
            ParamDescriptor::new(ParamType::Bool, "Preserve colours", Value::Bool(true)),
        ),
        ("choice", "a choice", method),
        (
            "curve",
            "a curve (points)",
            list(
                "Curve",
                vec![0.0, 0.0, 0.25, 0.2, 0.75, 0.85, 1.0, 1.0],
                egui_hudget::Widget::Curve,
            ),
        ),
        (
            "colour",
            "a colour",
            list("Lift", vec![0.08, 0.3, 0.1], egui_hudget::Widget::Wheel),
        ),
        (
            "text",
            "a text",
            ParamDescriptor::new(ParamType::Text, "Notes", Value::Text(String::new())),
        ),
    ]
}

/// What can go on the bench, in shelf order: business controls grouped by
/// purpose, then the implicit controls by value type.
#[derive(Clone, Debug)]
pub struct Shelf {
    pub entries: Vec<ShelfEntry>,
}

impl Shelf {
    /// The shelf of a registry's controls, and the registry with the
    /// shelf's implicit controls added.
    pub fn of(mut registry: Registry) -> (Self, Registry) {
        let mut entries: Vec<ShelfEntry> = registry
            .controls()
            .filter(|c| !c.implicit)
            .map(|c| ShelfEntry {
                name: c.id.clone(),
                control: c.id.clone(),
                label: c.label.clone(),
                group: group_of(c).to_owned(),
                hint: c.purpose.clone(),
                descriptor: descriptor_of(c),
            })
            .collect();
        entries.sort_by(|a, b| a.group.cmp(&b.group).then(a.label.cmp(&b.label)));
        for (name, hint, d) in implicit_descriptors() {
            let control = egui_hudget::implicit(&format!("shelf/{name}"), &d);
            entries.push(ShelfEntry {
                name: name.to_owned(),
                control: control.id.clone(),
                label: d.label.clone(),
                group: IMPLICIT_GROUP.to_owned(),
                hint: format!("Implicit control of {hint}"),
                descriptor: d,
            });
            // Each value type once: the shelf is built once per registry.
            let _ = registry.register(control);
        }
        (Self { entries }, registry)
    }

    /// The entry named `name`, or holding control `name`.
    pub fn get(&self, name: &str) -> Option<&ShelfEntry> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .or_else(|| self.entries.iter().find(|e| e.control == name))
    }

    /// Its groups, in order, each with its entries matching `query`;
    /// groups with none left out.
    pub fn groups(&self, query: &str) -> Vec<(String, Vec<&ShelfEntry>)> {
        let mut out: Vec<(String, Vec<&ShelfEntry>)> = Vec::new();
        for e in self.entries.iter().filter(|e| e.matches(query)) {
            match out.iter_mut().find(|(g, _)| *g == e.group) {
                Some((_, list)) => list.push(e),
                None => out.push((e.group.clone(), vec![e])),
            }
        }
        out
    }
}

/// A control on the bench: its key there, and its control's id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Piece {
    pub key: String,
    pub control: String,
}

/// A case on the bench: parts under a legend, with optional help text and
/// a closing separator (its chrome).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenchCase {
    pub key: String,
    pub legend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(default)]
    pub separator: bool,
    pub parts: Vec<Piece>,
}

/// One entry of the bench, at the top level.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Entry {
    Control(Piece),
    Case(BenchCase),
}

impl Entry {
    pub fn key(&self) -> &str {
        match self {
            Entry::Control(p) => &p.key,
            Entry::Case(c) => &c.key,
        }
    }
}

/// Where something goes or is: an index at the top level, or in a case.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<String>,
    pub index: usize,
}

impl Spot {
    pub fn top(index: usize) -> Self {
        Self { case: None, index }
    }

    pub fn in_case(case: &str, index: usize) -> Self {
        Self {
            case: Some(case.into()),
            index,
        }
    }
}

/// Which bench item a card item comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A control, where it sits.
    Control { key: String, spot: Spot },
    /// A case's chrome.
    Chrome { case: String, kind: ChromeKind },
}

/// What is on the bench, in logical order. Saved as data (slice 8).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Composition {
    pub entries: Vec<Entry>,
}

impl Composition {
    /// Every control's key, in logical order.
    pub fn keys(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for e in &self.entries {
            match e {
                Entry::Control(p) => out.push(p.key.as_str()),
                Entry::Case(c) => out.extend(c.parts.iter().map(|p| p.key.as_str())),
            }
        }
        out
    }

    /// Every control, in logical order.
    pub fn pieces(&self) -> Vec<(&Piece, Option<&BenchCase>)> {
        let mut out = Vec::new();
        for e in &self.entries {
            match e {
                Entry::Control(p) => out.push((p, None)),
                Entry::Case(c) => out.extend(c.parts.iter().map(|p| (p, Some(c)))),
            }
        }
        out
    }

    /// Whether `key` names a control or a case on the bench.
    pub fn holds(&self, key: &str) -> bool {
        self.find(key).is_some()
    }

    /// Where a control or a case is.
    pub fn find(&self, key: &str) -> Option<Spot> {
        for (i, e) in self.entries.iter().enumerate() {
            if e.key() == key {
                return Some(Spot::top(i));
            }
            if let Entry::Case(c) = e
                && let Some(j) = c.parts.iter().position(|p| p.key == key)
            {
                return Some(Spot::in_case(&c.key, j));
            }
        }
        None
    }

    /// The case keyed `key`.
    pub fn case(&self, key: &str) -> Option<&BenchCase> {
        self.entries.iter().find_map(|e| match e {
            Entry::Case(c) if c.key == key => Some(c),
            _ => None,
        })
    }

    pub fn case_mut(&mut self, key: &str) -> Option<&mut BenchCase> {
        self.entries.iter_mut().find_map(|e| match e {
            Entry::Case(c) if c.key == key => Some(c),
            _ => None,
        })
    }

    /// Takes a control or a case off the bench. A case left with no part
    /// goes too.
    pub fn take(&mut self, key: &str) -> Option<Entry> {
        let spot = self.find(key)?;
        match &spot.case {
            None => Some(self.entries.remove(spot.index)),
            Some(case) => {
                let c = self.case_mut(case)?;
                let piece = c.parts.remove(spot.index);
                if c.parts.is_empty() {
                    let case = case.clone();
                    self.entries.retain(|e| e.key() != case);
                }
                Some(Entry::Control(piece))
            }
        }
    }

    /// Puts an entry at a spot, its index held to the list's length. A
    /// case goes only at the top level.
    pub fn insert(&mut self, entry: Entry, spot: &Spot) -> Result<(), String> {
        match (&spot.case, entry) {
            (None, entry) => {
                let i = spot.index.min(self.entries.len());
                self.entries.insert(i, entry);
            }
            (Some(case), Entry::Control(piece)) => {
                let c = self
                    .case_mut(case)
                    .ok_or_else(|| format!("no case `{case}` on the bench"))?;
                let i = spot.index.min(c.parts.len());
                c.parts.insert(i, piece);
            }
            (Some(_), Entry::Case(c)) => {
                return Err(format!("case `{}` cannot go inside a case", c.key));
            }
        }
        Ok(())
    }

    /// Moves a control or a case to a spot, the index being its place once
    /// moved.
    pub fn place(&mut self, key: &str, to: &Spot) -> Result<(), String> {
        let entry = self
            .take(key)
            .ok_or_else(|| format!("nothing keyed `{key}` on the bench"))?;
        if matches!(entry, Entry::Case(_)) && to.case.is_some() {
            let back = self.entries.len();
            self.entries.insert(back.min(to.index), entry);
            return Err(format!("case `{key}` cannot go inside a case"));
        }
        if let Some(case) = &to.case
            && self.case(case).is_none()
        {
            // The case went with its last part: back at the top level.
            self.entries.push(entry);
            return Err(format!("no case `{case}` on the bench"));
        }
        self.insert(entry, to)
    }

    /// The final spot of `key` dropped at an insertion point (before the
    /// item now at `at.index`).
    pub fn landing(&self, key: &str, at: &Spot) -> Spot {
        match self.find(key) {
            Some(from) if from.case == at.case && from.index < at.index => Spot {
                case: at.case.clone(),
                index: at.index - 1,
            },
            _ => at.clone(),
        }
    }

    /// Groups controls into a new case at the first one's place, under a
    /// legend; answers the case's key. Controls already in a case leave it.
    pub fn group(&mut self, keys: &[String], legend: &str, case_key: String) -> Result<(), String> {
        if keys.is_empty() {
            return Err("a case needs at least one control".into());
        }
        for k in keys {
            match self.find(k) {
                None => return Err(format!("no control `{k}` on the bench")),
                Some(s) if s.case.is_none() && matches!(self.entries[s.index], Entry::Case(_)) => {
                    return Err(format!("`{k}` is a case: cases do not nest"));
                }
                Some(_) => {}
            }
        }
        // The case goes where the first control (in logical order) was,
        // or where its case was.
        let order = self.keys();
        let first = order
            .iter()
            .find(|k| keys.iter().any(|w| w == *k))
            .map(|k| (*k).to_owned())
            .unwrap_or_default();
        let at = self.find(&first).map_or(0, |s| match s.case {
            None => s.index,
            Some(c) => self.find(&c).map_or(0, |cs| cs.index),
        });
        let mut parts = Vec::new();
        for k in order.iter().filter(|k| keys.iter().any(|w| w == *k)) {
            parts.push((*k).to_owned());
        }
        let mut pieces = Vec::new();
        for k in &parts {
            if let Some(Entry::Control(p)) = self.take(k) {
                pieces.push(p);
            }
        }
        let at = at.min(self.entries.len());
        self.entries.insert(
            at,
            Entry::Case(BenchCase {
                key: case_key,
                legend: legend.to_owned(),
                help: None,
                separator: true,
                parts: pieces,
            }),
        );
        Ok(())
    }

    /// Puts a case's parts back at the top level, where it was.
    pub fn ungroup(&mut self, case: &str) -> Result<(), String> {
        let index = self
            .entries
            .iter()
            .position(|e| matches!(e, Entry::Case(c) if c.key == case))
            .ok_or_else(|| format!("no case `{case}` on the bench"))?;
        // The entry at `index` is that case.
        if let Entry::Case(c) = self.entries.remove(index) {
            for (i, p) in c.parts.into_iter().enumerate() {
                self.entries.insert(index + i, Entry::Control(p));
            }
        }
        Ok(())
    }

    /// The bench's cards, in logical order: each run of loose controls is
    /// one card, each case another (its legend, help, parts and
    /// separator), so dense packing never pulls a part out of its case;
    /// with where each card item comes from.
    pub fn sections(&self) -> Vec<(Card, Vec<Origin>)> {
        let mut out: Vec<(Card, Vec<Origin>)> = Vec::new();
        let mut loose = false;
        for (i, e) in self.entries.iter().enumerate() {
            match e {
                Entry::Control(p) => {
                    if !loose {
                        out.push((Card::default(), Vec::new()));
                        loose = true;
                    }
                    let (card, origins) = out.last_mut().expect("pushed above");
                    card.items.push(CardItem::control(&p.key, &p.control));
                    origins.push(Origin::Control {
                        key: p.key.clone(),
                        spot: Spot::top(i),
                    });
                }
                Entry::Case(c) => {
                    loose = false;
                    let mut items = Vec::new();
                    let mut origins = Vec::new();
                    let mut chrome = |kind, text: Option<&str>| {
                        let mut ch = Chrome::new(kind);
                        ch.text = text.map(str::to_owned);
                        items.push(CardItem::Chrome(ch));
                        origins.push(Origin::Chrome {
                            case: c.key.clone(),
                            kind,
                        });
                    };
                    chrome(ChromeKind::Legend, Some(&c.legend));
                    if let Some(help) = &c.help {
                        chrome(ChromeKind::Help, Some(help));
                    }
                    for (j, p) in c.parts.iter().enumerate() {
                        items.push(CardItem::control(&p.key, &p.control));
                        origins.push(Origin::Control {
                            key: p.key.clone(),
                            spot: Spot::in_case(&c.key, j),
                        });
                    }
                    if c.separator {
                        items.push(CardItem::Chrome(Chrome::new(ChromeKind::Separator)));
                        origins.push(Origin::Chrome {
                            case: c.key.clone(),
                            kind: ChromeKind::Separator,
                        });
                    }
                    out.push((Card::new(items), origins));
                }
            }
        }
        out
    }
}

/// The gap between two of the bench's cards, in points.
pub const SECTION_GAP: f32 = 6.0;

/// One of the bench's cards, resolved, at its offset from the bench's top.
#[derive(Clone, Debug)]
pub struct Section {
    pub placement: CardPlacement,
    pub origins: Vec<Origin>,
    pub top: f32,
}

/// The bench laid out: its cards stacked.
#[derive(Clone, Debug, Default)]
pub struct Laid {
    pub sections: Vec<Section>,
    /// The height used, in points.
    pub height: f32,
}

impl Laid {
    /// Every item: its placement, where it comes from, and its slot in
    /// points from the bench's top left when placed.
    pub fn items(&self) -> impl Iterator<Item = (&Placement, &Origin, Option<CardRect>)> {
        self.sections.iter().flat_map(|s| {
            s.placement.items.iter().zip(&s.origins).map(move |(p, o)| {
                let r = p.rect.map(|r| CardRect {
                    y: r.y + s.top,
                    ..r
                });
                (p, o, r)
            })
        })
    }

    /// The placement of the control keyed `key`.
    pub fn get(&self, key: &str) -> Option<&Placement> {
        self.sections.iter().find_map(|s| s.placement.get(key))
    }

    fn stack(sections: Vec<(CardPlacement, Vec<Origin>)>) -> Self {
        let mut top = 0.0;
        let mut out = Vec::new();
        for (placement, origins) in sections {
            let h = placement.layout.as_ref().map_or(0.0, CardLayout::height);
            out.push(Section {
                placement,
                origins,
                top,
            });
            top += h + SECTION_GAP;
        }
        Self {
            sections: out,
            height: (top - SECTION_GAP).max(0.0),
        }
    }
}

/// The `controls.bench.*` commands.
pub const COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "controls.bench.add",
        help: "Puts a control from the shelf on the bench: {control} (a shelf name such as `exposure` or `toggle`, or a control id), optional {at} (its index) and {case} (a case's key); answers its {key}",
        read_only: false,
    },
    CommandInfo {
        name: "controls.bench.remove",
        help: "Takes a control or a case off the bench: {key}",
        read_only: false,
    },
    CommandInfo {
        name: "controls.bench.move",
        help: "Moves a control or a case: {key}, {to} (its index once moved), optional {case} (into that case; a case stays at the top level)",
        read_only: false,
    },
    CommandInfo {
        name: "controls.bench.case",
        help: "Groups controls into a case: {keys}, optional {legend}, {help} and {separator}; answers the case's {key}",
        read_only: false,
    },
    CommandInfo {
        name: "controls.bench.list",
        help: "What is on the bench: its width and box, each control (key, control, case, value, placement: slot, representation, orientation, rect) and each case",
        read_only: true,
    },
    CommandInfo {
        name: "controls.bench.clear",
        help: "Takes everything off the bench",
        read_only: false,
    },
];

/// The bench: its shelf, its composition, its values and its box.
pub struct Bench {
    /// The catalogue and the shelf's implicit controls.
    pub registry: Registry,
    pub shelf: Shelf,
    pub composition: Composition,
    /// A value per control on the bench, at `node/bench/<key>`.
    pub store: MemoryStore,
    steps: Steps,
    /// Number gestures open, by key.
    gestures: BTreeMap<String, GestureOf<MemoryStore>>,
    /// The other values' gesture open (a curve dragged), by key.
    set_gesture: Option<(String, u64)>,
    /// How the last change of each control ended.
    pub last: BTreeMap<String, String>,
    /// The bench's width, in points.
    pub width: f32,
    keys: ContextKeys,
    /// Each card's last resolution: hysteresis holds while the card is
    /// the same.
    previous: Vec<(Card, CardPlacement)>,
    next_case: u32,
}

impl Default for Bench {
    fn default() -> Self {
        Self::new(Registry::standard())
    }
}

impl Bench {
    /// An empty bench over a registry's controls.
    pub fn new(registry: Registry) -> Self {
        let (shelf, registry) = Shelf::of(registry);
        let mut keys = ContextKeys::default();
        keys.set_box(CardBox::from_points(
            Surface::Panel,
            DEFAULT_WIDTH,
            2000.0,
            Density::default(),
        ));
        Self {
            registry,
            shelf,
            composition: Composition::default(),
            store: MemoryStore::new(),
            steps: Steps::new(),
            gestures: BTreeMap::new(),
            set_gesture: None,
            last: BTreeMap::new(),
            width: DEFAULT_WIDTH,
            keys,
            previous: Vec::new(),
            next_case: 1,
        }
    }

    /// The context keys the bench is resolved in (the box from its
    /// width).
    pub fn keys(&self) -> &ContextKeys {
        &self.keys
    }

    /// Sets the bench's width, held to [`WIDTH_RANGE`]; the box follows,
    /// the space key with its hysteresis.
    pub fn set_width(&mut self, width: f32) {
        self.width = width.clamp(WIDTH_RANGE.0, WIDTH_RANGE.1);
        let density = self.keys.density;
        self.keys.set_box(CardBox::from_points(
            Surface::Panel,
            self.width,
            2000.0,
            density,
        ));
    }

    /// A free key for a control named `name`: `name`, then `name-2`…
    fn free_key(&self, name: &str) -> String {
        let taken = |k: &str| self.composition.holds(k);
        if !taken(name) {
            return name.to_owned();
        }
        // One more than the bench holds is always free.
        let n = self.composition.keys().len() + 2;
        (2..=n)
            .map(|n| format!("{name}-{n}"))
            .find(|k| !taken(k))
            .unwrap_or_default()
    }

    /// Puts a shelf entry (by name or control id) on the bench at a spot
    /// (the end when `None`), its value at its default; answers its key.
    pub fn add(&mut self, name: &str, at: Option<Spot>) -> Result<String, String> {
        let entry = self
            .shelf
            .get(name)
            .cloned()
            .ok_or_else(|| format!("nothing named `{name}` on the shelf"))?;
        let key = self.free_key(&entry.name);
        let spot = at.unwrap_or_else(|| Spot::top(self.composition.entries.len()));
        self.composition.insert(
            Entry::Control(Piece {
                key: key.clone(),
                control: entry.control.clone(),
            }),
            &spot,
        )?;
        self.store.declare(&path_of(&key), entry.descriptor);
        Ok(key)
    }

    /// Takes a control or a case (with its parts) off the bench.
    pub fn remove(&mut self, key: &str) -> Result<(), String> {
        self.composition
            .take(key)
            .map(|_| ())
            .ok_or_else(|| format!("nothing keyed `{key}` on the bench"))
    }

    /// Groups controls into a new case; answers its key.
    pub fn group(&mut self, keys: &[String], legend: &str) -> Result<String, String> {
        let key = loop {
            let k = format!("case-{}", self.next_case);
            self.next_case += 1;
            if !self.composition.holds(&k) {
                break k;
            }
        };
        self.composition.group(keys, legend, key.clone())?;
        Ok(key)
    }

    /// Takes everything off the bench.
    pub fn clear(&mut self) {
        self.composition = Composition::default();
    }

    /// The value of a control on the bench.
    pub fn value(&self, key: &str) -> Option<Value> {
        self.store.get(&path_of(key))
    }

    /// A control's value as it reads (`+0.67 EV`, `on`, `Filmic`).
    pub fn readout(&self, key: &str) -> String {
        let Some(v) = self.value(key) else {
            return String::new();
        };
        let control = self
            .composition
            .pieces()
            .into_iter()
            .find(|(p, _)| p.key == key)
            .and_then(|(p, _)| self.registry.get(&p.control));
        match (&v, control.and_then(|c| c.value.number())) {
            (Value::Float(_) | Value::Int(_), Some(n)) => n.format(v.as_f64().unwrap_or(0.0)),
            (Value::Bool(b), _) => if *b { "on" } else { "off" }.to_owned(),
            (Value::Choice(s) | Value::Text(s), _) => s.clone(),
            (Value::Floats(f), _) => format!("{} numbers", f.len()),
            _ => v.to_json().to_string(),
        }
    }

    /// The bench resolved in its box: each of its cards through
    /// `resolve`, with its previous resolution for hysteresis while that
    /// card is unchanged, stacked.
    pub fn resolve(&mut self) -> Result<Laid, String> {
        let sections = self.composition.sections();
        let mut resolved = Vec::new();
        for (i, (card, origins)) in sections.iter().enumerate() {
            let previous = self
                .previous
                .get(i)
                .filter(|(c, _)| c == card)
                .map(|(_, p)| p);
            resolved.push((self.resolve_card(card, previous)?, origins.clone()));
        }
        self.previous = sections
            .into_iter()
            .zip(&resolved)
            .map(|((card, _), (p, _))| (card, p.clone()))
            .collect();
        Ok(Laid::stack(resolved))
    }

    /// A composition laid out in the bench's box without keeping it (a
    /// drop's preview).
    pub fn resolve_trial(&self, composition: &Composition) -> Result<Laid, String> {
        let mut resolved = Vec::new();
        for (card, origins) in composition.sections() {
            resolved.push((self.resolve_card(&card, None)?, origins));
        }
        Ok(Laid::stack(resolved))
    }

    fn resolve_card(
        &self,
        card: &Card,
        previous: Option<&CardPlacement>,
    ) -> Result<CardPlacement, String> {
        let values = card
            .keys()
            .filter_map(|k| Some((k.to_owned(), self.value(k)?.to_json())))
            .collect();
        let moment = Moment {
            gesture_open: !self.gestures.is_empty() || self.set_gesture.is_some(),
            ..Moment::default()
        };
        helios_controls::resolve::resolve(
            &self.registry,
            card,
            &self.keys,
            &Layouts::default(),
            &values,
            &moment,
            previous,
        )
    }

    /// The control of a key on the bench.
    fn control_of(&self, key: &str) -> Option<Control> {
        let (p, _) = self
            .composition
            .pieces()
            .into_iter()
            .find(|(p, _)| p.key == key)?;
        self.registry.get(&p.control).cloned()
    }

    /// Writes a card's edits through the value flow: a number's gesture
    /// previews and commits one undo step (or cancels with no trace);
    /// another value's gesture the same, on the store's gestures.
    pub fn apply(&mut self, edits: Vec<render::Edit>) {
        for e in edits {
            match e {
                render::Edit::Number { key, change } => self.number(&key, &change),
                render::Edit::Set { key, to, done } => self.set(&key, to, done),
            }
        }
    }

    fn number(&mut self, key: &str, c: &render::Change) {
        let Some(control) = self.control_of(key) else {
            return;
        };
        let path = path_of(key);
        let open = self.gestures.remove(key);
        let outcome = if !c.done {
            let g = match open {
                Some(g) => g,
                None => match flow::begin(&mut self.store, &control, &path, POINTER) {
                    Ok(g) => g,
                    Err(_) => return,
                },
            };
            let _ = g.preview(&mut self.store, c.to, false);
            self.gestures.insert(key.to_owned(), g);
            return;
        } else if c.cancelled {
            if let Some(g) = open {
                g.cancel(&mut self.store);
            }
            "cancelled".to_owned()
        } else {
            let result = match open {
                Some(g) => {
                    let _ = g.preview(&mut self.store, c.to, c.cause == Cause::Typed);
                    Ok(g.commit(&mut self.store, &self.steps, c.cause, POINTER))
                }
                None => flow::apply(
                    &mut self.store,
                    &self.steps,
                    &control,
                    &path,
                    flow::OneShot {
                        to: c.to,
                        force: c.cause == Cause::Typed,
                        origin: POINTER,
                        cause: c.cause,
                        device: POINTER,
                    },
                ),
            };
            outcome_text(result)
        };
        self.last.insert(key.to_owned(), outcome);
    }

    fn set(&mut self, key: &str, to: Value, done: bool) {
        let Some(control) = self.control_of(key) else {
            return;
        };
        let path = path_of(key);
        let gesture = match self.set_gesture.take() {
            Some((k, g)) if k == key => g,
            other => {
                // Another key's gesture ends where this one begins.
                if let Some((_, g)) = other {
                    self.store.commit(g);
                }
                self.store.begin(&POINTER, &control.label)
            }
        };
        if let Err(e) = self.store.preview(gesture, &path, to, &POINTER, false) {
            self.store.cancel(gesture);
            self.last.insert(key.to_owned(), e);
            return;
        }
        if done {
            let before = self.store.undo_steps().len();
            self.store.commit(gesture);
            let text = if self.store.undo_steps().len() > before {
                "committed: one undo step"
            } else {
                "unchanged"
            };
            self.last.insert(key.to_owned(), text.to_owned());
        } else {
            self.set_gesture = Some((key.to_owned(), gesture));
        }
    }

    /// Undoes the last step; `false` with none (or a gesture open).
    pub fn undo(&mut self) -> bool {
        self.store.undo().is_some()
    }

    pub fn redo(&mut self) -> bool {
        self.store.redo().is_some()
    }

    /// Runs a `controls.bench.*` command.
    pub fn run(&mut self, name: &str, args: &Json) -> Result<Json, String> {
        let text = |k: &str| args.get(k).and_then(Json::as_str);
        let key = || text("key").ok_or_else(|| "`key` is required".to_owned());
        let spot = |index: Option<usize>| -> Option<Spot> {
            let case = text("case").map(str::to_owned);
            match (index, case) {
                (None, None) => None,
                (i, case) => Some(Spot {
                    index: i.unwrap_or(usize::MAX),
                    case,
                }),
            }
        };
        let index = |k: &str| {
            args.get(k)
                .and_then(Json::as_u64)
                .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
        };
        match name {
            "controls.bench.add" => {
                let control = text("control").ok_or("`control` is required")?;
                let key = self.add(control, spot(index("at")))?;
                Ok(json!({ "key": key }))
            }
            "controls.bench.remove" => {
                let key = key()?;
                self.remove(key)?;
                Ok(json!({ "removed": key }))
            }
            "controls.bench.move" => {
                let key = key()?;
                let to = spot(index("to")).ok_or("`to` or `case` is required")?;
                self.composition.place(key, &to)?;
                Ok(json!({ "key": key, "at": self.composition.find(key) }))
            }
            "controls.bench.case" => {
                let keys: Vec<String> = args
                    .get("keys")
                    .and_then(Json::as_array)
                    .ok_or("`keys` is required")?
                    .iter()
                    .filter_map(|k| k.as_str().map(str::to_owned))
                    .collect();
                let legend = text("legend").unwrap_or("Case");
                let case = self.group(&keys, legend)?;
                if let Some(c) = self.composition.case_mut(&case) {
                    c.help = text("help").map(str::to_owned);
                    if let Some(s) = args.get("separator").and_then(Json::as_bool) {
                        c.separator = s;
                    }
                }
                Ok(json!({ "key": case }))
            }
            "controls.bench.list" => self.list(),
            "controls.bench.clear" => {
                let n = self.composition.keys().len();
                self.clear();
                Ok(json!({ "removed": n }))
            }
            _ => Err(format!("no command `{name}` on the bench")),
        }
    }

    /// What `controls.bench.list` answers.
    pub fn list(&mut self) -> Result<Json, String> {
        let placement = self.resolve()?;
        let controls: Vec<Json> = self
            .composition
            .pieces()
            .into_iter()
            .map(|(p, case)| {
                let placed = placement.get(&p.key);
                json!({
                    "key": p.key,
                    "control": p.control,
                    "case": case.map(|c| c.key.clone()),
                    "value": self.value(&p.key).map(|v| v.to_json()),
                    "readout": self.readout(&p.key),
                    "shown": placed.is_some_and(|x| x.visibility.is_shown()),
                    "slot": placed.and_then(|x| x.slot).map(|s| s.units.to_string()),
                    "representation": placed.and_then(|x| x.representation),
                    "orientation": placed.and_then(|x| x.orientation),
                    "position": placed.and_then(|x| x.position),
                    "rect": placed.and_then(|x| x.rect),
                })
            })
            .collect();
        let cases: Vec<Json> = self
            .composition
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Case(c) => Some(json!({
                    "key": c.key,
                    "legend": c.legend,
                    "help": c.help,
                    "separator": c.separator,
                    "parts": c.parts.iter().map(|p| p.key.clone()).collect::<Vec<_>>(),
                })),
                Entry::Control(_) => None,
            })
            .collect();
        Ok(json!({
            "width": self.width,
            "columns": self.keys.card_box.columns(),
            "space": self.keys.space,
            "order": self.composition.keys(),
            "controls": controls,
            "cases": cases,
            "undo_steps": self.store.undo_steps(),
        }))
    }

    /// The keys selected that are controls on the bench.
    pub fn controls_among(&self, keys: &BTreeSet<String>) -> Vec<String> {
        self.composition
            .keys()
            .into_iter()
            .filter(|k| keys.contains(*k))
            .map(str::to_owned)
            .collect()
    }
}

fn outcome_text(result: Result<Outcome, String>) -> String {
    match result {
        Ok(Outcome::Committed { .. }) => "committed: one undo step".to_owned(),
        Ok(Outcome::Vetoed) => "vetoed".to_owned(),
        Ok(Outcome::Unchanged) => "unchanged".to_owned(),
        Err(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bench_of(names: &[&str]) -> Bench {
        let mut b = Bench::default();
        for n in names {
            b.add(n, None).unwrap();
        }
        b
    }

    #[test]
    fn the_shelf_lists_the_catalogue_by_purpose_then_each_value_type() {
        let b = Bench::default();
        let groups = b.shelf.groups("");
        assert_eq!(groups[0].0, "Tone");
        assert_eq!(groups[0].1[0].name, "exposure");
        let implicit = &groups.last().unwrap().1;
        let names: Vec<&str> = implicit.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "number", "whole", "toggle", "choice", "curve", "colour", "text"
            ]
        );
        for e in &b.shelf.entries {
            assert!(
                b.registry.get(&e.control).is_some(),
                "{} registered",
                e.name
            );
        }
        let found = b.shelf.groups("BOOL");
        assert!(found.is_empty(), "no entry says bool");
        let found = b.shelf.groups("toggle");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.len(), 1);
    }

    #[test]
    fn adding_twice_gives_two_keys_each_with_its_value() {
        let mut b = bench_of(&["exposure", "exposure"]);
        assert_eq!(b.composition.keys(), ["exposure", "exposure-2"]);
        assert_eq!(b.value("exposure-2"), Some(Value::Float(0.0)));
        assert!(b.add("nothing", None).is_err());
        let p = b.resolve().unwrap();
        assert!(p.get("exposure-2").unwrap().visibility.is_shown());
    }

    #[test]
    fn moves_land_where_asked_inside_and_out_of_cases() {
        let mut b = bench_of(&["exposure", "number", "toggle", "choice"]);
        b.composition.place("choice", &Spot::top(0)).unwrap();
        assert_eq!(
            b.composition.keys(),
            ["choice", "exposure", "number", "toggle"]
        );
        // Dropped before `toggle` (index 3): lands at 2 once taken out.
        let at = b.composition.landing("choice", &Spot::top(3));
        b.composition.place("choice", &at).unwrap();
        assert_eq!(
            b.composition.keys(),
            ["exposure", "number", "choice", "toggle"]
        );
        let case = b
            .group(&["number".into(), "toggle".into()], "Basics")
            .unwrap();
        assert_eq!(b.composition.entries.len(), 3);
        assert_eq!(b.composition.find("toggle"), Some(Spot::in_case(&case, 1)));
        b.composition
            .place("exposure", &Spot::in_case(&case, 0))
            .unwrap();
        assert_eq!(
            b.composition.keys(),
            ["exposure", "number", "toggle", "choice"]
        );
        assert!(
            b.composition
                .place(&case, &Spot::in_case(&case, 0))
                .is_err()
        );
        assert!(b.composition.holds(&case), "a refused move keeps the case");
        b.composition.ungroup(&case).unwrap();
        assert_eq!(b.composition.entries.len(), 4);
    }

    #[test]
    fn a_case_is_a_card_of_its_chrome_and_parts() {
        let mut b = bench_of(&["exposure", "number", "toggle"]);
        let case = b.group(&["number".into()], "Mix").unwrap();
        b.composition.case_mut(&case).unwrap().help = Some("How much".into());
        let sections = b.composition.sections();
        assert_eq!(sections.len(), 3, "exposure, the case, toggle");
        let (card, origins) = &sections[1];
        assert_eq!(card.items.len(), 4, "legend, help, part, separator");
        assert_eq!(
            origins[0],
            Origin::Chrome {
                case: case.clone(),
                kind: ChromeKind::Legend
            }
        );
        let laid = b.resolve().unwrap();
        assert!(laid.items().all(|(p, _, _)| p.visibility.is_shown()));
        // Stacked: the case's legend below exposure, toggle below the case.
        let top = |k: &str| {
            laid.items()
                .find(|(p, _, _)| p.key == k)
                .unwrap()
                .2
                .unwrap()
                .y
        };
        let legend = laid
            .items()
            .find(|(_, o, _)| {
                matches!(
                    o,
                    Origin::Chrome {
                        kind: ChromeKind::Legend,
                        ..
                    }
                )
            })
            .unwrap()
            .2
            .unwrap()
            .y;
        assert!(
            top("exposure") < legend && legend < top("number") && top("number") < top("toggle")
        );
        // Removing the last part takes the case away.
        b.remove("number").unwrap();
        assert!(!b.composition.holds(&case));
    }

    #[test]
    fn the_commands_build_a_bench_and_list_it() {
        let mut b = Bench::default();
        let k = b
            .run("controls.bench.add", &json!({"control": "exposure"}))
            .unwrap();
        assert_eq!(k["key"], "exposure");
        b.run("controls.bench.add", &json!({"control": "toggle", "at": 0}))
            .unwrap();
        b.run(
            "controls.bench.add",
            &json!({"control": "implicit:shelf/number"}),
        )
        .unwrap();
        b.run("controls.bench.move", &json!({"key": "number", "to": 0}))
            .unwrap();
        let case = b
            .run(
                "controls.bench.case",
                &json!({"keys": ["toggle", "exposure"], "legend": "Light"}),
            )
            .unwrap();
        let list = b.run("controls.bench.list", &json!({})).unwrap();
        assert_eq!(list["order"], json!(["number", "toggle", "exposure"]));
        assert_eq!(list["cases"][0]["key"], case["key"]);
        assert_eq!(list["controls"][2]["readout"], "0.00 EV");
        assert!(list["controls"][2]["slot"].is_string(), "placed in a slot");
        b.run("controls.bench.remove", &json!({"key": "number"}))
            .unwrap();
        assert!(
            b.run("controls.bench.remove", &json!({"key": "number"}))
                .is_err()
        );
        let out = b.run("controls.bench.clear", &json!({})).unwrap();
        assert_eq!(out["removed"], 2);
        assert!(b.composition.entries.is_empty());
        assert!(b.run("controls.bench.nothing", &json!({})).is_err());
    }

    #[test]
    fn a_change_goes_through_the_flow_and_undoes() {
        let mut b = bench_of(&["exposure", "toggle"]);
        let change = |to, done| render::Change {
            to,
            cause: Cause::Drag,
            done,
            cancelled: false,
        };
        b.apply(vec![
            render::Edit::Number {
                key: "exposure".into(),
                change: change(0.5, false),
            },
            render::Edit::Number {
                key: "exposure".into(),
                change: change(0.7, true),
            },
            render::Edit::Set {
                key: "toggle".into(),
                to: Value::Bool(false),
                done: true,
            },
        ]);
        assert_eq!(b.readout("exposure"), "+0.70 EV");
        assert_eq!(b.readout("toggle"), "off");
        assert_eq!(b.store.undo_steps().len(), 2);
        assert!(b.undo());
        assert_eq!(b.readout("toggle"), "on");
        assert!(b.undo());
        assert_eq!(b.readout("exposure"), "0.00 EV");
        assert!(b.redo());
        assert_eq!(b.readout("exposure"), "+0.70 EV");
    }

    #[test]
    fn the_box_follows_the_width_with_hysteresis() {
        // Alone, the slider grows across the columns it is given.
        let mut b = bench_of(&["exposure"]);
        b.set_width(240.0);
        let narrow = b.resolve().unwrap();
        b.set_width(480.0);
        let wide = b.resolve().unwrap();
        let rect = |p: &Laid, k: &str| p.get(k).unwrap().rect.unwrap();
        assert!(rect(&wide, "exposure").w > rect(&narrow, "exposure").w);
        b.set_width(5000.0);
        assert_eq!(b.width, WIDTH_RANGE.1);
    }
}
