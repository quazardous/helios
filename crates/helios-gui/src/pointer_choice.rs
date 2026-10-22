//! The pointer profile Helios starts with: Adobe's (Lightroom's habits),
//! where the wheel scrolls the panel and a slider takes it only once
//! clicked or focused, so scrolling past sliders never changes a value
//! (darktable's pitfall 7, `docs/darktable-pitfalls/ui-platforms.md`).
//! A profile the user saved is kept; darktable's stays one choice away in
//! Settings › Input.

/// The pointer profile by default (egui-pop's `input.pointer_profile`).
pub const DEFAULT_PROFILE: &str = "adobe";

/// darktable's profile, faithful to darktable.
pub const DARKTABLE_PROFILE: &str = "darktable";
