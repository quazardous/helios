//! History items Helios cannot apply: kept in their place with their raw
//! params, reported once, and never in the way of the items around them.

use std::fmt::Write as _;

use helios_core::Value as Param;
use helios_xmp::blend::{self, Blend};
use helios_xmp::drawn::Frame;
use helios_xmp::{BlendIssue, Layouts, Reason, Sidecar, encode, import, import_text, import_with};
use serde_json::{Value, json};

/// darktable's params encoding of `values` for the newest version of `op`.
fn params(op: &str, values: &Value) -> (String, u32) {
    let layouts = Layouts::built_in();
    let version = *layouts.versions(op).iter().max().unwrap();
    let hex = encode(layouts.get(op, version).unwrap(), values)
        .unwrap()
        .iter()
        .fold(String::new(), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        });
    (hex, version)
}

/// An `rdf:li` of the history: `attrs` are written as they are.
fn li(num: u32, op: &str, attrs: &str) -> String {
    format!(
        r#"<rdf:li darktable:num="{num}" darktable:operation="{op}" darktable:enabled="1" {attrs}/>"#
    )
}

fn exposure(num: u32, ev: f64, extra: &str) -> String {
    let (hex, version) = params("exposure", &json!({ "exposure": ev }));
    li(
        num,
        "exposure",
        &format!(r#"darktable:modversion="{version}" darktable:params="{hex}" {extra}"#),
    )
}

fn xmp(items: &[String], end: usize) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/"
   darktable:xmp_version="5" darktable:iop_order_version="4" darktable:history_end="{end}">
   <darktable:history><rdf:Seq>
{}
   </rdf:Seq></darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#,
        items.join("\n")
    )
}

fn ev(node: &helios_xmp::Node) -> f64 {
    match node.params.get("exposure") {
        Some(Param::Float(v)) => *v,
        other => panic!("{other:?}"),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    })
}

/// Blend params of an older version whose first word, the mask mode, is
/// `mode`.
fn old_blend(mode: u32) -> String {
    let mut bytes = mode.to_le_bytes().to_vec();
    bytes.extend_from_slice(&[0x11; 60]);
    hex(&bytes)
}

/// The attributes of an item blending with `b` (version 14).
fn blending(b: &Blend) -> String {
    format!(
        r#"darktable:blendop_version="14" darktable:blendop_params="{}""#,
        hex(&b.write())
    )
}

/// A form of the masks history (version 6), its points given as fields.
fn form(id: i64, kind: u32, fields: &[[u8; 4]]) -> String {
    let n = if kind == 4 { fields.len() / 4 } else { 1 };
    format!(
        r#"<rdf:li darktable:mask_num="0" darktable:mask_id="{id}" darktable:mask_type="{kind}" darktable:mask_name="form {id}" darktable:mask_version="6" darktable:mask_points="{}" darktable:mask_nb="{n}" darktable:mask_src="0000000000000000"/>"#,
        hex(&fields.concat())
    )
}

/// A group of `members` (form, state), each at full opacity.
fn group(id: i64, members: &[(i64, i32)]) -> String {
    let fields: Vec<[u8; 4]> = members
        .iter()
        .flat_map(|(f, s)| {
            [
                (*f as i32).to_le_bytes(),
                (id as i32).to_le_bytes(),
                s.to_le_bytes(),
                1.0f32.to_le_bytes(),
            ]
        })
        .collect();
    form(id, 4, &fields)
}

fn circle(id: i64) -> String {
    let f = |v: f32| v.to_le_bytes();
    form(id, 1, &[f(0.5), f(0.5), f(0.1), f(0.05)])
}

fn with_masks(xml: &str, forms: &[String]) -> String {
    xml.replace(
        "   <darktable:history>",
        &format!(
            "   <darktable:masks_history><rdf:Seq>{}</rdf:Seq></darktable:masks_history>\n   <darktable:history>",
            forms.concat()
        ),
    )
}

#[test]
fn an_unknown_module_or_version_is_kept_and_the_rest_applies() {
    let items = [
        exposure(0, 1.0, r#"darktable:multi_priority="0""#),
        li(
            1,
            "frobnicate",
            r#"darktable:modversion="3" darktable:params="0a0b0c""#,
        ),
        // A params version Helios has no layout for, after the others.
        li(
            2,
            "sigmoid",
            r#"darktable:modversion="99" darktable:params="00""#,
        ),
        exposure(
            3,
            0.5,
            r#"darktable:multi_priority="1" darktable:multi_name="1""#,
        ),
    ];
    let text = xmp(&items, items.len());
    let (sidecar, imported) = import_text(&text, &Layouts::built_in()).unwrap();
    let r = &imported.report;
    // Both exposures apply, each with its own value: nothing shifted.
    let nodes: Vec<_> = imported
        .nodes
        .iter()
        .map(|n| (n.name.as_str(), ev(n)))
        .collect();
    assert_eq!(nodes, [("exposure", 1.0), ("exposure 1", 0.5)], "{r:#?}");
    let not: Vec<_> = r
        .not_applied
        .iter()
        .map(|n| (n.label.as_str(), &n.reason))
        .collect();
    assert_eq!(not.len(), 2, "{r:#?}");
    assert!(not.contains(&("frobnicate v3", &Reason::UnknownModule)));
    assert!(not.iter().any(|(label, reason)| *label == "sigmoid v99"
        && matches!(reason, Reason::UnsupportedVersion { known } if known.contains(&3))));
    assert_eq!(r.items, 4);

    // One line says it, once, for the whole import.
    let message = r.summary().unwrap();
    // By key, for an interface to translate; each reason too.
    assert_eq!(message.key, "xmp-not-applied");
    assert_eq!(message.args["count"], helios_core::Arg::Number(2.0));
    let summary = message.english;
    assert!(
        summary.starts_with("2 of 4 history items not applied"),
        "{summary}"
    );
    assert!(
        summary.contains("frobnicate v3 (no Helios module)"),
        "{summary}"
    );
    assert!(
        summary.contains("sigmoid v99 (version not supported"),
        "{summary}"
    );

    // The edit keeps every item, raw params included, and the sidecar.
    let edit = imported.edit();
    let history = edit["darktable"]["history"].as_array().unwrap();
    assert_eq!(history.len(), sidecar.history.len());
    assert_eq!(history[1]["status"], "not_applied");
    assert_eq!(history[1]["reason"]["kind"], "unknown_module");
    assert_eq!(history[1]["attributes"]["darktable:params"], "0a0b0c");
    assert_eq!(history[1]["version"], 3);
    assert_eq!(history[3]["status"], "applied");
    assert_eq!(history[3]["node"], "exposure 1");
    assert_eq!(edit["darktable"]["sidecar"], text);
    // The kept text reads back to the same edit.
    let again = import_text(
        edit["darktable"]["sidecar"].as_str().unwrap(),
        &Layouts::built_in(),
    )
    .unwrap()
    .1;
    assert_eq!(again.edit(), edit);
}

#[test]
fn an_item_that_cannot_be_read_does_not_stop_the_import_or_move_the_end() {
    let items = [
        exposure(0, 1.0, ""),
        // Params that are not hex, and no version.
        li(
            1,
            "exposure",
            r#"darktable:params="zz" darktable:multi_priority="1""#,
        ),
        exposure(2, 2.0, r#"darktable:multi_priority="2""#),
        // Undone: after the end.
        exposure(3, 3.0, ""),
    ];
    let sidecar = Sidecar::parse(&xmp(&items, 3)).unwrap();
    assert_eq!(sidecar.history.len(), 4);
    assert!(sidecar.history[1].unreadable.is_some());
    let imported = import(&sidecar, &Layouts::built_in());
    let values: Vec<f64> = imported.nodes.iter().map(ev).collect();
    // The end still falls after the third item: 3.0 stays undone.
    assert_eq!(values, [1.0, 2.0]);
    let r = &imported.report;
    assert_eq!(r.not_applied.len(), 1, "{r:#?}");
    assert!(matches!(r.not_applied[0].reason, Reason::Unreadable { .. }));
    let history = imported.darktable["history"].as_array().unwrap();
    assert_eq!(history[3]["status"], "undone");
    assert_eq!(history[1]["attributes"]["darktable:params"], "zz");
}

#[test]
fn a_blend_helios_cannot_do_is_kept_not_applied_to_the_whole_image() {
    let mut multiply = Blend::drawn(0);
    multiply.mask_mode = blend::mode::ENABLED;
    multiply.blend_mode = 0x04;
    let mut parametric = Blend::drawn(10);
    parametric.mask_mode |= blend::mode::CONDITIONAL;
    let items = [
        exposure(
            0,
            1.0,
            &blending(&Blend {
                mask_mode: 0,
                ..Blend::drawn(0)
            }),
        ),
        // A drawn mask, read without the image's size.
        exposure(
            1,
            2.0,
            &format!(
                r#"darktable:multi_priority="1" {}"#,
                blending(&Blend::drawn(10))
            ),
        ),
        // Blend params from a darktable newer than Helios.
        exposure(
            2,
            3.0,
            &format!(
                r#"darktable:multi_priority="2" darktable:blendop_version="16" darktable:blendop_params="{}""#,
                old_blend(1)
            ),
        ),
        // darktable reads all-zero blend params as no blending.
        exposure(
            3,
            4.0,
            r#"darktable:multi_priority="3" darktable:blendop_version="15" darktable:blendop_params="00000000""#,
        ),
        exposure(
            4,
            5.0,
            &format!(r#"darktable:multi_priority="4" {}"#, blending(&multiply)),
        ),
        exposure(
            5,
            6.0,
            &format!(r#"darktable:multi_priority="5" {}"#, blending(&parametric)),
        ),
        // An older layout, not read.
        exposure(
            6,
            7.0,
            &format!(
                r#"darktable:multi_priority="6" darktable:blendop_version="13" darktable:blendop_params="{}""#,
                old_blend(2)
            ),
        ),
    ];
    let xml = with_masks(&xmp(&items, 7), &[circle(11), group(10, &[(11, 8)])]);
    let imported = import(&Sidecar::parse(&xml).unwrap(), &Layouts::built_in());
    let values: Vec<f64> = imported.nodes.iter().map(ev).collect();
    assert_eq!(values, [1.0, 4.0]);
    let reasons: Vec<_> = imported
        .report
        .not_applied
        .iter()
        .map(|n| (n.instance, n.reason.clone()))
        .collect();
    assert_eq!(
        reasons,
        [
            (1, Reason::MaskWithoutImage),
            (
                2,
                Reason::UnknownBlendVersion {
                    blendop_version: 16
                }
            ),
            (
                4,
                Reason::Blend {
                    issue: BlendIssue::Mode { mode: 4 }
                }
            ),
            (
                5,
                Reason::Blend {
                    issue: BlendIssue::Parametric
                }
            ),
            (
                6,
                Reason::Blending {
                    blendop_version: 13
                }
            ),
        ]
    );
}

#[test]
fn a_drawn_mask_becomes_the_module_s_sub_network() {
    let mut b = Blend::drawn(10);
    b.opacity = 80.0;
    b.mask_combine |= blend::combine::MASKS_POS;
    b.blur_radius = 2.0;
    let items = [
        exposure(0, 1.0, &blending(&b)),
        exposure(1, 0.5, r#"darktable:multi_priority="1""#),
    ];
    let xml = with_masks(
        &xmp(&items, 2),
        &[circle(11), circle(12), group(10, &[(11, 8), (12, 8 | 4)])],
    );
    let frame = Frame::new(3000.0, 2000.0);
    let imported = import_with(
        &Sidecar::parse(&xml).unwrap(),
        &Layouts::built_in(),
        Some(&frame),
    );
    assert_eq!(imported.report.summary(), None, "{:?}", imported.report);
    let mask = imported.nodes[0].mask.as_ref().expect("a mask");
    assert!((mask.opacity - 0.8).abs() < 1e-9);
    // The group, inverted, then blurred.
    assert_eq!(mask.net.mask, "exposure mask blur");
    let edit = imported.edit();
    let modules: Vec<(&str, &str)> = edit["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["name"].as_str().unwrap(), m["module"].as_str().unwrap()))
        .collect();
    assert_eq!(
        modules,
        [
            ("exposure", "exposure"),
            ("exposure shape 11", "mask_circle"),
            ("exposure shape 12", "mask_circle"),
            ("exposure group 10.0", "mask_combine"),
            ("exposure mask invert", "mask_combine"),
            ("exposure mask blur", "mask_blur"),
            ("exposure blend", "mask_blend"),
            ("exposure 1", "exposure"),
        ]
    );
    let into = |to: &str, input: &str| -> String {
        let c = edit["connections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["to"] == to && c["input"] == input)
            .unwrap_or_else(|| panic!("nothing into {to}.{input}"));
        format!(
            "{}.{}",
            c["from"].as_str().unwrap(),
            c["port"].as_str().unwrap()
        )
    };
    assert_eq!(into("exposure", "input"), "src.image");
    assert_eq!(into("exposure blend", "input"), "src.image");
    assert_eq!(into("exposure blend", "processed"), "exposure.output");
    assert_eq!(into("exposure blend", "mask"), "exposure mask blur.mask");
    assert_eq!(into("exposure group 10.0", "in1"), "exposure shape 12.mask");
    assert_eq!(into("exposure 1", "input"), "exposure blend.output");
    assert_eq!(
        edit["modules"][6]["params"]["opacity"],
        json!({ "float": 0.8 })
    );
}

#[test]
fn a_mask_helios_cannot_draw_keeps_its_module_out_and_says_why() {
    let f = |v: f32| v.to_le_bytes();
    let items = [exposure(0, 1.0, &blending(&Blend::drawn(10)))];
    // A clone shape in the group: retouch's, not a mask's.
    let xml = with_masks(
        &xmp(&items, 1),
        &[
            form(11, 1 | 8, &[f(0.5), f(0.5), f(0.1), f(0.05)]),
            group(10, &[(11, 8)]),
        ],
    );
    let imported = import_with(
        &Sidecar::parse(&xml).unwrap(),
        &Layouts::built_in(),
        Some(&Frame::new(100.0, 100.0)),
    );
    assert!(imported.nodes.is_empty());
    let summary = imported.report.summary().unwrap();
    assert!(
        summary.english.contains("drawn mask not supported"),
        "{}",
        summary.english
    );
    assert!(matches!(
        imported.report.not_applied[0].reason,
        Reason::Mask { .. }
    ));
}

#[test]
fn a_second_instance_of_a_single_instance_module_is_kept_aside() {
    let (wb, version) = params(
        "temperature",
        &json!({"red": 2.0, "green": 1.0, "blue": 1.5}),
    );
    let items = [
        li(
            0,
            "temperature",
            &format!(r#"darktable:modversion="{version}" darktable:params="{wb}""#),
        ),
        li(
            1,
            "temperature",
            &format!(
                r#"darktable:modversion="{version}" darktable:params="{wb}" darktable:multi_priority="1""#
            ),
        ),
    ];
    let imported = import(
        &Sidecar::parse(&xmp(&items, 2)).unwrap(),
        &Layouts::built_in(),
    );
    assert_eq!(imported.nodes.len(), 1);
    assert_eq!(imported.report.not_applied[0].reason, Reason::ExtraInstance);
}

#[test]
fn instances_sharing_a_name_get_distinct_nodes() {
    let items = [
        exposure(
            0,
            1.0,
            r#"darktable:multi_priority="1" darktable:multi_name="warm""#,
        ),
        exposure(
            1,
            2.0,
            r#"darktable:multi_priority="2" darktable:multi_name="warm""#,
        ),
    ];
    let imported = import(
        &Sidecar::parse(&xmp(&items, 2)).unwrap(),
        &Layouts::built_in(),
    );
    let names: Vec<_> = imported.nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["exposure warm", "exposure warm #1"]);
}

#[test]
fn everything_applied_says_nothing() {
    let items = [exposure(0, 1.0, "")];
    let imported = import(
        &Sidecar::parse(&xmp(&items, 1)).unwrap(),
        &Layouts::built_in(),
    );
    assert_eq!(imported.report.summary(), None);
}
