//! A card laid out on the slot engine, `egui_gridflow`
//! (`docs/design/controls.md`, sections 3.2 and 4).
//!
//! A [`Card`] is a module's card or a case: controls and chrome in their
//! logical order. [`layout`] turns each item into a slot engine element
//! (a control's variant gives its forms, one per representation, and its
//! orientations; chrome takes a line or an icon's slot), packs them into a
//! [`Rack`], and maps the placements back: item → slot rectangle,
//! orientation, representation and status.
//!
//! The rack counts its width in **slot columns**: one unit across is a
//! column, one unit down is a row, so a `1x1` slot is a panel row and a
//! `1x6` block six rows high. Drawing on top of the placements comes with
//! the renderers.

use core::fmt::Write as _;

use egui_gridflow as gf;
use serde::Serialize;

use crate::model::{Case, CaseItem, Kind, Representation, Surface, representation_name};
use crate::registry::Registry;
use crate::slot::{Chrome, Integration, Orientation, Size};

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
        }
    }
}

/// What a card holds, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    pub items: Vec<CardItem>,
}

impl Card {
    pub fn new(items: Vec<CardItem>) -> Self {
        Self { items }
    }

    /// A case's parts and chrome, each part keyed by its name.
    pub fn from_case(case: &Case) -> Self {
        Self::new(
            case.items
                .iter()
                .map(|i| match i {
                    CaseItem::Part(p) => CardItem::control(&p.name, &p.control),
                    CaseItem::Chrome(c) => CardItem::Chrome(c.clone()),
                })
                .collect(),
        )
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
    fn from_gridflow(s: gf::Status) -> Self {
        match s {
            gf::Status::Placed => SlotStatus::Placed,
            gf::Status::Unpinned(r) => SlotStatus::Unpinned(r.to_string()),
            gf::Status::Overflow(r) => SlotStatus::Overflow(r.to_string()),
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
    pub grid: Option<gf::GridRect>,
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

/// A laid-out card.
#[derive(Clone, Debug, PartialEq)]
pub struct CardLayout {
    pub rack: Rack,
    /// The slot engine's result, kept for hysteresis and its dump.
    pub grid: gf::Layout,
    /// One per item, in logical order.
    pub slots: Vec<SlotPlacement>,
}

impl CardLayout {
    /// The placement of the item keyed `key`.
    pub fn get(&self, key: &str) -> Option<&SlotPlacement> {
        self.slots.iter().find(|s| s.key == key)
    }

    /// The height used, in points.
    pub fn height(&self) -> f32 {
        self.grid.rows as f32 * self.rack.row / gf::TWELFTHS_PER_UNIT as f32
    }

    /// The slot engine's picture of the grid, then one line per item:
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
    let t = (columns * gf::TWELFTHS_PER_UNIT as f32).floor() as u32;
    let t = if t >= gf::TWELFTHS_PER_UNIT {
        t / gf::TWELFTHS_PER_UNIT * gf::TWELFTHS_PER_UNIT
    } else {
        [9, 8, 6, 4, 3].into_iter().find(|&f| f <= t).unwrap_or(3)
    };
    Size::from_twelfths(t).unwrap_or(Size::QUARTER)
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
    let mut sources = Vec::with_capacity(card.items.len());
    let mut elements = Vec::with_capacity(card.items.len());
    for (i, item) in card.items.iter().enumerate() {
        let key = i as u64;
        match item {
            CardItem::Control {
                key: name,
                control,
                variant,
                orientation,
                pin,
            } => {
                let c = registry
                    .get(control)
                    .ok_or_else(|| format!("card item `{name}`: no control `{control}`"))?;
                let v = match variant {
                    Some(id) => c.variants.iter().find(|v| &v.id == id).ok_or_else(|| {
                        format!("card item `{name}`: control `{control}` has no variant `{id}`")
                    })?,
                    None => c.variants.first().ok_or_else(|| {
                        format!("card item `{name}`: control `{control}` has no variant")
                    })?,
                };
                let mut forms = v.slots.iter();
                let first = forms.next().ok_or_else(|| {
                    format!(
                        "card item `{name}`: variant `{}` of `{control}` has no slot",
                        v.id
                    )
                })?;
                let mut e = gf::Element::new(key, first.policy.to_gridflow()).with_orientations(
                    &v.orientation_order()
                        .into_iter()
                        .map(Orientation::to_gridflow)
                        .collect::<Vec<_>>(),
                );
                for f in forms {
                    e = e.with_form(f.policy.to_gridflow());
                }
                if let Some(o) = orientation {
                    e = e.with_pinned_orientation(o.to_gridflow());
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
                    forms: v.slots.iter().map(|f| f.representation).collect(),
                    integration: Some(v.integration(rack.surface)),
                });
            }
            CardItem::Chrome(chrome) => {
                elements.push(gf::Element::new(key, chrome.policy(line).to_gridflow()));
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
    let container = gf::Container::new(columns, 1.0);
    let grid = gf::layout(&container, &elements, previous.map(|p| &p.grid));
    let per = gf::TWELFTHS_PER_UNIT as f32;
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
            orientation: Orientation::from_gridflow(p.orientation),
            slot: p.rect.map(|r| format!("{}x{}", size_of(r.w), size_of(r.h))),
            grid: p.rect,
            rect: p.rect.map(|r| Rect {
                x: r.x as f32 * rack.column / per,
                y: r.y as f32 * rack.row / per,
                w: r.w as f32 * rack.column / per,
                h: r.h as f32 * rack.row / per,
            }),
            integration: s.integration,
            status: SlotStatus::from_gridflow(p.status),
            visual: p.visual,
        })
        .collect();
    Ok(CardLayout { rack, grid, slots })
}
