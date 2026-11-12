//! The Lightroom catalogue importer on a small synthetic `.lrcat` built
//! here with the tables and columns the importer reads (see the crate's
//! documentation for where that schema comes from; no real catalogue is
//! committed): what it lists, what it refuses, that it never writes the
//! catalogue, and the program as Helios ships it, run by the broker.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use helios_plugin::{Broker, Policy, bundled};
use helios_plugin_lightroom_catalog::{COLLECTION_TAG, IMPORTER, NOT_A_SOURCE, scan, scan_paged};
use rusqlite::Connection;
use serde_json::{Value, json};

const PLUGIN: &str = "io.github.quazardous.helios.lightroom-catalog";
const PROGRAM: &str = env!("CARGO_BIN_EXE_helios-lightroom-catalog");

/// A directory of the test's own.
fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-lrcat-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The tables the importer reads, with a few of the columns Lightroom
/// has beside them, as the open-source readers list them.
const SCHEMA: &str = "
CREATE TABLE Adobe_variablesGlobal (id_local INTEGER PRIMARY KEY, name, value);
CREATE TABLE AgLibraryRootFolder (id_local INTEGER PRIMARY KEY, id_global UNIQUE NOT NULL,
    absolutePath UNIQUE NOT NULL DEFAULT '', name NOT NULL DEFAULT '',
    relativePathFromCatalog);
CREATE TABLE AgLibraryFolder (id_local INTEGER PRIMARY KEY, id_global UNIQUE NOT NULL,
    parentId INTEGER, pathFromRoot NOT NULL DEFAULT '', rootFolder INTEGER NOT NULL DEFAULT 0,
    visibility INTEGER);
CREATE TABLE AgLibraryFile (id_local INTEGER PRIMARY KEY, id_global UNIQUE NOT NULL,
    baseName NOT NULL DEFAULT '', errorMessage, errorTime, extension NOT NULL DEFAULT '',
    externalModTime, folder INTEGER NOT NULL DEFAULT 0, idx_filename NOT NULL DEFAULT '',
    importHash, lc_idx_filename NOT NULL DEFAULT '', lc_idx_filenameExtension NOT NULL DEFAULT '',
    md5, modTime, originalFilename NOT NULL DEFAULT '', sidecarExtensions);
CREATE TABLE Adobe_images (id_local INTEGER PRIMARY KEY, id_global UNIQUE NOT NULL,
    aspectRatioCache NOT NULL DEFAULT -1, bitDepth NOT NULL DEFAULT 0, captureTime,
    colorChannels NOT NULL DEFAULT 0, colorLabels NOT NULL DEFAULT '',
    copyName, fileFormat NOT NULL DEFAULT 'unset', fileHeight, fileWidth,
    masterImage INTEGER, orientation, pick NOT NULL DEFAULT 0, rating,
    rootFile INTEGER NOT NULL DEFAULT 0, touchCount NOT NULL DEFAULT 0, touchTime NOT NULL DEFAULT 0);
CREATE TABLE AgLibraryKeyword (id_local INTEGER PRIMARY KEY, id_global UNIQUE NOT NULL,
    dateCreated NOT NULL DEFAULT '', genealogy NOT NULL DEFAULT '', imageCountCache DEFAULT -1,
    includeOnExport INTEGER NOT NULL DEFAULT 1, includeParents INTEGER NOT NULL DEFAULT 1,
    includeSynonyms INTEGER NOT NULL DEFAULT 1, keywordType, lastApplied, lc_name, name,
    parent INTEGER);
CREATE TABLE AgLibraryKeywordImage (id_local INTEGER PRIMARY KEY,
    image INTEGER NOT NULL DEFAULT 0, tag INTEGER NOT NULL DEFAULT 0);
CREATE TABLE AgLibraryCollection (id_local INTEGER PRIMARY KEY, creationId NOT NULL DEFAULT '',
    genealogy NOT NULL DEFAULT '', imageCount, name NOT NULL DEFAULT '', parent INTEGER,
    systemOnly NOT NULL DEFAULT '');
CREATE TABLE AgLibraryCollectionImage (id_local INTEGER PRIMARY KEY,
    collection INTEGER NOT NULL DEFAULT 0, image INTEGER NOT NULL DEFAULT 0,
    pick NOT NULL DEFAULT 0, positionInCollection);
INSERT INTO Adobe_variablesGlobal (name, value) VALUES ('Adobe_DBVersion', '1300025');
";

/// A catalogue over `photos` (`<dir>/photos`): a.jpg 3 stars, red,
/// picked, keywords Places|Lyon and people, in collection Trips|Lyon 2024,
/// with a virtual copy carrying the keyword `copy only`; b.jpg rejected,
/// a label from another label set, in the smart collection only; c.jpg
/// no marks; gone.jpg on a missing drive.
fn catalogue(dir: &Path) -> PathBuf {
    let photos = dir.join("photos");
    std::fs::create_dir_all(photos.join("2024")).unwrap();
    for name in ["a.jpg", "b.jpg"] {
        std::fs::write(photos.join("2024").join(name), b"jpeg").unwrap();
    }
    std::fs::write(photos.join("c.jpg"), b"jpeg").unwrap();
    let root = format!("{}/", photos.to_string_lossy().replace('\\', "/"));
    let path = dir.join("Lightroom Catalog.lrcat");
    let con = Connection::open(&path).unwrap();
    con.execute_batch(SCHEMA).unwrap();
    con.execute(
        "INSERT INTO AgLibraryRootFolder (id_local, id_global, absolutePath, name) \
         VALUES (1, 'R1', ?1, 'photos'), (2, 'R2', '/media/gone/', 'gone')",
        [&root],
    )
    .unwrap();
    con.execute_batch(
        "INSERT INTO AgLibraryFolder (id_local, id_global, pathFromRoot, rootFolder) VALUES
             (10, 'F10', '', 1), (11, 'F11', '2024/', 1), (12, 'F12', '', 2);
         INSERT INTO AgLibraryFile (id_local, id_global, baseName, extension, folder) VALUES
             (100, 'L100', 'a', 'jpg', 11), (101, 'L101', 'b', 'jpg', 11),
             (102, 'L102', 'c', 'jpg', 10), (103, 'L103', 'gone', 'jpg', 12);
         INSERT INTO Adobe_images (id_local, id_global, rootFile, masterImage, rating,
                                   colorLabels, pick) VALUES
             (1000, 'I1000', 100, NULL, 3.0, 'Red', 1.0),
             (1001, 'I1001', 100, 1000, 3.0, 'Red', 1.0),
             (1002, 'I1002', 101, NULL, NULL, 'Rouge', -1.0),
             (1003, 'I1003', 102, NULL, 0.0, '', 0.0),
             (1004, 'I1004', 103, NULL, 5.0, 'Blue', 0.0);
         INSERT INTO AgLibraryKeyword (id_local, id_global, name, parent) VALUES
             (1, 'K1', NULL, NULL), (2, 'K2', 'Places', 1), (3, 'K3', 'Lyon', 2),
             (4, 'K4', 'people', 1), (5, 'K5', 'copy only', 1);
         INSERT INTO AgLibraryKeywordImage (image, tag) VALUES
             (1000, 3), (1000, 4), (1001, 5);
         INSERT INTO AgLibraryCollection (id_local, creationId, name, parent, systemOnly) VALUES
             (20, 'com.adobe.ag.library.group', 'Trips', NULL, 0),
             (21, 'com.adobe.ag.library.collection', 'Lyon 2024', 20, 0),
             (22, 'com.adobe.ag.library.smart_collection', 'Rejects', NULL, 0),
             (23, 'com.adobe.ag.library.collection', 'quick collection', NULL, 1);
         INSERT INTO AgLibraryCollectionImage (collection, image) VALUES
             (21, 1000), (23, 1003);",
    )
    .unwrap();
    drop(con);
    path
}

fn params(source: &Path) -> Value {
    json!({"importer": IMPORTER, "kind": "file", "source": source})
}

fn name(image: &Value) -> String {
    Path::new(image["path"].as_str().unwrap())
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[test]
fn a_scan_lists_each_file_with_lightroom_s_marks_keywords_and_collections() {
    let dir = scratch("scan");
    let lrcat = catalogue(&dir);
    let before = std::fs::read(&lrcat).unwrap();
    let mut ticks = Vec::new();
    let result = scan(&params(&lrcat), &AtomicBool::new(false), &mut |d, t, _| {
        ticks.push((d, t));
    })
    .unwrap();
    let images = result["images"].as_array().unwrap();
    // One entry per file: a.jpg's virtual copy is not a second one; the
    // file on the missing drive is left out.
    let names: Vec<String> = images.iter().map(name).collect();
    assert_eq!(names, ["a.jpg", "b.jpg", "c.jpg"]);
    assert_eq!(
        Path::new(images[0]["path"].as_str().unwrap()),
        dir.join("photos/2024/a.jpg")
    );
    let a = &images[0];
    assert_eq!(a["rating"], 3);
    assert_eq!(a["rejected"], false);
    assert_eq!(a["labels"], json!(["red"]));
    // its keywords as paths, its copy's keyword, its collection under
    // its set
    assert_eq!(
        a["tags"],
        json!([
            format!("{COLLECTION_TAG}|Trips|Lyon 2024"),
            "Places|Lyon",
            "copy only",
            "people"
        ])
    );
    let b = &images[1];
    assert_eq!((&b["rejected"], b.get("rating")), (&json!(true), None));
    assert_eq!((b.get("labels"), b.get("tags")), (None, None));
    // the system-only quick collection is left out
    let c = &images[2];
    assert_eq!((&c["rating"], c.get("tags")), (&json!(0), None));

    let warnings: Vec<&str> = result["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    let has = |part: &str| warnings.iter().any(|w| w.contains(part));
    assert!(has("1 photo of the catalogue left out"), "{warnings:?}");
    assert!(has("/media/gone"), "{warnings:?}");
    assert!(has("1 virtual copy folded"), "{warnings:?}");
    assert!(has("1 photo flagged as picks"), "{warnings:?}");
    assert!(has("Rouge (1)"), "{warnings:?}");
    assert!(has("1 smart collection was not imported"), "{warnings:?}");
    assert_eq!(ticks.first(), Some(&(0, 4)));
    assert_eq!(ticks.last(), Some(&(4, 4)));
    // read-only: not a byte changed, no journal left beside it
    assert_eq!(std::fs::read(&lrcat).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_paged_scan_sends_the_same_images_in_pages() {
    let dir = scratch("paged");
    let lrcat = catalogue(&dir);
    let mut pages = Vec::new();
    let mut send = |images: Vec<Value>, done: usize, total: usize| {
        pages.push((images.len(), done, total));
    };
    let result = scan_paged(
        &params(&lrcat),
        &AtomicBool::new(false),
        &mut |_, _, _| {},
        Some((2, &mut send)),
    )
    .unwrap();
    assert_eq!((&result["pages"], &result["total"]), (&json!(2), &json!(4)));
    assert_eq!(pages, [(2, 2, 4), (1, 4, 4)]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_catalogue_lightroom_holds_open_is_read_from_a_copy() {
    let dir = scratch("open");
    let lrcat = catalogue(&dir);
    let mut lock = lrcat.as_os_str().to_owned();
    lock.push(".lock");
    std::fs::write(&lock, b"").unwrap();
    let result = scan(&params(&lrcat), &AtomicBool::new(false), &mut |_, _, _| {}).unwrap();
    assert_eq!(result["images"].as_array().unwrap().len(), 3);
    assert!(
        result["warnings"][0]
            .as_str()
            .unwrap()
            .starts_with("Lightroom had the catalogue open")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn what_is_not_a_lightroom_catalogue_is_refused() {
    let dir = scratch("refused");
    let text = dir.join("notes.lrcat");
    std::fs::write(&text, "not a database, just some text that is long enough").unwrap();
    let e = scan(&params(&text), &AtomicBool::new(false), &mut |_, _, _| {}).unwrap_err();
    assert_eq!(e.code(), NOT_A_SOURCE);
    assert!(
        e.message().contains("not a Lightroom Classic catalogue"),
        "{}",
        e.message()
    );

    let other = dir.join("other.lrcat");
    Connection::open(&other)
        .unwrap()
        .execute_batch("CREATE TABLE t (x);")
        .unwrap();
    let e = scan(&params(&other), &AtomicBool::new(false), &mut |_, _, _| {}).unwrap_err();
    assert_eq!(e.code(), NOT_A_SOURCE);
    assert!(
        e.message().contains("no table AgLibraryRootFolder"),
        "{}",
        e.message()
    );

    let older = dir.join("older.lrcat");
    Connection::open(&older)
        .unwrap()
        .execute_batch(&SCHEMA.replace("masterImage INTEGER,", ""))
        .unwrap();
    let e = scan(&params(&older), &AtomicBool::new(false), &mut |_, _, _| {}).unwrap_err();
    assert!(
        e.message().contains("Adobe_images lacks masterImage"),
        "{}",
        e.message()
    );

    let e = scan(
        &json!({"importer": IMPORTER, "kind": "folder", "source": dir}),
        &AtomicBool::new(false),
        &mut |_, _, _| {},
    )
    .unwrap_err();
    assert!(e.message().contains(".lrcat"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_shipped_program_scans_through_the_broker() {
    let dir = scratch("broker");
    let lrcat = catalogue(&dir);
    let mut dirs = Vec::new();
    let programs = Path::new(PROGRAM).parent().unwrap();
    let added = bundled::add_to(&mut dirs, &dir.join("plugins"), programs).unwrap();
    assert!(added.dir.is_some(), "{added:?}");
    let mut b = Broker::new(Policy {
        backoff: Duration::ZERO,
        ..Policy::default()
    });
    b.discover(&dirs);
    assert!(b.rejected().is_empty(), "{:?}", b.rejected());
    let (_, decl) = b
        .importers()
        .into_iter()
        .find(|(p, _)| p == PLUGIN)
        .expect("the Lightroom importer is declared");
    assert_eq!(decl.patterns, ["*.lrcat"]);
    let jpeg = |p: &Path| p.extension().is_some_and(|e| e == "jpg");
    let scan = b
        .importer_scan(PLUGIN, IMPORTER, &lrcat, &jpeg, None, |_| {})
        .unwrap();
    assert!(scan.refused.is_empty(), "{:?}", scan.refused);
    let photos = dir.join("photos").canonicalize().unwrap();
    let a = &scan.images[0];
    assert_eq!(a.path, photos.join("2024/a.jpg"));
    assert_eq!(
        (a.rating, a.rejected, a.labels.as_slice()),
        (Some(3), Some(false), ["red".to_string()].as_slice())
    );
    assert!(a.tags.contains(&"Places|Lyon".to_string()), "{:?}", a.tags);
    assert_eq!(scan.images.len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn develop_settings_come_mapped_with_the_unmatched_listed_per_photo() {
    let dir = scratch("develop");
    let lrcat = catalogue(&dir);
    let con = Connection::open(&lrcat).unwrap();
    con.execute_batch(
        r#"CREATE TABLE Adobe_imageDevelopSettings (id_local INTEGER PRIMARY KEY,
               allowFastRender INTEGER, croppedHeight, croppedWidth, digest,
               grayscale INTEGER, hasDevelopAdjustments INTEGER, historySettingsID,
               image INTEGER, processVersion, settingsID, text, validatedForVersion);
           INSERT INTO Adobe_imageDevelopSettings (image, processVersion, text) VALUES
               (1000, '11.0', 's = { Exposure2012 = -0.5, Contrast2012 = 20,
                   WhiteBalance = "As Shot", HasCrop = false,
                   ToneCurvePV2012 = { 0, 0, 255, 255 } }'),
               (1001, '11.0', 's = { Exposure2012 = 2 }'),
               (1002, '11.0', 's = { Exposure2012 = ');
           UPDATE Adobe_images SET orientation = 'DA' WHERE id_local = 1000;"#,
    )
    .unwrap();
    drop(con);
    // c.jpg has no catalogue row: its sidecar is read
    std::fs::write(
        dir.join("photos/c.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF><rdf:Description
            xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
            crs:Exposure2012="+1.00" crs:Vibrance="+12"/></rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();
    let result = scan(&params(&lrcat), &AtomicBool::new(false), &mut |_, _, _| {}).unwrap();
    let images = result["images"].as_array().unwrap();
    // a.jpg: the master's settings, not its virtual copy's
    assert_eq!(
        images[0]["develop"],
        json!({
            "from": "catalogue",
            "modules": [
                {"module": "exposure", "params": {"exposure": -0.5}},
                {"module": "flip", "params": {"orientation": 6}},
            ],
            "unmatched": {"Contrast2012": 20.0},
        })
    );
    assert_eq!(images[1].get("develop"), None);
    assert_eq!(images[2]["develop"]["from"], "xmp");
    assert_eq!(
        images[2]["develop"]["modules"],
        json!([{"module": "exposure", "params": {"exposure": 1.0}}])
    );
    let warnings: Vec<&str> = result["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    let has = |part: &str| warnings.iter().any(|w| w.contains(part));
    assert!(has("develop settings read for 2 photos"), "{warnings:?}");
    assert!(
        has("equivalent, not imported: a.jpg: Contrast2012 = +20"),
        "{warnings:?}"
    );
    assert!(has("c.jpg: Vibrance = +12"), "{warnings:?}");
    assert!(has("could not be read, skipped: b.jpg"), "{warnings:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
