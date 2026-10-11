//! The contract every [`Store`] must pass: the vault's rules and budgets
//! hold whatever keeps the rows.
//!
//! ```ignore
//! helios_vault::contract::check(|| helios_vault_sqlite::SqliteStore::in_memory().unwrap());
//! ```

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use std::sync::Arc;

use helios_core::{Bus, ContentHash};
use serde_json::json;

use crate::fields::{Command, Order, Query};
use crate::{Image, ImageId, Location, Store, Vault};

/// Runs every scenario against fresh stores made by `make`.
pub fn check<S: Store>(make: impl Fn() -> S) {
    images_are_added_once(make());
    locations_belong_to_one_image_and_round_trip(make());
    import_reads_only_what_changed(make(), &scratch("import"));
    repair_relinks_by_content_reading_as_little_as_possible(make(), &scratch("repair"));
    fields_round_trip_and_come_in_one_batch(make());
    queries_filter_and_order(make());
    commands_skip_unchanged_values_coalesce_and_audit(make());
    cursors_cost_one_query_and_report_deltas(make());
    versions_are_content_addressed_and_heads_follow_forks_and_merges(make());
}

fn versions_are_content_addressed_and_heads_follow_forks_and_merges<S: Store>(store: S) {
    use crate::versions::Version;

    let bus = Arc::new(Bus::new());
    let vault = Vault::new(store).with_bus(bus.clone());
    let img = image(1);
    let edit = |ev: f64| json!({ "modules": [{ "name": "exp", "module": "exposure", "params": { "exposure": { "float": ev } } }] });
    let first = Version::new(img.id, vec![], edit(0.5), "import:xmp", 10);
    assert!(
        vault.add_version(&first).is_err(),
        "an unknown image has no versions"
    );
    {
        vault.store.lock().unwrap().add_image(&img).unwrap();
    }
    assert!(vault.add_version(&first).unwrap());
    // The same edit on the same parents is the same version, whoever and whenever.
    let again = Version::new(img.id, vec![], edit(0.5), "user", 99);
    assert_eq!(again.id, first.id);
    assert!(!vault.add_version(&again).unwrap());

    // Two edits from the same parent: both kept, two heads.
    let user = Version::new(img.id, vec![first.id], edit(1.0), "user", 20).labelled("brighter");
    let agent = Version::new(img.id, vec![first.id], edit(0.8), "agent:a1", 21);
    vault.add_version(&user).unwrap();
    vault.add_version(&agent).unwrap();
    let heads: Vec<_> = vault.heads(&img.id).unwrap().iter().map(|v| v.id).collect();
    assert_eq!(heads, [user.id, agent.id]);
    // A merge has both as parents, in either order the same version.
    let merge = Version::new(img.id, vec![agent.id, user.id], edit(0.9), "user", 30);
    assert_eq!(
        merge.id,
        Version::new(img.id, vec![user.id, agent.id], edit(0.9), "user", 30).id
    );
    vault.add_version(&merge).unwrap();
    assert_eq!(vault.heads(&img.id).unwrap(), [merge.clone()]);

    let all = vault.versions(&img.id).unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(all[0], first);
    assert_eq!(
        vault.version(&user.id).unwrap().unwrap().label.as_deref(),
        Some("brighter")
    );

    // A parent must be a version of the same image.
    let other = image(2);
    vault.store.lock().unwrap().add_image(&other).unwrap();
    let stray = Version::new(other.id, vec![first.id], edit(0.1), "user", 40);
    assert!(
        vault
            .add_version(&stray)
            .unwrap_err()
            .contains("another image")
    );

    let announced = bus
        .since(0)
        .iter()
        .filter(|e| e.kind == "vault.version_added")
        .count();
    assert_eq!(announced, 4);
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-vault-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn image(n: u8) -> Image {
    Image {
        id: ImageId(ContentHash([n; 32])),
        size: 100 + u64::from(n),
        partial: ContentHash([n + 1; 32]),
    }
}

fn location(path: &str) -> Location {
    Location {
        path: PathBuf::from(path),
        size: 101,
        modified: 5,
        last_verified: 7,
        present: true,
    }
}

fn images_are_added_once<S: Store>(mut s: S) {
    assert!(s.add_image(&image(1)).unwrap());
    assert!(!s.add_image(&image(1)).unwrap(), "added twice");
    s.add_image(&image(2)).unwrap();
    assert_eq!(s.image(&image(1).id).unwrap(), Some(image(1)));
    assert_eq!(s.image(&image(9).id).unwrap(), None);
    assert_eq!(s.images().unwrap().len(), 2);
}

fn locations_belong_to_one_image_and_round_trip<S: Store>(mut s: S) {
    assert!(
        s.put_location(&image(1).id, &location("/a.nef"))
            .unwrap_err()
            .contains("no image")
    );
    s.add_image(&image(1)).unwrap();
    s.add_image(&image(2)).unwrap();
    let lost = Location {
        present: false,
        ..location("/b.nef")
    };
    s.put_location(&image(1).id, &location("/a.nef")).unwrap();
    s.put_location(&image(1).id, &lost).unwrap();
    assert_eq!(
        s.locations(&image(1).id).unwrap(),
        vec![location("/a.nef"), lost.clone()]
    );
    assert_eq!(
        s.at_path(Path::new("/b.nef")).unwrap(),
        Some((image(1).id, lost))
    );
    // A path now holding another image's content belongs to that image.
    s.put_location(&image(2).id, &location("/a.nef")).unwrap();
    assert_eq!(
        s.at_path(Path::new("/a.nef")).unwrap().unwrap().0,
        image(2).id
    );
    assert_eq!(s.locations(&image(1).id).unwrap().len(), 1);
    assert_eq!(s.all_locations().unwrap().len(), 2);
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Bytes of a fake raw: different seeds give different content of `len` bytes.
fn content(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn import_reads_only_what_changed<S: Store>(s: S, dir: &Path) {
    let v = Vault::new(s);
    write(&dir.join("a/one.NEF"), &content(1, 200_000));
    write(&dir.join("a/two.dng"), &content(2, 1_000));
    write(&dir.join("a/notes.txt"), b"not an image");
    let first = v.import(dir, 10).unwrap();
    assert_eq!((first.added, first.located, first.unchanged), (2, 0, 0));
    assert_eq!(v.reads().full_hashes, 2);

    let before = v.reads();
    let again = v.import(dir, 20).unwrap();
    assert_eq!(again.unchanged, 2);
    assert_eq!(v.reads(), before, "an unchanged file is never read again");

    write(&dir.join("b/copy.nef"), &content(1, 200_000));
    let copy = v.import(dir, 30).unwrap();
    assert_eq!(
        (copy.added, copy.located),
        (0, 1),
        "the same content is the same image"
    );
    let id = ImageId(ContentHash(*blake3::hash(&content(1, 200_000)).as_bytes()));
    assert_eq!(v.locations(&id).unwrap().len(), 2);
    assert_eq!(v.images().unwrap().len(), 2);
}

fn repair_relinks_by_content_reading_as_little_as_possible<S: Store>(s: S, dir: &Path) {
    let v = Vault::new(s);
    write(&dir.join("shoot/kept.raf"), &content(1, 300_000));
    write(&dir.join("shoot/moved.raf"), &content(2, 300_000));
    write(&dir.join("shoot/gone.raf"), &content(3, 4_000));
    v.import(&dir.join("shoot"), 10).unwrap();

    std::fs::rename(dir.join("shoot/moved.raf"), dir.join("elsewhere.raf")).unwrap_or_else(|_| {
        write(&dir.join("elsewhere.raf"), &content(2, 300_000));
        std::fs::remove_file(dir.join("shoot/moved.raf")).unwrap();
    });
    std::fs::remove_file(dir.join("shoot/gone.raf")).unwrap();
    // Same size as the moved file, other content: read partially, never fully.
    write(&dir.join("decoy/other.raf"), &content(9, 300_000));
    // Other size: never read at all.
    write(&dir.join("decoy/small.raf"), &content(8, 10));

    let before = v.reads();
    let report = v.repair(&[dir], 20).unwrap();
    assert_eq!((report.lost, report.relinked, report.missing), (2, 1, 1));
    let reads = v.reads();
    assert_eq!(
        reads.full_hashes - before.full_hashes,
        1,
        "only the true candidate is fully hashed"
    );
    assert_eq!(
        reads.partial_hashes - before.partial_hashes,
        2,
        "only same-size files are partially hashed"
    );

    let moved = ImageId(ContentHash(*blake3::hash(&content(2, 300_000)).as_bytes()));
    let present: Vec<PathBuf> = v
        .locations(&moved)
        .unwrap()
        .into_iter()
        .filter(|l| l.present)
        .map(|l| l.path)
        .collect();
    assert_eq!(present, vec![dir.join("elsewhere.raf")]);
    let _ = std::fs::remove_dir_all(dir);
}

fn with_images<S: Store>(mut s: S, n: u8) -> S {
    for i in 1..=n {
        s.add_image(&image(i)).unwrap();
    }
    s
}

fn fields_round_trip_and_come_in_one_batch<S: Store>(s: S) {
    let mut s = with_images(s, 3);
    let (a, b, c) = (image(1).id, image(2).id, image(3).id);
    assert!(
        s.set_field(&image(9).id, "rating", Some(&json!(3)))
            .unwrap_err()
            .contains("no image")
    );
    s.set_field(&a, "rating", Some(&json!(4))).unwrap();
    s.set_field(&a, "labels", Some(&json!(["blue", "red"])))
        .unwrap();
    s.set_field(&b, "title", Some(&json!("Fjord"))).unwrap();
    assert_eq!(s.field(&a, "rating").unwrap(), Some(json!(4)));
    let rows = s.fields_of(&[c, a, b], &["rating", "title"]).unwrap();
    assert_eq!(rows[0].len(), 0, "an image without the fields");
    assert_eq!(rows[1].get("rating"), Some(&json!(4)));
    assert!(!rows[1].contains_key("labels"), "only the fields asked for");
    assert_eq!(rows[2].get("title"), Some(&json!("Fjord")));
    s.set_field(&a, "rating", None).unwrap();
    assert_eq!(s.field(&a, "rating").unwrap(), None);
}

fn queries_filter_and_order<S: Store>(s: S) {
    let mut s = with_images(s, 4);
    let ids: Vec<ImageId> = (1..=4).map(|i| image(i).id).collect();
    s.set_field(&ids[0], "rating", Some(&json!(2))).unwrap();
    s.set_field(&ids[1], "rating", Some(&json!(5))).unwrap();
    s.set_field(&ids[2], "rating", Some(&json!(-1))).unwrap();
    s.set_field(&ids[1], "labels", Some(&json!(["red"])))
        .unwrap();
    s.set_field(&ids[3], "labels", Some(&json!(["blue", "red"])))
        .unwrap();
    assert_eq!(s.select(&Query::default()).unwrap(), ids);
    let rated = Query {
        min_rating: Some(1),
        ..Query::default()
    };
    assert_eq!(s.select(&rated).unwrap(), vec![ids[0], ids[1]]);
    let red = Query {
        label: Some("red".into()),
        ..Query::default()
    };
    assert_eq!(s.select(&red).unwrap(), vec![ids[1], ids[3]]);
    let by_rating = Query {
        order: Order::RatingDesc,
        ..Query::default()
    };
    assert_eq!(
        s.select(&by_rating).unwrap(),
        vec![ids[1], ids[0], ids[3], ids[2]],
        "unrated counts as 0, rejected as -1"
    );
}

fn commands_skip_unchanged_values_coalesce_and_audit<S: Store>(s: S) {
    let bus = Arc::new(Bus::new());
    let v = Vault::new(with_images(s, 2)).with_bus(bus.clone());
    let (a, b) = (image(1).id, image(2).id);
    let set = |image, field: &str, value| Command {
        image,
        field: field.into(),
        value: Some(value),
    };
    let report = v
        .apply(
            &[
                set(a, "rating", json!(3)),
                set(a, "rating", json!(4)),
                set(b, "labels", json!(["red", "blue", "red"])),
            ],
            "user",
            Some(7),
            100,
        )
        .unwrap();
    assert_eq!(
        (report.changed, report.coalesced, report.unchanged),
        (2, 1, 0)
    );
    let again = v
        .apply(
            &[
                set(b, "labels", json!(["blue", "red"])),
                set(a, "rating", json!(4)),
            ],
            "user",
            None,
            101,
        )
        .unwrap();
    assert_eq!(
        (again.changed, again.unchanged),
        (0, 2),
        "equal values are not written"
    );
    let audit = v.audit(&a).unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(
        (audit[0].old.clone(), audit[0].new.clone(), audit[0].job),
        (None, Some(json!(4)), Some(7))
    );
    assert_eq!(bus.since(0).len(), 2, "one event per real change");
    let refused = v.apply(
        &[set(a, "rating", json!(1)), set(a, "rating", json!(9))],
        "user",
        None,
        102,
    );
    assert!(
        refused
            .unwrap_err()
            .contains("not a whole number in [-1, 5]")
    );
    assert_eq!(
        v.audit(&a).unwrap().len(),
        1,
        "a refused batch writes nothing"
    );
    assert!(
        v.apply(&[set(a, "nope", json!(1))], "user", None, 103)
            .unwrap_err()
            .contains("unknown field")
    );
}

fn cursors_cost_one_query_and_report_deltas<S: Store>(s: S) {
    let v = Vault::new(with_images(s, 5));
    let ids: Vec<ImageId> = (1..=5).map(|i| image(i).id).collect();
    let rate = |image, stars: i64| Command {
        image,
        field: "rating".into(),
        value: Some(json!(stars)),
    };
    v.apply(&[rate(ids[0], 3), rate(ids[2], 5)], "user", None, 1)
        .unwrap();
    let q0 = v.reads().queries;
    let cursor = v
        .open_cursor(Query {
            min_rating: Some(3),
            order: Order::RatingDesc,
            ..Query::default()
        })
        .unwrap();
    assert_eq!(cursor.count, 2);
    let rows = v.fetch(cursor.id, 0, 50, &["rating", "labels"]).unwrap();
    assert_eq!(
        v.reads().queries - q0,
        2,
        "open = one query, a window = one query, whatever its size"
    );
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![ids[2], ids[0]]
    );
    assert_eq!(rows[0].fields.get("rating"), Some(&json!(5)));
    assert_eq!(v.rank_of(cursor.id, &ids[0]).unwrap(), Some(1));

    let unchanged = v.refresh(cursor.id).unwrap();
    assert!(!unchanged.changed);
    assert_eq!(
        v.reads().queries - q0,
        2,
        "refreshing an unchanged catalog costs nothing"
    );

    v.apply(&[rate(ids[4], 4), rate(ids[0], 1)], "user", None, 2)
        .unwrap();
    let delta = v.refresh(cursor.id).unwrap();
    assert_eq!(
        (delta.inserted.clone(), delta.removed.clone(), delta.count),
        (vec![ids[4]], vec![ids[0]], 2)
    );
    assert_eq!(v.rank_of(cursor.id, &ids[4]).unwrap(), Some(1));
    v.close_cursor(cursor.id);
    assert!(v.fetch(cursor.id, 0, 1, &[]).is_err());
}
