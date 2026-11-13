//! The desktop's calibration commands (`docs/design/calibration.md`,
//! section 8), run through the desktop's embedded node as the CLI and
//! agents run them over its endpoint (`helios_node_core::calibrate`):
//!
//! - `calibrate.chart`: a colour chart shot measured and its correction
//!   solved; with `save`, kept as a profile in the vault.
//! - `calibrate.profiles.list`, `.get`, `.delete`: the saved profiles.
//! - `calibrate.profiles.setDefault`: a camera's default profiles.
//! - `calibrate.apply`: a profile (or `auto`, or `none`) put on photos'
//!   edits, the `chartprofile` node.
//! - `calibrate.profiles.export`, `.import`: a profile as a file to share.
//! - `look.camera.fit`: a camera look fitted to photos' embedded JPEG and
//!   written into their edits (`cameracolour`, `cameratone`).
//! - `calibrate.choose`: the open photo's profile, chosen in the module's
//!   header, one step of the edit's history (not of the library's).
//! - `look.camera.choose`: the open photo's camera look fitted, turned on
//!   or off from the Camera look header, one step of the edit's history.
//! - `look.camera.setDefault`: a camera's default camera look kept or
//!   cleared (the header's "Save as the camera's default").
//! - `look.camera.fitSelection`: one camera look fitted from photos of one
//!   camera (the Library's selection), run as a job with its progress in
//!   the jobs centre, written into each photo as one library step.
//!
//! When a raw opens for the first time, its camera's default profile is
//! put on it, then its camera's default look ([`on_open`]); a default
//! that picks none says why in the status bar and leaves the photo as it
//! is.
//!
//! Each run is announced on the node's bus with who ran it
//! (`control.command`). A save, a deletion, a default set or an apply is
//! one step of the library's history, so Ctrl+Z takes it back. Without a library, the chart's dry
//! run is still answered; the profile commands say they need one.

use std::sync::Arc;

use egui_pop::api::history::Label;
use helios_core::{Bus, Message};
use helios_node_core::calibrate::{
    self as node, APPLY, CHART, DELETE, EXPORT, GET, IMPORT, LIST, LOOK_FIT, LOOK_SET_DEFAULT,
    SET_DEFAULT,
};
use helios_shell::commands::CommandSpec;
use helios_vault::{Changes, Store, Vault};
use serde_json::Value;

use crate::library_history::LibraryHistory;
use crate::shell::Shell;

/// Measures a chart shot and answers the report; saves it when asked.
pub const CHART_COMMAND: &str = CHART;

/// The vault the desktop's node owns and its bus.
pub struct Node<S: Store> {
    pub vault: Arc<Vault<S>>,
    pub bus: Arc<Bus>,
}

fn seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The history step of command `id` on the profile `name` (an apply
/// with none: the profile turned off; a default: of camera `name`).
pub fn label(id: &str, name: &str) -> Label {
    let m = if id == LOOK_SET_DEFAULT {
        Message::new(
            "gui-history-camera-look-default",
            "Change the default camera look of { $name }",
        )
    } else if id == LOOK_FIT {
        Message::new(
            "gui-history-camera-look-fit",
            "Fit the camera look to the camera's JPEG",
        )
    } else if id == DELETE {
        Message::new(
            "gui-history-calibration-delete",
            "Delete the calibration profile “{ $name }”",
        )
    } else if id == APPLY && name.is_empty() {
        Message::new(
            "gui-history-calibration-off",
            "Turn the calibration profile off",
        )
    } else if id == APPLY {
        Message::new(
            "gui-history-calibration-apply",
            "Apply the calibration profile “{ $name }”",
        )
    } else if id == IMPORT {
        Message::new(
            "gui-history-calibration-import",
            "Import the calibration profile “{ $name }”",
        )
    } else if id == SET_DEFAULT {
        Message::new(
            "gui-history-calibration-default",
            "Set the calibration default of { $name }",
        )
    } else {
        Message::new(
            "gui-history-calibration-save",
            "Save the calibration profile “{ $name }”",
        )
    }
    .with("name", name);
    Label {
        text: m.english.clone(),
        key: Some(m.key),
        args: [("name".to_string(), name.to_string())].into(),
    }
}

/// The profile's name in an answer of `id` (an apply turning it off:
/// none; a default: its camera).
fn named(id: &str, answer: &Value) -> String {
    if id == APPLY {
        return answer["images"][0]["name"]
            .as_str()
            .unwrap_or_default()
            .to_string();
    }
    if id == SET_DEFAULT || id == LOOK_SET_DEFAULT {
        return answer["camera"].as_str().unwrap_or_default().to_string();
    }
    let profile = if id == DELETE {
        &answer["deleted"]
    } else {
        &answer["profile"]
    };
    profile["name"].as_str().unwrap_or_default().to_string()
}

/// Runs command `id` through the node holding `vault`, by `who`; the
/// change it made recorded in `history`.
fn through_node<S: Store>(
    node: &Node<S>,
    history: &LibraryHistory,
    id: &str,
    args: &Value,
    who: &str,
) -> Result<Value, String> {
    let (answer, changes): (Value, Changes) =
        helios_node_core::endpoint::command(&node.vault, &node.bus, id, args, who)?;
    if !changes.is_empty() {
        history.record(label(id, &named(id, &answer)), changes, seconds());
    }
    Ok(answer)
}

/// Registers the calibration commands on the shell, run through `node`
/// (none without a library), their changes recorded in `history`.
///
/// # Errors
/// A command cannot be registered.
pub fn register<S: Store + Send + 'static>(
    shell: &mut Shell,
    node: Option<Node<S>>,
    history: LibraryHistory,
) -> Result<(), String> {
    let node = node.map(Arc::new);
    let specs = [
        (
            CHART,
            "Measures a colour chart shot and solves its correction: the report per patch, the light and the matrix; with `save`, keeps it as a calibration profile",
            false,
        ),
        (
            LIST,
            "Lists the calibration profiles (`camera`: one camera's)",
            true,
        ),
        (GET, "Shows a calibration profile (`profile`: its id)", true),
        (
            DELETE,
            "Deletes a calibration profile (`profile`: its id)",
            false,
        ),
        (
            SET_DEFAULT,
            "Sets a camera's default calibration profiles (`profiles`: their ids, empty with `camera` to clear)",
            false,
        ),
        (
            APPLY,
            "Applies a calibration profile to photos (`images`; `profile`: an id, `auto` for the camera's nearest light, or `none`)",
            false,
        ),
        (
            EXPORT,
            "Exports a calibration profile to a file to share (`profile`; `path`: a folder or a file)",
            true,
        ),
        (
            IMPORT,
            "Imports a calibration profile from a shared file (`path`), checked first",
            false,
        ),
        (
            LOOK_FIT,
            "Fits a camera look to photos' embedded JPEG (`images`: raws) and writes it into their edits; a refusal names its reason; `together`: one look from photos of one camera",
            false,
        ),
        (
            LOOK_SET_DEFAULT,
            "Keeps a camera's default camera look, which its new photos open with (`camera`; `look`: the two nodes' parameters, or `image`: its edit's look), or clears it (`clear`)",
            false,
        ),
    ];
    let values = shell.values().clone();
    for (id, label, read_only) in specs {
        let mut spec = CommandSpec::new(id, label, "Calibration");
        if read_only {
            spec = spec.read_only();
        }
        let (node, history, values) = (node.clone(), history.clone(), values.clone());
        shell.register(spec, move |args, origin| {
            let who = crate::persist::who(origin);
            match &node {
                Some(node) => {
                    let answer = through_node(node, &history, id, args, &who)?;
                    if matches!(id, CHART | DELETE | IMPORT | LOOK_SET_DEFAULT) {
                        refresh_offer(&node.vault, &values);
                    }
                    Ok(answer)
                }
                None => node::run(None::<&Vault<S>>, id, args, 0)
                    .map(|done| done.answer)
                    .map_err(|m| m.english),
            }
        })?;
    }
    let (look_node, look_values) = (node.clone(), values.clone());
    let (fit_node, fit_history, jobs) = (node.clone(), history.clone(), shell.jobs());
    let spec = CommandSpec::new(
        LOOK_FIT_SELECTION,
        "Fits one camera look from photos of one camera (`images`; `default`: keep it as the camera's default too) as a job, and writes it into each",
        "Calibration",
    );
    shell.register(spec, move |args, origin| match &fit_node {
        Some(node) => fit_selection(node, &fit_history, &jobs, args, origin),
        None => Err("no library open: the camera look fits photos of it".into()),
    })?;
    let spec = CommandSpec::new(
        CHOOSE,
        "Sets the open photo's calibration profile (`profile`: an id of its camera's, or `none`), one step of its edit",
        "Calibration",
    );
    shell.register(spec, move |args, origin| match &node {
        Some(node) => choose(&node.vault, &values, args, origin),
        None => Err("no library open: calibration profiles live in one".into()),
    })?;
    let spec = CommandSpec::new(
        LOOK_CHOOSE,
        "Sets the open photo's camera look (`look`: `fit` to fit it to the camera's JPEG, `on` or `off`), one step of its edit",
        "Calibration",
    );
    shell.register(spec, move |args, origin| match &look_node {
        Some(node) => look_choose(node, &look_values, args, origin),
        None => Err("no library open: the camera look fits a photo of it".into()),
    })?;
    Ok(())
}

/// Sets the open photo's chart profile (`profile`: an id of its camera's
/// profiles, or `none`) on its `chartprofile` node, as one step of the
/// edit's history: the profile's values copied into the parameters and
/// the node turned on, or the node bypassed.
pub const CHOOSE: &str = "calibrate.choose";

/// The open photo's camera and the profiles it may take, as the module's
/// header lists them (a JSON text; empty for a photo without the
/// module): `{camera, make, model, light, profiles: [{id, name,
/// illuminant, quality}]}`.
pub const OFFER: &str = "view/calibration/offer";

/// Sets the open photo's camera look (`look`: `fit`, `on` or `off`) on
/// its `cameracolour` and `cameratone` nodes, as one step of the edit's
/// history (`docs/design/camera-look.md`, section 3.4).
pub const LOOK_CHOOSE: &str = "look.camera.choose";

/// Fits one camera look from photos of one camera (`images`: the
/// Library's selection), as a job: [`fit_selection`].
pub const LOOK_FIT_SELECTION: &str = "look.camera.fitSelection";

/// Why the open photo's last fit was refused, in words (empty: none).
pub const LOOK_REFUSAL: &str = "view/look/refusal";

/// The camera's embedded JPEG is the viewport's "before": the before and
/// after split shows it left of the render (`docs/design/camera-look.md`,
/// section 3.4). A view, not an edit: never undone, never saved.
pub const LOOK_COMPARE: &str = "view/look/compare";

/// Why the camera's JPEG could not be shown (empty: nothing to say):
/// [`NO_JPEG`] or [`NOT_A_RAW`].
pub const LOOK_COMPARE_MISSING: &str = "view/look/compare_missing";

/// Declares [`OFFER`], [`LOOK_REFUSAL`], [`LOOK_COMPARE`] and
/// [`LOOK_COMPARE_MISSING`]: transient, never undone.
pub fn declare(store: &mut helios_values::Store) {
    store.declare(
        LOOK_COMPARE,
        helios_values::Descriptor::bool("compare with the camera's JPEG", false).transient(),
    );
    store.declare(
        LOOK_COMPARE_MISSING,
        helios_values::Descriptor::text("camera JPEG missing", "").transient(),
    );
    store.declare(
        OFFER,
        helios_values::Descriptor::text("calibration profiles offered", "").transient(),
    );
    store.declare(
        LOOK_REFUSAL,
        helios_values::Descriptor::text("camera look refused", "").transient(),
    );
}

/// The node running `module` in the network the store holds, by name.
fn module_node(store: &helios_values::Store, module: &str) -> Option<String> {
    store
        .list("node/")
        .into_iter()
        .find(|(_, e)| e.descriptor.module.as_deref() == Some(module))
        .and_then(|(p, _)| {
            p.strip_prefix("node/")?
                .rsplit_once('/')
                .map(|(n, _)| n.to_string())
        })
}

/// The `chartprofile` node of the network the store holds, by name.
pub fn chart_node(store: &helios_values::Store) -> Option<String> {
    module_node(store, helios_calibrate::apply::MODULE)
}

/// The `cameratone` node of the network the store holds, by name: the
/// Camera look entry.
pub fn look_node(store: &helios_values::Store) -> Option<String> {
    module_node(store, helios_calibrate::look::edit::TONE)
}

/// What the photo's chart profile node knows of it: its camera's key and
/// its as-shot light; its make and model. `None` without the node or a
/// raw's camera.
pub fn photo_of(graph: &helios_core::Graph) -> Option<(node::Photo, String, String)> {
    use helios_photo::camera::fields;
    let id = graph.find(helios_calibrate::apply::MODULE)?;
    let context = graph.context(id).ok()??;
    let text = |f: &str| context.get(f).and_then(|v| v.as_choice()).map(String::from);
    let list = |f: &str| {
        context
            .get(f)
            .and_then(|v| v.as_list())
            .map(<[f64]>::to_vec)
    };
    let (make, model) = (text(fields::MAKER)?, text(fields::MODEL)?);
    let light = match (list(fields::XYZ_TO_CAMERA), list(fields::WHITE_BALANCE)) {
        (Some(m), Some(wb)) => helios_calibrate::apply::as_shot_light(&m, &wb),
        _ => None,
    };
    Some((
        node::Photo {
            camera: helios_calibrate::camera_key(&make, &model),
            light,
        },
        make,
        model,
    ))
}

/// The offer for a camera: its profiles, as listed.
fn offer<S: Store>(vault: &Vault<S>, camera: &str, make: &str, model: &str, light: Value) -> Value {
    let profiles = node::run(
        Some(vault),
        LIST,
        &serde_json::json!({ "camera": camera }),
        0,
    )
    .map_or_else(|_| serde_json::json!([]), |d| d.answer["profiles"].clone());
    // The camera's default look, by what it was fitted to: the header
    // says when the open photo's look is it.
    let look_default = vault
        .look_default(camera)
        .ok()
        .flatten()
        .and_then(|d| node::look::defaults::preview_of(&d))
        .map_or(
            Value::Null,
            |preview| serde_json::json!({ "preview": preview }),
        );
    serde_json::json!({
        "camera": camera,
        "make": make,
        "model": model,
        "light": light,
        "profiles": profiles,
        "look_default": look_default,
    })
}

/// Sets the offer the module's header reads (none: no photo with it).
pub fn set_offer(store: &std::sync::Mutex<helios_values::Store>, offer: Option<&Value>) {
    let text = offer.map_or_else(String::new, Value::to_string);
    let mut s = store.lock().expect("value store lock");
    let _ = s.set(helios_values::Set::new(
        OFFER,
        helios_values::Change::To(helios_core::Value::Text(text)),
        helios_shell::commands::Origin::User,
    ));
}

/// The offer the store holds, read.
pub fn read_offer(store: &helios_values::Store) -> Option<Value> {
    match &store.get(OFFER)?.value {
        helios_core::Value::Text(t) if !t.is_empty() => serde_json::from_str(t).ok(),
        _ => None,
    }
}

/// Lists the offered camera's profiles again (after a save, a deletion,
/// an import).
fn refresh_offer<S: Store>(vault: &Vault<S>, store: &std::sync::Mutex<helios_values::Store>) {
    let Some(old) = read_offer(&store.lock().expect("value store lock")) else {
        return;
    };
    let text = |k: &str| old[k].as_str().unwrap_or_default().to_string();
    let mut new = offer(
        vault,
        &text("camera"),
        &text("make"),
        &text("model"),
        old["light"].clone(),
    );
    new["image"] = old["image"].clone();
    set_offer(store, Some(&new));
}

/// A photo opened in the viewport (`graph`, its network built): the
/// module's header offered the camera's profiles and, opened for the
/// first time, the camera's default profile put on it
/// ([`node::default_on_open`]), then the camera's default look
/// ([`node::look::default_on_open`]). Answers what to tell: the profile
/// and the look applied, or why the default picked none; never an error
/// dialog.
pub fn on_open<S: Store>(
    vault: &Vault<S>,
    image: helios_vault::ImageId,
    graph: &helios_core::Graph,
    store: &std::sync::Mutex<helios_values::Store>,
) -> Vec<Message> {
    set_compare(store, false, "");
    let Some((photo, make, model)) = photo_of(graph) else {
        set_offer(store, None);
        set_refusal(store, "");
        return Vec::new();
    };
    // Never edited before this opening: the camera's defaults apply.
    let fresh = vault.versions(&image).is_ok_and(|v| v.is_empty());
    let light = photo.light.map_or(Value::Null, |l| serde_json::json!(l));
    let mut offered = offer(vault, &photo.camera, &make, &model, light);
    // The camera look's header fits this image, and compares its file's
    // embedded JPEG.
    offered["image"] = Value::String(image.hex());
    if let Some(path) = vault
        .locations(&image)
        .unwrap_or_default()
        .into_iter()
        .map(|l| l.path)
        .find(|p| p.exists())
    {
        offered["path"] = Value::String(path.to_string_lossy().into_owned());
    }
    set_offer(store, Some(&offered));
    set_refusal(store, "");
    let now = seconds() as u64;
    let mut said = Vec::new();
    match node::default_on_open(vault, image, &photo, crate::persist::WHO, now) {
        Ok(node::OnOpen::Applied { name, .. }) => said.push(
            Message::new(
                "gui-calibration-default-applied",
                "Calibration profile “{ $name }” applied: the default of { $camera }",
            )
            .with("name", name)
            .with("camera", photo.camera.clone()),
        ),
        Ok(node::OnOpen::Refused(reason)) => said.push(
            Message::new(
                "gui-calibration-default-none",
                "No calibration profile applied: { $reason }",
            )
            .with("reason", reason.english),
        ),
        Ok(_) => {}
        Err(e) => said.push(e),
    }
    match node::look::default_on_open(vault, image, &photo.camera, fresh, crate::persist::WHO, now)
    {
        Ok(Some(_)) => said.push(
            Message::new(
                "gui-look-default-applied",
                "Camera look applied: the default of { $camera }",
            )
            .with("camera", photo.camera),
        ),
        Ok(None) => {}
        Err(e) => said.push(e),
    }
    said
}

/// [`CHOOSE`]: the profile set on the open photo's `chartprofile` node,
/// one step of the edit's history, by `origin`.
fn choose<S: Store>(
    vault: &Vault<S>,
    store: &std::sync::Mutex<helios_values::Store>,
    args: &Value,
    origin: &helios_shell::commands::Origin,
) -> Result<Value, String> {
    use helios_values::{Change, Set, bypass_path, node_path};
    let asked = args["profile"].as_str().unwrap_or_default();
    let mut s = store.lock().expect("value store lock");
    let target = chart_node(&s).ok_or("the photo shown has no chart profile module")?;
    let camera = read_offer(&s).map(|o| o["camera"].clone());
    let (label, writes) = if asked == "none" {
        (
            "Calibration profile off".to_string(),
            vec![(bypass_path(&target), helios_core::Value::Bool(true))],
        )
    } else {
        let done = node::run(
            Some(vault),
            GET,
            &serde_json::json!({ "profile": asked }),
            0,
        )
        .map_err(|m| m.english)?;
        let profile: helios_calibrate::Profile =
            serde_json::from_value(done.answer["profile"].clone()).map_err(|e| e.to_string())?;
        if let Some(camera) = camera
            && camera != profile.camera.key.as_str()
        {
            return Err(format!(
                "the profile is for camera {}, the photo is from {camera}",
                profile.camera.key
            ));
        }
        (
            format!("Calibration profile “{}”", profile.name),
            vec![
                (
                    node_path(&target, "matrix"),
                    helios_core::Value::List(profile.matrix.iter().flatten().copied().collect()),
                ),
                (
                    node_path(&target, "lut"),
                    helios_core::Value::List(helios_calibrate::apply::lut_param(
                        profile.lut.as_ref(),
                    )),
                ),
                (
                    node_path(&target, "profile"),
                    helios_core::Value::Text(asked.to_string()),
                ),
                (bypass_path(&target), helios_core::Value::Bool(false)),
            ],
        )
    };
    let group = s.begin_group(origin.clone(), &label);
    let mut failed = None;
    for (path, value) in writes {
        if let Err(e) = s.set(Set::new(&path, Change::To(value), origin.clone()).gesture(group)) {
            failed = Some(e.to_string());
        }
    }
    s.end(group);
    match failed {
        Some(e) => Err(e),
        None => Ok(serde_json::json!({ "node": target, "profile": asked })),
    }
}

/// The `chartprofile` module's header in the Parameters panel, under the
/// node's name: the profile it applies (or none), the photo's camera and
/// the profile's light and quality, and a list to choose another of the
/// camera's profiles or none ([`CHOOSE`], one undoable step of the
/// edit). Nothing for another node, or a photo without a camera.
#[allow(
    clippy::too_many_lines,
    reason = "the header reads, lists and offers in one place"
)]
pub fn header_ui(ui: &mut egui::Ui, store: &std::sync::Mutex<helios_values::Store>, node: &str) {
    use helios_values::{bypass_path, node_path};
    let (offer, current, bypassed) = {
        let s = store.lock().expect("value store lock");
        if chart_node(&s).as_deref() != Some(node) {
            return;
        }
        let current = match s.get(&node_path(node, "profile")).map(|e| &e.value) {
            Some(helios_core::Value::Text(t)) => t.clone(),
            _ => String::new(),
        };
        let bypassed = s
            .get(&bypass_path(node))
            .and_then(|e| e.value.as_bool())
            .unwrap_or(true);
        (read_offer(&s), current, bypassed)
    };
    let Some(offer) = offer else {
        return;
    };
    let ctx = ui.ctx().clone();
    let text = |m: Message| crate::i18n::text(&ctx, m);
    let profiles: Vec<Value> = offer["profiles"].as_array().cloned().unwrap_or_default();
    let on = !bypassed && !current.is_empty();
    let entry = profiles.iter().find(|p| p["id"] == current.as_str());
    let none = text(Message::new("gui-calibration-header-none", "No profile"));
    let light = |p: &Value| {
        p["illuminant"]["cct"]
            .as_f64()
            .map_or_else(String::new, |k| format!("{k:.0} K"))
    };
    let shown = match (on, entry) {
        (false, _) => none.clone(),
        (true, Some(p)) => p["name"].as_str().unwrap_or_default().to_string(),
        // A profile no longer in the library: its values are in the edit.
        (true, None) => text(
            Message::new(
                "gui-calibration-header-gone",
                "Profile { $id } (not in the library)",
            )
            .with("id", current.chars().take(8).collect::<String>()),
        ),
    };
    let camera = [offer["make"].as_str(), offer["model"].as_str()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let about = match (on, entry) {
        (true, Some(p)) => text(
            Message::new(
                "gui-calibration-header-about",
                "{ $camera }, { $light }, quality { $quality }",
            )
            .with("camera", camera.clone())
            .with("light", light(p))
            .with("quality", p["quality"].as_str().unwrap_or_default()),
        ),
        _ => camera.clone(),
    };
    ui.weak(about);
    let name = text(Message::new(
        "gui-calibration-header-choose",
        "Calibration profile",
    ));
    let mut picked = None;
    let r = egui::ComboBox::from_id_salt(("helios.chartprofile.choose", node))
        .selected_text(&shown)
        .width(ui.available_width().min(240.0))
        .show_ui(ui, |ui| {
            if ui.selectable_label(!on, &none).clicked() && on {
                picked = Some("none".to_string());
            }
            for p in &profiles {
                let id = p["id"].as_str().unwrap_or_default();
                let label = format!("{} ({})", p["name"].as_str().unwrap_or_default(), light(p));
                let chosen = on && id == current;
                if ui.selectable_label(chosen, label).clicked() && !chosen {
                    picked = Some(id.to_string());
                }
            }
        })
        .response;
    egui_pop::core::a11y::name(&r, egui::Role::ComboBox, &name);
    let r = r.on_hover_text(&name);
    crate::name_rect(&ctx, "helios.chartprofile.choose", r.rect);
    if profiles.is_empty() {
        ui.weak(text(Message::new(
            "gui-calibration-header-empty",
            "No calibration profile for this camera yet",
        )));
    }
    let make = text(Message::new(
        "gui-calibration-header-make",
        "Make a profile…",
    ));
    let r = ui.small_button(&make);
    egui_pop::core::a11y::name(&r, egui::Role::Button, &make);
    if r.clicked() {
        crate::dialogs::request(
            &ctx,
            crate::dialogs::Request::Run(
                crate::shell::CALIBRATE_COMMAND.into(),
                serde_json::json!({ "camera": offer["camera"] }),
                helios_shell::commands::Origin::User,
            ),
        );
    }
    if let Some(profile) = picked {
        crate::dialogs::request(
            &ctx,
            crate::dialogs::Request::Run(
                CHOOSE.into(),
                serde_json::json!({ "profile": profile }),
                helios_shell::commands::Origin::User,
            ),
        );
    }
}

/// Sets why the open photo's last fit was refused (empty: none).
pub fn set_refusal(store: &std::sync::Mutex<helios_values::Store>, words: &str) {
    let mut s = store.lock().expect("value store lock");
    let _ = s.set(helios_values::Set::new(
        LOOK_REFUSAL,
        helios_values::Change::To(helios_core::Value::Text(words.to_string())),
        helios_shell::commands::Origin::User,
    ));
}

/// Why the open photo's last fit was refused, as the store holds it.
pub fn read_refusal(store: &helios_values::Store) -> String {
    match store.get(LOOK_REFUSAL).map(|e| &e.value) {
        Some(helios_core::Value::Text(t)) => t.clone(),
        _ => String::new(),
    }
}

/// Turns the compare with the camera's JPEG on or off, with the viewport's
/// before and after split; `missing` says why the JPEG is not shown
/// (empty: nothing to say).
pub fn set_compare(store: &std::sync::Mutex<helios_values::Store>, on: bool, missing: &str) {
    let mut s = store.lock().expect("value store lock");
    write_compare(&mut s, on, missing);
}

/// [`set_compare`] on a store already locked. Turning it off ends the
/// split only when the compare had turned it on: a before and after of
/// the user's own stays.
pub fn write_compare(store: &mut helios_values::Store, on: bool, missing: &str) {
    use helios_core::Value as V;
    let split = on || is_comparing(store);
    let mut writes = vec![
        (LOOK_COMPARE, V::Bool(on)),
        (LOOK_COMPARE_MISSING, V::Text(missing.to_string())),
    ];
    if split {
        writes.push((crate::viewport::COMPARE, V::Bool(on)));
    }
    for (path, value) in writes {
        let _ = store.set(helios_values::Set::new(
            path,
            helios_values::Change::To(value),
            helios_shell::commands::Origin::User,
        ));
    }
}

/// Whether the camera's JPEG is asked for as the viewport's "before".
pub fn is_comparing(store: &helios_values::Store) -> bool {
    store
        .get(LOOK_COMPARE)
        .and_then(|e| e.value.as_bool())
        .unwrap_or(false)
}

/// The file of the open photo the camera's JPEG is read from, when it is
/// at hand.
pub fn compared_path(store: &helios_values::Store) -> Option<std::path::PathBuf> {
    read_offer(store)?["path"]
        .as_str()
        .map(std::path::PathBuf::from)
}

/// [`LOOK_COMPARE_MISSING`]: a raw without an embedded JPEG.
pub const NO_JPEG: &str = "no-jpeg";

/// [`LOOK_COMPARE_MISSING`]: a file that is no raw (or cannot be read).
pub const NOT_A_RAW: &str = "not-a-raw";

/// The camera's embedded JPEG of the raw at `path`, upright, in the
/// working space: the viewport's "before" when comparing.
///
/// # Errors
/// Why not: [`NO_JPEG`], [`NOT_A_RAW`].
pub fn camera_jpeg(path: &std::path::Path) -> Result<helios_core::Image, &'static str> {
    match helios_load::preview_oriented(path) {
        Ok((Some(preview), tag)) => Ok(helios_load::upright(preview, tag)),
        Ok((None, _)) => Err(NO_JPEG),
        Err(_) => Err(NOT_A_RAW),
    }
}

/// Why the camera's JPEG is not shown, as the store holds it.
fn read_missing(store: &helios_values::Store) -> String {
    match store.get(LOOK_COMPARE_MISSING).map(|e| &e.value) {
        Some(helios_core::Value::Text(t)) => t.clone(),
        _ => String::new(),
    }
}

/// Whether `node` is bypassed in the store (a node without the switch
/// counts as bypassed).
fn is_bypassed(store: &helios_values::Store, node: &str) -> bool {
    store
        .get(&helios_values::bypass_path(node))
        .and_then(|e| e.value.as_bool())
        .unwrap_or(true)
}

/// Whether the store's `cameratone` node holds a fitted look: its tone
/// curve has knots.
fn is_fitted(store: &helios_values::Store, tone: &str) -> bool {
    matches!(
        store.get(&helios_values::node_path(tone, "tone")).map(|e| &e.value),
        Some(helios_core::Value::List(k)) if !k.is_empty()
    )
}

/// [`LOOK_CHOOSE`]: the open photo's camera look fitted through the node
/// (`look.camera.fit`, a dry run: the numbers come back and are written
/// here, into the edit), turned on or off; one step of the edit's
/// history, by `origin`. A refused fit writes nothing and is kept, in
/// words, for the header ([`LOOK_REFUSAL`]).
fn look_choose<S: Store>(
    node: &Node<S>,
    store: &std::sync::Mutex<helios_values::Store>,
    args: &Value,
    origin: &helios_shell::commands::Origin,
) -> Result<Value, String> {
    use helios_calibrate::look::edit::{COLOUR, DISPLAY, TONE};
    use helios_values::{Change, Set, bypass_path, node_path};
    let asked = args["look"].as_str().unwrap_or_default();
    let (tone, colour, display, charted, fitted, image) = {
        let s = store.lock().expect("value store lock");
        let tone = look_node(&s).ok_or("the photo shown has no camera look module")?;
        let charted = chart_node(&s).is_some_and(|c| !is_bypassed(&s, &c));
        let fitted = is_fitted(&s, &tone);
        let image = read_offer(&s).and_then(|o| o["image"].as_str().map(String::from));
        (
            tone,
            module_node(&s, COLOUR),
            module_node(&s, DISPLAY),
            charted,
            fitted,
            image,
        )
    };
    // The switches turning the look on: the tone half in sigmoid's place,
    // the colour half unless a chart profile rules the colours.
    let on = |writes: &mut Vec<(String, helios_core::Value)>| {
        writes.push((bypass_path(&tone), helios_core::Value::Bool(false)));
        if let Some(c) = &colour {
            writes.push((bypass_path(c), helios_core::Value::Bool(charted)));
        }
        if let Some(d) = &display {
            writes.push((bypass_path(d), helios_core::Value::Bool(true)));
        }
    };
    let mut writes = Vec::new();
    let mut answer = serde_json::json!({ "node": tone, "look": asked });
    let label = match asked {
        "off" => {
            writes.push((bypass_path(&tone), helios_core::Value::Bool(true)));
            if let Some(c) = &colour {
                writes.push((bypass_path(c), helios_core::Value::Bool(true)));
            }
            if let Some(d) = &display {
                writes.push((bypass_path(d), helios_core::Value::Bool(false)));
            }
            "Camera look off"
        }
        "on" if !fitted => {
            return Err("the camera look is not fitted yet: fit it to the camera's JPEG".into());
        }
        "on" => {
            on(&mut writes);
            "Camera look on"
        }
        "fit" => {
            let image = image.ok_or("the photo shown is not one of the library's")?;
            let who = crate::persist::who(origin);
            let (fit, _) = helios_node_core::endpoint::command(
                &node.vault,
                &node.bus,
                LOOK_FIT,
                &serde_json::json!({ "images": [image], "dry_run": true }),
                &who,
            )?;
            let done = fit["images"][0].clone();
            answer["fit"] = done.clone();
            if let Some(words) = done["refused"]["words"].as_str() {
                set_refusal(store, words);
                return Ok(answer);
            }
            for (module, name) in [(COLOUR, colour.as_ref()), (TONE, Some(&tone))] {
                let Some(name) = name else { continue };
                let params: helios_core::Params =
                    serde_json::from_value(done["params"][module].clone())
                        .map_err(|e| format!("the fit's {module} parameters: {e}"))?;
                for (param, value) in params.iter() {
                    writes.push((node_path(name, param), value.clone()));
                }
            }
            on(&mut writes);
            set_refusal(store, "");
            "Camera look fitted to the camera's JPEG"
        }
        other => return Err(format!("`look` is `fit`, `on` or `off`, not `{other}`")),
    };
    let mut s = store.lock().expect("value store lock");
    let group = s.begin_group(origin.clone(), label);
    let mut failed = None;
    for (path, value) in writes {
        if let Err(e) = s.set(Set::new(&path, Change::To(value), origin.clone()).gesture(group)) {
            failed = Some(e.to_string());
        }
    }
    s.end(group);
    match failed {
        Some(e) => Err(e),
        None => Ok(answer),
    }
}

/// [`LOOK_FIT_SELECTION`]: one camera look fitted from the photos `images`
/// names (refused at once when the library knows them to be of several
/// cameras), run on a thread of the node's as a `calibration` job: its
/// progress counts the photos read, the jobs centre cancels it before
/// the next photo. The look written into every photo (and with
/// `default`, kept as the camera's default) is one step of the library's
/// history; a fit that finds no look writes nothing and says why in the
/// job's log. Answers the job's id at once.
fn fit_selection<S: Store + Send + 'static>(
    node: &Arc<Node<S>>,
    history: &LibraryHistory,
    jobs: &helios_core::jobs::Jobs,
    args: &Value,
    origin: &helios_shell::commands::Origin,
) -> Result<Value, String> {
    use std::sync::atomic::{AtomicBool, Ordering};

    use helios_core::jobs::{Progress, Start};
    use node::look::together;

    let images: Vec<String> = args["images"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    if images.is_empty() {
        return Err("no photo to fit: select photos of one camera in the Library".into());
    }
    let ids = images
        .iter()
        .map(|h| helios_vault::ImageId::from_hex(h))
        .collect::<Result<Vec<_>, _>>()?;
    let cameras = together::cameras(&node.vault, &ids).map_err(|m| m.english)?;
    together::one_camera(&cameras).map_err(|m| m.english)?;
    let count = images.len();
    let id = format!(
        "camera-look-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis())
    );
    let title = Message::new(
        "gui-look-fit-selection-job",
        "Fit a camera look from { $count } photos",
    )
    .with("count", count)
    .english;
    let report = jobs.start(Start::new(&id, &title, "calibration").total(count as u64));
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let cancel = cancel.clone();
        report.on_cancel(move || cancel.store(true, Ordering::SeqCst));
    }
    let params = serde_json::json!({
        "images": images,
        "together": true,
        "default": args["default"] == true,
        "dry_run": args["dry_run"] == true,
    });
    let (node, history, who) = (node.clone(), history.clone(), crate::persist::who(origin));
    std::thread::Builder::new()
        .name("helios-camera-look".into())
        .spawn(move || {
            let step = |done: usize, total: usize| {
                report.progress(Progress::new().counts(done as u64, total as u64));
                !cancel.load(Ordering::SeqCst)
            };
            let now = seconds() as u64;
            match node::look::fit_with_progress(
                &node.vault,
                &params,
                &who,
                now,
                &node::look::shot,
                &step,
            ) {
                Ok(done) => {
                    let a = &done.answer;
                    report.log(fit_summary(a).english);
                    if !done.changes.is_empty() {
                        let name = a["camera"].as_str().unwrap_or_default();
                        history.record(label(LOOK_FIT, name), done.changes, seconds());
                    }
                    report.finished();
                }
                Err(m) if m.key == "look-fit-cancelled" => report.cancelled(),
                Err(m) => report.failed(&m.english),
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "job": id, "photos": count }))
}

/// What a fit of photos together did, in words.
fn fit_summary(answer: &Value) -> Message {
    let refusals: Vec<String> = answer["images"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i["refused"]["words"].as_str().map(String::from))
        .collect();
    match answer["look"]["held_out"]["mean"].as_f64() {
        Some(held) => Message::new(
            "gui-look-fit-selection-done",
            "Camera look of { $camera } fitted from { $fitted } of { $photos } photos: Delta E { $held } (default { $baseline })",
        )
        .with("camera", answer["camera"].as_str().unwrap_or_default())
        .with("fitted", answer["fitted"].as_u64().unwrap_or(0))
        .with("photos", answer["photos"].as_u64().unwrap_or(0))
        .with("held", format!("{held:.1}"))
        .with(
            "baseline",
            format!(
                "{:.1}",
                answer["look"]["baseline"]["mean"].as_f64().unwrap_or(0.0)
            ),
        ),
        None => Message::new(
            "gui-look-fit-selection-none",
            "No camera look fitted: { $reasons }",
        )
        .with("reasons", refusals.join("; ")),
    }
}

/// The Camera look entry's header in the Parameters panel, under the
/// `cameratone` node's name (`docs/design/camera-look.md`, section 3.4):
/// what the look was fitted to and how well (the held-out Delta E against
/// the default render's), or the gate that refused it in words; a switch
/// turning it on and off; "Fit to the camera's JPEG" (Refit once fitted).
/// Each is [`LOOK_CHOOSE`], one undoable step of the edit. Nothing for
/// another node.
pub fn look_header_ui(
    ui: &mut egui::Ui,
    store: &std::sync::Mutex<helios_values::Store>,
    node: &str,
) {
    use helios_values::node_path;
    let (fitted, on, colour_off, held, baseline, refusal, image, compare, default) = {
        let s = store.lock().expect("value store lock");
        if look_node(&s).as_deref() != Some(node) {
            return;
        }
        let float = |p: &str| s.get(&node_path(node, p)).and_then(|e| e.value.as_f64());
        let charted = chart_node(&s).is_some_and(|c| !is_bypassed(&s, &c));
        let colour = module_node(&s, helios_calibrate::look::edit::COLOUR);
        let on = !is_bypassed(&s, node);
        (
            is_fitted(&s, node),
            on,
            on && charted && colour.is_some_and(|c| is_bypassed(&s, &c)),
            float("held_out"),
            float("baseline"),
            read_refusal(&s),
            read_offer(&s).is_some_and(|o| o["image"].is_string()),
            Compare {
                file: compared_path(&s).is_some(),
                on: is_comparing(&s)
                    && s.get(crate::viewport::COMPARE)
                        .and_then(|e| e.value.as_bool())
                        .unwrap_or(false),
                missing: read_missing(&s),
            },
            CameraDefault::read(&s, node),
        )
    };
    let ctx = ui.ctx().clone();
    let text = |m: Message| crate::i18n::text(&ctx, m);
    let score = |v: Option<f64>| v.map_or_else(|| "?".to_string(), |v| format!("{v:.1}"));
    let status = if !refusal.is_empty() {
        Message::new("gui-look-header-refused", "Not fitted: { $reason }").with("reason", refusal)
    } else if fitted {
        Message::new(
            "gui-look-header-fitted",
            "Fitted to the camera's JPEG: Delta E { $held } (default { $baseline })",
        )
        .with("held", score(held))
        .with("baseline", score(baseline))
    } else {
        Message::new(
            "gui-look-header-none",
            "Not fitted yet: the look starts from the camera's own JPEG",
        )
    };
    ui.weak(text(status));
    if colour_off {
        ui.weak(text(Message::new(
            "gui-look-header-chart",
            "Colour half off: the chart profile rules the colours",
        )));
    }
    let run = |look: &str| {
        crate::dialogs::request(
            &ctx,
            crate::dialogs::Request::Run(
                LOOK_CHOOSE.into(),
                serde_json::json!({ "look": look }),
                helios_shell::commands::Origin::User,
            ),
        );
    };
    ui.horizontal(|ui| {
        let name = text(Message::new("gui-look-header-switch", "Camera look"));
        let mut checked = on && fitted;
        let r = ui.add_enabled(fitted, egui::Checkbox::new(&mut checked, &name));
        egui_pop::core::a11y::name(&r, egui::Role::CheckBox, &name);
        crate::name_rect(&ctx, "helios.cameralook.switch", r.rect);
        if r.changed() {
            run(if checked { "on" } else { "off" });
        }
        let fit = if fitted {
            text(Message::new("gui-look-header-refit", "Refit"))
        } else {
            text(Message::new(
                "gui-look-header-fit",
                "Fit to the camera's JPEG",
            ))
        };
        let r = ui.add_enabled(image, egui::Button::new(&fit).small());
        egui_pop::core::a11y::name(&r, egui::Role::Button, &fit);
        let r = r.on_hover_text(text(Message::new(
            "gui-look-header-fit-about",
            "Fits a matrix, a hue and saturation table and tone curves to the camera's embedded JPEG, checked on pixels it did not fit",
        )));
        crate::name_rect(&ctx, "helios.cameralook.fit", r.rect);
        if r.clicked() {
            run("fit");
        }
    });
    compare_ui(ui, store, &compare);
    if let Some(default) = default {
        default_ui(ui, store, &default, on && fitted, fitted);
    }
}

/// What the header knows of the camera's default look.
struct CameraDefault {
    /// The camera's key, and its make and model as said.
    camera: String,
    name: String,
    /// What the camera's default look was fitted to; none: no default.
    preview: Option<String>,
    /// What the open photo's look was fitted to.
    current: String,
}

impl CameraDefault {
    /// Read from the offer and the `cameratone` node `tone`; none for a
    /// photo without a camera.
    fn read(store: &helios_values::Store, tone: &str) -> Option<Self> {
        let offer = read_offer(store)?;
        let camera = offer["camera"].as_str()?.to_string();
        let name = [offer["make"].as_str(), offer["model"].as_str()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        let current = match store
            .get(&helios_values::node_path(tone, "preview"))
            .map(|e| &e.value)
        {
            Some(helios_core::Value::Text(t)) => t.clone(),
            _ => String::new(),
        };
        Some(Self {
            camera,
            name,
            preview: offer["look_default"]["preview"].as_str().map(String::from),
            current,
        })
    }

    /// The open photo's look is the camera's default.
    fn in_use(&self, on: bool) -> bool {
        on && !self.current.is_empty() && self.preview.as_deref() == Some(self.current.as_str())
    }
}

/// A node's parameters as the store holds them.
fn node_params(store: &helios_values::Store, node: &str) -> helios_core::Params {
    let prefix = format!("node/{node}/");
    store
        .list(&prefix)
        .into_iter()
        .filter_map(|(path, e)| {
            let param = path.strip_prefix(&prefix)?;
            (!param.contains('/')).then(|| (param.to_string(), e.value.clone()))
        })
        .fold(helios_core::Params::new(), |p, (name, value)| {
            p.with(&name, value)
        })
}

/// The header's per-camera default, as raw editors keep develop defaults
/// per camera model: whether the open photo's look is the camera's
/// default (new photos of the camera open with it), "Save as the
/// camera's default" (the look as the edit holds it now) and "Clear the
/// default". Each is `look.camera.setDefault`, one step of the library's
/// history.
fn default_ui(
    ui: &mut egui::Ui,
    store: &std::sync::Mutex<helios_values::Store>,
    default: &CameraDefault,
    on: bool,
    fitted: bool,
) {
    use helios_calibrate::look::edit::{COLOUR, TONE};
    let ctx = ui.ctx().clone();
    let text = |m: Message| crate::i18n::text(&ctx, m);
    let camera = if default.name.is_empty() {
        default.camera.clone()
    } else {
        default.name.clone()
    };
    if default.in_use(on) {
        ui.weak(text(
            Message::new(
                "gui-look-header-default-in-use",
                "The default look of { $camera }: its new photos open with it",
            )
            .with("camera", camera.clone()),
        ));
    } else if default.preview.is_some() {
        ui.weak(text(
            Message::new(
                "gui-look-header-default-other",
                "{ $camera } has another default look: its new photos open with that one",
            )
            .with("camera", camera.clone()),
        ));
    }
    let run = |args: Value| {
        crate::dialogs::request(
            &ctx,
            crate::dialogs::Request::Run(
                LOOK_SET_DEFAULT.into(),
                args,
                helios_shell::commands::Origin::User,
            ),
        );
    };
    // Wrapped: in a narrow panel "Clear the default" goes to the next
    // line instead of past the panel's edge, where it cannot be clicked.
    ui.horizontal_wrapped(|ui| {
        let save = text(Message::new(
            "gui-look-header-default-save",
            "Save as the camera's default",
        ));
        let r = ui.add_enabled(
            fitted && !default.in_use(on),
            egui::Button::new(&save).small(),
        );
        egui_pop::core::a11y::name(&r, egui::Role::Button, &save);
        let r = r.on_hover_text(text(
            Message::new(
                "gui-look-header-default-save-about",
                "New photos of { $camera } open with this camera look; photos already edited keep theirs",
            )
            .with("camera", camera.clone()),
        ));
        crate::name_rect(&ctx, "helios.cameralook.default", r.rect);
        if r.clicked() {
            let look = {
                let s = store.lock().expect("value store lock");
                let tone = look_node(&s).map(|n| node_params(&s, &n));
                let colour = module_node(&s, COLOUR).map(|n| node_params(&s, &n));
                serde_json::json!({ COLOUR: colour.unwrap_or_default(), TONE: tone.unwrap_or_default() })
            };
            run(serde_json::json!({ "camera": default.camera, "look": look }));
        }
        if default.preview.is_some() {
            let clear = text(Message::new(
                "gui-look-header-default-clear",
                "Clear the default",
            ));
            let r = ui.small_button(&clear);
            egui_pop::core::a11y::name(&r, egui::Role::Button, &clear);
            crate::name_rect(&ctx, "helios.cameralook.default.clear", r.rect);
            if r.clicked() {
                run(serde_json::json!({ "camera": default.camera, "clear": true }));
            }
        }
    });
}

/// What the header knows of the compare with the camera's JPEG.
struct Compare {
    /// The open photo's file is at hand.
    file: bool,
    /// The camera's JPEG is the before of the split shown.
    on: bool,
    /// Why it could not be shown (empty: nothing to say).
    missing: String,
}

/// The header's "Compare with the camera's JPEG" switch: the viewport's
/// before and after split with the camera's embedded JPEG as the before,
/// left of the split, as raw editors compare with a before (a view, not
/// an edit step); why it could not be shown, in words.
fn compare_ui(
    ui: &mut egui::Ui,
    store: &std::sync::Mutex<helios_values::Store>,
    compare: &Compare,
) {
    let ctx = ui.ctx().clone();
    let text = |m: Message| crate::i18n::text(&ctx, m);
    let name = text(Message::new(
        "gui-look-header-compare",
        "Compare with the camera's JPEG",
    ));
    let mut checked = compare.on;
    let r = ui.add_enabled(compare.file, egui::Checkbox::new(&mut checked, &name));
    egui_pop::core::a11y::name(&r, egui::Role::CheckBox, &name);
    let r = r.on_hover_text(text(Message::new(
        "gui-look-header-compare-about",
        "Shows the camera's embedded JPEG left of the split and the render right of it; drag the split to move it",
    )));
    crate::name_rect(&ctx, "helios.cameralook.compare", r.rect);
    if r.changed() {
        set_compare(store, checked, "");
    }
    let missing = match compare.missing.as_str() {
        "" => None,
        NO_JPEG => Some(Message::new(
            "gui-look-header-compare-no-jpeg",
            "No camera JPEG to compare: the raw carries none",
        )),
        _ => Some(Message::new(
            "gui-look-header-compare-not-raw",
            "No camera JPEG to compare: the photo is not a raw",
        )),
    };
    if let Some(m) = missing {
        ui.weak(text(m));
    }
}
