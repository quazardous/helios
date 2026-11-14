//! What is mounted on macOS, as `diskutil info -plist <mount point>` says:
//!
//! - the mounted folders are `/` and those under `/Volumes` (a link there,
//!   such as the startup disk's own name, is skipped);
//! - each one's `diskutil` record gives its volume UUID (its identity),
//!   its name, the disk it lies on, and whether it is removable media
//!   (a card, a stick), an external device or an internal disk;
//! - free space and size come from `statvfs`, as on Linux.
//!
//! The property list is read by [`parse_plist`], a small reader of the
//! XML form `diskutil` writes, and turned into a [`Volume`] by
//! [`volume_of`]: both pure, so their tests read recorded `diskutil`
//! output on any system. `diskutil` runs once per newly mounted volume:
//! the records are kept while a folder stays mounted on the same device.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::detect::{label_of, weak};
use crate::{Identity, Volume, VolumeId, VolumeKind};

/// A value of a property list.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Dict(BTreeMap<String, Value>),
    Array(Vec<Value>),
    String(String),
    Integer(i64),
    Real(f64),
    Bool(bool),
    /// `<data>` and `<date>`, kept as their text.
    Other(String),
}

impl Value {
    /// The entry `key` of a dictionary.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(d) => d.get(key),
            _ => None,
        }
    }

    /// The text of a string entry; `None` for an empty one.
    pub fn text(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::String(s) if !s.trim().is_empty() => Some(s),
            _ => None,
        }
    }

    /// A boolean entry; `false` when absent.
    pub fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some(Value::Bool(true)))
    }

    /// A positive integer entry.
    pub fn size(&self, key: &str) -> Option<u64> {
        match self.get(key)? {
            Value::Integer(n) if *n > 0 => u64::try_from(*n).ok(),
            _ => None,
        }
    }
}

/// An element being read: a dictionary (with the key waiting for its
/// value) or an array.
enum Open {
    Dict(BTreeMap<String, Value>, Option<String>),
    Array(Vec<Value>),
}

/// Reads an XML property list (the root `<plist>`'s one value). `None`
/// when it is not one.
pub fn parse_plist(xml: &str) -> Option<Value> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<Open> = Vec::new();
    let mut text = String::new();
    let mut root = None;
    // Puts a finished value where it belongs: in the dictionary or array
    // being read, else at the root.
    let place =
        |stack: &mut Vec<Open>, root: &mut Option<Value>, value: Value| match stack.last_mut() {
            Some(Open::Dict(map, key)) => {
                if let Some(k) = key.take() {
                    map.insert(k, value);
                }
            }
            Some(Open::Array(items)) => items.push(value),
            None => *root = Some(value),
        };
    loop {
        match reader.read_event().ok()? {
            Event::Start(e) => {
                text.clear();
                match e.name().as_ref() {
                    "dict" => stack.push(Open::Dict(BTreeMap::new(), None)),
                    "array" => stack.push(Open::Array(Vec::new())),
                    _ => {}
                }
            }
            Event::Empty(e) => {
                let value = match e.name().as_ref() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    "dict" => Value::Dict(BTreeMap::new()),
                    "array" => Value::Array(Vec::new()),
                    "string" => Value::String(String::new()),
                    _ => continue,
                };
                place(&mut stack, &mut root, value);
            }
            Event::Text(e) => text.push_str(&e),
            Event::CData(e) => {
                // Escaped again, so the whole text unescapes in one go.
                let raw: &str = &e;
                text.push_str(&quick_xml::escape::escape(raw));
            }
            Event::GeneralRef(e) => {
                text.push('&');
                text.push_str(&e);
                text.push(';');
            }
            Event::End(e) => {
                let content =
                    || -> Option<Cow<'_, str>> { quick_xml::escape::unescape(&text).ok() };
                let value = match e.name().as_ref() {
                    "dict" => match stack.pop()? {
                        Open::Dict(map, _) => Value::Dict(map),
                        Open::Array(_) => return None,
                    },
                    "array" => match stack.pop()? {
                        Open::Array(items) => Value::Array(items),
                        Open::Dict(..) => return None,
                    },
                    "key" => {
                        let key = content()?.into_owned();
                        if let Some(Open::Dict(_, waiting)) = stack.last_mut() {
                            *waiting = Some(key);
                        }
                        text.clear();
                        continue;
                    }
                    "string" => Value::String(content()?.into_owned()),
                    "integer" => Value::Integer(content()?.trim().parse().ok()?),
                    "real" => Value::Real(content()?.trim().parse().ok()?),
                    "data" | "date" => Value::Other(content()?.trim().to_string()),
                    _ => {
                        text.clear();
                        continue;
                    }
                };
                text.clear();
                place(&mut stack, &mut root, value);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    root
}

/// Mount points that hold the system's own volumes (the startup disk's
/// data, preboot, recovery, swap…), never photos the user put there: the
/// user's files are read through `/`.
fn system_mount(mount: &Path) -> bool {
    mount.starts_with("/System/Volumes") || mount.starts_with("/private/var/vm")
}

/// The volume a `diskutil info -plist` record describes, as the record
/// says (no file is read): `None` when it is not mounted or is one of
/// the system's own. `node` is the machine: fixed disks belong to it.
pub fn volume_of(info: &Value, node: &str) -> Option<Volume> {
    let mount = Path::new(info.text("MountPoint")?);
    if system_mount(mount) {
        return None;
    }
    let label = info
        .text("VolumeName")
        .map_or_else(|| label_of(mount), str::to_string);
    let capacity = info.size("TotalSize").or_else(|| info.size("Size"));
    let mut volume = match info.text("VolumeUUID") {
        Some(uuid) => Volume {
            id: VolumeId(format!("uuid:{}", uuid.to_lowercase())),
            identified_by: Identity::Uuid,
            ..weak(&label, capacity)
        },
        None => weak(&label, capacity),
    };
    volume.kind = if info.flag("RemovableMedia") || info.flag("Removable") {
        VolumeKind::Removable
    } else if !info.flag("Internal")
        || info.flag("Ejectable")
        || info.flag("RemovableMediaOrExternalDevice")
    {
        VolumeKind::External
    } else {
        VolumeKind::Internal
    };
    volume.drive = info.text("ParentWholeDisk").map(str::to_string);
    volume.mount = Some(mount.to_path_buf());
    volume.node = volume.kind.is_fixed().then(|| node.to_string());
    Some(volume)
}

#[cfg(target_os = "macos")]
pub(crate) use system::scan;

#[cfg(target_os = "macos")]
mod system {
    use std::collections::{HashMap, HashSet};
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Mutex;

    use super::{parse_plist, volume_of};
    use crate::Volume;
    use crate::detect::{space, with_marker};

    /// The `diskutil` records read so far, by mount point and the device
    /// mounted there: a card swapped for another at the same folder is
    /// read again.
    static RECORDS: Mutex<Option<HashMap<(PathBuf, u64), Option<Volume>>>> = Mutex::new(None);

    /// The folders volumes are mounted on: `/` and the folders of
    /// `/Volumes` (not its links: the startup disk's name points at `/`).
    fn mount_points() -> Vec<PathBuf> {
        let mut found = vec![PathBuf::from("/")];
        if let Ok(entries) = std::fs::read_dir("/Volumes") {
            let mut more: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect();
            more.sort();
            found.extend(more);
        }
        found
    }

    fn diskutil(mount: &Path, node: &str) -> Option<Volume> {
        let out = Command::new("/usr/sbin/diskutil")
            .args(["info", "-plist"])
            .arg(mount)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        volume_of(&parse_plist(&String::from_utf8_lossy(&out.stdout))?, node)
    }

    /// The volumes mounted now.
    pub(crate) fn scan(node: &str) -> Vec<Volume> {
        let mut records = RECORDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let records = records.get_or_insert_with(HashMap::new);
        let mut now = HashMap::new();
        let mut seen = HashSet::new();
        let mut found = Vec::new();
        for dir in mount_points() {
            let Ok(meta) = std::fs::metadata(&dir) else {
                continue;
            };
            let key = (dir.clone(), meta.dev());
            let record = match records.remove(&key) {
                Some(r) => r,
                None => diskutil(&dir, node),
            };
            now.insert(key, record.clone());
            let Some(mut volume) = record else { continue };
            if !seen.insert(volume.id.clone()) {
                continue;
            }
            if let Some((free, total)) = space(&dir) {
                volume.free = Some(free);
                volume.capacity = volume.capacity.or(Some(total));
            }
            volume.has_dcim = dir.join("DCIM").is_dir();
            volume.roles = volume.default_roles();
            found.push(with_marker(volume, &dir));
        }
        *records = now;
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str) -> Value {
        let xml = match name {
            "card" => include_str!("../fixtures/diskutil/card.plist"),
            "internal" => include_str!("../fixtures/diskutil/internal.plist"),
            "untitled" => include_str!("../fixtures/diskutil/untitled.plist"),
            _ => unreachable!(),
        };
        parse_plist(xml).expect("a property list")
    }

    #[test]
    fn a_property_list_is_read_with_its_types_and_escapes() {
        let v = parse_plist(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>A &amp; B</key>
	<string>Tom &amp; Jerry&#39;s</string>
	<key>Empty</key>
	<string></string>
	<key>List</key>
	<array>
		<integer>-3</integer>
		<real>1.5</real>
		<dict/>
		<false/>
	</array>
	<key>Yes</key>
	<true/>
</dict>
</plist>"#,
        )
        .unwrap();
        assert_eq!(v.text("A & B"), Some("Tom & Jerry's"));
        assert_eq!(v.text("Empty"), None);
        assert!(v.flag("Yes"));
        assert_eq!(
            v.get("List"),
            Some(&Value::Array(vec![
                Value::Integer(-3),
                Value::Real(1.5),
                Value::Dict(BTreeMap::new()),
                Value::Bool(false),
            ]))
        );
        assert_eq!(parse_plist("not a plist"), None);
    }

    #[test]
    fn a_card_is_a_removable_volume_known_by_its_uuid() {
        let v = volume_of(&record("card"), "studio-mac").unwrap();
        assert_eq!(v.id.as_str(), "uuid:0e239bc6-f960-3107-89cf-1c97f78bb46b");
        assert_eq!(v.identified_by, Identity::Uuid);
        assert_eq!(v.label.as_deref(), Some("EOS_DIGITAL"));
        assert_eq!(v.name, "EOS_DIGITAL");
        assert_eq!(v.kind, VolumeKind::Removable);
        assert_eq!(v.mount.as_deref(), Some(Path::new("/Volumes/EOS_DIGITAL")));
        assert_eq!(v.drive.as_deref(), Some("disk4"));
        assert_eq!(v.capacity, Some(63_847_940_096));
        // A card moves between machines.
        assert_eq!(v.node, None);
    }

    #[test]
    fn the_startup_disk_is_internal_and_belongs_to_its_node() {
        let v = volume_of(&record("internal"), "studio-mac").unwrap();
        assert_eq!(v.id.as_str(), "uuid:5a3e7d4b-9c2f-4e1a-8b6d-0f1e2d3c4b5a");
        assert_eq!(v.kind, VolumeKind::Internal);
        assert_eq!(v.label.as_deref(), Some("Macintosh HD"));
        assert_eq!(v.mount.as_deref(), Some(Path::new("/")));
        assert_eq!(v.node.as_deref(), Some("studio-mac"));
    }

    #[test]
    fn a_volume_without_a_name_takes_its_folder_s() {
        let v = volume_of(&record("untitled"), "studio-mac").unwrap();
        assert_eq!(v.label.as_deref(), Some("Untitled"));
        assert_eq!(v.kind, VolumeKind::External);
        assert_eq!(v.id.as_str(), "uuid:9f1b6c2a-3d4e-3a5b-8c7d-6e5f4a3b2c1d");
        assert_eq!(v.node, None);
    }

    #[test]
    fn a_volume_without_a_uuid_is_weak_and_system_volumes_are_skipped() {
        let mut map = match record("card") {
            Value::Dict(m) => m,
            _ => unreachable!(),
        };
        map.remove("VolumeUUID");
        let v = volume_of(&Value::Dict(map.clone()), "n").unwrap();
        assert_eq!(v.identified_by, Identity::Weak);
        assert_eq!(v.id.as_str(), "weak:EOS_DIGITAL:63847940096");

        map.insert(
            "MountPoint".into(),
            Value::String("/System/Volumes/Data".into()),
        );
        assert_eq!(volume_of(&Value::Dict(map.clone()), "n"), None);
        map.remove("MountPoint");
        assert_eq!(volume_of(&Value::Dict(map), "n"), None);
    }
}
