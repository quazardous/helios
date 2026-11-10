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
//!   text and a closing separator as the case's chrome. It flattens into
//!   one [`Card`], each case a group of it (its legend the header, its
//!   help and parts the members, its separator after it), so the bench is
//!   laid out by the slot engine through `resolve` as any module's card
//!   is: densely, but never with a part pulled above its legend or a
//!   loose control into a case.
//! - [`Bench`] holds the composition, an in-memory value store
//!   ([`MemoryStore`]) with a value per control on the bench, the box's
//!   width, the previous resolution (for hysteresis), and runs the
//!   `controls.bench.*` commands ([`COMMANDS`]). Every value changes
//!   through the value flow: a gesture previews, commits one undo step or
//!   cancels with no trace.
//!
//! Bench B (slice 8) adds the switches and the measures:
//!
//! - The bench resolves in the **shared context** of the `controls.*`
//!   commands ([`SharedContexts`], the one `register_with` takes), so the
//!   context bar and `controls.context` read and set the same keys; its
//!   box follows its width, its [`BenchMode`] (the surface) and the
//!   density.
//! - A [`BenchMode`] shows the composition on a panel, a dock, a toolbar,
//!   a float, or as HUD chips (the HUD has no slots: each control's chip
//!   form, or a note that it has none).
//! - [`explain`] says, per control, its slot, orientation,
//!   representation and visibility, why, and which rule or layout decided
//!   it: the placement and reason pane, and `controls.bench.list`.
//! - A bench saves as data ([`Saved`]): the composition (tiers, variants,
//!   the card's rules), its context, its mode and width, and the person's
//!   layout under the setting key `controls.layout.bench`
//!   (`docs/design/controls.md`, section 3.9).
//! - [`Bench::sample`] is the scenarios' bench (`control-contexts.md`,
//!   section 5): develop basics and a gradient mask case, with tiers and
//!   rules.
//!
//! The drawing is in [`crate::bench_ui`].

use std::collections::{BTreeMap, BTreeSet};

use std::sync::{MutexGuard, PoisonError};

use egui_hudget::commands::{CommandInfo, Contexts, SharedContexts};
use egui_hudget::context::{Rule, Tier};
use egui_hudget::flow::{self, Cause, GestureOf, Outcome, Steps};
use egui_hudget::model::{Control, Representation, ValueSpec, ValueType, representation_name};
use egui_hudget::resolve::{Layout, Layouts, Placement, Source, Visibility};
use egui_hudget::slot::{Chrome, ChromeKind, Orientation};
use egui_hudget::value::{MemoryStore, ParamDescriptor, ParamType, Value, ValueSource, ValueStore};
use egui_hudget::{CardPlacement, ContextKeys, Registry};
use helios_controls::card::{Card, CardGroup, CardItem};
use helios_controls::context::CardBox;
use helios_controls::render;
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

/// The sample parameters of the scenarios' bench (`control-contexts.md`,
/// section 5): a develop card's black level and contrast, and a gradient
/// mask's feather, angle and profile; each with its shelf group.
fn sample_descriptors() -> Vec<(&'static str, &'static str, ParamDescriptor)> {
    let mut black = ParamDescriptor::float("Black level", -0.1, 0.1, 0.0);
    black.step = 0.001;
    black.coarse = 0.01;
    black.precision = 3;
    let contrast = ParamDescriptor::float("Contrast", -1.0, 1.0, 0.0);
    let mut feather = ParamDescriptor::float("Feather", 0.0, 200.0, 20.0);
    feather.unit = Some("px".into());
    feather.step = 1.0;
    feather.coarse = 10.0;
    feather.precision = 0;
    let mut angle = ParamDescriptor::float("Angle", -180.0, 180.0, 0.0);
    angle.unit = Some("°".into());
    angle.step = 1.0;
    angle.coarse = 15.0;
    angle.precision = 0;
    let mut profile =
        ParamDescriptor::new(ParamType::Choice, "Profile", Value::Choice("Linear".into()));
    profile.choices = vec!["Linear".into(), "Smooth".into(), "Sigmoid".into()];
    vec![
        ("black", SAMPLE_DEVELOP, black),
        ("contrast", SAMPLE_DEVELOP, contrast),
        ("feather", SAMPLE_MASK, feather),
        ("angle", SAMPLE_MASK, angle),
        ("profile", SAMPLE_MASK, profile),
    ]
}

/// The shelf group of the develop card's sample parameters.
pub const SAMPLE_DEVELOP: &str = "Sample: develop basics";
/// The shelf group of the gradient mask's sample parameters.
pub const SAMPLE_MASK: &str = "Sample: gradient mask";

/// What can go on the bench, in shelf order: business controls grouped by
/// purpose, the scenarios' sample parameters, then the implicit controls
/// by value type.
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
        for (name, group, d) in sample_descriptors() {
            let control = egui_hudget::implicit(&format!("shelf/{name}"), &d);
            entries.push(ShelfEntry {
                name: name.to_owned(),
                control: control.id.clone(),
                label: d.label.clone(),
                group: group.to_owned(),
                hint: "Sample parameter of the scenarios".to_owned(),
                descriptor: d,
            });
            let _ = registry.register(control);
        }
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

/// A control on the bench: its key there, its control's id, the variant
/// picked (the default when `None`) and how rare it is (its tier).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Piece {
    pub key: String,
    pub control: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Tier::is_standard")]
    pub tier: Tier,
}

impl Piece {
    /// A control in its default variant, at the standard tier.
    pub fn new(key: &str, control: &str) -> Self {
        Self {
            key: key.into(),
            control: control.into(),
            variant: None,
            tier: Tier::Standard,
        }
    }

    /// Its card item.
    fn item(&self) -> CardItem {
        let item = CardItem::control(&self.key, &self.control).tier(self.tier);
        match &self.variant {
            Some(v) => item.variant(v),
            None => item,
        }
    }
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

/// What is on the bench, in logical order, and the card's rules over the
/// context keys. Saved as data ([`Saved`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    pub entries: Vec<Entry>,
    /// The card's rules; a part a rule names that is not on the bench is
    /// left out of it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
}

/// A rule with only the parts on the bench.
fn prune(rule: &Rule, keys: &[&str]) -> Rule {
    let mut r = rule.clone();
    for list in [
        &mut r.effect.fold,
        &mut r.effect.show,
        &mut r.effect.inapplicable,
        &mut r.effect.emphasise,
        &mut r.effect.hud,
    ] {
        list.retain(|k| keys.contains(&k.as_str()));
    }
    r
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

    /// A control on the bench, to change its tier or variant.
    pub fn piece_mut(&mut self, key: &str) -> Option<&mut Piece> {
        self.entries.iter_mut().find_map(|e| match e {
            Entry::Control(p) if p.key == key => Some(p),
            Entry::Case(c) => c.parts.iter_mut().find(|p| p.key == key),
            Entry::Control(_) => None,
        })
    }

    /// Every control, in logical order, with its case.
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

    /// One card of every control and every case's chrome, in logical
    /// order, and where each card item comes from.
    pub fn card(&self) -> (Card, Vec<Origin>) {
        let mut items = Vec::new();
        let mut origins = Vec::new();
        let mut groups = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            match e {
                Entry::Control(p) => {
                    items.push(p.item());
                    origins.push(Origin::Control {
                        key: p.key.clone(),
                        spot: Spot::top(i),
                    });
                }
                Entry::Case(c) => {
                    let start = items.len();
                    let mut chrome = |kind, text: Option<&str>| {
                        let mut ch = Chrome::new(kind);
                        if let Some(t) = text {
                            ch = ch.text(t);
                        }
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
                        items.push(p.item());
                        origins.push(Origin::Control {
                            key: p.key.clone(),
                            spot: Spot::in_case(&c.key, j),
                        });
                    }
                    if c.separator {
                        let mut ch = Chrome::new(ChromeKind::Separator);
                        ch.text = None;
                        items.push(CardItem::Chrome(ch));
                        origins.push(Origin::Chrome {
                            case: c.key.clone(),
                            kind: ChromeKind::Separator,
                        });
                    }
                    groups.push(CardGroup::of_case(&c.key, start, &items[start..]));
                }
            }
        }
        let mut card = Card::new(items);
        card.groups = groups;
        let keys: Vec<&str> = self.keys();
        card.rules = self.rules.iter().map(|r| prune(r, &keys)).collect();
        (card, origins)
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
        name: "controls.bench.part",
        help: "Changes a control on the bench: {key}, optional {tier} (essential, standard, expert) and {variant} (its declaration), {visibility} (shown, hidden, or null) and {representation} (full, compact, simplified, or null), pinned in the person's layout",
        read_only: false,
    },
    CommandInfo {
        name: "controls.bench.rules",
        help: "Sets the bench card's rules over the context keys: {rules}, a list of {when, effect, reason, source}",
        read_only: false,
    },
    CommandInfo {
        name: "controls.bench.mode",
        help: "Shows the bench as a {mode}: panel, dock, toolbar, hud or float; optional {width} in points. The box follows (set the other keys with `controls.context`)",
        read_only: false,
    },
    CommandInfo {
        name: "controls.bench.list",
        help: "What is on the bench: its mode, width and context, each control (key, control, case, tier, value, placement: slot, representation, orientation, rect, visibility, why and what decided it) and each case",
        read_only: true,
    },
    CommandInfo {
        name: "controls.bench.save",
        help: "Saves the bench (composition, context, mode, width, and the person's layout as the `controls.layout.bench` setting); answers it as {bench}, and writes it to {path} when given",
        read_only: true,
    },
    CommandInfo {
        name: "controls.bench.load",
        help: "Loads a saved bench, from {bench} (what `controls.bench.save` answers) or from the file at {path}; its values go back to their defaults",
        read_only: false,
    },
    CommandInfo {
        name: "controls.bench.sample",
        help: "Loads the scenarios' bench: develop basics and a gradient mask case, with tiers and rules",
        read_only: false,
    },
    CommandInfo {
        name: "controls.bench.clear",
        help: "Takes everything off the bench",
        read_only: false,
    },
];

/// How the bench shows its composition: on a boxed surface (a panel, a
/// dock, a toolbar, a float), or as HUD chips.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BenchMode {
    #[default]
    Panel,
    Dock,
    Toolbar,
    Hud,
    Float,
}

impl BenchMode {
    pub const ALL: [BenchMode; 5] = [
        BenchMode::Panel,
        BenchMode::Dock,
        BenchMode::Toolbar,
        BenchMode::Hud,
        BenchMode::Float,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BenchMode::Panel => "Panel",
            BenchMode::Dock => "Dock",
            BenchMode::Toolbar => "Toolbar",
            BenchMode::Hud => "HUD",
            BenchMode::Float => "Float",
        }
    }

    /// The surface the composition is laid out on.
    pub fn surface(self) -> Surface {
        match self {
            BenchMode::Panel => Surface::Panel,
            BenchMode::Dock => Surface::Dock,
            BenchMode::Toolbar => Surface::Toolbar,
            BenchMode::Hud => Surface::Hud,
            BenchMode::Float => Surface::Float,
        }
    }

    /// Whether it lays controls out in slots (every mode but the HUD).
    pub fn boxed(self) -> bool {
        self.surface().is_boxed()
    }
}

/// A control's HUD and image forms, as declared (the HUD and the image
/// have no slots: a chip, a gizmo).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HudForm {
    pub key: String,
    pub label: String,
    /// Its variant declares a HUD chip.
    pub chip: bool,
    /// Its variant declares a gizmo on the image.
    pub gizmo: bool,
    /// What the chip reads.
    pub readout: String,
}

/// One control's placement, explained: the placement and reason pane.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Explained {
    pub key: String,
    pub label: String,
    /// Its slot (`1x1`) and the columns it spans, when placed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<Orientation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
    /// `shown`, `folded` or `not applicable`.
    pub visibility: &'static str,
    /// Why it is there, or not.
    pub why: String,
    /// The rule, layout or engine that decided it.
    pub decided_by: String,
    /// The other reasons it carries (a fallback, a reveal, a wait).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub deferred: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub emphasised: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub changed: bool,
}

/// Who a reason's source is, as the pane reads it.
fn source_name(source: Source) -> &'static str {
    match source {
        Source::Detail => "the detail level, against its tier",
        Source::Group => "a rule of the card",
        Source::Profile => "the profile",
        Source::Layout => "the person's layout",
        Source::Size => "the slot engine: no room in the box",
        Source::Changed => "a changed value is never folded",
        Source::Revealed => "revealed for the moment",
        Source::Deferred => "anti-jump: waits for the pointer to leave",
        Source::Fallback => "the physical limits: a fallback",
    }
}

fn reason_line(r: &egui_hudget::resolve::Reason) -> String {
    match &r.when {
        Some(when) => format!("{} (when `{when}`)", r.text),
        None => r.text.clone(),
    }
}

/// One placement, explained: its slot and form, its visibility, why, and
/// what decided it; `columns` is the box's width in slot columns.
pub fn explain(p: &Placement, columns: f32) -> Explained {
    let label = p.label.clone().unwrap_or_else(|| p.key.clone());
    let notes: Vec<String> = p
        .reasons
        .iter()
        .map(|r| format!("{}: {}", source_name(r.source), reason_line(r)))
        .collect();
    let rep = p.representation.map_or("none", representation_name);
    let slot = p.slot.map(|s| {
        format!(
            "{} · {} column{}",
            s.units,
            s.span,
            if s.span == 1 { "" } else { "s" }
        )
    });
    let (visibility, why, decided_by) = match &p.visibility {
        Visibility::Shown => {
            let decided = match p.reasons.last() {
                Some(r) => source_name(r.source).to_owned(),
                None => "the declaration and the slot engine (size policy)".to_owned(),
            };
            let why = format!(
                "{rep} in {} of a {columns:.1}-column box",
                slot.as_deref().unwrap_or("no slot")
            );
            ("shown", why, decided)
        }
        Visibility::Folded { reason } => (
            "folded",
            reason_line(reason),
            source_name(reason.source).to_owned(),
        ),
        Visibility::NotApplicable { reason } => (
            "not applicable",
            reason_line(reason),
            source_name(reason.source).to_owned(),
        ),
    };
    Explained {
        key: p.key.clone(),
        label,
        slot,
        orientation: p.orientation,
        representation: p.representation,
        visibility,
        why,
        decided_by,
        notes,
        deferred: p.deferred,
        emphasised: p.emphasised,
        changed: p.changed,
    }
}

/// The setting key of the bench card's layout (`controls.layout.<card>`).
pub const LAYOUT_KEY: &str = "controls.layout.bench";

/// What a saved bench says it is.
pub const FORMAT: &str = "helios-controls-bench/1";

/// A bench as data: the composition, its context, its mode and width, and
/// the person's settings for its card (its layout under [`LAYOUT_KEY`],
/// in the Layout settings format of `controls.md`, section 3.9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub format: String,
    pub mode: BenchMode,
    pub width: f32,
    /// The keys a person sets: task, detail, image, tool, device, density
    /// (`controls.context`'s names; the space follows the box).
    pub context: Json,
    pub composition: Composition,
    #[serde(default)]
    pub settings: BTreeMap<String, Json>,
}

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
    /// The surface it shows the composition on.
    pub mode: BenchMode,
    /// The person's layout of the bench's card: pins per part.
    pub layout: Layout,
    /// The pointer is over the bench (the drawing says): context changes
    /// wait until it leaves.
    pub pointer_in: bool,
    /// The context shared with the `controls.*` commands.
    contexts: SharedContexts,
    /// The last resolution and the card it was of: hysteresis holds while
    /// the card is the same.
    previous: Option<(Card, CardPlacement)>,
    next_case: u32,
}

impl Default for Bench {
    fn default() -> Self {
        Self::new(Registry::standard())
    }
}

impl Bench {
    /// An empty bench over a registry's controls, in a context of its own.
    pub fn new(registry: Registry) -> Self {
        Self::with_contexts(registry, SharedContexts::default())
    }

    /// An empty bench resolved in a context it shares with the
    /// `controls.*` commands (`controls.context` sets its keys).
    pub fn with_contexts(registry: Registry, contexts: SharedContexts) -> Self {
        let (shelf, registry) = Shelf::of(registry);
        let bench = Self {
            registry,
            shelf,
            composition: Composition::default(),
            store: MemoryStore::new(),
            steps: Steps::new(),
            gestures: BTreeMap::new(),
            set_gesture: None,
            last: BTreeMap::new(),
            width: DEFAULT_WIDTH,
            mode: BenchMode::default(),
            layout: Layout::default(),
            pointer_in: false,
            contexts,
            previous: None,
            next_case: 1,
        };
        bench.sync_box();
        bench
    }

    /// The scenarios' bench (`control-contexts.md`, section 5): exposure,
    /// black level and contrast, then a "Gradient mask" case of feather,
    /// angle and profile. Exposure and feather are essential, the profile
    /// expert; contrast and black level fold while retouching; the black
    /// level does not apply to a picture (it is raw only); the gradient
    /// tool emphasises the mask's parts.
    pub fn sample(registry: Registry) -> Self {
        let mut b = Self::new(registry);
        b.load_sample();
        b
    }

    /// Replaces what is on the bench with the scenarios' bench.
    pub fn load_sample(&mut self) {
        let piece = |key: &str, tier| Piece {
            tier,
            ..Piece::new(key, &format!("implicit:shelf/{key}"))
        };
        let composition = Composition {
            entries: vec![
                Entry::Control(Piece {
                    tier: Tier::Essential,
                    ..Piece::new("exposure", helios_controls::catalogue::EXPOSURE)
                }),
                Entry::Control(piece("black", Tier::Standard)),
                Entry::Control(piece("contrast", Tier::Standard)),
                Entry::Case(BenchCase {
                    key: "gradient".into(),
                    legend: "Gradient mask".into(),
                    help: None,
                    separator: true,
                    parts: vec![
                        piece("feather", Tier::Essential),
                        piece("angle", Tier::Standard),
                        piece("profile", Tier::Expert),
                    ],
                }),
            ],
            rules: vec![
                Rule::new("ctl.task == 'retouch'")
                    .because("Folded while retouching")
                    .fold(&["black", "contrast"]),
                Rule::new("!ctl.image.raw")
                    .because("Raw only")
                    .inapplicable(&["black"]),
                Rule::new("ctl.tool == 'gradient'").emphasise(&["feather", "angle"]),
            ],
        };
        let saved = Saved {
            format: FORMAT.into(),
            mode: self.mode,
            width: self.width,
            context: json!({}),
            composition,
            settings: BTreeMap::new(),
        };
        // The sample is built from the shelf: it always loads.
        let _ = self.load(saved);
    }

    fn lock(&self) -> MutexGuard<'_, Contexts> {
        self.contexts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The context it shares with the `controls.*` commands.
    pub fn contexts(&self) -> SharedContexts {
        self.contexts.clone()
    }

    /// The box from the width, the mode's surface and the density; the
    /// space follows with its hysteresis.
    fn sync_box(&self) {
        let mut c = self.lock();
        let b = CardBox::from_points(self.mode.surface(), self.width, 2000.0, c.keys.density);
        if c.keys.card_box != b {
            c.keys.set_box(b);
        }
    }

    /// The context keys the bench is resolved in (the box from its width
    /// and mode).
    pub fn keys(&self) -> ContextKeys {
        self.sync_box();
        self.lock().keys.clone()
    }

    /// Sets context keys as `controls.context` does (`task`, `detail`,
    /// `image`, `tool`, `device`, `density`…); a task change ends the
    /// reveals. The box stays the bench's.
    pub fn set_context(&self, args: &Json) -> Result<(), String> {
        let r = egui_hudget::commands::set_context(&mut self.lock(), args);
        self.sync_box();
        r
    }

    /// Sets the bench's width, held to [`WIDTH_RANGE`]; the box follows,
    /// the space key with its hysteresis.
    pub fn set_width(&mut self, width: f32) {
        self.width = width.clamp(WIDTH_RANGE.0, WIDTH_RANGE.1);
        self.sync_box();
    }

    /// Shows the composition in another mode; the box follows.
    pub fn set_mode(&mut self, mode: BenchMode) {
        self.mode = mode;
        self.sync_box();
    }

    /// Shows a folded part for the moment (until the task changes), as
    /// `controls.reveal` does.
    pub fn reveal(&self, key: &str) {
        self.lock()
            .moments
            .entry(NODE.to_owned())
            .or_default()
            .revealed
            .insert(key.to_owned());
    }

    /// Shows every part for the moment, as `controls.reveal_all` does.
    pub fn reveal_all(&self) {
        self.lock()
            .moments
            .entry(NODE.to_owned())
            .or_default()
            .reveal_all = true;
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
        self.composition
            .insert(Entry::Control(Piece::new(&key, &entry.control)), &spot)?;
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

    /// Takes everything off the bench, its rules and layout too.
    pub fn clear(&mut self) {
        self.composition = Composition::default();
        self.layout = Layout::default();
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
        match (
            &v,
            self.control_of(key).and_then(|c| c.value.number().cloned()),
        ) {
            (Value::Float(_) | Value::Int(_), Some(n)) => n.format(v.as_f64().unwrap_or(0.0)),
            (Value::Bool(b), _) => if *b { "on" } else { "off" }.to_owned(),
            (Value::Choice(s) | Value::Text(s), _) => s.clone(),
            (Value::Floats(f), _) => format!("{} numbers", f.len()),
            _ => v.to_json().to_string(),
        }
    }

    /// The bench resolved in its box: the card of its composition through
    /// `resolve`, with the previous resolution for hysteresis and
    /// anti-jump while the card is unchanged; and where each item comes
    /// from. Refused in the HUD, which has no slots ([`Bench::hud`]).
    pub fn resolve(&mut self) -> Result<(CardPlacement, Vec<Origin>), String> {
        let (card, origins) = self.composition.card();
        let p = self.resolve_card(&card, true)?;
        self.previous = Some((card, p.clone()));
        Ok((p, origins))
    }

    /// A composition resolved in the bench's box without keeping it (a
    /// drop's preview).
    pub fn resolve_trial(&self, composition: &Composition) -> Result<CardPlacement, String> {
        let (card, _) = composition.card();
        self.resolve_card(&card, false)
    }

    fn resolve_card(&self, card: &Card, keep: bool) -> Result<CardPlacement, String> {
        if !self.mode.boxed() {
            return Err(format!(
                "The {} has no slots: each control shows as its chip",
                self.mode.label()
            ));
        }
        self.sync_box();
        let values = card
            .keys()
            .filter_map(|k| Some((k.to_owned(), self.value(k)?.to_json())))
            .collect();
        let (keys, moment) = {
            let c = self.lock();
            let mut moment = c.moments.get(NODE).cloned().unwrap_or_default();
            moment.gesture_open =
                !self.gestures.is_empty() || self.set_gesture.is_some() || c.hand.gesture_open;
            moment.pointer_in_panel = self.pointer_in || c.hand.pointer_in_panel;
            moment.handled.clone_from(&c.hand.handled);
            (c.keys.clone(), moment)
        };
        let previous = self
            .previous
            .as_ref()
            .filter(|(c, _)| keep && c == card)
            .map(|(_, p)| p);
        helios_controls::resolve::resolve(
            &self.registry,
            card,
            &keys,
            &Layouts::person(self.layout.clone()),
            &values,
            &moment,
            previous,
        )
    }

    /// Each control's HUD and image forms: the HUD has no slots, a control
    /// shows there as its chip when its variant declares one.
    pub fn hud(&self) -> Vec<HudForm> {
        self.composition
            .pieces()
            .into_iter()
            .map(|(p, _)| {
                let c = self.registry.get(&p.control);
                let v = c.and_then(|c| match &p.variant {
                    Some(id) => c.variants.iter().find(|v| &v.id == id),
                    None => c.variants.first(),
                });
                HudForm {
                    key: p.key.clone(),
                    label: c.map_or_else(|| p.key.clone(), |c| c.label.clone()),
                    chip: v.is_some_and(|v| v.chip),
                    gizmo: v.is_some_and(|v| v.gizmo),
                    readout: self.readout(&p.key),
                }
            })
            .collect()
    }

    /// The last resolution's controls, explained (the reason pane).
    pub fn explain(&self, placement: &CardPlacement) -> Vec<Explained> {
        let columns = placement.keys.card_box.columns();
        placement.controls().map(|p| explain(p, columns)).collect()
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

    /// Steps a number on the bench by `steps` of its step, through the
    /// flow (one undo step), as a device's encoder or `controls.nudge`
    /// would; answers how it ended.
    pub fn nudge(&mut self, key: &str, steps: i64, device: &'static str) -> Result<String, String> {
        let control = self
            .control_of(key)
            .ok_or_else(|| format!("nothing keyed `{key}` on the bench"))?;
        let now = self
            .value(key)
            .and_then(|v| v.as_f64())
            .ok_or_else(|| format!("`{key}` is not a number"))?;
        let to = control.nudged(now, steps, false, false);
        let result = flow::apply(
            &mut self.store,
            &self.steps,
            &control,
            &path_of(key),
            flow::OneShot {
                to,
                force: false,
                origin: device,
                cause: Cause::Nudge,
                device,
            },
        );
        let text = outcome_text(result);
        self.last.insert(key.to_owned(), text.clone());
        Ok(text)
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

    /// Changes a control on the bench: its tier and variant (its
    /// declaration), and its visibility and representation pinned in the
    /// person's layout (`null` unpins). Fields left out stay.
    pub fn part(&mut self, key: &str, args: &Json) -> Result<(), String> {
        let field = |k: &str| args.get(k);
        let piece = self
            .composition
            .piece_mut(key)
            .ok_or_else(|| format!("no control `{key}` on the bench"))?;
        if let Some(t) = field("tier") {
            piece.tier = serde_json::from_value(t.clone()).map_err(|e| format!("`tier`: {e}"))?;
        }
        if let Some(v) = field("variant") {
            let id: Option<String> =
                serde_json::from_value(v.clone()).map_err(|e| format!("`variant`: {e}"))?;
            if let Some(id) = &id
                && !self
                    .registry
                    .get(&piece.control)
                    .is_some_and(|c| c.variants.iter().any(|v| &v.id == id))
            {
                return Err(format!("`{}` has no variant `{id}`", piece.control));
            }
            piece.variant = id;
        }
        let part = self.layout.parts.entry(key.to_owned()).or_default();
        if let Some(v) = field("visibility") {
            part.visibility =
                serde_json::from_value(v.clone()).map_err(|e| format!("`visibility`: {e}"))?;
        }
        if let Some(r) = field("representation") {
            part.representation =
                serde_json::from_value(r.clone()).map_err(|e| format!("`representation`: {e}"))?;
        }
        if *part == egui_hudget::resolve::PartLayout::default() {
            self.layout.parts.remove(key);
        }
        Ok(())
    }

    /// The bench as data: the composition, the context a person sets, the
    /// mode and width, and the person's layout under [`LAYOUT_KEY`] (its
    /// order the composition's).
    pub fn save(&self) -> Saved {
        let k = self.keys();
        let mut layout = self.layout.clone();
        layout.order = self
            .composition
            .keys()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut settings = BTreeMap::new();
        settings.insert(
            LAYOUT_KEY.to_owned(),
            serde_json::to_value(&layout).unwrap_or(Json::Null),
        );
        Saved {
            format: FORMAT.into(),
            mode: self.mode,
            width: self.width,
            context: json!({
                "task": k.task,
                "detail": k.detail,
                "image": k.image,
                "tool": k.tool,
                "device": k.device,
                "density": k.density,
            }),
            composition: self.composition.clone(),
            settings,
        }
    }

    /// Loads a saved bench: every control must be on the shelf (and its
    /// variant declared); the values go back to their defaults. Refused,
    /// it leaves the bench as it was.
    pub fn load(&mut self, saved: Saved) -> Result<(), String> {
        if saved.format != FORMAT {
            return Err(format!(
                "not a saved bench: format `{}`, not `{FORMAT}`",
                saved.format
            ));
        }
        let mut descriptors = Vec::new();
        for (p, _) in saved.composition.pieces() {
            let e = self
                .shelf
                .get(&p.control)
                .ok_or_else(|| format!("`{}`: no control `{}` on the shelf", p.key, p.control))?;
            if let Some(id) = &p.variant
                && !self
                    .registry
                    .get(&p.control)
                    .is_some_and(|c| c.variants.iter().any(|v| &v.id == id))
            {
                return Err(format!(
                    "`{}`: `{}` has no variant `{id}`",
                    p.key, p.control
                ));
            }
            descriptors.push((p.key.clone(), e.descriptor.clone()));
        }
        let (card, _) = saved.composition.card();
        card.check()?;
        let layout: Layout = match saved.settings.get(LAYOUT_KEY) {
            Some(l) => {
                serde_json::from_value(l.clone()).map_err(|e| format!("{LAYOUT_KEY}: {e}"))?
            }
            None => Layout::default(),
        };
        // Checked first: a bad context leaves the bench as it was.
        let mut trial = Contexts::default();
        egui_hudget::commands::set_context(&mut trial, &saved.context)?;
        self.set_context(&saved.context)?;
        self.store = MemoryStore::new();
        for (key, d) in descriptors {
            self.store.declare(&path_of(&key), d);
        }
        self.gestures.clear();
        self.set_gesture = None;
        self.last.clear();
        self.previous = None;
        self.composition = saved.composition;
        self.layout = layout;
        self.lock().moments.remove(NODE);
        while self.composition.holds(&format!("case-{}", self.next_case)) {
            self.next_case += 1;
        }
        self.mode = saved.mode;
        self.set_width(saved.width);
        Ok(())
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
            "controls.bench.part" => {
                let key = key()?.to_owned();
                self.part(&key, args)?;
                Ok(json!({ "key": key, "layout": self.layout.parts.get(&key) }))
            }
            "controls.bench.rules" => {
                let rules: Vec<Rule> = serde_json::from_value(
                    args.get("rules").cloned().ok_or("`rules` is required")?,
                )
                .map_err(|e| format!("`rules`: {e}"))?;
                let mut trial = self.composition.clone();
                trial.rules = rules;
                trial.card().0.check()?;
                self.composition = trial;
                Ok(json!({ "rules": self.composition.rules.len() }))
            }
            "controls.bench.mode" => {
                let mode: BenchMode =
                    serde_json::from_value(args.get("mode").cloned().ok_or("`mode` is required")?)
                        .map_err(|e| format!("`mode`: {e}"))?;
                self.set_mode(mode);
                if let Some(w) = args.get("width").and_then(Json::as_f64) {
                    self.set_width(w as f32);
                }
                Ok(json!({ "mode": self.mode, "width": self.width, "keys": self.keys().report() }))
            }
            "controls.bench.list" => self.list(),
            "controls.bench.save" => {
                let saved = serde_json::to_value(self.save()).map_err(|e| e.to_string())?;
                if let Some(path) = text("path") {
                    let body = serde_json::to_string_pretty(&saved).map_err(|e| e.to_string())?;
                    std::fs::write(path, body + "\n").map_err(|e| format!("{path}: {e}"))?;
                }
                Ok(json!({ "bench": saved, "path": text("path") }))
            }
            "controls.bench.load" => {
                let saved = match (args.get("bench"), text("path")) {
                    (Some(b), _) => b.clone(),
                    (None, Some(path)) => {
                        let body =
                            std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
                        serde_json::from_str(&body).map_err(|e| format!("{path}: {e}"))?
                    }
                    (None, None) => return Err("`bench` or `path` is required".into()),
                };
                let saved: Saved =
                    serde_json::from_value(saved).map_err(|e| format!("not a saved bench: {e}"))?;
                self.load(saved)?;
                Ok(json!({ "order": self.composition.keys(), "mode": self.mode }))
            }
            "controls.bench.sample" => {
                self.load_sample();
                Ok(json!({ "order": self.composition.keys() }))
            }
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
        let placement = if self.mode.boxed() {
            Some(self.resolve()?.0)
        } else {
            None
        };
        let hud = self.hud();
        let controls: Vec<Json> = self
            .composition
            .pieces()
            .into_iter()
            .map(|(p, case)| {
                let placed = placement.as_ref().and_then(|x| x.get(&p.key));
                let explained = placed.map(|x| explain(x, self.keys().card_box.columns()));
                json!({
                    "key": p.key,
                    "control": p.control,
                    "variant": p.variant,
                    "tier": p.tier,
                    "case": case.map(|c| c.key.clone()),
                    "value": self.value(&p.key).map(|v| v.to_json()),
                    "readout": self.readout(&p.key),
                    "shown": placed.is_some_and(|x| x.visibility.is_shown()),
                    "slot": placed.and_then(|x| x.slot).map(|s| s.units.to_string()),
                    "representation": placed.and_then(|x| x.representation),
                    "orientation": placed.and_then(|x| x.orientation),
                    "position": placed.and_then(|x| x.position),
                    "rect": placed.and_then(|x| x.rect),
                    "explained": explained,
                    "hud": hud.iter().find(|h| h.key == p.key),
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
        let keys = self.keys();
        Ok(json!({
            "mode": self.mode,
            "width": self.width,
            "columns": keys.card_box.columns(),
            "space": keys.space,
            "context": keys.report(),
            "order": self.composition.keys(),
            "controls": controls,
            "cases": cases,
            "more": placement.as_ref().and_then(|p| p.more.clone()),
            "pending": placement.as_ref().is_some_and(|p| p.pending),
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
        let (p, _) = b.resolve().unwrap();
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
    fn a_case_is_a_group_of_its_chrome_and_parts() {
        let mut b = bench_of(&["exposure", "number", "toggle"]);
        let case = b.group(&["number".into()], "Mix").unwrap();
        b.composition.case_mut(&case).unwrap().help = Some("How much".into());
        let (card, origins) = b.composition.card();
        assert_eq!(
            card.items.len(),
            6,
            "control, legend, help, part, separator, control"
        );
        assert_eq!(
            origins[1],
            Origin::Chrome {
                case: case.clone(),
                kind: ChromeKind::Legend
            }
        );
        // The legend heads the group; the separator follows it.
        assert_eq!(
            card.groups,
            [CardGroup {
                key: case.clone(),
                items: 1..4,
                header: true
            }]
        );
        let (p, _) = b.resolve().unwrap();
        assert!(p.items.iter().all(|i| i.visibility.is_shown()));
        // In order, one card: exposure, then the case's band (its legend
        // above its part), then the toggle below it.
        let rect = |k: &str| p.get(k).unwrap().rect.unwrap();
        let band = p
            .layout
            .as_ref()
            .unwrap()
            .group(&case)
            .unwrap()
            .rect
            .unwrap();
        let legend = p.items[1].rect.unwrap();
        assert!(rect("exposure").y + rect("exposure").h <= band.y);
        assert_eq!(legend.y, band.y);
        assert!(legend.y < rect("number").y && rect("number").y < band.y + band.h);
        assert!(rect("toggle").y >= band.y + band.h);
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
        let (narrow, _) = b.resolve().unwrap();
        b.set_width(480.0);
        let (wide, _) = b.resolve().unwrap();
        let rect = |p: &CardPlacement, k: &str| p.get(k).unwrap().rect.unwrap();
        assert!(rect(&wide, "exposure").w > rect(&narrow, "exposure").w);
        b.set_width(5000.0);
        assert_eq!(b.width, WIDTH_RANGE.1);
    }

    #[test]
    fn the_sample_folds_by_tier_rule_and_relevance_and_says_why() {
        let mut b = Bench::default();
        b.load_sample();
        assert_eq!(
            b.composition.keys(),
            [
                "exposure", "black", "contrast", "feather", "angle", "profile"
            ]
        );
        // At the standard detail, the expert profile folds.
        let (p, _) = b.resolve().unwrap();
        let why = b.explain(&p);
        let profile = why.iter().find(|e| e.key == "profile").unwrap();
        assert_eq!(profile.visibility, "folded");
        assert_eq!(profile.decided_by, "the detail level, against its tier");
        let exposure = why.iter().find(|e| e.key == "exposure").unwrap();
        assert_eq!(exposure.visibility, "shown");
        assert!(exposure.slot.is_some() && exposure.representation.is_some());
        // Retouching folds contrast and black level by the card's rule.
        b.set_context(&json!({"task": "retouch"})).unwrap();
        let (p, _) = b.resolve().unwrap();
        let contrast = explain(p.get("contrast").unwrap(), 2.0);
        assert_eq!(contrast.visibility, "folded");
        assert_eq!(contrast.decided_by, "a rule of the card");
        assert!(
            contrast.why.contains("ctl.task == 'retouch'"),
            "{}",
            contrast.why
        );
        // A picture: the black level does not apply.
        b.set_context(
            &json!({"task": "develop", "image": {"raw": false, "edited": false, "masked": false}}),
        )
        .unwrap();
        let (p, _) = b.resolve().unwrap();
        assert_eq!(
            explain(p.get("black").unwrap(), 2.0).visibility,
            "not applicable"
        );
        egui_hudget::resolve::reachable(&p).unwrap();
        // Every slow-key combination keeps every control reachable.
        let (card, _) = b.composition.card();
        egui_hudget::resolve::reachable_everywhere(
            &b.registry,
            &card,
            Surface::Panel,
            &Layouts::person(b.layout.clone()),
            &BTreeMap::new(),
        )
        .unwrap();
    }

    #[test]
    fn a_bench_saves_and_loads_with_its_context_and_layout() {
        let mut b = Bench::default();
        b.load_sample();
        b.run(
            "controls.bench.part",
            &json!({"key": "exposure", "variant": "scrub", "representation": "compact"}),
        )
        .unwrap();
        b.run(
            "controls.bench.part",
            &json!({"key": "angle", "visibility": "hidden"}),
        )
        .unwrap();
        b.run(
            "controls.bench.mode",
            &json!({"mode": "dock", "width": 340}),
        )
        .unwrap();
        b.set_context(&json!({"task": "retouch", "detail": "expert", "density": "compact"}))
            .unwrap();
        let saved = b.run("controls.bench.save", &json!({})).unwrap()["bench"].clone();
        assert_eq!(saved["format"], FORMAT);
        assert_eq!(
            saved["settings"][LAYOUT_KEY]["parts"]["angle"]["visibility"],
            "hidden"
        );
        assert_eq!(saved["settings"][LAYOUT_KEY]["order"][0], "exposure");
        assert_eq!(saved["context"]["task"], "retouch");

        let mut other = Bench::default();
        other.add("toggle", None).unwrap();
        other
            .run("controls.bench.load", &json!({"bench": saved}))
            .unwrap();
        assert_eq!(other.composition, b.composition);
        assert_eq!(other.mode, BenchMode::Dock);
        assert!((other.width - 340.0).abs() < 0.5);
        let k = other.keys();
        assert_eq!(k.task, egui_hudget::context::Task::Retouch);
        assert_eq!(k.density, egui_hudget::context::Density::Compact);
        assert_eq!(k.card_box.surface, Surface::Dock);
        let (p, _) = other.resolve().unwrap();
        assert!(matches!(
            p.get("angle").unwrap().visibility,
            Visibility::Folded { .. }
        ));
        assert_eq!(other.value("feather"), Some(Value::Float(20.0)));
        assert_eq!(other.save(), b.save());

        // Refused: the bench stays as it was.
        let mut bad = saved_of(&b);
        bad["composition"]["entries"][0]["control"] = json!("nothing");
        assert!(
            other
                .run("controls.bench.load", &json!({"bench": bad}))
                .is_err()
        );
        let mut bad = saved_of(&b);
        bad["format"] = json!("other");
        assert!(
            other
                .run("controls.bench.load", &json!({"bench": bad}))
                .is_err()
        );
        let mut bad = saved_of(&b);
        bad["context"]["task"] = json!("sleep");
        assert!(
            other
                .run("controls.bench.load", &json!({"bench": bad}))
                .is_err()
        );
        assert_eq!(other.composition, b.composition);
    }

    fn saved_of(b: &Bench) -> Json {
        serde_json::to_value(b.save()).unwrap()
    }

    #[test]
    fn the_modes_lay_out_on_their_surface_and_the_hud_shows_chips() {
        let mut b = Bench::default();
        b.load_sample();
        for mode in [
            BenchMode::Panel,
            BenchMode::Dock,
            BenchMode::Toolbar,
            BenchMode::Float,
        ] {
            b.set_mode(mode);
            let (p, _) = b.resolve().unwrap();
            assert_eq!(p.keys.card_box.surface, mode.surface());
            egui_hudget::resolve::reachable(&p).unwrap();
        }
        b.set_mode(BenchMode::Hud);
        assert!(b.resolve().is_err(), "the HUD has no slots");
        let hud = b.hud();
        assert!(hud.iter().find(|h| h.key == "exposure").unwrap().chip);
        assert!(!hud.iter().find(|h| h.key == "feather").unwrap().chip);
        let list = b.list().unwrap();
        assert_eq!(list["mode"], "hud");
        assert_eq!(list["controls"][0]["hud"]["chip"], true);
    }

    #[test]
    fn rules_and_parts_are_checked() {
        let mut b = Bench::default();
        b.load_sample();
        assert!(
            b.run(
                "controls.bench.rules",
                &json!({"rules": [{"when": "ctl.tool == 'crop'", "effect": {"fold": ["feather"]}}]}),
            )
            .is_err(),
            "a fast key never folds"
        );
        assert_eq!(b.composition.rules.len(), 3);
        assert!(
            b.run(
                "controls.bench.part",
                &json!({"key": "feather", "variant": "nope"})
            )
            .is_err()
        );
        // A rule naming a control taken off the bench still checks.
        b.remove("black").unwrap();
        assert!(b.resolve().is_ok());
    }

    #[test]
    fn a_nudge_is_one_undo_step() {
        let mut b = Bench::default();
        b.load_sample();
        assert_eq!(
            b.nudge("feather", 3, "midi").unwrap(),
            "committed: one undo step"
        );
        assert_eq!(b.readout("feather"), "23 px");
        assert_eq!(b.store.undo_steps().len(), 1);
        assert!(b.nudge("profile", 1, "midi").is_err(), "not a number");
    }
}
