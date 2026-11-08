//! darktable's drawn masks on a turned photo: the image's orientation is
//! resolved as darktable's `flip` resolves it (the EXIF tag by default,
//! the `flip` item's when it sets one, none when it is disabled), and the
//! masks turn with the image.

use std::fmt::Write as _;

use helios_core::Value as Param;
use helios_xmp::blend::Blend;
use helios_xmp::drawn::{Frame, exif_orientation};
use helios_xmp::{Layouts, Sidecar, encode, import_with};
use serde_json::json;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    })
}

/// A masked exposure of +1 EV, through the circle `10`, after a `flip`
/// item (`None`: no flip item) with darktable's `orientation` value.
fn sidecar(flip: Option<(i32, bool)>) -> Sidecar {
    let layouts = Layouts::built_in();
    let version = *layouts.versions("exposure").iter().max().unwrap();
    let exposure = hex(&encode(
        layouts.get("exposure", version).unwrap(),
        &json!({ "exposure": 1.0 }),
    )
    .unwrap());
    let mut items = Vec::new();
    if let Some((o, enabled)) = flip {
        items.push(format!(
            r#"<rdf:li darktable:num="0" darktable:operation="flip" darktable:enabled="{}" darktable:modversion="2" darktable:params="{}"/>"#,
            u8::from(enabled),
            hex(&o.to_le_bytes())
        ));
    }
    items.push(format!(
        r#"<rdf:li darktable:num="1" darktable:operation="exposure" darktable:enabled="1" darktable:modversion="{version}" darktable:params="{exposure}" darktable:blendop_version="14" darktable:blendop_params="{}"/>"#,
        hex(&Blend::drawn(10).write())
    ));
    // A circle at (0.25, 0.4), radius 0.1, feather 0.05.
    let circle: Vec<u8> = [0.25f32, 0.4, 0.1, 0.05]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let xml = format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/"
   darktable:xmp_version="5" darktable:iop_order_version="4" darktable:history_end="2">
   <darktable:masks_history><rdf:Seq><rdf:li darktable:mask_num="0" darktable:mask_id="10" darktable:mask_type="1" darktable:mask_name="circle" darktable:mask_version="6" darktable:mask_points="{}" darktable:mask_nb="1" darktable:mask_src="0000000000000000"/></rdf:Seq></darktable:masks_history>
   <darktable:history><rdf:Seq>
{}
   </rdf:Seq></darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#,
        hex(&circle),
        items.join("\n")
    );
    Sidecar::parse(&xml).unwrap()
}

/// The circle's centre once imported on a 3000 x 2000 sensor, turned as
/// the sidecar turns an image whose EXIF tag is `exif`.
fn centre(sidecar: &Sidecar, exif: i64) -> (f64, f64) {
    let frame = Frame::new(3000.0, 2000.0).oriented(sidecar.orientation(exif_orientation(exif)));
    let imported = import_with(sidecar, &Layouts::built_in(), Some(&frame));
    assert_eq!(imported.report.summary(), None, "{:?}", imported.report);
    let exposure = imported
        .nodes
        .iter()
        .find(|n| n.operation == "exposure")
        .unwrap();
    let mask = exposure.mask.as_ref().expect("a mask");
    let circle = mask
        .net
        .nodes
        .iter()
        .find(|n| n.module == "mask_circle")
        .unwrap();
    let get = |k: &str| match circle.params.get(k) {
        Some(Param::Float(v)) => *v,
        other => panic!("{k}: {other:?}"),
    };
    (get("x"), get("y"))
}

fn near(got: (f64, f64), want: (f64, f64)) {
    assert!(
        (got.0 - want.0).abs() < 1e-3 && (got.1 - want.1).abs() < 1e-3,
        "{got:?}, not {want:?}"
    );
}

#[test]
fn darktable_turns_an_image_by_its_exif_tag_unless_its_flip_says_otherwise() {
    // EXIF 6: the camera was turned clockwise; darktable rotates the
    // image clockwise (ORIENTATION_ROTATE_CW_90_DEG, 5).
    assert_eq!(exif_orientation(6), 5);
    assert_eq!(exif_orientation(8), 6);
    assert_eq!(exif_orientation(1), 0);
    assert_eq!(exif_orientation(0), 0);
    // flip at auto (-1), and no flip at all: the EXIF tag.
    assert_eq!(sidecar(Some((-1, true))).orientation(5), 5);
    assert_eq!(sidecar(None).orientation(5), 5);
    // flip set by the user beats the tag.
    assert_eq!(sidecar(Some((6, true))).orientation(5), 6);
    assert_eq!(sidecar(Some((0, true))).orientation(5), 0);
    // flip disabled: the image is not turned.
    assert_eq!(sidecar(Some((-1, false))).orientation(5), 0);
}

#[test]
fn a_circle_on_a_portrait_raw_lands_where_darktable_draws_it() {
    // The circle sits at (750, 800) on the sensor. darktable's flip for
    // EXIF 6 mirrors y, then swaps x and y; Helios's flip turns about the
    // origin the same way, so the image's pixel (750, 800) is at
    // (-800, 750) once turned, and the circle with it.
    near(centre(&sidecar(Some((-1, true))), 6), (-800.0, 750.0));
    // No flip item: darktable runs flip at its default, the tag's.
    near(centre(&sidecar(None), 6), (-800.0, 750.0));
    // An upright photo: where it was drawn.
    near(centre(&sidecar(Some((-1, true))), 1), (750.0, 800.0));
}

#[test]
fn a_flip_set_in_darktable_overrides_the_exif_tag() {
    // Turned counter-clockwise by the user (6: mirror x, then swap) on a
    // photo the camera tagged 6.
    near(centre(&sidecar(Some((6, true))), 6), (800.0, -750.0));
    // flip disabled: darktable leaves the image as the sensor has it, and
    // the import keeps Helios's flip from turning it by the tag.
    let s = sidecar(Some((-1, false)));
    near(centre(&s, 6), (750.0, 800.0));
    let imported = import_with(&s, &Layouts::built_in(), Some(&Frame::new(3000.0, 2000.0)));
    let flip = imported
        .nodes
        .iter()
        .find(|n| n.operation == "flip")
        .expect("a flip node");
    assert_eq!(flip.params.get("orientation"), Some(&Param::Int(0)));
    assert_eq!(imported.report.disabled, ["flip"]);
}
