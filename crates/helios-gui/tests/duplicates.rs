//! Exact duplicates on the desktop: what an import's policy did with each
//! one is listed in the Duplicates panel, and with `ask` each file waits
//! for its own answer (keep it too, or skip it), nothing deleted.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use egui::vec2;
use helios_gui::duplicates::Fate;
use helios_gui::{Shell, headless, helios_panels};
use helios_library::import::{Duplicates, Options, import};
use helios_shell::commands::Origin;
use helios_vault::{MemoryStore, Vault};
use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-duplicates-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Imports `source` with `policy`, its lines given to the shell as the
/// desktop's `library.import` gives them.
fn run_import(shell: &Shell, vault: &Vault<MemoryStore>, source: &Path, policy: &str) {
    let args = json!({ "path": source, "duplicates": policy });
    let options = Options {
        duplicates: Duplicates::parse(policy).unwrap(),
        signals: false,
        ..Options::default()
    };
    let sink = shell.duplicates_sink();
    let jobs = helios_core::jobs::Jobs::new(std::sync::Arc::new(helios_core::Bus::new()));
    import(
        vault,
        &jobs,
        &[source.to_path_buf()],
        &options,
        1,
        &mut |line: &Value| sink.send((args.clone(), line.clone())).unwrap(),
    )
    .unwrap();
}

fn write(path: &Path, bytes: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn asked_duplicates_wait_for_an_answer_each() {
    let dir = scratch("ask");
    let vault = Vault::new(MemoryStore::default());
    // The library has a picture already.
    write(&dir.join("library/sea.jpg"), "the sea");
    let shell = Shell::new(helios_panels(), None);
    run_import(&shell, &vault, &dir.join("library"), "skip");
    // A card: the same picture, and one file twice.
    write(&dir.join("card/DCIM/a.jpg"), "the sea");
    write(&dir.join("card/DCIM/b.jpg"), "a beach");
    write(&dir.join("card/DCIM/c.jpg"), "a beach");
    run_import(&shell, &vault, &dir.join("card"), "ask");
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    {
        let d = h.state().duplicates();
        let found = d.found();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|f| f.fate == Fate::Asked));
        let of: Vec<&str> = found.iter().map(|f| f.of.as_str()).collect();
        assert!(of.contains(&"library") && of.contains(&"source"), "{of:?}");
        assert_eq!(d.waiting(), 2);
    }
    // The note says so.
    assert!(
        h.state()
            .notes
            .iter()
            .any(|n| n.contains("2 duplicates wait")),
        "{:?}",
        h.state().notes
    );
    // Answered in the panel, file by file.
    h.state_mut()
        .run("helios.duplicates.focus", json!(null), Origin::User)
        .unwrap();
    h.run();
    let library_copy = h
        .state()
        .duplicates()
        .found()
        .iter()
        .position(|f| f.of == "library")
        .unwrap();
    let other = 1 - library_copy;
    h.state_mut().duplicates_mut().keep(library_copy).unwrap();
    h.state_mut().duplicates_mut().skip(other).unwrap();
    assert!(
        h.state_mut().duplicates_mut().keep(other).is_err(),
        "answered once"
    );
    let imports = h.state_mut().duplicates_mut().take_imports();
    let kept = h.state().duplicates().found()[library_copy].clone();
    assert_eq!(kept.fate, Fate::Kept);
    assert_eq!(
        imports,
        [json!({ "path": kept.path, "duplicates": "keep" })]
    );
    // The host imports it again with `keep`: one more copy of the image.
    let id = helios_vault::ImageId::from_hex(&kept.id).unwrap();
    assert_eq!(vault.locations(&id).unwrap().len(), 1);
    run_import(h.state(), &vault, &kept.path, "keep");
    assert_eq!(vault.locations(&id).unwrap().len(), 2);
    // Skipping touched nothing.
    for f in ["a.jpg", "b.jpg", "c.jpg"] {
        assert!(dir.join("card/DCIM").join(f).exists(), "{f}");
    }
    h.state_mut().duplicates_mut().clear_answered();
    h.run();
    assert_eq!(h.state().duplicates().waiting(), 0);
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn skip_and_keep_policies_are_listed_with_what_they_did() {
    let dir = scratch("policies");
    let vault = Vault::new(MemoryStore::default());
    write(&dir.join("one/x.jpg"), "same");
    write(&dir.join("two/x.jpg"), "same");
    write(&dir.join("three/x.jpg"), "same");
    let shell = Shell::new(helios_panels(), None);
    run_import(&shell, &vault, &dir.join("one"), "skip");
    run_import(&shell, &vault, &dir.join("two"), "skip");
    run_import(&shell, &vault, &dir.join("three"), "keep");
    let asked = shell.duplicates_mut().receive();
    assert_eq!(asked, 0);
    let fates: Vec<Fate> = shell.duplicates().found().iter().map(|f| f.fate).collect();
    assert_eq!(fates, [Fate::Skipped, Fate::Located]);
    // Nothing waits: answering is refused.
    assert!(shell.duplicates_mut().keep(0).is_err());
    shell.duplicates_mut().answer_all(true);
    assert!(shell.duplicates_mut().take_imports().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
