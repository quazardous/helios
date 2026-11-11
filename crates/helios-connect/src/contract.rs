//! The contract every connector passes (`docs/design/import-connectors.md`,
//! slice 1): a fixture source of known files, and [`check`], which lists
//! it whole, page by page, from a cursor on a fresh instance, and reads
//! each item back. In-process connectors run it over a temporary tree;
//! the plugin adapter will run it through the broker.

use std::path::{Path, PathBuf};

use crate::{
    Access, Cancel, Connector, ConnectorError, Cursor, Item, ItemKind, ItemRef, PAGE, SourceRef,
};

/// A file of the fixture: its id in the source and its bytes.
#[derive(Clone, Debug)]
pub struct Expected {
    pub id: String,
    pub bytes: Vec<u8>,
    pub kind: ItemKind,
}

/// A source of known files.
#[derive(Clone, Debug)]
pub struct Fixture {
    /// Where it is on disk, for local connectors.
    pub root: PathBuf,
    /// The items it must list, in listing order.
    pub expected: Vec<Expected>,
    /// Ids of items the source holds as one DCF object (a RAW+JPEG pair).
    pub pair: [String; 2],
    /// An item of another object, never grouped with the pair.
    pub single: String,
}

impl Fixture {
    /// Writes a tree under `root`: a RAW+JPEG pair, a JPEG alone, a raw in
    /// a second folder, and files Helios does not read (a text file, an
    /// unknown extension). With `dcim`, they are under `DCIM/` as on a
    /// card, beside a folder outside `DCIM` that is not read.
    pub fn write(root: &Path, dcim: bool) -> std::io::Result<Self> {
        let _ = std::fs::remove_dir_all(root);
        let top = if dcim { "DCIM/" } else { "" };
        let files: [(&str, ItemKind); 4] = [
            ("100CANON/IMG_0001.CR3", ItemKind::Raw),
            ("100CANON/IMG_0001.JPG", ItemKind::Photo),
            ("100CANON/IMG_0002.JPG", ItemKind::Photo),
            ("101CANON/IMG_0003.NEF", ItemKind::Raw),
        ];
        let mut expected = Vec::new();
        for (i, (name, kind)) in files.iter().enumerate() {
            let id = format!("{top}{name}");
            let bytes: Vec<u8> = (0..(100 + i * 37)).map(|b| (b * (i + 3)) as u8).collect();
            let path = root.join(&id);
            std::fs::create_dir_all(path.parent().unwrap_or(root))?;
            std::fs::write(&path, &bytes)?;
            expected.push(Expected {
                id,
                bytes,
                kind: *kind,
            });
        }
        let ignored = root.join(format!("{top}100CANON"));
        std::fs::write(ignored.join("notes.txt"), b"not a photo")?;
        std::fs::write(ignored.join("IMG_0001.xyz"), b"unknown")?;
        if dcim {
            // Outside DCIM: a card is read from DCIM only.
            std::fs::create_dir_all(root.join("MISC"))?;
            std::fs::write(root.join("MISC/OTHER.JPG"), b"not under DCIM")?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            pair: [expected[0].id.clone(), expected[1].id.clone()],
            single: expected[2].id.clone(),
            expected,
        })
    }
}

fn ids(items: &[Item]) -> Vec<String> {
    items.iter().map(|i| i.id.0.clone()).collect()
}

/// Lists the whole source, `page` items at a time.
fn list_all(
    connector: &dyn Connector,
    source: &SourceRef,
    page: usize,
) -> Result<(Vec<Item>, usize), ConnectorError> {
    let cancel = Cancel::new();
    let mut items = Vec::new();
    let mut from: Option<Cursor> = None;
    let mut pages = 0;
    loop {
        let got = connector.list(source, from.as_ref(), page, &cancel)?;
        pages += 1;
        assert!(
            got.items.len() <= page,
            "a page holds at most what was asked"
        );
        items.extend(got.items);
        match (got.complete, got.next) {
            (true, None) => return Ok((items, pages)),
            (false, Some(next)) => from = Some(next),
            (complete, next) => {
                panic!("a page is complete without a cursor: complete {complete}, next {next:?}")
            }
        }
        assert!(pages <= 10_000, "the listing never ends");
    }
}

/// Runs the contract on the connector `make` builds, over `source`, which
/// holds `fixture`. `make` is called more than once: a cursor is resumed
/// on a fresh instance.
pub fn check(make: &dyn Fn() -> Box<dyn Connector>, source: &SourceRef, fixture: &Fixture) {
    let connector = make();
    let info = connector.describe();
    assert_eq!(info.id, source.connector, "the source names its connector");

    // Its sources include the fixture's.
    let sources = connector.sources(None).expect("sources");
    assert!(
        sources.iter().any(|s| s.id == source.id),
        "{}: {:?} not among its sources {sources:?}",
        info.id,
        source.id
    );

    // The whole listing: the files Helios reads, in a stable order.
    let (all, pages) = list_all(connector.as_ref(), source, PAGE).expect("list");
    assert_eq!(pages, 1);
    let want: Vec<String> = fixture.expected.iter().map(|e| e.id.clone()).collect();
    assert_eq!(ids(&all), want, "{}: the listing", info.id);
    for (item, e) in all.iter().zip(&fixture.expected) {
        assert_eq!(item.kind, e.kind, "{}: kind", e.id);
        if info.caps.sizes {
            assert_eq!(item.size, Some(e.bytes.len() as u64), "{}: size", e.id);
        }
        assert!(!item.path.is_empty(), "{}: a path to show", e.id);
    }

    // Page by page: the same items, each once.
    let (paged, pages) = list_all(connector.as_ref(), source, 2).expect("list paged");
    assert_eq!(ids(&paged), want, "{}: paged", info.id);
    assert_eq!(pages, want.len().div_ceil(2));

    // A cursor kept, resumed on a fresh instance: the rest, nothing twice.
    let cancel = Cancel::new();
    let first = connector
        .list(source, None, 1, &cancel)
        .expect("first page");
    let cursor = first.next.expect("a cursor after the first page");
    let fresh = make();
    let rest = fresh
        .list(source, Some(&cursor), PAGE, &cancel)
        .expect("resumed");
    assert!(rest.complete && rest.next.is_none());
    let mut resumed = ids(&first.items);
    resumed.extend(ids(&rest.items));
    assert_eq!(resumed, want, "{}: resumed from a cursor", info.id);

    // Groups: the RAW+JPEG pair shares one, the JPEG alone another.
    let group = |id: &str| {
        all.iter()
            .find(|i| i.id.0 == id)
            .and_then(|i| i.group.clone())
    };
    let pair = group(&fixture.pair[0]);
    assert!(pair.is_some(), "{}: the pair has a group", info.id);
    assert_eq!(pair, group(&fixture.pair[1]), "{}: one DCF object", info.id);
    assert_ne!(pair, group(&fixture.single), "{}: another object", info.id);

    // Each item's bytes: in place, or fetched; from an offset when it can
    // resume.
    for (item, e) in all.iter().zip(&fixture.expected) {
        let at = ItemRef {
            source: source.clone(),
            id: item.id.clone(),
        };
        if let Access::InPlace(path) = connector.access(&at) {
            assert_eq!(
                std::fs::read(&path).ok().as_ref(),
                Some(&e.bytes),
                "{}",
                e.id
            );
        }
        let mut sink = Vec::new();
        let fetched = connector.fetch(&at, 0, &mut sink, &cancel).expect("fetch");
        assert_eq!(fetched.bytes, e.bytes.len() as u64);
        assert_eq!(sink, e.bytes, "{}: fetched", e.id);
        if info.caps.resume {
            let mut tail = Vec::new();
            connector.fetch(&at, 7, &mut tail, &cancel).expect("resume");
            assert_eq!(tail, e.bytes[7..], "{}: from an offset", e.id);
        }
    }

    // Cancelled: no listing.
    let cancelled = Cancel::new();
    cancelled.cancel();
    assert_eq!(
        make().list(source, None, PAGE, &cancelled),
        Err(ConnectorError::Cancelled)
    );
}
