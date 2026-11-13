//! The desktop's `calibrate.chart` command (`docs/design/calibration.md`,
//! section 8): a colour chart shot measured and its correction solved, a
//! dry run that writes nothing, as a node's `calibrate.chart` request
//! answers it.
//!
//! Arguments: `path` (a decoded picture: TIFF, PNG, JPEG, or a raw measured
//! on its sensor data), `corners` (the centres of the four corner patches,
//! `[x, y]` in the picture's pixels as stored or the raw's, any order),
//! `chart`, `exclude`, `dry_run` (true only).

use std::path::PathBuf;

use helios_calibrate::{ChartArgs, calibrate_picture, calibrate_raw};
use helios_shell::commands::CommandSpec;
use serde_json::{Value, json};

use crate::shell::Shell;

/// Measures a chart shot and answers the report.
pub const CHART_COMMAND: &str = "calibrate.chart";

/// The answer of [`CHART_COMMAND`] for `args`.
///
/// # Errors
/// Arguments that do not parse, a file that cannot be read, or a chart
/// that cannot be measured and solved as asked.
pub fn chart(args: &Value) -> Result<Value, String> {
    let path: PathBuf = args
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or("calibrate.chart needs `path`, the chart shot")?;
    let chart: ChartArgs =
        serde_json::from_value(args.clone()).map_err(|e| format!("calibrate.chart: {e}"))?;
    if !chart.dry_run {
        return Err(helios_calibrate::ChartError::NotDryRun.to_string());
    }
    let loaded = helios_load::load(&path)?;
    let region = loaded.image.region();
    let raw = loaded.camera.is_some();
    let answer = match &loaded.camera {
        Some(camera) => calibrate_raw(&chart, loaded.image.clone(), camera),
        None => calibrate_picture(
            &chart,
            loaded.image.data(),
            region.width as usize,
            region.height as usize,
            loaded.image.channels() as usize,
        ),
    }
    .map_err(|e| e.to_string())?;
    let mut value = json!(answer);
    value["image"] = json!({
        "hash": loaded.hash,
        "width": region.width,
        "height": region.height,
        "raw": raw,
    });
    Ok(value)
}

/// Registers [`CHART_COMMAND`] on the shell.
///
/// # Errors
/// The command cannot be registered.
pub fn register(shell: &mut Shell) -> Result<(), String> {
    shell.register(
        CommandSpec::new(
            CHART_COMMAND,
            "Measures a colour chart shot and solves its correction, writing nothing: the report per patch, the light and the matrix",
            "Developer",
        )
        .read_only(),
        |args, _| chart(args),
    )
}
