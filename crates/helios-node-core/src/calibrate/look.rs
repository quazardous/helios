//! `look.camera.fit` (`docs/design/camera-look.md`, step 3): a camera
//! look fitted to each raw's embedded JPEG and written into its edit.
//!
//! For each image: its raw rendered small through the develop chain (its
//! edit's raw front and `colorin` over the template's defaults), its
//! embedded preview reduced to the same proxy
//! (`helios_calibrate::look::render`), the fit and its gates
//! (`helios_calibrate::look::fit`). A fitted look goes into the edit with
//! `helios_calibrate::look::edit::with_look`: `cameratone` on and
//! `sigmoid` off, `cameracolour` on unless a chart profile is on. A
//! refused image is answered with its gate in words and left untouched.
//! Every image written is one undoable step of the library together. With
//! `dry_run`, nothing is written.
//!
//! With `together`, the photos (one camera's, refused otherwise) give one
//! look ([`together`]): each fitted on its own, the looks that passed
//! averaged and scored on every photo's held-out pairs, then written into
//! every raw of the selection; with `default`, kept as the camera's
//! default too. `look.camera.setDefault` keeps or clears a camera's
//! default look, which a new photo of that camera opens with
//! ([`defaults`]).

use std::collections::BTreeMap;
use std::path::Path;

use helios_calibrate::look::{self, Gate, Look, Refusal, edit, render};
use helios_core::{Image, Message, Params};
use helios_photo::CameraInfo;
use helios_vault::{Changes, ImageId, Store, Vault};
use serde_json::{Value, json};

use super::{Done, at_hand, image_id, store_error};
use crate::commands::{CommandSpec, Need, object};

pub mod defaults;
pub mod together;

pub use defaults::{SET_DEFAULT, default_on_open, set_default};

/// Fits a camera look to photos' embedded JPEG.
pub const FIT: &str = "look.camera.fit";

/// `look.camera.fit`'s spec.
pub fn spec() -> CommandSpec {
    CommandSpec::new(FIT, "Fit a camera look to the camera's JPEG", "Calibration")
        .needs(Need::Vault)
        .params(object(&[
            (
                "images",
                json!({ "type": "array", "minItems": 1, "items": { "type": "string" },
                        "description": "image ids, or absolute paths the vault knows: raws" }),
                true,
            ),
            (
                "dry_run",
                json!({ "type": "boolean",
                        "description": "answer each fit or refusal, write nothing" }),
                false,
            ),
            (
                "together",
                json!({ "type": "boolean",
                        "description": "one look from all the photos, of one camera, written into each" }),
                false,
            ),
            (
                "default",
                json!({ "type": "boolean",
                        "description": "with `together`: keep the look as the camera's default too" }),
                false,
            ),
        ]))
}

/// What the fit reads of a raw: its sensor data, its camera and its
/// largest embedded preview (working space, linear, as stored).
pub struct Shot {
    pub mosaic: Image,
    pub camera: CameraInfo,
    pub preview: Option<Image>,
}

/// A raw's shot read from its file; `None` for a picture.
///
/// # Errors
/// The file cannot be read.
pub fn shot(path: &Path) -> Result<Option<Shot>, Message> {
    let failed = |e: String| {
        Message::new("look-load-failed", "cannot read the photo: { $error }").with("error", e)
    };
    let loaded = helios_load::load(path).map_err(failed)?;
    let Some(camera) = loaded.camera else {
        return Ok(None);
    };
    let (preview, _) = helios_load::preview_oriented(path).map_err(failed)?;
    Ok(Some(Shot {
        mosaic: loaded.image,
        camera,
        preview,
    }))
}

/// A preview's hash: what the look was fitted to.
pub(crate) fn preview_hash(preview: &Image) -> String {
    let mut h = blake3::Hasher::new();
    for v in preview.data() {
        h.update(&v.to_le_bytes());
    }
    h.finalize().to_hex().to_string()
}

/// A refusal as answered: its gate, in words, and the numbers behind it.
pub(crate) fn refused(r: &Refusal) -> Value {
    json!({
        "gate": r.gate,
        "words": r.gate.words(),
        "detail": r.detail,
    })
}

/// A fitted look as answered: its kind, its scores and its notes.
pub(crate) fn report(look: &Look) -> Value {
    let notes: Vec<Value> = look
        .report
        .notes
        .iter()
        .map(|g| json!({ "gate": g, "words": g.words() }))
        .collect();
    json!({
        "kind": look.kind,
        "fitter": look.version,
        "held_out": look.report.held_out,
        "baseline": look.report.baseline,
        "pairs": look.report.pairs,
        "held_out_pairs": look.report.held_out_pairs,
        "attempt": look.report.attempt,
        "table": look.table.is_some(),
        "chroma": look.chroma.is_some(),
        "notes": notes,
    })
}

/// The chain's parameters an edit gives, by module id.
fn chain_of(edit: &Value) -> Result<render::Chain, Message> {
    let modules = helios_vault::edit::modules(edit).map_err(store_error)?;
    let mut given: BTreeMap<String, Params> = BTreeMap::new();
    for (name, (module, params)) in modules {
        given.entry(module.unwrap_or(name)).or_insert(params);
    }
    Ok(render::from_edit(&given))
}

/// A photo's look, fitted.
pub(crate) struct Fit {
    /// The look as the chain runs it.
    pub look: Look,
    /// The hash of the preview it was fitted to.
    pub hash: String,
    /// What it was fitted on, to score another look on the same pairs.
    pub prepared: render::Prepared,
}

/// The look fitted to `shot`, rendered through `edit`'s chain, as the
/// chain runs it; or the gate that refused it.
pub(crate) fn fit_shot(shot: &Shot, edit: &Value) -> Result<Result<Fit, Refusal>, Message> {
    let Some(preview) = &shot.preview else {
        return Ok(Err(Refusal {
            gate: Gate::PreviewTooSmall,
            detail: "the raw carries no embedded preview".into(),
        }));
    };
    let chain = chain_of(edit)?;
    let prepared = render::prepare(&shot.mosaic, &shot.camera, preview, &chain).map_err(|e| {
        Message::new("look-render-failed", "cannot render the raw: { $error }").with("error", e)
    })?;
    Ok(look::fit(&prepared.inputs()).map(|fitted| Fit {
        look: render::for_chain(&fitted, prepared.gain),
        hash: preview_hash(preview),
        prepared,
    }))
}

/// A long fit's progress: told before each photo (`done` of `total`),
/// it answers whether to go on (`false`: the job was cancelled).
pub type Step<'a> = &'a dyn Fn(usize, usize) -> bool;

/// The answer to a cancelled fit.
pub(crate) fn cancelled() -> Message {
    Message::new(
        "look-fit-cancelled",
        "the camera look's fit was cancelled: nothing was written",
    )
}

/// `look.camera.fit`: each image's look fitted and, without `dry_run`,
/// written into its edit as a version; every image written is one
/// undoable step together. `shot_of` reads a raw's file (`None` for a
/// picture).
///
/// # Errors
/// An image unknown or not at hand, a file that cannot be read or
/// rendered. A refused fit is no error: it is answered with its gate.
pub fn fit_with<S: Store>(
    vault: &Vault<S>,
    params: &Value,
    origin: &str,
    now: u64,
    shot_of: &dyn Fn(&Path) -> Result<Option<Shot>, Message>,
) -> Result<Done, Message> {
    fit_with_progress(vault, params, origin, now, shot_of, &|_, _| true)
}

/// [`fit_with`], its progress told to `step` before each photo (a job's
/// progress), which stops it when it answers `false`: nothing is written
/// then.
///
/// # Errors
/// As [`fit_with`]; with `together`, photos of several cameras; a fit
/// cancelled.
pub fn fit_with_progress<S: Store>(
    vault: &Vault<S>,
    params: &Value,
    origin: &str,
    now: u64,
    shot_of: &dyn Fn(&Path) -> Result<Option<Shot>, Message>,
    step: Step<'_>,
) -> Result<Done, Message> {
    let dry_run = params["dry_run"].as_bool().unwrap_or(false);
    let mut ids: Vec<ImageId> = Vec::new();
    for named in params["images"].as_array().into_iter().flatten() {
        ids.push(image_id(vault, named.as_str().unwrap_or_default())?);
    }
    if params["together"].as_bool().unwrap_or(false) {
        let ask = together::Ask {
            ids,
            dry_run,
            default: params["default"].as_bool().unwrap_or(false),
            origin,
            now,
        };
        return together::fit(vault, &ask, shot_of, step);
    }
    let mut answers = Vec::new();
    let mut changes = Changes::default();
    let total = ids.len();
    for (done, id) in ids.into_iter().enumerate() {
        if !step(done, total) {
            return Err(cancelled());
        }
        let path = at_hand(vault, &id)?;
        let (mut session, head) =
            helios_vault::EditSession::open(vault, id).map_err(store_error)?;
        let before_edit = head.map_or(Value::Null, |v| v.edit);
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
        let (fitted, hash) = match fit_shot(&shot, &before_edit)? {
            Ok(fit) => (fit.look, fit.hash),
            Err(r) => {
                answer["refused"] = refused(&r);
                answers.push(answer);
                continue;
            }
        };
        let after = edit::with_look(&before_edit, &fitted, &hash);
        answer["look"] = report(&fitted);
        answer["colour"] = json!(edit::is_on(&after, edit::COLOUR));
        answer["preview"] = json!(hash);
        // The numbers each node takes, so a client writing the open
        // photo's edit itself (the desktop's header) sets the same look.
        answer["params"] = json!({
            edit::COLOUR: edit::colour_params(&fitted, &hash),
            edit::TONE: edit::tone_params(&fitted, &hash),
        });
        if !dry_run {
            let rows_before = vault.rows(&id).map_err(store_error)?;
            let saved = session
                .save(vault, after, "Camera look", origin, now)
                .map_err(store_error)?;
            answer["version"] = json!(saved.as_ref().map(|v| v.id.hex()));
            if saved.is_some() {
                let rows_after = vault.rows(&id).map_err(store_error)?;
                changes.images.push(helios_vault::undo::Change {
                    image: id,
                    before: rows_before,
                    after: rows_after,
                });
            }
        }
        answers.push(answer);
    }
    let fitted = answers.iter().filter(|a| a.get("look").is_some()).count();
    Ok(Done {
        answer: json!({ "dry_run": dry_run, "fitted": fitted, "images": answers }),
        changes,
    })
}

#[cfg(test)]
mod tests {
    //! The command on an in-memory vault, on a synthetic raw whose
    //! "camera JPEG" is the develop chain with a harder tone curve.

    use helios_calibrate::look::render::{Chain, defaults, render};
    use helios_core::{Region, Value as Param};
    use helios_photo::Cfa;
    use helios_vault::MemoryStore;

    use super::*;

    const W: u32 = 512;
    const H: u32 = 384;

    fn camera() -> CameraInfo {
        CameraInfo {
            maker: "Synthetic".into(),
            model: "Look".into(),
            iso: 100.0,
            exposure_bias: 0.0,
            exposure_time: 0.01,
            aperture: 4.0,
            focal_length: 50.0,
            black: [512.0; 4],
            white: 16383.0,
            white_balance: [2.0, 1.0, 1.5, 1.0],
            xyz_to_camera: [
                0.9020, -0.2890, -0.0715, -0.4535, 1.2436, 0.2348, -0.0934, 0.1919, 0.7086,
            ],
            cfa: Cfa::parse("RGGB").unwrap(),
            orientation: 1,
        }
    }

    /// Patches of many colours, each a ramp from deep shadow to bright,
    /// in white-balanced camera RGB, mosaicked as the camera records it.
    /// Saturation stays moderate (channels within 1 ± 0.3 of the ramp,
    /// the ramp from 0.01 to 0.41): the range where a fitted look comes
    /// within the `too-far` gate of the sigmoid "camera".
    fn mosaic() -> Image {
        patches(0.1, 0.01, 0.4)
    }

    /// Saturated patches (channels within 1 ± 0.75 of the ramp, the ramp
    /// from 0.02 to 0.72): the look's tone curve scales the three
    /// channels by one luminance ratio, so a bright patch's dominant
    /// channel passes 1 where the sigmoid "camera", compressing each
    /// channel, stays under white; the look's gamut compression brings it
    /// back under 1, its hue kept.
    fn saturated_mosaic() -> Image {
        patches(0.25, 0.02, 0.7)
    }

    /// Patches whose channels spread `step`, twice or three times it
    /// around a ramp from `low` over `span`.
    fn patches(step: f64, low: f64, span: f64) -> Image {
        let mut rgb = Vec::with_capacity((W * H * 3) as usize);
        for y in 0..H {
            for x in 0..W {
                let k = (y / 64) * 8 + x / 64;
                let hue = f64::from(k) * 0.618_034 % 1.0 * std::f64::consts::TAU;
                let sat = step * f64::from(1 + k % 3);
                let ramp = low + span * f64::from(x % 64 + (y % 64) / 8) / 72.0;
                let c = [0.0, 2.094, 4.189]
                    .map(|o: f64| (ramp * (1.0 + sat * (hue + o).cos())).clamp(0.0, 0.9) as f32);
                rgb.extend(c);
            }
        }
        let wb = helios_module_whitebalance::d65_coefficients(&camera().xyz_to_camera).unwrap();
        helios_testkit::mosaic(&camera(), wb, Region::sized(W, H), &rgb)
    }

    /// The camera's JPEG: the develop chain exposed a stop lower, with a
    /// harder sigmoid, at `size` pixels on the long edge. Exposed as the
    /// default render, a sigmoid this hard reaches 255 on bright patches
    /// the raw keeps, which the fit rightly refuses (its preview-clipped
    /// gate); a stop lower, its shoulder stays under white, as a camera
    /// keeping its highlights does.
    fn preview(mosaic: &Image, size: u32) -> Image {
        let mut chain: Chain = defaults();
        chain.insert(
            "sigmoid".into(),
            Params::new().with("contrast", Param::Float(2.6)),
        );
        if let Some(exposure) = chain.get_mut("exposure") {
            exposure.set("exposure", Param::Float(-0.3));
        }
        render(mosaic, &camera(), &chain, size, false)
            .unwrap()
            .display
    }

    /// A vault holding one photo (a file at hand) and its id.
    fn vault(name: &str) -> (Vault<MemoryStore>, std::path::PathBuf, ImageId) {
        let dir =
            std::env::temp_dir().join(format!("helios-look-node-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let image = Image::new(Region::sized(4, 4), 3, vec![0.5; 48]);
        let path = dir.join("shot.tif");
        std::fs::write(
            &path,
            helios_export::encode(&image, &helios_export::Format::Tiff).unwrap(),
        )
        .unwrap();
        let vault = Vault::new(MemoryStore::default());
        let id = vault.import_file(&path, 1).unwrap();
        (vault, dir, id)
    }

    /// A vault holding `n` photos (files at hand, each its own), and
    /// their ids in order.
    fn vault_of(name: &str, n: usize) -> (Vault<MemoryStore>, std::path::PathBuf, Vec<ImageId>) {
        let (vault, dir, first) = vault(name);
        let mut ids = vec![first];
        for i in 1..n {
            let image = Image::new(Region::sized(4, 4), 3, vec![0.1 * i as f32; 48]);
            let path = dir.join(format!("shot-{i}.tif"));
            std::fs::write(
                &path,
                helios_export::encode(&image, &helios_export::Format::Tiff).unwrap(),
            )
            .unwrap();
            ids.push(vault.import_file(&path, 1).unwrap());
        }
        (vault, dir, ids)
    }

    /// The shot of every file: the synthetic raw, its camera's model
    /// `model_of` the file's name says.
    fn shots(
        model_of: fn(&Path) -> &'static str,
    ) -> impl Fn(&Path) -> Result<Option<Shot>, Message> {
        let mosaic = mosaic();
        let shown = preview(&mosaic, W);
        move |path: &Path| {
            let mut camera = camera();
            camera.model = model_of(path).into();
            Ok(Some(Shot {
                mosaic: mosaic.clone(),
                camera,
                preview: Some(shown.clone()),
            }))
        }
    }

    #[test]
    fn photos_fitted_together_give_one_look_written_into_each_and_kept_as_the_default() {
        let (vault, dir, ids) = vault_of("together", 2);
        let read = shots(|_| "Look");
        let images: Vec<String> = ids.iter().map(ImageId::hex).collect();
        let told = std::cell::RefCell::new(Vec::new());
        let step = |done: usize, total: usize| {
            told.borrow_mut().push((done, total));
            true
        };
        let params = json!({ "images": images, "together": true, "default": true });
        let done = fit_with_progress(&vault, &params, "user", 30, &read, &step).unwrap();
        assert_eq!(*told.borrow(), vec![(0, 2), (1, 2), (2, 2)]);
        let answer = &done.answer;
        assert_eq!(answer["fitted"], 2, "{answer}");
        assert_eq!(answer["camera"], "synthetic look");
        let (held, base) = (
            answer["look"]["held_out"]["mean"].as_f64().unwrap(),
            answer["look"]["baseline"]["mean"].as_f64().unwrap(),
        );
        assert!(held < 0.7 * base, "{answer}");
        // Both photos carry the one look, the camera keeps it: one step.
        assert_eq!(done.changes.images.len(), 2);
        assert_eq!(done.changes.documents.len(), 1);
        let tone = |id: ImageId| {
            head(&vault, id).unwrap()["modules"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["name"] == edit::TONE)
                .unwrap()["params"]
                .clone()
        };
        assert_eq!(tone(ids[0]), tone(ids[1]));
        let kept = vault.look_default("synthetic look").unwrap().unwrap();
        assert_eq!(
            defaults::preview_of(&kept).as_deref(),
            answer["preview"].as_str()
        );
        vault.revert(&done.changes).unwrap();
        assert_eq!(head(&vault, ids[0]), None);
        assert_eq!(vault.look_default("synthetic look").unwrap(), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn photos_of_two_cameras_are_refused_together_and_nothing_is_written() {
        let (vault, dir, ids) = vault_of("mixed", 2);
        let read = shots(|p| {
            if p.ends_with("shot.tif") {
                "Look"
            } else {
                "Other"
            }
        });
        let images: Vec<String> = ids.iter().map(ImageId::hex).collect();
        let params = json!({ "images": images, "together": true });
        let refused = fit_with(&vault, &params, "user", 30, &read).unwrap_err();
        assert_eq!(refused.key, "look-mixed-cameras");
        assert!(
            refused.english.contains("synthetic other"),
            "{}",
            refused.english
        );
        assert_eq!(head(&vault, ids[0]), None);
        // A cancelled fit writes nothing either.
        let params = json!({ "images": [ids[0].hex()], "together": true });
        let stopped =
            fit_with_progress(&vault, &params, "user", 30, &read, &|_, _| false).unwrap_err();
        assert_eq!(stopped.key, "look-fit-cancelled");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_camera_s_default_look_is_kept_put_on_a_new_photo_once_and_cleared() {
        let (vault, dir, ids) = vault_of("default", 2);
        let read = shots(|_| "Look");
        // The first photo fitted on its own, its look kept as the default.
        fit_with(
            &vault,
            &json!({ "images": [ids[0].hex()] }),
            "user",
            10,
            &read,
        )
        .unwrap();
        let kept = set_default(
            &vault,
            &json!({ "camera": "synthetic look", "image": ids[0].hex() }),
            20,
        )
        .unwrap();
        assert_eq!(kept.answer["kept"], true);
        assert_eq!(kept.changes.documents.len(), 1);
        // A new photo of that camera opens with it; opened again, not.
        let put = default_on_open(&vault, ids[1], "synthetic look", true, "user", 30)
            .unwrap()
            .unwrap();
        assert_eq!(put.images.len(), 1);
        assert!(edit::applied(&head(&vault, ids[1]).unwrap()));
        assert!(
            default_on_open(&vault, ids[1], "synthetic look", false, "user", 40)
                .unwrap()
                .is_none()
        );
        assert!(
            default_on_open(&vault, ids[1], "another camera", true, "user", 40)
                .unwrap()
                .is_none()
        );
        // A look never fitted is refused; `clear` clears.
        let refused = set_default(
            &vault,
            &json!({ "camera": "synthetic look", "look": { "cameracolour": {}, "cameratone": {} } }),
            50,
        )
        .unwrap_err();
        assert_eq!(refused.key, "look-default-not-fitted");
        let cleared = set_default(
            &vault,
            &json!({ "camera": "synthetic look", "clear": true }),
            60,
        )
        .unwrap();
        assert_eq!(cleared.answer["had"], true);
        assert_eq!(vault.look_default("synthetic look").unwrap(), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn head(vault: &Vault<MemoryStore>, image: ImageId) -> Option<Value> {
        helios_vault::EditSession::open(vault, image)
            .unwrap()
            .1
            .map(|v| v.edit)
    }

    #[test]
    fn a_fit_writes_the_look_in_one_step_and_undo_takes_it_back() {
        let (vault, dir, id) = vault("fit");
        let mosaic = mosaic();
        let shown = preview(&mosaic, W);
        let read = |_: &Path| {
            Ok(Some(Shot {
                mosaic: mosaic.clone(),
                camera: camera(),
                preview: Some(shown.clone()),
            }))
        };
        let params = json!({ "images": [id.hex()], "dry_run": true });
        let dry = fit_with(&vault, &params, "agent:test", 10, &read).unwrap();
        assert!(dry.changes.is_empty());
        assert_eq!(head(&vault, id), None, "a dry run writes nothing");
        let answer = &dry.answer["images"][0];
        assert!(answer.get("look").is_some(), "{answer}");
        assert!(
            answer["params"]["cameratone"]["tone"]["list"].is_array(),
            "{answer}"
        );
        assert!(answer["params"]["cameracolour"]["matrix"]["list"].is_array());

        let done = fit_with(
            &vault,
            &json!({ "images": [id.hex()] }),
            "agent:test",
            20,
            &read,
        )
        .unwrap();
        assert_eq!(done.answer["fitted"], 1);
        assert_eq!(done.changes.images.len(), 1, "one step");
        let edit = head(&vault, id).unwrap();
        assert!(edit::applied(&edit));
        assert!(!edit::is_on(&edit, edit::DISPLAY), "sigmoid off");
        assert!(
            edit::is_on(&edit, edit::COLOUR),
            "no chart profile: colour on"
        );
        // The look's held-out score beats the default render's.
        let look = &done.answer["images"][0]["look"];
        let (held, base) = (
            look["held_out"]["mean"].as_f64().unwrap(),
            look["baseline"]["mean"].as_f64().unwrap(),
        );
        assert!(held < 0.7 * base, "{look}");
        // Same inputs, same look.
        assert_eq!(dry.answer["images"][0]["look"], *look);

        vault.revert(&done.changes).unwrap();
        assert_eq!(head(&vault, id), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_saturated_scene_s_look_is_not_refused_for_clipping_highlights() {
        let (vault, dir, id) = vault("saturated");
        let mosaic = saturated_mosaic();
        let shown = preview(&mosaic, W);
        let read = |_: &Path| {
            Ok(Some(Shot {
                mosaic: mosaic.clone(),
                camera: camera(),
                preview: Some(shown.clone()),
            }))
        };
        let params = json!({ "images": [id.hex()], "dry_run": true });
        let dry = fit_with(&vault, &params, "agent:test", 10, &read).unwrap();
        let answer = &dry.answer["images"][0];
        // The gamut compression keeps the bright saturated patches under
        // white, their hue kept: the look is fitted or, short of the
        // camera's own saturation, refused further on, never for clipping.
        assert_ne!(answer["refused"]["gate"], "clips-highlights", "{answer}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_too_small_preview_is_refused_with_its_gate_and_nothing_is_written() {
        let (vault, dir, id) = vault("small");
        let mosaic = mosaic();
        let small = preview(&mosaic, 128);
        let read = |_: &Path| {
            Ok(Some(Shot {
                mosaic: mosaic.clone(),
                camera: camera(),
                preview: Some(small.clone()),
            }))
        };
        let done = fit_with(&vault, &json!({ "images": [id.hex()] }), "user", 10, &read).unwrap();
        assert!(done.changes.is_empty());
        assert_eq!(head(&vault, id), None);
        let refused = &done.answer["images"][0]["refused"];
        assert_eq!(refused["gate"], "preview-too-small");
        assert_eq!(refused["words"], Gate::PreviewTooSmall.words());
        assert_eq!(done.answer["fitted"], 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn with_a_chart_profile_on_the_look_s_colour_half_stays_off() {
        let (vault, dir, id) = vault("chart");
        let (mut session, _) = helios_vault::EditSession::open(&vault, id).unwrap();
        let charted = helios_calibrate::apply::with_profile(
            &Value::Null,
            "profile",
            &helios_photo::color::IDENTITY,
            None,
        );
        session.save(&vault, charted, "Profile", "user", 5).unwrap();
        let mosaic = mosaic();
        let shown = preview(&mosaic, W);
        let read = |_: &Path| {
            Ok(Some(Shot {
                mosaic: mosaic.clone(),
                camera: camera(),
                preview: Some(shown.clone()),
            }))
        };
        let done = fit_with(&vault, &json!({ "images": [id.hex()] }), "user", 10, &read).unwrap();
        assert_eq!(done.answer["images"][0]["colour"], false, "{}", done.answer);
        let edit = head(&vault, id).unwrap();
        assert!(edit::applied(&edit), "the tone half on top of the chart");
        assert!(!edit::is_on(&edit, edit::COLOUR));
        assert!(helios_calibrate::apply::applied(&edit).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_picture_is_answered_as_not_a_raw() {
        let (vault, dir, id) = vault("picture");
        let done = fit_with(
            &vault,
            &json!({ "images": [id.hex()] }),
            "user",
            10,
            &|_: &Path| Ok(None),
        )
        .unwrap();
        assert_eq!(done.answer["images"][0]["refused"]["gate"], "not-a-raw");
        assert!(done.changes.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
