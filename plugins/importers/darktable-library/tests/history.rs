//! Importer interface 2: each photo's edit rebuilt from the library's
//! `history` and `masks_history` rows maps onto the same Helios chain as
//! the same edit read from its `.xmp`; the library's history or the
//! `.xmp` is chosen as darktable's own rules say; and a large library
//! answers in pages, with progress, cancelled between pages.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use helios_plugin::{Broker, Policy, bundled};
use helios_plugin_darktable_library::fixture::{self, HistoryRow, MaskRow, Options, Schema};
use helios_plugin_darktable_library::{ScanError, scan_paged, sidecar_name};
use helios_xmp::{Layouts, Sidecar};
use serde_json::{Value, json};

const PLUGIN: &str = "io.github.quazardous.helios.darktable-library";
const IMPORTER: &str = "darktable-library";
const PROGRAM: &str = env!("CARGO_BIN_EXE_helios-darktable-library");
/// darktable's history hash of the edit, as `history_hash.current_hash`
/// and the sidecar's `darktable:history_current_hash` spell it.
const HASH: [u8; 16] = [
    0x0f, 0x1e, 0x2d, 0x3c, 0x4b, 0x5a, 0x69, 0x78, 0x87, 0x96, 0xa5, 0xb4, 0xc3, 0xd2, 0xe1, 0xf0,
];

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-dt-history-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn jpeg_bytes() -> Vec<u8> {
    std::fs::read(root().join("crates/helios-plugin/tests/fixtures/rated-photos/c.jpg")).unwrap()
}

/// A darktable folder over `photos` (each a copy of a test JPEG).
fn library(dir: &Path, photos: &[String]) -> PathBuf {
    let folder = dir.join("photos");
    std::fs::create_dir_all(&folder).unwrap();
    let bytes = jpeg_bytes();
    let first = folder.join(&photos[0]);
    std::fs::write(&first, &bytes).unwrap();
    for name in &photos[1..] {
        // hard links: thousands of photos cost nothing
        if std::fs::hard_link(&first, folder.join(name)).is_err() {
            std::fs::write(folder.join(name), &bytes).unwrap();
        }
    }
    let out = dir.join("darktable");
    fixture::build(&out, Schema::V5_2, &folder, &Options::default()).unwrap();
    out
}

/// A darktable edit: the masks test's circle-masked exposure
/// (`tests/masks/circle.xmp`: colorin, highlights, demosaic, exposure
/// with a drawn circle, colorout, flip) with a sigmoid tone mapper at its
/// defaults on top: its sidecar's text, and the sidecar read.
fn edit() -> (String, Sidecar) {
    let xml = std::fs::read_to_string(root().join("tests/masks/circle.xmp")).unwrap();
    let base = Sidecar::parse(&xml).unwrap();
    let layouts = Layouts::built_in();
    let version = *layouts.versions("sigmoid").last().unwrap();
    let params = helios_xmp::encode(layouts.get("sigmoid", version).unwrap(), &json!({})).unwrap();
    let num = base.history.len();
    let end = base.history_end.unwrap();
    assert_eq!(end as usize, num, "the whole history applies");
    let hex = to_hex(&params);
    let hash = to_hex(&HASH);
    let li = format!(
        r#"<rdf:li darktable:num="{num}" darktable:operation="sigmoid" darktable:enabled="1" darktable:modversion="{version}" darktable:params="{hex}" darktable:multi_name="" darktable:multi_priority="0"/>
    </rdf:Seq>
   </darktable:history>"#
    );
    let history_close = xml.find("</darktable:history>").unwrap();
    let at = xml[..history_close].rfind("</rdf:Seq>").unwrap();
    let close = history_close + "</darktable:history>".len();
    let mut text = format!("{}{li}{}", &xml[..at], &xml[close..]);
    text = text.replace(
        &format!("darktable:history_end=\"{end}\""),
        &format!(
            "darktable:history_end=\"{}\" darktable:history_current_hash=\"{hash}\"",
            end + 1
        ),
    );
    let sidecar = Sidecar::parse(&text).unwrap();
    assert_eq!(sidecar.history.len(), num + 1, "{text}");
    assert_eq!(sidecar.history.last().unwrap().operation, "sigmoid");
    assert!(!sidecar.masks.is_empty(), "a drawn mask");
    (text, sidecar)
}

/// The library rows darktable keeps for a sidecar's edit.
fn rows(sidecar: &Sidecar) -> (Vec<HistoryRow>, Vec<MaskRow>) {
    let history = sidecar
        .history
        .iter()
        .map(|i| HistoryRow {
            num: i64::from(i.num),
            operation: i.operation.clone(),
            modversion: i64::from(i.modversion),
            params: i.params.clone(),
            enabled: i.enabled,
            blendop_version: i.blendop_version.map(i64::from),
            blendop_params: (!i.blendop_params.is_empty()).then(|| i.blendop_params.clone()),
            multi_priority: i64::from(i.multi_priority),
            multi_name: i.multi_name.clone(),
        })
        .collect();
    let masks = sidecar
        .masks
        .iter()
        .map(|f| MaskRow {
            num: i64::from(f.num),
            formid: f.id,
            form: i64::from(f.kind),
            name: f.name.clone(),
            version: i64::from(f.version),
            points: f.points.clone(),
            points_count: i64::from(f.count),
        })
        .collect();
    (history, masks)
}

fn scan_all(source: &Path) -> Value {
    let params = json!({"importer": IMPORTER, "kind": "folder", "source": source});
    let mut images = Vec::new();
    let mut page = |list: Vec<Value>, _: usize, _: usize| images.extend(list);
    let answer = scan_paged(
        &params,
        &AtomicBool::new(false),
        &mut |_, _, _| {},
        Some((100, &mut page)),
    )
    .unwrap();
    json!({ "answer": answer, "images": images })
}

fn edits_of(scan: &Value, name: &str) -> Vec<Value> {
    scan["images"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"].as_str().unwrap().ends_with(name))
        .unwrap()["edits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn chain(sidecar: &Sidecar) -> (Value, Value) {
    let imported = helios_xmp::import(sidecar, &Layouts::built_in());
    let edit = imported.edit();
    (edit["modules"].clone(), edit["connections"].clone())
}

#[test]
fn a_library_edit_maps_onto_the_chain_its_xmp_gives() {
    let dir = scratch("same");
    let photos = vec!["c.jpg".to_string()];
    let source = library(&dir, &photos);
    let (xml, sidecar) = edit();
    let (history, masks) = rows(&sidecar);
    let end = i64::from(sidecar.history_end.unwrap());
    let photo = dir.join("photos/c.jpg");
    let now = 2_000_000_000;
    fixture::add_edit(&source, "c.jpg", 0, &history, &masks, end, &HASH, now).unwrap();

    // No .xmp (darktable's write_sidecar_files off): the library's edit.
    let scan = scan_all(&source);
    let edits = edits_of(&scan, "c.jpg");
    assert_eq!(edits.len(), 1, "{scan}");
    assert_eq!(edits[0]["from"], "library", "{edits:?}");
    assert!(edits[0]["why"].as_str().unwrap().contains("no .xmp"));
    let from_library = Sidecar::from_history(&edits[0]["history"]).unwrap();
    let (modules, connections) = chain(&from_library);
    assert_eq!((modules.clone(), connections.clone()), chain(&sidecar));
    assert!(!modules.as_array().unwrap().is_empty(), "{modules}");
    assert_eq!(
        from_library.masks.len(),
        sidecar.masks.len(),
        "the drawn mask"
    );
    assert!(
        from_library
            .history
            .iter()
            .any(|i| i.operation == "colorin")
    );
    assert!(
        from_library
            .history
            .iter()
            .any(|i| i.operation == "sigmoid")
    );

    // The same edit in the .xmp (same hash): the .xmp, said so.
    let xmp = dir.join("photos").join(sidecar_name("c.jpg", 0));
    std::fs::write(&xmp, &xml).unwrap();
    let edits = edits_of(&scan_all(&source), "c.jpg");
    assert_eq!(edits[0]["from"], "xmp", "{edits:?}");
    assert!(
        edits[0]["why"]
            .as_str()
            .unwrap()
            .contains("same history hash")
    );
    assert_eq!(edits[0]["xmp"], json!(xmp.to_string_lossy()));

    // The hashes differ and the .xmp is older than darktable's last
    // write: the library is newer.
    std::fs::write(&xmp, xml.replace("0f1e2d3c", "00000000")).unwrap();
    let edits = edits_of(&scan_all(&source), "c.jpg");
    assert_eq!(edits[0]["from"], "library", "{edits:?}");
    assert!(edits[0]["why"].as_str().unwrap().contains("hash differs"));

    // Changed after darktable last wrote it: the .xmp is newer.
    let con = rusqlite::Connection::open(source.join("library.db")).unwrap();
    con.execute("UPDATE images SET write_timestamp = 1000", [])
        .unwrap();
    drop(con);
    let edits = edits_of(&scan_all(&source), "c.jpg");
    assert_eq!(edits[0]["from"], "xmp", "{edits:?}");
    assert!(
        edits[0]["why"]
            .as_str()
            .unwrap()
            .contains("newer than the library")
    );
    assert!(photo.is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn duplicate_sidecars_are_named_as_darktable_names_them() {
    assert_eq!(sidecar_name("a.cr2", 0), "a.cr2.xmp");
    assert_eq!(sidecar_name("a.b.cr2", 3), "a.b_03.cr2.xmp");
    assert_eq!(sidecar_name("noext", 12), "noext_12.xmp");
}

fn many(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("p{i:05}.jpg")).collect()
}

#[test]
fn a_large_library_answers_in_pages_and_a_cancel_stops_between_them() {
    let dir = scratch("pages");
    let source = library(&dir, &many(3000));
    let params = json!({"importer": IMPORTER, "kind": "folder", "source": source});
    // Pages of 500: six, each with its progress.
    let mut pages: Vec<(usize, usize, usize)> = Vec::new();
    let mut page = |list: Vec<Value>, done: usize, total: usize| {
        pages.push((list.len(), done, total));
    };
    let answer = scan_paged(
        &params,
        &AtomicBool::new(false),
        &mut |_, _, _| {},
        Some((500, &mut page)),
    )
    .unwrap();
    assert_eq!(answer["pages"], 6, "{answer}");
    assert!(answer.get("images").is_none());
    // the missing drive's photo is a file of the total, never listed
    assert_eq!(answer["total"], 3001);
    assert_eq!(pages.iter().map(|p| p.0).sum::<usize>(), 3000);
    assert!(pages.windows(2).all(|w| w[0].1 < w[1].1), "{pages:?}");
    assert_eq!(pages.last().unwrap().1, 3000);

    // Cancelled during the second page: no third page is sent.
    let cancelled = AtomicBool::new(false);
    let mut sent = 0;
    let mut page = |_: Vec<Value>, _: usize, _: usize| {
        sent += 1;
        if sent == 2 {
            cancelled.store(true, Ordering::SeqCst);
        }
    };
    let r = scan_paged(
        &params,
        &cancelled,
        &mut |_, _, _| {},
        Some((500, &mut page)),
    );
    assert_eq!(r.unwrap_err(), ScanError::Cancelled);
    assert_eq!(sent, 2);

    // Through the broker, as Helios runs it: every image, progress per
    // page.
    let mut dirs = Vec::new();
    let programs = Path::new(PROGRAM).parent().unwrap();
    bundled::add_to(&mut dirs, &dir.join("plugins"), programs).unwrap();
    let mut b = Broker::new(Policy::default());
    b.discover(&dirs);
    assert!(b.rejected().is_empty(), "{:?}", b.rejected());
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let s = seen.clone();
    let jpeg = |p: &Path| p.extension().is_some_and(|e| e == "jpg");
    let scan = b
        .importer_scan(PLUGIN, IMPORTER, &source, &jpeg, None, move |p| {
            s.lock().unwrap().push(p.clone());
        })
        .unwrap();
    assert_eq!(scan.images.len(), 3000);
    assert!(scan.refused.is_empty(), "{:?}", scan.refused);
    let pages = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.get("images").is_some())
        .count();
    assert_eq!(pages, 6);
    let _ = std::fs::remove_dir_all(&dir);
}
