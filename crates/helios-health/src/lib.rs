//! Helios's health check: small checks of the setup, run in the
//! background, whose findings the desktop lists (Help > Health check) and
//! sums up in a status bar badge.
//!
//! - A [`Check`] has an id, a title and a [`Check::run`] over a
//!   [`Context`] (the volumes mounted with their roles, the library's
//!   place, the vault's file, its backups, the photos changed outside
//!   Helios), giving [`Finding`]s.
//! - A [`Finding`] has a [`Level`] (ok, warning, problem), a message and a
//!   detail line for people ([`Message`], translated by the desktop), and
//!   maybe a [`Fix`]: a command with its arguments.
//! - The [`Registry`] holds the checks: Helios's own ([`checks`],
//!   [`backup`], [`gpu`], [`plugins`], [`snapshots`]), and
//!   later those plugins bring (the seam is [`Registry::add`]).
//! - [`Findings`] keeps the last findings in memory beside the
//!   [`Ignored`] list, saved in a small JSON file and undoable.
//! - [`runner`] runs every check as one background job, at start (after a
//!   delay), daily and when a volume comes or goes.

pub mod backup;
pub mod checks;
pub mod gpu;
pub mod plugins;
pub mod runner;
pub mod snapshots;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::Message;
use helios_storage::Volume;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// How a finding stands, the worst last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Ok,
    Warning,
    Problem,
}

/// What fixes a finding: a command of the desktop, with its arguments
/// ("open the folder", "run repair"), and the button's words.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fix {
    pub command: String,
    #[serde(default)]
    pub args: Json,
    pub label: Message,
}

/// What a check found.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Finding {
    /// Its id, stable between runs: the check's id, then what it is about
    /// (`storage.free-space/uuid:…`). What "Ignore" keeps.
    pub id: String,
    /// The check that found it.
    pub check: String,
    pub level: Level,
    /// One line for people.
    pub message: Message,
    /// The detail under it (numbers, a path, what to do).
    pub detail: Message,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

impl Finding {
    pub fn new(check: &str, about: &str, level: Level, message: Message, detail: Message) -> Self {
        let id = if about.is_empty() {
            check.to_string()
        } else {
            format!("{check}/{about}")
        };
        Self {
            id,
            check: check.to_string(),
            level,
            message,
            detail,
            fix: None,
        }
    }

    pub fn fix(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }
}

/// What a check reads: gathered once per run, off the interface thread.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// The volumes mounted now, with the roles the vault gives them.
    pub volumes: Vec<Volume>,
    /// The library's folder, when known: the volume holding it is the
    /// library's.
    pub library: Option<PathBuf>,
    /// The vault's database file, when there is one.
    pub vault: Option<PathBuf>,
    /// The library's backups, read from the vault; `None` without one
    /// (the backup checks then say nothing).
    pub backup: Option<backup::Backup>,
    /// The adapters wgpu sees and the one Helios renders on; `None` when
    /// unknown (the GPU checks then say nothing).
    pub gpu: Option<gpu::Gpu>,
    /// The plugins installed, each probed (started, handshake answered).
    pub plugins: Vec<plugins::Plugin>,
    /// The catalogue's snapshots; `None` until Helios takes them (the
    /// check then says there are none yet).
    pub snapshots: Option<snapshots::Snapshots>,
    /// How many Library photos are marked changed outside Helios (their
    /// file rewritten by another application); `None` without a vault.
    pub changed_outside: Option<usize>,
    /// What the node read of the vault's database file (it opens, its
    /// integrity); `None` when not read (the library and integrity
    /// checks then say nothing).
    pub database: Option<checks::Database>,
}

/// One check of the setup.
pub trait Check: Send + Sync {
    /// A stable id (`storage.free-space`), prefixing its findings' ids.
    fn id(&self) -> &str;
    /// Its name for people.
    fn title(&self) -> Message;
    /// What it finds now. A check with nothing to say returns nothing;
    /// one that cannot look says so as a warning.
    fn run(&self, ctx: &Context) -> Vec<Finding>;
}

/// The checks the runner runs, in order. Helios's own are
/// [`Registry::builtin`]; a plugin's will be added through
/// [`Registry::add`] once the plugin protocol carries them.
#[derive(Clone, Default)]
pub struct Registry {
    checks: Vec<Arc<dyn Check>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.checks.iter().map(|c| c.id().to_string()))
            .finish()
    }
}

impl Registry {
    /// Helios's own checks; the GPU kernels check is added by the desktop,
    /// which knows the kernels ([`gpu::Kernels::new`]).
    pub fn builtin() -> Self {
        let mut r = Self::default();
        r.add(Arc::new(checks::FreeSpace::default()));
        r.add(Arc::new(checks::VaultIntegrity));
        r.add(Arc::new(checks::Library::default()));
        r.add(Arc::new(checks::ChangedOutside));
        r.add(Arc::new(backup::StrategySet));
        r.add(Arc::new(backup::StrategyMet));
        r.add(Arc::new(backup::DestinationsSeen));
        r.add(Arc::new(backup::Unverified));
        r.add(Arc::new(backup::Cards));
        r.add(Arc::new(gpu::AdapterCheck));
        r.add(Arc::new(plugins::Plugins));
        r.add(Arc::new(snapshots::SnapshotsCheck));
        r
    }

    /// Adds a check; one with the id of a check already there replaces it.
    pub fn add(&mut self, check: Arc<dyn Check>) {
        self.checks.retain(|c| c.id() != check.id());
        self.checks.push(check);
    }

    pub fn checks(&self) -> &[Arc<dyn Check>] {
        &self.checks
    }

    /// Runs every check; a check that panics gives a problem of its own
    /// rather than stopping the others.
    pub fn run(&self, ctx: &Context) -> Vec<Finding> {
        self.checks.iter().flat_map(|c| run_one(c, ctx)).collect()
    }
}

/// Runs one check, a panic turned into a finding.
pub fn run_one(check: &Arc<dyn Check>, ctx: &Context) -> Vec<Finding> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check.run(ctx))).unwrap_or_else(|_| {
        vec![Finding::new(
            check.id(),
            "",
            Level::Problem,
            Message::new("health-check-crashed", "The check { $check } failed")
                .with("check", check.title()),
            Message::new(
                "health-check-crashed-detail",
                "It stopped before it could say anything.",
            ),
        )]
    })
}

/// The findings the user chose to ignore, by id, kept in a small JSON
/// file when given one.
#[derive(Clone, Debug, Default)]
pub struct Ignored {
    ids: BTreeSet<String>,
    file: Option<PathBuf>,
}

impl Ignored {
    /// Reads the list kept in `file` (none yet is an empty list).
    pub fn load(file: &Path) -> Self {
        let ids = std::fs::read_to_string(file)
            .ok()
            .and_then(|t| serde_json::from_str::<BTreeSet<String>>(&t).ok())
            .unwrap_or_default();
        Self {
            ids,
            file: Some(file.to_path_buf()),
        }
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Ignores `id`, or takes it back (`on` false); saved at once.
    pub fn set(&mut self, id: &str, on: bool) -> Result<(), String> {
        let changed = if on {
            self.ids.insert(id.to_string())
        } else {
            self.ids.remove(id)
        };
        if changed { self.save() } else { Ok(()) }
    }

    fn save(&self) -> Result<(), String> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(&self.ids).map_err(|e| e.to_string())?;
        std::fs::write(file, text).map_err(|e| format!("{}: {e}", file.display()))
    }
}

/// The last findings, and the ignored list.
#[derive(Clone, Debug, Default)]
pub struct Findings {
    all: Vec<Finding>,
    pub ignored: Ignored,
    /// Whether a run came through yet.
    ran: bool,
}

impl Findings {
    pub fn with_ignored(ignored: Ignored) -> Self {
        Self {
            ignored,
            ..Self::default()
        }
    }

    /// Takes a run's findings in place of the last ones.
    pub fn replace(&mut self, findings: Vec<Finding>) {
        self.all = findings;
        self.ran = true;
    }

    pub fn ran(&self) -> bool {
        self.ran
    }

    /// The findings not ignored, the worst first (in run order within a
    /// level).
    pub fn shown(&self) -> Vec<&Finding> {
        let mut shown: Vec<&Finding> = self
            .all
            .iter()
            .filter(|f| !self.ignored.contains(&f.id))
            .collect();
        shown.sort_by_key(|f| std::cmp::Reverse(f.level));
        shown
    }

    /// The findings ignored that the last run still found.
    pub fn ignored_findings(&self) -> Vec<&Finding> {
        self.all
            .iter()
            .filter(|f| self.ignored.contains(&f.id))
            .collect()
    }

    /// The worst level shown that needs attention, and how many findings
    /// need attention (warnings and problems): `None` when all is ok.
    pub fn worst(&self) -> Option<(Level, usize)> {
        let attention: Vec<Level> = self
            .shown()
            .iter()
            .map(|f| f.level)
            .filter(|l| *l > Level::Ok)
            .collect();
        let worst = attention.iter().max()?;
        Some((*worst, attention.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(id: &str, level: Level) -> Finding {
        Finding::new("test", id, level, Message::plain(id), Message::plain(""))
    }

    #[test]
    fn levels_order_the_worst_last() {
        assert!(Level::Ok < Level::Warning);
        assert!(Level::Warning < Level::Problem);
    }

    #[test]
    fn the_worst_counts_warnings_and_problems_only() {
        let mut f = Findings::default();
        assert_eq!(f.worst(), None);
        f.replace(vec![finding("a", Level::Ok)]);
        assert_eq!(f.worst(), None, "all ok: no badge");
        f.replace(vec![
            finding("a", Level::Ok),
            finding("b", Level::Warning),
            finding("c", Level::Problem),
        ]);
        assert_eq!(f.worst(), Some((Level::Problem, 2)));
        let shown: Vec<&str> = f.shown().iter().map(|f| f.id.as_str()).collect();
        assert_eq!(shown, ["test/c", "test/b", "test/a"]);
    }

    #[test]
    fn ignoring_hides_a_finding_and_undoing_brings_it_back() {
        let dir = std::env::temp_dir().join(format!("helios-health-{}", std::process::id()));
        let file = dir.join("ignored.json");
        let _ = std::fs::remove_file(&file);
        let mut f = Findings::with_ignored(Ignored::load(&file));
        f.replace(vec![
            finding("b", Level::Warning),
            finding("c", Level::Problem),
        ]);
        f.ignored.set("test/c", true).unwrap();
        assert_eq!(f.worst(), Some((Level::Warning, 1)));
        assert_eq!(f.ignored_findings().len(), 1);
        // Kept in the file, read back by the next run.
        assert!(Ignored::load(&file).contains("test/c"));
        f.ignored.set("test/c", false).unwrap();
        assert_eq!(f.worst(), Some((Level::Problem, 2)));
        assert!(Ignored::load(&file).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct Panics;
    impl Check for Panics {
        fn id(&self) -> &'static str {
            "panics"
        }
        fn title(&self) -> Message {
            Message::plain("Panics")
        }
        fn run(&self, _: &Context) -> Vec<Finding> {
            panic!("on purpose")
        }
    }

    #[test]
    fn a_check_that_panics_is_a_problem_and_the_others_still_run() {
        let mut r = Registry::default();
        r.add(Arc::new(Panics));
        r.add(Arc::new(checks::FreeSpace::default()));
        let found = r.run(&Context::default());
        assert_eq!(found[0].id, "panics");
        assert_eq!(found[0].level, Level::Problem);
    }
}
