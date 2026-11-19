//! Wall-clock budgets: a frame, a click's effect, a close must come
//! within a time. Over budget fails, except on a shared machine
//! (`HELIOS_SHARED_MACHINE`, set by the bench box's shared test jobs),
//! where other jobs, swap and throttling slow every frame several times
//! over: there it only warns. A real regression still fails on CI and in
//! the box's `test-times` job, which holds the machine alone (see
//! docs/bench.md).
//!
//! What a test can check without a clock (a frame needed or not, a
//! count, a state) it checks without one; a budget is for "at once", the
//! one claim only a clock can see.

use std::time::Duration;

/// Whether the tests share the machine with other jobs.
pub fn shared_machine() -> bool {
    std::env::var_os("HELIOS_SHARED_MACHINE").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Checks that `took` is under `budget`: fails over it, or warns on a
/// shared machine.
pub fn within(what: &str, took: Duration, budget: Duration) {
    if took < budget {
        return;
    }
    let message = format!("{what}: took {took:?}, over {budget:?}");
    if shared_machine() {
        eprintln!("warning (shared machine, budget not enforced): {message}");
    } else {
        panic!("{message}");
    }
}
