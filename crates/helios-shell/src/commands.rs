//! Commands: every action of the application, named. A menu, a key, the
//! palette, an agent through MCP and a test all run the same command by
//! its id, and each run says where it came from.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::when::{Context, When};

/// Who asked for a command: shown in the history, and used to keep an
/// agent from fighting a human's gesture (`docs/widgets.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum Origin {
    User,
    Agent(String),
    Script(String),
    Midi,
    Remote(String),
}

/// What a command declares.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CommandSpec {
    /// Stable, never translated: `view.palette`, `culling.rate.3`.
    pub id: String,
    pub title: String,
    pub category: String,
    /// Where the command applies; empty: everywhere.
    pub when: String,
    /// It only reads: what a status or a record table of the Settings app
    /// may run to show its value, and what an agent may run when it may
    /// only read.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    /// It takes `dry_run`: says what it would do and changes nothing. A
    /// dry run of a command that does not is refused before it runs
    /// ([`crate::policy::dry_run`]).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
}

impl CommandSpec {
    pub fn new(id: &str, title: &str, category: &str) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            category: category.into(),
            when: String::new(),
            read_only: false,
            dry_run: false,
        }
    }

    pub fn when(mut self, when: &str) -> Self {
        self.when = when.into();
        self
    }

    /// Declares it read-only: it changes nothing.
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// Declares that it takes `dry_run`: with it, it says what it would
    /// do and changes nothing.
    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }
}

/// A command's handler, shared: [`Registry::reader`] hands a read-only
/// one out, to be called away from the registry's lock.
pub type Handler = std::sync::Arc<dyn Fn(&Value, &Origin) -> Result<Value, String> + Send + Sync>;

struct Entry {
    spec: CommandSpec,
    when: When,
    handler: Handler,
}

/// One run, as the history keeps it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Run {
    pub id: String,
    pub args: Value,
    pub origin: Origin,
    pub result: Result<Value, String>,
}

#[derive(Default)]
pub struct Registry {
    commands: BTreeMap<String, Entry>,
    history: Vec<Run>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares a command; a second declaration of an id is refused.
    pub fn register(
        &mut self,
        spec: CommandSpec,
        handler: impl Fn(&Value, &Origin) -> Result<Value, String> + Send + Sync + 'static,
    ) -> Result<(), String> {
        let when = When::parse(&spec.when).map_err(|e| format!("command `{}`: {e}", spec.id))?;
        if self.commands.contains_key(&spec.id) {
            return Err(format!("command `{}` is already declared", spec.id));
        }
        self.commands.insert(
            spec.id.clone(),
            Entry {
                spec,
                when,
                handler: std::sync::Arc::new(handler),
            },
        );
        Ok(())
    }

    /// A read-only command's handler, to call without holding the
    /// registry (a read that takes its time, on another thread); `None`
    /// for an unknown command or one that changes something. The run is
    /// not kept in the history.
    pub fn reader(&self, id: &str) -> Option<Handler> {
        self.commands
            .get(id)
            .filter(|e| e.spec.read_only)
            .map(|e| e.handler.clone())
    }

    pub fn spec(&self, id: &str) -> Option<&CommandSpec> {
        self.commands.get(id).map(|e| &e.spec)
    }

    /// Every declared command, wherever it applies, by id: what tools such
    /// as the English catalog's generation read.
    pub fn specs(&self) -> impl Iterator<Item = &CommandSpec> {
        self.commands.values().map(|e| &e.spec)
    }

    /// The commands that apply in a context, for the palette.
    pub fn available(&self, cx: &Context) -> Vec<&CommandSpec> {
        self.commands
            .values()
            .filter(|e| e.when.eval(cx))
            .map(|e| &e.spec)
            .collect()
    }

    /// Commands whose title or id contains every word of `query`, in that
    /// context: what the palette lists while one types.
    pub fn search(&self, query: &str, cx: &Context) -> Vec<&CommandSpec> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.available(cx)
            .into_iter()
            .filter(|s| {
                let hay = format!("{} {} {}", s.category, s.title, s.id).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .collect()
    }

    /// Runs a command; refused when unknown or when it does not apply here.
    pub fn run(
        &mut self,
        id: &str,
        args: Value,
        origin: Origin,
        cx: &Context,
    ) -> Result<Value, String> {
        let entry = self
            .commands
            .get(id)
            .ok_or_else(|| format!("no command `{id}`"))?;
        let result = if entry.when.eval(cx) {
            (entry.handler)(&args, &origin)
        } else {
            Err(format!("`{id}` does not apply here ({})", entry.spec.when))
        };
        self.history.push(Run {
            id: id.into(),
            args,
            origin,
            result: result.clone(),
        });
        result
    }

    /// Every run so far, oldest first.
    pub fn history(&self) -> &[Run] {
        &self.history
    }
}
