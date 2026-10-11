//! Every WGSL shader in the workspace parses and validates with naga, the
//! shader compiler wgpu uses: a broken shader fails `cargo test`, even on a
//! machine without a GPU, where the backend's own tests skip.

use std::path::{Path, PathBuf};

/// The `.wgsl` files under `dir`, build outputs left out.
fn shaders(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "target") {
                shaders(&path, found);
            }
        } else if path.extension().is_some_and(|e| e == "wgsl") {
            found.push(path);
        }
    }
}

#[test]
fn every_shader_validates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut found = Vec::new();
    // The GUI prototypes (proto/) build on their own and are left out.
    for dir in ["crates", "modules"] {
        shaders(&root.join(dir), &mut found);
    }
    found.sort();
    assert!(!found.is_empty(), "no WGSL shader found under {root:?}");

    let mut failures = Vec::new();
    for path in &found {
        let source = std::fs::read_to_string(path).expect("a readable shader");
        let name = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let module = match naga::front::wgsl::parse_str(&source) {
            Ok(module) => module,
            Err(e) => {
                failures.push(e.emit_to_string_with_path(&source, &name));
                continue;
            }
        };
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        );
        if let Err(e) = validator.validate(&module) {
            failures.push(e.emit_to_string_with_path(&source, &name));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} shaders do not validate:\n{}",
        failures.len(),
        found.len(),
        failures.join("\n")
    );
}
