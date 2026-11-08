//! The catalogue snapshots check: the last verified snapshot of the
//! catalogue is recent and restorable. Until snapshots exist, the desktop
//! gives none ([`Context::snapshots`] is `None`) and the check says so in
//! one line; snapshots, once taken, only fill a [`Snapshots`].

use std::path::PathBuf;

use helios_core::Message;
use serde_json::json;

use crate::{Check, Context, Finding, Fix, Level};

pub const SNAPSHOTS: &str = "catalogue.snapshots";

/// Days the last verified snapshot may be old before a warning, by
/// default.
pub const MAX_AGE_DAYS: u64 = 7;

/// One snapshot of the catalogue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub file: PathBuf,
    /// When it was taken, in seconds since the epoch.
    pub taken: u64,
    /// When it was last read back and found whole; `None`: never.
    pub verified: Option<u64>,
    /// Whether that reading found it restorable.
    pub restorable: bool,
}

/// The catalogue's snapshots, and what "recent" means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshots {
    pub list: Vec<Snapshot>,
    /// Now, in seconds since the epoch.
    pub now: u64,
    pub max_age_days: u64,
}

impl Snapshots {
    /// The newest snapshot verified, if any.
    pub fn last_verified(&self) -> Option<&Snapshot> {
        self.list
            .iter()
            .filter(|s| s.verified.is_some())
            .max_by_key(|s| s.taken)
    }
}

/// The last verified snapshot: none, or not restorable, is a problem;
/// older than `max_age_days`, a warning.
#[derive(Clone, Copy, Debug, Default)]
pub struct SnapshotsCheck;

impl Check for SnapshotsCheck {
    fn id(&self) -> &str {
        SNAPSHOTS
    }

    fn title(&self) -> Message {
        Message::new("health-snapshots", "Catalogue snapshots")
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        if ctx.vault.is_none() {
            return Vec::new();
        }
        let Some(snapshots) = &ctx.snapshots else {
            return vec![Finding::new(
                SNAPSHOTS,
                "",
                Level::Ok,
                Message::new("health-snapshots-none-yet", "No catalogue snapshots yet"),
                Message::new(
                    "health-snapshots-none-yet-detail",
                    "Helios does not take catalogue snapshots yet; the backups keep the photos.",
                ),
            )];
        };
        let Some(last) = snapshots.last_verified() else {
            return vec![Finding::new(
                SNAPSHOTS,
                "",
                Level::Problem,
                Message::new(
                    "health-snapshots-unverified",
                    "No catalogue snapshot is verified",
                ),
                Message::new(
                    "health-snapshots-unverified-detail",
                    "{ $count } snapshots, none read back yet.",
                )
                .with("count", snapshots.list.len()),
            )];
        };
        let file = last.file.display().to_string();
        let folder = last
            .file
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let fix = Fix {
            command: "library.openFolder".into(),
            args: json!({ "path": folder }),
            label: Message::new("health-fix-snapshots-folder", "Open the snapshots' folder"),
        };
        let days = snapshots.now.saturating_sub(last.taken) / 86_400;
        if !last.restorable {
            return vec![
                Finding::new(
                    SNAPSHOTS,
                    "",
                    Level::Problem,
                    Message::new(
                        "health-snapshots-broken",
                        "The last catalogue snapshot cannot be restored",
                    ),
                    Message::new(
                        "health-snapshots-file-detail",
                        "{ $file }, { $days } days old",
                    )
                    .with("file", file)
                    .with("days", days),
                )
                .fix(fix),
            ];
        }
        if days > snapshots.max_age_days {
            return vec![
                Finding::new(
                    SNAPSHOTS,
                    "",
                    Level::Warning,
                    Message::new(
                        "health-snapshots-old",
                        "The last catalogue snapshot is { $days } days old",
                    )
                    .with("days", days),
                    Message::new(
                        "health-snapshots-file-detail",
                        "{ $file }, { $days } days old",
                    )
                    .with("file", file)
                    .with("days", days),
                )
                .fix(fix),
            ];
        }
        vec![Finding::new(
            SNAPSHOTS,
            "",
            Level::Ok,
            Message::new(
                "health-snapshots-ok",
                "The last catalogue snapshot is recent and restorable",
            ),
            Message::new(
                "health-snapshots-file-detail",
                "{ $file }, { $days } days old",
            )
            .with("file", file)
            .with("days", days),
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400;

    fn ctx(snapshots: Option<Snapshots>) -> Context {
        Context {
            vault: Some(PathBuf::from("/lib/vault.sqlite")),
            snapshots,
            ..Context::default()
        }
    }

    fn snap(taken: u64, verified: bool, restorable: bool) -> Snapshot {
        Snapshot {
            file: PathBuf::from(format!("/lib/snapshots/{taken}.sqlite")),
            taken,
            verified: verified.then_some(taken),
            restorable,
        }
    }

    #[test]
    fn none_yet_is_one_info_line_and_phase_seven_fills_the_data() {
        let found = SnapshotsCheck.run(&ctx(None));
        assert_eq!(found[0].level, Level::Ok);
        assert_eq!(found[0].message.key, "health-snapshots-none-yet");
        let level = |list: Vec<Snapshot>| {
            SnapshotsCheck.run(&ctx(Some(Snapshots {
                list,
                now: 30 * DAY,
                max_age_days: MAX_AGE_DAYS,
            })))[0]
                .level
        };
        assert_eq!(level(vec![snap(29 * DAY, true, true)]), Level::Ok);
        assert_eq!(level(vec![snap(10 * DAY, true, true)]), Level::Warning);
        assert_eq!(level(vec![snap(29 * DAY, true, false)]), Level::Problem);
        assert_eq!(level(vec![snap(29 * DAY, false, true)]), Level::Problem);
        assert!(SnapshotsCheck.run(&Context::default()).is_empty());
    }
}
