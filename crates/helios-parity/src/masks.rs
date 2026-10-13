//! Drawn-mask parity: hand-made sidecars where an exposure of +1 EV is
//! masked by one drawn shape or a group of them, on raws of the corpus
//! (`docs/raw-corpus.md`), rendered by darktable-cli and by Helios (see
//! `docs/masks.md`, "Parity with darktable", and the `mask-parity`
//! binary).
//!
//! The sidecars are generated here and kept in `tests/masks/` (the test
//! `mask_fixtures_are_up_to_date` checks they match; `HELIOS_BLESS=1`
//! writes them). Each holds darktable's classic masks as darktable writes
//! them: version 6 forms in `darktable:masks_history`, blend params
//! version 14 on the exposure.

use std::fmt::Write as _;

use helios_xmp::Layouts;
use helios_xmp::blend::{self, Blend};
use helios_xmp::drawn::{kind, state};
use serde_json::json;

use crate::{Input, Item, sidecar};

/// The exposure the masks hold back: +1 EV doubles the image where the
/// mask is 1, so a mask reads back as `masked / unmasked - 1`.
pub const EXPOSURE: f64 = 1.0;

/// darktable's masks version the fixtures are written in.
pub const MASKS_VERSION: u32 = 6;

/// A form of the masks history: its id, type bits, name and points.
#[derive(Clone, Debug)]
pub struct Form {
    pub id: i32,
    pub kind: u32,
    pub name: &'static str,
    /// How many points `fields` holds.
    pub count: u32,
    pub fields: Vec<[u8; 4]>,
}

fn f(v: f32) -> [u8; 4] {
    v.to_le_bytes()
}

fn i(v: i32) -> [u8; 4] {
    v.to_le_bytes()
}

/// A member's state in a group, as darktable's GUI sets it: in use, shown,
/// and its operator.
const USE: i32 = 1 | 2;

/// A circle: centre, radius and feather, normalised (radii by the smaller
/// side).
pub fn circle(id: i32, x: f32, y: f32, radius: f32, feather: f32) -> Form {
    Form {
        id,
        kind: kind::CIRCLE,
        name: "circle",
        count: 1,
        fields: vec![f(x), f(y), f(radius), f(feather)],
    }
}

/// An ellipse with an equidistant feather.
pub fn ellipse(id: i32, x: f32, y: f32, a: f32, b: f32, rotation: f32, feather: f32) -> Form {
    Form {
        id,
        kind: kind::ELLIPSE,
        name: "ellipse",
        count: 1,
        fields: vec![f(x), f(y), f(a), f(b), f(rotation), f(feather), i(0)],
    }
}

/// A straight sigmoid gradient: anchor, rotation and compression.
pub fn gradient(id: i32, x: f32, y: f32, rotation: f32, compression: f32) -> Form {
    Form {
        id,
        kind: kind::GRADIENT,
        name: "gradient",
        count: 1,
        // anchor, rotation, compression, steepness, curvature, state (sigmoid).
        fields: vec![
            f(x),
            f(y),
            f(rotation),
            f(compression),
            f(0.0),
            f(0.0),
            i(2),
        ],
    }
}

/// Bézier handles through `corners` (a closed curve when `closed`), as
/// darktable's Catmull-Rom initialisation places them, a third of the way
/// to the neighbours' tangent. Returns `(before, after)` per corner.
fn handles(corners: &[[f32; 2]], closed: bool) -> Vec<([f32; 2], [f32; 2])> {
    let n = corners.len();
    (0..n)
        .map(|k| {
            let prev = if k > 0 {
                corners[k - 1]
            } else if closed {
                corners[n - 1]
            } else {
                corners[k]
            };
            let next = if k + 1 < n {
                corners[k + 1]
            } else if closed {
                corners[0]
            } else {
                corners[k]
            };
            let c = corners[k];
            let t = [(next[0] - prev[0]) / 6.0, (next[1] - prev[1]) / 6.0];
            ([c[0] - t[0], c[1] - t[1]], [c[0] + t[0], c[1] + t[1]])
        })
        .collect()
}

/// A closed smooth path through `corners`, one feather for all.
pub fn path(id: i32, corners: &[[f32; 2]], feather: f32) -> Form {
    let mut fields = Vec::new();
    for (c, (before, after)) in corners.iter().zip(handles(corners, true)) {
        fields.extend([
            f(c[0]),
            f(c[1]),
            f(before[0]),
            f(before[1]),
            f(after[0]),
            f(after[1]),
            f(feather),
            f(feather),
            // DT_MASKS_POINT_STATE_USER: handles placed, not recomputed.
            i(2),
        ]);
    }
    Form {
        id,
        kind: kind::PATH,
        name: "path",
        count: corners.len() as u32,
        fields,
    }
}

/// A brush stroke through `points` (corner, size), one hardness and
/// density for all.
pub fn brush(id: i32, points: &[([f32; 2], f32)], hardness: f32, density: f32) -> Form {
    let corners: Vec<[f32; 2]> = points.iter().map(|p| p.0).collect();
    let mut fields = Vec::new();
    for ((c, size), (before, after)) in points.iter().zip(handles(&corners, false)) {
        fields.extend([
            f(c[0]),
            f(c[1]),
            f(before[0]),
            f(before[1]),
            f(after[0]),
            f(after[1]),
            f(*size),
            f(*size),
            f(density),
            f(hardness),
            i(2),
        ]);
    }
    Form {
        id,
        kind: kind::BRUSH,
        name: "brush",
        count: points.len() as u32,
        fields,
    }
}

/// A member of a group: the form, its operator bit, inversion, opacity.
pub struct Member {
    pub form: i32,
    pub op: i32,
    pub invert: bool,
    pub opacity: f32,
}

/// A member at full opacity.
pub fn member(form: i32, op: i32) -> Member {
    Member {
        form,
        op,
        invert: false,
        opacity: 1.0,
    }
}

/// A group of members.
pub fn group(id: i32, members: &[Member]) -> Form {
    let mut fields = Vec::new();
    for m in members {
        let inverse = if m.invert { state::INVERSE } else { 0 };
        fields.extend([i(m.form), i(id), i(USE | m.op | inverse), f(m.opacity)]);
    }
    Form {
        id,
        kind: kind::GROUP,
        name: "group",
        count: members.len() as u32,
        fields,
    }
}

/// One parity case: a raw of the corpus and an exposure masked by `forms`'
/// group `mask`, blending with `blend`.
pub struct Fixture {
    pub name: &'static str,
    /// The raw's file name in the corpus (`<id>.<ext>`).
    pub raw: &'static str,
    pub forms: Vec<Form>,
    pub blend: Blend,
}

/// The raws the fixtures use: a crop that starts away from the sensor's
/// corner (52, 19), and a 6-megapixel one.
pub const CANON: &str = "canon-eos-350d-cr2-raw-3-2.CR2";
pub const NIKON: &str = "nikon-d70-nef-12bit-12bit-compressed-lossy-type-1-3-2.NEF";

/// The module's group id.
const ROOT: i32 = 1000;

fn single(name: &'static str, raw: &'static str, form: Form) -> Fixture {
    let id = form.id;
    Fixture {
        name,
        raw,
        forms: vec![form, group(ROOT, &[member(id, state::UNION)])],
        blend: Blend::drawn(ROOT),
    }
}

/// The fixtures: one per shape kind, two groups.
pub fn fixtures() -> Vec<Fixture> {
    let mut inverted_path = single(
        "path",
        NIKON,
        path(
            11,
            &[
                [0.30, 0.25],
                [0.70, 0.30],
                [0.75, 0.70],
                [0.45, 0.80],
                [0.25, 0.55],
            ],
            0.03,
        ),
    );
    // The module's own inversion: exposure everywhere but in the path.
    inverted_path.blend.mask_combine |= blend::combine::MASKS_POS;
    let mut mixed = Fixture {
        name: "group-mixed",
        raw: NIKON,
        forms: vec![
            ellipse(21, 0.5, 0.5, 0.3, 0.2, -15.0, 0.04),
            circle(22, 0.62, 0.5, 0.2, 0.05),
            circle(23, 0.6, 0.45, 0.06, 0.02),
            group(
                ROOT,
                &[
                    member(21, state::UNION),
                    member(22, state::INTERSECTION),
                    Member {
                        opacity: 0.8,
                        ..member(23, state::DIFFERENCE)
                    },
                ],
            ),
        ],
        blend: Blend::drawn(ROOT),
    };
    // The module's opacity and blur.
    mixed.blend.opacity = 80.0;
    mixed.blend.blur_radius = 4.0;
    vec![
        single("circle", CANON, circle(1, 0.4, 0.45, 0.15, 0.05)),
        single("ellipse", NIKON, ellipse(2, 0.6, 0.5, 0.2, 0.1, 30.0, 0.05)),
        single("gradient", CANON, gradient(3, 0.5, 0.5, 20.0, 0.1)),
        inverted_path,
        single(
            "brush",
            CANON,
            brush(
                12,
                &[
                    ([0.20, 0.30], 0.03),
                    ([0.40, 0.45], 0.04),
                    ([0.60, 0.40], 0.05),
                    ([0.75, 0.65], 0.04),
                ],
                0.5,
                1.0,
            ),
        ),
        Fixture {
            name: "group-union",
            raw: CANON,
            forms: vec![
                circle(13, 0.3, 0.5, 0.12, 0.04),
                ellipse(14, 0.65, 0.45, 0.18, 0.1, 60.0, 0.03),
                group(
                    ROOT,
                    &[
                        member(13, state::UNION),
                        Member {
                            invert: true,
                            opacity: 0.6,
                            ..member(14, state::UNION)
                        },
                    ],
                ),
            ],
            blend: Blend::drawn(ROOT),
        },
        mixed,
    ]
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The history the fixtures share before their exposure: highlights
/// clipped and RCD (as Helios's chain), no rotation, through the camera's
/// matrix into linear Rec.2020 and out of it unchanged (see [`sidecar`]).
fn items(exposure: bool) -> Vec<Item> {
    let mut items = vec![
        Item::new(
            "highlights",
            json!({"mode": "DT_IOP_HIGHLIGHTS_CLIP", "clip": 1.0}),
        ),
        Item::new(
            "demosaic",
            json!({"demosaicing_method": "DT_IOP_DEMOSAIC_RCD"}),
        ),
        Item::new("flip", json!({"orientation": "ORIENTATION_NONE"})),
    ];
    if exposure {
        // Without darktable's highlight compensation, which Helios's exposure
        // does not do.
        items.push(Item::new(
            "exposure",
            json!({ "exposure": EXPOSURE, "compensate_hilite_pres": false }),
        ));
    }
    items
}

impl Fixture {
    /// The fixture's sidecar: the exposure blends with its masks.
    pub fn sidecar(&self, layouts: &Layouts) -> Result<String, String> {
        let xml = sidecar(&items(true), Input::Camera, layouts)?;
        // The exposure's history item: its number is where its masks live.
        let at = xml
            .find(r#"darktable:operation="exposure""#)
            .ok_or("no exposure item")?;
        let start = xml[..at].rfind("<rdf:li").ok_or("no exposure item")?;
        let num: u32 = xml[start..]
            .split('"')
            .nth(1)
            .and_then(|n| n.parse().ok())
            .ok_or("no exposure number")?;
        let end = start + xml[start..].find("/>").ok_or("no item end")?;
        let blend = format!(
            r#" darktable:blendop_version="14" darktable:blendop_params="{}""#,
            hex(&self.blend.write())
        );
        let mut forms = String::new();
        for form in &self.forms {
            let points: Vec<u8> = form.fields.iter().flatten().copied().collect();
            let _ = write!(
                forms,
                "\n     <rdf:li darktable:mask_num=\"{num}\" darktable:mask_id=\"{}\" darktable:mask_type=\"{}\" darktable:mask_name=\"{} {}\" darktable:mask_version=\"{MASKS_VERSION}\" darktable:mask_points=\"{}\" darktable:mask_nb=\"{}\" darktable:mask_src=\"0000000000000000\"/>",
                form.id,
                form.kind,
                form.name,
                form.id,
                hex(&points),
                form.count,
            );
        }
        let masks = format!(
            "<darktable:masks_history>\n    <rdf:Seq>{forms}\n    </rdf:Seq>\n   </darktable:masks_history>\n   <darktable:history>"
        );
        Ok(format!("{}{blend}{}", &xml[..end], &xml[end..]).replacen(
            "<darktable:history>",
            &masks,
            1,
        ))
    }
}

/// The same history without the exposure: what the mask reads back
/// against.
pub fn unmasked_sidecar(layouts: &Layouts) -> Result<String, String> {
    sidecar(&items(false), Input::Camera, layouts)
}
