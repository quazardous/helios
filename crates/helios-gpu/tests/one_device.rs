//! One wgpu instance and one device a process (see `helios_gpu::shared`):
//! asked twice, the device is the same; no source file but `shared.rs`
//! creates an instance, and none but `lib.rs` opens a device.

use std::path::{Path, PathBuf};
use std::sync::Arc;

#[test]
fn the_process_has_one_instance_and_one_device() {
    let _gpu = helios_testkit::gpu_lock();
    let first = match helios_gpu::shared::gpu() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let again = helios_gpu::shared::gpu().unwrap();
    assert!(Arc::ptr_eq(&first, &again));
    // Whatever another caller would choose, the device open is the answer.
    let chosen = helios_gpu::shared::gpu_choosing(|_| None).unwrap();
    assert!(Arc::ptr_eq(&first, &chosen));
    assert_eq!(helios_gpu::shared::instances_created(), 1);
    assert_eq!(helios_gpu::shared::devices_opened(), 1);
    assert!(helios_gpu::shared::owns(&first.handles().instance));
    // Another adapter is refused, never opened beside it.
    let (index, _) = helios_gpu::shared::opened().unwrap();
    let others = helios_gpu::shared::adapters().len();
    if let Some(other) = (0..others).find(|i| *i != index) {
        let err = helios_gpu::shared::gpu_on(other).err().unwrap();
        assert!(err.contains("one device a process"), "{err}");
        assert_eq!(helios_gpu::shared::devices_opened(), 1);
    }
}

/// Every `.rs` file under `dir`, `target` directories left out.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "target") {
                sources(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_other_source_creates_an_instance_or_opens_a_device() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["crates", "modules", "plugins", "xtask"] {
        sources(&root.join(dir), &mut files);
    }
    // Split so that this file does not match itself.
    let rules: [(String, &str); 5] = [
        (
            ["Instance", "::new("].concat(),
            "crates/helios-gpu/src/shared.rs",
        ),
        (
            ["Instance", "::default("].concat(),
            "crates/helios-gpu/src/shared.rs",
        ),
        (
            ["request", "_device("].concat(),
            "crates/helios-gpu/src/lib.rs",
        ),
        // eframe and egui_kittest open their own instance and device unless
        // handed the process's (`WgpuSetup::Existing`).
        (["WgpuSetup", "::CreateNew"].concat(), ""),
        (["default", "_wgpu_setup("].concat(), ""),
    ];
    let mut found = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        let shown = file.strip_prefix(&root).unwrap_or(file);
        let shown = shown.to_string_lossy().replace('\\', "/");
        for (pattern, allowed) in &rules {
            if text.contains(pattern.as_str()) && shown != *allowed {
                found.push(format!("{shown}: {pattern}"));
            }
        }
    }
    assert!(
        found.is_empty(),
        "wgpu instances and devices come from helios_gpu::shared only:\n{}",
        found.join("\n")
    );
}
