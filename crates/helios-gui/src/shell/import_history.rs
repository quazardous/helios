//! The import history on the desktop (`helios_library::import_history`):
//! the imports the library recorded, whoever ran them (this window, the
//! node, the CLI, an agent), listed at the foot of the import window's
//! Sources column, each with "Import again".
//!
//! - `import.history.list` (`limit`): the records, newest first.
//! - `import.history.again` (`id`): the import run again with its
//!   sources and choices, queued as `library.import` is, as whoever asks.
//!   An importer plugin's import is run again from its menu entry.

use std::sync::Arc;
use std::time::{Duration, Instant};

use helios_core::Message;
use helios_library::import_history::Record;
use helios_shell::commands::Origin;
use serde_json::{Value, json};

use super::Shell;

/// The command listing the history.
pub const LIST: &str = "import.history.list";
/// The command running one import of it again.
pub const AGAIN: &str = "import.history.again";

/// How many imports the window lists.
const SHOWN: usize = 10;

/// How often the open window reads the history again: an import ending,
/// here or on the node, shows within it.
const READ_EVERY: Duration = Duration::from_secs(2);

/// Reads the history from the library, newest first.
pub type Reader = Arc<dyn Fn() -> Result<Vec<Record>, String> + Send + Sync>;

/// A [`Reader`] of `vault`'s history.
pub fn reader_of<S: helios_vault::Store + 'static>(vault: Arc<helios_vault::Vault<S>>) -> Reader {
    Arc::new(move || helios_library::import_history::list(&vault, None))
}

/// The window's part: the reader, and the records last read.
#[derive(Default)]
pub(super) struct History {
    reader: Option<Reader>,
    records: Vec<Record>,
    read_at: Option<Instant>,
}

impl History {
    /// Reads the records again when they are older than [`READ_EVERY`].
    pub(super) fn follow(&mut self) {
        let Some(reader) = &self.reader else { return };
        if self.read_at.is_some_and(|at| at.elapsed() < READ_EVERY) {
            return;
        }
        self.read_at = Some(Instant::now());
        // A history that cannot be read lists nothing; the commands say
        // why.
        self.records = reader().unwrap_or_default();
    }
}

/// The history's words, in the shell's language.
pub(super) struct Words {
    title: String,
    none: String,
    again: String,
}

impl Words {
    pub(super) fn new(t: &dyn Fn(Message) -> String) -> Self {
        Words {
            title: t(Message::new("gui-import-history", "History")),
            none: t(Message::new("gui-import-history-none", "No import yet")),
            again: t(Message::new("gui-import-history-again", "Import again")),
        }
    }
}

/// A record's line: its source, when, and what came in.
fn line(r: &Record, t: &dyn Fn(Message) -> String) -> String {
    let when = chrono::DateTime::from_timestamp(i64::try_from(r.when).unwrap_or(0), 0)
        .map(|utc| {
            utc.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    let what = if r.error.is_some() {
        t(Message::new("gui-import-history-failed", "failed"))
    } else {
        t(Message::new("gui-import-history-added", "{ $count } added")
            .with("count", r.counts.added + r.counts.located))
    };
    format!("{} · {when} · {what}", r.source_label())
}

/// A record's details, on hover: who asked, every source, the counts.
fn details(r: &Record, t: &dyn Fn(Message) -> String) -> String {
    let mut out = Vec::new();
    if !r.origin.is_empty() {
        out.push(t(Message::new("gui-import-history-by", "By { $origin }")
            .with("origin", r.origin.clone())));
    }
    out.extend(r.sources.iter().map(|s| s.display().to_string()));
    if let Some(e) = &r.error {
        out.push(e.clone());
    } else {
        let c = &r.counts;
        out.push(t(Message::new(
            "gui-import-history-counts",
            "{ $added } added, { $located } located, { $unchanged } unchanged, { $skipped } skipped, { $failed } failed",
        )
        .with("added", c.added)
        .with("located", c.located)
        .with("unchanged", c.unchanged)
        .with("skipped", c.skipped + c.asked)
        .with("failed", c.failed)));
    }
    out.join("\n")
}

/// The history at the foot of the Sources column: the id of the import
/// asked to run again, if one was.
pub(super) fn history_ui(
    ui: &mut egui::Ui,
    history: &History,
    words: &Words,
    t: &dyn Fn(Message) -> String,
) -> Option<String> {
    history.reader.as_ref()?;
    let mut asked = None;
    ui.separator();
    ui.strong(&words.title);
    if history.records.is_empty() {
        ui.weak(&words.none);
    }
    for r in history.records.iter().take(SHOWN) {
        let text = line(r, t);
        ui.horizontal(|ui| {
            ui.label(&text).on_hover_text(details(r, t));
            // An importer's import is run again from its menu entry.
            if r.importer.is_none() {
                let button = ui.small_button(&words.again);
                // Each row's button says which import it runs again.
                let name = format!("{}: {text}", words.again);
                button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
                if button.on_hover_text(&words.again).clicked() {
                    asked = Some(r.id.clone());
                }
            }
        });
    }
    asked
}

impl Shell {
    /// Where the import window and the history's commands read the
    /// library's import history.
    pub fn set_import_history(&mut self, reader: Reader) {
        let mut w = self.import.get_mut();
        w.history.reader = Some(reader);
        w.history.read_at = None;
    }

    fn history_reader(&self) -> Result<Reader, String> {
        self.import
            .get()
            .history
            .reader
            .clone()
            .ok_or_else(|| "no library open: it keeps the import history".to_string())
    }

    /// `import.history.list`: the records, newest first (`limit`).
    pub(super) fn import_history_list(&self, args: &Value) -> Result<Value, String> {
        let mut records = self.history_reader()?()?;
        if let Some(limit) = args.get("limit").and_then(Value::as_u64) {
            records.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        }
        Ok(json!({ "imports": records }))
    }

    /// `import.history.again`: the import `id` queued again with its
    /// sources and choices, as `origin`.
    pub(super) fn import_again(&mut self, args: &Value, origin: &Origin) -> Result<Value, String> {
        self.can_import()?;
        let id = args
            .get("id")
            .and_then(Value::as_str)
            .ok_or("`id` is missing: an import of `import.history.list`")?;
        let record = self.history_reader()?()?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| format!("no import `{id}` in the history"))?;
        let mut params = record.params()?;
        params["origin"] = json!(crate::persist::who(origin));
        self.core.imports.push(params.clone());
        // The window lists the new record once it is written.
        self.import.get_mut().history.read_at = None;
        Ok(json!({ "queued": params }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_says_the_source_and_what_came_in() {
        let t = |m: Message| m.english;
        let r = Record {
            sources: vec!["/media/EOS_DIGITAL".into(), "/photos/b".into()],
            counts: helios_library::import_history::Counts {
                added: 3,
                located: 1,
                ..Default::default()
            },
            origin: "agent:tester".into(),
            ..Record::default()
        };
        assert!(line(&r, &t).starts_with("EOS_DIGITAL (+1) · "));
        assert!(details(&r, &t).contains("/photos/b"));
    }
}
