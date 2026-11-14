//! `look.camera.fit` with `together`: one camera look from several photos
//! of one camera (`docs/design/camera-look.md`, "Built: a selection and a
//! default per camera").
//!
//! The photos must be of one camera: the library's capture fields are
//! read first, so a mixed selection is refused before any raw is read,
//! and each raw's own camera is checked again as it is read. Each raw is
//! fitted on its own, with its gates; the looks that passed are averaged
//! into one and scored on every photo's held-out pairs together
//! (`helios_calibrate::look::combine`). Without `dry_run`, that look is
//! written into every raw of the selection (a photo whose own fit was
//! refused gets the camera's look too), one undoable step of the
//! library; with `default`, it is kept as the camera's default in the
//! same step.

use std::collections::BTreeSet;
use std::path::Path;

use helios_calibrate::look::combine::{self, Fitted};
use helios_calibrate::look::edit;
use helios_core::Message;
use helios_vault::{Changes, ImageId, Store, Vault};
use serde_json::{Value, json};

use super::{Fit, Shot, Step, cancelled, defaults, fit_shot, refused, report};
use crate::calibrate::{Done, at_hand, store_error};

/// A fit of several photos together.
pub struct Ask<'a> {
    pub ids: Vec<ImageId>,
    pub dry_run: bool,
    /// Keep the look as the camera's default too.
    pub default: bool,
    pub origin: &'a str,
    pub now: u64,
}

/// The camera keys the library's capture fields give `ids` (make and
/// model as the files' Exif says them); photos without them are left
/// out.
///
/// # Errors
/// The vault cannot be read.
pub fn cameras<S: Store>(vault: &Vault<S>, ids: &[ImageId]) -> Result<BTreeSet<String>, Message> {
    use helios_vault::capture::{CAMERA_MAKE, CAMERA_MODEL};
    let fields = vault
        .fields_of(ids, &[CAMERA_MAKE, CAMERA_MODEL])
        .map_err(store_error)?;
    Ok(fields
        .iter()
        .filter_map(|f| {
            let text = |k: &str| f.get(k).and_then(Value::as_str).unwrap_or_default();
            let (make, model) = (text(CAMERA_MAKE), text(CAMERA_MODEL));
            (!model.is_empty()).then(|| helios_calibrate::camera_key(make, model))
        })
        .collect())
}

/// Refuses photos of several cameras, naming them.
///
/// # Errors
/// More than one camera.
pub fn one_camera(cameras: &BTreeSet<String>) -> Result<(), Message> {
    if cameras.len() <= 1 {
        return Ok(());
    }
    Err(mixed(cameras))
}

/// Photos of several cameras, refused in words.
fn mixed(cameras: &BTreeSet<String>) -> Message {
    Message::new(
        "look-mixed-cameras",
        "the photos are of { $count } cameras ({ $cameras }): a camera look is fitted from photos of one camera; select one camera's photos",
    )
    .with("count", cameras.len())
    .with("cameras", cameras.iter().cloned().collect::<Vec<_>>().join(", "))
}

/// The look of the photos `ask` names, fitted together, answered and,
/// without `dry_run`, written.
///
/// # Errors
/// Photos of several cameras, an image not at hand, a file that cannot
/// be read or rendered, the fit cancelled.
pub fn fit<S: Store>(
    vault: &Vault<S>,
    ask: &Ask<'_>,
    shot_of: &dyn Fn(&Path) -> Result<Option<Shot>, Message>,
    step: Step<'_>,
) -> Result<Done, Message> {
    one_camera(&cameras(vault, &ask.ids)?)?;
    let total = ask.ids.len();
    let mut answers = Vec::new();
    let mut fits: Vec<Fit> = Vec::new();
    let mut raws: Vec<ImageId> = Vec::new();
    // The camera's key, and its make and model as said.
    let mut camera: Option<(String, String)> = None;
    for (done, id) in ask.ids.iter().enumerate() {
        if !step(done, total) {
            return Err(cancelled());
        }
        let path = at_hand(vault, id)?;
        let edit = helios_vault::EditSession::open(vault, *id)
            .map_err(store_error)?
            .1
            .map_or(Value::Null, |v| v.edit);
        let mut answer = json!({ "image": id.hex() });
        let Some(shot) = shot_of(&path)? else {
            answer["refused"] = json!({
                "gate": "not-a-raw",
                "words": "the photo is not a raw: it has no sensor data to fit",
                "detail": "",
            });
            answers.push(answer);
            continue;
        };
        let key = helios_calibrate::camera_key(&shot.camera.maker, &shot.camera.model);
        match &camera {
            Some((first, _)) if *first != key => {
                return Err(mixed(&[first.clone(), key].into()));
            }
            Some(_) => {}
            None => {
                let name = format!("{} {}", shot.camera.maker, shot.camera.model);
                camera = Some((key, name.trim().to_string()));
            }
        }
        raws.push(*id);
        match fit_shot(&shot, &edit)? {
            Ok(fit) => {
                answer["look"] = report(&fit.look);
                fits.push(fit);
            }
            Err(r) => answer["refused"] = refused(&r),
        }
        answers.push(answer);
    }
    step(total, total);
    let mut answer = json!({
        "dry_run": ask.dry_run,
        "together": true,
        "photos": total,
        "fitted": fits.len(),
        "images": answers,
    });
    let Some((key, name)) = camera else {
        return Ok(Done {
            answer,
            changes: Changes::default(),
        });
    };
    answer["camera"] = json!(key);
    answer["name"] = json!(name);
    let fitted: Vec<Fitted<'_>> = fits
        .iter()
        .map(|f| Fitted {
            look: &f.look,
            gain: f.prepared.gain,
            inputs: f.prepared.inputs(),
        })
        .collect();
    let Some(one) = combine::combine(&fitted) else {
        return Ok(Done {
            answer,
            changes: Changes::default(),
        });
    };
    // What the look was fitted to: every preview, in the photos' order.
    let mut h = blake3::Hasher::new();
    for f in &fits {
        h.update(f.hash.as_bytes());
    }
    let hash = h.finalize().to_hex().to_string();
    let (colour, tone) = (
        edit::colour_params(&one, &hash),
        edit::tone_params(&one, &hash),
    );
    answer["look"] = report(&one);
    answer["preview"] = json!(hash);
    answer["params"] = json!({ edit::COLOUR: colour, edit::TONE: tone });
    let mut changes = Changes::default();
    if !ask.dry_run {
        for id in &raws {
            let (mut session, head) =
                helios_vault::EditSession::open(vault, *id).map_err(store_error)?;
            let before = head.map_or(Value::Null, |v| v.edit);
            let after = edit::with_params(&before, &colour, &tone);
            let rows_before = vault.rows(id).map_err(store_error)?;
            let saved = session
                .save(vault, after, "Camera look", ask.origin, ask.now)
                .map_err(store_error)?;
            if saved.is_some() {
                let rows_after = vault.rows(id).map_err(store_error)?;
                changes.images.push(helios_vault::undo::Change {
                    image: *id,
                    before: rows_before,
                    after: rows_after,
                });
            }
        }
        answer["written"] = json!(changes.images.len());
        if ask.default {
            let doc = defaults::document(&name, &colour, &tone, fits.len(), ask.now);
            let kept = vault
                .set_look_default(&key, Some(&doc))
                .map_err(store_error)?;
            changes.documents.extend(kept.documents);
            answer["default"] = json!(true);
        }
    }
    Ok(Done { answer, changes })
}
