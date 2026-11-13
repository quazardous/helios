//! "Fit a camera look from the selection" (`library.fitCameraLook`,
//! `docs/design/camera-look.md`): the Library's selection handed to the
//! node's `look.camera.fitSelection`, which refuses photos of several
//! cameras at once and runs the fit as a job.

use egui_pop::api::host::Reply;
use egui_pop::model::ids::CommandId;
use helios_shell::commands::Origin;
use serde_json::Value;

use super::Shell;

impl Shell {
    /// `library.fitCameraLook`: `images` when given, else the Library's
    /// selection (the photo under the cursor without one), fitted
    /// together as one camera look; `default` keeps it as the camera's
    /// default too. Answers the job started.
    pub(super) fn fit_camera_look(
        &mut self,
        args: &Value,
        origin: Origin,
    ) -> Result<Value, String> {
        let images: Vec<Value> = match args.get("images").and_then(Value::as_array) {
            Some(ids) => ids.clone(),
            None => self
                .focus
                .library
                .borrow()
                .targets()
                .map_err(|_| "no photo selected: select photos of one camera in the Library")?
                .into_iter()
                .map(Value::String)
                .collect(),
        };
        let mut ask = serde_json::json!({ "images": images });
        if let Some(default) = args.get("default") {
            ask["default"] = default.clone();
        }
        // Run here rather than through `Shell::run`: a refusal (photos of
        // several cameras) is told once, by the queue that applies this.
        let id = CommandId::new(crate::calibrate::LOOK_FIT_SELECTION).map_err(|e| e.to_string())?;
        match self.core.app.run(&id, ask, crate::host::to_pop(&origin)) {
            Reply::Done(r) => r,
            Reply::Later(_) => Ok(serde_json::json!({ "queued": true })),
        }
    }
}
