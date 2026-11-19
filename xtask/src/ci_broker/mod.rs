//! The CI broker: which tests a CI run needs, from what earlier runs saw.
//!
//! Step B1 of the broker (see docs/ci-broker.md): the per-test history.
//! Each CI run's results become one line of a day file, and a summary per
//! test (passes, failures, flakes, last failure, median duration) is folded
//! from those lines. The selection policy builds on it in later steps:
//! the always-run set (B2, `select`) and the shadow week (B3, `shadow`).
//!
//! This module depends on std and `serde_json` only, and knows nothing of
//! Helios or of how its reports are read: the xtask command `ci-history`
//! (`ci_history.rs`) reads the JUnit files and the git branch. It can move
//! to a shared crate as it is, when egui-pop needs it.

pub mod history;
pub mod report;
pub mod select;
pub mod shadow;
