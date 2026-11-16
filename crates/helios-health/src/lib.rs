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
//!   plugin that declares them), each a listener of the health report's
//!   collect point ([`HealthReport`], `health.report`, on egui-pop's hook
//!   kernel). Each module adds its own (`checks::register_storage`,
//!   `backup::register`…, plugins' through their process:
//!   `helios_plugin::health`); the registry lists no module. The seam is
//!   [`Registry::add`], which refuses an unsound declaration; a run
//!   collects what the listeners answer ([`Registry::report`]).
//! - [`Findings`] keeps the last findings in memory per source (each
//!   check, with its owner) beside the [`Ignored`] list, saved in a small
//!   JSON file and undoable.
//! - [`runner`] runs the checks as background jobs: all of them a while
//!   after start, then each as its declaration says (how often, on which
//!   [`Event`]s).

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
use std::time::Duration;

use egui_pop_hooks::{Kernel, ListenOpts, Point, PointSpec};
use helios_core::Message;
use helios_storage::Volume;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

pub use contract::{Declaration, Event};

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
    /// The library folder chosen (where imports copy photos), read from
    /// the vault: `None` when not read (no vault: the check says
    /// nothing), `Some(None)` when none is chosen.
    pub library_folder: Option<Option<PathBuf>>,
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
    /// How many RAW+JPEG pairs the Library shows as two photos (imported
    /// before pairs were one photo); `None` without a vault.
    pub raw_jpeg_pairs: Option<usize>,
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

/// The health report's collect point (`health.report`): each module
/// that knows a health fact answers it with its findings, under its
/// check's id and its own name as owner; the health module only collects,
/// ranks and shows them. A listener answers `None` when its check is not
/// part of this run ([`Report::wants`]).
pub struct HealthReport;

impl Point for HealthReport {
    const ID: &'static str = "health.report";
    type Input = Report;
    type Value = Option<Vec<Finding>>;
}

/// How long a check may take before its findings are dropped as late: it
/// runs on the run's own thread, never the interface's, so the bound is
/// wide (a database's integrity check on a large library).
pub const REPORT_DEADLINE: Duration = Duration::from_mins(10);

/// What a run of the health report hands every listener: what the checks
/// read, which checks run (all of them for none), and who hears each
/// check start (the jobs centre's progress).
pub struct Report {
    pub context: Context,
    pub only: Option<BTreeSet<String>>,
    progress: Option<Progress>,
}

/// Hears each check of a report as it starts, by id.
type Progress = Box<dyn Fn(&str) + Send + Sync>;

impl Report {
    /// Every check, over `context`.
    pub fn all(context: Context) -> Self {
        Self {
            context,
            only: None,
            progress: None,
        }
    }

    /// The checks `ids` only.
    #[must_use]
    pub fn only(mut self, ids: impl IntoIterator<Item = String>) -> Self {
        self.only = Some(ids.into_iter().collect());
        self
    }

    /// Hears each check as it starts, by id.
    #[must_use]
    pub fn progress(mut self, f: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.progress = Some(Box::new(f));
        self
    }

    /// Whether the check `id` runs in this report.
    pub fn wants(&self, id: &str) -> bool {
        self.only.as_ref().is_none_or(|ids| ids.contains(id))
    }
}

/// The checks, each a listener of [`HealthReport`] owned by the module or
/// the plugin that declares it, in the order they were added. A module
/// adds its own ([`Registry::add`]: `checks::register`,
/// `helios_plugin::health::register`…); the registry knows no module, it
/// refuses an unsound declaration and keeps each check's declaration for
/// the schedule. Its hook kernel ([`Registry::kernel`]) shows who answers
/// the point, in order (`egui_pop_hooks::Kernel::listeners`).
pub struct Registry {
    kernel: Arc<Kernel>,
    checks: Vec<Arc<dyn Check>>,
    /// The owner of each check, by position.
    owners: Vec<String>,
}

impl Default for Registry {
    fn default() -> Self {
        let kernel = Kernel::new();
        let declared = kernel.declare::<HealthReport>(
            PointSpec::new(
                "Health findings: each module answers with what it finds of its own state",
            )
            .deadline(REPORT_DEADLINE),
        );
        if let Err(e) = declared {
            log::error!("health: {e}");
        }
        Self {
            kernel: Arc::new(kernel),
            checks: Vec::new(),
            owners: Vec::new(),
        }
    }
}

impl Clone for Registry {
    /// Another registry with the same checks, on a kernel of its own: a
    /// check added to one is not in the other.
    fn clone(&self) -> Self {
        let mut r = Self::default();
        for (owner, check) in self.owned() {
            r.listen(owner, check.clone());
        }
        r.checks.clone_from(&self.checks);
        r.owners.clone_from(&self.owners);
        r
    }
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
    /// Adds `owner`'s check, as a listener of [`HealthReport`]; one with
    /// the id of a check already there replaces it. A check whose
    /// declaration is unsound is refused, with its reasons.
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
            self.kernel.unlisten(HealthReport::ID, check.id());
        }
        self.listen(owner, check.clone());
        self.checks.push(check);
        self.owners.push(owner.to_owned());
        Ok(())
    }

    /// `check` answering the report, owned by `owner`.
    fn listen(&self, owner: &str, check: Arc<dyn Check>) {
        let id = check.id().to_owned();
        let listened = self.kernel.listen::<HealthReport>(
            id.clone(),
            ListenOpts::new().owner(owner),
            move |report, out| {
                if report.wants(&id) {
                    if let Some(progress) = &report.progress {
                        progress(&id);
                    }
                    *out = Some(run_one(&check, &report.context));
                }
                Ok(())
            },
        );
        if let Err(e) = listened {
            log::error!("health: {e}");
        }
    }

    /// Removes every check of `owner` (a module or a plugin gone); how
    /// many went.
    pub fn remove_owner(&mut self, owner: &str) -> usize {
        self.kernel.forget(owner);
        let before = self.checks.len();
        let mut at = 0;
        self.checks.retain(|_| {
            let keep = self.owners[at] != owner;
            at += 1;
            keep
        });
        self.owners.retain(|o| o != owner);
        before - self.checks.len()
    }

    /// The hook kernel the checks listen on.
    pub fn kernel(&self) -> &Arc<Kernel> {
        &self.kernel
    }

    pub fn checks(&self) -> &[Arc<dyn Check>] {
        &self.checks
    }

    /// Each check with its owner, in order.
    pub fn owned(&self) -> impl Iterator<Item = (&str, &Arc<dyn Check>)> {
        self.owners.iter().map(String::as_str).zip(&self.checks)
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

    /// What runs the report on another thread: the kernel and the
    /// checks as they are now.
    pub fn reporter(&self) -> Reporter {
        Reporter {
            kernel: self.kernel.clone(),
            checks: self
                .owned()
                .map(|(o, c)| (o.to_owned(), c.clone()))
                .collect(),
        }
    }

    /// Emits the report ([`Reporter::report`]).
    pub fn report(&self, report: Report) -> Vec<(String, String, Vec<Finding>)> {
        self.reporter().report(report)
    }

    /// Runs every check over `ctx`; its findings in check order.
    pub fn run(&self, ctx: &Context) -> Vec<Finding> {
        self.report(Report::all(ctx.clone()))
            .into_iter()
            .flat_map(|(_, _, found)| found)
            .collect()
    }
}

/// A registry's kernel and checks, held to emit the report off the
/// interface thread ([`Registry::reporter`]).
pub struct Reporter {
    kernel: Arc<Kernel>,
    checks: Vec<(String, Arc<dyn Check>)>,
}

impl Reporter {
    /// Emits the report: each check it wants answers, in order, as
    /// `(owner, check, findings)`. A check that panics, answers outside
    /// its declaration, fails or is late gives a problem of its own
    /// rather than stopping the others.
    pub fn report(&self, report: Report) -> Vec<(String, String, Vec<Finding>)> {
        let collected = self.kernel.collect::<HealthReport>(report);
        let mut answers: Vec<(String, Vec<Finding>)> = collected
            .parts
            .into_iter()
            .filter_map(|(id, found)| Some((id, found?)))
            .collect();
        for failure in collected.failures.iter().filter(|f| f.dropped()) {
            log::warn!("health: {failure}");
            if let Some((_, check)) = self.checks.iter().find(|(_, c)| c.id() == failure.listener) {
                answers.push((failure.listener.clone(), vec![crashed(check)]));
            }
        }
        self.checks
            .iter()
            .filter_map(|(owner, check)| {
                let at = answers.iter().position(|(id, _)| id == check.id())?;
                let (id, found) = answers.swap_remove(at);
                Some((owner.clone(), id, found))
            })
            .collect()
    }
}

/// Adds `checks` to `registry`, owned by `owner`; a refused one is logged
/// (Helios's own declarations are tested sound).
pub fn add_owned(registry: &mut Registry, owner: &str, checks: Vec<Arc<dyn Check>>) {
    for check in checks {
        if let Err(problems) = registry.add(owner, check) {
            log::error!("health: {}", problems.join("; "));
        }
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

/// One check's last findings, with its owner (the module or the plugin
/// that declares it).
#[derive(Clone, Debug)]
pub struct Source {
    pub owner: String,
    pub check: String,
    pub findings: Vec<Finding>,
}

/// The last findings, kept per source (each check, with its owner): a
/// run of some checks only replaces theirs. And the ignored list.
#[derive(Clone, Debug, Default)]
pub struct Findings {
    sources: Vec<Source>,
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

    /// Takes `owner`'s check `check`'s findings in place of its last
    /// ones; a check not seen yet comes after the others.
    pub fn put(&mut self, owner: &str, check: &str, findings: Vec<Finding>) {
        match self.sources.iter_mut().find(|s| s.check == check) {
            Some(s) => {
                owner.clone_into(&mut s.owner);
                s.findings = findings;
            }
            None => self.sources.push(Source {
                owner: owner.to_owned(),
                check: check.to_owned(),
                findings,
            }),
        }
        self.ran = true;
    }

    /// Forgets the findings of the checks `keep` refuses (no longer
    /// registered).
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.sources.retain(|s| keep(&s.check));
    }

    pub fn ran(&self) -> bool {
        self.ran
    }

    /// Each check's findings, in the order the checks first ran.
    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    /// The owner of the check that found `finding`.
    pub fn owner(&self, finding: &Finding) -> Option<&str> {
        self.sources
            .iter()
            .find(|s| s.check == finding.check)
            .map(|s| s.owner.as_str())
    }

    fn all(&self) -> impl Iterator<Item = &Finding> {
        self.sources.iter().flat_map(|s| &s.findings)
    }

    /// The findings not ignored, the worst first (in run order within a
    /// level).
    pub fn shown(&self) -> Vec<&Finding> {
        let mut shown: Vec<&Finding> = self
            .all()
            .filter(|f| !self.ignored.contains(&f.id))
            .collect();
        shown.sort_by_key(|f| std::cmp::Reverse(f.level));
        shown
    }

    /// The findings ignored that the last run still found.
    pub fn ignored_findings(&self) -> Vec<&Finding> {
        self.all()
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
        f.put("o", "test", vec![finding("a", Level::Ok)]);
        assert_eq!(f.worst(), None, "all ok: no badge");
        f.put(
            "o",
            "test",
            vec![
                finding("a", Level::Ok),
                finding("b", Level::Warning),
                finding("c", Level::Problem),
            ],
        );
        assert_eq!(f.worst(), Some((Level::Problem, 2)));
        let shown: Vec<&str> = f.shown().iter().map(|f| f.id.as_str()).collect();
        assert_eq!(shown, ["test/c", "test/b", "test/a"]);
    }

    #[test]
    fn findings_are_kept_per_source_and_a_partial_run_replaces_only_its_own() {
        let found = |check: &str, about: &str, level| {
            Finding::new(
                check,
                about,
                level,
                Message::plain(about),
                Message::plain(""),
            )
        };
        let mut f = Findings::default();
        assert!(!f.ran());
        f.put(
            "disks",
            "disks.space",
            vec![found("disks.space", "a", Level::Warning)],
        );
        f.put(
            "net",
            "net.cable",
            vec![found("net.cable", "b", Level::Problem)],
        );
        assert!(f.ran());
        // A run of one check only: the other's findings stay.
        f.put(
            "disks",
            "disks.space",
            vec![found("disks.space", "a", Level::Ok)],
        );
        let ids: Vec<(&str, Level)> = f.shown().iter().map(|x| (x.id.as_str(), x.level)).collect();
        assert_eq!(
            ids,
            [
                ("net.cable/b", Level::Problem),
                ("disks.space/a", Level::Ok)
            ]
        );
        let net = f.shown()[0];
        assert_eq!(f.owner(net), Some("net"));
        let order: Vec<&str> = f.sources().iter().map(|s| s.owner.as_str()).collect();
        assert_eq!(order, ["disks", "net"]);
        f.retain(|check| check != "net.cable");
        assert_eq!(f.worst(), None, "an unregistered check's findings go");
    }

    #[test]
    fn ignoring_hides_a_finding_and_undoing_brings_it_back() {
        let dir = std::env::temp_dir().join(format!("helios-health-{}", std::process::id()));
        let file = dir.join("ignored.json");
        let _ = std::fs::remove_file(&file);
        let mut f = Findings::with_ignored(Ignored::load(&file));
        f.put(
            "o",
            "test",
            vec![finding("b", Level::Warning), finding("c", Level::Problem)],
        );
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
        let mut r = Registry::default();
        checks::register_storage(&mut r);
        checks::register_library(&mut r);
        backup::register(&mut r);
        gpu::register(&mut r);
        plugins::register(&mut r);
        snapshots::register(&mut r);
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
                        "library.folder",
                        "library.changed-outside",
                        "library.raw-jpeg-pairs"
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
    fn a_module_s_finding_comes_through_the_report_point_and_goes_with_the_module() {
        let mut r = Registry::default();
        r.add("disks", fake("disks.space", Level::Warning, None))
            .unwrap();
        r.add("fans", fake("fans.speed", Level::Ok, None)).unwrap();
        // The point lists each module's listener, in order, with its owner.
        let listeners = r.kernel().listeners(HealthReport::ID).unwrap();
        let who: Vec<(&str, Option<&str>)> = listeners
            .iter()
            .map(|l| (l.id.as_str(), l.owner.as_deref()))
            .collect();
        assert_eq!(
            who,
            [("disks.space", Some("disks")), ("fans.speed", Some("fans"))]
        );
        let ids = |r: &Registry| -> Vec<String> {
            r.run(&Context::default())
                .into_iter()
                .map(|f| f.id)
                .collect()
        };
        assert_eq!(ids(&r), ["disks.space", "fans.speed"]);
        // A report of some checks only: the others do not answer.
        let some = r.report(Report::all(Context::default()).only(["fans.speed".to_owned()]));
        let answered: Vec<(&str, &str)> = some
            .iter()
            .map(|(o, c, _)| (o.as_str(), c.as_str()))
            .collect();
        assert_eq!(answered, [("fans", "fans.speed")]);
        // The module goes: its listener and its findings with it.
        assert_eq!(r.remove_owner("disks"), 1);
        assert_eq!(ids(&r), ["fans.speed"]);
        assert_eq!(r.kernel().listeners(HealthReport::ID).unwrap().len(), 1);
        assert_eq!(r.owner("disks.space"), None);
        // A clone has its own kernel: what is added to it stays there.
        let mut other = r.clone();
        other
            .add("disks", fake("disks.space", Level::Ok, None))
            .unwrap();
        assert_eq!(ids(&other), ["fans.speed", "disks.space"]);
        assert_eq!(ids(&r), ["fans.speed"]);
    }

    /// A check slower than the report's deadline allows.
    struct Slow;
    impl Check for Slow {
        fn id(&self) -> &'static str {
            "slow"
        }
        fn title(&self) -> Message {
            Message::plain("Slow")
        }
        fn declaration(&self) -> Declaration {
            Declaration::new("Nothing").levels(&[Level::Ok])
        }
        fn run(&self, _: &Context) -> Vec<Finding> {
            vec![Finding::new(
                "slow",
                "",
                Level::Ok,
                Message::plain("m"),
                Message::plain("d"),
            )]
        }
    }

    #[test]
    fn a_listener_failing_in_the_kernel_is_a_problem_of_its_check() {
        let mut r = Registry::default();
        r.add("slow", Arc::new(Slow)).unwrap();
        // Another listener of the point, not a check, that fails: dropped
        // and logged, the checks' answers stand.
        r.kernel()
            .listen::<HealthReport>("broken", ListenOpts::new(), |_, _| Err("on purpose".into()))
            .unwrap();
        let found = r.run(&Context::default());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].level, Level::Ok);
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
