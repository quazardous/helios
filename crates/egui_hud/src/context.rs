//! The context keys and the rules over them
//! (`docs/design/control-contexts.md`, sections 6.1 and 6.2). No egui.
//!
//! - [`ContextKeys`]: the six keys (`ctl.task`, `ctl.space`,
//!   `ctl.detail`, `ctl.image`, `ctl.tool`, `ctl.device`), the build's
//!   [`Target`], and the two facts handed with them to every control: the
//!   [`CardBox`] and the [`Density`]. `ctl.space` is derived from the box
//!   in slot columns, with half a unit of hysteresis (C4).
//! - Each key has a [`Class`]: slow (task, space, detail), per image, or
//!   fast (tool, device). The class rule (C1): slow keys may fold parts
//!   and change representations; the image key only marks parts that do
//!   not apply; fast keys only add, grow and emphasise.
//! - A part's [`Tier`] folds it below the detail level (C2: `standard`
//!   by default, so expert parts fold).
//! - A [`Rule`] is a `when` clause over the keys (egui-pop's evaluator,
//!   the one key bindings use) and an [`Effect`].

use egui_pop_model::when::{When, WhenContext};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::model::{Mode, Representation, Surface};

/// What the person is doing: an explicit switch, never inferred.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Task {
    Cull,
    #[default]
    Develop,
    Retouch,
    Export,
}

impl Task {
    pub const ALL: [Task; 4] = [Task::Cull, Task::Develop, Task::Retouch, Task::Export];
}

/// The card's box, coarsely: derived from its width in slot columns.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Space {
    Narrow,
    #[default]
    Regular,
    Wide,
}

/// Below this width, in twelfths of a unit, a box is narrow (one and a
/// half columns).
pub const NARROW_BELOW: u32 = 18;
/// From this width, in twelfths, a box is wide (three and a half
/// columns).
pub const WIDE_FROM: u32 = 42;
/// How far past a threshold, in twelfths, a box must go to change its
/// space: half a unit (C4).
pub const HYSTERESIS: u32 = 6;

impl Space {
    pub const ALL: [Space; 3] = [Space::Narrow, Space::Regular, Space::Wide];

    /// The space of a box `width` twelfths wide. With the previous space,
    /// a threshold is crossed only half a unit past it, so a resize
    /// around it does not flicker.
    pub fn derive(width: u32, previous: Option<Space>) -> Space {
        let raw = |narrow: u32, wide: u32| {
            if width < narrow {
                Space::Narrow
            } else if width < wide {
                Space::Regular
            } else {
                Space::Wide
            }
        };
        match previous {
            None => raw(NARROW_BELOW, WIDE_FROM),
            // Leaving a space takes half a unit more; staying takes none.
            Some(Space::Narrow) => raw(NARROW_BELOW + HYSTERESIS, WIDE_FROM + HYSTERESIS),
            Some(Space::Regular) => raw(
                NARROW_BELOW.saturating_sub(HYSTERESIS),
                WIDE_FROM + HYSTERESIS,
            ),
            Some(Space::Wide) => raw(
                NARROW_BELOW.saturating_sub(HYSTERESIS),
                WIDE_FROM.saturating_sub(HYSTERESIS),
            ),
        }
    }

    /// A box width, in twelfths, well inside this space (for tests and
    /// sweeps).
    pub fn sample_width(self) -> u32 {
        match self {
            Space::Narrow => 12,
            Space::Regular => 30,
            Space::Wide => 60,
        }
    }
}

/// How much a person wants to see: only the person sets it, never
/// inferred (C2: `standard` by default).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Detail {
    Essential,
    #[default]
    Standard,
    Expert,
}

impl Detail {
    pub const ALL: [Detail; 3] = [Detail::Essential, Detail::Standard, Detail::Expert];
}

/// How rare a part is. A part shows at a detail level at or above its
/// tier, and folds under "More" below it.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    Essential,
    /// What implicit controls get.
    #[default]
    Standard,
    Expert,
}

impl Tier {
    pub fn is_standard(&self) -> bool {
        *self == Tier::Standard
    }

    /// Whether it folds at a detail level.
    pub fn folds_at(self, detail: Detail) -> bool {
        let level = match detail {
            Detail::Essential => Tier::Essential,
            Detail::Standard => Tier::Standard,
            Detail::Expert => Tier::Expert,
        };
        self > level
    }

    /// The reason shown on "More" for a part folded by its tier.
    pub fn reason(self) -> &'static str {
        match self {
            Tier::Essential => "Essential",
            Tier::Standard => "Standard detail",
            Tier::Expert => "Expert",
        }
    }
}

/// What the image is: raw or not, edited, masked. Changes per image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImageFlags {
    pub raw: bool,
    pub edited: bool,
    pub masked: bool,
}

impl Default for ImageFlags {
    /// An untouched raw with no mask.
    fn default() -> Self {
        Self {
            raw: true,
            edited: false,
            masked: false,
        }
    }
}

impl ImageFlags {
    /// The eight combinations.
    pub fn all() -> Vec<ImageFlags> {
        (0..8u8)
            .map(|b| ImageFlags {
                raw: b & 1 != 0,
                edited: b & 2 != 0,
                masked: b & 4 != 0,
            })
            .collect()
    }
}

/// The source of the last input event, or a connected controller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Device {
    #[default]
    Pointer,
    Pen,
    Touch,
    Keys,
    Midi,
}

/// The build target: a fact rules may test, nothing changes it at run
/// time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    #[default]
    Desktop,
    Web,
}

/// A person setting the device may suggest, never switch: it scales the
/// base unit, so it changes how many slot columns a box holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
    Touch,
}

impl Density {
    pub const ALL: [Density; 3] = [Density::Comfortable, Density::Compact, Density::Touch];

    /// The base unit, one panel row, in points.
    pub fn row(self) -> f32 {
        match self {
            Density::Comfortable => 24.0,
            Density::Compact => 20.0,
            Density::Touch => 32.0,
        }
    }

    /// One slot column, in points: ten rows across.
    pub fn column(self) -> f32 {
        self.row() * 10.0
    }
}

/// The box a card has (`Box` in the design note, renamed so it does not
/// shadow Rust's): its surface and its size in twelfths of the base
/// unit, one unit across being one slot column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CardBox {
    pub surface: Surface,
    /// The width, in twelfths of a column.
    pub width: u32,
    /// The height, in twelfths of a row.
    pub height: u32,
}

impl Default for CardBox {
    /// A panel two columns wide and twenty rows high.
    fn default() -> Self {
        Self {
            surface: Surface::Panel,
            width: 24,
            height: 240,
        }
    }
}

impl CardBox {
    pub fn new(surface: Surface, width: u32, height: u32) -> Self {
        Self {
            surface,
            width,
            height,
        }
    }

    /// The box of `width` × `height` points at a density.
    pub fn from_points(surface: Surface, width: f32, height: f32, density: Density) -> Self {
        // `as` saturates and maps NaN to 0.
        let t = |points: f32, unit: f32| (points * 12.0 / unit).floor() as u32;
        Self::new(
            surface,
            t(width, density.column()),
            t(height, density.row()),
        )
    }

    /// Its width, in slot columns.
    pub fn columns(&self) -> f32 {
        self.width as f32 / 12.0
    }
}

/// How fast a key changes, and so what a rule over it may do (C1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    /// Task, space, detail (and the build target): may fold parts and
    /// change representations.
    Slow,
    /// The image: may only mark the parts that do not apply.
    Image,
    /// Tool, device: may only add, grow and emphasise.
    Fast,
}

/// Every key a rule may test, with its class.
pub const KEYS: [(&str, Class); 10] = [
    ("ctl.task", Class::Slow),
    ("ctl.space", Class::Slow),
    ("ctl.detail", Class::Slow),
    ("ctl.target", Class::Slow),
    ("ctl.image", Class::Image),
    ("ctl.image.raw", Class::Image),
    ("ctl.image.edited", Class::Image),
    ("ctl.image.masked", Class::Image),
    ("ctl.tool", Class::Fast),
    ("ctl.device", Class::Fast),
];

/// The class of a key, if it is one.
pub fn class_of(key: &str) -> Option<Class> {
    KEYS.iter().find(|(k, _)| *k == key).map(|(_, c)| *c)
}

/// The context keys and the two facts handed with them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ContextKeys {
    pub task: Task,
    /// Derived from the box: set it with [`ContextKeys::set_box`].
    pub space: Space,
    pub detail: Detail,
    pub image: ImageFlags,
    /// The active tool's id, empty for none.
    #[serde(default)]
    pub tool: String,
    pub device: Device,
    #[serde(default)]
    pub target: Target,
    /// The box the card has.
    #[serde(rename = "box")]
    pub card_box: CardBox,
    pub density: Density,
}

/// A value's name as data (`develop`).
fn name<T: Serialize>(v: T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl ContextKeys {
    /// The keys for a box, every other key at its default; the space
    /// derived from the box.
    pub fn new(card_box: CardBox) -> Self {
        Self {
            space: Space::derive(card_box.width, None),
            card_box,
            ..Self::default()
        }
    }

    /// A new box: the space follows, with hysteresis against the current
    /// one.
    pub fn set_box(&mut self, card_box: CardBox) {
        self.space = Space::derive(card_box.width, Some(self.space));
        self.card_box = card_box;
    }

    /// The same keys with a new box.
    pub fn with_box(mut self, card_box: CardBox) -> Self {
        self.set_box(card_box);
        self
    }

    /// The keys as egui-pop context pairs (`ctl.task` = `develop`), so
    /// key bindings and menus may use them in `when` clauses too.
    pub fn to_context(&self) -> Vec<(String, String)> {
        let flag = |on: bool| if on { "true" } else { "false" }.to_owned();
        let mut image = Vec::new();
        for (on, n) in [
            (self.image.raw, "raw"),
            (self.image.edited, "edited"),
            (self.image.masked, "masked"),
        ] {
            if on {
                image.push(n);
            }
        }
        vec![
            ("ctl.task".into(), name(self.task)),
            ("ctl.space".into(), name(self.space)),
            ("ctl.detail".into(), name(self.detail)),
            ("ctl.target".into(), name(self.target)),
            ("ctl.image".into(), image.join(" ")),
            ("ctl.image.raw".into(), flag(self.image.raw)),
            ("ctl.image.edited".into(), flag(self.image.edited)),
            ("ctl.image.masked".into(), flag(self.image.masked)),
            ("ctl.tool".into(), self.tool.clone()),
            ("ctl.device".into(), name(self.device)),
        ]
    }

    /// The keys as a `when` context.
    pub fn when_context(&self) -> WhenContext {
        let mut cx = WhenContext::new();
        for (k, v) in self.to_context() {
            cx.set(&k, &v);
        }
        cx
    }

    /// The keys, the box and the density as data, each key with its
    /// class: what `controls.context` answers.
    pub fn report(&self) -> Json {
        let keys: Vec<Json> = self
            .to_context()
            .into_iter()
            .map(|(k, v)| json!({"key": k, "value": v, "class": class_of(&k)}))
            .collect();
        json!({
            "task": self.task,
            "space": self.space,
            "detail": self.detail,
            "image": self.image,
            "tool": self.tool,
            "device": self.device,
            "target": self.target,
            "box": self.card_box,
            "columns": self.card_box.columns(),
            "density": self.density,
            "keys": keys,
        })
    }
}

/// Where a rule comes from, for its reason.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleSource {
    /// The case or the module's card.
    #[default]
    Group,
    /// A shipped or shared profile.
    Profile,
}

/// What a rule does when its clause holds. Part names are the card's
/// keys.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    /// The representation wanted for the group's parts (slow keys only);
    /// the size policy then says what fits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
    /// Parts folded under "More" (slow keys only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fold: Vec<String>,
    /// Parts shown that their tier or an earlier rule folded (slow keys
    /// only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub show: Vec<String>,
    /// Parts that do not apply (a raw-only part on a picture): slow or
    /// image keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inapplicable: Vec<String>,
    /// Parts scrolled into view and highlighted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emphasise: Vec<String>,
    /// Parts whose HUD chip is offered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hud: Vec<String>,
    /// A mode the group is offered in (a toolbar, a docked strip), added
    /// beside the panel, never instead of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<Mode>,
    /// Hit areas and chips grown (a pen, a touch screen).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub grow: bool,
}

/// A group's rule: a `when` clause over the context keys and its effect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub when: String,
    #[serde(default)]
    pub source: RuleSource,
    /// The reason shown for what it folds ("Raw only", "Folded while
    /// culling"); the clause when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub effect: Effect,
}

fn names(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|&p| p.to_owned()).collect()
}

impl Rule {
    /// A rule with no effect yet; the builders below add them.
    pub fn new(when: &str) -> Self {
        Self {
            when: when.into(),
            source: RuleSource::Group,
            reason: None,
            effect: Effect::default(),
        }
    }

    pub fn because(mut self, reason: &str) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn from_profile(mut self) -> Self {
        self.source = RuleSource::Profile;
        self
    }

    pub fn fold(mut self, parts: &[&str]) -> Self {
        self.effect.fold.extend(names(parts));
        self
    }

    pub fn show(mut self, parts: &[&str]) -> Self {
        self.effect.show.extend(names(parts));
        self
    }

    pub fn inapplicable(mut self, parts: &[&str]) -> Self {
        self.effect.inapplicable.extend(names(parts));
        self
    }

    pub fn emphasise(mut self, parts: &[&str]) -> Self {
        self.effect.emphasise.extend(names(parts));
        self
    }

    pub fn hud(mut self, parts: &[&str]) -> Self {
        self.effect.hud.extend(names(parts));
        self
    }

    pub fn represent(mut self, representation: Representation) -> Self {
        self.effect.representation = Some(representation);
        self
    }

    pub fn offer(mut self, mode: Mode) -> Self {
        self.effect.offer = Some(mode);
        self
    }

    pub fn grow(mut self) -> Self {
        self.effect.grow = true;
        self
    }

    /// Its clause, parsed.
    pub fn clause(&self) -> Result<When, String> {
        When::parse(&self.when).map_err(|e| format!("rule `{}`: {e}", self.when))
    }

    /// The class of its clause: the fastest of the keys it tests (slow
    /// for a clause that always holds). A key that is not a context key
    /// is refused.
    pub fn class(&self) -> Result<Class, String> {
        let clause = self.clause()?;
        let mut class = Class::Slow;
        for k in clause.keys() {
            let c = class_of(k)
                .ok_or_else(|| format!("rule `{}`: `{k}` is not a context key", self.when))?;
            class = class.max(c);
        }
        Ok(class)
    }

    /// What is wrong with it: a clause that does not parse or tests an
    /// unknown key, a part the group does not have, or an effect its
    /// class may not have (C1: a fast key never folds, shows, marks or
    /// changes a representation; the image key only marks parts that do
    /// not apply).
    pub fn check(&self, parts: &[&str]) -> Result<(), String> {
        let class = self.class()?;
        let e = &self.effect;
        for p in e
            .fold
            .iter()
            .chain(&e.show)
            .chain(&e.inapplicable)
            .chain(&e.emphasise)
            .chain(&e.hud)
        {
            if !parts.contains(&p.as_str()) {
                return Err(format!("rule `{}`: no part `{p}`", self.when));
            }
        }
        let refuse = |what: &str, why: &str| {
            Err(format!(
                "rule `{}`: {why} may not {what} (the class rule, C1)",
                self.when
            ))
        };
        match class {
            Class::Slow => Ok(()),
            Class::Image => {
                if !e.fold.is_empty() || !e.show.is_empty() {
                    refuse(
                        "fold or show parts by taste, only mark those that do not apply",
                        "the image key",
                    )
                } else if e.representation.is_some() {
                    refuse("change a representation", "the image key")
                } else {
                    Ok(())
                }
            }
            Class::Fast => {
                if !e.fold.is_empty() || !e.show.is_empty() || !e.inapplicable.is_empty() {
                    refuse("fold, show or mark parts", "a fast key (tool, device)")
                } else if e.representation.is_some() {
                    refuse("change a representation", "a fast key (tool, device)")
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Whether its clause holds (a clause that does not parse never
    /// does; [`Rule::check`] refuses it at registration).
    pub fn holds(&self, cx: &WhenContext) -> bool {
        self.clause().is_ok_and(|w| w.eval(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_has_half_a_unit_of_hysteresis() {
        assert_eq!(Space::derive(12, None), Space::Narrow);
        assert_eq!(Space::derive(24, None), Space::Regular);
        assert_eq!(Space::derive(48, None), Space::Wide);
        // Just past the narrow threshold: still narrow, until half a unit.
        assert_eq!(
            Space::derive(NARROW_BELOW + 2, Some(Space::Narrow)),
            Space::Narrow
        );
        assert_eq!(
            Space::derive(NARROW_BELOW + HYSTERESIS, Some(Space::Narrow)),
            Space::Regular
        );
        // And back: regular until half a unit below.
        assert_eq!(
            Space::derive(NARROW_BELOW - 2, Some(Space::Regular)),
            Space::Regular
        );
        assert_eq!(
            Space::derive(NARROW_BELOW - HYSTERESIS - 1, Some(Space::Regular)),
            Space::Narrow
        );
        assert_eq!(
            Space::derive(WIDE_FROM + 3, Some(Space::Regular)),
            Space::Regular
        );
        assert_eq!(Space::derive(WIDE_FROM - 3, Some(Space::Wide)), Space::Wide);
        assert_eq!(
            Space::derive(WIDE_FROM - 7, Some(Space::Wide)),
            Space::Regular
        );
    }

    #[test]
    fn the_box_counts_in_columns_at_the_density() {
        // 480 points: two columns comfortable, more compact, fewer touch.
        let c = CardBox::from_points(Surface::Panel, 480.0, 240.0, Density::Comfortable);
        assert_eq!(c.width, 24);
        let k = CardBox::from_points(Surface::Panel, 480.0, 240.0, Density::Compact);
        assert!(k.width > 24);
        let t = CardBox::from_points(Surface::Panel, 480.0, 240.0, Density::Touch);
        assert!(t.width < 24);
        let mut keys = ContextKeys::new(c);
        assert_eq!(keys.space, Space::Regular);
        keys.set_box(t);
        assert_eq!(keys.space, Space::Regular, "{t:?}");
    }

    #[test]
    fn keys_publish_as_when_context() {
        let keys = ContextKeys::default();
        let cx = keys.when_context();
        assert!(
            When::parse("ctl.task == 'develop' && ctl.image.raw")
                .unwrap()
                .eval(&cx)
        );
        assert!(!When::parse("ctl.image.masked").unwrap().eval(&cx));
        assert_eq!(cx.get("ctl.detail"), "standard");
        assert_eq!(cx.get("ctl.device"), "pointer");
        assert!(keys.to_context().iter().all(|(k, _)| class_of(k).is_some()));
    }

    #[test]
    fn tiers_fold_below_the_detail_level() {
        assert!(!Tier::Essential.folds_at(Detail::Essential));
        assert!(Tier::Standard.folds_at(Detail::Essential));
        assert!(!Tier::Standard.folds_at(Detail::Standard));
        // C2: the default detail folds the rare parts only.
        assert!(Tier::Expert.folds_at(Detail::default()));
        assert!(!Tier::Expert.folds_at(Detail::Expert));
    }

    #[test]
    fn the_class_rule_c1() {
        let parts = ["a", "b"];
        // Slow keys may fold, show, change a representation.
        let slow = Rule::new("ctl.task == 'cull'")
            .fold(&["a"])
            .represent(Representation::Simplified);
        assert_eq!(slow.class(), Ok(Class::Slow));
        assert!(slow.check(&parts).is_ok());
        // The image key marks what does not apply, never folds by taste.
        let image = Rule::new("!ctl.image.raw").inapplicable(&["a"]);
        assert_eq!(image.class(), Ok(Class::Image));
        assert!(image.check(&parts).is_ok());
        assert!(
            Rule::new("ctl.image.edited")
                .fold(&["a"])
                .check(&parts)
                .is_err()
        );
        // Fast keys add, grow, emphasise; never fold, show or mark.
        let fast = Rule::new("ctl.tool == 'gradient'")
            .emphasise(&["b"])
            .hud(&["b"])
            .offer(Mode::Toolbar);
        assert_eq!(fast.class(), Ok(Class::Fast));
        assert!(fast.check(&parts).is_ok());
        assert!(
            Rule::new("ctl.device == 'pen'")
                .grow()
                .check(&parts)
                .is_ok()
        );
        for bad in [
            Rule::new("ctl.tool == 'crop'").fold(&["a"]),
            Rule::new("ctl.device == 'touch'").show(&["a"]),
            Rule::new("ctl.device == 'touch'").inapplicable(&["a"]),
            Rule::new("ctl.device == 'midi'").represent(Representation::Compact),
            // A fast key anywhere in the clause makes the rule fast.
            Rule::new("ctl.task == 'cull' && ctl.tool == 'crop'").fold(&["a"]),
        ] {
            assert!(bad.check(&parts).is_err(), "{bad:?}");
        }
        // Unknown keys, parts and bad clauses are refused.
        assert!(
            Rule::new("place == 'darkroom'")
                .fold(&["a"])
                .check(&parts)
                .is_err()
        );
        assert!(
            Rule::new("ctl.task == 'cull'")
                .fold(&["z"])
                .check(&parts)
                .is_err()
        );
        assert!(Rule::new("ctl.task ==").check(&parts).is_err());
    }
}
