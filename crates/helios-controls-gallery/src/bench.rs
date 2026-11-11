//! The gallery's bench: `egui_hud`'s bench (`egui_hud::bench`) over Helios's
//! catalogue, with the scenarios' sample (`docs/design/control-contexts.md`,
//! section 5) as its first preset.

use std::collections::BTreeMap;

pub use egui_hud::bench::*;
use egui_hud::context::{Rule, Tier};
use egui_hud::value::{ParamDescriptor, ParamType, Value};
use helios_controls::HeliosRegistry as _;
use helios_controls::catalogue::{BLACK_LEVEL, EXPOSURE};

/// The shelf group of the develop card's sample parameters.
pub const SAMPLE_DEVELOP: &str = "Sample: develop basics";
/// The shelf group of the gradient mask's sample parameters.
pub const SAMPLE_MASK: &str = "Sample: gradient mask";
/// The scenarios' bench, the kit's first preset.
pub const SAMPLE: &str = "Sample";

/// The tools the context bar offers (`ctl.tool`), none first.
pub const TOOLS: [&str; 4] = ["", "gradient", "brush", "crop"];

/// The sample parameters of the scenarios' bench: a develop card's black
/// level and contrast, and a gradient mask's feather, angle and profile;
/// each with its shelf group.
fn sample_parameters() -> Vec<ShelfParam> {
    let mut black = ParamDescriptor::float("Black level", -0.1, 0.1, 0.0);
    black.step = 0.001;
    black.coarse = 0.01;
    black.precision = 3;
    let contrast = ParamDescriptor::float("Contrast", -1.0, 1.0, 0.0);
    let mut feather = ParamDescriptor::float("Feather", 0.0, 200.0, 20.0);
    feather.unit = Some("px".into());
    feather.step = 1.0;
    feather.coarse = 10.0;
    feather.precision = 0;
    let mut angle = ParamDescriptor::float("Angle", -180.0, 180.0, 0.0);
    angle.unit = Some("°".into());
    angle.step = 1.0;
    angle.coarse = 15.0;
    angle.precision = 0;
    let mut profile =
        ParamDescriptor::new(ParamType::Choice, "Profile", Value::Choice("Linear".into()));
    profile.choices = vec!["Linear".into(), "Smooth".into(), "Sigmoid".into()];
    [
        ("black", SAMPLE_DEVELOP, black),
        ("contrast", SAMPLE_DEVELOP, contrast),
        ("feather", SAMPLE_MASK, feather),
        ("angle", SAMPLE_MASK, angle),
        ("profile", SAMPLE_MASK, profile),
    ]
    .into_iter()
    .map(|(name, group, descriptor)| ShelfParam {
        name: name.into(),
        group: group.into(),
        hint: "Sample parameter of the scenarios".into(),
        descriptor,
    })
    .collect()
}

/// The scenarios' bench (`control-contexts.md`, section 5): exposure,
/// black level and contrast, then a "Gradient mask" case of feather,
/// angle and profile. Exposure and feather are essential, the profile
/// expert; contrast and black level fold while retouching; the black
/// level does not apply to a picture (it is raw only); the gradient tool
/// emphasises the mask's parts.
fn sample() -> Preset {
    let piece = |key: &str, tier| Piece {
        tier,
        ..Piece::new(key, &format!("implicit:shelf/{key}"))
    };
    Preset {
        name: SAMPLE.into(),
        help: "The scenarios' bench: develop basics and a gradient mask".into(),
        composition: Composition {
            entries: vec![
                Entry::Control(Piece {
                    tier: Tier::Essential,
                    ..Piece::new("exposure", EXPOSURE)
                }),
                Entry::Control(piece("black", Tier::Standard)),
                Entry::Control(piece("contrast", Tier::Standard)),
                Entry::Case(BenchCase {
                    key: "gradient".into(),
                    legend: "Gradient mask".into(),
                    help: None,
                    separator: true,
                    parts: vec![
                        piece("feather", Tier::Essential),
                        piece("angle", Tier::Standard),
                        piece("profile", Tier::Expert),
                    ],
                }),
            ],
            rules: vec![
                Rule::new("ctl.task == 'retouch'")
                    .because("Folded while retouching")
                    .fold(&["black", "contrast"]),
                Rule::new("!ctl.image.raw")
                    .because("Raw only")
                    .inapplicable(&["black"]),
                Rule::new("ctl.tool == 'gradient'").emphasise(&["feather", "angle"]),
            ],
        },
        context: None,
        mode: None,
        width: None,
        settings: BTreeMap::new(),
    }
}

/// Helios's kit: the catalogue (its tone controls under "Tone"), the
/// scenarios' sample parameters and preset, the generic controls by
/// value type, and the photo tools.
pub fn kit() -> Kit {
    Kit {
        registry: egui_hud::Registry::standard(),
        groups: [EXPOSURE, BLACK_LEVEL]
            .into_iter()
            .map(|id| (id.to_owned(), "Tone".to_owned()))
            .collect(),
        parameters: sample_parameters(),
        implicit: true,
        presets: vec![sample()],
        tools: TOOLS.iter().map(|t| (*t).to_owned()).collect(),
    }
}

/// An empty bench over Helios's kit.
pub fn standard() -> Bench {
    Bench::new(kit())
}
