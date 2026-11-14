//! A camera look written into an edit (`docs/design/camera-look.md`,
//! sections 3.2 and 3.3): the fit's numbers copied into the
//! `cameracolour` and `cameratone` nodes' parameters, so a render never
//! refits.
//!
//! - [`with_look`] turns the look on: `cameratone` on and `sigmoid` off
//!   (the look's tone curve takes the display transform's place),
//!   `cameracolour` on unless a chart profile is on: the chart rules the
//!   colour, and the look's tone stays on top of it (Q3 A).
//! - [`with_params`] does the same from the nodes' parameters (a
//!   camera's default look).
//! - [`without_look`] turns both halves off and `sigmoid` back on, their
//!   parameters kept.
//! - [`crate::apply::with_profile`] turns `cameracolour` off for the same
//!   reason.

use helios_core::{Params, Value as Param};
use serde_json::{Value, json};

use super::Look;
use crate::apply;

/// The colour half's module, and the node it runs in.
pub const COLOUR: &str = "cameracolour";

/// The tone half's module, and the node it runs in.
pub const TONE: &str = "cameratone";

/// The display transform the tone half replaces.
pub const DISPLAY: &str = "sigmoid";

/// The `cameracolour` node's parameters for `look`, fitted to the preview
/// whose hash is `preview`.
pub fn colour_params(look: &Look, preview: &str) -> Params {
    Params::new()
        .with(
            "matrix",
            Param::List(look.matrix.iter().flatten().copied().collect()),
        )
        .with(
            "lut",
            Param::List(
                look.table
                    .as_ref()
                    .map(super::HueSatTable::to_param)
                    .unwrap_or_default(),
            ),
        )
        .with("fitter", Param::Int(i64::from(look.version)))
        .with("preview", Param::Text(preview.into()))
}

/// The `cameratone` node's parameters for `look`, fitted to the preview
/// whose hash is `preview`.
pub fn tone_params(look: &Look, preview: &str) -> Params {
    Params::new()
        .with(
            "tone",
            Param::List(look.tone.knots.iter().flatten().copied().collect()),
        )
        .with(
            "chroma",
            Param::List(
                look.chroma
                    .as_ref()
                    .map(|c| c.nodes.to_vec())
                    .unwrap_or_default(),
            ),
        )
        .with("fitter", Param::Int(i64::from(look.version)))
        .with("preview", Param::Text(preview.into()))
        .with("held_out", Param::Float(look.report.held_out.mean))
        .with("baseline", Param::Float(look.report.baseline.mean))
}

/// `edit` (an image's edit, `null` for none yet) with `look` on: its two
/// nodes set, `cameratone` on and `sigmoid` off, `cameracolour` on unless
/// a chart profile is on; the other nodes kept as they are.
pub fn with_look(edit: &Value, look: &Look, preview: &str) -> Value {
    with_params(
        edit,
        &colour_params(look, preview),
        &tone_params(look, preview),
    )
}

/// [`with_look`] from the two nodes' parameters as they were written
/// (a camera's default look, kept as its nodes' parameters).
pub fn with_params(edit: &Value, colour: &Params, tone: &Params) -> Value {
    let charted = apply::applied(edit).is_some();
    let mut edit = base(edit);
    set(&mut edit, COLOUR, colour, charted);
    set(&mut edit, TONE, tone, false);
    bypass(&mut edit, DISPLAY, true);
    edit
}

/// `edit` with the look off: both nodes bypassed (their parameters kept)
/// and `sigmoid` back on; the same edit when it has no look.
pub fn without_look(edit: &Value) -> Value {
    let mut edit = base(edit);
    let has = |e: &Value, name: &str| {
        e["modules"]
            .as_array()
            .is_some_and(|m| m.iter().any(|m| m["name"] == name))
    };
    if !has(&edit, COLOUR) && !has(&edit, TONE) {
        return edit;
    }
    for name in [COLOUR, TONE] {
        if has(&edit, name) {
            bypass(&mut edit, name, true);
        }
    }
    bypass(&mut edit, DISPLAY, false);
    edit
}

/// Whether an edit's look is on: its tone half, the colour half on or not.
pub fn applied(edit: &Value) -> bool {
    is_on(edit, TONE)
}

/// Whether `node` is in `edit` and on.
pub fn is_on(edit: &Value, node: &str) -> bool {
    edit["modules"]
        .as_array()
        .is_some_and(|m| m.iter().any(|m| m["name"] == node && m["bypassed"] != true))
}

/// `edit` with `node` set to `params`, bypassed or not.
fn set(edit: &mut Value, node: &str, params: &Params, bypassed: bool) {
    let mut entry = json!({
        "name": node,
        "module": node,
        "params": serde_json::to_value(params).expect("parameters serialise"),
    });
    if bypassed {
        entry["bypassed"] = Value::Bool(true);
    }
    let modules = edit["modules"].as_array_mut().expect("an array");
    match modules.iter_mut().find(|m| m["name"] == node) {
        Some(m) => *m = entry,
        None => modules.push(entry),
    }
}

/// `edit` with `node` bypassed or not, its parameters kept; added without
/// parameters when the edit has none for it.
pub(crate) fn bypass(edit: &mut Value, node: &str, bypassed: bool) {
    let modules = edit["modules"].as_array_mut().expect("an array");
    let i = modules
        .iter()
        .position(|m| m["name"] == node)
        .unwrap_or_else(|| {
            modules.push(json!({ "name": node, "module": node }));
            modules.len() - 1
        });
    let map = modules[i]
        .as_object_mut()
        .expect("a module entry is an object");
    if bypassed {
        map.insert("bypassed".into(), Value::Bool(true));
    } else {
        map.remove("bypassed");
    }
}

fn base(edit: &Value) -> Value {
    let mut edit = if edit.is_object() {
        edit.clone()
    } else {
        json!({})
    };
    if !edit["modules"].is_array() {
        edit["modules"] = json!([]);
    }
    edit
}
