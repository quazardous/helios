//! History items Helios cannot apply: kept in their place with their raw
//! params, reported once, and never in the way of the items around them.

use std::fmt::Write as _;

use helios_core::Value as Param;
use helios_xmp::{Layouts, Reason, Sidecar, encode, import, import_text};
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

/// Blend params of version 14 whose first word, the mask mode, is `mode`.
fn blend(mode: u32) -> String {
    let mut bytes = mode.to_le_bytes().to_vec();
    bytes.extend_from_slice(&[0x11; 60]);
    bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    })
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
fn a_blended_item_is_kept_not_applied_to_the_whole_image() {
    let items = [
        exposure(
            0,
            1.0,
            &format!(
                r#"darktable:blendop_version="14" darktable:blendop_params="{}""#,
                blend(0)
            ),
        ),
        // A drawn mask: exposure only where it is.
        exposure(
            1,
            2.0,
            &format!(
                r#"darktable:multi_priority="1" darktable:blendop_version="14" darktable:blendop_params="{}""#,
                blend(2)
            ),
        ),
        // Blend params from a darktable newer than Helios.
        exposure(
            2,
            3.0,
            &format!(
                r#"darktable:multi_priority="2" darktable:blendop_version="15" darktable:blendop_params="{}""#,
                blend(1)
            ),
        ),
        // darktable reads all-zero blend params as no blending.
        exposure(
            3,
            4.0,
            r#"darktable:multi_priority="3" darktable:blendop_version="15" darktable:blendop_params="00000000""#,
        ),
    ];
    let imported = import(
        &Sidecar::parse(&xmp(&items, 4)).unwrap(),
        &Layouts::built_in(),
    );
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
            (
                1,
                Reason::Blending {
                    blendop_version: 14
                }
            ),
            (
                2,
                Reason::UnknownBlendVersion {
                    blendop_version: 15
                }
            ),
        ]
    );
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
