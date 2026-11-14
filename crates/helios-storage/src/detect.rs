//! What is mounted, read from the files Linux keeps about it:
//!
//! - `/proc/mounts`: every mounted filesystem, its device and type;
//!   pseudo filesystems (proc, tmpfs, cgroup, snaps' loop devices…) are
//!   skipped, and a filesystem mounted twice (btrfs subvolumes, binds) is
//!   one volume, at its first mount point;
//! - `/dev/disk/by-uuid` and `/dev/disk/by-label`: the filesystem's UUID
//!   and label (else the mount folder's name);
//! - `/sys/block`: the disk behind a partition (a device mapper's
//!   through its `slaves`), whether it is removable, whether it hangs on
//!   USB (an external disk); `/dev/disk/by-id` names that disk, so two
//!   partitions of one drive are known as the same drive;
//! - network shares from the filesystem type (`cifs`, `nfs`,
//!   `fuse.sshfs`) and its source, and gvfs's mounts (`smb-share:…`
//!   under its `fuse.gvfsd-fuse` folder);
//! - free space and size through `statvfs`.
//!
//! Every root is injected ([`Roots`]), so tests read sample trees in a
//! temporary folder, never the machine's.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::{Identity, Volume, VolumeId, VolumeKind, marker};

/// Where the system's files are read from.
#[derive(Clone, Debug)]
pub struct Roots {
    /// The mount table (`/proc/mounts`).
    pub mounts: PathBuf,
    /// The device folder's root: `/dev/sdb1` is read at `dev` +
    /// `dev/sdb1`, and `/dev/disk/by-uuid` at `dev` + `dev/disk/by-uuid`.
    pub dev: PathBuf,
    /// sysfs (`/sys`).
    pub sys: PathBuf,
    /// The user, whose removable media mount under `/run/media/<user>`.
    pub user: String,
}

impl Roots {
    /// The machine's own.
    pub fn system() -> Self {
        Self {
            mounts: "/proc/mounts".into(),
            dev: "/".into(),
            sys: "/sys".into(),
            user: std::env::var("USER").unwrap_or_default(),
        }
    }

    fn disk_links(&self, kind: &str) -> PathBuf {
        self.dev.join("dev/disk").join(kind)
    }
}

/// A line of `/proc/mounts`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub device: String,
    pub dir: PathBuf,
    pub fstype: String,
}

/// The lines of a mount table, octal escapes (`\040`) undone.
pub fn parse_mounts(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let (device, dir, fstype) = (words.next()?, words.next()?, words.next()?);
            Some(Mount {
                device: unescape(device),
                dir: PathBuf::from(unescape(dir)),
                fstype: fstype.to_string(),
            })
        })
        .collect()
}

/// Filesystems that hold no user files.
fn pseudo(fstype: &str) -> bool {
    matches!(
        fstype,
        "proc"
            | "sysfs"
            | "tmpfs"
            | "devtmpfs"
            | "devpts"
            | "cgroup"
            | "cgroup2"
            | "securityfs"
            | "pstore"
            | "bpf"
            | "debugfs"
            | "tracefs"
            | "configfs"
            | "fusectl"
            | "mqueue"
            | "hugetlbfs"
            | "autofs"
            | "binfmt_misc"
            | "rpc_pipefs"
            | "nsfs"
            | "efivarfs"
            | "selinuxfs"
            | "ramfs"
            | "overlay"
            | "squashfs"
            | "zram"
            | "swap"
            | "fuse.portal"
            | "fuse.gvfsd-fuse"
    )
}

/// Folders that never hold photos: boot partitions, snaps, systemd's.
fn system_dir(dir: &Path) -> bool {
    [
        "/boot",
        "/efi",
        "/snap",
        "/var/lib/snapd",
        "/run/credentials",
    ]
    .iter()
    .any(|p| dir.starts_with(p))
}

/// A network share's address from its filesystem type and source
/// (`//nas/photos` for cifs, `nas:/volume1/photos` for nfs,
/// `user@host:/path` for sshfs).
pub fn share_address(fstype: &str, source: &str) -> Option<String> {
    match fstype {
        "cifs" | "smb3" | "smbfs" => {
            let rest = source.trim_start_matches(['/', '\\']).replace('\\', "/");
            let (host, share) = rest.split_once('/')?;
            Some(format!("smb://{}/{share}", host.to_lowercase()))
        }
        "nfs" | "nfs4" => {
            let (host, path) = source.split_once(':')?;
            Some(format!("nfs://{}:{path}", host.to_lowercase()))
        }
        "fuse.sshfs" => {
            let (who, path) = source.split_once(':')?;
            let host = who.rsplit('@').next().unwrap_or(who).to_lowercase();
            let path = if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/~/{path}")
            };
            Some(format!("sftp://{host}{path}"))
        }
        _ => None,
    }
}

/// A gvfs mount's address from its folder name under the
/// `fuse.gvfsd-fuse` mount (`smb-share:server=nas,share=photos`).
pub fn gvfs_address(name: &str) -> Option<String> {
    let (scheme, params) = name.split_once(':')?;
    let params: BTreeMap<&str, String> = params
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k, percent_decode(v)))
        .collect();
    let host = || {
        params
            .get("server")
            .or(params.get("host"))
            .map(|h| h.to_lowercase())
    };
    match scheme {
        "smb-share" => Some(format!("smb://{}/{}", host()?, params.get("share")?)),
        "sftp" => Some(format!(
            "sftp://{}{}",
            host()?,
            params.get("prefix").map_or("", String::as_str)
        )),
        "nfs" => Some(format!(
            "nfs://{}:{}",
            host()?,
            params.get("prefix").map_or("/", String::as_str)
        )),
        "afp-volume" => Some(format!("afp://{}/{}", host()?, params.get("volume")?)),
        "dav" | "davs" => Some(format!(
            "{scheme}://{}{}",
            host()?,
            params.get("prefix").map_or("", String::as_str)
        )),
        _ => None,
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(Ok(n)) = s.get(i + 1..i + 3).map(|h| u8::from_str_radix(h, 16))
        {
            out.push(n);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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

/// udev's `\x20` escapes in `/dev/disk/by-label` names undone.
fn unescape_udev(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && bytes.get(i + 1) == Some(&b'x')
            && let Some(Ok(n)) = s.get(i + 2..i + 4).map(|h| u8::from_str_radix(h, 16))
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

/// A `/dev/disk/by-*` folder read: the device each link points to (its
/// file name, `sdb1`), with the link's name.
pub fn disk_links(dir: &Path) -> BTreeMap<String, Vec<String>> {
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let Ok(target) = std::fs::read_link(entry.path()) else {
            continue;
        };
        let Some(device) = target.file_name() else {
            continue;
        };
        found
            .entry(device.to_string_lossy().into_owned())
            .or_default()
            .push(entry.file_name().to_string_lossy().into_owned());
    }
    for names in found.values_mut() {
        names.sort();
    }
    found
}

/// A device's kernel name (`sdb1`, `dm-0`): its path resolved under the
/// device root (`/dev/mapper/luks-…` is a link to `../dm-0`), else its
/// file name.
fn device_name(roots: &Roots, device: &str) -> String {
    let path = roots.dev.join(device.trim_start_matches('/'));
    std::fs::canonicalize(&path)
        .ok()
        .as_deref()
        .and_then(Path::file_name)
        .or_else(|| Path::new(device).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The whole disk behind a block device: itself if it is one, the disk
/// holding it if it is a partition (`/sys/block/sdb/sdb1`), and the disk
/// under a device mapper (`/sys/block/dm-0/slaves/sda2`).
pub fn disk_of(sys: &Path, name: &str) -> Option<String> {
    disk_of_depth(sys, name, 4)
}

fn disk_of_depth(sys: &Path, name: &str, depth: u8) -> Option<String> {
    let block = sys.join("block");
    if block.join(name).exists() {
        if name.starts_with("dm-") && depth > 0 {
            let slave = std::fs::read_dir(block.join(name).join("slaves"))
                .ok()?
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .min()?;
            return disk_of_depth(sys, &slave, depth - 1);
        }
        return Some(name.to_string());
    }
    std::fs::read_dir(&block)
        .ok()?
        .flatten()
        .find(|disk| disk.path().join(name).is_dir())
        .map(|disk| disk.file_name().to_string_lossy().into_owned())
}

fn read_trim(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Free bytes of the filesystem holding `dir`, `None` where it cannot be
/// read.
pub fn free_space(dir: &Path) -> Option<u64> {
    space(dir).map(|(free, _)| free)
}

/// Free and total bytes of the filesystem holding `dir`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn space(dir: &Path) -> Option<(u64, u64)> {
    let s = rustix::fs::statvfs(dir).ok()?;
    let unit = if s.f_frsize > 0 {
        s.f_frsize
    } else {
        s.f_bsize
    };
    Some((s.f_bavail * unit, s.f_blocks * unit))
}

#[cfg(windows)]
pub(crate) fn space(dir: &Path) -> Option<(u64, u64)> {
    crate::windows::space(dir)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(crate) fn space(_dir: &Path) -> Option<(u64, u64)> {
    None
}

/// The folder a volume is mounted on, as a name.
pub(crate) fn label_of(dir: &Path) -> String {
    dir.file_name().map_or_else(
        || dir.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// A volume known only by its label and size.
pub fn weak(label: &str, capacity: Option<u64>) -> Volume {
    Volume {
        id: VolumeId(format!("weak:{label}:{}", capacity.unwrap_or(0))),
        identified_by: Identity::Weak,
        name: label.to_string(),
        label: Some(label.to_string()),
        kind: VolumeKind::Removable,
        roles: std::collections::BTreeSet::new(),
        drive: None,
        capacity,
        free: None,
        mount: None,
        node: None,
        last_seen: None,
        has_dcim: false,
        off_site: false,
        raid: false,
    }
}

/// Whether `dir` is where the desktop mounts removable media:
/// `/run/media/<user>/X`, `/media/<user>/X` or `/media/X`.
fn media_dir(dir: &Path, user: &str) -> bool {
    let one_down = |base: &Path| {
        dir.strip_prefix(base)
            .is_ok_and(|rest| rest.components().count() == 1)
    };
    (!user.is_empty()
        && (one_down(&Path::new("/run/media").join(user))
            || one_down(&Path::new("/media").join(user))))
        || one_down(Path::new("/media"))
}

/// Applies the strongest identity: the marker at the volume's root, if
/// there is one, wins over what the filesystem says.
pub(crate) fn with_marker(mut volume: Volume, root: &Path) -> Volume {
    if let Some(m) = marker::read(root) {
        volume.id = m.id;
        volume.identified_by = Identity::Marker;
    }
    volume
}

/// The volumes mounted now, as `roots` say. `node` is the machine
/// scanning: fixed disks belong to it.
pub fn scan(roots: &Roots, node: &str) -> Vec<Volume> {
    let Ok(table) = std::fs::read_to_string(&roots.mounts) else {
        return Vec::new();
    };
    let uuids = disk_links(&roots.disk_links("by-uuid"));
    let labels = disk_links(&roots.disk_links("by-label"));
    let ids = disk_links(&roots.disk_links("by-id"));
    let mut devices = HashSet::new();
    let mut shares = HashSet::new();
    let mut found = Vec::new();
    for m in parse_mounts(&table) {
        if m.fstype == "fuse.gvfsd-fuse" {
            found.extend(gvfs_shares(&m.dir, &mut shares));
            continue;
        }
        if let Some(address) = share_address(&m.fstype, &m.device) {
            if shares.insert(address.clone()) {
                found.push(share(&address, &m.dir));
            }
            continue;
        }
        if pseudo(&m.fstype)
            || !m.device.starts_with("/dev/")
            || m.device.starts_with("/dev/loop")
            || system_dir(&m.dir)
        {
            continue;
        }
        let name = device_name(roots, &m.device);
        if name.is_empty() || !devices.insert(name.clone()) {
            continue;
        }
        found.push(block_volume(roots, node, &m, &name, &uuids, &labels, &ids));
    }
    found
}

fn block_volume(
    roots: &Roots,
    node: &str,
    m: &Mount,
    name: &str,
    uuids: &BTreeMap<String, Vec<String>>,
    labels: &BTreeMap<String, Vec<String>>,
    ids: &BTreeMap<String, Vec<String>>,
) -> Volume {
    let disk = disk_of(&roots.sys, name);
    let removable = name.starts_with("mmcblk")
        || disk.as_deref().is_some_and(|d| {
            read_trim(&roots.sys.join("block").join(d).join("removable")).as_deref() == Some("1")
        });
    let usb = disk.as_deref().is_some_and(|d| {
        std::fs::read_link(roots.sys.join("block").join(d))
            .is_ok_and(|t| t.to_string_lossy().contains("/usb"))
    });
    let kind = if removable {
        VolumeKind::Removable
    } else if usb || media_dir(&m.dir, &roots.user) {
        VolumeKind::External
    } else {
        VolumeKind::Internal
    };
    // The drive: its stable name in by-id (model and serial), the
    // kernel's name failing that.
    let drive = disk.map(|d| {
        ids.get(&d)
            .and_then(|names| {
                names
                    .iter()
                    .find(|n| !n.starts_with("wwn-") && !n.starts_with("nvme-eui"))
                    .or(names.first())
                    .cloned()
            })
            .unwrap_or(d)
    });
    let label = labels
        .get(name)
        .and_then(|l| l.first())
        .map_or_else(|| label_of(&m.dir), |l| unescape_udev(l));
    // The size a card says is its partition's; free space is the
    // filesystem's.
    let space = space(&m.dir);
    let free = space.map(|(free, _)| free);
    let capacity = sysfs_size(&roots.sys, disk_name(&roots.sys, name).as_deref(), name)
        .or(space.map(|(_, total)| total));
    let mut volume = match uuids.get(name).and_then(|u| u.first()) {
        Some(uuid) => Volume {
            id: VolumeId(format!("uuid:{}", uuid.to_lowercase())),
            identified_by: Identity::Uuid,
            ..weak(&label, capacity)
        },
        None => weak(&label, capacity),
    };
    volume.kind = kind;
    volume.drive = drive;
    volume.free = free;
    volume.mount = Some(m.dir.clone());
    volume.node = kind.is_fixed().then(|| node.to_string());
    volume.has_dcim = m.dir.join("DCIM").is_dir();
    volume.roles = volume.default_roles();
    with_marker(volume, &m.dir)
}

/// The disk holding a partition, for its size file.
fn disk_name(sys: &Path, name: &str) -> Option<String> {
    if sys.join("block").join(name).exists() {
        return None;
    }
    disk_of(sys, name)
}

/// A block device's size from sysfs (in 512 byte sectors).
fn sysfs_size(sys: &Path, disk: Option<&str>, name: &str) -> Option<u64> {
    let block = sys.join("block");
    let dir = match disk {
        Some(d) => block.join(d).join(name),
        None => block.join(name),
    };
    read_trim(&dir.join("size"))?
        .parse::<u64>()
        .ok()
        .map(|s| s * 512)
}

fn share(address: &str, dir: &Path) -> Volume {
    let label = address
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .filter(|l| !l.is_empty())
        .unwrap_or(address)
        .to_string();
    let (free, capacity) = space(dir).map_or((None, None), |(f, t)| (Some(f), Some(t)));
    let volume = Volume {
        id: VolumeId(format!("share:{address}")),
        identified_by: Identity::Share,
        kind: VolumeKind::Share,
        free,
        mount: Some(dir.to_path_buf()),
        ..weak(&label, capacity)
    };
    with_marker(volume, dir)
}

/// The shares gvfs mounts, one folder each under its own mount.
fn gvfs_shares(dir: &Path, seen: &mut HashSet<String>) -> Vec<Volume> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| Some((gvfs_address(&e.file_name().to_string_lossy())?, e.path())))
        .collect();
    found.sort();
    found
        .into_iter()
        .filter(|(address, _)| seen.insert(address.clone()))
        .map(|(address, path)| share(&address, &path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Card, Role, Volumes};

    const MOUNTS: &str = "\
sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0
proc /proc proc rw 0 0
/dev/nvme0n1p3 / btrfs rw,relatime,ssd 0 0
/dev/nvme0n1p3 /home btrfs rw,relatime,ssd 0 0
/dev/nvme0n1p1 /boot/efi vfat rw 0 0
/dev/loop3 /var/lib/snapd/snap/core/1 squashfs ro 0 0
tmpfs /run/user/1000 tmpfs rw,nosuid,nodev 0 0
/dev/sdb1 /run/media/david/EOS_DIGITAL vfat rw,nosuid,nodev,relatime 0 0
/dev/sdc1 /run/media/david/SD\\040CARD exfat rw,nosuid,nodev 0 0
/dev/sdd1 /run/media/david/Backup-1 ext4 rw 0 0
/dev/sdd2 /run/media/david/Backup-2 ext4 rw 0 0
//NAS/photos /mnt/nas cifs rw 0 0
nas:/volume1/raw /mnt/raw nfs4 rw 0 0
me@box:/srv/pics /home/david/box fuse.sshfs rw 0 0
gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw 0 0
";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "helios-storage-detect-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    fn link(target: &str, at: &Path) {
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, at).unwrap();
    }

    /// A machine: `/dev/disk` links and `/sys/block`, with mount points
    /// moved under the scratch folder (a mount table names them there).
    #[cfg(unix)]
    fn machine(dir: &Path) -> Roots {
        let dev = dir.join("dev");
        let sys = dir.join("sys");
        let by = |kind: &str, name: &str, target: &str| {
            link(
                &format!("../../{target}"),
                &dev.join("dev/disk").join(kind).join(name),
            );
        };
        by("by-uuid", "0B1F-2C3D", "sdb1");
        by("by-uuid", "aaaa-1111", "sdd1");
        by("by-uuid", "BBBB-2222", "sdd2");
        by("by-uuid", "f00d-cafe", "nvme0n1p3");
        by("by-label", "SD\\x20CARD", "sdc1");
        by("by-label", "fedora", "nvme0n1p3");
        by("by-id", "usb-WD_Elements_1234-0:0", "sdd");
        by("by-id", "wwn-0x50014ee", "sdd");
        for (disk, parts, removable, usb) in [
            ("nvme0n1", &["nvme0n1p1", "nvme0n1p3"][..], "0", false),
            ("sdb", &["sdb1"], "1", true),
            ("sdc", &["sdc1"], "1", true),
            ("sdd", &["sdd1", "sdd2"], "0", true),
        ] {
            let real = sys
                .join("devices")
                .join(if usb { "pci/usb2" } else { "pci/nvme" });
            let disk_dir = real.join(disk);
            for p in parts {
                std::fs::create_dir_all(disk_dir.join(p)).unwrap();
                std::fs::write(disk_dir.join(p).join("size"), "2048\n").unwrap();
            }
            std::fs::write(disk_dir.join("removable"), format!("{removable}\n")).unwrap();
            link(disk_dir.to_str().unwrap(), &sys.join("block").join(disk));
        }
        let table = MOUNTS
            .replace(" /run/media/", &format!(" {}/run/media/", dir.display()))
            .replace(" /mnt/", &format!(" {}/mnt/", dir.display()));
        std::fs::write(dir.join("mounts"), table).unwrap();
        Roots {
            mounts: dir.join("mounts"),
            dev,
            sys,
            user: "david".into(),
        }
    }

    #[test]
    fn a_mount_table_is_parsed_with_its_escapes() {
        let m = parse_mounts(MOUNTS);
        assert_eq!(m[8].dir, Path::new("/run/media/david/SD CARD"));
        assert_eq!(m[8].fstype, "exfat");
    }

    #[test]
    fn shares_are_read_from_their_type_and_source() {
        assert_eq!(
            share_address("cifs", "//NAS/photos").as_deref(),
            Some("smb://nas/photos")
        );
        assert_eq!(
            share_address("nfs4", "nas:/volume1/raw").as_deref(),
            Some("nfs://nas:/volume1/raw")
        );
        assert_eq!(
            share_address("fuse.sshfs", "me@box:/srv/pics").as_deref(),
            Some("sftp://box/srv/pics")
        );
        assert_eq!(share_address("ext4", "/dev/sda1"), None);
        assert_eq!(
            gvfs_address("smb-share:server=nas,share=photos,user=me").as_deref(),
            Some("smb://nas/photos")
        );
        assert_eq!(
            gvfs_address("nfs:host=nas,prefix=%2Fvolume1").as_deref(),
            Some("nfs://nas:/volume1")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_machine_is_scanned_from_injected_roots() {
        let dir = scratch("machine");
        let roots = machine(&dir);
        let card = dir.join("run/media/david/EOS_DIGITAL");
        std::fs::create_dir_all(card.join("DCIM/100CANON")).unwrap();
        let gvfs = dir.join("gvfs");
        std::fs::create_dir_all(gvfs.join("smb-share:server=nas,share=photos")).unwrap();
        std::fs::create_dir_all(gvfs.join("smb-share:server=studio,share=raw")).unwrap();
        let table = std::fs::read_to_string(&roots.mounts)
            .unwrap()
            .replace("/run/user/1000/gvfs", gvfs.to_str().unwrap());
        std::fs::write(&roots.mounts, table).unwrap();

        let found = scan(&roots, "studio-pc");
        let ids: Vec<&str> = found.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "uuid:f00d-cafe",
                "uuid:0b1f-2c3d",
                "weak:SD CARD:1048576",
                "uuid:aaaa-1111",
                "uuid:bbbb-2222",
                "share:smb://nas/photos",
                "share:nfs://nas:/volume1/raw",
                "share:sftp://box/srv/pics",
                // The NAS's share, mounted by the kernel and by gvfs, is one.
                "share:smb://studio/raw",
            ]
        );
        let by_id = |id: &str| found.iter().find(|v| v.id.as_str() == id).unwrap();

        // The system disk: fixed, on this node, labelled.
        let root = by_id("uuid:f00d-cafe");
        assert_eq!(root.kind, VolumeKind::Internal);
        assert_eq!(root.node.as_deref(), Some("studio-pc"));
        assert_eq!(root.name, "fedora");
        assert_eq!(root.drive.as_deref(), Some("nvme0n1"));

        // The card: removable, no node, a DCIM, an ingest source.
        let eos = by_id("uuid:0b1f-2c3d");
        assert_eq!(eos.kind, VolumeKind::Removable);
        assert_eq!(eos.node, None);
        assert!(eos.has_dcim && eos.is_card());
        assert_eq!(eos.roles, [Role::Ingest].into());
        assert_eq!(eos.label.as_deref(), Some("EOS_DIGITAL"));

        // No UUID: weak, labelled by by-label, sized by sysfs (not mounted
        // for real, so statvfs says nothing).
        let sd = by_id("weak:SD CARD:1048576");
        assert_eq!(sd.identified_by, Identity::Weak);
        assert_eq!(sd.capacity, Some(1_048_576));

        // Two partitions of one USB disk: external, the same drive.
        let (b1, b2) = (by_id("uuid:aaaa-1111"), by_id("uuid:bbbb-2222"));
        assert_eq!(b1.kind, VolumeKind::External);
        assert_eq!(b1.node, None);
        assert_eq!(b1.drive.as_deref(), Some("usb-WD_Elements_1234-0:0"));
        assert_eq!(b1.drive, b2.drive);

        let nas = by_id("share:smb://nas/photos");
        assert_eq!(
            (nas.kind, nas.identified_by),
            (VolumeKind::Share, Identity::Share)
        );
        assert_eq!(nas.name, "photos");

        // The import window's filter: the one card.
        let cards = found.cards();
        assert_eq!(
            cards,
            [Card {
                label: "EOS_DIGITAL".into(),
                root: card,
                capacity: eos.capacity,
            }]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Marker over UUID over share over weak.
    #[cfg(unix)]
    #[test]
    fn the_strongest_identity_wins() {
        let dir = scratch("identity");
        let roots = machine(&dir);
        let put = |root: PathBuf, id: &str| {
            std::fs::create_dir_all(root.join(".helios")).unwrap();
            let doc = serde_json::json!({ "id": id, "library": "lib", "created": 1 });
            std::fs::write(marker::path(&root), doc.to_string()).unwrap();
        };
        put(dir.join("run/media/david/Backup-1"), "marker:b1");
        put(dir.join("mnt/nas"), "marker:nas");
        let found = scan(&roots, "studio-pc");
        let how = |id: &str| {
            found
                .iter()
                .find(|v| v.id.as_str() == id)
                .map(|v| v.identified_by)
        };
        // Its UUID and its share address give way to the marker.
        assert_eq!(how("marker:b1"), Some(Identity::Marker));
        assert_eq!(how("uuid:aaaa-1111"), None);
        assert_eq!(how("marker:nas"), Some(Identity::Marker));
        assert_eq!(how("share:smb://nas/photos"), None);
        // Then a UUID, a share, and only then label and size.
        assert_eq!(how("uuid:bbbb-2222"), Some(Identity::Uuid));
        assert_eq!(how("share:nfs://nas:/volume1/raw"), Some(Identity::Share));
        assert_eq!(how("weak:SD CARD:1048576"), Some(Identity::Weak));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_device_mapper_is_traced_to_its_disk() {
        let dir = scratch("dm");
        let sys = dir.join("sys");
        std::fs::create_dir_all(sys.join("block/sda/sda2")).unwrap();
        std::fs::create_dir_all(sys.join("block/dm-0/slaves/sda2")).unwrap();
        assert_eq!(disk_of(&sys, "dm-0").as_deref(), Some("sda"));
        assert_eq!(disk_of(&sys, "sda2").as_deref(), Some("sda"));
        assert_eq!(disk_of(&sys, "sda").as_deref(), Some("sda"));
        assert_eq!(disk_of(&sys, "sdz1"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn media_folders_are_one_level_down() {
        assert!(media_dir(Path::new("/run/media/david/X"), "david"));
        assert!(media_dir(Path::new("/media/david/X"), "david"));
        assert!(media_dir(Path::new("/media/usb0"), "david"));
        assert!(!media_dir(Path::new("/run/media/david/X/inner"), "david"));
        assert!(!media_dir(Path::new("/run/media/other/X"), "david"));
    }
}
