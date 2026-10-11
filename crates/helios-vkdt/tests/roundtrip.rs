//! Every module of a vkdt checkout goes to a Helios manifest and back to
//! the same files. The checkout comes from `VKDT_SRC`; without one the test
//! is skipped, saying so (vkdt's files are read, never copied into Helios).

use std::path::PathBuf;

use helios_vkdt::{Files, read, write};

/// Non-blank lines, fields split, numbers compared by value.
fn normalized(text: Option<&str>) -> Vec<Vec<String>> {
    text.unwrap_or("")
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            l.split(':')
                .map(|f| match f.parse::<f64>() {
                    Ok(v) if !f.is_empty() => format!("{v}"),
                    _ => f.to_string(),
                })
                .collect()
        })
        .collect()
}

#[test]
fn every_vkdt_module_round_trips() {
    let Some(src) = std::env::var_os("VKDT_SRC").map(PathBuf::from) else {
        eprintln!("skipped: set VKDT_SRC to a vkdt checkout");
        return;
    };
    let modules = src.join("src/pipe/modules");
    let mut count = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(&modules).unwrap() {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let read_file = |f: &str| std::fs::read_to_string(dir.join(f)).ok();
        let files = Files {
            params: read_file("params"),
            connectors: read_file("connectors"),
            params_ui: read_file("params.ui"),
        };
        if files == Files::default() {
            continue;
        }
        let module = match read(&name, &files) {
            Ok(m) => m,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        let back = write(&module);
        for (file, a, b) in [
            ("params", &files.params, &back.params),
            ("connectors", &files.connectors, &back.connectors),
            ("params.ui", &files.params_ui, &back.params_ui),
        ] {
            if normalized(a.as_deref()) != normalized(b.as_deref()) {
                failures.push(format!("{name}/{file} differs:\n{a:?}\n{b:?}"));
            }
        }
        count += 1;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    assert!(count > 90, "{count} modules");
    eprintln!("{count} modules round-trip");
}

#[test]
fn a_module_reads_into_a_manifest() {
    let files = Files {
        params: Some("exposure:float:1:0.0\n".into()),
        connectors: Some(
            "input:read:rgba:f16\noutput:write:&input:*\nhist:write:ssbo:f32\n".into(),
        ),
        params_ui: Some("exposure:slider:-10:10\n".into()),
    };
    let m = read("exposure", &files).unwrap();
    let p = &m.manifest.params[0];
    assert_eq!(
        p.kind,
        helios_core::ParamKind::Float {
            min: -10.0,
            max: 10.0,
            default: 0.0
        },
        "the slider's range is the parameter's"
    );
    assert_eq!(m.manifest.inputs[0].channels(), Some(4));
    assert_eq!(m.manifest.inputs[0].layout.as_deref(), Some("rgba"));
    assert_eq!(m.manifest.outputs.len(), 2);
    assert_eq!(
        m.manifest.outputs[0].channels(),
        None,
        "same as input: not fixed"
    );
    let back = write(&m);
    assert_eq!(back.connectors, files.connectors);
    assert_eq!(back.params_ui, files.params_ui);
    assert_eq!(
        normalized(back.params.as_deref()),
        normalized(files.params.as_deref()),
        "`0.0` comes back as `0`: the same value"
    );
    assert!(
        read(
            "x",
            &Files {
                params: Some("a:float".into()),
                ..Files::default()
            }
        )
        .is_err()
    );
}
