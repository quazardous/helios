//! The library read at the scale of a working photographer's archive
//! (`docs/design/library.md`, slice L1): a synthetic vault of many photos,
//! read as the desktop reads it when its window opens and after each
//! import or scan. Each read prints what it cost (time, store queries,
//! files stat-ed) as a JSON line, kept by nextest when the test passes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use helios_core::{Bus, ContentHash};
use helios_gui::library::Entry;
use helios_gui::persist::{Persist, read_library};
use helios_gui::{Shell, helios_panels};
use helios_vault::capture::CAPTURED;
use helios_vault::fields::{LABELS, RATING, TAGS};
use helios_vault::storage::VolumeId;
use helios_vault::{Image, ImageId, Location, Order, Presence, Query, Sort, Store, Vault};
use helios_vault_sqlite::SqliteStore;
use serde_json::json;

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-scale-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The bytes of every photo file on disk: smaller than any size the vault
/// holds, so an entry showing it was read from the file (a `stat`), not
/// from the vault.
const ON_DISK: &[u8] = b"photo";

/// The size the vault holds for photo `i`.
fn stored_size(i: u32) -> u64 {
    10_000 + u64::from(i)
}

/// When the vault saw photo `i` last modified.
fn stored_modified(i: u32) -> u64 {
    1_700_000_000 + u64::from(i) * 60
}

/// When photo `i` was taken: the later photos earlier, a third never by
/// a camera.
fn captured(i: u32) -> Option<u64> {
    (!i.is_multiple_of(3)).then(|| 1_600_000_000 - u64::from(i) * 60)
}

/// The photos in the order a sort shows them: by id, or by capture time
/// (the photos never taken last, by their file name, `IMG_{i}` for every
/// photo).
fn sorted(n: u32, order: Order) -> Vec<u32> {
    let mut ids: Vec<u32> = (0..n).collect();
    match order.by {
        Sort::Id => ids.sort_by_key(|&i| id(i)),
        Sort::Captured => ids.sort_by_key(|&i| (captured(i).is_none(), captured(i), i)),
        other => panic!("no expected order for {other:?}"),
    }
    ids
}

fn id(i: u32) -> ImageId {
    let mut bytes = [0u8; 32];
    bytes[..4].copy_from_slice(&i.to_be_bytes());
    bytes[31] = 1;
    ImageId(ContentHash(bytes))
}

/// What photo `i` is: present (seven in ten), on an unplugged volume,
/// missing, or missing at one path and present at another.
fn presence(i: u32) -> Presence {
    match i % 10 {
        7 => Presence::Offline,
        8 => Presence::Missing,
        _ => Presence::Present,
    }
}

/// A vault of `n` photos in `dir`: the present ones are hard links to one
/// small file, each with its size and date as the vault saw them; marks on
/// some.
fn synthetic(dir: &Path, n: u32) -> PathBuf {
    grow(dir, 0..n)
}

/// The vault in `dir` with photos `ids` added (created when it is not
/// there): a library measured at one size grows to the next instead of
/// being built again.
fn grow(dir: &Path, ids: std::ops::Range<u32>) -> PathBuf {
    // One source file per 20,000 links: a file system caps a file's
    // hard links (ext4 at 65,000), and the measure goes to 100,000 photos.
    let source = |i: u32| {
        let file = dir.join(format!("photo-{}.raw", i / 20_000));
        if !file.exists() {
            std::fs::write(&file, ON_DISK).unwrap();
        }
        file
    };
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    let path = dir.join("vault.db");
    let mut store = SqliteStore::open(&path).unwrap();
    store.begin().unwrap();
    for i in ids {
        let id = id(i);
        store
            .add_to_library(&Image {
                id,
                size: stored_size(i),
                partial: id.0,
            })
            .unwrap();
        let at = photos.join(format!("IMG_{i:06}.nef"));
        let seen = Location::seen(at.clone(), stored_size(i), stored_modified(i), 1);
        let location = match i % 10 {
            7 => Location {
                volume: Some(VolumeId("uuid:away".into())),
                rel_path: Some(format!("IMG_{i:06}.nef").into()),
                presence: Presence::Offline,
                ..seen
            },
            8 => Location {
                presence: Presence::Missing,
                ..seen
            },
            9 => {
                store
                    .put_location(
                        &id,
                        &Location {
                            presence: Presence::Missing,
                            ..Location::seen(
                                photos.join(format!("AAA_{i:06}.nef")),
                                stored_size(i),
                                1,
                                1,
                            )
                        },
                    )
                    .unwrap();
                std::fs::hard_link(source(i), &at).unwrap();
                seen
            }
            _ => {
                std::fs::hard_link(source(i), &at).unwrap();
                seen
            }
        };
        store.put_location(&id, &location).unwrap();
        if i.is_multiple_of(2) {
            store.set_field(&id, RATING, Some(&json!(i % 6))).unwrap();
        }
        if i.is_multiple_of(7) {
            store.set_field(&id, LABELS, Some(&json!(["red"]))).unwrap();
        }
        if i.is_multiple_of(11) {
            store.set_field(&id, TAGS, Some(&json!(["x"]))).unwrap();
        }
        if let Some(t) = captured(i) {
            store.set_field(&id, CAPTURED, Some(&json!(t))).unwrap();
        }
    }
    store.commit().unwrap();
    path
}

/// Photo `i` as the library must show it.
fn check(i: u32, e: &Entry) {
    assert_eq!(e.id, id(i).hex());
    assert_eq!(e.presence, presence(i), "photo {i}");
    assert_eq!(
        e.offline_on.as_deref(),
        (i % 10 == 7).then_some("uuid:away"),
        "photo {i}"
    );
    let present = presence(i) == Presence::Present;
    let name = if present {
        format!("IMG_{i:06}.nef")
    } else {
        id(i).hex()[..12].to_string()
    };
    assert_eq!(e.name, name);
    assert_eq!(
        e.rating,
        if i.is_multiple_of(2) {
            i64::from(i % 6)
        } else {
            0
        }
    );
    assert_eq!(e.labels.is_empty(), !i.is_multiple_of(7));
    assert_eq!(e.tags.is_empty(), !i.is_multiple_of(11));
}

/// What one read cost.
struct Cost {
    images: usize,
    elapsed: Duration,
    queries: u64,
    /// Entries whose size is the file's on disk: read by a `stat`.
    stats: usize,
}

fn read(path: &Path, n: u32, order: Order) -> Result<Cost, String> {
    let vault = Vault::new(SqliteStore::open(path).unwrap());
    let q0 = vault.reads().queries;
    let start = Instant::now();
    let rows = read_library(&vault, order)?;
    let elapsed = start.elapsed();
    let cost = Cost {
        images: rows.len(),
        elapsed,
        queries: vault.reads().queries - q0,
        stats: rows
            .entries()
            .iter()
            .filter(|e| e.bytes == Some(ON_DISK.len() as u64))
            .count(),
    };
    assert_eq!(rows.len(), n as usize);
    for (i, e) in sorted(n, order).into_iter().zip(rows.entries()) {
        check(i, e);
    }
    Ok(cost)
}

/// A measure, not a deadline: the library read at growing sizes, each
/// read's cost printed (its time, queries and stats, and the time the
/// vault took to grow to that size). What the read must cost in queries
/// and stats is checked by the test below; this one checks only that each
/// read shows every photo as the vault holds it. One vault grows from one
/// size to the next, so the fixture is built once, whatever the machine's
/// speed.
#[test]
fn the_library_read_at_scale_measured() {
    let dir = scratch("read");
    let mut built = 0;
    for n in [30_000, 100_000] {
        let start = Instant::now();
        let path = grow(&dir, built..n);
        built = n;
        let grown = start.elapsed();
        let line = match read(&path, n, helios_gui::library::DEFAULT_SORT) {
            Ok(c) => json!({
                "images": c.images,
                "ms": c.elapsed.as_millis(),
                "queries": c.queries,
                "stats": c.stats,
                "built_ms": grown.as_millis(),
            }),
            Err(e) => json!({ "images": n, "error": e }),
        };
        println!("library-read {line}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A shell whose library is filled from the vault at `path`.
fn open(path: &Path) -> (Shell, Persist<SqliteStore>) {
    let mut shell = Shell::new(helios_panels(), None);
    let persist = Persist::new(SqliteStore::open(path).unwrap(), Arc::new(Bus::new()));
    persist.fill_library(&mut shell).unwrap();
    (shell, persist)
}

/// The read's cost does not grow a query or a file system call per photo:
/// one query opens the cursor, one reads each window of
/// `helios_gui::persist::PAGE` photos, and the sizes and dates shown are the
/// vault's, never a `stat`'s. Bound on counts, not seconds: a read gone
/// slow per photo shows as queries or stats, whatever the machine's speed.
/// Sorted by capture time (the library's default) the photos come in its
/// order from the same query.
#[test]
fn the_library_read_at_scale_is_a_query_per_window_and_no_stat() {
    let n = 50_000;
    let dir = scratch("guard");
    let path = synthetic(&dir, n);
    for order in [Order::ID, helios_gui::library::DEFAULT_SORT] {
        let cost = read(&path, n, order).unwrap();
        let windows = (n as usize).div_ceil(helios_gui::persist::PAGE) as u64;
        assert!(
            cost.queries <= windows + 1,
            "{order:?}: {} queries for {n} photos: one per photo again?",
            cost.queries
        );
        assert_eq!(cost.stats, 0, "the read stat-ed the photos' files");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The whole filter runs in the vault (`docs/design/library.md`, L4): a
/// filtered read at scale is still one query to open the cursor and one
/// per window, and the facet counts are one query more, whatever the
/// library's size. Words, presence and rating together here.
#[test]
fn a_filtered_read_at_scale_is_a_query_per_window_and_facets_one_more() {
    use helios_gui::library::query_of;
    let n = 50_000;
    let dir = scratch("filter");
    let path = synthetic(&dir, n);
    let vault = Vault::new(SqliteStore::open(&path).unwrap());
    let filter =
        egui_pop::views::library::filter::Filter::parse("img_01 presence:present rating>=4");
    let query = Query {
        order: helios_gui::library::DEFAULT_SORT,
        ..query_of(&filter)
    };
    let want: Vec<u32> = sorted(n, helios_gui::library::DEFAULT_SORT)
        .into_iter()
        .filter(|&i| {
            presence(i) == Presence::Present
                && (10_000..20_000).contains(&i)
                && i.is_multiple_of(2)
                && i % 6 >= 4
        })
        .collect();
    assert!(!want.is_empty());
    let q0 = vault.reads().queries;
    let start = Instant::now();
    let cursor = vault.open_cursor(query.clone()).unwrap();
    let mut got = Vec::new();
    for offset in (0..cursor.count).step_by(helios_gui::persist::PAGE) {
        for row in vault
            .fetch_library(cursor.id, offset, helios_gui::persist::PAGE, &[RATING])
            .unwrap()
        {
            got.push(row.id);
        }
    }
    let windows = cursor.count.div_ceil(helios_gui::persist::PAGE) as u64;
    assert_eq!(got, want.iter().map(|&i| id(i)).collect::<Vec<_>>());
    assert!(
        vault.reads().queries - q0 <= windows + 1,
        "{} queries for {} photos",
        vault.reads().queries - q0,
        got.len()
    );
    let q1 = vault.reads().queries;
    let facets = vault.facets(&query, None).unwrap();
    assert_eq!(vault.reads().queries - q1, 1, "every facet in one query");
    assert_eq!(facets.count("presence", "present"), want.len() as u64);
    assert_eq!(
        facets.count("rating", "4") + facets.count("rating", "5"),
        want.len() as u64
    );
    let elapsed = start.elapsed();
    println!(
        "library-filter {}",
        json!({ "images": n, "matched": want.len(), "ms": elapsed.as_millis() })
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// After a scan or an import, the library shown reads only the photos added
/// and those whose copies moved, and ends as a whole read would show it;
/// with nothing changed it reads nothing.
#[test]
fn a_refresh_reads_only_what_changed_and_shows_what_a_whole_read_shows() {
    let n = 1_000;
    let dir = scratch("refresh");
    let path = synthetic(&dir, n);
    let (mut shell, persist) = open(&path);
    let vault = persist.vault().clone();
    assert_eq!(shell.library().entries.len(), n as usize);

    let q0 = vault.reads().queries;
    persist.fill_library(&mut shell).unwrap();
    assert_eq!(vault.reads().queries, q0, "nothing changed: nothing read");

    // A scan finds photo 7's volume plugged in, photo 8 is relinked.
    let away = vault.locations(&id(7)).unwrap().remove(0);
    let back = Location {
        presence: Presence::Present,
        ..away.clone()
    };
    let lost = vault.locations(&id(8)).unwrap().remove(0);
    let found = Location {
        presence: Presence::Present,
        ..lost.clone()
    };
    vault
        .update_locations(&[
            (id(7), away.path.clone(), back),
            (id(8), lost.path.clone(), found),
        ])
        .unwrap();
    // An import adds a photo.
    let new = dir.join("photos").join("NEW.nef");
    std::fs::write(&new, b"a new photo").unwrap();
    vault.import_file(&new, 5).unwrap();
    let q0 = vault.reads().queries;
    persist.fill_library(&mut shell).unwrap();
    assert!(
        vault.reads().queries - q0 <= 2,
        "a refresh is the cursor's query and one window of what changed: {}",
        vault.reads().queries - q0
    );
    let refreshed = shell.library().entries.clone();
    assert_eq!(refreshed.len(), n as usize + 1);
    let shown = |i: u32| refreshed.iter().find(|e| e.id == id(i).hex()).unwrap();
    assert_eq!(shown(7).presence, Presence::Present);
    assert_eq!(shown(7).offline_on, None);
    assert_eq!(shown(8).presence, Presence::Present);
    assert_eq!(shown(8).name, "IMG_000008.nef");

    // As a library filled from scratch shows it.
    let (whole, _) = open(&path);
    assert_eq!(refreshed, whole.library().entries);
    drop((shell, persist, whole));
    let _ = std::fs::remove_dir_all(&dir);
}
