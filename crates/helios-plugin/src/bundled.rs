//! The plugins Helios ships, its official importers and sources
//! (`plugins/importers/`, `plugins/sources/` in its repository): Rust programs built with
//! Helios, so nothing else needs to be installed.
//!
//! Each one's program is built beside Helios's own
//! (`helios-darktable-library` next to `helios-gui`) and ships with it;
//! its manifest and catalogues are compiled into Helios. [`add_to`]
//! writes them out once, with a copy of the program, into a folder of
//! their own (named by their contents and the program's size and date, so
//! a new build gets a fresh copy and an unchanged one reuses it) that the
//! broker reads as built-in. A plugin an installed built-in folder
//! (`<exe>/../share/helios/plugins`) already holds is not written: a
//! packaged Helios's copy wins.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use crate::broker::Scope;
use crate::manifest::MANIFEST;

/// One file of a bundled plugin: its path in the plugin's folder (`/`
/// between levels) and its contents.
#[derive(Clone, Copy, Debug)]
pub struct File {
    pub path: &'static str,
    pub contents: &'static str,
}

/// A bundled plugin: the name of its folder, its program (the manifest's
/// command, built beside Helios) and its other files.
#[derive(Clone, Copy, Debug)]
pub struct Bundled {
    pub name: &'static str,
    pub program: &'static str,
    pub files: &'static [File],
}

macro_rules! file {
    ($plugin:literal, $path:literal) => {
        file!("importers", $plugin, $path)
    };
    ($category:literal, $plugin:literal, $path:literal) => {
        File {
            path: $path,
            contents: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../plugins/",
                $category,
                "/",
                $plugin,
                "/",
                $path
            )),
        }
    };
}

/// The plugins Helios ships.
pub const PLUGINS: &[Bundled] = &[
    Bundled {
        name: "darktable-library",
        program: "helios-darktable-library",
        files: &[
            file!("darktable-library", "helios-plugin.toml"),
            file!("darktable-library", "i18n/en/plugin.ftl"),
            file!("darktable-library", "i18n/fr/plugin.ftl"),
        ],
    },
    Bundled {
        name: "lightroom-catalog",
        program: "helios-lightroom-catalog",
        files: &[
            file!("lightroom-catalog", "helios-plugin.toml"),
            file!("lightroom-catalog", "i18n/en/plugin.ftl"),
            file!("lightroom-catalog", "i18n/fr/plugin.ftl"),
        ],
    },
    Bundled {
        name: "google-photos",
        program: "helios-google-photos",
        files: &[
            file!("sources", "google-photos", "helios-plugin.toml"),
            file!("sources", "google-photos", "i18n/en/plugin.ftl"),
            file!("sources", "google-photos", "i18n/fr/plugin.ftl"),
        ],
    },
];

/// A marker written last: a folder without it was left half written.
const COMPLETE: &str = ".complete";

/// Where the bundled plugins are written by default: Helios's cache
/// folder (`$XDG_CACHE_HOME/helios/plugins`, or `~/.cache/helios/plugins`;
/// `%LOCALAPPDATA%\helios\plugins` on Windows). They are rewritten
/// whenever it is cleared.
pub fn default_root() -> Option<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))?;
    Some(cache.join("helios").join("plugins"))
}

/// Where the bundled plugins' programs ship: beside the running
/// executable.
pub fn programs_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
}

/// A program's file name on this system (`.exe` on Windows).
pub fn program_file(program: &str) -> String {
    format!("{program}{}", std::env::consts::EXE_SUFFIX)
}

/// What [`add_to`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Added {
    /// The folder added to the plugin folders, if any.
    pub dir: Option<PathBuf>,
    /// Bundled plugins left out because their program is not beside
    /// Helios (a build of `helios-gui` alone).
    pub without_program: Vec<&'static str>,
}

/// Adds to `dirs` (as [`crate::default_dirs`] gives them, broadest first)
/// the folder the bundled plugins are written to under `root`, as
/// built-in, right after the installed built-in folders. Their programs
/// are taken from `programs`; a plugin whose program is not there, or
/// that an installed built-in folder already holds, is left out. Nothing
/// is added when none is left.
pub fn add_to(dirs: &mut Vec<(Scope, PathBuf)>, root: &Path, programs: &Path) -> io::Result<Added> {
    let installed = |name: &str| {
        dirs.iter()
            .any(|(scope, dir)| *scope == Scope::Builtin && dir.join(name).join(MANIFEST).is_file())
    };
    let mut added = Added::default();
    let mut plugins = Vec::new();
    for p in PLUGINS.iter().filter(|p| !installed(p.name)) {
        let program = programs.join(program_file(p.program));
        if program.is_file() {
            plugins.push((p, program));
        } else {
            added.without_program.push(p.name);
        }
    }
    if plugins.is_empty() {
        return Ok(added);
    }
    let dir = unpack(root, &plugins)?;
    let at = dirs
        .iter()
        .rposition(|(scope, _)| *scope == Scope::Builtin)
        .map_or(0, |i| i + 1);
    dirs.insert(at, (Scope::Builtin, dir.clone()));
    added.dir = Some(dir);
    Ok(added)
}

/// Writes `plugins` (each with its program's path) under
/// `root/<digest>/<name>/`, unless a complete copy is there already;
/// returns `root/<digest>`. Written beside it first, then renamed into
/// place, so two Helios starting together never read a half-written copy.
fn unpack(root: &Path, plugins: &[(&Bundled, PathBuf)]) -> io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = root.join(digest(plugins)?);
    if dir.join(COMPLETE).is_file() {
        return Ok(dir);
    }
    std::fs::create_dir_all(root)?;
    let tmp = root.join(format!(
        ".unpacking-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let written = (|| {
        for (plugin, program) in plugins {
            let into = tmp.join(plugin.name);
            for file in plugin.files {
                let path = file
                    .path
                    .split('/')
                    .fold(into.clone(), |p, part| p.join(part));
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, file.contents)?;
            }
            // the permissions come along: it stays executable
            std::fs::copy(program, into.join(program_file(plugin.program)))?;
        }
        std::fs::write(tmp.join(COMPLETE), "")?;
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::rename(&tmp, &dir)
    })();
    if written.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
        // another Helios may have written the same copy meanwhile
        if dir.join(COMPLETE).is_file() {
            return Ok(dir);
        }
    }
    written.map(|()| dir)
}

/// The copy's name: Helios's version, every file, and each program's
/// size and date (FNV-1a, 64 bits).
fn digest(plugins: &[(&Bundled, PathBuf)]) -> io::Result<String> {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes.iter().chain([0u8].iter()) {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    eat(env!("CARGO_PKG_VERSION").as_bytes());
    for (plugin, program) in plugins {
        eat(plugin.name.as_bytes());
        for file in plugin.files {
            eat(file.path.as_bytes());
            eat(file.contents.as_bytes());
        }
        let meta = std::fs::metadata(program)?;
        let date = meta
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        eat(format!("{} {date}", meta.len()).as_bytes());
    }
    Ok(format!("{}-{h:016x}", env!("CARGO_PKG_VERSION")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Broker, Policy};

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("helios-bundled-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Every manifest and catalogue of each shipped plugin is bundled:
    /// one added to its folder and not to [`PLUGINS`] would be missing
    /// from a release.
    #[test]
    fn every_file_of_the_shipped_plugins_is_bundled() {
        let plugins = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        let mut dirs = std::collections::BTreeMap::new();
        for category in ["importers", "sources"] {
            for e in std::fs::read_dir(plugins.join(category)).unwrap() {
                let e = e.unwrap();
                dirs.insert(e.file_name().to_string_lossy().into_owned(), e.path());
            }
        }
        let mut bundled: Vec<&str> = PLUGINS.iter().map(|p| p.name).collect();
        bundled.sort_unstable();
        assert_eq!(dirs.keys().map(String::as_str).collect::<Vec<_>>(), bundled);
        for plugin in PLUGINS {
            let dir = &dirs[plugin.name];
            let mut files = vec![MANIFEST.to_string()];
            for locale in std::fs::read_dir(dir.join("i18n")).unwrap() {
                let locale = locale.unwrap().path();
                for f in std::fs::read_dir(&locale).unwrap() {
                    files.push(format!(
                        "i18n/{}/{}",
                        locale.file_name().unwrap().to_string_lossy(),
                        f.unwrap().file_name().to_string_lossy()
                    ));
                }
            }
            files.sort();
            let mut listed: Vec<String> = plugin.files.iter().map(|f| f.path.into()).collect();
            listed.sort();
            assert_eq!(files, listed, "{}", plugin.name);
            // The manifest runs the program.
            let manifest = plugin.files[0].contents;
            assert!(
                manifest.contains(&format!("command = [\"{}\"]", plugin.program)),
                "{}",
                plugin.name
            );
        }
    }

    /// Written out with a stand-in program, the bundled plugins are
    /// accepted by this Helios as built-in (their `compat.helios` range
    /// follows the release), once.
    #[test]
    fn the_bundled_plugins_unpack_once_and_are_accepted() {
        let root = scratch("unpack");
        let programs = root.join("bin");
        std::fs::create_dir_all(&programs).unwrap();
        // Without its program, a plugin is left out.
        let mut dirs = vec![(Scope::User, root.join("user"))];
        let added = add_to(&mut dirs, &root.join("plugins"), &programs).unwrap();
        assert_eq!(added.dir, None);
        assert_eq!(
            added.without_program,
            ["darktable-library", "lightroom-catalog", "google-photos"]
        );
        assert_eq!(dirs.len(), 1);
        for plugin in PLUGINS {
            std::fs::write(programs.join(program_file(plugin.program)), "stand-in").unwrap();
        }
        let dir = add_to(&mut dirs, &root.join("plugins"), &programs)
            .unwrap()
            .dir
            .unwrap();
        assert_eq!(dirs[0], (Scope::Builtin, dir.clone()));
        assert!(
            dir.join("darktable-library")
                .join(program_file("helios-darktable-library"))
                .is_file()
        );
        let mut broker = Broker::new(Policy::default());
        broker.discover(&dirs);
        assert!(broker.rejected().is_empty(), "{:?}", broker.rejected());
        assert!(broker.importers().iter().any(|(p, i)| p
            == "io.github.quazardous.helios.darktable-library"
            && i.id == "darktable-library"));
        // A second start reuses the copy.
        let stamp = std::fs::metadata(dir.join(COMPLETE))
            .unwrap()
            .modified()
            .unwrap();
        let mut again = Vec::new();
        let added = add_to(&mut again, &root.join("plugins"), &programs).unwrap();
        assert_eq!(added.dir, Some(dir.clone()));
        let after = std::fs::metadata(dir.join(COMPLETE))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(stamp, after);
        // An installed built-in copy wins: nothing is written.
        let installed = root.join("installed");
        for plugin in PLUGINS {
            std::fs::create_dir_all(installed.join(plugin.name)).unwrap();
            std::fs::write(installed.join(plugin.name).join(MANIFEST), "").unwrap();
        }
        let mut dirs = vec![(Scope::Builtin, installed.clone())];
        let added = add_to(&mut dirs, &root.join("other"), &programs).unwrap();
        assert_eq!(added, Added::default());
        assert_eq!(dirs.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
