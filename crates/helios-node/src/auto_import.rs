//! Automatic imports in `helios-node --vault`
//! (`docs/design/import-connectors.md`, section 3.10): the saved sources
//! that import on their own do so with no window, so a machine with no
//! desktop open ingests its cards and hot folders.
//!
//! - **The sources** are the person's document the node keeps,
//!   `user/import-sources` (TOML, a `[[source]]` table each), as the
//!   desktop's import window saves them; read again at each pass, so a
//!   source a client changes is followed at once.
//! - **A card** (`auto = "plugged"`) is imported when it is mounted, one
//!   there at start included, matched by its volume id (`volume`), else by
//!   the path it was saved at. Its photos are always copied.
//! - **A folder** (`auto = "new-files"`) is looked at every
//!   [`Timing::poll`]; a file is imported once it held still for
//!   [`Timing::settle`], and once while the folder is watched
//!   ([`Settling`]).
//! - **The safeguards** are the desktop's
//!   ([`helios_library::auto_import::verdict`]), from the person's
//!   settings (`import.automatic.paused`, `ask_files`, `ask_gb`) read at
//!   each pass. With no window to ask in, a batch held (paused, a first
//!   run not confirmed yet, above the limits) is left alone: said on the
//!   node's error output and on its bus ([`Held`],
//!   `import.automatic.held`). A first run is confirmed in a desktop's
//!   import window on the same vault.
//! - **The import** is the node's own ([`Importer`]): a job on its jobs,
//!   followed by the strategy's after-import backup, duplicates always
//!   skipped, with the source's own choices (else the local sources'
//!   defaults: the desktop's window choices live on the desktop). The
//!   import history records it as `auto:<source>`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Instant;

use helios_config::{Config, Layer, Schema};
use helios_core::{Envelope, Event};
use helios_library::auto_import::{self as guard, Batch, Limits, Settling, Timing, Verdict};
use helios_library::import::{Duplicates, Layout, Options, Placement};
use helios_node_core::settings::{NodeSettings, USER};
use helios_node_core::volumes::{Change, VolumeWatch};
use helios_storage::Card;
use helios_vault_sqlite::SqliteStore;
use serde::{Deserialize, Serialize};

use crate::vault_node::{Importer, SOURCE};

/// The person's document the saved sources are kept in.
pub const SOURCES: &str = "user/import-sources";

/// A batch an automatic import left alone: paused, a first run not
/// confirmed yet, or above the limits.
#[derive(Clone, Debug, Serialize)]
pub struct Held<'a> {
    /// The source's name.
    pub source: &'a str,
    /// The card's mount point or the folder.
    pub root: &'a Path,
    pub verdict: Verdict,
    pub files: u64,
    pub bytes: u64,
}

impl Event for Held<'_> {
    const KIND: &'static str = "import.automatic.held";
}

/// A saved source, as much of it as an automatic import reads.
#[derive(Clone, Debug, Default, Deserialize)]
struct Saved {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    path: Option<PathBuf>,
    /// A card's volume id.
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    label: Option<String>,
    /// `plugged`, `new-files`; absent: only when asked.
    #[serde(default)]
    auto: Option<String>,
    #[serde(default)]
    confirmed: Option<String>,
    #[serde(default)]
    choices: Option<Choices>,
}

/// The document, as the desktop keeps it.
#[derive(Default, Deserialize)]
struct Document {
    #[serde(default)]
    source: Vec<Saved>,
}

/// A source's own import choices, as the desktop saves them.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
struct Choices {
    copy_folders: bool,
    into: Option<PathBuf>,
    layout: Layout,
    rename: String,
    darktable: bool,
}

impl Default for Choices {
    /// The local sources' defaults: added where they are, by day, the
    /// original names, darktable sidecars read.
    fn default() -> Self {
        Self {
            copy_folders: false,
            into: None,
            layout: Layout::Day,
            rename: "{stem}".into(),
            darktable: true,
        }
    }
}

const PLUGGED: &str = "plugged";
const NEW_FILES: &str = "new-files";

/// The saved sources of the document's TOML `text`; none when it does not
/// read.
fn sources(text: &str) -> Vec<Saved> {
    toml::from_str::<Document>(text).map_or_else(
        |e| {
            eprintln!("helios-node: automatic imports: {SOURCES}: {e}");
            Vec::new()
        },
        |d| d.source,
    )
}

/// The source of the card `id` mounted at `root` that imports when
/// plugged in: by its volume, else by the path it was saved at.
fn card_source<'a>(list: &'a [Saved], id: &str, root: &Path) -> Option<&'a Saved> {
    list.iter()
        .filter(|s| s.auto.as_deref() == Some(PLUGGED))
        .find(|s| match &s.volume {
            Some(v) => v == id,
            None => s.path.as_deref() == Some(root),
        })
}

/// How an automatic import of `saved` runs: its choices (a card's copied),
/// duplicates skipped, recorded as `auto:<name>`.
fn options(
    saved: &Saved,
    name: &str,
    card: bool,
    cancel: &Arc<AtomicBool>,
) -> Result<Options, String> {
    let choices = saved.choices.clone().unwrap_or_default();
    let mut options = Options {
        duplicates: Duplicates::Skip,
        darktable: choices.darktable,
        cancel: Some(cancel.clone()),
        origin: Some(guard::origin(name)),
        ..Options::default()
    };
    if card || choices.copy_folders {
        options.placement = Placement::Copy { into: choices.into };
        options.layout = choices.layout;
        options.rename = Some(helios_library::rename::Pattern::parse(&choices.rename)?);
    }
    Ok(options)
}

/// What the runner reads and imports with.
struct Runner {
    importer: Importer,
    settings: NodeSettings<SqliteStore>,
    timing: Timing,
    stop: Arc<AtomicBool>,
}

impl Runner {
    /// The safeguards' settings as the person's layer has them now.
    fn limits(&self) -> Limits {
        let mut config = Config::new(Schema::core());
        match self.settings.read(USER) {
            Ok(Some(text)) => {
                config.load_str_lenient(Layer::User, &text);
            }
            Ok(None) => {}
            Err(e) => eprintln!("helios-node: automatic imports: the settings: {e}"),
        }
        Limits::new(
            config.toggle(guard::PAUSED),
            config.number(guard::ASK_FILES),
            config.number(guard::ASK_GB),
        )
    }

    fn sources(&self) -> Vec<Saved> {
        match self.settings.read(SOURCES) {
            Ok(text) => sources(&text.unwrap_or_default()),
            Err(e) => {
                eprintln!("helios-node: automatic imports: {SOURCES}: {e}");
                Vec::new()
            }
        }
    }

    /// The thread: the cards mounted and the volume events of `heard`
    /// followed; at each look, the cards plugged in and the files settled
    /// in watched folders imported, as the safeguards say.
    fn run(&self, heard: &Receiver<Envelope>, mut plugged: Vec<(String, Card)>) {
        let mut mounted: HashSet<PathBuf> = plugged.iter().map(|(_, c)| c.root.clone()).collect();
        let mut settling = Settling::default();
        let mut next_look = Instant::now();
        while !self.stop.load(Ordering::SeqCst) {
            match heard.recv_timeout(self.timing.poll.min(PACE)) {
                Ok(e) => match Change::of(&e) {
                    Some(Change::Mounted(m)) => {
                        if let Some(card) = m.card
                            && mounted.insert(card.root.clone())
                        {
                            plugged.push((m.id.0, card));
                        }
                    }
                    Some(Change::Unmounted(u)) => {
                        if let Some(card) = u.card {
                            mounted.remove(&card.root);
                            plugged.retain(|(_, c)| c.root != card.root);
                        }
                    }
                    None => {}
                },
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            let now = Instant::now();
            let look = now >= next_look;
            if plugged.is_empty() && !look {
                continue;
            }
            let (limits, list) = (self.limits(), self.sources());
            for (id, card) in std::mem::take(&mut plugged) {
                let Some(saved) = card_source(&list, &id, &card.root) else {
                    continue;
                };
                let files = helios_connect::local::files_of(&card.root).unwrap_or_default();
                self.import(saved, &card.label, &card.root, &files, &limits, true);
            }
            if look {
                next_look = now + self.timing.poll;
                // Paused, no folder is watched: what arrives meanwhile is
                // found by the first look once they run again.
                let folders: Vec<PathBuf> = list
                    .iter()
                    .filter(|s| !limits.paused && s.auto.as_deref() == Some(NEW_FILES))
                    .filter_map(|s| s.path.clone())
                    .collect();
                settling.keep(&folders);
                for folder in &folders {
                    let ready = settling.look(folder, self.timing.settle, Instant::now());
                    let Some(saved) = list.iter().find(|s| {
                        s.auto.as_deref() == Some(NEW_FILES) && s.path.as_ref() == Some(folder)
                    }) else {
                        continue;
                    };
                    let label = folder.file_name().map_or_else(
                        || folder.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    self.import(saved, &label, folder, &ready, &limits, false);
                }
            }
        }
    }

    /// What becomes of `files` new on `saved` (at `root`): imported, or
    /// held and said.
    fn import(
        &self,
        saved: &Saved,
        label: &str,
        root: &Path,
        files: &[PathBuf],
        limits: &Limits,
        card: bool,
    ) {
        if files.is_empty() {
            return;
        }
        let name = saved
            .name
            .clone()
            .or_else(|| saved.label.clone())
            .unwrap_or_else(|| label.to_owned());
        let bytes = files
            .iter()
            .filter_map(|f| std::fs::metadata(f).ok())
            .map(|m| m.len())
            .sum();
        let batch = Batch {
            confirmed: saved.confirmed.is_some(),
            files: files.len() as u64,
            bytes,
        };
        let verdict = guard::verdict(limits, &batch);
        if verdict != Verdict::Run {
            let why = match verdict {
                Verdict::Paused => "automatic imports are paused".to_owned(),
                Verdict::Confirm => {
                    "its first automatic import is not confirmed yet: confirm it in a desktop's import window".to_owned()
                }
                _ => format!(
                    "{} files ({bytes} bytes), more than an automatic import takes without asking: import them from a desktop's import window",
                    batch.files
                ),
            };
            eprintln!("helios-node: automatic import from {name}: not imported, {why}");
            self.importer.bus().publish(
                SOURCE,
                None,
                &Held {
                    source: &name,
                    root,
                    verdict,
                    files: batch.files,
                    bytes,
                },
            );
            return;
        }
        let options = match options(saved, &name, card, &self.stop) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("helios-node: automatic import from {name}: {e}");
                return;
            }
        };
        eprintln!(
            "helios-node: automatic import from {name}: {} files",
            batch.files
        );
        if let Err(e) = self.importer.import(files, &options, &mut |_| {}) {
            eprintln!("helios-node: automatic import from {name}: {e}");
        }
    }
}

/// How often the runner looks whether it must stop, at most.
const PACE: std::time::Duration = std::time::Duration::from_millis(100);

/// The automatic imports the node runs, on a thread of their own, until
/// stopped.
pub struct AutoImports {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AutoImports {
    /// Starts running the saved sources' automatic imports: the cards of
    /// `watch` (those mounted now included) and the folders watched, every
    /// `timing.poll`, imported through `importer`.
    pub fn start(
        importer: Importer,
        settings: NodeSettings<SqliteStore>,
        watch: &VolumeWatch,
        timing: Timing,
    ) -> Self {
        // Subscribed before the list is read: a card in both is taken
        // once (by its root).
        let heard = watch.subscribe();
        let plugged: Vec<(String, Card)> = watch
            .mounted()
            .iter()
            .filter_map(|v| Card::of(v).map(|c| (v.id.0.clone(), c)))
            .collect();
        let stop = Arc::new(AtomicBool::new(false));
        let runner = Runner {
            importer,
            settings,
            timing,
            stop: stop.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("helios-auto-import".into())
            .spawn(move || runner.run(&heard, plugged))
            .map_err(|e| eprintln!("helios-node: no thread for the automatic imports: {e}"))
            .ok();
        Self { stop, thread }
    }

    /// Stops them: an import under way is cancelled. Waits for it.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for AutoImports {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_import_sources_read_as_the_desktop_saves_them() {
        let text = r#"
[[source]]
connector = "card"
name = "Camera"
path = "/media/EOS_DIGITAL"
volume = "uuid:1234"
auto = "plugged"
confirmed = "2026-11-13"
browser = "grid"

[source.choices]
copy_folders = false
into = "/photos"
layout = "month"
rename = "{stem}"
duplicates = "keep"
darktable = false

[[source]]
connector = "folder"
path = "/old/card"
auto = "plugged"
"#;
        let list = sources(text);
        assert_eq!(list.len(), 2);
        let camera = card_source(&list, "uuid:1234", Path::new("/media/other")).unwrap();
        assert_eq!(camera.name.as_deref(), Some("Camera"));
        assert!(
            card_source(&list, "uuid:9999", Path::new("/media/EOS_DIGITAL")).is_none(),
            "another card mounted where it was is another source"
        );
        assert!(card_source(&list, "uuid:9999", Path::new("/old/card")).is_some());

        let stop = Arc::new(AtomicBool::new(false));
        let o = options(camera, "Camera", true, &stop).unwrap();
        assert_eq!(
            o.placement,
            Placement::Copy {
                into: Some("/photos".into())
            }
        );
        assert_eq!(o.layout, Layout::Month);
        assert_eq!(o.duplicates, Duplicates::Skip, "never kept nor asked");
        assert!(!o.darktable);
        assert_eq!(o.origin.as_deref(), Some("auto:Camera"));
        let folder = options(&Saved::default(), "hot", false, &stop).unwrap();
        assert_eq!(folder.placement, Placement::Reference, "added where it is");
        assert!(folder.darktable);
        assert!(sources("not toml [").is_empty());
    }
}
