//! What is mounted on Windows, as the volume API says, one drive letter
//! at a time:
//!
//! - `GetLogicalDriveStringsW`: the drive roots (`C:\`, `E:\`…);
//! - `GetDriveTypeW`: removable (a card, a stick) or fixed (a disk);
//!   network, optical and RAM drives are not listed for now;
//! - `GetVolumeInformationW`: the filesystem's serial number (its
//!   identity), its label and its type;
//! - `GetVolumeNameForVolumeMountPointW`: the volume's GUID path, the
//!   identity of a volume without a serial number;
//! - `GetDiskFreeSpaceExW`: its size and free space.
//!
//! The answers are gathered in a [`Drive`], turned into a [`Volume`] by
//! [`volume_of`]: pure, so its tests run on any system. A FAT or exFAT
//! card's serial number is the one Linux lists as its UUID (`0B1F-2C3D`),
//! so a card has the same id on both.

use std::path::{Path, PathBuf};

use crate::detect::weak;
use crate::{Identity, Volume, VolumeId, VolumeKind};

/// What `GetDriveTypeW` says of a drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveType {
    Removable,
    Fixed,
    Remote,
    Optical,
    Ram,
    Unknown,
}

impl DriveType {
    /// From `GetDriveTypeW`'s answer (`DRIVE_REMOVABLE` is 2, …).
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            2 => Self::Removable,
            3 => Self::Fixed,
            4 => Self::Remote,
            5 => Self::Optical,
            6 => Self::Ram,
            _ => Self::Unknown,
        }
    }
}

/// One drive letter, as the volume API describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    /// Its root, `E:\`.
    pub root: PathBuf,
    pub kind: DriveType,
    /// The filesystem's serial number; `None` (or 0) when it has none.
    pub serial: Option<u32>,
    /// The filesystem's label, empty when it has none.
    pub label: String,
    /// The filesystem's type (`FAT32`, `exFAT`, `NTFS`).
    pub filesystem: String,
    /// The volume's GUID path, `\\?\Volume{…}\`.
    pub guid: Option<String>,
    /// Its size in bytes.
    pub total: Option<u64>,
    /// Its free bytes.
    pub free: Option<u64>,
}

/// The GUID of a volume's GUID path, lower case: `\\?\Volume{ABC…}\`
/// gives `abc…`.
fn guid_of(path: &str) -> Option<String> {
    let start = path.find('{')? + 1;
    let end = start + path[start..].find('}')?;
    let guid = &path[start..end];
    (!guid.is_empty()).then(|| guid.to_lowercase())
}

/// A drive root as a name: `E:\` gives `E:`.
fn letter_of(root: &Path) -> String {
    root.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_string()
}

/// The volume a drive is, as the volume API said (no file is read):
/// `None` for a drive that is not a disk or a card (a network share, an
/// optical drive) or has no medium. `node` is the machine: fixed disks
/// belong to it.
pub fn volume_of(drive: &Drive, node: &str) -> Option<Volume> {
    let kind = match drive.kind {
        DriveType::Removable => VolumeKind::Removable,
        DriveType::Fixed => VolumeKind::Internal,
        _ => return None,
    };
    let label = if drive.label.trim().is_empty() {
        letter_of(&drive.root)
    } else {
        drive.label.clone()
    };
    let identity = match (drive.serial.filter(|s| *s != 0), &drive.guid) {
        // Written as Linux lists a FAT or exFAT filesystem's UUID.
        (Some(serial), _) => Some(format!("{:04x}-{:04x}", serial >> 16, serial & 0xffff)),
        (None, Some(path)) => guid_of(path),
        (None, None) => None,
    };
    let mut volume = match identity {
        Some(id) => Volume {
            id: VolumeId(format!("uuid:{id}")),
            identified_by: Identity::Uuid,
            ..weak(&label, drive.total)
        },
        None => weak(&label, drive.total),
    };
    volume.kind = kind;
    volume.free = drive.free;
    volume.mount = Some(drive.root.clone());
    volume.node = kind.is_fixed().then(|| node.to_string());
    Some(volume)
}

#[cfg(windows)]
pub(crate) use system::{scan, space};

#[cfg(windows)]
#[allow(unsafe_code)] // the Win32 volume API: plain values and buffers this module owns
mod system {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};

    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
        GetVolumeNameForVolumeMountPointW,
    };
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SEM_FAILCRITICALERRORS, SetThreadErrorMode,
    };

    use super::{Drive, DriveType, volume_of};
    use crate::Volume;
    use crate::detect::with_marker;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain([0]).collect()
    }

    fn string(buf: &[u16]) -> String {
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        OsString::from_wide(&buf[..end])
            .to_string_lossy()
            .into_owned()
    }

    /// The drive roots, `C:\` first.
    fn roots() -> Vec<PathBuf> {
        let mut buf = vec![0u16; 512];
        // SAFETY: the buffer's length is passed with it.
        let n = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
        if n == 0 || n > buf.len() {
            return Vec::new();
        }
        buf[..n]
            .split(|c| *c == 0)
            .filter(|s| !s.is_empty())
            .map(|s| PathBuf::from(OsString::from_wide(s)))
            .collect()
    }

    /// Free and total bytes of the volume holding `dir`.
    pub(crate) fn space(dir: &Path) -> Option<(u64, u64)> {
        let path = wide(dir);
        let (mut free, mut total) = (0u64, 0u64);
        // SAFETY: a NUL-terminated path and two u64 out-values; the third
        // is optional.
        let ok = unsafe {
            GetDiskFreeSpaceExW(path.as_ptr(), &mut free, &mut total, std::ptr::null_mut())
        };
        (ok != 0).then_some((free, total))
    }

    fn drive(root: &Path) -> Option<Drive> {
        let path = wide(root);
        // SAFETY: a NUL-terminated path.
        let kind = DriveType::from_raw(unsafe { GetDriveTypeW(path.as_ptr()) });
        if !matches!(kind, DriveType::Removable | DriveType::Fixed) {
            return None;
        }
        let mut label = [0u16; 261];
        let mut filesystem = [0u16; 261];
        let mut serial = 0u32;
        // SAFETY: each buffer's length is passed with it; the out-values
        // the call does not need are null.
        let ok = unsafe {
            GetVolumeInformationW(
                path.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                &mut serial,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                filesystem.as_mut_ptr(),
                filesystem.len() as u32,
            )
        };
        // No medium (an empty card reader): no volume.
        if ok == 0 {
            return None;
        }
        let mut guid = [0u16; 64];
        // SAFETY: a NUL-terminated root ending in a backslash, and the
        // buffer's length.
        let has_guid = unsafe {
            GetVolumeNameForVolumeMountPointW(path.as_ptr(), guid.as_mut_ptr(), guid.len() as u32)
        } != 0;
        let space = space(root);
        Some(Drive {
            root: root.to_path_buf(),
            kind,
            serial: Some(serial),
            label: string(&label),
            filesystem: string(&filesystem),
            guid: has_guid.then(|| string(&guid)),
            total: space.map(|(_, t)| t),
            free: space.map(|(f, _)| f),
        })
    }

    /// The volumes mounted now.
    pub(crate) fn scan(node: &str) -> Vec<Volume> {
        // An empty card reader must not open "There is no disk in the
        // drive" while it is asked about.
        let mut before = 0;
        // SAFETY: plain values.
        unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, &mut before) };
        let found = roots()
            .iter()
            .filter_map(|root| drive(root))
            .filter_map(|d| {
                let mut volume = volume_of(&d, node)?;
                volume.has_dcim = d.root.join("DCIM").is_dir();
                volume.roles = volume.default_roles();
                Some(with_marker(volume, &d.root))
            })
            .collect();
        // SAFETY: plain values.
        unsafe { SetThreadErrorMode(before, std::ptr::null_mut()) };
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives as the volume API answered them on a laptop with a card in
    /// its reader.
    fn drives() -> [Drive; 3] {
        [
            Drive {
                root: r"C:\".into(),
                kind: DriveType::from_raw(3),
                serial: Some(0x5a3e_7d4b),
                label: "Windows".into(),
                filesystem: "NTFS".into(),
                guid: Some(r"\\?\Volume{6b29fc40-ca47-1067-b31d-00dd010662da}\".into()),
                total: Some(510_770_802_688),
                free: Some(201_326_592_000),
            },
            Drive {
                root: r"E:\".into(),
                kind: DriveType::from_raw(2),
                serial: Some(0x0b1f_2c3d),
                label: "EOS_DIGITAL".into(),
                filesystem: "exFAT".into(),
                guid: Some(r"\\?\Volume{0A1B2C3D-0000-0000-0000-100000000000}\".into()),
                total: Some(63_847_940_096),
                free: Some(41_239_162_880),
            },
            Drive {
                root: r"F:\".into(),
                kind: DriveType::from_raw(2),
                serial: Some(0),
                label: String::new(),
                filesystem: "FAT32".into(),
                guid: Some(r"\\?\Volume{9F1B6C2A-3D4E-3A5B-8C7D-6E5F4A3B2C1D}\".into()),
                total: Some(31_914_983_424),
                free: None,
            },
        ]
    }

    #[test]
    fn a_card_is_removable_and_known_by_its_serial_as_on_linux() {
        let v = volume_of(&drives()[1], "studio-pc").unwrap();
        // Linux lists the same exFAT serial as /dev/disk/by-uuid/0B1F-2C3D.
        assert_eq!(v.id.as_str(), "uuid:0b1f-2c3d");
        assert_eq!(v.identified_by, Identity::Uuid);
        assert_eq!(v.kind, VolumeKind::Removable);
        assert_eq!(v.label.as_deref(), Some("EOS_DIGITAL"));
        assert_eq!(v.mount.as_deref(), Some(Path::new(r"E:\")));
        assert_eq!(v.capacity, Some(63_847_940_096));
        assert_eq!(v.free, Some(41_239_162_880));
        assert_eq!(v.node, None);
    }

    #[test]
    fn a_fixed_disk_is_internal_and_belongs_to_its_node() {
        let v = volume_of(&drives()[0], "studio-pc").unwrap();
        assert_eq!(v.id.as_str(), "uuid:5a3e-7d4b");
        assert_eq!(v.kind, VolumeKind::Internal);
        assert_eq!(v.node.as_deref(), Some("studio-pc"));
    }

    #[test]
    fn a_volume_without_label_or_serial_takes_its_letter_and_guid() {
        let v = volume_of(&drives()[2], "studio-pc").unwrap();
        assert_eq!(v.label.as_deref(), Some("F:"));
        assert_eq!(v.name, "F:");
        assert_eq!(v.id.as_str(), "uuid:9f1b6c2a-3d4e-3a5b-8c7d-6e5f4a3b2c1d");
        assert_eq!(v.identified_by, Identity::Uuid);

        let mut bare = drives()[2].clone();
        bare.guid = None;
        let v = volume_of(&bare, "studio-pc").unwrap();
        assert_eq!(v.identified_by, Identity::Weak);
        assert_eq!(v.id.as_str(), "weak:F::31914983424");
    }

    #[test]
    fn shares_and_optical_drives_are_not_listed() {
        for raw in [4, 5, 6, 0, 1] {
            let mut d = drives()[1].clone();
            d.kind = DriveType::from_raw(raw);
            assert_eq!(volume_of(&d, "n"), None, "drive type {raw}");
        }
    }
}
