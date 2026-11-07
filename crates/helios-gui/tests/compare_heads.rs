//! The Compare panel draws a raw from its head version, rendered through
//! the workspace's network, not from the camera's embedded preview: the
//! preview is the first paint, the render replaces it once it lands, and
//! it is made again when the edit or the network changes. A picture is
//! drawn from its thumbnail, as before.
//!
//! The raw is darktable's integration image (`HELIOS_TEST_IMAGES`, else
//! a darktable checkout next to Helios's); missing, the test that needs
//! it is skipped, saying so.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::{Bus, Engine, Image, Region};
use helios_gui::head_render::{self, HeadRender};
use helios_gui::library::{Entry, Library};
use helios_gui::templates;
use helios_vault::{EditSession, MemoryStore, Vault, edit};
use serde_json::json;

fn mire() -> Option<PathBuf> {
    let dir = std::env::var_os("HELIOS_TEST_IMAGES").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../darktable/src/tests/integration/images")
        },
        PathBuf::from,
    );
    let path = dir.join("mire1.cr2");
    if !path.is_file() {
        eprintln!("skipped: no {}", path.display());
        return None;
    }
    Some(path)
}

/// The mean absolute difference of two images' colour channels, `b`
/// sampled at `a`'s size.
fn difference(a: &Image, b: &Image) -> f64 {
    let (ra, rb) = (a.region(), b.region());
    let scale = f64::from(ra.width) / f64::from(rb.width);
    let b = b.sample_at(Region::sized(ra.width, ra.height), scale);
    let (na, nb) = (a.channels() as usize, b.channels() as usize);
    let mut sum = 0.0;
    let mut n = 0usize;
    for (p, q) in a.data().chunks_exact(na).zip(b.data().chunks_exact(nb)) {
        for c in 0..3 {
            sum += f64::from((p[c] - q[c]).abs());
            n += 1;
        }
    }
    sum / n.max(1) as f64
}

fn mean(image: &Image) -> f64 {
    let n = image.channels() as usize;
    let px: Vec<f64> = image
        .data()
        .chunks_exact(n)
        .flat_map(|p| p[..3].iter().map(|v| f64::from(*v)))
        .collect();
    px.iter().sum::<f64>() / px.len() as f64
}

#[test]
fn a_raw_s_head_version_renders_developed_pixels_not_its_preview() {
    let Some(path) = mire() else { return };
    let vault = Vault::new(MemoryStore::default());
    let image = vault.import_file(&path, 0).unwrap();
    let engine = Engine::new(Arc::new(Bus::new()));
    let bundle = templates::find("helios.develop-preview").unwrap();

    // Never edited: the network's own development, at the size asked.
    let (version, developed) =
        head_render::render(&vault, &engine, &bundle, &path, image, 256).unwrap();
    assert_eq!(version, None);
    let r = developed.region();
    assert!((255..=257).contains(&r.width.max(r.height)), "{r:?}");
    let preview = helios_load::preview(&path).unwrap().expect("a preview");
    let off = difference(&developed, &preview);
    assert!(off > 0.005, "the render is the preview's pixels ({off})");

    // Edited: the head version's exposure shows.
    let (graph, _) = templates::develop_file(&path, &bundle).unwrap();
    let mut edited = edit::of_graph(&graph);
    for m in edited["modules"].as_array_mut().unwrap() {
        if m["name"] == "exposure" {
            m["params"]["exposure"] = json!({ "float": 2.0 });
        }
    }
    let (mut session, _) = EditSession::open(&vault, image).unwrap();
    let saved = session
        .save(&vault, edited, "Exposure", "test", 1)
        .unwrap()
        .expect("a version");
    let (version, brighter) =
        head_render::render(&vault, &engine, &bundle, &path, image, 256).unwrap();
    assert_eq!(version, Some(saved.id.hex()));
    assert!(mean(&brighter) > mean(&developed) * 1.1);
}

fn entry(id: &str, name: &str) -> Entry {
    Entry {
        id: id.into(),
        name: name.into(),
        ..Entry::default()
    }
}

fn landed(id: &str, version: &str, size: u32, shade: f32) -> HeadRender {
    HeadRender {
        id: id.into(),
        version: Some(version.into()),
        size,
        image: Ok(Image::from_fn(Region::sized(8, 6), 4, |_, _, c| {
            if c == 3 { 1.0 } else { shade }
        })),
    }
}

#[test]
fn compare_asks_a_raw_s_render_once_keeps_it_and_asks_again_when_the_edit_changes() {
    let ctx = egui::Context::default();
    let mut library = Library::default();
    library.set_entries(vec![entry("a", "a.cr2"), entry("b", "b.jpg")]);
    let keys = ["a".to_string(), "b".to_string()];

    // A raw is asked, a picture is not; asked once while on its way.
    library.ask_heads(&keys, 600);
    assert_eq!(library.take_head_requests(), [("a".to_string(), 600)]);
    library.ask_heads(&keys, 600);
    assert!(library.take_head_requests().is_empty());
    assert!(library.head_render("a").is_none(), "the preview first");

    // It lands: drawn, not asked again at that size.
    library.head_sink().send(landed("a", "v1", 600, 0.5));
    assert!(library.receive_heads(&ctx));
    assert_eq!(library.head_render("a"), Some((Some("v1"), [8, 6], false)));
    library.ask_heads(&keys, 600);
    assert!(library.take_head_requests().is_empty());

    // The edit changes: the old render is drawn until the new one lands.
    library.edit_changed("a");
    assert_eq!(library.head_render("a"), Some((Some("v1"), [8, 6], true)));
    library.ask_heads(&keys, 600);
    assert_eq!(library.take_head_requests(), [("a".to_string(), 600)]);
    library.head_sink().send(landed("a", "v2", 600, 0.6));
    library.receive_heads(&ctx);
    assert_eq!(library.head_render("a"), Some((Some("v2"), [8, 6], false)));

    // A render asked before an edit lands stale: asked again.
    library.edit_changed("a");
    library.ask_heads(&keys, 600);
    assert_eq!(library.take_head_requests().len(), 1);
    library.edit_changed("a");
    library.head_sink().send(landed("a", "v3", 600, 0.7));
    library.receive_heads(&ctx);
    assert!(library.head_render("a").unwrap().2);

    // The network changes: every render is made again.
    library.ask_heads(&keys, 600);
    assert_eq!(library.take_head_requests().len(), 1);
    library.head_sink().send(landed("a", "v4", 600, 0.7));
    library.receive_heads(&ctx);
    assert!(!library.head_render("a").unwrap().2);
    library.network_changed();
    assert!(library.head_render("a").unwrap().2);
}
