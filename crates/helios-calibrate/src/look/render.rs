//! The fit's inputs read from a raw and its embedded preview
//! (`docs/design/camera-look.md`, section 4.1): the raw rendered small
//! through the develop chain, the preview reduced to the same proxy.
//!
//! The sensor side is read where the look's colour half runs: after
//! colour calibration (`channelmixerrgb`, CAT16 from the as-shot light),
//! so the matrix and the hue/saturation table see in the chain what they
//! were fitted on. Exposure then multiplies by a gain, which the tone half
//! sees: [`for_chain`] moves the fitted tone curve's knots by that gain,
//! so the chain renders what the fit scored. Colour calibration's CAT16 is
//! no diagonal in working RGB, so a table fitted after it could not run
//! before it: that is why the colour half sits after it, not at
//! `colorin`'s output.
//!
//! Same inputs, same bits: the modules run on the CPU, each proxy pixel
//! reduced in a fixed order.

use std::collections::BTreeMap;
use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, Image, ModuleRef, NodeId, Params, Region, Value};
use helios_photo::CameraInfo;
use helios_photo::color::{REC2020_TO_XYZ, SRGB_TO_XYZ, apply as mat_apply, inverse, mul};

use super::{Look, LookInputs, PROXY, Proxy, Source, luma};
use crate::sampler::RAW_CLIP_LEVEL;

/// Parameters for the chain's modules, by module id: the edit's raw front
/// over the develop template's defaults.
pub type Chain = BTreeMap<String, Params>;

/// The chain's steps, in order: the raw front, the camera colours, colour
/// calibration, exposure and the default display transform.
pub const STEPS: [&str; 8] = [
    "rawprepare",
    "whitebalance",
    "highlights",
    "demosaic",
    "colorin",
    "channelmixerrgb",
    "exposure",
    "sigmoid",
];

/// The develop template's defaults for the chain's steps (white balance to
/// the camera's D65 reference, colour calibration from the camera's
/// illuminant, exposure +0.7 EV with the camera's bias compensated).
pub fn defaults() -> Chain {
    let mut chain = Chain::new();
    chain.insert(
        "whitebalance".into(),
        Params::new().with("mode", Value::Choice("camera_reference".into())),
    );
    chain.insert(
        "channelmixerrgb".into(),
        Params::new().with("illuminant", Value::Choice("camera".into())),
    );
    chain.insert(
        "exposure".into(),
        Params::new()
            .with("exposure", Value::Float(0.7))
            .with("black", Value::Float(-0.000_244_140_625))
            .with("compensate_bias", Value::Bool(true)),
    );
    chain
}

/// [`defaults`] with `edit`'s parameters of the raw front's modules and
/// `colorin` laid over them, by module id; the other steps keep the
/// defaults (section 4.1: the fit reads the as-shot render).
pub fn from_edit(edit: &BTreeMap<String, Params>) -> Chain {
    let mut chain = defaults();
    for module in &STEPS[..5] {
        if let Some(given) = edit.get(*module) {
            let entry = chain.entry((*module).to_string()).or_default();
            for (k, v) in given.iter() {
                entry.set(k, v.clone());
            }
        }
    }
    chain
}

fn module(id: &str) -> Option<ModuleRef> {
    Some(match id {
        "rawprepare" => ModuleRef::new(helios_module_rawprepare::RawPrepare),
        "whitebalance" => ModuleRef::new(helios_module_whitebalance::WhiteBalance),
        "highlights" => ModuleRef::new(helios_module_highlights::Highlights),
        "demosaic" => ModuleRef::new(helios_module_demosaic::Demosaic),
        "colorin" => ModuleRef::new(helios_module_colorin::ColorIn),
        "channelmixerrgb" => ModuleRef::new(helios_module_channelmixerrgb::ChannelMixerRgb),
        "exposure" => ModuleRef::new(helios_module_exposure::Exposure),
        "sigmoid" => ModuleRef::new(helios_module_sigmoid::Sigmoid),
        _ => return None,
    })
}

/// The chain on `mosaic`, each step's node by its module id.
fn graph(
    mosaic: &Image,
    camera: &CameraInfo,
    chain: &Chain,
) -> Result<(Graph, BTreeMap<&'static str, NodeId>), String> {
    let mut graph = Graph::new();
    let source = graph.add_source("raw", mosaic.clone());
    let context = camera.to_context();
    let none = Params::new();
    let mut ids = BTreeMap::new();
    let (mut from, mut port) = (source, "image");
    for step in STEPS {
        let m = module(step).ok_or_else(|| format!("no module `{step}`"))?;
        let params = chain.get(step).unwrap_or(&none);
        let id = graph
            .add_module(step, m, params)
            .map_err(|e| format!("`{step}`: {e}"))?;
        graph
            .connect(from, port, id, "input")
            .map_err(|e| format!("`{step}`: {e}"))?;
        graph
            .set_context(id, context.clone())
            .map_err(|e| format!("`{step}`: {e}"))?;
        ids.insert(step, id);
        (from, port) = (id, "output");
    }
    Ok((graph, ids))
}

/// The chain rendered at one scale.
pub struct Rendered {
    /// The raw's extent, full resolution.
    pub extent: Region,
    pub scale: f64,
    /// Colour calibration's output: what the colour half sees.
    pub colour: Image,
    /// Exposure's output: what the tone half sees.
    pub tone: Image,
    /// The default display transform's output: the baseline.
    pub display: Image,
    /// The raw front's levels, full resolution (for clipping); rendered
    /// only when asked.
    pub levels: Option<Image>,
}

/// Renders `mosaic` through `chain`, its long edge at most `size` pixels;
/// the levels at full resolution too when `levels`.
///
/// # Errors
/// A module refusing the camera or the parameters.
pub fn render(
    mosaic: &Image,
    camera: &CameraInfo,
    chain: &Chain,
    size: u32,
    levels: bool,
) -> Result<Rendered, String> {
    let (graph, ids) = graph(mosaic, camera, chain)?;
    let extent = graph
        .extent(ids["sigmoid"])
        .map_err(|e| e.to_string())?
        .ok_or("the raw has no extent")?;
    let long = extent.width.max(extent.height).max(1);
    let scale = (f64::from(size.max(1)) / f64::from(long)).min(1.0);
    let side = |n: u32| ((f64::from(n) * scale).floor() as u32).max(1);
    let at = |v: i64| (v as f64 * scale).floor() as i64;
    let region = Region::new(
        at(extent.x),
        at(extent.y),
        side(extent.width),
        side(extent.height),
    );
    let engine = Engine::new(Arc::new(Bus::new()));
    let out = |step: &str| {
        engine
            .render_at(&graph, ids[step], "output", region, scale)
            .map_err(|e| format!("`{step}`: {e}"))
    };
    let levels = if levels {
        Some(
            engine
                .render(&graph, ids["rawprepare"], "output", extent)
                .map_err(|e| format!("`rawprepare`: {e}"))?,
        )
    } else {
        None
    };
    Ok(Rendered {
        extent,
        scale,
        colour: out("channelmixerrgb")?,
        tone: out("exposure")?,
        display: out("sigmoid")?,
        levels,
    })
}

/// An image's pixels as a proxy (its first three channels).
pub fn proxy_of(image: &Image) -> Proxy {
    let r = image.region();
    let c = image.channels() as usize;
    let rgb = image
        .data()
        .chunks_exact(c)
        .map(|p| {
            let g = |i: usize| f64::from(p[i.min(c - 1)]);
            [g(0), g(1), g(2)]
        })
        .collect();
    Proxy::new(r.width as usize, r.height as usize, rgb)
}

/// What [`prepare`] reads: the fit's inputs, owned.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub sensor: Proxy,
    pub preview: Proxy,
    pub baseline: Proxy,
    pub clipped: Vec<bool>,
    pub preview_clipped: Vec<bool>,
    pub neutral: [f64; 3],
    pub source: Source,
    /// Exposure's gain from the colour half's input to the tone half's.
    pub gain: f64,
}

impl Prepared {
    /// The fit's inputs.
    pub fn inputs(&self) -> LookInputs<'_> {
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
}

/// The cells of a `full`-wide axis reduced to `proxy` cells: each cell's
/// first and past-last index.
fn cells(full: usize, proxy: usize) -> Vec<(usize, usize)> {
    (0..proxy)
        .map(|i| {
            let a = i * full / proxy;
            let b = ((i + 1) * full / proxy).max(a + 1).min(full);
            (a, b)
        })
        .collect()
}

/// A preview pixel is at 255 when a channel of its sRGB values reaches
/// this (linear).
const PREVIEW_WHITE: f64 = 0.995;

/// The preview (working space, linear, as the raw stores it) box-reduced
/// to `width × height`, and which proxy pixels hold a pixel at 255.
pub fn reduce_preview(preview: &Image, width: usize, height: usize) -> (Proxy, Vec<bool>) {
    let r = preview.region();
    let (pw, ph) = (r.width as usize, r.height as usize);
    let c = preview.channels() as usize;
    let data = preview.data();
    let to_srgb = mul(
        &inverse(&SRGB_TO_XYZ).expect("sRGB's matrix inverts"),
        &REC2020_TO_XYZ,
    );
    let xs = cells(pw, width);
    let ys = cells(ph, height);
    let mut rgb = Vec::with_capacity(width * height);
    let mut clipped = Vec::with_capacity(width * height);
    for &(y0, y1) in &ys {
        for &(x0, x1) in &xs {
            let mut sum = [0.0; 3];
            let mut white = false;
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = &data[(y * pw + x) * c..][..c];
                    let v = [0, 1, 2].map(|i| f64::from(p[i.min(c - 1)]));
                    for (s, v) in sum.iter_mut().zip(v) {
                        *s += v;
                    }
                    white |= mat_apply(&to_srgb, v).iter().any(|&s| s >= PREVIEW_WHITE);
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as f64;
            rgb.push(sum.map(|s| s / n));
            clipped.push(white);
        }
    }
    (Proxy::new(width, height, rgb), clipped)
}

/// Which proxy pixels hold a sensor sample at 0.99 of the clip level or
/// above, from the raw front's levels at full resolution.
fn sensor_clipped(levels: &Image, width: usize, height: usize) -> Vec<bool> {
    let r = levels.region();
    let (lw, lh) = (r.width as usize, r.height as usize);
    let c = levels.channels() as usize;
    let data = levels.data();
    let level = (RAW_CLIP_LEVEL * 0.99) as f32;
    let xs = cells(lw, width);
    let ys = cells(lh, height);
    let mut out = Vec::with_capacity(width * height);
    for &(y0, y1) in &ys {
        for &(x0, x1) in &xs {
            out.push((y0..y1).any(|y| {
                (x0..x1).any(|x| data[(y * lw + x) * c..][..c].iter().any(|&v| v >= level))
            }));
        }
    }
    out
}

/// The fit's inputs for a raw (`mosaic`, `camera`) and its embedded
/// `preview` (working space, linear, as the raw stores it), the raw
/// rendered through `chain` at [`PROXY`] pixels.
///
/// # Errors
/// A module refusing the camera or the parameters.
pub fn prepare(
    mosaic: &Image,
    camera: &CameraInfo,
    preview: &Image,
    chain: &Chain,
) -> Result<Prepared, String> {
    let rendered = render(mosaic, camera, chain, PROXY as u32, true)?;
    let sensor = proxy_of(&rendered.colour);
    let tone = proxy_of(&rendered.tone);
    let baseline = proxy_of(&rendered.display);
    let (w, h) = (sensor.width, sensor.height);
    let (preview_proxy, preview_clipped) = reduce_preview(preview, w, h);
    let clipped = rendered
        .levels
        .as_ref()
        .map_or_else(|| vec![false; w * h], |l| sensor_clipped(l, w, h));
    // The gain exposure applies, read off the two renders in a fixed
    // order (exposure is a gain and a black offset too small to matter).
    let (mut num, mut den) = (0.0, 0.0);
    for (a, b) in tone.rgb.iter().zip(&sensor.rgb) {
        let lb = luma(*b);
        if lb > 1e-4 {
            num += luma(*a);
            den += lb;
        }
    }
    let gain = if den > 0.0 { num / den } else { 1.0 };
    let pr = preview.region();
    Ok(Prepared {
        sensor,
        preview: preview_proxy,
        baseline,
        clipped,
        preview_clipped,
        // Colour calibration adapts the as-shot white to the working
        // space's: the neutral is equal RGB.
        neutral: [1.0; 3],
        source: Source {
            raw_size: [rendered.extent.width, rendered.extent.height],
            preview_size: [pr.width, pr.height],
            jpeg_quality: None,
            local_effect: None,
        },
        gain,
    })
}

/// `look` as the chain runs it: its tone curve's scene axis multiplied by
/// exposure's `gain`, the tone half reading exposure's output while the
/// fit read colour calibration's. The colour half is unchanged.
pub fn for_chain(look: &Look, gain: f64) -> Look {
    let mut look = look.clone();
    for k in &mut look.tone.knots {
        k[0] *= gain;
    }
    look
}
