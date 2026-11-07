//! Raws developed on the desktop: a file opened in a workspace made from
//! "Develop one image" goes through its sensor data, the same chain and
//! the same pixels as the node's raw path, and a picture is refused there.
//! A network whose source takes an image (a picture or a raw) develops a
//! raw through the raw front: the same pixels as "Develop one image".
//!
//! The raws are found as the other render tests find them: a few files
//! of the raw corpus (`HELIOS_RAW_CORPUS`, a directory filled by
//! `tools/bench/fill-corpus.sh`), and darktable's integration images
//! (`HELIOS_TEST_IMAGES`, else a darktable checkout next to Helios's).
//! A file missing is skipped, saying so.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, Image, NodeId, Region};
use helios_gui::templates;

/// Corpus raws, by manifest id: a Bayer CR2, a Bayer NEF and an X-Trans
/// RAF.
const CORPUS: [&str; 3] = [
    "canon-eos-350d-cr2-raw-3-2",
    "nikon-d300-nef-12bit-lossless-compression",
    "fujifilm-x-pro1-raf-12bit-12bit-uncompressed-3-2",
];

/// A corpus raw that is not a mosaic: the loader refuses it.
const NOT_A_MOSAIC: &str = "canon-eos-5d-mark-iv-cr2-sraw";

/// darktable's integration images: a Bayer CR2 and an X-Trans RAF.
const DARKTABLE: [&str; 2] = ["mire1.cr2", "mire1-xtrans.raf"];

fn corpus() -> Option<PathBuf> {
    let dir = std::env::var_os("HELIOS_RAW_CORPUS").map(PathBuf::from);
    if dir.is_none() {
        eprintln!("skipped: HELIOS_RAW_CORPUS is not set");
    }
    dir
}

/// A corpus file by id, whatever its extension.
fn corpus_file(dir: &Path, id: &str) -> Option<PathBuf> {
    let found = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(id));
    if found.is_none() {
        eprintln!("skipped: no {id} in {}", dir.display());
    }
    found
}

fn darktable_images() -> PathBuf {
    std::env::var_os("HELIOS_TEST_IMAGES").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../darktable/src/tests/integration/images")
        },
        PathBuf::from,
    )
}

fn develop() -> helios_template::Bundle {
    templates::find("helios.develop").expect("Develop one image is built in")
}

/// A square of `side` pixels at the middle of `extent`.
fn middle(extent: Region, side: u32) -> Region {
    let (w, h) = (extent.width.min(side), extent.height.min(side));
    Region::new(
        extent.x + i64::from((extent.width - w) / 2),
        extent.y + i64::from((extent.height - h) / 2),
        w,
        h,
    )
}

fn render(graph: &Graph, node: NodeId, port: &str, region: Region) -> Image {
    Engine::new(Arc::new(Bus::new()))
        .render(graph, node, port, region)
        .unwrap()
}

/// The node's raw path on the same file: the chain "Develop one image"
/// wires, with its parameters (its exposure darktable's default for a
/// raw).
fn node_render(raw: &Path, region: Region) -> Image {
    let modules: Vec<serde_json::Value> = develop()
        .template
        .network
        .nodes
        .iter()
        .map(|n| serde_json::json!({ "name": n.name, "module": n.module, "params": n.params }))
        .collect();
    let network = serde_json::json!({
        "sources": [{ "name": "raw", "file": raw }],
        "modules": modules,
        "connections": [
            { "from": "raw", "port": "image", "to": "rawprepare", "input": "input" },
            { "from": "rawprepare", "port": "output", "to": "whitebalance", "input": "input" },
            { "from": "whitebalance", "port": "output", "to": "highlights", "input": "input" },
            { "from": "highlights", "port": "output", "to": "demosaic", "input": "input" },
            { "from": "demosaic", "port": "output", "to": "colorin", "input": "input" },
            { "from": "colorin", "port": "output", "to": "exposure", "input": "input" },
            { "from": "exposure", "port": "output", "to": "sigmoid", "input": "input" }
        ]
    });
    let loaded = helios_node::load(serde_json::from_value(network).unwrap()).unwrap();
    let tone = loaded.node("sigmoid").unwrap();
    render(&loaded.graph, tone, "output", region)
}

/// Opens `raw` in "Develop one image" as the desktop does and renders the
/// middle of the image: real light, in colour, and the node's pixels.
fn develops(raw: &Path) {
    let name = raw.display();
    let (graph, node) = templates::open_file(raw, &develop()).unwrap();
    let extent = graph.extent(node).unwrap().expect("a source feeds it");
    // The sensor's data, not a preview: as large as the loader's crop.
    let loaded = helios_load::load(raw).unwrap();
    assert_eq!(extent, loaded.image.region(), "{name}");
    let region = middle(extent, 256);
    let out = render(&graph, node, "output", region);
    assert_eq!(out.channels(), 4, "{name}");
    let px = out.to_vec();
    assert!(px.iter().all(|v| v.is_finite()), "{name}: not finite");
    let mut mean = [0.0f64; 3];
    for p in px.chunks(4) {
        for c in 0..3 {
            mean[c] += f64::from(p[c]);
        }
    }
    let n = (px.len() / 4) as f64;
    let mean = mean.map(|m| m / n);
    let grey = (mean[0] + mean[1] + mean[2]) / 3.0;
    eprintln!("{name}: {extent:?}, the middle's mean RGB {mean:?}");
    assert!(grey > 0.02 && grey < 0.98, "{name}: mean {mean:?}");
    // The node's raw path gives the same pixels.
    let theirs = node_render(raw, region).to_vec();
    let worst = px
        .iter()
        .zip(&theirs)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "{name}: differs from the node's by {worst}");
}

/// "Batch export", whose source takes an image: a picture, or a raw
/// through the raw front.
fn image_network() -> helios_template::Bundle {
    let b = templates::find("helios.batch-export").expect("Batch export is built in");
    assert!(b.template.network.takes_raw());
    b
}

/// Opens `raw` in a network taking an image: its sensor data through the
/// raw front, the same pixels as "Develop one image".
fn develops_in_an_image_network(raw: &Path) {
    let name = raw.display();
    let (graph, node) = templates::open_file(raw, &image_network()).unwrap();
    assert!(graph.find("demosaic").is_some(), "{name}: no raw front");
    let extent = graph.extent(node).unwrap().expect("a source feeds it");
    let region = middle(extent, 128);
    let ours = render(&graph, node, "output", region).to_vec();
    let (graph, node) = templates::open_file(raw, &develop()).unwrap();
    let theirs = render(&graph, node, "output", region).to_vec();
    let worst = ours
        .iter()
        .zip(&theirs)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "{name}: differs from Develop's by {worst}");
}

#[test]
fn corpus_raws_develop_through_the_raw_front_of_an_image_network() {
    let Some(dir) = corpus() else { return };
    for id in CORPUS {
        if let Some(raw) = corpus_file(&dir, id) {
            develops_in_an_image_network(&raw);
        }
    }
}

#[test]
fn darktable_test_raws_develop_through_the_raw_front_of_an_image_network() {
    let dir = darktable_images();
    for file in DARKTABLE {
        let raw = dir.join(file);
        if raw.is_file() {
            develops_in_an_image_network(&raw);
        } else {
            eprintln!("skipped: no {}", raw.display());
        }
    }
}

/// Exports `raw` as "Batch export" does (no edit): the same pixels as
/// "Develop one image" shows; a network taking pictures only refuses it
/// rather than export its preview.
fn exports_as_develop_shows(raw: &Path) {
    let name = raw.display();
    let vault = helios_vault::Vault::new(helios_vault::MemoryStore::default());
    let engine = Engine::new(Arc::new(Bus::new()));
    let item = helios_export::batch::Item {
        image: "0".repeat(64),
        version: None,
        source: raw.to_path_buf(),
        output: raw.with_extension("jpg"),
        skip: None,
        error: None,
    };
    let exported = helios_gui::export::render_item(&vault, &engine, &image_network(), &item)
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let (graph, node) = templates::open_file(raw, &develop()).unwrap();
    let extent = graph.extent(node).unwrap().unwrap();
    assert_eq!(exported.region(), extent, "{name}: not the sensor's size");
    let region = middle(extent, 128);
    let shown = render(&graph, node, "output", region).to_vec();
    let cut = exported.crop(region).expect("inside the export").to_vec();
    let worst = cut
        .iter()
        .zip(&shown)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "{name}: the export differs by {worst}");
    let mut pictures = image_network();
    pictures.template.network.image_sources.clear();
    let e = helios_gui::export::render_item(&vault, &engine, &pictures, &item)
        .err()
        .unwrap();
    assert!(e.contains("cannot develop a raw"), "{name}: {e}");
}

#[test]
fn a_raw_exported_from_batch_export_has_develop_s_pixels() {
    let dir = darktable_images();
    let raw = dir.join(DARKTABLE[0]);
    if raw.is_file() {
        exports_as_develop_shows(&raw);
    } else {
        eprintln!("skipped: no {}", raw.display());
    }
    if let Some(dir) = corpus()
        && let Some(raw) = corpus_file(&dir, CORPUS[0])
    {
        exports_as_develop_shows(&raw);
    }
}

/// darktable's exposure test: its sidecar (a full default history of a
/// raw, exposure raised) and its raw.
fn darktable_edit() -> Option<(PathBuf, serde_json::Value)> {
    let dir = darktable_images();
    let (xmp, raw) = (
        dir.join("../0001-exposure/exposure.xmp"),
        dir.join(DARKTABLE[0]),
    );
    if !xmp.is_file() || !raw.is_file() {
        eprintln!("skipped: no {} or {}", xmp.display(), raw.display());
        return None;
    }
    let text = std::fs::read_to_string(&xmp).unwrap();
    let (_, imported) = helios_library::darktable::read_sidecar(&text).unwrap();
    Some((raw, imported.edit()))
}

#[test]
fn a_darktable_edit_lands_on_the_raw_front_as_on_develop() {
    let Some((raw, edit)) = darktable_edit() else {
        return;
    };
    let mut skipped = Vec::new();
    let mut renders = Vec::new();
    for bundle in [develop(), image_network()] {
        let (mut graph, node) = templates::open_file(&raw, &bundle).unwrap();
        let left = helios_vault::edit::apply_to_graph(&mut graph, &edit).unwrap();
        // Every raw item has its node: only what neither network has is
        // left out.
        for item in [
            "rawprepare",
            "temperature",
            "highlights",
            "demosaic",
            "colorin",
        ] {
            assert!(
                !left.iter().any(|l| l.starts_with(&format!("`{item}`"))),
                "{}: {item} left out: {left:?}",
                bundle.template.id
            );
        }
        // darktable's white balance is the user's own coefficients now.
        let wb = graph.find("whitebalance").unwrap();
        assert_eq!(
            graph.params(wb).unwrap().unwrap().get("mode"),
            Some(&helios_core::Value::Choice("user".into())),
            "{}",
            bundle.template.id
        );
        let extent = graph.extent(node).unwrap().unwrap();
        renders.push(render(&graph, node, "output", middle(extent, 64)).to_vec());
        skipped.push(left);
    }
    assert_eq!(skipped[0], skipped[1]);
    let worst = renders[0]
        .iter()
        .zip(&renders[1])
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "the same edit renders apart by {worst}");
}

#[test]
fn a_picture_goes_straight_through_an_image_network() {
    let dir = std::env::temp_dir().join(format!("helios-image-network-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("grey.png");
    image::RgbImage::from_pixel(16, 8, image::Rgb([120, 120, 120]))
        .save(&png)
        .unwrap();
    let (graph, node) = templates::open_file(&png, &image_network()).unwrap();
    assert_eq!(graph.extent(node).unwrap(), Some(Region::sized(16, 8)));
    assert!(graph.find("rawprepare").is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn corpus_raws_develop_from_their_sensor_data() {
    let Some(dir) = corpus() else { return };
    for id in CORPUS {
        if let Some(raw) = corpus_file(&dir, id) {
            develops(&raw);
        }
    }
}

#[test]
fn darktable_test_raws_develop_from_their_sensor_data() {
    let dir = darktable_images();
    for file in DARKTABLE {
        let raw = dir.join(file);
        if raw.is_file() {
            develops(&raw);
        } else {
            eprintln!("skipped: no {}", raw.display());
        }
    }
}

#[test]
fn a_raw_that_is_not_a_mosaic_is_refused_by_develop_and_previewed_elsewhere() {
    let Some(dir) = corpus() else { return };
    let Some(raw) = corpus_file(&dir, NOT_A_MOSAIC) else {
        return;
    };
    let e = templates::open_file(&raw, &develop()).err().unwrap();
    assert!(e.contains("not a mosaic"), "{e}");
    // The preview template, which takes a picture too, shows its
    // embedded preview.
    let preview = templates::find("helios.develop-preview").unwrap();
    let (graph, node) = templates::open_file(&raw, &preview).unwrap();
    assert!(graph.extent(node).unwrap().is_some());
}

#[test]
fn a_picture_is_refused_by_develop_and_opens_in_the_preview_template() {
    let dir = std::env::temp_dir().join(format!("helios-develop-raw-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("grey.png");
    image::RgbImage::from_pixel(16, 8, image::Rgb([120, 120, 120]))
        .save(&png)
        .unwrap();
    let e = templates::open_file(&png, &develop()).err().unwrap();
    assert!(e.contains("develops raw files"), "{e}");
    let preview = templates::find("helios.develop-preview").unwrap();
    let (graph, node) = templates::open_file(&png, &preview).unwrap();
    assert_eq!(graph.extent(node).unwrap(), Some(Region::sized(16, 8)));
    std::fs::remove_dir_all(&dir).unwrap();
}
