//! A brush stroke: darktable's brush (`src/develop/masks/brush.c`), an
//! open curve painted with a round brush whose radius, hardness and density
//! change from point to point.
//!
//! The stroke runs through its points along cubic Bézier segments (from a
//! point's corner, by the handle after it and the next point's handle
//! before it, to the next corner). Along a segment the radius follows a
//! smoothstep from one point's to the next, the hardness and the density
//! change linearly, as darktable interpolates them. A pixel at distance
//! `d` from the stroke, where its radius is `r`, its hardness `h` and its
//! density `ρ`, takes `ρ` up to `h · r`, then falls linearly to 0 at `r`:
//! darktable draws the same profile along rays from the stroke to its
//! border. Both ends are round, as darktable closes a stroke with half
//! circles. Where the stroke crosses itself, the larger value wins.
//!
//! The stroke is cut into straight pieces within 0.1 image pixel of the
//! curve, as a path is, and finer where the radius changes fast. The
//! value is anti-aliased like every shape's (the mean of the profile over
//! a pixel's width across the edge). CPU only: a stroke can hold many more
//! points than a GPU uniform buffer.

use helios_core::{
    Context, Extent, Image, Manifest, Module, ParamHint, ParamKind, ParamSpec, Params, Region,
    Value,
};

use super::{Bounds, Common, FAR, FLATNESS, MAX_PIECES, Profile, hints, shape_manifest};

/// The most points a stroke holds.
pub const MAX_BRUSH_POINTS: usize = 256;

/// Values a stroke point holds: its corner, the handle before it, the
/// handle after it (`x`, `y` each), its radius, hardness and density.
pub const BRUSH_POINT_VALUES: usize = 9;

/// One point of a stroke.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushPoint {
    pub corner: [f64; 2],
    pub before: [f64; 2],
    pub after: [f64; 2],
    /// The brush's radius there, image pixels.
    pub radius: f64,
    /// The share of the radius at full density, 0 to 1.
    pub hardness: f64,
    /// The value at the stroke's centre, 0 to 1.
    pub density: f64,
}

/// A stroke's points as the `points` parameter holds them.
pub fn brush_values(points: &[BrushPoint]) -> Value {
    Value::List(
        points
            .iter()
            .flat_map(|p| {
                [
                    p.corner[0],
                    p.corner[1],
                    p.before[0],
                    p.before[1],
                    p.after[0],
                    p.after[1],
                    p.radius,
                    p.hardness,
                    p.density,
                ]
            })
            .collect(),
    )
}

/// The `points` parameter read back.
pub fn brush_points(values: &[f64]) -> Result<Vec<BrushPoint>, String> {
    if !values.len().is_multiple_of(BRUSH_POINT_VALUES) {
        return Err(format!(
            "{} brush values, not a multiple of {BRUSH_POINT_VALUES}",
            values.len()
        ));
    }
    Ok(values
        .chunks_exact(BRUSH_POINT_VALUES)
        .map(|v| BrushPoint {
            corner: [v[0], v[1]],
            before: [v[2], v[3]],
            after: [v[4], v[5]],
            radius: v[6].max(0.0),
            hardness: v[7].clamp(0.0, 1.0),
            density: v[8].clamp(0.0, 1.0),
        })
        .collect())
}

/// A point of the stroke cut into pieces, with the brush there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Knot {
    pub at: [f64; 2],
    pub radius: f64,
    pub hardness: f64,
    pub density: f64,
}

impl Knot {
    fn lerp(a: &Knot, b: &Knot, s: f64) -> Knot {
        let mix = |u: f64, v: f64| u + (v - u) * s;
        Knot {
            at: [mix(a.at[0], b.at[0]), mix(a.at[1], b.at[1])],
            radius: mix(a.radius, b.radius),
            hardness: mix(a.hardness, b.hardness),
            density: mix(a.density, b.density),
        }
    }
}

/// The radius change a piece may hold, in image pixels: the radius is
/// linear along a piece, the smoothstep between pieces.
const RADIUS_STEP: f64 = 0.5;

/// Cuts the stroke into pieces: each Bézier segment within [`FLATNESS`] of
/// its curve and with its radius within [`RADIUS_STEP`] of the smoothstep,
/// at most [`MAX_PIECES`] a segment. One point is a dot.
pub fn stroke(points: &[BrushPoint]) -> Vec<Knot> {
    let knot = |p: &BrushPoint| Knot {
        at: p.corner,
        radius: p.radius,
        hardness: p.hardness,
        density: p.density,
    };
    let Some(first) = points.first() else {
        return Vec::new();
    };
    let mut out = vec![knot(first)];
    for pair in points.windows(2) {
        let (p, q) = (&pair[0], &pair[1]);
        let (p0, p1, p2, p3) = (p.corner, p.after, q.before, q.corner);
        let second = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
            (a[0] - 2.0 * b[0] + c[0]).hypot(a[1] - 2.0 * b[1] + c[1])
        };
        let bend = 6.0 * second(p0, p1, p2).max(second(p1, p2, p3));
        let by_bend = (bend / (8.0 * FLATNESS)).sqrt().ceil();
        let by_radius = ((q.radius - p.radius).abs() / RADIUS_STEP).ceil();
        let pieces = (by_bend.max(by_radius) as usize).clamp(1, MAX_PIECES);
        for k in 1..=pieces {
            let t = k as f64 / pieces as f64;
            let s = 1.0 - t;
            let (a, b, c, d) = (s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
            let smooth = t * t * (3.0 - 2.0 * t);
            out.push(Knot {
                at: [
                    a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
                    a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
                ],
                radius: p.radius + (q.radius - p.radius) * smooth,
                hardness: p.hardness + (q.hardness - p.hardness) * t,
                density: p.density + (q.density - p.density) * t,
            });
        }
    }
    out
}

/// A brush stroke as a mask member. See the module's documentation.
pub struct MaskBrush;

#[derive(Clone, Debug, PartialEq)]
pub struct BrushPlan {
    pub knots: Vec<Knot>,
    pub common: Common,
}

impl BrushPlan {
    /// The stroke's pieces: one for a dot.
    fn pieces(&self) -> impl Iterator<Item = (&Knot, &Knot)> + '_ {
        let dot = (self.knots.len() == 1).then(|| (&self.knots[0], &self.knots[0]));
        self.knots.windows(2).map(|w| (&w[0], &w[1])).chain(dot)
    }

    /// One piece's value at image point `p`.
    fn piece(&self, a: &Knot, b: &Knot, p: [f64; 2]) -> f64 {
        let (dx, dy) = (b.at[0] - a.at[0], b.at[1] - a.at[1]);
        let (px, py) = (p[0] - a.at[0], p[1] - a.at[1]);
        let len2 = dx * dx + dy * dy;
        let s = if len2 > 0.0 {
            ((px * dx + py * dy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let k = Knot::lerp(a, b, s);
        let d = (px - s * dx).hypot(py - s * dy);
        k.density * falloff(d, k.radius, k.hardness, self.common.h)
    }

    /// The stroke's value at image point `(x, y)`: every piece is read.
    pub fn value(&self, x: f64, y: f64) -> f64 {
        self.pieces()
            .map(|(a, b)| self.piece(a, b, [x, y]))
            .fold(0.0, f64::max)
    }

    pub fn bounds(&self) -> Bounds {
        if self.knots.is_empty() {
            return Bounds::Empty;
        }
        let mut b = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for k in &self.knots {
            b = [
                b[0].min(k.at[0] - k.radius),
                b[1].min(k.at[1] - k.radius),
                b[2].max(k.at[0] + k.radius),
                b[3].max(k.at[1] + k.radius),
            ];
        }
        Bounds::Box(b)
    }

    /// One row of `output`: only the pieces whose reach meets the row are
    /// read, and of those only the ones whose reach meets the pixel. Every
    /// value equals [`BrushPlan::value`]'s.
    fn row(&self, output: Region, y: i64, out: &mut Vec<f32>) {
        let member = self.common.member;
        let (_, yc) = self.common.centre(0, y);
        let h = self.common.h;
        let near: Vec<(&Knot, &Knot, f64)> = self
            .pieces()
            .filter_map(|(a, b)| {
                let reach = a.radius.max(b.radius) + h;
                (a.at[1].min(b.at[1]) - reach <= yc && yc <= a.at[1].max(b.at[1]) + reach)
                    .then_some((a, b, reach))
            })
            .collect();
        for x in output.x..output.right() {
            let (xc, _) = self.common.centre(x, y);
            let mut v = 0.0f64;
            for (a, b, reach) in &near {
                if a.at[0].min(b.at[0]) - reach <= xc && xc <= a.at[0].max(b.at[0]) + reach {
                    v = v.max(self.piece(a, b, [xc, yc]));
                }
            }
            out.push(member.apply(v));
        }
    }
}

/// The brush's profile at distance `d`: 1 up to `hardness · radius`, then
/// linear to 0 at `radius`, averaged over `[d - h, d + h]`.
fn falloff(d: f64, radius: f64, hardness: f64, h: f64) -> f64 {
    let solid = hardness * radius;
    if radius - solid < 1e-9 {
        // A hard brush: the coverage of its edge.
        return ((radius - (d - h)) / (2.0 * h)).clamp(0.0, 1.0);
    }
    Profile {
        lo: solid,
        hi: radius,
        below: 1.0,
        above: 0.0,
        f: |u: f64| (radius - u) / (radius - solid),
        pieces: 1,
    }
    .mean(d, h)
}

impl Module for MaskBrush {
    type Plan = BrushPlan;

    fn manifest(&self) -> Manifest {
        let line = [
            BrushPoint {
                corner: [8.0, 12.0],
                before: [8.0, 12.0],
                after: [14.0, 6.0],
                radius: 4.0,
                hardness: 0.5,
                density: 1.0,
            },
            BrushPoint {
                corner: [26.0, 12.0],
                before: [20.0, 18.0],
                after: [26.0, 12.0],
                radius: 4.0,
                hardness: 0.5,
                density: 1.0,
            },
        ];
        let Value::List(default) = brush_values(&line) else {
            unreachable!("brush_values gives a list")
        };
        shape_manifest(
            "mask_brush",
            "A brush stroke (radius, hardness and density along a curve) as a mask member, in image pixels",
            vec![ParamSpec {
                name: "points".into(),
                description: format!(
                    "up to {MAX_BRUSH_POINTS} points, {BRUSH_POINT_VALUES} values each: the \
                     corner, the handle before it, the handle after it (x, y each, image \
                     pixels), the radius (image pixels), the hardness and the density (0 to 1)"
                ),
                kind: ParamKind::Floats {
                    count: (MAX_BRUSH_POINTS * BRUSH_POINT_VALUES) as u32,
                    min: -FAR,
                    max: FAR,
                    default,
                },
            }],
        )
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        hints(&[], &[])
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<BrushPlan, String> {
        self.prepare_at(params, context, 1.0)
    }

    fn prepare_at(&self, p: &Params, _: &Context, scale: f64) -> Result<BrushPlan, String> {
        let values = p.get("points").and_then(Value::as_list).unwrap_or(&[]);
        Ok(BrushPlan {
            knots: stroke(&brush_points(values)?),
            common: Common::new(p, scale),
        })
    }

    fn extent(&self, plan: &BrushPlan) -> Option<Extent> {
        plan.common.extent(plan.bounds())
    }

    fn process(
        &self,
        plan: &BrushPlan,
        _: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = Vec::with_capacity(output.area());
        for y in output.y..output.bottom() {
            plan.row(output, y, &mut data);
        }
        Ok(vec![Image::new(output, 1, data)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f64, y: f64, radius: f64, hardness: f64, density: f64) -> BrushPoint {
        BrushPoint {
            corner: [x, y],
            before: [x, y],
            after: [x, y],
            radius,
            hardness,
            density,
        }
    }

    fn plan(points: &[BrushPoint], scale: f64) -> BrushPlan {
        let params = MaskBrush
            .manifest()
            .defaults()
            .with("points", brush_values(points));
        MaskBrush
            .prepare_at(&params, &Context::new(), scale)
            .unwrap()
    }

    #[test]
    fn a_straight_stroke_has_its_profile_across_and_round_ends() {
        let p = plan(
            &[
                point(10.0, 50.0, 10.0, 0.5, 0.8),
                point(90.0, 50.0, 10.0, 0.5, 0.8),
            ],
            4.0,
        );
        // On the stroke and within the hard part: the density.
        assert!((p.value(50.0, 50.0) - 0.8).abs() < 1e-9);
        assert!((p.value(50.0, 54.0) - 0.8).abs() < 1e-9);
        // Halfway through the falloff (5 to 10): half the density.
        assert!((p.value(50.0, 57.5) - 0.4).abs() < 1e-3);
        assert_eq!(p.value(50.0, 61.0), 0.0);
        // Round ends: the same profile around the last point.
        assert!((p.value(97.5, 50.0) - 0.4).abs() < 1e-3);
        assert!((p.value(90.0 + 7.5 * 0.6, 50.0 + 7.5 * 0.8) - 0.4).abs() < 1e-3);
    }

    #[test]
    fn radius_and_density_change_along_the_stroke() {
        let p = plan(
            &[
                point(0.0, 0.0, 4.0, 1.0, 1.0),
                point(100.0, 0.0, 20.0, 1.0, 0.5),
            ],
            1.0,
        );
        // Halfway the smoothstep is halfway: radius 12, density 0.75.
        assert!((p.value(50.0, 11.0) - 0.75).abs() < 1e-6);
        assert_eq!(p.value(50.0, 13.0), 0.0);
        // Handles on their corners: the curve's x is a smoothstep of t too,
        // so a quarter of the way along x is where the radius has made a
        // quarter of its change (8) and t is 0.3264.
        let t: f64 = 0.326_351_9;
        assert!((3.0 * t * t - 2.0 * t * t * t - 0.25).abs() < 1e-6);
        // Neighbours of a denser part reach it too (round pieces, where
        // darktable draws straight across): a little more, never less.
        let v = p.value(25.0, 7.4);
        assert!(v >= 1.0 - 0.5 * t - 1e-9 && v < 1.0 - 0.5 * t + 0.01, "{v}");
        assert_eq!(p.value(25.0, 8.6), 0.0);
    }

    #[test]
    fn a_tiled_render_equals_the_whole_one_and_tiles_away_are_filled() {
        let stroke = [
            BrushPoint {
                corner: [20.0, 20.0],
                before: [20.0, 20.0],
                after: [40.0, 0.0],
                radius: 6.0,
                hardness: 0.3,
                density: 1.0,
            },
            BrushPoint {
                corner: [60.0, 40.0],
                before: [50.0, 50.0],
                after: [70.0, 30.0],
                radius: 9.0,
                hardness: 0.6,
                density: 0.7,
            },
            point(70.0, 70.0, 3.0, 0.0, 1.0),
        ];
        for scale in [1.0, 0.37] {
            let p = plan(&stroke, scale);
            let whole = Region::sized(40, 40);
            let full = MaskBrush.process(&p, &[], whole).unwrap().remove(0);
            for tile in whole.tiles(7) {
                let part = MaskBrush.process(&p, &[], tile).unwrap().remove(0);
                for y in tile.y..tile.bottom() {
                    for x in tile.x..tile.right() {
                        assert_eq!(part.pixel(x, y)[0], full.pixel(x, y)[0]);
                    }
                }
            }
            // Rows match the slow value at every pixel.
            for y in 0..40 {
                for x in 0..40 {
                    let (u, v) = ((x as f64 + 0.5) / scale, (y as f64 + 0.5) / scale);
                    assert_eq!(full.pixel(x, y)[0], p.common.member.apply(p.value(u, v)));
                }
            }
            let extent = MaskBrush.extent(&p).unwrap();
            let far = MaskBrush
                .process(&p, &[], Region::new(500, 500, 4, 4))
                .unwrap()
                .remove(0);
            assert!(!extent.region.overlaps(&Region::new(500, 500, 4, 4)));
            assert!(far.data().iter().all(|v| *v == extent.fill));
        }
    }
}
