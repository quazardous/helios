//! The photos chosen in a service's page (Google Photos), in the import
//! window's grid (`docs/design/google-photos-source.md`): once the person
//! pressed Done there, what they chose fills the Contents column as a
//! card's photos do, and the footer's Import downloads and imports them.
//!
//! - **Listed, not fetched.** Each item chosen is a cell under a place of
//!   the account's own (no file is behind it): its name, size and capture
//!   time as the service lists them, marked "in your Library" when the
//!   Library holds it already (its item id seen before, or its claimed
//!   hash: [`super::import_accounts::KnownRemote`]). "New only", the type
//!   filter and Select all / none / day work as for a card.
//! - **Thumbnails** are the service's small previews
//!   ([`crate::import_remote::RemoteGlances`]), read on a thread of their
//!   own within the connector's declared limits and kept in the glance
//!   cache; a connector without previews leaves the cells without one.
//! - **"Choose more…"** opens the page again: what is chosen there joins
//!   the grid (a photo chosen twice is listed once).
//! - **Import** takes the cells ticked: each visit's photos through the
//!   connector that listed them
//!   ([`super::import_accounts::ImportRemote`]), with the Choices
//!   column's choices (Into decides the folder), as a card's import runs:
//!   a job with its progress, each photo's line, a toast at the end, one
//!   undo step. Nothing is fetched before.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use egui::ViewportId;
use serde_json::{Value, json};

use super::Shell;
use super::import_accounts::{Batch, RemoteImport};
use super::import_window::ImportWindow;
use crate::import_browser::NOWHERE;
use crate::import_scan::{Found, GlanceCache, Glanced};

/// How long the previews wait when the connector's limits are met before
/// they are read on.
const RATE_WAIT: Duration = Duration::from_secs(5);

/// The photos chosen under one account, as the grid shows them.
pub(super) struct ChosenContents {
    pub(super) account: String,
    /// The album shown, its source's id, when the photos are an album's
    /// ([`super::import_albums`]) rather than chosen in the page.
    pub(super) album: Option<String>,
    /// The place their cells are under: no folder, the account's own.
    pub(super) root: PathBuf,
    /// The visits to the page taken in, in order.
    batches: Vec<Arc<Batch>>,
    /// Each cell's file: its visit and its item.
    items: HashMap<PathBuf, (usize, usize)>,
    /// The names given, lowercased: two items of one name get two cells.
    names: HashSet<String>,
    /// The previews read, as they come.
    glances: Receiver<(PathBuf, Arc<Glanced>)>,
    sender: Sender<(PathBuf, Arc<Glanced>)>,
    /// Stops the previews' reads when the grid shows something else.
    cancel: helios_connect::Cancel,
}

impl Drop for ChosenContents {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl ChosenContents {
    fn new(account: &str, album: Option<&str>) -> Self {
        let (sender, glances) = channel();
        let root = match album {
            Some(a) => PathBuf::from(format!("{NOWHERE}{account}/{a}")),
            None => PathBuf::from(format!("{NOWHERE}{account}")),
        };
        Self {
            account: account.to_owned(),
            album: album.map(str::to_owned),
            root,
            batches: Vec::new(),
            items: HashMap::new(),
            names: HashSet::new(),
            glances,
            sender,
            cancel: helios_connect::Cancel::new(),
        }
    }

    /// The cell's name of an item: the last part of its listed path,
    /// numbered when another item has it already.
    fn name_for(&mut self, item: &helios_connect::Item) -> String {
        let last = item
            .path
            .rsplit(['/', '\\'])
            .find(|p| !p.is_empty() && *p != "." && *p != "..")
            .unwrap_or("");
        let clean: String = last
            .chars()
            .map(|c| if c.is_control() || c == ':' { '_' } else { c })
            .collect();
        let clean = clean.trim();
        let base = if clean.is_empty() { "item" } else { clean };
        let (stem, ext) = match base.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s, Some(e)),
            _ => (base, None),
        };
        let mut name = base.to_owned();
        let mut n = 2;
        while !self.names.insert(name.to_lowercase()) {
            name = match ext {
                Some(e) => format!("{stem} ({n}).{e}"),
                None => format!("{stem} ({n})"),
            };
            n += 1;
        }
        name
    }

    /// The items of the cells `files`, by the visit that listed them.
    fn items_of(&self, files: &[PathBuf]) -> Vec<Vec<helios_connect::Item>> {
        let mut per = vec![Vec::new(); self.batches.len()];
        for f in files {
            if let Some(&(b, i)) = self.items.get(f) {
                per[b].push(self.batches[b].items[i].clone());
            }
        }
        per
    }

    /// Reads the previews of `batch`'s items (cells `paths`) on a thread
    /// of its own, within the connector's limits; an item without one is
    /// said to have none, so its cell stops waiting.
    fn read_glances(
        &self,
        batch: Arc<Batch>,
        paths: Vec<PathBuf>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) {
        let (tx, cancel) = (self.sender.clone(), self.cancel.clone());
        let spawned = std::thread::Builder::new()
            .name("chosen-glances".into())
            .spawn(move || {
                let glances = crate::import_remote::RemoteGlances::new(
                    GlanceCache::global().clone(),
                    batch.connector.clone(),
                );
                let at: HashMap<&helios_connect::ItemId, usize> = batch
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| (&item.id, i))
                    .collect();
                let mut seen = HashSet::new();
                loop {
                    let done = glances.read(
                        (&batch.source, &batch.items),
                        &[],
                        &cancel,
                        &Instant::now,
                        &mut |item, glance| {
                            if let Some(&i) = at.get(&item.id)
                                && seen.insert(i)
                            {
                                let _ = tx.send((paths[i].clone(), glance.clone()));
                                wake();
                            }
                        },
                    );
                    if !matches!(done.paused, Some(crate::import_remote::Pause::Rate)) {
                        break;
                    }
                    let until = Instant::now() + RATE_WAIT;
                    while Instant::now() < until && !cancel.is_cancelled() {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    if cancel.is_cancelled() {
                        return;
                    }
                }
                if cancel.is_cancelled() {
                    return;
                }
                let none = Arc::new(Glanced {
                    taken: None,
                    camera: None,
                    thumb: None,
                });
                for (i, path) in paths.iter().enumerate() {
                    if !seen.contains(&i) {
                        let _ = tx.send((path.clone(), none.clone()));
                    }
                }
                wake();
            });
        if let Err(e) = spawned {
            log::warn!("chosen photos' previews: {e}");
        }
    }
}

/// An item's capture time, as the grid's cells keep it (the camera's
/// local time, kept as if it were UTC).
fn taken_of(item: &helios_connect::Item) -> Option<chrono::NaiveDateTime> {
    item.taken
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|t| t.naive_utc())
}

impl ImportWindow {
    /// Before the window is drawn: whether the Contents column shows the
    /// photos chosen under the account selected, taken in as they come
    /// (their cells, then their previews). Otherwise the grid goes back
    /// to the sources ticked, read again.
    pub(super) fn follow_chosen(&mut self) -> bool {
        // An album selected: its photos once listed.
        let shown = if let Some((account, source, photos)) = self.album_shown() {
            let batches = match photos {
                super::import_albums::Photos::Listed(b) => vec![b],
                _ => Vec::new(),
            };
            Some((account, Some(source.id), batches))
        } else {
            self.sources
                .account
                .clone()
                .filter(|id| self.accounts.shows_chosen(id))
                .map(|account| {
                    let batches = self.accounts.chosen(&account);
                    (account, None, batches)
                })
        };
        let Some((account, album, batches)) = shown else {
            if self.chosen.is_some() {
                self.clear_contents();
            }
            return false;
        };
        if self
            .chosen
            .as_ref()
            .is_none_or(|c| c.account != account || c.album != album)
        {
            self.clear_contents();
            self.chosen = Some(ChosenContents::new(&account, album.as_deref()));
        }
        let thumbs = self.thumbnails();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(self.waker());
        let Some(chosen) = self.chosen.as_mut() else {
            return false;
        };
        for batch in batches.iter().skip(chosen.batches.len()) {
            let b = chosen.batches.len();
            let mut paths = Vec::with_capacity(batch.items.len());
            for (i, item) in batch.items.iter().enumerate() {
                let name = chosen.name_for(item);
                let path = chosen.root.join(name);
                chosen.items.insert(path.clone(), (b, i));
                self.contents.add(Found {
                    source: chosen.root.clone(),
                    path: path.clone(),
                    bytes: item.size.unwrap_or(0),
                    modified: item.modified,
                    known: batch.known.get(i).copied().unwrap_or(false),
                    offline: false,
                    key: None,
                });
                let camera = item.metadata.get("tiff:Model").cloned();
                self.contents.glance(&path, taken_of(item), camera, None);
                paths.push(path);
            }
            chosen.batches.push(batch.clone());
            chosen.read_glances(batch.clone(), paths, wake.clone());
        }
        let came: Vec<_> = chosen.glances.try_iter().collect();
        for (path, g) in came {
            super::import_window::take_glance(
                &mut self.contents,
                &thumbs,
                &path,
                g.taken,
                g.camera.clone(),
                g.thumb.clone(),
            );
        }
        true
    }

    /// The place the grid shows the chosen photos under, when it does.
    pub(super) fn chosen_root(&self) -> Option<&Path> {
        self.chosen.as_ref().map(|c| c.root.as_path())
    }

    /// Whether the grid shows the photos chosen under `account`.
    pub(super) fn shows_chosen_of(&self, account: &str) -> bool {
        self.chosen
            .as_ref()
            .is_some_and(|c| c.account == account && c.album.is_none())
    }

    /// Whether the grid shows the photos of the album selected.
    pub(super) fn shows_album(&self) -> bool {
        self.chosen.as_ref().is_some_and(|c| c.album.is_some())
    }
}

/// The reports of several imports as one: their counts added up.
fn merged(reports: Vec<Value>) -> Value {
    let mut all = reports.into_iter();
    let Some(mut first) = all.next() else {
        return json!({});
    };
    for report in all {
        let (Some(into), Value::Object(from)) = (first.as_object_mut(), report) else {
            continue;
        };
        for (key, value) in from {
            match (into.get_mut(&key), value) {
                (Some(Value::Number(a)), Value::Number(b)) => {
                    if let (Some(x), Some(y)) = (a.as_u64(), b.as_u64()) {
                        *a = (x + y).into();
                    }
                }
                (Some(Value::Array(a)), Value::Array(b)) => a.extend(b),
                (Some(Value::Bool(a)), Value::Bool(b)) => *a |= b,
                (None, v) => {
                    into.insert(key, v);
                }
                _ => {}
            }
        }
    }
    first
}

impl Shell {
    /// The footer's Import while the grid shows photos chosen in a
    /// service's page: the ones ticked downloaded into the source's
    /// `download` folder (else the Detail column's Into) and imported with
    /// the Detail column's choices, off the interface's thread, as a card's
    /// import runs (its lines and its end reach the shell as a card's do:
    /// the toast, the Log panel, the photos selected); the pickings
    /// ended, the window closed. Nothing is fetched before.
    pub(super) fn import_chosen(&mut self) {
        let sink = self.import_sink();
        let (hook, jobs, args, ctx, album) = {
            let mut w = self.import.get_mut();
            let Some((root, account, album)) = w
                .chosen
                .as_ref()
                .map(|c| (c.root.clone(), c.account.clone(), c.album.is_some()))
            else {
                return;
            };
            let Some(hook) = w.accounts.import.clone() else {
                drop(w);
                self.warn("no library to import the chosen photos into".to_owned());
                return;
            };
            let files = w.contents.selected_files(&root);
            let Some(chosen) = w.chosen.as_ref() else {
                return;
            };
            let per = chosen.items_of(&files);
            let listed = chosen.batches.clone();
            let mut args = w.choices.args(&[], true);
            if let Some(o) = args.as_object_mut() {
                o.remove("paths");
                // A connector with a `download` folder: the photos go
                // there as they are named, never asked.
                if let Some(dir) = w.download_folder(&account) {
                    o.insert("into".into(), json!(dir));
                    o.insert("layout".into(), json!("flat"));
                    o.remove("rename");
                }
            }
            // Each visit's photos with the connector that listed them; a
            // visit taken in since this pass imports none.
            // An album's photos as listed; else the pages' chosen.
            let batches = if album {
                listed.clone()
            } else {
                w.accounts.take_chosen(&account)
            };
            let jobs: Vec<(Arc<Batch>, Vec<helios_connect::Item>)> = batches
                .into_iter()
                .map(|batch| {
                    let items = listed
                        .iter()
                        .position(|b| Arc::ptr_eq(b, &batch))
                        .map(|b| per[b].clone())
                        .unwrap_or_default();
                    (batch, items)
                })
                .collect();
            (hook, jobs, args, w.ctx.clone(), album)
        };
        if self.core.activity.has_node() {
            self.hand_chosen(jobs, &args, album);
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("import-chosen".into())
            .spawn(move || {
                let mut reports = Vec::new();
                let mut failed = None;
                for (batch, items) in jobs {
                    if !items.is_empty() && failed.is_none() {
                        let job = RemoteImport {
                            connector: batch.connector.clone(),
                            source: batch.source.clone(),
                            items,
                            args: args.clone(),
                        };
                        let mut emit = |line: &Value| {
                            let _ = sink.send((args.clone(), line.clone()));
                        };
                        match hook(job, &mut emit) {
                            Ok(report) => reports.push(report),
                            Err(e) => failed = Some(e),
                        }
                    }
                    // Imported or given up: the service forgets the page
                    // (an album has none).
                    if album {
                        continue;
                    }
                    if let Err(e) = batch.connector.end_pick() {
                        log::debug!("import chosen: end of a picking: {e:?}");
                    }
                }
                let end = match failed {
                    Some(e) => json!({ "failed": e }),
                    None => json!({ "ended": merged(reports) }),
                };
                let _ = sink.send((args, end));
                if let Some(ctx) = ctx.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                    ctx.request_repaint_of(ViewportId::ROOT);
                }
            });
        if let Err(e) = spawned {
            self.warn(format!("import: {e}"));
            return;
        }
        let mut w = self.import.get_mut();
        if album {
            w.albums.close();
        }
        w.close_after_import();
    }

    /// [`Self::import_chosen`] on the desktop's node: each visit's photos
    /// handed to it (`import_promise::hand`) and imported as its promise
    /// (`library.import {remote}`), answered at once. The downloads run in
    /// the node's job, each photo's progress and outcome on the promise's
    /// line; the window stays, showing it, and may close while it runs.
    fn hand_chosen(
        &mut self,
        jobs: Vec<(Arc<Batch>, Vec<helios_connect::Item>)>,
        args: &Value,
        album: bool,
    ) {
        let mut params = args.clone();
        if let Some(fields) = params.as_object_mut() {
            for key in ["paths", "path", "origin"] {
                fields.remove(key);
            }
            let darktable = fields.get("darktable") != Some(&json!(false));
            fields.insert("darktable".into(), json!(darktable));
        }
        for (batch, items) in jobs {
            if items.is_empty() {
                // Given up: the service forgets the page.
                if !album {
                    super::import_accounts::end_picks(vec![batch]);
                }
                continue;
            }
            let key =
                helios_node_core::import_promise::hand(helios_node_core::import_promise::Handed {
                    connector: batch.connector.clone(),
                    source: batch.source.clone(),
                    items,
                    end_pick: !album,
                });
            let mut asked = params.clone();
            asked["remote"] = json!(key);
            asked["rule"] = json!("all");
            if let Err(e) = self.core.activity.start(asked) {
                self.warn(e);
            }
        }
        let mut w = self.import.get_mut();
        if album {
            w.albums.close();
        }
        w.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str) -> helios_connect::Item {
        helios_connect::Item {
            id: helios_connect::ItemId(path.into()),
            path: path.into(),
            size: None,
            modified: None,
            taken: None,
            kind: helios_connect::ItemKind::Photo,
            mime: None,
            metadata: std::collections::BTreeMap::new(),
            claim: None,
            revision: None,
            group: None,
        }
    }

    #[test]
    fn two_items_of_one_name_get_a_cell_each() {
        let mut c = ChosenContents::new("acc", None);
        assert_eq!(c.name_for(&item("IMG_1.JPG")), "IMG_1.JPG");
        assert_eq!(c.name_for(&item("a/img_1.jpg")), "img_1 (2).jpg");
        assert_eq!(c.name_for(&item("IMG_1.JPG")), "IMG_1 (3).JPG");
        assert_eq!(c.name_for(&item("")), "item");
        assert_eq!(c.name_for(&item("x:y")), "x_y");
    }

    #[test]
    fn several_imports_report_as_one() {
        let one = json!({ "added": 2, "files": 2, "cancelled": false, "x": [1] });
        let two = json!({ "added": 1, "files": 3, "cancelled": true, "x": [2] });
        let all = merged(vec![one, two]);
        assert_eq!(all["added"], 3);
        assert_eq!(all["files"], 5);
        assert_eq!(all["cancelled"], true);
        assert_eq!(all["x"], json!([1, 2]));
    }
}
