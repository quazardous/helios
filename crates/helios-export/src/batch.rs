//! Batch exports: many images written by one request.
//!
//! A batch is described by a [`BatchRequest`], plain data that serializes
//! to JSON: the settings (format, colour space, size, folder, how names
//! clash) and one [`Item`] per image, its output path already decided
//! when the batch was asked for (see `docs/export.md`). The same request
//! is meant to run here, on the desktop's scheduler ([`run`]), or later
//! as a workflow run on a node, which keeps its items as journal rows.
//!
//! [`run`] reports through `helios_core::jobs`: a parent job "Export N
//! photos" (kind `batch`) and a child export per image under it, queued
//! at once on the scheduler, which runs as many side by side as it has
//! workers. Cancelling the parent cancels the exports still queued or
//! running; each image that fails is listed under the parent with the
//! reason; the parent ends finished (offering `actions`, such as opening
//! the folder), failed when an image failed, or cancelled.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use helios_core::jobs::{Jobs, Progress, Report, Start};
use helios_core::{Image, JobId, Message, Scheduler};
use serde::{Deserialize, Serialize};

use crate::{End, InBatch, Output};

/// The request format this Helios writes and reads; a newer one is
/// refused, never guessed.
pub const FORMAT: u32 = 1;

/// What happens when an image's file name is taken already.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Clash {
    /// Another name: `name-2.jpg`, `name-3.jpg`…
    #[default]
    Number,
    /// The file on disk is replaced.
    Overwrite,
    /// The image is left out: the file on disk stays.
    Skip,
}

impl Clash {
    /// Its word, as a template and a request write it.
    pub fn word(self) -> &'static str {
        match self {
            Clash::Number => "number",
            Clash::Overwrite => "overwrite",
            Clash::Skip => "skip",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "number" => Some(Clash::Number),
            "overwrite" => Some(Clash::Overwrite),
            "skip" => Some(Clash::Skip),
            _ => None,
        }
    }
}

/// The name pattern when none is given: the original name.
pub const DEFAULT_PATTERN: &str = "{stem}";

fn default_pattern() -> String {
    DEFAULT_PATTERN.into()
}

/// How a batch writes its files.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Format, colour space and size.
    pub output: Output,
    /// The folder written to, absolute.
    pub folder: PathBuf,
    /// The file names, without extension: the import's renaming fields
    /// (`{stem}`, `{date}`, `{seq}`…). Resolved into each item's output
    /// when the batch is asked for; kept for the record.
    #[serde(default = "default_pattern")]
    pub pattern: String,
    #[serde(default)]
    pub clash: Clash,
}

/// The extension a format's files take.
pub fn extension(format: &crate::Format) -> &'static str {
    match format {
        crate::Format::Jpeg { .. } => "jpg",
        crate::Format::Png { .. } => "png",
        crate::Format::Tiff => "tif",
    }
}

/// One image of a batch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// The image: its library id (64 hex digits).
    pub image: String,
    /// The version exported, the image's head when the batch was asked
    /// for; none for an image never edited (the network as it is).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where the image is read from.
    pub source: PathBuf,
    /// Where it is written: decided when the batch was asked for, never
    /// again against the files on disk.
    pub output: PathBuf,
    /// Left out, and why (its file exists and clashes skip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<String>,
    /// Refused when the batch was asked for, and why (no copy of the
    /// image is present): a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Item {
    /// What people call it: its file's name, else its id.
    pub fn name(&self) -> String {
        self.source.file_name().map_or_else(
            || self.image.chars().take(12).collect(),
            |n| n.to_string_lossy().into_owned(),
        )
    }
}

/// A batch export, as asked for: what a local run takes, and what a node
/// would take as a workflow run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BatchRequest {
    /// [`FORMAT`] when written.
    pub format: u32,
    /// The graph node rendered, by name: each export names it, so that
    /// node's queue lists them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    pub settings: Settings,
    pub items: Vec<Item>,
}

impl BatchRequest {
    /// A request of this Helios's format.
    pub fn new(node: Option<String>, settings: Settings, items: Vec<Item>) -> Self {
        BatchRequest {
            format: FORMAT,
            node,
            settings,
            items,
        }
    }

    /// Read from JSON, a newer format refused.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let request: BatchRequest =
            serde_json::from_str(json).map_err(|e| format!("batch request: {e}"))?;
        request.check()?;
        Ok(request)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a request serializes")
    }

    /// Refuses what this Helios cannot run.
    pub fn check(&self) -> Result<(), String> {
        if self.format > FORMAT {
            return Err(format!(
                "batch request: format {} is newer than this Helios's ({FORMAT})",
                self.format
            ));
        }
        if !self.settings.folder.is_absolute() {
            return Err(format!(
                "batch request: the folder {} is not absolute",
                self.settings.folder.display()
            ));
        }
        Ok(())
    }
}

/// What a batch says as it goes, to whoever asked for it.
#[derive(Clone, Debug)]
pub enum Line {
    /// An image failed (`item`: its name), and why.
    Failed {
        item: String,
        source: PathBuf,
        error: String,
    },
    /// The batch ended.
    Ended(Summary),
}

/// How a batch ended.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Summary {
    /// The parent job's id.
    pub job: String,
    /// The files written, in the order they were.
    pub written: Vec<PathBuf>,
    pub failed: usize,
    pub skipped: usize,
    /// Images cancelled (the batch, or the image alone).
    pub cancelled: usize,
    /// The batch itself was cancelled.
    pub interrupted: bool,
    /// The folder written to.
    pub folder: PathBuf,
}

/// How an image is rendered: the image's network with its version, as
/// the item names them.
pub type Render = Arc<dyn Fn(&Item) -> Result<Image, String> + Send + Sync>;

/// The batches' job ids in this process: `batch-export-1`…
static NEXT: AtomicU64 = AtomicU64::new(1);

/// A batch's counts as its exports end.
struct Tally {
    total: u64,
    remaining: usize,
    summary: Summary,
    /// The exports' scheduler jobs, which cancelling the batch cancels.
    children: Vec<JobId>,
}

impl Tally {
    fn progress(&self) -> Progress {
        let done = self.total - self.remaining as u64;
        Progress::new()
            .counts(done, self.total)
            .failed(self.summary.failed as u64)
            .skipped(self.summary.skipped as u64)
    }
}

/// Says the batch's end, once its last image ended.
fn finish(report: &Report, summary: &Summary, total: u64, actions: &serde_json::Value) {
    let written = summary.written.len();
    report.log(format!(
        "{written} written, {} failed, {} skipped, {} cancelled",
        summary.failed, summary.skipped, summary.cancelled
    ));
    if summary.interrupted {
        report.cancelled();
    } else if summary.failed > 0 {
        report.failed(&format!("{} of {total} photos failed", summary.failed));
    } else {
        report.finished_with(actions.clone());
    }
}

/// Runs a batch on `scheduler`, reporting to `jobs`: its parent job's id.
/// `render` renders each image; `said` hears each failure and the end;
/// `actions` are what the finished batch offers (`finished_with`'s).
pub fn run(
    scheduler: &Scheduler,
    jobs: &Jobs,
    request: &BatchRequest,
    render: Render,
    said: impl Fn(Line) + Send + Sync + 'static,
    actions: serde_json::Value,
) -> Result<String, String> {
    request.check()?;
    let total = request.items.len() as u64;
    let id = format!("batch-export-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let report = Arc::new(
        jobs.start(Start::new(&id, &format!("Export {total} photos"), "batch").total(total)),
    );
    let said: Arc<dyn Fn(Line) + Send + Sync> = Arc::new(said);
    let todo: Vec<&Item> = request
        .items
        .iter()
        .filter(|i| i.skip.is_none() && i.error.is_none())
        .collect();
    let tally = Arc::new(Mutex::new(Tally {
        total,
        remaining: todo.len(),
        summary: Summary {
            job: id.clone(),
            folder: request.settings.folder.clone(),
            ..Summary::default()
        },
        children: Vec::new(),
    }));
    let stop = Arc::new(AtomicBool::new(false));
    // Left out or refused when asked for: said at once.
    {
        let mut t = tally.lock().expect("tally lock");
        for item in &request.items {
            if let Some(why) = &item.skip {
                t.summary.skipped += 1;
                report.log(format!("{}: skipped: {why}", item.name()));
            } else if let Some(why) = &item.error {
                t.summary.failed += 1;
                report.item_failed(&item.name(), why);
                report.log(format!("{}: {why}", item.name()));
                said(Line::Failed {
                    item: item.name(),
                    source: item.source.clone(),
                    error: why.clone(),
                });
            }
        }
        report.progress(t.progress());
        if todo.is_empty() {
            let summary = t.summary.clone();
            drop(t);
            finish(&report, &summary, total, &actions);
            said(Line::Ended(summary));
            return Ok(id);
        }
    }
    // Cancelled: the exports not ended yet are cancelled, queued or
    // running, and the batch ends cancelled once they all said so.
    {
        let (stop, tally, canceller) = (stop.clone(), tally.clone(), scheduler.canceller());
        report.on_cancel(move || {
            stop.store(true, Ordering::SeqCst);
            // Not under the lock: a queued export cancelled ends at once.
            let children = tally.lock().expect("tally lock").children.clone();
            for job in children {
                canceller.cancel(job);
            }
        });
    }
    let actions = Arc::new(actions);
    for item in todo {
        let ended = {
            let (tally, report, said, stop) =
                (tally.clone(), report.clone(), said.clone(), stop.clone());
            let actions = actions.clone();
            let (name, source) = (item.name(), item.source.clone());
            Box::new(move |end: End| {
                let mut t = tally.lock().expect("tally lock");
                match end {
                    End::Written(e) => t.summary.written.push(e.path),
                    End::Cancelled => t.summary.cancelled += 1,
                    End::Failed(why) => {
                        t.summary.failed += 1;
                        report.item_failed(&name, &why.english);
                        report.log(format!("{name}: {}", why.english));
                        said(Line::Failed {
                            item: name,
                            source,
                            error: why.english,
                        });
                    }
                }
                t.remaining -= 1;
                report.progress(t.progress());
                if t.remaining == 0 {
                    t.summary.interrupted = stop.load(Ordering::SeqCst);
                    let summary = t.summary.clone();
                    let total = t.total;
                    drop(t);
                    finish(&report, &summary, total, &actions);
                    said(Line::Ended(summary));
                }
            })
        };
        let render = {
            let (render, item) = (render.clone(), item.clone());
            move || render(&item)
        };
        let job = crate::submit_counted(
            scheduler,
            jobs,
            InBatch { parent: &id, ended },
            request.node.as_deref(),
            &item.output,
            request.settings.output.clone(),
            render,
        );
        let cancel_now = {
            let mut t = tally.lock().expect("tally lock");
            t.children.push(job);
            stop.load(Ordering::SeqCst)
        };
        // Cancelled while the exports were being queued.
        if cancel_now {
            scheduler.cancel(job);
        }
    }
    Ok(id)
}

/// The folder a batch writes to, made when missing.
pub fn make_folder(folder: &Path) -> Result<(), Message> {
    std::fs::create_dir_all(folder).map_err(|e| {
        Message::new(
            "export-folder-failed",
            "cannot make the folder { $folder }: { $error }",
        )
        .with("folder", folder)
        .with("error", e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Format;

    fn settings() -> Settings {
        Settings {
            output: Output {
                format: Format::Jpeg { quality: 80 },
                space: crate::OutputSpace::DisplayP3,
                long_edge: Some(1024),
            },
            folder: PathBuf::from("/out"),
            pattern: "{date}-{seq}".into(),
            clash: Clash::Skip,
        }
    }

    #[test]
    fn a_request_is_plain_json_and_a_newer_format_is_refused() {
        let request = BatchRequest::new(
            Some("exposure".into()),
            settings(),
            vec![Item {
                image: "ab".repeat(32),
                version: Some("cd".repeat(16)),
                source: "/photos/a.png".into(),
                output: "/out/a.jpg".into(),
                skip: None,
                error: None,
            }],
        );
        let json = request.to_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["format"], FORMAT);
        assert_eq!(value["settings"]["output"]["format"], "jpeg");
        assert_eq!(value["settings"]["output"]["quality"], 80);
        assert_eq!(value["settings"]["output"]["space"], "display-p3");
        assert_eq!(value["settings"]["clash"], "skip");
        assert_eq!(BatchRequest::from_json(&json).unwrap(), request);
        let newer = json.replacen("\"format\":1", "\"format\":2", 1);
        assert!(
            BatchRequest::from_json(&newer)
                .unwrap_err()
                .contains("newer")
        );
        let mut relative = request.clone();
        relative.settings.folder = "out".into();
        assert!(relative.check().is_err());
    }

    #[test]
    fn clashes_have_words() {
        for c in [Clash::Number, Clash::Overwrite, Clash::Skip] {
            assert_eq!(Clash::from_word(c.word()), Some(c));
        }
        assert_eq!(Clash::from_word("ask"), None);
    }
}
