//! The drawn-mask fixtures of `tests/masks/` are the ones
//! `helios_parity::masks` generates, and Helios imports each with its mask.
//! `HELIOS_BLESS=1` writes them again.

use std::path::Path;

use helios_parity::masks::fixtures;
use helios_xmp::drawn::Frame;
use helios_xmp::{Layouts, Sidecar, import_with};

fn dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/masks")
}

#[test]
fn mask_fixtures_are_up_to_date() {
    let bless = std::env::var_os("HELIOS_BLESS").is_some();
    let layouts = Layouts::built_in();
    let mut stale = Vec::new();
    for fixture in fixtures() {
        let path = dir().join(format!("{}.xmp", fixture.name));
        let text = fixture.sidecar(&layouts).unwrap();
        if bless {
            std::fs::create_dir_all(dir()).unwrap();
            std::fs::write(&path, &text).unwrap();
        } else if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            stale.push(fixture.name);
        }
    }
    assert!(
        stale.is_empty(),
        "tests/masks/ is out of date for {stale:?}: run with HELIOS_BLESS=1"
    );
}

#[test]
fn every_fixture_imports_with_its_mask() {
    let layouts = Layouts::built_in();
    for fixture in fixtures() {
        let text = fixture.sidecar(&layouts).unwrap();
        let sidecar = Sidecar::parse(&text).unwrap();
        let imported = import_with(&sidecar, &layouts, Some(&Frame::new(3000.0, 2000.0)));
        let exposure = imported
            .nodes
            .iter()
            .find(|n| n.operation == "exposure")
            .unwrap_or_else(|| panic!("{}: {:#?}", fixture.name, imported.report));
        assert!(exposure.mask.is_some(), "{}", fixture.name);
    }
}
