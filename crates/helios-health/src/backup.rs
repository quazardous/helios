//! The backup checks: a strategy is set, every photo meets it, its
//! destinations were seen lately, the copies are verified, and the cards
//! still inserted are safe to format. They read a [`Backup`], gathered
//! from the vault once per run ([`Backup::read`]).

use helios_core::Message;
use helios_storage::strategy::{self, BackupStatus, Scope, Strategy};
use helios_storage::{Volume, registry};
use helios_vault::{Store, Vault};
use serde_json::json;

use crate::{Check, Context, Finding, Fix, Level};

/// Days a backup destination may stay away before a warning, by default.
pub const DESTINATION_DAYS: u64 = 14;

const DAY: u64 = 86_400;

/// A card an import read, still inserted, and how many of its photos are
/// short of their backups.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardShort {
    pub label: String,
    pub short: u64,
}

/// The library's backups as the checks read them.
#[derive(Clone, Debug, Default)]
pub struct Backup {
    /// The library's strategy; `None` when there is none.
    pub strategy: Option<Strategy>,
    /// Every volume the vault knows, those plugged in now with their
    /// mount point.
    pub volumes: Vec<Volume>,
    /// Every photo's backup status against the strategy.
    pub statuses: Vec<BackupStatus>,
    /// The cards still inserted that an import left short of backups.
    pub cards: Vec<CardShort>,
    /// When it was read, seconds since the epoch.
    pub now: u64,
    /// Days a destination may stay away before a warning.
    pub destination_days: u64,
}

impl Backup {
    /// Reads the strategy, the volumes known (with those `mounted`) and
    /// every photo's status from `vault` at `now`.
    pub fn read<S: Store>(vault: &Vault<S>, mounted: &[Volume], now: u64) -> Result<Self, String> {
        let strategy = strategy::load(vault, &Scope::Library)?;
        let volumes = registry::merged(vault, mounted)?;
        let statuses = strategy::statuses(vault, &volumes, strategy.as_ref())?
            .into_values()
            .map(|(s, _)| s)
            .collect();
        Ok(Self {
            strategy,
            volumes,
            statuses,
            cards: Vec::new(),
            now,
            destination_days: DESTINATION_DAYS,
        })
    }

    /// Whether a destination of the strategy is plugged in now.
    fn destination_online(&self) -> bool {
        self.strategy
            .as_ref()
            .is_some_and(|s| !strategy::destinations_online(s, &self.volumes).is_empty())
    }
}

/// The strategy, when the run read the backups and one is set.
fn strategy_of(ctx: &Context) -> Option<(&Backup, &Strategy)> {
    let b = ctx.backup.as_ref()?;
    Some((b, b.strategy.as_ref()?))
}

/// The Storage page, where the strategy editor is at the top.
fn open_storage(label: Message) -> Fix {
    Fix {
        command: "library.storage".into(),
        args: json!({}),
        label,
    }
}

fn back_up_now(label: Message) -> Fix {
    Fix {
        command: "library.backupNow".into(),
        args: json!({}),
        label,
    }
}

fn back_up_now_label() -> Message {
    Message::new("health-fix-back-up-now", "Back up now")
}

pub const STRATEGY_SET: &str = "backup.strategy";

/// A backup strategy is set.
#[derive(Clone, Copy, Debug, Default)]
pub struct StrategySet;

impl Check for StrategySet {
    fn id(&self) -> &str {
        STRATEGY_SET
    }

    fn title(&self) -> Message {
        Message::new("health-strategy", "Backup strategy")
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let Some(backup) = &ctx.backup else {
            return Vec::new();
        };
        let finding = match &backup.strategy {
            None => Finding::new(
                STRATEGY_SET,
                "",
                Level::Warning,
                Message::new("health-strategy-none", "No backup is set up"),
                Message::new(
                    "health-strategy-none-detail",
                    "Each photo has one copy, in the Library: a dead disk loses it.",
                ),
            )
            .fix(Fix {
                // The Settings app at Backup › Strategy.
                command: "admin.open".into(),
                args: json!({ "path": "backup/strategy" }),
                label: Message::new("health-fix-set-strategy", "Set up a backup strategy"),
            }),
            Some(s) => Finding::new(
                STRATEGY_SET,
                "",
                Level::Ok,
                Message::new("health-strategy-set", "Backups follow { $name }")
                    .with("name", s.name.clone()),
                Message::new(
                    "health-strategy-set-detail",
                    "Verified copies per photo: { $copies }",
                )
                .with("copies", u64::from(s.copies)),
            ),
        };
        vec![finding]
    }
}

pub const STRATEGY_MET: &str = "backup.met";

/// Every photo has the copies its strategy asks for.
#[derive(Clone, Copy, Debug, Default)]
pub struct StrategyMet;

impl Check for StrategyMet {
    fn id(&self) -> &str {
        STRATEGY_MET
    }

    fn title(&self) -> Message {
        Message::new("health-met", "Photos backed up")
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let Some((backup, s)) = strategy_of(ctx) else {
            return Vec::new();
        };
        let short = backup.statuses.iter().filter(|st| !st.met()).count();
        if short == 0 {
            return vec![Finding::new(
                STRATEGY_MET,
                "",
                Level::Ok,
                Message::new("health-met-ok", "Every photo has its backups"),
                Message::new("health-met-ok-detail", "Photos checked: { $count }")
                    .with("count", backup.statuses.len()),
            )];
        }
        let fix = if backup.destination_online() {
            back_up_now(back_up_now_label())
        } else {
            open_storage(Message::new(
                "health-fix-no-destination",
                "No backup disk is plugged in: open the Storage page",
            ))
        };
        vec![
            Finding::new(
                STRATEGY_MET,
                "",
                Level::Warning,
                Message::new(
                    "health-met-short",
                    "Photos short of their backups: { $count }",
                )
                .with("count", short),
                Message::new(
                    "health-met-short-detail",
                    "Verified copies { $name } asks for per photo: { $copies }",
                )
                .with("name", s.name.clone())
                .with("copies", u64::from(s.copies)),
            )
            .fix(fix),
        ]
    }
}

pub const DESTINATIONS_SEEN: &str = "backup.destinations-seen";

/// The strategy's destinations were plugged in lately.
#[derive(Clone, Copy, Debug, Default)]
pub struct DestinationsSeen;

impl Check for DestinationsSeen {
    fn id(&self) -> &str {
        DESTINATIONS_SEEN
    }

    fn title(&self) -> Message {
        Message::new("health-seen", "Backup disks seen lately")
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let Some((backup, s)) = strategy_of(ctx) else {
            return Vec::new();
        };
        let destinations: Vec<&Volume> = backup
            .volumes
            .iter()
            .filter(|v| s.destinations.iter().any(|d| d.takes(v)))
            .filter(|v| strategy::may_back_up_to(v).is_ok())
            .collect();
        if destinations.is_empty() {
            return Vec::new();
        }
        let limit = backup.destination_days;
        let stale: Vec<Finding> = destinations
            .iter()
            .filter(|v| !v.online())
            .filter_map(|v| {
                let at = v.last_seen.as_ref()?.at;
                let days = backup.now.saturating_sub(at) / DAY;
                (days > limit).then(|| {
                    Finding::new(
                        DESTINATIONS_SEEN,
                        v.id.as_str(),
                        Level::Warning,
                        Message::new(
                            "health-seen-stale",
                            "{ $volume } was last seen { $days } days ago",
                        )
                        .with("volume", v.name.clone())
                        .with("days", days),
                        Message::new(
                            "health-seen-stale-detail",
                            "Its backups are not kept up to date while it stays away (more than { $limit } days).",
                        )
                        .with("limit", limit),
                    )
                    .fix(Fix {
                        command: "library.storage".into(),
                        args: json!({ "volume": v.id.as_str() }),
                        label: Message::new(
                            "health-fix-plug-in",
                            "Plug in { $volume } to back up",
                        )
                        .with("volume", v.name.clone()),
                    })
                })
            })
            .collect();
        if !stale.is_empty() {
            return stale;
        }
        vec![Finding::new(
            DESTINATIONS_SEEN,
            "",
            Level::Ok,
            Message::new("health-seen-ok", "Every backup disk was seen lately"),
            Message::new("health-seen-ok-detail", "Within the last { $limit } days")
                .with("limit", limit),
        )]
    }
}

pub const UNVERIFIED: &str = "backup.unverified";

/// Backup copies without a verification record.
#[derive(Clone, Copy, Debug, Default)]
pub struct Unverified;

impl Check for Unverified {
    fn id(&self) -> &str {
        UNVERIFIED
    }

    fn title(&self) -> Message {
        Message::new("health-unverified", "Verified copies")
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let Some((backup, _)) = strategy_of(ctx) else {
            return Vec::new();
        };
        let count = backup.statuses.iter().filter(|s| s.unverified > 0).count();
        if count == 0 {
            return Vec::new();
        }
        vec![
            Finding::new(
                UNVERIFIED,
                "",
                Level::Warning,
                Message::new(
                    "health-unverified-some",
                    "Photos with backup copies not verified: { $count }",
                )
                .with("count", count),
                Message::new(
                    "health-unverified-detail",
                    "A copy counts once it is read back and matches its hash.",
                ),
            )
            .fix(back_up_now(Message::new(
                "health-fix-verify",
                "Back up now, which verifies them",
            ))),
        ]
    }
}

pub const CARDS: &str = "backup.cards";

/// The cards still inserted that are not safe to format yet.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cards;

impl Check for Cards {
    fn id(&self) -> &str {
        CARDS
    }

    fn title(&self) -> Message {
        Message::new("health-cards", "Cards safe to format")
    }

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let Some(backup) = &ctx.backup else {
            return Vec::new();
        };
        backup
            .cards
            .iter()
            .filter(|c| c.short > 0)
            .map(|c| {
                Finding::new(
                    CARDS,
                    &c.label,
                    Level::Warning,
                    Message::new("health-card-short", "{ $card } is not safe to format yet")
                        .with("card", c.label.clone()),
                    Message::new(
                        "health-card-short-detail",
                        "Photos on it short of their backups: { $count }. Keep the card.",
                    )
                    .with("count", c.short),
                )
                .fix(back_up_now(back_up_now_label()))
            })
            .collect()
    }
}
