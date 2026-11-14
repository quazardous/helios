//! Helios's init contract: what a module asks before it can run, such as
//! the library folder, declared as data the way modules declare their
//! settings and health checks.
//!
//! - A [`Step`] has an id, a title, why it is asked, the settings it
//!   fills (so a host draws its fields with the declarative forms), the
//!   command that may fill it instead, whether it is required, a
//!   [`Done`] test read from the settings (a step whose setting is
//!   cleared later is missing again), and the steps it comes after.
//! - A module declares the steps it requires before it runs
//!   ([`Registry::require`]): import requires the library folder.
//! - The [`Registry`] holds every step by owner, ordered by their
//!   dependencies ([`Registry::ordered`]), and says which are done
//!   ([`Registry::status`]) and which a module still misses
//!   ([`Registry::missing`]), over any [`Values`].
//! - A partial run ([`ensure`]): a caller asks for some steps (and so
//!   their dependencies), maybe starting on one of them, and gets back
//!   [`Outcome::Done`] or [`Outcome::Cancelled`] with what is still
//!   missing. Nothing here assumes where the steps are drawn: a [`Host`]
//!   draws a [`Plan`] (a first-run focus screen, a wizard bound to a
//!   window, an agent filling settings) and ends it through its
//!   [`Reply`]; the outcome is read from the settings, not taken from
//!   the host.

use std::collections::BTreeMap;

use helios_core::Message;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The library folder step: where imports copy photos.
pub const LIBRARY_FOLDER: &str = "library.folder";

/// The setting the library folder step fills: the library folder's path.
pub const LIBRARY_FOLDER_SETTING: &str = "library.folder";

/// The command that fills the library folder step (its `path`, or the
/// folder chooser): the health warning's fix too.
pub const CHOOSE_LIBRARY_FOLDER: &str = "library.chooseFolder";

/// The library module, owner of the library folder step.
pub const LIBRARY_OWNER: &str = "library";

/// The import module, which requires the library folder.
pub const IMPORT: &str = "import";

/// When a step is done, read from the settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Done {
    /// Every one of these settings is set, and not empty (an empty
    /// string, array or object counts as unset).
    Set(Vec<String>),
}

impl Done {
    /// Whether the test passes over `values`.
    pub fn passes(&self, values: &dyn Values) -> bool {
        match self {
            Done::Set(keys) => keys.iter().all(|k| values.value(k).is_some_and(set)),
        }
    }
}

/// A setting's value counts as set.
fn set(value: Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

/// Where steps read the settings they fill: the node's vault and layers,
/// a desktop's, or a map in a test.
pub trait Values {
    /// Setting `key`'s value, `None` when unset.
    fn value(&self, key: &str) -> Option<Value>;
}

impl Values for BTreeMap<String, Value> {
    fn value(&self, key: &str) -> Option<Value> {
        self.get(key).cloned()
    }
}

/// A setup step a module declares.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    /// Stable: `library.folder`.
    pub id: String,
    pub title: Message,
    /// Why it is asked, one sentence for people.
    pub why: Message,
    /// The settings it fills: its fields.
    pub fills: Vec<String>,
    /// A command that fills it instead of its fields (a chooser, a sign
    /// in), when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// A required step cannot be skipped; an optional one can be left
    /// for later.
    pub required: bool,
    /// When it is done.
    pub done: Done,
    /// The steps it comes after.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
}

impl Step {
    /// A required step `id`; done once every setting it fills is set.
    pub fn new(id: &str, title: Message, why: Message) -> Self {
        Self {
            id: id.to_owned(),
            title,
            why,
            fills: Vec::new(),
            command: None,
            required: true,
            done: Done::Set(Vec::new()),
            after: Vec::new(),
        }
    }

    /// The settings it fills; done once each is set.
    #[must_use]
    pub fn fills(mut self, settings: &[&str]) -> Self {
        self.fills = settings.iter().map(|s| (*s).to_owned()).collect();
        self.done = Done::Set(self.fills.clone());
        self
    }

    /// The command that may fill it.
    #[must_use]
    pub fn command(mut self, command: &str) -> Self {
        self.command = Some(command.to_owned());
        self
    }

    /// It may be left for later.
    #[must_use]
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// It comes after `steps`.
    #[must_use]
    pub fn after(mut self, steps: &[&str]) -> Self {
        self.after = steps.iter().map(|s| (*s).to_owned()).collect();
        self
    }

    /// Whether it is done over `values`.
    pub fn is_done(&self, values: &dyn Values) -> bool {
        self.done.passes(values)
    }

    /// What is wrong with its declaration; empty: it is sound.
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if !sound_name(&self.id) {
            p.push(format!(
                "step `{}`: an id of lowercase letters, digits, `.`, `-` and `_`",
                self.id
            ));
        }
        if self.title.english.trim().is_empty() {
            p.push(format!("step `{}`: a title", self.id));
        }
        if self.why.english.trim().is_empty() {
            p.push(format!("step `{}`: why it is asked", self.id));
        }
        if self.fills.is_empty() {
            p.push(format!("step `{}`: the settings it fills", self.id));
        }
        let Done::Set(keys) = &self.done;
        if keys.is_empty() {
            p.push(format!("step `{}`: a done test naming settings", self.id));
        }
        if self.after.contains(&self.id) {
            p.push(format!("step `{}` comes after itself", self.id));
        }
        p
    }
}

/// Lowercase letters, digits, `.`, `-` and `_`, not empty.
fn sound_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
}

/// The library module's steps: the library folder.
pub fn library() -> Vec<Step> {
    vec![
        Step::new(
            LIBRARY_FOLDER,
            Message::new("init-library-folder", "Library folder"),
            Message::new(
                "init-library-folder-why",
                "Imports copy photos into it, and picked photos go there.",
            ),
        )
        .fills(&[LIBRARY_FOLDER_SETTING])
        .command(CHOOSE_LIBRARY_FOLDER),
    ]
}

/// A step and where it stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Status {
    pub id: String,
    pub owner: String,
    pub required: bool,
    pub done: bool,
}

/// Every step by owner, and the steps each module requires.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    steps: Vec<Step>,
    /// The owner of each step, by position.
    owners: Vec<String>,
    requires: BTreeMap<String, Vec<String>>,
}

impl Registry {
    /// Helios's own steps, each module's, and their requirements.
    pub fn builtin() -> Self {
        let mut r = Self::default();
        for step in library() {
            if let Err(problems) = r.add(LIBRARY_OWNER, step) {
                // Helios's own declarations are tested sound.
                log_problems(&problems);
            }
        }
        r.require(IMPORT, &[LIBRARY_FOLDER]);
        r
    }

    /// Adds `owner`'s step; one with the id of a step already there
    /// replaces it. An unsound declaration is refused, with its reasons.
    pub fn add(&mut self, owner: &str, step: Step) -> Result<(), Vec<String>> {
        let mut problems = step.problems();
        if !sound_name(owner) {
            problems.insert(
                0,
                format!("owner `{owner}`: lowercase letters, digits, `.`, `-` and `_`"),
            );
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        if let Some(at) = self.steps.iter().position(|s| s.id == step.id) {
            self.steps.remove(at);
            self.owners.remove(at);
        }
        self.steps.push(step);
        self.owners.push(owner.to_owned());
        Ok(())
    }

    /// Module `module` requires `steps` to be done before it runs.
    pub fn require(&mut self, module: &str, steps: &[&str]) {
        let list = self.requires.entry(module.to_owned()).or_default();
        for s in steps {
            if !list.iter().any(|l| l == s) {
                list.push((*s).to_owned());
            }
        }
    }

    /// The steps each module requires, by module.
    pub fn requirements(&self) -> &BTreeMap<String, Vec<String>> {
        &self.requires
    }

    pub fn step(&self, id: &str) -> Option<&Step> {
        self.steps.iter().find(|s| s.id == id)
    }

    /// The owner of step `id`.
    pub fn owner(&self, id: &str) -> Option<&str> {
        let at = self.steps.iter().position(|s| s.id == id)?;
        Some(&self.owners[at])
    }

    /// Every step, each after the steps it comes after, otherwise in the
    /// order they were added.
    ///
    /// # Errors
    /// A step comes after one nobody declared, or steps come after each
    /// other in a loop.
    pub fn ordered(&self) -> Result<Vec<&Step>, String> {
        for step in &self.steps {
            if let Some(unknown) = step.after.iter().find(|a| self.step(a).is_none()) {
                return Err(format!(
                    "step `{}` comes after `{unknown}`, which no module declares",
                    step.id
                ));
            }
        }
        let mut out: Vec<&Step> = Vec::with_capacity(self.steps.len());
        while out.len() < self.steps.len() {
            let next = self.steps.iter().find(|s| {
                !out.iter().any(|o| o.id == s.id)
                    && s.after.iter().all(|a| out.iter().any(|o| &o.id == a))
            });
            let Some(s) = next else {
                let looped: Vec<&str> = self
                    .steps
                    .iter()
                    .filter(|s| !out.iter().any(|o| o.id == s.id))
                    .map(|s| s.id.as_str())
                    .collect();
                return Err(format!(
                    "steps come after each other in a loop: {}",
                    looped.join(", ")
                ));
            };
            out.push(s);
        }
        Ok(out)
    }

    /// Every step in order, with whether it is done over `values`.
    ///
    /// # Errors
    /// As [`Registry::ordered`].
    pub fn status(&self, values: &dyn Values) -> Result<Vec<Status>, String> {
        Ok(self
            .ordered()?
            .into_iter()
            .map(|s| Status {
                id: s.id.clone(),
                owner: self.owner(&s.id).unwrap_or_default().to_owned(),
                required: s.required,
                done: s.is_done(values),
            })
            .collect())
    }

    /// `steps` and every step they come after, in order.
    ///
    /// # Errors
    /// An unknown step, or as [`Registry::ordered`].
    pub fn with_dependencies(&self, steps: &[String]) -> Result<Vec<&Step>, String> {
        let mut wanted: Vec<&str> = Vec::new();
        let mut todo: Vec<&str> = steps.iter().map(String::as_str).collect();
        while let Some(id) = todo.pop() {
            if wanted.contains(&id) {
                continue;
            }
            let step = self
                .step(id)
                .ok_or_else(|| format!("no init step `{id}`"))?;
            wanted.push(id);
            todo.extend(step.after.iter().map(String::as_str));
        }
        Ok(self
            .ordered()?
            .into_iter()
            .filter(|s| wanted.contains(&s.id.as_str()))
            .collect())
    }

    /// The steps module `module` still misses before it runs: those it
    /// requires and those they come after, not done over `values`, in
    /// order. Empty: it can run (a module requiring nothing too).
    ///
    /// # Errors
    /// A requirement names an unknown step, or as [`Registry::ordered`].
    pub fn missing(&self, module: &str, values: &dyn Values) -> Result<Vec<String>, String> {
        let required = self.requires.get(module).cloned().unwrap_or_default();
        Ok(self
            .with_dependencies(&required)?
            .into_iter()
            .filter(|s| !s.is_done(values))
            .map(|s| s.id.clone())
            .collect())
    }

    /// What a host draws for `request`: the steps asked and those they
    /// come after, those not done yet, in order, opening on the step
    /// asked first when it is among them.
    ///
    /// # Errors
    /// An unknown step, a first step not asked, or as
    /// [`Registry::ordered`].
    pub fn plan(&self, request: &Request, values: &dyn Values) -> Result<Plan, String> {
        let asked = match &request.steps {
            Some(steps) => self.with_dependencies(steps)?,
            None => self.ordered()?,
        };
        if let Some(first) = &request.start
            && !asked.iter().any(|s| &s.id == first)
        {
            return Err(format!("the first step `{first}` is not among those asked"));
        }
        let steps: Vec<Step> = asked
            .into_iter()
            .filter(|s| !s.is_done(values))
            .cloned()
            .collect();
        let start = request
            .start
            .as_ref()
            .and_then(|first| steps.iter().position(|s| &s.id == first))
            .unwrap_or(0);
        Ok(Plan { steps, start })
    }
}

fn log_problems(problems: &[String]) {
    // No logger dependency: a broken built-in declaration fails its test.
    debug_assert!(problems.is_empty(), "init: {}", problems.join("; "));
}

/// What a caller asks: some steps (and so those they come after), or all
/// of them, maybe opening on one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// The steps asked; none: every step (the first run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<String>>,
    /// The step to open on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
}

impl Request {
    /// Every step: the first run.
    pub fn all() -> Self {
        Self::default()
    }

    /// `steps`, and those they come after.
    pub fn steps(steps: &[&str]) -> Self {
        Self {
            steps: Some(steps.iter().map(|s| (*s).to_owned()).collect()),
            start: None,
        }
    }

    /// Opens on step `id`.
    #[must_use]
    pub fn starting_at(mut self, id: &str) -> Self {
        self.start = Some(id.to_owned());
        self
    }
}

/// The steps a host draws, in order, and the one it opens on.
#[derive(Clone, Debug, Serialize)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// The index of the step to open on.
    pub start: usize,
}

impl Plan {
    /// Nothing to ask: every step asked is done.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// The outcome over `values`: done when every required step of the
    /// plan is, cancelled with the required ones still missing.
    pub fn outcome(&self, values: &dyn Values) -> Outcome {
        let missing: Vec<String> = self
            .steps
            .iter()
            .filter(|s| s.required && !s.is_done(values))
            .map(|s| s.id.clone())
            .collect();
        if missing.is_empty() {
            Outcome::Done
        } else {
            Outcome::Cancelled { missing }
        }
    }
}

/// How a run ended, for its caller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// Every required step asked is done.
    Done,
    /// Left before: the required steps still missing, for the caller to
    /// say why it cannot run yet.
    Cancelled { missing: Vec<String> },
}

/// The way back to the caller of a run, handed to its host.
pub struct Reply {
    plan: Plan,
    to: Box<dyn FnOnce(Outcome) + Send>,
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reply")
            .field("plan", &self.plan)
            .finish_non_exhaustive()
    }
}

impl Reply {
    /// The host is done with the run (finished or left): the caller gets
    /// the outcome read from `values`, whatever the host thinks.
    pub fn end(self, values: &dyn Values) -> Outcome {
        let outcome = self.plan.outcome(values);
        (self.to)(outcome.clone());
        outcome
    }
}

/// Where a run is drawn: a first-run focus screen, a wizard bound to a
/// window, an agent's own loop. It shows the plan's steps from its start,
/// lets the user fill them (their settings, or their command), and ends
/// the run through the [`Reply`] once finished or left.
pub trait Host {
    fn run(&mut self, plan: Plan, reply: Reply);
}

/// Makes sure the steps `request` asks are done: when they are already,
/// `done` hears [`Outcome::Done`] at once and `host` is not asked;
/// otherwise `host` runs the plan, and `done` hears its outcome when the
/// host ends it.
///
/// # Errors
/// As [`Registry::plan`]; `done` is not called then.
pub fn ensure(
    registry: &Registry,
    values: &dyn Values,
    request: &Request,
    host: &mut dyn Host,
    done: impl FnOnce(Outcome) + Send + 'static,
) -> Result<(), String> {
    let plan = registry.plan(request, values)?;
    if plan.is_empty() {
        done(Outcome::Done);
        return Ok(());
    }
    let reply = Reply {
        plan: plan.clone(),
        to: Box::new(done),
    };
    host.run(plan, reply);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::*;

    /// A text for a test step, kept out of the catalogue: the extractor
    /// reads `Message::new` with literal texts only.
    fn text(english: &str) -> Message {
        Message::new(&format!("test-{english}"), english)
    }

    fn step(id: &str, after: &[&str]) -> Step {
        Step::new(id, text(id), text("why"))
            .fills(&[id])
            .after(after)
    }

    fn values(set: &[(&str, Value)]) -> BTreeMap<String, Value> {
        set.iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    fn ids(steps: &[&Step]) -> Vec<String> {
        steps.iter().map(|s| s.id.clone()).collect()
    }

    #[test]
    fn helios_own_steps_are_sound() {
        let r = Registry::builtin();
        for s in library() {
            assert_eq!(s.problems(), Vec::<String>::new(), "{}", s.id);
        }
        assert_eq!(ids(&r.ordered().unwrap()), [LIBRARY_FOLDER]);
        assert_eq!(r.owner(LIBRARY_FOLDER), Some(LIBRARY_OWNER));
        assert_eq!(r.requirements()[IMPORT], [LIBRARY_FOLDER]);
    }

    #[test]
    fn steps_come_after_their_dependencies() {
        let mut r = Registry::default();
        r.add("b", step("connect", &["backup", "folder"])).unwrap();
        r.add("b", step("backup", &["folder"])).unwrap();
        r.add("a", step("folder", &[])).unwrap();
        r.add("c", step("theme", &[])).unwrap();
        assert_eq!(
            ids(&r.ordered().unwrap()),
            ["folder", "backup", "connect", "theme"]
        );

        r.add("a", step("folder", &["connect"])).unwrap();
        let err = r.ordered().unwrap_err();
        assert!(err.contains("loop"), "{err}");

        let mut r = Registry::default();
        r.add("a", step("x", &["nowhere"])).unwrap();
        assert!(r.ordered().unwrap_err().contains("nowhere"));
    }

    #[test]
    fn an_unsound_step_is_refused() {
        let mut r = Registry::default();
        let bad = Step::new("Bad Id", text(""), text(""));
        let problems = r.add("Owner", bad).unwrap_err();
        assert_eq!(problems.len(), 6, "{problems:?}");
        assert!(r.ordered().unwrap().is_empty());
    }

    #[test]
    fn status_follows_the_setting_set_and_cleared() {
        let r = Registry::builtin();
        let done = |v: &BTreeMap<String, Value>| r.status(v).unwrap()[0].done;
        assert!(!done(&values(&[])));
        assert!(done(&values(&[(
            LIBRARY_FOLDER_SETTING,
            json!("/p/Helios")
        )])));
        assert!(!done(&values(&[(LIBRARY_FOLDER_SETTING, json!(""))])));
        assert!(!done(&values(&[(LIBRARY_FOLDER_SETTING, Value::Null)])));
    }

    #[test]
    fn a_requirement_resolves_to_its_missing_steps() {
        let mut r = Registry::builtin();
        r.add("backup", step("backup.disk", &[LIBRARY_FOLDER]))
            .unwrap();
        r.require("backup", &["backup.disk"]);
        let none = values(&[]);
        assert_eq!(r.missing(IMPORT, &none).unwrap(), [LIBRARY_FOLDER]);
        assert_eq!(
            r.missing("backup", &none).unwrap(),
            [LIBRARY_FOLDER, "backup.disk"]
        );
        assert!(r.missing("export", &none).unwrap().is_empty());
        let chosen = values(&[(LIBRARY_FOLDER_SETTING, json!("/p"))]);
        assert!(r.missing(IMPORT, &chosen).unwrap().is_empty());
        assert_eq!(r.missing("backup", &chosen).unwrap(), ["backup.disk"]);
    }

    /// A host that remembers what it was asked and ends when told.
    #[derive(Default)]
    struct Fake {
        shown: Option<(Vec<String>, usize)>,
        reply: Option<Reply>,
    }

    impl Host for Fake {
        fn run(&mut self, plan: Plan, reply: Reply) {
            self.shown = Some((
                plan.steps.iter().map(|s| s.id.clone()).collect(),
                plan.start,
            ));
            self.reply = Some(reply);
        }
    }

    #[test]
    fn a_partial_run_asks_the_steps_and_their_dependencies_and_answers_its_caller() {
        let mut r = Registry::builtin();
        r.add("backup", step("backup.disk", &[LIBRARY_FOLDER]))
            .unwrap();
        r.add("ai", step("ai.provider", &[]).optional()).unwrap();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let listen = |heard: &Arc<Mutex<Vec<Outcome>>>| {
            let heard = heard.clone();
            move |o| heard.lock().unwrap().push(o)
        };

        // Cancelled: the caller hears what is missing.
        let mut host = Fake::default();
        let request = Request::steps(&["backup.disk"]).starting_at("backup.disk");
        ensure(&r, &values(&[]), &request, &mut host, listen(&heard)).unwrap();
        assert_eq!(
            host.shown,
            Some((vec![LIBRARY_FOLDER.into(), "backup.disk".into()], 1))
        );
        host.reply.take().unwrap().end(&values(&[]));
        assert_eq!(
            heard.lock().unwrap().pop(),
            Some(Outcome::Cancelled {
                missing: vec![LIBRARY_FOLDER.into(), "backup.disk".into()]
            })
        );

        // Filled: done, whatever step the host left on.
        let mut host = Fake::default();
        let request = Request::steps(&[LIBRARY_FOLDER]);
        ensure(&r, &values(&[]), &request, &mut host, listen(&heard)).unwrap();
        assert_eq!(host.shown, Some((vec![LIBRARY_FOLDER.into()], 0)));
        let filled = values(&[(LIBRARY_FOLDER_SETTING, json!("/p"))]);
        assert_eq!(host.reply.take().unwrap().end(&filled), Outcome::Done);
        assert_eq!(heard.lock().unwrap().pop(), Some(Outcome::Done));

        // Already done: the host is not asked.
        let mut host = Fake::default();
        ensure(&r, &filled, &request, &mut host, listen(&heard)).unwrap();
        assert!(host.shown.is_none());
        assert_eq!(heard.lock().unwrap().pop(), Some(Outcome::Done));

        // The first run asks every step; an optional one left is no miss.
        let plan = r.plan(&Request::all(), &filled).unwrap();
        let shown: Vec<&str> = plan.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(shown, ["backup.disk", "ai.provider"]);
        let disk = values(&[
            (LIBRARY_FOLDER_SETTING, json!("/p")),
            ("backup.disk", json!("/d")),
        ]);
        assert_eq!(plan.outcome(&disk), Outcome::Done);

        // A first step not asked, or an unknown step, is refused.
        assert!(
            r.plan(
                &Request::steps(&[LIBRARY_FOLDER]).starting_at("ai.provider"),
                &filled
            )
            .is_err()
        );
        assert!(r.plan(&Request::steps(&["nope"]), &filled).is_err());
    }
}
