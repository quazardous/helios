//! The contract every [`Store`] must pass: the vault's rules and budgets
//! hold whatever keeps the rows.
//!
//! ```ignore
//! helios_vault::contract::check(|| SqliteStore::in_memory().unwrap());
//! ```

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use helios_core::ContentHash;

use crate::{Image, ImageId, Location, Store, Vault};

/// Runs every scenario against fresh stores made by `make`.
pub fn check<S: Store>(make: impl Fn() -> S) {
    images_are_added_once(make());
    locations_belong_to_one_image_and_round_trip(make());
    import_reads_only_what_changed(make(), &scratch("import"));
    repair_relinks_by_content_reading_as_little_as_possible(make(), &scratch("repair"));
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
