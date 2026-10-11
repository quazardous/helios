//! A mask group: darktable's flexi group fold (`_group_get_mask_roi_flexi`
//! in `src/develop/masks/group.c`).

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamHint, ParamKind, ParamSpec,
    Params, Region, Role, Value,
};

use crate::{flag, mask_port};

/// How many members a group takes: its inputs `in0` to `in7`, each optional.
pub const MAX_MEMBERS: usize = 8;

/// How a group folds its members, in port order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator {
    /// `max(a, b)`, darktable's default (union).
    Maximum,
    /// `a + b - ab`, a soft union.
    Screen,
    /// `min(a, b)`.
    Minimum,
    /// `a * b`.
    Product,
    /// `min(1, a + b)`.
    Sum,
    /// The first member, less each later one: `a * (1 - b)`.
    Difference,
    /// Classic exclusion, member by member: `max((1 - a) b, a (1 - b))`
    /// where both are positive, `max(a, b)` elsewhere. Not associative.
    Exclusion,
}

impl Operator {
    pub const NAMES: [&'static str; 7] = [
        "maximum",
        "screen",
        "minimum",
        "product",
        "sum",
        "difference",
        "exclusion",
    ];

    pub fn from_name(name: &str) -> Option<Operator> {
        Some(match name {
            "maximum" => Operator::Maximum,
            "screen" => Operator::Screen,
            "minimum" => Operator::Minimum,
            "product" => Operator::Product,
            "sum" => Operator::Sum,
            "difference" => Operator::Difference,
            "exclusion" => Operator::Exclusion,
            _ => return None,
        })
    }

    /// What the fold starts from: everything for minimum and product,
    /// nothing for the others (each copies its first member onto 0).
    fn seed(self) -> f32 {
        match self {
            Operator::Minimum | Operator::Product => 1.0,
            _ => 0.0,
        }
    }

    /// Folds one member `m` (opacity and inversion applied) into `d`.
    /// `first` says whether `m` is the first visible member: difference has
    /// no seed that copies, so its first member is copied.
    fn step(self, d: f32, m: f32, first: bool) -> f32 {
        let both = d > 0.0 && m > 0.0;
        match self {
            Operator::Maximum => d.max(m),
            Operator::Difference if first => d.max(m),
            Operator::Screen => d + m - d * m,
            Operator::Minimum => d.max(0.0).min(m.max(0.0)),
            Operator::Product => d * m,
            Operator::Sum => (d + m).min(1.0),
            Operator::Difference => d * (1.0 - if both { m } else { 0.0 }),
            Operator::Exclusion if both => ((1.0 - d) * m).max(d * (1.0 - m)),
            Operator::Exclusion => d.max(m),
        }
    }
}

/// Folds members, each `(value, opacity, invert)`, in order: inversion
/// first (`1 - value`), then opacity, as darktable does for a member. `None`
/// when there is no member.
pub fn fold(op: Operator, members: impl IntoIterator<Item = (f32, f32, bool)>) -> Option<f32> {
    let mut d = op.seed();
    let mut first = true;
    for (value, opacity, invert) in members {
        let m = opacity * if invert { 1.0 - value } else { value };
        d = op.step(d, m, first);
        first = false;
    }
    (!first).then_some(d)
}

/// A group of up to [`MAX_MEMBERS`] masks: fold, then group inversion, then
/// group opacity. A group's refinement, when it has one, is a chain of
/// refinement modules after it (see `docs/masks.md`).
pub struct MaskCombine;

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub op: Operator,
    /// Opacity and inversion of each member, by port.
    pub members: [(f32, bool); MAX_MEMBERS],
    pub invert: bool,
    pub opacity: f32,
}

impl Module for MaskCombine {
    type Plan = Plan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "mask_combine".into(),
            version: 1,
            description: "A mask group: folds its members with one operator, then inverts and scales the result".into(),
            params: vec![
                ParamSpec {
                    name: "operator".into(),
                    description: "how the members fold together, in port order".into(),
                    kind: ParamKind::Choice {
                        options: Operator::NAMES.iter().map(|s| s.to_string()).collect(),
                        default: "maximum".into(),
                    },
                },
                ParamSpec {
                    name: "opacity".into(),
                    description: "each member's opacity, by port (missing values are 1)".into(),
                    kind: ParamKind::Floats {
                        count: MAX_MEMBERS as u32,
                        min: 0.0,
                        max: 1.0,
                        default: vec![1.0; MAX_MEMBERS],
                    },
                },
                ParamSpec {
                    name: "invert".into(),
                    description: "1 to invert a member before its opacity, by port".into(),
                    kind: ParamKind::Ints {
                        count: MAX_MEMBERS as u32,
                        min: 0,
                        max: 1,
                        default: vec![0; MAX_MEMBERS],
                    },
                },
                ParamSpec {
                    name: "group_invert".into(),
                    description: "invert the folded mask".into(),
                    kind: ParamKind::Bool { default: false },
                },
                ParamSpec::float(
                    "group_opacity",
                    "multiplies the folded mask, after its inversion",
                    0.0,
                    1.0,
                    1.0,
                ),
            ],
            inputs: (0..MAX_MEMBERS)
                .map(|i| mask_port(&format!("in{i}")).optional())
                .collect(),
            outputs: vec![mask_port("mask")],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Mask,
            role: Role::General,
        }
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        vec![ParamHint::new("group_opacity").unit("%").display(100.0, 0)]
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<Plan, String> {
        let name = params
            .get("operator")
            .and_then(Value::as_choice)
            .unwrap_or("maximum");
        let op = Operator::from_name(name).ok_or_else(|| format!("unknown operator `{name}`"))?;
        let opacities = params
            .get("opacity")
            .and_then(Value::as_list)
            .unwrap_or(&[]);
        let inverts: &[i64] = match params.get("invert") {
            Some(Value::Ints(v)) => v,
            _ => &[],
        };
        let mut members = [(1.0, false); MAX_MEMBERS];
        for (i, m) in members.iter_mut().enumerate() {
            *m = (
                opacities.get(i).copied().unwrap_or(1.0) as f32,
                inverts.get(i).is_some_and(|v| *v != 0),
            );
        }
        Ok(Plan {
            op,
            members,
            invert: flag(params, "group_invert"),
            opacity: params.float("group_opacity") as f32,
        })
    }

    fn process(
        &self,
        plan: &Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let present: Vec<(&Image, (f32, bool))> = inputs
            .iter()
            .zip(plan.members)
            .filter(|(image, _)| !image.is_absent())
            .map(|(image, m)| (*image, m))
            .collect();
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            let members = present
                .iter()
                .map(|(image, (opacity, invert))| (image.pixel(x, y)[0], *opacity, *invert));
            // No visible member: darktable's "no active mask element", the
            // module applies fully.
            let Some(d) = fold(plan.op, members) else {
                return 1.0;
            };
            let d = if plan.invert { 1.0 - d } else { d };
            d * plan.opacity
        })])
    }
}
