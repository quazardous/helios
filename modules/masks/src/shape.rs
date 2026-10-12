//! Drawn shape members: a circle, an ellipse, a linear gradient and a path
//! (a polygon or Bézier curves), darktable's drawn masks
//! (`src/develop/masks/{circle,ellipse,gradient,path}.c`).
//!
//! **Image coordinates.** A shape's parameters are full-resolution pixels
//! of the image the network reads: pixel `(i, j)` covers `[i, i + 1) x [j,
//! j + 1)`. A render at scale `s` evaluates its pixel `(i, j)` at `((i +
//! 0.5) / s, (j + 0.5) / s)`, so a shape stays where it was drawn at any
//! zoom and in any crop or tile.
//!
//! **Anti-aliasing.** Each pixel takes the mean of the shape's profile over
//! the pixel's width (`1 / s` image pixels) across the edge, in closed form
//! (Gauss-Legendre quadrature, exact for the polynomial profiles): a hard
//! edge (feather 0) gets the pixel's coverage, a soft one is barely
//! changed.
//!
//! **Members.** Each shape has a feather (its falloff width), an opacity
//! and an inversion, applied as for every member: `(invert ? 1 - v : v) *
//! opacity`. Away from its outer edge a shape gives exactly 0 (its
//! opacity, inverted), and declares it as its [`Extent`]: the engine fills
//! tiles away from it without running it.
//!
//! **GPU.** Each has a WGSL kernel computing the same profile in `f32`;
//! the CPU code computes in `f64`. They agree within
//! [`GPU_TOLERANCE`].
//!
//! **Editing.** A surface (the viewport's overlay and handles) edits a
//! shape through the value store, like any parameter: it writes
//! `node/<node>/<param>`, a drag inside one gesture so it is one undo
//! step. What it reads to draw is [`outline`]: the edge and the feather's
//! end as polylines, and the handles, each named after the parameters it
//! moves, all in image pixels (the viewport maps them to the screen).
//!
//! | module | parameters a handle writes |
//! |---|---|
//! | `mask_circle` | `x`, `y` (centre); `radius`; `feather` |
//! | `mask_ellipse` | `x`, `y`; `radius_x`, `radius_y`, `rotation`; `feather`; `proportional` |
//! | `mask_gradient` | `x`, `y` (a point of the line); `rotation`; `feather`; `profile` |
//! | `mask_path` | `points` (the whole list, see [`path_values`]); `feather` |
//! | every shape | `opacity`, `invert` |

use helios_core::{
    Context, Extent, Family, GpuKernel, Image, Manifest, Module, ModuleKind, Needs, ParamHint,
    ParamKind, ParamSpec, Params, Region, Role, Value,
};

use crate::{flag, mask_port};

/// The largest difference between a shape's GPU and CPU values (both in
/// 0 to 1): the GPU computes in `f32`.
pub const GPU_TOLERANCE: f32 = 2e-3;

/// The most points a path holds.
pub const MAX_PATH_POINTS: usize = 64;

/// Values a path point holds: its corner, the handle before it and the
/// handle after it (`x`, `y` each).
pub const PATH_POINT_VALUES: usize = 6;

/// The most straight pieces one Bézier segment is cut into.
const MAX_PIECES: usize = 31;

/// Coordinates and lengths in image pixels stay within this.
const FAR: f64 = 1.0e6;

const SHAPES_WGSL: &str = include_str!("shapes.wgsl");
const PATH_WGSL: &str = include_str!("path.wgsl");

/// `vec4`s of the path kernel's uniforms: two of header, then two points
/// a `vec4`.
const PATH_UNIFORM_VEC4S: usize = 1024;

// ------------------------------------------------------------------ profiles

/// Three-point Gauss-Legendre quadrature on `[-1, 1]`: exact for
/// polynomials up to degree 5.
const GAUSS: [(f64, f64); 3] = [
    (-0.774_596_669_241_483_4, 5.0 / 9.0),
    (0.0, 8.0 / 9.0),
    (0.774_596_669_241_483_4, 5.0 / 9.0),
];

/// A profile along one coordinate `u`: `below` up to `lo`, `above` from
/// `hi`, `f(u)` between.
struct Profile<F: Fn(f64) -> f64> {
    lo: f64,
    hi: f64,
    below: f64,
    above: f64,
    f: F,
    /// Quadrature pieces over `[lo, hi]` (1 for a polynomial).
    pieces: u32,
}

impl<F: Fn(f64) -> f64> Profile<F> {
    /// The mean of the profile over `[u - h, u + h]`: one pixel's box
    /// filter across the edge. Exactly `below` or `above` when the window
    /// lies on one side.
    fn mean(&self, u: f64, h: f64) -> f64 {
        let (a, b) = (u - h, u + h);
        if b <= self.lo {
            return self.below;
        }
        if a >= self.hi {
            return self.above;
        }
        let (m0, m1) = (a.max(self.lo), b.min(self.hi));
        let mut sum = self.below * (m0 - a) + self.above * (b - m1);
        if m1 > m0 {
            let step = (m1 - m0) / f64::from(self.pieces);
            let half = step / 2.0;
            for k in 0..self.pieces {
                let c = m0 + (f64::from(k) + 0.5) * step;
                for (x, w) in GAUSS {
                    sum += w * half * (self.f)(c + half * x);
                }
            }
        }
        sum / (b - a)
    }
}

/// darktable's circle and ellipse falloff along a ray from the centre: 1
/// inside radius `r`, 0 past `t`, `((t² - l²) / (t² - r²))²` between.
fn radial(l: f64, r: f64, t: f64, h: f64) -> f64 {
    let d = (t - r) * (t + r);
    Profile {
        lo: r,
        hi: t,
        below: 1.0,
        above: 0.0,
        f: |u: f64| {
            let q = (t - u) * (t + u) / d;
            q * q
        },
        pieces: 1,
    }
    .mean(l, h)
}

/// A linear falloff over a signed distance `d` (negative inside): 1 up to
/// 0, 0 from `feather`.
fn linear(d: f64, feather: f64, h: f64) -> f64 {
    Profile {
        lo: 0.0,
        hi: feather,
        below: 1.0,
        above: 0.0,
        f: |u: f64| 1.0 - u / feather,
        pieces: 1,
    }
    .mean(d, h)
}

/// The error function, Abramowitz and Stegun 7.1.26 (within 1.5e-7), the
/// same formula as the GPU's.
pub fn erf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let poly = ((((1.061_405_429 * t - 1.453_152_027) * t + 1.421_413_741) * t - 0.284_496_736)
        * t
        + 0.254_829_592)
        * t;
    let y = 1.0 - poly * (-x * x).exp();
    if x < 0.0 { -y } else { y }
}

// ------------------------------------------------------------------ members

/// A member's inversion and opacity, applied to the shape's value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Member {
    pub invert: bool,
    pub opacity: f32,
}

impl Member {
    fn read(params: &Params) -> Member {
        Member {
            invert: flag(params, "invert"),
            opacity: params.float("opacity") as f32,
        }
    }

    /// `(invert ? 1 - v : v) * opacity`.
    pub fn apply(self, v: f64) -> f32 {
        let v = v as f32;
        (if self.invert { 1.0 - v } else { v }) * self.opacity
    }

    /// What it gives where the shape is 0.
    pub fn fill(self) -> f32 {
        self.apply(0.0)
    }

    fn uniforms(self) -> [f32; 2] {
        [self.opacity, if self.invert { 1.0 } else { 0.0 }]
    }
}

/// Where a shape can be other than 0, in image pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bounds {
    /// Nothing: the shape is 0 everywhere.
    Empty,
    /// `[x0, y0, x1, y1]`.
    Box([f64; 4]),
    /// Everywhere (a gradient).
    Unbounded,
}

impl Bounds {
    /// The pixels at `scale` whose window (`h` image pixels around their
    /// centre) can meet the shape, with a pixel to spare.
    pub fn region(self, scale: f64, h: f64) -> Option<Region> {
        match self {
            Bounds::Unbounded => None,
            Bounds::Empty => Some(Region::new(0, 0, 0, 0)),
            Bounds::Box([x0, y0, x1, y1]) => {
                let lo = |v: f64| ((v - h) * scale - 0.5).floor() as i64 - 1;
                let hi = |v: f64| ((v + h) * scale - 0.5).ceil() as i64 + 1;
                let (rx, ry) = (lo(x0), lo(y0));
                Some(Region::new(
                    rx,
                    ry,
                    (hi(x1) - rx + 1) as u32,
                    (hi(y1) - ry + 1) as u32,
                ))
            }
        }
    }
}

/// What every shape's plan holds besides its geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Common {
    pub member: Member,
    /// The render's scale.
    pub scale: f64,
    /// Half a render pixel, in image pixels: the anti-aliasing window.
    pub h: f64,
}

impl Common {
    fn new(params: &Params, scale: f64) -> Common {
        Common {
            member: Member::read(params),
            scale,
            h: 0.5 / scale,
        }
    }

    /// The image coordinates of pixel `(x, y)`'s centre.
    fn centre(&self, x: i64, y: i64) -> (f64, f64) {
        ((x as f64 + 0.5) / self.scale, (y as f64 + 0.5) / self.scale)
    }

    fn extent(&self, bounds: Bounds) -> Option<Extent> {
        bounds.region(self.scale, self.h).map(|region| Extent {
            region,
            fill: self.member.fill(),
        })
    }

    /// The shape's member values over `output`, from `value` at each
    /// pixel's centre.
    fn render(&self, output: Region, value: impl Fn(f64, f64) -> f64) -> Image {
        Image::from_fn(output, 1, |x, y, _| {
            let (u, v) = self.centre(x, y);
            self.member.apply(value(u, v))
        })
    }

    fn uniforms(&self) -> [f32; 4] {
        let [opacity, invert] = self.member.uniforms();
        [self.h as f32, self.scale as f32, opacity, invert]
    }
}

fn member_params() -> [ParamSpec; 2] {
    [
        ParamSpec::float(
            "opacity",
            "multiplies the shape, after its inversion",
            0.0,
            1.0,
            1.0,
        ),
        ParamSpec {
            name: "invert".into(),
            description: "invert the shape, before its opacity".into(),
            kind: ParamKind::Bool { default: false },
        },
    ]
}

fn coordinate(name: &str, description: &str, default: f64) -> ParamSpec {
    ParamSpec::float(name, description, -FAR, FAR, default)
}

fn length(name: &str, description: &str, default: f64) -> ParamSpec {
    ParamSpec::float(name, description, 0.0, FAR, default)
}

fn angle(name: &str, description: &str) -> ParamSpec {
    ParamSpec::float(name, description, -360.0, 360.0, 0.0)
}

fn shape_manifest(name: &str, description: &str, mut params: Vec<ParamSpec>) -> Manifest {
    params.extend(member_params());
    Manifest {
        name: name.into(),
        version: 1,
        description: description.into(),
        params,
        inputs: vec![],
        outputs: vec![mask_port("mask")],
        context: vec![],
        needs: Needs::default(),
        kind: ModuleKind::Process,
        family: Family::Mask,
        role: Role::General,
    }
}

fn hints(pixels: &[&str], degrees: &[&str]) -> Vec<ParamHint> {
    let mut out = vec![ParamHint::new("opacity").unit("%").display(100.0, 0)];
    out.extend(pixels.iter().map(|p| ParamHint::new(p).unit("px")));
    out.extend(degrees.iter().map(|p| ParamHint::new(p).unit("°")));
    out
}

fn shapes_kernel(entry: &'static str) -> GpuKernel {
    GpuKernel {
        wgsl: SHAPES_WGSL,
        entry,
    }
}

/// The shapes kernel's uniforms: three `vec4`s, geometry first, the
/// common values last.
fn shapes_uniforms(geometry: &[f32], common: &Common) -> Vec<f32> {
    let mut u = geometry.to_vec();
    u.resize(8, 0.0);
    u.extend(common.uniforms());
    u
}

// ------------------------------------------------------------------ circle

/// A circle: 1 inside `radius`, falling to 0 at `radius + feather` with
/// darktable's quadratic falloff.
pub struct MaskCircle;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CirclePlan {
    pub cx: f64,
    pub cy: f64,
    pub radius: f64,
    /// The outer radius, `radius + feather`.
    pub outer: f64,
    pub common: Common,
}

impl CirclePlan {
    /// The shape's value (before inversion and opacity) at image point
    /// `(x, y)`.
    pub fn value(&self, x: f64, y: f64) -> f64 {
        let l = (x - self.cx).hypot(y - self.cy);
        radial(l, self.radius, self.outer, self.common.h)
    }

    pub fn bounds(&self) -> Bounds {
        let t = self.outer;
        Bounds::Box([self.cx - t, self.cy - t, self.cx + t, self.cy + t])
    }
}

impl Module for MaskCircle {
    type Plan = CirclePlan;

    fn manifest(&self) -> Manifest {
        shape_manifest(
            "mask_circle",
            "A drawn circle as a mask member, in image pixels",
            vec![
                coordinate("x", "centre, image pixels from the left", 16.0),
                coordinate("y", "centre, image pixels from the top", 12.0),
                length("radius", "where the falloff starts, image pixels", 8.0),
                length(
                    "feather",
                    "falloff width past the radius, image pixels",
                    4.0,
                ),
            ],
        )
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        hints(&["x", "y", "radius", "feather"], &[])
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<CirclePlan, String> {
        self.prepare_at(params, context, 1.0)
    }

    fn prepare_at(&self, p: &Params, _: &Context, scale: f64) -> Result<CirclePlan, String> {
        let radius = p.float("radius");
        Ok(CirclePlan {
            cx: p.float("x"),
            cy: p.float("y"),
            radius,
            outer: radius + p.float("feather"),
            common: Common::new(p, scale),
        })
    }

    fn extent(&self, plan: &CirclePlan) -> Option<Extent> {
        plan.common.extent(plan.bounds())
    }

    fn process(
        &self,
        plan: &CirclePlan,
        _: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        Ok(vec![plan.common.render(output, |x, y| plan.value(x, y))])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(shapes_kernel("circle"))
    }

    fn gpu_uniforms(&self, plan: &CirclePlan) -> Vec<f32> {
        let g = [plan.cx, plan.cy, plan.radius, plan.outer].map(|v| v as f32);
        shapes_uniforms(&g, &plan.common)
    }
}

// ------------------------------------------------------------------ ellipse

/// An ellipse, rotated: 1 inside, falling to 0 over `feather` with
/// darktable's quadratic falloff along each ray from the centre.
pub struct MaskEllipse;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EllipsePlan {
    pub cx: f64,
    pub cy: f64,
    /// Inner half-axes: along the rotated x axis, then y.
    pub a: f64,
    pub b: f64,
    /// Outer half-axes, where the falloff ends.
    pub ta: f64,
    pub tb: f64,
    pub cos: f64,
    pub sin: f64,
    pub common: Common,
}

/// The radius of the ellipse of half-axes `a`, `b` along a direction whose
/// cosine and sine (in the ellipse's frame) are `c`, `s`.
fn ellipse_radius(a: f64, b: f64, c: f64, s: f64) -> f64 {
    let d = a * a * s * s + b * b * c * c;
    if d > 0.0 { a * b / d.sqrt() } else { 0.0 }
}

impl EllipsePlan {
    pub fn value(&self, x: f64, y: f64) -> f64 {
        let (x, y) = (x - self.cx, y - self.cy);
        let l = x.hypot(y);
        let (xn, yn) = if l > 0.0 { (x / l, y / l) } else { (0.0, 1.0) };
        let c = xn * self.cos + yn * self.sin;
        let s = -xn * self.sin + yn * self.cos;
        radial(
            l,
            ellipse_radius(self.a, self.b, c, s),
            ellipse_radius(self.ta, self.tb, c, s),
            self.common.h,
        )
    }

    pub fn bounds(&self) -> Bounds {
        let ex = (self.ta * self.cos).hypot(self.tb * self.sin);
        let ey = (self.ta * self.sin).hypot(self.tb * self.cos);
        Bounds::Box([self.cx - ex, self.cy - ey, self.cx + ex, self.cy + ey])
    }
}

impl Module for MaskEllipse {
    type Plan = EllipsePlan;

    fn manifest(&self) -> Manifest {
        let mut params = vec![
            coordinate("x", "centre, image pixels from the left", 16.0),
            coordinate("y", "centre, image pixels from the top", 12.0),
            length(
                "radius_x",
                "half-axis along the rotated x axis, image pixels",
                10.0,
            ),
            length(
                "radius_y",
                "half-axis along the rotated y axis, image pixels",
                6.0,
            ),
            angle(
                "rotation",
                "degrees from the x axis toward the y axis (clockwise on screen)",
            ),
            length(
                "feather",
                "falloff width past the edge, image pixels (along the longer axis when proportional)",
                4.0,
            ),
        ];
        params.push(ParamSpec {
            name: "proportional".into(),
            description: "the falloff scales with each half-axis instead of keeping one width"
                .into(),
            kind: ParamKind::Bool { default: false },
        });
        shape_manifest(
            "mask_ellipse",
            "A drawn ellipse as a mask member, in image pixels",
            params,
        )
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        hints(
            &["x", "y", "radius_x", "radius_y", "feather"],
            &["rotation"],
        )
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<EllipsePlan, String> {
        self.prepare_at(params, context, 1.0)
    }

    fn prepare_at(&self, p: &Params, _: &Context, scale: f64) -> Result<EllipsePlan, String> {
        let (a, b) = (p.float("radius_x"), p.float("radius_y"));
        let feather = p.float("feather");
        let (ta, tb) = if flag(p, "proportional") {
            let longer = a.max(b);
            let ratio = if longer > 0.0 { feather / longer } else { 0.0 };
            (a * (1.0 + ratio), b * (1.0 + ratio))
        } else {
            (a + feather, b + feather)
        };
        let alpha = p.float("rotation").to_radians();
        Ok(EllipsePlan {
            cx: p.float("x"),
            cy: p.float("y"),
            a,
            b,
            ta,
            tb,
            cos: alpha.cos(),
            sin: alpha.sin(),
            common: Common::new(p, scale),
        })
    }

    fn extent(&self, plan: &EllipsePlan) -> Option<Extent> {
        plan.common.extent(plan.bounds())
    }

    fn process(
        &self,
        plan: &EllipsePlan,
        _: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        Ok(vec![plan.common.render(output, |x, y| plan.value(x, y))])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(shapes_kernel("ellipse"))
    }

    fn gpu_uniforms(&self, plan: &EllipsePlan) -> Vec<f32> {
        let g = [
            plan.cx, plan.cy, plan.a, plan.b, plan.ta, plan.tb, plan.cos, plan.sin,
        ]
        .map(|v| v as f32);
        shapes_uniforms(&g, &plan.common)
    }
}

// ------------------------------------------------------------------ gradient

/// A linear gradient: 0 on one side of a line through an anchor, 1 on the
/// other, over a transition `feather` wide centred on the line, linear or
/// sigmoid (darktable's gradient, without its curvature).
pub struct MaskGradient;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientPlan {
    pub ax: f64,
    pub ay: f64,
    /// The unit normal pointing to the full side.
    pub nx: f64,
    pub ny: f64,
    pub feather: f64,
    pub sigmoid: bool,
    pub common: Common,
}

impl GradientPlan {
    pub fn value(&self, x: f64, y: f64) -> f64 {
        let d = (x - self.ax) * self.nx + (y - self.ay) * self.ny;
        let w = self.feather;
        if self.sigmoid {
            Profile {
                lo: -2.0 * w,
                hi: 2.0 * w,
                below: 0.0,
                above: 1.0,
                f: |u: f64| 0.5 + 0.5 * erf(2.0 * u / w),
                pieces: 4,
            }
            .mean(d, self.common.h)
        } else {
            Profile {
                lo: -0.5 * w,
                hi: 0.5 * w,
                below: 0.0,
                above: 1.0,
                f: |u: f64| 0.5 + u / w,
                pieces: 1,
            }
            .mean(d, self.common.h)
        }
    }
}

/// The profiles a gradient takes.
pub const GRADIENT_PROFILES: [&str; 2] = ["linear", "sigmoid"];

impl Module for MaskGradient {
    type Plan = GradientPlan;

    fn manifest(&self) -> Manifest {
        let mut params = vec![
            coordinate("x", "a point of the line, image pixels from the left", 16.0),
            coordinate("y", "a point of the line, image pixels from the top", 12.0),
            angle(
                "rotation",
                "degrees: 0 is full above the line, 90 full left of it",
            ),
            length("feather", "width of the transition, image pixels", 8.0),
        ];
        params.push(ParamSpec {
            name: "profile".into(),
            description: "the transition: linear, or sigmoid (an error function)".into(),
            kind: ParamKind::Choice {
                options: GRADIENT_PROFILES.iter().map(|s| (*s).to_string()).collect(),
                default: "linear".into(),
            },
        });
        shape_manifest(
            "mask_gradient",
            "A drawn linear gradient as a mask member, in image pixels",
            params,
        )
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        hints(&["x", "y", "feather"], &["rotation"])
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<GradientPlan, String> {
        self.prepare_at(params, context, 1.0)
    }

    fn prepare_at(&self, p: &Params, _: &Context, scale: f64) -> Result<GradientPlan, String> {
        let r = p.float("rotation").to_radians();
        Ok(GradientPlan {
            ax: p.float("x"),
            ay: p.float("y"),
            nx: -r.sin(),
            ny: -r.cos(),
            feather: p.float("feather"),
            sigmoid: p.get("profile").and_then(Value::as_choice) == Some("sigmoid"),
            common: Common::new(p, scale),
        })
    }

    fn process(
        &self,
        plan: &GradientPlan,
        _: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        Ok(vec![plan.common.render(output, |x, y| plan.value(x, y))])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(shapes_kernel("gradient"))
    }

    fn gpu_uniforms(&self, plan: &GradientPlan) -> Vec<f32> {
        let g = [
            plan.ax,
            plan.ay,
            plan.nx,
            plan.ny,
            plan.feather,
            if plan.sigmoid { 1.0 } else { 0.0 },
        ]
        .map(|v| v as f32);
        shapes_uniforms(&g, &plan.common)
    }
}

// ------------------------------------------------------------------ path

/// A closed path through up to [`MAX_PATH_POINTS`] points, each with a
/// handle before and after it (cubic Bézier segments; a handle on its
/// corner makes a straight side, so a polygon is a path whose handles all
/// sit on their corners). 1 inside (even-odd rule), falling linearly to 0
/// at `feather` outside, as darktable's path falloff does.
pub struct MaskPath;

#[derive(Clone, Debug, PartialEq)]
pub struct PathPlan {
    /// The path cut into straight pieces, closed (the last point joins
    /// the first): image pixels.
    pub polygon: Vec<[f64; 2]>,
    pub feather: f64,
    pub common: Common,
}

/// One point of a path: its corner, the handle before it, the handle after it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathPoint {
    pub corner: [f64; 2],
    pub before: [f64; 2],
    pub after: [f64; 2],
}

impl PathPoint {
    /// A polygon's corner: both handles on it.
    pub fn corner(x: f64, y: f64) -> PathPoint {
        PathPoint {
            corner: [x, y],
            before: [x, y],
            after: [x, y],
        }
    }
}

/// A path's points as the `points` parameter holds them.
pub fn path_values(points: &[PathPoint]) -> Value {
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
                ]
            })
            .collect(),
    )
}

/// The `points` parameter read back.
pub fn path_points(values: &[f64]) -> Result<Vec<PathPoint>, String> {
    if !values.len().is_multiple_of(PATH_POINT_VALUES) {
        return Err(format!(
            "{} path values, not a multiple of {PATH_POINT_VALUES}",
            values.len()
        ));
    }
    Ok(values
        .chunks_exact(PATH_POINT_VALUES)
        .map(|v| PathPoint {
            corner: [v[0], v[1]],
            before: [v[2], v[3]],
            after: [v[4], v[5]],
        })
        .collect())
}

/// How far a straight piece may stray from its curve, in image pixels.
const FLATNESS: f64 = 0.1;

/// Cuts the closed path into straight pieces, each within [`FLATNESS`] of
/// its curve (at most [`MAX_PIECES`] a segment; a straight segment is one
/// piece). The pieces do not depend on the render's scale, so a preview
/// draws the same polygon as the full render.
pub fn flatten(points: &[PathPoint]) -> Vec<[f64; 2]> {
    let mut out = Vec::new();
    let n = points.len();
    for (i, p) in points.iter().enumerate() {
        let q = &points[(i + 1) % n];
        let (p0, p1, p2, p3) = (p.corner, p.after, q.before, q.corner);
        out.push(p0);
        if p1 == p0 && p2 == p3 {
            continue;
        }
        // A cubic's second derivative is at most 6 times its control
        // polygon's largest second difference; n pieces stray at most an
        // eighth of it over n².
        let second = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
            (a[0] - 2.0 * b[0] + c[0]).hypot(a[1] - 2.0 * b[1] + c[1])
        };
        let bend = 6.0 * second(p0, p1, p2).max(second(p1, p2, p3));
        let pieces = ((bend / (8.0 * FLATNESS)).sqrt().ceil() as usize).clamp(1, MAX_PIECES);
        for k in 1..pieces {
            let t = k as f64 / pieces as f64;
            let s = 1.0 - t;
            let (a, b, c, d) = (s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
            out.push([
                a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
                a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
            ]);
        }
    }
    out
}

/// Distance from `p` to the segment `a`-`b`.
fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let (px, py) = (p[0] - a[0], p[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        ((px * dx + py * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (px - t * dx).hypot(py - t * dy)
}

impl PathPlan {
    /// The polygon's sides.
    fn sides(&self) -> impl Iterator<Item = ([f64; 2], [f64; 2])> + '_ {
        let n = self.polygon.len();
        (0..n).map(move |i| (self.polygon[i], self.polygon[(i + 1) % n]))
    }

    /// The shape's value at image point `(x, y)` (slow: every side is
    /// read; [`MaskPath`] renders rows faster, to the same values).
    pub fn value(&self, x: f64, y: f64) -> f64 {
        if self.polygon.len() < 3 {
            return 0.0;
        }
        let mut inside = false;
        let mut dist = f64::INFINITY;
        for (a, b) in self.sides() {
            if (a[1] <= y) != (b[1] <= y) && x > a[0] + (y - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) {
                inside = !inside;
            }
            dist = dist.min(segment_distance([x, y], a, b));
        }
        linear(
            if inside { -dist } else { dist },
            self.feather,
            self.common.h,
        )
    }

    pub fn bounds(&self) -> Bounds {
        if self.polygon.len() < 3 {
            return Bounds::Empty;
        }
        let mut b = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for p in &self.polygon {
            b = [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ];
        }
        let f = self.feather;
        Bounds::Box([b[0] - f, b[1] - f, b[2] + f, b[3] + f])
    }

    /// One row of `output` (pixel row `y`): crossings of the row by the
    /// sides give inside or outside, and only sides within the falloff's
    /// reach are measured. Every value equals [`PathPlan::value`]'s.
    fn row(&self, output: Region, y: i64, out: &mut Vec<f32>) {
        let member = self.common.member;
        if self.polygon.len() < 3 {
            out.extend(std::iter::repeat_n(member.fill(), output.width as usize));
            return;
        }
        let (_, yc) = self.common.centre(0, y);
        let reach = self.feather + self.common.h;
        let mut crossings = Vec::new();
        let mut near = Vec::new();
        for (a, b) in self.sides() {
            if (a[1] <= yc) != (b[1] <= yc) {
                crossings.push(a[0] + (yc - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
            }
            if a[1].min(b[1]) - reach <= yc && yc <= a[1].max(b[1]) + reach {
                near.push((a, b));
            }
        }
        crossings.sort_by(f64::total_cmp);
        let mut left = 0;
        for x in output.x..output.right() {
            let (xc, _) = self.common.centre(x, y);
            while left < crossings.len() && crossings[left] < xc {
                left += 1;
            }
            let inside = left % 2 == 1;
            let mut dist = f64::INFINITY;
            for (a, b) in &near {
                if a[0].min(b[0]) - reach <= xc && xc <= a[0].max(b[0]) + reach {
                    dist = dist.min(segment_distance([xc, yc], *a, *b));
                }
            }
            // Past the reach the window lies on one side: exactly 1 or 0,
            // whichever sides were measured.
            let v = if dist > reach {
                if inside { 1.0 } else { 0.0 }
            } else {
                linear(
                    if inside { -dist } else { dist },
                    self.feather,
                    self.common.h,
                )
            };
            out.push(member.apply(v));
        }
    }
}

impl Module for MaskPath {
    type Plan = PathPlan;

    fn manifest(&self) -> Manifest {
        let square = [
            PathPoint::corner(8.0, 4.0),
            PathPoint::corner(26.0, 4.0),
            PathPoint::corner(26.0, 18.0),
            PathPoint::corner(8.0, 18.0),
        ];
        let Value::List(default) = path_values(&square) else {
            unreachable!("path_values gives a list")
        };
        shape_manifest(
            "mask_path",
            "A drawn closed path (a polygon or Bézier curves) as a mask member, in image pixels",
            vec![
                ParamSpec {
                    name: "points".into(),
                    description: format!(
                        "up to {MAX_PATH_POINTS} points, {PATH_POINT_VALUES} values each: \
                         the corner, the handle before it, the handle after it (x, y each, \
                         image pixels); fewer than 3 points draw nothing"
                    ),
                    kind: ParamKind::Floats {
                        count: (MAX_PATH_POINTS * PATH_POINT_VALUES) as u32,
                        min: -FAR,
                        max: FAR,
                        default,
                    },
                },
                length(
                    "feather",
                    "falloff width outside the path, image pixels",
                    4.0,
                ),
            ],
        )
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        hints(&["feather"], &[])
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<PathPlan, String> {
        self.prepare_at(params, context, 1.0)
    }

    fn prepare_at(&self, p: &Params, _: &Context, scale: f64) -> Result<PathPlan, String> {
        let values = p.get("points").and_then(Value::as_list).unwrap_or(&[]);
        let points = path_points(values)?;
        let polygon = if points.len() < 3 {
            Vec::new()
        } else {
            flatten(&points)
        };
        Ok(PathPlan {
            polygon,
            feather: p.float("feather"),
            common: Common::new(p, scale),
        })
    }

    fn extent(&self, plan: &PathPlan) -> Option<Extent> {
        plan.common.extent(plan.bounds())
    }

    fn process(&self, plan: &PathPlan, _: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let mut data = Vec::with_capacity(output.area());
        for y in output.y..output.bottom() {
            plan.row(output, y, &mut data);
        }
        Ok(vec![Image::new(output, 1, data)])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: PATH_WGSL,
            entry: "path",
        })
    }

    fn gpu_uniforms(&self, plan: &PathPlan) -> Vec<f32> {
        let n = if plan.polygon.len() < 3 {
            0
        } else {
            plan.polygon.len()
        };
        let mut u = vec![n as f32, plan.feather as f32, 0.0, 0.0];
        u.extend(plan.common.uniforms());
        for p in &plan.polygon {
            u.extend([p[0] as f32, p[1] as f32]);
        }
        u.resize(PATH_UNIFORM_VEC4S * 4, 0.0);
        u
    }
}

// ------------------------------------------------------------------ outlines

/// What a surface draws over the image for a shape: in image pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outline {
    /// The shape's edge (where it starts falling off), closed unless it is
    /// a gradient's line.
    pub edge: Vec<[f64; 2]>,
    /// Where the falloff ends (empty when the feather is 0, and for a path,
    /// whose falloff is a distance from its edge). A gradient's is its
    /// band: one side's two points, then the other's.
    pub feather: Vec<[f64; 2]>,
    /// Whether `edge` and `feather` close on themselves.
    pub closed: bool,
    pub handles: Vec<Handle>,
}

/// A point a surface lets the user drag, and the parameters it moves.
#[derive(Clone, Debug, PartialEq)]
pub struct Handle {
    /// `centre`, `radius`, `radius_x`, `radius_y`, `feather`, `rotation`,
    /// or for a path `corner.<i>`, `before.<i>`, `after.<i>`.
    pub name: String,
    pub at: [f64; 2],
    /// The parameters a drag of this handle writes.
    pub writes: Vec<&'static str>,
}

fn handle(name: &str, at: [f64; 2], writes: &[&'static str]) -> Handle {
    Handle {
        name: name.into(),
        at,
        writes: writes.to_vec(),
    }
}

/// An ellipse of half-axes `a`, `b` rotated by `cos`, `sin` around
/// `(cx, cy)`, as a polygon of 72 points.
fn ellipse_points(cx: f64, cy: f64, a: f64, b: f64, cos: f64, sin: f64) -> Vec<[f64; 2]> {
    (0..72)
        .map(|k| {
            let phi = f64::from(k) * std::f64::consts::TAU / 72.0;
            let (x, y) = (a * phi.cos(), b * phi.sin());
            [cx + x * cos - y * sin, cy + x * sin + y * cos]
        })
        .collect()
}

/// What to draw for a shape member: its module's name and its parameters
/// (defaults filled in for those missing). An error for a module that is
/// not a shape, or parameters it refuses.
pub fn outline(module: &str, params: &Params) -> Result<Outline, String> {
    let resolve = |m: &Manifest| {
        let mut p = m.defaults();
        for (k, v) in params.iter() {
            p.set(k, v.clone());
        }
        p
    };
    let cx = Context::new();
    match module {
        "mask_circle" => {
            let p = MaskCircle.prepare(&resolve(&MaskCircle.manifest()), &cx)?;
            let (x, y) = (p.cx, p.cy);
            Ok(Outline {
                edge: ellipse_points(x, y, p.radius, p.radius, 1.0, 0.0),
                feather: if p.outer > p.radius {
                    ellipse_points(x, y, p.outer, p.outer, 1.0, 0.0)
                } else {
                    Vec::new()
                },
                closed: true,
                handles: vec![
                    handle("centre", [x, y], &["x", "y"]),
                    handle("radius", [x + p.radius, y], &["radius"]),
                    handle("feather", [x + p.outer, y], &["feather"]),
                ],
            })
        }
        "mask_ellipse" => {
            let p = MaskEllipse.prepare(&resolve(&MaskEllipse.manifest()), &cx)?;
            let along = |r: f64| [p.cx + r * p.cos, p.cy + r * p.sin];
            let across = |r: f64| [p.cx - r * p.sin, p.cy + r * p.cos];
            Ok(Outline {
                edge: ellipse_points(p.cx, p.cy, p.a, p.b, p.cos, p.sin),
                feather: if p.ta > p.a || p.tb > p.b {
                    ellipse_points(p.cx, p.cy, p.ta, p.tb, p.cos, p.sin)
                } else {
                    Vec::new()
                },
                closed: true,
                handles: vec![
                    handle("centre", [p.cx, p.cy], &["x", "y"]),
                    handle("radius_x", along(p.a), &["radius_x", "rotation"]),
                    handle("radius_y", across(p.b), &["radius_y", "rotation"]),
                    handle("feather", along(p.ta), &["feather"]),
                ],
            })
        }
        "mask_gradient" => {
            let p = MaskGradient.prepare(&resolve(&MaskGradient.manifest()), &cx)?;
            // The line runs across the normal; drawn 10 000 pixels each way.
            let (tx, ty) = (-p.ny, p.nx);
            let line = |off: f64| {
                let (ox, oy) = (p.ax + off * p.nx, p.ay + off * p.ny);
                vec![
                    [ox - 1e4 * tx, oy - 1e4 * ty],
                    [ox + 1e4 * tx, oy + 1e4 * ty],
                ]
            };
            let half = 0.5 * p.feather;
            let mut feather = Vec::new();
            if half > 0.0 {
                feather = line(-half);
                feather.extend(line(half).into_iter().rev());
            }
            Ok(Outline {
                edge: line(0.0),
                feather,
                closed: false,
                handles: vec![
                    handle("centre", [p.ax, p.ay], &["x", "y"]),
                    handle(
                        "rotation",
                        [p.ax + 50.0 * tx, p.ay + 50.0 * ty],
                        &["rotation"],
                    ),
                    handle(
                        "feather",
                        [p.ax + half * p.nx, p.ay + half * p.ny],
                        &["feather"],
                    ),
                ],
            })
        }
        "mask_path" => {
            let resolved = resolve(&MaskPath.manifest());
            let p = MaskPath.prepare(&resolved, &cx)?;
            let values = resolved
                .get("points")
                .and_then(Value::as_list)
                .unwrap_or(&[]);
            let mut handles = Vec::new();
            for (i, point) in path_points(values)?.iter().enumerate() {
                handles.push(handle(&format!("corner.{i}"), point.corner, &["points"]));
                handles.push(handle(&format!("before.{i}"), point.before, &["points"]));
                handles.push(handle(&format!("after.{i}"), point.after, &["points"]));
            }
            Ok(Outline {
                edge: p.polygon,
                feather: Vec::new(),
                closed: true,
                handles,
            })
        }
        other => Err(format!("`{other}` is not a drawn shape")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_on_one_side_gives_the_constant_exactly() {
        assert_eq!(radial(10.0, 3.0, 5.0, 0.5), 0.0);
        assert_eq!(radial(2.0, 3.0, 5.0, 0.5), 1.0);
        assert_eq!(linear(-3.0, 2.0, 0.5), 1.0);
        assert_eq!(linear(2.5, 2.0, 0.5), 0.0);
    }

    #[test]
    fn a_hard_edge_gives_the_pixel_coverage() {
        // Feather 0: the box filter is the share of the window inside.
        assert!((linear(0.0, 0.0, 0.5) - 0.5).abs() < 1e-12);
        assert!((linear(0.25, 0.0, 0.5) - 0.25).abs() < 1e-12);
        assert!((radial(3.0, 3.0, 3.0, 0.5) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn the_box_filter_integrates_the_profiles_exactly() {
        // The quadratic falloff's mean over a window, against a fine sum.
        let (r, t, h) = (3.0, 7.0, 0.8);
        for l in [2.5, 3.0, 4.4, 6.5, 7.1] {
            let n = 20_000;
            let fine: f64 = (0..n)
                .map(|k| {
                    let u = l - h + (f64::from(k) + 0.5) / f64::from(n) * 2.0 * h;
                    radial(u, r, t, 1e-9)
                })
                .sum::<f64>()
                / f64::from(n);
            assert!((radial(l, r, t, h) - fine).abs() < 1e-6, "at {l}");
        }
    }

    #[test]
    fn erf_matches_known_values() {
        for (x, e) in [
            (0.0, 0.0),
            (0.5, 0.520_499_877_8),
            (1.0, 0.842_700_792_9),
            (-2.0, -0.995_322_265),
        ] {
            assert!((erf(x) - e).abs() < 2e-7, "erf({x})");
        }
    }

    #[test]
    fn a_polygon_path_flattens_to_its_corners() {
        let square = [
            PathPoint::corner(0.0, 0.0),
            PathPoint::corner(4.0, 0.0),
            PathPoint::corner(4.0, 4.0),
        ];
        assert_eq!(flatten(&square).len(), 3);
        let curved = [
            PathPoint {
                after: [50.0, -40.0],
                ..PathPoint::corner(0.0, 0.0)
            },
            PathPoint::corner(100.0, 0.0),
            PathPoint::corner(50.0, 80.0),
        ];
        let polygon = flatten(&curved);
        assert!(polygon.len() > 3 && polygon.len() <= 3 * MAX_PIECES);
        // Every piece's midpoint stays within the flatness of the curve:
        // the first segment, sampled finely.
        let pieces = polygon.len() - 2;
        let curve = |t: f64| {
            let s = 1.0 - t;
            let (b, c) = (3.0 * s * s * t, 3.0 * s * t * t);
            [50.0 * b + 100.0 * (c + t * t * t), -40.0 * b]
        };
        for k in 0..pieces {
            let (p, q) = (polygon[k], polygon[k + 1]);
            let mid = [f64::midpoint(p[0], q[0]), f64::midpoint(p[1], q[1])];
            let near = (0..=1000)
                .map(|i| {
                    let c = curve(f64::from(i) / 1000.0);
                    (c[0] - mid[0]).hypot(c[1] - mid[1])
                })
                .fold(f64::INFINITY, f64::min);
            assert!(near < FLATNESS + 0.01, "piece {k}: {near}");
        }
    }

    #[test]
    fn outlines_sit_where_the_shape_s_edge_is() {
        let p = Params::new()
            .with("x", Value::Float(50.0))
            .with("y", Value::Float(40.0))
            .with("radius", Value::Float(10.0))
            .with("feather", Value::Float(5.0));
        let o = outline("mask_circle", &p).unwrap();
        assert!(o.closed);
        for q in &o.edge {
            assert!(((q[0] - 50.0).hypot(q[1] - 40.0) - 10.0).abs() < 1e-9);
        }
        let names: Vec<&str> = o.handles.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["centre", "radius", "feather"]);
        assert_eq!(o.handles[2].at, [65.0, 40.0]);

        let g = outline(
            "mask_gradient",
            &Params::new().with("rotation", Value::Float(90.0)),
        )
        .unwrap();
        // Full left of a vertical line: the line runs along y.
        assert!((g.edge[0][0] - g.edge[1][0]).abs() < 1e-6);
        assert_eq!(g.feather.len(), 4);

        let path = outline("mask_path", &Params::new()).unwrap();
        assert_eq!(path.handles.len(), 12);
        assert!(outline("mask_blur", &Params::new()).is_err());
    }

    #[test]
    fn bounds_cover_every_pixel_the_shape_reaches() {
        let b = Bounds::Box([10.0, 20.0, 30.0, 40.0]);
        let r = b.region(1.0, 0.5).unwrap();
        // Pixels 9..=30 have centres within half a pixel of the box.
        assert!(
            r.x <= 9 && r.right() > 30 && r.y <= 19 && r.bottom() > 40,
            "{r}"
        );
        let r = b.region(0.5, 1.0).unwrap();
        assert!(r.x <= 4 && r.right() > 15, "{r}");
        assert_eq!(
            Bounds::Empty.region(1.0, 0.5),
            Some(Region::new(0, 0, 0, 0))
        );
        assert_eq!(Bounds::Unbounded.region(1.0, 0.5), None);
    }
}
