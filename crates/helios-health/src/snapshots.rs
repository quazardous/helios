//! The catalogue snapshots check: the last verified snapshot of the
//! catalogue is recent and restorable. Until snapshots exist, the desktop
//! gives none ([`Context::snapshots`] is `None`) and the check says so in
//! one line; snapshots, once taken, only fill a [`Snapshots`].

use std::path::PathBuf;
use std::sync::Arc;

use helios_core::Message;
use serde_json::json;

use crate::{Check, Context, Declaration, Finding, Fix, Level};

/// The owner of the snapshots' check: the catalogue.
pub const OWNER: &str = "catalogue";

/// The catalogue's checks: its last verified snapshot.
pub fn checks() -> Vec<Arc<dyn Check>> {
    vec![Arc::new(SnapshotsCheck)]
}

pub const SNAPSHOTS: &str = "catalogue.snapshots";

/// Days the last verified snapshot may be old before a warning, by
/// default.
pub const MAX_AGE_DAYS: u64 = 7;

/// The desktop's command taking a snapshot now: the fix of a library
/// with none yet, or none verified.
pub const SNAPSHOT_NOW: &str = "library.catalogue.snapshot";

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

/// The last verified snapshot: not restorable is a problem; older than
/// `max_age_days`, or snapshots none of which is verified yet, a warning.
/// A library with no snapshot yet (a new one: the desktop takes the first
/// a few minutes after it starts) is fine, with "snapshot now" offered.
#[derive(Clone, Copy, Debug, Default)]
pub struct SnapshotsCheck;

impl Check for SnapshotsCheck {
    fn id(&self) -> &str {
        SNAPSHOTS
    }

    fn title(&self) -> Message {
        Message::new("health-snapshots", "Catalogue snapshots")
    }

    fn declaration(&self) -> Declaration {
        Declaration::new("The catalogue's last verified snapshot: present, restorable and recent")
            .levels(&[Level::Ok, Level::Warning, Level::Problem])
            .fixes(&["library.openFolder", SNAPSHOT_NOW])
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
        let snapshot_now = || Fix {
            command: SNAPSHOT_NOW.into(),
            args: json!({}),
            label: Message::new("health-fix-snapshot-now", "Snapshot the catalogue now"),
        };
        if snapshots.list.is_empty() {
            return vec![
                Finding::new(
                    SNAPSHOTS,
                    "",
                    Level::Ok,
                    Message::new("health-snapshots-first", "No catalogue snapshot yet"),
                    Message::new(
                        "health-snapshots-first-detail",
                        "Helios takes the first a few minutes after it starts, then on its schedule.",
                    ),
                )
                .fix(snapshot_now()),
            ];
        }
        let Some(last) = snapshots.last_verified() else {
            return vec![
                Finding::new(
                    SNAPSHOTS,
                    "",
                    Level::Warning,
                    Message::new(
                        "health-snapshots-unverified",
                        "No catalogue snapshot is verified yet",
                    ),
                    Message::new(
                        "health-snapshots-unverified-detail",
                        "{ $count } snapshots, none read back yet: a new one is verified as it is taken.",
                    )
                    .with("count", snapshots.list.len()),
                )
                .fix(snapshot_now()),
            ];
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

    fn run(list: Vec<Snapshot>) -> Finding {
        SnapshotsCheck
            .run(&ctx(Some(Snapshots {
                list,
                now: 30 * DAY,
                max_age_days: MAX_AGE_DAYS,
            })))
            .remove(0)
    }

    #[test]
    fn none_yet_is_one_info_line_and_phase_seven_fills_the_data() {
        let found = SnapshotsCheck.run(&ctx(None));
        assert_eq!(found[0].level, Level::Ok);
        assert_eq!(found[0].message.key, "health-snapshots-none-yet");
        let level = |list| run(list).level;
        assert_eq!(level(vec![snap(29 * DAY, true, true)]), Level::Ok);
        assert_eq!(level(vec![snap(10 * DAY, true, true)]), Level::Warning);
        assert_eq!(level(vec![snap(29 * DAY, true, false)]), Level::Problem);
        assert!(SnapshotsCheck.run(&Context::default()).is_empty());
    }

    #[test]
    fn a_new_library_without_snapshots_is_fine_and_offers_one_now() {
        let found = run(Vec::new());
        assert_eq!(found.level, Level::Ok);
        assert_eq!(found.message.key, "health-snapshots-first");
        assert_eq!(found.fix.map(|f| f.command).as_deref(), Some(SNAPSHOT_NOW));
    }

    #[test]
    fn snapshots_none_verified_yet_warn_with_snapshot_now() {
        let found = run(vec![snap(29 * DAY, false, true)]);
        assert_eq!(found.level, Level::Warning);
        assert_eq!(found.message.key, "health-snapshots-unverified");
        assert_eq!(found.fix.map(|f| f.command).as_deref(), Some(SNAPSHOT_NOW));
    }

    #[test]
    fn a_verified_recent_snapshot_is_ok_and_every_finding_fits_the_declaration() {
        let found = run(vec![snap(29 * DAY, true, true)]);
        assert_eq!(found.level, Level::Ok);
        assert_eq!(found.message.key, "health-snapshots-ok");
        let declared = SnapshotsCheck.declaration();
        for list in [
            Vec::new(),
            vec![snap(29 * DAY, false, true)],
            vec![snap(29 * DAY, true, true)],
            vec![snap(10 * DAY, true, true)],
            vec![snap(29 * DAY, true, false)],
        ] {
            let found = run(list);
            assert_eq!(declared.breach(SNAPSHOTS, &found), None);
            assert!(found.level == Level::Ok || found.fix.is_some());
        }
    }
}
