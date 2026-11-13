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
//! - Each check declares what it watches, the levels it may raise and
//!   the fixes it may offer ([`contract::Declaration`]); its findings are
//!   held to it.
//! - The [`Registry`] holds the checks by owner (the module or the
//!   plugin that declares them): Helios's own, each module listing its
//!   checks ([`checks`], [`backup`], [`gpu`], [`plugins`],
//!   [`snapshots`]), and those plugins declare in their manifest, run
//!   through their process (`helios_plugin::health`); the seam is
//!   [`Registry::add`], which refuses an unsound declaration.
//! - [`Findings`] keeps the last findings in memory beside the
//!   [`Ignored`] list, saved in a small JSON file and undoable.
//! - [`runner`] runs every check as one background job, at start (after a
//!   delay), daily and when a volume comes or goes.

pub mod backup;
pub mod checks;
pub mod contract;
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

pub use contract::Declaration;

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
    /// What it watches, the levels it may raise and the fixes it may
    /// offer: its findings are held to it.
    fn declaration(&self) -> Declaration;
    /// What it finds now. A check with nothing to say returns nothing;
    /// one that cannot look says so as a warning.
    fn run(&self, ctx: &Context) -> Vec<Finding>;
}

/// The checks the runner runs, in order, each with its owner: the module
/// or the plugin that declares it. Helios's own are
/// [`Registry::builtin`]; a plugin's are added through [`Registry::add`]
/// by the plugin host (`helios_plugin::health`), its owner the plugin's
/// id.
#[derive(Clone, Default)]
pub struct Registry {
    checks: Vec<Arc<dyn Check>>,
    /// The owner of each check, by position.
    owners: Vec<String>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(
                self.checks
                    .iter()
                    .zip(&self.owners)
                    .map(|(c, o)| format!("{o}: {}", c.id())),
            )
            .finish()
    }
}

impl Registry {
    /// Helios's own checks, each module's; the GPU kernels check is added
    /// by the desktop, which knows the kernels ([`gpu::Kernels::new`]).
    pub fn builtin() -> Self {
        let mut r = Self::default();
        let owned = [
            (checks::STORAGE_OWNER, checks::storage()),
            (checks::LIBRARY_OWNER, checks::library()),
            (backup::OWNER, backup::checks()),
            (gpu::OWNER, gpu::checks()),
            (plugins::OWNER, plugins::checks()),
            (snapshots::OWNER, snapshots::checks()),
        ];
        for (owner, checks) in owned {
            for check in checks {
                if let Err(problems) = r.add(owner, check) {
                    // Helios's own declarations are tested sound.
                    log::error!("health: {}", problems.join("; "));
                }
            }
        }
        r
    }

    /// Adds `owner`'s check; one with the id of a check already there
    /// replaces it. A check whose declaration is unsound is refused, with
    /// its reasons.
    pub fn add(&mut self, owner: &str, check: Arc<dyn Check>) -> Result<(), Vec<String>> {
        let mut problems = check.declaration().problems(check.id());
        if !contract::sound_name(owner) {
            problems.insert(
                0,
                format!("owner `{owner}`: lowercase letters, digits, `.`, `-` and `_`"),
            );
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        if let Some(at) = self.checks.iter().position(|c| c.id() == check.id()) {
            self.checks.remove(at);
            self.owners.remove(at);
        }
        self.checks.push(check);
        self.owners.push(owner.to_owned());
        Ok(())
    }

    pub fn checks(&self) -> &[Arc<dyn Check>] {
        &self.checks
    }

    /// The owner of the check `id`.
    pub fn owner(&self, id: &str) -> Option<&str> {
        let at = self.checks.iter().position(|c| c.id() == id)?;
        Some(&self.owners[at])
    }

    /// The checks by owner, the owners in the order their first check
    /// came.
    pub fn by_owner(&self) -> Vec<(&str, Vec<&Arc<dyn Check>>)> {
        let mut out: Vec<(&str, Vec<&Arc<dyn Check>>)> = Vec::new();
        for (check, owner) in self.checks.iter().zip(&self.owners) {
            match out.iter_mut().find(|(o, _)| o == owner) {
                Some((_, checks)) => checks.push(check),
                None => out.push((owner, vec![check])),
            }
        }
        out
    }

    /// Runs every check; a check that panics, or answers outside its
    /// declaration, gives a problem of its own rather than stopping the
    /// others.
    pub fn run(&self, ctx: &Context) -> Vec<Finding> {
        self.checks.iter().flat_map(|c| run_one(c, ctx)).collect()
    }
}

/// Runs one check, a panic or a finding outside its declaration turned
/// into a problem.
pub fn run_one(check: &Arc<dyn Check>, ctx: &Context) -> Vec<Finding> {
    let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check.run(ctx)));
    let declaration = check.declaration();
    match found {
        Ok(found) => {
            let breach = found
                .iter()
                .filter(|f| !(is_failure(f) && f.check == check.id()))
                .find_map(|f| declaration.breach(check.id(), f));
            match breach {
                None => found,
                Some(why) => {
                    log::warn!("health: {why}");
                    vec![crashed(check)]
                }
            }
        }
        Err(_) => vec![crashed(check)],
    }
}

/// The problem a check that failed gives in place of its findings.
fn crashed(check: &Arc<dyn Check>) -> Finding {
    failed(check.id(), check.title())
}

/// The key of the problem a check that failed gives.
pub const FAILED: &str = "health-check-crashed";

/// The problem the check `id`, named `title`, gives when it failed (it
/// panicked, its plugin crashed or did not answer in time) in place of
/// its findings. A check may always give it, whatever it declares.
pub fn failed(id: &str, title: Message) -> Finding {
    Finding::new(
        id,
        "",
        Level::Problem,
        Message::new("health-check-crashed", "The check { $check } failed").with("check", title),
        Message::new(
            "health-check-crashed-detail",
            "It stopped before it could say anything.",
        ),
    )
}

/// Whether `finding` is the problem of its check having failed
/// ([`failed`]).
fn is_failure(finding: &Finding) -> bool {
    finding.level == Level::Problem && finding.message.key == FAILED && finding.id == finding.check
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
        fn declaration(&self) -> Declaration {
            Declaration::new("Nothing").levels(&[Level::Ok])
        }
        fn run(&self, _: &Context) -> Vec<Finding> {
            panic!("on purpose")
        }
    }

    #[test]
    fn a_check_that_panics_is_a_problem_and_the_others_still_run() {
        let mut r = Registry::default();
        r.add("test", Arc::new(Panics)).unwrap();
        r.add("storage", Arc::new(checks::FreeSpace::default()))
            .unwrap();
        let found = r.run(&Context::default());
        assert_eq!(found[0].id, "panics");
        assert_eq!(found[0].level, Level::Problem);
    }

    /// A module's check: declared at `levels`, it finds `level` with the
    /// fix `fix`.
    struct Fake {
        id: &'static str,
        levels: &'static [Level],
        level: Level,
        fix: Option<&'static str>,
    }

    impl Check for Fake {
        fn id(&self) -> &str {
            self.id
        }
        fn title(&self) -> Message {
            Message::plain(self.id)
        }
        fn declaration(&self) -> Declaration {
            Declaration::new("A fake module's state")
                .levels(self.levels)
                .fixes(&["fake.repair"])
        }
        fn run(&self, _: &Context) -> Vec<Finding> {
            let f = Finding::new(
                self.id,
                "",
                self.level,
                Message::plain("m"),
                Message::plain("d"),
            );
            vec![match self.fix {
                Some(command) => f.fix(Fix {
                    command: command.into(),
                    args: Json::Null,
                    label: Message::plain("Repair"),
                }),
                None => f,
            }]
        }
    }

    fn fake(id: &'static str, level: Level, fix: Option<&'static str>) -> Arc<dyn Check> {
        Arc::new(Fake {
            id,
            levels: &[Level::Ok, Level::Warning],
            level,
            fix,
        })
    }

    #[test]
    fn helios_own_checks_are_declared_soundly_and_owned_by_their_modules() {
        let r = Registry::builtin();
        let owners: Vec<(&str, Vec<&str>)> = r
            .by_owner()
            .into_iter()
            .map(|(o, checks)| (o, checks.iter().map(|c| c.id()).collect()))
            .collect();
        assert_eq!(
            owners,
            [
                ("storage", vec!["storage.free-space"]),
                (
                    "library",
                    vec![
                        "vault.integrity",
                        "library.default",
                        "library.changed-outside"
                    ]
                ),
                (
                    "backup",
                    vec![
                        "backup.strategy",
                        "backup.met",
                        "backup.destinations-seen",
                        "backup.unverified",
                        "backup.cards"
                    ]
                ),
                ("gpu", vec!["gpu.adapter"]),
                ("plugins", vec!["plugins"]),
                ("catalogue", vec!["catalogue.snapshots"]),
            ]
        );
        for check in r.checks() {
            assert_eq!(
                check.declaration().problems(check.id()),
                Vec::<String>::new()
            );
        }
        let kernels = gpu::Kernels::new(Vec::new);
        assert_eq!(
            kernels.declaration().problems(kernels.id()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_module_declares_its_checks_and_their_findings_come_by_owner() {
        let mut r = Registry::default();
        r.add(
            "fake",
            fake("fake.disk", Level::Warning, Some("fake.repair")),
        )
        .unwrap();
        r.add("other", fake("other.cable", Level::Ok, None))
            .unwrap();
        r.add("fake", fake("fake.fan", Level::Ok, None)).unwrap();
        let found = r.run(&Context::default());
        let ids: Vec<&str> = found.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["fake.disk", "other.cable", "fake.fan"]);
        assert_eq!(found[0].fix.as_ref().unwrap().command, "fake.repair");
        let grouped: Vec<(&str, usize)> = r.by_owner().iter().map(|(o, c)| (*o, c.len())).collect();
        assert_eq!(grouped, [("fake", 2), ("other", 1)]);
        assert_eq!(r.owner("fake.fan"), Some("fake"));
        // The same id again replaces the check, and its owner.
        r.add("other", fake("fake.fan", Level::Ok, None)).unwrap();
        assert_eq!(r.owner("fake.fan"), Some("other"));
        assert_eq!(r.checks().len(), 3);
    }

    #[test]
    fn an_unsound_declaration_or_owner_is_refused_with_its_reasons() {
        let mut r = Registry::default();
        let none = Arc::new(Fake {
            id: "fake.disk",
            levels: &[],
            level: Level::Ok,
            fix: None,
        });
        assert_eq!(
            r.add("Fake Module", none),
            Err(vec![
                "owner `Fake Module`: lowercase letters, digits, `.`, `-` and `_`".to_owned(),
                "check `fake.disk`: `levels` names at least one level".to_owned(),
            ])
        );
        assert!(r.checks().is_empty());
    }

    /// A check that says itself it failed (its plugin did not answer).
    struct Fails;
    impl Check for Fails {
        fn id(&self) -> &'static str {
            "fails"
        }
        fn title(&self) -> Message {
            Message::plain("Fails")
        }
        fn declaration(&self) -> Declaration {
            Declaration::new("Nothing").levels(&[Level::Ok])
        }
        fn run(&self, _: &Context) -> Vec<Finding> {
            vec![failed(self.id(), self.title())]
        }
    }

    #[test]
    fn a_check_may_say_it_failed_whatever_it_declares() {
        let mut r = Registry::default();
        r.add("test", Arc::new(Fails)).unwrap();
        let found = r.run(&Context::default());
        assert_eq!(found.len(), 1);
        assert_eq!(
            (
                found[0].id.as_str(),
                found[0].level,
                found[0].message.key.as_str()
            ),
            ("fails", Level::Problem, FAILED)
        );
    }

    #[test]
    fn a_finding_outside_its_declaration_is_a_problem_of_its_check() {
        let mut r = Registry::default();
        r.add("fake", fake("fake.level", Level::Problem, None))
            .unwrap();
        r.add("fake", fake("fake.fix", Level::Ok, Some("plugin.disable")))
            .unwrap();
        r.add("fake", fake("fake.fine", Level::Ok, None)).unwrap();
        let found = r.run(&Context::default());
        let got: Vec<(&str, Level, &str)> = found
            .iter()
            .map(|f| (f.id.as_str(), f.level, f.message.key.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("fake.level", Level::Problem, "health-check-crashed"),
                ("fake.fix", Level::Problem, "health-check-crashed"),
                ("fake.fine", Level::Ok, ""),
            ]
        );
    }
}
