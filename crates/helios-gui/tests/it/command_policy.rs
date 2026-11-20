//! Every command run passes Helios's policy on egui-pop's `command.run`
//! before its handler, and every Helios request point is fail-closed.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use helios_gui::{Shell, helios_panels};
use helios_shell::commands::{CommandSpec, Origin};
use serde_json::json;

/// A dry run of a command that takes none is refused by the policy,
/// whoever asks: the handler does not run, the caller reads who refused
/// and why, and the journal says `command.refused` with `by`. A command
/// that takes `dry_run`, or a run that asks for none, goes on.
#[test]
fn a_dry_run_of_a_command_that_takes_none_is_refused_by_the_policy() {
    let mut shell = Shell::new(helios_panels(), None);
    let ran = Arc::new(AtomicUsize::new(0));
    let r = ran.clone();
    shell
        .register(
            CommandSpec::new("test.write", "Write", "Test"),
            move |_, _| {
                r.fetch_add(1, Ordering::SeqCst);
                Ok(json!("written"))
            },
        )
        .unwrap();
    shell
        .register(
            CommandSpec::new("test.dry", "Dry", "Test").dry_run(),
            |args, _| Ok(json!({ "dry": args["dry_run"] })),
        )
        .unwrap();

    for origin in [Origin::User, Origin::Agent("t".into())] {
        let refused = shell
            .run("test.write", json!({ "dry_run": true }), origin.clone())
            .expect_err("a dry run that would act is refused");
        assert!(refused.contains("helios.policy"), "{origin:?}: {refused}");
        assert!(refused.contains("takes no `dry_run`"), "{refused}");
    }
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the handler never ran");
    let said: Vec<serde_json::Value> = shell
        .app()
        .handle()
        .events_since(0)
        .into_iter()
        .filter(|e| e.name == "command.refused")
        .map(|e| e.data.clone())
        .collect();
    assert_eq!(said.len(), 2, "{said:?}");
    assert_eq!(said[0]["by"], "helios.policy");
    assert_eq!(said[0]["command"], "test.write");
    assert_eq!(said[0]["failed"], false);

    assert_eq!(
        shell
            .run("test.write", json!({ "dry_run": false }), Origin::User)
            .unwrap(),
        json!("written")
    );
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    assert_eq!(
        shell
            .run("test.dry", json!({ "dry_run": true }), Origin::User)
            .unwrap(),
        json!({ "dry": true })
    );
}

/// `command.run`'s listeners on the desktop's bus, in their order
/// (`tests/bus/commands.txt`).
#[test]
fn the_command_run_listeners_run_in_a_declared_order() {
    let shell = Shell::new(helios_panels(), None);
    assert_eq!(
        helios_gui::command_policy::order(&*shell.app().handle().bus()),
        include_str!("../bus/commands.txt")
    );
}

/// The `.rs` files under `dir`, build folders skipped.
fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "target" && !name.to_string_lossy().starts_with('.') {
                sources(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The points of `source` declared as requests: the type of each
/// `impl Point for <T>` whose block says `PointKind::Request`.
fn requests(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (at, _) in source.match_indices("Point for ") {
        let rest = &source[at + "Point for ".len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let block = rest.split_once("\n}").map_or(rest, |(b, _)| b);
        if !name.is_empty() && block.contains("PointKind::Request") {
            found.push(name);
        }
    }
    found
}

/// The request points that are fail-open on purpose, each with its
/// reason. The list only shrinks.
const FAIL_OPEN: &[(&str, &str)] = &[
    (
        "AppQuit",
        "quitting must stay possible: a listener that fails or hangs is reported and the quit goes on; a listener keeping Helios open refuses",
    ),
    (
        "NodeQuit",
        "the node's stop must end: a jobs listener past its deadline is cut and reported, and the library's close still runs, last",
    ),
];

/// Every request point Helios declares is fail-closed
/// (`PointDecl::fail_closed`): a listener that fails, panics or misses
/// its deadline refuses the request instead of being skipped, so a
/// refusal can never be lost. The exceptions are listed ([`FAIL_OPEN`]).
#[test]
fn every_helios_request_point_is_declared_fail_closed() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["crates", "modules", "plugins", "xtask"] {
        sources(&root.join(dir), &mut files);
    }
    let texts: Vec<String> = files
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .collect();
    let points: Vec<String> = texts.iter().flat_map(|t| requests(t)).collect();
    for expected in ["ImageOpen", "ImageClose", "ExportStart", "LibraryWrite"] {
        assert!(
            points.iter().any(|p| p == expected),
            "the scan finds {expected}: {points:?}"
        );
    }
    let mut open = Vec::new();
    for (point, _why) in FAIL_OPEN {
        assert!(
            points.iter().any(|p| p.as_str() == *point),
            "{point}: listed fail-open but no longer a request point"
        );
    }
    for point in points
        .iter()
        .filter(|p| !FAIL_OPEN.iter().any(|(open, _)| *open == p.as_str()))
    {
        let decl = format!("PointDecl::of::<{point}>(");
        let n = decl.len();
        let declared: Vec<&str> = texts
            .iter()
            .flat_map(|t| t.match_indices(decl.as_str()).map(|(at, _)| &t[at + n..]))
            .collect();
        if declared.is_empty() {
            open.push(format!("{point}: never declared in a contract table"));
        }
        for rest in declared {
            // The declaration runs to the next one, or the table's end.
            let end = [rest.find("PointDecl::of::<"), rest.find("];")]
                .into_iter()
                .flatten()
                .min()
                .unwrap_or(rest.len());
            if !rest[..end].contains(".fail_closed()") {
                open.push(format!("{point}: declared without .fail_closed()"));
            }
        }
    }
    assert!(
        open.is_empty(),
        "request points that are not fail-closed:\n{}",
        open.join("\n")
    );
}
