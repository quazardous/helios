//! Helios reads the fixtures' mask blobs as darktable does: the flexi
//! masks' darktable decodes them (`darktable --harvest-masks-xmp`, run by
//! the bench box's mask-parity job, kept as `tests/masks/harvest.json`),
//! and every field Helios reads must say the same. A change of format
//! (the harvest's version, a masks version Helios does not read) fails
//! here. Skipped, saying so, until a harvest is kept.

use std::path::Path;

use helios_parity::masks::fixtures;
use helios_xmp::drawn::{self, Frame, Mapped};
use helios_xmp::{Layouts, Sidecar};
use serde_json::Value;

/// The harvest format this test reads.
const HARVEST_FORMAT: i64 = 1;

/// Every form object of the harvest, wherever it sits.
fn forms<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            if m.contains_key("formid") && m.contains_key("type") && m.contains_key("version") {
                out.push(v);
            }
            for x in m.values() {
                forms(x, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| forms(x, out)),
        _ => {}
    }
}

fn close(a: f64, b: &Value, what: &str) {
    let b = b.as_f64().unwrap_or_else(|| panic!("{what}: {b}"));
    assert!(
        (a - b).abs() <= 1e-6 * a.abs().max(1.0),
        "{what}: Helios {a}, darktable {b}"
    );
}

#[test]
fn helios_reads_the_fixtures_as_darktable_does() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/masks/harvest.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!(
            "skipped: no {} (the mask-parity job writes it)",
            path.display()
        );
        return;
    };
    let harvest: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(harvest["format"], "darktable-mask-harvest");
    assert_eq!(
        harvest["format_version"], HARVEST_FORMAT,
        "a new harvest format"
    );
    assert_eq!(harvest["current_masks_version"], 7, "a new masks version");
    let mut theirs = Vec::new();
    forms(&harvest, &mut theirs);
    assert!(!theirs.is_empty(), "no forms harvested");

    let layouts = Layouts::built_in();
    let mut ours = Vec::new();
    for f in fixtures() {
        let sidecar = Sidecar::parse(&f.sidecar(&layouts).unwrap()).unwrap();
        ours.extend(sidecar.masks);
    }
    let frame = Frame::new(1.0, 1.0);
    let mut checked = 0;
    for t in theirs {
        let id = t["formid"].as_i64().unwrap();
        let Some(form) = ours.iter().find(|f| f.id == id) else {
            continue;
        };
        assert_eq!(
            t["version"].as_u64(),
            Some(u64::from(form.version)),
            "form {id}"
        );
        assert!(t.get("points_error").is_none(), "form {id}: {t}");
        let points = t["points"].as_array().unwrap();
        let word = |k: usize, i: usize, stride: usize| {
            let at = k * stride + i * 4;
            f64::from(f32::from_le_bytes(
                form.points[at..at + 4].try_into().unwrap(),
            ))
        };
        match drawn::map(form, &frame) {
            Mapped::Group(g) => {
                // The marker is a point too, first.
                let members: Vec<&Value> = points
                    .iter()
                    .filter(|p| {
                        p["state"].as_i64().unwrap() & i64::from(drawn::state::GROUP_MARKER) == 0
                    })
                    .collect();
                assert_eq!(members.len(), g.members.len(), "group {id}");
                for (m, p) in g.members.iter().zip(members) {
                    assert_eq!(p["formid"].as_i64(), Some(m.form));
                    let st = p["state"].as_i64().unwrap() as i32;
                    assert_eq!(st & drawn::state::INVERSE != 0, m.invert);
                    close(m.opacity, &p["opacity"], "opacity");
                    if form.version >= 7 {
                        let r = &p["refinement"];
                        assert_eq!(r["enabled"].as_i64(), Some(i64::from(m.refinement.scope)));
                        close(
                            f64::from(m.refinement.blur_radius),
                            &r["blur_radius"],
                            "blur",
                        );
                    }
                }
                if let Some(marker) = &g.marker {
                    let p = points
                        .iter()
                        .find(|p| {
                            p["state"].as_i64().unwrap() & i64::from(drawn::state::GROUP_MARKER)
                                != 0
                        })
                        .expect("a marker");
                    close(marker.opacity, &p["group_opacity"], "group opacity");
                }
            }
            Mapped::Shape(_) => {
                let p = &points[0];
                let stride = form.points.len() / form.count as usize;
                let name = t["type_name"].as_str().unwrap_or("");
                let fields: &[(&str, usize)] = match name {
                    "circle" => &[
                        ("center.0", 0),
                        ("center.1", 1),
                        ("radius", 2),
                        ("border", 3),
                    ],
                    "ellipse" => &[
                        ("center.0", 0),
                        ("center.1", 1),
                        ("radius.0", 2),
                        ("radius.1", 3),
                        ("rotation", 4),
                        ("border", 5),
                    ],
                    "gradient" => &[
                        ("anchor.0", 0),
                        ("anchor.1", 1),
                        ("rotation", 2),
                        ("compression", 3),
                        ("curvature", 5),
                    ],
                    "path" => &[
                        ("corner.0", 0),
                        ("corner.1", 1),
                        ("ctrl1.0", 2),
                        ("ctrl2.1", 5),
                        ("border.0", 6),
                    ],
                    "brush" => &[
                        ("corner.0", 0),
                        ("ctrl1.1", 3),
                        ("border.1", 7),
                        ("density", 8),
                        ("hardness", 9),
                    ],
                    other => panic!("form {id}: type {other}"),
                };
                for (field, i) in fields {
                    let v = match field.split_once('.') {
                        Some((k, n)) => &p[k][n.parse::<usize>().unwrap()],
                        None => &p[*field],
                    };
                    close(word(0, *i, stride), v, &format!("form {id} {field}"));
                }
            }
            Mapped::Skipped { problem, .. } => panic!("form {id}: {problem:?}"),
        }
        checked += 1;
    }
    assert!(checked >= ours.len() / 2, "only {checked} forms matched");
}
