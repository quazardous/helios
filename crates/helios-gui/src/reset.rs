//! Settings > Danger zone: resetting Helios from the desktop, never a
//! photo (the kernel is `helios_node_core::reset`).
//!
//! Two commands:
//!
//! - [`PLAN_COMMAND`] (read-only) says what a reset of a scope takes and
//!   what it keeps: each target and its fate, the backup folder, and the
//!   photos that stay on disk, counted, with their folders.
//! - [`COMMAND`] (`helios.app.reset`) resets. It runs only with the word
//!   [`WORD`] typed (`confirm`), is destructive and is **kept from
//!   agents**: the agent bridge refuses it whatever the agent's grant
//!   (`crate::destructive::NOT_FOR_AGENTS`). It plans the reset at once
//!   (a plan refused, a photo folder among the targets, is said and
//!   nothing moves), keeps it in [`Reset::pending`] and asks the desktop
//!   to quit ([`Reset::go`]): Helios quits through `helios.app.quit`,
//!   runs the plan once the library is closed, and starts again on the
//!   first-run setup.
//!
//! A plan lives for **the quit it was confirmed for** only: that quit
//! refused, or its question answered with anything but Quit (unsaved
//! work, running jobs, then Cancel), drops it, said in a notice and in
//! the log ([`Shell::on_quit_abandoned`]). It never runs at a later,
//! ordinary quit.
//!
//! The photo folders the plan is checked against are the library's
//! folders (where its photos are) and its library folder (where imports
//! copy to), from [`Reset::photos`].
//!
//! The Danger zone section offers a catalogue snapshot first, then
//! "Reset Helios…", which opens the [`WIZARD`]: the scope, then the word
//! typed, checked as it is typed ([`CHECK_WORD`]). An "everything" reset
//! may forget the accounts too (`forget_accounts`, `crate::forget`): the
//! keyring's Helios entries are removed with the plan, after the quit.
//! "Forget accounts…" does that alone (`crate::forget::WIZARD`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui_pop::api::admin::model::hooks::{Hook, Problem};
use egui_pop::api::admin::model::{
    Action, Confirmation, Field, FieldKind, Records, SectionBuilder, Status, Tone, Wizard,
};
use helios_core::Message;
use helios_node_core::reset::{self, Fate, Places, Plan, Scope, What};
use helios_shell::commands::CommandSpec;
use serde_json::{Value, json};

use crate::shell::Shell;

/// Resets Helios: `scope` (`everything`, `caches`, `settings`) and
/// `confirm`, the word [`WORD`] typed.
pub const COMMAND: &str = "helios.app.reset";

/// What a reset of `scope` takes and keeps (read-only).
pub const PLAN_COMMAND: &str = "helios.app.reset.plan";

/// The word typed to confirm a reset.
pub const WORD: &str = "RESET";

/// The reset's wizard: the scope, then the word.
pub const WIZARD: &str = "helios.reset";

/// The wizard's check of the word typed.
pub const CHECK_WORD: &str = "helios.reset.word";

/// The Settings page the Danger zone is on.
pub const SECTION: &str = "danger/reset";

pub use helios_node_core::reset::{Photos, photos_of};

/// A confirmed plan, waiting for the quit it was confirmed for: the
/// desktop takes it once the library is closed ([`Pending::take`]); an
/// abandoned quit drops it.
#[derive(Clone, Debug, Default)]
pub struct Pending(Arc<Mutex<Option<(Plan, bool)>>>);

impl Pending {
    fn slot(&self) -> std::sync::MutexGuard<'_, Option<(Plan, bool)>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Keeps `plan` for the quit asked next.
    pub fn set(&self, plan: Plan) {
        self.set_with_accounts(plan, false);
    }

    /// Keeps `plan` for the quit asked next, the accounts forgotten with
    /// it when `forget_accounts` (`crate::forget`).
    pub fn set_with_accounts(&self, plan: Plan, forget_accounts: bool) {
        *self.slot() = Some((plan, forget_accounts));
    }

    /// The plan, taken: by the desktop once Helios has quit, or dropped.
    pub fn take(&self) -> Option<Plan> {
        self.take_with_accounts().map(|(plan, _)| plan)
    }

    /// The plan and whether the accounts go with it, taken.
    pub fn take_with_accounts(&self) -> Option<(Plan, bool)> {
        self.slot().take()
    }

    /// Whether a plan waits.
    pub fn is_set(&self) -> bool {
        self.slot().is_some()
    }
}

/// What the desktop hands the reset commands.
#[derive(Clone)]
pub struct Reset {
    /// Where Helios keeps its own things.
    pub places: Places,
    /// The photos, read when a plan is made.
    pub photos: Arc<dyn Fn() -> Photos + Send + Sync>,
    /// The confirmed plan, kept for the quit [`Reset::go`] asks.
    pub pending: Pending,
    /// Asks Helios to quit and start again, the plan run in between.
    pub go: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
}

/// Where Helios keeps its things on this machine: its configuration,
/// data, state and cache folders ([`Places::from_env`], read by
/// `helios --reset` too), and `library` (the library file in use).
/// `None` when one of the folders cannot be told.
pub fn places(library: Option<PathBuf>) -> Option<Places> {
    Places::from_env(|name| std::env::var(name).ok(), library)
}

/// The scope a call names; `everything` when it names none.
fn scope_of(args: &Value) -> Result<Scope, String> {
    match args.get("scope").and_then(Value::as_str) {
        None | Some("") => Ok(Scope::Everything),
        Some(name) => Scope::parse(name)
            .ok_or_else(|| format!("`scope` is `{name}`: everything, caches or settings")),
    }
}

fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    secs.to_string()
}

/// The words for what a target is.
fn what_text(what: What) -> Message {
    match what {
        What::Config => Message::new(
            "gui-reset-what-config",
            "Settings and workspaces (configuration folder)",
        ),
        What::Data => Message::new(
            "gui-reset-what-data",
            "Default library, its snapshots and image map (data folder)",
        ),
        What::State => Message::new("gui-reset-what-state", "Logs and crash reports"),
        What::Cache => Message::new("gui-reset-what-cache", "Caches"),
        What::Library => Message::new("gui-reset-what-library", "Library catalogue"),
        What::Thumbnails => Message::new("gui-reset-what-thumbnails", "Thumbnails"),
    }
}

/// The answer of [`PLAN_COMMAND`]: the plan's targets as rows, what stays,
/// and a one-line summary; a plan refused says why.
pub fn plan_answer(
    reset: &Reset,
    scope: Scope,
    t: &dyn Fn(Message) -> String,
) -> Result<Value, String> {
    let photos = (reset.photos)();
    let plan = reset::plan(&reset.places, scope, &photos.roots, &stamp());
    let stays = t(Message::new(
        "gui-reset-stays",
        "Stay untouched: your { $count } photos and their sidecars (folders: { $folders }), and the backup disks.",
    )
    .with("count", photos.count.to_string())
    .with("folders", photos.roots.len().to_string()));
    let plan = match plan {
        Ok(plan) => plan,
        Err(refused) => {
            return Ok(json!({
                "refused": refused.0,
                "summary": { "text": t(Message::new(
                    "gui-reset-refused",
                    "A reset is not possible here: { $why }",
                ).with("why", refused.0.clone())), "tone": "danger" },
                "stays": stays,
                "photos": photos.count,
                "targets": [],
            }));
        }
    };
    let targets: Vec<Value> = plan
        .targets
        .iter()
        .map(|target| {
            json!({
                "id": target.path.display().to_string(),
                "what": target.what,
                "whatText": t(what_text(target.what)),
                "path": target.path.display().to_string(),
                "fate": target.fate,
                "fateText": t(match target.fate {
                    Fate::Backup => Message::new("gui-reset-fate-backup", "moved to the backup folder"),
                    Fate::Remove => Message::new("gui-reset-fate-remove", "removed"),
                }),
            })
        })
        .collect();
    let backup = plan.backup.as_ref().map(|b| b.display().to_string());
    let summary = match &backup {
        Some(_) => t(Message::new(
            "gui-reset-summary-backup",
            "{ $count } places go: moved to a dated helios-reset folder beside the data folder, so a reset can be undone by hand; the caches are removed.",
        )
        .with("count", targets.len().to_string())),
        None => t(Message::new("gui-reset-summary", "{ $count } places are removed.")
            .with("count", targets.len().to_string())),
    };
    Ok(json!({
        "scope": plan.scope,
        "targets": targets,
        "backup": backup,
        "photos": photos.count,
        "folders": photos.roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>(),
        "stays": stays,
        "summary": { "text": summary },
        "staysLine": { "text": stays },
    }))
}

/// Runs [`COMMAND`]: the word checked, the plan made (refused when it
/// would touch a photo folder), then handed to [`Reset::go`].
pub fn run(reset: &Reset, args: &Value) -> Result<Value, String> {
    let typed = args.get("confirm").and_then(Value::as_str).unwrap_or("");
    if typed.trim() != WORD {
        return Err(format!(
            "a reset needs `confirm`: the word {WORD} typed by the person"
        ));
    }
    let scope = scope_of(args)?;
    let forget_accounts = args.get("forget_accounts") == Some(&Value::Bool(true));
    if forget_accounts && scope != Scope::Everything {
        return Err("`forget_accounts` goes with an `everything` reset only".into());
    }
    let photos = (reset.photos)();
    let plan = reset::plan(&reset.places, scope, &photos.roots, &stamp()).map_err(|r| r.0)?;
    log::warn!(
        target: "helios::reset",
        "reset asked ({scope:?}): {} targets, backup {:?}, accounts forgotten: {forget_accounts}; Helios quits and starts again",
        plan.targets.len(),
        plan.backup
    );
    let answer = json!({
        "restarting": true,
        "scope": plan.scope,
        "backup": plan.backup,
        "targets": plan.targets.len(),
        "forget_accounts": forget_accounts,
    });
    reset.pending.set_with_accounts(plan, forget_accounts);
    if let Err(e) = (reset.go)() {
        reset.pending.take();
        return Err(e);
    }
    Ok(answer)
}

/// Registers [`PLAN_COMMAND`] and [`COMMAND`] on the shell.
///
/// # Errors
/// A command cannot be registered.
pub fn register(shell: &mut Shell, reset: Reset) -> Result<(), String> {
    let pending = reset.pending.clone();
    shell.on_quit_abandoned(move || {
        let plan = pending.take()?;
        log::warn!(
            target: "helios::reset",
            "reset dropped ({:?}): Helios did not quit, nothing was reset",
            plan.scope
        );
        Some(Message::new(
            "gui-reset-dropped",
            "Reset cancelled: Helios did not quit, so nothing was reset. Confirm it again to reset.",
        ))
    });
    let t = shell.texts();
    let for_plan = reset.clone();
    shell.register(
        CommandSpec::new(
            PLAN_COMMAND,
            "What a reset of Helios (`scope`: everything, caches, settings) takes and keeps: each place and its fate, the backup folder, the photos kept",
            "Help",
        )
        .read_only(),
        move |args, _| plan_answer(&for_plan, scope_of(args)?, &t),
    )?;
    shell.register(
        CommandSpec::new(
            COMMAND,
            "Reset Helios (`scope`: everything, caches or settings; `confirm`: the word RESET; `forget_accounts`: the keyring's secrets too, with everything): Helios quits, puts its own folders aside and starts again; photos are never touched",
            "Help",
        ),
        move |args, _| run(&reset, args),
    )
}

/// The Danger zone section: what a full reset takes and keeps, a
/// catalogue snapshot offered first, and "Reset Helios…" opening the
/// [`wizard`].
pub fn section(s: &mut SectionBuilder<'_>, t: &dyn Fn(Message) -> String) {
    s.description(&t(Message::new(
        "gui-reset-about",
        "Reset Helios as on its first launch: its settings, workspaces, catalogue, caches and logs are put aside in a dated backup folder (the caches are removed). Photos and their sidecars are never touched, wherever they are.",
    )));
    s.status(Status::command(
        "resetPlan",
        &t(Message::new("gui-reset-goes", "What goes")),
        PLAN_COMMAND,
        "/summary",
    ));
    s.status(Status::command(
        "resetStays",
        &t(Message::new("gui-reset-stays-title", "What stays")),
        PLAN_COMMAND,
        "/staysLine",
    ));
    s.list(
        Records::command("reset-targets", "", PLAN_COMMAND, "/targets", "id")
            .titles("whatText", Some("path"))
            .column("fateText", ""),
    );
    s.action(
        Action::command(crate::catalogue::SNAPSHOT_COMMAND)
            .title(&t(Message::new(
                "gui-reset-snapshot",
                "Snapshot the catalogue first",
            )))
            .primary(),
    );
    s.action(
        Action::command(egui_pop::api::admin::CMD_WIZARD_OPEN.as_str())
            .title(&t(Message::new("gui-reset-open", "Reset Helios…")))
            .args(json!({ "id": WIZARD }))
            .danger(),
    );
    s.action(
        Action::command(egui_pop::api::admin::CMD_WIZARD_OPEN.as_str())
            .title(&t(Message::new("gui-forget-open", "Forget accounts…")))
            .args(json!({ "id": crate::forget::WIZARD }))
            .danger(),
    );
}

/// The reset's wizard: what to reset, then the word typed; its finish
/// runs [`COMMAND`] with both, asked once more.
pub fn wizard(t: &dyn Fn(Message) -> String) -> Wizard {
    let scope = Field::draft(
        WIZARD,
        "scope",
        &t(Message::new("gui-reset-scope", "Reset")),
        FieldKind::Choice,
    )
    .choice(
        json!("everything"),
        &t(Message::new(
            "gui-reset-scope-everything",
            "Everything: the next launch is a first launch",
        )),
    )
    .choice(
        json!("settings"),
        &t(Message::new(
            "gui-reset-scope-settings",
            "Settings only: the library stays",
        )),
    )
    .choice(
        json!("caches"),
        &t(Message::new(
            "gui-reset-scope-caches",
            "Caches only: thumbnails and previews are made again",
        )),
    )
    .default_value(json!("everything"))
    .required();
    let accounts = Field::draft(
        WIZARD,
        "forget_accounts",
        &t(Message::new(
            "gui-reset-forget-accounts",
            "Also forget the accounts (their sign-ins and AI keys in the system keyring)",
        )),
        FieldKind::Toggle,
    )
    .default_value(json!(false));
    let word = Field::draft(
        WIZARD,
        "confirm",
        &t(Message::new("gui-reset-type", "Type { $word } to confirm").with("word", WORD)),
        FieldKind::Text,
    )
    .required()
    .validate(CHECK_WORD);
    Wizard::new(
        WIZARD,
        &t(Message::new("gui-reset-wizard", "Reset Helios")),
        COMMAND,
    )
    .description(&t(Message::new(
        "gui-reset-wizard-about",
        "Helios quits, puts its own folders aside and starts again. Photos are never touched.",
    )))
    .step(
        "scope",
        &t(Message::new("gui-reset-step-scope", "What to reset")),
        |s| {
            s.field(scope);
            s.group("accounts", "")
                .group_when("form.scope == 'everything'")
                .field(accounts);
        },
    )
    .step(
        "confirm",
        &t(Message::new("gui-reset-step-confirm", "Confirm")),
        |s| {
            s.description(&t(Message::new(
                "gui-reset-confirm-about",
                "Take a catalogue snapshot first if you may want your ratings, tags and edits back. Then type the word: Helios quits and starts again.",
            )))
            .field(word);
        },
    )
    .args(json!({
        "scope": "${scope}",
        "confirm": "${confirm}",
        "forget_accounts": "${forget_accounts}",
    }))
    .no_summary()
    .cancel(Confirmation::new(
        &t(Message::new("gui-reset-cancel", "Leave the reset?")),
        "",
        &t(Message::new("gui-reset-cancel-verb", "Leave")),
    ))
    .verb(&t(Message::new("gui-reset-verb", "Reset and restart")))
}

/// The [`CHECK_WORD`] hook: a word typed that is not [`WORD`] is a
/// problem, said in the field.
pub fn check_word(t: impl Fn(Message) -> String + Send + Sync + 'static) -> Hook {
    typed_word(WORD, t)
}

/// A `validate` hook over the draft's `confirm`: a word typed that is
/// not `word` is a problem, said in the field.
pub fn typed_word(
    word: &'static str,
    t: impl Fn(Message) -> String + Send + Sync + 'static,
) -> Hook {
    Hook::Validate(Box::new(move |values| {
        let typed = values.get("confirm").and_then(Value::as_str).unwrap_or("");
        if typed.is_empty() || typed.trim() == word {
            return Vec::new();
        }
        vec![Problem {
            field: "confirm".to_owned(),
            tone: Tone::Danger,
            message: t(
                Message::new("gui-reset-word-wrong", "Type { $word } exactly").with("word", word),
            ),
        }]
    }))
}
