//! A card is known by its volume, not by where it is mounted: the same
//! card mounted at another path is the same import source, with its
//! choices and its automatic trigger; another card mounted where it was
//! is another source; a source saved by path on a card's root takes the
//! card's volume the first time it is seen there; a card whose
//! filesystem has no UUID is known by its label and size. Fake volume
//! lists only, no device.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use helios_gui::node_volumes::VolumeWatch;
use helios_gui::shell::{ImportTrigger, ReadSources, WriteSources};
use helios_gui::{Shell, headless, helios_panels};
use helios_storage::{Card, Identity, Volume, VolumeId, Volumes};
use serde_json::Value;

fn temp(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("helios-gui-card-id-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A card's folder at `root`, with one photo in `DCIM`.
fn card_dir(root: &Path) {
    let dcim = root.join("DCIM/100CANON");
    std::fs::create_dir_all(&dcim).unwrap();
    let jpeg = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../helios-plugin/tests/fixtures/rated-photos/a.jpg");
    std::fs::copy(jpeg, dcim.join("IMG_0001.JPG")).unwrap();
}

/// A card's volume mounted at `root`: by its filesystem's UUID, or by
/// its label and size when `uuid` is `None`.
fn card(label: &str, root: &Path, uuid: Option<&str>) -> Volume {
    let mut v = Card {
        label: label.into(),
        root: root.to_path_buf(),
        capacity: Some(64_000_000_000),
    }
    .volume();
    if let Some(uuid) = uuid {
        v.id = VolumeId(format!("uuid:{uuid}"));
        v.identified_by = Identity::Uuid;
    }
    v
}

/// The volumes mounted, as a test plugs and unplugs them.
#[derive(Default)]
struct Slot(Mutex<Vec<Volume>>);

impl Slot {
    fn plug(&self, v: Volume) {
        self.0.lock().unwrap().push(v);
    }

    fn unplug(&self) {
        self.0.lock().unwrap().clear();
    }
}

impl Volumes for Slot {
    fn volumes(&self) -> Vec<Volume> {
        self.0.lock().unwrap().clone()
    }
}

/// The sources' document, kept in memory.
#[derive(Clone, Default)]
struct Doc(Arc<Mutex<Option<String>>>);

impl Doc {
    fn of(text: &str) -> Self {
        Self(Arc::new(Mutex::new(Some(text.to_owned()))))
    }

    fn text(&self) -> String {
        self.0.lock().unwrap().clone().unwrap_or_default()
    }
}

/// A shell keeping its sources in `doc`, following the volumes of
/// `slot` (looked at every 30 ms).
fn shell(doc: &Doc, slot: &Arc<Slot>) -> Harness<'static, Shell> {
    let (read, write) = (doc.clone(), doc.clone());
    let read: ReadSources = Box::new(move || Ok(read.0.lock().unwrap().clone()));
    let write: WriteSources = Box::new(move |text| {
        *write.0.lock().unwrap() = Some(text.to_owned());
        Ok(())
    });
    let mut shell = Shell::new(helios_panels(), None);
    shell.keep_import_sources_with(read, write);
    shell.follow_volumes(VolumeWatch::start(
        slot.clone(),
        Arc::new(helios_core::Bus::new()),
        Duration::from_millis(30),
    ));
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
}

/// Frames stepped until `done` (at most 20 s); the imports asked
/// meanwhile.
fn until(
    h: &mut Harness<'static, Shell>,
    what: &str,
    mut done: impl FnMut(&mut Shell) -> bool,
) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut imports = Vec::new();
    loop {
        h.step();
        imports.extend(h.state_mut().take_imports());
        if done(h.state_mut()) {
            return imports;
        }
        assert!(Instant::now() < deadline, "never got there in 20 s: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Frames stepped for `long`, the imports asked meanwhile.
fn imports_during(h: &mut Harness<'static, Shell>, long: Duration) -> Vec<Value> {
    let end = Instant::now() + long;
    let mut imports = Vec::new();
    while Instant::now() < end {
        h.step();
        imports.extend(h.state_mut().take_imports());
        std::thread::sleep(Duration::from_millis(20));
    }
    imports
}

/// A card source of `volume` last mounted at `path`, imported when
/// plugged in, renaming its photos `{stem}-kept`.
fn saved(path: &Path, volume: Option<&str>) -> String {
    let volume = volume.map_or_else(String::new, |v| format!("volume = \"{v}\"\n"));
    format!(
        "[[source]]\nconnector = \"card\"\nname = \"Camera\"\npath = {path:?}\nauto = \"plugged\"\n{volume}\n\
         [source.choices]\ncopy_folders = true\nlayout = \"month\"\nrename = \"{{stem}}-kept\"\n\
         duplicates = \"skip\"\ndarktable = true\n"
    )
}

/// Steps until the sources list says `done`.
fn sources_until(
    h: &mut Harness<'static, Shell>,
    what: &str,
    done: impl Fn(&[helios_gui::shell::SavedSource]) -> bool,
) -> Vec<Value> {
    until(h, what, |s| done(&s.import_sources()))
}

#[test]
fn the_same_card_at_another_mount_point_is_the_same_source_and_imports_by_its_volume() {
    let dir = temp("moved");
    let (first, second) = (
        dir.join("media/EOS_DIGITAL"),
        dir.join("media/EOS_DIGITAL1"),
    );
    card_dir(&first);
    card_dir(&second);
    let doc = Doc::of(&saved(&first, Some("uuid:0b1f-2c3d")));
    let slot = Arc::new(Slot::default());
    let mut h = shell(&doc, &slot);
    // Mounted elsewhere (a second reader): its trigger fires, with its
    // own choices, from where it is now.
    slot.plug(card("EOS_DIGITAL", &second, Some("0b1f-2c3d")));
    let mut imports = until(&mut h, "the card imported", |s| {
        !s.import_cards().is_empty()
    });
    imports.extend(imports_during(&mut h, Duration::from_millis(300)));
    assert_eq!(imports.len(), 1, "{imports:?}");
    assert_eq!(
        imports[0]["paths"],
        serde_json::json!([second.join("DCIM/100CANON/IMG_0001.JPG")])
    );
    assert_eq!(imports[0]["rename"], "{stem}-kept", "its own choices");
    // Still one source, now where the card is, its label recorded.
    let sources = h.state().import_sources();
    assert_eq!(sources.len(), 1, "{sources:?}");
    assert_eq!(sources[0].path.as_deref(), Some(second.as_path()));
    assert_eq!(sources[0].volume.as_deref(), Some("uuid:0b1f-2c3d"));
    assert_eq!(sources[0].label.as_deref(), Some("EOS_DIGITAL"));
    assert!(doc.text().contains("EOS_DIGITAL1"), "{}", doc.text());
    // Turned off there: the one source changes, none is added.
    h.state_mut().set_import_auto(&second, None);
    h.step();
    let sources = h.state().import_sources();
    assert_eq!(sources.len(), 1, "{sources:?}");
    assert_eq!(sources[0].auto, None);
    assert!(sources[0].choices.is_some(), "its choices kept");
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn another_card_mounted_at_the_same_path_is_another_source() {
    let dir = temp("other");
    let root = dir.join("media/EOS_DIGITAL");
    card_dir(&root);
    let doc = Doc::of(&saved(&root, Some("uuid:aaaa-1111")));
    let slot = Arc::new(Slot::default());
    let mut h = shell(&doc, &slot);
    // Another card, same label, same mount point: not the saved one.
    slot.plug(card("EOS_DIGITAL", &root, Some("bbbb-2222")));
    let mut imports = until(&mut h, "the card listed", |s| !s.import_cards().is_empty());
    imports.extend(imports_during(&mut h, Duration::from_millis(300)));
    assert!(imports.is_empty(), "not its source's trigger: {imports:?}");
    // Set to import there, it is a source of its own.
    h.state_mut()
        .set_import_auto(&root, Some(ImportTrigger::Plugged));
    h.step();
    let sources = h.state().import_sources();
    let volumes: Vec<_> = sources.iter().map(|s| s.volume.as_deref()).collect();
    assert_eq!(
        volumes,
        [Some("uuid:aaaa-1111"), Some("uuid:bbbb-2222")],
        "{sources:?}"
    );
    assert!(sources[1].choices.is_none(), "not the other card's choices");
    // The first card back at that path: its own source, its trigger.
    slot.unplug();
    until(&mut h, "the card unplugged", |s| {
        s.import_cards().is_empty()
    });
    slot.plug(card("EOS_DIGITAL", &root, Some("aaaa-1111")));
    let mut imports = until(&mut h, "the first card listed", |s| {
        !s.import_cards().is_empty()
    });
    imports.extend(imports_during(&mut h, Duration::from_millis(300)));
    assert_eq!(imports.len(), 1, "{imports:?}");
    assert_eq!(imports[0]["rename"], "{stem}-kept");
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_source_saved_by_path_takes_the_cards_volume_the_first_time_it_is_seen() {
    let dir = temp("migrate");
    let (root, later) = (dir.join("media/CARD"), dir.join("run/media/CARD"));
    card_dir(&root);
    card_dir(&later);
    let doc = Doc::of(&saved(&root, None));
    let slot = Arc::new(Slot::default());
    let mut h = shell(&doc, &slot);
    assert_eq!(h.state().import_sources()[0].volume, None);
    slot.plug(card("CARD", &root, Some("f00d-cafe")));
    let imports = sources_until(&mut h, "the source migrated", |s| {
        s.first().and_then(|s| s.volume.as_deref()) == Some("uuid:f00d-cafe")
    });
    let sources = h.state().import_sources();
    assert_eq!(sources.len(), 1, "{sources:?}");
    assert_eq!(sources[0].name.as_deref(), Some("Camera"));
    assert_eq!(
        sources[0].choices.as_ref().map(|c| c.rename.as_str()),
        Some("{stem}-kept"),
        "its choices kept"
    );
    assert!(
        doc.text().contains("volume = \"uuid:f00d-cafe\""),
        "{}",
        doc.text()
    );
    let mut imports = imports;
    imports.extend(imports_during(&mut h, Duration::from_millis(300)));
    assert_eq!(imports.len(), 1, "{imports:?}");
    // Mounted elsewhere afterwards: still that source.
    slot.unplug();
    until(&mut h, "the card unplugged", |s| {
        s.import_cards().is_empty()
    });
    slot.plug(card("CARD", &later, Some("f00d-cafe")));
    let mut imports = sources_until(&mut h, "the source followed the card", |s| {
        s.first().and_then(|s| s.path.as_deref()) == Some(later.as_path())
    });
    imports.extend(imports_during(&mut h, Duration::from_millis(300)));
    assert_eq!(h.state().import_sources().len(), 1);
    assert_eq!(imports.len(), 1, "{imports:?}");
    assert_eq!(
        imports[0]["paths"],
        serde_json::json!([later.join("DCIM/100CANON/IMG_0001.JPG")])
    );
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_card_without_a_uuid_is_known_by_its_label_and_size() {
    let dir = temp("weak");
    let (root, other) = (dir.join("media/NIKON"), dir.join("media/NIKON1"));
    card_dir(&root);
    card_dir(&other);
    let doc = Doc::of(&saved(&root, None));
    let slot = Arc::new(Slot::default());
    let mut h = shell(&doc, &slot);
    slot.plug(card("NIKON", &root, None));
    sources_until(&mut h, "the source migrated", |s| {
        s.first().and_then(|s| s.volume.as_deref()) == Some("weak:NIKON:64000000000")
    });
    // The same label and size elsewhere: the same source.
    slot.unplug();
    until(&mut h, "the card unplugged", |s| {
        s.import_cards().is_empty()
    });
    slot.plug(card("NIKON", &other, None));
    let mut imports = sources_until(&mut h, "the source followed the card", |s| {
        s.first().and_then(|s| s.path.as_deref()) == Some(other.as_path())
    });
    imports.extend(imports_during(&mut h, Duration::from_millis(300)));
    assert_eq!(h.state().import_sources().len(), 1);
    assert_eq!(imports.len(), 1, "{imports:?}");
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
