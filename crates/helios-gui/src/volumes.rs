//! Cards and cameras mounted on the machine, for the import window
//! (`docs/design/import-window.md`): removable volumes holding `DCIM`,
//! named by their label and capacity.
//!
//! - [`Volumes`] lists them; [`system`] is the machine's. Linux reads
//!   `/proc/mounts` for what is mounted under `/run/media/$USER` and
//!   `/media`; other systems list none for now, behind the same trait.
//! - [`Watch`] polls a [`Volumes`] about once a second on a thread of its
//!   own, never on the interface's, and keeps the last list, with the
//!   cards inserted since the first one.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// A card or a camera: a mounted volume with a `DCIM` folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// Its label (the folder it is mounted on, for udisks).
    pub label: String,
    /// Where it is mounted.
    pub root: PathBuf,
    /// Its size in bytes, when the system says.
    pub capacity: Option<u64>,
}

/// Lists the cards and cameras mounted now.
pub trait Volumes: Send + Sync {
    fn cards(&self) -> Vec<Volume>;
}

/// A fixed list, as given (tests, a tool).
impl Volumes for Vec<Volume> {
    fn cards(&self) -> Vec<Volume> {
        self.clone()
    }
}

/// The machine's cards.
pub fn system() -> Arc<dyn Volumes> {
    Arc::new(System)
}

struct System;

impl Volumes for System {
    #[cfg(target_os = "linux")]
    fn cards(&self) -> Vec<Volume> {
        let Ok(mounts) = std::fs::read_to_string("/proc/mounts") else {
            return Vec::new();
        };
        let user = std::env::var("USER").unwrap_or_default();
        removable(&mounts, &user)
            .into_iter()
            .filter(|m| m.dir.join("DCIM").is_dir())
            .map(|m| Volume {
                label: label_of(&m.dir),
                capacity: capacity_of(&m.device),
                root: m.dir,
            })
            .collect()
    }

    #[cfg(not(target_os = "linux"))]
    fn cards(&self) -> Vec<Volume> {
        Vec::new()
    }
}

/// A line of `/proc/mounts`: the device and where it is mounted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub device: String,
    pub dir: PathBuf,
}

/// What `/proc/mounts` mounts where removable media go: under
/// `/run/media/<user>` and `/media` (`/media/<user>` included), one level
/// down, from a device (`/dev/…`). Spaces and the like come escaped
/// (`\040`).
pub fn removable(mounts: &str, user: &str) -> Vec<Mount> {
    let run_media = format!("/run/media/{user}/");
    let media_user = format!("/media/{user}/");
    let mut found = Vec::new();
    for line in mounts.lines() {
        let mut words = line.split_whitespace();
        let (Some(device), Some(dir)) = (words.next(), words.next()) else {
            continue;
        };
        if !device.starts_with("/dev/") {
            continue;
        }
        let dir = unescape(dir);
        let mut bases = Vec::with_capacity(3);
        if !user.is_empty() {
            bases.extend([run_media.as_str(), media_user.as_str()]);
        }
        bases.push("/media/");
        let Some(rest) = bases.into_iter().find_map(|b| dir.strip_prefix(b)) else {
            continue;
        };
        if rest.is_empty() || rest.contains('/') {
            continue;
        }
        found.push(Mount {
            device: device.into(),
            dir: PathBuf::from(dir),
        });
    }
    found
}

/// `/proc/mounts`' octal escapes (`\040` a space) undone.
fn unescape(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && let Some(Ok(n)) = s.get(i + 1..i + 4).map(|o| u8::from_str_radix(o, 8))
        {
            out.push(n);
            i += 4;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn label_of(dir: &Path) -> String {
    dir.file_name().map_or_else(
        || dir.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// A block device's size from `/sys/class/block/<name>/size` (in 512
/// byte sectors).
#[cfg(target_os = "linux")]
fn capacity_of(device: &str) -> Option<u64> {
    let name = Path::new(device).file_name()?;
    let size =
        std::fs::read_to_string(Path::new("/sys/class/block").join(name).join("size")).ok()?;
    size.trim().parse::<u64>().ok().map(|s| s * 512)
}

/// A capacity as a card says it: decimal gigabytes ("64 GB"), megabytes
/// below one.
pub fn capacity_text(bytes: u64) -> String {
    const GB: f64 = 1e9;
    let gb = bytes as f64 / GB;
    if gb >= 1.0 {
        format!("{} GB", gb.round() as u64)
    } else {
        format!("{} MB", (bytes as f64 / 1e6).round() as u64)
    }
}

#[derive(Default)]
struct Seen {
    cards: Vec<Volume>,
    /// Whether a first list came.
    listed: bool,
    /// Cards inserted since the first list, not taken yet.
    inserted: Vec<Volume>,
}

/// The cards, polled on a thread of their own (about once a second).
pub struct Watch {
    seen: Arc<Mutex<Seen>>,
    stop: Arc<AtomicBool>,
}

impl Watch {
    /// Starts polling `volumes`; `changed` is called (from the thread)
    /// when the list changes, to ask the interface for a frame.
    pub fn start(volumes: Arc<dyn Volumes>, changed: impl Fn() + Send + 'static) -> Self {
        let seen: Arc<Mutex<Seen>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (s, halt) = (seen.clone(), stop.clone());
        let spawned = std::thread::Builder::new()
            .name("helios-cards".into())
            .spawn(move || {
                while !halt.load(Ordering::Relaxed) {
                    let now = volumes.cards();
                    let moved = {
                        let mut seen = s.lock().unwrap_or_else(PoisonError::into_inner);
                        let moved = !seen.listed || seen.cards != now;
                        if seen.listed {
                            let new: Vec<Volume> = now
                                .iter()
                                .filter(|c| !seen.cards.iter().any(|o| o.root == c.root))
                                .cloned()
                                .collect();
                            seen.inserted.extend(new);
                        }
                        seen.cards = now;
                        seen.listed = true;
                        moved
                    };
                    if moved {
                        changed();
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            });
        if let Err(e) = spawned {
            log::warn!("cards: no thread to watch them: {e}");
        }
        Self { seen, stop }
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The cards mounted, as last polled.
    pub fn cards(&self) -> Vec<Volume> {
        self.seen().cards.clone()
    }

    /// Whether a first list came.
    pub fn listed(&self) -> bool {
        self.seen().listed
    }

    /// The cards inserted since the last call (not those there at start).
    pub fn take_inserted(&self) -> Vec<Volume> {
        std::mem::take(&mut self.seen().inserted)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTS: &str = "\
sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0
/dev/nvme0n1p3 / btrfs rw,relatime,ssd 0 0
/dev/nvme0n1p3 /home btrfs rw,relatime,ssd 0 0
tmpfs /run/user/1000 tmpfs rw,nosuid,nodev 0 0
/dev/sdb1 /run/media/david/EOS_DIGITAL vfat rw,nosuid,nodev,relatime 0 0
/dev/sdc1 /run/media/david/SD\\040CARD exfat rw,nosuid,nodev 0 0
/dev/sdd1 /run/media/other/THEIRS vfat rw 0 0
/dev/sde1 /media/david/NIKON vfat rw 0 0
/dev/sdf1 /media/usb0 vfat rw 0 0
/dev/sdg1 /run/media/david/DISK/inner vfat rw 0 0
gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw 0 0
";

    #[test]
    fn removable_volumes_are_read_from_proc_mounts() {
        let found = removable(MOUNTS, "david");
        let dirs: Vec<&str> = found.iter().map(|m| m.dir.to_str().unwrap()).collect();
        assert_eq!(
            dirs,
            [
                "/run/media/david/EOS_DIGITAL",
                "/run/media/david/SD CARD",
                "/media/david/NIKON",
                "/media/usb0",
            ]
        );
        assert_eq!(found[0].device, "/dev/sdb1");
        assert_eq!(label_of(&found[1].dir), "SD CARD");
    }

    #[test]
    fn a_capacity_reads_as_on_the_card() {
        assert_eq!(capacity_text(64_021_856_256), "64 GB");
        assert_eq!(capacity_text(512_000_000), "512 MB");
    }
}
