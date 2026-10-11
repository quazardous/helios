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
}

impl CommandSpec {
    pub fn new(id: &str, title: &str, category: &str) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            category: category.into(),
            when: String::new(),
        }
    }

    pub fn when(mut self, when: &str) -> Self {
        self.when = when.into();
        self
    }
}

type Handler = Box<dyn Fn(&Value, &Origin) -> Result<Value, String> + Send + Sync>;

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
                handler: Box::new(handler),
            },
        );
        Ok(())
    }

    pub fn spec(&self, id: &str) -> Option<&CommandSpec> {
        self.commands.get(id).map(|e| &e.spec)
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
