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
//! - `calibrate.choose`: the open photo's profile, chosen in the module's
//!   header, one step of the edit's history (not of the library's).
//!
//! When a raw opens for the first time, its camera's default profile is
//! put on it ([`on_open`]); a default that picks none says why in the
//! status bar and leaves the photo as it is.
//!
//! Each run is announced on the node's bus with who ran it
//! (`control.command`). A save, a deletion, a default set or an apply is
//! one step of the library's history, so Ctrl+Z takes it back. Without a library, the chart's dry
//! run is still answered; the profile commands say they need one.

use std::sync::Arc;

use egui_pop::api::history::Label;
use helios_core::{Bus, Message};
use helios_node_core::calibrate::{
    self as node, APPLY, CHART, DELETE, EXPORT, GET, IMPORT, LIST, SET_DEFAULT,
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
    let m = if id == DELETE {
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
    if id == SET_DEFAULT {
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
                    if matches!(id, CHART | DELETE | IMPORT) {
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
    let spec = CommandSpec::new(
        CHOOSE,
        "Sets the open photo's calibration profile (`profile`: an id of its camera's, or `none`), one step of its edit",
        "Calibration",
    );
    shell.register(spec, move |args, origin| match &node {
        Some(node) => choose(&node.vault, &values, args, origin),
        None => Err("no library open: calibration profiles live in one".into()),
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

/// Declares [`OFFER`]: transient, never undone.
pub fn declare(store: &mut helios_values::Store) {
    store.declare(
        OFFER,
        helios_values::Descriptor::text("calibration profiles offered", "").transient(),
    );
}

/// The `chartprofile` node of the network the store holds, by name.
pub fn chart_node(store: &helios_values::Store) -> Option<String> {
    store
        .list("node/")
        .into_iter()
        .find(|(_, e)| e.descriptor.module.as_deref() == Some(helios_calibrate::apply::MODULE))
        .and_then(|(p, _)| {
            p.strip_prefix("node/")?
                .rsplit_once('/')
                .map(|(n, _)| n.to_string())
        })
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
    serde_json::json!({
        "camera": camera,
        "make": make,
        "model": model,
        "light": light,
        "profiles": profiles,
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
    let new = offer(
        vault,
        &text("camera"),
        &text("make"),
        &text("model"),
        old["light"].clone(),
    );
    set_offer(store, Some(&new));
}

/// A photo opened in the viewport (`graph`, its network built): the
/// module's header offered the camera's profiles and, opened for the
/// first time, the camera's default profile put on it
/// ([`node::default_on_open`]). Answers what to tell: the profile
/// applied, or why the default picked none; never an error dialog.
pub fn on_open<S: Store>(
    vault: &Vault<S>,
    image: helios_vault::ImageId,
    graph: &helios_core::Graph,
    store: &std::sync::Mutex<helios_values::Store>,
) -> Option<Message> {
    let Some((photo, make, model)) = photo_of(graph) else {
        set_offer(store, None);
        return None;
    };
    let light = photo.light.map_or(Value::Null, |l| serde_json::json!(l));
    set_offer(
        store,
        Some(&offer(vault, &photo.camera, &make, &model, light)),
    );
    let now = seconds() as u64;
    match node::default_on_open(vault, image, &photo, crate::persist::WHO, now) {
        Ok(node::OnOpen::Applied { name, .. }) => Some(
            Message::new(
                "gui-calibration-default-applied",
                "Calibration profile “{ $name }” applied: the default of { $camera }",
            )
            .with("name", name)
            .with("camera", photo.camera),
        ),
        Ok(node::OnOpen::Refused(reason)) => Some(
            Message::new(
                "gui-calibration-default-none",
                "No calibration profile applied: { $reason }",
            )
            .with("reason", reason.english),
        ),
        Ok(_) => None,
        Err(e) => Some(e),
    }
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
