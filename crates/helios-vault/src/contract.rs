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

use crate::fields::{Command, LABELS, Op, Order, Query, RATING, REJECTED};
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
    documents_round_trip_by_namespace(make());
    transactions_keep_all_or_nothing(make());
    a_batch_failing_at_its_audit_row_leaves_nothing(make());
    label_commands_keep_what_another_writer_added(make());
    taking_a_reject_back_keeps_the_stars(make());
    a_default_never_overwrites_a_choice(make());
    a_stale_writer_changes_only_its_own_fields(make());
}

fn labels_of<S: Store>(v: &Vault<S>, id: ImageId) -> Option<serde_json::Value> {
    v.fields_of(&[id], &[LABELS]).unwrap()[0]
        .get(LABELS)
        .cloned()
}

/// darktable's "labels wiped on refresh": a writer whose copy of the set
/// is stale adds or removes one label; the label another writer added
/// meanwhile stays, because the vault applies the change to the stored
/// set.
fn label_commands_keep_what_another_writer_added<S: Store>(s: S) {
    let bus = Arc::new(Bus::new());
    let v = Vault::new(with_images(s, 1)).with_bus(bus.clone());
    let a = image(1).id;
    // Both writers saw no label. The agent adds red, then the user, from
    // that stale view, adds blue.
    v.apply(&[Command::add(a, LABELS, "red")], "agent", None, 1)
        .unwrap();
    v.apply(&[Command::add(a, LABELS, "blue")], "user", None, 2)
        .unwrap();
    assert_eq!(labels_of(&v, a), Some(json!(["blue", "red"])));
    // Taking blue off keeps red.
    v.apply(&[Command::remove(a, LABELS, "blue")], "user", None, 3)
        .unwrap();
    assert_eq!(labels_of(&v, a), Some(json!(["red"])));
    // Removing a label that is not there changes nothing: no row, no event.
    let before = bus.last_seq();
    let report = v
        .apply(&[Command::remove(a, LABELS, "green")], "user", None, 4)
        .unwrap();
    assert_eq!((report.changed, report.unchanged), (0, 1));
    assert_eq!(bus.last_seq(), before);
    // A set left empty is cleared, as a set never written.
    v.apply(&[Command::remove(a, LABELS, "red")], "user", None, 5)
        .unwrap();
    assert_eq!(labels_of(&v, a), None);
    // Commands on one field fold in order.
    let folded = v
        .apply(
            &[
                Command::add(a, LABELS, "green"),
                Command::add(a, LABELS, "purple"),
                Command::remove(a, LABELS, "green"),
            ],
            "user",
            None,
            6,
        )
        .unwrap();
    assert_eq!((folded.changed, folded.coalesced), (1, 2));
    assert_eq!(labels_of(&v, a), Some(json!(["purple"])));
    assert!(
        v.apply(&[Command::add(a, RATING, "red")], "user", None, 7)
            .unwrap_err()
            .contains("not a set")
    );
    assert!(
        v.apply(&[Command::add(a, LABELS, "pink")], "user", None, 8)
            .unwrap_err()
            .contains("not one of")
    );
}

/// darktable keeps a reject as rating -1, so taking it back writes 0 and
/// erases the stars. Here the reject is a field of its own.
fn taking_a_reject_back_keeps_the_stars<S: Store>(s: S) {
    let v = Vault::new(with_images(s, 1));
    let a = image(1).id;
    v.apply(&[Command::set(a, RATING, json!(4))], "user", None, 1)
        .unwrap();
    v.apply(&[Command::set(a, REJECTED, json!(true))], "user", None, 2)
        .unwrap();
    let both = |v: &Vault<S>| v.fields_of(&[a], &[RATING, REJECTED]).unwrap()[0].clone();
    assert_eq!(both(&v).get(RATING), Some(&json!(4)));
    assert_eq!(both(&v).get(REJECTED), Some(&json!(true)));
    v.apply(&[Command::clear(a, REJECTED)], "user", None, 3)
        .unwrap();
    assert_eq!(both(&v).get(RATING), Some(&json!(4)), "the stars are back");
    assert_eq!(both(&v).get(REJECTED), None);
    assert!(
        v.apply(&[Command::set(a, RATING, json!(-1))], "user", None, 4)
            .is_err(),
        "a reject is not a rating"
    );
}

/// A default (a sidecar's rating) fills an empty field and never replaces
/// what is there.
fn a_default_never_overwrites_a_choice<S: Store>(s: S) {
    let v = Vault::new(with_images(s, 1));
    let a = image(1).id;
    let default = |stars: i64| Command::with(a, RATING, Op::SetIfAbsent(json!(stars)));
    assert_eq!(
        v.apply(&[default(3)], "import", None, 1).unwrap().changed,
        1
    );
    v.apply(&[Command::set(a, RATING, json!(5))], "user", None, 2)
        .unwrap();
    let report = v.apply(&[default(2)], "import", None, 3).unwrap();
    assert_eq!((report.changed, report.unchanged), (0, 1));
    assert_eq!(
        v.fields_of(&[a], &[RATING]).unwrap()[0].get(RATING),
        Some(&json!(5))
    );
}

/// darktable#22569: a decode job copied the whole image record, decoded,
/// then wrote its stale copy back over a rating set meanwhile. A writer
/// here changes only the fields it names: a job writing what it decoded,
/// from a copy read before the rating changes, races a user rating the
/// image, and every rating survives.
fn a_stale_writer_changes_only_its_own_fields<S: Store>(s: S) {
    let v = Vault::new(with_images(s, 1));
    let a = image(1).id;
    let rounds = 40;
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for i in 0..rounds {
                let stars = i % 5 + 1;
                v.apply(&[Command::set(a, RATING, json!(stars))], "user", None, i)
                    .unwrap();
            }
        });
        scope.spawn(|| {
            // The job's copy, read once before it starts.
            let stale = v.fields_of(&[a], &[RATING, "title"]).unwrap();
            for i in 0..rounds {
                let title = format!("decoded {i}");
                v.apply(
                    &[Command::set(a, "title", json!(title))],
                    "decode",
                    Some(9),
                    i,
                )
                .unwrap();
            }
            drop(stale);
        });
    });
    let now = v.fields_of(&[a], &[RATING, "title"]).unwrap()[0].clone();
    assert_eq!(now.get(RATING), Some(&json!((rounds - 1) % 5 + 1)));
    assert_eq!(
        now.get("title"),
        Some(&json!(format!("decoded {}", rounds - 1)))
    );
    // Every rating the user set is in the trail, each from the one before.
    let ratings: Vec<_> = v
        .audit(&a)
        .unwrap()
        .into_iter()
        .filter(|e| e.field == RATING)
        .collect();
    assert_eq!(ratings.len() as u64, rounds);
    for pair in ratings.windows(2) {
        assert_eq!(pair[1].old, pair[0].new);
    }
}

fn transactions_keep_all_or_nothing<S: Store>(s: S) {
    let mut s = with_images(s, 1);
    let a = image(1).id;
    s.begin().unwrap();
    assert!(s.begin().is_err(), "transactions do not nest");
    s.set_field(&a, "rating", Some(&json!(2))).unwrap();
    s.set_document("config", "user", Some(&json!(1))).unwrap();
    assert_eq!(
        s.field(&a, "rating").unwrap(),
        Some(json!(2)),
        "a transaction reads its own writes"
    );
    s.rollback().unwrap();
    assert_eq!(s.field(&a, "rating").unwrap(), None);
    assert_eq!(s.document("config", "user").unwrap(), None);
    s.begin().unwrap();
    s.set_field(&a, "rating", Some(&json!(3))).unwrap();
    s.commit().unwrap();
    assert_eq!(s.field(&a, "rating").unwrap(), Some(json!(3)));
    assert!(s.commit().is_err(), "nothing left to commit");
    assert!(s.rollback().is_err(), "nothing left to roll back");
}

/// A store whose audit writes fail: the fault lands between a field write
/// and its audit row.
struct FailingAudit<S>(S);

impl<S: Store> Store for FailingAudit<S> {
    fn add_image(&mut self, image: &Image) -> Result<bool, String> {
        self.0.add_image(image)
    }
    fn image(&self, id: &ImageId) -> Result<Option<Image>, String> {
        self.0.image(id)
    }
    fn images(&self) -> Result<Vec<Image>, String> {
        self.0.images()
    }
    fn put_location(&mut self, id: &ImageId, location: &Location) -> Result<(), String> {
        self.0.put_location(id, location)
    }
    fn locations(&self, id: &ImageId) -> Result<Vec<Location>, String> {
        self.0.locations(id)
    }
    fn at_path(&self, path: &Path) -> Result<Option<(ImageId, Location)>, String> {
        self.0.at_path(path)
    }
    fn all_locations(&self) -> Result<Vec<(ImageId, Location)>, String> {
        self.0.all_locations()
    }
    fn field(&self, id: &ImageId, name: &str) -> Result<Option<serde_json::Value>, String> {
        self.0.field(id, name)
    }
    fn set_field(
        &mut self,
        id: &ImageId,
        name: &str,
        value: Option<&serde_json::Value>,
    ) -> Result<(), String> {
        self.0.set_field(id, name, value)
    }
    fn fields_of(
        &self,
        ids: &[ImageId],
        names: &[&str],
    ) -> Result<Vec<std::collections::BTreeMap<String, serde_json::Value>>, String> {
        self.0.fields_of(ids, names)
    }
    fn select(&self, query: &Query) -> Result<Vec<ImageId>, String> {
        self.0.select(query)
    }
    fn add_audit(&mut self, _: &crate::AuditEntry) -> Result<u64, String> {
        Err("injected fault".into())
    }
    fn audit(&self, id: &ImageId) -> Result<Vec<crate::AuditEntry>, String> {
        self.0.audit(id)
    }
    fn put_version(&mut self, version: &crate::Version) -> Result<bool, String> {
        self.0.put_version(version)
    }
    fn version(&self, id: &crate::VersionId) -> Result<Option<crate::Version>, String> {
        self.0.version(id)
    }
    fn versions(&self, image: &ImageId) -> Result<Vec<crate::Version>, String> {
        self.0.versions(image)
    }
    fn document(&self, ns: &str, key: &str) -> Result<Option<serde_json::Value>, String> {
        self.0.document(ns, key)
    }
    fn set_document(
        &mut self,
        ns: &str,
        key: &str,
        value: Option<&serde_json::Value>,
    ) -> Result<(), String> {
        self.0.set_document(ns, key, value)
    }
    fn documents(&self, ns: &str) -> Result<Vec<(String, serde_json::Value)>, String> {
        self.0.documents(ns)
    }
    fn begin(&mut self) -> Result<(), String> {
        self.0.begin()
    }
    fn commit(&mut self) -> Result<(), String> {
        self.0.commit()
    }
    fn rollback(&mut self) -> Result<(), String> {
        self.0.rollback()
    }
}

fn a_batch_failing_at_its_audit_row_leaves_nothing<S: Store>(s: S) {
    let bus = Arc::new(Bus::new());
    let v = Vault::new(FailingAudit(with_images(s, 2))).with_bus(bus.clone());
    let (a, b) = (image(1).id, image(2).id);
    let set = |image, value| Command::set(image, "rating", value);
    let failed = v.apply(&[set(a, json!(3)), set(b, json!(4))], "user", None, 1);
    assert!(failed.unwrap_err().contains("injected fault"));
    let rows = v
        .store
        .lock()
        .unwrap()
        .fields_of(&[a, b], &["rating"])
        .unwrap();
    assert!(
        rows.iter().all(std::collections::BTreeMap::is_empty),
        "no field changed without its audit row: {rows:?}"
    );
    assert!(bus.since(0).is_empty(), "nothing announced");
    // The store is usable again: the failed transaction is closed.
    v.store.lock().unwrap().begin().unwrap();
    v.store.lock().unwrap().rollback().unwrap();
}

fn documents_round_trip_by_namespace<S: Store>(mut store: S) {
    assert_eq!(store.document("config", "user").unwrap(), None);
    store
        .set_document("config", "user", Some(&json!({ "toml": "a = 1" })))
        .unwrap();
    store
        .set_document("config", "user", Some(&json!({ "toml": "a = 2" })))
        .unwrap();
    store
        .set_document(
            "journal/w",
            "img\u{1f}fetch",
            Some(&json!({ "status": "done" })),
        )
        .unwrap();
    assert_eq!(
        store.document("config", "user").unwrap(),
        Some(json!({ "toml": "a = 2" }))
    );
    assert_eq!(store.documents("config").unwrap().len(), 1);
    assert_eq!(
        store.documents("journal/w").unwrap(),
        [("img\u{1f}fetch".to_string(), json!({ "status": "done" }))]
    );
    store.set_document("config", "user", None).unwrap();
    assert!(store.documents("config").unwrap().is_empty());
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
    assert_eq!(vault.heads(&img.id).unwrap(), std::slice::from_ref(&merge));

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
    s.set_field(&ids[2], "rejected", Some(&json!(true)))
        .unwrap();
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
        vec![ids[1], ids[0], ids[2], ids[3]],
        "unrated counts as 0, then by id"
    );
}

fn commands_skip_unchanged_values_coalesce_and_audit<S: Store>(s: S) {
    let bus = Arc::new(Bus::new());
    let v = Vault::new(with_images(s, 2)).with_bus(bus.clone());
    let (a, b) = (image(1).id, image(2).id);
    let set = |image, field: &str, value| Command::set(image, field, value);
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
            .contains("not a whole number in [0, 5]")
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
    let rate = |image, stars: i64| Command::set(image, "rating", json!(stars));
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
