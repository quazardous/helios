//! darktable's drawn masks on a picture (JPEG, PNG, TIFF): placed on its
//! pixels as stored and turned as darktable turns the picture, by its
//! EXIF tag unless the edit's `flip` says otherwise.

use std::fmt::Write as _;
use std::path::PathBuf;

use helios_core::Value as Param;
use helios_xmp::blend::Blend;
use helios_xmp::{Layouts, Sidecar, encode};
use serde_json::json;

const W: u32 = 300;
const H: u32 = 200;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    })
}

/// A `W x H` JPEG, tagged with the EXIF orientation `tag` when given.
fn picture(name: &str, tag: Option<u16>) -> PathBuf {
    use image::ImageEncoder as _;
    let img = image::RgbImage::from_pixel(W, H, image::Rgb([120, 120, 120]));
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90);
    if let Some(tag) = tag {
        let mut exif = b"II*\0".to_vec();
        exif.extend(8u32.to_le_bytes());
        exif.extend(1u16.to_le_bytes());
        exif.extend(0x0112u16.to_le_bytes());
        exif.extend(3u16.to_le_bytes());
        exif.extend(1u32.to_le_bytes());
        exif.extend(tag.to_le_bytes());
        exif.extend([0, 0, 0, 0, 0, 0]);
        enc.set_exif_metadata(exif).unwrap();
    }
    enc.write_image(img.as_raw(), W, H, image::ExtendedColorType::Rgb8)
        .unwrap();
    let dir = std::env::temp_dir().join(format!("helios-library-pictures-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, out).unwrap();
    path
}

/// A masked exposure of +1 EV through the circle `10`, at (0.25, 0.4),
/// after a `flip` item with darktable's `orientation` (none: no item).
fn sidecar(flip: Option<i32>) -> Sidecar {
    let layouts = Layouts::built_in();
    let version = *layouts.versions("exposure").iter().max().unwrap();
    let exposure = hex(&encode(
        layouts.get("exposure", version).unwrap(),
        &json!({ "exposure": 1.0 }),
    )
    .unwrap());
    let mut items = Vec::new();
    if let Some(o) = flip {
        items.push(format!(
            r#"<rdf:li darktable:num="0" darktable:operation="flip" darktable:enabled="1" darktable:modversion="2" darktable:params="{}"/>"#,
            hex(&o.to_le_bytes())
        ));
    }
    items.push(format!(
        r#"<rdf:li darktable:num="1" darktable:operation="exposure" darktable:enabled="1" darktable:modversion="{version}" darktable:params="{exposure}" darktable:blendop_version="14" darktable:blendop_params="{}"/>"#,
        hex(&Blend::drawn(10).write())
    ));
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

/// The circle's centre in Helios's pixels once `sidecar` is imported for
/// the picture `path`.
fn centre(sidecar: &Sidecar, path: &std::path::Path) -> (f64, f64) {
    let imported = helios_library::darktable::import_for(sidecar, path);
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
fn a_circle_on_a_portrait_picture_lands_where_darktable_draws_it() {
    let six = picture("six.jpg", Some(6));
    // The circle sits at (75, 80) on the stored pixels; EXIF 6 turns the
    // picture clockwise about the origin, as Helios's flip does: (-80, 75).
    near(centre(&sidecar(Some(-1)), &six), (-80.0, 75.0));
    near(centre(&sidecar(None), &six), (-80.0, 75.0));
    // The edit's own flip beats the tag: none, then counter-clockwise.
    near(centre(&sidecar(Some(0)), &six), (75.0, 80.0));
    near(centre(&sidecar(Some(6)), &six), (80.0, -75.0));
}

#[test]
fn an_upright_or_untagged_picture_keeps_the_circle_where_it_was_drawn() {
    for (name, tag) in [("one.jpg", Some(1)), ("none.jpg", None)] {
        near(
            centre(&sidecar(Some(-1)), &picture(name, tag)),
            (75.0, 80.0),
        );
    }
}
