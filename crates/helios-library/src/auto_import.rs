//! The safeguards of automatic imports (`docs/design/import-connectors.md`,
//! section 3.3, "Never a surprise"): what an automatic import does with a
//! batch of new files from a saved source, decided the same way by
//! whichever runs it, the desktop today, the headless node later.
//!
//! - **Paused** (`import.automatic.paused`): nothing is imported.
//! - **The first run is confirmed**: a source whose automatic import was
//!   never confirmed is shown in the import window, where pressing Import
//!   confirms it.
//! - **Above a size, a question** (`import.automatic.ask_files`,
//!   `import.automatic.ask_gb`): more files or bytes than that are
//!   offered, not imported.
//! - Otherwise the import runs.

/// The setting that pauses every automatic import.
pub const PAUSED: &str = "import.automatic.paused";
/// The setting above which many files ask first (0: never).
pub const ASK_FILES: &str = "import.automatic.ask_files";
/// The setting above which many gigabytes ask first (0: never).
pub const ASK_GB: &str = "import.automatic.ask_gb";

/// The settings' defaults, as `helios-config` declares them.
pub const DEFAULT_ASK_FILES: u64 = 500;
pub const DEFAULT_ASK_GB: f64 = 20.0;

/// The limits the settings set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub paused: bool,
    /// More files than this ask first; 0 never asks.
    pub ask_files: u64,
    /// More bytes than this ask first; 0 never asks.
    pub ask_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(false, DEFAULT_ASK_FILES as f64, DEFAULT_ASK_GB)
    }
}

impl Limits {
    /// From the settings' values: paused, a count of files, gigabytes.
    pub fn new(paused: bool, ask_files: f64, ask_gb: f64) -> Self {
        Self {
            paused,
            ask_files: ask_files.max(0.0) as u64,
            ask_bytes: (ask_gb.max(0.0) * 1e9) as u64,
        }
    }
}

/// A batch of new files a source's trigger found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Batch {
    /// The source's automatic import was confirmed once.
    pub confirmed: bool,
    pub files: u64,
    pub bytes: u64,
}

/// What becomes of a batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Imported.
    Run,
    /// Automatic imports are paused: left alone.
    Paused,
    /// The source's first run: shown in the import window to confirm.
    Confirm,
    /// Above the limits: offered, not imported.
    Ask,
}

/// What becomes of `batch` under `limits`: paused first, then the first
/// run, then the size.
pub fn verdict(limits: &Limits, batch: &Batch) -> Verdict {
    let above = |n: u64, cap: u64| cap > 0 && n > cap;
    if limits.paused {
        Verdict::Paused
    } else if !batch.confirmed {
        Verdict::Confirm
    } else if above(batch.files, limits.ask_files) || above(batch.bytes, limits.ask_bytes) {
        Verdict::Ask
    } else {
        Verdict::Run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(confirmed: bool, files: u64, bytes: u64) -> Batch {
        Batch {
            confirmed,
            files,
            bytes,
        }
    }

    #[test]
    fn auto_import_paused_wins_then_the_first_run_then_the_size() {
        let limits = Limits::new(false, 10.0, 1.0);
        assert_eq!(verdict(&limits, &batch(true, 3, 1_000)), Verdict::Run);
        assert_eq!(verdict(&limits, &batch(false, 3, 1_000)), Verdict::Confirm);
        assert_eq!(verdict(&limits, &batch(true, 11, 1_000)), Verdict::Ask);
        assert_eq!(verdict(&limits, &batch(true, 10, 1_000)), Verdict::Run);
        assert_eq!(
            verdict(&limits, &batch(true, 3, 1_000_000_001)),
            Verdict::Ask
        );
        let paused = Limits {
            paused: true,
            ..limits
        };
        assert_eq!(verdict(&paused, &batch(false, 99, 0)), Verdict::Paused);
        assert_eq!(verdict(&paused, &batch(true, 1, 0)), Verdict::Paused);
    }

    #[test]
    fn auto_import_a_limit_of_zero_never_asks() {
        let limits = Limits::new(false, 0.0, 0.0);
        assert_eq!(
            verdict(&limits, &batch(true, 1_000_000, u64::MAX)),
            Verdict::Run
        );
        assert_eq!(
            Limits::default(),
            Limits {
                paused: false,
                ask_files: 500,
                ask_bytes: 20_000_000_000
            }
        );
    }
}
