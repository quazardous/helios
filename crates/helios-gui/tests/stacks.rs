//! Stacks on the desktop: `stacks.suggest` proposals reviewed in the
//! Stacks panel and recorded as declined, corrected or accepted; the
//! stacks made shown collapsed in the library, expanded on demand.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use egui::vec2;
use helios_core::Bus;
use helios_gui::persist::Persist;
use helios_gui::{Shell, headless, helios_panels};
use helios_library::signals::Signals;
use helios_library::stacks::Status;
use helios_shell::commands::Origin;
use helios_vault::ImageId;
use helios_vault_sqlite::SqliteStore;
use serde_json::json;

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-stacks-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn shot(taken: f64, hash: u64, sharpness: f64, bias: f64) -> Signals {
    Signals {
        phash: Some(format!("{hash:016x}")),
        taken: Some(taken),
        taken_from: Some("exif".into()),
        exposure_time: Some(0.01),
        aperture: Some(4.0),
        iso: Some(100.0),
        exposure_bias: Some(bias),
        sharpness: Some(sharpness),
        ..Signals::default()
    }
}

/// A burst of three (the middle one sharpest) and a bracket of three (its
/// middle one sharpest), a minute apart, each image a small file of its
/// own in the vault with its signals.
fn shoot(dir: &Path, persist: &Persist<SqliteStore>) -> (Vec<ImageId>, Vec<ImageId>) {
    let base: u64 = 0xF0F0_F0F0_0F0F_0F0F;
    let wide = 0x0000_0000_0000_0FFF_u64;
    let shots = [
        shot(0.0, base, 1.0, 0.0),
        shot(0.5, base ^ 0b1, 3.0, 0.0),
        shot(1.0, base ^ 0b11, 2.0, 0.0),
        shot(120.0, base, 1.0, -1.0),
        shot(120.3, base ^ wide, 4.0, 0.0),
        shot(120.6, base, 2.0, 1.0),
    ];
    let ids: Vec<ImageId> = shots
        .iter()
        .enumerate()
        .map(|(n, s)| {
            let file = dir.join(format!("IMG_{n:04}.CR2"));
            std::fs::write(&file, format!("frame {n}")).unwrap();
            let id = persist.vault().import_file(&file, 1).unwrap();
            let doc = serde_json::to_value(s).unwrap();
            persist.vault().set_signals(&id, Some(&doc)).unwrap();
            id
        })
        .collect();
    (ids[..3].to_vec(), ids[3..].to_vec())
}

fn setup(
    name: &str,
) -> (
    PathBuf,
    Shell,
    Persist<SqliteStore>,
    Vec<ImageId>,
    Vec<ImageId>,
) {
    let dir = scratch(name);
    let persist = Persist::new(
        SqliteStore::open(&dir.join("vault.db")).unwrap(),
        Arc::new(Bus::new()),
    );
    let (burst, bracket) = shoot(&dir, &persist);
    let mut shell = Shell::new(helios_panels(), None);
    persist.fill_library(&mut shell).unwrap();
    (dir, shell, persist, burst, bracket)
}

fn by(d: &helios_library::stacks::Decision) -> String {
    serde_json::to_value(d.answer.as_ref().unwrap()).unwrap()["by"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn proposals_are_declined_corrected_or_accepted_from_the_panel() {
    let (dir, mut shell, mut persist, burst, bracket) = setup("review");
    let decision = |persist: &Persist<SqliteStore>, id: &str| {
        helios_library::stacks::decision(persist.vault().as_ref(), id).unwrap()
    };

    // Asked, over every image: two groups, nothing applied.
    shell.stack_review_mut().suggest(Vec::new());
    persist.frame(&mut shell, 10.0);
    let first = shell.stack_review().decisions()[0].clone();
    assert_eq!(first.proposal.groups.len(), 2);
    assert!(persist.vault().stacks().unwrap().is_empty());

    // Declined: recorded, no stack made, the proposal leaves the panel.
    shell.stack_review_mut().decline(&first.id);
    persist.frame(&mut shell, 11.0);
    let declined = decision(&persist, &first.id);
    assert_eq!(declined.status, Status::Declined);
    assert_eq!(by(&declined), "human");
    assert!(persist.vault().stacks().unwrap().is_empty());
    assert!(shell.stack_review().decisions().is_empty());

    // Asked again; the bracket unchecked: only the burst is taken, a
    // correction of the proposal.
    shell.stack_review_mut().suggest(Vec::new());
    persist.frame(&mut shell, 12.0);
    let second = shell.stack_review().decisions()[0].clone();
    let bracket_group = second
        .proposal
        .groups
        .iter()
        .position(|g| g.kind == "bracket")
        .unwrap();
    shell
        .stack_review_mut()
        .check(&second.id, bracket_group, false);
    assert!(!shell.stack_review().checked(&second.id, bracket_group));
    shell.stack_review_mut().accept_checked(&second.id).unwrap();
    persist.frame(&mut shell, 13.0);
    let corrected = decision(&persist, &second.id);
    assert_eq!(corrected.status, Status::Accepted);
    assert_eq!(by(&corrected), "corrected");
    let stacks = persist.vault().stacks().unwrap();
    assert_eq!(stacks.len(), 1);
    assert_eq!(stacks[0].top, burst[1].hex(), "the sharpest on top");

    // Asked again: the stacked burst is left out, the bracket accepted as
    // proposed.
    shell.stack_review_mut().suggest(Vec::new());
    persist.frame(&mut shell, 14.0);
    let third = shell.stack_review().decisions()[0].clone();
    assert_eq!(third.proposal.groups.len(), 1);
    assert_eq!(third.left_out.len(), 3);
    shell.stack_review_mut().accept_all(&third.id);
    persist.frame(&mut shell, 15.0);
    assert_eq!(by(&decision(&persist, &third.id)), "accepted");
    assert_eq!(persist.vault().stacks().unwrap().len(), 2);
    let tops: Vec<String> = persist
        .vault()
        .stacks()
        .unwrap()
        .iter()
        .map(|s| s.top.clone())
        .collect();
    assert!(tops.contains(&bracket[1].hex()));
    drop((shell, persist));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn accepting_with_nothing_checked_is_refused() {
    let (dir, mut shell, mut persist, _, _) = setup("none-checked");
    shell.stack_review_mut().suggest(Vec::new());
    persist.frame(&mut shell, 10.0);
    let d = shell.stack_review().decisions()[0].clone();
    for g in 0..d.proposal.groups.len() {
        shell.stack_review_mut().check(&d.id, g, false);
    }
    let refused = shell.stack_review_mut().accept_checked(&d.id);
    assert!(refused.unwrap_err().contains("decline"));
    drop((shell, persist));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stacks_show_collapsed_and_expand_on_demand() {
    let (dir, mut shell, mut persist, burst, bracket) = setup("collapsed");
    shell.stack_review_mut().suggest(Vec::new());
    persist.frame(&mut shell, 10.0);
    let d = shell.stack_review().decisions()[0].id.clone();
    shell.stack_review_mut().accept_all(&d);
    persist.frame(&mut shell, 11.0);
    let stack = persist
        .vault()
        .stacks()
        .unwrap()
        .into_iter()
        .find(|s| s.top == burst[1].hex())
        .unwrap();
    {
        let library = shell.library();
        // Each stack's top first, its members right after it, collapsed.
        let shown = library.shown();
        let at = |id: &ImageId| shown.iter().position(|s| *s == id.hex()).unwrap();
        for (group, top) in [(&burst, &burst[1]), (&bracket, &bracket[1])] {
            let mut places: Vec<usize> = group.iter().map(at).collect();
            places.sort_unstable();
            assert_eq!(places[0], at(top), "the top first");
            assert_eq!(places[2], places[0] + 2, "the members right after it");
        }
        assert!(!library.is_expanded(&burst[1].hex()));
        let member = library
            .entries
            .iter()
            .find(|e| e.id == burst[2].hex())
            .unwrap();
        let of = member.stack.as_ref().unwrap();
        assert_eq!((of.top.clone(), of.count), (burst[1].hex(), 3));
    }
    // S on an image of the stack opens it, and the vault keeps it open.
    let place = shell
        .library()
        .entries
        .iter()
        .position(|e| e.id == burst[0].hex())
        .unwrap();
    shell.library_mut().select(place);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h.state_mut()
        .run("vault.stack.toggle", json!(null), Origin::User)
        .unwrap();
    h.run();
    assert!(h.state().library().is_expanded(&burst[1].hex()));
    persist.frame(h.state_mut(), 12.0);
    let kept = persist.vault().stack(&stack.id).unwrap().unwrap();
    assert!(kept.expanded);
    // Opened in the Stacks panel too: drawn without an answer waiting.
    h.state_mut()
        .run("helios.stacks.focus", json!(null), Origin::User)
        .unwrap();
    h.run();
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&dir);
}
