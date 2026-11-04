//! The user's library of composites (`<config>/composites/`): the
//! macros published from a workspace (`macro.publish`, see
//! `docs/design/macros.md`, "Saving"), offered to every workspace after
//! the composites Helios ships.
//!
//! A library file is never rewritten: a version published is the file
//! that stays, and a change to the macro is published as its next
//! version, a new file. Publishing writes outside the workspace, so no
//! undo takes it back; removing the file from the folder does.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};

use crate::catalogue::Catalogue;
use crate::format::{COMPOSITE_EXTENSION, CompositeFile, CompositeRef};

/// The library's folder under the configuration directory.
pub const DIR: &str = "composites";

/// `<config>/composites`.
pub fn dir(config: &Path) -> PathBuf {
    config.join(DIR)
}

/// Where the library keeps the composite `r`:
/// `<config>/composites/<id>@<version>.helios-composite.json`.
pub fn path_of(config: &Path, r: &CompositeRef) -> PathBuf {
    dir(config).join(format!("{r}{COMPOSITE_EXTENSION}"))
}

/// The library's files, in file-name order; one that does not read is
/// its path and why. None when the folder is not there.
pub fn read(config: &Path) -> Vec<Result<CompositeFile, String>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir(config))
        .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    paths.retain(|p| p.to_string_lossy().ends_with(COMPOSITE_EXTENSION) && p.is_file());
    paths.sort();
    paths.iter().map(|p| read_file(p)).collect()
}

fn read_file(path: &Path) -> Result<CompositeFile, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file =
        crate::bundle::parse_composite(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name != file.file_name() {
        return Err(format!(
            "{}: holds `{}`, whose file is named {}",
            path.display(),
            file.reference(),
            file.file_name()
        ));
    }
    Ok(file)
}

/// `catalogue` with the library's files it lacks, after what it holds:
/// a shipped composite is never replaced by a library file of the same
/// id and version. Files that do not read are left out.
pub fn with_library(mut catalogue: Catalogue, files: &[CompositeFile]) -> Catalogue {
    for f in files {
        if catalogue.get_composite(&f.reference()).is_none() {
            catalogue.add_composite(f.clone());
        }
    }
    catalogue
}

/// The library's files that read.
pub fn files(config: &Path) -> Vec<CompositeFile> {
    read(config).into_iter().filter_map(Result::ok).collect()
}

/// The ids of the composites Helios ships: never published.
fn shipped_ids() -> BTreeSet<String> {
    crate::shipped::composites()
        .into_iter()
        .map(|f| f.id)
        .collect()
}

/// What publishing `files` (a macro's file first, then the composites it
/// nests that the library needs) does, without writing: each file's
/// path and whether it is there already. Refused for a shipped
/// composite, for a file whose version the library holds with other
/// contents (a file is never rewritten), and when the first file is in
/// the library already.
pub fn plan(config: &Path, files: &[CompositeFile]) -> Result<Vec<(PathBuf, bool)>, String> {
    let first = files.first().ok_or("nothing to publish")?;
    let shipped = shipped_ids();
    let mut out = Vec::new();
    for f in files {
        let r = f.reference();
        if shipped.contains(&f.id) {
            return Err(format!(
                "`{r}` is shipped with Helios: every workspace has it already"
            ));
        }
        let path = path_of(config, &r);
        let there = if path.exists() {
            let held = read_file(&path)?;
            if &held != f {
                return Err(format!(
                    "your library holds another `{r}` ({}): a published file is never rewritten; change the macro (a new version) or make it under another name",
                    path.display()
                ));
            }
            true
        } else {
            false
        };
        out.push((path, there));
    }
    if out[0].1 {
        return Err(format!(
            "`{}` is in your library already ({}): a change to the macro is published as its next version",
            first.reference(),
            out[0].0.display()
        ));
    }
    Ok(out)
}

/// Publishes `files` as [`plan`] says, or only says what it would do
/// (`dry_run`). Each file is written new, never over another. The answer:
/// the macro's `composite` and `file`, every file `written` (or to be),
/// those `already` there, `undoable: false`.
pub fn publish(config: &Path, files: &[CompositeFile], dry_run: bool) -> Result<Json, String> {
    let planned = plan(config, files)?;
    let mut written = Vec::new();
    let mut already = Vec::new();
    for ((path, there), f) in planned.iter().zip(files) {
        if *there {
            already.push(path.display().to_string());
            continue;
        }
        if !dry_run {
            write_new(path, f)?;
        }
        written.push(path.display().to_string());
    }
    Ok(json!({
        "composite": files[0].reference(),
        "file": planned[0].0.display().to_string(),
        "library": dir(config).display().to_string(),
        "written": written,
        "already": already,
        "undoable": false,
        "dry_run": dry_run,
    }))
}

/// Writes `file` at `path`, refused when a file is there (never over
/// another, even one written meanwhile).
fn write_new(path: &Path, file: &CompositeFile) -> Result<(), String> {
    let e = |e: std::io::Error| format!("{}: {e}", path.display());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(e)?;
    }
    let text = serde_json::to_string_pretty(file).map_err(|e| format!("composite: {e}"))?;
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(e)?;
    out.write_all(format!("{text}\n").as_bytes()).map_err(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("helios-library-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn look(version: u32) -> CompositeFile {
        let mut f = crate::shipped::composites().remove(0);
        f.id = "user.look".into();
        f.name = "Look".into();
        f.version = version;
        f
    }

    #[test]
    fn a_published_file_is_written_once_and_read_back() {
        let config = temp("once");
        let dry = publish(&config, &[look(1)], true).unwrap();
        assert_eq!(dry["dry_run"], true);
        assert!(!path_of(&config, &look(1).reference()).exists());
        let done = publish(&config, &[look(1)], false).unwrap();
        assert_eq!(done["undoable"], false);
        assert_eq!(files(&config), vec![look(1)]);
        // Twice: refused, the file as it was.
        let e = publish(&config, &[look(1)], false).unwrap_err();
        assert!(e.contains("in your library already"), "{e}");
        // Another file at that version: refused, never rewritten.
        let mut other = look(1);
        other.description = "another".into();
        let e = publish(&config, &[other], false).unwrap_err();
        assert!(e.contains("never rewritten"), "{e}");
        assert_eq!(files(&config), vec![look(1)]);
        // The next version: a new file beside it.
        publish(&config, &[look(2)], false).unwrap();
        assert_eq!(files(&config), vec![look(1), look(2)]);
        let _ = std::fs::remove_dir_all(&config);
    }

    #[test]
    fn shipped_composites_are_not_published_and_stay_first() {
        let config = temp("shipped");
        let shipped = crate::shipped::composites().remove(0);
        let e = publish(&config, std::slice::from_ref(&shipped), false).unwrap_err();
        assert!(e.contains("shipped with Helios"), "{e}");
        let mut c = Catalogue::new();
        c.add_composite(shipped.clone());
        let mut copy = shipped.clone();
        copy.description = "a copy".into();
        let c = with_library(c, &[copy, look(1)]);
        assert_eq!(c.get_composite(&shipped.reference()), Some(&shipped));
        assert!(c.get_composite(&look(1).reference()).is_some());
        let _ = std::fs::remove_dir_all(&config);
    }

    #[test]
    fn a_file_misnamed_in_the_library_is_reported() {
        let config = temp("misnamed");
        std::fs::create_dir_all(dir(&config)).unwrap();
        let text = serde_json::to_string(&look(1)).unwrap();
        std::fs::write(
            dir(&config).join("user.other@1.helios-composite.json"),
            text,
        )
        .unwrap();
        let read = read(&config);
        assert_eq!(read.len(), 1);
        assert!(
            read[0]
                .as_ref()
                .unwrap_err()
                .contains("whose file is named")
        );
        assert!(files(&config).is_empty());
        let _ = std::fs::remove_dir_all(&config);
    }
}
