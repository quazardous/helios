//! Every WGSL shader in the workspace parses and validates with naga, the
//! shader compiler wgpu uses: a broken shader fails `cargo test`, even on a
//! machine without a GPU, where the backend's own tests skip.
//!
//! A shader that is put after another one at run time says so on a line
//! `// wgsl-prelude: <file>` (a path next to it), and is validated with
//! that file in front, as it is compiled.
//!
//! Validating is not compiling everywhere: on Windows, wgpu's DX12 backend
//! hands naga's HLSL to FXC, which refuses a write into a local vector at
//! an index known only at run time (`v[i] = x`, error X3500, "not natively
//! addressable"). Such a kernel fails there and its step runs on the CPU,
//! while Vulkan and Metal take it. Every shader is checked for that write
//! here, where FXC is not at hand.

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
        let mut source = std::fs::read_to_string(path).expect("a readable shader");
        let preludes: Vec<String> = source
            .lines()
            .filter_map(|l| l.trim().strip_prefix("// wgsl-prelude:"))
            .map(|f| f.trim().to_string())
            .collect();
        for prelude in preludes.iter().rev() {
            let file = path.with_file_name(prelude);
            let text = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("{}: prelude {prelude}: {e}", path.display()));
            source = format!("{text}\n{source}");
        }
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
        let functions = module
            .functions
            .iter()
            .map(|(_, f)| f)
            .chain(module.entry_points.iter().map(|e| &e.function));
        for function in functions {
            let mut writes = 0;
            dynamic_vector_writes(&module, function, &function.body, &mut writes);
            if writes > 0 {
                failures.push(format!(
                    "{name}: fn {}: {writes} write(s) into a local vector at a run-time \
                     index, which DX12's FXC refuses (X3500): choose the value per \
                     component (select) instead",
                    function.name.as_deref().unwrap_or("?")
                ));
            }
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

/// Counts in `block` the stores into a component of a function's local
/// vector at a run-time index (`Access`; a constant index is `AccessIndex`).
fn dynamic_vector_writes(
    module: &naga::Module,
    function: &naga::Function,
    block: &naga::Block,
    writes: &mut usize,
) {
    use naga::{Expression, Statement, TypeInner};
    for statement in block.iter() {
        match statement {
            Statement::Store { pointer, .. } => {
                if let Expression::Access { base, .. } = function.expressions[*pointer]
                    && let Expression::LocalVariable(local) = function.expressions[base]
                {
                    let ty = function.local_variables[local].ty;
                    if matches!(module.types[ty].inner, TypeInner::Vector { .. }) {
                        *writes += 1;
                    }
                }
            }
            Statement::Block(inner) => dynamic_vector_writes(module, function, inner, writes),
            Statement::If { accept, reject, .. } => {
                dynamic_vector_writes(module, function, accept, writes);
                dynamic_vector_writes(module, function, reject, writes);
            }
            Statement::Switch { cases, .. } => {
                for case in cases {
                    dynamic_vector_writes(module, function, &case.body, writes);
                }
            }
            Statement::Loop {
                body, continuing, ..
            } => {
                dynamic_vector_writes(module, function, body, writes);
                dynamic_vector_writes(module, function, continuing, writes);
            }
            _ => {}
        }
    }
}
