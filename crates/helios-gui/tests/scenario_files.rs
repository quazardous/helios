//! Helios's scenarios (`tests/scenarios/*.ron`) are run live, against a
//! running helios-gui in a headless compositor, by the box's `scenarios`
//! job (tools/bench/scenarios.py). Here, each file reads as a scenario
//! and names only Helios's commands, so a typo fails in seconds rather
//! than on the box.

use std::path::PathBuf;

use egui_pop_test::scenario::{Runner, Step};
use helios_gui::{Shell, helios_panels};

fn files() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/scenarios");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ron"))
        .collect();
    files.sort();
    files
}

#[test]
fn every_scenario_reads_and_calls_known_commands() {
    let files = files();
    assert!(files.len() >= 3, "{files:?}");
    let shell = Shell::new(helios_panels(), None);
    let caps = shell.app().handle().debug_capabilities();
    let known = |name: &str| {
        caps["capabilities"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c["name"] == name))
    };
    for f in files {
        let s = Runner::load(&f).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            f.file_stem().unwrap().to_string_lossy(),
            s.name,
            "a scenario is named after its file"
        );
        for step in &s.steps {
            if let Step::Call(name, _) = step {
                assert!(known(name), "{}: no capability `{name}`", s.name);
            }
        }
    }
}
