//! The node's calibration commands (`docs/design/calibration.md`,
//! section 8): `calibrate.chart` measures a colour chart shot and, asked
//! to `save`, keeps the result as a profile in the vault the node owns;
//! `calibrate.profiles.list`, `.get` and `.delete` read and remove them.
//! `helios-node`, its MCP tool and the `helios` CLI answer through here,
//! and so does the desktop, through its embedded node.
//!
//! A shot is a file (`path`) or an image of the vault (`image`: its id,
//! or a path the vault knows). A picture (TIFF, PNG, JPEG) is measured on
//! its decoded linear working RGB; a raw on its sensor data through the
//! raw front without `colorin` (`helios_calibrate::raw`). A raw the vault
//! knows is measured with the raw-front parameters of its newest edit,
//! so the profile matches what that edit's `colorin` receives; a raw the
//! vault does not know, or never edited, with the modules' defaults.
//!
//! A save or a removal answers its vault [`Changes`], so a surface that
//! keeps a history (the desktop's library history) undoes it as it
//! undoes an album's change.

use std::path::{Path, PathBuf};

use helios_calibrate::raw::RAW_FRONT;
use helios_calibrate::{
    ChartArgs, ChartError, Profile, ProfileSource, RawFront, SolveError, calibrate_picture,
    calibrate_raw_with,
};
use helios_core::Message;
use helios_vault::{Changes, ImageId, Store, Vault};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, Need, object};

/// Measures a chart shot, and saves its profile when asked.
pub const CHART: &str = "calibrate.chart";
/// Lists the saved profiles.
pub const LIST: &str = "calibrate.profiles.list";
/// Reads one profile.
pub const GET: &str = "calibrate.profiles.get";
/// Removes one profile.
pub const DELETE: &str = "calibrate.profiles.delete";

/// The commands on the vault's profiles (`calibrate.chart` aside: a node
/// without a vault answers its dry run).
pub const PROFILE_IDS: &[&str] = &[LIST, GET, DELETE];

/// Whether `id` is one of the commands run here.
pub fn handles(id: &str) -> bool {
    id == CHART || PROFILE_IDS.contains(&id)
}

fn string() -> Value {
    json!({ "type": "string" })
}

fn or_null(kind: &str) -> Value {
    json!({ "type": [kind, "null"] })
}

/// `calibrate.chart`'s params.
pub fn chart_params() -> Value {
    object(&[
        (
            "path",
            json!({ "type": ["string", "null"], "description": "an absolute path: the chart shot" }),
            false,
        ),
        (
            "image",
            json!({ "type": ["string", "null"],
                    "description": "the chart shot in the vault: its id, or a path the vault knows" }),
            false,
        ),
        (
            "corners",
            json!({ "type": "array", "minItems": 4, "maxItems": 4,
                    "items": { "type": "array", "minItems": 2, "maxItems": 2,
                               "items": { "type": "number" } },
                    "description": "the four corner patches' centres, [x, y] in pixels" }),
            true,
        ),
        ("chart", or_null("string"), false),
        (
            "exclude",
            json!({ "type": "array", "items": { "type": "string" } }),
            false,
        ),
        ("dry_run", json!({ "type": "boolean" }), false),
        (
            "save",
            json!({ "type": "boolean", "description": "keep the result as a profile in the vault" }),
            false,
        ),
        (
            "name",
            json!({ "type": ["string", "null"], "description": "the saved profile's name" }),
            false,
        ),
    ])
}

/// `calibrate.chart`'s spec.
pub fn chart_spec() -> CommandSpec {
    CommandSpec::new(CHART, "Measure a colour chart shot", "Calibration").params(chart_params())
}

/// The profile commands' specs.
pub fn profile_specs() -> Vec<CommandSpec> {
    let calibration =
        |id: &str, label: &str| CommandSpec::new(id, label, "Calibration").needs(Need::Vault);
    let profile = || object(&[("profile", string(), true)]);
    vec![
        calibration(LIST, "List the calibration profiles")
            .params(object(&[("camera", or_null("string"), false)]))
            .read_only(),
        calibration(GET, "Show a calibration profile")
            .params(profile())
            .read_only(),
        calibration(DELETE, "Delete a calibration profile").params(profile()),
    ]
}

/// Every command run here.
pub fn specs() -> Vec<CommandSpec> {
    std::iter::once(chart_spec())
        .chain(profile_specs())
        .collect()
}

/// A command's answer and what it changed in the vault.
#[derive(Debug, Default)]
pub struct Done {
    pub answer: Value,
    pub changes: Changes,
}

impl Done {
    fn read(answer: Value) -> Self {
        Self {
            answer,
            changes: Changes::default(),
        }
    }
}

fn needs_vault() -> Message {
    Message::new(
        "calibrate-needs-vault",
        "the node keeps no vault: saving and reading profiles need one (start it with --vault)",
    )
}

fn store_error(e: String) -> Message {
    Message::new("calibrate-vault-error", "the vault: { $error }").with("error", e)
}

/// Runs command `id` (params already checked against its schema) on
/// `vault` (none for a node without one), at `now` (seconds since the
/// epoch).
///
/// # Errors
/// The command refused, each reason keyed for translation.
pub fn run<S: Store>(
    vault: Option<&Vault<S>>,
    id: &str,
    params: &Value,
    now: u64,
) -> Result<Done, Message> {
    if id == CHART {
        return chart(vault, params, now);
    }
    let vault = vault.ok_or_else(needs_vault)?;
    let profile_id = params["profile"].as_str().unwrap_or_default();
    match id {
        LIST => {
            let camera = params["camera"].as_str().map(|c| {
                // A make and model, or a key already.
                helios_calibrate::camera_key("", c)
            });
            let profiles: Vec<Value> = vault
                .calibration_profiles()
                .map_err(store_error)?
                .into_iter()
                .filter(|(_, p)| camera.as_ref().is_none_or(|c| p["camera"]["key"] == **c))
                .map(|(id, p)| summary(&id, &p))
                .collect();
            Ok(Done::read(json!({ "profiles": profiles })))
        }
        GET => match vault.calibration_profile(profile_id).map_err(store_error)? {
            Some(p) => Ok(Done::read(json!({ "id": profile_id, "profile": p }))),
            None => Err(no_profile(profile_id)),
        },
        DELETE => {
            let Some(p) = vault.calibration_profile(profile_id).map_err(store_error)? else {
                return Err(no_profile(profile_id));
            };
            let changes = vault
                .set_calibration_profile(profile_id, None)
                .map_err(store_error)?;
            Ok(Done {
                answer: json!({ "deleted": summary(profile_id, &p) }),
                changes,
            })
        }
        other => {
            Err(Message::new("calibrate-unknown-command", "no command `{ $id }`").with("id", other))
        }
    }
}

fn no_profile(id: &str) -> Message {
    Message::new("calibrate-no-profile", "no calibration profile `{ $id }`").with("id", id)
}

/// A profile as listed: its id, name, camera, light and quality.
fn summary(id: &str, profile: &Value) -> Value {
    json!({
        "id": id,
        "name": profile["name"],
        "camera": profile["camera"],
        "illuminant": profile["illuminant"],
        "quality": profile["report"]["quality"],
        "created": profile["created"],
    })
}

/// Where the shot is and what the vault knows of it.
struct Shot {
    path: PathBuf,
    /// Its id, when the vault knows it.
    image: Option<ImageId>,
}

fn shot<S: Store>(vault: Option<&Vault<S>>, params: &Value) -> Result<Shot, Message> {
    let path = params["path"].as_str().map(PathBuf::from);
    let named = params["image"].as_str();
    match (named, path) {
        (Some(named), _) => {
            let vault = vault.ok_or_else(needs_vault)?;
            let id = match ImageId::from_hex(named) {
                Ok(id) => id,
                Err(_) => vault
                    .at_path(Path::new(named))
                    .map_err(store_error)?
                    .map(|(id, _)| id)
                    .ok_or_else(|| unknown_image(named))?,
            };
            let path = vault
                .locations(&id)
                .map_err(store_error)?
                .into_iter()
                .map(|l| l.path)
                .find(|p| p.exists())
                .ok_or_else(|| {
                    Message::new(
                        "calibrate-image-not-at-hand",
                        "no file of image { $image } is at hand",
                    )
                    .with("image", id.hex())
                })?;
            Ok(Shot {
                path,
                image: Some(id),
            })
        }
        (None, Some(path)) => {
            let image = match vault {
                Some(v) => v.at_path(&path).map_err(store_error)?.map(|(id, _)| id),
                None => None,
            };
            Ok(Shot { path, image })
        }
        (None, None) => Err(Message::new(
            "calibrate-no-shot",
            "calibrate.chart needs `path` or `image`, the chart shot",
        )),
    }
}

fn unknown_image(named: &str) -> Message {
    Message::new(
        "calibrate-unknown-image",
        "`{ $image }` is neither an image id nor a file the vault knows",
    )
    .with("image", named)
}

/// The raw-front parameters of the image's newest edit, by module id, and
/// that edit's version; nothing for an image never edited.
fn raw_front<S: Store>(
    vault: &Vault<S>,
    image: ImageId,
) -> Result<(RawFront, Option<String>), Message> {
    let (_, head) = helios_vault::EditSession::open(vault, image).map_err(store_error)?;
    let Some(head) = head else {
        return Ok((RawFront::new(), None));
    };
    let modules = helios_vault::edit::modules(&head.edit).map_err(store_error)?;
    let mut front = RawFront::new();
    for (name, (module, params)) in modules {
        let module = module.unwrap_or(name);
        if RAW_FRONT.contains(&module.as_str()) {
            front.entry(module).or_insert(params);
        }
    }
    Ok((front, Some(head.id.hex())))
}

/// `calibrate.chart`: the shot measured and solved; with `save`, the
/// result kept as a profile, its id answered.
///
/// # Errors
/// The shot cannot be found or read, the chart cannot be measured and
/// solved as asked, or a save without a vault.
pub fn chart<S: Store>(
    vault: Option<&Vault<S>>,
    params: &Value,
    now: u64,
) -> Result<Done, Message> {
    // A field given as null is a field left out.
    let mut given = params.clone();
    if let Some(fields) = given.as_object_mut() {
        fields.retain(|_, v| !v.is_null());
    }
    let mut args: ChartArgs = serde_json::from_value(given).map_err(|e| {
        Message::new("calibrate-bad-params", "calibrate.chart: { $error }")
            .with("error", e.to_string())
    })?;
    let save = params["save"].as_bool().unwrap_or(false);
    if !args.dry_run && !save {
        return Err(refusal(&ChartError::NotDryRun));
    }
    if save && vault.is_none() {
        return Err(needs_vault());
    }
    args.dry_run = true;
    let shot = shot(vault, params)?;
    let loaded = helios_load::load(&shot.path).map_err(|e| {
        Message::new(
            "calibrate-load-failed",
            "cannot read the chart shot: { $error }",
        )
        .with("error", e)
    })?;
    let region = loaded.image.region();
    let (front, edit) = match (vault, shot.image, &loaded.camera) {
        (Some(v), Some(id), Some(_)) => raw_front(v, id)?,
        _ => (RawFront::new(), None),
    };
    let mut answer = match &loaded.camera {
        Some(camera) => calibrate_raw_with(&args, loaded.image.clone(), camera, &front),
        None => calibrate_picture(
            &args,
            loaded.image.data(),
            region.width as usize,
            region.height as usize,
            loaded.image.channels() as usize,
        ),
    }
    .map_err(|e| refusal(&e))?;
    let mut changes = Changes::default();
    let mut saved = None;
    if let Some(vault) = vault.filter(|_| save) {
        answer.dry_run = false;
        let source = ProfileSource {
            camera: loaded.camera.as_ref(),
            image: &loaded.hash,
            edit: edit.clone(),
            excluded: &args.exclude,
        };
        let mut profile = Profile::new(&answer, &source, "", now);
        profile.name = match params["name"].as_str().map(str::trim) {
            Some(name) if !name.is_empty() => name.into(),
            _ => profile.default_name(),
        };
        let id = profile.id();
        let doc = serde_json::to_value(&profile).expect("a profile serialises");
        changes = vault
            .set_calibration_profile(&id, Some(&doc))
            .map_err(store_error)?;
        saved = Some(json!({ "id": id, "name": profile.name, "camera": profile.camera }));
    }
    let mut value = json!(answer);
    value["image"] = json!({
        "hash": loaded.hash,
        "width": region.width,
        "height": region.height,
        "raw": loaded.camera.is_some(),
        "edit": edit,
    });
    if let Some(saved) = saved {
        value["profile"] = saved;
    }
    Ok(Done {
        answer: value,
        changes,
    })
}

/// A refusal of the chart, keyed for translation.
pub fn refusal(e: &ChartError) -> Message {
    match e {
        ChartError::NotDryRun => Message::new(
            "calibrate-save-needed",
            "a run that writes needs `save`: the profile is what it writes",
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

#[cfg(test)]
mod tests;
