//! `calibrate.chart` on a node: the file read and measured, the dry run
//! answered (`docs/design/calibration.md`, section 8).
//!
//! A picture (TIFF, PNG, JPEG) is measured on its decoded linear working
//! RGB, the corners in its pixels as stored. A raw is measured on its
//! sensor data through the measurement graph the calibration builds (the
//! raw front without `colorin`, `helios_calibrate::raw`), the corners in
//! the raw's pixels (the loader's crop, before any turn).

use std::path::Path;

use helios_calibrate::{ChartArgs, ChartError, SolveError, calibrate_picture, calibrate_raw};
use helios_core::Message;
use serde_json::{Value, json};

/// Answers `calibrate.chart` for the file at `path`.
///
/// # Errors
/// The file cannot be read, or the chart cannot be measured and solved
/// as asked; each refusal keyed for translation.
pub fn chart(path: &Path, args: &ChartArgs) -> Result<Value, Message> {
    if !args.dry_run {
        return Err(refusal(&ChartError::NotDryRun));
    }
    let loaded = helios_load::load(path).map_err(|e| {
        Message::new(
            "calibrate-load-failed",
            "cannot read the chart shot: { $error }",
        )
        .with("error", e)
    })?;
    let region = loaded.image.region();
    let raw = loaded.camera.is_some();
    let answer = match &loaded.camera {
        Some(camera) => calibrate_raw(args, loaded.image.clone(), camera),
        None => calibrate_picture(
            args,
            loaded.image.data(),
            region.width as usize,
            region.height as usize,
            loaded.image.channels() as usize,
        ),
    }
    .map_err(|e| refusal(&e))?;
    let mut value = json!(answer);
    value["image"] = json!({
        "hash": loaded.hash,
        "width": region.width,
        "height": region.height,
        "raw": raw,
    });
    Ok(value)
}

/// A refusal of the dry run, keyed for translation.
pub fn refusal(e: &ChartError) -> Message {
    match e {
        ChartError::NotDryRun => Message::new(
            "calibrate-dry-run-only",
            "only the dry run is answered so far: saving a profile is not built yet",
        ),
        ChartError::UnknownChart(id) => {
            Message::new("calibrate-unknown-chart", "no chart `{ $chart }`")
                .with("chart", id.clone())
        }
        ChartError::UnknownPatch(name) => Message::new(
            "calibrate-unknown-patch",
            "the chart has no patch `{ $patch }`",
        )
        .with("patch", name.clone()),
        ChartError::BadImage => Message::new(
            "calibrate-bad-image",
            "the chart shot has no pixel or fewer than three channels",
        ),
        ChartError::Unsolved(Some(SolveError::NoNeutral)) => Message::new(
            "calibrate-no-neutral",
            "the chart could not be solved at these corners: no neutral patch is kept",
        ),
        ChartError::Unsolved(Some(SolveError::Underdetermined)) => Message::new(
            "calibrate-too-few-patches",
            "the chart could not be solved at these corners: too few patches are kept",
        ),
        ChartError::OutsideImage => Message::new(
            "calibrate-outside-image",
            "the chart's patches fall outside the image",
        ),
        ChartError::NoCameraMatrix => Message::new(
            "calibrate-no-camera-matrix",
            "the camera has no usable colour matrix to measure the raw against",
        ),
        ChartError::RawGraph(reason) => Message::new(
            "calibrate-raw-graph",
            "the raw could not be developed for measuring: { $error }",
        )
        .with("error", reason.clone()),
        ChartError::Unsolved(_) => Message::new(
            "calibrate-unsolved",
            "the chart could not be measured and solved at these corners",
        ),
    }
}
