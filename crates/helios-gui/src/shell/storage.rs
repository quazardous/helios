//! The Storage page (Library > Storage…, `library.storage`): every volume
//! the vault knows (`helios_storage::registry`), online ones first, then
//! offline ones. Each shows its name (edited in place), kind, capacity and
//! free space, roles (toggles), the off-site and RAID flags the user sets,
//! the node a fixed disk belongs to, when and where an offline one was
//! last seen, and how many photos the library holds on it. A weakly
//! identified volume offers "Mark this disk…", which writes the identity
//! file only after the user reads why and confirms. Every edit is saved
//! to the registry at once.
//!
//! The shell holds no vault: the host gives a [`StorageLink`]
//! ([`VaultStorage`] over the vault and the machine's volumes, tests
//! their own). The same link names the volumes of offline photos in the
//! Library ([`crate::library::Library::set_volumes`]).
//!
//! `library.storage` takes `{"volume": id}` to open the page on one volume
//! (the health check's free-space fix does).

use std::collections::HashMap;
use std::sync::Arc;

use egui::Context as Egui;
use egui_pop::core::icon::Icon;
use helios_core::Message;
use helios_storage::{Identity, Role, Volume, VolumeId, VolumeKind, Volumes, registry};
use helios_vault::{Store, Vault};
use serde_json::Value;

use super::Shell;

/// Opens the Storage page; `volume` (an id) puts that volume first.
pub const COMMAND: &str = "library.storage";

/// What the Storage page reads and writes.
pub trait StorageLink: Send + Sync {
    /// Every volume known: those mounted now (with their mount point and
    /// fresh free space), then the others, offline.
    fn volumes(&self) -> Result<Vec<Volume>, String>;
    /// How many copies of photos the library holds on each volume.
    fn photos(&self) -> Result<HashMap<VolumeId, usize>, String>;
    /// Saves what the user set (name, roles, off site, RAID).
    fn save(&self, volume: &Volume) -> Result<Volume, String>;
    /// Writes the identity file on a mounted volume (the user asked).
    fn mark(&self, volume: &Volume) -> Result<Volume, String>;
}

/// The link over a vault and the volumes mounted.
pub struct VaultStorage<S: Store> {
    vault: Arc<Vault<S>>,
    mounted: Arc<dyn Volumes>,
    /// What a marker names as the library that wrote it.
    library: String,
}

impl<S: Store> VaultStorage<S> {
    pub fn new(
        vault: Arc<Vault<S>>,
        mounted: Arc<dyn Volumes>,
        library: impl Into<String>,
    ) -> Self {
        Self {
            vault,
            mounted,
            library: library.into(),
        }
    }
}

/// `known` with what is mounted now: online ones first, each group by
/// name (as the registry sorts them).
pub fn merge(known: Vec<Volume>, mounted: &[Volume]) -> Vec<Volume> {
    let (mut online, offline): (Vec<Volume>, Vec<Volume>) = known
        .into_iter()
        .map(|mut v| {
            if let Some(m) = mounted.iter().find(|m| m.id == v.id) {
                v.mount.clone_from(&m.mount);
                v.free = m.free.or(v.free);
                v.capacity = m.capacity.or(v.capacity);
            }
            v
        })
        .partition(Volume::online);
    online.extend(offline);
    online
}

impl<S> StorageLink for VaultStorage<S>
where
    S: Store + Send + 'static,
{
    fn volumes(&self) -> Result<Vec<Volume>, String> {
        Ok(merge(
            registry::known(&self.vault)?,
            &self.mounted.volumes(),
        ))
    }

    fn photos(&self) -> Result<HashMap<VolumeId, usize>, String> {
        let mut counts = HashMap::new();
        for (_, location) in self.vault.all_locations()? {
            if let Some(v) = location.volume {
                *counts.entry(v).or_default() += 1;
            }
        }
        Ok(counts)
    }

    fn save(&self, volume: &Volume) -> Result<Volume, String> {
        registry::save(&self.vault, volume)
    }

    fn mark(&self, volume: &Volume) -> Result<Volume, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        registry::mark(&self.vault, volume, &self.library, now)
    }
}

/// The page's state, kept by the shell.
#[derive(Default)]
pub(crate) struct StoragePage {
    link: Option<Arc<dyn StorageLink>>,
    open: bool,
    volumes: Vec<Volume>,
    photos: HashMap<VolumeId, usize>,
    /// The names being typed, by volume.
    names: HashMap<VolumeId, String>,
    /// The volume asked for (the health check's fix): shown first.
    focus: Option<VolumeId>,
    /// The volume whose "Mark this disk" explanation is shown.
    marking: Option<VolumeId>,
    error: Option<String>,
}

/// What the page's controls asked.
enum Asked {
    Save(Volume),
    AskMark(VolumeId),
    Mark(Volume),
    CancelMark,
}

impl Shell {
    /// Reads and writes volumes through `link`: the Storage page, and the
    /// names of offline photos' volumes in the Library.
    pub fn set_storage(&mut self, link: Arc<dyn StorageLink>) {
        self.storage.link = Some(link);
        self.refresh_volumes();
    }

    /// Reads the volumes again: the page's list, and the volumes the
    /// Library names (a scan ran, a disk came or went).
    pub fn refresh_volumes(&mut self) {
        let Some(link) = self.storage.link.clone() else {
            return;
        };
        match link.volumes() {
            Ok(volumes) => {
                self.library.borrow_mut().set_volumes(&volumes);
                self.storage.volumes = volumes;
                self.storage.error = None;
            }
            Err(e) => self.storage.error = Some(e),
        }
        if self.storage.open {
            match link.photos() {
                Ok(p) => self.storage.photos = p,
                Err(e) => self.storage.error = Some(e),
            }
        }
        let known: Vec<VolumeId> = self.storage.volumes.iter().map(|v| v.id.clone()).collect();
        self.storage.names.retain(|id, _| known.contains(id));
    }

    /// Whether the Storage page is open.
    pub fn storage_page_open(&self) -> bool {
        self.storage.open
    }

    /// The volumes the Storage page lists, in its order.
    pub fn storage_volumes(&self) -> &[Volume] {
        &self.storage.volumes
    }

    /// `library.storage`: opens the page, on `volume` when given.
    pub(super) fn open_storage(&mut self, args: &Value) {
        self.storage.open = true;
        self.storage.focus = args["volume"].as_str().map(|v| VolumeId(v.to_string()));
        self.refresh_volumes();
    }

    /// After the panels: the page, when open.
    pub(super) fn storage_window(&mut self, ctx: &Egui) {
        if !self.storage.open {
            return;
        }
        let t = |m: Message| crate::i18n::text(ctx, m);
        let mut open = true;
        let mut asked = None;
        let max = ctx.content_rect().size() - egui::vec2(32.0, 32.0);
        let page = &mut self.storage;
        // The volume asked for first, then online ones, then offline.
        let mut order: Vec<usize> = (0..page.volumes.len()).collect();
        if let Some(focus) = &page.focus {
            order.sort_by_key(|&i| &page.volumes[i].id != focus);
        }
        egui::Window::new(t(Message::new("gui-storage-title", "Storage")))
            .id(egui::Id::unique("helios.storage.window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(620.0, 480.0).min(max))
            .max_size(max)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(t(Message::new(
                        "gui-storage-intro",
                        "The disks Helios has seen. Name them and say what each is for: Helios writes only to library, archive, backup and scratch disks.",
                    )))
                    .small(),
                );
                if let Some(e) = &page.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if page.volumes.is_empty() {
                        ui.weak(t(Message::new(
                            "gui-storage-none",
                            "No disk known yet: the scan runs in the background.",
                        )));
                    }
                    for i in order {
                        let volume = &page.volumes[i];
                        let count = page.photos.get(&volume.id).copied().unwrap_or(0);
                        let marking = page.marking.as_ref() == Some(&volume.id);
                        let focused = page.focus.as_ref() == Some(&volume.id);
                        let name = page
                            .names
                            .entry(volume.id.clone())
                            .or_insert_with(|| volume.name.clone());
                        volume_ui(ui, &t, volume, name, count, marking, focused, &mut asked);
                    }
                });
            });
        if !open {
            self.storage.open = false;
            self.storage.marking = None;
        }
        let Some(asked) = asked else {
            return;
        };
        let Some(link) = self.storage.link.clone() else {
            return;
        };
        let done = match asked {
            Asked::Save(v) => link.save(&v).map(|_| ()),
            Asked::AskMark(id) => {
                self.storage.marking = Some(id);
                Ok(())
            }
            Asked::CancelMark => {
                self.storage.marking = None;
                Ok(())
            }
            Asked::Mark(v) => {
                self.storage.marking = None;
                link.mark(&v).map(|marked| {
                    // Its record moved to the marker's id.
                    self.storage.names.remove(&v.id);
                    if self.storage.focus.as_ref() == Some(&v.id) {
                        self.storage.focus = Some(marked.id);
                    }
                })
            }
        };
        if let Err(e) = done {
            self.warn(format!("storage: {e}"));
        }
        self.refresh_volumes();
    }
}

/// A kind's icon and words.
fn kind_of(kind: VolumeKind) -> (Icon, Message) {
    match kind {
        VolumeKind::Internal => (
            Icon::Home,
            Message::new("gui-storage-kind-internal", "Internal disk"),
        ),
        VolumeKind::External => (
            Icon::Save,
            Message::new("gui-storage-kind-external", "External disk"),
        ),
        VolumeKind::Removable => (
            Icon::File,
            Message::new("gui-storage-kind-removable", "Card or stick"),
        ),
        VolumeKind::Share => (
            Icon::Folder,
            Message::new("gui-storage-kind-share", "Network share"),
        ),
        VolumeKind::Remote => (
            Icon::Up,
            Message::new("gui-storage-kind-remote", "Remote store"),
        ),
    }
}

/// A role's words.
fn role_text(role: Role) -> Message {
    match role {
        Role::Library => Message::new("gui-storage-role-library", "Library"),
        Role::Archive => Message::new("gui-storage-role-archive", "Archive"),
        Role::Backup => Message::new("gui-storage-role-backup", "Backup"),
        Role::Ingest => Message::new("gui-storage-role-ingest", "Ingest"),
        Role::Scratch => Message::new("gui-storage-role-scratch", "Scratch"),
    }
}

/// When a volume was last seen, as a day.
fn day_of(at: u64) -> String {
    i64::try_from(at)
        .ok()
        .and_then(crate::library::day_text)
        .unwrap_or_else(|| "?".into())
}

/// One volume.
#[allow(clippy::too_many_arguments)]
fn volume_ui(
    ui: &mut egui::Ui,
    t: &dyn Fn(Message) -> String,
    volume: &Volume,
    name: &mut String,
    photos: usize,
    marking: bool,
    focused: bool,
    asked: &mut Option<Asked>,
) {
    let frame = egui::Frame::group(ui.style());
    let frame = if focused {
        frame.stroke(egui::Stroke::new(1.5, ui.visuals().selection.stroke.color))
    } else {
        frame
    };
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        let (icon, kind) = kind_of(volume.kind);
        let kind = t(kind);
        ui.horizontal(|ui| {
            ui.add(icon.label(&kind));
            let edit = ui.add(
                egui::TextEdit::singleline(name)
                    .id(egui::Id::unique(("helios.storage.name", volume.id.as_str())))
                    .desired_width(220.0),
            );
            let edit = edit.on_hover_text(t(Message::new(
                "gui-storage-name-hint",
                "Its name; Enter saves it",
            )));
            if edit.lost_focus() && name.trim() != volume.name {
                *asked = Some(Asked::Save(Volume {
                    name: name.clone(),
                    ..volume.clone()
                }));
            }
            let state = if volume.online() {
                t(Message::new("gui-storage-online", "online"))
            } else {
                t(Message::new("gui-storage-offline", "offline"))
            };
            ui.weak(format!("{kind} · {state}"));
        });
        // Capacity and free space.
        if let Some(capacity) = volume.capacity.filter(|&c| c > 0) {
            let free = volume.free.unwrap_or(0).min(capacity);
            let used = (capacity - free) as f32 / capacity as f32;
            let text = t(Message::new("gui-storage-free", "{ $free } free of { $capacity }")
                .with("free", helios_storage::capacity_text(free))
                .with("capacity", helios_storage::capacity_text(capacity)));
            ui.add(
                egui::ProgressBar::new(used)
                    .desired_width(ui.available_width().min(360.0))
                    .text(text),
            );
        }
        // Roles.
        ui.horizontal_wrapped(|ui| {
            ui.label(t(Message::new("gui-storage-roles", "For:")));
            for role in Role::ALL {
                let mut on = volume.roles.contains(&role);
                if ui.checkbox(&mut on, t(role_text(role))).changed() {
                    let mut v = volume.clone();
                    if on {
                        v.roles.insert(role);
                    } else {
                        v.roles.remove(&role);
                    }
                    *asked = Some(Asked::Save(v));
                }
            }
        });
        // The user's flags.
        ui.horizontal_wrapped(|ui| {
            let mut off_site = volume.off_site;
            if ui
                .checkbox(&mut off_site, t(Message::new("gui-storage-off-site", "Off site")))
                .on_hover_text(t(Message::new(
                    "gui-storage-off-site-hint",
                    "Kept away from the other disks (another building)",
                )))
                .changed()
            {
                *asked = Some(Asked::Save(Volume {
                    off_site,
                    ..volume.clone()
                }));
            }
            let mut raid = volume.raid;
            if ui
                .checkbox(&mut raid, t(Message::new("gui-storage-raid", "RAID")))
                .changed()
            {
                *asked = Some(Asked::Save(Volume {
                    raid,
                    ..volume.clone()
                }));
            }
            if volume.raid {
                ui.weak(t(Message::new(
                    "gui-storage-raid-note",
                    "A RAID counts as one copy: it survives a dead disk, not a deletion or a theft.",
                )));
            }
        });
        let mut facts = Vec::new();
        if volume.kind.is_fixed()
            && let Some(node) = &volume.node
        {
            facts.push(t(
                Message::new("gui-storage-node", "In { $node }").with("node", node.clone())
            ));
        }
        if !volume.online()
            && let Some(seen) = &volume.last_seen
        {
            facts.push(t(Message::new(
                "gui-storage-last-seen",
                "Last seen { $date } on { $node }",
            )
            .with("date", day_of(seen.at))
            .with("node", seen.node.clone())));
        }
        facts.push(t(
            Message::new(
                "gui-storage-photos",
                "Photos on it: { $count }",
            )
            .with("count", photos),
        ));
        ui.label(egui::RichText::new(facts.join(" · ")).small());
        // A weak identity: the identity file, on the user's say only (a
        // card is never written).
        if volume.identified_by == Identity::Weak && volume.online() && !volume.is_card() {
            if marking {
                ui.label(t(Message::new(
                    "gui-storage-mark-explain",
                    "This disk is known only by its label and size, so Helios could take another disk for it. Marking it writes a small file, .helios/volume.json, at its root: nothing else on the disk changes.",
                )));
                ui.horizontal(|ui| {
                    if ui
                        .button(t(Message::new(
                            "gui-storage-mark-confirm",
                            "Write the identity file",
                        )))
                        .clicked()
                    {
                        *asked = Some(Asked::Mark(volume.clone()));
                    }
                    if ui
                        .button(t(Message::new("gui-storage-mark-cancel", "Cancel")))
                        .clicked()
                    {
                        *asked = Some(Asked::CancelMark);
                    }
                });
            } else if ui
                .button(t(Message::new("gui-storage-mark", "Mark this disk…")))
                .on_hover_text(t(Message::new(
                    "gui-storage-mark-hint",
                    "Known only by its label and size: an identity file tells it apart",
                )))
                .clicked()
            {
                *asked = Some(Asked::AskMark(volume.id.clone()));
            }
        }
    });
}
