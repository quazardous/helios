//! The health check on the desktop ([`helios_health`]): its page is the
//! Settings app's Health scope (`crate::config::manifest`), declared and
//! drawn by the Settings app from these commands, never by the shell.
//!
//! - `help.health` opens the Settings app at Health › Check.
//! - `health.findings` (read-only): the findings of the last run, the
//!   worst first, then the ignored ones, as rows (`/findings`) with their
//!   tone, their fix's words and command; the same rows by owner (the
//!   module or plugin whose check found them: `/owners/<owner>`, those
//!   the page has no group for in `/others`); and a summary line with its
//!   tone (`/summary`). The page's groups show by the context keys
//!   [`owner_key`] and [`OTHERS`], set while they have rows.
//! - `health.fix` (`{"id"}`): asks for a finding's fix; the checks run
//!   again once the fix's command is done (its own `command.done` or
//!   `command.failed` in the journal, heard by the listener
//!   `health.recheck`), never before, so a slow fix's finding clears.
//! - `health.ignore` (`{"id", "on"}`): ignores a finding, or shows it
//!   again.
//! - `health.checkNow`: runs the checks again.
//! - `library.chooseFolder` (`{"path"}`, asked through the folder chooser
//!   without it): sets the library folder, then runs the checks again;
//!   the fix of "No library folder chosen".
//!
//! On the bus: a run ending says `health.checked` (a run starting
//! `health.checking`, a finding ignored `health.ignored`), on the journal
//! and to the Settings app's reads, which read `health.findings` again by
//! that event (`shell::reads`'s policies); and the hook point
//! [`HealthChecked`] passes the findings to its listeners in order:
//! `status.health` (the status bar's badge), then `health.context-keys`
//! (the page's groups).
//!
//! The checks run as background jobs ([`helios_health::runner`]): all a
//! while after start, then each as often as it declares (daily by
//! default) and when a volume comes or goes, for those that ask (the
//! storage watch's listener). The status bar's badge shows the worst
//! finding needing attention and their count; it opens the page. Nothing
//! shows while all is well, and nothing is ever modal. On the desktop the
//! runner is the local node's job too ([`crate::node_jobs::Health`]): the
//! node keeps its schedule with or without a window, and the shell holds
//! a handle on the same runner to show its findings.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use egui::Context as Egui;
use egui_pop::api::bus::bus_contract::{Bus, ListenOpts, Point, PointDecl, Refused};
use egui_pop::api::files::{self, FILES, FileRequest};
use egui_pop::api::host::{Event, Pending, Seq};
use egui_pop::model::ids::CommandId;
use helios_core::Message;
use helios_health::runner::Runner;
use helios_health::{Finding, Findings, Level};
use serde_json::{Value, json};

use super::Shell;

/// What only the desktop knows, handed to the modules' health checks
/// ([`checks`]): the `health.destinationDays` setting, the cards an import
/// left short, still inserted, the GPU in use, the plugins installed (each
/// probed), and the catalogue's snapshots. [`Default`]: none of them, as
/// on a node without a window.
pub struct Desktop {
    pub destination_days: Box<dyn Fn() -> u64 + Send + Sync>,
    pub cards: Box<dyn Fn() -> Vec<helios_storage::health::backup::CardShort> + Send + Sync>,
    pub gpu: helios_health::Gather<Option<helios_gpu::health::Gpu>>,
    pub plugins: helios_health::Gather<Vec<helios_plugin::health::installed::Plugin>>,
    pub snapshots: Option<helios_node_core::health::SnapshotsOf>,
}

impl Default for Desktop {
    fn default() -> Self {
        Self {
            destination_days: Box::new(|| helios_storage::health::backup::DESTINATION_DAYS),
            cards: Box::new(Vec::new),
            gpu: helios_health::Gather::fixed(None),
            plugins: helios_health::Gather::fixed(Vec::new()),
            snapshots: None,
        }
    }
}

/// The owners of Helios's own checks on the desktop ([`checks`]), in the
/// Health page's order: what the page has a group for, known without
/// building the checks.
pub const OWNERS: &[&str] = &[
    helios_storage::health::OWNER,
    helios_library::health::OWNER,
    helios_storage::health::backup::OWNER,
    helios_gpu::health::OWNER,
    helios_plugin::health::installed::OWNER,
    helios_node_core::health::snapshots::OWNER,
];

/// Helios's own checks on the desktop, each module adding its own over
/// what it reads, in the Health page's order: the storage, the library,
/// the backups, the GPU, the plugin host, the catalogue. The desktop then
/// adds the GPU kernels' check (over `desktop.gpu`) and each plugin's.
/// The checks listen on `bus`, the application's (the shell's: one bus).
pub fn checks<S: helios_vault::Store + 'static>(
    bus: Arc<dyn Bus>,
    inputs: &helios_node_core::health::HealthInputs<S>,
    desktop: Desktop,
) -> helios_health::Registry {
    let mut r = helios_health::Registry::new(bus);
    inputs.register_storage(&mut r);
    inputs.register_library(&mut r);
    inputs.register_backup(&mut r, desktop.destination_days, desktop.cards);
    helios_gpu::health::register(&mut r, &desktop.gpu);
    helios_plugin::health::installed::register(&mut r, &desktop.plugins);
    inputs.register_catalogue(&mut r, desktop.snapshots);
    r
}

/// Opens the Health page.
pub const COMMAND: &str = "help.health";
/// Runs the checks now.
pub const CHECK_NOW: &str = "health.checkNow";
/// The findings of the last run (read-only).
pub const FINDINGS: &str = "health.findings";
/// Runs a finding's fix.
pub const FIX: &str = "health.fix";
/// Ignores a finding or shows it again.
pub const IGNORE: &str = "health.ignore";
/// The status bar badge's id.
pub const BADGE: &str = "helios.health.badge";
/// The Health page in the Settings app.
pub const PAGE: &str = "health/check";
/// The context key set while findings of an owner the page has no group
/// for are there.
pub const OTHERS: &str = "healthOthers";

/// The owner of the health check's listeners on the bus.
pub const BUS_OWNER: &str = "helios.health";
/// The listener setting the status bar's badge from the findings.
pub const STATUS_LISTENER: &str = "status.health";
/// The listener setting the page's groups' context keys.
pub const KEYS_LISTENER: &str = "health.context-keys";
/// The listener running the checks again once a fix is done.
pub const RECHECK_LISTENER: &str = "health.recheck";
/// Said on the journal when a run ends.
pub const CHECKED: &str = "health.checked";
/// Said on the journal when a run starts.
pub const CHECKING: &str = "health.checking";
/// Said on the journal when a finding is ignored or shown again.
pub const IGNORED: &str = "health.ignored";

/// The hook point a run's findings pass through once read
/// (`health.checked`): each listener fills its part of what the desktop
/// then shows, in their declared order.
pub struct HealthChecked;

impl Point for HealthChecked {
    const ID: &'static str = CHECKED;
    type Input = Checked;
    type Value = Shown;
}

/// What the findings say, for [`HealthChecked`]'s listeners.
#[derive(Clone, Debug, Default)]
pub struct Checked {
    /// The worst finding needing attention and their count.
    pub worst: Option<(Level, usize)>,
    /// The owners with rows on the page.
    pub owners: Vec<String>,
    /// Whether rows of an owner the page has no group for are there.
    pub others: bool,
}

/// What [`HealthChecked`]'s listeners say the desktop shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// Whether `status.health` set the status bar's badge.
    pub badge_set: bool,
    /// The badge: the worst finding needing attention and their count.
    pub badge: Option<(Level, usize)>,
    /// The context keys on for the page's groups
    /// (`health.context-keys`); `None` leaves them.
    pub keys: Option<Vec<String>>,
}

/// The desktop's health points, its contract with the application's bus,
/// declared by [`listen_checked`].
pub const POINTS: &[PointDecl] = &[PointDecl::of::<HealthChecked>(
    "A health run's findings, read: what the desktop shows of them",
)];

/// The listeners of [`HealthChecked`] on `bus`, in order: the badge, then
/// the page's groups.
pub fn listen_checked(bus: &dyn Bus) {
    // Declared once; a second shell on the same bus finds it there.
    for refused in egui_pop::api::bus::bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "health: {refused}");
        }
    }
    let opts = || ListenOpts::new().owner(BUS_OWNER);
    for (id, run) in [
        (
            STATUS_LISTENER,
            Box::new(|c: &Checked, s: &mut Shown| {
                s.badge_set = true;
                s.badge = c.worst;
                Ok(())
            }) as Box<dyn Fn(&Checked, &mut Shown) -> _ + Send + Sync>,
        ),
        (
            KEYS_LISTENER,
            Box::new(|c: &Checked, s: &mut Shown| {
                let mut keys: Vec<String> = c.owners.iter().map(|o| owner_key(o)).collect();
                if c.others {
                    keys.push(OTHERS.to_owned());
                }
                s.keys = Some(keys);
                Ok(())
            }),
        ),
    ] {
        bus.unlisten(HealthChecked::ID, id);
        let o = if id == KEYS_LISTENER {
            opts().after(STATUS_LISTENER)
        } else {
            opts()
        };
        if let Err(e) = bus.listen::<HealthChecked>(id, o, move |c, s| run(c, s)) {
            log::error!(target: "helios", "health: {e}");
        }
    }
}

/// The order the health points' listeners run in on `bus` and in the
/// health report of `registry` ("who runs when"): on the desktop both are
/// the one bus.
pub fn order(bus: &dyn Bus, registry: &helios_health::Registry) -> String {
    let mut out =
        egui_pop::api::bus::order_dump(&**registry.bus(), &[helios_health::HealthReport::ID]);
    out.push_str(&egui_pop::api::bus::order_dump(bus, &[HealthChecked::ID]));
    out
}

/// A fix asked: its command, and the journal from where its dispatch is
/// looked for.
#[derive(Clone, Debug)]
struct Fixing {
    command: String,
    since: Seq,
    /// Its dispatch, once heard: its `command.done` names it as cause.
    dispatched: Option<Seq>,
}

/// The context key set while the owner `owner` (a module, a plugin's id)
/// has findings: its group on the page shows.
pub fn owner_key(owner: &str) -> String {
    let safe: String = owner
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("healthOwner_{safe}")
}

/// The runner the bus's listeners ask some checks of again
/// (`card.safe`'s `health.cards`, the accounts' `health.accounts`), and
/// the checks waiting for a run under way to end.
#[derive(Clone, Default)]
pub(crate) struct CardsRecheck(Arc<(Mutex<Option<Runner>>, Mutex<Vec<String>>)>);

/// The checks `ids` run again now; when a run is under way, once it ends
/// ([`Shell::health_tick`], [`recheck_pending`]).
fn recheck(recheck: &CardsRecheck, ids: Vec<String>) {
    let (runner, pending) = &*recheck.0;
    let mut pending = pending.lock().unwrap_or_else(PoisonError::into_inner);
    for id in ids {
        if !pending.contains(&id) {
            pending.push(id);
        }
    }
    if pending.is_empty() {
        return;
    }
    let ran = runner
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .is_none_or(|r| r.run_only(&pending));
    if ran {
        pending.clear();
    }
}

/// The checks still waiting for a run under way to end, asked again.
pub(super) fn recheck_pending(r: &CardsRecheck) {
    recheck(r, Vec::new());
}

/// `health.cards`: the cards check runs again now, from the one card
/// list; when a run is under way, once it ends ([`Shell::health_tick`]).
pub(super) fn recheck_cards(r: &CardsRecheck) {
    recheck(r, vec![helios_storage::health::backup::CARDS.to_owned()]);
}

/// `health.accounts`, an account signed out or removed: the checks
/// handed the accounts run again, the plugins' (`plugins`: a source
/// plugin is needed once its connector has an account) and each
/// plugin's own (`<plugin>.<check>`, owned by its plugin).
pub(super) fn recheck_accounts(r: &CardsRecheck) {
    let ids: Vec<String> =
        r.0.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|runner| {
                runner
                    .registry()
                    .owned()
                    .filter(|(owner, check)| {
                        *owner == helios_plugin::health::installed::OWNER
                            || check.id().starts_with(&format!("{owner}."))
                    })
                    .map(|(_, check)| check.id().to_owned())
                    .collect()
            })
            .unwrap_or_default();
    recheck(r, ids);
}

/// The fixes that change something not easily taken back: their button
/// asks first.
const DESTRUCTIVE: &[&str] = &["plugin.disable"];

/// The health check's state, kept by the shell.
#[derive(Default)]
pub(crate) struct HealthPage {
    runner: Option<Runner>,
    /// The runner `health.cards` asks the cards check of, once set.
    pub(super) cards_runner: CardsRecheck,
    /// The context the job's thread asks a frame of.
    pub(super) ctx: Arc<Mutex<Option<Egui>>>,
    /// How many times a volume came or went, once listened to.
    volumes: Arc<AtomicU64>,
    listening: bool,
    /// The run state the answer was made at.
    seen: (u64, bool),
    /// `health.findings`'s answer, made again when a run starts or ends
    /// and when a finding is ignored, and read by the command at once.
    answer: Arc<Mutex<Value>>,
    /// The plugins installed: each has a group on the page.
    plugins: crate::config::plugins::Shared,
    /// The context keys set on, for the groups with findings.
    keys: Vec<String>,
    /// The folder chooser asked for the library folder.
    choosing_folder: Option<Pending>,
    /// The fixes asked whose command is not done yet.
    fixing: Vec<Fixing>,
    /// A fix ended while a run was under way: the checks run again once
    /// it ends.
    recheck: bool,
}

impl HealthPage {
    pub(crate) fn new(plugins: crate::config::plugins::Shared) -> Self {
        Self {
            plugins,
            ..Self::default()
        }
    }
}

/// The findings as the page reads them: each finding a row with `id`,
/// `level` (`problem`, `warning`, `ok`), `tone`, `title` and `text` (its
/// message and detail), `fix` (whether it has one), `fixLabel` and
/// `fixCommand`, `destructive` (its fix asks first), `attention` (a
/// warning or a problem) and `ignored`; the shown ones worst first, then
/// the ignored ones. `owners` holds the same rows by owner, for the
/// owners `known` (those the page has a group for), and `others` the
/// rows of any other owner. `summary` says how it stands, with its tone;
/// `ran` and `running` the run's state.
pub fn findings_answer(
    t: &dyn Fn(Message) -> String,
    findings: &Findings,
    running: bool,
    known: &[String],
) -> Value {
    let row = |f: &Finding, ignored: bool| {
        let (level, tone) = match f.level {
            Level::Problem => ("problem", "danger"),
            Level::Warning => ("warning", "warning"),
            Level::Ok => ("ok", "success"),
        };
        json!({
            "id": f.id,
            "level": level,
            "tone": if ignored { Value::Null } else { json!(tone) },
            "title": t(f.message.clone()),
            "text": t(f.detail.clone()),
            "fix": f.fix.is_some(),
            "fixLabel": f.fix.as_ref().map(|x| t(x.label.clone())).unwrap_or_default(),
            "fixCommand": f.fix.as_ref().map(|x| x.command.clone()),
            "destructive": f.fix.as_ref().is_some_and(|x| DESTRUCTIVE.contains(&x.command.as_str())),
            "attention": f.level > Level::Ok,
            "ignored": ignored,
            "ignoredText": if ignored {
                t(Message::new("gui-health-ignored-row", "ignored"))
            } else {
                String::new()
            },
        })
    };
    let shown = findings.shown();
    let ignored = findings.ignored_findings();
    let mut rows: Vec<Value> = shown.iter().map(|f| row(f, false)).collect();
    rows.extend(ignored.iter().map(|f| row(f, true)));
    let mut owners = serde_json::Map::new();
    let mut others = Vec::new();
    for (f, row) in shown.iter().chain(&ignored).zip(&rows) {
        let owner = findings.owner(f).unwrap_or_default();
        if known.iter().any(|k| k == owner) {
            if let Value::Array(list) = owners.entry(owner).or_insert_with(|| json!([])) {
                list.push(row.clone());
            }
        } else {
            others.push(row.clone());
        }
    }
    let count = |l: Level| shown.iter().filter(|f| f.level == l).count();
    let (problems, warnings) = (count(Level::Problem), count(Level::Warning));
    let (text, tone) = if !findings.ran() && running {
        (t(Message::new("gui-health-checking", "Checking…")), "info")
    } else if !findings.ran() {
        (
            t(Message::new(
                "gui-health-not-yet",
                "Not checked yet: the checks run in the background.",
            )),
            "info",
        )
    } else if problems > 0 {
        (
            t(Message::new(
                "gui-health-summary-problems",
                "Problems: { $problems }, warnings: { $warnings }",
            )
            .with("problems", problems)
            .with("warnings", warnings)),
            "danger",
        )
    } else if warnings > 0 {
        (
            t(
                Message::new("gui-health-summary-warnings", "Warnings: { $count }")
                    .with("count", warnings),
            ),
            "warning",
        )
    } else {
        (
            t(Message::new("gui-health-summary-ok", "All good")),
            "success",
        )
    };
    json!({
        "findings": rows,
        "owners": owners,
        "others": others,
        "ignored": ignored.len(),
        "ran": findings.ran(),
        "running": running,
        "summary": { "text": text, "tone": tone },
    })
}

impl Shell {
    /// Runs the health checks with `runner`, reported in the jobs centre;
    /// the desktop gives Helios's checks over the machine and the vault,
    /// tests their own.
    pub fn set_health(&mut self, runner: Runner) {
        runner.report_to(self.jobs());
        let ctx = self.core.health.ctx.clone();
        runner.on_done(move || {
            if let Some(ctx) = ctx.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                ctx.request_repaint();
            }
        });
        *self
            .core
            .health
            .cards_runner
            .0
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(runner.clone());
        self.core.health.runner = Some(runner);
        listen_checked(&*self.core.app.handle().bus());
        if !self.has_command(FINDINGS) {
            let answer = self.core.health.answer.clone();
            let registered = self.register(
                helios_shell::commands::CommandSpec::new(
                    FINDINGS,
                    "The health check's findings, worst first, then the ignored ones: level, words, fix; and a summary",
                    "Help",
                )
                .read_only(),
                move |_, _| Ok(answer.lock().unwrap_or_else(PoisonError::into_inner).clone()),
            );
            if let Err(e) = registered {
                log::warn!("health: {e}");
            }
        }
        self.health_answer(None);
    }

    /// The order the health points' listeners run in ([`order`]).
    pub fn health_order(&self) -> String {
        let bus = self.core.app.handle().bus();
        match self.core.health.runner.as_ref() {
            Some(r) => order(&*bus, r.registry()),
            None => egui_pop::api::bus::order_dump(&*bus, &[HealthChecked::ID]),
        }
    }

    /// Makes `health.findings`'s answer again and passes the findings
    /// through [`HealthChecked`]'s listeners; `said`, the event saying
    /// why, goes on the journal and to the Settings app's reads.
    fn health_answer(&mut self, said: Option<&str>) {
        let Some(runner) = self.core.health.runner.as_ref() else {
            return;
        };
        let t = |m: Message| self.text(m);
        let known = crate::config::health_owners(&crate::config::lock(&self.core.health.plugins));
        let findings = runner.findings();
        let answer = findings_answer(&t, &findings, runner.running(), &known);
        let checked = Checked {
            worst: findings.worst(),
            owners: answer["owners"]
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default(),
            others: answer["others"].as_array().is_some_and(|o| !o.is_empty()),
        };
        drop(findings);
        *self
            .core
            .health
            .answer
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = answer;
        let handle = self.core.app.handle().clone();
        let altered = handle
            .bus()
            .alter::<HealthChecked>(checked, Shown::default());
        for failure in &altered.failures {
            log::warn!(target: "helios", "health: {failure}");
        }
        let shown = altered.value;
        if shown.badge_set {
            self.core.status.borrow_mut().health = shown.badge;
        }
        if let Some(keys) = shown.keys {
            for gone in self.core.health.keys.iter().filter(|k| !keys.contains(k)) {
                handle.set_context(gone, "false");
            }
            for on in &keys {
                handle.set_context(on, "true");
            }
            self.core.health.keys = keys;
        }
        if let Some(name) = said {
            handle.publish("helios", None, name, Value::Null);
            self.read_fact(name);
        }
    }

    /// The health checks' runner, when set.
    pub fn health(&self) -> Option<&Runner> {
        self.core.health.runner.as_ref()
    }

    /// Whether the Settings app is open on its Health scope.
    pub fn health_page_open(&self) -> bool {
        let app = self.core.app.handle();
        app.service(&egui_pop::api::admin::ADMIN)
            .is_some_and(|a| a.is_open() && a.shown().0.as_deref() == Some("health"))
    }

    /// `help.health`: the Settings app at Health › Check.
    pub(super) fn open_health(&mut self) -> Result<Value, String> {
        self.open_settings_page(PAGE)
    }

    /// `health.checkNow`: whether a run started (none while one runs).
    pub(super) fn health_check_now(&mut self) -> bool {
        self.core
            .health
            .runner
            .as_ref()
            .is_some_and(Runner::run_now)
    }

    /// `health.fix`: the finding's fix, then the checks again.
    pub(super) fn health_fix(&mut self, args: &Value) -> Result<Value, String> {
        let id = args["id"].as_str().ok_or("`id` is missing")?;
        let fix = {
            let runner = self.core.health.runner.as_ref().ok_or("no health check")?;
            let findings = runner.findings();
            let f = findings
                .shown()
                .into_iter()
                .chain(findings.ignored_findings())
                .find(|f| f.id == id)
                .cloned()
                .ok_or_else(|| format!("no finding `{id}`"))?;
            f.fix.ok_or_else(|| format!("`{id}` has no fix"))?
        };
        let command = CommandId::new(&fix.command).map_err(|e| format!("{}: {e}", fix.command))?;
        log::info!(
            target: "helios",
            "health.fix: finding `{id}` runs `{}` with {}",
            fix.command,
            fix.args
        );
        let handle = self.core.app.handle();
        // Its dispatch and its end come after what the journal holds now.
        self.core.health.fixing.push(Fixing {
            command: fix.command.clone(),
            since: handle.last_event(),
            dispatched: None,
        });
        handle.run(command, fix.args.clone());
        Ok(json!({ "id": id, "command": fix.command }))
    }

    /// `health.ignore`: ignored (`on`, the default) or shown again.
    pub(super) fn health_ignore(&mut self, args: &Value) -> Result<Value, String> {
        let id = args["id"].as_str().ok_or("`id` is missing")?;
        let on = args["on"].as_bool().unwrap_or(true);
        let runner = self.core.health.runner.as_ref().ok_or("no health check")?;
        runner.findings().ignored.set(id, on)?;
        self.health_answer(Some(IGNORED));
        Ok(json!({ "id": id, "ignored": on }))
    }

    /// `library.chooseFolder`: sets the library folder to `path`, or asks
    /// for it through the folder chooser (set once chosen); the checks
    /// then run again.
    pub(super) fn choose_library_folder(&mut self, args: &Value) -> Result<Value, String> {
        if let Some(path) = args["path"].as_str().filter(|p| !p.is_empty()) {
            self.set_library_folder(Path::new(path))?;
            return Ok(json!({ "path": path }));
        }
        let app = self.core.app.handle();
        let f = app.service(&FILES).ok_or("no folder chooser")?;
        let title = Message::new("gui-pick-choose-folder", "Choose the library folder…");
        let title = self
            .core
            .health
            .ctx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or_else(
                || title.english.clone(),
                |c| crate::i18n::text(c, title.clone()),
            );
        let request = FileRequest::folder(&title).purpose("helios.library.folder");
        self.core.health.choosing_folder = Some(f.choose(app, request));
        Ok(json!({ "asked": true }))
    }

    /// Remembers `dir` as the library folder, and runs the checks again.
    pub(super) fn set_library_folder(&mut self, dir: &Path) -> Result<(), String> {
        let library = self
            .import
            .get_mut()
            .accounts
            .library
            .clone()
            .ok_or("no library to choose a folder for")?;
        library.set(dir)?;
        self.health_check_now();
        Ok(())
    }

    /// The folder the chooser gave for the library, set.
    fn receive_library_folder(&mut self) {
        let Some(result) = self
            .core
            .health
            .choosing_folder
            .as_ref()
            .and_then(Pending::take)
        else {
            return;
        };
        self.core.health.choosing_folder = None;
        if let Ok(files::Chosen::Paths(paths)) = files::chosen(&result)
            && let Some(p) = paths.into_iter().next()
            && let Err(e) = self.set_library_folder(&p)
        {
            log::warn!("library folder: {e}");
        }
    }

    /// `health.recheck`, a listener of the journal: a fix asked is
    /// dispatched, then done (or failed, or refused): the checks run
    /// again, now that what it changed is there.
    pub(super) fn health_heard(&mut self, e: &Event) {
        let fixing = &mut self.core.health.fixing;
        if fixing.is_empty() {
            return;
        }
        let command = e.data["command"].as_str().unwrap_or_default();
        let ended = match e.name.as_str() {
            "command.dispatched" => {
                if let Some(f) = fixing
                    .iter_mut()
                    .find(|f| f.dispatched.is_none() && f.command == command && e.seq > f.since)
                {
                    f.dispatched = Some(e.seq);
                }
                false
            }
            "command.done" | "command.failed" => {
                let before = fixing.len();
                fixing.retain(|f| f.dispatched.is_none() || f.dispatched != e.cause);
                fixing.len() != before
            }
            "command.refused" => {
                let before = fixing.len();
                fixing
                    .retain(|f| f.dispatched.is_some() || f.command != command || e.seq <= f.since);
                fixing.len() != before
            }
            _ => false,
        };
        if ended {
            log::info!(target: "helios", "{RECHECK_LISTENER}: `{command}` ended, the checks run again");
            self.core.health.recheck = true;
        }
    }

    /// Before the bar is drawn: a run when one is due, or asked by a fix
    /// that ended; [`HealthChecked`] passed through when a run starts or
    /// ends.
    pub(super) fn health_tick(&mut self, ctx: &Egui) {
        {
            let mut held = self
                .core
                .health
                .ctx
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if held.is_none() {
                *held = Some(ctx.clone());
            }
        }
        if !self.core.health.listening
            && let Some(watch) = self.volume_watch()
        {
            // A volume came or went: the checks that follow the volumes
            // run again, once the scan said where the copies are.
            let (volumes, ctx) = (self.core.health.volumes.clone(), ctx.clone());
            let listened = crate::node_volumes::listen_both(
                &**watch.hooks(),
                crate::node_volumes::HEALTH,
                "helios.health",
                move |change| {
                    if !change.at_start() {
                        volumes.fetch_add(1, Ordering::SeqCst);
                        ctx.request_repaint();
                    }
                },
            );
            if let Err(e) = listened {
                log::error!(target: "helios", "{}: {e}", crate::node_volumes::HEALTH);
            }
            self.core.health.listening = true;
        }
        self.receive_library_folder();
        let generation = self
            .core
            .health
            .listening
            .then(|| self.core.health.volumes.load(Ordering::SeqCst));
        let Some(runner) = self.core.health.runner.as_ref() else {
            return;
        };
        if self.core.health.recheck && runner.run_now() {
            self.core.health.recheck = false;
        }
        recheck_pending(&self.core.health.cards_runner);
        runner.tick(Instant::now(), generation);
        let seen = (runner.runs(), runner.running());
        if seen != self.core.health.seen {
            self.core.health.seen = seen;
            self.health_answer(Some(if seen.1 { CHECKING } else { CHECKED }));
        }
    }
}

#[cfg(test)]
mod tests {
    use helios_health::{Fix, Ignored};

    use super::*;

    fn english(m: Message) -> String {
        helios_i18n::message(&helios_i18n::I18n::new("en"), &m)
    }

    #[test]
    fn an_owner_key_is_a_context_key() {
        assert_eq!(owner_key("org.example.x-y"), "healthOwner_org_example_x_y");
        assert_eq!(owner_key("backup"), "healthOwner_backup");
    }

    #[test]
    fn findings_are_rows_worst_first_then_the_ignored_ones() {
        let plain = Message::plain;
        let fine = Finding::new("a.fine", "", Level::Ok, plain("Fine"), plain(""));
        let warn = Finding::new("a.warn", "", Level::Warning, plain("Warn"), plain(""));
        let bad = Finding::new("a.bad", "", Level::Problem, plain("Bad"), plain("Why")).fix(Fix {
            command: "plugin.disable".into(),
            args: json!({ "plugin": "p" }),
            label: plain("Disable it"),
        });
        let dir = std::env::temp_dir().join(format!("helios-health-rows-{}", std::process::id()));
        let mut findings = Findings::with_ignored(Ignored::load(&dir.join("ignored.json")));
        let known = ["disks".to_owned()];
        let a = findings_answer(&english, &findings, false, &known);
        assert_eq!(a["summary"]["tone"], "info", "not checked yet");
        findings.put("disks", "a.fine", vec![fine]);
        findings.put("other", "a.warn", vec![warn]);
        findings.put("disks", "a.bad", vec![bad]);
        let a = findings_answer(&english, &findings, false, &known);
        let rows = a["findings"].as_array().unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["a.bad", "a.warn", "a.fine"]);
        assert_eq!(rows[0]["tone"], "danger");
        assert_eq!(rows[0]["fixLabel"], "Disable it");
        assert_eq!(rows[0]["destructive"], true);
        assert_eq!(rows[2]["attention"], false);
        assert_eq!(a["summary"]["tone"], "danger");
        // By owner: the known one's rows worst first, the other's apart.
        let ids = |rows: &Value| -> Vec<String> {
            rows.as_array()
                .unwrap()
                .iter()
                .map(|r| r["id"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(ids(&a["owners"]["disks"]), ["a.bad", "a.fine"]);
        assert_eq!(ids(&a["others"]), ["a.warn"]);
        assert_eq!(a["owners"].as_object().unwrap().len(), 1);
        findings.ignored.set("a.bad", true).unwrap();
        let a = findings_answer(&english, &findings, false, &known);
        assert_eq!(
            ids(&a["owners"]["disks"]),
            ["a.fine", "a.bad"],
            "the ignored last"
        );
        let rows = a["findings"].as_array().unwrap();
        assert_eq!(rows[2]["id"], "a.bad");
        assert_eq!(rows[2]["ignored"], true);
        assert!(rows[2]["tone"].is_null());
        assert_eq!(a["summary"]["tone"], "warning");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
