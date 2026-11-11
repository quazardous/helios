//! What a control asks of a boxed surface (`docs/design/controls.md`,
//! section 3.2): standard slots, size policies per representation,
//! orientations, and its integration level per surface. Declarations
//! only; [`crate::card`] lays them out with `egui_fit`.
//!
//! - A [`Slot`] is a width × height in allowed sizes: ¼, ⅓, ½, ⅔, ¾ of the
//!   base unit, or whole units. Written `1/2x1`, `1x6`.
//! - A [`SizePolicy`] gives the smallest, preferred and largest slot, an
//!   aspect ratio if any, and the axis the control grows along.
//! - A [`SlotForm`] is the policy of one [`Representation`]: a variant
//!   lists its forms largest first (full, compact, simplified), which are
//!   the slot engine's forms 0, 1, 2… in that order.
//! - An [`Integration`] says, per surface, whether the host draws the
//!   control (managed) or the control draws itself (integrated), and then
//!   what managed [`Fallback`] stands in where its drawing cannot go.

use core::fmt;
use core::str::FromStr;

use egui_fit as fit;
use serde::{Deserialize, Serialize};

use crate::model::Representation;

/// A slot size along one axis, in the slot engine's allowed sizes.
pub use fit::Size;

/// A slot: a width and a height in allowed sizes of the base unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Slot {
    pub width: Size,
    pub height: Size,
}

impl Slot {
    pub const fn new(width: Size, height: Size) -> Self {
        Self { width, height }
    }

    /// One unit wide and one row high: the old panel `row`.
    pub const ROW: Slot = Slot::new(Size::ONE, Size::ONE);
    /// Half a unit wide, one row high: the old panel `half`.
    pub const HALF: Slot = Slot::new(Size::HALF, Size::ONE);

    /// `w` units wide, one row high.
    pub const fn wide(w: u32) -> Slot {
        Slot::new(Size::units(w), Size::ONE)
    }

    /// One unit wide, `n` rows high: the old panel `block(n)`.
    pub const fn block(n: u32) -> Slot {
        Slot::new(Size::ONE, Size::units(n))
    }

    fn to_fit(self) -> fit::Size2 {
        fit::Size2::new(self.width, self.height)
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

fn parse_size(s: &str) -> Result<Size, String> {
    let twelfths = match s.trim() {
        "1/4" => 3,
        "1/3" => 4,
        "1/2" => 6,
        "2/3" => 8,
        "3/4" => 9,
        n => {
            let n: u32 = n.parse().map_err(|_| format!("`{s}` is not a slot size"))?;
            n.saturating_mul(fit::TWELFTHS_PER_UNIT)
        }
    };
    Size::from_twelfths(twelfths).ok_or_else(|| format!("`{s}` is not a slot size"))
}

impl FromStr for Slot {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (w, h) = s
            .split_once('x')
            .ok_or_else(|| format!("`{s}` is not a slot (width x height, as `1/2x1`)"))?;
        Ok(Slot::new(parse_size(w)?, parse_size(h)?))
    }
}

impl TryFrom<String> for Slot {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        s.parse()
    }
}

impl From<Slot> for String {
    fn from(s: Slot) -> String {
        s.to_string()
    }
}

/// The axis a control grows along, in its horizontal terms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Grow {
    /// Keeps its preferred slot (a bitmap-like drawing).
    #[default]
    Fixed,
    /// Takes leftover room across, slot step by slot step.
    Width,
    /// Takes leftover room down.
    Height,
}

/// A width-to-height ratio the control keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AspectRatio {
    pub width: u32,
    pub height: u32,
}

/// How a control is sized in its slot, written for its horizontal
/// orientation (vertical swaps the axes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SizePolicy {
    /// Below it, the control steps down to its next form.
    pub min: Slot,
    /// The slot it is placed in.
    pub size: Slot,
    /// The largest slot growth may reach.
    pub max: Slot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<AspectRatio>,
    #[serde(default, skip_serializing_if = "is_fixed")]
    pub grow: Grow,
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_fixed(g: &Grow) -> bool {
    *g == Grow::Fixed
}

impl SizePolicy {
    /// One slot, never bigger nor smaller.
    pub const fn fixed(slot: Slot) -> Self {
        Self {
            min: slot,
            size: slot,
            max: slot,
            aspect_ratio: None,
            grow: Grow::Fixed,
        }
    }

    /// Placed at `size`, down to `min`, growing along `grow` up to `max`.
    pub const fn growable(min: Slot, size: Slot, max: Slot, grow: Grow) -> Self {
        Self {
            min,
            size,
            max,
            aspect_ratio: None,
            grow,
        }
    }

    /// The same policy keeping a width-to-height ratio.
    pub const fn aspect(mut self, width: u32, height: u32) -> Self {
        self.aspect_ratio = Some(AspectRatio { width, height });
        self
    }

    /// What is wrong with it: min ≤ size ≤ max on each axis, growth only
    /// if the maximum is larger, no zero ratio.
    pub fn check(&self) -> Result<(), String> {
        let (min, size, max) = (self.min, self.size, self.max);
        if !(min.width <= size.width
            && size.width <= max.width
            && min.height <= size.height
            && size.height <= max.height)
        {
            return Err(format!("{min} <= {size} <= {max} does not hold"));
        }
        if self.grow == Grow::Fixed && (min != size || size != max) {
            return Err(format!("a fixed policy has one slot, not {min} to {max}"));
        }
        if matches!(self.aspect_ratio, Some(r) if r.width == 0 || r.height == 0) {
            return Err("an aspect ratio has two non-zero terms".into());
        }
        Ok(())
    }

    /// The slot engine's policy.
    pub fn to_fit(&self) -> fit::SizePolicy {
        let mut p = fit::SizePolicy {
            min_size: self.min.to_fit(),
            size: self.size.to_fit(),
            max_size: self.max.to_fit(),
            aspect_ratio: None,
            grow: match self.grow {
                Grow::Fixed => fit::Grow::Fixed,
                Grow::Width => fit::Grow::Along(fit::Axis::Width),
                Grow::Height => fit::Grow::Along(fit::Axis::Height),
            },
        };
        if let Some(r) = self.aspect_ratio {
            p = p.with_aspect_ratio(r.width, r.height);
        }
        p
    }
}

impl fmt::Display for SizePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.grow {
            Grow::Fixed => write!(f, "fixed {}", self.size)?,
            Grow::Width | Grow::Height => write!(
                f,
                "{} (from {} to {}, grows {})",
                self.size,
                self.min,
                self.max,
                if self.grow == Grow::Width {
                    "across"
                } else {
                    "down"
                }
            )?,
        }
        if let Some(r) = self.aspect_ratio {
            write!(f, ", ratio {}:{}", r.width, r.height)?;
        }
        Ok(())
    }
}

/// The size policy of one representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotForm {
    pub representation: Representation,
    pub policy: SizePolicy,
}

/// The direction a control is laid out along.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Orientation {
    Horizontal,
    /// Turned: the policy's width runs down. Up is more.
    Vertical,
}

impl Orientation {
    pub fn to_fit(self) -> fit::Orientation {
        match self {
            Orientation::Horizontal => fit::Orientation::Horizontal,
            Orientation::Vertical => fit::Orientation::Vertical,
        }
    }

    pub fn from_fit(o: fit::Orientation) -> Self {
        match o {
            fit::Orientation::Horizontal => Orientation::Horizontal,
            fit::Orientation::Vertical => Orientation::Vertical,
        }
    }
}

impl fmt::Display for Orientation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Orientation::Horizontal => "horizontal",
            Orientation::Vertical => "vertical",
        })
    }
}

/// What stands in for an integrated control's own drawing where it cannot
/// go (a small representation, a folded group, a narrow panel, a device
/// with no pointer).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "kebab-case")]
pub enum Fallback {
    /// Another variant of the same control, managed on that surface.
    Variant(String),
    /// The value type's standard managed renderer (a curve's points as
    /// numbered rows, a colour as a swatch and its channels).
    ValueType,
    /// A case's parts, each in its own managed slot.
    Parts,
}

/// Who draws a control on a surface.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "level", rename_all = "kebab-case")]
pub enum Integration {
    /// The host draws it with the standard renderer for its slot,
    /// orientation and representation.
    #[default]
    Managed,
    /// It brings its own drawing, bound by the same contract. Without a
    /// fallback it is an exception, counted by the inspector.
    Integrated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fallback: Option<Fallback>,
    },
}

impl Integration {
    /// Integrated, with a managed fallback.
    pub fn integrated(fallback: Fallback) -> Self {
        Integration::Integrated {
            fallback: Some(fallback),
        }
    }
}

/// A case's static item: drawn by the case, with no value and no jack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChromeKind {
    /// The case's title, as a `fieldset`'s `legend`.
    Legend,
    /// A rule between groups of parts.
    Separator,
    /// A line of help text.
    Help,
    /// An icon.
    Icon,
}

/// A piece of a case's chrome: a layout item, not a control.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chrome {
    pub kind: ChromeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl Chrome {
    pub fn new(kind: ChromeKind) -> Self {
        Self { kind, text: None }
    }

    pub fn text(mut self, text: &str) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Its slot in a box `width` wide (an allowed size): a legend, a
    /// separator and help text take the whole line, an icon a quarter
    /// unit.
    pub fn policy(&self, width: Size) -> SizePolicy {
        let line = |h| SizePolicy::fixed(Slot::new(width, h));
        match self.kind {
            ChromeKind::Legend => line(Size::HALF),
            ChromeKind::Separator => line(Size::QUARTER),
            ChromeKind::Help => line(Size::ONE),
            ChromeKind::Icon => SizePolicy::fixed(Slot::new(Size::QUARTER, Size::ONE)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_read_and_write_as_text() {
        for s in ["1/2x1", "1x6", "2/3x1", "4x1", "1/4x1/4"] {
            assert_eq!(s.parse::<Slot>().unwrap().to_string(), s);
        }
        assert!("1/5x1".parse::<Slot>().is_err());
        assert!("0x1".parse::<Slot>().is_err());
        assert!("1".parse::<Slot>().is_err());
        assert_eq!(Slot::block(6).to_string(), "1x6");
    }

    #[test]
    fn policies_are_checked() {
        let p = SizePolicy::growable(Slot::HALF, Slot::ROW, Slot::wide(4), Grow::Width);
        assert!(p.check().is_ok());
        assert_eq!(p.to_string(), "1x1 (from 1/2x1 to 4x1, grows across)");
        let bad = SizePolicy::growable(Slot::ROW, Slot::HALF, Slot::wide(4), Grow::Width);
        assert!(bad.check().is_err());
        let mut fixed = SizePolicy::fixed(Slot::ROW);
        fixed.max = Slot::wide(2);
        assert!(fixed.check().is_err());
        assert!(SizePolicy::fixed(Slot::ROW).aspect(0, 1).check().is_err());
    }
}
