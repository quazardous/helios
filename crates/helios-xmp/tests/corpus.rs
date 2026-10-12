//! Real darktable sidecars from darktable 3.1 to 5.7
//! (`tests/darktable-sidecars`, where they come from is in its README):
//! what the import keeps of each. The pipe orders below are darktable's
//! own, as darktable-cli runs these sidecars (checked by
//! `helios-parity/tests/corpus.rs` where darktable-cli is installed).

use std::path::Path;

use helios_xmp::{Layouts, Reason, import_text};

struct Case {
    name: &'static str,
    /// darktable's order the import names.
    order: &'static str,
    history_end: u32,
    /// Items in force, in pipe order.
    pipe: &'static [&'static str],
    /// Helios nodes, in pipe order.
    nodes: &'static [&'static str],
    /// Items kept, not applied, with the kind of reason.
    not_applied: &'static [(&'static str, &'static str)],
    disabled: &'static [&'static str],
    inserted: &'static [&'static str],
}

const UNKNOWN: &str = "unknown_module";
const BLENDING: &str = "blending";

const CASES: &[Case] = &[
    Case {
        name: "0001-exposure",
        order: "legacy",
        history_end: 9,
        pipe: &[
            "rawprepare v1",
            "temperature v3",
            "highlights v2",
            "demosaic v3",
            "exposure v5",
            "flip v2",
            "colorin v6",
            "colorout v5",
            "gamma v1",
        ],
        nodes: &[
            "rawprepare",
            "temperature",
            "highlights",
            "demosaic",
            "exposure",
            "colorin",
        ],
        not_applied: &[
            ("flip v2", UNKNOWN),
            ("colorout v5", UNKNOWN),
            ("gamma v1", UNKNOWN),
        ],
        disabled: &[],
        inserted: &[],
    },
    Case {
        name: "0004-masks",
        order: "v3.0 RAW",
        // 10 in the file, and darktable's clip highlights.
        history_end: 11,
        pipe: &[
            "rawprepare v1",
            "temperature v3",
            "highlights v4",
            "demosaic v3",
            "flip v2",
            "exposure v5",
            "mask_manager v2",
            "colorin v6",
            "colorout v5",
            "gamma v1",
        ],
        nodes: &[
            "rawprepare",
            "temperature",
            "highlights",
            "demosaic",
            "colorin",
        ],
        not_applied: &[
            ("flip v2", UNKNOWN),
            // Edited twice; the second time with a drawn mask.
            ("exposure v5", BLENDING),
            ("mask_manager v2", UNKNOWN),
            ("colorout v5", UNKNOWN),
            ("gamma v1", UNKNOWN),
        ],
        disabled: &[],
        inserted: &["highlights v4"],
    },
    Case {
        name: "0033-blending-modes-uniform",
        // The preset named; the sidecar's own list orders it.
        order: "legacy",
        history_end: 15,
        pipe: &[
            "rawprepare v1",
            "temperature v3",
            "highlights v4",
            "demosaic v3",
            "exposure v6",
            "exposure 1 v6",
            "exposure 2 v6",
            "exposure 3 v6",
            "exposure 4 v6",
            "flip v2",
            "colorin v6",
            "colorbalance v3",
            "colorbalance 1 v3",
            "colorout v5",
            "gamma v1",
        ],
        nodes: &[
            "rawprepare",
            "temperature",
            "highlights",
            "demosaic",
            "exposure",
            "colorin",
        ],
        not_applied: &[
            ("exposure 1 v6", BLENDING),
            ("exposure 2 v6", BLENDING),
            ("exposure 3 v6", BLENDING),
            ("exposure 4 v6", BLENDING),
            ("flip v2", UNKNOWN),
            ("colorbalance v3", UNKNOWN),
            ("colorbalance 1 v3", UNKNOWN),
            ("colorout v5", UNKNOWN),
            ("gamma v1", UNKNOWN),
        ],
        disabled: &[],
        inserted: &["highlights v4"],
    },
    Case {
        name: "0035-multiple-modules",
        order: "v3.0 RAW",
        history_end: 20,
        pipe: &[
            "rawprepare v1",
            "temperature v3",
            "highlights v2",
            "demosaic v3",
            "denoiseprofile v10",
            "flip v2",
            "clipping v5",
            "liquify v1",
            "retouch v1",
            "exposure v6",
            "mask_manager v2",
            "colorin v6",
            "sharpen v1",
            "colorbalance v3",
            "rgbcurve v1",
            "filmicrgb v4",
            "bilat v3",
            "colorout v5",
            "borders v3",
            "gamma v1",
        ],
        nodes: &[
            "rawprepare",
            "temperature",
            "highlights",
            "demosaic",
            "exposure",
            "colorin",
        ],
        not_applied: &[
            ("denoiseprofile v10", UNKNOWN),
            ("flip v2", UNKNOWN),
            ("clipping v5", UNKNOWN),
            ("liquify v1", UNKNOWN),
            ("retouch v1", UNKNOWN),
            ("sharpen v1", UNKNOWN),
            ("colorbalance v3", UNKNOWN),
            ("rgbcurve v1", UNKNOWN),
            ("filmicrgb v4", UNKNOWN),
            ("bilat v3", UNKNOWN),
            ("colorout v5", UNKNOWN),
            ("borders v3", UNKNOWN),
            ("gamma v1", UNKNOWN),
        ],
        disabled: &["mask_manager"],
        inserted: &[],
    },
    Case {
        name: "0097-sigmoid",
        order: "v3.0 RAW",
        history_end: 15,
        pipe: &[
            "rawprepare v2",
            "temperature v3",
            "highlights v4",
            "demosaic v4",
            "denoiseprofile v11",
            "lens v6",
            "flip v2",
            "exposure v6",
            "colorin v7",
            "channelmixerrgb v3",
            "sigmoid v1",
            "filmicrgb v5",
            "colorout v5",
            "gamma v1",
        ],
        nodes: &[
            "rawprepare",
            "temperature",
            "highlights",
            "demosaic",
            "exposure",
            "colorin",
            "sigmoid",
        ],
        not_applied: &[
            ("denoiseprofile v11", UNKNOWN),
            ("lens v6", UNKNOWN),
            ("flip v2", UNKNOWN),
            ("channelmixerrgb v3", UNKNOWN),
            ("colorout v5", UNKNOWN),
            ("gamma v1", UNKNOWN),
        ],
        disabled: &["filmicrgb"],
        inserted: &[],
    },
    Case {
        name: "0151-sigmoid-no-hue-preservation",
        order: "v3.0 RAW",
        history_end: 16,
        pipe: &[
            "rawprepare v2",
            "temperature v3",
            "highlights v4",
            "demosaic v4",
            "denoiseprofile v11",
            "lens v9",
            "flip v2",
            "exposure v6",
            "colorin v7",
            "channelmixerrgb v3",
            "sigmoid v1",
            "filmicrgb v6",
            "colorout v5",
            "gamma v1",
        ],
        nodes: &[
            "rawprepare",
            "temperature",
            "highlights",
            "demosaic",
            "exposure",
            "colorin",
            "sigmoid",
        ],
        not_applied: &[
            ("denoiseprofile v11", UNKNOWN),
            ("lens v9", UNKNOWN),
            ("flip v2", UNKNOWN),
            ("channelmixerrgb v3", UNKNOWN),
            ("colorout v5", UNKNOWN),
            ("gamma v1", UNKNOWN),
        ],
        disabled: &["filmicrgb"],
        inserted: &[],
    },
    Case {
        name: "0180-spektrafilm-simple",
        order: "v5.0 RAW",
        history_end: 12,
        pipe: &[
            "rawprepare v2",
            "temperature v4",
            "highlights v5",
            "demosaic v7",
            "flip v2",
            "exposure v7",
            "colorin v7",
            "channelmixerrgb v3",
            "sigmoid v3",
            "spektrafilm v3",
            "colorout v5",
            "gamma v1",
        ],
        nodes: &[
            "rawprepare",
            "temperature",
            "highlights",
            "demosaic",
            "exposure",
            "colorin",
        ],
        not_applied: &[
            ("flip v2", UNKNOWN),
            ("channelmixerrgb v3", UNKNOWN),
            ("spektrafilm v3", UNKNOWN),
            ("colorout v5", UNKNOWN),
            ("gamma v1", UNKNOWN),
        ],
        disabled: &["sigmoid"],
        inserted: &[],
    },
];

fn read(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/darktable-sidecars")
        .join(format!("{name}.xmp"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn kind(reason: &Reason) -> &'static str {
    match reason {
        Reason::UnknownModule => UNKNOWN,
        Reason::Blending { .. } => BLENDING,
        Reason::UnsupportedVersion { .. } => "unsupported_version",
        Reason::Unreadable { .. } => "unreadable",
        Reason::UnknownBlendVersion { .. } => "unknown_blend_version",
        Reason::ExtraInstance => "extra_instance",
    }
}

#[test]
fn every_era_imports_its_history_order_and_modules() {
    let layouts = Layouts::built_in();
    for case in CASES {
        let name = case.name;
        let (sidecar, imported) = import_text(&read(name), &layouts).unwrap();
        let r = &imported.report;
        assert_eq!(r.pipe_order, case.order, "{name}");
        assert_eq!(sidecar.history_end, Some(case.history_end), "{name}");
        let pipe: Vec<String> = sidecar
            .in_pipe_order()
            .items
            .iter()
            .map(|i| i.label())
            .collect();
        assert_eq!(pipe, case.pipe, "{name}");
        let nodes: Vec<&str> = imported.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(nodes, case.nodes, "{name}: {r:#?}");
        let not: Vec<(&str, &str)> = r
            .not_applied
            .iter()
            .map(|n| (n.label.as_str(), kind(&n.reason)))
            .collect();
        assert_eq!(not, case.not_applied, "{name}");
        assert_eq!(r.disabled, case.disabled, "{name}");
        assert_eq!(r.inserted, case.inserted, "{name}");
        assert!(r.unordered.is_empty(), "{name}: {:?}", r.unordered);
        // Nothing is lost: every item of the file is kept, as written.
        let kept = imported.darktable["history"].as_array().unwrap();
        let from_file = kept.iter().filter(|i| i["inserted"] != true).count();
        assert_eq!(
            from_file + r.inserted.len(),
            sidecar.history.len(),
            "{name}"
        );
        assert!(imported.report.summary().is_some(), "{name}");
    }
}

#[test]
fn old_values_arrive_migrated() {
    let layouts = Layouts::built_in();
    let value = |name: &str, node: &str, param: &str| -> f64 {
        let (_, imported) = import_text(&read(name), &layouts).unwrap();
        let node = imported.nodes.iter().find(|n| n.name == node).unwrap();
        match node.params.get(param) {
            Some(helios_core::Value::Float(v)) => *v,
            other => panic!("{name} {param}: {other:?}"),
        }
    };
    // Exposure v5 (darktable 3.1) and v6 to today's v7.
    assert_eq!(value("0001-exposure", "exposure", "exposure"), 1.0);
    assert!((value("0097-sigmoid", "exposure", "exposure") - 0.7).abs() < 1e-6);
    // Sigmoid v1 (darktable 4.2) to today's v3, its values kept.
    assert!((value("0097-sigmoid", "sigmoid", "contrast") - 1.788).abs() < 1e-5);
    assert!((value("0097-sigmoid", "sigmoid", "preserve_hue") - 50.62).abs() < 1e-4);
    assert_eq!(
        value(
            "0151-sigmoid-no-hue-preservation",
            "sigmoid",
            "preserve_hue"
        ),
        0.0
    );
    // White balance v3 to v4.
    assert!(value("0001-exposure", "temperature", "red") > 2.0);
}
