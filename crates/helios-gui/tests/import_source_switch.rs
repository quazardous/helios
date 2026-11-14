//! The import window's middle zone shows the source selected in the
//! Sources column, and only it: a source's listing that ends after
//! another was clicked never lands in the grid, and a source read
//! through before the click never stays there.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use helios_connect::folder::{self, Folder, FolderProvider};
use helios_connect::{
    Access, AccountRef, Cancel, Connector, ConnectorError, ConnectorInfo, ConnectorManifest,
    ConnectorProvider, ConnectorServices, Cursor, FetchSink, Fetched, ItemRef, Page, Settings,
    SourceRef,
};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-import-switch-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `n` small JPEGs in `folder`, named after it: each source's photos are
/// told apart by name.
fn photos(folder: &Path, n: usize) -> Vec<PathBuf> {
    std::fs::create_dir_all(folder).unwrap();
    let stem = folder.file_name().unwrap().to_string_lossy().into_owned();
    (0..n)
        .map(|i| {
            let img = image::RgbImage::from_fn(32, 24, |x, y| {
                image::Rgb([(x as usize + i) as u8, (y * 3) as u8, (i * 4) as u8])
            });
            let path = folder.join(format!("{stem}_{i:02}.JPG"));
            img.save(&path).unwrap();
            path
        })
        .collect()
}

/// A gate a listing waits at until it opens.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    turn: Condvar,
}

impl Gate {
    /// Waits, cancelled or not: the listing's result comes after the
    /// click, which is what must not land.
    fn wait(&self) {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        while !*open {
            open = self
                .turn
                .wait_timeout(open, Duration::from_millis(50))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn release(&self) {
        *self.open.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.turn.notify_all();
    }
}

/// The folder connector, whose listing of `slow` waits at a gate.
struct Gated {
    inner: Box<dyn Connector>,
    slow: SourceRef,
    gate: Arc<Gate>,
}

impl Connector for Gated {
    fn describe(&self) -> ConnectorInfo {
        self.inner.describe()
    }

    fn sources(&self, account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError> {
        self.inner.sources(account)
    }

    fn list(
        &self,
        source: &SourceRef,
        from: Option<&Cursor>,
        page: usize,
        cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        if source.id == self.slow.id {
            self.gate.wait();
        }
        self.inner.list(source, from, page, cancel)
    }

    fn access(&self, item: &ItemRef) -> Access {
        self.inner.access(item)
    }

    fn fetch(
        &self,
        item: &ItemRef,
        offset: u64,
        sink: &mut dyn FetchSink,
        cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError> {
        self.inner.fetch(item, offset, sink, cancel)
    }
}

struct GatedProvider {
    slow: PathBuf,
    gate: Arc<Gate>,
}

impl ConnectorProvider for GatedProvider {
    fn manifest(&self) -> ConnectorManifest {
        FolderProvider.manifest()
    }

    fn create(
        &self,
        settings: &Settings,
        services: &ConnectorServices,
    ) -> Result<Box<dyn Connector>, ConnectorError> {
        Ok(Box::new(Gated {
            inner: FolderProvider.create(settings, services)?,
            slow: Folder::source(&self.slow),
            gate: self.gate.clone(),
        }))
    }
}

/// The import window open on `sources` (the first one selected), the
/// listing of `slow` gated.
fn window(sources: &[&Path], slow: &Path) -> (Harness<'static, Shell>, Arc<Gate>) {
    assert_eq!(folder::ID, FolderProvider.manifest().id);
    let gate = Arc::new(Gate::default());
    let mut shell = Shell::new(helios_panels(), None);
    shell.add_import_connector(Arc::new(GatedProvider {
        slow: slow.to_path_buf(),
        gate: gate.clone(),
    }));
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h.state_mut()
        .run(
            "library.importWindow",
            json!({ "paths": sources }),
            Origin::User,
        )
        .unwrap();
    h.run_steps(3);
    (h, gate)
}

/// Steps frames until every source selected is read through, then a few
/// more for the grid to follow.
fn read_through(h: &mut Harness<'static, Shell>) {
    let deadline = Instant::now() + Duration::from_secs(20);
    h.step();
    while !h.state().import_read() {
        assert!(Instant::now() < deadline, "the sources not read in 20 s");
        std::thread::sleep(Duration::from_millis(20));
        h.step();
    }
    h.run_steps(5);
}

fn click(h: &mut Harness<'static, Shell>, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(3);
}

/// The photos the grid lists, by path, sorted.
fn listed(h: &Harness<'static, Shell>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = h
        .state()
        .import_browser()
        .map(|b| b.entries().iter().map(|e| e.path.clone()).collect())
        .unwrap_or_default();
    paths.sort();
    paths
}

/// The grid, the count and the Import button show `expected` alone.
fn shows_only(h: &Harness<'static, Shell>, expected: &[PathBuf], what: &str) {
    assert_eq!(listed(h), expected, "the grid of {what}");
    let counts = h.state().import_counts();
    assert_eq!(counts.selected, expected.len(), "the count of {what}");
    let _ = h.get_by_label(&format!("Import {}", expected.len()));
}

/// A slow source clicked away before its listing ends: the listing's end
/// never lands in the other source's grid, there or in its count.
#[test]
fn a_slow_source_s_listing_ending_after_another_is_selected_never_shows_in_its_grid() {
    let dir = temp("slow");
    let (slow, quick) = (dir.join("Slow"), dir.join("Quick"));
    let slow_photos = photos(&slow, 4);
    let quick_photos = photos(&quick, 2);
    let (mut h, gate) = window(&[&slow, &quick], &slow);
    assert_eq!(h.state().import_ticked(), std::slice::from_ref(&slow));
    // The other one selected while the first is still being listed.
    click(&mut h, "Quick");
    assert_eq!(h.state().import_ticked(), std::slice::from_ref(&quick));
    read_through(&mut h);
    shows_only(&h, &quick_photos, "Quick");
    // The slow listing ends now: the grid keeps the selected source's.
    gate.release();
    std::thread::sleep(Duration::from_millis(300));
    h.run_steps(10);
    shows_only(&h, &quick_photos, "Quick, the slow listing ended");
    // Selected again, the slow one shows its photos alone.
    click(&mut h, "Slow");
    read_through(&mut h);
    shows_only(&h, &slow_photos, "Slow");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Back and forth between two sources, each click taken in the same
/// frame the grid is drawn: the grid always shows the source selected.
#[test]
fn back_and_forth_between_sources_the_grid_shows_the_selected_one_alone() {
    let dir = temp("forth");
    let (one, two) = (dir.join("Trip"), dir.join("Garden"));
    let one_photos = photos(&one, 3);
    let two_photos = photos(&two, 2);
    // No source gated: each is read through before the next click.
    let (mut h, gate) = window(&[&one, &two], &dir.join("none"));
    gate.release();
    read_through(&mut h);
    shows_only(&h, &one_photos, "Trip");
    click(&mut h, "Garden");
    read_through(&mut h);
    shows_only(&h, &two_photos, "Garden, after Trip");
    click(&mut h, "Trip");
    read_through(&mut h);
    shows_only(&h, &one_photos, "Trip, after Garden");
    // Quickly: A, B, A, a frame apart, then read through.
    click(&mut h, "Garden");
    click(&mut h, "Trip");
    read_through(&mut h);
    shows_only(&h, &one_photos, "Trip, after a quick Garden");
    let _ = std::fs::remove_dir_all(&dir);
}
