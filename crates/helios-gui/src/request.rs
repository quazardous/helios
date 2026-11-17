//! Requests on the bus, fail-closed: any listener failure is a refusal.
//!
//! A request point ([`PointKind::Request`]) asks its listeners whether
//! something may go on (an image switch); a listener refuses by writing a
//! [`Refusal`] into the request's value. The bus keeps a listener's value
//! only when it succeeds: one that returns an error, panics or returns
//! past its deadline has its value dropped, refusal included, and the
//! listeners after it still run. Read as is, a refusal written by a late
//! save would be lost and the request would go through (fail-open).
//!
//! This module closes that, at the emit site and in the listeners:
//! - [`pass`] emits a request and turns any failure that dropped a
//!   contribution (error, panic, late) into a refusal by that listener,
//!   "refused by <listener>: <failure>", unless an earlier one refused;
//! - [`listener`] wraps a listener: it is not run once the request is
//!   refused, and it refuses itself when a listener before it in the same
//!   pass ran but its contribution is gone (it failed): what follows a
//!   failed listener (a history cleared after a failed save) does not run.
//!
//! Every request point in Helios is passed through [`pass`] (a test scans
//! the sources). The bus contract's fail-closed veto, once it exists,
//! replaces this module.
//!
//! [`PointKind::Request`]: egui_pop::api::bus::bus_contract::PointKind::Request

use std::cell::RefCell;

use egui_pop::api::bus::bus_contract::{Bus, HookError, Point};

/// A refusal: the listener that refused, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub by: String,
    pub reason: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "refused by {}: {}", self.by, self.reason)
    }
}

/// A request's value: the listeners that ran, in order, and the refusal
/// if one refused.
pub trait Refusable {
    /// The listeners that ran, in their order.
    fn ran(&mut self) -> &mut Vec<String>;
    /// The refusal, if a listener refused.
    fn refusal(&mut self) -> &mut Option<Refusal>;
}

thread_local! {
    /// The passes going on on this thread, innermost last: the listeners
    /// that started in each, in order.
    static PASSES: RefCell<Vec<Vec<String>>> = const { RefCell::new(Vec::new()) };
}

/// The listener `id` around `run`, fail-closed: not run once the request
/// is refused; refusing itself when a listener that started before it in
/// this pass left no trace in the value (it failed, its contribution
/// dropped); `run`'s `Err(reason)` refuses the request.
pub fn listener<I, V: Refusable>(
    id: &'static str,
    run: impl Fn(&I, &mut V) -> Result<(), String>,
) -> impl Fn(&I, &mut V) -> Result<(), HookError> {
    move |input, value| {
        if value.refusal().is_some() {
            return Ok(());
        }
        let lost = PASSES.with(|p| {
            let p = p.borrow();
            let started = p.last()?;
            started
                .iter()
                .find(|s| !value.ran().iter().any(|r| r == *s))
                .cloned()
        });
        if let Some(by) = lost {
            *value.refusal() = Some(Refusal {
                by,
                reason: "it failed: its answer was lost".into(),
            });
            return Ok(());
        }
        PASSES.with(|p| {
            if let Some(started) = p.borrow_mut().last_mut() {
                started.push(id.to_owned());
            }
        });
        value.ran().push(id.to_owned());
        if let Err(reason) = run(input, value) {
            *value.refusal() = Some(Refusal {
                by: id.to_owned(),
                reason,
            });
        }
        Ok(())
    }
}

/// Passes `input` through the request `P` on `bus`, from `value`: the
/// value its listeners left, fail-closed. A listener whose contribution
/// the bus dropped (an error, a panic, past its deadline) refuses the
/// request, "refused by <listener>: <failure>", unless one before it
/// refused already. Each failure is logged.
pub fn pass<P: Point>(bus: &dyn Bus, input: P::Input, value: P::Value) -> P::Value
where
    P::Value: Refusable + Clone,
{
    PASSES.with(|p| p.borrow_mut().push(Vec::new()));
    let altered = bus.alter::<P>(input, value);
    PASSES.with(|p| p.borrow_mut().pop());
    let mut value = altered.value;
    for failure in &altered.failures {
        log::warn!(target: "helios", "{}: {failure}", P::ID);
        if !failure.dropped() {
            continue;
        }
        let refusal = value.refusal();
        // A refusal said by a listener after it, for the one that failed,
        // gets the failure's words.
        if refusal.as_ref().is_none_or(|r| r.by == failure.listener) {
            *refusal = Some(Refusal {
                by: failure.listener.clone(),
                reason: failure_words(failure),
            });
        }
    }
    value
}

/// What went wrong with a listener, in the bus's words, without the
/// point and listener it starts with ("took 3s, past its deadline of 1s").
fn failure_words(failure: &egui_pop::api::bus::bus_contract::Failure) -> String {
    let said = failure.to_string();
    let head = format!(
        "hook `{}`, listener `{}`: ",
        failure.point, failure.listener
    );
    said.strip_prefix(&head)
        .map_or_else(|| said.clone(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_pop::api::bus::bus_contract::{ListenOpts, PointKind};
    use egui_pop::api::bus::egui_pop_hooks::Kernel;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    #[derive(Clone, Debug, Default)]
    struct Asked {
        ran: Vec<String>,
        refused: Option<Refusal>,
    }

    impl Refusable for Asked {
        fn ran(&mut self) -> &mut Vec<String> {
            &mut self.ran
        }
        fn refusal(&mut self) -> &mut Option<Refusal> {
            &mut self.refused
        }
    }

    struct Ask;

    impl Point for Ask {
        const ID: &'static str = "test.ask";
        type Input = ();
        type Value = Asked;
        const KIND: PointKind = PointKind::Request;
    }

    /// A first listener doing `first`, then a second counting its runs.
    fn bus(first: impl Fn() -> Result<(), String> + 'static) -> (Kernel, Rc<Cell<u32>>) {
        let bus = Kernel::with_default_deadline(Duration::from_millis(20));
        bus.listen_local::<Ask>(
            "first",
            ListenOpts::new(),
            listener("first", move |(), _: &mut Asked| first()),
        )
        .unwrap();
        let after = Rc::new(Cell::new(0));
        let a = after.clone();
        bus.listen_local::<Ask>(
            "second",
            ListenOpts::new().after("first"),
            listener("second", move |(), _: &mut Asked| {
                a.set(a.get() + 1);
                Ok(())
            }),
        )
        .unwrap();
        (bus, after)
    }

    fn slow() {
        std::thread::sleep(Duration::from_millis(60));
    }

    #[test]
    fn a_request_every_listener_accepts_goes_through() {
        let (bus, after) = bus(|| Ok(()));
        let asked = pass::<Ask>(&bus, (), Asked::default());
        assert_eq!(asked.refused, None);
        assert_eq!(asked.ran, ["first", "second"]);
        assert_eq!(after.get(), 1);
    }

    #[test]
    fn a_refusal_in_time_is_kept_and_stops_the_listeners_after_it() {
        let (bus, after) = bus(|| Err("no".into()));
        let asked = pass::<Ask>(&bus, (), Asked::default());
        assert_eq!(asked.refused.unwrap().to_string(), "refused by first: no");
        assert_eq!(after.get(), 0);
    }

    #[test]
    fn a_late_refusal_is_still_a_refusal_and_stops_the_listeners_after_it() {
        let (bus, after) = bus(|| {
            slow();
            Err("the disk failed".into())
        });
        let asked = pass::<Ask>(&bus, (), Asked::default());
        let refused = asked.refused.expect("refused");
        assert_eq!(refused.by, "first");
        assert!(refused.reason.contains("past its deadline"), "{refused}");
        assert_eq!(after.get(), 0, "nothing after a failed listener runs");
    }

    #[test]
    fn a_late_acceptance_is_a_refusal() {
        let (bus, after) = bus(|| {
            slow();
            Ok(())
        });
        let asked = pass::<Ask>(&bus, (), Asked::default());
        assert_eq!(asked.refused.expect("refused").by, "first");
        assert_eq!(after.get(), 0);
    }

    #[test]
    fn a_panic_is_a_refusal() {
        let (bus, after) = bus(|| panic!("boom"));
        let asked = pass::<Ask>(&bus, (), Asked::default());
        let refused = asked.refused.expect("refused");
        assert_eq!(refused.by, "first");
        assert!(refused.reason.contains("boom"), "{refused}");
        assert_eq!(after.get(), 0);
    }

    #[test]
    fn a_failure_of_the_last_listener_is_a_refusal() {
        let bus = Kernel::with_default_deadline(Duration::from_millis(20));
        bus.listen_local::<Ask>(
            "only",
            ListenOpts::new(),
            |(), _: &mut Asked| -> Result<(), HookError> { Err(HookError("broken".into())) },
        )
        .unwrap();
        let asked = pass::<Ask>(&bus, (), Asked::default());
        assert_eq!(
            asked.refused.expect("refused").to_string(),
            "refused by only: failed: broken"
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

    /// The points of `source` declared as requests: the type each
    /// `impl Point for <T>` whose block says `PointKind::Request`.
    fn requests(source: &str) -> Vec<String> {
        let mut found = Vec::new();
        for (at, _) in source.match_indices("impl Point for ") {
            let rest = &source[at + "impl Point for ".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let block = rest.split_once("\n}").map_or(rest, |(b, _)| b);
            if block.contains("PointKind::Request") {
                found.push(name);
            }
        }
        found
    }

    /// Every request point Helios declares is emitted through [`pass`]
    /// (fail-closed), never straight on the bus: a raw `alter` would let
    /// a late or failed listener's refusal be lost.
    #[test]
    fn every_request_point_is_passed_through_the_fail_closed_helper() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        for dir in ["crates", "plugins", "xtask"] {
            sources(&root.join(dir), &mut files);
        }
        let texts: Vec<(std::path::PathBuf, String)> = files
            .into_iter()
            .filter_map(|f| Some((f.clone(), std::fs::read_to_string(&f).ok()?)))
            .collect();
        let points: Vec<String> = texts
            .iter()
            .filter(|(f, _)| !f.ends_with("helios-gui/src/request.rs"))
            .flat_map(|(_, t)| requests(t))
            .collect();
        assert!(
            points.contains(&"ImageOpen".to_owned()) && points.contains(&"ImageClose".to_owned()),
            "the scan finds the darkroom's requests: {points:?}"
        );
        let mut raw = Vec::new();
        for point in &points {
            let mut passed = false;
            for (file, text) in &texts {
                for emit in ["alter", "alter_any", "collect", "emit", "notify"] {
                    if text.contains(&format!(".{emit}::<{point}>(")) {
                        raw.push(format!("{}: {emit}::<{point}>", file.display()));
                    }
                }
                passed |= text.contains(&format!("request::pass::<{point}>("));
            }
            if !passed {
                raw.push(format!("{point}: never passed through request::pass"));
            }
        }
        assert!(
            raw.is_empty(),
            "request points emitted around the fail-closed helper:\n{}",
            raw.join("\n")
        );
    }
}
