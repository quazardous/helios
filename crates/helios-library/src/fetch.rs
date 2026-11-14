//! Import from a connector whose items are fetched
//! (`docs/design/import-connectors.md`, section 2.7, slice 2): a remote
//! source, a phone, anything whose [`Access`] is [`Access::Fetch`].
//! Folders and cards are read in place and go through [`crate::import`].
//!
//! - **The source is a volume.** Each account or source of a connector is
//!   a volume of kind `remote` ([`Volume::remote`]), remembered in the
//!   registry when reached; each item is a sighting on it, its place the
//!   item id, its decorative path kept beside it, offline (no file to
//!   read there).
//! - **Duplicates before download**, cheapest first, before any byte is
//!   fetched: (1) the item id's sighting already holds an image in the
//!   Library, the item unchanged (same size and date); (2) its claimed
//!   hash names a Library image: a claimed BLAKE3 is the image id itself,
//!   another algorithm is looked up in the claimed-hash table
//!   ([`Vault::claimed`]). Both are exact: under [`Duplicates::Skip`] or
//!   [`Duplicates::Ask`] the item is not fetched, under
//!   [`Duplicates::Keep`] it is. (3) With no claim, a **probable** match
//!   on capture time, camera and dimensions against Library images: with
//!   [`Remote::new_only`], an item of a source whose bytes are re-encoded
//!   ([`Originals::Rendered`]) is not fetched ("probably in your
//!   Library"); an exact-original source's item is fetched and settled by
//!   its BLAKE3.
//! - **Fetching.** The bytes land in a part file under the library
//!   folder's `.helios-part/fetch/`, named by the item, hashed while they
//!   are written in BLAKE3 (the identity) and in the claim's algorithm
//!   ([`helios_connect::land`]). A cut (offline, a transient error, an
//!   expired link, a rate limit) goes on from the bytes landed when the
//!   connector can resume, from zero otherwise; a part left by a cut run
//!   is picked up by the next. Bytes that do not match the claim or the
//!   announced size are fetched again once from zero, then the item
//!   fails; nothing half-verified becomes an image.
//! - **Always a verified copy** in the library folder (choice C3): the
//!   part takes its name there (layout, rename pattern) and is recorded as
//!   the image's location, flagged in the Library by the import's job; a
//!   claim checked is written to the claimed-hash table; the item's
//!   sighting on the remote volume is linked to the image.
//! - **Exact duplicates after download**, by BLAKE3, as [`crate::import`]
//!   does: skipped, kept or asked by [`Options::duplicates`].
//! - **A job** of kind `import`, one JSON line per item to `emit`, its
//!   report and undo as [`crate::import::import`]'s.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use helios_connect::land::{Check, Landed, Landing};
use helios_connect::{
    Access, Cancel, Caps, Connector, ConnectorError, HashAlgo, Item, ItemRef, Originals, SourceRef,
};
use helios_core::jobs::{Jobs, Progress, Start};
use helios_storage::{Volume, registry};
use helios_vault::{
    Capture, Changes, ImageId, ImageRows, Location, Presence, Sighting, Store, Vault,
};
use serde_json::{Value, json};

use crate::import::{
    Duplicates, KIND, NEXT, Options, Placement, Report, Status, facts_kept, library_root, place,
    set_library_root,
};
use crate::rename::Naming;
use crate::signals::{self, Signals};

/// How many cuts in a row an item's fetch goes on after.
const CUTS: u32 = 5;
/// The longest wait a rate limit gets before the fetch goes on.
const MAX_WAIT: Duration = Duration::from_mins(5);
/// How long a rate limit without `Retry-After` waits, doubled each time.
const BACKOFF: Duration = Duration::from_secs(1);

/// What to import from a connector's source.
pub struct Remote<'a> {
    pub connector: &'a dyn Connector,
    pub source: SourceRef,
    /// The items to import, as the source listed them.
    pub items: Vec<Item>,
    /// "New only": items probably in the Library are left out when the
    /// source re-encodes its bytes (choice C9).
    pub new_only: bool,
}

/// The volume a connector's source is: one per account or source.
pub fn volume_of(source: &SourceRef) -> Volume {
    Volume::remote(&source.connector, &source.id, &source.label)
}

/// A name for a file from an item's decorative path: its last part, with
/// nothing that could leave the folder it is put in.
fn file_name(item: &Item) -> String {
    let last = item
        .path
        .rsplit(['/', '\\'])
        .find(|p| !p.is_empty() && *p != "." && *p != "..")
        .unwrap_or("");
    let clean: String = last
        .chars()
        .map(|c| if c.is_control() || c == ':' { '_' } else { c })
        .collect();
    let clean = clean.trim_matches('.').trim();
    if clean.is_empty() {
        "item".into()
    } else {
        clean.to_string()
    }
}

/// The part file of an item: named by the item (its source, id and
/// revision), so a fetch cut in one run is resumed by the next.
fn part_of(staging: &Path, source: &SourceRef, item: &Item) -> PathBuf {
    let mut h = blake3::Hasher::new();
    for part in [
        source.connector.as_str(),
        source.id.as_str(),
        item.id.0.as_str(),
        item.revision.as_deref().unwrap_or(""),
    ] {
        h.update(part.as_bytes());
        h.update(&[0]);
    }
    let key = h.finalize().to_hex();
    staging.join(format!("{}.part", &key[..32]))
}

/// The camera model a listing says, as XMP names it.
fn camera(item: &Item) -> Option<&str> {
    item.metadata
        .get("tiff:Model")
        .map(|m| m.trim())
        .filter(|m| !m.is_empty())
}

/// The pixel size a listing says.
fn dimensions(item: &Item) -> Option<(i64, i64)> {
    let get = |keys: [&str; 2]| {
        keys.iter()
            .find_map(|k| item.metadata.get(*k)?.trim().parse::<i64>().ok())
    };
    Some((
        get(["exif:PixelXDimension", "tiff:ImageWidth"])?,
        get(["exif:PixelYDimension", "tiff:ImageLength"])?,
    ))
}

/// Whether a Library image's capture probably is the item: the same
/// capture time (to the second) and camera, and the same dimensions
/// (either way round) when both say.
fn probably(item: &Item, capture: &Capture) -> bool {
    let (Some(taken), Some(captured)) = (item.taken, capture.captured) else {
        return false;
    };
    if (captured - taken as f64).abs() >= 1.0 {
        return false;
    }
    let (Some(a), Some(b)) = (camera(item), capture.model.as_deref()) else {
        return false;
    };
    if !a.eq_ignore_ascii_case(b.trim()) {
        return false;
    }
    match (dimensions(item), capture.width.zip(capture.height)) {
        (Some((w, h)), Some((cw, ch))) => (w, h) == (cw, ch) || (w, h) == (ch, cw),
        _ => true,
    }
}

/// Why an item is known before any byte is fetched.
enum Known {
    /// Its sighting already holds the image.
    Item(ImageId),
    /// Its claimed hash names the image.
    Claim(ImageId),
}

/// Whether the item is known before any byte is fetched: its sighting
/// (`old`, at its place on the source's volume), then its claim.
fn known_before<S: Store>(
    vault: &Vault<S>,
    item: &Item,
    old: Option<&Sighting>,
) -> Result<Option<Known>, String> {
    if let Some(s) = old
        && let Some(id) = s.image
        && item.size.is_none_or(|z| z == s.size)
        && item.modified.is_none_or(|m| m.max(0) as u64 == s.modified)
        && vault.in_library(&id)?
    {
        return Ok(Some(Known::Item(id)));
    }
    let Some(claim) = &item.claim else {
        return Ok(None);
    };
    let id = match &claim.algorithm {
        // A Helios peer's claim is the image id itself.
        HashAlgo::Blake3 => ImageId::from_hex(&claim.digest.to_ascii_lowercase()).ok(),
        HashAlgo::Other(_) => None,
        algo => vault.claimed(algo.name(), &claim.digest)?,
    };
    Ok(match id {
        Some(id) if vault.in_library(&id)? => Some(Known::Claim(id)),
        _ => None,
    })
}

/// Which of `items`, listed under `source`, are already in the Library,
/// known before any byte is fetched (their item id seen before and
/// unchanged, or their claimed hash): what the import window marks "in
/// your Library". Reads the vault only: never on the interface thread.
pub fn in_library<S: Store>(
    vault: &Vault<S>,
    source: &SourceRef,
    items: &[Item],
) -> Result<Vec<bool>, String> {
    let volume = volume_of(source);
    items
        .iter()
        .map(|item| {
            let old = vault.sighting_at(
                Some(&volume.id),
                Some(&PathBuf::from(&item.id.0)),
                Path::new(&item.path),
            )?;
            Ok(known_before(vault, item, old.as_ref())?.is_some())
        })
        .collect()
}

/// The state of one import from a connector.
struct Run<'a, S: Store> {
    vault: &'a Vault<S>,
    options: &'a Options,
    remote: &'a Remote<'a>,
    caps: Caps,
    volume: Volume,
    root: PathBuf,
    /// The part files' folder: `.helios-part/fetch/` in the library folder.
    staging: PathBuf,
    now: u64,
    by: String,
    seen: HashMap<ImageId, String>,
    /// The Library images' captures, read once when a probable match is
    /// first looked for.
    captures: Option<Vec<(ImageId, Capture)>>,
    placed: u64,
    report: Report,
    stop: Arc<AtomicBool>,
    touched: Vec<(ImageId, Option<ImageRows>)>,
    log: Vec<String>,
}

impl<S: Store> Run<'_, S> {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
            || self
                .options
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::SeqCst))
    }

    fn touch(&mut self, id: ImageId) -> Result<(), String> {
        if !self.touched.iter().any(|(t, _)| *t == id) {
            let before = self.vault.rows(&id)?;
            self.touched.push((id, before));
        }
        Ok(())
    }

    fn changes(&self) -> Changes {
        let images = self
            .touched
            .iter()
            .filter_map(|(id, before)| {
                let after = self.vault.rows(id).ok()?;
                let change = helios_vault::Change {
                    image: *id,
                    before: facts_kept(before.clone(), after.as_ref()),
                    after,
                };
                change.changed().then_some(change)
            })
            .collect();
        Changes {
            images,
            ..Changes::default()
        }
    }

    /// The item's place on the source's volume.
    fn place_of(item: &Item) -> PathBuf {
        PathBuf::from(&item.id.0)
    }

    /// What was seen before at the item's place.
    fn sighting(&self, item: &Item) -> Result<Option<Sighting>, String> {
        self.vault.sighting_at(
            Some(&self.volume.id),
            Some(&Self::place_of(item)),
            Path::new(&item.path),
        )
    }

    /// Records the item as a sighting on the source's volume, linked to
    /// `image` once its bytes are known; `size` what was fetched, when it
    /// was.
    fn sight(
        &mut self,
        item: &Item,
        image: Option<ImageId>,
        size: Option<u64>,
    ) -> Result<(), String> {
        let old = self.sighting(item)?;
        let mut seen = Sighting::seen(
            PathBuf::from(&item.path),
            size.or(item.size).unwrap_or(0),
            item.modified.map_or(0, |m| m.max(0) as u64),
            self.now,
            "import",
        );
        seen.volume = Some(self.volume.id.clone());
        seen.rel_path = Some(Self::place_of(item));
        seen.taken = item.taken;
        seen.camera = camera(item).map(str::to_string);
        // No file to read there: the item is fetched through its connector.
        seen.presence = Presence::Offline;
        let new = match image {
            Some(id) => {
                let mut new = seen.listed_again(old.as_ref()).unwrap_or_else(|| {
                    let mut s = old.clone().expect("a sighting kept is there");
                    s.last_seen = self.now;
                    s
                });
                new.image = Some(id);
                new.size = size.or(item.size).unwrap_or(new.size);
                new.last_verified = Some(self.now);
                self.touch(id)?;
                new
            }
            None => match seen.listed_again(old.as_ref()) {
                Some(new) => new,
                // A changed item whose sighting holds an image: left as it
                // is until fetched.
                None => return Ok(()),
            },
        };
        self.vault.record_sighting(&new)
    }

    /// A Library image the item probably is, by capture time, camera and
    /// dimensions.
    fn probable(&mut self, item: &Item) -> Result<Option<ImageId>, String> {
        if item.taken.is_none() || camera(item).is_none() {
            return Ok(None);
        }
        if self.captures.is_none() {
            let mut ids = Vec::new();
            for image in self.vault.images()? {
                if self.vault.in_library(&image.id)? {
                    ids.push(image.id);
                }
            }
            let captures = self.vault.captures_of(&ids)?;
            self.captures = Some(
                ids.into_iter()
                    .zip(captures)
                    .filter_map(|(id, c)| Some((id, c?)))
                    .collect(),
            );
        }
        Ok(self
            .captures
            .iter()
            .flatten()
            .find(|(_, c)| probably(item, c))
            .map(|(id, _)| *id))
    }

    /// Fetches the item into its part file, verified: its BLAKE3 and its
    /// claim, its size against the announced one. A cut goes on (from the
    /// bytes landed when the connector resumes); a mismatch is tried again
    /// once from zero.
    fn fetch(&mut self, item: &Item, part: &Path, line: &mut Value) -> Result<Landed, String> {
        let item_ref = ItemRef {
            source: self.remote.source.clone(),
            id: item.id.clone(),
        };
        let cancel = Cancel::from_flag(self.stop.clone());
        let held = std::fs::metadata(part).map_or(0, |m| m.len());
        let mut offset = if self.caps.resume { held } else { 0 };
        let (mut mismatches, mut cuts, mut wait) = (0u32, 0u32, BACKOFF);
        let name = &item.path;
        loop {
            if self.stopped() {
                return Err(format!("{name}: cancelled"));
            }
            let mut landing = Landing::open(part, offset, item.claim.as_ref())
                .map_err(|e| format!("{}: {e}", part.display()))?;
            let start = landing.written();
            if start > 0 {
                line["resumed_at"] = json!(start);
            }
            let fetched = self
                .remote
                .connector
                .fetch(&item_ref, start, &mut landing, &cancel);
            let got = landing.written();
            self.report.read += got - start;
            let Err(error) = fetched else {
                let landed = landing
                    .finish()
                    .map_err(|e| format!("{}: {e}", part.display()))?;
                let wrong = match &landed.check {
                    Check::Mismatch { computed } => Some(format!(
                        "{} claimed {}, received {computed}",
                        item.claim.as_ref().map_or("", |c| c.algorithm.name()),
                        item.claim.as_ref().map_or("", |c| c.digest.as_str()),
                    )),
                    _ => item
                        .size
                        .filter(|&s| s != landed.bytes)
                        .map(|s| format!("{s} bytes announced, {} received", landed.bytes)),
                };
                let Some(wrong) = wrong else {
                    return Ok(landed);
                };
                mismatches += 1;
                if mismatches > 1 {
                    let _ = std::fs::remove_file(part);
                    return Err(format!(
                        "{name}: the bytes received are not the ones announced ({wrong})"
                    ));
                }
                self.report.retried += 1;
                line["retried"] = json!(wrong);
                self.log
                    .push(format!("{name}: the bytes received are not the ones announced ({wrong}): fetching again"));
                offset = 0;
                continue;
            };
            drop(landing);
            match error {
                ConnectorError::Cancelled => return Err(format!("{name}: cancelled")),
                ConnectorError::RateLimited { retry_after } if cuts < CUTS => {
                    cuts += 1;
                    let pause = retry_after.unwrap_or(wait).min(MAX_WAIT);
                    wait = (wait * 2).min(MAX_WAIT);
                    self.log.push(format!(
                        "{}: asked to wait {} s",
                        self.remote.source.label,
                        pause.as_secs()
                    ));
                    let until = std::time::Instant::now() + pause;
                    while std::time::Instant::now() < until && !self.stopped() {
                        std::thread::sleep(Duration::from_millis(20).min(pause));
                    }
                }
                ConnectorError::Offline
                | ConnectorError::Transient(_)
                | ConnectorError::Expired
                    if cuts < CUTS =>
                {
                    cuts += 1;
                    self.log.push(format!(
                        "{name}: cut at {got} bytes ({error}): {}",
                        if self.caps.resume {
                            "going on from there"
                        } else {
                            "fetching again"
                        }
                    ));
                }
                // The part is kept: a later run resumes it.
                e => return Err(format!("{name}: {e}")),
            }
            offset = if self.caps.resume { got } else { 0 };
        }
    }

    /// Imports one item: its line, and what happened.
    fn item(&mut self, item: &Item) -> Result<(Value, Status), String> {
        let mut line = json!({ "path": item.path, "item": item.id.0 });
        if let Access::InPlace(path) = self.remote.connector.access(&ItemRef {
            source: self.remote.source.clone(),
            id: item.id.clone(),
        }) {
            return Err(format!(
                "{}: read in place, import it as a file",
                path.display()
            ));
        }
        let old = self.sighting(item)?;
        let known = known_before(self.vault, item, old.as_ref())?;
        if let Some(known) = &known {
            let (id, of) = match known {
                Known::Item(id) => (*id, "item"),
                Known::Claim(id) => (*id, "claim"),
            };
            line["id"] = json!(id.hex());
            line["duplicate"] = json!({ "of": of, "before_fetch": true });
            if self.options.duplicates != Duplicates::Keep {
                self.report.unfetched += 1;
                self.sight(item, Some(id), None)?;
                self.seen.entry(id).or_insert_with(|| item.path.clone());
                return Ok((
                    line,
                    if self.options.duplicates == Duplicates::Ask {
                        Status::Asked
                    } else {
                        Status::Skipped
                    },
                ));
            }
        } else if item.claim.is_none()
            && self.remote.new_only
            && self.caps.originals == Originals::Rendered
            && let Some(id) = self.probable(item)?
        {
            // Fetching cannot prove anything for re-encoded bytes, and
            // costs the source's quota: shown as probably in the Library.
            line["duplicate"] =
                json!({ "of": "probable", "image": id.hex(), "before_fetch": true });
            self.report.unfetched += 1;
            self.sight(item, None, None)?;
            return Ok((line, Status::Skipped));
        }
        self.sight(item, None, None)?;
        let part = part_of(&self.staging, &self.remote.source, item);
        // A cut or cancelled fetch keeps its part for a later run.
        let landed = self.fetch(item, &part, &mut line)?;
        self.report.fetched += 1;
        let done = self.landed(item, &landed, line);
        // Placed, the part took its name; else it goes.
        let _ = std::fs::remove_file(&part);
        done
    }

    /// Takes a verified part into the Library: a duplicate by its BLAKE3
    /// handled by the policy, else placed in the library folder.
    fn landed(
        &mut self,
        item: &Item,
        landed: &Landed,
        mut line: Value,
    ) -> Result<(Value, Status), String> {
        let part = &landed.path;
        let id = ImageId(helios_core::ContentHash(landed.blake3));
        line["id"] = json!(id.hex());
        line["claim"] = json!(match landed.check {
            Check::None => "none",
            Check::Matched => "checked",
            Check::Unchecked => "unchecked",
            Check::Mismatch { .. } => unreachable!("a mismatch is not landed"),
        });
        self.touch(id)?;
        let in_library = self.vault.in_library(&id)?;
        let duplicate = if let Some(first) = self.seen.get(&id) {
            Some(json!({ "of": "source", "first": first }))
        } else if in_library {
            let places: Vec<PathBuf> = self
                .vault
                .locations(&id)?
                .into_iter()
                .filter(Location::present)
                .map(|l| l.path)
                .collect();
            Some(json!({ "of": "library", "at": places }))
        } else {
            None
        };
        self.seen.entry(id).or_insert_with(|| item.path.clone());
        if let Some(d) = duplicate {
            line["duplicate"] = d;
            if self.options.duplicates != Duplicates::Keep {
                // Its content is known now: the sighting is linked, the
                // claim recorded.
                self.claim_checked(item, landed, id)?;
                self.sight(item, Some(id), Some(landed.bytes))?;
                return Ok((
                    line,
                    if self.options.duplicates == Duplicates::Ask {
                        Status::Asked
                    } else {
                        Status::Skipped
                    },
                ));
            }
        }
        let measured = match signals::measure(part) {
            Ok(s) => Some(s),
            Err(e) => {
                line["signals_error"] = json!(e);
                None
            }
        };
        let none = Signals::default();
        let signals = measured.as_ref().unwrap_or(&none);
        let folder = self.root.join(self.options.layout.folder(signals.date()));
        let file = file_name(item);
        let (stem, ext) = match file.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s.to_string(), e.to_string()),
            _ => (file.clone(), String::new()),
        };
        let name = match &self.options.rename {
            Some(pattern) => pattern.name(&Naming {
                stem: &stem,
                signals,
                seq: self.placed + 1,
            }),
            None => stem.clone(),
        };
        let (dest, copied) = place(part, &folder, &name, &ext, &id)?;
        self.placed += 1;
        if copied {
            self.report.copied += 1;
            self.report.copies.push(dest.clone());
        }
        line["to"] = json!(dest);
        line["copied"] = json!(copied);
        let new = self
            .vault
            .import_known(&dest, id, self.now, Some(&self.by))?;
        if self.options.signals
            && let Some(s) = &measured
        {
            let doc = serde_json::to_value(s).expect("signals serialize");
            self.vault.set_signals(&id, Some(&doc))?;
        }
        if self.vault.captures_of(&[id])?[0].is_none() {
            match crate::capture::read(&dest) {
                Ok(c) => {
                    self.vault.set_captures(&[(id, c)], None)?;
                }
                Err(e) => line["capture_error"] = json!(e),
            }
        }
        self.claim_checked(item, landed, id)?;
        self.sight(item, Some(id), Some(landed.bytes))?;
        Ok((line, if new { Status::Added } else { Status::Located }))
    }

    /// Writes a claim the bytes were checked against to the claimed-hash
    /// table.
    fn claim_checked(&self, item: &Item, landed: &Landed, id: ImageId) -> Result<(), String> {
        match (&item.claim, &landed.check) {
            (Some(claim), Check::Matched) if claim.algorithm != HashAlgo::Blake3 => self
                .vault
                .set_claim(claim.algorithm.name(), &claim.digest, &id),
            _ => Ok(()),
        }
    }
}

/// The library folder a fetch copies into: always a copy (choice C3), into
/// the folder given (remembered) or the one remembered.
fn root_of<S: Store>(vault: &Vault<S>, options: &Options) -> Result<PathBuf, String> {
    match &options.placement {
        Placement::Copy { into: Some(dir) } => {
            let dir = std::path::absolute(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            set_library_root(vault, &dir)?;
            Ok(dir)
        }
        _ => library_root(vault)?.ok_or_else(|| {
            "a fetched item is copied into the library folder: give it once (`--into DIR`) and it is remembered"
                .into()
        }),
    }
}

/// Imports the items of a connector's source into `vault`, as a job
/// reported to `jobs`; one JSON line per item goes to `emit` (`path`,
/// `item`, `id`, `status`, and `duplicate`, `to`, `claim`, `retried`,
/// `resumed_at` or `error` when they apply). An item that fails is
/// reported and the others go on; the import fails only when it cannot
/// start. Whatever [`Options::placement`] says, each item becomes a
/// verified copy in the library folder. It leaves its record in the
/// import history ([`crate::import_history::record_remote`]).
pub fn import_remote<S: Store>(
    vault: &Vault<S>,
    jobs: &Jobs,
    remote: &Remote<'_>,
    options: &Options,
    now: u64,
    emit: &mut impl FnMut(&Value),
) -> Result<Report, String> {
    let ended = run_remote(vault, jobs, remote, options, now, emit);
    // As a local import's: a record that cannot be written does not undo
    // the import, it is said.
    let outcome = ended.as_ref().map_err(String::as_str);
    if let Err(e) =
        crate::import_history::record_remote(vault, &remote.source, options, now, outcome)
    {
        eprintln!("import history: {e}");
    }
    ended
}

/// [`import_remote`], before its record.
fn run_remote<S: Store>(
    vault: &Vault<S>,
    jobs: &Jobs,
    remote: &Remote<'_>,
    options: &Options,
    now: u64,
    emit: &mut impl FnMut(&Value),
) -> Result<Report, String> {
    let info = remote.connector.describe();
    let title = format!("Import from {}", remote.source.label);
    let id = format!("import-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let total = remote.items.len() as u64;
    let prepared = root_of(vault, options).and_then(|root| {
        let staging = root.join(".helios-part").join("fetch");
        std::fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
        let volume = registry::remember(vault, &volume_of(&remote.source), now)?;
        Ok((root, staging, volume))
    });
    let (root, staging, volume) = match prepared {
        Ok(p) => p,
        Err(e) => {
            jobs.start(Start::new(&id, &title, KIND)).failed(&e);
            return Err(e);
        }
    };
    let job = jobs.start(Start::new(&id, &title, KIND).total(total));
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        job.on_cancel(move || stop.store(true, Ordering::SeqCst));
    }
    let mut run = Run {
        vault,
        options,
        remote,
        caps: info.caps,
        volume,
        root,
        staging,
        now,
        by: id,
        seen: HashMap::new(),
        captures: None,
        placed: 0,
        report: Report::default(),
        stop,
        touched: Vec::new(),
        log: Vec::new(),
    };
    for (done, item) in remote.items.iter().enumerate() {
        if run.stopped() {
            break;
        }
        let name = file_name(item);
        job.progress(
            Progress::new()
                .counts(done as u64, total)
                .item(&name)
                .failed(run.report.failed)
                .skipped(run.report.skipped + run.report.asked),
        );
        let outcome = run.item(item);
        for said in run.log.drain(..) {
            job.log(said);
        }
        if run.stopped() && outcome.is_err() {
            emit(&json!({ "path": item.path, "item": item.id.0, "status": "cancelled" }));
            break;
        }
        let (mut line, status) = outcome.unwrap_or_else(|e| {
            (
                json!({ "path": item.path, "item": item.id.0, "error": e }),
                Status::Failed,
            )
        });
        line["status"] = json!(status);
        run.report.count(status);
        match status {
            Status::Failed => job.item_failed(&name, line["error"].as_str().unwrap_or("failed")),
            Status::Added | Status::Located | Status::Unchanged => {
                job.log(format!(
                    "{}: {}",
                    item.path,
                    line["status"].as_str().unwrap_or_default()
                ));
            }
            Status::Skipped | Status::Asked => {}
        }
        emit(&line);
    }
    // Parts left by cuts stay for the next run; the folder goes when empty.
    let _ = std::fs::remove_dir(&run.staging);
    if let Some(shared) = run.staging.parent() {
        let _ = std::fs::remove_dir(shared);
    }
    run.report.changes = run.changes();
    if run.stopped() && run.report.files < total {
        run.report.cancelled = true;
        job.log(format!(
            "cancelled after {} of {total} items",
            run.report.files
        ));
        job.cancelled();
        return Ok(run.report);
    }
    job.progress(
        Progress::new()
            .counts(total, total)
            .failed(run.report.failed)
            .skipped(run.report.skipped + run.report.asked),
    );
    job.log(format!(
        "{} added, {} located, {} skipped ({} before fetching), {} asked, {} failed",
        run.report.added,
        run.report.located,
        run.report.skipped,
        run.report.unfetched,
        run.report.asked,
        run.report.failed
    ));
    job.finished();
    Ok(run.report)
}
