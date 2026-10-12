//! darktable's pipe order: sidecars whose history order is not the order
//! darktable runs their modules in.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use helios_core::Value as Param;
use helios_xmp::iop_order::{self, MIGRATIONS, Preset};
use helios_xmp::{Layouts, Sidecar, encode, import};
use serde_json::{Value, json};

/// One history item: operation, instance, values of its params.
struct Item(&'static str, u32, Value);

/// A darktable sidecar with `items` as its history, in that order, and
/// `attrs` (the order attributes) on its description.
fn sidecar(items: &[Item], attrs: &str) -> Sidecar {
    let layouts = Layouts::built_in();
    let mut lines = String::new();
    for (num, Item(op, instance, values)) in items.iter().enumerate() {
        let (params, version) = match layouts.versions(op).iter().max() {
            Some(&v) => (
                encode(layouts.get(op, v).unwrap(), values)
                    .unwrap()
                    .iter()
                    .fold(String::new(), |mut hex, b| {
                        let _ = write!(hex, "{b:02x}");
                        hex
                    }),
                v,
            ),
            None => ("00".to_string(), 1),
        };
        let _ = writeln!(
            lines,
            r#"<rdf:li darktable:num="{num}" darktable:operation="{op}" darktable:enabled="1" darktable:modversion="{version}" darktable:params="{params}" darktable:multi_name="{}" darktable:multi_priority="{instance}"/>"#,
            if *instance == 0 {
                String::new()
            } else {
                instance.to_string()
            }
        );
    }
    let xml = format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/"
   darktable:history_end="{}" {attrs}>
   <darktable:history><rdf:Seq>
{lines}   </rdf:Seq></darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#,
        items.len()
    );
    Sidecar::parse(&xml).unwrap()
}

const V50: &str = r#"darktable:xmp_version="5" darktable:iop_order_version="4""#;

/// The order of a sidecar's effective items, as `operation instance`.
fn order(s: &Sidecar) -> Vec<String> {
    s.in_pipe_order()
        .items
        .iter()
        .map(|i| format!("{} {}", i.operation, i.multi_priority))
        .collect()
}

/// darktable's v5.0 list, with `edit` applied to it, as a sidecar spells it.
fn list(edit: impl FnOnce(&mut Vec<(String, u32)>)) -> String {
    let mut entries: Vec<(String, u32)> = Preset::V50
        .table()
        .iter()
        .map(|(_, op)| (op.to_string(), 0))
        .collect();
    edit(&mut entries);
    entries
        .iter()
        .map(|(op, i)| format!("{op},{i}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[test]
fn a_module_edited_again_keeps_its_place_in_the_pipe() {
    // Exposure, then sigmoid, then exposure again: the history ends with
    // exposure, but darktable runs it before the tone mapper.
    let s = sidecar(
        &[
            Item("exposure", 0, json!({"exposure": 0.5})),
            Item("sigmoid", 0, json!({"middle_grey_contrast": 1.5})),
            Item("exposure", 0, json!({"exposure": 1.0})),
        ],
        V50,
    );
    let effective: Vec<_> = s.effective().iter().map(|i| i.operation.clone()).collect();
    assert_eq!(effective, ["sigmoid", "exposure"]);
    assert_eq!(order(&s), ["exposure 0", "sigmoid 0"]);

    let imported = import(&s, &Layouts::built_in());
    let modules: Vec<_> = imported.nodes.iter().map(|n| n.module.as_str()).collect();
    assert_eq!(modules, ["exposure", "sigmoid"]);
    assert_eq!(
        imported.nodes[0].params.get("exposure"),
        Some(&Param::Float(1.0)),
        "the last edit is the one in force"
    );
    assert_eq!(imported.report.pipe_order, "v5.0 RAW");
    assert!(imported.report.unordered.is_empty());
    let edit = imported.edit();
    assert_eq!(edit["connections"][0]["to"], "exposure");
    assert_eq!(edit["connections"][1]["from"], "exposure");
    assert_eq!(edit["connections"][1]["to"], "sigmoid");
}

#[test]
fn without_a_version_darktable_takes_its_legacy_order() {
    // The legacy order runs exposure before the lens correction, v5.0 after.
    let items = [Item("lens", 0, json!({})), Item("exposure", 0, json!({}))];
    let legacy = sidecar(&items, r#"darktable:xmp_version="5""#);
    assert_eq!(order(&legacy), ["exposure 0", "lens 0"]);
    assert_eq!(legacy.in_pipe_order().order.source, "legacy");
    let v50 = sidecar(&items, V50);
    assert_eq!(order(&v50), ["lens 0", "exposure 0"]);
    let v30_jpeg = sidecar(
        &[
            Item("colorin", 0, json!({})),
            Item("exposure", 0, json!({})),
        ],
        r#"darktable:xmp_version="5" darktable:iop_order_version="3""#,
    );
    // For non-linear input, v3.0 JPEG converts to the working space first.
    assert_eq!(order(&v30_jpeg), ["colorin 0", "exposure 0"]);
}

#[test]
fn multiple_instances_follow_the_sidecars_list() {
    // Instance 1 was added and edited first; the list runs it after 0.
    let after = list(|l| {
        let at = l.iter().position(|(op, _)| op == "exposure").unwrap();
        l.insert(at + 1, ("exposure".into(), 1));
    });
    let items = [
        Item("exposure", 1, json!({"exposure": 1.0})),
        Item("exposure", 0, json!({"exposure": 2.0})),
    ];
    let s = sidecar(
        &items,
        &format!(r#"{V50} darktable:iop_order_list="{after}""#),
    );
    assert_eq!(order(&s), ["exposure 0", "exposure 1"]);
    // The same history with the other order.
    let before = list(|l| {
        let at = l.iter().position(|(op, _)| op == "exposure").unwrap();
        l.insert(at, ("exposure".into(), 1));
    });
    let s = sidecar(
        &items,
        &format!(r#"{V50} darktable:iop_order_list="{before}""#),
    );
    assert_eq!(order(&s), ["exposure 1", "exposure 0"]);
    let imported = import(&s, &Layouts::built_in());
    let names: Vec<_> = imported.edit()["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["exposure 1", "exposure"]);
}

#[test]
fn an_instance_the_list_does_not_place_goes_after_the_one_below_it() {
    // A preset names no instance but 0: darktable writes a list as soon as
    // there are several. Without one, they follow their instance number.
    let s = sidecar(
        &[
            Item("exposure", 2, json!({})),
            Item("exposure", 1, json!({})),
            Item("sigmoid", 0, json!({})),
            Item("exposure", 0, json!({})),
        ],
        V50,
    );
    assert_eq!(
        order(&s),
        ["exposure 0", "exposure 1", "exposure 2", "sigmoid 0"]
    );
    let report = import(&s, &Layouts::built_in()).report;
    assert_eq!(
        report.unordered,
        [("exposure".to_string(), 2), ("exposure".to_string(), 1)]
    );
}

#[test]
fn a_custom_order_is_followed() {
    // A user moved sigmoid before exposure.
    let custom = list(|l| {
        let sigmoid = l.iter().position(|(op, _)| op == "sigmoid").unwrap();
        let moved = l.remove(sigmoid);
        let at = l.iter().position(|(op, _)| op == "exposure").unwrap();
        l.insert(at, moved);
    });
    let s = sidecar(
        &[
            Item("exposure", 0, json!({})),
            Item("sigmoid", 0, json!({})),
        ],
        &format!(
            r#"darktable:xmp_version="5" darktable:iop_order_version="0" darktable:iop_order_list="{custom}""#
        ),
    );
    assert_eq!(order(&s), ["sigmoid 0", "exposure 0"]);
    assert_eq!(s.in_pipe_order().order.source, "custom");
}

#[test]
fn an_older_custom_list_gains_the_modules_that_came_after_it() {
    // A list from before sigmoid existed: darktable puts it before filmicrgb.
    let old = list(|l| l.retain(|(op, _)| op != "sigmoid" && op != "agx"));
    let s = sidecar(
        &[
            Item("filmicrgb", 0, json!({})),
            Item("sigmoid", 0, json!({})),
            Item("colorbalancergb", 0, json!({})),
        ],
        &format!(
            r#"darktable:xmp_version="5" darktable:iop_order_version="0" darktable:iop_order_list="{old}""#
        ),
    );
    assert_eq!(order(&s), ["colorbalancergb 0", "sigmoid 0", "filmicrgb 0"]);
    assert!(s.in_pipe_order().unordered.is_empty());
}

#[test]
fn a_broken_list_falls_back_to_the_preset_and_says_so() {
    let s = sidecar(
        &[Item("exposure", 0, json!({}))],
        r#"darktable:xmp_version="5" darktable:iop_order_version="0" darktable:iop_order_list="exposure,0,colorin,0""#,
    );
    let source = s.in_pipe_order().order.source;
    assert!(
        source.starts_with("v5.0 RAW (iop_order_list refused"),
        "{source}"
    );
    assert!(iop_order::parse_list("rawprepare,0,gamma").is_err());
    assert!(iop_order::parse_list("rawprepare,x,gamma,0").is_err());
}

#[test]
fn an_unknown_module_is_reported_and_runs_last() {
    let s = sidecar(
        &[
            Item("not_a_module", 0, json!({})),
            Item("exposure", 0, json!({})),
        ],
        V50,
    );
    assert_eq!(order(&s), ["exposure 0", "not_a_module 0"]);
    let report = import(&s, &Layouts::built_in()).report;
    assert_eq!(report.unordered, [("not_a_module".to_string(), 0)]);
    assert_eq!(report.unknown_modules, ["not_a_module"]);
}

#[test]
fn a_module_newer_than_a_custom_list_takes_its_rank_in_the_preset() {
    // A list with neither sigmoid nor its anchor: sigmoid is placed after
    // the modules v5.0 runs before it.
    let without = list(|l| l.retain(|(op, _)| op != "sigmoid" && op != "filmicrgb"));
    let s = sidecar(
        &[
            Item("sigmoid", 0, json!({})),
            Item("colorbalancergb", 0, json!({})),
            Item("tonecurve", 0, json!({})),
        ],
        &format!(
            r#"darktable:xmp_version="5" darktable:iop_order_version="0" darktable:iop_order_list="{without}""#
        ),
    );
    assert_eq!(order(&s), ["colorbalancergb 0", "sigmoid 0", "tonecurve 0"]);
    assert_eq!(s.in_pipe_order().unordered, [("sigmoid".to_string(), 0)]);
}

#[test]
fn older_sidecars_order_items_by_their_own_numbers() {
    // xmp_version 3: an order number per item.
    let xml = |version: u32, orders: [&str; 2]| {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/" darktable:xmp_version="{version}">
<darktable:history><rdf:Seq>
<rdf:li darktable:num="0" darktable:operation="exposure" darktable:enabled="1" darktable:modversion="5" darktable:params="00" darktable:multi_priority="0" {}/>
<rdf:li darktable:num="1" darktable:operation="exposure" darktable:enabled="1" darktable:modversion="5" darktable:params="00" darktable:multi_priority="1" {}/>
</rdf:Seq></darktable:history></rdf:Description></rdf:RDF></x:xmpmeta>"#,
            orders[0], orders[1]
        )
    };
    let v3 = Sidecar::parse(&xml(
        3,
        [
            r#"darktable:iop_order="12.2""#,
            r#"darktable:iop_order="11.9""#,
        ],
    ))
    .unwrap();
    assert_eq!(order(&v3), ["exposure 1", "exposure 0"]);
    let v3 = Sidecar::parse(&xml(
        3,
        [
            r#"darktable:iop_order="12.0""#,
            r#"darktable:iop_order="12.5""#,
        ],
    ))
    .unwrap();
    assert_eq!(order(&v3), ["exposure 0", "exposure 1"]);
    // Before version 3, instances run before their module, the highest first.
    let v2 = Sidecar::parse(&xml(2, ["", ""])).unwrap();
    assert_eq!(order(&v2), ["exposure 1", "exposure 0"]);
    assert_eq!(v2.in_pipe_order().order.source, "legacy (xmp_version 2)");
}

/// A darktable checkout: `HELIOS_DARKTABLE_SRC`, else one next to Helios's.
fn darktable_source() -> Option<PathBuf> {
    let dir = std::env::var_os("HELIOS_DARKTABLE_SRC").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../darktable"),
        PathBuf::from,
    );
    if dir.join("src/common/iop_order.c").is_file() {
        Some(dir)
    } else {
        eprintln!("skipped: no darktable source in {}", dir.display());
        None
    }
}

/// The text between `start` and the next line that is `};` or `}`.
fn block<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let from = source.find(start).unwrap_or_else(|| panic!("no `{start}`"));
    let rest = &source[from..];
    &rest[..rest.find(end).unwrap()]
}

/// The quoted strings of a line.
fn quoted(line: &str) -> Vec<&str> {
    line.split('"').skip(1).step_by(2).collect()
}

#[test]
fn the_embedded_orders_are_darktables() {
    let Some(dir) = darktable_source() else {
        return;
    };
    let source = std::fs::read_to_string(dir.join("src/common/iop_order.c")).unwrap();
    for (name, preset) in [
        ("legacy_order", Preset::Legacy),
        ("v30_order", Preset::V30),
        ("v30_jpg_order", Preset::V30Jpeg),
        ("v50_order", Preset::V50),
        ("v50_jpg_order", Preset::V50Jpeg),
    ] {
        let table = block(
            &source,
            &format!("const dt_iop_order_entry_t {name}[] = {{"),
            "\n};",
        );
        let entries: Vec<(f32, String)> = table
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let number = line.strip_prefix('{')?.trim().strip_prefix('{')?;
                let number = number.split('}').next()?.trim().trim_end_matches('f');
                let op = quoted(line).first()?.to_string();
                (!op.is_empty()).then(|| (number.parse().unwrap(), op))
            })
            .collect();
        let embedded: Vec<(f32, String)> = preset
            .table()
            .iter()
            .map(|&(f, op)| (f, op.to_string()))
            .collect();
        assert_eq!(embedded, entries, "{name}");
    }
    let migrations: Vec<(String, String, bool)> = block(
        &source,
        "void dt_ioppr_migrate_legacy_iop_order_list",
        "\n}",
    )
    .lines()
    .filter(|line| line.contains("_insert_before_after("))
    .map(|line| {
        let q = quoted(line);
        (q[0].to_string(), q[1].to_string(), line.contains("TRUE"))
    })
    .collect();
    let embedded: Vec<(String, String, bool)> = MIGRATIONS
        .iter()
        .map(|&(a, m, b)| (a.to_string(), m.to_string(), b))
        .collect();
    assert_eq!(embedded, migrations);
}

/// darktable's public test sidecars (see `sidecars.rs`).
fn tests_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("HELIOS_DT_TESTS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../darktable/src/tests/integration"),
        PathBuf::from,
    );
    dir.join("0001-exposure/exposure.xmp")
        .is_file()
        .then_some(dir)
}

#[test]
fn darktables_test_sidecars_place_every_module() {
    let Some(dir) = tests_dir() else {
        eprintln!("skipped: no darktable integration tests");
        return;
    };
    let (mut sidecars, mut reordered) = (0, 0);
    for entry in std::fs::read_dir(&dir).unwrap() {
        for file in std::fs::read_dir(entry.unwrap().path())
            .into_iter()
            .flatten()
        {
            let path = file.unwrap().path();
            if path.extension().is_none_or(|e| e != "xmp") {
                continue;
            }
            let s = Sidecar::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let arranged = s.in_pipe_order();
            // Sidecars darktable wrote place every module they use.
            assert!(
                arranged.unordered.is_empty(),
                "{}: {:?}",
                path.display(),
                arranged.unordered
            );
            assert_eq!(arranged.items.len(), s.effective().len());
            if arranged.items != s.effective() {
                reordered += 1;
            }
            sidecars += 1;
        }
    }
    eprintln!("{reordered} of {sidecars} sidecars run in another order than their history");
    assert!(sidecars > 150 && reordered > 0, "{reordered} of {sidecars}");
}
