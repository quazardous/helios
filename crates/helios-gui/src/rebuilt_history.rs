//! An image's undo history after a restart: rebuilt from its saved
//! versions (`library.edit.history`, the step markers of
//! `helios_vault::edit_steps`) and installed into the value store when
//! the image opens with no line parked for it (`history.resume`), so
//! Ctrl+Z undoes the last saved step as if the app had never stopped.
//!
//! Each rebuilt step is the difference between the edit it started from
//! and the one it reached, as value paths: each node's parameters
//! (`node/<n>/<p>`), whether it is bypassed (`bypass/<n>`), and the
//! masks' labels and locks (`label/…`, `lock/…`). A path one edit does
//! not name holds its default there. A path the store does not hold (a
//! node the network no longer has) is left out, and a step left with no
//! change is no step.

use std::collections::BTreeMap;

use helios_core::Value;
use helios_values::{Step, Store};
use serde_json::Value as Json;

use crate::edit_snapshots::{origin_of, values_of_edit};

/// The steps of `answer` (`library.edit.history`'s), as the store's:
/// undo steps oldest first, redo steps the next first. `unedited` is the
/// image's network with no edit, where a first step starts from.
pub fn steps_of(
    store: &Store,
    answer: &Json,
    unedited: &Json,
) -> Result<(Vec<Step>, Vec<Step>), String> {
    // Each step starts where the one before ended: the first from the
    // unedited image, `redo`'s first where `undo`'s last ended.
    let mut before = values_of_edit(unedited)?;
    let mut steps = |key: &str| -> Result<Vec<Step>, String> {
        let mut out = Vec::new();
        for row in answer[key].as_array().into_iter().flatten() {
            let after = values_of_edit(&row["edit"])?;
            let changes = diff(store, &before, &after);
            before = after;
            if changes.is_empty() {
                continue;
            }
            out.push(Step {
                id: 0,
                label: row["label"].as_str().unwrap_or_default().to_owned(),
                origin: origin_of(row["origin"].as_str().unwrap_or("user")),
                changes,
                place: Vec::new(),
                group: None,
                aim: None,
            });
        }
        Ok(out)
    };
    let undo = steps("undo")?;
    Ok((undo, steps("redo")?))
}

/// The changes from `before` to `after` on the paths `store` holds
/// (path, old, new), in path order.
fn diff(
    store: &Store,
    before: &[(String, Value)],
    after: &[(String, Value)],
) -> Vec<(String, Value, Value)> {
    let mut both: BTreeMap<&str, (Option<&Value>, Option<&Value>)> = BTreeMap::new();
    for (path, v) in before {
        both.entry(path).or_default().0 = Some(v);
    }
    for (path, v) in after {
        both.entry(path).or_default().1 = Some(v);
    }
    let mut out = Vec::new();
    for (path, (b, a)) in both {
        if b == a {
            continue;
        }
        let Some(e) = store.get(path) else {
            continue;
        };
        let b = b.unwrap_or(&e.descriptor.default);
        let a = a.unwrap_or(&e.descriptor.default);
        if b != a {
            out.push((path.to_owned(), b.clone(), a.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::{ContentHash, Params};
    use helios_shell::commands::Origin;
    use helios_values::Descriptor;
    use helios_vault::edit_steps::{StepKind, marked, rebuild};
    use helios_vault::{ImageId, Version, edit};

    const NODES: usize = 20;
    const PARAMS: usize = 10;

    /// An edit of [`NODES`] nodes of [`PARAMS`] parameters each, node
    /// `moved`'s first parameter at `v`.
    fn edit_with(moved: usize, v: f64) -> Json {
        let mut modules = BTreeMap::new();
        for n in 0..NODES {
            let mut params = Params::new();
            for p in 0..PARAMS {
                let x = if n == moved && p == 0 { v } else { 0.0 };
                params.set(&format!("p{p}"), Value::Float(x));
            }
            modules.insert(format!("n{n}"), ("module".to_owned(), params));
        }
        edit::from_modules(&modules)
    }

    fn store() -> Store {
        let mut s = Store::new();
        for n in 0..NODES {
            for p in 0..PARAMS {
                let d = Descriptor::float("p", -1e6, 1e6, 0.0);
                s.declare(&format!("node/n{n}/p{p}"), d);
            }
            s.declare(&format!("bypass/n{n}"), Descriptor::bool("bypass", false));
        }
        s
    }

    fn at(s: &mut Store, path: &str, v: f64) {
        s.declare_with(
            path,
            Descriptor::float("p", -1e6, 1e6, 0.0),
            Value::Float(v),
        );
    }

    /// A line of `count` versions, each a `do` moving one parameter, the
    /// last `undone` of them undone (one `undo` version each).
    fn line(count: usize, undone: usize) -> Vec<Version> {
        let image = ImageId(ContentHash([3; 32]));
        let mut out: Vec<Version> = Vec::new();
        let push = |edit: Json, kind: StepKind, out: &mut Vec<Version>| {
            let parents = out.last().map(|v| vec![v.id]).unwrap_or_default();
            let at = out.len() as u64;
            let v = Version::new(image, parents, edit, "user", at)
                .labelled(&format!("step {at}"))
                .with_meta(Some(marked(None, kind)));
            out.push(v);
        };
        for i in 0..count {
            push(edit_with(i % NODES, (i + 1) as f64), StepKind::Do, &mut out);
        }
        for i in (count - undone..count).rev() {
            let back = if i == 0 {
                edit_with(0, 0.0)
            } else {
                edit_with((i - 1) % NODES, i as f64)
            };
            push(back, StepKind::Undo, &mut out);
        }
        out
    }

    #[test]
    fn a_line_rebuilt_undoes_its_last_step_and_redoes_the_undone() {
        let versions = line(5, 1);
        let answer = rebuild(&versions).to_json();
        let unedited = edit_with(0, 0.0);
        let mut s = store();
        // The values the head says: 4 steps made, the 5th undone (each
        // edit moves one node's parameter, the others at 0).
        at(&mut s, "node/n3/p0", 4.0);
        let (undo, redo) = steps_of(&s, &answer, &unedited).unwrap();
        assert_eq!((undo.len(), redo.len()), (4, 1));
        assert_eq!(undo[3].label, "step 3");
        assert_eq!(undo[3].origin, Origin::User);
        assert!(s.install_image_line(undo, redo));
        s.undo(&Origin::User).unwrap();
        assert_eq!(s.get("node/n3/p0").unwrap().value, Value::Float(0.0));
        assert_eq!(s.get("node/n2/p0").unwrap().value, Value::Float(3.0));
        s.redo(&Origin::User).unwrap();
        s.redo(&Origin::User).unwrap();
        assert_eq!(s.get("node/n4/p0").unwrap().value, Value::Float(5.0));
    }

    /// The plan's goal (`docs/design/undo.md`, R5): a line of 300
    /// versions rebuilt and loaded under 20 ms in a release build. A debug
    /// build parses JSON some 20 times slower (about 0.5 s on a desktop):
    /// it is given 2 s, which still catches a cost that grows with the
    /// square of the line.
    #[test]
    fn three_hundred_versions_rebuild_and_load_in_time() {
        let versions = line(300, 0);
        let unedited = edit_with(0, 0.0);
        let mut s = store();
        at(&mut s, "node/n19/p0", 300.0);
        let budget =
            std::time::Duration::from_millis(if cfg!(debug_assertions) { 2_000 } else { 20 });
        let started = std::time::Instant::now();
        let answer = rebuild(&versions).to_json();
        let (undo, redo) = steps_of(&s, &answer, &unedited).unwrap();
        assert!(s.install_image_line(undo, redo));
        let took = started.elapsed();
        assert_eq!(s.undo_steps().len(), 300);
        assert!(
            took <= budget,
            "rebuilt and loaded in {took:?}, over {budget:?}"
        );
    }
}
