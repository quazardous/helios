//! Synthetic camera-rendered pairs: a scene in scene-linear working RGB,
//! a known "camera" rendering it into an 8-bit sRGB preview, and the fit
//! asked to find the camera back.

use super::*;
use helios_photo::color::{REC2020_TO_XYZ, SRGB_TO_XYZ, inverse, mul, srgb_decode, srgb_encode};

/// A deterministic generator (splitmix64).
struct Rng(u64);

impl Rng {
    fn uniform(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// How a synthetic scene is drawn.
#[derive(Clone, Copy)]
struct Scene {
    width: usize,
    height: usize,
    /// Patch luminances, log-uniform between these.
    levels: (f64, f64),
    /// Each channel off the neutral by up to this, relative.
    colour: f64,
    /// A horizontal gradient across each patch, relative.
    gradient: f64,
    /// Per-pixel noise, relative.
    noise: f64,
}

const PATCH: usize = 20;

impl Scene {
    fn new(width: usize, height: usize) -> Scene {
        Scene {
            width,
            height,
            // Within the 8-bit sRGB preview once rendered, nearly
            // unclipped.
            levels: (0.005, 0.4),
            colour: 0.2,
            gradient: 0.4,
            noise: 0.01,
        }
    }

    fn draw(&self, seed: u64) -> Proxy {
        let mut rng = Rng(seed);
        let (px, py) = (self.width.div_ceil(PATCH), self.height.div_ceil(PATCH));
        let patches: Vec<[f64; 3]> = (0..px * py)
            .map(|_| {
                let (lo, hi) = self.levels;
                let level = (lo.ln() + rng.uniform() * (hi.ln() - lo.ln())).exp();
                let c = [0; 3].map(|_| 1.0 + self.colour * (2.0 * rng.uniform() - 1.0));
                let l = luma(c);
                c.map(|v| v / l * level)
            })
            .collect();
        let mut rgb = Vec::with_capacity(self.width * self.height);
        for y in 0..self.height {
            for x in 0..self.width {
                let p = patches[(y / PATCH) * px + x / PATCH];
                let along = (x % PATCH) as f64 / PATCH as f64 - 0.5;
                let g = 1.0 + self.gradient * along;
                rgb.push(
                    p.map(|v| (v * g * (1.0 + self.noise * (2.0 * rng.uniform() - 1.0))).max(1e-6)),
                );
            }
        }
        Proxy::new(self.width, self.height, rgb)
    }
}

/// A known camera: a matrix keeping (1, 1, 1), when asked a hue twist
/// (greens and yellows turned, luminance kept), an S curve with a
/// shoulder applied as a luminance ratio, and, when asked, chroma falling
/// off in the highlights.
#[derive(Clone, Copy)]
struct Camera {
    matrix: Mat3,
    twist: bool,
    falloff: bool,
}

/// The twist's hue shift, in degrees, at hue `h`: up to 15 around 100.
fn twist(h: f64) -> f64 {
    15.0 * (-(table::hue_difference(h, 100.0) / 35.0).powi(2)).exp()
}

fn twisted(p: [f64; 3]) -> [f64; 3] {
    let [h, s, v] = table::hsv(p);
    if s <= 0.0 {
        return p;
    }
    let o = table::from_hsv([h + twist(h), s, v]);
    let k = luma(p) / luma(o);
    o.map(|c| c * k)
}

const CAMERA_MATRIX: Mat3 = [
    [1.25, -0.18, -0.07],
    [-0.08, 1.15, -0.07],
    [0.02, -0.12, 1.10],
];

fn camera_curve(s: f64) -> f64 {
    let g = s.max(0.0).powf(1.5);
    (1.08 * g / (g + 0.15)).min(1.0)
}

fn falloff(d: f64) -> f64 {
    if d < 0.6 { 1.0 } else { 1.0 - 1.25 * (d - 0.6) }
}

impl Camera {
    fn new() -> Camera {
        Camera {
            matrix: CAMERA_MATRIX,
            twist: false,
            falloff: true,
        }
    }

    fn render(&self, x: [f64; 3]) -> [f64; 3] {
        let p = mat_apply(&self.matrix, x);
        let p = if self.twist { twisted(p) } else { p };
        let s = luma(p);
        if s <= 0.0 {
            return [0.0; 3];
        }
        let d = camera_curve(s);
        let o = p.map(|v| v * d / s);
        if self.falloff {
            let k = falloff(d);
            o.map(|v| d + k * (v - d))
        } else {
            o
        }
    }
}

fn rec2020_to_srgb() -> Mat3 {
    mul(&inverse(&SRGB_TO_XYZ).unwrap(), &REC2020_TO_XYZ)
}

fn srgb_to_rec2020() -> Mat3 {
    mul(&inverse(&REC2020_TO_XYZ).unwrap(), &SRGB_TO_XYZ)
}

/// A display-linear working pixel through an 8-bit sRGB JPEG: clamped,
/// encoded, a little noise, quantised, read back. And whether a channel
/// reached 255.
fn jpeg(o: [f64; 3], rng: &mut Rng) -> ([f64; 3], bool) {
    // A grey is the same grey in both spaces (the rounded matrices
    // would split it by a level now and then).
    let s = if o[0] == o[1] && o[1] == o[2] {
        o
    } else {
        mat_apply(&rec2020_to_srgb(), o)
    };
    // One draw per pixel: a grey stays grey, as in a black-and-white JPEG.
    let noise = (rng.uniform() - 0.5) * 0.8;
    let q = s.map(|v| {
        let e = srgb_encode(v.clamp(0.0, 1.0)) * 255.0 + noise;
        e.round().clamp(0.0, 255.0)
    });
    let clipped = q.contains(&255.0);
    let back = q.map(|v| srgb_decode(v / 255.0));
    (mat_apply(&srgb_to_rec2020(), back), clipped)
}

/// Helios's default render, in the tests: no matrix, a generic curve per
/// channel.
fn default_render(x: [f64; 3]) -> [f64; 3] {
    x.map(|v| (1.1 * v / (v + 0.15)).clamp(0.0, 1.0))
}

/// A fit's inputs, owned.
struct Case {
    sensor: Proxy,
    preview: Proxy,
    baseline: Proxy,
    clipped: Vec<bool>,
    preview_clipped: Vec<bool>,
    neutral: [f64; 3],
    source: Source,
}

impl Case {
    fn new(scene: &Scene, camera: Camera) -> Case {
        Case::drawn(scene, |_, _, x| camera.render(x))
    }

    /// The scene rendered by `camera(x, y, pixel)`.
    fn drawn(scene: &Scene, camera: impl Fn(usize, usize, [f64; 3]) -> [f64; 3]) -> Case {
        let sensor = scene.draw(7);
        let mut rng = Rng(11);
        let (mut preview, mut preview_clipped) = (Vec::new(), Vec::new());
        for (i, &x) in sensor.rgb.iter().enumerate() {
            let (p, c) = jpeg(camera(i % scene.width, i / scene.width, x), &mut rng);
            preview.push(p);
            preview_clipped.push(c);
        }
        let baseline = sensor.rgb.iter().map(|&x| default_render(x)).collect();
        let n = sensor.rgb.len();
        Case {
            preview: Proxy::new(scene.width, scene.height, preview),
            baseline: Proxy::new(scene.width, scene.height, baseline),
            sensor,
            clipped: vec![false; n],
            preview_clipped,
            neutral: [1.0; 3],
            source: Source {
                raw_size: [6000, 4000],
                preview_size: [1620, 1080],
                jpeg_quality: Some(92),
                local_effect: None,
            },
        }
    }

    fn inputs(&self) -> LookInputs<'_> {
        LookInputs {
            sensor: &self.sensor,
            preview: &self.preview,
            baseline: &self.baseline,
            clipped: &self.clipped,
            preview_clipped: &self.preview_clipped,
            neutral: self.neutral,
            source: &self.source,
        }
    }

    fn fit(&self) -> Result<Look, Refusal> {
        fit(&self.inputs())
    }

    fn refused(&self) -> Gate {
        match self.fit() {
            Ok(look) => panic!("fitted where a refusal was expected: {:?}", look.report),
            Err(r) => r.gate,
        }
    }
}

fn standard() -> Case {
    Case::new(&Scene::new(192, 128), Camera::new())
}

#[test]
fn a_known_camera_is_found_back_at_both_proxy_sizes() {
    for (w, h) in [(192, 128), (96, 64)] {
        let look = Case::new(&Scene::new(w, h), Camera::new()).fit().unwrap();
        let r = &look.report;
        assert_eq!(look.kind, Kind::Full, "{w}×{h}");
        assert_eq!(look.version, FITTER_VERSION);
        assert_eq!(r.attempt, Attempt::AllPixels);
        assert!(r.held_out.mean < 1.0, "{w}×{h}: {r:?}");
        assert!(r.held_out.mean < t::BAR * r.baseline.mean, "{r:?}");
        assert!(r.notes.is_empty(), "{r:?}");
    }
}

#[test]
fn a_matrix_only_camera_gives_its_matrix_back() {
    let camera = Camera {
        falloff: false,
        ..Camera::new()
    };
    let look = Case::new(&Scene::new(192, 128), camera).fit().unwrap();
    let pairs = look
        .matrix
        .iter()
        .flatten()
        .zip(CAMERA_MATRIX.iter().flatten());
    for (k, (got, want)) in pairs.enumerate() {
        assert!(
            (got - want).abs() < 0.01,
            "coefficient {k}: {got} for {want}"
        );
    }
}

#[test]
fn the_chroma_curve_stays_only_when_it_helps() {
    let with = standard().fit().unwrap();
    let chroma = with
        .chroma
        .expect("a camera bleaching highlights needs the chroma curve");
    // Bleached above 0.6, untouched below.
    assert!(chroma.eval(0.7) < 0.95, "{chroma:?}");
    assert!((chroma.eval(0.3) - 1.0).abs() < 0.05, "{chroma:?}");
    let plain = Camera {
        falloff: false,
        ..Camera::new()
    };
    let without = Case::new(&Scene::new(192, 128), plain).fit().unwrap();
    assert_eq!(without.chroma, None, "{:?}", without.report);
}

#[test]
fn neutrals_stay_neutral() {
    // The matrix keeps any neutral, by construction.
    let case = standard();
    let neutral = [0.8, 1.0, 1.2];
    let m = fit_matrix(&case.sensor.rgb, &case.preview.rgb, neutral).unwrap();
    let back = mat_apply(&m, neutral);
    for c in 0..3 {
        assert!((back[c] - neutral[c]).abs() < 1e-12, "{back:?}");
    }
    // A fitted look renders a grey grey, chroma curve included.
    let look = case.fit().unwrap();
    assert!(look.chroma.is_some());
    for g in [0.01, 0.05, 0.18, 0.5, 1.0, 2.0] {
        let o = apply(&look, [g; 3]);
        assert!(
            (o[0] - o[1]).abs() < 1e-12 && (o[1] - o[2]).abs() < 1e-12,
            "{g}: {o:?}"
        );
    }
}

#[test]
fn the_tone_curve_is_monotone_bounded_and_keeps_black() {
    let tone = standard().fit().unwrap().tone;
    assert_eq!(tone.eval(0.0), 0.0);
    assert_eq!(tone.eval(-1.0), 0.0);
    let mut last = 0.0;
    for i in 1..=4000 {
        let d = tone.eval(f64::from(i) / 1000.0);
        assert!(d >= last, "falls at {i}: {d} after {last}");
        assert!(d <= 1.0, "{d}");
        last = d;
    }
    assert!(tone.eval(100.0) <= 1.0);
    assert!(tone.elasticity() <= t::MAX_ELASTICITY);
    // Close to the camera's curve at mid tones (the matrix is
    // luminance-neutral enough for the scene luminance to compare).
    assert!((tone.eval(0.18) - camera_curve(0.18)).abs() < 0.03);
}

#[test]
fn a_cliff_in_the_tone_curve_is_refused() {
    let scene: Vec<f64> = (0..4000)
        .map(|i| 0.1 + 0.9 * f64::from(i) / 4000.0)
        .collect();
    let display: Vec<f64> = scene
        .iter()
        .map(|&s| if s < 0.5 { 0.05 } else { 0.9 })
        .collect();
    assert_eq!(fit_tone(&scene, &display).unwrap_err().gate, Gate::TooSteep);
    // A gentle curve passes.
    let gentle: Vec<f64> = scene.iter().map(|&s| camera_curve(s)).collect();
    assert!(fit_tone(&scene, &gentle).is_ok());
}

#[test]
fn a_tiny_preview_is_refused() {
    let mut case = standard();
    case.source.preview_size = [300, 200];
    assert_eq!(case.refused(), Gate::PreviewTooSmall);
    // Large enough in pixels, under a tenth of the raw.
    case.source.raw_size = [9000, 6000];
    case.source.preview_size = [810, 540];
    assert_eq!(case.refused(), Gate::PreviewTooSmall);
    case.source.preview_size = [0, 0];
    assert_eq!(case.refused(), Gate::PreviewTooSmall);
}

#[test]
fn a_preview_of_another_aspect_is_refused() {
    let mut case = standard();
    case.source.preview_size = [1920, 1080];
    assert_eq!(case.refused(), Gate::AspectMismatch);
}

#[test]
fn a_heavily_compressed_preview_is_refused() {
    let mut case = standard();
    case.source.jpeg_quality = Some(60);
    assert_eq!(case.refused(), Gate::TooCompressed);
    case.source.jpeg_quality = Some(80);
    assert!(case.fit().is_ok());
}

#[test]
fn a_local_picture_effect_is_refused() {
    let mut case = standard();
    case.source.local_effect = Some("toy camera".into());
    assert_eq!(case.refused(), Gate::LocalEffect);
}

#[test]
fn a_shifted_preview_is_refused() {
    let case = standard();
    let (w, h) = (case.preview.width, case.preview.height);
    let shifted = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            case.preview.rgb[y * w + x.saturating_sub(3)]
        })
        .collect();
    let case = Case {
        preview: Proxy::new(w, h, shifted),
        ..case
    };
    assert_eq!(case.refused(), Gate::Structure);
}

#[test]
fn too_few_pairs_are_refused() {
    let case = Case::new(&Scene::new(16, 12), Camera::new());
    assert_eq!(case.refused(), Gate::TooFewPairs);
}

#[test]
fn too_few_held_out_pairs_are_refused() {
    let mut case = standard();
    // Every held-out pixel clipped: a third of the proxy, under the
    // clipping gate.
    case.clipped = split(192, 128);
    assert_eq!(case.refused(), Gate::TooFewHeldOut);
}

#[test]
fn a_grey_scene_gets_no_matrix() {
    let scene = Scene {
        colour: 0.0,
        ..Scene::new(192, 128)
    };
    let look = Case::new(&scene, Camera::new()).fit().unwrap();
    assert_eq!(look.kind, Kind::Partial);
    assert_eq!(look.matrix, IDENTITY);
    assert_eq!(look.report.notes, vec![Gate::NotEnoughColour]);
}

#[test]
fn a_flat_scene_is_refused() {
    let scene = Scene {
        levels: (0.2, 0.2),
        gradient: 0.0,
        ..Scene::new(192, 128)
    };
    assert_eq!(Case::new(&scene, Camera::new()).refused(), Gate::FlatScene);
}

#[test]
fn heavy_clipping_is_refused() {
    let mut case = standard();
    let n = case.clipped.len();
    for (i, c) in case.clipped.iter_mut().enumerate() {
        *c = i < n * 6 / 10;
    }
    assert_eq!(case.refused(), Gate::SensorClipped);
    // The camera clipping what the raw kept.
    let mut case = standard();
    for (i, c) in case.preview_clipped.iter_mut().enumerate() {
        *c |= i % 10 == 0;
    }
    assert_eq!(case.refused(), Gate::PreviewClipped);
}

#[test]
fn a_black_and_white_preview_gets_a_tone_curve_only() {
    let camera = Camera::new();
    let case = Case::drawn(&Scene::new(192, 128), |_, _, x| [luma(camera.render(x)); 3]);
    let look = case.fit().unwrap();
    assert_eq!(look.kind, Kind::ToneOnly);
    assert_eq!(look.matrix, IDENTITY);
    assert_eq!(look.chroma, None);
    assert_eq!(look.report.notes, vec![Gate::Monochrome]);
}

#[test]
fn an_unrelated_preview_is_refused() {
    let case = standard();
    let n = case.preview.rgb.len();
    let mut order: Vec<usize> = (0..n).collect();
    let mut rng = Rng(3);
    for i in (1..n).rev() {
        let j = (rng.uniform() * (i + 1) as f64) as usize;
        order.swap(i, j);
    }
    let preview = order.iter().map(|&i| case.preview.rgb[i]).collect();
    let preview_clipped = order.iter().map(|&i| case.preview_clipped[i]).collect();
    let case = Case {
        preview: Proxy::new(192, 128, preview),
        preview_clipped,
        ..case
    };
    assert_eq!(case.refused(), Gate::Unrelated);
}

#[test]
fn a_vignette_in_the_preview_fails_the_score() {
    let camera = Camera::new();
    let (w, h) = (192.0, 128.0);
    let case = Case::drawn(&Scene::new(192, 128), |x, y, p| {
        let (dx, dy) = (
            (x as f64 - w / 2.0) / (w / 2.0),
            (y as f64 - h / 2.0) / (h / 2.0),
        );
        let r2 = f64::midpoint(dx * dx, dy * dy);
        camera.render(p).map(|v| v * (1.0 - 0.9 * r2))
    });
    // Refused by the curve it would need or by the score.
    let gate = case.refused();
    assert!(
        matches!(gate, Gate::TooFar | Gate::NotBetter | Gate::TooSteep),
        "{gate:?}"
    );
}

#[test]
fn a_fit_no_better_than_the_default_is_refused() {
    let mut case = standard();
    // A default render that is the preview itself.
    case.baseline = case.preview.clone();
    assert_eq!(case.refused(), Gate::NotBetter);
}

#[test]
fn non_finite_pixels_are_left_out() {
    let mut case = standard();
    for i in (0..case.sensor.rgb.len()).step_by(97) {
        case.sensor.rgb[i] = [f64::NAN, 0.1, f64::INFINITY];
    }
    let look = case.fit().unwrap();
    assert!(look.matrix.iter().flatten().all(|v| v.is_finite()));
    assert!(look.report.held_out.mean.is_finite());
}

#[test]
fn mismatched_inputs_are_refused() {
    let mut case = standard();
    case.clipped.pop();
    assert_eq!(case.refused(), Gate::Inputs);
}

#[test]
fn the_clipping_budget() {
    assert!(clips_more(0.01, 0.0, 0.0));
    assert!(!clips_more(0.0005, 0.0, 0.0));
    assert!(!clips_more(0.02, 0.02, 0.0));
    assert!(!clips_more(0.02, 0.0, 0.0195));
    assert!(clips_more(0.03, 0.02, 0.01));
}

#[test]
fn the_split_is_fixed_a_third_and_shared_by_every_attempt() {
    for (w, h) in [(192, 128), (96, 64)] {
        let s = split(w, h);
        assert_eq!(s, split(w, h));
        assert_eq!(s.iter().filter(|&&b| b).count() * 3, w * h, "{w}×{h}");
        // Whole tiles.
        for y in 0..h {
            for x in 0..w {
                assert_eq!(s[y * w + x], s[(y / TILE * TILE) * w + x / TILE * TILE]);
            }
        }
    }
    let case = standard();
    let held = split(192, 128);
    let all = pairs(&case.inputs(), &held, None);
    let edges = away_from_edges(&case.sensor);
    let narrow = pairs(&case.inputs(), &held, Some(&edges));
    for p in [&all, &narrow] {
        assert!(p.held.iter().all(|&i| held[i]));
        assert!(p.train.iter().chain(&p.tone).all(|&i| !held[i]));
    }
    assert!(narrow.held.iter().all(|i| all.held.contains(i)));
}

#[test]
fn two_runs_are_bit_identical() {
    let one = standard().fit().unwrap();
    let two = standard().fit().unwrap();
    assert_eq!(
        serde_json::to_string(&one).unwrap(),
        serde_json::to_string(&two).unwrap()
    );
    let bits = |l: &Look| {
        l.matrix
            .iter()
            .flatten()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>()
    };
    assert_eq!(bits(&one), bits(&two));
    // Stored and read back, it keeps its numbers (to serde_json's parsing
    // precision, which is not always the last bit).
    let back: Look = serde_json::from_str(&serde_json::to_string(&one).unwrap()).unwrap();
    assert_eq!(back.kind, one.kind);
    let close = back
        .matrix
        .iter()
        .flatten()
        .zip(one.matrix.iter().flatten());
    assert!(
        close
            .into_iter()
            .all(|(a, b)| (a - b).abs() <= 1e-15 * b.abs().max(1.0))
    );
}

#[test]
fn the_jpeg_quality_is_estimated_from_its_table() {
    const IJG: [u16; 64] = [
        16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69,
        56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81,
        104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
    ];
    for q in [30u32, 50, 75, 90] {
        let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };
        let table = IJG.map(|v| ((u32::from(v) * scale + 50) / 100).clamp(1, 255) as u16);
        let got = i64::from(estimate_quality(&table));
        assert!((got - i64::from(q)).abs() <= 1, "{q}: {got}");
    }
}

#[test]
fn every_gate_has_words() {
    let r = Refusal::new(Gate::TooFar, "Delta E 7.0");
    assert_eq!(
        r.to_string(),
        "the look stays too far from the camera's JPEG (Delta E 7.0)"
    );
}

fn twisted_camera() -> Camera {
    Camera {
        twist: true,
        falloff: false,
        ..Camera::new()
    }
}

fn matrix_only_camera() -> Camera {
    Camera {
        falloff: false,
        ..Camera::new()
    }
}

/// The held-out mean of `look` with its table taken out.
fn held_out_without_table(case: &Case, look: &Look) -> f64 {
    let held = pairs(&case.inputs(), &split(192, 128), None).held;
    let bare = Look {
        table: None,
        ..look.clone()
    };
    score(&errors(
        |i| apply(&bare, case.sensor.rgb[i]),
        &case.preview,
        &held,
        false,
    ))
    .mean
}

#[test]
fn the_table_earns_its_place() {
    // A camera with a hue twist: the table is kept, and taking it out
    // costs more than the margin.
    let case = Case::new(&Scene::new(192, 128), twisted_camera());
    let look = case.fit().unwrap();
    assert_eq!(look.kind, Kind::Full);
    let table = look.table.clone().expect("a hue twist needs the table");
    let r = &look.report;
    let without = held_out_without_table(&case, &look);
    assert!(
        r.held_out.mean < without * (1.0 - t::KEEP_GAIN),
        "with {} without {without}",
        r.held_out.mean
    );
    // The zero-saturation row is the identity, and greys stay grey.
    for v in 0..VALUES {
        for h in 0..HUES {
            assert_eq!(table.node(v, h, 0), [0.0, 1.0, 1.0]);
        }
    }
    for g in [0.01, 0.18, 1.0, 2.0] {
        let o = apply(&look, [g; 3]);
        assert!(
            (o[0] - o[1]).abs() < 1e-12 && (o[1] - o[2]).abs() < 1e-12,
            "{g}: {o:?}"
        );
    }
    // A matrix-only camera: the table is dropped.
    let plain = Case::new(&Scene::new(192, 128), matrix_only_camera())
        .fit()
        .unwrap();
    assert_eq!(plain.table, None, "{:?}", plain.report);
}

#[test]
fn the_delta_e_refinement_lowers_the_held_out_error() {
    let case = Case::new(
        &Scene::new(192, 128),
        Camera {
            twist: true,
            ..Camera::new()
        },
    );
    let inputs = case.inputs();
    let p = pairs(&inputs, &split(192, 128), None);
    let pick = |v: &[[f64; 3]], idx: &[usize]| idx.iter().map(|&i| v[i]).collect::<Vec<_>>();
    let (x, y) = (
        pick(&case.sensor.rgb, &p.train),
        pick(&case.preview.rgb, &p.train),
    );
    let plain = fit_matrix(&x, &y, case.neutral).unwrap();
    let refined = refine_matrix(&plain, &x, &y, case.neutral).unwrap();
    assert_ne!(plain, refined);
    // The neutral is still kept.
    let n = mat_apply(&refined, case.neutral);
    for c in 0..3 {
        assert!((n[c] - case.neutral[c]).abs() < 1e-12, "{n:?}");
    }
    // Completed alike (tone and chroma curves), the refined matrix is
    // closer to the preview on the held-out pairs.
    let context = Context {
        inputs: &inputs,
        sensor_clip_render: 0.0,
        preview_clip: 0.0,
        colour: true,
    };
    let a = context.complete(&p, Kind::Full, plain, None).unwrap();
    let b = context.complete(&p, Kind::Full, refined, None).unwrap();
    assert!(
        b.held_out.mean < a.held_out.mean,
        "refined {} plain {}",
        b.held_out.mean,
        a.held_out.mean
    );
}

#[test]
fn a_near_black_frame_gets_a_tone_curve_only() {
    let scene = Scene {
        levels: (0.003, 0.02),
        ..Scene::new(192, 128)
    };
    let case = Case::new(&scene, Camera::new());
    let look = case.fit().unwrap();
    assert_eq!(look.kind, Kind::ToneOnly);
    assert_eq!(look.report.notes, vec![Gate::NearBlack]);
    assert_eq!(look.matrix, IDENTITY);
    assert_eq!(look.table, None);
    assert_eq!(look.chroma, None);
    assert!(look.report.held_out_pairs >= t::MIN_HELD_OUT * t::NEAR_BLACK_COUNTS);
    assert_eq!(
        Gate::NearBlack.words(),
        "the picture is nearly black: tone only"
    );
}

#[test]
fn two_runs_with_a_table_are_bit_identical() {
    let run = || {
        Case::new(&Scene::new(192, 128), twisted_camera())
            .fit()
            .unwrap()
    };
    let (one, two) = (run(), run());
    assert!(one.table.is_some());
    assert_eq!(
        serde_json::to_string(&one).unwrap(),
        serde_json::to_string(&two).unwrap()
    );
    let bits = |l: &Look| {
        l.matrix
            .iter()
            .flatten()
            .chain(l.table.iter().flat_map(|t| t.nodes.iter().flatten()))
            .map(|v| v.to_bits())
            .collect::<Vec<_>>()
    };
    assert_eq!(bits(&one), bits(&two));
}
