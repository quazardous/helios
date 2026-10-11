//! The parametric mask against analytic ramps: one channel at a time,
//! feather edges, inversion, several channels together.

use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, Image, Module, ModuleRef, Params, Region, Value};
use helios_module_masks::{Channel, MaskParametric, Range, jzczhz};

/// A ramp of 101 pixels along x: `colour(t)` with `t = x / 100`.
fn ramp(colour: impl Fn(f32) -> [f32; 3]) -> Image {
    Image::from_fn(Region::sized(101, 1), 4, |x, _, c| {
        let rgb = colour(x as f32 / 100.0);
        if c == 3 { 1.0 } else { rgb[c as usize] }
    })
}

/// The mask of `params` over `image`, through a network.
fn mask(image: &Image, params: &Params) -> Vec<f32> {
    let mut g = Graph::new();
    let src = g.add_source("src", image.clone());
    let m = g
        .add_module("parametric", ModuleRef::new(MaskParametric), params)
        .unwrap();
    g.connect(src, "image", m, "input").unwrap();
    Engine::new(Arc::new(Bus::new()))
        .render(&g, m, "mask", image.region())
        .unwrap()
        .data()
        .to_vec()
}

fn range(name: &str, points: [f64; 4]) -> Params {
    Params::new().with(name, Value::List(points.to_vec()))
}

/// darktable's factor for a range `low..high`, feathers `below`, `above`
/// (non-zero here): 0 up to `low - below` included, 1 from `low` to `high`
/// included, 0 from `high + above` on.
fn expected(v: f32, low: f32, high: f32, below: f32, above: f32) -> f32 {
    if v <= low - below {
        0.0
    } else if v < low {
        (v - (low - below)) / below
    } else if v <= high {
        1.0
    } else if v < high + above {
        1.0 - (v - high) / above
    } else {
        0.0
    }
}

fn assert_close(got: &[f32], want: impl Fn(usize) -> f32, what: &str) {
    for (i, g) in got.iter().enumerate() {
        let w = want(i);
        assert!((g - w).abs() < 1e-4, "{what} at x = {i}: {g}, expected {w}");
    }
}

#[test]
fn each_rgb_channel_follows_its_own_ramp() {
    for (i, channel) in [Channel::Red, Channel::Green, Channel::Blue]
        .into_iter()
        .enumerate()
    {
        // Only this channel ramps: the others sit at 0.5, outside the range
        // the others would need to cut anything (they are open).
        let image = ramp(|t| {
            let mut rgb = [0.5; 3];
            rgb[i] = t;
            rgb
        });
        let got = mask(&image, &range(channel.name(), [0.4, 0.6, 0.2, 0.1]));
        assert_close(
            &got,
            |x| expected(x as f32 / 100.0, 0.4, 0.6, 0.2, 0.1),
            channel.name(),
        );
    }
}

#[test]
fn luminance_is_the_working_space_luminance() {
    // A grey's luminance is its value.
    let image = ramp(|t| [t; 3]);
    let got = mask(&image, &range("luminance", [0.3, 0.5, 0.1, 0.25]));
    assert_close(
        &got,
        |x| expected(x as f32 / 100.0, 0.3, 0.5, 0.1, 0.25),
        "luminance",
    );
    // A pure green of 1 has the green coefficient as its luminance.
    let green = Image::from_fn(Region::sized(1, 1), 4, |_, _, c| f32::from(c == 1));
    let got = mask(&green, &range("luminance", [0.6, 0.7, 0.0, 0.0]));
    assert_eq!(got, vec![1.0], "0.678 lies in 0.6..0.7");
}

#[test]
fn feather_edges_are_linear_and_hard_without_feather() {
    let image = ramp(|t| [t; 3]);
    // No feather: darktable's slope of 1000, a step one thousandth wide
    // that starts at `low` (0 there) and ends at `high` (1 there).
    let got = mask(&image, &range("red", [0.25, 0.75, 0.0, 0.0]));
    for (x, v) in got.iter().enumerate() {
        let want = if (26..=75).contains(&x) { 1.0 } else { 0.0 };
        assert_eq!(*v, want, "x = {x}");
    }
    // Feather below only: half way up at 0.15, full at 0.2.
    let got = mask(&image, &range("red", [0.2, 0.75, 0.1, 0.0]));
    assert!((got[15] - 0.5).abs() < 1e-5, "{}", got[15]);
    assert!((got[12] - 0.2).abs() < 1e-5, "{}", got[12]);
    assert_eq!(got[10], 0.0);
    assert_eq!(got[20], 1.0);
}

#[test]
fn open_ends_take_everything_past_them() {
    // Values beyond the span: 0 to 8 in steps of 0.08.
    let image = ramp(|t| [t * 8.0; 3]);
    // Low at 0 is open below, high at the span (4) is open above.
    let got = mask(&image, &range("red", [0.0, 1.0, 0.0, 0.5]));
    assert_eq!(got[0], 1.0);
    let got = mask(&image, &range("red", [2.0, 4.0, 1.0, 0.0]));
    assert_eq!(got[100], 1.0, "8 is past the open end at 4");
    assert_eq!(got[50], 1.0);
    assert!(
        (got[19] - 0.52).abs() < 1e-5,
        "1.52 on the feather: {}",
        got[19]
    );
    // The whole span open: the channel changes nothing.
    assert!(
        mask(&image, &range("red", [0.0, 4.0, 0.0, 0.0]))
            .iter()
            .all(|v| *v == 1.0)
    );
}

#[test]
fn a_channel_inverts_and_the_whole_mask_inverts() {
    let image = ramp(|t| [t; 3]);
    let params = range("blue", [0.4, 0.6, 0.2, 0.2]);
    let plain = mask(&image, &params);
    let inverted = mask(
        &image,
        &params.clone().with("blue_invert", Value::Bool(true)),
    );
    let whole = mask(&image, &params.with("invert", Value::Bool(true)));
    for x in 0..101 {
        assert!((inverted[x] - (1.0 - plain[x])).abs() < 1e-6);
        assert!((whole[x] - (1.0 - plain[x])).abs() < 1e-6);
    }
    // An inverted channel that selects everything selects nothing:
    // darktable's cancelling channel.
    let none = mask(
        &image,
        &Params::new().with("green_invert", Value::Bool(true)),
    );
    assert!(none.iter().all(|v| *v == 0.0));
}

#[test]
fn several_channels_multiply_and_invert_into_a_union() {
    // Red ramps up along x, green down.
    let image = ramp(|t| [t, 1.0 - t, 0.0]);
    let r = [0.2, 0.6, 0.2, 0.0];
    let g = [0.5, 0.9, 0.0, 0.3];
    let both = mask(
        &image,
        &range("red", r).with("green", Value::List(g.to_vec())),
    );
    let fr = |t: f32| expected(t, 0.2, 0.6, 0.2, 0.0);
    let fg = |t: f32| expected(1.0 - t, 0.5, 0.9, 0.0, 0.3);
    assert_close(
        &both,
        |x| fr(x as f32 / 100.0) * fg(x as f32 / 100.0),
        "product",
    );
    // Each channel inverted and the whole inverted: 1 - (1 - fr)(1 - fg),
    // darktable's inclusive combination.
    let union = mask(
        &image,
        &range("red", r)
            .with("green", Value::List(g.to_vec()))
            .with("red_invert", Value::Bool(true))
            .with("green_invert", Value::Bool(true))
            .with("invert", Value::Bool(true)),
    );
    assert_close(
        &union,
        |x| {
            let t = x as f32 / 100.0;
            1.0 - (1.0 - fr(t)) * (1.0 - fg(t))
        },
        "union",
    );
}

#[test]
fn jzczhz_matches_reference_values() {
    // Computed apart from this code, in double precision, from the JzAzBz
    // definition (Safdar et al. 2017) as darktable writes it, and the
    // Rec.2020 primaries: lightness and chroma x 100, hue in turns.
    let cases: [([f32; 3], [f32; 3]); 4] = [
        ([0.18, 0.18, 0.18], [0.586_344, 0.001_488, f32::NAN]),
        ([0.8, 0.1, 0.1], [0.983_028, 2.079_743, 0.076_405]),
        ([0.1, 0.6, 0.1], [0.872_803, 1.935_778, 0.412_878]),
        ([0.1, 0.1, 0.8], [0.601_900, 1.973_897, 0.742_469]),
    ];
    for (rgb, want) in cases {
        let got = jzczhz(&rgb);
        let got = [got[0] * 100.0, got[1] * 100.0, got[2]];
        for c in 0..3 {
            if want[c].is_nan() {
                continue; // a grey's hue is not defined
            }
            assert!(
                (got[c] - want[c]).abs() < 1e-4,
                "{rgb:?} channel {c}: {}, expected {}",
                got[c],
                want[c]
            );
        }
    }
}

#[test]
fn lightness_chroma_and_hue_select_what_they_say() {
    // A grey ramp: lightness grows with the value, chroma stays near 0.
    let grey = ramp(|t| [t; 3]);
    let lightness: Vec<f32> = grey.data().chunks(4).map(|p| jzczhz(p)[0]).collect();
    assert!(lightness.windows(2).all(|w| w[1] > w[0]));
    let dark = mask(&grey, &range("lightness", [0.0, 0.586, 0.0, 0.0]));
    assert!(dark[..18].iter().all(|v| *v == 1.0) && dark[19..].iter().all(|v| *v == 0.0));
    let chroma = mask(&grey, &range("chroma", [0.0, 0.05, 0.0, 0.0]));
    assert!(chroma.iter().all(|v| *v == 1.0), "a grey has no chroma");
    // Three saturated colours: a hue range around green takes green only.
    let colours = Image::from_fn(Region::sized(3, 1), 4, |x, _, c| match (x, c) {
        (_, 3) => 1.0,
        (0, 0) | (1, 1) | (2, 2) => 0.7,
        _ => 0.1,
    });
    let hue = mask(&colours, &range("hue", [0.35, 0.45, 0.05, 0.05]));
    assert_eq!(hue, vec![0.0, 1.0, 0.0]);
    let saturated = mask(&colours, &range("chroma", [1.0, 4.0, 0.5, 0.0]));
    assert_eq!(saturated, vec![1.0; 3]);
}

#[test]
fn the_image_passes_through_and_ranges_are_checked() {
    let image = ramp(|t| [t; 3]);
    let mut g = Graph::new();
    let src = g.add_source("src", image.clone());
    let m = g
        .add_module(
            "p",
            ModuleRef::new(MaskParametric),
            &range("red", [0.2, 0.4, 0.0, 0.0]),
        )
        .unwrap();
    g.connect(src, "image", m, "input").unwrap();
    let out = Engine::new(Arc::new(Bus::new()))
        .render(&g, m, "output", image.region())
        .unwrap();
    assert!(out.same_buffer(&image));
    // High below low is refused when the plan is made.
    let plan = MaskParametric.prepare(
        &MaskParametric
            .manifest()
            .resolve(&range("red", [0.6, 0.4, 0.0, 0.0]))
            .unwrap(),
        &helios_core::Context::new(),
    );
    assert!(plan.is_err());
    // An open range is skipped from the plan.
    let open = Range::new(0.0, 4.0, 0.0, 0.0, 4.0, false);
    assert!(open.is_open());
}

#[test]
fn the_parametric_mask_keeps_the_module_contract() {
    helios_testkit::assert_contract(MaskParametric);
}
