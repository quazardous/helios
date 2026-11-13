//! The contract every [`Store`] must pass: the vault's rules and budgets
//! hold whatever keeps the rows.
//!
//! ```ignore
//! helios_vault::contract::check(|| helios_vault_sqlite::SqliteStore::in_memory().unwrap());
//! ```

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use std::sync::Arc;

use helios_core::{Bus, ContentHash, Describe};
use serde_json::json;

use crate::fields::{Command, LABELS, Op, Order, PICKED, Query, RATING, REJECTED, Sort, TAGS};
use crate::storage::VolumeId;
use crate::{Image, ImageId, Location, Presence, Store, ToRead, Vault};

/// Runs every scenario against fresh stores made by `make`.
pub fn check<S: Store>(make: impl Fn() -> S) {
    images_are_added_once(make());
    locations_belong_to_one_image_and_round_trip(make());
    import_reads_only_what_changed(make(), &scratch("import"));
    repair_relinks_by_content_reading_as_little_as_possible(make(), &scratch("repair"));
    repair_leaves_offline_locations_alone(make(), &scratch("offline"));
    fields_round_trip_and_come_in_one_batch(make());
    queries_filter_and_order(make());
    queries_filter_by_labels_flags_and_tags(make());
    queries_sort_by_every_order_in_one_query(make());
    tags_are_a_free_set(make());
    commands_skip_unchanged_values_coalesce_and_audit(make());
    cursors_cost_one_query_and_report_deltas(make());
    library_rows_come_with_their_locations_and_moves_show_in_deltas(make());
    versions_are_content_addressed_and_heads_follow_forks_and_merges(make());
    documents_round_trip_by_namespace(make());
    transactions_keep_all_or_nothing(make());
    a_batch_failing_at_its_audit_row_leaves_nothing(make());
    label_commands_keep_what_another_writer_added(make());
    taking_a_reject_back_keeps_the_stars(make());
    a_default_never_overwrites_a_choice(make());
    a_stale_writer_changes_only_its_own_fields(make());
    an_edit_is_saved_step_by_step_and_reopened_at_its_head(make());
    removals_forget_rows_and_refuse_what_is_still_named(make());
    a_step_is_reverted_and_replayed_keeping_the_files(make(), &scratch("undo"));
    an_image_off_the_library_never_shows_in_a_library_read(make());
    sightings_are_kept_one_per_place(make());
    listings_record_and_keep_what_was_read(make());
    the_map_reads_by_kind_newest_first(make());
    forgetting_and_pruning_never_touch_the_library(make());
    capture_fields_are_read_only_and_a_date_range_is_one_query(make());
    queries_filter_by_every_library_term_in_one_query(make());
    facets_count_each_value_in_one_query(make());
    folders_count_their_photos_and_those_below_in_one_query(make());
    albums_are_made_filled_renamed_and_forgotten(make());
    an_album_s_changes_are_undone_and_redone(make());
    smart_album_rules_select_in_one_query(make());
    relative_dates_follow_the_query_s_clock(make());
    smart_albums_scope_count_and_undo(make());
}

/// Capture fields are written whole by `set_captures` (a value not read
/// is cleared), refused to commands, read back as a [`Capture`], and a
/// capture range selects in the one query a cursor opens with; the
/// backfill's progress is kept in the same transaction, and images gone
/// meanwhile are passed over.
fn capture_fields_are_read_only_and_a_date_range_is_one_query<S: Store>(s: S) {
    use crate::capture::{self, Backfill, Capture, day_start};
    let v = Vault::new(with_images(s, 4));
    let ids: Vec<ImageId> = (1..=4).map(|i| image(i).id).collect();
    let june = |day: i64, hour: i64| {
        (day_start("2024-06-01").unwrap() + (day - 1) * 86_400 + hour * 3600) as f64
    };
    let shot = |captured: f64| Capture {
        captured: Some(captured),
        offset: Some(120),
        captured_from: Some("exif".into()),
        make: Some("Nikon".into()),
        model: Some("Z 8".into()),
        lens: Some("NIKKOR Z 24-120mm f/4 S".into()),
        focal_length: Some(35.0),
        aperture: Some(5.6),
        shutter: Some(1.0 / 250.0),
        iso: Some(400),
        width: Some(8256),
        height: Some(5504),
        orientation: Some(1),
        file_type: Some("nef".into()),
    };
    let gone = ImageId(ContentHash([99; 32]));
    let progress = Backfill {
        after: Some(ids[2].hex()),
        done: 3,
        total: 4,
        read: 2,
        kept: 1,
        finished: false,
    };
    let written = v
        .set_captures(
            &[
                (ids[0], shot(june(1, 23) + 0.25)),
                (ids[1], shot(june(2, 0))),
                (ids[2], shot(june(3, 12))),
                (gone, shot(june(1, 1))),
            ],
            Some(&progress),
        )
        .unwrap();
    assert_eq!(written, 3, "an image gone meanwhile is passed over");
    assert_eq!(v.backfill().unwrap(), Some(progress));
    let back = v.captures_of(&ids).unwrap();
    assert_eq!(back[0], Some(shot(june(1, 23) + 0.25)));
    assert_eq!(back[3], None, "never captured");
    assert_eq!(back[0].as_ref().unwrap().day(), Some((2024, 6, 1)));
    // Read-only: no command changes them, and nothing is audited.
    let refused = v
        .apply(
            &[Command::set(ids[0], capture::ISO, json!(800))],
            "user",
            None,
            1,
        )
        .unwrap_err();
    assert!(refused.contains("read from the file"), "{refused}");
    assert!(v.audit(&ids[0]).unwrap().is_empty());
    // Written again whole: what the file no longer says is cleared.
    let lens_only = Capture {
        lens: Some("manual lens".into()),
        ..Capture::default()
    };
    v.set_captures(&[(ids[2], lens_only.clone())], None)
        .unwrap();
    assert_eq!(v.captures_of(&ids[2..3]).unwrap()[0], Some(lens_only));
    v.set_captures(&[(ids[2], shot(june(3, 12)))], None)
        .unwrap();
    // The first of June, by the camera's clock: one query.
    let q0 = v.reads().queries;
    let first = v
        .open_cursor(Query {
            captured_from: day_start("2024-06-01"),
            captured_before: day_start("2024-06-02"),
            ..Query::default()
        })
        .unwrap();
    assert_eq!(
        v.reads().queries - q0,
        1,
        "a date range is the cursor's one query"
    );
    assert_eq!(v.cursor_ids(first.id, 0, 9).unwrap(), [ids[0]]);
    // Open ends, and a range with other criteria.
    let from_second = v
        .open_cursor(Query {
            captured_from: day_start("2024-06-02"),
            ..Query::default()
        })
        .unwrap();
    assert_eq!(
        v.cursor_ids(from_second.id, 0, 9).unwrap(),
        [ids[1], ids[2]]
    );
    v.apply(&[Command::set(ids[2], RATING, json!(4))], "user", None, 2)
        .unwrap();
    let rated_before_third = v
        .open_cursor(Query {
            captured_before: day_start("2024-06-04"),
            min_rating: Some(3),
            ..Query::default()
        })
        .unwrap();
    assert_eq!(v.cursor_ids(rated_before_third.id, 0, 9).unwrap(), [ids[2]]);
    // Captures move the catalog: a cursor's refresh lists them again.
    let delta_before = v.refresh(first.id).unwrap();
    assert!(delta_before.changed);
    v.set_captures(&[(ids[3], shot(june(1, 8)))], None).unwrap();
    let delta = v.refresh(first.id).unwrap();
    assert_eq!(delta.inserted, [ids[3]]);
}

fn on(volume: &str, rel: &str, size: u64, seen: u64) -> crate::Sighting {
    crate::Sighting {
        volume: Some(VolumeId(volume.into())),
        rel_path: Some(rel.into()),
        ..crate::Sighting::seen(PathBuf::from("/m").join(rel), size, 5, seen, "import")
    }
}

/// A listing records each file at its place; listed again unchanged it
/// keeps what was read of it, changed it is read again, and a library
/// file changed in place keeps its location for the map job.
fn listings_record_and_keep_what_was_read<S: Store>(s: S) {
    let v = Vault::new(s);
    let a = image(1);
    v.transaction(|s| s.add_to_library(&a).map(|_| ())).unwrap();
    let mut first = on("uuid:d", "shoot/x.cr3", 101, 10);
    first.partial = Some(a.partial);
    first.camera = Some("Z 8".into());
    let lib = crate::Sighting {
        image: Some(a.id),
        ..on("uuid:d", "lib/y.cr3", 101, 10)
    };
    assert_eq!(v.record_listed(&[first.clone(), lib.clone()]).unwrap(), 2);
    let d = VolumeId("uuid:d".into());
    // Unchanged: the new listing read nothing, the old reading stays.
    let again = crate::Sighting {
        seen_by: "dialog".into(),
        ..on("uuid:d", "shoot/x.cr3", 101, 20)
    };
    v.record_listed(std::slice::from_ref(&again)).unwrap();
    let got = v
        .sighting_at(Some(&d), Some(Path::new("shoot/x.cr3")), Path::new("/x"))
        .unwrap()
        .unwrap();
    assert_eq!(got.partial, Some(a.partial));
    assert_eq!(got.camera.as_deref(), Some("Z 8"));
    assert_eq!((got.first_seen, got.last_seen), (10, 20));
    assert_eq!(got.seen_by, "import", "what listed it first");
    // Changed in place: read again.
    let changed = crate::Sighting {
        modified: 6,
        ..on("uuid:d", "shoot/x.cr3", 101, 30)
    };
    v.record_listed(&[changed]).unwrap();
    let got = v
        .sighting_at(Some(&d), Some(Path::new("shoot/x.cr3")), Path::new("/x"))
        .unwrap()
        .unwrap();
    assert_eq!((got.partial, got.camera), (None, None));
    // A library file changed in place is left to the map job.
    let lib_changed = crate::Sighting {
        size: 999,
        ..on("uuid:d", "lib/y.cr3", 999, 30)
    };
    assert_eq!(v.record_listed(&[lib_changed]).unwrap(), 0);
    assert_eq!(v.sightings(&a.id).unwrap().len(), 1);
    // A folder and below.
    assert_eq!(
        v.sightings_under(Some(&d), Path::new("shoot"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(v.sightings_under(Some(&d), Path::new("")).unwrap().len(), 2);
    assert_eq!(
        v.sightings_under(Some(&d), Path::new("sho")).unwrap().len(),
        0
    );
    let loose = crate::Sighting::seen(PathBuf::from("/tmp/p/q.jpg"), 3, 4, 5, "drop");
    v.record_listed(&[loose]).unwrap();
    assert_eq!(
        v.sightings_under(None, Path::new("/tmp/p")).unwrap().len(),
        1
    );
    assert_eq!(
        v.sightings_under(None, Path::new("/tmp/pp")).unwrap().len(),
        0
    );
}

/// What the map job reads: files with no partial hash, newest first;
/// library files, verified longest ago first; duplicates to confirm.
fn the_map_reads_by_kind_newest_first<S: Store>(mut s: S) {
    let (a, b) = (image(1), image(2));
    s.add_to_library(&a).unwrap();
    s.add_to_library(&b).unwrap();
    s.set_library_flag(&b.id, &crate::LibraryFlag::default())
        .unwrap();
    let old = on("uuid:d", "a.jpg", 7, 10);
    let new = on("uuid:e", "b.jpg", 7, 20);
    let mut gone = on("uuid:d", "c.jpg", 7, 30);
    gone.presence = Presence::Missing;
    let mut dup = on("uuid:d", "dup.cr3", a.size, 5);
    dup.partial = Some(a.partial);
    let mut lib1 = on("uuid:d", "l1.cr3", a.size, 5);
    lib1.partial = Some(a.partial);
    lib1.image = Some(a.id);
    lib1.last_verified = Some(50);
    let mut lib2 = on("uuid:d", "l2.cr3", a.size, 5);
    lib2.partial = Some(a.partial);
    lib2.image = Some(a.id);
    lib2.last_verified = Some(40);
    let mut out = on("uuid:d", "out.cr3", b.size, 5);
    out.partial = Some(b.partial);
    out.image = Some(b.id);
    for x in [&old, &new, &gone, &dup, &lib1, &lib2, &out] {
        s.put_sighting(x).unwrap();
    }
    let paths =
        |v: Vec<crate::Sighting>| -> Vec<PathBuf> { v.into_iter().map(|s| s.path).collect() };
    assert_eq!(
        paths(s.sightings_to_read(ToRead::Partial, None, 10).unwrap()),
        [new.path.clone(), old.path.clone()]
    );
    assert_eq!(
        paths(s.sightings_to_read(ToRead::Partial, None, 1).unwrap()),
        std::slice::from_ref(&new.path)
    );
    let d = VolumeId("uuid:d".into());
    assert_eq!(
        paths(s.sightings_to_read(ToRead::Partial, Some(&d), 10).unwrap()),
        std::slice::from_ref(&old.path)
    );
    assert_eq!(
        paths(s.sightings_to_read(ToRead::Library, None, 10).unwrap()),
        [lib2.path.clone(), lib1.path.clone()]
    );
    assert_eq!(
        paths(s.sightings_to_read(ToRead::Confirm, None, 10).unwrap()),
        [dup.path.clone()]
    );
}

/// Forgetting a folder or a volume drops what is not in the Library and
/// marks the folder; pruning drops long-missing files, then the oldest
/// past the cap; library sightings stay through both.
fn forgetting_and_pruning_never_touch_the_library<S: Store>(s: S) {
    let v = Vault::new(s);
    let a = image(1);
    v.transaction(|s| s.add_to_library(&a).map(|_| ())).unwrap();
    let mut lib = on("uuid:d", "shoot/l.cr3", a.size, 1);
    lib.image = Some(a.id);
    lib.partial = Some(a.partial);
    let mut rows = vec![lib.clone()];
    for i in 0..4 {
        rows.push(on("uuid:d", &format!("shoot/{i}.jpg"), 7, 10 + i));
    }
    rows.push(on("uuid:d", "other/z.jpg", 7, 1));
    rows.push(on("uuid:e", "shoot/e.jpg", 7, 1));
    v.record_listed(&rows).unwrap();
    let d = VolumeId("uuid:d".into());
    assert_eq!(v.forget(Some(&d), Some(Path::new("shoot")), 99).unwrap(), 4);
    let folder = v.folder(Some(&d), Path::new("shoot")).unwrap().unwrap();
    assert_eq!(folder.state, crate::FolderState::Forgotten);
    assert!(v.forgotten(Some(&d), Path::new("shoot/new/n.jpg")).unwrap());
    assert!(!v.forgotten(Some(&d), Path::new("other/n.jpg")).unwrap());
    assert_eq!(v.sightings(&a.id).unwrap().len(), 1, "the library stays");
    // Listed again on purpose: no longer forgotten.
    v.folder_listed(Some(&d), Path::new("shoot"), 3, 100)
        .unwrap();
    assert!(!v.forgotten(Some(&d), Path::new("shoot/n.jpg")).unwrap());
    // A whole volume.
    assert_eq!(v.forget(Some(&d), None, 99).unwrap(), 1);
    assert_eq!(v.sightings_under(Some(&d), Path::new("")).unwrap().len(), 1);
    // Pruning: missing for long, then the cap, oldest first.
    let e = VolumeId("uuid:e".into());
    let mut rows = Vec::new();
    for i in 0..5 {
        let mut s = on("uuid:e", &format!("p/{i}.jpg"), 7, 100 + i);
        if i == 4 {
            s.presence = Presence::Missing;
            s.last_seen = 50;
        }
        rows.push(s);
    }
    v.record_listed(&rows).unwrap();
    // Six outside the Library, all on e (one missing since 50): the
    // missing one goes, then the cap of 3 drops the two seen longest ago
    // (shoot/e.jpg, then p/0.jpg).
    assert_eq!(v.prune_sightings(3, 60).unwrap(), 3);
    let left: Vec<PathBuf> = v
        .sightings_under(Some(&e), Path::new(""))
        .unwrap()
        .into_iter()
        .filter_map(|s| s.rel_path)
        .collect();
    assert_eq!(
        left,
        [
            PathBuf::from("p/1.jpg"),
            PathBuf::from("p/2.jpg"),
            PathBuf::from("p/3.jpg")
        ],
        "{left:?}"
    );
    assert_eq!(v.sightings(&a.id).unwrap().len(), 1, "the library stays");
}

/// The Library is the flagged images: one taken out of it is in no
/// listing, query, cursor, stack or location list, yet it keeps its rows
/// and its sightings, and putting it back is an undoable step.
fn an_image_off_the_library_never_shows_in_a_library_read<S: Store>(s: S) {
    let v = Vault::new(s);
    let (a, b, c) = (image(1), image(2), image(3));
    v.transaction(|s| {
        for (i, img) in [&a, &b, &c].into_iter().enumerate() {
            assert!(s.add_to_library(img)?);
            s.put_location(&img.id, &location(&format!("/p/{i}.nef")))?;
            s.set_field(&img.id, RATING, Some(&json!(5)))?;
            s.set_field(&img.id, LABELS, Some(&json!(["red"])))?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(
        v.library_flag(&a.id).unwrap(),
        Some(crate::LibraryFlag::added()),
        "an image added is in the Library"
    );
    v.create_stack(&[a.id, b.id, c.id], Some(b.id), "user", 1)
        .unwrap();
    let cursor = v.open_cursor(Query::default()).unwrap();
    assert_eq!(cursor.count, 3);
    // Taken out.
    let out = v.set_in_library(&[b.id, b.id], false, "user", 9).unwrap();
    assert_eq!(out.images.len(), 1, "one image, once");
    assert!(out.images[0].changed());
    assert!(!v.in_library(&b.id).unwrap());
    let flag = v.library_flag(&b.id).unwrap().unwrap();
    assert_eq!((flag.at, flag.by.as_deref()), (Some(9), Some("user")));
    let ids = |images: Vec<Image>| images.into_iter().map(|i| i.id).collect::<Vec<_>>();
    assert_eq!(ids(v.images().unwrap()), [a.id, c.id], "images");
    let queries = [
        Query::default(),
        Query {
            min_rating: Some(4),
            ..Query::default()
        },
        Query {
            label: Some("red".into()),
            order: Order::desc(Sort::Rating),
            ..Query::default()
        },
        Query {
            labels: vec!["red".into()],
            flags: vec!["none".into()],
            ..Query::default()
        },
    ];
    for q in &queries {
        assert_eq!(
            v.store.lock().unwrap().select(q).unwrap(),
            [a.id, c.id],
            "{q:?}"
        );
    }
    let delta = v.refresh(cursor.id).unwrap();
    assert_eq!(
        (delta.count, delta.removed.as_slice()),
        (2, [b.id].as_slice())
    );
    assert_eq!(v.rank_of(cursor.id, &b.id).unwrap(), None);
    let fetched: Vec<ImageId> = v
        .fetch(cursor.id, 0, 10, &[RATING])
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(fetched, [a.id, c.id], "cursor rows");
    let located: Vec<ImageId> = v
        .all_locations()
        .unwrap()
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    assert_eq!(located, [a.id, c.id], "every location");
    let stacks = v.stacks().unwrap();
    assert_eq!(stacks.len(), 1);
    assert_eq!(stacks[0].members, [a.id.hex(), c.id.hex()], "stack members");
    assert_eq!(stacks[0].top, a.id.hex(), "a top taken out moves");
    assert_eq!(v.stack(&stacks[0].id).unwrap(), Some(stacks[0].clone()));
    assert_eq!(v.stack_of(&b.id).unwrap(), None);
    assert_eq!(v.describe()["images"], 2);
    // Repair walks the Library: the files are gone, only its locations
    // are marked missing.
    assert_eq!(v.repair(&[], 10).unwrap().lost, 2);
    assert_eq!(v.locations(&b.id).unwrap()[0].presence, Presence::Present);
    // Still known by its id, with its rows and its sightings.
    assert_eq!(v.image(&b.id).unwrap(), Some(b.clone()));
    assert_eq!(v.locations(&b.id).unwrap().len(), 1);
    assert_eq!(
        v.fields_of(&[b.id], &[RATING]).unwrap()[0][RATING],
        json!(5)
    );
    let seen = v.sightings(&b.id).unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        (seen[0].image, seen[0].partial),
        (Some(b.id), Some(b.partial))
    );
    assert_eq!(v.sightings_like(seen[0].size, &b.partial).unwrap(), seen);
    assert!(v.at_path(Path::new("/p/1.nef")).unwrap().is_some());
    // A third image out leaves no stack of two.
    v.set_in_library(&[c.id], false, "user", 10).unwrap();
    assert!(v.stacks().unwrap().is_empty());
    assert_eq!(v.stack_of(&a.id).unwrap(), None);
    // Undo puts the flag back; redo takes it out again.
    v.revert(&out).unwrap();
    assert!(v.in_library(&b.id).unwrap());
    assert_eq!(ids(v.images().unwrap()), [a.id, b.id]);
    assert_eq!(v.refresh(cursor.id).unwrap().inserted, [b.id]);
    v.replay(&out).unwrap();
    assert!(!v.in_library(&b.id).unwrap());
    assert!(
        v.set_in_library(&[image(9).id], true, "user", 11)
            .unwrap_err()
            .contains("no image")
    );
    // Back in by an import's job: listed again.
    let back = v
        .set_in_library(&[b.id, c.id], true, "import-1", 12)
        .unwrap();
    assert_eq!(back.images.len(), 2);
    assert_eq!(ids(v.images().unwrap()), [a.id, b.id, c.id]);
}

/// A sighting is one file at one place: recorded again it is replaced,
/// keeping when it was first seen; a location is a sighting with an image.
fn sightings_are_kept_one_per_place<S: Store>(mut s: S) {
    let a = image(1);
    s.add_to_library(&a).unwrap();
    let mut loose = crate::Sighting::seen(PathBuf::from("/card/x.cr3"), 101, 5, 20, "dialog");
    loose.partial = Some(a.partial);
    loose.camera = Some("EOS R5".into());
    s.put_sighting(&loose).unwrap();
    assert_eq!(
        s.at_path(Path::new("/card/x.cr3")).unwrap(),
        None,
        "no image yet"
    );
    assert_eq!(s.sightings_like(101, &a.partial).unwrap(), [loose.clone()]);
    assert!(s.sightings_like(102, &a.partial).unwrap().is_empty());
    assert!(s.sightings_of(&a.id).unwrap().is_empty());
    // Seen again later: one row, first seen kept.
    let again = crate::Sighting {
        first_seen: 30,
        last_seen: 30,
        image: Some(a.id),
        ..loose.clone()
    };
    s.put_sighting(&again).unwrap();
    let kept = s.sightings_like(101, &a.partial).unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!((kept[0].first_seen, kept[0].last_seen), (20, 30));
    assert_eq!(s.sightings_of(&a.id).unwrap(), kept);
    // On a volume, the place is the volume and the path there.
    let on = crate::Sighting {
        volume: Some(VolumeId("uuid:card".into())),
        rel_path: Some("DCIM/x.cr3".into()),
        path: PathBuf::from("/media/a/DCIM/x.cr3"),
        ..again.clone()
    };
    s.put_sighting(&on).unwrap();
    let moved = crate::Sighting {
        path: PathBuf::from("/media/b/DCIM/x.cr3"),
        first_seen: 40,
        ..on.clone()
    };
    s.put_sighting(&moved).unwrap();
    let all = s.sightings_of(&a.id).unwrap();
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[1].path, PathBuf::from("/media/b/DCIM/x.cr3"));
    assert_eq!(all[1].first_seen, 30, "the earliest kept");
    assert!(
        s.put_sighting(&crate::Sighting {
            image: Some(image(9).id),
            ..loose.clone()
        })
        .unwrap_err()
        .contains("no image")
    );
    assert!(
        s.put_sighting(&crate::Sighting {
            rel_path: None,
            ..on.clone()
        })
        .is_err(),
        "a volume needs its path there"
    );
    // A location at a loose sighting's path takes it over, keeping its
    // glance and first sighting; removing it forgets it.
    let b = image(2);
    s.add_to_library(&b).unwrap();
    let mut file = crate::Sighting::seen(PathBuf::from("/in/y.nef"), 102, 5, 3, "drop");
    file.camera = Some("Z8".into());
    s.put_sighting(&file).unwrap();
    s.put_location(
        &b.id,
        &Location::seen(PathBuf::from("/in/y.nef"), 102, 5, 8),
    )
    .unwrap();
    let as_location = s.sightings_of(&b.id).unwrap();
    assert_eq!(as_location.len(), 1);
    assert_eq!(
        (
            as_location[0].first_seen,
            as_location[0].last_verified,
            as_location[0].partial,
            as_location[0].seen_by.as_str()
        ),
        (3, Some(8), Some(b.partial), "drop")
    );
    assert!(s.remove_location(Path::new("/in/y.nef")).unwrap());
    assert!(s.sightings_of(&b.id).unwrap().is_empty());
    assert!(s.remove_image(&b.id).unwrap());
    assert!(
        s.remove_image(&a.id).unwrap_err().contains("still has"),
        "a sighting names it"
    );
}

fn removals_forget_rows_and_refuse_what_is_still_named<S: Store>(mut s: S) {
    let a = image(1);
    s.add_to_library(&a).unwrap();
    s.put_location(&a.id, &location("/a.nef")).unwrap();
    s.set_field(&a.id, "rating", Some(&json!(3))).unwrap();
    let base = crate::Version::new(a.id, vec![], json!({ "n": 1 }), "user", 1);
    let child = crate::Version::new(a.id, vec![base.id], json!({ "n": 2 }), "user", 2);
    s.put_version(&base).unwrap();
    s.put_version(&child).unwrap();
    assert!(s.remove_image(&a.id).unwrap_err().contains("still has"));
    assert!(s.remove_version(&base.id).unwrap_err().contains("parent"));
    assert!(s.remove_version(&child.id).unwrap());
    assert!(s.remove_version(&base.id).unwrap());
    assert!(!s.remove_version(&base.id).unwrap(), "removed twice");
    assert!(s.remove_location(Path::new("/a.nef")).unwrap());
    assert!(!s.remove_location(Path::new("/a.nef")).unwrap());
    assert!(s.remove_image(&a.id).unwrap());
    assert!(!s.remove_image(&a.id).unwrap());
    assert_eq!(s.image(&a.id).unwrap(), None);
    // Its fields went with it: added again, it holds none.
    s.add_to_library(&a).unwrap();
    assert_eq!(s.field(&a.id, "rating").unwrap(), None);
}

fn a_step_is_reverted_and_replayed_keeping_the_files<S: Store>(s: S, dir: &Path) {
    use crate::{Change, Changes};
    let v = Vault::new(s);
    // One image known before the step, and one file it brings.
    let (old_file, new_file) = (dir.join("old.nef"), dir.join("new.nef"));
    write(&old_file, &content(1, 300));
    write(&new_file, &content(2, 300));
    let old = v.import_file(&old_file, 1).unwrap();
    v.apply(&[Command::set(old, RATING, json!(2))], "user", None, 1)
        .unwrap();
    let old_before = v.rows(&old).unwrap();
    // The step: a new image with its signals, a field and a version; the
    // old one found at a second place, rated by an importer.
    let new = v.import_file(&new_file, 2).unwrap();
    v.set_signals(&new, Some(&json!({ "w": 10 }))).unwrap();
    v.apply(&[Command::set(new, RATING, json!(5))], "importer", None, 2)
        .unwrap();
    let kept = crate::Version::new(new, vec![], json!({ "dt": 1 }), "import:xmp", 2);
    v.add_version(&kept).unwrap();
    let copy = dir.join("copy-of-old.nef");
    std::fs::copy(&old_file, &copy).unwrap();
    v.import_hashed(&copy, old, 2).unwrap();
    v.apply(&[Command::set(old, RATING, json!(4))], "importer", None, 2)
        .unwrap();
    let changes = Changes {
        images: vec![
            Change {
                image: old,
                before: old_before.clone(),
                after: v.rows(&old).unwrap(),
            },
            Change {
                image: new,
                before: None,
                after: v.rows(&new).unwrap(),
            },
        ],
        ..Changes::default()
    };
    assert!(changes.images.iter().all(Change::changed));
    let after: Vec<_> = changes.images.iter().map(|c| c.after.clone()).collect();

    let undone = v.revert(&changes).unwrap();
    assert_eq!((undone.removed, undone.changed, undone.kept), (1, 1, 0));
    assert_eq!(v.image(&new).unwrap(), None, "the new image is forgotten");
    assert_eq!(v.signals(&new).unwrap(), None);
    assert!(v.versions(&new).unwrap().is_empty());
    assert_eq!(v.rows(&old).unwrap(), old_before, "the old image as it was");
    for f in [&old_file, &new_file, &copy] {
        assert!(f.is_file(), "{} stays on disk", f.display());
    }

    let redone = v.replay(&changes).unwrap();
    assert_eq!((redone.restored, redone.changed), (1, 1));
    let now: Vec<_> = [old, new].iter().map(|i| v.rows(i).unwrap()).collect();
    assert_eq!(now, after, "redo puts every row back");

    // Changed since: a rating given after the redo stays, and an image
    // edited since is kept by the next undo.
    v.apply(&[Command::set(old, RATING, json!(1))], "user", None, 3)
        .unwrap();
    let edit = crate::Version::new(new, vec![kept.id], json!({ "dt": 2 }), "user", 3);
    v.add_version(&edit).unwrap();
    let again = v.revert(&changes).unwrap();
    assert_eq!(again.kept, 1, "{again:?}");
    assert!(v.image(&new).unwrap().is_some(), "an edited image stays");
    let old_now = v.rows(&old).unwrap().unwrap();
    assert_eq!(old_now.fields[RATING], json!(1), "a later rating stays");
    assert_eq!(old_now.locations.len(), 1, "the second place is forgotten");
    for f in [&old_file, &new_file, &copy] {
        assert!(f.is_file(), "{} stays on disk", f.display());
    }
}

fn an_edit_is_saved_step_by_step_and_reopened_at_its_head<S: Store>(s: S) {
    use crate::EditSession;
    let bus = Arc::new(Bus::new());
    let v = Vault::new(with_images(s, 1)).with_bus(bus.clone());
    let a = image(1).id;
    let edit = |ev: f64| {
        json!({ "modules": [{ "name": "exp", "module": "exposure",
        "params": { "exposure": { "float": ev } } }] })
    };
    let (mut session, head) = EditSession::open(&v, a).unwrap();
    assert!(head.is_none(), "never edited");
    let first = session
        .save(&v, edit(1.0), "set exposure", "user", 1)
        .unwrap();
    assert!(first.is_some());
    assert!(
        session
            .save(&v, edit(1.0), "set exposure", "user", 2)
            .unwrap()
            .is_none(),
        "a step that changed nothing is not saved"
    );
    // A drag saved while it goes, then at its end with the same values:
    // the end is saved too, so the history names the finished gesture.
    session
        .save_in_progress(&v, edit(1.5), "drag exposure", "user", 3)
        .unwrap()
        .unwrap();
    let end = session
        .save(&v, edit(1.5), "drag exposure", "user", 4)
        .unwrap()
        .unwrap();
    assert_eq!(session.steps(), 3);
    let saved: Vec<_> = bus
        .since(0)
        .into_iter()
        .filter(|e| e.kind == "edit.saved")
        .map(|e| {
            (
                e.payload["step"].as_u64().unwrap(),
                e.payload["label"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        saved,
        [
            (1, "set exposure".to_string()),
            (2, "drag exposure (in progress)".to_string()),
            (3, "drag exposure".to_string()),
        ]
    );
    // Reopened: the head is the last step, its history the chain.
    let (mut again, head) = EditSession::open(&v, a).unwrap();
    let head = head.unwrap();
    assert_eq!(head.id, end.id);
    assert_eq!(head.edit, edit(1.5));
    let versions = v.versions(&a).unwrap();
    assert_eq!(versions.len(), 3);
    assert_eq!(v.heads(&a).unwrap().len(), 1, "one line of history");
    // Two sessions continuing the same head make two heads; the newest
    // is the one opened next.
    let (mut other, _) = EditSession::open(&v, a).unwrap();
    other
        .save(&v, edit(2.0), "set exposure", "agent", 10)
        .unwrap();
    again
        .save(&v, edit(0.5), "set exposure", "user", 11)
        .unwrap();
    assert_eq!(v.heads(&a).unwrap().len(), 2, "neither overwrote the other");
    let (_, newest) = EditSession::open(&v, a).unwrap();
    assert_eq!(newest.unwrap().edit, edit(0.5));
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
    fn library_flag(&self, id: &ImageId) -> Result<Option<crate::LibraryFlag>, String> {
        self.0.library_flag(id)
    }
    fn set_library_flag(&mut self, id: &ImageId, flag: &crate::LibraryFlag) -> Result<(), String> {
        self.0.set_library_flag(id, flag)
    }
    fn put_sighting(&mut self, sighting: &crate::Sighting) -> Result<(), String> {
        self.0.put_sighting(sighting)
    }
    fn sightings_of(&self, id: &ImageId) -> Result<Vec<crate::Sighting>, String> {
        self.0.sightings_of(id)
    }
    fn sightings_like(
        &self,
        size: u64,
        partial: &ContentHash,
    ) -> Result<Vec<crate::Sighting>, String> {
        self.0.sightings_like(size, partial)
    }
    fn sighting_at(
        &self,
        volume: Option<&crate::storage::VolumeId>,
        rel_path: Option<&Path>,
        path: &Path,
    ) -> Result<Option<crate::Sighting>, String> {
        self.0.sighting_at(volume, rel_path, path)
    }
    fn sightings_under(
        &self,
        volume: Option<&crate::storage::VolumeId>,
        rel_dir: &Path,
    ) -> Result<Vec<crate::Sighting>, String> {
        self.0.sightings_under(volume, rel_dir)
    }
    fn sightings_to_read(
        &self,
        read: crate::ToRead,
        volume: Option<&crate::storage::VolumeId>,
        limit: usize,
    ) -> Result<Vec<crate::Sighting>, String> {
        self.0.sightings_to_read(read, volume, limit)
    }
    fn forget_sightings(
        &mut self,
        volume: Option<&crate::storage::VolumeId>,
        rel_dir: Option<&Path>,
    ) -> Result<usize, String> {
        self.0.forget_sightings(volume, rel_dir)
    }
    fn prune_sightings(&mut self, limit: usize, missing_before: u64) -> Result<usize, String> {
        self.0.prune_sightings(limit, missing_before)
    }
    fn put_folder(&mut self, folder: &crate::Folder) -> Result<(), String> {
        self.0.put_folder(folder)
    }
    fn folder(
        &self,
        volume: Option<&crate::storage::VolumeId>,
        rel_dir: &Path,
    ) -> Result<Option<crate::Folder>, String> {
        self.0.folder(volume, rel_dir)
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
    fn remove_location(&mut self, path: &Path) -> Result<bool, String> {
        self.0.remove_location(path)
    }
    fn remove_image(&mut self, id: &ImageId) -> Result<bool, String> {
        self.0.remove_image(id)
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
    fn facets(
        &self,
        query: &Query,
        backups: Option<&crate::Backups>,
    ) -> Result<crate::Facets, String> {
        self.0.facets(query, backups)
    }
    fn folders(&self, query: &Query) -> Result<crate::VolumeFolders, String> {
        self.0.folders(query)
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
    fn remove_version(&mut self, id: &crate::VersionId) -> Result<bool, String> {
        self.0.remove_version(id)
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
        vault.store.lock().unwrap().add_to_library(&img).unwrap();
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
    vault.store.lock().unwrap().add_to_library(&other).unwrap();
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
    Location::seen(PathBuf::from(path), 101, 5, 7)
}

fn images_are_added_once<S: Store>(mut s: S) {
    assert!(s.add_image(&image(1)).unwrap());
    assert!(!s.add_image(&image(1)).unwrap(), "added twice");
    s.add_image(&image(2)).unwrap();
    assert_eq!(s.image(&image(1).id).unwrap(), Some(image(1)));
    assert_eq!(s.image(&image(9).id).unwrap(), None);
    // Recorded, not imported: off the Library until flagged.
    assert!(s.images().unwrap().is_empty());
    assert!(!s.library_flag(&image(1).id).unwrap().unwrap().in_library);
    assert!(s.add_to_library(&image(1)).unwrap());
    assert!(!s.add_to_library(&image(1)).unwrap(), "flagged twice");
    s.add_to_library(&image(2)).unwrap();
    assert_eq!(s.images().unwrap().len(), 2);
}

fn locations_belong_to_one_image_and_round_trip<S: Store>(mut s: S) {
    assert!(
        s.put_location(&image(1).id, &location("/a.nef"))
            .unwrap_err()
            .contains("no image")
    );
    s.add_to_library(&image(1)).unwrap();
    s.add_to_library(&image(2)).unwrap();
    let lost = Location {
        presence: Presence::Missing,
        ..location("/b.nef")
    };
    // On a volume, unplugged: its volume and path there round-trip too.
    let away = Location {
        volume: Some(VolumeId("uuid:backup".into())),
        rel_path: Some(PathBuf::from("2024/c.nef")),
        presence: Presence::Offline,
        ..location("/media/backup/2024/c.nef")
    };
    s.put_location(&image(1).id, &location("/a.nef")).unwrap();
    s.put_location(&image(1).id, &lost).unwrap();
    s.put_location(&image(1).id, &away).unwrap();
    assert_eq!(
        s.locations(&image(1).id).unwrap(),
        vec![location("/a.nef"), lost.clone(), away.clone()]
    );
    assert_eq!(
        s.at_path(Path::new("/media/backup/2024/c.nef")).unwrap(),
        Some((image(1).id, away))
    );
    assert!(
        s.remove_location(Path::new("/media/backup/2024/c.nef"))
            .unwrap()
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

/// A copy on an unplugged disk is offline: repair neither marks it
/// missing nor looks for its image elsewhere, and the scan alone brings
/// it back.
fn repair_leaves_offline_locations_alone<S: Store>(s: S, dir: &Path) {
    let v = Vault::new(s);
    write(&dir.join("disk/away.raf"), &content(4, 5_000));
    v.import(&dir.join("disk"), 10).unwrap();
    let (id, at) = v.all_locations().unwrap().remove(0);
    let offline = Location {
        volume: Some(VolumeId("uuid:backup".into())),
        rel_path: Some(PathBuf::from("away.raf")),
        presence: Presence::Offline,
        ..at.clone()
    };
    v.update_locations(&[(id, at.path.clone(), offline.clone())])
        .unwrap();
    // The disk is unplugged: its file is not there.
    std::fs::remove_dir_all(dir.join("disk")).unwrap();
    let before = v.reads();
    let report = v.repair(&[dir], 20).unwrap();
    assert_eq!((report.lost, report.relinked, report.missing), (0, 0, 0));
    assert_eq!(v.reads(), before, "nothing is looked for");
    assert_eq!(v.locations(&id).unwrap(), vec![offline]);
    let _ = std::fs::remove_dir_all(dir);
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
        .filter(Location::present)
        .map(|l| l.path)
        .collect();
    assert_eq!(present, vec![dir.join("elsewhere.raf")]);
    let _ = std::fs::remove_dir_all(dir);
}

fn with_images<S: Store>(mut s: S, n: u8) -> S {
    for i in 1..=n {
        s.add_to_library(&image(i)).unwrap();
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
        order: Order::desc(Sort::Rating),
        ..Query::default()
    };
    assert_eq!(
        s.select(&by_rating).unwrap(),
        vec![ids[1], ids[0], ids[2], ids[3]],
        "unrated counts as 0, then by id"
    );
}

/// Every sort, both ways, in the one query a cursor opens with: the
/// photos without a value last whatever the direction, ties broken the
/// same way (by file name for the capture time, then by id).
fn queries_sort_by_every_order_in_one_query<S: Store>(s: S) {
    let mut s = with_images(s, 5);
    let id = |n: u8| image(n).id;
    let root = Path::new(std::path::MAIN_SEPARATOR_STR);
    let file = |dir: &str, name: &str| Location::seen(root.join(dir).join(name), 101, 5, 7);
    // Names: 1 `e`, 2 `b`, 3 `a`, 4 none (no copy known), 5 `d` (a
    // missing copy at a path before its present one is passed over).
    s.put_location(&id(1), &file("p", "e.nef")).unwrap();
    s.put_location(&id(2), &file("p", "b.nef")).unwrap();
    s.put_location(&id(3), &file("p", "a.nef")).unwrap();
    s.put_location(
        &id(5),
        &Location {
            presence: Presence::Missing,
            ..file("a", "x.nef")
        },
    )
    .unwrap();
    s.put_location(&id(5), &file("q", "d.nef")).unwrap();
    // Taken: 2 and 3 in the same second, 4 never by a camera.
    for (n, at) in [(1, 300), (2, 100), (3, 100), (5, 200)] {
        s.set_field(&id(n), crate::capture::CAPTURED, Some(&json!(at)))
            .unwrap();
    }
    // Imported: 3 at no known time, 4 and 5 at the same.
    for (n, at) in [
        (1, Some(50)),
        (2, Some(10)),
        (3, None),
        (4, Some(30)),
        (5, Some(30)),
    ] {
        let flag = crate::LibraryFlag {
            in_library: true,
            at,
            by: Some("import".into()),
        };
        s.set_library_flag(&id(n), &flag).unwrap();
    }
    // Stars: 3 unrated, 5 at none (both 0), 1 and 4 the same.
    for (n, stars) in [(1, 2), (2, 5), (4, 2), (5, 0)] {
        s.set_field(&id(n), RATING, Some(&json!(stars))).unwrap();
    }
    // Edited: 2 last (its newest version), 3 and 4 never.
    for (n, at) in [(1, 7), (2, 3), (2, 9), (5, 1)] {
        let version = crate::Version::new(id(n), vec![], json!({ "at": at }), "user", at);
        s.put_version(&version).unwrap();
    }
    let v = Vault::new(s);
    let cases: [(Sort, [u8; 5], [u8; 5]); 7] = [
        (Sort::Captured, [3, 2, 5, 1, 4], [1, 5, 3, 2, 4]),
        (Sort::Imported, [2, 4, 5, 1, 3], [1, 4, 5, 2, 3]),
        (Sort::Name, [3, 2, 5, 1, 4], [1, 5, 2, 3, 4]),
        (Sort::Rating, [3, 5, 1, 4, 2], [2, 1, 4, 3, 5]),
        (Sort::Edited, [5, 1, 2, 3, 4], [2, 1, 5, 3, 4]),
        (Sort::Size, [1, 2, 3, 4, 5], [5, 4, 3, 2, 1]),
        (Sort::Id, [1, 2, 3, 4, 5], [5, 4, 3, 2, 1]),
    ];
    for (by, ascending, descending) in cases {
        assert_eq!(Sort::parse(by.name()), Some(by));
        for (order, want) in [(Order::asc(by), ascending), (Order::desc(by), descending)] {
            let q0 = v.reads().queries;
            let cursor = v
                .open_cursor(Query {
                    order,
                    ..Query::default()
                })
                .unwrap();
            assert_eq!(v.reads().queries - q0, 1, "{order:?}: one query");
            let got = v.cursor_ids(cursor.id, 0, 10).unwrap();
            let want: Vec<ImageId> = want.into_iter().map(id).collect();
            assert_eq!(got, want, "{order:?}");
            v.close_cursor(cursor.id);
        }
    }
    // A filter keeps the order.
    let rated = Query {
        min_rating: Some(1),
        order: Order::asc(Sort::Rating),
        ..Query::default()
    };
    let got = v.store.lock().unwrap().select(&rated).unwrap();
    assert_eq!(got, [id(1), id(4), id(2)]);
    assert_eq!(Order::default(), Order::ID);
}

fn queries_filter_by_labels_flags_and_tags<S: Store>(s: S) {
    let mut s = with_images(s, 5);
    let ids: Vec<ImageId> = (1..=5).map(|i| image(i).id).collect();
    s.set_field(&ids[0], PICKED, Some(&json!(true))).unwrap();
    s.set_field(&ids[1], REJECTED, Some(&json!(true))).unwrap();
    // Picked, then rejected: the reject wins.
    s.set_field(&ids[2], PICKED, Some(&json!(true))).unwrap();
    s.set_field(&ids[2], REJECTED, Some(&json!(true))).unwrap();
    s.set_field(&ids[0], LABELS, Some(&json!(["red"]))).unwrap();
    s.set_field(&ids[3], LABELS, Some(&json!(["blue", "green"])))
        .unwrap();
    s.set_field(&ids[0], TAGS, Some(&json!(["beach", "sea"])))
        .unwrap();
    s.set_field(&ids[4], TAGS, Some(&json!(["sea"]))).unwrap();
    let q = |f: &dyn Fn(&mut Query)| {
        let mut q = Query::default();
        f(&mut q);
        s.select(&q).unwrap()
    };
    assert_eq!(q(&|q| q.flags = vec!["pick".into()]), vec![ids[0]]);
    assert_eq!(
        q(&|q| q.flags = vec!["reject".into()]),
        vec![ids[1], ids[2]]
    );
    assert_eq!(
        q(&|q| q.flags = vec!["none".into(), "pick".into()]),
        vec![ids[0], ids[3], ids[4]]
    );
    assert_eq!(
        q(&|q| q.labels = vec!["red".into(), "green".into()]),
        vec![ids[0], ids[3]],
        "any of the labels"
    );
    assert_eq!(q(&|q| q.tags = vec!["sea".into()]), vec![ids[0], ids[4]]);
    assert_eq!(
        q(&|q| q.tags = vec!["sea".into(), "beach".into()]),
        vec![ids[0]],
        "every tag"
    );
    assert_eq!(
        q(&|q| {
            q.tags = vec!["sea".into()];
            q.flags = vec!["none".into()];
        }),
        vec![ids[4]]
    );
    let bad = Query {
        flags: vec!["maybe".into()],
        ..Query::default()
    };
    assert!(s.select(&bad).unwrap_err().contains("not a flag"));
}

fn tags_are_a_free_set<S: Store>(s: S) {
    let v = Vault::new(with_images(s, 1));
    let a = image(1).id;
    v.apply(
        &[
            Command::add(a, TAGS, " sea "),
            Command::add(a, TAGS, "Beach trip"),
            Command::add(a, TAGS, "sea"),
        ],
        "user",
        None,
        1,
    )
    .unwrap();
    assert_eq!(
        v.fields_of(&[a], &[TAGS]).unwrap()[0].get(TAGS),
        Some(&json!(["Beach trip", "sea"])),
        "trimmed, sorted, once each"
    );
    v.apply(&[Command::remove(a, TAGS, "sea")], "user", None, 2)
        .unwrap();
    assert_eq!(
        v.fields_of(&[a], &[TAGS]).unwrap()[0].get(TAGS),
        Some(&json!(["Beach trip"]))
    );
    assert!(
        v.apply(&[Command::add(a, TAGS, "  ")], "user", None, 3)
            .unwrap_err()
            .contains("empty tag")
    );
    assert!(
        v.apply(&[Command::add(a, PICKED, "x")], "user", None, 4)
            .unwrap_err()
            .contains("not a set")
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
            order: Order::desc(Sort::Rating),
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

/// A library window reads each image's fields and locations together, in
/// one query whatever its size, as `fields_of` and `locations` give them;
/// a refresh lists the images whose copies moved since.
fn library_rows_come_with_their_locations_and_moves_show_in_deltas<S: Store>(s: S) {
    let mut s = with_images(s, 3);
    let (a, b, c) = (image(1).id, image(2).id, image(3).id);
    let away = Location {
        volume: Some(VolumeId("uuid:backup".into())),
        rel_path: Some("shoot/b.nef".into()),
        presence: Presence::Offline,
        ..location("/media/backup/shoot/b.nef")
    };
    s.put_location(&a, &location("/z/a.nef")).unwrap();
    s.put_location(&a, &location("/y/a2.nef")).unwrap();
    s.put_location(&b, &away).unwrap();
    s.set_field(&a, RATING, Some(&json!(4))).unwrap();
    s.set_field(&b, TAGS, Some(&json!(["x"]))).unwrap();
    let v = Vault::new(s);
    let cursor = v.open_cursor(Query::default()).unwrap();
    let q0 = v.reads().queries;
    let rows = v
        .fetch_library(cursor.id, 0, cursor.count, &[RATING, TAGS])
        .unwrap();
    assert_eq!(v.reads().queries - q0, 1, "a library window is one query");
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        v.cursor_ids(cursor.id, 0, cursor.count).unwrap()
    );
    let ids = [a, b, c];
    let fields = v.fields_of(&ids, &[RATING, TAGS]).unwrap();
    for (id, fields) in ids.iter().zip(fields) {
        let row = rows.iter().find(|r| r.id == *id).unwrap();
        assert_eq!(row.fields, fields, "the fields of {}", id.hex());
        assert_eq!(
            row.locations,
            v.locations(id).unwrap(),
            "where {} is",
            id.hex()
        );
    }
    // In the order asked, no field asked, an image with no copy.
    let some = v.library_rows(&[c, a], &[]).unwrap();
    assert_eq!(some.iter().map(|r| r.id).collect::<Vec<_>>(), vec![c, a]);
    assert!(some[0].locations.is_empty() && some[0].fields.is_empty());
    assert_eq!(some[1].locations.len(), 2);
    assert!(v.library_rows(&[], &[RATING]).unwrap().is_empty());

    // A scan finds b's volume plugged in: b's row is read again, nothing
    // else.
    let back = Location {
        presence: Presence::Present,
        ..away.clone()
    };
    v.update_locations(&[(b, away.path.clone(), back)]).unwrap();
    let delta = v.refresh(cursor.id).unwrap();
    assert!(delta.changed);
    assert_eq!(delta.replaced, vec![b]);
    assert!(delta.inserted.is_empty() && delta.removed.is_empty());
    // Read once: the next refresh has nothing new.
    assert!(!v.refresh(cursor.id).unwrap().changed);
    // A field changed: no copy moved.
    v.apply(&[Command::set(c, RATING, json!(2))], "user", None, 9)
        .unwrap();
    assert!(v.refresh(cursor.id).unwrap().replaced.is_empty());
    v.close_cursor(cursor.id);
}

/// The Library of the filter and facet scenarios (`docs/design/library.md`,
/// L4), and the backup rule they are measured against: 1 `IMG_0001.CR3`
/// present, tagged `Été`, a verified copy on one backup drive (short of
/// two); 2 `beach.jpg` present, its backup copy not verified; 3 offline,
/// verified on two drives, one off site (met); 4 missing, its backup copy
/// missing too (none); 5 no copy at all (none).
fn library_fixture<S: Store>(s: S) -> (Vault<S>, Vec<ImageId>, crate::Backups) {
    use crate::capture::{CAMERA_MODEL, FILE_TYPE, LENS};
    let mut s = with_images(s, 5);
    let ids: Vec<ImageId> = (1..=5).map(|i| image(i).id).collect();
    let root = Path::new(std::path::MAIN_SEPARATOR_STR).join("p");
    let copy = |path: PathBuf, volume: &str, rel: &str, presence: Presence| Location {
        volume: Some(VolumeId(volume.into())),
        rel_path: Some(PathBuf::from(rel)),
        presence,
        ..Location::seen(path, 101, 5, 7)
    };
    let shots = root.join("shots");
    let backup = root.join("backup");
    let placed = [
        (
            0,
            copy(
                shots.join("IMG_0001.CR3"),
                "uuid:lib",
                "shots/IMG_0001.CR3",
                Presence::Present,
            ),
        ),
        (
            0,
            copy(
                backup.join("IMG_0001.CR3"),
                "uuid:b1",
                "a/IMG_0001.CR3",
                Presence::Present,
            ),
        ),
        (
            1,
            copy(
                shots.join("beach.jpg"),
                "uuid:lib",
                "shots/beach.jpg",
                Presence::Present,
            ),
        ),
        (
            1,
            copy(
                backup.join("beach.jpg"),
                "uuid:b1",
                "a/beach.jpg",
                Presence::Present,
            ),
        ),
        (
            2,
            copy(
                root.join("other").join("x.nef"),
                "uuid:lib",
                "other/x.nef",
                Presence::Offline,
            ),
        ),
        (
            2,
            copy(
                backup.join("x.nef"),
                "uuid:b1",
                "a/x.nef",
                Presence::Offline,
            ),
        ),
        (
            2,
            copy(
                root.join("far").join("x.nef"),
                "uuid:b2",
                "x.nef",
                Presence::Offline,
            ),
        ),
        (
            3,
            copy(
                shots.join("y.nef"),
                "uuid:lib",
                "shots/y.nef",
                Presence::Missing,
            ),
        ),
        (
            3,
            copy(
                backup.join("y.nef"),
                "uuid:b1",
                "a/y.nef",
                Presence::Missing,
            ),
        ),
    ];
    for (i, location) in &placed {
        s.put_location(&ids[*i], location).unwrap();
    }
    let verified = |pairs: &[(&str, &str)]| {
        let mut doc = serde_json::Map::new();
        for (volume, rel) in pairs {
            doc.insert(format!("{volume}\t{}", Path::new(rel).display()), json!(5));
        }
        serde_json::Value::Object(doc)
    };
    let docs = [
        (0, verified(&[("uuid:b1", "a/IMG_0001.CR3")])),
        (2, verified(&[("uuid:b1", "a/x.nef"), ("uuid:b2", "x.nef")])),
        (3, verified(&[("uuid:b1", "a/y.nef")])),
    ];
    for (i, doc) in &docs {
        s.set_document(crate::storage::VERIFIED, &ids[*i].hex(), Some(doc))
            .unwrap();
    }
    for (i, field, value) in [
        (0, TAGS, json!(["Été"])),
        (0, LABELS, json!(["red"])),
        (2, LABELS, json!(["blue", "red"])),
        (0, RATING, json!(3)),
        (1, RATING, json!(3)),
        (1, PICKED, json!(true)),
        (0, CAMERA_MODEL, json!("Z 8")),
        (1, CAMERA_MODEL, json!("Z 8")),
        (2, CAMERA_MODEL, json!("R5")),
        (0, LENS, json!("L1")),
        (2, LENS, json!("L2")),
        (0, FILE_TYPE, json!("cr3")),
        (1, FILE_TYPE, json!("jpg")),
        (2, FILE_TYPE, json!("nef")),
    ] {
        s.set_field(&ids[i], field, Some(&value)).unwrap();
    }
    let rule = crate::Backups {
        volumes: vec![
            crate::BackupVolume {
                id: VolumeId("uuid:b1".into()),
                drive: "d1".into(),
                off_site: false,
            },
            crate::BackupVolume {
                id: VolumeId("uuid:b2".into()),
                drive: "d2".into(),
                off_site: true,
            },
        ],
        copies: 2,
        off_site_required: true,
    };
    (Vault::new(s), ids, rule)
}

/// Every term of the Library's filter runs in the one query a cursor
/// opens with: words (in the name the Library shows or a tag, whatever
/// the case, accents included), camera, lens, file type, folder,
/// presence and backup state, alone and together.
fn queries_filter_by_every_library_term_in_one_query<S: Store>(s: S) {
    use crate::{BackupFilter, BackupState};
    let (v, ids, rule) = library_fixture(s);
    let pick = |n: &[usize]| n.iter().map(|&i| ids[i]).collect::<Vec<_>>();
    let run = |f: &dyn Fn(&mut Query)| {
        let mut q = Query::default();
        f(&mut q);
        let q0 = v.reads().queries;
        let cursor = v.open_cursor(q.clone()).unwrap();
        assert_eq!(v.reads().queries - q0, 1, "{q:?}: one query");
        let got = v.cursor_ids(cursor.id, 0, 99).unwrap();
        v.close_cursor(cursor.id);
        got
    };
    let words = |w: &[&str]| {
        let w: Vec<String> = w.iter().map(ToString::to_string).collect();
        run(&|q| q.words.clone_from(&w))
    };
    assert_eq!(words(&["img"]), pick(&[0]), "the name, whatever its case");
    assert_eq!(words(&["BEACH", ".jpg"]), pick(&[1]), "every word");
    assert_eq!(words(&["été"]), pick(&[0]), "a tag, accents folded");
    assert_eq!(
        words(&["x.nef"]),
        pick(&[]),
        "no present copy: the Library names it by its id"
    );
    assert_eq!(words(&[&ids[4].hex()[..8]]), pick(&[4]));
    assert_eq!(run(&|q| q.cameras = vec!["Z 8".into()]), pick(&[0, 1]));
    assert_eq!(run(&|q| q.cameras = vec!["z 8".into()]), pick(&[]), "exact");
    assert_eq!(
        run(&|q| q.lenses = vec!["L2".into(), "L1".into()]),
        pick(&[0, 2])
    );
    assert_eq!(run(&|q| q.file_types = vec!["nef".into()]), pick(&[2]));
    let shots = Path::new(std::path::MAIN_SEPARATOR_STR)
        .join("p")
        .join("shots");
    let folder = |f: String| run(&|q| q.folder = Some(f.clone()));
    assert_eq!(folder(shots.display().to_string()), pick(&[0, 1, 3]));
    assert_eq!(
        folder(format!("{}{}", shots.display(), std::path::MAIN_SEPARATOR)),
        pick(&[0, 1, 3])
    );
    assert_eq!(
        folder(shots.display().to_string()[..6].to_string()),
        pick(&[]),
        "a folder, not the start of a name"
    );
    assert_eq!(run(&|q| q.presence = vec![Presence::Offline]), pick(&[2]));
    assert_eq!(
        run(&|q| q.presence = vec![Presence::Missing]),
        pick(&[3, 4])
    );
    assert_eq!(
        run(&|q| q.presence = vec![Presence::Present]),
        pick(&[0, 1])
    );
    let backup = |states: Vec<BackupState>| {
        run(&|q| {
            q.backup = Some(BackupFilter {
                rule: rule.clone(),
                states: states.clone(),
            });
        })
    };
    assert_eq!(backup(vec![BackupState::Short]), pick(&[0]));
    assert_eq!(backup(vec![BackupState::Unverified]), pick(&[1]));
    assert_eq!(backup(vec![BackupState::Met]), pick(&[2]));
    assert_eq!(backup(vec![BackupState::None]), pick(&[3, 4]));
    assert_eq!(
        backup(vec![BackupState::Short, BackupState::Unverified]),
        pick(&[0, 1])
    );
    assert_eq!(backup(vec![]), pick(&[]), "no state: no photo");
    // Every term together, sorted.
    assert_eq!(
        run(&|q| {
            q.words = vec![".".into()];
            q.cameras = vec!["Z 8".into()];
            q.presence = vec![Presence::Present];
            q.min_rating = Some(3);
            q.backup = Some(BackupFilter {
                rule: rule.clone(),
                states: vec![BackupState::Short, BackupState::Unverified],
            });
            q.order = Order::desc(Sort::Name);
        }),
        pick(&[1, 0]),
        "`beach.jpg` after `IMG_0001.CR3`, descending"
    );
}

/// The count per value of every facet comes in one query, for the whole
/// Library and for a filter.
fn facets_count_each_value_in_one_query<S: Store>(s: S) {
    let (v, _, rule) = library_fixture(s);
    let q0 = v.reads().queries;
    let all = v.facets(&Query::default(), Some(&rule)).unwrap();
    assert_eq!(v.reads().queries - q0, 1, "every facet in one query");
    let want = |pairs: &[(&str, &[(&str, u64)])]| {
        let mut f = crate::Facets::default();
        for (facet, values) in pairs {
            for (value, n) in *values {
                f.add(facet, value, *n);
            }
        }
        f
    };
    assert_eq!(
        all,
        want(&[
            ("rating", &[("0", 3), ("3", 2)]),
            ("label", &[("red", 2), ("blue", 1)]),
            ("flag", &[("pick", 1), ("none", 4)]),
            ("camera", &[("Z 8", 2), ("R5", 1)]),
            ("lens", &[("L1", 1), ("L2", 1)]),
            ("file_type", &[("cr3", 1), ("jpg", 1), ("nef", 1)]),
            (
                "presence",
                &[("present", 2), ("offline", 1), ("missing", 2)]
            ),
            (
                "backup",
                &[("met", 1), ("short", 1), ("unverified", 1), ("none", 2)]
            ),
        ])
    );
    assert_eq!(all.count("camera", "Z 8"), 2);
    // For a filter, without a rule: no backup counts.
    let z8 = Query {
        cameras: vec!["Z 8".into()],
        ..Query::default()
    };
    let some = v.facets(&z8, None).unwrap();
    assert_eq!(
        some,
        want(&[
            ("rating", &[("3", 2)]),
            ("label", &[("red", 1)]),
            ("flag", &[("pick", 1), ("none", 1)]),
            ("camera", &[("Z 8", 2)]),
            ("lens", &[("L1", 1)]),
            ("file_type", &[("cr3", 1), ("jpg", 1)]),
            ("presence", &[("present", 2)]),
        ])
    );
}

/// The folders tree's counts (`docs/design/library.md`, L5): every folder
/// holding a copy and each folder above it, apart for each volume, with
/// how many photos have a copy there or below, in one query; a folder on
/// one volume counts what the `folder:` filter selects.
fn folders_count_their_photos_and_those_below_in_one_query<S: Store>(s: S) {
    let (v, _, _) = library_fixture(s);
    let root = Path::new(std::path::MAIN_SEPARATOR_STR);
    let p = root.join("p");
    let key = |path: &Path| path.to_string_lossy().into_owned();
    let on = |name: &str| Some(VolumeId(name.into()));
    let q0 = v.reads().queries;
    let all = v.folders(&Query::default()).unwrap();
    assert_eq!(v.reads().queries - q0, 1, "every folder in one query");
    let counts = |rows: &[(&Path, u64)]| -> std::collections::BTreeMap<String, u64> {
        rows.iter().map(|(path, n)| (key(path), *n)).collect()
    };
    let want: crate::VolumeFolders = [
        (
            on("uuid:lib"),
            counts(&[
                (root, 4),
                (&p, 4),
                (&p.join("shots"), 3),
                (&p.join("other"), 1),
            ]),
        ),
        (
            on("uuid:b1"),
            counts(&[(root, 4), (&p, 4), (&p.join("backup"), 4)]),
        ),
        (
            on("uuid:b2"),
            counts(&[(root, 1), (&p, 1), (&p.join("far"), 1)]),
        ),
    ]
    .into_iter()
    .collect();
    assert_eq!(all, want);
    for dir in ["shots", "other", "backup", "far"] {
        let folder = key(&p.join(dir));
        let n = all.values().find_map(|f| f.get(&folder)).unwrap();
        let cursor = v
            .open_cursor(Query {
                folder: Some(folder.clone()),
                ..Query::default()
            })
            .unwrap();
        v.close_cursor(cursor.id);
        assert_eq!(cursor.count as u64, *n, "{folder}: as `folder:` selects");
    }
    // Within a filter: its photos only.
    let z8 = Query {
        cameras: vec!["Z 8".into()],
        ..Query::default()
    };
    let some = v.folders(&z8).unwrap();
    let lib = &some[&on("uuid:lib")];
    assert_eq!(lib.get(&key(&p.join("shots"))), Some(&2));
    assert_eq!(lib.get(&key(&p.join("other"))), None);
    assert_eq!(some.get(&on("uuid:b2")), None);
}

/// The photos of an album, as a query selects them.
fn in_album<S: Store>(v: &Vault<S>, album: &str) -> Vec<ImageId> {
    let c = v
        .open_cursor(Query {
            album: Some(album.into()),
            ..Query::default()
        })
        .unwrap();
    let ids = v.cursor_ids(c.id, 0, 100).unwrap();
    v.close_cursor(c.id);
    ids
}

/// Albums (`docs/design/library.md`, L6): made with a key from their
/// name, names unique whatever the case, filled and emptied, listed by
/// name, forgotten; a query on an album selects its photos.
fn albums_are_made_filled_renamed_and_forgotten<S: Store>(s: S) {
    let v = Vault::new(with_images(s, 3));
    let id = |n: u8| image(n).id;
    let (a, _) = v.create_album(None, " Trip ", &[id(1), id(1)]).unwrap();
    assert_eq!((a.id.as_str(), a.name.as_str()), ("trip", "Trip"));
    assert_eq!(a.images, vec![id(1).hex()]);
    assert!(
        v.create_album(None, "TRIP", &[]).is_err(),
        "names are unique"
    );
    assert!(v.create_album(Some("trip"), "Other", &[]).is_err());
    assert!(v.create_album(None, " ", &[]).is_err());
    v.add_to_album("trip", &[id(3), id(1)]).unwrap();
    assert_eq!(in_album(&v, "trip"), vec![id(1), id(3)]);
    v.remove_from_album("trip", &[id(1)]).unwrap();
    assert_eq!(in_album(&v, "trip"), vec![id(3)]);
    assert_eq!(v.rename_album("trip", "Summer").unwrap().documents.len(), 1);
    assert!(v.rename_album("trip", "Summer").unwrap().is_empty());
    v.create_album(Some("b"), "Beach", &[]).unwrap();
    let names: Vec<String> = v.albums().unwrap().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["Beach", "Summer"]);
    assert!(v.rename_album("b", "summer").is_err());
    v.delete_album("trip").unwrap();
    assert!(v.album("trip").unwrap().is_none());
    assert!(in_album(&v, "trip").is_empty(), "no album, no photo");
    assert!(v.delete_album("trip").is_err());
    assert!(v.add_to_album("nope", &[id(1)]).is_err());
}

/// An album's changes are steps: undone and redone, a document changed
/// since left alone.
fn an_album_s_changes_are_undone_and_redone<S: Store>(s: S) {
    let v = Vault::new(with_images(s, 3));
    let id = |n: u8| image(n).id;
    let (_, made) = v.create_album(None, "Trip", &[id(1)]).unwrap();
    let added = v.add_to_album("trip", &[id(2)]).unwrap();
    v.revert(&added).unwrap();
    assert_eq!(in_album(&v, "trip"), vec![id(1)]);
    v.replay(&added).unwrap();
    assert_eq!(in_album(&v, "trip"), vec![id(1), id(2)]);
    let gone = v.delete_album("trip").unwrap();
    v.revert(&gone).unwrap();
    assert_eq!(v.album("trip").unwrap().unwrap().images.len(), 2);
    v.add_to_album("trip", &[id(3)]).unwrap();
    let kept = v.revert(&made).unwrap();
    assert_eq!(kept.kept, 1);
    assert_eq!(in_album(&v, "trip").len(), 3);
}

/// A smart album's rule tree (`docs/design/library.md`, L6) compiles into
/// the one query a cursor opens with, whatever its nesting: each kind of
/// field and operator, `all`, `any` and `none` groups, an empty group.
fn smart_album_rules_select_in_one_query<S: Store>(s: S) {
    use crate::rules::Rule;
    let (v, ids, _) = library_fixture(s);
    v.create_album(Some("trip"), "Trip", &[ids[2], ids[4]])
        .unwrap();
    let pick = |n: &[usize]| n.iter().map(|&i| ids[i]).collect::<Vec<_>>();
    let run = |tree: serde_json::Value| {
        let rules = Rule::parse(&tree).unwrap();
        let q0 = v.reads().queries;
        let cursor = v
            .open_cursor(Query {
                rules: Some(rules),
                ..Query::default()
            })
            .unwrap();
        assert_eq!(v.reads().queries - q0, 1, "{tree}: one query");
        let got = v.cursor_ids(cursor.id, 0, 99).unwrap();
        v.close_cursor(cursor.id);
        got
    };
    let one = |field: &str, op: &str, value: serde_json::Value| {
        run(json!({"match": "all", "rules": [{"field": field, "op": op, "value": value}]}))
    };
    let shots = Path::new(std::path::MAIN_SEPARATOR_STR)
        .join("p")
        .join("shots")
        .display()
        .to_string();
    assert_eq!(one("rating", ">=", json!(3)), pick(&[0, 1]));
    assert_eq!(
        one("rating", "=", json!(0)),
        pick(&[2, 3, 4]),
        "unrated is 0"
    );
    assert_eq!(one("rating", "between", json!([1, 3])), pick(&[0, 1]));
    assert_eq!(one("rating", "!=", json!(3)), pick(&[2, 3, 4]));
    assert_eq!(one("label", "has", json!("red")), pick(&[0, 2]));
    assert_eq!(one("label", "lacks", json!("red")), pick(&[1, 3, 4]));
    assert_eq!(one("tag", "has", json!("Été")), pick(&[0]));
    assert_eq!(one("flag", "is", json!("pick")), pick(&[1]));
    assert_eq!(one("flag", "is_not", json!("none")), pick(&[1]));
    assert_eq!(one("camera", "is", json!("Z 8")), pick(&[0, 1]));
    assert_eq!(
        one("camera", "contains", json!("z")),
        pick(&[0, 1]),
        "any case"
    );
    assert_eq!(one("camera", "is_not", json!("Z 8")), pick(&[2, 3, 4]));
    assert_eq!(one("lens", "is", json!("L2")), pick(&[2]));
    assert_eq!(one("file_type", "is", json!("nef")), pick(&[2]));
    assert_eq!(one("iso", ">", json!(0)), pick(&[]), "no ISO: no match");
    assert_eq!(one("folder", "in", json!(shots)), pick(&[0, 1, 3]));
    assert_eq!(one("folder", "not_in", json!(shots)), pick(&[2, 4]));
    assert_eq!(one("album", "in", json!("trip")), pick(&[2, 4]));
    assert_eq!(one("album", "not_in", json!("trip")), pick(&[0, 1, 3]));
    // Groups, nested.
    assert_eq!(
        run(json!({"match": "all", "rules": []})),
        pick(&[0, 1, 2, 3, 4])
    );
    assert_eq!(run(json!({"match": "any", "rules": []})), pick(&[]));
    assert_eq!(
        run(json!({"match": "any", "rules": [
            {"match": "all", "rules": [
                {"field": "camera", "op": "is", "value": "Z 8"},
                {"field": "flag", "op": "is", "value": "pick"},
            ]},
            {"field": "label", "op": "has", "value": "blue"},
        ]})),
        pick(&[1, 2])
    );
    assert_eq!(
        run(json!({"match": "all", "rules": [
            {"field": "folder", "op": "in", "value": shots},
            {"match": "none", "rules": [
                {"field": "rating", "op": ">=", "value": 3},
                {"match": "any", "rules": [
                    {"field": "file_type", "op": "is", "value": "jpg"},
                ]},
            ]},
        ]})),
        pick(&[3])
    );
}

/// A relative date (`within`) is kept relative and resolved against the
/// clock of each query: the same tree selects other photos on another
/// day, in one query, nested or in a smart album.
fn relative_dates_follow_the_query_s_clock<S: Store>(s: S) {
    use crate::capture::{Capture, day_start};
    use crate::rules::Rule;
    let v = Vault::new(with_images(s, 5));
    let ids: Vec<ImageId> = (1..=5).map(|i| image(i).id).collect();
    let at = |day: &str, hour: i64| (day_start(day).unwrap() + hour * 3600) as f64;
    let shot = |captured: f64| Capture {
        captured: Some(captured),
        ..Capture::default()
    };
    v.set_captures(
        &[
            (ids[0], shot(at("2024-06-15", 9))),
            (ids[1], shot(at("2024-06-14", 23))),
            (ids[2], shot(at("2024-06-02", 12))),
            (ids[3], shot(at("2024-01-01", 0))),
            (ids[4], shot(at("2023-12-31", 23))),
        ],
        None,
    )
    .unwrap();
    let pick = |n: &[usize]| n.iter().map(|&i| ids[i]).collect::<Vec<_>>();
    let run = |query: Query| {
        let q0 = v.reads().queries;
        let cursor = v.open_cursor(query).unwrap();
        assert_eq!(v.reads().queries - q0, 1, "one query");
        let got = v.cursor_ids(cursor.id, 0, 99).unwrap();
        v.close_cursor(cursor.id);
        got
    };
    let within = |value: serde_json::Value| {
        Rule::parse(&json!({"match": "all", "rules": [
            {"field": "captured", "op": "within", "value": value},
        ]}))
        .unwrap()
    };
    let on = |rules: Rule, day: &str| {
        run(Query {
            rules: Some(rules),
            now: Some(day_start(day).unwrap() + 15 * 3600),
            ..Query::default()
        })
    };
    let sorted = |n: &[usize]| {
        let mut p = pick(n);
        p.sort_by_key(super::ImageId::hex);
        p
    };
    assert_eq!(on(within(json!("today")), "2024-06-15"), sorted(&[0]));
    assert_eq!(on(within(json!("today")), "2024-06-14"), sorted(&[1]));
    let week = within(json!({"last": 1, "unit": "weeks"}));
    assert_eq!(on(week.clone(), "2024-06-15"), sorted(&[0, 1]));
    assert_eq!(on(week, "2024-06-21"), sorted(&[0]), "a week later");
    assert_eq!(
        on(within(json!({"last": 14, "unit": "days"})), "2024-06-15"),
        sorted(&[0, 1, 2])
    );
    assert_eq!(
        on(within(json!({"last": 1, "unit": "months"})), "2024-07-02"),
        sorted(&[0, 1]),
        "from June 3rd"
    );
    assert_eq!(
        on(within(json!({"last": 1, "unit": "months"})), "2024-07-01"),
        sorted(&[0, 1, 2]),
        "from June 2nd"
    );
    assert_eq!(
        on(within(json!("this_year")), "2024-06-15"),
        sorted(&[0, 1, 2, 3])
    );
    assert_eq!(on(within(json!("this_year")), "2023-12-31"), sorted(&[4]));
    // Nested, and negated.
    let tree = Rule::parse(&json!({"match": "any", "rules": [
        {"match": "none", "rules": [
            {"field": "captured", "op": "within", "value": "this_year"},
        ]},
        {"field": "captured", "op": "within", "value": "today"},
    ]}))
    .unwrap();
    assert_eq!(on(tree, "2024-06-15"), sorted(&[0, 4]));
    // Kept relative in a smart album: its count follows the clock.
    let today = within(json!("today"));
    v.create_smart_album(None, "Today", today.clone()).unwrap();
    let kept = v.album("today").unwrap().unwrap().rules.unwrap();
    assert_eq!(kept, today, "stored relative");
    let album = |day: &str| {
        run(Query {
            album: Some("today".into()),
            now: Some(day_start(day).unwrap()),
            ..Query::default()
        })
    };
    assert_eq!(album("2024-06-15"), sorted(&[0]));
    assert_eq!(album("2024-06-02"), sorted(&[2]));
}

/// A smart album is a collection of its own kind: a query naming it runs
/// its rules (one query, with the other terms), its count follows the
/// photos, its rules are set as one step undone and redone; photos are
/// never put in it, and rules that do not parse are refused.
fn smart_albums_scope_count_and_undo<S: Store>(s: S) {
    use crate::rules::{Match, Rule};
    let (v, ids, _) = library_fixture(s);
    let tree = |json: serde_json::Value| Rule::parse(&json).unwrap();
    let rated =
        tree(json!({"match": "all", "rules": [{"field": "rating", "op": ">=", "value": 3}]}));
    let (a, made) = v.create_smart_album(None, "Best", rated.clone()).unwrap();
    assert_eq!((a.id.as_str(), a.is_smart()), ("best", true));
    assert!(
        v.create_smart_album(None, "best", rated.clone()).is_err(),
        "names are unique"
    );
    assert_eq!(in_album(&v, "best"), vec![ids[0], ids[1]]);
    let q0 = v.reads().queries;
    let c = v
        .open_cursor(Query {
            album: Some("best".into()),
            cameras: vec!["Z 8".into()],
            flags: vec!["pick".into()],
            ..Query::default()
        })
        .unwrap();
    assert_eq!(v.reads().queries - q0, 1, "one query");
    assert_eq!(v.cursor_ids(c.id, 0, 9).unwrap(), vec![ids[1]]);
    v.close_cursor(c.id);
    let facets = v
        .facets(
            &Query {
                album: Some("best".into()),
                ..Query::default()
            },
            None,
        )
        .unwrap();
    assert_eq!(facets.count("rating", "3"), 2, "{facets:?}");
    let best = |v: &Vault<S>| {
        v.albums()
            .unwrap()
            .into_iter()
            .find(|a| a.id == "best")
            .unwrap()
    };
    assert_eq!(best(&v).count, Some(2));
    // The count follows the photos.
    v.apply(&[Command::set(ids[4], RATING, json!(5))], "user", None, 1)
        .unwrap();
    assert_eq!(best(&v).count, Some(3));
    // Rules set as one step, undone and redone.
    let red =
        tree(json!({"match": "any", "rules": [{"field": "label", "op": "has", "value": "red"}]}));
    let set = v.set_album_rules("best", red.clone()).unwrap();
    assert_eq!(set.documents.len(), 1);
    assert!(v.set_album_rules("best", red.clone()).unwrap().is_empty());
    assert_eq!(in_album(&v, "best"), vec![ids[0], ids[2]]);
    v.revert(&set).unwrap();
    assert_eq!(best(&v).rules, Some(rated.clone()));
    assert_eq!(in_album(&v, "best"), vec![ids[0], ids[1], ids[4]]);
    v.replay(&set).unwrap();
    assert_eq!(best(&v).rules, Some(red));
    // Photos are never put in a smart album; a plain album takes no rules.
    assert!(
        v.add_to_album("best", &[ids[3]])
            .unwrap_err()
            .contains("smart")
    );
    assert!(v.remove_from_album("best", &[ids[0]]).is_err());
    v.create_album(Some("trip"), "Trip", &[]).unwrap();
    let empty = Rule::group(Match::All, Vec::new());
    assert!(v.set_album_rules("trip", empty.clone()).is_err());
    assert!(v.set_album_rules("nope", empty).is_err());
    // Rules a hand wrote wrong: refused when run, with the reason.
    let store_doc = json!({"kind": "smart", "name": "Bad", "rules":
        {"match": "all", "rules": [{"field": "colour", "op": "is", "value": "red"}]}});
    v.transaction(|s| s.set_document(crate::collections::COLLECTIONS, "bad", Some(&store_doc)))
        .unwrap();
    let e = v
        .open_cursor(Query {
            album: Some("bad".into()),
            ..Query::default()
        })
        .unwrap_err();
    assert!(e.contains("unknown field `colour`"), "{e}");
    // Undoing back to the album's making forgets it.
    v.revert(&set).unwrap();
    v.revert(&made).unwrap();
    assert!(v.album("best").unwrap().is_none());
}
