//! A parametric mask member: darktable's blendif in its scene-referred RGB
//! space (`src/develop/blends/blendif_rgb_jzczhz.c`), a range per channel,
//! the channels multiplied together.
//!
//! Whether the mask reads a module's input or its output is not a
//! parameter: it is which image is connected to `input`.

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamKind, ParamSpec, Params,
    PortSpec, Region, Role, Value,
};

use crate::{flag, luminance, mask_port};

/// A channel the mask can select on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Luminance of the working space (linear Rec.2020), as [`luminance`].
    Luminance,
    Red,
    Green,
    Blue,
    /// Lightness of `JzCzhz`, `Jz` × 100 (darktable's default boost).
    Lightness,
    /// Chroma of `JzCzhz`, `Cz` × 100 (darktable's default boost).
    Chroma,
    /// Hue of `JzCzhz`, in turns: 0 to 1.
    Hue,
}

impl Channel {
    pub const ALL: [Channel; 7] = [
        Channel::Luminance,
        Channel::Red,
        Channel::Green,
        Channel::Blue,
        Channel::Lightness,
        Channel::Chroma,
        Channel::Hue,
    ];

    /// The name of its range parameter; its inversion is `<name>_invert`.
    pub fn name(self) -> &'static str {
        match self {
            Channel::Luminance => "luminance",
            Channel::Red => "red",
            Channel::Green => "green",
            Channel::Blue => "blue",
            Channel::Lightness => "lightness",
            Channel::Chroma => "chroma",
            Channel::Hue => "hue",
        }
    }

    /// The span a range is given in. An end of a range at the span's end is
    /// open: it takes every value past it, as darktable's sliders do at 0
    /// and 1.
    pub fn span(self) -> f32 {
        match self {
            Channel::Hue => 1.0,
            _ => SPAN,
        }
    }

    fn perceptual(self) -> bool {
        matches!(self, Channel::Lightness | Channel::Chroma | Channel::Hue)
    }

    fn description(self) -> &'static str {
        match self {
            Channel::Luminance => "luminance range: low, high, feather below, feather above",
            Channel::Red => "red range: low, high, feather below, feather above",
            Channel::Green => "green range: low, high, feather below, feather above",
            Channel::Blue => "blue range: low, high, feather below, feather above",
            Channel::Lightness => {
                "lightness range (Jz x 100): low, high, feather below, feather above"
            }
            Channel::Chroma => "chroma range (Cz x 100): low, high, feather below, feather above",
            Channel::Hue => "hue range, in turns: low, high, feather below, feather above",
        }
    }
}

/// The upper end of the span of every channel but hue: two stops above 1,
/// room for scene-referred highlights.
const SPAN: f32 = 4.0;

/// One channel's range, as darktable's four points: the mask rises from 0
/// at `points[0]` to 1 at `points[1]`, stays 1 up to `points[2]` and falls
/// back to 0 at `points[3]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub points: [f32; 4],
    /// `1 / (points[1] - points[0])`, at most 1000 (darktable's slopes).
    pub rise: f32,
    /// `1 / (points[3] - points[2])`, at most 1000.
    pub fall: f32,
    pub invert: bool,
}

impl Range {
    /// From a range `low`..`high` with a feather of `below` and `above`
    /// outside it, over a span `0..span`; an end at the span's end is open
    /// (darktable's `dt_develop_blendif_process_parameters`).
    pub fn new(low: f32, high: f32, below: f32, above: f32, span: f32, invert: bool) -> Range {
        let mut points = [low - below, low, high, high + above];
        let rise = 1.0 / (points[1] - points[0]).max(0.001);
        let fall = 1.0 / (points[3] - points[2]).max(0.001);
        if low <= 0.0 {
            points[0] = f32::MIN;
            points[1] = f32::MIN;
        }
        if high >= span {
            points[2] = f32::MAX;
            points[3] = f32::MAX;
        }
        Range {
            points,
            rise,
            fall,
            invert,
        }
    }

    /// Selects every value: the channel changes nothing unless inverted.
    pub fn is_open(&self) -> bool {
        self.points[1] == f32::MIN && self.points[2] == f32::MAX
    }

    /// darktable's `_blendif_compute_factor`.
    pub fn factor(&self, v: f32) -> f32 {
        let p = &self.points;
        let f = if v <= p[0] {
            0.0
        } else if v < p[1] {
            (v - p[0]) * self.rise
        } else if v <= p[2] {
            1.0
        } else if v < p[3] {
            1.0 - (v - p[2]) * self.fall
        } else {
            0.0
        };
        if self.invert { 1.0 - f } else { f }
    }
}

/// Linear Rec.2020 RGB to CIE XYZ, D65 white.
const REC2020_TO_XYZ: [[f64; 3]; 3] = [
    [0.636_958_048_3, 0.144_616_903_6, 0.168_880_975_2],
    [0.262_700_212_0, 0.677_998_071_5, 0.059_301_716_5],
    [0.0, 0.028_072_693_0, 1.060_985_057_7],
];

/// darktable's `JzCzhz` of a linear Rec.2020 colour (`dt_XYZ_2_JzAzBz`,
/// `dt_JzAzBz_2_JzCzhz`), unscaled: `Jz`, `Cz`, and the hue in turns, 0
/// to 1. Computed in double precision, where darktable rearranges its
/// single-precision code to keep the near-neutral hues it would lose.
pub fn jzczhz(rgb: &[f32]) -> [f32; 3] {
    let (b, g) = (1.15, 0.66);
    let (c1, c2, c3) = (0.835_937_5, 18.851_562_5, 18.687_5);
    let (n, p, d, d0) = (
        0.159_301_758,
        134.034_375,
        -0.56,
        1.629_549_953_282_156_6e-11,
    );
    let rgb = [f64::from(rgb[0]), f64::from(rgb[1]), f64::from(rgb[2])];
    let xyz = REC2020_TO_XYZ.map(|row| row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2]);
    let x = b * xyz[0] - (b - 1.0) * xyz[2];
    let y = g * xyz[1] - (g - 1.0) * xyz[0];
    let z = xyz[2];
    let lms = [
        0.414_789_72 * x + 0.579_999 * y + 0.014_648 * z,
        -0.201_51 * x + 1.120_649 * y + 0.053_100_8 * z,
        -0.016_600_8 * x + 0.264_8 * y + 0.668_479_9 * z,
    ];
    let pq = lms.map(|v| {
        let t = (v / 10000.0).max(0.0).powf(n);
        ((c1 + c2 * t) / (1.0 + c3 * t)).powf(p)
    });
    let iz = 0.5 * pq[0] + 0.5 * pq[1];
    let az = 3.524 * pq[0] - 4.066_708 * pq[1] + 0.542_708 * pq[2];
    let bz = 0.199_076 * pq[0] + 1.096_799 * pq[1] - 1.295_875 * pq[2];
    let jz = (((1.0 + d) * iz) / (1.0 + d * iz) - d0).max(0.0);
    let h = bz.atan2(az) / std::f64::consts::TAU;
    [
        jz as f32,
        az.hypot(bz) as f32,
        if h >= 0.0 { h as f32 } else { (1.0 + h) as f32 },
    ]
}

/// A mask from ranges of the image's channels: luminance, red, green, blue,
/// and the lightness, chroma and hue of `JzCzhz` (darktable's blendif in
/// scene-referred RGB). Each channel has a range (low, high, a feather
/// below and above) and an inversion; the channels multiply, then the
/// whole may be inverted. A channel whose range is open at both ends
/// selects everything and is skipped (inverted, it selects nothing).
///
/// darktable's "inclusive" combination (a union of channels) is the
/// inversion of every channel and of the whole: `1 - Π(1 - f)`.
///
/// Its `output` is its input unchanged, as [`crate::MaskFromLuma`].
pub struct MaskParametric;

#[derive(Clone, Debug, PartialEq)]
pub struct ParametricPlan {
    /// The channels that change the mask, with their range.
    pub channels: Vec<(Channel, Range)>,
    pub invert: bool,
}

impl Module for MaskParametric {
    type Plan = ParametricPlan;

    fn manifest(&self) -> Manifest {
        let mut params = Vec::new();
        for channel in Channel::ALL {
            let span = f64::from(channel.span());
            params.push(ParamSpec {
                name: channel.name().into(),
                description: channel.description().into(),
                kind: ParamKind::Floats {
                    count: 4,
                    min: 0.0,
                    max: span,
                    default: vec![0.0, span, 0.0, 0.0],
                },
            });
            params.push(ParamSpec {
                name: format!("{}_invert", channel.name()),
                description: "invert this channel's selection".into(),
                kind: ParamKind::Bool { default: false },
            });
        }
        params.push(ParamSpec {
            name: "invert".into(),
            description: "invert the combined mask".into(),
            kind: ParamKind::Bool { default: false },
        });
        Manifest {
            name: "mask_parametric".into(),
            version: 1,
            description: "A mask from ranges of luminance, RGB and perceptual lightness, chroma and hue; passes the image through".into(),
            params,
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4), mask_port("mask")],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Mask,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<ParametricPlan, String> {
        let mut channels = Vec::new();
        for channel in Channel::ALL {
            let span = channel.span();
            let v = params
                .get(channel.name())
                .and_then(Value::as_list)
                .unwrap_or(&[]);
            let at = |i: usize, default: f32| v.get(i).map_or(default, |x| *x as f32);
            let (low, high) = (at(0, 0.0), at(1, span));
            if high < low {
                return Err(format!(
                    "{}: high {high} is below low {low}",
                    channel.name()
                ));
            }
            let invert = flag(params, &format!("{}_invert", channel.name()));
            let range = Range::new(low, high, at(2, 0.0), at(3, 0.0), span, invert);
            if !range.is_open() || invert {
                channels.push((channel, range));
            }
        }
        Ok(ParametricPlan {
            channels,
            invert: flag(params, "invert"),
        })
    }

    fn process(
        &self,
        plan: &ParametricPlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        let perceptual = plan.channels.iter().any(|(c, _)| c.perceptual());
        let mask = Image::from_fn(output, 1, |x, y, _| {
            let px = input.pixel(x, y);
            let jch = if perceptual { jzczhz(px) } else { [0.0; 3] };
            let mut m = 1.0;
            for (channel, range) in &plan.channels {
                let v = match channel {
                    Channel::Luminance => luminance(px),
                    Channel::Red => px[0],
                    Channel::Green => px[1],
                    Channel::Blue => px[2],
                    Channel::Lightness => jch[0] * 100.0,
                    Channel::Chroma => jch[1] * 100.0,
                    Channel::Hue => jch[2],
                };
                m *= range.factor(v);
            }
            if plan.invert { 1.0 - m } else { m }
        });
        Ok(vec![input.sample(output), mask])
    }
}
