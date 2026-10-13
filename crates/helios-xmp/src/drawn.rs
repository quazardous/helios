//! darktable's drawn masks, as a sidecar keeps them
//! (`darktable:masks_history`), mapped onto Helios's shape members
//! (`mask_circle`, `mask_ellipse`, `mask_gradient`, `mask_path`,
//! `mask_brush`) and groups (`mask_combine`). See `docs/masks.md`, "From
//! darktable's drawn masks".
//!
//! darktable stores a form's points as its C structs, little-endian, in
//! coordinates normalised to the image the pipe starts from (the raw's
//! full size): positions by width and height, radii and feathers by the
//! smaller side, a gradient's compression by the diagonal. Helios's shapes
//! are in pixels of the image its network reads, which the loader may have
//! cropped: [`Frame`] says how the two relate.
//!
//! What does not map is said, never dropped silently: each mapped member
//! lists what it loses, and forms Helios cannot draw (clones, AI objects,
//! unknown types or versions) come back as [`Mapped::Skipped`] with the
//! [`Problem`]. [`network`] builds a module's whole mask, or says why it
//! cannot: a mask that would differ from darktable's is never built.

use helios_core::{Message, Params, Value};
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use serde::Serialize;

use crate::sidecar::decode_params;

/// darktable's form types (`dt_masks_type_t`), as bits.
pub mod kind {
    pub const CIRCLE: u32 = 1;
    pub const PATH: u32 = 1 << 1;
    pub const GROUP: u32 = 1 << 2;
    pub const CLONE: u32 = 1 << 3;
    pub const GRADIENT: u32 = 1 << 4;
    pub const ELLIPSE: u32 = 1 << 5;
    pub const BRUSH: u32 = 1 << 6;
    pub const NON_CLONE: u32 = 1 << 7;
    pub const OBJECT: u32 = 1 << 8;
    /// The flexi masks' parametric member (masks version 7).
    pub const PARAMETRIC: u32 = 1 << 9;
    /// The flexi masks' raster member (masks version 7).
    pub const RASTER: u32 = 1 << 10;
}

/// A member's state in its group (`dt_masks_state_t`), as bits.
pub mod state {
    pub const INVERSE: i32 = 1 << 2;
    pub const UNION: i32 = 1 << 3;
    pub const INTERSECTION: i32 = 1 << 4;
    pub const DIFFERENCE: i32 = 1 << 5;
    pub const EXCLUSION: i32 = 1 << 6;
    pub const SUM: i32 = 1 << 7;
    /// Flexi (masks version 7): a member hidden by solo, or disabled.
    pub const HIDDEN: i32 = 1 << 8;
    pub const DISABLE: i32 = 1 << 17;
    /// Flexi: on a group's marker, its operator (none: maximum).
    pub const FLEXI_SCREEN: i32 = 1 << 9;
    pub const FLEXI_MINIMUM: i32 = 1 << 12;
    pub const FLEXI_PRODUCT: i32 = 1 << 15;
    pub const FLEXI_SUM: i32 = 1 << 19;
    pub const FLEXI_DIFFERENCE: i32 = 1 << 20;
    pub const FLEXI_EXCLUSION: i32 = 1 << 21;
    /// Flexi: on a group's marker, bypass (the group contributes nothing)
    /// and inversion of the folded mask.
    pub const OP_BYPASS: i32 = 1 << 14;
    pub const OP_INVERT: i32 = 1 << 16;
    /// Flexi: this point is its group's marker (its settings), not a member.
    pub const GROUP_MARKER: i32 = 1 << 18;
}

/// The versions of darktable's masks this reads: 6 is darktable's current
/// (4 and 5 have the same structs, with defaults for what came later), 7
/// the flexi masks' (pull request darktable-org/darktable#22553, pinned in
/// `docs/masks.md`), whose group points grow from 16 to 240 bytes. A later
/// version is refused, so that a format change shows.
pub const VERSIONS: std::ops::RangeInclusive<u32> = 4..=7;

/// The bytes of a group point at masks `version`.
fn group_stride(version: u32) -> usize {
    if version >= 7 { 240 } else { 16 }
}

/// The most points a Helios path holds (`helios_module_masks::shape`).
const MAX_PATH_POINTS: usize = 64;

/// The most points a Helios brush stroke holds.
const MAX_BRUSH_POINTS: usize = 256;

/// How deep darktable nests groups (`DT_MASKS_NESTING_MAX`).
pub const MAX_NESTING: usize = 8;

/// The most members a Helios group folds (`helios_module_masks::MAX_MEMBERS`).
const MAX_MEMBERS: usize = 8;

/// One form of the sidecar's masks history.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Form {
    /// The history item it belongs to (`mask_num`).
    pub num: u32,
    pub id: i64,
    /// [`kind`] bits.
    pub kind: u32,
    pub name: String,
    pub version: u32,
    /// How many points `points` holds.
    pub count: u32,
    /// The points as darktable wrote them, decoded from hex or `gz`.
    #[serde(skip)]
    pub points: Vec<u8>,
}

/// How darktable's normalised coordinates relate to Helios's pixels: the
/// size of the image darktable's pipe starts from, and where Helios's
/// image starts in it (the loader's crop).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Frame {
    pub width: f64,
    pub height: f64,
    /// Helios's `(0, 0)` in darktable's pixels.
    pub x0: f64,
    pub y0: f64,
}

impl Frame {
    /// A frame without crop.
    pub fn new(width: f64, height: f64) -> Frame {
        Frame {
            width,
            height,
            x0: 0.0,
            y0: 0.0,
        }
    }

    fn x(&self, nx: f32) -> f64 {
        f64::from(nx) * self.width - self.x0
    }

    fn y(&self, ny: f32) -> f64 {
        f64::from(ny) * self.height - self.y0
    }

    /// A length normalised by the smaller side.
    fn length(&self, n: f32) -> f64 {
        f64::from(n) * self.width.min(self.height)
    }
}

/// Why a form, or a whole mask, is not carried over.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "problem", rename_all = "snake_case")]
pub enum Problem {
    /// A form Helios has no member for (a clone, an AI object).
    Unsupported { form: String },
    /// A form type darktable does not have, as far as Helios knows.
    UnknownType { kind: u32 },
    /// A masks version Helios does not read.
    Version { version: u32 },
    /// Points that do not match the form's type and count.
    Size { bytes: usize, expected: usize },
    /// More points than a Helios member holds.
    TooManyPoints { count: u32, max: usize },
    /// Something darktable's form holds that the member would lose: the
    /// mask would differ.
    Lost { what: String },
    /// A group refers to a form that is not in the history.
    Missing { id: i64 },
    /// Groups nested deeper than darktable allows (or in a loop).
    TooDeep,
    /// A group without members.
    EmptyGroup { id: i64 },
}

impl Problem {
    /// In a few words, for people, by key.
    pub fn describe(&self) -> Message {
        match self {
            Problem::Unsupported { form } => Message::new(
                "xmp-mask-unsupported",
                "a { $form } shape, which Helios does not draw yet",
            )
            .with("form", form.as_str()),
            Problem::UnknownType { kind } => {
                Message::new("xmp-mask-unknown-type", "an unknown shape type ({ $kind })")
                    .with("kind", *kind)
            }
            Problem::Version { version } => Message::new(
                "xmp-mask-version",
                "masks version { $version } not supported (reads 4 to 7)",
            )
            .with("version", *version),
            Problem::Size { bytes, expected } => Message::new(
                "xmp-mask-size",
                "shape points of { $bytes } bytes, { $expected } expected",
            )
            .with("bytes", *bytes)
            .with("expected", *expected),
            Problem::TooManyPoints { count, max } => Message::new(
                "xmp-mask-too-many-points",
                "a shape of { $count } points, at most { $max }",
            )
            .with("count", *count)
            .with("max", *max),
            Problem::Lost { what } => {
                Message::new("xmp-mask-lost", "the mask would lose { $what }")
                    .with("what", what.as_str())
            }
            Problem::Missing { id } => Message::new(
                "xmp-mask-missing",
                "a mask group refers to shape { $id }, not in the history",
            )
            .with("id", *id),
            Problem::TooDeep => Message::new("xmp-mask-too-deep", "mask groups nested too deep"),
            Problem::EmptyGroup { id } => {
                Message::new("xmp-mask-empty-group", "mask group { $id } is empty").with("id", *id)
            }
        }
    }
}

/// A shape member: the Helios module and its parameters.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Member {
    pub form: i64,
    pub name: String,
    pub module: &'static str,
    pub params: Params,
    /// What darktable's form holds that the member does not.
    pub losses: Vec<Problem>,
}

/// How a member folds into what its group holds before it (darktable's
/// classic group, `_group_get_mask_roi`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fold {
    /// `max(d, m)`.
    Union,
    /// `min(max(d, 0), max(m, 0))`.
    Intersection,
    /// `d · (1 − m)` where both are positive.
    Difference,
    /// classic's exclusion.
    Exclusion,
    /// `min(1, d + m)`.
    Sum,
    /// No operator: the member replaces what came before.
    Replace,
}

impl Fold {
    /// The Helios `mask_combine` operator folding the same way, `None` for
    /// [`Fold::Replace`].
    pub fn operator(self) -> Option<&'static str> {
        match self {
            Fold::Union => Some("maximum"),
            Fold::Intersection => Some("minimum"),
            Fold::Difference => Some("difference"),
            Fold::Exclusion => Some("exclusion"),
            Fold::Sum => Some("sum"),
            Fold::Replace => None,
        }
    }

    fn of(s: i32) -> Fold {
        if s & state::UNION != 0 {
            Fold::Union
        } else if s & state::INTERSECTION != 0 {
            Fold::Intersection
        } else if s & state::DIFFERENCE != 0 {
            Fold::Difference
        } else if s & state::SUM != 0 {
            Fold::Sum
        } else if s & state::EXCLUSION != 0 {
            Fold::Exclusion
        } else {
            Fold::Replace
        }
    }
}

/// A mask refinement of the flexi masks (`dt_masks_refinement_t`): on a
/// member (element scope) or on a group's marker (group scope).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Refinement {
    /// 0 off, 1 element scope, 2 group scope.
    pub scope: i32,
    pub details: f32,
    pub feathering_radius: f32,
    pub blur_radius: f32,
    pub contrast: f32,
    pub brightness: f32,
}

/// A member of a group: the form it refers to, how it folds into the
/// members before it (classic groups), its inversion and opacity.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GroupMember {
    pub form: i64,
    pub fold: Fold,
    pub invert: bool,
    pub opacity: f64,
    /// Flexi: hidden or disabled, so not folded.
    pub hidden: bool,
    /// Masks version 7: the member's own refinement.
    pub refinement: Refinement,
}

/// A flexi group's settings, held by its marker.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Marker {
    /// The Helios `mask_combine` operator.
    pub operator: &'static str,
    pub invert: bool,
    /// Bypassed: the group contributes nothing.
    pub bypass: bool,
    pub opacity: f64,
    pub refinement: Refinement,
}

/// A group of forms: darktable's classic group, or a flexi group when its
/// points start with a marker.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Group {
    pub form: i64,
    pub name: String,
    pub marker: Option<Marker>,
    pub members: Vec<GroupMember>,
}

/// A form, mapped.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Mapped {
    Shape(Member),
    Group(Group),
    /// A form Helios does not draw, and why.
    Skipped {
        form: i64,
        name: String,
        problem: Problem,
    },
}

/// Reads the masks history of a sidecar's text, in its order.
pub fn read(xml: &str) -> Result<Vec<Form>, String> {
    let mut reader = Reader::from_str(xml);
    let mut forms = Vec::new();
    let mut inside = false;
    loop {
        match reader.read_event().map_err(|e| format!("XMP: {e}"))? {
            Event::Start(e) if e.name().as_ref() == "darktable:masks_history" => inside = true,
            Event::End(e) if e.name().as_ref() == "darktable:masks_history" => inside = false,
            Event::Start(e) | Event::Empty(e) if inside && e.name().as_ref() == "rdf:li" => {
                forms.push(form(&e)?);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(forms)
}

fn form(e: &BytesStart) -> Result<Form, String> {
    let mut attrs = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|e| e.to_string())?;
        let value = a
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|e| e.to_string())?;
        attrs.push((a.key.as_ref().to_string(), value.into_owned()));
    }
    let get = |key: &str| {
        attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    let number = |key: &str| -> Result<i64, String> {
        let v = get(key).ok_or_else(|| format!("mask without `{key}`"))?;
        v.parse()
            .map_err(|_| format!("`{key}`: `{v}` is not a number"))
    };
    let points = get("darktable:mask_points").ok_or("mask without `darktable:mask_points`")?;
    Ok(Form {
        num: u32::try_from(number("darktable:mask_num")?).map_err(|e| e.to_string())?,
        id: number("darktable:mask_id")?,
        kind: u32::try_from(number("darktable:mask_type")?).map_err(|e| e.to_string())?,
        name: get("darktable:mask_name").unwrap_or("").to_string(),
        version: u32::try_from(number("darktable:mask_version")?).map_err(|e| e.to_string())?,
        count: u32::try_from(number("darktable:mask_nb")?).map_err(|e| e.to_string())?,
        points: decode_params(points)?,
    })
}

/// The forms in force once the history is applied up to `history_end`
/// items: darktable keeps all forms with each history item that changed
/// them, so they are those of the last such item before the end.
pub fn in_force(forms: &[Form], history_end: Option<u32>) -> Vec<&Form> {
    let end = history_end.unwrap_or(u32::MAX);
    let Some(num) = forms.iter().map(|f| f.num).filter(|n| *n < end).max() else {
        return Vec::new();
    };
    forms.iter().filter(|f| f.num == num).collect()
}

/// Little-endian fields of a form's points.
struct Points<'a> {
    bytes: &'a [u8],
    size: usize,
}

impl Points<'_> {
    fn f(&self, point: usize, field: usize) -> f32 {
        let at = point * self.size + field * 4;
        f32::from_le_bytes(self.bytes[at..at + 4].try_into().expect("four bytes"))
    }

    fn i(&self, point: usize, field: usize) -> i32 {
        let at = point * self.size + field * 4;
        i32::from_le_bytes(self.bytes[at..at + 4].try_into().expect("four bytes"))
    }
}

fn float(name: &str, v: f64) -> (String, Value) {
    (name.into(), Value::Float(v))
}

/// The low and high of some values, and whether they differ by more than
/// 1 % of their mean.
fn spread(values: &[f32]) -> (f32, f32, bool) {
    let mean = values.iter().map(|b| f64::from(*b)).sum::<f64>() / values.len().max(1) as f64;
    let (lo, hi) = values
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), b| {
            (lo.min(*b), hi.max(*b))
        });
    (lo, hi, f64::from(hi - lo) > 0.01 * mean.abs().max(1e-6))
}

/// Maps one form. `frame` places darktable's normalised coordinates in
/// Helios's image.
pub fn map(form: &Form, frame: &Frame) -> Mapped {
    let skip = |problem: Problem| Mapped::Skipped {
        form: form.id,
        name: form.name.clone(),
        problem,
    };
    let k = form.kind;
    let (size, module) = if k & kind::CLONE != 0 {
        return skip(Problem::Unsupported {
            form: "clone (retouch)".into(),
        });
    } else if k & kind::OBJECT != 0 {
        return skip(Problem::Unsupported {
            form: "AI object".into(),
        });
    } else if k & kind::PARAMETRIC != 0 {
        return skip(Problem::Unsupported {
            form: "parametric".into(),
        });
    } else if k & kind::RASTER != 0 {
        return skip(Problem::Unsupported {
            form: "raster".into(),
        });
    } else if k & kind::GROUP != 0 {
        (group_stride(form.version), "")
    } else if k & kind::CIRCLE != 0 {
        (16, "mask_circle")
    } else if k & kind::ELLIPSE != 0 {
        (28, "mask_ellipse")
    } else if k & kind::GRADIENT != 0 {
        (28, "mask_gradient")
    } else if k & kind::PATH != 0 {
        (36, "mask_path")
    } else if k & kind::BRUSH != 0 {
        (44, "mask_brush")
    } else {
        return skip(Problem::UnknownType { kind: k });
    };
    if !VERSIONS.contains(&form.version) {
        return skip(Problem::Version {
            version: form.version,
        });
    }
    let count = form.count as usize;
    if count == 0 || form.points.len() != count * size {
        return skip(Problem::Size {
            bytes: form.points.len(),
            expected: count.max(1) * size,
        });
    }
    let p = Points {
        bytes: &form.points,
        size,
    };
    if k & kind::GROUP != 0 {
        return Mapped::Group(group(form, &p));
    }
    let mut losses = Vec::new();
    let params: Vec<(String, Value)> = match module {
        "mask_circle" => vec![
            float("x", frame.x(p.f(0, 0))),
            float("y", frame.y(p.f(0, 1))),
            float("radius", frame.length(p.f(0, 2))),
            float("feather", frame.length(p.f(0, 3))),
        ],
        "mask_ellipse" => {
            let (a, b) = (frame.length(p.f(0, 2)), frame.length(p.f(0, 3)));
            let border = p.f(0, 5);
            let proportional = form.version >= 4 && p.i(0, 6) & 1 != 0;
            let feather = if proportional {
                f64::from(border) * a.max(b)
            } else {
                frame.length(border)
            };
            vec![
                float("x", frame.x(p.f(0, 0))),
                float("y", frame.y(p.f(0, 1))),
                float("radius_x", a),
                float("radius_y", b),
                float("rotation", f64::from(p.f(0, 4))),
                float("feather", feather),
                ("proportional".into(), Value::Bool(proportional)),
            ]
        }
        "mask_gradient" => {
            let diagonal = frame.width.hypot(frame.height);
            let compression = f64::from(p.f(0, 3)).max(0.001);
            let curvature = if form.version >= 5 { p.f(0, 5) } else { 0.0 };
            if curvature != 0.0 {
                losses.push(Problem::Lost {
                    what: format!("a gradient's curvature ({curvature})"),
                });
            }
            let sigmoid = form.version >= 6 && p.i(0, 6) == 2;
            vec![
                float("x", frame.x(p.f(0, 0))),
                float("y", frame.y(p.f(0, 1))),
                float("rotation", f64::from(p.f(0, 2))),
                float("feather", 2.0 * compression * diagonal),
                (
                    "profile".into(),
                    Value::Choice(if sigmoid { "sigmoid" } else { "linear" }.into()),
                ),
            ]
        }
        "mask_path" => {
            if count > MAX_PATH_POINTS {
                return skip(Problem::TooManyPoints {
                    count: form.count,
                    max: MAX_PATH_POINTS,
                });
            }
            let mut values = Vec::with_capacity(count * 6);
            let mut borders = Vec::with_capacity(count * 2);
            for i in 0..count {
                // corner, ctrl1 (before), ctrl2 (after).
                for (fx, fy) in [(0, 1), (2, 3), (4, 5)] {
                    values.push(frame.x(p.f(i, fx)));
                    values.push(frame.y(p.f(i, fy)));
                }
                borders.extend([p.f(i, 6), p.f(i, 7)]);
            }
            let (lo, hi, differ) = spread(&borders);
            if differ {
                losses.push(Problem::Lost {
                    what: format!("a path's feather per point ({lo} to {hi})"),
                });
            }
            let mean = borders.iter().map(|b| f64::from(*b)).sum::<f64>() / borders.len() as f64;
            vec![
                ("points".into(), Value::List(values)),
                float("feather", frame.length(mean as f32)),
            ]
        }
        _ => {
            if count > MAX_BRUSH_POINTS {
                return skip(Problem::TooManyPoints {
                    count: form.count,
                    max: MAX_BRUSH_POINTS,
                });
            }
            // corner, ctrl1, ctrl2, border[2], density, hardness, state.
            let mut values = Vec::with_capacity(count * 9);
            for i in 0..count {
                for (fx, fy) in [(0, 1), (2, 3), (4, 5)] {
                    values.push(frame.x(p.f(i, fx)));
                    values.push(frame.y(p.f(i, fy)));
                }
                let (b0, b1) = (p.f(i, 6), p.f(i, 7));
                if spread(&[b0, b1]).2 {
                    losses.push(Problem::Lost {
                        what: format!("a brush point's two sizes ({b0} and {b1})"),
                    });
                }
                values.push(frame.length(0.5 * (b0 + b1)));
                values.push(f64::from(p.f(i, 9)));
                values.push(f64::from(p.f(i, 8)));
            }
            vec![("points".into(), Value::List(values))]
        }
    };
    Mapped::Shape(Member {
        form: form.id,
        name: form.name.clone(),
        module,
        params: params
            .into_iter()
            .fold(Params::new(), |p, (k, v)| p.with(&k, v)),
        losses,
    })
}

fn group(form: &Form, p: &Points) -> Group {
    let v7 = form.version >= 7;
    // A version 7 point's refinement: enabled, details, feathering radius
    // and guide, blur, contrast, brightness.
    let refinement = |i: usize| {
        if !v7 {
            return Refinement::default();
        }
        Refinement {
            scope: p.i(i, 4),
            details: p.f(i, 5),
            feathering_radius: p.f(i, 6),
            blur_radius: p.f(i, 8),
            contrast: p.f(i, 9),
            brightness: p.f(i, 10),
        }
    };
    let mut marker = None;
    let mut members = Vec::with_capacity(form.count as usize);
    for i in 0..form.count as usize {
        let s = p.i(i, 2);
        if v7 && s & state::GROUP_MARKER != 0 {
            // group_opacity, after the name (128 bytes from offset 44).
            let operator = if s & state::FLEXI_SCREEN != 0 {
                "screen"
            } else if s & state::FLEXI_MINIMUM != 0 {
                "minimum"
            } else if s & state::FLEXI_PRODUCT != 0 {
                "product"
            } else if s & state::FLEXI_SUM != 0 {
                "sum"
            } else if s & state::FLEXI_DIFFERENCE != 0 {
                "difference"
            } else if s & state::FLEXI_EXCLUSION != 0 {
                "exclusion"
            } else {
                "maximum"
            };
            marker = Some(Marker {
                operator,
                invert: s & state::OP_INVERT != 0,
                bypass: s & state::OP_BYPASS != 0,
                opacity: f64::from(p.f(i, 43)),
                refinement: refinement(i),
            });
            continue;
        }
        members.push(GroupMember {
            form: i64::from(p.i(i, 0)),
            fold: Fold::of(s),
            invert: s & state::INVERSE != 0,
            opacity: f64::from(p.f(i, 3)),
            hidden: v7 && s & (state::HIDDEN | state::DISABLE) != 0,
            refinement: refinement(i),
        });
    }
    Group {
        form: form.id,
        name: form.name.clone(),
        marker,
        members,
    }
}

// ------------------------------------------------------------------ network

/// A node of a mask network.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MaskNode {
    pub name: String,
    pub module: &'static str,
    pub params: Params,
}

/// A connection from a node's output `port` into another's `input`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Link {
    pub from: String,
    pub port: String,
    pub to: String,
    pub input: String,
}

/// A module's mask as nodes: shapes, groups and refinement. Its output is
/// `mask`'s `mask` port. Links from [`INPUT`] or [`OUTPUT`] read the
/// masked module's input or output (a feathering guide).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MaskNet {
    pub nodes: Vec<MaskNode>,
    pub links: Vec<Link>,
    pub mask: String,
}

/// The masked module's input, as a link's source.
pub const INPUT: &str = "@input";
/// The masked module's output, as a link's source.
pub const OUTPUT: &str = "@output";

impl MaskNet {
    fn add(&mut self, name: String, module: &'static str, params: Params) -> String {
        self.nodes.push(MaskNode {
            name: name.clone(),
            module,
            params,
        });
        name
    }

    fn link(&mut self, from: &str, port: &str, to: &str, input: &str) {
        self.links.push(Link {
            from: from.into(),
            port: port.into(),
            to: to.into(),
            input: input.into(),
        });
    }

    /// Appends a node reading the current mask on `mask` and makes it the
    /// output.
    pub fn then(&mut self, name: String, module: &'static str, params: Params) {
        let from = std::mem::take(&mut self.mask);
        let to = self.add(name, module, params);
        self.link(&from, "mask", &to, "mask");
        self.mask = to;
    }

    /// Appends a group folding one member: the current mask, inverted.
    pub fn invert(&mut self, name: String) {
        let from = std::mem::take(&mut self.mask);
        let to = self.add(
            name,
            "mask_combine",
            Params::new().with("invert", Value::Ints(vec![1])),
        );
        self.link(&from, "mask", &to, "in0");
        self.mask = to;
    }
}

/// A member's mask on its way into a group's port.
struct Port {
    from: String,
    invert: bool,
    opacity: f64,
}

/// Builds the network of a module's drawn mask, the form `root` (darktable's
/// `mask_id`), from the forms in force. Node names start with `prefix`.
/// Every form must map without loss, or the mask is not built: the first
/// problem met is returned.
pub fn network(
    forms: &[&Form],
    root: i64,
    frame: &Frame,
    prefix: &str,
) -> Result<MaskNet, Problem> {
    build(forms, root, frame, prefix, false)
}

/// [`network`] for a flexi mask (the blend's `DEVELOP_MASK_FLEXI`): each
/// group folds its visible members with its marker's operator, then its
/// refinement, inversion and opacity (`_group_get_mask_roi_flexi`). A
/// group that contributes nothing (bypassed, or no visible member) is left
/// out of its parent; at the root it is darktable's "no active mask
/// element", 1 everywhere.
pub fn network_flexi(
    forms: &[&Form],
    root: i64,
    frame: &Frame,
    prefix: &str,
) -> Result<MaskNet, Problem> {
    build(forms, root, frame, prefix, true)
}

fn build(
    forms: &[&Form],
    root: i64,
    frame: &Frame,
    prefix: &str,
    flexi: bool,
) -> Result<MaskNet, Problem> {
    let mut b = Builder {
        forms,
        frame,
        prefix,
        flexi,
        net: MaskNet::default(),
        built: Vec::new(),
    };
    b.net.mask = match b.form(root, 0)? {
        Some(mask) => mask,
        None => b.net.add(
            format!("{prefix} group {root}.empty"),
            "mask_combine",
            Params::new(),
        ),
    };
    Ok(b.net)
}

struct Builder<'a> {
    forms: &'a [&'a Form],
    frame: &'a Frame,
    prefix: &'a str,
    flexi: bool,
    net: MaskNet,
    /// Forms already built: their node (`None`: nothing to fold).
    built: Vec<(i64, Option<String>)>,
}

impl Builder<'_> {
    /// The node giving form `id`'s mask, `None` for a flexi group that
    /// contributes nothing.
    fn form(&mut self, id: i64, depth: usize) -> Result<Option<String>, Problem> {
        if depth > MAX_NESTING {
            return Err(Problem::TooDeep);
        }
        if let Some((_, name)) = self.built.iter().find(|(f, _)| *f == id) {
            return Ok(name.clone());
        }
        let form = self
            .forms
            .iter()
            .find(|f| f.id == id)
            .ok_or(Problem::Missing { id })?;
        let name = match map(form, self.frame) {
            Mapped::Skipped { problem, .. } => return Err(problem),
            Mapped::Shape(member) => {
                if let Some(lost) = member.losses.into_iter().next() {
                    return Err(lost);
                }
                Some(self.net.add(
                    format!("{} shape {id}", self.prefix),
                    member.module,
                    member.params,
                ))
            }
            Mapped::Group(group) if self.flexi => self.flexi_group(&group, depth)?,
            Mapped::Group(group) => Some(self.group(&group, depth)?),
        };
        self.built.push((id, name.clone()));
        Ok(name)
    }

    /// A classic group as a chain of `mask_combine`s: darktable folds from
    /// 0, member by member, each with its own operator. Consecutive members
    /// with the same operator share one node (up to 8 ports); the chain
    /// carries what came before in the next node's first port.
    fn group(&mut self, group: &Group, depth: usize) -> Result<String, Problem> {
        if group.members.is_empty() {
            return Err(Problem::EmptyGroup { id: group.form });
        }
        if group.marker.is_some() {
            return Err(Problem::Lost {
                what: "a flexi group in a classic mask".into(),
            });
        }
        let mut members = Vec::with_capacity(group.members.len());
        for (k, m) in group.members.iter().enumerate() {
            let from = self.form(m.form, depth + 1)?.ok_or(Problem::TooDeep)?;
            let from = self.refine(from, &m.refinement, 1, &format!("{}.{k}", group.form))?;
            members.push((
                m.fold,
                Port {
                    from,
                    invert: m.invert,
                    opacity: m.opacity,
                },
            ));
        }
        let id = group.form;
        let mut parts = 0;
        let mut node =
            |net: &mut MaskNet, operator: &str, ports: Vec<Port>, constant: Option<f64>| {
                let name = format!("{} group {id}.{parts}", self.prefix);
                parts += 1;
                let mut params = Params::new().with("operator", Value::Choice(operator.into()));
                params.set(
                    "opacity",
                    Value::List(ports.iter().map(|p| p.opacity).collect()),
                );
                params.set(
                    "invert",
                    Value::Ints(ports.iter().map(|p| i64::from(p.invert)).collect()),
                );
                if let Some(c) = constant {
                    // No member: 1, scaled to the constant.
                    params.set("group_opacity", Value::Float(c));
                }
                let name = net.add(name, "mask_combine", params);
                for (i, p) in ports.iter().enumerate() {
                    net.link(&p.from, "mask", &name, &format!("in{i}"));
                }
                name
            };
        // What the fold holds so far: none yet (darktable's 0).
        let mut acc: Option<String> = None;
        let mut i = 0;
        while i < members.len() {
            let fold = members[i].0;
            let mut run = vec![];
            while i < members.len() && members[i].0 == fold && run.len() < MAX_MEMBERS - 1 {
                let (_, port) = std::mem::replace(
                    &mut members[i],
                    (
                        fold,
                        Port {
                            from: String::new(),
                            invert: false,
                            opacity: 0.0,
                        },
                    ),
                );
                run.push(port);
                i += 1;
                if fold == Fold::Replace {
                    break;
                }
            }
            let carried = |acc: &String| Port {
                from: acc.clone(),
                invert: false,
                opacity: 1.0,
            };
            acc = Some(match (fold, acc.take()) {
                // Replace: the member alone (a maximum of one).
                (Fold::Replace, _) => node(&mut self.net, "maximum", run, None),
                // From darktable's 0, union, sum and exclusion give the
                // first member: Helios folds from 0 too.
                (Fold::Union | Fold::Sum | Fold::Exclusion, None) => node(
                    &mut self.net,
                    fold.operator().expect("an operator"),
                    run,
                    None,
                ),
                // Intersection and difference of 0 stay 0.
                (Fold::Intersection | Fold::Difference, None) => {
                    let zero = node(&mut self.net, "maximum", Vec::new(), Some(0.0));
                    let mut ports = vec![carried(&zero)];
                    ports.extend(run);
                    node(
                        &mut self.net,
                        fold.operator().expect("an operator"),
                        ports,
                        None,
                    )
                }
                (_, Some(before)) => {
                    let mut ports = vec![carried(&before)];
                    ports.extend(run);
                    node(
                        &mut self.net,
                        fold.operator().expect("an operator"),
                        ports,
                        None,
                    )
                }
            });
        }
        Ok(acc.expect("a group with members"))
    }

    /// `from` through `r` when it has the `scope`: blur, then contrast and
    /// brightness, as darktable's `dt_develop_blend_refine_form_mask`.
    /// Feathering and the details threshold would give another mask.
    fn refine(
        &mut self,
        from: String,
        r: &Refinement,
        scope: i32,
        key: &str,
    ) -> Result<String, Problem> {
        if r.scope != scope {
            return Ok(from);
        }
        if r.feathering_radius > 0.1 {
            return Err(Problem::Lost {
                what: "a refinement's feathering".into(),
            });
        }
        if r.details != 0.0 {
            return Err(Problem::Lost {
                what: "a refinement's details threshold".into(),
            });
        }
        let mut net = MaskNet {
            mask: from,
            ..MaskNet::default()
        };
        if r.blur_radius > 0.1 {
            net.then(
                format!("{} refine {key} blur", self.prefix),
                "mask_blur",
                Params::new().with("radius", Value::Float(f64::from(r.blur_radius))),
            );
        }
        if r.contrast.abs() >= 0.01 || r.brightness.abs() >= 0.01 {
            net.then(
                format!("{} refine {key} levels", self.prefix),
                "mask_levels",
                Params::new()
                    .with("contrast", Value::Float(f64::from(r.contrast)))
                    .with("brightness", Value::Float(f64::from(r.brightness))),
            );
        }
        self.net.nodes.extend(net.nodes);
        self.net.links.extend(net.links);
        Ok(net.mask)
    }

    /// A flexi group: its visible members folded with its marker's
    /// operator, in chained `mask_combine`s of up to 8 ports (each carrying
    /// what came before in its first port, exact for every operator), then
    /// its refinement, inversion and opacity. `None` when it contributes
    /// nothing.
    fn flexi_group(&mut self, group: &Group, depth: usize) -> Result<Option<String>, Problem> {
        let marker = group.marker.as_ref().ok_or(Problem::Lost {
            what: "a classic group in a flexi mask".into(),
        })?;
        if marker.bypass {
            return Ok(None);
        }
        let id = group.form;
        let mut ports = Vec::new();
        for (k, m) in group.members.iter().enumerate() {
            if m.hidden {
                continue;
            }
            let Some(from) = self.form(m.form, depth + 1)? else {
                continue;
            };
            let from = self.refine(from, &m.refinement, 1, &format!("{id}.{k}"))?;
            ports.push(Port {
                from,
                invert: m.invert,
                opacity: m.opacity,
            });
        }
        if ports.is_empty() {
            return Ok(None);
        }
        let mut part = 0;
        let mut last: Option<String> = None;
        let mut rest = ports.into_iter().peekable();
        while rest.peek().is_some() {
            let mut chunk: Vec<Port> = last
                .take()
                .map(|from| Port {
                    from,
                    invert: false,
                    opacity: 1.0,
                })
                .into_iter()
                .collect();
            while chunk.len() < MAX_MEMBERS {
                match rest.next() {
                    Some(p) => chunk.push(p),
                    None => break,
                }
            }
            let name = self.net.add(
                format!("{} group {id}.{part}", self.prefix),
                "mask_combine",
                Params::new()
                    .with("operator", Value::Choice(marker.operator.into()))
                    .with(
                        "opacity",
                        Value::List(chunk.iter().map(|p| p.opacity).collect()),
                    )
                    .with(
                        "invert",
                        Value::Ints(chunk.iter().map(|p| i64::from(p.invert)).collect()),
                    ),
            );
            for (i, p) in chunk.iter().enumerate() {
                self.net.link(&p.from, "mask", &name, &format!("in{i}"));
            }
            part += 1;
            last = Some(name);
        }
        let folded = last.expect("a member");
        let refined = self.refine(folded.clone(), &marker.refinement, 2, &id.to_string())?;
        if marker.invert || marker.opacity != 1.0 {
            if refined == folded {
                // No refinement: the fold's last node inverts and scales.
                let node = self
                    .net
                    .nodes
                    .iter_mut()
                    .find(|n| n.name == folded)
                    .expect("the fold's node");
                node.params.set("group_invert", Value::Bool(marker.invert));
                node.params.set(
                    "group_opacity",
                    Value::Float(marker.opacity.clamp(0.0, 1.0)),
                );
            } else {
                let mut net = MaskNet {
                    mask: refined.clone(),
                    ..MaskNet::default()
                };
                net.then(
                    format!("{} group {id} close", self.prefix),
                    "mask_levels",
                    Params::new()
                        .with("invert", Value::Bool(marker.invert))
                        .with("opacity", Value::Float(marker.opacity.clamp(0.0, 1.0))),
                );
                self.net.nodes.extend(net.nodes);
                self.net.links.extend(net.links);
                return Ok(Some(net.mask));
            }
        }
        Ok(Some(refined))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(fields: &[[u8; 4]]) -> String {
        use std::fmt::Write as _;
        fields.iter().flatten().fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
    }

    fn f(v: f32) -> [u8; 4] {
        v.to_le_bytes()
    }

    fn i(v: i32) -> [u8; 4] {
        v.to_le_bytes()
    }

    fn li(num: u32, id: i64, kind: u32, nb: u32, points: &str) -> String {
        format!(
            r#"<rdf:li darktable:mask_num="{num}" darktable:mask_id="{id}" darktable:mask_type="{kind}" darktable:mask_name="form {id}" darktable:mask_version="6" darktable:mask_points="{points}" darktable:mask_nb="{nb}" darktable:mask_src="0000000000000000"/>"#
        )
    }

    fn sidecar(items: &[String]) -> String {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:darktable="http://darktable.sf.net/" darktable:history_end="3"><darktable:masks_history><rdf:Seq>{}</rdf:Seq></darktable:masks_history></rdf:Description></rdf:RDF></x:xmpmeta>"#,
            items.concat()
        )
    }

    fn shape(m: Mapped) -> Member {
        match m {
            Mapped::Shape(s) => s,
            other => panic!("not a shape: {other:?}"),
        }
    }

    fn get(m: &Member, name: &str) -> f64 {
        m.params.get(name).and_then(Value::as_f64).unwrap()
    }

    const FRAME: Frame = Frame {
        width: 6000.0,
        height: 4000.0,
        x0: 10.0,
        y0: 20.0,
    };

    #[test]
    fn a_circle_and_an_ellipse_land_in_helios_pixels() {
        let xml = sidecar(&[
            li(
                1,
                7,
                kind::CIRCLE,
                1,
                &hex(&[f(0.5), f(0.25), f(0.1), f(0.05)]),
            ),
            li(
                1,
                8,
                kind::ELLIPSE,
                1,
                &hex(&[f(0.2), f(0.8), f(0.05), f(0.02), f(30.0), f(0.5), i(1)]),
            ),
        ]);
        let forms = read(&xml).unwrap();
        assert_eq!(forms.len(), 2);
        let c = shape(map(&forms[0], &FRAME));
        assert_eq!(c.module, "mask_circle");
        assert!((get(&c, "x") - 2990.0).abs() < 1e-3);
        assert!((get(&c, "y") - 980.0).abs() < 1e-3);
        assert!(
            (get(&c, "radius") - 400.0).abs() < 1e-2,
            "by the smaller side"
        );
        assert!((get(&c, "feather") - 200.0).abs() < 1e-2);
        assert!(c.losses.is_empty());

        let e = shape(map(&forms[1], &FRAME));
        assert_eq!(e.module, "mask_ellipse");
        assert!((get(&e, "radius_x") - 200.0).abs() < 1e-2);
        assert!((get(&e, "radius_y") - 80.0).abs() < 1e-2);
        assert!((get(&e, "rotation") - 30.0).abs() < 1e-6);
        // Proportional: half of the longer half-axis.
        assert!((get(&e, "feather") - 100.0).abs() < 1e-2);
        assert_eq!(e.params.get("proportional"), Some(&Value::Bool(true)));
    }

    #[test]
    fn a_gradient_keeps_its_transition_and_reports_its_curvature() {
        let points = hex(&[f(0.5), f(0.5), f(-20.0), f(0.1), f(0.5), f(0.3), i(2)]);
        let forms = read(&sidecar(&[li(2, 9, kind::GRADIENT, 1, &points)])).unwrap();
        let g = shape(map(&forms[0], &Frame::new(3000.0, 4000.0)));
        assert_eq!(g.module, "mask_gradient");
        assert!((get(&g, "feather") - 1000.0).abs() < 1e-2, "2 · 0.1 · 5000");
        assert_eq!(
            g.params.get("profile"),
            Some(&Value::Choice("sigmoid".into()))
        );
        assert_eq!(g.losses.len(), 1, "{:?}", g.losses);
    }

    #[test]
    fn a_path_keeps_its_handles_and_means_its_feathers() {
        let point = |x: f32, y: f32, border: f32| {
            vec![
                f(x),
                f(y),
                f(x - 0.01),
                f(y),
                f(x + 0.01),
                f(y),
                f(border),
                f(border),
                i(0),
            ]
        };
        let mut fields = point(0.1, 0.1, 0.01);
        fields.extend(point(0.5, 0.1, 0.01));
        fields.extend(point(0.3, 0.6, 0.03));
        let forms = read(&sidecar(&[li(1, 3, kind::PATH, 3, &hex(&fields))])).unwrap();
        let p = shape(map(&forms[0], &Frame::new(1000.0, 500.0)));
        assert_eq!(p.module, "mask_path");
        let Some(Value::List(v)) = p.params.get("points") else {
            panic!("points")
        };
        assert_eq!(v.len(), 18);
        assert!(
            (v[0] - 100.0).abs() < 1e-3
                && (v[2] - 90.0).abs() < 1e-3
                && (v[4] - 110.0).abs() < 1e-3
        );
        assert!((get(&p, "feather") - 500.0 * 0.05 / 3.0).abs() < 1e-3);
        assert_eq!(p.losses.len(), 1, "feathers differ: {:?}", p.losses);
    }

    #[test]
    fn a_brush_keeps_its_points_sizes_hardness_and_density() {
        let point = |x: f32, border: f32, density: f32, hardness: f32| {
            vec![
                f(x),
                f(0.5),
                f(x - 0.01),
                f(0.5),
                f(x + 0.01),
                f(0.5),
                f(border),
                f(border),
                f(density),
                f(hardness),
                i(0),
            ]
        };
        let mut fields = point(0.1, 0.02, 1.0, 0.5);
        fields.extend(point(0.4, 0.04, 0.6, 0.2));
        let forms = read(&sidecar(&[li(1, 4, kind::BRUSH, 2, &hex(&fields))])).unwrap();
        let b = shape(map(&forms[0], &Frame::new(1000.0, 500.0)));
        assert_eq!(b.module, "mask_brush");
        assert!(b.losses.is_empty());
        let Some(Value::List(v)) = b.params.get("points") else {
            panic!("points")
        };
        assert_eq!(v.len(), 18);
        // corner, handles, radius (by the smaller side), hardness, density.
        assert!((v[0] - 100.0).abs() < 1e-3 && (v[1] - 250.0).abs() < 1e-3);
        assert!((v[6] - 10.0).abs() < 1e-3);
        assert!((v[7] - 0.5).abs() < 1e-6 && (v[8] - 1.0).abs() < 1e-6);
        assert!((v[15] - 20.0).abs() < 1e-3);
        assert!((v[16] - 0.2).abs() < 1e-6 && (v[17] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn groups_give_their_members_folds_and_unknown_forms_are_skipped() {
        let members = hex(&[
            i(7),
            i(1),
            i(state::UNION),
            f(1.0),
            i(8),
            i(1),
            i(state::DIFFERENCE | state::INVERSE),
            f(0.5),
        ]);
        let forms = read(&sidecar(&[
            li(1, 1, kind::GROUP, 2, &members),
            li(1, 5, kind::CIRCLE | kind::CLONE, 1, &hex(&[f(0.0); 4])),
            li(1, 6, 1 << 12, 1, &hex(&[f(0.0); 4])),
        ]))
        .unwrap();
        let Mapped::Group(g) = map(&forms[0], &FRAME) else {
            panic!("a group")
        };
        assert_eq!(g.members.len(), 2);
        assert_eq!(g.members[0].fold, Fold::Union);
        assert_eq!(g.members[1].fold, Fold::Difference);
        assert!(g.members[1].invert && (g.members[1].opacity - 0.5).abs() < 1e-6);
        for (form, key) in [
            (&forms[1], "xmp-mask-unsupported"),
            (&forms[2], "xmp-mask-unknown-type"),
        ] {
            let Mapped::Skipped { problem, .. } = map(form, &FRAME) else {
                panic!("skipped")
            };
            assert_eq!(problem.describe().key, key);
        }
    }

    #[test]
    fn the_forms_in_force_are_those_of_the_last_item_applied() {
        let circle = hex(&[f(0.5), f(0.5), f(0.1), f(0.0)]);
        let forms = read(&sidecar(&[
            li(0, 1, kind::CIRCLE, 1, &circle),
            li(2, 1, kind::CIRCLE, 1, &circle),
            li(2, 2, kind::CIRCLE, 1, &circle),
            li(5, 1, kind::CIRCLE, 1, &circle),
        ]))
        .unwrap();
        let ids = |end| -> Vec<(u32, i64)> {
            in_force(&forms, end)
                .iter()
                .map(|f| (f.num, f.id))
                .collect()
        };
        assert_eq!(ids(Some(3)), [(2, 1), (2, 2)]);
        assert_eq!(ids(None), [(5, 1)]);
        assert!(ids(Some(0)).is_empty());
    }

    #[test]
    fn a_wrong_size_or_version_is_skipped_with_why() {
        let mut form = read(&sidecar(&[li(1, 1, kind::CIRCLE, 2, "00000000")]))
            .unwrap()
            .remove(0);
        let Mapped::Skipped { problem, .. } = map(&form, &FRAME) else {
            panic!("skipped")
        };
        assert!(matches!(problem, Problem::Size { .. }), "{problem:?}");
        form.version = 8;
        let Mapped::Skipped { problem, .. } = map(&form, &FRAME) else {
            panic!("skipped")
        };
        assert_eq!(problem, Problem::Version { version: 8 });
        assert_eq!(problem.describe().key, "xmp-mask-version");
    }

    fn circle(id: i64) -> String {
        li(
            1,
            id,
            kind::CIRCLE,
            1,
            &hex(&[f(0.5), f(0.5), f(0.1), f(0.01)]),
        )
    }

    fn group_li(id: i64, members: &[(i64, i32, f32)]) -> String {
        let fields: Vec<[u8; 4]> = members
            .iter()
            .flat_map(|(form, s, o)| [i(*form as i32), i(id as i32), i(*s), f(*o)])
            .collect();
        li(1, id, kind::GROUP, members.len() as u32, &hex(&fields))
    }

    fn net(items: &[String], root: i64) -> Result<MaskNet, Problem> {
        let forms = read(&sidecar(items)).unwrap();
        let refs: Vec<&Form> = forms.iter().collect();
        network(&refs, root, &FRAME, "m")
    }

    fn node<'a>(n: &'a MaskNet, name: &str) -> &'a MaskNode {
        n.nodes.iter().find(|m| m.name == name).unwrap()
    }

    fn inputs(n: &MaskNet, to: &str) -> Vec<(String, String)> {
        n.links
            .iter()
            .filter(|l| l.to == to)
            .map(|l| (l.input.clone(), l.from.clone()))
            .collect()
    }

    #[test]
    fn a_group_of_one_operator_is_one_combine() {
        let n = net(
            &[
                circle(1),
                circle(2),
                group_li(
                    9,
                    &[
                        (1, state::UNION, 1.0),
                        (2, state::UNION | state::INVERSE, 0.5),
                    ],
                ),
            ],
            9,
        )
        .unwrap();
        assert_eq!(n.mask, "m group 9.0");
        let g = node(&n, "m group 9.0");
        assert_eq!(
            g.params.get("operator"),
            Some(&Value::Choice("maximum".into()))
        );
        assert_eq!(g.params.get("invert"), Some(&Value::Ints(vec![0, 1])));
        assert_eq!(g.params.get("opacity"), Some(&Value::List(vec![1.0, 0.5])));
        assert_eq!(
            inputs(&n, "m group 9.0"),
            [
                ("in0".into(), "m shape 1".into()),
                ("in1".into(), "m shape 2".into())
            ]
        );
    }

    #[test]
    fn operators_changing_along_a_group_chain_its_nodes() {
        let n = net(
            &[
                circle(1),
                circle(2),
                circle(3),
                group_li(
                    9,
                    &[
                        (1, state::UNION, 1.0),
                        (2, state::UNION, 1.0),
                        (3, state::DIFFERENCE, 0.7),
                    ],
                ),
            ],
            9,
        )
        .unwrap();
        assert_eq!(n.mask, "m group 9.1");
        let d = node(&n, "m group 9.1");
        assert_eq!(
            d.params.get("operator"),
            Some(&Value::Choice("difference".into()))
        );
        assert_eq!(
            d.params.get("opacity"),
            Some(&Value::List(vec![1.0, f64::from(0.7f32)]))
        );
        assert_eq!(
            inputs(&n, "m group 9.1"),
            [
                ("in0".into(), "m group 9.0".into()),
                ("in1".into(), "m shape 3".into())
            ]
        );
    }

    #[test]
    fn a_first_intersection_starts_from_zero_and_nesting_is_followed() {
        let n = net(
            &[
                circle(1),
                circle(2),
                group_li(8, &[(1, state::UNION, 1.0)]),
                group_li(9, &[(2, state::INTERSECTION, 1.0), (8, state::SUM, 0.4)]),
            ],
            9,
        )
        .unwrap();
        // 9.0: the constant 0; 9.1: min(0, shape 2); 9.2: sum with group 8.
        assert_eq!(
            node(&n, "m group 9.0").params.get("group_opacity"),
            Some(&Value::Float(0.0))
        );
        assert_eq!(
            inputs(&n, "m group 9.1"),
            [
                ("in0".into(), "m group 9.0".into()),
                ("in1".into(), "m shape 2".into())
            ]
        );
        assert_eq!(
            inputs(&n, "m group 9.2"),
            [
                ("in0".into(), "m group 9.1".into()),
                ("in1".into(), "m group 8.0".into())
            ]
        );
        assert_eq!(n.mask, "m group 9.2");
    }

    #[test]
    fn a_mask_that_would_differ_is_not_built() {
        let gradient = li(
            1,
            3,
            kind::GRADIENT,
            1,
            &hex(&[f(0.5), f(0.5), f(0.0), f(0.1), f(0.5), f(0.3), i(1)]),
        );
        let problem = net(&[gradient, group_li(9, &[(3, state::UNION, 1.0)])], 9).unwrap_err();
        assert_eq!(problem.describe().key, "xmp-mask-lost");
        assert_eq!(
            net(&[group_li(9, &[(4, state::UNION, 1.0)])], 9).unwrap_err(),
            Problem::Missing { id: 4 }
        );
        assert_eq!(
            net(&[group_li(9, &[(9, state::UNION, 1.0)])], 9).unwrap_err(),
            Problem::TooDeep
        );
    }

    /// A masks version 7 group point (240 bytes).
    fn v7_point(
        formid: i32,
        parent: i32,
        st: i32,
        opacity: f32,
        scope: i32,
        blur: f32,
        group_opacity: f32,
    ) -> Vec<[u8; 4]> {
        let mut out = vec![i(formid), i(parent), i(st), f(opacity)];
        out.extend([i(scope), f(0.0), f(0.0), i(5), f(blur), f(0.0), f(0.0)]);
        out.extend(std::iter::repeat_n([0u8; 4], 32));
        out.push(f(group_opacity));
        out.extend(std::iter::repeat_n([0u8; 4], 16));
        assert_eq!(out.len() * 4, 240);
        out
    }

    /// A flexi group: its marker (operator, invert, bypass bits in
    /// `marker`), then `(form, state, opacity, blur)` members.
    fn flexi_li(
        id: i64,
        marker: i32,
        group_opacity: f32,
        members: &[(i64, i32, f32, f32)],
    ) -> String {
        let id32 = id as i32;
        let mut fields = v7_point(
            id32 + 500,
            id32,
            state::GROUP_MARKER | marker,
            1.0,
            0,
            0.0,
            group_opacity,
        );
        for (form, st, o, blur) in members {
            let scope = i32::from(*blur > 0.0);
            fields.extend(v7_point(*form as i32, id32, *st, *o, scope, *blur, 1.0));
        }
        li(1, id, kind::GROUP, members.len() as u32 + 1, &hex(&fields)).replace(
            r#"darktable:mask_version="6""#,
            r#"darktable:mask_version="7""#,
        )
    }

    fn flexi_net(items: &[String], root: i64) -> Result<MaskNet, Problem> {
        let forms = read(&sidecar(items)).unwrap();
        let refs: Vec<&Form> = forms.iter().collect();
        network_flexi(&refs, root, &FRAME, "m")
    }

    #[test]
    fn a_version_7_group_reads_its_marker_and_members() {
        let forms = read(&sidecar(&[flexi_li(
            9,
            state::FLEXI_SCREEN | state::OP_INVERT,
            0.8,
            &[
                (1, 0, 1.0, 0.0),
                (2, state::INVERSE | state::HIDDEN, 0.5, 2.0),
            ],
        )]))
        .unwrap();
        let Mapped::Group(g) = map(&forms[0], &FRAME) else {
            panic!("a group")
        };
        let marker = g.marker.unwrap();
        assert_eq!(marker.operator, "screen");
        assert!(marker.invert && !marker.bypass);
        assert!((marker.opacity - 0.8).abs() < 1e-6);
        assert_eq!(g.members.len(), 2);
        assert!(g.members[1].hidden && g.members[1].invert);
        assert_eq!(g.members[1].refinement.scope, 1);
        assert!((g.members[1].refinement.blur_radius - 2.0).abs() < 1e-6);
    }

    #[test]
    fn a_flexi_tree_folds_visible_members_and_skips_empty_groups() {
        let n = flexi_net(
            &[
                circle(1),
                circle(2),
                circle(3),
                // A nested group whose only member is hidden: left out.
                flexi_li(8, 0, 1.0, &[(3, state::HIDDEN, 1.0, 0.0)]),
                flexi_li(
                    9,
                    state::FLEXI_MINIMUM | state::OP_INVERT,
                    0.6,
                    &[
                        (1, 0, 1.0, 2.0),
                        (8, 0, 1.0, 0.0),
                        (2, state::INVERSE, 0.5, 0.0),
                    ],
                ),
            ],
            9,
        )
        .unwrap();
        assert_eq!(n.mask, "m group 9.0");
        let g = node(&n, "m group 9.0");
        assert_eq!(
            g.params.get("operator"),
            Some(&Value::Choice("minimum".into()))
        );
        assert_eq!(g.params.get("group_invert"), Some(&Value::Bool(true)));
        assert_eq!(
            g.params.get("group_opacity"),
            Some(&Value::Float(f64::from(0.6f32)))
        );
        assert_eq!(g.params.get("invert"), Some(&Value::Ints(vec![0, 1])));
        // The first member through its own blur.
        assert_eq!(
            inputs(&n, "m group 9.0"),
            [
                ("in0".into(), "m refine 9.0 blur".into()),
                ("in1".into(), "m shape 2".into())
            ]
        );
        assert_eq!(
            inputs(&n, "m refine 9.0 blur"),
            [("mask".into(), "m shape 1".into())]
        );
        assert!(n.nodes.iter().all(|m| !m.name.contains("shape 3")));
    }

    #[test]
    fn a_bypassed_flexi_root_is_one_everywhere_and_a_classic_group_is_refused() {
        let n = flexi_net(
            &[
                circle(1),
                flexi_li(9, state::OP_BYPASS, 1.0, &[(1, 0, 1.0, 0.0)]),
            ],
            9,
        )
        .unwrap();
        assert_eq!(n.nodes.len(), 1);
        assert_eq!(n.nodes[0].module, "mask_combine");
        assert!(n.links.is_empty());
        let classic = flexi_net(&[circle(1), group_li(9, &[(1, state::UNION, 1.0)])], 9);
        assert!(matches!(classic, Err(Problem::Lost { .. })), "{classic:?}");
    }

    #[test]
    fn a_later_masks_version_is_refused() {
        let mut form = read(&sidecar(&[circle(1)])).unwrap().remove(0);
        form.version = 8;
        let Mapped::Skipped { problem, .. } = map(&form, &FRAME) else {
            panic!("skipped")
        };
        assert_eq!(problem, Problem::Version { version: 8 });
    }
}
