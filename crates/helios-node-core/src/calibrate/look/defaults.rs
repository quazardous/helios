//! A camera's default camera look (`docs/design/camera-look.md`, "Built:
//! a selection and a default per camera"), as raw editors keep develop
//! defaults per camera model (Lightroom's "Set Default Settings" for a
//! camera): the look's two nodes' parameters kept in the vault's
//! `look/defaults` namespace under the camera's key, and put on the raws
//! of that camera an import brings in ([`default_on_import`]).
//!
//! - `look.camera.setDefault {camera, look | image | clear}`: keeps the
//!   look given (the two nodes' parameters), the look of an image's
//!   newest edit, or clears the default. One undoable step of the
//!   library; photos already edited keep their edits.

use std::collections::BTreeMap;
use std::path::Path;

use helios_calibrate::look::edit::{self, COLOUR, TONE};
use helios_core::{Message, Params, Value as Param};
use helios_vault::{Changes, ImageId, Store, Vault};
use serde_json::{Value, json};

use crate::calibrate::{Done, image_id, store_error};
use crate::commands::{CommandSpec, Need, object};

/// Keeps or clears a camera's default camera look.
pub const SET_DEFAULT: &str = "look.camera.setDefault";

/// The default document's format.
const FORMAT: u64 = 1;

/// `look.camera.setDefault`'s spec.
pub fn spec() -> CommandSpec {
    CommandSpec::new(
        SET_DEFAULT,
        "Keep or clear a camera's default camera look",
        "Calibration",
    )
    .needs(Need::Vault)
    .params(object(&[
        (
            "camera",
            json!({ "type": "string",
                    "description": "the camera's key, or its make and model" }),
            true,
        ),
        (
            "look",
            json!({ "type": "object",
                    "description": "the look's parameters: `cameracolour` and `cameratone`, each its node's" }),
            false,
        ),
        (
            "image",
            json!({ "type": "string",
                    "description": "an image id or a path the vault knows: its newest edit's look" }),
            false,
        ),
        (
            "clear",
            json!({ "type": "boolean", "description": "clear the camera's default" }),
            false,
        ),
    ]))
}

/// A default's document: the camera as said, the two nodes' parameters,
/// how many photos it was fitted from, when it was kept.
pub fn document(name: &str, colour: &Params, tone: &Params, photos: usize, now: u64) -> Value {
    json!({
        "format": FORMAT,
        "name": name,
        COLOUR: colour,
        TONE: tone,
        "photos": photos,
        "saved": now,
    })
}

/// A default's two nodes' parameters, read back; none for a document
/// without a fitted tone curve.
pub fn params_of(doc: &Value) -> Option<(Params, Params)> {
    let colour: Params = serde_json::from_value(doc[COLOUR].clone()).ok()?;
    let tone: Params = serde_json::from_value(doc[TONE].clone()).ok()?;
    fitted(&tone).then_some((colour, tone))
}

/// What a default's look was fitted to (its tone node's `preview`): the
/// open photo's look is the default when its own says the same.
pub fn preview_of(doc: &Value) -> Option<String> {
    let (_, tone) = params_of(doc)?;
    match tone.get("preview") {
        Some(Param::Text(t)) if !t.is_empty() => Some(t.clone()),
        _ => None,
    }
}

/// Whether a tone node's parameters hold a fitted curve.
fn fitted(tone: &Params) -> bool {
    matches!(tone.get("tone"), Some(Param::List(k)) if !k.is_empty())
}

fn not_fitted() -> Message {
    Message::new(
        "look-default-not-fitted",
        "no fitted camera look to keep: fit one to the camera's JPEG first",
    )
}

/// The look of an image's newest edit: its two nodes' parameters.
fn look_of_image<S: Store>(vault: &Vault<S>, id: &ImageId) -> Result<(Params, Params), Message> {
    let edit = helios_vault::EditSession::open(vault, *id)
        .map_err(store_error)?
        .1
        .map_or(Value::Null, |v| v.edit);
    let modules = helios_vault::edit::modules(&edit).map_err(store_error)?;
    let node = |name: &str| {
        modules
            .get(name)
            .map(|(_, p)| p.clone())
            .unwrap_or_default()
    };
    let (colour, tone) = (node(COLOUR), node(TONE));
    if !fitted(&tone) {
        return Err(not_fitted());
    }
    Ok((colour, tone))
}

/// `look.camera.setDefault`: the camera's default look kept (from `look`
/// or `image`) or cleared (`clear`), one undoable step.
///
/// # Errors
/// None or several of `look`, `image`, `clear`; a look not fitted; an
/// unknown image; the vault failing.
pub fn set_default<S: Store>(vault: &Vault<S>, params: &Value, now: u64) -> Result<Done, Message> {
    let named = params["camera"].as_str().unwrap_or_default();
    let camera = helios_calibrate::camera_key("", named);
    if camera.is_empty() {
        return Err(Message::new(
            "look-default-no-camera",
            "a camera's default look needs `camera`",
        ));
    }
    let clear = params["clear"].as_bool().unwrap_or(false);
    let given = [
        params["look"].is_object(),
        params["image"].is_string(),
        clear,
    ];
    if given.iter().filter(|g| **g).count() != 1 {
        return Err(Message::new(
            "look-default-what",
            "give one of `look`, `image` or `clear`",
        ));
    }
    let before = vault.look_default(&camera).map_err(store_error)?;
    let after = if clear {
        None
    } else {
        let (colour, tone) = if let Some(image) = params["image"].as_str() {
            look_of_image(vault, &image_id(vault, image)?)?
        } else {
            let read = |node: &str| {
                serde_json::from_value::<Params>(params["look"][node].clone()).map_err(|e| {
                    Message::new(
                        "look-default-bad-params",
                        "the look's { $node } parameters: { $error }",
                    )
                    .with("node", node)
                    .with("error", e.to_string())
                })
            };
            (read(COLOUR)?, read(TONE)?)
        };
        if !fitted(&tone) {
            return Err(not_fitted());
        }
        Some(document(named, &colour, &tone, 1, now))
    };
    let changes = vault
        .set_look_default(&camera, after.as_ref())
        .map_err(store_error)?;
    Ok(Done {
        answer: json!({
            "camera": camera,
            "had": before.is_some(),
            "kept": after.is_some(),
        }),
        changes,
    })
}

/// The origin of the version a camera's default look makes at import:
/// a photo whose versions all have it was never edited by hand, so its
/// first opening still puts the camera's default profile on it.
pub const IMPORT_ORIGIN: &str = "import:camera-look";

/// Whether versions of a photo are only the camera's default look put
/// at import (none at all included): a photo nobody edited yet.
pub fn untouched(versions: &[helios_vault::Version]) -> bool {
    versions.iter().all(|v| v.origin == IMPORT_ORIGIN)
}

/// The camera key of the raw at `path`, read from its header without
/// decoding the mosaic, as the open photo's is named; none for a picture
/// or a file that cannot be read.
pub fn raw_camera(path: &Path) -> Option<String> {
    helios_load::raw_camera(path)
        .ok()
        .flatten()
        .map(|(make, model)| helios_calibrate::camera_key(&make, &model))
}

/// The cameras' default looks put on the photos an import brought in, as
/// raw editors apply a camera's develop defaults at import (Lightroom):
/// each image new to the vault in `changes`, never edited, whose file is
/// a raw of a camera with a default look (`camera_of` names a file's
/// camera, none for a picture: [`raw_camera`]) gets the look as its
/// first version, by [`IMPORT_ORIGIN`]. The import's own change of each
/// photo takes the version in, so undoing the import takes it back too.
/// Answers how many photos got a look.
///
/// # Errors
/// The vault cannot be read or written.
pub fn default_on_import<S: Store>(
    vault: &Vault<S>,
    changes: &mut Changes,
    now: u64,
    camera_of: &dyn Fn(&Path) -> Option<String>,
) -> Result<usize, Message> {
    let looks: BTreeMap<String, (Params, Params)> = vault
        .look_defaults()
        .map_err(store_error)?
        .into_iter()
        .filter_map(|(camera, doc)| params_of(&doc).map(|p| (camera, p)))
        .collect();
    if looks.is_empty() {
        return Ok(0);
    }
    let mut put = 0;
    for change in &mut changes.images {
        if change.before.is_some() || change.after.is_none() {
            continue;
        }
        let image = change.image;
        if !vault.versions(&image).map_err(store_error)?.is_empty() {
            continue;
        }
        let Some((colour, tone)) = vault
            .file_to_read(&image)
            .map_err(store_error)?
            .and_then(|l| camera_of(&l.path))
            .and_then(|camera| looks.get(&camera))
        else {
            continue;
        };
        if save_look(vault, image, colour, tone, IMPORT_ORIGIN, now)? {
            change.after = vault.rows(&image).map_err(store_error)?;
            put += 1;
        }
    }
    Ok(put)
}

/// A look's two nodes' parameters saved as a new version of `image`'s
/// edit by `origin`; false when the edit held them already.
fn save_look<S: Store>(
    vault: &Vault<S>,
    image: ImageId,
    colour: &Params,
    tone: &Params,
    origin: &str,
    now: u64,
) -> Result<bool, Message> {
    let (mut session, head) = helios_vault::EditSession::open(vault, image).map_err(store_error)?;
    let before = head.map_or(Value::Null, |v| v.edit);
    let after = edit::with_params(&before, colour, tone);
    let saved = session
        .save(
            vault,
            after,
            "Camera look: the camera's default",
            origin,
            now,
        )
        .map_err(store_error)?;
    Ok(saved.is_some())
}

/// The camera's default look, or none, as `look.camera.choose`'s
/// "Camera look" preset reads it: the two nodes' parameters.
///
/// # Errors
/// The vault cannot be read.
pub fn look_of_camera<S: Store>(
    vault: &Vault<S>,
    camera: &str,
) -> Result<Option<(Params, Params)>, Message> {
    Ok(vault
        .look_default(camera)
        .map_err(store_error)?
        .as_ref()
        .and_then(params_of))
}
