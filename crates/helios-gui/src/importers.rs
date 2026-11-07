//! Importer plugins on the desktop (see `docs/extensions.md`,
//! "Importers"): each importer a plugin declares is an entry under
//! File > Import from (the submenu shows only when one is installed).
//! Choosing it asks for the source (`library.importWith`), then the host
//! runs [`run`]: the plugin lists what the source holds
//! (`importer.scan`, a job of its own, cancellable, its progress the
//! plugin's), then Helios imports the files through `helios_library`
//! with the values the plugin gave each (rating, reject, labels, tags,
//! title, description). The plugin never writes the library; what it
//! found, left out and warned about goes to the Log panel.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use helios_core::Message;
use helios_core::jobs::{Jobs, Progress, Start};
use helios_library::import::{self, Duplicates, Listed, Metadata, Options, Placement, Report};
use helios_plugin::manifest::SourceKind;
use helios_plugin::{Broker, Cancel};
use helios_vault::{IMAGE_EXTENSIONS, Store, Vault};
use serde_json::{Value, json};

/// The File menu's submenu of importers, and the list that fills it.
pub const MENU: egui_pop::model::ids::MenuId =
    egui_pop::model::ids::MenuId::from_static("helios.file.importFrom");
pub const LIST: &str = "helios.importers";
/// The command each entry runs (`plugin`, `importer`; `source` asked
/// when absent).
pub const COMMAND: &str = "library.importWith";
/// The context key that says whether any importer is installed.
pub const CONTEXT: &str = "heliosImporters";

/// The kind of a scan's job.
pub const SCAN_KIND: &str = "importer.scan";

static NEXT: AtomicU64 = AtomicU64::new(1);

/// One importer, as the menu and the import dialog need it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub plugin: String,
    pub importer: String,
    /// In English, from the manifest.
    pub label: String,
    pub kind: SourceKind,
    /// For a file source, the extensions its file picker offers.
    pub extensions: Vec<String>,
}

impl Entry {
    /// Its label, translated in the plugin's own catalogues
    /// (`<plugin>-importer-<importer>`, both slugged), the manifest's
    /// English otherwise.
    pub fn message(&self) -> Message {
        Message::new(&label_key(&self.plugin, &self.importer), &self.label)
    }
}

/// The key of an importer's label in its plugin's catalogues.
pub fn label_key(plugin: &str, importer: &str) -> String {
    format!(
        "{}-importer-{}",
        helios_i18n::ids::slug(plugin),
        helios_i18n::ids::slug(importer)
    )
}

/// The importers a broker offers.
pub fn entries(broker: &Broker) -> Vec<Entry> {
    broker
        .importers()
        .into_iter()
        .map(|(plugin, decl)| Entry {
            extensions: decl.extensions(),
            plugin,
            importer: decl.id,
            label: decl.label,
            kind: decl.source,
        })
        .collect()
}

/// The importers installed, shared by the menu and the shell.
pub type Shared = Rc<RefCell<Vec<Entry>>>;

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// An import's options from `library.import`'s arguments: `copy` or
/// `into` (copied into the library folder), `rename`, `layout` (`day`,
/// `month`, `year`, `flat`), `duplicates`, `darktable` (true when absent).
pub fn options(args: &Value) -> Result<Options, String> {
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(String::from);
    let mut options = Options::default();
    if args.get("copy").and_then(Value::as_bool).unwrap_or(false) || args.get("into").is_some() {
        options.placement = Placement::Copy {
            into: text("into").map(Into::into),
        };
    }
    if let Some(pattern) = text("rename") {
        options.rename = Some(helios_library::rename::Pattern::parse(&pattern)?);
    }
    if let Some(layout) = text("layout") {
        options.layout = helios_library::import::Layout::parse(&layout)?;
    }
    if let Some(policy) = text("duplicates") {
        options.duplicates = Duplicates::parse(&policy)?;
    }
    options.darktable = args
        .get("darktable")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    Ok(options)
}

/// Runs an importer as `library.import` is asked to (`importer`:
/// `plugin`, `importer`, `source`, with the import's options beside it):
/// the plugin's scan as a job of its own (cancelled from the jobs centre,
/// the plugin told to stop), one `{ "scanned": … }` line saying what it
/// found, left out and warned about, then the import of the files found
/// as `helios_library::import` runs it, each file's line to `emit`. A
/// scan cancelled is an import cancelled before its first file.
pub fn run<S: Store>(
    broker: &Broker,
    vault: &Vault<S>,
    jobs: &Jobs,
    args: &Value,
    now: u64,
    emit: &mut impl FnMut(&Value),
) -> Result<Report, String> {
    let spec = &args["importer"];
    let field = |key: &str| {
        spec.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("`importer.{key}` is missing"))
    };
    let (plugin, importer, source) = (field("plugin")?, field("importer")?, field("source")?);
    let mut options = options(args)?;
    let id = format!("importer-scan-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let job =
        std::sync::Arc::new(jobs.start(Start::new(&id, &format!("Scan {source}"), SCAN_KIND)));
    let cancel = Cancel::new();
    {
        let cancel = cancel.clone();
        job.on_cancel(move || cancel.cancel());
    }
    let progress = {
        let job = job.clone();
        move |p: &Value| {
            let mut step = Progress::new();
            if let (Some(done), Some(total)) = (p["done"].as_u64(), p["total"].as_u64()) {
                step = step.counts(done, total);
            }
            if let Some(item) = p["message"].as_str().filter(|m| !m.is_empty()) {
                step = step.item(item);
            }
            job.progress(step);
        }
    };
    let scanned = broker.importer_scan(
        plugin,
        importer,
        Path::new(source),
        &is_image,
        Some(&cancel),
        progress,
    );
    let scan = match scanned {
        Ok(scan) => scan,
        Err(_) if cancel.is_cancelled() => {
            job.cancelled();
            return Ok(Report {
                cancelled: true,
                ..Report::default()
            });
        }
        Err(e) => {
            job.failed(&e);
            return Err(e);
        }
    };
    for r in &scan.refused {
        job.log(format!("{}: left out: {}", r.path, r.why));
    }
    for w in &scan.warnings {
        job.log(w);
    }
    job.log(format!(
        "{} images found, {} left out",
        scan.images.len(),
        scan.refused.len()
    ));
    job.finished();
    emit(&json!({ "scanned": scan }));
    if scan.images.is_empty() {
        return Ok(Report::default());
    }
    let files: Vec<PathBuf> = scan.images.iter().map(|i| i.path.clone()).collect();
    options.listed = Some(Listed {
        by: format!("importer:{plugin}/{importer}"),
        files: scan
            .images
            .into_iter()
            .map(|i| {
                let metadata = Metadata {
                    rating: i.rating,
                    rejected: i.rejected,
                    labels: i.labels,
                    tags: i.tags,
                    title: i.title,
                    description: i.description,
                };
                (i.path, metadata)
            })
            .collect(),
    });
    import::import(vault, jobs, &files, &options, now, emit)
}
