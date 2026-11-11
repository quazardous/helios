//! A card laid out on the slot engine, `egui_fit`
//! (`docs/design/controls.md`, sections 3.2 and 4).
//!
//! A [`Card`] is a module's card or a case: controls and chrome in their
//! logical order. [`layout`] turns each item into a slot engine element
//! (a control's variant gives its forms, one per representation, and its
//! orientations; chrome takes a line or an icon's slot), packs them into a
//! [`Rack`], and maps the placements back: item → slot rectangle,
//! orientation, representation and status.
//!
//! A card may group its items: a case's legend, help and parts are one
//! group, packed densely inside but never mixed with what comes before or
//! after it ([`CardGroup`]), so a later control never moves up above a
//! case's legend. The layout gives each group's band, for its frame.
//!
//! The rack counts its width in **slot columns**: one unit across is a
//! column, one unit down is a row, so a `1x1` slot is a panel row and a
//! `1x6` block six rows high. Drawing on top of the placements comes with
//! the renderers.

use core::fmt::Write as _;
use core::ops::Range;

use egui_fit as fit;
use serde::Serialize;

use crate::context::{Rule, Tier};
use crate::model::{Case, CaseItem, Kind, Representation, Surface, representation_name};
use crate::registry::Registry;
use crate::slot::{Chrome, ChromeKind, Integration, Orientation, Size, SizePolicy, Slot};

/// One item of a card, in its logical order.
#[derive(Clone, Debug, PartialEq)]
pub enum CardItem {
    /// A control: `key` names it in the card (a part's name, a
    /// parameter), `control` is its id in the registry, `variant` the
    /// person's pick (the default when `None`).
    Control {
        key: String,
        control: String,
        variant: Option<String>,
        /// An orientation the person pinned.
        orientation: Option<Orientation>,
        /// A position the person pinned, in twelfths of a unit.
        pin: Option<(u32, u32)>,
        /// How rare it is: folded under "More" below its detail level.
        tier: Tier,
        /// The largest representation wanted (a rule's or the person's):
        /// the larger forms are left out. `None`: from full down.
        representation: Option<Representation>,
        /// The slot the person gave it, within its form's policy: the
        /// form's preferred size.
        slot: Option<Slot>,
    },
    /// A piece of chrome.
    Chrome(Chrome),
}

impl CardItem {
    /// A control in its default variant, nothing pinned.
    pub fn control(key: &str, control: &str) -> Self {
        CardItem::Control {
            key: key.into(),
            control: control.into(),
            variant: None,
            orientation: None,
            pin: None,
            tier: Tier::Standard,
            representation: None,
            slot: None,
        }
    }

    /// The same control at another tier.
    pub fn tier(mut self, to: Tier) -> Self {
        if let CardItem::Control { tier, .. } = &mut self {
            *tier = to;
        }
        self
    }

    /// The same control in another variant.
    pub fn variant(mut self, id: &str) -> Self {
        if let CardItem::Control { variant, .. } = &mut self {
            *variant = Some(id.into());
        }
        self
    }

    /// Its key, for a control.
    pub fn key(&self) -> Option<&str> {
        match self {
            CardItem::Control { key, .. } => Some(key),
            CardItem::Chrome(_) => None,
        }
    }
}

/// A run of a card's items laid out apart from the rest: a case's
/// legend (its header, on a line of its own), help and parts. Dense
/// packing stays within the group; nothing before or after it moves into
/// its gaps. Groups nest when one lies within another's members.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardGroup {
    /// Its name (a case's key).
    pub key: String,
    /// Its items, as indices into the card's items.
    pub items: Range<usize>,
    /// Whether its first item is its header (a legend).
    pub header: bool,
}

impl CardGroup {
    /// The group of a case whose items start at `start` in the card: a
    /// leading legend is its header, a trailing separator follows it,
    /// outside.
    pub fn of_case(key: &str, start: usize, items: &[CardItem]) -> Self {
        let chrome =
            |i: Option<&CardItem>, kind| matches!(i, Some(CardItem::Chrome(c)) if c.kind == kind);
        let end = if chrome(items.last(), ChromeKind::Separator) {
            items.len() - 1
        } else {
            items.len()
        };
        Self {
            key: key.into(),
            items: start..start + end,
            header: chrome(items.first(), ChromeKind::Legend),
        }
    }
}

/// What a card holds, in order, its groups, and the rules it declares
/// over the context keys (`docs/design/control-contexts.md`, section 6.2).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    pub items: Vec<CardItem>,
    pub groups: Vec<CardGroup>,
    pub rules: Vec<Rule>,
}

impl Card {
    pub fn new(items: Vec<CardItem>) -> Self {
        Self {
            items,
            groups: Vec::new(),
            rules: Vec::new(),
        }
    }

    /// The same card with one more group.
    pub fn group(mut self, group: CardGroup) -> Self {
        self.groups.push(group);
        self
    }

    /// The same card with one more rule.
    pub fn rule(mut self, rule: Rule) -> Self {
        self.rules.push(rule);
        self
    }

    /// A case's parts (each keyed by its name, at its tier), its chrome
    /// and its rules; under a legend, the case is a group.
    pub fn from_case(case: &Case) -> Self {
        let items: Vec<CardItem> = case
            .items
            .iter()
            .map(|i| match i {
                CaseItem::Part(p) => CardItem::control(&p.name, &p.control).tier(p.tier),
                CaseItem::Chrome(c) => CardItem::Chrome(c.clone()),
            })
            .collect();
        let group = CardGroup::of_case("case", 0, &items);
        Self {
            groups: if group.header {
                vec![group]
            } else {
                Vec::new()
            },
            items,
            rules: case.rules.clone(),
        }
    }

    /// Its controls' keys, in order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.items.iter().filter_map(CardItem::key)
    }

    /// What is wrong with its rules: each checked against the card's
    /// parts and the class rule (C1).
    pub fn check(&self) -> Result<(), String> {
        let keys: Vec<&str> = self.keys().collect();
        for r in &self.rules {
            r.check(&keys)?;
        }
        Ok(())
    }
}

/// The box a card is laid out in.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Rack {
    /// The boxed surface (panel, dock, toolbar, float).
    pub surface: Surface,
    /// The width, in points.
    pub width: f32,
    /// One slot column's width, in points.
    pub column: f32,
    /// One row's height, in points (the base unit, scaled by density).
    pub row: f32,
}

impl Rack {
    /// A rack `width` points wide in columns of `column` points and rows
    /// of `row` points.
    pub fn new(surface: Surface, width: f32, column: f32, row: f32) -> Self {
        Self {
            surface,
            width,
            column,
            row,
        }
    }
}

/// What a laid-out item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemKind {
    Control,
    Indicator,
    Chrome,
}

/// A rectangle in points, from the card's top left.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Where an item went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", content = "reason", rename_all = "kebab-case")]
pub enum SlotStatus {
    Placed,
    /// Placed by the flow: its pin could not be honoured.
    Unpinned(String),
    /// Not placed: the host folds it or shows it elsewhere.
    Overflow(String),
}

impl SlotStatus {
    fn from_fit(s: fit::Status) -> Self {
        match s {
            fit::Status::Placed => SlotStatus::Placed,
            fit::Status::Unpinned(r) => SlotStatus::Unpinned(r.to_string()),
            fit::Status::Overflow(r) => SlotStatus::Overflow(r.to_string()),
        }
    }
}

/// One item's placement, in logical order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SlotPlacement {
    /// The item's key (a control's key, `chrome:<kind>#<index>`).
    pub key: String,
    pub kind: ItemKind,
    /// The control's id, for a control or an indicator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control: Option<String>,
    /// The variant laid out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// The representation chosen (the slot engine's form); `None` for
    /// chrome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
    pub orientation: Orientation,
    /// The slot, as a slot size, when placed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    /// The slot in twelfths of a unit, when placed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grid: Option<fit::GridRect>,
    /// The slot in points, when placed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect: Option<Rect>,
    /// Who draws it on this surface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integration: Option<Integration>,
    #[serde(flatten)]
    pub status: SlotStatus,
    /// Its rank in reading order, when placed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visual: Option<usize>,
}

/// Where a group went.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GroupPlacement {
    /// The group's key.
    pub key: String,
    /// Its band, across the whole card, in twelfths of a unit, when any
    /// of its items is placed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grid: Option<fit::GridRect>,
    /// Its band in points, for its frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect: Option<Rect>,
}

/// A laid-out card.
#[derive(Clone, Debug, PartialEq)]
pub struct CardLayout {
    pub rack: Rack,
    /// The slot engine's result, kept for hysteresis and its dump.
    pub grid: fit::Layout,
    /// One per item, in logical order.
    pub slots: Vec<SlotPlacement>,
    /// One per group of the card, in its order.
    pub groups: Vec<GroupPlacement>,
}

impl CardLayout {
    /// The placement of the item keyed `key`.
    pub fn get(&self, key: &str) -> Option<&SlotPlacement> {
        self.slots.iter().find(|s| s.key == key)
    }

    /// The height used, in points.
    pub fn height(&self) -> f32 {
        self.grid.rows as f32 * self.rack.row / fit::TWELFTHS_PER_UNIT as f32
    }

    /// The placement of the group keyed `key`.
    pub fn group(&self, key: &str) -> Option<&GroupPlacement> {
        self.groups.iter().find(|g| g.key == key)
    }

    /// The slot engine's picture of the grid (its groups' bands in its
    /// gutter, then one line per group), then one line per item:
    /// its letter, key, representation, orientation, slot and status.
    pub fn dump(&self) -> String {
        let grid = self.grid.dump();
        let lines: Vec<&str> = grid.lines().collect();
        let picture = lines.len().saturating_sub(self.slots.len());
        let mut out = String::new();
        for l in &lines[..picture] {
            out.push_str(l);
            out.push('\n');
        }
        for (i, s) in self.slots.iter().enumerate() {
            let letter = lines
                .get(picture + i)
                .and_then(|l| l.chars().next())
                .unwrap_or('#');
            let _ = write!(out, "{letter} {}", s.key);
            if let Some(r) = s.representation {
                let _ = write!(out, " {}", representation_name(r));
            }
            if s.kind != ItemKind::Chrome {
                let _ = write!(out, " {}", s.orientation);
            }
            if let (Some(slot), Some(g)) = (&s.slot, s.grid) {
                let _ = write!(out, " {slot} at {},{}", g.x, g.y);
            }
            match &s.status {
                SlotStatus::Placed => {}
                SlotStatus::Unpinned(r) => {
                    let _ = write!(out, " unpinned({r})");
                }
                SlotStatus::Overflow(r) => {
                    let _ = write!(out, " overflow({r})");
                }
            }
            out.push('\n');
        }
        out
    }
}

/// What the engine was given for one item, to map its placement back.
struct Source {
    key: String,
    kind: ItemKind,
    control: Option<String>,
    variant: Option<String>,
    /// The representation of each engine form, in order.
    forms: Vec<Representation>,
    integration: Option<Integration>,
}

/// A grid extent in twelfths as a slot size (the engine only places
/// allowed sizes, so the fallback never shows).
fn size_of(twelfths: u32) -> String {
    Size::from_twelfths(twelfths).map_or_else(|| format!("{twelfths}/12"), |s| s.to_string())
}

/// The widest allowed size at most `columns` wide (a quarter at least).
fn line_width(columns: f32) -> Size {
    // `as` saturates and maps NaN to 0.
    let t = (columns * fit::TWELFTHS_PER_UNIT as f32).floor() as u32;
    let t = if t >= fit::TWELFTHS_PER_UNIT {
        t / fit::TWELFTHS_PER_UNIT * fit::TWELFTHS_PER_UNIT
    } else {
        [9, 8, 6, 4, 3].into_iter().find(|&f| f <= t).unwrap_or(3)
    };
    Size::from_twelfths(t).unwrap_or(Size::QUARTER)
}

/// Whether `slot` lies within `policy`'s smallest and largest slots.
pub fn fits(policy: &SizePolicy, slot: Slot) -> bool {
    policy.min.width <= slot.width
        && slot.width <= policy.max.width
        && policy.min.height <= slot.height
        && slot.height <= policy.max.height
}

/// Lays `card` out in `rack`. Pass the card's previous layout in the same
/// rack to keep its choices while the box stays within half a unit of a
/// threshold (hysteresis).
///
/// Refused: a surface that is not boxed, a control the registry does not
/// know, a variant it does not have, or one with no slot forms.
pub fn layout(
    registry: &Registry,
    card: &Card,
    rack: Rack,
    previous: Option<&CardLayout>,
) -> Result<CardLayout, String> {
    let items: Vec<(usize, &CardItem)> = card.items.iter().enumerate().collect();
    layout_items(registry, &items, &card.groups, rack, previous)
}

/// Lays some of a card's items out, each with its index in the card: the
/// index keys it for the engine (so hysteresis holds when other items are
/// folded) and names its chrome; `groups` are the card's, over those
/// indices. The items of a group must come together, its header first.
/// What `resolve` lays the shown items out with.
pub fn layout_items(
    registry: &Registry,
    items: &[(usize, &CardItem)],
    groups: &[CardGroup],
    rack: Rack,
    previous: Option<&CardLayout>,
) -> Result<CardLayout, String> {
    if !rack.surface.is_boxed() {
        return Err(format!(
            "{:?} has no slots: the HUD shows chips, the image gizmos",
            rack.surface
        ));
    }
    let columns = if rack.column > 0.0 {
        rack.width / rack.column
    } else {
        0.0
    };
    // The width in columns, as the widest allowed slot a line can take.
    let line = line_width(columns);
    let mut sources = Vec::with_capacity(items.len());
    let mut elements = Vec::with_capacity(items.len());
    for &(i, item) in items {
        let key = i as u64;
        match item {
            CardItem::Control {
                key: name,
                control,
                variant,
                orientation,
                pin,
                representation,
                slot,
                ..
            } => {
                let c = registry
                    .get(control)
                    .ok_or_else(|| format!("card item `{name}`: no control `{control}`"))?;
                let v = match variant {
                    Some(id) => c.faces.iter().find(|v| &v.id == id).ok_or_else(|| {
                        format!("card item `{name}`: control `{control}` has no face `{id}`")
                    })?,
                    None => c.faces.first().ok_or_else(|| {
                        format!("card item `{name}`: control `{control}` has no face")
                    })?,
                };
                // The forms from the wanted representation down.
                let wanted: Vec<_> = v
                    .slots
                    .iter()
                    .filter(|f| representation.is_none_or(|r| f.representation >= r))
                    .collect();
                let kept = if wanted.is_empty() {
                    v.slots.iter().collect()
                } else {
                    wanted
                };
                let mut forms = kept.iter().copied();
                let first = forms.next().ok_or_else(|| {
                    format!(
                        "card item `{name}`: variant `{}` of `{control}` has no slot",
                        v.id
                    )
                })?;
                let first_policy = match slot {
                    Some(s) if fits(&first.policy, *s) => SizePolicy {
                        size: *s,
                        ..first.policy
                    },
                    _ => first.policy,
                };
                let mut e = fit::Element::new(key, first_policy.to_fit()).with_orientations(
                    &v.orientation_order()
                        .into_iter()
                        .map(Orientation::to_fit)
                        .collect::<Vec<_>>(),
                );
                for f in forms {
                    e = e.with_form(f.policy.to_fit());
                }
                if let Some(o) = orientation {
                    e = e.with_pinned_orientation(o.to_fit());
                }
                if let Some((col, row)) = pin {
                    e = e.with_pin(*col, *row);
                }
                elements.push(e);
                sources.push(Source {
                    key: name.clone(),
                    kind: match c.kind {
                        Kind::Control => ItemKind::Control,
                        Kind::Indicator => ItemKind::Indicator,
                    },
                    control: Some(c.id.clone()),
                    variant: Some(v.id.clone()),
                    forms: kept.iter().map(|f| f.representation).collect(),
                    integration: Some(v.integration(rack.surface)),
                });
            }
            CardItem::Chrome(chrome) => {
                elements.push(fit::Element::new(key, chrome.policy(line).to_fit()));
                let kind = serde_json::to_value(chrome.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                sources.push(Source {
                    key: format!("chrome:{kind}#{i}"),
                    kind: ItemKind::Chrome,
                    control: None,
                    variant: None,
                    forms: Vec::new(),
                    integration: None,
                });
            }
        }
    }
    // The engine counts in columns: one unit is one column across.
    let container = fit::Container::new(columns, 1.0);
    let engine_groups: Vec<fit::Group> = groups
        .iter()
        .enumerate()
        .map(|(g, group)| {
            // The shown items of the group, by their place in `items`.
            let inside: Vec<usize> = items
                .iter()
                .enumerate()
                .filter(|(_, (i, _))| group.items.contains(i))
                .map(|(at, _)| at)
                .collect();
            let range = if let (Some(&first), Some(&last)) = (inside.first(), inside.last()) {
                first..last + 1
            } else {
                // Nothing shown: an empty group where it would be.
                let at = items
                    .iter()
                    .position(|(i, _)| *i >= group.items.start)
                    .unwrap_or(items.len());
                at..at
            };
            let header = group.header
                && inside
                    .first()
                    .is_some_and(|&at| items[at].0 == group.items.start);
            fit::Group {
                key: g as u64,
                range,
                header,
            }
        })
        .collect();
    let grid = fit::layout_grouped(
        &container,
        &elements,
        &engine_groups,
        previous.map(|p| &p.grid),
    );
    let per = fit::TWELFTHS_PER_UNIT as f32;
    let to_rect = |r: fit::GridRect| Rect {
        x: r.x as f32 * rack.column / per,
        y: r.y as f32 * rack.row / per,
        w: r.w as f32 * rack.column / per,
        h: r.h as f32 * rack.row / per,
    };
    let group_places = groups
        .iter()
        .zip(&grid.groups)
        .map(|(g, p)| GroupPlacement {
            key: g.key.clone(),
            grid: p.rect,
            rect: p.rect.map(to_rect),
        })
        .collect();
    let slots = grid
        .placements
        .iter()
        .zip(sources)
        .map(|(p, s)| SlotPlacement {
            key: s.key,
            kind: s.kind,
            control: s.control,
            variant: s.variant,
            representation: s.forms.get(p.form).copied(),
            orientation: Orientation::from_fit(p.orientation),
            slot: p.rect.map(|r| format!("{}x{}", size_of(r.w), size_of(r.h))),
            grid: p.rect,
            rect: p.rect.map(to_rect),
            integration: s.integration,
            status: SlotStatus::from_fit(p.status),
            visual: p.visual,
        })
        .collect();
    Ok(CardLayout {
        rack,
        grid,
        slots,
        groups: group_places,
    })
}
