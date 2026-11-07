//! darktable's colour calibration (`channelmixerrgb`): its CAT16
//! adaptation maps to Helios's module of the same name; the parts not
//! ported yet keep the item out, named.

use std::fmt::Write as _;

use helios_core::Value as Param;
use helios_xmp::{Layouts, Reason, encode, import_text};
use serde_json::{Value, json};

/// A sidecar with one colour calibration item, its params `values` on top
/// of an identity mixer.
fn sidecar(values: Value) -> String {
    let layouts = Layouts::built_in();
    let version = *layouts.versions("channelmixerrgb").iter().max().unwrap();
    let mut fields = json!({
        "red": [1.0, 0.0, 0.0, 0.0],
        "green": [0.0, 1.0, 0.0, 0.0],
        "blue": [0.0, 0.0, 1.0, 0.0],
        "illuminant": "DT_ILLUMINANT_D",
        "illum_fluo": "DT_ILLUMINANT_FLUO_F3",
        "illum_led": "DT_ILLUMINANT_LED_B5",
        "adaptation": "DT_ADAPTATION_CAT16",
        "x": 0.333,
        "y": 0.333,
        "temperature": 5003.0,
        "gamut": 1.0,
        "clip": true,
        "version": "CHANNELMIXERRGB_V_3",
    });
    for (k, v) in values.as_object().unwrap() {
        fields[k] = v.clone();
    }
    let bytes = encode(layouts.get("channelmixerrgb", version).unwrap(), &fields).unwrap();
    let hex = bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    });
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/"
   darktable:xmp_version="5" darktable:iop_order_version="4" darktable:history_end="1">
   <darktable:history><rdf:Seq>
<rdf:li darktable:num="0" darktable:operation="channelmixerrgb" darktable:enabled="1" darktable:modversion="{version}" darktable:params="{hex}"/>
   </rdf:Seq></darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#
    )
}

fn import(values: Value) -> helios_xmp::Imported {
    import_text(&sidecar(values), &Layouts::built_in())
        .unwrap()
        .1
}

fn float(p: &helios_core::Params, name: &str) -> f64 {
    match p.get(name) {
        Some(Param::Float(v)) => *v,
        other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn a_cat16_item_maps_with_darktables_names_and_units() {
    let imported = import(json!({
        "illuminant": "DT_ILLUMINANT_BB",
        "temperature": 3200.0,
        "gamut": 2.5,
        "clip": false,
    }));
    let report = &imported.report;
    assert!(report.not_applied.is_empty(), "{report:#?}");
    assert!(report.dropped_fields.is_empty(), "{report:#?}");
    let [node] = imported.nodes.as_slice() else {
        panic!("{:#?}", imported.nodes)
    };
    assert_eq!(node.module, "channelmixerrgb");
    let p = &node.params;
    assert_eq!(p.get("illuminant"), Some(&Param::Choice("bb".into())));
    assert_eq!(p.get("adaptation"), Some(&Param::Choice("cat16".into())));
    assert_eq!(p.get("illum_fluo"), Some(&Param::Choice("f3".into())));
    assert_eq!(p.get("illum_led"), Some(&Param::Choice("b5".into())));
    assert_eq!(float(p, "temperature"), 3200.0);
    assert_eq!(float(p, "gamut"), 2.5);
    assert_eq!(p.get("clip"), Some(&Param::Bool(false)));
    assert!((float(p, "x") - 0.333).abs() < 1e-6);
}

#[test]
fn the_camera_illuminant_darktables_default_maps() {
    // darktable's scene-referred default: CAT16, illuminant as shot in
    // camera, its found chromaticity and temperature saved beside.
    let imported = import(json!({
        "illuminant": "DT_ILLUMINANT_CAMERA",
        "x": 0.3648,
        "y": 0.3773,
        "temperature": 4447.0,
    }));
    let report = &imported.report;
    assert!(report.not_applied.is_empty(), "{report:#?}");
    let [node] = imported.nodes.as_slice() else {
        panic!("{:#?}", imported.nodes)
    };
    assert_eq!(node.module, "channelmixerrgb");
    assert_eq!(
        node.params.get("illuminant"),
        Some(&Param::Choice("camera".into()))
    );
    assert_eq!(
        node.params.get("adaptation"),
        Some(&Param::Choice("cat16".into()))
    );
}

#[test]
fn a_custom_illuminant_keeps_its_chromaticity() {
    let imported = import(json!({
        "illuminant": "DT_ILLUMINANT_CUSTOM",
        "x": 0.4,
        "y": 0.38,
    }));
    let p = &imported.nodes[0].params;
    assert_eq!(p.get("illuminant"), Some(&Param::Choice("custom".into())));
    assert!((float(p, "x") - 0.4).abs() < 1e-6);
    assert!((float(p, "y") - 0.38).abs() < 1e-6);
}

#[test]
fn the_parts_not_ported_keep_the_item_out_by_name() {
    for (values, part) in [
        (
            json!({ "illuminant": "DT_ILLUMINANT_DETECT_EDGES" }),
            "illuminant",
        ),
        (
            json!({ "adaptation": "DT_ADAPTATION_LINEAR_BRADFORD" }),
            "adaptation",
        ),
        (json!({ "red": [0.9, 0.1, 0.0, 0.0] }), "red"),
        (json!({ "saturation": [0.0, 0.2, 0.0, 0.0] }), "saturation"),
        (json!({ "grey": [0.3, 0.6, 0.1, 0.0] }), "grey"),
    ] {
        let imported = import(values.clone());
        assert!(imported.nodes.is_empty(), "{values}");
        let [item] = imported.report.not_applied.as_slice() else {
            panic!("{values}: {:#?}", imported.report)
        };
        match &item.reason {
            Reason::Unsupported { part: named } => {
                assert!(named.starts_with(part), "{values}: {named}");
            }
            other => panic!("{values}: {other:?}"),
        }
    }
}
