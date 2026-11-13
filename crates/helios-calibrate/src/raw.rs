//! A raw chart shot measured on its own sensor data (see
//! `docs/design/calibration.md`, section 4 and slice 4c).
//!
//! The calibration builds its own measurement graph, the raw front without
//! `colorin`:
//!
//! ```text
//! mosaic → rawprepare → whitebalance (camera reference) → highlights → demosaic
//! ```
//!
//! rendered at scale 1 on the bounding box of the chart only
//! ([`measured_region`]). `demosaic`'s output is white-balanced linear
//! camera RGB, the values `colorin` would receive; `rawprepare`'s output on
//! the same region is where clipping is read, at the level `highlights`
//! cuts ([`RAW_CLIP_LEVEL`]). The standard matrix `M_s` is `colorin`'s, from
//! the camera's matrix ([`camera_to_working`]). The modules take the
//! parameters of the image's edit when the caller gives them
//! ([`RawFront`], [`calibrate_raw_with`]), so the profile matches what the
//! edit's `colorin` receives; otherwise their defaults, white balance in
//! camera reference (D65) mode as Helios's develop network has it.
//!
//! The rendered region is cut from the image, so the handles are shifted
//! into it for the measurement and the answer's corners shifted back: the
//! caller sees the raw's own coordinates (the loader's crop, before any
//! geometry). Same inputs, same answer: the modules run on the CPU, each
//! pixel computed alone.

use std::collections::BTreeMap;
use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, Image, ModuleRef, NodeId, Params, Region, Value};
use helios_module_demosaic::Demosaic;
use helios_module_highlights::Highlights;
use helios_module_rawprepare::RawPrepare;
use helios_module_whitebalance::{CAMERA_REFERENCE, WhiteBalance};
use helios_photo::CameraInfo;
use helios_photo::color::{Mat3, camera_to_working};

use crate::chart::{Chart, chart_by_id};
use crate::layout::{ImageView, Layout, Point};
use crate::request::{ChartAnswer, ChartArgs, ChartError, calibrate_shot};
use crate::run::Shot;
use crate::sampler::{ClipSource, RAW_CLIP_LEVEL};
use crate::solver::Orientation;

/// Pixels added around the chart's bounding box: room for a handle placed
/// a little off and for the demosaicing's neighbourhood at the edge.
pub const MARGIN: i64 = 16;

/// What the measurement graph gives over the measured region.
#[derive(Clone, Debug)]
pub struct Measured {
    /// `demosaic`'s output: white-balanced linear camera RGB, four
    /// channels (the fourth unused).
    pub image: Image,
    /// `rawprepare`'s output: the mosaic with black at 0 and the sensor's
    /// white at 1, one channel.
    pub clip: Image,
    /// `M_s`, `colorin`'s standard matrix.
    pub standard: Mat3,
}

/// The region the chart covers in an image of `extent`: the bounds of
/// every patch's whole square under the eight ways the handles may have
/// been given, grown by [`MARGIN`], its edges on even pixels (a whole
/// Bayer block), cut to `extent`. `None` when the handles are degenerate
/// or the chart lies outside the image.
pub fn measured_region(
    chart: &'static Chart,
    handles: [Point; 4],
    extent: Region,
) -> Option<Region> {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let mut laid = false;
    for orientation in Orientation::ALL {
        let Some(layout) = Layout::new(chart, orientation.corners(handles)) else {
            continue;
        };
        laid = true;
        for [x, y] in (0..chart.len()).flat_map(|k| layout.quad(k, 1.0)) {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    if !(laid && x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        return None;
    }
    let even_down = |v: f64| (v.floor() as i64 - MARGIN).div_euclid(2) * 2;
    let even_up = |v: f64| (v.ceil() as i64 + MARGIN + 1).div_euclid(2) * 2;
    let (left, top) = (even_down(x0).max(extent.x), even_down(y0).max(extent.y));
    let (right, bottom) = (
        even_up(x1).min(extent.right()),
        even_up(y1).min(extent.bottom()),
    );
    (right > left && bottom > top)
        .then(|| Region::new(left, top, (right - left) as u32, (bottom - top) as u32))
}

/// The raw front's modules, in order, by module id.
pub const RAW_FRONT: [&str; 4] = ["rawprepare", "whitebalance", "highlights", "demosaic"];

/// Parameters for the raw front's modules, by module id ([`RAW_FRONT`]):
/// an edit's. A module not named keeps its defaults.
pub type RawFront = BTreeMap<String, Params>;

fn module_error(e: impl std::fmt::Display) -> ChartError {
    ChartError::RawGraph(e.to_string())
}

/// The measurement graph on `mosaic`: its source, `rawprepare` (the clip
/// source) and `demosaic` (the measured values).
fn graph(
    mosaic: Image,
    camera: &CameraInfo,
    front: &RawFront,
) -> Result<(Graph, NodeId, NodeId), ChartError> {
    let mut graph = Graph::new();
    let source = graph.add_source("raw", mosaic);
    let none = Params::new();
    let balance = Params::new().with("mode", Value::Choice(CAMERA_REFERENCE.into()));
    let given = |name: &str, default: &Params| front.get(name).unwrap_or(default).clone();
    let steps = [
        (
            "rawprepare",
            ModuleRef::new(RawPrepare),
            given("rawprepare", &none),
        ),
        (
            "whitebalance",
            ModuleRef::new(WhiteBalance),
            given("whitebalance", &balance),
        ),
        (
            "highlights",
            ModuleRef::new(Highlights),
            given("highlights", &none),
        ),
        (
            "demosaic",
            ModuleRef::new(Demosaic),
            given("demosaic", &none),
        ),
    ];
    let context = camera.to_context();
    let mut ids = Vec::with_capacity(steps.len());
    let (mut from, mut port) = (source, "image");
    for (name, module, params) in steps {
        let id = graph
            .add_module(name, module, &params)
            .map_err(module_error)?;
        graph
            .connect(from, port, id, "input")
            .map_err(module_error)?;
        graph
            .set_context(id, context.clone())
            .map_err(module_error)?;
        ids.push(id);
        (from, port) = (id, "output");
    }
    Ok((graph, ids[0], ids[3]))
}

/// Renders the measurement graph of `mosaic` (a raw's sensor data in
/// counts, one channel, the loader's region) over `region`, its modules
/// with `front`'s parameters.
///
/// # Errors
/// A camera without a usable matrix, or a module refusing the camera's
/// description or the parameters.
pub fn measure_raw(
    mosaic: Image,
    camera: &CameraInfo,
    region: Region,
    front: &RawFront,
) -> Result<Measured, ChartError> {
    let standard = camera_to_working(&camera.xyz_to_camera).ok_or(ChartError::NoCameraMatrix)?;
    let (graph, prepared, demosaiced) = graph(mosaic, camera, front)?;
    let engine = Engine::new(Arc::new(Bus::new()));
    let image = engine
        .render(&graph, demosaiced, "output", region)
        .map_err(module_error)?;
    let clip = engine
        .render(&graph, prepared, "output", region)
        .map_err(module_error)?;
    Ok(Measured {
        image,
        clip,
        standard,
    })
}

fn view(image: &Image) -> ImageView<'_> {
    let region = image.region();
    ImageView {
        data: image.data(),
        width: region.width as usize,
        height: region.height as usize,
        channels: image.channels() as usize,
    }
}

/// A raw chart shot's dry run: `mosaic` its sensor data (one channel, in
/// counts, over the loader's region), `camera` its description; the
/// corners in the mosaic's coordinates.
///
/// # Errors
/// As [`calibrate_shot`]; a chart outside the image; a camera without a
/// usable matrix or refused by the raw modules.
pub fn calibrate_raw(
    args: &ChartArgs,
    mosaic: Image,
    camera: &CameraInfo,
) -> Result<ChartAnswer, ChartError> {
    calibrate_raw_with(args, mosaic, camera, &RawFront::new())
}

/// [`calibrate_raw`] with the raw front's modules set to `front`'s
/// parameters (the image's edit), the others at their defaults.
///
/// # Errors
/// As [`calibrate_raw`].
pub fn calibrate_raw_with(
    args: &ChartArgs,
    mosaic: Image,
    camera: &CameraInfo,
    front: &RawFront,
) -> Result<ChartAnswer, ChartError> {
    if !args.dry_run {
        return Err(ChartError::NotDryRun);
    }
    let chart =
        chart_by_id(&args.chart).ok_or_else(|| ChartError::UnknownChart(args.chart.clone()))?;
    if mosaic.channels() != 1 {
        return Err(ChartError::BadImage);
    }
    if Layout::new(chart, args.corners).is_none() {
        // Three handles on a line, or two on one point: no chart to place.
        return Err(ChartError::Unsolved(None));
    }
    let region =
        measured_region(chart, args.corners, mosaic.region()).ok_or(ChartError::OutsideImage)?;
    let measured = measure_raw(mosaic, camera, region, front)?;
    let (dx, dy) = (region.x as f64, region.y as f64);
    let mut shifted = args.clone();
    shifted.corners = args.corners.map(|[x, y]| [x - dx, y - dy]);
    let shot = Shot {
        image: view(&measured.image),
        clip: Some(ClipSource {
            image: view(&measured.clip),
            level: RAW_CLIP_LEVEL,
        }),
        standard: measured.standard,
    };
    let mut answer = calibrate_shot(&shifted, &shot)?;
    answer.corners = answer.corners.map(|[x, y]| [x + dx, y + dy]);
    Ok(answer)
}

#[cfg(test)]
mod tests;
