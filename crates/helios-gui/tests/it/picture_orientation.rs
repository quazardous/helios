//! Pictures through the built-in templates turned as their EXIF
//! orientation says, as raws are: a `flip` step at its default reads the
//! tag, a user's orientation beats it, an upright or untagged picture
//! stays as it is, an export comes out upright, and a darktable drawn
//! mask lands on the turned picture where darktable draws it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, Image, NodeId, Region, Value};
use helios_gui::templates;
use helios_template::Bundle;

const W: u32 = 60;
const H: u32 = 40;

/// A `W x H` JPEG with a gradient, tagged `tag` when given.
fn picture(name: &str, tag: Option<u16>) -> PathBuf {
    use image::ImageEncoder as _;
    let img = image::RgbImage::from_fn(W, H, |x, y| {
        image::Rgb([(30 + x * 3) as u8, (40 + y * 4) as u8, 100])
    });
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95);
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
    let dir = std::env::temp_dir().join(format!("helios-gui-orientation-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, out).unwrap();
    path
}

fn find(id: &str) -> Bundle {
    templates::find(id).unwrap_or_else(|| panic!("{id} is built in"))
}

fn whole(graph: &Graph, node: NodeId) -> Image {
    let extent = graph.extent(node).unwrap().expect("a source feeds it");
    Engine::new(Arc::new(Bus::new()))
        .render(graph, node, "output", extent)
        .unwrap()
}

fn opened(path: &Path, id: &str) -> (Graph, NodeId) {
    templates::open_file(path, &find(id)).unwrap_or_else(|e| panic!("{id}: {e}"))
}

#[test]
fn a_portrait_picture_is_turned_upright_unless_the_user_says_otherwise() {
    let six = picture("six.jpg", Some(6));
    let one = picture("one.jpg", Some(1));
    for id in [
        "helios.develop-preview",
        "helios.batch-export",
        "helios.portrait-retouch",
        "helios.import-cull-dedupe",
    ] {
        let (graph, upright) = opened(&one, id);
        let stored = whole(&graph, upright);
        let (mut graph, view) = opened(&six, id);
        let extent = graph.extent(view).unwrap().unwrap();
        assert_eq!((extent.width, extent.height), (H, W), "{id}: {extent}");
        // Turned 90 degrees clockwise: the left column comes from the
        // bottom row.
        let turned = whole(&graph, view);
        let h = i64::from(H);
        for v in 0..i64::from(W) {
            for u in 0..h {
                assert_eq!(
                    turned.pixel(extent.x + u, extent.y + v),
                    stored.pixel(v, h - 1 - u),
                    "{id}: ({u}, {v})"
                );
            }
        }
        // The user's orientation beats the tag.
        let flip = graph
            .find("flip")
            .unwrap_or_else(|| panic!("{id}: no flip"));
        graph.set_param(flip, "orientation", Value::Int(0)).unwrap();
        assert_eq!(
            graph.extent(view).unwrap(),
            Some(Region::sized(W, H)),
            "{id}"
        );
        assert_eq!(whole(&graph, view).to_vec(), stored.to_vec(), "{id}");
    }
}

#[test]
fn an_upright_or_untagged_picture_is_unchanged() {
    let none = picture("none.jpg", None);
    let one = picture("one.jpg", Some(1));
    let (graph, view) = opened(&none, "helios.develop-preview");
    assert_eq!(graph.extent(view).unwrap(), Some(Region::sized(W, H)));
    let untagged = whole(&graph, view);
    let (graph, view) = opened(&one, "helios.develop-preview");
    assert_eq!(whole(&graph, view).to_vec(), untagged.to_vec());
    // The flip step is there, at its default, and turns nothing.
    let flip = graph.find("flip").unwrap();
    assert_eq!(
        graph.params(flip).unwrap().unwrap().get("orientation"),
        Some(&Value::Int(-1))
    );
}

#[test]
fn a_portrait_picture_exports_upright() {
    let six = picture("export-six.jpg", Some(6));
    let (graph, view) = templates::develop_file(&six, &find("helios.batch-export")).unwrap();
    let extent = graph.extent(view).unwrap().unwrap();
    assert_eq!((extent.width, extent.height), (H, W));
}

#[test]
fn a_darktable_circle_is_placed_on_the_turned_picture_where_darktable_draws_it() {
    use std::fmt::Write as _;
    let hex = |bytes: &[u8]| {
        bytes.iter().fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
    };
    let layouts = helios_xmp::Layouts::built_in();
    let version = *layouts.versions("exposure").iter().max().unwrap();
    let exposure = hex(&helios_xmp::encode(
        layouts.get("exposure", version).unwrap(),
        &serde_json::json!({ "exposure": 1.0 }),
    )
    .unwrap());
    // A circle at (0.25, 0.4), radius 0.1: (15, 16) on the stored pixels.
    let circle: Vec<u8> = [0.25f32, 0.4, 0.1, 0.02]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let xml = format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/"
   darktable:xmp_version="5" darktable:iop_order_version="4" darktable:history_end="1">
   <darktable:masks_history><rdf:Seq><rdf:li darktable:mask_num="0" darktable:mask_id="10" darktable:mask_type="1" darktable:mask_name="circle" darktable:mask_version="6" darktable:mask_points="{}" darktable:mask_nb="1" darktable:mask_src="0000000000000000"/></rdf:Seq></darktable:masks_history>
   <darktable:history><rdf:Seq>
    <rdf:li darktable:num="0" darktable:operation="exposure" darktable:enabled="1" darktable:modversion="{version}" darktable:params="{exposure}" darktable:blendop_version="14" darktable:blendop_params="{}"/>
   </rdf:Seq></darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#,
        hex(&circle),
        hex(&helios_xmp::blend::Blend::drawn(10).write())
    );
    let six = picture("mask-six.jpg", Some(6));
    let (_, imported) = helios_library::darktable::read_sidecar_for(&xml, &six).unwrap();
    assert_eq!(imported.report.summary(), None, "{:?}", imported.report);
    // Stored (15, 16) is at (-16, 15) once turned clockwise about the
    // origin, where the picture's flip step puts that pixel.
    let exposure = imported
        .nodes
        .iter()
        .find(|n| n.operation == "exposure")
        .unwrap();
    let mask = exposure.mask.as_ref().expect("the masked exposure applied");
    let circle = mask
        .net
        .nodes
        .iter()
        .find(|n| n.module == "mask_circle")
        .unwrap();
    let at = |k: &str| match circle.params.get(k) {
        Some(Value::Float(v)) => *v,
        other => panic!("{k}: {other:?}"),
    };
    assert!((at("x") + 16.0).abs() < 1e-3 && (at("y") - 15.0).abs() < 1e-3);
    // The picture's own render puts its stored (15, 16) there too.
    let (graph, view) = opened(&six, "helios.develop-preview");
    let extent = graph.extent(view).unwrap().unwrap();
    assert!(extent.contains(&Region::new(-16, 15, 1, 1)), "{extent}");
}
