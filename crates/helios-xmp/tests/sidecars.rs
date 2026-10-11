//! Real darktable sidecars, from darktable's public integration tests (the
//! `src/tests/integration` submodule of a darktable checkout, or
//! `HELIOS_DT_TESTS`). Skipped, saying so, when they are not there.

use std::path::{Path, PathBuf};

use helios_core::Value as Param;
use helios_xmp::sidecar::decode_params;
use helios_xmp::{Layouts, Sidecar, decode, import};

fn tests_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("HELIOS_DT_TESTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../darktable/src/tests/integration")
        });
    if dir.join("0001-exposure/exposure.xmp").is_file() {
        Some(dir)
    } else {
        eprintln!(
            "skipped: no darktable integration tests in {}",
            dir.display()
        );
        None
    }
}

fn read(dir: &Path, path: &str) -> Sidecar {
    Sidecar::parse(&std::fs::read_to_string(dir.join(path)).unwrap()).unwrap()
}

#[test]
fn params_decode_from_hex_and_from_compressed_text() {
    assert_eq!(decode_params("00ff10").unwrap(), [0, 255, 16]);
    assert!(decode_params("0g").unwrap_err().contains("not hex"));
    // darktable's own compressed blend params from a test sidecar.
    let gz = decode_params("gz11eJxjYGBgkGAAgRNODGiAEV0AJ2iwh+CRyscOAAdeGQQ=").unwrap();
    assert!(gz.len() > 100, "{} bytes", gz.len());
}

#[test]
fn the_built_in_layouts_cover_the_modules_helios_maps() {
    let layouts = Layouts::built_in();
    assert!(layouts.len() > 90, "{} modules", layouts.len());
    for (module, version) in [
        ("rawprepare", 2),
        ("temperature", 4),
        ("highlights", 5),
        ("demosaic", 7),
        ("colorin", 7),
        ("exposure", 7),
        ("sigmoid", 3),
    ] {
        assert!(
            layouts.get(module, version).is_some(),
            "{module} v{version}"
        );
    }
}

#[test]
fn a_current_sidecar_imports_with_its_values_and_reports_the_rest() {
    let Some(dir) = tests_dir() else { return };
    let sidecar = read(&dir, "0180-spektrafilm-simple/spektrafilm-simple.xmp");
    assert_eq!(sidecar.derived_from.as_deref(), Some("hlrecovery.arw"));
    assert_eq!(sidecar.history_end, Some(12));
    let layouts = Layouts::built_in();

    // Exposure's raw params decode to darktable's own field names.
    let item = sidecar
        .effective()
        .into_iter()
        .find(|i| i.operation == "exposure")
        .unwrap();
    let fields = decode(layouts.get("exposure", 7).unwrap(), &item.params).unwrap();
    assert!(fields["exposure"].is_number() && fields["compensate_exposure_bias"].is_boolean());
    assert_eq!(fields["mode"], "EXPOSURE_MODE_MANUAL");

    let imported = import(&sidecar, &layouts);
    let modules: Vec<_> = imported.nodes.iter().map(|n| n.module.as_str()).collect();
    assert_eq!(
        modules,
        [
            "rawprepare",
            "demosaic",
            "colorin",
            "whitebalance",
            "highlights",
            "exposure"
        ],
        "{:#?}",
        imported.report
    );
    let exposure = imported
        .nodes
        .iter()
        .find(|n| n.module == "exposure")
        .unwrap();
    assert_eq!(
        exposure.params.get("exposure"),
        Some(&Param::Float(fields["exposure"].as_f64().unwrap()))
    );
    let report = &imported.report;
    // What Helios has no module for is named, as is the disabled sigmoid.
    for missing in [
        "colorout",
        "gamma",
        "channelmixerrgb",
        "flip",
        "spektrafilm",
    ] {
        assert!(
            report.unknown_modules.iter().any(|m| m == missing),
            "{missing}: {report:#?}"
        );
    }
    assert_eq!(report.disabled, ["sigmoid"]);
    assert!(report.unsupported_versions.is_empty(), "{report:#?}");
}

#[test]
fn an_old_sidecar_names_the_versions_to_migrate() {
    let Some(dir) = tests_dir() else { return };
    let sidecar = read(&dir, "0001-exposure/exposure.xmp");
    let imported = import(&sidecar, &Layouts::built_in());
    let old: Vec<_> = imported
        .report
        .unsupported_versions
        .iter()
        .map(|(op, v, _)| (op.as_str(), *v))
        .collect();
    for expected in [
        ("rawprepare", 1),
        ("temperature", 3),
        ("highlights", 2),
        ("demosaic", 3),
    ] {
        assert!(old.contains(&expected), "{expected:?} in {old:?}");
    }
}

#[test]
fn every_test_sidecar_reads_and_nothing_is_lost_silently() {
    let Some(dir) = tests_dir() else { return };
    let layouts = Layouts::built_in();
    let mut sidecars = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let test = entry.unwrap().path();
        for file in std::fs::read_dir(&test).into_iter().flatten() {
            let path = file.unwrap().path();
            if path.extension().is_some_and(|e| e == "xmp") {
                let text = std::fs::read_to_string(&path).unwrap();
                let sidecar =
                    Sidecar::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                let imported = import(&sidecar, &layouts);
                // Every effective item is accounted for: a node or a line of the report.
                let r = &imported.report;
                let accounted = imported.nodes.len()
                    + r.unknown_modules.len()
                    + r.unsupported_versions.len()
                    + r.undecodable.len()
                    + r.disabled.len();
                assert_eq!(accounted, sidecar.effective().len(), "{}", path.display());
                assert!(
                    r.undecodable.is_empty(),
                    "{}: {:?}",
                    path.display(),
                    r.undecodable
                );
                sidecars += 1;
            }
        }
    }
    assert!(sidecars > 150, "{sidecars} sidecars");
}
