//! A card resolved in its context: which parts show, in which
//! representation, slot and orientation, and why
//! (`docs/design/control-contexts.md`, sections 6.2 to 6.5). No egui.
//!
//! - [`resolve`] applies the precedence, weakest first: the declaration
//!   (each part's [`Tier`] against the detail level), the card's rules,
//!   the profile's layout, the person's [`Layout`], then the moment (a
//!   part revealed, "show all"). Above all of them, a part whose value is
//!   not its default is never folded. It then lays the shown items out
//!   on the slot engine through [`card::layout_items`], in the box and at
//!   the density the keys carry.
//! - A part is never removed: its [`Visibility`] is shown, folded (with
//!   a [`Reason`]) or not applicable. Folded parts are counted in the
//!   card's [`More`] entry, with their reasons.
//! - Anti-jump: while a gesture is open or the pointer is in the panel,
//!   anything a context change would move keeps its previous placement
//!   (deferred), and a part being handled never moves.
//! - [`reachable`] is the invariant (section 6.4) as a pure function;
//!   [`reachable_everywhere`] checks it for every slow-key combination.
//! - [`find`] is the controls search: shown or folded, every control of
//!   the card.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::card::{self, Card, CardItem, CardLayout, ItemKind, Rack, Rect, SlotStatus, fits};
use crate::context::{ContextKeys, Detail, ImageFlags, RuleSource, Space, Task, Tier};
use crate::model::{Control, Mode, Representation, Surface, ValueSpec, representation_name};
use crate::registry::Registry;
use crate::slot::{Integration, Orientation, Size, Slot};

/// Who or what a reason comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// The part's tier against the detail level.
    Detail,
    /// A rule of the case or the module's card.
    Group,
    /// A profile's rule or layout.
    Profile,
    /// The person's saved layout.
    Layout,
    /// The box: the slot engine could not place it.
    Size,
    /// Its value is not its default.
    Changed,
    /// The person revealed it, or showed every part.
    Revealed,
    /// A change waits until the hand leaves the panel.
    Deferred,
    /// A layout's wish the physical limits refused.
    Fallback,
}

/// Why a part is where it is: its source, a short text for the "More"
/// row's tooltip, and the clause of the rule when one decided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    pub source: Source,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

impl Reason {
    pub fn new(source: Source, text: impl Into<String>) -> Self {
        Self {
            source,
            text: text.into(),
            when: None,
        }
    }
}

/// Whether a part shows. There is no "removed": a hidden part is folded,
/// counted and reachable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Visibility {
    Shown,
    /// Under the card's "More (n)".
    Folded {
        reason: Reason,
    },
    /// It does not apply here (a raw-only part on a picture): named in
    /// "More", not counted.
    NotApplicable {
        reason: Reason,
    },
}

impl Visibility {
    pub fn is_shown(&self) -> bool {
        *self == Visibility::Shown
    }

    /// The reason it is not shown.
    pub fn reason(&self) -> Option<&Reason> {
        match self {
            Visibility::Shown => None,
            Visibility::Folded { reason } | Visibility::NotApplicable { reason } => Some(reason),
        }
    }

    fn same_state(&self, other: &Visibility) -> bool {
        core::mem::discriminant(self) == core::mem::discriminant(other)
    }
}

/// A slot as given: its size and the columns it spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotSpan {
    pub units: Slot,
    pub span: u32,
}

/// One item of a card, resolved.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent marks agents read, each skipped when false"
)]
pub struct Placement {
    /// The item's key (a part's name, a parameter, `chrome:<kind>#<n>`).
    pub key: String,
    pub kind: ItemKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
    /// Where the host mounts it.
    pub mode: Mode,
    /// Who draws it, when shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integration: Option<Integration>,
    /// Its slot, when placed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<SlotSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<Orientation>,
    /// The representation (form) it is laid out in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
    /// Its slot in twelfths of a unit, when placed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<egui_gridflow::GridRect>,
    /// Its slot in points at the density, when placed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect: Option<Rect>,
    /// Its rank in reading order, when placed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visual: Option<usize>,
    pub visibility: Visibility,
    /// Scrolled into view and highlighted.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub emphasised: bool,
    /// Its HUD chip offered.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub chip: bool,
    /// Hit areas and chip grown.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub grown: bool,
    /// Its value is not its default.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub changed: bool,
    /// It kept its previous placement: the new one waits for the hand to
    /// leave.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub deferred: bool,
    /// Why it is shown against a rule, why it fell back: the notes the
    /// visibility does not carry.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<Reason>,
}

/// A part named by the "More" entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MoreItem {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub reason: Reason,
}

/// The card's "More (n)" entry: the folded parts, counted, with their
/// reasons; the parts that do not apply, named.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct More {
    pub count: usize,
    pub folded: Vec<MoreItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inapplicable: Vec<MoreItem>,
}

impl More {
    /// What its row reads: `More (3)`.
    pub fn title(&self) -> String {
        format!("More ({})", self.count)
    }

    /// Whether it names `key`.
    pub fn names(&self, key: &str) -> bool {
        self.folded
            .iter()
            .chain(&self.inapplicable)
            .any(|m| m.key == key)
    }
}

/// A card resolved.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CardPlacement {
    /// The keys it was resolved for.
    pub keys: ContextKeys,
    /// One per card item, in logical order.
    pub items: Vec<Placement>,
    /// The "More (n)" entry, when anything is folded or does not apply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more: Option<More>,
    /// A mode a rule offers the card in, beside the panel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offer: Option<Mode>,
    /// What the person revealed, to tell their own act from a context
    /// change.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub revealed: BTreeSet<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub reveal_all: bool,
    /// Some items kept their previous placement: resolve again once the
    /// hand leaves.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    /// The shown items' slot layout, kept for hysteresis.
    #[serde(skip)]
    pub layout: Option<CardLayout>,
}

impl CardPlacement {
    pub fn get(&self, key: &str) -> Option<&Placement> {
        self.items.iter().find(|p| p.key == key)
    }

    /// The controls (and indicators), without the chrome.
    pub fn controls(&self) -> impl Iterator<Item = &Placement> {
        self.items.iter().filter(|p| p.kind != ItemKind::Chrome)
    }
}

/// Visible or hidden, as the person pinned it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pinned {
    Shown,
    Hidden,
}

/// What a layout says of one part; `None` leaves it to the rules.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PartLayout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Pinned>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
    /// A slot size within its policy; its width is its span in columns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<Slot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orientation: Option<Orientation>,
    /// A pinned position, in twelfths of a unit (column, row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<(u32, u32)>,
}

impl PartLayout {
    fn is_placed(&self) -> bool {
        *self != PartLayout::default()
    }
}

/// A saved layout of one card (`controls.layout.<module>`): an order and
/// what is pinned per part, optionally for one task only (C3: it always
/// wins over the rules, never over the physical limits).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// The task it is for; every task when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<Task>,
    /// Part keys in the person's order; the others follow in the
    /// declared order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parts: BTreeMap<String, PartLayout>,
}

impl Layout {
    /// A layout for one task.
    pub fn for_task(task: Task) -> Self {
        Self {
            task: Some(task),
            ..Self::default()
        }
    }

    fn part(&mut self, key: &str) -> &mut PartLayout {
        self.parts.entry(key.into()).or_default()
    }

    pub fn pin_shown(mut self, key: &str) -> Self {
        self.part(key).visibility = Some(Pinned::Shown);
        self
    }

    pub fn hide(mut self, key: &str) -> Self {
        self.part(key).visibility = Some(Pinned::Hidden);
        self
    }

    pub fn represent(mut self, key: &str, r: Representation) -> Self {
        self.part(key).representation = Some(r);
        self
    }

    pub fn slot(mut self, key: &str, slot: Slot) -> Self {
        self.part(key).slot = Some(slot);
        self
    }

    pub fn orient(mut self, key: &str, o: Orientation) -> Self {
        self.part(key).orientation = Some(o);
        self
    }

    pub fn position(mut self, key: &str, col: u32, row: u32) -> Self {
        self.part(key).position = Some((col, row));
        self
    }

    pub fn order(mut self, keys: &[&str]) -> Self {
        self.order = keys.iter().map(|&k| k.to_owned()).collect();
        self
    }

    fn applies(&self, task: Task) -> bool {
        self.task.is_none_or(|t| t == task)
    }
}

/// The layouts over a card: a profile's (shipped or shared), then the
/// person's, each general first and then per task.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layouts {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile: Vec<Layout>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub person: Vec<Layout>,
}

impl Layouts {
    /// Only the person's layout.
    pub fn person(layout: Layout) -> Self {
        Self {
            profile: Vec::new(),
            person: vec![layout],
        }
    }

    /// The layouts that apply to a task, weakest first, with their
    /// source.
    fn applying(&self, task: Task) -> Vec<(Source, &Layout)> {
        let mut out = Vec::new();
        for (source, list) in [
            (Source::Profile, &self.profile),
            (Source::Layout, &self.person),
        ] {
            for general in [true, false] {
                for l in list {
                    if l.applies(task) && l.task.is_none() == general {
                        out.push((source, l));
                    }
                }
            }
        }
        out
    }
}

/// What the person just did and where their hand is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moment {
    /// Parts revealed (from "More" or by search), until they leave the
    /// card or the task.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub revealed: BTreeSet<String>,
    /// Every part shown for the moment (`controls.reveal_all`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reveal_all: bool,
    /// A gesture is open.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gesture_open: bool,
    /// The pointer is over the panel.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pointer_in_panel: bool,
    /// The part being handled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handled: Option<String>,
}

impl Moment {
    /// Whether a context change must wait.
    pub fn frozen(&self) -> bool {
        self.gesture_open || self.pointer_in_panel || self.handled.is_some()
    }

    /// Whether the hand is at work: then even the person's own reveal
    /// waits.
    fn handling(&self) -> bool {
        self.gesture_open || self.handled.is_some()
    }
}

/// The parts' current values, by key, as data.
pub type Values = BTreeMap<String, Json>;

/// Whether `value` differs from the control's default (unknown for a
/// value with no declared default: then never).
pub fn differs_from_default(control: &Control, value: &Json) -> bool {
    match &control.value {
        ValueSpec::Number(n) => value
            .as_f64()
            .is_some_and(|v| (v - n.default).abs() > n.step.abs() * 1e-3 + f64::EPSILON),
        ValueSpec::Bool { default } => value.as_bool().is_some_and(|v| v != *default),
        ValueSpec::OneOf { default, .. } => value.as_str().is_some_and(|v| v != default),
        ValueSpec::Other { .. } => false,
    }
}

/// The mode a box's surface mounts its controls in.
fn mode_of(surface: Surface) -> Mode {
    match surface {
        Surface::Dock => Mode::Docked,
        Surface::Float => Mode::Floating,
        Surface::Toolbar => Mode::Toolbar,
        Surface::Hud => Mode::Hud,
        Surface::Panel | Surface::Image => Mode::Panel,
    }
}

/// One control item while the precedence is applied.
struct Draft {
    visibility: Visibility,
    representation: Option<(Representation, Source)>,
    slot: Option<Slot>,
    orientation: Option<Orientation>,
    position: Option<(u32, u32)>,
    emphasised: bool,
    chip: bool,
    changed: bool,
    reasons: Vec<Reason>,
}

fn rule_reason(rule: &crate::context::Rule, fallback: &str) -> Reason {
    Reason {
        source: match rule.source {
            RuleSource::Group => Source::Group,
            RuleSource::Profile => Source::Profile,
        },
        text: rule.reason.clone().unwrap_or_else(|| fallback.to_owned()),
        when: Some(rule.when.clone()),
    }
}

/// The card's items in the order the layouts give its controls; chrome
/// keeps its places.
fn ordered<'a>(card: &'a Card, layouts: &[(Source, &Layout)]) -> Vec<(usize, &'a CardItem)> {
    let all: Vec<(usize, &CardItem)> = card.items.iter().enumerate().collect();
    let Some(order) = layouts
        .iter()
        .rev()
        .map(|(_, l)| &l.order)
        .find(|o| !o.is_empty())
    else {
        return all;
    };
    let mut controls: Vec<(usize, &CardItem)> = all
        .iter()
        .copied()
        .filter(|(_, i)| i.key().is_some())
        .collect();
    controls.sort_by_key(|(i, item)| {
        let k = item.key().unwrap_or_default();
        (order.iter().position(|o| o == k).unwrap_or(order.len()), *i)
    });
    let mut next = controls.into_iter();
    all.into_iter()
        .map(|(i, item)| {
            if item.key().is_some() {
                next.next().unwrap_or((i, item))
            } else {
                (i, item)
            }
        })
        .collect()
}

/// Resolves a card in its context: the precedence (declaration, the
/// card's rules, the profile, the person's layout, the moment), the
/// never-fold-changed rule above them, then the slot engine in the
/// keys' box and density. Pass the previous resolution for hysteresis
/// and anti-jump.
///
/// Refused: a rule that does not check (the class rule, an unknown part
/// or key), a control the registry does not know.
pub fn resolve(
    registry: &Registry,
    card: &Card,
    keys: &ContextKeys,
    layouts: &Layouts,
    values: &Values,
    moment: &Moment,
    previous: Option<&CardPlacement>,
) -> Result<CardPlacement, String> {
    card.check()?;
    let cx = keys.when_context();
    let holding: Vec<&crate::context::Rule> = card.rules.iter().filter(|r| r.holds(&cx)).collect();
    let applying = layouts.applying(keys.task);
    let mut offer = None;
    let mut grown = false;
    for r in &holding {
        offer = r.effect.offer.or(offer);
        grown |= r.effect.grow;
    }

    // 1 to 5: the precedence, per control item.
    let mut drafts: BTreeMap<usize, Draft> = BTreeMap::new();
    for (i, item) in card.items.iter().enumerate() {
        let CardItem::Control {
            key, control, tier, ..
        } = item
        else {
            continue;
        };
        let c = registry
            .get(control)
            .ok_or_else(|| format!("card item `{key}`: no control `{control}`"))?;
        let mut d = Draft {
            visibility: Visibility::Shown,
            representation: None,
            slot: None,
            orientation: None,
            position: None,
            emphasised: false,
            chip: false,
            changed: values.get(key).is_some_and(|v| differs_from_default(c, v)),
            reasons: Vec::new(),
        };
        // 1. The declaration: the tier against the detail level.
        if tier.folds_at(keys.detail) {
            d.visibility = Visibility::Folded {
                reason: Reason::new(Source::Detail, tier.reason()),
            };
        }
        // 2. The card's rules, in order.
        for r in &holding {
            let e = &r.effect;
            let k = key.as_str();
            if e.inapplicable.iter().any(|p| p == k) {
                d.visibility = Visibility::NotApplicable {
                    reason: rule_reason(r, "Does not apply"),
                };
            }
            if !matches!(d.visibility, Visibility::NotApplicable { .. }) {
                if e.fold.iter().any(|p| p == k) {
                    d.visibility = Visibility::Folded {
                        reason: rule_reason(r, &format!("Folded when {}", r.when)),
                    };
                } else if e.show.iter().any(|p| p == k) {
                    d.visibility = Visibility::Shown;
                }
            }
            if let Some(rep) = e.representation {
                d.representation = Some((rep, Source::Group));
            }
            d.emphasised |= e.emphasise.iter().any(|p| p == k);
            d.chip |= e.hud.iter().any(|p| p == k);
        }
        // 3 and 4. The profile's layouts, then the person's: they win on
        // what they place.
        for (source, l) in &applying {
            let Some(p) = l.parts.get(key) else { continue };
            match p.visibility {
                Some(Pinned::Shown) => {
                    if let Some(r) = d.visibility.reason() {
                        d.reasons.push(r.clone());
                    }
                    d.visibility = Visibility::Shown;
                }
                Some(Pinned::Hidden) => {
                    d.visibility = Visibility::Folded {
                        reason: Reason::new(
                            *source,
                            if *source == Source::Layout {
                                "Hidden in your layout"
                            } else {
                                "Hidden by the profile"
                            },
                        ),
                    };
                }
                None => {}
            }
            if let Some(rep) = p.representation {
                d.representation = Some((rep, *source));
            }
            if p.is_placed() {
                d.slot = p.slot.or(d.slot);
                d.orientation = p.orientation.or(d.orientation);
                d.position = p.position.or(d.position);
            }
        }
        // 5. The moment: what the person revealed.
        if (moment.reveal_all || moment.revealed.contains(key)) && !d.visibility.is_shown() {
            if let Some(r) = d.visibility.reason() {
                d.reasons.push(r.clone());
            }
            d.reasons
                .push(Reason::new(Source::Revealed, "Revealed for the moment"));
            d.visibility = Visibility::Shown;
        }
        // Above all: a changed value is never folded.
        if d.changed && !d.visibility.is_shown() {
            if let Some(r) = d.visibility.reason() {
                d.reasons.push(r.clone());
            }
            d.reasons
                .push(Reason::new(Source::Changed, "Shown: its value is changed"));
            d.visibility = Visibility::Shown;
        }
        // The physical limits: what the variant supports.
        let v = item_variant(c, item)?;
        if let Some((rep, source)) = d.representation
            && v.form(rep).is_none()
        {
            let smaller = v
                .slots
                .iter()
                .map(|f| f.representation)
                .find(|r| *r > rep)
                .or_else(|| v.slots.last().map(|f| f.representation));
            d.reasons.push(Reason::new(
                Source::Fallback,
                format!(
                    "No {} form: {}",
                    representation_name(rep),
                    smaller.map_or("none", representation_name)
                ),
            ));
            d.representation = smaller.map(|r| (r, source));
        }
        if let Some(o) = d.orientation
            && !v.orientation_order().contains(&o)
        {
            d.reasons.push(Reason::new(
                Source::Fallback,
                format!("{} does not turn {o}", c.label),
            ));
            d.orientation = None;
        }
        if let Some(s) = d.slot {
            let from = d.representation.map(|(r, _)| r);
            let forms: Vec<_> = v
                .slots
                .iter()
                .filter(|f| from.is_none_or(|r| f.representation >= r))
                .collect();
            if let Some(f) = forms.iter().find(|f| fits(&f.policy, s)) {
                let first = forms.first().map(|f| f.representation);
                if Some(f.representation) != first {
                    d.reasons.push(Reason::new(
                        Source::Fallback,
                        format!(
                            "{s} is below its minimum: {}",
                            representation_name(f.representation)
                        ),
                    ));
                    d.representation = Some((f.representation, Source::Fallback));
                }
            } else {
                d.reasons.push(Reason::new(
                    Source::Fallback,
                    format!("{s} is outside what {} takes", c.label),
                ));
                d.slot = None;
            }
        }
        drafts.insert(i, d);
    }

    // The slot engine, on the shown items and the chrome, in the
    // layouts' order.
    let order = ordered(card, &applying);
    let mut shown_items: Vec<(usize, CardItem)> = Vec::new();
    for &(i, item) in &order {
        match item {
            CardItem::Chrome(_) => shown_items.push((i, item.clone())),
            CardItem::Control {
                key,
                control,
                variant,
                tier,
                ..
            } => {
                let d = &drafts[&i];
                if !d.visibility.is_shown() {
                    continue;
                }
                shown_items.push((
                    i,
                    CardItem::Control {
                        key: key.clone(),
                        control: control.clone(),
                        variant: variant.clone(),
                        orientation: d.orientation,
                        pin: d.position,
                        tier: *tier,
                        representation: d.representation.map(|(r, _)| r),
                        slot: d.slot,
                    },
                ));
            }
        }
    }
    let rack = Rack::new(
        keys.card_box.surface,
        keys.card_box.columns() * keys.density.column(),
        keys.density.column(),
        keys.density.row(),
    );
    let refs: Vec<(usize, &CardItem)> = shown_items.iter().map(|(i, it)| (*i, it)).collect();
    let prev_grid = previous
        .filter(|p| p.keys.card_box.surface == keys.card_box.surface)
        .and_then(|p| p.layout.as_ref());
    let laid = card::layout_items(registry, &refs, rack, prev_grid)?;
    let slot_of: BTreeMap<usize, &card::SlotPlacement> =
        refs.iter().map(|(i, _)| *i).zip(&laid.slots).collect();

    let mode = mode_of(keys.card_box.surface);
    let mut items = Vec::with_capacity(card.items.len());
    for (i, item) in card.items.iter().enumerate() {
        let s = slot_of.get(&i);
        let geometry = |p: &mut Placement| {
            if let Some(s) = s {
                p.orientation = Some(s.orientation);
                p.representation = s.representation;
                p.position = s.grid;
                p.rect = s.rect;
                p.visual = s.visual;
                p.integration.clone_from(&s.integration);
                p.slot = s.grid.and_then(|g| {
                    Some(SlotSpan {
                        units: Slot::new(Size::from_twelfths(g.w)?, Size::from_twelfths(g.h)?),
                        span: g.w.div_ceil(12),
                    })
                });
            }
        };
        match item {
            CardItem::Chrome(_) => {
                let mut p = Placement {
                    key: s.map_or_else(|| format!("chrome#{i}"), |s| s.key.clone()),
                    kind: ItemKind::Chrome,
                    control: None,
                    label: None,
                    variant: None,
                    tier: None,
                    mode,
                    integration: None,
                    slot: None,
                    orientation: None,
                    representation: None,
                    position: None,
                    rect: None,
                    visual: None,
                    visibility: Visibility::Shown,
                    emphasised: false,
                    chip: false,
                    grown: false,
                    changed: false,
                    deferred: false,
                    reasons: Vec::new(),
                };
                geometry(&mut p);
                p.orientation = None;
                items.push(p);
            }
            CardItem::Control {
                key, control, tier, ..
            } => {
                let c = registry
                    .get(control)
                    .ok_or_else(|| format!("card item `{key}`: no control `{control}`"))?;
                let v = item_variant(c, item)?;
                let d = drafts.remove(&i).expect("every control was drafted");
                let mut p = Placement {
                    key: key.clone(),
                    kind: match c.kind {
                        crate::model::Kind::Control => ItemKind::Control,
                        crate::model::Kind::Indicator => ItemKind::Indicator,
                    },
                    control: Some(c.id.clone()),
                    label: Some(c.label.clone()),
                    variant: Some(v.id.clone()),
                    tier: Some(*tier),
                    mode,
                    integration: None,
                    slot: None,
                    orientation: None,
                    representation: None,
                    position: None,
                    rect: None,
                    visual: None,
                    visibility: d.visibility,
                    emphasised: d.emphasised,
                    chip: d.chip && v.chip,
                    grown,
                    changed: d.changed,
                    deferred: false,
                    reasons: d.reasons,
                };
                geometry(&mut p);
                if let Some(s) = s {
                    match &s.status {
                        SlotStatus::Placed => {}
                        SlotStatus::Unpinned(r) => p
                            .reasons
                            .push(Reason::new(Source::Fallback, format!("Not pinned: {r}"))),
                        SlotStatus::Overflow(r) => {
                            // The box cannot hold it: folded, never lost.
                            p.visibility = Visibility::Folded {
                                reason: Reason::new(Source::Size, format!("No room: {r}")),
                            };
                        }
                    }
                    if let (Some(got), Some((want, _))) = (s.representation, d.representation)
                        && got > want
                    {
                        p.reasons.push(Reason::new(
                            Source::Size,
                            format!(
                                "Too narrow for {}: {}",
                                representation_name(want),
                                representation_name(got)
                            ),
                        ));
                    }
                }
                items.push(p);
            }
        }
    }

    let mut out = CardPlacement {
        keys: keys.clone(),
        items,
        more: None,
        offer,
        revealed: moment.revealed.clone(),
        reveal_all: moment.reveal_all,
        pending: false,
        layout: Some(laid),
    };
    if let Some(prev) = previous {
        hold_still(&mut out, prev, moment);
    }
    out.more = more_of(&out.items);
    Ok(out)
}

/// The variant an item lays out (the person's pick or the default).
fn item_variant<'a>(c: &'a Control, item: &CardItem) -> Result<&'a crate::model::Variant, String> {
    let CardItem::Control { key, variant, .. } = item else {
        return Err("chrome has no variant".into());
    };
    match variant {
        Some(id) => c.variants.iter().find(|v| &v.id == id).ok_or_else(|| {
            format!(
                "card item `{key}`: control `{}` has no variant `{id}`",
                c.id
            )
        }),
        None => c
            .variants
            .first()
            .ok_or_else(|| format!("card item `{key}`: control `{}` has no variant", c.id)),
    }
}

/// Whether two placements of one item put it in the same place.
fn same_place(a: &Placement, b: &Placement) -> bool {
    a.visibility.same_state(&b.visibility)
        && a.position == b.position
        && a.representation == b.representation
        && a.orientation == b.orientation
}

/// Anti-jump: while the hand is in the panel, anything that would move
/// keeps its previous placement. A reveal by the person applies at once
/// (their own act), unless a gesture is open.
fn hold_still(out: &mut CardPlacement, prev: &CardPlacement, moment: &Moment) {
    if !moment.frozen() {
        return;
    }
    let own_act = out.revealed != prev.revealed || out.reveal_all != prev.reveal_all;
    if own_act && !moment.handling() {
        return;
    }
    let mut held = false;
    for p in &mut out.items {
        let Some(old) = prev.items.iter().find(|o| o.key == p.key) else {
            continue;
        };
        if same_place(p, old) {
            continue;
        }
        // Keep where it was and how it showed; take what does not move
        // (emphasis, chips, grown hit areas, the changed mark).
        let (emphasised, chip, grown, changed) = (p.emphasised, p.chip, p.grown, p.changed);
        let mut reasons = old.reasons.clone();
        reasons.push(Reason::new(
            Source::Deferred,
            "Waits until the pointer leaves the panel",
        ));
        *p = Placement {
            emphasised,
            chip,
            grown,
            changed,
            deferred: true,
            reasons,
            ..old.clone()
        };
        held = true;
    }
    if held {
        out.pending = true;
        out.layout.clone_from(&prev.layout);
    }
}

/// The "More (n)" entry of a card's items.
fn more_of(items: &[Placement]) -> Option<More> {
    let mut more = More::default();
    for p in items {
        let entry = |reason: &Reason| MoreItem {
            key: p.key.clone(),
            label: p.label.clone(),
            reason: reason.clone(),
        };
        match &p.visibility {
            Visibility::Shown => {}
            Visibility::Folded { reason } => more.folded.push(entry(reason)),
            Visibility::NotApplicable { reason } => more.inapplicable.push(entry(reason)),
        }
    }
    more.count = more.folded.len();
    (more.count > 0 || !more.inapplicable.is_empty()).then_some(more)
}

/// The controls search (`controls.find`): every control of the card
/// whose key, id or label holds `query` (any case), shown or folded; all
/// of them for an empty query.
pub fn find<'a>(placement: &'a CardPlacement, query: &str) -> Vec<&'a Placement> {
    let q = query.trim().to_lowercase();
    placement
        .controls()
        .filter(|p| {
            q.is_empty()
                || p.key.to_lowercase().contains(&q)
                || p.control
                    .as_deref()
                    .is_some_and(|c| c.to_lowercase().contains(&q))
                || p.label
                    .as_deref()
                    .is_some_and(|l| l.to_lowercase().contains(&q))
        })
        .collect()
}

/// The invariant (section 6.4): every control is shown in a slot, or
/// one action away from a visible element that names or counts it (the
/// "More" entry), and always found by search under its key (which keys
/// and agents reach it by).
pub fn reachable(placement: &CardPlacement) -> Result<(), String> {
    let more = placement.more.as_ref();
    let folded = placement
        .controls()
        .filter(|p| matches!(p.visibility, Visibility::Folded { .. }))
        .count();
    if more.map_or(0, |m| m.count) != folded {
        return Err(format!(
            "\"More\" counts {} but {folded} parts are folded",
            more.map_or(0, |m| m.count)
        ));
    }
    for p in placement.controls() {
        match &p.visibility {
            Visibility::Shown if p.position.is_none() => {
                return Err(format!("`{}` is shown but has no slot", p.key));
            }
            Visibility::Shown => {}
            Visibility::Folded { .. } | Visibility::NotApplicable { .. } => {
                if !more.is_some_and(|m| m.names(&p.key)) {
                    return Err(format!(
                        "`{}` is hidden and \"More\" does not name it",
                        p.key
                    ));
                }
            }
        }
        if !find(placement, &p.key).iter().any(|f| f.key == p.key) {
            return Err(format!("`{}` is not found by search", p.key));
        }
        if p.changed && !p.visibility.is_shown() && !p.deferred {
            return Err(format!("`{}` is changed but folded", p.key));
        }
    }
    Ok(())
}

/// [`reachable`] for every combination of the slow keys (task, space
/// through the box, detail) and of the image flags, with the person's
/// layouts and values as given, at every density.
pub fn reachable_everywhere(
    registry: &Registry,
    card: &Card,
    surface: Surface,
    layouts: &Layouts,
    values: &Values,
) -> Result<(), String> {
    for task in Task::ALL {
        for space in Space::ALL {
            for detail in Detail::ALL {
                for image in ImageFlags::all() {
                    for density in crate::context::Density::ALL {
                        let mut keys = ContextKeys::new(crate::context::CardBox::new(
                            surface,
                            space.sample_width(),
                            240,
                        ));
                        keys.task = task;
                        keys.detail = detail;
                        keys.image = image;
                        keys.density = density;
                        let p = resolve(
                            registry,
                            card,
                            &keys,
                            layouts,
                            values,
                            &Moment::default(),
                            None,
                        )?;
                        reachable(&p).map_err(|e| {
                            format!("{task:?}, {space:?}, {detail:?}, {image:?}, {density:?}: {e}")
                        })?;
                    }
                }
            }
        }
    }
    Ok(())
}
