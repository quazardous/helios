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
//! - `health.fix` (`{"id"}`): runs a finding's fix, then the checks again
//!   (so a fix done at once, an acknowledgement, clears its finding).
//! - `health.ignore` (`{"id", "on"}`): ignores a finding, or shows it
//!   again.
//! - `health.checkNow`: runs the checks again.
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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use egui::Context as Egui;
use egui_pop::model::ids::CommandId;
use helios_core::Message;
use helios_health::runner::Runner;
use helios_health::{Finding, Findings, Level};
use serde_json::{Value, json};

use super::Shell;

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

/// The context key set while the owner `owner` (a module, a plugin's id)
/// has findings: its group on the page shows.
pub fn owner_key(owner: &str) -> String {
    let safe: String = owner
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("healthOwner_{safe}")
}

/// The fixes that change something not easily taken back: their button
/// asks first.
const DESTRUCTIVE: &[&str] = &["plugin.disable"];

/// The health check's state, kept by the shell.
#[derive(Default)]
pub(crate) struct HealthPage {
    runner: Option<Runner>,
    /// The context the job's thread asks a frame of.
    ctx: Arc<Mutex<Option<Egui>>>,
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
        self.core.health.runner = Some(runner);
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
        self.health_answer();
    }

    /// Makes `health.findings`'s answer again, and has the Settings app
    /// read it.
    fn health_answer(&mut self) {
        let Some(runner) = self.core.health.runner.as_ref() else {
            return;
        };
        let t = |m: Message| self.text(m);
        let known = crate::config::health_owners(&crate::config::lock(&self.core.health.plugins));
        let answer = findings_answer(&t, &runner.findings(), runner.running(), &known);
        // The groups with rows show; those that had and have no more hide.
        let mut keys: Vec<String> = answer["owners"]
            .as_object()
            .map(|o| o.keys().map(|k| owner_key(k)).collect())
            .unwrap_or_default();
        if answer["others"].as_array().is_some_and(|o| !o.is_empty()) {
            keys.push(OTHERS.to_owned());
        }
        let handle = self.core.app.handle();
        for gone in self.core.health.keys.iter().filter(|k| !keys.contains(k)) {
            handle.set_context(gone, "false");
        }
        for on in &keys {
            handle.set_context(on, "true");
        }
        self.core.health.keys = keys;
        *self
            .core
            .health
            .answer
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = answer;
        if let Some(admin) = self.core.app.handle().service(&egui_pop::api::admin::ADMIN) {
            admin.refresh();
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
        self.core.app.handle().run(command, fix.args.clone());
        if let Ok(again) = CommandId::new(CHECK_NOW) {
            self.core.app.handle().run(again, Value::Null);
        }
        Ok(json!({ "id": id, "command": fix.command }))
    }

    /// `health.ignore`: ignored (`on`, the default) or shown again.
    pub(super) fn health_ignore(&mut self, args: &Value) -> Result<Value, String> {
        let id = args["id"].as_str().ok_or("`id` is missing")?;
        let on = args["on"].as_bool().unwrap_or(true);
        let runner = self.core.health.runner.as_ref().ok_or("no health check")?;
        runner.findings().ignored.set(id, on)?;
        let worst = runner.findings().worst();
        self.core.status.borrow_mut().health = worst;
        self.health_answer();
        Ok(json!({ "id": id, "ignored": on }))
    }

    /// Before the bar is drawn: a run when one is due, and the badge's
    /// worst finding; the page read again when a run starts or ends.
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
            let (volumes, ctx) = (self.core.health.volumes.clone(), ctx.clone());
            watch.on_change(move |change| {
                if !change.at_start() {
                    volumes.fetch_add(1, Ordering::SeqCst);
                    ctx.request_repaint();
                }
            });
            self.core.health.listening = true;
        }
        let generation = self
            .core
            .health
            .listening
            .then(|| self.core.health.volumes.load(Ordering::SeqCst));
        let Some(runner) = self.core.health.runner.as_ref() else {
            return;
        };
        runner.tick(Instant::now(), generation);
        let worst = runner.findings().worst();
        let seen = (runner.runs(), runner.running());
        self.core.status.borrow_mut().health = worst;
        if seen != self.core.health.seen {
            self.core.health.seen = seen;
            self.health_answer();
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
